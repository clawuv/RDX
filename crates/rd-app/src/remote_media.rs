//! Experimental single-stream H.264 media adapter. Call only after the session
//! has authenticated and agreed on this payload format. No cross-host clock sync.

use bytes::{BufMut, BytesMut};
use rd_codec::{DecodedFrame, Decoder as _, EncodedPacket, OpenH264Decoder};
use rd_protocol::{Frame, MAX_PAYLOAD_LEN};
use rd_transport::Transport;

pub type MediaResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const HEADER: usize = 20; // H264 magic/version + u64 access-unit index + u64 timestamp

pub struct MediaSender {
    stream_id: u8,
    next_id: u64,
}

impl MediaSender {
    pub fn new(stream_id: u8) -> Self {
        Self {
            stream_id,
            next_id: 0,
        }
    }

    /// One packet is one access unit. On transport failure close the session;
    /// retrying a partially sent access unit is not supported.
    pub async fn send(
        &mut self,
        transport: &mut impl Transport,
        packet: EncodedPacket,
    ) -> MediaResult<()> {
        if packet.data.is_empty() || packet.data.len() > MAX_PAYLOAD_LEN as usize - HEADER {
            return Err("invalid H.264 access-unit length".into());
        }
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or("media id exhausted; reconnect required")?;
        let mut payload = BytesMut::with_capacity(HEADER + packet.data.len());
        payload.extend_from_slice(b"H26\x01");
        payload.put_u64(id);
        payload.put_u64(packet.timestamp_us);
        payload.extend_from_slice(&packet.data);
        transport
            .send(Frame::new(
                self.stream_id,
                id as u16,
                packet.keyframe,
                payload.freeze(),
            ))
            .await?;
        Ok(())
    }
}

pub struct MediaReceiver {
    stream_id: u8,
    decoder: OpenH264Decoder,
    last_id: Option<u64>,
    needs_keyframe: bool,
    last_request: Option<std::time::Instant>,
}

impl MediaReceiver {
    pub fn new(stream_id: u8) -> MediaResult<Self> {
        Ok(Self {
            stream_id,
            decoder: OpenH264Decoder::new()?,
            last_id: None,
            needs_keyframe: true,
            last_request: None,
        })
    }

    /// Consume one network frame; None means a foreign, stale, or unusable delta
    /// frame was dropped. A gap resets decoder state and waits for a fresh IDR.
    /// The caller can publish Some(frame) through the existing latest-frame UI slot.
    pub async fn receive(
        &mut self,
        transport: &mut impl Transport,
    ) -> MediaResult<Option<DecodedFrame>> {
        let frame = transport.recv().await?;
        self.decode(frame).await
    }

    /// WebRTC receive path with reliable, rate-limited recovery requests.
    /// Retry at most every 250ms while incoming video still needs an IDR.
    pub async fn receive_webrtc(
        &mut self,
        transport: &mut rd_transport::Str0mTransport,
    ) -> MediaResult<Option<DecodedFrame>> {
        let frame = transport.recv().await?;
        let result = self.decode(frame).await;
        if self.needs_keyframe {
            if let Some(id) = self.last_id {
                let now = std::time::Instant::now();
                if self.last_request.is_none_or(|last| {
                    now.duration_since(last) >= std::time::Duration::from_millis(250)
                }) {
                    let message =
                        rd_protocol::control::ControlMessage::keyframe_request(self.stream_id, id)
                            .encode();
                    transport.send_control(&message).await?;
                    self.last_request = Some(now);
                }
            }
        } else {
            self.last_request = None;
        }
        result
    }

