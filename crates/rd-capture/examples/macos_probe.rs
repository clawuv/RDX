//! Explicit opt-in screen capture probe; never writes desktop pixels to disk.
#[cfg(target_os = "macos")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rd_capture::{macos_sck::MacScreenCapturer, CaptureError, Capturer};
    let Some(index) = std::env::args().nth(1) else {
        println!(
            "Usage: macos_probe <display-index>; captures one frame only after explicit selection"
        );
        return Ok(());
    };
    let mut capture = MacScreenCapturer::new(index.parse()?, 1920, 30, std::time::Instant::now())?;
    for _ in 0..50 {
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
                return Ok(());
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
