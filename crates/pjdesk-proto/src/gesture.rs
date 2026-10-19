//! Pointer mapping and Android accessibility gestures.
//!
//! A remote picture is letterboxed inside the view. Touches in the bars are
//! not input. Everything that is input is normalized to the picture:
//! `0` is the left or top edge and `65535` is the right or bottom edge, the
//! same space as [`crate::peer::InputEvent::MouseMove`].
//!
//! Rotation is a new picture size. The host captures the display in its
//! current orientation, so a turn from portrait to landscape arrives as a
//! frame whose width and height have swapped. [`rotate_norm`] is the mapping
//! for a buffer that itself rotated while the view still has the old corners.

use crate::peer::InputEvent;

/// Tap. Point 0 is the contact.
pub const GESTURE_TAP: u8 = 0;
/// Long-press. Point 0 is the contact. `duration_ms` is how long it was held.
pub const GESTURE_LONG_PRESS: u8 = 1;
/// Swipe. Point 0 is the start, point 1 is the end.
pub const GESTURE_SWIPE: u8 = 2;
/// Pinch. Points 0 and 1 are finger A start/end. Points 2 and 3 are finger B.
pub const GESTURE_PINCH: u8 = 3;

/// Movement, in normalized units, that turns a press into a swipe.
/// About 1.2% of the picture, so a finger that barely shifts still taps.
pub const MOVE_THRESHOLD: u16 = 800;
/// A press held at least this long, without crossing [`MOVE_THRESHOLD`], is a long-press.
pub const LONG_PRESS_MS: u64 = 500;

/// The remote picture, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
}

/// The view the picture is drawn into, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub width: u32,
    pub height: u32,
}

/// Where the picture sits inside the view. `x` and `y` may be zero; the bars
/// are the view pixels outside this rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Letterbox {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Largest rectangle with the frame's aspect ratio that fits the view, centred.
pub fn letterbox(frame: Frame, view: View) -> Letterbox {
    if frame.width == 0 || frame.height == 0 || view.width == 0 || view.height == 0 {
        return Letterbox { x: 0, y: 0, width: 0, height: 0 };
    }
    let (width, height) = if frame.width as u64 * view.height as u64 <= view.width as u64 * frame.height as u64 {
        let h = view.height;
        let w = ((frame.width as u64 * h as u64) / frame.height as u64).clamp(1, view.width as u64) as u32;
        (w, h)
    } else {
        let w = view.width;
        let h = ((frame.height as u64 * w as u64) / frame.width as u64).clamp(1, view.height as u64) as u32;
        (w, h)
    };
    Letterbox { x: ((view.width - width) / 2) as i32, y: ((view.height - height) / 2) as i32, width, height }
}

/// A view pixel inside the picture, as a normalized coordinate.
/// `None` when the point is in a letterbox bar or the picture is empty.
pub fn view_to_norm(placed: Letterbox, x: f32, y: f32) -> Option<(u16, u16)> {
    if placed.width < 2 || placed.height < 2 {
        return None;
    }
    let right = placed.x as f32 + placed.width as f32;
    let bottom = placed.y as f32 + placed.height as f32;
    if x < placed.x as f32 || y < placed.y as f32 || x >= right || y >= bottom {
        return None;
    }
    let nx = ((x - placed.x as f32) / (placed.width as f32 - 1.0) * 65535.0).round();
    let ny = ((y - placed.y as f32) / (placed.height as f32 - 1.0) * 65535.0).round();
    Some((nx.clamp(0.0, 65535.0) as u16, ny.clamp(0.0, 65535.0) as u16))
}

/// A view-space delta, scaled by the picture rather than the whole view.
/// Moving across the picture's width is `65535` in x, whatever the bars.
pub fn view_delta_to_norm(placed: Letterbox, dx: f32, dy: f32) -> (i32, i32) {
    if placed.width < 2 || placed.height < 2 {
        return (0, 0);
    }
    let sx = dx / (placed.width as f32 - 1.0) * 65535.0;
    let sy = dy / (placed.height as f32 - 1.0) * 65535.0;
    (sx.round() as i32, sy.round() as i32)
}

/// Normalized coordinate to a pixel on an axis of `size` pixels.
pub fn norm_to_pixel(n: u16, size: u32) -> u32 {
    if size <= 1 {
        return 0;
    }
    ((n as u64 * (size as u64 - 1) + 32767) / 65535).min(size as u64 - 1) as u32
}

