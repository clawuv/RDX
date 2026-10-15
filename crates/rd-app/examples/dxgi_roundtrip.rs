//! cargo run -p rd-app --example dxgi_roundtrip --release -- 1 10
#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rd_app::stats::Samples;
    use rd_capture::{dxgi, CaptureError, Capturer, PixelFormat};
    use rd_codec::{CodecId, Decoder, Encoder, EncoderConfig, OpenH264Decoder, OpenH264Encoder};
    use std::time::{Duration, Instant};
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: dxgi_roundtrip DISPLAY_INDEX SECONDS(1..60)".into());
    }
    let index: usize = args[0].parse()?;
    let seconds: u64 = args[1].parse()?;
    if !(1..=60).contains(&seconds) {
        return Err("seconds must be 1..60".into());
    }
    let displays = dxgi::displays()?;
    let display = displays.get(index).ok_or("display index out of range")?;
    let origin = Instant::now();
    let mut capture = dxgi::RecoveringDxgiCapturer::new(display, origin)?;
    let mut encoder = OpenH264Encoder::new(EncoderConfig {
        codec: CodecId::H264,
        bitrate_kbps: 8000,
        framerate: 30,
        keyframe_interval: 60,
    })?;
    let mut decoder = OpenH264Decoder::new()?;
    let mut capture_ms = Samples::default();
    let mut encode_ms = Samples::default();
    let mut decode_ms = Samples::default();
    let mut acquired_to_decoded_ms = Samples::default();
    let mut count = 0u64;
    let mut timeouts = 0u64;
    let mut skipped = 0u64;
    let mut encoded_bytes = 0usize;
    let mut last_timestamp = None;
    let start = Instant::now();
    let mut pace = tokio::time::interval(Duration::from_secs_f64(1.0 / 30.0));
    pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    println!(
        "display={} {}x{} desktop_origin=({}, {}) requested_fps=30 bitrate_kbps=8000",
        display.name, display.width, display.height, display.desktop_x, display.desktop_y
    );
    while start.elapsed() < Duration::from_secs(seconds) {
        pace.tick().await;
        let t = Instant::now();
        let frame = match capture.next_frame().await {
            Ok(frame) => frame,
            Err(CaptureError::Timeout) => {
                timeouts += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        capture_ms.push(t.elapsed().as_secs_f32() * 1000.0);
        if last_timestamp.is_some_and(|last| frame.timestamp_us < last) {
            return Err("non-monotonic capture timestamp".into());
        }
        last_timestamp = Some(frame.timestamp_us);
        let t = Instant::now();
        let packets = encoder.encode(&frame).await?;
        encode_ms.push(t.elapsed().as_secs_f32() * 1000.0);
        if packets.is_empty() {
            skipped += 1;
            continue;
        }
        for packet in packets {
            encoded_bytes += packet.data.len();
            let t = Instant::now();
            let decoded = decoder.decode(&packet).await?;
            decode_ms.push(t.elapsed().as_secs_f32() * 1000.0);
            if (decoded.width, decoded.height, decoded.timestamp_us)
                != (frame.width, frame.height, frame.timestamp_us)
                || decoded.format != PixelFormat::I420
                || decoded.data.len() != frame.width as usize * frame.height as usize * 3 / 2
            {
                return Err("decoded frame dimensions, layout or timestamp mismatch".into());
            }
            acquired_to_decoded_ms
                .push((origin.elapsed().as_micros() as u64 - frame.timestamp_us) as f32 / 1000.0);
            count += 1;
        }
    }
    println!(
        "summary seconds={:.2} decoded={} timeouts={} fps={:.2} encoded_bytes={} encoder_skipped={}",
        start.elapsed().as_secs_f64(),
        count,
        timeouts,
        count as f64 / start.elapsed().as_secs_f64(),
        encoded_bytes,
        skipped
    );
    for (name, samples) in [
        ("capture_call", capture_ms),
        ("encode", encode_ms),
        ("decode", decode_ms),
        ("acquired_to_decoded", acquired_to_decoded_ms),
    ] {
        println!(
            "{name} rolling_n={} p50/p95={:.2}/{:.2}ms",
            samples.len(),
            samples.percentile(0.5),
            samples.percentile(0.95)
        );
    }
    println!(
        "CPU-only validation; no GPU upload or presentation measurement; no pixel files saved"
    );
    if count == 0 {
        return Err("no decoded frames".into());
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("dxgi_roundtrip requires Windows");
    std::process::exit(1);
}
