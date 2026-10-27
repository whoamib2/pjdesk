//! Host mode: stay online, show ID + password, stream to one controller at a time.

pub mod capture;
mod ctrl;
pub(crate) mod direct;
mod encoder;
mod input;
pub(crate) mod notice;
mod stream;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use pjdesk_proto::auth::generate_password;
use pjdesk_proto::framing::write_msg;
use pjdesk_proto::peer::HostMsg;
use pjdesk_proto::signal::{ClientMsg, ErrorCode, Policy, ServerMsg, SessionId};
use pjdesk_proto::tls::TlsIdentity;
use pjdesk_proto::udp::Role;
use pjdesk_proto::{format_id, PEER_SERVER_NAME};
use quinn::{Connection, Endpoint};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use pjdesk_link::admit::{self, Admit};
use pjdesk_link::lockout::{self, Lockout};
use pjdesk_link::session::{grant_auth, refuse_auth, refuse_before_password, NeedsApproval};

use crate::config::Config;
use crate::session::{self, Authenticated, LockedOut, WrongPassword};
use crate::signaling::{ServerAddr, SignalClient};
use crate::{p2p, HostArgs};

/// Live status shared with a GUI (or any supervisor) while hosting.
///
/// The host publishes its ID, current password and who is connected; the UI can
/// ask for a new random password. All fields are safe to
/// read at any time; `None`/empty means "not known yet".
#[derive(Default)]
pub struct HostShared {
    /// Registered with the server and reachable.
    pub online: AtomicBool,
    /// Round-trip of the last server ping, in milliseconds. 0 until the first reply.
    pub rtt_ms: AtomicU64,
    /// Unix time of the last Register or Pong. 0 when this computer is not registered.
    pub acked_unix: AtomicU64,
    /// Server address the last registration targeted.
    pub server: Mutex<String>,
    /// `LAN` or `Internet` once the signaling connection is up.
    pub reach: Mutex<String>,
    /// This device's ID once registered.
    pub id: Mutex<Option<String>>,
    /// The password a controller must enter right now.
    pub password: Mutex<String>,
    /// Temporary 8-digit support code. Shown with [`Self::support_password`].
    pub support_code: Mutex<String>,
    /// Temporary password that matches the support code. Not the permanent one.
    pub support_password: Mutex<String>,
    /// Unix time the current support pair was created. 0 until the first pair.
    pub support_born: AtomicU64,
    /// Name of the controller currently connected, if any.
    pub connected: Mutex<Option<String>>,
    /// Last error worth showing the user (e.g. can't reach the server).
    pub last_error: Mutex<Option<String>>,
    /// True when a permanent password is also set (both passwords work).
    pub permanent_set: AtomicBool,
    /// UI → host: switch to this exact password.
    set_password: Mutex<Option<String>>,
    /// UI → host: pick a fresh random password.
    regenerate: AtomicBool,
    /// UI → host: replace the support code and its temporary password.
    refresh_support: AtomicBool,
    /// UI → host: stop hosting and return (e.g. the server changed).
    stop: AtomicBool,
    /// A connection waiting for the person at this computer to accept it.
    consent: Mutex<Option<ConsentAsk>>,
    consent_reply: Mutex<Option<ConsentReply>>,
    consent_seq: AtomicU64,
    chat: Mutex<Vec<(bool, String)>>,
    chat_out: Mutex<Vec<String>>,
    send_files: Mutex<Vec<std::path::PathBuf>>,
    /// Lines for a transfer list. Nothing in the window reads them yet.
    #[allow(dead_code)]
    transfers: Mutex<Vec<String>>,
    /// Primary address for a relayed session the server asked to move.
    pub relay_move: Mutex<Option<String>>,
    /// The host has forwarded that move to the controller.
    pub relay_sent: AtomicBool,
}

/// Shown when someone wants to connect and approval is set to prompt.
#[derive(Clone, Debug)]
pub struct ConsentAsk {
    pub id: u64,
    /// Who asked, and from where. Stored for the approval prompt.
    #[allow(dead_code)]
    pub from: String,
    #[allow(dead_code)]
    pub ip: String,
    /// Permission bits the person may grant.
    #[allow(dead_code)]
    pub offered: u32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConsentReply {
    pub id: u64,
    pub accept: bool,
    pub permissions: u32,
    #[serde(default)]
    pub receive_dir: String,
}

/// The first line of a dial error, short enough to show next to an address.
fn one_line(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("failed");
    let mut out = String::new();
    for part in line.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }
    if out.is_empty() {
        "failed".into()
    } else if out.chars().count() > 180 {
        let cut: String = out.chars().take(179).collect();
        format!("{cut}…")
    } else {
        out
    }
}

