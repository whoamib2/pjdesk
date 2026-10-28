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
    /// Address that answered most recently. The next dial tries it first.
    #[serde(default)]
    pub preferred: String,
    /// Consecutive dial timeouts. Two demotes an address. Four drops it.
    #[serde(default)]
    pub misses: Vec<RouteMiss>,
    /// Version the answering server reported. Empty when it did not say.
    #[serde(default)]
    pub server_version: String,
}

/// How many times one address failed to answer, in a row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteMiss {
    pub addr: String,
    #[serde(default)]
    pub count: u32,
}

impl ServerRoutes {
    pub fn is_empty(&self) -> bool {
        self.primary_lan.is_empty()
            && self.primary_public.is_empty()
            && self.standby_lan.is_empty()
            && self.standby_public.is_empty()
            && self.names.is_empty()
    }

    /// True when a server published an IP. A hostname alone is not enough:
    /// dialing must try those IPs before DNS.
    pub fn has_advertised_ip(&self) -> bool {
        self.primary_lan.iter().chain(self.primary_public.iter()).chain(self.standby_lan.iter()).chain(self.standby_public.iter()).any(
            |addr| is_ip_endpoint(addr),
        )
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
    if !saved.is_empty() {
        if order.is_empty() {
            push_unique(&mut order, canonical(saved));
        } else if !is_ip_endpoint(saved) && !order.iter().any(|have| same_endpoint(have, saved)) {
            push_unique(&mut order, canonical(saved));
        }
    }
    if order.is_empty() {
        return order;
    }
    let order = apply_route_health(order, routes);
    let order = prefer_public_off_lan(order, routes, mine);
    promote_same_lan(order, routes, mine)
}

/// Account sign-in targets for one saved address.
///
/// `prefer` may be the public active address. A LAN address on `mine`'s /24
/// is still first. The result is never only that public address when such a
/// LAN address is known.
pub fn account_dial_order(prefer: &str, routes: &ServerRoutes, mine: &[Ipv4Addr]) -> Vec<String> {
    let mut order = dial_targets(routes, mine, prefer);
    let prefer = canonical(prefer);
    if order.is_empty() && !prefer.is_empty() {
        order.push(prefer.clone());
    }
    let mut lan = Vec::new();
    for raw in routes.primary_lan.iter().chain(routes.standby_lan.iter()) {
        let addr = canonical(raw);
        if on_subnet(&addr, mine) {
            push_unique(&mut lan, addr);
        }
    }
    if lan.is_empty() {
        return order;
    }
    let mut out = lan;
    for addr in order {
        push_unique(&mut out, addr);
    }
    out
}

/// Standby addresses for a sign-in the primary could not check.
/// A same-subnet LAN address is first. The primary's public address is not included.
pub fn standby_account_order(routes: &ServerRoutes, mine: &[Ipv4Addr], saved_standby: &str) -> Vec<String> {
    let mut lan = Vec::new();
    let mut rest = Vec::new();
    for raw in &routes.standby_lan {
        let addr = canonical(raw);
        if addr.is_empty() {
            continue;
        }
        if on_subnet(&addr, mine) {
            push_unique(&mut lan, addr);
        } else {
            push_unique(&mut rest, addr);
        }
    }
    let saved = canonical(saved_standby);
    if !saved.is_empty() {
        if on_subnet(&saved, mine) {
            push_unique(&mut lan, saved);
        } else {
            push_unique(&mut rest, saved);
        }
    }
    for raw in &routes.standby_public {
        let addr = canonical(raw);
        if !addr.is_empty() {
            push_unique(&mut rest, addr);
        }
    }
    let mut out = lan;
    out.extend(rest);
    out
}

/// Every known server, for a sign-in the primary could not check.
///
/// `extras` are addresses from discovery and the live heartbeat. A LAN address
/// on `mine`'s /24 comes first, except `answered`, which already failed and
/// is tried after the other servers. The stored standby is not the only target.
pub fn sign_in_peer_order(routes: &ServerRoutes, mine: &[Ipv4Addr], extras: &[String], answered: &str) -> Vec<String> {
    let mut all = Vec::new();
    for raw in routes
        .primary_lan
        .iter()
        .chain(routes.standby_lan.iter())
        .chain(routes.primary_public.iter())
        .chain(routes.standby_public.iter())
        .chain(routes.names.iter())
        .chain(extras.iter())
    {
        let addr = canonical(raw);
        if !addr.is_empty() {
            push_unique(&mut all, addr);
        }
    }
    let answered = canonical(answered);
    let mut same = Vec::new();
    let mut same_answered = Vec::new();
    let mut private = Vec::new();
    let mut public = Vec::new();
    for addr in all {
        let on = on_subnet(&addr, mine);
        let already = !answered.is_empty() && same_endpoint(&addr, &answered);
        if on && !already {
            same.push(addr);
        } else if on && already {
            same_answered.push(addr);
        } else if endpoint_v4(&addr).is_some_and(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local()) {
            private.push(addr);
        } else {
            public.push(addr);
        }
    }
    let mut out = same;
    out.extend(private);
    out.extend(same_answered);
    out.extend(public);
    out
}

/// The banner, then one `address: error` line for every dial that failed.
pub fn registration_miss(attempts: &[(&str, &str)]) -> String {
    let last = attempts
        .last()
        .map(|(_, err)| err.trim())
        .filter(|err| !err.is_empty())
        .unwrap_or("no address answered");
    let mut text = format!("could not reach the primary or the standby; retrying. {last}");
    for (addr, err) in attempts {
        let addr = addr.trim();
        if addr.is_empty() {
            continue;
        }
        let err = err.trim();
        let err = if err.is_empty() { "failed" } else { err };
        text.push('\n');
        text.push_str(addr);
        text.push_str(": ");
        text.push_str(err);
    }
    text
}

/// The per-address lines under the registration banner. The first line is the banner.
pub fn registration_detail_lines(text: &str) -> Vec<String> {
    text.lines().skip(1).map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

/// LAN addresses of a peer that share a /24 with this computer, on the
/// direct-connect port. Other subnets and hostnames are left out so a relay
/// is not required for two computers on the same network.
pub fn same_subnet_direct(addrs: &[String], mine: &[Ipv4Addr]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in addrs {
        if !on_subnet(raw, mine) {
            continue;
        }
        let Some(ip) = endpoint_v4(raw) else { continue };
        if ip.is_unspecified() || ip.is_loopback() || ip.is_broadcast() || ip.is_multicast() {
            continue;
        }
        push_unique(&mut out, format!("{ip}:{}", crate::DIRECT_PORT));
    }
    out
}

/// The address a server named, then the advertised order.
///
/// A follow address that does not answer is not the only attempt. The primary
/// LAN (same /24), the primary public IP, the other server, and DNS still run.
pub fn registration_targets(follow: &str, routes: &ServerRoutes, mine: &[Ipv4Addr], saved: &str) -> Vec<String> {
    let mut order = dial_targets(routes, mine, saved);
    let follow = canonical(follow);
    if follow.is_empty() {
        return order;
    }
    order.retain(|addr| !same_endpoint(addr, &follow));
    order.insert(0, follow);
    promote_same_lan(order, routes, mine)
}

/// Uses the older LAN and public fields when `routes` has no advertised IP.
/// Those addresses are dialed before the hostname. A catalog that already
/// has an IP is left as it is.
pub fn routes_with_legacy(
    routes: &ServerRoutes,
    primary: &str,
    primary_lan: &[String],
    primary_public_ip: &str,
    standby: &str,
    standby_lan: &[String],
    standby_public_ip: &str,
) -> ServerRoutes {
    if routes.has_advertised_ip() {
        return routes.clone();
    }
    let mut legacy = ServerRoutes::from_legacy(primary, primary_lan, primary_public_ip, standby, standby_lan, standby_public_ip);
    carry_health(&mut legacy, routes);
    if !legacy.has_advertised_ip() {
        return if routes.is_empty() { legacy } else { routes.clone() };
    }
    for name in &routes.names {
        let text = canonical(name);
        if text.is_empty() || legacy.names.iter().any(|have| same_endpoint(have, &text)) {
            continue;
        }
        legacy.names.push(text);
    }
    legacy
}

/// Keeps addresses the new catalog left out.
///
/// A primary that only knows itself would otherwise erase the other server.
/// Those addresses stay, so a computer can register with the new primary after
/// the old one is gone.
pub fn keep_other_server(previous: &ServerRoutes, incoming: &ServerRoutes) -> ServerRoutes {
    if incoming.is_empty() {
        return previous.clone();
    }
    let mut next = incoming.clone();
    carry_health(&mut next, previous);
    let previous_addrs = listed_addrs(previous);
    let fill_primary = next.primary_lan.is_empty() && next.primary_public.is_empty();
    let fill_standby = next.standby_lan.is_empty() && next.standby_public.is_empty();
    if !fill_primary && !fill_standby {
        // A newer catalog can name one standby and leave out another server
        // on the same account. Keep every address already learned.
        for addr in previous_addrs {
            if listed(&next, &addr) {
                continue;
            }
            classify_kept(&addr, &mut next.standby_lan, &mut next.standby_public, &mut next.names);
        }
        return next;
    }
    for addr in previous_addrs {
        if listed(&next, &addr) {
            continue;
        }
        let (lan, public) = if fill_primary {
            (&mut next.primary_lan, &mut next.primary_public)
        } else {
            (&mut next.standby_lan, &mut next.standby_public)
        };
        classify_kept(&addr, lan, public, &mut next.names);
    }
    next
}

/// Replaces the cached catalog with what the answering server just advertised.
///
/// An address the advertisement leaves out is not kept once that half is
/// filled. A half the advertisement left empty still keeps a previous address,
/// unless that address has already timed out twice.
pub fn adopt_advertised(previous: &ServerRoutes, incoming: &ServerRoutes) -> ServerRoutes {
    if incoming.is_empty() {
        return previous.clone();
    }
    let mut next = incoming.clone();
    carry_health(&mut next, previous);
    let fill_primary = next.primary_lan.is_empty() && next.primary_public.is_empty();
    let fill_standby = next.standby_lan.is_empty() && next.standby_public.is_empty();
    if fill_primary || fill_standby {
        for addr in listed_addrs(previous) {
            if listed(&next, &addr) || miss_count(previous, &addr) >= 2 {
                continue;
            }
            let (lan, public) = if fill_primary {
                (&mut next.primary_lan, &mut next.primary_public)
            } else {
                (&mut next.standby_lan, &mut next.standby_public)
            };
            classify_kept(&addr, lan, public, &mut next.names);
        }
    }
    let preferred = next.preferred.clone();
    let kept: Vec<RouteMiss> = next
        .misses
        .iter()
        .filter(|row| listed(&next, &row.addr) || same_endpoint(&row.addr, &preferred))
        .cloned()
        .collect();
    next.misses = kept;
    next
}

/// The address that answered moves to the front. Two timeouts in a row move
/// an address to the end. Four drop it, unless that would leave nothing to dial.
pub fn apply_route_health(order: Vec<String>, routes: &ServerRoutes) -> Vec<String> {
    if routes.preferred.trim().is_empty() && routes.misses.is_empty() {
        return order;
    }
    let mut kept = Vec::new();
    let mut demoted = Vec::new();
    let mut dropped = Vec::new();
    for addr in order {
        match miss_count(routes, &addr) {
            count if count >= 4 => dropped.push(addr),
            count if count >= 2 => push_unique(&mut demoted, addr),
            _ => push_unique(&mut kept, addr),
        }
    }
    if kept.is_empty() && demoted.is_empty() {
        kept = dropped;
    }
    kept.extend(demoted);
    let preferred = canonical(&routes.preferred);
    if !preferred.is_empty() && miss_count(routes, &preferred) == 0 {
        if let Some(pos) = kept.iter().position(|addr| same_endpoint(addr, &preferred)) {
            let item = kept.remove(pos);
            kept.insert(0, item);
        }
    }
    kept
}

/// Off the server's LAN, a public address is the path that can work.
///
/// A LAN address that answered at home is not put back in front. A public
/// address that has timed out four times is not put back either: that count
/// is the route-health drop, and a hairpin failure must not keep winning.
fn prefer_public_off_lan(order: Vec<String>, routes: &ServerRoutes, mine: &[Ipv4Addr]) -> Vec<String> {
    let on_lan = routes.primary_lan.iter().chain(routes.standby_lan.iter()).any(|addr| on_subnet(addr, mine));
    if on_lan {
        return order;
    }
    let mut public = Vec::new();
    for raw in routes.primary_public.iter().chain(routes.standby_public.iter()) {
        let addr = canonical(raw);
        if !is_ip_endpoint(&addr) {
            continue;
        }
        if endpoint_v4(&addr).is_some_and(|ip| ip.is_private()) {
            continue;
        }
        if miss_count(routes, &addr) >= 4 {
            continue;
        }
        push_unique(&mut public, addr);
    }
    if public.is_empty() {
        return order;
    }
    for addr in order {
        if public.iter().any(|have| same_endpoint(have, &addr)) {
            continue;
        }
        push_unique(&mut public, addr);
    }
    public
}

/// On the server's LAN, that LAN address stays ahead of a public address.
///
/// A saved public route, including one that answered on cellular, does not
/// jump the queue while this computer shares the server's /24. A LAN address
/// that answered recently is included when this computer's own address is
/// unknown, so a missed interface list does not hide the path that just worked.
fn promote_same_lan(order: Vec<String>, routes: &ServerRoutes, mine: &[Ipv4Addr]) -> Vec<String> {
    let mut lan = Vec::new();
    for raw in routes.primary_lan.iter().chain(routes.standby_lan.iter()) {
        let addr = canonical(raw);
        if on_subnet(&addr, mine) {
            push_unique(&mut lan, addr);
        }
    }
    let preferred = canonical(&routes.preferred);
    let listed = routes
        .primary_lan
        .iter()
        .chain(routes.standby_lan.iter())
        .any(|addr| same_endpoint(addr, &preferred));
    if listed && miss_count(routes, &preferred) < 4 && (mine.is_empty() || on_subnet(&preferred, mine)) {
        if let Some(pos) = lan.iter().position(|addr| same_endpoint(addr, &preferred)) {
            let item = lan.remove(pos);
            lan.insert(0, item);
        } else {
            lan.insert(0, preferred);
        }
    }
    if lan.is_empty() {
        return order;
    }
    let mut out = lan;
    for addr in order {
        push_unique(&mut out, addr);
    }
    out
}

/// True when the next dial should leave `current` for a same-subnet LAN address.
pub fn lan_should_replace(current: &str, routes: &ServerRoutes, mine: &[Ipv4Addr]) -> bool {
    let targets = dial_targets(routes, mine, "");
    match targets.first() {
        Some(first) if on_subnet(first, mine) && !same_endpoint(first, current) => true,
        _ => false,
    }
}

pub fn miss_count(routes: &ServerRoutes, addr: &str) -> u32 {
    routes.misses.iter().find(|row| same_endpoint(&row.addr, addr)).map(|row| row.count).unwrap_or(0)
}

/// This address answered. Later dials try it first, and its timeout count is cleared.
pub fn note_answer(routes: &mut ServerRoutes, addr: &str) {
    let addr = canonical(addr);
    if addr.is_empty() {
        return;
    }
    routes.preferred = addr.clone();
    routes.misses.retain(|row| !same_endpoint(&row.addr, &addr));
}

/// This address did not answer. The count is consecutive.
pub fn note_timeout(routes: &mut ServerRoutes, addr: &str) {
    let addr = canonical(addr);
    if addr.is_empty() {
        return;
    }
    if let Some(row) = routes.misses.iter_mut().find(|row| same_endpoint(&row.addr, &addr)) {
        row.count = row.count.saturating_add(1);
        row.addr = addr;
        return;
    }
    routes.misses.push(RouteMiss { addr, count: 1 });
}

pub fn note_server_version(routes: &mut ServerRoutes, version: &str) {
    let version = version.trim();
    if version.is_empty() {
        return;
    }
    routes.server_version = version.to_string();
}

/// Host without the port, for a sentence a person reads.
pub fn display_host(text: &str) -> String {
    host_of(text).unwrap_or("").to_string()
}

fn carry_health(next: &mut ServerRoutes, previous: &ServerRoutes) {
    if next.preferred.trim().is_empty() {
        next.preferred = previous.preferred.clone();
    }
    if next.misses.is_empty() {
        next.misses = previous.misses.clone();
    }
    if next.server_version.is_empty() {
        next.server_version = previous.server_version.clone();
    }
}

fn listed_addrs(routes: &ServerRoutes) -> Vec<String> {
    routes
        .primary_lan
        .iter()
        .chain(routes.primary_public.iter())
        .chain(routes.standby_lan.iter())
        .chain(routes.standby_public.iter())
        .chain(routes.names.iter())
        .cloned()
        .collect()
}

/// Both servers published the same public IP. That address cannot reach both sites.
pub fn shared_public_line(routes: &ServerRoutes) -> String {
    let primary = public_ips(&routes.primary_public);
    let standby = public_ips(&routes.standby_public);
    for ip in &primary {
        if standby.contains(ip) {
            return format!(
                "This server and the other server both advertise {ip}. Computers at the other site cannot use that address to reach this one. Set each server's public address to the address its own internet connection uses."
            );
        }
    }
    String::new()
}

/// The primary's public address did not accept a connection.
pub fn probe_line(addr: &str, open: bool, from_outside: bool, self_check: bool) -> String {
    if open || addr.trim().is_empty() {
        return String::new();
    }
    if from_outside {
        return format!(
            "The primary's public address {addr} did not answer a reachability check from pjdesk.com. Computers that are not on its local network cannot register with it."
        );
    }
    if self_check {
        return format!(
            "This server could not open its public address {addr}. Computers off this network may not be able to register. A router that does not loop that address back can fail this check even when the address works from outside."
        );
    }
    format!(
        "The primary's public address {addr} did not answer a reachability check from this server. Computers that are not on its local network cannot register with it."
    )
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

fn listed(routes: &ServerRoutes, addr: &str) -> bool {
    routes
        .primary_lan
        .iter()
        .chain(routes.primary_public.iter())
        .chain(routes.standby_lan.iter())
        .chain(routes.standby_public.iter())
        .chain(routes.names.iter())
        .any(|have| same_endpoint(have, addr))
}

fn classify_kept(addr: &str, lan: &mut Vec<String>, public: &mut Vec<String>, names: &mut Vec<String>) {
    let text = canonical(addr);
    if text.is_empty() {
        return;
    }
    if let Some(ip) = endpoint_v4(&text) {
        if ip.is_private() {
            push_unique(lan, text);
        } else {
            push_unique(public, text);
        }
    } else {
        push_unique(names, text);
    }
}

fn public_ips(list: &[String]) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for raw in list {
        let Some(ip) = endpoint_v4(raw) else { continue };
        if ip.is_private() || out.contains(&ip) {
            continue;
        }
        out.push(ip);
    }
    out
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
            ..ServerRoutes::default()
        }
    }

