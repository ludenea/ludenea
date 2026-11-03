//! Reference workload history capture spike, for the control surface
//! research. Throwaway, never product code.
//!
//! The smallest always-on capture from which causal queries are computable
//! later: the per-tick input log, the per-tick hash, the event log and a
//! snapshot on a cadence, over the reference workload with kicks as inputs
//! and wall bounces as events. The spike measures, instead of assuming,
//! whether a world restored from any snapshot and replayed from the log
//! reaches the live run's hashes and events at every worker count, what the
//! capture costs per tick at cadences of 60, 10 and 1, how long a causal
//! query on one entity takes by instrumented replay on a harness world while
//! a paced host keeps stepping the live world, whether the live world is
//! unaffected, what the capture retains in memory against the arithmetic of
//! the proposal, and whether a tick outside retention is a structured error.

mod capture;
mod world;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use capture::{Capture, CaptureConfig, CausalAnswer, causal_query};
use reference_workload::{DEFAULT_TICK_COUNT, Json, State, first_divergent_tick, run_hash, write_platform_and_build};
use world::{Kick, Size, StepOutput, World, WorldConfig};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-history-capture";
const BATCH: u32 = 10;
const KICK_EVERY: u32 = 10;
const WATCHED: u32 = 16;
const CADENCE: u32 = 60;
const RETENTION: u32 = 600;
const REPLAY_WORKERS: [usize; 4] = [1, 2, 4, 8];
const QUERY_COUNT: u32 = 20;
const PACE: Duration = Duration::from_millis(1);
const QUERY_PAUSE: Duration = Duration::from_millis(20);
/// The skeleton spike's snapshot size and the toy and frame allocator
/// spikes' per-tick log and event counts, the inputs of the proposal's arithmetic.
const PREDICTED_SNAPSHOT_BYTES: f64 = 250_041.0;
const PREDICTED_LOG_BYTES_PER_TICK: f64 = 24.4;
const PREDICTED_EVENTS_PER_TICK: f64 = 4.508;

fn live_workers() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get()).min(4)
}

/// The script's kicks at a tick boundary: every tenth tick, the sixteen
/// watched entities that are alive, toward the center of the world from
/// their centroid, as the toy spike's agent decides.
fn script_kicks(state: &State, boundary: u32) -> Vec<Kick> {
    if !boundary.is_multiple_of(KICK_EVERY) {
        return Vec::new();
    }
    let present: Vec<usize> = (0..WATCHED as usize).filter(|&slot| state.alive[slot]).collect();
    if present.is_empty() {
        return Vec::new();
    }
    let n = present.len() as f32;
    let cx = present.iter().map(|&s| state.x[s]).sum::<f32>() / n;
    let cy = present.iter().map(|&s| state.y[s]).sum::<f32>() / n;
    let (dvx, dvy) = (if cx < 512.0 { 8.0 } else { -8.0 }, if cy < 512.0 { 8.0 } else { -8.0 });
    present.iter().map(|&s| Kick { index: s as u32, generation: state.generation[s], delta_vx: dvx, delta_vy: dvy }).collect()
}

struct LiveRun {
    world: World,
    capture: Capture,
    kicks: u64,
    events: u64,
    wall: Duration,
}

/// A live world stepped to `ticks` in batches, the script's kicks decided at
/// every boundary, captured by `config` (a cadence of 0 captures the log and
/// the events without snapshots).
fn live_run(world_config: WorldConfig, config: CaptureConfig, batch: u32, ticks: u32) -> LiveRun {
    World::new(world_config).step(vec![Vec::new(); 10]).expect("the warm-up steps");
    let mut world = World::new(world_config);
    let mut capture = Capture::new(config);
    let mut kicks = 0;
    let mut events = 0;
    let started = Instant::now();
    while world.tick() < ticks {
        let count = batch.min(ticks - world.tick());
        let mut records = vec![Vec::new(); count as usize];
        records[0] = script_kicks(world.state(), world.tick());
        kicks += records[0].len() as u64;
        let output: StepOutput = world.step(records.clone()).expect("the live world steps");
        events += output.events.iter().map(|e| e.len() as u64).sum::<u64>();
        capture.after_step(&world, &records, &output);
    }
    LiveRun { world, capture, kicks, events, wall: started.elapsed() }
}