/// Rotate a normalized point 90 degrees clockwise, for a capture buffer that
/// turned while the controller still has the previous corners.
///
/// Image y grows downward, so the top-left corner `(0, 0)` becomes the
/// top-right of the new buffer.
pub fn rotate_norm_cw(x: u16, y: u16) -> (u16, u16) {
    (y, 65535u16.saturating_sub(x))
}

/// Rotate a normalized point 90 degrees counter-clockwise.
pub fn rotate_norm_ccw(x: u16, y: u16) -> (u16, u16) {
    (65535u16.saturating_sub(y), x)
}

/// One gesture ready to send as [`InputEvent::Gesture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gesture {
    pub kind: u8,
    pub duration_ms: u16,
    pub x: [u16; 4],
    pub y: [u16; 4],
}

impl Gesture {
    pub fn to_event(self) -> InputEvent {
        InputEvent::Gesture { kind: self.kind, duration_ms: self.duration_ms, x: self.x, y: self.y }
    }

    pub fn tap(x: u16, y: u16) -> Self {
        let mut g = Self { kind: GESTURE_TAP, duration_ms: 50, x: [0; 4], y: [0; 4] };
        g.x[0] = x;
        g.y[0] = y;
        g
    }

    pub fn long_press(x: u16, y: u16, duration_ms: u16) -> Self {
        let mut g =
            Self { kind: GESTURE_LONG_PRESS, duration_ms: duration_ms.max(LONG_PRESS_MS as u16), x: [0; 4], y: [0; 4] };
        g.x[0] = x;
        g.y[0] = y;
        g
    }

    pub fn swipe(x0: u16, y0: u16, x1: u16, y1: u16, duration_ms: u16) -> Self {
        let mut g = Self { kind: GESTURE_SWIPE, duration_ms: duration_ms.clamp(80, 1200), x: [0; 4], y: [0; 4] };
        g.x[0] = x0;
        g.y[0] = y0;
        g.x[1] = x1;
        g.y[1] = y1;
        g
    }

    pub fn pinch(a0: (u16, u16), a1: (u16, u16), b0: (u16, u16), b1: (u16, u16), duration_ms: u16) -> Self {
        Self {
            kind: GESTURE_PINCH,
            duration_ms: duration_ms.clamp(80, 1200),
            x: [a0.0, a1.0, b0.0, b1.0],
            y: [a0.1, a1.1, b0.1, b1.1],
        }
    }
}

/// Turns a mouse press on an Android host into a tap, long-press, or swipe.
#[derive(Clone, Debug)]
pub struct StrokeTracker {
    down: bool,
    down_ms: u64,
    origin: (u16, u16),
    last: (u16, u16),
    moved: bool,
    /// A long-press was already emitted, so the release is not also a tap.
    fired: bool,
}

impl Default for StrokeTracker {
    fn default() -> Self {
        Self { down: false, down_ms: 0, origin: (0, 0), last: (0, 0), moved: false, fired: false }
    }
}

impl StrokeTracker {
    pub fn is_down(&self) -> bool {
        self.down
    }

    pub fn last(&self) -> (u16, u16) {
        self.last
    }

    pub fn down(&mut self, t_ms: u64, x: u16, y: u16) {
        self.down = true;
        self.down_ms = t_ms;
        self.origin = (x, y);
        self.last = (x, y);
        self.moved = false;
        self.fired = false;
    }

    pub fn move_to(&mut self, x: u16, y: u16) {
        if !self.down {
            self.last = (x, y);
            return;
        }
        self.last = (x, y);
        if dist(self.origin, (x, y)) >= MOVE_THRESHOLD as u32 {
            self.moved = true;
        }
    }

    /// If the press has been held still long enough, returns the long-press
    /// once. Later calls return `None` until the next [`down`].
    pub fn poll_long_press(&mut self, t_ms: u64) -> Option<Gesture> {
        if !self.down || self.moved || self.fired {
            return None;
        }
        if t_ms.saturating_sub(self.down_ms) < LONG_PRESS_MS {
            return None;
        }
        self.fired = true;
        Some(Gesture::long_press(self.origin.0, self.origin.1, LONG_PRESS_MS as u16))
    }

