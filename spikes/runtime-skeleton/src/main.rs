//! Host executable of the runtime skeleton spike, for the runtime architecture
//! research. Throwaway, never product code.
//!
//! The spike measures a minimal crate set in three layers (a core stepping the
//! reference workload under time control, a presentation capability behind the
//! facade's feature flag, this host) rather than assuming it:
//!
//! - the time control API, in this build's configuration and in the other one:
//!   a headless host stepping 1 000 ticks back to back; a harness host stepping
//!   them in batches of 1, 7 and 100 with a pause after every batch, the
//!   batches of 100 alternating between two threads; a snapshot taken at tick
//!   500 and restored into a new world stepped to 1 000; two worlds with
//!   different seeds stepped alternately; a panic injected in one of two worlds
//!   at tick 300. Every world's tick hashes are compared with the sequential
//!   reference of its seed;
//! - the build-time level of the capability model: the dependency graph of the
//!   host with and without the flag (from `cargo tree`), the presence of the
//!   presentation crate's library among the built artifacts, and the clean
//!   release, clean dev and incremental build times of both configurations,
//!   the incremental ones after a one-line change in the core;
//! - the headless fact: the binary links no windowing, graphics or audio
//!   library, and runs wherever the research CI runs it.
//!
//! Started with `--time-control <workers>...`, the binary runs the time
//! control scenarios only and prints their report; the spike uses this to
//! measure the configuration it was not built in.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use reference_workload::{
    DEFAULT_SEED, DEFAULT_TICK_COUNT, Json, Parameters, Run, Size, first_divergent_tick, run_hash,
    run_sequential, write_platform_and_build,
};
use skeleton_facade::{Capabilities, Injection, Node, PRESENTATION, World, WorldConfig};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "runtime-skeleton";
const PACKAGE: &str = "runtime-skeleton";
const PRESENTATION_CRATE: &str = "skeleton-presentation";
const BASE_WORKER_COUNTS: [usize; 2] = [1, 4];
/// The seed of the second world of the isolation and failure scenarios.
const SECOND_SEED: u64 = 0x4c75_6465_6e65_6132;
/// The harness host's batches: the batch size and whether a pause follows
/// every batch (a pause of 1 ms, as a stepping tool would leave between steps).
const BATCHES: [(u32, bool); 4] = [(1, true), (1, false), (7, true), (100, true)];
const ALTERNATING_THREADS_BATCH: u32 = 100;
const PAUSE: Duration = Duration::from_millis(1);
const ALTERNATION_BATCH: u32 = 10;
const CLEAN_SAMPLES: u32 = 3;
const INCREMENTAL_SAMPLES: u32 = 3;

fn configuration_name(presentation: bool) -> &'static str {
    if presentation { "with-presentation" } else { "without-presentation" }
}

fn worker_counts() -> Vec<usize> {
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = BASE_WORKER_COUNTS.to_vec();
    if !counts.contains(&available) {
        counts.push(available);
        counts.sort_unstable();
    }
    counts
}

/// The scenarios' parameters: the measured ones on the default size, smaller
/// ones for the self-checks.
#[derive(Clone, Copy)]
struct Scenario {
    size: Size,
    ticks: u32,
    snapshot_tick: u32,
    injection_tick: u32,
}

const MEASURED: Scenario = Scenario { size: Size::Default, ticks: DEFAULT_TICK_COUNT, snapshot_tick: 500, injection_tick: 300 };

impl Scenario {
    fn injection(&self) -> Injection {
        Injection { tick: self.injection_tick, node: Node::Repulsion, chunk: 2 }
    }

    fn reference(&self, seed: u64) -> Run {
        run_sequential(&Parameters { seed, ..Parameters::new(self.size, self.ticks, false) })
    }
}

// ---------------------------------------------------------------------------
// Time control scenarios
// ---------------------------------------------------------------------------

/// What the host can read back from the presentation capability it
/// registered, without the report code naming the presentation type.
trait Extracted: Send + Sync {
    fn extracted_ticks(&self) -> u64;
}

#[cfg(feature = "presentation")]
impl Extracted for skeleton_facade::presentation::StubExtraction {
    fn extracted_ticks(&self) -> u64 {
        skeleton_facade::presentation::StubExtraction::extracted_ticks(self)
    }
}

