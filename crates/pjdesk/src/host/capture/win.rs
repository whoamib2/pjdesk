//! Windows helpers shared by the capture backends: monitor enumeration, a GDI
//! DIB-section canvas, and mouse-cursor compositing.

use std::ffi::c_void;
use std::ptr::null_mut;

use anyhow::{bail, Result};
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GdiFlush, GetDC,
    GetMonitorInfoW, GetObjectW, ReleaseDC, SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HCURSOR, HICON, ICONINFO,
    MONITORINFOF_PRIMARY,
};

use super::Rect;

/// One attached display, primary sorted first (the `--display N` order).
pub struct MonitorDetail {
    pub rect: Rect,
    pub primary: bool,
    pub name: String,
    pub dpi: u32,
}

/// All monitors: the primary first, then left to right (this is the order of
/// `--display N`).
pub fn monitors() -> Vec<Rect> {
    monitor_details().into_iter().map(|d| d.rect).collect()
}

/// Every attached display, with its virtual-desktop position and DPI.
pub fn monitor_details() -> Vec<MonitorDetail> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _dc: HDC, _clip: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<MonitorDetail>) };
        let mut info = MONITORINFOEXW {
            monitorInfo: windows::Win32::Graphics::Gdi::MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
            let r = info.monitorInfo.rcMonitor;
            let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
            let mut dpi_x = 0u32;
            let mut dpi_y = 0u32;
            let dpi = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
                .ok()
                .map(|_| dpi_x.max(1))
                .unwrap_or(0);
            let name = device_name(&info.szDevice);
            list.push(MonitorDetail {
                rect: Rect { left: r.left, top: r.top, width: r.right - r.left, height: r.bottom - r.top },
                primary,
                name,
                dpi,
            });
        }
        BOOL(1)
    }
    let mut list: Vec<MonitorDetail> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by_key(|d| (!d.primary, d.rect.left, d.rect.top));
    for (i, d) in list.iter_mut().enumerate() {
        if d.name.is_empty() {
            d.name = format!("Display {}", i + 1);
        }
    }
    list
}

fn device_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|c| *c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end]).trim().to_string()
}

/// `major.minor.build` from `RtlGetVersion` (not the compatibility-shimmed
/// `GetVersionEx`, which still reports Windows 8).
pub fn windows_version() -> String {
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    unsafe {
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        if RtlGetVersion(&mut info).is_ok() {
            format!("{}.{}.{}", info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber)
        } else {
            String::new()
        }
    }
}

/// Primary first, then by position. Shared with the DXGI output ordering.
pub fn sort_displays<T>(list: &mut [(Rect, T)])
where
    T: Copy + Into<bool>,
{
    list.sort_by_key(|(rect, primary)| (!(*primary).into(), rect.left, rect.top));
}

/// A 32-bit top-down DIB section: memory we can both write directly and draw
/// on with GDI (used to composite the cursor).
pub struct DibCanvas {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    pub width: usize,
    pub height: usize,
}

impl DibCanvas {
    pub fn new(width: usize, height: usize) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("empty display");
        }
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            if dc.is_invalid() {
                bail!("CreateCompatibleDC failed");
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // negative = top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) if !bits.is_null() => bitmap,
                _ => {
                    let _ = DeleteDC(dc);
                    bail!("CreateDIBSection failed for {width}x{height}");
                }
            };
            let previous = SelectObject(dc, bitmap.into());
            Ok(Self { dc, bitmap, previous, bits: bits.cast(), width, height })
        }
    }

    pub fn dc(&self) -> HDC {
        self.dc
    }

    pub fn stride(&self) -> usize {
        self.width * 4
    }

    pub fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bits, self.width * self.height * 4) }
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.width * self.height * 4) }
    }
}

impl Drop for DibCanvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Where the cursor was drawn, to detect cursor-only changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorState {
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub handle: usize,
}

