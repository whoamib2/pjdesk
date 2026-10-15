//! Host mode: stay online, show ID + password, accept one controller at a time.

pub mod capture;
mod encoder;
mod input;
mod stream;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Shared state of the hosting process.
struct HostState {
    /// Certificate for the QUIC connection (its fingerprint goes to controllers).
    identity: TlsIdentity,
    password: Mutex<String>,
    fixed_password: bool,
    busy: AtomicBool,
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
        if self.busy.load(Ordering::SeqCst) {
            return Some("this computer is already in a remote session".into());
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
        busy: AtomicBool::new(false),
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
    // Routes PeerInfo messages to the session task that is waiting for them.
    let mut active: Option<(SessionId, mpsc::UnboundedSender<ServerMsg>)> = None;

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
                        state.busy.store(true, Ordering::SeqCst);
                        signal.send(ClientMsg::SessionResponse { session_id, accept: true, reason: String::new() })?;
                        let (tx, rx) = mpsc::unbounded_channel();
                        active = Some((session_id, tx));
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
                        if let Some((id, tx)) = &active {
                            if *id == session_id {
                                let _ = tx.send(msg);
                            }
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
    match session_inner(&state, signal, peer_info, session_id, token, server_udp).await {
        Ok(()) => info!("Remote session ended"),
        Err(e) if e.is::<WrongPassword>() => {
            warn!("\"{controller_name}\" ({controller_ip}) entered a wrong password");
            state.record_failure();
        }
        Err(e) => warn!("Remote session failed: {e:#}"),
    }
    state.busy.store(false, Ordering::SeqCst);
}

async fn session_inner(
    state: &Arc<HostState>,
    signal: mpsc::UnboundedSender<ClientMsg>,
    mut peer_info: mpsc::UnboundedReceiver<ServerMsg>,
    session_id: SessionId,
    token: [u8; 16],
    server_udp: SocketAddr,
) -> Result<()> {
    let binding = p2p::bind_udp(server_udp, session_id, token, Role::Host).await?;
    debug!("our public address for this session: {}", binding.observed);
    let endpoint = p2p::host_endpoint(binding.socket, &state.identity)?;
    signal.send(ClientMsg::UdpReady {
        session_id,
        local_addrs: binding.local_addrs.clone(),
        cert_fingerprint: Some(state.identity.fingerprint()),
    })?;

    let (peer_public, peer_local) = match timeout(Duration::from_secs(20), peer_info.recv()).await {
        Ok(Some(ServerMsg::PeerInfo { public_addr, local_addrs, .. })) => (public_addr, local_addrs),
        _ => bail!("the controller never finished setting up the connection"),
    };
    let targets = p2p::candidates(peer_public, &peer_local);
    debug!("controller candidates: {targets:?}");
    let punching = p2p::spawn_punching(binding.punch_socket, targets, session_id, Duration::from_secs(8));

    let password = state.password.lock().unwrap().clone();
    let accepted = accept_authenticated(&endpoint, &password, &state.name, Duration::from_secs(30)).await;
    punching.abort();
    let (connection, auth) = accepted?;
    state.reset_failures();

    let path = if connection.remote_address() == server_udp { "relayed via the server" } else { "direct P2P" };
    info!("\"{}\" connected ({path}, {})", auth.peer_name, connection.remote_address());
    let result = stream::run(connection.clone(), auth.send, auth.recv, state.args.clone()).await;
    connection.close(0u32.into(), b"session over");
    let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    result
}

/// Accepts QUIC connections until one authenticates (several may arrive when
/// the controller tries multiple addresses at once).
async fn accept_authenticated(
    endpoint: &Endpoint,
    password: &str,
    my_name: &str,
    limit: Duration,
) -> Result<(Connection, Authenticated)> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut pending: JoinSet<Result<(Connection, Authenticated)>> = JoinSet::new();
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { bail!("the QUIC endpoint closed") };
                let (password, my_name) = (password.to_owned(), my_name.to_owned());
                pending.spawn(async move {
                    let connection = incoming.await?;
                    let auth = session::host_auth(&connection, &password, &my_name).await?;
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
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        // Physical pixels everywhere: capture, cursor position and SendInput agree.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
