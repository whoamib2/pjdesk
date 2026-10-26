//! LAN discovery beacons.
//!
//! A host that allows direct connections broadcasts a small packet every few
//! seconds. It carries the device ID, the direct-connect port, the computer
//! name and the QUIC certificate fingerprint. It does not carry a password.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// Whether the server that sent a beacon is the live primary.
/// A 42-byte beacon from 0.6.30 and earlier has [`BeaconRole::Unknown`].
/// That is not proof of the primary: a standby shares the server key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BeaconRole {
    #[default]
    Unknown,
    Primary,
    Standby,
}

impl BeaconRole {
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Primary,
            2 => Self::Standby,
            _ => Self::Unknown,
        }
    }

    pub fn to_byte(self) -> u8 {
        match self {
            Self::Primary => 1,
            Self::Standby => 2,
            Self::Unknown => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Standby => "standby",
            Self::Unknown => "unspecified",
        }
    }
}

/// The PJDesk server's LAN signaling endpoint. The packet's source address is
/// the LAN IP. `port` is the TCP/UDP server port (21200). The fingerprint is
/// the server key, so a client with a pinned key can ignore a different server.
/// `role` is an optional trailing byte. Older servers omit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerBeacon {
    pub port: u16,
    pub fingerprint: [u8; 32],
    pub role: BeaconRole,
}

pub fn encode_server(beacon: &ServerBeacon) -> Vec<u8> {
    let mut out = Vec::with_capacity(43);
    out.extend_from_slice(SERVER_MAGIC);
    out.extend_from_slice(&beacon.port.to_be_bytes());
    out.extend_from_slice(&beacon.fingerprint);
    out.push(beacon.role.to_byte());
    out
}

pub fn decode_server(packet: &[u8]) -> Result<ServerBeacon> {
    if (packet.len() != 42 && packet.len() != 43) || &packet[..8] != SERVER_MAGIC {
        bail!("not a PJDesk server beacon");
    }
    let port = u16::from_be_bytes([packet[8], packet[9]]);
    if port == 0 {
        bail!("server beacon has no port");
    }
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&packet[10..42]);
    let role = if packet.len() == 43 { BeaconRole::from_byte(packet[42]) } else { BeaconRole::Unknown };
    Ok(ServerBeacon { port, fingerprint, role })
}

struct HeardServer {
    addr: SocketAddr,
    fingerprint: [u8; 32],
    role: BeaconRole,
    at: Instant,
}

fn server_slot() -> &'static Mutex<Vec<HeardServer>> {
    static SLOT: std::sync::OnceLock<Mutex<Vec<HeardServer>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(Vec::new()))
}

/// Records a server beacon already received on the shared discovery socket.
/// [`listen_servers`] returns these when that socket is already open.
pub fn note_server_beacon(addr: SocketAddr, fingerprint: [u8; 32], role: BeaconRole) {
    let mut slot = server_slot().lock().unwrap_or_else(|err| err.into_inner());
    let now = Instant::now();
    slot.retain(|heard| now.saturating_duration_since(heard.at) < Duration::from_secs(8));
    if let Some(have) = slot.iter_mut().find(|heard| heard.addr == addr && heard.fingerprint == fingerprint) {
        have.role = role;
        have.at = now;
        return;
    }
    slot.push(HeardServer { addr, fingerprint, role, at: now });
}

fn cached_servers() -> Vec<(SocketAddr, ServerBeacon)> {
    let mut slot = server_slot().lock().unwrap_or_else(|err| err.into_inner());
    let now = Instant::now();
    slot.retain(|heard| now.saturating_duration_since(heard.at) < Duration::from_secs(8));
    slot.iter()
        .map(|heard| {
            (heard.addr, ServerBeacon { port: heard.addr.port(), fingerprint: heard.fingerprint, role: heard.role })
        })
        .collect()
}

