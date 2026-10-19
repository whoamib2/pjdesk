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
    NOTE.lock().unwrap().clone()
}

fn set_note(line: &str) {
    *NOTE.lock().unwrap() = line.to_string();
    info!("{line}");
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
                if reply.ok {
                    set_note(&reply.message);
                    if !reply.message.starts_with("Installing") {
                        RUNNING.store(false, Ordering::SeqCst);
                    }
                } else {
                    warn!("update failed: {}", reply.message);
                    set_note(&format!("Could not update: {}", reply.message));
                    RUNNING.store(false, Ordering::SeqCst);
                }
                return;
            }
            set_note("The PJDesk service is not running. Approve the administrator prompt to install the update.");
            if crate::platform::run_elevated(&["update"]) {
                set_note("Approve the administrator prompt. If an update is available, PJDesk will close and open again.");
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

fn finish(outcome: Result<pjdesk_proto::update::Outcome, String>) {
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
            warn!("update failed: {e}");
            set_note(&format!("Could not update: {e}"));
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
}
