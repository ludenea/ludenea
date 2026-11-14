//! The harness of the prototype of the asset table and of the loader.
//! Throwaway, never product code. See the README beside the workspace.
//!
//! A run prints one JSON document on standard output (the facts the research
//! CI keeps), a short summary on standard error, and writes its full log,
//! every difference with an oracle in words, to a file.
//!
//! Usage: `asset-table-and-loader <cell> [family ...]`. The readers of a save
//! run as processes of this same binary, with `--reader` or `--spike-reader`
//! first. Four more modes read no oracle file: `--first-check` runs the asset
//! table spike's scenarios alone, `--enumeration` the enumeration of small
//! records, `--gate-cost [repetitions]` the cost of the gate on a large
//! table, and `--drive [family ...]` drives every scenario and every control
//! of the families and compares nothing. `--readings [family ...]` runs
//! every reading the build did not take on every scenario, against the
//! oracles, and prints which scenarios tell each apart.

mod enumeration;
mod families;
mod families_same_harness;
mod gate_cost;
mod oracle;
mod play;
mod run;
mod sequential;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use prototype_core::world::Size;
use reference_workload::{Json, write_platform_and_build};

use play::FamilyPlan;
use run::{Claim, Settings};

const SCHEMA_VERSION: u64 = 2;
const PROTOTYPE: &str = "asset-table-and-loader";
/// The repetitions of the measurement of the gate's cost.
const GATE_COST_REPETITIONS: usize = 5;
/// The bound of the claim on the gate's cost: a tenth of a tick.
const GATE_COST_BOUND: f64 = 0.1;

#[global_allocator]
static ALLOCATOR: spike_check::Counting = spike_check::Counting;

/// A family judged on states: its name and what makes its plan.
type Family = (&'static str, fn(&Settings) -> FamilyPlan);

/// The families judged on states: those of the first sheet, then those of
/// the same-harness sheet.
const FAMILIES: [Family; 14] = [
    ("order", families::order),
    ("owners", families::owners),
    ("named-set", families::named_set),
    ("restore", families::restore),
    ("retry", families::retry),
    ("types", families::types),
    ("dependencies", families::dependencies),
    ("revision", families::revision),
    ("pending-parent", families::pending_parent),
    ("record", families::record),
    ("system-wants", families_same_harness::system_wants),
    ("shared-loader", families_same_harness::shared_loader),
    ("mount", families_same_harness::mount),
    ("budget", families_same_harness::budget),
];

fn family(settings: &Settings, name: &str) -> Option<FamilyPlan> {
    FAMILIES.iter().find(|(other, _)| *other == name).map(|(_, plan)| plan(settings))
}

fn settings(scratch: &str) -> Settings {
    let size = if cfg!(debug_assertions) { Size::Small } else { Size::Default };
    let scratch = std::env::temp_dir().join(format!("{PROTOTYPE}-{scratch}-{}", std::process::id()));
    Settings { size, workers: vec![1, 4, 8], scratch }
}

/// The claims judged on states, each against its oracles; or, with no
/// oracle, every scenario driven and nothing compared. `only` names the
/// families to run, every one where it is empty. The readings the build did
/// not take are run on the scenarios that differ from their oracle, or on
/// every scenario where `every_reading` is set.
fn claims(settings: &Settings, only: &[String], every_reading: bool, processes: bool, against_the_oracles: bool) -> Vec<Claim> {
    let readings = families::readings();
    FAMILIES
        .iter()
        .filter(|(name, _)| only.is_empty() || only.iter().any(|other| other == name))
        .map(|(_, plan)| play::run_family(settings, &plan(settings), &readings, every_reading, processes, against_the_oracles))
        .collect()
}

/// A scenario's standing against its oracle, over the worker counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    /// Equal at every worker count.
    Equal,
    /// It differs, and equals the oracle under another reading of the text.
    DiffersByAReading,
    Differs,
}

impl Standing {
    fn text(self) -> &'static str {
        match self {
            Standing::Equal => "equals its oracle",
            Standing::DiffersByAReading => "differs, and equals its oracle under another reading",
            Standing::Differs => "differs from its oracle",
        }
    }
}

