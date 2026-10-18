//! Protocol 3 session on the controller: permissions, files, clipboard, chat,
//! audio, tunnels, and the host messages that drive the panel.
//!
//! The first unidirectional stream is the picture. Later ones are files, audio,
//! or a clipboard image. Version-2 hosts never receive [`ControlMsg::Hello`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pjdesk_proto::filesafe::{self, MAX_TRANSFER_BYTES};
use pjdesk_proto::framing::{read_msg, MAX_CONTROL_FRAME};
use pjdesk_proto::peer::{
    read_side_chunk, read_side_header, read_video_frame, write_side_chunk, write_side_header, Clipboard, ClipKind,
    ControlMsg, FileMsg, HostMsg, MonitorInfo, SideHeader, TunnelMsg, VideoHeader, AUDIO_MAGIC, CLIP_MAGIC, FILE_MAGIC,
    MAX_CLIP_IMAGE, MAX_CLIP_TEXT,
};
use pjdesk_proto::perms::{self, has};
use pjdesk_proto::quality;
use quinn::Connection;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

use crate::audio::Player;
use crate::clip;

#[derive(Clone)]
pub struct Transfer {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub bytes: u64,
    pub incoming: bool,
    pub state: String,
}

pub struct Live {
    pub v3: bool,
    pub perms: u32,
    pub monitors: Vec<MonitorInfo>,
    pub remote_files: Vec<String>,
    pub transfers: Vec<Transfer>,
    pub chat: Vec<(bool, String)>,
    pub notices: Vec<String>,
    pub privacy: bool,
    pub audio: bool,
    pub block_host: bool,
    pub recording: bool,
    pub restart: bool,
    pub closed: Option<String>,
    paths: HashMap<u64, PathBuf>,
    wanted: HashSet<String>,
    next_id: u64,
    clip_text: Option<String>,
}

impl Live {
    fn new(v3: bool) -> Self {
        Self {
            v3,
            perms: if v3 { 0 } else { perms::CLASSIC },
            monitors: Vec::new(),
            remote_files: Vec::new(),
            transfers: Vec::new(),
            chat: Vec::new(),
            notices: Vec::new(),
            privacy: false,
            audio: false,
            block_host: false,
            recording: false,
            restart: false,
            closed: None,
            paths: HashMap::new(),
            wanted: HashSet::new(),
            next_id: 1,
            clip_text: None,
        }
    }

    pub fn allowed(&self, bit: u32) -> bool {
        self.v3 && has(self.perms, bit)
    }

    /// Stores text we just placed on the clipboard so the watcher does not send it back.
    pub fn remember_clip(&mut self, text: &str) {
        self.clip_text = Some(text.to_string());
    }

