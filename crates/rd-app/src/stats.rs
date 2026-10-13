//! P1 延迟测量（PLAN.md 验收要求）：小环形样本 + 分位数，不引入依赖。

use std::collections::VecDeque;

const CAPACITY: usize = 256;

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

    /// 最近邻秩分位数；`0.5` 为 p50，`0.95` 为 p95。无样本时返回 0。
    pub fn percentile(&self, fraction: f32) -> f32 {
        let sorted = self.sorted();
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((fraction * (sorted.len() as f32 - 1.0)).round() as usize).min(sorted.len() - 1);
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
