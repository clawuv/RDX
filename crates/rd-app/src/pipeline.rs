//! P1 单机回环管道（专用线程 + current-thread tokio 运行时）：
//! 合成捕获 → openh264 编码 → 解码 → UI 通道，并按节拍驱动重绘。

use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

use rd_capture::SyntheticCapturer;
use rd_codec::{
    CodecId, DecodedFrame, Decoder as _, Encoder as _, EncoderConfig, OpenH264Decoder,
    OpenH264Encoder,
};
use rd_protocol::control::KeyframeRequest;

use crate::stats::Samples;

/// 全进程单调起点，捕获时间戳与显示侧 E2E 测量共用。
static ORIGIN: OnceLock<Instant> = OnceLock::new();

pub fn now_us() -> u64 {
    ORIGIN.get_or_init(Instant::now).elapsed().as_micros() as u64
}

#[cfg(feature = "webrtc")]
pub(crate) fn clock_origin() -> Instant {
    *ORIGIN.get_or_init(Instant::now)
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

#[derive(Clone)]
pub struct PipelineConfig {
    pub stream_id: u8,
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
}

pub fn spawn(config: PipelineConfig, repaint: egui::Context) -> watch::Receiver<Option<LoopFrame>> {
    let (_requests_tx, requests_rx) = mpsc::channel(1);
    spawn_inner(config, repaint, requests_rx)
}

/// Start a single-stream encoder loop with a bounded path for active-session
/// recovery requests from rd-engine. The caller owns the sender.
pub fn spawn_with_keyframe_requests(
    config: PipelineConfig,
    repaint: egui::Context,
) -> (
    watch::Receiver<Option<LoopFrame>>,
    mpsc::Sender<KeyframeRequest>,
) {
    let (requests_tx, requests_rx) = mpsc::channel(32);
    (spawn_inner(config, repaint, requests_rx), requests_tx)
}

fn spawn_inner(
    config: PipelineConfig,
    repaint: egui::Context,
    requests_rx: mpsc::Receiver<KeyframeRequest>,
) -> watch::Receiver<Option<LoopFrame>> {
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
        runtime.block_on(run(config, tx, repaint, requests_rx));
    });
    rx
}

async fn run(
    config: PipelineConfig,
    tx: watch::Sender<Option<LoopFrame>>,
    repaint: egui::Context,
    requests: mpsc::Receiver<KeyframeRequest>,
) {
    let PipelineConfig {
        width,
        height,
        framerate,
        ..
    } = config;

    #[cfg(windows)]
    if let Ok(value) = std::env::var("RDX_CAPTURE_DISPLAY") {
        let result = (|| {
            let index: usize = value.parse().map_err(|_| {
                rd_capture::CaptureError::Unavailable("invalid display index".into())
            })?;
            let displays = rd_capture::dxgi::displays()?;
            let display = displays.get(index).ok_or_else(|| {
                rd_capture::CaptureError::Unavailable("display index out of range".into())
            })?;
            rd_capture::dxgi::RecoveringDxgiCapturer::new(
                display,
                *ORIGIN.get_or_init(Instant::now),
            )
        })();
        match result {
            Ok(capturer) => run_captured(capturer, config, tx, repaint, requests).await,
            Err(e) => eprintln!("rdx: DXGI initialization failed: {e}"),
        }
        return;
    }
    #[cfg(target_os = "macos")]
    if let Ok(value) = std::env::var("RDX_CAPTURE_DISPLAY") {
        let result = value
            .parse::<usize>()
            .map_err(|_| rd_capture::CaptureError::Unavailable("invalid display index".into()))
            .and_then(|index| {
                rd_capture::macos_sck::MacScreenCapturer::new(
                    index,
                    width,
                    framerate,
                    *ORIGIN.get_or_init(Instant::now),
                )
            });
        match result {
            Ok(capturer) => run_captured(capturer, config, tx, repaint, requests).await,
            Err(e) => eprintln!("rdx: ScreenCaptureKit initialization failed: {e}"),
        }
        return;
    }
    let capturer = SyntheticCapturer::with_clock_origin(
        width,
        height,
        framerate,
        *ORIGIN.get_or_init(Instant::now),
    );
    run_captured(capturer, config, tx, repaint, requests).await;
}