    /// A copy of the fields the panel draws. Taken so the window can drop the lock.
    pub fn snapshot(&self) -> Self {
        Self {
            v3: self.v3,
            perms: self.perms,
            monitors: self.monitors.clone(),
            remote_files: self.remote_files.clone(),
            transfers: self.transfers.clone(),
            chat: self.chat.clone(),
            notices: self.notices.clone(),
            privacy: self.privacy,
            audio: self.audio,
            block_host: self.block_host,
            recording: self.recording,
            restart: self.restart,
            closed: self.closed.clone(),
            paths: HashMap::new(),
            wanted: HashSet::new(),
            next_id: 0,
            clip_text: None,
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.notices.push(text.into());
        if self.notices.len() > 8 {
            self.notices.remove(0);
        }
    }

    fn transfer_mut(&mut self, id: u64) -> Option<&mut Transfer> {
        self.transfers.iter_mut().find(|t| t.id == id)
    }
}

pub enum Cmd {
    SendFile(PathBuf),
    Accept(u64),
    Pull(String),
    ListRemote,
    Tunnel { local: u16, host: String, port: u16 },
    Record(bool),
    ClipboardImage,
}

struct PendingTunnel {
    waiters: HashMap<u64, oneshot::Sender<Result<(), String>>>,
}

/// Starts the host-message loop and the side-stream reader.
pub fn start(
    connection: Connection,
    control_recv: quinn::RecvStream,
    control: mpsc::UnboundedSender<ControlMsg>,
    frames: std::sync::mpsc::Sender<(VideoHeader, Vec<u8>)>,
    peer_proto: u16,
    wake: impl Fn() + Send + Sync + 'static,
) -> (Arc<Mutex<Live>>, mpsc::UnboundedSender<Cmd>, Arc<Mutex<RecorderSlot>>) {
    let live = Arc::new(Mutex::new(Live::new(peer_proto >= 3)));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let recorder = Arc::new(Mutex::new(RecorderSlot::new()));
    let wake = Arc::new(wake);
    let player = Arc::new(Mutex::new(None::<Player>));
    let tunnels = Arc::new(Mutex::new(PendingTunnel { waiters: HashMap::new() }));

    if peer_proto >= 3 {
        let _ = control.send(ControlMsg::Hello {
            permissions: perms::ALL,
            monitor: 0,
            quality: quality::preset("balanced").unwrap_or_default(),
        });
    }

    let conn = connection.clone();
    let live_uni = live.clone();
    let player_uni = player.clone();
    tokio::spawn(async move {
        dispatch_uni(conn, frames, live_uni, player_uni).await;
    });

    let live_loop = live.clone();
    let control_loop = control.clone();
    let conn_loop = connection.clone();
    let wake_loop = wake.clone();
    let recorder_loop = recorder.clone();
    tokio::spawn(async move {
        run_loop(conn_loop, control_recv, control_loop, cmd_rx, live_loop, tunnels, recorder_loop, wake_loop).await;
    });

    let live_clip = live.clone();
    let control_clip = control;
    let wake_clip = wake;
    tokio::spawn(async move {
        clip_watch(live_clip, control_clip, wake_clip).await;
    });

    (live, cmd_tx, recorder)
}

async fn run_loop(
    connection: Connection,
    mut control_recv: quinn::RecvStream,
    control: mpsc::UnboundedSender<ControlMsg>,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    live: Arc<Mutex<Live>>,
    tunnels: Arc<Mutex<PendingTunnel>>,
    recorder: Arc<Mutex<RecorderSlot>>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    loop {
        tokio::select! {
            msg = read_msg::<_, HostMsg>(&mut control_recv, MAX_CONTROL_FRAME) => {
                match msg {
                    Ok(msg) => on_host(msg, &connection, &control, &live, &tunnels, &wake),
                    Err(_) => {
                        let mut g = live.lock().unwrap();
                        if g.closed.is_none() && !g.restart {
                            g.closed = Some("the host closed the session".into());
                        }
                        wake();
                        break;
                    }
                }
            }
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                on_cmd(cmd, &connection, &control, &live, &tunnels, &recorder, &wake).await;
            }
            _ = connection.closed() => {
                let mut g = live.lock().unwrap();
                if g.closed.is_none() && !g.restart {
                    g.closed = Some("the connection ended".into());
                }
                wake();
                break;
            }
        }
    }
}

