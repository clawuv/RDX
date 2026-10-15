//! openh264 软编/软解（ADR 计划中的"永远可用的基线"实现）。
//!
//! P1 注意：encode/decode 是阻塞 C 调用，直接在 async fn 内执行；当前仅在
//! 回环管道的专用运行时线程上使用。真实会话接入时再评估 spawn_blocking。

use bytes::{Bytes, BytesMut};
use openh264::formats::{BgraSliceU8, YUVBuffer, YUVSource as _};
use openh264::{
    decoder::{Decoder as H264Decoder, DecoderConfig as H264DecoderConfig},
    encoder::{
        BitRate, Encoder as H264Encoder, EncoderConfig as H264EncoderConfig, FrameRate, FrameType,
        RateControlMode,
    },
    OpenH264API,
};
use rd_capture::{CapturedFrame, PixelFormat};

use crate::{CodecError, CodecId, DecodedFrame, Decoder, EncodedPacket, Encoder, EncoderConfig};

/// 借用式 I420 源，避免把帧拷进 openh264 自带的 YUVBuffer。
struct I420Ref<'a> {
    width: usize,
    height: usize,
    y: &'a [u8],
    u: &'a [u8],
    v: &'a [u8],
}

impl openh264::formats::YUVSource for I420Ref<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
    fn strides(&self) -> (usize, usize, usize) {
        (self.width, self.width / 2, self.width / 2)
    }
    fn y(&self) -> &[u8] {
        self.y
    }
    fn u(&self) -> &[u8] {
        self.u
    }
    fn v(&self) -> &[u8] {
        self.v
    }
}

fn split_i420(frame: &CapturedFrame) -> Result<I420Ref<'_>, CodecError> {
    if frame.format != PixelFormat::I420 {
        return Err(CodecError::Malformed(format!(
            "openh264 encoder expects I420, got {:?}",
            frame.format
        )));
    }
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w == 0 || h == 0 || !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        return Err(CodecError::Malformed(
            "I420 requires nonzero even dimensions".into(),
        ));
    }
    let y_len = w
        .checked_mul(h)
        .filter(|n| n.checked_add(n / 2).is_some())
        .ok_or_else(|| CodecError::Malformed("I420 dimensions overflow".into()))?;
    let uv_len = (w / 2) * (h / 2);
    if frame.data.len() < y_len + 2 * uv_len {
        return Err(CodecError::Malformed(format!(
            "I420 buffer too short: {} bytes for {}x{}",
            frame.data.len(),
            w,
            h
        )));
    }
    Ok(I420Ref {
        width: w,
        height: h,
        y: &frame.data[..y_len],
        u: &frame.data[y_len..y_len + uv_len],
        v: &frame.data[y_len + uv_len..y_len + 2 * uv_len],
    })
}

pub struct OpenH264Encoder {
    config: EncoderConfig,
    encoder: H264Encoder,
    keyframe_requested: bool,
    bgra_yuv: Option<YUVBuffer>,
}

impl OpenH264Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self, CodecError> {
        if config.codec != CodecId::H264 {
            return Err(CodecError::Unavailable(config.codec));
        }
        Ok(Self {
            encoder: build_encoder(config)?,
            config,
            keyframe_requested: false,
            bgra_yuv: None,
        })
    }
}

fn build_encoder(config: EncoderConfig) -> Result<H264Encoder, CodecError> {
    let native = H264EncoderConfig::new()
        .bitrate(BitRate::from_bps(config.bitrate_kbps * 1000))
        .max_frame_rate(FrameRate::from_hz(config.framerate as f32))
        .rate_control_mode(RateControlMode::Bitrate);
    H264Encoder::with_api_config(OpenH264API::from_source(), native)
        .map_err(|e| CodecError::Other(e.into()))
}

#[allow(async_fn_in_trait)]
impl Encoder for OpenH264Encoder {
    fn config(&self) -> &EncoderConfig {
        &self.config
    }

