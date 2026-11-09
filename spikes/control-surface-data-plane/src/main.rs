//! Data plane and transactions spike, for the control surface research.
//! Throwaway, never product code.
//!
//! Over the reference workload and the control surface toy spike's world:
//! a shared memory ring per session handed over the local transport, with a
//! state stream and a frame stream stand-in under a lossless or a latest-wins
//! policy; the same streams over a second socket connection; transactions of
//! 1, 10 and 100 elements validated as a whole at the input fixed point, with
//! a full-evaluation mode; the inverse transaction returned on request and
//! applied as a new transaction; and the per-session command sequence with
//! the input log as its record. The spike measures, instead of assuming, what
//! the ring costs the client and the world, whether a stalled client reaches
//! the world, whether a transaction ever applies in part, whether the inverse
//! restores the hash and at what cost, and whether a retry, a gap and an
//! altered retry are handled as designed, with the sequential oracle's hashes
//! throughout.

mod agent;
mod commands;
mod data_plane;
mod ring;
mod scenarios;
mod script;
mod shared_memory;
mod surface;
mod transport;
mod wire;
mod world;

use agent::{Fallible, Receipt};
use commands::Oracle;
use data_plane::{FRAME_STREAM, PlaneConfig, Policy, STATE_STREAM};
use reference_workload::{DEFAULT_TICK_COUNT, Json, run_hash, write_platform_and_build};
use scenarios::{
    BATCH, CommandsRun, InverseCost, PACE, PacedRun, PlainRun, PlaneSpec, Reception, Scale, SocketRead, Stall, divergence, inverse_cost, replay, run_commands, run_direct, run_paced, run_plain,
    run_socket_reads,
};
use world::Size;

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "control-surface-data-plane";
const WORKER_COUNTS: [usize; 2] = [1, 4];
/// The ring's client cost against the socket read: the target and the refuting threshold.
const CLIENT_COST_TARGET: f64 = 0.1;
const CLIENT_COST_REFUTED: f64 = 0.5;
/// The throughput a data plane or a stalled client may cost the world.
const THROUGHPUT_LOSS_LIMIT: f64 = 0.02;
/// The inverse's cost against applying the transaction.
const INVERSE_COST_LIMIT: f64 = 0.1;
const INVERSE_COST_ROUNDS: usize = 61;

fn full_scale() -> Scale {
    Scale { size: Size::Default, ticks: DEFAULT_TICK_COUNT, plane: PlaneConfig::default(), log_retention: 300, stall_at: 300, socket_reads: 200, repetitions: 3, ring_clients_wait: false }
}

/// Both streams under one policy. A stepped client of a lossless second stream
/// waits for its reader between two steps: the socket is slower than the world.
fn plane(second_stream: bool, policy: Policy, verify: bool, stall: Stall) -> PlaneSpec {
    PlaneSpec { second_stream, state: policy, frame: policy, verify, stall, follow: second_stream && policy == Policy::Lossless }
}

/// The best of the repetitions of one way of stepping the world.
struct Throughput {
    name: &'static str,
    plane: Option<PlaneSpec>,
    transport: &'static str,
    /// Ticks per second of the time spent stepping, per repetition.
    stepping: Vec<f64>,
    /// Ticks per second of the agent's wall time, per repetition.
    wall: Vec<f64>,
    first_divergent_tick: Option<u32>,
    state: Receipt,
    frame: Receipt,
    torn_reads: u64,
    content_mismatches: u64,
    frames_evicted: u64,
    ticks_skipped: u64,
    frames_refused: u64,
    pauses: u64,
    endpoint_copy_nanos: u64,
    endpoint_frames: u64,
    extraction_micros: f64,
    stalled_at: Option<u32>,
    ended: bool,
}

fn best(values: &[f64]) -> f64 {
    values.iter().copied().fold(0.0, f64::max)
}

impl Throughput {
    fn new(name: &'static str, plane: Option<PlaneSpec>) -> Throughput {
        Throughput {
            name,
            plane,
            transport: "",
            stepping: Vec::new(),
            wall: Vec::new(),
            first_divergent_tick: None,
            state: Receipt::default(),
            frame: Receipt::default(),
            torn_reads: 0,
            content_mismatches: 0,
            frames_evicted: 0,
            ticks_skipped: 0,
            frames_refused: 0,
            pauses: 0,
            endpoint_copy_nanos: 0,
            endpoint_frames: 0,
            extraction_micros: 0.0,
            stalled_at: None,
            ended: false,
        }
    }

