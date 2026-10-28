//! System-tray icon for PJDesk Server.
//!
//! This process never draws the window. A click starts `pjdesk-server.exe --window`
//! or brings that process forward. Closing the window exits the window process.
//! The tray keeps running.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::core::w;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, OpenMutexW, ReleaseMutex, SetEvent, WaitForSingleObject,
    EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

const TRAY_MUTEX: windows::core::PCWSTR = w!("Local\\PJDeskServerTray");
const TRAY_SHOW: windows::core::PCWSTR = w!("Local\\PJDeskServerTrayShow");
const WINDOW_MUTEX: windows::core::PCWSTR = w!("Local\\PJDeskServerWindow");
const WINDOW_SHOW: windows::core::PCWSTR = w!("Local\\PJDeskServerWindowShow");
const WINDOW_QUIT: windows::core::PCWSTR = w!("Local\\PJDeskServerWindowQuit");
const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const PREF_KEY: windows::core::PCWSTR = w!("Software\\PJDeskServer");
const RUN_VALUE: windows::core::PCWSTR = w!("PJDesk Server");
const PREF_VALUE: windows::core::PCWSTR = w!("TrayAtLogin");
const WINDOW_TITLE: &str = "PJDesk Server";

/// What the user asked for via the tray. The window process reads these.
pub enum Action {
    Show,
    CheckUpdates,
    Promote,
    Demote,
    SyncNow,
    Quit,
}

struct Held {
    _mutex: usize,
    show: usize,
    quit: usize,
}

struct SpawnArgs {
    port: u16,
    data_dir: PathBuf,
}

fn raw(handle: HANDLE) -> usize {
    handle.0 as usize
}

fn handle(raw: usize) -> HANDLE {
    HANDLE(raw as *mut std::ffi::c_void)
}

static TRAY: Mutex<Option<Held>> = Mutex::new(None);
static WINDOW: Mutex<Option<Held>> = Mutex::new(None);
static SPAWN: Mutex<Option<SpawnArgs>> = Mutex::new(None);
static LAST_OPEN_MS: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
static TOOLTIP: Mutex<Option<String>> = Mutex::new(None);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// True when this process owns the window mutex. The tray process does not.
pub fn holds_window() -> bool {
    WINDOW.lock().unwrap().is_some()
}

/// Takes the tray mutex, or leaves when another tray already holds it.
/// A second `--tray` does not open a window.
pub fn acquire_tray() -> bool {
    acquire_mutex(TRAY_MUTEX, TRAY_SHOW, None, &TRAY)
}

/// Takes the window mutex. A second window asks the first one forward and returns false.
pub fn acquire_window() -> bool {
    if acquire_mutex(WINDOW_MUTEX, WINDOW_SHOW, Some(WINDOW_QUIT), &WINDOW) {
        return true;
    }
    signal_event(WINDOW_SHOW);
    tracing::info!("PJDesk Server is already open. Asked that window to come forward.");
    false
}

pub fn gui_is_running() -> bool {
    mutex_held(WINDOW_MUTEX)
}

pub fn tray_is_running() -> bool {
    mutex_held(TRAY_MUTEX)
}

pub fn signal_gui() {
    // Restore from this process. The window process may not be painting,
    // so a show event alone would sit until the next frame.
    crate::winshow::show_from_tray(WINDOW_TITLE);
    let _ = signal_event(WINDOW_SHOW);
}

pub fn signal_tray() {
    let _ = signal_event(TRAY_SHOW);
}

pub fn signal_gui_quit() {
    let _ = signal_event(WINDOW_QUIT);
}

fn window_show_requested() -> bool {
    event_ready(&WINDOW, |held| held.show)
}

fn window_quit_requested() -> bool {
    event_ready(&WINDOW, |held| held.quit)
}

fn tray_show_requested() -> bool {
    event_ready(&TRAY, |held| held.show)
}

/// The window process reads a tray menu action and a request to come forward.
pub fn gui_actions() -> Vec<Action> {
    let mut out = Vec::new();
    if window_quit_requested() {
        out.push(Action::Quit);
        return out;
    }
    if let Some(token) = take_action() {
        match token.as_str() {
            "check" => out.push(Action::CheckUpdates),
            "promote" => out.push(Action::Promote),
            "demote" => out.push(Action::Demote),
            "sync" => out.push(Action::SyncNow),
            _ => {}
        }
    }
    if window_show_requested() {
        out.push(Action::Show);
    }
    out
}

