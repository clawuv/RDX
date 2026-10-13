//! RDX placeholder dashboard adapted from SquirrelDisk / DCX (AGPL-3.0-only).
mod cards;
#[cfg(target_os = "macos")]
mod macos;
mod theme;
mod widgets;
use eframe::egui::{self, Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Vec2};
use theme::{with_alpha, Theme};
use widgets::{bold, cr, display, font, BtnStyle, Icon};

fn main() -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1086.0, 813.0])
        .with_min_inner_size([860.0, 560.0]);
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    eframe::run_native(
        "RDX",
        eframe::NativeOptions {
            viewport,
            persist_window: false,
            ..Default::default()
        },
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(App {
                theme: theme::by_name("Paper"),
                sidebar: true,
                scroll: 0.0,
            }))
        }),
    )
}
fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-bold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    let mut fallback = fonts.families[&FontFamily::Proportional].clone();
    for path in [
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
            fallback.push("cjk".into());
            break;
        }
    }
    for (family, first) in [
        (FontFamily::Proportional, "inter"),
        (FontFamily::Name("bold".into()), "inter-bold"),
        (FontFamily::Name("display".into()), "inter-bold"),
    ] {
        let mut names = vec![first.into()];
        names.extend(fallback.clone());
        fonts.families.insert(family, names);
    }
    ctx.set_fonts(fonts);
}
struct App {
    theme: Theme,
    sidebar: bool,
    scroll: f32,
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[allow(unused_mut)]
        let mut chrome_right = 68.0;
        #[cfg(target_os = "macos")]
        {
            if let Some((cy, _, left)) = macos::traffic_lights(frame) {
                let (dx, dy) = ((27.0 - left) as f64, (24.0 - cy) as f64);
                if dx.abs() > 0.5 || dy.abs() > 0.5 {
                    macos::shift_traffic_lights(frame, dx, dy);
                }
            }
            if let Some((_, right, _)) = macos::traffic_lights(frame) {
                chrome_right = right;
            }
        }
        let _ = frame;
        let screen = ctx.content_rect();
        ui.painter().rect_filled(screen, 0.0, self.theme.shell_bg());
        let drag = ui.interact(
            Rect::from_min_size(screen.min, Vec2::new(screen.width(), 72.0)),
            Id::new("window-drag"),
            Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        let width = 204.0 * ctx.animate_bool_with_time(Id::new("sidebar-open"), self.sidebar, 0.2);
        if width > 1.0 {
            let side = Rect::from_min_max(
                screen.min,
                Pos2::new(screen.left() + width, screen.bottom()),
            );
            draw_sidebar(ui, side, &self.theme);
        }
        let content = Rect::from_min_max(
            Pos2::new(screen.left() + width + 8.0, screen.top() + 8.0),
            screen.max - Vec2::splat(8.0),
        );
        widgets::shadow(ui.painter(), content, 16.0, 0.45, &self.theme);
        ui.painter()
            .rect_filled(content, cr(12.0), self.theme.inset_bg());
        let bx = if cfg!(target_os = "macos") {
            chrome_right + 28.0
        } else {
            30.0
        };
        let toggle = Rect::from_center_size(Pos2::new(bx, content.top() + 16.0), Vec2::splat(28.0));
        if widgets::icon_button_bold(
            ui,
            toggle,
            Id::new("sidebar-toggle"),
            Icon::Panel,
            &self.theme,
            true,
        )
        .clicked()
        {
            self.sidebar = !self.sidebar;
        }
        widgets::icon_button_bold(
            ui,
            toggle.translate(Vec2::new(34.0, 0.0)),
            Id::new("new-device"),
            Icon::Plus,
            &self.theme,
            true,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_clip_rect(content);
            self.dashboard(ui, content);
        });
    }
}
fn draw_sidebar(ui: &mut egui::Ui, side: Rect, theme: &Theme) {
    let item = Rect::from_min_size(
        side.min + Vec2::new(12.0, 58.0),
        Vec2::new(side.width() - 16.0, 36.0),
    );
    nav_item(
        ui,
        item,
        Id::new("nav-dash"),
        Icon::Grid,
        "仪表板",
        true,
        false,
        None,
        theme,
    );
    let y = item.bottom() + 54.0;
    ui.painter().text(
        Pos2::new(side.left() + 24.0, y),
        Align2::LEFT_TOP,
        "最近设备",
        bold(12.5),
        with_alpha(Color32::BLACK, 0.55),
    );
    nav_item(
        ui,
        Rect::from_min_size(Pos2::new(side.left() + 12.0, y + 22.0), item.size()),
        Id::new("nav-device"),
        Icon::Disk,
        "RDX 本机",
        false,
        true,
        None,
        theme,
    );
    nav_item(
        ui,
        Rect::from_min_size(
            Pos2::new(side.left() + 12.0, side.bottom() - 44.0),
            item.size(),
        ),
        Id::new("nav-settings"),
        Icon::Gear,
        "设置",
        false,
        false,
        None,
        theme,
    );
}
impl App {
    fn dashboard(&mut self, ui: &mut egui::Ui, content: Rect) {
        let t = &self.theme;
        let header = if self.sidebar { 22.0 } else { 48.0 };
        let content_h = header + 52.0 + 122.0 + 334.0 + 16.0 + 118.0 + 64.0 + 44.0;
        let max_scroll = (content_h - content.height()).max(0.0);
        if ui.rect_contains_pointer(content) {
            self.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let (x, w) = (content.left() + 24.0, content.width() - 48.0);
        let mut y = content.top() - self.scroll + header;
        ui.painter().text(
            Pos2::new(x, y),
            Align2::LEFT_TOP,
            "仪表板",
            display(22.0),
            t.text,
        );
        widgets::icon_button(
            ui,
            Rect::from_center_size(Pos2::new(x + w - 16.0, y + 16.0), Vec2::splat(32.0)),
            Id::new("settings"),
            Icon::Gear,
            t,
            true,
        );
        y += 52.0;
        let tw = (w - 42.0) / 4.0;
        for (k, (label, value, sub, icon, accent)) in [
            ("设备", "5", "已登记设备", Icon::Disk, t.accent),
            ("在线会话", "空闲", "暂无进行中的会话", Icon::Bolt, t.warn),
            ("已授权", "3", "可远程访问", Icon::Grid, t.accent2),
            ("可用通道", "2", "等待连接请求", Icon::Check, t.ok),
        ]
        .into_iter()
        .enumerate()
        {
            cards::dash_tile(
                ui,
                Rect::from_min_size(
                    Pos2::new(x + k as f32 * (tw + 14.0), y),
                    Vec2::new(tw, 108.0),
                ),
                label,
                value,
                sub,
                icon,
                accent,
                t,
            );
        }
        y += 122.0;
        let card = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 334.0));
        cards::card_bg(ui, card, t);
        ui.painter().text(
            card.min + Vec2::new(18.0, 22.0),
            Align2::LEFT_CENTER,
            "设备",
            bold(12.5),
            t.text_dim,
        );
        let devices = [
            ("RDX 本机", "RDX-298 417 652 · 本机", 0.74, "在线"),
            ("工作室 iMac", "RDX-841 220 106 · 受信任", 0.86, "可用"),
            ("ThinkPad X1", "RDX-555 012 490 · 受信任", 0.81, "等待"),
            ("测试设备 A", "RDX-119 705 824 · 占位", 0.78, "等待"),
            ("测试设备 B", "RDX-908 446 130 · 占位", 0.34, "可用"),
        ];
        for (k, data) in devices.into_iter().enumerate() {
            device_row(
                ui,
                Rect::from_min_size(
                    card.min + Vec2::new(10.0, 40.0 + k as f32 * 56.0),
                    Vec2::new(w - 20.0, 54.0),
                ),
                k,
                data,
                t,
            );
        }
        y += 350.0;
        let cw = (w - 28.0) / 3.0;
        for (k, (icon, title, sub)) in [
            (Icon::Folder, "本机桌面", "此 Mac 当前的共享桌面"),
            (Icon::Server, "远程设备", "通过伙伴 ID 建立会话"),
            (Icon::Cloud, "Web 控制台", "浏览器控制端（规划中）"),
        ]
        .into_iter()
        .enumerate()
        {
            cards::source_card(
                ui,
                Rect::from_min_size(
                    Pos2::new(x + k as f32 * (cw + 14.0), y),
                    Vec2::new(cw, 104.0),
                ),
                Id::new(("source", k)),
                icon,
                title,
                sub,
                t,
            );
        }
        y += 118.0;
        promo(
            ui,
            Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 52.0)),
            t,
        );
        y += 64.0;
        ui.painter().text(
            Pos2::new(
                content.center().x,
                (y + 20.0).max(content.bottom() - 26.0 - self.scroll),
            ),
            Align2::CENTER_CENTER,
            "v0.1.0 · 选择设备以开始远程会话 · Esc 返回",
            font(11.5),
            t.text_faint,
        );
        if max_scroll > 0.0 {
            let track = Rect::from_min_max(
                Pos2::new(content.right() - 13.0, content.top() + 8.0),
                Pos2::new(content.right() - 8.0, content.bottom() - 8.0),
            );
            let height = (track.height() * content.height() / content_h).max(32.0);
            let thumb = Rect::from_min_size(
                Pos2::new(
                    track.left(),
                    track.top() + (track.height() - height) * self.scroll / max_scroll,
                ),
                Vec2::new(5.0, height),
            );
            let resp = ui.interact(
                thumb.expand2(Vec2::new(4.0, 0.0)),
                Id::new("home-scroll"),
                Sense::drag(),
            );
            if resp.dragged() {
                self.scroll = (self.scroll
                    + resp.drag_delta().y * max_scroll / (track.height() - height).max(1.0))
                .clamp(0.0, max_scroll);
            }
            ui.painter()
                .rect_filled(track, cr(3.0), with_alpha(t.stroke, 0.35));
            ui.painter()
                .rect_filled(thumb, cr(3.0), with_alpha(t.text_faint, 0.8));
        }
    }
}
fn device_row(ui: &mut egui::Ui, rect: Rect, k: usize, data: (&str, &str, f32, &str), t: &Theme) {
    let (name, sub, used, state) = data;
    let id = Id::new(("device", k));
    let resp = ui.interact(rect, id, Sense::click());
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered(), 0.15);
    let p = ui.painter().clone();
    if h > 0.0 {
        p.rect_filled(rect, cr(10.0), with_alpha(t.surface_hi, h * 0.7));
    }
    let tile = Rect::from_center_size(
        Pos2::new(rect.left() + 32.0, rect.center().y),
        Vec2::splat(36.0),
    );
    let tc = if k == 0 { t.accent } else { t.warn };
    p.rect_filled(tile, cr(8.0), with_alpha(tc, 0.18));
    widgets::draw_icon(
        &p,
        if k == 0 { Icon::Disk } else { Icon::External },
        tile.shrink(9.0),
        tc,
    );
    p.text(
        Pos2::new(tile.right() + 12.0, rect.center().y - 9.0),
        Align2::LEFT_CENTER,
        name,
        bold(13.5),
        t.text,
    );
    p.text(
        Pos2::new(tile.right() + 12.0, rect.center().y + 10.0),
        Align2::LEFT_CENTER,
        widgets::truncate(&p, sub, &font(11.5), rect.width() * 0.4),
        font(11.5),
        t.text_dim,
    );
    let bar = Rect::from_min_size(
        Pos2::new(rect.right() - 370.0, rect.center().y - 9.0),
        Vec2::new(180.0, 8.0),
    );
    let col = if used > 0.9 {
        t.danger
    } else if used > 0.75 {
        t.warn
    } else {
        t.ok
    };
    widgets::progress_bar(&p, bar, Some(used), t, 0.25, Some(col));
    p.text(
        Pos2::new(bar.left(), bar.bottom() + 10.0),
        Align2::LEFT_CENTER,
        format!("状态 {state}"),
        font(11.0),
        t.text_faint,
    );
    p.text(
        Pos2::new(bar.right(), bar.bottom() + 10.0),
        Align2::RIGHT_CENTER,
        "通道就绪",
        bold(11.0),
        col,
    );
    let btn = Rect::from_min_size(
        Pos2::new(rect.right() - 102.0, rect.center().y - 15.0),
        Vec2::new(90.0, 30.0),
    );
    widgets::button(
        ui,
        btn,
        id.with("connect"),
        if k == 0 { "查看" } else { "连接" },
        Some(if k == 0 { Icon::Eye } else { Icon::Bolt }),
        BtnStyle::Primary,
        t,
    );
    if k == 0 {
        widgets::icon_button(
            ui,
            Rect::from_center_size(
                Pos2::new(btn.left() - 20.0, btn.center().y),
                Vec2::splat(26.0),
            ),
            id.with("refresh"),
            Icon::Refresh,
            t,
            true,
        );
    }
}

