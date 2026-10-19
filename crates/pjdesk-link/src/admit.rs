//! Whether a password that already matched may open the session by itself.
//!
//! A valid permanent password starts the session with no Allow prompt when
//! policy allows password access. Unattended mode ("always allow the permanent
//! password") does the same for any valid password once a permanent password
//! is set. Anything else waits for the person at the computer.

use pjdesk_proto::perms::{self, grant};
use pjdesk_proto::signal::Policy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    /// Permission bits for a session that does not ask anyone.
    Open(u32),
    /// Show the on-device Allow prompt.
    Ask,
}

pub fn after_password(
    approval: &str,
    matched_permanent: bool,
    permanent_set: bool,
    policy: &Policy,
    view_only: bool,
) -> Admit {
    // File transfer stays inside the host's receive folder (the Desktop when
    // that folder was left blank). View-only still loses it via `grant`.
    let restricted = grant(perms::UNATTENDED_DEFAULT | perms::FILE, policy.max_permissions, view_only);
    let password_access = policy.unattended_allowed && !policy.force_prompt;
    if matched_permanent && password_access {
        return Admit::Open(restricted);
    }
    if approval == "unattended" && permanent_set && password_access {
        return Admit::Open(restricted);
    }
    Admit::Ask
}

/// `passwords` is the host's list for this attempt: one-time first, permanent second.
pub fn matched_permanent(passwords: &[String], slot: u8, permanent: Option<&str>) -> bool {
    let Some(permanent) = permanent.map(str::trim).filter(|p| !p.is_empty()) else {
        return false;
    };
    passwords.get(slot as usize).is_some_and(|pw| pw == permanent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_permanent_password_does_not_ask() {
        let policy = Policy::default();
        let passwords = vec!["once".into(), "permanent-secret".into()];
        assert!(matched_permanent(&passwords, 1, Some("permanent-secret")));
        assert!(!matched_permanent(&passwords, 0, Some("permanent-secret")));
        assert!(matches!(
            after_password("prompt", true, true, &policy, false),
            Admit::Open(_)
        ));
        assert!(matches!(
            after_password("unattended", true, true, &policy, false),
            Admit::Open(_)
        ));
    }

    #[test]
    fn unattended_mode_accepts_any_valid_password_and_prompt_mode_asks_for_the_one_time() {
        let policy = Policy::default();
        assert!(matches!(after_password("unattended", false, true, &policy, false), Admit::Open(_)));
        assert!(matches!(after_password("prompt", false, true, &policy, false), Admit::Ask));
        assert!(matches!(after_password("unattended", false, false, &policy, false), Admit::Ask));
        let mut forced = policy.clone();
        forced.force_prompt = true;
        assert!(matches!(after_password("unattended", true, true, &forced, false), Admit::Ask));
    }

    #[test]
    fn an_unattended_session_can_receive_files_unless_it_is_view_only() {
        let policy = Policy::default();
        match after_password("unattended", true, true, &policy, false) {
            Admit::Open(bits) => assert!(perms::has(bits, perms::FILE)),
            Admit::Ask => panic!("expected the session to open"),
        }
        match after_password("unattended", true, true, &policy, true) {
            Admit::Open(bits) => assert!(!perms::has(bits, perms::FILE)),
            Admit::Ask => panic!("expected the session to open"),
        }
    }
}