fn standings(claim: &Claim) -> BTreeMap<(String, String), Standing> {
    let mut standings = BTreeMap::new();
    for outcome in &claim.outcomes {
        let equal = outcome.differences.is_empty() && outcome.replays != Some(false);
        let by_a_reading = claim.variants.iter().any(|variant| variant.scenario == outcome.scenario && !variant.control && !variant.differs);
        let standing = if equal {
            Standing::Equal
        } else if by_a_reading {
            Standing::DiffersByAReading
        } else {
            Standing::Differs
        };
        let held = standings.entry((outcome.scenario.clone(), outcome.world.clone())).or_insert(standing);
        if standing != Standing::Equal && *held != Standing::Differs {
            *held = standing;
        }
    }
    standings
}

fn verdict(claim: &Claim) -> &'static str {
    let standings = standings(claim);
    if standings.values().any(|standing| *standing == Standing::Differs) {
        "differs from an oracle"
    } else if !claim.controls_differ() {
        "a control equals its oracle"
    } else if standings.values().all(|standing| *standing == Standing::Equal) {
        "equals its oracles"
    } else {
        "equals its oracles but where the text is read two ways"
    }
}

fn write_claim(out: &mut Json, log: &mut String, claim: &Claim) {
    let standings = standings(claim);
    out.object_start();
    out.string("claim", &claim.name);
    out.string("verdict", verdict(claim));
    out.bool("one_run_hash_per_scenario_across_worker_counts", claim.one_hash_per_scenario());
    out.bool("every_control_differs", claim.controls_differ());
    out.key("scenarios");
    out.array_start();
    let _ = writeln!(log, "\n== {}: {}", claim.name, verdict(claim));
    // In the order the scenarios ran.
    let mut order: Vec<(String, String)> = Vec::new();
    for outcome in &claim.outcomes {
        let key = (outcome.scenario.clone(), outcome.world.clone());
        if !order.contains(&key) {
            order.push(key);
        }
    }
    for key in &order {
        let (scenario, world) = key;
        let standing = standings[key];
        let outcomes: Vec<_> = claim.outcomes.iter().filter(|outcome| outcome.scenario == *scenario && outcome.world == *world).collect();
        out.object_start();
        out.string("scenario", scenario);
        out.string("world", world);
        out.string("standing", standing.text());
        out.hex("run_hash", outcomes.first().map_or(0, |outcome| outcome.run_hash));
        out.u64("differences", outcomes.iter().map(|outcome| outcome.differences.len() as u64).max().unwrap_or(0));
        let replays = outcomes.iter().filter_map(|outcome| outcome.replays).collect::<Vec<_>>();
        out.string("replay", if replays.is_empty() { "none" } else if replays.iter().all(|replays| *replays) { "replays" } else { "does not replay" });
        out.object_end();
        let _ = writeln!(log, "  {scenario} [{world}]: {}", standing.text());
        for outcome in outcomes.iter().filter(|outcome| !outcome.differences.is_empty() || outcome.replays == Some(false)) {
            let _ = writeln!(log, "    at {} workers (replay {:?}):", outcome.workers, outcome.replays);
            for difference in &outcome.differences {
                let _ = writeln!(log, "      {difference}");
            }
        }
        if standing != Standing::Equal {
            for line in outcomes.iter().flat_map(|outcome| &outcome.observed) {
                let _ = writeln!(log, "    the run gave | {line}");
            }
        }
    }
    out.array_end();
    out.key("controls_and_readings");
    out.array_start();
    for variant in &claim.variants {
        out.object_start();
        out.string("scenario", &variant.scenario);
        out.string("kind", if variant.control { "control" } else { "reading" });
        out.string("change", &variant.name);
        out.bool("differs_from_the_oracle", variant.differs);
        out.string("first_difference", &variant.first_difference);
        out.object_end();
        // Of the readings run on a scenario that differs, the log keeps
        // those under which it equals its oracle.
        if variant.control || !variant.differs {
            let kind = if variant.control { "control" } else { "reading" };
            let _ = writeln!(log, "  {} [{kind}] {}: differs {} | {}", variant.scenario, variant.name, variant.differs, variant.first_difference);
        }
    }
    out.array_end();
    out.key("notes");
    out.array_start();
    for note in &claim.notes {
        out.object_start();
        out.string("note", note);
        out.object_end();
        let _ = writeln!(log, "  note: {note}");
    }
    out.array_end();
    out.object_end();
}