/// The capabilities this build registers, and a handle on the presentation
/// stub when the build has one.
fn capabilities() -> (Capabilities, Option<Arc<dyn Extracted>>) {
    #[cfg(feature = "presentation")]
    {
        let stub = Arc::new(skeleton_facade::presentation::StubExtraction::new());
        (Capabilities { extraction: Some(stub.clone()) }, Some(stub))
    }
    #[cfg(not(feature = "presentation"))]
    {
        (Capabilities::default(), None)
    }
}

fn new_world(scenario: &Scenario, workers: usize, seed: u64, injection: Option<Injection>) -> (World, Option<Arc<dyn Extracted>>) {
    let mut config = WorldConfig::new(scenario.size, workers).seeded(seed);
    if let Some(injection) = injection {
        config = config.injecting(injection);
    }
    let (capabilities, stub) = capabilities();
    (World::new(config, capabilities), stub)
}

/// The first tick at which the world's history differs from the reference,
/// if any, over the ticks the world has completed (a failed world's history
/// stops at its last good tick).
fn divergence(world: &World, reference: &Run) -> Option<usize> {
    let from = world.history_start() as usize;
    let to = world.tick() as usize;
    let ours: Vec<u64> = (world.history_start()..=world.tick()).map(|t| world.tick_hash(t).expect("in the history")).collect();
    first_divergent_tick(&ours, &reference.tick_hashes[from..=to]).map(|t| t + from)
}

fn write_comparison(out: &mut Json, world: &World, reference: &Run) {
    let divergence = divergence(world, reference);
    out.u64("tick", u64::from(world.tick()));
    out.bool("matches_reference", divergence.is_none());
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    out.hex("final_hash", world.hash());
}

fn ticks_per_second(ticks: u32, duration: Duration) -> f64 {
    if duration.is_zero() { 0.0 } else { f64::from(ticks) / duration.as_secs_f64() }
}

fn headless(out: &mut Json, scenario: &Scenario, workers: usize, reference: &Run) {
    // A warm-up world of a few ticks, then the measured one.
    let (mut warm_up, _) = new_world(scenario, workers, DEFAULT_SEED, None);
    warm_up.step(10).expect("the warm-up steps");
    let (mut world, stub) = new_world(scenario, workers, DEFAULT_SEED, None);
    world.step(scenario.ticks).expect("the headless host steps");
    out.key("headless");
    out.object_start();
    out.u64("ticks", u64::from(scenario.ticks));
    write_comparison(out, &world, reference);
    let hashes: Vec<u64> = (0..=world.tick()).map(|t| world.tick_hash(t).expect("in the history")).collect();
    out.hex("run_hash", run_hash(&hashes));
    out.decimal("stepping_ms", world.stepping().as_secs_f64() * 1_000.0);
    out.decimal("ticks_per_second", ticks_per_second(scenario.ticks, world.stepping()));
    out.bool("extraction_present", stub.is_some());
    match stub {
        Some(stub) => out.u64("extracted_ticks", stub.extracted_ticks()),
        None => out.string("extracted_ticks", "none"),
    }
    out.object_end();
}

fn harness_batches(out: &mut Json, scenario: &Scenario, workers: usize, reference: &Run) {
    out.key("harness");
    out.array_start();
    for (batch, pause) in BATCHES {
        let (mut world, _) = new_world(scenario, workers, DEFAULT_SEED, None);
        let mut paused = Duration::ZERO;
        let mut batches = 0_u64;
        let mut other_thread = false;
        let alternate_threads = batch == ALTERNATING_THREADS_BATCH;
        while world.tick() < scenario.ticks {
            let ticks = batch.min(scenario.ticks - world.tick());
            if alternate_threads && batches % 2 == 1 {
                thread::scope(|scope| {
                    scope.spawn(|| world.step(ticks).expect("the harness steps"));
                });
                other_thread = true;
            } else {
                world.step(ticks).expect("the harness steps");
            }
            batches += 1;
            if pause {
                let pausing = Instant::now();
                thread::sleep(PAUSE);
                paused += pausing.elapsed();
            }
        }
        out.object_start();
        out.u64("batch", u64::from(batch));
        out.u64("batches", batches);
        out.bool("pause_after_every_batch", pause);
        out.bool("alternating_threads", other_thread);
        write_comparison(out, &world, reference);
        out.decimal("stepping_ms", world.stepping().as_secs_f64() * 1_000.0);
        out.decimal("paused_ms", paused.as_secs_f64() * 1_000.0);
        out.decimal("ticks_per_second_while_stepping", ticks_per_second(scenario.ticks, world.stepping()));
        out.object_end();
    }
    out.array_end();
}