    fn add(&mut self, scale: &Scale, run: PlainRun, oracle: &[u64]) {
        let ticks = f64::from(scale.ticks);
        self.transport = run.transport;
        self.stepping.push(ticks / (run.statistics.stepping_nanos as f64 / 1e9));
        self.wall.push(ticks / run.wall.as_secs_f64());
        self.first_divergent_tick = self.first_divergent_tick.or(divergence(&run.hashes, oracle));
        self.extraction_micros = self.extraction_micros.max(f64::from(run.statistics.extraction_micros));
        let Some(reception) = run.reception else { return };
        for (total, stream) in [(&mut self.state, STATE_STREAM), (&mut self.frame, FRAME_STREAM)] {
            let receipt = reception.report.streams[stream as usize];
            total.frames += receipt.frames;
            total.bytes += receipt.bytes;
            total.read_nanos += receipt.read_nanos;
            total.missing += receipt.missing;
            total.duplicates += receipt.duplicates;
        }
        self.torn_reads += reception.report.torn_reads;
        self.content_mismatches += reception.content_mismatches + reception.report.malformed_frames;
        self.stalled_at = self.stalled_at.or(reception.stalled_at);
        for stream in &reception.streams {
            self.frames_evicted += stream.frames_evicted;
            self.ticks_skipped += stream.ticks_skipped;
            self.frames_refused += stream.frames_refused;
            self.pauses += stream.pauses;
            self.ended |= !stream.ended.is_empty();
            if stream.id == STATE_STREAM {
                self.endpoint_copy_nanos += stream.copy_nanos;
                self.endpoint_frames += stream.frames_written;
            }
        }
    }

    fn write(&self, out: &mut Json, direct: f64) {
        out.object_start();
        out.string("way", self.name);
        out.string("transport", self.transport);
        out.string("data_plane", self.plane.map_or("none", |spec| spec.mode()));
        out.string("policy", self.plane.map_or("none", |spec| spec.state.name()));
        optional(out, "client_stalled_at", self.stalled_at);
        out.decimal("ticks_per_second", best(&self.stepping));
        out.decimal("ratio_to_direct", best(&self.stepping) / direct);
        out.decimal("agent_ticks_per_second", best(&self.wall));
        out.key("repetitions_ticks_per_second");
        out.array_start();
        for value in &self.stepping {
            out.object_start();
            out.decimal("ticks_per_second", *value);
            out.object_end();
        }
        out.array_end();
        out.bool("matches_oracle", self.first_divergent_tick.is_none());
        optional(out, "first_divergent_tick", self.first_divergent_tick);
        out.u64("state_frames", self.state.frames);
        out.u64("state_frames_missing", self.state.missing);
        out.u64("frame_frames", self.frame.frames);
        out.decimal("client_us_per_state_frame", self.state.micros_per_frame());
        out.decimal("client_us_per_frame_frame", self.frame.micros_per_frame());
        out.decimal("endpoint_copy_us_per_state_frame", if self.endpoint_frames == 0 { 0.0 } else { self.endpoint_copy_nanos as f64 / self.endpoint_frames as f64 / 1e3 });
        out.decimal("extraction_us_per_tick", self.extraction_micros);
        out.u64("frames_overwritten", self.frames_evicted);
        out.u64("ticks_skipped", self.ticks_skipped);
        out.u64("frames_refused_by_a_full_sink", self.frames_refused);
        out.u64("lossless_pauses", self.pauses);
        out.u64("torn_reads_discarded", self.torn_reads);
        out.u64("content_mismatches", self.content_mismatches);
        out.bool("a_stream_ended", self.ended);
        out.object_end();
    }
}

fn optional(out: &mut Json, key: &str, value: Option<u32>) {
    match value {
        Some(value) => out.u64(key, u64::from(value)),
        None => out.raw(key, "null"),
    }
}

/// A client that stops reading a lossless stream, then resumes by cursor.
struct StallRun {
    spec: PlaneSpec,
    transport: &'static str,
    first_divergent_tick: Option<u32>,
    reception: Reception,
    ticks: u32,
}

impl StallRun {
    fn holds(&self) -> bool {
        self.first_divergent_tick.is_none()
            && self.reception.ended_with_the_budget_error()
            && self.reception.resumed()
            && self.reception.lossless_complete(&self.spec, self.ticks)
            && self.reception.content_mismatches == 0
            && self.reception.report.malformed_frames == 0
            && self.reception.report.error.is_none()
    }
}

fn write_reception(out: &mut Json, spec: &PlaneSpec, reception: &Reception, ticks: u32) {
    out.string("data_plane", spec.mode());
    out.string("policy", spec.state.name());
    optional(out, "client_stalled_at", reception.stalled_at);
    optional(out, "streams_ended_at", reception.ended_at);
    out.string("ended_with", &reception.ended_codes.join(", "));
    optional(out, "resumed_at", reception.resumed_at);
    out.key("resumes");
    out.array_start();
    for (stream, cursor, server_cursor, result) in &reception.resumes {
        out.object_start();
        out.u64("stream", u64::from(*stream));
        out.u64("client_cursor", u64::from(*cursor));
        out.u64("server_cursor", u64::from(*server_cursor));
        out.string("result", result);
        out.object_end();
    }
    out.array_end();
    for (name, stream) in [("state", STATE_STREAM), ("frame", FRAME_STREAM)] {
        let receipt = reception.report.streams[stream as usize];
        out.u64(&format!("{name}_frames"), receipt.frames);
        out.u64(&format!("{name}_frames_missing"), receipt.missing);
        out.u64(&format!("{name}_frames_duplicated"), receipt.duplicates);
    }
    out.bool("every_frame_once", reception.lossless_complete(spec, ticks));
    out.u64("frames_checked_against_the_announced_hashes", reception.frames_checked);
    out.u64("content_mismatches", reception.content_mismatches + reception.report.malformed_frames);
    out.u64("lossless_pauses", reception.streams.iter().map(|stream| stream.pauses).sum());
    out.string("reader_error", reception.report.error.as_deref().unwrap_or(""));
}

