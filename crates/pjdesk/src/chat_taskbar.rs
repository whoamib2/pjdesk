//! Taskbar identity for the chat window process.
//!
//! This process is the same `pjdesk` executable as the main window, so Windows
//! would otherwise group the chat under that icon (or show a generic window
//! glyph). We give it its own AppUserModelID, `WS_EX_APPWINDOW`, the chat
//! bubble via `WM_SETICON` (built from the embedded RGBA, not from a temp
//! `.ico`), and `System.AppUserModel.RelaunchIconResource` pointing at the
//! chat icon resource in this executable (`id 2`). All of this runs on the
//! chat window's own thread. It does not call `ShowWindow`, and it never does
//! a synchronous window call into another process (0.6.94 hang rules).

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleBitmap, DeleteObject, GetDC, ReleaseDC, SetDIBits, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, EnumWindows, FlashWindowEx, GetWindow, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, SendNotifyMessageW, SetWindowLongPtrW,
    SetWindowPos, FLASHWINFO, FLASHW_TIMERNOFG, FLASHW_TRAY, GW_OWNER, GWL_EXSTYLE, HWND_TOP,
    ICONINFO, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_SETICON,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::Win32::System::Threading::GetCurrentProcessId;

use pjdesk_proto::winplace::scale_rgba;

/// Process AppUserModelID so the chat button does not group under the main app.
pub const APP_USER_MODEL_ID: &str = "PJDesk.Chat";

/// Win32 resource id of `assets/chat.ico`, embedded by `build.rs`.
pub const CHAT_ICON_RESOURCE_ID: u16 = 2;

const ICON_SMALL: usize = 0;
const ICON_BIG: usize = 1;

/// `PKEY_AppUserModel_*` (fmtid 9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3).
const PKEY_FMTID: windows::core::GUID =
    windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3);

fn pkey(pid: u32) -> PROPERTYKEY {
    PROPERTYKEY { fmtid: PKEY_FMTID, pid }
}

static PREPARED: AtomicBool = AtomicBool::new(false);
static PROPS_APPLIED: AtomicBool = AtomicBool::new(false);
static HWND_CACHE: Mutex<isize> = Mutex::new(0);
// Raw handles: HICON is a pointer and is not Send in windows-rs, but these
// icons are process-wide and only used on the window thread after creation.
static ICONS: Mutex<Option<(isize, isize)>> = Mutex::new(None);

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
/// Safe to call every frame; icon assignment is cheap once the HICONs are
/// cached. We set `WM_SETICON` again so a later framework icon change cannot
/// leave the generic default in place.
pub fn apply_taskbar_icon() {
    let Some(hwnd) = this_process_window() else { return };
    if let Ok(mut g) = HWND_CACHE.lock() {
        *g = hwnd.0 as isize;
    }
    apply_appwindow(hwnd);
    apply_icons(hwnd);
    if !PROPS_APPLIED.load(Ordering::SeqCst) && apply_relaunch_props(hwnd) {
        PROPS_APPLIED.store(true, Ordering::SeqCst);
    }
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

pub(crate) fn this_process_hwnd() -> Option<HWND> {
    this_process_window()
}

fn this_process_window() -> Option<HWND> {
    let mut found = Found::default();
    unsafe {
        let _ = EnumWindows(Some(enum_ours), LPARAM(&mut found as *mut Found as isize));
    }
    if !found.named.0.is_null() {
        Some(found.named)
    } else if !found.any.0.is_null() {
        Some(found.any)
    } else {
        None
    }
}

#[derive(Default)]
struct Found {
    named: HWND,
    any: HWND,
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
        let slot = lparam.0 as *mut Found;
        if (*slot).any.0.is_null() {
            (*slot).any = hwnd;
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            if title == "PJDesk Chat" || title.starts_with("PJDesk Chat ") || title.starts_with("PJDesk Chat–") || title.starts_with("PJDesk Chat –") {
                (*slot).named = hwnd;
                return windows::core::BOOL(0);
            }
        }
        windows::core::BOOL(1)
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
    let Some((small, big)) = cached_icons() else {
        tracing::warn!("Could not build the chat taskbar icons from the embedded artwork");
        return;
    };
    unsafe {
        // SendNotifyMessage does not wait for the window procedure. We are on
        // the window thread anyway; this still avoids a nested send.
        let _ = SendNotifyMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL), LPARAM(small.0 as isize));
        let _ = SendNotifyMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG), LPARAM(big.0 as isize));
    }
}

