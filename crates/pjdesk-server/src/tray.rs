//! System-tray icon for PJDesk Server.
//!
//! The service runs in session 0 and cannot show an icon. This module is the
//! per-user companion: `pjdesk-server.exe` or `pjdesk-server.exe --tray`.
//! A named mutex keeps a single icon per signed-in user. A named event lets a
//! second launch ask the first one to open the window.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::core::w;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\PJDeskServerTray");
const EVENT_NAME: windows::core::PCWSTR = w!("Local\\PJDeskServerTrayShow");
const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const PREF_KEY: windows::core::PCWSTR = w!("Software\\PJDeskServer");
const RUN_VALUE: windows::core::PCWSTR = w!("PJDesk Server");
const PREF_VALUE: windows::core::PCWSTR = w!("TrayAtLogin");

/// What the user asked for via the tray.
pub enum Action {
    Show,
    ServiceStart,
    ServiceStop,
    ServiceRestart,
    CheckUpdates,
    OpenLog,
    Promote,
    Demote,
    SyncNow,
    Quit,
}

struct Singleton {
    _mutex: usize,
    event: usize,
}

fn raw(handle: HANDLE) -> usize {
    handle.0 as usize
}

fn handle(raw: usize) -> HANDLE {
    HANDLE(raw as *mut std::ffi::c_void)
}

static HELD: Mutex<Option<Singleton>> = Mutex::new(None);
static MENU_TX: Mutex<Option<std::sync::mpsc::Sender<Action>>> = Mutex::new(None);
static MENU_RX: Mutex<Option<std::sync::mpsc::Receiver<Action>>> = Mutex::new(None);
static DEFERRED: Mutex<Vec<Action>> = Mutex::new(Vec::new());
static ICON_CLICKED: AtomicBool = AtomicBool::new(false);
static REOPEN: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);
static WINDOW_OPEN: AtomicBool = AtomicBool::new(false);
static ICON_READY: AtomicBool = AtomicBool::new(false);
static ICON_THREAD: AtomicBool = AtomicBool::new(false);
static TOOLTIP: Mutex<Option<String>> = Mutex::new(None);
static WAKE: Mutex<Option<egui::Context>> = Mutex::new(None);

fn window_is_open() -> bool {
    WINDOW_OPEN.load(Ordering::SeqCst)
}

/// The window thread sets this while eframe is running. Clearing it drops
/// the context, so a click after the window is gone does not repaint it.
pub fn set_window_open(open: bool) {
    WINDOW_OPEN.store(open, Ordering::SeqCst);
    if !open {
        if let Ok(mut slot) = WAKE.lock() {
            *slot = None;
        }
    }
}

pub fn process_should_exit() -> bool {
    QUIT.load(Ordering::SeqCst)
}

pub fn request_quit() {
    QUIT.store(true, Ordering::SeqCst);
}

pub fn icon_running() -> bool {
    ICON_READY.load(Ordering::SeqCst)
}

fn set_wake(ctx: &egui::Context) {
    if let Ok(mut slot) = WAKE.lock() {
        *slot = Some(ctx.clone());
    }
}

