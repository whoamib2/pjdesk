//! Signaling server: registration, ID management and session setup.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use pjdesk_proto::framing::{read_msg, write_msg, MAX_SIGNAL_FRAME};
use pjdesk_proto::signal::{ClientMsg, ErrorCode, ServerMsg, SessionId};
use pjdesk_proto::signal::{DeviceBrief, Role as AccountRole};
use pjdesk_proto::tls::{self, TlsIdentity};
use pjdesk_proto::udp::Role;
use pjdesk_proto::{
    format_id, normalize_id, protocol_ok, ALPN_SIGNAL, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, SIGNAL_SERVER_NAME,
};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, Notify};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

use crate::registry::Registry;
use crate::ServerOpts;

/// Clients must send something within this time. Current hosts ping every 2 s
/// and reconnect after a few seconds of silence. 0.6.26 hosts still ping every
/// 25 s, so this stays at 90 s and does not drop them.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// How long Connect waits when the computer was registered recently but is
/// missing from the live table. It is usually reconnecting.
const CONNECT_WAIT: Duration = Duration::from_secs(12);
const CONNECT_POLL: Duration = Duration::from_millis(250);
/// A last-seen time inside this window is recent enough to wait.
const CONNECT_RECENT_SECS: u64 = 120;

/// True when `last_seen` is non-zero and no older than [`CONNECT_RECENT_SECS`].
fn host_was_seen_recently(last_seen: u64, now: u64) -> bool {
    last_seen > 0 && now.saturating_sub(last_seen) <= CONNECT_RECENT_SECS
}
/// Sessions that never complete are forgotten after this long.
const SESSION_TTL: Duration = Duration::from_secs(600);
/// Open sessions one address may have at once. Past this, further connection
/// attempts are refused so one source cannot fill the relay.
const MAX_SESSIONS_PER_IP: usize = 8;
/// Hard cap on sessions the process will track.
const MAX_SESSIONS: usize = 2000;
/// Signaling messages one connection may send in a second before it is dropped.
const MAX_MSGS_PER_SEC: u32 = 40;
/// Open TCP connections, process-wide and per source address. Past this, new
/// connections are dropped before the TLS handshake so a flood cannot spawn
/// unbounded tasks.
const MAX_CONNECTIONS: usize = 1024;
const MAX_CONNECTIONS_PER_IP: usize = 32;

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
    /// Set while relayed sessions are moving to the new primary.
    /// Step-down is written only after they have followed, or after a short wait.
    pub pending_move: Option<PendingMove>,
    pub limiter: RateLimiter,
    pub udp_port: u16,
    /// Data folder, so accounts can be re-read when they change out-of-process.
    pub data_dir: PathBuf,
    /// The relay's UDP socket, so packets from a peer relaying over TCP can be
    /// sent on to a peer that uses UDP. `None` in tests.
    pub udp_socket: Option<Arc<UdpSocket>>,
    /// UDP address of a peer whose other side relays over TCP → its session.
    pub tcp_routes: HashMap<SocketAddr, SessionId>,
}

pub type Shared = Arc<Mutex<State>>;

/// A host that is currently connected.
pub struct OnlineHost {
    pub conn_id: u64,
    pub tx: mpsc::UnboundedSender<ServerMsg>,
    pub hostname: String,
    pub app_version: String,
    /// Address the registration arrived from, so a second registration of the
    /// same device from somewhere else can be logged.
    pub addr: SocketAddr,
    /// Protocol the host registered with. New server messages go only to version 3.
    pub proto: u16,
    /// When this connection became the active registration.
    pub since: Instant,
    /// Wakes the connection task so a removed computer drops immediately.
    pub close: Arc<Notify>,
    /// Access key this registration presented. Empty when it used an account or grace.
    pub access_key_id: String,
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
    pub controller_ip: IpAddr,
    pub controller_tx: mpsc::UnboundedSender<ServerMsg>,
    pub accepted: bool,
    pub host: PeerSlot,
    pub controller: PeerSlot,
    pub info_sent: bool,
    pub created: Instant,
    /// Both peers bound, so the UDP relay is carrying this session.
    pub relayed: bool,
    /// [`ServerMsg::MoveRelay`] was sent to the host.
    pub move_told: bool,
    /// The host reported [`ClientMsg::RelayMoved`].
    pub move_acked: bool,
    /// The relay pieces last logged as missing, so each change is logged once.
    pub wait_logged: String,
}

/// A role change that is waiting for relayed sessions to leave.
pub struct PendingMove {
    pub primary: String,
    pub since: Instant,
    pub kind: StepKind,
}

/// The other server already owns the next generation. Sessions move, then
/// this server records standby.
pub enum StepKind {
    Yield { epoch: u64, instance: String, name: String },
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
    /// 0.6.67+: this side relays its QUIC packets over its TLS connection.
    pub tcp: bool,
    /// Bytes relayed toward this side over TCP, and the current one-second
    /// window, so a fast UDP sender cannot queue without bound.
    pub tcp_bytes: u64,
    pub tcp_window: Option<(Instant, u64)>,
}

/// The public address recorded for a side that relays over TCP. Peers skip it
/// as a direct candidate (QUIC refuses port 0) and use the relay.
pub const TCP_PLACEHOLDER: SocketAddr = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);
/// Most a TCP side may be sent per second before packets are dropped (QUIC
/// then slows down). About 24 Mbit/s.
pub const TCP_RELAY_BYTES_PER_SEC: u64 = 3 * 1024 * 1024;

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
    /// What the relay still needs before it can send PeerInfo. Empty when ready.
    pub fn relay_missing(&self) -> Vec<&'static str> {
        pjdesk_proto::signal::relay_missing(
            self.host.public.is_some(),
            self.host.local.is_some(),
            self.controller.public.is_some(),
            self.controller.local.is_some(),
        )
    }

    pub fn maybe_send_peer_info(&mut self, session_id: SessionId) {
        if self.info_sent {
            return;
        }
        let missing = self.relay_missing();
        if !missing.is_empty() {
            let text = missing.join(", ");
            if self.accepted && text != self.wait_logged {
                info!(
                    "Session {session_id:016x}: host {} relay setup is waiting for {text}",
                    format_id(&self.host_id)
                );
                self.wait_logged = text;
            }
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

/// When one side of a session relays over TCP and the other has bound UDP,
/// packets from that UDP address go to the TCP side.
pub fn link_tcp_route(st: &mut State, session_id: SessionId) {
    let Some(session) = st.sessions.get(&session_id) else { return };
    for (tcp_side, udp_side) in [(&session.host, &session.controller), (&session.controller, &session.host)] {
        if tcp_side.tcp && !udp_side.tcp {
            if let Some(addr) = udp_side.public.filter(|a| *a != TCP_PLACEHOLDER) {
                st.tcp_routes.insert(addr, session_id);
            }
        }
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700));
    }
    // Repair the data folder's permissions (SYSTEM and Administrators only)
    // before touching the identity files, so a folder an earlier version left
    // unreadable — or readable by every local user — can't stop the server
    // or leak its key. Harmless if the service path already did it.
    #[cfg(windows)]
    crate::service::repair_data_dir(&data_dir);
    crate::public_addr::remember_port(&data_dir, opts.port);
    let identity = load_identity(&data_dir)?;
    crate::access::remember_fingerprint(&data_dir, &identity.fingerprint());
    if crate::access::restore_grace(&data_dir) {
        info!("Require signed-in accounts was on without being chosen. Grace mode is on again, so existing computers can connect.");
    }
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

    crate::self_update::note_installer_result(&data_dir);
    info!("PJDesk server {} started", env!("CARGO_PKG_VERSION"));
    info!("  Signaling (TCP, TLS 1.3) : {}:{}", opts.bind, opts.port);
    info!("  Rendezvous/relay (UDP)   : {}:{}", opts.bind, opts.port);
    info!("  Data folder              : {}", data_dir.display());
    info!("  Registered devices       : {device_count}");
    if account_count == 0 {
        info!("  Local accounts           : none (a PJDesk account is the server sign-in)");
    } else {
        info!("  Local accounts (legacy)  : {account_count}");
    }
    info!("  Server key fingerprint   : {}", tls::format_fingerprint(&identity.fingerprint()));
    info!("Clients trust this key on first connect; compare it with what they print.");

    let state: Shared = Arc::new(Mutex::new(State {
        registry,
        accounts,
        online: HashMap::new(),
        standby: HashMap::new(),
        sessions: HashMap::new(),
        pending_move: None,
        limiter: RateLimiter::default(),
        udp_port: opts.port,
        data_dir: data_dir.clone(),
        udp_socket: None,
        tcp_routes: HashMap::new(),
    }));
    // Swappable, so a standby server can take on the key it syncs from the cloud.
    let acceptor = Arc::new(RwLock::new(TlsAcceptor::from(Arc::new(tls::server_config(&identity, ALPN_SIGNAL)?))));

    let beacon_port = opts.port;
    let beacon_fp = identity.fingerprint();
    let beacon_dir = data_dir.clone();
    let _ = std::thread::Builder::new().name("lan-beacon".into()).spawn(move || {
        crate::public_addr::announce_lan(beacon_port, beacon_fp, beacon_dir);
    });

    let udp = Arc::new(udp);
    state.lock().unwrap().udp_socket = Some(udp.clone());
    let relay = tokio::spawn(crate::relay::run(udp, state.clone()));
    let janitor = tokio::spawn(janitor(state.clone()));
    let fleet = tokio::spawn(fleet_sync(state.clone()));
    let cloud = tokio::spawn(cloud_sync(state.clone(), acceptor.clone()));
    let failover = tokio::spawn(failover_watch(state.clone(), acceptor.clone()));
    let updates = tokio::spawn(server_updates(data_dir.clone()));
    // The management window reads this when it cannot open the data folder.
    {
        let pipe_dir = data_dir.clone();
        let pipe_port = opts.port;
        let _ = std::thread::Builder::new().name("status-pipe".into()).spawn(move || {
            crate::status_pipe::serve(pipe_dir, pipe_port);
        });
        let control_dir = data_dir.clone();
        let _ = std::thread::Builder::new().name("control-pipe".into()).spawn(move || {
            crate::control::serve(control_dir);
        });
    }
    crate::self_update::retire_legacy_task_if_disabled(&data_dir);
    let budget = Arc::new(Mutex::new(ConnBudget::default()));

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
            let Some(permit) = ConnBudget::try_acquire(&budget, addr.ip()) else {
                debug!("{addr}: connection refused (too many open)");
                continue;
            };
            let acceptor = acceptor.read().unwrap().clone();
            let state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(e) = handle_connection(acceptor, stream, addr, state).await {
                    debug!("{addr}: connection closed: {e:#}");
                }
            });
        }
    };

    tokio::select! {
        _ = accept_loop => {}
        _ = shutdown => {
            info!("Shutting down");
            release_relays_on_stop(&state).await;
        }
        r = relay => warn!("UDP relay stopped: {r:?}"),
    }
    janitor.abort();
    fleet.abort();
    cloud.abort();
    failover.abort();
    updates.abort();
    Ok(())
}

/// Tracks how many TCP connections are open so a flood is dropped at accept.
#[derive(Default)]
struct ConnBudget {
    total: usize,
    per_ip: HashMap<IpAddr, usize>,
}

struct ConnPermit {
    ip: IpAddr,
    budget: Arc<Mutex<ConnBudget>>,
}

impl ConnBudget {
    fn try_acquire(this: &Arc<Mutex<Self>>, ip: IpAddr) -> Option<ConnPermit> {
        let mut guard = this.lock().unwrap();
        if guard.total >= MAX_CONNECTIONS {
            return None;
        }
        let n = guard.per_ip.get(&ip).copied().unwrap_or(0);
        if n >= MAX_CONNECTIONS_PER_IP {
            return None;
        }
        guard.total += 1;
        guard.per_ip.insert(ip, n + 1);
        Some(ConnPermit { ip, budget: Arc::clone(this) })
    }
}

impl Drop for ConnPermit {
    fn drop(&mut self) {
        let mut guard = self.budget.lock().unwrap();
        guard.total = guard.total.saturating_sub(1);
        if let Some(n) = guard.per_ip.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                guard.per_ip.remove(&self.ip);
            }
        }
    }
}

