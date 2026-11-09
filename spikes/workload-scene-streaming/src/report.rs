//! The report: one JSON document on standard output (the facts the research
//! CI publishes and `summarize.sh` reads) and the per-tick log written to a
//! file beside it. Throwaway, never product code.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use reference_workload::{Json, write_platform_and_build};

use crate::{
    HYSTERESIS_SAVING_LIMIT, LIVE_BATCH, ModeRun, REMAP_DIFFERENCE_LIMIT, Report, SPIKE, STRUCTURAL_SHARE_LIMIT, ScenarioReport, Settings,
    grid::DEFAULT_CELL_SIDE,
};

const SCHEMA_VERSION: u64 = 1;

/// The per-tick log: one section per run (its name, its settings, its phase
/// times), then one line per tick (the tick, its hash, the structural node's
/// nanoseconds, the structural changes' nanoseconds, the tick's wall
/// nanoseconds, the entities spawned and despawned, the loaded cells).
pub struct TickLog {
    writer: Option<BufWriter<File>>,

    pub lines: u64,
}

impl TickLog {
    pub fn new(path: Option<PathBuf>) -> TickLog {
        let writer = path.as_ref().and_then(|path| match File::create(path) {
            Ok(file) => Some(BufWriter::new(file)),
            Err(error) => {
                eprintln!("the per-tick log at {} cannot be created ({error}); the report carries the summary alone", path.display());
                None
            }
        });
        TickLog { writer, lines: 0 }
    }

    pub fn record(&mut self, run: &ModeRun) {
        let Some(writer) = self.writer.as_mut() else { return };
        let mut write = || -> std::io::Result<()> {
            writeln!(
                writer,
                "# run {} loader={} workers={} ticks={} wall_ms={:.3} final_hash={:#018x} run_hash={:#018x} matches_oracle={} first_divergent_tick={} loads={} unloads={} arrivals={} rejected={} reads_absent={} reads_present={} reads_stale={} max_remap_entries={} max_loaded_cells={}",
                run.scenario.name(),
                run.loader.name(),
                run.workers,
                run.ticks,
                run.wall_ms(),
                run.final_hash(),
                run.run_hash(),
                run.matches_oracle,
                run.first_divergent_tick.map_or("none".to_string(), |t| t.to_string()),
                run.log.accepted_loads(),
                run.log.accepted_unloads(),
                run.log.accepted_arrivals(),
                run.log.rejected(),
                run.reads.absent,
                run.reads.present,
                run.reads.stale,
                run.max_remap_entries,
                run.max_loaded_cells
            )?;
            let phases: Vec<String> = run.phase_ms().iter().map(|(name, ms)| format!("{name}={ms:.3}")).collect();
            let i = &run.instruments;
            writeln!(
                writer,
                "# phase_ms {} changes={:.3} spawn={:.3} despawn={:.3} policy={:.3} input={:.3} fetch={:.3} spawned={} despawned={}",
                phases.join(" "),
                i.changes_ns as f64 / 1e6,
                i.spawn_ns as f64 / 1e6,
                i.despawn_ns as f64 / 1e6,
                i.policy_ns as f64 / 1e6,
                i.input_ns as f64 / 1e6,
                i.fetch_ns as f64 / 1e6,
                i.spawned_entities,
                i.despawned_entities
            )?;
            writeln!(writer, "# tick hash structural_ns changes_ns tick_wall_ns spawned despawned loaded_cells")?;
            for t in 0..i.structural_ns.len() {
                writeln!(
                    writer,
                    "{} {:#018x} {} {} {} {} {} {}",
                    t + 1,
                    run.hashes[t + 1],
                    i.structural_ns[t],
                    i.changes_per_tick_ns[t],
                    i.tick_wall_ns[t],
                    i.spawned_per_tick[t],
                    i.despawned_per_tick[t],
                    i.loaded_per_tick[t]
                )?;
            }
            Ok(())
        };
        match write() {
            Ok(()) => self.lines += 3 + run.instruments.structural_ns.len() as u64,
            Err(error) => eprintln!("writing the per-tick log failed: {error}"),
        }
    }

    pub fn finish(&mut self) {
        if let Some(mut writer) = self.writer.take()
            && let Err(error) = writer.flush()
        {
            eprintln!("flushing the per-tick log failed: {error}");
        }
    }
}

fn write_option_usize(out: &mut Json, key: &str, value: Option<usize>) {
    match value {
        Some(value) => out.u64(key, value as u64),
        None => out.string(key, "none"),
    }
}

