//! Local-only WebRTC development pipeline, not an authenticated remote session.
//! Both peers live in this process; only the newest decoded frame reaches UI.

use crate::pipeline::{LoopFrame, PipelineConfig};
use crate::remote_media::{MediaReceiver, MediaResult, MediaSender};
use rd_capture::{CaptureError, Capturer, SyntheticCapturer};
use rd_codec::{CodecId, Encoder as _, EncoderConfig, OpenH264Encoder};
use rd_transport::Str0mEvent;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

/// Lifetime retry budget: successful frames do not reset the budget.
const MAX_RECONNECTS: usize = 3;

#[derive(Clone)]
enum CaptureSource {
    Synthetic,
    #[cfg(windows)]
    Display(String),
}

fn capture_source() -> MediaResult<CaptureSource> {
    if let Some(value) = std::env::var_os("RDX_CAPTURE_DISPLAY") {
        #[cfg(windows)]
        {
            let index: usize = value.to_str().ok_or("invalid display index")?.parse()?;
            let displays = rd_capture::dxgi::displays()?;
            let display = displays.get(index).ok_or("display index out of range")?;
            return Ok(CaptureSource::Display(display.name.clone()));
        }
        #[cfg(not(windows))]
        {
            let _ = value;
            return Err("WebRTC display capture is currently Windows-only".into());
        }
    }
    Ok(CaptureSource::Synthetic)
}

async fn run_reconnecting(
    config: PipelineConfig,
    tx: watch::Sender<Option<LoopFrame>>,
    repaint: egui::Context,
) -> MediaResult<()> {
    let source = capture_source()?;
    reconnect(
        || run_selected(config.clone(), tx.clone(), repaint.clone(), source.clone()),
        &tx,
        &repaint,
        Duration::from_millis(250),
    )
    .await
}

fn retryable(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error.is::<rd_transport::TransportError>() || error.is::<tokio::time::error::Elapsed>()
}

async fn reconnect<F, Fut>(
    mut attempt: F,
    tx: &watch::Sender<Option<LoopFrame>>,
    repaint: &egui::Context,
    base_delay: Duration,
) -> MediaResult<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = MediaResult<()>>,
{
    for retry in 0..=MAX_RECONNECTS {
        let result = tokio::select! {
            biased;
            _ = tx.closed() => return Ok(()),
            result = attempt() => result,
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if !retryable(error.as_ref()) {
            return Err(error);
        }
        // Attempt has returned: its sockets, tasks and decoder are already gone.
        if tx.send(None).is_err() {
            return Ok(());
        }
        repaint.request_repaint();
        if retry == MAX_RECONNECTS {
            return Err(error);
        }
        let delay = base_delay * (1 << retry);
        eprintln!(
            "rdx WebRTC reconnect {}/{} in {}ms: {error}",
            retry + 1,
            MAX_RECONNECTS,
            delay.as_millis()
        );
        tokio::select! {
            biased;
            _ = tx.closed() => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
    }
    unreachable!("bounded retry loop always returns")
}

pub fn spawn_loopback(
    config: PipelineConfig,
    repaint: egui::Context,
) -> watch::Receiver<Option<LoopFrame>> {
    let (tx, rx) = watch::channel(None);
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })
            .and_then(|runtime| runtime.block_on(run_reconnecting(config, tx, repaint)));
        if let Err(error) = result {
            eprintln!("rdx WebRTC loop stopped: {error}");
        }
    });
    rx
}

#[cfg(test)]
async fn run_loopback(
    config: PipelineConfig,
    tx: watch::Sender<Option<LoopFrame>>,
    repaint: egui::Context,
) -> MediaResult<()> {
    run_selected(config, tx, repaint, CaptureSource::Synthetic).await
}

