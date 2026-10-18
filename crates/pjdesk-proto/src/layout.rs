//! Display arrangement and pointer mapping.
//!
//! Positions are virtual-desktop pixels. A display left of or above the primary
//! has a negative origin. Mouse coordinates on the wire stay normalized
//! (`0..=65535`) inside whichever rectangle is being shared: one display, or
//! the bounding box of every display in all-screens view.
//!
//! DPI is recorded so the controller can label a display. Pointer mapping uses
//! the physical pixel size. Windows `BitBlt` and `MOUSEEVENTF_VIRTUALDESK` are
//! physical, so a 144 DPI display is not scaled down to 96 DPI before the hit
//! test.

/// One display in virtual-desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    pub index: u8,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Effective DPI. `0` means the host did not report one.
    pub dpi: u32,
}

impl Screen {
    /// `dpi / 96`, or `1.0` when DPI was not reported.
    pub fn dpi_scale(self) -> f32 {
        let dpi = if self.dpi == 0 { 96 } else { self.dpi };
        dpi as f32 / 96.0
    }
}

/// A rectangle in the monitor-switcher map, in box pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapRect {
    pub index: u8,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Bounding box of every display: `(x, y, width, height)`.
pub fn bounds(screens: &[Screen]) -> Option<(i32, i32, i32, i32)> {
    let mut iter = screens.iter().filter(|s| s.width > 0 && s.height > 0);
    let first = iter.next()?;
    let mut left = first.x;
    let mut top = first.y;
    let mut right = first.x.saturating_add(first.width as i32);
    let mut bottom = first.y.saturating_add(first.height as i32);
    for s in iter {
        left = left.min(s.x);
        top = top.min(s.y);
        right = right.max(s.x.saturating_add(s.width as i32));
        bottom = bottom.max(s.y.saturating_add(s.height as i32));
    }
    let w = right.saturating_sub(left).max(1);
    let h = bottom.saturating_sub(top).max(1);
    Some((left, top, w, h))
}

/// Which display contains a virtual-desktop pixel, and where on that display
/// the pointer sits, normalized to `0..=65535`.
pub fn hit(screens: &[Screen], vx: i32, vy: i32) -> Option<(u8, u16, u16)> {
    for s in screens {
        if s.width == 0 || s.height == 0 {
            continue;
        }
        let right = s.x.saturating_add(s.width as i32);
        let bottom = s.y.saturating_add(s.height as i32);
        if vx >= s.x && vx < right && vy >= s.y && vy < bottom {
            return Some((s.index, axis(vx, s.x, s.width as i32), axis(vy, s.y, s.height as i32)));
        }
    }
    None
}

/// Host mapping: `origin + (n * (size - 1) + 32767) / 65535`.
pub fn to_virtual(origin: i32, size: i32, n: u16) -> i32 {
    if size <= 1 {
        return origin;
    }
    origin + ((n as i64 * (size as i64 - 1) + 32767) / 65535) as i32
}

/// A point in a letterboxed all-screens canvas, mapped back to a display.
///
/// The bounding box is scaled uniformly to fit `canvas_w` × `canvas_h`. Points
/// in the margin (the bars around the picture) return `None`.
pub fn canvas_to_virtual(screens: &[Screen], cx: f32, cy: f32, canvas_w: f32, canvas_h: f32) -> Option<(u8, u16, u16)> {
    let (bx, by, bw, bh) = bounds(screens)?;
    if canvas_w < 2.0 || canvas_h < 2.0 || bw < 1 || bh < 1 {
        return None;
    }
    let scale = (canvas_w / bw as f32).min(canvas_h / bh as f32);
    if scale <= 0.0 {
        return None;
    }
    let used_w = bw as f32 * scale;
    let used_h = bh as f32 * scale;
    let ox = (canvas_w - used_w) / 2.0;
    let oy = (canvas_h - used_h) / 2.0;
    if cx < ox || cy < oy || cx >= ox + used_w || cy >= oy + used_h {
        return None;
    }
    let vx = bx as f32 + (cx - ox) / scale;
    let vy = by as f32 + (cy - oy) / scale;
    hit(screens, vx.floor() as i32, vy.floor() as i32)
}

