//! Version-3 session controls: permissions, files, clipboard, chat, audio,
//! tunnels, privacy, and quality. Version-2 peers never see the new messages.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pjdesk_proto::filesafe::{self, MAX_TRANSFER_BYTES};
use pjdesk_proto::framing::write_msg;
use pjdesk_proto::peer::{
    read_side_chunk, read_side_header, write_side_chunk, write_side_header, Clipboard, ClipKind, ControlMsg,
    FileMsg, HostMsg, KeyboardMode, SideHeader, SpecialCommand, TunnelMsg, AUDIO_MAGIC, CLIP_MAGIC, FILE_MAGIC,
    MAX_CLIP_IMAGE, MAX_CLIP_TEXT, SIDE_CHUNK,
};
use pjdesk_proto::perms::{self, grant, has};
use pjdesk_proto::quality::{self, BitrateControl, StreamQuality, AUDIO_CHANNELS, AUDIO_FRAME_SAMPLES, AUDIO_RATE};
use pjdesk_proto::{discover, PROTOCOL_VERSION};
use quinn::{Connection, RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::capture::Rect;
use super::input::InputInjector;
use crate::host::HostShared;

/// What this session is allowed to do, decided before the stream starts.
pub struct Grant {
    pub ceiling: u32,
    pub peer_proto: u16,
    pub peer_name: String,
    /// Server policy: forwards may leave loopback.
    pub tunnel_remote: bool,
    /// Ports the person (or the policy) listed.
    pub tunnel_ports: Vec<u16>,
    pub receive_dir: String,
}

struct ExpectedFile {
    name: String,
    size: u64,
    offset: u64,
}

struct OutFile {
    id: u64,
    path: PathBuf,
    resume: u64,
}

/// Shared with the capture thread and the side-stream tasks.
pub struct Session {
    pub perms: AtomicU32,
    pub display: AtomicUsize,
    pub fps: AtomicU32,
    pub bitrate: AtomicU32,
    pub max_width: AtomicU32,
    peer_proto: u16,
    ceiling: u32,
    view_only: bool,
    receive_dir: Mutex<String>,
    tunnel_ports: Vec<u16>,
    tunnel_remote: bool,
    service_host: bool,
    keyboard: Mutex<KeyboardMode>,
    expected: Mutex<HashMap<u64, ExpectedFile>>,
    outgoing: Mutex<HashMap<u64, OutFile>>,
    next_id: AtomicU64,
    host_tx: mpsc::UnboundedSender<HostMsg>,
    sent_at: Mutex<VecDeque<(u32, Instant)>>,
    bitrate_ctl: Mutex<BitrateControl>,
    privacy: AtomicBool,
    blocked: AtomicBool,
    audio_stop: Arc<AtomicBool>,
    conn: Connection,
    shared: Option<Arc<HostShared>>,
    peer_name: String,
}

impl Session {
    pub fn new(
        args_fps: u32,
        args_bitrate: u32,
        args_max_width: u32,
        args_display: usize,
        view_only: bool,
        service_host: bool,
        grant: Grant,
        conn: Connection,
        host_tx: mpsc::UnboundedSender<HostMsg>,
        shared: Option<Arc<HostShared>>,
    ) -> Arc<Self> {
        let initial = if grant.peer_proto < 3 {
            perms::grant(perms::CLASSIC, grant.ceiling, view_only)
        } else {
            0
        };
        Arc::new(Self {
            perms: AtomicU32::new(initial),
            display: AtomicUsize::new(args_display),
            fps: AtomicU32::new(args_fps.clamp(1, 60)),
            bitrate: AtomicU32::new(args_bitrate.clamp(200, 50_000)),
            max_width: AtomicU32::new(args_max_width.clamp(640, 3840)),
            peer_proto: grant.peer_proto,
            ceiling: grant.ceiling,
            view_only,
            receive_dir: Mutex::new(grant.receive_dir),
            tunnel_ports: grant.tunnel_ports,
            tunnel_remote: grant.tunnel_remote,
            service_host,
            keyboard: Mutex::new(KeyboardMode::Map),
            expected: Mutex::new(HashMap::new()),
            outgoing: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            host_tx,
            sent_at: Mutex::new(VecDeque::new()),
            bitrate_ctl: Mutex::new(BitrateControl::new(args_bitrate)),
            privacy: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
            audio_stop: Arc::new(AtomicBool::new(true)),
            conn,
            shared,
            peer_name: grant.peer_name,
        })
    }

    pub fn initial_mouse_keyboard(&self) -> (bool, bool) {
        let p = self.perms.load(Ordering::SeqCst);
        (has(p, perms::MOUSE), has(p, perms::KEYBOARD))
    }

    pub fn note_sent(&self, seq: u32) {
        let mut q = self.sent_at.lock().unwrap();
        q.push_back((seq, Instant::now()));
        while q.len() > 32 {
            q.pop_front();
        }
    }

    pub fn note_ack(&self, seq: u32) {
        let delay = {
            let mut q = self.sent_at.lock().unwrap();
            let pos = q.iter().position(|(s, _)| *s == seq);
            pos.and_then(|i| q.remove(i)).map(|(_, t)| t.elapsed().as_millis() as u32)
        };
        let Some(delay) = delay else { return };
        let mut ctl = self.bitrate_ctl.lock().unwrap();
        if ctl.observe(delay) {
            self.bitrate.store(ctl.current, Ordering::SeqCst);
        }
    }

    fn v3(&self) -> bool {
        self.peer_proto >= 3
    }

    fn allow(&self, bit: u32) -> bool {
        has(self.perms.load(Ordering::SeqCst), bit)
    }

    async fn deny(&self, send: &mut SendStream, what: &str, reason: &str) -> Result<()> {
        if self.v3() {
            write_msg(send, &HostMsg::Denied { what: what.into(), reason: reason.into() }).await?;
        }
        Ok(())
    }

    /// Handles one controller message. `true` means the session should end
    /// (a restart was accepted).
    pub async fn handle(
        &self,
        msg: ControlMsg,
        send: &mut SendStream,
        injector: &mut InputInjector,
        display: Rect,
        keyframe: &AtomicBool,
    ) -> Result<bool> {
        match msg {
            ControlMsg::Input(event) => {
                if *self.keyboard.lock().unwrap() == KeyboardMode::Translate {
                    if let pjdesk_proto::peer::InputEvent::Key { .. } = event {
                        // Printable keys arrive as Text in Translate mode.
                        // Modifier and special keys still use scan codes.
                    }
                }
                injector.handle(event, display);
            }
            ControlMsg::FrameAck { seq } => self.note_ack(seq),
            ControlMsg::RequestKeyframe => keyframe.store(true, Ordering::SeqCst),
            ControlMsg::Special(cmd) => self.special(cmd, send).await?,
            ControlMsg::Ping { t_us } => write_msg(send, &HostMsg::Pong { t_us }).await?,
            ControlMsg::Hello { permissions, monitor, quality } => {
                self.hello(permissions, monitor, quality, send, injector, keyframe).await?
            }
            ControlMsg::Chat { text } => self.chat_in(&text, send).await?,
            ControlMsg::Clipboard(clip) => self.clip_in(clip, send).await?,
            ControlMsg::SetQuality(q) => {
                if !self.v3() {
                    return Ok(false);
                }
                self.apply_quality(q, keyframe);
            }
            ControlMsg::SelectMonitor { index } => {
                if !self.v3() {
                    return Ok(false);
                }
                self.display.store(index as usize, Ordering::SeqCst);
                keyframe.store(true, Ordering::SeqCst);
                info!("Controller selected monitor {index}");
            }
            ControlMsg::File(msg) => self.file_ctrl(msg, send).await?,
            ControlMsg::Restart => {
                if !self.allow(perms::RESTART) {
                    self.deny(send, "restart", "restart was not allowed for this session").await?;
                } else {
                    info!("Remote restart requested by {}", self.peer_name);
                    write_msg(send, &HostMsg::Restarting).await?;
                    let _ = send.flush().await;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    if !crate::platform::restart_windows() {
                        self.deny(send, "restart", "Windows did not accept the restart").await?;
                    } else {
                        return Ok(true);
                    }
                }
            }
            ControlMsg::Privacy { enabled } => self.privacy(enabled, send).await?,
            ControlMsg::BlockInput { enabled } => self.block_input(enabled, send).await?,
            ControlMsg::SetKeyboardMode(mode) => {
                if self.allow(perms::KEYBOARD) {
                    *self.keyboard.lock().unwrap() = mode;
                }
            }
            ControlMsg::Text { chars } => {
                if self.allow(perms::KEYBOARD) && *self.keyboard.lock().unwrap() == KeyboardMode::Translate {
                    injector.text(&chars);
                }
            }
            ControlMsg::Tunnel(msg) => self.tunnel(msg, send).await?,
            ControlMsg::Audio { enabled } => self.audio(enabled, send).await?,
        }
        Ok(false)
    }

    async fn hello(
        &self,
        requested: u32,
        monitor: u8,
        quality: StreamQuality,
        send: &mut SendStream,
        injector: &mut InputInjector,
        keyframe: &AtomicBool,
    ) -> Result<()> {
        if !self.v3() {
            return Ok(());
        }
        let mut ceiling = self.ceiling;
        // A service session only gets file transfer when a person approved it.
        // The ceiling already excludes it for unattended access.
        if self.service_host && !has(ceiling, perms::FILE) {
            ceiling &= !perms::FILE;
        }
        let granted = grant(requested, ceiling, self.view_only);
        let dir = self.receive_dir.lock().unwrap().clone();
        let granted = if dir.trim().is_empty() { granted & !perms::FILE } else { granted };
        self.perms.store(granted, Ordering::SeqCst);
        injector.set_allowed(has(granted, perms::MOUSE), has(granted, perms::KEYBOARD));
        self.display.store(monitor as usize, Ordering::SeqCst);
        self.apply_quality(quality, keyframe);
        let monitors = super::capture::monitor_infos();
        info!("{} granted: {}", self.peer_name, perms::describe(granted));
        write_msg(send, &HostMsg::Welcome { permissions: granted, monitors, proto: PROTOCOL_VERSION }).await?;
        if let Some(shared) = &self.shared {
            shared.set_session_perms(granted);
        }
        Ok(())
    }

    fn apply_quality(&self, quality: StreamQuality, keyframe: &AtomicBool) {
        let q = quality.clamp();
        self.fps.store(q.fps, Ordering::SeqCst);
        self.max_width.store(q.max_width, Ordering::SeqCst);
        self.bitrate_ctl.lock().unwrap().set_target(q.bitrate_kbps);
        let current = self.bitrate_ctl.lock().unwrap().current;
        self.bitrate.store(current, Ordering::SeqCst);
        keyframe.store(true, Ordering::SeqCst);
    }

    async fn special(&self, cmd: SpecialCommand, send: &mut SendStream) -> Result<()> {
        if !self.allow(perms::KEYBOARD) {
            return self.deny(send, "keyboard", "keyboard control was not allowed").await;
        }
        match cmd {
            SpecialCommand::CtrlAltDel => crate::platform::send_ctrl_alt_del(),
            SpecialCommand::Lock => {
                if !crate::platform::lock_workstation() {
                    self.deny(send, "lock", "this session could not lock Windows").await?;
                }
            }
            SpecialCommand::TaskManager => {
                if !open_task_manager() {
                    self.deny(send, "taskmgr", "could not open Task Manager").await?;
                }
            }
        }
        Ok(())
    }

    async fn chat_in(&self, text: &str, send: &mut SendStream) -> Result<()> {
        if !self.allow(perms::CHAT) {
            return self.deny(send, "chat", "chat was not allowed").await;
        }
        let text = clip_line(text, 2000);
        if text.is_empty() {
            return Ok(());
        }
        info!("Chat from {}: {text}", self.peer_name);
        if let Some(shared) = &self.shared {
            shared.push_chat(false, &text);
        }
        Ok(())
    }

    async fn clip_in(&self, clip: Clipboard, send: &mut SendStream) -> Result<()> {
        if !self.allow(perms::CLIPBOARD) || self.view_only {
            return self.deny(send, "clipboard", "clipboard sync was not allowed").await;
        }
        match clip.kind {
            ClipKind::Text => {
                if clip.bytes.len() > MAX_CLIP_TEXT {
                    return self.deny(send, "clipboard", "clipboard text is too large").await;
                }
                let Ok(text) = String::from_utf8(clip.bytes) else {
                    return self.deny(send, "clipboard", "clipboard text was not UTF-8").await;
                };
                crate::clip::set_text(&text);
            }
            ClipKind::ImagePng => {
                if clip.bytes.is_empty() {
                    return Ok(());
                }
                if clip.bytes.len() > MAX_CLIP_IMAGE as usize {
                    return self.deny(send, "clipboard", "clipboard image is too large").await;
                }
                if let Some((w, h, rgba)) = crate::clip::decode_png(&clip.bytes) {
                    crate::clip::set_image(w, h, &rgba);
                }
            }
        }
        Ok(())
    }

    async fn privacy(&self, enabled: bool, send: &mut SendStream) -> Result<()> {
        if !self.allow(perms::PRIVACY) {
            return self.deny(send, "privacy", "privacy mode was not allowed").await;
        }
        self.privacy.store(enabled, Ordering::SeqCst);
        crate::platform::set_privacy_screen(enabled);
        if self.v3() {
            write_msg(send, &HostMsg::Privacy { enabled }).await?;
        }
        info!("Privacy mode {}", if enabled { "on" } else { "off" });
        Ok(())
    }

    async fn block_input(&self, enabled: bool, send: &mut SendStream) -> Result<()> {
        if !self.allow(perms::BLOCK_INPUT) {
            return self.deny(send, "block-input", "blocking local input was not allowed").await;
        }
        if !crate::platform::set_block_input(enabled) {
            return self.deny(send, "block-input", "Windows refused to block local input").await;
        }
        self.blocked.store(enabled, Ordering::SeqCst);
        info!("Local input {}", if enabled { "blocked" } else { "restored" });
        Ok(())
    }

    async fn audio(&self, enabled: bool, send: &mut SendStream) -> Result<()> {
        if !self.v3() {
            return Ok(());
        }
        if enabled && !self.allow(perms::AUDIO) {
            return self.deny(send, "audio", "audio was not allowed").await;
        }
        if !enabled {
            self.audio_stop.store(true, Ordering::SeqCst);
            return Ok(());
        }
        self.audio_stop.store(false, Ordering::SeqCst);
        let conn = self.conn.clone();
        let flag = Arc::clone(&self.audio_stop);
        let tx = self.host_tx.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = crate::audio::loopback_to(conn, flag) {
                warn!("Audio capture stopped: {e:#}");
                let _ = tx.send(HostMsg::Denied { what: "audio".into(), reason: format!("{e:#}") });
            }
        });
        Ok(())
    }

    async fn file_ctrl(&self, msg: FileMsg, send: &mut SendStream) -> Result<()> {
        if !self.v3() {
            return Ok(());
        }
        if !self.allow(perms::FILE) {
            let id = file_id(&msg);
            return write_msg(send, &HostMsg::File(FileMsg::Reject { id, reason: "file transfer was not allowed".into() }))
                .await;
        }
        match msg {
            FileMsg::Offer { id, name, size, resume_from } => self.offer_in(id, &name, size, resume_from, send).await?,
            FileMsg::Accept { id, resume_from } => self.accept_out(id, resume_from),
            FileMsg::Cancel { id } => {
                self.expected.lock().unwrap().remove(&id);
                self.outgoing.lock().unwrap().remove(&id);
            }
            FileMsg::Reject { id, reason } => {
                self.outgoing.lock().unwrap().remove(&id);
                self.note(&format!("Transfer {id} refused: {reason}"));
            }
            FileMsg::Progress { .. } | FileMsg::Done { .. } | FileMsg::Error { .. } => {}
        }
        Ok(())
    }

    async fn offer_in(&self, id: u64, name: &str, size: u64, resume_from: u64, send: &mut SendStream) -> Result<()> {
        let dir = self.receive_dir.lock().unwrap().clone();
        let root = PathBuf::from(dir.trim());
        if root.as_os_str().is_empty() || !root.is_dir() {
            return write_msg(
                send,
                &HostMsg::File(FileMsg::Reject { id, reason: "choose a receive folder on this computer first".into() }),
            )
            .await;
        }
        if size > MAX_TRANSFER_BYTES || filesafe::sanitize_name(name).is_err() {
            return write_msg(send, &HostMsg::File(FileMsg::Reject { id, reason: "that file is not accepted".into() }))
                .await;
        }
        let have = filesafe::partial_len(&root, name).unwrap_or(0);
        let offset = if resume_from == have { have } else { 0 };
        self.expected.lock().unwrap().insert(id, ExpectedFile { name: name.to_string(), size, offset });
        info!("Accepting file \"{name}\" ({size} bytes) from {}, resume {offset}", self.peer_name);
        self.note(&format!("Receiving {name}"));
        write_msg(send, &HostMsg::File(FileMsg::Accept { id, resume_from: offset })).await
    }

    fn accept_out(&self, id: u64, resume_from: u64) {
        let Some(job) = self.outgoing.lock().unwrap().get(&id).map(|j| OutFile {
            id: j.id,
            path: j.path.clone(),
            resume: resume_from,
        }) else {
            return;
        };
        let conn = self.conn.clone();
        let tx = self.host_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = send_file_bytes(conn, job.id, &job.path, job.resume, tx.clone()).await {
                let _ = tx.send(HostMsg::File(FileMsg::Error { id: job.id, reason: format!("{e:#}") }));
            }
        });
    }

    /// Local UI asked to send a file or a chat line.
    pub async fn poll_local(&self, send: &mut SendStream) -> Result<()> {
        let Some(shared) = &self.shared else { return Ok(()) };
        if self.allow(perms::CHAT) {
            for line in shared.take_chat_out() {
                write_msg(send, &HostMsg::Chat { text: clip_line(&line, 2000) }).await?;
            }
        }
        if self.allow(perms::FILE) && self.v3() {
            for path in shared.take_send_files() {
                self.queue_send(send, path).await?;
            }
        }
        Ok(())
    }

    async fn queue_send(&self, send: &mut SendStream, path: PathBuf) -> Result<()> {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if filesafe::sanitize_name(&name).is_err() {
            self.note("That file name cannot be sent");
            return Ok(());
        }
        let size = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() <= MAX_TRANSFER_BYTES => m.len(),
            _ => {
                self.note("That file cannot be sent");
                return Ok(());
            }
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.outgoing.lock().unwrap().insert(id, OutFile { id, path, resume: 0 });
        self.note(&format!("Offering {name}"));
        write_msg(send, &HostMsg::File(FileMsg::Offer { id, name, size, resume_from: 0 })).await
    }

    pub fn release_privacy(&self) {
        if self.privacy.swap(false, Ordering::SeqCst) {
            crate::platform::set_privacy_screen(false);
        }
        if self.blocked.swap(false, Ordering::SeqCst) {
            crate::platform::set_block_input(false);
        }
        self.audio_stop.store(true, Ordering::SeqCst);
    }

    fn note(&self, text: &str) {
        if let Some(shared) = &self.shared {
            shared.note_transfer(text);
        }
    }

    pub fn spawn_side_loops(self: &Arc<Self>) {
        let session = self.clone();
        let conn = self.conn.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    incoming = conn.accept_uni() => {
                        let Ok(stream) = incoming else { break };
                        let session = session.clone();
                        tokio::spawn(async move {
                            if let Err(e) = session.read_side(stream).await {
                                warn!("Side stream ended: {e:#}");
                            }
                        });
                    }
                    incoming = conn.accept_bi() => {
                        let Ok((send, recv)) = incoming else { break };
                        let session = session.clone();
                        tokio::spawn(async move {
                            if let Err(e) = session.read_tunnel(send, recv).await {
                                warn!("Tunnel ended: {e:#}");
                            }
                        });
                    }
                }
            }
        });
    }

    async fn read_side(&self, mut stream: RecvStream) -> Result<()> {
        let (magic, header) = read_side_header::<_, SideHeader>(&mut stream).await?;
        if &magic == FILE_MAGIC {
            self.receive_file(header, &mut stream).await?;
        } else if &magic == CLIP_MAGIC {
            self.receive_clip_image(header, &mut stream).await?;
        }
        Ok(())
    }

    async fn receive_file(&self, header: SideHeader, stream: &mut RecvStream) -> Result<()> {
        let job = self.expected.lock().unwrap().get(&header.id).map(|j| ExpectedFile {
            name: j.name.clone(),
            size: j.size,
            offset: j.offset,
        });
        let Some(job) = job else {
            let _ = self.host_tx.send(HostMsg::File(FileMsg::Error { id: header.id, reason: "unexpected file stream".into() }));
            return Ok(());
        };
        let root = PathBuf::from(self.receive_dir.lock().unwrap().trim());
        let (mut file, offset) = filesafe::open_for_receive(&root, &job.name, header.offset)?;
        let mut got = offset;
        while let Some(chunk) = read_side_chunk(stream).await? {
            if got.saturating_add(chunk.len() as u64) > job.size.max(MAX_TRANSFER_BYTES) {
                let _ = self.host_tx.send(HostMsg::File(FileMsg::Error { id: header.id, reason: "file grew past the offered size".into() }));
                return Ok(());
            }
            file.write_all(&chunk).context("writing the received file")?;
            got += chunk.len() as u64;
            if got % (256 * 1024) < chunk.len() as u64 {
                let _ = self.host_tx.send(HostMsg::File(FileMsg::Progress { id: header.id, bytes: got }));
            }
        }
        file.flush()?;
        drop(file);
        if got != job.size && job.size != 0 {
            let _ = self.host_tx.send(HostMsg::File(FileMsg::Error {
                id: header.id,
                reason: format!("received {got} of {} bytes; the partial file was kept", job.size),
            }));
            return Ok(());
        }
        let saved = filesafe::finish_receive(&root, &job.name)?;
        self.expected.lock().unwrap().remove(&header.id);
        info!("Saved {}", saved.display());
        self.note(&format!("Saved {}", job.name));
        let _ = self.host_tx.send(HostMsg::File(FileMsg::Done { id: header.id }));
        Ok(())
    }

    async fn receive_clip_image(&self, header: SideHeader, stream: &mut RecvStream) -> Result<()> {
        if !self.allow(perms::CLIPBOARD) || self.view_only {
            return Ok(());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = read_side_chunk(stream).await? {
            if bytes.len() + chunk.len() > MAX_CLIP_IMAGE as usize {
                return Ok(());
            }
            bytes.extend_from_slice(&chunk);
        }
        let _ = header;
        if let Some((w, h, rgba)) = crate::clip::decode_png(&bytes) {
            crate::clip::set_image(w, h, &rgba);
        }
        Ok(())
    }

    async fn tunnel(&self, msg: TunnelMsg, send: &mut SendStream) -> Result<()> {
        let TunnelMsg::Open { id, remote_host, remote_port } = msg else {
            return Ok(());
        };
        if !self.allow(perms::TUNNEL) || !self.v3() {
            return write_msg(send, &HostMsg::Tunnel(TunnelMsg::Close { id, reason: "port forwarding was not allowed".into() })).await;
        }
        if !discover::tunnel_allowed(&remote_host, remote_port, &self.tunnel_ports, self.tunnel_remote) {
            return write_msg(
                send,
                &HostMsg::Tunnel(TunnelMsg::Close { id, reason: "that host or port is not in the forward list".into() }),
            )
            .await;
        }
        let addr = format!("{remote_host}:{remote_port}");
        match tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(tcp)) => {
                tcp.set_nodelay(true).ok();
                let std_tcp = tcp.into_std()?;
                tunnels().lock().unwrap().insert(id, std_tcp);
                info!("Tunnel {id} to {addr}");
                write_msg(send, &HostMsg::Tunnel(TunnelMsg::Ready { id })).await
            }
            _ => {
                write_msg(send, &HostMsg::Tunnel(TunnelMsg::Close { id, reason: format!("could not connect to {addr}") })).await
            }
        }
    }

    async fn read_tunnel(&self, mut quic_send: SendStream, mut quic_recv: RecvStream) -> Result<()> {
        let mut idb = [0u8; 8];
        quic_recv.read_exact(&mut idb).await?;
        let id = u64::from_be_bytes(idb);
        let Some(std_tcp) = tunnels().lock().unwrap().remove(&id) else {
            return Ok(());
        };
        std_tcp.set_nonblocking(true).ok();
        let mut tcp = tokio::net::TcpStream::from_std(std_tcp)?;
        let mut quic = tokio::io::join(quic_recv, quic_send);
        let _ = tokio::io::copy_bidirectional(&mut quic, &mut tcp).await;
        Ok(())
    }
}

