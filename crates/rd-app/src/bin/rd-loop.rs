//! P1 单机回环演示（独立于主 UI 的开发工具）：
//! `cargo run -p rd-app --bin rd-loop`
//!
//! 合成捕获 → openh264 编码 → 解码 → egui/wgpu YUV 渲染，屏上叠加延迟统计；
//! 管道线程每 5 秒向 stderr 输出一次 p50/p95 汇总（无头验证用）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use rd_app::pipeline::{self, LoopFrame, PipelineConfig};
use rd_app::stats::{GpuDoneSnapshot, RenderCounters, Samples};
use rd_app::video::{VideoShared, YuvPaintCallback};
use tokio::sync::watch;

fn main() -> eframe::Result<()> {
    let duration = match std::env::var("RDX_LOOP_SECONDS") {
        Ok(value) => match value.parse::<u64>() {
            Ok(seconds @ 1..=120) => Some(Duration::from_secs(seconds)),
            _ => {
                eprintln!("RDX_LOOP_SECONDS must be 1..120");
                return Ok(());
            }
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => {
            eprintln!("invalid RDX_LOOP_SECONDS");
            return Ok(());
        }
    };
    let hide_after = match std::env::var("RDX_LOOP_HIDE_AFTER") {
        Ok(value) => match value.parse::<u64>() {
            Ok(seconds @ 1..=120) => Some(Duration::from_secs(seconds)),
            _ => {
                eprintln!("RDX_LOOP_HIDE_AFTER must be 1..120");
                return Ok(());
            }
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => {
            eprintln!("invalid RDX_LOOP_HIDE_AFTER");
            return Ok(());
        }
    };
    let options = eframe::NativeOptions {
        wgpu_options: rd_app::wgpu_configuration(),
        viewport: egui::ViewportBuilder::default()
            .with_title("RDX — P1 回环")
            .with_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RDX P1 Loopback",
        options,
        Box::new(move |cc| {
            let shared = cc.wgpu_render_state.as_ref().map(VideoShared::new);
            let mut app = LoopApp::new(cc.egui_ctx.clone(), shared);
            app.duration = duration;
            app.hide_after = hide_after;
            Ok(Box::new(app))
        }),
    )
}

struct LoopApp {
    started: Instant,
    duration: Option<Duration>,
    hide_after: Option<Duration>,
    hidden: bool,
    summary_reported: bool,
    pipeline_stopped: bool,
    receiver: watch::Receiver<Option<LoopFrame>>,
    shared: Option<Arc<VideoShared>>,
    pending: Option<LoopFrame>,
    e2e_ms: Samples,
    display_fps: f32,
    frames_this_second: u32,
    second_start_us: u64,
    previous_counters: RenderCounters,
    logic_ticks: u32,
    unique_draw_fps: f32,
}

impl LoopApp {
    fn new(ctx: egui::Context, shared: Option<Arc<VideoShared>>) -> Self {
        let receiver = pipeline::spawn(
            PipelineConfig {
                width: 1920,
                height: 1080,
                framerate: 30,
                bitrate_kbps: 8000,
            },
            ctx,
        );
        Self {
            started: Instant::now(),
            duration: None,
            hide_after: None,
            hidden: false,
            summary_reported: false,
            pipeline_stopped: false,
            receiver,
            shared,
            pending: None,
            e2e_ms: Samples::default(),
            display_fps: 0.0,
            frames_this_second: 0,
            second_start_us: pipeline::now_us(),
            previous_counters: RenderCounters::default(),
            logic_ticks: 0,
            unique_draw_fps: 0.0,
        }
    }

    fn drain_pipeline(&mut self) {
        if self.pipeline_stopped {
            return;
        }
        // 只保留最新帧：UI 慢于管道时直接丢弃过帧，不排队放大延迟。
        self.pipeline_stopped = self.receiver.has_changed().is_err();
        let latest = self.receiver.borrow_and_update();
        if latest.has_changed() {
            self.pending = latest.clone();
            if self.pending.is_none() {
                if let Some(shared) = &self.shared {
                    shared.clear_frame();
                }
            }
        }
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
    }

    fn report_rates(&mut self) {
        self.report_rates_at(pipeline::now_us());
    }

    fn report_rates_at(&mut self, now: u64) {
        if now - self.second_start_us >= 1_000_000 {
            let (counts, gpu) = match self.shared.as_ref() {
                Some(shared) => (shared.counters(), shared.gpu_done()),
                None => (RenderCounters::default(), GpuDoneSnapshot::default()),
            };
            let seconds = (now - self.second_start_us) as f32 / 1e6;
            self.unique_draw_fps =
                (counts.unique_draws - self.previous_counters.unique_draws) as f32 / seconds;
            eprintln!(
                "rdx render: logic={:.1}/s draw={:.1}/s unique_draw={:.1}/s superseded={} (CPU callbacks; not presentation)",
                self.logic_ticks as f32 / seconds,
                (counts.draws - self.previous_counters.draws) as f32 / seconds,
                self.unique_draw_fps,
                counts.superseded - self.previous_counters.superseded,
            );
            eprintln!(
                "rdx gpu-done: completions={} register→fire p50/p95={:.2}/{:.2}ms capture→gpu_done p50/p95={:.2}/{:.2}ms (uploads+prior draws; not presentation)",
                gpu.completions,
                gpu.register_to_fire_p50_ms,
                gpu.register_to_fire_p95_ms,
                gpu.capture_to_gpu_done_p50_ms,
                gpu.capture_to_gpu_done_p95_ms,
            );
            self.previous_counters = counts;
            self.logic_ticks = 0;
            self.display_fps =
                self.frames_this_second as f32 * 1e6_f32 / (now - self.second_start_us) as f32;
            self.frames_this_second = 0;
            self.second_start_us = now;
            // Report even without new uploads; samples remain a rolling window.
            eprintln!(
                "rdx upload: fps={:.1} capture_to_upload_submit p50/p95={:.1}/{:.1} ms",
                self.display_fps,
                self.e2e_ms.percentile(0.5),
                self.e2e_ms.percentile(0.95),
            );
        }
    }
}

impl eframe::App for LoopApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self
            .duration
            .is_some_and(|limit| self.started.elapsed() >= limit)
        {
            if !self.summary_reported {
                let (counts, gpu) = match self.shared.as_ref() {
                    Some(shared) => (shared.counters(), shared.gpu_done()),
                    None => (RenderCounters::default(), GpuDoneSnapshot::default()),
                };
                eprintln!("rdx bounded summary: elapsed={:.2}s uploads={} draws={} unique_draws={} superseded={} pipeline_stopped={} rolling_upload_p50/p95={:.2}/{:.2}ms gpu_done={} register→fire_p50/p95={:.2}/{:.2}ms capture→gpu_done_p50/p95={:.2}/{:.2}ms (uploads+prior draws; not presentation)",
                    self.started.elapsed().as_secs_f64(), counts.uploads, counts.draws, counts.unique_draws, counts.superseded, self.pipeline_stopped, self.e2e_ms.percentile(0.5), self.e2e_ms.percentile(0.95), gpu.completions, gpu.register_to_fire_p50_ms, gpu.register_to_fire_p95_ms, gpu.capture_to_gpu_done_p50_ms, gpu.capture_to_gpu_done_p95_ms);
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
                "rdx: window hidden at {:.2}s",
                self.started.elapsed().as_secs_f64()
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.logic_ticks += 1;
        self.drain_pipeline();
        self.upload_latest();
        self.report_rates();
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
                ui.label(format!(
                    "upload {:.0}/s · unique draw {:.0}/s",
                    self.display_fps, self.unique_draw_fps
                ));
                ui.separator();
                ui.label(format!(
                    "capture → upload submit p50/p95 {:.1}/{:.1} ms (n={})",
                    self.e2e_ms.percentile(0.5),
                    self.e2e_ms.percentile(0.95),
                    self.e2e_ms.len()
                ));
            });
            ui.add_space(4.0);
            ui.label("GPU queue-drain via on_submitted_work_done; presentation not measured.");
            if self.pipeline_stopped {
                ui.colored_label(
                    egui::Color32::RED,
                    "Pipeline stopped; showing the last received frame.",
                );
            }
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
                    ui.centered_and_justified(|ui| ui.label("等待视频帧…（初始化或显示器恢复中）"));
                }
            }
        });
    }
}