/// Lay the displays out in `box_w` × `box_h` the way they sit on the desktop.
pub fn map_rects(screens: &[Screen], box_w: f32, box_h: f32, pad: f32) -> Vec<MapRect> {
    let Some((bx, by, bw, bh)) = bounds(screens) else {
        return Vec::new();
    };
    let inner_w = (box_w - pad * 2.0).max(1.0);
    let inner_h = (box_h - pad * 2.0).max(1.0);
    let scale = (inner_w / bw as f32).min(inner_h / bh as f32);
    let used_w = bw as f32 * scale;
    let used_h = bh as f32 * scale;
    let ox = pad + (inner_w - used_w) / 2.0;
    let oy = pad + (inner_h - used_h) / 2.0;
    screens
        .iter()
        .filter(|s| s.width > 0 && s.height > 0)
        .map(|s| MapRect {
            index: s.index,
            x: ox + (s.x - bx) as f32 * scale,
            y: oy + (s.y - by) as f32 * scale,
            w: (s.width as f32 * scale).max(6.0),
            h: (s.height as f32 * scale).max(6.0),
        })
        .collect()
}

fn axis(pixel: i32, origin: i32, size: i32) -> u16 {
    if size <= 1 {
        return 0;
    }
    let rel = (pixel - origin).clamp(0, size - 1) as i64;
    ((rel * 65535) / (size as i64 - 1)).clamp(0, 65535) as u16
}

/// The three-display layout used by the mapping tests and the toolbar shot.
pub fn sample_layout() -> Vec<Screen> {
    vec![
        Screen { index: 0, x: 0, y: 0, width: 1920, height: 1080, dpi: 96 },
        Screen { index: 1, x: -1920, y: 0, width: 1920, height: 1080, dpi: 96 },
        Screen { index: 2, x: 0, y: -800, width: 1280, height: 800, dpi: 144 },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_origins_hit_the_display_that_owns_them() {
        let screens = sample_layout();
        assert_eq!(bounds(&screens), Some((-1920, -800, 3840, 1880)));
        assert_eq!(hit(&screens, -1, 10).map(|h| h.0), Some(1));
        assert_eq!(hit(&screens, 0, 10).map(|h| h.0), Some(0));
        assert_eq!(hit(&screens, 10, -1).map(|h| h.0), Some(2));
        assert_eq!(hit(&screens, 10, 0).map(|h| h.0), Some(0));
        assert!(hit(&screens, 5000, 10).is_none());
    }

    #[test]
    fn host_rounding_stays_inside_the_display() {
        let origin = -1920;
        let size = 1920;
        for n in [0u16, 1, 32768, 65534, 65535] {
            let px = to_virtual(origin, size, n);
            assert!(px >= origin && px < origin + size, "{n} -> {px}");
            let (index, _, _) = hit(&sample_layout(), px, 40).unwrap();
            assert_eq!(index, 1);
        }
        let (index, nx, _) = hit(&sample_layout(), to_virtual(origin, size, 0), 40).unwrap();
        assert_eq!((index, nx), (1, 0));
        let (_, nx, _) = hit(&sample_layout(), to_virtual(origin, size, 65535), 40).unwrap();
        assert_eq!(nx, 65535);
        assert_eq!(to_virtual(0, 1080, 0), 0);
        assert_eq!(to_virtual(0, 1080, 65535), 1079);
        assert_eq!(to_virtual(-800, 800, 0), -800);
    }

    #[test]
    fn fitted_canvas_crosses_display_boundaries() {
        let screens = sample_layout();
        let (w, h) = (800.0_f32, 400.0);
        let left = canvas_to_virtual(&screens, 20.0, 220.0, w, h).unwrap();
        assert_eq!(left.0, 1, "left side of the fitted picture is the display at x=-1920");
        let primary = canvas_to_virtual(&screens, 500.0, 250.0, w, h).unwrap();
        assert_eq!(primary.0, 0);
        let above = canvas_to_virtual(&screens, 500.0, 40.0, w, h).unwrap();
        assert_eq!(above.0, 2);
        assert!(canvas_to_virtual(&screens, 2.0, 2.0, w, h).is_none());
    }

    #[test]
    fn dpi_is_a_label_not_a_coordinate_scale() {
        let above = sample_layout()[2];
        assert!((above.dpi_scale() - 1.5).abs() < 0.001);
        let physical = hit(&sample_layout(), 1279, -800).unwrap();
        assert_eq!(physical.0, 2);
        assert_eq!(physical.1, 65535);
        let unknown = Screen { dpi: 0, ..above };
        assert!((unknown.dpi_scale() - 1.0).abs() < 0.001);
    }

    #[test]
    fn map_keeps_the_desktop_arrangement() {
        let rects = map_rects(&sample_layout(), 280.0, 150.0, 8.0);
        let of = |i: u8| rects.iter().find(|r| r.index == i).unwrap();
        assert!(of(1).x < of(0).x);
        assert!(of(2).y < of(0).y);
        assert!(of(0).w > 8.0 && of(2).h > 6.0);
    }
}