fn cached_icons() -> Option<(windows::Win32::UI::WindowsAndMessaging::HICON, windows::Win32::UI::WindowsAndMessaging::HICON)> {
    let mut guard = ICONS.lock().ok()?;
    if let Some((small, big)) = *guard {
        return Some((
            windows::Win32::UI::WindowsAndMessaging::HICON(small as *mut _),
            windows::Win32::UI::WindowsAndMessaging::HICON(big as *mut _),
        ));
    }
    let big_rgba: &[u8] = include_bytes!("../assets/chat_icon_32.rgba");
    let small_rgba = scale_rgba(big_rgba, 32, 32, 16, 16);
    let small = icon_from_rgba(&small_rgba, 16, 16)?;
    let big = icon_from_rgba(big_rgba, 32, 32)?;
    *guard = Some((small.0 as isize, big.0 as isize));
    Some((small, big))
}

/// Build an HICON from unpremultiplied RGBA pixels. This avoids `LoadImageW`
/// on a PNG-compressed ICO, which can fail or yield a blank icon.
fn icon_from_rgba(rgba: &[u8], w: u32, h: u32) -> Option<windows::Win32::UI::WindowsAndMessaging::HICON> {
    if w == 0 || h == 0 || rgba.len() != (w * h * 4) as usize {
        return None;
    }
    let mut bgra = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let color = CreateCompatibleBitmap(screen, w as i32, h as i32);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: bgra.len() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let written = SetDIBits(Some(screen), color, 0, h, bgra.as_ptr() as *const _, &info, DIB_RGB_COLORS);
        // 1-bit mask, all zeros = opaque. Stride is 32-bit aligned.
        let stride = ((w as usize + 31) / 32) * 4;
        let mask_bits = vec![0u8; stride * h as usize];
        let mask = CreateBitmap(w as i32, h as i32, 1, 1, Some(mask_bits.as_ptr() as *const _));
        let _ = ReleaseDC(None, screen);
        if written == 0 || color.is_invalid() || mask.is_invalid() {
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            return None;
        }
        let ii = ICONINFO {
            fIcon: windows::core::BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&ii);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon.ok()
    }
}

fn apply_relaunch_props(hwnd: HWND) -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let store = match unsafe { SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) } {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Could not open the chat window property store: {e}");
            return false;
        }
    };
    let Some(exe) = module_path() else {
        tracing::warn!("Could not read this executable's path for the chat icon resource");
        return false;
    };
    // Resource id 2 is assets/chat.ico, embedded by build.rs. The shell loads
    // PNG-in-ICO frames from an exe resource even when LoadImageW will not.
    let icon_res = format!("{exe},-{CHAT_ICON_RESOURCE_ID}");
    let command = if exe.contains(' ') { format!("\"{exe}\"") } else { exe.clone() };
    let sets = [
        (pkey(5), APP_USER_MODEL_ID),          // ID
        (pkey(2), command.as_str()),           // RelaunchCommand
        (pkey(3), icon_res.as_str()),          // RelaunchIconResource
        (pkey(4), "PJDesk Chat"),              // RelaunchDisplayNameResource
    ];
    for (key, value) in sets {
        let pv = PROPVARIANT::from(value);
        if let Err(e) = unsafe { store.SetValue(&key, &pv) } {
            tracing::warn!("Could not set a chat AppUserModel property: {e}");
            return false;
        }
    }
    if let Err(e) = unsafe { store.Commit() } {
        tracing::warn!("Could not commit the chat window property store: {e}");
        return false;
    }
    true
}

fn module_path() -> Option<String> {
    let mut buf = [0u16; 32768];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) };
    if n == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rgba_icons_are_nonempty_accent_bubbles() {
        let big: &[u8] = include_bytes!("../assets/chat_icon_32.rgba");
        assert_eq!(big.len(), 32 * 32 * 4);
        let small = scale_rgba(big, 32, 32, 16, 16);
        for (bytes, side) in [(big, 32usize), (small.as_slice(), 16usize)] {
            let opaque = bytes.chunks(4).filter(|p| p[3] > 180).count();
            assert!(opaque > side * 2, "{side}px bubble should have opaque pixels");
            let blue = bytes
                .chunks(4)
                .filter(|p| p[3] > 180 && p[2] > 140 && p[2] > p[0].saturating_add(20))
                .count();
            assert!(blue > 20, "{side}px bubble should keep the accent");
            // Corners stay transparent so the taskbar does not draw a white square.
            for (x, y) in [(0, 0), (side - 1, 0), (0, side - 1), (side - 1, side - 1)] {
                assert_eq!(bytes[(y * side + x) * 4 + 3], 0, "corner {x},{y}");
            }
        }
        let icon = icon_from_rgba(big, 32, 32).expect("CreateIconIndirect should accept the 32 px bubble");
        assert!(!icon.is_invalid());
        let icon16 = icon_from_rgba(&small, 16, 16).expect("CreateIconIndirect should accept the 16 px bubble");
        assert!(!icon16.is_invalid());
    }
}