fn nav_item(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    icon: Icon,
    label: &str,
    active: bool,
    closeable: bool,
    progress: Option<f32>,
    theme: &Theme,
) -> (bool, bool) {
    let resp = ui.interact(rect, id, Sense::click());
    let hov = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered() && !active, 0.12);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter().clone();
    // selected and hover share the same pill; the label colour tells them apart
    let fill = if active { 1.0 } else { hov };
    if fill > 0.0 {
        p.rect_filled(rect, cr(8.0), with_alpha(theme.text, 0.05 * fill));
    }
    let col = if theme.name == "Paper" {
        Color32::BLACK
    } else if active {
        theme.text
    } else {
        theme::lerp_color(theme.text_dim, theme.text, hov)
    };
    let mut tx = rect.left() + 14.0;
    // icon sized to the 13px label so both read as one line
    widgets::draw_icon(
        &p,
        icon,
        Rect::from_center_size(Pos2::new(tx + 9.0, rect.center().y), Vec2::splat(16.0)),
        col,
    );
    tx += 34.0;
    let max_w = rect.right() - tx - if closeable { 34.0 } else { 12.0 };
    let lbl = widgets::truncate(&p, label, &bold(13.0), max_w);
    // -0.5: half-step optical lift; CJK ink hangs slightly low in the line box
    p.text(
        Pos2::new(tx, rect.center().y + 1.0),
        Align2::LEFT_CENTER,
        lbl,
        bold(13.0),
        col,
    );
    let mut close = false;
    if closeable && resp.hovered() {
        let x = Rect::from_center_size(
            Pos2::new(rect.right() - 20.0, rect.center().y),
            Vec2::splat(22.0),
        );
        if widgets::icon_button(ui, x, id.with("x"), Icon::Close, theme, true).clicked() {
            close = true;
        }
    }
    if let Some(f) = progress {
        let bar = Rect::from_min_size(
            Pos2::new(rect.left() + 14.0, rect.bottom() - 3.0),
            Vec2::new((rect.width() - 28.0) * f.clamp(0.02, 1.0), 2.0),
        );
        p.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.left() + 14.0, rect.bottom() - 3.0),
                Vec2::new(rect.width() - 28.0, 2.0),
            ),
            cr(1.0),
            with_alpha(theme.stroke, 0.5),
        );
        p.rect_filled(bar, cr(1.0), theme.accent);
    }
    (resp.clicked(), close)
}

