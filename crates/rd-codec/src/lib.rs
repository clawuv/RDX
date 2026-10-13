//! 编解码 trait（ADR-0006）。P1 起接入实现：
//! 软编基线 openh264（永远可用），硬编按 MF / VideoToolbox / VAAPI 顺序；
//! AV1 只做硬编（软编实时不可用），解码软解起步后加 dav1d/硬解。

use bytes::Bytes;
use rd_capture::CapturedFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecId {
    H264,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    pub codec: CodecId,
    pub bitrate_kbps: u32,
    pub framerate: u32,
    /// 关键帧间隔（帧数）。屏幕场景可远大于摄像头场景。
    pub keyframe_interval: u32,
}

#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub keyframe: bool,
    pub timestamp_us: u64,
    pub data: Bytes,
}

/// 解码输出保持 NV12/I420 原样上 GPU，YUV→RGB 在渲染端 shader 完成（ADR-0001）。
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub format: rd_capture::PixelFormat,
    pub data: Bytes,
    pub timestamp_us: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("codec {0:?} not available on this platform/configuration")]
    Unavailable(CodecId),
    #[error("malformed bitstream: {0}")]
    Malformed(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[allow(async_fn_in_trait)]
pub trait Encoder: Send {
    fn config(&self) -> &EncoderConfig;
    fn reconfigure(&mut self, config: EncoderConfig);
    /// 请求下一帧输出关键帧（新观看者加入 / 丢包恢复时调用）。
    fn request_keyframe(&mut self);
    async fn encode(&mut self, frame: &CapturedFrame) -> Result<Vec<EncodedPacket>, CodecError>;
}

#[allow(async_fn_in_trait)]
pub trait Decoder: Send {
    async fn decode(&mut self, packet: &EncodedPacket) -> Result<DecodedFrame, CodecError>;
}
