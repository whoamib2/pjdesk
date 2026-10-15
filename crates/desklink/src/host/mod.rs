//! Host mode: stay online, show ID + password, stream to one controller at a time.

pub mod capture;
mod encoder;
mod input;
mod stream;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use desklink_proto::auth::generate_password;
use desklink_proto::signal::{ClientMsg, ErrorCode, ServerMsg, SessionId};
use desklink_proto::tls::TlsIdentity;
use desklink_proto::udp::Role;
use desklink_proto::{format_id, PEER_SERVER_NAME, PROTOCOL_VERSION};
use quinn::{Connection, Endpoint};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::session::{self, Authenticated, WrongPassword};
use crate::signaling::{ServerAddr, SignalClient};
use crate::{p2p, HostArgs};

/// Connection requests that may be in setup at once. Only an authenticated
/// session is exclusive, so strangers who know the ID can't keep the host
/// "busy" by sending requests they never complete.
const MAX_PENDING: usize = 3;

/// Shared state of the hosting process.
struct HostState {
    /// Certificate for the QUIC connection (its fingerprint goes to controllers).
    identity: TlsIdentity,
    password: Mutex<String>,
    fixed_password: bool,
    /// An authenticated controller is connected.
    streaming: AtomicBool,
    /// Sessions being set up (not yet authenticated).
    pending: AtomicUsize,
    /// Routes the server's PeerInfo messages to the session waiting for them.
    peer_info_routes: Mutex<HashMap<SessionId, mpsc::UnboundedSender<ServerMsg>>>,
    failures: Mutex<Failures>,
    args: HostArgs,
    name: String,
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
            Some(until) if Instant::now() < until => Some("too many wrong passwords; try again in a minute".into()),
            _ => None,
        }
    }

    /// Counts a wrong password. Random passwords are replaced after three
    /// misses; fixed passwords lock out new sessions for a minute after five.
    fn record_failure(&self) {
        let mut failures = self.failures.lock().unwrap();
        failures.count += 1;
        if !self.fixed_password && failures.count >= 3 {
            let new_password = generate_password();
            *self.password.lock().unwrap() = new_password.clone();
            failures.count = 0;
            println!("\nThree wrong passwords — the password is now: {new_password}\n");
        } else if self.fixed_password && failures.count >= 5 {
            failures.count = 0;
            failures.locked_until = Some(Instant::now() + Duration::from_secs(60));
            warn!("Five wrong passwords; refusing connections for one minute");
        }
    }

    fn reset_failures(&self) {
        self.failures.lock().unwrap().count = 0;
    }
}

/// Runs host mode until a fatal error (it reconnects to the server as needed).
pub fn run(server: ServerAddr, config: Config, args: HostArgs) -> Result<()> {
    platform_init();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    runtime.block_on(host_main(server, config, args))
}