async fn run_selected(
    config: PipelineConfig,
    tx: watch::Sender<Option<LoopFrame>>,
    repaint: egui::Context,
    source: CaptureSource,
) -> MediaResult<()> {
    #[cfg(windows)]
    if let CaptureSource::Display(name) = source {
        let displays = rd_capture::dxgi::displays()?;
        let mut matches = displays.iter().filter(|display| display.name == name);
        let display = matches.next().ok_or("selected display missing")?;
        if matches.next().is_some() {
            return Err("selected display is ambiguous".into());
        }
        let capture = rd_capture::dxgi::RecoveringDxgiCapturer::new(
            display,
            crate::pipeline::clock_origin(),
        )?;
        return run_captured(config, tx, repaint, capture).await;
    }
    #[cfg(not(windows))]
    let CaptureSource::Synthetic = source;
    // Validate the rate before constructing the synthetic source (division).
    if !(1..=240).contains(&config.framerate) {
        return Err("invalid WebRTC loop framerate".into());
    }
    let capture = SyntheticCapturer::with_clock_origin(
        config.width,
        config.height,
        config.framerate,
        crate::pipeline::clock_origin(),
    );
    run_captured(config, tx, repaint, capture).await
}

async fn run_captured(
    config: PipelineConfig,
    tx: watch::Sender<Option<LoopFrame>>,
    repaint: egui::Context,
    mut capture: impl Capturer,
) -> MediaResult<()> {
    let valid_dimensions = |width: u32, height: u32| {
        width > 0 && height > 0 && width.is_multiple_of(2) && height.is_multiple_of(2)
    };
    if !valid_dimensions(capture.info().width, capture.info().height)
        || !(1..=240).contains(&config.framerate)
        || config.bitrate_kbps == 0
    {
        return Err("invalid WebRTC loop dimensions, framerate or bitrate".into());
    }
    let session = async {
        let (mut host, mut viewer) = rd_transport::connect_pair()?;
        tokio::try_join!(
            crate::remote::exchange_capabilities(&mut host),
            crate::remote::exchange_capabilities(&mut viewer)
        )?;
        let mut events = host.take_events().ok_or("host events already taken")?;
        let (requests_tx, mut requests_rx) = mpsc::channel(32);
        let encoder_config = EncoderConfig {
            codec: CodecId::H264,
            bitrate_kbps: config.bitrate_kbps,
            framerate: config.framerate,
            keyframe_interval: config.framerate * 2,
        };
        let mut encoder = OpenH264Encoder::new(encoder_config)?;
        let mut sender = MediaSender::new(config.stream_id);
        let mut receiver = MediaReceiver::new(config.stream_id)?;
        let produce = async {
            let mut pace =
                tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(config.framerate)));
            pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if tx.is_closed() {
                    return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
                }
                pace.tick().await;
                let captured = match capture.next_frame().await {
                    Ok(frame) => frame,
                    // Unchanged desktops may legitimately produce no frames.
                    Err(CaptureError::Timeout) => continue,
                    Err(CaptureError::DisplayChanged) => {
                        tx.send(None)?;
                        repaint.request_repaint();
                        if !crate::pipeline::recover_capture(&mut capture, &tx).await {
                            if tx.is_closed() {
                                return Ok(());
                            }
                            return Err("capture recovery exhausted".into());
                        }
                        if !valid_dimensions(capture.info().width, capture.info().height) {
                            return Err("recovered capture has unsupported dimensions".into());
                        }
                        // New encoder emits SPS/PPS + IDR; keep media IDs monotonic.
                        encoder = OpenH264Encoder::new(encoder_config)?;
                        pace.reset();
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                crate::pipeline::apply_keyframe_requests(
                    &mut encoder,
                    config.stream_id,
                    &mut requests_rx,
                );
                for packet in encoder.encode(&captured).await? {
                    tokio::time::timeout(Duration::from_secs(5), sender.send(&mut host, packet))
                        .await??;
                }
            }
        };
        let consume = async {
            loop {
                if tx.is_closed() {
                    return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
                }
                let start = Instant::now();
                // Silence alone is not disconnection: a desktop can be static.
                let decoded = receiver.receive_webrtc(&mut viewer).await?;
                if let Some(frame) = decoded {
                    tx.send(Some(LoopFrame {
                        frame,
                        // Sender encode cost is not carried in the media format.
                        encode_ms: 0.0,
                        // Includes network waiting, not a decoder-only benchmark.
                        decode_ms: start.elapsed().as_secs_f32() * 1000.0,
                    }))?;
                    repaint.request_repaint();
                }
            }
        };
        let recovery = async {
            while let Some(event) = events.recv().await {
                if let Str0mEvent::KeyframeRequested {
                    stream_id,
                    after_frame_id,
                } = event
                {
                    requests_tx
                        .send(rd_protocol::control::KeyframeRequest {
                            stream_id: u32::from(stream_id),
                            after_frame_id,
                        })
                        .await?;
                }
            }
            Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                rd_transport::TransportError::Closed.into(),
            )
        };
        tokio::try_join!(produce, consume, recovery)?;
        Ok(())
    };
    // Dropping all UI receivers cancels setup, network waits and every worker.
    tokio::select! {
        _ = tx.closed() => Ok(()),
        result = session => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PipelineConfig {
        PipelineConfig {
            stream_id: 1,
            width: 128,
            height: 96,
            framerate: 30,
            bitrate_kbps: 800,
        }
    }

    #[tokio::test]
    async fn reconnect_has_lifetime_budget_and_never_retries_incompatible_peer() {
        let (tx, _rx) = watch::channel(None);
        let ctx = egui::Context::default();
        let mut attempts = 0;
        let result = reconnect(
            || {
                attempts += 1;
                async { Err(rd_transport::TransportError::Closed.into()) }
            },
            &tx,
            &ctx,
            Duration::ZERO,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, MAX_RECONNECTS + 1);
        attempts = 0;
        let result = reconnect(
            || {
                attempts += 1;
                async { Err(rd_protocol::control::IncompatibleCapabilities.into()) }
            },
            &tx,
            &ctx,
            Duration::ZERO,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn closing_ui_cancels_reconnect_backoff() {
        let (tx, rx) = watch::channel(None);
        let ctx = egui::Context::default();
        let mut attempts = 0;
        let worker = reconnect(
            || {
                attempts += 1;
                async { Err(rd_transport::TransportError::Closed.into()) }
            },
            &tx,
            &ctx,
            Duration::from_secs(60),
        );
        let close = async {
            tokio::task::yield_now().await;
            drop(rx);
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(worker, close)
        })
        .await
        .unwrap();
        result.unwrap();
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn peer_shutdown_triggers_reconnect_and_replaces_old_media() {
        let (tx, mut rx) = watch::channel(None);
        let ctx = egui::Context::default();
        let mut attempts = 0;
        let worker = reconnect(
            || {
                attempts += 1;
                let current = attempts;
                let tx = tx.clone();
                let ctx = ctx.clone();
                async move {
                    if current > 1 {
                        let mut next = config();
                        next.width = 160;
                        next.height = 120;
                        return run_loopback(next, tx, ctx).await;
                    }
                    // Stop the actual peer after decoding real media. The
                    // receiver must discover the loss itself; do not inject
                    // a synthetic Closed error into the supervisor.
                    let (mut host, mut viewer) = rd_transport::connect_pair()?;
                    tokio::try_join!(
                        crate::remote::exchange_capabilities(&mut host),
                        crate::remote::exchange_capabilities(&mut viewer)
                    )?;
                    let mut capture = SyntheticCapturer::with_clock_origin(
                        128,
                        96,
                        30,
                        crate::pipeline::clock_origin(),
                    );
                    let mut encoder = OpenH264Encoder::new(EncoderConfig {
                        codec: CodecId::H264,
                        bitrate_kbps: 800,
                        framerate: 30,
                        keyframe_interval: 60,
                    })?;
                    let mut sender = MediaSender::new(1);
                    let mut receiver = MediaReceiver::new(1)?;
                    for _ in 0..3 {
                        let captured = capture.next_frame().await?;
                        for packet in encoder.encode(&captured).await? {
                            sender.send(&mut host, packet).await?;
                            let frame = receiver
                                .receive_webrtc(&mut viewer)
                                .await?
                                .ok_or("missing initial frame")?;
                            tx.send(Some(LoopFrame {
                                frame,
                                encode_ms: 0.0,
                                decode_ms: 0.0,
                            }))?;
                            tokio::task::yield_now().await;
                        }
                    }
                    drop(host);
                    let error = tokio::time::timeout(
                        Duration::from_secs(10),
                        receiver.receive_webrtc(&mut viewer),
                    )
                    .await
                    .expect("peer shutdown must terminate media reception")
                    .expect_err("stopped peer cannot produce another video frame");
                    assert!(error.is::<rd_transport::TransportError>());
                    Err(error)
                }
            },
            &tx,
            &ctx,
            Duration::from_millis(25),
        );
        let ui = async {
            let mut saw_old = false;
            let mut saw_clear = false;
            loop {
                rx.changed().await.unwrap();
                let latest = rx.borrow_and_update().clone();
                match latest {
                    Some(frame) if frame.frame.width == 128 => saw_old = true,
                    None => saw_clear = true,
                    Some(frame) if frame.frame.width == 160 => {
                        assert_eq!(frame.frame.height, 120);
                        assert!(saw_old && saw_clear);
                        break;
                    }
                    _ => panic!("unexpected frame after reconnect"),
                }
            }
            drop(rx);
        };
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(worker, ui) })
                .await
                .unwrap();
        result.unwrap();
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn continuous_webrtc_replaces_stalled_ui_slot_and_stops_on_close() {
        let (tx, mut rx) = watch::channel(None);
        let worker = run_loopback(config(), tx, egui::Context::default());
        let ui = async {
            tokio::time::timeout(Duration::from_secs(10), rx.changed())
                .await
                .unwrap()
                .unwrap();
            let first = rx.borrow_and_update().as_ref().unwrap().frame.timestamp_us;
            tokio::time::sleep(Duration::from_millis(350)).await;
            let latest = rx.borrow_and_update().clone().unwrap().frame;
            assert!(latest.timestamp_us > first + 100_000);
            assert_eq!((latest.width, latest.height), (128, 96));
            assert!(!rx.has_changed().unwrap());
            drop(rx);
        };
        tokio::time::timeout(Duration::from_secs(12), async {
            let (result, ()) = tokio::join!(worker, ui);
            result.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rejects_invalid_config_and_closes_output() {
        let (tx, rx) = watch::channel(None);
        let mut invalid = config();
        invalid.framerate = 0;
        assert!(run_loopback(invalid, tx, egui::Context::default())
            .await
            .is_err());
        assert!(rx.has_changed().is_err());
    }

    struct ChangingCapture {
        source: SyntheticCapturer,
        stage: u8,
    }

    impl Capturer for ChangingCapture {
        fn info(&self) -> &rd_capture::CaptureInfo {
            self.source.info()
        }

        fn recover(&mut self) -> Result<(), CaptureError> {
            self.source =
                SyntheticCapturer::with_clock_origin(160, 120, 30, crate::pipeline::clock_origin());
            Ok(())
        }

        async fn next_frame(&mut self) -> Result<rd_capture::CapturedFrame, CaptureError> {
            self.stage = self.stage.saturating_add(1);
            match self.stage {
                2 => {
                    tokio::time::sleep(Duration::from_millis(5200)).await;
                    Err(CaptureError::Timeout)
                }
                3 => Err(CaptureError::DisplayChanged),
                _ => self.source.next_frame().await,
            }
        }
    }

    #[tokio::test]
    async fn idle_desktop_survives_then_resolution_change_decodes_over_webrtc() {
        let (tx, mut rx) = watch::channel(None);
        let capture = ChangingCapture {
            source: SyntheticCapturer::with_clock_origin(
                128,
                96,
                30,
                crate::pipeline::clock_origin(),
            ),
            stage: 0,
        };
        let worker = run_captured(config(), tx, egui::Context::default(), capture);
        let ui = async {
            let mut saw_initial = false;
            let mut saw_clear = false;
            loop {
                rx.changed()
                    .await
                    .expect("pipeline stopped before recovery");
                let latest = rx.borrow_and_update().clone();
                match latest {
                    Some(frame) if frame.frame.width == 128 => saw_initial = true,
                    None => saw_clear = true,
                    Some(frame) if frame.frame.width == 160 => {
                        assert_eq!(frame.frame.height, 120);
                        assert!(saw_initial && saw_clear);
                        break;
                    }
                    _ => panic!("unexpected capture dimensions"),
                }
            }
            drop(rx);
        };
        tokio::time::timeout(Duration::from_secs(15), async {
            let (result, ()) = tokio::join!(worker, ui);
            result.unwrap();
        })
        .await
        .unwrap();
    }
}
