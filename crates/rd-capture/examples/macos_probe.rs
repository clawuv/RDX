//! Explicit opt-in screen capture probe; never writes desktop pixels to disk.
#[cfg(target_os = "macos")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rd_capture::{macos_sck::MacScreenCapturer, CaptureError, Capturer};
    let Some(index) = std::env::args().nth(1) else {
        println!("Usage: macos_probe <display-index> [--recreate | --seconds 1..60]; no desktop pixels are saved");
        return Ok(());
    };
    let mode = std::env::args().nth(2);
    let sample_seconds = if mode.as_deref() == Some("--seconds") {
        let seconds: u64 = std::env::args().nth(3).ok_or("missing duration")?.parse()?;
        if !(1..=60).contains(&seconds) {
            return Err("duration must be 1..60 seconds".into());
        }
        Some(seconds)
    } else {
        None
    };
    let mut recreate = match mode.as_deref() {
        None => false,
        Some("--seconds") => false,
        Some("--recreate") => true,
        Some(_) => return Err("unknown probe option".into()),
    };
    let origin = std::time::Instant::now();
    let mut capture = MacScreenCapturer::new(index.parse()?, 1920, 30, origin)?;
    if let Some(seconds) = sample_seconds {
        let start = std::time::Instant::now();
        let mut ages = Vec::new();
        let mut timeouts = 0;
        let mut previous = None;
        while start.elapsed().as_secs_f64() < seconds as f64 {
            match capture.next_frame().await {
                Ok(frame) => {
                    if previous.is_some_and(|last| frame.timestamp_us < last) {
                        return Err("capture clock went backwards".into());
                    }
                    previous = Some(frame.timestamp_us);
                    let expected = frame.width as usize * frame.height as usize * 3 / 2;
                    if frame.data.len() != expected {
                        return Err("unexpected frame size".into());
                    }
                    ages.push(
                        origin
                            .elapsed()
                            .as_micros()
                            .saturating_sub(frame.timestamp_us as u128)
                            as f64
                            / 1000.0,
                    );
                }
                Err(CaptureError::Timeout) => timeouts += 1,
                Err(error) => return Err(error.into()),
            }
        }
        if ages.is_empty() {
            return Err("no frames during continuous sample".into());
        }
        ages.sort_by(f64::total_cmp);
        println!("capture diagnostics: {:?}", capture.diagnostics());
        let percentile = |p: f64| ages[((ages.len() - 1) as f64 * p).round() as usize];
        println!("capture sample: {}x{}, frames={}, elapsed={:.2}s, fps={:.2}, timeouts={}, callback_to_consumer_age_p50/p95={:.2}/{:.2}ms",
            capture.info().width, capture.info().height, ages.len(), start.elapsed().as_secs_f64(),
            ages.len() as f64 / start.elapsed().as_secs_f64(), timeouts, percentile(0.5), percentile(0.95));
        return Ok(());
    }
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut previous_timestamp = None;
    while std::time::Instant::now() < deadline {
        match capture.next_frame().await {
            Ok(frame) => {
                println!(
                    "captured {}x{} {:?}, bytes={}, timestamp_us={}",
                    frame.width,
                    frame.height,
                    frame.format,
                    frame.data.len(),
                    frame.timestamp_us
                );
                if let Some(previous) = previous_timestamp {
                    if frame.timestamp_us < previous {
                        return Err("capture clock went backwards after recreation".into());
                    }
                }
                if recreate {
                    previous_timestamp = Some(frame.timestamp_us);
                    capture.recover()?;
                    recreate = false;
                    deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    println!("original display stream recreated; waiting for a fresh frame");
                } else {
                    return Ok(());
                }
            }
            Err(CaptureError::Timeout) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(
        "No frame within five seconds; check Screen Recording permission and display availability"
            .into(),
    )
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_probe requires macOS");
}
