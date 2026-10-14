//! macOS ScreenCaptureKit 捕获后端（P1）。
//!
//! 采用 420v（NV12 双平面）输出，捕获边界处一次去交织转成 I420，
//! 下游（openh264 编码、GPU 渲染）保持单一平面格式。
//! 需要 Screen Recording 权限（TCC）；超时不单独视为权限被拒绝。

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::watch;

use bytes::Bytes;
use screencapturekit::cm::CMSampleBufferExt;
use screencapturekit::cv::CVPixelBufferLockGuard;
use screencapturekit::prelude::{
    CMSampleBuffer, SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutputTrait, SCStreamOutputType,
};
use screencapturekit::stream::configuration::PixelFormat as SckPixelFormat;

use crate::{CaptureError, CaptureInfo, CapturedFrame, Capturer, PixelFormat};

/// 已转为 I420 的一帧（在 SC 回调线程上完成拷贝与转换）。
#[derive(Clone)]
struct RawFrame {
    sequence: u64,
    width: u32,
    height: u32,
    data: Bytes,
    timestamp_us: u64,
}

struct OutputForwarder {
    counters: Arc<CallbackCounters>,
    sender: watch::Sender<Option<RawFrame>>,
    origin: Instant,
}

impl SCStreamOutputTrait for OutputForwarder {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        if !matches!(output_type, SCStreamOutputType::Screen) {
            return;
        }
        self.counters.callbacks.fetch_add(1, Ordering::Relaxed);
        let Some(buffer) = sample.pixel_buffer() else {
            return;
        };
        let Ok(guard) = buffer.lock_read_only() else {
            return;
        };
        let start = Instant::now();
        let mut frame = convert_nv12_to_i420(&guard, self.origin);
        self.counters
            .conversion_us
            .fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
        drop(guard);
        if frame.width != 0 {
            frame.sequence = self.counters.published.fetch_add(1, Ordering::Relaxed) + 1;
            self.sender.send_replace(Some(frame));
        }
    }
}

/// 420v 双平面 → 紧缩 I420。格式/布局不符时返回零尺寸帧供调用方丢弃。
fn convert_nv12_to_i420(guard: &CVPixelBufferLockGuard<'_>, origin: Instant) -> RawFrame {
    let timestamp_us = origin.elapsed().as_micros() as u64;
    let empty = || RawFrame {
        sequence: 0,
        width: 0,
        height: 0,
        data: Bytes::new(),
        timestamp_us: 0,
    };
    let (w, h) = (guard.width(), guard.height());
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || guard.plane_count() != 2 {
        return empty();
    }
    let y_bpr = guard.bytes_per_row_of_plane(0);
    let uv_bpr = guard.bytes_per_row_of_plane(1);
    if y_bpr < w
        || uv_bpr < w
        || guard.height_of_plane(0) < h
        || guard.height_of_plane(1) < h / 2
        || w.checked_mul(h).and_then(|n| n.checked_mul(3)).is_none()
        || y_bpr.checked_mul(h).is_none()
        || uv_bpr.checked_mul(h / 2).is_none()
    {
        return empty();
    }
    let (Some(y_base), Some(uv_base)) = (
        guard.base_address_of_plane(0),
        guard.base_address_of_plane(1),
    ) else {
        return empty();
    };

    let mut out = vec![0; w * h * 3 / 2];
    let (y_out, chroma) = out.split_at_mut(w * h);
    let (u_out, v_out) = chroma.split_at_mut(w * h / 4);
    // SAFETY: the read-only guard keeps both planes locked. Their row strides
    // and heights were checked above, so each row slice remains within a plane.
    unsafe {
        for row in 0..h {
            let src = std::slice::from_raw_parts(y_base.add(row * y_bpr), w);
            y_out[row * w..(row + 1) * w].copy_from_slice(src);
        }
        // UV 平面行距按字节计，每行 w 字节 = w/2 个 (U,V) 对；去交织成平面 U、V。
        for row in 0..h / 2 {
            let src = std::slice::from_raw_parts(uv_base.add(row * uv_bpr), w);
            let range = row * (w / 2)..(row + 1) * (w / 2);
            split_uv_row(src, &mut u_out[range.clone()], &mut v_out[range]);
        }
    }

    RawFrame {
        sequence: 0,
        width: w as u32,
        height: h as u32,
        data: Bytes::from(out),
        timestamp_us,
    }
}