/// The placement of the crates: the table has no edge to the loading
/// capability, and the loading capability none to the pipeline.
fn placement() -> Vec<(&'static str, Vec<&'static str>, bool)> {
    let names = ["prototype-core", "content-store", "asset-loader", "asset-pipeline"];
    let manifests = [
        ("prototype-core", include_str!("../../core/Cargo.toml"), &["content-store", "asset-loader", "asset-pipeline"][..]),
        ("content-store", include_str!("../../store/Cargo.toml"), &["prototype-core", "asset-loader", "asset-pipeline"][..]),
        ("asset-loader", include_str!("../../loader/Cargo.toml"), &["asset-pipeline"][..]),
        ("asset-pipeline", include_str!("../../pipeline/Cargo.toml"), &["asset-loader"][..]),
    ];
    manifests
        .into_iter()
        .map(|(name, manifest, forbidden)| {
            let dependencies = manifest.split("[dependencies]").nth(1).unwrap_or("");
            let edges: Vec<&str> = names.into_iter().filter(|other| dependencies.lines().any(|line| line.starts_with(other))).collect();
            let respected = forbidden.iter().all(|other| !edges.contains(other));
            (name, edges, respected)
        })
        .collect()
}

fn report(settings: &Settings, cell: &str, only: &[String]) -> (String, String, String, bool) {
    let mut out = Json::new();
    let mut log = String::new();
    let mut summary = String::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("prototype", PROTOTYPE);
    out.string("stage", "the same-harness stage");
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.string("size", settings.size.name());
    out.string("worker_counts", "1, 4, 8");
    let whole = only.is_empty();
    let mut holds = !whole || first_check(&mut out, &mut summary, &mut log);

    let placement = placement();
    out.key("placement");
    out.array_start();
    for (name, edges, respected) in &placement {
        out.object_start();
        out.string("crate", name);
        out.string("depends_on", &edges.join(", "));
        out.bool("placement_respected", *respected);
        out.object_end();
        holds &= respected;
    }
    out.array_end();
    let _ = writeln!(summary, "placement of the crates respected: {}", placement.iter().all(|(_, _, respected)| *respected));

    out.key("claims");
    out.array_start();
    for claim in claims(settings, only, false, true, true) {
        write_claim(&mut out, &mut log, &claim);
        let standings = standings(&claim);
        let count = |standing: Standing| standings.values().filter(|other| **other == standing).count();
        let controls = claim.variants.iter().filter(|variant| variant.control).count();
        let differing = claim.variants.iter().filter(|variant| variant.control && variant.differs).count();
        let _ = writeln!(
            summary,
            "{}: {} | {} worlds equal, {} differ by a reading, {} differ | {differing} of {controls} controls differ | one hash across worker counts: {}",
            claim.name,
            verdict(&claim),
            count(Standing::Equal),
            count(Standing::DiffersByAReading),
            count(Standing::Differs),
            claim.one_hash_per_scenario()
        );
        holds &= count(Standing::Differs) == 0 && claim.controls_differ() && claim.one_hash_per_scenario();
    }
    out.array_end();

    if whole {
        // A magnitude, judged in the release profile alone.
        if cfg!(debug_assertions) {
            let _ = writeln!(summary, "the gate's cost: not measured in the unoptimized profile");
        } else {
            holds &= gate_cost(&mut out, &mut summary, &mut log, GATE_COST_REPETITIONS, &settings.workers);
        }
        holds &= enumerate(&mut out, &mut summary, &mut log);
    }

    out.bool("every_scenario_stands", holds);
    out.object_end();
    (out.finish(), summary, log, holds)
}

/// The first check: the asset table spike's scenarios, run again on the
/// prototype's table and world. Their oracles are the spike's own, computed
/// by its sequential oracle of the workload: the check reads no oracle file.
/// Returns whether every scenario holds.
fn first_check(out: &mut Json, summary: &mut String, log: &mut String) -> bool {
    let spike_settings = if cfg!(debug_assertions) { spike_check::Settings::unoptimized() } else { spike_check::Settings::measured() };
    out.key("first_check");
    out.object_start();
    let scenarios = spike_check::run(out, &spike_settings);
    out.object_end();
    let _ = writeln!(log, "== the first check: the asset table spike's scenarios, run again");
    let passed = scenarios.iter().filter(|(_, verdict)| *verdict).count();
    for (name, verdict) in &scenarios {
        let _ = writeln!(log, "  {name}: {}", if *verdict { "holds" } else { "does not hold" });
    }
    let _ = writeln!(summary, "first check (the asset table spike's scenarios run again): {passed} of {} hold", scenarios.len());
    passed == scenarios.len()
}

