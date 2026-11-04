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

/// No byte moved on any running transfer for this long while the link still
/// looks up: the session is treated as dropped and reconnected.
pub(crate) const STALL_LIMIT: Duration = Duration::from_secs(20);

/// Shown in the files window while the session reconnects.
pub(crate) const RECONNECTING: &str = "Connection lost \u{2014} reconnecting\u{2026}";

/// Carried across a reconnect: the open window and what was still moving.
pub struct Carry {
    pop: crate::filepop::FilePop,
    sends: Vec<std::path::PathBuf>,
    pulls: Vec<String>,
    _beat: crate::filepop::KeepAlive,
}

impl Carry {
    /// Reconnecting gave up: close the window.
    pub fn close(mut self) {
        self.pop.close();
    }
}

/// How a files-only session stopped.
pub enum FilesStop {
    /// The person closed the window, or the host ended the session.
    Done,
    /// The link dropped or stalled with work left; reconnect and resume.
    Reconnect(Carry),
}

/// Watches running transfers for progress.
#[derive(Default)]
pub(crate) struct StallWatch {
    last: Option<(u64, Instant)>,
}

impl StallWatch {
    /// True once running transfers have made no progress for `STALL_LIMIT`.
    pub(crate) fn stalled(&mut self, live: &Live, now: Instant) -> bool {
        let running: Vec<_> =
            live.transfers.iter().filter(|t| matches!(t.state.as_str(), "sending" | "downloading")).collect();
        if running.is_empty() {
            self.last = None;
            return false;
        }
        let bytes: u64 = running.iter().map(|t| t.bytes).sum();
        match self.last {
            Some((seen, at)) if seen == bytes => now.duration_since(at) >= STALL_LIMIT,
            _ => {
                self.last = Some((bytes, now));
                false
            }
        }
    }
}

/// Runs until the person closes the files window or the session ends. A
/// dropped or stalled link with transfers left returns `Reconnect` so the
/// caller signs in again and the transfers continue from the partial files.
pub fn run(runtime: &tokio::runtime::Runtime, link: Link, carry: Option<Carry>) -> Result<FilesStop> {
    let Link { connection, endpoint, control_send, control_recv, host_name, host_id, path, peer_proto, .. } = link;
    if peer_proto < 3 {
        connection.close(0u32.into(), b"files only not supported");
        if let Some(c) = carry {
            c.close();
        }
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
    let mut pop = match carry {
        Some(c) => {
            let Carry { pop, sends, pulls, _beat } = c;
            drop(_beat);
            tracing::info!("Files-only session back; resuming {} upload(s), {} download(s)", sends.len(), pulls.len());
            for file in sends {
                let _ = cmds.send(Cmd::ResumeFile(file));
            }
            for name in pulls {
                let _ = cmds.send(Cmd::Pull(name));
            }
            pop
        }
        None => {
            let mut pop = crate::filepop::FilePop::new("files-only");
            pop.update(files_state(&live.lock().unwrap()));
            pop.show();
            pop
        }
    };
    let started = Instant::now();
    let mut batch = crate::reveal::Batch::default();
    let mut watch = StallWatch::default();
    enum End {
        User,
        Closed(String),
        Lost(String),
    }
    let ended = loop {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(reason) = connection.close_reason() {
            let host_ended = matches!(reason, quinn::ConnectionError::ApplicationClosed(_));
            let text = format!("The connection ended: {reason}");
            break if host_ended { End::Closed(text) } else { End::Lost(text) };
        }
        if let Some(text) = live.lock().unwrap().closed.clone() {
            break End::Closed(text);
        }
        if watch.stalled(&live.lock().unwrap(), Instant::now()) {
            break End::Lost(format!("no transfer progress for {} s", STALL_LIMIT.as_secs()));
        }
        // The window takes a moment to start; after that, closing it ends the session.
        if !pop.is_open() && started.elapsed() > Duration::from_secs(3) {
            break End::User;
        }
        pump(&live, &cmds, &mut pop, &mut batch);
    };
    connection.close(0u32.into(), b"files window closed");
    let _ = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await });
    let (sends, pulls) = {
        let g = live.lock().unwrap();
        (controls::unfinished_sends(&g), controls::unfinished_pulls(&g))
    };
    let reconnect = |mut pop: crate::filepop::FilePop, why: &str| {
        tracing::warn!("Files-only session dropped ({why}); reconnecting and resuming");
        let mut state = files_state(&live.lock().unwrap());
        state.note = RECONNECTING.to_string();
        pop.update(state);
        let _beat = pop.keep_alive();
        Ok(FilesStop::Reconnect(Carry { pop, sends: sends.clone(), pulls: pulls.clone(), _beat }))
    };
    let work_left = !sends.is_empty() || !pulls.is_empty();
    match ended {
        End::Closed(text) if text == controls::FILES_ONLY_TOO_OLD => {
            pop.close();
            crate::note_failure(&text);
            bail!("{text}")
        }
        End::Lost(text) if pop.is_open() => reconnect(pop, &text),
        End::Closed(text) if work_left && pop.is_open() && !host_refused(&text) => reconnect(pop, &text),
        End::Lost(text) | End::Closed(text) => {
            pop.close();
            tracing::info!("Files-only session ended: {text}");
            Ok(FilesStop::Done)
        }
        End::User => {
            pop.close();
            tracing::info!("Files window closed; files-only session over");
            Ok(FilesStop::Done)
        }
    }
}