/// Tray-only process. Returns when the user picks Exit tray. Never calls eframe.
pub fn run_tray(data_dir: PathBuf, port: u16) -> anyhow::Result<()> {
    if !acquire_tray() {
        tracing::info!("PJDesk Server tray is already running. This copy is leaving.");
        return Ok(());
    }
    ensure_login_default();
    if let Ok(mut slot) = SPAWN.lock() {
        *slot = Some(SpawnArgs { port, data_dir });
    }
    let Some(icon) = build_icon() else {
        tracing::info!("PJDesk Server could not show a tray icon.");
        return Ok(());
    };
    tracing::info!("PJDesk Server tray is running. This process does not open a window until a click, and it never draws one.");
    pump(icon);
    tracing::info!("PJDesk Server left the tray. The service is left running.");
    Ok(())
}

/// Starts a tray process and does not wait for it.
pub fn spawn_tray(port: u16, data_dir: &Path) -> bool {
    let port_text = port.to_string();
    let dir_text = data_dir.display().to_string();
    spawn_process(&["--tray", "--port", &port_text, "--data-dir", &dir_text], None)
}

fn spawn_window(replace_pid: Option<u32>) -> bool {
    let args = SPAWN.lock().ok().and_then(|slot| slot.as_ref().map(|args| (args.port, args.data_dir.clone())));
    let (port, data_dir) = args.unwrap_or_else(|| (pjdesk_proto::DEFAULT_PORT, PathBuf::from(".")));
    let port_text = port.to_string();
    let dir_text = data_dir.display().to_string();
    spawn_process(&["--window", "--port", &port_text, "--data-dir", &dir_text], replace_pid)
}

fn spawn_process(args: &[&str], replace_pid: Option<u32>) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        tracing::info!("Could not find PJDesk Server to start.");
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(args);
        if let Some(pid) = replace_pid {
            cmd.env("PJDESK_REPLACE_PID", pid.to_string());
        }
        cmd.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP);
        match cmd.spawn() {
            Ok(child) => {
                tracing::info!("Started {} {} (pid {}).", exe.display(), args.join(" "), child.id());
                return true;
            }
            Err(err) => {
                tracing::info!("Could not start {} {}: {err}", exe.display(), args.join(" "));
            }
        }
        let mut again = std::process::Command::new(&exe);
        again.args(args);
        if let Some(pid) = replace_pid {
            again.env("PJDESK_REPLACE_PID", pid.to_string());
        }
        again.creation_flags(DETACHED_PROCESS);
        match again.spawn() {
            Ok(child) => {
                tracing::info!("Started {} {} (pid {}).", exe.display(), args.join(" "), child.id());
                true
            }
            Err(err) => {
                tracing::info!("Could not start {} {}: {err}", exe.display(), args.join(" "));
                false
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, args, replace_pid);
        false
    }
}

/// One open per second. A double-click 90 ms later does not start a second window.
fn claim_open(reason: &str) -> bool {
    let now = now_ms();
    let mut seen = LAST_OPEN_MS.load(Ordering::SeqCst);
    loop {
        let last = if seen == 0 { None } else { Some(seen) };
        if !pjdesk_proto::trayshow::open_allowed(last, now) {
            let ago = now.saturating_sub(last.unwrap_or(now));
            tracing::info!("Tray: {reason} ignored. The last open was {ago} ms ago.");
            return false;
        }
        match LAST_OPEN_MS.compare_exchange(seen, now.max(1), Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(current) => seen = current,
        }
    }
}

