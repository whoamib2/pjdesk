//! Taskbar identity for the chat window process.
//!
//! This process is the same `pjdesk` executable as the main window, so Windows
//! would otherwise group the chat under that icon (or show a generic window
//! glyph). We give it its own AppUserModelID, `WS_EX_APPWINDOW`, and the chat
//! bubble icons via `WM_SETICON`. All of this runs on the chat window's own
//! thread. It does not call `ShowWindow`, and it never does a synchronous
//! window call into another process (0.6.94 hang rules).

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FlashWindowEx, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindow,
    IsWindowVisible, LoadImageW, SendNotifyMessageW, SetWindowLongPtrW, SetWindowPos, FLASHWINFO,
    FLASHW_TIMERNOFG, FLASHW_TRAY, GW_OWNER, GWL_EXSTYLE, HWND_TOP, IMAGE_ICON, LR_DEFAULTCOLOR,
    LR_LOADFROMFILE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    WM_SETICON, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// Process AppUserModelID so the chat button does not group under the main app.
pub const APP_USER_MODEL_ID: &str = "PJDesk.Chat";

const ICON_SMALL: usize = 0;
const ICON_BIG: usize = 1;

static PREPARED: AtomicBool = AtomicBool::new(false);
static APPLIED: AtomicBool = AtomicBool::new(false);
static HWND_CACHE: Mutex<isize> = Mutex::new(0);

/// Call once before the window is created.
pub fn set_app_user_model_id() {
    if PREPARED.swap(true, Ordering::SeqCst) {
        return;
    }
    let wide: Vec<u16> = APP_USER_MODEL_ID.encode_utf16().chain(std::iter::once(0)).collect();
    if let Err(e) = unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide.as_ptr())) } {
        tracing::warn!("Could not set the chat AppUserModelID: {e}");
    }
}

/// After the window exists: own taskbar button, chat icons, not a tool window.
/// Safe to call every frame; it only talks to this process's window.
pub fn apply_taskbar_icon() {
    if APPLIED.load(Ordering::SeqCst) {
        if let Some(hwnd) = cached_hwnd() {
            if unsafe { IsWindow(Some(hwnd)).as_bool() } {
                return;
            }
            APPLIED.store(false, Ordering::SeqCst);
        }
    }
    let Some(hwnd) = this_process_window() else { return };
    if let Ok(mut g) = HWND_CACHE.lock() {
        *g = hwnd.0 as isize;
    }
    apply_appwindow(hwnd);
    apply_icons(hwnd);
    APPLIED.store(true, Ordering::SeqCst);
}

/// Flash the taskbar button when a new message arrives and the window is not
/// already in front. `FlashWindowEx` does not call the window procedure.
pub fn flash_taskbar() {
    apply_taskbar_icon();
    let Some(hwnd) = cached_hwnd() else { return };
    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd,
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 5,
        dwTimeout: 0,
    };
    let _ = unsafe { FlashWindowEx(&info) };
}

fn cached_hwnd() -> Option<HWND> {
    let raw = *HWND_CACHE.lock().ok()?;
    if raw == 0 {
        return None;
    }
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    if unsafe { IsWindow(Some(hwnd)).as_bool() } {
        Some(hwnd)
    } else {
        None
    }
}

fn this_process_window() -> Option<HWND> {
    let mut found = HWND::default();
    unsafe {
        let _ = EnumWindows(Some(enum_ours), LPARAM(&mut found as *mut HWND as isize));
    }
    if found.0.is_null() {
        None
    } else {
        Some(found)
    }
}

unsafe extern "system" fn enum_ours(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return windows::core::BOOL(1);
        }
        if GetWindow(hwnd, GW_OWNER).ok().is_some_and(|owner| !owner.is_invalid()) {
            return windows::core::BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != GetCurrentProcessId() {
            return windows::core::BOOL(1);
        }
        let slot = lparam.0 as *mut HWND;
        *slot = hwnd;
        windows::core::BOOL(0)
    }
}

fn apply_appwindow(hwnd: HWND) {
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let next = (current | WS_EX_APPWINDOW.0 as isize) & !(WS_EX_TOOLWINDOW.0 as isize);
        if next == current {
            return;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

fn apply_icons(hwnd: HWND) {
    let Some(path) = icon_file() else { return };
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let small = LoadImageW(
            None,
            PCWSTR(wide.as_ptr()),
            IMAGE_ICON,
            16,
            16,
            LR_LOADFROMFILE | LR_DEFAULTCOLOR,
        );
        let big = LoadImageW(
            None,
            PCWSTR(wide.as_ptr()),
            IMAGE_ICON,
            32,
            32,
            LR_LOADFROMFILE | LR_DEFAULTCOLOR,
        );
        // SendNotifyMessage does not wait for the window procedure. We are on
        // the window thread anyway; this still avoids a nested send.
        if let Ok(h) = small {
            let _ = SendNotifyMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL), LPARAM(h.0 as isize));
        }
        if let Ok(h) = big {
            let _ = SendNotifyMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG), LPARAM(h.0 as isize));
        }
    }
}

fn icon_file() -> Option<String> {
    let dir = std::env::temp_dir();
    let path = dir.join("pjdesk-chat-icon.ico");
    let bytes: &[u8] = include_bytes!("../assets/chat.ico");
    if std::fs::read(&path).ok().as_deref() != Some(bytes) {
        std::fs::write(&path, bytes).ok()?;
    }
    path.to_str().map(str::to_string)
}
