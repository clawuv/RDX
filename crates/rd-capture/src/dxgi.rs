//! DXGI Desktop Duplication CPU baseline with pointer composition.
#[path = "dxgi_pointer.rs"]
mod pointer;
use crate::{CaptureError, CaptureInfo, CapturedFrame, Capturer, PixelFormat};
use std::time::Instant;
use windows::{
    core::Interface,
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
};

#[derive(Debug, Clone)]
pub struct Display {
    pub adapter: u32,
    pub output: u32,
    pub name: String,
    /// Origin in physical desktop pixels; may be negative on secondary outputs.
    pub desktop_x: i32,
    pub desktop_y: i32,
    pub width: u32,
    pub height: u32,
}
fn error(e: windows::core::Error) -> CaptureError {
    if e.code() == DXGI_ERROR_ACCESS_LOST {
        CaptureError::DisplayChanged
    } else {
        CaptureError::Unavailable(e.to_string())
    }
}
pub fn displays() -> Result<Vec<Display>, CaptureError> {
    // SAFETY: COM interfaces are returned by DXGI; enumeration stops at NOT_FOUND.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(error)?;
        let mut result = Vec::new();
        for ai in 0.. {
            let adapter = match factory.EnumAdapters1(ai) {
                Ok(a) => a,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(error(e)),
            };
            for oi in 0.. {
                let output = match adapter.EnumOutputs(oi) {
                    Ok(o) => o,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(error(e)),
                };
                let d = output.GetDesc().map_err(error)?;
                if !d.AttachedToDesktop.as_bool() {
                    continue;
                }
                let end = d
                    .DeviceName
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(d.DeviceName.len());
                result.push(Display {
                    adapter: ai,
                    output: oi,
                    name: String::from_utf16_lossy(&d.DeviceName[..end]),
                    desktop_x: d.DesktopCoordinates.left,
                    desktop_y: d.DesktopCoordinates.top,
                    width: (d.DesktopCoordinates.right - d.DesktopCoordinates.left) as u32,
                    height: (d.DesktopCoordinates.bottom - d.DesktopCoordinates.top) as u32,
                });
            }
        }
        Ok(result)
    }
}
pub struct DxgiCapturer {
    pointer: pointer::Pointer,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    info: CaptureInfo,
    origin: Instant,
}