fn unix_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl HostShared {
    pub fn id(&self) -> Option<String> {
        self.id.lock().unwrap().clone()
    }
    pub fn password(&self) -> String {
        self.password.lock().unwrap().clone()
    }
    pub fn connected_peer(&self) -> Option<String> {
        self.connected.lock().unwrap().clone()
    }
    pub fn error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }
    pub fn note_ack(&self) {
        self.acked_unix.store(unix_secs(), Ordering::SeqCst);
    }
    pub fn clear_registration(&self) {
        self.online.store(false, Ordering::SeqCst);
        self.acked_unix.store(0, Ordering::SeqCst);
    }
    /// Online only when the server has acked within [`pjdesk_proto::REGISTRATION_ACK_SECS`].
    pub fn acked_recently(&self) -> bool {
        pjdesk_proto::registration_acked(self.is_online(), self.acked_unix.load(Ordering::SeqCst), unix_secs())
    }
    pub fn server(&self) -> String {
        self.server.lock().unwrap().clone()
    }
    pub fn rtt_ms(&self) -> u64 {
        self.rtt_ms.load(Ordering::Relaxed)
    }
    pub fn reach(&self) -> String {
        self.reach.lock().unwrap().clone()
    }
    pub fn permanent_set(&self) -> bool {
        self.permanent_set.load(Ordering::SeqCst)
    }
    /// Request a fresh random password.
    pub fn request_regenerate(&self) {
        self.regenerate.store(true, Ordering::SeqCst);
    }
    pub fn request_support_refresh(&self) {
        self.refresh_support.store(true, Ordering::SeqCst);
    }
    pub fn support_code(&self) -> String {
        self.support_code.lock().unwrap().clone()
    }
    pub fn support_password(&self) -> String {
        self.support_password.lock().unwrap().clone()
    }
    pub fn support_born(&self) -> u64 {
        self.support_born.load(Ordering::SeqCst)
    }
    /// Ask the host loop to stop (used before restarting with new settings).
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn begin_consent(&self, from: &str, ip: &str, offered: u32) -> u64 {
        let id = self.consent_seq.fetch_add(1, Ordering::SeqCst) + 1;
        *self.consent_reply.lock().unwrap() = None;
        *self.consent.lock().unwrap() = Some(ConsentAsk { id, from: from.to_string(), ip: ip.to_string(), offered });
        id
    }

    pub fn take_reply(&self, id: u64) -> Option<ConsentReply> {
        let mut slot = self.consent_reply.lock().unwrap();
        if slot.as_ref().is_some_and(|r| r.id == id) {
            slot.take()
        } else {
            None
        }
    }

    pub fn clear_consent(&self, id: u64) {
        let mut slot = self.consent.lock().unwrap();
        if slot.as_ref().is_some_and(|c| c.id == id) {
            *slot = None;
        }
    }

    pub fn pending_consent(&self) -> Option<ConsentAsk> {
        self.consent.lock().unwrap().clone()
    }

    /// The window's Allow or Decline. `offered` bits are granted on Allow.
    pub fn answer_consent(&self, id: u64, accept: bool) {
        let offered = self
            .consent
            .lock()
            .unwrap()
            .as_ref()
            .filter(|ask| ask.id == id)
            .map(|ask| ask.offered)
            .unwrap_or(pjdesk_proto::perms::KEYBOARD | pjdesk_proto::perms::MOUSE);
        *self.consent_reply.lock().unwrap() = Some(ConsentReply {
            id,
            accept,
            permissions: if accept { offered } else { 0 },
            receive_dir: String::new(),
        });
    }

    pub fn push_chat(&self, from_me: bool, text: &str) {
        let mut chat = self.chat.lock().unwrap();
        chat.push((from_me, text.to_string()));
        if chat.len() > 200 {
            let drop_n = chat.len() - 200;
            chat.drain(0..drop_n);
        }
    }

    pub fn chat(&self) -> Vec<(bool, String)> {
        self.chat.lock().unwrap().clone()
    }

    pub fn send_chat(&self, text: String) {
        if !text.trim().is_empty() {
            self.push_chat(true, text.trim());
            self.chat_out.lock().unwrap().push(text.trim().to_string());
        }
    }

    pub fn take_chat_out(&self) -> Vec<String> {
        std::mem::take(&mut *self.chat_out.lock().unwrap())
    }

    pub fn take_send_files(&self) -> Vec<std::path::PathBuf> {
        std::mem::take(&mut *self.send_files.lock().unwrap())
    }

    pub fn note_transfer(&self, text: &str) {
        let mut lines = self.transfers.lock().unwrap();
        lines.push(text.to_string());
        if lines.len() > 40 {
            let n = lines.len() - 40;
            lines.drain(0..n);
        }
    }
}

/// Connection requests that may be in setup at once. Only an authenticated
/// session is exclusive, so strangers who know the ID can't keep the host
/// "busy" by sending requests they never complete.
const MAX_PENDING: usize = 3;

/// Shared state of the hosting process.
pub(crate) struct HostState {
    /// Certificate for the QUIC connection (its fingerprint goes to controllers).
    identity: TlsIdentity,
    /// The rotating one-time password, always available and shown in the app.
    password: Mutex<String>,
    /// An optional permanent password that also authenticates, when the user
    /// has set one. Both this and the one-time password let a controller in.
    permanent: Mutex<Option<String>>,
    /// Temporary 8-digit code shown as "Let someone connect".
    support_code: Mutex<String>,
    /// Temporary password that matches the support code. Not the permanent one.
    support_password: Mutex<String>,
    /// When the current support pair was created.
    support_born: Mutex<Instant>,
    /// An authenticated controller is connected.
    streaming: AtomicBool,
    /// Sessions being set up (not yet authenticated).
    pending: AtomicUsize,
    /// Routes the server's PeerInfo messages to the session waiting for them.
    peer_info_routes: Mutex<HashMap<SessionId, mpsc::UnboundedSender<ServerMsg>>>,
    /// Permission ceiling and receive folder chosen when the session was approved.
    ceilings: Mutex<HashMap<SessionId, (u32, String)>>,
    lockout: Mutex<Lockout>,
    /// Wrong passwords since the one-time password last changed.
    otp_misses: AtomicUsize,
    args: HostArgs,
    name: String,
    /// Optional status channel to a GUI/supervisor.
    shared: Option<Arc<HostShared>>,
    /// Rules pushed by a version-3 server. Defaults allow the classic session.
    policy: Mutex<Policy>,
    /// Protocol the server accepted for this registration.
    server_proto: AtomicU16,
    /// This computer's ID, once the server has assigned it (also used by LAN beacons).
    device_id: Mutex<String>,
    /// Refresh token and key id for the connection log. Absent when this computer
    /// has no support key. The digits are not here.
    support_log: Mutex<Option<SupportLog>>,
    /// New primary from [`ServerMsg::MoveRelay`]. The next registration dials it.
    follow: Mutex<Option<String>>,
    /// Session the server asked to move.
    moving: Mutex<Option<SessionId>>,
}

#[derive(Clone)]
struct SupportLog {
    #[allow(dead_code)]
    key_id: String,
    refresh: String,
    device_id: String,
    pjdesk_id: String,
}

impl HostState {
    fn remember_support(&self, config: &crate::config::Config) {
        let grant = config.support_grant.as_ref();
        let next = grant.and_then(|grant| {
            if grant.key_id.trim().is_empty() {
                return None;
            }
            let refresh = config.network_refresh_token();
            if refresh.is_empty() {
                return None;
            }
            Some(SupportLog {
                key_id: grant.key_id.clone(),
                refresh,
                device_id: config.network_device_id.clone().unwrap_or_default(),
                pjdesk_id: config.device_id.clone().unwrap_or_default(),
            })
        });
        *self.support_log.lock().unwrap() = next;
    }

    fn note_support_connection(&self, via_server: bool, from_name: &str) {
        if !via_server {
            return;
        }
        let Some(log) = self.support_log.lock().unwrap().clone() else { return };
        let from_name = from_name.trim().to_string();
        std::thread::spawn(move || {
            if let Err(e) = crate::support_keys::log_connection(&log.refresh, &log.device_id, &log.pjdesk_id, &from_name) {
                debug!("support connection was not logged: {e}");
            }
        });
    }

    /// Publishes the current password to the UI handle, if any.
    fn publish_password(&self) {
        if let Some(shared) = &self.shared {
            *shared.password.lock().unwrap() = self.password.lock().unwrap().clone();
            shared.permanent_set.store(self.permanent.lock().unwrap().is_some(), Ordering::SeqCst);
        }
    }

    /// The passwords a controller may authenticate with: the one-time password,
    /// the permanent one when set, and the temporary support password.
    fn auth_passwords(&self) -> Vec<String> {
        let mut pws = vec![self.password.lock().unwrap().clone()];
        if let Some(p) = self.permanent.lock().unwrap().clone() {
            if !p.trim().is_empty() && !pws.contains(&p) {
                pws.push(p);
            }
        }
        let support = self.support_password.lock().unwrap().clone();
        if !support.trim().is_empty() && !pws.contains(&support) {
            pws.push(support);
        }
        pws
    }

