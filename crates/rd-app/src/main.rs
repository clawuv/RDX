//! RDX 桌面应用主入口（egui/wgpu）。
//!
//! 当前为 P1 阶段的最小壳：连接管理、设备列表等 UI 随 P4 会话管理成型。
//! 视频回环演示见 `cargo run -p rd-app --bin rd-loop`。

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RDX")
            .with_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RDX",
        options,
        Box::new(|_cc| Ok(Box::new(RdxApp::default()))),
    )
}

#[derive(Default)]
struct RdxApp {
    peer_id: String,
}

impl eframe::App for RdxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.add_space(24.0);
            ui.heading("RDX");
            ui.label(format!("v{} · P1 开发中", env!("CARGO_PKG_VERSION")));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label("伙伴 ID:");
                ui.text_edit_singleline(&mut self.peer_id);
                if ui.button("连接").clicked() && !self.peer_id.is_empty() {
                    // P2：经 Transport 建立会话。
                }
            });
            ui.add_space(16.0);
            ui.weak("视频回环演示：cargo run -p rd-app --bin rd-loop");
        });
    }
}