    /// Finishes the press. `None` when nothing was down, or the long-press
    /// was already delivered.
    pub fn up(&mut self, t_ms: u64) -> Option<Gesture> {
        if !self.down {
            return None;
        }
        self.down = false;
        if self.fired {
            return None;
        }
        let held = t_ms.saturating_sub(self.down_ms);
        if self.moved {
            let duration = swipe_duration(dist(self.origin, self.last));
            return Some(Gesture::swipe(self.origin.0, self.origin.1, self.last.0, self.last.1, duration));
        }
        if held >= LONG_PRESS_MS {
            return Some(Gesture::long_press(self.origin.0, self.origin.1, held.min(u16::MAX as u64) as u16));
        }
        Some(Gesture::tap(self.origin.0, self.origin.1))
    }

    pub fn cancel(&mut self) {
        self.down = false;
        self.fired = false;
        self.moved = false;
    }
}

fn dist(a: (u16, u16), b: (u16, u16)) -> u32 {
    let dx = a.0.abs_diff(b.0) as u32;
    let dy = a.1.abs_diff(b.1) as u32;
    dx.max(dy)
}

fn swipe_duration(distance: u32) -> u16 {
    // A short flick is quick; a long drag across the screen takes longer,
    // which matches how dispatchGesture plays the path.
    (120 + distance / 40).clamp(120, 700) as u16
}

/// A mouse wheel notch becomes a swipe through the pointer.
/// `notches` is positive when the wheel rolls away from the user (content moves down
/// on Windows, so the finger swipes up). One notch is about 18% of the picture.
pub fn wheel_swipe(x: u16, y: u16, notches: i32) -> Gesture {
    let delta = (notches.clamp(-6, 6) * 12000).clamp(-65535, 65535);
    let end = (y as i32 - delta).clamp(0, 65535) as u16;
    let duration = swipe_duration(y.abs_diff(end) as u32);
    Gesture::swipe(x, y, x, end, duration)
}

/// Relative trackpad motion. `cursor` is the virtual pointer in normalized
/// space; `dx`/`dy` come from [`view_delta_to_norm`]. Returns the new cursor.
pub fn trackpad_move(cursor: (u16, u16), dx: i32, dy: i32) -> (u16, u16) {
    let x = (cursor.0 as i32 + dx).clamp(0, 65535) as u16;
    let y = (cursor.1 as i32 + dy).clamp(0, 65535) as u16;
    (x, y)
}

/// Mouse events from a controller that does not speak [`InputEvent::Gesture`],
/// folded into the same taps and swipes.
#[derive(Clone, Debug, Default)]
pub struct PointerAdapter {
    pub tracker: StrokeTracker,
    button_down: bool,
}

impl PointerAdapter {
    pub fn mouse_move(&mut self, x: u16, y: u16) {
        self.tracker.move_to(x, y);
    }

    pub fn mouse_button(&mut self, down: bool, t_ms: u64) -> Option<Gesture> {
        if down {
            self.button_down = true;
            let (x, y) = self.tracker.last();
            self.tracker.down(t_ms, x, y);
            None
        } else if self.button_down {
            self.button_down = false;
            self.tracker.up(t_ms)
        } else {
            None
        }
    }

    pub fn wheel(&mut self, dy_windows: i16) -> Option<Gesture> {
        if dy_windows == 0 {
            return None;
        }
        // 120 Windows units is one notch. Positive dy is away from the user.
        let notches = (dy_windows as i32) / 120;
        let notches = if notches == 0 { dy_windows.signum() as i32 } else { notches };
        let (x, y) = self.tracker.last();
        Some(wheel_swipe(x, y, notches))
    }