async fn run_captured(
    mut capturer: impl rd_capture::Capturer,
    config: PipelineConfig,
    tx: watch::Sender<Option<LoopFrame>>,
    repaint: egui::Context,
    mut requests: mpsc::Receiver<KeyframeRequest>,
) {
    let (mut width, mut height) = (capturer.info().width, capturer.info().height);
    let PipelineConfig {
        stream_id,
        framerate,
        bitrate_kbps,
        ..
    } = config;
    if width == 0
        || height == 0
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || framerate == 0
    {
        eprintln!("rdx: capture requires positive even dimensions and positive framerate");
        return;
    }
    let mut pace = tokio::time::interval(Duration::from_secs_f64(1.0 / framerate as f64));
    pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
        pace.tick().await;
        let captured = match capturer.next_frame().await {
            Ok(frame) => frame,
            Err(rd_capture::CaptureError::Timeout) => continue,
            Err(rd_capture::CaptureError::DisplayChanged) => {
                if tx.send(None).is_err() {
                    return;
                }
                repaint.request_repaint();
                if !recover_capture(&mut capturer, &tx).await {
                    return;
                }
                (width, height) = (capturer.info().width, capturer.info().height);
                if width == 0
                    || height == 0
                    || !width.is_multiple_of(2)
                    || !height.is_multiple_of(2)
                {
                    eprintln!("rdx: recovered display has unsupported dimensions");
                    return;
                }
                let (Ok(new_encoder), Ok(new_decoder)) =
                    (OpenH264Encoder::new(encoder_cfg), OpenH264Decoder::new())
                else {
                    eprintln!("rdx: codec recreation failed");
                    return;
                };
                encoder = new_encoder;
                decoder = new_decoder;
                encode_ms = Samples::default();
                decode_ms = Samples::default();
                window_frames = 0;
                window_start_us = now_us();
                pace.reset();
                continue;
            }
            Err(e) => {
                eprintln!("rdx: capture stopped: {e}; restart after display changes");
                return;
            }
        };
        apply_keyframe_requests(&mut encoder, stream_id, &mut requests);
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

pub(crate) fn apply_keyframe_requests(
    encoder: &mut impl rd_codec::Encoder,
    stream_id: u8,
    requests: &mut mpsc::Receiver<KeyframeRequest>,
) -> Option<u64> {
    let after_frame_id = take_keyframe_request(stream_id, requests)?;
    encoder.request_keyframe();
    Some(after_frame_id)
}

// Each pipeline owns one stream; discard requests for other streams.
fn take_keyframe_request(
    stream_id: u8,
    requests: &mut mpsc::Receiver<KeyframeRequest>,
) -> Option<u64> {
    let mut latest = None;
    while let Ok(request) = requests.try_recv() {
        if request.stream_id == u32::from(stream_id) {
            latest = Some(latest.map_or(request.after_frame_id, |id: u64| {
                id.max(request.after_frame_id)
            }));
        }
    }
    latest
}

pub(crate) async fn recover_capture(
    capturer: &mut impl rd_capture::Capturer,
    tx: &watch::Sender<Option<LoopFrame>>,
) -> bool {
    recover_capture_with_delay(capturer, tx, Duration::from_millis(250)).await
}

async fn recover_capture_with_delay(
    capturer: &mut impl rd_capture::Capturer,
    tx: &watch::Sender<Option<LoopFrame>>,
    base_delay: Duration,
) -> bool {
    for attempt in 0..5 {
        if tx.is_closed() {
            return false;
        }
        let delay = base_delay * (1 << attempt);
        tokio::select! {
            _ = tx.closed() => return false,
            _ = tokio::time::sleep(delay) => {}
        }
        match capturer.recover() {
            Ok(()) => {
                eprintln!("rdx: capture recovered on attempt {}", attempt + 1);
                return true;
            }
            Err(e) => eprintln!("rdx: capture recovery attempt {} failed: {e}", attempt + 1),
        }
    }
    eprintln!("rdx: capture recovery exhausted; stopping");
    false
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use rd_capture::{CaptureError, CaptureInfo, CapturedFrame, Capturer, PixelFormat};
    struct Fake {
        attempts: usize,
        succeed_at: usize,
        info: CaptureInfo,
    }
    impl Capturer for Fake {
        fn info(&self) -> &CaptureInfo {
            &self.info
        }
        fn recover(&mut self) -> Result<(), CaptureError> {
            self.attempts += 1;
            if self.attempts >= self.succeed_at {
                Ok(())
            } else {
                Err(CaptureError::DisplayChanged)
            }
        }
        async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
            Err(CaptureError::Timeout)
        }
    }
    fn fake(succeed_at: usize) -> Fake {
        Fake {
            attempts: 0,
            succeed_at,
            info: CaptureInfo {
                width: 2,
                height: 2,
                format: PixelFormat::Bgra8,
                max_framerate: 30,
            },
        }
    }
    #[tokio::test]
    async fn retries_then_recovers() {
        let (tx, _rx) = watch::channel(None);
        let mut capture = fake(3);
        assert!(recover_capture_with_delay(&mut capture, &tx, Duration::ZERO).await);
        assert_eq!(capture.attempts, 3);
    }
    #[tokio::test]
    async fn stops_after_five_attempts() {
        let (tx, _rx) = watch::channel(None);
        let mut capture = fake(6);
        assert!(!recover_capture_with_delay(&mut capture, &tx, Duration::ZERO).await);
        assert_eq!(capture.attempts, 5);
    }
    #[tokio::test]
    async fn closing_ui_cancels_backoff() {
        let (tx, rx) = watch::channel(None);
        let mut capture = fake(1);
        let recovery = recover_capture_with_delay(&mut capture, &tx, Duration::from_secs(60));
        let close = async move {
            tokio::task::yield_now().await;
            drop(rx);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(recovery, close)
        })
        .await
        .unwrap();
        assert!(!result);
        assert_eq!(capture.attempts, 0);
    }
}

