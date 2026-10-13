//! RDX 桌面应用（ADR-0001：egui/wgpu，唯一二进制）。
//!
//! P0 只是最小骨架；P1 在这里接入视频渲染 pass（`egui_wgpu` CallbackTrait，
//! YUV→RGB 在 shader 中完成）。

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RDX")
            .with_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RDX",
        options,
        Box::new(|_cc| Ok(Box::new(RdxApp))),
    )
}

struct RdxApp;

impl eframe::App for RdxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("RDX");
        ui.label("P0 骨架已就绪 — 下一站: P1 单机视频垂直切片");
    }
}
