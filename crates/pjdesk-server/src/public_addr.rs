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
    if let Some(mut existing) = load(data_dir) {
        if !existing.address.trim().is_empty() {
            if is_hostname(&existing.address) {
                return existing;
            }
            let port = if port == 0 { remembered_port(data_dir) } else { port };
            let host = legacy_host();
            let restored = prefer_hostname(&existing.address, host, port, &resolve_host(host));
            if restored != existing.address.trim() {
                existing.address = restored;
                existing.source = "legacy".into();
                existing.note =
                    "Kept the hostname computers already use. A public IP change does not replace it.".into();
                save(data_dir, &existing);
            }
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
    fetch_public_ip_from(&crate::cloud::default_api())
}

/// Asks pjdesk.com which address it sees. The Worker route echoes `CF-Connecting-IP`.
pub fn fetch_public_ip_from(api: &str) -> Option<IpAddr> {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(5)).build();
    let url = format!("{}/api/ip", api.trim_end_matches('/'));
    let text = agent.get(&url).call().ok()?.into_string().ok()?;
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        ip: String,
    }
    let ip = serde_json::from_str::<Reply>(&text).ok()?.ip;
    let ip = ip.trim();
    if ip.is_empty() {
        return None;
    }
    ip.parse().ok()
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
pub fn choose_address(
    port: u16,
    legacy_host: &str,
    legacy_ips: &[IpAddr],
    public_ip: Option<IpAddr>,
    local_ips: &[IpAddr],
) -> Guess {
    let port = if port == 0 { pjdesk_proto::DEFAULT_PORT } else { port };
    let on_public = public_ip.is_some_and(|ip| !is_private_ip(ip) && legacy_ips.contains(&ip));
    let on_local_public = legacy_ips.iter().any(|ip| !is_private_ip(*ip) && local_ips.contains(ip));
    if !legacy_host.is_empty() && (on_public || on_local_public) {
        return Guess { address: format!("{legacy_host}:{port}"), source: "legacy", needs_confirmation: false };
    }
    if let Some(ip) = public_ip.filter(|ip| !is_private_ip(*ip)) {
        return Guess { address: format_endpoint(ip, port), source: "public-ip", needs_confirmation: false };
    }
    if let Some(ip) =
        local_ips.iter().copied().find(|ip| is_private_ip(*ip) && !ip.is_unspecified() && !ip.is_loopback())
    {
        return Guess { address: format_endpoint(ip, port), source: "lan", needs_confirmation: true };
    }
    Guess { address: String::new(), source: "none", needs_confirmation: true }
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

pub fn host_of(address: &str) -> Option<&str> {
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

/// True when the address is a name, not a raw IP.
pub fn is_hostname(address: &str) -> bool {
    match host_of(address) {
        Some(host) if !host.is_empty() => host.parse::<IpAddr>().is_err(),
        _ => false,
    }
}

/// Keeps a hostname. A saved raw IP is rewritten to `legacy_host` only when that
/// name resolves to the saved IP, which is how pjdesk.spader.us was replaced.
pub fn prefer_hostname(current: &str, legacy_host: &str, port: u16, legacy_ips: &[IpAddr]) -> String {
    let current = current.trim();
    if current.is_empty() {
        return String::new();
    }
    if is_hostname(current) {
        return current.to_string();
    }
    let Some(host) = host_of(current) else { return current.to_string() };
    let Ok(ip) = host.parse::<IpAddr>() else { return current.to_string() };
    if !legacy_host.is_empty() && legacy_ips.contains(&ip) {
        let port = if port == 0 { pjdesk_proto::DEFAULT_PORT } else { port };
        return format!("{legacy_host}:{port}");
    }
    current.to_string()
}

/// The toggle defaults on for an IP or an empty field, and off for a hostname.
pub fn auto_update_default(address: &str) -> bool {
    !is_hostname(address)
}

/// A hostname is never replaced by a raw IP. An IP is replaced only when the toggle is on.
pub fn address_after_ip_change(current: &str, new_ip: &str, port: u16, auto: bool) -> String {
    let current = current.trim();
    if is_hostname(current) || !auto || new_ip.trim().is_empty() {
        return current.to_string();
    }
    let port = if port == 0 { pjdesk_proto::DEFAULT_PORT } else { port };
    if let Ok(ip) = new_ip.trim().parse::<IpAddr>() {
        return format_endpoint(ip, port);
    }
    current.to_string()
}

/// Two matching reads are required before a new public IP is published.
/// The first time an address is seen it is recorded and is not treated as a change.
pub fn note_ip_reading(observed: &str, pending: &str, hits: u8, read: &str) -> (String, String, u8, bool) {
    let read = read.trim();
    if read.is_empty() {
        return (observed.to_string(), pending.to_string(), hits, false);
    }
    if observed.is_empty() {
        if pending == read {
            let hits = hits.saturating_add(1);
            if hits >= 2 {
                return (read.to_string(), String::new(), 0, false);
            }
            return (String::new(), read.to_string(), hits, false);
        }
        return (String::new(), read.to_string(), 1, false);
    }
    if read == observed {
        return (observed.to_string(), String::new(), 0, false);
    }
    if pending == read {
        let hits = hits.saturating_add(1);
        if hits >= 2 {
            return (read.to_string(), String::new(), 0, true);
        }
        return (observed.to_string(), read.to_string(), hits, false);
    }
    (observed.to_string(), read.to_string(), 1, false)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpWatch {
    #[serde(default)]
    pub observed: String,
    #[serde(default)]
    pub pending: String,
    #[serde(default)]
    pub hits: u8,
    #[serde(default)]
    pub checked_at: u64,
    /// `None` follows [`auto_update_default`].
    #[serde(default)]
    pub auto: Option<bool>,
}

const WATCH_FILE: &str = "ip-watch.json";
pub const IP_CHECK_EVERY: u64 = 300;

impl IpWatch {
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join(WATCH_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(dir.join(WATCH_FILE), bytes);
        }
    }

    pub fn enabled(&self, address: &str) -> bool {
        self.auto.unwrap_or_else(|| auto_update_default(address))
    }

    pub fn line(&self) -> String {
        if self.observed.is_empty() {
            return "Public IP: not checked yet".into();
        }
        if self.checked_at == 0 {
            return format!("Public IP: {}", self.observed);
        }
        format!("Public IP: {} (last checked {})", self.observed, crate::registry::format_local(self.checked_at))
    }
}

/// Broadcasts this server's signaling port, key, and role on UDP 21202 until
/// the process ends. The role is read again every announcement, so a promote
/// or demote shows up without a restart. Clients ignore a standby.
pub fn announce_lan(port: u16, fingerprint: [u8; 32], data_dir: std::path::PathBuf) {
    let port = if port == 0 { pjdesk_proto::DEFAULT_PORT } else { port };
    let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else { return };
    let _ = socket.set_broadcast(true);
    let dest = std::net::SocketAddr::from(([255, 255, 255, 255], pjdesk_proto::DISCOVERY_PORT));
    loop {
        let role = if crate::cloud::accepts_host_registrations(&data_dir) {
            pjdesk_proto::discover::BeaconRole::Primary
        } else {
            pjdesk_proto::discover::BeaconRole::Standby
        };
        let packet =
            pjdesk_proto::discover::encode_server(&pjdesk_proto::discover::ServerBeacon { port, fingerprint, role });
        let _ = socket.send_to(&packet, dest);
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

/// Private IPv4 addresses of the interfaces this host would use.
pub fn lan_endpoints(port: u16) -> Vec<String> {
    let port = if port == 0 { pjdesk_proto::DEFAULT_PORT } else { port };
    let mut ips = Vec::new();
    for dest in ["1.1.1.1:80", "10.255.255.254:9", "172.31.255.254:9", "192.168.255.254:9"] {
        let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else { continue };
        let Ok(target) = dest.parse::<std::net::SocketAddr>() else { continue };
        if socket.connect(target).is_err() {
            continue;
        }
        if let Some(IpAddr::V4(ip)) = socket.local_addr().ok().map(|addr| addr.ip()) {
            if is_private_ip(IpAddr::V4(ip)) && !ip.is_loopback() && !ip.is_unspecified() {
                ips.push(ip);
            }
        }
    }
    ips.sort();
    ips.dedup();
    ips.into_iter().map(|ip| format!("{ip}:{port}")).collect()
}

/// Checks the public IP at most every five minutes. Returns true when a
/// confirmed change should be published.
pub fn poll_public_ip(data_dir: &Path, now: u64) -> bool {
    let mut watch = IpWatch::load(data_dir);
    if watch.checked_at != 0 && now.saturating_sub(watch.checked_at) < IP_CHECK_EVERY {
        return false;
    }
    let Some(ip) = fetch_public_ip() else { return false };
    let (observed, pending, hits, changed) =
        note_ip_reading(&watch.observed, &watch.pending, watch.hits, &ip.to_string());
    let previous = watch.observed.clone();
    watch.observed = observed;
    watch.pending = pending;
    watch.hits = hits;
    watch.checked_at = now;
    watch.save(data_dir);
    if changed {
        tracing::info!("public IP changed from {previous} to {}", watch.observed);
        let port = remembered_port(data_dir);
        if let Some(mut record) = load(data_dir) {
            let next = address_after_ip_change(&record.address, &watch.observed, port, watch.enabled(&record.address));
            if next != record.address.trim() && !next.is_empty() {
                record.address = next;
                record.source = "public-ip".into();
                record.confirmed = true;
                record.note = format!("Public IP changed to {}. The network slot was updated.", watch.observed);
                save(data_dir, &record);
            }
        }
    }
    changed || watch.observed == ip.to_string() && previous != watch.observed
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
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || is_unique_local(v6) || is_v6_link_local(v6),
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
        let guess = choose_address(
            21200,
            "pjdesk.spader.us",
            &[v4(203, 0, 113, 10)],
            Some(v4(203, 0, 113, 10)),
            &[v4(192, 168, 1, 20)],
        );
        assert_eq!(guess.address, "pjdesk.spader.us:21200");
        assert_eq!(guess.source, "legacy");
        assert!(!guess.needs_confirmation);
        assert!(!is_private_address(&guess.address));
    }

    #[test]
    fn a_public_ip_is_used_when_the_legacy_name_points_elsewhere() {
        let guess = choose_address(
            21200,
            "pjdesk.spader.us",
            &[v4(203, 0, 113, 10)],
            Some(v4(198, 51, 100, 8)),
            &[v4(192, 168, 1, 20)],
        );
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

    #[test]
    fn a_hostname_is_never_replaced_by_a_raw_ip() {
        assert!(is_hostname("pjdesk.spader.us:21200"));
        assert!(!is_hostname("204.248.122.44:21200"));
        assert!(!auto_update_default("pjdesk.spader.us:21200"));
        assert!(auto_update_default("204.248.122.44:21200"));
        assert!(auto_update_default(""));
        let kept = prefer_hostname("pjdesk.spader.us:21200", "other.example", 21200, &[v4(204, 248, 122, 44)]);
        assert_eq!(kept, "pjdesk.spader.us:21200");
        let restored = prefer_hostname("204.248.122.44:21200", "pjdesk.spader.us", 21200, &[v4(204, 248, 122, 44)]);
        assert_eq!(restored, "pjdesk.spader.us:21200");
        let foreign = prefer_hostname("198.51.100.8:21200", "pjdesk.spader.us", 21200, &[v4(204, 248, 122, 44)]);
        assert_eq!(foreign, "198.51.100.8:21200");
        assert_eq!(
            address_after_ip_change("pjdesk.spader.us:21200", "203.0.113.9", 21200, true),
            "pjdesk.spader.us:21200"
        );
        assert_eq!(address_after_ip_change("204.248.122.44:21200", "203.0.113.9", 21200, true), "203.0.113.9:21200");
        assert_eq!(
            address_after_ip_change("204.248.122.44:21200", "203.0.113.9", 21200, false),
            "204.248.122.44:21200"
        );
    }

    #[test]
    fn a_public_ip_change_waits_for_two_matching_reads() {
        let (obs, pend, hits, changed) = note_ip_reading("", "", 0, "204.248.122.44");
        assert!(!changed);
        assert_eq!(pend, "204.248.122.44");
        let (obs, pend, hits, changed) = note_ip_reading(&obs, &pend, hits, "204.248.122.44");
        assert!(!changed);
        assert_eq!(obs, "204.248.122.44");
        assert!(pend.is_empty());
        let (obs, pend, hits, changed) = note_ip_reading(&obs, &pend, hits, "203.0.113.9");
        assert!(!changed);
        assert_eq!(hits, 1);
        let (obs, _, _, changed) = note_ip_reading(&obs, &pend, hits, "203.0.113.9");
        assert!(changed);
        assert_eq!(obs, "203.0.113.9");
        let (_, _, _, changed) = note_ip_reading(&obs, "", 0, "198.51.100.1");
        assert!(!changed);
    }
}
