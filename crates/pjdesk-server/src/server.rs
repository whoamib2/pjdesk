//! Signaling server: registration, ID management and session setup.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use pjdesk_proto::framing::{read_msg, write_msg, MAX_SIGNAL_FRAME};
use pjdesk_proto::signal::{ClientMsg, ErrorCode, ServerMsg, SessionId};
use pjdesk_proto::tls::{self, TlsIdentity};
use pjdesk_proto::udp::Role;
use pjdesk_proto::{format_id, normalize_id, ALPN_SIGNAL, PROTOCOL_VERSION, SIGNAL_SERVER_NAME};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

use crate::registry::Registry;
use crate::ServerOpts;

/// Clients must send something (hosts ping every 25 s) within this time.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// Sessions that never complete are forgotten after this long.
const SESSION_TTL: Duration = Duration::from_secs(600);

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// Everything the server knows, behind one mutex (it is only held briefly).
pub struct State {
    pub registry: Registry,
    pub accounts: crate::accounts::Accounts,
    pub online: HashMap<String, OnlineHost>,
    /// Older connections still open for an online ID — normally the app and
    /// the background service of one computer briefly overlapping while one
    /// hands over to the other. If the active connection drops, the newest
    /// still-open one here takes over, so the computer doesn't look offline.
    pub standby: HashMap<String, Vec<OnlineHost>>,
    pub sessions: HashMap<SessionId, Session>,
    pub limiter: RateLimiter,
    pub udp_port: u16,
    /// Data folder, so accounts can be re-read when they change out-of-process.
    pub data_dir: PathBuf,
}

pub type Shared = Arc<Mutex<State>>;

/// A host that is currently connected.
pub struct OnlineHost {
    pub conn_id: u64,
    pub tx: mpsc::UnboundedSender<ServerMsg>,
    pub hostname: String,
    pub app_version: String,
}

/// A connection attempt between a controller and a host.
pub struct Session {
    /// Separate UDP bind tokens, so neither peer can bind as the other.
    pub host_token: [u8; 16],
    pub controller_token: [u8; 16],
    pub host_id: String,
    pub host_name: String,
    pub host_conn: u64,
    pub host_tx: mpsc::UnboundedSender<ServerMsg>,
    pub controller_conn: u64,
    pub controller_tx: mpsc::UnboundedSender<ServerMsg>,
    pub accepted: bool,
    pub host: PeerSlot,
    pub controller: PeerSlot,
    pub info_sent: bool,
    pub created: Instant,
}

/// What we know about one side of a session.
#[derive(Default)]
pub struct PeerSlot {
    /// Source address of its UDP Bind (its public NAT mapping).
    pub public: Option<SocketAddr>,
    /// LAN candidates from its UdpReady message.
    pub local: Option<Vec<SocketAddr>>,
    /// Host only: QUIC certificate fingerprint.
    pub fingerprint: Option<[u8; 32]>,
}

impl Session {
    pub fn token_for(&self, role: Role) -> [u8; 16] {
        match role {
            Role::Host => self.host_token,
            Role::Controller => self.controller_token,
        }
    }

    pub fn slot_mut(&mut self, role: Role) -> &mut PeerSlot {
        match role {
            Role::Host => &mut self.host,
            Role::Controller => &mut self.controller,
        }
    }

    /// Once both peers have bound their UDP socket and reported their LAN
    /// addresses, tell each side how to reach the other.
    pub fn maybe_send_peer_info(&mut self, session_id: SessionId) {
        if self.info_sent {
            return;
        }
        let (Some(host_pub), Some(host_lan), Some(ctrl_pub), Some(ctrl_lan)) =
            (self.host.public, self.host.local.as_ref(), self.controller.public, self.controller.local.as_ref())
        else {
            return;
        };
        let _ = self.host_tx.send(ServerMsg::PeerInfo {
            session_id,
            public_addr: ctrl_pub,
            local_addrs: ctrl_lan.clone(),
            cert_fingerprint: None,
        });
        let _ = self.controller_tx.send(ServerMsg::PeerInfo {
            session_id,
            public_addr: host_pub,
            local_addrs: host_lan.clone(),
            cert_fingerprint: self.host.fingerprint,
        });
        self.info_sent = true;
        info!(
            "Session {session_id:016x}: host {} at {host_pub}, controller at {ctrl_pub}; peers may now connect",
            format_id(&self.host_id)
        );
    }
}

