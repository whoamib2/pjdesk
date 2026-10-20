//! Host registration and controller sessions for the Android app.
//!
//! Video bytes stay in Kotlin (MediaCodec). This task owns the sockets.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use pjdesk_link::admit::{self, Admit};
use pjdesk_link::lockout::{self, Lockout};
use pjdesk_link::p2p::{self, LinkPath};
use pjdesk_link::session::{self, grant_auth, refuse_auth, refuse_before_password, LockedOut, NeedsApproval, WrongPassword};
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
use pjdesk_proto::{derive_device_secret, format_id, is_valid_id, normalize_id, LEGACY_REGISTERED_SERVER, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Shown in Register and on the About screen. Independent of the Windows crate version.
pub const APP_VERSION: &str = "0.6.9";

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

#[derive(Clone, Default)]
struct HostFacts {
    hostname: String,
    os_family: String,
    os_version: String,
    console_user: String,
    display_name: String,
}

struct State {
    dir: PathBuf,
    hostname: String,
    /// OS and the Android profile name, filled by the app before registration.
    facts: Mutex<HostFacts>,
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
    /// True while MediaProjection is producing frames.
    capturing: AtomicBool,
    /// Set when the person denies screen sharing, or the consent fails.
    capture_deny: Mutex<String>,
    /// Bumped to end the current session. The host registration keeps running.
    gen: AtomicU64,
    approval_tx: Mutex<Option<oneshot::Sender<Result<(), String>>>>,
    password_tx: Mutex<Option<oneshot::Sender<TypedAuth>>>,
    lockout: Mutex<Lockout>,
    end_reason: Mutex<String>,
    control: Mutex<Option<mpsc::UnboundedSender<ControlMsg>>>,
    video: Mutex<Option<Arc<VideoOut>>>,
    app_stop: AtomicBool,
    access_token: Mutex<String>,
    /// pjdesk.com account email while this phone is signed in. Empty otherwise.
    account: Mutex<String>,
    trusted_accounts: Mutex<Vec<String>>,
    trusted_computers: Mutex<Vec<String>>,
    view_ask: Mutex<ViewAsk>,
    /// Set when a server from before 0.6.0 drops the connection after PresentAccess.
    suppress_token: AtomicBool,
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
    #[serde(default)]
    access_token: String,
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
        let server = if !disk.server.trim().is_empty() {
            disk.server.clone()
        } else if !disk.device_id.trim().is_empty() {
            // Already registered before 0.6.0. Keep it online. Do not write the name back.
            LEGACY_REGISTERED_SERVER.to_string()
        } else {
            String::new()
        };
        let identity = TlsIdentity::load_or_create(&dir.join("host.crt"), &dir.join("host.key"), "pjdesk-host")?;
        let mut stored = disk;
        if android_id.trim().is_empty() && stored.secret_hex.is_empty() {
            stored.secret_hex = hex::encode(secret);
            save_disk(&dir, &stored);
        }
        let state = Arc::new(State {
            dir,
            hostname: if hostname.trim().is_empty() { "Android".into() } else { hostname },
            facts: Mutex::new(HostFacts::default()),
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
            capturing: AtomicBool::new(false),
            capture_deny: Mutex::new(String::new()),
            gen: AtomicU64::new(1),
            approval_tx: Mutex::new(None),
            password_tx: Mutex::new(None),
            lockout: Mutex::new(Lockout::default()),
            end_reason: Mutex::new(String::new()),
            control: Mutex::new(None),
            video: Mutex::new(None),
            app_stop: AtomicBool::new(false),
            access_token: Mutex::new(stored.access_token.clone()),
            account: Mutex::new(String::new()),
            trusted_accounts: Mutex::new(Vec::new()),
            trusted_computers: Mutex::new(Vec::new()),
            view_ask: Mutex::new(ViewAsk::default()),
            suppress_token: AtomicBool::new(false),
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
        access_token: state.access_token.lock().unwrap().clone(),
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

fn log_line(cb: &Arc<dyn Callbacks>, text: impl AsRef<str>) {
    let text = text.as_ref();
    info!("{text}");
    emit(cb, &json!({"event":"log","text": text}));
}

async fn apply(state: &Arc<State>, cb: &Arc<dyn Callbacks>, cmd: Value) {
    let name = cmd.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "setPermanent" => {
            let pw = cmd.get("password").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if pw.is_empty() {
                *state.permanent.lock().unwrap() = None;
                *state.approval.lock().unwrap() = "prompt".into();
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
            let want = mode == "unattended";
            let allowed = want && state.permanent.lock().unwrap().is_some();
            if want && !allowed {
                *state.approval.lock().unwrap() = "prompt".into();
                publish(state, cb);
                emit(cb, &json!({"event":"error","text":"Save a permanent password first."}));
            } else {
                *state.approval.lock().unwrap() = if allowed { "unattended".into() } else { "prompt".into() };
                publish(state, cb);
            }
        }
        "regenerate" => {
            *state.otp.lock().unwrap() = generate_password();
            publish(state, cb);
        }
        "hostFacts" => {
            let mut facts = state.facts.lock().unwrap();
            if let Some(host) = cmd.get("hostname").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                facts.hostname = host.chars().take(64).collect();
            }
            if let Some(family) = cmd.get("os_family").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                facts.os_family = family.chars().take(32).collect();
            }
            if let Some(version) = cmd.get("os_version").and_then(|v| v.as_str()).map(str::trim) {
                facts.os_version = version.chars().take(40).collect();
            }
            if let Some(user) = cmd.get("console_user").and_then(|v| v.as_str()).map(str::trim) {
                facts.console_user = user.chars().filter(|c| !c.is_control()).take(80).collect();
            }
            if cmd.get("display_name").is_some() {
                let name = cmd.get("display_name").and_then(|v| v.as_str()).unwrap_or("");
                facts.display_name = name.chars().filter(|c| !c.is_control()).take(64).collect();
            }
        }
        "accessToken" => {
            let token = cmd.get("token").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            *state.access_token.lock().unwrap() = token;
            state.suppress_token.store(false, Ordering::SeqCst);
            persist(state);
        }
        "account" => {
            let email = cmd.get("email").and_then(|v| v.as_str()).unwrap_or("").trim();
            *state.account.lock().unwrap() = email.chars().filter(|c| !c.is_control()).take(80).collect();
        }
        "setTrusted" => {
            *state.trusted_accounts.lock().unwrap() = trusted_list(cmd.get("accounts"));
            *state.trusted_computers.lock().unwrap() = trusted_list(cmd.get("computers"));
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
        "capturing" => {
            let on = cmd.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
            state.capturing.store(on, Ordering::SeqCst);
            if on {
                *state.capture_deny.lock().unwrap() = String::new();
            }
        }
        "captureDenied" => {
            let reason = cmd.get("reason").and_then(|v| v.as_str()).unwrap_or("Screen capture permission denied");
            *state.capture_deny.lock().unwrap() = reason.chars().filter(|c| !c.is_control()).take(200).collect();
            state.capturing.store(false, Ordering::SeqCst);
        }
        "connect" => {
            let id = normalize_id(cmd.get("id").and_then(|v| v.as_str()).unwrap_or(""));
            let password = cmd.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !is_valid_id(&id) {
                emit(cb, &json!({"event":"error","text":"That isn't a 9-digit PJDesk ID."}));
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
        "submitPassword" => {
            let password = cmd.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(tx) = state.password_tx.lock().unwrap().take() {
                let _ = tx.send(TypedAuth::Password(password));
            }
        }
        "cancelPassword" => {
            if let Some(tx) = state.password_tx.lock().unwrap().take() {
                let _ = tx.send(TypedAuth::Cancel);
            }
        }
        "checkUpdate" => {
            let cb = cb.clone();
            tokio::spawn(async move {
                offer_android_update(&cb, false).await;
            });
        }
        "askPerson" => {
            if let Some(tx) = state.password_tx.lock().unwrap().take() {
                let _ = tx.send(TypedAuth::AskPerson);
            }
        }
        "accept" => {
            log_line(cb, "approval: allow");
            if let Some(tx) = state.approval_tx.lock().unwrap().take() {
                let _ = tx.send(Ok(()));
            }
        }
        "decline" => {
            let reason = cmd.get("reason").and_then(|v| v.as_str()).unwrap_or("the person at this phone declined");
            let reason = reason.chars().take(200).collect::<String>();
            log_line(cb, format!("approval: decline ({reason})"));
            if let Some(tx) = state.approval_tx.lock().unwrap().take() {
                let _ = tx.send(Err(reason));
            }
        }
        "stop" => {
            let reason = cmd.get("reason").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("stopped");
            let reason = reason.chars().take(200).collect::<String>();
            *state.end_reason.lock().unwrap() = reason.clone();
            state.gen.fetch_add(1, Ordering::SeqCst);
            *state.control.lock().unwrap() = None;
            *state.video.lock().unwrap() = None;
            emit(cb, &json!({"event":"disconnected","role":"session","reason": reason}));
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
    if server_text.trim().is_empty() {
        emit(&cb, &json!({"event":"status","text":"Sign in required"}));
        bail!("Sign in required");
    }
    let server = ServerAddr::parse(&server_text)?;
    let pinned = state.fingerprint.lock().unwrap().clone().and_then(|s| pjdesk_proto::tls::parse_fingerprint(&s).ok());
    let mut signal = SignalClient::connect(&server, pinned).await?;
    if let Some(fp) = signal.observed {
        let text = pjdesk_proto::tls::format_fingerprint(&fp);
        *state.fingerprint.lock().unwrap() = Some(text);
        persist(&state);
        publish(&state, &cb);
    }
    let token = if state.suppress_token.swap(false, Ordering::SeqCst) {
        String::new()
    } else {
        state.access_token.lock().unwrap().clone()
    };
    let presented = !token.is_empty();
    if presented {
        let _ = signal.send(ClientMsg::PresentAccess { token });
    }
    let device_id = state.device_id.lock().unwrap().clone().filter(|id| is_valid_id(id));
    signal.send(ClientMsg::Register {
        proto: PROTOCOL_VERSION,
        device_id,
        device_secret: state.secret,
        hostname: {
            let reported = state.facts.lock().unwrap().hostname.clone();
            if reported.trim().is_empty() { state.hostname.clone() } else { reported }
        },
        app_version: APP_VERSION.into(),
    })?;
    let server_ip = signal.server_addr.ip();
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    ping.tick().await;
    let mut routes: HashMap<u64, mpsc::UnboundedSender<ServerMsg>> = HashMap::new();
    let mut saw_registered = false;
    let mut report_host = false;
    let mut since_report = 0u32;
    loop {
        if state.app_stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        tokio::select! {
            _ = ping.tick() => {
                signal.send(ClientMsg::Ping)?;
                if report_host {
                    since_report += 1;
                    if since_report >= 5 {
                        since_report = 0;
                        let _ = signal.send(host_report(&state));
                        let _ = signal.send(display_name_report(&state));
                    }
                }
            }
            msg = signal.recv() => {
                let Some(msg) = msg else {
                    if presented && !saw_registered {
                        state.suppress_token.store(true, Ordering::SeqCst);
                    }
                    return Ok(());
                };
                match msg {
                    ServerMsg::Registered { device_id } => {
                        saw_registered = true;
                        *state.device_id.lock().unwrap() = Some(device_id.clone());
                        persist(&state);
                        publish(&state, &cb);
                        log_line(&cb, format!("registered as {}", format_id(&device_id)));
                        emit(&cb, &json!({"event":"status","text":"Ready"}));
                    }
                    ServerMsg::Policy(policy) => {
                        *state.policy.lock().unwrap() = policy;
                    }
                    ServerMsg::IncomingSession { session_id, token, udp_port, controller_name, controller_ip } => {
                        log_line(&cb, format!("incoming connection from {controller_name} ({controller_ip})"));
                        log_line(&cb, "incoming connection accepted, checking the password before any prompt");
                        signal.send(ClientMsg::SessionResponse { session_id, accept: true, reason: String::new() })?;
                        let (tx, rx) = mpsc::unbounded_channel();
                        let server_udp = SocketAddr::new(server_ip, udp_port);
                        let (state, cb, signal_tx) = (state.clone(), cb.clone(), signal.sender());
                        routes.insert(session_id, tx);
                        tokio::spawn(async move {
                            let ended = host_session(
                                state.clone(),
                                cb.clone(),
                                signal_tx,
                                rx,
                                session_id,
                                token,
                                server_udp,
                                controller_name,
                                controller_ip,
                            )
                            .await;
                            let reason = match &ended {
                                Ok(()) => {
                                    let saved = state.end_reason.lock().unwrap().clone();
                                    if saved.is_empty() {
                                        "the other computer closed the session".to_string()
                                    } else {
                                        saved
                                    }
                                }
                                Err(e) => format!("{e:#}"),
                            };
                            log_line(&cb, format!("host session ended: {reason}"));
                            emit(&cb, &json!({"event":"disconnected","role":"host","reason": reason}));
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
                    ServerMsg::WantHostReport => {
                        report_host = true;
                        since_report = 0;
                        let _ = signal.send(host_report(&state));
                        let _ = signal.send(display_name_report(&state));
                    }
                    ServerMsg::Pong => {}
                    ServerMsg::UpdateNow => {
                        log_line(&cb, "the server asked this phone to update");
                        let cb = cb.clone();
                        tokio::spawn(async move {
                            offer_android_update(&cb, true).await;
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn offer_android_update(cb: &Arc<dyn Callbacks>, from_server: bool) {
    if from_server {
        emit(cb, &json!({"event":"status","text":"The server asked this phone to update. Checking…"}));
    } else {
        emit(cb, &json!({"event":"status","text":"Checking for updates…"}));
    }
    let found = tokio::task::spawn_blocking(|| pjdesk_proto::update::check_android(pjdesk_proto::UPDATE_REPO, APP_VERSION)).await;
    match found {
        Ok(Ok(Some(update))) => {
            emit(
                cb,
                &json!({
                    "event": "openUrl",
                    "url": update.installer_url,
                    "text": format!("Downloading PJDesk {}", update.version),
                }),
            );
        }
        Ok(Ok(None)) => {
            emit(cb, &json!({"event":"notice","text": format!("PJDesk {APP_VERSION} is up to date.")}));
        }
        Ok(Err(e)) => {
            let message = format!("{e:#}");
            if message.contains("no Android") {
                emit(
                    cb,
                    &json!({
                        "event": "openUrl",
                        "url": "https://github.com/whoamib2/pjdesk/releases/latest",
                        "text": "Opening the latest release. No APK was attached.",
                    }),
                );
            } else {
                emit(cb, &json!({"event":"error","text": format!("Could not check for updates: {message}")}));
            }
        }
        Err(_) => emit(cb, &json!({"event":"error","text": "Could not check for updates."})),
    }
}

fn display_name_report(state: &State) -> ClientMsg {
    let name = state.facts.lock().unwrap().display_name.clone();
    ClientMsg::ReportDisplayName { name }
}

fn host_report(state: &State) -> ClientMsg {
    let facts = state.facts.lock().unwrap().clone();
    let hostname = if facts.hostname.trim().is_empty() { state.hostname.clone() } else { facts.hostname };
    ClientMsg::ReportHost {
        hostname,
        os_family: if facts.os_family.is_empty() { "android".into() } else { facts.os_family },
        os_version: facts.os_version,
        console_user: facts.console_user,
    }
}

enum Approval {
    Yes(u32),
    No(String),
}

#[derive(Clone, Debug, Default)]
struct ViewAsk {
    view: bool,
    account: String,
    computer: String,
    id: String,
}

fn trusted_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A view request skips the PJDesk prompt when this account or computer is
/// trusted, or when unattended mode is using the permanent password.
fn view_is_preapproved(state: &State, ask: &ViewAsk) -> bool {
    let approval = state.approval.lock().unwrap().clone();
    let permanent = state.permanent.lock().unwrap().clone().unwrap_or_default();
    if approval == "unattended" && !permanent.trim().is_empty() {
        return true;
    }
    let account = ask.account.trim();
    if !account.is_empty() && state.trusted_accounts.lock().unwrap().iter().any(|saved| saved.eq_ignore_ascii_case(account)) {
        return true;
    }
    let computer = ask.computer.trim();
    let id = ask.id.trim();
    state.trusted_computers.lock().unwrap().iter().any(|saved| {
        let saved = saved.trim();
        if saved.is_empty() {
            return false;
        }
        (!computer.is_empty() && saved.eq_ignore_ascii_case(computer)) || (!id.is_empty() && saved.chars().filter(|c| c.is_ascii_digit()).eq(id.chars()))
    })
}

enum TypedAuth {
    Password(String),
    AskPerson,
    Cancel,
}

const ASK_SLOT: u8 = 255;

async fn approve(state: &State, cb: &Arc<dyn Callbacks>, name: &str, ip: &str, view: bool, bypass: bool) -> Approval {
    let policy = state.policy.lock().unwrap().clone();
    let (tx, rx) = oneshot::channel();
    *state.approval_tx.lock().unwrap() = Some(tx);
    log_line(cb, format!("waiting for approval from {name}"));
    emit(
        cb,
        &json!({
            "event": "incoming",
            "name": name,
            "ip": ip,
            "view": view,
            "bypass": bypass,
        }),
    );
    // Long enough for Allow and for Android's own screen-share consent.
    let wait = if view { Duration::from_secs(90) } else { Duration::from_secs(45) };
    let answer = tokio::time::timeout(wait, rx).await;
    *state.approval_tx.lock().unwrap() = None;
    match answer {
        Ok(Ok(Ok(()))) => Approval::Yes(grant(perms::KEYBOARD | perms::MOUSE, policy.max_permissions, false)),
        Ok(Ok(Err(reason))) => Approval::No(reason),
        Ok(Err(_)) => Approval::No("the approval prompt closed".into()),
        Err(_) if view => Approval::No(format!("Timed out. No one at {} answered the request to view the screen", state.hostname)),
        Err(_) => Approval::No(format!("No one at {} answered the request", state.hostname)),
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
    controller_name: String,
    controller_ip: String,
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
    let permanent = state.permanent.lock().unwrap().clone();
    log_line(
        &cb,
        format!("checking the password ({} slot{})", passwords.len(), if passwords.len() == 1 { "" } else { "s" }),
    );
    let accepted = accept_authenticated(&state, &cb, &endpoint, &passwords, &state.hostname, server_udp.ip(), peer_public, &controller_ip, &controller_name).await;
    punching.abort();
    let (connection, mut auth, ask) = match accepted {
        Ok(done) => done,
        Err(e) if e.is::<WrongPassword>() => bail!("Wrong password"),
        Err(e) => return Err(e),
    };
    let source = lockout::source_key(
        connection.remote_address().ip(),
        Some(server_udp.ip()),
        Some(peer_public.ip()),
        controller_ip.parse().ok(),
        &controller_name,
    );
    *state.view_ask.lock().unwrap() = ask.clone();
    let ceiling = if auth.password_slot == ASK_SLOT {
        let who = if ask.view && !auth.peer_name.trim().is_empty() { auth.peer_name.clone() } else { controller_name.clone() };
        let bypass = ask.view && view_is_preapproved(&state, &ask);
        if bypass {
            log_line(&cb, format!("view request from {who} is pre-approved"));
        }
        match approve(&state, &cb, &who, &controller_ip, ask.view, bypass).await {
            Approval::Yes(bits) => {
                grant_auth(&mut auth.send, &state.hostname).await?;
                bits
            }
            Approval::No(reason) => {
                let _ = refuse_auth(&mut auth.send, &reason).await;
                bail!(reason);
            }
        }
    } else {
        state.lockout.lock().unwrap().succeed(&source);
        let matched = admit::matched_permanent(&passwords, auth.password_slot, permanent.as_deref());
        let permanent_set = permanent.as_deref().map(str::trim).is_some_and(|p| !p.is_empty());
        let approval = state.approval.lock().unwrap().clone();
        let policy = state.policy.lock().unwrap().clone();
        match admit::after_password(&approval, matched, permanent_set, &policy, false) {
            Admit::Open(bits) => {
                log_line(&cb, "accepting with the permanent password, without asking");
                grant_auth(&mut auth.send, &state.hostname).await?;
                bits
            }
            Admit::Ask => match approve(&state, &cb, &controller_name, &controller_ip, false, false).await {
                Approval::Yes(bits) => {
                    grant_auth(&mut auth.send, &state.hostname).await?;
                    bits
                }
                Approval::No(reason) => {
                    let _ = refuse_auth(&mut auth.send, &reason).await;
                    bail!(reason);
                }
            },
        }
    };
    if auth.password_slot != ASK_SLOT {
        log_line(&cb, format!("password accepted for {}", auth.peer_name));
    }
    if state.gen.load(Ordering::SeqCst) != gen {
        connection.close(0u32.into(), b"stopped");
        bail!("stopped");
    }
    // The password (or Allow) is done. The picture still needs MediaProjection.
    // Tell the controller before the video stream opens, and keep this session
    // up while the phone shows the consent notification.
    let mut share_deadline = None;
    if !state.capturing.load(Ordering::SeqCst) {
        *state.capture_deny.lock().unwrap() = String::new();
        let _ = write_msg(&mut auth.send, &HostMsg::Notice(SCREEN_WAIT.into())).await;
        emit(&cb, &json!({"event":"needCapture","text": SCREEN_WAIT}));
        log_line(&cb, SCREEN_WAIT.to_string());
        share_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(60));
    }
    if share_deadline.is_none() {
        emit(&cb, &json!({"event":"hosting","peer": auth.peer_name}));
    }
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
            let saved = state.end_reason.lock().unwrap().clone();
            let saved = if saved.is_empty() { "stopped".to_string() } else { saved };
            break Err(anyhow!("{saved}"));
        }
        if let Some(deadline) = share_deadline {
            if state.capturing.load(Ordering::SeqCst) {
                share_deadline = None;
                log_line(&cb, "screen sharing is on");
                emit(&cb, &json!({"event":"hosting","peer": auth.peer_name}));
                emit(&cb, &json!({"event":"keyframe"}));
            } else {
                let deny = state.capture_deny.lock().unwrap().clone();
                let failed = if !deny.is_empty() {
                    Some(screen_share_failure(&deny))
                } else if tokio::time::Instant::now() >= deadline {
                    Some(screen_share_failure(""))
                } else {
                    None
                };
                if let Some(reason) = failed {
                    let _ = write_msg(&mut control_send, &HostMsg::Notice(reason.clone())).await;
                    log_line(&cb, reason.clone());
                    break Err(anyhow!(reason));
                }
            }
        }
        let idle = if share_deadline.is_some() { Duration::from_millis(250) } else { Duration::from_secs(20) };
        let msg = tokio::time::timeout(
            idle,
            read_msg::<_, ControlMsg>(&mut control_recv, MAX_CONTROL_FRAME),
        )
        .await;
        let msg = match msg {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => {
                let reason = if connection.close_reason().is_some() {
                    "the other computer closed the session".to_string()
                } else {
                    format!("the control stream closed: {e:#}")
                };
                break Err(anyhow!("{reason}"));
            }
            Err(_) => {
                // Idle phones still send frames. A quiet control stream is fine
                // as long as the QUIC connection is up.
                if connection.close_reason().is_some() {
                    break Err(anyhow!("the other computer closed the session"));
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
            ControlMsg::Visitor { account, computer, user, id } => {
                let card = pjdesk_proto::peer::visitor_card(&account, &computer, &user, &id);
                let who = if card.secondary.is_empty() {
                    card.headline
                } else {
                    format!("{} ({})", card.headline, card.secondary)
                };
                emit(&cb, &json!({"event":"hosting","peer": who, "who": who}));
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
    let sharing = result.as_ref().err().is_some_and(|e| e.to_string().contains("screen sharing"));
    connection.close(0u32.into(), if sharing { b"screen sharing not allowed" } else { b"session ended" });
    result
}

/// Shown to the controller until MediaProjection is producing frames.
const SCREEN_WAIT: &str = "Waiting for the phone to allow screen sharing";

/// A reason the Windows client can show instead of a generic picture timeout.
fn screen_share_failure(reason: &str) -> String {
    let text = reason.trim();
    let lower = text.to_ascii_lowercase();
    if text.is_empty() {
        return "screen sharing not allowed on the phone: tap the PJDesk notification to allow".into();
    }
    if lower.contains("foreground") || lower.contains("could not start the screen-capture service") {
        return "screen sharing not allowed on the phone: the foreground service is not running".into();
    }
    if lower.contains("expired") || lower.contains("missing") {
        return "screen sharing not allowed on the phone: MediaProjection consent missing or expired".into();
    }
    if lower.contains("denied") || lower.contains("not allowed") || lower.contains("stopped") {
        return format!("screen sharing not allowed on the phone: {text}");
    }
    format!("screen sharing not allowed on the phone: {text}")
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
    if requested & perms::VISITOR != 0 {
        write_msg(send, &HostMsg::AskVisitor).await?;
    }
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

/// The pjdesk.com account when this phone is signed in, otherwise a blank account
/// so the other computer names this phone instead.
fn android_visitor(state: &State) -> ControlMsg {
    let account = state.account.lock().unwrap().clone();
    let facts = state.facts.lock().unwrap().clone();
    let computer = if !facts.hostname.trim().is_empty() { facts.hostname } else { state.hostname.clone() };
    let id = state.device_id.lock().unwrap().clone().unwrap_or_default();
    ControlMsg::Visitor { account, computer, user: facts.console_user, id }
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
    state: &Arc<State>,
    cb: &Arc<dyn Callbacks>,
    endpoint: &quinn::Endpoint,
    passwords: &[String],
    my_name: &str,
    relay_ip: std::net::IpAddr,
    peer_public: SocketAddr,
    controller_ip: &str,
    controller_name: &str,
) -> Result<(quinn::Connection, session::Authenticated, ViewAsk)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let attempt_used = Arc::new(AtomicBool::new(false));
    let mut pending: JoinSet<Result<(quinn::Connection, session::Authenticated, ViewAsk)>> = JoinSet::new();
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
                let (state, cb, controller_ip, controller_name) =
                    (state.clone(), cb.clone(), controller_ip.to_string(), controller_name.to_string());
                pending.spawn(async move {
                    let connection = incoming.await?;
                    let source = lockout::source_key(
                        connection.remote_address().ip(),
                        Some(relay_ip),
                        Some(peer_public.ip()),
                        controller_ip.parse().ok(),
                        &controller_name,
                    );
                    let locked = state.lockout.lock().unwrap().check(&source, std::time::Instant::now());
                    if locked.locked {
                        log_line(&cb, format!("locked out {source} for {} minutes", locked.minutes));
                        refuse_before_password(&connection, &lockout::message(locked.minutes)).await?;
                        return Err(LockedOut { minutes: locked.minutes }.into());
                    }
                    let fail_state = state.clone();
                    let fail_cb = cb.clone();
                    let fail_source = source.clone();
                    let fail = move || {
                        let status = fail_state.lockout.lock().unwrap().fail(&fail_source, std::time::Instant::now());
                        if status.fresh {
                            let who = if status.global { "all connections" } else { fail_source.as_str() };
                            log_line(&fail_cb, format!("locked out {who} for {} minutes after repeated wrong passwords", status.minutes));
                        }
                        if status.locked { lockout::message(status.minutes) } else { "wrong password".into() }
                    };
                    match session::host_auth(&connection, &passwords, &my_name, &attempt_used, &fail).await {
                        Ok(auth) => Ok((connection, auth, ViewAsk::default())),
                        Err(e) if e.is::<NeedsApproval>() => {
                            let wait = e.downcast::<NeedsApproval>().unwrap();
                            let ask = ViewAsk {
                                view: wait.view,
                                account: wait.account.clone(),
                                computer: wait.computer.clone(),
                                id: wait.id.clone(),
                            };
                            let peer_name = if wait.peer_name.trim().is_empty() { controller_name } else { wait.peer_name };
                            Ok((
                                connection,
                                session::Authenticated {
                                    send: wait.send,
                                    recv: wait.recv,
                                    peer_name,
                                    peer_proto: wait.peer_proto,
                                    password_slot: ASK_SLOT,
                                },
                                ask,
                            ))
                        }
                        Err(e) => Err(e),
                    }
                });
            }
            Some(joined) = pending.join_next(), if !pending.is_empty() => match joined {
                Ok(Ok(done)) => return Ok(done),
                Ok(Err(e)) if e.is::<LockedOut>() => return Err(e),
                Ok(Err(e)) if e.is::<WrongPassword>() => {
                    attempt_used.store(false, Ordering::SeqCst);
                }
                Ok(Err(e)) => info!("a connection attempt failed: {e:#}"),
                Err(_) => {}
            },
            _ = tokio::time::sleep_until(deadline) => bail!("the controller did not connect in time"),
        }
    }
}

async fn wait_typed_password(state: &State) -> Result<TypedAuth> {
    let (tx, rx) = oneshot::channel();
    *state.password_tx.lock().unwrap() = Some(tx);
    match tokio::time::timeout(Duration::from_secs(90), rx).await {
        Ok(Ok(reply)) => Ok(reply),
        _ => {
            *state.password_tx.lock().unwrap() = None;
            bail!("the password was not entered in time");
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
    *state.end_reason.lock().unwrap() = String::new();
    emit(&cb, &json!({"event":"status","text": format!("Connecting to {}", format_id(target))}));
    let server = ServerAddr::parse(&state.server.lock().unwrap())?;
    let pinned = state.fingerprint.lock().unwrap().clone().and_then(|s| pjdesk_proto::tls::parse_fingerprint(&s).ok());
    let mut signal = SignalClient::connect(&server, pinned).await?;
    if let Some(fp) = signal.observed {
        *state.fingerprint.lock().unwrap() = Some(pjdesk_proto::tls::format_fingerprint(&fp));
        persist(&state);
    }
    let access = state.access_token.lock().unwrap().clone();
    if !access.is_empty() {
        let _ = signal.send(ClientMsg::PresentAccess { token: access });
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
        match tokio::time::timeout(Duration::from_secs(90), signal.recv()).await {
            Ok(Some(ServerMsg::SessionReady { session_id, token, udp_port, host_name })) => {
                log_line(&cb, format!("the other computer accepted, session {session_id:x}"));
                break (session_id, token, udp_port, host_name);
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
    let mut password = password.to_string();
    let mut from_store = !password.is_empty();
    let mut rejected = false;
    let mut note = String::new();
    let mut ask_person = false;
    let (connection, path, auth) = loop {
        if password.is_empty() && !ask_person {
            emit(
                &cb,
                &json!({"event":"needPassword","id": target, "name": host_name, "rejected": rejected, "message": note}),
            );
            match wait_typed_password(&state).await? {
                TypedAuth::Password(typed) => {
                    if typed.is_empty() {
                        bail!("cancelled");
                    }
                    password = typed;
                    from_store = false;
                }
                TypedAuth::AskPerson => ask_person = true,
                TypedAuth::Cancel => bail!("cancelled"),
            }
        }
        let punching = binding.punch_socket.try_clone().ok().map(|sock| {
            p2p::spawn_punching(sock, candidates.clone(), session_id, Duration::from_secs(8))
        });
        log_line(
            &cb,
            "Trying a direct connection on this network first (UDP 21201), then the relay",
        );
        let connected = p2p::connect_to_host(&endpoint, &candidates, server_udp, false).await;
        if let Some(punching) = punching {
            punching.abort();
        }
        let (connection, path) = connected?;
        let authed = if ask_person {
            session::controller_ask_person(&connection, &state.hostname).await
        } else {
            session::controller_auth(&connection, &password, &state.hostname).await
        };
        match authed {
            Ok(auth) => break (connection, path, auth),
            Err(e) if e.is::<LockedOut>() => {
                connection.close(1u32.into(), b"locked out");
                bail!(e.to_string());
            }
            Err(e) if e.is::<WrongPassword>() => {
                connection.close(1u32.into(), b"authentication failed");
                let mut text = format!("Incorrect password for {}", format_id(target));
                if from_store {
                    emit(&cb, &json!({"event":"clearSaved","id": target}));
                    text.push_str(". The saved password on this computer was cleared.");
                }
                note = text;
                password.clear();
                from_store = false;
                rejected = true;
                ask_person = false;
            }
            Err(e) => {
                connection.close(1u32.into(), b"authentication failed");
                return Err(e);
            }
        }
    };
    let peer = if auth.peer_name.is_empty() { host_name } else { auth.peer_name };
    let path = match path {
        LinkPath::Direct(addr) if p2p::is_lan_ip(addr.ip()) => "LAN",
        LinkPath::Direct(_) => "Internet",
        LinkPath::Relay(_) => "Internet, relayed",
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
            permissions: perms::ALL | perms::LAYOUT | perms::VISITOR,
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
        let mut video = match tokio::time::timeout(Duration::from_secs(70), conn.accept_uni()).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                emit(
                    &cb_v,
                    &json!({"event":"error","text": format!("The other computer did not open the video stream: {e}")}),
                );
                return;
            }
            Err(_) => {
                emit(
                    &cb_v,
                    &json!({"event":"error","text": "No picture arrived. That computer did not start sending its screen."}),
                );
                return;
            }
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
    let end_reason = loop {
        if state.gen.load(Ordering::SeqCst) != gen || state.app_stop.load(Ordering::SeqCst) {
            let saved = state.end_reason.lock().unwrap().clone();
            break if saved.is_empty() { "stopped".to_string() } else { saved };
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
            Ok(HostMsg::AskVisitor) => {
                let _ = control_tx.send(android_visitor(&state));
            }
            Ok(HostMsg::Denied { what, reason }) => {
                emit(&cb, &json!({"event":"status","text": format!("{what}: {reason}")}))
            }
            Ok(_) => {}
            Err(e) => {
                break if connection.close_reason().is_some() {
                    "the other computer closed the session".to_string()
                } else {
                    format!("the control stream closed: {e:#}")
                };
            }
        }
    };
    writer.abort();
    video_task.abort();
    *state.control.lock().unwrap() = None;
    connection.close(0u32.into(), b"viewer closed");
    log_line(&cb, format!("controller session ended: {end_reason}"));
    emit(&cb, &json!({"event":"disconnected","role":"controller","reason": end_reason}));
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

    /// Save, drop the in-memory host (a process restart), hand the stored secret
    /// back the way the Android Keystore does on the next launch, and check that
    /// SPAKE2 accepts that password and rejects a different one.
    #[test]
    fn permanent_password_survives_restart_and_authenticates() {
        let saved = "correct horse";
        let dir = std::env::temp_dir().join(format!("pjdesk-permanent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let cb: Arc<dyn Callbacks> = Arc::new(Silent);
            let state = test_state(&dir);
            let otp = state.otp.lock().unwrap().clone();
            apply(&state, &cb, json!({"cmd":"setPermanent","password": saved})).await;
            assert_eq!(auth_passwords(&state)[1], saved);
            assert!(host_accepts_typed(&auth_passwords(&state), saved));
            assert!(host_accepts_typed(&auth_passwords(&state), &otp));
            persist(&state);
            let on_disk = std::fs::read_to_string(dir.join("pjdesk.json")).unwrap_or_default();
            assert!(!on_disk.contains(saved), "permanent password was written to disk");
            drop(state);

            let restarted = test_state(&dir);
            assert!(restarted.permanent.lock().unwrap().is_none());
            apply(&restarted, &cb, json!({"cmd":"setPermanent","password": saved})).await;
            let passwords = auth_passwords(&restarted);
            assert_eq!(passwords.len(), 2);
            assert_eq!(passwords[1], saved);
            assert!(host_accepts_typed(&passwords, saved));
            assert!(host_accepts_typed(&passwords, &passwords[0]));
            assert!(!host_accepts_typed(&passwords, "wrong horse battery"));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_screen_share_names_the_reason() {
        assert_eq!(
            screen_share_failure(""),
            "screen sharing not allowed on the phone: tap the PJDesk notification to allow"
        );
        assert_eq!(
            screen_share_failure("Could not start the screen-capture service"),
            "screen sharing not allowed on the phone: the foreground service is not running"
        );
        assert_eq!(
            screen_share_failure("MediaProjection consent expired"),
            "screen sharing not allowed on the phone: MediaProjection consent missing or expired"
        );
        assert_eq!(
            screen_share_failure("Screen capture permission denied"),
            "screen sharing not allowed on the phone: Screen capture permission denied"
        );
    }

    #[test]
    fn a_trusted_account_or_computer_or_permanent_password_skips_the_view_prompt() {
        let dir = std::env::temp_dir().join(format!("pjdesk-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = test_state(&dir);
        let ben = ViewAsk { view: true, account: "ben@spader.us".into(), computer: "BENLENOVODESK".into(), id: "692996394".into() };
        assert!(!view_is_preapproved(&state, &ben));
        *state.trusted_accounts.lock().unwrap() = vec!["Ben@Spader.us".into()];
        assert!(view_is_preapproved(&state, &ben));
        *state.trusted_accounts.lock().unwrap() = vec![];
        *state.trusted_computers.lock().unwrap() = vec!["386 626 882".into()];
        let pixel = ViewAsk { view: true, account: String::new(), computer: "Pixel".into(), id: "386626882".into() };
        assert!(view_is_preapproved(&state, &pixel));
        assert!(!view_is_preapproved(&state, &ben));
        *state.trusted_computers.lock().unwrap() = vec![];
        *state.approval.lock().unwrap() = "unattended".into();
        *state.permanent.lock().unwrap() = Some("long-enough-password".into());
        assert!(view_is_preapproved(&state, &ben));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_round_trip_ignores_unknown_future_fields() {
        let raw = r#"{"server":"office.example:21200","fingerprint":"abcd","device_id":"123456789","extra":1}"#;
        let disk: Disk = serde_json::from_str(raw).unwrap();
        assert_eq!(disk.device_id, "123456789");
        assert!(disk.secret_hex.is_empty());
    }

    struct Silent;
    impl Callbacks for Silent {
        fn on_event(&self, _json: &str) {}
        fn on_video(&self, _data: &[u8], _width: u32, _height: u32, _keyframe: bool) {}
    }

    fn test_state(dir: &std::path::Path) -> Arc<State> {
        let identity = TlsIdentity::load_or_create(&dir.join("host.crt"), &dir.join("host.key"), "pjdesk-host").unwrap();
        Arc::new(State {
            dir: dir.to_path_buf(),
            hostname: "Pixel".into(),
            facts: Mutex::new(HostFacts::default()),
            secret: [7u8; 32],
            identity,
            device_id: Mutex::new(None),
            server: Mutex::new("office.example:21200".into()),
            fingerprint: Mutex::new(None),
            otp: Mutex::new(generate_password()),
            permanent: Mutex::new(None),
            approval: Mutex::new("prompt".into()),
            policy: Mutex::new(pjdesk_proto::signal::Policy::default()),
            capture: Mutex::new((1080, 2400)),
            capturing: AtomicBool::new(false),
            capture_deny: Mutex::new(String::new()),
            gen: AtomicU64::new(1),
            approval_tx: Mutex::new(None),
            password_tx: Mutex::new(None),
            lockout: Mutex::new(Lockout::default()),
            end_reason: Mutex::new(String::new()),
            control: Mutex::new(None),
            video: Mutex::new(None),
            app_stop: AtomicBool::new(false),
            access_token: Mutex::new(String::new()),
            account: Mutex::new(String::new()),
            trusted_accounts: Mutex::new(Vec::new()),
            trusted_computers: Mutex::new(Vec::new()),
            view_ask: Mutex::new(ViewAsk::default()),
            suppress_token: AtomicBool::new(false),
        })
    }

    /// The host's SPAKE2 check: the typed password has to match one slot.
    fn host_accepts_typed(host_passwords: &[String], typed: &str) -> bool {
        use pjdesk_proto::auth::{controller_start, host_start};
        let binding = b"restart-channel";
        const SLOTS: usize = 2;
        let (mut ctrl_ex, mut ctrl_msgs) = (Vec::new(), Vec::new());
        for _ in 0..SLOTS {
            let (ex, msg) = controller_start(typed);
            ctrl_ex.push(ex);
            ctrl_msgs.push(msg);
        }
        let n = host_passwords.len().min(ctrl_msgs.len());
        let (mut host_keys, mut host_msgs, mut host_confirms) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..n {
            let (ex, msg) = host_start(&host_passwords[i]);
            let key = ex.finish(&ctrl_msgs[i]).expect("host spake");
            host_confirms.push(key.host_confirm(binding));
            host_msgs.push(msg);
            host_keys.push(key);
        }
        for (i, ex) in ctrl_ex.into_iter().enumerate() {
            if i >= host_msgs.len() {
                break;
            }
            if let Ok(key) = ex.finish(&host_msgs[i]) {
                if key.verify_host(binding, &host_confirms[i]) {
                    let confirm = key.controller_confirm(binding);
                    return host_keys[i].verify_controller(binding, &confirm);
                }
            }
        }
        false
    }
}
