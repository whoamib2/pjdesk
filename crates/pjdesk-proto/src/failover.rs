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

/// Same reconnect, naming the primary the session must use.
/// The first line stays [`RELAY_RESUME`] so an older check still matches.
pub fn relay_resume_message(primary: &str) -> String {
    let primary = primary.trim();
    if primary.is_empty() {
        RELAY_RESUME.to_string()
    } else {
        format!("{RELAY_RESUME}\nprimary {primary}")
    }
}

/// Primary address carried by [`relay_resume_message`]. Empty when the
/// session should use the account's current primary.
pub fn primary_from_resume(message: &str) -> String {
    message
        .lines()
        .find_map(|line| line.trim().strip_prefix("primary "))
        .unwrap_or("")
        .trim()
        .to_string()
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

/// Why a relayed session is leaving the server that was carrying it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveCause {
    /// This server is being demoted, or the other server was promoted.
    /// The move happens before the role change is finished.
    RoleChange,
    /// The relay or the process died. Backup for a crash.
    ServerGone,
}

/// Where one session goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionHop {
    /// Direct, LAN, or hole-punched. The server is not on the path.
    Keep,
    /// Open the session again on this primary. The password is already known.
    ToPrimary { address: String, cause: MoveCause },
}

/// A relayed session moves to the primary. Every other path stays up.
pub fn hop_session(path: MediaPath, cause: MoveCause, primary: &str) -> SessionHop {
    match session_fate(path) {
        SessionFate::Survives => SessionHop::Keep,
        SessionFate::Reconnect => SessionHop::ToPrimary {
            address: primary.trim().to_string(),
            cause,
        },
    }
}

/// Address to name in the move. A public endpoint, then a LAN endpoint, then a hostname.
pub fn move_target(public_addrs: &[String], lan_addrs: &[String], names: &[String]) -> String {
    for list in [public_addrs, lan_addrs, names] {
        if let Some(addr) = list.iter().map(|item| item.trim()).find(|item| !item.is_empty()) {
            return addr.to_string();
        }
    }
    String::new()
}

/// This server's role while relayed sessions are handed to the new primary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RolePhase {
    /// Still relaying. No role change has started.
    Primary,
    /// Sessions have been told. Step-down is not finished.
    Moving,
    /// Step-down is recorded. This server carries no relayed sessions.
    Standby,
}

/// One live session from the point of view of the server that is leaving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSession {
    pub id: u64,
    pub path: MediaPath,
    /// `"here"` while this server carries it. The primary address after it moves.
    pub server: String,
}

/// A demotion or a promotion of the other server.
///
/// [`RoleChange::begin`] tells relayed sessions and does not finish.
/// [`RoleChange::followed`] records that a session is on the new primary.
/// Only then is the phase [`RolePhase::Standby`]. Shutting the old server
/// down after that does not end those sessions.
#[derive(Clone, Debug)]
pub struct RoleChange {
    pub phase: RolePhase,
    pub primary: String,
    told: Vec<u64>,
}

impl RoleChange {
    pub fn new(primary: impl Into<String>) -> Self {
        Self { phase: RolePhase::Primary, primary: primary.into(), told: Vec::new() }
    }

    /// Tell every relayed session still on this server. Returns their ids.
    /// Direct sessions are not told. An empty tell finishes at once.
    pub fn begin(&mut self, sessions: &[LiveSession]) -> Vec<u64> {
        if self.phase == RolePhase::Standby {
            return Vec::new();
        }
        let ids: Vec<u64> = sessions
            .iter()
            .filter(|session| {
                session.server == "here"
                    && matches!(
                        hop_session(session.path, MoveCause::RoleChange, &self.primary),
                        SessionHop::ToPrimary { .. }
                    )
            })
            .map(|session| session.id)
            .collect();
        if ids.is_empty() {
            self.phase = RolePhase::Standby;
            self.told.clear();
            return Vec::new();
        }
        self.phase = RolePhase::Moving;
        self.told = ids.clone();
        ids
    }

    /// That session is now on the new primary. The last one finishes the role change.
    pub fn followed(&mut self, sessions: &mut [LiveSession], id: u64) -> bool {
        if let Some(session) = sessions.iter_mut().find(|session| session.id == id) {
            if matches!(
                hop_session(session.path, MoveCause::RoleChange, &self.primary),
                SessionHop::ToPrimary { .. }
            ) {
                session.server = self.primary.clone();
            }
        }
        self.told.retain(|told| *told != id);
        if self.phase == RolePhase::Moving && self.told.is_empty() {
            self.phase = RolePhase::Standby;
        }
        self.phase == RolePhase::Standby
    }

