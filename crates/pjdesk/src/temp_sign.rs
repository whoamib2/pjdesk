//! A sign-in that lasts only until this PJDesk window is done.
//!
//! The refresh token stays in memory. The account's saved list, passwords
//! saved during the visit, and recent computers from the visit are removed
//! when the window closes, the Windows session locks or ends, or the computer
//! sits idle. The computer stays in the account's Saved list.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Config;

pub const WARNING_SECS: u64 = 60;

/// Seconds until a temporary sign-in should warn, then end.
/// `None` while the session is still quiet. `Some(0)` means sign out now.
pub fn idle_remaining(last_activity: Instant, minutes: u32, now: Instant) -> Option<u64> {
    let minutes = minutes.clamp(1, 24 * 60);
    let limit = Duration::from_secs(u64::from(minutes) * 60);
    let idle = now.saturating_duration_since(last_activity);
    if idle >= limit {
        return Some(0);
    }
    let left = (limit - idle).as_secs();
    if left <= WARNING_SECS { Some(left) } else { None }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Marker {
    passwords: Vec<String>,
    recents: Vec<String>,
    /// The computer was already signed in before this visit.
    had_sign_in: bool,
}

fn marker_path() -> Option<PathBuf> {
    Config::path().parent().map(|dir| dir.join("temp-session.json"))
}

pub fn marker_active() -> bool {
    marker_path().is_some_and(|path| path.is_file())
}

pub fn begin_marker(had_sign_in: bool) {
    let Some(path) = marker_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let marker = Marker { had_sign_in, ..Marker::default() };
    let _ = std::fs::write(path, serde_json::to_vec(&marker).unwrap_or_default());
}

pub fn marker_ids() -> (Vec<String>, Vec<String>) {
    let marker: Marker = marker_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    (marker.passwords, marker.recents)
}

pub fn note_password(id: &str) {
    note("passwords", id);
}

pub fn note_recent(id: &str) {
    note("recents", id);
}

fn note(which: &str, id: &str) {
    let id = pjdesk_proto::normalize_id(id);
    if !pjdesk_proto::is_valid_id(&id) {
        return;
    }
    let Some(path) = marker_path() else { return };
    let mut marker: Marker = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let list = if which == "passwords" { &mut marker.passwords } else { &mut marker.recents };
    if !list.iter().any(|e| e == &id) {
        list.push(id);
    }
    let _ = std::fs::write(path, serde_json::to_vec(&marker).unwrap_or_default());
}

pub fn clear_marker() {
    if let Some(path) = marker_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Puts the config back to what it was before the temporary sign-in.
/// Passwords and recent rows from the visit are removed even if a viewer
/// wrote them after the snapshot.
pub fn restore_after_temporary(disk: &mut Config, snapshot: &Config, session_passwords: &[String], session_recents: &[String]) {
    for id in disk.saved_passwords.keys().cloned().collect::<Vec<_>>() {
        if !snapshot.saved_passwords.contains_key(&id) {
            disk.saved_passwords.remove(&id);
        }
    }
    for id in session_passwords {
        disk.saved_passwords.remove(id);
    }
    for id in session_recents {
        if let Some(note) = disk.settings.peers.get_mut(id) {
            let before = snapshot.settings.peers.get(id).map(|n| n.last_connected).unwrap_or(0);
            if before == 0 {
                note.last_connected = 0;
            }
        }
    }
    let learned = disk.account_removed.clone();
    disk.address_book = snapshot.address_book.clone();
    disk.saved_hidden = snapshot.saved_hidden.clone();
    disk.account_removed = snapshot.account_removed.clone();
    for id in learned {
        if !disk.account_removed.iter().any(|h| h == &id) {
            disk.account_removed.push(id);
        }
    }
    disk.network_email = snapshot.network_email.clone();
    disk.network_refresh = snapshot.network_refresh.clone();
    disk.network_access = snapshot.network_access.clone();
    disk.network_access_exp = snapshot.network_access_exp;
    disk.network_list_merged = snapshot.network_list_merged;
    disk.network_standby = snapshot.network_standby.clone();
    disk.network_standby_fingerprint = snapshot.network_standby_fingerprint.clone();
    disk.network_lan = snapshot.network_lan.clone();
    disk.network_public_ip = snapshot.network_public_ip.clone();
    disk.server = snapshot.server.clone();
    disk.server_fingerprint = snapshot.server_fingerprint.clone();
    disk.settings.peers = snapshot.settings.peers.clone();
    let gone = disk.account_removed.clone();
    disk.address_book.retain(|e| !gone.iter().any(|h| h == &e.id));
    for id in &gone {
        disk.settings.peers.remove(id);
        disk.saved_passwords.remove(id);
    }
}

/// A killed temporary sign-in left a marker. Drop the passwords and recents it
/// names. The refresh token was never written.
pub fn scrub_abandoned() {
    if !marker_active() {
        return;
    }
    let marker: Marker = marker_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if let Ok(mut disk) = Config::load() {
        for id in &marker.passwords {
            disk.saved_passwords.remove(id);
        }
        for id in &marker.recents {
            if let Some(note) = disk.settings.peers.get_mut(id) {
                note.last_connected = 0;
            }
        }
        if !marker.had_sign_in {
            disk.network_email = None;
            disk.network_refresh = None;
            disk.network_access = None;
            disk.network_access_exp = 0;
        }
        let _ = disk.save();
    }
    clear_marker();
}

/// Tells the window when Windows locks, signs out, or shuts down.
/// One hidden window for the process. A reopened PJDesk window gets the
/// next signal; the old receiver is dropped with the window it belonged to.
#[cfg(windows)]
pub fn watch_session_end() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    if let Ok(mut slot) = SESSION_TX.lock() {
        *slot = Some(tx);
    }
    if !SESSION_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        std::thread::Builder::new().name("session-end".into()).spawn(session_window).ok();
    }
    rx
}

