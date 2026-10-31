//! Files and the session companion on the controlled computer.
//!
//! When a session starts, a small minimized "PJDesk – X is connected" window
//! appears on the taskbar with Chat and Send files. It closes when the session
//! ends. The Files window (the same one the viewer uses) opens when the person
//! here clicks Send files, and on its own as soon as a file starts arriving,
//! so the transfer can be watched live.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::companion::{Ask, Companion, State as CompanionState};
use crate::filepop::{FilePop, Item, Request, State as FilesState};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ALLOWED: AtomicBool = AtomicBool::new(false);
static OPEN_FILES: AtomicBool = AtomicBool::new(false);
static RAISE: AtomicU64 = AtomicU64::new(0);
static PEER: Mutex<String> = Mutex::new(String::new());
static SAVE_DIR: Mutex<String> = Mutex::new(String::new());
static NOTE: Mutex<String> = Mutex::new(String::new());
static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());
static SENDS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CANCELS: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// Ends the session's windows when the session task ends.
pub struct Guard;

impl Guard {
    pub fn begin(peer: &str) -> Guard {
        begin(peer);
        Guard
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        end();
    }
}

pub fn begin(peer: &str) {
    *PEER.lock().unwrap() = peer.to_string();
    ITEMS.lock().unwrap().clear();
    SENDS.lock().unwrap().clear();
    *CANCELS.lock().unwrap() = Some(HashSet::new());
    NOTE.lock().unwrap().clear();
    OPEN_FILES.store(false, Ordering::SeqCst);
    ALLOWED.store(false, Ordering::SeqCst);
    ACTIVE.store(true, Ordering::SeqCst);
    ensure_thread();
}

pub fn end() {
    ACTIVE.store(false, Ordering::SeqCst);
    OPEN_FILES.store(false, Ordering::SeqCst);
}

/// Called often by the session: what the person here may do and where files land.
pub fn set_allowed(allowed: bool, save_dir: &str) {
    ALLOWED.store(allowed, Ordering::SeqCst);
    let mut dir = SAVE_DIR.lock().unwrap();
    if *dir != save_dir {
        *dir = save_dir.to_string();
    }
}

fn upsert(item: Item) {
    let mut items = ITEMS.lock().unwrap();
    if let Some(it) = items.iter_mut().find(|i| i.id == item.id && i.incoming == item.incoming) {
        *it = item;
    } else {
        items.push(item);
    }
}

fn with_item(id: u64, incoming: bool, f: impl FnOnce(&mut Item)) {
    if let Some(it) = ITEMS.lock().unwrap().iter_mut().find(|i| i.id == id && i.incoming == incoming) {
        f(it);
    }
}

/// A file from the other computer started arriving. Opens the Files window.
pub fn incoming(id: u64, name: &str, size: u64, offset: u64) {
    upsert(Item { id, name: name.into(), size, bytes: offset, incoming: true, state: "downloading".into() });
    show_files();
}

pub fn incoming_progress(id: u64, bytes: u64) {
    with_item(id, true, |it| it.bytes = bytes.max(it.bytes));
}

pub fn incoming_done(id: u64, error: Option<String>) {
    with_item(id, true, |it| match error {
        None => {
            it.bytes = it.size;
            it.state = "done".into();
        }
        Some(e) => it.state = e,
    });
}

pub fn outgoing(id: u64, name: &str, size: u64) {
    upsert(Item { id, name: name.into(), size, bytes: 0, incoming: false, state: "offered".into() });
}

pub fn outgoing_state(id: u64, bytes: u64, state: &str) {
    with_item(id, false, |it| {
        it.bytes = bytes;
        it.state = state.into();
    });
}

pub fn outgoing_bytes(id: u64, bytes: u64) {
    with_item(id, false, |it| it.bytes = bytes);
}

/// Paths the person here asked to send to the other computer.
pub fn take_sends() -> Vec<PathBuf> {
    std::mem::take(&mut *SENDS.lock().unwrap())
}

pub fn cancelled(id: u64) -> bool {
    CANCELS.lock().unwrap().as_ref().is_some_and(|c| c.contains(&id))
}

pub fn note(text: &str) {
    *NOTE.lock().unwrap() = text.to_string();
}

