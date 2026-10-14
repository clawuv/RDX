//! 数据面裸帧（ADR-0003）：`[stream_id:u8][flags:u8][seq:u16][len:u32]` 大端序定长头 + 载荷。
//!
//! 布局一经 P2 定稿不再改动，扩展只通过 flags 位。

use bytes::{Buf, BufMut, Bytes, BytesMut};

pub const HEADER_LEN: usize = 8;

/// 单帧载荷上限，防止畸形头把分配打爆。
pub const MAX_PAYLOAD_LEN: u32 = 16 * 1024 * 1024;

pub mod flags {
    /// 该帧承载关键帧数据。
    pub const KEYFRAME: u8 = 1 << 0;
    /// 帧分片的最后一片（大帧跨多个包时的尾部标记）。
    pub const LAST_SLICE: u8 = 1 << 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub stream_id: u8,
    pub flags: u8,
    pub seq: u16,
    pub payload_len: u32,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    /// 缓冲字节不足一帧（头或载荷）；**不消费**任何字节，调用方可后续追加后重试。
    #[error("need {need} bytes to complete frame, have {have}")]
    NeedMore { need: usize, have: usize },
    #[error("frame header truncated: need {need} bytes, have {have}")]
    HeaderTruncated { need: usize, have: usize },
    #[error("payload length {len} exceeds maximum {max}")]
    PayloadTooLarge { len: u32, max: u32 },
}

impl FrameHeader {
    pub fn encode_into(&self, dst: &mut BytesMut) {
        dst.put_u8(self.stream_id);
        dst.put_u8(self.flags);
        dst.put_u16(self.seq);
        dst.put_u32(self.payload_len);
    }

    /// 从缓冲区前缀窥视帧头，**不消费**字节。
    pub fn peek(src: &[u8]) -> Result<Self, FrameError> {
        if src.len() < HEADER_LEN {
            return Err(FrameError::HeaderTruncated {
                need: HEADER_LEN,
                have: src.len(),
            });
        }
        let stream_id = src[0];
        let flags = src[1];
        let seq = u16::from_be_bytes([src[2], src[3]]);
        let payload_len = u32::from_be_bytes([src[4], src[5], src[6], src[7]]);
        if payload_len > MAX_PAYLOAD_LEN {
            return Err(FrameError::PayloadTooLarge {
                len: payload_len,
                max: MAX_PAYLOAD_LEN,
            });
        }
        Ok(Self {
            stream_id,
            flags,
            seq,
            payload_len,
        })
    }

    /// 从缓冲区头部消费 8 字节并解码。字节不足或载荷超限时不消费。
    pub fn decode_from(src: &mut BytesMut) -> Result<Self, FrameError> {
        let header = Self::peek(src)?;
        src.advance(HEADER_LEN);
        Ok(header)
    }

    pub fn is_keyframe(&self) -> bool {
        self.flags & flags::KEYFRAME != 0
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub header: FrameHeader,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(stream_id: u8, seq: u16, keyframe: bool, payload: Bytes) -> Self {
        debug_assert!(payload.len() <= MAX_PAYLOAD_LEN as usize);
        let flags = if keyframe { flags::KEYFRAME } else { 0 };
        Self {
            header: FrameHeader {
                stream_id,
                flags,
                seq,
                payload_len: payload.len() as u32,
            },
            payload,
        }
    }

    pub fn is_keyframe(&self) -> bool {
        self.header.is_keyframe()
    }

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(HEADER_LEN + self.payload.len());
        self.header.encode_into(&mut buf);
        buf.extend_from_slice(&self.payload);
        buf.freeze()
    }

    /// 从缓冲区头部消费一个完整帧。
    ///
    /// 头或整帧字节不足时返回 [`FrameError::NeedMore`] 且**零消费**，可追加字节后重试。
    /// 载荷声明超限时返回 [`FrameError::PayloadTooLarge`] 且零消费。
    pub fn decode(buf: &mut BytesMut) -> Result<Self, FrameError> {
        let header = match FrameHeader::peek(buf) {
            Ok(h) => h,
            Err(FrameError::HeaderTruncated { need, have }) => {
                return Err(FrameError::NeedMore { need, have });
            }
            Err(e) => return Err(e),
        };
        let total = HEADER_LEN + header.payload_len as usize;
        if buf.len() < total {
            return Err(FrameError::NeedMore {
                need: total,
                have: buf.len(),
            });
        }
        buf.advance(HEADER_LEN);
        let payload = buf.split_to(header.payload_len as usize).freeze();
        Ok(Self { header, payload })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let mut buf = BytesMut::new();
        FrameHeader {
            stream_id: 3,
            flags: flags::KEYFRAME | flags::LAST_SLICE,
            seq: 0xBEEF,
            payload_len: 0x00F0_0304,
        }
        .encode_into(&mut buf);
        assert_eq!(buf.len(), HEADER_LEN);
        let decoded = FrameHeader::decode_from(&mut buf).unwrap();
        assert_eq!(
            decoded,
            FrameHeader {
                stream_id: 3,
                flags: flags::KEYFRAME | flags::LAST_SLICE,
                seq: 0xBEEF,
                payload_len: 0x00F0_0304,
            }
        );
        assert!(decoded.is_keyframe());
        assert!(buf.is_empty());
    }