struct SavedArea {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

/// Draws the current cursor onto a canvas, remembering the pixels underneath
/// so the next cursor-only update can undo it without a new desktop image.
#[derive(Default)]
pub struct CursorOverlay {
    /// Last cursor handle and its hotspot/size.
    cached: Option<(usize, i32, i32, i32, i32)>,
    saved: Option<SavedArea>,
}

impl CursorOverlay {
    /// Puts back the pixels covered by the previously drawn cursor.
    pub fn restore(&mut self, canvas: &mut DibCanvas) {
        if let Some(area) = self.saved.take() {
            let stride = canvas.stride();
            let pixels = canvas.pixels_mut();
            for row in 0..area.height {
                let dst = (area.y + row) * stride + area.x * 4;
                pixels[dst..dst + area.width * 4]
                    .copy_from_slice(&area.pixels[row * area.width * 4..(row + 1) * area.width * 4]);
            }
        }
    }

    /// The canvas was fully overwritten; there is nothing to restore.
    pub fn invalidate(&mut self) {
        self.saved = None;
    }

    /// Draws the cursor; `origin` is the desktop position of canvas pixel (0, 0).
    pub fn draw(&mut self, canvas: &mut DibCanvas, origin: (i32, i32)) -> CursorState {
        let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if unsafe { GetCursorInfo(&mut info) }.is_err()
            || info.flags.0 & CURSOR_SHOWING.0 == 0
            || info.hCursor.is_invalid()
        {
            return CursorState::default();
        }
        let (hot_x, hot_y, cw, ch) = self.metrics(info.hCursor);
        let x = info.ptScreenPos.x - origin.0 - hot_x;
        let y = info.ptScreenPos.y - origin.1 - hot_y;
        let state = CursorState { visible: true, x, y, handle: info.hCursor.0 as usize };

        // Clip the cursor box to the canvas; skip if it's on another monitor.
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + cw).min(canvas.width as i32), (y + ch).min(canvas.height as i32));
        if x0 >= x1 || y0 >= y1 {
            // On another monitor: not visible here, so moving it around
            // there doesn't count as a change of this display.
            return CursorState::default();
        }
        let area_w = (x1 - x0) as usize;
        let area_h = (y1 - y0) as usize;
        let stride = canvas.stride();
        let mut saved = Vec::with_capacity(area_w * area_h * 4);
        {
            let pixels = canvas.pixels();
            for row in 0..area_h {
                let src = (y0 as usize + row) * stride + x0 as usize * 4;
                saved.extend_from_slice(&pixels[src..src + area_w * 4]);
            }
        }
        self.saved = Some(SavedArea { x: x0 as usize, y: y0 as usize, width: area_w, height: area_h, pixels: saved });

        unsafe {
            let _ = DrawIconEx(canvas.dc(), x, y, HICON(info.hCursor.0), 0, 0, 0, None, DI_NORMAL);
            // Make sure GDI has finished writing before we read the pixels.
            let _ = GdiFlush();
        }
        state
    }

    /// Hotspot and size of a cursor, cached per handle.
    fn metrics(&mut self, cursor: HCURSOR) -> (i32, i32, i32, i32) {
        let key = cursor.0 as usize;
        if let Some((cached, hx, hy, w, h)) = self.cached {
            if cached == key {
                return (hx, hy, w, h);
            }
        }
        let mut result = (0, 0, 32, 32);
        unsafe {
            let mut icon = ICONINFO::default();
            if GetIconInfo(HICON(cursor.0), &mut icon).is_ok() {
                let mut bitmap = BITMAP::default();
                let has_color = !icon.hbmColor.is_invalid();
                let measured = if has_color { icon.hbmColor } else { icon.hbmMask };
                if GetObjectW(
                    measured.into(),
                    std::mem::size_of::<BITMAP>() as i32,
                    Some(&mut bitmap as *mut _ as *mut c_void),
                ) != 0
                {
                    // Monochrome cursors stack the AND and XOR masks vertically.
                    let height = if has_color { bitmap.bmHeight } else { bitmap.bmHeight / 2 };
                    result = (icon.xHotspot as i32, icon.yHotspot as i32, bitmap.bmWidth, height);
                }
                if !icon.hbmMask.is_invalid() {
                    let _ = DeleteObject(icon.hbmMask.into());
                }
                if has_color {
                    let _ = DeleteObject(icon.hbmColor.into());
                }
            }
        }
        self.cached = Some((key, result.0, result.1, result.2, result.3));
        result
    }
}