    pub fn poll(&mut self, t_ms: u64) -> Option<Gesture> {
        self.tracker.poll_long_press(t_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portrait() -> Frame {
        Frame { width: 1080, height: 2400 }
    }
    fn landscape() -> Frame {
        Frame { width: 2400, height: 1080 }
    }

    #[test]
    fn portrait_frame_in_a_landscape_view_letterboxes_the_sides() {
        let view = View { width: 1920, height: 1080 };
        let placed = letterbox(portrait(), view);
        assert!(placed.x > 100, "side bars, x={}", placed.x);
        assert_eq!(placed.y, 0);
        assert_eq!(placed.height, 1080);
        assert!(view_to_norm(placed, 10.0, 100.0).is_none(), "left bar is not the picture");
        let mid = view_to_norm(placed, view.width as f32 / 2.0, view.height as f32 / 2.0).unwrap();
        assert!((mid.0 as i32 - 32768).abs() < 120, "centre x {mid:?}");
        assert!((mid.1 as i32 - 32768).abs() < 120, "centre y {mid:?}");
        let top = view_to_norm(placed, view.width as f32 / 2.0, placed.y as f32).unwrap();
        assert_eq!(top.1, 0);
    }

    #[test]
    fn landscape_frame_in_a_portrait_view_letterboxes_top_and_bottom() {
        let view = View { width: 1080, height: 1920 };
        let placed = letterbox(landscape(), view);
        assert_eq!(placed.x, 0);
        assert!(placed.y > 100, "top bar, y={}", placed.y);
        assert!(view_to_norm(placed, 100.0, 8.0).is_none());
        let left = view_to_norm(placed, 0.0, placed.y as f32 + placed.height as f32 / 2.0).unwrap();
        assert_eq!(left.0, 0);
        let right = view_to_norm(placed, (placed.width - 1) as f32, placed.y as f32 + 10.0).unwrap();
        assert_eq!(right.0, 65535);
    }

    #[test]
    fn rotation_keeps_the_same_physical_corner() {
        // The phone turns clockwise: the old top-left is the new top-right.
        let (x, y) = rotate_norm_cw(0, 0);
        assert_eq!((x, y), (0, 65535));
        let (x, y) = rotate_norm_cw(65535, 0);
        assert_eq!((x, y), (0, 0));
        let (x, y) = rotate_norm_cw(0, 65535);
        assert_eq!((x, y), (65535, 65535));
        // A full turn comes back.
        let p = (12345u16, 40000u16);
        let mut q = p;
        for _ in 0..4 {
            q = rotate_norm_cw(q.0, q.1);
        }
        assert_eq!(q, p);
        let back = rotate_norm_ccw(rotate_norm_cw(p.0, p.1).0, rotate_norm_cw(p.0, p.1).1);
        assert_eq!(back, p);
    }

    #[test]
    fn rotation_changes_which_view_axis_is_the_long_one() {
        let wide = View { width: 800, height: 400 };
        let before = letterbox(portrait(), wide);
        let after = letterbox(landscape(), wide);
        assert!(before.width < before.height || before.x > after.x);
        assert!(before.x > after.x, "portrait picture is narrower in this view");
        assert_eq!(after.x, 0);
        assert!(after.width > before.width);
        // The same view centre still hits the picture after the rotate.
        let c = view_to_norm(after, 400.0, 200.0).unwrap();
        assert!(c.0 > 30000 && c.0 < 36000);
    }

    #[test]
    fn scaling_uses_the_picture_not_the_bars() {
        let placed = letterbox(portrait(), View { width: 1920, height: 1080 });
        let across = view_delta_to_norm(placed, placed.width as f32 - 1.0, 0.0);
        assert!((across.0 - 65535).abs() < 2, "a drag across the picture is the full axis, got {}", across.0);
        // Ten pixels is a much larger fraction of the narrow picture than of the wide view.
        let on_picture = view_delta_to_norm(placed, 10.0, 0.0).0.unsigned_abs();
        let on_view = (10.0f32 / (1920.0 - 1.0) * 65535.0).round() as u32;
        assert!(on_picture > on_view * 2, "picture {on_picture} view {on_view}");
        // A 10px move is a larger fraction of a short picture than a tall one.
        let short = Letterbox { x: 0, y: 0, width: 100, height: 1000 };
        let tall = Letterbox { x: 0, y: 0, width: 1000, height: 100 };
        let a = view_delta_to_norm(short, 10.0, 0.0).0.unsigned_abs();
        let b = view_delta_to_norm(tall, 10.0, 0.0).0.unsigned_abs();
        assert!(a > b * 5);
    }

    #[test]
    fn swipe_across_the_picture_spans_the_normalized_range() {
        let placed = letterbox(landscape(), View { width: 1080, height: 1920 });
        let start = view_to_norm(placed, placed.x as f32, placed.y as f32 + 4.0).unwrap();
        let end = view_to_norm(placed, (placed.x + placed.width as i32 - 1) as f32, placed.y as f32 + 4.0).unwrap();
        assert_eq!(start.0, 0);
        assert_eq!(end.0, 65535);
        let g = Gesture::swipe(start.0, start.1, end.0, end.1, 300);
        assert_eq!(g.kind, GESTURE_SWIPE);
        assert_eq!((g.x[0], g.x[1]), (0, 65535));
    }

    #[test]
    fn click_is_a_tap_and_a_drag_is_a_swipe() {
        let mut t = StrokeTracker::default();
        t.down(1_000, 1000, 2000);
        t.move_to(1100, 2050);
        let tap = t.up(1_100).unwrap();
        assert_eq!(tap.kind, GESTURE_TAP);
        assert_eq!((tap.x[0], tap.y[0]), (1000, 2000));

        t.down(2_000, 1000, 2000);
        t.move_to(1000, 2000 + MOVE_THRESHOLD + 50);
        let swipe = t.up(2_200).unwrap();
        assert_eq!(swipe.kind, GESTURE_SWIPE);
        assert_eq!(swipe.y[0], 2000);
        assert!(swipe.y[1] > swipe.y[0]);

        t.down(3_000, 4000, 4000);
        assert!(t.poll_long_press(3_200).is_none());
        let long = t.poll_long_press(3_000 + LONG_PRESS_MS).unwrap();
        assert_eq!(long.kind, GESTURE_LONG_PRESS);
        assert!(t.up(3_000 + LONG_PRESS_MS + 20).is_none(), "release after a long-press is not a tap");
    }

    #[test]
    fn wheel_becomes_a_vertical_swipe() {
        let up = wheel_swipe(1000, 30000, 1);
        assert_eq!(up.kind, GESTURE_SWIPE);
        assert_eq!(up.x[0], up.x[1]);
        assert!(up.y[1] < up.y[0], "rolling away swipes toward the top");
        let down = wheel_swipe(1000, 30000, -1);
        assert!(down.y[1] > down.y[0]);
        let clamped = wheel_swipe(0, 1000, 6);
        assert_eq!(clamped.y[1], 0);
    }

    #[test]
    fn pinch_keeps_both_fingers() {
        let g = Gesture::pinch((100, 200), (50, 200), (500, 200), (800, 200), 240);
        assert_eq!(g.kind, GESTURE_PINCH);
        assert_eq!(g.x, [100, 50, 500, 800]);
        assert_eq!(g.y[2], 200);
        assert_eq!(g.duration_ms, 240);
    }

    #[test]
    fn trackpad_motion_stays_inside_the_picture() {
        assert_eq!(trackpad_move((100, 200), -500, 40), (0, 240));
        assert_eq!(trackpad_move((65500, 65500), 1000, 1000), (65535, 65535));
    }

    #[test]
    fn norm_round_trip_hits_the_edges() {
        assert_eq!(norm_to_pixel(0, 1080), 0);
        assert_eq!(norm_to_pixel(65535, 1080), 1079);
        assert_eq!(norm_to_pixel(32768, 1080), 540);
        assert_eq!(norm_to_pixel(0, 1), 0);
    }

    #[test]
    fn legacy_mouse_events_become_a_swipe() {
        let mut a = PointerAdapter::default();
        a.mouse_move(1000, 2000);
        assert!(a.mouse_button(true, 10).is_none());
        a.mouse_move(1000, 2000 + MOVE_THRESHOLD + 10);
        let g = a.mouse_button(false, 80).unwrap();
        assert_eq!(g.kind, GESTURE_SWIPE);
        let wheel = {
            a.mouse_move(3000, 3000);
            a.wheel(120).unwrap()
        };
        assert_eq!(wheel.kind, GESTURE_SWIPE);
        assert!(wheel.y[1] < 3000);
    }

    #[test]
    fn gesture_round_trips_on_the_wire_after_the_version2_inputs() {
        let event = Gesture::tap(3, 4).to_event();
        let bytes = postcard::to_stdvec(&event).unwrap();
        assert_eq!(bytes[0], 5, "Gesture is appended after ReleaseAll");
        let back: InputEvent = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, event);
        let old = postcard::to_stdvec(&InputEvent::MouseMove { x: 9, y: 8 }).unwrap();
        assert_eq!(old[0], 0);
        assert!(matches!(postcard::from_bytes::<InputEvent>(&old).unwrap(), InputEvent::MouseMove { x: 9, y: 8 }));
    }
}
