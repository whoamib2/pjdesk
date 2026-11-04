//! Plain session-state notices (lock screen, Remote Desktop, tscon, no
//! monitor). These are information only: the viewer shows them as a short,
//! fading banner, never a dialog, and never ends the session over one.
//! The wording avoids "disconnected" and "stopped sharing" so viewers older
//! than 0.6.87 also keep the session open.

/// What the host can see about its Windows session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    NoDesktop,
    RemoteDesktop,
    NotOnScreen,
    OtherSession,
    NoMonitor,
    SignInScreen,
    Back,
}

/// One plain sentence naming the computer.
pub fn text(computer: &str, state: SessionState) -> String {
    let pc = if computer.trim().is_empty() { "The other computer" } else { computer.trim() };
    match state {
        SessionState::NoDesktop => format!("{pc}: PJDesk runs without a desktop here. Install the PJDesk service."),
        SessionState::RemoteDesktop => format!("{pc} is being used through Remote Desktop, so its own screen isn't shown."),
        SessionState::NotOnScreen => format!(
            "{pc} isn't showing its Windows session (Remote Desktop was closed). It comes back when someone signs in there."
        ),
        SessionState::OtherSession => format!("{pc} is at the Windows sign-in screen or another user's session."),
        SessionState::NoMonitor => format!("{pc} has no monitor on. Connect one or an HDMI dummy plug."),
        SessionState::SignInScreen => format!("{pc} is at the Windows sign-in screen."),
        SessionState::Back => format!("{pc}'s screen is back."),
    }
}

/// True for notices that only describe the host's session state, from this
/// build or from 0.6.86 hosts ("Remote session: …").
pub fn is_session_state(notice: &str) -> bool {
    let n = notice.trim();
    if n.starts_with("Remote session:") || n == "The remote session is back on the screen." {
        return true;
    }
    [
        ": PJDesk runs without a desktop here.",
        " is being used through Remote Desktop,",
        " isn't showing its Windows session",
        " is at the Windows sign-in screen",
        " has no monitor on.",
        "'s screen is back.",
    ]
    .iter()
    .any(|marker| n.contains(marker))
}

/// Seconds a session-state banner stays up.
pub const BANNER_SECS: u64 = 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_is_recognised_and_never_ends_a_session() {
        for s in [
            SessionState::NoDesktop,
            SessionState::RemoteDesktop,
            SessionState::NotOnScreen,
            SessionState::OtherSession,
            SessionState::NoMonitor,
            SessionState::SignInScreen,
            SessionState::Back,
        ] {
            let t = text("CABINSIGHTHOUND", s);
            assert!(is_session_state(&t), "{t}");
            let lower = t.to_ascii_lowercase();
            assert!(!lower.contains("disconnected") && !lower.contains("stopped sharing"), "{t}");
        }
        assert_eq!(text("CABINSIGHTHOUND", SessionState::SignInScreen), "CABINSIGHTHOUND is at the Windows sign-in screen.");
    }

    #[test]
    fn old_host_wording_is_session_state() {
        assert!(is_session_state("Remote session: the Windows session (session 3) is disconnected, usually after Remote Desktop was closed"));
        assert!(is_session_state("The remote session is back on the screen."));
        assert!(!is_session_state("The person at this computer disconnected"));
        assert!(!is_session_state("wrong password"));
    }
}