fn show_files() {
    OPEN_FILES.store(true, Ordering::SeqCst);
    RAISE.fetch_add(1, Ordering::SeqCst);
}

/// Files inside the dropped or chosen folders, so a folder sends its files.
pub fn expand(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = paths;
    while let Some(p) = stack.pop() {
        if out.len() >= 2_000 {
            break;
        }
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() => out.push(p),
            Ok(m) if m.is_dir() => {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
                    kids.sort();
                    kids.reverse();
                    stack.extend(kids);
                }
            }
            _ => {}
        }
    }
    out
}

fn ensure_thread() {
    #[cfg(not(test))]
    {
        static START: std::sync::Once = std::sync::Once::new();
        START.call_once(|| {
            let _ = std::thread::Builder::new().name("session-windows".into()).spawn(ui_thread);
        });
    }
}

#[cfg_attr(test, allow(dead_code))]
fn ui_thread() {
    let mut companion = Companion::new("host");
    let mut pop = FilePop::new("host");
    let mut shown_raise = 0u64;
    let mut files_up = false;
    let mut was_active = false;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let active = ACTIVE.load(Ordering::SeqCst);
        if !active {
            if was_active {
                companion.close();
                pop.close();
                files_up = false;
                was_active = false;
            }
            continue;
        }
        was_active = true;
        let peer = PEER.lock().unwrap().clone();
        let allowed = ALLOWED.load(Ordering::SeqCst);
        companion.keep(
            CompanionState { peer: peer.clone(), chat: super::chatwin::armed(), files: allowed, ..Default::default() },
            true,
        );
        for ask in companion.take() {
            match ask {
                Ask::Chat => super::chatwin::open(),
                Ask::Files => show_files(),
            }
        }
        if OPEN_FILES.load(Ordering::SeqCst) {
            pop.update(FilesState {
                peer: peer.clone(),
                allowed,
                note: NOTE.lock().unwrap().clone(),
                items: ITEMS.lock().unwrap().clone(),
                remote: Vec::new(),
                save_dir: SAVE_DIR.lock().unwrap().clone(),
                no_pull: true,
                ..Default::default()
            });
            let raise = RAISE.load(Ordering::SeqCst);
            if raise != shown_raise || !files_up {
                pop.show();
                shown_raise = raise;
                files_up = true;
            } else if !pop.is_open() {
                // The person closed it. It opens again on the next file or click.
                OPEN_FILES.store(false, Ordering::SeqCst);
                files_up = false;
            }
            for req in pop.take_requests() {
                match req {
                    Request::Send(paths) if allowed => SENDS.lock().unwrap().extend(expand(paths)),
                    Request::Cancel(id) => {
                        if let Some(c) = CANCELS.lock().unwrap().as_mut() {
                            c.insert(id);
                        }
                        let mut items = ITEMS.lock().unwrap();
                        for it in items.iter_mut().filter(|i| i.id == id && i.active()) {
                            it.state = "cancelled".into();
                        }
                    }
                    Request::ClearFinished => ITEMS.lock().unwrap().retain(|i| i.active()),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arriving_file_opens_the_window_and_tracks_progress() {
        begin("BENLENOVO");
        assert!(!OPEN_FILES.load(Ordering::SeqCst));
        incoming(7, "movie.mp4", 1_100, 0);
        assert!(OPEN_FILES.load(Ordering::SeqCst));
        incoming_progress(7, 600);
        assert_eq!(ITEMS.lock().unwrap()[0].bytes, 600);
        incoming_done(7, None);
        assert_eq!(ITEMS.lock().unwrap()[0].state, "done");
        outgoing(1, "report.pdf", 10);
        outgoing_state(1, 4, "sending");
        assert!(!cancelled(1));
        end();
        assert!(!ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    fn a_folder_sends_its_files() {
        let dir = std::env::temp_dir().join(format!("pjdesk-expand-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), b"b").unwrap();
        let mut got: Vec<String> = expand(vec![dir.clone()]).iter().map(|p| p.file_name().unwrap().to_string_lossy().into()).collect();
        got.sort();
        assert_eq!(got, vec!["a.txt", "b.txt"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
