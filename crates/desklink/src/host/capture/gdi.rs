//! GDI capture fallback: `BitBlt` the monitor into a DIB section.
//!
//! GDI can't tell us when the screen changed, so each grab is compared with
//! the previous one and unchanged frames are skipped.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use windows::Win32::Graphics::Gdi::{BitBlt, GetDC, ReleaseDC, CAPTUREBLT, HDC, ROP_CODE, SRCCOPY};

use super::win::{monitors, CursorOverlay, CursorState, DibCanvas};
use super::{Capturer, Frame, Rect};

pub struct GdiCapturer {
    screen: HDC,
    canvas: DibCanvas,
    /// The previous grab without the cursor, for change detection.
    previous: Vec<u8>,
    cursor: CursorOverlay,
    last_cursor: CursorState,
    rect: Rect,
    has_image: bool,
    use_captureblt: bool,
}

impl GdiCapturer {
    pub fn new(display: usize) -> Result<Self> {
        let all = monitors();
        let rect = *all.get(display).ok_or_else(|| anyhow!("display {display} not found ({} attached)", all.len()))?;
        let screen = unsafe { GetDC(None) };
        if screen.is_invalid() {
            return Err(anyhow!("GetDC failed"));
        }
        Ok(Self {
            screen,
            canvas: DibCanvas::new(rect.width as usize, rect.height as usize)?,
            previous: Vec::new(),
            cursor: CursorOverlay::default(),
            last_cursor: CursorState::default(),
            rect,
            has_image: false,
            use_captureblt: true,
        })
    }

    fn frame(&self) -> Frame<'_> {
        Frame {
            data: self.canvas.pixels(),
            width: self.canvas.width,
            height: self.canvas.height,
            stride: self.canvas.stride(),
        }
    }
}

impl Capturer for GdiCapturer {
    fn name(&self) -> &'static str {
        "GDI BitBlt"
    }

    fn display_rect(&self) -> Rect {
        self.rect
    }

    fn capture(&mut self, _timeout: Duration) -> Result<Option<Frame<'_>>> {
        let grab = |rop: ROP_CODE| unsafe {
            BitBlt(
                self.canvas.dc(),
                0,
                0,
                self.rect.width,
                self.rect.height,
                Some(self.screen),
                self.rect.left,
                self.rect.top,
                rop,
            )
        };
        // CAPTUREBLT includes layered (semi-transparent) windows such as
        // tooltips and menus; some drivers don't support it, so retry without.
        if self.use_captureblt && grab(ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0)).is_err() {
            self.use_captureblt = false;
        }
        if !self.use_captureblt {
            grab(SRCCOPY).context("BitBlt")?;
        }
        let image_changed = self.previous.as_slice() != self.canvas.pixels();
        if image_changed {
            self.previous.clear();
            self.previous.extend_from_slice(self.canvas.pixels());
        }
        // The grab overwrote the whole canvas, so there's nothing to restore.
        self.cursor.invalidate();
        let cursor = self.cursor.draw(&mut self.canvas, (self.rect.left, self.rect.top));
        if !image_changed && self.has_image && cursor == self.last_cursor {
            return Ok(None);
        }
        self.last_cursor = cursor;
        self.has_image = true;
        Ok(Some(self.frame()))
    }
}

impl Drop for GdiCapturer {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(None, self.screen);
        }
    }
}
