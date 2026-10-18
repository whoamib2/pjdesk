//! Screen capture backends.
//!
//! * [`dxgi`] – DXGI Desktop Duplication (Windows 8+): the GPU tells us when
//!   the desktop changes and hands over the image; very low CPU use.
//! * [`gdi`] – `BitBlt` from the screen DC: slower, but works where
//!   duplication doesn't (some VMs, RDP sessions, rotated displays).
//! * [`synthetic`] – a moving test pattern, for testing without a desktop.
//!
//! Both Windows backends composite the mouse cursor into the image, since
//! neither API includes it.

#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
mod gdi;
pub mod synthetic;
#[cfg(windows)]
mod win;

use std::time::Duration;

use anyhow::Result;
use tracing::warn;

use crate::HostArgs;

/// A rectangle in virtual-desktop pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// `Session.display` value meaning "capture every display as one picture".
///
/// Bandwidth:
/// * Single-screen mode captures and encodes only the selected display (DXGI
///   Desktop Duplication when it is available, otherwise one GDI `BitBlt` of
///   that monitor).
/// * All-screens mode is one GDI `BitBlt` of the virtual-desktop bounding box,
///   encoded as one stream. DXGI is per-output, so it is not used for the
///   stitched view. The mouse stays normalized across that one rectangle.
/// * Separate windows pack only the displays that still have a local window
///   into one picture ([`SEPARATE_DISPLAYS`]). Each is its own GDI `BitBlt`;
///   a closed window is not captured. The tiles are described by
///   [`pjdesk_proto::peer::HostMsg::Atlas`] and encoded as one stream. DXGI is
///   not used, because it duplicates one output at a time.
pub const ALL_DISPLAYS: usize = usize::MAX;

/// Capture the displays named by [`pjdesk_proto::peer::ControlMsg::Watch`].
pub const SEPARATE_DISPLAYS: usize = usize::MAX - 1;

/// A captured BGRA image (alpha is unused).
pub struct Frame<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

pub trait Capturer {
    fn name(&self) -> &'static str;

    /// Desktop area the frames show (used to map the controller's mouse).
    fn display_rect(&self) -> Rect;

    /// Waits up to `timeout` for the screen or cursor to change. Returns the
    /// new image, or `None` if nothing changed.
    fn capture(&mut self, timeout: Duration) -> Result<Option<Frame<'_>>>;

    /// True for the GDI fallback that was used only because DXGI failed.
    fn is_fallback(&self) -> bool {
        false
    }

    /// Separate-windows picture: capture size and where each open display sits.
    fn atlas(&self) -> Option<(u16, u16, Vec<pjdesk_proto::peer::AtlasTile>)> {
        None
    }
}

/// Opens the best available capturer for the host options.
pub fn open(args: &HostArgs) -> Result<Box<dyn Capturer>> {
    if args.test_pattern {
        return Ok(Box::new(synthetic::Synthetic::new(1280, 720)));
    }
    if args.display == ALL_DISPLAYS {
        return open_all();
    }
    if args.display == SEPARATE_DISPLAYS {
        return open_separate(&[]);
    }
    #[cfg(windows)]
    {
        if !args.gdi {
            match dxgi::DxgiCapturer::new(args.display) {
                Ok(capturer) => return Ok(Box::new(capturer)),
                Err(e) => warn!("DXGI Desktop Duplication unavailable ({e:#}); using GDI capture"),
            }
        }
        Ok(Box::new(gdi::GdiCapturer::new(args.display, !args.gdi)?))
    }
    #[cfg(not(windows))]
    {
        warn!("Real screen capture is implemented for Windows hosts only; streaming a test pattern");
        Ok(Box::new(synthetic::Synthetic::new(1280, 720)))
    }
}

/// Separate windows: one packed picture of `indexes` only.
pub fn open_separate(indexes: &[u8]) -> Result<Box<dyn Capturer>> {
    #[cfg(windows)]
    {
        Ok(Box::new(gdi::GdiCapturer::selected(indexes)?))
    }
    #[cfg(not(windows))]
    {
        let _ = indexes;
        warn!("Separate-window capture is implemented for Windows hosts; streaming a test pattern");
        Ok(Box::new(synthetic::Synthetic::new(1280, 720)))
    }
}

/// All-screens capture: one picture of the virtual desktop, not DXGI.
fn open_all() -> Result<Box<dyn Capturer>> {
    #[cfg(windows)]
    {
        Ok(Box::new(gdi::GdiCapturer::virtual_desktop()?))
    }
    #[cfg(not(windows))]
    {
        warn!("All-screens capture is implemented for Windows hosts; streaming a test pattern");
        Ok(Box::new(synthetic::Synthetic::new(1920, 1080)))
    }
}

/// One entry per attached monitor, in the same order as `--display`.
pub fn monitor_infos() -> Vec<pjdesk_proto::peer::MonitorInfo> {
    #[cfg(windows)]
    {
        win::monitors()
            .into_iter()
            .enumerate()
            .map(|(i, r)| pjdesk_proto::peer::MonitorInfo {
                index: i as u8,
                width: r.width.clamp(0, u16::MAX as i32) as u16,
                height: r.height.clamp(0, u16::MAX as i32) as u16,
                primary: i == 0,
                name: format!("Monitor {}", i + 1),
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![pjdesk_proto::peer::MonitorInfo {
            index: 0,
            width: 1280,
            height: 720,
            primary: true,
            name: "Test pattern".into(),
        }]
    }
}

/// Displays with position and DPI, primary first. Empty on a host that cannot
/// enumerate (the controller then keeps the single Welcome monitor).
pub fn display_layouts() -> Vec<pjdesk_proto::peer::DisplayLayout> {
    #[cfg(windows)]
    {
        win::monitor_details()
            .into_iter()
            .enumerate()
            .map(|(i, d)| pjdesk_proto::peer::DisplayLayout {
                index: i as u8,
                name: d.name,
                width: d.rect.width.clamp(0, u16::MAX as i32) as u16,
                height: d.rect.height.clamp(0, u16::MAX as i32) as u16,
                x: d.rect.left,
                y: d.rect.top,
                primary: d.primary,
                dpi: d.dpi,
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![pjdesk_proto::peer::DisplayLayout {
            index: 0,
            name: "Test pattern".into(),
            width: 1280,
            height: 720,
            x: 0,
            y: 0,
            primary: true,
            dpi: 96,
        }]
    }
}

/// Family and version string for [`pjdesk_proto::peer::HostMsg::Platform`].
pub fn platform_report() -> (String, String) {
    let family = match std::env::consts::OS {
        "windows" | "macos" | "linux" | "android" => std::env::consts::OS,
        other => other,
    };
    (family.to_string(), os_version())
}

fn os_version() -> String {
    #[cfg(windows)]
    {
        return win::windows_version();
    }
    #[cfg(not(windows))]
    {
        if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
            for key in ["VERSION_ID=", "PRETTY_NAME="] {
                if let Some(line) = text.lines().find(|l| l.starts_with(key)) {
                    let v = line[key.len()..].trim_matches('"').trim();
                    if !v.is_empty() {
                        return v.chars().take(64).collect();
                    }
                }
            }
        }
        String::new()
    }
}

/// Tries the preferred backend again (used to leave the GDI fallback).
pub fn try_preferred(args: &HostArgs) -> Option<Box<dyn Capturer>> {
    if args.display == ALL_DISPLAYS || args.display == SEPARATE_DISPLAYS {
        return None;
    }
    #[cfg(windows)]
    {
        dxgi::DxgiCapturer::new(args.display).ok().map(|c| Box::new(c) as Box<dyn Capturer>)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        None
    }
}