    /// A new support code and a new temporary password. The permanent password
    /// is left alone. The digits and the password stay in memory.
    fn fresh_support(&self) {
        *self.support_code.lock().unwrap() = pjdesk_proto::random_digits(8);
        *self.support_password.lock().unwrap() = generate_password();
        *self.support_born.lock().unwrap() = Instant::now();
        self.publish_support();
    }

    fn publish_support(&self) {
        let Some(shared) = &self.shared else { return };
        *shared.support_code.lock().unwrap() = self.support_code.lock().unwrap().clone();
        *shared.support_password.lock().unwrap() = self.support_password.lock().unwrap().clone();
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        shared.support_born.store(unix, Ordering::SeqCst);
    }

    /// Replaces a support pair that sat unused for 10 minutes. A live session
    /// keeps the pair that opened it.
    fn maybe_expire_support(&self) {
        if self.streaming.load(Ordering::SeqCst) {
            return;
        }
        let born = *self.support_born.lock().unwrap();
        if born.elapsed() >= Duration::from_secs(10 * 60) {
            self.fresh_support();
        }
    }

    /// Applies any password change the UI asked for (regenerate or set).
    fn apply_ui_requests(&self) {
        let Some(shared) = &self.shared else { return };
        let mut changed = false;
        if shared.regenerate.swap(false, Ordering::SeqCst) {
            *self.password.lock().unwrap() = generate_password();
            changed = true;
        }
        if let Some(pw) = shared.set_password.lock().unwrap().take() {
            if !pw.trim().is_empty() {
                *self.password.lock().unwrap() = pw.trim().to_string();
                changed = true;
            }
        }
        if changed {
            self.publish_password();
        }
        if shared.refresh_support.swap(false, Ordering::SeqCst) {
            self.fresh_support();
        } else {
            self.maybe_expire_support();
        }
    }
}

impl HostState {
    /// Why a new session can't be accepted right now, if it can't.
    /// A password lockout is not one of these: the controller still connects
    /// and is told how long to wait.
    fn refuse_reason(&self) -> Option<String> {
        if self.streaming.load(Ordering::SeqCst) {
            return Some("this computer is already in a remote session".into());
        }
        if self.pending.load(Ordering::SeqCst) >= MAX_PENDING {
            return Some("too many connection attempts in progress; try again shortly".into());
        }
        None
    }

    /// Counts a wrong password. Returns the text to send the controller.
    fn note_wrong_password(&self, source: &str) -> String {
        let status = self.lockout.lock().unwrap().fail(source, Instant::now());
        let misses = self.otp_misses.fetch_add(1, Ordering::SeqCst) + 1;
        if misses % 3 == 0 {
            *self.password.lock().unwrap() = generate_password();
            self.publish_password();
        }
        if status.locked {
            self.log_lock(source, &status);
            lockout::message(status.minutes)
        } else {
            "wrong password".into()
        }
    }

    fn note_success(&self, source: &str) {
        self.lockout.lock().unwrap().succeed(source);
        self.otp_misses.store(0, Ordering::SeqCst);
    }

    fn log_lock(&self, source: &str, status: &lockout::Status) {
        if !status.fresh {
            return;
        }
        if status.global {
            warn!("Locked out all connections for {} minutes after repeated wrong passwords", status.minutes);
        } else {
            warn!("Locked out {source} for {} minutes after repeated wrong passwords", status.minutes);
        }
    }
}

/// Runs host mode until a fatal error (it reconnects to the server as needed).
pub fn run(server: ServerAddr, config: Config, args: HostArgs) -> Result<()> {
    run_with(server, config, args, None)
}

/// Like [`run`], but reports status to (and takes password changes from) a GUI.
pub fn run_with(server: ServerAddr, config: Config, args: HostArgs, shared: Option<Arc<HostShared>>) -> Result<()> {
    platform_init();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    runtime.block_on(host_main(server, config, args, shared))
}

async fn host_main(
    server: ServerAddr,
    mut config: Config,
    args: HostArgs,
    shared: Option<Arc<HostShared>>,
) -> Result<()> {
    let state = Arc::new(HostState {
        identity: TlsIdentity::generate(PEER_SERVER_NAME)?,
        password: Mutex::new(generate_password()),
        permanent: Mutex::new(args.permanent_password.clone().filter(|p| !p.trim().is_empty())),
        support_code: Mutex::new(String::new()),
        support_password: Mutex::new(String::new()),
        support_born: Mutex::new(Instant::now()),
        streaming: AtomicBool::new(false),
        pending: AtomicUsize::new(0),
        peer_info_routes: Mutex::new(HashMap::new()),
        ceilings: Mutex::new(HashMap::new()),
        lockout: Mutex::new(Lockout::default()),
        otp_misses: AtomicUsize::new(0),
        name: crate::hostname(),
        args: args.clone(),
        shared: shared.clone(),
        policy: Mutex::new(Policy::default()),
        server_proto: AtomicU16::new(pjdesk_proto::PROTOCOL_VERSION),
        device_id: Mutex::new(String::new()),
        support_log: Mutex::new(None),
        follow: Mutex::new(None),
        moving: Mutex::new(None),
    });
    if args.allow_direct {
        let direct = state.clone();
        tokio::spawn(async move {
            if let Err(e) = direct::serve(direct).await {
                warn!("Direct connections stopped: {e:#}");
            }
        });
    }
    state.publish_password();
    state.fresh_support();

    let mut delay = Duration::from_secs(1);
    let mut force_refresh = false;
    let mut public_only = false;
    let mut use_standby = false;
    let mut fast_tries = 0u32;
    let role = if args.service_host { "service" } else { "window" };
    loop {
        if state.shared.as_ref().is_some_and(|s| s.stopping()) {
            return Ok(());
        }
        let outcome = signaling_loop(&server, &mut config, &state, force_refresh, public_only, use_standby).await;
        force_refresh = true;
        if state.shared.as_ref().is_some_and(|s| s.stopping()) {
            if let Some(shared) = &state.shared {
                shared.clear_registration();
            }
            return Ok(());
        }
        let has_standby = config.network_standby.as_deref().is_some_and(|s| !s.trim().is_empty());
        let fail = match &outcome {
            Ok(SignalEnd::Stop) => {
                if let Some(shared) = &state.shared {
                    shared.clear_registration();
                }
                return Ok(());
            }
            Ok(SignalEnd::Again) | Ok(SignalEnd::Follow) => None,
            Ok(SignalEnd::Lost) => Some(pjdesk_proto::failover::RegFail::Unreachable),
            Err(e) if format!("{e:#}").contains("SERVER KEY MISMATCH") => {
                if let Some(shared) = &state.shared {
                    shared.clear_registration();
                    *shared.last_error.lock().unwrap() = Some(format!("{e:#}"));
                }
                info!("This computer is not registered ({role}). {e:#}");
                return Err(anyhow::anyhow!("{e:#}"));
            }
            Err(e) if pjdesk_proto::signal::is_standby_refusal(&format!("{e:#}")) => {
                info!("This computer is not registered ({role}). {e:#}");
                if let Some(shared) = &state.shared {
                    *shared.last_error.lock().unwrap() = Some(format!("{e:#}"));
                }
                Some(pjdesk_proto::failover::RegFail::StillStandby)
            }
            Err(e) => {
                info!("This computer is not registered ({role}). {e:#}");
                if let Some(shared) = &state.shared {
                    *shared.last_error.lock().unwrap() = Some(format!("{e:#}"));
                }
                Some(pjdesk_proto::failover::RegFail::Unreachable)
            }
        };
        let streaming = state.streaming.load(Ordering::SeqCst);
        if streaming && matches!(fail, Some(pjdesk_proto::failover::RegFail::Unreachable)) {
            info!(
                "rendezvous connection lost; a direct, LAN, or hole-punched session keeps running. A relayed session reconnects on its own."
            );
        }
        let mut advance = false;
        if let Some(kind) = fail {
            let hop = pjdesk_proto::failover::next_register(kind, use_standby, has_standby, fast_tries, delay);
            let keep = pjdesk_proto::failover::keep_saved_status(
                has_standby,
                state.shared.as_ref().is_some_and(|s| s.acked_recently()),
            );
            if let Some(shared) = &state.shared {
                if !keep {
                    shared.clear_registration();
                }
                *shared.connected.lock().unwrap() = None;
                *shared.reach.lock().unwrap() = String::new();
            }
            if !hop.log.is_empty() {
                let where_next = if hop.use_standby {
                    config.network_standby.clone().unwrap_or_default()
                } else {
                    config.effective_server()
                };
                info!("failover: {} {where_next}", hop.log);
            }
            use_standby = hop.use_standby;
            public_only = hop.use_standby || kind == pjdesk_proto::failover::RegFail::StillStandby;
            delay = hop.delay;
            advance = hop.advance_backoff;
            if advance {
                fast_tries = pjdesk_proto::failover::FAST_REGISTER_TRIES;
            } else {
                fast_tries = fast_tries.saturating_add(1);
            }
        } else {
            public_only = false;
            fast_tries = 0;
            delay = Duration::from_secs(1);
        }
        let pause = pjdesk_proto::signal::jitter_reconnect(delay);
        info!("Reconnecting to the server in {:.1} s...", pause.as_secs_f32());
        tokio::time::sleep(pause).await;
        if advance {
            delay = pjdesk_proto::signal::next_reconnect_delay(delay);
        }
    }
}

