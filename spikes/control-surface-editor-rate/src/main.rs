//! Editor-rate spike of the control surface research: a paced host steps the
//! reference workload at 60 ticks per second with a paced editor session
//! (coalesced change sets over every entity, a hundred live values, a
//! scripted drag closed by a transaction), beside a stepped agent session on
//! a second world of the same process. Throwaway, never product code. See
//! README.md for the hypotheses and the method.
//!
//! Prints one JSON document on standard output; the first argument is the
//! cell label.

mod agent;
mod client;
mod host;
mod pool;
mod transport;
mod wire;
mod world;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use reference_workload::{Json, first_divergent_tick, write_platform_and_build};

use client::EditorScript;
use host::{BareReport, EditorReport, EditorRun, TickTimes, Transport};
use pool::Pool;
use wire::{Names, Rows};
use world::{Command, Oracle, Outcome, Size, WorldConfig};

const TICK_RATE: u32 = 60;
/// The stepping thread's share of the tick on behalf of the session, and the
/// agent's loss of throughput, both refute at this many percent.
const SHARE_THRESHOLD_PERCENT: f64 = 5.0;
/// The latency from a `set` to its live value refutes above this many ticks.
const LATENCY_THRESHOLD_TICKS: f64 = 2.0;
/// The mean change set of the second half of a run may differ from the first
/// half's by this ratio at most before it counts as growing with the ticks.
const GROWTH_TOLERANCE: f64 = 0.1;
/// Ticks left out of the per-tick statistics at the start of a run.
const SETTLING_TICKS: usize = 10;

struct Plan {
    size: Size,
    worker_counts: Vec<usize>,
    /// The sampling periods of the watched component: 1, 10 and 100 percent
    /// of the entities changing per tick.
    sampling_periods: [u32; 3],
    tick_period: Duration,
    drag_ticks: u32,
    bare_ticks: u32,
    /// The in-process transport runs at this worker count only.
    in_process_workers: usize,
    agent_ticks: u32,
    agent_rounds: usize,
    /// The editor session runs this long before the agent starts beside it.
    editor_lead: Duration,
}

impl Plan {
    fn measured() -> Plan {
        Plan {
            // An unoptimized build measures the small size, as the other spikes do.
            size: if cfg!(debug_assertions) { Size::Small } else { Size::Default },
            worker_counts: vec![1, 4],
            sampling_periods: [100, 10, 1],
            tick_period: Duration::from_nanos(1_000_000_000 / u64::from(TICK_RATE)),
            drag_ticks: 10 * TICK_RATE,
            bare_ticks: 3 * TICK_RATE,
            in_process_workers: 4,
            agent_ticks: 2_000,
            agent_rounds: 3,
            editor_lead: Duration::from_millis(500),
        }
    }
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

fn sorted(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut values: Vec<f64> = values.collect();
    values.sort_by(f64::total_cmp);
    values
}

/// The nearest-rank quantile of sorted values; 0 for none.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (mut sum, mut count) = (0.0, 0_u32);
    for value in values {
        sum += value;
        count += 1;
    }
    if count == 0 { 0.0 } else { sum / f64::from(count) }
}

/// The per-tick times after the settling ticks, one part of them, in microseconds, sorted.
fn part_us(times: &[TickTimes], part: impl Fn(&TickTimes) -> u32) -> Vec<f64> {
    let skip = if times.len() > 2 * SETTLING_TICKS { SETTLING_TICKS } else { 0 };
    sorted(times[skip..].iter().map(|t| f64::from(part(t)) / 1_000.0))
}

fn percent(part: f64, whole: f64) -> f64 {
    if whole > 0.0 { 100.0 * part / whole } else { 0.0 }
}

// ---------------------------------------------------------------------------
// The editor runs
// ---------------------------------------------------------------------------

/// The oracle's verdict on an editor run: the admitted records replayed
/// sequentially.
struct Replayed {
    matches_oracle: bool,
    first_divergent_tick: i64,
    outcomes_match: bool,
    sets_applied: u64,
    sets_rejected: u64,
    transaction_applied: bool,
    inverse_matches_oracle: bool,
}

