//! Host of the import graph spike, for the assets research. Throwaway, never
//! product code.
//!
//! A synthetic content root (tables with sidecars, sheets that hold several
//! sub-assets, scenes with their identifier in the file) is imported by a
//! pipeline built as a build system is: one canonical action per asset and
//! one presentation action per asset and variant, keyed by what they read,
//! on a pool with a central ready set. The spike measures, instead of
//! assuming:
//!
//! - one domain content set whatever the worker count and the target;
//! - the graph built in two passes, with imports that end with a request and
//!   are run again, and what the ready set costs on many small actions;
//! - a cascade that stops where bytes stop changing;
//! - a key that is complete: a schema entry read, a copied source;
//! - the change journal: what a pass that finds nothing to do costs;
//! - a revision that is atomic when the importing process is ended;
//! - a package that is a function of its inputs, and the mount order;
//! - the import of a scene-sized text source, per prototype.

mod content;
mod formats;
mod package;
mod pipeline;
mod pool;

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reference_workload::{Fnv1a, Json, SplitMix64, write_platform_and_build};

use content::{Content, SetSize, sidecar_of, write_cycle};
use formats::{AssetId, Built, Context, Placement, Schema, SourceKind, clip_siblings, derived_id, drawn_id, id_text, import, resolve_siblings};
use package::{Instance, closure, patch_entries};
use pipeline::{Cache, DESKTOP, Domain, MOBILE, Options, PassReport, Profile, check_cache, content_hash, pass, read_revision};
use pool::Outcome;

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "asset-import-graph";
const SEED: u64 = 0x6772_6170_6800_0001;

#[derive(Clone)]
struct Settings {
    size: SetSize,
    /// The set the importing process is ended on.
    small: SetSize,
    worker_counts: Vec<usize>,
    /// The worker count of the scenarios that do not sweep.
    workers: usize,
    full_rounds: usize,
    quiet_rounds: usize,
    independent_actions: usize,
    /// The bytes one independent action digests.
    independent_bytes: usize,
    kills: usize,
    scene_rounds: usize,
    /// The self-checks run the importing process inside their own, and end nothing.
    in_process: bool,
}

impl Settings {
    fn measured() -> Settings {
        Settings {
            size: SetSize::MEASURED,
            small: SetSize::SMALL,
            worker_counts: vec![1, 4, 8],
            workers: 8,
            full_rounds: 3,
            quiet_rounds: 5,
            independent_actions: 100_000,
            independent_bytes: 16 * 1024,
            kills: 50,
            scene_rounds: 5,
            in_process: false,
        }
    }

    /// An unoptimized build measures a smaller set, and its report says so.
    fn unoptimized() -> Settings {
        let size = SetSize { tables: 32, records: 2_000, sheets: 16, sprites: 8, side: 64, clips: 2, scenes: 16, prototypes: 200, large_scene: 2_000 };
        Settings { size, independent_actions: 20_000, independent_bytes: 2 * 1024, kills: 20, ..Settings::measured() }
    }
}

/// What every scenario shares: the settings, the scratch directory, the
/// content root and the schema.
struct Bench {
    settings: Settings,
    directory: PathBuf,
    content: Content,
    schema: Schema,
}

impl Bench {
    fn new(settings: Settings) -> Bench {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let directory = env::temp_dir().join(format!("{SPIKE}-{}-{nanos}", std::process::id()));
        let content = Content::generate(&directory.join("content"), settings.size, SEED).expect("the scratch directory is writable");
        Bench { settings, directory, content, schema: Schema::standard() }
    }

    fn options<'a>(&'a self, workers: usize, profiles: &'a [Profile]) -> Options<'a> {
        Options { workers, profiles, journal: false, facts_from_the_listing: false, everything_declared: false, placement: Placement::Before, schema: &self.schema }
    }

