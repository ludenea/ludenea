//! The report: one JSON document on standard output (the facts the research
//! CI publishes and `summarize.sh` reads) and the full log written to a file
//! beside it. Throwaway, never product code.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use reference_workload::{Json, first_divergent_tick, run_hash, write_platform_and_build};

use crate::generate::{Sizes, asset_name};
use crate::oracle::OracleRun;
use crate::scene::{COMPONENT_NAMES, KIND_NAMES};
use crate::state::{Event, InstancePath, Record};
use crate::world::{Application, NODE_NAMES};
use crate::{CHECKPOINTS, Identities, PLAN, Report, RunReport, SPIKE, ScenarioReport, Settings, TICKS};

const SCHEMA_VERSION: u64 = 1;

fn path_text(path: &InstancePath) -> String {
    let locals: Vec<String> = path.locals.iter().map(|local| format!("{local:016x}")).collect();
    format!("{}/{}", path.root, locals.join("/"))
}

/// The full log: one section per run (its settings, its phase times, then
/// per tick the two hashes, the instantiation's nanoseconds, the tick's wall
/// nanoseconds, the entities spawned and despawned and the loaded
/// instances), then per size the input log, the instantiation's events and
/// what the import answered to every planted conflict.
pub struct TickLog {
    writer: Option<BufWriter<File>>,
    pub lines: u64,
}

impl TickLog {
    pub fn new(path: Option<PathBuf>) -> TickLog {
        let writer = path.as_ref().and_then(|path| match File::create(path) {
            Ok(file) => Some(BufWriter::new(file)),
            Err(error) => {
                eprintln!("the log at {} cannot be created ({error}); the report carries the summary alone", path.display());
                None
            }
        });
        TickLog { writer, lines: 0 }
    }

    fn write(&mut self, lines: Vec<String>) {
        let Some(writer) = self.writer.as_mut() else { return };
        for line in &lines {
            if let Err(error) = writeln!(writer, "{line}") {
                eprintln!("writing the log failed: {error}");
                return;
            }
        }
        self.lines += lines.len() as u64;
    }

    pub fn record(&mut self, sizes: &Sizes, run: &RunReport, oracle: &OracleRun) {
        let i = &run.instruments;
        let phases: Vec<String> = NODE_NAMES.iter().zip(i.phase_ns).map(|(name, ns)| format!("{name}={:.3}", ns as f64 / 1e6)).collect();
        let mut lines = vec![
            format!(
                "# run {} {} workers={} ticks={} wall_ms={:.3} state_run_hash={:#018x} content_run_hash={:#018x} matches_oracle={} first_divergent_tick={:?} reads_absent={} reads_present={} reads_stale={} gate_polls={} slots={} entities={}",
                sizes.name,
                run.application.name(),
                run.workers,
                TICKS,
                run.wall.as_secs_f64() * 1e3,
                run_hash(&run.hashes),
                run_hash(&run.content),
                run.matches_oracle,
                first_divergent_tick(&run.hashes, &oracle.hashes),
                run.reads.absent,
                run.reads.present,
                run.reads.stale,
                run.gate_polls,
                run.slots,
                run.entities
            ),
            format!("# phase_ms {} instantiation={:.3} spawns={:.3} spawned={} despawned={}", phases.join(" "), i.changes_ns as f64 / 1e6, i.spawn_ns as f64 / 1e6, i.spawned_entities, i.despawned_entities),
            "# tick state_hash content_hash instantiation_ns tick_wall_ns spawned despawned loaded_instances".to_string(),
        ];
        for t in 0..i.changes_per_tick_ns.len() {
            lines.push(format!(
                "{} {:#018x} {:#018x} {} {} {} {} {}",
                t + 1,
                run.hashes[t + 1],
                run.content[t + 1],
                i.changes_per_tick_ns[t],
                i.tick_wall_ns[t],
                i.spawned_per_tick[t],
                i.despawned_per_tick[t],
                i.loaded_per_tick[t]
            ));
        }
        self.write(lines);
    }

