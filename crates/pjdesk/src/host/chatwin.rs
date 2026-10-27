//! Always-on-top chat window for the person at the computer being controlled.
//!
//! Messages stay in this process for the session. Closing the window does not
//! end the session. A service host that handed the connect notice to the
//! signed-in PJDesk window writes the same lines to a file; that window shows
//! them and writes replies back.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use pjdesk_proto::peer::{chat_line, chat_name, chat_now, format_chat_line, ChatEntry};

struct Log {
    lines: Vec<ChatEntry>,
    seq: u64,
    /// The newest change is a message from the other computer.
    alert: bool,
}

static LOG: Mutex<Log> = Mutex::new(Log { lines: Vec::new(), seq: 0, alert: false });
static OUT: Mutex<Vec<ChatEntry>> = Mutex::new(Vec::new());
static OPEN: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
/// Service process: the signed-in window draws the chat.
static HANDED: AtomicBool = AtomicBool::new(false);
/// Signed-in window: this chat belongs to the service session.
static BRIDGE: AtomicBool = AtomicBool::new(false);
static ALERT: AtomicBool = AtomicBool::new(false);
static SEEN: AtomicU64 = AtomicU64::new(0);
static THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

const WM_APP_REFRESH: u32 = 0x8000;

/// Clears chat when the session task ends. Closing the window does not drop this.
pub struct Guard;

impl Guard {
    pub fn begin(handed: bool) -> Self {
        begin(handed);
        Guard
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        end();
    }
}

/// Starts a session with an empty transcript. `handed` means the signed-in
/// PJDesk window will draw the chat.
pub fn begin(handed: bool) {
    HANDED.store(handed, Ordering::SeqCst);
    BRIDGE.store(false, Ordering::SeqCst);
    ARMED.store(false, Ordering::SeqCst);
    OPEN.store(false, Ordering::SeqCst);
    ALERT.store(false, Ordering::SeqCst);
    SEEN.store(0, Ordering::SeqCst);
    *LOG.lock().unwrap() = Log { lines: Vec::new(), seq: 0, alert: false };
    OUT.lock().unwrap().clear();
    let _ = std::fs::remove_file(snapshot_path());
    let _ = std::fs::remove_file(reply_path());
    wake();
}

/// The other computer speaks 0.6.50 chat. The person here can open the window first.
pub fn arm() {
    ARMED.store(true, Ordering::SeqCst);
    publish();
    wake();
}

pub fn armed() -> bool {
    ARMED.load(Ordering::SeqCst)
}

/// The person at this computer opened chat. Does nothing until [`arm`].
/// Does not end the session.
pub fn open() {
    if !ARMED.load(Ordering::SeqCst) {
        return;
    }
    if HANDED.load(Ordering::SeqCst) {
        return;
    }
    OPEN.store(true, Ordering::SeqCst);
    ensure_thread();
    wake();
}

/// Hides the window. The session keeps running and the lines stay.
pub fn user_closed() {
    OPEN.store(false, Ordering::SeqCst);
    wake();
}

/// True only when the session itself is over. Closing the window leaves this false.
pub fn session_over() -> bool {
    false
}

/// Drops the transcript. Called when the session ends, not when the window closes.
pub fn end() {
    begin(false);
}

/// A line from the other computer. Opens the window and asks for a sound and a flash.
pub fn incoming(entry: ChatEntry) {
    push(entry, true);
}

/// A line this computer already queued on the session. Shown, not sent again.
pub fn note(entry: ChatEntry) {
    push(entry, false);
}

/// The service sent a reply that the signed-in window typed. Fold it into the
/// transcript the window is reading. An in-process window already stored it.
pub fn note_sent(entry: ChatEntry) {
    if HANDED.load(Ordering::SeqCst) {
        push(entry, false);
    }
}

/// The person at this computer typed a reply in the chat window.
pub fn queue_reply(text: &str) -> Option<ChatEntry> {
    let text = chat_line(text)?;
    let entry = ChatEntry {
        mine: true,
        name: chat_name(&crate::hostname(), "Computer"),
        text,
        unix: chat_now(),
    };
    push(entry.clone(), false);
    if BRIDGE.load(Ordering::SeqCst) {
        push_reply_file(&entry);
    } else if !HANDED.load(Ordering::SeqCst) {
        OUT.lock().unwrap().push(entry.clone());
    }
    Some(entry)
}

