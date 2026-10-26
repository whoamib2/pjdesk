//! Advertised addresses for both servers, and the order a client dials them.
//!
//! A saved hostname is one entry among the rest, and it is last. An advertised
//! IP is never skipped in favor of DNS or a hosts file.

use std::net::Ipv4Addr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a public IP or a hostname dial may take before the next address.
pub const PUBLIC_DIAL: Duration = Duration::from_secs(3);
/// A LAN address is on the local network. Give up sooner than a public dial.
pub const LAN_DIAL: Duration = Duration::from_millis(1500);

/// Public and LAN addresses of both servers, plus hostnames.
///
/// Public and LAN lists are IP addresses. Hostnames stay in `names` so DNS
/// cannot run ahead of an advertised IP.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRoutes {
    #[serde(default)]
    pub primary_lan: Vec<String>,
    #[serde(default)]
    pub primary_public: Vec<String>,
    #[serde(default)]
    pub standby_lan: Vec<String>,
    #[serde(default)]
    pub standby_public: Vec<String>,
    #[serde(default)]
    pub names: Vec<String>,
}

impl ServerRoutes {
    pub fn is_empty(&self) -> bool {
        self.primary_lan.is_empty()
            && self.primary_public.is_empty()
            && self.standby_lan.is_empty()
            && self.standby_public.is_empty()
            && self.names.is_empty()
    }

    /// Builds a catalog from the account fields older servers already publish.
    /// A hostname goes into `names`. An IP goes into the public list.
    pub fn from_legacy(
        primary: &str,
        primary_lan: &[String],
        primary_public_ip: &str,
        standby: &str,
        standby_lan: &[String],
        standby_public_ip: &str,
    ) -> Self {
        let mut routes = Self::default();
        routes.primary_lan = only_ips(primary_lan);
        routes.standby_lan = only_ips(standby_lan);
        push_public(&mut routes.primary_public, &mut routes.names, primary_public_ip);
        push_public(&mut routes.standby_public, &mut routes.names, standby_public_ip);
        push_endpoint(&mut routes, true, primary);
        push_endpoint(&mut routes, false, standby);
        routes
    }
}

/// What one server publishes about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Advertised {
    pub public: Vec<String>,
    pub lan: Vec<String>,
    pub names: Vec<String>,
}

/// Splits a published address, the observed public IP, and LAN endpoints.
/// The hostname is kept. It is not the only address.
pub fn advertise(published: &str, public_ip: &str, lan: &[String], port: u16) -> Advertised {
    let mut public = Vec::new();
    let mut names = Vec::new();
    push_public(&mut public, &mut names, &with_port(public_ip, port));
    classify_into(&mut public, &mut names, &with_port(published, port));
    Advertised { public, lan: only_ips(lan), names }
}

/// Dial order.
///
/// 1. The primary's LAN IP, when this computer is on that subnet.
/// 2. The primary's public IP.
/// 3. The standby's LAN IP, when this computer is on that subnet, then its public IP.
/// 4. Hostnames, only after every advertised IP.
///
/// A name that was stored in a public list is still dialed last. A LAN IP on
/// another subnet is left out, so it does not delay the public IP.
pub fn dial_order(routes: &ServerRoutes, mine: &[Ipv4Addr]) -> Vec<String> {
    let mut primary_lan = Vec::new();
    let mut primary_public = Vec::new();
    let mut standby_lan = Vec::new();
    let mut standby_public = Vec::new();
    let mut names = Vec::new();
    for raw in &routes.primary_lan {
        sort_endpoint(&mut primary_lan, &mut names, raw);
    }
    for raw in &routes.primary_public {
        sort_endpoint(&mut primary_public, &mut names, raw);
    }
    for raw in &routes.standby_lan {
        sort_endpoint(&mut standby_lan, &mut names, raw);
    }
    for raw in &routes.standby_public {
        sort_endpoint(&mut standby_public, &mut names, raw);
    }
    for raw in &routes.names {
        let text = canonical(raw);
        if text.is_empty() {
            continue;
        }
        if is_ip_endpoint(&text) {
            push_unique(&mut standby_public, text);
        } else {
            push_unique(&mut names, text);
        }
    }
    let mut out = Vec::new();
    for addr in primary_lan {
        if on_subnet(&addr, mine) {
            push_unique(&mut out, addr);
        }
    }
    for addr in primary_public {
        push_unique(&mut out, addr);
    }
    for addr in standby_lan {
        if on_subnet(&addr, mine) {
            push_unique(&mut out, addr);
        }
    }
    for addr in standby_public {
        push_unique(&mut out, addr);
    }
    for addr in names {
        push_unique(&mut out, addr);
    }
    out
}

/// The fresh advertised order, then a saved hostname that is not already listed.
/// A saved IP that the servers did not advertise is not inserted ahead of the
/// fresh set, and it is not used while any advertised address exists.
pub fn dial_targets(routes: &ServerRoutes, mine: &[Ipv4Addr], saved: &str) -> Vec<String> {
    let mut order = dial_order(routes, mine);
    let saved = saved.trim();
    if saved.is_empty() {
        return order;
    }
    if order.is_empty() {
        push_unique(&mut order, canonical(saved));
        return order;
    }
    if !is_ip_endpoint(saved) && !order.iter().any(|have| same_endpoint(have, saved)) {
        push_unique(&mut order, canonical(saved));
    }
    order
}

