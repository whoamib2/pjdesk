//! System-tray icon for PJDesk.
//!
//! This process never draws the window. A click starts `pjdesk.exe --window`
//! or brings that process forward. Closing the window exits the window process.
//! The tray keeps running.

/// What the user asked for via the tray.
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Action {
    Show,
    Update,
    Revoke,
    Quit,
}

/// The window process reads a menu action left by the tray, plus show and quit.
pub fn gui_actions() -> Vec<Action> {
    let mut out = Vec::new();
    if super::instance::quit_requested() {
        out.push(Action::Quit);
        return out;
    }
    if let Some(token) = take_window_action() {
        match token.as_str() {
            "update" => out.push(Action::Update),
            "revoke" => out.push(Action::Revoke),
            _ => {}
        }
    }
    if super::instance::show_requested() {
        out.push(Action::Show);
    }
    out
}

fn action_path() -> std::path::PathBuf {
    crate::config::Config::path().parent().map(|dir| dir.join("window.action")).unwrap_or_else(|| std::path::PathBuf::from("window.action"))
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn remember_action(token: &str) {
    let path = action_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, token);
}

fn take_window_action() -> Option<String> {
    let path = action_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Tray-only process. Never calls eframe. A click starts `--window` or brings it forward.
pub fn run_tray() {
    #[cfg(windows)]
    imp::run_tray();
}

/// Starts `pjdesk.exe --tray` and does not wait.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn spawn_tray() -> bool {
    #[cfg(windows)]
    {
        imp::spawn_detached(&["--tray"], None)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn tray_is_running() -> bool {
    #[cfg(windows)]
    {
        imp::tray_is_running()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn signal_tray() {
    #[cfg(windows)]
    imp::signal_tray();
}

#[cfg(windows)]
pub use imp::Tray;
#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    static TOOLTIP: Mutex<Option<String>> = Mutex::new(None);
    static LAST_OPEN_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);
    static TRAY_HELD: Mutex<Option<usize>> = Mutex::new(None);

    const TRAY_MUTEX: windows::core::PCWSTR = windows::core::w!("Local\\PJDeskClientTray");
    const TRAY_SHOW: windows::core::PCWSTR = windows::core::w!("Local\\PJDeskClientTrayShow");

    fn now_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

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

    pub fn run_tray() {
        if !acquire_tray() {
            tracing::info!("PJDesk tray is already running. This copy is leaving.");
            return;
        }
        if crate::config::Config::load().ok().is_some_and(|cfg| cfg.settings.start_with_windows) {
            let _ = super::super::settings::set_run_at_login(true);
        }
        let Some(icon) = build() else {
            tracing::info!("PJDesk could not show a tray icon.");
            return;
        };
        tracing::info!("PJDesk tray is running. This process does not open a window until a click, and it never draws one.");
        pump(icon);
        tracing::info!("PJDesk left the tray. The service is left running.");
    }

    pub fn tray_is_running() -> bool {
        mutex_held(TRAY_MUTEX)
    }

    pub fn signal_tray() {
        let _ = signal_event(TRAY_SHOW);
    }

    fn tray_show_requested() -> bool {
        let guard = TRAY_HELD.lock().unwrap();
        let Some(raw_handle) = *guard else { return false };
        if raw_handle == 0 {
            return false;
        }
        unsafe {
            windows::Win32::System::Threading::WaitForSingleObject(windows::Win32::Foundation::HANDLE(raw_handle as *mut std::ffi::c_void), 0)
                == windows::Win32::Foundation::WAIT_OBJECT_0
        }
    }

    fn acquire_tray() -> bool {
        use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, WaitForSingleObject};
        unsafe {
            let mutex = match CreateMutexW(None, true, TRAY_MUTEX) {
                Ok(h) => h,
                Err(_) => return false,
            };
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let wait = WaitForSingleObject(mutex, 0);
                if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
                    let _ = CloseHandle(mutex);
                    return false;
                }
            }
            let show = match CreateEventW(None, false, false, TRAY_SHOW) {
                Ok(event) => event,
                Err(_) => return true,
            };
            *TRAY_HELD.lock().unwrap() = Some(show.0 as usize);
            let _ = mutex;
            true
        }
    }

    fn mutex_held(name: windows::core::PCWSTR) -> bool {
        use windows::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{
            OpenMutexW, ReleaseMutex, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE,
        };
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
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
        unsafe {
            let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, name) else {
                return false;
            };
            let ok = SetEvent(event).is_ok();
            let _ = CloseHandle(event);
            ok
        }
    }

    pub fn spawn_detached(args: &[&str], replace_pid: Option<u32>) -> bool {
        let Ok(exe) = std::env::current_exe() else {
            tracing::info!("Could not find PJDesk to start.");
            return false;
        };
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
            Err(err) => tracing::info!("Could not start {} {}: {err}", exe.display(), args.join(" ")),
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

    fn end_process(pid: u32) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        };
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) else {
                return;
            };
            let _ = TerminateProcess(handle, 1);
            let _ = WaitForSingleObject(handle, 2_000);
            let _ = CloseHandle(handle);
        }
    }
    /// Bring the existing window forward, or start `pjdesk.exe --window`.
    /// This process does not call `eframe::run_native`.
    fn ask_open(reason: &str) {
        if !claim_open(reason) {
            return;
        }
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(0xFFFF_FFFF);
        }
        let running = super::super::instance::gui_is_running();
        let decision = pjdesk_proto::trayshow::tray_open(now_ms(), None, running, false);
        match decision {
            pjdesk_proto::trayshow::TrayOpen::Ignore => {}
            pjdesk_proto::trayshow::TrayOpen::Spawn | pjdesk_proto::trayshow::TrayOpen::Replace => {
                tracing::info!("Tray: {reason}. Starting a separate PJDesk window process.");
                spawn_detached(&["--window"], None);
            }
            pjdesk_proto::trayshow::TrayOpen::Signal => {
                crate::winshow::show_from_tray("PJDesk");
                super::super::instance::signal_show();
                tracing::info!("Tray: {reason}. The window is already running. Bringing it forward.");
                std::thread::spawn(|| {
                    if !crate::winshow::window_is_hung("PJDesk") {
                        return;
                    }
                    let Some(pid) = crate::winshow::window_process("PJDesk") else { return };
                    if pid == 0 || pid == std::process::id() {
                        return;
                    }
                    tracing::info!("The PJDesk window process {pid} is hung. Replacing that process. The tray stays.");
                    end_process(pid);
                    spawn_detached(&["--window"], Some(pid));
                });
            }
        }
    }

    /// The window keeps this so a tooltip write still compiles. The icon
    /// itself lives in the tray process, which reads `TOOLTIP` on its loop.
    pub struct Tray;

    impl Tray {
        pub fn set_tooltip(&self, text: &str) {
            if let Ok(mut slot) = TOOLTIP.try_lock() {
                *slot = Some(text.to_string());
            }
        }
    }

    fn build() -> Option<TrayIcon> {
        let icon = Icon::from_rgba(include_bytes!("../../assets/icon_32.rgba").to_vec(), 32, 32).ok()?;
        let menu = Menu::new();
        let show = MenuItem::new("Open PJDesk", true, None);
        let update = MenuItem::new("Check for updates now", true, None);
        let revoke = MenuItem::new("Revoke access", true, None);
        let quit = MenuItem::new("Quit PJDesk", true, None);
        menu.append(&show).ok()?;
        menu.append(&update).ok()?;
        menu.append(&revoke).ok()?;
        menu.append(&quit).ok()?;
        let show_id = show.id().0.clone();
        let update_id = update.id().0.clone();
        let revoke_id = revoke.id().0.clone();
        let quit_id = quit.id().0.clone();

        let show_now = show_id.clone();
        let update_now = update_id.clone();
        let revoke_now = revoke_id.clone();
        let quit_now = quit_id.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id.0 == show_now {
                ask_open("Open");
                return;
            }
            if e.id.0 == update_now {
                super::remember_action("update");
                ask_open("Check for updates");
                return;
            }
            if e.id.0 == revoke_now {
                super::remember_action("revoke");
                ask_open("Revoke access");
                return;
            }
            if e.id.0 == quit_now {
                tracing::info!("Tray click: Quit PJDesk. The window process is asked to exit. The service stays.");
                super::super::instance::signal_quit();
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
            .with_tooltip("PJDesk")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()
    }

    /// Pumps the tray window. This thread is not the window thread, so a
    /// stuck window cannot hide a click.
    fn pump(icon: TrayIcon) {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT,
            WM_QUIT,
        };
        let mut logged_at = std::time::Instant::now() - std::time::Duration::from_secs(3);
        loop {
            let tip = TOOLTIP.try_lock().ok().and_then(|mut slot| slot.take());
            if let Some(tip) = tip {
                let _ = icon.set_tooltip(Some(tip));
            }
            if STOP.load(Ordering::SeqCst) {
                return;
            }
            // A second launch sets the tray event. This process starts the window
            // or brings it forward. It does not draw the window itself.
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
}

#[cfg(not(windows))]
pub use stub::Tray;
#[cfg(not(windows))]
mod stub {
    pub struct Tray;
    impl Tray {
        pub fn set_tooltip(&self, _text: &str) {}
    }
}