fn write_run(out: &mut Json, run: &ModeRun) {
    out.object_start();
    out.string("scenario", &run.scenario.name());
    out.u64("grid", u64::from(run.scenario.grid));
    out.u64("cells", u64::from(run.scenario.grid * run.scenario.grid));
    out.u64("density", u64::from(run.scenario.density));
    out.u64("ticks_per_cell", u64::from(run.scenario.ticks_per_cell));
    out.bool("hysteresis", run.scenario.hysteresis);
    out.bool("sliced", run.scenario.sliced);
    out.bool("mirrored_path", run.scenario.mirrored);
    out.string("loader", run.loader.name());
    out.u64("workers", run.workers as u64);
    out.u64("ticks", u64::from(run.ticks));
    out.hex("final_hash", run.final_hash());
    out.hex("run_hash", run.run_hash());
    out.bool("matches_oracle", run.matches_oracle);
    write_option_usize(out, "first_divergent_tick", run.first_divergent_tick);
    out.u64("reads_absent", run.reads.absent);
    out.u64("reads_present", run.reads.present);
    out.u64("stale_slots", run.reads.stale);
    out.bool("reads_equal_oracle", run.reads == run.oracle.reads);
    out.u64("oracle_check_failures", run.oracle.check_failures);
    out.u64("max_remap_entries", run.max_remap_entries as u64);
    out.u64("max_loaded_cells", run.max_loaded_cells as u64);
    out.u64("loads", run.log.accepted_loads() as u64);
    out.u64("unloads", run.log.accepted_unloads() as u64);
    out.u64("arrivals", run.log.accepted_arrivals() as u64);
    out.u64("rejected_commands", run.log.rejected_commands() as u64);
    out.u64("abandoned_arrivals", run.log.rejected_arrivals() as u64);
    out.u64("replay_mismatches", run.replay_mismatches);
    out.u64("dropped_pending", run.dropped_pending);
    out.u64("spawned_entities", run.instruments.spawned_entities);
    out.u64("despawned_entities", run.instruments.despawned_entities);
    out.decimal("wall_ms", run.wall_ms());
    out.decimal("ticks_per_second", run.ticks_per_second());
    out.decimal("mean_tick_us", run.mean_tick_us());
    out.decimal("peak_tick_us", run.peak_tick_us());
    out.decimal("spawn_us_per_cell", run.spawn_us_per_cell());
    out.decimal("despawn_us_per_cell", run.despawn_us_per_cell());
    out.decimal("spawn_median_us_per_cell", run.spawn_median_us_per_cell());
    out.decimal("despawn_median_us_per_cell", run.despawn_median_us_per_cell());
    out.decimal("us_per_spawned_entity", run.us_per_spawned_entity());
    out.decimal("changes_share_of_tick", run.changes_share());
    out.decimal("structural_node_share_of_tick", run.structural_node_share());
    out.decimal("peak_changes_us", run.peak_changes_us());
    out.decimal("peak_structural_us", run.peak_structural_us());
    out.key("phase_ms");
    out.object_start();
    for (name, ms) in run.phase_ms() {
        out.decimal(name, ms);
    }
    out.decimal("changes", run.instruments.changes_ns as f64 / 1e6);
    out.decimal("spawn", run.instruments.spawn_ns as f64 / 1e6);
    out.decimal("despawn", run.instruments.despawn_ns as f64 / 1e6);
    out.decimal("policy", run.instruments.policy_ns as f64 / 1e6);
    out.decimal("input", run.instruments.input_ns as f64 / 1e6);
    out.decimal("fetch", run.instruments.fetch_ns as f64 / 1e6);
    out.object_end();
    out.object_end();
}

fn write_scenario(out: &mut Json, scenario: &ScenarioReport) {
    out.object_start();
    out.string("scenario", &scenario.scenario.name());
    out.u64("grid", u64::from(scenario.scenario.grid));
    out.u64("density", u64::from(scenario.scenario.density));
    out.u64("ticks_per_cell", u64::from(scenario.scenario.ticks_per_cell));
    out.u64("ticks", u64::from(scenario.ticks));
    out.bool("scripted_identical_across_workers", scenario.scripted_identical());
    out.bool("scripted_match_oracle", scenario.scripted.iter().all(|r| r.matches_oracle));
    out.hex("scripted_final_hash", scenario.scripted[0].final_hash());
    out.hex("scripted_run_hash", scenario.scripted[0].run_hash());
    out.bool("live_matches_oracle", scenario.live.matches_oracle);
    out.hex("live_final_hash", scenario.live.final_hash());
    out.bool("replays_identical_to_live", scenario.replays_identical_to_live());
    out.bool("replays_match_oracle", scenario.replays.iter().all(|r| r.matches_oracle));
    out.u64("replay_mismatches", scenario.replays.iter().map(|r| r.replay_mismatches).sum());
    out.u64("loads", scenario.scripted[0].log.accepted_loads() as u64);
    out.u64("unloads", scenario.scripted[0].log.accepted_unloads() as u64);
    out.u64("live_loads", scenario.live.log.accepted_loads() as u64);
    out.u64("max_loaded_cells", scenario.scripted[0].max_loaded_cells as u64);
    out.u64("max_remap_entries", scenario.scripted[0].max_remap_entries as u64);
    out.bool("holds", scenario.holds());
    out.object_end();
}

