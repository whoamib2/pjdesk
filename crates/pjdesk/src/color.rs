//! Color conversion between the screen's BGRA pixels and the I420 (YUV 4:2:0)
//! pictures the H.264 codec works with.
//!
//! Full-range BT.601 is used in both directions (and signaled in the stream's
//! VUI), which keeps desktop blacks and whites exact. Large images are split
//! across a few threads.

use openh264::formats::YUVSource;

/// A planar YUV 4:2:0 picture with tightly packed planes and even dimensions.
#[derive(Default, Clone)]
pub struct I420Buffer {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420Buffer {
    fn resize(&mut self, width: usize, height: usize) {
        debug_assert!(width % 2 == 0 && height % 2 == 0);
        self.width = width;
        self.height = height;
        self.y.resize(width * height, 0);
        self.u.resize(width / 2 * (height / 2), 128);
        self.v.resize(width / 2 * (height / 2), 128);
    }
}

impl YUVSource for I420Buffer {
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn strides(&self) -> (usize, usize, usize) {
        (self.width, self.width / 2, self.width / 2)
    }

    fn y(&self) -> &[u8] {
        &self.y
    }

    fn u(&self) -> &[u8] {
        &self.u
    }

    fn v(&self) -> &[u8] {
        &self.v
    }
}

/// Encoded size of a `width`×`height` image downscaled by `factor`
/// (rounded down to even numbers, as 4:2:0 requires).
pub fn scaled_size(width: usize, height: usize, factor: usize) -> (usize, usize) {
    ((width / factor) & !1, (height / factor) & !1)
}

/// Number of threads worth using for an image of `pixels` pixels.
fn worker_count(pixels: usize) -> usize {
    if pixels < 640 * 480 {
        return 1;
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)
}

#[inline(always)]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    ((77 * r + 150 * g + 29 * b + 128) >> 8) as u8
}

/// U from the sums of four pixels' R, G, B.
#[inline(always)]
fn chroma_u(sr: i32, sg: i32, sb: i32) -> u8 {
    (((-43 * sr - 85 * sg + 128 * sb + 512) >> 10) + 128).clamp(0, 255) as u8
}

/// V from the sums of four pixels' R, G, B.
#[inline(always)]
fn chroma_v(sr: i32, sg: i32, sb: i32) -> u8 {
    (((128 * sr - 107 * sg - 21 * sb + 512) >> 10) + 128).clamp(0, 255) as u8
}

