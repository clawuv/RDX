//! 受控动态测试画面（真屏捕获验收用）：
//! `cargo run -p rd-app --example pattern_window --locked`
//!
//! 图案完全由帧序号决定：逐帧翻转的棋盘底、匀速扫过的白条、指纹角标与
//! 大号帧号，按固定节拍出图。全屏放在被采集的显示器上，真屏回环即得到
//! 可复现的动态素材（PLAN.md P1“固定动态测试画面”验收前置）。
//! 本工具不采集、不保存、不发送任何屏幕像素；F 键切换全屏。
//!
//! 可选环境变量：
//! - `RDX_PATTERN_FPS=1..120`（默认 30）
//! - `RDX_PATTERN_SECONDS=1..120`：限时运行后输出汇总并正常关闭
//! - `RDX_PATTERN_FULLSCREEN=1`：启动即全屏
//! - `RDX_PATTERN_TRACE=1`：逐帧输出 `pattern-trace <frame> <unix_us>`，
//!   供 present_probe 跨进程对齐（时钟为系统墙钟）
//! - `RDX_PATTERN_HIDE_AFTER=<秒>`：到时隐藏窗口（Visible(false)），用于前台/
//!   遮挡对比；macOS 全屏窗口对 Minimized 命令无响应，实测改用隐藏

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() -> eframe::Result<()> {
    let fps: u32 = env_in_range("RDX_PATTERN_FPS", 1..=120).unwrap_or(30);
    let seconds: Option<u64> = env_in_range("RDX_PATTERN_SECONDS", 1..=120);
    let hide_after: Option<u64> = env_in_range("RDX_PATTERN_HIDE_AFTER", 1..=120);
    let fullscreen = std::env::var("RDX_PATTERN_FULLSCREEN").as_deref() == Ok("1");
    let trace = std::env::var("RDX_PATTERN_TRACE").as_deref() == Ok("1");
    let options = eframe::NativeOptions {
        wgpu_options: rd_app::wgpu_configuration(),
        viewport: egui::ViewportBuilder::default()
            .with_title("RDX — 受控动态测试画面")
            .with_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RDX Pattern Window",
        options,
        Box::new(move |cc| {
            if fullscreen {
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            }
            Ok(Box::new(PatternApp::new(
                fps, seconds, hide_after, fullscreen, trace,
            )))
        }),
    )
}

fn env_in_range<T>(name: &str, range: std::ops::RangeInclusive<T>) -> Option<T>
where
    T: std::str::FromStr + PartialOrd + Copy,
{
    let value = std::env::var(name).ok()?;
    match value.parse() {
        Ok(parsed) if range.contains(&parsed) => Some(parsed),
        _ => {
            eprintln!("{name} out of range or invalid");
            None
        }
    }
}

fn unix_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

struct PatternApp {
    fps: u32,
    duration: Option<Duration>,
    hide_after: Option<Duration>,
    started: Instant,
    next_frame_at: Instant,
    hidden: bool,
    trace: bool,
    frame: u64,
    reported_second: u64,
    summary_reported: bool,
    fullscreen: bool,
}

impl PatternApp {
    fn new(
        fps: u32,
        seconds: Option<u64>,
        hide_after: Option<u64>,
        fullscreen: bool,
        trace: bool,
    ) -> Self {
        let interval = Duration::from_secs_f64(1.0 / fps as f64);
        Self {
            fps,
            duration: seconds.map(Duration::from_secs),
            hide_after: hide_after.map(Duration::from_secs),
            started: Instant::now(),
            next_frame_at: Instant::now() + interval,
            hidden: false,
            trace,
            frame: 0,
            reported_second: 0,
            summary_reported: false,
            fullscreen,
        }
    }
}

impl eframe::App for PatternApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self
            .duration
            .is_some_and(|limit| self.started.elapsed() >= limit)
        {
            if !self.summary_reported {
                eprintln!(
                    "pattern summary: frames={} wall={:.2}s avg={:.2}/s target={}fps",
                    self.frame,
                    self.started.elapsed().as_secs_f64(),
                    self.frame as f64 / self.started.elapsed().as_secs_f64().max(f64::EPSILON),
                    self.fps,
                );
                self.summary_reported = true;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.hidden
            && self
                .hide_after
                .is_some_and(|delay| self.started.elapsed() >= delay)
        {
            self.hidden = true;
            eprintln!(
                "pattern hidden at wall={:.2}s frame={}",
                self.started.elapsed().as_secs_f64(),
                self.frame
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        // 帧号按截止时间推进、错过重基：图案节拍与 UI 重绘率（vsync）解耦。
        let now = Instant::now();
        if now >= self.next_frame_at {
            self.frame += 1;
            let interval = Duration::from_secs_f64(1.0 / self.fps as f64);
            let scheduled = self.next_frame_at + interval;
            self.next_frame_at = if scheduled <= now {
                now + interval
            } else {
                scheduled
            };
            if self.trace {
                eprintln!("pattern-trace {} {}", self.frame, unix_us());
            }
            let elapsed = self.started.elapsed().as_secs();
            if elapsed > self.reported_second {
                self.reported_second = elapsed;
                eprintln!("pattern: frame={} wall={elapsed:.0}s", self.frame);
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        ctx.request_repaint_after(
            self.next_frame_at
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        );
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let rect = ui.max_rect();
        let painter = ui.painter_at(rect);
        let ppp = ui.ctx().pixels_per_point();
        rd_app::pattern::draw(&painter, rect, ppp, self.frame, self.fps);
    }
}
