//! Dashboard primitives adapted from SquirrelDisk / DCX (AGPL-3.0-only).
use super::theme::{lerp_color, with_alpha};
use super::widgets::{self, bold, cr, font, Icon};
use eframe::egui::{self, Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
/// Card chrome shared by dashboard tiles and cards.
pub fn card_bg(ui: &Ui, rect: Rect, theme: &super::theme::Theme) {
    let p = ui.painter();
    widgets::shadow(p, rect, 8.0, 0.35, theme);
    p.rect_filled(rect, cr(10.0), theme.surface);
    p.rect_stroke(
        rect,
        cr(10.0),
        Stroke::new(1.0, with_alpha(theme.stroke, 0.55)),
        egui::StrokeKind::Inside,
    );
}

#[allow(clippy::too_many_arguments)] // 卡片几何/标识/文案/主题/预留宽度参数天然多
pub fn source_card(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: Icon,
    title: &str,
    sub: &str,
    theme: &super::theme::Theme,
    reserve_right: f32,
) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click());
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered(), 0.15);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    let p = ui.painter();
    let r = rect;

    card_bg(ui, r, theme);
    p.rect_filled(
        r,
        cr(10.0),
        lerp_color(Color32::TRANSPARENT, theme.surface_hi, h * 0.5),
    );
    p.rect_stroke(
        r,
        cr(10.0),
        Stroke::new(
            1.0,
            lerp_color(with_alpha(theme.stroke, 0.55), theme.accent, h * 0.7),
        ),
        egui::StrokeKind::Inside,
    );
    let ic = Pos2::new(r.left() + 30.0, r.center().y);
    p.rect_filled(
        Rect::from_center_size(ic, Vec2::splat(34.0)),
        cr(8.0),
        with_alpha(theme.accent, 0.18 + 0.1 * h),
    );
    widgets::draw_icon(
        p,
        icon,
        Rect::from_center_size(ic, Vec2::splat(20.0)),
        lerp_color(theme.accent, theme.text, h * 0.5),
    );
    p.text(
        Pos2::new(r.left() + 56.0, r.center().y - 10.0),
        Align2::LEFT_CENTER,
        widgets::truncate(
            p,
            title,
            &bold(14.0),
            (r.width() - 68.0 - reserve_right).max(20.0),
        ),
        bold(14.0),
        theme.text,
    );
    let s = widgets::truncate(
        p,
        sub,
        &font(11.5),
        (r.width() - 68.0 - reserve_right).max(20.0),
    );
    p.text(
        Pos2::new(r.left() + 56.0, r.center().y + 10.0),
        Align2::LEFT_CENTER,
        s,
        font(11.5),
        theme.text_dim,
    );
    resp
}