fn on_host(
    msg: HostMsg,
    connection: &Connection,
    control: &mpsc::UnboundedSender<ControlMsg>,
    live: &Arc<Mutex<Live>>,
    tunnels: &Arc<Mutex<PendingTunnel>>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) {
    let mut g = live.lock().unwrap();
    match msg {
        HostMsg::Welcome { permissions, monitors, .. } => {
            g.perms = permissions;
            g.monitors = monitors;
            g.note(format!("This session can {}", perms::describe(permissions)));
        }
        HostMsg::Notice(text) => g.note(text),
        HostMsg::Denied { what, reason } => g.note(format!("{what}: {reason}")),
        HostMsg::Chat { text } => g.chat.push((false, text)),
        HostMsg::Privacy { enabled } => g.privacy = enabled,
        HostMsg::Restarting => {
            g.restart = true;
            g.note("The other computer is restarting");
        }
        HostMsg::Clipboard(clip) => apply_clip(&mut g, clip),
        HostMsg::Tunnel(TunnelMsg::Ready { id }) => {
            if let Some(tx) = tunnels.lock().unwrap().waiters.remove(&id) {
                let _ = tx.send(Ok(()));
            }
        }
        HostMsg::Tunnel(TunnelMsg::Close { id, reason }) => {
            if let Some(tx) = tunnels.lock().unwrap().waiters.remove(&id) {
                let _ = tx.send(Err(reason.clone()));
            }
            g.note(format!("Port forward closed: {reason}"));
        }
        HostMsg::File(msg) => {
            let accept_id = match &msg {
                FileMsg::Offer { id, name, .. } if g.wanted.contains(name) => Some(*id),
                _ => None,
            };
            if let Some((id, path, resume)) = take_send(&mut g, &msg) {
                let conn = connection.clone();
                let live = std::sync::Arc::clone(live);
                let wake = std::sync::Arc::clone(wake);
                let control = control.clone();
                tokio::spawn(async move {
                    if let Err(e) = send_bytes(conn, id, &path, resume, &live).await {
                        if let Some(t) = live.lock().unwrap().transfer_mut(id) {
                            t.state = format!("{e:#}");
                        }
                        warn!("send file: {e:#}");
                    } else if let Some(t) = live.lock().unwrap().transfer_mut(id) {
                        t.bytes = t.size;
                        t.state = "done".into();
                    }
                    let _ = control.send(ControlMsg::File(FileMsg::Done { id }));
                    wake();
                });
            }
            if let Some(id) = accept_id {
                let resume = resume_download(&g, id);
                if let Some(t) = g.transfer_mut(id) {
                    t.bytes = resume;
                }
                let _ = control.send(ControlMsg::File(FileMsg::Accept { id, resume_from: resume }));
            }
        }
        HostMsg::Pong { .. } | HostMsg::Tunnel(TunnelMsg::Open { .. }) => {}
    }
    drop(g);
    wake();
}

fn apply_clip(g: &mut Live, clip: Clipboard) {
    if !g.allowed(perms::CLIPBOARD) {
        return;
    }
    match clip.kind {
        ClipKind::Text => {
            if let Ok(text) = String::from_utf8(clip.bytes) {
                g.clip_text = Some(text.clone());
                clip::set_text(&text);
            }
        }
        ClipKind::ImagePng => {
            if let Some((w, h, rgba)) = clip::decode_png(&clip.bytes) {
                clip::set_image(w, h, &rgba);
            }
        }
    }
}

/// When the host accepts an upload, returns the local file to send.
fn take_send(g: &mut Live, msg: &FileMsg) -> Option<(u64, PathBuf, u64)> {
    match msg {
        FileMsg::Entries { names, .. } => g.remote_files = names.clone(),
        FileMsg::Offer { id, name, size, resume_from } => {
            let auto = g.wanted.remove(name);
            g.transfers.push(Transfer {
                id: *id,
                name: name.clone(),
                size: *size,
                bytes: *resume_from,
                incoming: true,
                state: if auto { "downloading".into() } else { "offered".into() },
            });
            if auto {
                // Accepted below by the caller via a follow-up; stored as downloading.
            }
        }
        FileMsg::Accept { id, resume_from } => {
            if let Some(t) = g.transfer_mut(*id) {
                t.bytes = *resume_from;
                t.state = "sending".into();
            }
            if let Some(path) = g.paths.get(id).cloned() {
                return Some((*id, path, *resume_from));
            }
        }
        FileMsg::Progress { id, bytes } => {
            if let Some(t) = g.transfer_mut(*id) {
                t.bytes = *bytes;
            }
        }
        FileMsg::Done { id } => {
            if let Some(t) = g.transfer_mut(*id) {
                t.bytes = t.size;
                t.state = "done".into();
            }
        }
        FileMsg::Reject { id, reason } | FileMsg::Error { id, reason } => {
            if let Some(t) = g.transfer_mut(*id) {
                t.state = reason.clone();
            }
            g.note(reason.clone());
        }
        FileMsg::Cancel { id } => {
            if let Some(t) = g.transfer_mut(*id) {
                t.state = "cancelled".into();
            }
        }
        FileMsg::List { .. } | FileMsg::Pull { .. } => {}
    }
    None
}

