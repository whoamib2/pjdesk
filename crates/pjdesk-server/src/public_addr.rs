//! The address computers use to reach this server.
//!
//! A public guess can be published. A private LAN address is shown in the
//! Server window and is not sent until an administrator confirms it.

use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "public-address.json";
const PORT_FILE: &str = "listen-port.txt";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicAddress {
    pub address: String,
    /// An administrator accepted this address, including a local one.
    #[serde(default)]
    pub confirmed: bool,
    /// `legacy`, `public-ip`, `lan`, or `manual`.
    #[serde(default)]
    pub source: String,
    /// pjdesk.com already lists a different server for this slot.
    #[serde(default)]
    pub needs_confirm: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guess {
    pub address: String,
    pub source: &'static str,
    pub needs_confirmation: bool,
}

pub fn remember_port(data_dir: &Path, port: u16) {
    if port == 0 {
        return;
    }
    let _ = std::fs::write(data_dir.join(PORT_FILE), port.to_string());
}

pub fn remembered_port(data_dir: &Path) -> u16 {
    std::fs::read_to_string(data_dir.join(PORT_FILE))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .filter(|port| *port > 0)
        .unwrap_or(pjdesk_proto::DEFAULT_PORT)
}

pub fn load(data_dir: &Path) -> Option<PublicAddress> {
    let bytes = std::fs::read(data_dir.join(FILE_NAME)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save(data_dir: &Path, address: &PublicAddress) {
    if let Ok(bytes) = serde_json::to_vec_pretty(address) {
        let _ = std::fs::write(data_dir.join(FILE_NAME), bytes);
    }
}

/// Keeps an address the administrator already saved. Discovers one otherwise.
pub fn ensure(data_dir: &Path, port: u16) -> PublicAddress {
    if let Some(existing) = load(data_dir) {
        if !existing.address.trim().is_empty() {
            return existing;
        }
    }
    let guess = discover(port);
    let empty = guess.address.is_empty();
    let publishable = !guess.needs_confirmation && !empty;
    let note = if guess.needs_confirmation {
        "This is a local network address. Confirm it before it is published. It is not sent until you do."
    } else if empty {
        "Type the public address computers use to reach this server."
    } else if guess.source == "legacy" {
        "Filled in from the name computers already use to reach this server."
    } else {
        "Filled in from this server's public address."
    };
    let record = PublicAddress {
        address: guess.address,
        confirmed: publishable,
        source: guess.source.into(),
        needs_confirm: false,
        note: note.into(),
    };
    save(data_dir, &record);
    record
}

fn discover(port: u16) -> Guess {
    let host = legacy_host();
    let legacy_ips = resolve_host(host);
    let public_ip = fetch_public_ip();
    let local_ips = local_ips();
    choose_address(port, host, &legacy_ips, public_ip, &local_ips)
}

fn legacy_host() -> &'static str {
    pjdesk_proto::LEGACY_REGISTERED_SERVER.split(':').next().unwrap_or("pjdesk.spader.us")
}

fn resolve_host(host: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    (host, pjdesk_proto::DEFAULT_PORT)
        .to_socket_addrs()
        .map(|iter| iter.map(|addr| addr.ip()).collect())
        .unwrap_or_default()
}

fn fetch_public_ip() -> Option<IpAddr> {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(5)).build();
    let text = agent.get("https://cloudflare.com/cdn-cgi/trace").call().ok()?.into_string().ok()?;
    for line in text.lines() {
        if let Some(ip) = line.strip_prefix("ip=") {
            return ip.trim().parse().ok();
        }
    }
    None
}

fn local_ips() -> Vec<IpAddr> {
    let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else { return Vec::new() };
    if socket.connect("1.1.1.1:80").is_err() {
        return Vec::new();
    }
    socket.local_addr().ok().map(|addr| addr.ip()).into_iter().collect()
}

/// Picks the address to show. A name that resolves to this server's public
/// address wins. A public IP is next. A LAN address is kept for the window
/// and is not publishable until someone confirms it.
pub fn choose_address(port: u16, legacy_host: &str, legacy_ips: &[IpAddr], public_ip: Option<IpAddr>, local_ips: &[IpAddr]) -> Guess {
    let port = if port == 0 { pjdesk_proto::DEFAULT_PORT } else { port };
    let on_public = public_ip.is_some_and(|ip| !is_private_ip(ip) && legacy_ips.contains(&ip));
    let on_local_public = legacy_ips.iter().any(|ip| !is_private_ip(*ip) && local_ips.contains(ip));
    if !legacy_host.is_empty() && (on_public || on_local_public) {
        return Guess {
            address: format!("{legacy_host}:{port}"),
            source: "legacy",
            needs_confirmation: false,
        };
    }
    if let Some(ip) = public_ip.filter(|ip| !is_private_ip(*ip)) {
        return Guess {
            address: format_endpoint(ip, port),
            source: "public-ip",
            needs_confirmation: false,
        };
    }
    if let Some(ip) = local_ips.iter().copied().find(|ip| is_private_ip(*ip) && !ip.is_unspecified() && !ip.is_loopback()) {
        return Guess {
            address: format_endpoint(ip, port),
            source: "lan",
            needs_confirmation: true,
        };
    }
    Guess {
        address: String::new(),
        source: "none",
        needs_confirmation: true,
    }
}

fn format_endpoint(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

pub fn is_private_address(address: &str) -> bool {
    let Some(host) = host_of(address) else { return false };
    match host.parse::<IpAddr>() {
        Ok(ip) => is_private_ip(ip) || ip.is_unspecified(),
        Err(_) => false,
    }
}

fn host_of(address: &str) -> Option<&str> {
    let text = address.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix('[') {
        return rest.split(']').next().filter(|host| !host.is_empty());
    }
    if text.parse::<std::net::Ipv4Addr>().is_ok() || text.parse::<std::net::Ipv6Addr>().is_ok() {
        return Some(text);
    }
    if let Some((host, port)) = text.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) && !host.is_empty() {
            return Some(host);
        }
    }
    Some(text)
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || is_unique_local(v6) || is_v6_link_local(v6)
        }
    }
}

