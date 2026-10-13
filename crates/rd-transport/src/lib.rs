//! 传输层（ADR-0002）：`Transport` trait 只有两类实现——
//! 原生端 str0m（用户态 WebRTC，P2 接入），Web 端浏览器原生 WebRTC（P8，wasm）。
//! 打洞/中继交给 ICE + coturn(TURN)，不自建中继。

use rd_protocol::Frame;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("transport closed")]
    Closed,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// ICE 建立后的帧管道。实现负责分片与重组（数据通道 MTU 限制）。
#[allow(async_fn_in_trait)]
pub trait Transport: Send {
    async fn send(&mut self, frame: Frame) -> Result<(), TransportError>;
    async fn recv(&mut self) -> Result<Frame, TransportError>;
}

mod channel;

pub use channel::{channel_pair, ChannelTransport};