fn write_cells(out: &mut Json, log: &mut String, key: &str, repetitions: &[Vec<gate_cost::Cell>]) {
    out.key(key);
    out.array_start();
    for (repetition, cells) in repetitions.iter().enumerate() {
        for cell in cells {
            out.object_start();
            out.u64("repetition", repetition as u64 + 1);
            out.string("case", cell.case);
            out.string("pending_record", cell.size);
            out.u64("workers", cell.workers as u64);
            out.u64("median_evaluation_ns", cell.evaluation.as_nanos() as u64);
            out.u64("median_copy_ns", cell.copy.as_nanos() as u64);
            out.u64("median_tick_ns", cell.tick.as_nanos() as u64);
            out.decimal("ratio", cell.ratio());
            out.bool("verdict_as_stated", cell.verdict_as_stated);
            out.object_end();
            let _ = writeln!(
                log,
                "  repetition {} {:<6} {:<5} {} workers: evaluation {:>9} ns of which the copy {:>8} ns, tick {:>9} ns, ratio {:.4}, verdict as stated {}",
                repetition + 1,
                cell.case,
                cell.size,
                cell.workers,
                cell.evaluation.as_nanos(),
                cell.copy.as_nanos(),
                cell.tick.as_nanos(),
                cell.ratio(),
                cell.verdict_as_stated
            );
        }
    }
    out.array_end();
}

/// The cost of the gate evaluated on a large table. It reads no oracle: it
/// is a magnitude. Returns whether the largest ratio stands under the bound
/// and every verdict is the one its case states.
fn gate_cost(out: &mut Json, summary: &mut String, log: &mut String, repetitions: usize, workers: &[usize]) -> bool {
    const HOW: &str = "a copy of the whole table, on which the pending record is applied, where the world is gated; \
                       where it is not gated the build answers from the state alone and reads no pending record";
    let cost = gate_cost::run(repetitions, workers);
    let verdicts = cost.repetitions.iter().chain(&cost.open_with_the_record_read).flatten().all(|cell| cell.verdict_as_stated);
    let largest = cost.largest();
    let holds = largest < GATE_COST_BOUND && verdicts && cost.table_as_stated && cost.record_as_assembled;
    out.key("gate_cost");
    out.object_start();
    out.string("how_the_table_the_gate_is_evaluated_on_is_obtained", HOW);
    out.u64("repetitions", repetitions as u64);
    out.bool("table_as_stated", cost.table_as_stated);
    out.bool("record_built_as_the_world_assembles_it", cost.record_as_assembled);
    out.bool("every_verdict_as_stated", verdicts);
    out.decimal("largest_ratio", largest);
    out.decimal("bound", GATE_COST_BOUND);
    out.bool("under_the_bound", largest < GATE_COST_BOUND);
    out.decimal("largest_ratio_of_the_case_open_with_the_pending_record_read", cost.largest_with_the_record_read());
    out.decimal("largest_ratio_of_the_copy_alone", cost.largest_copy());
    let _ = writeln!(log, "\n== the gate's cost on a table of ten thousand assets: {HOW}");
    write_cells(out, log, "figures", &cost.repetitions);
    let _ = writeln!(log, "  the case open under the other reading, the pending record of a world that is not gated being read:");
    write_cells(out, log, "open_with_the_pending_record_read", &cost.open_with_the_record_read);
    out.object_end();
    let per_repetition: Vec<String> = cost.repetitions.iter().map(|cells| format!("{:.3}", cells.iter().map(gate_cost::Cell::ratio).fold(0.0, f64::max))).collect();
    let largest_cell = cost.repetitions.iter().flatten().max_by(|a, b| a.ratio().total_cmp(&b.ratio()));
    let at = largest_cell.map_or_else(String::new, |cell| format!("{} {} at {} workers", cell.case, cell.size, cell.workers));
    let _ = writeln!(
        summary,
        "the gate's cost: the largest of the ratios of an evaluation to a tick is {largest:.3} ({at}); per repetition {}; under a tenth of a tick: {}; \
         of an evaluation, the copy of the table alone is at most {:.3} of a tick; the case open with the pending record read: {:.3}; every verdict as stated: {verdicts}",
        per_repetition.join(", "),
        largest < GATE_COST_BOUND,
        cost.largest_copy(),
        cost.largest_with_the_record_read()
    );
    holds
}

