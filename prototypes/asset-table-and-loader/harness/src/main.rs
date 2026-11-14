//! The harness of the prototype of the asset table and of the loader.
//! Throwaway, never product code. See the README beside the workspace.
//!
//! A run prints one JSON document on standard output (the facts the research
//! CI keeps), a short summary on standard error, and writes its full log,
//! every difference with an oracle in words, to a file.
//!
//! Usage: `asset-table-and-loader <cell>`. The readers of a save run as
//! processes of this same binary, with `--reader` or `--spike-reader` first.

mod enumeration;
mod named_set;
mod oracle;
mod restore;
mod run;
mod scripted;
mod sequential;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use prototype_core::world::Size;
use reference_workload::{Json, write_platform_and_build};

use run::{Claim, Settings};

const SCHEMA_VERSION: u64 = 1;
const PROTOTYPE: &str = "asset-table-and-loader";

#[global_allocator]
static ALLOCATOR: spike_check::Counting = spike_check::Counting;

fn settings(scratch: &str) -> Settings {
    let size = if cfg!(debug_assertions) { Size::Small } else { Size::Default };
    let scratch = std::env::temp_dir().join(format!("{PROTOTYPE}-{scratch}-{}", std::process::id()));
    Settings { size, workers: vec![1, 4, 8], scratch }
}

/// The four claims of this stage, each against its oracles.
fn claims(settings: &Settings, processes: bool) -> Vec<Claim> {
    vec![
        scripted::run(
            "the order at a tick's input fixed point gives the stated states",
            settings,
            &scripted::order_family(settings),
            scripted::ORDER,
            include_str!("../oracles/order.txt"),
        ),
        scripted::run("a want's owner orders the table", settings, &scripted::owners_family(settings), scripted::OWNERS, include_str!("../oracles/owners.txt")),
        named_set::run(settings, &named_set::family(settings), include_str!("../oracles/named-set.txt")),
        restore::run(settings, processes),
    ]
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
            Standing::DiffersByAReading => "differs by a reading of the text alone",
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
    if !claim.controls_differ() {
        "a control equals its oracle: the scenario proves nothing"
    } else if standings.values().all(|standing| *standing == Standing::Equal) {
        "equals its oracles"
    } else if standings.values().all(|standing| *standing != Standing::Differs) {
        "equals its oracles but where the text is read two ways"
    } else {
        "differs from an oracle"
    }
}