fn write_cost_run(out: &mut Json, key: &str, run: &ModeRun) {
    out.key(key);
    out.object_start();
    out.string("scenario", &run.scenario.name());
    out.u64("workers", run.workers as u64);
    out.bool("matches_oracle", run.matches_oracle);
    out.decimal("changes_share_of_tick", run.changes_share());
    out.decimal("structural_node_share_of_tick", run.structural_node_share());
    out.decimal("peak_changes_us", run.peak_changes_us());
    out.decimal("peak_structural_us", run.peak_structural_us());
    out.decimal("peak_tick_us", run.peak_tick_us());
    out.decimal("mean_tick_us", run.mean_tick_us());
    out.decimal("us_per_spawned_entity", run.us_per_spawned_entity());
    out.decimal("spawn_us_per_cell", run.spawn_us_per_cell());
    out.decimal("despawn_us_per_cell", run.despawn_us_per_cell());
    out.u64("spawned_entities", run.instruments.spawned_entities);
    out.u64("loads", run.log.accepted_loads() as u64);
    out.decimal("ticks_per_second", run.ticks_per_second());
    out.object_end();
}

pub fn write(cell: &str, settings: &Settings, report: &Report) -> String {
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    out.string("profile", if cfg!(debug_assertions) { "dev" } else { "release" });
    write_platform_and_build(&mut out);

    out.key("settings");
    out.object_start();
    out.decimal("cell_side", f64::from(DEFAULT_CELL_SIDE));
    out.decimal("entering_distance_cells", f64::from(settings.entering));
    out.decimal("leaving_distance_cells", f64::from(settings.leaving));
    out.u64("spawn_slice", u64::from(settings.spawn_slice));
    out.u64("scripted_delay_ticks", u64::from(settings.scripted_delay_ticks));
    out.u64("live_delay_ms_low", settings.live_delay_ms.0);
    out.u64("live_delay_ms_high", settings.live_delay_ms.1);
    out.decimal("live_pause_ms", settings.live_pause.as_secs_f64() * 1_000.0);
    out.u64("live_batch_ticks", u64::from(LIVE_BATCH));
    out.u64("path_moves", settings.moves.len() as u64);
    out.u64("rounds", settings.rounds as u64);
    out.key("grids");
    out.array_start();
    for &grid in &settings.grids {
        out.object_start();
        out.u64("side", u64::from(grid));
        out.u64("cells", u64::from(grid * grid));
        out.object_end();
    }
    out.array_end();
    out.key("densities");
    out.array_start();
    for &density in &settings.densities {
        out.object_start();
        out.u64("entities_per_cell", u64::from(density));
        out.object_end();
    }
    out.array_end();
    out.key("speeds");
    out.array_start();
    for &speed in &settings.speeds {
        out.object_start();
        out.u64("ticks_per_cell", u64::from(speed));
        out.object_end();
    }
    out.array_end();
    out.key("worker_counts");
    out.array_start();
    for &workers in &settings.worker_counts {
        out.object_start();
        out.u64("workers", workers as u64);
        out.object_end();
    }
    out.array_end();
    out.string("tick_log", &settings.log_path.as_ref().map_or("none".to_string(), |p| p.display().to_string()));
    out.object_end();

    out.key("runs");
    out.array_start();
    for run in report.every_run() {
        write_run(&mut out, run);
    }
    out.array_end();

    out.key("determinism");
    out.object_start();
    out.bool("holds", report.determinism_holds());
    out.key("scenarios");
    out.array_start();
    for scenario in &report.scenarios {
        write_scenario(&mut out, scenario);
    }
    out.array_end();
    out.object_end();

    let reads = report.reads();
    out.key("not_present");
    out.object_start();
    out.bool("holds", report.not_present_holds());
    out.u64("reads_absent", reads.absent);
    out.u64("reads_present", reads.present);
    out.u64("stale_slots", reads.stale);
    out.u64("oracle_check_failures", report.oracle_check_failures());
    out.u64("runs", report.every_run().len() as u64);
    out.u64("runs_agreeing_with_oracle_counts", report.every_run().iter().filter(|r| r.reads == r.oracle.reads).count() as u64);
    out.object_end();

    let remap = &report.remap_cost;
    out.key("remap_cost");
    out.object_start();
    out.bool("measured", remap.measured());
    out.bool("holds", remap.holds());
    out.u64("density", u64::from(remap.density));
    out.u64("ticks_per_cell", u64::from(remap.ticks_per_cell));
    out.u64("workers", remap.workers as u64);
    out.decimal("difference_limit", REMAP_DIFFERENCE_LIMIT);
    out.decimal("spawn_difference", remap.spawn_difference());
    out.decimal("despawn_difference", remap.despawn_difference());
    out.decimal("table_difference", remap.table_difference());
    out.key("grids");
    out.array_start();
    for grid in &remap.grids {
        out.object_start();
        out.u64("grid", u64::from(grid.grid));
        out.u64("cells", u64::from(grid.grid * grid.grid));
        out.u64("rounds", grid.rounds.len() as u64);
        out.decimal("best_spawn_us_per_cell", grid.best_spawn_us_per_cell());
        out.decimal("best_despawn_us_per_cell", grid.best_despawn_us_per_cell());
        out.decimal("best_mean_spawn_us_per_cell", grid.best_mean_spawn_us_per_cell());
        out.decimal("best_mean_despawn_us_per_cell", grid.best_mean_despawn_us_per_cell());
        out.u64("max_remap_entries", grid.max_remap_entries() as u64);
        out.u64("max_loaded_cells", grid.max_loaded_cells() as u64);
        out.u64("loads", grid.rounds[0].log.accepted_loads() as u64);
        out.bool("every_round_matches_oracle", grid.rounds.iter().all(ModeRun::clean));
        out.key("rounds_spawn_us_per_cell");
        out.array_start();
        for round in &grid.rounds {
            out.object_start();
            out.decimal("spawn_median", round.spawn_median_us_per_cell());
            out.decimal("despawn_median", round.despawn_median_us_per_cell());
            out.decimal("spawn_mean", round.spawn_us_per_cell());
            out.decimal("despawn_mean", round.despawn_us_per_cell());
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
    out.object_end();

    let hysteresis = &report.hysteresis;
    out.key("hysteresis");
    out.object_start();
    out.bool("holds", hysteresis.holds());
    out.string("scenario", &hysteresis.with.scenario.name());
    out.decimal("saving_limit", HYSTERESIS_SAVING_LIMIT);
    out.decimal("saving", hysteresis.saving());
    out.u64("loads_with", hysteresis.with.log.accepted_loads() as u64);
    out.u64("loads_without", hysteresis.without.log.accepted_loads() as u64);
    out.u64("unloads_with", hysteresis.with.log.accepted_unloads() as u64);
    out.u64("unloads_without", hysteresis.without.log.accepted_unloads() as u64);
    out.u64("max_loaded_cells_with", hysteresis.with.max_loaded_cells as u64);
    out.u64("max_loaded_cells_without", hysteresis.without.max_loaded_cells as u64);
    out.bool("with_matches_oracle", hysteresis.with.matches_oracle);
    out.bool("without_matches_oracle", hysteresis.without.matches_oracle);
    out.hex("without_final_hash", hysteresis.without.final_hash());
    out.object_end();

    out.key("cost");
    out.object_start();
    out.bool("holds", report.cost.holds());
    out.decimal("share_limit", STRUCTURAL_SHARE_LIMIT);
    write_cost_run(&mut out, "sliced", &report.cost.sliced);
    write_cost_run(&mut out, "unsliced", &report.cost.unsliced);
    out.object_end();

    let two = &report.two_worlds;
    out.key("two_worlds");
    out.object_start();
    out.bool("holds", two.holds());
    out.string("first_scenario", &two.first.scenario.name());
    out.string("second_scenario", &two.second.scenario.name());
    out.u64("workers", two.first.workers as u64);
    out.bool("first_matches_oracle", two.first.matches_oracle);
    out.bool("second_matches_oracle", two.second.matches_oracle);
    out.hex("first_final_hash", two.first.final_hash());
    out.hex("second_final_hash", two.second.final_hash());
    out.u64("first_loads", two.first.log.accepted_loads() as u64);
    out.u64("second_loads", two.second.log.accepted_loads() as u64);
    out.u64("payloads_in_store", two.payloads as u64);
    out.u64("imports", two.imports);
    out.u64("requests_served", two.requests_served);
    out.object_end();

    out.object_end();
    out.finish()
}