/// Per-connection context.
struct Conn {
    id: u64,
    addr: SocketAddr,
    tx: mpsc::UnboundedSender<ServerMsg>,
    host_id: Option<String>,
    /// Highest protocol version this connection has offered.
    proto: u16,
    /// Network access token from [`ClientMsg::PresentAccess`], if this peer sent one.
    access_token: String,
    /// Set when this computer is removed. The read loop ends on the next wait.
    close: Arc<Notify>,
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
    let close = Arc::new(Notify::new());
    let writer_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_msg(&mut writer, &msg).await.is_err() {
                break;
            }
        }
    });

    let mut conn = Conn {
        id: NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed),
        addr,
        tx,
        host_id: None,
        proto: 0,
        access_token: String::new(),
        close,
    };
    debug!("{addr}: connected (conn {})", conn.id);

    let result: Result<()> = async {
        let mut burst_start = Instant::now();
        let mut burst = 0u32;
        loop {
            let close = Arc::clone(&conn.close);
            let closed = close.notified();
            let closed = std::pin::pin!(closed);
            let msg: ClientMsg = tokio::select! {
                biased;
                _ = closed => bail!("removed from this server"),
                msg = tokio::time::timeout(IDLE_TIMEOUT, read_msg(&mut reader, MAX_SIGNAL_FRAME)) => {
                    match msg {
                        Ok(msg) => msg?,
                        Err(_) => bail!("idle timeout"),
                    }
                }
            };
            // Relayed QUIC packets are many per second and rate-limited by bytes.
            if let ClientMsg::RelayData { session_id, data } = msg {
                crate::relay::from_tcp(&state, conn.id, session_id, data);
                continue;
            }
            // A single connection that sprays messages holds the process-wide
            // lock on every one of them. Drop it instead of queuing forever.
            if burst_start.elapsed() >= Duration::from_secs(1) {
                burst_start = Instant::now();
                burst = 0;
            }
            burst += 1;
            if burst > MAX_MSGS_PER_SEC {
                bail!("too many messages");
            }
            if let ClientMsg::TakePrimary { epoch, at, proof } = msg {
                let dir = state.lock().unwrap().data_dir.clone();
                let reply = tokio::task::spawn_blocking(move || crate::role_handoff::accept_take_primary(&dir, epoch, at, &proof))
                    .await
                    .unwrap_or_else(|_| crate::role_handoff::TakeReply {
                        ok: false,
                        epoch: 0,
                        name: String::new(),
                        message: "This server could not take the primary.".into(),
                    });
                conn.send(ServerMsg::Handoff {
                    ok: reply.ok,
                    epoch: reply.epoch,
                    name: reply.name,
                    message: reply.message,
                });
                continue;
            }
            if let ClientMsg::SetIncomingOnly { on, password } = &msg {
                let (dir, host_id, currently) = {
                    let st = state.lock().unwrap();
                    let host_id = conn.host_id.clone();
                    let currently = host_id.as_deref().is_some_and(|id| st.registry.incoming_only(id));
                    (st.data_dir.clone(), host_id, currently)
                };
                let Some(host_id) = host_id else {
                    conn.error(ErrorCode::Unauthorized, "This computer is not registered.");
                    continue;
                };
                let allowed = if *on || !currently {
                    true
                } else {
                    let dir = dir.clone();
                    let password = password.clone();
                    tokio::task::spawn_blocking(move || incoming_password_ok(&dir, &password))
                        .await
                        .unwrap_or(false)
                };
                if !allowed {
                    conn.error(
                        ErrorCode::Unauthorized,
                        "Enter the account owner's password to allow this computer to connect to others.",
                    );
                    continue;
                }
                let mut st = state.lock().unwrap();
                if let Err(e) = st.registry.set_incoming_only(&host_id, *on) {
                    conn.error(ErrorCode::BadRequest, e.to_string());
                    continue;
                }
                conn.send(ServerMsg::IncomingOnly { on: *on });
                crate::audit::AuditLog::open(&st.data_dir).record(
                    "incoming_only",
                    &host_id,
                    if *on { "on" } else { "off" },
                );
                continue;
            }
            if let ClientMsg::Account { .. } = &msg {
                // Before registration and before the Require gate. A 0.6.52 server
                // already answers Account on a connection that has not registered.
                let dir = state.lock().unwrap().data_dir.clone();
                let fleet_dir = dir.clone();
                let request = msg;
                let reply = tokio::task::spawn_blocking(move || crate::account_mirror::answer(&dir, request))
                    .await
                    .unwrap_or_else(|_| {
                        ServerMsg::AccountResult {
                            ok: false,
                            op: String::new(),
                            message: "This server could not check the account.".into(),
                            access: String::new(),
                            exp: 0,
                            refresh: String::new(),
                            email: String::new(),
                            role: String::new(),
                            device_id: String::new(),
                            vault: String::new(),
                            vault_updated: 0,
                            app_lock: String::new(),
                            app_lock_updated: 0,
                            computers: Vec::new(),
                            removed: Vec::new(),
                            pause_secs: 0,
                            active_address: String::new(),
                            active_fingerprint: String::new(),
                            standby_address: String::new(),
                            standby_fingerprint: String::new(),
                            active_lan: String::new(),
                            standby_lan: String::new(),
                            active_public_ip: String::new(),
                            standby_public_ip: String::new(),
                        }
                    });
                if matches!(reply, ServerMsg::AccountResult { ok: true, .. }) {
                    if let Some(fleet) = crate::account_mirror::fleet_for(&fleet_dir, "") {
                        conn.send(fleet);
                    }
                }
                conn.send(reply);
                continue;
            }
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
            let held = st.online.get(id).map(|h| h.since.elapsed().as_secs()).unwrap_or(0);
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
                    info!(
                        "Host {} dropped its connection after {held}s; another PJDesk process on this computer took over",
                        format_id(id)
                    );
                    st.online.insert(id.clone(), h);
                }
                None => info!("Host {} is offline after {held}s", format_id(id)),
            }
        } else if let Some(list) = st.standby.get_mut(id) {
            if list.iter().any(|h| h.conn_id == conn.id) {
                info!("Host {} closed a standby connection from {}", format_id(id), conn.addr.ip());
            }
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

fn handle_login(
    state: &Shared,
    conn: &mut Conn,
    proto: u16,
    username: String,
    credential: pjdesk_proto::signal::Credential,
    device_name: String,
) {
    if !note_proto(conn, proto) {
        return;
    }
    let data_dir = {
        let mut guard = state.lock().unwrap();
        if !guard.limiter.allow(conn.addr.ip(), "login", 15) {
            conn.error(ErrorCode::RateLimited, "too many sign-in attempts, wait a minute");
            return;
        }
        // Pick up accounts added out of process (the management window or the
        // CLI) so a new sign-in works without restarting the service.
        if let Ok(fresh) = crate::accounts::Accounts::load(&guard.data_dir) {
            guard.accounts = fresh;
        }
        guard.data_dir.clone()
    };
    let device_name: String = device_name.chars().take(64).collect();
    let result = match credential {
        pjdesk_proto::signal::Credential::Password(pw) => {
            let prep = state.lock().unwrap().accounts.prep_password_login(&username);
            // Argon2 runs here, with the server lock released.
            let matched = crate::accounts::verify_password(&prep.hash, &pw).is_ok();
            let mut guard = state.lock().unwrap();
            if let Ok(fresh) = crate::accounts::Accounts::load(&data_dir) {
                guard.accounts = fresh;
            }
            if matched {
                let session = guard.accounts.commit_password_login(&prep);
                if session.is_ok() && crate::password_policy::is_weak(&pw, &username) {
                    conn.send(ServerMsg::LoginResult {
                        ok: true,
                        token: session.as_ref().map(|s| s.token.clone()).unwrap_or_default(),
                        display_name: session.as_ref().map(|s| s.display_name.clone()).unwrap_or_default(),
                        is_admin: session.as_ref().map(|s| s.is_admin).unwrap_or(false),
                        message: crate::password_policy::weak_login_message().into(),
                    });
                    let user = session.as_ref().map(|s| s.username.clone()).unwrap_or_else(|_| username.clone());
                    drop(guard);
                    crate::audit::AuditLog::open(&data_dir).record(
                        "login",
                        &user,
                        "password accepted; weaker than the current policy",
                    );
                    info!("Account \"{user}\" signed in with a password that no longer meets policy");
                    return;
                }
                session
            } else {
                Err(anyhow::anyhow!("wrong username or password"))
            }
        }
        pjdesk_proto::signal::Credential::Token(token) => {
            let mut guard = state.lock().unwrap();
            guard.accounts.login_token(&username, &token)
        }
    };
    let data_dir = state.lock().unwrap().data_dir.clone();
    match result {
        Ok(session) => {
            crate::audit::AuditLog::open(&data_dir).record(
                "login",
                &session.username,
                &format!("from {} ({device_name})", conn.addr.ip()),
            );
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
            crate::audit::AuditLog::open(&data_dir).record("login_fail", &username, "rejected");
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

/// How this connection was admitted.
enum Admitted {
    /// Grace: no token, and signed-in accounts are not required yet.
    Open,
    Signed(pjdesk_proto::access::Claims),
    Key(crate::access_keys::LiveKey),
}

/// Loads the policy and checks this connection's token.
/// An access key that is unknown, revoked, or expired is refused even during grace.
/// `Err` is "Sign in required" for Connect. Register does not use that refusal:
/// a computer with no token still receives an ID so it can sign in.
/// The computer limit is checked by the caller on Register.
fn gate(st: &State, conn: &Conn) -> Result<Admitted, ()> {
    let token = conn.access_token.trim();
    if pjdesk_proto::access::is_access_key(token) {
        return match crate::access_keys::Store::check(&st.data_dir, token, crate::access::now_unix()) {
            Ok(key) => Ok(Admitted::Key(key)),
            Err(reason) => {
                warn!("{}: access key refused ({reason})", conn.addr);
                Err(())
            }
        };
    }
    let policy = crate::access::Policy::load(&st.data_dir);
    let mirror = crate::account_mirror::current_sign_public(&st.data_dir);
    match crate::access::admit_keys(&policy, token, crate::access::now_unix(), &mirror) {
        Ok(crate::access::Decision::Open) => Ok(Admitted::Open),
        Ok(crate::access::Decision::Signed(claims)) => Ok(Admitted::Signed(claims)),
        Err(reason) => {
            warn!("{}: network access refused ({reason})", conn.addr);
            Err(())
        }
    }
}

/// True when this connection's token belongs to the account the server is signed in as.
fn owner_token(st: &State, conn: &Conn) -> bool {
    let owner = crate::cloud::saved_account_email(&st.data_dir);
    if owner.is_empty() {
        return false;
    }
    match gate(st, conn) {
        Ok(Admitted::Signed(claims)) => claims.email.eq_ignore_ascii_case(owner.trim()),
        _ => false,
    }
}

/// An account password sign-in, or a token for that account, removes a block
/// so the same computer can be given an ID again.
fn registration_is_blocked(st: &mut State, conn: &Conn, secrets: &[&[u8; 32]], requested: Option<&str>, app_version: &str) -> bool {
    let lift = owner_token(st, conn) || crate::account_mirror::owner_reenroll_open(&st.data_dir);
    if lift {
        for secret in secrets {
            if st.registry.unblock_secret(secret) {
                info!("{}: account sign-in removed the block on this computer", conn.addr);
            }
        }
        if let Some(id) = requested {
            if st.registry.is_blocked_id(id) {
                if st.registry.unblock(id).unwrap_or(false) {
                    info!("{}: account sign-in unblocked {}", conn.addr, format_id(id));
                }
            }
        }
    }
    let blocked = requested.is_some_and(|id| st.registry.is_blocked_id(id))
        || secrets.iter().any(|secret| st.registry.is_blocked_secret(secret));
    if !blocked {
        return false;
    }
    offer_routes(st, conn, app_version);
    conn.error(ErrorCode::Unauthorized, crate::registry::Registry::BLOCKED_MESSAGE);
    true
}

/// Which side of `session_id` this connection is, if either.
fn session_role(st: &State, session_id: SessionId, conn: &Conn) -> Option<Role> {
    let session = st.sessions.get(&session_id)?;
    if session.host_conn == conn.id {
        Some(Role::Host)
    } else if session.controller_conn == conn.id {
        Some(Role::Controller)
    } else {
        None
    }
}

/// Whether a session's UDP/relay setup message is allowed. Only the controller
/// is gated: the host is the registered computer the (already gated) controller
/// asked for, and registration needs no sign-in. Gating the host stranded every
/// session to a computer whose service had no usable token (0.6.68 fix).
fn setup_allowed(st: &State, conn: &Conn, role: Role) -> bool {
    match role {
        Role::Host => true,
        Role::Controller => gate(st, conn).is_ok(),
    }
}

/// Require signed-in accounts does not refuse registration. A computer with no
/// token is given an ID (pending, not signed in) so it can sign in afterwards.
/// Connect still requires a token when the switch is on.
fn admit_registration(st: &State, conn: &Conn) -> Admitted {
    match gate(st, conn) {
        Ok(admitted) => admitted,
        Err(()) => {
            info!(
                "{}: registration has no usable sign-in yet; issuing an ID so this computer can sign in",
                conn.addr
            );
            Admitted::Open
        }
    }
}

/// Names this server before any registration reply. An older client cannot
/// decode the new message, so it is sent only when the client's own version can.
fn offer_server_version(conn: &Conn, app_version: &str) {
    if pjdesk_proto::update::version_at_least(app_version, "0.6.52") {
        conn.send(ServerMsg::ServerVersion { version: env!("CARGO_PKG_VERSION").to_string() });
    }
}

fn offer_routes(st: &State, conn: &Conn, app_version: &str) {
    if pjdesk_proto::update::version_at_least(app_version, "0.6.45") {
        conn.send(ServerMsg::Routes(crate::cloud::advertised_routes(&st.data_dir)));
    }
}

fn manage_access_keys(
    st: &mut State,
    conn: &Conn,
    token: &str,
    op: &str,
    label: &str,
    expires_secs: u64,
    computer_limit: u32,
    key_id: &str,
) {
    let actor = match owner_email(st, token) {
        Ok(email) => email,
        Err(message) => {
            conn.send(ServerMsg::AccessKeys { ok: false, message, secret: String::new(), keys: Vec::new() });
            return;
        }
    };
    let op = op.trim();
    let outcome = match op {
        "list" => Ok((String::new(), crate::access_keys::Store::public_list(&st.data_dir))),
        "create" => match crate::access_keys::Store::create(&st.data_dir, label, expires_secs, computer_limit) {
            Ok(secret) => Ok((secret, crate::access_keys::Store::public_list(&st.data_dir))),
            Err(message) => Err(message),
        },
        "revoke" => match crate::access_keys::Store::revoke(&st.data_dir, key_id) {
            Ok(()) => {
                let _ = drop_access_key(st, key_id.trim());
                crate::audit::AuditLog::open(&st.data_dir).record("access_key", &actor, "revoked");
                Ok((String::new(), crate::access_keys::Store::public_list(&st.data_dir)))
            }
            Err(message) => Err(message),
        },
        _ => Err("That was not understood.".into()),
    };
    match outcome {
        Ok((secret, keys)) => {
            if op == "create" {
                crate::audit::AuditLog::open(&st.data_dir).record("access_key", &actor, &format!("created {label}"));
            }
            let message = if secret.is_empty() {
                String::new()
            } else {
                "Copy this key now. It is shown once.".into()
            };
            conn.send(ServerMsg::AccessKeys { ok: true, message, secret, keys });
        }
        Err(message) => conn.send(ServerMsg::AccessKeys { ok: false, message, secret: String::new(), keys: Vec::new() }),
    }
}

/// The presented network token must name the account this server is signed in to.
fn owner_email(st: &State, token: &str) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() || pjdesk_proto::access::is_access_key(token) {
        return Err("Sign in on this computer as the owner of this server.".into());
    }
    let policy = crate::access::Policy::load(&st.data_dir);
    let mirror = crate::account_mirror::current_sign_public(&st.data_dir);
    if policy.public_key.trim().len() != 64 && mirror.is_empty() && policy.trusted_keys.is_empty() {
        return Err("This server has not checked in with pjdesk.com yet, so it cannot confirm the owner.".into());
    }
    let claims = match crate::access::admit_keys(&policy, token, crate::access::now_unix(), &mirror) {
        Ok(crate::access::Decision::Signed(claims)) => claims,
        _ => return Err("Sign in again. This server could not confirm the account.".into()),
    };
    let Some(cfg) = crate::cloud::CloudConfig::load(&st.data_dir) else {
        return Err("This server is not signed in to a PJDesk account.".into());
    };
    if !cfg.is_account() || cfg.email.trim().is_empty() {
        return Err("This server is not signed in to a PJDesk account.".into());
    }
    if !cfg.email.trim().eq_ignore_ascii_case(claims.email.trim()) {
        return Err("Only the account this server is signed in to can manage access keys.".into());
    }
    Ok(claims.email)
}

struct ControllerSide {
    conn_id: u64,
    addr: SocketAddr,
    tx: mpsc::UnboundedSender<ServerMsg>,
}

fn offline_message(target: &str) -> String {
    format!("device {} is not online", format_id(target))
}

fn send_error(tx: &mpsc::UnboundedSender<ServerMsg>, code: ErrorCode, message: impl Into<String>) {
    let _ = tx.send(ServerMsg::Error { code, message: message.into() });
}

/// Opens a session when `target` is in the live table. The caller has already
/// checked the network-access gate and the connect rate limit.
fn start_session(st: &mut State, ctrl: &ControllerSide, target: &str, controller_name: &str) {
    let Some(host) = st.online.get(target) else {
        send_error(&ctrl.tx, ErrorCode::HostOffline, offline_message(target));
        return;
    };
    if host.conn_id == ctrl.conn_id {
        send_error(&ctrl.tx, ErrorCode::BadRequest, "a device cannot connect to itself on the same connection");
        return;
    }
    let from_here = st.sessions.values().filter(|s| s.controller_ip == ctrl.addr.ip()).count();
    if from_here >= MAX_SESSIONS_PER_IP || st.sessions.len() >= MAX_SESSIONS {
        send_error(&ctrl.tx, ErrorCode::RateLimited, "too many connection attempts, wait a minute");
        return;
    }
    let session_id = pjdesk_proto::random_u64();
    let host_token = pjdesk_proto::random_bytes::<16>();
    let controller_token = pjdesk_proto::random_bytes::<16>();
    let _ = host.tx.send(ServerMsg::IncomingSession {
        session_id,
        token: host_token,
        udp_port: st.udp_port,
        controller_name: controller_name.to_string(),
        controller_ip: ctrl.addr.ip().to_string(),
    });
    let session = Session {
        host_token,
        controller_token,
        host_id: target.to_string(),
        host_name: host.hostname.clone(),
        host_conn: host.conn_id,
        host_tx: host.tx.clone(),
        controller_conn: ctrl.conn_id,
        controller_ip: ctrl.addr.ip(),
        controller_tx: ctrl.tx.clone(),
        accepted: false,
        host: PeerSlot::default(),
        controller: PeerSlot::default(),
        info_sent: false,
        created: Instant::now(),
        relayed: false,
        move_told: false,
        move_acked: false,
        wait_logged: String::new(),
    };
    st.sessions.insert(session_id, session);
    crate::audit::AuditLog::open(&st.data_dir).record(
        "connect",
        controller_name,
        &format!("to {} from {}", format_id(target), ctrl.addr.ip()),
    );
    info!("Session {session_id:016x}: \"{controller_name}\" ({}) → host {}", ctrl.addr.ip(), format_id(target));
}

/// Puts a registered computer online and tells it the ID it owns.
/// `reassigned_from` is set when a second install key asked for an ID the
/// first install already holds. The first row is left as it is.
fn finish_registration(
    st: &mut State,
    conn: &mut Conn,
    id: String,
    hostname: &str,
    app_version: &str,
    admitted: &Admitted,
    reassigned_from: Option<&str>,
) {
    let access_key_id = match admitted {
        Admitted::Key(key) => key.id.clone(),
        Admitted::Open | Admitted::Signed(_) => String::new(),
    };
    let host = OnlineHost {
        conn_id: conn.id,
        tx: conn.tx.clone(),
        hostname: hostname.to_string(),
        app_version: app_version.to_string(),
        addr: conn.addr,
        proto: conn.proto,
        since: Instant::now(),
        close: Arc::clone(&conn.close),
        access_key_id: access_key_id.clone(),
    };
    if let Some(prev) = st.online.insert(id.clone(), host) {
        if prev.conn_id != conn.id {
            let held = prev.since.elapsed().as_secs();
            if prev.addr.ip() != conn.addr.ip() {
                warn!(
                    "Host {} is now registered from {} (it was {}, held {held}s). The device secret is known to both.",
                    format_id(&id),
                    conn.addr.ip(),
                    prev.addr.ip()
                );
            } else {
                info!(
                    "Host {} registered again from {} (another PJDesk process on this computer, previous connection held {held}s)",
                    format_id(&id),
                    conn.addr.ip()
                );
            }
            if !prev.tx.is_closed() {
                info!("Host {} kept the previous connection on standby", format_id(&id));
                st.standby.entry(id.clone()).or_default().push(prev);
            }
        }
    }
    conn.host_id = Some(id.clone());
    st.registry.note_address(&id, &conn.addr.ip().to_string());
    // A failed token must not clear a sign-in this computer already had.
    // An update reconnects before the new token is ready, and that must not
    // take the computer offline.
    let signed_in = matches!(admitted, Admitted::Signed(_) | Admitted::Key(_)) || st.registry.is_signed_in(&id);
    st.registry.note_access_key(&id, &access_key_id);
    st.registry.mark_signed_in(&id, signed_in);
    if let Some(kept) = reassigned_from {
        warn!(
            "ID {} is already held by another install. That computer keeps it. \"{hostname}\" is now {}.",
            format_id(kept),
            format_id(&id)
        );
    }
    conn.send(ServerMsg::Registered { device_id: id.clone() });
    if conn.proto >= 3 {
        conn.send(ServerMsg::Policy(st.accounts.policy()));
    }
    let os = st.registry.os_family(&id);
    if crate::registry::supports_host_report_for(app_version, &os) {
        conn.send(ServerMsg::WantHostReport);
    }
    crate::audit::AuditLog::open(&st.data_dir).record(
        "register",
        hostname,
        &format!("{} from {}", format_id(&id), conn.addr.ip()),
    );
    info!("Host {} \"{hostname}\" online from {} (v{app_version}); this server is the primary", format_id(&id), conn.addr.ip());
    if pjdesk_proto::update::version_at_least(app_version, "0.6.45") {
        conn.send(ServerMsg::Routes(crate::cloud::advertised_routes(&st.data_dir)));
    }
    if crate::account_mirror::client_can_account(app_version) {
        if let Some(fleet) = crate::account_mirror::fleet_for(&st.data_dir, "") {
            conn.send(fleet);
        }
        if let Some(release) = crate::account_mirror::release_message(&st.data_dir) {
            conn.send(release);
        }
    }
    // 0.6.50 learns the lock here. An older client is not sent this message.
    if pjdesk_proto::update::version_at_least(app_version, "0.6.50") {
        conn.send(ServerMsg::IncomingOnly { on: st.registry.incoming_only(&id) });
    }
    // Same computer name, offline leftovers only. A different install key is a
    // different hostname here, and a different name is not removed.
    let online: HashSet<String> = st.online.keys().cloned().collect();
    let removed = st.registry.remove_stale_duplicates(&id, &online);
    if removed > 0 {
        info!("Removed {removed} old record(s) for \"{hostname}\"; it is {} now", format_id(&id));
    }
}

fn handle_message(state: &Shared, conn: &mut Conn, msg: ClientMsg) {
    // Password checks are slow (Argon2). Do them off the process-wide lock so
    // one sign-in attempt cannot stall every other connection.
    if let ClientMsg::Login { proto, username, credential, device_name } = msg {
        handle_login(state, conn, proto, username, credential, device_name);
        return;
    }
    let mut guard = state.lock().unwrap();
    let st = &mut *guard;
    match msg {
        // Handled before this match so the password hash does not hold `state`'s lock.
        ClientMsg::Login { .. } => unreachable!("login is handled before the server lock"),

        ClientMsg::Ping => {
            if let Some(id) = conn.host_id.clone() {
                if let Some(version) = st.online.get(&id).map(|host| host.app_version.clone()) {
                    if st.registry.note_app_version(&id, &version) {
                        info!("Host {} version is now {version}", format_id(&id));
                    }
                }
            }
            conn.send(ServerMsg::Pong);
        }

        ClientMsg::PresentAccess { token } => {
            conn.access_token = token.chars().take(4096).collect();
        }

        ClientMsg::ManageAccessKey { token, op, label, expires_secs, computer_limit, key_id } => {
            manage_access_keys(st, conn, &token, &op, &label, expires_secs, computer_limit, &key_id);
        }

        ClientMsg::Register { proto, device_id, device_secret, hostname, app_version } => {
            offer_server_version(conn, &app_version);
            if !crate::cloud::accepts_host_registrations(&st.data_dir) {
                info!(
                    "Redirecting host registration from {}: this server is the standby and is not the live table",
                    conn.addr
                );
                conn.error(ErrorCode::Unauthorized, crate::cloud::redirect_text(&st.data_dir));
                return;
            }
            if registration_is_blocked(st, conn, &[&device_secret], device_id.as_deref(), &app_version) {
                return;
            }
            let admitted = admit_registration(st, conn);
            if let Admitted::Key(key) = &admitted {
                if !st.registry.key_has_room(&key.id, &device_secret, key.computer_limit) {
                    warn!("{}: access key {} is at its computer limit", conn.addr, key.label);
                    conn.error(ErrorCode::Unauthorized, crate::access_keys::LIMIT_MESSAGE);
                    return;
                }
            }
            if !note_proto(conn, proto) {
                return;
            }
            if !st.limiter.allow(conn.addr.ip(), "register", 20) {
                conn.error(ErrorCode::RateLimited, "too many registrations, slow down");
                return;
            }
            let hostname: String = hostname.chars().take(64).collect();
            let app_version: String = app_version.chars().take(32).collect();
            match st.registry.register(device_id.as_deref(), &device_secret, &hostname, &app_version) {
                Ok(id) => finish_registration(st, conn, id, &hostname, &app_version, &admitted, None),
                Err(e) => {
                    warn!("{}: registration refused: {e}", conn.addr);
                    if matches!(e, crate::registry::RegisterError::Blocked(_)) {
                        conn.error(ErrorCode::Unauthorized, crate::registry::Registry::BLOCKED_MESSAGE);
                    } else {
                        conn.error(ErrorCode::IdConflict, e.to_string());
                    }
                }
            }
        }

        ClientMsg::RegisterInstall { proto, device_id, device_secret, install_key, hostname, app_version } => {
            offer_server_version(conn, &app_version);
            if !crate::cloud::accepts_host_registrations(&st.data_dir) {
                info!(
                    "Redirecting host registration from {}: this server is the standby and is not the live table",
                    conn.addr
                );
                conn.error(ErrorCode::Unauthorized, crate::cloud::redirect_text(&st.data_dir));
                return;
            }
            if registration_is_blocked(
                st,
                conn,
                &[&device_secret, &install_key],
                device_id.as_deref(),
                &app_version,
            ) {
                return;
            }
            let admitted = admit_registration(st, conn);
            if install_key == [0u8; 32] {
                conn.error(ErrorCode::BadRequest, "this install has no device key");
                return;
            }
            let limit_secret = st.registry.install_limit_secret(device_id.as_deref(), &device_secret, &install_key);
            if let Admitted::Key(key) = &admitted {
                if !st.registry.key_has_room(&key.id, &limit_secret, key.computer_limit) {
                    warn!("{}: access key {} is at its computer limit", conn.addr, key.label);
                    conn.error(ErrorCode::Unauthorized, crate::access_keys::LIMIT_MESSAGE);
                    return;
                }
            }
            if !note_proto(conn, proto) {
                return;
            }
            if !st.limiter.allow(conn.addr.ip(), "register", 20) {
                conn.error(ErrorCode::RateLimited, "too many registrations, slow down");
                return;
            }
            let hostname: String = hostname.chars().take(64).collect();
            let app_version: String = app_version.chars().take(32).collect();
            match st.registry.register_install(device_id.as_deref(), &device_secret, &install_key, &hostname, &app_version)
            {
                Ok((id, replaced)) => {
                    finish_registration(st, conn, id, &hostname, &app_version, &admitted, replaced.as_deref())
                }
                Err(e) => {
                    warn!("{}: registration refused: {e}", conn.addr);
                    if matches!(e, crate::registry::RegisterError::Blocked(_)) {
                        conn.error(ErrorCode::Unauthorized, crate::registry::Registry::BLOCKED_MESSAGE);
                    } else {
                        conn.error(ErrorCode::IdConflict, e.to_string());
                    }
                }
            }
        }

        ClientMsg::Connect { proto, target_id, controller_name } => {
            if let Some(id) = conn.host_id.clone() {
                if st.registry.incoming_only(&id) {
                    conn.error(ErrorCode::Unauthorized, pjdesk_proto::signal::INCOMING_ONLY_REFUSAL);
                    return;
                }
            }
            if !crate::cloud::accepts_host_registrations(&st.data_dir) {
                info!("Redirecting a connection from {}: this server is the standby and is not the live table", conn.addr);
                conn.error(ErrorCode::Unauthorized, crate::cloud::redirect_text(&st.data_dir));
                return;
            }
            // Before any ID lookup, so a refused peer learns nothing about the target.
            if gate(st, conn).is_err() {
                conn.error(ErrorCode::Unauthorized, crate::access::peer_message());
                return;
            }
            if !note_proto(conn, proto) {
                return;
            }
            // Throttles ID scanning and password guessing from one address.
            if !st.limiter.allow(conn.addr.ip(), "connect", 30) {
                conn.error(ErrorCode::RateLimited, "too many connection attempts, wait a minute");
                return;
            }
            let target = normalize_id(&target_id);
            let controller_name: String = controller_name.chars().take(64).collect();
            let side = ControllerSide { conn_id: conn.id, addr: conn.addr, tx: conn.tx.clone() };
            if st.online.contains_key(&target) {
                start_session(st, &side, &target, &controller_name);
                return;
            }
            let seen = st.registry.last_seen_of(&target);
            let now = crate::registry::unix_now();
            if !host_was_seen_recently(seen, now) {
                let when = if seen == 0 { "never".to_string() } else { format!("{}s ago", now.saturating_sub(seen)) };
                info!("Connect to {} refused; it is not online (last seen {when})", format_id(&target));
                conn.error(ErrorCode::HostOffline, format!("device {} is not online", format_id(&target)));
                return;
            }
            let age = now.saturating_sub(seen);
            info!(
                "Connect to {} is waiting up to {}s; it was seen {age}s ago and is not registered right now",
                format_id(&target),
                CONNECT_WAIT.as_secs()
            );
            let state = Arc::clone(state);
            let target_for_wait = target.clone();
            let name_for_wait = controller_name.clone();
            tokio::spawn(async move {
                let deadline = Instant::now() + CONNECT_WAIT;
                loop {
                    {
                        let mut guard = state.lock().unwrap();
                        if guard.online.contains_key(&target_for_wait) {
                            info!(
                                "Connect to {} is going ahead; it registered during the wait",
                                format_id(&target_for_wait)
                            );
                            start_session(&mut guard, &side, &target_for_wait, &name_for_wait);
                            return;
                        }
                    }
                    if Instant::now() >= deadline {
                        info!(
                            "Connect to {} gave up after {}s; it did not register",
                            format_id(&target_for_wait),
                            CONNECT_WAIT.as_secs()
                        );
                        let _ = side.tx.send(ServerMsg::Error {
                            code: ErrorCode::HostOffline,
                            message: format!("device {} is not online", format_id(&target_for_wait)),
                        });
                        return;
                    }
                    tokio::time::sleep(CONNECT_POLL).await;
                }
            });
        }

        ClientMsg::SessionResponse { session_id, accept, reason } => {
            let relay_udp_port = st.udp_port;
            let Some(session) = st.sessions.get_mut(&session_id) else { return };
            if session.host_conn != conn.id {
                return;
            }
            let audit_detail = if accept {
                "accepted".to_string()
            } else {
                format!("refused: {}", reason.chars().take(80).collect::<String>())
            };
            crate::audit::AuditLog::open(&st.data_dir).record("session", &session.host_id, &audit_detail);
            if !accept && pjdesk_proto::signal::is_host_udp_failure(&reason) {
                warn!(
                    "Session {session_id:016x}: host {} could not bind UDP to the relay: {}",
                    format_id(&session.host_id),
                    reason.chars().take(200).collect::<String>()
                );
            }
            if accept {
                session.accepted = true;
                session.wait_logged = "host UDP bind, host UdpReady, controller UDP bind, controller UdpReady".into();
                info!(
                    "Session {session_id:016x}: host {} accepted; waiting for host UDP bind and controller UDP bind on UDP {} ({}s timeout)",
                    format_id(&session.host_id),
                    relay_udp_port,
                    pjdesk_proto::signal::RELAY_SETUP_TIMEOUT_SECS
                );
                spawn_relay_setup_timeout(state, session_id);
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
            // The UDP bind that follows is authorized by the session token issued
            // here, so refusing this message refuses punch and relay as well.
            let Some(role) = session_role(st, session_id, conn) else { return };
            if !setup_allowed(st, conn, role) {
                conn.error(ErrorCode::Unauthorized, crate::access::peer_message());
                return;
            }
            let Some(session) = st.sessions.get_mut(&session_id) else { return };
            let slot = session.slot_mut(role);
            slot.local = Some(local_addrs.into_iter().take(8).collect());
            if role == Role::Host {
                slot.fingerprint = cert_fingerprint;
            }
            session.maybe_send_peer_info(session_id);
        }

        ClientMsg::RelayOverTcp { session_id } => {
            let Some(role) = session_role(st, session_id, conn) else { return };
            if !setup_allowed(st, conn, role) {
                conn.error(ErrorCode::Unauthorized, crate::access::peer_message());
                return;
            }
            let Some(session) = st.sessions.get_mut(&session_id) else { return };
            if !session.accepted {
                return;
            }
            let slot = session.slot_mut(role);
            slot.tcp = true;
            slot.public = Some(TCP_PLACEHOLDER);
            if slot.local.is_none() {
                slot.local = Some(Vec::new());
            }
            session.relayed = true;
            info!(
                "Session {session_id:016x}: host {} — the {} could not use UDP; relaying it over its TCP connection",
                format_id(&session.host_id),
                if role == Role::Host { "host" } else { "controller" }
            );
            conn.send(ServerMsg::TcpRelayReady { session_id });
            session.maybe_send_peer_info(session_id);
            link_tcp_route(st, session_id);
        }

        // Handled before the burst limit in the connection loop.
        ClientMsg::RelayData { .. } => {}

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
                } else {
                    crate::audit::AuditLog::open(&st.data_dir).record(
                        "address_book",
                        &user,
                        &format!("{} computers", entries.len()),
                    );
                }
            }
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::ListDevices { token } => match authorized(st, &token, true) {
            Some(_) => {
                let online = st.online.keys().cloned().collect::<HashSet<_>>();
                let devices = st
                    .registry
                    .list()
                    .into_iter()
                    .map(|d| DeviceBrief {
                        online: online.contains(&d.id),
                        id: d.id,
                        hostname: d.hostname,
                        group: d.group,
                        version: d.version,
                        last_seen: d.last_seen,
                    })
                    .collect();
                conn.send(ServerMsg::DeviceList { devices });
            }
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::SetDeviceGroup { token, device_id, group } => match authorized(st, &token, true) {
            Some(user) => {
                let id = normalize_id(&device_id);
                match st.registry.set_group(&id, &group) {
                    Ok(()) => {
                        crate::audit::AuditLog::open(&st.data_dir).record(
                            "group",
                            &user,
                            &format!("{} -> {}", format_id(&id), group.chars().take(40).collect::<String>()),
                        );
                        conn.send(ServerMsg::Pong);
                    }
                    Err(e) => conn.error(ErrorCode::BadRequest, e.to_string()),
                }
            }
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::GetPolicy { token } => match authorized(st, &token, false) {
            Some(_) => conn.send(ServerMsg::Policy(st.accounts.policy())),
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::SetPolicy { token, policy } => match authorized(st, &token, false) {
            Some(user) if st.accounts.role_of(&user) == Some(AccountRole::Admin) => {
                if let Err(e) = st.accounts.set_policy(policy) {
                    conn.error(ErrorCode::Internal, e.to_string());
                } else {
                    crate::audit::AuditLog::open(&st.data_dir).record("policy", &user, "updated");
                    push_policy(st);
                    conn.send(ServerMsg::Policy(st.accounts.policy()));
                }
            }
            Some(_) => conn.error(ErrorCode::Unauthorized, "only an administrator can change policy"),
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::GetAudit { token, limit } => match authorized(st, &token, true) {
            Some(_) => {
                let events = crate::audit::AuditLog::open(&st.data_dir).recent(limit as usize);
                conn.send(ServerMsg::Audit { events });
            }
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::SetRole { token, username, role } => match authorized(st, &token, false) {
            Some(actor) if st.accounts.role_of(&actor) == Some(AccountRole::Admin) => {
                match st.accounts.set_role(&username, role) {
                    Ok(()) => {
                        crate::audit::AuditLog::open(&st.data_dir).record(
                            "role",
                            &actor,
                            &format!("{username} is now {}", role.as_str()),
                        );
                        conn.send(ServerMsg::Pong);
                    }
                    Err(e) => conn.error(ErrorCode::BadRequest, e.to_string()),
                }
            }
            Some(_) => conn.error(ErrorCode::Unauthorized, "only an administrator can change roles"),
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::ReportDisplayName { name } => {
            let Some(id) = conn.host_id.clone() else { return };
            let name = name.chars().filter(|c| !c.is_control()).take(64).collect::<String>();
            st.registry.note_display_name(&id, &name);
        }

        ClientMsg::ReportHost { hostname, os_family, os_version, console_user } => {
            let Some(id) = conn.host_id.clone() else { return };
            let hostname = hostname.chars().filter(|c| !c.is_control()).take(64).collect::<String>();
            st.registry.note_report(&id, &hostname, &os_family, &os_version, &console_user);
            if !hostname.is_empty() {
                if let Some(host) = st.online.get_mut(&id) {
                    host.hostname = hostname.clone();
                }
            }
            info!(
                "Host {} reported {}",
                format_id(&id),
                if hostname.is_empty() { "its system".into() } else { hostname }
            );
        }

        ClientMsg::ChangePassword { token, new_password } => match st.accounts.user_for_token(&token) {
            Some(username) => match st.accounts.set_password(&username, &new_password) {
                Ok(()) => {
                    crate::audit::AuditLog::open(&st.data_dir).record("password", &username, "changed their password");
                    conn.send(ServerMsg::LoginResult {
                        ok: true,
                        token: String::new(),
                        display_name: username,
                        is_admin: false,
                        message: "Password updated. Sign in again.".into(),
                    });
                }
                Err(e) => conn.error(ErrorCode::BadRequest, e.to_string()),
            },
            None => conn.error(ErrorCode::Unauthorized, "please sign in again"),
        },

        ClientMsg::RelayMoved { session_id } => {
            if let Some(session) = st.sessions.get_mut(&session_id) {
                session.move_acked = true;
                info!("failover: session {session_id:016x} is moving to the new primary");
            }
        }

        // The connection task handles this before taking the server lock.
        ClientMsg::TakePrimary { .. } => {}

        ClientMsg::PublishSaved { computers, app_lock, app_lock_set } => {
            let _ = (computers, app_lock, app_lock_set);
            // Saved-list edits from 0.6.48 arrive as Account. This arm exists
            // so an older draft of that message does not stall the connection.
        }

        ClientMsg::Account { .. } => {
            // The connection task answers this before the lock. Do not turn it
            // into "Sign in required".
        }

        ClientMsg::SetIncomingOnly { on, password } => {
            let Some(id) = conn.host_id.clone() else {
                conn.error(ErrorCode::Unauthorized, "This computer is not registered.");
                return;
            };
            let currently = st.registry.incoming_only(&id);
            if !on && currently && !incoming_password_ok(&st.data_dir, &password) {
                conn.error(
                    ErrorCode::Unauthorized,
                    "Enter the account owner's password to allow this computer to connect to others.",
                );
                return;
            }
            if let Err(e) = st.registry.set_incoming_only(&id, on) {
                conn.error(ErrorCode::BadRequest, e.to_string());
                return;
            }
            conn.send(ServerMsg::IncomingOnly { on });
            crate::audit::AuditLog::open(&st.data_dir).record("incoming_only", &id, if on { "on" } else { "off" });
        }
    }
}

/// The account password matches the copy on this server. An empty password does not.
/// The password is not written to the log.
fn incoming_password_ok(data_dir: &std::path::Path, password: &str) -> bool {
    let email = crate::cloud::saved_account_email(data_dir);
    crate::account_mirror::confirm_owner(data_dir, &email, password).is_ok()
}

/// Records the client's protocol version. False when it is too old or too new
/// to speak to this server — the error text contains "version mismatch" so a
/// newer client can retry with an older version.
fn note_proto(conn: &mut Conn, proto: u16) -> bool {
    if !protocol_ok(proto) {
        conn.error(
            ErrorCode::VersionMismatch,
            format!(
                "version mismatch (server speaks protocol {MIN_PROTOCOL_VERSION}-{PROTOCOL_VERSION}, client {proto})"
            ),
        );
        return false;
    }
    if proto > conn.proto {
        conn.proto = proto;
    }
    true
}

/// The signed-in username, if `token` is live. `manage` requires operator or admin.
fn authorized(st: &State, token: &str, manage: bool) -> Option<String> {
    let user = st.accounts.user_for_token(token)?;
    let role = st.accounts.role_of(&user).unwrap_or(AccountRole::User);
    if manage && !role.can_manage_devices() {
        return None;
    }
    Some(user)
}

fn push_policy(st: &State) {
    let msg = ServerMsg::Policy(st.accounts.policy());
    for host in st.online.values() {
        if host.proto >= 3 {
            let _ = host.tx.send(msg.clone());
        }
    }
}

/// Ends a session whose peers did not both bind UDP in time, and tells the
/// controller which side was missing so it does not wait out its own timeout.
fn spawn_relay_setup_timeout(state: &Shared, session_id: SessionId) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(pjdesk_proto::signal::RELAY_SETUP_TIMEOUT_SECS)).await;
        let mut st = state.lock().unwrap();
        let udp_port = st.udp_port;
        let Some(session) = st.sessions.get(&session_id) else { return };
        if session.info_sent {
            return;
        }
        let missing = session.relay_missing();
        let notice = pjdesk_proto::signal::relay_timeout_notice(&missing, udp_port);
        warn!("Session {session_id:016x}: host {}: {notice}", format_id(&session.host_id));
        let _ = session.controller_tx.send(ServerMsg::Error { code: ErrorCode::HostBusy, message: notice });
        st.sessions.remove(&session_id);
    });
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

/// Removes every computer that used this key and disconnects the ones that are online.
/// Returns true when a computer was removed or disconnected.
fn drop_access_key(st: &mut State, key_id: &str) -> bool {
    if !crate::access_keys::is_key_id(key_id) {
        return false;
    }
    let removed = st.registry.take_access_key(key_id);
    let mut live: Vec<String> = st
        .online
        .iter()
        .filter(|(_, host)| host.access_key_id == key_id)
        .map(|(id, _)| id.clone())
        .collect();
    for (id, hosts) in &st.standby {
        if hosts.iter().any(|host| host.access_key_id == key_id) && !live.contains(id) {
            live.push(id.clone());
        }
    }
    let any = !removed.is_empty() || !live.is_empty();
    for id in removed.iter().chain(live.iter()) {
        disconnect_device(st, id, "This access key is no longer valid.");
    }
    any
}

/// How long a role change waits for relayed sessions to follow.
const MOVE_WAIT: Duration = Duration::from_secs(8);

/// Tells every relayed session that has not been told yet. An older host is
/// marked done so this server does not wait for a message it cannot read.
fn tell_open_relays(st: &mut State, primary: &str) {
    let pending_ids: Vec<(SessionId, String)> = st
        .sessions
        .iter()
        .filter(|(_, session)| session.relayed && !session.move_told)
        .map(|(id, session)| (*id, session.host_id.clone()))
        .collect();
    let host_versions: Vec<(SessionId, String)> = pending_ids
        .into_iter()
        .map(|(id, host_id)| {
            let version = st.online.get(&host_id).map(|host| host.app_version.clone()).unwrap_or_default();
            (id, version)
        })
        .collect();
    for (id, version) in host_versions {
        let Some(session) = st.sessions.get_mut(&id) else { continue };
        session.move_told = true;
        if !pjdesk_proto::update::version_at_least(&version, "0.6.47") {
            session.move_acked = true;
            info!("failover: session {id:016x} is on an older client; it reconnects when the relay stops");
            continue;
        }
        let msg = ServerMsg::MoveRelay { session_id: id, primary: primary.to_string() };
        let _ = session.host_tx.send(msg);
        let where_next = if primary.is_empty() { "the current primary" } else { primary };
        info!("failover: told session {id:016x} to reconnect through {where_next}");
    }
}

/// Before the process exits, ask relayed sessions to open on the other server.
/// Direct sessions are not on this path. The wait is short so a stop still finishes.
async fn release_relays_on_stop(state: &Shared) {
    let carrying = state.lock().unwrap().sessions.values().any(|session| session.relayed);
    if !carrying {
        return;
    }
    let already = state.lock().unwrap().pending_move.is_some();
    if !already {
        let data_dir = state.lock().unwrap().data_dir.clone();
        let primary = crate::cloud::cached_peer_address(&data_dir);
        let where_next = if primary.is_empty() { "the current primary".to_string() } else { primary.clone() };
        info!("failover: shutting down; telling relayed sessions to move to {where_next}");
        tell_open_relays(&mut state.lock().unwrap(), &primary);
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        let waiting = state.lock().unwrap().sessions.values().any(|session| session.relayed && !session.move_acked);
        if !waiting {
            info!("failover: relayed sessions left before this server stopped");
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    warn!("failover: stopping with a relayed session still here; that session reconnects through the next server");
}

/// Tells relayed sessions to open again on `primary`. Returns true when the
/// role change may be written: nothing is waiting, or every told session has
/// followed, or the wait ran out (the crash path drops the relay).
fn step_down_ready(state: &Shared, primary: &str, kind: StepKind) -> bool {
    let mut st = state.lock().unwrap();
    if st.pending_move.is_none() {
        let target = if primary.is_empty() { "the current primary".to_string() } else { primary.to_string() };
        info!("failover: role change started; relayed sessions move to {target} before this server steps down");
        st.pending_move = Some(PendingMove { primary: primary.to_string(), since: Instant::now(), kind });
    }
    let (primary, since) = {
        let pending = st.pending_move.as_ref().unwrap();
        (pending.primary.clone(), pending.since)
    };
    tell_open_relays(&mut st, &primary);
    let waiting = st.sessions.values().any(|session| session.relayed && !session.move_acked);
    let timed_out = since.elapsed() > MOVE_WAIT;
    if waiting && !timed_out {
        return false;
    }
    if waiting && timed_out {
        warn!("failover: relayed sessions did not confirm the move; finishing step-down. A lost relay reconnects through the primary.");
    }
    true
}

/// Writes the standby role, then drops every relayed session.
async fn commit_step_down(state: &Shared, acceptor: &RwLock<TlsAcceptor>) {
    let pending = {
        let mut st = state.lock().unwrap();
        st.pending_move.take()
    };
    let Some(pending) = pending else { return };
    let data_dir = state.lock().unwrap().data_dir.clone();
    let StepKind::Yield { epoch, instance, name } = pending.kind;
    let dir = data_dir.clone();
    let label = name.clone();
    let _ = tokio::task::spawn_blocking(move || crate::cloud::yield_to_epoch(&dir, epoch, &instance, &name)).await;
    warn!("failover: generation {epoch} belongs to {label}; this server is now the standby");
    drop_relayed_sessions(state);
    release_hosts_for_failover(state);
    let data_dir = state.lock().unwrap().data_dir.clone();
    reload_from_disk(state, acceptor, &data_dir);
}

/// A standby carries no relay routes. Removing the session makes the relay
/// task drop the forwarding entries.
fn drop_relayed_sessions(state: &Shared) {
    let mut st = state.lock().unwrap();
    let ids: Vec<SessionId> = st.sessions.iter().filter(|(_, session)| session.relayed).map(|(id, _)| *id).collect();
    let n = ids.len();
    for id in ids {
        st.sessions.remove(&id);
    }
    if n > 0 {
        info!("failover: dropped {n} relayed session(s); this standby carries none");
    }
}

/// Closes live registrations so those computers connect to the new primary.
/// A direct or hole-punched session does not use this socket for its picture.
fn release_hosts_for_failover(state: &Shared) {
    let ids: Vec<String> = {
        let st = state.lock().unwrap();
        st.online.keys().cloned().chain(st.standby.keys().cloned()).collect()
    };
    let mut st = state.lock().unwrap();
    let n = ids.len();
    for id in ids {
        disconnect_device(&mut st, &id, pjdesk_proto::signal::STANDBY_REGISTRATION);
    }
    if n > 0 {
        info!("failover: released {n} computers so they register with the new primary");
    }
}

/// Drops a computer's live connections and any session it is hosting.
fn disconnect_device(st: &mut State, id: &str, message: &str) {
    let mut notify = Vec::new();
    if let Some(host) = st.online.remove(id) {
        let _ = host.tx.send(ServerMsg::Error { code: ErrorCode::Unauthorized, message: message.to_string() });
        notify.push(host.close);
    }
    if let Some(list) = st.standby.remove(id) {
        for host in list {
            let _ = host.tx.send(ServerMsg::Error { code: ErrorCode::Unauthorized, message: message.into() });
            notify.push(host.close);
        }
    }
    let ended: Vec<SessionId> = st.sessions.iter().filter(|(_, session)| session.host_id == id).map(|(sid, _)| *sid).collect();
    for sid in ended {
        if let Some(session) = st.sessions.remove(&sid) {
            let _ = session.host_tx.send(ServerMsg::Error {
                code: ErrorCode::Unauthorized,
                message: message.into(),
            });
            let _ = session.controller_tx.send(ServerMsg::Error {
                code: ErrorCode::HostOffline,
                message: "That computer was removed from the server.".into(),
            });
        }
    }
    for close in notify {
        close.notify_one();
    }
}

/// One online device, published to `online.json` for the management window.
/// `at` is unix seconds. Readers use it when the file's modification time stalls.
#[derive(serde::Serialize)]
struct OnlineInfo {
    id: String,
    hostname: String,
    version: String,
    at: u64,
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
                let ips: Vec<(String, String)> =
                    st.online.iter().map(|(id, host)| (id.clone(), host.addr.ip().to_string())).collect();
                st.registry.touch(&ids);
                for (id, ip) in ips {
                    st.registry.note_address(&id, &ip);
                }
            }
            let at = crate::registry::unix_now();
            let snapshot: Vec<OnlineInfo> = st
                .online
                .iter()
                .map(|(id, h)| OnlineInfo {
                    id: id.clone(),
                    hostname: h.hostname.clone(),
                    version: h.app_version.clone(),
                    at,
                })
                .collect();
            (st.data_dir.clone(), snapshot)
        };
        let online_path = data_dir.join("online.json");
        if let Ok(bytes) = serde_json::to_vec(&snapshot) {
            let tmp = online_path.with_extension("json.tmp");
            let created = !online_path.exists();
            let replaced = std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &online_path).is_ok();
            if replaced {
                if created {
                    crate::cloud::protect(&online_path);
                }
            } else {
                warn!("could not replace online.json; writing it in place");
                match std::fs::write(&online_path, &bytes) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&tmp);
                        if created {
                            crate::cloud::protect(&online_path);
                        }
                    }
                    Err(e) => warn!("could not write online.json: {e}"),
                }
            }
        }

        let updates = take(&data_dir, UPDATE_FILE);
        let forgets = take(&data_dir, FORGET_FILE);
        let blocks = take(&data_dir, crate::requests::BLOCK_FILE);
        let unblocks = take(&data_dir, crate::requests::UNBLOCK_FILE);
        let groups = take(&data_dir, crate::requests::GROUP_FILE);
        let incoming = take(&data_dir, crate::requests::INCOMING_FILE);
        let revokes = take(&data_dir, crate::requests::REVOKE_KEY_FILE);
        let require_now = take(&data_dir, crate::requests::REQUIRE_DISCONNECT_FILE);
        let dead = crate::access_keys::Store::dead_ids(&data_dir, crate::registry::unix_now());
        if updates.is_empty()
            && forgets.is_empty()
            && groups.is_empty()
            && blocks.is_empty()
            && unblocks.is_empty()
            && revokes.is_empty()
            && require_now.is_empty()
            && dead.is_empty()
            && incoming.is_empty()
        {
            continue;
        }
        let mut st = state.lock().unwrap();
        for req in &updates {
            if req == "*" {
                info!(
                    "Update all online computers: asking {} client(s) to install the latest release",
                    st.online.len()
                );
                for (id, h) in &st.online {
                    info!("Asking {} \"{}\" (v{}) to update now", format_id(id), h.hostname, h.app_version);
                    let _ = h.tx.send(ServerMsg::UpdateNow);
                }
            } else if let Some(h) = st.online.get(req) {
                info!("Asking {} \"{}\" (v{}) to update now", format_id(req), h.hostname, h.app_version);
                let _ = h.tx.send(ServerMsg::UpdateNow);
            } else {
                info!("Update request for {} was not sent; that computer is not online", format_id(req));
            }
        }
        for id in &forgets {
            if let Err(e) = st.registry.remove(id) {
                warn!("could not forget {}: {e}", format_id(id));
            } else {
                info!("Forgot device {}", format_id(id));
                crate::audit::AuditLog::open(&st.data_dir).record("forget", "server", &format_id(id));
            }
        }
        for id in &blocks {
            match st.registry.block(id) {
                Ok(true) => {
                    info!("Blocked {} and dropped its connection", format_id(id));
                    disconnect_device(&mut st, id, "This computer was removed from PJDesk Server.");
                    crate::audit::AuditLog::open(&st.data_dir).record("block", "server", &format_id(id));
                }
                Ok(false) => info!("Block request for {} did not match a computer", format_id(id)),
                Err(e) => warn!("could not block {}: {e}", format_id(id)),
            }
        }
        for id in &unblocks {
            match st.registry.unblock(id) {
                Ok(true) => {
                    info!("Unblocked {}", format_id(id));
                    crate::audit::AuditLog::open(&st.data_dir).record("unblock", "server", &format_id(id));
                }
                Ok(false) => info!("Unblock request for {} did not match a blocked computer", format_id(id)),
                Err(e) => warn!("could not unblock {}: {e}", format_id(id)),
            }
        }
        if !require_now.is_empty() && crate::access::Policy::load(&data_dir).require_accounts {
            let ids = st.registry.grace_ids();
            for id in &ids {
                info!("Require signed-in accounts dropped {}", format_id(id));
                disconnect_device(&mut st, id, "This server now requires a signed-in PJDesk account.");
            }
        }
        let mut dropped_keys = revokes;
        for id in dead {
            if !dropped_keys.iter().any(|have| have == &id) {
                dropped_keys.push(id);
            }
        }
        for key_id in &dropped_keys {
            if drop_access_key(&mut st, key_id) {
                info!("Removed the computers that used access key {key_id}");
                crate::audit::AuditLog::open(&st.data_dir).record("access_key", "server", "removed computers");
            }
        }
        for target in &incoming {
            if let Some((id, on)) = crate::requests::parse_incoming_target(target) {
                match st.registry.set_incoming_only(&id, on) {
                    Ok(()) => {
                        info!("Incoming only for {} is {}", format_id(&id), if on { "on" } else { "off" });
                        if let Some(host) = st.online.get(&id) {
                            if pjdesk_proto::update::version_at_least(&host.app_version, "0.6.50") {
                                let _ = host.tx.send(ServerMsg::IncomingOnly { on });
                            }
                        }
                        crate::audit::AuditLog::open(&st.data_dir).record(
                            "incoming_only",
                            "server",
                            &format!("{} {}", format_id(&id), if on { "on" } else { "off" }),
                        );
                    }
                    Err(e) => warn!("could not set incoming only on {}: {e}", format_id(&id)),
                }
            }
        }
        for target in &groups {
            if let Some((id, group)) = crate::requests::parse_group_target(target) {
                match st.registry.set_group(&id, &group) {
                    Ok(()) => {
                        info!("Device {} is now in group \"{group}\"", format_id(&id));
                        crate::audit::AuditLog::open(&st.data_dir).record(
                            "group",
                            "server",
                            &format!("{} -> {group}", format_id(&id)),
                        );
                    }
                    Err(e) => warn!("could not set a group on {}: {e}", format_id(&id)),
                }
            }
        }
    }
}

