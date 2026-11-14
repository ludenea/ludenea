//! The scenarios of one claim: a history is never continued under other
//! content, in its half on the restore under the content at hand. Throwaway,
//! never product code.
//!
//! One world is saved with a table that holds available assets, a failed
//! entry, an unanswered required want and an entry with no want. Each reader
//! loads that save as a restore under the content at hand, in a process of
//! its own, over a store of its own.

use std::fs;
use std::path::Path;
use std::process::Command;

use asset_loader::{Attachment, Loader, Options};
use content_store::set_hash;
use prototype_core::table::{AssetId, Policy, Record, Rules};
use prototype_core::world::{Links, Restore, Size, Wait, World, WorldConfig};
use reference_workload::run_hash;

use crate::oracle::{Expected, Names, Verdict, parse};
use crate::run::{Act, BASE, Claim, Content, Driven, Family, Outcome, Settings, Variant, config, content, import, judge, late_extract};

pub const NAME: &str = "a history is never continued under other content, on a restore under the content at hand";
pub const ORACLES: &str = include_str!("../oracles/restore.txt");
const SAVED: &str = "restore/saved-world";
const SAVE_TICK: u32 = 8;
const LAST: u32 = 16;
const BASE_CONTENT: [&str; 9] = ["D1.1", "D2.1", "F.1", "L1.1", "L2.1", "P.1", "T.1", "U.1", "Z.1"];

/// A reader: the content at hand as changes to the saved world's, and what
/// its controls and readings change.
struct Reader {
    name: &'static str,
    /// Contents written over the saved world's.
    replaced: &'static [&'static str],
    /// Assets the content at hand does not hold.
    absent: &'static [&'static str],
    variants: &'static [(&'static str, bool)],
}

const EVENT_FIRST: &str = "reading: the restore's event first";

const READERS: &[Reader] = &[
    Reader { name: "restore/same-content-other-set", replaced: &["M.1"], absent: &[], variants: &[("the set's hash under the state hash", true)] },
    Reader { name: "restore/one-changed", replaced: &["L1.2"], absent: &[], variants: &[("the answers posted unmarked", true), (EVENT_FIRST, false)] },
    Reader {
        name: "restore/one-absent",
        replaced: &[],
        absent: &["L1"],
        variants: &[
            ("no answer posted for the absent asset", true),
            ("reading: an answer to a restore that fails emits its event and fails the instance", false),
            ("reading: the restore's event first + reading: an answer to a restore that fails fails the instance and emits no event", false),
        ],
    },
    Reader {
        name: "restore/entries-not-available",
        replaced: &[],
        absent: &[],
        variants: &[("the failed entry answered again", true), ("the unanswered want dropped", true), ("the entry with no want answered", true)],
    },
    Reader { name: "restore/unanswered-at-another-hash", replaced: &["U.2"], absent: &[], variants: &[] },
    Reader { name: "restore/revision-while-waiting", replaced: &[], absent: &[], variants: &[("a marked answer replaced in the queue", true)] },
    Reader {
        name: "restore/parent-with-another-dependency",
        replaced: &["P.2"],
        absent: &[],
        variants: &[(EVENT_FIRST, false), ("reading: the answers to a restore do not follow the closure", false)],
    },
    Reader { name: "restore/another-composition-refused", replaced: &[], absent: &[], variants: &[("the entry dropped in silence", true)] },
    Reader { name: "restore/another-composition-dropped", replaced: &[], absent: &[], variants: &[("the entry dropped in silence", true), (EVENT_FIRST, false)] },
];

pub fn family(settings: &Settings) -> Family {
    let contents = [
        content("D1", 1),
        content("D2", 1),
        content("F", 1),
        content("L1", 1),
        content("L1", 2),
        content("L2", 1),
        Content { asset: "P", version: 1, asset_type: 1, dependencies: &["D1"] },
        Content { asset: "P", version: 2, asset_type: 1, dependencies: &["D2"] },
        Content { asset: "T", version: 1, asset_type: 2, dependencies: &[] },
        content("U", 1),
        content("U", 2),
        content("Z", 1),
        content("M", 1),
    ];
    let assets = ["D1", "D2", "F", "L1", "L2", "P", "T", "U", "Z", "M"];
    Family::new(&assets, &["L1", "L2", "P", "U"], &contents, &[("one", Policy::Required, true)], settings)
}