fn replay(config: &WorldConfig, report: &EditorReport) -> Replayed {
    let mut oracle = Oracle::new(config);
    let expected: Vec<Vec<Outcome>> = report.log.iter().map(|record| oracle.step(record)).collect();
    let divergent = first_divergent_tick(&report.hashes, &oracle.tick_hashes);
    let (mut sets_applied, mut sets_rejected) = (0, 0);
    for (record, outcomes) in report.log.iter().zip(&report.outcomes) {
        for (command, outcome) in record.iter().zip(outcomes) {
            match (command, outcome) {
                (Command::Set(_), Outcome::Applied) => sets_applied += 1,
                (Command::Set(_), _) => sets_rejected += 1,
                _ => {}
            }
        }
    }
    let answered = report.client.transaction.as_ref();
    let transaction_applied = answered.is_some_and(|result| matches!(result.outcome, Outcome::TransactionApplied { .. }));
    // The result the client holds must be the one the oracle decides at that tick.
    let inverse_matches_oracle = answered.is_some_and(|result| {
        let inverse_whole = match &result.outcome {
            Outcome::TransactionApplied { inverse } => inverse.len() == report.client.transaction_sets.len(),
            _ => true,
        };
        inverse_whole && result.tick >= 1 && expected.get(result.tick as usize - 1).is_some_and(|outcomes| outcomes.contains(&result.outcome))
    });
    Replayed {
        matches_oracle: divergent.is_none(),
        first_divergent_tick: divergent.map_or(-1, |tick| tick as i64),
        outcomes_match: expected == report.outcomes,
        sets_applied,
        sets_rejected,
        transaction_applied,
        inverse_matches_oracle,
    }
}

/// What the hypotheses need from one editor run.
struct EditorSummary {
    workers: usize,
    fraction_percent: f64,
    transport: &'static str,
    session_share_percent: f64,
    with_extraction_share_percent: f64,
    difference_share_percent: f64,
    difference_from_extracting_share_percent: f64,
    latency_p99_ms: f64,
    latency_max_ms: f64,
    latency_p99_ticks: f64,
    latency_max_ticks: f64,
    change_set_bytes_mean: f64,
    change_set_bytes_max: f64,
    growth_ratio: f64,
    full_state_bytes: f64,
    world_us_median: f64,
    checks: bool,
}

fn write_bare(out: &mut Json, key: &str, plan: &Plan, bare: &BareReport, matches_reference: bool) -> f64 {
    let world = part_us(&bare.times, TickTimes::total);
    out.key(key);
    out.object_start();
    out.u64("ticks", bare.times.len() as u64);
    out.bool("matches_reference", matches_reference);
    out.decimal("ticks_per_second", bare.times.len() as f64 / bare.wall.as_secs_f64());
    out.decimal("tick_us_median", quantile(&world, 0.5));
    out.decimal("tick_us_mean", mean(world.iter().copied()));
    out.decimal("tick_us_p99", quantile(&world, 0.99));
    out.decimal("sampling_us_median", quantile(&part_us(&bare.times, |t| t.sampling), 0.5));
    out.decimal("extraction_us_median", quantile(&part_us(&bare.times, |t| t.extraction), 0.5));
    out.decimal("tick_share_of_period_percent", percent(quantile(&world, 0.5), plan.tick_period.as_secs_f64() * 1e6));
    out.object_end();
    quantile(&world, 0.5)
}