/// Bring the existing window forward, or start `pjdesk-server.exe --window`.
/// This process does not call `eframe::run_native`.
fn ask_open(reason: &str) {
    if !claim_open(reason) {
        return;
    }
    unsafe {
        let _ = AllowSetForegroundWindow(0xFFFF_FFFF);
    }
    let running = gui_is_running();
    let decision = pjdesk_proto::trayshow::tray_open(now_ms(), None, running, false);
    match decision {
        pjdesk_proto::trayshow::TrayOpen::Ignore => {}
        pjdesk_proto::trayshow::TrayOpen::Spawn | pjdesk_proto::trayshow::TrayOpen::Replace => {
            tracing::info!("Tray: {reason}. Starting a separate PJDesk Server window process.");
            spawn_window(None);
        }
        pjdesk_proto::trayshow::TrayOpen::Signal => {
            crate::winshow::show_from_tray(WINDOW_TITLE);
            signal_gui();
            tracing::info!("Tray: {reason}. The window is already running. Bringing it forward.");
            std::thread::spawn(|| {
                if !crate::winshow::window_is_hung(WINDOW_TITLE) {
                    return;
                }
                let Some(pid) = crate::winshow::window_process(WINDOW_TITLE) else { return };
                if pid == 0 || pid == std::process::id() {
                    return;
                }
                tracing::info!("The PJDesk Server window process {pid} is hung. Replacing that process. The tray stays.");
                end_process(pid);
                spawn_window(Some(pid));
            });
        }
    }
}

fn end_process(pid: u32) {
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) else {
            return;
        };
        let _ = TerminateProcess(handle, 1);
        let _ = WaitForSingleObject(handle, 2_000);
        let _ = CloseHandle(handle);
    }
}

fn remember_action(token: &str) {
    let path = action_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, token);
}

fn take_action() -> Option<String> {
    let path = action_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn action_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("PJDesk").join("server-window.action")
}