#[cfg(test)]
mod acceptance_tests {
    use super::*;

    fn app(receiver: watch::Receiver<Option<LoopFrame>>) -> LoopApp {
        LoopApp {
            started: Instant::now(),
            duration: None,
            hide_after: None,
            hidden: false,
            summary_reported: false,
            pipeline_stopped: false,
            receiver,
            shared: None,
            pending: None,
            e2e_ms: Samples::default(),
            display_fps: 30.0,
            frames_this_second: 0,
            second_start_us: 0,
            previous_counters: RenderCounters::default(),
            logic_ticks: 10,
            unique_draw_fps: 30.0,
        }
    }

    #[test]
    fn idle_interval_resets_rates_and_retains_latency_samples() {
        let (_tx, rx) = watch::channel(None);
        let mut app = app(rx);
        app.e2e_ms.push(24.0);
        app.report_rates_at(1_000_000);
        assert_eq!(app.display_fps, 0.0);
        assert_eq!(app.unique_draw_fps, 0.0);
        assert_eq!(app.e2e_ms.percentile(0.95), 24.0);
        assert_eq!(app.logic_ticks, 0);
    }

    #[test]
    fn closed_pipeline_delivers_final_frame_once() {
        let (tx, rx) = watch::channel(None);
        tx.send(Some(LoopFrame {
            frame: rd_codec::DecodedFrame {
                width: 2,
                height: 2,
                format: rd_capture::PixelFormat::I420,
                data: bytes::Bytes::from_static(&[16, 16, 16, 16, 128, 128]),
                timestamp_us: 42,
            },
            encode_ms: 0.0,
            decode_ms: 0.0,
        }))
        .unwrap();
        drop(tx);
        let mut app = app(rx);
        app.drain_pipeline();
        assert_eq!(app.pending.take().unwrap().frame.timestamp_us, 42);
        assert!(app.receiver.has_changed().is_err());
        app.drain_pipeline();
        assert!(app.pending.is_none());
    }

    #[test]
    fn closing_after_consumption_does_not_repeat_the_frame() {
        let (tx, rx) = watch::channel(None);
        let mut app = app(rx);
        tx.send(Some(LoopFrame {
            frame: rd_codec::DecodedFrame {
                width: 2,
                height: 2,
                format: rd_capture::PixelFormat::I420,
                data: bytes::Bytes::from_static(&[16, 16, 16, 16, 128, 128]),
                timestamp_us: 7,
            },
            encode_ms: 0.0,
            decode_ms: 0.0,
        }))
        .unwrap();
        app.drain_pipeline();
        assert_eq!(app.pending.take().unwrap().frame.timestamp_us, 7);
        drop(tx);
        app.drain_pipeline();
        assert!(app.pipeline_stopped);
        assert!(app.pending.is_none());
    }
}
