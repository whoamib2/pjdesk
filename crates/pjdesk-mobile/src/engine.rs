//! Host registration and controller sessions for the Android app.
//!
//! Video bytes stay in Kotlin (MediaCodec). This task owns the sockets.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use pjdesk_link::admit::{self, Admit};
use pjdesk_link::lockout::{self, Lockout};
use pjdesk_link::p2p::{self, LinkPath};
use pjdesk_link::session::{
    self, grant_auth, refuse_auth, refuse_before_password, LockedOut, NeedsApproval, WrongPassword,
};
use pjdesk_link::signal::{ServerAddr, SignalClient};
use pjdesk_proto::auth::generate_password;
use pjdesk_proto::filesafe::{self, MAX_TRANSFER_BYTES};
use pjdesk_proto::framing::{read_msg, write_msg, MAX_CONTROL_FRAME};
use pjdesk_proto::gesture::{Gesture, PointerAdapter, GESTURE_LONG_PRESS, GESTURE_PINCH, GESTURE_SWIPE, GESTURE_TAP};
use pjdesk_proto::peer::{
    read_video_frame, write_side_chunk, write_side_header, write_video_frame, ClipKind, Clipboard, ControlMsg,
    ChatEntry, DisplayLayout, FileMsg, HostMsg, InputEvent, KeyboardMode, MonitorInfo, SideHeader, SpecialCommand,
    VideoHeader,
    FILE_MAGIC, MAX_CLIP_TEXT, VIEW_SINGLE,
};
use pjdesk_proto::perms::{self, grant};
use pjdesk_proto::quality::{self, StreamQuality};
use pjdesk_proto::signal::{ClientMsg, ServerMsg};
use pjdesk_proto::tls::TlsIdentity;
use pjdesk_proto::udp::Role;
use pjdesk_proto::{
    derive_device_secret, format_id, is_valid_id, normalize_id, LEGACY_REGISTERED_SERVER, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Shown in Register and on the About screen. Independent of the Windows crate version.
pub const APP_VERSION: &str = "0.6.61";

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
    /// Random key stored in pjdesk.json. Separate from the Android id secret.
    install_key: [u8; 32],
    /// Set when a server closed RegisterInstall before answering.
    legacy_register: AtomicBool,
    identity: TlsIdentity,
    device_id: Mutex<Option<String>>,
    server: Mutex<String>,
    /// Private addresses of the active server, from the account (`ip` or `ip:port`).
    lan: Mutex<Vec<String>>,
    /// Public IP the account published for the active server.
    public_ip: Mutex<String>,
    /// Standby server from the account, used when the primary stops answering.
    standby: Mutex<String>,
    /// This phone's public IP from pjdesk.com, used to decide the LAN path.
    my_public_ip: Mutex<String>,
    /// IPv4 addresses the phone reported for the network it is on now.
    phone_mine: Mutex<Vec<String>>,
    /// Already left a public registration once to try the server's LAN address.
    lan_shift: AtomicBool,
    /// Advertised addresses of both servers. Hostnames are dialed last.
    routes: Mutex<pjdesk_proto::routes::ServerRoutes>,
    fingerprint: Mutex<Option<String>>,
    otp: Mutex<String>,
    /// Temporary 8-digit code for Let someone connect.
    support_code: Mutex<String>,
    /// Temporary password that matches the support code. Not the permanent one.
    support_password: Mutex<String>,
    /// Unix time the current support pair was created.
    support_born: AtomicU64,
    /// Set once a session has started, so it gets a new support pair when it ends.
    support_hold: AtomicBool,
    permanent: Mutex<Option<String>>,
    approval: Mutex<String>,
    policy: Mutex<pjdesk_proto::signal::Policy>,
    capture: Mutex<(u16, u16)>,
    /// True while MediaProjection is producing frames.
    capturing: AtomicBool,
    /// Settings → Security → Unattended access. Off until the person turns it on.
    /// A permanent-password or trusted session then accepts the screen-share dialog.
    unattended_access: AtomicBool,
    /// The accessibility service is connected and can turn input into taps.
    control_ready: AtomicBool,
    /// A notice the open host session should send, such as control being off.
    control_note: Mutex<Option<String>>,
    /// Wakes the host session so a local stop is not stuck behind the idle read.
    session_wake: Notify,
    /// Set when the person denies screen sharing, or the consent fails.
    capture_deny: Mutex<String>,
    /// Bumped to end the current session. The host registration keeps running.
    gen: AtomicU64,
    /// Bumped to end a session this phone is controlling, without dropping the host.
    view_gen: AtomicU64,
    /// Wakes a controller retry so Close and Reconnect now are not stuck in the wait.
    view_wake: Notify,
    /// Monitor to select again after the same session reconnects.
    view_monitor: AtomicU8,
    /// True when the session was showing one monitor's layout, not the whole desktop.
    view_layout: AtomicBool,
    approval_tx: Mutex<Option<oneshot::Sender<Result<(), String>>>>,
    password_tx: Mutex<Option<oneshot::Sender<TypedAuth>>>,
    lockout: Mutex<Lockout>,
    end_reason: Mutex<String>,
    control: Mutex<Option<mpsc::UnboundedSender<ControlMsg>>>,
    video: Mutex<Option<Arc<VideoOut>>>,
    app_stop: AtomicBool,
    access_token: Mutex<String>,
    /// Refresh token kept in memory so an older server can be signed in before Register.
    /// It is not written to pjdesk.json.
    account_refresh: Mutex<String>,
    account_device: Mutex<String>,
    /// pjdesk.com account email while this phone is signed in. Empty otherwise.
    account: Mutex<String>,
    trusted_accounts: Mutex<Vec<String>>,
    trusted_computers: Mutex<Vec<String>>,
    view_ask: Mutex<ViewAsk>,
    /// Set when a server from before 0.6.0 drops the connection after PresentAccess.
    suppress_token: AtomicBool,
    /// Server access key. Kept in memory. The Android keystore is what saves it.
    access_key: Mutex<String>,
    /// Bumped when the key changes so the host presents it on the next connection.
    access_epoch: AtomicU64,
    /// Connection for the session this phone is controlling. File bytes use it.
    viewer: Mutex<Option<quinn::Connection>>,
    files: Mutex<HashMap<u64, FileOffer>>,
    file_seq: AtomicU64,
    /// Non-zero while a controller is connected to this phone. A later session
    /// uses a new value, so an older session ending cannot clear it.
    host_live: AtomicU64,
    host_live_seq: AtomicU64,
    /// Primary a relayed session was told to use. Tried before the catalog.
    move_primary: Mutex<String>,
    /// Session the server asked this phone to move, and the primary address.
    relay_move: Mutex<Option<(u64, String)>>,
    /// Wakes registration so this phone follows that primary.
    moved: Notify,
    /// Set with [`State::moved`] so a notify that arrives early is not lost.
    leave_primary: AtomicBool,
    /// The controller's Hello carried CHAT_PEER. This phone may send ChatLine.
    host_chat_peer: AtomicBool,
    /// The computer being controlled sent ChatReady.
    view_chat_peer: AtomicBool,
    /// Replies from the person at this phone, written on the host session.
    host_chat: Mutex<Vec<ChatEntry>>,
    /// This phone can be controlled and cannot start a session.
    incoming_only: AtomicBool,
    /// The server has sent IncomingOnly, so this phone may answer.
    incoming_ready: AtomicBool,
    /// This phone has told the server about a lock it already had.
    incoming_told: AtomicBool,
    /// `(on, password)` waiting for the server. The password is not logged.
    incoming_ask: Mutex<Option<(bool, String)>>,
    /// The heartbeating registration connection, once the server has registered this phone.
    host_tx: Mutex<Option<mpsc::UnboundedSender<ClientMsg>>>,
    /// `address:port` of [`State::host_tx`]. Empty when that connection is down.
    host_at: Mutex<String>,
    /// Sign-in waiting for an AccountResult on [`State::host_tx`].
    pending_account: Mutex<Option<(String, oneshot::Sender<ServerMsg>)>>,
    /// A sign-in connection registered this phone. The heartbeat connection should dial again.
    sign_in_reregister: AtomicBool,
    /// Bumped each time a registration connection is ready. Sign-in waits for the next one.
    host_gen: AtomicU64,
}

#[derive(Clone)]
struct FileOffer {
    path: PathBuf,
    name: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Disk {
    #[serde(default)]
    server: String,
    #[serde(default)]
    lan: String,
    #[serde(default)]
    public_ip: String,
    #[serde(default)]
    standby: String,
    #[serde(default)]
    my_public_ip: String,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    device_id: String,
    /// Used only when the device has no Android id to derive a secret from.
    #[serde(default)]
    secret_hex: String,
    /// Random per-install key. Empty in a file written before 0.6.47.
    #[serde(default)]
    install_key_hex: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    routes: String,
    /// This phone cannot start a session. Missing in a file from before 0.6.50.
    #[serde(default)]
    incoming_only: bool,
}

pub struct Engine {
    state: Arc<State>,
    cmds: mpsc::UnboundedSender<Value>,
}

/// One controller session. Dropping it, or [`HostLive::end`], clears the flag
/// only when no newer session has replaced it.
struct HostLive {
    state: Arc<State>,
    gen: u64,
}

impl HostLive {
    fn enter(state: &Arc<State>) -> Self {
        let gen = state.host_live_seq.fetch_add(1, Ordering::SeqCst).wrapping_add(1).max(1);
        state.host_live.store(gen, Ordering::SeqCst);
        Self { state: state.clone(), gen }
    }

