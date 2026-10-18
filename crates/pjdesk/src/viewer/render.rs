//! Draws the remote picture into the window buffer.
//!
//! The caller decides *where* the picture goes — a [`Layout`] rectangle in
//! window pixels, which may be smaller than the window (fit / zoomed out,
//! letterboxed) or larger than it (zoomed in, cropped to what's visible and
//! panned). [`draw`] renders the frame into that rectangle with bilinear
//! scaling and fills the rest with the background colour.

/// Where the remote picture sits inside the window, in window pixels. `x`/`y`
/// may be negative when zoomed in (the picture extends past the top-left).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
}

/// A decoded remote picture in 0RGB pixels.
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

/// Largest rectangle with the picture's aspect ratio that fits the window
/// (centred, letterboxed) — the "fit / adaptive" zoom.
pub fn fit(src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Layout {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Layout::default();
    }
    let (width, height) = if src_w * dst_h <= dst_w * src_h {
        ((src_w * dst_h / src_h).clamp(1, dst_w), dst_h)
    } else {
        (dst_w, (src_h * dst_w / src_w).clamp(1, dst_h))
    };
    Layout { x: ((dst_w - width) / 2) as i32, y: ((dst_h - height) / 2) as i32, width, height }
}

/// Places the picture at an explicit scale (`1.0` = 100%, pixel-for-pixel),
/// centred when it is smaller than the window and panned by `pan` (clamped so
/// it can't drift off-screen) when it is larger.
pub fn placement(src_w: usize, src_h: usize, dst_w: usize, dst_h: usize, scale: f32, pan: (i32, i32)) -> Layout {
    let width = ((src_w as f32 * scale).round() as usize).max(1);
    let height = ((src_h as f32 * scale).round() as usize).max(1);
    let x = if width <= dst_w {
        ((dst_w - width) / 2) as i32
    } else {
        pan.0.clamp(dst_w as i32 - width as i32, 0)
    };
    let y = if height <= dst_h {
        ((dst_h - height) / 2) as i32
    } else {
        pan.1.clamp(dst_h as i32 - height as i32, 0)
    };
    Layout { x, y, width, height }
}

/// Copies one atlas tile out of a decoded frame.
///
/// `atlas_w`/`atlas_h` are the capture size from the host. The decoded frame
/// may be smaller, so the tile is scaled by the same factor as the picture.
pub fn crop_tile(frame: &RgbFrame, atlas_w: u16, atlas_h: u16, tile_x: u16, tile_y: u16, tile_w: u16, tile_h: u16) -> Option<RgbFrame> {
    if atlas_w == 0 || atlas_h == 0 || frame.width == 0 || frame.height == 0 || tile_w == 0 || tile_h == 0 {
        return None;
    }
    let sx = frame.width as f32 / atlas_w as f32;
    let sy = frame.height as f32 / atlas_h as f32;
    let x0 = ((tile_x as f32 * sx).round() as isize).clamp(0, frame.width as isize - 1) as usize;
    let y0 = ((tile_y as f32 * sy).round() as isize).clamp(0, frame.height as isize - 1) as usize;
    let x1 = ((tile_x as f32 * sx + tile_w as f32 * sx).round() as isize).clamp(x0 as isize + 1, frame.width as isize) as usize;
    let y1 = ((tile_y as f32 * sy + tile_h as f32 * sy).round() as isize).clamp(y0 as isize + 1, frame.height as isize) as usize;
    let width = x1 - x0;
    let height = y1 - y0;
    let mut pixels = Vec::with_capacity(width * height);
    for row in y0..y1 {
        let start = row * frame.width + x0;
        pixels.extend_from_slice(&frame.pixels[start..start + width]);
    }
    Some(RgbFrame { width, height, pixels })
}

/// Renders `frame` into the `dst_w`×`dst_h` buffer at `layout`, cropping to the
/// window and filling everything outside the picture with `background`.
pub fn draw_on(frame: &RgbFrame, dst: &mut [u32], dst_w: usize, dst_h: usize, layout: Layout, background: u32) {
    if frame.width == 0 || frame.height == 0 || layout.width == 0 || layout.height == 0 {
        dst[..dst_w * dst_h].fill(background);
        return;
    }
    // Visible slice of the picture, in window pixels.
    let vx0 = layout.x.max(0) as usize;
    let vy0 = layout.y.max(0) as usize;
    let vx1 = (layout.x + layout.width as i32).clamp(0, dst_w as i32) as usize;
    let vy1 = (layout.y + layout.height as i32).clamp(0, dst_h as i32) as usize;

    // Background: whole rows above/below the picture, and the side margins.
    for (row, line) in dst.chunks_exact_mut(dst_w).enumerate().take(dst_h) {
        if row < vy0 || row >= vy1 {
            line.fill(background);
        } else {
            line[..vx0.min(dst_w)].fill(background);
            line[vx1.min(dst_w)..].fill(background);
        }
    }
    if vx1 <= vx0 || vy1 <= vy0 {
        return;
    }

    let (sw, sh) = (frame.width, frame.height);
    // Per visible column, the two source columns and the blend weight.
    let columns: Vec<(usize, usize, u32)> = (vx0..vx1)
        .map(|dx| {
            let (x0, fx) = source_at(dx as i32 - layout.x, layout.width, sw);
            (x0, (x0 + 1).min(sw - 1), fx)
        })
        .collect();

    let visible_w = vx1 - vx0;
    let visible_h = vy1 - vy0;
    let threads = if visible_w * visible_h >= 640 * 480 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)
    } else {
        1
    };
    let rows_per_chunk = visible_h.div_ceil(threads);
    let band = &mut dst[vy0 * dst_w..vy1 * dst_w];
    let columns = &columns;
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in band.chunks_mut(rows_per_chunk * dst_w).enumerate() {
            scope.spawn(move || {
                for (r, line) in chunk.chunks_exact_mut(dst_w).enumerate() {
                    let dy = vy0 + chunk_index * rows_per_chunk + r;
                    let (y0, fy) = source_at(dy as i32 - layout.y, layout.height, sh);
                    let y1 = (y0 + 1).min(sh - 1);
                    let top = &frame.pixels[y0 * sw..(y0 + 1) * sw];
                    let bottom = &frame.pixels[y1 * sw..(y1 + 1) * sw];
                    for (dx, &(x0, x1, fx)) in (vx0..vx1).zip(columns) {
                        let upper = lerp(top[x0], top[x1], fx);
                        let lower = lerp(bottom[x0], bottom[x1], fx);
                        line[dx] = lerp(upper, lower, fy);
                    }
                }
            });
        }
    });
}