/// Probes the primary every few seconds while this server is the standby,
/// and steps down when another server publishes a higher generation.
async fn failover_watch(state: Shared, acceptor: Arc<RwLock<TlsAcceptor>>) {
    let data_dir = state.lock().unwrap().data_dir.clone();
    let mut tick = tokio::time::interval(Duration::from_secs(pjdesk_proto::failover::BEAT_SECS));
    let mut misses = 0u32;
    let mut last_log = String::new();
    let mut routes_gen = 0u64;
    loop {
        tick.tick().await;
        let dir = data_dir.clone();
        let seen = misses;
        let step = match tokio::task::spawn_blocking(move || crate::failover::watch_once(&dir, seen)).await {
            Ok(step) => step,
            Err(e) => {
                warn!("failover watch stopped: {e}");
                continue;
            }
        };
        match step {
            crate::failover::Step::Stay { misses: next, log } => {
                misses = next;
                if !log.is_empty() && log != last_log {
                    info!("{log}");
                    last_log = log;
                }
            }
            crate::failover::Step::Promote { from, misses: next } => {
                misses = 0;
                last_log.clear();
                warn!(
                    "failover: primary {from} missed {next} heartbeats and its cloud health is stale; taking over"
                );
                let dir = data_dir.clone();
                match tokio::task::spawn_blocking(move || crate::cloud::promote(&dir)).await {
                    Ok(Ok(())) => {
                        info!("failover: this server is now the primary");
                        reload_from_disk(&state, &acceptor, &data_dir);
                    }
                    Ok(Err(e)) => warn!("failover: could not take over: {e:#}"),
                    Err(e) => warn!("failover: could not take over: {e}"),
                }
            }
            crate::failover::Step::YieldPending { epoch, name, instance, primary } => {
                let primary = if primary.is_empty() {
                    let dir = data_dir.clone();
                    tokio::task::spawn_blocking(move || crate::cloud::migration_target(&dir))
                        .await
                        .unwrap_or_default()
                } else {
                    primary
                };
                if step_down_ready(
                    &state,
                    &primary,
                    StepKind::Yield { epoch, instance, name },
                ) {
                    misses = 0;
                    last_log.clear();
                    commit_step_down(&state, &acceptor).await;
                }
            }
        }
        if let Some(routes) = crate::cloud::take_routes_push(&data_dir, &mut routes_gen) {
            push_routes(&state, &routes);
        }
    }
}

