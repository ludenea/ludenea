//! Reference workload lockstep spike, for the networking and multiplayer
//! research. Throwaway, never product code.
//!
//! Two instances of the reference workload in deterministic lockstep over a
//! simulated link with latency, jitter and loss, each peer owning one input
//! stream of kicks, the inputs merged in a canonical order by source identity
//! and applied at the tick they were issued for plus an input delay. The spike
//! measures, instead of assuming, whether the two worlds' per-tick hashes stay
//! identical at every worker count for input delays of 2 and 4 ticks and a
//! loss rate up to 10 percent, whether they are the sequential oracle's hashes
//! for the merged records, whether a divergence injected in one peer is
//! detected by the hash exchange at its tick and named by its first divergent
//! system from the two retained states, how many frames a peer stalls per link
//! setting, and what a rollback variant costs per rollback.

use std::thread;
use std::time::Duration;

use reference_workload::{DEFAULT_TICK_COUNT, Json, run_hash, write_platform_and_build};
use workload_lockstep::lockstep::{DivergenceReport, KICKS_PER_TICK, LinkSettings, Outcome, Settings, run};
use workload_lockstep::world::{GROUP_NAMES, GROUP_SYSTEMS, Size, WorldConfig, oracle};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-lockstep";
const DELAYS: [u32; 2] = [2, 4];
/// Latency in frames, jitter maximum in frames, loss probability.
const LINKS: [(u32, u32, f32); 5] = [(0, 0, 0.0), (1, 0, 0.0), (2, 1, 0.02), (4, 1, 0.10), (6, 2, 0.10)];
const DIVERGENCE_LINK: (u32, u32, f32) = (1, 0, 0.0);
const DIVERGENCE_RING: usize = 16;
const ROLLBACK_LINKS: [(u32, u32, f32); 3] = [(1, 0, 0.0), (3, 1, 0.0), (5, 1, 0.05)];
const ROLLBACK_RING: usize = 8;
const LINK_SEED: u64 = 0x4c69_6e6b_5365_6564;

fn worker_counts() -> Vec<usize> {
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = vec![1, 4];
    if !counts.contains(&available) {
        counts.push(available);
        counts.sort_unstable();
    }
    counts
}

fn harness_workers() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get()).min(4)
}

#[derive(Clone, Copy)]
struct Scenario {
    size: Size,
    ticks: u32,
    divergence_tick: u32,
}

const MEASURED: Scenario = Scenario { size: Size::Default, ticks: DEFAULT_TICK_COUNT, divergence_tick: 500 };

fn link(setting: (u32, u32, f32)) -> LinkSettings {
    LinkSettings { latency: setting.0, jitter_max: setting.1, loss: setting.2 }
}

