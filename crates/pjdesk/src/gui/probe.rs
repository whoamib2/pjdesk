//! Background reads of Windows service state.
//!
//! `schtasks /Query` (is the auto-update task registered?) and the Service
//! Control Manager query both block. Calling either from a click handler or
//! from a frame made Settings and the rest of the window wait — often for
//! seconds — before anything painted. This thread owns those calls. The UI
//! keeps the last snapshot and paints from it.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eframe::egui;

use crate::syspaths::ServiceStatus;

/// The latest answer from the probe thread. Safe to read on the UI thread.
#[derive(Clone, Debug)]
pub struct ProbeSnap {
    pub installed: bool,
    pub running: bool,
    /// `None` until the scheduled-task query has returned.
    pub update_task: Option<bool>,
    pub svc: ServiceStatus,
}

enum ProbeCmd {
    /// Run `schtasks` again (the user opened Settings).
    RefreshUpdateTask,
}

pub struct Probe {
    cmd: Sender<ProbeCmd>,
    rx: Receiver<ProbeSnap>,
    pub latest: ProbeSnap,
}

impl Probe {
    /// Starts the probe. The context is only used to wake the UI when a
    /// result arrives, so the window does not wait for the next mouse move.
    pub fn start(ctx: egui::Context) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (snap_tx, snap_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("pjdesk-probe".into())
            .spawn(move || probe_loop(cmd_rx, snap_tx, ctx))
            .expect("spawn probe thread");
        Self {
            cmd: cmd_tx,
            rx: snap_rx,
            latest: ProbeSnap {
                installed: false,
                running: false,
                update_task: None,
                svc: ServiceStatus::default(),
            },
        }
    }

    pub fn request_update_task(&self) {
        let _ = self.cmd.send(ProbeCmd::RefreshUpdateTask);
    }

    /// Non-blocking. Returns true when a newer snapshot was applied.
    pub fn poll(&mut self) -> bool {
        let mut got = false;
        while let Ok(snap) = self.rx.try_recv() {
            self.latest = snap;
            got = true;
        }
        got
    }
}

fn sample(update_task: Option<bool>) -> ProbeSnap {
    let (installed, running) = crate::service::status();
    let svc = ServiceStatus::load().unwrap_or_default();
    ProbeSnap { installed, running, update_task, svc }
}

/// Missed service checks required before the window registers, after the
/// service has been seen running. One miss must not start a second registrar.
pub const SERVICE_DOWN_POLLS: u32 = 2;

/// Who should register this computer after one service-status sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostDuty {
    pub service_running: bool,
    pub down_polls: u32,
    /// The window should have its own host after this sample.
    pub window_hosts: bool,
}

/// The service keeps the registration only when its status is fresh and online,
/// and the server it registered with is the one this window is using.
/// An empty server name is an older status file and still counts.
pub fn service_holds_registration(hosting: bool, status_server: &str, expected_server: &str) -> bool {
    if !hosting {
        return false;
    }
    let got = status_server.trim();
    if got.is_empty() {
        return true;
    }
    got.eq_ignore_ascii_case(expected_server.trim())
}

/// Who should register this computer after one service-status sample.
///
/// When the service is not installed, the first sample starts the window
/// immediately. When it is installed, the window waits for the service to
/// register — including the first look after sign-in — so a user-mode app
/// that starts at logon does not steal the session from the SYSTEM
/// session-host. After the service has been seen hosting (or was given
/// that chance), [`SERVICE_DOWN_POLLS`] consecutive misses are required
/// before the window takes over. `sample_hosting` is true only when the
/// service status file is fresh and says this computer is online. Service
/// Control Manager "Running" is not enough to count as hosting, but
/// "installed" is enough to wait.
pub fn host_duty(
    ready: bool,
    service_running: bool,
    down_polls: u32,
    sample_hosting: bool,
    service_installed: bool,
) -> HostDuty {
    if sample_hosting {
        return HostDuty { service_running: true, down_polls: 0, window_hosts: false };
    }
    if service_installed {
        if !ready {
            return HostDuty { service_running: true, down_polls: 0, window_hosts: false };
        }
        if service_running {
            let polls = down_polls.saturating_add(1);
            if polls >= SERVICE_DOWN_POLLS {
                HostDuty { service_running: false, down_polls: polls, window_hosts: true }
            } else {
                HostDuty { service_running: true, down_polls: polls, window_hosts: false }
            }
        } else {
            HostDuty { service_running: false, down_polls, window_hosts: true }
        }
    } else if !ready {
        HostDuty { service_running: false, down_polls: 0, window_hosts: true }
    } else if !service_running {
        HostDuty { service_running: false, down_polls: 0, window_hosts: true }
    } else {
        let polls = down_polls.saturating_add(1);
        if polls >= SERVICE_DOWN_POLLS {
            HostDuty { service_running: false, down_polls: polls, window_hosts: true }
        } else {
            HostDuty { service_running: true, down_polls: polls, window_hosts: false }
        }
    }
}

