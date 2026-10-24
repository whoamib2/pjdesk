//! Per-source and per-host password lockout.
//!
//! Five wrong passwords from one source, or twenty from any mix of sources,
//! inside a twenty-minute window lock password checks for twenty minutes.
//! A correct password clears that source only. While a lock is in force the
//! host answers with [`message`] and does not say "wrong password".

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// How long a failure keeps counting, and how long a lock lasts.
pub const WINDOW: Duration = Duration::from_secs(20 * 60);
const PER_SOURCE: usize = 5;
const GLOBAL: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub locked: bool,
    pub minutes: u32,
    /// This call is the one that started the lock.
    pub fresh: bool,
    /// The lock covers every source, not only the one that just failed.
    pub global: bool,
}

impl Status {
    fn open() -> Self {
        Self { locked: false, minutes: 0, fresh: false, global: false }
    }
}

#[derive(Default)]
pub struct Lockout {
    /// Wrong-password times still inside the window, tagged by source.
    hits: Vec<(String, Instant)>,
    source_until: HashMap<String, Instant>,
    global_until: Option<Instant>,
}

impl Lockout {
    pub fn check(&mut self, source: &str, now: Instant) -> Status {
        self.prune(now);
        self.locked_status(source, now, false)
    }

    /// Records one wrong password. A source that is already locked does not
    /// gain extra failures and its timer does not start over.
    pub fn fail(&mut self, source: &str, now: Instant) -> Status {
        self.prune(now);
        if self.locked_status(source, now, false).locked {
            return self.locked_status(source, now, false);
        }
        self.hits.push((source.to_string(), now));
        let source_hits = self.hits.iter().filter(|(s, _)| s == source).count();
        let global_hits = self.hits.len();
        let mut fresh_source = false;
        let mut fresh_global = false;
        if source_hits >= PER_SOURCE {
            self.source_until.insert(source.to_string(), now + WINDOW);
            fresh_source = true;
        }
        if global_hits >= GLOBAL {
            self.global_until = Some(now + WINDOW);
            fresh_global = true;
        }
        if fresh_source || fresh_global {
            self.locked_status(source, now, true)
        } else {
            Status::open()
        }
    }

    /// A correct password clears this source. Other sources, and a global lock, stay.
    pub fn succeed(&mut self, source: &str) {
        self.hits.retain(|(s, _)| s != source);
        self.source_until.remove(source);
    }

    fn prune(&mut self, now: Instant) {
        self.hits.retain(|(_, at)| now.saturating_duration_since(*at) < WINDOW);
        self.source_until.retain(|_, until| *until > now);
        if self.global_until.is_some_and(|until| until <= now) {
            self.global_until = None;
        }
    }

    fn locked_status(&self, source: &str, now: Instant, fresh: bool) -> Status {
        let source_until = self.source_until.get(source).copied().filter(|until| *until > now);
        let global_until = self.global_until.filter(|until| *until > now);
        let until = match (source_until, global_until) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(until) = until else {
            return Status::open();
        };
        Status {
            locked: true,
            minutes: minutes_left(until, now),
            fresh,
            global: global_until.is_some(),
        }
    }
}

pub fn minutes_left(until: Instant, now: Instant) -> u32 {
    let secs = until.saturating_duration_since(now).as_secs();
    ((secs + 59) / 60).max(1) as u32
}

pub fn message(minutes: u32) -> String {
    format!("locked out, try again in {} minutes", minutes.max(1))
}

pub fn parse_minutes(text: &str) -> Option<u32> {
    let rest = text.trim().strip_prefix("locked out, try again in ")?;
    let n = rest.strip_suffix(" minutes")?.trim();
    n.parse().ok()
}

