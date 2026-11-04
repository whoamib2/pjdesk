//! Where injected input goes, and how it keeps working across session changes.
//!
//! Every `SendInput` from the host goes through one dedicated "pjdesk-input"
//! thread (see [`submit`]). That thread:
//!
//! * re-attaches to the *input desktop* whenever it changes (Default ↔
//!   Winlogon for lock/UAC/sign-in, and after `tscon`/RDP moves the console),
//!   checked at most every [`DESK_RECHECK`] and immediately after any failure;
//! * retries a rejected batch once after re-attaching, so clicks, button-ups
//!   and key-ups are not lost across a switch;
//! * forwards to the SYSTEM input agent (`__input-agent`, started by the
//!   service in the same console session) when one is listening, so elevated
//!   windows (UIPI) and the secure desktop accept input; it falls back to
//!   local injection when the agent is missing and reconnects automatically;
//! * self-checks absolute moves with `GetCursorPos` and reports when input is
//!   being swallowed (e.g. an elevated window with no agent), see [`SelfCheck`].
//!
//! Nothing here needs the viewer to reconnect.

use std::time::{Duration, Instant};

/// How often the input thread compares the input desktop with the one it is on.
pub const DESK_RECHECK: Duration = Duration::from_millis(200);
/// How long to wait before trying the SYSTEM agent pipe again.
pub const AGENT_RETRY: Duration = Duration::from_secs(2);

/// One `INPUT` record in a form that crosses a pipe. Mouse: a=dx b=dy c=data
/// d=flags. Keyboard: a=vk b=scan c=flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rec {
    pub kind: u8,
    pub a: i32,
    pub b: i32,
    pub c: u32,
    pub d: u32,
}

pub const REC_MOUSE: u8 = 0;
pub const REC_KEYBOARD: u8 = 1;
/// Not an INPUT: asks the SYSTEM agent for the secure attention sequence
/// (Ctrl+Alt+Del). Only SYSTEM may call SendSAS; the session helper can't.
pub const REC_SAS: u8 = 2;
const REC_LEN: usize = 17;
/// The most records one request may carry (Unicode text is chunked per char).
pub const MAX_RECS: usize = 64;

/// `[count u8][count × 17 bytes]`.
pub fn encode(recs: &[Rec]) -> Vec<u8> {
    let n = recs.len().min(MAX_RECS);
    let mut out = Vec::with_capacity(1 + n * REC_LEN);
    out.push(n as u8);
    for r in &recs[..n] {
        out.push(r.kind);
        out.extend_from_slice(&r.a.to_le_bytes());
        out.extend_from_slice(&r.b.to_le_bytes());
        out.extend_from_slice(&r.c.to_le_bytes());
        out.extend_from_slice(&r.d.to_le_bytes());
    }
    out
}

/// Bytes that follow the count byte for `count` records.
pub fn body_len(count: u8) -> Option<usize> {
    let n = count as usize;
    (n > 0 && n <= MAX_RECS).then_some(n * REC_LEN)
}

pub fn decode(body: &[u8]) -> Option<Vec<Rec>> {
    if body.is_empty() || body.len() % REC_LEN != 0 || body.len() / REC_LEN > MAX_RECS {
        return None;
    }
    let i32_at = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let u32_at = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    body.chunks_exact(REC_LEN)
        .map(|c| {
            let kind = c[0];
            (kind == REC_MOUSE || kind == REC_KEYBOARD || kind == REC_SAS).then(|| Rec {
                kind,
                a: i32_at(&c[1..]),
                b: i32_at(&c[5..]),
                c: u32_at(&c[9..]),
                d: u32_at(&c[13..]),
            })
        })
        .collect()
}

/// Reply: `[accepted u8][last_error u32]`.
pub fn sas_request() -> Vec<Rec> {
    vec![Rec { kind: REC_SAS, a: 0, b: 0, c: 0, d: 0 }]
}

/// True when the request is the secure attention sequence.
pub fn is_sas(recs: &[Rec]) -> bool {
    recs.len() == 1 && recs[0].kind == REC_SAS
}

