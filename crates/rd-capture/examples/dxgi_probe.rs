#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rd_capture::{dxgi, CaptureError, Capturer};
    let displays = dxgi::displays()?;
    for (index, display) in displays.iter().enumerate() {
        println!("{index}: {display:?}");
    }
    let Some(index) = std::env::args().nth(1) else {
        return Ok(());
    };
    let index: usize = index.parse()?;
    let display = displays.get(index).ok_or("display index out of range")?;
    let mut capture = dxgi::RecoveringDxgiCapturer::new(display, std::time::Instant::now())?;
    let mut rebuild = std::env::args().nth(2).as_deref() == Some("--recreate");
    let seconds = match std::env::args().nth(2).as_deref() {
        Some("--seconds") => std::env::args()
            .nth(3)
            .ok_or("missing seconds")?
            .parse::<u64>()?,
        None | Some("--recreate") => 0,
        _ => return Err("use --recreate or --seconds 1..60".into()),
    };
    if seconds > 60 || (std::env::args().nth(2).as_deref() == Some("--seconds") && seconds == 0) {
        return Err("seconds must be 1..60".into());
    }
    let start = std::time::Instant::now();
    let mut frames = 0u64;
    let mut timeouts = 0u64;
    let mut bytes = 0u64;
    let mut timings = Vec::new();
    let mut pace = tokio::time::interval(std::time::Duration::from_millis(33));
    pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    for attempt in 0.. {
        if seconds == 0 && attempt >= 20 {
            break;
        }
        if seconds > 0 && start.elapsed().as_secs() >= seconds {
            break;
        }
        pace.tick().await;
        let before = std::time::Instant::now();
        match capture.next_frame().await {
            Ok(frame) => {
                frames += 1;
                bytes += frame.data.len() as u64;
                timings.push(before.elapsed().as_secs_f64() * 1000.0);
                if seconds > 0 {
                    continue;
                }
                println!(
                    "captured {}x{} {:?}, bytes={}, timestamp_us={}",
                    frame.width,
                    frame.height,
                    frame.format,
                    frame.data.len(),
                    frame.timestamp_us
                );
                if rebuild {
                    capture.recover()?;
                    rebuild = false;
                    println!("duplication recreated; waiting for a new frame");
                } else {
                    return Ok(());
                }
            }
            Err(CaptureError::Timeout) => {
                timeouts += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    if seconds > 0 {
        timings.sort_by(f64::total_cmp);
        let percentile = |p: f64| {
            timings
                .get(((timings.len() as f64 * p).ceil() as usize).saturating_sub(1))
                .copied()
                .unwrap_or(0.0)
        };
        println!("summary: seconds={:.2} frames={} timeouts={} fps={:.2} bytes={} capture_call_p50/p95={:.2}/{:.2}ms (includes wait/readback/cursor; no encode/GPU presentation)",start.elapsed().as_secs_f64(),frames,timeouts,frames as f64/start.elapsed().as_secs_f64(),bytes,percentile(0.5),percentile(0.95));
        return if frames > 0 {
            Ok(())
        } else {
            Err("no frames captured".into())
        };
    }
    Err("no desktop update within 20 attempts".into())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("DXGI probe requires Windows");
}