fn hashes(world: &World) -> Vec<u64> {
    (world.history_start()..=world.tick()).map(|t| world.tick_hash(t).expect("in the history")).collect()
}

// ---------------------------------------------------------------------------
// Replays from every snapshot
// ---------------------------------------------------------------------------

fn replay_sweep(out: &mut Json, live: &LiveRun) {
    let ticks = live.capture.snapshot_ticks();
    out.key("replays");
    out.array_start();
    let mut matched = vec![0_u64; REPLAY_WORKERS.len()];
    let mut total = 0_u64;
    for window in ticks.windows(2).map(|w| (w[0], w[1])).chain(ticks.last().filter(|&&last| last < live.world.tick()).map(|&last| (last, live.world.tick()))) {
        let (from, to) = window;
        let plan = live.capture.plan_from(from, to).expect("a retained window replays");
        for (index, &workers) in REPLAY_WORKERS.iter().enumerate() {
            let restoring = Instant::now();
            let mut replayed = World::restore(&plan.snapshot, workers, None).expect("the snapshot restores");
            let restore = restoring.elapsed();
            let replaying = Instant::now();
            let output = replayed.step(plan.records.iter().map(|r| r.kicks.clone()).collect()).expect("the replay steps");
            let replay = replaying.elapsed();
            let recorded: Vec<u64> = std::iter::once(live.world.tick_hash(from).expect("in the history")).chain(plan.records.iter().map(|r| r.hash)).collect();
            let divergence = first_divergent_tick(&hashes(&replayed), &recorded);
            let events_match = output.events == plan.records.iter().map(|r| r.events.clone()).collect::<Vec<_>>();
            let matches = divergence.is_none() && events_match;
            total += 1;
            if matches {
                matched[index] += 1;
            }
            out.object_start();
            out.u64("snapshot_tick", u64::from(from));
            out.u64("to_tick", u64::from(to));
            out.u64("workers", workers as u64);
            out.bool("hashes_match", divergence.is_none());
            out.i64("first_divergent_tick", divergence.map_or(-1, |t| (t as u32 + from) as i64));
            out.bool("events_match", events_match);
            out.decimal("restore_us", restore.as_secs_f64() * 1e6);
            out.decimal("replay_ms", replay.as_secs_f64() * 1_000.0);
            out.object_end();
        }
    }
    out.array_end();
    out.key("replay_summary");
    out.object_start();
    out.u64("windows", total / REPLAY_WORKERS.len() as u64);
    out.u64("replays", total);
    out.u64("replays_matching", matched.iter().sum());
    out.bool("every_replay_matches", matched.iter().sum::<u64>() == total);
    out.key("per_worker_count");
    out.array_start();
    for (index, &workers) in REPLAY_WORKERS.iter().enumerate() {
        out.object_start();
        out.u64("workers", workers as u64);
        out.u64("matching", matched[index]);
        out.object_end();
    }
    out.array_end();
    out.object_end();
}

// ---------------------------------------------------------------------------
// The always-on cost
// ---------------------------------------------------------------------------

fn write_cost(out: &mut Json, cadence: u32, batch: u32, workers: usize, live: &LiveRun) {
    let stepping = live.world.stepping();
    let capture = live.capture.timing.total();
    out.object_start();
    out.u64("cadence", u64::from(cadence));
    out.u64("batch", u64::from(batch));
    out.u64("workers", workers as u64);
    out.hex("run_hash", run_hash(&hashes(&live.world)));
    out.u64("kicks", live.kicks);
    out.u64("events", live.events);
    out.decimal("wall_ms", live.wall.as_secs_f64() * 1_000.0);
    out.decimal("ticks_per_second", f64::from(live.world.tick()) / live.wall.as_secs_f64());
    out.decimal("stepping_ms", stepping.as_secs_f64() * 1_000.0);
    out.decimal("capture_ms", capture.as_secs_f64() * 1_000.0);
    out.decimal("log_ms", live.capture.timing.log.as_secs_f64() * 1_000.0);
    out.decimal("events_ms", live.capture.timing.events.as_secs_f64() * 1_000.0);
    out.decimal("snapshots_ms", live.capture.timing.snapshots.as_secs_f64() * 1_000.0);
    out.u64("snapshots", live.capture.timing.snapshot_count);
    out.decimal("snapshot_us_each", if live.capture.timing.snapshot_count == 0 { 0.0 } else { live.capture.timing.snapshots.as_secs_f64() * 1e6 / live.capture.timing.snapshot_count as f64 });
    out.decimal("capture_share_of_run", capture.as_secs_f64() / (stepping + capture).as_secs_f64().max(1e-9));
    out.decimal("capture_us_per_tick", capture.as_secs_f64() * 1e6 / f64::from(live.world.tick()));
    out.decimal("log_bytes_per_tick", live.capture.log_bytes_written as f64 / f64::from(live.world.tick()));
    out.decimal("event_bytes_per_tick", live.capture.event_bytes_written as f64 / f64::from(live.world.tick()));
    out.object_end();
}

