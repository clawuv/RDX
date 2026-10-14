//! RDX placeholder dashboard adapted from SquirrelDisk / DCX (AGPL-3.0-only).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assistance;
mod cards;
#[cfg(target_os = "macos")]
mod macos;
mod settings;
mod share;
mod theme;
mod updates;
mod widgets;
#[cfg(windows)]
mod win;
use eframe::egui::{self, Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Vec2};
use theme::{with_alpha, Theme};
use widgets::{bold, cr, display, font, menu, BtnStyle, Icon};

pub(crate) const PAGE_PADDING_X: f32 = 48.0;
pub(crate) const TOOLBAR_H: f32 = 36.0;
const SIDEBAR_ITEM_LEFT: f32 = 12.0;
const SIDEBAR_ICON_CENTER_X: f32 = 30.0;

pub(crate) fn page_header_offset(sidebar: bool) -> f32 {
    if sidebar || cfg!(windows) {
        22.0
    } else {
        48.0
    }
}

fn main() -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([860.0, 560.0])
        .with_min_inner_size([860.0, 560.0]);
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_transparent(true)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    #[cfg(windows)]
    let viewport = viewport.with_decorations(false);
    eframe::run_native(
        "RDX",
        eframe::NativeOptions {
            viewport,
            wgpu_options: rd_app::wgpu_configuration(),
            persist_window: false,
            ..Default::default()
        },
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::ThemePreference::System);
            Ok(Box::new(App {
                theme: theme::mist(),
                sidebar: true,
                scroll: 0.0,
                vibrancy: false,
                page: Page::Tools,
                assistance: assistance::Assistance::default(),
                settings: settings::Settings::default(),
                device_page: 0,
                devices_per_page: 5,
                device_filter: 0,
                updates: updates::Updates::default(),
            }))
        }),
    )
}
#[cfg(all(test, target_os = "macos"))]
mod font_alignment_tests {
    use super::*;

    #[test]
    fn macos_text_is_centered_across_page_fonts() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for size in [12.0, 14.0, 18.0, 22.0] {
                for family in [font(size), menu(size), bold(size), display(size)] {
                    for text in ["远程协助", "RDX", "RDX 本机"] {
                        let g = ui.painter().layout_no_wrap(
                            text.into(),
                            family.clone(),
                            Color32::WHITE,
                        );
                        let offset = g.mesh_bounds.center().y - g.rect.center().y;
                        assert!(offset.abs() <= 1.5, "{family:?} {text}: offset={offset}");
                    }
                }
            }
        });
        output.textures_delta.clear();
    }
}

fn cjk_font_data(path: &str, bytes: Vec<u8>) -> egui::FontData {
    let mut data = egui::FontData::from_owned(bytes);
    // Hiragino's ink sits roughly 0.23 em above Inter's line center in egui.
    // Correct the fallback glyphs (including TextEdit/labels), not page geometry
    // or Latin digits. Keep YaHei and other platform fallbacks unchanged.
    if cfg!(target_os = "macos") && path.ends_with("Hiragino Sans GB.ttc") {
        data.tweak.y_offset_factor = 0.23;
    }
    data
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
                .insert("cjk".into(), Arc::new(cjk_font_data(path, bytes)));
            fallback.push("cjk".into());
            break;
        }
    }
    // Bold CJK companion; without it "bold"-family CJK labels (menus) fall
    // through to the regular-weight fallback above.
    let mut cjk_bold = false;
    for path in [
        "C:/Windows/Fonts/msyhbd.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk-bold".into(), Arc::new(cjk_font_data(path, bytes)));
            cjk_bold = true;
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
    // Sidebar menu family: Latin goes inter-bold, CJK goes to the bold
    // fallback when available, then the regular one.
    let mut menu_family = vec!["inter-bold".into()];
    if cjk_bold {
        menu_family.push("cjk-bold".into());
    }
    menu_family.extend(fallback.clone());
    fonts
        .families
        .insert(FontFamily::Name("menu".into()), menu_family);
    ctx.set_fonts(fonts);
}
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Tools,
    Devices,
    Assistance,
    Settings,
}