/// Default data folder: %ProgramData%\PJDesk\server on Windows,
/// ~/.local/share/pjdesk-server elsewhere.
pub fn resolve_data_dir(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = explicit {
        return dir;
    }
    if cfg!(windows) {
        let base =
            std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        return base.join("PJDesk").join("server");
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".local/share/pjdesk-server"),
        None => PathBuf::from("pjdesk-server-data"),
    }
}

/// Loads (or creates on first run) the server's TLS identity.
pub fn load_identity(data_dir: &Path) -> Result<TlsIdentity> {
    TlsIdentity::load_or_create(&data_dir.join("server-cert.pem"), &data_dir.join("server-key.pem"), SIGNAL_SERVER_NAME)
}

/// Reads the identity for display only — never creates or regenerates it, so
/// the management window can show the fingerprint without rotating the key.
pub fn read_identity(data_dir: &Path) -> Option<TlsIdentity> {
    TlsIdentity::load(&data_dir.join("server-cert.pem"), &data_dir.join("server-key.pem")).ok()
}

/// Runs the server until `shutdown` completes.
pub async fn run(opts: ServerOpts, data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> Result<()> {
    std::fs::create_dir_all(&data_dir).with_context(|| format!("creating data folder {}", data_dir.display()))?;
    // Repair the data folder's permissions (reset them to the inherited
    // SYSTEM/Administrators/Users ACL) before touching the identity files, so a
    // folder an earlier version left unreadable can't stop the server from
    // starting. Harmless if the service path already did it.
    #[cfg(windows)]
    crate::service::repair_data_dir(&data_dir);
    let identity = load_identity(&data_dir)?;
    let registry = Registry::load(data_dir.join("devices.json"))?;
    let device_count = registry.len();
    let accounts = crate::accounts::Accounts::load(&data_dir)?;
    let account_count = accounts.list().len();

    let tcp = TcpListener::bind((opts.bind, opts.port))
        .await
        .with_context(|| format!("cannot listen on TCP {}:{} (is another server running?)", opts.bind, opts.port))?;
    let udp = UdpSocket::bind((opts.bind, opts.port))
        .await
        .with_context(|| format!("cannot listen on UDP {}:{}", opts.bind, opts.port))?;

    info!("PJDesk server {} started", env!("CARGO_PKG_VERSION"));
    info!("  Signaling (TCP, TLS 1.3) : {}:{}", opts.bind, opts.port);
    info!("  Rendezvous/relay (UDP)   : {}:{}", opts.bind, opts.port);
    info!("  Data folder              : {}", data_dir.display());
    info!("  Registered devices       : {device_count}");
    info!("  User accounts            : {account_count}");
    info!("  Server key fingerprint   : {}", tls::format_fingerprint(&identity.fingerprint()));
    info!("Clients trust this key on first connect; compare it with what they print.");

    let state: Shared = Arc::new(Mutex::new(State {
        registry,
        accounts,
        online: HashMap::new(),
        standby: HashMap::new(),
        sessions: HashMap::new(),
        limiter: RateLimiter::default(),
        udp_port: opts.port,
        data_dir: data_dir.clone(),
    }));
    let acceptor = TlsAcceptor::from(Arc::new(tls::server_config(&identity, ALPN_SIGNAL)?));

    let relay = tokio::spawn(crate::relay::run(Arc::new(udp), state.clone()));
    let janitor = tokio::spawn(janitor(state.clone()));
    let fleet = tokio::spawn(fleet_sync(state.clone()));

    let accept_loop = async {
        loop {
            let (stream, addr) = match tcp.accept().await {
                Ok(v) => v,
                Err(e) => {
                    warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let acceptor = acceptor.clone();
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(acceptor, stream, addr, state).await {
                    debug!("{addr}: connection closed: {e:#}");
                }
            });
        }
    };

    tokio::select! {
        _ = accept_loop => {}
        _ = shutdown => info!("Shutting down"),
        r = relay => warn!("UDP relay stopped: {r:?}"),
    }
    janitor.abort();
    fleet.abort();
    Ok(())
}

/// Per-connection context.
struct Conn {
    id: u64,
    addr: SocketAddr,
    tx: mpsc::UnboundedSender<ServerMsg>,
    host_id: Option<String>,
}

impl Conn {
    fn send(&self, msg: ServerMsg) {
        let _ = self.tx.send(msg);
    }

    fn error(&self, code: ErrorCode, message: impl Into<String>) {
        self.send(ServerMsg::Error { code, message: message.into() });
    }
}

async fn handle_connection(acceptor: TlsAcceptor, tcp: TcpStream, addr: SocketAddr, state: Shared) -> Result<()> {
    tcp.set_nodelay(true)?;
    let tls = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp))
        .await
        .context("TLS handshake timed out")??;
    let (mut reader, mut writer) = tokio::io::split(tls);

    // Outgoing messages go through a channel so other tasks (e.g. the UDP
    // relay, or the other side of a session) can push messages to this client.
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();
    let writer_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_msg(&mut writer, &msg).await.is_err() {
                break;
            }
        }
    });

    let mut conn = Conn { id: NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed), addr, tx, host_id: None };
    debug!("{addr}: connected (conn {})", conn.id);

    let result: Result<()> = async {
        loop {
            let msg: ClientMsg = match tokio::time::timeout(IDLE_TIMEOUT, read_msg(&mut reader, MAX_SIGNAL_FRAME)).await
            {
                Ok(msg) => msg?,
                Err(_) => bail!("idle timeout"),
            };
            handle_message(&state, &mut conn, msg);
        }
    }
    .await;

    // Clean up: a host that disconnects goes offline, unless it already
    // reconnected on a newer connection or another open connection for the
    // same computer can take over. Either way its last-seen time is saved.
    if let Some(id) = &conn.host_id {
        let mut guard = state.lock().unwrap();
        let st = &mut *guard;
        if st.online.get(id).map(|h| h.conn_id) == Some(conn.id) {
            st.online.remove(id);
            let mut replacement = None;
            if let Some(list) = st.standby.get_mut(id) {
                while let Some(h) = list.pop() {
                    if !h.tx.is_closed() {
                        replacement = Some(h);
                        break;
                    }
                }
            }
            match replacement {
                Some(h) => {
                    st.online.insert(id.clone(), h);
                }
                None => info!("Host {} is offline", format_id(id)),
            }
        } else if let Some(list) = st.standby.get_mut(id) {
            list.retain(|h| h.conn_id != conn.id);
        }
        if st.standby.get(id).is_some_and(|l| l.is_empty()) {
            st.standby.remove(id);
        }
        st.registry.touch(std::slice::from_ref(id));
    }
    // Give queued replies (e.g. an error) a moment to go out.
    tokio::time::sleep(Duration::from_millis(200)).await;
    writer_task.abort();
    result
}

