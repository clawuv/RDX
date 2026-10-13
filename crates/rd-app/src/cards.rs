//! Dashboard primitives adapted from SquirrelDisk / DCX (AGPL-3.0-only).
use super::theme::{lerp_color, with_alpha};
use super::widgets::{self, bold, cr, display, font, Icon};
use eframe::egui::{self, Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
/// One stat tile: label top-left, big value under it, a coloured underline
/// at the bottom edge (like the Motrix dashboard tiles).
pub fn dash_tile(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    value: &str,
    sub: &str,
    icon: Icon,
    accent: Color32,
    theme: &super::theme::Theme,
) {
    let p = ui.painter();
    card_bg(ui, rect, theme);
    p.text(
        rect.left_top() + Vec2::new(16.0, 22.0),
        Align2::LEFT_CENTER,
        label,
        font(12.0),
        theme.text_dim,
    );
    widgets::draw_icon(
        p,
        icon,
        Rect::from_center_size(
            Pos2::new(rect.right() - 24.0, rect.top() + 22.0),
            Vec2::splat(16.0),
        ),
        theme.text_faint,
    );
    let g = p.layout_no_wrap(value.to_string(), display(24.0), theme.text);
    let fs = if g.size().x > rect.width() - 32.0 {
        24.0 * (rect.width() - 32.0) / g.size().x
    } else {
        24.0
    };
    p.text(
        Pos2::new(rect.left() + 16.0, rect.top() + 54.0),
        Align2::LEFT_CENTER,
        value,
        display(fs),
        theme.text,
    );
    let sub = widgets::truncate(p, sub, &font(11.0), rect.width() - 32.0);
    p.text(
        Pos2::new(rect.left() + 16.0, rect.top() + 80.0),
        Align2::LEFT_CENTER,
        sub,
        font(11.0),
        theme.text_faint,
    );
    let line = Rect::from_min_size(
        Pos2::new(rect.left() + 14.0, rect.bottom() - 12.0),
        Vec2::new(rect.width() - 28.0, 4.0),
    );
    p.rect_filled(line, cr(2.0), with_alpha(theme.stroke, 0.5));
    p.rect_filled(
        Rect::from_min_size(line.min, Vec2::new(line.width() * 0.35, 4.0)),
        cr(2.0),
        accent,
    );
}

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

pub fn source_card(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    icon: Icon,
    title: &str,
    sub: &str,
    theme: &super::theme::Theme,
) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click());
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered(), 0.15);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
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
    let ic = Pos2::new(r.left() + 30.0, r.top() + 32.0);
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
        Pos2::new(r.left() + 16.0, r.top() + 66.0),
        Align2::LEFT_CENTER,
        title,
        bold(14.0),
        theme.text,
    );
    let s = widgets::truncate(p, sub, &font(11.5), r.width() - 28.0);
    p.text(
        Pos2::new(r.left() + 16.0, r.top() + 86.0),
        Align2::LEFT_CENTER,
        s,
        font(11.5),
        theme.text_dim,
    );
    resp
}
