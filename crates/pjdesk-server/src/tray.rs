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
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0};
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
static MENU_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ICON_CLICKED: AtomicBool = AtomicBool::new(false);

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
    const WAIT_ABANDONED: u32 = 0x80;
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

pub struct Tray {
    icon: TrayIcon,
    show_id: String,
    start_id: String,
    stop_id: String,
    restart_id: String,
    updates_id: String,
    log_id: String,
    promote_id: String,
    demote_id: String,
    sync_id: String,
    quit_id: String,
}

impl Tray {
    pub fn new(ctx: &egui::Context) -> Option<Self> {
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
        let c1 = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id.0 == show_now {
                crate::winshow::activate("Open", "PJDesk Server", holds_window());
            }
            MENU_IDS.lock().unwrap().push(e.id.0.clone());
            c1.request_repaint();
        }));
        let c2 = ctx.clone();
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
            crate::winshow::activate(reason, "PJDesk Server", holds_window());
            ICON_CLICKED.store(true, Ordering::SeqCst);
            c2.request_repaint();
        }));

        let icon = TrayIconBuilder::new()
            .with_tooltip("PJDesk Server")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;
        Some(Self {
            icon,
            show_id,
            start_id,
            stop_id,
            restart_id,
            updates_id,
            log_id,
            promote_id,
            demote_id,
            sync_id,
            quit_id,
        })
    }

    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(text));
    }

    pub fn poll(&self) -> Vec<Action> {
        let mut out = Vec::new();
        let clicked = ICON_CLICKED.swap(false, Ordering::SeqCst);
        let second = show_requested();
        if second {
            crate::winshow::activate("second launch", "PJDesk Server", holds_window());
        }
        if clicked || second {
            out.push(Action::Show);
        }
        for id in MENU_IDS.lock().unwrap().drain(..) {
            if id == self.show_id {
                out.push(Action::Show);
            } else if id == self.start_id {
                out.push(Action::ServiceStart);
            } else if id == self.stop_id {
                out.push(Action::ServiceStop);
            } else if id == self.restart_id {
                out.push(Action::ServiceRestart);
            } else if id == self.updates_id {
                out.push(Action::CheckUpdates);
            } else if id == self.log_id {
                out.push(Action::OpenLog);
            } else if id == self.promote_id {
                out.push(Action::Promote);
            } else if id == self.demote_id {
                out.push(Action::Demote);
            } else if id == self.sync_id {
                out.push(Action::SyncNow);
            } else if id == self.quit_id {
                out.push(Action::Quit);
            }
        }
        out
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
