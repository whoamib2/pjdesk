//! NAT traversal and the QUIC transport.
//!
//! Per session, each peer:
//! 1. binds one UDP socket and registers it with the server ([`bind_udp`]),
//!    which reveals the socket's public address;
//! 2. learns the other peer's public + LAN addresses through signaling;
//! 3. sends "punch" datagrams to them ([`spawn_punching`]) so both NATs
//!    expect traffic from the other side;
//! 4. runs QUIC on the very same socket. The controller races a connection to
//!    every candidate address and, if none answers within a few seconds,
//!    connects through the server's UDP relay instead ([`connect_to_host`]).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use desklink_proto::signal::SessionId;
use desklink_proto::tls::{self, Fingerprint, PinnedCertVerifier, TlsIdentity};
use desklink_proto::udp::{self, Role, UdpMsg};
use desklink_proto::{ALPN_PEER, PEER_SERVER_NAME};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use tokio::task::{JoinHandle, JoinSet};
use tracing::debug;

/// How long the controller tries direct connections before using the relay.
const DIRECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a relayed handshake may take.
const RELAY_TIMEOUT: Duration = Duration::from_secs(10);

/// A UDP socket registered with the server for one session.
pub struct UdpBinding {
    /// The socket QUIC will use.
    pub socket: UdpSocket,
    /// A second handle to the same socket for raw punch datagrams.
    pub punch_socket: UdpSocket,
    /// LAN candidate addresses (same port as `socket`).
    pub local_addrs: Vec<SocketAddr>,
    /// Our address as seen by the server.
    pub observed: SocketAddr,
}

/// Binds a UDP socket and registers it with the server's rendezvous port.
pub async fn bind_udp(
    server_udp: SocketAddr,
    session_id: SessionId,
    token: [u8; 16],
    role: Role,
) -> Result<UdpBinding> {
    tokio::task::spawn_blocking(move || bind_udp_blocking(server_udp, session_id, token, role)).await?
}

fn bind_udp_blocking(server_udp: SocketAddr, session_id: SessionId, token: [u8; 16], role: Role) -> Result<UdpBinding> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).context("binding a UDP socket")?;
    socket.set_read_timeout(Some(Duration::from_millis(100))).context("configuring the UDP socket")?;
    let bind_msg = udp::encode(&UdpMsg::Bind { session_id, token, role });
    let mut buf = [0u8; 2048];
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut observed = None;

    'retry: while Instant::now() < deadline {
        let _ = socket.send_to(&bind_msg, server_udp);
        let wait_until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < wait_until {
            match socket.recv_from(&mut buf) {
                Ok((len, _from)) => {
                    if let Some(UdpMsg::BindAck { session_id: sid, observed: addr }) = udp::decode(&buf[..len]) {
                        if sid == session_id {
                            observed = Some(addr);
                            break 'retry;
                        }
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                // e.g. ICMP "port unreachable" surfacing as WSAECONNRESET on Windows.
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    let observed = observed.ok_or_else(|| {
        anyhow!(
            "no UDP reply from the server at {server_udp}. Make sure UDP port {} is allowed in the server's \
             firewall (and forwarded by its router if it's behind NAT)",
            server_udp.port()
        )
    })?;

    socket.set_read_timeout(None).context("configuring the UDP socket")?;
    socket.set_nonblocking(true).context("configuring the UDP socket")?;
    let punch_socket = socket.try_clone().context("duplicating the UDP socket")?;
    let port = socket.local_addr()?.port();
    let local_addrs = local_ip_toward(server_udp).map(|ip| vec![SocketAddr::new(ip, port)]).unwrap_or_default();
    debug!("UDP socket :{port} registered; server sees us as {observed}");
    Ok(UdpBinding { socket, punch_socket, local_addrs, observed })
}

/// The local address the OS would use to reach `target` (sends nothing).
fn local_ip_toward(target: SocketAddr) -> Option<IpAddr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    probe.connect(target).ok()?;
    let ip = probe.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// All addresses worth trying for a peer: its public address plus its LAN
/// addresses, without duplicates.
pub fn candidates(public: SocketAddr, local: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut out = vec![public];
    for addr in local {
        if !out.contains(addr) && addr.is_ipv4() && !addr.ip().is_unspecified() {
            out.push(*addr);
        }
    }
    out
}

/// Sends a punch datagram to every target every 100 ms for `duration`.
pub fn spawn_punching(
    socket: UdpSocket,
    targets: Vec<SocketAddr>,
    session_id: SessionId,
    duration: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let msg = udp::encode(&UdpMsg::Punch { session_id });
        let end = tokio::time::Instant::now() + duration;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        while tokio::time::Instant::now() < end {
            tick.tick().await;
            for target in &targets {
                let _ = socket.send_to(&msg, target);
            }
        }
    })
}

fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    // Keeps NAT mappings and relay routes alive; detects dead peers quickly.
    t.keep_alive_interval(Some(Duration::from_secs(3)));
    t.max_idle_timeout(Some(quinn::IdleTimeout::try_from(Duration::from_secs(15)).expect("valid timeout")));
    // Retransmit lost handshake packets quickly while NAT pinholes open up.
    t.initial_rtt(Duration::from_millis(100));
    t.stream_receive_window(quinn::VarInt::from_u32(8 * 1024 * 1024));
    t.send_window(16 * 1024 * 1024);
    Arc::new(t)
}