/// Windows virtual keys for scan codes Windows' shell matches by virtual key
/// (Win and Menu keys): a scan-code-only Win key did not open Start or run
/// Win+D/Win+E reliably.
pub fn vk_for_scan(code: u16) -> Option<u16> {
    match code {
        0xE05B => Some(0x5B), // VK_LWIN
        0xE05C => Some(0x5C), // VK_RWIN
        0xE05D => Some(0x5D), // VK_APPS
        _ => None,
    }
}

pub fn encode_reply(accepted: bool, err: u32) -> [u8; 5] {
    let e = err.to_le_bytes();
    [accepted as u8, e[0], e[1], e[2], e[3]]
}

/// Pipe the agent for `session` listens on.
pub fn agent_pipe_name(session: u32) -> String {
    format!(r"\\.\pipe\PJDeskInput-{session}")
}

/// Decides when the input thread must re-attach to the input desktop.
#[derive(Debug, Default)]
pub struct DeskFollower {
    attached: String,
    checked: Option<Instant>,
    pub reattaches: u64,
}

impl DeskFollower {
    /// Before sending: true if the thread should (re-)attach now. `name` reads
    /// the current input desktop; it is only called when a recheck is due.
    pub fn should_attach(&mut self, now: Instant, name: impl FnOnce() -> Option<String>) -> bool {
        if self.checked.is_some_and(|t| now.duration_since(t) < DESK_RECHECK) && !self.attached.is_empty() {
            return false;
        }
        self.checked = Some(now);
        match name() {
            // Can't read it (mid-switch): keep what we have, try again later.
            None => self.attached.is_empty(),
            Some(current) if current.is_empty() => false,
            Some(current) => current != self.attached,
        }
    }

    /// Record a (re-)attach to `name` (empty if unknown).
    pub fn attached(&mut self, now: Instant, name: Option<String>) {
        self.attached = name.unwrap_or_default();
        self.checked = Some(now);
        self.reattaches += 1;
    }

    /// A send failed: recheck on the next send no matter what.
    pub fn invalidate(&mut self) {
        self.attached.clear();
        self.checked = None;
    }

    pub fn current(&self) -> &str {
        &self.attached
    }
}

/// Watches whether absolute moves actually land.
#[derive(Debug, Default)]
pub struct SelfCheck {
    misses: u32,
    reported: bool,
}

/// What the input thread should do after a move check.
#[derive(Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    Fine,
    /// A few misses in a row: re-attach and reconnect the agent.
    Recover,
    /// Still not landing after recovery: tell the viewer once.
    Report,
}

/// Misses in a row before recovery, and before reporting.
pub const MISSES_TO_RECOVER: u32 = 3;
pub const MISSES_TO_REPORT: u32 = 12;

impl SelfCheck {
    /// `want` and `got` are pixel positions; a move within 2 px counts (DPI
    /// rounding), as does `got == None` (cursor position unreadable, e.g. on
    /// the secure desktop without the agent — that is not a miss we can judge).
    pub fn observe(&mut self, want: (i32, i32), got: Option<(i32, i32)>) -> CheckOutcome {
        let landed = match got {
            None => true,
            Some((x, y)) => (x - want.0).abs() <= 2 && (y - want.1).abs() <= 2,
        };
        if landed {
            self.misses = 0;
            self.reported = false;
            return CheckOutcome::Fine;
        }
        self.misses += 1;
        if self.misses >= MISSES_TO_REPORT && !self.reported {
            self.reported = true;
            CheckOutcome::Report
        } else if self.misses == MISSES_TO_RECOVER {
            CheckOutcome::Recover
        } else {
            CheckOutcome::Fine
        }
    }
}

