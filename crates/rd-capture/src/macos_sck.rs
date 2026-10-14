//! macOS ScreenCaptureKit 捕获后端（P1）。
//!
//! 采用 420v（NV12 双平面）输出，捕获边界处一次去交织转成 I420，
//! 下游（openh264 编码、GPU 渲染）保持单一平面格式。
//! 需要 Screen Recording 权限（TCC）；超时不单独视为权限被拒绝。

use std::time::{Duration, Instant};
use tokio::sync::watch;

use bytes::{Bytes, BytesMut};
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
    width: u32,
    height: u32,
    data: Bytes,
    timestamp_us: u64,
}

struct OutputForwarder {
    sender: watch::Sender<Option<RawFrame>>,
    origin: Instant,
}

impl SCStreamOutputTrait for OutputForwarder {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        if !matches!(output_type, SCStreamOutputType::Screen) {
            return;
        }
        let Some(buffer) = sample.pixel_buffer() else {
            return;
        };
        let Ok(guard) = buffer.lock_read_only() else {
            return;
        };
        let frame = convert_nv12_to_i420(&guard, self.origin);
        drop(guard);
        if frame.width != 0 {
            self.sender.send_replace(Some(frame));
        }
    }
}

/// 420v 双平面 → 紧缩 I420。格式/布局不符时返回零尺寸帧供调用方丢弃。
fn convert_nv12_to_i420(guard: &CVPixelBufferLockGuard<'_>, origin: Instant) -> RawFrame {
    let timestamp_us = origin.elapsed().as_micros() as u64;
    let empty = || RawFrame {
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

    let mut out = BytesMut::with_capacity(w * h * 3 / 2);
    // SAFETY: the read-only guard keeps both planes locked. Their row strides
    // and heights were checked above, so each row slice remains within a plane.
    unsafe {
        for row in 0..h {
            let src = std::slice::from_raw_parts(y_base.add(row * y_bpr), w);
            out.extend_from_slice(src);
        }
        // UV 平面行距按字节计，每行 w 字节 = w/2 个 (U,V) 对；去交织成平面 U、V。
        let half_h = h / 2;
        let uv_rows: Vec<&[u8]> = (0..half_h)
            .map(|row| std::slice::from_raw_parts(uv_base.add(row * uv_bpr), w))
            .collect();
        for row in &uv_rows {
            out.extend(row.iter().step_by(2).copied());
        }
        for row in &uv_rows {
            out.extend(row.iter().skip(1).step_by(2).copied());
        }
    }

    RawFrame {
        width: w as u32,
        height: h as u32,
        data: Bytes::from(out),
        timestamp_us,
    }
}

/// SCStream implements Send in the upstream binding; no local unsafe assertion.
struct StreamHandle(SCStream);

pub struct MacScreenCapturer {
    info: CaptureInfo,
    receiver: watch::Receiver<Option<RawFrame>>,
    stream: Option<StreamHandle>,
}

impl MacScreenCapturer {
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
        let mut stream =
            SCStream::new(&filter, &config).map_err(|e| CaptureError::Other(e.into()))?;

        let (tx, rx) = watch::channel(None);
        stream
            .add_output_handler(
                OutputForwarder { sender: tx, origin },
                SCStreamOutputType::Screen,
            )
            .map_err(|e| CaptureError::Unavailable(format!("add_output_handler: {e:?}")))?;
        stream
            .start_capture()
            .map_err(|e| CaptureError::Unavailable(format!("start_capture: {e:?}")))?;

        Ok(Self {
            info: CaptureInfo {
                width: w,
                height: h,
                format: PixelFormat::I420,
                max_framerate: framerate,
            },
            receiver: rx,
            stream: Some(StreamHandle(stream)),
        })
    }
}

#[allow(async_fn_in_trait)]
impl Capturer for MacScreenCapturer {
    fn info(&self) -> &CaptureInfo {
        &self.info
    }

    async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
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
    fn capture() -> (watch::Sender<Option<RawFrame>>, MacScreenCapturer) {
        let (tx, receiver) = watch::channel(None);
        (
            tx,
            MacScreenCapturer {
                info: CaptureInfo {
                    width: 2,
                    height: 2,
                    format: PixelFormat::I420,
                    max_framerate: 30,
                },
                receiver,
                stream: None,
            },
        )
    }
    fn frame(timestamp_us: u64) -> RawFrame {
        RawFrame {
            width: 2,
            height: 2,
            data: Bytes::from_static(&[0; 6]),
            timestamp_us,
        }
    }
    #[tokio::test]
    async fn latest_frame_replaces_stale_frames_and_keeps_clock() {
        let (tx, mut capture) = capture();
        tx.send_replace(Some(frame(100)));
        tx.send_replace(Some(frame(200)));
        assert_eq!(capture.next_frame().await.unwrap().timestamp_us, 200);
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
