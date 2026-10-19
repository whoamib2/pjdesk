//! Small always-on-top notice on the computer being controlled.
//!
//! It opens at the top-right of the primary screen when a session starts,
//! names who connected, and offers Disconnect. After two seconds it collapses
//! to a pill. It does not take focus, and it asks Windows to leave it out of
//! screen capture. The window lives in this process, which is the signed-in
//! session when the service is hosting.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
#[cfg(not(windows))]
use std::time::Duration;

use pjdesk_proto::peer::{visitor_card, VisitorCard};

#[derive(Clone, Debug, Default)]
struct Who {
    account: String,
    computer: String,
    user: String,
    id: String,
}

struct Slot {
    who: Option<Who>,
    seq: u64,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot { who: None, seq: 0 });
static DISCONNECT: AtomicBool = AtomicBool::new(false);
static THREAD: AtomicU32 = AtomicU32::new(0);
#[cfg_attr(not(windows), allow(dead_code))]
static LAST_SEQ: AtomicU64 = AtomicU64::new(0);

const WM_APP_REFRESH: u32 = 0x8000;

/// Hides the notice when the session task ends, including on a disconnect.
pub struct Guard;

impl Guard {
    pub fn begin(computer: &str) -> Self {
        begin(computer);
        Guard
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        end();
    }
}

/// Shows the notice for `computer` until [`update`] or [`end`].
pub fn begin(computer: &str) {
    DISCONNECT.store(false, Ordering::SeqCst);
    publish(Who { computer: computer.to_string(), ..Who::default() });
}

/// Replaces the notice with the account, computer, user, and ID from the controller.
pub fn update(account: &str, computer: &str, user: &str, id: &str) {
    let mut who = Who { account: account.to_string(), computer: computer.to_string(), user: user.to_string(), id: id.to_string() };
    if who.computer.trim().is_empty() {
        if let Some(prev) = SLOT.lock().unwrap().who.clone() {
            if !prev.computer.trim().is_empty() {
                who.computer = prev.computer;
            }
        }
    }
    publish(who);
}

/// Hides the notice. The session is over.
pub fn end() {
    let mut slot = SLOT.lock().unwrap();
    slot.who = None;
    slot.seq = slot.seq.wrapping_add(1);
    drop(slot);
    wake();
}

/// True once, when the person at this computer pressed Disconnect.
pub fn disconnect_requested() -> bool {
    DISCONNECT.swap(false, Ordering::SeqCst)
}

fn publish(who: Who) {
    {
        let mut slot = SLOT.lock().unwrap();
        slot.who = Some(who);
        slot.seq = slot.seq.wrapping_add(1);
    }
    ensure_thread();
    wake();
}

fn wake() {
    let id = THREAD.load(Ordering::SeqCst);
    if id == 0 {
        return;
    }
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(id, WM_APP_REFRESH, windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0));
    }
}

fn ensure_thread() {
    #[cfg(test)]
    {
        return;
    }
    #[cfg(not(test))]
    {
        static START: std::sync::Once = std::sync::Once::new();
        START.call_once(|| {
            let _ = std::thread::Builder::new().name("notice".into()).spawn(ui_thread);
        });
    }
}

fn current_card() -> Option<VisitorCard> {
    let who = SLOT.lock().unwrap().who.clone()?;
    Some(visitor_card(&who.account, &who.computer, &who.user, &who.id))
}