/// LAN dials are short. Public IPs and hostnames get a few seconds.
pub fn dial_budget(target: &str) -> Duration {
    match endpoint_v4(target) {
        Some(ip) if ip.is_private() => LAN_DIAL,
        _ => PUBLIC_DIAL,
    }
}

/// True when `addr` is on the same /24 as one of this computer's LAN addresses.
pub fn on_subnet(addr: &str, mine: &[Ipv4Addr]) -> bool {
    let Some(ip) = endpoint_v4(addr) else { return false };
    mine.iter().any(|own| same_slash24(ip, *own))
}

pub fn same_slash24(left: Ipv4Addr, right: Ipv4Addr) -> bool {
    left.octets()[..3] == right.octets()[..3]
}

pub fn is_ip_endpoint(text: &str) -> bool {
    endpoint_v4(text).is_some()
}

/// `host:port` for an IPv4 address. Empty when `text` is a name or blank.
pub fn endpoint_v4(text: &str) -> Option<Ipv4Addr> {
    let host = host_of(text)?;
    host.parse().ok()
}

pub fn same_endpoint(left: &str, right: &str) -> bool {
    canonical(left).eq_ignore_ascii_case(&canonical(right))
}

pub fn canonical(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let (host, port) = split_host_port(text);
    if host.is_empty() {
        return String::new();
    }
    format!("{}:{}", host, port)
}

/// Shown in the server window. Empty lists stay visible so a missing address
/// is obvious.
pub fn routes_text(routes: &ServerRoutes) -> String {
    format!(
        "Primary public: {}\nPrimary LAN: {}\nStandby public: {}\nStandby LAN: {}\nNames: {}",
        show_list(&routes.primary_public),
        show_list(&routes.primary_lan),
        show_list(&routes.standby_public),
        show_list(&routes.standby_lan),
        show_list(&routes.names),
    )
}

fn show_list(list: &[String]) -> String {
    if list.is_empty() {
        "—".into()
    } else {
        list.join(", ")
    }
}

/// Warns when a published name or address resolves to something that is not
/// this server. `resolved` empty means the lookup did not answer, which is
/// not the same as a stale answer.
pub fn address_warning(
    published: &str,
    resolved: &[std::net::IpAddr],
    lan: &[Ipv4Addr],
    public_ip: Option<Ipv4Addr>,
) -> String {
    let published = published.trim();
    if published.is_empty() || resolved.is_empty() {
        return String::new();
    }
    let stale: Vec<String> = resolved
        .iter()
        .filter(|ip| !reaches_this_server(ip, lan, public_ip))
        .map(|ip| ip.to_string())
        .collect();
    if stale.is_empty() {
        return String::new();
    }
    let name = host_of(published).unwrap_or(published);
    format!(
        "{name} resolves to {}, which does not reach this server. Computers that use only that name can miss this server. Advertised IP addresses are still published.",
        stale.join(", ")
    )
}

fn reaches_this_server(ip: &std::net::IpAddr, lan: &[Ipv4Addr], public_ip: Option<Ipv4Addr>) -> bool {
    if public_ip.is_some_and(|own| std::net::IpAddr::V4(own) == *ip) {
        return true;
    }
    match ip {
        std::net::IpAddr::V4(v4) => lan.contains(v4),
        std::net::IpAddr::V6(_) => false,
    }
}

fn push_endpoint(routes: &mut ServerRoutes, primary: bool, raw: &str) {
    let text = canonical(raw);
    if text.is_empty() {
        return;
    }
    if is_ip_endpoint(&text) {
        let list = if primary { &mut routes.primary_public } else { &mut routes.standby_public };
        push_unique(list, text);
    } else {
        push_unique(&mut routes.names, text);
    }
}

fn push_public(ips: &mut Vec<String>, names: &mut Vec<String>, raw: &str) {
    classify_into(ips, names, raw);
}

fn classify_into(ips: &mut Vec<String>, names: &mut Vec<String>, raw: &str) {
    let text = canonical(raw);
    if text.is_empty() {
        return;
    }
    if is_ip_endpoint(&text) {
        push_unique(ips, text);
    } else {
        push_unique(names, text);
    }
}

fn sort_endpoint(ips: &mut Vec<String>, names: &mut Vec<String>, raw: &str) {
    classify_into(ips, names, raw);
}

fn only_ips(list: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in list {
        if is_ip_endpoint(raw) {
            push_unique(&mut out, canonical(raw));
        }
    }
    out
}

