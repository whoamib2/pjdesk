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
}

/// Opens the best available capturer for the host options.
pub fn open(args: &HostArgs) -> Result<Box<dyn Capturer>> {
    if args.test_pattern {
        return Ok(Box::new(synthetic::Synthetic::new(1280, 720)));
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

/// Tries the preferred backend again (used to leave the GDI fallback).
pub fn try_preferred(args: &HostArgs) -> Option<Box<dyn Capturer>> {
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