#[allow(clippy::too_many_arguments)]
fn write_editor(out: &mut Json, plan: &Plan, config: &WorldConfig, sampling_period: u32, report: &EditorReport, bare_tick_us: f64, extracting_tick_us: f64) -> EditorSummary {
    let n = config.size.entity_count();
    let period_us = plan.tick_period.as_secs_f64() * 1e6;
    let replayed = replay(config, report);
    let ticks = report.times.len();

    let world = part_us(&report.times, TickTimes::world);
    let session = part_us(&report.times, TickTimes::session);
    let extraction = part_us(&report.times, |t| t.extraction);
    let total = part_us(&report.times, TickTimes::total);
    let session_median = quantile(&session, 0.5);
    let extraction_median = quantile(&extraction, 0.5);
    let session_share = percent(session_median, bare_tick_us);
    let with_extraction_share = percent(session_median + extraction_median, bare_tick_us);
    let difference_share = percent(quantile(&total, 0.5) - bare_tick_us, bare_tick_us);
    let difference_from_extracting_share = percent(quantile(&total, 0.5) - extracting_tick_us, extracting_tick_us);

    out.key("with_session");
    out.object_start();
    out.u64("ticks", ticks as u64);
    out.decimal("ticks_per_second", ticks as f64 / report.wall.as_secs_f64());
    out.bool("matches_oracle", replayed.matches_oracle);
    out.i64("first_divergent_tick", replayed.first_divergent_tick);
    out.bool("outcomes_match_oracle", replayed.outcomes_match);
    out.bool("timed_out", report.timed_out);
    out.u64("refused_messages", report.refused_messages.len() as u64);
    out.string("client_error", report.client.error.as_deref().unwrap_or(""));
    out.string("io_executor_error", report.io.error.as_deref().unwrap_or(""));

    out.key("stepping_thread");
    out.object_start();
    out.decimal("tick_us_median", quantile(&world, 0.5));
    out.decimal("extraction_us_median", extraction_median);
    out.decimal("admission_us_median", quantile(&part_us(&report.times, |t| t.admission), 0.5));
    out.decimal("change_publication_us_median", quantile(&part_us(&report.times, |t| t.publication), 0.5));
    out.decimal("session_us_median", session_median);
    out.decimal("session_us_mean", mean(session.iter().copied()));
    out.decimal("session_us_p99", quantile(&session, 0.99));
    out.decimal("session_us_max", quantile(&session, 1.0));
    out.decimal("total_us_median", quantile(&total, 0.5));
    out.decimal("session_share_of_tick_percent", session_share);
    out.decimal("session_share_of_period_percent", percent(session_median, period_us));
    out.decimal("session_and_extraction_share_of_tick_percent", with_extraction_share);
    out.decimal("session_and_extraction_share_of_period_percent", percent(session_median + extraction_median, period_us));
    out.decimal("with_minus_without_share_of_tick_percent", difference_share);
    out.decimal("with_minus_extraction_alone_share_of_tick_percent", difference_from_extracting_share);
    out.object_end();

    let latency_ms = sorted(report.client.latencies.iter().map(|(ms, _)| f64::from(*ms)));
    let latency_ticks = sorted(report.client.latencies.iter().map(|(_, ticks)| f64::from(*ticks)));
    out.key("set_to_live_value_latency");
    out.object_start();
    out.u64("commands", latency_ms.len() as u64);
    out.decimal("ms_median", quantile(&latency_ms, 0.5));
    out.decimal("ms_p99", quantile(&latency_ms, 0.99));
    out.decimal("ms_max", quantile(&latency_ms, 1.0));
    out.decimal("ticks_median", quantile(&latency_ticks, 0.5));
    out.decimal("ticks_p99", quantile(&latency_ticks, 0.99));
    out.decimal("ticks_max", quantile(&latency_ticks, 1.0));
    out.decimal("periods_p99", quantile(&latency_ms, 0.99) * 1_000.0 / period_us);
    out.object_end();

    let sets = report.client.sets_sent.max(1) as f64;
    out.key("drag");
    out.object_start();
    out.u64("sets_sent", report.client.sets_sent);
    out.u64("sets_applied", replayed.sets_applied);
    out.u64("sets_rejected_stale", replayed.sets_rejected);
    out.u64("first_tick", u64::from(report.client.drag_first_tick));
    out.u64("last_tick", u64::from(report.client.drag_last_tick));
    out.decimal("sets_per_tick", report.client.sets_sent as f64 / f64::from((report.client.drag_last_tick - report.client.drag_first_tick + 1).max(1)));
    out.u64("transactions_sent", u64::from(report.client.transactions_sent));
    out.bool("transaction_applied", replayed.transaction_applied);
    out.u64("transaction_sets", report.client.transaction_sets.len() as u64);
    out.bool("inverse_matches_oracle", replayed.inverse_matches_oracle);
    out.decimal("set_command_bytes_string_names", report.client.set_bytes_strings as f64 / sets);
    out.decimal("set_command_bytes_interned_names", report.client.set_bytes_interned as f64 / sets);
    out.object_end();

    let periods = &report.io.periods;
    let bytes = |facts: &host::PeriodFacts| f64::from(facts.change_set_bytes);
    let half = periods.len() / 2;
    let first_half = mean(periods[..half].iter().map(bytes));
    let second_half = mean(periods[half..].iter().map(bytes));
    let growth_ratio = if first_half > 0.0 { second_half / first_half } else { 0.0 };
    let rows_total: u64 = periods.iter().map(|facts| u64::from(facts.rows)).sum();
    let ticks_total: u64 = periods.iter().map(|facts| u64::from(facts.ticks)).sum();
    let fraction_percent = percent(rows_total as f64, (ticks_total.max(1) * n as u64) as f64);
    let size_mean = |layout, names| mean(periods.iter().map(|facts| wire::change_set_size(facts.rows as usize, layout, names) as f64));
    let bytes_mean = mean(periods.iter().map(bytes));
    let bytes_max = periods.iter().map(bytes).fold(0.0, f64::max);
    out.key("change_set");
    out.object_start();
    out.u64("deliveries", periods.len() as u64);
    out.u64("idle_periods", u64::from(report.io.idle_periods));
    out.decimal("ticks_per_delivery_mean", mean(periods.iter().map(|facts| f64::from(facts.ticks))));
    out.u64("ticks_per_delivery_max", periods.iter().map(|facts| u64::from(facts.ticks)).max().unwrap_or(0));
    out.decimal("rows_mean", mean(periods.iter().map(|facts| f64::from(facts.rows))));
    out.u64("rows_max", periods.iter().map(|facts| u64::from(facts.rows)).max().unwrap_or(0));
    out.decimal("measured_fraction_percent", fraction_percent);
    out.decimal("rows_written_per_tick_mean", report.sampled_rows as f64 / ticks.max(1) as f64);
    out.decimal("bytes_mean", bytes_mean);
    out.decimal("bytes_max", bytes_max);
    out.decimal("bytes_mean_first_half", first_half);
    out.decimal("bytes_mean_second_half", second_half);
    out.decimal("second_half_over_first_half", growth_ratio);
    out.decimal("bytes_mean_canonical_rows_string_names", size_mean(Rows::Canonical, Names::Strings));
    out.decimal("bytes_mean_canonical_rows_interned_names", size_mean(Rows::Canonical, Names::Interned));
    out.decimal("bytes_mean_tagged_rows_string_names", size_mean(Rows::Tagged, Names::Strings));
    out.decimal("bytes_mean_tagged_rows_interned_names", size_mean(Rows::Tagged, Names::Interned));
    out.bool("client_copy_matches_host", report.mirror_matches);
    out.bool("sequence_in_order", report.client.sequence_in_order);
    out.object_end();

    out.key("live_values");
    out.object_start();
    out.u64("messages", u64::from(report.client.live_messages));
    out.decimal("values_mean", mean(periods.iter().map(|facts| f64::from(facts.live_values))));
    out.decimal("bytes_mean_string_names", mean(periods.iter().map(|facts| f64::from(facts.live_bytes_strings))));
    out.decimal("bytes_mean_interned_names", mean(periods.iter().map(|facts| f64::from(facts.live_bytes_interned))));
    out.object_end();

    out.key("full_state_read_bytes");
    out.object_start();
    out.u64("canonical_rows_string_names", report.io.full_state_bytes[0] as u64);
    out.u64("canonical_rows_interned_names", report.io.full_state_bytes[1] as u64);
    out.u64("tagged_rows_string_names", report.io.full_state_bytes[2] as u64);
    out.u64("tagged_rows_interned_names", report.io.full_state_bytes[3] as u64);
    out.object_end();
    out.decimal("change_set_max_over_full_state_read", if report.io.full_state_bytes[0] > 0 { bytes_max / report.io.full_state_bytes[0] as f64 } else { 0.0 });

    let io_us = sorted(periods.iter().map(|facts| f64::from(facts.nanos) / 1_000.0));
    out.key("io_executor");
    out.object_start();
    out.decimal("us_per_delivery_median", quantile(&io_us, 0.5));
    out.decimal("us_per_delivery_p99", quantile(&io_us, 0.99));
    out.object_end();
    out.u64("client_bytes_sent", report.client.bytes_sent);
    out.u64("client_bytes_received", report.client.bytes_received);
    out.object_end();

    let checks = replayed.matches_oracle
        && replayed.outcomes_match
        && replayed.inverse_matches_oracle
        && report.mirror_matches
        && report.client.sequence_in_order
        && report.client.ended
        && report.client.error.is_none()
        && report.io.error.is_none()
        && report.refused_messages.is_empty()
        && !report.timed_out;
    for message in &report.refused_messages {
        eprintln!("refused message: {message}");
    }
    EditorSummary {
        workers: config.workers,
        fraction_percent: 100.0 / f64::from(sampling_period),
        transport: report.transport,
        session_share_percent: session_share,
        with_extraction_share_percent: with_extraction_share,
        difference_share_percent: difference_share,
        difference_from_extracting_share_percent: difference_from_extracting_share,
        latency_p99_ms: quantile(&latency_ms, 0.99),
        latency_max_ms: quantile(&latency_ms, 1.0),
        latency_p99_ticks: quantile(&latency_ticks, 0.99),
        latency_max_ticks: quantile(&latency_ticks, 1.0),
        change_set_bytes_mean: bytes_mean,
        change_set_bytes_max: bytes_max,
        growth_ratio,
        full_state_bytes: report.io.full_state_bytes[0] as f64,
        world_us_median: quantile(&world, 0.5),
        checks,
    }
}

