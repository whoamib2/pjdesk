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
use pjdesk_proto::signal::SessionId;
use pjdesk_proto::tls::{self, Fingerprint, PinnedCertVerifier, TlsIdentity};
use pjdesk_proto::udp::{self, Role, UdpMsg};
use pjdesk_proto::{ALPN_PEER, PEER_SERVER_NAME};
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

/// True for a private or link-local address (a same-LAN candidate).
pub fn is_lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            (octets[0] & 0xfe) == 0xfc || (octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80)
        }
    }
}

/// All addresses worth trying for a peer. LAN addresses come first so two
/// computers on the same network do not hairpin out through the public IP.
pub fn candidates(public: SocketAddr, local: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut lan = Vec::new();
    let mut other = Vec::new();
    for addr in local {
        if *addr == public || !addr.is_ipv4() || addr.ip().is_unspecified() || addr.ip().is_loopback() {
            continue;
        }
        if is_lan_ip(addr.ip()) {
            if !lan.contains(addr) {
                lan.push(*addr);
            }
        } else if !other.contains(addr) {
            other.push(*addr);
        }
    }
    let mut out = lan;
    if !out.contains(&public) {
        out.push(public);
    }
    for addr in other {
        if !out.contains(&addr) {
            out.push(addr);
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
    Ok(controller_endpoint_pin(socket, Some(host_fingerprint))?.0)
}

/// Like [`controller_endpoint`], but `None` accepts the certificate on first use
/// and returns the verifier so the caller can store the fingerprint it saw.
pub fn controller_endpoint_pin(
    socket: UdpSocket,
    host_fingerprint: Option<Fingerprint>,
) -> Result<(quinn::Endpoint, Arc<PinnedCertVerifier>)> {
    let verifier = PinnedCertVerifier::new(host_fingerprint);
    let tls_config = tls::client_config(verifier.clone(), ALPN_PEER)?;
    let mut client = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls_config)?));
    client.transport_config(transport_config());
    let mut endpoint = make_endpoint(None, socket)?;
    endpoint.set_default_client_config(client);
    Ok((endpoint, verifier))
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
            LinkPath::Direct(addr) if is_lan_ip(addr.ip()) => write!(f, "LAN ({addr})"),
            LinkPath::Direct(addr) => write!(f, "Internet ({addr})"),
            LinkPath::Relay(addr) => write!(f, "Internet, relayed via {addr}"),
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
        let any_lan = candidates.iter().any(|addr| is_lan_ip(addr.ip()));
        let mut attempts = JoinSet::new();
        for &addr in candidates {
            let endpoint = endpoint.clone();
            // A LAN candidate starts immediately. The public address waits so a
            // same-LAN handshake wins when both are reachable. The host key is
            // checked on every path by this endpoint's pinned verifier.
            let delay = if any_lan && !is_lan_ip(addr.ip()) { Duration::from_millis(300) } else { Duration::ZERO };
            attempts.spawn(async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
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

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    #[test]
    fn lan_candidates_are_tried_before_the_public_address() {
        let public = "203.0.113.5:21116".parse().unwrap();
        let local = vec!["192.168.1.20:9".parse().unwrap(), "10.0.0.4:9".parse().unwrap(), public];
        let got = candidates(public, &local);
        assert!(is_lan_ip(got[0].ip()));
        assert!(is_lan_ip(got[1].ip()));
        assert_eq!(*got.last().unwrap(), public);
    }

    #[test]
    fn the_path_says_lan_or_internet() {
        let lan: SocketAddr = "192.168.1.9:1".parse().unwrap();
        let wan: SocketAddr = "203.0.113.8:1".parse().unwrap();
        assert_eq!(LinkPath::Direct(lan).to_string(), "LAN (192.168.1.9:1)");
        assert_eq!(LinkPath::Direct(wan).to_string(), "Internet (203.0.113.8:1)");
        assert_eq!(LinkPath::Relay(wan).to_string(), "Internet, relayed via 203.0.113.8:1");
    }
}