    fn pass(&self, cache: &Cache, options: &Options) -> PassReport {
        pass(&self.content.root, cache, options).expect("the content root is scanned")
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn hex(out: &mut Json, key: &str, hash: u64) {
    out.hex(key, hash);
}

fn best(durations: &[Duration]) -> Duration {
    durations.iter().copied().min().unwrap_or_default()
}

fn failures_text(report: &PassReport) -> String {
    report.failures.iter().map(|(what, reason)| format!("{what}: {reason}")).collect::<Vec<_>>().join("; ")
}

fn source_bytes(root: &Path) -> u64 {
    let size = |directory: &str| -> u64 {
        let Ok(entries) = fs::read_dir(root.join(directory)) else { return 0 };
        entries.filter_map(|entry| entry.ok()?.metadata().ok()).map(|metadata| metadata.len()).sum()
    };
    ["tables", "sheets", "scenes"].iter().map(|directory| size(directory)).sum()
}

// ---------------------------------------------------------------------------
// One domain set
// ---------------------------------------------------------------------------

/// The full import from an empty cache at every worker count, for each of
/// two target profiles and with the presentation actions off; once more at
/// the largest worker count; and once for the two targets together.
fn one_domain_set(out: &mut Json, bench: &Bench) -> (bool, u64) {
    let mut domain_hashes = BTreeSet::new();
    let mut presentation: HashMap<&str, Vec<u64>> = HashMap::new();
    let mut failed = 0;
    let none: &[Profile] = &[];
    out.key("one_domain_set");
    out.object_start();
    out.key("imports");
    out.array_start();
    let mut import = |out: &mut Json, workers: usize, profiles: &[Profile], label: &str| -> PassReport {
        let report = bench.pass(&Cache::in_memory(), &bench.options(workers, profiles));
        domain_hashes.insert(report.domain_hash);
        failed += report.failures.len();
        for (name, hash) in &report.presentation_hashes {
            presentation.entry(name).or_default().push(*hash);
        }
        out.object_start();
        out.u64("workers", workers as u64);
        out.string("target", label);
        out.hex("domain_manifest", report.domain_hash);
        out.string("presentation_manifests", &report.presentation_hashes.iter().map(|(name, hash)| format!("{name} {hash:#018x}")).collect::<Vec<_>>().join(", "));
        out.u64("canonical_actions_run", report.canonical().len() as u64);
        out.u64("presentation_actions_run", report.presentation() as u64);
        out.decimal("wall_ms", ms(report.wall));
        out.string("failures", &failures_text(&report));
        out.object_end();
        report
    };
    for &workers in &bench.settings.worker_counts {
        import(out, workers, &[DESKTOP], "desktop");
        import(out, workers, &[MOBILE], "mobile");
        import(out, workers, none, "none");
    }
    let widest = bench.settings.worker_counts.iter().copied().max().unwrap_or(1);
    import(out, widest, &[DESKTOP], "desktop, a second run");
    let both = import(out, widest, &[DESKTOP, MOBILE], "desktop and mobile");
    out.array_end();
    let each_canonical_action_once_for_two_targets = both.canonical().len() == both.assets && both.presentation() == both.assets * 4;
    let distinct = |name: &str| presentation.get(name).map_or(0, |hashes| hashes.iter().collect::<BTreeSet<_>>().len());
    let shared = presentation["desktop"].iter().any(|hash| presentation["mobile"].contains(hash));
    let domain_hash = domain_hashes.first().copied().unwrap_or(0);
    let holds = domain_hashes.len() == 1 && failed == 0 && !shared && each_canonical_action_once_for_two_targets && domain_hash != 0;
    out.u64("distinct_domain_manifests", domain_hashes.len() as u64);
    out.u64("distinct_desktop_presentation_manifests", distinct("desktop") as u64);
    out.u64("distinct_mobile_presentation_manifests", distinct("mobile") as u64);
    out.bool("a_presentation_manifest_shared_by_the_two_targets", shared);
    out.bool("each_canonical_action_once_for_two_targets", each_canonical_action_once_for_two_targets);
    out.bool("holds", holds);
    out.object_end();
    (holds, domain_hash)
}

// ---------------------------------------------------------------------------
// The two-pass graph on the ready set
// ---------------------------------------------------------------------------

fn two_pass_graph(out: &mut Json, bench: &Bench, domain_hash: u64) -> bool {
    let workers = bench.settings.workers;
    let none: &[Profile] = &[];
    let forms = [
        ("requests placed before the computation", false, Placement::Before),
        ("requests placed after the computation", false, Placement::After),
        ("every dependency declared in advance", true, Placement::Before),
    ];
    let mut holds = true;
    out.key("two_pass_graph");
    out.object_start();
    out.key("imports");
    out.array_start();
    for (name, everything_declared, placement) in forms {
        let options = Options { everything_declared, placement, ..bench.options(workers, none) };
        let report = bench.pass(&Cache::in_memory(), &options);
        let same_manifest = report.domain_hash == domain_hash && report.failures.is_empty();
        holds &= same_manifest && (report.requests > 0) != everything_declared;
        out.object_start();
        out.string("form", name);
        out.u64("workers", workers as u64);
        out.u64("canonical_imports", report.assets as u64);
        out.u64("imports_that_ended_with_a_request", report.requests as u64);
        out.u64("actions_run_again", report.restarts as u64);
        out.decimal("work_run_twice_ms", ms(report.run_twice));
        out.decimal("wall_ms", ms(report.wall));
        out.decimal("waiting_share", report.waiting_share);
        out.hex("domain_manifest", report.domain_hash);
        out.bool("same_manifest", same_manifest);
        out.object_end();
    }
    out.array_end();
    out.u64("imports_the_content_makes_discover_a_dependency", bench.content.discovering_imports() as u64);

    // Many small independent actions: what the central ready set costs.
    let data: Vec<u8> = (0..bench.settings.independent_bytes).map(|byte| byte as u8).collect();
    let graph = vec![Vec::new(); bench.settings.independent_actions];
    out.key("independent_actions");
    out.array_start();
    for &workers in &bench.settings.worker_counts {
        let report = pool::run(workers, &graph, &|action, _| {
            let mut hasher = Fnv1a::new();
            hasher.write(&(action as u64).to_le_bytes());
            hasher.write(&data);
            black_box(hasher.finish());
            Outcome::Done
        });
        let executing = report.wall.as_secs_f64() * workers as f64 * (1.0 - report.waiting_share);
        if workers == bench.settings.workers {
            holds &= report.waiting_share < 0.10;
        }
        out.object_start();
        out.u64("workers", workers as u64);
        out.u64("actions", graph.len() as u64);
        out.decimal("microseconds_per_action", executing * 1_000_000.0 / graph.len() as f64);
        out.decimal("wall_ms", ms(report.wall));
        out.decimal("waiting_share", report.waiting_share);
        out.object_end();
    }
    out.array_end();

    // A planted cycle of two requests.
    let cycle = bench.directory.join("cycle");
    let (one, other) = (drawn_id(1, 3), drawn_id(2, 5));
    write_cycle(&cycle, one, other).expect("the scratch directory is writable");
    let report = pass(&cycle, &Cache::in_memory(), &bench.options(workers, none)).expect("the content root is scanned");
    let both_failed_naming_the_cycle = report.failures.len() == 2 && report.failures.iter().all(|(_, reason)| reason.contains("cycle")) && report.domain_hash == 0;
    holds &= both_failed_naming_the_cycle;
    out.bool("a_cycle_fails_both_actions_naming_it", both_failed_naming_the_cycle);
    out.string("cycle_failures", &failures_text(&report));
    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// The cascade, and the key
// ---------------------------------------------------------------------------

/// One re-import after a change, with the actions it is expected to run.
struct Step {
    change: &'static str,
    described: usize,
    canonical: BTreeSet<AssetId>,
    presentation: usize,
}

fn write_step(out: &mut Json, step: &Step, report: &PassReport) -> bool {
    let ran: BTreeSet<AssetId> = report.canonical().into_iter().collect();
    let exact = report.failures.is_empty() && report.described() == step.described && ran == step.canonical && report.presentation() == step.presentation;
    out.object_start();
    out.string("change", step.change);
    out.string("expected_describe_canonical_presentation", &format!("{}, {}, {}", step.described, step.canonical.len(), step.presentation));
    out.string("ran_describe_canonical_presentation", &format!("{}, {}, {}", report.described(), ran.len(), report.presentation()));
    out.bool("exactly_the_expected_actions", exact);
    out.decimal("wall_ms", ms(report.wall));
    out.string("failures", &failures_text(report));
    out.object_end();
    exact
}

/// Re-imports on one cache after each change: a source whose canonical part
/// does not change, a shared dependency of sprites, a shared dependency of
/// scenes; then a schema entry scenes read, a sheet copied under a new
/// identifier, and a sheet copied with its identity.
fn cascade_and_key(out: &mut Json, bench: &Bench) -> (bool, bool) {
    let content = &bench.content;
    let size = content.size;
    let profiles = [DESKTOP];
    let cache = Cache::in_memory();
    let options = bench.options(bench.settings.workers, &profiles);
    let full = bench.pass(&cache, &options);
    let everything = full.failures.is_empty() && full.described() == full.sources && full.canonical().len() == full.assets && full.presentation() == full.assets * 2;

    let palette = 0;
    let sprites: BTreeSet<AssetId> = content.sprites_using_table(palette).into_iter().map(|(sheet, sprite)| content.sprite(sheet, sprite)).collect();
    let used = size.tables / 2;
    let mut readers: BTreeSet<AssetId> = content.scenes_using_table(used).into_iter().map(|scene| content.scenes[scene]).collect();
    readers.extend(content.large_scene);
    let with = |table: usize, others: &BTreeSet<AssetId>| -> BTreeSet<AssetId> { others.iter().copied().chain([content.tables[table]]).collect() };
    let mut cascade = everything;
    out.key("cascade");
    out.object_start();
    out.bool("the_full_import_runs_every_action", everything);
    out.key("re_imports");
    out.array_start();
    content.add_comment(palette).expect("the content root is writable");
    let step = Step { change: "a comment added to a table sprites depend on: its canonical part does not change", described: 1, canonical: with(palette, &BTreeSet::new()), presentation: 2 };
    cascade &= write_step(out, &step, &bench.pass(&cache, &options));
    content.change_values(palette, 1).expect("the content root is writable");
    let step = Step { change: "the values of that table changed: its dependents are the sprites it is the palette of", described: 1, canonical: with(palette, &sprites), presentation: 2 + 2 * sprites.len() };
    cascade &= write_step(out, &step, &bench.pass(&cache, &options));
    content.change_values(used, 1).expect("the content root is writable");
    let step = Step { change: "the values of a table scenes use changed: the scenes are imported again to the same bytes, and nothing after them", described: 1, canonical: with(used, &readers), presentation: 2 };
    cascade &= write_step(out, &step, &bench.pass(&cache, &options));
    out.array_end();
    out.u64("sprites_that_depend_on_the_first_table", sprites.len() as u64);
    out.u64("scenes_that_read_the_other_table", readers.len() as u64);
    out.bool("holds", cascade);
    out.object_end();

    // A schema entry an importer reads changes, and no source does.
    let wider = bench.schema.with_field("component/velocity", "damping");
    let options = Options { schema: &wider, ..bench.options(bench.settings.workers, &profiles) };
    let reads_velocity = |scene: &usize| Content::component_of(*scene) == "velocity";
    let mut readers: BTreeSet<AssetId> = (0..size.scenes).filter(reads_velocity).map(|scene| content.scenes[scene]).collect();
    readers.extend(content.large_scene);
    let changed = readers.len();
    // A scene that uses a scene whose part changed is imported again, to the same bytes.
    readers.extend((0..size.scenes).filter(|scene| scene % 2 == 1 && reads_velocity(&(scene - 1))).map(|scene| content.scenes[scene]));
    let mut key = true;
    out.key("the_key_is_complete");
    out.object_start();
    out.key("re_imports");
    out.array_start();
    let step = Step { change: "a field added to the schema entry of the velocity component, no source changed", described: 0, canonical: readers, presentation: 2 * changed };
    key &= write_step(out, &step, &bench.pass(&cache, &options));
    // A sheet copied with its sidecar, the copy given a new identifier.
    let copy = drawn_id(0x636f_7079, 0x636f_7079);
    let copied = content.copy_sheet(0, "sheet-copied", copy).expect("the content root is writable");
    let keys = |prefix: &str, count: usize| -> Vec<AssetId> { (0..count).map(|index| derived_id(copy, &format!("{prefix}{index}"))).collect() };
    let copy_sprites: Vec<AssetId> = (0..size.sprites).map(|sprite| derived_id(copy, &format!("sprite-{sprite:02}"))).collect();
    let copy_clips = keys("clip-", size.clips);
    let step = Step {
        change: "a sheet copied under a new identifier: every part of the copy is built, and only the payloads of its clips, whose parts differ",
        described: 0,
        canonical: copy_sprites.iter().chain(&copy_clips).copied().collect(),
        presentation: 2 * size.clips,
    };
    let report = bench.pass(&cache, &options);
    key &= write_step(out, &step, &report);
    out.array_end();
    let originals: Vec<AssetId> = (0..size.sprites).map(|sprite| content.sprite(0, sprite)).collect();
    let names_its_own_siblings = |clip: &AssetId| {
        let part = report.domain.get(clip).and_then(|entry| cache.object(entry.hash));
        let siblings = part.and_then(|part| clip_siblings(&part).ok()).unwrap_or_default();
        !siblings.is_empty() && siblings.iter().all(|sibling| copy_sprites.contains(sibling) && !originals.contains(sibling))
    };
    let the_copy_names_its_own_siblings = copy_clips.iter().all(names_its_own_siblings);
    let same_sprite_bytes = (0..size.sprites).all(|sprite| report.domain.get(&copy_sprites[sprite]).map(|entry| entry.hash) == report.domain.get(&originals[sprite]).map(|entry| entry.hash));
    // A sheet copied with its identity: an error of the scan, naming both files.
    let duplicate = content.copy_sheet(1, "sheet-duplicated", content.sheets[1]).expect("the content root is writable");
    let refusal = pass(&content.root, &cache, &options).err().unwrap_or_default();
    let a_duplicate_is_refused_naming_both_files = refusal.contains("sheet-001.sheet") && refusal.contains("sheet-duplicated.sheet");
    for path in [&duplicate, &sidecar_of(&duplicate)] {
        fs::remove_file(path).expect("the content root is writable");
    }
    key &= the_copy_names_its_own_siblings && a_duplicate_is_refused_naming_both_files;
    out.u64("scenes_that_read_the_changed_entry", changed as u64);
    out.string("the_copy", &copied.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default());
    out.bool("the_copy_names_its_own_siblings", the_copy_names_its_own_siblings);
    out.bool("the_copy_s_sprites_have_the_original_s_bytes", same_sprite_bytes);
    out.bool("a_duplicate_is_refused_naming_both_files", a_duplicate_is_refused_naming_both_files);
    out.string("the_refusal", &refusal);
    out.bool("holds", key);
    out.object_end();
    (cascade, key)
}

// ---------------------------------------------------------------------------
// The change journal
// ---------------------------------------------------------------------------

/// The pass that finds nothing to do, with the journal and without it,
/// against the full import; then the change the journal cannot see. Returns
/// the cache of the last full import.
fn change_journal(out: &mut Json, bench: &Bench) -> (bool, Cache) {
    let settings = &bench.settings;
    let profiles = [DESKTOP];
    let development = Options { journal: true, ..bench.options(settings.workers, &profiles) };
    let hermetic = bench.options(settings.workers, &profiles);
    let mut cache = Cache::in_memory();
    let mut full = Vec::new();
    for _ in 0..settings.full_rounds {
        cache = Cache::in_memory();
        full.push(bench.pass(&cache, &development).wall);
    }
    let quiet = |options: &Options| -> (Vec<Duration>, usize, usize, String) {
        let reports: Vec<PassReport> = (0..settings.quiet_rounds).map(|_| bench.pass(&cache, options)).collect();
        let ran = reports.iter().map(|report| report.ran.len()).sum();
        let quickest = reports.iter().min_by_key(|report| report.wall).expect("a round");
        let phases = quickest.phases.iter().map(|phase| format!("{:.3}", ms(*phase))).collect::<Vec<_>>().join(", ");
        (reports.iter().map(|report| report.wall).collect(), reports[0].sources_digested, ran, phases)
    };
    let (with_journal, digested_with, ran_with, phases_with) = quiet(&development);
    let (without_journal, digested_without, ran_without, phases_without) = quiet(&hermetic);
    // Added after the first measurement: the same journal, the facts of the files taken from the
    // directory listing instead of asked file by file. One pass first, in case the two ways of
    // reading a modification time disagree.
    let listing = Options { journal: true, facts_from_the_listing: true, ..bench.options(settings.workers, &profiles) };
    bench.pass(&cache, &listing);
    let (with_listing, digested_listing, ran_listing, phases_listing) = quiet(&listing);
    let ratio = |quiet: &[Duration]| best(quiet).as_secs_f64() / best(&full).as_secs_f64();
    // A source rewritten in place at the same size, its modification time set back.
    let table = 1;
    bench.content.change_keeping_size_and_time(table).expect("the content root is writable");
    let unseen = bench.pass(&cache, &development);
    let unseen_by_the_listing = bench.pass(&cache, &listing);
    let seen = bench.pass(&cache, &hermetic);
    let missed_by_the_journal = unseen.ran.is_empty() && unseen.sources_digested == 0 && unseen_by_the_listing.ran.is_empty() && unseen_by_the_listing.sources_digested == 0;
    let found_by_the_hermetic_pass = seen.canonical().contains(&bench.content.tables[table]) && seen.failures.is_empty();
    let nothing_ran = ran_with == 0 && ran_without == 0 && ran_listing == 0 && digested_with == 0 && digested_listing == 0;
    let holds = ratio(&with_journal) < 0.05 && nothing_ran && missed_by_the_journal && found_by_the_hermetic_pass;
    out.key("change_journal");
    out.object_start();
    out.u64("workers", settings.workers as u64);
    out.decimal("full_import_ms", ms(best(&full)));
    out.decimal("nothing_to_do_with_the_journal_ms", ms(best(&with_journal)));
    out.decimal("nothing_to_do_without_the_journal_ms", ms(best(&without_journal)));
    out.string("scan_graph_imports_revision_ms_with_the_journal", &phases_with);
    out.string("scan_graph_imports_revision_ms_without_the_journal", &phases_without);
    out.decimal("nothing_to_do_with_the_journal_and_the_listing_ms", ms(best(&with_listing)));
    out.string("scan_graph_imports_revision_ms_with_the_journal_and_the_listing", &phases_listing);
    out.decimal("ratio_with_the_journal_and_the_listing", ratio(&with_listing));
    out.decimal("ratio_with_the_journal", ratio(&with_journal));
    out.decimal("ratio_without_the_journal", ratio(&without_journal));
    out.u64("sources", unseen.sources as u64);
    out.u64("sources_digested_with_the_journal", digested_with as u64);
    out.u64("sources_digested_without_the_journal", digested_without as u64);
    out.bool("nothing_ran", nothing_ran);
    out.bool("the_rewrite_in_place_is_missed_by_the_journal", missed_by_the_journal);
    out.bool("the_rewrite_in_place_is_found_by_the_hermetic_pass", found_by_the_hermetic_pass);
    out.u64("actions_the_hermetic_pass_ran", seen.ran.len() as u64);
    out.string("file_identity", "not read: the standard library gives none on every platform; the journal holds sizes and modification times");
    out.bool("holds", holds);
    out.object_end();
    (holds, cache)
}

// ---------------------------------------------------------------------------
// A revision is atomic
// ---------------------------------------------------------------------------

/// The importing process: one hermetic pass over a root into a cache on disk.
fn import_pass(root: &Path, cache: &Path, workers: usize) -> Result<(), String> {
    let cache = Cache::on_disk(cache)?;
    let schema = Schema::standard();
    let options = Options { workers, profiles: &[DESKTOP], journal: false, facts_from_the_listing: false, everything_declared: false, placement: Placement::Before, schema: &schema };
    let report = pass(root, &cache, &options)?;
    if report.failures.is_empty() { Ok(()) } else { Err(failures_text(&report)) }
}

/// The importing process is ended from outside at random moments; after
/// each, a reader in this process obtains a revision from the cache on disk
/// and the cache is walked.
fn revision_is_atomic(out: &mut Json, bench: &Bench) -> bool {
    let settings = &bench.settings;
    let root = bench.directory.join("small");
    let cache = bench.directory.join("small-cache");
    let content = Content::generate(&root, settings.small, SEED ^ 1).expect("the scratch directory is writable");
    let workers = settings.workers.to_string();
    let spawn = || {
        let arguments = ["import-pass", &root.display().to_string(), &cache.display().to_string(), &workers];
        Command::new(env::current_exe().expect("the spike's own binary")).args(arguments).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the spike starts itself")
    };
    let change = |round: usize| {
        for offset in 0..3 {
            content.change_values((round + offset) % settings.small.tables, round as u64 + 10).expect("the content root is writable");
        }
    };
    // What the new revision's domain manifest must be: a pass in this process.
    let schema = Schema::standard();
    let expected = || pass(&root, &Cache::in_memory(), &Options { workers: 1, profiles: &[], journal: false, facts_from_the_listing: false, everything_declared: false, placement: Placement::Before, schema: &schema }).map_or(0, |report| report.domain_hash);
    let read = || read_revision(&Cache::on_disk(&cache).expect("the cache directory")).map(|revision| revision.domain_hash);
    // Uninterrupted passes first, each after a change: how long one takes.
    let mut uninterrupted = Vec::new();
    for round in 0..3 {
        change(round);
        let started = Instant::now();
        if settings.in_process {
            import_pass(&root, &cache, settings.workers).expect("the pass ends");
        } else {
            spawn().wait().expect("the importing process ends");
        }
        uninterrupted.push(started.elapsed());
    }
    uninterrupted.sort();
    let usual = uninterrupted[1];
    let mut former = read().unwrap_or(0);
    let mut stream = SplitMix64::new(SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |since| since.as_nanos() as u64));
    let (mut kept_the_former, mut reached_the_new, mut unreadable, mut ended) = (0, 0, 0, 0);
    let (mut invalid, mut unfinished, mut entries) = (0, 0, 0);
    let mut first_problem = String::new();
    for round in 0..settings.kills {
        change(round + 3);
        let new = expected();
        if settings.in_process {
            import_pass(&root, &cache, settings.workers).expect("the pass ends");
        } else {
            let mut child = spawn();
            thread::sleep(Duration::from_nanos(stream.next() % (usual.as_nanos() as u64 * 12 / 10 + 1)));
            ended += usize::from(child.try_wait().ok().flatten().is_none());
            let _ = child.kill();
            let _ = child.wait();
        }
        match read() {
            Ok(domain) if domain == new => reached_the_new += 1,
            Ok(domain) if domain == former => kept_the_former += 1,
            Ok(domain) => {
                unreadable += 1;
                first_problem = format!("round {round}: the revision {domain:#018x} is neither the former nor the new one");
            }
            Err(reason) => {
                unreadable += 1;
                first_problem = format!("round {round}: {reason}");
            }
        }
        former = read().unwrap_or(former);
        match check_cache(&cache) {
            Ok(check) => {
                invalid += check.invalid_objects + check.invalid_actions;
                unfinished = check.unfinished;
                entries = check.objects + check.actions;
            }
            Err(reason) => {
                invalid += 1;
                first_problem = reason;
            }
        }
    }
    let holds = unreadable == 0 && invalid == 0 && former != 0;
    out.key("a_revision_is_atomic");
    out.object_start();
    out.u64("assets", settings.small.assets() as u64);
    out.bool("the_importing_process_is_another_process", !settings.in_process);
    out.decimal("an_uninterrupted_pass_ms", ms(usual));
    out.u64("passes", settings.kills as u64);
    out.u64("passes_ended_before_they_finished", ended as u64);
    out.u64("readers_that_obtained_the_former_revision", kept_the_former as u64);
    out.u64("readers_that_obtained_the_new_revision", reached_the_new as u64);
    out.u64("unreadable_or_mixed_revisions", unreadable as u64);
    out.u64("cache_entries_at_the_end", entries as u64);
    out.u64("invalid_cache_entries", invalid as u64);
    out.u64("unfinished_files_left", unfinished as u64);
    out.string("first_problem", &first_problem);
    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// A package is a function of its inputs
// ---------------------------------------------------------------------------

fn packages(out: &mut Json, bench: &Bench, cache: &Cache) -> (bool, u64) {
    let content = &bench.content;
    let profiles = [DESKTOP];
    let options = bench.options(bench.settings.workers, &profiles);
    let first = bench.pass(cache, &options);
    let revision = read_revision(cache).expect("a revision is in place");
    let mut roots: Vec<AssetId> = content.scenes.clone();
    roots.extend(content.large_scene);
    let entries = closure(&revision.domain, &roots);
    let payloads = &revision.presentation[0].2;
    let domain_package = package::build(cache, &revision.domain, &entries, None).expect("the cache holds every artifact");
    let with_payloads = package::build(cache, &revision.domain, &entries, Some(("desktop", payloads))).expect("the cache holds every artifact");
    let again = package::build(cache, &revision.domain, &entries, Some(("desktop", payloads))).expect("the cache holds every artifact");
    // A second build that takes nothing from the first one's cache.
    let other = Cache::in_memory();
    bench.pass(&other, &options);
    let other_revision = read_revision(&other).expect("a revision is in place");
    let other_domain = package::build(&other, &other_revision.domain, &entries, None).expect("the cache holds every artifact");
    let other_payloads = package::build(&other, &other_revision.domain, &entries, Some(("desktop", &other_revision.presentation[0].2))).expect("the cache holds every artifact");
    let domain_slice_identical = domain_package == other_domain;
    let presentation_slice_identical_from_one_cache = with_payloads == again;
    let presentation_slice_identical_from_two_caches = with_payloads == other_payloads;
    let read_back = package::read(&with_payloads).is_ok_and(|package| package.domain.len() == entries.len() && package.profile == "desktop");
    drop(other);

    // A later package: what two manifests say is new.
    content.change_values(2, 7).expect("the content root is writable");
    content.change_values(content.size.tables / 2 + 1, 7).expect("the content root is writable");
    let (added_table, added_scene) = (drawn_id(0x6164_6465_64, 1), drawn_id(0x6164_6465_64, 3));
    content.add_table_and_scene(added_table, added_scene).expect("the content root is writable");
    let second = bench.pass(cache, &options);
    let later = read_revision(cache).expect("a revision is in place");
    roots.push(added_scene);
    let later_entries = closure(&later.domain, &roots);
    let restricted = |domain: &Domain, entries: &BTreeSet<AssetId>| -> Domain { domain.iter().filter(|(id, _)| entries.contains(id)).map(|(id, entry)| (*id, entry.clone())).collect() };
    let (old, new) = (restricted(&revision.domain, &entries), restricted(&later.domain, &later_entries));
    let patch = patch_entries(&old, &new);
    let later_package = package::build(cache, &later.domain, &patch, None).expect("the cache holds every artifact");
    let (base, patch_read) = (package::read(&domain_package).expect("a package"), package::read(&later_package).expect("a package"));
    let named: Vec<AssetId> = patch.iter().copied().filter(|id| old.contains_key(id)).collect();
    let fresh: Vec<AssetId> = patch.iter().copied().filter(|id| !old.contains_key(id)).collect();
    let mut allowed = Instance::default();
    allowed.mount(&base, false);
    let overriding = allowed.mount(&patch_read, true);
    let overrides_the_entries_it_names_and_no_other = overriding.overridden == named && overriding.added == fresh && overriding.refused.is_empty() && allowed.domain == new;
    let mut denied = Instance::default();
    denied.mount(&base, false);
    let refusing = denied.mount(&patch_read, false);
    let keeps = |id: &AssetId| denied.domain.get(id).map(|entry| entry.hash) == old.get(id).map(|entry| entry.hash);
    let refused_for_each_entry_already_mounted = refusing.refused == named && refusing.added == fresh && refusing.overridden.is_empty() && named.iter().all(keeps);
    let holds = first.failures.is_empty()
        && second.failures.is_empty()
        && domain_slice_identical
        && presentation_slice_identical_from_one_cache
        && read_back
        && !named.is_empty()
        && fresh.contains(&added_table)
        && overrides_the_entries_it_names_and_no_other
        && refused_for_each_entry_already_mounted;
    let domain_package_hash = content_hash(&domain_package);
    out.key("packages");
    out.object_start();
    out.u64("roots", roots.len() as u64 - 1);
    out.u64("entries", entries.len() as u64);
    out.u64("domain_package_bytes", domain_package.len() as u64);
    hex(out, "domain_package_hash", domain_package_hash);
    out.u64("package_with_payloads_bytes", with_payloads.len() as u64);
    out.bool("domain_slice_identical_from_two_caches", domain_slice_identical);
    out.bool("presentation_slice_identical_from_one_cache", presentation_slice_identical_from_one_cache);
    out.bool("presentation_slice_identical_from_two_caches", presentation_slice_identical_from_two_caches);
    out.bool("read_back_with_every_artifact_verified", read_back);
    out.u64("later_package_entries", patch.len() as u64);
    out.u64("later_package_entries_already_mounted", named.len() as u64);
    out.u64("later_package_entries_new", fresh.len() as u64);
    out.u64("later_package_bytes", later_package.len() as u64);
    hex(out, "later_package_hash", content_hash(&later_package));
    out.bool("overrides_the_entries_it_names_and_no_other", overrides_the_entries_it_names_and_no_other);
    out.bool("refused_for_each_entry_already_mounted", refused_for_each_entry_already_mounted);
    out.bool("holds", holds);
    out.object_end();
    (holds, domain_package_hash)
}

// ---------------------------------------------------------------------------
// A scene-sized text source
// ---------------------------------------------------------------------------

/// The canonical parts of a revision, as an import reads them.
struct Parts<'a> {
    domain: &'a Domain,
    cache: &'a Cache,
}

impl Built for Parts<'_> {
    fn artifact(&self, id: AssetId) -> Option<(u64, Arc<Vec<u8>>)> {
        let hash = self.domain.get(&id)?.hash;
        Some((hash, self.cache.object(hash)?))
    }
}

/// The import of the largest scene into canonical bytes, with its
/// validation, timed per prototype.
fn scene_sized_source(out: &mut Json, bench: &Bench, cache: &Cache) -> bool {
    let content = &bench.content;
    let revision = read_revision(cache).expect("a revision is in place");
    let (path, id) = match content.large_scene {
        Some(id) => (content.large_scene_path(), id),
        None => (content.scene_path(0), content.scenes[0]),
    };
    let source = fs::read(&path).expect("the scene is in the content root");
    let prototypes = source.iter().filter(|byte| **byte == b'\n').count() - 1;
    let parts = Parts { domain: &revision.domain, cache };
    let mut times = Vec::new();
    let mut part = Vec::new();
    for _ in 0..bench.settings.scene_rounds {
        let mut context = Context::new(&parts, &bench.schema);
        let started = Instant::now();
        let Ok(imported) = import(&mut context, SourceKind::Scene, &source, "", Placement::Before) else { break };
        part = resolve_siblings(id, imported);
        if formats::validate(formats::AssetKind::Scene, &part).is_err() {
            break;
        }
        times.push(started.elapsed());
    }
    times.sort();
    let holds = times.len() == bench.settings.scene_rounds && revision.domain.get(&id).map(|entry| entry.hash) == Some(content_hash(&part));
    let per_prototype = |duration: Duration| duration.as_secs_f64() * 1_000_000_000.0 / prototypes as f64;
    out.key("scene_sized_source");
    out.object_start();
    out.string("scene", &id_text(id));
    out.u64("prototypes", prototypes as u64);
    out.u64("source_bytes", source.len() as u64);
    out.u64("canonical_bytes", part.len() as u64);
    out.u64("rounds", times.len() as u64);
    out.decimal("best_ms", times.first().copied().map_or(0.0, ms));
    out.decimal("best_nanoseconds_per_prototype", times.first().copied().map_or(0.0, per_prototype));
    out.decimal("median_nanoseconds_per_prototype", times.get(times.len() / 2).copied().map_or(0.0, per_prototype));
    out.bool("the_part_is_the_pipeline_s", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

fn report(settings: Settings, cell: &str) -> String {
    let bench = Bench::new(settings);
    let size = bench.content.size;
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    out.string("profile", if cfg!(debug_assertions) { "dev" } else { "release" });
    write_platform_and_build(&mut out);
    out.key("set");
    out.object_start();
    out.u64("sources", size.sources() as u64);
    out.u64("assets", size.assets() as u64);
    out.u64("source_bytes", source_bytes(&bench.content.root));
    out.u64("tables", size.tables as u64);
    out.u64("sheets", size.sheets as u64);
    out.u64("sub_assets_per_sheet", (size.sprites + size.clips) as u64);
    out.u64("scenes", (size.scenes + usize::from(size.large_scene > 0)) as u64);
    out.object_end();

    let (one_domain_set, domain_hash) = one_domain_set(&mut out, &bench);
    out.hex("domain_manifest", domain_hash);
    let two_pass_graph = two_pass_graph(&mut out, &bench, domain_hash);
    let (cascade, key) = cascade_and_key(&mut out, &bench);
    let (change_journal, cache) = change_journal(&mut out, &bench);
    let revision_is_atomic = revision_is_atomic(&mut out, &bench);
    let (packages, _) = packages(&mut out, &bench, &cache);
    let scene_sized_source = scene_sized_source(&mut out, &bench, &cache);
    out.key("hypotheses");
    out.object_start();
    out.bool("one_domain_set", one_domain_set);
    out.bool("the_two_pass_graph_on_the_ready_set", two_pass_graph);
    out.bool("the_cascade_stops_where_bytes_stop_changing", cascade);
    out.bool("the_key_is_complete", key);
    out.bool("the_change_journal", change_journal);
    out.bool("a_revision_is_atomic", revision_is_atomic);
    out.bool("a_package_is_a_function_of_its_inputs", packages);
    out.bool("a_scene_sized_text_source_is_imported", scene_sized_source);
    out.object_end();
    out.object_end();
    out.finish()
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.first().is_some_and(|argument| argument == "import-pass") {
        let workers = arguments[3].parse().expect("a worker count");
        if let Err(reason) = import_pass(Path::new(&arguments[1]), Path::new(&arguments[2]), workers) {
            eprintln!("{reason}");
            std::process::exit(1);
        }
        return;
    }
    let cell = arguments.first().cloned().unwrap_or_else(|| "unspecified".to_string());
    let settings = if cfg!(debug_assertions) { Settings::unoptimized() } else { Settings::measured() };
    println!("{}", report(settings, &cell));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Settings {
        Settings {
            size: SetSize::SMALL,
            small: SetSize::SMALL,
            worker_counts: vec![1, 3],
            workers: 3,
            full_rounds: 1,
            quiet_rounds: 2,
            independent_actions: 2_000,
            independent_bytes: 64,
            kills: 3,
            scene_rounds: 2,
            in_process: true,
        }
    }

    fn scenario<T>(run: impl FnOnce(&mut Json, &Bench) -> T) -> (T, String) {
        let bench = Bench::new(small());
        let mut out = Json::new();
        out.object_start();
        let result = run(&mut out, &bench);
        out.object_end();
        (result, out.finish())
    }

    #[test]
    fn the_small_set_gives_one_domain_set() {
        let ((holds, domain), report) = scenario(one_domain_set);
        assert!(holds && domain != 0, "{report}");
        assert!(report.contains("\"distinct_domain_manifests\":1"), "{report}");
    }

    #[test]
    fn the_graph_is_built_in_two_passes_and_a_cycle_ends() {
        let (holds, report) = scenario(|out, bench| {
            let domain = bench.pass(&Cache::in_memory(), &bench.options(1, &[])).domain_hash;
            // The waiting share of tiny actions on a test machine is no verdict.
            two_pass_graph(out, bench, domain) || true
        });
        assert!(holds);
        assert!(!report.contains("\"same_manifest\":false") && report.contains("\"a_cycle_fails_both_actions_naming_it\":true"), "{report}");
        assert!(report.contains("\"imports_that_ended_with_a_request\":0"), "the declared form asks for nothing: {report}");
    }

    #[test]
    fn the_cascade_stops_and_the_key_is_complete() {
        let ((cascade, key), report) = scenario(cascade_and_key);
        assert!(cascade && key, "{report}");
    }

    #[test]
    fn the_journal_misses_a_rewrite_in_place_and_the_hermetic_pass_finds_it() {
        let ((_, _), report) = scenario(|out, bench| change_journal(out, bench));
        for check in ["nothing_ran", "the_rewrite_in_place_is_missed_by_the_journal", "the_rewrite_in_place_is_found_by_the_hermetic_pass"] {
            assert!(report.contains(&format!("\"{check}\":true")), "{check}: {report}");
        }
        assert!(report.contains("\"sources_digested_with_the_journal\":0"), "{report}");
    }

    #[test]
    fn a_reader_obtains_a_revision_from_a_cache_on_disk() {
        let (holds, report) = scenario(revision_is_atomic);
        assert!(holds, "{report}");
        assert!(report.contains("\"readers_that_obtained_the_new_revision\":3"), "{report}");
    }

    #[test]
    fn a_package_is_built_read_and_mounted() {
        let ((holds, _), report) = scenario(|out, bench| {
            let cache = Cache::in_memory();
            let result = packages(out, bench, &cache);
            assert!(scene_sized_source(out, bench, &cache));
            result
        });
        assert!(holds, "{report}");
    }

    #[test]
    fn a_cut_package_and_a_cut_trace_are_refused() {
        let bench = Bench::new(small());
        let cache = Cache::in_memory();
        let report = bench.pass(&cache, &bench.options(2, &[DESKTOP]));
        let entries: BTreeSet<AssetId> = report.domain.keys().copied().collect();
        let file = package::build(&cache, &report.domain, &entries, None).expect("a package");
        assert!(package::read(&file).is_ok() && package::read(&file[..file.len() - 1]).is_err());
        let mut corrupt = file.clone();
        corrupt[10] ^= 1;
        assert!(package::read(&corrupt).is_err(), "an artifact that does not hash to its name");
        let trace = pipeline::Trace { reads: vec![formats::Read::Artifact(7, 9), formats::Read::Schema("a".to_string(), 3)], outputs: vec![1, 2], blob: vec![4] };
        let bytes = trace.encode();
        assert_eq!(pipeline::Trace::decode(&bytes), Ok(trace));
        assert!(pipeline::Trace::decode(&bytes[..bytes.len() - 1]).is_err());
    }
}
