//! 受控动态测试图案：绘制与读取共用同一套设备像素几何。
//!
//! 图案内容完全由帧序号决定；右上角 8 个方块是帧号低 8 位的“指纹”。
//! `present_probe` 从 ScreenCaptureKit 捕获帧的 Y 平面读回指纹，与
//! pattern_window 的逐帧 trace 对齐，测得“图案帧发布 → 被显示器内容
//! 捕获观察到”的时延。读取要求图案窗口全屏且捕获未缩放。

use egui::{Painter, Rect};

/// 指纹方块边长（设备像素）。
pub const SQUARE_PX: f32 = 24.0;
/// 相邻指纹方块间距（设备像素）。
pub const GAP_PX: f32 = 4.0;
/// 指纹区域距窗口右、上边缘（设备像素）。
pub const MARGIN_PX: f32 = 24.0;
/// 指纹位数，等于帧号低 8 位。
pub const FINGERPRINT_BITS: u32 = 8;

/// 指纹方块左边到窗口右缘的距离（设备像素）。
pub fn bit_offset_px(bit: u32) -> f32 {
    (FINGERPRINT_BITS - bit) as f32 * (SQUARE_PX + GAP_PX)
}

/// 指纹方块在窗口点坐标下的矩形；bit i 显示 (frame >> i) & 1。
pub fn bit_rect(rect: Rect, ppp: f32, bit: u32) -> Rect {
    let x = rect.right() - bit_offset_px(bit) / ppp;
    let size = SQUARE_PX / ppp;
    Rect::from_min_size(
        egui::pos2(x, rect.top() + MARGIN_PX / ppp),
        egui::vec2(size, size),
    )
}

/// 采样指纹方块中心的 Y 行（设备像素）。
pub fn sample_row_px() -> usize {
    (MARGIN_PX + SQUARE_PX / 2.0) as usize
}

/// 采样指纹方块中心的 Y 列（设备像素）；width 为捕获图像宽。
/// 方块越出图像边界（含 5×5 采样窗口）时返回 None。
pub fn sample_col_px(width: usize, bit: u32) -> Option<usize> {
    let x = width as f32 - bit_offset_px(bit) + SQUARE_PX / 2.0;
    if x < 3.0 || x + 3.0 > width as f32 {
        None
    } else {
        Some(x as usize)
    }
}

/// 从捕获帧 Y 平面读取指纹；返回帧号低 8 位（全屏图案，行偏移 0）。
pub fn read_fingerprint(y_plane: &[u8], width: usize) -> Option<u8> {
    read_fingerprint_with_offset(y_plane, width, 0)
}

/// 带行偏移读取指纹：无边框条带窗口模式下，窗口内容起点在菜单栏下方
/// （实测约 30px），需把指纹采样行整体下移。偏移来自实际窗口位置。
/// 采样各方块中心 5×5 平均亮度，阈值 128。宽度或数据不足返回 None。
pub fn read_fingerprint_with_offset(
    y_plane: &[u8],
    width: usize,
    y_offset_px: usize,
) -> Option<u8> {
    let row = sample_row_px() + y_offset_px;
    if width == 0 || y_plane.len() <= (row + 3) * width {
        return None;
    }
    let mut value = 0u8;
    for bit in 0..FINGERPRINT_BITS {
        let cx = sample_col_px(width, bit)?;
        let mut sum = 0u32;
        for dy in -2i32..=2 {
            let base = ((row as i32 + dy) as usize) * width;
            for dx in -2i32..=2 {
                sum += y_plane[base + ((cx as i32 + dx) as usize)] as u32;
            }
        }
        if sum / 25 > 128 {
            value |= 1 << bit;
        }
    }
    Some(value)
}

/// 绘制受控动态图案：逐帧整体翻转的棋盘底、匀速白条、指纹角标与大号帧号。
/// `ppp` 为窗口 pixels_per_point，指纹方块按设备像素定尺寸。
pub fn draw(painter: &Painter, rect: Rect, ppp: f32, frame: u64, fps: u32) {
    let cell: f32 = 64.0;
    let (dark, light) = (
        egui::Color32::from_rgb(28, 28, 32),
        egui::Color32::from_rgb(44, 44, 50),
    );
    let parity = (frame % 2) as u32;
    let mut y = rect.top();
    let mut gy = 0u32;
    while y < rect.bottom() {
        let mut x = rect.left();
        let mut gx = 0u32;
        while x < rect.right() {
            let on = (gx + gy + parity).is_multiple_of(2);
            painter.rect_filled(
                Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(cell.min(rect.right() - x), cell.min(rect.bottom() - y)),
                ),
                0.0,
                if on { light } else { dark },
            );
            x += cell;
            gx += 1;
        }
        y += cell;
        gy += 1;
    }
    // 匀速白条：位置只依赖帧序号，一屏一周期。
    let bar_w = (rect.width() / 32.0).max(8.0);
    let travel = (frame % (rect.width() as u64 + 1)) as f32;
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(rect.left() + travel, rect.top()),
            egui::vec2(bar_w, rect.height()),
        ),
        0.0,
        egui::Color32::WHITE,
    );
    for bit in 0..FINGERPRINT_BITS {
        let on = (frame >> bit) & 1 == 1;
        painter.rect_filled(
            bit_rect(rect, ppp, bit),
            2.0,
            if on {
                egui::Color32::WHITE
            } else {
                egui::Color32::BLACK
            },
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("#{} @{}fps", frame, fps),
        egui::FontId::monospace(96.0),
        egui::Color32::from_rgb(230, 230, 235),
    );
}

