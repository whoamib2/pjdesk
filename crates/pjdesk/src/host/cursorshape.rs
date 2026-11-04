//! Sends the host's mouse pointer to the controller as an image (0.6.86), so
//! the controller shows exactly one pointer, moved locally with no lag, in the
//! host's shape (RustDesk style). The video no longer carries the pointer for
//! that session.

/// Recovers straight RGBA from the pointer drawn once on black and once on
/// white (BGRA rows). This handles colour, alpha, monochrome and inverting
/// pointers the same way: alpha = 255 - (white - black), colour = black / alpha.
pub fn rgba_from_black_white(black: &[u8], white: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(black.len());
    for (b, w) in black.chunks_exact(4).zip(white.chunks_exact(4)) {
        let diff = |i: usize| (w[i] as i32 - b[i] as i32).max(0);
        let spread = (diff(0) + diff(1) + diff(2)) / 3;
        let alpha = (255 - spread).clamp(0, 255);
        let un = |c: u8| if alpha == 0 { 0 } else { ((c as i32 * 255 + alpha / 2) / alpha).min(255) as u8 };
        if alpha == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            out.extend_from_slice(&[un(b[2]), un(b[1]), un(b[0]), alpha as u8]);
        }
    }
    out
}

#[cfg(windows)]
pub use win::spawn;

#[cfg(not(windows))]
pub fn spawn(_tx: tokio::sync::mpsc::UnboundedSender<pjdesk_proto::peer::HostMsg>) {}

#[cfg(windows)]
mod win {
    use std::time::{Duration, Instant};

    use pjdesk_proto::peer::{HostMsg, CURSOR_MAX_SIDE};
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

    fn run(tx: UnboundedSender<HostMsg>) {
        let mut desk = DeskFollower::default();
        let mut last: Option<(bool, usize)> = None;
        while !tx.is_closed() {
            let now = Instant::now();
            if desk.should_attach(now, crate::platform::input_desktop_name) {
                crate::platform::attach_thread_to_input_desktop();
                desk.attached(now, crate::platform::input_desktop_name());
                last = None; // resend on the new desktop
            }
            let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
            if unsafe { GetCursorInfo(&mut info) }.is_ok() {
                let showing = info.flags.0 & CURSOR_SHOWING.0 != 0 && !info.hCursor.is_invalid();
                let key = (showing, info.hCursor.0 as usize);
                if last != Some(key) {
                    let msg = if showing { shape(info.hCursor) } else { None }.unwrap_or(HostMsg::CursorShape {
                        id: 0,
                        width: 0,
                        height: 0,
                        hot_x: 0,
                        hot_y: 0,
                        rgba: Vec::new(),
                    });
                    if tx.send(msg).is_err() {
                        return;
                    }
                    last = Some(key);
                }
            } else {
                desk.invalidate();
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn shape(cursor: HCURSOR) -> Option<HostMsg> {
        unsafe {
            let mut icon = ICONINFO::default();
            GetIconInfo(HICON(cursor.0), &mut icon).ok()?;
            let has_color = !icon.hbmColor.is_invalid();
            let mut bm = BITMAP::default();
            let measured = if has_color { icon.hbmColor } else { icon.hbmMask };
            let got = GetObjectW(measured.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
            let (hot_x, hot_y) = (icon.xHotspot, icon.yHotspot);
            if !icon.hbmMask.is_invalid() {
                let _ = DeleteObject(icon.hbmMask.into());
            }
            if has_color {
                let _ = DeleteObject(icon.hbmColor.into());
            }
            if got == 0 {
                return None;
            }
            let w = bm.bmWidth.clamp(1, CURSOR_MAX_SIDE as i32);
            let h = (if has_color { bm.bmHeight } else { bm.bmHeight / 2 }).clamp(1, CURSOR_MAX_SIDE as i32);
            let black = render(cursor, w, h, 0x00)?;
            let white = render(cursor, w, h, 0xFF)?;
            let rgba = super::rgba_from_black_white(&black, &white);
            Some(HostMsg::CursorShape {
                id: cursor.0 as usize as u64,
                width: w as u16,
                height: h as u16,
                hot_x: (hot_x as i32).clamp(0, w - 1) as u16,
                hot_y: (hot_y as i32).clamp(0, h - 1) as u16,
                rgba,
            })
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_transparent_and_half_pixels_come_back() {
        // BGRA: opaque red, transparent, 50% white, inverting (white on black, black on white).
        let black = [0, 0, 255, 0, 0, 0, 0, 0, 128, 128, 128, 0, 255, 255, 255, 0];
        let white = [0, 0, 255, 0, 255, 255, 255, 0, 255, 255, 255, 0, 0, 0, 0, 0];
        let out = rgba_from_black_white(&black, &white);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
        assert_eq!(out[11], 128);
        assert!(out[8] >= 254);
        assert_eq!(&out[12..16], &[255, 255, 255, 255]);
    }
}
