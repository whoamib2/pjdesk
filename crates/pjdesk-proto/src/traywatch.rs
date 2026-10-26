//! Whether a tray open was answered by the window.
//!
//! The Windows apps keep the clock. These checks stay here so a frozen
//! window can be told from one that is still running, without Windows.

/// How long the window has to answer a tray open before it is replaced.
pub const ANSWER_WAIT_MS: u64 = 2_000;

/// The window was asked to open and has not said it is still running.
pub fn unanswered(open_gen: u64, ack_gen: u64) -> bool {
    open_gen != 0 && ack_gen < open_gen
}

/// Only the newest unanswered open replaces the window. An older click
/// leaves that to the later one, so two clicks do not start two windows.
pub fn should_replace(click_gen: u64, open_gen: u64, ack_gen: u64) -> bool {
    click_gen != 0 && click_gen == open_gen && unanswered(open_gen, ack_gen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_window_is_not_replaced() {
        assert!(!unanswered(0, 0));
        assert!(!should_replace(0, 0, 0));
    }

    #[test]
    fn an_open_with_no_answer_is_replaced_once() {
        assert!(unanswered(1, 0));
        assert!(should_replace(1, 1, 0));
        assert!(!should_replace(1, 2, 0));
        assert_eq!(ANSWER_WAIT_MS, 2_000);
    }

    #[test]
    fn an_answer_and_an_older_click_do_not_replace() {
        assert!(!unanswered(2, 2));
        assert!(!should_replace(2, 2, 2));
        assert!(!should_replace(1, 2, 1));
        assert!(should_replace(3, 3, 2));
    }

    #[test]
    fn close_and_reopen_can_repeat_when_the_window_answers() {
        let mut open = 0u64;
        let mut ack = 0u64;
        for _ in 0..20 {
            open += 1;
            assert!(should_replace(open, open, ack));
            ack = open;
            assert!(!should_replace(open, open, ack));
        }
    }
}