fn editor_runs(out: &mut Json, plan: &Plan) -> Vec<EditorSummary> {
    let mut summaries = Vec::new();
    let reference = {
        let mut oracle = Oracle::new(&WorldConfig::new(plan.size, 1));
        for _ in 0..plan.bare_ticks {
            oracle.step(&[]);
        }
        oracle.tick_hashes
    };
    out.key("editor_runs");
    out.array_start();
    for &workers in &plan.worker_counts {
        let config = WorldConfig::new(plan.size, workers);
        let n = config.size.entity_count();
        for sampling_period in plan.sampling_periods {
            eprintln!("editor session: {workers} worker(s), 1 entity in {sampling_period} changing per tick");
            let bare = host::run_bare(config, Pool::new(workers - 1), sampling_period, plan.bare_ticks, plan.tick_period, false);
            let extracting = host::run_bare(config, Pool::new(workers - 1), sampling_period, plan.bare_ticks, plan.tick_period, true);
            let bare_matches = first_divergent_tick(&bare.hashes, &reference).is_none() && first_divergent_tick(&extracting.hashes, &reference).is_none();
            let mut transports = vec![Transport::Loopback];
            if workers == plan.in_process_workers {
                transports.push(Transport::InProcess);
            }
            for transport in transports {
                let run = EditorRun {
                    config,
                    pool: Pool::new(workers - 1),
                    sampling_period,
                    transport,
                    script: EditorScript::new(n, plan.drag_ticks),
                    tick_period: plan.tick_period,
                    max_ticks: plan.drag_ticks + 10 * TICK_RATE,
                };
                out.object_start();
                out.u64("workers", workers as u64);
                out.u64("sampling_period_ticks", u64::from(sampling_period));
                out.decimal("fraction_percent", 100.0 / f64::from(sampling_period));
                match host::run_editor(&run) {
                    Ok(report) => {
                        out.string("transport", report.transport);
                        let bare_tick_us = write_bare(out, "without_session", plan, &bare, bare_matches);
                        let extracting_tick_us = write_bare(out, "extraction_without_session", plan, &extracting, bare_matches);
                        let mut summary = write_editor(out, plan, &config, sampling_period, &report, bare_tick_us, extracting_tick_us);
                        summary.checks &= bare_matches;
                        summaries.push(summary);
                    }
                    Err(error) => {
                        eprintln!("the editor run could not start: {error}");
                        out.string("error", &error.to_string());
                    }
                }
                out.object_end();
            }
        }
    }
    out.array_end();
    summaries
}