fn push_routes(state: &Shared, routes: &pjdesk_proto::routes::ServerRoutes) {
    let guard = state.lock().unwrap();
    let msg = ServerMsg::Routes(routes.clone());
    for host in guard.online.values() {
        if host.proto >= 3 && pjdesk_proto::update::version_at_least(&host.app_version, "0.6.45") {
            let _ = host.tx.send(msg.clone());
        }
    }
}

/// Cloud sync (see [`crate::cloud`]): every 5 minutes, or when the window asks,
/// uploads this server's state if it is in use, or takes the newest copy from
/// the main server if it is on standby — then reloads the key, accounts and
/// computer list so it is ready to take over.
async fn cloud_sync(state: Shared, acceptor: Arc<RwLock<TlsAcceptor>>) {
    use crate::cloud::{promote, sync_once, Role};
    use crate::requests::{take, DEMOTE_FILE, PROMOTE_FILE, RELOAD_FILE, SYNC_FILE};
    let data_dir = state.lock().unwrap().data_dir.clone();
    let startup = data_dir.clone();
    match tokio::task::spawn_blocking(move || crate::account_mirror::recover_if_empty(&startup)).await {
        Ok(line) => info!("{line}"),
        Err(err) => warn!("account copy: {err}"),
    }
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut ticks: u32 = 0;
    let mut want_demote = false;
    loop {
        tick.tick().await;
        ticks = ticks.wrapping_add(1);
        if !take(&data_dir, RELOAD_FILE).is_empty() {
            reload_from_disk(&state, &acceptor, &data_dir);
        }
        let promote_now = !take(&data_dir, PROMOTE_FILE).is_empty();
        if !take(&data_dir, DEMOTE_FILE).is_empty() {
            want_demote = true;
        }
        if promote_now {
            want_demote = false;
            let dir = data_dir.clone();
            match tokio::task::spawn_blocking(move || promote(&dir)).await {
                Ok(Ok(())) => {
                    let notice = crate::cloud::CloudStatus::load(&data_dir)
                        .map(|status| status.notice)
                        .filter(|notice| !notice.trim().is_empty())
                        .unwrap_or_else(|| "This server is now the primary.".into());
                    info!("{notice}");
                    crate::role_handoff::write_result(&data_dir, true, &notice);
                }
                Ok(Err(e)) => {
                    warn!("could not become the cloud primary: {e:#}");
                    crate::role_handoff::write_result(&data_dir, false, &format!("{e:#}"));
                }
                Err(e) => {
                    warn!("could not become the cloud primary: {e}");
                    crate::role_handoff::write_result(&data_dir, false, "Could not become the primary.");
                }
            }
            reload_from_disk(&state, &acceptor, &data_dir);
        } else if want_demote {
            let accepted = crate::role_handoff::load_accepted(&data_dir);
            if let Some(took) = accepted {
                if step_down_ready(
                    &state,
                    &took.address,
                    StepKind::Yield { epoch: took.epoch, instance: took.instance, name: took.name.clone() },
                ) {
                    commit_step_down(&state, &acceptor).await;
                    let sentence = pjdesk_proto::failover::standby_result(&took.name, took.epoch);
                    info!("{sentence}");
                    crate::role_handoff::write_result(&data_dir, true, &sentence);
                    crate::role_handoff::clear_accepted(&data_dir);
                    want_demote = false;
                }
            } else {
                let epoch = crate::cloud::local_epoch(&data_dir);
                let message = format!(
                    "The other server has not taken the primary. This server stayed the primary (generation {epoch})."
                );
                warn!("{message}");
                crate::role_handoff::write_result(&data_dir, false, &message);
                want_demote = false;
            }
        }
        let forced = !take(&data_dir, SYNC_FILE).is_empty();
        // Every 5 minutes ±20%, backing off up to 30 minutes while pjdesk.com fails.
        if !promote_now && !forced && !crate::cloud::batch_due() {
            continue;
        }
        // The standby does not upload. It still downloads the account copy so a
        // phone can sign in against either server.
        if !crate::cloud::accepts_host_registrations(&data_dir) && !forced {
            let dir = data_dir.clone();
            match tokio::task::spawn_blocking(move || crate::account_mirror::pull_now(&dir)).await {
                Ok(line) => {
                    info!("standby account copy: {line}");
                    crate::cloud::batch_done(!crate::account_mirror::cloud_failed(&data_dir));
                }
                Err(err) => {
                    warn!("standby account copy: {err}");
                    crate::cloud::batch_done(false);
                }
            }
            continue;
        }
        // An empty table after a restart is not a demotion. Treating it as
        // standby wrote cloud-status.json as standby, and that file then
        // refused every registration, so every computer stayed offline.
        let accepts = crate::cloud::accepts_host_registrations(&data_dir);
        let role = if accepts { Role::Active } else { Role::Standby };
        let dir = data_dir.clone();
        let applied = tokio::task::spawn_blocking(move || {
            let applied = sync_once(&dir, role, forced);
            crate::account_mirror::batch(&dir, forced);
            if !crate::account_mirror::has_owner_copy(&dir) {
                let _ = crate::account_mirror::recover_if_empty(&dir);
            }
            crate::account_mirror::refresh_release(&dir);
            applied
        })
        .await
        .unwrap_or(false);
        if applied {
            reload_from_disk(&state, &acceptor, &data_dir);
        }
    }
}

