//! 内存回环 Transport，供引擎测试与 P1 单机垂直切片使用。

use rd_protocol::Frame;
use tokio::sync::{mpsc, Mutex};

use crate::{Transport, TransportError};

pub struct ChannelTransport {
    tx: mpsc::Sender<Frame>,
    rx: Mutex<mpsc::Receiver<Frame>>,
}

/// 创建一对互通的回环管道：a 发出的帧 b 收到，反之亦然。
pub fn channel_pair(buffer: usize) -> (ChannelTransport, ChannelTransport) {
    let (tx_a, rx_a) = mpsc::channel(buffer);
    let (tx_b, rx_b) = mpsc::channel(buffer);
    (
        ChannelTransport {
            tx: tx_a,
            rx: Mutex::new(rx_b),
        },
        ChannelTransport {
            tx: tx_b,
            rx: Mutex::new(rx_a),
        },
    )
}

impl Transport for ChannelTransport {
    async fn send(&mut self, frame: Frame) -> Result<(), TransportError> {
        self.tx
            .send(frame)
            .await
            .map_err(|_| TransportError::Closed)
    }

    async fn recv(&mut self) -> Result<Frame, TransportError> {
        self.rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(TransportError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[tokio::test]
    async fn frames_flow_both_ways() {
        let (mut a, mut b) = channel_pair(4);
        a.send(Frame::new(1, 7, true, Bytes::from_static(b"hello")))
            .await
            .unwrap();
        let got = b.recv().await.unwrap();
        assert_eq!(got.header.seq, 7);
        assert!(got.is_keyframe());
        assert_eq!(&got.payload[..], b"hello");

        b.send(Frame::new(1, 8, false, Bytes::from_static(b"ack")))
            .await
            .unwrap();
        let got = a.recv().await.unwrap();
        assert_eq!(&got.payload[..], b"ack");
    }

    #[tokio::test]
    async fn peer_drop_closes() {
        let (mut a, b) = channel_pair(1);
        drop(b);
        assert!(matches!(a.recv().await, Err(TransportError::Closed)));
    }
}