enum SignalEnd {
    Stop,
    /// Register again with the same server. The access key changed.
    Again,
    /// The server stopped answering.
    Lost,
    /// Register with the primary named by a relay move.
    Follow,
}

/// Saves a catalog without dropping the other server when this push omits it.
fn remember_routes(config: &mut Config, routes: pjdesk_proto::routes::ServerRoutes) -> bool {
    if routes.is_empty() {
        return false;
    }
    let merged = pjdesk_proto::routes::keep_other_server(&config.network_routes, &routes);
    if merged == config.network_routes {
        return false;
    }
    config.network_routes = merged;
    let _ = config.save();
    true
}

/// Dials the fresh advertised IPs, then a saved hostname, and registers.
/// A standby redirect is followed at once. A named primary that does not
/// answer is not the only address tried.
async fn register_catalog(
    config: &mut Config,
    state: &Arc<HostState>,
    role: &str,
    key: &str,
) -> Result<(SignalClient, ServerAddr, String)> {
    let mine = tokio::task::spawn_blocking(pjdesk_proto::discover::private_ipv4s).await.unwrap_or_default();
    let token = config.presented_access();
    let access_key = pjdesk_proto::access::is_access_key(&token);
    let device_secret = config.device_secret()?;
    let mut without_token = false;
    let mut redirects = 0u8;
    // Take the follow address now. A miss must not pin every later try to it.
    let follow = state.follow.lock().unwrap().take().filter(|addr| !addr.trim().is_empty());
    let targets = pjdesk_proto::routes::registration_targets(
        follow.as_deref().unwrap_or(""),
        &config.network_routes,
        &mine,
        &config.effective_server(),
    );
    if targets.is_empty() {
        info!("registration has no address to dial");
        bail!("could not reach the primary or the standby; retrying. the server address is empty");
    }
    let mut signal = None;
    let mut attempts = Vec::new();
    for target in &targets {
        info!("Dialing {target}");
        match SignalClient::connect_endpoint(target, config, pjdesk_proto::routes::dial_budget(target)).await {
            Ok(got) => {
                info!("Server answered at {target} via {} ({})", got.via, got.server_addr);
                signal = Some(got);
                break;
            }
            Err(err) => {
                let why = one_line(&err);
                info!("registration route {target} failed: {why}");
                attempts.push((target.clone(), why));
            }
        }
    }
    let mut signal = signal.ok_or_else(|| {
        let pairs: Vec<(&str, &str)> = attempts.iter().map(|(addr, err)| (addr.as_str(), err.as_str())).collect();
        let report = pjdesk_proto::routes::registration_miss(&pairs);
        info!("{report}");
        anyhow::anyhow!("{report}")
    })?;
    loop {
        let register_proto = state.server_proto.load(Ordering::SeqCst);
        let server = ServerAddr { host: signal.server_addr.ip().to_string(), port: signal.server_addr.port() };
        info!("Registration target ({role}): {server} via {} ({}), key {key}", signal.via, signal.server_addr);
        let presented = if without_token { String::new() } else { token.clone() };
        if !presented.is_empty() {
            let _ = signal.send(ClientMsg::PresentAccess { token: presented });
        }
        signal.send(ClientMsg::Register {
            proto: register_proto,
            device_id: config.device_id.clone(),
            device_secret,
            hostname: state.name.clone(),
            app_version: env!("CARGO_PKG_VERSION").into(),
        })?;
        loop {
            match timeout(Duration::from_secs(10), signal.recv()).await {
                Ok(Some(ServerMsg::Registered { device_id })) => {
                    *state.follow.lock().unwrap() = None;
                    return Ok((signal, server, device_id));
                }
                Ok(Some(ServerMsg::Routes(routes))) => {
                    remember_routes(config, routes);
                }
                Ok(Some(ServerMsg::Error { code: ErrorCode::IdConflict, message })) => {
                    config.device_id = None;
                    config.save()?;
                    bail!("{message}; a new ID will be requested");
                }
                Ok(Some(ServerMsg::Error { message, .. }))
                    if pjdesk_proto::signal::is_standby_refusal(&message) && redirects < 1 =>
                {
                    redirects += 1;
                    let Some((addr, routes)) = pjdesk_proto::signal::parse_standby_redirect(&message) else {
                        bail!("could not reach the primary after the standby redirect; retrying");
                    };
                    remember_routes(config, routes);
                    if addr.is_empty() {
                        bail!("could not reach the primary after the standby redirect; retrying");
                    }
                    info!("Standby redirected registration to {addr}");
                    signal = SignalClient::connect_endpoint(&addr, config, pjdesk_proto::routes::dial_budget(&addr)).await.map_err(|err| {
                        let why = one_line(&err);
                        info!("registration route {addr} failed: {why}");
                        anyhow::anyhow!("{}", pjdesk_proto::routes::registration_miss(&[(&addr, why.as_str())]))
                    })?;
                    break;
                }
                Ok(Some(ServerMsg::Error { message, .. })) => {
                    if pjdesk_proto::version_rejected(&message) && register_proto > pjdesk_proto::MIN_PROTOCOL_VERSION {
                        state.server_proto.store(pjdesk_proto::MIN_PROTOCOL_VERSION, Ordering::SeqCst);
                        bail!("the server speaks an older protocol; reconnecting");
                    }
                    bail!("the server refused registration: {message}");
                }
                Ok(Some(other)) => bail!("unexpected reply from the server: {other:?}"),
                Ok(None) if !token.is_empty() && !access_key && !without_token => {
                    without_token = true;
                    signal = SignalClient::connect_catalog(config, &mine).await?;
                    break;
                }
                Ok(None) => bail!("the server closed the connection"),
                Err(_) => bail!("the server did not answer"),
            }
        }
    }
}