/// One pass over interleaved UV, without per-byte buffer growth or row allocations.
fn split_uv_row(source: &[u8], u: &mut [u8], v: &mut [u8]) {
    assert_eq!(source.len(), u.len() * 2);
    assert_eq!(u.len(), v.len());
    for ((pair, u), v) in source.chunks_exact(2).zip(u).zip(v) {
        *u = pair[0];
        *v = pair[1];
    }
}

/// SCStream implements Send in the upstream binding; no local unsafe assertion.
struct StreamHandle(SCStream);

#[derive(Default)]
struct CallbackCounters {
    callbacks: AtomicU64,
    published: AtomicU64,
    conversion_us: AtomicU64,
}

/// Snapshot within the current stream generation; callback totals may advance concurrently.
#[derive(Debug)]
pub struct CaptureDiagnostics {
    pub callbacks: u64,
    pub published: u64,
    pub consumed: u64,
    /// Exact sequence gaps observed by the consumer, excluding any unread final frame.
    pub skipped_before_read: u64,
    pub conversion_total_us: u64,
}

fn matching_display(ids: &[u32], wanted: u32) -> Result<usize, CaptureError> {
    let mut matches = ids.iter().enumerate().filter(|(_, id)| **id == wanted);
    let index = matches.next().ok_or(CaptureError::DisplayChanged)?.0;
    if matches.next().is_some() {
        return Err(CaptureError::DisplayChanged);
    }
    Ok(index)
}

pub struct MacScreenCapturer {
    counters: Arc<CallbackCounters>,
    consumed: u64,
    last_sequence: u64,
    skipped: u64,
    info: CaptureInfo,
    receiver: watch::Receiver<Option<RawFrame>>,
    stream: Option<StreamHandle>,
    display_id: u32,
    max_width: u32,
    origin: Instant,
    failure: watch::Receiver<Option<String>>,
}

impl MacScreenCapturer {
    pub fn diagnostics(&self) -> CaptureDiagnostics {
        CaptureDiagnostics {
            callbacks: self.counters.callbacks.load(Ordering::Relaxed),
            published: self.counters.published.load(Ordering::Relaxed),
            consumed: self.consumed,
            skipped_before_read: self.skipped,
            conversion_total_us: self.counters.conversion_us.load(Ordering::Relaxed),
        }
    }
    /// 显式选择本次枚举的显示器索引，按 max_width 限制宽度并保持纵横比。
    pub fn new(
        display_index: usize,
        max_width: u32,
        framerate: u32,
        origin: Instant,
    ) -> Result<Self, CaptureError> {
        if max_width < 2 || framerate == 0 || framerate > 120 {
            return Err(CaptureError::Unavailable(
                "invalid capture size or framerate".into(),
            ));
        }
        let permission_hint = "；若刚安装请授予 Screen Recording 权限并重启进程";
        let content = SCShareableContent::get().map_err(|e| {
            CaptureError::Unavailable(format!(
                "SCShareableContent::get 失败: {e:?}{permission_hint}"
            ))
        })?;
        let display = content
            .displays()
            .into_iter()
            .nth(display_index)
            .ok_or_else(|| {
                // 无 Screen Recording 权限时 get() 不报错，而是返回空列表——这是主要症状。
                CaptureError::Unavailable(
                    "未找到可用显示器（最常见原因：未授予 Screen Recording 权限，".to_string()
                        + "在 系统设置 → 隐私与安全性 → 屏幕录制 中允许本应用后重启）",
                )
            })?;
        Self::for_display(display, max_width, framerate, origin)
    }

