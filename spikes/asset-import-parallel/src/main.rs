//! Host of the parallel import spike, for the assets research and the
//! background pool of the concurrency model. Throwaway, never product code.
//!
//! A few hundred synthetic source assets of a plain kind (record tables,
//! integer arithmetic), a geometric kind (meshes, exact floating-point
//! operations only) and a control kind (meshes normalized with the platform
//! math library) are imported in parallel on a background pool with dynamic
//! claiming into canonical content-hashed bytes. The spike measures, instead
//! of assuming:
//!
//! - whether the outputs are byte-identical across runs and worker counts
//!   (the manifest hash per kind), and reports the per-kind hashes for the
//!   comparison across cells;
//! - whether a re-import after one source changes imports that asset and its
//!   dependents only, by the action keys and the cache;
//! - how the full import's throughput scales with the workers, and what the
//!   dependency structure costs the pool in idle time;
//! - that the order in which the pool completes the actions differs between
//!   runs while the manifest does not.

mod import;
mod pool;
mod sources;

use std::env;
use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use reference_workload::{Fnv1a, Json, fnv1a, write_platform_and_build};

use import::{ActionInputs, Cache, Settings, TARGET_PLATFORM, action_key, content_hash, import, importer_for};
use pool::{BatchReport, Pool};
use sources::{Kind, SetSize, SourceSet};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "asset-import-parallel";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
/// The seed of the measured set.
const SEED: u64 = 0x4c75_6465_6e65_6121;
/// The runs per worker count of the identity scenario.
const IDENTITY_RUNS: usize = 3;
/// The alternating rounds of the throughput scenario.
const THROUGHPUT_ROUNDS: usize = 3;
/// The worker count of the claiming order scenario and of the incremental one.
const CLAIMING_WORKERS: usize = 8;
/// The attempts of the claiming order scenario before giving up on seeing two orders.
const CLAIMING_ATTEMPTS: usize = 5;

/// The base counts, 16 when the machine has at least sixteen hardware
/// threads (the local threshold is stated at 16 workers), and the machine's
/// available parallelism when it is not among them.
fn worker_counts() -> Vec<usize> {
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = BASE_WORKER_COUNTS.to_vec();
    if available >= 16 {
        counts.push(16);
    }
    if !counts.contains(&available) {
        counts.push(available);
    }
    counts.sort_unstable();
    counts.dedup();
    counts
}

/// What every action of a set shares: the importer version and the settings.
#[derive(Clone, Copy, Debug)]
struct ImportConfig {
    importer_version: u32,
    settings: Settings,
}

impl ImportConfig {
    const DEFAULT: ImportConfig = ImportConfig { importer_version: 1, settings: Settings::DEFAULT };
}

/// The manifest hashes: the sorted list of asset identifier and content
/// hash of a kind, hashed as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Manifests {
    plain: u64,
    geometric: u64,
    control: u64,
    all: u64,
}

impl Manifests {
    fn of(set: &SourceSet, hashes: &[u64]) -> Manifests {
        let manifest = |kind: Option<Kind>| {
            let mut entries: Vec<(&str, u64)> = set
                .assets()
                .iter()
                .zip(hashes)
                .filter(|(asset, _)| kind.is_none_or(|kind| asset.kind == kind))
                .map(|(asset, &hash)| (asset.id.as_str(), hash))
                .collect();
            entries.sort_unstable();
            let mut hasher = Fnv1a::new();
            for (id, hash) in entries {
                hasher.write(id.as_bytes());
                hasher.write(&[0]);
                hasher.write(&hash.to_le_bytes());
            }
            hasher.finish()
        };
        Manifests { plain: manifest(Some(Kind::Plain)), geometric: manifest(Some(Kind::Geometric)), control: manifest(Some(Kind::Control)), all: manifest(None) }
    }

    fn of_kind(&self, kind: Kind) -> u64 {
        match kind {
            Kind::Plain => self.plain,
            Kind::Geometric => self.geometric,
            Kind::Control => self.control,
        }
    }
}

/// One import of a set on the pool.
struct SetImport {
    /// The content hash of every asset's output, in asset order.
    hashes: Vec<u64>,
    output_bytes: u64,
    manifests: Manifests,
    report: BatchReport,
}