    fn reconfigure(&mut self, config: EncoderConfig) {
        // openh264 参数在初始化时固定，码率变更通过重建编码器实现。
        match build_encoder(config) {
            Ok(encoder) => {
                self.encoder = encoder;
                self.config = config;
            }
            Err(err) => tracing::warn!(?err, "encoder reconfigure failed, keeping previous config"),
        }
    }

    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    async fn encode(&mut self, frame: &CapturedFrame) -> Result<Vec<EncodedPacket>, CodecError> {
        if self.keyframe_requested {
            self.encoder.force_intra_frame();
            self.keyframe_requested = false;
        }
        let stream = match frame.format {
            PixelFormat::Bgra8 => {
                let (w, h) = (frame.width as usize, frame.height as usize);
                let expected = w.checked_mul(h).and_then(|n| n.checked_mul(4));
                if w == 0
                    || h == 0
                    || !w.is_multiple_of(2)
                    || !h.is_multiple_of(2)
                    || expected != Some(frame.data.len())
                {
                    return Err(CodecError::Malformed(
                        "BGRA requires packed data and nonzero even dimensions".into(),
                    ));
                }
                if self
                    .bgra_yuv
                    .as_ref()
                    .is_none_or(|b| b.dimensions() != (w, h))
                {
                    self.bgra_yuv = Some(YUVBuffer::new(w, h));
                }
                let yuv = self
                    .bgra_yuv
                    .as_mut()
                    .expect("conversion buffer initialized");
                yuv.read_bgra8(BgraSliceU8::new(&frame.data, (w, h)));
                self.encoder.encode(yuv)
            }
            _ => self.encoder.encode(&split_i420(frame)?),
        }
        .map_err(|e| CodecError::Malformed(e.to_string()))?;
        // Rate control may deliberately skip a frame. It is not a decodable
        // access unit; forwarding it makes the decoder report missing data.
        if matches!(stream.frame_type(), FrameType::Skip) {
            return Ok(vec![]);
        }
        let keyframe = matches!(stream.frame_type(), FrameType::IDR | FrameType::I);
        let data = stream.to_vec();
        if data.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![EncodedPacket {
            keyframe,
            timestamp_us: frame.timestamp_us,
            data: Bytes::from(data),
        }])
    }
}

pub struct OpenH264Decoder {
    decoder: H264Decoder,
}

impl OpenH264Decoder {
    pub fn new() -> Result<Self, CodecError> {
        // Decoder::with_api_config 默认配置即无延迟输出。
        Ok(Self {
            decoder: H264Decoder::with_api_config(
                OpenH264API::from_source(),
                H264DecoderConfig::default(),
            )
            .map_err(|e| CodecError::Other(e.into()))?,
        })
    }
}

#[allow(async_fn_in_trait)]
impl Decoder for OpenH264Decoder {
    async fn decode(&mut self, packet: &EncodedPacket) -> Result<DecodedFrame, CodecError> {
        let decoded = self
            .decoder
            .decode(&packet.data)
            .map_err(|e| CodecError::Malformed(e.to_string()))?
            .ok_or_else(|| CodecError::Malformed("decoder needs more data".into()))?;
        let (w, h) = decoded.dimensions();
        let (cw, ch) = (w / 2, h / 2);
        let (ys, us, vs) = decoded.strides();
        // 解码器输出带对齐 stride，必须逐行紧缩拷贝。
        let mut out = BytesMut::with_capacity(w * h + 2 * cw * ch);
        for row in decoded.y().chunks(ys).take(h) {
            out.extend_from_slice(&row[..w]);
        }
        for (plane, stride) in [(decoded.u(), us), (decoded.v(), vs)] {
            for row in plane.chunks(stride).take(ch) {
                out.extend_from_slice(&row[..cw]);
            }
        }
        Ok(DecodedFrame {
            width: w as u32,
            height: h as u32,
            format: PixelFormat::I420,
            data: Bytes::from(out),
            timestamp_us: packet.timestamp_us,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rd_capture::DamageRect;

    fn test_frame(index: u64) -> CapturedFrame {
        let (w, h) = (128usize, 96usize);
        let mut data = BytesMut::with_capacity(w * h * 3 / 2);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[((x + y + index as usize) % 256) as u8]);
            }
        }
        for i in 0..(w / 2) * (h / 2) * 2 {
            data.extend_from_slice(&[(i % 256) as u8]);
        }
        CapturedFrame {
            width: w as u32,
            height: h as u32,
            format: PixelFormat::I420,
            data: Bytes::from(data),
            damage: Vec::<DamageRect>::new(),
            timestamp_us: index * 33_333,
        }
    }

