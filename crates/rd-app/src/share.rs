//! Share the project's release landing page, not a version-specific asset.
use crate::{
    theme::Theme,
    widgets::{self, bold, font, Icon},
};
use eframe::egui::{self, Id, Rect, RichText, Stroke};

pub(crate) const DOWNLOAD_URL: &str = "https://github.com/clawuv/RDX/releases";

pub fn show(ui: &mut egui::Ui, rect: Rect, theme: &Theme) {
    let response =
        widgets::icon_button_bold(ui, rect, Id::new("share-app"), Icon::Share, theme, true)
            .on_hover_text("分享 RDX 下载链接");
    let popup_id = Id::new("share-app-popup");
    let copied_id = popup_id.with("copied");
    if response.clicked() {
        ui.ctx().data_mut(|data| data.remove::<bool>(copied_id));
    }
    egui::Popup::menu(&response)
        .id(popup_id)
        .width(300.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .frame(
            egui::Frame::popup(ui.style())
                .fill(theme.surface)
                .stroke(Stroke::new(1.0, theme.stroke))
                .corner_radius(8.0)
                .inner_margin(12.0),
        )
        .show(|ui| {
            ui.label(RichText::new("分享 RDX").font(bold(15.0)).color(theme.text));
            ui.add_space(6.0);
            ui.label(
                RichText::new("应用下载与版本发布页")
                    .font(font(12.0))
                    .color(theme.text_dim),
            );
            ui.label(
                RichText::new(DOWNLOAD_URL)
                    .font(font(12.0))
                    .color(theme.text),
            );
            ui.label(
                RichText::new("安装包以发布页为准；暂无版本时需等待发布。")
                    .font(font(11.0))
                    .color(theme.text_dim),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let copied = ui
                    .ctx()
                    .data(|data| data.get_temp::<bool>(copied_id).unwrap_or(false));
                let (copy_rect, _) =
                    ui.allocate_exact_size(egui::vec2(128.0, 30.0), egui::Sense::hover());
                if widgets::button(
                    ui,
                    copy_rect,
                    popup_id.with("copy"),
                    if copied {
                        "已复制链接"
                    } else {
                        "复制下载链接"
                    },
                    None,
                    widgets::BtnStyle::Subtle,
                    theme,
                )
                .clicked()
                {
                    ui.ctx().copy_text(DOWNLOAD_URL.into());
                    ui.ctx().data_mut(|data| data.insert_temp(copied_id, true));
                }
                let (open_rect, _) =
                    ui.allocate_exact_size(egui::vec2(128.0, 30.0), egui::Sense::hover());
                if widgets::button(
                    ui,
                    open_rect,
                    popup_id.with("open"),
                    "打开下载页",
                    Some(Icon::LinkOut),
                    widgets::BtnStyle::Subtle,
                    theme,
                )
                .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(DOWNLOAD_URL));
                    ui.close();
                }
            });
        });
}