// ---------------------------------------------------------------------------
// The interference runs
// ---------------------------------------------------------------------------

struct Beside {
    ticks_per_second: f64,
    agent_matches: bool,
    editor_checks: bool,
    editor_tick_us_median: f64,
}

/// The agent stepping its world while an editor session runs on another
/// world of the process, at every entity changing per tick.
fn agent_beside_editor(plan: &Plan, workers: usize, agent_pool: Arc<Pool>, editor_pool: Arc<Pool>, expected: &[u64]) -> Beside {
    let config = WorldConfig::new(plan.size, workers);
    let end_drag = Arc::new(AtomicBool::new(false));
    let mut script = EditorScript::new(config.size.entity_count(), u32::MAX);
    script.end_drag = Some(end_drag.clone());
    let run = EditorRun { config, pool: editor_pool, sampling_period: 1, transport: Transport::Loopback, script, tick_period: plan.tick_period, max_ticks: 120 * TICK_RATE };
    let editor = thread::Builder::new().name("editor-host".to_string()).spawn(move || host::run_editor(&run).map(|report| (replay(&run.config, &report), report))).expect("spawn the editor host");
    thread::sleep(plan.editor_lead);
    let mut world = world::World::new(config, agent_pool);
    let wall = agent::run(&mut world, plan.agent_ticks);
    end_drag.store(true, Ordering::SeqCst);
    let agent_matches = first_divergent_tick(&world.tick_hashes(), expected).is_none();
    let (editor_checks, editor_tick_us_median) = match editor.join().expect("the editor host never panics") {
        Ok((replayed, report)) => (
            replayed.matches_oracle && replayed.outcomes_match && report.mirror_matches && report.client.error.is_none() && report.io.error.is_none() && !report.timed_out,
            quantile(&part_us(&report.times, TickTimes::world), 0.5),
        ),
        Err(error) => {
            eprintln!("the editor run could not start: {error}");
            (false, 0.0)
        }
    };
    Beside { ticks_per_second: f64::from(plan.agent_ticks) / wall.as_secs_f64(), agent_matches, editor_checks, editor_tick_us_median }
}

