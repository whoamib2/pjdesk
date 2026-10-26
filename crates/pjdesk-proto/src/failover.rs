//! Automatic standby failover decisions.
//!
//! The standby probes the primary and reads the primary's cloud health.
//! It promotes only when both say the primary is gone. A network partition
//! that hides the primary while its health is still fresh does not promote.
//! A returning primary that sees a higher epoch demotes itself and does not
//! take the role back on its own.

use std::time::Duration;

/// How often a server publishes health and how often the standby probes.
pub const BEAT_SECS: u64 = 3;
/// Missed direct probes before a takeover is even considered.
pub const MISSES_TO_PROMOTE: u32 = 3;
/// Cloud health older than this confirms the primary stopped publishing.
/// Three beats is 9 s; 10 s still treats one slow write as alive.
pub const CONFIRM_STALE_SECS: u64 = 10;
/// First reconnects after a primary loss wait this long, not the long backoff.
pub const FAST_REGISTER_SECS: u64 = 1;
/// How many of those fast tries happen before the normal backoff starts.
pub const FAST_REGISTER_TRIES: u32 = 12;

/// Sent when a relayed session's forwarder died and the controller should
/// open the session again with the password it already has.
pub const RELAY_RESUME: &str = "relayed session lost the rendezvous server; reconnecting";

pub fn is_relay_resume(message: &str) -> bool {
    message.contains("relayed session lost the rendezvous server")
}

/// What the latest direct probe of the primary saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// TCP signaling port accepted a connection.
    Reachable,
    /// TCP failed, but the rendezvous port sent a datagram back.
    UdpOnly,
    /// TCP failed and UDP did not answer. UDP silence alone is not proof.
    Silent,
}

/// The confirming path. Cloud health is this path. A failed read is not
/// evidence that the primary is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecondPath {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Takeover {
    Stay(&'static str),
    Promote,
}

pub struct Watch {
    pub misses: u32,
    pub probe: Probe,
    pub second: SecondPath,
    pub already_primary: bool,
    pub have_address: bool,
}

/// Promote only after repeated direct misses and a stale confirming path.
pub fn takeover(watch: &Watch) -> Takeover {
    if watch.already_primary {
        return Takeover::Stay("this server is already the primary");
    }
    if !watch.have_address {
        return Takeover::Stay("no primary address to probe");
    }
    match watch.probe {
        Probe::Reachable => Takeover::Stay("primary answered the direct probe"),
        Probe::UdpOnly => Takeover::Stay("primary answered on the rendezvous port"),
        Probe::Silent if watch.misses < MISSES_TO_PROMOTE => {
            Takeover::Stay("waiting for more missed heartbeats")
        }
        Probe::Silent => match watch.second {
            SecondPath::Fresh => Takeover::Stay("primary is still publishing health; not taking over"),
            SecondPath::Unknown => Takeover::Stay("cloud health could not be read; not taking over"),
            SecondPath::Stale => Takeover::Promote,
        },
    }
}

/// `newest_other_at` is the cloud health stamp of another server.
/// `None` with a successful read means that server is absent.
pub fn second_path(read_ok: bool, newest_other_at: Option<u64>, now: u64) -> SecondPath {
    if !read_ok {
        return SecondPath::Unknown;
    }
    match newest_other_at {
        Some(at) if now.saturating_sub(at) <= CONFIRM_STALE_SECS => SecondPath::Fresh,
        Some(_) | None => SecondPath::Stale,
    }
}