fn composed(settings: &Settings, family: &Family, workers: usize, asset_types: &[u32]) -> WorldConfig {
    let wants = [(family.asset("L2"), Policy::Required), (family.asset("P"), Policy::Required)];
    let mut config = config(settings, workers, &family.names).wanting(BASE, &wants);
    config.asset_types = asset_types.to_vec();
    config
}

/// Writes a content root and imports it into a loose store, with the
/// artifact of `F` cut short under its name unless `whole`.
fn store(family: &Family, directory: &Path, contents: &[&str], whole: bool) {
    family.write_sources(&directory.join("content"), contents);
    let (cache, _) = import(&directory.join("content"), &directory.join("store"));
    if !whole && contents.contains(&"F.1") {
        cache.cut_object(family.hash("F.1")).expect("the artifact of F is in the store");
    }
}

/// The saved world: run to the tick of the save, saved, then run on to the
/// end as the uninterrupted run.
fn saved_world(settings: &Settings, family: &Family, workers: usize) -> (Driven, Vec<u8>) {
    let directory = settings.directory(&format!("{SAVED}-{workers}"));
    store(family, &directory, &BASE_CONTENT, false);
    let loader = Loader::start(&directory.join("store"), Options::default()).expect("the loading capability starts");
    let (extract, slot) = late_extract();
    let mut links = Links::new(loader.residents());
    links.extract = Some(extract);
    let queue = links.queue.clone();
    let world = World::new(composed(settings, family, workers, &[1, 2]), Vec::new(), links);
    let attachment = Attachment { queue: queue.clone(), set: None, unanswered: world.unanswered(), named: world.named(), restore: Vec::new() };
    let _ = slot.set(loader.attach(attachment).expect("the world is attached"));
    let acts = [
        (2, Act::Post("instantiate one L1 required reload on")),
        (2, Act::Post("want Z source:host optional")),
        (2, Act::Post("want F source:host optional")),
        (2, Act::Post("want T source:host optional")),
        (8, Act::Post("release Z source:host")),
        (8, Act::Post("want U source:host required")),
    ];
    let mut driven = Driven::new(world, queue);
    driven.drive(SAVE_TICK, &acts, &family.names, &|| loader.settle());
    let save = driven.world.save();
    driven.drive(LAST, &acts, &family.names, &|| loader.settle());
    (driven, save)
}

/// What one reader gave.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub differences: Vec<String>,
    pub notes: Vec<String>,
    pub run_hash: u64,
    pub replays: Option<bool>,
}

