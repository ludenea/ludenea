//! The worst-case rollback frame, measured the same way for every example game: load the
//! state the rollback window back, then replay a window plus one ticks saving the state after each.
//!
//! Shared by the rollback benchmarks of the example games, which include it with
//! `#[path = "../../../rollback_bench.rs"] mod rollback_bench;`. It is not part of any
//! published crate.

use std::time::Instant;

use ludenea::app::{SavedState, Simulation};
use ludenea::net::MatchRules;

/// What a rollback frame must fit in: half a 60 Hz frame, in microseconds, so the rest of
/// the frame stays for the game's own work.
pub const BUDGET_MICROS: u128 = 8_333;

/// The saved states a session keeps: the window, the current tick, and one to spare.
const SPARE: u64 = 2;

/// The rollback window of the default rules: a frame loads the state this many ticks back.
fn window() -> u64 {
    u64::from(MatchRules::default().rollback_window)
}

fn ring_len() -> u64 {
    window() + SPARE
}

#[expect(
    clippy::disallowed_methods,
    reason = "benchmark timing, never feeds the simulation"
)]
fn now() -> Instant {
    Instant::now()
}

/// The distribution of the measured frames, in microseconds.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub frames: usize,
    pub mean: u128,
    pub p99: u128,
    pub worst: u128,
}

impl Report {
    /// The distribution of `frames`, or `None` when there is none.
    pub fn new(mut frames: Vec<u128>) -> Option<Self> {
        frames.sort_unstable();
        let count = frames.len();
        let worst = *frames.last()?;
        Some(Self {
            frames: count,
            mean: frames.iter().sum::<u128>() / u128::try_from(count).ok()?,
            p99: frames[(count * 99).div_ceil(100) - 1],
            worst,
        })
    }

    /// The two lines every benchmark prints.
    pub fn print(&self) {
        let Self {
            frames,
            mean,
            p99,
            worst,
        } = self;
        println!("{frames} frames (1 thread): mean {mean} us, p99 {p99} us, worst {worst} us");
        let verdict = if *worst <= BUDGET_MICROS {
            "fits"
        } else {
            "exceeds"
        };
        println!("worst frame {verdict} half a 60 Hz frame ({BUDGET_MICROS} us)");
    }
}

/// Refuses a run of `total` ticks in which no rollback frame fits.
pub fn check_length(total: u64) -> Result<(), String> {
    if total <= window() {
        return Err(format!(
            "the run has {total} ticks: a rollback frame needs more than {}",
            window()
        ));
    }
    Ok(())
}

fn slot(tick: u64) -> usize {
    usize::try_from(tick % ring_len()).expect("a ring slot fits usize")
}

/// Plays `simulation` (at tick 0) straight through `hashes.len() - 1` ticks with `play`,
/// and at every tick from the window on times one worst-case rollback frame ending at the next
/// tick. `hashes[t]` is the state hash of tick `t` in a straight run, `reference` names where
/// it comes from; a frame that does not end on it is an error, not a timing.
pub fn run(
    simulation: &mut Simulation,
    hashes: &[u64],
    reference: &str,
    mut play: impl FnMut(&mut Simulation, u64) -> Result<(), String>,
) -> Result<Report, String> {
    let total = (hashes.len() as u64).saturating_sub(1);
    check_length(total)?;
    // Every slot holds a state before the first measure, so the timed frames reuse
    // allocations as a running session does.
    let mut ring: Vec<SavedState> = (0..ring_len()).map(|_| SavedState::default()).collect();
    for saved in &mut ring {
        simulation.save_state(saved);
    }
    let mut frames = Vec::new();
    for tick in 0..total {
        simulation.save_state(&mut ring[slot(tick)]);
        if tick < window() {
            play(simulation, tick)?;
            continue;
        }
        // The frame ends at tick + 1, where straight play would be after this tick.
        let start = now();
        simulation
            .load_state(&ring[slot(tick - window())])
            .map_err(|error| error.to_string())?;
        for replayed in tick - window()..=tick {
            play(simulation, replayed)?;
            simulation.save_state(&mut ring[slot(replayed + 1)]);
        }
        let elapsed = start.elapsed().as_micros();
        // A wrong load must not produce a timing.
        if simulation.hash() != hashes[(tick + 1) as usize] {
            return Err(format!(
                "tick {} differs from {reference} after a rollback frame",
                tick + 1
            ));
        }
        frames.push(elapsed);
    }
    Report::new(frames).ok_or_else(|| "no rollback frame was measured".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_takes_the_nearest_rank_99th_percentile() {
        let report = Report::new((1..=200).rev().collect()).expect("frames");
        assert_eq!(
            report,
            Report {
                frames: 200,
                mean: 100,
                p99: 198,
                worst: 200
            }
        );
    }

    #[test]
    fn report_of_one_frame_is_that_frame() {
        let report = Report::new(vec![7]).expect("a frame");
        assert_eq!((report.mean, report.p99, report.worst), (7, 7, 7));
    }

    #[test]
    fn report_of_no_frame_is_none() {
        assert_eq!(Report::new(Vec::new()), None);
    }

    #[test]
    fn a_run_no_longer_than_the_window_is_refused() {
        assert!(check_length(window()).is_err());
        assert!(check_length(window() + 1).is_ok());
    }
}