fn handle_message(state: &Shared, conn: &mut Conn, msg: ClientMsg) {
    let mut guard = state.lock().unwrap();
    let st = &mut *guard;
    match msg {
        ClientMsg::Ping => conn.send(ServerMsg::Pong),

        ClientMsg::Register { proto, device_id, device_secret, hostname, app_version } => {
            if proto != PROTOCOL_VERSION {
                conn.error(
                    ErrorCode::VersionMismatch,
                    format!("server speaks protocol {PROTOCOL_VERSION}, client {proto}"),
                );
                return;
            }
            if !st.limiter.allow(conn.addr.ip(), "register", 20) {
                conn.error(ErrorCode::RateLimited, "too many registrations, slow down");
                return;
            }
            let hostname: String = hostname.chars().take(64).collect();
            let app_version: String = app_version.chars().take(32).collect();
            match st.registry.register(device_id.as_deref(), &device_secret, &hostname, &app_version) {
                Ok(id) => {
                    let host = OnlineHost {
                        conn_id: conn.id,
                        tx: conn.tx.clone(),
                        hostname: hostname.clone(),
                        app_version: app_version.clone(),
                    };
                    if let Some(prev) = st.online.insert(id.clone(), host) {
                        if prev.conn_id != conn.id && !prev.tx.is_closed() {
                            st.standby.entry(id.clone()).or_default().push(prev);
                        }
                    }
                    conn.host_id = Some(id.clone());
                    conn.send(ServerMsg::Registered { device_id: id.clone() });
                    info!("Host {} \"{hostname}\" online from {} (v{app_version})", format_id(&id), conn.addr.ip());
                    // One record per computer: drop its old, offline records.
                    let online: HashSet<String> = st.online.keys().cloned().collect();
                    let removed = st.registry.remove_stale_duplicates(&id, &online);
                    if removed > 0 {
                        info!("Removed {removed} old record(s) for \"{hostname}\"; it is {} now", format_id(&id));
                    }
                }
                Err(e) => {
                    warn!("{}: registration refused: {e}", conn.addr);
                    conn.error(ErrorCode::IdConflict, e.to_string());
                }
            }
        }

        ClientMsg::Connect { proto, target_id, controller_name } => {
            if proto != PROTOCOL_VERSION {
                conn.error(
                    ErrorCode::VersionMismatch,
                    format!("server speaks protocol {PROTOCOL_VERSION}, client {proto}"),
                );
                return;
            }
            // Throttles ID scanning and password guessing from one address.
            if !st.limiter.allow(conn.addr.ip(), "connect", 30) {
                conn.error(ErrorCode::RateLimited, "too many connection attempts, wait a minute");
                return;
            }
            let target = normalize_id(&target_id);
            let Some(host) = st.online.get(&target) else {
                conn.error(ErrorCode::HostOffline, format!("device {} is not online", format_id(&target)));
                return;
            };
            if host.conn_id == conn.id {
                conn.error(ErrorCode::BadRequest, "a device cannot connect to itself on the same connection");
                return;
            }
            let session_id = pjdesk_proto::random_u64();
            let host_token = pjdesk_proto::random_bytes::<16>();
            let controller_token = pjdesk_proto::random_bytes::<16>();
            let controller_name: String = controller_name.chars().take(64).collect();
            let _ = host.tx.send(ServerMsg::IncomingSession {
                session_id,
                token: host_token,
                udp_port: st.udp_port,
                controller_name: controller_name.clone(),
                controller_ip: conn.addr.ip().to_string(),
            });
            let session = Session {
                host_token,
                controller_token,
                host_id: target.clone(),
                host_name: host.hostname.clone(),
                host_conn: host.conn_id,
                host_tx: host.tx.clone(),
                controller_conn: conn.id,
                controller_tx: conn.tx.clone(),
                accepted: false,
                host: PeerSlot::default(),
                controller: PeerSlot::default(),
                info_sent: false,
                created: Instant::now(),
            };
            st.sessions.insert(session_id, session);
            info!(
                "Session {session_id:016x}: \"{controller_name}\" ({}) → host {}",
                conn.addr.ip(),
                format_id(&target)
            );
        }

        ClientMsg::SessionResponse { session_id, accept, reason } => {
            let Some(session) = st.sessions.get_mut(&session_id) else { return };
            if session.host_conn != conn.id {
                return;
            }
            if accept {
                session.accepted = true;
                let _ = session.controller_tx.send(ServerMsg::SessionReady {
                    session_id,
                    token: session.controller_token,
                    udp_port: st.udp_port,
                    host_name: session.host_name.clone(),
                });
            } else {
                let _ = session.controller_tx.send(ServerMsg::Error { code: ErrorCode::HostBusy, message: reason });
                st.sessions.remove(&session_id);
            }
        }

        ClientMsg::UdpReady { session_id, local_addrs, cert_fingerprint } => {
            let Some(session) = st.sessions.get_mut(&session_id) else { return };
            let role = if session.host_conn == conn.id {
                Role::Host
            } else if session.controller_conn == conn.id {
                Role::Controller
            } else {
                return;
            };
            let slot = session.slot_mut(role);
            slot.local = Some(local_addrs.into_iter().take(8).collect());
            if role == Role::Host {
                slot.fingerprint = cert_fingerprint;
            }
            session.maybe_send_peer_info(session_id);
        }

        ClientMsg::Login { proto, username, credential, device_name } => {
            if proto != PROTOCOL_VERSION {
                conn.error(
                    ErrorCode::VersionMismatch,
                    format!("server speaks protocol {PROTOCOL_VERSION}, client {proto}"),
                );
                return;
            }
            // Throttle password guessing per IP.
            if !st.limiter.allow(conn.addr.ip(), "login", 15) {
                conn.error(ErrorCode::RateLimited, "too many sign-in attempts, wait a minute");
                return;
            }
            // Pick up accounts added or changed out-of-process (the management
            // window or the CLI) since startup, so a new sign-in works without
            // restarting the service.
            {
                let dir = st.data_dir.clone();
                if let Ok(fresh) = crate::accounts::Accounts::load(&dir) {
                    st.accounts = fresh;
                }
            }
            let device_name: String = device_name.chars().take(64).collect();
            let result = match &credential {
                pjdesk_proto::signal::Credential::Password(pw) => {
                    st.accounts.login_password(&username, pw, &device_name)
                }
                pjdesk_proto::signal::Credential::Token(token) => st.accounts.login_token(&username, token),
            };
            match result {
                Ok(session) => {
                    info!("Account \"{}\" signed in from {} ({device_name})", session.username, conn.addr.ip());
                    conn.send(ServerMsg::LoginResult {
                        ok: true,
                        token: session.token,
                        display_name: session.display_name,
                        is_admin: session.is_admin,
                        message: String::new(),
                    });
                }
                Err(e) => {
                    conn.send(ServerMsg::LoginResult {
                        ok: false,
                        token: String::new(),
                        display_name: String::new(),
                        is_admin: false,
                        message: e.to_string(),
                    });
                }
            }
        }

        ClientMsg::Logout { token } => st.accounts.logout(&token),

        ClientMsg::GetAddressBook { token } => match st.accounts.user_for_token(&token) {
            Some(user) => conn.send(ServerMsg::AddressBook { entries: st.accounts.address_book(&user) }),
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::SaveAddressBook { token, entries } => match st.accounts.user_for_token(&token) {
            Some(user) => {
                let entries: Vec<_> = entries.into_iter().take(2000).collect();
                if let Err(e) = st.accounts.save_address_book(&user, &entries) {
                    conn.error(ErrorCode::Internal, format!("could not save the address book: {e}"));
                }
            }
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },
    }
}

/// Periodically forgets stale sessions and rate-limit counters.
async fn janitor(state: Shared) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let mut st = state.lock().unwrap();
        st.sessions.retain(|_, s| s.created.elapsed() < SESSION_TTL);
        st.limiter.sweep();
        st.standby.retain(|_, list| {
            list.retain(|h| !h.tx.is_closed());
            !list.is_empty()
        });
    }
}

