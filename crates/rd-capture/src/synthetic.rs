//! 合成测试图案 Capturer（P1）：SMPTE 75% 彩条 + 按帧移动的白色块。
//!
//! 直接在 YUV（BT.601 limited range）空间生成，无 RGB 转换开销，保证 debug
//! 构建也能稳定出帧；图案确定可复现，作为延迟测量的标准素材（PLAN.md P1 验收要求）。

use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};

use crate::{CaptureError, CaptureInfo, CapturedFrame, Capturer, PixelFormat};

/// SMPTE 75% 彩条的 (Y, U, V)，BT.601 limited range。
const BARS: [(u8, u8, u8); 7] = [
    (180, 128, 128), // 白灰
    (162, 44, 142),  // 黄
    (131, 84, 43),   // 青
    (112, 72, 58),   // 绿
    (84, 184, 198),  // 品
    (65, 147, 212),  // 红
    (35, 128, 128),  // 蓝
];

pub struct SyntheticCapturer {
    info: CaptureInfo,
    interval: Duration,
    frame_index: u64,
    next_frame_at: Instant,
    clock_origin: Instant,
}

impl SyntheticCapturer {
    pub fn new(width: u32, height: u32, framerate: u32) -> Self {
        Self::with_clock_origin(width, height, framerate, Instant::now())
    }

    /// Use the same monotonic origin as the pipeline and renderer.
    pub fn with_clock_origin(
        width: u32,
        height: u32,
        framerate: u32,
        clock_origin: Instant,
    ) -> Self {
        Self {
            info: CaptureInfo {
                width,
                height,
                format: PixelFormat::I420,
                max_framerate: framerate,
            },
            interval: Duration::from_secs_f64(1.0 / framerate as f64),
            frame_index: 0,
            next_frame_at: Instant::now(),
            clock_origin,
        }
    }
}

#[allow(async_fn_in_trait)]
impl Capturer for SyntheticCapturer {
    fn info(&self) -> &CaptureInfo {
        &self.info
    }

    async fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        let now = Instant::now();
        if now < self.next_frame_at {
            tokio::time::sleep(self.next_frame_at - now).await;
        }
        // Rebase missed deadlines: never burst overdue frames after a stall.
        self.next_frame_at = next_deadline(self.next_frame_at, Instant::now(), self.interval);
        self.frame_index += 1;
        Ok(self.render(self.frame_index - 1))
    }
}

impl SyntheticCapturer {
    fn render(&self, frame: u64) -> CapturedFrame {
        let timestamp_us = self.clock_origin.elapsed().as_micros() as u64;
        let (w, h) = (self.info.width as usize, self.info.height as usize);
        let (cw, ch) = (w / 2, h / 2);
        let bar_width = w / BARS.len();

        // 按行生成：竖条带的每行相同，先造模板行再整体拷贝，
        // debug 构建下也远快于实时（逐像素写 720p 会吃掉 ~50ms/帧）。
        let mut base_row = vec![0u8; w];
        for x in 0..w {
            base_row[x] = BARS[(x / bar_width).min(BARS.len() - 1)].0;
        }
        let block_w = (w / 16).max(1);
        let block_h = (h / 8).max(1);
        let travel = (frame % (w as u64)) as usize;
        let bx = if (frame / w as u64).is_multiple_of(2) {
            travel
        } else {
            w - travel
        };
        let bx = bx.min(w - block_w);
        let by = (h - block_h) / 2;
        let mut block_row = base_row.clone();
        for slot in &mut block_row[bx..(bx + block_w).min(w)] {
            *slot = 235;
        }

        let mut buf = BytesMut::with_capacity(w * h + 2 * cw * ch);
        for y in 0..h {
            let row = if y >= by && y < by + block_h {
                &block_row
            } else {
                &base_row
            };
            buf.extend_from_slice(row);
        }
        for plane in 0..=1 {
            let row: Vec<u8> = (0..cw)
                .map(|x| {
                    let bar = ((x * 2) / bar_width).min(BARS.len() - 1);
                    if plane == 0 {
                        BARS[bar].1
                    } else {
                        BARS[bar].2
                    }
                })
                .collect();
            for _y in 0..ch {
                buf.extend_from_slice(&row);
            }
        }

        CapturedFrame {
            width: w as u32,
            height: h as u32,
            format: PixelFormat::I420,
            data: Bytes::from(buf),
            damage: Vec::new(), // 整帧刷新
            timestamp_us,
        }
    }
}

fn next_deadline(previous: Instant, now: Instant, interval: Duration) -> Instant {
    let scheduled = previous + interval;
    if scheduled <= now { now + interval } else { scheduled }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missed_deadlines_do_not_burst() {
        let start = Instant::now();
        let interval = Duration::from_millis(33);
        assert_eq!(next_deadline(start, start, interval), start + interval);
        let resumed = start + Duration::from_secs(10);
        assert_eq!(next_deadline(start, resumed, interval), resumed + interval);
    }

    fn black_box_size(w: u32, h: u32) -> usize {
        (w * h * 3 / 2) as usize
    }

    #[tokio::test]
    async fn timestamps_use_shared_origin() {
        let origin = Instant::now() - Duration::from_secs(2);
        let mut capturer = SyntheticCapturer::with_clock_origin(320, 180, 30, origin);
        let before = origin.elapsed().as_micros() as u64;
        let frame = capturer.next_frame().await.unwrap();
        let after = origin.elapsed().as_micros() as u64;
        assert!((before..=after).contains(&frame.timestamp_us));
        assert!(frame.timestamp_us >= 2_000_000);
    }

    #[tokio::test]
    async fn produces_paced_deterministic_frames() {
        let mut c = SyntheticCapturer::new(320, 180, 30);
        let f0 = c.next_frame().await.unwrap();
        assert_eq!(f0.format, PixelFormat::I420);
        assert_eq!(f0.data.len(), black_box_size(320, 180));
        // 帧内容确定性：同参数下首帧应逐字节一致。
        let mut c2 = SyntheticCapturer::new(320, 180, 30);
        let f0b = c2.next_frame().await.unwrap();
        assert_eq!(f0.data, f0b.data);
        // 第 2 帧因移动白块与首帧不同。
        let f1 = c.next_frame().await.unwrap();
        assert_ne!(f0.data, f1.data);
    }
}
