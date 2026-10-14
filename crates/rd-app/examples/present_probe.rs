//! 呈现观测探针（仅 macOS）：
//! `cargo run -p rd-app --example present_probe --locked -- 0 20 /tmp/pattern-trace.log`
//!
//! 前置：pattern_window 以 `RDX_PATTERN_FULLSCREEN=1 RDX_PATTERN_TRACE=1`
//! 全屏运行于同一显示器。本探针用 ScreenCaptureKit 捕获该显示器，从 Y 平面
//! 读回帧号指纹，与图案侧逐帧 trace（墙钟微秒）对齐，输出“图案帧发布 →
//! 被显示器内容捕获观察到”的时延分布。该时延包含窗口合成、显示刷新与
//! SCK 采集链路，是呈现时刻的观测上界，不是 GPU 呈现回调。
//! 只统计指纹发生变化的观测（新内容到达）；冻结段（如最小化）另行汇报。
//! 本探针不保存、不发送任何屏幕像素。

#[cfg(target_os = "macos")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rd_app::pattern;
    use rd_app::stats::Samples;
    use rd_capture::{macos_sck::MacScreenCapturer, CaptureError, Capturer};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn unix_us() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0)
    }

    let mut args = std::env::args().skip(1);
    let (Some(index), Some(seconds), Some(trace_path)) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: present_probe <display-index> <seconds 5..60> <pattern-trace-file>");
        return Ok(());
    };
    let index: usize = index.parse()?;
    let seconds: u64 = seconds.parse()?;
    if !(5..=60).contains(&seconds) {
        return Err("seconds must be 5..60".into());
    }

    let origin = Instant::now();
    let origin_unix = unix_us();
    // 指纹行偏移：无边框条带窗口内容起点在菜单栏下方（实测约 30px）；
    // 全屏图案为 0。RDX_FP_Y_OFFSET=0..1024。
    let fp_y_offset: usize = std::env::var("RDX_FP_Y_OFFSET")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v <= 1024)
        .unwrap_or(0);
    // max_width 传 8192：不缩放，指纹几何按原生设备像素对齐。
    let mut capture = MacScreenCapturer::new(index, 8192, 60, origin)?;

    fn unix_us_elapsed(origin: Instant, start: Instant) -> u64 {
        (start.elapsed() + start.duration_since(origin)).as_micros() as u64
    }

    let mut observations: Vec<(u64, u8)> = Vec::new();
    let mut frames_total = 0u64;
    let mut timeouts = 0u64;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        match capture.next_frame().await {
            Ok(frame) => {
                frames_total += 1;
                let y_len = frame.width as usize * frame.height as usize;
                if let Some(bits) = pattern::read_fingerprint_with_offset(
                    &frame.data[..y_len.min(frame.data.len())],
                    frame.width as usize,
                    fp_y_offset,
                ) {
                    observations.push((origin_unix + frame.timestamp_us, bits));
                }
            }
            Err(CaptureError::Timeout) => timeouts += 1,
            Err(error) => return Err(error.into()),
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!("capture diagnostics: {:?}", capture.diagnostics());

    let trace_text = std::fs::read_to_string(&trace_path)?;
    let trace = pattern::parse_trace(&trace_text);
    if trace.is_empty() {
        return Err("no pattern-trace lines found".into());
    }

    // 只对“指纹变化”的观测配对：新内容到达时刻与该帧发布时刻之差。
    const MAX_DELAY_US: u64 = 2_000_000;
    let mut transitions = 0u64;
    let mut previous_bits = None;
    let mut fingerprint_transitions: Vec<(u64, u8)> = Vec::new();
    for (t_obs, bits) in &observations {
        if previous_bits == Some(*bits) {
            continue;
        }
        previous_bits = Some(*bits);
        transitions += 1;
        fingerprint_transitions.push((*t_obs, *bits));
    }
    let matched_delays = pattern::match_delays_ms(&trace, &fingerprint_transitions, MAX_DELAY_US);
    let matched = matched_delays.len() as u64;
    let max_delay_ms = matched_delays.iter().copied().fold(0.0f32, f32::max);
    let mut delays = Samples::default();
    for delay in matched_delays {
        delays.push(delay);
    }
    let transition_times: Vec<u64> = fingerprint_transitions.iter().map(|(t, _)| *t).collect();

    // 相邻转场间隔 >1s 视为冻结段（含最小化/隐藏窗口、静止桌面）；
    // 末次转场到采样结束的尾段冻结也计入。
    let sample_end_unix = origin_unix + unix_us_elapsed(origin, start);
    let mut freeze_segments = 0u64;
    let mut freeze_total_ms = 0.0f32;
    let mut freeze_longest_ms = 0.0f32;
    let mut boundaries: Vec<u64> = transition_times.clone();
    boundaries.push(sample_end_unix);
    for pair in boundaries.windows(2) {
        let gap_ms = (pair[1] - pair[0]) as f32 / 1000.0;
        if gap_ms > 1000.0 {
            freeze_segments += 1;
            freeze_total_ms += gap_ms;
            freeze_longest_ms = freeze_longest_ms.max(gap_ms);
        }
    }
    let mut distinct_bits: Vec<u8> = observations.iter().map(|(_, bits)| *bits).collect();
    distinct_bits.sort_unstable();
    distinct_bits.dedup();

    println!(
        "present probe: {}x{} elapsed={elapsed:.2}s frames={} observations={} distinct_bits={} timeouts={} transitions={} matched={} delay_p50/p95/max={:.2}/{:.2}/{:.2}ms freeze>1s={} total={:.0}ms longest={:.0}ms (observed by display capture; includes compositing and SCK pipeline, not a GPU present callback)",
        capture.info().width,
        capture.info().height,
        frames_total,
        observations.len(),
        distinct_bits.len(),
        timeouts,
        transitions,
        matched,
        delays.percentile(0.5),
        delays.percentile(0.95),
        max_delay_ms,
        freeze_segments,
        freeze_total_ms,
        freeze_longest_ms,
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("present_probe is macOS-only (ScreenCaptureKit)");
}
