//! Frame time statistics of a played session, for `play --stats`.

/// The duration of every displayed frame.
// ponytail: keeps every frame (8 bytes each, about 4 MB per hour at 144 frames per second);
// switch to a histogram if sessions of many hours need statistics.
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    seconds: Vec<f64>,
}

impl FrameStats {
    /// Adds a displayed frame that took `frame_seconds`.
    pub fn record(&mut self, frame_seconds: f64) {
        self.seconds.push(frame_seconds);
    }

    /// "frames N, frame time p50 X ms, p95 Y ms, p99 Z ms" (nearest-rank percentiles), or
    /// "frames 0" before any frame.
    pub fn summary(&self) -> String {
        if self.seconds.is_empty() {
            return "frames 0".to_string();
        }
        let mut sorted = self.seconds.clone();
        sorted.sort_by(f64::total_cmp);
        let milliseconds = |percent: usize| {
            let rank = (percent * sorted.len()).div_ceil(100);
            sorted[rank.saturating_sub(1)] * 1000.0
        };
        format!(
            "frames {}, frame time p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms",
            sorted.len(),
            milliseconds(50),
            milliseconds(95),
            milliseconds(99)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_of_known_frames() {
        let mut stats = FrameStats::default();
        assert_eq!(stats.summary(), "frames 0");
        // 1 to 100 ms, out of order.
        for ms in (1..=100).rev() {
            stats.record(f64::from(ms) / 1000.0);
        }
        assert_eq!(
            stats.summary(),
            "frames 100, frame time p50 50.00 ms, p95 95.00 ms, p99 99.00 ms"
        );
    }
}