/// Runs one action: reads the source, computes the key from the source
/// digest, the importer, the settings, the target and the dependencies'
/// content hashes, serves the output from the cache or imports and stores it.
fn run_action(set: &SourceSet, cache: &Cache, config: &ImportConfig, index: usize, dependencies: &[Arc<Vec<u8>>]) -> Result<(Arc<Vec<u8>>, bool), String> {
    let asset = &set.assets()[index];
    let source = fs::read(&asset.path).map_err(|error| format!("{}: reading {}: {error}", asset.id, asset.path.display()))?;
    let dependency_hashes: Vec<u64> = dependencies.iter().map(|output| content_hash(output)).collect();
    let key = action_key(&ActionInputs {
        source_digest: fnv1a(&source),
        importer: importer_for(asset.kind, config.importer_version),
        settings: &config.settings,
        target_platform: TARGET_PLATFORM,
        dependency_hashes: &dependency_hashes,
    });
    if let Some(output) = cache.get(key)? {
        return Ok((output, false));
    }
    let dependency_slices: Vec<&[u8]> = dependencies.iter().map(|output| output.as_slice()).collect();
    let output = Arc::new(import(asset.kind, config.importer_version, &config.settings, &source, &dependency_slices).map_err(|error| format!("{}: {error}", asset.id))?);
    cache.put(key, Arc::clone(&output))?;
    Ok((output, true))
}

/// Imports the whole set on the pool with the cache; fails if any action failed.
fn import_set(set: &Arc<SourceSet>, cache: &Arc<Cache>, pool: &Pool, config: &ImportConfig) -> Result<SetImport, String> {
    let dependencies: Vec<Vec<usize>> = set.assets().iter().map(|asset| asset.dependencies.clone()).collect();
    let (set_for_actions, cache_for_actions, config) = (Arc::clone(set), Arc::clone(cache), *config);
    let report = pool.import(dependencies, move |index, dependency_outputs| run_action(&set_for_actions, &cache_for_actions, &config, index, dependency_outputs));
    let mut hashes = Vec::with_capacity(report.outputs.len());
    let mut output_bytes = 0;
    for (index, output) in report.outputs.iter().enumerate() {
        let output = output.as_ref().map_err(|error| format!("action {index} ({}) failed: {error}", set.assets()[index].id))?;
        hashes.push(content_hash(output));
        output_bytes += output.len() as u64;
    }
    let manifests = Manifests::of(set, &hashes);
    Ok(SetImport { hashes, output_bytes, manifests, report })
}

/// The sequential truth: every asset imported in asset order on the calling
/// thread (dependencies come first in that order), no cache.
#[cfg(test)]
fn sequential_import(set: &SourceSet, config: &ImportConfig) -> Result<Vec<u64>, String> {
    let mut outputs: Vec<Arc<Vec<u8>>> = Vec::with_capacity(set.assets().len());
    let cache = Cache::in_memory();
    for (index, asset) in set.assets().iter().enumerate() {
        let dependencies: Vec<Arc<Vec<u8>>> = asset
            .dependencies
            .iter()
            .map(|&dependency| outputs.get(dependency).cloned().ok_or_else(|| format!("{}: dependency {dependency} comes after it", asset.id)))
            .collect::<Result<_, _>>()?;
        let (output, _) = run_action(set, &cache, config, index, &dependencies)?;
        outputs.push(output);
    }
    Ok(outputs.iter().map(|output| content_hash(output)).collect())
}

