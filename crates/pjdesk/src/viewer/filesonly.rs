//! Files-only sessions: the same sign-in as a normal session, then only the
//! drag-and-drop files window. No picture window, video or input. Closing the
//! files window ends the session.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use pjdesk_proto::peer::ControlMsg;
use tokio::sync::mpsc;

use super::controls::{self, Cmd, Live};
use super::Link;

/// What the files window shows for one session.
pub(super) fn files_state(live: &Live) -> crate::filepop::State {
    let items = live
        .transfers
        .iter()
        .map(|t| crate::filepop::Item {
            id: t.id,
            name: t.name.clone(),
            size: t.size,
            bytes: t.bytes,
            incoming: t.incoming,
            state: t.state.clone(),
        })
        .collect();
    let allowed = live.allowed(pjdesk_proto::perms::FILE);
    let note = if live.v3 && live.perms != 0 && !allowed {
        "The other computer did not allow file transfer.".to_string()
    } else {
        String::new()
    };
    crate::filepop::State {
        peer: live.peer_label.clone(),
        allowed: allowed || live.perms == 0,
        note,
        items,
        remote: live.remote_files.clone(),
        save_dir: controls::downloads_dir().to_string_lossy().into_owned(),
        ..Default::default()
    }
}

/// Runs until the person closes the files window or the session ends.
pub fn run(runtime: &tokio::runtime::Runtime, link: Link) -> Result<()> {
    let Link { connection, endpoint, control_send, control_recv, host_name, host_id, path, peer_proto, .. } = link;
    if peer_proto < 3 {
        connection.close(0u32.into(), b"files only not supported");
        crate::note_failure(controls::FILES_ONLY_TOO_OLD);
        bail!("{}", controls::FILES_ONLY_TOO_OLD);
    }
    controls::set_files_only(true);
    let relayed = matches!(path, crate::p2p::LinkPath::Relay(_));
    let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlMsg>();
    runtime.spawn(pjdesk_proto::inputflow::pump_control(control_send, control_rx));
    // Nothing reads pictures; a host never sends any in this mode.
    let (frame_tx, _frames) = std::sync::mpsc::channel();
    let (live, cmds, _recorder) = controls::start(
        runtime.handle().clone(),
        connection.clone(),
        control_recv,
        control_tx,
        frame_tx,
        peer_proto,
        host_id,
        host_name.clone(),
        relayed,
        None,
        None,
        || {},
    );
    let mut pop = crate::filepop::FilePop::new("files-only");
    pop.update(files_state(&live.lock().unwrap()));
    pop.show();
    let started = Instant::now();
    let mut batch = crate::reveal::Batch::default();
    let ended = loop {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(text) = live.lock().unwrap().closed.clone() {
            break Some(text);
        }
        if let Some(reason) = connection.close_reason() {
            break Some(format!("The connection ended: {reason}"));
        }
        // The window takes a moment to start; after that, closing it ends the session.
        if !pop.is_open() && started.elapsed() > Duration::from_secs(3) {
            break None;
        }
        pump(&live, &cmds, &mut pop, &mut batch);
    };
    pop.close();
    connection.close(0u32.into(), b"files window closed");
    let _ = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await });
    match ended {
        Some(text) if text == controls::FILES_ONLY_TOO_OLD => {
            crate::note_failure(&text);
            bail!("{text}")
        }
        Some(text) => {
            tracing::info!("Files-only session ended: {text}");
            Ok(())
        }
        None => {
            tracing::info!("Files window closed; files-only session over");
            Ok(())
        }
    }
}

fn pump(
    live: &Arc<Mutex<Live>>,
    cmds: &mpsc::UnboundedSender<Cmd>,
    pop: &mut crate::filepop::FilePop,
    batch: &mut crate::reveal::Batch,
) {
    let (saved, arriving) = {
        let mut g = live.lock().unwrap();
        let saved = std::mem::take(&mut g.saved);
        let arriving = g.transfers.iter().any(|t| t.incoming && matches!(t.state.as_str(), "offered" | "downloading"));
        (saved, arriving)
    };
    for path in saved {
        batch.saved(path);
    }
    if let Some(paths) = batch.take_if_settled(arriving) {
        crate::reveal::select_in_explorer(paths);
    }
    for req in pop.take_requests() {
        match req {
            crate::filepop::Request::Send(paths) => {
                for path in &paths {
                    for file in super::window::expand_drop(path) {
                        let _ = cmds.send(Cmd::SendFile(file));
                    }
                }
            }
            crate::filepop::Request::Cancel(id) => {
                let _ = cmds.send(Cmd::Cancel(id));
            }
            crate::filepop::Request::List => {
                let _ = cmds.send(Cmd::ListRemote);
            }
            crate::filepop::Request::Pull(name) => {
                let _ = cmds.send(Cmd::Pull(name));
            }
            crate::filepop::Request::ClearFinished => live.lock().unwrap().clear_finished(),
            crate::filepop::Request::Resume(id) => {
                let _ = cmds.send(Cmd::Retry(id));
            }
        }
    }
    // Files the other side offers are taken while the window is open.
    let offered: Vec<u64> =
        live.lock().unwrap().transfers.iter().filter(|t| t.incoming && t.state == "offered").map(|t| t.id).collect();
    for id in offered {
        let _ = cmds.send(Cmd::Accept(id));
    }
    pop.update(files_state(&live.lock().unwrap()));
}