#[cfg(test)]
mod keyframe_request_tests {
    use super::*;
    use bytes::Bytes;
    use rd_capture::{CapturedFrame, PixelFormat};

    #[tokio::test]
    async fn keeps_latest_request_for_selected_stream() {
        let (tx, mut rx) = mpsc::channel(8);
        tx.send(KeyframeRequest {
            stream_id: 2,
            after_frame_id: 10,
        })
        .await
        .unwrap();
        tx.send(KeyframeRequest {
            stream_id: 1,
            after_frame_id: 50,
        })
        .await
        .unwrap();
        tx.send(KeyframeRequest {
            stream_id: 2,
            after_frame_id: 12,
        })
        .await
        .unwrap();
        assert_eq!(take_keyframe_request(2, &mut rx), Some(12));
        assert_eq!(take_keyframe_request(1, &mut rx), None);
    }

    #[tokio::test]
    async fn pipeline_request_forces_next_encoded_access_unit_to_idr() {
        let config = EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: 800,
            framerate: 30,
            keyframe_interval: 1000,
        };
        let mut encoder = OpenH264Encoder::new(config).unwrap();
        let frame = CapturedFrame {
            width: 128,
            height: 96,
            format: PixelFormat::I420,
            data: Bytes::from(vec![128; 128 * 96 * 3 / 2]),
            damage: Vec::new(),
            timestamp_us: 0,
        };
        let first = encoder.encode(&frame).await.unwrap().remove(0);
        assert!(first.keyframe);
        let mut next_frame = frame.clone();
        next_frame.timestamp_us = 33_333;
        let next = encoder.encode(&next_frame).await.unwrap().remove(0);
        assert!(!next.keyframe);

        let (tx, mut rx) = mpsc::channel(4);
        tx.send(KeyframeRequest {
            stream_id: 1,
            after_frame_id: 42,
        })
        .await
        .unwrap();
        apply_keyframe_requests(&mut encoder, 1, &mut rx);
        next_frame.timestamp_us = 66_666;
        let recovered = encoder.encode(&next_frame).await.unwrap().remove(0);
        assert!(recovered.keyframe);
    }
}