fn snapshot_and_restore(out: &mut Json, scenario: &Scenario, workers: usize, reference: &Run) {
    let batch = 100.min(scenario.snapshot_tick);
    let (mut original, _) = new_world(scenario, workers, DEFAULT_SEED, None);
    while original.tick() < scenario.snapshot_tick {
        original.step(batch.min(scenario.snapshot_tick - original.tick())).expect("steps to the snapshot tick");
    }
    let taking = Instant::now();
    let snapshot = original.snapshot().expect("a healthy world snapshots");
    let snapshot_time = taking.elapsed();
    let restoring = Instant::now();
    let (capabilities, _) = capabilities();
    let mut restored = World::restore(&snapshot, workers, capabilities).expect("the snapshot restores");
    let restore_time = restoring.elapsed();
    let hash_at_restore_matches =
        restored.hash() == original.hash() && restored.hash() == reference.tick_hashes[scenario.snapshot_tick as usize];
    let round_trip = restored.snapshot().expect("a restored world snapshots") == snapshot;
    while restored.tick() < scenario.ticks {
        let ticks = batch.min(scenario.ticks - restored.tick());
        restored.step(ticks).expect("the restored world steps");
        original.step(ticks).expect("the original steps");
    }
    let final_snapshots_identical = restored.snapshot().unwrap() == original.snapshot().unwrap();
    out.key("snapshot");
    out.object_start();
    out.u64("snapshot_tick", u64::from(scenario.snapshot_tick));
    out.u64("snapshot_bytes", snapshot.bytes.len() as u64);
    out.decimal("snapshot_us", snapshot_time.as_secs_f64() * 1_000_000.0);
    out.decimal("restore_us", restore_time.as_secs_f64() * 1_000_000.0);
    out.bool("hash_at_restore_matches", hash_at_restore_matches);
    out.bool("snapshot_round_trip_identical", round_trip);
    out.key("restored");
    out.object_start();
    out.u64("history_start", u64::from(restored.history_start()));
    write_comparison(out, &restored, reference);
    out.object_end();
    out.key("original");
    out.object_start();
    write_comparison(out, &original, reference);
    out.object_end();
    out.bool("final_snapshots_identical", final_snapshots_identical);
    out.object_end();
}

fn isolation(out: &mut Json, scenario: &Scenario, workers: usize, reference: &Run, second_reference: &Run) {
    let (mut first, _) = new_world(scenario, workers, DEFAULT_SEED, None);
    let (mut second, _) = new_world(scenario, workers, SECOND_SEED, None);
    while first.tick() < scenario.ticks {
        let ticks = ALTERNATION_BATCH.min(scenario.ticks - first.tick());
        first.step(ticks).expect("the first world steps");
        second.step(ticks).expect("the second world steps");
    }
    out.key("isolation");
    out.object_start();
    out.u64("alternation_batch", u64::from(ALTERNATION_BATCH));
    out.hex("second_seed", SECOND_SEED);
    out.key("first_world");
    out.object_start();
    write_comparison(out, &first, reference);
    out.object_end();
    out.key("second_world");
    out.object_start();
    write_comparison(out, &second, second_reference);
    out.object_end();
    out.object_end();
}

/// A panic hook that keeps the injected panic out of the standard error
/// stream and prints every other panic as usual.
fn silence_injected_panics(info: &std::panic::PanicHookInfo<'_>) {
    let payload = info.payload();
    let message =
        payload.downcast_ref::<String>().map(String::as_str).or_else(|| payload.downcast_ref::<&str>().copied()).unwrap_or("");
    if !message.contains("injected panic") {
        eprintln!("{info}");
    }
}