fn elevate(args: &[&str]) {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let Ok(exe) = std::env::current_exe() else { return };
    let params = args.join(" ");
    tracing::info!("Tray: asking Windows to approve `{}`.", args.join(" "));
    unsafe {
        let _ = ShellExecuteW(
            None,
            &HSTRING::from("runas"),
            &HSTRING::from(exe.as_os_str()),
            &HSTRING::from(params),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn acquire_mutex(
    mutex_name: windows::core::PCWSTR,
    show_name: windows::core::PCWSTR,
    quit_name: Option<windows::core::PCWSTR>,
    slot: &Mutex<Option<Held>>,
) -> bool {
    unsafe {
        let mutex = match CreateMutexW(None, true, mutex_name) {
            Ok(h) => h,
            Err(_) => return false,
        };
        if GetLastError() == ERROR_ALREADY_EXISTS && !abandoned_or_free(mutex) {
            let _ = CloseHandle(mutex);
            return false;
        }
        let show = match CreateEventW(None, false, false, show_name) {
            Ok(event) => event,
            Err(_) => return true,
        };
        let quit = match quit_name {
            Some(name) => CreateEventW(None, false, false, name).map(raw).unwrap_or(0),
            None => 0,
        };
        *slot.lock().unwrap() = Some(Held { _mutex: raw(mutex), show: raw(show), quit });
        true
    }
}

fn abandoned_or_free(mutex: HANDLE) -> bool {
    let wait = unsafe { WaitForSingleObject(mutex, 0) };
    wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED
}

fn mutex_held(name: windows::core::PCWSTR) -> bool {
    unsafe {
        let Ok(mutex) = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, name) else {
            return false;
        };
        let wait = WaitForSingleObject(mutex, 0);
        let held = wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED;
        if !held {
            let _ = ReleaseMutex(mutex);
        }
        let _ = CloseHandle(mutex);
        held
    }
}

fn signal_event(name: windows::core::PCWSTR) -> bool {
    unsafe {
        let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, name) else {
            return false;
        };
        let ok = SetEvent(event).is_ok();
        let _ = CloseHandle(event);
        ok
    }
}

fn event_ready(slot: &Mutex<Option<Held>>, which: impl Fn(&Held) -> usize) -> bool {
    let guard = slot.lock().unwrap();
    let Some(held) = guard.as_ref() else { return false };
    let raw_handle = which(held);
    if raw_handle == 0 {
        return false;
    }
    unsafe { WaitForSingleObject(handle(raw_handle), 0) == WAIT_OBJECT_0 }
}

/// Whether this user starts the tray at sign-in. Missing preference means on.
pub fn login_enabled() -> bool {
    read_pref_dword().unwrap_or(1) != 0
}

/// Writes the HKCU Run value, or removes it. Also remembers the choice.
pub fn set_login(enable: bool) {
    write_pref_dword(if enable { 1 } else { 0 });
    if enable {
        if let Ok(exe) = std::env::current_exe() {
            let value = format!("\"{}\" --tray", exe.display());
            write_run_sz(&value);
        }
    } else {
        delete_run_value();
    }
}

/// On by default: a first launch with no preference turns the Run value on,
/// and an existing "on" refreshes the path after an upgrade.
pub fn ensure_login_default() {
    match read_pref_dword() {
        Some(0) => {}
        Some(_) => set_login(true),
        None => set_login(true),
    }
}

/// The window used to own this icon. The icon now lives in the tray process,
/// so the window keeps the field and does not construct one.
#[allow(dead_code)]
pub struct Tray;

#[allow(dead_code)]
impl Tray {
    pub fn set_tooltip(&self, text: &str) {
        if let Ok(mut slot) = TOOLTIP.try_lock() {
            *slot = Some(text.to_string());
        }
    }

    pub fn poll(&self) -> Vec<Action> {
        Vec::new()
    }
}

fn build_icon() -> Option<TrayIcon> {
    let icon = Icon::from_rgba(include_bytes!("../assets/icon_server_32.rgba").to_vec(), 32, 32).ok()?;

    let menu = Menu::new();
    let show = MenuItem::new("Open PJDesk Server", true, None);
    let start = MenuItem::new("Start service", true, None);
    let stop = MenuItem::new("Stop service", true, None);
    let restart = MenuItem::new("Restart service", true, None);
    let updates = MenuItem::new("Check for updates now", true, None);
    let log = MenuItem::new("Open log folder", true, None);
    let promote = MenuItem::new("Make this the primary", true, None);
    let demote = MenuItem::new("Step down to standby", true, None);
    let sync = MenuItem::new("Cloud sync now", true, None);
    let quit = MenuItem::new("Exit tray", true, None);
    menu.append(&show).ok()?;
    menu.append(&start).ok()?;
    menu.append(&stop).ok()?;
    menu.append(&restart).ok()?;
    menu.append(&updates).ok()?;
    menu.append(&log).ok()?;
    menu.append(&promote).ok()?;
    menu.append(&demote).ok()?;
    menu.append(&sync).ok()?;
    menu.append(&quit).ok()?;

    let show_id = show.id().0.clone();
    let start_id = start.id().0.clone();
    let stop_id = stop.id().0.clone();
    let restart_id = restart.id().0.clone();
    let updates_id = updates.id().0.clone();
    let log_id = log.id().0.clone();
    let promote_id = promote.id().0.clone();
    let demote_id = demote.id().0.clone();
    let sync_id = sync.id().0.clone();
    let quit_id = quit.id().0.clone();

    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id.0 == show_id {
            ask_open("Open");
        } else if e.id.0 == start_id {
            elevate(&["service", "start"]);
        } else if e.id.0 == stop_id {
            elevate(&["service", "stop"]);
        } else if e.id.0 == restart_id {
            elevate(&["service", "restart"]);
        } else if e.id.0 == updates_id {
            remember_action("check");
            ask_open("Check for updates");
        } else if e.id.0 == log_id {
            let dir = SPAWN.lock().ok().and_then(|slot| slot.as_ref().map(|args| args.data_dir.clone()));
            if let Some(dir) = dir {
                open_log_folder(&dir);
            }
        } else if e.id.0 == promote_id {
            remember_action("promote");
            ask_open("Make this the primary");
        } else if e.id.0 == demote_id {
            remember_action("demote");
            ask_open("Step down to standby");
        } else if e.id.0 == sync_id {
            remember_action("sync");
            ask_open("Cloud sync now");
        } else if e.id.0 == quit_id {
            tracing::info!("Tray click: Exit tray. The window process is asked to exit. The service stays.");
            signal_gui_quit();
            STOP.store(true, Ordering::SeqCst);
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let click = match e {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                Some(pjdesk_proto::trayshow::Click::Left)
            }
            TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => Some(pjdesk_proto::trayshow::Click::LeftDouble),
            _ => None,
        };
        let Some(click) = click else { return };
        if !pjdesk_proto::trayshow::opens_window(click) {
            return;
        }
        let reason = if click == pjdesk_proto::trayshow::Click::LeftDouble { "double-click" } else { "left click" };
        ask_open(reason);
    }));

    TrayIconBuilder::new()
        .with_tooltip("PJDesk Server")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()
}