/// The enumeration of small records: with the elements of the first stage,
/// then with those the amended rules add. It reads no oracle. Returns
/// whether every invariant holds.
fn enumerate(out: &mut Json, summary: &mut String, log: &mut String) -> bool {
    let mut holds = true;
    out.key("enumeration_of_small_records");
    out.array_start();
    for (name, elements) in [("the elements of the first stage", enumeration::Elements::FirstStage), ("with the elements of the amended rules", enumeration::Elements::Amended)] {
        let enumeration = enumeration::run(elements);
        out.object_start();
        out.string("elements", name);
        out.u64("records", enumeration.records);
        out.u64("applications", enumeration.applications);
        out.u64("fell_while_wanted", enumeration.fell_while_wanted);
        out.u64("stayed_without_a_want", enumeration.stayed_without_a_want);
        out.u64("order_of_making", enumeration.order_of_making);
        out.u64("round_trip", enumeration.round_trip);
        out.u64("type_changed", enumeration.type_changed);
        out.u64("failed_under_a_new_want", enumeration.failed_under_a_new_want);
        out.u64("events", enumeration.events);
        out.u64("left", enumeration.left);
        out.u64("wanted_and_released_in_one_record", enumeration.wanted_and_released_in_one_record);
        out.u64("order_decides_the_type", enumeration.order_decides_the_type);
        out.string("first_violation", &enumeration.first_violation);
        out.object_end();
        let _ = writeln!(
            summary,
            "enumeration of small records, {name}: {} records, {} applications, invariants hold: {} ({} applications wanted and released an entry with no want inside one record; in {} the order of two wants decides a type)",
            enumeration.records,
            enumeration.applications,
            enumeration.holds(),
            enumeration.wanted_and_released_in_one_record,
            enumeration.order_decides_the_type
        );
        let _ = writeln!(log, "\n== the enumeration of small records, {name}: {enumeration:?}");
        holds &= enumeration.holds();
    }
    out.array_end();
    holds
}

/// Every scenario and every control of the families driven with no oracle:
/// what a world gave is written in words, and nothing is compared. Returns
/// the log and whether no world stayed gated and every log replayed.
fn drive(settings: &Settings, only: &[String]) -> (String, bool) {
    let mut log = String::new();
    let mut clean = true;
    for claim in claims(settings, only, false, false, false) {
        let _ = writeln!(log, "\n== {}", claim.name);
        for outcome in &claim.outcomes {
            if !outcome.observed.is_empty() {
                let _ = writeln!(log, "  {} [{}]", outcome.scenario, outcome.world);
                for line in &outcome.observed {
                    let _ = writeln!(log, "    {line}");
                }
            }
        }
        for note in &claim.notes {
            let _ = writeln!(log, "  note: {note}");
        }
        clean &= claim.one_hash_per_scenario() && claim.outcomes.iter().all(|outcome| outcome.replays != Some(false));
        clean &= !claim.notes.iter().any(|note| note.contains("never ran tick"));
    }
    (log, clean)
}

/// Every reading the build did not take, run alone on every scenario at one
/// worker count: per reading, the scenarios that tell it apart from the
/// build as it stands (the build equals its oracle and the reading differs)
/// and those that equal their oracle under it alone.
fn every_reading(settings: &Settings, only: &[String]) -> String {
    let mut told: BTreeMap<String, (Vec<String>, Vec<String>)> = families::readings().into_iter().map(|reading| (reading.name, (Vec::new(), Vec::new()))).collect();
    for claim in claims(settings, only, true, false, true) {
        for variant in claim.variants.iter().filter(|variant| !variant.control) {
            let equal = claim.outcomes.iter().filter(|outcome| outcome.scenario == variant.scenario).all(|outcome| outcome.differences.is_empty());
            let (apart, equal_under_it) = told.entry(variant.name.clone()).or_default();
            match (equal, variant.differs) {
                (true, true) => apart.push(variant.scenario.clone()),
                (false, false) => equal_under_it.push(variant.scenario.clone()),
                _ => {}
            }
        }
    }
    let mut log = String::new();
    for (reading, (apart, equal_under_it)) in &told {
        let _ = writeln!(log, "{reading}");
        let _ = writeln!(log, "  told apart by {} scenarios: {}", apart.len(), apart.join(", "));
        let _ = writeln!(log, "  the oracle's reading in {} scenarios: {}", equal_under_it.len(), equal_under_it.join(", "));
    }
    log
}