/// One signaling connection: register, then handle incoming sessions.
/// Returns `Ok` if we were registered before the connection dropped.
async fn signaling_loop(
    server: &ServerAddr,
    config: &mut Config,
    state: &Arc<HostState>,
    force_refresh: bool,
    public_only: bool,
    use_standby: bool,
) -> Result<SignalEnd> {
    let _ = (server, force_refresh, public_only, use_standby);
    tokio::task::spawn_blocking({
        let mut copy = config.clone();
        move || {
            crate::network::refresh_config_now(&mut copy);
            copy
        }
    })
    .await
    .ok()
    .map(|refreshed| {
        config.network_access = refreshed.network_access;
        config.network_access_exp = refreshed.network_access_exp;
        config.network_refresh = refreshed.network_refresh;
        config.network_email = refreshed.network_email;
        config.server = refreshed.server;
        config.server_fingerprint = refreshed.server_fingerprint;
        config.network_lan = refreshed.network_lan;
        config.network_public_ip = refreshed.network_public_ip;
        config.network_standby = refreshed.network_standby;
        config.network_standby_fingerprint = refreshed.network_standby_fingerprint;
        config.network_routes = refreshed.network_routes;
    });
    let _ = config.reload_access_key();
    state.remember_support(config);
    let role = if state.args.service_host { "service" } else { "window" };
    let key = config
        .server_fingerprint
        .as_deref()
        .map(pjdesk_proto::discover::fingerprint_prefix)
        .unwrap_or_else(|| "unpinned".into());
    let (mut signal, server, device_id) = register_catalog(config, state, role, &key).await?;
    *state.device_id.lock().unwrap() = device_id.clone();
    if config.device_id.as_deref() != Some(device_id.as_str()) {
        config.device_id = Some(device_id.clone());
        config.save()?;
    }
    state.remember_support(config);
    let has_permanent = state.permanent.lock().unwrap().is_some();
    print_banner(&device_id, &state.password.lock().unwrap(), has_permanent, &server);
    crate::HOST_ONLINE.store(true, Ordering::SeqCst);
    info!(
        "Server ack: registered {} ({role}) with {server} via {} ({}), key {key}",
        format_id(&device_id),
        signal.via,
        signal.server_addr
    );
    if let Some(shared) = &state.shared {
        *shared.id.lock().unwrap() = Some(device_id.clone());
        *shared.last_error.lock().unwrap() = None;
        *shared.reach.lock().unwrap() = signal.via.to_string();
        *shared.server.lock().unwrap() = server.to_string();
        shared.online.store(true, Ordering::SeqCst);
        shared.note_ack();
    }
    state.publish_password();

    let server_ip = signal.server_addr.ip();
    let mut ping = tokio::time::interval(pjdesk_proto::signal::HEARTBEAT);
    ping.tick().await;
    let mut ui_tick = tokio::time::interval(Duration::from_millis(300));
    let mut report_host = false;
    let mut since_report = 0u32;
    // The server answers every ping. If it goes quiet (its computer died, or
    // DNS now points to a backup server), reconnect, which looks the server's
    // name up again.
    let mut last_heard = std::time::Instant::now();
    let mut ping_at: Option<Instant> = None;
    let mut awaiting_pong = false;
    let mut heartbeats = 0u32;

    loop {
        tokio::select! {
            _ = ui_tick.tick() => {
                if state.shared.as_ref().is_some_and(|s| s.stopping()) {
                    return Ok(SignalEnd::Stop);
                }
                state.apply_ui_requests();
                let sent = state.shared.as_ref().is_some_and(|s| s.relay_sent.load(Ordering::SeqCst));
                if sent && !state.streaming.load(Ordering::SeqCst) {
                    if let Some(id) = state.moving.lock().unwrap().take() {
                        let _ = signal.send(ClientMsg::RelayMoved { session_id: id });
                    }
                    if let Some(shared) = &state.shared {
                        shared.relay_sent.store(false, Ordering::SeqCst);
                    }
                    info!("failover: relayed session left; registering with the new primary");
                    return Ok(SignalEnd::Follow);
                }
            }
            _ = ping.tick() => {
                if awaiting_pong {
                    warn!(
                        "Heartbeat missed ({role}) from {server} via {} ({}); no pong since the last ping. Reconnecting",
                        signal.via, signal.server_addr
                    );
                    return Ok(SignalEnd::Lost);
                }
                if last_heard.elapsed() >= pjdesk_proto::signal::HEARTBEAT_SILENCE {
                    warn!("No reply from the server for {} s ({role}); reconnecting", pjdesk_proto::signal::HEARTBEAT_SILENCE.as_secs());
                    return Ok(SignalEnd::Lost);
                }
                if config.reload_access_key() {
                    tracing::info!("The access key changed; registering again");
                    return Ok(SignalEnd::Again);
                }
                if heartbeats == 0 {
                    // The first ack also carries this build's version, so an
                    // in-place update is stored before the next heartbeat.
                    if let Ok(device_secret) = config.device_secret() {
                        let _ = signal.send(ClientMsg::Register {
                            proto: state.server_proto.load(Ordering::SeqCst),
                            device_id: config.device_id.clone(),
                            device_secret,
                            hostname: state.name.clone(),
                            app_version: env!("CARGO_PKG_VERSION").into(),
                        });
                    }
                }
                signal.send(ClientMsg::Ping)?;
                awaiting_pong = true;
                ping_at = Some(Instant::now());
                if report_host {
                    since_report += 1;
                    if since_report >= 5 {
                        since_report = 0;
                        signal.send(crate::hostfacts::report())?;
                        signal.send(crate::hostfacts::display_name_report())?;
                    }
                }
            }
            msg = signal.recv() => {
                let Some(msg) = msg else {
                    warn!("Lost the connection to the server ({role}); reconnecting");
                    return Ok(SignalEnd::Lost);
                };
                last_heard = std::time::Instant::now();
                match msg {
                    ServerMsg::IncomingSession { session_id, token, udp_port, controller_name, controller_ip } => {
                        if let Some(reason) = state.refuse_reason() {
                            info!("Refused a connection from \"{controller_name}\" ({controller_ip}): {reason}");
                            signal.send(ClientMsg::SessionResponse { session_id, accept: false, reason })?;
                            continue;
                        }
                        // Accept the signaling session before any Allow prompt so the
                        // controller can offer a password. The prompt, if one is
                        // needed, happens after that password is checked.
                        state.pending.fetch_add(1, Ordering::SeqCst);
                        signal.send(ClientMsg::SessionResponse { session_id, accept: true, reason: String::new() })?;
                        let (tx, rx) = mpsc::unbounded_channel();
                        state.peer_info_routes.lock().unwrap().insert(session_id, tx);
                        let server_udp = SocketAddr::new(server_ip, udp_port);
                        tokio::spawn(run_session(
                            state.clone(),
                            signal.sender(),
                            rx,
                            session_id,
                            token,
                            server_udp,
                            controller_name,
                            controller_ip,
                        ));
                    }
                    ServerMsg::PeerInfo { session_id, .. } => {
                        if let Some(tx) = state.peer_info_routes.lock().unwrap().get(&session_id) {
                            let _ = tx.send(msg);
                        }
                    }
                    ServerMsg::Policy(policy) => {
                        info!("Server policy applied (unattended {}, recording {})", policy.unattended_allowed, policy.recording_allowed);
                        *state.policy.lock().unwrap() = policy;
                    }
                    ServerMsg::Pong => {
                        awaiting_pong = false;
                        heartbeats = heartbeats.saturating_add(1);
                        let rtt = ping_at.take().map(|sent| sent.elapsed().as_millis() as u64).unwrap_or(0);
                        if let Some(shared) = &state.shared {
                            if rtt > 0 {
                                shared.rtt_ms.store(rtt, Ordering::Relaxed);
                            }
                            shared.note_ack();
                        }
                        if heartbeats == 1 || heartbeats % 6 == 0 {
                            info!(
                                "Heartbeat from {server} via {} ({}) ({role}), {} ms",
                                signal.via, signal.server_addr, rtt
                            );
                        }
                    }
                    ServerMsg::UpdateNow => {
                        info!("Server asked this computer to update now");
                        if !crate::client_update::start() {
                            info!("An update is already running");
                        }
                    }
                    ServerMsg::Routes(routes) => {
                        if remember_routes(config, routes) {
                            info!("Updated server addresses from the primary");
                        }
                    }
                    ServerMsg::MoveRelay { session_id, primary } => {
                        info!("failover: moving relayed session {session_id:016x} to the primary {primary}");
                        *state.follow.lock().unwrap() = Some(primary.clone());
                        *state.moving.lock().unwrap() = Some(session_id);
                        if let Some(shared) = &state.shared {
                            *shared.relay_move.lock().unwrap() = Some(primary);
                            shared.relay_sent.store(false, Ordering::SeqCst);
                        }
                        if !state.streaming.load(Ordering::SeqCst) {
                            let _ = signal.send(ClientMsg::RelayMoved { session_id });
                            return Ok(SignalEnd::Follow);
                        }
                    }
                    ServerMsg::WantHostReport => {
                        report_host = true;
                        since_report = 0;
                        signal.send(crate::hostfacts::report())?;
                        signal.send(crate::hostfacts::display_name_report())?;
                    }
                    ServerMsg::Error { message, .. } => warn!("Server: {message}"),
                    other => debug!("ignoring {other:?}"),
                }
            }
        }
    }
}