fn failure(out: &mut Json, scenario: &Scenario, workers: usize, reference: &Run, second_reference: &Run) {
    let injection = scenario.injection();
    let (mut healthy, _) = new_world(scenario, workers, DEFAULT_SEED, None);
    let (mut failing, _) = new_world(scenario, workers, SECOND_SEED, Some(injection));
    let mut failure = None;
    let mut healthy_tick_at_failure = None;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(silence_injected_panics));
    while healthy.tick() < scenario.ticks {
        let ticks = ALTERNATION_BATCH.min(scenario.ticks - healthy.tick());
        healthy.step(ticks).expect("the healthy world steps");
        if failure.is_none()
            && let Err(failed) = failing.step(ticks)
        {
            failure = Some(failed.0);
            healthy_tick_at_failure = Some(healthy.tick());
        }
    }
    let step_after_failure_rejected = failing.step(ALTERNATION_BATCH).is_err();
    let snapshot_after_failure_rejected = failing.snapshot().is_err();
    std::panic::set_hook(previous_hook);
    out.key("failure");
    out.object_start();
    out.key("injected");
    out.object_start();
    out.u64("tick", u64::from(injection.tick));
    out.string("node", injection.node.name());
    out.u64("chunk", injection.chunk as u64);
    out.object_end();
    out.bool("failure_reported", failure.is_some());
    if let Some(failure) = &failure {
        out.key("reported");
        out.object_start();
        out.u64("tick", u64::from(failure.tick));
        out.string("node", failure.node);
        out.u64("chunk", failure.chunk as u64);
        out.u64("worker", failure.worker as u64);
        out.string("message", &failure.message);
        out.object_end();
    }
    out.i64("healthy_world_tick_at_failure", healthy_tick_at_failure.map_or(-1, i64::from));
    out.key("failed_world");
    out.object_start();
    write_comparison(out, &failing, second_reference);
    out.bool("step_after_failure_rejected", step_after_failure_rejected);
    out.bool("snapshot_after_failure_rejected", snapshot_after_failure_rejected);
    out.object_end();
    out.key("healthy_world");
    out.object_start();
    write_comparison(out, &healthy, reference);
    out.object_end();
    out.object_end();
}

/// The time control report of this build's configuration: one object with
/// one entry per worker count.
fn time_control_report(scenario: &Scenario, worker_counts: &[usize]) -> String {
    let reference = scenario.reference(DEFAULT_SEED);
    let second_reference = scenario.reference(SECOND_SEED);
    let mut out = Json::new();
    out.object_start();
    out.string("configuration", configuration_name(PRESENTATION));
    out.bool("presentation", PRESENTATION);
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", scenario.size.name());
    out.u64("ticks", u64::from(scenario.ticks));
    out.hex("reference_run_hash", reference.run_hash);
    out.hex("second_reference_run_hash", second_reference.run_hash);
    out.key("runs");
    out.array_start();
    for &workers in worker_counts {
        out.object_start();
        out.u64("workers", workers as u64);
        headless(&mut out, scenario, workers, &reference);
        harness_batches(&mut out, scenario, workers, &reference);
        snapshot_and_restore(&mut out, scenario, workers, &reference);
        isolation(&mut out, scenario, workers, &reference, &second_reference);
        failure(&mut out, scenario, workers, &reference, &second_reference);
        out.object_end();
    }
    out.array_end();
    out.object_end();
    out.finish()
}

// ---------------------------------------------------------------------------
// Build measurements
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the spike lives in the workspace").to_path_buf()
}