/// The scripted agent's run with its oracle.
struct Commanded {
    run: CommandsRun,
    plane: PlaneSpec,
    replay: scenarios::Replay,
    first_divergent_tick: Option<u32>,
    ticks: u32,
}

impl Commanded {
    fn new(scale: &Scale, run: CommandsRun, plane: PlaneSpec) -> Commanded {
        let replay = replay(scale.size, scale.ticks, &run.record);
        let first_divergent_tick = divergence(&run.hashes, &replay.oracle_hashes);
        Commanded { run, plane, replay, first_divergent_tick, ticks: scale.ticks }
    }

    fn matches_oracle(&self) -> bool {
        self.first_divergent_tick.is_none() && self.replay.outcome_mismatches == 0 && self.replay.digest_mismatches == 0
    }

    fn data_plane_holds(&self) -> bool {
        self.run.reception.as_ref().is_some_and(|reception| reception.lossless_complete(&self.plane, self.ticks) && reception.content_mismatches == 0 && reception.report.malformed_frames == 0)
    }

    /// The log holds each command once: what the agent sent and nothing else.
    fn log_matches(&self) -> bool {
        self.run.statistics.log_commands == self.run.commands_sent && self.replay.commands == self.run.commands_sent
    }

    fn write(&self, out: &mut Json) {
        let run = &self.run;
        out.object_start();
        out.string("transport", run.transport);
        out.bool("matches_oracle", self.first_divergent_tick.is_none());
        optional(out, "first_divergent_tick", self.first_divergent_tick);
        out.hex("run_hash", run_hash(&run.hashes));
        out.u64("commands_sent", run.commands_sent);
        out.u64("commands_in_the_input_log", run.statistics.log_commands);
        out.decimal("log_bytes_per_tick", run.statistics.log_bytes as f64 / f64::from(self.ticks));
        out.u64("outcomes_compared", self.replay.commands);
        out.u64("outcome_mismatches", self.replay.outcome_mismatches);
        out.u64("digests_compared", self.replay.digests);
        out.u64("digest_mismatches", self.replay.digest_mismatches);
        out.u64("kicks_applied", u64::from(run.kicks_applied));
        out.u64("kicks_stale", u64::from(run.kicks_stale));
        out.key("data_plane");
        out.object_start();
        if let Some(reception) = &run.reception {
            write_reception(out, &self.plane, reception, self.ticks);
        }
        out.bool("holds", self.data_plane_holds());
        out.object_end();
        out.key("transactions");
        out.array_start();
        for mix in &run.mixes {
            out.object_start();
            out.string("kind", mix.kind.name());
            out.u64("size", mix.size as u64);
            out.u64("tick", u64::from(mix.tick));
            out.bool("applied_whole", mix.applied_whole && mix.changed_the_state);
            out.string("one_refused_verdicts", &format!("{} accepted, {} rejected, {} not evaluated", mix.one_refused_verdicts.0, mix.one_refused_verdicts.1, mix.one_refused_verdicts.2));
            out.string("refused_with", mix.refused_with);
            out.bool("one_refused_rejects_the_whole", mix.one_refused_as_expected && mix.one_refused_left_the_state);
            out.u64("refusals_planted", mix.planted as u64);
            out.u64("refusals_listed", mix.listed as u64);
            out.bool("full_evaluation_applies_nothing", mix.full_evaluation_left_the_state);
            out.bool("atomic", mix.atomic());
            out.u64("inverse_elements", mix.inverse_elements as u64);
            out.bool("inverse_applied", mix.inverse_applied);
            out.bool("inverse_restores_the_full_hash", mix.restored_full_hash);
            out.bool("inverse_restores_the_component_data", mix.restored_component_data);
            out.bool("inverse_restores_the_labels", mix.restored_labels);
            out.u64("spawns", mix.spawns as u64);
            out.u64("restores_over_a_reused_slot", mix.restores_over_a_reused_slot as u64);
            out.bool("original_applies_again", mix.reapplied);
            out.object_end();
        }
        out.array_end();
        let (probes, statistics) = (&run.probes, &run.statistics);
        out.key("sequence");
        out.object_start();
        out.u64("retries_sent", u64::from(probes.retries_sent));
        out.u64("retries_answered_alike", u64::from(probes.retries_answered_alike));
        out.u64("retries_answered_from_the_log", statistics.retries_answered);
        out.u64("gaps_sent", u64::from(probes.gaps_sent));
        out.u64("gaps_refused", u64::from(probes.gaps_refused_naming_the_expected));
        out.u64("altered_retries_sent", u64::from(probes.altered_sent));
        out.u64("altered_retries_refused_by_name", u64::from(probes.altered_refused_by_name));
        out.bool("probes_left_the_state", probes.probes_left_the_state == probes.rounds);
        out.bool("retry_outside_retention_refused", probes.outside_retention_refused);
        out.bool("log_holds_each_command_once", self.log_matches());
        out.bool("holds", probes.hold() && self.log_matches() && self.matches_oracle());
        out.object_end();
        out.object_end();
    }
}

