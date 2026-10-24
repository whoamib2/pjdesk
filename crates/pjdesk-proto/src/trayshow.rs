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

/// No live window means the tray starts the program. A destroyed window
/// (a handle that is no longer a window) is not live.
pub fn should_launch(window_alive: bool) -> bool {
    !window_alive
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
        assert!(should_launch(false));
        assert!(!should_launch(true));
    }
}