/// A deliberate end from the host side, not a drop: do not sign in again.
fn host_refused(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["denied", "refused", "ended the session", "not allowed", "disconnected you"].iter().any(|w| t.contains(w))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::controls::Transfer;

    fn row(state: &str, bytes: u64) -> Transfer {
        Transfer {
            id: 1,
            name: "big.exe".into(),
            size: 310 << 20,
            bytes,
            incoming: false,
            state: state.into(),
            pulled: false,
        }
    }

    #[test]
    fn a_transfer_with_no_progress_is_a_stall_and_moving_bytes_are_not() {
        let mut live = Live::new(true);
        let mut watch = StallWatch::default();
        let t0 = Instant::now();
        live.transfers.push(row("sending", 26 << 20));
        assert!(!watch.stalled(&live, t0));
        assert!(!watch.stalled(&live, t0 + STALL_LIMIT - Duration::from_secs(1)));
        assert!(watch.stalled(&live, t0 + STALL_LIMIT));
        // Bytes moving again resets the clock.
        live.transfers[0].bytes += 1;
        assert!(!watch.stalled(&live, t0 + STALL_LIMIT));
        // Nothing running is never a stall.
        live.transfers[0].state = "done".into();
        assert!(!watch.stalled(&live, t0 + STALL_LIMIT * 10));
    }

    #[derive(clap::Parser)]
    struct Wrap {
        #[command(flatten)]
        host: crate::HostArgs,
    }

    /// A files-only session where the host never opens a video stream must
    /// stay up past the old 20 s "no picture" limit while files move.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn files_only_session_outlives_30s_with_no_video_while_transferring() {
        use clap::Parser;
        use pjdesk_proto::tls::TlsIdentity;
        use pjdesk_proto::PEER_SERVER_NAME;
        use std::net::UdpSocket;

        controls::set_files_only(true);
        assert!(!controls::expects_video_stream());
        let dir = std::env::temp_dir().join(format!("pjdesk-fo-test-{}", std::process::id()));
        let recv_dir = dir.join("in");
        std::fs::create_dir_all(&recv_dir).unwrap();

        let identity = TlsIdentity::generate(PEER_SERVER_NAME).unwrap();
        let fp = identity.fingerprint();
        let hs = UdpSocket::bind("127.0.0.1:0").unwrap();
        hs.set_nonblocking(true).unwrap();
        let addr = hs.local_addr().unwrap();
        let host_ep = crate::p2p::host_endpoint(hs, &identity).unwrap();
        let cs = UdpSocket::bind("127.0.0.1:0").unwrap();
        cs.set_nonblocking(true).unwrap();
        let ctrl_ep = crate::p2p::controller_endpoint(cs, fp).unwrap();

        let args = Wrap::parse_from(["test"]).host;
        let grant = crate::host::ctrl::Grant {
            ceiling: pjdesk_proto::perms::FILE | pjdesk_proto::perms::PEER_VERSION,
            peer_proto: 3,
            peer_name: "LENOVO".into(),
            tunnel_remote: false,
            tunnel_ports: Vec::new(),
            receive_dir: recv_dir.to_string_lossy().into_owned(),
        };
        let host = tokio::spawn(async move {
            let conn = host_ep.accept().await.unwrap().await.unwrap();
            let (send, recv) = conn.accept_bi().await.unwrap();
            let _ = crate::host::stream::begin(conn, send, recv, args, grant, None).await;
        });

        let conn = ctrl_ep.connect(addr, PEER_SERVER_NAME).unwrap().await.unwrap();
        let (send, recv) = conn.open_bi().await.unwrap();
        let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlMsg>();
        tokio::spawn(pjdesk_proto::inputflow::pump_control(send, control_rx));
        let (frame_tx, _frames) = std::sync::mpsc::channel();
        let (live, cmds, _rec) = controls::start(
            tokio::runtime::Handle::current(),
            conn.clone(),
            recv,
            control_tx,
            frame_tx,
            3,
            "123456789".into(),
            "CABINSIGHTHOUND".into(),
            false,
            None,
            None,
            || {},
        );

        let send_one = |name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, vec![7u8; 3 << 20]).unwrap();
            cmds.send(Cmd::SendFile(path)).unwrap();
        };
        let all_done = |n: usize| {
            let g = live.lock().unwrap();
            g.transfers.len() == n && g.transfers.iter().all(|t| t.state == "done")
        };

        send_one("first.bin");
        let start = Instant::now();
        // Over 30 s with no video at all, pumping like the files window does.
        while start.elapsed() < Duration::from_secs(32) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let accept: Vec<u64> = live
                .lock()
                .unwrap()
                .transfers
                .iter()
                .filter(|t| t.incoming && t.state == "offered")
                .map(|t| t.id)
                .collect();
            for id in accept {
                let _ = cmds.send(Cmd::Accept(id));
            }
            let closed = live.lock().unwrap().closed.clone();
            assert!(closed.is_none(), "session closed after {:?}: {closed:?}", start.elapsed());
            assert!(conn.close_reason().is_none(), "connection dropped after {:?}", start.elapsed());
        }
        assert!(all_done(1), "first file did not finish: {:?}", live.lock().unwrap().transfers.iter().map(|t| t.state.clone()).collect::<Vec<_>>());

        // A transfer started after the old limit still goes through.
        send_one("second.bin");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !all_done(2) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(all_done(2), "second file did not finish");
        assert!(live.lock().unwrap().closed.is_none());
        assert_eq!(std::fs::metadata(recv_dir.join("second.bin")).unwrap().len(), 3 << 20);

        conn.close(0u32.into(), b"test over");
        host.abort();
        controls::set_files_only(false);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
