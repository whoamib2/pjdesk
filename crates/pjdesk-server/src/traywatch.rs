//! If a tray open is not answered, replace this window process.
//!
//! The tray thread only records the open. It does not call `ShowWindow` and
//! it does not wait for the window. The window answers from its own thread.
//! If that answer does not arrive, this module starts a new window and ends
//! this process. The service is a different process and is left running.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

static OPEN_GEN: AtomicU64 = AtomicU64::new(0);
static ACK_GEN: AtomicU64 = AtomicU64::new(0);
static REPLACING: AtomicBool = AtomicBool::new(false);

/// The tray thread records an open and returns. It does not wait.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn arm() {
    let gen = OPEN_GEN.fetch_add(1, Ordering::SeqCst).saturating_add(1);
    let spawned = std::thread::Builder::new().name("tray-watch".into()).spawn(move || {
        std::thread::sleep(Duration::from_millis(pjdesk_proto::traywatch::ANSWER_WAIT_MS));
        let open = OPEN_GEN.load(Ordering::SeqCst);
        let ack = ACK_GEN.load(Ordering::SeqCst);
        if !pjdesk_proto::traywatch::should_replace(gen, open, ack) {
            return;
        }
        if REPLACING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }
        if !replace() {
            REPLACING.store(false, Ordering::SeqCst);
        }
    });
    if spawned.is_err() {
        tracing::info!("Tray: could not start the watch for PJDesk Server.");
    }
}

/// The window thread calls this as soon as it handles the open.
pub fn ack() {
    let open = OPEN_GEN.load(Ordering::SeqCst);
    let prev = ACK_GEN.swap(open, Ordering::SeqCst);
    if open != 0 && prev < open {
        tracing::info!("The PJDesk Server window answered the tray open.");
    }
}

/// A replacement waits until the frozen window process has left, then
/// takes the single-instance lock. Called before that lock is acquired.
pub fn wait_for_replaced_process() {
    let Ok(raw) = std::env::var("PJDESK_REPLACE_PID") else { return };
    std::env::remove_var("PJDESK_REPLACE_PID");
    let Ok(pid) = raw.parse::<u32>() else { return };
    if pid == 0 || pid == std::process::id() {
        return;
    }
    #[cfg(windows)]
    wait_windows(pid);
}

fn replace() -> bool {
    let pid = std::process::id();
    let line = format!(
        "The PJDesk Server window did not answer a tray open within 2 seconds. Replacing process {pid}. The service is left running."
    );
    tracing::info!("{line}");
    append_log(&line);
    if !spawn_replacement(pid) {
        return false;
    }
    kill_self()
}

fn log_path() -> std::path::PathBuf {
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("PJDesk");
    dir.join("pjdesk-server.log")
}

fn append_log(line: &str) {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{line}");
    let _ = file.flush();
}

fn spawn_replacement(pid: u32) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        append_log("Could not find PJDesk Server to open a new window.");
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        // No --tray. The new process shows the window. The service is not started here.
        let mut cmd = std::process::Command::new(&exe);
        cmd.env("PJDESK_REPLACE_PID", pid.to_string());
        cmd.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP);
        if let Ok(child) = cmd.spawn() {
            let line = format!("Started a new PJDesk Server window (pid {}).", child.id());
            tracing::info!("{line}");
            append_log(&line);
            return true;
        }
        let mut again = std::process::Command::new(&exe);
        again.env("PJDESK_REPLACE_PID", pid.to_string());
        again.creation_flags(DETACHED_PROCESS);
        match again.spawn() {
            Ok(child) => {
                let line = format!("Started a new PJDesk Server window (pid {}).", child.id());
                tracing::info!("{line}");
                append_log(&line);
                true
            }
            Err(err) => {
                let line = format!("Could not start a new PJDesk Server window: {err}");
                tracing::info!("{line}");
                append_log(&line);
                false
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, pid);
        false
    }
}

fn kill_self() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    std::process::exit(0);
}

#[cfg(windows)]
fn wait_windows(pid: u32) {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) {
            Ok(handle) => {
                let wait = WaitForSingleObject(handle, 5_000);
                if wait == WAIT_OBJECT_0 {
                    tracing::info!("The previous PJDesk Server window process {pid} has exited. Opening a new window.");
                } else {
                    tracing::info!(
                        "The previous PJDesk Server window process {pid} is still running. Ending it so this window can open."
                    );
                    let _ = TerminateProcess(handle, 1);
                    let _ = WaitForSingleObject(handle, 2_000);
                }
                let _ = CloseHandle(handle);
            }
            Err(_) => {
                tracing::info!("The previous PJDesk Server window process {pid} is already gone. Opening a new window.");
            }
        }
    }
}
