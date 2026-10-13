//! P1 单机回环演示（独立于主 UI 的开发工具）：
//! `cargo run -p rd-app --bin rd-loop`
//!
//! 合成捕获 → openh264 编码 → 解码 → egui/wgpu YUV 渲染，屏上叠加延迟统计；
//! 管道线程每 5 秒向 stderr 输出一次 p50/p95 汇总（无头验证用）。

use std::sync::Arc;
use std::time::Duration;

use rd_app::pipeline::{self, LoopFrame, PipelineConfig};
use rd_app::stats::Samples;
use rd_app::video::{VideoShared, YuvPaintCallback};
use std::sync::mpsc::Receiver;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RDX — P1 回环")
            .with_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RDX P1 Loopback",
        options,
        Box::new(|cc| {
            let shared = cc.wgpu_render_state.as_ref().map(VideoShared::new);
            Ok(Box::new(LoopApp::new(cc.egui_ctx.clone(), shared)))
        }),
    )
}

struct LoopApp {
    receiver: Receiver<LoopFrame>,
    shared: Option<Arc<VideoShared>>,
    pending: Option<LoopFrame>,
    e2e_ms: Samples,
    display_fps: f32,
    frames_this_second: u32,
    second_start_us: u64,
}

impl LoopApp {
    fn new(ctx: egui::Context, shared: Option<Arc<VideoShared>>) -> Self {
        let receiver = pipeline::spawn(
            PipelineConfig {
                width: 1280,
                height: 720,
                framerate: 30,
                bitrate_kbps: 4000,
            },
            ctx,
        );
        Self {
            receiver,
            shared,
            pending: None,
            e2e_ms: Samples::default(),
            display_fps: 0.0,
            frames_this_second: 0,
            second_start_us: pipeline::now_us(),
        }
    }

    fn drain_pipeline(&mut self) {
        // 只保留最新帧：UI 慢于管道时直接丢弃过帧，不排队放大延迟。
        let mut latest = self.pending.take();
        while let Ok(frame) = self.receiver.try_recv() {
            latest = Some(frame);
        }
        self.pending = latest;
    }

    fn upload_latest(&mut self) {
        let Some(frame) = self.pending.take() else {
            return;
        };
        let Some(shared) = self.shared.as_ref() else {
            return;
        };
        shared.upload(&frame.frame);
        self.e2e_ms
            .push((pipeline::now_us() - frame.frame.timestamp_us) as f32 / 1000.0);
        self.frames_this_second += 1;
        let now = pipeline::now_us();
        if now - self.second_start_us >= 1_000_000 {
            self.display_fps =
                self.frames_this_second as f32 * 1e6_f32 / (now - self.second_start_us) as f32;
            self.frames_this_second = 0;
            self.second_start_us = now;
            // 与管道侧同节奏的呈现侧汇总，供无头验证。
            eprintln!(
                "rdx display: fps={:.1} e2e p50/p95={:.1}/{:.1} ms",
                self.display_fps,
                self.e2e_ms.percentile(0.5),
                self.e2e_ms.percentile(0.95),
            );
        }
    }
}

impl eframe::App for LoopApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_pipeline();
        self.upload_latest();
        // 兜底重绘：即使管道事件丢失也保持 UI 活着。
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(shared) = self.shared.as_ref() {
                    if let Some((w, h)) = shared.dimensions() {
                        ui.label(format!("{w}×{h}"));
                        ui.separator();
                    }
                }
                ui.label(format!("display {:.0} fps", self.display_fps));
                ui.separator();
                ui.label(format!(
                    "e2e p50/p95 {:.1}/{:.1} ms (n={})",
                    self.e2e_ms.percentile(0.5),
                    self.e2e_ms.percentile(0.95),
                    self.e2e_ms.len()
                ));
            });
            ui.add_space(4.0);
            match self.shared.as_ref() {
                Some(shared) if shared.is_ready() => {
                    let size = ui.available_size();
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                        rect,
                        YuvPaintCallback {
                            shared: Arc::clone(shared),
                        },
                    ));
                }
                _ => {
                    ui.centered_and_justified(|ui| ui.label("等待管道首帧…（编码器初始化中）"));
                }
            }
        });
    }
}