struct Interference {
    workers: usize,
    loss_shared_percent: f64,
    shared_resource: String,
    checks: bool,
}

fn interference_runs(out: &mut Json, plan: &Plan, editor_tick_us: impl Fn(usize) -> f64) -> Vec<Interference> {
    let mut results = Vec::new();
    out.key("interference");
    out.array_start();
    for &workers in &plan.worker_counts {
        eprintln!("interference: {workers} worker(s)");
        let config = WorldConfig::new(plan.size, workers);
        let expected = agent::oracle(&config, plan.agent_ticks);
        let (mut alone, mut shared, mut dedicated) = (0.0_f64, 0.0_f64, 0.0_f64);
        let (mut editor_shared_us, mut editor_dedicated_us) = (f64::MAX, f64::MAX);
        let mut checks = true;
        for _ in 0..plan.agent_rounds {
            let mut world = world::World::new(config, Pool::new(workers - 1));
            let wall = agent::run(&mut world, plan.agent_ticks);
            checks &= first_divergent_tick(&world.tick_hashes(), &expected).is_none();
            alone = alone.max(f64::from(plan.agent_ticks) / wall.as_secs_f64());

            let pool = Pool::new(workers - 1);
            let beside = agent_beside_editor(plan, workers, pool.clone(), pool, &expected);
            checks &= beside.agent_matches && beside.editor_checks;
            shared = shared.max(beside.ticks_per_second);
            editor_shared_us = editor_shared_us.min(beside.editor_tick_us_median);

            if workers > 1 {
                let beside = agent_beside_editor(plan, workers, Pool::new(workers - 1), Pool::new(workers - 1), &expected);
                checks &= beside.agent_matches && beside.editor_checks;
                dedicated = dedicated.max(beside.ticks_per_second);
                editor_dedicated_us = editor_dedicated_us.min(beside.editor_tick_us_median);
            }
        }
        let loss_shared = percent(alone - shared, alone);
        let loss_dedicated = (workers > 1).then(|| percent(alone - dedicated, alone));
        let shared_resource = if loss_shared <= SHARE_THRESHOLD_PERCENT {
            "none: the loss is under the threshold".to_string()
        } else {
            match loss_dedicated {
                None => "the processor's cores: at one worker the two worlds share no pool helper, only the cores that the editor world's stepping thread, the I/O executor and the client also run on".to_string(),
                Some(loss) if loss <= SHARE_THRESHOLD_PERCENT => "the frame pool: with one pool per world the loss falls under the threshold".to_string(),
                Some(_) => "the processor's cores, which the editor world's team, the I/O executor and the client also run on: the loss stays with one pool per world".to_string(),
            }
        };
        out.object_start();
        out.u64("workers", workers as u64);
        out.u64("agent_ticks", u64::from(plan.agent_ticks));
        out.u64("rounds", plan.agent_rounds as u64);
        out.bool("every_world_matches_its_oracle", checks);
        out.decimal("agent_alone_ticks_per_second", alone);
        out.decimal("agent_beside_editor_shared_pool_ticks_per_second", shared);
        out.decimal("loss_shared_pool_percent", loss_shared);
        if let Some(loss) = loss_dedicated {
            out.decimal("agent_beside_editor_one_pool_per_world_ticks_per_second", dedicated);
            out.decimal("loss_one_pool_per_world_percent", loss);
            out.decimal("editor_tick_us_median_one_pool_per_world", editor_dedicated_us);
        }
        out.decimal("editor_tick_us_median_shared_pool", editor_shared_us);
        out.decimal("editor_tick_us_median_alone", editor_tick_us(workers));
        out.string("shared_resource", &shared_resource);
        out.object_end();
        results.push(Interference { workers, loss_shared_percent: loss_shared, shared_resource, checks });
    }
    out.array_end();
    results
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

fn at(summary: &EditorSummary) -> String {
    format!("{} worker(s), {} percent, {}", summary.workers, summary.fraction_percent, summary.transport)
}

fn worst<'a>(summaries: &'a [EditorSummary], figure: impl Fn(&EditorSummary) -> f64) -> Option<&'a EditorSummary> {
    summaries.iter().max_by(|a, b| figure(a).total_cmp(&figure(b)))
}