fn print_banner(id: &str, _password: &str, has_permanent: bool, server: &ServerAddr) {
    // The password is deliberately NOT printed here: this text can reach the
    // service log and the console, so the password is shown only in the app
    // window (masked, with a reveal button).
    println!();
    println!("  ==================================================");
    println!("    This computer can now be controlled remotely");
    println!("      ID       : {}", format_id(id));
    println!(
        "      Password : {} (shown in the PJDesk window)",
        if has_permanent { "one-time + permanent" } else { "one-time" }
    );
    println!("      Server   : {server}");
    println!("  ==================================================");
    println!();
}

/// Wraps [`session_inner`] with logging and bookkeeping.
#[allow(clippy::too_many_arguments)]
async fn run_session(
    state: Arc<HostState>,
    signal: mpsc::UnboundedSender<ClientMsg>,
    peer_info: mpsc::UnboundedReceiver<ServerMsg>,
    session_id: SessionId,
    token: [u8; 16],
    server_udp: SocketAddr,
    controller_name: String,
    controller_ip: String,
) {
    info!("Connection request from \"{controller_name}\" ({controller_ip})");
    let mut still_pending = true;
    match session_inner(
        &state,
        signal,
        peer_info,
        session_id,
        token,
        server_udp,
        &controller_name,
        &controller_ip,
        &mut still_pending,
    )
    .await
    {
        Ok(()) => info!("Remote session ended"),
        Err(e) if e.is::<WrongPassword>() => {
            warn!("\"{controller_name}\" ({controller_ip}) entered a wrong password");
        }
        Err(e) if e.is::<LockedOut>() => {
            warn!("\"{controller_name}\" ({controller_ip}) is locked out: {e}");
        }
        Err(e) => warn!("Connection from \"{controller_name}\" ended: {e:#}"),
    }
    if still_pending {
        state.pending.fetch_sub(1, Ordering::SeqCst);
    }
    state.peer_info_routes.lock().unwrap().remove(&session_id);
    state.ceilings.lock().unwrap().remove(&session_id);
}