/// Miss counter after one probe. A live answer resets it.
pub fn next_misses(misses: u32, probe: Probe) -> u32 {
    match probe {
        Probe::Reachable | Probe::UdpOnly => 0,
        Probe::Silent => misses.saturating_add(1),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yield {
    Hold,
    Demote,
}

/// Fail-back policy: never automatic.
///
/// `healthy_for` and `idle` are part of the decision so a test can show that
/// a healthy, idle returning primary still does not promote itself. A person
/// uses Become the primary when they want the old server back. Automatic
/// fail-back would bounce the generation every time the link blipped.
pub fn on_return(
    local_epoch: u64,
    remote_epoch: u64,
    remote_is_me: bool,
    healthy_for: Duration,
    idle: bool,
) -> Yield {
    let _ = (healthy_for, idle);
    if !remote_is_me && remote_epoch > local_epoch {
        Yield::Demote
    } else {
        Yield::Hold
    }
}

/// True when another instance holds a newer generation than this server.
pub fn epoch_fences(local_epoch: u64, fenced_epoch: u64, fenced_instance: &str, mine: &str) -> bool {
    fenced_epoch > local_epoch && !fenced_instance.is_empty() && fenced_instance != mine
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaPath {
    /// Direct IP, no rendezvous server in the path.
    Direct,
    Lan,
    /// Internet path that punched through and then left the server behind.
    HolePunched,
    Relayed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionFate {
    /// QUIC does not go through the server. The session keeps running.
    Survives,
    /// The relay process was the path. Open the session again in the
    /// background with the password already entered. No new prompt.
    Reconnect,
}

pub fn session_fate(path: MediaPath) -> SessionFate {
    match path {
        MediaPath::Direct | MediaPath::Lan | MediaPath::HolePunched => SessionFate::Survives,
        MediaPath::Relayed => SessionFate::Reconnect,
    }
}

pub fn classify_link(relayed: bool, lan: bool, direct_ip: bool) -> MediaPath {
    if direct_ip {
        MediaPath::Direct
    } else if relayed {
        MediaPath::Relayed
    } else if lan {
        MediaPath::Lan
    } else {
        MediaPath::HolePunched
    }
}

/// The address a standby should probe. `None` when this server is the
/// published primary, or when the account has no active address.
pub fn choose_peer(mine: &str, active: &str, _standby: &str) -> Option<String> {
    let active = active.trim();
    if active.is_empty() || same_server(mine, active) {
        None
    } else {
        Some(active.to_string())
    }
}

pub fn same_server(a: &str, b: &str) -> bool {
    let (ah, ap) = host_port(a);
    let (bh, bp) = host_port(b);
    if ah.is_empty() || bh.is_empty() || ah != bh {
        return false;
    }
    match (ap, bp) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

fn host_port(text: &str) -> (String, Option<u16>) {
    let text = text.trim();
    if let Some((host, port)) = text.rsplit_once(':') {
        if !host.is_empty() && !host.contains(']') && port.chars().all(|c| c.is_ascii_digit()) {
            return (host.to_ascii_lowercase(), port.parse().ok());
        }
    }
    (text.to_ascii_lowercase(), None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegFail {
    Unreachable,
    StillStandby,
    KeyMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NextRegister {
    pub use_standby: bool,
    pub delay: Duration,
    pub advance_backoff: bool,
    pub log: &'static str,
}

/// Where to register next. A primary loss tries the known standby in one
/// second. A standby that has not taken over yet tries the primary again
/// just as quickly. The long backoff starts only after those fast tries.
pub fn next_register(
    fail: RegFail,
    on_standby: bool,
    has_standby: bool,
    fast_tries: u32,
    backoff: Duration,
) -> NextRegister {
    let fast = Duration::from_secs(FAST_REGISTER_SECS);
    let quick = fast_tries < FAST_REGISTER_TRIES;
    match fail {
        RegFail::KeyMismatch => NextRegister {
            use_standby: on_standby,
            delay: backoff,
            advance_backoff: false,
            log: "",
        },
        RegFail::StillStandby if has_standby && quick => NextRegister {
            use_standby: false,
            delay: fast,
            advance_backoff: false,
            log: "standby has not taken over; trying the primary again",
        },
        RegFail::Unreachable if has_standby && quick && !on_standby => NextRegister {
            use_standby: true,
            delay: fast,
            advance_backoff: false,
            log: "primary did not answer; registering with the standby",
        },
        RegFail::Unreachable if has_standby && quick && on_standby => NextRegister {
            use_standby: false,
            delay: fast,
            advance_backoff: false,
            log: "standby did not answer; trying the primary again",
        },
        _ => NextRegister {
            use_standby: has_standby && !on_standby,
            delay: backoff,
            advance_backoff: true,
            log: "",
        },
    }
}

/// Keep the saved-list Online mark through a short failover while the last
/// server ack is still inside the normal freshness window.
pub fn keep_saved_status(has_standby: bool, acked_recently: bool) -> bool {
    has_standby && acked_recently
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(misses: u32, probe: Probe, second: SecondPath) -> Watch {
        Watch { misses, probe, second, already_primary: false, have_address: true }
    }

    #[test]
    fn three_missed_probes_and_a_dead_second_path_promote() {
        let misses = next_misses(next_misses(next_misses(0, Probe::Silent), Probe::Silent), Probe::Silent);
        assert_eq!(misses, 3);
        assert_eq!(
            takeover(&watch(misses, Probe::Silent, SecondPath::Stale)),
            Takeover::Promote
        );
    }

    #[test]
    fn probe_failure_with_a_fresh_cloud_heartbeat_does_not_promote() {
        let misses = 3;
        assert_eq!(
            takeover(&watch(misses, Probe::Silent, SecondPath::Fresh)),
            Takeover::Stay("primary is still publishing health; not taking over")
        );
        assert_eq!(second_path(true, Some(1_000), 1_008), SecondPath::Fresh);
        assert_eq!(second_path(true, Some(1_000), 1_011), SecondPath::Stale);
        assert_eq!(second_path(false, None, 1_011), SecondPath::Unknown);
    }

    #[test]
    fn a_higher_epoch_demotes_a_sticky_primary() {
        assert!(epoch_fences(4, 5, "chip20", "lenovo"));
        assert!(!epoch_fences(5, 5, "chip20", "lenovo"));
        assert!(!epoch_fences(4, 5, "lenovo", "lenovo"));
        assert_eq!(
            on_return(4, 5, false, Duration::from_secs(0), false),
            Yield::Demote
        );
    }

    #[test]
    fn a_returning_primary_does_not_fail_back_by_itself() {
        assert_eq!(
            on_return(4, 5, false, Duration::from_secs(10 * 60), true),
            Yield::Demote,
            "healthy and idle still does not take the primary back"
        );
        assert_eq!(on_return(5, 5, true, Duration::from_secs(10 * 60), true), Yield::Hold);
        assert_eq!(on_return(6, 5, false, Duration::from_secs(0), true), Yield::Hold);
    }

    #[test]
    fn a_direct_session_survives_rendezvous_loss_and_a_relayed_session_must_reconnect() {
        assert_eq!(session_fate(MediaPath::Direct), SessionFate::Survives);
        assert_eq!(session_fate(MediaPath::Lan), SessionFate::Survives);
        assert_eq!(session_fate(MediaPath::HolePunched), SessionFate::Survives);
        assert_eq!(session_fate(MediaPath::Relayed), SessionFate::Reconnect);
        assert_eq!(classify_link(false, true, false), MediaPath::Lan);
        assert_eq!(classify_link(false, false, false), MediaPath::HolePunched);
        assert_eq!(classify_link(true, false, false), MediaPath::Relayed);
        assert_eq!(classify_link(true, false, true), MediaPath::Direct);
        assert!(is_relay_resume(RELAY_RESUME));
    }

    #[test]
    fn two_misses_and_an_unread_cloud_do_not_promote() {
        assert_eq!(
            takeover(&watch(2, Probe::Silent, SecondPath::Stale)),
            Takeover::Stay("waiting for more missed heartbeats")
        );
        assert_eq!(
            takeover(&watch(3, Probe::Silent, SecondPath::Unknown)),
            Takeover::Stay("cloud health could not be read; not taking over")
        );
        assert_eq!(
            takeover(&watch(3, Probe::UdpOnly, SecondPath::Stale)),
            Takeover::Stay("primary answered on the rendezvous port")
        );
        assert_eq!(next_misses(4, Probe::Reachable), 0);
    }

    #[test]
    fn the_standby_probes_the_active_address_and_the_primary_does_not_probe_itself() {
        assert_eq!(
            choose_peer("chip.example:21200", "pjdesk.spader.us:21200", "chip.example:21200").as_deref(),
            Some("pjdesk.spader.us:21200")
        );
        assert_eq!(choose_peer("pjdesk.spader.us:21200", "pjdesk.spader.us", "chip.example:21200"), None);
        assert_eq!(choose_peer("chip.example", "", "chip.example"), None);
    }

    #[test]
    fn a_primary_loss_registers_with_the_standby_without_the_long_backoff() {
        let hop = next_register(RegFail::Unreachable, false, true, 0, Duration::from_secs(8));
        assert!(hop.use_standby);
        assert_eq!(hop.delay, Duration::from_secs(1));
        assert!(!hop.advance_backoff);
        assert!(hop.log.contains("standby"));
        let back = next_register(RegFail::StillStandby, true, true, 1, Duration::from_secs(8));
        assert!(!back.use_standby);
        assert_eq!(back.delay, Duration::from_secs(1));
        assert!(back.log.contains("not taken over"));
        let later = next_register(
            RegFail::Unreachable,
            false,
            true,
            FAST_REGISTER_TRIES,
            Duration::from_secs(4),
        );
        assert!(later.advance_backoff);
        assert_eq!(later.delay, Duration::from_secs(4));
        assert!(keep_saved_status(true, true));
        assert!(!keep_saved_status(true, false));
        assert!(!keep_saved_status(false, true));
    }
}