fn file_id(msg: &FileMsg) -> u64 {
    match msg {
        FileMsg::Offer { id, .. }
        | FileMsg::Accept { id, .. }
        | FileMsg::Reject { id, .. }
        | FileMsg::Progress { id, .. }
        | FileMsg::Done { id }
        | FileMsg::Cancel { id }
        | FileMsg::Error { id, .. } => *id,
    }
}

fn clip_line(text: &str, max: usize) -> String {
    text.chars().filter(|c| *c != '\n' && *c != '\r' && !c.is_control()).take(max).collect()
}

fn open_task_manager() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        return std::process::Command::new("taskmgr.exe").creation_flags(0x0800_0000).spawn().is_ok();
    }
    #[cfg(not(windows))]
    {
        false
    }
}

static TUNNELS: std::sync::OnceLock<Mutex<HashMap<u64, std::net::TcpStream>>> = std::sync::OnceLock::new();

fn tunnels() -> &'static Mutex<HashMap<u64, std::net::TcpStream>> {
    TUNNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn send_file_bytes(
    conn: Connection,
    id: u64,
    path: &Path,
    resume: u64,
    tx: mpsc::UnboundedSender<HostMsg>,
) -> Result<()> {
    let mut file = std::fs::File::open(path)?;
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(resume))?;
    let mut uni = conn.open_uni().await?;
    write_side_header(&mut uni, FILE_MAGIC, &SideHeader { id, offset: resume, a: 0, b: 0 }).await?;
    let mut buf = vec![0u8; SIDE_CHUNK];
    let mut sent = resume;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        write_side_chunk(&mut uni, &buf[..n]).await?;
        sent += n as u64;
        let _ = tx.send(HostMsg::File(FileMsg::Progress { id, bytes: sent }));
    }
    write_side_chunk(&mut uni, &[]).await?;
    let _ = uni.finish();
    let _ = tx.send(HostMsg::File(FileMsg::Done { id }));
    Ok(())
}

impl Session {
    pub fn set_receive_dir(&self, dir: String) {
        *self.receive_dir.lock().unwrap() = dir;
    }
}

