//! The persistent pool prototype of the fundamental architecture phase.
//! Throwaway, never product code.
//!
//! The scenarios and the report: see the README for the hypotheses they
//! measure. `measure`         assert!(report.hashes.two_worlds.iter().all(|run| run.first_matches && run.second_matches && run.final_hashes_differ));s every scenario on a size and a tick count (the
//! self-check runs it on the small size) and `write_report` prints the
//! outcome as one JSON document.

mod pool;
mod world;

use std::sync::Once;
use std::thread;
use std::time::{Duration, Instant};

use reference_workload::{DEFAULT_SEED, DEFAULT_TICK_COUNT, Json, Parameters, Run, WARM_UP_TICKS, first_divergent_tick, run_hash, run_sequential, write_platform_and_build};

use pool::{DEFAULT_SPIN_BOUND, Executor, Runtime, StepScoped};
use world::{Injection, Node, Size, World, WorldConfig};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-persistent-pool";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
const SECOND_SEED: u64 = DEFAULT_SEED ^ 0x9e37_79b9_7f4a_7c15;
/// The alternating rounds of the short timed scenarios; the best is kept.
const ROUNDS: usize = 3;
const HASH_BATCHES: [u32; 4] = [1, 10, 100, 1_000];
const ALTERNATING_BATCH: u32 = 10;
const HARNESS_WORLDS: usize = 16;
const HARNESS_POOL_WORKERS: usize = 4;
const PAUSE_WORKER_COUNTS: [usize; 2] = [1, 4];
const FAULT_WORKER_COUNTS: [usize; 3] = [1, 2, 4];
const INJECTED_CHUNK: usize = 2;

/// Installs, once per process, a panic hook that keeps the injected panics
/// out of the output and prints every other one.
pub fn silence_injected_panics() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let message = payload.downcast_ref::<String>().map(String::as_str).or_else(|| payload.downcast_ref::<&str>().copied()).unwrap_or("");
            if !message.contains("injected panic") {
                previous(info);
            }
        }));
    });
}

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

/// The tick of the injected panic: 300 of a 1 000-tick run, scaled to the run.
fn injection_tick(ticks: u32) -> u32 {
    ticks * 3 / 10
}

/// What a run of the report measures on: the size and tick count of the
/// worlds, the worker counts of the pools, the idle pause and the pauses of
/// the frame loop scenario.
struct Settings {
    size: Size,
    ticks: u32,
    worker_counts: Vec<usize>,
    idle: Duration,
    pauses_ms: Vec<u64>,
}

impl Settings {
    fn reference(&self, seed: u64) -> Run {
        run_sequential(&Parameters { seed, ..Parameters::new(self.size, self.ticks, false) })
    }

    fn world(&self, seed: u64) -> World {
        World::new(WorldConfig::new(self.size).seeded(seed))
    }

    /// Steps a throwaway world for the warm-up ticks on the executor.
    fn warm_up(&self, executor: &dyn Executor) {
        self.world(DEFAULT_SEED).step(WARM_UP_TICKS.min(self.ticks), executor).expect("the warm-up world has no injection");
    }
}

