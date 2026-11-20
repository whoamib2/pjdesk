//! Sends the host's mouse pointer to the controller as an image (0.6.86), so
//! the controller shows exactly one pointer, moved locally with no lag, in the
//! host's shape (RustDesk style). The video no longer carries the pointer for
//! that session.
//!
//! Standard system pointers (resize arrows, I-beam, and the rest) are also
//! named in [`HostMsg::CursorShape::id`] so a 0.7.0+ viewer can use its own
//! native cursor. The RGBA payload stays in the message for older viewers.
//! Android does not use this path: an Android viewer never asks for
//! `CursorShapes`, and an Android host has no Win32 pointer to send.

/// Recovers straight RGBA from the pointer drawn once on black and once on
/// white (BGRA rows). Colour, alpha, monochrome and inverting pixels are all
/// handled this way. Invert pixels are drawn black with a white outline so
/// they stay visible on light and dark backgrounds.
pub fn rgba_from_black_white(black: &[u8], white: &[u8], width: u32, height: u32) -> Vec<u8> {
    let n = width as usize * height as usize;
    let mut out = vec![0u8; n * 4];
    let mut invert = vec![false; n];
    for (i, (b, w)) in black.chunks_exact(4).zip(white.chunks_exact(4)).enumerate().take(n) {
        let pos = |c: usize| (w[c] as i32 - b[c] as i32).max(0);
        let neg = |c: usize| (b[c] as i32 - w[c] as i32).max(0);
        let to_white = (pos(0) + pos(1) + pos(2)) / 3;
        let to_black = (neg(0) + neg(1) + neg(2)) / 3;
        if to_white >= 192 && to_black <= 32 {
            continue;
        }
        if to_black >= 192 && to_white <= 32 {
            invert[i] = true;
            continue;
        }
        let alpha = (255 - to_white).clamp(0, 255);
        let un = |c: u8| if alpha == 0 { 0 } else { ((c as i32 * 255 + alpha / 2) / alpha).min(255) as u8 };
        if alpha == 0 {
            continue;
        }
        let o = i * 4;
        out[o] = un(b[2]);
        out[o + 1] = un(b[1]);
        out[o + 2] = un(b[0]);
        out[o + 3] = alpha as u8;
    }
    apply_invert_outline(&mut out, width, height, &invert);
    out
}

/// Builds straight RGBA from monochrome AND / XOR masks (`width * height`
/// bytes, a non-zero byte is bit 1). Masks use the Win32 convention:
/// (AND=0, XOR=0) black, (0, 1) white, (1, 0) transparent, (1, 1) invert.
pub fn rgba_from_and_xor(width: u32, height: u32, and_bits: &[u8], xor_bits: &[u8]) -> Vec<u8> {
    let n = width as usize * height as usize;
    let mut out = vec![0u8; n * 4];
    let mut invert = vec![false; n];
    for i in 0..n {
        let and = and_bits.get(i).copied().unwrap_or(0) != 0;
        let xor = xor_bits.get(i).copied().unwrap_or(0) != 0;
        match (and, xor) {
            (false, false) => out[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]),
            (false, true) => out[i * 4..i * 4 + 4].copy_from_slice(&[255, 255, 255, 255]),
            (true, false) => {}
            (true, true) => invert[i] = true,
        }
    }
    apply_invert_outline(&mut out, width, height, &invert);
    out
}

/// Invert pixels become black. Each transparent neighbour of an invert
/// pixel becomes a white outline pixel.
pub fn apply_invert_outline(rgba: &mut [u8], width: u32, height: u32, invert: &[bool]) {
    let w = width as usize;
    let h = height as usize;
    let n = w.saturating_mul(h);
    if n == 0 || invert.len() < n || rgba.len() < n * 4 {
        return;
    }
    for (i, &inv) in invert.iter().take(n).enumerate() {
        if inv {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
    }
    let mut outline = vec![false; n];
    for y in 0..h {
        for x in 0..w {
            if !invert[y * w + x] {
                continue;
            }
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !invert[j] && rgba[j * 4 + 3] == 0 {
                        outline[j] = true;
                    }
                }
            }
        }
    }
    for (i, on) in outline.iter().enumerate() {
        if *on {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
}

/// Whether the image has at least one visible pixel.
pub fn rgba_is_visible(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4).any(|p| p[3] > 0)
}