    fn end(&self) {
        let _ = self.state.host_live.compare_exchange(self.gen, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

impl Drop for HostLive {
    fn drop(&mut self) {
        self.end();
    }
}

fn host_attached(state: &State) -> bool {
    state.host_live.load(Ordering::SeqCst) != 0
}

impl Engine {
    /// True while a controller is still connected. Capture must not start after this is false.
    pub fn host_attached(&self) -> bool {
        host_attached(&self.state)
    }

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
        let mut dirty = false;
        if android_id.trim().is_empty() && stored.secret_hex.is_empty() {
            stored.secret_hex = hex::encode(secret);
            dirty = true;
        }
        let install_key = match decode_install_key(&stored.install_key_hex) {
            Some(key) => key,
            None => {
                let key = pjdesk_proto::random_bytes::<32>();
                stored.install_key_hex = hex::encode(key);
                dirty = true;
                key
            }
        };
        if dirty {
            save_disk(&dir, &stored);
        }
        let routes = stored_routes(&stored);
        let state = Arc::new(State {
            dir,
            hostname: if hostname.trim().is_empty() { "Android".into() } else { hostname },
            facts: Mutex::new(HostFacts::default()),
            secret,
            install_key,
            legacy_register: AtomicBool::new(false),
            identity,
            device_id: Mutex::new((!stored.device_id.is_empty()).then_some(stored.device_id)),
            server: Mutex::new(server),
            lan: Mutex::new(split_lan(&stored.lan)),
            public_ip: Mutex::new(stored.public_ip),
            standby: Mutex::new(stored.standby),
            my_public_ip: Mutex::new(stored.my_public_ip),
            phone_mine: Mutex::new(Vec::new()),
            lan_shift: AtomicBool::new(false),
            routes: Mutex::new(routes),
            fingerprint: Mutex::new((!stored.fingerprint.is_empty()).then_some(stored.fingerprint)),
            otp: Mutex::new(generate_password()),
            support_code: Mutex::new(pjdesk_proto::random_digits(8)),
            support_password: Mutex::new(generate_password()),
            support_born: AtomicU64::new(unix_now()),
            support_hold: AtomicBool::new(false),
            permanent: Mutex::new(None),
            approval: Mutex::new("prompt".into()),
            policy: Mutex::new(pjdesk_proto::signal::Policy::default()),
            capture: Mutex::new((1080, 2400)),
            capturing: AtomicBool::new(false),
            unattended_access: AtomicBool::new(false),
            control_ready: AtomicBool::new(false),
            control_note: Mutex::new(None),
            session_wake: Notify::new(),
            capture_deny: Mutex::new(String::new()),
            gen: AtomicU64::new(1),
            view_gen: AtomicU64::new(1),
            view_wake: Notify::new(),
            view_monitor: AtomicU8::new(0),
            view_layout: AtomicBool::new(false),
            approval_tx: Mutex::new(None),
            password_tx: Mutex::new(None),
            lockout: Mutex::new(Lockout::default()),
            end_reason: Mutex::new(String::new()),
            control: Mutex::new(None),
            video: Mutex::new(None),
            app_stop: AtomicBool::new(false),
            access_token: Mutex::new(stored.access_token.clone()),
            account_refresh: Mutex::new(String::new()),
            account_device: Mutex::new(String::new()),
            account: Mutex::new(String::new()),
            trusted_accounts: Mutex::new(Vec::new()),
            trusted_computers: Mutex::new(Vec::new()),
            view_ask: Mutex::new(ViewAsk::default()),
            suppress_token: AtomicBool::new(false),
            access_key: Mutex::new(String::new()),
            access_epoch: AtomicU64::new(0),
            viewer: Mutex::new(None),
            files: Mutex::new(HashMap::new()),
            file_seq: AtomicU64::new(1),
            host_live: AtomicU64::new(0),
            host_live_seq: AtomicU64::new(0),
            move_primary: Mutex::new(String::new()),
            relay_move: Mutex::new(None),
            moved: Notify::new(),
            leave_primary: AtomicBool::new(false),
            host_chat_peer: AtomicBool::new(false),
            view_chat_peer: AtomicBool::new(false),
            host_chat: Mutex::new(Vec::new()),
            incoming_only: AtomicBool::new(stored.incoming_only),
            incoming_ready: AtomicBool::new(false),
            incoming_told: AtomicBool::new(false),
            incoming_ask: Mutex::new(None),
            host_tx: Mutex::new(None),
            host_at: Mutex::new(String::new()),
            pending_account: Mutex::new(None),
            sign_in_reregister: AtomicBool::new(false),
            host_gen: AtomicU64::new(0),
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
        let mut slot = video.latest.lock().unwrap();
        let replace = slot.as_ref().map(|pending| should_replace_frame(pending.keyframe, keyframe)).unwrap_or(true);
        if replace {
            *slot = Some(Outgoing { data, width, height, keyframe, encode_us });
        }
        drop(slot);
        if replace {
            video.notify.notify_one();
        }
    }
}

/// A delta must not replace an IDR that has not been sent yet. The viewer
/// asked for that IDR because the previous picture would not decode.
pub(crate) fn should_replace_frame(pending_keyframe: bool, next_keyframe: bool) -> bool {
    !pending_keyframe || next_keyframe
}

fn decode_install_key(hexed: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hexed.trim()).ok()?;
    let key = <[u8; 32]>::try_from(bytes).ok()?;
    if key == [0u8; 32] { None } else { Some(key) }
}

fn reported_hostname(state: &State) -> String {
    let reported = state.facts.lock().unwrap().hostname.clone();
    if reported.trim().is_empty() { state.hostname.clone() } else { reported }
}

fn send_host_register(signal: &mut SignalClient, state: &State) -> Result<()> {
    let device_id = state.device_id.lock().unwrap().clone().filter(|id| is_valid_id(id));
    let hostname = reported_hostname(state);
    if state.legacy_register.load(Ordering::SeqCst) {
        signal.send(ClientMsg::Register {
            proto: PROTOCOL_VERSION,
            device_id,
            device_secret: state.secret,
            hostname,
            app_version: APP_VERSION.into(),
        })
    } else {
        signal.send(ClientMsg::RegisterInstall {
            proto: PROTOCOL_VERSION,
            device_id,
            device_secret: state.secret,
            install_key: state.install_key,
            hostname,
            app_version: APP_VERSION.into(),
        })
    }
}

enum PhoneRefresh {
    Token(String),
    /// The server answered, and did not issue a token. The connection is still open.
    Refused,
    /// The connection closed. Register has to dial again.
    Closed,
}

/// One Account refresh on this connection, before Register. The token is not logged.
async fn refresh_phone_access(signal: &mut SignalClient, state: &State) -> PhoneRefresh {
    let refresh = state.account_refresh.lock().unwrap().clone();
    let device = state.account_device.lock().unwrap().clone();
    if refresh.is_empty() || device.trim().is_empty() {
        return PhoneRefresh::Refused;
    }
    if signal
        .send(ClientMsg::Account {
            op: "refresh".into(),
            email: String::new(),
            auth_key: String::new(),
            refresh,
            device_id: device,
            device_name: reported_hostname(state),
            vault: String::new(),
            app_lock: String::new(),
            computers: Vec::new(),
            proof: String::new(),
        })
        .is_err()
    {
        return PhoneRefresh::Closed;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return PhoneRefresh::Refused;
        }
        match tokio::time::timeout(left, signal.recv()).await {
            Ok(Some(ServerMsg::AccountResult { ok: true, access, .. })) if !access.is_empty() => {
                return PhoneRefresh::Token(access);
            }
            Ok(Some(ServerMsg::ServerVersion { version })) => {
                pjdesk_proto::routes::note_server_version(&mut state.routes.lock().unwrap(), &version);
            }
            Ok(Some(ServerMsg::Routes(_))) => {}
            Ok(Some(ServerMsg::AccountResult { .. })) | Ok(Some(ServerMsg::Error { .. })) => return PhoneRefresh::Refused,
            Ok(None) => return PhoneRefresh::Closed,
            Err(_) => return PhoneRefresh::Refused,
            Ok(Some(_)) => {}
        }
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
        lan: state.lan.lock().unwrap().join(","),
        public_ip: state.public_ip.lock().unwrap().clone(),
        standby: state.standby.lock().unwrap().clone(),
        my_public_ip: state.my_public_ip.lock().unwrap().clone(),
        fingerprint: state.fingerprint.lock().unwrap().clone().unwrap_or_default(),
        device_id: state.device_id.lock().unwrap().clone().unwrap_or_default(),
        secret_hex: String::new(),
        install_key_hex: hex::encode(state.install_key),
        access_token: state.access_token.lock().unwrap().clone(),
        routes: serde_json::to_string(&*state.routes.lock().unwrap()).unwrap_or_default(),
        incoming_only: state.incoming_only.load(Ordering::SeqCst),
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
            "supportCode": state.support_code.lock().unwrap().clone(),
            "supportPassword": state.support_password.lock().unwrap().clone(),
            "supportBorn": state.support_born.load(Ordering::SeqCst),
            "permanent": permanent,
            "fingerprint": fp,
            "server": server,
            "approval": state.approval.lock().unwrap().clone(),
            "appVersion": APP_VERSION,
            "incomingOnly": state.incoming_only.load(Ordering::SeqCst),
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
                state.unattended_access.store(false, Ordering::SeqCst);
                publish(state, cb);
            } else if permanent_ok(&pw) {
                *state.permanent.lock().unwrap() = Some(pw);
                publish(state, cb);
            } else {
                emit(cb, &json!({"event":"error","text":"Use at least 8 characters for a permanent password."}));
            }
        }
        "setControl" => {
            let on = cmd.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
            let was = state.control_ready.swap(on, Ordering::SeqCst);
            if on != was {
                let text = if on { CONTROL_ON } else { CONTROL_OFF };
                *state.control_note.lock().unwrap() = Some(text.to_string());
                state.session_wake.notify_waiters();
            }
        }
        "screenWait" => {
            // A single-app capture was thrown away. The picture is not up yet,
            // or it has to wait again. Same sentence the PC already knows.
            state.capturing.store(false, Ordering::SeqCst);
            *state.control_note.lock().unwrap() = Some(SCREEN_WAIT.to_string());
            state.session_wake.notify_waiters();
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
        "setUnattendedAccess" => {
            let on = cmd.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
            // A process start reports the saved switch. It must not be cleared
            // here because the password or accessibility service is not ready yet.
            let sync = cmd.get("sync").and_then(|v| v.as_bool()).unwrap_or(false);
            if sync {
                state.unattended_access.store(on, Ordering::SeqCst);
            } else {
                let allowed = on && state.permanent.lock().unwrap().as_deref().is_some_and(|p| !p.trim().is_empty());
                state.unattended_access.store(allowed, Ordering::SeqCst);
                if on && !allowed {
                    emit(cb, &json!({"event":"error","text":"Save a permanent password first."}));
                }
            }
        }
        "regenerate" => {
            *state.otp.lock().unwrap() = generate_password();
            publish(state, cb);
        }
        "refreshSupport" => {
            refresh_support(state);
            publish(state, cb);
        }
        "hostFacts" => {
            let mut facts = state.facts.lock().unwrap();
            if let Some(host) = cmd.get("hostname").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
                facts.hostname = host.chars().take(64).collect();
            }
            if let Some(family) = cmd.get("os_family").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty())
            {
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
            let refresh = cmd.get("refresh").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            let device = cmd.get("device").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            // An empty token with a refresh keeps the saved access token. Sign-out
            // sends neither, and that clears the session. The refresh secret is not logged.
            if token.is_empty() && refresh.is_none() && device.is_none() {
                *state.access_token.lock().unwrap() = String::new();
                *state.account_refresh.lock().unwrap() = String::new();
                *state.account_device.lock().unwrap() = String::new();
                emit(cb, &json!({"event":"accountSession","state":""}));
            } else {
                if !token.is_empty() {
                    *state.access_token.lock().unwrap() = token;
                    emit(cb, &json!({"event":"accountSession","state":"valid"}));
                }
                if let Some(refresh) = refresh {
                    *state.account_refresh.lock().unwrap() = refresh;
                }
                if let Some(device) = device {
                    *state.account_device.lock().unwrap() = device;
                }
            }
            state.suppress_token.store(false, Ordering::SeqCst);
            persist(state);
        }
        "setAccessKey" => {
            let key = cmd.get("key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            *state.access_key.lock().unwrap() = key;
            state.access_epoch.fetch_add(1, Ordering::SeqCst);
        }
        "account" => {
            let email = cmd.get("email").and_then(|v| v.as_str()).unwrap_or("").trim();
            *state.account.lock().unwrap() = email.chars().filter(|c| !c.is_control()).take(80).collect();
        }
        "accountOp" => {
            let op = cmd.get("op").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let email = cmd.get("email").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let auth_key = cmd.get("auth_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let refresh = cmd.get("refresh").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let device_id = cmd.get("device_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let device_name = cmd.get("device_name").and_then(|v| v.as_str()).unwrap_or("Android").to_string();
            let vault = cmd.get("vault").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let app_lock = cmd.get("app_lock").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let computers = fleet_from_command(cmd.get("computers"));
            if !refresh.is_empty() {
                *state.account_refresh.lock().unwrap() = refresh.clone();
            }
            if !device_id.is_empty() {
                *state.account_device.lock().unwrap() = device_id.clone();
            }
            let prefer = cmd.get("address").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let state = state.clone();
            let cb = cb.clone();
            tokio::spawn(async move {
                let asked = op.clone();
                let msg = ClientMsg::Account {
                    op,
                    email,
                    auth_key,
                    refresh,
                    device_id,
                    device_name,
                    vault,
                    app_lock,
                    computers,
                    proof: String::new(),
                };
                let outcome = if prefer.is_empty() {
                    exchange_account(&state, &cb, &asked, msg).await
                } else {
                    account_on_address(&state, &cb, &asked, &prefer, msg).await
                };
                match outcome {
                    Ok((reply, addr)) => {
                        let mut ev = account_event(&asked, &reply);
                        ev["server"] = json!(addr);
                        emit(&cb, &ev);
                    }
                    Err(err) => {
                        let message = err.to_string();
                        let message = message.lines().next().unwrap_or(&message).trim();
                        sign_in_log(&cb, "", "failed", message);
                        emit(&cb, &json!({"event":"account","ok": false, "op": asked, "message": message}));
                    }
                }
            });
        }
        "phoneMine" => {
            if let Some(list) = cmd.get("mine").and_then(|v| v.as_array()) {
                *state.phone_mine.lock().unwrap() = list
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        }
        "setTrusted" => {
            *state.trusted_accounts.lock().unwrap() = trusted_list(cmd.get("accounts"));
            *state.trusted_computers.lock().unwrap() = trusted_list(cmd.get("computers"));
        }
        "server" => {
            let address = cmd.get("address").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let quiet = cmd.get("quiet").and_then(|v| v.as_bool()).unwrap_or(false);
            if ServerAddr::parse(&address).is_err() {
                emit(cb, &json!({"event":"error","text":"That server address is not host or host:port."}));
                log_line(cb, format!("server rejected address len={}", address.len()));
            } else {
                if let Some(fp) =
                    cmd.get("fingerprint").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty())
                {
                    if let Ok(parsed) = pjdesk_proto::tls::parse_fingerprint(fp) {
                        *state.fingerprint.lock().unwrap() = Some(pjdesk_proto::tls::format_fingerprint(&parsed));
                    }
                }
                *state.server.lock().unwrap() = address.clone();
                if let Some(list) = cmd.get("lan").and_then(|v| v.as_array()) {
                    *state.lan.lock().unwrap() = list
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                if let Some(ip) = cmd.get("publicIp").and_then(|v| v.as_str()) {
                    *state.public_ip.lock().unwrap() = ip.trim().to_string();
                }
                if let Some(standby) = cmd.get("standby").and_then(|v| v.as_str()) {
                    *state.standby.lock().unwrap() = standby.trim().to_string();
                }
                if let Some(ip) = cmd.get("myPublicIp").and_then(|v| v.as_str()) {
                    *state.my_public_ip.lock().unwrap() = ip.trim().to_string();
                }
                if let Some(list) = cmd.get("mine").and_then(|v| v.as_array()) {
                    *state.phone_mine.lock().unwrap() = list
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                let incoming = routes_from_command(&cmd);
                let previous = state.routes.lock().unwrap().clone();
                *state.routes.lock().unwrap() = pjdesk_proto::routes::keep_other_server(&previous, &incoming);
                persist(state);
                publish(state, cb);
                log_line(
                    cb,
                    format!(
                        "server set {} fingerprint={}",
                        address,
                        if state.fingerprint.lock().unwrap().is_some() { "set" } else { "missing" }
                    ),
                );
                if !quiet {
                    emit(
                        cb,
                        &json!({"event":"status","text":"Server saved. Reconnect by leaving and opening PJDesk if the old server is still up."}),
                    );
                }
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
        "incomingOnly" => {
            let on = cmd.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
            let password = cmd.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if on {
                state.incoming_only.store(true, Ordering::SeqCst);
                persist(state);
                emit(cb, &json!({"event":"incomingOnly","on": true}));
            }
            *state.incoming_ask.lock().unwrap() = Some((on, password));
            publish(state, cb);
        }
        "connect" => {
            if state.incoming_only.load(Ordering::SeqCst) {
                emit(cb, &json!({"event":"error","text": pjdesk_proto::signal::INCOMING_ONLY_REFUSAL}));
                return;
            }
            let id = normalize_id(cmd.get("id").and_then(|v| v.as_str()).unwrap_or(""));
            let password = cmd.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ask_now = cmd.get("ask").and_then(|v| v.as_bool()).unwrap_or(false);
            let label = cmd.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let shown = if label.is_empty() { format_id(&id) } else { label.to_string() };
            let server_text = state.server.lock().unwrap().clone();
            let pw = if ask_now {
                "ask-person"
            } else if password.is_empty() {
                "none"
            } else {
                "present"
            };
            log_line(
                cb,
                format!(
                    "connect id={} name={} password={} server={}",
                    format_id(&id),
                    if shown.is_empty() { "(unnamed)" } else { shown.as_str() },
                    pw,
                    if server_text.trim().is_empty() { "missing" } else { server_text.trim() },
                ),
            );
            if !is_valid_id(&id) {
                emit(cb, &json!({"event":"error","text":"That isn't a 9-digit PJDesk ID."}));
                return;
            }
            if server_text.trim().is_empty() {
                emit(
                    cb,
                    &json!({"event":"error","text":"This phone has no server address yet. Sign in again, or set the server in Settings."}),
                );
                return;
            }
            emit(cb, &json!({"event":"status","text": format!("Connecting to {shown}…")}));
            state.gen.fetch_add(1, Ordering::SeqCst);
            let gen = state.gen.load(Ordering::SeqCst);
            let view_mark = state.view_gen.load(Ordering::SeqCst);
            let (state, cb) = (state.clone(), cb.clone());
            tokio::spawn(async move {
                let mut established = false;
                let mut delay = Duration::from_secs(1);
                let mut prefer_standby = false;
                loop {
                    if state.view_gen.load(Ordering::SeqCst) != view_mark
                        || state.gen.load(Ordering::SeqCst) != gen
                        || state.app_stop.load(Ordering::SeqCst)
                    {
                        break;
                    }
                    match controller_session(
                        state.clone(),
                        cb.clone(),
                        &id,
                        &password,
                        gen,
                        view_mark,
                        ask_now,
                        prefer_standby,
                        established,
                    )
                    .await
                    {
                        Ok(()) => break,
                        Err(e) => {
                            if state.view_gen.load(Ordering::SeqCst) != view_mark
                                || state.gen.load(Ordering::SeqCst) != gen
                                || state.app_stop.load(Ordering::SeqCst)
                            {
                                break;
                            }
                            let raw = format!("{e:#}");
                            let head = raw.lines().next().unwrap_or("").trim().to_string();
                            let lower = head.to_ascii_lowercase();
                            if lower == "stopped" || lower == "cancelled" || lower.contains("cancelled") {
                                break;
                            }
                            if let Some(plain) = pjdesk_proto::failover::session_refusal(&head) {
                                log_line(&cb, format!("connect refused: {plain}"));
                                if established {
                                    emit(&cb, &json!({"event":"disconnected","role":"controller","reason": plain}));
                                } else if let Some(text) = surfaced_connect_error(&head) {
                                    let text = explain_connect_error(&id, "", &anyhow::Error::msg(text));
                                    emit(&cb, &json!({"event":"error","text": &text}));
                                    emit(&cb, &json!({"event":"disconnected","role":"controller","reason": &text}));
                                } else {
                                    emit(&cb, &json!({"event":"disconnected","role":"controller","reason": plain}));
                                }
                                break;
                            }
                            let lost = established
                                || lower.contains("session lost:")
                                || pjdesk_proto::failover::is_relay_resume(&head);
                            if !lost {
                                log_line(&cb, format!("connect ended: {head}"));
                                if let Some(text) = surfaced_connect_error(&head) {
                                    let text = explain_connect_error(&id, "", &anyhow::Error::msg(text));
                                    emit(&cb, &json!({"event":"error","text": &text}));
                                    emit(&cb, &json!({"event":"disconnected","role":"controller","reason": &text}));
                                }
                                break;
                            }
                            established = true;
                            prefer_standby = false;
                            let secs = delay.as_secs().max(1);
                            log_line(&cb, format!("connection lost; trying again in {secs} s"));
                            emit(
                                &cb,
                                &json!({
                                    "event": "reconnecting",
                                    "after": secs,
                                    "reason": "Connection lost, reconnecting…"
                                }),
                            );
                            let woke = tokio::select! {
                                _ = tokio::time::sleep(delay) => false,
                                _ = state.view_wake.notified() => true,
                            };
                            if state.view_gen.load(Ordering::SeqCst) != view_mark
                                || state.gen.load(Ordering::SeqCst) != gen
                                || state.app_stop.load(Ordering::SeqCst)
                            {
                                break;
                            }
                            if woke {
                                delay = Duration::from_secs(1);
                            } else {
                                delay = pjdesk_proto::signal::next_reconnect_delay(delay);
                            }
                        }
                    }
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
        "stopView" => {
            let reason = cmd.get("reason").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("stopped");
            let reason = reason.chars().take(200).collect::<String>();
            *state.end_reason.lock().unwrap() = reason.clone();
            state.view_gen.fetch_add(1, Ordering::SeqCst);
            state.view_wake.notify_one();
            if let Some(tx) = state.password_tx.lock().unwrap().take() {
                let _ = tx.send(TypedAuth::Cancel);
            }
            let active = state.control.lock().unwrap().is_some();
            *state.control.lock().unwrap() = None;
            if active {
                emit(cb, &json!({"event":"disconnected","role":"controller","reason": reason}));
            }
        }
        "stop" => {
            let reason = cmd.get("reason").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("stopped");
            let reason = reason.chars().take(200).collect::<String>();
            *state.end_reason.lock().unwrap() = reason.clone();
            state.view_gen.fetch_add(1, Ordering::SeqCst);
            state.view_wake.notify_one();
            state.gen.fetch_add(1, Ordering::SeqCst);
            *state.control.lock().unwrap() = None;
            *state.video.lock().unwrap() = None;
            state.session_wake.notify_waiters();
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
            if let Some(q) = stream_quality(&cmd) {
                send_control(state, ControlMsg::SetQuality(q));
            }
        }
        "privacy" => {
            let enabled = cmd.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            send_control(state, ControlMsg::Privacy { enabled });
        }
        "keyboardMode" => {
            let mode = match cmd.get("mode").and_then(|v| v.as_str()).unwrap_or("translate") {
                "map" => KeyboardMode::Map,
                _ => KeyboardMode::Translate,
            };
            send_control(state, ControlMsg::SetKeyboardMode(mode));
        }
        "restart" => send_control(state, ControlMsg::Restart),
        "ping" => {
            let t_us = cmd.get("t_us").and_then(|v| v.as_u64()).unwrap_or(0);
            send_control(state, ControlMsg::Ping { t_us });
        }
        "fileList" => {
            let id = state.file_seq.fetch_add(1, Ordering::SeqCst);
            send_control(state, ControlMsg::File(FileMsg::List { id, path: String::new() }));
        }
        "fileSend" => {
            let path = PathBuf::from(cmd.get("path").and_then(|v| v.as_str()).unwrap_or(""));
            let Some(name) = path.file_name().and_then(|s| s.to_str()).map(|s| s.to_string()) else {
                emit(cb, &json!({"event":"status","text":"That file cannot be sent"}));
                return;
            };
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if filesafe::sanitize_name(&name).is_err() || size == 0 || size > MAX_TRANSFER_BYTES {
                emit(cb, &json!({"event":"status","text":"That file cannot be sent"}));
                return;
            }
            if state.viewer.lock().unwrap().is_none() {
                emit(cb, &json!({"event":"status","text":"Not connected"}));
                return;
            }
            let id = state.file_seq.fetch_add(1, Ordering::SeqCst);
            state.files.lock().unwrap().insert(id, FileOffer { path, name: name.clone() });
            send_control(state, ControlMsg::File(FileMsg::Offer { id, name: name.clone(), size, resume_from: 0 }));
            emit(cb, &json!({"event":"status","text": format!("Offering {name}")}));
        }
        "reconnectNow" => {
            state.view_wake.notify_one();
        }
        "display" => {
            let index = cmd.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
            let layout = cmd.get("layout").and_then(|v| v.as_bool()).unwrap_or(false);
            state.view_monitor.store(index, Ordering::SeqCst);
            state.view_layout.store(layout, Ordering::SeqCst);
            if layout {
                send_control(state, ControlMsg::View { mode: VIEW_SINGLE, display: index });
            } else {
                send_control(state, ControlMsg::SelectMonitor { index });
            }
        }
        "keyframe" => send_control(state, ControlMsg::RequestKeyframe),
        "chatSeen" => {
            emit(cb, &json!({"event":"chatSeen"}));
        }
        "chat" => {
            let text = cmd.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let controlling = state.control.lock().unwrap().is_some();
            if controlling {
                if let Some(entry) = prepare_chat_line(state, text, false) {
                    send_control(
                        state,
                        ControlMsg::ChatLine { text: entry.text.clone(), name: entry.name.clone(), unix: entry.unix },
                    );
                    emit_chat(cb, &entry, false);
                }
            } else if host_attached(state) {
                if let Some(entry) = prepare_chat_line(state, text, true) {
                    state.host_chat.lock().unwrap().push(entry.clone());
                    state.session_wake.notify_waiters();
                    emit_chat(cb, &entry, false);
                }
            }
        }
        "paste" => {
            let text = cmd.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let bytes = clip_text(text);
            if !bytes.is_empty() {
                let len = bytes.len() as u32;
                send_control(state, ControlMsg::Clipboard(Clipboard { kind: ClipKind::Text, bytes, byte_len: len }));
            }
        }
        "shutdown" => state.app_stop.store(true, Ordering::SeqCst),
        _ => {}
    }
}

fn send_control(state: &State, msg: ControlMsg) {
    if let Some(tx) = state.control.lock().unwrap().as_ref() {
        let _ = tx.send(msg);
    }
}

/// Preset names, plus Best/Speed aliases and a custom fps/bitrate.
fn stream_quality(cmd: &Value) -> Option<StreamQuality> {
    let name = cmd.get("name").and_then(|v| v.as_str()).unwrap_or("balanced");
    if let Some(q) = quality::preset(name) {
        return Some(q);
    }
    match name {
        "best" => quality::preset("high"),
        "speed" => quality::preset("smooth"),
        "custom" => {
            let fps = cmd.get("fps").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
            let bitrate = cmd.get("bitrate").and_then(|v| v.as_u64()).unwrap_or(6_000) as u32;
            let max_width = cmd.get("maxWidth").and_then(|v| v.as_u64()).unwrap_or(1920) as u32;
            Some(StreamQuality { fps, bitrate_kbps: bitrate, max_width }.clamp())
        }
        _ => None,
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
        9 => SpecialCommand::Notifications,
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

fn mouse_button(name: &str) -> pjdesk_proto::peer::MouseButton {
    match name {
        "right" => pjdesk_proto::peer::MouseButton::Right,
        "middle" => pjdesk_proto::peer::MouseButton::Middle,
        _ => pjdesk_proto::peer::MouseButton::Left,
    }
}

fn clip_text(text: &str) -> Vec<u8> {
    let mut end = text.len().min(MAX_CLIP_TEXT);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.as_bytes()[..end].to_vec()
}

fn mouse_from(cmd: &Value) -> Option<InputEvent> {
    let kind = cmd.get("kind").and_then(|v| v.as_str())?;
    let x = cmd.get("x").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let y = cmd.get("y").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let button = mouse_button(cmd.get("button").and_then(|v| v.as_str()).unwrap_or("left"));
    Some(match kind {
        "move" => InputEvent::MouseMove { x, y },
        "down" => InputEvent::MouseButton { button, down: true },
        "up" => InputEvent::MouseButton { button, down: false },
        "wheel" => {
            let dy = cmd.get("dy").and_then(|v| v.as_i64()).unwrap_or(0).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
            InputEvent::Wheel { dx: 0, dy }
        }
        _ => return None,
    })
}

fn merged_mine(extra: &[String]) -> Vec<Ipv4Addr> {
    let mut mine = pjdesk_proto::discover::private_ipv4s();
    for raw in extra {
        let Ok(ip) = raw.trim().parse::<Ipv4Addr>() else { continue };
        if ip.is_loopback() || ip.is_unspecified() || mine.contains(&ip) {
            continue;
        }
        mine.push(ip);
    }
    mine
}

fn split_lan(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn stored_routes(disk: &Disk) -> pjdesk_proto::routes::ServerRoutes {
    serde_json::from_str(&disk.routes).unwrap_or_else(|_| {
        pjdesk_proto::routes::ServerRoutes::from_legacy(&disk.server, &split_lan(&disk.lan), &disk.public_ip, &disk.standby, &[], "")
    })
}

fn routes_from_command(cmd: &Value) -> pjdesk_proto::routes::ServerRoutes {
    if let Some(text) = cmd.get("routes").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(parsed) = serde_json::from_str::<pjdesk_proto::routes::ServerRoutes>(text) {
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    let address = cmd.get("address").and_then(|v| v.as_str()).unwrap_or("");
    let lan = cmd
        .get("lan")
        .and_then(|v| v.as_array())
        .map(|list| list.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    let public_ip = cmd.get("publicIp").and_then(|v| v.as_str()).unwrap_or("");
    let standby = cmd.get("standby").and_then(|v| v.as_str()).unwrap_or("");
    let standby_lan = cmd
        .get("standbyLan")
        .and_then(|v| v.as_str())
        .map(split_lan)
        .or_else(|| {
            cmd.get("standbyLan").and_then(|v| v.as_array()).map(|list| {
                list.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
            })
        })
        .unwrap_or_default();
    let standby_public = cmd.get("standbyPublicIp").and_then(|v| v.as_str()).unwrap_or("");
    pjdesk_proto::routes::ServerRoutes::from_legacy(address, &lan, public_ip, standby, &standby_lan, standby_public)
}

async fn open_ordered(state: &State, cb: &Arc<dyn Callbacks>, pinned: Option<pjdesk_proto::tls::Fingerprint>) -> Result<SignalClient> {
    let routes = state.routes.lock().unwrap().clone();
    let saved = state.server.lock().unwrap().clone();
    let told = std::mem::take(&mut *state.move_primary.lock().unwrap());
    let extra = state.phone_mine.lock().unwrap().clone();
    let mine = tokio::task::spawn_blocking(move || merged_mine(&extra)).await.unwrap_or_default();
    let targets = pjdesk_proto::routes::registration_targets(&told, &routes, &mine, &saved);
    if targets.is_empty() {
        bail!("This phone has no server address yet. Sign in again, or set the server in Settings.");
    }
    let mut attempts = Vec::new();
    for target in &targets {
        log_line(cb, format!("dialing {target}"));
        match SignalClient::connect_endpoint(target, pinned, pjdesk_proto::routes::dial_budget(target)).await {
            Ok(signal) => {
                log_line(cb, format!("signaling via {} {} (TCP 21200)", signal.via, signal.server_addr));
                pjdesk_proto::routes::note_answer(&mut state.routes.lock().unwrap(), target);
                persist(state);
                return Ok(signal);
            }
            Err(err) => {
                let why = err.to_string();
                let why = why.lines().next().unwrap_or("failed").trim();
                log_line(cb, format!("registration route {target} failed: {why}"));
                pjdesk_proto::routes::note_timeout(&mut state.routes.lock().unwrap(), target);
                persist(state);
                attempts.push((target.clone(), why.to_string()));
            }
        }
    }
    let pairs: Vec<(&str, &str)> = attempts.iter().map(|(addr, err)| (addr.as_str(), err.as_str())).collect();
    let report = pjdesk_proto::routes::registration_miss(&pairs);
    log_line(cb, report.clone());
    Err(anyhow!(report))
}

/// TCP 21200 to the account's LAN address and the public address, the same race Windows uses.
#[allow(dead_code)]
async fn open_signal(
    state: &State,
    cb: &Arc<dyn Callbacks>,
    server: &ServerAddr,
    pinned: Option<pjdesk_proto::tls::Fingerprint>,
    public_only: bool,
) -> Result<SignalClient> {
    let lan = state.lan.lock().unwrap().clone();
    let published = state.public_ip.lock().unwrap().clone();
    let mine = state.my_public_ip.lock().unwrap().clone();
    let public = server.to_string();
    log_line(cb, format!("looking up {public}"));
    if !public_only && !lan.is_empty() {
        log_line(cb, format!("trying the server on LAN ({}) and {public}", lan.join(", ")));
    }
    match SignalClient::connect_reaching(server, pinned, &lan, &published, &mine, public_only).await {
        Ok(signal) => {
            log_line(cb, format!("signaling via {} {} (TCP 21200)", signal.via, signal.server_addr));
            emit(
                cb,
                &json!({
                    "event": "signal",
                    "via": signal.via,
                    "reached": signal.server_addr.to_string(),
                    "error": "",
                    "lan": lan.join(", "),
                    "public": public,
                }),
            );
            Ok(signal)
        }
        Err(e) => {
            let text = format!("{e:#}");
            log_line(cb, format!("signaling failed: {text}"));
            emit(cb, &json!({"event":"serverLink","up": false}));
            emit(
                cb,
                &json!({
                    "event": "signal",
                    "via": "",
                    "reached": "",
                    "error": text,
                    "lan": lan.join(", "),
                    "public": public,
                }),
            );
            Err(e)
        }
    }
}

enum PhoneHost {
    Stop,
    Again,
    /// Sign-in asked for a new registration now, without the long outage backoff.
    Soon,
    Lost,
}

async fn host_loop(state: Arc<State>, cb: Arc<dyn Callbacks>) {
    let mut wait = Duration::from_secs(1);
    let mut public_only = false;
    let mut use_standby = false;
    let mut fast_tries = 0u32;
    while !state.app_stop.load(Ordering::SeqCst) {
        let mut advance = false;
        let has_standby = !state.standby.lock().unwrap().trim().is_empty();
        match host_once(state.clone(), cb.clone(), public_only, use_standby).await {
            Ok(PhoneHost::Stop) => break,
            Ok(PhoneHost::Again) => {
                public_only = false;
                fast_tries = 0;
                wait = Duration::from_secs(1);
            }
            Ok(PhoneHost::Soon) => {
                public_only = false;
                fast_tries = 0;
                wait = Duration::from_millis(200);
                advance = false;
            }
            Ok(PhoneHost::Lost) => {
                emit(&cb, &json!({"event":"serverLink","up": false}));
                if state.host_live.load(Ordering::SeqCst) != 0 {
                    log_line(
                        &cb,
                        "rendezvous connection lost; a direct, LAN, or hole-punched session keeps running. A relayed session reconnects on its own.",
                    );
                }
                let hop = pjdesk_proto::failover::next_register(
                    pjdesk_proto::failover::RegFail::Unreachable,
                    use_standby,
                    has_standby,
                    fast_tries,
                    wait,
                );
                if !hop.log.is_empty() {
                    let next = if hop.use_standby {
                        state.standby.lock().unwrap().clone()
                    } else {
                        state.server.lock().unwrap().clone()
                    };
                    log_line(&cb, format!("failover: {} {next}", hop.log));
                }
                emit(&cb, &json!({"event":"status","text":"Reconnecting"}));
                use_standby = hop.use_standby;
                public_only = hop.use_standby;
                wait = hop.delay;
                advance = hop.advance_backoff;
                if advance {
                    fast_tries = pjdesk_proto::failover::FAST_REGISTER_TRIES;
                } else {
                    fast_tries = fast_tries.saturating_add(1);
                }
            }
            Err(e) => {
                let full = format!("{e:#}");
                let mismatch = full.contains("SERVER KEY MISMATCH");
                let head = full.lines().next().unwrap_or("could not reach the PJDesk server").trim();
                if mismatch {
                    warn!("host signaling: the server key changed. This phone will keep trying.");
                } else {
                    warn!("host signaling: {full}");
                }
                let kind = if mismatch {
                    pjdesk_proto::failover::RegFail::KeyMismatch
                } else if pjdesk_proto::signal::is_standby_refusal(&full) {
                    pjdesk_proto::failover::RegFail::StillStandby
                } else {
                    pjdesk_proto::failover::RegFail::Unreachable
                };
                let hop = pjdesk_proto::failover::next_register(kind, use_standby, has_standby, fast_tries, wait);
                if !hop.log.is_empty() {
                    let next = if hop.use_standby {
                        state.standby.lock().unwrap().clone()
                    } else {
                        state.server.lock().unwrap().clone()
                    };
                    log_line(&cb, format!("failover: {} {next}", hop.log));
                }
                let shown = if matches!(
                    kind,
                    pjdesk_proto::failover::RegFail::Unreachable
                        | pjdesk_proto::failover::RegFail::StillStandby
                        | pjdesk_proto::failover::RegFail::KeyMismatch
                ) {
                    "Reconnecting"
                } else {
                    head
                };
                emit(&cb, &json!({"event":"status","text": shown}));
                use_standby = hop.use_standby;
                public_only = hop.use_standby || kind == pjdesk_proto::failover::RegFail::StillStandby;
                wait = hop.delay;
                advance = hop.advance_backoff;
                if kind == pjdesk_proto::failover::RegFail::KeyMismatch {
                    wait = pjdesk_proto::signal::RECONNECT_CAP;
                    fast_tries = pjdesk_proto::failover::FAST_REGISTER_TRIES;
                    advance = false;
                    log_line(&cb, "The server key changed. This phone will keep trying.");
                } else if advance {
                    fast_tries = pjdesk_proto::failover::FAST_REGISTER_TRIES;
                } else {
                    fast_tries = fast_tries.saturating_add(1);
                }
            }
        }
        if state.app_stop.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(pjdesk_proto::signal::jitter_reconnect(wait)).await;
        if advance {
            wait = pjdesk_proto::signal::next_reconnect_delay(wait);
        }
    }
}

async fn host_once(state: Arc<State>, cb: Arc<dyn Callbacks>, public_only: bool, use_standby: bool) -> Result<PhoneHost> {
    let _ = (public_only, use_standby);
    let pinned = state.fingerprint.lock().unwrap().clone().and_then(|s| pjdesk_proto::tls::parse_fingerprint(&s).ok());
    let mut signal = open_ordered(&state, &cb, pinned).await?;
    let server = signal.server_addr.to_string();
    log_line(&cb, format!("registration target (phone): {server}"));
    let extra = state.phone_mine.lock().unwrap().clone();
    let mine = tokio::task::spawn_blocking(move || merged_mine(&extra)).await.unwrap_or_default();
    let routes = state.routes.lock().unwrap().clone();
    if pjdesk_proto::routes::on_subnet(&server, &mine) {
        state.lan_shift.store(false, Ordering::SeqCst);
    } else if !state.lan_shift.load(Ordering::SeqCst) && pjdesk_proto::routes::lan_should_replace(&server, &routes, &mine) {
        state.lan_shift.store(true, Ordering::SeqCst);
        let next = routes.primary_lan.iter().chain(routes.standby_lan.iter()).find(|addr| pjdesk_proto::routes::on_subnet(addr, &mine));
        log_line(
            &cb,
            format!(
                "This phone is on the server's LAN. Reconnecting to {} instead of {server}.",
                next.map(|addr| addr.as_str()).unwrap_or("the LAN address")
            ),
        );
        return Ok(PhoneHost::Again);
    }
    let configured = state.server.lock().unwrap().clone();
    emit(
        &cb,
        &json!({
            "event": "signal",
            "via": signal.via,
            "reached": server,
            "error": "",
            "lan": state.lan.lock().unwrap().join(", "),
            "public": configured,
        }),
    );
    let mut redirected = false;
    if let Some(fp) = signal.observed {
        let text = pjdesk_proto::tls::format_fingerprint(&fp);
        *state.fingerprint.lock().unwrap() = Some(text);
        persist(&state);
        publish(&state, &cb);
    }
    let epoch = state.access_epoch.load(Ordering::SeqCst);
    let key = state.access_key.lock().unwrap().trim().to_string();
    let keyed = !key.is_empty();
    // Register first. A 0.6.52 server heartbeats on that connection and does not
    // answer an account message sent before Register. Sign-in uses this connection
    // after the server has registered the phone.
    let _host_publish = HostPublish { state: state.clone() };
    let token = if keyed {
        key
    } else if state.suppress_token.swap(false, Ordering::SeqCst) {
        String::new()
    } else {
        state.access_token.lock().unwrap().clone()
    };
    let presented = !token.is_empty();
    if presented {
        let _ = signal.send(ClientMsg::PresentAccess { token });
    }
    send_host_register(&mut signal, &state)?;
    let server_ip = signal.server_addr.ip();
    let mut ping = tokio::time::interval(pjdesk_proto::signal::HEARTBEAT);
    ping.tick().await;
    let mut last_heard = Instant::now();
    let mut awaiting_pong = false;
    let mut heartbeats = 0u32;
    let mut routes: HashMap<u64, mpsc::UnboundedSender<ServerMsg>> = HashMap::new();
    let mut saw_registered = false;
    let mut tried_proxy = false;
    let mut report_host = false;
    let mut since_report = 0u32;
    let mut incoming_ready = state.incoming_ready.load(Ordering::SeqCst);
    let mut incoming_told = state.incoming_told.load(Ordering::SeqCst);
    loop {
        if state.app_stop.load(Ordering::SeqCst) {
            return Ok(PhoneHost::Stop);
        }
        if state.leave_primary.swap(false, Ordering::SeqCst) {
            log_line(&cb, "failover: registering with the new primary");
            emit(&cb, &json!({"event":"status","text":"Reconnecting"}));
            return Ok(PhoneHost::Lost);
        }
        if state.sign_in_reregister.swap(false, Ordering::SeqCst) {
            log_line(&cb, format!("sign-in address={server} step=reconnect reply=dialing again now"));
            emit(&cb, &json!({"event":"status","text":"Reconnecting"}));
            return Ok(PhoneHost::Soon);
        }
        tokio::select! {
            _ = state.moved.notified() => {
                state.leave_primary.store(false, Ordering::SeqCst);
                if state.sign_in_reregister.swap(false, Ordering::SeqCst) {
                    log_line(&cb, format!("sign-in address={server} step=reconnect reply=dialing again now"));
                    emit(&cb, &json!({"event":"status","text":"Reconnecting"}));
                    return Ok(PhoneHost::Soon);
                }
                log_line(&cb, "failover: registering with the new primary");
                emit(&cb, &json!({"event":"status","text":"Reconnecting"}));
                return Ok(PhoneHost::Lost);
            }
            _ = ping.tick() => {
                if state.access_epoch.load(Ordering::SeqCst) != epoch {
                    return Ok(PhoneHost::Again);
                }
                if awaiting_pong {
                    log_line(
                        &cb,
                        format!(
                            "heartbeat missed (phone) from {server} via {} ({}); no pong since the last ping. Reconnecting",
                            signal.via, signal.server_addr
                        ),
                    );
                    emit(&cb, &json!({"event":"status","text":"Reconnecting"}));
                    return Ok(PhoneHost::Lost);
                }
                if last_heard.elapsed() >= pjdesk_proto::signal::HEARTBEAT_SILENCE {
                    warn!(
                        "No reply from the server for {} s (phone); reconnecting",
                        pjdesk_proto::signal::HEARTBEAT_SILENCE.as_secs()
                    );
                    emit(&cb, &json!({"event":"status","text":"Reconnecting"}));
                    return Ok(PhoneHost::Lost);
                }
                if state.host_live.load(Ordering::SeqCst) == 0 {
                    let born = state.support_born.load(Ordering::SeqCst);
                    if born > 0 && unix_now().saturating_sub(born) >= 10 * 60 {
                        refresh_support(&state);
                        publish(&state, &cb);
                    }
                }
                if heartbeats == 0 {
                    let _ = send_host_register(&mut signal, &state);
                }
                if let Some((want, password)) = state.incoming_ask.lock().unwrap().clone() {
                    if incoming_ready {
                        state.incoming_ask.lock().unwrap().take();
                        if want {
                            state.incoming_only.store(true, Ordering::SeqCst);
                            persist(&state);
                            emit(&cb, &json!({"event":"incomingOnly","on": true}));
                            let _ = signal.send(ClientMsg::SetIncomingOnly { on: true, password: String::new() });
                        } else if password.is_empty() {
                            emit(
                                &cb,
                                &json!({"event":"error","text": "Enter the account owner's password to allow this computer to connect to others."}),
                            );
                        } else {
                            let _ = signal.send(ClientMsg::SetIncomingOnly { on: false, password });
                        }
                    }
                }
                signal.send(ClientMsg::Ping)?;
                awaiting_pong = true;
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
                    warn!("Lost the connection to the server (phone); reconnecting");
                    if !saw_registered && !state.legacy_register.load(Ordering::SeqCst) {
                        state.legacy_register.store(true, Ordering::SeqCst);
                        log_line(&cb, "this server closed the install-key registration; retrying with the legacy registration");
                    }
                    if presented && !saw_registered && !keyed {
                        state.suppress_token.store(true, Ordering::SeqCst);
                    }
                    return Ok(PhoneHost::Lost);
                };
                last_heard = Instant::now();
                match msg {
                    ServerMsg::Registered { device_id } => {
                        emit(&cb, &json!({"event":"serverLink","up": true}));
                        saw_registered = true;
                        *state.host_tx.lock().unwrap() = Some(signal.sender());
                        *state.host_at.lock().unwrap() = signal.server_addr.to_string();
                        state.host_gen.fetch_add(1, Ordering::SeqCst);
                        *state.device_id.lock().unwrap() = Some(device_id.clone());
                        persist(&state);
                        publish(&state, &cb);
                        let key = state.fingerprint.lock().unwrap().clone().unwrap_or_default();
                        let key = pjdesk_proto::discover::fingerprint_prefix(&key);
                        log_line(
                            &cb,
                            format!(
                                "server ack: registered {} (phone) with {} via {} ({}), key {key}",
                                format_id(&device_id),
                                server,
                                signal.via,
                                signal.server_addr
                            ),
                        );
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
                            if state.support_hold.swap(false, Ordering::SeqCst) {
                                refresh_support(&state);
                                publish(&state, &cb);
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
                    ServerMsg::ServerVersion { version } => {
                        pjdesk_proto::routes::note_server_version(&mut state.routes.lock().unwrap(), &version);
                        persist(&state);
                    }
                    ServerMsg::Routes(incoming) => {
                        if !incoming.is_empty() {
                            let previous = state.routes.lock().unwrap().clone();
                            *state.routes.lock().unwrap() = pjdesk_proto::routes::adopt_advertised(&previous, &incoming);
                            persist(&state);
                        }
                    }
                    ServerMsg::Error { message, .. } => {
                        if !tried_proxy && pjdesk_proto::signal::is_sign_in_required(&message) {
                            tried_proxy = true;
                            log_line(&cb, "registration was refused until this phone is signed in; refreshing the account on this server");
                            if let PhoneRefresh::Token(access) = refresh_phone_access(&mut signal, &state).await {
                                *state.access_token.lock().unwrap() = access.clone();
                                let _ = signal.send(ClientMsg::PresentAccess { token: access });
                                send_host_register(&mut signal, &state)?;
                                continue;
                            }
                            let host = signal.server_addr.ip().to_string();
                            let version = state.routes.lock().unwrap().server_version.clone();
                            let notice = pjdesk_proto::signal::sign_in_required_notice(&host, &version);
                            log_line(&cb, notice.clone());
                            emit(&cb, &json!({"event":"status","text": notice}));
                            continue;
                        }
                        if !redirected && pjdesk_proto::signal::is_standby_refusal(&message) {
                            redirected = true;
                            if let Some((addr, routes)) = pjdesk_proto::signal::parse_standby_redirect(&message) {
                                if !routes.is_empty() {
                                    let previous = state.routes.lock().unwrap().clone();
                                    *state.routes.lock().unwrap() = pjdesk_proto::routes::keep_other_server(&previous, &routes);
                                    persist(&state);
                                }
                                if !addr.is_empty() {
                                    log_line(&cb, format!("standby redirected registration to {addr}"));
                                    signal = SignalClient::connect_endpoint(&addr, pinned, pjdesk_proto::routes::dial_budget(&addr)).await?;
                                    if presented {
                                        let token = if keyed {
                                            state.access_key.lock().unwrap().trim().to_string()
                                        } else {
                                            state.access_token.lock().unwrap().clone()
                                        };
                                        if !token.is_empty() {
                                            let _ = signal.send(ClientMsg::PresentAccess { token });
                                        }
                                    }
                                    send_host_register(&mut signal, &state)?;
                                    continue;
                                }
                            }
                        }
                        if pjdesk_proto::signal::is_standby_refusal(&message) {
                            let key = state.fingerprint.lock().unwrap().clone().unwrap_or_default();
                            let key = pjdesk_proto::discover::fingerprint_prefix(&key);
                            log_line(
                                &cb,
                                format!(
                                    "registration refused by the standby at {} via {} ({}), key {key}; connecting to the primary",
                                    server, signal.via, signal.server_addr
                                ),
                            );
                            bail!("{message}");
                        }
                        if message.contains("account owner's password") {
                            emit(&cb, &json!({"event":"error","text": message}));
                        } else {
                            emit(&cb, &json!({"event":"status","text": message}));
                        }
                    }
                    ServerMsg::WantHostReport => {
                        report_host = true;
                        since_report = 0;
                        let _ = signal.send(host_report(&state));
                        let _ = signal.send(display_name_report(&state));
                    }
                    ServerMsg::Pong => {
                        awaiting_pong = false;
                        heartbeats = heartbeats.saturating_add(1);
                        if heartbeats == 1 || heartbeats % 6 == 0 {
                            log_line(
                                &cb,
                                format!("heartbeat from {server} via {} ({}) (phone)", signal.via, signal.server_addr),
                            );
                        }
                    }
                    ServerMsg::MoveRelay { session_id, primary } => {
                        log_line(&cb, format!("failover: moving the relayed session to the primary {primary}"));
                        *state.move_primary.lock().unwrap() = primary.clone();
                        if routes.contains_key(&session_id) {
                            *state.relay_move.lock().unwrap() = Some((session_id, primary));
                            state.session_wake.notify_waiters();
                        } else {
                            state.leave_primary.store(true, Ordering::SeqCst);
                            state.moved.notify_waiters();
                        }
                    }
                    ServerMsg::Fleet { computers, .. } => {
                        emit(&cb, &json!({"event":"fleet","computers": computers.len()}));
                    }
                    ServerMsg::Release { version, client_url, .. } => {
                        emit(&cb, &json!({"event":"release","version": version, "url": client_url}));
                    }
                    reply @ ServerMsg::AccountResult { .. } => {
                        if !deliver_pending_account(&state, &reply) {
                            emit(&cb, &account_event("", &reply));
                        }
                    }
                    ServerMsg::IncomingOnly { on } => {
                        incoming_ready = true;
                        state.incoming_ready.store(true, Ordering::SeqCst);
                        let local = state.incoming_only.load(Ordering::SeqCst);
                        let (apply_on, tell) = pjdesk_proto::signal::reconcile_incoming_only(local, on, incoming_told);
                        state.incoming_only.store(apply_on, Ordering::SeqCst);
                        if tell {
                            let _ = signal.send(ClientMsg::SetIncomingOnly { on: true, password: String::new() });
                        }
                        incoming_told = true;
                        state.incoming_told.store(true, Ordering::SeqCst);
                        persist(&state);
                        emit(&cb, &json!({"event":"incomingOnly","on": apply_on}));
                        publish(&state, &cb);
                    }
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
    let found =
        tokio::task::spawn_blocking(|| pjdesk_proto::update::check_android(pjdesk_proto::UPDATE_REPO, APP_VERSION))
            .await;
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

fn fleet_from_command(value: Option<&Value>) -> Vec<pjdesk_proto::signal::FleetComputer> {
    let Some(list) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|row| {
            let id = row
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>();
            if id.len() != 9 {
                return None;
            }
            let text = |key: &str| row.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
            Some(pjdesk_proto::signal::FleetComputer {
                id,
                name: text("name"),
                hostname: text("hostname"),
                os_family: text("os_family"),
                os_version: text("os_version"),
                display_name: text("display_name"),
                starred: row.get("starred").and_then(|v| v.as_bool()).unwrap_or(false),
                position: row.get("position").and_then(|v| v.as_i64()).unwrap_or(0),
                hidden: row.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false),
                group: text("group"),
            })
        })
        .collect()
}

/// Drops the heartbeating connection's sender when registration ends.
struct HostPublish {
    state: Arc<State>,
}

impl Drop for HostPublish {
    fn drop(&mut self) {
        *self.state.host_tx.lock().unwrap() = None;
        *self.state.host_at.lock().unwrap() = String::new();
        self.state.pending_account.lock().unwrap().take();
    }
}

/// Where a sign-in is tried. A 0.6.52 server answers on the connection that
/// already registered (the heartbeat). An account message before Register
/// on a new connection can sit unanswered. Register, then the account message,
/// is the fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignInAttempt {
    Heartbeat,
    AccountFirst,
    RegisterThenAccount,
}

fn sign_in_attempts(heartbeat_up: bool) -> &'static [SignInAttempt] {
    if heartbeat_up {
        &[SignInAttempt::Heartbeat, SignInAttempt::AccountFirst, SignInAttempt::RegisterThenAccount]
    } else {
        &[SignInAttempt::AccountFirst, SignInAttempt::RegisterThenAccount]
    }
}

/// How long a sign-in waits on the connection that is already heartbeating.
const HEARTBEAT_SIGN_IN_WAIT: Duration = Duration::from_secs(20);

/// A failed dial. Says "answered" only when the caller received a server message,
/// which this helper never has. Names the address and whether it is LAN or public.
fn sign_in_step_failure(step: &str, address: &str, detail: &str) -> String {
    let address = address.trim();
    let kind = match pjdesk_proto::routes::endpoint_v4(address) {
        Some(ip) if ip.is_private() || ip.is_loopback() || ip.is_link_local() => "LAN",
        Some(_) => "public",
        None => "",
    };
    let place = if address.is_empty() {
        "the server".to_string()
    } else if kind.is_empty() {
        address.to_string()
    } else {
        format!("the {kind} address {address}")
    };
    let detail = detail.trim().replace(['\n', '\r'], " ");
    let detail: String = detail.chars().take(160).collect();
    if detail.is_empty() {
        format!("Could not connect to {place} at step={step}.")
    } else {
        format!("Could not connect to {place} at step={step}. {detail}")
    }
}

async fn wait_fresh_host(
    state: &State,
    generation: u64,
    budget: Duration,
) -> Option<(mpsc::UnboundedSender<ClientMsg>, String)> {
    let deadline = tokio::time::Instant::now() + budget;
    while tokio::time::Instant::now() < deadline {
        if state.host_gen.load(Ordering::SeqCst) != generation {
            let tx = state.host_tx.lock().unwrap().clone();
            let at = state.host_at.lock().unwrap().clone();
            if let Some(tx) = tx {
                if !at.is_empty() {
                    return Some((tx, at));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

fn sign_in_log(cb: &Arc<dyn Callbacks>, address: &str, step: &str, detail: &str) {
    let address = if address.trim().is_empty() { "none" } else { address.trim() };
    let detail = detail.replace(['\n', '\r'], " ");
    log_line(cb, format!("sign-in address={address} step={step} {detail}"));
}

fn reply_note(msg: &ServerMsg) -> String {
    match msg {
        ServerMsg::AccountResult { ok, op, message, .. } => {
            let message: String = message.chars().take(160).collect();
            format!("reply=AccountResult op={op} ok={ok} message={message}")
        }
        ServerMsg::Error { message, .. } => {
            let message: String = message.chars().take(160).collect();
            format!("reply=Error message={message}")
        }
        ServerMsg::Registered { .. } => "reply=Registered".into(),
        ServerMsg::ServerVersion { version } => format!("reply=ServerVersion version={version}"),
        ServerMsg::Routes(_) => "reply=Routes".into(),
        _ => "reply=other".into(),
    }
}

fn deliver_pending_account(state: &State, msg: &ServerMsg) -> bool {
    let op = match msg {
        ServerMsg::AccountResult { op, .. } => op.as_str(),
        _ => return false,
    };
    let mut slot = state.pending_account.lock().unwrap();
    let matches = slot.as_ref().is_some_and(|(want, _)| want == op || op.is_empty() || want.is_empty());
    if !matches {
        return false;
    }
    if let Some((_, tx)) = slot.take() {
        tx.send(msg.clone()).is_ok()
    } else {
        false
    }
}

enum AccountRead {
    Reply(ServerMsg),
    Timeout,
    Closed,
}

async fn await_account(signal: &mut SignalClient, state: &State, cb: &Arc<dyn Callbacks>, address: &str, step: &str, wait: Duration) -> AccountRead {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            sign_in_log(cb, address, step, "reply=timeout");
            return AccountRead::Timeout;
        }
        match tokio::time::timeout(left, signal.recv()).await {
            Ok(Some(ServerMsg::ServerVersion { version })) => {
                pjdesk_proto::routes::note_server_version(&mut state.routes.lock().unwrap(), &version);
                sign_in_log(cb, address, step, &format!("reply=ServerVersion version={version}"));
            }
            Ok(Some(ServerMsg::Routes(_))) => sign_in_log(cb, address, step, "reply=Routes"),
            Ok(Some(ServerMsg::Registered { .. })) => sign_in_log(cb, address, step, "reply=Registered"),
            Ok(Some(reply @ ServerMsg::AccountResult { .. })) | Ok(Some(reply @ ServerMsg::Error { .. })) => {
                sign_in_log(cb, address, step, &reply_note(&reply));
                return AccountRead::Reply(reply);
            }
            Ok(Some(_)) => sign_in_log(cb, address, step, "reply=other"),
            Ok(None) => {
                sign_in_log(cb, address, step, "reply=closed");
                return AccountRead::Closed;
            }
            Err(_) => {
                sign_in_log(cb, address, step, "reply=timeout");
                return AccountRead::Timeout;
            }
        }
    }
}

async fn account_on_address(
    state: &State,
    cb: &Arc<dyn Callbacks>,
    asked: &str,
    prefer: &str,
    msg: ClientMsg,
) -> Result<(ServerMsg, String)> {
    sign_in_log(cb, prefer, "dial-one", &format!("op={asked}"));
    let pinned = state.fingerprint.lock().unwrap().clone().and_then(|text| pjdesk_proto::tls::parse_fingerprint(&text).ok());
    // The saved address may be the public active address. open_ordered walks the
    // route list and puts a same-subnet LAN address first. The fingerprint stays pinned.
    let mut signal = match open_ordered(state, cb, pinned).await {
        Ok(opened) => opened,
        Err(err) => {
            let why = err.to_string();
            let why = why.lines().next().unwrap_or("failed").trim();
            sign_in_log(cb, prefer, "dial-one", &format!("reply=no-transport {why}"));
            bail!("{}", sign_in_step_failure("dial-one", prefer, &why));
        }
    };
    let addr = signal.server_addr.to_string();
    sign_in_log(cb, &addr, "connected", "transport=tcp");
    if signal.send(msg).is_err() {
        sign_in_log(cb, &addr, "dial-one", "reply=send-failed");
        bail!("{}", sign_in_step_failure("dial-one", &addr, "the sign-in was not sent"));
    }
    match await_account(&mut signal, state, cb, &addr, "dial-one", Duration::from_secs(12)).await {
        AccountRead::Reply(reply) => Ok((reply, addr)),
        AccountRead::Timeout | AccountRead::Closed => {
            bail!("{}", sign_in_step_failure("dial-one", &addr, "timed out"));
        }
    }
}

async fn exchange_account(state: &State, cb: &Arc<dyn Callbacks>, asked: &str, msg: ClientMsg) -> Result<(ServerMsg, String)> {
    let heartbeat_addr = state.host_at.lock().unwrap().clone();
    let heartbeat_up = state.host_tx.lock().unwrap().is_some();
    sign_in_log(cb, &heartbeat_addr, "start", &format!("op={asked}"));
    if heartbeat_up {
        let tx = state.host_tx.lock().unwrap().clone();
        let addr = state.host_at.lock().unwrap().clone();
        if let Some(tx) = tx {
            sign_in_log(cb, &addr, "heartbeat", &format!("op={asked}"));
            let (rtx, rrx) = oneshot::channel();
            *state.pending_account.lock().unwrap() = Some((asked.to_string(), rtx));
            if tx.send(msg.clone()).is_err() {
                state.pending_account.lock().unwrap().take();
                sign_in_log(cb, &addr, "heartbeat", "reply=send-failed");
            } else {
                // Stay on this connection. A second dial would race the reply.
                match tokio::time::timeout(HEARTBEAT_SIGN_IN_WAIT, rrx).await {
                    Ok(Ok(reply)) => {
                        sign_in_log(cb, &addr, "heartbeat", &reply_note(&reply));
                        return Ok((reply, addr));
                    }
                    Ok(Err(_)) => {
                        sign_in_log(cb, &addr, "heartbeat", "reply=closed");
                        bail!("{}", sign_in_step_failure("heartbeat", &addr, "the connection closed"));
                    }
                    Err(_) => {
                        state.pending_account.lock().unwrap().take();
                        sign_in_log(cb, &addr, "heartbeat", "reply=timeout");
                        let generation = state.host_gen.load(Ordering::SeqCst);
                        state.sign_in_reregister.store(true, Ordering::SeqCst);
                        state.moved.notify_waiters();
                        sign_in_log(cb, &addr, "reconnect", "forcing a new connection");
                        match wait_fresh_host(state, generation, Duration::from_secs(15)).await {
                            Some((tx, new_addr)) => {
                                sign_in_log(cb, &new_addr, "heartbeat", "op=retry");
                                let (rtx, rrx) = oneshot::channel();
                                *state.pending_account.lock().unwrap() = Some((asked.to_string(), rtx));
                                if tx.send(msg.clone()).is_err() {
                                    state.pending_account.lock().unwrap().take();
                                    bail!("{}", sign_in_step_failure("heartbeat", &new_addr, "the sign-in was not sent"));
                                }
                                match tokio::time::timeout(HEARTBEAT_SIGN_IN_WAIT, rrx).await {
                                    Ok(Ok(reply)) => {
                                        sign_in_log(cb, &new_addr, "heartbeat", &reply_note(&reply));
                                        return Ok((reply, new_addr));
                                    }
                                    Ok(Err(_)) => {
                                        bail!("{}", sign_in_step_failure("heartbeat", &new_addr, "the connection closed"));
                                    }
                                    Err(_) => {
                                        state.pending_account.lock().unwrap().take();
                                        bail!("{}", sign_in_step_failure("heartbeat", &new_addr, "no answer within 20s"));
                                    }
                                }
                            }
                            None => bail!("{}", sign_in_step_failure("reconnect", &addr, "the new connection did not come up")),
                        }
                    }
                }
            }
        } else {
            sign_in_log(cb, &addr, "heartbeat", "reply=no-connection");
        }
    }
    let mut signal: Option<SignalClient> = None;
    for attempt in sign_in_attempts(false) {
        match attempt {
            SignInAttempt::Heartbeat => {}
            SignInAttempt::AccountFirst => {
                if signal.is_none() {
                    sign_in_log(cb, "", "dial", "op=account-before-register");
                    match open_ordered(state, cb, None).await {
                        Ok(opened) => {
                            let addr = opened.server_addr.to_string();
                            sign_in_log(cb, &addr, "connected", "transport=tcp");
                            signal = Some(opened);
                        }
                        Err(err) => {
                            let why = err.to_string();
                            let why = why.lines().next().unwrap_or("failed").trim();
                            sign_in_log(cb, "", "dial", &format!("reply=no-transport {why}"));
                            bail!("{}", sign_in_step_failure("dial", "", &why));
                        }
                    }
                }
                let Some(opened) = signal.as_mut() else { continue };
                let addr = opened.server_addr.to_string();
                sign_in_log(cb, &addr, "account-before-register", &format!("op={asked}"));
                if opened.send(msg.clone()).is_err() {
                    sign_in_log(cb, &addr, "account-before-register", "reply=send-failed");
                    signal = None;
                    continue;
                }
                match await_account(opened, state, cb, &addr, "account-before-register", Duration::from_secs(4)).await {
                    AccountRead::Reply(reply) => return Ok((reply, addr)),
                    AccountRead::Timeout => sign_in_log(cb, &addr, "account-before-register", "reply=timeout"),
                    AccountRead::Closed => signal = None,
                }
            }
            SignInAttempt::RegisterThenAccount => {
                if signal.is_none() {
                    sign_in_log(cb, "", "dial", "op=register-then-account");
                    match open_ordered(state, cb, None).await {
                        Ok(opened) => {
                            let addr = opened.server_addr.to_string();
                            sign_in_log(cb, &addr, "connected", "transport=tcp");
                            signal = Some(opened);
                        }
                        Err(err) => {
                            let why = err.to_string();
                            let why = why.lines().next().unwrap_or("failed").trim();
                            sign_in_log(cb, "", "dial", &format!("reply=no-transport {why}"));
                            bail!("{}", sign_in_step_failure("dial", "", &why));
                        }
                    }
                }
                let Some(opened) = signal.as_mut() else { continue };
                let addr = opened.server_addr.to_string();
                sign_in_log(cb, &addr, "register-then-account", &format!("op={asked}"));
                if send_host_register(opened, state).is_err() {
                    sign_in_log(cb, &addr, "register-then-account", "reply=send-failed");
                    bail!("{}", sign_in_step_failure("register-then-account", &addr, "the register was not sent"));
                }
                state.sign_in_reregister.store(true, Ordering::SeqCst);
                state.moved.notify_waiters();
                let mut registered = false;
                let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                while tokio::time::Instant::now() < deadline {
                    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                    match tokio::time::timeout(left, opened.recv()).await {
                        Ok(Some(ServerMsg::Registered { .. })) => {
                            sign_in_log(cb, &addr, "register-then-account", "reply=Registered");
                            registered = true;
                            break;
                        }
                        Ok(Some(reply @ ServerMsg::AccountResult { .. })) | Ok(Some(reply @ ServerMsg::Error { .. })) => {
                            sign_in_log(cb, &addr, "register-then-account", &reply_note(&reply));
                            return Ok((reply, addr));
                        }
                        Ok(Some(ServerMsg::ServerVersion { version })) => {
                            pjdesk_proto::routes::note_server_version(&mut state.routes.lock().unwrap(), &version);
                            sign_in_log(cb, &addr, "register-then-account", &format!("reply=ServerVersion version={version}"));
                        }
                        Ok(Some(ServerMsg::Routes(_))) => sign_in_log(cb, &addr, "register-then-account", "reply=Routes"),
                        Ok(Some(_)) => sign_in_log(cb, &addr, "register-then-account", "reply=other"),
                        Ok(None) => {
                            sign_in_log(cb, &addr, "register-then-account", "reply=closed");
                            bail!("{}", sign_in_step_failure("register-then-account", &addr, "the connection closed"));
                        }
                        Err(_) => {
                            sign_in_log(cb, &addr, "register-then-account", "reply=timeout");
                            break;
                        }
                    }
                }
                if !registered {
                    sign_in_log(cb, &addr, "register-then-account", "reply=not-registered");
                }
                if opened.send(msg.clone()).is_err() {
                    sign_in_log(cb, &addr, "register-then-account", "reply=send-failed");
                    bail!("{}", sign_in_step_failure("register-then-account", &addr, "the sign-in was not sent"));
                }
                match await_account(opened, state, cb, &addr, "register-then-account", Duration::from_secs(12)).await {
                    AccountRead::Reply(reply) => return Ok((reply, addr)),
                    AccountRead::Timeout | AccountRead::Closed => {
                        bail!("{}", sign_in_step_failure("register-then-account", &addr, "timed out"));
                    }
                }
            }
        }
    }
    bail!("{}", sign_in_step_failure("register-then-account", "", "no answer"))
}

fn account_event(asked: &str, msg: &ServerMsg) -> Value {
    let ServerMsg::AccountResult {
        op,
        ok,
        message,
        access,
        exp,
        refresh,
        email,
        role,
        device_id,
        vault,
        vault_updated,
        app_lock,
        app_lock_updated,
        computers,
        pause_secs,
        active_address,
        active_fingerprint,
        standby_address,
        standby_fingerprint,
        active_lan,
        standby_lan,
        active_public_ip,
        standby_public_ip,
        ..
    } = msg
    else {
        let message = match msg {
            ServerMsg::Error { message, .. } => message.clone(),
            _ => "the server did not answer the sign-in".to_string(),
        };
        return json!({"event":"account","ok": false, "op": asked, "message": message});
    };
    let op = if op.is_empty() { asked.to_string() } else { op.clone() };
    json!({
        "event": "account",
        "ok": ok,
        "op": op,
        "message": message,
        "access": access,
        "exp": exp,
        "refresh": refresh,
        "email": email,
        "role": role,
        "device_id": device_id,
        "vault": vault,
        "vault_updated": vault_updated,
        "app_lock": app_lock,
        "app_lock_updated": app_lock_updated,
        "computers": computers,
        "pause": pause_secs,
        "address": active_address,
        "fingerprint": active_fingerprint,
        "standby": standby_address,
        "standbyFingerprint": standby_fingerprint,
        "lan": active_lan,
        "standbyLan": standby_lan,
        "publicIp": active_public_ip,
        "standbyPublicIp": standby_public_ip,
    })
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

/// True when this session may accept Android's screen-share dialog by itself.
/// The permanent password has to be the one that matched, or the view request
/// has to be preapproved. The one-time password does not.
pub(crate) fn should_auto_share(unattended_access: bool, matched_permanent: bool, trusted_view: bool) -> bool {
    unattended_access && (matched_permanent || trusted_view)
}

/// A view request skips the PJDesk prompt only when unattended access is on.
/// That is also what makes the phone tap Android's screen-share dialog.
/// Eligible viewers are a trusted account, a trusted computer, or another
/// device signed into this phone's own pjdesk.com account.
fn view_is_preapproved(state: &State, ask: &ViewAsk) -> bool {
    if !state.unattended_access.load(Ordering::SeqCst) {
        return false;
    }
    let permanent = state.permanent.lock().unwrap().clone().unwrap_or_default();
    if permanent.trim().is_empty() {
        return false;
    }
    let mine = state.account.lock().unwrap().clone();
    let account = ask.account.trim();
    if !account.is_empty() && !mine.trim().is_empty() && account.eq_ignore_ascii_case(mine.trim()) {
        return true;
    }
    if !account.is_empty()
        && state.trusted_accounts.lock().unwrap().iter().any(|saved| saved.eq_ignore_ascii_case(account))
    {
        return true;
    }
    let computer = ask.computer.trim();
    let id = ask.id.trim();
    state.trusted_computers.lock().unwrap().iter().any(|saved| {
        let saved = saved.trim();
        if saved.is_empty() {
            return false;
        }
        (!computer.is_empty() && saved.eq_ignore_ascii_case(computer))
            || (!id.is_empty() && saved.chars().filter(|c| c.is_ascii_digit()).eq(id.chars()))
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
        Err(_) if view => {
            Approval::No(format!("Timed out. No one at {} answered the request to view the screen", state.hostname))
        }
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
    let accepted = accept_authenticated(
        &state,
        &cb,
        &endpoint,
        &passwords,
        &state.hostname,
        server_udp.ip(),
        peer_public,
        &controller_ip,
        &controller_name,
    )
    .await;
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
    let mut auto_share = false;
    let ceiling = if auth.password_slot == ASK_SLOT {
        let who = if ask.view && !auth.peer_name.trim().is_empty() {
            auth.peer_name.clone()
        } else {
            controller_name.clone()
        };
        let bypass = ask.view && view_is_preapproved(&state, &ask);
        let auto = should_auto_share(state.unattended_access.load(Ordering::SeqCst), false, bypass);
        if auto {
            log_line(&cb, format!("view request from {who} is accepted automatically"));
            grant_auth(&mut auth.send, &state.hostname).await?;
            auto_share = true;
            let policy = state.policy.lock().unwrap().clone();
            grant(perms::KEYBOARD | perms::MOUSE, policy.max_permissions, false)
        } else {
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
        }
    } else {
        state.lockout.lock().unwrap().succeed(&source);
        let support = state.support_password.lock().unwrap().clone();
        if admit::matched_support(&passwords, auth.password_slot, Some(support.as_str())) {
            log_line(&cb, "accepting with the support password, without asking");
            grant_auth(&mut auth.send, &state.hostname).await?;
            let policy = state.policy.lock().unwrap().clone();
            admit::support_open(&policy, false)
        } else {
            let matched = admit::matched_permanent(&passwords, auth.password_slot, permanent.as_deref());
            let permanent_set = permanent.as_deref().map(str::trim).is_some_and(|p| !p.is_empty());
            let approval = state.approval.lock().unwrap().clone();
            let policy = state.policy.lock().unwrap().clone();
            match admit::after_password(&approval, matched, permanent_set, &policy, false) {
                Admit::Open(bits) => {
                    auto_share = should_auto_share(state.unattended_access.load(Ordering::SeqCst), matched, false);
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
    // up while the phone shows the consent notification. The viewer is attached
    // from here until this function ends, so a late consent cannot start a capture.
    state.support_hold.store(true, Ordering::SeqCst);
    let host_live = HostLive::enter(&state);
    state.host_chat_peer.store(false, Ordering::SeqCst);
    state.host_chat.lock().unwrap().clear();
    let mut share_deadline = None;
    if !state.capturing.load(Ordering::SeqCst) {
        *state.capture_deny.lock().unwrap() = String::new();
        let _ = write_msg(&mut auth.send, &HostMsg::Notice(SCREEN_WAIT.into())).await;
        emit(&cb, &json!({"event":"needCapture","text": SCREEN_WAIT, "auto": auto_share}));
        log_line(&cb, SCREEN_WAIT.to_string());
        share_deadline = Some(tokio::time::Instant::now() + SHARE_WAIT);
    }
    if share_deadline.is_none() {
        // Already sharing, so there is no dialog. Tell the phone this session
        // was accepted on its own. A dialog that appears later must not reuse this.
        emit(&cb, &json!({"event":"hosting","peer": auth.peer_name, "auto": auto_share}));
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
    let mut control_warned = false;
    let mut consent_ticks = 0u32;
    let _ = deliver_control(&state, &mut control_send, &mut control_warned).await;
    let mut greeted = auth.peer_proto < 3;
    if greeted {
        let _ = send_welcome(&state, &mut control_send, perms::CLASSIC, ceiling, false).await;
        emit(&cb, &json!({"event":"keyframe"}));
    }
    let result = loop {
        let pending_move = state.relay_move.lock().unwrap().clone();
        if let Some((sid, primary)) = pending_move {
            if sid == session_id {
                *state.relay_move.lock().unwrap() = None;
                let _ = write_msg(&mut control_send, &HostMsg::MoveRelay { primary: primary.clone() }).await;
                let _ = signal.send(ClientMsg::RelayMoved { session_id });
                *state.move_primary.lock().unwrap() = primary;
                log_line(&cb, "failover: told the other computer to move; registering with the new primary");
                state.leave_primary.store(true, Ordering::SeqCst);
                state.moved.notify_waiters();
                break Err(anyhow!("relayed session moving to the new primary"));
            }
        }
        if connection.close_reason().is_some() {
            break Err(anyhow!("the other computer closed the session"));
        }
        if state.gen.load(Ordering::SeqCst) != gen || state.app_stop.load(Ordering::SeqCst) {
            let saved = state.end_reason.lock().unwrap().clone();
            let saved = if saved.is_empty() { "stopped".to_string() } else { saved };
            break Err(anyhow!("{saved}"));
        }
        let chat_lines = std::mem::take(&mut *state.host_chat.lock().unwrap());
        if state.host_chat_peer.load(Ordering::SeqCst) {
            for line in chat_lines {
                let _ = write_msg(
                    &mut control_send,
                    &HostMsg::ChatLine { text: line.text, name: line.name, unix: line.unix },
                )
                .await;
            }
        }
        let control_text = state.control_note.lock().unwrap().take();
        if let Some(text) = control_text {
            if text == CONTROL_ON {
                control_warned = false;
            }
            if text == CONTROL_OFF {
                control_warned = true;
            }
            let _ = write_msg(&mut control_send, &HostMsg::Notice(text)).await;
        }
        if let Some(deadline) = share_deadline {
            consent_ticks = consent_ticks.saturating_add(1);
            if consent_ticks % CONSENT_KEEPALIVE_TICKS == 0 {
                // Pong is ignored by the viewer. The packet keeps the idle
                // timeout from closing the session during the minute we wait.
                let _ = write_msg(&mut control_send, &HostMsg::Pong { t_us: 0 }).await;
                log_line(&cb, "still waiting for screen sharing");
            }
            if state.capturing.load(Ordering::SeqCst) {
                share_deadline = None;
                log_line(&cb, "screen sharing is on");
                emit(&cb, &json!({"event":"hosting","peer": auth.peer_name, "auto": false}));
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
        let msg = tokio::select! {
            _ = state.session_wake.notified() => continue,
            msg = tokio::time::timeout(
                idle,
                read_msg::<_, ControlMsg>(&mut control_recv, MAX_CONTROL_FRAME),
            ) => msg,
        };
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
            ControlMsg::Input(event) => {
                if deliver_control(&state, &mut control_send, &mut control_warned).await {
                    forward_input(&cb, &mut adapter, event);
                }
            }
            ControlMsg::Text { chars } => {
                if deliver_control(&state, &mut control_send, &mut control_warned).await {
                    emit(&cb, &json!({"event":"input","kind":"text","chars": chars}));
                }
            }
            ControlMsg::Special(cmd) => {
                if deliver_control(&state, &mut control_send, &mut control_warned).await {
                    emit(&cb, &json!({"event":"input","kind":"special","which": special_index(cmd)}));
                }
            }
            ControlMsg::Clipboard(clip) => {
                if matches!(clip.kind, ClipKind::Text) {
                    let text = String::from_utf8_lossy(&clip.bytes).into_owned();
                    if !text.is_empty() && deliver_control(&state, &mut control_send, &mut control_warned).await {
                        emit(&cb, &json!({"event":"input","kind":"text","chars": text}));
                    }
                }
            }
            ControlMsg::FrameAck { seq } => {
                video_out.acked.store(seq, Ordering::SeqCst);
                video_out.notify.notify_one();
            }
            ControlMsg::RequestKeyframe => emit(&cb, &json!({"event":"keyframe"})),
            ControlMsg::Hello { permissions, quality, .. } => {
                let _ = send_welcome(&state, &mut control_send, permissions, ceiling, true).await;
                if state.host_chat_peer.load(Ordering::SeqCst) {
                    emit(&cb, &json!({"event":"chatReady"}));
                }
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
            ControlMsg::ChatLine { text, name, unix } => {
                if state.host_chat_peer.load(Ordering::SeqCst) {
                    if let Some(text) = pjdesk_proto::peer::chat_line(&text) {
                        let name = pjdesk_proto::peer::chat_name(&name, "Computer");
                        emit_chat(&cb, &ChatEntry { mine: false, name, text, unix }, true);
                    }
                }
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
    // Clear this before the cleanup awaits. A consent result on the main thread
    // must already see that the viewer is gone.
    host_live.end();
    let stopped_sharing = result.as_ref().err().is_some_and(|e| {
        let text = e.to_string().to_ascii_lowercase();
        text.contains("stopped sharing") || text.contains("screen capture stopped")
    });
    if stopped_sharing {
        let _ = write_msg(&mut control_send, &HostMsg::Notice(PHONE_STOPPED.into())).await;
    }
    video_task.abort();
    *state.video.lock().unwrap() = None;
    let sharing = result.as_ref().err().is_some_and(|e| e.to_string().contains("screen sharing"));
    let close_reason: &[u8] = if stopped_sharing {
        PHONE_STOPPED.as_bytes()
    } else if sharing {
        b"screen sharing not allowed"
    } else {
        b"session ended"
    };
    connection.close(0u32.into(), close_reason);
    result
}

/// Shown on the PC when this phone cannot turn mouse and keyboard into taps.
const CONTROL_OFF: &str = "Control not enabled on phone";
/// The accessibility service connected during the session.
const CONTROL_ON: &str = "Control is on";
/// The phone ended MediaProjection. The PC session closes on this sentence.
const PHONE_STOPPED: &str = "The phone stopped sharing";

/// False when the accessibility service is not connected. The PC is told once.
async fn deliver_control(state: &State, send: &mut quinn::SendStream, warned: &mut bool) -> bool {
    if state.control_ready.load(Ordering::SeqCst) {
        return true;
    }
    if !*warned {
        *warned = true;
        let _ = write_msg(send, &HostMsg::Notice(CONTROL_OFF.into())).await;
    }
    false
}

/// Shown to the controller until MediaProjection is producing frames.
const SCREEN_WAIT: &str = "Waiting for the phone to allow screen sharing";
/// How long the host keeps the session while Android's share dialog is up.
const SHARE_WAIT: Duration = Duration::from_secs(60);
/// A write on the control stream inside the 5 second QUIC idle timeout.
const CONSENT_KEEPALIVE_TICKS: u32 = 8;

/// A replaced Connect (`stopped`) or a dismissed password (`cancelled`) is not a failure to show.
fn surfaced_connect_error(raw: &str) -> Option<String> {
    let text = raw.trim();
    let head = text.lines().next().unwrap_or(text).trim();
    if head.is_empty() || head == "stopped" || head == "cancelled" {
        None
    } else {
        Some(head.to_string())
    }
}

/// Turns a transport failure into a sentence the phone can show. The password is never included.
fn explain_connect_error(target: &str, host_name: &str, err: &anyhow::Error) -> String {
    let raw = format!("{err:#}");
    let head = raw.lines().next().unwrap_or(&raw).trim();
    let lower = head.to_ascii_lowercase();
    let who = if host_name.trim().is_empty() { format_id(target) } else { host_name.trim().to_string() };
    if pjdesk_proto::signal::is_sign_in_required(head) {
        return pjdesk_proto::signal::SIGN_IN_EXPIRED.to_string();
    }
    if lower.contains(pjdesk_proto::signal::SIGN_IN_EXPIRED) {
        return pjdesk_proto::signal::SIGN_IN_EXPIRED.to_string();
    }
    if lower.contains("even through the relay") {
        return format!("Could not reach {who}, even through the relay.");
    }
    if lower.contains("no udp reply") {
        return head.to_string();
    }
    if lower.contains("did not finish the relay") || lower.contains("did not finish setting up") {
        return format!("{who} is on the PJDesk server, but it did not finish the relay setup.");
    }
    if lower.contains("did not start a session")
        || lower.contains("did not answer through the pjdesk server")
        || lower.contains("did not respond")
        || lower.contains("is offline, or it did not answer")
    {
        return format!(
            "{who} did not answer through the PJDesk server. This phone reached the server, but that computer did not start a session."
        );
    }
    if lower.contains("is offline") && head.contains(&who) {
        return head.to_string();
    }
    if raw.contains("could not reach the primary") || raw.contains("could not reach the standby") {
        if let Some(text) = public_dial_failure(&raw, &who) {
            return text;
        }
    }
    if lower.contains("server address is empty") || lower.contains("no server address") {
        return "This phone has no server address yet. Sign in again, or set the server in Settings.".into();
    }
    if lower.contains("cannot reach the pjdesk server")
        || lower.contains("timed out connecting to the server")
        || lower.contains("tls handshake")
    {
        return format!("Could not reach the PJDesk server, so {who} could not be connected.");
    }
    head.to_string()
}

/// Names the public address that failed when this phone is not on the server LAN.
fn public_dial_failure(raw: &str, who: &str) -> Option<String> {
    let mut public_addr = None;
    let mut saw_line = false;
    for line in raw.lines().skip(1) {
        let line = line.trim();
        let Some((addr, err)) = line.split_once(": ") else { continue };
        saw_line = true;
        let addr = addr.trim();
        if pjdesk_proto::routes::endpoint_v4(addr).is_some_and(|ip| !ip.is_private()) {
            public_addr = Some((addr.to_string(), err.trim().to_string()));
            break;
        }
    }
    if let Some((addr, err)) = public_addr {
        let why = if err.is_empty() { "it did not answer" } else { err.as_str() };
        return Some(format!(
            "Could not reach the PJDesk server at {addr} ({why}). This phone is not on that server's local network, so {who} could not be connected."
        ));
    }
    if saw_line {
        return Some(format!(
            "This phone is not on the server's local network, and no public address answered, so {who} could not be connected."
        ));
    }
    Some(format!("Could not reach the PJDesk server from this phone, so {who} could not be connected."))
}

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
        if !pw.trim().is_empty() && !pws.contains(&pw) {
            pws.push(pw);
        }
    }
    let support = state.support_password.lock().unwrap().clone();
    if !support.trim().is_empty() && !pws.contains(&support) {
        pws.push(support);
    }
    pws
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn refresh_support(state: &State) {
    *state.support_code.lock().unwrap() = pjdesk_proto::random_digits(8);
    *state.support_password.lock().unwrap() = generate_password();
    state.support_born.store(unix_now(), Ordering::SeqCst);
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
    let mut cap = ceiling & (perms::KEYBOARD | perms::MOUSE);
    if requested & perms::CHAT_PEER != 0 {
        cap |= perms::CHAT;
    }
    let granted = grant(requested, cap, false);
    let (w, h) = *state.capture.lock().unwrap();
    let monitors = vec![MonitorInfo { index: 0, width: w, height: h, primary: true, name: "Screen".into() }];
    write_msg(send, &HostMsg::Welcome { permissions: granted, monitors, proto: PROTOCOL_VERSION }).await?;
    if requested & perms::VISITOR != 0 {
        write_msg(send, &HostMsg::AskVisitor).await?;
    }
    if requested & perms::CHAT_PEER != 0 && pjdesk_proto::perms::has(granted, perms::CHAT) {
        state.host_chat_peer.store(true, Ordering::SeqCst);
        write_msg(send, &HostMsg::ChatReady).await?;
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
/// Account and device name packed into the one name the connect and ask messages carry.
fn presenter(state: &State) -> String {
    let account = state.account.lock().unwrap().trim().to_string();
    let facts = state.facts.lock().unwrap().clone();
    let named = facts.display_name.trim();
    let device = if !named.is_empty() {
        named.to_string()
    } else if !facts.hostname.trim().is_empty() {
        facts.hostname.trim().to_string()
    } else {
        state.hostname.clone()
    };
    let device: String = device.chars().filter(|c| !c.is_control()).take(64).collect();
    if account.contains('@') {
        format!("{account} — {device}").chars().take(120).collect()
    } else {
        device
    }
}

fn emit_chat(cb: &Arc<dyn Callbacks>, entry: &ChatEntry, alert: bool) {
    emit(
        cb,
        &json!({
            "event": "chat",
            "mine": entry.mine,
            "name": entry.name,
            "text": entry.text,
            "unix": entry.unix,
            "alert": alert,
        }),
    );
}

/// None until the other computer has shown it speaks 0.6.50 chat, or the text is empty.
fn prepare_chat_line(state: &State, text: &str, as_host: bool) -> Option<ChatEntry> {
    let ready = if as_host {
        state.host_chat_peer.load(Ordering::SeqCst)
    } else {
        state.view_chat_peer.load(Ordering::SeqCst)
    };
    if !ready {
        return None;
    }
    let text = pjdesk_proto::peer::chat_line(text)?;
    let name = if as_host { host_chat_name(state) } else { mobile_chat_name(state) };
    Some(ChatEntry { mine: true, name, text, unix: unix_now() })
}

fn phone_computer_name(state: &State) -> String {
    let facts = state.facts.lock().unwrap().clone();
    if !facts.display_name.trim().is_empty() {
        facts.display_name.trim().to_string()
    } else if !facts.hostname.trim().is_empty() {
        facts.hostname.trim().to_string()
    } else {
        state.hostname.clone()
    }
}

/// Account when this phone is signed in, otherwise its computer name.
fn mobile_chat_name(state: &State) -> String {
    let account = state.account.lock().unwrap().trim().to_string();
    let computer = phone_computer_name(state);
    let picked = if account.contains('@') { account } else { computer.clone() };
    pjdesk_proto::peer::chat_name(&picked, &computer)
}

/// The phone's computer name. A host chat line does not use the account email.
fn host_chat_name(state: &State) -> String {
    let computer = phone_computer_name(state);
    pjdesk_proto::peer::chat_name(&computer, "Android")
}

fn android_visitor(state: &State) -> ControlMsg {
    let account = state.account.lock().unwrap().clone();
    let facts = state.facts.lock().unwrap().clone();
    let named = facts.display_name.trim();
    let computer = if !named.is_empty() {
        named.to_string()
    } else if !facts.hostname.trim().is_empty() {
        facts.hostname.clone()
    } else {
        state.hostname.clone()
    };
    let id = state.device_id.lock().unwrap().clone().unwrap_or_default();
    ControlMsg::Visitor { account, computer, user: facts.console_user, id }
}

fn ask_sentence(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    let lower = text.to_ascii_lowercase();
    if lower.contains("no one is available") {
        "No one is available to accept".into()
    } else if lower.contains("timed out") || lower.contains("did not answer") || lower.contains("no answer") {
        "No answer".into()
    } else {
        "Declined".into()
    }
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
        SpecialCommand::Notifications => 9,
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
    view_mark: u64,
    ask_now: bool,
    prefer_standby: bool,
    established: bool,
) -> Result<()> {
    *state.end_reason.lock().unwrap() = String::new();
    let primary = state.server.lock().unwrap().clone();
    let standby = state.standby.lock().unwrap().clone();
    let _ = (primary, standby, prefer_standby);
    let pinned = state.fingerprint.lock().unwrap().clone().and_then(|s| pjdesk_proto::tls::parse_fingerprint(&s).ok());
    let mut signal = open_ordered(&state, &cb, pinned).await?;
    let mut followed_redirect = false;
    if let Some(fp) = signal.observed {
        *state.fingerprint.lock().unwrap() = Some(pjdesk_proto::tls::format_fingerprint(&fp));
        persist(&state);
    }
    let key = state.access_key.lock().unwrap().trim().to_string();
    let mut access = if !key.is_empty() { key } else { state.access_token.lock().unwrap().clone() };
    let mut tried_refresh = false;
    if !access.is_empty() {
        let _ = signal.send(ClientMsg::PresentAccess { token: access.clone() });
    }
    let label = presenter(&state);
    signal.send(ClientMsg::Connect {
        proto: PROTOCOL_VERSION,
        target_id: target.into(),
        controller_name: label.clone(),
    })?;
    let (session_id, token, udp_port, host_name) = loop {
        if state.gen.load(Ordering::SeqCst) != gen || state.view_gen.load(Ordering::SeqCst) != view_mark {
            bail!("stopped");
        }
        match tokio::time::timeout(Duration::from_secs(90), signal.recv()).await {
            Ok(Some(ServerMsg::SessionReady { session_id, token, udp_port, host_name })) => {
                *state.move_primary.lock().unwrap() = String::new();
                log_line(&cb, format!("the other computer accepted, session {session_id:x}"));
                break (session_id, token, udp_port, host_name);
            }
            Ok(Some(ServerMsg::Error { message, .. }))
                if !followed_redirect && pjdesk_proto::signal::is_standby_refusal(&message) =>
            {
                followed_redirect = true;
                let Some((addr, routes)) = pjdesk_proto::signal::parse_standby_redirect(&message) else {
                    bail!("{message}");
                };
                if !routes.is_empty() {
                    let previous = state.routes.lock().unwrap().clone();
                    *state.routes.lock().unwrap() = pjdesk_proto::routes::keep_other_server(&previous, &routes);
                    persist(&state);
                }
                if addr.is_empty() {
                    bail!("{message}");
                }
                log_line(&cb, format!("standby redirected the session to {addr}"));
                signal = SignalClient::connect_endpoint(&addr, pinned, pjdesk_proto::routes::dial_budget(&addr)).await?;
                if !access.is_empty() {
                    let _ = signal.send(ClientMsg::PresentAccess { token: access.clone() });
                }
                signal.send(ClientMsg::Connect {
                    proto: PROTOCOL_VERSION,
                    target_id: target.into(),
                    controller_name: label.clone(),
                })?;
                continue;
            }
            Ok(Some(ServerMsg::Error { message, .. }))
                if !tried_refresh && pjdesk_proto::signal::is_sign_in_required(&message) =>
            {
                tried_refresh = true;
                log_line(
                    &cb,
                    "the server requires a signed-in account; refreshing this phone's session on this connection",
                );
                if let PhoneRefresh::Token(fresh) = refresh_phone_access(&mut signal, &state).await {
                    *state.access_token.lock().unwrap() = fresh.clone();
                    persist(&state);
                    access = fresh;
                    let _ = signal.send(ClientMsg::PresentAccess { token: access.clone() });
                    signal.send(ClientMsg::Connect {
                        proto: PROTOCOL_VERSION,
                        target_id: target.into(),
                        controller_name: label.clone(),
                    })?;
                    emit(&cb, &json!({"event":"accountSession","state":"valid"}));
                    continue;
                }
                emit(&cb, &json!({"event":"accountSession","state":"expired"}));
                bail!("{}", pjdesk_proto::signal::SIGN_IN_EXPIRED);
            }
            Ok(Some(ServerMsg::Error { message, .. })) if pjdesk_proto::signal::is_sign_in_required(&message) => {
                emit(&cb, &json!({"event":"accountSession","state":"expired"}));
                bail!("{}", pjdesk_proto::signal::SIGN_IN_EXPIRED);
            }
            Ok(Some(ServerMsg::Error { message, .. })) => bail!("{message}"),
            Ok(Some(_)) => continue,
            Ok(None) => bail!("the server closed the connection"),
            Err(_) => bail!("the PJDesk server did not start a session"),
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
            Err(_) => bail!("the computer did not finish the relay setup"),
        }
    };
    drop(signal);
    let endpoint = p2p::controller_endpoint(binding.socket, fingerprint)?;
    let extra = state.phone_mine.lock().unwrap().clone();
    let mine = tokio::task::spawn_blocking(move || merged_mine(&extra)).await.unwrap_or_default();
    let direct = p2p::candidates(peer_public, &peer_local);
    let candidates = p2p::candidates_from(peer_public, &peer_local, &mine);
    if candidates.len() != direct.len() {
        log_line(
            &cb,
            "This phone is not on that computer's network. Trying its public address, then the relay.",
        );
    }
    let mut password = if ask_now { String::new() } else { password.to_string() };
    let mut from_store = !password.is_empty();
    let mut rejected = false;
    let mut note = String::new();
    let mut ask_person = ask_now;
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
        let punching = binding
            .punch_socket
            .try_clone()
            .ok()
            .map(|sock| p2p::spawn_punching(sock, candidates.clone(), session_id, Duration::from_secs(8)));
        log_line(&cb, "Trying a direct connection on this network first (UDP 21201), then the relay");
        let connected = p2p::connect_to_host(&endpoint, &candidates, server_udp, false).await;
        if let Some(punching) = punching {
            punching.abort();
        }
        let (connection, path) = match connected {
            Ok(pair) => pair,
            Err(e) => {
                let text = explain_connect_error(target, &host_name, &e);
                log_line(&cb, format!("connect failed: {text}"));
                return Err(anyhow::Error::msg(text));
            }
        };
        let authed = if ask_person {
            let who = if host_name.trim().is_empty() { format_id(target) } else { host_name.clone() };
            emit(&cb, &json!({"event":"status","text": format!("Waiting for {who} to accept…")}));
            let asked = session::controller_ask_person_for(&connection, &label, Duration::from_secs(60));
            tokio::pin!(asked);
            let result = loop {
                tokio::select! {
                    result = &mut asked => break result,
                    _ = tokio::time::sleep(Duration::from_millis(150)) => {
                        if state.view_gen.load(Ordering::SeqCst) != view_mark || state.gen.load(Ordering::SeqCst) != gen {
                            connection.close(0u32.into(), b"viewer closed");
                            bail!("cancelled");
                        }
                    }
                }
            };
            match result {
                Ok(auth) => Ok(auth),
                Err(e) if e.is::<LockedOut>() => Err(e),
                Err(e) => Err(anyhow::Error::msg(ask_sentence(&e))),
            }
        } else {
            session::controller_auth(&connection, &password, &label).await
        };
        match authed {
            Ok(auth) => break (connection, path, auth),
            Err(e) if e.is::<LockedOut>() => {
                connection.close(1u32.into(), b"locked out");
                log_line(&cb, format!("connect locked out: {e}"));
                bail!(e.to_string());
            }
            Err(e) if e.is::<WrongPassword>() => {
                connection.close(1u32.into(), b"authentication failed");
                log_line(&cb, format!("incorrect password for {} (saved cleared: {from_store})", format_id(target)));
                if established {
                    bail!("incorrect password");
                }
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
    let _relayed = matches!(path, LinkPath::Relay(_));
    let path = match path {
        LinkPath::Direct(addr) if p2p::is_lan_ip(addr.ip()) => "LAN",
        LinkPath::Direct(_) => "Internet",
        LinkPath::Relay(_) => "Internet, relayed",
    };
    log_line(&cb, format!("controlling {peer} via {path}"));
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
    *state.viewer.lock().unwrap() = Some(connection.clone());
    let watch_conn = connection.clone();
    let watch_state = state.clone();
    let watcher = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(150)).await;
            if watch_state.view_gen.load(Ordering::SeqCst) != view_mark
                || watch_state.gen.load(Ordering::SeqCst) != gen
                || watch_state.app_stop.load(Ordering::SeqCst)
            {
                watch_conn.close(0u32.into(), b"viewer closed");
                break;
            }
        }
    });
    state.view_chat_peer.store(false, Ordering::SeqCst);
    if auth.peer_proto >= 3 {
        let _ = control_tx.send(ControlMsg::Hello {
            permissions: perms::ALL | perms::LAYOUT | perms::VISITOR | perms::CHAT_PEER,
            monitor: 0,
            quality: quality::preset("balanced").unwrap_or(StreamQuality::default()),
        });
        let _ = control_tx.send(ControlMsg::SetKeyboardMode(KeyboardMode::Translate));
        let index = state.view_monitor.load(Ordering::SeqCst);
        let layout = state.view_layout.load(Ordering::SeqCst);
        if layout {
            let _ = control_tx.send(ControlMsg::View { mode: VIEW_SINGLE, display: index });
        } else if index > 0 {
            let _ = control_tx.send(ControlMsg::SelectMonitor { index });
        }
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
        if state.gen.load(Ordering::SeqCst) != gen
            || state.view_gen.load(Ordering::SeqCst) != view_mark
            || state.app_stop.load(Ordering::SeqCst)
        {
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
            Ok(HostMsg::MoveRelay { primary }) => {
                *state.move_primary.lock().unwrap() = primary.clone();
                log_line(&cb, format!("failover: moving this relayed session to the primary {primary}"));
                break pjdesk_proto::failover::relay_resume_message(&primary);
            }
            Ok(HostMsg::AskVisitor) => {
                let _ = control_tx.send(android_visitor(&state));
            }
            Ok(HostMsg::ChatReady) => {
                state.view_chat_peer.store(true, Ordering::SeqCst);
                emit(&cb, &json!({"event":"chatReady"}));
            }
            Ok(HostMsg::ChatLine { text, name, unix }) => {
                state.view_chat_peer.store(true, Ordering::SeqCst);
                if let Some(text) = pjdesk_proto::peer::chat_line(&text) {
                    let name = pjdesk_proto::peer::chat_name(&name, &peer);
                    emit_chat(&cb, &ChatEntry { mine: false, name, text, unix }, true);
                }
            }
            Ok(HostMsg::Chat { text }) => {
                if let Some(text) = pjdesk_proto::peer::chat_line(&text) {
                    emit_chat(&cb, &ChatEntry { mine: false, name: peer.clone(), text, unix: unix_now() }, true);
                }
            }
            Ok(HostMsg::Denied { what, reason }) => {
                emit(&cb, &json!({"event":"status","text": format!("{what}: {reason}")}))
            }
            Ok(HostMsg::Pong { t_us }) => emit(&cb, &json!({"event":"pong","t_us": t_us})),
            Ok(HostMsg::Privacy { enabled }) => emit(&cb, &json!({"event":"privacy","enabled": enabled})),
            Ok(HostMsg::Restarting) => emit(&cb, &json!({"event":"status","text": "The other computer is restarting"})),
            Ok(HostMsg::Clipboard(clip)) => {
                if matches!(clip.kind, ClipKind::Text) {
                    if let Ok(text) = String::from_utf8(clip.bytes) {
                        emit(&cb, &json!({"event":"clipboard","text": text}));
                    }
                }
            }
            Ok(HostMsg::File(msg)) => match msg {
                FileMsg::Entries { names, .. } => emit(&cb, &json!({"event":"files","names": names})),
                FileMsg::Accept { id, resume_from } => {
                    let job = state.files.lock().unwrap().get(&id).cloned();
                    let conn = state.viewer.lock().unwrap().clone();
                    if let (Some(job), Some(conn)) = (job, conn) {
                        let cb_file = cb.clone();
                        tokio::spawn(async move {
                            match send_file_bytes(conn, id, &job.path, resume_from).await {
                                Ok(()) => {
                                    emit(&cb_file, &json!({"event":"status","text": format!("Sent {}", job.name)}))
                                }
                                Err(e) => emit(
                                    &cb_file,
                                    &json!({"event":"status","text": format!("Could not send the file: {e}")}),
                                ),
                            }
                        });
                    }
                }
                FileMsg::Reject { reason, .. } | FileMsg::Error { reason, .. } => {
                    emit(&cb, &json!({"event":"status","text": reason}))
                }
                _ => {}
            },
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
    watcher.abort();
    *state.control.lock().unwrap() = None;
    *state.viewer.lock().unwrap() = None;
    state.files.lock().unwrap().clear();
    connection.close(0u32.into(), b"viewer closed");
    let still_watching = state.view_gen.load(Ordering::SeqCst) == view_mark
        && state.gen.load(Ordering::SeqCst) == gen
        && !state.app_stop.load(Ordering::SeqCst);
    if !still_watching {
        return Ok(());
    }
    if let Some(plain) = pjdesk_proto::failover::session_refusal(&end_reason) {
        log_line(&cb, format!("controller session refused: {plain}"));
        emit(&cb, &json!({"event":"disconnected","role":"controller","reason": plain}));
        return Ok(());
    }
    if pjdesk_proto::failover::is_relay_resume(&end_reason) {
        log_line(&cb, end_reason.clone());
        bail!(end_reason);
    }
    log_line(&cb, format!("controller session lost: {end_reason}"));
    bail!("session lost: {end_reason}");
}

async fn send_file_bytes(conn: quinn::Connection, id: u64, path: &Path, resume: u64) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(resume))?;
    let mut uni = conn.open_uni().await?;
    write_side_header(&mut uni, FILE_MAGIC, &SideHeader { id, offset: resume, a: 0, b: 0 }).await?;
    let mut buf = vec![0u8; pjdesk_proto::peer::SIDE_CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        write_side_chunk(&mut uni, &buf[..n]).await?;
    }
    write_side_chunk(&mut uni, &[]).await?;
    let _ = uni.finish();
    Ok(())
}

// quinn SendStream write_all is on AsyncWrite.
use tokio::io::AsyncWriteExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_heartbeat_is_not_reported_as_unreachable() {
        assert_eq!(HEARTBEAT_SIGN_IN_WAIT.as_secs(), 20);
        let timed = sign_in_step_failure("heartbeat", "192.168.0.193:21200", "no answer within 20s");
        assert!(timed.contains("step=heartbeat"));
        assert!(timed.contains("192.168.0.193:21200"));
        assert!(timed.contains("LAN"));
        assert!(!timed.contains("answered"));
        let dial = sign_in_step_failure("dial-one", "204.248.122.44:21200", "timed out");
        assert!(dial.contains("step=dial-one"));
        assert!(dial.contains("public"));
        assert!(dial.contains("204.248.122.44:21200"));
        assert!(dial.contains("Could not connect"));
        assert!(!dial.contains("answered"));
        assert!(!dial.to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn custom_quality_is_clamped_and_aliases_map() {
        let q = stream_quality(&json!({"name":"custom","fps":120,"bitrate":10,"maxWidth":100})).unwrap();
        assert_eq!(q.fps, 60);
        assert_eq!(q.bitrate_kbps, 200);
        assert_eq!(q.max_width, 640);
        assert_eq!(stream_quality(&json!({"name":"speed"})).unwrap().fps, 24);
        assert_eq!(stream_quality(&json!({"name":"best"})).unwrap().fps, 60);
        assert!(stream_quality(&json!({"name":"nope"})).is_none());
    }

    #[test]
    fn presenter_names_the_account_and_the_phone() {
        let dir = std::env::temp_dir().join(format!("pjdesk-presenter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = test_state(&dir);
        assert_eq!(presenter(&state), "Pixel");
        state.facts.lock().unwrap().display_name = "Ben Pixel 11".into();
        *state.account.lock().unwrap() = "ben@spader.us".into();
        assert_eq!(presenter(&state), "ben@spader.us — Ben Pixel 11");
        let card = android_visitor(&state);
        match card {
            ControlMsg::Visitor { account, computer, .. } => {
                assert_eq!(account, "ben@spader.us");
                assert_eq!(computer, "Ben Pixel 11");
            }
            _ => panic!("visitor"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_is_refused_until_the_other_computer_is_ready() {
        let dir = std::env::temp_dir().join(format!("pjdesk-chat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = test_state(&dir);
        state.facts.lock().unwrap().display_name = "Ben Pixel 11".into();
        *state.account.lock().unwrap() = "ben@spader.us".into();
        assert!(prepare_chat_line(&state, "hello", false).is_none());
        assert!(prepare_chat_line(&state, "hello", true).is_none());
        state.view_chat_peer.store(true, Ordering::SeqCst);
        state.host_chat_peer.store(true, Ordering::SeqCst);
        let controller = prepare_chat_line(&state, " hello ", false).unwrap();
        assert_eq!(controller.name, "ben@spader.us");
        assert_eq!(controller.text, "hello");
        let host = prepare_chat_line(&state, "here", true).unwrap();
        assert_eq!(host.name, "Ben Pixel 11");
        assert_eq!(pjdesk_proto::peer::CHAT_NEEDS_TIP, "The other computer needs PJDesk 0.6.50 or newer");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
    fn mouse_buttons_follow_the_command() {
        let right = mouse_from(&json!({"kind":"down","button":"right","x":1,"y":2})).unwrap();
        assert!(matches!(
            right,
            InputEvent::MouseButton { button: pjdesk_proto::peer::MouseButton::Right, down: true }
        ));
        let left = mouse_from(&json!({"kind":"up","x":1,"y":2})).unwrap();
        assert!(matches!(left, InputEvent::MouseButton { button: pjdesk_proto::peer::MouseButton::Left, down: false }));
        assert_eq!(clip_text("héllo").len(), "héllo".len());
    }

    #[test]
    fn specials_keep_the_windows_indexes_and_append_android() {
        assert_eq!(special_index(SpecialCommand::CtrlAltDel), 0);
        assert_eq!(special_index(SpecialCommand::Back), 3);
        assert_eq!(special_index(SpecialCommand::Power), 8);
        assert_eq!(special_index(SpecialCommand::Notifications), 9);
        assert!(matches!(special_from(4), Some(SpecialCommand::Home)));
        assert!(matches!(special_from(9), Some(SpecialCommand::Notifications)));
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
            assert_eq!(passwords.len(), 3);
            assert_eq!(passwords[1], saved);
            assert_ne!(passwords[2], saved);
            assert!(!passwords[2].is_empty());
            assert!(host_accepts_typed(&passwords, saved));
            assert!(host_accepts_typed(&passwords, &passwords[0]));
            assert!(!host_accepts_typed(&passwords, "wrong horse battery"));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_ended_viewer_does_not_clear_the_next_session() {
        let dir = std::env::temp_dir().join(format!("pjdesk-host-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = test_state(&dir);
        assert!(!host_attached(&state));
        let first = HostLive::enter(&state);
        assert!(host_attached(&state));
        let second = HostLive::enter(&state);
        drop(first);
        assert!(host_attached(&state), "the older session must not clear the newer one");
        second.end();
        assert!(!host_attached(&state));
        drop(second);
        assert!(!host_attached(&state));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_consent_wait_sends_a_packet_inside_the_idle_timeout() {
        assert_eq!(SHARE_WAIT, Duration::from_secs(60));
        assert!(CONSENT_KEEPALIVE_TICKS * 250 < 5_000);
    }

    #[test]
    fn a_single_app_capture_tells_the_pc_to_keep_waiting() {
        let dir = std::env::temp_dir().join(format!("pjdesk-screen-wait-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let cb: Arc<dyn Callbacks> = Arc::new(Silent);
            let state = test_state(&dir);
            state.capturing.store(true, Ordering::SeqCst);
            apply(&state, &cb, json!({"cmd":"screenWait"})).await;
            assert!(!state.capturing.load(Ordering::SeqCst));
            assert_eq!(state.control_note.lock().unwrap().as_deref(), Some(SCREEN_WAIT));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connect_failures_name_the_relay_the_offline_host_and_a_lockout() {
        assert_eq!(surfaced_connect_error("stopped"), None);
        assert_eq!(surfaced_connect_error("cancelled"), None);
        assert_eq!(surfaced_connect_error(""), None);
        let relay = anyhow::anyhow!("could not reach the host, even through the relay");
        assert_eq!(
            explain_connect_error("902488771", "Cabin Desk", &relay),
            "Could not reach Cabin Desk, even through the relay."
        );
        let offline = anyhow::anyhow!("the host did not respond");
        assert_eq!(
            explain_connect_error("902488771", "", &offline),
            "902 488 771 did not answer through the PJDesk server. This phone reached the server, but that computer did not start a session."
        );
        let expired = anyhow::anyhow!("Sign in required");
        assert_eq!(explain_connect_error("902488771", "Cabin Desk", &expired), "Your sign-in expired");
        let relay_setup = anyhow::anyhow!("the computer did not finish the relay setup");
        assert_eq!(
            explain_connect_error("902488771", "BENLENOVODESK", &relay_setup),
            "BENLENOVODESK is on the PJDesk server, but it did not finish the relay setup."
        );
        let udp = anyhow::anyhow!(
            "no UDP reply from the server at 204.248.122.44:21201. Make sure UDP port 21201 is allowed in the server's firewall"
        );
        assert!(explain_connect_error("902488771", "Cabin Desk", &udp).contains("no UDP reply from the server at 204.248.122.44:21201"));
        let missed = anyhow::anyhow!(
            "could not reach the primary or the standby; retrying. timed out\n192.168.0.20:21200: timed out\n204.248.122.44:21200: connection refused"
        );
        let named = explain_connect_error("902488771", "BENLENOVODESK", &missed);
        assert!(named.contains("204.248.122.44:21200"), "{named}");
        assert!(named.contains("not on that server's local network"), "{named}");
        assert!(named.contains("BENLENOVODESK"), "{named}");
        let locked = anyhow::anyhow!("locked out, try again in 20 minutes");
        assert_eq!(explain_connect_error("902488771", "Cabin Desk", &locked), "locked out, try again in 20 minutes");
        let empty = anyhow::anyhow!("the server address is empty");
        assert_eq!(
            explain_connect_error("902488771", "Cabin Desk", &empty),
            "This phone has no server address yet. Sign in again, or set the server in Settings."
        );
        let down = anyhow::anyhow!("timed out connecting to the server at pjdesk.spader.us:21200");
        assert_eq!(
            explain_connect_error("902488771", "Cabin Desk", &down),
            "Could not reach the PJDesk server, so Cabin Desk could not be connected."
        );
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
    fn a_view_is_preapproved_only_when_unattended_access_is_on() {
        let dir = std::env::temp_dir().join(format!("pjdesk-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = test_state(&dir);
        let ben = ViewAsk {
            view: true,
            account: "ben@spader.us".into(),
            computer: "BENLENOVODESK".into(),
            id: "692996394".into(),
        };
        *state.permanent.lock().unwrap() = Some("long-enough-password".into());
        *state.approval.lock().unwrap() = "unattended".into();
        assert!(!view_is_preapproved(&state, &ben), "the old approval mode does not skip Allow");
        state.unattended_access.store(true, Ordering::SeqCst);
        *state.account.lock().unwrap() = "Ben@Spader.us".into();
        assert!(view_is_preapproved(&state, &ben), "the same signed-in account is eligible");
        *state.account.lock().unwrap() = String::new();
        assert!(!view_is_preapproved(&state, &ben));
        *state.trusted_accounts.lock().unwrap() = vec!["Ben@Spader.us".into()];
        assert!(view_is_preapproved(&state, &ben));
        state.unattended_access.store(false, Ordering::SeqCst);
        assert!(!view_is_preapproved(&state, &ben), "a trusted account still needs the switch");
        state.unattended_access.store(true, Ordering::SeqCst);
        *state.trusted_accounts.lock().unwrap() = vec![];
        *state.trusted_computers.lock().unwrap() = vec!["386 626 882".into()];
        let pixel = ViewAsk { view: true, account: String::new(), computer: "Pixel".into(), id: "386626882".into() };
        assert!(view_is_preapproved(&state, &pixel));
        assert!(!view_is_preapproved(&state, &ben));
        *state.permanent.lock().unwrap() = None;
        assert!(!view_is_preapproved(&state, &pixel), "no permanent password falls back to the prompt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_sync_remembers_the_switch_without_a_password() {
        let dir = std::env::temp_dir().join(format!("pjdesk-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let cb: Arc<dyn Callbacks> = Arc::new(Silent);
            let state = test_state(&dir);
            apply(&state, &cb, json!({"cmd":"setUnattendedAccess","on":true,"sync":true})).await;
            assert!(state.unattended_access.load(Ordering::SeqCst));
            apply(&state, &cb, json!({"cmd":"setUnattendedAccess","on":true})).await;
            assert!(!state.unattended_access.load(Ordering::SeqCst));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unsent_idr_is_not_replaced_by_a_delta() {
        assert!(!should_replace_frame(true, false));
        assert!(should_replace_frame(true, true));
        assert!(should_replace_frame(false, false));
        assert!(should_replace_frame(false, true));
    }

    #[test]
    fn unattended_access_accepts_the_permanent_password_and_a_trusted_view() {
        assert!(!should_auto_share(false, true, false));
        assert!(!should_auto_share(true, false, false));
        assert!(should_auto_share(true, true, false));
        assert!(should_auto_share(true, false, true));
        assert!(!should_auto_share(false, false, true));
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
        let identity =
            TlsIdentity::load_or_create(&dir.join("host.crt"), &dir.join("host.key"), "pjdesk-host").unwrap();
        Arc::new(State {
            dir: dir.to_path_buf(),
            hostname: "Pixel".into(),
            facts: Mutex::new(HostFacts::default()),
            secret: [7u8; 32],
            install_key: [9u8; 32],
            legacy_register: AtomicBool::new(false),
            identity,
            device_id: Mutex::new(None),
            server: Mutex::new("office.example:21200".into()),
            lan: Mutex::new(Vec::new()),
            public_ip: Mutex::new(String::new()),
            standby: Mutex::new(String::new()),
            my_public_ip: Mutex::new(String::new()),
            phone_mine: Mutex::new(Vec::new()),
            lan_shift: AtomicBool::new(false),
            routes: Mutex::new(pjdesk_proto::routes::ServerRoutes::default()),
            fingerprint: Mutex::new(None),
            otp: Mutex::new(generate_password()),
            support_code: Mutex::new(pjdesk_proto::random_digits(8)),
            support_password: Mutex::new(generate_password()),
            support_born: AtomicU64::new(unix_now()),
            support_hold: AtomicBool::new(false),
            permanent: Mutex::new(None),
            approval: Mutex::new("prompt".into()),
            policy: Mutex::new(pjdesk_proto::signal::Policy::default()),
            capture: Mutex::new((1080, 2400)),
            capturing: AtomicBool::new(false),
            unattended_access: AtomicBool::new(false),
            control_ready: AtomicBool::new(false),
            control_note: Mutex::new(None),
            session_wake: Notify::new(),
            capture_deny: Mutex::new(String::new()),
            gen: AtomicU64::new(1),
            view_gen: AtomicU64::new(1),
            view_wake: Notify::new(),
            view_monitor: AtomicU8::new(0),
            view_layout: AtomicBool::new(false),
            approval_tx: Mutex::new(None),
            password_tx: Mutex::new(None),
            lockout: Mutex::new(Lockout::default()),
            end_reason: Mutex::new(String::new()),
            control: Mutex::new(None),
            video: Mutex::new(None),
            app_stop: AtomicBool::new(false),
            access_token: Mutex::new(String::new()),
            account_refresh: Mutex::new(String::new()),
            account_device: Mutex::new(String::new()),
            account: Mutex::new(String::new()),
            trusted_accounts: Mutex::new(Vec::new()),
            trusted_computers: Mutex::new(Vec::new()),
            view_ask: Mutex::new(ViewAsk::default()),
            suppress_token: AtomicBool::new(false),
            access_key: Mutex::new(String::new()),
            access_epoch: AtomicU64::new(0),
            viewer: Mutex::new(None),
            files: Mutex::new(HashMap::new()),
            file_seq: AtomicU64::new(1),
            host_live: AtomicU64::new(0),
            host_live_seq: AtomicU64::new(0),
            move_primary: Mutex::new(String::new()),
            relay_move: Mutex::new(None),
            moved: Notify::new(),
            leave_primary: AtomicBool::new(false),
            host_chat_peer: AtomicBool::new(false),
            view_chat_peer: AtomicBool::new(false),
            host_chat: Mutex::new(Vec::new()),
            incoming_only: AtomicBool::new(false),
            incoming_ready: AtomicBool::new(false),
            incoming_told: AtomicBool::new(false),
            incoming_ask: Mutex::new(None),
            host_tx: Mutex::new(None),
            host_at: Mutex::new(String::new()),
            pending_account: Mutex::new(None),
            sign_in_reregister: AtomicBool::new(false),
            host_gen: AtomicU64::new(0),
        })
    }

    /// A 0.6.52 server that does not answer an account message until the
    /// connection has registered. The heartbeat connection is already registered.
    struct Server0652 {
        registered: bool,
        transport: bool,
        heartbeat: bool,
        steps: Vec<&'static str>,
    }

    impl Server0652 {
        fn answer(&mut self, attempt: SignInAttempt) -> Result<&'static str, &'static str> {
            self.steps.push(match attempt {
                SignInAttempt::Heartbeat => "heartbeat",
                SignInAttempt::AccountFirst => "account-before-register",
                SignInAttempt::RegisterThenAccount => "register-then-account",
            });
            match attempt {
                SignInAttempt::Heartbeat => {
                    if self.heartbeat && self.registered {
                        Ok("signed-in")
                    } else {
                        Err("timeout")
                    }
                }
                SignInAttempt::AccountFirst => {
                    if !self.transport {
                        return Err("no-transport");
                    }
                    if self.registered {
                        Ok("signed-in")
                    } else {
                        Err("timeout")
                    }
                }
                SignInAttempt::RegisterThenAccount => {
                    if !self.transport {
                        return Err("no-transport");
                    }
                    self.registered = true;
                    Ok("signed-in")
                }
            }
        }
    }

    fn run_sign_in(server: &mut Server0652) -> Result<&'static str, &'static str> {
        let mut saw_transport = server.heartbeat;
        for attempt in sign_in_attempts(server.heartbeat) {
            if matches!(attempt, SignInAttempt::AccountFirst | SignInAttempt::RegisterThenAccount) && !server.transport {
                return Err("unreachable");
            }
            if matches!(attempt, SignInAttempt::AccountFirst | SignInAttempt::RegisterThenAccount) {
                saw_transport = true;
            }
            match server.answer(*attempt) {
                Ok(done) => return Ok(done),
                Err("no-transport") => return Err("unreachable"),
                Err(_) => continue,
            }
        }
        if saw_transport {
            Err("no-sign-in-reply")
        } else {
            Err("unreachable")
        }
    }

    #[test]
    fn a_0_6_52_server_signs_in_on_the_heartbeat_connection() {
        let mut live = Server0652 { registered: true, transport: true, heartbeat: true, steps: Vec::new() };
        assert_eq!(run_sign_in(&mut live), Ok("signed-in"));
        assert_eq!(live.steps, vec!["heartbeat"]);
        let mut quiet = Server0652 { registered: false, transport: true, heartbeat: false, steps: Vec::new() };
        assert_eq!(run_sign_in(&mut quiet), Ok("signed-in"));
        assert_eq!(quiet.steps, vec!["account-before-register", "register-then-account"]);
        let mut down = Server0652 { registered: false, transport: false, heartbeat: false, steps: Vec::new() };
        assert_eq!(run_sign_in(&mut down), Err("unreachable"));
        let mut ignored = Server0652 { registered: true, transport: true, heartbeat: true, steps: Vec::new() };
        ignored.registered = false;
        ignored.heartbeat = true;
        // The open connection has not finished registering, so the account message
        // before Register is silence and the next step registers, then signs in.
        assert_eq!(run_sign_in(&mut ignored), Ok("signed-in"));
        assert_eq!(ignored.steps, vec!["heartbeat", "account-before-register", "register-then-account"]);
    }

    #[test]
    fn connect_is_refused_when_incoming_only_is_on() {
        let dir = std::env::temp_dir().join(format!("pjdesk-phone-incoming-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let events = Arc::new(Mutex::new(Vec::new()));
            struct Rec(Arc<Mutex<Vec<String>>>);
            impl Callbacks for Rec {
                fn on_event(&self, json: &str) {
                    self.0.lock().unwrap().push(json.to_string());
                }
                fn on_video(&self, _data: &[u8], _width: u32, _height: u32, _keyframe: bool) {}
            }
            let state = test_state(&dir);
            state.incoming_only.store(true, Ordering::SeqCst);
            let cb: Arc<dyn Callbacks> = Arc::new(Rec(events.clone()));
            apply(&state, &cb, json!({"cmd":"connect","id":"123456789","password":"secret"})).await;
            let text = events.lock().unwrap().join("\n");
            assert!(text.contains(pjdesk_proto::signal::INCOMING_ONLY_REFUSAL), "{text}");
            assert!(!text.contains("secret"));
            assert!(state.viewer.lock().unwrap().is_none());
            apply(&state, &cb, json!({"cmd":"incomingOnly","on": false, "password":"secret"})).await;
            assert!(state.incoming_only.load(Ordering::SeqCst), "turning it off waits for the server");
            assert!(!events.lock().unwrap().iter().any(|line| line.contains("secret")));
        });
        let _ = std::fs::remove_dir_all(&dir);
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