    pub fn events(&mut self, scenario: &ScenarioReport) {
        let run = scenario.first(Application::Prepared);
        let mut lines = vec![format!("# input log {}", scenario.sizes.name)];
        for logged in &run.log {
            let record = match logged.record {
                Record::Load { root, asset, required } => format!("load root={root} scene={} required={required}", asset_name(asset)),
                Record::Unload { root } => format!("unload root={root}"),
                Record::Arrival { asset, content_hash } => format!("arrival scene={} content_hash={content_hash:#018x}", asset_name(asset)),
            };
            lines.push(format!("{} {record} accepted={}", logged.tick, logged.accepted));
        }
        for application in Application::ALL {
            lines.push(format!("# events {} {}", scenario.sizes.name, application.name()));
            for event in &scenario.first(application).events {
                lines.push(match event {
                    Event::Spawned { tick, path, members, digest } => format!("{tick} spawned {} members={members} digest={digest:#018x}", path_text(path)),
                    Event::Despawned { tick, path, members } => format!("{tick} despawned {} members={members}", path_text(path)),
                    Event::Failed { tick, path, reason } => format!("{tick} failed {} reason={reason:?}", path_text(path)),
                });
            }
        }
        lines.push(format!("# planted conflicts {}", scenario.sizes.name));
        for case in &scenario.import.planted {
            let answer = case.answer.map_or("ACCEPTED".to_string(), |refusal| refusal.to_string());
            lines.push(format!("{} | {} | expected: {} | answered: {answer}", case.place, case.what, case.expected.words()));
        }
        self.write(lines);
    }

    pub fn finish(&mut self) {
        if let Some(mut writer) = self.writer.take()
            && let Err(error) = writer.flush()
        {
            eprintln!("flushing the log failed: {error}");
        }
    }
}

fn write_option(out: &mut Json, key: &str, value: Option<usize>) {
    match value {
        Some(value) => out.u64(key, value as u64),
        None => out.string(key, "none"),
    }
}

fn write_run(out: &mut Json, run: &RunReport, oracle: &OracleRun) {
    let i = &run.instruments;
    let stepping = i.stepping.as_secs_f64();
    out.object_start();
    out.string("application", run.application.name());
    out.u64("workers", run.workers as u64);
    out.u64("ticks", u64::from(TICKS));
    out.hex("state_run_hash", run_hash(&run.hashes));
    out.hex("content_run_hash", run_hash(&run.content));
    out.hex("final_state_hash", *run.hashes.last().expect("a history"));
    out.hex("final_content_hash", *run.content.last().expect("a history"));
    out.bool("matches_oracle", run.matches_oracle);
    write_option(out, "first_divergent_tick", first_divergent_tick(&run.hashes, &oracle.hashes).or(first_divergent_tick(&run.content, &oracle.content)));
    out.u64("reads_absent", run.reads.absent);
    out.u64("reads_present", run.reads.present);
    out.u64("stale_slots", run.reads.stale);
    out.bool("reads_equal_oracle", run.reads == oracle.reads);
    out.u64("commands_accepted", run.log.iter().filter(|logged| logged.accepted && !matches!(logged.record, Record::Arrival { .. })).count() as u64);
    out.u64("commands_refused", run.log.iter().filter(|logged| !logged.accepted).count() as u64);
    out.u64("arrivals", run.log.iter().filter(|logged| matches!(logged.record, Record::Arrival { .. })).count() as u64);
    out.u64("gate_polls", run.gate_polls);
    out.u64("ticks_with_a_required_instance_missing", run.required_missing);
    out.u64("spawns_not_prepared", run.prepared_misses);
    out.u64("instances_spawned", run.events.iter().filter(|event| matches!(event, Event::Spawned { .. })).count() as u64);
    out.u64("instances_failed", run.events.iter().filter(|event| matches!(event, Event::Failed { .. })).count() as u64);
    out.u64("entities_spawned", i.spawned_entities);
    out.u64("entities_despawned", i.despawned_entities);
    out.u64("entities_at_the_end", run.entities as u64);
    out.u64("slots", run.slots as u64);
    out.decimal("wall_ms", run.wall.as_secs_f64() * 1e3);
    out.decimal("ticks_per_second", f64::from(TICKS) / stepping);
    out.decimal("spawn_us_per_entity", i.spawn_ns as f64 / 1e3 / i.spawned_entities.max(1) as f64);
    out.decimal("instantiation_share_of_stepping", i.changes_ns as f64 / 1e9 / stepping);
    out.decimal("peak_instantiation_us", i.changes_per_tick_ns.iter().copied().max().unwrap_or(0) as f64 / 1e3);
    out.object_end();
}

