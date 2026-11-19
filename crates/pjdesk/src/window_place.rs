//! Pick a monitor work area and place a window inside it.
//!
//! On Windows the work area is `rcWork` (the monitor minus the taskbar), in
//! physical pixels, with per-monitor DPI. The preferred monitor is the one
//! that shows the main PJDesk window, then the one under the cursor.

use pjdesk_proto::trayshow::Rect;

/// The work area that should host a new viewer or chat window.
///
/// `window` is the window being placed, used as a last-resort hint. On
/// platforms without a work-area API this falls back to the full monitor.
pub fn target_work(window: Option<&winit::window::Window>) -> Rect {
    #[cfg(windows)]
    {
        if let Some(work) = os::target_work(window) {
            return work;
        }
    }
    if let Some(window) = window {
        if let Some(m) = window.current_monitor().or_else(|| window.primary_monitor()) {
            let pos = m.position();
            let size = m.size();
            return Rect { x: pos.x, y: pos.y, w: size.width.max(1) as i32, h: size.height.max(1) as i32 };
        }
    }
    Rect { x: 0, y: 0, w: 1920, h: 1080 }
}

/// Outer minus inner size of `window`, in physical pixels.
pub fn chrome(window: &winit::window::Window) -> (i32, i32) {
    let outer = window.outer_size();
    let inner = window.inner_size();
    (
        (outer.width as i32).saturating_sub(inner.width as i32).max(0),
        (outer.height as i32).saturating_sub(inner.height as i32).max(0),
    )
}

/// Move and size `window` so its outer frame matches `outer`. Does not call
/// `ShowWindow`. On Windows this is one `SetWindowPos` with `SWP_NOACTIVATE`.
pub fn apply_outer(window: &winit::window::Window, outer: Rect) {
    let (chrome_w, chrome_h) = chrome(window);
    let inner_w = (outer.w - chrome_w).max(1) as u32;
    let inner_h = (outer.h - chrome_h).max(1) as u32;
    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(inner_w, inner_h));
    window.set_outer_position(winit::dpi::PhysicalPosition::new(outer.x, outer.y));
    #[cfg(windows)]
    os::set_outer(window, outer);
}

/// Work area of the monitor named `name` (winit's `\\.\DISPLAYn` on Windows),
/// or `None` when that monitor is not attached.
pub fn work_by_name(window: &winit::window::Window, name: &str) -> Option<Rect> {
    if name.is_empty() {
        return None;
    }
    #[cfg(windows)]
    {
        if let Some(work) = os::work_by_name(name) {
            return Some(work);
        }
    }
    for m in window.available_monitors() {
        if m.name().as_deref() == Some(name) {
            let pos = m.position();
            let size = m.size();
            return Some(Rect { x: pos.x, y: pos.y, w: size.width.max(1) as i32, h: size.height.max(1) as i32 });
        }
    }
    None
}

/// The work area that should host the chat window, in logical points (the
/// units egui's `ViewportBuilder` uses), plus the scale used to convert.
pub fn chat_work_points() -> Option<(Rect, f32)> {
    #[cfg(windows)]
    {
        return os::chat_work_points();
    }
    #[cfg(not(windows))]
    None
}

/// After the chat HWND exists, clamp or center it in the work area using
/// physical pixels so mixed-DPI monitors stay correct. Once only.
pub fn apply_chat_hwnd_place(x: f32, y: f32, w: f32, h: f32) {
    #[cfg(windows)]
    os::apply_chat_hwnd_place(x, y, w, h);
    #[cfg(not(windows))]
    let _ = (x, y, w, h);
}