// ---------------------------------------------------------------------------
// Causal queries
// ---------------------------------------------------------------------------

fn write_answer_summary(out: &mut Json, answer: &CausalAnswer) {
    out.u64("index", u64::from(answer.index));
    out.u64("tick", u64::from(answer.tick));
    out.u64("snapshot_tick", u64::from(answer.snapshot_tick));
    out.u64("replayed_ticks", u64::from(answer.replayed_ticks));
    out.bool("hash_matches", answer.hash_matches);
    out.u64("chain_ticks", answer.chain.len() as u64);
    out.u64("kicks_on_the_entity", answer.kicks() as u64);
    out.u64("ticks_with_neighbors", answer.neighbor_ticks() as u64);
    out.u64("bounces", answer.chain.iter().map(|t| u64::from(t.bounced_x) + u64::from(t.bounced_y)).sum());
    out.u64("component_changes", answer.changes() as u64);
    out.decimal("restore_us", answer.restore.as_secs_f64() * 1e6);
    out.decimal("replay_ms", answer.replay.as_secs_f64() * 1_000.0);
    out.decimal("total_ms", answer.total.as_secs_f64() * 1_000.0);
}

fn write_example(out: &mut Json, answer: &CausalAnswer) {
    out.key("example");
    out.object_start();
    write_answer_summary(out, answer);
    out.key("chain");
    out.array_start();
    for tick in answer.chain.iter().filter(|t| !t.kicks.is_empty() || t.bounced_x || t.bounced_y || !t.neighbors.is_empty()).take(4) {
        out.object_start();
        out.u64("tick", u64::from(tick.tick));
        out.string("kicks", &format!("{:?}", tick.kicks));
        out.string("neighbors", &format!("{:?}", tick.neighbors));
        out.string("changes", &tick.changes.iter().map(|(system, component)| format!("{system}: {component}")).collect::<Vec<_>>().join(", "));
        out.bool("bounced_x", tick.bounced_x);
        out.bool("bounced_y", tick.bounced_y);
        out.string("state_after", &format!("{:?}", tick.state_after));
        out.object_end();
    }
    out.array_end();
    out.object_end();
}

