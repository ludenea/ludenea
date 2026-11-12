//! The spike's one clock and the statistics its durations are reported with.
//! Throwaway, never product code.

use std::sync::OnceLock;
use std::time::Instant;

use crate::json::Json;

static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Nanoseconds on the process's monotonic clock since its first reading.
pub fn now_ns() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// The value at `fraction` of the sorted samples (nearest rank), or zero for
/// no sample.
pub fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Durations in nanoseconds, summarized in microseconds.
#[derive(Default, Clone)]
pub struct Durations {
    samples: Vec<u64>,
}

impl Durations {
    pub fn add(&mut self, nanoseconds: u64) {
        self.samples.push(nanoseconds);
    }

    pub fn sorted(&self) -> Vec<u64> {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        sorted
    }

    /// Count, median, 99th percentile and largest, in microseconds.
    pub fn summary(&self) -> Json {
        let sorted = self.sorted();
        let micro = |nanoseconds: u64| Json::Float(nanoseconds as f64 / 1000.0);
        Json::object()
            .with("count", sorted.len())
            .with("median_us", micro(percentile(&sorted, 0.5)))
            .with("p99_us", micro(percentile(&sorted, 0.99)))
            .with("max_us", micro(sorted.last().copied().unwrap_or(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn the_percentile_is_the_nearest_rank() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&sorted, 0.5), 50);
        assert_eq!(percentile(&sorted, 0.99), 99);
        assert_eq!(percentile(&sorted, 1.0), 100);
        assert_eq!(percentile(&[], 0.99), 0);
        assert_eq!(percentile(&[7], 0.99), 7);
    }
}
