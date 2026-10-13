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

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame header truncated: need {need} bytes, have {have}")]
    HeaderTruncated { need: usize, have: usize },
    #[error("frame payload truncated: need {need} bytes, have {have}")]
    PayloadTruncated { need: usize, have: usize },
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

    /// 从缓冲区头部消费 8 字节并解码。
    pub fn decode_from(src: &mut BytesMut) -> Result<Self, FrameError> {
        if src.len() < HEADER_LEN {
            return Err(FrameError::HeaderTruncated {
                need: HEADER_LEN,
                have: src.len(),
            });
        }
        let stream_id = src.get_u8();
        let flags = src.get_u8();
        let seq = src.get_u16();
        let payload_len = src.get_u32();
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

    /// 从缓冲区头部消费一个完整帧；头或载荷不完整时报错且不消费载荷部分。
    pub fn decode(buf: &mut BytesMut) -> Result<Self, FrameError> {
        let header = FrameHeader::decode_from(buf)?;
        if buf.len() < header.payload_len as usize {
            return Err(FrameError::PayloadTruncated {
                need: header.payload_len as usize,
                have: buf.len(),
            });
        }
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
        assert!(matches!(
            FrameHeader::decode_from(&mut buf),
            Err(FrameError::HeaderTruncated { .. })
        ));
    }

    #[test]
    fn payload_too_large_rejected() {
        let mut buf = BytesMut::new();
        FrameHeader {
            stream_id: 0,
            flags: 0,
            seq: 0,
            payload_len: MAX_PAYLOAD_LEN + 1,
        }
        .encode_into(&mut buf);
        assert!(matches!(
            FrameHeader::decode_from(&mut buf),
            Err(FrameError::PayloadTooLarge { .. })
        ));
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
    fn payload_truncated_keeps_header_consumed_state() {
        let mut buf = BytesMut::new();
        buf.extend(Frame::new(1, 1, false, Bytes::from_static(b"abcdef")).encode());
        buf.truncate(HEADER_LEN + 3);
        assert!(matches!(
            Frame::decode(&mut buf),
            Err(FrameError::PayloadTruncated { .. })
        ));
    }
}