/// Tooltip from the service session. `None` when the service did not answer.
fn tray_sign_in_notice() -> Option<String> {
    let resp = crate::control::call(&crate::control::Request { cmd: "session".into(), ..Default::default() })?;
    if resp.account_copy.contains(crate::account_mirror::SERVER_SIGN_IN_NEEDED) {
        Some(format!("PJDesk Server\n{}", crate::account_mirror::SERVER_SIGN_IN_NEEDED))
    } else {
        Some("PJDesk Server".into())
    }
}

/// Pumps the tray window on this thread. There is no eframe loop here.
fn pump(icon: TrayIcon) {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };
    let mut logged_at = std::time::Instant::now() - std::time::Duration::from_secs(3);
    let mut sign_in_at = std::time::Instant::now() - std::time::Duration::from_secs(30);
    loop {
        if STOP.load(Ordering::SeqCst) {
            return;
        }
        let tip = TOOLTIP.try_lock().ok().and_then(|mut slot| slot.take());
        if let Some(tip) = tip {
            let _ = icon.set_tooltip(Some(tip));
        }
        if sign_in_at.elapsed() >= std::time::Duration::from_secs(15) {
            sign_in_at = std::time::Instant::now();
            if let Some(notice) = tray_sign_in_notice() {
                let _ = icon.set_tooltip(Some(notice));
            }
        }
        if tray_show_requested() {
            ask_open("second launch");
        }
        let elapsed = logged_at.elapsed().as_millis() as u64;
        if pjdesk_proto::trayshow::parked_alive_due(elapsed) {
            tracing::info!("The tray loop is alive. This process is not drawing a window.");
            logged_at = std::time::Instant::now();
        }
        let wait = unsafe { MsgWaitForMultipleObjects(None, false, 200, QS_ALLINPUT) };
        if wait != WAIT_OBJECT_0 {
            continue;
        }
        let mut saw = false;
        loop {
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            if !unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                break;
            }
            saw = true;
            if msg.message == WM_QUIT {
                return;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if !saw {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

pub fn open_log_folder(dir: &Path) {
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

fn write_run_sz(value: &str) {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
    };
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, Some(0), KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        let value = HSTRING::from(value);
        let bytes: &[u8] =
            std::slice::from_raw_parts(value.as_ptr() as *const u8, (value.len() + 1) * std::mem::size_of::<u16>());
        let _ = RegSetValueExW(key, RUN_VALUE, Some(0), REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        let _ = PCWSTR::null();
    }
}

fn delete_run_value() {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, Some(0), KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        let _ = RegDeleteValueW(key, RUN_VALUE);
        let _ = RegCloseKey(key);
    }
}

fn read_pref_dword() -> Option<u32> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_DWORD, REG_VALUE_TYPE,
    };
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PREF_KEY, Some(0), KEY_QUERY_VALUE, &mut key).is_err() {
            return None;
        }
        let mut kind = REG_VALUE_TYPE::default();
        let mut data = [0u8; 4];
        let mut len = data.len() as u32;
        let ok = RegQueryValueExW(key, PREF_VALUE, None, Some(&mut kind), Some(data.as_mut_ptr()), Some(&mut len));
        let _ = RegCloseKey(key);
        if ok.is_err() || kind != REG_DWORD || len < 4 {
            return None;
        }
        Some(u32::from_le_bytes(data))
    }
}

fn write_pref_dword(value: u32) {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD,
        REG_OPTION_NON_VOLATILE,
    };
    unsafe {
        let mut key = HKEY::default();
        let mut disp = windows::Win32::System::Registry::REG_CREATE_KEY_DISPOSITION::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PREF_KEY,
            Some(0),
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            Some(&mut disp),
        )
        .is_err()
        {
            return;
        }
        let bytes = value.to_le_bytes();
        let _ = RegSetValueExW(key, PREF_VALUE, Some(0), REG_DWORD, Some(&bytes));
        let _ = RegCloseKey(key);
    }
}