fn write_hypotheses(out: &mut Json, plan: &Plan, editors: &[EditorSummary], interference: &[Interference]) {
    let period_ms = plan.tick_period.as_secs_f64() * 1_000.0;
    out.key("hypotheses");
    out.object_start();

    out.key("stepping_thread_share");
    out.object_start();
    out.string("statement", "the stepping thread's time on behalf of the session (admitting its commands, publishing the change bitset) stays under 5 percent of the tick at every fraction");
    if let Some(run) = worst(editors, |s| s.session_share_percent) {
        out.bool("holds", run.session_share_percent < SHARE_THRESHOLD_PERCENT);
        out.decimal("worst_share_of_tick_percent", run.session_share_percent);
        out.string("worst_at", &at(run));
    }
    out.decimal("threshold_percent", SHARE_THRESHOLD_PERCENT);
    if let Some(run) = worst(editors, |s| s.with_extraction_share_percent) {
        out.bool("holds_with_the_extraction_charged_to_the_session", run.with_extraction_share_percent < SHARE_THRESHOLD_PERCENT);
        out.decimal("worst_share_with_the_extraction_percent", run.with_extraction_share_percent);
        out.string("worst_with_the_extraction_at", &at(run));
    }
    if let Some(run) = worst(editors, |s| s.difference_share_percent) {
        out.decimal("worst_with_minus_without_share_percent", run.difference_share_percent);
        out.string("worst_with_minus_without_at", &at(run));
    }
    if let Some(run) = worst(editors, |s| s.difference_from_extracting_share_percent) {
        out.decimal("worst_with_minus_extraction_alone_share_percent", run.difference_from_extracting_share_percent);
        out.string("worst_with_minus_extraction_alone_at", &at(run));
    }
    out.object_end();

    out.key("set_to_live_value_latency");
    out.object_start();
    out.string("statement", "the latency from a set command to its value in the live-value stream is at most one tick plus one period (the 99th percentile of the commands, in milliseconds, against two tick periods)");
    if let Some(run) = worst(editors, |s| s.latency_p99_ms) {
        out.bool("holds", run.latency_p99_ms <= LATENCY_THRESHOLD_TICKS * period_ms);
        out.decimal("worst_p99_ms", run.latency_p99_ms);
        out.decimal("worst_p99_ticks", run.latency_p99_ms / period_ms);
        out.string("worst_at", &at(run));
    }
    out.decimal("threshold_ms", LATENCY_THRESHOLD_TICKS * period_ms);
    out.decimal("threshold_ticks", LATENCY_THRESHOLD_TICKS);
    if let Some(run) = worst(editors, |s| s.latency_max_ms) {
        out.bool("maximum_within_threshold", run.latency_max_ms <= LATENCY_THRESHOLD_TICKS * period_ms);
        out.decimal("worst_max_ms", run.latency_max_ms);
        out.string("worst_max_at", &at(run));
    }
    if let Some(run) = worst(editors, |s| s.latency_p99_ticks) {
        out.decimal("worst_p99_delivered_ticks", run.latency_p99_ticks);
    }
    if let Some(run) = worst(editors, |s| s.latency_max_ticks) {
        out.decimal("worst_max_delivered_ticks", run.latency_max_ticks);
    }
    out.object_end();

    // The sizes: per worker count and transport, growing with the fraction;
    // per run, level over the run; at every entity changing, under the full read.
    let mut grows_with_fraction = !editors.is_empty();
    for a in editors {
        for b in editors {
            if a.workers == b.workers && a.transport == b.transport && a.fraction_percent < b.fraction_percent {
                grows_with_fraction &= a.change_set_bytes_mean < b.change_set_bytes_mean;
            }
        }
    }
    let steadiest = worst(editors, |s| (s.growth_ratio - 1.0).abs());
    let level = steadiest.is_some_and(|run| (run.growth_ratio - 1.0).abs() <= GROWTH_TOLERANCE);
    let full = worst(editors, |s| if s.fraction_percent >= 100.0 && s.full_state_bytes > 0.0 { s.change_set_bytes_max / s.full_state_bytes } else { 0.0 });
    let below_full = full.is_some_and(|run| run.fraction_percent >= 100.0 && run.change_set_bytes_max < run.full_state_bytes);
    out.key("change_set_size");
    out.object_start();
    out.string("statement", "the change set's size grows with the fraction and not with the tick count, and the message per period at 100 percent stays below the full-state read's bytes");
    out.bool("holds", grows_with_fraction && level && below_full);
    out.bool("grows_with_the_fraction", grows_with_fraction);
    out.bool("level_over_the_run", level);
    if let Some(run) = steadiest {
        out.decimal("worst_second_half_over_first_half", run.growth_ratio);
        out.string("worst_second_half_over_first_half_at", &at(run));
    }
    out.decimal("growth_tolerance", GROWTH_TOLERANCE);
    out.bool("below_the_full_state_read_at_100_percent", below_full);
    if let Some(run) = full {
        out.decimal("largest_change_set_bytes_at_100_percent", run.change_set_bytes_max);
        out.decimal("full_state_read_bytes", run.full_state_bytes);
    }
    out.object_end();

    out.key("agent_interference");
    out.object_start();
    out.string("statement", "a stepped agent session on a second world of the same process loses under 5 percent of its throughput while the editor session runs");
    if let Some(run) = interference.iter().max_by(|a, b| a.loss_shared_percent.total_cmp(&b.loss_shared_percent)) {
        out.bool("holds", run.loss_shared_percent < SHARE_THRESHOLD_PERCENT);
        out.decimal("worst_loss_percent", run.loss_shared_percent);
        out.u64("worst_at_workers", run.workers as u64);
        out.string("shared_resource", &run.shared_resource);
    }
    out.decimal("threshold_percent", SHARE_THRESHOLD_PERCENT);
    out.object_end();

    out.object_end();
}