/// Identity for a lockout bucket.
///
/// A relayed QUIC connection's remote address is the server. The address the
/// relay observed for the peer (`observed`) is the real source. When no IP is
/// available, `device_id` is the controller's id (or its name, which is all
/// the host is sent today).
pub fn source_key(
    quic_ip: IpAddr,
    relay_ip: Option<IpAddr>,
    observed: Option<IpAddr>,
    signaled: Option<IpAddr>,
    device_id: &str,
) -> String {
    let usable = |ip: IpAddr| !ip.is_unspecified();
    let relayed = relay_ip.is_some_and(|relay| relay == quic_ip);
    if !relayed && usable(quic_ip) {
        return format!("ip:{quic_ip}");
    }
    if let Some(ip) = observed.filter(|ip| usable(*ip) && relay_ip != Some(*ip)) {
        return format!("ip:{ip}");
    }
    if let Some(ip) = signaled.filter(|ip| usable(*ip)) {
        return format!("ip:{ip}");
    }
    let id = device_id.trim();
    if id.is_empty() {
        "id:unknown".to_string()
    } else {
        format!("id:{id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn five_failures_lock_one_source_and_a_success_clears_it() {
        let mut lock = Lockout::default();
        let start = Instant::now();
        for n in 0..4 {
            let status = lock.fail("ip:203.0.113.8", start + Duration::from_secs(n));
            assert!(!status.locked, "failure {n} should still be open");
        }
        let tripped = lock.fail("ip:203.0.113.8", start + Duration::from_secs(10));
        assert!(tripped.locked && tripped.fresh && !tripped.global);
        assert_eq!(tripped.minutes, 20);
        assert_eq!(message(tripped.minutes), "locked out, try again in 20 minutes");
        let other = lock.check("ip:203.0.113.9", start + Duration::from_secs(11));
        assert!(!other.locked, "a different source is not locked");
        lock.succeed("ip:203.0.113.8");
        let again = lock.check("ip:203.0.113.8", start + Duration::from_secs(12));
        assert!(!again.locked);
    }

    #[test]
    fn twenty_failures_lock_every_source() {
        let mut lock = Lockout::default();
        let start = Instant::now();
        for n in 0..19 {
            let status = lock.fail(&format!("ip:203.0.113.{n}"), start + Duration::from_secs(n));
            assert!(!status.locked);
        }
        let tripped = lock.fail("ip:203.0.113.19", start + Duration::from_secs(19));
        assert!(tripped.locked && tripped.fresh && tripped.global);
        let bystander = lock.check("ip:198.51.100.4", start + Duration::from_secs(20));
        assert!(bystander.locked && bystander.global);
        lock.succeed("ip:203.0.113.19");
        assert!(lock.check("ip:198.51.100.4", start + Duration::from_secs(21)).locked);
    }

    #[test]
    fn failures_older_than_the_window_stop_counting() {
        let mut lock = Lockout::default();
        let start = Instant::now();
        for n in 0..4 {
            assert!(!lock.fail("ip:203.0.113.8", start + Duration::from_secs(n)).locked);
        }
        let later = start + WINDOW + Duration::from_secs(5);
        assert!(!lock.fail("ip:203.0.113.8", later).locked, "a failure outside the window starts the count over");
    }

    #[test]
    fn a_lock_expires_and_does_not_restart_when_prodded() {
        let mut lock = Lockout::default();
        let start = Instant::now();
        for n in 0..5 {
            lock.fail("ip:203.0.113.8", start + Duration::from_secs(n));
        }
        let during = lock.fail("ip:203.0.113.8", start + Duration::from_secs(30));
        assert!(during.locked && !during.fresh, "a repeat while locked does not start a new lock");
        let later = start + WINDOW + Duration::from_secs(31);
        assert!(!lock.check("ip:203.0.113.8", later).locked);
        assert!(!lock.fail("ip:203.0.113.8", later).locked);
    }

    #[test]
    fn relay_uses_the_observed_address_and_otherwise_the_device_id() {
        let relay = ip("203.0.113.5");
        let client = ip("198.51.100.20");
        assert_eq!(
            source_key(relay, Some(relay), Some(client), None, "692996394"),
            "ip:198.51.100.20"
        );
        assert_eq!(source_key(relay, Some(relay), None, None, "692996394"), "id:692996394");
        let direct = IpAddr::V4(Ipv4Addr::new(192, 168, 0, 10));
        assert_eq!(source_key(direct, Some(relay), Some(client), None, "692996394"), "ip:192.168.0.10");
        let private = ip("192.168.0.231");
        assert_eq!(source_key(private, None, None, None, ""), format!("ip:{private}"));
    }
}