async fn on_cmd(
    cmd: Cmd,
    connection: &Connection,
    control: &mpsc::UnboundedSender<ControlMsg>,
    live: &Arc<Mutex<Live>>,
    tunnels: &Arc<Mutex<PendingTunnel>>,
    recorder: &Arc<Mutex<RecorderSlot>>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) {
    match cmd {
        Cmd::SendFile(path) => {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let mut g = live.lock().unwrap();
            if !g.allowed(perms::FILE) {
                g.note("File transfer was not allowed for this session");
                drop(g);
                wake();
                return;
            }
            if filesafe::sanitize_name(&name).is_err() || size == 0 || size > MAX_TRANSFER_BYTES {
                g.note("That file cannot be sent");
                drop(g);
                wake();
                return;
            }
            let id = g.next_id;
            g.next_id += 1;
            g.paths.insert(id, path);
            g.transfers.push(Transfer {
                id,
                name: name.clone(),
                size,
                bytes: 0,
                incoming: false,
                state: "offering".into(),
            });
            drop(g);
            let _ = control.send(ControlMsg::File(FileMsg::Offer { id, name, size, resume_from: 0 }));
        }
        Cmd::Accept(id) => {
            let resume = resume_download(&live.lock().unwrap(), id);
            let _ = control.send(ControlMsg::File(FileMsg::Accept { id, resume_from: resume }));
            if let Some(t) = live.lock().unwrap().transfer_mut(id) {
                t.bytes = resume;
                t.state = "downloading".into();
            }
        }
        Cmd::Pull(name) => {
            let mut g = live.lock().unwrap();
            if !g.allowed(perms::FILE) {
                g.note("File transfer was not allowed for this session");
            } else {
                g.wanted.insert(name.clone());
                let id = g.next_id;
                g.next_id += 1;
                drop(g);
                let _ = control.send(ControlMsg::File(FileMsg::Pull { id, name }));
                wake();
                return;
            }
        }
        Cmd::ListRemote => {
            let mut g = live.lock().unwrap();
            if !g.allowed(perms::FILE) {
                g.note("File transfer was not allowed for this session");
            } else {
                let id = g.next_id;
                g.next_id += 1;
                drop(g);
                let _ = control.send(ControlMsg::File(FileMsg::List { id, path: String::new() }));
            }
        }
        Cmd::Tunnel { local, host, port } => {
            if !live.lock().unwrap().allowed(perms::TUNNEL) {
                live.lock().unwrap().note("Port forwarding was not allowed for this session");
            } else {
                let conn = connection.clone();
                let control = control.clone();
                let tunnels = tunnels.clone();
                let live = live.clone();
                let wake = wake.clone();
                tokio::spawn(async move {
                    if let Err(e) = listen_tunnel(conn, control, tunnels, local, host, port).await {
                        live.lock().unwrap().note(format!("Port forward: {e:#}"));
                        wake();
                    }
                });
            }
        }
        Cmd::ClipboardImage => {
            let g = live.lock().unwrap();
            if !g.allowed(perms::CLIPBOARD) {
                drop(g);
                live.lock().unwrap().note("Clipboard sync was not allowed for this session");
            } else {
                send_clipboard_image(control, connection, &g);
            }
        }
        Cmd::Record(on) => {
            let mut g = live.lock().unwrap();
            if on && !g.allowed(perms::RECORD) {
                g.note("Recording was not allowed for this session");
                g.recording = false;
            } else if on {
                recorder.lock().unwrap().arm();
                g.recording = true;
                g.note("Recording starts with the next picture");
            } else {
                g.recording = false;
                if let Some(rec) = recorder.lock().unwrap().stop() {
                    let _ = rec.finish();
                    g.note("Recording saved");
                }
            }
        }
    }
    wake();
}

/// Holds a recording until the first picture arrives, so the file uses that picture's size.
pub struct RecorderSlot {
    active: Option<pjdesk_proto::record::Recorder<std::fs::File>>,
    armed: bool,
}

impl RecorderSlot {
    fn new() -> Self {
        Self { active: None, armed: false }
    }

    fn arm(&mut self) {
        self.armed = true;
    }

    pub fn stop(&mut self) -> Option<pjdesk_proto::record::Recorder<std::fs::File>> {
        self.armed = false;
        self.active.take()
    }

    /// Appends one frame. The file is created on the first frame, at that frame's size.
    pub fn push(&mut self, annex_b: &[u8], keyframe: bool, width: u16, height: u16) -> anyhow::Result<()> {
        if self.active.is_none() {
            if !self.armed || width == 0 || height == 0 {
                return Ok(());
            }
            self.active = Some(open_recording(width, height)?);
        }
        if let Some(rec) = self.active.as_mut() {
            rec.push(annex_b, keyframe)?;
        }
        Ok(())
    }
}

