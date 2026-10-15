//! GDI capture fallback: `BitBlt` the monitor into a DIB section.
//!
//! GDI can't tell us when the screen changed, so each grab is compared with
//! the previous one and unchanged frames are skipped.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use windows::Win32::Graphics::Gdi::{BitBlt, GetDC, ReleaseDC, HDC, SRCCOPY};

use super::win::{monitors, CursorOverlay, CursorState, DibCanvas};
use super::{Capturer, Frame, Rect};

pub struct GdiCapturer {
    screen: HDC,
    canvas: DibCanvas,
    /// The previous grab without the cursor, for change detection.
    previous: Vec<u8>,
    cursor: CursorOverlay,
    last_cursor: CursorState,
    display: usize,
    rect: Rect,
    has_image: bool,
    /// Used only because DXGI failed (vs. chosen with `--gdi`).
    fallback: bool,
    next_layout_check: Instant,
}

impl GdiCapturer {
    pub fn new(display: usize, fallback: bool) -> Result<Self> {
        let rect = monitor_rect(display)?;
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
            display,
            rect,
            has_image: false,
            fallback,
            next_layout_check: Instant::now() + Duration::from_secs(2),
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

    /// Follows resolution and monitor-layout changes.
    fn check_layout(&mut self) -> Result<()> {
        if Instant::now() < self.next_layout_check {
            return Ok(());
        }
        self.next_layout_check = Instant::now() + Duration::from_secs(2);
        let rect = monitor_rect(self.display)?;
        if rect != self.rect {
            if (rect.width, rect.height) != (self.rect.width, self.rect.height) {
                self.canvas = DibCanvas::new(rect.width as usize, rect.height as usize)?;
            }
            self.rect = rect;
            self.previous.clear();
            self.has_image = false;
        }
        Ok(())
    }
}

fn monitor_rect(display: usize) -> Result<Rect> {
    let all = monitors();
    all.get(display).copied().ok_or_else(|| anyhow!("display {display} not found ({} attached)", all.len()))
}

impl Capturer for GdiCapturer {
    fn name(&self) -> &'static str {
        "GDI BitBlt"
    }

    fn display_rect(&self) -> Rect {
        self.rect
    }

    fn is_fallback(&self) -> bool {
        self.fallback
    }

    fn capture(&mut self, _timeout: Duration) -> Result<Option<Frame<'_>>> {
        self.check_layout()?;
        // Plain SRCCOPY: with desktop composition (Windows 8+) the screen DC
        // already includes layered windows, and CAPTUREBLT makes the host's
        // own mouse cursor flicker.
        unsafe {
            BitBlt(
                self.canvas.dc(),
                0,
                0,
                self.rect.width,
                self.rect.height,
                Some(self.screen),
                self.rect.left,
                self.rect.top,
                SRCCOPY,
            )
            .context("BitBlt")?;
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
