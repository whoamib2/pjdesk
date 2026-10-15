//! DXGI Desktop Duplication capture.
//!
//! The GPU compositor hands us the desktop image only when something changed
//! (`AcquireNextFrame` blocks until then), so an idle screen costs nothing.
//! Each new image is copied GPU → staging texture → CPU, then the cursor is
//! drawn on top (duplication delivers the pointer separately).

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

use super::win::{sort_displays, CursorOverlay, CursorState, DibCanvas};
use super::{Capturer, Frame, Rect};

pub struct DxgiCapturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    staging: Option<ID3D11Texture2D>,
    canvas: DibCanvas,
    cursor: CursorOverlay,
    last_cursor: CursorState,
    rect: Rect,
    has_image: bool,
}

impl DxgiCapturer {
    pub fn new(display: usize) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            // Every output attached to the desktop, on every adapter (hybrid
            // laptops have outputs on more than one GPU).
            let mut outputs = Vec::new();
            let mut adapter_index = 0;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
                let mut output_index = 0;
                while let Ok(output) = adapter.EnumOutputs(output_index) {
                    let desc = output.GetDesc()?;
                    if desc.AttachedToDesktop.as_bool() {
                        let r = desc.DesktopCoordinates;
                        let rect = Rect { left: r.left, top: r.top, width: r.right - r.left, height: r.bottom - r.top };
                        let primary = r.left == 0 && r.top == 0;
                        outputs.push(((rect, primary), adapter.clone(), output, desc.Rotation));
                    }
                    output_index += 1;
                }
                adapter_index += 1;
            }
            let mut order: Vec<(Rect, bool)> = outputs.iter().map(|o| o.0).collect();
            sort_displays(&mut order);
            let wanted =
                *order.get(display).ok_or_else(|| anyhow!("display {display} not found ({} attached)", order.len()))?;
            let (_, adapter, output, rotation) =
                outputs.into_iter().find(|o| o.0 == wanted).ok_or_else(|| anyhow!("display {display} not found"))?;
            if rotation != DXGI_MODE_ROTATION_IDENTITY && rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
                bail!("display {display} is rotated");
            }
            let rect = wanted.0;

            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("D3D11CreateDevice")?;
            let device = device.context("no Direct3D device")?;
            let context = context.context("no Direct3D context")?;
            let output: IDXGIOutput1 = output.cast()?;
            let duplication = output.DuplicateOutput(&device).context("DuplicateOutput")?;

            Ok(Self {
                canvas: DibCanvas::new(rect.width as usize, rect.height as usize)?,
                device,
                context,
                output,
                duplication: Some(duplication),
                staging: None,
                cursor: CursorOverlay::default(),
                last_cursor: CursorState::default(),
                rect,
                has_image: false,
            })
        }
    }

    fn frame(&self) -> Frame<'_> {
        Frame {
            data: self.canvas.pixels(),
            width: self.canvas.width,
            height: self.canvas.height,
            stride: self.canvas.stride(),
        }
    }

    /// Copies a new desktop image into the canvas. Returns false for
    /// pointer-only updates.
    fn copy_image(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO, resource: Option<IDXGIResource>) -> Result<bool> {
        if info.LastPresentTime == 0 && self.has_image {
            return Ok(false);
        }
        let Some(resource) = resource else { return Ok(false) };
        let texture: ID3D11Texture2D = resource.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            bail!("unsupported desktop format {:?}", desc.Format);
        }
        if desc.Width as usize != self.canvas.width || desc.Height as usize != self.canvas.height {
            bail!("the display resolution changed");
        }

        if self.staging.is_none() {
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: desc.Width,
                Height: desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            unsafe { self.device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
            self.staging = staging;
        }
        let staging = self.staging.clone().context("no staging texture")?;

        unsafe {
            self.context.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let row_bytes = self.canvas.width * 4;
            let rows = self.canvas.height;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * rows);
            let dst = self.canvas.pixels_mut();
            for row in 0..rows {
                dst[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(&src[row * pitch..row * pitch + row_bytes]);
            }
            self.context.Unmap(&staging, 0);
        }
        Ok(true)
    }
}

impl Capturer for DxgiCapturer {
    fn name(&self) -> &'static str {
        "DXGI Desktop Duplication"
    }

    fn display_rect(&self) -> Rect {
        self.rect
    }

    fn capture(&mut self, timeout: Duration) -> Result<Option<Frame<'_>>> {
        if self.duplication.is_none() {
            // Lost access earlier (mode change, UAC prompt, full-screen app): retry.
            match unsafe { self.output.DuplicateOutput(&self.device) } {
                Ok(duplication) => self.duplication = Some(duplication),
                Err(e) => {
                    std::thread::sleep(timeout);
                    return Err(e).context("re-creating desktop duplication");
                }
            }
        }
        let duplication = self.duplication.clone().context("no duplication")?;

        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
        match unsafe { duplication.AcquireNextFrame(millis, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.duplication = None;
                self.has_image = false;
                return Ok(None);
            }
            Err(e) => return Err(e).context("AcquireNextFrame"),
        }
        let copied = self.copy_image(&info, resource);
        unsafe {
            let _ = duplication.ReleaseFrame();
        }
        let image_changed = copied?;

        if image_changed {
            self.cursor.invalidate();
        } else {
            self.cursor.restore(&mut self.canvas);
        }
        let cursor = self.cursor.draw(&mut self.canvas, (self.rect.left, self.rect.top));
        if !image_changed && self.has_image && cursor == self.last_cursor {
            return Ok(None);
        }
        self.last_cursor = cursor;
        self.has_image = true;
        Ok(Some(self.frame()))
    }
}
