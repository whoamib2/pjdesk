//! What a tray click should do, and where an off-screen window belongs.
//!
//! The Windows apps perform the actual `ShowWindow` / `SetForegroundWindow`
//! calls. These decisions stay here so they can be tested without Windows.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Left,
    LeftDouble,
    Right,
    Other,
}

/// A single left click and a double-click both open the window.
/// A right click keeps the menu.
pub fn opens_window(click: Click) -> bool {
    matches!(click, Click::Left | Click::LeftDouble)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn intersects(a: Rect, b: Rect) -> bool {
    a.w > 0
        && a.h > 0
        && b.w > 0
        && b.h > 0
        && a.x < b.x.saturating_add(b.w)
        && b.x < a.x.saturating_add(a.w)
        && a.y < b.y.saturating_add(b.h)
        && b.y < a.y.saturating_add(a.h)
}

/// True when any part of the window lies on a connected monitor.
pub fn on_any_monitor(window: Rect, monitors: &[Rect]) -> bool {
    monitors.iter().copied().any(|monitor| intersects(window, monitor))
}

/// Center `window` in the work area. The size stays. The title bar stays
/// inside the work area when the window is larger than the screen.
pub fn place_on_work_area(window: Rect, work: Rect) -> Rect {
    let w = window.w.max(1);
    let h = window.h.max(1);
    let mut x = work.x + (work.w - w) / 2;
    let mut y = work.y + (work.h - h) / 2;
    let right_limit = work.x + work.w - 48;
    let bottom_limit = work.y + work.h - 48;
    if x > right_limit {
        x = right_limit;
    }
    if y > bottom_limit {
        y = bottom_limit;
    }
    if x < work.x {
        x = work.x;
    }
    if y < work.y {
        y = work.y;
    }
    Rect { x, y, w, h }
}

/// A minimized or hidden window has to be restored before it can be seen.
/// The PIN lock is drawn in that window, so the lock still needs the restore.
pub fn needs_restore(minimized: bool, visible: bool) -> bool {
    minimized || !visible
}

/// Hide-to-tray must not undo the open that just happened.
pub fn hide_because_minimized(minimized: bool, opening: bool) -> bool {
    minimized && !opening
}

/// Older decision: cancel the close and minimize. The apps no longer do
/// that. A minimized window is still a surface, and presenting into it
/// stops the message loop. [`destroy_window_on_close`] is the close button.
pub fn minimize_on_close(has_tray: bool, really_quit: bool, just_opened: bool) -> bool {
    has_tray && !really_quit && !just_opened
}

/// The close button destroys the eframe window and leaves the process in
/// the tray. Quit ends the process. With no tray icon, the close ends it.
/// The window is not minimized, so nothing is presented while it is parked.
pub fn destroy_window_on_close(has_tray: bool, really_quit: bool) -> bool {
    has_tray && !really_quit
}

/// How often a parked process says the loop is still alive.
pub const PARKED_ALIVE_MS: u64 = 2_000;

/// True when the last alive line is old enough to write another.
pub fn parked_alive_due(elapsed_ms: u64) -> bool {
    elapsed_ms >= PARKED_ALIVE_MS
}

/// One tray open per second. A double-click is two opens about 90 ms apart,
/// and the second one must not start another window.
pub const OPEN_DEBOUNCE_MS: u64 = 1_000;

/// `last_ms` is `None` before the first open.
pub fn open_allowed(last_ms: Option<u64>, now_ms: u64) -> bool {
    match last_ms {
        None => true,
        Some(last) => now_ms.saturating_sub(last) >= OPEN_DEBOUNCE_MS,
    }
}

/// What the tray process does with a click. The tray never draws the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayOpen {
    /// A click already opened a window in the last second.
    Ignore,
    /// A GUI process is up. Bring that window forward.
    Signal,
    /// The GUI process is hung. End it and start another.
    Replace,
    /// No GUI process. Start `exe --window`.
    Spawn,
}

/// A second click inside one second is ignored. A hung window is replaced.
/// Otherwise an existing GUI is signaled, or a new one is started.
pub fn tray_open(now_ms: u64, last_open_ms: Option<u64>, gui_running: bool, gui_hung: bool) -> TrayOpen {
    if !open_allowed(last_open_ms, now_ms) {
        return TrayOpen::Ignore;
    }
    if gui_hung {
        TrayOpen::Replace
    } else if gui_running {
        TrayOpen::Signal
    } else {
        TrayOpen::Spawn
    }
}

/// No live window means the tray starts the program. A destroyed window
/// (a handle that is no longer a window) is not live.
pub fn should_launch(window_alive: bool) -> bool {
    !window_alive
}

/// What a tray click does with the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAction {
    /// The window is still there. Restore it and bring it forward.
    Restore,
    /// This process still owns the single window. Show that window.
    /// Starting another copy would hit the same lock and exit.
    ShowThisProcess,
    /// The window process is gone. Start the program.
    Launch,
}

impl OpenAction {
    /// Short text for the tray log. No secrets.
    pub fn as_log(self) -> &'static str {
        match self {
            OpenAction::Restore => "restore",
            OpenAction::ShowThisProcess => "show the window this process owns",
            OpenAction::Launch => "start because the window process is not running",
        }
    }
}