async fn host_main(server: ServerAddr, mut config: Config, args: HostArgs) -> Result<()> {
    let state = Arc::new(HostState {
        identity: TlsIdentity::generate(PEER_SERVER_NAME)?,
        password: Mutex::new(args.password.clone().unwrap_or_else(generate_password)),
        fixed_password: args.password.is_some(),
        streaming: AtomicBool::new(false),
        pending: AtomicUsize::new(0),
        peer_info_routes: Mutex::new(HashMap::new()),
        failures: Mutex::new(Failures::default()),
        name: crate::hostname(),
        args,
    });

    let mut delay = Duration::from_secs(2);
    loop {
        match signaling_loop(&server, &mut config, &state).await {
            Ok(()) => delay = Duration::from_secs(2),
            Err(e) if format!("{e:#}").contains("SERVER KEY MISMATCH") => return Err(e),
            Err(e) => warn!("{e:#}"),
        }
        info!("Reconnecting to the server in {} s...", delay.as_secs());
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

/// One signaling connection: register, then handle incoming sessions.
/// Returns `Ok` if we were registered before the connection dropped.
async fn signaling_loop(server: &ServerAddr, config: &mut Config, state: &Arc<HostState>) -> Result<()> {
    let mut signal = SignalClient::connect(server, config).await?;
    let device_secret = config.device_secret()?;
    signal.send(ClientMsg::Register {
        proto: PROTOCOL_VERSION,
        device_id: config.device_id.clone(),
        device_secret,
        hostname: state.name.clone(),
        app_version: env!("CARGO_PKG_VERSION").into(),
    })?;

    let device_id = match timeout(Duration::from_secs(10), signal.recv()).await {
        Ok(Some(ServerMsg::Registered { device_id })) => device_id,
        Ok(Some(ServerMsg::Error { code: ErrorCode::IdConflict, message })) => {
            config.device_id = None;
            config.save()?;
            bail!("{message}; a new ID will be requested");
        }
        Ok(Some(ServerMsg::Error { message, .. })) => bail!("the server refused registration: {message}"),
        Ok(Some(other)) => bail!("unexpected reply from the server: {other:?}"),
        Ok(None) => bail!("the server closed the connection"),
        Err(_) => bail!("the server did not answer"),
    };
    if config.device_id.as_deref() != Some(device_id.as_str()) {
        config.device_id = Some(device_id.clone());
        config.save()?;
    }
    print_banner(&device_id, &state.password.lock().unwrap(), state.fixed_password, server);
    crate::HOST_ONLINE.store(true, Ordering::SeqCst);

    let server_ip = signal.server_addr.ip();
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    ping.tick().await;

    loop {
        tokio::select! {
            _ = ping.tick() => signal.send(ClientMsg::Ping)?,
            msg = signal.recv() => {
                let Some(msg) = msg else {
                    warn!("Lost the connection to the server");
                    return Ok(());
                };
                match msg {
                    ServerMsg::IncomingSession { session_id, token, udp_port, controller_name, controller_ip } => {
                        if let Some(reason) = state.refuse_reason() {
                            info!("Refused a connection from \"{controller_name}\" ({controller_ip}): {reason}");
                            signal.send(ClientMsg::SessionResponse { session_id, accept: false, reason })?;
                            continue;
                        }
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
                    ServerMsg::Pong => {}
                    ServerMsg::Error { message, .. } => warn!("Server: {message}"),
                    other => debug!("ignoring {other:?}"),
                }
            }
        }
    }
}

fn print_banner(id: &str, password: &str, fixed: bool, server: &ServerAddr) {
    println!();
    println!("  ==================================================");
    println!("    This computer can now be controlled remotely");
    println!("      ID       : {}", format_id(id));
    println!("      Password : {password}{}", if fixed { "  (fixed)" } else { "" });
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

    let password = state.password.lock().unwrap().clone();
    let accepted = accept_authenticated(&endpoint, &password, &state.name, Duration::from_secs(20)).await;
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

    let path = if connection.remote_address() == server_udp { "relayed via the server" } else { "direct P2P" };
    info!("\"{}\" connected ({path}, {})", auth.peer_name, connection.remote_address());
    let result = stream::run(connection.clone(), auth.send, auth.recv, state.args.clone()).await;
    state.streaming.store(false, Ordering::SeqCst);
    connection.close(0u32.into(), b"session over");
    let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    result
}

/// Accepts QUIC connections until one authenticates. The controller may try
/// several addresses at once, so several connections can arrive, but only
/// one password attempt is allowed per session.
async fn accept_authenticated(
    endpoint: &Endpoint,
    password: &str,
    my_name: &str,
    limit: Duration,
) -> Result<(Connection, Authenticated)> {
    const MAX_CONNECTIONS: usize = 4;
    let deadline = tokio::time::Instant::now() + limit;
    let attempt_used = Arc::new(AtomicBool::new(false));
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
                let (password, my_name, attempt_used) = (password.to_owned(), my_name.to_owned(), attempt_used.clone());
                pending.spawn(async move {
                    let connection = incoming.await?;
                    let auth = session::host_auth(&connection, &password, &my_name, &attempt_used).await?;
                    Ok((connection, auth))
                });
            }
            Some(joined) = pending.join_next(), if !pending.is_empty() => match joined {
                Ok(Ok(done)) => return Ok(done),
                Ok(Err(e)) if e.is::<WrongPassword>() => return Err(e),
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