struct PerWorkers {
    workers: usize,
    direct: Vec<f64>,
    direct_divergent: Option<u32>,
    throughput: Vec<Throughput>,
    /// Ways run once: their hashes and their streams matter, not their rate.
    single_runs: Vec<Throughput>,
    socket_reads: Vec<SocketRead>,
    stalls: Vec<StallRun>,
    commanded: Vec<Commanded>,
}

impl PerWorkers {
    fn way(&self, name: &str) -> &Throughput {
self.throughput.iter().chain(&self.single_runs).find(|way| way.name == name).expect("a measured way")
    }

    /// The socket read on the transport the ring's session uses.
    fn socket_read(&self) -> &SocketRead {
        &self.socket_reads[0]
    }

    fn client_cost_ratio(&self) -> f64 {
        self.way(RING_LOSSLESS).state.micros_per_frame() / self.socket_read().median_micros
    }

    fn lossless_loss(&self) -> f64 {
        1.0 - best(&self.way(RING_LOSSLESS).stepping) / best(&self.direct)
    }

    fn extraction_loss(&self) -> f64 {
        1.0 - best(&self.way(EXTRACTION_ONLY).stepping) / best(&self.direct)
    }

    fn stalled_loss(&self) -> f64 {
        1.0 - best(&self.way(RING_LATEST_STALLED).stepping) / best(&self.way(RING_LATEST).stepping)
    }
}

const SURFACE_ONLY: &str = "surface only";
const EXTRACTION_ONLY: &str = "ring without a stream (extraction only)";
const RING_LOSSLESS: &str = "ring, lossless";
const RING_LATEST: &str = "ring, latest-wins";
const RING_LATEST_STALLED: &str = "ring, latest-wins, client stalled";
const SECOND_LOSSLESS: &str = "second stream, lossless";
const SECOND_LATEST_STALLED: &str = "second stream, latest-wins, client stalled";
const SECOND_LOSSLESS_NOT_WAITING: &str = "second stream, lossless, client not waiting for its stream";

fn measure(scale: &Scale, workers: usize, oracle: &[u64]) -> Fallible<PerWorkers> {
    let forever = Stall::Forever(scale.stall_at);
    let mut throughput = vec![
        Throughput::new(SURFACE_ONLY, None),
        Throughput::new(EXTRACTION_ONLY, Some(plane(false, Policy::Off, false, Stall::Never))),
        Throughput::new(RING_LOSSLESS, Some(plane(false, Policy::Lossless, false, Stall::Never))),
        Throughput::new(RING_LATEST, Some(plane(false, Policy::LatestWins, false, Stall::Never))),
        Throughput::new(RING_LATEST_STALLED, Some(plane(false, Policy::LatestWins, false, forever))),
        Throughput::new(SECOND_LOSSLESS, Some(plane(true, Policy::Lossless, false, Stall::Never))),
    ];
    let mut single_runs = vec![
        Throughput::new(SECOND_LATEST_STALLED, Some(plane(true, Policy::LatestWins, false, forever))),
        Throughput::new(SECOND_LOSSLESS_NOT_WAITING, Some(PlaneSpec { follow: false, ..plane(true, Policy::Lossless, false, Stall::Never) })),
    ];
    let mut direct = Vec::new();
    let mut direct_divergent = None;
    for repetition in 0..scale.repetitions {
        eprintln!("{workers} workers: throughput, repetition {}", repetition + 1);
        let (hashes, stepping) = run_direct(scale, workers);
        direct.push(f64::from(scale.ticks) / stepping.as_secs_f64());
        direct_divergent = direct_divergent.or(divergence(&hashes, oracle));
        for way in &mut throughput {
            let run = run_plain(scale, workers, way.plane)?;
            way.add(scale, run, oracle);
        }
    }
    eprintln!("{workers} workers: socket reads, stalled clients, commands");
    for way in &mut single_runs {
        let run = run_plain(scale, workers, way.plane)?;
        way.add(scale, run, oracle);
    }
    let socket_reads = run_socket_reads(scale, workers)?;
    let mut stalls = Vec::new();
    for second_stream in [false, true] {
        let spec = plane(second_stream, Policy::Lossless, true, Stall::UntilEnded(scale.stall_at));
        let run = run_plain(scale, workers, Some(spec))?;
        let first_divergent_tick = divergence(&run.hashes, oracle);
        stalls.push(StallRun { spec, transport: run.transport, first_divergent_tick, reception: run.reception.expect("a run with a data plane"), ticks: scale.ticks });
    }
    let mut commanded = Vec::new();
    for second_stream in [false, true] {
        let spec = plane(second_stream, Policy::Lossless, true, Stall::Never);
        commanded.push(Commanded::new(scale, run_commands(scale, workers, Some(spec))?, spec));
    }
    Ok(PerWorkers { workers, direct, direct_divergent, throughput, single_runs, socket_reads, stalls, commanded })
}