fn us(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Whether a world's history equals the reference's hashes over the ticks it
/// ran, and the first tick that differs otherwise.
fn compare(world: &World, reference: &Run) -> (bool, Option<usize>) {
    let (from, to) = (world.history_start() as usize, world.tick() as usize);
    let divergent = first_divergent_tick(&world.history(), &reference.tick_hashes[from..=to]).map(|index| index + from);
    (divergent.is_none(), divergent)
}

/// Steps a world to the end in batches of at most `batch` ticks.
fn step_to_end(world: &mut World, ticks: u32, batch: u32, executor: &dyn Executor) {
    while world.tick() < ticks {
        world.step(batch.min(ticks - world.tick()), executor).expect("a world without injection never fails");
    }
}

// ---------------------------------------------------------------------------
// The hashes on the persistent pool
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct BatchRun {
    workers: usize,
    batch: u32,
    matches_reference: bool,
    first_divergent_tick: Option<usize>,
    run_hash: u64,
}

#[derive(Debug)]
struct TwoWorldsRun {
    workers: usize,
    first_matches: bool,
    second_matches: bool,
    final_hashes_differ: bool,
}

#[derive(Debug)]
struct SixteenWorldsRun {
    workers: usize,
    worlds: usize,
    matching: usize,
    all_match: bool,
}

#[derive(Debug)]
struct Hashes {
    batches: Vec<BatchRun>,
    two_worlds: Vec<TwoWorldsRun>,
    sixteen_worlds: Vec<SixteenWorldsRun>,
    holds: bool,
}

/// Steps one world per executor from as many threads, one tick per call, each
/// world restored from one snapshot of the default seed at tick 0 (the test
/// harness spike's fixture pattern); the wall time of the whole run and how
/// many worlds match the reference.
fn worlds_from_threads(settings: &Settings, reference: &Run, executors: &[&dyn Executor]) -> (Duration, usize) {
    let fixture = settings.world(DEFAULT_SEED).snapshot().expect("a fresh world has no failure");
    let fixture = &fixture;
    let started = Instant::now();
    let matching = thread::scope(|scope| {
        let handles: Vec<_> = executors
            .iter()
            .map(|&executor| {
                scope.spawn(move || {
                    let mut world = World::restore(fixture, WorldConfig::new(settings.size)).expect("the fixture is a snapshot of this size");
                    step_to_end(&mut world, settings.ticks, 1, executor);
                    compare(&world, reference).0
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().expect("a stepping thread never panics")).filter(|&matches| matches).count()
    });
    (started.elapsed(), matching)
}

fn measure_hashes(settings: &Settings, reference: &Run, second_reference: &Run) -> Hashes {
    let mut batches = Vec::new();
    let mut two_worlds = Vec::new();
    let mut sixteen_worlds = Vec::new();
    for &workers in &settings.worker_counts {
        let runtime = Runtime::new(workers);
        let pool = runtime.pool();
        for batch in HASH_BATCHES {
            let mut world = settings.world(DEFAULT_SEED);
            step_to_end(&mut world, settings.ticks, batch, pool);
            let (matches_reference, first_divergent_tick) = compare(&world, reference);
            batches.push(BatchRun { workers, batch, matches_reference, first_divergent_tick, run_hash: run_hash(&world.history()) });
        }
        let mut first = settings.world(DEFAULT_SEED);
        let mut second = settings.world(SECOND_SEED);
        while first.tick() < settings.ticks {
            let batch = ALTERNATING_BATCH.min(settings.ticks - first.tick());
            first.step(batch, pool).expect("no injection");
            second.step(batch, pool).expect("no injection");
        }
        two_worlds.push(TwoWorldsRun {
            workers,
            first_matches: compare(&first, reference).0,
            second_matches: compare(&second, second_reference).0,
            final_hashes_differ: first.hash() != second.hash(),
        });
        let executors: Vec<&dyn Executor> = (0..HARNESS_WORLDS).map(|_| pool as &dyn Executor).collect();
        let (_, matching) = worlds_from_threads(settings, reference, &executors);
        sixteen_worlds.push(SixteenWorldsRun { workers, worlds: HARNESS_WORLDS, matching, all_match: matching == HARNESS_WORLDS });
    }
    let holds = batches.iter().all(|run| run.matches_reference)
        && two_worlds.iter().all(|run| run.first_matches && run.second_matches)
        && sixteen_worlds.iter().all(|run| run.all_match);
    Hashes { batches, two_worlds, sixteen_worlds, holds }
}

// ---------------------------------------------------------------------------
// The cost per one-tick step and the throughput of long batches
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CostRun {
    pool: &'static str,
    workers: usize,
    thousand_calls_ms: f64,
    one_batch_ms: f64,
    overhead_us_per_step: f64,
}

#[derive(Debug)]
struct CostRatio {
    workers: usize,
    step_scoped_us: f64,
    persistent_us: f64,
    /// Step-scoped over persistent; 0 when the persistent overhead is not positive.
    ratio: f64,
}

#[derive(Debug)]
struct AtFourWorkers {
    measured: bool,
    persistent_us: f64,
    step_scoped_us: f64,
    under_50_us: bool,
    under_fifth_of_step_scoped: bool,
    holds: bool,
}

#[derive(Debug)]
struct StepCost {
    /// The wall time of one hash of the state, the serial task a one-tick
    /// step cannot overlap with the next tick (a batch overlaps it).
    state_hash_us: f64,
    runs: Vec<CostRun>,
    ratios: Vec<CostRatio>,
    at_four_workers: AtFourWorkers,
}

#[derive(Debug)]
struct ThroughputRun {
    pool: &'static str,
    workers: usize,
    ticks_per_second: f64,
}

#[derive(Debug)]
struct ThroughputComparison {
    workers: usize,
    step_scoped_tps: f64,
    persistent_tps: f64,
    /// Persistent over step-scoped.
    ratio: f64,
    within_3_percent: bool,
}

#[derive(Debug)]
struct Throughput {
    runs: Vec<ThroughputRun>,
    comparisons: Vec<ThroughputComparison>,
    holds: bool,
}

/// The two executors of one worker count, created for one round.
enum Pools {
    StepScoped(StepScoped),
    Persistent(Runtime),
}

impl Pools {
    fn create(kind: &str, workers: usize) -> Pools {
        match kind {
            "step_scoped" => Pools::StepScoped(StepScoped::new(workers)),
            _ => Pools::Persistent(Runtime::new(workers)),
        }
    }

    fn executor(&self) -> &dyn Executor {
        match self {
            Pools::StepScoped(executor) => executor,
            Pools::Persistent(runtime) => runtime.pool(),
        }
    }
}

const POOL_KINDS: [&str; 2] = ["step_scoped", "persistent"];

/// The mean wall time of one hash of the state at tick 0, over twenty hashes
/// after one discarded: the serial task that a one-tick step runs alone at
/// its end, while a batch overlaps it with the next tick's spatial hashing.
fn state_hash_us(settings: &Settings) -> f64 {
    let state = reference_workload::State::initialize(&Parameters::new(settings.size, 0, false));
    std::hint::black_box(state.hash());
    let rounds = 20;
    let started = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(state.hash());
    }
    us(started.elapsed()) / f64::from(rounds)
}

/// One round of the step cost measurement on one executor: the wall time of
/// `ticks` calls of `step(1)` and of one call of `step(ticks)`, each after a
/// warm-up on a separate world.
fn cost_round(settings: &Settings, executor: &dyn Executor) -> (Duration, Duration) {
    settings.warm_up(executor);
    let mut one_by_one = settings.world(DEFAULT_SEED);
    let started = Instant::now();
    step_to_end(&mut one_by_one, settings.ticks, 1, executor);
    let calls = started.elapsed();
    settings.warm_up(executor);
    let mut batched = settings.world(DEFAULT_SEED);
    let started = Instant::now();
    step_to_end(&mut batched, settings.ticks, settings.ticks, executor);
    (calls, started.elapsed())
}

fn measure_step_cost_and_throughput(settings: &Settings) -> (StepCost, Throughput) {
    // Indexed by (worker count, pool kind): the best of the rounds.
    let mut best: Vec<Vec<Option<(Duration, Duration)>>> = vec![vec![None; POOL_KINDS.len()]; settings.worker_counts.len()];
    for _ in 0..ROUNDS {
        for (w, &workers) in settings.worker_counts.iter().enumerate() {
            for (k, kind) in POOL_KINDS.iter().enumerate() {
                let pools = Pools::create(kind, workers);
                let (calls, batch) = cost_round(settings, pools.executor());
                let entry = best[w][k].get_or_insert((calls, batch));
                entry.0 = entry.0.min(calls);
                entry.1 = entry.1.min(batch);
            }
        }
    }
    let ticks = f64::from(settings.ticks);
    let mut runs = Vec::new();
    let mut ratios = Vec::new();
    let mut throughput_runs = Vec::new();
    let mut comparisons = Vec::new();
    for (w, &workers) in settings.worker_counts.iter().enumerate() {
        let mut overheads = [0.0; 2];
        let mut rates = [0.0; 2];
        for (k, kind) in POOL_KINDS.iter().enumerate() {
            let (calls, batch) = best[w][k].expect("every cell measured");
            overheads[k] = (us(calls) - us(batch)) / ticks;
            rates[k] = ticks / batch.as_secs_f64();
            runs.push(CostRun { pool: kind, workers, thousand_calls_ms: ms(calls), one_batch_ms: ms(batch), overhead_us_per_step: overheads[k] });
            throughput_runs.push(ThroughputRun { pool: kind, workers, ticks_per_second: rates[k] });
        }
        let ratio = if overheads[1] > 0.0 { overheads[0] / overheads[1] } else { 0.0 };
        ratios.push(CostRatio { workers, step_scoped_us: overheads[0], persistent_us: overheads[1], ratio });
        let rate_ratio = rates[1] / rates[0];
        comparisons.push(ThroughputComparison {
            workers,
            step_scoped_tps: rates[0],
            persistent_tps: rates[1],
            ratio: rate_ratio,
            within_3_percent: (rate_ratio - 1.0).abs() <= 0.03,
        });
    }
    let at_four_workers = match ratios.iter().find(|ratio| ratio.workers == 4) {
        Some(ratio) => {
            let under_50_us = ratio.persistent_us < 50.0;
            let under_fifth_of_step_scoped = ratio.persistent_us <= ratio.step_scoped_us / 5.0;
            AtFourWorkers {
                measured: true,
                persistent_us: ratio.persistent_us,
                step_scoped_us: ratio.step_scoped_us,
                under_50_us,
                under_fifth_of_step_scoped,
                holds: under_50_us && under_fifth_of_step_scoped,
            }
        }
        None => AtFourWorkers { measured: false, persistent_us: 0.0, step_scoped_us: 0.0, under_50_us: false, under_fifth_of_step_scoped: false, holds: false },
    };
    let holds = comparisons.iter().all(|comparison| comparison.within_3_percent);
    (StepCost { state_hash_us: state_hash_us(settings), runs, ratios, at_four_workers }, Throughput { runs: throughput_runs, comparisons, holds })
}

// ---------------------------------------------------------------------------
// The wake-up after a pause
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PauseRun {
    pool: &'static str,
    workers: usize,
    pause_ms: u64,
    step_us_mean: f64,
    step_us_max: f64,
    batch_us_per_tick: f64,
    excess_us_mean: f64,
}

#[derive(Debug)]
struct PauseComparison {
    workers: usize,
    pause_ms: u64,
    step_scoped_mean_us: f64,
    persistent_mean_us: f64,
    persistent_cheaper: bool,
    persistent_excess_us: f64,
}

#[derive(Debug)]
struct WakeAfterPause {
    runs: Vec<PauseRun>,
    comparisons: Vec<PauseComparison>,
    persistent_cheaper_everywhere: bool,
    excess_under_100_us_at_4_workers: bool,
    holds: bool,
}

/// One round of the pause measurement: the mean and maximum wall time of a
/// one-tick step after the pause, over `ticks` steps, after a warm-up.
fn pause_round(settings: &Settings, executor: &dyn Executor, pause: Duration) -> (Duration, Duration) {
    settings.warm_up(executor);
    let mut world = settings.world(DEFAULT_SEED);
    let mut total = Duration::ZERO;
    let mut max = Duration::ZERO;
    for _ in 0..settings.ticks {
        thread::sleep(pause);
        let started = Instant::now();
        world.step(1, executor).expect("no injection");
        let step = started.elapsed();
        total += step;
        max = max.max(step);
    }
    (total / settings.ticks, max)
}

/// One round of the batch reference of the pause scenario: one call of
/// `step(ticks)` after a warm-up, per tick.
fn batch_round(settings: &Settings, executor: &dyn Executor) -> Duration {
    settings.warm_up(executor);
    let mut world = settings.world(DEFAULT_SEED);
    let started = Instant::now();
    step_to_end(&mut world, settings.ticks, settings.ticks, executor);
    started.elapsed() / settings.ticks
}

fn measure_wake_after_pause(settings: &Settings) -> WakeAfterPause {
    struct Best {
        batch: Duration,
        pauses: Vec<(Duration, Duration)>,
    }
    // Indexed by (worker count, pool kind).
    let mut best: Vec<Vec<Option<Best>>> = PAUSE_WORKER_COUNTS.iter().map(|_| POOL_KINDS.iter().map(|_| None).collect()).collect();
    for _ in 0..ROUNDS {
        for (w, &workers) in PAUSE_WORKER_COUNTS.iter().enumerate() {
            for (k, kind) in POOL_KINDS.iter().enumerate() {
                let pools = Pools::create(kind, workers);
                let executor = pools.executor();
                let batch = batch_round(settings, executor);
                let pauses: Vec<(Duration, Duration)> =
                    settings.pauses_ms.iter().map(|&pause_ms| pause_round(settings, executor, Duration::from_millis(pause_ms))).collect();
                match &mut best[w][k] {
                    None => best[w][k] = Some(Best { batch, pauses }),
                    Some(entry) => {
                        entry.batch = entry.batch.min(batch);
                        for (kept, measured) in entry.pauses.iter_mut().zip(pauses) {
                            if measured.0 < kept.0 {
                                *kept = measured;
                            }
                        }
                    }
                }
            }
        }
    }
    let mut runs = Vec::new();
    let mut comparisons = Vec::new();
    for (w, &workers) in PAUSE_WORKER_COUNTS.iter().enumerate() {
        for (p, &pause_ms) in settings.pauses_ms.iter().enumerate() {
            let mut means = [0.0; 2];
            let mut excesses = [0.0; 2];
            for (k, kind) in POOL_KINDS.iter().enumerate() {
                let entry = best[w][k].as_ref().expect("every cell measured");
                let (mean, max) = entry.pauses[p];
                means[k] = us(mean);
                excesses[k] = us(mean) - us(entry.batch);
                runs.push(PauseRun {
                    pool: kind,
                    workers,
                    pause_ms,
                    step_us_mean: us(mean),
                    step_us_max: us(max),
                    batch_us_per_tick: us(entry.batch),
                    excess_us_mean: excesses[k],
                });
            }
            comparisons.push(PauseComparison {
                workers,
                pause_ms,
                step_scoped_mean_us: means[0],
                persistent_mean_us: means[1],
                persistent_cheaper: means[1] < means[0],
                persistent_excess_us: excesses[1],
            });
        }
    }
    let persistent_cheaper_everywhere = comparisons.iter().all(|comparison| comparison.persistent_cheaper);
    let excess_under_100_us_at_4_workers =
        comparisons.iter().filter(|comparison| comparison.workers == 4).all(|comparison| comparison.persistent_excess_us < 100.0);
    WakeAfterPause { runs, comparisons, persistent_cheaper_everywhere, excess_under_100_us_at_4_workers, holds: persistent_cheaper_everywhere && excess_under_100_us_at_4_workers }
}

// ---------------------------------------------------------------------------
// The idle cost
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct IdleWorker {
    worker: usize,
    spin_us: f64,
    parks: u64,
    unparks: u64,
    spurious_wakes: u64,
    parked_at_step_end: bool,
    parked_at_end: bool,
}

#[derive(Debug)]
struct IdleRun {
    workers: usize,
    idle_ms: f64,
    workers_stats: Vec<IdleWorker>,
    max_spin_us: f64,
    max_parks: u64,
    every_worker_parked_at_end: bool,
    holds: bool,
}

#[derive(Debug)]
struct Idle {
    spin_bound_us: f64,
    runs: Vec<IdleRun>,
    holds: bool,
}

fn idle_worker_counts() -> Vec<usize> {
    let mut counts = vec![HARNESS_POOL_WORKERS, available_parallelism()];
    counts.sort_unstable();
    counts.dedup();
    counts
}

fn measure_idle(settings: &Settings) -> Idle {
    let mut runs = Vec::new();
    for workers in idle_worker_counts() {
        let runtime = Runtime::new(workers);
        let pool = runtime.pool();
        let mut world = settings.world(DEFAULT_SEED);
        world.step(WARM_UP_TICKS.min(settings.ticks), pool).expect("no injection");
        pool.reset_idle_stats();
        let at_step_end = pool.idle_stats();
        thread::sleep(settings.idle);
        let at_end = pool.idle_stats();
        let workers_stats: Vec<IdleWorker> = at_end
            .iter()
            .zip(&at_step_end)
            .map(|(end, start)| IdleWorker {
                worker: end.worker,
                spin_us: us(end.spin),
                parks: end.parks,
                unparks: end.unparks,
                spurious_wakes: end.spurious_wakes,
                parked_at_step_end: start.parked,
                parked_at_end: end.parked,
            })
            .collect();
        let max_spin_us = workers_stats.iter().map(|worker| worker.spin_us).fold(0.0, f64::max);
        let max_parks = workers_stats.iter().map(|worker| worker.parks).max().unwrap_or(0);
        let every_worker_parked_at_end = workers_stats.iter().all(|worker| worker.parked_at_end);
        let holds = workers_stats
            .iter()
            .all(|worker| worker.spin_us < 200.0 && worker.parks <= 1 && worker.parked_at_end && (worker.parked_at_step_end || worker.parks == 1));
        runs.push(IdleRun { workers, idle_ms: ms(settings.idle), workers_stats, max_spin_us, max_parks, every_worker_parked_at_end, holds });
    }
    let holds = runs.iter().all(|run| run.holds);
    Idle { spin_bound_us: us(DEFAULT_SPIN_BOUND), runs, holds }
}

// ---------------------------------------------------------------------------
// The harness pattern on a shared pool
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct HarnessRun {
    label: &'static str,
    pool: &'static str,
    pools: usize,
    workers: usize,
    wall_ms: f64,
    matching: usize,
    all_match: bool,
}

#[derive(Debug)]
struct HarnessPattern {
    worlds: usize,
    runs: Vec<HarnessRun>,
    ratio_shared_over_step_scoped: f64,
    shared_not_slower_by_10_percent: bool,
    holds: bool,
}

fn measure_harness_pattern(settings: &Settings, reference: &Run) -> HarnessPattern {
    let mut runs = Vec::new();

    let step_scoped: Vec<StepScoped> = (0..HARNESS_WORLDS).map(|_| StepScoped::new(HARNESS_POOL_WORKERS)).collect();
    let executors: Vec<&dyn Executor> = step_scoped.iter().map(|executor| executor as &dyn Executor).collect();
    let (wall, matching) = worlds_from_threads(settings, reference, &executors);
    runs.push(HarnessRun {
        label: "sixteen_step_scoped_pools",
        pool: "step_scoped",
        pools: HARNESS_WORLDS,
        workers: step_scoped[0].workers(),
        wall_ms: ms(wall),
        matching,
        all_match: matching == HARNESS_WORLDS,
    });

    for (label, workers) in [("one_persistent_pool_of_the_parallelism", available_parallelism()), ("one_persistent_pool_of_four", HARNESS_POOL_WORKERS)] {
        let runtime = Runtime::new(workers);
        let executors: Vec<&dyn Executor> = (0..HARNESS_WORLDS).map(|_| runtime.pool() as &dyn Executor).collect();
        let (wall, matching) = worlds_from_threads(settings, reference, &executors);
        runs.push(HarnessRun { label, pool: "persistent", pools: 1, workers: runtime.pool().workers(), wall_ms: ms(wall), matching, all_match: matching == HARNESS_WORLDS });
    }

    let ratio_shared_over_step_scoped = runs[1].wall_ms / runs[0].wall_ms;
    let shared_not_slower_by_10_percent = ratio_shared_over_step_scoped <= 1.10;
    let holds = shared_not_slower_by_10_percent && runs.iter().all(|run| run.all_match);
    HarnessPattern { worlds: HARNESS_WORLDS, runs, ratio_shared_over_step_scoped, shared_not_slower_by_10_percent, holds }
}

// ---------------------------------------------------------------------------
// The fault isolation on a shared pool
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FaultRun {
    workers: usize,
    injected_tick: u32,
    failure_tick: u32,
    failure_node: String,
    failure_chunk: usize,
    failure_worker: usize,
    message: String,
    healthy_matches: bool,
    failed_tick_done: u32,
    failed_history_matches: bool,
    failed_refuses_step: bool,
    pool_serves_after: bool,
    holds: bool,
}

#[derive(Debug)]
struct FaultIsolation {
    runs: Vec<FaultRun>,
    holds: bool,
}

fn measure_fault_isolation(settings: &Settings, reference: &Run, second_reference: &Run) -> FaultIsolation {
    let injected_tick = injection_tick(settings.ticks);
    let mut runs = Vec::new();
    for workers in FAULT_WORKER_COUNTS {
        let runtime = Runtime::new(workers);
        let pool = runtime.pool();
        let injection = Injection { tick: injected_tick, node: Node::Repulsion, chunk: INJECTED_CHUNK };
        let mut healthy = settings.world(DEFAULT_SEED);
        let mut failing = World::new(WorldConfig::new(settings.size).seeded(SECOND_SEED).injecting(injection));
        let mut failure = None;
        while healthy.tick() < settings.ticks {
            healthy.step(ALTERNATING_BATCH.min(settings.ticks - healthy.tick()), pool).expect("the healthy world has no injection");
            if failure.is_none()
                && failing.tick() < settings.ticks
                && let Err(failed) = failing.step(ALTERNATING_BATCH.min(settings.ticks - failing.tick()), pool)
            {
                failure = Some(failed.0);
            }
        }
        let healthy_matches = compare(&healthy, reference).0;
        let failed_history_matches = compare(&failing, second_reference).0;
        let failed_refuses_step = failing.step(1, pool).is_err();
        let mut fresh = settings.world(DEFAULT_SEED);
        step_to_end(&mut fresh, settings.ticks, ALTERNATING_BATCH, pool);
        let pool_serves_after = compare(&fresh, reference).0;
        let (failure_tick, failure_node, failure_chunk, failure_worker, message) = match &failure {
            Some(failure) => (failure.tick, failure.node.to_string(), failure.chunk, failure.worker, failure.message.clone()),
            None => (0, "none".to_string(), 0, 0, "the injected panic did not fail the world".to_string()),
        };
        let holds = failure.is_some()
            && failure_tick == injected_tick
            && failure_node == "repulsion"
            && failure_chunk == INJECTED_CHUNK
            && failure_worker < workers
            && failing.tick() == injected_tick - 1
            && healthy_matches
            && failed_history_matches
            && failed_refuses_step
            && pool_serves_after;
        runs.push(FaultRun {
            workers,
            injected_tick,
            failure_tick,
            failure_node,
            failure_chunk,
            failure_worker,
            message,
            healthy_matches,
            failed_tick_done: failing.tick(),
            failed_history_matches,
            failed_refuses_step,
            pool_serves_after,
            holds,
        });
    }
    let holds = runs.iter().all(|run| run.holds);
    FaultIsolation { runs, holds }
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

struct Report {
    reference_run_hash: u64,
    reference_ticks_per_second: f64,
    second_reference_run_hash: u64,
    hashes: Hashes,
    step_cost: StepCost,
    throughput: Throughput,
    wake_after_pause: WakeAfterPause,
    idle: Idle,
    harness_pattern: HarnessPattern,
    fault_isolation: FaultIsolation,
}

/// Runs every scenario: the determinism and isolation scenarios first, the
/// timed scenarios last so that they run on a warm machine.
fn measure(settings: &Settings) -> Report {
    silence_injected_panics();
    let reference = settings.reference(DEFAULT_SEED);
    let second_reference = settings.reference(SECOND_SEED);
    let hashes = measure_hashes(settings, &reference, &second_reference);
    let fault_isolation = measure_fault_isolation(settings, &reference, &second_reference);
    let idle = measure_idle(settings);
    let harness_pattern = measure_harness_pattern(settings, &reference);
    let (step_cost, throughput) = measure_step_cost_and_throughput(settings);
    let wake_after_pause = measure_wake_after_pause(settings);
    Report {
        reference_run_hash: reference.run_hash,
        reference_ticks_per_second: reference.ticks_per_second(),
        second_reference_run_hash: second_reference.run_hash,
        hashes,
        step_cost,
        throughput,
        wake_after_pause,
        idle,
        harness_pattern,
        fault_isolation,
    }
}

fn write_option_usize(out: &mut Json, key: &str, value: Option<usize>) {
    match value {
        Some(value) => out.u64(key, value as u64),
        None => out.string(key, "none"),
    }
}

fn write_report(cell: &str, settings: &Settings, report: &Report) -> String {
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    out.string("size", settings.size.name());
    out.u64("entity_count", settings.size.entity_count() as u64);
    out.u64("ticks", u64::from(settings.ticks));
    out.string("profile", if cfg!(debug_assertions) { "dev" } else { "release" });
    write_platform_and_build(&mut out);
    out.key("worker_counts");
    out.array_start();
    for &workers in &settings.worker_counts {
        out.object_start();
        out.u64("workers", workers as u64);
        out.object_end();
    }
    out.array_end();
    out.u64("rounds", ROUNDS as u64);
    out.decimal("spin_bound_us", us(DEFAULT_SPIN_BOUND));
    out.hex("reference_run_hash", report.reference_run_hash);
    out.decimal("reference_ticks_per_second", report.reference_ticks_per_second);
    out.hex("second_seed", SECOND_SEED);
    out.hex("second_reference_run_hash", report.second_reference_run_hash);

    out.key("hashes");
    out.object_start();
    out.key("batches");
    out.array_start();
    for run in &report.hashes.batches {
        out.object_start();
        out.string("pool", "persistent");
        out.u64("workers", run.workers as u64);
        out.u64("batch", u64::from(run.batch));
        out.bool("matches_reference", run.matches_reference);
        write_option_usize(&mut out, "first_divergent_tick", run.first_divergent_tick);
        out.hex("run_hash", run.run_hash);
        out.object_end();
    }
    out.array_end();
    out.key("two_worlds");
    out.array_start();
    for run in &report.hashes.two_worlds {
        out.object_start();
        out.u64("workers", run.workers as u64);
        out.u64("batch", u64::from(ALTERNATING_BATCH));
        out.bool("first_matches", run.first_matches);
        out.bool("second_matches", run.second_matches);
        out.bool("final_hashes_differ", run.final_hashes_differ);
        out.object_end();
    }
    out.array_end();
    out.key("sixteen_worlds");
    out.array_start();
    for run in &report.hashes.sixteen_worlds {
        out.object_start();
        out.u64("workers", run.workers as u64);
        out.u64("worlds", run.worlds as u64);
        out.u64("matching", run.matching as u64);
        out.bool("all_match", run.all_match);
        out.object_end();
    }
    out.array_end();
    out.bool("holds", report.hashes.holds);
    out.object_end();

    out.key("step_cost");
    out.object_start();
    out.key("runs");
    out.array_start();
    for run in &report.step_cost.runs {
        out.object_start();
        out.string("pool", run.pool);
        out.u64("workers", run.workers as u64);
        out.decimal("thousand_calls_ms", run.thousand_calls_ms);
        out.decimal("one_batch_ms", run.one_batch_ms);
        out.decimal("overhead_us_per_step", run.overhead_us_per_step);
        out.object_end();
    }
    out.array_end();
    out.key("ratios");
    out.array_start();
    for ratio in &report.step_cost.ratios {
        out.object_start();
        out.u64("workers", ratio.workers as u64);
        out.decimal("step_scoped_us", ratio.step_scoped_us);
        out.decimal("persistent_us", ratio.persistent_us);
        out.decimal("ratio_step_scoped_over_persistent", ratio.ratio);
        out.object_end();
    }
    out.array_end();
    out.decimal("state_hash_us", report.step_cost.state_hash_us);
    out.key("at_four_workers");
    out.object_start();
    let four = &report.step_cost.at_four_workers;
    out.bool("measured", four.measured);
    out.decimal("persistent_us", four.persistent_us);
    out.decimal("step_scoped_us", four.step_scoped_us);
    out.bool("under_50_us", four.under_50_us);
    out.bool("under_fifth_of_step_scoped", four.under_fifth_of_step_scoped);
    out.bool("holds", four.holds);
    out.object_end();
    out.bool("holds", four.holds);
    out.object_end();

    out.key("wake_after_pause");
    out.object_start();
    out.key("runs");
    out.array_start();
    for run in &report.wake_after_pause.runs {
        out.object_start();
        out.string("pool", run.pool);
        out.u64("workers", run.workers as u64);
        out.u64("pause_ms", run.pause_ms);
        out.decimal("step_us_mean", run.step_us_mean);
        out.decimal("step_us_max", run.step_us_max);
        out.decimal("batch_us_per_tick", run.batch_us_per_tick);
        out.decimal("excess_us_mean", run.excess_us_mean);
        out.object_end();
    }
    out.array_end();
    out.key("comparisons");
    out.array_start();
    for comparison in &report.wake_after_pause.comparisons {
        out.object_start();
        out.u64("workers", comparison.workers as u64);
        out.u64("pause_ms", comparison.pause_ms);
        out.decimal("step_scoped_mean_us", comparison.step_scoped_mean_us);
        out.decimal("persistent_mean_us", comparison.persistent_mean_us);
        out.bool("persistent_cheaper", comparison.persistent_cheaper);
        out.decimal("persistent_excess_us", comparison.persistent_excess_us);
        out.object_end();
    }
    out.array_end();
    out.bool("persistent_cheaper_everywhere", report.wake_after_pause.persistent_cheaper_everywhere);
    out.bool("excess_under_100_us_at_4_workers", report.wake_after_pause.excess_under_100_us_at_4_workers);
    out.bool("holds", report.wake_after_pause.holds);
    out.object_end();

    out.key("idle");
    out.object_start();
    out.decimal("spin_bound_us", report.idle.spin_bound_us);
    out.key("runs");
    out.array_start();
    for run in &report.idle.runs {
        out.object_start();
        out.u64("workers", run.workers as u64);
        out.decimal("idle_ms", run.idle_ms);
        out.key("workers_stats");
        out.array_start();
        for worker in &run.workers_stats {
            out.object_start();
            out.u64("worker", worker.worker as u64);
            out.decimal("spin_us", worker.spin_us);
            out.u64("parks", worker.parks);
            out.u64("unparks", worker.unparks);
            out.u64("spurious_wakes", worker.spurious_wakes);
            out.bool("parked_at_step_end", worker.parked_at_step_end);
            out.bool("parked_at_end", worker.parked_at_end);
            out.object_end();
        }
        out.array_end();
        out.decimal("max_spin_us", run.max_spin_us);
        out.u64("max_parks", run.max_parks);
        out.u64("workers_parked_at_step_end", run.workers_stats.iter().filter(|worker| worker.parked_at_step_end).count() as u64);
        out.bool("every_worker_parked_at_end", run.every_worker_parked_at_end);
        out.bool("holds", run.holds);
        out.object_end();
    }
    out.array_end();
    out.bool("holds", report.idle.holds);
    out.object_end();

    out.key("harness_pattern");
    out.object_start();
    out.u64("worlds", report.harness_pattern.worlds as u64);
    out.string("processor_time", "not measured: no portable standard library API");
    out.key("runs");
    out.array_start();
    for run in &report.harness_pattern.runs {
        out.object_start();
        out.string("label", run.label);
        out.string("pool", run.pool);
        out.u64("pools", run.pools as u64);
        out.u64("workers", run.workers as u64);
        out.decimal("wall_ms", run.wall_ms);
        out.u64("matching", run.matching as u64);
        out.bool("all_match", run.all_match);
        out.object_end();
    }
    out.array_end();
    out.decimal("ratio_shared_over_step_scoped", report.harness_pattern.ratio_shared_over_step_scoped);
    out.bool("shared_not_slower_by_10_percent", report.harness_pattern.shared_not_slower_by_10_percent);
    out.bool("holds", report.harness_pattern.holds);
    out.object_end();

    out.key("fault_isolation");
    out.object_start();
    out.key("runs");
    out.array_start();
    for run in &report.fault_isolation.runs {
        out.object_start();
        out.u64("workers", run.workers as u64);
        out.u64("injected_tick", u64::from(run.injected_tick));
        out.u64("failure_tick", u64::from(run.failure_tick));
        out.string("failure_node", &run.failure_node);
        out.u64("failure_chunk", run.failure_chunk as u64);
        out.u64("failure_worker", run.failure_worker as u64);
        out.string("message", &run.message);
        out.bool("healthy_matches", run.healthy_matches);
        out.u64("failed_tick_done", u64::from(run.failed_tick_done));
        out.bool("failed_history_matches", run.failed_history_matches);
        out.bool("failed_refuses_step", run.failed_refuses_step);
        out.bool("pool_serves_after", run.pool_serves_after);
        out.bool("holds", run.holds);
        out.object_end();
    }
    out.array_end();
    out.bool("holds", report.fault_isolation.holds);
    out.object_end();

    out.key("throughput");
    out.object_start();
    out.key("runs");
    out.array_start();
    for run in &report.throughput.runs {
        out.object_start();
        out.string("pool", run.pool);
        out.u64("workers", run.workers as u64);
        out.decimal("ticks_per_second", run.ticks_per_second);
        out.object_end();
    }
    out.array_end();
    out.key("comparisons");
    out.array_start();
    for comparison in &report.throughput.comparisons {
        out.object_start();
        out.u64("workers", comparison.workers as u64);
        out.decimal("step_scoped_tps", comparison.step_scoped_tps);
        out.decimal("persistent_tps", comparison.persistent_tps);
        out.decimal("ratio_persistent_over_step_scoped", comparison.ratio);
        out.bool("within_3_percent", comparison.within_3_percent);
        out.object_end();
    }
    out.array_end();
    out.bool("holds", report.throughput.holds);
    out.object_end();

    out.object_end();
    out.finish()
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    // A build with debug assertions measures the small size: the dev profile's
    // worlds of 10 000 entities would take too long. The report says which size ran.
    let (size, ticks) = if cfg!(debug_assertions) { (Size::Small, 40) } else { (Size::Default, DEFAULT_TICK_COUNT) };
    let settings = Settings { size, ticks, worker_counts: worker_counts(), idle: Duration::from_secs(1), pauses_ms: vec![1, 16] };
    let report = measure(&settings);
    println!("{}", write_report(&cell, &settings, &report));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_on_the_small_size_passes_every_determinism_isolation_and_instrument_check() {
        let settings = Settings { size: Size::Small, ticks: 40, worker_counts: vec![1, 3], idle: Duration::from_millis(300), pauses_ms: vec![1, 4] };
        let report = measure(&settings);

        assert!(report.hashes.holds, "{:?}", report.hashes);
        assert_eq!(report.hashes.batches.len(), 2 * 4, "two worker counts, four batches");
        assert!(report.hashes.batches.iter().all(|run| run.matches_reference));
        assert!(report.hashes.two_worlds.iter().all(|run| run.first_matches && run.second_matches));
        assert!(report.hashes.sixteen_worlds.iter().all(|run| run.all_match && run.worlds == 16));

        assert_eq!(report.step_cost.runs.len(), 2 * 2, "two pools, two worker counts");
        assert!(report.step_cost.runs.iter().all(|run| run.one_batch_ms > 0.0 && run.thousand_calls_ms > 0.0));
        assert_eq!(report.step_cost.ratios.len(), 2);
        assert!(report.step_cost.state_hash_us > 0.0);
        assert!(!report.step_cost.at_four_workers.measured, "4 workers is not among the counts asked");

        assert_eq!(report.wake_after_pause.runs.len(), 2 * 2 * 2, "two pools, two worker counts, two pauses");
        assert!(report.wake_after_pause.runs.iter().all(|run| run.step_us_max >= run.step_us_mean && run.step_us_mean > 0.0));
        assert_eq!(report.wake_after_pause.comparisons.len(), 2 * 2);

        assert!(!report.idle.runs.is_empty() && report.idle.runs.len() <= 2, "a pool of 4 and a pool of the machine's parallelism");
        for run in &report.idle.runs {
            assert_eq!(run.workers_stats.len(), run.workers - 1);
            assert!(run.every_worker_parked_at_end, "{run:?}");
            assert!(run.workers_stats.iter().all(|worker| worker.parked_at_step_end || worker.parks >= 1), "{run:?}");
        }

        assert_eq!(report.harness_pattern.runs.len(), 3);
        assert!(report.harness_pattern.runs.iter().all(|run| run.all_match && run.wall_ms > 0.0), "{:?}", report.harness_pattern);

        assert!(report.fault_isolation.holds, "{:?}", report.fault_isolation);
        assert_eq!(report.fault_isolation.runs.len(), 3, "1, 2 and 4 workers");
        for run in &report.fault_isolation.runs {
            assert_eq!((run.failure_tick, run.failure_node.as_str(), run.failure_chunk), (12, "repulsion", 2));
            assert!(run.healthy_matches && run.failed_refuses_step && run.pool_serves_after && run.failed_history_matches);
            assert_eq!(run.failed_tick_done, 11);
        }

        assert_eq!(report.throughput.comparisons.len(), 2);
        assert!(report.throughput.runs.iter().all(|run| run.ticks_per_second > 0.0));

        let json = write_report("self-check", &settings, &report);
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"spike\":\"workload-persistent-pool\""));
        assert!(json.contains("\"cell\":\"self-check\""));
        assert!(json.contains("\"size\":\"small\""));
        assert!(!json.contains("\"matches_reference\":false"));
        assert!(!json.contains("\"all_match\":false"));
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

    #[test]
    fn the_injection_tick_scales_with_the_tick_count() {
        assert_eq!(injection_tick(1_000), 300);
        assert_eq!(injection_tick(40), 12);
    }
}
