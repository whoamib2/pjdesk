//! Host mode: stay online, show ID + password, stream to one controller at a time.

pub mod capture;
mod ctrl;
pub(crate) mod direct;
mod encoder;
mod input;
mod stream;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use pjdesk_proto::auth::generate_password;
use pjdesk_proto::signal::{ClientMsg, ErrorCode, Policy, ServerMsg, SessionId};
use pjdesk_proto::tls::TlsIdentity;
use pjdesk_proto::udp::Role;
use pjdesk_proto::{format_id, PEER_SERVER_NAME};
use quinn::{Connection, Endpoint};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::session::{self, Authenticated, WrongPassword};
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
    /// `LAN` or `Internet` once the signaling connection is up.
    pub reach: Mutex<String>,
    /// This device's ID once registered.
    pub id: Mutex<Option<String>>,
    /// The password a controller must enter right now.
    pub password: Mutex<String>,
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
    /// An authenticated controller is connected.
    streaming: AtomicBool,
    /// Sessions being set up (not yet authenticated).
    pending: AtomicUsize,
    /// Routes the server's PeerInfo messages to the session waiting for them.
    peer_info_routes: Mutex<HashMap<SessionId, mpsc::UnboundedSender<ServerMsg>>>,
    /// Permission ceiling and receive folder chosen when the session was approved.
    ceilings: Mutex<HashMap<SessionId, (u32, String)>>,
    failures: Mutex<Failures>,
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
}

impl HostState {
    /// Publishes the current password to the UI handle, if any.
    fn publish_password(&self) {
        if let Some(shared) = &self.shared {
            *shared.password.lock().unwrap() = self.password.lock().unwrap().clone();
            shared.permanent_set.store(self.permanent.lock().unwrap().is_some(), Ordering::SeqCst);
        }
    }

    /// The passwords a controller may authenticate with: the one-time password
    /// and, if set, the permanent one (either grants access).
    fn auth_passwords(&self) -> Vec<String> {
        let mut pws = vec![self.password.lock().unwrap().clone()];
        if let Some(p) = self.permanent.lock().unwrap().clone() {
            if !p.trim().is_empty() && !pws.contains(&p) {
                pws.push(p);
            }
        }
        pws
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
    }
}

#[derive(Default)]
struct Failures {
    count: u32,
    locked_until: Option<Instant>,
}

impl HostState {
    /// Why a new session can't be accepted right now, if it can't.
    fn refuse_reason(&self) -> Option<String> {
        if self.streaming.load(Ordering::SeqCst) {
            return Some("this computer is already in a remote session".into());
        }
        if self.pending.load(Ordering::SeqCst) >= MAX_PENDING {
            return Some("too many connection attempts in progress; try again shortly".into());
        }
        let failures = self.failures.lock().unwrap();
        match failures.locked_until {
            Some(until) if Instant::now() < until => {
                Some("too many wrong passwords; try again in a few minutes".into())
            }
            _ => None,
        }
    }

    /// Counts a wrong password. The one-time password is replaced every three
    /// misses (so an overheard or guessed one stops working); if a permanent
    /// password is also set, five misses lock out new sessions for five
    /// minutes to slow brute-forcing it.
    fn record_failure(&self) {
        let has_permanent = self.permanent.lock().unwrap().is_some();
        let mut failures = self.failures.lock().unwrap();
        failures.count += 1;
        let rotate = failures.count % 3 == 0;
        if has_permanent && failures.count >= 5 {
            failures.count = 0;
            failures.locked_until = Some(Instant::now() + Duration::from_secs(5 * 60));
            warn!("Too many wrong passwords; refusing connections for five minutes");
        }
        drop(failures);
        if rotate {
            *self.password.lock().unwrap() = generate_password();
            self.publish_password();
        }
    }

    fn reset_failures(&self) {
        self.failures.lock().unwrap().count = 0;
    }
}

/// Runs host mode until a fatal error (it reconnects to the server as needed).
pub fn run(server: ServerAddr, config: Config, args: HostArgs) -> Result<()> {
    run_with(server, config, args, None)
}

