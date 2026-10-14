//! P1 单机回环管道（专用线程 + current-thread tokio 运行时）：
//! 合成捕获 → openh264 编码 → 解码 → UI 通道，并按节拍驱动重绘。

use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::watch;

use rd_capture::{Capturer as _, SyntheticCapturer};
use rd_codec::{
    CodecId, DecodedFrame, Decoder as _, Encoder as _, EncoderConfig, OpenH264Decoder,
    OpenH264Encoder,
};

use crate::stats::Samples;

/// 全进程单调起点，捕获时间戳与显示侧 E2E 测量共用。
static ORIGIN: OnceLock<Instant> = OnceLock::new();

pub fn now_us() -> u64 {
    ORIGIN.get_or_init(Instant::now).elapsed().as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_consumer_keeps_only_latest_frame_and_disconnects() {
        let (tx, mut rx) = watch::channel(None);
        for timestamp_us in 0..100 {
            tx.send(Some(LoopFrame {
                frame: DecodedFrame {
                    width: 2,
                    height: 2,
                    format: rd_capture::PixelFormat::I420,
                    data: bytes::Bytes::from_static(&[16, 16, 16, 16, 128, 128]),
                    timestamp_us,
                },
                encode_ms: 0.0,
                decode_ms: 0.0,
            }))
            .unwrap();
        }
        assert_eq!(
            rx.borrow_and_update().as_ref().unwrap().frame.timestamp_us,
            99
        );
        assert!(!rx.has_changed().unwrap());
        drop(rx);
        assert!(tx.is_closed());
        assert!(tx.send(None).is_err());
    }
}

#[derive(Clone)]
pub struct LoopFrame {
    pub frame: DecodedFrame,
    pub encode_ms: f32,
    pub decode_ms: f32,
}

pub struct PipelineConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
}

pub fn spawn(config: PipelineConfig, repaint: egui::Context) -> watch::Receiver<Option<LoopFrame>> {
    // One replaceable slot: a stalled UI never accumulates decoded frames.
    let (tx, rx) = watch::channel(None);
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let Ok(runtime) = runtime else {
            eprintln!("rdx: pipeline runtime init failed");
            return;
        };
        runtime.block_on(run(config, tx, repaint));
    });
    rx
}

async fn run(config: PipelineConfig, tx: watch::Sender<Option<LoopFrame>>, repaint: egui::Context) {
    let PipelineConfig {
        width,
        height,
        framerate,
        bitrate_kbps,
    } = config;

    let mut capturer = SyntheticCapturer::with_clock_origin(
        width,
        height,
        framerate,
        *ORIGIN.get_or_init(Instant::now),
    );
    let encoder_cfg = EncoderConfig {
        codec: CodecId::H264,
        bitrate_kbps,
        framerate,
        keyframe_interval: framerate * 2,
    };
    let Ok(mut encoder) = OpenH264Encoder::new(encoder_cfg) else {
        eprintln!("rdx: openh264 encoder init failed");
        return;
    };
    let Ok(mut decoder) = OpenH264Decoder::new() else {
        eprintln!("rdx: openh264 decoder init failed");
        return;
    };

    let mut encode_ms = Samples::default();
    let mut decode_ms = Samples::default();
    let mut window_frames: u64 = 0;
    let mut window_start_us = now_us();
    const REPORT_INTERVAL: Duration = Duration::from_secs(5);

    loop {
        if tx.is_closed() {
            return;
        }
        let Ok(captured) = capturer.next_frame().await else {
            eprintln!("rdx: synthetic capture failed, stopping pipeline");
            return;
        };
        let t_encode_start = now_us();
        let Ok(packets) = encoder.encode(&captured).await else {
            eprintln!("rdx: encode failed, dropping frame");
            continue;
        };
        let t_encode_end = now_us();
        encode_ms.push((t_encode_end - t_encode_start) as f32 / 1000.0);

        for packet in packets {
            let t_decode_start = now_us();
            let Ok(decoded) = decoder.decode(&packet).await else {
                eprintln!("rdx: decode failed, dropping packet");
                continue;
            };
            let t_decode_end = now_us();
            decode_ms.push((t_decode_end - t_decode_start) as f32 / 1000.0);
            if tx
                .send(Some(LoopFrame {
                    frame: decoded,
                    encode_ms: encode_ms.percentile(0.95).max(0.0),
                    decode_ms: decode_ms.percentile(0.95).max(0.0),
                }))
                .is_err()
            {
                return; // UI 已退出
            }
            repaint.request_repaint();
        }

        window_frames += 1;
        let elapsed_us = now_us() - window_start_us;
        if elapsed_us >= REPORT_INTERVAL.as_micros() as u64 {
            let fps = window_frames as f64 / (elapsed_us as f64 / 1e6);
            eprintln!(
                "rdx pipeline: {}x{} @{:.0}fps | fps={:.1} encode p50/p95={:.2}/{:.2}ms decode p50/p95={:.2}/{:.2}ms",
                width,
                height,
                framerate,
                fps,
                encode_ms.percentile(0.5),
                encode_ms.percentile(0.95),
                decode_ms.percentile(0.5),
                decode_ms.percentile(0.95),
            );
            window_frames = 0;
            window_start_us = now_us();
        }
    }
}