    pub fn finished(&self) -> bool {
        self.phase == RolePhase::Standby
    }

    /// True when every relayed session already runs on the new primary, so
    /// stopping the old server does not end it.
    pub fn survives_old_shutdown(&self, sessions: &[LiveSession]) -> bool {
        self.finished()
            && sessions
                .iter()
                .filter(|session| session.path == MediaPath::Relayed)
                .all(|session| session.server == self.primary)
    }
}

/// A standby does not keep relay routes.
pub fn standby_relays(phase: RolePhase, relayed_here: usize) -> bool {
    phase == RolePhase::Standby && relayed_here > 0
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

/// How long a planned update or restart asks the standby to keep waiting.
pub const MAINTENANCE_HOLD_SECS: u64 = 15 * 60;

/// This server is the primary for the current generation.
///
/// A server that stepped down stays standby. Once a generation is recorded,
/// a server that still wants to be primary does not accept hosts when a
/// different instance owns that generation. A generation of zero keeps the
/// older desired-role rule so a server that has not fenced yet is not locked out.
pub fn generation_is_primary(
    desired: &str,
    epoch: u64,
    active_instance: &str,
    mine: &str,
    fenced_epoch: u64,
    fenced_instance: &str,
    connected: bool,
) -> bool {
    if epoch_fences(epoch, fenced_epoch, fenced_instance, mine) {
        return false;
    }
    if desired == "standby" {
        return false;
    }
    if epoch > 0 && !active_instance.is_empty() && active_instance != mine {
        return false;
    }
    if desired == "active" {
        return true;
    }
    if !active_instance.is_empty() && active_instance == mine {
        return true;
    }
    if !active_instance.is_empty() {
        return false;
    }
    connected
}

pub fn same_computer_name(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// True when a health row is this computer, including an older instance id.
pub fn is_same_party(instance: &str, name: &str, mine_instance: &str, mine_name: &str) -> bool {
    (!instance.is_empty() && instance == mine_instance) || same_computer_name(name, mine_name)
}

/// A different computer that published a version. An old row for this computer
/// is not the peer, and a row with no version is not shown.
pub fn counts_as_other_server(instance: &str, name: &str, version: &str, mine_instance: &str, mine_name: &str) -> bool {
    !is_same_party(instance, name, mine_instance, mine_name) && !version.trim().is_empty()
}

/// Role word for the other server, from this server's generation.
pub fn peer_role_label(we_are_primary: bool) -> &'static str {
    if we_are_primary { "standby" } else { "active" }
}

/// Cloud health plus a maintenance hold. A primary that announced a planned
/// restart stays "fresh" until that deadline, even when its last stamp is old.
pub fn confirming_path(read_ok: bool, newest_other_at: Option<u64>, maintenance_until: u64, now: u64) -> SecondPath {
    if maintenance_until > now {
        return SecondPath::Fresh;
    }
    second_path(read_ok, newest_other_at, now)
}

/// Remember an unhealthy primary until this instant.
pub fn unhealthy_hold_until(at: u64) -> u64 {
    at.saturating_add(MAINTENANCE_HOLD_SECS)
}

/// This computer's row is online when it is registered, or when the current
/// primary says it is.
pub fn own_row_online(registered: bool, primary_says_online: bool) -> bool {
    registered || primary_says_online
}

/// The account's published primary replaces a saved hostname when it names a
/// different server. The same endpoint keeps a hostname over a raw IP.
pub fn follow_primary(current: &str, published: &str) -> String {
    let published = published.trim();
    let current = current.trim();
    if published.is_empty() {
        return current.to_string();
    }
    if same_server(current, published) && host_is_name(current) && !host_is_name(published) {
        return current.to_string();
    }
    published.to_string()
}

fn host_is_name(address: &str) -> bool {
    let (host, _) = host_port(address);
    !host.is_empty() && host.parse::<std::net::IpAddr>().is_err()
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

    #[test]
    fn the_generation_owner_is_primary_and_a_stepped_down_server_stays_standby() {
        assert!(generation_is_primary("active", 0, "other", "mine", 0, "", false));
        assert!(!generation_is_primary("active", 5, "other", "mine", 0, "", true));
        assert!(generation_is_primary("active", 5, "mine", "mine", 0, "", false));
        assert!(!generation_is_primary("standby", 5, "mine", "mine", 0, "", true));
        assert!(generation_is_primary("", 4, "mine", "mine", 0, "", false));
        assert!(!generation_is_primary("", 0, "other", "mine", 0, "", true));
        assert!(generation_is_primary("", 0, "", "mine", 0, "", true));
        assert!(!generation_is_primary("", 0, "", "mine", 0, "", false));
        assert!(!generation_is_primary("active", 4, "mine", "mine", 5, "other", true));
    }

    #[test]
    fn the_peer_is_a_different_computer_and_a_planned_restart_stays_fresh() {
        assert!(same_computer_name("CHIP20", "chip20"));
        assert!(is_same_party("old-instance", "BENLENOVODESK", "new-instance", "benlenovodesk"));
        assert!(!counts_as_other_server("old", "CHIP20", "0.6.40", "new", "CHIP20"));
        assert!(!counts_as_other_server("other", "LENOVO", "", "mine", "CHIP20"));
        assert!(counts_as_other_server("other", "BENLENOVODESK", "0.6.44", "mine", "CHIP20"));
        assert_eq!(peer_role_label(true), "standby");
        assert_eq!(peer_role_label(false), "active");
        assert_eq!(confirming_path(true, Some(1_000), 2_000, 1_050), SecondPath::Fresh);
        assert_eq!(confirming_path(true, Some(1_000), 0, 1_020), SecondPath::Stale);
        assert_eq!(unhealthy_hold_until(1_000), 1_000 + MAINTENANCE_HOLD_SECS);
    }

    #[test]
    fn a_new_primary_address_replaces_the_old_name_and_this_computer_is_online_when_registered() {
        assert_eq!(follow_primary("pjdesk.spader.us:21200", "204.248.122.44:21200"), "204.248.122.44:21200");
        assert_eq!(follow_primary("pjdesk.spader.us:21200", ""), "pjdesk.spader.us:21200");
        assert_eq!(follow_primary("pjdesk.spader.us:21200", "pjdesk.spader.us"), "pjdesk.spader.us");
        assert!(own_row_online(true, false));
        assert!(own_row_online(false, true));
        assert!(!own_row_online(false, false));
    }

    #[test]
    fn demote_moves_relayed_sessions_before_step_down_and_shutdown_leaves_them_on_the_primary() {
        let primary = "203.0.113.10:21200";
        let mut sessions = vec![
            LiveSession { id: 1, path: MediaPath::Relayed, server: "here".into() },
            LiveSession { id: 2, path: MediaPath::Direct, server: "here".into() },
            LiveSession { id: 3, path: MediaPath::HolePunched, server: "here".into() },
        ];
        let mut change = RoleChange::new(primary);

        let told = change.begin(&sessions);
        assert_eq!(told, vec![1], "only the relayed session is told to move");
        assert_eq!(change.phase, RolePhase::Moving);
        assert!(!change.finished(), "step-down waits until that session is on the new primary");
        assert_eq!(sessions[0].server, "here", "the move is not finished at the moment of the tell");
        assert!(!change.survives_old_shutdown(&sessions), "shutting down now would still drop the relay");

        assert!(change.followed(&mut sessions, 1));
        assert!(change.finished());
        assert_eq!(sessions[0].server, primary);
        assert_eq!(sessions[1].server, "here", "a direct session never used the relay");
        assert_eq!(sessions[2].server, "here");
        assert!(!standby_relays(change.phase, 0));
        assert!(standby_relays(RolePhase::Standby, 1));

        // The old server can stop. The session is already on the new primary.
        assert!(change.survives_old_shutdown(&sessions));
        assert_eq!(
            sessions.iter().filter(|session| session.path == MediaPath::Relayed && session.server == "here").count(),
            0
        );

        // A crash, with no role change, still reconnects through the primary.
        assert_eq!(
            hop_session(MediaPath::Relayed, MoveCause::ServerGone, primary),
            SessionHop::ToPrimary { address: primary.into(), cause: MoveCause::ServerGone }
        );
        assert!(matches!(hop_session(MediaPath::Lan, MoveCause::RoleChange, primary), SessionHop::Keep));
        assert_eq!(primary_from_resume(&relay_resume_message(primary)), primary);
        assert!(is_relay_resume(&relay_resume_message(primary)));
        assert_eq!(
            move_target(&["203.0.113.10:21200".into()], &["192.0.2.5:21200".into()], &["pjdesk.example".into()]),
            "203.0.113.10:21200"
        );
    }
}
