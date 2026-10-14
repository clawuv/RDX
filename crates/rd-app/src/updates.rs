//! Non-blocking release checks. Merely checking never installs or opens a URL.
use crate::{
    share::DOWNLOAD_URL,
    theme::Theme,
    widgets::{self, Icon},
};
use eframe::egui::{self, Color32, Id, Pos2, Rect, RichText, Stroke, Vec2};
use std::{
    sync::mpsc::{self, Receiver},
    time::Duration,
};

#[derive(Debug, PartialEq)]
enum Release {
    Unpublished,
    Current,
    New(String),
}

#[derive(Default)]
pub struct Updates {
    started: bool,
    pending: Option<Receiver<Result<Release, String>>>,
    available: Option<String>,
    message: String,
}

fn classify(value: &serde_json::Value, current: &str) -> Result<Release, String> {
    if value["draft"].as_bool() == Some(true) || value["prerelease"].as_bool() == Some(true) {
        return Err("发布页返回了非正式版本".into());
    }
    let tag = value["tag_name"].as_str().ok_or("版本信息缺失")?;
    let remote = semver::Version::parse(tag.trim_start_matches(['v', 'V']))
        .map_err(|_| "无法识别发布版本号")?;
    let local = semver::Version::parse(current).map_err(|_| "无法识别当前版本号")?;
    Ok(if remote > local {
        Release::New(tag.to_owned())
    } else {
        Release::Current
    })
}

fn check() -> Result<Release, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();
    match agent
        .get("https://api.github.com/repos/clawuv/RDX/releases/latest")
        .set("User-Agent", concat!("RDX/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(response) => {
            let value: serde_json::Value = response.into_json().map_err(|_| "更新信息解析失败")?;
            classify(&value, env!("CARGO_PKG_VERSION"))
        }
        Err(ureq::Error::Status(404, _)) => Ok(Release::Unpublished),
        Err(ureq::Error::Status(403 | 429, _)) => Err("请求受限，请稍后重试".into()),
        Err(_) => Err("检查失败，请检查网络后重试".into()),
    }
}

impl Updates {
    fn start(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() {
            return;
        }
        self.started = true;
        self.message = "正在检查更新…".into();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(check());
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.start(ctx);
        }
        let result = self.pending.as_ref().map(|rx| rx.try_recv());
        match result {
            Some(Ok(result)) => {
                self.pending = None;
                match result {
                    Ok(Release::New(tag)) => {
                        self.message = format!("发现新版本 {tag}");
                        self.available = Some(tag);
                    }
                    Ok(status) => {
                        self.available = None;
                        self.message = if status == Release::Unpublished {
                            "暂无可访问的正式版本".into()
                        } else {
                            "当前已是最新版本".into()
                        };
                    }
                    Err(error) => self.message = error,
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.pending = None;
                self.message = "检查中断，请重试".into();
            }
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, footer: Rect, theme: &Theme) {
        let mut icon_theme = theme.clone();
        if self.available.is_some() {
            let blue = Color32::from_rgb(59, 130, 246);
            icon_theme.text = blue;
            icon_theme.text_dim = blue;
        }
        let response = widgets::icon_button_bold(
            ui,
            rect,
            Id::new("software-update"),
            Icon::Download,
            &icon_theme,
            true,
        )
        .on_hover_text(format!("软件更新 · {}\n点击重新检查", self.message));
        if response.clicked() {
            self.start(ui.ctx());
        }
        egui::Popup::menu(&response)
            .anchor(footer)
            .align(egui::emath::RectAlign::TOP_START)
            .align_alternatives(&[])
            .gap(12.0)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .width(footer.width())
            .frame(
                egui::Frame::popup(ui.style())
                    .fill(theme.surface)
                    .stroke(Stroke::new(1.0, theme.stroke))
                    .corner_radius(8.0)
                    .inner_margin(12.0),
            )
            .show(|ui| {
                // Keep the complete frame inside the sidebar, including padding.
                ui.set_width((footer.width() - 26.0).max(80.0));
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.horizontal_top(|ui| {
                    let title = ui.label(RichText::new("软件更新").color(theme.text));
                    let close = Rect::from_center_size(
                        Pos2::new(ui.max_rect().right() - 11.0, title.rect.center().y),
                        Vec2::splat(22.0),
                    );
                    let btn = widgets::icon_button(
                        ui,
                        close,
                        Id::new("update-close"),
                        Icon::Close,
                        theme,
                        true,
                    );
                    if btn.clicked() {
                        ui.close();
                    }
                });
                ui.label(
                    RichText::new(format!("当前版本 {}", env!("CARGO_PKG_VERSION")))
                        .color(theme.text_dim),
                );
                ui.label(RichText::new(&self.message).color(theme.text));
                if self.pending.is_some() {
                    ui.spinner();
                }
                if self.available.is_some() && ui.button("前往下载新版本").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(DOWNLOAD_URL));
                    ui.close();
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_version_survives_failed_recheck_and_clears_when_current() {
        let ctx = egui::Context::default();
        let mut updates = Updates {
            started: true,
            ..Default::default()
        };
        for result in [
            Ok(Release::New("v0.2.0".into())),
            Err("offline".into()),
            Ok(Release::Current),
        ] {
            let should_highlight = !matches!(result, Ok(Release::Current));
            let (tx, rx) = mpsc::channel();
            tx.send(result).unwrap();
            updates.pending = Some(rx);
            updates.poll(&ctx);
            assert_eq!(updates.available.is_some(), should_highlight);
            assert!(updates.pending.is_none());
        }
    }

    #[test]
    fn compares_numeric_versions() {
        for (tag, newer) in [
            ("v0.2.0", true),
            ("v0.10.0", true),
            ("0.1.0", false),
            ("0.0.9", false),
        ] {
            let result = classify(&serde_json::json!({"tag_name":tag}), "0.1.0").unwrap();
            assert_eq!(matches!(result, Release::New(_)), newer);
        }
        assert!(classify(&serde_json::json!({"tag_name":"nightly"}), "0.1.0").is_err());
        assert!(classify(
            &serde_json::json!({"tag_name":"1.0.0", "prerelease":true}),
            "0.1.0"
        )
        .is_err());
    }
}