/// Converts a BGRA/BGRX image to I420, box-downscaling by an integer `factor`.
pub fn bgra_to_i420(src: &[u8], width: usize, height: usize, stride: usize, factor: usize, out: &mut I420Buffer) {
    let factor = factor.max(1);
    let (ow, oh) = scaled_size(width, height, factor);
    out.resize(ow, oh);
    if ow == 0 || oh == 0 {
        return;
    }
    let pairs = oh / 2;
    let threads = worker_count(ow * oh).min(pairs).max(1);
    let pairs_per_chunk = pairs.div_ceil(threads);
    let cw = ow / 2;

    if threads == 1 {
        convert_pairs(src, stride, factor, ow, 0, &mut out.y, &mut out.u, &mut out.v);
        return;
    }
    let y_chunks = out.y.chunks_mut(pairs_per_chunk * 2 * ow);
    let u_chunks = out.u.chunks_mut(pairs_per_chunk * cw);
    let v_chunks = out.v.chunks_mut(pairs_per_chunk * cw);
    std::thread::scope(|scope| {
        for (i, ((y, u), v)) in y_chunks.zip(u_chunks).zip(v_chunks).enumerate() {
            let first_pair = i * pairs_per_chunk;
            scope.spawn(move || convert_pairs(src, stride, factor, ow, first_pair, y, u, v));
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn convert_pairs(
    src: &[u8],
    stride: usize,
    factor: usize,
    ow: usize,
    first_pair: usize,
    y: &mut [u8],
    u: &mut [u8],
    v: &mut [u8],
) {
    let cw = ow / 2;
    let pairs = y.len() / (2 * ow);
    for pair in 0..pairs {
        let oy = (first_pair + pair) * 2; // output row
        let (y0, y1) = y[pair * 2 * ow..(pair + 1) * 2 * ow].split_at_mut(ow);
        let u_row = &mut u[pair * cw..(pair + 1) * cw];
        let v_row = &mut v[pair * cw..(pair + 1) * cw];

        if factor == 1 {
            let r0 = &src[oy * stride..oy * stride + ow * 4];
            let r1 = &src[(oy + 1) * stride..(oy + 1) * stride + ow * 4];
            for cx in 0..cw {
                let i = cx * 8;
                let (b00, g00, r00) = (r0[i] as i32, r0[i + 1] as i32, r0[i + 2] as i32);
                let (b01, g01, r01) = (r0[i + 4] as i32, r0[i + 5] as i32, r0[i + 6] as i32);
                let (b10, g10, r10) = (r1[i] as i32, r1[i + 1] as i32, r1[i + 2] as i32);
                let (b11, g11, r11) = (r1[i + 4] as i32, r1[i + 5] as i32, r1[i + 6] as i32);
                y0[2 * cx] = luma(r00, g00, b00);
                y0[2 * cx + 1] = luma(r01, g01, b01);
                y1[2 * cx] = luma(r10, g10, b10);
                y1[2 * cx + 1] = luma(r11, g11, b11);
                let (sr, sg, sb) = (r00 + r01 + r10 + r11, g00 + g01 + g10 + g11, b00 + b01 + b10 + b11);
                u_row[cx] = chroma_u(sr, sg, sb);
                v_row[cx] = chroma_v(sr, sg, sb);
            }
        } else {
            let n = (factor * factor) as i32;
            for cx in 0..cw {
                let mut px = [(0i32, 0i32, 0i32); 4];
                for (k, (dy, dx)) in [(0usize, 0usize), (0, 1), (1, 0), (1, 1)].into_iter().enumerate() {
                    let sy = (oy + dy) * factor;
                    let sx = (2 * cx + dx) * factor;
                    let (mut r, mut g, mut b) = (0i32, 0i32, 0i32);
                    for yy in 0..factor {
                        let row = &src[(sy + yy) * stride + sx * 4..][..factor * 4];
                        for p in row.chunks_exact(4) {
                            b += p[0] as i32;
                            g += p[1] as i32;
                            r += p[2] as i32;
                        }
                    }
                    px[k] = (r / n, g / n, b / n);
                }
                y0[2 * cx] = luma(px[0].0, px[0].1, px[0].2);
                y0[2 * cx + 1] = luma(px[1].0, px[1].1, px[1].2);
                y1[2 * cx] = luma(px[2].0, px[2].1, px[2].2);
                y1[2 * cx + 1] = luma(px[3].0, px[3].1, px[3].2);
                let sr = px.iter().map(|p| p.0).sum();
                let sg = px.iter().map(|p| p.1).sum();
                let sb = px.iter().map(|p| p.2).sum();
                u_row[cx] = chroma_u(sr, sg, sb);
                v_row[cx] = chroma_v(sr, sg, sb);
            }
        }
    }
}

#[inline(always)]
fn clamp_u8(v: i32) -> u32 {
    v.clamp(0, 255) as u32
}

/// Converts an I420 picture to `0x00RRGGBB` pixels (the format softbuffer wants).
#[allow(clippy::too_many_arguments)]
pub fn i420_to_rgb32(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    y_stride: usize,
    uv_stride: usize,
    width: usize,
    height: usize,
    out: &mut [u32],
) {
    assert!(out.len() >= width * height);
    if width == 0 || height == 0 {
        return;
    }
    let convert_rows = |first_row: usize, chunk: &mut [u32]| {
        let rows = chunk.len() / width;
        for r in 0..rows {
            let row = first_row + r;
            let y_row = &y[row * y_stride..row * y_stride + width];
            let u_row = &u[(row / 2) * uv_stride..];
            let v_row = &v[(row / 2) * uv_stride..];
            let dst = &mut chunk[r * width..(r + 1) * width];
            for x in 0..width {
                let d = u_row[x / 2] as i32 - 128;
                let e = v_row[x / 2] as i32 - 128;
                let l = y_row[x] as i32;
                let red = clamp_u8(l + ((359 * e + 128) >> 8));
                let green = clamp_u8(l - ((88 * d + 183 * e + 128) >> 8));
                let blue = clamp_u8(l + ((454 * d + 128) >> 8));
                dst[x] = (red << 16) | (green << 8) | blue;
            }
        }
    };

    let threads = worker_count(width * height).min(height.div_ceil(2)).max(1);
    if threads == 1 {
        convert_rows(0, &mut out[..width * height]);
        return;
    }
    // Chunks are a whole number of row pairs so chroma rows aren't split.
    let rows_per_chunk = height.div_ceil(2).div_ceil(threads) * 2;
    std::thread::scope(|scope| {
        for (i, chunk) in out[..width * height].chunks_mut(rows_per_chunk * width).enumerate() {
            let convert_rows = &convert_rows;
            scope.spawn(move || convert_rows(i * rows_per_chunk, chunk));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgra_image(w: usize, h: usize, f: impl Fn(usize, usize) -> (u8, u8, u8)) -> Vec<u8> {
        let mut img = vec![0u8; w * h * 4];
        for yy in 0..h {
            for xx in 0..w {
                let (r, g, b) = f(xx, yy);
                let i = (yy * w + xx) * 4;
                img[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        img
    }

    fn roundtrip(img: &[u8], w: usize, h: usize, factor: usize) -> (I420Buffer, Vec<u32>) {
        let mut yuv = I420Buffer::default();
        bgra_to_i420(img, w, h, w * 4, factor, &mut yuv);
        let mut rgb = vec![0u32; yuv.width * yuv.height];
        i420_to_rgb32(&yuv.y, &yuv.u, &yuv.v, yuv.width, yuv.width / 2, yuv.width, yuv.height, &mut rgb);
        (yuv, rgb)
    }

    #[test]
    fn solid_colors_survive_roundtrip() {
        for &(r, g, b) in &[(0u8, 0u8, 0u8), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255), (12, 200, 99)] {
            let img = bgra_image(64, 48, |_, _| (r, g, b));
            let (_, rgb) = roundtrip(&img, 64, 48, 1);
            let px = rgb[10 * 64 + 10];
            let got = ((px >> 16) as i32, ((px >> 8) & 255) as i32, (px & 255) as i32);
            assert!(
                (got.0 - r as i32).abs() <= 3 && (got.1 - g as i32).abs() <= 3 && (got.2 - b as i32).abs() <= 3,
                "({r},{g},{b}) came back as {got:?}"
            );
        }
    }

    #[test]
    fn large_image_uses_threads_consistently() {
        // A gradient big enough to be split across threads; every row must be converted.
        let (w, h) = (1280, 722);
        let img = bgra_image(w, h, |x, y| ((x % 256) as u8, (y % 256) as u8, 128));
        let (yuv, rgb) = roundtrip(&img, w, h, 1);
        assert_eq!((yuv.width, yuv.height), (1280, 722));
        let px = rgb[(h - 1) * w + 300];
        assert!(((px >> 16) as i32 - 300 % 256).abs() <= 4);
        assert!((((px >> 8) & 255) as i32 - ((h - 1) % 256) as i32).abs() <= 4);
    }

    #[test]
    fn downscale_halves_dimensions() {
        let img = bgra_image(101, 61, |x, _| if x < 50 { (255, 0, 0) } else { (0, 0, 255) });
        let (yuv, rgb) = roundtrip(&img, 101, 61, 2);
        assert_eq!((yuv.width, yuv.height), (50, 30));
        let left = rgb[15 * 50 + 5];
        let right = rgb[15 * 50 + 45];
        assert!((left >> 16) > 240 && (left & 255) < 15);
        assert!((right >> 16) < 15 && (right & 255) > 240);
    }
}
