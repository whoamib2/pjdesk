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

fn probe_loop(cmd: Receiver<ProbeCmd>, tx: Sender<ProbeSnap>, ctx: egui::Context) {
    let mut update_task = None;
    loop {
        // Service Control Manager + the status file are the fast part. Publish
        // them before the scheduled-task query, which is the slow part.
        if tx.send(sample(update_task)).is_err() {
            return;
        }
        ctx.request_repaint();
        if update_task.is_none() {
            update_task = Some(crate::service::update_task_enabled());
            if tx.send(sample(update_task)).is_err() {
                return;
            }
            ctx.request_repaint();
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
