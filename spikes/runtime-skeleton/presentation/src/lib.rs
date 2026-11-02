//! Presentation capability of the runtime skeleton spike. Throwaway, never
//! product code.
//!
//! The one thing a presentation capability adds to the core in this spike: an
//! implementation of the extraction extension point that the fixed point
//! calls after every tick with the state after it. The stub reads the state
//! (it counts the alive entities, as an extraction of a presentation state
//! would read positions) and writes nothing; its only outputs are its own
//! counters, which the host reads to show that the node ran.

use std::sync::atomic::{AtomicU64, Ordering};

use skeleton_core::{Extraction, State};

#[derive(Default)]
pub struct StubExtraction {
    extracted_ticks: AtomicU64,
    alive_total: AtomicU64,
}

impl StubExtraction {
    pub fn new() -> StubExtraction {
        StubExtraction::default()
    }

    /// How many ticks the fixed point handed to the stub.
    pub fn extracted_ticks(&self) -> u64 {
        self.extracted_ticks.load(Ordering::Relaxed)
    }

    /// The sum over the extracted ticks of the number of alive entities.
    pub fn alive_total(&self) -> u64 {
        self.alive_total.load(Ordering::Relaxed)
    }
}

impl Extraction for StubExtraction {
    fn extract(&self, _tick: u32, state: &State) {
        let alive = state.alive.iter().filter(|&&alive| alive).count() as u64;
        self.extracted_ticks.fetch_add(1, Ordering::Relaxed);
        self.alive_total.fetch_add(alive, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skeleton_core::{Parameters, Size};

    #[test]
    fn the_stub_counts_what_it_reads_and_changes_nothing() {
        let state = State::initialize(&Parameters::new(Size::Small, 0, false));
        let before = state.hash();
        let stub = StubExtraction::new();
        stub.extract(1, &state);
        stub.extract(2, &state);
        assert_eq!(stub.extracted_ticks(), 2);
        assert_eq!(stub.alive_total(), 2 * Size::Small.entity_count() as u64);
        assert_eq!(state.hash(), before);
    }
}