/// Like [`run`], but reports status to (and takes password changes from) a GUI.
pub fn run_with(server: ServerAddr, config: Config, args: HostArgs, shared: Option<Arc<HostShared>>) -> Result<()> {
    platform_init();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
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
        streaming: AtomicBool::new(false),
        pending: AtomicUsize::new(0),
        peer_info_routes: Mutex::new(HashMap::new()),
        ceilings: Mutex::new(HashMap::new()),
        failures: Mutex::new(Failures::default()),
        name: crate::hostname(),
        args: args.clone(),
        shared: shared.clone(),
        policy: Mutex::new(Policy::default()),
        server_proto: AtomicU16::new(pjdesk_proto::PROTOCOL_VERSION),
        device_id: Mutex::new(String::new()),
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

    let mut delay = Duration::from_secs(2);
    let mut force_refresh = false;
    loop {
        if state.shared.as_ref().is_some_and(|s| s.stopping()) {
            return Ok(());
        }
        let outcome = signaling_loop(&server, &mut config, &state, force_refresh).await;
        force_refresh = true;
        if let Some(shared) = &state.shared {
            shared.online.store(false, Ordering::SeqCst);
            *shared.connected.lock().unwrap() = None;
            *shared.reach.lock().unwrap() = String::new();
            if shared.stopping() {
                return Ok(());
            }
        }
        match outcome {
            Ok(()) => delay = Duration::from_secs(2),
            Err(e) if format!("{e:#}").contains("SERVER KEY MISMATCH") => {
                if let Some(shared) = &state.shared {
                    *shared.last_error.lock().unwrap() = Some(format!("{e:#}"));
                }
                return Err(e);
            }
            Err(e) => {
                warn!("{e:#}");
                if let Some(shared) = &state.shared {
                    *shared.last_error.lock().unwrap() = Some(format!("{e:#}"));
                }
            }
        }
        info!("Reconnecting to the server in {} s...", delay.as_secs());
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

/// One signaling connection: register, then handle incoming sessions.
/// Returns `Ok` if we were registered before the connection dropped.
async fn signaling_loop(server: &ServerAddr, config: &mut Config, state: &Arc<HostState>, force_refresh: bool) -> Result<()> {
    tokio::task::spawn_blocking({
        let mut copy = config.clone();
        move || {
            if force_refresh {
                crate::network::refresh_config_now(&mut copy);
            } else {
                crate::network::refresh_config(&mut copy);
            }
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
    });
    let server = ServerAddr::parse(&config.effective_server()).unwrap_or_else(|_| server.clone());
    let token = config.network_access_token();
    let mut signal = SignalClient::connect(&server, config).await?;
    if !token.is_empty() {
        let _ = signal.send(ClientMsg::PresentAccess { token: token.clone() });
    }
    let device_secret = config.device_secret()?;
    let register_proto = state.server_proto.load(Ordering::SeqCst);
    let register = ClientMsg::Register {
        proto: register_proto,
        device_id: config.device_id.clone(),
        device_secret,
        hostname: state.name.clone(),
        app_version: env!("CARGO_PKG_VERSION").into(),
    };
    signal.send(register.clone())?;

    let first = timeout(Duration::from_secs(10), signal.recv()).await;
    let device_id = match first {
        Ok(None) if !token.is_empty() => {
            // A server from before 0.6.0 cannot decode PresentAccess and drops
            // the socket. Try once without the token so this computer stays online.
            signal = SignalClient::connect(&server, config).await?;
            signal.send(register)?;
            timeout(Duration::from_secs(10), signal.recv()).await
        }
        other => other,
    };
    let device_id = match device_id {
        Ok(Some(ServerMsg::Registered { device_id })) => device_id,
        Ok(Some(ServerMsg::Error { code: ErrorCode::IdConflict, message })) => {
            config.device_id = None;
            config.save()?;
            bail!("{message}; a new ID will be requested");
        }
        Ok(Some(ServerMsg::Error { message, .. })) => {
            if pjdesk_proto::version_rejected(&message) && register_proto > pjdesk_proto::MIN_PROTOCOL_VERSION {
                state.server_proto.store(pjdesk_proto::MIN_PROTOCOL_VERSION, Ordering::SeqCst);
                bail!("the server speaks an older protocol; reconnecting");
            }
            bail!("the server refused registration: {message}");
        }
        Ok(Some(other)) => bail!("unexpected reply from the server: {other:?}"),
        Ok(None) => bail!("the server closed the connection"),
        Err(_) => bail!("the server did not answer"),
    };
    *state.device_id.lock().unwrap() = device_id.clone();
    if config.device_id.as_deref() != Some(device_id.as_str()) {
        config.device_id = Some(device_id.clone());
        config.save()?;
    }
    let has_permanent = state.permanent.lock().unwrap().is_some();
    print_banner(&device_id, &state.password.lock().unwrap(), has_permanent, &server);
    crate::HOST_ONLINE.store(true, Ordering::SeqCst);
    if let Some(shared) = &state.shared {
        *shared.id.lock().unwrap() = Some(device_id.clone());
        *shared.last_error.lock().unwrap() = None;
        *shared.reach.lock().unwrap() = signal.via.to_string();
        shared.online.store(true, Ordering::SeqCst);
    }
    state.publish_password();

    let server_ip = signal.server_addr.ip();
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    ping.tick().await;
    let mut ui_tick = tokio::time::interval(Duration::from_millis(300));
    let mut report_host = false;
    let mut since_report = 0u32;
    // The server answers every ping. If it goes quiet (its computer died, or
    // DNS now points to a backup server), reconnect, which looks the server's
    // name up again.
    let mut last_heard = std::time::Instant::now();
    let mut ping_at: Option<Instant> = None;

    loop {
        tokio::select! {
            _ = ui_tick.tick() => {
                if state.shared.as_ref().is_some_and(|s| s.stopping()) {
                    return Ok(());
                }
                state.apply_ui_requests();
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > Duration::from_secs(70) {
                    warn!("No reply from the server for over a minute; reconnecting");
                    return Ok(());
                }
                signal.send(ClientMsg::Ping)?;
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
                    warn!("Lost the connection to the server");
                    return Ok(());
                };
                last_heard = std::time::Instant::now();
                match msg {
                    ServerMsg::IncomingSession { session_id, token, udp_port, controller_name, controller_ip } => {
                        if let Some(reason) = state.refuse_reason() {
                            info!("Refused a connection from \"{controller_name}\" ({controller_ip}): {reason}");
                            signal.send(ClientMsg::SessionResponse { session_id, accept: false, reason })?;
                            continue;
                        }
                        let approved = match approve(state, &controller_name, &controller_ip).await {
                            Some(bits) => bits,
                            None => {
                                info!("Declined connection from \"{controller_name}\" ({controller_ip})");
                                signal.send(ClientMsg::SessionResponse {
                                    session_id,
                                    accept: false,
                                    reason: "the person at this computer declined".into(),
                                })?;
                                continue;
                            }
                        };
                        state.pending.fetch_add(1, Ordering::SeqCst);
                        signal.send(ClientMsg::SessionResponse { session_id, accept: true, reason: String::new() })?;
                        state.ceilings.lock().unwrap().insert(session_id, approved);
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
                        if let Some(sent) = ping_at.take() {
                            if let Some(shared) = &state.shared {
                                shared.rtt_ms.store(sent.elapsed().as_millis() as u64, Ordering::Relaxed);
                            }
                        }
                    }
                    ServerMsg::UpdateNow => {
                        info!("Server requested an update; running the update task now");
                        #[cfg(windows)]
                        {
                            use std::os::windows::process::CommandExt;
                            let _ = std::process::Command::new("schtasks")
                                .args(["/Run", "/TN", crate::service::UPDATE_TASK])
                                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                                .spawn();
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
    match session_inner(&state, signal, peer_info, session_id, token, server_udp, &mut still_pending).await {
        Ok(()) => info!("Remote session ended"),
        Err(e) if e.is::<WrongPassword>() => {
            warn!("\"{controller_name}\" ({controller_ip}) entered a wrong password");
            state.record_failure();
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
    let accepted = accept_authenticated(&endpoint, &passwords, &state.name, Duration::from_secs(90)).await;
    punching.abort();
    let (connection, auth) = accepted?;
    state.reset_failures();

    // Authenticated: this session takes the screen, unless another one got there first.
    state.pending.fetch_sub(1, Ordering::SeqCst);
    *still_pending = false;
    if state.streaming.swap(true, Ordering::SeqCst) {
        connection.close(2u32.into(), b"another controller is already connected");
        let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
        bail!("another controller connected first");
    }

    let remote = connection.remote_address();
    let path = if remote == server_udp {
        "Internet, relayed via the server"
    } else if p2p::is_lan_ip(remote.ip()) {
        "LAN"
    } else {
        "Internet"
    };
    info!("\"{}\" connected ({path}, {})", auth.peer_name, remote);
    if let Some(shared) = &state.shared {
        *shared.connected.lock().unwrap() = Some(auth.peer_name.clone());
    }
    let (mut ceiling, receive_dir) = state
        .ceilings
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned()
        .unwrap_or((pjdesk_proto::perms::CLASSIC, state.args.receive_dir.clone()));
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
    let result = stream::run(connection.clone(), auth.send, auth.recv, state.args.clone(), grant, state.shared.clone()).await;
    state.streaming.store(false, Ordering::SeqCst);
    if let Some(shared) = &state.shared {
        *shared.connected.lock().unwrap() = None;
    }
    connection.close(0u32.into(), b"session over");
    let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    result
}

/// Accepts QUIC connections until one authenticates. The controller may try
/// several addresses at once, so several connections can arrive, but only
/// one password attempt is allowed per session.
async fn accept_authenticated(
    endpoint: &Endpoint,
    passwords: &[String],
    my_name: &str,
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
                let (passwords, my_name, attempt_used) = (passwords.to_vec(), my_name.to_owned(), attempt_used.clone());
                pending.spawn(async move {
                    let connection = incoming.await?;
                    let auth = session::host_auth(&connection, &passwords, &my_name, &attempt_used).await?;
                    Ok((connection, auth))
                });
            }
            Some(joined) = pending.join_next(), if !pending.is_empty() => match joined {
                Ok(Ok(done)) => return Ok(done),
                Ok(Err(e)) if e.is::<WrongPassword>() => {
                    // The person can correct the password without asking this
                    // computer to allow the session again.
                    attempt_used.store(false, Ordering::SeqCst);
                    wrong += 1;
                    if wrong >= 5 {
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

/// Asks the person at this computer before a session starts. `None` declines.
/// The tuple is the granted permission bits and the folder received files may
/// be written into (empty unless file transfer was allowed).
async fn approve(state: &Arc<HostState>, name: &str, ip: &str) -> Option<(u32, String)> {
    use pjdesk_proto::perms::{self, grant};
    let policy = state.policy.lock().unwrap().clone();
    let prompt = state.args.approval != "unattended" || policy.force_prompt;
    let restricted = grant(perms::UNATTENDED_DEFAULT, policy.max_permissions, state.args.view_only) & !perms::FILE;
    let permanent = state.permanent.lock().unwrap().is_some();
    let unattended_ok = permanent && policy.unattended_allowed && !policy.force_prompt;

    if !prompt {
        return unattended_ok.then_some((restricted, String::new()));
    }

    if let Some(shared) = &state.shared {
        let id = shared.begin_consent(name, ip, policy.max_permissions);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(reply) = shared.take_reply(id) {
                shared.clear_consent(id);
                return consent_grant(&policy, reply);
            }
            if tokio::time::Instant::now() >= deadline {
                shared.clear_consent(id);
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    } else if state.args.service_host {
        if let Some(answer) = service_consent(state, name, ip, policy.max_permissions).await {
            return Some(answer);
        }
    }

    unattended_ok.then_some((restricted, String::new()))
}

fn consent_grant(policy: &Policy, reply: ConsentReply) -> Option<(u32, String)> {
    use pjdesk_proto::perms;
    if !reply.accept {
        return None;
    }
    let mut bits = reply.permissions & policy.max_permissions;
    let dir = if bits & perms::FILE != 0 {
        let chosen = reply.receive_dir.trim();
        if chosen.is_empty() { crate::resolve_receive_dir("") } else { chosen.to_string() }
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
async fn service_consent(state: &HostState, name: &str, ip: &str, offered: u32) -> Option<(u32, String)> {
    let dir = crate::syspaths::service_dir();
    let _ = std::fs::create_dir_all(&dir);
    let ask_path = dir.join("consent.json");
    let reply_path = dir.join("consent-reply.json");
    let _ = std::fs::remove_file(&reply_path);
    let ask = serde_json::json!({ "from": name, "ip": ip, "offered": offered });
    std::fs::write(&ask_path, ask.to_string()).ok()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
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
    consent_grant(&state.policy.lock().unwrap(), reply?)
}