fn probe_loop(cmd: Receiver<ProbeCmd>, tx: Sender<ProbeSnap>, ctx: egui::Context) {
    let mut update_task = None;
    loop {
        // Service Control Manager + the status file are the fast part. Publish
        // them before the scheduled-task query, which is the slow part.
        if tx.send(sample(update_task)).is_err() {
            return;
        }
        super::hangwatch::request_repaint_if_visible(&ctx);
        super::poll_client_heartbeat();
        if update_task.is_none() {
            update_task = Some(crate::service::update_task_enabled());
            if tx.send(sample(update_task)).is_err() {
                return;
            }
            super::hangwatch::request_repaint_if_visible(&ctx);
        }
        match cmd.recv_timeout(Duration::from_secs(2)) {
            Ok(ProbeCmd::RefreshUpdateTask) => {
                update_task = Some(crate::service::update_task_enabled());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_down_starts_window() {
        let first_down = host_duty(false, false, 0, false, false);
        assert!(first_down.window_hosts, "the first sample starts the window when the service is down");
        assert!(!first_down.service_running);

        let first_up = host_duty(false, false, 0, true, false);
        assert!(!first_up.window_hosts);
        assert!(first_up.service_running);

        let one_miss = host_duty(true, true, 0, false, false);
        assert_eq!(one_miss.down_polls, 1);
        assert!(one_miss.service_running);
        assert!(!one_miss.window_hosts, "one missed check must not start a second registrar");

        let two_miss = host_duty(true, true, one_miss.down_polls, false, false);
        assert_eq!(two_miss.down_polls, SERVICE_DOWN_POLLS);
        assert!(!two_miss.service_running);
        assert!(two_miss.window_hosts);

        let back = host_duty(true, false, two_miss.down_polls, true, false);
        assert!(back.service_running);
        assert_eq!(back.down_polls, 0);
        assert!(!back.window_hosts);
    }

    #[test]
    fn the_window_yields_only_when_the_service_is_actually_online() {
        let mut stale = ServiceStatus::default();
        stale.online = true;
        assert!(!stale.is_hosting());
        let still = host_duty(true, false, 0, stale.is_hosting(), false);
        assert!(still.window_hosts, "a service process with no fresh registration does not take over");

        let fresh = ServiceStatus::now(Some("692996394".into()), true, None, None, false);
        assert!(fresh.is_hosting());
        let yielded = host_duty(true, false, 0, fresh.is_hosting(), false);
        assert!(!yielded.window_hosts);
        assert!(yielded.service_running);
    }

    #[test]
    #[test]
    fn an_installed_service_is_given_time_to_register_after_sign_in() {
        let first = host_duty(false, false, 0, false, true);
        assert!(!first.window_hosts, "the window must not grab the host before the installed service registers");
        assert!(first.service_running);

        let one = host_duty(true, true, 0, false, true);
        assert_eq!(one.down_polls, 1);
        assert!(!one.window_hosts);
        assert!(one.service_running);

        let two = host_duty(true, true, one.down_polls, false, true);
        assert!(two.window_hosts);
        assert!(!two.service_running);

        let kept = host_duty(true, false, two.down_polls, false, true);
        assert!(kept.window_hosts, "after taking over, keep hosting until the service is actually online");

        let back = host_duty(true, false, kept.down_polls, true, true);
        assert!(!back.window_hosts);
        assert!(back.service_running);
    }

    #[test]
    fn the_window_takes_over_when_the_service_registered_somewhere_else() {
        assert!(service_holds_registration(true, "", "pjdesk.spader.us:21200"));
        assert!(service_holds_registration(true, "pjdesk.spader.us:21200", "PJDESK.SPADER.US:21200"));
        assert!(!service_holds_registration(true, "192.168.1.50:21200", "pjdesk.spader.us:21200"));
        assert!(!service_holds_registration(false, "pjdesk.spader.us:21200", "pjdesk.spader.us:21200"));
    }
}
