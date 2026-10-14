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
    pub data: Bytes,
    pub damage: Vec<DamageRect>,
    /// 单调时间戳（微秒），端侧自定；用于延迟测量与丢帧判定。
    pub timestamp_us: u64,
}

pub mod synthetic;

pub use synthetic::SyntheticCapturer;

#[cfg(target_os = "macos")]
pub mod macos_sck;

#[cfg(target_os = "macos")]
pub use macos_sck::MacScreenCapturer;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
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
    async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError>;
}
