//! Host of the late asset arrival spike, for the assets research. Throwaway,
//! never product code.
//!
//! The reference workload depends on one piece of domain data, the repulsion
//! parameters table, delivered by a loader thread after a real delay that
//! differs between runs. The spike measures, instead of assuming, what the
//! fence between asset arrival and the deterministic domain gives and costs:
//!
//! - the loading gate: a world that requires the asset waits for it and then
//!   gives the reference hashes at every worker count whatever the delay;
//! - the declared default: a world that does not wait reads the default table
//!   until the tick the arrival takes effect at, which depends on the delay;
//!   that tick is an input recorded in the log, so that a replay of the log
//!   reproduces the run at every worker count, and a sequential oracle with
//!   the table switched at that tick agrees;
//! - the fence removed, as the control: the pass reads the table the moment
//!   the loader stores it, and runs diverge;
//! - a hot reload: a second version of the table applied at a scripted tick,
//!   deterministic across worker counts and replayed;
//! - the fence's cost against a world with no asset machinery at all;
//! - failed transitions: an unknown asset or a corrupt payload is a failed
//!   availability, recorded, and the world keeps stepping with the default.

mod world;

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reference_workload::{
    DEFAULT_TICK_COUNT, Json, Parameters, Run, Size, first_divergent_tick, run_hash, run_sequential,
    write_platform_and_build,
};
use world::{Arrival, Availability, InputLog, RepulsionTable, StepError, World, WorldConfig, oracle};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-asset-arrival";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
/// The worker counts of the scenarios that do not sweep every count.
const PAIR_WORKER_COUNTS: [usize; 2] = [1, 4];
/// The host steps the worlds that do not wait in batches of this many ticks.
const BATCH: u32 = 10;
/// The explicit loader delays of the gate scenario, beside the clock's.
const EXPLICIT_DELAYS_MS: [u64; 2] = [0, 500];
/// The alternating rounds of the cost scenario.
const COST_ROUNDS: u64 = 3;
const KNOWN_TABLES: [RepulsionTable; 2] = [RepulsionTable::VERSION_1, RepulsionTable::VERSION_2];

/// The scenario's parameters: the measured ones on the default size, smaller
/// ones for the self-checks (where a pause between batches lets a loader with
/// a short delay land inside a run of a few milliseconds).
#[derive(Clone, Copy)]
struct Scenario {
    size: Size,
    ticks: u32,
    hot_reload_tick: u32,
    /// The clock-drawn loader delay lies in this range, in milliseconds.
    delay_ms: (u64, u64),
    batch_pause: Duration,
}

const MEASURED: Scenario =
    Scenario { size: Size::Default, ticks: DEFAULT_TICK_COUNT, hot_reload_tick: 500, delay_ms: (20, 300), batch_pause: Duration::ZERO };

impl Scenario {
    fn parameters(&self) -> Parameters {
        Parameters::new(self.size, self.ticks, false)
    }

    /// A real delay drawn from the clock, different between runs.
    fn clock_delay(&self) -> Duration {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| u64::from(since.subsec_nanos()));
        let (low, high) = self.delay_ms;
        Duration::from_millis(low + nanos % (high - low + 1))
    }

    fn step_in_batches(&self, world: &mut World) {
        while world.tick() < self.ticks {
            world.step(BATCH.min(self.ticks - world.tick())).expect("the world steps");
            if !self.batch_pause.is_zero() {
                thread::sleep(self.batch_pause);
            }
        }
    }
}

fn worker_counts() -> Vec<usize> {
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = BASE_WORKER_COUNTS.to_vec();
    if !counts.contains(&available) {
        counts.push(available);
        counts.sort_unstable();
    }
    counts
}

/// A loader: sleeps the delay (the simulated load), then posts the arrival.
fn spawn_loader(queue: Arc<Mutex<Vec<Arrival>>>, delay: Duration, arrival: Arrival) -> JoinHandle<Duration> {
    thread::spawn(move || {
        let started = Instant::now();
        thread::sleep(delay);
        queue.lock().expect("the queue is never poisoned").push(arrival);
        started.elapsed()
    })
}