#[cfg(not(windows))]
#[cfg_attr(test, allow(dead_code))]
fn ui_thread() {
    loop {
        let _ = current_card();
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[cfg(windows)]
fn ui_thread() {
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetMonitorInfoW,
        InvalidateRect, MonitorFromWindow, SelectObject, SetBkMode, SetTextColor, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
        DEFAULT_QUALITY, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
        OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
        KillTimer, PostQuitMessage, RegisterClassW, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
        TranslateMessage, HWND_TOPMOST, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE,
        WINDOW_DISPLAY_AFFINITY, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONUP,
        WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WNDCLASSW,
    };

    const WS_POPUP: u32 = 0x8000_0000;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
    const MA_NOACTIVATE: isize = 3;
    struct Ui {
        collapsed: bool,
        armed: bool,
    }
    unsafe impl Send for Ui {}

    static UI: Mutex<Option<Ui>> = Mutex::new(None);

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE),
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER => {
                if let Some(ui) = UI.lock().unwrap().as_mut() {
                    ui.collapsed = true;
                    ui.armed = false;
                }
                let _ = KillTimer(Some(hwnd), 1);
                place(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let collapsed = UI.lock().unwrap().as_ref().map(|u| u.collapsed).unwrap_or(true);
                let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
                if collapsed {
                    if let Some(ui) = UI.lock().unwrap().as_mut() {
                        ui.collapsed = false;
                    }
                    place(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                } else if hit_disconnect(hwnd, x, y) {
                    DISCONNECT.store(true, Ordering::SeqCst);
                } else if hit_collapse(hwnd, x, y) {
                    if let Some(ui) = UI.lock().unwrap().as_mut() {
                        ui.collapsed = true;
                    }
                    place(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn dpi(hwnd: HWND) -> i32 {
        let value = GetDpiForWindow(hwnd) as i32;
        if value > 0 { value } else { 96 }
    }

    unsafe fn scale(hwnd: HWND, px: i32) -> i32 {
        px * dpi(hwnd) / 96
    }

    unsafe fn place(hwnd: HWND) {
        let collapsed = UI.lock().unwrap().as_ref().map(|u| u.collapsed).unwrap_or(false);
        let (w, h) = if collapsed { (scale(hwnd, 168), scale(hwnd, 32)) } else { (scale(hwnd, 400), scale(hwnd, 128)) };
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..MONITORINFO::default() };
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        let (right, top) = if GetMonitorInfoW(mon, &mut info).as_bool() {
            (info.rcWork.right, info.rcWork.top)
        } else {
            (scale(hwnd, 1920), 0)
        };
        let margin = scale(hwnd, 12);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            right - w - margin,
            top + margin,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }

    unsafe fn hit_disconnect(hwnd: HWND, x: i32, y: i32) -> bool {
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let r = disconnect_rect(&rect, dpi(hwnd));
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    }

    unsafe fn hit_collapse(hwnd: HWND, x: i32, y: i32) -> bool {
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        x > rect.right - scale(hwnd, 36) && y < scale(hwnd, 28)
    }

    fn disconnect_rect(client: &RECT, dpi: i32) -> RECT {
        let s = |px: i32| px * dpi / 96;
        RECT { left: client.right - s(118), top: client.bottom - s(36), right: client.right - s(12), bottom: client.bottom - s(10) }
    }

    unsafe fn paint(hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        if dc.is_invalid() {
            return;
        }
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let bg = CreateSolidBrush(COLORREF(0x00_30_22_1b));
        let _ = FillRect(dc, &rect, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));
        let card = current_card();
        let collapsed = UI.lock().unwrap().as_ref().map(|u| u.collapsed).unwrap_or(false);
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, COLORREF(0x00_f2_ec_e8));
        let d = dpi(hwnd);
        if collapsed {
            let label = card.as_ref().map(|c| c.headline.clone()).unwrap_or_else(|| "PJDesk".into());
            let shown: String = label.chars().take(18).collect();
            text(dc, &shown, RECT { left: rect.left + scale(hwnd, 12), top: rect.top, right: rect.right - scale(hwnd, 8), bottom: rect.bottom }, -(13 * d / 96), true);
        } else if let Some(card) = card {
            let mut y = rect.top + scale(hwnd, 10);
            text(dc, &card.headline, RECT { left: scale(hwnd, 14), top: y, right: rect.right - scale(hwnd, 36), bottom: y + scale(hwnd, 24) }, -(16 * d / 96), true);
            y += scale(hwnd, 24);
            let _ = SetTextColor(dc, COLORREF(0x00_c0_b4_a8));
            if !card.secondary.is_empty() {
                text(dc, &card.secondary, RECT { left: scale(hwnd, 14), top: y, right: rect.right - scale(hwnd, 12), bottom: y + scale(hwnd, 20) }, -(13 * d / 96), false);
                y += scale(hwnd, 20);
            }
            if !card.detail.is_empty() {
                text(dc, &card.detail, RECT { left: scale(hwnd, 14), top: y, right: rect.right - scale(hwnd, 12), bottom: y + scale(hwnd, 20) }, -(12 * d / 96), false);
            }
            let btn = disconnect_rect(&rect, d);
            let red = CreateSolidBrush(COLORREF(0x00_3b_3b_c4));
            let _ = FillRect(dc, &btn, red);
            let _ = DeleteObject(HGDIOBJ(red.0));
            let _ = SetTextColor(dc, COLORREF(0x00_ff_ff_ff));
            text(dc, "Disconnect", btn, -(13 * d / 96), true);
            let _ = SetTextColor(dc, COLORREF(0x00_c0_b4_a8));
            text(dc, "–", RECT { left: rect.right - scale(hwnd, 28), top: scale(hwnd, 6), right: rect.right - scale(hwnd, 8), bottom: scale(hwnd, 26) }, -(16 * d / 96), true);
        }
        let _ = EndPaint(hwnd, &ps);
    }

    unsafe fn text(dc: windows::Win32::Graphics::Gdi::HDC, value: &str, mut rect: RECT, height: i32, single: bool) {
        let font = CreateFontW(
            height,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            0,
            w!("Segoe UI"),
        );
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let flags = if single { DT_LEFT | DT_SINGLELINE | DT_VCENTER } else { DT_LEFT | DT_SINGLELINE | DT_VCENTER };
        let _ = DrawTextW(dc, &mut wide, &mut rect, flags);
        let _ = SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }

    unsafe fn refresh(hwnd: HWND) {
        let showing = SLOT.lock().unwrap().who.is_some();
        if showing {
            let seq = SLOT.lock().unwrap().seq;
            let fresh = LAST_SEQ.swap(seq, Ordering::SeqCst) != seq;
            if fresh {
                if let Some(ui) = UI.lock().unwrap().as_mut() {
                    if !ui.armed {
                        ui.collapsed = false;
                        ui.armed = true;
                        let _ = SetTimer(Some(hwnd), 1, 2000, None);
                    }
                }
            }
            place(hwnd);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        } else {
            if let Some(ui) = UI.lock().unwrap().as_mut() {
                ui.armed = false;
                ui.collapsed = false;
            }
            let _ = KillTimer(Some(hwnd), 1);
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }

    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: w!("PJDeskConnectNotice"),
            ..WNDCLASSW::default()
        };
        if RegisterClassW(&class) == 0 && windows::Win32::Foundation::GetLastError().0 != 1410 {
            tracing::warn!("Could not register the connect notice window");
            return;
        }
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE),
            w!("PJDeskConnectNotice"),
            w!("PJDesk"),
            WINDOW_STYLE(WS_POPUP),
            0,
            0,
            10,
            10,
            None,
            None,
            None,
            None,
        ) else {
            tracing::warn!("Could not create the connect notice window");
            return;
        };
        let _ = SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(0x11));
        *UI.lock().unwrap() = Some(Ui { collapsed: false, armed: false });
        THREAD.store(windows::Win32::System::Threading::GetCurrentThreadId(), Ordering::SeqCst);
        refresh(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_APP_REFRESH {
                refresh(hwnd);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = DestroyWindow(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_is_reported_once() {
        assert!(!disconnect_requested());
        DISCONNECT.store(true, Ordering::SeqCst);
        assert!(disconnect_requested());
        assert!(!disconnect_requested());
    }

    #[test]
    fn begin_and_end_publish_a_session() {
        begin("BENLENOVODESK");
        let card = current_card().expect("notice");
        assert_eq!(card.headline, "BENLENOVODESK");
        update("ben@spader.us", "", "Ben", "692996394");
        let card = current_card().expect("notice");
        assert_eq!(card.headline, "ben@spader.us");
        assert_eq!(card.secondary, "BENLENOVODESK");
        end();
        assert!(current_card().is_none());
    }
}
