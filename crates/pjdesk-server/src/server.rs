//! Signaling server: registration, ID management and session setup.

use std::collections::HashMap;
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
    pub sessions: HashMap<SessionId, Session>,
    pub limiter: RateLimiter,
    pub udp_port: u16,
}

pub type Shared = Arc<Mutex<State>>;

/// A host that is currently connected.
pub struct OnlineHost {
    pub conn_id: u64,
    pub tx: mpsc::UnboundedSender<ServerMsg>,
    pub hostname: String,
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

/// Runs the server until `shutdown` completes.
pub async fn run(opts: ServerOpts, data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> Result<()> {
    std::fs::create_dir_all(&data_dir).with_context(|| format!("creating data folder {}", data_dir.display()))?;
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
        sessions: HashMap::new(),
        limiter: RateLimiter::default(),
        udp_port: opts.port,
    }));
    let acceptor = TlsAcceptor::from(Arc::new(tls::server_config(&identity, ALPN_SIGNAL)?));

    let relay = tokio::spawn(crate::relay::run(Arc::new(udp), state.clone()));
    let janitor = tokio::spawn(janitor(state.clone()));

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

    // Clean up: a host that disconnects goes offline (unless it already
    // reconnected on a newer connection).
    if let Some(id) = &conn.host_id {
        let mut st = state.lock().unwrap();
        if st.online.get(id).map(|h| h.conn_id) == Some(conn.id) {
            st.online.remove(id);
            info!("Host {} is offline", format_id(id));
        }
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
            match st.registry.register(device_id.as_deref(), &device_secret, &hostname) {
                Ok(id) => {
                    st.online.insert(
                        id.clone(),
                        OnlineHost { conn_id: conn.id, tx: conn.tx.clone(), hostname: hostname.clone() },
                    );
                    conn.host_id = Some(id.clone());
                    conn.send(ServerMsg::Registered { device_id: id.clone() });
                    info!("Host {} \"{hostname}\" online from {} (v{app_version})", format_id(&id), conn.addr.ip());
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
