//! Account popover: personal session info (fetched once in the background),
//! a share-link action and a demo sign-out toggle. No real auth yet — the
//! signed-in state lives only for the running preview session.
use crate::{
    theme::Theme,
    widgets::{self, font, Icon},
};
use eframe::egui::{self, Align2, Id, Pos2, Rect, Stroke, Vec2};
use std::{sync::Mutex, time::Duration};

pub(crate) const DOWNLOAD_URL: &str = "https://github.com/clawuv/RDX/releases";

#[derive(Clone, Default)]
struct AccountInfo {
    ip: String,
    login_at: String,
    device: String,
    sys: String,
}

struct Fetch {
    started: bool,
    info: Option<AccountInfo>,
}
static FETCH: Mutex<Fetch> = Mutex::new(Fetch {
    started: false,
    info: None,
});

fn shell(cmd: &str) -> String {
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
        String::new()
    }
}

fn or_dash(value: String) -> String {
    if value.is_empty() {
        "—".into()
    } else {
        value
    }
}

fn fetch_account() -> AccountInfo {
    let mut device = shell("scutil --get ComputerName");
    if device.is_empty() {
        device = shell("hostname");
    }
    let device = or_dash(device);
    // Darwin kernel "27.0.0" is shown as the compact "Darwin 2700" form.
    let release = shell("uname -r");
    let sys = if release.is_empty() {
        "—".into()
    } else {
        format!("Darwin {}", release.replace('.', ""))
    };
    let ip = ureq::get("https://api.ipify.org")
        .timeout(Duration::from_secs(8))
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .filter(|s| s.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .unwrap_or_else(|| "—".into());
    AccountInfo {
        ip,
        login_at: or_dash(shell("date '+%Y-%m-%d %H:%M:%S'")),
        device,
        sys,
    }
}

fn info_row(ui: &mut egui::Ui, label: &str, value: &str, theme: &Theme) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), egui::Sense::hover());
    let p = ui.painter();
    p.text(
        Pos2::new(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font(12.0),
        theme.text_dim,
    );
    let vx = rect.left() + 88.0;
    p.text(
        Pos2::new(vx, rect.center().y),
        Align2::LEFT_CENTER,
        widgets::truncate(p, value, &font(12.0), rect.right() - vx),
        font(12.0),
        theme.text,
    );
}

pub fn show(ui: &mut egui::Ui, rect: Rect, theme: &Theme) {
    let ctx = ui.ctx().clone();
    let info = {
        let mut fetch = FETCH.lock().expect("account fetch lock");
        if !fetch.started {
            fetch.started = true;
            std::thread::spawn(move || {
                let info = fetch_account();
                if let Ok(mut f) = FETCH.lock() {
                    f.info = Some(info);
                }
                ctx.request_repaint();
            });
        }
        fetch.info.clone()
    };
    let logged_out_id = Id::new("rdx-logged-out");
    let copied_id = Id::new("rdx-share-copied");
    let logged_out = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(logged_out_id).unwrap_or(false));
    let response = widgets::icon_button_bold(ui, rect, Id::new("account"), Icon::User, theme, true)
        .on_hover_text(if logged_out {
            "未登录"
        } else {
            "账户信息"
        });
    egui::Popup::menu(&response)
        .id(Id::new("account-popup"))
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
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
            if logged_out {
                info_row(ui, "当前状态", "未登录（演示环境）", theme);
                ui.add_space(6.0);
                let (login_rect, _) =
                    ui.allocate_exact_size(Vec2::new(130.0, 30.0), egui::Sense::hover());
                if widgets::button(
                    ui,
                    login_rect,
                    Id::new("row-login"),
                    "登录",
                    Some(Icon::User),
                    widgets::BtnStyle::Subtle,
                    theme,
                )
                .clicked()
                {
                    ui.ctx().data_mut(|d| d.insert_temp(logged_out_id, false));
                    ui.close();
                }
                return;
            }
            match info {
                Some(info) => {
                    info_row(ui, "最后登录IP", &info.ip, theme);
                    info_row(ui, "最后登录时间", &info.login_at, theme);
                    info_row(ui, "设备名", &info.device, theme);
                    info_row(ui, "系统版本", &info.sys, theme);
                }
                None => info_row(ui, "登录信息", "获取中…", theme),
            }
            ui.add_space(6.0);
            let (line, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter()
                .hline(line.x_range(), line.center().y, widgets::divider(theme));
            ui.add_space(4.0);
            let copied = ui
                .ctx()
                .data(|d| d.get_temp::<bool>(copied_id).unwrap_or(false));
            ui.horizontal(|ui| {
                let (share_rect, _) =
                    ui.allocate_exact_size(Vec2::new(130.0, 30.0), egui::Sense::hover());
                if widgets::button(
                    ui,
                    share_rect,
                    Id::new("row-share"),
                    if copied { "已复制" } else { "分享链接" },
                    Some(Icon::LinkOut),
                    widgets::BtnStyle::Subtle,
                    theme,
                )
                .clicked()
                {
                    ui.ctx().copy_text(DOWNLOAD_URL.into());
                    ui.ctx().data_mut(|d| d.insert_temp(copied_id, true));
                }
                let (logout_rect, _) =
                    ui.allocate_exact_size(Vec2::new(130.0, 30.0), egui::Sense::hover());
                if widgets::button(
                    ui,
                    logout_rect,
                    Id::new("row-logout"),
                    "退出登录",
                    Some(Icon::Logout),
                    widgets::BtnStyle::Subtle,
                    theme,
                )
                .clicked()
                {
                    ui.ctx().data_mut(|d| d.insert_temp(logged_out_id, true));
                    ui.ctx().data_mut(|d| d.remove::<bool>(copied_id));
                    ui.close();
                }
            });
        });
}
