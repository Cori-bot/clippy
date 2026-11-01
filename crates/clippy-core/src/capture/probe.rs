//! A small region of the live picture, read back without stalling capture.
//!
//! Auto-clip watches part of the screen (a game's kill feed) a few times a
//! second. [`super::stage::stage_bgra`] would do, but it copies the whole
//! frame and waits on the GPU, costing a frame each time. This copies only
//! the region into a small staging texture and maps it on a *later* frame
//! with `D3D11_MAP_FLAG_DO_NOT_WAIT`, so the capture callback never blocks.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING;

use super::stage::StagedFrame;

/// A rectangle in thousandths of the frame: x, y, width, height.
pub type ProbeRegion = (u16, u16, u16, u16);

/// Pixel rectangle of `region` in a `width`×`height` frame, at least 1×1.
pub fn region_pixels(region: ProbeRegion, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let px = |v: u16, of: u32| (u64::from(v.min(1000)) * u64::from(of) / 1000) as u32;
    let (x, y) = (px(region.0, width).min(width - 1), px(region.1, height).min(height - 1));
    let w = px(region.2, width).clamp(1, width - x);
    let h = px(region.3, height).clamp(1, height - y);
    (x, y, w, h)
}

pub struct RegionProbe {
    region: ProbeRegion,
    interval: Duration,
    staging: Option<(ID3D11Texture2D, (u32, u32))>,
    /// A copy was issued and has not been read back yet.
    pending: bool,
    issued: Option<Instant>,
    latest: Option<StagedFrame>,
}

impl RegionProbe {
    pub fn new(region: ProbeRegion, interval: Duration) -> Self {
        Self { region, interval, staging: None, pending: false, issued: None, latest: None }
    }

    pub fn region(&self) -> ProbeRegion {
        self.region
    }

    /// The newest region read back, once.
    pub fn take(&mut self) -> Option<StagedFrame> {
        self.latest.take()
    }

    /// Called once per captured frame. Reads back a finished copy, or issues
    /// a new one when the interval has passed. Never waits on the GPU.
    pub fn tick(&mut self, texture: &ID3D11Texture2D) -> Result<()> {
        unsafe {
            let device: ID3D11Device = texture.GetDevice().context("capture texture device")?;
            let context = device.GetImmediateContext().context("immediate context")?;
            if self.pending {
                let Some((staging, (w, h))) = &self.staging else { return Ok(()) };
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                match context.Map(
                    staging,
                    0,
                    D3D11_MAP_READ,
                    D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                    Some(&mut mapped),
                ) {
                    Ok(()) => {}
                    Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING => return Ok(()),
                    Err(e) => return Err(e).context("mapping the probe region"),
                }
                let row = *w as usize * 4;
                let mut bgra = vec![0u8; row * *h as usize];
                for y in 0..*h as usize {
                    let src = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                    std::ptr::copy_nonoverlapping(src, bgra.as_mut_ptr().add(y * row), row);
                }
                context.Unmap(staging, 0);
                self.latest = Some(StagedFrame { bgra, width: *w, height: *h, stride: row });
                self.pending = false;
                return Ok(());
            }
            if self.issued.is_some_and(|at| at.elapsed() < self.interval) {
                return Ok(());
            }
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM || desc.Width < 2 || desc.Height < 2 {
                return Ok(());
            }
            let (x, y, w, h) = region_pixels(self.region, desc.Width, desc.Height);
            if self.staging.as_ref().is_none_or(|(_, size)| *size != (w, h)) {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                    ..desc
                };
                let mut staging = None;
                device
                    .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                    .context("CreateTexture2D(probe)")?;
                self.staging = Some((staging.context("no probe texture")?, (w, h)));
            }
            let Some((staging, _)) = &self.staging else { return Ok(()) };
            let area = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
            context.CopySubresourceRegion(staging, 0, 0, 0, 0, texture, 0, Some(&area));
            self.pending = true;
            self.issued = Some(Instant::now());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_pixels_scales_and_stays_inside_the_frame() {
        assert_eq!(region_pixels((700, 70, 300, 250), 1920, 1080), (1344, 75, 576, 270));
        assert_eq!(region_pixels((1000, 1000, 500, 500), 100, 100), (99, 99, 1, 1));
        assert_eq!(region_pixels((0, 0, 0, 0), 10, 10), (0, 0, 1, 1));
    }
}