#[cfg(windows)]
mod os {
    use super::*;
    use pjdesk_proto::trayshow::title_matches;
    use pjdesk_proto::winplace::{clamp_or_center, clamp_to_work_area};
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
        MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetCursorPos, GetWindow, GetWindowRect, GetWindowTextW, IsWindowVisible,
        SetWindowPos, GW_OWNER, HWND_TOP, SWP_NOACTIVATE, SWP_NOZORDER,
    };
    use windows::core::BOOL;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    static CHAT_PLACED: AtomicBool = AtomicBool::new(false);

    struct Area {
        work: Rect,
        name: String,
        scale: f32,
        primary: bool,
        handle: HMONITOR,
    }

    pub(super) fn target_work(window: Option<&winit::window::Window>) -> Option<Rect> {
        let areas = list_areas();
        if areas.is_empty() {
            return None;
        }
        if let Some(hwnd) = main_pjdesk_window() {
            let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            if let Some(a) = areas.iter().find(|a| a.handle == mon) {
                return Some(a.work);
            }
        }
        if let Some(pt) = cursor_point() {
            let mon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
            if let Some(a) = areas.iter().find(|a| a.handle == mon) {
                return Some(a.work);
            }
        }
        if let Some(window) = window {
            if let Some(hwnd) = hwnd_of(window) {
                let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
                if let Some(a) = areas.iter().find(|a| a.handle == mon) {
                    return Some(a.work);
                }
            }
        }
        areas.iter().find(|a| a.primary).or(areas.first()).map(|a| a.work)
    }

    pub(super) fn work_by_name(name: &str) -> Option<Rect> {
        list_areas().into_iter().find(|a| a.name == name).map(|a| a.work)
    }

    pub(super) fn set_outer(window: &winit::window::Window, outer: Rect) {
        let Some(hwnd) = hwnd_of(window) else { return };
        set_hwnd_rect(hwnd, outer);
    }

    pub(super) fn chat_work_points() -> Option<(Rect, f32)> {
        let areas = list_areas();
        let work = target_work(None)?;
        let scale = areas
            .iter()
            .find(|a| a.work.x == work.x && a.work.y == work.y && a.work.w == work.w && a.work.h == work.h)
            .map(|a| a.scale)
            .unwrap_or(1.0)
            .max(0.5);
        Some((
            Rect {
                x: (work.x as f32 / scale).round() as i32,
                y: (work.y as f32 / scale).round() as i32,
                w: (work.w as f32 / scale).round() as i32,
                h: (work.h as f32 / scale).round() as i32,
            },
            scale,
        ))
    }

    pub(super) fn apply_chat_hwnd_place(x: f32, y: f32, w: f32, h: f32) {
        if CHAT_PLACED.load(Ordering::SeqCst) {
            return;
        }
        let Some(hwnd) = crate::chat_taskbar::this_process_hwnd() else { return };
        CHAT_PLACED.store(true, Ordering::SeqCst);
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let scale = dpi as f32 / 96.0;
        let mut rect = unsafe {
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r).is_err() {
                return;
            }
            to_rect(r)
        };
        // Prefer the live window size when the frame has already been created;
        // fall back to the saved inner size converted to physical pixels.
        if rect.w < 50 || rect.h < 50 {
            rect.w = (w * scale).round() as i32;
            rect.h = (h * scale).round() as i32;
        }
        if x.is_finite() && y.is_finite() {
            rect.x = (x * scale).round() as i32;
            rect.y = (y * scale).round() as i32;
        }
        let Some(work) = target_work(None).or_else(|| {
            let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            list_areas().into_iter().find(|a| a.handle == mon).map(|a| a.work)
        }) else {
            return;
        };
        let placed = if x.is_finite() && y.is_finite() {
            clamp_to_work_area(rect, work)
        } else {
            let (px, py, pw, ph) = clamp_or_center(f32::NAN, f32::NAN, rect.w as f32, rect.h as f32, work);
            Rect { x: px as i32, y: py as i32, w: pw as i32, h: ph as i32 }
        };
        if placed != rect {
            tracing::info!(
                "Chat window moved from {},{} {}x{} to {},{} {}x{} so it fits the work area",
                rect.x, rect.y, rect.w, rect.h, placed.x, placed.y, placed.w, placed.h
            );
        }
        set_hwnd_rect(hwnd, placed);
    }

    fn set_hwnd_rect(hwnd: HWND, outer: Rect) {
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                outer.x,
                outer.y,
                outer.w.max(1),
                outer.h.max(1),
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    pub(super) fn hwnd_of(window: &winit::window::Window) -> Option<HWND> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::Win32(win) = handle.as_raw() else { return None };
        Some(HWND(win.hwnd.get() as *mut _))
    }

    fn cursor_point() -> Option<POINT> {
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt) }.ok()?;
        Some(pt)
    }

    fn main_pjdesk_window() -> Option<HWND> {
        let mut found = HWND::default();
        unsafe {
            let _ = EnumWindows(Some(enum_main), LPARAM(&mut found as *mut HWND as isize));
        }
        if found.0.is_null() {
            None
        } else {
            Some(found)
        }
    }

    unsafe extern "system" fn enum_main(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            if GetWindow(hwnd, GW_OWNER).ok().is_some_and(|owner| !owner.is_invalid()) {
                return BOOL(1);
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n <= 0 {
                return BOOL(1);
            }
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            if title_matches(&title, "PJDesk")
                && !title_matches(&title, "PJDesk Server")
                && !title_matches(&title, "PJDesk Chat")
            {
                let slot = lparam.0 as *mut HWND;
                *slot = hwnd;
                return BOOL(0);
            }
            BOOL(1)
        }
    }

    fn list_areas() -> Vec<Area> {
        let mut areas = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(enum_monitor), LPARAM(&mut areas as *mut Vec<Area> as isize));
        }
        areas
    }

    unsafe extern "system" fn enum_monitor(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<Area>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let ok = unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool();
        if !ok {
            return BOOL(1);
        }
        let mut dpix = 96u32;
        let mut dpiy = 96u32;
        let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpix, &mut dpiy) };
        let scale = (dpix as f32 / 96.0).max(0.5);
        let name = String::from_utf16_lossy(&info.szDevice)
            .trim_end_matches('\0')
            .to_string();
        list.push(Area {
            work: to_rect(info.monitorInfo.rcWork),
            name,
            scale,
            primary: info.monitorInfo.dwFlags & 1 != 0,
            handle: monitor,
        });
        BOOL(1)
    }

    fn to_rect(rect: RECT) -> Rect {
        Rect {
            x: rect.left,
            y: rect.top,
            w: rect.right.saturating_sub(rect.left),
            h: rect.bottom.saturating_sub(rect.top),
        }
    }
}