/// The control's loader: writes the table to the shared slot the moment it lands.
fn spawn_direct_writer(slot: Arc<AtomicU64>, delay: Duration, table: RepulsionTable) -> JoinHandle<Duration> {
    thread::spawn(move || {
        let started = Instant::now();
        thread::sleep(delay);
        slot.store(table.packed(), Ordering::Relaxed);
        started.elapsed()
    })
}

/// An availability state as the report prints it.
fn availability_text(availability: &Availability) -> String {
    match availability {
        Availability::Unavailable => "unavailable".to_string(),
        Availability::Available(hash) => format!("available at 0x{hash:016x}"),
        Availability::Failed(reason) => format!("failed: {reason}"),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn ticks_per_second(ticks: u32, duration: Duration) -> f64 {
    if duration.is_zero() { 0.0 } else { f64::from(ticks) / duration.as_secs_f64() }
}

fn divergence(ours: &[u64], theirs: &[u64]) -> i64 {
    first_divergent_tick(ours, theirs).map_or(-1, |t| t as i64)
}

/// Polls the gate until it opens, as a host would; returns the wait.
fn wait_for_gate(world: &mut World) -> Duration {
    let started = Instant::now();
    while world.loading() {
        thread::sleep(Duration::from_micros(200));
    }
    started.elapsed()
}

fn replay(scenario: &Scenario, workers: usize, required: bool, log: &InputLog) -> World {
    let mut world = World::new(WorldConfig::new(scenario.size, workers).fenced(required), log.script(&KNOWN_TABLES));
    if required {
        assert!(!world.loading(), "a replayed gate opens on its scripted arrival");
    }
    world.step(scenario.ticks).expect("the replay steps");
    world
}

// ---------------------------------------------------------------------------
// The scenarios
// ---------------------------------------------------------------------------

/// The loading gate: the asset is required, the loader lands after the delay,
/// the host waits for the gate and then steps; the hashes are the reference's.
fn gate(out: &mut Json, scenario: &Scenario, workers: usize, delay: Option<Duration>, reference: &Run) {
    let requested = delay.unwrap_or_else(|| scenario.clock_delay());
    let mut world = World::new(WorldConfig::new(scenario.size, workers).fenced(true), Vec::new());
    let loader = spawn_loader(world.queue(), requested, RepulsionTable::VERSION_1.arrival(1));
    let step_while_loading = if world.loading() {
        if matches!(world.step(1), Err(StepError::Loading)) && world.tick() == 0 { "refused" } else { "not refused" }
    } else {
        "gate already open"
    };
    let wait = wait_for_gate(&mut world);
    let measured = loader.join().expect("the loader ends");
    world.step(scenario.ticks).expect("the gated world steps once open");
    let ours = world.hashes();
    out.object_start();
    out.u64("workers", workers as u64);
    out.string("delay", if delay.is_some() { "explicit" } else { "clock" });
    out.decimal("delay_ms_requested", ms(requested));
    out.decimal("delay_ms_measured", ms(measured));
    out.decimal("gate_wait_ms", ms(wait));
    out.string("step_while_loading", step_while_loading);
    out.i64("arrival_tick", world.log().arrival_tick(RepulsionTable::VERSION_1.content_hash()).map_or(-1, i64::from));
    out.i64("gate_opened_at", world.log().gate_opened_at.map_or(-1, i64::from));
    out.hex("run_hash", run_hash(&ours));
    out.bool("matches_reference", first_divergent_tick(&ours, &reference.tick_hashes).is_none());
    out.i64("first_divergent_tick", divergence(&ours, &reference.tick_hashes));
    out.decimal("ticks_per_second", ticks_per_second(scenario.ticks, world.stepping()));
    out.object_end();
}

struct DefaultRun {
    arrival_tick: Option<u32>,
    run_hash: u64,
    every_replay_matches: bool,
}

/// The declared default: the host steps at once in batches; the arrival takes
/// effect at a tick the delay decides; that tick is an input, so that the
/// recorded log replays the run at every worker count and the sequential
/// oracle with the switch at that tick agrees.
fn default_and_replay(out: &mut Json, scenario: &Scenario, workers: usize, worker_counts: &[usize], reference: &Run) -> DefaultRun {
    let delay = scenario.clock_delay();
    let mut world = World::new(WorldConfig::new(scenario.size, workers).fenced(false), Vec::new());
    let loader = spawn_loader(world.queue(), delay, RepulsionTable::VERSION_1.arrival(1));
    scenario.step_in_batches(&mut world);
    let measured = loader.join().expect("the loader ends");
    let live = world.hashes();
    let arrival_tick = world.log().arrival_tick(RepulsionTable::VERSION_1.content_hash());
    let schedule: Vec<(u32, RepulsionTable)> = arrival_tick.map(|t| vec![(t, RepulsionTable::VERSION_1)]).unwrap_or_default();
    let oracle_hashes = oracle(&scenario.parameters(), &schedule);
    let log_bytes = world.log().encode();
    let log = InputLog::decode(&log_bytes).expect("the recorded log decodes");
    out.object_start();
    out.u64("workers", workers as u64);
    out.decimal("delay_ms", ms(delay));
    out.decimal("delay_ms_measured", ms(measured));
    out.i64("arrival_tick", arrival_tick.map_or(-1, i64::from));
    out.hex("live_run_hash", run_hash(&live));
    out.bool("live_matches_reference", first_divergent_tick(&live, &reference.tick_hashes).is_none());
    out.i64("first_divergent_tick_from_reference", divergence(&live, &reference.tick_hashes));
    out.bool("live_matches_oracle", first_divergent_tick(&live, &oracle_hashes).is_none());
    out.i64("first_divergent_tick_from_oracle", divergence(&live, &oracle_hashes));
    out.decimal("ticks_per_second", ticks_per_second(scenario.ticks, world.stepping()));
    out.u64("log_bytes", log_bytes.len() as u64);
    out.u64("log_transitions", log.transitions.len() as u64);
    let mut every_replay_matches = true;
    let mut replay_hash = None;
    out.key("replay_workers");
    out.array_start();
    for &w in worker_counts {
        let replayed = replay(scenario, w, false, &log);
        let hashes = replayed.hashes();
        let matches = first_divergent_tick(&hashes, &live).is_none();
        every_replay_matches &= matches;
        replay_hash.get_or_insert(run_hash(&hashes));
        out.object_start();
        out.u64("workers", w as u64);
        out.bool("matches_live", matches);
        out.i64("first_divergent_tick", divergence(&hashes, &live));
        out.i64("arrival_tick", replayed.log().arrival_tick(RepulsionTable::VERSION_1.content_hash()).map_or(-1, i64::from));
        out.object_end();
    }
    out.array_end();
    out.hex("replay_run_hash", replay_hash.unwrap_or(0));
    out.bool("replay_matches_live", every_replay_matches);
    out.object_end();
    DefaultRun { arrival_tick, run_hash: run_hash(&live), every_replay_matches }
}

/// The control: the pass reads the shared slot the loader writes directly,
/// whenever it lands, with no fixed point. Two runs per worker count, each
/// compared with the oracle switched at the tick during which the new value
/// was first read, and with the oracle switched one tick later; then the two
/// runs compared with each other.
fn fence_removed(out: &mut Json, scenario: &Scenario, workers: usize) {
    let mut runs = Vec::new();
    for _ in 0..2 {
        let delay = scenario.clock_delay();
        let mut world = World::new(WorldConfig::new(scenario.size, workers).fence_removed(), Vec::new());
        let writer = spawn_direct_writer(world.slot(), delay, RepulsionTable::VERSION_1);
        scenario.step_in_batches(&mut world);
        let measured = writer.join().expect("the writer ends");
        let observed = world.first_observed_tick();
        let hashes = world.hashes();
        let parameters = scenario.parameters();
        let at_observed = oracle(&parameters, &observed.map(|t| vec![(t, RepulsionTable::VERSION_1)]).unwrap_or_default());
        let one_later = oracle(&parameters, &observed.map(|t| vec![(t + 1, RepulsionTable::VERSION_1)]).unwrap_or_default());
        runs.push((delay, measured, observed, hashes, at_observed, one_later, world.stepping()));
    }
    let between = divergence(&runs[0].3, &runs[1].3);
    for (delay, measured, observed, hashes, at_observed, one_later, stepping) in &runs {
        out.object_start();
        out.u64("workers", workers as u64);
        out.decimal("delay_ms", ms(*delay));
        out.decimal("delay_ms_measured", ms(*measured));
        out.i64("arrival_tick_observed", observed.map_or(-1, i64::from));
        out.hex("run_hash", run_hash(hashes));
        out.bool("matches_oracle", first_divergent_tick(hashes, at_observed).is_none());
        out.i64("first_divergent_tick_from_oracle", divergence(hashes, at_observed));
        out.bool("matches_oracle_one_tick_later", first_divergent_tick(hashes, one_later).is_none());
        out.i64("first_divergent_tick_from_oracle_one_tick_later", divergence(hashes, one_later));
        out.i64("first_divergent_tick_between_runs", between);
        out.bool("runs_identical", between < 0);
        out.decimal("ticks_per_second", ticks_per_second(scenario.ticks, *stepping));
        out.object_end();
    }
}

/// The hot reload: the gate mode with version 2 of the table scripted at the
/// fixed point of a chosen tick; the oracle switches there; the recorded log
/// replays it. Returns the recorded log's bytes.
fn hot_reload(out: &mut Json, scenario: &Scenario, workers: usize, oracle_hashes: &[u64]) -> Vec<u8> {
    let delay = scenario.clock_delay();
    let script = vec![(scenario.hot_reload_tick, RepulsionTable::VERSION_2.arrival(2))];
    let mut world = World::new(WorldConfig::new(scenario.size, workers).fenced(true), script);
    let loader = spawn_loader(world.queue(), delay, RepulsionTable::VERSION_1.arrival(1));
    let wait = wait_for_gate(&mut world);
    loader.join().expect("the loader ends");
    scenario.step_in_batches(&mut world);
    let live = world.hashes();
    let log_bytes = world.log().encode();
    let log = InputLog::decode(&log_bytes).expect("the recorded log decodes");
    let replayed = replay(scenario, workers, true, &log);
    out.object_start();
    out.u64("workers", workers as u64);
    out.decimal("delay_ms", ms(delay));
    out.decimal("gate_wait_ms", ms(wait));
    out.i64("switch_tick", log.arrival_tick(RepulsionTable::VERSION_2.content_hash()).map_or(-1, i64::from));
    out.string("availability_at_the_end", &availability_text(&world.availability()));
    out.hex("run_hash", run_hash(&live));
    out.bool("matches_oracle", first_divergent_tick(&live, oracle_hashes).is_none());
    out.i64("first_divergent_tick_from_oracle", divergence(&live, oracle_hashes));
    out.bool("replay_matches", first_divergent_tick(&replayed.hashes(), &live).is_none());
    out.u64("log_bytes", log_bytes.len() as u64);
    out.decimal("ticks_per_second", ticks_per_second(scenario.ticks, world.stepping()));
    out.object_end();
    log_bytes
}

/// The fence's cost, in two steps: a plain world (the kernel with the
/// specification's constants inlined, no asset machinery), a parameterized
/// world (the same kernel reading its table from a runtime value the world
/// holds, no asset machinery) and a fenced world (the table read per chunk
/// from the domain state, the queue drained at every fixed point, version 1
/// scripted at tick 1), after a warm-up of each, in alternating rounds; the
/// best round of each is the figure, the one least disturbed by whatever else
/// the machine was doing. The parameterized world against the plain one is
/// what the kernel's parameterization costs; the fenced world against the
/// parameterized one is what the fence costs.
fn cost(out: &mut Json, scenario: &Scenario, workers: usize, reference: &Run) {
    let warm_up_ticks = 10;
    let script = vec![(1, RepulsionTable::VERSION_1.arrival(1))];
    let plain_config = WorldConfig::new(scenario.size, workers);
    let parameterized_config = WorldConfig::new(scenario.size, workers).parameterized();
    let fenced_config = WorldConfig::new(scenario.size, workers).fenced(false);
    for config in [plain_config, parameterized_config, fenced_config] {
        let mut warm = World::new(config, script.clone());
        warm.step(warm_up_ticks).expect("the warm-up steps");
    }
    let mut plain_rates = Vec::new();
    let mut parameterized_rates = Vec::new();
    let mut fenced_rates = Vec::new();
    let mut every_run_matches = true;
    for _ in 0..COST_ROUNDS {
        let mut plain = World::new(plain_config, Vec::new());
        plain.step(scenario.ticks).expect("the plain world steps");
        let mut parameterized = World::new(parameterized_config, Vec::new());
        parameterized.step(scenario.ticks).expect("the parameterized world steps");
        let mut fenced = World::new(fenced_config, script.clone());
        fenced.step(scenario.ticks).expect("the fenced world steps");
        plain_rates.push(ticks_per_second(scenario.ticks, plain.stepping()));
        parameterized_rates.push(ticks_per_second(scenario.ticks, parameterized.stepping()));
        fenced_rates.push(ticks_per_second(scenario.ticks, fenced.stepping()));
        every_run_matches &= first_divergent_tick(&plain.hashes(), &reference.tick_hashes).is_none()
            && first_divergent_tick(&parameterized.hashes(), &reference.tick_hashes).is_none()
            && first_divergent_tick(&fenced.hashes(), &reference.tick_hashes).is_none();
    }
    let best = |rates: &[f64]| rates.iter().copied().fold(0.0_f64, f64::max);
    let ratio = |numerator: f64, denominator: f64| if denominator > 0.0 { numerator / denominator } else { 0.0 };
    let (plain_rate, parameterized_rate, fenced_rate) = (best(&plain_rates), best(&parameterized_rates), best(&fenced_rates));
    out.object_start();
    out.u64("workers", workers as u64);
    out.u64("rounds", COST_ROUNDS);
    out.decimal("plain_ticks_per_second", plain_rate);
    out.decimal("parameterized_ticks_per_second", parameterized_rate);
    out.decimal("fenced_ticks_per_second", fenced_rate);
    out.decimal("ratio", ratio(fenced_rate, plain_rate));
    out.decimal("ratio_parameterized_to_plain", ratio(parameterized_rate, plain_rate));
    out.decimal("ratio_fenced_to_parameterized", ratio(fenced_rate, parameterized_rate));
    out.key("rounds_ticks_per_second");
    out.array_start();
    for ((plain, parameterized), fenced) in plain_rates.iter().zip(&parameterized_rates).zip(&fenced_rates) {
        out.object_start();
        out.decimal("plain", *plain);
        out.decimal("parameterized", *parameterized);
        out.decimal("fenced", *fenced);
        out.object_end();
    }
    out.array_end();
    out.bool("every_run_matches_reference", every_run_matches);
    out.object_end();
}

/// Failed transitions: an unknown asset, a payload of the wrong length and a
/// payload whose bytes do not hash to the announced content hash, scripted,
/// then a good payload; the world keeps stepping with the default and equals
/// the oracle switched at the good payload's tick.
fn failed_transitions(out: &mut Json, scenario: &Scenario, workers: usize) {
    let good_tick = scenario.ticks / 2;
    let mut corrupt = RepulsionTable::VERSION_1.arrival(2);
    corrupt.bytes.truncate(5);
    let mut wrong_hash = RepulsionTable::VERSION_1.arrival(3);
    wrong_hash.content_hash ^= 1;
    let unknown = Arrival { request: 1, asset: 7, content_hash: 0, bytes: RepulsionTable::VERSION_1.bytes() };
    let script = vec![
        (good_tick / 4, unknown),
        (good_tick / 2, corrupt),
        (good_tick * 3 / 4, wrong_hash),
        (good_tick, RepulsionTable::VERSION_1.arrival(4)),
    ];
    let mut world = World::new(WorldConfig::new(scenario.size, workers).fenced(false), script);
    let mut availability_after_failures = None;
    while world.tick() < scenario.ticks {
        world.step(BATCH.min(scenario.ticks - world.tick())).expect("the world steps through failed transitions");
        if world.tick() + BATCH >= good_tick && availability_after_failures.is_none() {
            availability_after_failures = Some(world.availability());
        }
    }
    let oracle_hashes = oracle(&scenario.parameters(), &[(good_tick, RepulsionTable::VERSION_1)]);
    let log = world.log();
    out.object_start();
    out.u64("workers", workers as u64);
    out.u64("transitions_logged", log.transitions.len() as u64);
    out.u64("transitions_failed", log.transitions.iter().filter(|t| t.failed).count() as u64);
    out.key("reasons");
    out.array_start();
    for transition in &log.transitions {
        out.object_start();
        out.u64("tick", u64::from(transition.tick));
        out.bool("failed", transition.failed);
        out.string("reason", &transition.reason);
        out.object_end();
    }
    out.array_end();
    out.string("availability_before_the_good_payload", &availability_text(&availability_after_failures.unwrap_or(Availability::Unavailable)));
    out.bool("available_at_the_end", matches!(world.availability(), Availability::Available(_)));
    out.hex("run_hash", run_hash(&world.hashes()));
    out.bool("matches_oracle", first_divergent_tick(&world.hashes(), &oracle_hashes).is_none());
    out.i64("first_divergent_tick_from_oracle", divergence(&world.hashes(), &oracle_hashes));
    out.object_end();
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

fn report(scenario: &Scenario, cell: &str, worker_counts: &[usize]) -> String {
    let parameters = scenario.parameters();
    let reference = run_sequential(&parameters);
    let oracle_version_1 = oracle(&parameters, &[(1, RepulsionTable::VERSION_1)]);
    let hot_reload_oracle = oracle(&parameters, &[(1, RepulsionTable::VERSION_1), (scenario.hot_reload_tick, RepulsionTable::VERSION_2)]);
    let pair_counts: Vec<usize> = PAIR_WORKER_COUNTS.iter().copied().filter(|w| worker_counts.contains(w)).collect();
    let default_workers = if worker_counts.contains(&4) { 4 } else { *worker_counts.last().expect("a worker count") };

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", scenario.size.name());
    out.u64("ticks", u64::from(scenario.ticks));
    out.key("worker_counts");
    out.array_start();
    for &w in worker_counts {
        out.object_start();
        out.u64("workers", w as u64);
        out.object_end();
    }
    out.array_end();
    out.hex("reference_run_hash", reference.run_hash);
    out.decimal("reference_ticks_per_second", reference.ticks_per_second());
    out.bool("oracle_with_version_1_equals_reference", oracle_version_1 == reference.tick_hashes);
    out.key("tables");
    out.object_start();
    out.hex("version_1_content_hash", RepulsionTable::VERSION_1.content_hash());
    out.hex("version_2_content_hash", RepulsionTable::VERSION_2.content_hash());
    out.string("default", &format!("strength {} radius {}", RepulsionTable::DEFAULT.strength, RepulsionTable::DEFAULT.radius));
    out.object_end();

    out.key("gate");
    out.array_start();
    for &workers in worker_counts {
        gate(&mut out, scenario, workers, None, &reference);
        for delay_ms in EXPLICIT_DELAYS_MS {
            gate(&mut out, scenario, workers, Some(Duration::from_millis(delay_ms)), &reference);
        }
    }
    out.array_end();

    out.key("default_and_replay");
    out.array_start();
    let first = default_and_replay(&mut out, scenario, default_workers, worker_counts, &reference);
    let second = default_and_replay(&mut out, scenario, default_workers, worker_counts, &reference);
    out.array_end();
    out.key("default_runs_compared");
    out.object_start();
    out.bool("arrival_ticks_differ", first.arrival_tick != second.arrival_tick);
    out.bool("run_hashes_differ", first.run_hash != second.run_hash);
    out.bool("every_replay_matches_its_live_run", first.every_replay_matches && second.every_replay_matches);
    out.object_end();

    out.key("fence_removed");
    out.array_start();
    for &workers in &pair_counts {
        fence_removed(&mut out, scenario, workers);
    }
    out.array_end();

    out.key("hot_reload");
    out.array_start();
    let mut log_bytes = 0;
    for &workers in worker_counts {
        log_bytes = hot_reload(&mut out, scenario, workers, &hot_reload_oracle).len();
    }
    out.array_end();

    out.key("cost");
    out.array_start();
    for &workers in &pair_counts {
        cost(&mut out, scenario, workers, &reference);
    }
    out.array_end();

    out.key("failed_transitions");
    failed_transitions(&mut out, scenario, default_workers);

    out.u64("log_bytes", log_bytes as u64);
    out.object_end();
    out.finish()
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    println!("{}", report(&MEASURED, &cell, &worker_counts()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_holds_every_scenario_and_every_check_passes() {
        let scenario = Scenario { size: Size::Small, ticks: 40, hot_reload_tick: 20, delay_ms: (5, 12), batch_pause: Duration::from_millis(2) };
        let report = report(&scenario, "self-check", &[1, 3]);
        assert!(report.starts_with('{') && report.ends_with('}'));
        for key in ["\"gate\"", "\"default_and_replay\"", "\"fence_removed\"", "\"hot_reload\"", "\"cost\"", "\"failed_transitions\"", "\"log_bytes\""] {
            assert!(report.contains(key), "missing {key}");
        }
        for check in [
            "oracle_with_version_1_equals_reference",
            "live_matches_oracle",
            "replay_matches_live",
            "replay_matches",
            "every_run_matches_reference",
            "every_replay_matches_its_live_run",
            "available_at_the_end",
        ] {
            assert!(!report.contains(&format!("\"{check}\":false")), "{check} fails somewhere: {report}");
        }
        // The gate entries all match the reference; the hot reload entries all match their oracle.
        let gate = &report[report.find("\"gate\"").unwrap()..report.find("\"default_and_replay\"").unwrap()];
        assert!(!gate.contains("\"matches_reference\":false"), "{gate}");
        assert!(!gate.contains("\"step_while_loading\":\"not refused\""), "{gate}");
        let hot = &report[report.find("\"hot_reload\"").unwrap()..report.find("\"cost\"").unwrap()];
        assert!(!hot.contains("\"matches_oracle\":false"), "{hot}");
        assert!(hot.contains("\"switch_tick\":20"));
        let failed = &report[report.find("\"failed_transitions\"").unwrap()..];
        assert!(failed.contains("\"transitions_failed\":3") && failed.contains("\"matches_oracle\":true"), "{failed}");
    }

    #[test]
    fn the_clock_delay_stays_in_its_range() {
        for _ in 0..50 {
            let delay = MEASURED.clock_delay().as_millis() as u64;
            assert!((MEASURED.delay_ms.0..=MEASURED.delay_ms.1).contains(&delay), "{delay}");
        }
    }
}