    async fn decode(&mut self, frame: Frame) -> MediaResult<Option<DecodedFrame>> {
        if frame.header.stream_id != self.stream_id {
            return Ok(None);
        }
        if frame.payload.len() <= HEADER
            || frame.payload.len() > MAX_PAYLOAD_LEN as usize
            || frame.header.payload_len as usize != frame.payload.len()
            || &frame.payload[..4] != b"H26\x01"
        {
            return Err("invalid experimental H.264 media payload".into());
        }
        let id = u64::from_be_bytes(frame.payload[4..12].try_into()?);
        if frame.header.seq != id as u16 {
            return Err("media sequence mismatch".into());
        }
        if let Some(last) = self.last_id {
            if id <= last {
                return Ok(None);
            }
            if id - last != 1 {
                self.needs_keyframe = true;
            }
        }
        self.last_id = Some(id);
        if self.needs_keyframe {
            if !frame.is_keyframe() {
                return Ok(None);
            }
            self.decoder = OpenH264Decoder::new()?;
        }
        let packet = EncodedPacket {
            keyframe: frame.is_keyframe(),
            timestamp_us: u64::from_be_bytes(frame.payload[12..20].try_into()?),
            data: frame.payload.slice(HEADER..),
        };
        match self.decoder.decode(&packet).await {
            Ok(decoded) => {
                self.needs_keyframe = false;
                Ok(Some(decoded))
            }
            Err(error) => {
                self.needs_keyframe = true;
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rd_capture::{Capturer as _, SyntheticCapturer};
    use rd_codec::{CodecId, Encoder as _, EncoderConfig, OpenH264Encoder};
    use std::time::Duration;

    fn encoder() -> OpenH264Encoder {
        OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 800,
            framerate: 30,
            keyframe_interval: 1000,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn real_webrtc_video_decodes_and_preserves_timestamps_across_sequence_wrap() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut host, mut viewer) = rd_transport::connect_pair().unwrap();
            tokio::try_join!(
                crate::remote::exchange_capabilities(&mut host),
                crate::remote::exchange_capabilities(&mut viewer)
            )
            .unwrap();
            let mut sender = MediaSender::new(3);
            sender.next_id = 65_535;
            let mut receiver = MediaReceiver::new(3).unwrap();
            let mut capture = SyntheticCapturer::new(128, 96, 30);
            let mut encoder = encoder();
            for index in 0..3 {
                let frame = capture.next_frame().await.unwrap();
                let packet = encoder.encode(&frame).await.unwrap().remove(0);
                assert_eq!(packet.keyframe, index == 0);
                sender.send(&mut host, packet).await.unwrap();
                let decoded = receiver.receive(&mut viewer).await.unwrap().unwrap();
                assert_eq!((decoded.width, decoded.height), (128, 96));
                assert_eq!(decoded.timestamp_us, frame.timestamp_us);
                assert_eq!(decoded.data.len(), 128 * 96 * 3 / 2);
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn gap_drops_deltas_and_stale_frames_until_fresh_idr() {
        let (mut host, mut wire) = rd_transport::channel_pair(4);
        let mut sender = MediaSender::new(1);
        let mut receiver = MediaReceiver::new(1).unwrap();
        let mut capture = SyntheticCapturer::new(128, 96, 30);
        let mut encoder = encoder();
        for index in 0..4 {
            if index == 3 {
                encoder.request_keyframe();
            }
            let frame = capture.next_frame().await.unwrap();
            sender
                .send(&mut host, encoder.encode(&frame).await.unwrap().remove(0))
                .await
                .unwrap();
            let frame = wire.recv().await.unwrap();
            if index == 1 {
                continue;
            } // lost entire access unit
            let decoded = receiver.decode(frame.clone()).await.unwrap();
            assert_eq!(decoded.is_some(), index == 0 || index == 3);
            assert!(receiver.decode(frame).await.unwrap().is_none());
        }
        assert!(receiver
            .decode(Frame::new(1, 4, true, bytes::Bytes::from_static(b"bad")))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn whole_frame_loss_requests_idr_over_control_and_recovers() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut host, mut viewer) = rd_transport::connect_pair().unwrap();
            let (requests, mut request_rx) = tokio::sync::mpsc::channel(4);
            let bridge = crate::remote::attach_transport(&mut host, requests).unwrap();
            for event in [
                rd_engine::SessionEvent::HandshakeDone,
                rd_engine::SessionEvent::AuthOk,
                rd_engine::SessionEvent::CapabilitiesAgreed,
            ] {
                bridge.send_event(event).await.unwrap();
            }
            let mut sender = MediaSender::new(1);
            let mut receiver = MediaReceiver::new(1).unwrap();
            let mut encoder = encoder();
            let mut capture = SyntheticCapturer::new(128, 96, 30);
            for index in 0..4 {
                let frame = capture.next_frame().await.unwrap();
                let packet = encoder.encode(&frame).await.unwrap().remove(0);
                if index == 1 {
                    sender.next_id += 1;
                    continue;
                }
                sender.send(&mut host, packet).await.unwrap();
                let decoded = receiver.receive_webrtc(&mut viewer).await.unwrap();
                if index == 2 {
                    assert!(decoded.is_none());
                    let request = request_rx.recv().await.unwrap();
                    assert_eq!((request.stream_id, request.after_frame_id), (1, 2));
                    encoder.request_keyframe();
                } else {
                    assert!(decoded.is_some());
                }
            }
            bridge.shutdown().await.unwrap();
        })
        .await
        .unwrap();
    }
}