fn is_unique_local(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_v6_link_local(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn a_legacy_name_wins_when_it_points_at_this_server() {
        let guess = choose_address(21200, "pjdesk.spader.us", &[v4(203, 0, 113, 10)], Some(v4(203, 0, 113, 10)), &[v4(192, 168, 1, 20)]);
        assert_eq!(guess.address, "pjdesk.spader.us:21200");
        assert_eq!(guess.source, "legacy");
        assert!(!guess.needs_confirmation);
        assert!(!is_private_address(&guess.address));
    }

    #[test]
    fn a_public_ip_is_used_when_the_legacy_name_points_elsewhere() {
        let guess = choose_address(21200, "pjdesk.spader.us", &[v4(203, 0, 113, 10)], Some(v4(198, 51, 100, 8)), &[v4(192, 168, 1, 20)]);
        assert_eq!(guess.address, "198.51.100.8:21200");
        assert_eq!(guess.source, "public-ip");
        assert!(!guess.needs_confirmation);
    }

    #[test]
    fn a_lan_address_needs_confirmation_and_is_private() {
        let guess = choose_address(21200, "pjdesk.spader.us", &[v4(203, 0, 113, 10)], None, &[v4(192, 168, 1, 20)]);
        assert_eq!(guess.address, "192.168.1.20:21200");
        assert_eq!(guess.source, "lan");
        assert!(guess.needs_confirmation);
        assert!(is_private_address(&guess.address));
        assert!(is_private_address("10.1.2.3:21200"));
        assert!(is_private_address("100.64.0.1:21200"));
        assert!(!is_private_address("pjdesk.spader.us:21200"));
        assert!(!is_private_address("203.0.113.10:21200"));
    }
}