/// Where the full log of a run is written: beside the facts the run script
/// keeps, where that directory exists, and in the working directory otherwise.
fn log_path(cell: &str) -> PathBuf {
    let beside = PathBuf::from("out").join(cell);
    if beside.is_dir() { beside.join("log.txt") } else { PathBuf::from(format!("{PROTOTYPE}-{cell}.log")) }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--reader") => {
            print!("{}", play::child(&arguments[1..], &family, &families::readings()));
            return;
        }
        Some("--spike-reader") => {
            print!("{}", spike_check::child(&arguments[1..]));
            return;
        }
        Some(alone @ ("--first-check" | "--enumeration" | "--gate-cost")) => {
            // The first check alone, the enumeration of small records alone,
            // or the gate's cost alone: none runs a scenario against an
            // oracle nor reads an oracle file.
            let (mut out, mut summary, mut log) = (Json::new(), String::new(), String::new());
            out.object_start();
            let holds = match alone {
                "--first-check" => first_check(&mut out, &mut summary, &mut log),
                "--enumeration" => enumerate(&mut out, &mut summary, &mut log),
                _ => {
                    let repetitions = arguments.get(1).and_then(|count| count.parse().ok()).unwrap_or(GATE_COST_REPETITIONS);
                    gate_cost(&mut out, &mut summary, &mut log, repetitions, &[1, 4, 8])
                }
            };
            out.object_end();
            println!("{}", out.finish());
            eprint!("{summary}{log}");
            eprintln!("holds: {holds}");
            return;
        }
        Some("--readings") => {
            let mut settings = settings("readings");
            settings.workers = vec![4];
            let log = every_reading(&settings, &arguments[1..]);
            let _ = std::fs::remove_dir_all(&settings.scratch);
            println!("{log}");
            return;
        }
        Some("--drive") => {
            let settings = settings("drive");
            let (log, clean) = drive(&settings, &arguments[1..]);
            let _ = std::fs::remove_dir_all(&settings.scratch);
            println!("{log}");
            eprintln!("every world ran to its last tick and every log replayed: {clean}");
            return;
        }
        _ => {}
    }
    let cell = arguments.first().cloned().unwrap_or_else(|| "local".to_string());
    let only = arguments.get(1..).unwrap_or(&[]).to_vec();
    let settings = settings(&cell);
    let (facts, summary, log, holds) = report(&settings, &cell, &only);
    let _ = std::fs::remove_dir_all(&settings.scratch);
    let path = log_path(&cell);
    let written = std::fs::write(&path, format!("{summary}\n{log}"));
    println!("{facts}");
    eprint!("{summary}");
    match written {
        Ok(()) => eprintln!("full log: {}", path.display()),
        Err(error) => eprintln!("the full log could not be written to {}: {error}", path.display()),
    }
    eprintln!("every scenario stands: {holds}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every scenario and every control of every family, driven on the small
    /// size with no oracle, the readers of a save inside the test process:
    /// every world runs to its last tick, every input log replays at another
    /// worker count, and a run hash does not depend on the worker count. The
    /// test compares nothing with an oracle.
    #[test]
    fn every_scenario_is_driven_to_its_last_tick_and_replays() {
        let mut settings = settings("test");
        settings.size = Size::Small;
        settings.workers = vec![1, 4];
        let (log, clean) = drive(&settings, &[]);
        let _ = std::fs::remove_dir_all(&settings.scratch);
        assert!(clean, "{log}");
    }

    #[test]
    fn the_placement_of_the_crates_is_respected() {
        assert!(placement().iter().all(|(_, _, respected)| *respected));
    }
}