/// Loads the save as a restore under the content at hand and judges the
/// reader against its oracle. `variant` names a control or a reading of the
/// reader, or is empty.
pub fn read(settings: &Settings, scenario: &str, workers: usize, save: &[u8], uninterrupted: &[u64], variant: &str) -> Read {
    let family = family(settings);
    let names = &family.names;
    let reader = READERS.iter().find(|reader| reader.name == scenario).expect("a reader of the scenario sheet");
    let stated = parse(ORACLES, names).unwrap_or_else(|error| panic!("the oracle of the restore does not read: {error}"));
    let nothing = Expected::default();
    let expected = stated.get(scenario).and_then(|worlds| worlds.get("world")).unwrap_or(&nothing);
    let before = stated.get(SAVED).and_then(|worlds| worlds.get("world")).unwrap_or(&nothing);

    let (mut rules, mut options) = (Rules::default(), Options::default());
    let mut set_under_the_hash = false;
    let mut drop_in_silence = false;
    let (mut answer_the_failed_entry, mut drop_the_unanswered_want) = (false, false);
    for change in variant.split(" + ").filter(|change| !change.is_empty()) {
      match change {
        "the set's hash under the state hash" => set_under_the_hash = true,
        "the answers posted unmarked" => options.answers_unmarked = true,
        "no answer posted for the absent asset" => options.no_answer_for_an_absent_asset = true,
        "reading: an answer to a restore that fails emits its event and fails the instance" => rules.marked_failure_emits = true,
        "reading: an answer to a restore that fails fails the instance and emits no event" => rules.marked_failure_fails_the_instance = true,
        "the failed entry answered again" => answer_the_failed_entry = true,
        "the unanswered want dropped" => drop_the_unanswered_want = true,
        "the entry with no want answered" => rules.restore_awaits_entries_without_a_want = true,
        "a marked answer replaced in the queue" => options.revisions_not_held = true,
        "the entry dropped in silence" => drop_in_silence = true,
        "reading: the restore's event first" => rules.restore_event_first = true,
        "reading: the answers to a restore do not follow the closure" => options.restore_answers_do_not_follow_the_closure = true,
        other => panic!("unknown variant {other}"),
      }
    }

    let directory = settings.directory(&format!("{scenario}-{workers}-{}", variant.len()));
    let mut contents: Vec<&str> = BASE_CONTENT.iter().copied().filter(|name| !reader.absent.contains(&name.split('.').next().unwrap_or(""))).collect();
    for replaced in reader.replaced {
        let asset = replaced.split('.').next().unwrap_or("");
        contents.retain(|name| name.split('.').next() != Some(asset));
        contents.push(replaced);
    }
    store(&family, &directory, &contents, false);
    let loader = Loader::start(&directory.join("store"), options).expect("the loading capability starts");
    let manifest = loader.manifest(None);
    let type_of = |asset: AssetId| manifest.get(&asset).map(|entry| entry.asset_type);
    let another_composition = scenario.starts_with("restore/another-composition");
    let asset_types: &[u32] = if another_composition { &[1] } else { &[1, 2] };
    let dropped: Vec<AssetId> = if scenario == "restore/another-composition-dropped" { vec![family.asset("T")] } else { Vec::new() };
    let set = (scenario == "restore/same-content-other-set").then(|| set_hash(&manifest));
    let configured = |workers: usize| {
        let mut config = composed(settings, &family, workers, asset_types);
        config.content_set = set;
        config.rules = rules;
        config.set_under_the_hash = set_under_the_hash;
        config
    };
    let request = || Restore::ContentAtHand { type_of: &type_of, dropped: &dropped, drop_in_silence };

    let (extract, slot) = late_extract();
    let mut links = Links::new(loader.residents());
    links.extract = Some(extract);
    let queue = links.queue.clone();
    let mut result = Read::default();
    let world = match World::restore(configured(workers), save, Vec::new(), links, request()) {
        Ok(world) => world,
        Err(refusal) => {
            let observed = crate::oracle::Observed { first: SAVE_TICK, verdict: Some(Verdict::Refused(names.asset_name(refusal.asset))), ..Default::default() };
            result.differences = crate::oracle::compare(expected, &observed, LAST, &[], names);
            result.notes.push(format!("the restore is refused, naming {}: {}", names.asset_name(refusal.asset), refusal.reason));
            return result;
        }
    };
    let mut driven = Driven::new(world, queue.clone());
    // A step without waiting, before the availability source has answered.
    let first = driven.world.step(1, Wait::No).expect("the world does not fail");
    if first.ticks_ran == 0 {
        driven.observed.blocked.push((SAVE_TICK + 1, first.unanswered));
    } else {
        result.notes.push("the first tick ran before any answer".to_string());
        driven.observed.tables.push(driven.world.table_bytes());
    }
    let mut restore = driven.world.restore_query();
    if answer_the_failed_entry {
        restore.push((family.asset("F"), 0));
    }
    let unanswered = if drop_the_unanswered_want { Vec::new() } else { driven.world.unanswered() };
    let attachment = Attachment { queue, set, unanswered, named: driven.world.named(), restore };
    let _ = slot.set(loader.attach(attachment).expect("the world is attached"));
    if scenario == "restore/revision-while-waiting" {
        family.write_sources(&directory.join("content"), &["L1.2"]);
        import(&directory.join("content"), &directory.join("store"));
        loader.revision_in_place().expect("the revision is read");
    }
    let mut acts = Vec::new();
    if scenario == "restore/entries-not-available" {
        // The content at hand then holds F whole: its artifact is written again under its name.
        let (family, store) = (family.clone(), directory.join("store"));
        let whole = move || {
            let cache = asset_pipeline::Cache::on_disk(&store).expect("the loose store opens");
            let _ = cache.remove_object(family.hash("F.1"));
            cache.put_object(&family.sources["F.1"].part()).expect("the artifact of F is written whole");
        };
        acts.push((11, Act::Call(Box::new(whole))));
        acts.push((11, Act::Post("want F source:tool optional")));
    }
    driven.drive(LAST, &acts, names, &|| loader.settle());
    result.differences = judge(settings, names, expected, &driven.observed, Some(before), LAST);
    result.run_hash = run_hash(&driven.observed.state);
    result.notes.extend(observations(scenario, &driven, uninterrupted, &loader));

    // The reader's own log, read back from its bytes, replays the restore at another worker count.
    if driven.observed.stuck.is_none() {
        let other = if workers == 1 { 4 } else { 1 };
        let records: Vec<Record> = prototype_core::table::decode_log(&prototype_core::table::encode_log(&driven.observed.log)).expect("a log decodes");
        let links = Links::new(loader.residents());
        let mut replayed = World::restore(configured(other), save, records, links, request()).expect("the restore is admitted again");
        replayed.step(LAST - SAVE_TICK, Wait::No).expect("the replay does not fail");
        result.replays = Some(replayed.state_hashes() == driven.observed.state);
    }
    result
}

