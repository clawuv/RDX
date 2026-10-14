//! P1 延迟测量（PLAN.md 验收要求）：小环形样本 + 分位数，不引入依赖。

use std::collections::VecDeque;
use std::time::Duration;

const CAPACITY: usize = 256;

/// CPU-side rendering events; drawing does not prove GPU completion or presentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderCounters {
    pub uploads: u64,
    pub draws: u64,
    pub unique_draws: u64,
    pub superseded: u64,
    last_drawn_upload: u64,
}

impl RenderCounters {
    pub fn uploaded(&mut self) {
        if self.uploads > self.last_drawn_upload {
            self.superseded += 1;
        }
        self.uploads += 1;
    }

    pub fn drawn(&mut self) {
        if self.uploads == 0 {
            return;
        }
        self.draws += 1;
        if self.uploads != self.last_drawn_upload {
            self.unique_draws += 1;
            self.last_drawn_upload = self.uploads;
        }
    }
}

#[derive(Default)]
pub struct Samples {
    data: VecDeque<f32>,
}

/// GPU 队列清空（on_submitted_work_done 回调）记账。
///
/// 绘制回调内注册、下一次设备 poll 时触发：样本覆盖注册前已提交的全部
/// 命令（含当帧纹理上传，不含当帧绘制与任何呈现）。不含呈现时刻。
#[derive(Default)]
pub struct GpuDone {
    pending: bool,
    last_upload_ts_us: u64,
    completions: u64,
    register_to_fire_ms: Samples,
    capture_to_gpu_done_ms: Samples,
}

/// 一次性快照，供每秒汇总与限时汇总读取。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuDoneSnapshot {
    pub completions: u64,
    pub register_to_fire_p50_ms: f32,
    pub register_to_fire_p95_ms: f32,
    pub capture_to_gpu_done_p50_ms: f32,
    pub capture_to_gpu_done_p95_ms: f32,
}

impl GpuDone {
    pub fn note_upload(&mut self, capture_ts_us: u64) {
        self.last_upload_ts_us = capture_ts_us;
    }

    /// 绘制回调调用：无在途回调时占用名额，返回注册时刻的上传时间戳快照。
    pub fn begin(&mut self) -> Option<u64> {
        if self.pending {
            return None;
        }
        self.pending = true;
        Some(self.last_upload_ts_us)
    }

    pub fn finish(&mut self, ts_snapshot_us: u64, register_to_fire: Duration, fire_now_us: u64) {
        self.pending = false;
        self.completions += 1;
        self.register_to_fire_ms
            .push(register_to_fire.as_secs_f32() * 1000.0);
        if ts_snapshot_us > 0 {
            self.capture_to_gpu_done_ms
                .push((fire_now_us.saturating_sub(ts_snapshot_us)) as f32 / 1000.0);
        }
    }

    pub fn snapshot(&self) -> GpuDoneSnapshot {
        GpuDoneSnapshot {
            completions: self.completions,
            register_to_fire_p50_ms: self.register_to_fire_ms.percentile(0.5),
            register_to_fire_p95_ms: self.register_to_fire_ms.percentile(0.95),
            capture_to_gpu_done_p50_ms: self.capture_to_gpu_done_ms.percentile(0.5),
            capture_to_gpu_done_p95_ms: self.capture_to_gpu_done_ms.percentile(0.95),
        }
    }
}

impl Samples {
    pub fn push(&mut self, value_ms: f32) {
        if self.data.len() == CAPACITY {
            self.data.pop_front();
        }
        self.data.push_back(value_ms);
    }

    fn sorted(&self) -> Vec<f32> {
        let mut v: Vec<f32> = self.data.iter().copied().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// 最近秩（nearest-rank）分位数；`0.5` 为 p50，`0.95` 为 p95。无样本时返回 0。
    pub fn percentile(&self, fraction: f32) -> f32 {
        let sorted = self.sorted();
        if sorted.is_empty() {
            return 0.0;
        }
        let rank = (fraction * sorted.len() as f32).ceil().max(1.0);
        let idx = (rank as usize - 1).min(sorted.len() - 1);
        sorted[idx]
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_counts_repeats_and_superseded_uploads_separately() {
        let mut counts = RenderCounters::default();
        counts.drawn();
        assert_eq!(counts.draws, 0);
        counts.uploaded();
        counts.uploaded();
        counts.drawn();
        counts.drawn();
        counts.uploaded();
        counts.drawn();
        assert_eq!(counts.uploads, 3);
        assert_eq!(counts.draws, 3);
        assert_eq!(counts.unique_draws, 2);
        assert_eq!(counts.superseded, 1);
    }

    #[test]
    fn percentiles_of_known_sample() {
        let mut s = Samples::default();
        for v in 1..=100 {
            s.push(v as f32);
        }
        assert_eq!(s.percentile(0.5), 50.0);
        assert_eq!(s.percentile(0.95), 95.0);
        assert_eq!(s.percentile(0.0), 1.0);
        assert_eq!(s.percentile(1.0), 100.0);
    }

    #[test]
    fn empty_samples_report_zero() {
        assert_eq!(Samples::default().percentile(0.5), 0.0);
    }

    #[test]
    fn gpu_done_allows_one_in_flight_and_records_both_series() {
        let mut done = GpuDone::default();
        assert_eq!(done.begin(), Some(0)); // 无上传时间戳时也允许注册。
        assert_eq!(done.begin(), None); // 在途时不重复注册。
        done.note_upload(1_000);
        done.finish(0, Duration::from_millis(2), 5_000);
        // 完成后可再次注册，且拿到最近一次上传的时间戳。
        assert_eq!(done.begin(), Some(1_000));
        done.finish(1_000, Duration::from_millis(6), 9_000);

        let snapshot = done.snapshot();
        assert_eq!(snapshot.completions, 2);
        // nearest-rank 分位数：两个样本 [2, 6] 的 p50 是 2.0，不是均值。
        assert_eq!(snapshot.register_to_fire_p50_ms, 2.0);
        assert_eq!(snapshot.register_to_fire_p95_ms, 6.0);
        // 首个样本 ts=0 不计入 capture 序列；仅第二个样本 9ms - 1ms。
        assert_eq!(snapshot.capture_to_gpu_done_p50_ms, 8.0);
        assert_eq!(snapshot.capture_to_gpu_done_p95_ms, 8.0);
    }
}