fn endpoint_config() -> quinn::EndpointConfig {
    let mut config = quinn::EndpointConfig::default();
    // Keep the QUIC "fixed bit" set so our control datagrams (first byte 0x00)
    // can never be mistaken for QUIC packets.
    config.grease_quic_bit(false);
    config
}

/// Creates a QUIC endpoint on `socket`, falling back to plain UDP if the OS
/// rejects quinn's advanced socket options.
fn make_endpoint(server: Option<quinn::ServerConfig>, socket: UdpSocket) -> Result<quinn::Endpoint> {
    let runtime = Arc::new(quinn::TokioRuntime);
    match quinn::udp::UdpSocketState::new((&socket).into()) {
        Ok(_) => quinn::Endpoint::new(endpoint_config(), server, socket, runtime),
        Err(e) => {
            tracing::info!("Using plain UDP sockets (advanced socket options unavailable: {e})");
            let socket = Arc::new(crate::udp_fallback::PlainUdpSocket::new(socket)?);
            quinn::Endpoint::new_with_abstract_socket(endpoint_config(), server, socket, runtime)
        }
    }
    .context("starting QUIC on the UDP socket")
}

/// QUIC endpoint for the host: a server presenting the host's certificate.
pub fn host_endpoint(socket: UdpSocket, identity: &TlsIdentity) -> Result<quinn::Endpoint> {
    let crypto = QuicServerConfig::try_from(tls::server_config(identity, ALPN_PEER)?)?;
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(transport_config());
    make_endpoint(Some(server), socket)
}

/// QUIC endpoint for the controller, pinned to the host's certificate.
pub fn controller_endpoint(socket: UdpSocket, host_fingerprint: Fingerprint) -> Result<quinn::Endpoint> {
    let tls_config = tls::client_config(PinnedCertVerifier::new(Some(host_fingerprint)), ALPN_PEER)?;
    let mut client = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls_config)?));
    client.transport_config(transport_config());
    let mut endpoint = make_endpoint(None, socket)?;
    endpoint.set_default_client_config(client);
    Ok(endpoint)
}

/// How the controller reached the host.
#[derive(Debug, Clone, Copy)]
pub enum LinkPath {
    Direct(SocketAddr),
    Relay(SocketAddr),
}

impl fmt::Display for LinkPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkPath::Direct(addr) => write!(f, "direct P2P to {addr}"),
            LinkPath::Relay(addr) => write!(f, "relayed via {addr}"),
        }
    }
}

/// Races direct connections to all `candidates`; falls back to the relay.
pub async fn connect_to_host(
    endpoint: &quinn::Endpoint,
    candidates: &[SocketAddr],
    relay: SocketAddr,
    force_relay: bool,
) -> Result<(quinn::Connection, LinkPath)> {
    if !force_relay && !candidates.is_empty() {
        let mut attempts = JoinSet::new();
        for &addr in candidates {
            let endpoint = endpoint.clone();
            attempts.spawn(async move {
                let connection = endpoint.connect(addr, PEER_SERVER_NAME)?.await?;
                Ok::<_, anyhow::Error>((connection, addr))
            });
        }
        let winner = tokio::time::timeout(DIRECT_TIMEOUT, async {
            while let Some(result) = attempts.join_next().await {
                match result {
                    Ok(Ok(found)) => return Some(found),
                    Ok(Err(e)) => debug!("direct attempt failed: {e:#}"),
                    Err(_) => {}
                }
            }
            None
        })
        .await
        .ok()
        .flatten();
        attempts.abort_all();
        if let Some((connection, addr)) = winner {
            return Ok((connection, LinkPath::Direct(addr)));
        }
        tracing::info!("No direct route to the host (NAT/firewall); using the server's relay");
    }

    let connecting = endpoint.connect(relay, PEER_SERVER_NAME)?;
    let connection = tokio::time::timeout(RELAY_TIMEOUT, connecting)
        .await
        .map_err(|_| anyhow!("could not reach the host, even through the relay"))??;
    Ok((connection, LinkPath::Relay(relay)))
}