struct App {
    theme: Theme,
    sidebar: bool,
    scroll: f32,
    vibrancy: bool,
    page: Page,
    assistance: assistance::Assistance,
    settings: settings::Settings,
    device_page: usize,
    devices_per_page: usize,
    device_filter: usize,
    updates: updates::Updates,
}
impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.updates.poll(&ctx);
        self.settings.apply_theme(&ctx, &mut self.theme);
        ui.set_style(ctx.global_style());
        #[allow(unused_mut)]
        let mut chrome_right = 68.0;
        #[cfg(target_os = "macos")]
        {
            if !self.vibrancy {
                self.vibrancy = macos::install_vibrancy(frame);
            }
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
        #[cfg(windows)]
        win::install_resize_frame(frame);
        let _ = frame;
        let screen = ctx.content_rect();
        // Keep the reference charcoal stable even over a light macOS backdrop.
        let tint = if self.vibrancy && !self.theme.dark {
            with_alpha(self.theme.shell_bg(), 0.20)
        } else {
            self.theme.shell_bg()
        };
        ui.painter().rect_filled(screen, 0.0, tint);
        if self.theme.name == "Mist" {
            let chrome =
                Rect::from_min_max(screen.min, Pos2::new(screen.right(), screen.top() + 96.0));
            widgets::vgradient(
                ui.painter(),
                chrome,
                with_alpha(self.theme.bg_top, if self.vibrancy { 0.65 } else { 1.0 }),
                with_alpha(self.theme.bg_bottom, if self.vibrancy { 0.0 } else { 1.0 }),
            );
        }
        let drag = ui.interact(
            Rect::from_min_size(
                screen.min,
                Vec2::new(screen.width(), if cfg!(windows) { TOOLBAR_H } else { 72.0 }),
            ),
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
            if let Some(page) = draw_sidebar(ui, side, &self.theme, self.page, &mut self.updates) {
                self.page = page;
                self.scroll = 0.0;
            }
        }
        let content = Rect::from_min_max(
            Pos2::new(
                screen.left() + width + 8.0,
                screen.top() + if cfg!(windows) { TOOLBAR_H + 8.0 } else { 8.0 },
            ),
            screen.max - Vec2::splat(8.0),
        );
        widgets::shadow(ui.painter(), content, 16.0, 0.45, &self.theme);
        ui.painter()
            .rect_filled(content, cr(12.0), self.theme.inset_bg());
        let bx = if cfg!(target_os = "macos") {
            chrome_right + 28.0
        } else {
            SIDEBAR_ICON_CENTER_X
        };
        let by = if cfg!(windows) {
            screen.top() + TOOLBAR_H / 2.0
        } else {
            content.top() + 16.0
        };
        let toggle = Rect::from_center_size(Pos2::new(screen.left() + bx, by), Vec2::splat(28.0));
        if widgets::icon_button_bold(
            ui,
            toggle,
            Id::new("sidebar-toggle"),
            Icon::Panel,
            &self.theme,
            true,
        )
        .on_hover_text(if self.sidebar {
            "收起侧栏"
        } else {
            "展开侧栏"
        })
        .clicked()
        {
            self.sidebar = !self.sidebar;
        }
        widgets::icon_button_bold(
            ui,
            toggle.translate(Vec2::new(34.0, 0.0)),
            Id::new("notifications"),
            Icon::Bell,
            &self.theme,
            true,
        )
        .on_hover_text("通知");
        share::show(ui, toggle.translate(Vec2::new(68.0, 0.0)), &self.theme);
        #[cfg(windows)]
        win::caption_buttons(ui, screen, &self.theme);
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_clip_rect(content);
            match self.page {
                Page::Tools => self.tools_page(ui, content),
                Page::Devices => self.devices_page(ui, content),
                Page::Assistance => self.assistance.show(ui, content, &self.theme, self.sidebar),
                Page::Settings => self
                    .settings
                    .show(ui, content, &self.theme, &mut self.sidebar),
            }
        });
    }
}
fn draw_sidebar(
    ui: &mut egui::Ui,
    side: Rect,
    theme: &Theme,
    current: Page,
    updates: &mut updates::Updates,
) -> Option<Page> {
    // Align the first menu item's top edge with the content sheet's top edge.
    let menu_top = if cfg!(windows) { TOOLBAR_H + 8.0 } else { 58.0 };
    let item = Rect::from_min_size(
        side.min + Vec2::new(SIDEBAR_ITEM_LEFT, menu_top),
        Vec2::new(side.width() - 16.0, 36.0),
    );
    let mut selected = None;
    for (index, page, icon, label) in [
        (0, Page::Assistance, Icon::Monitor, "远程协助"),
        (1, Page::Devices, Icon::Server, "设备"),
        (2, Page::Tools, Icon::Grid, "工具"),
    ] {
        if nav_item(
            ui,
            item.translate(Vec2::new(0.0, index as f32 * 40.0)),
            Id::new(("page", index)),
            icon,
            label,
            current == page,
            false,
            None,
            theme,
        )
        .0
        {
            selected = Some(page);
        }
    }
    let y = item.bottom() + 134.0;
    ui.painter().text(
        Pos2::new(side.left() + 24.0, y),
        Align2::LEFT_TOP,
        "最近设备",
        menu(13.5),
        theme.text_dim,
    );
    if nav_item(
        ui,
        Rect::from_min_size(
            Pos2::new(side.left() + SIDEBAR_ITEM_LEFT, y + 22.0),
            item.size(),
        ),
        Id::new("nav-device"),
        Icon::Disk,
        "RDX 本机",
        false,
        false,
        None,
        theme,
    )
    .0
    {
        selected = Some(Page::Devices);
    }
    let settings_row = Rect::from_min_size(
        Pos2::new(side.left() + SIDEBAR_ITEM_LEFT, side.bottom() - 44.0),
        item.size(),
    );
    let update_rect = Rect::from_center_size(
        Pos2::new(settings_row.right() - 18.0, settings_row.center().y),
        Vec2::splat(28.0),
    );
    if nav_item(
        ui,
        Rect::from_min_max(
            settings_row.min,
            Pos2::new(update_rect.left() - 8.0, settings_row.bottom()),
        ),
        Id::new("nav-settings"),
        Icon::Gear,
        "设置",
        current == Page::Settings,
        false,
        None,
        theme,
    )
    .0
    {
        selected = Some(Page::Settings);
    }
    updates.show(ui, update_rect, settings_row, theme);
    selected
}