/// Automatic server updates, when the administrator has turned them on.
async fn server_updates(data_dir: std::path::PathBuf) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        let dir = data_dir.clone();
        let _ = tokio::task::spawn_blocking(move || crate::self_update::service_tick(&dir)).await;
    }
}

/// Re-reads the server key, accounts and computer list from the data folder.
fn reload_from_disk(state: &Shared, acceptor: &RwLock<TlsAcceptor>, data_dir: &Path) {
    match load_identity(data_dir).and_then(|id| {
        let fp = tls::format_fingerprint(&id.fingerprint());
        Ok((TlsAcceptor::from(Arc::new(tls::server_config(&id, ALPN_SIGNAL)?)), fp))
    }) {
        Ok((new, fp)) => {
            *acceptor.write().unwrap() = new;
            info!("Server key is now {fp}");
        }
        Err(e) => warn!("could not reload the server key: {e:#}"),
    }
    let mut st = state.lock().unwrap();
    match Registry::load(data_dir.join("devices.json")) {
        Ok(mut reg) => {
            // A cloud reload must not put an older version back over a live registration.
            for (id, host) in &st.online {
                reg.note_app_version(id, &host.app_version);
            }
            st.registry = reg;
        }
        Err(e) => warn!("could not reload the computer list: {e:#}"),
    }
    match crate::accounts::Accounts::load(data_dir) {
        Ok(acc) => st.accounts = acc,
        Err(e) => warn!("could not reload accounts: {e:#}"),
    }
    info!("Reloaded {} computers and {} accounts from disk", st.registry.len(), st.accounts.list().len());
    push_policy(&st);
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
    fn presence_host_was_seen_recently() {
        assert!(!host_was_seen_recently(0, 1_000));
        assert!(host_was_seen_recently(1_000, 1_000));
        assert!(host_was_seen_recently(1_000, 1_120));
        assert!(!host_was_seen_recently(1_000, 1_121));
        assert!(host_was_seen_recently(2_000, 1_000), "a clock ahead of the server is still recent");
    }

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

    fn state_in(dir: &std::path::Path) -> Shared {
        let _ = std::fs::create_dir_all(dir);
        Arc::new(Mutex::new(State {
            registry: Registry::load(dir.join("devices.json")).unwrap(),
            accounts: crate::accounts::Accounts::load(dir).unwrap(),
            online: HashMap::new(),
            standby: HashMap::new(),
            sessions: HashMap::new(),
            pending_move: None,
            limiter: RateLimiter::default(),
            udp_port: 21200,
            data_dir: dir.to_path_buf(),
            udp_socket: None,
            tcp_routes: HashMap::new(),
        }))
    }

    fn conn_pair() -> (Conn, mpsc::UnboundedReceiver<ServerMsg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let conn = Conn {
            id: 1,
            addr: "192.0.2.10:40000".parse().unwrap(),
            tx,
            host_id: None,
            proto: 0,
            access_token: String::new(),
            close: Arc::new(Notify::new()),
        };
        (conn, rx)
    }

    #[test]
    fn connect_without_a_token_does_not_confirm_the_id_when_accounts_are_required() {
        let dir = std::env::temp_dir().join(format!("pjdesk-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::access::Policy { require_accounts: true, ..crate::access::Policy::default() }.save(&dir).unwrap();
        let state = state_in(&dir);
        let (mut conn, mut rx) = conn_pair();
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Connect { proto: 3, target_id: "123456789".into(), controller_name: "laptop".into() },
        );
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, message } => {
                assert_eq!(code, ErrorCode::Unauthorized);
                assert_eq!(message, "Sign in required");
                assert!(!message.contains("123"));
            }
            other => panic!("{other:?}"),
        }
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Register {
                proto: 3,
                device_id: Some("123456789".into()),
                device_secret: [1u8; 32],
                hostname: "Office".into(),
                app_version: "0.6.49".into(),
            },
        );
        let id = match rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => device_id,
            other => panic!("a computer must get an ID even when sign-in is required, got {other:?}"),
        };
        assert_eq!(id, "123456789");
        assert!(!state.lock().unwrap().registry.is_signed_in(&id), "the ID is pending until the account signs in");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_account_sign_in_is_not_turned_into_sign_in_required() {
        let dir = std::env::temp_dir().join(format!("pjdesk-account-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::access::Policy { require_accounts: true, require_chosen: true, ..crate::access::Policy::default() }
            .save(&dir)
            .unwrap();
        let state = state_in(&dir);
        let (mut conn, mut rx) = conn_pair();
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Account {
                op: "sign-in".into(),
                email: "ben@spader.us".into(),
                auth_key: "zz".into(),
                refresh: String::new(),
                device_id: "phone".into(),
                device_name: "Android".into(),
                vault: String::new(),
                app_lock: String::new(),
                computers: Vec::new(),
                proof: String::new(),
            },
        );
        assert!(rx.try_recv().is_err(), "sign-in is answered before the require gate, not refused here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refreshed_mirror_session_can_connect() {
        let dir = std::env::temp_dir().join(format!("pjdesk-mirror-connect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fp = "ab".repeat(32);
        let mut copy = pjdesk_proto::account_mirror::AccountCopy::default();
        copy.account_id = "acct".into();
        copy.email = "ben@spader.us".into();
        copy.auth_hash = "11".repeat(32);
        copy.sign_pkcs8 = "not-a-key".into();
        copy.network.id = "net".into();
        copy.network.active_fingerprint = fp.clone();
        copy.network.active_address = "192.168.0.193:21200".into();
        let refresh = "cd".repeat(32);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        copy.devices.push(pjdesk_proto::account_mirror::DeviceRow {
            device_id: "phone".into(),
            account_id: "acct".into(),
            email: "ben@spader.us".into(),
            name: "Android".into(),
            refresh_hash: pjdesk_proto::account_mirror::refresh_hash(&refresh),
            enrolled_at: now.saturating_sub(10),
            revoked_at: 0,
        });
        std::fs::write(dir.join("account-mirror.json"), serde_json::to_vec(&copy).unwrap()).unwrap();
        crate::access::Policy {
            require_accounts: true,
            require_chosen: true,
            public_key: "bb".repeat(32),
            fingerprint: fp.clone(),
            ..crate::access::Policy::default()
        }
        .save(&dir)
        .unwrap();
        let reply = crate::account_mirror::answer(
            &dir,
            ClientMsg::Account {
                op: "refresh".into(),
                email: String::new(),
                auth_key: String::new(),
                refresh,
                device_id: "phone".into(),
                device_name: String::new(),
                vault: String::new(),
                app_lock: String::new(),
                computers: Vec::new(),
                proof: String::new(),
            },
        );
        let access = match reply {
            ServerMsg::AccountResult { ok, access, message, .. } => {
                assert!(ok, "{message}");
                access
            }
            other => panic!("{other:?}"),
        };
        assert!(!access.is_empty());
        let mirror = crate::account_mirror::current_sign_public(&dir);
        assert_eq!(mirror.len(), 64, "the mirror must have a signing key");
        let policy = crate::access::Policy::load(&dir);
        assert!(
            policy.trusted_keys.iter().any(|key| key.eq_ignore_ascii_case(&mirror)),
            "the new public key is trusted as soon as it is created"
        );
        let cloud_key = hex::decode(policy.public_key.trim()).unwrap();
        assert!(
            pjdesk_proto::access::verify(&cloud_key, &access, now, &fp).is_err(),
            "pjdesk.com's policy key does not sign this session"
        );

        let state = state_in(&dir);
        let (host_tx, mut host_rx) = mpsc::unbounded_channel();
        state.lock().unwrap().online.insert(
            "123456789".into(),
            OnlineHost {
                conn_id: 9,
                tx: host_tx,
                hostname: "CHIP23".into(),
                app_version: "0.6.64".into(),
                addr: "192.168.0.193:21200".parse().unwrap(),
                proto: 3,
                since: Instant::now(),
                close: Arc::new(Notify::new()),
                access_key_id: String::new(),
            },
        );
        let (mut conn, mut rx) = conn_pair();
        conn.id = 2;
        conn.access_token = access;
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Connect { proto: 3, target_id: "123456789".into(), controller_name: "phone".into() },
        );
        match rx.try_recv() {
            Ok(ServerMsg::Error { message, .. }) => panic!("connect refused: {message}"),
            Ok(other) => panic!("{other:?}"),
            Err(_) => {}
        }
        match host_rx.try_recv().unwrap() {
            ServerMsg::IncomingSession { .. } => {}
            other => panic!("connect did not reach the computer: {other:?}"),
        }
        assert_eq!(state.lock().unwrap().sessions.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_blocked_computer_is_told_where_to_unblock_until_the_owner_signs_in() {
        let dir = std::env::temp_dir().join(format!("pjdesk-reblock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::access::Policy { require_accounts: true, require_chosen: true, ..crate::access::Policy::default() }
            .save(&dir)
            .unwrap();
        let state = state_in(&dir);
        let secret = [5u8; 32];
        let (mut conn, mut rx) = conn_pair();
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Register {
                proto: 3,
                device_id: Some("123456789".into()),
                device_secret: secret,
                hostname: "Office".into(),
                app_version: "0.6.49".into(),
            },
        );
        let id = match rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => device_id,
            other => panic!("{other:?}"),
        };
        state.lock().unwrap().registry.block(&id).unwrap();
        let (mut again, mut again_rx) = conn_pair();
        again.id = 2;
        handle_message(
            &state,
            &mut again,
            ClientMsg::Register {
                proto: 3,
                device_id: Some(id.clone()),
                device_secret: secret,
                hostname: "Office".into(),
                app_version: "0.6.49".into(),
            },
        );
        let message = loop {
            match again_rx.try_recv().unwrap() {
                ServerMsg::Routes(_) => continue,
                ServerMsg::Error { message, .. } => break message,
                other => panic!("{other:?}"),
            }
        };
        assert!(message.contains("Unblock"), "{message}");
        assert!(!message.contains("does not have an ID"));
        assert_ne!(message, "Sign in required");
        crate::account_mirror::note_owner_reenroll(&dir);
        let (mut back, mut back_rx) = conn_pair();
        back.id = 3;
        handle_message(
            &state,
            &mut back,
            ClientMsg::Register {
                proto: 3,
                device_id: Some(id.clone()),
                device_secret: secret,
                hostname: "Office".into(),
                app_version: "0.6.49".into(),
            },
        );
        match back_rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => assert_eq!(device_id, id),
            other => panic!("owner sign-in must register the computer again, got {other:?}"),
        }
        assert!(state.lock().unwrap().online.contains_key(&id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revoke_sign_out_then_owner_sign_in_registers_the_computer_again() {
        let dir = std::env::temp_dir().join(format!("pjdesk-revoke-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::access::Policy { require_accounts: true, require_chosen: true, ..crate::access::Policy::default() }
            .save(&dir)
            .unwrap();
        let state = state_in(&dir);
        let secret = [6u8; 32];
        let register = |conn: &mut Conn, id: Option<String>| {
            handle_message(
                &state,
                conn,
                ClientMsg::Register {
                    proto: 3,
                    device_id: id,
                    device_secret: secret,
                    hostname: "Office".into(),
                    app_version: "0.6.49".into(),
                },
            );
        };
        let (mut conn, mut rx) = conn_pair();
        register(&mut conn, Some("123456789".into()));
        let id = match rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => device_id,
            other => panic!("first registration must issue an ID, got {other:?}"),
        };
        assert!(state.lock().unwrap().online.contains_key(&id));
        state.lock().unwrap().registry.mark_signed_in(&id, true);

        // Sign out ends the session and drops the saved access. Require stays on.
        state.lock().unwrap().online.remove(&id);
        let (mut signed_out, mut out_rx) = conn_pair();
        signed_out.id = 2;
        signed_out.access_token.clear();
        register(&mut signed_out, Some(id.clone()));
        match out_rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => assert_eq!(device_id, id),
            other => panic!("sign-out must not lock the computer out of registering, got {other:?}"),
        }
        assert!(state.lock().unwrap().online.contains_key(&id));

        // Revoke puts the same computer on the blocked list and ends the session again.
        state.lock().unwrap().registry.block(&id).unwrap();
        state.lock().unwrap().online.remove(&id);
        let (mut revoked, mut rev_rx) = conn_pair();
        revoked.id = 3;
        revoked.access_token.clear();
        register(&mut revoked, Some(id.clone()));
        let message = loop {
            match rev_rx.try_recv().unwrap() {
                ServerMsg::Routes(_) => continue,
                ServerMsg::Error { message, .. } => break message,
                other => panic!("{other:?}"),
            }
        };
        assert!(message.contains("Unblock"), "{message}");
        assert!(!message.contains("does not have an ID"), "{message}");
        assert_ne!(message, "Sign in required");
        assert!(!state.lock().unwrap().online.contains_key(&id));

        // The account owner signs in with email and password, then this computer registers again.
        crate::account_mirror::note_owner_reenroll(&dir);
        let (mut back, mut back_rx) = conn_pair();
        back.id = 4;
        back.access_token.clear();
        register(&mut back, Some(id.clone()));
        match back_rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => assert_eq!(device_id, id),
            other => panic!("owner sign-in must register the computer again, got {other:?}"),
        }
        let guard = state.lock().unwrap();
        assert!(guard.online.contains_key(&id), "the computer is online after sign-in");
        assert!(!guard.registry.is_blocked_id(&id));
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grace_still_registers_a_computer_that_has_not_signed_in() {
        let dir = std::env::temp_dir().join(format!("pjdesk-grace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = state_in(&dir);
        let (mut conn, mut rx) = conn_pair();
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Register {
                proto: 3,
                device_id: None,
                device_secret: [2u8; 32],
                hostname: "Office".into(),
                app_version: "0.6.0".into(),
            },
        );
        let id = match rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => device_id,
            other => panic!("{other:?}"),
        };
        let row = state.lock().unwrap().registry.list().into_iter().find(|d| d.id == id).unwrap();
        assert!(!row.signed_in);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_install_keys_claiming_one_id_stay_two_computers() {
        let dir = std::env::temp_dir().join(format!("pjdesk-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = state_in(&dir);
        let legacy = [4u8; 32];
        let (mut first_conn, mut first_rx) = conn_pair();
        handle_message(
            &state,
            &mut first_conn,
            ClientMsg::RegisterInstall {
                proto: 3,
                device_id: Some("588773545".into()),
                device_secret: legacy,
                install_key: [1u8; 32],
                hostname: "OPAS-DESKTOP".into(),
                app_version: "0.6.46".into(),
            },
        );
        let first = match first_rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => device_id,
            other => panic!("{other:?}"),
        };
        assert_eq!(first, "588773545");
        let (mut second_conn, mut second_rx) = conn_pair();
        second_conn.id = 2;
        second_conn.addr = "192.0.2.11:40000".parse().unwrap();
        handle_message(
            &state,
            &mut second_conn,
            ClientMsg::RegisterInstall {
                proto: 3,
                device_id: Some("588773545".into()),
                device_secret: legacy,
                install_key: [2u8; 32],
                hostname: "OMAS-DESKTOP".into(),
                app_version: "0.6.46".into(),
            },
        );
        let second = match second_rx.try_recv().unwrap() {
            ServerMsg::Registered { device_id } => device_id,
            other => panic!("{other:?}"),
        };
        assert_ne!(second, first);
        let rows = state.lock().unwrap().registry.list();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().find(|row| row.id == first).unwrap().hostname, "OPAS-DESKTOP");
        assert_eq!(rows.iter().find(|row| row.id == second).unwrap().hostname, "OMAS-DESKTOP");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn incoming_only_refuses_an_outgoing_connect_and_a_blank_password_does_not_clear_it() {
        let dir = std::env::temp_dir().join(format!("pjdesk-incoming-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = state_in(&dir);
        let (mut conn, mut rx) = conn_pair();
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Register {
                proto: 3,
                device_id: Some("123456789".into()),
                device_secret: [8u8; 32],
                hostname: "Office".into(),
                app_version: "0.6.50".into(),
            },
        );
        let mut saw_off = false;
        let id = loop {
            match rx.try_recv().unwrap() {
                ServerMsg::Registered { device_id } => break device_id,
                other => panic!("registration must answer with an ID first, got {other:?}"),
            }
        };
        assert_eq!(id, "123456789");
        while let Ok(msg) = rx.try_recv() {
            if let ServerMsg::IncomingOnly { on: false } = msg {
                saw_off = true;
            }
        }
        assert!(saw_off, "a 0.6.50 computer is told the lock, including when it is off");
        handle_message(
            &state,
            &mut conn,
            ClientMsg::SetIncomingOnly { on: true, password: String::new() },
        );
        match rx.try_recv().unwrap() {
            ServerMsg::IncomingOnly { on: true } => {}
            other => panic!("turning incoming only on must be confirmed, got {other:?}"),
        }
        assert!(state.lock().unwrap().registry.incoming_only(&id));
        handle_message(
            &state,
            &mut conn,
            ClientMsg::Connect { proto: 3, target_id: "222222222".into(), controller_name: "Office".into() },
        );
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, message } => {
                assert_eq!(code, ErrorCode::Unauthorized);
                assert_eq!(message, pjdesk_proto::signal::INCOMING_ONLY_REFUSAL);
            }
            other => panic!("an incoming-only computer must not start a session, got {other:?}"),
        }
        handle_message(
            &state,
            &mut conn,
            ClientMsg::SetIncomingOnly { on: false, password: String::new() },
        );
        match rx.try_recv().unwrap() {
            ServerMsg::Error { code, message } => {
                assert_eq!(code, ErrorCode::Unauthorized);
                assert!(message.contains("account owner's password"), "{message}");
            }
            other => panic!("a blank password must not turn incoming only off, got {other:?}"),
        }
        assert!(state.lock().unwrap().registry.incoming_only(&id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn relay_session(host_tx: mpsc::UnboundedSender<ServerMsg>, ctrl_tx: mpsc::UnboundedSender<ServerMsg>) -> Session {
        Session {
            host_token: [1; 16],
            controller_token: [2; 16],
            host_id: "902488771".into(),
            host_name: "Cabin Desk".into(),
            host_conn: 11,
            host_tx,
            controller_conn: 12,
            controller_ip: "192.168.0.169".parse().unwrap(),
            controller_tx: ctrl_tx,
            accepted: true,
            host: PeerSlot::default(),
            controller: PeerSlot::default(),
            info_sent: false,
            created: Instant::now(),
            relayed: false,
            move_told: false,
            move_acked: false,
            wait_logged: String::new(),
        }
    }

    #[test]
    fn a_host_without_udp_relays_over_its_tcp_connection() {
        let dir = std::env::temp_dir().join(format!("pjdesk-tcprelay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = state_in(&dir);
        let (host_conn, mut host_rx) = conn_pair();
        let (ctrl_conn, mut ctrl_rx) = conn_pair();
        let mut host_conn = host_conn;
        host_conn.id = 11;
        let sid = 0x6850_2bbe_0593_e5b0;
        // The controller (0.6.66) bound UDP and sent UdpReady as before.
        let ctrl_udp: SocketAddr = "203.0.113.9:50000".parse().unwrap();
        {
            let mut session = relay_session(host_conn.tx.clone(), ctrl_conn.tx.clone());
            session.controller.public = Some(ctrl_udp);
            session.controller.local = Some(vec!["192.168.0.169:50000".parse().unwrap()]);
            state.lock().unwrap().sessions.insert(sid, session);
        }
        handle_message(&state, &mut host_conn, ClientMsg::UdpReady { session_id: sid, local_addrs: vec![], cert_fingerprint: Some([7; 32]) });
        assert!(ctrl_rx.try_recv().is_err(), "nothing until the host has a transport");
        handle_message(&state, &mut host_conn, ClientMsg::RelayOverTcp { session_id: sid });
        assert!(matches!(host_rx.try_recv().unwrap(), ServerMsg::TcpRelayReady { session_id } if session_id == sid));
        match host_rx.try_recv().unwrap() {
            ServerMsg::PeerInfo { public_addr, .. } => assert_eq!(public_addr, ctrl_udp),
            other => panic!("{other:?}"),
        }
        match ctrl_rx.try_recv().unwrap() {
            ServerMsg::PeerInfo { public_addr, cert_fingerprint, .. } => {
                assert_eq!(public_addr, TCP_PLACEHOLDER);
                assert_eq!(cert_fingerprint, Some([7; 32]));
            }
            other => panic!("{other:?}"),
        }
        {
            let st = state.lock().unwrap();
            assert_eq!(st.tcp_routes.get(&ctrl_udp), Some(&sid), "UDP from the controller goes to the host's TCP");
            assert!(st.sessions[&sid].relayed);
        }
        // A packet from a connection that is not in the session goes nowhere.
        crate::relay::from_tcp(&state, 99, sid, vec![0x40; 100]);
        // Both sides on TCP: packets go straight across.
        let mut ctrl_conn = ctrl_conn;
        ctrl_conn.id = 12;
        handle_message(&state, &mut ctrl_conn, ClientMsg::RelayOverTcp { session_id: sid });
        assert!(matches!(ctrl_rx.try_recv().unwrap(), ServerMsg::TcpRelayReady { .. }));
        crate::relay::from_tcp(&state, 11, sid, vec![0x41; 1200]);
        match ctrl_rx.try_recv().unwrap() {
            ServerMsg::RelayData { session_id, data } => {
                assert_eq!(session_id, sid);
                assert_eq!(data.len(), 1200);
            }
            other => panic!("{other:?}"),
        }
        crate::relay::from_tcp(&state, 12, sid, vec![0x42; 10]);
        assert!(matches!(host_rx.try_recv().unwrap(), ServerMsg::RelayData { data, .. } if data == vec![0x42; 10]));
        // Over the per-second budget, packets are dropped so QUIC slows down.
        for _ in 0..(TCP_RELAY_BYTES_PER_SEC / 1200 + 10) {
            crate::relay::from_tcp(&state, 11, sid, vec![0x41; 1200]);
        }
        let mut got = 0u64;
        while let Ok(ServerMsg::RelayData { data, .. }) = ctrl_rx.try_recv() {
            got += data.len() as u64;
        }
        assert!(got + 1200 <= TCP_RELAY_BYTES_PER_SEC, "{got}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relay_over_tcp_is_ignored_before_the_host_accepts() {
        let dir = std::env::temp_dir().join(format!("pjdesk-tcprelay2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = state_in(&dir);
        let (mut conn, mut rx) = conn_pair();
        conn.id = 12;
        let mut session = relay_session(conn.tx.clone(), conn.tx.clone());
        session.accepted = false;
        state.lock().unwrap().sessions.insert(5, session);
        handle_message(&state, &mut conn, ClientMsg::RelayOverTcp { session_id: 5 });
        assert!(rx.try_recv().is_err());
        assert!(!state.lock().unwrap().sessions[&5].controller.tcp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_host_without_a_sign_in_still_finishes_relay_setup_when_accounts_are_required() {
        let dir = std::env::temp_dir().join(format!("pjdesk-hostgate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::access::Policy { require_accounts: true, ..crate::access::Policy::default() }.save(&dir).unwrap();
        let state = state_in(&dir);
        let (mut host_conn, mut host_rx) = conn_pair();
        host_conn.id = 11;
        let (mut ctrl_conn, mut ctrl_rx) = conn_pair();
        ctrl_conn.id = 12;
        let sid = 0x0068_0001;
        state.lock().unwrap().sessions.insert(sid, relay_session(host_conn.tx.clone(), ctrl_conn.tx.clone()));
        // The host (a service with no usable token) advertises its LAN address and key.
        handle_message(
            &state,
            &mut host_conn,
            ClientMsg::UdpReady { session_id: sid, local_addrs: vec!["192.168.0.169:21201".parse().unwrap()], cert_fingerprint: Some([7; 32]) },
        );
        assert!(host_rx.try_recv().is_err(), "the host is not told to sign in");
        {
            let st = state.lock().unwrap();
            assert_eq!(st.sessions[&sid].host.fingerprint, Some([7; 32]));
        }
        handle_message(&state, &mut host_conn, ClientMsg::RelayOverTcp { session_id: sid });
        assert!(matches!(host_rx.try_recv().unwrap(), ServerMsg::TcpRelayReady { .. }));
        // An unsigned controller connection is still refused.
        handle_message(&state, &mut ctrl_conn, ClientMsg::UdpReady { session_id: sid, local_addrs: vec![], cert_fingerprint: None });
        assert!(matches!(ctrl_rx.try_recv().unwrap(), ServerMsg::Error { code: ErrorCode::Unauthorized, .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