/// One online device, published to `online.json` for the management window.
#[derive(serde::Serialize)]
struct OnlineInfo {
    id: String,
    hostname: String,
    version: String,
}

/// Publishes the live set of online devices to `online.json`, carries out the
/// update and forget requests the management window leaves (see
/// [`crate::requests`]), and once a minute saves the last-seen time of every
/// connected device so it stays accurate however long they stay online.
async fn fleet_sync(state: Shared) {
    use crate::requests::{take, FORGET_FILE, UPDATE_FILE};
    const TOUCH_EVERY: u32 = 30; // ticks of 2 s
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut ticks: u32 = 0;
    loop {
        tick.tick().await;
        ticks = ticks.wrapping_add(1);
        let (data_dir, snapshot) = {
            let mut guard = state.lock().unwrap();
            let st = &mut *guard;
            if ticks % TOUCH_EVERY == 0 {
                let ids: Vec<String> = st.online.keys().cloned().collect();
                st.registry.touch(&ids);
            }
            let snapshot: Vec<OnlineInfo> = st
                .online
                .iter()
                .map(|(id, h)| OnlineInfo {
                    id: id.clone(),
                    hostname: h.hostname.clone(),
                    version: h.app_version.clone(),
                })
                .collect();
            (st.data_dir.clone(), snapshot)
        };
        let online_path = data_dir.join("online.json");
        if let Ok(bytes) = serde_json::to_vec(&snapshot) {
            let tmp = online_path.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &online_path);
            }
        }

        let updates = take(&data_dir, UPDATE_FILE);
        let forgets = take(&data_dir, FORGET_FILE);
        if updates.is_empty() && forgets.is_empty() {
            continue;
        }
        let mut st = state.lock().unwrap();
        for req in &updates {
            if req == "*" {
                for h in st.online.values() {
                    let _ = h.tx.send(ServerMsg::UpdateNow);
                }
            } else if let Some(h) = st.online.get(req) {
                let _ = h.tx.send(ServerMsg::UpdateNow);
            }
        }
        for id in &forgets {
            if let Err(e) = st.registry.remove(id) {
                warn!("could not forget {}: {e}", format_id(id));
            } else {
                info!("Forgot device {}", format_id(id));
            }
        }
    }
}

/// Fixed-window per-IP rate limiter.
#[derive(Default)]
pub struct RateLimiter {
    windows: HashMap<(IpAddr, &'static str), (Instant, u32)>,
}

impl RateLimiter {
    /// Allows at most `per_minute` events of `kind` per IP per minute.
    pub fn allow(&mut self, ip: IpAddr, kind: &'static str, per_minute: u32) -> bool {
        let now = Instant::now();
        let entry = self.windows.entry((ip, kind)).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= per_minute
    }

    pub fn sweep(&mut self) {
        self.windows.retain(|_, (start, _)| start.elapsed() < Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_blocks_after_limit() {
        let mut limiter = RateLimiter::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..5 {
            assert!(limiter.allow(ip, "x", 5));
        }
        assert!(!limiter.allow(ip, "x", 5));
        assert!(limiter.allow(ip, "y", 5));
    }
}
