//! Download and install the latest PJDesk client release right now.
//!
//! Settings, the tray menu, and a server "update now" push call [`start`].
//! That asks the PJDesk service over its local pipe. The service is already
//! elevated, so the window never changes permissions on
//! `C:\ProgramData\PJDesk\updates`. When the service is not running, one
//! administrator prompt runs `pjdesk update` and that process does the
//! download. The installer closes PJDesk and `update-handoff` opens it again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;

use tracing::{info, warn};

static RUNNING: AtomicBool = AtomicBool::new(false);
static NOTE: Mutex<String> = Mutex::new(String::new());

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn note() -> String {
    let mut guard = NOTE.lock().unwrap();
    if let Some(line) = crate::update_pipe::up_to_date_line(&guard) {
        if *guard != line {
            *guard = line.clone();
        }
        return line;
    }
    guard.clone()
}

fn set_note(line: &str) {
    if let Some(version) = crate::update_pipe::installing_version(line) {
        remember_updated_to(&version);
    }
    let line = crate::update_pipe::up_to_date_line(line).unwrap_or_else(|| line.to_string());
    *NOTE.lock().unwrap() = line.clone();
    info!("{line}");
}

/// The installer is about to replace this copy. The next launch of that version
/// says `Updated to X` and then forgets it, so a later open does not repeat it.
fn remember_updated_to(version: &str) {
    for path in marker_paths() {
        write_marker(&path, version);
    }
}

fn clear_pending_update() {
    for path in marker_paths() {
        let _ = std::fs::remove_file(path);
    }
}

/// After the installer closes PJDesk and opens it again, show `Updated to X`
/// once the running copy is the version that was installed.
pub fn adopt_completed_update() {
    let current = env!("CARGO_PKG_VERSION");
    for path in marker_paths() {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if let Some(line) = consume_marker_text(&path, current, text.trim()) {
            set_note(&line);
            clear_pending_update();
            return;
        }
    }
}

fn marker_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(parent) = crate::config::Config::path().parent() {
        paths.push(parent.join("updated-to"));
    }
    paths.push(crate::syspaths::service_dir().join("updated-to"));
    paths
}

fn write_marker(path: &std::path::Path, version: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, version.as_bytes());
}

fn consume_marker_text(path: &std::path::Path, current: &str, marked: &str) -> Option<String> {
    if marked.is_empty() {
        return None;
    }
    if pjdesk_proto::update::parse_version(marked) != pjdesk_proto::update::parse_version(current) {
        return None;
    }
    let _ = std::fs::remove_file(path);
    Some(crate::update_pipe::updated_line(current))
}

/// Starts the check on a background thread. Returns false when one is already running.
pub fn start() -> bool {
    start_inner(false)
}

/// The service calls this. The download stays in this process; it does not
/// ask itself over the pipe and it does not show an administrator prompt.
pub fn start_from_service() -> bool {
    start_inner(true)
}

fn start_inner(from_service: bool) -> bool {
    if RUNNING.swap(true, Ordering::SeqCst) {
        set_note("An update is already running.");
        return false;
    }
    set_note("Checking for updates…");
    thread::spawn(move || {
        if from_service {
            finish(service_install());
            return;
        }
        #[cfg(windows)]
        {
            if let Some(reply) = crate::update_pipe::request_update(&set_note) {
                apply_reply(reply);
                return;
            }
            set_note("The PJDesk service is not running. Approve the administrator prompt to install the update.");
            if crate::platform::run_elevated(&["update"]) {
                set_note(
                    "Approve the administrator prompt. If an update is available, PJDesk will close and open again.",
                );
            } else {
                set_note(
                    "Could not update: the PJDesk service is not running, and the administrator prompt was declined.",
                );
            }
            RUNNING.store(false, Ordering::SeqCst);
        }
        #[cfg(not(windows))]
        finish(service_install());
    });
    true
}

fn service_install() -> Result<pjdesk_proto::update::Outcome, String> {
    pjdesk_proto::update::install_client_as_service(env!("CARGO_PKG_VERSION"), &set_note).map_err(|e| format!("{e:#}"))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn apply_reply(reply: crate::update_pipe::UpdateReply) {
    if let Some(line) = crate::update_pipe::up_to_date_line(&reply.message) {
        clear_pending_update();
        set_note(&line);
        RUNNING.store(false, Ordering::SeqCst);
        return;
    }
    if reply.ok {
        if let Some(version) = crate::update_pipe::installing_version(&reply.message) {
            remember_updated_to(&version);
            set_note(&crate::update_pipe::updated_line(&version));
            return;
        }
        set_note(&reply.message);
        RUNNING.store(false, Ordering::SeqCst);
        return;
    }
    warn!("update failed: {}", reply.message);
    clear_pending_update();
    set_note(&format!("Could not update: {}", reply.message));
    RUNNING.store(false, Ordering::SeqCst);
}

fn finish(outcome: Result<pjdesk_proto::update::Outcome, String>) {
    match outcome {
        Ok(pjdesk_proto::update::Outcome::Installing(version)) => {
            remember_updated_to(&version);
            set_note(&crate::update_pipe::updated_line(&version));
        }
        Ok(pjdesk_proto::update::Outcome::UpToDate(version)) => {
            clear_pending_update();
            set_note(&format!("PJDesk {version} is up to date."));
            RUNNING.store(false, Ordering::SeqCst);
        }
        Ok(pjdesk_proto::update::Outcome::Found(version)) => {
            set_note(&format!("PJDesk {version} is available."));
            RUNNING.store(false, Ordering::SeqCst);
        }
        Err(e) => {
            if let Some(line) = crate::update_pipe::up_to_date_line(&e) {
                clear_pending_update();
                set_note(&line);
            } else {
                warn!("update failed: {e}");
                clear_pending_update();
                set_note(&format!("Could not update: {e}"));
            }
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finished_update_is_announced_once_for_that_version() {
        let dir = std::env::temp_dir().join(format!(
            "pjdesk-update-note-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("updated-to");
        write_marker(&path, "0.6.32");
        assert!(consume_marker_text(&path, "0.6.31", "0.6.32").is_none());
        assert!(path.is_file());
        assert_eq!(consume_marker_text(&path, "0.6.32", "0.6.32").as_deref(), Some("Updated to 0.6.32."));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_update_error_clears_when_the_versions_match() {
        let stale = "Could not update: The PJDesk service stopped during the update (PJDesk 0.6.31 is up to date. GitHub latest is v0.6.31.).";
        *NOTE.lock().unwrap() = stale.to_string();
        assert_eq!(note(), "PJDesk 0.6.31 is up to date.");
        assert_eq!(NOTE.lock().unwrap().as_str(), "PJDesk 0.6.31 is up to date.");

        set_note(stale);
        assert_eq!(NOTE.lock().unwrap().as_str(), "PJDesk 0.6.31 is up to date.");

        set_note("Could not update: The PJDesk service stopped during the update (Downloading PJDesk 0.6.32… 10%).");
        assert!(note().starts_with("Could not update:"));
        *NOTE.lock().unwrap() = String::new();
    }
}
