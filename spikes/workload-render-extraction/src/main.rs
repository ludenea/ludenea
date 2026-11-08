//! Host executable of the render extraction spike, for the rendering research.
//! Throwaway, never product code.
//!
//! The spike measures, instead of assuming, what a real extraction of render
//! state costs next to the reference workload and whether it leaves the
//! simulation untouched: the runtime skeleton spike's stub extraction node is
//! replaced by a chunked, parallel extraction of positions and a sprite handle
//! per entity into one of two render buffers (see `world.rs`), with and without
//! a per-chunk change detection skip, in the reference workload as specified
//! and in a variant whose upper half of chunks never moves (so that the skip has
//! chunks to skip). Every run is compared tick by tick with the sequential
//! reference (normal variant) or with the same variant at one worker without
//! extraction (static half); the render state's hashes are compared across
//! worker counts and modes; the extraction's processor time per tick and the
//! throughput against the baseline without extraction are reported.

use std::thread;
use std::time::Duration;

use reference_workload::{
    DEFAULT_TICK_COUNT, Json, Parameters, Size, first_divergent_tick, run_hash, run_sequential,
    write_platform_and_build,
};

mod world;
use world::{Extraction, Variant, World, WorldConfig};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-render-extraction";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
/// The ticks per step: the host reads the render state after every batch, as
/// a renderer would read the extracted state between ticks.
const BATCH: u32 = 100;
const WARM_UP_TICKS: u32 = 10;

fn available_parallelism() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get())
}

fn worker_counts() -> Vec<usize> {
    let mut counts = BASE_WORKER_COUNTS.to_vec();
    counts.push(available_parallelism());
    counts.sort_unstable();
    counts.dedup();
    counts
}

struct Outcome {
    variant: Variant,
    extraction: Extraction,
    workers: usize,
    ticks: u32,
    hashes: Vec<u64>,
    /// The render state hash after every batch (empty without extraction).
    render_hashes: Vec<u64>,
    stepping: Duration,
    extraction_time: Duration,
    gathered: u64,
    copied: u64,
}

impl Outcome {
    fn ticks_per_second(&self) -> f64 {
        f64::from(self.ticks) / self.stepping.as_secs_f64()
    }

    fn tick_us(&self) -> f64 {
        self.stepping.as_secs_f64() * 1_000_000.0 / f64::from(self.ticks)
    }

    fn extraction_us_per_tick(&self) -> f64 {
        self.extraction_time.as_secs_f64() * 1_000_000.0 / f64::from(self.ticks)
    }
}

/// A warm-up world of a few ticks, then the measured one, stepped by batches
/// with the render state hashed after every batch.
fn run(size: Size, ticks: u32, variant: Variant, extraction: Extraction, workers: usize) -> Outcome {
    let config = WorldConfig::new(size, workers).extracting(extraction).variant(variant);
    let mut warm_up = World::new(config);
    warm_up.step(WARM_UP_TICKS).expect("the warm-up steps");
    let mut world = World::new(config);
    let mut render_hashes = Vec::new();
    while world.tick() < ticks {
        world.step(BATCH.min(ticks - world.tick())).expect("the world steps");
        if extraction != Extraction::Off {
            render_hashes.push(world.render_hash());
        }
    }
    Outcome {
        variant,
        extraction,
        workers,
        ticks,
        hashes: world.hashes(),
        render_hashes,
        stepping: world.stepping(),
        extraction_time: world.extraction_time(),
        gathered: world.chunks_gathered(),
        copied: world.chunks_copied(),
    }
}