    #[tokio::test]
    async fn encode_decode_roundtrip() {
        let mut encoder = OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 2000,
            framerate: 30,
            keyframe_interval: 30,
        })
        .unwrap();
        let mut decoder = OpenH264Decoder::new().unwrap();

        let first = encoder.encode(&test_frame(0)).await.unwrap().remove(0);
        assert!(first.keyframe, "首帧应为关键帧");
        let decoded = decoder.decode(&first).await.unwrap();
        assert_eq!((decoded.width, decoded.height), (128, 96));
        assert_eq!(decoded.data.len(), 128 * 96 * 3 / 2);

        // 第二帧（P 帧）应能正常解码出非全黑画面。
        let second = encoder.encode(&test_frame(1)).await.unwrap().remove(0);
        let decoded = decoder.decode(&second).await.unwrap();
        assert!(decoded.data.iter().any(|&b| b > 32));
    }

    #[tokio::test]
    async fn rate_control_skips_do_not_reach_decoder() {
        let mut encoder = OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 1,
            framerate: 30,
            keyframe_interval: 60,
        })
        .unwrap();
        let mut decoder = OpenH264Decoder::new().unwrap();
        let mut skipped = 0;
        let mut decoded_count = 0;
        for index in 0..90 {
            let frame = test_frame(index * 71);
            let packets = encoder.encode(&frame).await.unwrap();
            if packets.is_empty() {
                skipped += 1;
            }
            for packet in packets {
                assert!(!packet.data.is_empty());
                let decoded = decoder.decode(&packet).await.unwrap();
                assert_eq!(decoded.timestamp_us, frame.timestamp_us);
                decoded_count += 1;
            }
        }
        assert!(
            skipped > 0,
            "fixture must exercise encoder rate-control skips"
        );
        assert!(decoded_count > 0);
    }

    #[tokio::test]
    async fn keyframe_request_is_honored() {
        let mut encoder = OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 2000,
            framerate: 30,
            keyframe_interval: 1000,
        })
        .unwrap();
        encoder.encode(&test_frame(0)).await.unwrap();
        encoder.encode(&test_frame(1)).await.unwrap();
        encoder.request_keyframe();
        let forced = encoder.encode(&test_frame(2)).await.unwrap().remove(0);
        assert!(forced.keyframe, "request_keyframe 后应输出关键帧");
    }

    #[tokio::test]
    async fn rejects_unsupported_nv12_input() {
        let mut encoder = OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 2000,
            framerate: 30,
            keyframe_interval: 30,
        })
        .unwrap();
        let mut frame = test_frame(0);
        frame.format = PixelFormat::Nv12;
        assert!(matches!(
            encoder.encode(&frame).await,
            Err(CodecError::Malformed(_))
        ));
    }
}

#[cfg(test)]
mod bgra_tests {
    use super::*;
    #[tokio::test]
    async fn padded_bgra_encodes_decodes_and_resizes() {
        let mut encoder = OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 2000,
            framerate: 30,
            keyframe_interval: 30,
        })
        .unwrap();
        let mut decoder = OpenH264Decoder::new().unwrap();
        for (width, height, color, y_expected, u_expected, v_expected) in [
            (128, 96, [0, 0, 255, 255], 82, 90, 240),
            (64, 64, [255, 0, 0, 255], 41, 240, 110),
        ] {
            let pitch = width as usize * 4 + 16;
            let mut pixels = vec![77; pitch * height as usize];
            for row in pixels.chunks_mut(pitch) {
                for pixel in row[..width as usize * 4].chunks_mut(4) {
                    pixel.copy_from_slice(&color);
                }
            }
            let frame = CapturedFrame::from_bgra_rows(width, height, pitch, &pixels, 123).unwrap();
            let packets = encoder.encode(&frame).await.unwrap();
            let decoded = decoder.decode(&packets[0]).await.unwrap();
            assert_eq!(
                (decoded.width, decoded.height, decoded.timestamp_us),
                (width, height, 123)
            );
            let y_len = (width * height) as usize;
            for (actual, expected) in [
                (decoded.data[0], y_expected),
                (decoded.data[y_len], u_expected),
                (decoded.data[y_len + y_len / 4], v_expected),
            ] {
                assert!(
                    (actual as i32 - expected).abs() <= 5,
                    "actual={actual} expected={expected}"
                );
            }
        }
    }
    #[tokio::test]
    async fn malformed_bgra_returns_error() {
        let mut encoder = OpenH264Encoder::new(EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 2000,
            framerate: 30,
            keyframe_interval: 30,
        })
        .unwrap();
        for (width, height, len) in [(0, 2, 0), (3, 2, 24), (2, 2, 15)] {
            let frame = CapturedFrame {
                width,
                height,
                format: PixelFormat::Bgra8,
                data: vec![0; len].into(),
                damage: vec![],
                timestamp_us: 0,
            };
            assert!(encoder.encode(&frame).await.is_err());
        }
    }
}
