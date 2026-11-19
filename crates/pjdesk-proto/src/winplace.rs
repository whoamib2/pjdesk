//! Size and place a window inside a monitor work area.
//!
//! Coordinates are in the same unit the caller uses (physical pixels on
//! Windows, logical points for the egui chat window). The functions do not
//! talk to the OS, so they can be tested anywhere.

use crate::trayshow::{intersects, place_on_work_area, Rect};

/// Smallest remote-viewer content size, before the toolbar is added.
pub const MIN_CONTENT_W: i32 = 320;
pub const MIN_CONTENT_H: i32 = 200;

/// Fit a remote picture of `content_w` × `content_h` plus `extra_h` (toolbar
/// or other chrome that belongs to the inner height) into `work`. The picture
/// keeps its aspect ratio, is never scaled up, and the window is centered and
/// stays inside the work area (the part of the monitor that excludes the
/// taskbar).
pub fn fit_centered(content_w: i32, content_h: i32, extra_h: i32, work: Rect) -> Rect {
    let work_w = work.w.max(1);
    let work_h = work.h.max(1);
    let extra = extra_h.max(0);
    let content_w = content_w.max(1);
    let content_h = content_h.max(1);
    let avail_h = (work_h - extra).max(1);
    let scale = (work_w as f64 / content_w as f64)
        .min(avail_h as f64 / content_h as f64)
        .min(1.0);
    let mut body_h = (content_h as f64 * scale).floor() as i32;
    let mut body_w = (content_w as f64 * body_h as f64 / content_h as f64).round() as i32;
    if body_w < MIN_CONTENT_W && MIN_CONTENT_W <= work_w && body_h < MIN_CONTENT_H && MIN_CONTENT_H + extra <= work_h {
        // A tiny picture can still get a usable window, without leaving the work area.
        let up = (MIN_CONTENT_W as f64 / body_w as f64).min(MIN_CONTENT_H as f64 / body_h as f64);
        body_w = (body_w as f64 * up).round() as i32;
        body_h = (body_h as f64 * up).round() as i32;
    }
    body_w = body_w.max(1).min(work_w);
    body_h = body_h.max(1).min(avail_h);
    place_on_work_area(
        Rect {
            x: work.x,
            y: work.y,
            w: body_w,
            h: body_h + extra,
        },
        work,
    )
}

/// Keep `window` inside `work`. A fully off-screen window is centered (and
/// shrunk if it cannot fit). A partly off-screen window is slid back, and
/// shrunk when it is larger than the work area, so no edge sticks out.
pub fn clamp_to_work_area(window: Rect, work: Rect) -> Rect {
    let work_w = work.w.max(1);
    let work_h = work.h.max(1);
    let w = window.w.max(1).min(work_w);
    let h = window.h.max(1).min(work_h);
    if !intersects(window, work) {
        return place_on_work_area(Rect { x: work.x, y: work.y, w, h }, work);
    }
    let mut x = window.x;
    let mut y = window.y;
    if x < work.x {
        x = work.x;
    }
    if y < work.y {
        y = work.y;
    }
    if x + w > work.x + work_w {
        x = work.x + work_w - w;
    }
    if y + h > work.y + work_h {
        y = work.y + work_h - h;
    }
    Rect { x, y, w, h }
}

/// Center `w` × `h` in `work`, or clamp a saved finite position. Non-finite
/// `x`/`y` (first open) are treated as "no saved place".
pub fn clamp_or_center(x: f32, y: f32, w: f32, h: f32, work: Rect) -> (f32, f32, f32, f32) {
    let w = (w.round() as i32).max(1).min(work.w.max(1));
    let h = (h.round() as i32).max(1).min(work.h.max(1));
    let placed = if x.is_finite() && y.is_finite() {
        clamp_to_work_area(
            Rect {
                x: x.round() as i32,
                y: y.round() as i32,
                w,
                h,
            },
            work,
        )
    } else {
        place_on_work_area(Rect { x: work.x, y: work.y, w, h }, work)
    };
    (placed.x as f32, placed.y as f32, placed.w as f32, placed.h as f32)
}