fn write_identities(out: &mut Json, application: Application, identities: &Identities) {
    out.object_start();
    out.string("application", application.name());
    out.u64("instances", identities.instances);
    out.u64("entities", identities.entities);
    out.u64("overridden", identities.overridden);
    out.u64("kept", identities.kept);
    out.u64("changed", identities.changed);
    out.u64("instances_failed_as_expected", identities.failed_as_expected);
    out.u64("instances_spawned_at_another_hash", identities.spawned_at_another_hash);
    out.object_end();
}

fn write_scenario(out: &mut Json, scenario: &ScenarioReport) {
    let sizes = &scenario.sizes;
    out.object_start();
    out.string("size", sizes.name);
    out.key("sizes");
    out.object_start();
    out.u64("room_entities", sizes.room as u64);
    out.u64("building_entities", sizes.building as u64);
    out.u64("level_entities", sizes.level as u64);
    out.u64("instances_per_building", sizes.rooms as u64);
    out.u64("buildings_per_level", sizes.buildings as u64);
    out.u64("overrides_per_kind_and_group", sizes.per_kind as u64);
    out.object_end();

    let import = &scenario.import;
    out.key("import");
    out.object_start();
    out.hex("payloads_digest", import.digest);
    out.key("payloads");
    out.array_start();
    for payload in &import.payloads {
        out.object_start();
        out.string("scene", &asset_name(payload.asset));
        out.hex("content_hash", payload.content_hash);
        out.u64("bytes", payload.bytes.len() as u64);
        out.u64("entities", payload.protos.len() as u64);
        out.u64("nested_instances", payload.nested.len() as u64);
        out.u64("overrides", payload.override_count() as u64);
        out.object_end();
    }
    out.array_end();
    out.u64("lists", import.lists);
    out.key("overrides_by_kind");
    out.object_start();
    for (kind, count) in import.items_by_kind.iter().enumerate() {
        out.u64(&KIND_NAMES[kind].replace(' ', "_"), *count);
    }
    out.object_end();
    out.u64("overrides_on_the_child", import.items_by_path_length[0]);
    out.u64("overrides_through_a_path", import.items_by_path_length[1]);
    out.u64("outer_overrides_on_what_an_inner_list_set", import.outer_on_inner);
    out.key("planted");
    out.array_start();
    for case in &import.planted {
        out.object_start();
        out.string("conflict", case.what);
        out.string("place", case.place);
        out.bool("named_by_the_measurement", case.named);
        out.string("expected", case.expected.words());
        out.string("answer", case.answer.map_or("accepted", |refusal| refusal.name.words()));
        out.i64("position", case.answer.map_or(-1, |refusal| refusal.position as i64));
        out.bool("refused_by_name", case.refused_by_name());
        out.object_end();
    }
    out.array_end();
    out.u64("named_conflicts", import.named().count() as u64);
    out.u64("named_conflicts_refused_by_name", import.named().filter(|case| case.refused_by_name()).count() as u64);
    out.u64("planted_conflicts", import.planted.len() as u64);
    out.u64("planted_conflicts_refused_by_name", import.planted.iter().filter(|case| case.refused_by_name()).count() as u64);
    out.u64("planted_conflicts_accepted", import.planted.iter().filter(|case| case.answer.is_none()).count() as u64);
    out.u64("orders_of_application_tried", import.orders_tried);
    out.u64("orders_of_application_differing", import.orders_differing);
    out.bool("holds", scenario.import_holds());
    out.object_end();

    out.key("runs");
    out.array_start();
    for run in &scenario.runs {
        write_run(out, run, if run.application == Application::InPlace { &scenario.oracle_in_world } else { &scenario.oracle_payload });
    }
    out.array_end();

    out.key("applications");
    out.array_start();
    for application in Application::ALL {
        let first = scenario.first(application);
        out.object_start();
        out.string("application", application.name());
        out.bool("identical_across_workers", scenario.identical_across_workers(application));
        out.bool("matches_oracle", scenario.matches_oracle(application));
        out.hex("state_run_hash", run_hash(&first.hashes));
        out.hex("content_run_hash", run_hash(&first.content));
        out.u64("slots", first.slots as u64);
        out.decimal("best_spawn_us_per_entity", scenario.of(application).map(|run| run.instruments.spawn_ns as f64 / 1e3 / run.instruments.spawned_entities.max(1) as f64).fold(f64::INFINITY, f64::min));
        out.object_end();
    }
    out.array_end();

    out.key("oracles");
    out.object_start();
    for (key, oracle) in [("payload_allocation", &scenario.oracle_payload), ("in_world_allocation", &scenario.oracle_in_world)] {
        out.key(key);
        out.object_start();
        out.hex("state_run_hash", run_hash(&oracle.hashes));
        out.hex("content_run_hash", run_hash(&oracle.content));
        out.u64("reads_absent", oracle.reads.absent);
        out.u64("reads_present", oracle.reads.present);
        out.u64("check_failures", oracle.check_failures);
        out.u64("record_mismatches", oracle.record_mismatches);
        out.u64("ticks_with_a_required_instance_missing", oracle.required_missing);
        out.u64("entities_spawned", oracle.spawned_entities);
        out.u64("entities_despawned", oracle.despawned_entities);
        out.u64("slots", oracle.slots as u64);
        out.object_end();
    }
    out.object_end();

    out.key("determinism");
    out.object_start();
    out.bool("holds", scenario.determinism_holds());
    out.u64("world_runs", scenario.runs.len() as u64);
    out.u64("runs_matching_their_oracle", scenario.runs.iter().filter(|run| run.matches_oracle).count() as u64);
    out.u64("stale_slots", scenario.runs.iter().map(|run| run.reads.stale).sum());
    out.object_end();

    out.key("application_paths");
    out.object_start();
    out.bool("payload_equals_ordered_in_staging", scenario.payload_equals_staging());
    out.bool("ordered_in_world_spawns_the_same_content", scenario.in_world_spawns_equal());
    out.bool("ordered_in_world_same_state_hash", scenario.in_world_first_state_divergence().is_none());
    write_option(out, "ordered_in_world_first_tick_of_another_state_hash", scenario.in_world_first_state_divergence());
    out.bool("ordered_in_world_same_content_hash", scenario.in_world_first_content_divergence().is_none());
    write_option(out, "ordered_in_world_first_tick_of_another_content_hash", scenario.in_world_first_content_divergence());
    out.u64("slots_payload", scenario.first(Application::Prepared).slots as u64);
    out.u64("slots_ordered_in_world", scenario.first(Application::InPlace).slots as u64);
    out.u64("prepared_payloads", scenario.prepared_payloads as u64);
    out.u64("prepared_instances", scenario.prepared_instances);
    out.decimal("prepare_ms", scenario.prepare.as_secs_f64() * 1e3);
    out.object_end();

    out.key("families");
    out.array_start();
    for family in &scenario.families {
        out.object_start();
        out.string("family", family.plan.name);
        out.u64("root", family.plan.root);
        out.bool("required", family.plan.required);
        out.u64("command_tick", u64::from(family.plan.tick));
        out.u64("depth", family.depth);
        out.u64("instances", family.instances);
        out.u64("entities", family.entities);
        out.u64("children_failed", family.failed_children);
        out.u64("first_tick", u64::from(family.first_tick));
        out.u64("last_tick", u64::from(family.last_tick));
        out.u64("ticks_from_the_command", u64::from(family.first_tick.saturating_sub(family.plan.tick)));
        out.u64("distinct_ticks", family.distinct_ticks);
        out.bool("parents_first", family.parents_first);
        out.bool("holds", family.holds);
        out.object_end();
    }
    out.array_end();
    out.bool("families_hold", scenario.families_hold());

    out.key("another_hash");
    out.object_start();
    out.bool("holds", scenario.another_hash_fails());
    out.u64("children_expected_to_fail", scenario.expected_failed_children);
    out.u64("children_failed", scenario.failed_children());
    out.u64("children_spawned_at_another_hash", scenario.identities.iter().map(|(_, identities)| identities.spawned_at_another_hash).sum());
    out.object_end();

    out.key("identities");
    out.object_start();
    out.bool("holds", scenario.identities_hold());
    out.key("by_application");
    out.array_start();
    for (application, identities) in &scenario.identities {
        write_identities(out, *application, identities);
    }
    out.array_end();
    let control = &scenario.control;
    out.key("against_the_world_without_overrides");
    out.object_start();
    out.u64("entities_without_overrides", control.plain_entities);
    out.u64("entities_with_overrides", control.overridden_entities);
    out.u64("full_references_kept", control.kept);
    out.u64("kept_and_overridden", control.kept_overridden);
    out.u64("removed_by_an_override", control.removed_by_override);
    out.u64("inside_a_removed_instance", control.inside_removed_instances);
    out.u64("added_by_an_override", control.added_by_override);
    out.u64("unexplained", control.unexplained);
    out.object_end();
    out.object_end();
    out.object_end();
}