struct Paced {
    run: PacedRun,
    spec: PlaneSpec,
    workers: usize,
    replay: scenarios::Replay,
    first_divergent_tick: Option<u32>,
    ticks: u32,
}

impl Paced {
    fn holds(&self) -> bool {
        let reception = &self.run.reception;
        self.first_divergent_tick.is_none()
            && self.replay.outcome_mismatches == 0
            && self.run.commands_never_applied == 0
            && self.run.statistics.log_commands == self.run.commands_sent
            && reception.ended_with_the_budget_error()
            && reception.resumed()
            && reception.lossless_complete(&self.spec, self.ticks)
            && reception.content_mismatches == 0
    }
}

struct Hypothesis {
    name: &'static str,
    holds: bool,
    refuted: bool,
    figure: String,
    threshold: &'static str,
}

fn hypotheses(runs: &[PerWorkers], paced: &Paced, costs: &[InverseCost]) -> Vec<Hypothesis> {
    let worst = |figure: fn(&PerWorkers) -> f64| runs.iter().map(figure).fold(f64::MIN, f64::max);
    let every = |check: fn(&PerWorkers) -> bool| runs.iter().all(check);
    let mixes = || runs.iter().flat_map(|run| &run.commanded).flat_map(|commanded| &commanded.run.mixes);

    let (ratio, loss) = (worst(PerWorkers::client_cost_ratio), worst(PerWorkers::lossless_loss));
    let ring_sound = every(|run| {
        let way = run.way(RING_LOSSLESS);
        way.first_divergent_tick.is_none() && way.state.missing == 0 && way.content_mismatches == 0 && run.direct_divergent.is_none()
    });
    let ring = Hypothesis {
        name: "the ring delivers the full extracted state at under a tenth of the socket read's cost to the client, and a lossless stream costs the world at most 2 percent of its throughput",
        holds: ring_sound && ratio < CLIENT_COST_TARGET && loss <= THROUGHPUT_LOSS_LIMIT,
        refuted: ratio > CLIENT_COST_REFUTED || loss > THROUGHPUT_LOSS_LIMIT,
        figure: format!(
            "client cost {:.3} of the socket read at worst, throughput loss {:.1} percent at worst (the extraction alone, with no stream: {:.1} percent at worst)",
            ratio,
            loss * 100.0,
            worst(PerWorkers::extraction_loss) * 100.0
        ),
        threshold: "under 0.1 of the socket read (above 0.5 refutes), at most 2 percent of the direct run's throughput",
    };

    let stalled_loss = worst(PerWorkers::stalled_loss);
    let stalled_hashes = every(|run| [RING_LATEST_STALLED, SECOND_LATEST_STALLED].iter().all(|name| run.way(name).first_divergent_tick.is_none() && !run.way(name).ended))
        && every(|run| run.stalls.iter().all(|stall| stall.first_divergent_tick.is_none()))
        && paced.first_divergent_tick.is_none();
    let lossless_resumes = every(|run| run.stalls.iter().all(StallRun::holds)) && paced.holds();
    let flow = Hypothesis {
        name: "a client that stops reading a latest-wins stream changes neither the hashes nor the throughput, and one that stops reading a lossless stream is ended at its budget with the structured error and resumes by cursor with every frame once",
        holds: stalled_hashes && lossless_resumes && stalled_loss <= THROUGHPUT_LOSS_LIMIT,
        refuted: !stalled_hashes,
        figure: format!(
            "hashes unchanged under every stalled client: {stalled_hashes}; throughput loss of the stalled latest-wins client {:.1} percent at worst; lossless streams ended at the budget and resumed with every frame once: {lossless_resumes}",
            stalled_loss * 100.0
        ),
        threshold: "no hash change (any refutes), at most 2 percent of the reading client's throughput, every frame of a lossless stream once",
    };

    let atomic = mixes().filter(|mix| mix.atomic()).count();
    let total = mixes().count();
    let commands_match = every(|run| run.commanded.iter().all(Commanded::matches_oracle));
    let untouched = mixes().all(|mix| mix.one_refused_left_the_state && mix.full_evaluation_left_the_state);
    let transactions = Hypothesis {
        name: "every transaction applies entirely or not at all with the oracle's hashes, and the full-evaluation mode lists every rejection without applying anything",
        holds: total > 0 && atomic == total && commands_match,
        refuted: !untouched || !commands_match,
        figure: format!("{atomic} of {total} transaction kinds and sizes atomic across the runs; hashes, outcomes and digests equal the oracle's: {commands_match}"),
        threshold: "every transaction whole or absent (one applied in part refutes), at 1 and 4 workers",
    };

    let full = mixes().filter(|mix| mix.inverse_applied && mix.restored_full_hash).count();
    let data = mixes().filter(|mix| mix.inverse_applied && mix.restored_component_data && mix.restored_labels).count();
    let costly: Vec<String> = costs.iter().filter(|cost| cost.ratio() >= INVERSE_COST_LIMIT).map(|cost| format!("{} of {}", cost.kind.name(), cost.size)).collect();
    let dearest = costs.iter().map(InverseCost::ratio).fold(f64::MIN, f64::max);
    let inverse = Hypothesis {
        name: "applying the inverse transaction returns the hash to its value before the original, and computing the inverse costs under 10 percent of applying the transaction",
        holds: total > 0 && full == total && costly.is_empty(),
        refuted: full != total,
        figure: format!(
            "full hash restored for {full} of {total}, component data and labels for {data} of {total} (a spawn undone by a despawn leaves the slot's generation moved); inverse cost {:.0} percent of the application at worst, at or above 10 percent for {} of {} kinds and sizes",
            dearest * 100.0,
            costly.len(),
            costs.len()
        ),
        threshold: "the hash restored for every kind and size (one not restored refutes the inverse as the undo base), under 10 percent of the application (above makes the inverse an opt-in)",
    };

    let probes_hold = every(|run| run.commanded.iter().all(|commanded| commanded.run.probes.hold() && commanded.log_matches()));
    let paced_log = paced.run.statistics.log_commands == paced.run.commands_sent && paced.replay.outcome_mismatches == 0 && paced.first_divergent_tick.is_none();
    let answered: u64 = runs.iter().flat_map(|run| &run.commanded).map(|commanded| commanded.run.statistics.retries_answered).sum::<u64>() + paced.run.statistics.retries_answered;
    let sequence = Hypothesis {
        name: "a retried sequence is answered from the input log, a skipped sequence is refused, an altered retry is refused by name, and the hashes equal the oracle's throughout",
        holds: probes_hold && paced_log && commands_match,
        refuted: !(commands_match && paced_log) || runs.iter().flat_map(|run| &run.commanded).any(|commanded| commanded.run.probes.gaps_refused_naming_the_expected != commanded.run.probes.gaps_sent || !commanded.log_matches()),
        figure: format!("{answered} retries answered from the log; every probe handled as designed: {probes_hold}; the log holds each command once and the hashes equal the oracle's: {}", paced_log && commands_match),
        threshold: "no retry applied twice, no gap admitted (either refutes)",
    };
    vec![ring, flow, transactions, inverse, sequence]
}