fn promo(ui: &mut egui::Ui, rect: Rect, theme: &Theme) {
    let id = Id::new("promo");

    // "why" badge on the right, before the arrow
    let arrow = Rect::from_center_size(
        Pos2::new(rect.right() - 20.0, rect.center().y),
        Vec2::splat(14.0),
    );
    let label_w = ui
        .painter()
        .layout_no_wrap("详情".into(), font(10.5), theme.text_faint)
        .size()
        .x;
    let why_rect = Rect::from_min_max(
        Pos2::new(rect.right() - 38.0 - label_w - 22.0, rect.top()),
        Pos2::new(rect.right() - 32.0, rect.bottom()),
    );
    let why = ui.interact(why_rect, id.with("why"), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered(), 0.12);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    p.rect_filled(
        rect,
        cr(10.0),
        theme::lerp_color(with_alpha(theme.surface, 0.72), theme.surface_hi, h * 0.7),
    );
    p.rect_stroke(
        rect,
        cr(10.0),
        Stroke::new(1.0, with_alpha(theme.stroke, 0.6)),
        egui::StrokeKind::Inside,
    );
    let tile = Rect::from_center_size(
        Pos2::new(rect.left() + 26.0, rect.center().y),
        Vec2::splat(32.0),
    );
    p.rect_filled(tile, cr(8.0), with_alpha(theme.accent, 0.18));
    widgets::draw_icon(p, Icon::Acorn, tile.shrink(8.0), theme.accent);
    widgets::draw_icon(
        p,
        Icon::LinkOut,
        arrow,
        theme::lerp_color(theme.text_dim, theme.text, h),
    );
    let label_x = rect.right() - 38.0;
    let why_hot = why.hovered();
    p.text(
        Pos2::new(label_x, rect.center().y),
        Align2::RIGHT_CENTER,
        "详情",
        font(10.5),
        if why_hot {
            theme.text
        } else {
            theme.text_faint
        },
    );
    // tiny "i" badge: hover to see why this sponsor was picked
    let info = Pos2::new(label_x - label_w - 10.0, rect.center().y);
    p.circle_stroke(
        info,
        6.0,
        Stroke::new(
            1.0,
            if why_hot {
                theme.text
            } else {
                theme.text_faint
            },
        ),
    );
    p.text(
        info,
        Align2::CENTER_CENTER,
        "i",
        bold(9.0),
        if why_hot {
            theme.text
        } else {
            theme.text_faint
        },
    );
    let tx = tile.right() + 12.0;
    let tw = (info.x - 14.0 - tx).max(40.0);
    let title = widgets::truncate(p, "RDX 占位区域", &bold(12.5), tw);
    if false {
        p.text(
            Pos2::new(tx, rect.center().y),
            Align2::LEFT_CENTER,
            title,
            bold(12.5),
            theme.text,
        );
    } else {
        p.text(
            Pos2::new(tx, rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            title,
            bold(12.5),
            theme.text,
        );
        let text = widgets::truncate(p, "后续可用于安全提醒、更新或状态通知。", &font(11.5), tw);
        p.text(
            Pos2::new(tx, rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            text,
            font(11.5),
            theme.text_dim,
        );
    }
}
