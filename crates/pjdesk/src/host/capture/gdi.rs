//! GDI capture fallback: `BitBlt` the monitor into a DIB section.
//!
//! GDI can't tell us when the screen changed, so each grab is compared with
//! the previous one and unchanged frames are skipped.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use windows::Win32::Graphics::Gdi::{BitBlt, GetDC, ReleaseDC, HDC, SRCCOPY};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetSystemMetrics, CURSORINFO, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

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
    /// Capture the whole virtual desktop instead of one monitor.
    all: bool,
    has_image: bool,
    /// Used only because DXGI failed (vs. chosen with `--gdi`).
    fallback: bool,
    next_layout_check: Instant,
    /// Separate windows: each source rectangle and where it sits in the picture.
    sources: Vec<SrcTile>,
    atlas: Option<(u16, u16, Vec<pjdesk_proto::peer::AtlasTile>)>,
}

struct SrcTile {
    src: Rect,
    dx: i32,
    dy: i32,
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
            all: false,
            has_image: false,
            fallback,
            next_layout_check: Instant::now() + Duration::from_secs(2),
            sources: Vec::new(),
            atlas: None,
        })
    }

    /// One `BitBlt` per open display, packed into a single picture with no gaps.
    pub fn selected(indexes: &[u8]) -> Result<Self> {
        let details = super::win::monitor_details();
        let screens: Vec<pjdesk_proto::layout::Screen> = details
            .iter()
            .enumerate()
            .map(|(i, d)| pjdesk_proto::layout::Screen {
                index: i as u8,
                x: d.rect.left,
                y: d.rect.top,
                width: d.rect.width.max(0) as u32,
                height: d.rect.height.max(0) as u32,
                dpi: d.dpi,
            })
            .collect();
        let (width, height, tiles) = pjdesk_proto::layout::pack_row(&screens, indexes);
        if tiles.is_empty() {
            return Err(anyhow!("no open displays to capture"));
        }
        let sources = tiles
            .iter()
            .filter_map(|t| {
                let detail = details.get(t.index as usize)?;
                Some(SrcTile { src: detail.rect, dx: t.x as i32, dy: t.y as i32 })
            })
            .collect::<Vec<_>>();
        let atlas_tiles = tiles
            .iter()
            .map(|t| pjdesk_proto::peer::AtlasTile {
                index: t.index,
                x: t.x.min(u16::MAX as u32) as u16,
                y: t.y.min(u16::MAX as u32) as u16,
                width: t.width.min(u16::MAX as u32) as u16,
                height: t.height.min(u16::MAX as u32) as u16,
            })
            .collect();
        let screen = unsafe { GetDC(None) };
        if screen.is_invalid() {
            return Err(anyhow!("GetDC failed"));
        }
        Ok(Self {
            screen,
            canvas: DibCanvas::new(width as usize, height as usize)?,
            previous: Vec::new(),
            cursor: CursorOverlay::default(),
            last_cursor: CursorState::default(),
            display: super::SEPARATE_DISPLAYS,
            rect: Rect { left: 0, top: 0, width: width as i32, height: height as i32 },
            all: false,
            has_image: false,
            fallback: false,
            next_layout_check: Instant::now() + Duration::from_secs(2),
            sources,
            atlas: Some((width.min(u16::MAX as u32) as u16, height.min(u16::MAX as u32) as u16, atlas_tiles)),
        })
    }

    /// One `BitBlt` of the virtual desktop. `is_fallback` is false so the DXGI
    /// retry does not replace this with a single monitor.
    pub fn virtual_desktop() -> Result<Self> {
        let rect = virtual_rect()?;
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
            display: super::ALL_DISPLAYS,
            rect,
            all: true,
            has_image: false,
            fallback: false,
            next_layout_check: Instant::now() + Duration::from_secs(2),
            sources: Vec::new(),
            atlas: None,
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

    /// Desktop position of canvas pixel (0, 0). For a packed picture this is
    /// the tile under the cursor, so the cursor is drawn on that display only.
    fn cursor_origin(&self) -> (i32, i32) {
        if self.sources.is_empty() {
            return (self.rect.left, self.rect.top);
        }
        let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if unsafe { GetCursorInfo(&mut info) }.is_err() {
            return (self.sources[0].src.left - self.sources[0].dx, self.sources[0].src.top - self.sources[0].dy);
        }
        let (x, y) = (info.ptScreenPos.x, info.ptScreenPos.y);
        for tile in &self.sources {
            let right = tile.src.left + tile.src.width;
            let bottom = tile.src.top + tile.src.height;
            if x >= tile.src.left && x < right && y >= tile.src.top && y < bottom {
                return (tile.src.left - tile.dx, tile.src.top - tile.dy);
            }
        }
        (self.sources[0].src.left - self.sources[0].dx, self.sources[0].src.top - self.sources[0].dy)
    }

    /// Follows resolution and monitor-layout changes.
    fn check_layout(&mut self) -> Result<()> {
        if Instant::now() < self.next_layout_check {
            return Ok(());
        }
        self.next_layout_check = Instant::now() + Duration::from_secs(2);
        if !self.sources.is_empty() {
            return Ok(());
        }
        let rect = if self.all { virtual_rect()? } else { monitor_rect(self.display)? };
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

fn virtual_rect() -> Result<Rect> {
    let rect = unsafe {
        Rect {
            left: GetSystemMetrics(SM_XVIRTUALSCREEN),
            top: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(anyhow!("the virtual desktop has no size"));
    }
    Ok(rect)
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

    fn atlas(&self) -> Option<(u16, u16, Vec<pjdesk_proto::peer::AtlasTile>)> {
        self.atlas.clone()
    }

    fn capture(&mut self, _timeout: Duration) -> Result<Option<Frame<'_>>> {
        self.check_layout()?;
        // Plain SRCCOPY: with desktop composition (Windows 8+) the screen DC
        // already includes layered windows, and CAPTUREBLT makes the host's
        // own mouse cursor flicker.
        if self.sources.is_empty() {
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
        } else {
            self.canvas.pixels_mut().fill(0);
            for tile in &self.sources {
                unsafe {
                    BitBlt(
                        self.canvas.dc(),
                        tile.dx,
                        tile.dy,
                        tile.src.width,
                        tile.src.height,
                        Some(self.screen),
                        tile.src.left,
                        tile.src.top,
                        SRCCOPY,
                    )
                    .context("BitBlt")?;
                }
            }
        }
        let image_changed = self.previous.as_slice() != self.canvas.pixels();
        if image_changed {
            self.previous.clear();
            self.previous.extend_from_slice(self.canvas.pixels());
        }
        // The grab overwrote the whole canvas, so there's nothing to restore.
        self.cursor.invalidate();
        let origin = self.cursor_origin();
        let cursor = self.cursor.draw(&mut self.canvas, origin);
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
