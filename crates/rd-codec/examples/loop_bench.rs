//! Bounded CPU loopback benchmark; never saves or transmits desktop pixels.
use rd_capture::{CaptureError, Capturer, SyntheticCapturer};
use rd_codec::{CodecId, Decoder, Encoder, EncoderConfig, OpenH264Decoder, OpenH264Encoder};
use std::time::{Duration, Instant};

fn percentile(samples: &mut [f64], fraction: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
}

async fn bench(
    mut source: impl Capturer,
    origin: Instant,
    seconds: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut encoder = OpenH264Encoder::new(EncoderConfig {
        codec: CodecId::H264,
        bitrate_kbps: 8000,
        framerate: 30,
        keyframe_interval: 60,
    })?;
    let mut decoder = OpenH264Decoder::new()?;
    let mut encode = Vec::new();
    let mut decode = Vec::new();
    let mut age = Vec::new();
    let mut timeouts = 0;
    let mut bytes = 0usize;
    let start = Instant::now();
    let mut pace = tokio::time::interval(Duration::from_secs_f64(1.0 / 30.0));
    pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while start.elapsed() < Duration::from_secs(seconds) {
        pace.tick().await;
        let frame = match source.next_frame().await {
            Ok(frame) => frame,
            Err(CaptureError::Timeout) => {
                timeouts += 1;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let now = Instant::now();
        let packets = encoder.encode(&frame).await?;
        encode.push(now.elapsed().as_secs_f64() * 1000.0);
        for packet in packets {
            bytes += packet.data.len();
            let now = Instant::now();
            let decoded = decoder.decode(&packet).await?;
            decode.push(now.elapsed().as_secs_f64() * 1000.0);
            if (decoded.width, decoded.height, decoded.timestamp_us)
                != (frame.width, frame.height, frame.timestamp_us)
            {
                return Err("decoded dimensions or timestamp mismatch".into());
            }
            age.push(
                origin
                    .elapsed()
                    .as_micros()
                    .saturating_sub(decoded.timestamp_us as u128) as f64
                    / 1000.0,
            );
        }
    }
    if decode.is_empty() {
        return Err("no decoded frames".into());
    }
    println!(
        "{}x{} elapsed={:.2}s captured={} decoded={} fps={:.2} timeouts={} encoded_bytes={}",
        source.info().width,
        source.info().height,
        start.elapsed().as_secs_f64(),
        encode.len(),
        decode.len(),
        decode.len() as f64 / start.elapsed().as_secs_f64(),
        timeouts,
        bytes
    );
    for (name, values) in [
        ("encode", &mut encode),
        ("decode", &mut decode),
        ("capture_callback_to_decode", &mut age),
    ] {
        println!(
            "{name} p50/p95={:.2}/{:.2}ms",
            percentile(values, 0.5),
            percentile(values, 0.95)
        );
    }
    println!("CPU-only sample, includes startup; excludes GPU upload and presentation.");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        println!("Usage: loop_bench <synthetic|mac:DISPLAY_INDEX> <seconds:1..60>");
        return Ok(());
    }
    let seconds: u64 = args[1].parse()?;
    if !(1..=60).contains(&seconds) {
        return Err("duration must be 1..60 seconds".into());
    }
    let origin = Instant::now();
    if args[0] == "synthetic" {
        return bench(
            SyntheticCapturer::with_clock_origin(1920, 1080, 30, origin),
            origin,
            seconds,
        )
        .await;
    }
    #[cfg(target_os = "macos")]
    if let Some(index) = args[0].strip_prefix("mac:") {
        let source =
            rd_capture::macos_sck::MacScreenCapturer::new(index.parse()?, 1920, 30, origin)?;
        return bench(source, origin, seconds).await;
    }
    Err("unsupported capture source".into())
}