/// Listens on UDP 21202 for [ServerBeacon]s. The returned address uses the
/// sender's IP and the port inside the packet. A busy port returns a beacon
/// the window already heard, or nothing.
pub fn listen_servers(limit: std::time::Duration) -> Vec<(std::net::SocketAddr, ServerBeacon)> {
    let cached = cached_servers();
    if !cached.is_empty() {
        return cached;
    }
    let socket = match std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, crate::DISCOVERY_PORT)) {
        Ok(socket) => socket,
        Err(_) => return Vec::new(),
    };
    collect_beacons(socket, limit)
}

/// Reads beacons until `limit`. The socket is non-blocking, so a failed
/// `SO_RCVTIMEO` cannot leave the registrar blocked in `recv` forever.
fn collect_beacons(
    socket: std::net::UdpSocket,
    limit: std::time::Duration,
) -> Vec<(std::net::SocketAddr, ServerBeacon)> {
    let _ = socket.set_nonblocking(true);
    let _ = socket.set_read_timeout(Some(std::time::Duration::from_millis(40)));
    let deadline = std::time::Instant::now() + limit;
    let mut found: Vec<(std::net::SocketAddr, ServerBeacon)> = Vec::new();
    let mut buf = [0u8; 128];
    while std::time::Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Ok(beacon) = decode_server(&buf[..n]) {
                    if let std::net::IpAddr::V4(ip) = src.ip() {
                        if ip.is_private() || ip.is_link_local() {
                            let addr = std::net::SocketAddr::from((ip, beacon.port));
                            if !found
                                .iter()
                                .any(|(have, heard)| *have == addr && heard.fingerprint == beacon.fingerprint)
                            {
                                found.push((addr, beacon));
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                std::thread::sleep(left.min(std::time::Duration::from_millis(40)));
            }
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

/// A discovery packet from this address may be shown.
/// Public addresses are ignored, so a beacon is only accepted on a private network.
pub fn accept_lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => accept_lan_ipv4(v4),
        IpAddr::V6(v6) => {
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

/// Private, link-local, loopback, or shared (carrier-grade NAT) IPv4.
/// Unspecified, broadcast, and multicast addresses are not peers.
pub fn accept_lan_ipv4(ip: Ipv4Addr) -> bool {
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return false;
    }
    let [a, b, _, _] = ip.octets();
    ip.is_loopback() || ip.is_private() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
}

/// True when `ips` includes a private LAN address worth announcing on.
/// Loopback alone does not count, so a public-only host stays quiet.
pub fn announce_allowed(ips: &[Ipv4Addr]) -> bool {
    ips.iter().copied().any(is_announce_ipv4)
}

fn is_announce_ipv4(ip: Ipv4Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return false;
    }
    let [a, b, _, _] = ip.octets();
    ip.is_private() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
}

/// Announce only when discovery is still on and a private interface exists.
/// `opted_out` is the Settings switch. The beacon itself never carries a password.
pub fn announce_now(opted_out: bool, discover_lan: bool, ips: &[Ipv4Addr]) -> bool {
    !opted_out && discover_lan && announce_allowed(ips)
}

/// IPv4 addresses this host would use toward a private network.
/// Empty when every route is public. Does not open a listening socket.
pub fn private_ipv4s() -> Vec<Ipv4Addr> {
    let mut ips = Vec::new();
    for dest in ["10.255.255.254:9", "172.31.255.254:9", "192.168.255.254:9", "100.127.255.254:9"] {
        let Ok(socket) = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { continue };
        let Ok(target) = dest.parse::<std::net::SocketAddr>() else { continue };
        if socket.connect(target).is_err() {
            continue;
        }
        if let Some(IpAddr::V4(ip)) = socket.local_addr().ok().map(|addr| addr.ip()) {
            if is_announce_ipv4(ip) {
                ips.push(ip);
            }
        }
    }
    ips.sort();
    ips.dedup();
    ips
}

/// Addresses to dial. `public` is the primary's reachable address. `lan` is
/// only a current primary on this same network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachPlan {
    pub public: SocketAddr,
    pub lan: Vec<SocketAddr>,
}

/// Chooses signaling addresses.
///
/// A LAN address, including this computer's own addresses and localhost, is
/// used only when it is the current primary. A standby beacon is never a
/// candidate, and neither is a role-less beacon: 0.6.30 standbys share the
/// server key and still send those. Account LAN addresses are used only when
/// `same_site` (this computer's public IP is the primary's). DNS that returns
/// this computer, or a private address on a different network, is replaced
/// with `published_public` when that is a public IPv4 address.
pub fn reach_plan(
    resolved: SocketAddr,
    published_public: &str,
    port: u16,
    same_site: bool,
    account_lan: &[SocketAddr],
    beacons: &[(SocketAddr, [u8; 32], BeaconRole)],
    pinned: Option<[u8; 32]>,
    own: &[Ipv4Addr],
) -> ReachPlan {
    let published = published_public_addr(published_public, if port == 0 { resolved.port() } else { port });
    let primary_here =
        |addr: SocketAddr| beacon_is_primary(addr, beacons, pinned) || (same_site && account_lan.contains(&addr));
    let public = if dns_is_local_or_foreign(resolved, same_site, own) {
        if primary_here(resolved) {
            resolved
        } else {
            published.unwrap_or(resolved)
        }
    } else {
        resolved
    };
    let mut lan = Vec::new();
    let mut push = |addr: SocketAddr| {
        if addr == public || lan.contains(&addr) {
            return;
        }
        let local = is_own_addr(addr, own);
        if local && !beacon_is_primary(addr, beacons, pinned) && !(same_site && account_lan.contains(&addr)) {
            return;
        }
        lan.push(addr);
    };
    if same_site {
        for addr in account_lan {
            push(*addr);
        }
    }
    for (addr, fingerprint, role) in beacons {
        if *role == BeaconRole::Primary && fingerprint_matches(*fingerprint, pinned) {
            push(*addr);
        }
    }
    ReachPlan { public, lan }
}

fn beacon_is_primary(
    addr: SocketAddr,
    beacons: &[(SocketAddr, [u8; 32], BeaconRole)],
    pinned: Option<[u8; 32]>,
) -> bool {
    beacons.iter().any(|(heard, fingerprint, role)| {
        *heard == addr && *role == BeaconRole::Primary && fingerprint_matches(*fingerprint, pinned)
    })
}

fn fingerprint_matches(fingerprint: [u8; 32], pinned: Option<[u8; 32]>) -> bool {
    pinned.is_none_or(|expected| expected == fingerprint)
}

fn dns_is_local_or_foreign(resolved: SocketAddr, same_site: bool, own: &[Ipv4Addr]) -> bool {
    if is_own_addr(resolved, own) {
        return true;
    }
    is_private_socket(resolved) && !same_site
}

fn is_own_addr(addr: SocketAddr, own: &[Ipv4Addr]) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => ip.is_loopback() || own.contains(&ip),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

fn is_private_socket(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(_) => false,
    }
}

fn published_public_addr(text: &str, port: u16) -> Option<SocketAddr> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let addr = if let Ok(addr) = text.parse::<SocketAddr>() {
        addr
    } else if let Ok(ip) = text.parse::<Ipv4Addr>() {
        SocketAddr::new(IpAddr::V4(ip), port)
    } else {
        return None;
    };
    match addr.ip() {
        IpAddr::V4(ip) if !ip.is_private() && !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() => {
            Some(addr)
        }
        _ => None,
    }
}