    fn for_display(
        display: screencapturekit::shareable_content::SCDisplay,
        max_width: u32,
        framerate: u32,
        origin: Instant,
    ) -> Result<Self, CaptureError> {
        let display_id = display.display_id();
        let (dw, dh) = (display.width(), display.height());
        let scale = (max_width as f32 / dw.max(1) as f32).min(1.0);
        let w = ((dw as f32 * scale) as u32) & !1;
        let h = ((dh as f32 * scale) as u32) & !1;

        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()
            .map_err(|e| CaptureError::Unavailable(format!("SCContentFilter: {e:?}")))?;
        if w == 0 || h == 0 {
            return Err(CaptureError::Unavailable(
                "invalid display dimensions".into(),
            ));
        }
        let config = SCStreamConfiguration::new()
            .with_width(w)
            .with_height(h)
            .with_pixel_format(SckPixelFormat::YCbCr_420v)
            .with_shows_cursor(true)
            .with_minimum_frame_interval(&screencapturekit::cm::CMTime::new(1, framerate as i32));
        let (failure_tx, failure) = watch::channel(None);
        let delegate = screencapturekit::stream::delegate_trait::ErrorHandler::new(move |error| {
            failure_tx.send_replace(Some(format!("ScreenCaptureKit stopped: {error}")));
        });
        let mut stream = SCStream::new_with_delegate(&filter, &config, delegate)
            .map_err(|e| CaptureError::Other(e.into()))?;

        let (tx, rx) = watch::channel(None);
        let counters = Arc::new(CallbackCounters::default());
        stream
            .add_output_handler(
                OutputForwarder {
                    sender: tx,
                    origin,
                    counters: Arc::clone(&counters),
                },
                SCStreamOutputType::Screen,
            )
            .map_err(|e| CaptureError::Unavailable(format!("add_output_handler: {e:?}")))?;
        stream
            .start_capture()
            .map_err(|e| CaptureError::Unavailable(format!("start_capture: {e:?}")))?;

        Ok(Self {
            counters,
            consumed: 0,
            last_sequence: 0,
            skipped: 0,
            info: CaptureInfo {
                width: w,
                height: h,
                format: PixelFormat::I420,
                max_framerate: framerate,
            },
            receiver: rx,
            stream: Some(StreamHandle(stream)),
            display_id,
            max_width,
            origin,
            failure,
        })
    }
}

#[allow(async_fn_in_trait)]
impl Capturer for MacScreenCapturer {
    fn info(&self) -> &CaptureInfo {
        &self.info
    }

    fn recover(&mut self) -> Result<(), CaptureError> {
        // Release the invalid stream before creating another; never select by the old index.
        if let Some(handle) = self.stream.take() {
            handle
                .0
                .stop_capture()
                .map_err(|e| CaptureError::Unavailable(format!("stop_capture: {e}")))?;
        }
        let content = SCShareableContent::get()
            .map_err(|e| CaptureError::Unavailable(format!("display enumeration: {e}")))?;
        let displays = content.displays();
        let ids: Vec<_> = displays
            .iter()
            .map(|display| display.display_id())
            .collect();
        let index = matching_display(&ids, self.display_id)?;
        let display = displays
            .into_iter()
            .nth(index)
            .ok_or(CaptureError::DisplayChanged)?;
        *self = Self::for_display(
            display,
            self.max_width,
            self.info.max_framerate,
            self.origin,
        )?;
        Ok(())
    }

    async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        if let Some(error) = self.failure.borrow().clone() {
            return Err(CaptureError::Unavailable(error));
        }
        match tokio::time::timeout(Duration::from_millis(100), self.receiver.changed()).await {
            Err(_) => return Err(CaptureError::Timeout),
            Ok(Err(_)) => return Err(CaptureError::Unavailable("capture stream closed".into())),
            Ok(Ok(())) => {}
        }
        let latest = self
            .receiver
            .borrow_and_update()
            .clone()
            .ok_or(CaptureError::Timeout)?;
        if latest.width != self.info.width || latest.height != self.info.height {
            return Err(CaptureError::DisplayChanged);
        }
        self.skipped += latest.sequence.saturating_sub(self.last_sequence + 1);
        self.last_sequence = latest.sequence;
        self.consumed += 1;
        Ok(CapturedFrame {
            width: latest.width,
            height: latest.height,
            format: PixelFormat::I420,
            data: latest.data,
            damage: Vec::new(),
            timestamp_us: latest.timestamp_us,
        })
    }
}

