//! LAN discovery beacons.
//!
//! A host that allows direct connections broadcasts a small packet every few
//! seconds. It carries the device ID, the direct-connect port, the computer
//! name and the QUIC certificate fingerprint. It does not carry a password.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const BEACON_MAGIC: &[u8; 8] = b"PJDESKLB";
const MAX_BODY: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanBeacon {
    pub proto: u16,
    pub id: String,
    pub port: u16,
    pub name: String,
    pub fingerprint: [u8; 32],
}

pub fn encode(beacon: &LanBeacon) -> Result<Vec<u8>> {
    if beacon.name.len() > 64 || beacon.id.len() > 16 {
        bail!("discovery beacon is too large");
    }
    let body = postcard::to_stdvec(beacon)?;
    if body.len() > MAX_BODY {
        bail!("discovery beacon is too large");
    }
    let mut out = Vec::with_capacity(8 + 2 + body.len());
    out.extend_from_slice(BEACON_MAGIC);
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Magic for a server announcing its signaling address on the LAN.
/// Separate from [`BEACON_MAGIC`], so a computer beacon is never read as a server.
pub const SERVER_MAGIC: &[u8; 8] = b"PJDESKSV";

/// The PJDesk server's LAN signaling endpoint. The packet's source address is
/// the LAN IP. `port` is the TCP/UDP server port (21200). The fingerprint is
/// the server key, so a client with a pinned key can ignore a different server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerBeacon {
    pub port: u16,
    pub fingerprint: [u8; 32],
}

pub fn encode_server(beacon: &ServerBeacon) -> Vec<u8> {
    let mut out = Vec::with_capacity(42);
    out.extend_from_slice(SERVER_MAGIC);
    out.extend_from_slice(&beacon.port.to_be_bytes());
    out.extend_from_slice(&beacon.fingerprint);
    out
}

pub fn decode_server(packet: &[u8]) -> Result<ServerBeacon> {
    if packet.len() != 42 || &packet[..8] != SERVER_MAGIC {
        bail!("not a PJDesk server beacon");
    }
    let port = u16::from_be_bytes([packet[8], packet[9]]);
    if port == 0 {
        bail!("server beacon has no port");
    }
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&packet[10..42]);
    Ok(ServerBeacon { port, fingerprint })
}

/// Listens on UDP 21202 for [ServerBeacon]s. The returned address uses the
/// sender's IP and the port inside the packet. A busy port returns nothing.
pub fn listen_servers(limit: std::time::Duration) -> Vec<(std::net::SocketAddr, [u8; 32])> {
    let socket = match std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, crate::DISCOVERY_PORT)) {
        Ok(socket) => socket,
        Err(_) => return Vec::new(),
    };
    let _ = socket.set_read_timeout(Some(std::time::Duration::from_millis(80)));
    let deadline = std::time::Instant::now() + limit;
    let mut found = Vec::new();
    let mut buf = [0u8; 128];
    while std::time::Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Ok(beacon) = decode_server(&buf[..n]) {
                    if let std::net::IpAddr::V4(ip) = src.ip() {
                        if ip.is_private() || ip.is_link_local() {
                            let addr = std::net::SocketAddr::from((ip, beacon.port));
                            if !found.iter().any(|(have, fp)| *have == addr && *fp == beacon.fingerprint) {
                                found.push((addr, beacon.fingerprint));
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    found
}

pub fn decode(packet: &[u8]) -> Result<LanBeacon> {
    if packet.len() < 10 || &packet[..8] != BEACON_MAGIC {
        bail!("not a PJDesk discovery beacon");
    }
    let len = u16::from_be_bytes([packet[8], packet[9]]) as usize;
    if len > MAX_BODY || packet.len() < 10 + len {
        bail!("discovery beacon is truncated");
    }
    Ok(postcard::from_bytes(&packet[10..10 + len])?)
}

/// True when `host` is a loopback name. Port forwards default to these only,
/// so a session cannot turn the host into an open proxy.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim();
    h == "127.0.0.1" || h.eq_ignore_ascii_case("localhost") || h == "::1"
}

/// A forward is allowed when the port was listed by the person at the computer
/// and the target is loopback, unless they explicitly allowed other hosts.
pub fn tunnel_allowed(host: &str, port: u16, allow_ports: &[u16], allow_remote_hosts: bool) -> bool {
    if port == 0 || !allow_ports.contains(&port) {
        return false;
    }
    allow_remote_hosts || is_loopback_host(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_roundtrip_and_reject() {
        let b = LanBeacon {
            proto: 3,
            id: "123456789".into(),
            port: crate::DIRECT_PORT,
            name: "cabindesk".into(),
            fingerprint: [7u8; 32],
        };
        let bytes = encode(&b).unwrap();
        assert_eq!(decode(&bytes).unwrap(), b);
        assert!(decode(b"nope").is_err());
        assert!(decode(&bytes[..9]).is_err());
        let server = encode_server(&ServerBeacon { port: crate::DEFAULT_PORT, fingerprint: [9u8; 32] });
        assert!(decode(&server).is_err());
    }

    #[test]
    fn server_beacon_is_a_fixed_packet() {
        let beacon = ServerBeacon { port: crate::DEFAULT_PORT, fingerprint: [4u8; 32] };
        let bytes = encode_server(&beacon);
        assert_eq!(&bytes[..8], b"PJDESKSV");
        assert_eq!(decode_server(&bytes).unwrap(), beacon);
        assert!(decode_server(b"PJDESKSV").is_err());
        assert!(decode_server(&[0u8; 42]).is_err());
    }

    #[test]
    fn tunnel_defaults_to_listed_loopback_ports() {
        assert!(tunnel_allowed("127.0.0.1", 3389, &[3389], false));
        assert!(tunnel_allowed("localhost", 22, &[22], false));
        assert!(!tunnel_allowed("10.0.0.5", 22, &[22], false));
        assert!(tunnel_allowed("10.0.0.5", 22, &[22], true));
        assert!(!tunnel_allowed("127.0.0.1", 80, &[22], false));
        assert!(!tunnel_allowed("127.0.0.1", 0, &[0], false));
    }
}
