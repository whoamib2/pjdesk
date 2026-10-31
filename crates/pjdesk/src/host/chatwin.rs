//! Always-on-top chat window for the person at the computer being controlled.
//!
//! Messages stay in this process for the session. Closing the window does not
//! end the session. A service host that handed the connect notice to the
//! signed-in PJDesk window writes the same lines to a file; that window shows
//! them and writes replies back.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use pjdesk_proto::peer::{chat_line, chat_name, chat_now, ChatEntry};

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
    if let Some(peer) = parsed.iter().rev().find(|l| !l.mine) {
        set_peer(&peer.name);
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
    if !entry.mine {
        set_peer(&entry.name);
    }
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

/// Keeps the compact chat window (its own small process) in step with the
/// transcript. Wakes on every change and at least every 250 ms.
#[cfg_attr(test, allow(dead_code))]
fn ui_thread() {
    #[cfg(windows)]
    THREAD.store(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, Ordering::SeqCst);
    let mut pop = crate::chatpop::ChatPop::new("host");
    let mut shown = false;
    loop {
        if OPEN.load(Ordering::SeqCst) {
            let peer = PEER.lock().unwrap().clone();
            let ready = ARMED.load(Ordering::SeqCst);
            let note = if ready { "" } else { "The session has ended." };
            pop.update(&peer, ready, note, &lines());
            let alert = take_alert();
            if !shown || alert || !pop.is_open() {
                if shown && !alert && pop.take_closed() {
                    user_closed();
                    continue;
                }
                pop.show(alert);
                shown = true;
            }
            for text in pop.take_replies() {
                let _ = queue_reply(&text);
            }
        } else if shown {
            pop.close();
            shown = false;
        }
        wait_for_wake(std::time::Duration::from_millis(250));
    }
}

#[cfg(windows)]
fn wait_for_wake(timeout: std::time::Duration) {
    use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, PeekMessageW, MSG, PM_REMOVE, QS_ALLINPUT};
    unsafe {
        let _ = MsgWaitForMultipleObjects(None, false, timeout.as_millis() as u32, QS_ALLINPUT);
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {}
    }
}

#[cfg(not(windows))]
fn wait_for_wake(timeout: std::time::Duration) {
    std::thread::sleep(timeout);
}

/// Who the person at this computer is chatting with, for the window header.
static PEER: Mutex<String> = Mutex::new(String::new());

pub fn set_peer(name: &str) {
    *PEER.lock().unwrap() = name.to_string();
}

#[cfg(test)]
mod tests {
    use super::*;
    use pjdesk_proto::peer::format_chat_line;

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
