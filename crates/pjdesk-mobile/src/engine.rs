//! Host registration and controller sessions for the Android app.
//!
//! Video bytes stay in Kotlin (MediaCodec). This task owns the sockets.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use pjdesk_link::p2p::{self, LinkPath};
use pjdesk_link::session::{self, WrongPassword};
use pjdesk_link::signal::{ServerAddr, SignalClient};
use pjdesk_proto::auth::generate_password;
use pjdesk_proto::framing::{read_msg, write_msg, MAX_CONTROL_FRAME};
use pjdesk_proto::gesture::{Gesture, PointerAdapter, GESTURE_LONG_PRESS, GESTURE_PINCH, GESTURE_SWIPE, GESTURE_TAP};
use pjdesk_proto::peer::{
    read_video_frame, write_video_frame, ControlMsg, DisplayLayout, HostMsg, InputEvent, KeyboardMode, MonitorInfo,
    SpecialCommand, VideoHeader, VIEW_SINGLE,
};
use pjdesk_proto::perms::{self, grant};
use pjdesk_proto::quality::{self, StreamQuality};
use pjdesk_proto::signal::{ClientMsg, ServerMsg};
use pjdesk_proto::tls::TlsIdentity;
use pjdesk_proto::udp::Role;
use pjdesk_proto::{derive_device_secret, format_id, is_valid_id, normalize_id, DEFAULT_SERVER, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Shown in Register and on the About screen. Independent of the Windows crate version.
pub const APP_VERSION: &str = "0.6.0";

const MAX_IN_FLIGHT: u32 = 3;

/// A permanent password is optional. When set, it has to be at least 8 characters,
/// the same rule as the Windows client.
pub fn permanent_ok(password: &str) -> bool {
    password.trim().chars().count() >= 8
}

pub trait Callbacks: Send + Sync {
    fn on_event(&self, json: &str);
    fn on_video(&self, data: &[u8], width: u32, height: u32, keyframe: bool);
}

struct VideoOut {
    latest: Mutex<Option<Outgoing>>,
    notify: Notify,
    sent: AtomicU32,
    acked: AtomicU32,
    seq: AtomicU32,
}

struct Outgoing {
    data: Vec<u8>,
    width: u16,
    height: u16,
    keyframe: bool,
    encode_us: u32,
}

struct State {
    dir: PathBuf,
    hostname: String,
    secret: [u8; 32],
    identity: TlsIdentity,
    device_id: Mutex<Option<String>>,
    server: Mutex<String>,
    fingerprint: Mutex<Option<String>>,
    otp: Mutex<String>,
    permanent: Mutex<Option<String>>,
    approval: Mutex<String>,
    policy: Mutex<pjdesk_proto::signal::Policy>,
    capture: Mutex<(u16, u16)>,
    /// Bumped to end the current session. The host registration keeps running.
    gen: AtomicU64,
    approval_tx: Mutex<Option<oneshot::Sender<bool>>>,
    control: Mutex<Option<mpsc::UnboundedSender<ControlMsg>>>,
    video: Mutex<Option<Arc<VideoOut>>>,
    app_stop: AtomicBool,
}

#[derive(Serialize, Deserialize, Default)]
struct Disk {
    #[serde(default)]
    server: String,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    device_id: String,
    /// Used only when the device has no Android id to derive a secret from.
    #[serde(default)]
    secret_hex: String,
}

pub struct Engine {
    state: Arc<State>,
    cmds: mpsc::UnboundedSender<Value>,
}

impl Engine {
    pub fn start(dir: PathBuf, android_id: String, hostname: String, callbacks: Arc<dyn Callbacks>) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let disk = load_disk(&dir);
        let secret = device_secret(&android_id, &disk);
        let server = if disk.server.trim().is_empty() { DEFAULT_SERVER.to_string() } else { disk.server.clone() };
        let identity = TlsIdentity::load_or_create(&dir.join("host.crt"), &dir.join("host.key"), "pjdesk-host")?;
        let mut stored = disk;
        if android_id.trim().is_empty() && stored.secret_hex.is_empty() {
            stored.secret_hex = hex::encode(secret);
            save_disk(&dir, &stored);
        }
        let state = Arc::new(State {
            dir,
            hostname: if hostname.trim().is_empty() { "Android".into() } else { hostname },
            secret,
            identity,
            device_id: Mutex::new((!stored.device_id.is_empty()).then_some(stored.device_id)),
            server: Mutex::new(server),
            fingerprint: Mutex::new((!stored.fingerprint.is_empty()).then_some(stored.fingerprint)),
            otp: Mutex::new(generate_password()),
            permanent: Mutex::new(None),
            approval: Mutex::new("prompt".into()),
            policy: Mutex::new(pjdesk_proto::signal::Policy::default()),
            capture: Mutex::new((1080, 2400)),
            gen: AtomicU64::new(1),
            approval_tx: Mutex::new(None),
            control: Mutex::new(None),
            video: Mutex::new(None),
            app_stop: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let engine = Self { state: state.clone(), cmds: tx };
        let cb = callbacks;
        let _ = tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).try_init();
        std::thread::Builder::new().name("pjdesk-engine".into()).spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
            rt.block_on(run(state, rx, cb));
        })?;
        Ok(engine)
    }

    pub fn command(&self, json: &str) {
        if let Ok(v) = serde_json::from_str::<Value>(json) {
            let _ = self.cmds.send(v);
        }
    }

    pub fn submit_video(&self, data: Vec<u8>, width: u16, height: u16, keyframe: bool, encode_us: u32) {
        let Some(video) = self.state.video.lock().unwrap().clone() else { return };
        if data.is_empty() || width == 0 || height == 0 {
            return;
        }
        *video.latest.lock().unwrap() = Some(Outgoing { data, width, height, keyframe, encode_us });
        video.notify.notify_one();
    }
}