fn report(cell: &str, scale: &Scale, worker_counts: &[usize]) -> Fallible<(String, Vec<Hypothesis>)> {
    eprintln!("the sequential oracle without commands");
    let mut plain = Oracle::new(scale.size);
    plain.step_to(scale.ticks);
    let mut runs = Vec::new();
    for &workers in worker_counts {
        runs.push(measure(scale, workers, &plain.hashes)?);
    }
    let paced_workers = *worker_counts.last().expect("a worker count");
    eprintln!("the paced session at {paced_workers} workers");
    let spec = plane(false, Policy::Lossless, true, Stall::UntilEnded(scale.stall_at));
    let run = run_paced(scale, paced_workers, spec)?;
    let paced_replay = replay(scale.size, scale.ticks, &run.record);
    let first_divergent_tick = divergence(&run.hashes, &paced_replay.oracle_hashes);
    let paced = Paced { run, spec, workers: paced_workers, replay: paced_replay, first_divergent_tick, ticks: scale.ticks };
    eprintln!("the inverse's cost");
    let costs = inverse_cost(scale.size, INVERSE_COST_ROUNDS);
    let verdicts = hypotheses(&runs, &paced, &costs);

    let mut out = Json::new();
    out.object_start();
    out.string("spike", SPIKE);
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.key("parameters");
    out.object_start();
    out.string("size", scale.size.name());
    out.u64("entity_count", scale.size.entity_count() as u64);
    out.u64("tick_count", u64::from(scale.ticks));
    out.u64("batch", u64::from(BATCH));
    out.u64("repetitions", scale.repetitions as u64);
    out.u64("ring_bytes", scale.plane.ring_capacity as u64);
    out.u64("frame_stand_in_bytes", scale.plane.frame_bytes as u64);
    out.u64("unacknowledged_budget_ticks", u64::from(scale.plane.budget));
    out.u64("extraction_retention_ticks", scale.plane.retention as u64);
    out.u64("input_log_retention_ticks", u64::from(scale.log_retention));
    out.u64("client_stalls_at_tick", u64::from(scale.stall_at));
    out.decimal("pace_ms", PACE.as_secs_f64() * 1e3);
    out.hex("oracle_run_hash_without_commands", run_hash(&plain.hashes));
    out.object_end();

    out.key("runs");
    out.array_start();
    for run in &runs {
        out.object_start();
        out.u64("workers", run.workers as u64);
        out.key("direct");
        out.object_start();
        out.decimal("ticks_per_second", best(&run.direct));
        out.key("repetitions_ticks_per_second");
        out.array_start();
        for value in &run.direct {
            out.object_start();
            out.decimal("ticks_per_second", *value);
            out.object_end();
        }
        out.array_end();
        out.bool("matches_oracle", run.direct_divergent.is_none());
        out.object_end();
        for (key, ways) in [("throughput", &run.throughput), ("single_runs", &run.single_runs)] {
            out.key(key);
            out.array_start();
            for way in ways {
                way.write(&mut out, best(&run.direct));
            }
            out.array_end();
        }
        out.key("socket_reads");
        out.array_start();
        for read in &run.socket_reads {
            out.object_start();
            out.string("transport", read.transport);
            out.u64("reads", u64::from(read.reads));
            out.u64("state_bytes", read.state_bytes as u64);
            out.decimal("median_us", read.median_micros);
            out.decimal("mean_us", read.mean_micros);
            out.decimal("client_hash_us", read.hash_micros);
            out.bool("every_read_hashes_to_the_tick", read.every_read_hashes_to_the_tick);
            out.object_end();
        }
        out.array_end();
        out.key("ring_client");
        out.object_start();
        out.decimal("us_per_state_frame", run.way(RING_LOSSLESS).state.micros_per_frame());
        out.decimal("socket_read_median_us", run.socket_read().median_micros);
        out.string("socket_read_transport", run.socket_read().transport);
        out.decimal("ratio_to_the_socket_read", run.client_cost_ratio());
        out.decimal("lossless_throughput_loss_percent", run.lossless_loss() * 100.0);
        out.decimal("extraction_only_throughput_loss_percent", run.extraction_loss() * 100.0);
        out.decimal("stalled_latest_wins_throughput_loss_percent", run.stalled_loss() * 100.0);
        out.object_end();
        out.key("stalled_lossless_clients");
        out.array_start();
        for stall in &run.stalls {
            out.object_start();
            out.string("transport", stall.transport);
            write_reception(&mut out, &stall.spec, &stall.reception, stall.ticks);
            out.bool("matches_oracle", stall.first_divergent_tick.is_none());
            optional(&mut out, "first_divergent_tick", stall.first_divergent_tick);
            out.bool("holds", stall.holds());
            out.object_end();
        }
        out.array_end();
        out.key("commands");
        out.array_start();
        for commanded in &run.commanded {
            commanded.write(&mut out);
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();

    out.key("paced");
    out.object_start();
    out.u64("workers", paced.workers as u64);
    out.string("transport", paced.run.transport);
    out.u64("host_steps", u64::from(paced.run.host_steps));
    out.decimal("host_ticks_per_second", paced.run.host_ticks_per_second);
    out.bool("matches_oracle", paced.first_divergent_tick.is_none());
    optional(&mut out, "first_divergent_tick", paced.first_divergent_tick);
    out.u64("commands_sent", paced.run.commands_sent);
    out.u64("commands_in_the_input_log", paced.run.statistics.log_commands);
    out.u64("commands_never_applied", paced.run.commands_never_applied as u64);
    out.u64("results_answered_from_the_log", paced.run.statistics.retries_answered);
    out.u64("outcome_mismatches", paced.replay.outcome_mismatches);
    out.u64("kicks_applied", u64::from(paced.run.kicks_applied));
    out.u64("kicks_stale", u64::from(paced.run.kicks_stale));
    write_reception(&mut out, &paced.spec, &paced.run.reception, paced.ticks);
    out.bool("holds", paced.holds());
    out.object_end();

    out.key("inverse_cost");
    out.array_start();
    for cost in &costs {
        out.object_start();
        out.string("kind", cost.kind.name());
        out.u64("size", cost.size as u64);
        out.u64("transactions_per_round", cost.transactions as u64);
        out.decimal("apply_ns", cost.apply_nanos);
        out.decimal("apply_with_inverse_ns", cost.apply_with_inverse_nanos);
        out.decimal("inverse_percent_of_apply", cost.ratio() * 100.0);
        out.object_end();
    }
    out.array_end();

    out.key("hypotheses");
    out.array_start();
    for hypothesis in &verdicts {
        out.object_start();
        out.string("hypothesis", hypothesis.name);
        out.string("verdict", if hypothesis.holds { "holds" } else { "fails" });
        out.bool("refuted_by_its_threshold", hypothesis.refuted);
        out.string("figure", &hypothesis.figure);
        out.string("threshold", hypothesis.threshold);
        out.object_end();
    }
    out.array_end();
    out.object_end();
    Ok((out.finish(), verdicts))
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unlabeled".to_string());
    match report(&cell, &full_scale(), &WORKER_COUNTS) {
        Ok((json, verdicts)) => {
            for hypothesis in &verdicts {
                eprintln!("{}: {} ({})", if hypothesis.holds { "holds" } else { "fails" }, hypothesis.name, hypothesis.figure);
            }
            println!("{json}");
        }
        Err(error) => {
            eprintln!("{SPIKE}: the run could not complete: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_scale() -> Scale {
        let plane = PlaneConfig { ring_capacity: 256 << 10, frame_bytes: 16 << 10, budget: 24, retention: 96 };
        // Small ticks are shorter than the reader's polling period: the clients
        // of lossless streams wait for their reader, and the budget exceeds the
        // ten ticks the world advances within one step.
        Scale { size: Size::Small, ticks: 200, plane, log_retention: 100, stall_at: 60, socket_reads: 10, repetitions: 1, ring_clients_wait: true }
    }

    /// The whole report on the small size: every check that does not depend
    /// on a timing holds.
    #[test]
    fn the_report_runs_and_every_check_that_is_not_a_timing_holds() {
        let scale = small_scale();
        let mut plain = Oracle::new(scale.size);
        plain.step_to(scale.ticks);
        for workers in [1, 3] {
            let run = measure(&scale, workers, &plain.hashes).unwrap();
            assert!(run.direct_divergent.is_none());
            for way in run.throughput.iter().chain(&run.single_runs) {
                assert_eq!(way.first_divergent_tick, None, "{}", way.name);
                assert_eq!(way.content_mismatches, 0, "{}", way.name);
            }
            let lossless = run.way(RING_LOSSLESS);
            assert_eq!((lossless.state.frames, lossless.state.missing, lossless.frame.frames, lossless.ended), (200, 0, 200, false), "a client that waits for its reader is never ended at the budget");
            let second = run.way(SECOND_LOSSLESS);
            assert_eq!((second.state.frames, second.state.missing, second.frame.frames, second.ended), (200, 0, 200, false));
            for name in [RING_LATEST_STALLED, SECOND_LATEST_STALLED] {
                let stalled = run.way(name);
                assert!(stalled.state.frames < 200 && !stalled.ended && stalled.stalled_at == Some(60), "{name}");
            }
            assert!(run.way(RING_LATEST_STALLED).frames_evicted > 0);
            assert!(run.socket_reads.iter().all(|read| read.every_read_hashes_to_the_tick && read.state_bytes > 20_000));
            for stall in &run.stalls {
                assert!(stall.holds(), "{} at {workers} workers: {:?}", stall.spec.mode(), stall.reception);
                assert!(stall.reception.resumes.iter().all(|resume| resume.1 == resume.2), "the client's cursor is the server's");
            }
            for commanded in &run.commanded {
                assert!(commanded.matches_oracle(), "{}: {:?} {:?}", commanded.plane.mode(), commanded.first_divergent_tick, commanded.replay);
                assert!(commanded.data_plane_holds(), "{}", commanded.plane.mode());
                assert!(commanded.log_matches());
                assert!(commanded.run.probes.hold(), "{:?}", commanded.run.probes);
                assert_eq!(commanded.run.mixes.len(), 15);
                for mix in &commanded.run.mixes {
                    assert!(mix.atomic(), "{mix:?}");
                    assert!(mix.inverse_applied && mix.restored_component_data && mix.restored_labels && mix.reapplied, "{mix:?}");
                    assert_eq!(mix.restored_full_hash, mix.spawns == mix.restores_over_a_reused_slot, "{mix:?}");
                }
                assert!(commanded.run.kicks_applied > 0);
            }
        }
    }

    #[test]
    fn the_paced_session_resumes_its_lossless_stream_and_matches_its_oracle() {
        let scale = small_scale();
        let spec = plane(false, Policy::Lossless, true, Stall::UntilEnded(scale.stall_at));
        let run = run_paced(&scale, 2, spec).unwrap();
        let paced_replay = replay(scale.size, scale.ticks, &run.record);
        let first_divergent_tick = divergence(&run.hashes, &paced_replay.oracle_hashes);
        let paced = Paced { run, spec, workers: 2, replay: paced_replay, first_divergent_tick, ticks: scale.ticks };
        assert!(paced.holds(), "{:?} {:?} {:?}", paced.first_divergent_tick, paced.replay.outcome_mismatches, paced.run.reception);
        assert!(paced.run.kicks_applied > 0 && paced.run.statistics.retries_answered >= paced.run.commands_sent);
    }

    #[test]
    fn the_inverse_cost_is_measured_for_every_kind_and_size() {
        let costs = inverse_cost(Size::Small, 3);
        assert_eq!(costs.len(), 15);
        assert!(costs.iter().all(|cost| cost.apply_nanos > 0.0 && cost.apply_with_inverse_nanos > 0.0));
    }

    #[test]
    fn the_whole_report_is_one_document_with_five_verdicts() {
        let (json, verdicts) = report("test", &small_scale(), &[2]).unwrap();
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert_eq!(verdicts.len(), 5);
        assert!(json.contains("\"hypotheses\":[{"));
        assert!(!verdicts[2].refuted && !verdicts[4].refuted, "transactions and the sequence are not timing matters");
    }
}
