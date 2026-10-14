use crate::{
    theme::{self, Theme},
    widgets::{bold, display, font},
    PAGE_PADDING_X,
};
use eframe::egui::{self, Align2, Pos2, Rect, Vec2};

pub struct Settings {
    scale: f32,
    appearance: egui::ThemePreference,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            scale: 1.0,
            appearance: egui::ThemePreference::System,
        }
    }
}
impl Settings {
    pub fn apply_theme(&self, ctx: &egui::Context, palette: &mut Theme) {
        ctx.set_theme(self.appearance);
        let dark = ctx.theme() == egui::Theme::Dark;
        if palette.dark != dark {
            *palette = if dark {
                theme::mist_dark()
            } else {
                theme::mist()
            };
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, content: Rect, theme: &Theme, sidebar: &mut bool) {
        let x = content.left() + PAGE_PADDING_X;
        let y = content.top() + crate::page_header_offset(*sidebar);
        ui.painter().text(
            Pos2::new(x, y),
            Align2::LEFT_TOP,
            "设置",
            display(22.0),
            theme.text,
        );
        let body = Rect::from_min_max(
            Pos2::new(content.left(), y + 44.0),
            content.max - Vec2::new(4.0, 12.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("settings-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Full-width scroll viewport, inset settings content.
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: PAGE_PADDING_X as i8,
                            right: (PAGE_PADDING_X - 4.0) as i8,
                            top: 0,
                            bottom: 16,
                        })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // Widgets without an explicit colour (checkbox/button
                            // captions) should follow the app palette, not egui's
                            // stock gray.
                            ui.visuals_mut().override_text_color = Some(theme.text);
                            ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
                            section(ui, "外观", "主题与界面显示", theme, |ui| {
                                let before = self.appearance;
                                ui.columns(3, |columns| {
                                    theme_choice(
                                        &mut columns[0],
                                        &mut self.appearance,
                                        egui::ThemePreference::Light,
                                        "浅色",
                                        "明亮清爽",
                                    );
                                    theme_choice(
                                        &mut columns[1],
                                        &mut self.appearance,
                                        egui::ThemePreference::Dark,
                                        "深色",
                                        "柔和暗色",
                                    );
                                    theme_choice(
                                        &mut columns[2],
                                        &mut self.appearance,
                                        egui::ThemePreference::System,
                                        "跟随系统",
                                        "自动切换",
                                    );
                                });
                                if before != self.appearance {
                                    ui.ctx().set_theme(self.appearance);
                                    ui.ctx().request_repaint();
                                }
                                ui.label(
                                    egui::RichText::new(
                                        if self.appearance == egui::ThemePreference::System {
                                            if theme.dark {
                                                "跟随系统 · 当前为深色"
                                            } else {
                                                "跟随系统 · 当前为浅色"
                                            }
                                        } else {
                                            "已手动选择主题，不受系统外观变化影响"
                                        },
                                    )
                                    .font(font(11.5))
                                    .color(theme.text_dim),
                                );
                                soft_separator(ui, theme);
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("显示侧栏")
                                                .font(bold(13.0))
                                                .color(theme.text),
                                        );
                                        ui.label(
                                            egui::RichText::new("快速切换设备、工具与远程协助")
                                                .font(font(11.5))
                                                .color(theme.text_dim),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.checkbox(sidebar, "启用");
                                        },
                                    );
                                });
                                soft_separator(ui, theme);
                                ui.label(
                                    egui::RichText::new("界面缩放")
                                        .font(bold(13.0))
                                        .color(theme.text),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().slider_width =
                                        (ui.available_width() - 150.0).clamp(100.0, 260.0);
                                    let changed = ui
                                        .add(
                                            egui::Slider::new(&mut self.scale, 0.8..=1.25)
                                                .step_by(0.05)
                                                .show_value(false),
                                        )
                                        .changed();
                                    ui.label(format!("{:.0}%", self.scale * 100.0));
                                    let reset = ui
                                        .add_enabled(
                                            (self.scale - 1.0).abs() > 0.001,
                                            egui::Button::new("重置"),
                                        )
                                        .clicked();
                                    if reset {
                                        self.scale = 1.0;
                                    }
                                    if changed || reset {
                                        ui.ctx().set_zoom_factor(self.scale);
                                    }
                                });
                            });
                            ui.add_space(20.0);
                            section(
                                ui,
                                "远程连接",
                                "远程服务尚未接入，以下功能暂不可用",
                                theme,
                                |ui| {
                                    ui.add_enabled_ui(false, |ui| {
                                        for label in
                                            ["允许无人值守访问", "同步剪贴板", "允许文件传输"]
                                        {
                                            let mut value = false;
                                            ui.checkbox(&mut value, label);
                                        }
                                    });
                                },
                            );
                            ui.add_space(20.0);
                            section(
                                ui,
                                "关于 RDX",
                                "远程桌面与协助 · 界面预览版",
                                theme,
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "版本 {}",
                                            env!("CARGO_PKG_VERSION")
                                        ))
                                        .color(theme.text_dim),
                                    );
                                },
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("外观调整即时生效，仅在本次运行中保留。")
                                    .font(font(11.5))
                                    .color(theme.text_faint),
                            );
                        });
                });
        });
    }
}
/// DCX-style theme swatch: miniature UI, animated lift and selection outline.
fn theme_choice(
    ui: &mut egui::Ui,
    selected: &mut egui::ThemePreference,
    mode: egui::ThemePreference,
    title: &str,
    hint: &str,
) {
    use crate::theme::{lerp_color, with_alpha};
    use egui::{Color32, Sense, Stroke, StrokeKind};
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 88.0), Sense::click());
    response = response
        .on_hover_text(hint)
        .on_hover_cursor(egui::CursorIcon::Default);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            *selected == mode,
            title,
        )
    });
    if response.clicked() {
        *selected = mode;
        response.mark_changed();
    }
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    let active =
        ui.ctx()
            .animate_bool_with_time(response.id.with("selected"), *selected == mode, 0.15);
    let r = rect
        .shrink2(Vec2::new(0.0, 3.0))
        .translate(Vec2::new(0.0, -2.0 * hover));
    let light = theme::mist();
    let dark = theme::mist_dark();
    let base = if mode == egui::ThemePreference::Dark {
        &dark
    } else {
        &light
    };
    let p = ui.painter();
    p.rect_filled(r, 10.0, lerp_color(base.bg_top, base.bg_bottom, 0.5));
    if mode == egui::ThemePreference::System {
        let clip = Rect::from_min_max(Pos2::new(r.center().x, r.top()), r.max);
        p.with_clip_rect(clip).rect_filled(r, 10.0, dark.bg_top);
    }
    // Miniature sidebar and sheet show the actual palettes instead of a label alone.
    let preview = Rect::from_min_max(
        r.min + Vec2::new(9.0, 9.0),
        Pos2::new(r.right() - 9.0, r.bottom() - 28.0),
    );
    let draw_preview = |p: &egui::Painter, t: &Theme| {
        p.rect_filled(preview, 4.0, t.shell_bg());
        let sheet = Rect::from_min_max(
            preview.min + Vec2::new(preview.width() * 0.24, 4.0),
            preview.max - Vec2::splat(4.0),
        );
        p.rect_filled(sheet, 3.0, t.surface);
        for row in 0..3 {
            let y = sheet.top() + 6.0 + row as f32 * 8.0;
            let width = (sheet.width() - 12.0) * (1.0 - row as f32 * 0.16);
            p.rect_filled(
                Rect::from_min_size(
                    Pos2::new(sheet.left() + 6.0, y),
                    Vec2::new(width.max(2.0), 3.0),
                ),
                1.5,
                with_alpha(t.text_dim, 0.5),
            );
        }
        p.rect_filled(
            Rect::from_min_size(
                preview.min + Vec2::new(4.0, 7.0),
                Vec2::new((preview.width() * 0.24 - 8.0).max(2.0), 4.0),
            ),
            2.0,
            t.accent,
        );
    };
    draw_preview(p, base);
    if mode == egui::ThemePreference::System {
        draw_preview(
            &p.with_clip_rect(Rect::from_min_max(Pos2::new(r.center().x, r.top()), r.max)),
            &dark,
        );
    }
    let label_pos = Pos2::new(r.left() + 10.0, r.bottom() - 13.0);
    if mode == egui::ThemePreference::System {
        for (clip, color) in [
            (
                Rect::from_min_max(r.min, Pos2::new(r.center().x, r.bottom())),
                light.text,
            ),
            (
                Rect::from_min_max(Pos2::new(r.center().x, r.top()), r.max),
                dark.text,
            ),
        ] {
            p.with_clip_rect(clip)
                .text(label_pos, Align2::LEFT_CENTER, title, bold(11.5), color);
        }
    } else {
        p.text(label_pos, Align2::LEFT_CENTER, title, bold(11.5), base.text);
    }
    let accent = Color32::from_rgb(100, 135, 220);
    p.rect_stroke(
        r,
        10.0,
        Stroke::new(
            1.0 + 1.5 * active,
            lerp_color(
                with_alpha(base.stroke, 0.8),
                accent,
                active.max(hover * 0.5),
            ),
        ),
        StrokeKind::Inside,
    );
    if *selected == mode {
        let c = Pos2::new(r.right() - 12.0, r.bottom() - 13.0);
        p.circle_filled(c, 6.0, accent);
        p.line_segment(
            [c + Vec2::new(-3.0, 0.0), c + Vec2::new(-0.5, 2.5)],
            Stroke::new(1.3, Color32::WHITE),
        );
        p.line_segment(
            [c + Vec2::new(-0.5, 2.5), c + Vec2::new(3.5, -2.0)],
            Stroke::new(1.3, Color32::WHITE),
        );
    }
    if response.has_focus() {
        p.rect_stroke(
            r.expand(2.0),
            12.0,
            ui.visuals().selection.stroke,
            StrokeKind::Outside,
        );
    }
}
/// Flat sections use typography and hairlines instead of enclosing cards.
/// Hairline divider, noticeably softer than egui's stock separator stroke.
fn soft_separator(ui: &mut egui::Ui, theme: &Theme) {
    use egui::Sense;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        crate::widgets::divider(theme),
    );
}