/// What a reader shows beside its oracle: how its hashes stand against the
/// uninterrupted run's, and what the loading capability held back.
fn observations(scenario: &str, driven: &Driven, uninterrupted: &[u64], loader: &Loader) -> Vec<String> {
    let mut notes = Vec::new();
    let from = SAVE_TICK as usize;
    let ours = &driven.observed.state;
    if uninterrupted.len() > from && ours.len() == uninterrupted.len() - from {
        let first = ours.iter().zip(&uninterrupted[from..]).position(|(ours, theirs)| ours != theirs);
        notes.push(match first {
            None => format!("{scenario}: the state hashes equal the uninterrupted run's from tick {SAVE_TICK} to tick {LAST}"),
            Some(at) => format!("{scenario}: the state hashes differ from the uninterrupted run's from tick {}", SAVE_TICK as usize + at),
        });
    }
    let report = loader.report();
    if report.held_moves > 0 {
        notes.push(format!("{scenario}: {} move of the manifest was held until the first tick had applied the answers", report.held_moves));
    }
    notes
}

fn settings_text(settings: &Settings) -> &'static str {
    settings.size.name()
}

/// Runs one reader as a process of its own and reads what it prints.
fn read_in_a_process(settings: &Settings, scenario: &str, workers: usize, save: &Path, variant: &str) -> Read {
    let exe = std::env::current_exe().expect("the harness knows its own path");
    let output = Command::new(exe)
        .args(["--reader", scenario, &workers.to_string(), &save.to_string_lossy(), &settings.scratch.to_string_lossy(), settings_text(settings), variant])
        .output()
        .expect("a reader starts");
    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = Read::default();
    if !output.status.success() {
        result.differences.push(format!("the reader ended with {}: {}", output.status, String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("")));
    }
    for line in text.lines() {
        match line.split_once(' ') {
            Some(("difference", text)) => result.differences.push(text.to_string()),
            Some(("note", text)) => result.notes.push(text.to_string()),
            Some(("hash", text)) => result.run_hash = u64::from_str_radix(text, 16).unwrap_or(0),
            Some(("replays", text)) => result.replays = Some(text == "true"),
            _ => {}
        }
    }
    result
}