/// Keeps the selected desktop name across enumeration reorderings.
/// This is a session identity, not a persistent hardware identifier.
pub struct RecoveringDxgiCapturer {
    inner: Option<DxgiCapturer>,
    name: String,
    origin: Instant,
    info: CaptureInfo,
}
impl RecoveringDxgiCapturer {
    pub fn new(display: &Display, origin: Instant) -> Result<Self, CaptureError> {
        let inner = DxgiCapturer::new(display, origin)?;
        Ok(Self {
            info: *inner.info(),
            inner: Some(inner),
            name: display.name.clone(),
            origin,
        })
    }
}
fn selected_display<'a>(displays: &'a [Display], name: &str) -> Result<&'a Display, CaptureError> {
    let mut matches = displays.iter().filter(|d| d.name == name);
    match (matches.next(), matches.next()) {
        (Some(display), None) => Ok(display),
        _ => Err(CaptureError::Unavailable(
            "selected display missing or ambiguous".into(),
        )),
    }
}
impl Capturer for RecoveringDxgiCapturer {
    fn info(&self) -> &CaptureInfo {
        &self.info
    }
    fn recover(&mut self) -> Result<(), CaptureError> {
        // Release the previous duplication before creating a replacement.
        self.inner.take();
        let displays = displays()?;
        let display = selected_display(&displays, &self.name)?;
        let inner = DxgiCapturer::new(display, self.origin)?;
        self.info = *inner.info();
        self.inner = Some(inner);
        Ok(())
    }
    async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        self.inner
            .as_mut()
            .ok_or(CaptureError::DisplayChanged)?
            .next_frame()
            .await
    }
}
impl DxgiCapturer {
    pub fn new(display: &Display, origin: Instant) -> Result<Self, CaptureError> {
        // SAFETY: output and D3D device use the same adapter; all out-pointers
        // refer to initialized locals and returned interfaces remain owned.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(error)?;
            let adapter = factory.EnumAdapters1(display.adapter).map_err(error)?;
            let output: IDXGIOutput1 = adapter
                .EnumOutputs(display.output)
                .map_err(error)?
                .cast()
                .map_err(error)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(error)?;
            let device =
                device.ok_or_else(|| CaptureError::Unavailable("D3D11 device missing".into()))?;
            let context =
                context.ok_or_else(|| CaptureError::Unavailable("D3D11 context missing".into()))?;
            let duplication = output.DuplicateOutput(&device).map_err(error)?;
            let d = duplication.GetDesc();
            if d.Rotation != DXGI_MODE_ROTATION_IDENTITY {
                return Err(CaptureError::Unavailable(
                    "rotated displays are not yet supported".into(),
                ));
            }
            Ok(Self {
                pointer: pointer::Pointer::default(),
                device,
                context,
                duplication,
                staging: None,
                info: CaptureInfo {
                    width: d.ModeDesc.Width,
                    height: d.ModeDesc.Height,
                    format: PixelFormat::Bgra8,
                    max_framerate: 60,
                },
                origin,
            })
        }
    }
    fn capture(&mut self) -> Result<CapturedFrame, CaptureError> {
        // SAFETY: exclusive access serializes the immediate context. The mapped
        // staging texture is CPU-readable and stays mapped until the copy ends.
        // Every successful Map/Acquire is paired with Unmap/Release on error paths.
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            self.duplication
                .AcquireNextFrame(100, &mut frame_info, &mut resource)
                .map_err(|e| {
                    if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                        CaptureError::Timeout
                    } else {
                        error(e)
                    }
                })?;
            // Release acquired frames on all Result paths, including mapping failures.
            let result = (|| {
                if frame_info.LastMouseUpdateTime != 0 {
                    self.pointer.visible = frame_info.PointerPosition.Visible.as_bool();
                    self.pointer.position = (
                        frame_info.PointerPosition.Position.x,
                        frame_info.PointerPosition.Position.y,
                    );
                }
                if frame_info.PointerShapeBufferSize > 0 {
                    if frame_info.PointerShapeBufferSize > 4 * 1024 * 1024 {
                        return Err(CaptureError::Unavailable(
                            "cursor shape exceeds 4MiB".into(),
                        ));
                    }
                    let mut bytes = vec![0; frame_info.PointerShapeBufferSize as usize];
                    let mut required = 0;
                    let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
                    self.duplication
                        .GetFramePointerShape(
                            frame_info.PointerShapeBufferSize,
                            bytes.as_mut_ptr().cast(),
                            &mut required,
                            &mut shape,
                        )
                        .map_err(error)?;
                    if required as usize > bytes.len() {
                        return Err(CaptureError::Unavailable("cursor size changed".into()));
                    }
                    bytes.truncate(required as usize);
                    self.pointer.bytes = bytes;
                    self.pointer.shape = shape;
                }
                // Acquisition time, not the compositor's LastPresentTime.
                let timestamp = self.origin.elapsed().as_micros() as u64;
                let texture: ID3D11Texture2D = resource
                    .ok_or_else(|| CaptureError::Unavailable("DXGI resource missing".into()))?
                    .cast()
                    .map_err(error)?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                texture.GetDesc(&mut desc);
                if desc.Width != self.info.width || desc.Height != self.info.height {
                    return Err(CaptureError::DisplayChanged);
                }
                if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                    return Err(CaptureError::Unavailable(
                        "unexpected DXGI pixel format".into(),
                    ));
                }
                if self.staging.is_none() {
                    desc.Usage = D3D11_USAGE_STAGING;
                    desc.BindFlags = 0;
                    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                    desc.MiscFlags = 0;
                    self.device
                        .CreateTexture2D(&desc, None, Some(&mut self.staging))
                        .map_err(error)?;
                }
                let staging = self
                    .staging
                    .as_ref()
                    .ok_or_else(|| CaptureError::Unavailable("staging texture missing".into()))?;
                self.context.CopyResource(staging, &texture);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context
                    .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .map_err(error)?;
                let result = (|| {
                    if desc.Height == 0 || u64::from(mapped.RowPitch) < u64::from(desc.Width) * 4 {
                        return Err(CaptureError::Unavailable("invalid mapped row pitch".into()));
                    }
                    let len = (mapped.RowPitch as usize)
                        .checked_mul(desc.Height.saturating_sub(1) as usize)
                        .and_then(|n| {
                            (desc.Width as usize)
                                .checked_mul(4)
                                .and_then(|row| n.checked_add(row))
                        })
                        .filter(|n| *n <= isize::MAX as usize)
                        .ok_or_else(|| CaptureError::Unavailable("mapped size overflow".into()))?;
                    if mapped.pData.is_null() {
                        return Err(CaptureError::Unavailable("null mapped surface".into()));
                    }
                    let bytes = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), len);
                    CapturedFrame::from_bgra_rows(
                        desc.Width,
                        desc.Height,
                        mapped.RowPitch as usize,
                        bytes,
                        timestamp,
                    )
                })();
                self.context.Unmap(staging, 0);
                let mut frame = result?;
                self.pointer.composite(&mut frame)?;
                Ok(frame)
            })();
            let release = self.duplication.ReleaseFrame().map_err(error);
            match result {
                Ok(frame) => {
                    release?;
                    Ok(frame)
                }
                Err(e) => Err(e),
            }
        }
    }
}
impl Capturer for DxgiCapturer {
    fn info(&self) -> &CaptureInfo {
        &self.info
    }
    async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        self.capture()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_never_falls_back_to_another_output() {
        let a = Display {
            adapter: 0,
            output: 0,
            name: "A".into(),
            desktop_x: 0,
            desktop_y: 0,
            width: 1920,
            height: 1080,
        };
        let b = Display {
            output: 1,
            name: "B".into(),
            ..a.clone()
        };
        assert_eq!(
            selected_display(&[b.clone(), a.clone()], "A").unwrap().name,
            "A"
        );
        assert!(selected_display(&[b], "A").is_err());
        assert!(selected_display(&[a.clone(), a], "A").is_err());
    }
    #[test]
    fn access_loss_requires_recreation() {
        assert!(matches!(
            error(windows::core::Error::from_hresult(DXGI_ERROR_ACCESS_LOST)),
            CaptureError::DisplayChanged
        ));
        assert!(matches!(
            error(windows::core::Error::from_hresult(DXGI_ERROR_UNSUPPORTED)),
            CaptureError::Unavailable(_)
        ));
    }
}
