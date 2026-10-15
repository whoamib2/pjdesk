//! Draws the remote picture into the window buffer: fit to the window with
//! the aspect ratio kept (letterboxed), 1:1 copy when sizes match, bilinear
//! scaling otherwise.

/// Where the remote picture sits inside the window, in window pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

const BACKGROUND: u32 = 0x00_1e_1e_22;

/// A decoded remote picture in 0RGB pixels.
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

/// Largest rectangle with the picture's aspect ratio that fits the window.
pub fn fit(src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Layout {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Layout::default();
    }
    let (width, height) = if src_w * dst_h <= dst_w * src_h {
        // Window is relatively wider: full height.
        ((src_w * dst_h / src_h).clamp(1, dst_w), dst_h)
    } else {
        (dst_w, (src_h * dst_w / src_w).clamp(1, dst_h))
    };
    Layout { x: (dst_w - width) / 2, y: (dst_h - height) / 2, width, height }
}

/// Renders `frame` into `dst` (a `dst_w`×`dst_h` buffer) and returns where it went.
pub fn draw(frame: &RgbFrame, dst: &mut [u32], dst_w: usize, dst_h: usize) -> Layout {
    let layout = fit(frame.width, frame.height, dst_w, dst_h);
    // Letterbox bars.
    for (row, line) in dst.chunks_exact_mut(dst_w).enumerate().take(dst_h) {
        if row < layout.y || row >= layout.y + layout.height {
            line.fill(BACKGROUND);
        } else {
            line[..layout.x].fill(BACKGROUND);
            line[layout.x + layout.width..].fill(BACKGROUND);
        }
    }
    if layout.width == 0 {
        return layout;
    }

    if layout.width == frame.width && layout.height == frame.height {
        for row in 0..layout.height {
            let start = (layout.y + row) * dst_w + layout.x;
            dst[start..start + layout.width].copy_from_slice(&frame.pixels[row * frame.width..(row + 1) * frame.width]);
        }
    } else {
        scale_bilinear(frame, dst, dst_w, layout);
    }
    layout
}

/// Blends two 0RGB pixels: `t` in 0..=256 is the weight of `b`.
#[inline(always)]
fn lerp(a: u32, b: u32, t: u32) -> u32 {
    let rb = (((a & 0xFF00FF) * (256 - t) + (b & 0xFF00FF) * t) >> 8) & 0xFF00FF;
    let g = (((a & 0x00FF00) * (256 - t) + (b & 0x00FF00) * t) >> 8) & 0x00FF00;
    rb | g
}

/// Maps destination index `i` of `dst_len` to a source position in 16.16 fixed point.
#[inline(always)]
fn source_pos(i: usize, dst_len: usize, src_len: usize) -> (usize, u32) {
    // Sample at pixel centers: (i + 0.5) * src/dst - 0.5
    let pos = (((2 * i + 1) * src_len) << 16) / (2 * dst_len);
    let pos = pos.saturating_sub(1 << 15);
    let whole = (pos >> 16).min(src_len - 1);
    let frac = ((pos & 0xFFFF) >> 8) as u32; // 0..=255
    (whole, frac)
}

fn scale_bilinear(frame: &RgbFrame, dst: &mut [u32], dst_w: usize, layout: Layout) {
    let (sw, sh) = (frame.width, frame.height);
    let columns: Vec<(usize, usize, u32)> = (0..layout.width)
        .map(|x| {
            let (x0, fx) = source_pos(x, layout.width, sw);
            (x0, (x0 + 1).min(sw - 1), fx)
        })
        .collect();

    let threads = if layout.width * layout.height >= 640 * 480 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)
    } else {
        1
    };
    let rows_per_chunk = layout.height.div_ceil(threads);
    let region = &mut dst[layout.y * dst_w..(layout.y + layout.height) * dst_w];
    let columns = &columns;
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in region.chunks_mut(rows_per_chunk * dst_w).enumerate() {
            scope.spawn(move || {
                for (r, line) in chunk.chunks_exact_mut(dst_w).enumerate() {
                    let y = chunk_index * rows_per_chunk + r;
                    let (y0, fy) = source_pos(y, layout.height, sh);
                    let y1 = (y0 + 1).min(sh - 1);
                    let top = &frame.pixels[y0 * sw..(y0 + 1) * sw];
                    let bottom = &frame.pixels[y1 * sw..(y1 + 1) * sw];
                    let out = &mut line[layout.x..layout.x + layout.width];
                    for (pixel, &(x0, x1, fx)) in out.iter_mut().zip(columns) {
                        let upper = lerp(top[x0], top[x1], fx);
                        let lower = lerp(bottom[x0], bottom[x1], fx);
                        *pixel = lerp(upper, lower, fy);
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_ratio() {
        assert_eq!(fit(1920, 1080, 1920, 1080), Layout { x: 0, y: 0, width: 1920, height: 1080 });
        assert_eq!(fit(1920, 1080, 1000, 1000), Layout { x: 0, y: 219, width: 1000, height: 562 });
        assert_eq!(fit(1000, 1000, 1920, 1080), Layout { x: 420, y: 0, width: 1080, height: 1080 });
    }

    #[test]
    fn draw_letterboxes_and_scales() {
        let frame = RgbFrame { width: 4, height: 2, pixels: vec![0xFF0000; 8] };
        let mut dst = vec![0u32; 8 * 8];
        let layout = draw(&frame, &mut dst, 8, 8);
        assert_eq!(layout, Layout { x: 0, y: 2, width: 8, height: 4 });
        assert_eq!(dst[0], BACKGROUND);
        assert_eq!(dst[3 * 8 + 4], 0xFF0000);
        assert_eq!(dst[7 * 8], BACKGROUND);
    }

    #[test]
    fn lerp_blends_channels() {
        assert_eq!(lerp(0x000000, 0xFFFFFF, 0), 0x000000);
        assert_eq!(lerp(0x000000, 0xFFFFFF, 256), 0xFFFFFF);
        assert_eq!(lerp(0x000000, 0xFF00FF, 128), 0x7F007F);
    }
}