/// Whether every asset outside `touched` kept its content hash.
fn untouched_unchanged(before: &[u64], after: &[u64], touched: &[usize]) -> bool {
    before.iter().zip(after).enumerate().all(|(index, (a, b))| touched.contains(&index) || a == b)
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Hypothesis: byte-identical across runs and worker counts. Returns the
/// manifests of the first run.
fn identical_across_runs_and_workers(out: &mut Json, set: &Arc<SourceSet>, worker_counts: &[usize], runs: usize) -> Result<(Manifests, u64), String> {
    let mut first = None;
    let mut seen: Vec<Vec<u64>> = vec![Vec::new(); 3];
    let mut failures = 0;
    out.key("identical_across_runs_and_workers");
    out.object_start();
    out.u64("runs_per_worker_count", runs as u64);
    out.key("per_worker_count");
    out.array_start();
    for &workers in worker_counts {
        let pool = Pool::new(workers);
        out.object_start();
        out.u64("workers", workers as u64);
        out.key("runs");
        out.array_start();
        for run in 0..runs {
            let imported = import_set(set, &Arc::new(Cache::in_memory()), &pool, &ImportConfig::DEFAULT)?;
            failures += imported.report.failures();
            out.object_start();
            out.u64("run", run as u64 + 1);
            for (slot, kind) in Kind::ALL.iter().enumerate() {
                out.hex(&format!("manifest_{kind}"), imported.manifests.of_kind(*kind));
                seen[slot].push(imported.manifests.of_kind(*kind));
            }
            out.hex("manifest_all", imported.manifests.all);
            out.u64("actions_imported", imported.report.imported_count() as u64);
            out.decimal("wall_ms", ms(imported.report.wall));
            out.object_end();
            if first.is_none() {
                first = Some((imported.manifests, imported.output_bytes));
            }
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
    let mut holds = failures == 0;
    for (slot, kind) in Kind::ALL.iter().enumerate() {
        seen[slot].sort_unstable();
        seen[slot].dedup();
        out.u64(&format!("distinct_{kind}"), seen[slot].len() as u64);
        holds &= seen[slot].len() == 1;
    }
    out.u64("failures", failures as u64);
    out.bool("holds", holds);
    out.object_end();
    first.ok_or_else(|| "no worker count to run".to_string())
}

/// One re-import of the incremental scenario, against the full import.
fn re_import(out: &mut Json, set: &Arc<SourceSet>, cache: &Arc<Cache>, pool: &Pool, config: &ImportConfig, change: &str, expected: &[usize], full: &SetImport) -> Result<bool, String> {
    let before = cache.counters();
    let imported = import_set(set, cache, pool, config)?;
    let after = cache.counters();
    let imported_actions: Vec<usize> = imported.report.imported.iter().enumerate().filter(|(_, flag)| **flag).map(|(index, _)| index).collect();
    let mut expected_sorted = expected.to_vec();
    expected_sorted.sort_unstable();
    let exact = imported_actions == expected_sorted;
    let untouched = untouched_unchanged(&full.hashes, &imported.hashes, expected);
    out.object_start();
    out.string("change", change);
    out.u64("expected_actions", expected.len() as u64);
    out.u64("actions_imported", imported.report.imported_count() as u64);
    out.bool("exactly_the_expected_actions", exact);
    out.u64("cache_memory_hits", after.memory_hits - before.memory_hits);
    out.u64("cache_disk_hits", after.disk_hits - before.disk_hits);
    out.bool("untouched_hashes_unchanged", untouched);
    out.decimal("elapsed_ms", ms(imported.report.wall));
    out.decimal("ratio_to_full_import", imported.report.wall.as_secs_f64() / full.report.wall.as_secs_f64().max(f64::EPSILON));
    out.u64("failures", imported.report.failures() as u64);
    let holds = exact && untouched && imported.report.failures() == 0;
    out.bool("holds", holds);
    out.object_end();
    Ok(holds)
}

/// Hypothesis: incremental re-import. The cache is mirrored on disk; a fresh
/// cache on the same mirror shows that a second process could hit it.
fn incremental(out: &mut Json, set: &Arc<SourceSet>, workers: usize) -> Result<(), String> {
    let pool = Pool::new(workers);
    let cache = Arc::new(Cache::mirrored(&set.cache_directory())?);
    let full = import_set(set, &cache, &pool, &ImportConfig::DEFAULT)?;
    let total: Vec<usize> = (0..set.assets().len()).collect();
    let (files, bytes) = cache.mirror_files()?;
    out.key("incremental");
    out.object_start();
    out.u64("workers", workers as u64);
    out.u64("full_import_actions", full.report.imported_count() as u64);
    out.decimal("full_import_ms", ms(full.report.wall));
    out.u64("disk_mirror_files", files as u64);
    out.u64("disk_mirror_bytes", bytes);
    out.key("re_imports");
    out.array_start();
    let mut holds = full.report.imported_count() == total.len() && full.report.failures() == 0;
    holds &= re_import(out, set, &cache, &pool, &ImportConfig::DEFAULT, "no change", &[], &full)?;
    let fresh = Arc::new(Cache::mirrored(&set.cache_directory())?);
    holds &= re_import(out, set, &fresh, &pool, &ImportConfig::DEFAULT, "no change, a fresh cache on the disk mirror", &[], &full)?;
    let geometric = set.index_of("geometric-000").ok_or("no geometric asset")?;
    let restore = set.alter_one_byte(geometric)?;
    holds &= re_import(out, set, &cache, &pool, &ImportConfig::DEFAULT, "one geometric source changed by one byte", &[geometric], &full)?;
    restore.restore()?;
    let plain = set.index_of("plain-000").ok_or("no plain asset")?;
    let mut touched = set.dependents_of(plain).to_vec();
    touched.push(plain);
    let restore = set.alter_one_byte(plain)?;
    let change = format!("one plain source shared by {} meshes changed by one byte", touched.len() - 1);
    holds &= re_import(out, set, &cache, &pool, &ImportConfig::DEFAULT, &change, &touched, &full)?;
    restore.restore()?;
    let bumped = ImportConfig { importer_version: ImportConfig::DEFAULT.importer_version + 1, ..ImportConfig::DEFAULT };
    holds &= re_import(out, set, &cache, &pool, &bumped, "the importer version changed", &total, &full)?;
    out.array_end();
    out.bool("holds", holds);
    out.object_end();
    Ok(())
}

/// Hypothesis: throughput scales with the workers. Best of alternating
/// rounds per worker count, after a warm-up import; the pools are created
/// before the timing, each timed import starts from an empty cache.
fn throughput(out: &mut Json, set: &Arc<SourceSet>, worker_counts: &[usize], rounds: usize) -> Result<(), String> {
    let pools: Vec<Pool> = worker_counts.iter().map(|&workers| Pool::new(workers)).collect();
    let warm_up = import_set(set, &Arc::new(Cache::in_memory()), pools.last().ok_or("no worker count")?, &ImportConfig::DEFAULT)?;
    let mut walls: Vec<Vec<Duration>> = vec![Vec::new(); worker_counts.len()];
    let mut idle_shares: Vec<Vec<f64>> = vec![Vec::new(); worker_counts.len()];
    let mut every_manifest_identical = true;
    for _ in 0..rounds {
        for (slot, pool) in pools.iter().enumerate() {
            let imported = import_set(set, &Arc::new(Cache::in_memory()), pool, &ImportConfig::DEFAULT)?;
            every_manifest_identical &= imported.manifests == warm_up.manifests;
            walls[slot].push(imported.report.wall);
            idle_shares[slot].push(imported.report.idle_share());
        }
    }
    let best: Vec<Duration> = walls.iter().map(|rounds| rounds.iter().copied().min().unwrap_or_default()).collect();
    let one_worker = worker_counts.iter().position(|&workers| workers == 1).map(|slot| best[slot]);
    out.key("throughput");
    out.object_start();
    out.u64("rounds", rounds as u64);
    out.decimal("warm_up_ms", ms(warm_up.report.wall));
    out.bool("every_manifest_identical", every_manifest_identical);
    out.key("per_worker_count");
    out.array_start();
    for (slot, &workers) in worker_counts.iter().enumerate() {
        out.object_start();
        out.u64("workers", workers as u64);
        out.decimal("best_ms", ms(best[slot]));
        out.decimal("assets_per_second", set.assets().len() as f64 / best[slot].as_secs_f64().max(f64::EPSILON));
        match one_worker {
            Some(one) => out.decimal("speedup", one.as_secs_f64() / best[slot].as_secs_f64().max(f64::EPSILON)),
            None => out.string("speedup", "n/a"),
        }
        let best_round = walls[slot].iter().position(|&wall| wall == best[slot]).unwrap_or(0);
        out.decimal("idle_share", idle_shares[slot][best_round]);
        out.key("rounds");
        out.array_start();
        for (round, wall) in walls[slot].iter().enumerate() {
            out.object_start();
            out.u64("round", round as u64 + 1);
            out.decimal("wall_ms", ms(*wall));
            out.decimal("idle_share", idle_shares[slot][round]);
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();
    out.object_end();
    Ok(())
}

/// Hypothesis: the claiming order is not the output order. Two runs at the
/// same worker count complete in different sequences while the manifest is
/// the same; a third (and more) if two happened to agree.
fn claiming_order(out: &mut Json, set: &Arc<SourceSet>, workers: usize, attempts: usize) -> Result<(), String> {
    let pool = Pool::new(workers);
    let mut orders = Vec::new();
    let mut manifests = Vec::new();
    out.key("claiming_order");
    out.object_start();
    out.u64("workers", workers as u64);
    out.key("runs");
    out.array_start();
    while orders.len() < attempts {
        let imported = import_set(set, &Arc::new(Cache::in_memory()), &pool, &ImportConfig::DEFAULT)?;
        let order = imported.report.completion_order_hash();
        out.object_start();
        out.u64("run", orders.len() as u64 + 1);
        out.hex("completion_order_hash", order);
        out.hex("manifest_all", imported.manifests.all);
        out.string("first_completions", &imported.report.completion_order.iter().take(6).map(|&index| set.assets()[index].id.as_str()).collect::<Vec<_>>().join(", "));
        out.object_end();
        let differs = orders.iter().any(|&seen| seen != order);
        orders.push(order);
        manifests.push(imported.manifests.all);
        if differs {
            break;
        }
    }
    out.array_end();
    let orders_differ = orders.windows(2).any(|pair| pair[0] != pair[1]) || orders.iter().any(|&order| order != orders[0]);
    let manifests_identical = manifests.iter().all(|&manifest| manifest == manifests[0]);
    out.u64("runs_needed", orders.len() as u64);
    out.bool("orders_differ", orders_differ);
    out.bool("manifests_identical", manifests_identical);
    out.bool("holds", orders_differ && manifests_identical);
    out.object_end();
    Ok(())
}

fn report(size: &SetSize, seed: u64, cell: &str, worker_counts: &[usize], rounds: usize) -> String {
    match try_report(size, seed, cell, worker_counts, rounds) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("the spike failed: {error}");
            std::process::exit(1);
        }
    }
}

fn try_report(size: &SetSize, seed: u64, cell: &str, worker_counts: &[usize], rounds: usize) -> Result<String, String> {
    let set = Arc::new(SourceSet::generate(size, seed)?);
    let (geometric_tables, control_tables) = size.transform_tables();
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.key("set");
    out.object_start();
    out.u64("seed", seed);
    for kind in Kind::ALL {
        out.u64(kind.name(), set.count(kind) as u64);
    }
    out.u64("total", set.assets().len() as u64);
    out.u64("records_per_table", size.records_per_table as u64);
    out.u64("vertices_per_mesh", size.vertices_per_mesh as u64);
    out.u64("triangles_per_mesh", size.triangles_per_mesh as u64);
    out.u64("transform_tables", (geometric_tables + control_tables) as u64);
    out.u64("dependents_per_transform", size.dependents_per_transform as u64);
    out.u64("source_bytes", set.source_bytes());
    out.object_end();
    out.key("worker_counts");
    out.array_start();
    for &workers in worker_counts {
        out.object_start();
        out.u64("workers", workers as u64);
        out.object_end();
    }
    out.array_end();

    let (manifests, output_bytes) = identical_across_runs_and_workers(&mut out, &set, worker_counts, IDENTITY_RUNS.min(rounds.max(2)))?;
    out.u64("output_bytes", output_bytes);
    out.hex("manifest_plain", manifests.plain);
    out.hex("manifest_geometric", manifests.geometric);
    out.hex("manifest_control", manifests.control);
    out.hex("manifest_all", manifests.all);
    incremental(&mut out, &set, CLAIMING_WORKERS)?;
    claiming_order(&mut out, &set, CLAIMING_WORKERS, CLAIMING_ATTEMPTS)?;
    // The timed scenario runs last.
    throughput(&mut out, &set, worker_counts, rounds)?;
    out.object_end();
    Ok(out.finish())
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    println!("{}", report(&SetSize::MEASURED, SEED, &cell, &worker_counts(), THROUGHPUT_ROUNDS));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn small_set(seed: u64) -> Arc<SourceSet> {
        Arc::new(SourceSet::generate(&SetSize::SMALL, seed).expect("the small set generates"))
    }

    #[test]
    fn the_manifest_hash_is_the_same_at_every_worker_count_and_equal_to_a_sequential_import() {
        let set = small_set(101);
        let sequential = sequential_import(&set, &ImportConfig::DEFAULT).unwrap();
        let expected = Manifests::of(&set, &sequential);
        assert_ne!(expected.plain, expected.geometric);
        assert_ne!(expected.geometric, expected.control, "the control differs from the geometric kind by its header");
        for workers in [1, 2, 3, 8] {
            let pool = Pool::new(workers);
            let imported = import_set(&set, &Arc::new(Cache::in_memory()), &pool, &ImportConfig::DEFAULT).unwrap();
            assert_eq!(imported.hashes, sequential, "{workers} workers");
            assert_eq!(imported.manifests, expected, "{workers} workers");
            assert_eq!(imported.report.imported_count(), set.assets().len());
            assert_eq!(imported.report.failures(), 0);
        }
    }

    #[test]
    fn the_incremental_counts_hold() {
        let set = small_set(102);
        let cache = Arc::new(Cache::mirrored(&set.cache_directory()).unwrap());
        let pool = Pool::new(4);
        let full = import_set(&set, &cache, &pool, &ImportConfig::DEFAULT).unwrap();
        assert_eq!(full.report.imported_count(), 45);
        let unchanged = import_set(&set, &cache, &pool, &ImportConfig::DEFAULT).unwrap();
        assert_eq!(unchanged.report.imported_count(), 0);
        assert_eq!(unchanged.hashes, full.hashes);
        // A second cache on the same mirror hits the disk for everything.
        let from_disk = import_set(&set, &Arc::new(Cache::mirrored(&set.cache_directory()).unwrap()), &pool, &ImportConfig::DEFAULT).unwrap();
        assert_eq!(from_disk.report.imported_count(), 0);
        assert_eq!(from_disk.hashes, full.hashes);

        let geometric = set.index_of("geometric-003").unwrap();
        let restore = set.alter_one_byte(geometric).unwrap();
        let one = import_set(&set, &cache, &pool, &ImportConfig::DEFAULT).unwrap();
        assert_eq!(one.report.imported_count(), 1);
        assert!(one.report.imported[geometric]);
        assert!(untouched_unchanged(&full.hashes, &one.hashes, &[geometric]));
        restore.restore().unwrap();

        let plain = set.index_of("plain-000").unwrap();
        let dependents = set.dependents_of(plain).to_vec();
        assert_eq!(dependents.len(), 8);
        let restore = set.alter_one_byte(plain).unwrap();
        let nine = import_set(&set, &cache, &pool, &ImportConfig::DEFAULT).unwrap();
        assert_eq!(nine.report.imported_count(), 9);
        assert!(nine.report.imported[plain] && dependents.iter().all(|&d| nine.report.imported[d]));
        assert_ne!(nine.hashes[plain], full.hashes[plain]);
        let mut touched = dependents.clone();
        touched.push(plain);
        assert!(untouched_unchanged(&full.hashes, &nine.hashes, &touched));
        restore.restore().unwrap();

        let bumped = ImportConfig { importer_version: ImportConfig::DEFAULT.importer_version + 1, ..ImportConfig::DEFAULT };
        let all = import_set(&set, &cache, &pool, &bumped).unwrap();
        assert_eq!(all.report.imported_count(), 45);
        assert!(all.hashes.iter().zip(&full.hashes).all(|(a, b)| a != b), "the importer version is in every header");
        let back = import_set(&set, &cache, &pool, &ImportConfig::DEFAULT).unwrap();
        assert_eq!(back.report.imported_count(), 0, "the restored sources are still cached");
        assert_eq!(back.hashes, full.hashes);
    }

    #[test]
    fn the_report_runs_on_the_small_set_with_every_check_passing() {
        let report = report(&SetSize::SMALL, 103, "self-check", &[1, 3], 2);
        assert!(report.starts_with('{') && report.ends_with('}'));
        for key in [
            "\"schema_version\"",
            "\"manifest_plain\"",
            "\"manifest_geometric\"",
            "\"manifest_control\"",
            "\"identical_across_runs_and_workers\"",
            "\"incremental\"",
            "\"throughput\"",
            "\"claiming_order\"",
        ] {
            assert!(report.contains(key), "missing {key}");
        }
        assert!(!report.contains("\"holds\":false"), "a check fails: {report}");
        assert!(!report.contains("\"failures\":1"), "an import failed: {report}");
        assert!(report.contains("\"distinct_plain\":1") && report.contains("\"distinct_geometric\":1") && report.contains("\"distinct_control\":1"));
        assert!(report.contains("\"expected_actions\":9") && report.contains("\"expected_actions\":45") && report.contains("\"expected_actions\":0"));
    }
}
