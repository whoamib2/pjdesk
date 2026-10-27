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
    ControlMsg, DisplayLayout, FileMsg, HostMsg, MonitorInfo, SideHeader, TunnelMsg, VideoHeader, AUDIO_MAGIC,
    AtlasTile, CLIP_MAGIC, FILE_MAGIC, MAX_CLIP_IMAGE, MAX_CLIP_TEXT, VIEW_ALL, VIEW_SEPARATE, VIEW_SINGLE,
};
use pjdesk_proto::perms::{self, has};
use pjdesk_proto::quality;
use quinn::Connection;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

/// The viewer window runs on the winit thread, which is not inside the Tokio
/// runtime. `tokio::spawn` from that thread panics ("there is no reactor
/// running"). Tasks go through the handle captured when the session starts.
static VIEWER_RT: std::sync::OnceLock<Handle> = std::sync::OnceLock::new();

fn spawn_viewer<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(fut);
        return;
    }
    VIEWER_RT
        .get()
        .expect("viewer task started before the session runtime was set")
        .spawn(fut);
}

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
    pub displays: Vec<DisplayLayout>,
    pub platform_family: String,
    pub platform_version: String,
    pub view_mode: u8,
    pub current_display: u8,
    /// View mode and display chosen when this session opened. Not the process environment,
    /// so a second computer in the same window does not inherit the first one's screen.
    opening_view: u8,
    opening_display: u8,
    pub atlas_w: u16,
    pub atlas_h: u16,
    pub atlas: Vec<AtlasTile>,
    pub remote_files: Vec<String>,
    pub transfers: Vec<Transfer>,
    pub chat: Vec<(bool, String)>,
    pub notices: Vec<String>,
    pub privacy: bool,
    pub audio: bool,
    pub block_host: bool,
    pub recording: bool,
    pub restart: bool,
    /// The QUIC path is the server relay.
    pub relayed: bool,
    /// That relay path dropped. The window reconnects without asking again.
    pub resume_relay: bool,
    /// Primary the relayed session should open on. Empty uses the account's current primary.
    pub move_primary: String,
    pub closed: Option<String>,
    /// The phone's accessibility service is off, so clicks do not land.
    pub control_off: bool,
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
            displays: Vec::new(),
            platform_family: String::new(),
            platform_version: String::new(),
            view_mode: VIEW_SINGLE,
            current_display: 0,
            opening_view: VIEW_SINGLE,
            opening_display: 0,
            atlas_w: 0,
            atlas_h: 0,
            atlas: Vec::new(),
            remote_files: Vec::new(),
            transfers: Vec::new(),
            chat: Vec::new(),
            notices: Vec::new(),
            privacy: false,
            audio: false,
            block_host: false,
            recording: false,
            restart: false,
            relayed: false,
            resume_relay: false,
            move_primary: String::new(),
            closed: None,
            control_off: false,
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
            displays: self.displays.clone(),
            platform_family: self.platform_family.clone(),
            platform_version: self.platform_version.clone(),
            view_mode: self.view_mode,
            current_display: self.current_display,
            opening_view: self.opening_view,
            opening_display: self.opening_display,
            atlas_w: self.atlas_w,
            atlas_h: self.atlas_h,
            atlas: self.atlas.clone(),
            remote_files: self.remote_files.clone(),
            transfers: self.transfers.clone(),
            chat: self.chat.clone(),
            notices: self.notices.clone(),
            privacy: self.privacy,
            audio: self.audio,
            block_host: self.block_host,
            recording: self.recording,
            restart: self.restart,
            relayed: self.relayed,
            resume_relay: self.resume_relay,
            move_primary: self.move_primary.clone(),
            closed: self.closed.clone(),
            control_off: self.control_off,
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
    rt: Handle,
    connection: Connection,
    control_recv: quinn::RecvStream,
    control: mpsc::UnboundedSender<ControlMsg>,
    frames: std::sync::mpsc::Sender<(VideoHeader, Vec<u8>)>,
    peer_proto: u16,
    host_id: String,
    relayed: bool,
    opening_view: Option<u8>,
    opening_display: Option<u8>,
    wake: impl Fn() + Send + Sync + 'static,
) -> (Arc<Mutex<Live>>, mpsc::UnboundedSender<Cmd>, Arc<Mutex<RecorderSlot>>) {
    let _ = VIEWER_RT.set(rt);
    let mut session = Live::new(peer_proto >= 3);
    session.relayed = relayed;
    session.opening_display = opening_display
        .or_else(|| std::env::var("PJDESK_DISPLAY").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    let view = opening_view
        .or_else(|| std::env::var("PJDESK_VIEW").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(VIEW_SINGLE);
    session.opening_view = match view {
        VIEW_ALL => VIEW_ALL,
        VIEW_SEPARATE => VIEW_SEPARATE,
        _ => VIEW_SINGLE,
    };
    let saved_display = session.opening_display;
    let live = Arc::new(Mutex::new(session));
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let recorder = Arc::new(Mutex::new(RecorderSlot::new()));
    let wake = Arc::new(wake);
    let player = Arc::new(Mutex::new(None::<Player>));
    let tunnels = Arc::new(Mutex::new(PendingTunnel { waiters: HashMap::new() }));

    if peer_proto >= 3 {
        let _ = control.send(ControlMsg::Hello {
            permissions: perms::ALL | perms::LAYOUT | perms::VISITOR,
            monitor: saved_display,
            quality: quality::preset("balanced").unwrap_or_default(),
        });
    }

    let conn = connection.clone();
    let live_uni = live.clone();
    let player_uni = player.clone();
    spawn_viewer(async move {
        dispatch_uni(conn, frames, live_uni, player_uni).await;
    });

    let live_loop = live.clone();
    let control_loop = control.clone();
    let conn_loop = connection.clone();
    let wake_loop = wake.clone();
    let recorder_loop = recorder.clone();
    spawn_viewer(async move {
            run_loop(
                conn_loop,
                control_recv,
                control_loop,
                cmd_rx,
                live_loop,
                tunnels,
                recorder_loop,
                wake_loop,
                host_id,
            )
            .await;
        });

    let live_clip = live.clone();
    let control_clip = control;
    let wake_clip = wake;
    spawn_viewer(async move {
        clip_watch(live_clip, control_clip, wake_clip).await;
    });

    (live, cmd_tx, recorder)
}

fn note_relay_resume(live: &mut Live) {
    live.resume_relay = true;
    live.closed = Some(pjdesk_proto::failover::RELAY_RESUME.into());
    live.note("Reconnecting…");
    tracing::info!("{}", pjdesk_proto::failover::RELAY_RESUME);
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
    host_id: String,
) {
    loop {
        tokio::select! {
            msg = read_msg::<_, HostMsg>(&mut control_recv, MAX_CONTROL_FRAME) => {
                match msg {
                    Ok(msg) => on_host(msg, &connection, &control, &live, &tunnels, &wake, &host_id),
                    Err(e) => {
                        let mut g = live.lock().unwrap();
                        if g.closed.is_none() && !g.restart {
                            if g.relayed {
                                note_relay_resume(&mut g);
                            } else {
                                let text = format!("The host closed the session: {e:#}");
                                tracing::error!("{text}");
                                crate::note_failure(&text);
                                g.closed = Some(text);
                            }
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
                    if g.relayed {
                        note_relay_resume(&mut g);
                    } else {
                        let why = connection
                            .close_reason()
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "the connection ended".into());
                        let text = session_end_message(&why);
                        tracing::error!("{text}");
                        crate::note_failure(&text);
                        g.closed = Some(text);
                    }
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
    host_id: &str,
) {
    let mut g = live.lock().unwrap();
    match msg {
        HostMsg::Welcome { permissions, monitors, .. } => {
            tracing::info!("Host granted {}", perms::describe(permissions));
            g.perms = permissions;
            g.monitors = monitors;
            g.note(format!("This session can {}", perms::describe(permissions)));
        }
        HostMsg::Displays { displays } => {
            let first = g.displays.is_empty();
            g.displays = displays;
            if first {
                let mode = g.opening_view;
                let display = g.opening_display;
                let display = if g.displays.iter().any(|d| d.index == display) {
                    display
                } else {
                    g.displays.first().map(|d| d.index).unwrap_or(0)
                };
                g.view_mode = mode;
                g.current_display = display;
                let _ = control.send(ControlMsg::View { mode, display });
                if mode == VIEW_SEPARATE {
                    let displays: Vec<u8> = g.displays.iter().map(|d| d.index).collect();
                    let _ = control.send(ControlMsg::Watch { displays });
                } else if mode == VIEW_SINGLE {
                    let _ = control.send(ControlMsg::SelectMonitor { index: display });
                }
            }
        }
        HostMsg::Atlas { width, height, tiles } => {
            g.atlas_w = width;
            g.atlas_h = height;
            g.atlas = tiles;
        }
        HostMsg::Platform { family, version } => {
            g.platform_family = family.clone();
            g.platform_version = version.clone();
            remember_platform(host_id, &family, &version);
        }
        HostMsg::Notice(text) => {
            tracing::info!("Host says: {text}");
            if text == CONTROL_OFF {
                g.control_off = true;
                crate::note_status(&text);
                g.note(text);
            } else if text == CONTROL_ON {
                g.control_off = false;
                crate::note_status(&text);
                g.note(text);
            } else if screen_share_failure_text(&text) || phone_stopped_sharing(&text) {
                crate::note_failure(&text);
                g.closed = Some(text);
            } else if screen_share_waiting(&text) {
                crate::note_status(&text);
                g.note(text);
            } else {
                crate::note_status(&format!("Host: {text}"));
                g.note(text);
            }
        }
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
                spawn_viewer(async move {
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
        HostMsg::MoveRelay { primary } => {
            g.move_primary = primary;
            g.note("Reconnecting…");
            note_relay_resume(&mut g);
            wake();
        }
        HostMsg::AskVisitor => {
            let _ = control.send(this_visitor());
        }
        HostMsg::Pong { .. } | HostMsg::Tunnel(TunnelMsg::Open { .. }) => {}
    }
    drop(g);
    wake();
}

/// Account email when this computer is signed in to pjdesk.com, otherwise blank.
/// The computer name, the person at this computer, and this computer's ID travel with it.
fn this_visitor() -> ControlMsg {
    let cfg = crate::config::Config::load().unwrap_or_default();
    let from_parent = std::env::var("PJDESK_ACCOUNT").unwrap_or_default();
    let email = if !from_parent.trim().is_empty() { from_parent } else { cfg.network_email.clone().unwrap_or_default() };
    let signed_in = !email.trim().is_empty()
        || !cfg.network_refresh_token().trim().is_empty()
        || !std::env::var("PJDESK_ACCESS").unwrap_or_default().trim().is_empty();
    let account = if signed_in { email.trim().chars().filter(|c| !c.is_control()).take(80).collect() } else { String::new() };
    ControlMsg::Visitor {
        account,
        computer: crate::hostname(),
        user: crate::hostfacts::console_user(),
        id: cfg.device_id.clone().unwrap_or_default(),
    }
}

fn remember_platform(host_id: &str, family: &str, version: &str) {
    if host_id.is_empty() {
        return;
    }
    let (id, family, version) = (host_id.to_string(), family.to_string(), version.to_string());
    std::thread::spawn(move || {
        if let Ok(mut cfg) = crate::config::Config::load() {
            let note = cfg.settings.peers.entry(id).or_default();
            note.os_family = family;
            note.os_version = version;
            let _ = cfg.save();
        }
    });
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
                spawn_viewer(async move {
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

/// The phone's accessibility service is not connected.
pub(crate) const CONTROL_OFF: &str = "Control not enabled on phone";
/// The accessibility service connected while the session was open.
pub(crate) const CONTROL_ON: &str = "Control is on";

/// The phone ended screen sharing. The session closes on this sentence.
pub(crate) fn phone_stopped_sharing(text: &str) -> bool {
    text.to_ascii_lowercase().contains("stopped sharing")
}

/// A connection close from the phone, as a sentence on the PC.
pub(crate) fn session_end_message(why: &str) -> String {
    if phone_stopped_sharing(why) {
        "The phone stopped sharing".into()
    } else if screen_share_failure_text(why) {
        why.to_string()
    } else {
        format!("Session stopped: {why}")
    }
}

/// The phone asked the person to allow screen sharing. Keep waiting.
pub(crate) fn screen_share_waiting(text: &str) -> bool {
    text.to_ascii_lowercase().contains("allow screen sharing")
}

/// A specific reason from the phone, shown instead of the generic picture timeout.
pub(crate) fn screen_share_failure_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("screen sharing not allowed")
        || text.contains("mediaprojection")
        || text.contains("foreground service is not running")
}

/// How long to wait for the first frame. A phone that is asking for screen
/// sharing gets a minute. Anything else stays at 15 seconds.
pub(crate) fn picture_limit_secs(notices: &[String]) -> u64 {
    if notices.iter().any(|note| screen_share_waiting(note)) {
        60
    } else {
        15
    }
}

/// How long to wait for the host to open the video stream. QUIC does not tell
/// the viewer the stream exists until the host writes a frame, and the phone
/// writes that frame only after screen-share consent. A phone that is still
/// asking gets the same minute. Anything else stays at 20 seconds.
pub(crate) fn video_open_limit_secs(notices: &[String]) -> u64 {
    if notices.iter().any(|note| screen_share_waiting(note)) {
        60
    } else {
        20
    }
}

/// The line the main window shows when no frame arrived.
pub(crate) fn picture_gave_up(host: &str, notices: &[String]) -> String {
    if let Some(reason) = notices.iter().rev().find(|note| screen_share_failure_text(note)) {
        return reason.clone();
    }
    if notices.iter().any(|note| screen_share_waiting(note)) {
        return "Timed out. No one at that phone allowed screen sharing.".into();
    }
    format!(
        "No picture arrived from {host}. The password was accepted, but that computer did not start sending its screen. {}",
        crate::host_log_hint()
    )
}

/// Wait until the host writes on a uni stream, or until the consent minute ends.
/// The limit is re-read so "Waiting for the phone to allow screen sharing"
/// extends a wait that started at 20 seconds.
async fn await_video_stream(connection: &Connection, live: &Arc<Mutex<Live>>) -> Result<quinn::RecvStream, String> {
    let started = tokio::time::Instant::now();
    loop {
        let notices = live.lock().unwrap().notices.clone();
        let limit = Duration::from_secs(video_open_limit_secs(&notices));
        let elapsed = started.elapsed();
        if elapsed >= limit {
            if notices.iter().any(|note| screen_share_waiting(note)) {
                return Err(picture_gave_up("the phone", &notices));
            }
            let why = connection
                .close_reason()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "the host never opened the video stream".into());
            return Err(format!("No picture from the host: {why}"));
        }
        let slice = (limit - elapsed).min(Duration::from_millis(250));
        match tokio::time::timeout(slice, connection.accept_uni()).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => return Err(format!("The host did not open the video stream: {e}")),
            Err(_) => {
                if let Some(reason) = connection.close_reason() {
                    return Err(format!("No picture from the host: {reason}"));
                }
            }
        }
    }
}

async fn dispatch_uni(
    connection: Connection,
    frames: std::sync::mpsc::Sender<(VideoHeader, Vec<u8>)>,
    live: Arc<Mutex<Live>>,
    player: Arc<Mutex<Option<Player>>>,
) {
    let video = match await_video_stream(&connection, &live).await {
        Ok(stream) => stream,
        Err(text) => {
            tracing::error!("{text}");
            crate::note_failure(&text);
            live.lock().unwrap().closed = Some(text);
            return;
        }
    };
    tracing::info!("Video stream received from the host");
    spawn_viewer(async move {
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
        spawn_viewer(async move {
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
        spawn_viewer(async move {
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
        spawn_viewer(async move {
            if let Ok(mut uni) = conn.open_uni().await {
                let _ = write_side_header(&mut uni, CLIP_MAGIC, &SideHeader { id: 0, offset: 0, a: w as u32, b: h as u32 }).await;
                let _ = write_side_chunk(&mut uni, &png).await;
                let _ = write_side_chunk(&mut uni, &[]).await;
                let _ = uni.finish();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::time::Duration;

    #[test]
    fn a_phone_that_is_asking_for_the_screen_waits_a_minute_and_names_the_reason() {
        assert_eq!(picture_limit_secs(&[]), 15);
        let waiting = "Waiting for the phone to allow screen sharing".to_string();
        assert_eq!(picture_limit_secs(&[waiting.clone()]), 60);
        assert_eq!(video_open_limit_secs(&[]), 20);
        assert_eq!(video_open_limit_secs(&[waiting.clone()]), 60);
        assert_eq!(
            picture_gave_up("Pixel", &[waiting]),
            "Timed out. No one at that phone allowed screen sharing."
        );
        let denied = "screen sharing not allowed on the phone: Screen capture permission denied".to_string();
        assert_eq!(picture_gave_up("Pixel", &[denied.clone()]), denied);
        assert!(picture_gave_up("Pixel", &[]).contains("No picture arrived from Pixel"));
        assert!(phone_stopped_sharing("The phone stopped sharing"));
        assert_eq!(session_end_message("The phone stopped sharing"), "The phone stopped sharing");
        assert_eq!(CONTROL_OFF, "Control not enabled on phone");
    }

    /// The window thread calls `start` after `block_on` has returned, so there
    /// is no current reactor. 0.6.6 panicked here.
    #[test]
    fn start_outside_the_runtime_does_not_panic() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let (connection, recv) = rt.block_on(session_pair());
        assert!(Handle::try_current().is_err(), "this thread must be outside the Tokio runtime");
        let (control, _rx) = mpsc::unbounded_channel();
        let (frames, _frames_rx) = std::sync::mpsc::channel();
        let _ = start(
            rt.handle().clone(),
            connection.clone(),
            recv,
            control,
            frames,
            3,
            "100000001".into(),
            false,
            None,
            None,
            || {},
        );
        connection.close(0u32.into(), b"test");
        rt.shutdown_timeout(Duration::from_secs(3));
    }

    async fn session_pair() -> (Connection, quinn::RecvStream) {
        let identity = pjdesk_proto::tls::TlsIdentity::generate("pjdesk-host").unwrap();
        let host_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        host_sock.set_nonblocking(true).unwrap();
        let addr = host_sock.local_addr().unwrap();
        let host = crate::p2p::host_endpoint(host_sock, &identity).unwrap();
        let client_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_sock.set_nonblocking(true).unwrap();
        let client = crate::p2p::controller_endpoint(client_sock, identity.fingerprint()).unwrap();
        let connecting = client.connect(addr, pjdesk_proto::PEER_SERVER_NAME).unwrap();
        let incoming = host.accept().await.expect("host accept").await.expect("host handshake");
        let connection = connecting.await.expect("client handshake");
        let mut send = incoming.open_uni().await.expect("open uni");
        send.write_all(&[0]).await.unwrap();
        let recv = connection.accept_uni().await.expect("accept uni");
        // The QUIC driver lives in the endpoints. Keep them for the rest of the test.
        std::mem::forget(host);
        std::mem::forget(client);
        std::mem::forget(send);
        (connection, recv)
    }
}