/// Set by the input thread when input stops landing; read by the session so
/// the viewer gets one clear notice.
pub static INPUT_PROBLEM: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn take_problem() -> Option<String> {
    INPUT_PROBLEM.lock().ok()?.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_the_pipe_format() {
        let recs = vec![
            Rec { kind: REC_MOUSE, a: -5, b: 65535, c: 120, d: 0x8001 },
            Rec { kind: REC_KEYBOARD, a: 0, b: 0x1E, c: 8, d: 0 },
        ];
        let bytes = encode(&recs);
        assert_eq!(bytes[0], 2);
        assert_eq!(body_len(bytes[0]), Some(bytes.len() - 1));
        assert_eq!(decode(&bytes[1..]).unwrap(), recs);
    }

    #[test]
    fn malformed_requests_are_rejected() {
        assert_eq!(body_len(0), None);
        assert_eq!(body_len(MAX_RECS as u8 + 1), None);
        assert!(decode(&[]).is_none());
        assert!(decode(&[0u8; 16]).is_none());
        let mut bad = encode(&[Rec { kind: REC_MOUSE, a: 0, b: 0, c: 0, d: 0 }]);
        bad[1] = 9;
        assert!(decode(&bad[1..]).is_none());
    }

    #[test]
    fn follower_attaches_first_then_only_on_change() {
        let t0 = Instant::now();
        let mut f = DeskFollower::default();
        assert!(f.should_attach(t0, || Some("Default".into())));
        f.attached(t0, Some("Default".into()));
        // Within the recheck window nothing is read.
        assert!(!f.should_attach(t0 + Duration::from_millis(10), || panic!("not due")));
        let t1 = t0 + DESK_RECHECK;
        assert!(!f.should_attach(t1, || Some("Default".into())));
        // Lock screen / UAC / tscon: the input desktop changes.
        let t2 = t1 + DESK_RECHECK;
        assert!(f.should_attach(t2, || Some("Winlogon".into())));
        f.attached(t2, Some("Winlogon".into()));
        let t3 = t2 + DESK_RECHECK;
        assert!(f.should_attach(t3, || Some("Default".into())));
    }

    #[test]
    fn follower_rechecks_immediately_after_a_failure() {
        let t0 = Instant::now();
        let mut f = DeskFollower::default();
        f.attached(t0, Some("Default".into()));
        f.invalidate();
        assert!(f.should_attach(t0, || Some("Default".into())));
    }

    #[test]
    fn follower_keeps_its_desktop_while_the_name_is_unreadable() {
        let t0 = Instant::now();
        let mut f = DeskFollower::default();
        f.attached(t0, Some("Default".into()));
        assert!(!f.should_attach(t0 + DESK_RECHECK, || None));
        // Never attached yet: try anyway.
        let mut g = DeskFollower::default();
        assert!(g.should_attach(t0, || None));
    }

    #[test]
    fn self_check_recovers_then_reports_once_then_rearms() {
        let mut c = SelfCheck::default();
        assert_eq!(c.observe((10, 10), Some((11, 9))), CheckOutcome::Fine);
        let mut outcomes = Vec::new();
        for _ in 0..MISSES_TO_REPORT + 3 {
            outcomes.push(c.observe((100, 100), Some((0, 0))));
        }
        assert_eq!(outcomes.iter().filter(|o| **o == CheckOutcome::Recover).count(), 1);
        assert_eq!(outcomes.iter().filter(|o| **o == CheckOutcome::Report).count(), 1);
        assert_eq!(c.observe((5, 5), Some((5, 5))), CheckOutcome::Fine);
        assert_eq!(c.observe((5, 5), None), CheckOutcome::Fine);
    }
}

#[cfg(test)]
mod sas_tests {
    use super::*;

    #[test]
    fn sas_crosses_the_pipe() {
        let recs = sas_request();
        let body = encode(&recs);
        let back = decode(&body[1..]).unwrap();
        assert!(is_sas(&back));
        assert!(!is_sas(&[Rec { kind: REC_KEYBOARD, a: 0, b: 0, c: 0, d: 0 }]));
    }

    #[test]
    fn win_keys_use_virtual_keys() {
        assert_eq!(vk_for_scan(0xE05B), Some(0x5B));
        assert_eq!(vk_for_scan(0xE05C), Some(0x5C));
        assert_eq!(vk_for_scan(0x0020), None);
    }
}