/// 解析 pattern_window 的 trace 文件，返回按时间升序的 (帧号, 发布墙钟μs)。
pub fn parse_trace(text: &str) -> Vec<(u64, u64)> {
    let mut trace: Vec<(u64, u64)> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            match (parts.next()?, parts.next()?, parts.next()?) {
                ("pattern-trace", frame, unix) => Some((frame.parse().ok()?, unix.parse().ok()?)),
                _ => None,
            }
        })
        .collect();
    trace.sort_unstable_by_key(|(_, unix)| *unix);
    trace
}

/// 对齐转场观测与图案 trace：为每个观测找最近的同指纹且不晚于它的发布时刻，
/// 返回成功配对的时延（毫秒）。超过 `max_delay_us` 或无候选的观测被丢弃。
/// 观测应已按“指纹变化”筛选（新内容到达），否则冻结内容会配到错误的轮次。
pub fn match_delays_ms(
    trace: &[(u64, u64)],
    transitions: &[(u64, u8)],
    max_delay_us: u64,
) -> Vec<f32> {
    let mut delays = Vec::new();
    for (t_obs, bits) in transitions {
        let issue = trace
            .iter()
            .rev()
            .find(|(frame, unix)| *unix <= *t_obs && (*frame & 0xff) == *bits as u64)
            .map(|(_, unix)| *unix);
        if let Some(unix) = issue {
            let delay_us = t_obs.saturating_sub(unix);
            if delay_us <= max_delay_us {
                delays.push(delay_us as f32 / 1000.0);
            }
        }
    }
    delays
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_y(width: usize, height: usize, frame_low8: u8) -> Vec<u8> {
        synthetic_y_offset(width, height, frame_low8, 0)
    }

    fn synthetic_y_offset(width: usize, height: usize, frame_low8: u8, offset: usize) -> Vec<u8> {
        let mut y = vec![40u8; width * height]; // 棋盘底灰度低于阈值。
        for bit in 0..FINGERPRINT_BITS {
            let bright = (frame_low8 >> bit) & 1 == 1;
            let cx = sample_col_px(width, bit).expect("square inside plane");
            let row = sample_row_px() + offset;
            for dy in 0..SQUARE_PX as usize {
                for dx in 0..SQUARE_PX as usize {
                    let x = cx - SQUARE_PX as usize / 2 + dx;
                    let r = row - SQUARE_PX as usize / 2 + dy;
                    y[r * width + x] = if bright { 235 } else { 16 };
                }
            }
        }
        y
    }

    #[test]
    fn fingerprint_round_trips_across_all_byte_values() {
        for frame_low8 in [0u8, 1, 0x55, 0xAA, 128, 200, 255] {
            let width = 3840;
            let y = synthetic_y(width, 96, frame_low8);
            assert_eq!(read_fingerprint(&y, width), Some(frame_low8));
            // 行偏移模式下采样行同步下移，同一帧数据仍可读回。
            let y = synthetic_y_offset(width, 96, frame_low8, 30);
            assert_eq!(
                read_fingerprint_with_offset(&y, width, 30),
                Some(frame_low8)
            );
            // 偏移不一致时读到棋盘底（恒 0）而不是原指纹。
            assert_eq!(read_fingerprint_with_offset(&y, width, 0), Some(0));
        }
    }

    #[test]
    fn rejects_narrow_or_short_planes() {
        let y = vec![200u8; 64 * 64];
        assert_eq!(read_fingerprint(&y, 64), None); // 放不下 8 个方块。
        assert_eq!(read_fingerprint(&[], 3840), None);
    }

    #[test]
    fn bit_rects_stay_inside_the_window() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        for bit in 0..FINGERPRINT_BITS {
            let r = bit_rect(rect, 1.0, bit);
            assert!(r.left() >= rect.left());
            assert!(r.right() <= rect.right());
            assert!(r.top() >= rect.top());
        }
    }

    #[test]
    fn delay_matching_picks_latest_candidate_and_drops_stale() {
        use super::match_delays_ms;
        // 帧号 1、257 指纹相同（低 8 位），相隔一个轮次。
        let trace = vec![(1u64, 1_000u64), (2, 34_000), (3, 67_000), (257, 8_600_000)];
        let ms = |us: u64, bits: u8| vec![(us, bits)];
        // 观测到指纹 2：应配最近的第 2 帧，时延 11ms。
        let out = match_delays_ms(&trace, &ms(45_000, 2), 2_000_000);
        assert_eq!(out, vec![11.0]);
        // 观测早于全部候选：无配对。
        assert!(match_delays_ms(&trace, &ms(500, 2), 2_000_000).is_empty());
        // 指纹 3 的唯一候选在 67ms，9s 的观测超时丢弃。
        assert!(match_delays_ms(&trace, &ms(9_000_000, 3), 2_000_000).is_empty());
        // 指纹 1 出现新轮次（257）：配最近的 257 而不是第 1 帧，时延 100ms。
        let out = match_delays_ms(&trace, &ms(8_700_000, 1), 2_000_000);
        assert_eq!(out, vec![100.0]);
    }
}