/// Runs every scenario of `plan`; returns the report and whether every
/// determinism and delivery check passed (the hypotheses' timing verdicts are
/// results, not checks).
fn report(cell: &str, plan: &Plan) -> (String, bool) {
    let mut out = Json::new();
    out.object_start();
    out.string("spike", "control-surface-editor-rate");
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.string("size", plan.size.name());
    out.u64("entity_count", plan.size.entity_count() as u64);
    out.u64("tick_rate", u64::from(TICK_RATE));
    out.decimal("tick_period_ms", plan.tick_period.as_secs_f64() * 1_000.0);
    out.u64("drag_ticks", u64::from(plan.drag_ticks));
    out.u64("dragged_entities", EditorScript::new(plan.size.entity_count(), 0).dragged.len() as u64);
    out.u64("live_fields", EditorScript::new(plan.size.entity_count(), 0).live.len() as u64 * 4);
    out.string("statistics", "medians over the ticks of a run after ten settling ticks unless named otherwise; the agent's throughput is the best of the rounds");

    let editors = editor_runs(&mut out, plan);
    let editor_tick_us = |workers: usize| {
        editors.iter().find(|s| s.workers == workers && s.fraction_percent >= 100.0 && s.transport == "tcp-loopback").map_or(0.0, |s| s.world_us_median)
    };
    let interference = interference_runs(&mut out, plan, editor_tick_us);
    write_hypotheses(&mut out, plan, &editors, &interference);

    let expected_runs = plan.worker_counts.iter().map(|&workers| plan.sampling_periods.len() * if workers == plan.in_process_workers { 2 } else { 1 }).sum::<usize>();
    let checks = editors.len() == expected_runs && editors.iter().all(|s| s.checks) && interference.iter().all(|i| i.checks);
    out.bool("every_check_passes", checks);
    out.object_end();
    (out.finish(), checks)
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unknown".to_string());
    let (json, checks) = report(&cell, &Plan::measured());
    println!("{json}");
    if !checks {
        eprintln!("a determinism or delivery check failed: see the report");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole report on the small size at a fast tick rate: every world
    /// matches its oracle, every delivery check passes. The timing verdicts
    /// are measured, never asserted: a loaded machine does not meet them.
    #[test]
    fn the_whole_report_runs_with_every_check_passing() {
        let plan = Plan {
            size: Size::Small,
            worker_counts: vec![1, 3],
            sampling_periods: [100, 10, 1],
            tick_period: Duration::from_millis(4),
            drag_ticks: 120,
            bare_ticks: 30,
            in_process_workers: 3,
            agent_ticks: 200,
            agent_rounds: 1,
            editor_lead: Duration::from_millis(60),
        };
        let (json, checks) = report("test", &plan);
        assert!(checks, "{json}");
        assert!(json.contains("\"every_check_passes\":true"));
        assert!(json.contains("\"agent_interference\""));
        assert_eq!(json.matches("\"with_session\"").count(), 9, "six loopback runs and three in-process runs");
    }

    #[test]
    fn quantiles_are_nearest_rank() {
        let values = sorted([5.0, 1.0, 3.0, 2.0, 4.0].into_iter());
        assert_eq!(quantile(&values, 0.5), 3.0);
        assert_eq!(quantile(&values, 0.99), 5.0);
        assert_eq!(quantile(&values, 1.0), 5.0);
        assert_eq!(quantile(&[], 0.5), 0.0);
    }
}
