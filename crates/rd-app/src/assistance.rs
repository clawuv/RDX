//! Remote assistance UI. Session controls remain placeholders until backend integration.
use crate::{
    theme::Theme,
    widgets::{self, bold, cr, display, font, BtnStyle, Icon},
};
use eframe::egui::{self, Align2, Id, Pos2, Rect, Stroke, Vec2};

pub struct Assistance {
    enabled: bool,
    reveal: bool,
    partner: String,
    password: String,
    files: bool,
    notice: String,
    banner: bool,
}
impl Default for Assistance {
    fn default() -> Self {
        Self {
            enabled: true,
            reveal: false,
            partner: String::new(),
            password: String::new(),
            files: false,
            notice: String::new(),
            banner: true,
        }
    }
}
impl Assistance {
    pub fn show(&mut self, ui: &mut egui::Ui, content: Rect, theme: &Theme, sidebar: bool) {
        let x = content.left() + crate::PAGE_PADDING_X;
        let top = content.top() + crate::page_header_offset(sidebar);
        ui.painter().text(
            Pos2::new(x, top),
            Align2::LEFT_TOP,
            "远程协助",
            display(22.0),
            theme.text,
        );
        let region = Rect::from_min_max(
            Pos2::new(x, top + 44.0),
            content.max - Vec2::new(crate::PAGE_PADDING_X, 16.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(region), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("assistance-scroll")
                .show(ui, |ui| {
                    let (area, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 364.0),
                        egui::Sense::hover(),
                    );
                    let x = area.left();
                    let y = area.top();
                    let w = area.width().min(720.0);
                    let p = ui.painter().clone();
                    p.text(
                        Pos2::new(x, y + 14.0),
                        Align2::LEFT_CENTER,
                        "远程协助本机",
                        font(15.0),
                        theme.text,
                    );
                    widgets::toggle(
                        ui,
                        Rect::from_center_size(
                            Pos2::new(x + 134.0, y + 14.0),
                            Vec2::new(40.0, 24.0),
                        ),
                        Id::new("assistance-enabled"),
                        &mut self.enabled,
                        theme,
                    );
                    widgets::draw_icon(
                        &p,
                        Icon::Shield,
                        Rect::from_center_size(Pos2::new(x + 180.0, y + 14.0), Vec2::splat(21.0)),
                        theme.accent,
                    );
                    p.text(
                        Pos2::new(x, y + 48.0),
                        Align2::LEFT_CENTER,
                        "本机识别码",
                        font(12.5),
                        theme.text_dim,
                    );
                    let second = x + (w * 0.48).max(260.0);
                    p.text(
                        Pos2::new(second, y + 48.0),
                        Align2::LEFT_CENTER,
                        "单次验证码",
                        font(12.5),
                        theme.text_dim,
                    );
                    p.text(
                        Pos2::new(x, y + 80.0),
                        Align2::LEFT_CENTER,
                        if self.enabled {
                            "298 417 652"
                        } else {
                            "协助已关闭"
                        },
                        display(30.0),
                        theme.text,
                    );
                    let copy =
                        Rect::from_center_size(Pos2::new(x + 229.0, y + 80.0), Vec2::splat(28.0));
                    let response =
                        ui.interact(copy, Id::new("copy-local-id"), egui::Sense::click());
                    let ink = if response.hovered() {
                        theme.text
                    } else {
                        theme.text_dim
                    };
                    p.rect_stroke(
                        Rect::from_min_size(copy.min + Vec2::new(10.0, 7.0), Vec2::new(10.0, 11.0)),
                        cr(2.0),
                        Stroke::new(1.2, ink),
                        egui::StrokeKind::Inside,
                    );
                    p.rect_stroke(
                        Rect::from_min_size(copy.min + Vec2::new(6.0, 11.0), Vec2::new(10.0, 11.0)),
                        cr(2.0),
                        Stroke::new(1.2, ink),
                        egui::StrokeKind::Inside,
                    );
                    if response.clicked() && self.enabled {
                        ui.ctx().copy_text("298417652".into());
                        self.notice = "已复制本机识别码".into();
                    }
                    p.text(
                        Pos2::new(second, y + 80.0),
                        Align2::LEFT_CENTER,
                        if !self.enabled {
                            "—"
                        } else if self.reveal {
                            "123456"
                        } else {
                            "• • • • • •"
                        },
                        display(20.0),
                        theme.text_dim,
                    );
                    if widgets::icon_button(
                        ui,
                        Rect::from_center_size(
                            Pos2::new(second + 132.0, y + 80.0),
                            Vec2::splat(28.0),
                        ),
                        Id::new("reveal-code"),
                        Icon::Eye,
                        theme,
                        self.enabled,
                    )
                    .clicked()
                    {
                        self.reveal = !self.reveal;
                    }
                    p.text(
                        Pos2::new(x, y + 110.0),
                        Align2::LEFT_CENTER,
                        "演示识别码与验证码，远程服务尚未接入",
                        font(11.0),
                        theme.text_faint,
                    );
                    p.text(
                        Pos2::new(x, y + 148.0),
                        Align2::LEFT_CENTER,
                        "远程协助他人",
                        font(15.0),
                        theme.text,
                    );
                    let input_y = y + 172.0;
                    let button_w = 112.0;
                    let field_w = w - button_w - 12.0;
                    let fields =
                        Rect::from_min_size(Pos2::new(x, input_y), Vec2::new(field_w, 38.0));
                    let partner_rect =
                        Rect::from_min_size(fields.min, Vec2::new(field_w * 0.56, 38.0));
                    let password_rect = Rect::from_min_max(
                        Pos2::new(partner_rect.right() + 8.0, input_y),
                        fields.max,
                    );
                    widgets::text_input(
                        ui,
                        partner_rect,
                        Id::new("partner-id"),
                        &mut self.partner,
                        "请输入伙伴识别码",
                        theme,
                    );
                    widgets::text_input(
                        ui,
                        password_rect,
                        Id::new("partner-code"),
                        &mut self.password,
                        "验证码（可为空）",
                        theme,
                    );
                    let connect = Rect::from_min_size(
                        Pos2::new(fields.right() + 12.0, input_y),
                        Vec2::new(button_w, 38.0),
                    );
                    if widgets::button(
                        ui,
                        connect,
                        Id::new("connect-partner"),
                        "连接",
                        None,
                        BtnStyle::Primary,
                        theme,
                    )
                    .clicked()
                    {
                        let id: String = self
                            .partner
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        self.notice = if id.len() != 9 || !id.bytes().all(|b| b.is_ascii_digit()) {
                            "请输入 9 位数字的伙伴识别码".into()
                        } else {
                            "连接功能尚未接入，当前为界面预览".into()
                        };
                    }
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(Rect::from_min_size(
                            Pos2::new(x, input_y + 46.0),
                            Vec2::new(w, 28.0),
                        )),
                        |ui| {
                            ui.horizontal(|ui| {
                                ui.radio_value(
                                    &mut self.files,
                                    false,
                                    egui::RichText::new("远程桌面").font(font(12.0)),
                                );
                                ui.add_space(20.0);
                                ui.radio_value(
                                    &mut self.files,
                                    true,
                                    egui::RichText::new("远程文件").font(font(12.0)),
                                );
                            });
                        },
                    );
                    if !self.notice.is_empty() {
                        p.text(
                            Pos2::new(x + w, input_y + 60.0),
                            Align2::RIGHT_CENTER,
                            &self.notice,
                            font(12.0),
                            theme.text_dim,
                        );
                    }
                    p.hline(x..=(x + w), y + 264.0, widgets::divider(theme));
                    if self.banner {
                        let banner =
                            Rect::from_min_size(Pos2::new(x, y + 280.0), Vec2::new(224.0, 76.0));
                        p.rect_filled(banner, cr(8.0), theme.surface_hi);
                        p.text(
                            banner.min + Vec2::new(14.0, 20.0),
                            Align2::LEFT_CENTER,
                            "RDX 远程协助",
                            bold(15.0),
                            theme.text,
                        );
                        p.text(
                            banner.min + Vec2::new(14.0, 40.0),
                            Align2::LEFT_CENTER,
                            "安全连接 · 随时协作",
                            font(12.0),
                            theme.text_dim,
                        );
                        p.text(
                            banner.min + Vec2::new(14.0, 60.0),
                            Align2::LEFT_CENTER,
                            "推广内容占位",
                            font(11.0),
                            theme.text_faint,
                        );
                        if widgets::icon_button(
                            ui,
                            Rect::from_center_size(
                                banner.right_top() + Vec2::new(-14.0, 14.0),
                                Vec2::splat(22.0),
                            ),
                            Id::new("dismiss-assistance-banner"),
                            Icon::Close,
                            theme,
                            true,
                        )
                        .clicked()
                        {
                            self.banner = false;
                        }
                    }
                });
        });
    }
}
