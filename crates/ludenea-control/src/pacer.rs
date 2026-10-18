//! Real-time pacing for play: turns elapsed wall time into whole ticks. The pacer never
//! reads a clock; the caller passes the time that passed.

use std::collections::VecDeque;

/// Most ticks one `advance` call returns at speed 1 (scaled by the speed), so a slow
/// simulation falls behind real time instead of spiralling.
pub const MAX_TICKS_PER_FRAME: u32 = 4;

/// Tolerance for float rounding in the tick accumulator (60 steps of 1/60 s must give 60 ticks).
const EPSILON: f64 = 1e-9;

/// Turns elapsed time into whole simulation ticks at a speed multiplier.
pub struct Pacer {
    tick_rate: u32,
    speed: f32,
    /// Fractional ticks owed.
    owed: f64,
    /// Recent frames as (elapsed seconds, ticks run), covering about the last second.
    history: VecDeque<(f64, u32)>,
    window: f64,
}

impl Pacer {
    pub fn new(tick_rate: u32) -> Self {
        Self {
            tick_rate,
            speed: 1.0,
            owed: 0.0,
            history: VecDeque::new(),
            window: 0.0,
        }
    }

    /// Sets the speed multiplier, clamped to `[0.1, 8.0]`.
    #[allow(clippy::disallowed_methods)]
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.1, 8.0);
    }

    /// The ticks due after `elapsed_seconds` more time, at most `MAX_TICKS_PER_FRAME` times
    /// the speed (rounded up). Debt beyond the cap is dropped: play slows down but never
    /// skips ticks. A negative, infinite, or NaN time counts as none.
    pub fn advance(&mut self, elapsed_seconds: f64) -> u32 {
        let elapsed_seconds = if elapsed_seconds.is_finite() && elapsed_seconds > 0.0 {
            elapsed_seconds
        } else {
            0.0
        };
        self.owed += elapsed_seconds * f64::from(self.speed) * f64::from(self.tick_rate);
        let due = (self.owed + EPSILON).floor();
        let cap = (f64::from(MAX_TICKS_PER_FRAME) * f64::from(self.speed)).ceil();
        let ticks = if due >= cap {
            self.owed = 0.0;
            cap as u32
        } else {
            self.owed -= due;
            due as u32
        };
        // A frame that ran ticks is kept even without elapsed time: `not_run` corrects it.
        if elapsed_seconds > 0.0 || ticks > 0 {
            self.record(elapsed_seconds, ticks);
        }
        ticks
    }

    /// Gives back `ticks` of the last `advance` that did not run: they stay owed and do not
    /// count in the effective rate.
    pub fn not_run(&mut self, ticks: u32) {
        if let Some((_, ran)) = self.history.back_mut() {
            let ticks = ticks.min(*ran);
            *ran -= ticks;
            self.owed += f64::from(ticks);
        }
    }

    /// Forgets the debt and the rate history, when play stops.
    pub fn stop(&mut self) {
        self.owed = 0.0;
        self.history.clear();
        self.window = 0.0;
    }

    /// Ticks per second of passed time over about the last second.
    pub fn effective_rate(&self) -> f32 {
        if self.window <= 0.0 {
            return 0.0;
        }
        let ticks: u32 = self.history.iter().map(|(_, ticks)| ticks).sum();
        (f64::from(ticks) / self.window) as f32
    }

    fn record(&mut self, elapsed_seconds: f64, ticks: u32) {
        self.history.push_back((elapsed_seconds, ticks));
        self.window += elapsed_seconds;
        while let Some(&(oldest, _)) = self.history.front() {
            if self.window - oldest < 1.0 {
                break;
            }
            self.window -= oldest;
            self.history.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_or_not_a_number_elapsed_time_counts_as_none() {
        let mut pacer = Pacer::new(60);
        for elapsed in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(pacer.advance(elapsed), 0, "{elapsed}");
        }
        let ticks: u32 = (0..60).map(|_| pacer.advance(1.0 / 60.0)).sum();
        assert_eq!(ticks, 60);
        assert_eq!(pacer.effective_rate(), 60.0);
    }

    #[test]
    fn frames_without_elapsed_time_are_not_kept() {
        let mut pacer = Pacer::new(60);
        for _ in 0..1_000 {
            pacer.advance(0.0);
        }
        assert!(pacer.history.is_empty());
    }

    #[test]
    fn stopping_resets_the_rate_and_the_debt() {
        let mut pacer = Pacer::new(60);
        assert_eq!(pacer.advance(2.0 / 60.0), 2);
        assert_eq!(pacer.advance(0.5 / 60.0), 0);
        pacer.stop();
        assert_eq!(pacer.effective_rate(), 0.0);
        assert_eq!(
            pacer.advance(0.75 / 60.0),
            0,
            "the half tick owed was dropped"
        );
    }

    #[test]
    fn ticks_not_run_stay_owed_and_do_not_count() {
        let mut pacer = Pacer::new(60);
        assert_eq!(pacer.advance(2.0 / 60.0), 2);
        pacer.not_run(2);
        assert_eq!(pacer.effective_rate(), 0.0);
        assert_eq!(pacer.advance(1.0 / 60.0), 3);
    }

    /// Ticks still owed run on a frame without elapsed time; giving some back must correct
    /// that frame, not an earlier one.
    #[test]
    fn ticks_not_run_on_a_frame_without_elapsed_time_stay_owed() {
        let mut pacer = Pacer::new(60);
        assert_eq!(pacer.advance(2.0 / 60.0), 2);
        pacer.not_run(2);
        assert_eq!(pacer.advance(0.0), 2);
        pacer.not_run(1);
        assert_eq!(pacer.advance(0.0), 1, "the tick given back is owed");
        // Two ticks ran in all, over two sixtieths of a second.
        assert_eq!(pacer.effective_rate(), 60.0);
    }

    #[test]
    fn pacer_runs_one_tick_per_sixtieth_second() {
        let mut pacer = Pacer::new(60);
        let ticks: u32 = (0..60).map(|_| pacer.advance(1.0 / 60.0)).sum();
        assert_eq!(ticks, 60);
        assert_eq!(pacer.advance(0.0), 0);
    }

    #[test]
    fn pacer_caps_ticks_and_drops_debt() {
        let mut pacer = Pacer::new(60);
        assert_eq!(pacer.advance(1.0), MAX_TICKS_PER_FRAME);
        assert_eq!(pacer.advance(0.0), 0);
        assert_eq!(pacer.effective_rate(), MAX_TICKS_PER_FRAME as f32);
    }

    #[test]
    fn speed_two_doubles_ticks() {
        let mut pacer = Pacer::new(60);
        pacer.set_speed(2.0);
        let ticks: u32 = (0..60).map(|_| pacer.advance(1.0 / 60.0)).sum();
        assert_eq!(ticks, 120);
        pacer.set_speed(100.0);
        assert_eq!(pacer.advance(1.0), 8 * MAX_TICKS_PER_FRAME);
    }

    #[test]
    fn every_speed_up_to_eight_runs_in_real_time() {
        for speed in [4.0, 6.0, 8.0] {
            let mut pacer = Pacer::new(60);
            pacer.set_speed(speed);
            let ticks: u32 = (0..60).map(|_| pacer.advance(1.0 / 60.0)).sum();
            assert_eq!(ticks, (60.0 * speed) as u32, "at {speed}x");
        }
        let mut slow = Pacer::new(60);
        slow.set_speed(0.1);
        assert_eq!(slow.advance(1.0), 1, "the cap never drops below one tick");
    }

    #[test]
    fn speed_is_clamped_to_one_tenth_from_below() {
        let mut pacer = Pacer::new(60);
        pacer.set_speed(0.01);
        let ticks: u32 = (0..60).map(|_| pacer.advance(1.0 / 60.0)).sum();
        assert_eq!(ticks, 6);
    }
}
