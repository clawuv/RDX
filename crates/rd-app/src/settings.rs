use crate::{
    theme::Theme,
    widgets::{bold, display, font},
    PAGE_PADDING_X,
};
use eframe::egui::{self, Align2, Pos2, Rect, Vec2};

pub struct Settings {
    scale: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}
impl Settings {
    pub fn show(&mut self, ui: &mut egui::Ui, content: Rect, theme: &Theme, sidebar: &mut bool) {
        let x = content.left() + PAGE_PADDING_X;
        let y = content.top() + if *sidebar { 22.0 } else { 48.0 };
        ui.painter().text(
            Pos2::new(x, y),
            Align2::LEFT_TOP,
            "设置",
            display(22.0),
            theme.text,
        );
        let body = Rect::from_min_max(
            Pos2::new(x, y + 44.0),
            content.max - Vec2::new(PAGE_PADDING_X, 20.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("settings-scroll")
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section(ui, "外观", theme);
                    ui.horizontal(|ui| {
                        ui.label("主题");
                        ui.add_space(24.0);
                        ui.label(egui::RichText::new("冷白灰").color(theme.text_dim));
                    });
                    ui.add_space(8.0);
                    ui.checkbox(sidebar, "显示侧栏");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("界面缩放");
                        if ui
                            .add(
                                egui::Slider::new(&mut self.scale, 0.8..=1.25)
                                    .step_by(0.05)
                                    .suffix("×"),
                            )
                            .changed()
                        {
                            ui.ctx().set_zoom_factor(self.scale);
                        }
                    });
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("外观调整即时生效，仅在本次运行中保留。")
                            .font(font(11.5))
                            .color(theme.text_faint),
                    );
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(14.0);
                    section(ui, "远程连接", theme);
                    ui.label(
                        egui::RichText::new("远程服务尚未接入，以下配置暂不可用。")
                            .color(theme.text_dim),
                    );
                    ui.add_space(8.0);
                    ui.add_enabled_ui(false, |ui| {
                        let mut unattended = false;
                        let mut clipboard = false;
                        let mut files = false;
                        ui.checkbox(&mut unattended, "允许无人值守访问");
                        ui.add_space(8.0);
                        ui.checkbox(&mut clipboard, "同步剪贴板");
                        ui.add_space(8.0);
                        ui.checkbox(&mut files, "允许文件传输");
                    });
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(14.0);
                    section(ui, "关于", theme);
                    ui.label(format!("RDX  {}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("远程桌面与协助 · 界面预览版").color(theme.text_dim),
                    );
                });
        });
    }
}
fn section(ui: &mut egui::Ui, title: &str, theme: &Theme) {
    ui.label(
        egui::RichText::new(title)
            .font(bold(18.0))
            .color(theme.text),
    );
    ui.add_space(10.0);
}