fn session_settings(scenario: &Scenario, workers: usize, delay: u32, setting: (u32, u32, f32), seed_index: u64) -> Settings {
    Settings {
        config: WorldConfig::new(scenario.size, workers),
        ticks: scenario.ticks,
        delay,
        link: link(setting),
        link_seed: LINK_SEED.wrapping_add(seed_index),
        ring_depth: 0,
        injection: None,
        rollback_peer: false,
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// The oracle's hashes per input delay, computed once from the first run's
/// records: the inputs are values drawn from the peers' streams against
/// states the lockstep keeps identical, so every run of one delay applies the
/// same records whatever the link and the worker count.
struct Oracles {
    hashes: Vec<(u32, Vec<u64>)>,
}

impl Oracles {
    fn run_hash_for(&mut self, scenario: &Scenario, outcome: &Outcome, delay: u32) -> u64 {
        if let Some((_, hashes)) = self.hashes.iter().find(|(d, _)| *d == delay) {
            return run_hash(hashes);
        }
        let hashes = oracle(&WorldConfig::new(scenario.size, 1), &outcome.records);
        let hash = run_hash(&hashes);
        self.hashes.push((delay, hashes));
        hash
    }
}

fn matches_oracle(outcome: &Outcome, oracle_run_hash: u64) -> bool {
    outcome.peers.iter().all(|peer| peer.run_hash == oracle_run_hash)
}

fn write_settings(out: &mut Json, settings: &Settings) {
    out.u64("workers", settings.config.workers as u64);
    out.u64("delay", u64::from(settings.delay));
    out.u64("latency", u64::from(settings.link.latency));
    out.u64("jitter_max", u64::from(settings.link.jitter_max));
    out.decimal("loss", f64::from(settings.link.loss));
}

fn write_link(out: &mut Json, settings: &Settings, outcome: &Outcome) {
    out.u64("frames", outcome.frames);
    out.u64("stall_frames_peer0", outcome.peers[0].stall_frames);
    out.u64("stall_frames_peer1", outcome.peers[1].stall_frames);
    out.u64("messages_sent", outcome.link.sent);
    out.u64("messages_delivered", outcome.link.delivered);
    out.u64("messages_dropped", outcome.link.dropped);
    out.u64("bytes_sent", outcome.link.bytes);
    out.decimal("bytes_per_tick_per_peer", outcome.link.bytes as f64 / (2.0 * f64::from(settings.ticks)));
    out.decimal("elapsed_ms", ms(outcome.elapsed));
    out.decimal("ticks_per_second_both_worlds", f64::from(outcome.peers[0].confirmed + outcome.peers[1].confirmed) / outcome.elapsed.as_secs_f64());
    out.decimal("stepping_ms_peer0", ms(outcome.peers[0].stepping));
    out.decimal("stepping_ms_peer1", ms(outcome.peers[1].stepping));
}

fn write_lockstep_run(out: &mut Json, settings: &Settings, outcome: &Outcome, oracle_run_hash: u64) {
    out.object_start();
    write_settings(out, settings);
    out.bool("hashes_identical", outcome.hashes_identical);
    out.i64("first_divergent_tick", outcome.first_divergent_tick.map_or(-1, i64::from));
    out.bool("matches_oracle", matches_oracle(outcome, oracle_run_hash));
    out.bool("divergence_detected", outcome.divergence.is_some());
    out.u64("final_tick_peer0", u64::from(outcome.peers[0].final_tick));
    out.u64("final_tick_peer1", u64::from(outcome.peers[1].final_tick));
    out.hex("final_hash", outcome.peers[0].final_hash);
    out.hex("run_hash", outcome.peers[0].run_hash);
    out.hex("run_hash_peer1", outcome.peers[1].run_hash);
    out.u64("kicks_applied_peer0", outcome.peers[0].applied);
    out.u64("kicks_stale_peer0", outcome.peers[0].stale);
    out.u64("hashes_compared_peer0", outcome.peers[0].hashes_compared);
    out.u64("hashes_compared_peer1", outcome.peers[1].hashes_compared);
    write_link(out, settings, outcome);
    out.object_end();
}

fn write_divergence(out: &mut Json, key: &str, settings: &Settings, outcome: &Outcome, oracle_run_hash: Option<u64>) {
    out.key(key);
    out.object_start();
    out.bool("detected", outcome.divergence.is_some());
    out.bool("hashes_identical", outcome.hashes_identical);
    out.i64("first_divergent_tick_after_the_run", outcome.first_divergent_tick.map_or(-1, i64::from));
    if let Some(hash) = oracle_run_hash {
        out.bool("matches_oracle", matches_oracle(outcome, hash));
    }
    if let Some(kick) = outcome.peers[1].injected_kick {
        out.key("injected_kick");
        out.object_start();
        out.u64("index", u64::from(kick.index));
        out.u64("generation", u64::from(kick.generation));
        out.decimal("delta_vx", f64::from(kick.delta_vx));
        out.decimal("delta_vy", f64::from(kick.delta_vy));
        out.object_end();
    }
    if let Some(report) = &outcome.divergence {
        write_divergence_report(out, report);
    }
    let snapshots = outcome.peers[0].snapshots_taken + outcome.peers[1].snapshots_taken;
    out.u64("snapshots_taken", snapshots);
    out.decimal("snapshot_us_each", if snapshots == 0 { 0.0 } else { (outcome.peers[0].snapshot_time + outcome.peers[1].snapshot_time).as_secs_f64() * 1_000_000.0 / snapshots as f64 });
    write_link(out, settings, outcome);
    out.object_end();
}

fn write_divergence_report(out: &mut Json, report: &DivergenceReport) {
    out.u64("detected_tick", u64::from(report.tick));
    out.u64("detected_at_frame", report.detected_at_frame);
    out.string("detected_by", &report.detected_by.iter().map(|p| format!("peer {p}")).collect::<Vec<_>>().join(", "));
    out.string("first_divergent_group", report.group_name());
    out.string("first_divergent_system", report.first_divergent_group.map_or("none", |g| GROUP_SYSTEMS[g]));
    match report.snapshot_bytes {
        Some([a, b]) => {
            out.u64("snapshot_bytes_peer0", a as u64);
            out.u64("snapshot_bytes_peer1", b as u64);
            out.bool("snapshots_identical", report.snapshots_identical.unwrap_or(true));
            out.i64("first_differing_byte", report.first_differing_byte.map_or(-1, |b| b as i64));
        }
        None => out.string("snapshots", "not retained at the detected tick"),
    }
    if let Some(hashes) = report.group_hashes {
        for (peer, hashes) in hashes.iter().enumerate() {
            out.key(&format!("group_hashes_peer{peer}"));
            out.object_start();
            for (group, hash) in GROUP_NAMES.iter().zip(hashes) {
                out.hex(group, *hash);
            }
            out.object_end();
        }
    }
}

fn write_rollback_run(out: &mut Json, settings: &Settings, outcome: &Outcome, oracle_run_hash: u64) {
    let peer = &outcome.peers[1];
    let rollback = &peer.rollback;
    out.object_start();
    write_settings(out, settings);
    out.u64("ring_depth", settings.ring_depth as u64);
    out.u64("rollbacks", rollback.rollbacks);
    out.u64("predicted_ticks", rollback.predicted_ticks);
    out.u64("mispredicted_ticks", rollback.mispredicted_ticks);
    out.u64("resimulated_ticks", rollback.resimulated_ticks);
    out.decimal("resimulated_ticks_mean", if rollback.rollbacks == 0 { 0.0 } else { rollback.resimulated_ticks as f64 / rollback.rollbacks as f64 });
    out.u64("resimulated_ticks_max", rollback.resimulated_max);
    out.decimal("rollback_ms_mean", if rollback.rollbacks == 0 { 0.0 } else { ms(rollback.time) / rollback.rollbacks as f64 });
    out.decimal("rollback_ms_max", ms(rollback.time_max));
    out.decimal("rollback_ms_total", ms(rollback.time));
    out.u64("confirmed_tick", u64::from(peer.confirmed));
    out.u64("final_tick_rollback_peer", u64::from(peer.final_tick));
    out.bool("confirmed_hashes_match", outcome.hashes_identical && outcome.divergence.is_none());
    out.u64("hashes_compared", peer.hashes_compared);
    out.bool("matches_oracle", matches_oracle(outcome, oracle_run_hash));
    out.hex("run_hash", peer.run_hash);
    out.u64("snapshots_taken", peer.snapshots_taken);
    out.decimal("snapshot_us_each", if peer.snapshots_taken == 0 { 0.0 } else { peer.snapshot_time.as_secs_f64() * 1_000_000.0 / peer.snapshots_taken as f64 });
    write_link(out, settings, outcome);
    out.object_end();
}

fn report(out: &mut Json, scenario: &Scenario) {
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", scenario.size.name());
    out.u64("ticks", u64::from(scenario.ticks));
    out.u64("kicks_per_tick_per_peer", KICKS_PER_TICK as u64);
    let mut oracles = Oracles { hashes: Vec::new() };

    out.key("lockstep");
    out.array_start();
    for workers in worker_counts() {
        for delay in DELAYS {
            for (index, setting) in LINKS.iter().enumerate() {
                let settings = session_settings(scenario, workers, delay, *setting, index as u64);
                let outcome = run(&settings);
                let oracle_run_hash = oracles.run_hash_for(scenario, &outcome, delay);
                write_lockstep_run(out, &settings, &outcome, oracle_run_hash);
            }
        }
    }
    out.array_end();

    let delay = DELAYS[0];
    let mut settings = session_settings(scenario, harness_workers(), delay, DIVERGENCE_LINK, 100);
    settings.ring_depth = DIVERGENCE_RING;
    out.key("divergence");
    out.object_start();
    out.u64("injected_tick", u64::from(scenario.divergence_tick));
    out.u64("ring_depth", DIVERGENCE_RING as u64);
    write_settings(out, &settings);
    let oracle_run_hash = oracles.hashes.iter().find(|(d, _)| *d == delay).map(|(_, hashes)| run_hash(hashes));
    settings.injection = Some(scenario.divergence_tick);
    let injected = run(&settings);
    write_divergence(out, "with_injection", &settings, &injected, None);
    settings.injection = None;
    let clean = run(&settings);
    write_divergence(out, "without_injection", &settings, &clean, oracle_run_hash);
    out.object_end();

    out.key("rollback");
    out.array_start();
    for (index, setting) in ROLLBACK_LINKS.iter().enumerate() {
        let mut settings = session_settings(scenario, harness_workers(), delay, *setting, 200 + index as u64);
        settings.ring_depth = ROLLBACK_RING;
        settings.rollback_peer = true;
        let outcome = run(&settings);
        let oracle_run_hash = oracles.run_hash_for(scenario, &outcome, delay);
        write_rollback_run(out, &settings, &outcome, oracle_run_hash);
    }
    out.array_end();
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
        report(&mut out, &Scenario { size: Size::Small, ticks: 40, divergence_tick: 20 });
        out.object_end();
        let json = out.finish();
        assert!(json.starts_with('{') && json.ends_with('}'));
        for check in ["matches_oracle", "confirmed_hashes_match"] {
            assert!(!json.contains(&format!("\"{check}\":false")), "{check} fails: {json}");
        }
        // Every hash check holds except the injected divergence, which is the
        // one run whose hashes differ and the one detection.
        assert_eq!(json.matches("\"hashes_identical\":false").count(), 1, "{json}");
        assert_eq!(json.matches("\"detected\":true").count(), 1, "{json}");
        assert_eq!(json.matches("\"divergence_detected\":true").count(), 0, "{json}");
        assert!(json.contains("\"detected_tick\":20"), "{json}");
        assert!(json.contains("\"first_divergent_group\":\"input_and_repulsion_velocities\""), "{json}");
        assert!(json.contains("\"snapshots_identical\":false"), "{json}");
        assert!(json.contains("\"rollbacks\":0"), "{json}");
        assert!(!json.contains("not retained"), "{json}");
    }
}