fn device_secret(android_id: &str, disk: &Disk) -> [u8; 32] {
    let id = android_id.trim();
    if !id.is_empty() {
        return derive_device_secret(id);
    }
    if let Ok(bytes) = hex::decode(disk.secret_hex.trim()) {
        if let Ok(secret) = <[u8; 32]>::try_from(bytes) {
            return secret;
        }
    }
    pjdesk_proto::random_bytes()
}

fn load_disk(dir: &Path) -> Disk {
    let path = dir.join("pjdesk.json");
    std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_disk(dir: &Path, disk: &Disk) {
    let path = dir.join("pjdesk.json");
    if let Ok(bytes) = serde_json::to_vec_pretty(disk) {
        let tmp = dir.join("pjdesk.json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

fn persist(state: &State) {
    let disk = Disk {
        server: state.server.lock().unwrap().clone(),
        fingerprint: state.fingerprint.lock().unwrap().clone().unwrap_or_default(),
        device_id: state.device_id.lock().unwrap().clone().unwrap_or_default(),
        secret_hex: String::new(),
    };
    // Keep a derived secret out of the file. A random one (no Android id) is rewritten
    // only when it was already stored.
    let mut disk = disk;
    let existing = load_disk(&state.dir);
    if !existing.secret_hex.is_empty() {
        disk.secret_hex = existing.secret_hex;
    }
    save_disk(&state.dir, &disk);
}

async fn run(state: Arc<State>, mut cmds: mpsc::UnboundedReceiver<Value>, cb: Arc<dyn Callbacks>) {
    publish(&state, &cb);
    let state_h = state.clone();
    let cb_h = cb.clone();
    let host = tokio::spawn(async move { host_loop(state_h, cb_h).await });
    while let Some(cmd) = cmds.recv().await {
        if state.app_stop.load(Ordering::SeqCst) {
            break;
        }
        apply(&state, &cb, cmd).await;
    }
    state.app_stop.store(true, Ordering::SeqCst);
    host.abort();
}

fn publish(state: &State, cb: &Arc<dyn Callbacks>) {
    let id = state.device_id.lock().unwrap().clone().unwrap_or_default();
    let password = state.otp.lock().unwrap().clone();
    let fp = state.fingerprint.lock().unwrap().clone().unwrap_or_default();
    let server = state.server.lock().unwrap().clone();
    let permanent = state.permanent.lock().unwrap().is_some();
    emit(
        cb,
        &json!({
            "event": "registered",
            "id": id,
            "idFormatted": if id.is_empty() { String::new() } else { format_id(&id) },
            "password": password,
            "permanent": permanent,
            "fingerprint": fp,
            "server": server,
            "approval": state.approval.lock().unwrap().clone(),
        }),
    );
}

fn emit(cb: &Arc<dyn Callbacks>, value: &Value) {
    cb.on_event(&value.to_string());
}

async fn apply(state: &Arc<State>, cb: &Arc<dyn Callbacks>, cmd: Value) {
    let name = cmd.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "setPermanent" => {
            let pw = cmd.get("password").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if pw.is_empty() {
                *state.permanent.lock().unwrap() = None;
                publish(state, cb);
            } else if permanent_ok(&pw) {
                *state.permanent.lock().unwrap() = Some(pw);
                publish(state, cb);
            } else {
                emit(cb, &json!({"event":"error","text":"Use at least 8 characters for a permanent password."}));
            }
        }
        "setApproval" => {
            let mode = cmd.get("mode").and_then(|v| v.as_str()).unwrap_or("prompt");
            *state.approval.lock().unwrap() = if mode == "unattended" { "unattended".into() } else { "prompt".into() };
            publish(state, cb);
        }
        "regenerate" => {
            *state.otp.lock().unwrap() = generate_password();
            publish(state, cb);
        }
        "server" => {
            let address = cmd.get("address").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if ServerAddr::parse(&address).is_err() {
                emit(cb, &json!({"event":"error","text":"That server address is not host or host:port."}));
            } else {
                *state.server.lock().unwrap() = address;
                persist(state);
                publish(state, cb);
                emit(
                    cb,
                    &json!({"event":"status","text":"Server saved. Reconnect by leaving and opening PJDesk if the old server is still up."}),
                );
            }
        }
        "resetFingerprint" => {
            *state.fingerprint.lock().unwrap() = None;
            persist(state);
            publish(state, cb);
        }
        "capture" => {
            let w = cmd.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let h = cmd.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            if w > 0 && h > 0 {
                *state.capture.lock().unwrap() = (w, h);
            }
        }
        "connect" => {
            let id = normalize_id(cmd.get("id").and_then(|v| v.as_str()).unwrap_or(""));
            let password = cmd.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !is_valid_id(&id) {
                emit(cb, &json!({"event":"error","text":"That isn't a 9-digit PJDesk ID."}));
                return;
            }
            if password.is_empty() {
                emit(cb, &json!({"event":"error","text":"Enter the password from the other computer."}));
                return;
            }
            state.gen.fetch_add(1, Ordering::SeqCst);
            let gen = state.gen.load(Ordering::SeqCst);
            let (state, cb) = (state.clone(), cb.clone());
            tokio::spawn(async move {
                if let Err(e) = controller_session(state.clone(), cb.clone(), &id, &password, gen).await {
                    emit(&cb, &json!({"event":"error","text": format!("{e:#}")}));
                    emit(&cb, &json!({"event":"disconnected","role":"controller","reason": format!("{e:#}")}));
                }
            });
        }
        "accept" => {
            if let Some(tx) = state.approval_tx.lock().unwrap().take() {
                let _ = tx.send(true);
            }
        }
        "decline" => {
            if let Some(tx) = state.approval_tx.lock().unwrap().take() {
                let _ = tx.send(false);
            }
        }
        "stop" => {
            state.gen.fetch_add(1, Ordering::SeqCst);
            *state.control.lock().unwrap() = None;
            *state.video.lock().unwrap() = None;
            emit(cb, &json!({"event":"disconnected","role":"session","reason":"stopped"}));
        }
        "text" => {
            let chars = cmd.get("chars").and_then(|v| v.as_str()).unwrap_or("");
            send_control(state, ControlMsg::Text { chars: chars.to_string() });
        }
        "special" => {
            let which = cmd.get("which").and_then(|v| v.as_u64()).unwrap_or(0);
            if let Some(cmd) = special_from(which) {
                send_control(state, ControlMsg::Special(cmd));
            }
        }
        "gesture" => {
            if let Some(event) = gesture_from(&cmd) {
                send_control(state, ControlMsg::Input(event));
            }
        }
        "mouse" => {
            if let Some(event) = mouse_from(&cmd) {
                send_control(state, ControlMsg::Input(event));
            }
        }
        "key" => {
            let scan = cmd.get("scan").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let down = cmd.get("down").and_then(|v| v.as_bool()).unwrap_or(false);
            send_control(state, ControlMsg::Input(InputEvent::Key { scan, down }));
        }
        "release" => send_control(state, ControlMsg::Input(InputEvent::ReleaseAll)),
        "quality" => {
            let name = cmd.get("name").and_then(|v| v.as_str()).unwrap_or("balanced");
            if let Some(q) = quality::preset(name) {
                send_control(state, ControlMsg::SetQuality(q));
            }
        }
        "display" => {
            let index = cmd.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
            let layout = cmd.get("layout").and_then(|v| v.as_bool()).unwrap_or(false);
            if layout {
                send_control(state, ControlMsg::View { mode: VIEW_SINGLE, display: index });
            } else {
                send_control(state, ControlMsg::SelectMonitor { index });
            }
        }
        "keyframe" => send_control(state, ControlMsg::RequestKeyframe),
        "shutdown" => state.app_stop.store(true, Ordering::SeqCst),
        _ => {}
    }
}

fn send_control(state: &State, msg: ControlMsg) {
    if let Some(tx) = state.control.lock().unwrap().as_ref() {
        let _ = tx.send(msg);
    }
}

fn special_from(which: u64) -> Option<SpecialCommand> {
    Some(match which {
        0 => SpecialCommand::CtrlAltDel,
        1 => SpecialCommand::Lock,
        2 => SpecialCommand::TaskManager,
        3 => SpecialCommand::Back,
        4 => SpecialCommand::Home,
        5 => SpecialCommand::Recents,
        6 => SpecialCommand::VolumeUp,
        7 => SpecialCommand::VolumeDown,
        8 => SpecialCommand::Power,
        _ => return None,
    })
}

fn gesture_from(cmd: &Value) -> Option<InputEvent> {
    let kind = cmd.get("kind").and_then(|v| v.as_u64())? as u8;
    let duration_ms = cmd.get("duration").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
    let mut x = [0u16; 4];
    let mut y = [0u16; 4];
    for (i, slot) in x.iter_mut().enumerate() {
        *slot = cmd.get("x").and_then(|v| v.get(i)).and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    }
    for (i, slot) in y.iter_mut().enumerate() {
        *slot = cmd.get("y").and_then(|v| v.get(i)).and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    }
    if !matches!(kind, GESTURE_TAP | GESTURE_LONG_PRESS | GESTURE_SWIPE | GESTURE_PINCH) {
        return None;
    }
    Some(InputEvent::Gesture { kind, duration_ms, x, y })
}

fn mouse_from(cmd: &Value) -> Option<InputEvent> {
    let kind = cmd.get("kind").and_then(|v| v.as_str())?;
    let x = cmd.get("x").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let y = cmd.get("y").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    Some(match kind {
        "move" => InputEvent::MouseMove { x, y },
        "down" => InputEvent::MouseButton { button: pjdesk_proto::peer::MouseButton::Left, down: true },
        "up" => InputEvent::MouseButton { button: pjdesk_proto::peer::MouseButton::Left, down: false },
        "wheel" => {
            let dy = cmd.get("dy").and_then(|v| v.as_i64()).unwrap_or(0).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
            InputEvent::Wheel { dx: 0, dy }
        }
        _ => return None,
    })
}

async fn host_loop(state: Arc<State>, cb: Arc<dyn Callbacks>) {
    let mut wait = Duration::from_secs(1);
    while !state.app_stop.load(Ordering::SeqCst) {
        match host_once(state.clone(), cb.clone()).await {
            Ok(()) => wait = Duration::from_secs(1),
            Err(e) => {
                warn!("host signaling: {e:#}");
                emit(&cb, &json!({"event":"status","text": format!("{e:#}")}));
                wait = (wait * 2).min(Duration::from_secs(20));
            }
        }
        tokio::time::sleep(wait).await;
    }
}

async fn host_once(state: Arc<State>, cb: Arc<dyn Callbacks>) -> Result<()> {
    let server_text = state.server.lock().unwrap().clone();
    let server = ServerAddr::parse(&server_text)?;
    let pinned = state.fingerprint.lock().unwrap().clone().and_then(|s| pjdesk_proto::tls::parse_fingerprint(&s).ok());
    let mut signal = SignalClient::connect(&server, pinned).await?;
    if let Some(fp) = signal.observed {
        let text = pjdesk_proto::tls::format_fingerprint(&fp);
        *state.fingerprint.lock().unwrap() = Some(text);
        persist(&state);
        publish(&state, &cb);
    }
    let device_id = state.device_id.lock().unwrap().clone().filter(|id| is_valid_id(id));
    signal.send(ClientMsg::Register {
        proto: PROTOCOL_VERSION,
        device_id,
        device_secret: state.secret,
        hostname: state.hostname.clone(),
        app_version: APP_VERSION.into(),
    })?;
    let server_ip = signal.server_addr.ip();
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    ping.tick().await;
    let mut routes: HashMap<u64, mpsc::UnboundedSender<ServerMsg>> = HashMap::new();
    loop {
        if state.app_stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        tokio::select! {
            _ = ping.tick() => { signal.send(ClientMsg::Ping)?; }
            msg = signal.recv() => {
                let Some(msg) = msg else { return Ok(()) };
                match msg {
                    ServerMsg::Registered { device_id } => {
                        *state.device_id.lock().unwrap() = Some(device_id);
                        persist(&state);
                        publish(&state, &cb);
                        emit(&cb, &json!({"event":"status","text":"Online"}));
                    }
                    ServerMsg::Policy(policy) => {
                        *state.policy.lock().unwrap() = policy;
                    }
                    ServerMsg::IncomingSession { session_id, token, udp_port, controller_name, controller_ip } => {
                        let approved = match approve(&state, &cb, &controller_name, &controller_ip).await {
                            Some(bits) => bits,
                            None => {
                                let _ = signal.send(ClientMsg::SessionResponse {
                                    session_id,
                                    accept: false,
                                    reason: "the person at this phone declined".into(),
                                });
                                continue;
                            }
                        };
                        signal.send(ClientMsg::SessionResponse { session_id, accept: true, reason: String::new() })?;
                        let (tx, rx) = mpsc::unbounded_channel();
                        let server_udp = SocketAddr::new(server_ip, udp_port);
                        let (state, cb, signal_tx) = (state.clone(), cb.clone(), signal.sender());
                        routes.insert(session_id, tx);
                        tokio::spawn(async move {
                            if let Err(e) = host_session(state, cb.clone(), signal_tx, rx, session_id, token, server_udp, approved).await {
                                warn!("android host session: {e:#}");
                                emit(&cb, &json!({"event":"disconnected","role":"host","reason": format!("{e:#}")}));
                            }
                        });
                    }
                    ServerMsg::PeerInfo { session_id, public_addr, local_addrs, cert_fingerprint } => {
                        let msg = ServerMsg::PeerInfo { session_id, public_addr, local_addrs, cert_fingerprint };
                        let failed = routes.get(&session_id).is_some_and(|tx| tx.send(msg).is_err());
                        if failed {
                            routes.remove(&session_id);
                        }
                    }
                    ServerMsg::Error { message, .. } => {
                        emit(&cb, &json!({"event":"status","text": message}));
                    }
                    ServerMsg::Pong | ServerMsg::UpdateNow => {}
                    _ => {}
                }
            }
        }
    }
}

async fn approve(state: &State, cb: &Arc<dyn Callbacks>, name: &str, ip: &str) -> Option<u32> {
    let policy = state.policy.lock().unwrap().clone();
    let permanent = state.permanent.lock().unwrap().is_some();
    let unattended_mode = state.approval.lock().unwrap().as_str() == "unattended";
    let prompt = !unattended_mode || policy.force_prompt;
    let restricted = grant(perms::UNATTENDED_DEFAULT, policy.max_permissions, false);
    let unattended_ok = permanent && policy.unattended_allowed && !policy.force_prompt;
    if !prompt {
        return unattended_ok.then_some(restricted);
    }
    let (tx, rx) = oneshot::channel();
    *state.approval_tx.lock().unwrap() = Some(tx);
    emit(
        cb,
        &json!({
            "event": "incoming",
            "name": name,
            "ip": ip,
        }),
    );
    let answer = tokio::time::timeout(Duration::from_secs(45), rx).await;
    *state.approval_tx.lock().unwrap() = None;
    match answer {
        Ok(Ok(true)) => Some(grant(perms::KEYBOARD | perms::MOUSE, policy.max_permissions, false)),
        Ok(Ok(false)) => None,
        _ => unattended_ok.then_some(restricted),
    }
}

#[allow(clippy::too_many_arguments)]
async fn host_session(
    state: Arc<State>,
    cb: Arc<dyn Callbacks>,
    signal: mpsc::UnboundedSender<ClientMsg>,
    mut peer_info: mpsc::UnboundedReceiver<ServerMsg>,
    session_id: u64,
    token: [u8; 16],
    server_udp: SocketAddr,
    ceiling: u32,
) -> Result<()> {
    let gen = state.gen.load(Ordering::SeqCst);
    let binding = p2p::bind_udp(server_udp, session_id, token, Role::Host).await?;
    let endpoint = p2p::host_endpoint(binding.socket, &state.identity)?;
    signal.send(ClientMsg::UdpReady {
        session_id,
        local_addrs: binding.local_addrs.clone(),
        cert_fingerprint: Some(state.identity.fingerprint()),
    })?;
    let (peer_public, peer_local) = match tokio::time::timeout(Duration::from_secs(15), peer_info.recv()).await {
        Ok(Some(ServerMsg::PeerInfo { public_addr, local_addrs, .. })) => (public_addr, local_addrs),
        _ => bail!("the controller never finished setting up the connection"),
    };
    let targets = p2p::candidates(peer_public, &peer_local);
    let punching = p2p::spawn_punching(binding.punch_socket, targets, session_id, Duration::from_secs(8));
    let passwords = auth_passwords(&state);
    let accepted = accept_authenticated(&endpoint, &passwords, &state.hostname).await;
    punching.abort();
    let (connection, auth) = accepted?;
    if state.gen.load(Ordering::SeqCst) != gen {
        connection.close(0u32.into(), b"stopped");
        bail!("stopped");
    }
    emit(&cb, &json!({"event":"hosting","peer": auth.peer_name}));
    let video_out = Arc::new(VideoOut {
        latest: Mutex::new(None),
        notify: Notify::new(),
        sent: AtomicU32::new(0),
        acked: AtomicU32::new(0),
        seq: AtomicU32::new(0),
    });
    *state.video.lock().unwrap() = Some(video_out.clone());
    let mut video = connection.open_uni().await?;
    let _ = video.set_priority(-1);
    let send_video = video_out.clone();
    let video_task = tokio::spawn(async move {
        loop {
            if send_video.sent.load(Ordering::SeqCst).wrapping_sub(send_video.acked.load(Ordering::SeqCst))
                >= MAX_IN_FLIGHT
            {
                send_video.notify.notified().await;
                continue;
            }
            let frame = send_video.latest.lock().unwrap().take();
            let Some(frame) = frame else {
                send_video.notify.notified().await;
                continue;
            };
            let seq = send_video.seq.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
            let header = VideoHeader {
                seq,
                keyframe: frame.keyframe,
                width: frame.width,
                height: frame.height,
                encode_us: frame.encode_us,
            };
            if write_video_frame(&mut video, &header, &frame.data).await.is_err() {
                break;
            }
            send_video.sent.store(seq, Ordering::SeqCst);
        }
    });

    let (mut control_send, mut control_recv) = (auth.send, auth.recv);
    let mut adapter = PointerAdapter::default();
    let mut greeted = auth.peer_proto < 3;
    if greeted {
        let _ = send_welcome(&state, &mut control_send, perms::CLASSIC, ceiling, false).await;
        emit(&cb, &json!({"event":"keyframe"}));
    }
    let result = loop {
        if state.gen.load(Ordering::SeqCst) != gen || state.app_stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        let msg = tokio::time::timeout(
            Duration::from_secs(20),
            read_msg::<_, ControlMsg>(&mut control_recv, MAX_CONTROL_FRAME),
        )
        .await;
        let msg = match msg {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => break Ok(()),
            Err(_) => {
                // Idle phones still send frames. A quiet control stream is fine
                // as long as the QUIC connection is up.
                if connection.close_reason().is_some() {
                    break Ok(());
                }
                continue;
            }
        };
        match msg {
            ControlMsg::Input(event) => forward_input(&cb, &mut adapter, event),
            ControlMsg::Text { chars } => emit(&cb, &json!({"event":"input","kind":"text","chars": chars})),
            ControlMsg::Special(cmd) => {
                emit(&cb, &json!({"event":"input","kind":"special","which": special_index(cmd)}))
            }
            ControlMsg::FrameAck { seq } => {
                video_out.acked.store(seq, Ordering::SeqCst);
                video_out.notify.notify_one();
            }
            ControlMsg::RequestKeyframe => emit(&cb, &json!({"event":"keyframe"})),
            ControlMsg::Hello { permissions, quality, .. } => {
                let _ = send_welcome(&state, &mut control_send, permissions, ceiling, true).await;
                emit(
                    &cb,
                    &json!({
                        "event":"quality",
                        "fps": quality.fps,
                        "bitrate": quality.bitrate_kbps,
                        "maxWidth": quality.max_width,
                    }),
                );
                if !greeted {
                    greeted = true;
                    emit(&cb, &json!({"event":"keyframe"}));
                }
            }
            ControlMsg::SetQuality(q) => {
                emit(&cb, &json!({"event":"quality","fps": q.fps,"bitrate": q.bitrate_kbps,"maxWidth": q.max_width}));
                emit(&cb, &json!({"event":"keyframe"}));
            }
            ControlMsg::Ping { t_us } => {
                let _ = write_msg(&mut control_send, &HostMsg::Pong { t_us }).await;
            }
            ControlMsg::SelectMonitor { .. } | ControlMsg::View { .. } => {
                emit(&cb, &json!({"event":"keyframe"}));
            }
            ControlMsg::SetKeyboardMode(_) => {}
            _ => {
                let _ = write_msg(
                    &mut control_send,
                    &HostMsg::Denied { what: "request".into(), reason: "this Android host does not do that".into() },
                )
                .await;
            }
        }
    };
    video_task.abort();
    *state.video.lock().unwrap() = None;
    connection.close(0u32.into(), b"session ended");
    emit(&cb, &json!({"event":"disconnected","role":"host","reason":"session ended"}));
    result
}

fn auth_passwords(state: &State) -> Vec<String> {
    let mut pws = vec![state.otp.lock().unwrap().clone()];
    if let Some(pw) = state.permanent.lock().unwrap().clone() {
        pws.push(pw);
    }
    pws
}

async fn send_welcome(
    state: &State,
    send: &mut quinn::SendStream,
    requested: u32,
    ceiling: u32,
    v3: bool,
) -> Result<()> {
    if !v3 {
        return Ok(());
    }
    let granted = grant(requested, ceiling & (perms::KEYBOARD | perms::MOUSE), false);
    let (w, h) = *state.capture.lock().unwrap();
    let monitors = vec![MonitorInfo { index: 0, width: w, height: h, primary: true, name: "Screen".into() }];
    write_msg(send, &HostMsg::Welcome { permissions: granted, monitors, proto: PROTOCOL_VERSION }).await?;
    if requested & perms::LAYOUT != 0 {
        let displays = vec![DisplayLayout {
            index: 0,
            name: "Screen".into(),
            width: w,
            height: h,
            x: 0,
            y: 0,
            primary: true,
            dpi: 0,
        }];
        write_msg(send, &HostMsg::Displays { displays }).await?;
        write_msg(send, &HostMsg::Platform { family: "android".into(), version: android_version() }).await?;
    }
    Ok(())
}

fn android_version() -> String {
    std::env::var("PJDESK_ANDROID_VERSION").unwrap_or_default().chars().take(64).collect()
}

fn forward_input(cb: &Arc<dyn Callbacks>, adapter: &mut PointerAdapter, event: InputEvent) {
    match event {
        InputEvent::Gesture { kind, duration_ms, x, y } => {
            emit(cb, &json!({"event":"input","kind":"gesture","gesture": kind,"duration": duration_ms,"x": x,"y": y}));
        }
        InputEvent::MouseMove { x, y } => adapter.mouse_move(x, y),
        InputEvent::MouseButton { button: pjdesk_proto::peer::MouseButton::Left, down } => {
            let now = now_ms();
            if let Some(g) = adapter.mouse_button(down, now) {
                emit_gesture(cb, g);
            } else if down {
                // A long-press is noticed on a later move or when the button lifts.
            }
        }
        InputEvent::MouseButton { .. } => {}
        InputEvent::Wheel { dy, .. } => {
            if let Some(g) = adapter.wheel(dy) {
                emit_gesture(cb, g);
            }
        }
        InputEvent::ReleaseAll => adapter.tracker.cancel(),
        InputEvent::Key { .. } => {}
    }
    if let Some(g) = adapter.poll(now_ms()) {
        emit_gesture(cb, g);
    }
}

fn emit_gesture(cb: &Arc<dyn Callbacks>, g: Gesture) {
    emit(
        cb,
        &json!({
            "event":"input",
            "kind":"gesture",
            "gesture": g.kind,
            "duration": g.duration_ms,
            "x": g.x,
            "y": g.y,
        }),
    );
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn special_index(cmd: SpecialCommand) -> u8 {
    match cmd {
        SpecialCommand::CtrlAltDel => 0,
        SpecialCommand::Lock => 1,
        SpecialCommand::TaskManager => 2,
        SpecialCommand::Back => 3,
        SpecialCommand::Home => 4,
        SpecialCommand::Recents => 5,
        SpecialCommand::VolumeUp => 6,
        SpecialCommand::VolumeDown => 7,
        SpecialCommand::Power => 8,
    }
}

async fn accept_authenticated(
    endpoint: &quinn::Endpoint,
    passwords: &[String],
    my_name: &str,
) -> Result<(quinn::Connection, session::Authenticated)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let attempt_used = Arc::new(AtomicBool::new(false));
    let mut pending: JoinSet<Result<(quinn::Connection, session::Authenticated)>> = JoinSet::new();
    let mut accepted = 0usize;
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { bail!("the QUIC endpoint closed") };
                accepted += 1;
                if accepted > 4 {
                    incoming.refuse();
                    continue;
                }
                let (passwords, my_name, attempt_used) = (passwords.to_vec(), my_name.to_owned(), attempt_used.clone());
                pending.spawn(async move {
                    let connection = incoming.await?;
                    let auth = session::host_auth(&connection, &passwords, &my_name, &attempt_used).await?;
                    Ok((connection, auth))
                });
            }
            Some(joined) = pending.join_next(), if !pending.is_empty() => match joined {
                Ok(Ok(done)) => return Ok(done),
                Ok(Err(e)) if e.is::<WrongPassword>() => return Err(e),
                Ok(Err(e)) => info!("a connection attempt failed: {e:#}"),
                Err(_) => {}
            },
            _ = tokio::time::sleep_until(deadline) => bail!("the controller did not connect in time"),
        }
    }
}