fn with_port(raw: &str, port: u16) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Some((host, suffix)) = raw.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') && suffix.chars().all(|c| c.is_ascii_digit()) {
            return canonical(raw);
        }
    }
    let port = if port == 0 { crate::DEFAULT_PORT } else { port };
    format!("{raw}:{port}")
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if value.is_empty() || list.iter().any(|have| same_endpoint(have, &value)) {
        return;
    }
    list.push(value);
}

fn host_of(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(split_host_port(text).0)
}

fn split_host_port(text: &str) -> (&str, u16) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            let port = tail.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(crate::DEFAULT_PORT);
            return (host, port);
        }
    }
    if let Some((host, port)) = text.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(port) = port.parse() {
                return (host, port);
            }
        }
    }
    (text, crate::DEFAULT_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    fn sample() -> ServerRoutes {
        ServerRoutes {
            primary_lan: vec!["192.168.0.20:21200".into(), "pjdesk.spader.us:21200".into()],
            primary_public: vec!["204.248.122.44".into(), "pjdesk.spader.us".into()],
            standby_lan: vec!["192.168.0.50:21200".into(), "10.1.1.8:21200".into()],
            standby_public: vec!["198.51.100.8:21200".into()],
            names: vec!["pjdesk.spader.us:21200".into()],
        }
    }

    #[test]
    fn advertised_ips_come_before_any_hostname() {
        let mine = [v4(192, 168, 0, 10)];
        let order = dial_order(&sample(), &mine);
        assert_eq!(
            order,
            vec![
                "192.168.0.20:21200".to_string(),
                "204.248.122.44:21200".to_string(),
                "192.168.0.50:21200".to_string(),
                "198.51.100.8:21200".to_string(),
                "pjdesk.spader.us:21200".to_string(),
            ]
        );
    }

    #[test]
    fn a_different_subnet_skips_lan_and_still_dials_public_ips_before_dns() {
        let mine = [v4(10, 9, 9, 9)];
        let order = dial_targets(&sample(), &mine, "pjdesk.spader.us:21200");
        assert_eq!(
            order,
            vec![
                "204.248.122.44:21200".to_string(),
                "198.51.100.8:21200".to_string(),
                "pjdesk.spader.us:21200".to_string(),
            ]
        );
    }

    #[test]
    fn a_saved_ip_cannot_jump_ahead_of_the_fresh_set() {
        let mine = [v4(192, 168, 0, 10)];
        let order = dial_targets(&sample(), &mine, "192.168.0.231:21200");
        assert_eq!(order[0], "192.168.0.20:21200");
        assert!(!order.iter().any(|addr| addr.starts_with("192.168.0.231")));
        assert_eq!(order.last().map(String::as_str), Some("pjdesk.spader.us:21200"));
    }

    #[test]
    fn an_empty_catalog_still_dials_the_saved_address() {
        let order = dial_targets(&ServerRoutes::default(), &[], "office.example:21100");
        assert_eq!(order, vec!["office.example:21100".to_string()]);
    }

    #[test]
    fn legacy_fields_keep_the_hostname_behind_the_public_ip() {
        let routes = ServerRoutes::from_legacy(
            "pjdesk.spader.us:21200",
            &["192.168.0.20:21200".into()],
            "204.248.122.44",
            "backup.example:21200",
            &["192.168.1.40:21200".into()],
            "198.51.100.8",
        );
        let order = dial_order(&routes, &[v4(192, 168, 0, 10)]);
        assert_eq!(order[0], "192.168.0.20:21200");
        assert_eq!(order[1], "204.248.122.44:21200");
        assert_eq!(order[2], "198.51.100.8:21200");
        assert_eq!(
            order,
            vec![
                "192.168.0.20:21200".to_string(),
                "204.248.122.44:21200".to_string(),
                "198.51.100.8:21200".to_string(),
                "pjdesk.spader.us:21200".to_string(),
                "backup.example:21200".to_string(),
            ]
        );
    }

    #[test]
    fn advertise_keeps_the_name_and_the_public_ip() {
        let got = advertise("pjdesk.spader.us:21200", "204.248.122.44", &["192.168.0.20:21200".into()], 21200);
        assert_eq!(got.public, vec!["204.248.122.44:21200".to_string()]);
        assert_eq!(got.lan, vec!["192.168.0.20:21200".to_string()]);
        assert_eq!(got.names, vec!["pjdesk.spader.us:21200".to_string()]);
    }

    #[test]
    fn a_stale_dns_answer_is_a_warning_and_our_own_address_is_not() {
        let lan = [v4(192, 168, 0, 20)];
        let public = Some(v4(204, 248, 122, 44));
        let stale = address_warning(
            "pjdesk.spader.us:21200",
            &[IpAddr::V4(v4(192, 168, 0, 231))],
            &lan,
            public,
        );
        assert!(stale.contains("192.168.0.231"));
        assert!(stale.contains("pjdesk.spader.us"));
        let ours = address_warning(
            "pjdesk.spader.us:21200",
            &[IpAddr::V4(v4(204, 248, 122, 44))],
            &lan,
            public,
        );
        assert!(ours.is_empty());
        assert!(address_warning("pjdesk.spader.us:21200", &[], &lan, public).is_empty());
    }
}