    #[test]
    fn saved_lan_is_dialed_before_the_hostname_when_the_catalog_has_no_ip() {
        let routes = ServerRoutes { names: vec!["pjdesk.spader.us:21200".into()], ..ServerRoutes::default() };
        let merged = routes_with_legacy(
            &routes,
            "pjdesk.spader.us:21200",
            &["192.168.0.193:21200".into()],
            "204.248.122.44",
            "",
            &[],
            "",
        );
        let order = dial_targets(&merged, &[v4(192, 168, 0, 10)], "pjdesk.spader.us:21200");
        assert_eq!(order.first().map(String::as_str), Some("192.168.0.193:21200"));
        assert!(order.iter().any(|addr| addr.starts_with("204.248.122.44")), "{order:?}");
        assert_eq!(order.last().map(String::as_str), Some("pjdesk.spader.us:21200"));
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
        assert!(!ServerRoutes::default().has_advertised_ip());
    }

    #[test]
    fn a_cached_catalog_is_dialed_before_the_saved_hostname() {
        let cached = ServerRoutes {
            primary_lan: vec!["10.1.1.8:21200".into()],
            primary_public: vec!["198.51.100.8:21200".into()],
            standby_public: vec!["204.248.122.44:21200".into()],
            ..ServerRoutes::default()
        };
        assert!(cached.has_advertised_ip());
        let house = [v4(192, 168, 0, 10)];
        let order = dial_targets(&cached, &house, "pjdesk.spader.us:21200");
        assert_eq!(order[0], "198.51.100.8:21200");
        assert!(order.iter().any(|addr| addr == "204.248.122.44:21200"));
        assert!(!order.iter().any(|addr| addr.starts_with("10.1.1.8")), "cabin LAN is a different subnet");
        assert_eq!(order.last().map(String::as_str), Some("pjdesk.spader.us:21200"));
        let cabin = [v4(10, 1, 1, 50)];
        let from_cabin = dial_targets(&cached, &cabin, "pjdesk.spader.us:21200");
        assert_eq!(from_cabin[0], "10.1.1.8:21200");
        assert_eq!(from_cabin.last().map(String::as_str), Some("pjdesk.spader.us:21200"));
    }