    #[test]
    fn header_truncated() {
        let mut buf = BytesMut::from(&b"\x01\x02\x03"[..]);
        let before = buf.clone();
        assert!(matches!(
            FrameHeader::decode_from(&mut buf),
            Err(FrameError::HeaderTruncated { .. })
        ));
        assert_eq!(buf, before);
    }

    #[test]
    fn payload_too_large_rejected_without_consume() {
        let mut buf = BytesMut::new();
        FrameHeader {
            stream_id: 0,
            flags: 0,
            seq: 0,
            payload_len: MAX_PAYLOAD_LEN + 1,
        }
        .encode_into(&mut buf);
        let before = buf.clone();
        assert!(matches!(
            FrameHeader::decode_from(&mut buf),
            Err(FrameError::PayloadTooLarge { .. })
        ));
        assert_eq!(buf, before);
        assert!(matches!(
            Frame::decode(&mut buf),
            Err(FrameError::PayloadTooLarge { .. })
        ));
        assert_eq!(buf, before);
    }

    #[test]
    fn frame_roundtrip_and_streaming() {
        let mut buf = BytesMut::new();
        buf.extend(Frame::new(1, 7, true, Bytes::from_static(b"hello")).encode());
        buf.extend(Frame::new(2, 8, false, Bytes::from_static(b"world!!")).encode());

        let first = Frame::decode(&mut buf).unwrap();
        assert!(first.is_keyframe());
        assert_eq!(&first.payload[..], b"hello");
        let second = Frame::decode(&mut buf).unwrap();
        assert!(!second.is_keyframe());
        assert_eq!(&second.payload[..], b"world!!");
        assert!(buf.is_empty());
    }

    #[test]
    fn incomplete_frame_is_zero_consume_and_retryable() {
        let full = Frame::new(1, 1, false, Bytes::from_static(b"abcdef")).encode();
        let mut buf = BytesMut::from(&full[..HEADER_LEN + 3]);
        let before = buf.clone();
        match Frame::decode(&mut buf) {
            Err(FrameError::NeedMore { need, have }) => {
                assert_eq!(need, HEADER_LEN + 6);
                assert_eq!(have, HEADER_LEN + 3);
            }
            other => panic!("expected NeedMore, got {other:?}"),
        }
        assert_eq!(buf, before);

        buf.extend_from_slice(&full[HEADER_LEN + 3..]);
        let frame = Frame::decode(&mut buf).unwrap();
        assert_eq!(&frame.payload[..], b"abcdef");
        assert!(buf.is_empty());
    }

    #[test]
    fn header_only_incomplete_is_need_more() {
        let mut buf = BytesMut::from(&b"\x01\x02\x03"[..]);
        let before = buf.len();
        match Frame::decode(&mut buf) {
            Err(FrameError::NeedMore { need, have }) => {
                assert_eq!(need, HEADER_LEN);
                assert_eq!(have, 3);
            }
            other => panic!("expected NeedMore, got {other:?}"),
        }
        assert_eq!(buf.len(), before);
    }

    #[test]
    fn chunked_byte_stream_roundtrip() {
        let frames = [
            Frame::new(1, 0, true, Bytes::from_static(b"alpha")),
            Frame::new(2, 1, false, Bytes::from_static(b"")),
            Frame::new(3, 2, false, Bytes::from(vec![0u8; 64])),
            Frame::new(4, 3, true, Bytes::from_static(b"end-marker")),
        ];
        let mut wire = BytesMut::new();
        for f in &frames {
            wire.extend(f.encode());
        }
        let bytes = wire.freeze();

        // Deterministic cut points: every 1, 3, 7, 11 bytes.
        for step in [1usize, 3, 7, 11] {
            let mut buf = BytesMut::new();
            let mut out = Vec::new();
            let mut offset = 0usize;
            while offset < bytes.len() || !buf.is_empty() {
                if offset < bytes.len() {
                    let end = (offset + step).min(bytes.len());
                    buf.extend_from_slice(&bytes[offset..end]);
                    offset = end;
                }
                match Frame::decode(&mut buf) {
                    Ok(frame) => out.push(frame),
                    Err(FrameError::NeedMore { .. }) => {
                        if offset >= bytes.len() {
                            panic!("stuck with {} leftover bytes at step {step}", buf.len());
                        }
                    }
                    Err(e) => panic!("unexpected error at step {step}: {e}"),
                }
            }
            assert_eq!(out.len(), frames.len(), "step {step}");
            for (got, expect) in out.iter().zip(frames.iter()) {
                assert_eq!(got.header, expect.header, "step {step}");
                assert_eq!(got.payload, expect.payload, "step {step}");
            }
        }
    }
}