#[cfg(windows)]
pub use win::spawn;

#[cfg(not(windows))]
pub fn spawn(_tx: tokio::sync::mpsc::UnboundedSender<pjdesk_proto::peer::HostMsg>) {}

#[cfg(windows)]
mod win {
    #[cfg(test)]
    use std::path::Path;
    use std::time::{Duration, Instant};

    use pjdesk_proto::peer::{standard_cursor_id, HostMsg, StandardCursor, CURSOR_MAX_SIDE};
    use tokio::sync::mpsc::UnboundedSender;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    use crate::host::inputroute::DeskFollower;

    /// Polls the pointer every 16 ms on its own thread (attached to the input
    /// desktop, so UAC and the lock screen pointers come through too) and sends
    /// a shape whenever it changes. Ends when the session's sender closes.
    pub fn spawn(tx: UnboundedSender<HostMsg>) {
        let _ = std::thread::Builder::new().name("pjdesk-cursor".into()).spawn(move || run(tx));
    }

    struct SystemCursors {
        desktop: Option<String>,
        handles: Vec<(usize, StandardCursor)>,
    }

    impl SystemCursors {
        fn empty() -> Self {
            Self { desktop: None, handles: Vec::new() }
        }

        fn refresh(&mut self, desktop: Option<String>) {
            if self.desktop == desktop && !self.handles.is_empty() {
                return;
            }
            self.desktop = desktop;
            self.handles = load_system_cursors();
        }

        fn classify(&self, cursor: HCURSOR) -> Option<StandardCursor> {
            let key = cursor.0 as usize;
            self.handles.iter().find(|(h, _)| *h == key).map(|(_, k)| *k)
        }
    }

    fn load_system_cursors() -> Vec<(usize, StandardCursor)> {
        const MAP: &[(windows::core::PCWSTR, StandardCursor)] = &[
            (IDC_ARROW, StandardCursor::Arrow),
            (IDC_IBEAM, StandardCursor::IBeam),
            (IDC_WAIT, StandardCursor::Wait),
            (IDC_CROSS, StandardCursor::Cross),
            (IDC_UPARROW, StandardCursor::UpArrow),
            (IDC_SIZENWSE, StandardCursor::SizeNwse),
            (IDC_SIZENESW, StandardCursor::SizeNesw),
            (IDC_SIZEWE, StandardCursor::SizeWe),
            (IDC_SIZENS, StandardCursor::SizeNs),
            (IDC_SIZEALL, StandardCursor::SizeAll),
            (IDC_NO, StandardCursor::No),
            (IDC_HAND, StandardCursor::Hand),
            (IDC_APPSTARTING, StandardCursor::AppStarting),
            (IDC_HELP, StandardCursor::Help),
        ];
        let mut out = Vec::new();
        for (id, kind) in MAP {
            if let Ok(h) = unsafe { LoadCursorW(None, *id) } {
                if !h.is_invalid() {
                    out.push((h.0 as usize, *kind));
                }
            }
        }
        out
    }