impl App {
    fn devices_page(&mut self, ui: &mut egui::Ui, content: Rect) {
        let t = &self.theme;
        let header = page_header_offset(self.sidebar);
        let (x, w) = (
            content.left() + PAGE_PADDING_X,
            content.width() - PAGE_PADDING_X * 2.0,
        );
        let top = content.top() + header;
        ui.painter().text(
            Pos2::new(x, top),
            Align2::LEFT_TOP,
            "设备",
            display(22.0),
            t.text,
        );
        let y = top + 44.0;
        ui.painter().text(
            Pos2::new(x, y + 14.0),
            Align2::LEFT_CENTER,
            "已登记设备",
            bold(18.0),
            t.text,
        );
        let devices = [
            ("RDX 本机", "RDX-298 417 652 · 本机", 0.74, "在线"),
            ("工作室 iMac", "RDX-841 220 106 · 受信任", 0.86, "可用"),
            ("ThinkPad X1", "RDX-555 012 490 · 受信任", 0.81, "等待"),
            ("测试设备 A", "RDX-119 705 824 · 占位", 0.78, "等待"),
            ("测试设备 B", "RDX-908 446 130 · 占位", 0.34, "可用"),
        ];
        widgets::icon_button(
            ui,
            Rect::from_center_size(Pos2::new(x + w - 150.0, y + 14.0), Vec2::splat(28.0)),
            Id::new("refresh-devices"),
            Icon::Refresh,
            t,
            true,
        )
        .on_hover_text("刷新设备列表");
        let previous_filter = self.device_filter;
        let filters: Vec<_> = ["全部设备", "本机", "受信任设备", "占位设备"]
            .into_iter()
            .enumerate()
            .map(|(value, label)| (value, label.to_owned()))
            .collect();
        let filter_label = if previous_filter == 0 {
            "筛选".to_owned()
        } else {
            filters[previous_filter].1.clone()
        };
        pagination_select(
            ui,
            Rect::from_min_size(Pos2::new(x + w - 128.0, y), Vec2::new(128.0, 28.0)),
            Id::new("device-filter"),
            &mut self.device_filter,
            filter_label,
            &filters,
            t,
        );
        if previous_filter != self.device_filter {
            self.device_page = 0;
        }
        let filtered: Vec<_> = devices
            .into_iter()
            .enumerate()
            .filter(|(_, device)| match self.device_filter {
                1 => device.1.ends_with("本机"),
                2 => device.1.ends_with("受信任"),
                3 => device.1.ends_with("占位"),
                _ => true,
            })
            .collect();
        let total = filtered.len();
        let pager_top = content.bottom() - 68.0;
        let pager = Rect::from_min_size(Pos2::new(x, pager_top), Vec2::new(w, 52.0));
        ui.painter().line_segment(
            [pager.left_top(), pager.right_top()],
            Stroke::new(1.0, with_alpha(t.stroke, 0.4)),
        );
        let controls_top = pager.top() + 20.0;
        let center_y = controls_top + 14.0;
        ui.painter().text(
            Pos2::new(x, center_y),
            Align2::LEFT_CENTER,
            "每页",
            font(12.0),
            t.text_dim,
        );
        let previous_size = self.devices_per_page;
        let sizes: Vec<_> = [3, 5, 10, 20]
            .into_iter()
            .map(|n| (n, format!("{n} 条")))
            .collect();
        pagination_select(
            ui,
            Rect::from_min_size(Pos2::new(x + 34.0, controls_top), Vec2::new(72.0, 28.0)),
            Id::new("devices-per-page"),
            &mut self.devices_per_page,
            format!("{} 条", previous_size),
            &sizes,
            t,
        );
        if previous_size != self.devices_per_page {
            self.device_page = 0;
        }
        let pages = total.div_ceil(self.devices_per_page).max(1);
        self.device_page = self.device_page.min(pages - 1);
        let nav_x = pager.center().x - 120.0;
        let prev = Rect::from_min_size(Pos2::new(nav_x, controls_top), Vec2::new(68.0, 28.0));
        if widgets::button_ex(
            ui,
            prev,
            Id::new("device-prev"),
            "上一页",
            None,
            BtnStyle::Subtle,
            t,
            self.device_page > 0,
        )
        .clicked()
        {
            self.device_page -= 1;
        }
        let options: Vec<_> = (0..pages)
            .map(|page| (page, format!("第 {} 页", page + 1)))
            .collect();
        let selected = format!("{} / {} 页", self.device_page + 1, pages);
        pagination_select(
            ui,
            Rect::from_min_size(Pos2::new(nav_x + 76.0, controls_top), Vec2::new(88.0, 28.0)),
            Id::new("device-page-number"),
            &mut self.device_page,
            selected,
            &options,
            t,
        );
        let next = Rect::from_min_size(
            Pos2::new(nav_x + 172.0, controls_top),
            Vec2::new(68.0, 28.0),
        );
        if widgets::button_ex(
            ui,
            next,
            Id::new("device-next"),
            "下一页",
            None,
            BtnStyle::Subtle,
            t,
            self.device_page + 1 < pages,
        )
        .clicked()
        {
            self.device_page += 1;
        }
        ui.painter().text(
            Pos2::new(pager.right(), center_y),
            Align2::RIGHT_CENTER,
            format!("总共 {total} 台"),
            font(12.0),
            t.text_dim,
        );
        let start = self.device_page * self.devices_per_page;
        let end = (start + self.devices_per_page).min(total);
        let list = Rect::from_min_max(Pos2::new(x, y + 30.0), Pos2::new(x + w, pager_top - 8.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("devices-scroll", self.device_page, self.devices_per_page))
                .max_height(list.height())
                .show(ui, |ui| {
                    for (position, &(k, data)) in filtered.iter().enumerate().take(end).skip(start)
                    {
                        let (row, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 60.0),
                            Sense::hover(),
                        );
                        device_row(ui, row, k, data, t);
                        if position + 1 < end {
                            ui.painter().line_segment(
                                [row.left_bottom(), row.right_bottom()],
                                Stroke::new(1.0, with_alpha(t.stroke, 0.4)),
                            );
                        }
                    }
                });
        });
    }
    fn tools_page(&mut self, ui: &mut egui::Ui, content: Rect) {
        let t = &self.theme;
        let header = page_header_offset(self.sidebar);
        let banner_top = content.bottom() - 16.0 - 52.0;
        let scroll_area =
            Rect::from_min_max(content.min, Pos2::new(content.right(), banner_top - 16.0));
        let original_clip = ui.clip_rect();
        ui.set_clip_rect(original_clip.intersect(scroll_area));
        let content_h = header + 44.0 + 2.0 * (28.0 + 84.0);
        let max_scroll = (content_h - scroll_area.height()).max(0.0);
        if ui.rect_contains_pointer(scroll_area) {
            self.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let (x, w) = (
            content.left() + PAGE_PADDING_X,
            content.width() - PAGE_PADDING_X * 2.0,
        );
        let mut y = content.top() - self.scroll + header;
        ui.painter().text(
            Pos2::new(x, y),
            Align2::LEFT_TOP,
            "工具",
            display(22.0),
            t.text,
        );
        if widgets::icon_button(
            ui,
            Rect::from_center_size(Pos2::new(x + w - 16.0, y + 16.0), Vec2::splat(32.0)),
            Id::new("settings"),
            Icon::Gear,
            t,
            true,
        )
        .clicked()
        {
            self.page = Page::Settings;
            self.scroll = 0.0;
            ui.set_clip_rect(original_clip);
            return;
        }
        y += 44.0;
        let cw = (w - 12.0) / 2.0;
        #[allow(clippy::type_complexity)] // 静态页数据表，展开成类型别名反而绕
        let groups: [(&str, &[(Icon, &str, &str)]); 2] = [
            (
                "本机工具",
                &[(Icon::Folder, "本机桌面", "此 Mac 当前的共享桌面")],
            ),
            (
                "远程访问",
                &[
                    (Icon::Server, "远程设备", "查看设备并建立远程会话"),
                    (Icon::Cloud, "Web 控制台", "浏览器控制端（规划中）"),
                ],
            ),
        ];
        let mut open_devices = false;
        for (group_index, (label, tools)) in groups.into_iter().enumerate() {
            ui.painter().text(
                Pos2::new(x, y + 12.0),
                Align2::LEFT_CENTER,
                label,
                bold(16.0),
                t.text,
            );
            y += 28.0;
            for (index, &(icon, title, subtitle)) in tools.iter().enumerate() {
                let response = cards::source_card(
                    ui,
                    Rect::from_min_size(
                        Pos2::new(x + index as f32 * (cw + 12.0), y),
                        Vec2::new(cw, 68.0),
                    ),
                    Id::new(("tool", group_index, index)),
                    icon,
                    title,
                    subtitle,
                    t,
                );
                if group_index == 1 && index == 0 && response.clicked() {
                    open_devices = true;
                }
            }
            y += 84.0;
        }
        if open_devices {
            self.page = Page::Devices;
            self.scroll = 0.0;
            ui.set_clip_rect(original_clip);
            return;
        }
        if max_scroll > 0.0 {
            let track = Rect::from_min_max(
                Pos2::new(content.right() - 13.0, content.top() + 8.0),
                Pos2::new(content.right() - 8.0, scroll_area.bottom() - 8.0),
            );
            let height = (track.height() * scroll_area.height() / content_h).max(32.0);
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
        ui.set_clip_rect(original_clip);
        promo(
            ui,
            Rect::from_min_size(Pos2::new(x, banner_top), Vec2::new(w, 52.0)),
            t,
        );
    }
}
fn device_row(ui: &mut egui::Ui, rect: Rect, k: usize, data: (&str, &str, f32, &str), t: &Theme) {
    let (name, sub, _, _) = data;
    let id = Id::new(("device", k));
    let resp = ui.interact(rect, id, Sense::click());
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered(), 0.15);
    let p = ui.painter().clone();
    if h > 0.0 {
        p.rect_filled(rect, cr(4.0), with_alpha(t.surface_hi, h * 0.5));
    }
    let tile = Rect::from_center_size(
        Pos2::new(rect.left() + 18.0, rect.center().y),
        Vec2::splat(36.0),
    );
    let tc = t.text_dim;
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
}

