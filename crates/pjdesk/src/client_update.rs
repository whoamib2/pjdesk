//! Download and install the latest PJDesk client release right now.
//!
//! Used by Settings, the tray menu, and a server "update now" push. The
//! installer closes this process and `update-handoff` opens PJDesk again.

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
    NOTE.lock().unwrap().clone()
}

fn set_note(line: &str) {
    *NOTE.lock().unwrap() = line.to_string();
    info!("{line}");
}

/// Starts the check on a background thread. Returns false when one is already running.
pub fn start() -> bool {
    if RUNNING.swap(true, Ordering::SeqCst) {
        set_note("An update is already running.");
        return false;
    }
    set_note("Checking for updates…");
    thread::spawn(|| {
        let outcome = pjdesk_proto::update::install_client_reporting(env!("CARGO_PKG_VERSION"), &set_note);
        match outcome {
            Ok(pjdesk_proto::update::Outcome::Installing(version)) => {
                set_note(&format!("Installing PJDesk {version}. PJDesk will close and open again."));
            }
            Ok(pjdesk_proto::update::Outcome::UpToDate(version)) => {
                set_note(&format!("PJDesk {version} is up to date."));
                RUNNING.store(false, Ordering::SeqCst);
            }
            Ok(pjdesk_proto::update::Outcome::Found(version)) => {
                set_note(&format!("PJDesk {version} is available."));
                RUNNING.store(false, Ordering::SeqCst);
            }
            Err(e) => {
                warn!("update failed: {e:#}");
                set_note(&format!("Could not update: {e:#}"));
                RUNNING.store(false, Ordering::SeqCst);
            }
        }
    });
    true
}