/// The first two groups of a formatted fingerprint, such as `d2fd-3afd`.
pub fn fingerprint_prefix(text: &str) -> String {
    let groups: Vec<&str> = text.split('-').filter(|part| !part.is_empty()).take(2).collect();
    if groups.len() >= 2 {
        format!("{}-{}", groups[0], groups[1])
    } else {
        text.chars().take(9).collect()
    }
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
        // The Android decoder matches these bytes. Do not reorder the postcard fields.
        assert_eq!(
            bytes,
            [
                0x50, 0x4a, 0x44, 0x45, 0x53, 0x4b, 0x4c, 0x42, 0x00, 0x38, 0x03, 0x09, 0x31, 0x32, 0x33, 0x34, 0x35,
                0x36, 0x37, 0x38, 0x39, 0xd1, 0xa5, 0x01, 0x09, 0x63, 0x61, 0x62, 0x69, 0x6e, 0x64, 0x65, 0x73, 0x6b,
                0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07,
                0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07, 0x07,
            ]
        );
        assert!(decode(b"nope").is_err());
        assert!(decode(&bytes[..9]).is_err());
        let server = encode_server(&ServerBeacon {
            port: crate::DEFAULT_PORT,
            fingerprint: [9u8; 32],
            role: BeaconRole::Primary,
        });
        assert!(decode(&server).is_err());
    }

    #[test]
    fn server_beacon_keeps_a_role_byte_and_still_reads_the_old_packet() {
        let beacon = ServerBeacon { port: crate::DEFAULT_PORT, fingerprint: [4u8; 32], role: BeaconRole::Primary };
        let bytes = encode_server(&beacon);
        assert_eq!(&bytes[..8], b"PJDESKSV");
        assert_eq!(bytes.len(), 43);
        assert_eq!(bytes[42], 1);
        assert_eq!(decode_server(&bytes).unwrap(), beacon);
        let standby = ServerBeacon { role: BeaconRole::Standby, ..beacon.clone() };
        assert_eq!(encode_server(&standby)[42], 2);
        assert_eq!(decode_server(&encode_server(&standby)).unwrap().role, BeaconRole::Standby);
        let mut legacy = bytes[..42].to_vec();
        assert_eq!(decode_server(&legacy).unwrap().role, BeaconRole::Unknown);
        legacy.push(9);
        assert_eq!(decode_server(&legacy).unwrap().role, BeaconRole::Unknown);
        assert!(decode_server(b"PJDESKSV").is_err());
        assert!(decode_server(&[0u8; 42]).is_err());
    }

    #[test]
    fn a_local_standby_is_not_the_signaling_address() {
        let own = [Ipv4Addr::new(10, 0, 0, 20)];
        let key = [7u8; 32];
        let local: SocketAddr = "10.0.0.20:21200".parse().unwrap();
        let primary_lan: SocketAddr = "192.168.0.231:21200".parse().unwrap();
        let public: SocketAddr = "204.248.122.44:21200".parse().unwrap();
        let standby = vec![(local, key, BeaconRole::Standby)];
        let unspecified = vec![(local, key, BeaconRole::Unknown)];
        let plan = reach_plan(local, "204.248.122.44", 21200, false, &[primary_lan], &standby, Some(key), &own);
        assert_eq!(plan.public, public);
        assert!(plan.lan.is_empty(), "{plan:?}");
        let legacy = reach_plan(local, "204.248.122.44", 21200, false, &[primary_lan], &unspecified, Some(key), &own);
        assert_eq!(legacy.public, public);
        assert!(legacy.lan.is_empty());
        let wrong_key = vec![(local, [1u8; 32], BeaconRole::Primary)];
        let mismatched =
            reach_plan(public, "204.248.122.44", 21200, false, &[primary_lan], &wrong_key, Some(key), &own);
        assert_eq!(mismatched.public, public);
        assert!(mismatched.lan.is_empty());
    }

    #[test]
    fn the_primary_on_this_network_is_still_used() {
        let own = [Ipv4Addr::new(192, 168, 0, 231)];
        let key = [7u8; 32];
        let local: SocketAddr = "192.168.0.231:21200".parse().unwrap();
        let public: SocketAddr = "204.248.122.44:21200".parse().unwrap();
        let beacons = vec![(local, key, BeaconRole::Primary)];
        let plan = reach_plan(local, "204.248.122.44", 21200, true, &[local], &beacons, Some(key), &own);
        assert_eq!(plan.public, local);
        assert!(plan.lan.is_empty());
        let other = [Ipv4Addr::new(192, 168, 0, 50)];
        let client = reach_plan(public, "204.248.122.44", 21200, true, &[local], &beacons, Some(key), &other);
        assert_eq!(client.public, public);
        assert_eq!(client.lan, vec![local]);
        let loopback: SocketAddr = "127.0.0.1:21200".parse().unwrap();
        let cabin = reach_plan(loopback, "204.248.122.44", 21200, false, &[local], &[], Some(key), &[]);
        assert_eq!(cabin.public, public);
        assert!(cabin.lan.is_empty());
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

    #[test]
    fn beacon_roundtrip_carries_no_password() {
        let beacon = LanBeacon {
            proto: 3,
            id: "692996394".into(),
            port: crate::DIRECT_PORT,
            name: "cabindesk".into(),
            fingerprint: [3u8; 32],
        };
        let bytes = encode(&beacon).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.to_ascii_lowercase().contains("password"));
        assert!(!text.to_ascii_lowercase().contains("support"));
        assert_eq!(decode(&bytes).unwrap(), beacon);
        assert_eq!(&bytes[..8], b"PJDESKLB");
    }

    #[test]
    fn lan_answers_only_on_private_networks() {
        assert!(accept_lan_ipv4(Ipv4Addr::new(192, 168, 1, 20)));
        assert!(accept_lan_ipv4(Ipv4Addr::new(10, 1, 2, 3)));
        assert!(accept_lan_ipv4(Ipv4Addr::new(172, 16, 5, 5)));
        assert!(accept_lan_ipv4(Ipv4Addr::new(169, 254, 1, 1)));
        assert!(accept_lan_ipv4(Ipv4Addr::new(100, 64, 1, 1)));
        assert!(accept_lan_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!accept_lan_ipv4(Ipv4Addr::new(8, 8, 8, 8)));
        assert!(!accept_lan_ipv4(Ipv4Addr::new(203, 0, 113, 10)));
        assert!(!accept_lan_ipv4(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(!accept_lan_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(accept_lan_ip("fe80::1".parse().unwrap()));
        assert!(!accept_lan_ip("2001:db8::1".parse().unwrap()));

        let lan = [Ipv4Addr::new(192, 168, 1, 20)];
        let public = [Ipv4Addr::new(8, 8, 8, 8), Ipv4Addr::new(127, 0, 0, 1)];
        let addr = "192.168.1.20:21200".parse().unwrap();
        note_server_beacon(addr, [4u8; 32], BeaconRole::Standby);
        let cached = cached_servers();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].0, addr);
        assert_eq!(cached[0].1.role, BeaconRole::Standby);
        note_server_beacon(addr, [4u8; 32], BeaconRole::Primary);
        assert_eq!(cached_servers().len(), 1);
        assert_eq!(cached_servers()[0].1.role, BeaconRole::Primary);

        assert!(announce_now(false, true, &lan));
        assert!(!announce_now(true, true, &lan));
        assert!(!announce_now(false, false, &lan));
        assert!(!announce_now(false, true, &public));
        assert!(!announce_now(false, true, &[]));
    }

    #[test]
    fn a_quiet_discovery_socket_returns_at_the_deadline() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let started = std::time::Instant::now();
        let found = collect_beacons(socket, std::time::Duration::from_millis(200));
        let elapsed = started.elapsed();
        assert!(found.is_empty());
        assert!(elapsed >= std::time::Duration::from_millis(150), "returned too early: {elapsed:?}");
        assert!(elapsed < std::time::Duration::from_secs(2), "listen stayed open for {elapsed:?}");
    }
}