async fn session_inner(
    state: &Arc<HostState>,
    signal: mpsc::UnboundedSender<ClientMsg>,
    mut peer_info: mpsc::UnboundedReceiver<ServerMsg>,
    session_id: SessionId,
    token: [u8; 16],
    server_udp: SocketAddr,
    controller_name: &str,
    controller_ip: &str,
    still_pending: &mut bool,
) -> Result<()> {
    let binding = p2p::bind_udp(server_udp, session_id, token, Role::Host).await?;
    debug!("our public address for this session: {}", binding.observed);
    let endpoint = p2p::host_endpoint(binding.socket, &state.identity)?;
    signal.send(ClientMsg::UdpReady {
        session_id,
        local_addrs: binding.local_addrs.clone(),
        cert_fingerprint: Some(state.identity.fingerprint()),
    })?;

    let (peer_public, peer_local) = match timeout(Duration::from_secs(15), peer_info.recv()).await {
        Ok(Some(ServerMsg::PeerInfo { public_addr, local_addrs, .. })) => (public_addr, local_addrs),
        _ => bail!("the controller never finished setting up the connection"),
    };
    let targets = p2p::candidates(peer_public, &peer_local);
    debug!("controller candidates: {targets:?}");
    let punching = p2p::spawn_punching(binding.punch_socket, targets, session_id, Duration::from_secs(8));

    let passwords = state.auth_passwords();
    let permanent = state.permanent.lock().unwrap().clone();
    let accepted = accept_authenticated(
        &endpoint,
        state,
        server_udp.ip(),
        peer_public,
        controller_ip,
        controller_name,
        &passwords,
        Duration::from_secs(90),
    )
    .await;
    punching.abort();
    let (connection, mut auth) = accepted?;
    let source = source_for(&connection, server_udp.ip(), peer_public, controller_ip, controller_name);
    if auth.password_slot != ASK_SLOT {
        state.note_success(&source);
    }
    let (mut ceiling, receive_dir) =
        open_session(state, &mut auth, &passwords, permanent.as_deref(), controller_name, controller_ip, true).await?;

    // Authenticated: this session takes the screen, unless another one got there first.
    state.pending.fetch_sub(1, Ordering::SeqCst);
    *still_pending = false;
    if state.streaming.swap(true, Ordering::SeqCst) {
        let message = "another controller is already connected";
        warn!("Password was accepted, then the session stopped: {message}");
        let _ = write_msg(&mut auth.send, &HostMsg::Notice(message.into())).await;
        connection.close(2u32.into(), message.as_bytes());
        let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
        bail!(message);
    }

    let remote = connection.remote_address();
    let path = if remote == server_udp {
        "Internet, relayed via the server"
    } else if p2p::is_lan_ip(remote.ip()) {
        "LAN"
    } else {
        "Internet"
    };
    info!("Password accepted for \"{}\". Starting the picture ({path}, {})", auth.peer_name, remote);
    if state.args.service_host {
        info!("Host log: C:\\ProgramData\\PJDesk\\service\\service.log");
    } else {
        info!("Host log: %APPDATA%\\PJDesk\\pjdesk.log");
    }
    if let Some(shared) = &state.shared {
        *shared.connected.lock().unwrap() = Some(auth.peer_name.clone());
    }
    let policy = state.policy.lock().unwrap().clone();
    ceiling &= policy.max_permissions;
    if !policy.recording_allowed {
        ceiling &= !pjdesk_proto::perms::RECORD;
    }
    let mut ports = state.args.tunnel_ports.clone();
    ports.extend(policy.tunnel_ports.iter().copied());
    ports.sort_unstable();
    ports.dedup();
    let grant = ctrl::Grant {
        ceiling,
        peer_proto: auth.peer_proto,
        peer_name: auth.peer_name.clone(),
        tunnel_remote: policy.tunnel_remote_hosts,
        tunnel_ports: ports,
        receive_dir,
    };
    let result =
        stream::run(connection.clone(), auth.send, auth.recv, state.args.clone(), grant, state.shared.clone()).await;
    state.streaming.store(false, Ordering::SeqCst);
    state.fresh_support();
    if let Some(shared) = &state.shared {
        *shared.connected.lock().unwrap() = None;
    }
    connection.close(0u32.into(), b"session over");
    let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    result
}

/// Password slot used when the controller asked for the person at this computer.
const ASK_SLOT: u8 = 255;

fn source_for(
    connection: &Connection,
    relay_ip: std::net::IpAddr,
    peer_public: SocketAddr,
    controller_ip: &str,
    controller_name: &str,
) -> String {
    lockout::source_key(
        connection.remote_address().ip(),
        Some(relay_ip),
        Some(peer_public.ip()),
        controller_ip.parse().ok(),
        controller_name,
    )
}

/// Accepts QUIC connections until one authenticates. The controller may try
/// several addresses at once, so several connections can arrive, but only
/// one password attempt is allowed per session. A lockout is answered here,
/// before any Allow prompt.
async fn accept_authenticated(
    endpoint: &Endpoint,
    state: &Arc<HostState>,
    relay_ip: std::net::IpAddr,
    peer_public: SocketAddr,
    controller_ip: &str,
    controller_name: &str,
    passwords: &[String],
    limit: Duration,
) -> Result<(Connection, Authenticated)> {
    const MAX_CONNECTIONS: usize = 8;
    let deadline = tokio::time::Instant::now() + limit;
    let attempt_used = Arc::new(AtomicBool::new(false));
    let mut wrong = 0u32;
    let mut pending: JoinSet<Result<(Connection, Authenticated)>> = JoinSet::new();
    let mut accepted = 0;
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { bail!("the QUIC endpoint closed") };
                accepted += 1;
                if accepted > MAX_CONNECTIONS {
                    incoming.refuse();
                    continue;
                }
                let (passwords, my_name, attempt_used) = (passwords.to_vec(), state.name.clone(), attempt_used.clone());
                let (state, controller_ip, controller_name) = (state.clone(), controller_ip.to_string(), controller_name.to_string());
                pending.spawn(async move {
                    let connection = incoming.await?;
                    let source = source_for(&connection, relay_ip, peer_public, &controller_ip, &controller_name);
                    let locked = state.lockout.lock().unwrap().check(&source, Instant::now());
                    if locked.locked {
                        state.log_lock(&source, &locked);
                        refuse_before_password(&connection, &lockout::message(locked.minutes)).await?;
                        return Err(LockedOut { minutes: locked.minutes }.into());
                    }
                    let fail_state = state.clone();
                    let fail_source = source.clone();
                    let fail = move || fail_state.note_wrong_password(&fail_source);
                    match session::host_auth(&connection, &passwords, &my_name, &attempt_used, &fail).await {
                        Ok(auth) => Ok((connection, auth)),
                        Err(e) if e.is::<NeedsApproval>() => {
                            let wait = e.downcast::<NeedsApproval>().unwrap();
                            Ok((connection, Authenticated {
                                send: wait.send,
                                recv: wait.recv,
                                peer_name: wait.peer_name,
                                peer_proto: wait.peer_proto,
                                password_slot: ASK_SLOT,
                            }))
                        }
                        Err(e) => Err(e),
                    }
                });
            }
            Some(joined) = pending.join_next(), if !pending.is_empty() => match joined {
                Ok(Ok(done)) => return Ok(done),
                Ok(Err(e)) if e.is::<LockedOut>() => return Err(e),
                Ok(Err(e)) if e.is::<WrongPassword>() => {
                    // The person can correct the password without asking this
                    // computer to allow the session again.
                    attempt_used.store(false, Ordering::SeqCst);
                    wrong += 1;
                    if wrong >= 8 {
                        return Err(e);
                    }
                }
                Ok(Err(e)) => debug!("a connection attempt failed: {e:#}"),
                Err(_) => {}
            },
            _ = tokio::time::sleep_until(deadline) => bail!("the controller did not connect in time"),
        }
    }
}

enum Person {
    Yes(u32, String),
    No,
    TimedOut,
    Unavailable,
}

