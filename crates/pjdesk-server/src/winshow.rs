//! Bring this app's window forward from the tray.
//!
//! Windows only lets `SetForegroundWindow` succeed for a moment after the
//! click. Waiting for the next frame is why the window often stays down.
//! `ShowWindow(SW_RESTORE)` is what actually undoes a minimize. Looking the
//! window up by its title avoids a handle kept after the window was destroyed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

static OPENED: Mutex<Option<Instant>> = Mutex::new(None);

pub fn note_user_open() {
    if let Ok(mut opened) = OPENED.lock() {
        *opened = Some(Instant::now());
    }
}

/// True for a short time after a tray open, so hide-to-tray does not undo it.
pub fn user_just_opened() -> bool {
    OPENED.lock().ok().and_then(|opened| *opened).is_some_and(|at| at.elapsed() < Duration::from_millis(1500))
}

pub fn activate(reason: &str, title: &str, owns_gui: bool) {
    note_user_open();
    #[cfg(windows)]
    imp::activate(reason, title, owns_gui);
    #[cfg(not(windows))]
    {
        let _ = (reason, title, owns_gui);
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
        GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, MoveWindow, SetForegroundWindow,
        SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE, SW_SHOW,
        WS_EX_TOOLWINDOW,
    };

    use super::note_user_open;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    static LAST_LAUNCH: Mutex<Option<Instant>> = Mutex::new(None);

    struct Query {
        title: Vec<u16>,
        ours: u32,
        ours_hwnd: Option<HWND>,
        other_hwnd: Option<HWND>,
    }

    struct Area {
        full: pjdesk_proto::trayshow::Rect,
        work: pjdesk_proto::trayshow::Rect,
        primary: bool,
    }

    pub fn activate(reason: &str, title: &str, owns_gui: bool) {
        note_user_open();
        tracing::info!("Tray: {reason}. Looking up {title} by its title.");
        match find(title) {
            Some((hwnd, pid)) if unsafe { IsWindow(Some(hwnd)) }.as_bool() => {
                let ours = unsafe { GetCurrentProcessId() };
                tracing::info!("Tray: {title} window is process {pid} (this process is {ours}).");
                restore(title, hwnd);
            }
            Some(_) => {
                tracing::info!("Tray: the {title} window handle is stale. It was closed while the tray stayed.");
                start_or_show(title, owns_gui);
            }
            None => {
                tracing::info!("Tray: no {title} window is open.");
                start_or_show(title, owns_gui);
            }
        }
    }

    fn start_or_show(title: &str, owns_gui: bool) {
        // No live window: a different process starts the program. This process
        // already is the program, so it shows the window it still owns.
        if owns_gui {
            tracing::info!("Tray: this process is {title}. The window will be shown again.");
            return;
        }
        let recently = LAST_LAUNCH.lock().ok().and_then(|at| *at).is_some_and(|at| at.elapsed() < Duration::from_secs(2));
        if recently {
            tracing::info!("Tray: {title} was already started.");
            return;
        }
        if let Ok(mut last) = LAST_LAUNCH.lock() {
            *last = Some(Instant::now());
        }
        let Ok(exe) = std::env::current_exe() else {
            tracing::info!("Tray: could not find the {title} program.");
            return;
        };
        tracing::info!("Tray: the {title} window is not running. Starting {}.", exe.display());
        let mut cmd = std::process::Command::new(&exe);
        cmd.creation_flags(DETACHED_PROCESS);
        match cmd.spawn() {
            Ok(child) => tracing::info!("Tray: started {} (pid {}).", exe.display(), child.id()),
            Err(err) => tracing::info!("Tray: could not start {title}: {err}"),
        }
    }

    fn find(title: &str) -> Option<(HWND, u32)> {
        let mut query = Query {
            title: title.encode_utf16().collect(),
            ours: unsafe { GetCurrentProcessId() },
            ours_hwnd: None,
            other_hwnd: None,
        };
        unsafe {
            let _ = EnumWindows(Some(enum_window), LPARAM(&mut query as *mut Query as isize));
        }
        let hwnd = query.ours_hwnd.or(query.other_hwnd)?;
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        Some((hwnd, pid))
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, data: LPARAM) -> BOOL {
        let query = unsafe { &mut *(data.0 as *mut Query) };
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return BOOL(1);
        }
        let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return BOOL(1);
        }
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
        if n <= 0 || buf[..n as usize] != query.title {
            return BOOL(1);
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == query.ours {
            query.ours_hwnd = Some(hwnd);
        } else if query.other_hwnd.is_none() {
            query.other_hwnd = Some(hwnd);
        }
        BOOL(1)
    }

    fn restore(title: &str, hwnd: HWND) {
        unsafe {
            let minimized = IsIconic(hwnd).as_bool();
            let visible = IsWindowVisible(hwnd).as_bool();
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            tracing::info!(
                "Tray: {title} is minimized={minimized} visible={visible} at {},{} {}x{}. {}",
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                if pjdesk_proto::trayshow::needs_restore(minimized, visible) {
                    "Restoring it."
                } else {
                    "Bringing it forward."
                }
            );
            let _ = AllowSetForegroundWindow(0xFFFF_FFFF);
            let _ = ShowWindow(hwnd, SW_RESTORE);
            if !IsWindowVisible(hwnd).as_bool() {
                tracing::info!("Tray: {title} was still hidden after SW_RESTORE. Showing it.");
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            let fg = GetForegroundWindow();
            let fg_thread = if fg.0.is_null() { 0 } else { GetWindowThreadProcessId(fg, None) };
            let us = GetCurrentThreadId();
            let attached = fg_thread != 0 && fg_thread != us && AttachThreadInput(fg_thread, us, true).as_bool();
            let brought = SetForegroundWindow(hwnd).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
            if attached {
                let _ = AttachThreadInput(fg_thread, us, false);
            }
            tracing::info!(
                "Tray: ShowWindow(SW_RESTORE) and SetForegroundWindow for {title} (attach={attached}, foreground={brought})."
            );
            move_if_off_screen(title, hwnd);
        }
    }

    fn move_if_off_screen(title: &str, hwnd: HWND) {
        let mut areas: Vec<Area> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(enum_monitor), LPARAM(&mut areas as *mut Vec<Area> as isize));
        }
        if areas.is_empty() {
            tracing::info!("Tray: no monitor list, so {title} was left where it is.");
            return;
        }
        areas.sort_by_key(|area| !area.primary);
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        let mut window = to_rect(rect);
        let monitors: Vec<_> = areas.iter().map(|area| area.full).collect();
        if pjdesk_proto::trayshow::on_any_monitor(window, &monitors) {
            return;
        }
        if window.w < 200 || window.h < 200 {
            window.w = 960;
            window.h = 640;
        }
        let placed = pjdesk_proto::trayshow::place_on_work_area(window, areas[0].work);
        tracing::info!(
            "Tray: {title} was off-screen at {},{}. Moving it to {},{}.",
            window.x,
            window.y,
            placed.x,
            placed.y
        );
        let _ = unsafe { MoveWindow(hwnd, placed.x, placed.y, placed.w, placed.h, true) };
    }

    unsafe extern "system" fn enum_monitor(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<Area>) };
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            list.push(Area {
                full: to_rect(info.rcMonitor),
                work: to_rect(info.rcWork),
                primary: info.dwFlags & 1 != 0,
            });
        }
        BOOL(1)
    }

    fn to_rect(rect: RECT) -> pjdesk_proto::trayshow::Rect {
        pjdesk_proto::trayshow::Rect {
            x: rect.left,
            y: rect.top,
            w: rect.right.saturating_sub(rect.left),
            h: rect.bottom.saturating_sub(rect.top),
        }
    }
}