fn cargo() -> Command {
    Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

struct BuildOutcome {
    seconds: f64,
    succeeded: bool,
    stderr_tail: String,
}

fn build(root: &Path, target_dir: &Path, release: bool, presentation: bool) -> BuildOutcome {
    let mut command = cargo();
    command.current_dir(root).args(["build", "--locked", "--package", PACKAGE, "--target-dir"]).arg(target_dir);
    if release {
        command.arg("--release");
    }
    if presentation {
        command.args(["--features", "presentation"]);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let started = Instant::now();
    let output = command.output();
    let seconds = started.elapsed().as_secs_f64();
    match output {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
            BuildOutcome { seconds, succeeded: output.status.success(), stderr_tail: tail }
        }
        Err(error) => BuildOutcome { seconds, succeeded: false, stderr_tail: error.to_string() },
    }
}

/// The names of the packages in the host's dependency graph for one
/// configuration, from `cargo tree` restricted to normal dependencies.
fn crate_graph(root: &Path, presentation: bool) -> Vec<String> {
    let mut command = cargo();
    command.current_dir(root).args(["tree", "--locked", "--package", PACKAGE, "--edges", "normal", "--prefix", "none"]);
    if presentation {
        command.args(["--features", "presentation"]);
    }
    let output = command.output().expect("cargo tree runs");
    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn presentation_library_built(target_dir: &Path, release: bool) -> bool {
    let deps = target_dir.join(if release { "release" } else { "debug" }).join("deps");
    fs::read_dir(deps).is_ok_and(|entries| {
        entries.flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("libskeleton_presentation"))
    })
}

/// The one-line change in the core: the marker constant's value. The original
/// text is restored when the guard drops, whatever happened in between.
struct EditMarker {
    path: PathBuf,
    original: String,
}

impl EditMarker {
    fn new(root: &Path) -> EditMarker {
        let path = root.join("runtime-skeleton").join("core").join("src").join("edit_marker.rs");
        let original = fs::read_to_string(&path).expect("the edit marker file exists");
        assert!(original.contains("EDIT_MARKER: u32 = 0;"), "the edit marker file holds the marker at 0");
        EditMarker { path, original }
    }

    fn set(&self, value: u32) {
        // Cargo compares the file's modification time with its last build's:
        // a short wait keeps the edit strictly after the previous build.
        thread::sleep(Duration::from_millis(20));
        let text = self.original.replace("EDIT_MARKER: u32 = 0;", &format!("EDIT_MARKER: u32 = {value};"));
        fs::write(&self.path, text).expect("the edit marker file is writable");
    }
}

impl Drop for EditMarker {
    fn drop(&mut self) {
        thread::sleep(Duration::from_millis(20));
        let _ = fs::write(&self.path, &self.original);
    }
}

fn write_build(out: &mut Json, configuration: &str, profile: &str, kind: &str, sample: u32, outcome: &BuildOutcome, presentation_library: Option<bool>) {
    out.object_start();
    out.string("configuration", configuration);
    out.string("profile", profile);
    out.string("kind", kind);
    out.u64("sample", u64::from(sample));
    out.decimal("seconds", outcome.seconds);
    out.bool("succeeded", outcome.succeeded);
    if let Some(present) = presentation_library {
        out.bool("presentation_library_built", present);
    }
    if !outcome.succeeded {
        out.string("stderr_tail", &outcome.stderr_tail);
    }
    out.object_end();
}

fn target_dir(presentation: bool) -> PathBuf {
    env::temp_dir().join(format!("{SPIKE}-build-{}", configuration_name(presentation)))
}

/// Measures both configurations: the dependency graph, then clean release,
/// clean dev, incremental dev and incremental release builds, the incremental
/// ones after a one-line change in the core. Returns the release binaries'
/// paths, without then with the presentation capability.
fn measure_builds(out: &mut Json, root: &Path) -> [PathBuf; 2] {
    let mut binaries = [PathBuf::new(), PathBuf::new()];
    out.key("crate_graph");
    out.object_start();
    for presentation in [false, true] {
        let names = crate_graph(root, presentation);
        out.key(configuration_name(presentation));
        out.object_start();
        out.bool("presentation_crate_in_graph", names.iter().any(|name| name == PRESENTATION_CRATE));
        out.u64("crates", names.len() as u64);
        out.key("names");
        out.array_start();
        for name in &names {
            out.object_start();
            out.string("name", name);
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.object_end();

    out.key("builds");
    out.array_start();
    for (index, presentation) in [false, true].into_iter().enumerate() {
        let configuration = configuration_name(presentation);
        let target_dir = target_dir(presentation);
        for sample in 1..=CLEAN_SAMPLES {
            let _ = fs::remove_dir_all(&target_dir);
            let release = build(root, &target_dir, true, presentation);
            write_build(out, configuration, "release", "clean", sample, &release, Some(presentation_library_built(&target_dir, true)));
        }
        binaries[index] = target_dir.join("release").join(format!("{PACKAGE}{}", env::consts::EXE_SUFFIX));
        for sample in 1..=CLEAN_SAMPLES {
            let _ = fs::remove_dir_all(target_dir.join("debug"));
            let dev = build(root, &target_dir, false, presentation);
            write_build(out, configuration, "dev", "clean", sample, &dev, Some(presentation_library_built(&target_dir, false)));
        }
        {
            let marker = EditMarker::new(root);
            for sample in 1..=INCREMENTAL_SAMPLES {
                marker.set(sample);
                let incremental = build(root, &target_dir, false, presentation);
                write_build(out, configuration, "dev", "incremental-after-core-edit", sample, &incremental, None);
            }
            // The guard restores the marker: one more build after the restore
            // is the same edit in the other direction.
            drop(marker);
            let restored = build(root, &target_dir, false, presentation);
            write_build(out, configuration, "dev", "incremental-after-core-edit", INCREMENTAL_SAMPLES + 1, &restored, None);
        }
        {
            let marker = EditMarker::new(root);
            marker.set(1);
            let incremental = build(root, &target_dir, true, presentation);
            write_build(out, configuration, "release", "incremental-after-core-edit", 1, &incremental, None);
            drop(marker);
            let restored = build(root, &target_dir, true, presentation);
            write_build(out, configuration, "release", "incremental-after-core-edit", 2, &restored, None);
        }
    }
    out.array_end();
    binaries
}

/// Runs the other configuration's binary for its time control report.
fn other_configuration_report(binary: &Path, worker_counts: &[usize]) -> Result<String, String> {
    let mut command = Command::new(binary);
    command.arg("--time-control");
    for workers in worker_counts {
        command.arg(workers.to_string());
    }
    let output = command.stdin(Stdio::null()).output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("exit {:?}: {}", output.status.code(), String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_headless_facts(out: &mut Json) {
    out.key("headless");
    out.object_start();
    out.bool("display_set", env::var_os("DISPLAY").is_some());
    out.bool("wayland_display_set", env::var_os("WAYLAND_DISPLAY").is_some());
    out.string("platform_libraries", "none: the binary links the standard library only");
    out.object_end();
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--time-control") {
        let counts: Vec<usize> = args[1..].iter().map(|a| a.parse().expect("a worker count")).collect();
        let counts = if counts.is_empty() { worker_counts() } else { counts };
        println!("{}", time_control_report(&MEASURED, &counts));
        return;
    }
    let cell = args.first().cloned().unwrap_or_else(|| "unspecified".to_string());
    let counts = worker_counts();
    let root = workspace_root();

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.string("built_configuration", configuration_name(PRESENTATION));
    out.key("worker_counts");
    out.array_start();
    for &w in &counts {
        out.object_start();
        out.u64("workers", w as u64);
        out.object_end();
    }
    out.array_end();

    // This configuration's time control report, in process.
    out.key("time_control");
    out.object_start();
    out.raw(configuration_name(PRESENTATION), &time_control_report(&MEASURED, &counts));
    // The build measurements, which also produce the other configuration's binary.
    let mut builds = Json::new();
    builds.object_start();
    let binaries = measure_builds(&mut builds, &root);
    builds.object_end();
    let other = !PRESENTATION;
    match other_configuration_report(&binaries[usize::from(other)], &counts) {
        Ok(report) => out.raw(configuration_name(other), &report),
        Err(error) => {
            out.key(configuration_name(other));
            out.object_start();
            out.string("error", &error);
            out.object_end();
        }
    }
    out.object_end();
    out.raw("build_measurements", &builds.finish());
    write_headless_facts(&mut out);
    for presentation in [false, true] {
        let _ = fs::remove_dir_all(target_dir(presentation));
    }
    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_control_report_holds_every_scenario_and_every_world_matches() {
        let scenario = Scenario { size: Size::Small, ticks: 40, snapshot_tick: 20, injection_tick: 13 };
        let report = time_control_report(&scenario, &[1, 3]);
        assert!(report.starts_with('{') && report.ends_with('}'));
        for key in ["\"headless\"", "\"harness\"", "\"snapshot\"", "\"isolation\"", "\"failure\"", "\"matches_reference\":true"] {
            assert!(report.contains(key), "missing {key}");
        }
        for check in [
            "matches_reference",
            "hash_at_restore_matches",
            "snapshot_round_trip_identical",
            "final_snapshots_identical",
            "step_after_failure_rejected",
            "snapshot_after_failure_rejected",
            "failure_reported",
        ] {
            assert!(!report.contains(&format!("\"{check}\":false")), "{check} fails somewhere: {report}");
        }
        assert!(report.contains("\"failure_reported\":true"));
        assert!(report.contains("\"healthy_world_tick_at_failure\":20"));
        assert_eq!(report.contains("\"extracted_ticks\":40"), PRESENTATION);
    }

    #[test]
    fn the_edit_marker_guard_restores_the_file() {
        let root = workspace_root();
        let path = root.join("runtime-skeleton/core/src/edit_marker.rs");
        let before = fs::read_to_string(&path).unwrap();
        {
            let marker = EditMarker::new(&root);
            marker.set(7);
            assert!(fs::read_to_string(&path).unwrap().contains("= 7;"));
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn the_configuration_names_follow_the_feature() {
        assert_eq!(configuration_name(false), "without-presentation");
        assert_eq!(configuration_name(true), "with-presentation");
        assert_eq!(PRESENTATION, cfg!(feature = "presentation"));
    }
}
