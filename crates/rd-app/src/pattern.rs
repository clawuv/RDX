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

/// 从捕获帧 Y 平面读取指纹；返回帧号低 8 位。
/// 采样各方块中心 5×5 平均亮度，阈值 128。宽度或数据不足返回 None。
pub fn read_fingerprint(y_plane: &[u8], width: usize) -> Option<u8> {
    let row = sample_row_px();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_y(width: usize, height: usize, frame_low8: u8) -> Vec<u8> {
        let mut y = vec![40u8; width * height]; // 棋盘底灰度低于阈值。
        for bit in 0..FINGERPRINT_BITS {
            let bright = (frame_low8 >> bit) & 1 == 1;
            let cx = sample_col_px(width, bit).expect("square inside plane");
            let row = sample_row_px();
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
}