#[cfg(windows)]
static SESSION_TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> = std::sync::Mutex::new(None);
#[cfg(windows)]
static SESSION_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(not(windows))]
pub fn watch_session_end() -> std::sync::mpsc::Receiver<()> {
    let (_tx, rx) = std::sync::mpsc::channel();
    rx
}

#[cfg(windows)]
fn session_window() {
    use windows::core::w;
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE,
        WM_ENDSESSION, WM_QUERYENDSESSION, WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK, WTS_SESSION_LOGOFF,
    };

    unsafe extern "system" fn proc(
        hwnd: windows::Win32::Foundation::HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let end = msg == WM_ENDSESSION
            || msg == WM_QUERYENDSESSION
            || (msg == WM_WTSSESSION_CHANGE && (wparam.0 as u32 == WTS_SESSION_LOCK || wparam.0 as u32 == WTS_SESSION_LOGOFF));
        if end {
            if let Some(tx) = SESSION_TX.lock().unwrap().as_ref() {
                let _ = tx.send(());
            }
        }
        if msg == WM_QUERYENDSESSION {
            return LRESULT(1);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: HINSTANCE(std::ptr::null_mut()),
            lpszClassName: w!("PJDeskSessionEnd"),
            ..WNDCLASSW::default()
        };
        if RegisterClassW(&class) == 0 && windows::Win32::Foundation::GetLastError().0 != 1410 {
            return;
        }
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("PJDeskSessionEnd"),
            w!("PJDesk"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        ) else {
            return;
        };
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_warns_for_the_last_minute_then_ends() {
        let quiet = Instant::now() - Duration::from_secs(28 * 60);
        assert!(idle_remaining(quiet, 30, Instant::now()).is_none());
        let late = Instant::now() - Duration::from_secs(29 * 60 + 30);
        let left = idle_remaining(late, 30, Instant::now()).unwrap();
        assert!(left > 0 && left <= WARNING_SECS);
        let done = Instant::now() - Duration::from_secs(31 * 60);
        assert_eq!(idle_remaining(done, 30, Instant::now()), Some(0));
    }

    #[test]
    fn restore_drops_the_visit_and_keeps_the_earlier_list() {
        let mut snapshot = Config::default();
        snapshot.address_book = vec![pjdesk_proto::signal::AddressEntry {
            id: "111111111".into(),
            alias: "Mine".into(),
            ..Default::default()
        }];
        snapshot.saved_passwords.insert("111111111".into(), "kept".into());
        snapshot.network_email = None;
        let mut disk = snapshot.clone();
        disk.address_book.push(pjdesk_proto::signal::AddressEntry {
            id: "692996394".into(),
            alias: "Ben".into(),
            ..Default::default()
        });
        disk.network_email = Some("ben@spader.us".into());
        disk.network_refresh = Some("plain:token".into());
        disk.saved_passwords.insert("222222222".into(), "session".into());
        disk.settings.peers.insert(
            "222222222".into(),
            crate::config::PeerNote { last_connected: 9, ..Default::default() },
        );
        restore_after_temporary(&mut disk, &snapshot, &["222222222".into()], &["222222222".into()]);
        assert_eq!(disk.address_book.len(), 1);
        assert_eq!(disk.address_book[0].id, "111111111");
        assert!(disk.network_email.is_none());
        assert!(disk.network_refresh.is_none());
        assert!(disk.saved_passwords.contains_key("111111111"));
        assert!(!disk.saved_passwords.contains_key("222222222"));
        assert_eq!(disk.settings.peers.get("222222222").map(|n| n.last_connected).unwrap_or(0), 0);
    }
}