    fn run(tx: UnboundedSender<HostMsg>) {
        let mut desk = DeskFollower::default();
        let mut last: Option<(bool, usize)> = None;
        let mut system = SystemCursors::empty();
        while !tx.is_closed() {
            let now = Instant::now();
            if desk.should_attach(now, crate::platform::input_desktop_name) {
                crate::platform::attach_thread_to_input_desktop();
                desk.attached(now, crate::platform::input_desktop_name());
                last = None; // resend on the new desktop
                system.handles.clear();
            }
            system.refresh(crate::platform::input_desktop_name());
            let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
            if unsafe { GetCursorInfo(&mut info) }.is_ok() {
                let showing = info.flags.0 & CURSOR_SHOWING.0 != 0 && !info.hCursor.is_invalid();
                let key = (showing, info.hCursor.0 as usize);
                if last != Some(key) {
                    if showing {
                        // A failed capture is not a hidden pointer. Skip and
                        // retry next poll; sending width 0 made the viewer
                        // drop the cursor on Cabin1 and similar hosts.
                        if let Some(msg) = shape(info.hCursor, system.classify(info.hCursor)) {
                            if tx.send(msg).is_err() {
                                return;
                            }
                            last = Some(key);
                        }
                    } else {
                        let msg = HostMsg::CursorShape {
                            id: 0,
                            width: 0,
                            height: 0,
                            hot_x: 0,
                            hot_y: 0,
                            rgba: Vec::new(),
                        };
                        if tx.send(msg).is_err() {
                            return;
                        }
                        last = Some(key);
                    }
                }
            } else {
                desk.invalidate();
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn shape(cursor: HCURSOR, standard: Option<StandardCursor>) -> Option<HostMsg> {
        unsafe {
            let copy = CopyIcon(HICON(cursor.0)).ok()?;
            if copy.is_invalid() {
                return None;
            }
            let result = shape_icon(HCURSOR(copy.0), standard);
            let _ = DestroyIcon(copy);
            result
        }
    }

    unsafe fn shape_icon(cursor: HCURSOR, standard: Option<StandardCursor>) -> Option<HostMsg> {
        let mut icon = ICONINFO::default();
        if GetIconInfo(HICON(cursor.0), &mut icon).is_err() {
            return pack(standard, cursor.0 as usize as u64, 1, 1, 0, 0, vec![0, 0, 0, 0]);
        }
        let has_color = !icon.hbmColor.is_invalid();
        let (hot_x, hot_y) = (icon.xHotspot, icon.yHotspot);
        let mut rgba = None;
        let mut width = 1i32;
        let mut height = 1i32;
        if !has_color && !icon.hbmMask.is_invalid() {
            if let Some((w, h, pixels)) = bitmap_bgra(icon.hbmMask) {
                // Monochrome cursors stack the AND mask on top of the XOR mask.
                if h >= 2 {
                    let w = w.clamp(1, CURSOR_MAX_SIDE as i32);
                    let vis_h = (h / 2).clamp(1, CURSOR_MAX_SIDE as i32);
                    let (and_bits, xor_bits) = split_mono_masks(&pixels, w as usize, vis_h as usize);
                    let built = super::rgba_from_and_xor(w as u32, vis_h as u32, &and_bits, &xor_bits);
                    if super::rgba_is_visible(&built) {
                        width = w;
                        height = vis_h;
                        rgba = Some(built);
                    }
                }
            }
        }
        if rgba.is_none() {
            let mut bm = BITMAP::default();
            let measured = if has_color { icon.hbmColor } else { icon.hbmMask };
            let got = if measured.is_invalid() {
                0
            } else {
                GetObjectW(measured.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _))
            };
            if got != 0 {
                let w = bm.bmWidth.clamp(1, CURSOR_MAX_SIDE as i32);
                let h = (if has_color { bm.bmHeight } else { bm.bmHeight / 2 }).clamp(1, CURSOR_MAX_SIDE as i32);
                if let (Some(black), Some(white)) = (render(cursor, w, h, 0x00), render(cursor, w, h, 0xFF)) {
                    let built = super::rgba_from_black_white(&black, &white, w as u32, h as u32);
                    if super::rgba_is_visible(&built) {
                        width = w;
                        height = h;
                        rgba = Some(built);
                    }
                }
            }
        }
        if rgba.is_none() && has_color && !icon.hbmColor.is_invalid() {
            if let Some((w, h, color)) = bitmap_bgra(icon.hbmColor) {
                let w = w.clamp(1, CURSOR_MAX_SIDE as i32);
                let h = h.clamp(1, CURSOR_MAX_SIDE as i32);
                let mut built = vec![0u8; (w * h * 4) as usize];
                for i in 0..(w * h) as usize {
                    built[i * 4] = color.get(i * 4 + 2).copied().unwrap_or(0);
                    built[i * 4 + 1] = color.get(i * 4 + 1).copied().unwrap_or(0);
                    built[i * 4 + 2] = color.get(i * 4).copied().unwrap_or(0);
                    built[i * 4 + 3] = color.get(i * 4 + 3).copied().unwrap_or(0);
                }
                if !super::rgba_is_visible(&built) && !icon.hbmMask.is_invalid() {
                    if let Some((_, mh, mask)) = bitmap_bgra(icon.hbmMask) {
                        let mask_h = if mh >= h * 2 { h } else { mh.min(h) };
                        for y in 0..h as usize {
                            if y >= mask_h as usize {
                                break;
                            }
                            for x in 0..w as usize {
                                let i = y * w as usize + x;
                                // Mask white (bit 1) is transparent; black is opaque.
                                let trans = mask.get((y * w as usize + x) * 4).copied().unwrap_or(0) > 127;
                                built[i * 4 + 3] = if trans { 0 } else { 255 };
                            }
                        }
                    }
                }
                if super::rgba_is_visible(&built) {
                    width = w;
                    height = h;
                    rgba = Some(built);
                }
            }
        }
        if !icon.hbmMask.is_invalid() {
            let _ = DeleteObject(icon.hbmMask.into());
        }
        if has_color {
            let _ = DeleteObject(icon.hbmColor.into());
        }
        let rgba = match rgba {
            Some(p) => p,
            None => return pack(standard, cursor.0 as usize as u64, 1, 1, 0, 0, vec![0, 0, 0, 0]),
        };
        let hx = (hot_x as i32).clamp(0, width - 1) as u16;
        let hy = (hot_y as i32).clamp(0, height - 1) as u16;
        pack(standard, cursor.0 as usize as u64, width as u16, height as u16, hx, hy, rgba)
    }

    fn pack(
        standard: Option<StandardCursor>,
        handle_id: u64,
        width: u16,
        height: u16,
        hot_x: u16,
        hot_y: u16,
        rgba: Vec<u8>,
    ) -> Option<HostMsg> {
        let visible = super::rgba_is_visible(&rgba);
        if !visible && standard.is_none() {
            return None;
        }
        let id = match standard {
            Some(kind) => standard_cursor_id(kind),
            None => handle_id,
        };
        // A standard pointer can travel without a usable bitmap: new viewers
        // use the local cursor. Older viewers ignore an empty 1x1 image and
        // keep the last shape, which is better than hiding the pointer.
        Some(HostMsg::CursorShape { id, width, height, hot_x, hot_y, rgba })
    }

    fn split_mono_masks(bgra: &[u8], width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
        let mut and_bits = vec![0u8; width * height];
        let mut xor_bits = vec![0u8; width * height];
        let stride = width * 4;
        for y in 0..height {
            for x in 0..width {
                let and_i = y * stride + x * 4;
                let xor_i = (y + height) * stride + x * 4;
                // 32-bit expansion of a 1-bpp mask: white (255) is bit 1.
                if bgra.get(and_i).copied().unwrap_or(0) > 127 {
                    and_bits[y * width + x] = 1;
                }
                if bgra.get(xor_i).copied().unwrap_or(0) > 127 {
                    xor_bits[y * width + x] = 1;
                }
            }
        }
        (and_bits, xor_bits)
    }

    unsafe fn bitmap_bgra(hbm: HBITMAP) -> Option<(i32, i32, Vec<u8>)> {
        let mut bm = BITMAP::default();
        if GetObjectW(hbm.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return None;
        }
        let w = bm.bmWidth;
        let h = bm.bmHeight.abs();
        if w <= 0 || h <= 0 {
            return None;
        }
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = vec![0u8; (w * h * 4) as usize];
        let got = GetDIBits(dc, hbm, 0, h as u32, Some(bits.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        if got == 0 {
            return None;
        }
        Some((w, h, bits))
    }

    /// The pointer drawn on a `w`×`h` background of `fill`, as BGRA rows.
    unsafe fn render(cursor: HCURSOR, w: i32, h: i32, fill: u8) -> Option<Vec<u8>> {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let _ = ReleaseDC(None, screen);
        if dc.is_invalid() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old = SelectObject(dc, dib.into());
        let len = (w * h * 4) as usize;
        std::ptr::write_bytes(bits as *mut u8, fill, len);
        let _ = DrawIconEx(dc, 0, 0, HICON(cursor.0), w, h, 0, None, DI_NORMAL);
        let _ = GdiFlush();
        let out = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(dc);
        Some(out)
    }

    /// Converts each standard system cursor and writes checkerboard PNGs into
    /// `dir`. Used to prove resize arrows and the I-beam come through.
    #[cfg(test)]
    pub fn write_system_cursor_proofs(dir: &Path) -> std::io::Result<Vec<String>> {
        std::fs::create_dir_all(dir)?;
        let mut lines = Vec::new();
        for (handle, kind) in load_system_cursors() {
            let Some(HostMsg::CursorShape { id, width, height, rgba, .. }) =
                shape(HCURSOR(handle as *mut _), Some(kind))
            else {
                lines.push(format!("{kind:?}: capture failed"));
                continue;
            };
            let visible = super::rgba_is_visible(&rgba);
            let name = format!("{kind:?}").to_ascii_lowercase();
            let file = dir.join(format!("{name}.png"));
            let composed = composite_checker(width as u32, height as u32, &rgba);
            if let Some(png) = crate::clip::encode_png(width.max(1) as u32, height.max(1) as u32, &composed) {
                std::fs::write(&file, png)?;
            }
            lines.push(format!(
                "{kind:?} id=0x{id:x} {width}x{height} visible={visible} file={}",
                file.display()
            ));
        }
        std::fs::write(dir.join("report.txt"), lines.join("\n") + "\n")?;
        Ok(lines)
    }

    #[cfg(test)]
    fn composite_checker(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let w = width.max(1) as usize;
        let h = height.max(1) as usize;
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let cell = ((x / 4) + (y / 4)) % 2;
                let g = if cell == 0 { 180 } else { 80 };
                let i = (y * w + x) * 4;
                let (r, g2, b, a) = if i + 3 < rgba.len() {
                    (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3])
                } else {
                    (0, 0, 0, 0)
                };
                let a = a as u16;
                out[i] = ((r as u16 * a + g as u16 * (255 - a)) / 255) as u8;
                out[i + 1] = ((g2 as u16 * a + g as u16 * (255 - a)) / 255) as u8;
                out[i + 2] = ((b as u16 * a + g as u16 * (255 - a)) / 255) as u8;
                out[i + 3] = 255;
            }
        }
        out
    }
}

#[cfg(all(windows, test))]
pub fn write_system_cursor_proofs(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    win::write_system_cursor_proofs(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_transparent_and_half_pixels_come_back() {
        // BGRA: opaque red, transparent, 50% white, inverting (white on black, black on white).
        let black = [0, 0, 255, 0, 0, 0, 0, 0, 128, 128, 128, 0, 255, 255, 255, 0];
        let white = [0, 0, 255, 0, 255, 255, 255, 0, 255, 255, 255, 0, 0, 0, 0, 0];
        let out = rgba_from_black_white(&black, &white, 4, 1);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
        assert_eq!(out[11], 128);
        assert!(out[8] >= 254);
        // Invert becomes black (a 1-row image has no transparent neighbour for an outline).
        assert_eq!(&out[12..16], &[0, 0, 0, 255]);
    }

    #[test]
    fn monochrome_masks_convert_black_white_transparent_and_invert() {
        // 2x2: black, white / transparent, invert
        let and_bits = [0, 0, 1, 1];
        let xor_bits = [0, 1, 0, 1];
        let out = rgba_from_and_xor(2, 2, &and_bits, &xor_bits);
        assert_eq!(&out[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out[4..8], &[255, 255, 255, 255]);
        // Invert at (1,1) outlines its transparent neighbour at (0,1).
        assert_eq!(&out[8..12], &[255, 255, 255, 255]);
        assert_eq!(&out[12..16], &[0, 0, 0, 255]);
    }

    #[test]
    fn invert_pixels_get_a_white_outline() {
        // 3x3 with a single invert pixel in the centre.
        let mut and_bits = [1u8; 9];
        let mut xor_bits = [0u8; 9];
        and_bits[4] = 1;
        xor_bits[4] = 1;
        let out = rgba_from_and_xor(3, 3, &and_bits, &xor_bits);
        assert_eq!(&out[4 * 4..4 * 4 + 4], &[0, 0, 0, 255]);
        for i in [0, 1, 2, 3, 5, 6, 7, 8] {
            assert_eq!(&out[i * 4..i * 4 + 4], &[255, 255, 255, 255], "outline at {i}");
        }
    }

    #[test]
    fn a_size_nwse_style_mask_is_visible() {
        // 7x7 diagonal resize arrow: white outline around a black diagonal.
        let mut and_bits = [1u8; 49];
        let mut xor_bits = [0u8; 49];
        let paint = |x: i32, y: i32, and: u8, xor: u8, and_bits: &mut [u8], xor_bits: &mut [u8]| {
            if (0..7).contains(&x) && (0..7).contains(&y) {
                let i = (y * 7 + x) as usize;
                and_bits[i] = and;
                xor_bits[i] = xor;
            }
        };
        for i in 1..6 {
            paint(i, i, 0, 0, &mut and_bits, &mut xor_bits);
            paint(i + 1, i, 0, 1, &mut and_bits, &mut xor_bits);
            paint(i, i + 1, 0, 1, &mut and_bits, &mut xor_bits);
            paint(i - 1, i, 0, 1, &mut and_bits, &mut xor_bits);
            paint(i, i - 1, 0, 1, &mut and_bits, &mut xor_bits);
        }
        let out = rgba_from_and_xor(7, 7, &and_bits, &xor_bits);
        assert!(rgba_is_visible(&out));
        let black = out.chunks_exact(4).filter(|p| p == &[0, 0, 0, 255]).count();
        let white = out.chunks_exact(4).filter(|p| p == &[255, 255, 255, 255]).count();
        assert!(black >= 5, "black={black}");
        assert!(white >= 8, "white={white}");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use pjdesk_proto::peer::{standard_cursor, StandardCursor, CURSOR_STANDARD_FLAG};

    #[test]
    fn standard_system_cursors_are_named_and_visible() {
        let dir = std::env::var_os("PJDESK_CURSOR_SHOTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("pjdesk-cursor-shots"));
        let lines = super::write_system_cursor_proofs(&dir).expect("write cursor proofs");
        let report = lines.join("\n");
        for kind in [
            StandardCursor::SizeNwse,
            StandardCursor::SizeNesw,
            StandardCursor::SizeWe,
            StandardCursor::SizeNs,
            StandardCursor::SizeAll,
            StandardCursor::IBeam,
            StandardCursor::Arrow,
        ] {
            let row = lines.iter().find(|l| l.starts_with(&format!("{kind:?}"))).unwrap_or_else(|| panic!("missing {kind:?} in {report}"));
            assert!(row.contains("visible=true"), "{row}");
            assert!(row.contains(&format!("id=0x{:x}", CURSOR_STANDARD_FLAG | kind as u8 as u64)) || standard_cursor_line(row).is_some(), "{row}");
        }
    }

    fn standard_cursor_line(row: &str) -> Option<StandardCursor> {
        let hex = row.split("id=0x").nth(1)?.split_whitespace().next()?;
        let id = u64::from_str_radix(hex, 16).ok()?;
        standard_cursor(id)
    }
}