fn wake() {
    let ctx = WAKE.lock().ok().and_then(|slot| slot.clone());
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

fn ensure_channel() {
    let mut rx = MENU_RX.lock().unwrap();
    if rx.is_none() {
        let (tx, recv) = std::sync::mpsc::channel();
        if let Ok(mut slot) = MENU_TX.lock() {
            *slot = Some(tx);
        }
        *rx = Some(recv);
    }
}

/// Starts the tray icon before any window exists. `--tray` uses this so the
/// first thing drawn is a click, not a hidden surface.
pub fn start_without_window() -> bool {
    ensure_channel();
    spawn_icon_thread()
}

fn spawn_icon_thread() -> bool {
    if ICON_THREAD.swap(true, Ordering::SeqCst) {
        return ICON_READY.load(Ordering::SeqCst);
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new().name("pjdesk-server-tray".into()).spawn(move || {
        let Some(icon) = build_icon() else {
            let _ = ready_tx.send(false);
            ICON_THREAD.store(false, Ordering::SeqCst);
            return;
        };
        let _ = ready_tx.send(true);
        tracing::info!("The PJDesk Server tray icon is on its own thread. A click does not wait for the window.");
        pump(icon);
    });
    if spawned.is_err() {
        ICON_THREAD.store(false, Ordering::SeqCst);
        return false;
    }
    match ready_rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(true) => {
            ICON_READY.store(true, Ordering::SeqCst);
            true
        }
        _ => false,
    }
}

/// Blocks on this thread until a tray click should open a window, or Quit.
/// Logs on a timer so a closed window can be told apart from a stuck one.
pub fn wait_for_reopen(name: &str) -> bool {
    let mut logged_at = std::time::Instant::now() - std::time::Duration::from_secs(3);
    loop {
        if process_should_exit() {
            return false;
        }
        if REOPEN.swap(false, Ordering::SeqCst) {
            return true;
        }
        let mut open = false;
        if let Ok(mut slot) = MENU_RX.lock() {
            if let Some(rx) = slot.as_mut() {
                while let Ok(action) = rx.try_recv() {
                    if matches!(action, Action::Quit) {
                        request_quit();
                        return false;
                    }
                    if let Ok(mut deferred) = DEFERRED.lock() {
                        deferred.push(action);
                    }
                    open = true;
                }
            }
        }
        if open {
            return true;
        }
        let elapsed = logged_at.elapsed().as_millis() as u64;
        if pjdesk_proto::trayshow::parked_alive_due(elapsed) {
            tracing::info!("The tray loop is alive. The {name} window is closed and is not being drawn.");
            logged_at = std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Restores a live window, or asks the main thread to create one. This
/// returns without waiting. There is no surface to present into while the
/// window is closed, so the click does not replace the process.
fn ask_open(reason: &str) {
    unsafe {
        let _ = AllowSetForegroundWindow(0xFFFF_FFFF);
    }
    if window_is_open() {
        ICON_CLICKED.store(true, Ordering::SeqCst);
        crate::winshow::show_from_tray("PJDesk Server");
        crate::traywatch::arm();
        wake();
        tracing::info!(
            "Tray: {reason}. Asked the PJDesk Server window to come forward. The tray thread restored it and did not wait."
        );
    } else {
        REOPEN.store(true, Ordering::SeqCst);
        tracing::info!("Tray: {reason}. The window is closed. Opening a new one. Nothing is drawn until it is open.");
    }
}

fn push_menu(action: Action) {
    // The window thread never holds this mutex. Two tries, then drop the
    // click, rather than waiting on the window.
    for _ in 0..2 {
        if let Ok(slot) = MENU_TX.try_lock() {
            if let Some(tx) = slot.as_ref() {
                let _ = tx.send(action);
            }
            return;
        }
        std::thread::yield_now();
    }
    tracing::info!("Tray menu click was not delivered because the menu queue was busy.");
}

/// Takes the per-user tray, or asks the owner to show its window and returns false.
pub fn acquire_or_signal() -> bool {
    unsafe {
        let mutex = match CreateMutexW(None, true, MUTEX_NAME) {
            Ok(h) => h,
            Err(_) => return false,
        };
        if GetLastError() == ERROR_ALREADY_EXISTS && !abandoned_or_free(mutex) {
            let _ = CloseHandle(mutex);
            // The new process is in front. Let the open window take the foreground.
            let _ = AllowSetForegroundWindow(0xFFFF_FFFF);
            for _ in 0..20 {
                if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, EVENT_NAME) {
                    let _ = SetEvent(event);
                    let _ = CloseHandle(event);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            tracing::info!("PJDesk Server is already open. Asked that window to come forward.");
            return false;
        }
        let event = match CreateEventW(None, false, false, EVENT_NAME) {
            Ok(event) => event,
            Err(_) => return true,
        };
        *HELD.lock().unwrap() = Some(Singleton { _mutex: raw(mutex), event: raw(event) });
        true
    }
}

/// `ERROR_ALREADY_EXISTS` does not grant ownership. A dead owner leaves
/// the mutex abandoned, and this wait takes it. A living owner still
/// holds it, so the wait times out and the caller signals that window.
fn abandoned_or_free(mutex: HANDLE) -> bool {
    let wait = unsafe { WaitForSingleObject(mutex, 0) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        tracing::info!(
            "The previous PJDesk Server window is not running. This launch is opening the window. The single-instance lock did not block it."
        );
        true
    } else {
        false
    }
}

/// True when this process owns the single window.
pub fn holds_window() -> bool {
    HELD.lock().unwrap().is_some()
}

/// True when another launch asked this process to open the window.
pub fn show_requested() -> bool {
    let guard = HELD.lock().unwrap();
    let Some(held) = guard.as_ref() else { return false };
    unsafe { WaitForSingleObject(handle(held.event), 0) == WAIT_OBJECT_0 }
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

pub struct Tray;

impl Tray {
    /// Builds the tray icon on its own thread, or returns `None` if that fails.
    /// A second window reuses the icon that stayed up while this one was closed.
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        set_wake(ctx);
        if start_without_window() { Some(Self) } else { None }
    }

    pub fn set_tooltip(&self, text: &str) {
        // The window thread does not call into the tray window. The tray
        // thread picks this up on its own loop.
        if let Ok(mut slot) = TOOLTIP.try_lock() {
            *slot = Some(text.to_string());
        }
    }

    pub fn poll(&self) -> Vec<Action> {
        let mut out = Vec::new();
        if let Ok(mut deferred) = DEFERRED.lock() {
            out.append(&mut deferred);
        }
        let clicked = ICON_CLICKED.swap(false, Ordering::SeqCst);
        let reopen = REOPEN.swap(false, Ordering::SeqCst);
        let second = show_requested();
        if clicked || reopen || second {
            out.push(Action::Show);
        }
        if let Ok(mut slot) = MENU_RX.lock() {
            if let Some(rx) = slot.as_mut() {
                while let Ok(action) = rx.try_recv() {
                    out.push(action);
                }
            }
        }
        out
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

        let show_now = show_id.clone();
        let start_now = start_id.clone();
        let stop_now = stop_id.clone();
        let restart_now = restart_id.clone();
        let updates_now = updates_id.clone();
        let log_now = log_id.clone();
        let promote_now = promote_id.clone();
        let demote_now = demote_id.clone();
        let sync_now = sync_id.clone();
        let quit_now = quit_id.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id.0 == show_now {
                ask_open("Open");
                return;
            }
            let action = if e.id.0 == start_now {
                Action::ServiceStart
            } else if e.id.0 == stop_now {
                Action::ServiceStop
            } else if e.id.0 == restart_now {
                Action::ServiceRestart
            } else if e.id.0 == updates_now {
                Action::CheckUpdates
            } else if e.id.0 == log_now {
                Action::OpenLog
            } else if e.id.0 == promote_now {
                Action::Promote
            } else if e.id.0 == demote_now {
                Action::Demote
            } else if e.id.0 == sync_now {
                Action::SyncNow
            } else if e.id.0 == quit_now {
                Action::Quit
            } else {
                return;
            };
            push_menu(action);
            wake();
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

/// Pumps the tray window. This thread is not the window thread, so a stuck
/// window cannot hide a click.
fn pump(icon: TrayIcon) {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };
    loop {
        let tip = TOOLTIP.try_lock().ok().and_then(|mut slot| slot.take());
        if let Some(tip) = tip {
            let _ = icon.set_tooltip(Some(tip));
        }
        // A second launch sets this event. A live window is restored here.
        // A closed window is created by the main thread, which is not inside
        // a present while it waits.
        if show_requested() {
            if window_is_open() {
                crate::winshow::show_from_tray("PJDesk Server");
                ICON_CLICKED.store(true, Ordering::SeqCst);
            } else {
                REOPEN.store(true, Ordering::SeqCst);
                tracing::info!("A second launch asked for PJDesk Server. The window was closed, so it will be opened again.");
            }
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