fn open_recording(width: u16, height: u16) -> anyhow::Result<pjdesk_proto::record::Recorder<std::fs::File>> {
    let dir = recordings_dir();
    std::fs::create_dir_all(&dir)?;
    let name = format!(
        "PJDesk-{}.mp4",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    );
    let file = std::fs::File::create(dir.join(name))?;
    pjdesk_proto::record::Recorder::start(file, width, height, 30)
}

fn resume_download(g: &Live, id: u64) -> u64 {
    g.transfers.iter().find(|t| t.id == id).and_then(|t| filesafe::partial_len(&downloads_dir(), &t.name).ok()).unwrap_or(0)
}

pub fn recordings_dir() -> PathBuf {
    let base = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Videos").join("PJDesk")
}

pub fn downloads_dir() -> PathBuf {
    let base = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Downloads").join("PJDesk");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

async fn send_bytes(conn: Connection, id: u64, path: &Path, resume: u64, live: &Arc<Mutex<Live>>) -> anyhow::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(resume))?;
    let mut uni = conn.open_uni().await?;
    write_side_header(&mut uni, FILE_MAGIC, &SideHeader { id, offset: resume, a: 0, b: 0 }).await?;
    let mut buf = vec![0u8; pjdesk_proto::peer::SIDE_CHUNK];
    let mut sent = resume;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        write_side_chunk(&mut uni, &buf[..n]).await?;
        sent += n as u64;
        if let Some(t) = live.lock().unwrap().transfer_mut(id) {
            t.bytes = sent;
        }
    }
    write_side_chunk(&mut uni, &[]).await?;
    let _ = uni.finish();
    Ok(())
}

