//! 屏幕捕获 trait（ADR-0006：纯数据类型，平台实现 cfg-gate 在各自模块）。
//!
//! 平台实现 P1 起接入：dxgi（Windows）、x11+damage（Linux，先做）、
//! screencapturekit（macOS）；Wayland（pipewire + portal）排 P2。

use bytes::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 4:2:0 双平面：Y 平面 + 交织 UV 平面。硬编/硬解的首选交接格式。
    Nv12,
    /// 4:2:0 三平面。软编软解的常见格式。
    I420,
    /// 屏幕原始格式（DXGI/Quartz 常见），编码前需转换。
    Bgra8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureInfo {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub max_framerate: u32,
}

/// 一帧内的失效区域；为空表示整帧刷新。来源：DXGI dirty rects / X11 damage / 捕获器 diff。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Tightly packed rows; platform row padding must be removed at ingestion.
    pub data: Bytes,
    pub damage: Vec<DamageRect>,
    /// 单调时间戳（微秒），端侧自定；用于延迟测量与丢帧判定。
    pub timestamp_us: u64,
}

#[cfg(windows)]
pub mod dxgi;
pub mod synthetic;

impl CapturedFrame {
    /// Copy a top-down mapped BGRA surface, excluding row padding.
    pub fn from_bgra_rows(
        width: u32,
        height: u32,
        row_pitch: usize,
        source: &[u8],
        timestamp_us: u64,
    ) -> Result<Self, CaptureError> {
        let invalid = || {
            CaptureError::Unavailable("invalid BGRA dimensions, row pitch or buffer length".into())
        };
        if width == 0 || height == 0 {
            return Err(invalid());
        }
        let row_bytes = (width as usize).checked_mul(4).ok_or_else(invalid)?;
        let size = row_bytes.checked_mul(height as usize).ok_or_else(invalid)?;
        let required = row_pitch
            .checked_mul(height as usize - 1)
            .and_then(|n| n.checked_add(row_bytes))
            .ok_or_else(invalid)?;
        if row_pitch < row_bytes || source.len() < required {
            return Err(invalid());
        }
        let mut packed = Vec::new();
        packed
            .try_reserve_exact(size)
            .map_err(|e| CaptureError::Other(e.into()))?;
        for row in 0..height as usize {
            let offset = row * row_pitch;
            packed.extend_from_slice(&source[offset..offset + row_bytes]);
        }
        Ok(Self {
            width,
            height,
            format: PixelFormat::Bgra8,
            data: packed.into(),
            damage: Vec::new(),
            timestamp_us,
        })
    }
}

pub use synthetic::SyntheticCapturer;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no new desktop frame before timeout")]
    Timeout,
    #[error("capture backend unavailable: {0}")]
    Unavailable(String),
    #[error("display configuration changed")]
    DisplayChanged,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// 逐帧拉取式捕获接口。实现负责在显示配置变化时报 `DisplayChanged`
/// 并等待上层重建实例（分辨率/显示器增减时的处理路径）。
#[allow(async_fn_in_trait)]
pub trait Capturer: Send {
    fn info(&self) -> &CaptureInfo;
    /// Recreate invalid platform resources; callers bound retries and back off.
    fn recover(&mut self) -> Result<(), CaptureError> {
        Err(CaptureError::DisplayChanged)
    }
    async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError>;
}

#[cfg(test)]
mod bgra_tests {
    use super::*;
    #[test]
    fn removes_padding_and_keeps_timestamp() {
        let frame =
            CapturedFrame::from_bgra_rows(1, 2, 8, &[1, 2, 3, 4, 99, 99, 99, 99, 5, 6, 7, 8], 42)
                .unwrap();
        assert_eq!(&frame.data[..], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(frame.timestamp_us, 42);
        assert_eq!(frame.format, PixelFormat::Bgra8);
    }
    #[test]
    fn rejects_invalid_surface_layout() {
        for (w, h, pitch, data) in [
            (0, 2, 8, vec![0; 16]),
            (2, 2, 7, vec![0; 16]),
            (2, 2, 8, vec![0; 15]),
            (2, 3, usize::MAX, vec![]),
        ] {
            assert!(CapturedFrame::from_bgra_rows(w, h, pitch, &data, 0).is_err());
        }
    }
}