pub fn lines() -> Vec<ChatEntry> {
    LOG.lock().unwrap().lines.clone()
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

/// Replies the session should write, including ones the signed-in window saved.
pub fn take_outgoing() -> Vec<ChatEntry> {
    let mut lines = std::mem::take(&mut *OUT.lock().unwrap());
    lines.extend(take_reply_file());
    lines
}

/// True once, when a new incoming line should beep and flash.
pub fn take_alert() -> bool {
    ALERT.swap(false, Ordering::SeqCst)
}

/// The signed-in window reads the service's transcript.
pub fn adopt() {
    let Ok(bytes) = std::fs::read(snapshot_path()) else {
        if BRIDGE.swap(false, Ordering::SeqCst) {
            OPEN.store(false, Ordering::SeqCst);
            ARMED.store(false, Ordering::SeqCst);
            *LOG.lock().unwrap() = Log { lines: Vec::new(), seq: 0, alert: false };
            wake();
        }
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    let seq = value.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
    let ready = value.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
    let alert = value.get("alert").and_then(|v| v.as_bool()).unwrap_or(false);
    let parsed = value
        .get("lines")
        .and_then(|v| serde_json::from_value::<Vec<ChatEntry>>(v.clone()).ok())
        .unwrap_or_default();
    BRIDGE.store(true, Ordering::SeqCst);
    ARMED.store(ready, Ordering::SeqCst);
    if SEEN.swap(seq, Ordering::SeqCst) == seq {
        return;
    }
    LOG.lock().unwrap().lines = parsed;
    LOG.lock().unwrap().seq = seq;
    if alert {
        OPEN.store(true, Ordering::SeqCst);
        ALERT.store(true, Ordering::SeqCst);
        ensure_thread();
    }
    wake();
}

fn push(entry: ChatEntry, incoming: bool) {
    {
        let mut log = LOG.lock().unwrap();
        log.lines.push(entry);
        if log.lines.len() > 200 {
            let drop_n = log.lines.len() - 200;
            log.lines.drain(0..drop_n);
        }
        log.seq = log.seq.wrapping_add(1);
        log.alert = incoming;
    }
    if incoming && !HANDED.load(Ordering::SeqCst) {
        OPEN.store(true, Ordering::SeqCst);
        ALERT.store(true, Ordering::SeqCst);
        ensure_thread();
    }
    publish();
    wake();
}

fn publish() {
    if !HANDED.load(Ordering::SeqCst) {
        return;
    }
    let log = LOG.lock().unwrap();
    let body = serde_json::json!({
        "ready": ARMED.load(Ordering::SeqCst),
        "seq": log.seq,
        "alert": log.alert,
        "lines": log.lines,
    });
    drop(log);
    let path = snapshot_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, body.to_string());
}

fn snapshot_path() -> PathBuf {
    crate::syspaths::service_dir().join("session-chat.json")
}

fn reply_path() -> PathBuf {
    crate::syspaths::service_dir().join("session-chat-out.json")
}

fn push_reply_file(entry: &ChatEntry) {
    let mut lines = read_replies();
    lines.push(entry.clone());
    let path = reply_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, serde_json::to_string(&lines).unwrap_or_else(|_| "[]".into()));
}

fn take_reply_file() -> Vec<ChatEntry> {
    let lines = read_replies();
    if !lines.is_empty() {
        let _ = std::fs::remove_file(reply_path());
    }
    lines
}

fn read_replies() -> Vec<ChatEntry> {
    let Ok(bytes) = std::fs::read(reply_path()) else { return Vec::new() };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn wake() {
    let id = THREAD.load(Ordering::SeqCst);
    if id == 0 {
        return;
    }
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
            id,
            WM_APP_REFRESH,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
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
            let _ = std::thread::Builder::new().name("chat".into()).spawn(ui_thread);
        });
    }
}