async fn dispatch_uni(
    connection: Connection,
    frames: std::sync::mpsc::Sender<(VideoHeader, Vec<u8>)>,
    live: Arc<Mutex<Live>>,
    player: Arc<Mutex<Option<Player>>>,
) {
    let video = match tokio::time::timeout(Duration::from_secs(20), connection.accept_uni()).await {
        Ok(Ok(s)) => s,
        _ => return,
    };
    tokio::spawn(async move {
        let mut video = video;
        loop {
            let mut data = Vec::new();
            match read_video_frame(&mut video, &mut data).await {
                Ok(header) => {
                    if frames.send((header, data)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    while let Ok(mut stream) = connection.accept_uni().await {
        let live = live.clone();
        let player = player.clone();
        tokio::spawn(async move {
            let _ = read_side(&mut stream, &live, &player).await;
        });
    }
}

async fn read_side(stream: &mut quinn::RecvStream, live: &Arc<Mutex<Live>>, player: &Arc<Mutex<Option<Player>>>) -> anyhow::Result<()> {
    let (magic, header) = read_side_header::<_, SideHeader>(stream).await?;
    if &magic == FILE_MAGIC {
        receive_file(stream, header, live).await?;
    } else if &magic == AUDIO_MAGIC {
        play_audio(stream, player).await?;
    } else if &magic == CLIP_MAGIC {
        let mut bytes = Vec::new();
        while let Some(chunk) = read_side_chunk(stream).await? {
            if bytes.len() + chunk.len() > MAX_CLIP_IMAGE as usize {
                break;
            }
            bytes.extend_from_slice(&chunk);
        }
        if live.lock().unwrap().allowed(perms::CLIPBOARD) {
            if let Some((w, h, rgba)) = clip::decode_png(&bytes) {
                clip::set_image(w, h, &rgba);
            }
        }
    }
    Ok(())
}

async fn receive_file(stream: &mut quinn::RecvStream, header: SideHeader, live: &Arc<Mutex<Live>>) -> anyhow::Result<()> {
    let name = {
        let g = live.lock().unwrap();
        g.transfers.iter().find(|t| t.id == header.id).map(|t| t.name.clone())
    };
    let Some(name) = name else { return Ok(()) };
    let root = downloads_dir();
    let (mut file, offset) = filesafe::open_for_receive(&root, &name, header.offset)?;
    let mut got = offset;
    while let Some(chunk) = read_side_chunk(stream).await? {
        std::io::Write::write_all(&mut file, &chunk)?;
        got += chunk.len() as u64;
        if let Some(t) = live.lock().unwrap().transfer_mut(header.id) {
            t.bytes = got;
        }
    }
    std::io::Write::flush(&mut file)?;
    drop(file);
    let _ = filesafe::finish_receive(&root, &name);
    if let Some(t) = live.lock().unwrap().transfer_mut(header.id) {
        t.bytes = got;
        t.state = "done".into();
    }
    Ok(())
}

async fn play_audio(stream: &mut quinn::RecvStream, player: &Arc<Mutex<Option<Player>>>) -> anyhow::Result<()> {
    if player.lock().unwrap().is_none() {
        *player.lock().unwrap() = crate::audio::start_player();
    }
    while let Some(chunk) = read_side_chunk(stream).await? {
        if let Some(p) = player.lock().unwrap().as_ref() {
            p.push(&chunk);
        }
    }
    Ok(())
}

async fn listen_tunnel(
    connection: Connection,
    control: mpsc::UnboundedSender<ControlMsg>,
    tunnels: Arc<Mutex<PendingTunnel>>,
    local: u16,
    host: String,
    port: u16,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", local)).await?;
    let next = AtomicU64::new(1);
    loop {
        let (mut tcp, _) = listener.accept().await?;
        let id = next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        tunnels.lock().unwrap().waiters.insert(id, tx);
        let _ = control.send(ControlMsg::Tunnel(TunnelMsg::Open { id, remote_host: host.clone(), remote_port: port }));
        let connection = connection.clone();
        let pending = tunnels.clone();
        tokio::spawn(async move {
            let ready = matches!(tokio::time::timeout(Duration::from_secs(8), rx).await, Ok(Ok(Ok(()))));
            if !ready {
                pending.lock().unwrap().waiters.remove(&id);
                return;
            }
            let Ok((mut send, recv)) = connection.open_bi().await else { return };
            if send.write_all(&id.to_be_bytes()).await.is_err() {
                return;
            }
            let mut quic = tokio::io::join(recv, send);
            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut quic).await;
        });
    }
}

async fn clip_watch(live: Arc<Mutex<Live>>, control: mpsc::UnboundedSender<ControlMsg>, wake: Arc<dyn Fn() + Send + Sync>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let (allowed, seen) = {
            let g = live.lock().unwrap();
            if g.closed.is_some() || g.restart {
                return;
            }
            (g.allowed(perms::CLIPBOARD), g.clip_text.clone())
        };
        if !allowed {
            continue;
        }
        let Some(text) = clip::get_text() else { continue };
        if text.len() > MAX_CLIP_TEXT || seen.as_ref() == Some(&text) {
            continue;
        }
        let first = seen.is_none();
        live.lock().unwrap().clip_text = Some(text.clone());
        if first || text.is_empty() {
            continue;
        }
        let _ = control.send(ControlMsg::Clipboard(Clipboard {
            kind: ClipKind::Text,
            byte_len: text.len() as u32,
            bytes: text.into_bytes(),
        }));
        wake();
    }
}

pub fn send_clipboard_image(control: &mpsc::UnboundedSender<ControlMsg>, connection: &Connection, live: &Live) {
    if !live.allowed(perms::CLIPBOARD) {
        return;
    }
    let Some((w, h, rgba)) = clip::get_image() else { return };
    let Some(png) = clip::encode_png(w as u32, h as u32, &rgba) else { return };
    if png.len() > MAX_CLIP_IMAGE as usize {
        return;
    }
    if png.len() <= 256 * 1024 {
        let _ = control.send(ControlMsg::Clipboard(Clipboard {
            kind: ClipKind::ImagePng,
            byte_len: png.len() as u32,
            bytes: png,
        }));
    } else {
        let conn = connection.clone();
        tokio::spawn(async move {
            if let Ok(mut uni) = conn.open_uni().await {
                let _ = write_side_header(&mut uni, CLIP_MAGIC, &SideHeader { id: 0, offset: 0, a: w as u32, b: h as u32 }).await;
                let _ = write_side_chunk(&mut uni, &png).await;
                let _ = write_side_chunk(&mut uni, &[]).await;
                let _ = uni.finish();
            }
        });
    }
}
