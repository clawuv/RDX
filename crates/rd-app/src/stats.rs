//! P1 延迟测量（PLAN.md 验收要求）：小环形样本 + 分位数，不引入依赖。

use std::collections::VecDeque;

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
}