async fn open_session(
    state: &Arc<HostState>,
    auth: &mut Authenticated,
    passwords: &[String],
    permanent: Option<&str>,
    controller_name: &str,
    controller_ip: &str,
    via_server: bool,
) -> Result<(u32, String)> {
    if auth.password_slot != ASK_SLOT {
        let support = state.support_password.lock().unwrap().clone();
        if admit::matched_support(passwords, auth.password_slot, Some(support.as_str())) {
            let policy = state.policy.lock().unwrap().clone();
            grant_auth(&mut auth.send, &state.name).await?;
            state.note_support_connection(via_server, controller_name);
            return Ok((
                admit::support_open(&policy, state.args.view_only),
                crate::resolve_receive_dir(&state.args.receive_dir),
            ));
        }
        let matched = admit::matched_permanent(passwords, auth.password_slot, permanent);
        let permanent_set = permanent.map(str::trim).is_some_and(|p| !p.is_empty());
        let policy = state.policy.lock().unwrap().clone();
        if let Admit::Open(bits) =
            admit::after_password(&state.args.approval, matched, permanent_set, &policy, state.args.view_only)
        {
            grant_auth(&mut auth.send, &state.name).await?;
            state.note_support_connection(via_server, controller_name);
            return Ok((bits, crate::resolve_receive_dir(&state.args.receive_dir)));
        }
    }
    if admit::support_key_admits(state.args.support_unattended, via_server) {
        let policy = state.policy.lock().unwrap().clone();
        grant_auth(&mut auth.send, &state.name).await?;
        state.note_support_connection(true, controller_name);
        return Ok((
            admit::support_open(&policy, state.args.view_only),
            crate::resolve_receive_dir(&state.args.receive_dir),
        ));
    }
    let who = if auth.password_slot == ASK_SLOT && !auth.peer_name.trim().is_empty() {
        auth.peer_name.as_str()
    } else {
        controller_name
    };
    match approve(state, who, controller_ip).await {
        Person::Yes(bits, dir) => {
            grant_auth(&mut auth.send, &state.name).await?;
            state.note_support_connection(via_server, controller_name);
            Ok((bits, dir))
        }
        Person::No => {
            let message = "the person at this computer declined";
            let _ = refuse_auth(&mut auth.send, message).await;
            bail!(message)
        }
        Person::TimedOut => {
            let message = "No answer";
            let _ = refuse_auth(&mut auth.send, message).await;
            bail!(message)
        }
        Person::Unavailable => {
            let message = "No one is available to accept";
            let _ = refuse_auth(&mut auth.send, message).await;
            bail!(message)
        }
    }
}

/// Process-wide setup for capture and input on Windows.
fn platform_init() {
    #[cfg(windows)]
    unsafe {
        use windows::core::{s, w, BOOL};
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows::Win32::UI::HiDpi::{
            SetProcessDpiAwareness, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            PROCESS_PER_MONITOR_DPI_AWARE,
        };
        // Physical pixels everywhere, so capture, cursor position and
        // SendInput agree on scaled displays. Per-monitor v2 needs Windows 10
        // 1703, so it is looked up at run time (importing it would stop the
        // exe from starting on Windows Server 2016); older systems fall back
        // to the Windows 8.1 API.
        type SetContext = unsafe extern "system" fn(DPI_AWARENESS_CONTEXT) -> BOOL;
        let per_monitor_v2 = GetModuleHandleW(w!("user32.dll"))
            .ok()
            .and_then(|user32| GetProcAddress(user32, s!("SetProcessDpiAwarenessContext")))
            .map(|f| {
                let set = std::mem::transmute::<unsafe extern "system" fn() -> isize, SetContext>(f);
                set(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool()
            })
            .unwrap_or(false);
        if !per_monitor_v2 {
            match SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE) {
                // E_ACCESSDENIED means the awareness was already set (e.g. by a manifest).
                Err(e) if e.code() != windows::Win32::Foundation::E_ACCESSDENIED => {
                    warn!("Could not make the process DPI aware ({e}); scaled displays may be captured incorrectly")
                }
                _ => {}
            }
        }
    }
}

/// Asks the person at this computer. Called only after a password was accepted
/// and the host's mode still requires approval, or the controller chose
/// "Ask the person instead". A timeout is not a decline.
async fn approve(state: &Arc<HostState>, name: &str, ip: &str) -> Person {
    let policy = state.policy.lock().unwrap().clone();
    if let Some(shared) = &state.shared {
        if !state.args.service_host {
            let id = shared.begin_consent(name, ip, policy.max_permissions);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
            loop {
                if let Some(reply) = shared.take_reply(id) {
                    shared.clear_consent(id);
                    return match consent_grant(&policy, reply) {
                        Some(answer) => Person::Yes(answer.0, answer.1),
                        None => Person::No,
                    };
                }
                if tokio::time::Instant::now() >= deadline {
                    shared.clear_consent(id);
                    return Person::TimedOut;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    service_consent(state, name, ip, policy.max_permissions).await
}

fn consent_grant(policy: &Policy, reply: ConsentReply) -> Option<(u32, String)> {
    use pjdesk_proto::perms;
    if !reply.accept {
        return None;
    }
    let mut bits = reply.permissions & policy.max_permissions;
    let dir = if bits & perms::FILE != 0 {
        let chosen = reply.receive_dir.trim();
        if chosen.is_empty() {
            crate::resolve_receive_dir("")
        } else {
            chosen.to_string()
        }
    } else {
        String::new()
    };
    if dir.is_empty() {
        bits &= !perms::FILE;
    }
    Some((bits, dir))
}

/// The service host has no window. It leaves `consent.json` for the signed-in
/// user's PJDesk window and waits for `consent-reply.json`.
fn someone_can_answer() -> bool {
    #[cfg(windows)]
    {
        if unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() } == 0xFFFF_FFFF {
            return false;
        }
    }
    notice::client_alive()
}

async fn service_consent(state: &HostState, name: &str, ip: &str, offered: u32) -> Person {
    if !someone_can_answer() {
        return Person::Unavailable;
    }
    let dir = crate::syspaths::service_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return Person::Unavailable;
    }
    let ask_path = dir.join("consent.json");
    let reply_path = dir.join("consent-reply.json");
    let _ = std::fs::remove_file(&reply_path);
    let id =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1);
    let ask = serde_json::json!({ "id": id, "from": name, "ip": ip, "offered": offered });
    if std::fs::write(&ask_path, ask.to_string()).is_err() {
        return Person::Unavailable;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let reply = loop {
        if let Ok(bytes) = std::fs::read(&reply_path) {
            if let Ok(reply) = serde_json::from_slice::<ConsentReply>(&bytes) {
                break Some(reply);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let _ = std::fs::remove_file(&ask_path);
    let _ = std::fs::remove_file(&reply_path);
    let Some(reply) = reply else {
        return Person::TimedOut;
    };
    match consent_grant(&state.policy.lock().unwrap(), reply) {
        Some(answer) => Person::Yes(answer.0, answer.1),
        None => Person::No,
    }
}
