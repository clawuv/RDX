//! macOS ScreenCaptureKit 捕获后端（P1）。
//!
//! 采用 420v（NV12 双平面）输出，捕获边界处一次去交织转成 I420，
//! 下游（openh264 编码、GPU 渲染）保持单一平面格式。
//! 需要 Screen Recording 权限（TCC）；缺失时表现为 `next_frame` 超时错误。

use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

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
struct RawFrame {
    width: u32,
    height: u32,
    data: Bytes,
    timestamp_us: u64,
}

struct OutputForwarder {
    sender: SyncSender<RawFrame>,
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
        // 通道满即丢帧：渲染端永远拿最新画面，不在捕获侧排队。
        match self.sender.try_send(frame) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

/// 420v 双平面 → 紧缩 I420。格式/布局不符时返回零尺寸帧供调用方丢弃。
fn convert_nv12_to_i420(guard: &CVPixelBufferLockGuard<'_>, origin: Instant) -> RawFrame {
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
    let (Some(y_base), Some(uv_base)) = (
        guard.base_address_of_plane(0),
        guard.base_address_of_plane(1),
    ) else {
        return empty();
    };

    let mut out = BytesMut::with_capacity(w * h * 3 / 2);
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
        timestamp_us: origin.elapsed().as_micros() as u64,
    }
}

/// SCStream 持有裸指针而非 Send。本包装的 Send 依据：捕获器在管道专用线程上
/// 构造并仅从该线程调用（`&mut self`）；跨线程仅有 SC 自己的回调队列，
/// 不产生 Rust 侧并发访问。
struct StreamHandle(SCStream);
unsafe impl Send for StreamHandle {}

pub struct MacScreenCapturer {
    info: CaptureInfo,
    receiver: Receiver<RawFrame>,
    stream: Option<StreamHandle>,
}

impl MacScreenCapturer {
    /// 捕获主显示器，长边缩到 `max_width` 以内（SC 侧完成缩放），保持纵横比。
    pub fn new(max_width: u32) -> Result<Self, CaptureError> {
        let permission_hint = "；若刚安装请授予 Screen Recording 权限并重启进程";
        let content = SCShareableContent::get().map_err(|e| {
            CaptureError::Unavailable(format!(
                "SCShareableContent::get 失败: {e:?}{permission_hint}"
            ))
        })?;
        let display = content.displays().into_iter().next().ok_or_else(|| {
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
        let config = SCStreamConfiguration::new()
            .with_width(w)
            .with_height(h)
            .with_pixel_format(SckPixelFormat::YCbCr_420v)
            .with_shows_cursor(true);
        let mut stream =
            SCStream::new(&filter, &config).map_err(|e| CaptureError::Other(e.into()))?;

        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        stream
            .add_output_handler(
                OutputForwarder {
                    sender: tx,
                    origin: Instant::now(),
                },
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
                max_framerate: 60,
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
        // 阻塞等一帧，再排空到最新：UI/编码慢时丢弃过帧而不是累积延迟。
        let mut latest = self
            .receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| {
                CaptureError::Unavailable(
                    "3 秒未收到帧：检查 Screen Recording 权限（系统设置 → 隐私与安全性）".into(),
                )
            })?;
        while let Ok(newer) = self.receiver.try_recv() {
            latest = newer;
        }
        if latest.width == 0 {
            return Err(CaptureError::Other(anyhow::anyhow!(
                "capture delivered a frame with unexpected layout"
            )));
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