#[cfg(not(windows))]
#[cfg_attr(test, allow(dead_code))]
fn ui_thread() {
    loop {
        let _ = lines();
        std::thread::sleep(std::time::Duration::from_secs(60));
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
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_RETURN};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FlashWindow, GetClientRect, GetMessageW,
        PostQuitMessage, RegisterClassW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, HWND_TOPMOST, MSG,
        SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CHAR, WM_CLOSE,
        WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_PAINT, WNDCLASSW,
    };

    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_MINIMIZEBOX: u32 = 0x0002_0000;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_APPWINDOW: u32 = 0x0004_0000;

    static DRAFT: Mutex<String> = Mutex::new(String::new());

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_CLOSE => {
                user_closed();
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if wparam.0 as u16 == VK_BACK.0 {
                    DRAFT.lock().unwrap().pop();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                } else if wparam.0 as u16 == VK_RETURN.0 {
                    send_draft();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_CHAR => {
                let unit = wparam.0 as u32;
                if unit >= 32 && unit != 127 {
                    if let Some(ch) = char::from_u32(unit) {
                        let mut draft = DRAFT.lock().unwrap();
                        if draft.chars().count() < pjdesk_proto::peer::MAX_CHAT_TEXT {
                            draft.push(ch);
                        }
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32);
                if hit_send(hwnd, x, y) {
                    send_draft();
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
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

    fn send_draft() {
        let text = std::mem::take(&mut *DRAFT.lock().unwrap());
        let _ = queue_reply(&text);
    }

    unsafe fn dpi(hwnd: HWND) -> i32 {
        let value = GetDpiForWindow(hwnd) as i32;
        if value > 0 { value } else { 96 }
    }

    unsafe fn scale(hwnd: HWND, px: i32) -> i32 {
        px * dpi(hwnd) / 96
    }

    unsafe fn hit_send(hwnd: HWND, x: i32, y: i32) -> bool {
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let r = send_rect(&rect, dpi(hwnd));
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    }

    fn send_rect(client: &RECT, dpi: i32) -> RECT {
        let s = |px: i32| px * dpi / 96;
        RECT { left: client.right - s(88), top: client.bottom - s(40), right: client.right - s(12), bottom: client.bottom - s(10) }
    }

    unsafe fn place(hwnd: HWND) {
        let w = scale(hwnd, 420);
        let h = scale(hwnd, 320);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..MONITORINFO::default() };
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        let (right, top) = if GetMonitorInfoW(mon, &mut info).as_bool() {
            (info.rcWork.right, info.rcWork.top)
        } else {
            (scale(hwnd, 1920), 0)
        };
        let margin = scale(hwnd, 12);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), right - w - margin, top + scale(hwnd, 150), w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }

    unsafe fn paint(hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        if dc.is_invalid() {
            return;
        }
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let bg = CreateSolidBrush(COLORREF(0x00_22_1b_16));
        let _ = FillRect(dc, &rect, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));
        let _ = SetBkMode(dc, TRANSPARENT);
        let d = dpi(hwnd);
        let lines = lines();
        let start = lines.len().saturating_sub(8);
        let mut y = rect.top + scale(hwnd, 8);
        for line in lines.iter().skip(start) {
            let shown = format_chat_line(line.unix, &line.name, &line.text);
            let shown: String = shown.chars().take(72).collect();
            let _ = SetTextColor(dc, COLORREF(if line.mine { 0x00_ff_e3_cf } else { 0x00_f2_ec_e8 }));
            text(
                dc,
                &shown,
                RECT { left: scale(hwnd, 12), top: y, right: rect.right - scale(hwnd, 12), bottom: y + scale(hwnd, 22) },
                -(13 * d / 96),
            );
            y += scale(hwnd, 22);
        }
        let draft_rect = RECT { left: scale(hwnd, 12), top: rect.bottom - scale(hwnd, 40), right: rect.right - scale(hwnd, 96), bottom: rect.bottom - scale(hwnd, 10) };
        let draft_bg = CreateSolidBrush(COLORREF(0x00_37_2c_26));
        let _ = FillRect(dc, &draft_rect, draft_bg);
        let _ = DeleteObject(HGDIOBJ(draft_bg.0));
        let draft = DRAFT.lock().unwrap().clone();
        let shown = if draft.is_empty() { "Message".to_string() } else { draft.chars().rev().take(48).collect::<String>().chars().rev().collect() };
        let _ = SetTextColor(dc, COLORREF(0x00_f2_ec_e8));
        text(dc, &shown, draft_rect, -(13 * d / 96));
        let btn = send_rect(&rect, d);
        let blue = CreateSolidBrush(COLORREF(0x00_ff_9b_4d));
        let _ = FillRect(dc, &btn, blue);
        let _ = DeleteObject(HGDIOBJ(blue.0));
        let _ = SetTextColor(dc, COLORREF(0x00_ff_ff_ff));
        text(dc, "Send", btn, -(13 * d / 96));
        let _ = EndPaint(hwnd, &ps);
    }

    unsafe fn text(dc: windows::Win32::Graphics::Gdi::HDC, value: &str, mut rect: RECT, height: i32) {
        let font = CreateFontW(height, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY, 0, w!("Segoe UI"));
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = DrawTextW(dc, &mut wide, &mut rect, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        let _ = SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }

    unsafe fn refresh(hwnd: HWND) {
        let title: Vec<u16> = "Chat".encode_utf16().chain(std::iter::once(0)).collect();
        let _ = SetWindowTextW(hwnd, windows::core::PCWSTR(title.as_ptr()));
        if OPEN.load(Ordering::SeqCst) {
            place(hwnd);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE);
            if take_alert() {
                let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(
                    windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
                );
                let _ = FlashWindow(hwnd, true);
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }

    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: w!("PJDeskChat"),
            ..WNDCLASSW::default()
        };
        if RegisterClassW(&class) == 0 && windows::Win32::Foundation::GetLastError().0 != 1410 {
            tracing::warn!("Could not register the chat window");
            return;
        }
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOPMOST | WS_EX_APPWINDOW),
            w!("PJDeskChat"),
            w!("Chat"),
            WINDOW_STYLE(WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX),
            0,
            0,
            10,
            10,
            None,
            None,
            None,
            None,
        ) else {
            tracing::warn!("Could not create the chat window");
            return;
        };
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

    static TEST: Mutex<()> = Mutex::new(());

    #[test]
    fn closing_the_window_keeps_the_session_and_the_lines() {
        let _hold = TEST.lock().unwrap();
        begin(false);
        assert!(queue_reply("  ").is_none());
        incoming(ChatEntry { mine: false, name: "ben@spader.us".into(), text: "hello".into(), unix: 1700000000 });
        assert!(format_chat_line(1700000000, "ben@spader.us", "hello").contains("22:13:20"));
        assert!(is_open());
        assert!(take_alert());
        assert!(!take_alert());
        user_closed();
        assert!(!is_open());
        assert!(!session_over());
        assert_eq!(lines().len(), 1);
        assert!(queue_reply("on my way").is_some());
        let out = take_outgoing();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "on my way");
        assert!(!session_over());
        end();
        assert!(lines().is_empty());
    }

    #[test]
    fn a_handed_session_writes_the_file_and_does_not_open_here() {
        let _hold = TEST.lock().unwrap();
        begin(true);
        arm();
        incoming(ChatEntry { mine: false, name: "Ben".into(), text: "ping".into(), unix: 1700000000 });
        assert!(!is_open());
        let bytes = std::fs::read(snapshot_path()).expect("snapshot");
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["ready"], true);
        assert_eq!(value["alert"], true);
        assert_eq!(value["lines"][0]["text"], "ping");
        end();
        assert!(!snapshot_path().exists());
    }

    #[test]
    fn a_handed_reply_is_folded_into_the_snapshot_once() {
        let _hold = TEST.lock().unwrap();
        begin(false);
        let reply = queue_reply("local").expect("reply");
        note_sent(reply);
        assert_eq!(lines().len(), 1);
        assert_eq!(take_outgoing().len(), 1);

        begin(true);
        arm();
        incoming(ChatEntry { mine: false, name: "Ben".into(), text: "ping".into(), unix: 1700000000 });
        note_sent(ChatEntry { mine: true, name: "OFFICE".into(), text: "pong".into(), unix: 1700000001 });
        let bytes = std::fs::read(snapshot_path()).expect("snapshot");
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["lines"].as_array().unwrap().len(), 2);
        assert_eq!(value["lines"][1]["text"], "pong");
        assert_eq!(value["alert"], false);
        end();
        assert!(!snapshot_path().exists());
    }
}