impl Drop for MacScreenCapturer {
    fn drop(&mut self) {
        if let Some(handle) = self.stream.take() {
            let _ = handle.0.stop_capture();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uv_split_preserves_channels_and_row_boundaries() {
        for width in [2, 4, 30, 1920] {
            let source: Vec<u8> = (0..width).map(|n| (n % 251) as u8).collect();
            let mut u = vec![255; width / 2];
            let mut v = vec![255; width / 2];
            split_uv_row(&source, &mut u, &mut v);
            assert_eq!(u, source.iter().step_by(2).copied().collect::<Vec<_>>());
            assert_eq!(
                v,
                source
                    .iter()
                    .skip(1)
                    .step_by(2)
                    .copied()
                    .collect::<Vec<_>>()
            );
        }
    }
    fn capture() -> (watch::Sender<Option<RawFrame>>, MacScreenCapturer) {
        let (tx, receiver) = watch::channel(None);
        (
            tx,
            MacScreenCapturer {
                counters: Arc::new(CallbackCounters::default()),
                consumed: 0,
                last_sequence: 0,
                skipped: 0,
                info: CaptureInfo {
                    width: 2,
                    height: 2,
                    format: PixelFormat::I420,
                    max_framerate: 30,
                },
                receiver,
                stream: None,
                display_id: 7,
                max_width: 1920,
                origin: Instant::now(),
                failure: watch::channel(None).1,
            },
        )
    }
    fn frame(timestamp_us: u64) -> RawFrame {
        RawFrame {
            sequence: timestamp_us / 100,
            width: 2,
            height: 2,
            data: Bytes::from_static(&[0; 6]),
            timestamp_us,
        }
    }
    #[test]
    fn recovery_matches_identity_not_enumeration_order() {
        assert_eq!(matching_display(&[8, 7], 7).unwrap(), 1);
        assert_eq!(matching_display(&[7, 8], 7).unwrap(), 0);
        assert!(matching_display(&[8], 7).is_err());
        assert!(matching_display(&[7, 7], 7).is_err());
        assert!(matching_display(&[], 7).is_err());
    }
    #[tokio::test]
    async fn native_failure_is_reported_before_stale_frame() {
        let (tx, mut capture) = capture();
        tx.send_replace(Some(frame(100)));
        let (failure_tx, failure) = watch::channel(Some("permission revoked".into()));
        capture.failure = failure;
        assert!(matches!(
            capture.next_frame().await,
            Err(CaptureError::Unavailable(_))
        ));
        drop(failure_tx);
    }
    #[tokio::test]
    async fn latest_frame_replaces_stale_frames_and_keeps_clock() {
        let (tx, mut capture) = capture();
        tx.send_replace(Some(frame(100)));
        tx.send_replace(Some(frame(200)));
        assert_eq!(capture.next_frame().await.unwrap().timestamp_us, 200);
        assert_eq!(capture.diagnostics().consumed, 1);
        assert_eq!(capture.diagnostics().skipped_before_read, 1);
        assert!(matches!(
            capture.next_frame().await,
            Err(CaptureError::Timeout)
        ));
        drop(tx);
        assert!(matches!(
            capture.next_frame().await,
            Err(CaptureError::Unavailable(_))
        ));
    }
    #[tokio::test]
    async fn detects_dimension_changes() {
        let (tx, mut capture) = capture();
        let mut changed = frame(0);
        changed.width = 4;
        tx.send_replace(Some(changed));
        assert!(matches!(
            capture.next_frame().await,
            Err(CaptureError::DisplayChanged)
        ));
    }
}