#[allow(clippy::too_many_arguments)] // UI 绘制辅助：矩形/ID/主题/文案参数天然多
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
    let col = if theme.name == "Paper" || theme.name == "Mist" {
        Color32::BLACK
    } else if active {
        theme.text
    } else {
        theme::lerp_color(theme.text_dim, theme.text, hov)
    };
    // Share the toolbar's icon axis; preserve the existing icon-to-label gap.
    let icon_x = rect.left() + SIDEBAR_ICON_CENTER_X - SIDEBAR_ITEM_LEFT;
    // Match the gear's visible extent, accounting for each glyph's inner padding.
    // Keep wide glyphs proportional rather than stretching them into squares.
    let icon_size = match icon {
        Icon::Monitor => 18.0,
        Icon::Server => 17.0,
        Icon::Grid => 15.75,
        _ => 16.0,
    };
    widgets::draw_icon(
        &p,
        icon,
        Rect::from_center_size(Pos2::new(icon_x, rect.center().y), Vec2::splat(icon_size)),
        col,
    );
    let tx = icon_x + 25.0;
    let max_w = rect.right() - tx - if closeable { 34.0 } else { 12.0 };
    let lbl = widgets::truncate(&p, label, &menu(14.0), max_w);
    // Preserve the existing YaHei compensation on Windows only. Hiragino
    // is corrected at font registration so every page shares the same baseline.
    let text_y = rect.center().y - if cfg!(windows) { 2.0 } else { 0.0 };
    p.text(
        Pos2::new(tx, text_y),
        Align2::LEFT_CENTER,
        lbl,
        menu(14.0),
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

/// Shared device filter/pagination dropdown with a consistent text baseline.
fn pagination_select(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    selected: &mut usize,
    label: String,
    options: &[(usize, String)],
    theme: &Theme,
) {
    let response = ui.interact(rect, id, Sense::click());
    let popup_id = id.with("menu");
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let hot = response.hovered() || open;
    let p = ui.painter();
    p.rect_filled(
        rect,
        cr(6.0),
        if hot { theme.surface_hi } else { theme.surface },
    );
    p.rect_stroke(
        rect,
        cr(6.0),
        Stroke::new(1.0, if hot { theme.text_faint } else { theme.stroke }),
        egui::StrokeKind::Inside,
    );
    p.text(
        Pos2::new(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font(12.0),
        theme.text,
    );
    let c = Pos2::new(rect.right() - 12.0, rect.center().y);
    let direction = if open { -1.0 } else { 1.0 };
    p.add(egui::Shape::line(
        vec![
            c + Vec2::new(-3.0, -1.5 * direction),
            c + Vec2::new(0.0, 1.5 * direction),
            c + Vec2::new(3.0, -1.5 * direction),
        ],
        Stroke::new(1.3, theme.text_dim),
    ));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    egui::Popup::menu(&response)
        .id(popup_id)
        .width(rect.width().max(96.0))
        .frame(
            egui::Frame::popup(ui.style())
                .fill(theme.surface)
                .stroke(Stroke::new(1.0, theme.stroke))
                .corner_radius(cr(7.0))
                .inner_margin(6.0),
        )
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for (value, text) in options {
                let (row, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
                if response.hovered() || *selected == *value {
                    ui.painter().rect_filled(row, cr(4.0), theme.surface_hi);
                }
                ui.painter().text(
                    Pos2::new(row.left() + 8.0, row.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    font(12.0),
                    theme.text,
                );
                if *selected == *value {
                    widgets::draw_icon(
                        ui.painter(),
                        Icon::Check,
                        Rect::from_center_size(
                            Pos2::new(row.right() - 10.0, row.center().y),
                            Vec2::splat(12.0),
                        ),
                        theme.text_dim,
                    );
                }
                if response.clicked() {
                    *selected = *value;
                    ui.close();
                }
            }
        });
}