/// The reader as a process: `--reader <scenario> <workers> <save> <scratch> <size> <variant>`.
pub fn child(arguments: &[String]) -> String {
    let [scenario, workers, save, scratch, size, variant] = arguments else { return "difference the reader was not given its six arguments\n".to_string() };
    let size = if size == "small" { Size::Small } else { Size::Default };
    let settings = Settings { size, workers: Vec::new(), scratch: Path::new(scratch).to_path_buf() };
    let bytes = fs::read(save).expect("the save is read");
    let uninterrupted = read_hashes(&Path::new(save).with_extension("hashes"));
    let result = read(&settings, scenario, workers.parse().expect("a worker count"), &bytes, &uninterrupted, variant);
    let mut out = String::new();
    for difference in &result.differences {
        out.push_str(&format!("difference {difference}\n"));
    }
    for note in &result.notes {
        out.push_str(&format!("note {note}\n"));
    }
    out.push_str(&format!("hash {:016x}\n", result.run_hash));
    if let Some(replays) = result.replays {
        out.push_str(&format!("replays {replays}\n"));
    }
    out
}

fn write_hashes(path: &Path, hashes: &[u64]) {
    let text: String = hashes.iter().map(|hash| format!("{hash:016x}\n")).collect();
    fs::write(path, text).expect("the hashes are written");
}

fn read_hashes(path: &Path) -> Vec<u64> {
    fs::read_to_string(path).map(|text| text.lines().filter_map(|line| u64::from_str_radix(line, 16).ok()).collect()).unwrap_or_default()
}

/// Runs the claim: the saved world against its oracle, then every reader,
/// each a process of its own where `processes` is set, at every worker
/// count; then each control and reading at one worker count.
pub fn run(settings: &Settings, processes: bool) -> Claim {
    let mut claim = Claim { name: NAME, ..Claim::default() };
    let family = family(settings);
    let names: &Names = &family.names;
    let stated = parse(ORACLES, names).unwrap_or_else(|error| panic!("the oracle of the restore does not read: {error}"));
    let nothing = Expected::default();
    let expected = stated.get(SAVED).and_then(|worlds| worlds.get("world")).unwrap_or(&nothing);
    let mut save_at_four = None;
    for &workers in &settings.workers {
        let (driven, save) = saved_world(settings, &family, workers);
        let differences = judge(settings, names, expected, &driven.observed, None, LAST);
        claim.outcomes.push(Outcome::new(SAVED, "world", workers, &driven.observed, differences));
        let path = settings.directory(&format!("save-{workers}")).join("world.save");
        fs::write(&path, &save).expect("the save is written");
        write_hashes(&path.with_extension("hashes"), &driven.observed.state);
        if workers == 4 || save_at_four.is_none() {
            save_at_four = Some((path.clone(), save.clone(), driven.observed.state.clone()));
        }
        for reader in READERS {
            let result = if processes {
                read_in_a_process(settings, reader.name, workers, &path, "")
            } else {
                read(settings, reader.name, workers, &save, &driven.observed.state, "")
            };
            claim.outcomes.push(Outcome {
                scenario: reader.name.to_string(),
                world: "world".to_string(),
                workers,
                differences: result.differences,
                run_hash: result.run_hash,
                replays: result.replays,
            });
            if workers == settings.workers[0] {
                claim.notes.extend(result.notes);
            }
        }
    }
    let (path, save, uninterrupted) = save_at_four.expect("at least one worker count");
    for reader in READERS {
        for (variant, control) in reader.variants {
            let result = if processes { read_in_a_process(settings, reader.name, 4, &path, variant) } else { read(settings, reader.name, 4, &save, &uninterrupted, variant) };
            claim.variants.push(Variant {
                scenario: reader.name.to_string(),
                name: variant,
                control: *control,
                differs: !result.differences.is_empty(),
                first_difference: result.differences.first().cloned().unwrap_or_default(),
            });
        }
    }
    claim
}
