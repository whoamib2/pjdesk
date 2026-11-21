//! Allow / Decline prompt: one request per viewer, and a single wait.
//!
//! The person at the host gets one popup. The viewer waits for that answer
//! and must not start another dial while the prompt is up. Decline or a
//! timeout is final: automatic retries stop.

use std::time::Duration;

/// How long the host shows Allow / Decline, and how long the viewer waits.
pub const WAIT: Duration = Duration::from_secs(60);

/// Status the viewer shows instead of starting another connection.
pub const WAITING: &str = "Waiting for the other computer to accept";

/// The two `from` labels name the same viewer (account and computer).
pub fn same_viewer(pending_from: &str, new_from: &str) -> bool {
    let a = pending_from.trim();
    let b = new_from.trim();
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// A second request from the same viewer should keep the existing popup and
/// any Allow already clicked. A different viewer starts a new prompt.
pub fn reuse_pending(pending_from: &str, new_from: &str) -> bool {
    same_viewer(pending_from, new_from)
}

/// The person clicked Decline, or the prompt ended without Allow.
pub fn is_final_verdict(reason: &str) -> bool {
    is_decline(reason) || is_timeout(reason)
}

/// The person at the other computer (or phone) refused the session.
pub fn is_decline(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("declined") || lower.contains("not allowed") || lower.contains("stopped sharing")
}

/// The Allow prompt ran out of time, or nobody was there to click it.
///
/// Does not match a dropped route such as "Disconnected: timed out".
pub fn is_timeout(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("no one is available to accept")
        || lower.contains("no one at that phone answered")
        || lower.contains("no one at the other computer")
        || lower.contains("did not accept in time")
    {
        return true;
    }
    let trimmed = lower.trim().trim_end_matches('.');
    trimmed == "no answer" || trimmed.ends_with(" no answer") || trimmed.ends_with(": no answer")
}

/// Hide the popup once Allow or Decline is on file, even if waiters remain.
pub fn prompt_is_open(has_pending: bool, has_reply: bool) -> bool {
    has_pending && !has_reply
}

/// The viewer must not start another Connect while Allow is up, or after a
/// Decline or timeout.
pub fn may_redial(waiting_for_accept: bool, last_error: Option<&str>) -> bool {
    if waiting_for_accept {
        return false;
    }
    match last_error {
        Some(reason) if is_final_verdict(reason) => false,
        _ => true,
    }
}

/// The leftover prompt can go away: no one is waiting, no answer, and the
/// wait has elapsed.
pub fn stale_prompt(waiters: u32, has_reply: bool, elapsed: Duration) -> bool {
    waiters == 0 && !has_reply && elapsed >= WAIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_request_from_the_same_viewer_reuses_the_popup() {
        assert!(same_viewer("ben@spader.us — CABINDESK", "ben@spader.us — CABINDESK"));
        assert!(reuse_pending("ben@spader.us — CABINDESK", "BEN@spader.us — CABINDESK"));
        assert!(!reuse_pending("ben@spader.us — CABINDESK", "ben@spader.us — LENOVO"));
        assert!(!reuse_pending("", "CABINDESK"));
        assert!(!reuse_pending("CABINDESK", ""));
    }

    #[test]
    fn decline_and_timeout_are_final_and_a_dropped_route_is_not() {
        assert!(is_final_verdict("the person at this computer declined"));
        assert!(is_decline("the person declined"));
        assert!(is_timeout("No answer"));
        assert!(is_timeout("No one is available to accept"));
        assert!(is_timeout("Timed out. No one at that phone answered the request to view the screen."));
        assert!(is_timeout("The other computer did not accept in time."));
        assert!(!is_timeout("Disconnected: timed out"));
        assert!(!is_timeout("the primary did not answer"));
        assert!(!is_timeout("the host did not finish setting up the connection"));
        assert!(!is_final_verdict("session lost: the control stream closed"));
    }

    #[test]
    fn the_popup_hides_after_an_answer_and_a_stale_prompt_expires() {
        assert!(prompt_is_open(true, false));
        assert!(!prompt_is_open(true, true), "Allow or Decline already recorded");
        assert!(!prompt_is_open(false, false));
        assert!(stale_prompt(0, false, WAIT));
        assert!(!stale_prompt(1, false, WAIT), "a waiter still owns the prompt");
        assert!(!stale_prompt(0, true, WAIT), "an answer is still needed");
        assert!(!stale_prompt(0, false, WAIT - Duration::from_secs(1)));
    }

    #[test]
    fn the_viewer_status_names_the_wait_and_is_long_enough_for_allow() {
        assert_eq!(WAITING, "Waiting for the other computer to accept");
        assert_eq!(WAIT, Duration::from_secs(60));
    }

    #[test]
    fn the_viewer_does_not_redial_while_waiting_or_after_a_final_verdict() {
        assert!(!may_redial(true, None));
        assert!(may_redial(false, None));
        assert!(!may_redial(false, Some("No answer")));
        assert!(!may_redial(false, Some("the person at this computer declined")));
        assert!(may_redial(false, Some("Disconnected: timed out")));
    }
}