    #[test]
    fn a_peer_on_this_subnet_is_dialed_on_the_direct_port() {
        let mine = [v4(192, 168, 1, 10)];
        let got = same_subnet_direct(
            &[
                "192.168.1.20:40000".into(),
                "192.168.1.20:21201".into(),
                "10.1.1.5:21201".into(),
                "pjdesk.spader.us:21200".into(),
            ],
            &mine,
        );
        assert_eq!(got, vec!["192.168.1.20:21201".to_string()]);
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

    #[test]
    fn a_dead_follow_address_does_not_hide_the_other_server() {
        let house = [v4(192, 168, 0, 10)];
        let order = registration_targets("192.168.0.20:21200", &sample(), &house, "pjdesk.spader.us:21200");
        assert_eq!(order[0], "192.168.0.20:21200");
        assert!(order.iter().any(|addr| addr == "198.51.100.8:21200"));
        assert_eq!(order.last().map(String::as_str), Some("pjdesk.spader.us:21200"));
        // 10.1.1.8 is the standby LAN on this computer's /24, so it is first.
        // The primary's public address follows.
        let cabin = [v4(10, 1, 1, 50)];
        let from_cabin = registration_targets("", &sample(), &cabin, "");
        assert_eq!(from_cabin[0], "10.1.1.8:21200");
        assert_eq!(from_cabin[1], "204.248.122.44:21200");
        assert!(from_cabin.iter().any(|addr| addr == "198.51.100.8:21200"));
        assert_eq!(from_cabin.last().map(String::as_str), Some("pjdesk.spader.us:21200"));
    }

    #[test]
    fn a_failed_registration_names_every_address_it_tried() {
        let report = registration_miss(&[
            ("192.168.0.20:21200", "timed out"),
            ("204.248.122.44:21200", "connection refused"),
            ("pjdesk.spader.us:21200", "timed out"),
        ]);
        assert!(report.starts_with("could not reach the primary or the standby; retrying. timed out"));
        let lines = registration_detail_lines(&report);
        assert_eq!(
            lines,
            vec![
                "192.168.0.20:21200: timed out".to_string(),
                "204.248.122.44:21200: connection refused".to_string(),
                "pjdesk.spader.us:21200: timed out".to_string(),
            ]
        );
        assert!(registration_detail_lines("could not reach the primary or the standby; retrying. timed out").is_empty());
    }

    #[test]
    fn a_routes_push_that_omits_the_other_server_keeps_it() {
        let previous = sample();
        let only_house = ServerRoutes {
            primary_lan: vec!["192.168.0.20:21200".into()],
            primary_public: vec!["204.248.122.44:21200".into()],
            names: vec!["pjdesk.spader.us:21200".into()],
            ..ServerRoutes::default()
        };
        let kept = keep_other_server(&previous, &only_house);
        assert!(kept.standby_public.iter().any(|addr| addr == "198.51.100.8:21200"));
        assert!(kept.standby_lan.iter().any(|addr| addr == "10.1.1.8:21200"));
        let house = [v4(192, 168, 1, 9)];
        let order = dial_order(&kept, &house);
        assert_eq!(order[0], "204.248.122.44:21200");
        assert!(order.iter().any(|addr| addr == "198.51.100.8:21200"));
        assert_eq!(order.last().map(String::as_str), Some("pjdesk.spader.us:21200"));
    }

    #[test]
    fn the_same_public_ip_on_both_servers_is_a_warning() {
        let shared = shared_public_line(&sample());
        assert!(shared.is_empty());
        let mut both = sample();
        both.standby_public = vec!["204.248.122.44:21200".into()];
        let line = shared_public_line(&both);
        assert!(line.contains("204.248.122.44"));
        assert!(line.contains("other server"));
        assert!(probe_line("198.51.100.8:21200", true, true, false).is_empty());
        assert!(probe_line("198.51.100.8:21200", false, true, false).contains("pjdesk.com"));
        assert!(probe_line("198.51.100.8:21200", false, false, true).contains("loop"));
    }

    #[test]
    fn an_address_that_answered_is_dialed_first_and_repeated_timeouts_are_dropped() {
        let mut routes = sample();
        note_answer(&mut routes, "pjdesk.spader.us:21200");
        note_timeout(&mut routes, "192.168.0.231:21200");
        note_timeout(&mut routes, "204.248.122.44:21200");
        note_timeout(&mut routes, "204.248.122.44:21200");
        note_timeout(&mut routes, "68.234.75.68:21200");
        note_timeout(&mut routes, "68.234.75.68:21200");
        note_timeout(&mut routes, "68.234.75.68:21200");
        note_timeout(&mut routes, "68.234.75.68:21200");
        routes.primary_lan.insert(0, "192.168.0.231:21200".into());
        routes.primary_public.push("68.234.75.68:21200".into());
        let house = [v4(192, 168, 0, 10)];
        let order = dial_targets(&routes, &house, "pjdesk.spader.us:21200");
        assert!(order[0].starts_with("192.168.0."), "{order:?}");
        let lan = order.iter().position(|addr| addr.starts_with("192.168.0.20")).unwrap();
        let stale = order.iter().position(|addr| addr.starts_with("204.248.122.44")).unwrap();
        assert!(lan < stale, "{order:?}");
        assert!(order.iter().any(|addr| addr == "pjdesk.spader.us:21200"), "{order:?}");
        assert!(!order.iter().any(|addr| addr.starts_with("68.234.75.68")), "{order:?}");
        assert_eq!(miss_count(&routes, "pjdesk.spader.us:21200"), 0);
    }

    #[test]
    fn off_the_local_network_the_public_address_stays_ahead_of_a_home_lan_address() {
        let mut routes = sample();
        note_answer(&mut routes, "192.168.0.20:21200");
        note_timeout(&mut routes, "204.248.122.44:21200");
        note_timeout(&mut routes, "204.248.122.44:21200");
        note_timeout(&mut routes, "204.248.122.44:21200");
        note_timeout(&mut routes, "204.248.122.44:21200");
        let cellular = [v4(10, 9, 9, 9)];
        let order = dial_targets(&routes, &cellular, "pjdesk.spader.us:21200");
        assert_eq!(order[0], "198.51.100.8:21200", "{order:?}");
        assert!(!order.iter().any(|addr| addr.starts_with("204.248.122.44")), "{order:?}");
        assert!(!order.iter().any(|addr| addr.starts_with("192.168.0.")), "{order:?}");
        let name = order.iter().position(|addr| addr.contains("pjdesk.spader.us")).unwrap();
        assert!(name > 0, "{order:?}");
    }

    #[test]
    fn the_home_wifi_dials_the_server_lan_before_a_stale_public_route() {
        let mut routes = ServerRoutes {
            primary_lan: vec!["192.168.0.193:21200".into()],
            primary_public: vec!["204.248.122.44:21200".into()],
            names: vec!["pjdesk.spader.us:21200".into()],
            preferred: "204.248.122.44:21200".into(),
            ..ServerRoutes::default()
        };
        let phone = [v4(192, 168, 0, 245)];
        let order = dial_targets(&routes, &phone, "204.248.122.44:21200");
        assert_eq!(order[0], "192.168.0.193:21200", "{order:?}");
        assert!(lan_should_replace("204.248.122.44:21200", &routes, &phone));
        assert!(!lan_should_replace("192.168.0.193:21200", &routes, &phone));
        for _ in 0..4 {
            note_timeout(&mut routes, "204.248.122.44:21200");
        }
        let pruned = dial_targets(&routes, &phone, "");
        assert_eq!(pruned[0], "192.168.0.193:21200", "{pruned:?}");
        assert!(!pruned.iter().any(|addr| addr.starts_with("204.248.122.44")), "{pruned:?}");
        let away = dial_targets(&routes, &[v4(10, 9, 9, 9)], "pjdesk.spader.us:21200");
        assert!(!away.iter().any(|addr| addr.starts_with("204.248.122.44")), "{away:?}");
        assert!(!away.iter().any(|addr| addr.starts_with("192.168.0.193")), "{away:?}");
        let remembered = ServerRoutes {
            primary_lan: vec!["192.168.0.193:21200".into()],
            primary_public: vec!["204.248.122.44:21200".into()],
            preferred: "192.168.0.193:21200".into(),
            ..ServerRoutes::default()
        };
        let without_phone_ip = dial_targets(&remembered, &[], "");
        assert_eq!(without_phone_ip[0], "192.168.0.193:21200", "{without_phone_ip:?}");
        let both = ServerRoutes {
            primary_lan: vec!["192.168.0.193:21200".into()],
            primary_public: vec!["204.248.122.44:21200".into()],
            standby_lan: vec!["192.168.0.169:21200".into()],
            ..ServerRoutes::default()
        };
        let expanded = account_dial_order("204.248.122.44:21200", &both, &phone);
        assert_eq!(expanded[0], "192.168.0.193:21200", "{expanded:?}");
        assert!(expanded.iter().any(|addr| addr.starts_with("192.168.0.169")), "{expanded:?}");
        assert!(expanded.len() > 1, "{expanded:?}");
        assert_ne!(expanded, vec!["204.248.122.44:21200".to_string()]);
        let standby = standby_account_order(&both, &phone, "");
        assert_eq!(standby[0], "192.168.0.169:21200", "{standby:?}");
        assert!(!standby.iter().any(|addr| addr.starts_with("204.248.122.44")), "{standby:?}");
        assert!(!standby.iter().any(|addr| addr.starts_with("192.168.0.193")), "{standby:?}");
    }

    #[test]
    fn an_advertisement_drops_an_address_that_already_timed_out_twice() {
        let mut previous = sample();
        note_timeout(&mut previous, "192.168.0.231:21200");
        note_timeout(&mut previous, "192.168.0.231:21200");
        previous.primary_lan.insert(0, "192.168.0.231:21200".into());
        let incoming = ServerRoutes {
            primary_lan: vec!["192.168.0.193:21200".into()],
            primary_public: vec!["204.248.122.44:21200".into()],
            standby_lan: vec!["10.1.1.8:21200".into()],
            standby_public: vec!["198.51.100.8:21200".into()],
            names: vec!["pjdesk.spader.us:21200".into()],
            ..ServerRoutes::default()
        };
        let adopted = adopt_advertised(&previous, &incoming);
        assert!(!adopted.primary_lan.iter().any(|addr| addr.starts_with("192.168.0.231")));
        assert!(adopted.primary_lan.iter().any(|addr| addr.starts_with("192.168.0.193")));
        let mut quiet = sample();
        note_timeout(&mut quiet, "10.9.9.9:21200");
        quiet.standby_lan.push("10.9.9.9:21200".into());
        let half = ServerRoutes {
            primary_lan: vec!["192.168.0.193:21200".into()],
            primary_public: vec!["204.248.122.44:21200".into()],
            names: vec!["pjdesk.spader.us:21200".into()],
            ..ServerRoutes::default()
        };
        let kept = adopt_advertised(&quiet, &half);
        assert!(kept.standby_lan.iter().any(|addr| addr.starts_with("10.9.9.9")));
        note_timeout(&mut quiet, "10.9.9.9:21200");
        let dropped = adopt_advertised(&quiet, &half);
        assert!(!dropped.standby_lan.iter().any(|addr| addr.starts_with("10.9.9.9")));
    }

    #[test]
    fn the_lenovo_on_this_subnet_is_tried_before_the_cabin() {
        let routes = ServerRoutes {
            primary_lan: vec!["192.168.0.193:21200".into()],
            primary_public: vec!["204.248.122.44:21200".into()],
            standby_lan: vec!["10.0.0.20:21200".into(), "10.5.0.21:21200".into(), "192.168.192.1:21200".into()],
            standby_public: vec!["68.234.75.68:21200".into()],
            ..ServerRoutes::default()
        };
        let phone = [v4(192, 168, 0, 245)];
        let discovered = ["192.168.0.169:21200".to_string()];
        let order = sign_in_peer_order(&routes, &phone, &discovered, "192.168.0.193:21200");
        assert_eq!(order[0], "192.168.0.169:21200", "{order:?}");
        let lenovo = order.iter().position(|addr| addr.starts_with("192.168.0.169")).unwrap();
        let cabin = order.iter().position(|addr| addr.starts_with("10.0.0.20")).unwrap();
        let cabin_public = order.iter().position(|addr| addr.starts_with("68.234.75.68")).unwrap();
        let primary = order.iter().position(|addr| addr.starts_with("192.168.0.193")).unwrap();
        assert!(lenovo < cabin, "{order:?}");
        assert!(cabin < cabin_public, "{order:?}");
        assert!(lenovo < primary, "{order:?}");
        let previous = ServerRoutes {
            primary_lan: vec!["192.168.0.193:21200".into()],
            standby_lan: vec!["192.168.0.169:21200".into()],
            ..ServerRoutes::default()
        };
        let kept = keep_other_server(&previous, &routes);
        assert!(kept.standby_lan.iter().any(|addr| addr.starts_with("192.168.0.169")), "{kept:?}");
        assert!(kept.standby_lan.iter().any(|addr| addr.starts_with("10.0.0.20")), "{kept:?}");
    }
}