/// Blends two 0RGB pixels: `t` in 0..=256 is the weight of `b`.
#[inline(always)]
fn lerp(a: u32, b: u32, t: u32) -> u32 {
    let rb = (((a & 0xFF00FF) * (256 - t) + (b & 0xFF00FF) * t) >> 8) & 0xFF00FF;
    let g = (((a & 0x00FF00) * (256 - t) + (b & 0x00FF00) * t) >> 8) & 0x00FF00;
    rb | g
}

/// Maps destination offset `i` (within a picture `dst_len` pixels tall/wide) to
/// a source pixel and blend weight, sampling at pixel centres.
#[inline(always)]
fn source_at(i: i32, dst_len: usize, src_len: usize) -> (usize, u32) {
    let pos = ((i as f64 + 0.5) * src_len as f64 / dst_len as f64 - 0.5).max(0.0);
    let whole = (pos as usize).min(src_len - 1);
    let frac = ((pos - whole as f64) * 256.0) as u32;
    (whole, frac.min(256))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKGROUND: u32 = 0x00_1e_1e_22;

    #[test]
    fn fit_keeps_aspect_ratio() {
        assert_eq!(fit(1920, 1080, 1920, 1080), Layout { x: 0, y: 0, width: 1920, height: 1080 });
        assert_eq!(fit(1920, 1080, 1000, 1000), Layout { x: 0, y: 219, width: 1000, height: 562 });
        assert_eq!(fit(1000, 1000, 1920, 1080), Layout { x: 420, y: 0, width: 1080, height: 1080 });
    }

    #[test]
    fn placement_centres_and_pans() {
        // Smaller than the window: centred, pan ignored.
        assert_eq!(placement(100, 100, 400, 400, 1.0, (50, 50)), Layout { x: 150, y: 150, width: 100, height: 100 });
        // Larger than the window: panned and clamped so an edge can't show gap.
        let big = placement(100, 100, 200, 200, 4.0, (-1000, 0));
        assert_eq!((big.width, big.height), (400, 400));
        assert_eq!(big.x, -200); // clamped to dst_w - width
        assert_eq!(big.y, 0); // centred pan 0 clamps to top when exactly offered
    }

    #[test]
    fn draw_letterboxes_and_scales() {
        let frame = RgbFrame { width: 4, height: 2, pixels: vec![0xFF0000; 8] };
        let mut dst = vec![0u32; 8 * 8];
        let layout = fit(4, 2, 8, 8);
        assert_eq!(layout, Layout { x: 0, y: 2, width: 8, height: 4 });
        draw_on(&frame, &mut dst, 8, 8, layout, BACKGROUND);
        assert_eq!(dst[0], BACKGROUND);
        assert_eq!(dst[3 * 8 + 4], 0xFF0000);
        assert_eq!(dst[7 * 8], BACKGROUND);
    }

    #[test]
    fn draw_crops_when_zoomed_in() {
        // 2x zoom of a 4x4 frame into an 8x8 window with the picture shifted
        // left by 4px: only the right half of the (8x8) scaled picture shows.
        let frame = RgbFrame { width: 4, height: 4, pixels: vec![0x00FF00; 16] };
        let mut dst = vec![0u32; 8 * 8];
        draw_on(&frame, &mut dst, 8, 8, Layout { x: -4, y: 0, width: 8, height: 8 }, BACKGROUND);
        assert_eq!(dst[0], 0x00FF00); // left half of the window is inside the picture
        assert_eq!(dst[3], 0x00FF00);
        assert_eq!(dst[7], BACKGROUND); // right half is past the left-shifted picture
    }

    #[test]
    fn lerp_blends_channels() {
        assert_eq!(lerp(0x000000, 0xFFFFFF, 0), 0x000000);
        assert_eq!(lerp(0x000000, 0xFFFFFF, 256), 0xFFFFFF);
        assert_eq!(lerp(0x000000, 0xFF00FF, 128), 0x7F007F);
    }
}