fn section(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    theme: &Theme,
    body: impl FnOnce(&mut egui::Ui),
) {
    ui.label(
        egui::RichText::new(title)
            .font(bold(13.0))
            .color(theme.text),
    );
    ui.label(
        egui::RichText::new(description)
            .font(font(11.5))
            .color(theme.text_dim),
    );
    soft_separator(ui, theme);
    ui.add_space(4.0);
    body(ui);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_renders_at_minimum_size_in_both_themes() {
        for dark in [false, true] {
            for sidebar in [false, true] {
                let ctx = egui::Context::default();
                crate::install_fonts(&ctx);
                let mut settings = Settings {
                    appearance: if dark {
                        egui::ThemePreference::Dark
                    } else {
                        egui::ThemePreference::Light
                    },
                    ..Default::default()
                };
                let mut palette = theme::mist();
                settings.apply_theme(&ctx, &mut palette);
                let mut sidebar = sidebar;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(688.0, 448.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let left = if sidebar { 212.0 } else { 8.0 };
                        settings.show(
                            ui,
                            Rect::from_min_max(Pos2::new(left, 44.0), Pos2::new(680.0, 440.0)),
                            &palette,
                            &mut sidebar,
                        );
                    },
                );
                assert!(!output.shapes.is_empty());
                output.textures_delta.clear();
            }
        }
    }
    #[test]
    fn follows_system_and_respects_manual_override() {
        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        let mut palette = theme::mist();
        for system in [egui::Theme::Dark, egui::Theme::Light, egui::Theme::Dark] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    system_theme: Some(system),
                    ..Default::default()
                },
                |_| {},
            );
            output.textures_delta.clear();
            settings.apply_theme(&ctx, &mut palette);
            assert_eq!(palette.dark, system == egui::Theme::Dark);
            assert_eq!(ctx.theme(), system);
        }
        settings.appearance = egui::ThemePreference::Light;
        settings.apply_theme(&ctx, &mut palette);
        assert!(!palette.dark);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        let mut output = ctx.run_ui(
            egui::RawInput {
                system_theme: Some(egui::Theme::Light),
                ..Default::default()
            },
            |_| {},
        );
        output.textures_delta.clear();
        settings.appearance = egui::ThemePreference::Dark;
        settings.apply_theme(&ctx, &mut palette);
        assert!(palette.dark);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
    }
}