fn write_outcome(out: &mut Json, outcome: &Outcome, compared: &[u64], baseline: &Outcome) {
    out.object_start();
    out.string("variant", outcome.variant.name());
    out.string("mode", outcome.extraction.name());
    out.u64("workers", outcome.workers as u64);
    out.hex("run_hash", run_hash(&outcome.hashes));
    out.hex("final_hash", *outcome.hashes.last().expect("a run has hashes"));
    let divergence = first_divergent_tick(&outcome.hashes, compared);
    match outcome.variant {
        Variant::Normal => out.bool("matches_reference", divergence.is_none()),
        Variant::StaticHalf => out.bool("matches_one_worker", divergence.is_none()),
    }
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    if outcome.extraction == Extraction::Off {
        out.string("render_run_hash", "none");
        out.string("render_hash_final", "none");
    } else {
        out.hex("render_run_hash", run_hash(&outcome.render_hashes));
        out.hex("render_hash_final", *outcome.render_hashes.last().expect("extraction ran"));
    }
    out.decimal("stepping_ms", outcome.stepping.as_secs_f64() * 1_000.0);
    out.decimal("ticks_per_second", outcome.ticks_per_second());
    out.decimal("ratio_to_baseline", outcome.ticks_per_second() / baseline.ticks_per_second());
    out.decimal("tick_us", outcome.tick_us());
    out.decimal("extraction_us_per_tick", outcome.extraction_us_per_tick());
    out.decimal("extraction_share_of_tick", outcome.extraction_us_per_tick() / outcome.tick_us());
    out.decimal("chunks_gathered_per_tick", outcome.gathered as f64 / f64::from(outcome.ticks));
    out.decimal("chunks_copied_per_tick", outcome.copied as f64 / f64::from(outcome.ticks));
    out.object_end();
}

/// The whole report: both variants, every mode, every worker count.
fn report(cell: &str, size: Size, ticks: u32, worker_counts: &[usize]) -> String {
    let reference = run_sequential(&Parameters::new(size, ticks, false));
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", size.name());
    out.u64("entity_count", size.entity_count() as u64);
    out.u64("chunk_count", size.entity_count().div_ceil(reference_workload::CHUNK) as u64);
    out.u64("ticks", u64::from(ticks));
    out.u64("batch", u64::from(BATCH));
    out.hex("reference_run_hash", reference.run_hash);
    out.decimal("reference_ticks_per_second", reference.ticks_per_second());
    out.key("worker_counts");
    out.array_start();
    for &workers in worker_counts {
        out.object_start();
        out.u64("workers", workers as u64);
        out.object_end();
    }
    out.array_end();
    out.key("runs");
    out.array_start();
    for variant in Variant::ALL {
        // The static-half variant is compared with itself at one worker
        // without extraction, the first run of the variant.
        let mut one_worker_hashes: Option<Vec<u64>> = None;
        for &workers in worker_counts {
            let baseline = run(size, ticks, variant, Extraction::Off, workers);
            let compared: Vec<u64> = match variant {
                Variant::Normal => reference.tick_hashes.clone(),
                Variant::StaticHalf => one_worker_hashes.get_or_insert_with(|| baseline.hashes.clone()).clone(),
            };
            write_outcome(&mut out, &baseline, &compared, &baseline);
            for extraction in [Extraction::On, Extraction::Skip] {
                let outcome = run(size, ticks, variant, extraction, workers);
                write_outcome(&mut out, &outcome, &compared, &baseline);
            }
        }
    }
    out.array_end();
    out.object_end();
    out.finish()
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    println!("{}", report(&cell, Size::Default, DEFAULT_TICK_COUNT, &worker_counts()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_holds_every_run_and_every_world_matches() {
        let report = report("self-check", Size::Small, 40, &[1, 3]);
        assert!(report.starts_with('{') && report.ends_with('}'));
        assert_eq!(report.matches("\"variant\":").count(), 12, "two variants, two worker counts, three modes");
        assert_eq!(report.matches("\"matches_reference\":true").count(), 6);
        assert_eq!(report.matches("\"matches_one_worker\":true").count(), 6);
        for check in ["matches_reference", "matches_one_worker"] {
            assert!(!report.contains(&format!("\"{check}\":false")), "{check} fails somewhere: {report}");
        }
        assert_eq!(report.matches("\"render_run_hash\":\"none\"").count(), 4, "the baselines have no render state");
        assert!(report.contains("\"chunks_copied_per_tick\":0.000"));
    }

    #[test]
    fn the_worker_counts_hold_the_base_counts_and_the_machine_once() {
        let counts = worker_counts();
        for base in BASE_WORKER_COUNTS {
            assert!(counts.contains(&base));
        }
        assert!(counts.contains(&available_parallelism()));
        let mut sorted = counts.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(counts, sorted);
    }
}