async fn controller_session(
    state: Arc<State>,
    cb: Arc<dyn Callbacks>,
    target: &str,
    password: &str,
    gen: u64,
) -> Result<()> {
    emit(&cb, &json!({"event":"status","text": format!("Connecting to {}", format_id(target))}));
    let server = ServerAddr::parse(&state.server.lock().unwrap())?;
    let pinned = state.fingerprint.lock().unwrap().clone().and_then(|s| pjdesk_proto::tls::parse_fingerprint(&s).ok());
    let mut signal = SignalClient::connect(&server, pinned).await?;
    if let Some(fp) = signal.observed {
        *state.fingerprint.lock().unwrap() = Some(pjdesk_proto::tls::format_fingerprint(&fp));
        persist(&state);
    }
    signal.send(ClientMsg::Connect {
        proto: PROTOCOL_VERSION,
        target_id: target.into(),
        controller_name: state.hostname.clone(),
    })?;
    let (session_id, token, udp_port, host_name) = loop {
        if state.gen.load(Ordering::SeqCst) != gen {
            bail!("stopped");
        }
        match tokio::time::timeout(Duration::from_secs(30), signal.recv()).await {
            Ok(Some(ServerMsg::SessionReady { session_id, token, udp_port, host_name })) => {
                break (session_id, token, udp_port, host_name)
            }
            Ok(Some(ServerMsg::Error { message, .. })) => bail!("{message}"),
            Ok(Some(_)) => continue,
            Ok(None) => bail!("the server closed the connection"),
            Err(_) => bail!("the host did not respond"),
        }
    };
    let server_udp = SocketAddr::new(signal.server_addr.ip(), udp_port);
    let binding = p2p::bind_udp(server_udp, session_id, token, Role::Controller).await?;
    signal.send(ClientMsg::UdpReady {
        session_id,
        local_addrs: binding.local_addrs.clone(),
        cert_fingerprint: None,
    })?;
    let (peer_public, peer_local, fingerprint) = loop {
        match tokio::time::timeout(Duration::from_secs(20), signal.recv()).await {
            Ok(Some(ServerMsg::PeerInfo { public_addr, local_addrs, cert_fingerprint, .. })) => {
                break (public_addr, local_addrs, cert_fingerprint.context("the server didn't send the host's key")?)
            }
            Ok(Some(ServerMsg::Error { message, .. })) => bail!("{message}"),
            Ok(Some(_)) => continue,
            Ok(None) => bail!("the server closed the connection"),
            Err(_) => bail!("the host did not finish setting up the connection"),
        }
    };
    drop(signal);
    let endpoint = p2p::controller_endpoint(binding.socket, fingerprint)?;
    let candidates = p2p::candidates(peer_public, &peer_local);
    let punching = p2p::spawn_punching(binding.punch_socket, candidates.clone(), session_id, Duration::from_secs(5));
    let connected = p2p::connect_to_host(&endpoint, &candidates, server_udp, false).await;
    punching.abort();
    let (connection, path) = connected?;
    let auth = match session::controller_auth(&connection, password, &state.hostname).await {
        Ok(auth) => auth,
        Err(e) => {
            connection.close(1u32.into(), b"authentication failed");
            if e.is::<WrongPassword>() {
                bail!("wrong password for {}", format_id(target));
            }
            return Err(e);
        }
    };
    let peer = if auth.peer_name.is_empty() { host_name } else { auth.peer_name };
    let path = match path {
        LinkPath::Direct(_) => "direct",
        LinkPath::Relay(_) => "relay",
    };
    emit(
        &cb,
        &json!({
            "event":"controlling",
            "peer": peer,
            "id": target,
            "path": path,
            "platform": "",
            "version": "",
            "layout": false,
            "displays": [],
        }),
    );
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<ControlMsg>();
    *state.control.lock().unwrap() = Some(control_tx.clone());
    if auth.peer_proto >= 3 {
        let _ = control_tx.send(ControlMsg::Hello {
            permissions: perms::ALL | perms::LAYOUT,
            monitor: 0,
            quality: quality::preset("balanced").unwrap_or(StreamQuality::default()),
        });
        let _ = control_tx.send(ControlMsg::SetKeyboardMode(KeyboardMode::Translate));
    }
    let mut control_send = auth.send;
    let mut control_recv = auth.recv;
    let writer = tokio::spawn(async move {
        let mut buf = Vec::new();
        while let Some(msg) = control_rx.recv().await {
            buf.clear();
            if let Ok(body) = postcard::to_stdvec(&msg) {
                buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
                buf.extend_from_slice(&body);
            }
            if control_send.write_all(&buf).await.is_err() || control_send.flush().await.is_err() {
                break;
            }
        }
    });
    let conn = connection.clone();
    let cb_v = cb.clone();
    let tx_ack = control_tx.clone();
    let video_task = tokio::spawn(async move {
        let mut video = match tokio::time::timeout(Duration::from_secs(15), conn.accept_uni()).await {
            Ok(Ok(v)) => v,
            _ => return,
        };
        let mut data = Vec::new();
        loop {
            match read_video_frame(&mut video, &mut data).await {
                Ok(header) => {
                    cb_v.on_video(&data, header.width as u32, header.height as u32, header.keyframe);
                    let _ = tx_ack.send(ControlMsg::FrameAck { seq: header.seq });
                }
                Err(_) => break,
            }
        }
    });
    let mut platform = String::new();
    let mut layout = false;
    loop {
        if state.gen.load(Ordering::SeqCst) != gen || state.app_stop.load(Ordering::SeqCst) {
            break;
        }
        match read_msg::<_, HostMsg>(&mut control_recv, MAX_CONTROL_FRAME).await {
            Ok(HostMsg::Platform { family, version }) => {
                platform = family.clone();
                emit(
                    &cb,
                    &json!({
                        "event":"platform",
                        "id": target,
                        "peer": peer,
                        "platform": family,
                        "version": version,
                    }),
                );
            }
            Ok(HostMsg::Displays { displays }) => {
                layout = true;
                emit(&cb, &json!({"event":"displays","layout": true,"displays": displays}));
            }
            Ok(HostMsg::Welcome { monitors, permissions, .. }) => {
                emit(
                    &cb,
                    &json!({"event":"welcome","permissions": permissions,"monitors": monitors,"layout": layout,"platform": platform}),
                );
            }
            Ok(HostMsg::Notice(text)) => emit(&cb, &json!({"event":"status","text": text})),
            Ok(HostMsg::Denied { what, reason }) => {
                emit(&cb, &json!({"event":"status","text": format!("{what}: {reason}")}))
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    writer.abort();
    video_task.abort();
    *state.control.lock().unwrap() = None;
    connection.close(0u32.into(), b"viewer closed");
    emit(&cb, &json!({"event":"disconnected","role":"controller","reason":"session ended"}));
    Ok(())
}

// quinn SendStream write_all is on AsyncWrite.
use tokio::io::AsyncWriteExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permanent_password_matches_the_windows_rule() {
        assert!(!permanent_ok("short"));
        assert!(!permanent_ok("1234567"));
        assert!(permanent_ok("12345678"));
        assert!(permanent_ok("a longer phrase"));
        assert!(!permanent_ok("        "));
    }

    #[test]
    fn gesture_command_uses_the_appended_input_variant() {
        let cmd = json!({"cmd":"gesture","kind": GESTURE_SWIPE,"duration": 180,"x":[1,2,0,0],"y":[3,4,0,0]});
        let event = gesture_from(&cmd).unwrap();
        let bytes = postcard::to_stdvec(&event).unwrap();
        assert_eq!(bytes[0], 5);
        assert!(matches!(event, InputEvent::Gesture { kind: GESTURE_SWIPE, .. }));
    }

    #[test]
    fn specials_keep_the_windows_indexes_and_append_android() {
        assert_eq!(special_index(SpecialCommand::CtrlAltDel), 0);
        assert_eq!(special_index(SpecialCommand::Back), 3);
        assert_eq!(special_index(SpecialCommand::Power), 8);
        assert!(matches!(special_from(4), Some(SpecialCommand::Home)));
    }

    #[test]
    fn disk_round_trip_ignores_unknown_future_fields() {
        let raw = r#"{"server":"pjdesk.spader.us:21200","fingerprint":"abcd","device_id":"123456789","extra":1}"#;
        let disk: Disk = serde_json::from_str(raw).unwrap();
        assert_eq!(disk.device_id, "123456789");
        assert!(disk.secret_hex.is_empty());
    }
}
