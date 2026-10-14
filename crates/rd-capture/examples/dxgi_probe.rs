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
    for _ in 0..20 {
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
                if rebuild {
                    capture.recover()?;
                    rebuild = false;
                    println!("duplication recreated; waiting for a new frame");
                } else {
                    return Ok(());
                }
            }
            Err(CaptureError::Timeout) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err("no desktop update within 20 attempts".into())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("DXGI probe requires Windows");
}