/// Box-filter an RGBA image to a new size. Used to build the 16 px chat icon
/// from the 32 px artwork.
pub fn scale_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() != (sw * sh * 4) as usize {
        return vec![0; (dw.saturating_mul(dh).saturating_mul(4)) as usize];
    }
    if sw == dw && sh == dh {
        return src.to_vec();
    }
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        let y0 = y * sh / dh;
        let y1 = ((y + 1) * sh / dh).max(y0 + 1).min(sh);
        for x in 0..dw {
            let x0 = x * sw / dw;
            let x1 = ((x + 1) * sw / dw).max(x0 + 1).min(sw);
            let mut r = 0u32;
            let mut g = 0u32;
            let mut b = 0u32;
            let mut a = 0u32;
            let mut n = 0u32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = ((yy * sw + xx) * 4) as usize;
                    r += u32::from(src[i]);
                    g += u32::from(src[i + 1]);
                    b += u32::from(src[i + 2]);
                    a += u32::from(src[i + 3]);
                    n += 1;
                }
            }
            let o = ((y * dw + x) * 4) as usize;
            if n > 0 {
                out[o] = (r / n) as u8;
                out[o + 1] = (g / n) as u8;
                out[o + 2] = (b / n) as u8;
                out[o + 3] = (a / n) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> Rect {
        // 1920×1080 monitor with a 40 px taskbar at the bottom.
        Rect { x: 0, y: 0, w: 1920, h: 1040 }
    }

    #[test]
    fn a_full_hd_picture_fits_the_work_area_and_stays_centered() {
        let placed = fit_centered(1920, 1080, 40, work());
        assert!(placed.w <= 1920);
        assert!(placed.h <= 1040);
        let body_h = placed.h - 40;
        assert!(
            (placed.w as i64 * 1080 - body_h as i64 * 1920).abs() <= 1920,
            "aspect ratio of the picture is kept (within one pixel): {}x{}",
            placed.w, body_h
        );
        assert!(placed.x >= 0 && placed.y >= 0);
        assert!(placed.x + placed.w <= 1920);
        assert!(placed.y + placed.h <= 1040);
        let mid_x = work().x + work().w / 2;
        let mid_y = work().y + work().h / 2;
        assert!((placed.x + placed.w / 2 - mid_x).abs() <= 1);
        assert!((placed.y + placed.h / 2 - mid_y).abs() <= 1);
    }

    #[test]
    fn a_smaller_picture_is_not_upscaled_and_is_centered() {
        let placed = fit_centered(800, 600, 40, work());
        assert_eq!(placed.w, 800);
        assert_eq!(placed.h, 640);
        assert_eq!(placed.x, (1920 - 800) / 2);
        assert_eq!(placed.y, (1040 - 640) / 2);
    }

    #[test]
    fn a_window_that_starts_low_is_pulled_onto_the_work_area() {
        // 1280×720 grown in place near the bottom, the 0.6.98 behaviour.
        let low = Rect { x: 200, y: 400, w: 1728, h: 972 };
        let placed = clamp_to_work_area(low, work());
        assert!(placed.y >= work().y);
        assert!(placed.y + placed.h <= work().y + work().h);
        assert!(placed.x >= work().x);
        assert!(placed.x + placed.w <= work().x + work().w);
        assert_eq!(placed.w, 1728);
        assert_eq!(placed.h, 972);
    }

    #[test]
    fn an_off_screen_saved_place_is_centered_and_shrunk_to_fit() {
        let parked = Rect { x: 4000, y: 80, w: 2000, h: 1400 };
        let placed = clamp_to_work_area(parked, work());
        assert_eq!(placed.w, 1920);
        assert_eq!(placed.h, 1040);
        assert_eq!(placed.x, 0);
        assert_eq!(placed.y, 0);
        assert!(intersects(placed, work()));
    }

    #[test]
    fn a_second_monitor_work_area_is_respected() {
        let work = Rect { x: 1920, y: 0, w: 2560, h: 1400 };
        let placed = fit_centered(2560, 1440, 0, work);
        assert!(placed.x >= 1920);
        assert!(placed.x + placed.w <= 1920 + 2560);
        assert!(placed.y >= 0);
        assert!(placed.y + placed.h <= 1400);
        assert!(
            (placed.w as i64 * 1440 - placed.h as i64 * 2560).abs() <= 2560,
            "aspect ratio is kept within one pixel: {}x{}",
            placed.w, placed.h
        );
    }

    #[test]
    fn a_chat_window_with_no_saved_place_is_centered() {
        let (x, y, w, h) = clamp_or_center(f32::NAN, f32::NAN, 360.0, 460.0, work());
        assert_eq!((w, h), (360.0, 460.0));
        assert_eq!(x, ((1920 - 360) / 2) as f32);
        assert_eq!(y, ((1040 - 460) / 2) as f32);
    }

    #[test]
    fn a_saved_chat_place_that_is_on_screen_is_kept() {
        let (x, y, w, h) = clamp_or_center(1422.0, 185.0, 360.0, 460.0, work());
        assert_eq!((x, y, w, h), (1422.0, 185.0, 360.0, 460.0));
    }

    #[test]
    fn a_saved_chat_place_off_the_right_edge_is_slid_back() {
        let (x, y, w, h) = clamp_or_center(1800.0, 185.0, 360.0, 460.0, work());
        assert_eq!(w, 360.0);
        assert_eq!(h, 460.0);
        assert_eq!(x, (1920 - 360) as f32);
        assert_eq!(y, 185.0);
    }

    #[test]
    fn scale_rgba_keeps_a_transparent_corner_and_the_accent() {
        let mut src = vec![0u8; 32 * 32 * 4];
        for y in 8..24 {
            for x in 8..24 {
                let i = (y * 32 + x) * 4;
                src[i] = 0x0A;
                src[i + 1] = 0x84;
                src[i + 2] = 0xFF;
                src[i + 3] = 255;
            }
        }
        let out = scale_rgba(&src, 32, 32, 16, 16);
        assert_eq!(out.len(), 16 * 16 * 4);
        assert_eq!(&out[0..4], &[0, 0, 0, 0]);
        let mid = (8 * 16 + 8) * 4;
        assert!(out[mid + 3] > 200);
        assert!(out[mid + 2] > out[mid].saturating_add(40));
    }
}