fn causal_queries(out: &mut Json, live: &LiveRun, workers: usize) {
    out.object_start();
    out.u64("workers", workers as u64);
    out.u64("queries", u64::from(QUERY_COUNT));
    let mut latencies = Vec::new();
    let mut restores = Vec::new();
    let mut replayed = Vec::new();
    let mut matched = 0_u64;
    let mut example: Option<CausalAnswer> = None;
    let mut errors = Vec::new();
    for i in 0..QUERY_COUNT {
        // Ticks spread over the run from the first snapshot on, never on a
        // snapshot tick, so that every query replays something.
        let tick = (CADENCE + 1 + 47 * i).min(live.world.tick());
        let index = i % WATCHED;
        match live.capture.plan(tick).and_then(|plan| causal_query(&plan, index, tick, workers)) {
            Ok(answer) => {
                latencies.push(answer.total);
                restores.push(answer.restore);
                replayed.push(answer.replayed_ticks);
                if answer.hash_matches {
                    matched += 1;
                }
                if example.as_ref().is_none_or(|e| e.kicks() < answer.kicks()) {
                    example = Some(answer);
                }
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    let mean = |values: &[Duration]| values.iter().sum::<Duration>().as_secs_f64() * 1_000.0 / values.len().max(1) as f64;
    out.u64("answered", latencies.len() as u64);
    out.u64("hashes_matching", matched);
    out.decimal("latency_ms_min", latencies.iter().min().map_or(0.0, |d| d.as_secs_f64() * 1_000.0));
    out.decimal("latency_ms_mean", mean(&latencies));
    out.decimal("latency_ms_max", latencies.iter().max().map_or(0.0, |d| d.as_secs_f64() * 1_000.0));
    out.decimal("restore_ms_mean", mean(&restores));
    out.decimal("replayed_ticks_mean", replayed.iter().map(|&t| f64::from(t)).sum::<f64>() / replayed.len().max(1) as f64);
    out.u64("replayed_ticks_max", u64::from(replayed.iter().copied().max().unwrap_or(0)));
    if let Some(example) = &example {
        write_example(out, example);
        // "Which inputs led here": the records of the window, a read.
        match live.capture.inputs_between(example.snapshot_tick + 1, example.tick) {
            Ok(inputs) => {
                out.u64("example_window_records", inputs.len() as u64);
                out.u64("example_window_kicks", inputs.iter().map(|(_, k)| k.len() as u64).sum());
            }
            Err(error) => out.string("example_window_error", &error.to_string()),
        }
    }
    if !errors.is_empty() {
        out.string("errors", &errors.join(" | "));
    }
    out.object_end();
}

// ---------------------------------------------------------------------------
// The live world under queries
// ---------------------------------------------------------------------------

struct PacedOutcome {
    hashes: Vec<u64>,
    steps: u64,
    stepping: Duration,
    wall: Duration,
    capture: Arc<Mutex<Capture>>,
}

/// A paced host stepping one tick every `PACE`, capturing into a shared
/// capture, while `queries` run against it from this thread when asked.
fn paced_run(world_config: WorldConfig, config: CaptureConfig, ticks: u32, run_queries: bool, query_workers: usize) -> (PacedOutcome, Vec<CausalAnswer>, Vec<String>) {
    let capture = Arc::new(Mutex::new(Capture::new(config)));
    let stop = Arc::new(AtomicBool::new(false));
    let host = {
        let capture = capture.clone();
        thread::spawn(move || {
            let mut world = World::new(world_config);
            let started = Instant::now();
            let mut steps = 0_u64;
            while world.tick() < ticks {
                let records = vec![script_kicks(world.state(), world.tick())];
                let output = world.step(records.clone()).expect("the paced host steps");
                capture.lock().unwrap().after_step(&world, &records, &output);
                steps += 1;
                thread::sleep(PACE);
            }
            (hashes(&world), steps, world.stepping(), started.elapsed())
        })
    };
    let mut answers = Vec::new();
    let mut errors = Vec::new();
    if run_queries {
        loop {
            let plan = {
                let capture = capture.lock().unwrap();
                if capture.last_tick() >= ticks {
                    break;
                }
                let last = capture.last_tick();
                // A query seven ticks behind the live world, once a snapshot exists.
                match capture.oldest_answerable_tick() {
                    Some(oldest) if last >= 7 && last - 7 >= oldest => capture.plan(last - 7).map_err(|e| errors.push(e.to_string())),
                    _ => Err(()),
                }
            };
            if let Ok(plan) = plan {
                match causal_query(&plan, answers.len() as u32 % WATCHED, plan.snapshot_tick + plan.records.len() as u32, query_workers) {
                    Ok(answer) => answers.push(answer),
                    Err(error) => errors.push(error.to_string()),
                }
            }
            thread::sleep(QUERY_PAUSE);
        }
    }
    let (hashes, steps, stepping, wall) = host.join().expect("the host joins");
    stop.store(true, Ordering::SeqCst);
    (PacedOutcome { hashes, steps, stepping, wall, capture }, answers, errors)
}

fn live_under_queries(out: &mut Json, scenario: &Scenario, workers: usize) {
    let world_config = WorldConfig::new(scenario.size, workers);
    let config = CaptureConfig { cadence: CADENCE, retention_ticks: scenario.retention };
    // The direct run: no capture, the same kicks, for the hashes.
    let direct = live_run(world_config, CaptureConfig { cadence: 0, retention_ticks: 0 }, 1, scenario.ticks);
    let (alone, _, _) = paced_run(world_config, config, scenario.ticks, false, workers);
    out.key("live_under_queries");
    out.object_start();
    out.u64("workers", workers as u64);
    out.decimal("pace_ms", PACE.as_secs_f64() * 1_000.0);
    out.u64("ticks", u64::from(scenario.ticks));
    out.bool("alone_matches_direct", alone.hashes == hashes(&direct.world));
    out.u64("host_steps_alone", alone.steps);
    out.decimal("host_step_us_alone", alone.stepping.as_secs_f64() * 1e6 / alone.steps.max(1) as f64);
    out.decimal("host_wall_ms_alone", alone.wall.as_secs_f64() * 1_000.0);
    // The harness worlds of the queries at the live world's worker count, then at one.
    let mut last_capture = None;
    out.key("queried");
    out.array_start();
    for query_workers in [workers, 1] {
        let (queried, answers, errors) = paced_run(world_config, config, scenario.ticks, true, query_workers);
        out.object_start();
        out.u64("query_workers", query_workers as u64);
        out.bool("queried_matches_direct", queried.hashes == hashes(&direct.world));
        out.u64("host_steps", queried.steps);
        out.decimal("host_step_us", queried.stepping.as_secs_f64() * 1e6 / queried.steps.max(1) as f64);
        out.decimal("host_wall_ms", queried.wall.as_secs_f64() * 1_000.0);
        out.u64("queries_answered", answers.len() as u64);
        out.u64("queries_hashes_matching", answers.iter().filter(|a| a.hash_matches).count() as u64);
        out.decimal("query_latency_ms_mean", answers.iter().map(|a| a.total.as_secs_f64() * 1_000.0).sum::<f64>() / answers.len().max(1) as f64);
        out.decimal("query_latency_ms_max", answers.iter().map(|a| a.total.as_secs_f64() * 1_000.0).fold(0.0, f64::max));
        out.decimal("query_replayed_ticks_mean", answers.iter().map(|a| f64::from(a.replayed_ticks)).sum::<f64>() / answers.len().max(1) as f64);
        if !errors.is_empty() {
            out.string("query_errors", &errors.join(" | "));
        }
        out.object_end();
        last_capture = Some(queried.capture);
        if workers == 1 {
            break;
        }
    }
    out.array_end();
    // The retention window at the end of the last run.
    let capture = last_capture.expect("a queried run");
    let capture = capture.lock().unwrap();
    out.key("retention");
    out.object_start();
    out.u64("retention_ticks", u64::from(scenario.retention));
    out.u64("cadence", u64::from(CADENCE));
    out.u64("last_tick", u64::from(capture.last_tick()));
    out.u64("snapshots_retained", capture.snapshot_count() as u64);
    out.i64("oldest_answerable_tick", capture.oldest_answerable_tick().map_or(-1, i64::from));
    let inside = capture.last_tick().saturating_sub(scenario.retention / 2);
    let outside = capture.oldest_answerable_tick().unwrap_or(1).saturating_sub(1);
    out.u64("inside_tick", u64::from(inside));
    out.bool("inside_answered", capture.plan(inside).is_ok());
    out.u64("outside_tick", u64::from(outside));
    match capture.plan(outside) {
        Ok(_) => out.bool("outside_rejected", false),
        Err(error) => {
            out.bool("outside_rejected", true);
            out.string("outside_code", error.code);
            out.string("outside_message", &error.message);
            for (key, value) in &error.context {
                out.string(key, value);
            }
        }
    }
    let memory = capture.memory();
    out.u64("retained_bytes", memory.total() as u64);
    out.u64("retained_snapshot_bytes", memory.snapshot_bytes as u64);
    out.object_end();
    out.object_end();
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

fn write_memory(out: &mut Json, live: &LiveRun) {
    let memory = live.capture.memory();
    let ticks = f64::from(live.world.tick());
    let predicted = PREDICTED_SNAPSHOT_BYTES * (ticks / f64::from(CADENCE)).floor() + PREDICTED_LOG_BYTES_PER_TICK * ticks + 12.0 * ticks + (4.0 + 4.0 * PREDICTED_EVENTS_PER_TICK) * ticks;
    out.key("memory");
    out.object_start();
    out.u64("snapshots", memory.snapshots as u64);
    out.u64("snapshot_bytes", memory.snapshot_bytes as u64);
    out.u64("snapshot_bytes_each", if memory.snapshots == 0 { 0 } else { (memory.snapshot_bytes / memory.snapshots) as u64 });
    out.u64("log_bytes", memory.log_bytes as u64);
    out.u64("hash_bytes", memory.hash_bytes as u64);
    out.u64("event_bytes", memory.event_bytes as u64);
    out.u64("total_bytes", memory.total() as u64);
    out.decimal("predicted_bytes", predicted);
    out.decimal("measured_over_predicted", memory.total() as f64 / predicted);
    out.bool("within_10_percent", (memory.total() as f64 / predicted - 1.0).abs() <= 0.1);
    out.decimal("ten_minutes_at_60_per_second_megabytes", (memory.total() as f64 / ticks) * 36_000.0 / 1e6);
    out.object_end();
}

#[derive(Clone, Copy)]
struct Scenario {
    size: Size,
    ticks: u32,
    retention: u32,
}

const MEASURED: Scenario = Scenario { size: Size::Default, ticks: DEFAULT_TICK_COUNT, retention: RETENTION };

fn report(out: &mut Json, scenario: &Scenario) {
    let workers = live_workers();
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", scenario.size.name());
    out.u64("ticks", u64::from(scenario.ticks));
    out.u64("cadence", u64::from(CADENCE));
    out.u64("kick_every", u64::from(KICK_EVERY));
    out.u64("watched_entities", u64::from(WATCHED));
    out.u64("live_workers", workers as u64);

    let world_config = WorldConfig::new(scenario.size, workers);
    let live = live_run(world_config, CaptureConfig { cadence: CADENCE, retention_ticks: 0 }, BATCH, scenario.ticks);
    out.key("live");
    out.object_start();
    out.hex("run_hash", run_hash(&hashes(&live.world)));
    out.u64("kicks", live.kicks);
    out.u64("events", live.events);
    out.u64("snapshots", live.capture.snapshot_count() as u64);
    out.string("snapshot_ticks", &format!("{:?}", live.capture.snapshot_ticks()));
    out.object_end();
    replay_sweep(out, &live);
    write_memory(out, &live);

    out.key("cost");
    out.array_start();
    for &w in &[1, workers] {
        if w == 1 && workers == 1 {
            continue;
        }
        let config = WorldConfig::new(scenario.size, w);
        for (cadence, batch) in [(0, BATCH), (CADENCE, BATCH), (10, BATCH), (0, 1), (1, 1)] {
            let run = live_run(config, CaptureConfig { cadence, retention_ticks: 0 }, batch, scenario.ticks);
            write_cost(out, cadence, batch, w, &run);
        }
    }
    if workers == 1 {
        for (cadence, batch) in [(0, BATCH), (CADENCE, BATCH), (10, BATCH), (0, 1), (1, 1)] {
            let run = live_run(world_config, CaptureConfig { cadence, retention_ticks: 0 }, batch, scenario.ticks);
            write_cost(out, cadence, batch, 1, &run);
        }
    }
    out.array_end();

    out.key("causal_queries");
    out.array_start();
    for &w in &[1, workers] {
        if w == workers && workers == 1 {
            break;
        }
        causal_queries(out, &live, w);
    }
    if workers == 1 {
        causal_queries(out, &live, 1);
    }
    out.array_end();

    live_under_queries(out, scenario, workers);
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    report(&mut out, &MEASURED);
    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_runs_on_the_small_size_with_every_check_true() {
        let mut out = Json::new();
        out.object_start();
        report(&mut out, &Scenario { size: Size::Small, ticks: 130, retention: 70 });
        out.object_end();
        let json = out.finish();
        for check in ["every_replay_matches", "hashes_match", "alone_matches_direct", "queried_matches_direct", "inside_answered", "outside_rejected"] {
            assert!(!json.contains(&format!("\"{check}\":false")), "{check} fails: {json}");
        }
        assert!(json.contains("\"every_replay_matches\":true"), "{json}");
        assert!(json.contains("\"outside_code\":\"outside_retention\""), "{json}");
        assert!(json.contains("\"queries_answered\":"), "{json}");
        assert!(!json.contains("\"errors\""), "{json}");
        assert!(!json.contains("query_errors"), "{json}");
    }

    #[test]
    fn the_script_kicks_the_alive_watched_entities_toward_the_center_every_tenth_tick() {
        let state = State::initialize(&reference_workload::Parameters::new(Size::Small, 0, false));
        assert!(script_kicks(&state, 3).is_empty());
        let kicks = script_kicks(&state, 10);
        assert_eq!(kicks.len(), 16);
        assert!(kicks.iter().all(|k| k.delta_vx.abs() == 8.0 && k.delta_vy.abs() == 8.0 && k.generation == 0));
    }
}