/// A live window is restored. The process that holds the single-instance
/// lock shows its own window. Only a process that does not hold that lock
/// starts a new one.
pub fn open_action(window_alive: bool, owns_gui: bool) -> OpenAction {
    if window_alive {
        OpenAction::Restore
    } else if owns_gui {
        OpenAction::ShowThisProcess
    } else {
        OpenAction::Launch
    }
}

/// Exact title, or a longer title that starts with `expected` and a separator.
/// "PJDesk Server" is not a match for "PJDesk".
pub fn title_matches(window_title: &str, expected: &str) -> bool {
    let window_title = window_title.trim_matches('\0').trim();
    let expected = expected.trim();
    if expected.is_empty() || window_title.is_empty() {
        return false;
    }
    if window_title == expected {
        return true;
    }
    let Some(rest) = window_title.strip_prefix(expected) else {
        return false;
    };
    if expected == "PJDesk" && rest.trim_start().starts_with("Server") {
        return false;
    }
    matches!(rest.chars().next(), Some(' ' | '-' | '—' | ':' | '·' | '('))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_click_and_double_click_open_the_window() {
        assert!(opens_window(Click::Left));
        assert!(opens_window(Click::LeftDouble));
        assert!(!opens_window(Click::Right));
        assert!(!opens_window(Click::Other));
    }

    #[test]
    fn a_disconnected_monitor_is_off_screen_and_the_window_moves_back() {
        let laptop = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let parked = Rect { x: -32000, y: -32000, w: 160, h: 28 };
        let on_missing = Rect { x: 3000, y: 80, w: 900, h: 700 };
        assert!(!on_any_monitor(parked, &[laptop]));
        assert!(!on_any_monitor(on_missing, &[laptop]));
        assert!(on_any_monitor(Rect { x: 100, y: 100, w: 800, h: 600 }, &[laptop]));

        let work = Rect { x: 0, y: 0, w: 1920, h: 1040 };
        let placed = place_on_work_area(on_missing, work);
        assert!(on_any_monitor(placed, &[laptop]));
        assert_eq!(placed.w, 900);
        assert_eq!(placed.h, 700);
        assert!(placed.x >= work.x && placed.y >= work.y);
    }

    #[test]
    fn the_lock_does_not_cancel_a_restore_and_a_dead_window_launches() {
        assert!(needs_restore(true, true));
        assert!(needs_restore(false, false));
        assert!(!needs_restore(false, true));
        assert!(!hide_because_minimized(true, true));
        assert!(hide_because_minimized(true, false));
        assert!(!hide_because_minimized(false, false));
        assert!(minimize_on_close(true, false, false));
        assert!(!minimize_on_close(true, true, false));
        assert!(!minimize_on_close(true, false, true));
        assert!(!minimize_on_close(false, false, false));
        assert!(destroy_window_on_close(true, false));
        assert!(!destroy_window_on_close(true, true));
        assert!(!destroy_window_on_close(false, false));
        assert!(!parked_alive_due(0));
        assert!(!parked_alive_due(PARKED_ALIVE_MS - 1));
        assert!(parked_alive_due(PARKED_ALIVE_MS));
        assert!(should_launch(false));
        assert!(!should_launch(true));
    }

    #[test]
    fn a_second_open_inside_one_second_is_ignored() {
        assert!(open_allowed(None, 0));
        assert!(!open_allowed(Some(1_000), 1_090));
        assert!(!open_allowed(Some(1_000), 1_999));
        assert!(open_allowed(Some(1_000), 2_000));
        assert_eq!(tray_open(1_090, Some(1_000), false, false), TrayOpen::Ignore);
        assert_eq!(tray_open(2_000, Some(1_000), false, false), TrayOpen::Spawn);
        assert_eq!(tray_open(2_000, Some(1_000), true, false), TrayOpen::Signal);
        assert_eq!(tray_open(2_000, Some(1_000), true, true), TrayOpen::Replace);
        assert_eq!(tray_open(2_000, Some(1_000), false, true), TrayOpen::Replace);
        assert_eq!(OPEN_DEBOUNCE_MS, 1_000);
    }

    #[test]
    fn a_hidden_window_is_restored_and_a_dead_owner_is_not_started_twice() {
        assert_eq!(open_action(true, true), OpenAction::Restore);
        assert_eq!(open_action(true, false), OpenAction::Restore);
        assert_eq!(open_action(false, true), OpenAction::ShowThisProcess);
        assert_eq!(open_action(false, false), OpenAction::Launch);
        assert!(should_launch(false));
        assert!(!should_launch(true));
        assert_eq!(OpenAction::Restore.as_log(), "restore");
        assert_eq!(OpenAction::ShowThisProcess.as_log(), "show the window this process owns");
        assert_eq!(OpenAction::Launch.as_log(), "start because the window process is not running");
    }

    #[test]
    fn the_client_title_does_not_match_the_server_window() {
        assert!(title_matches("PJDesk", "PJDesk"));
        assert!(title_matches("PJDesk - locked", "PJDesk"));
        assert!(!title_matches("PJDesk Server", "PJDesk"));
        assert!(!title_matches("PJDeskServer", "PJDesk"));
        assert!(title_matches("PJDesk Server", "PJDesk Server"));
        assert!(title_matches("PJDesk Server - status", "PJDesk Server"));
        assert!(!title_matches("", "PJDesk"));
        assert!(!title_matches("PJDesk", ""));
    }
}