/// The whole report as one JSON document.
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
    out.hex("seed", crate::SEED);
    out.u64("ticks", u64::from(TICKS));
    out.string("sizes", &settings.sizes.iter().map(|sizes| sizes.name).collect::<Vec<_>>().join(", "));
    out.string("worker_counts", &settings.worker_counts.iter().map(|workers| workers.to_string()).collect::<Vec<_>>().join(", "));
    out.string("checkpoints", &CHECKPOINTS.iter().map(|tick| tick.to_string()).collect::<Vec<_>>().join(", "));
    out.u64("scripted_loads", PLAN.len() as u64);
    out.string("components", &COMPONENT_NAMES.join(", "));
    out.object_end();
    out.key("scenarios");
    out.array_start();
    for scenario in &report.scenarios {
        write_scenario(&mut out, scenario);
    }
    out.array_end();
    let all = |check: &dyn Fn(&ScenarioReport) -> bool| report.scenarios.iter().all(check);
    out.key("verdicts");
    out.object_start();
    out.bool("one_history_at_every_worker_count_equal_to_the_oracle", all(&|scenario| scenario.determinism_holds()));
    out.bool("planted_conflicts_refused_by_name", all(&|scenario| scenario.import.conflicts_refused()));
    out.bool("a_validated_list_gives_one_result_in_every_order", all(&|scenario| scenario.import.orders_differing == 0));
    out.bool("payload_equals_ordered_in_staging", all(&|scenario| scenario.payload_equals_staging()));
    out.bool("ordered_in_world_spawns_the_same_content", all(&|scenario| scenario.in_world_spawns_equal()));
    out.bool("ordered_in_world_same_state_hash", all(&|scenario| scenario.in_world_first_state_divergence().is_none()));
    out.bool("families_appear_as_stated", all(&|scenario| scenario.families_hold()));
    out.bool("a_child_at_another_hash_fails", all(&|scenario| scenario.another_hash_fails()));
    out.bool("every_entity_keeps_its_full_reference", all(&|scenario| scenario.identities_hold()));
    out.object_end();
    out.u64("log_lines", report.log_lines);
    out.string("log_file", &report.log_path.as_ref().map(|path| path.display().to_string().replace('\\', "/")).unwrap_or_default());
    out.object_end();
    out.finish()
}