fn write_claim(out: &mut Json, log: &mut String, claim: &Claim) {
    let standings = standings(claim);
    out.object_start();
    out.string("claim", claim.name);
    out.string("verdict", verdict(claim));
    out.bool("one_run_hash_per_scenario_across_worker_counts", claim.one_hash_per_scenario());
    out.bool("every_control_differs", claim.controls_differ());
    out.key("scenarios");
    out.array_start();
    let _ = writeln!(log, "\n== {}: {}", claim.name, verdict(claim));
    for ((scenario, world), standing) in &standings {
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
    }
    out.array_end();
    out.key("controls_and_readings");
    out.array_start();
    for variant in &claim.variants {
        out.object_start();
        out.string("scenario", &variant.scenario);
        out.string("kind", if variant.control { "control" } else { "reading" });
        out.string("change", variant.name);
        out.bool("differs_from_the_oracle", variant.differs);
        out.string("first_difference", &variant.first_difference);
        out.object_end();
        let kind = if variant.control { "control" } else { "reading" };
        let _ = writeln!(log, "  {} [{kind}] {}: differs {} | {}", variant.scenario, variant.name, variant.differs, variant.first_difference);
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

fn report(settings: &Settings, cell: &str) -> (String, String, String, bool) {
    let mut out = Json::new();
    let mut log = String::new();
    let mut summary = String::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("prototype", PROTOTYPE);
    out.string("stage", "the four claims");
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.string("size", settings.size.name());
    out.string("worker_counts", "1, 4, 8");
    let mut holds = true;

    // The first check: the asset table spike's scenarios, run again.
    let spike_settings = if cfg!(debug_assertions) { spike_check::Settings::unoptimized() } else { spike_check::Settings::measured() };
    out.key("first_check");
    out.object_start();
    let scenarios = spike_check::run(&mut out, &spike_settings);
    out.object_end();
    let _ = writeln!(log, "== the first check: the asset table spike's scenarios, run again");
    let passed = scenarios.iter().filter(|(_, verdict)| *verdict).count();
    for (name, verdict) in &scenarios {
        let _ = writeln!(log, "  {name}: {}", if *verdict { "holds" } else { "does not hold" });
    }
    let _ = writeln!(summary, "first check (the asset table spike's scenarios run again): {passed} of {} hold", scenarios.len());
    holds &= passed == scenarios.len();

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
    for claim in claims(settings, true) {
        write_claim(&mut out, &mut log, &claim);
        let standings = standings(&claim);
        let count = |standing: Standing| standings.values().filter(|other| **other == standing).count();
        let controls = claim.variants.iter().filter(|variant| variant.control).count();
        let differing = claim.variants.iter().filter(|variant| variant.control && variant.differs).count();
        let _ = writeln!(
            summary,
            "{}: {} | {} scenarios equal, {} differ by a reading, {} differ | {differing} of {controls} controls differ | one hash across worker counts: {}",
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

    let enumeration = enumeration::run();
    out.key("enumeration_of_small_records");
    out.object_start();
    out.u64("records", enumeration.records);
    out.u64("applications", enumeration.applications);
    out.u64("fell_while_wanted", enumeration.fell_while_wanted);
    out.u64("stayed_without_a_want", enumeration.stayed_without_a_want);
    out.u64("order_of_making", enumeration.order_of_making);
    out.u64("round_trip", enumeration.round_trip);
    out.u64("wanted_and_released_in_one_record", enumeration.wanted_and_released_in_one_record);
    out.string("first_violation", &enumeration.first_violation);
    out.object_end();
    let _ = writeln!(
        summary,
        "enumeration of small records: {} records, {} applications, invariants hold: {} ({} applications reach the fall read two ways)",
        enumeration.records,
        enumeration.applications,
        enumeration.holds(),
        enumeration.wanted_and_released_in_one_record
    );
    let _ = writeln!(log, "\n== the enumeration of small records: {enumeration:?}");
    holds &= enumeration.holds();

    out.bool("every_scenario_stands", holds);
    out.object_end();
    (out.finish(), summary, log, holds)
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
            print!("{}", restore::child(&arguments[1..]));
            return;
        }
        Some("--spike-reader") => {
            print!("{}", spike_check::child(&arguments[1..]));
            return;
        }
        _ => {}
    }
    let cell = arguments.first().cloned().unwrap_or_else(|| "local".to_string());
    let settings = settings(&cell);
    let (facts, summary, log, holds) = report(&settings, &cell);
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

    /// Every scenario of the four claims on the small size, the readers of a
    /// save inside the test process: no scenario differs from its oracle but
    /// by a reading of the text, and every control differs.
    #[test]
    fn the_four_claims_stand_on_the_small_size() {
        let mut settings = settings("test");
        settings.size = Size::Small;
        for claim in claims(&settings, false) {
            let standings = standings(&claim);
            let differing: Vec<_> = standings.iter().filter(|(_, standing)| **standing == Standing::Differs).collect();
            assert!(differing.is_empty(), "{}: {differing:?}", claim.name);
            assert!(claim.controls_differ(), "{}: a control equals its oracle", claim.name);
            assert!(claim.one_hash_per_scenario(), "{}: a run hash depends on the worker count", claim.name);
        }
        let _ = std::fs::remove_dir_all(&settings.scratch);
    }

    #[test]
    fn the_placement_of_the_crates_is_respected() {
        assert!(placement().iter().all(|(_, _, respected)| *respected));
    }
}
