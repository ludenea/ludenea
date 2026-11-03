//! Headless modes spike, for the headless execution research. Throwaway, never
//! product code.
//!
//! The spike measures, instead of assuming, what "headless" means for a host
//! that steps the reference workload:
//!
//! - the headless and harness hosts run wherever the research CI runs them,
//!   a minimal Linux container without windowing, graphics or audio libraries
//!   included, and give the sequential reference's hashes;
//! - the binary built without the presentation capability imports no
//!   windowing, graphics or audio library of its platform, read from the
//!   binary's own dynamic imports, while the build with the capability imports
//!   the one library it links for discovery (the positive control);
//! - the same binary runs from a non-interactive context (a child with an
//!   empty environment, detached and without a console on Windows);
//! - N worlds on N threads reach N times the single-world throughput, the
//!   first measured number of worlds per machine for this workload;
//! - a capability that is not built, discovered absent, or requested absent
//!   gives one and the same self-description.
//!
//! Started with `--child-hosts`, the binary runs the hosts at one worker and
//! prints their report with its session facts; with `--describe`, it prints
//! its self-description and what its discovery found; `--headless` requests
//! the headless mode in either; `--small` shrinks the workload for the
//! self-checks.

mod imports;
mod session;
mod world;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use reference_workload::{DEFAULT_SEED, DEFAULT_TICK_COUNT, Json, Parameters, Run, Size, first_divergent_tick, run_hash, run_sequential, write_platform_and_build};

use imports::Imports;
use session::{Description, Discovery, discover, write_session};
use world::{Capabilities, Extraction, State, World, WorldConfig};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "headless-modes";
const PACKAGE: &str = "headless-modes";
const PRESENTATION: bool = cfg!(feature = "presentation");
const BASE_WORKER_COUNTS: [usize; 2] = [1, 4];
const BASE_WORLD_COUNTS: [usize; 3] = [1, 2, 4];
const HARNESS_BATCH: u32 = 10;
const PAUSE: Duration = Duration::from_millis(1);

fn configuration_name() -> &'static str {
    if PRESENTATION { "with-presentation" } else { "without-presentation" }
}

#[derive(Clone, Copy)]
struct Scenario {
    size: Size,
    ticks: u32,
}

const MEASURED: Scenario = Scenario { size: Size::Default, ticks: DEFAULT_TICK_COUNT };
const SMALL: Scenario = Scenario { size: Size::Small, ticks: 40 };

impl Scenario {
    fn reference(&self, seed: u64) -> Run {
        run_sequential(&Parameters { seed, ..Parameters::new(self.size, self.ticks, false) })
    }

    fn flag(&self) -> Option<&'static str> {
        (self.size == Size::Small).then_some("--small")
    }
}

fn available_parallelism() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get())
}

fn worker_counts() -> Vec<usize> {
    let mut counts = BASE_WORKER_COUNTS.to_vec();
    counts.push(available_parallelism());
    counts.sort_unstable();
    counts.dedup();
    counts
}

fn world_counts() -> Vec<usize> {
    let available = available_parallelism();
    let mut counts = BASE_WORLD_COUNTS.to_vec();
    counts.push(available);
    if available > 4 {
        counts.extend([6, 8].into_iter().filter(|&n| n < available));
    }
    counts.sort_unstable();
    counts.dedup();
    counts
}

// ---------------------------------------------------------------------------
// The presentation capability, when built
// ---------------------------------------------------------------------------

/// The skeleton spike's stub extraction node: reads the state after every
/// tick (it counts the alive entities) and writes nothing but its counters.
#[derive(Default)]
struct StubExtraction {
    extracted_ticks: std::sync::atomic::AtomicU64,
}

impl Extraction for StubExtraction {
    fn extract(&self, _tick: u32, state: &State) {
        let _alive = state.alive.iter().filter(|&&alive| alive).count();
        self.extracted_ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The capabilities a host registers: the extraction node when the capability
/// is built, was discovered and was not requested absent.
fn capabilities(present: bool) -> (Capabilities, Option<Arc<StubExtraction>>) {
    if present {
        let stub = Arc::new(StubExtraction::default());
        (Capabilities { extraction: Some(stub.clone()) }, Some(stub))
    } else {
        (Capabilities::default(), None)
    }
}

// ---------------------------------------------------------------------------
// The hosts
// ---------------------------------------------------------------------------

fn write_comparison(out: &mut Json, hashes: &[u64], reference: &Run) {
    let divergence = first_divergent_tick(hashes, &reference.tick_hashes);
    out.u64("tick", hashes.len() as u64 - 1);
    out.bool("matches_reference", divergence.is_none());
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    out.hex("run_hash", run_hash(hashes));
    out.hex("final_hash", *hashes.last().expect("at least the initial hash"));
}

fn ticks_per_second(ticks: u32, duration: Duration) -> f64 {
    if duration.is_zero() { 0.0 } else { f64::from(ticks) / duration.as_secs_f64() }
}

/// The headless host: `ticks` ticks back to back.
fn headless_host(scenario: &Scenario, workers: usize, seed: u64, capabilities: Capabilities) -> World {
    let mut world = World::new(WorldConfig::new(scenario.size, workers).seeded(seed), capabilities);
    world.step(scenario.ticks).expect("the headless host steps");
    world
}

/// The harness host: batches of ten with a 1 ms pause after every batch, as a
/// stepping tool would leave between steps. Returns the world and the time paused.
fn harness_host(scenario: &Scenario, workers: usize, capabilities: Capabilities) -> (World, Duration) {
    let mut world = World::new(WorldConfig::new(scenario.size, workers), capabilities);
    let mut paused = Duration::ZERO;
    while world.tick() < scenario.ticks {
        world.step(HARNESS_BATCH.min(scenario.ticks - world.tick())).expect("the harness host steps");
        let pausing = Instant::now();
        thread::sleep(PAUSE);
        paused += pausing.elapsed();
    }
    (world, paused)
}

/// Both hosts at each worker count, compared with the reference.
fn write_hosts(out: &mut Json, scenario: &Scenario, worker_counts: &[usize], reference: &Run, present: bool) {
    // A warm-up world of a few ticks, then the measured ones.
    headless_host(&Scenario { ticks: 10.min(scenario.ticks), ..*scenario }, worker_counts[0], DEFAULT_SEED, Capabilities::default());
    out.key("hosts");
    out.array_start();
    for &workers in worker_counts {
        out.object_start();
        out.u64("workers", workers as u64);
        let (registered, stub) = capabilities(present);
        let world = headless_host(scenario, workers, DEFAULT_SEED, registered);
        out.key("headless");
        out.object_start();
        write_comparison(out, &world.hashes(), reference);
        out.decimal("stepping_ms", world.stepping().as_secs_f64() * 1_000.0);
        out.decimal("ticks_per_second", ticks_per_second(scenario.ticks, world.stepping()));
        out.bool("extraction_present", stub.is_some());
        out.i64("extracted_ticks", stub.map_or(-1, |stub| stub.extracted_ticks.load(std::sync::atomic::Ordering::Relaxed) as i64));
        out.object_end();
        let (registered, _) = capabilities(present);
        let (world, paused) = harness_host(scenario, workers, registered);
        out.key("harness");
        out.object_start();
        out.u64("batch", u64::from(HARNESS_BATCH));
        write_comparison(out, &world.hashes(), reference);
        out.decimal("stepping_ms", world.stepping().as_secs_f64() * 1_000.0);
        out.decimal("paused_ms", paused.as_secs_f64() * 1_000.0);
        out.decimal("ticks_per_second_while_stepping", ticks_per_second(scenario.ticks, world.stepping()));
        out.object_end();
        out.object_end();
    }
    out.array_end();
}

// ---------------------------------------------------------------------------
// The self-description and the child modes
// ---------------------------------------------------------------------------

fn write_description(out: &mut Json, description: &Description, discovery: &Discovery, headless: bool) {
    out.string("configuration", configuration_name());
    out.bool("headless_requested", headless);
    out.raw("description", &description.json());
    discovery.write(out);
}

/// `--describe`: the self-description of this build in this environment.
fn describe_report(headless: bool) -> String {
    let discovery = discover();
    let description = Description::new(PRESENTATION, discovery.found, headless);
    let mut out = Json::new();
    out.object_start();
    write_description(&mut out, &description, &discovery, headless);
    write_session(&mut out);
    out.object_end();
    out.finish()
}

/// `--child-hosts`: the hosts at one worker, with the session facts of the
/// child's own environment and its own reference.
fn child_hosts_report(scenario: &Scenario, headless: bool) -> String {
    let discovery = discover();
    let description = Description::new(PRESENTATION, discovery.found, headless);
    let present = PRESENTATION && discovery.found && !headless;
    let reference = scenario.reference(DEFAULT_SEED);
    let mut out = Json::new();
    out.object_start();
    write_description(&mut out, &description, &discovery, headless);
    write_session(&mut out);
    out.hex("reference_run_hash", reference.run_hash);
    write_hosts(&mut out, scenario, &[1], &reference, present);
    out.object_end();
    out.finish()
}

/// The `description` object of a child's report, as the stable string.
fn extract_description(report: &str) -> Option<String> {
    let start = report.find("\"description\":{")? + "\"description\":".len();
    let end = report[start..].find('}')? + start + 1;
    Some(report[start..end].to_string())
}

fn run_child(command: &mut Command) -> Result<String, String> {
    let output = command.stdin(Stdio::null()).stderr(Stdio::piped()).output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("exit {:?}: {}", output.status.code(), String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_child(out: &mut Json, key: &str, result: &Result<String, String>) {
    match result {
        Ok(report) => out.raw(key, report),
        Err(error) => {
            out.key(key);
            out.object_start();
            out.string("error", error);
            out.object_end();
        }
    }
}

// ---------------------------------------------------------------------------
// The imports
// ---------------------------------------------------------------------------

fn write_imports(out: &mut Json, key: &str, path: &Path) {
    out.key(key);
    out.object_start();
    out.string("file", &path.display().to_string());
    match Imports::of_file(path) {
        Ok(imports) => {
            let denied = imports.denied();
            out.string("format", imports.format);
            out.key("libraries");
            out.array_start();
            for name in &imports.libraries {
                out.object_start();
                out.string("name", name);
                out.object_end();
            }
            out.array_end();
            out.bool("imports_a_windowing_graphics_or_audio_library", !denied.is_empty());
            out.key("denied");
            out.array_start();
            for name in &denied {
                out.object_start();
                out.string("name", name);
                out.object_end();
            }
            out.array_end();
        }
        Err(error) => out.string("error", &error),
    }
    out.object_end();
}

// ---------------------------------------------------------------------------
// The other configuration
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the spike lives in the workspace").to_path_buf()
}

fn other_target_dir() -> PathBuf {
    env::temp_dir().join(format!("{SPIKE}-build-other-{}", std::process::id()))
}

/// Builds the other configuration (with the presentation feature when this
/// build lacks it, without it otherwise) into a temporary target directory.
/// Returns the binary's path, or the build's failure as a fact.
fn build_other_configuration(out: &mut Json, root: &Path, target_dir: &Path) -> Option<PathBuf> {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(root).args(["build", "--release", "--locked", "--package", PACKAGE, "--target-dir"]).arg(target_dir);
    if !PRESENTATION {
        command.args(["--features", "presentation"]);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let started = Instant::now();
    let output = command.output();
    let seconds = started.elapsed().as_secs_f64();
    out.key("build");
    out.object_start();
    out.string("configuration", if PRESENTATION { "without-presentation" } else { "with-presentation" });
    out.decimal("seconds", seconds);
    let built = match output {
        Ok(output) => {
            out.bool("succeeded", output.status.success());
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let tail: String = stderr.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
                out.string("stderr_tail", &tail);
            }
            output.status.success()
        }
        Err(error) => {
            out.bool("succeeded", false);
            out.string("stderr_tail", &error.to_string());
            false
        }
    };
    out.object_end();
    built.then(|| target_dir.join("release").join(format!("{PACKAGE}{}", env::consts::EXE_SUFFIX)))
}

/// What the other configuration's run yields: its two descriptions
/// (discovered, and requested absent), whether its discovery found a display
/// or session, and its binary, kept until the caller drops the directory.
struct OtherConfiguration {
    discovered: Option<String>,
    requested_absent: Option<String>,
    found: Option<bool>,
    binary: Option<PathBuf>,
    target_dir: PathBuf,
}

impl Drop for OtherConfiguration {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.target_dir);
    }
}

/// The other configuration's imports, descriptions (discovered, and requested
/// absent) and hosts.
fn write_other_configuration(out: &mut Json, scenario: &Scenario) -> OtherConfiguration {
    let root = workspace_root();
    let target_dir = other_target_dir();
    out.key("other_configuration");
    out.object_start();
    let binary = build_other_configuration(out, &root, &target_dir);
    let mut discovered = None;
    let mut requested_absent = None;
    let mut found = None;
    if let Some(binary) = &binary {
        write_imports(out, "imports", binary);
        let describe = run_child(Command::new(binary).arg("--describe"));
        if let Ok(report) = &describe {
            discovered = extract_description(report);
            found = Some(report.contains("\"found\":true"));
        }
        write_child(out, "describe", &describe);
        let headless = run_child(Command::new(binary).args(["--describe", "--headless"]));
        if let Ok(report) = &headless {
            requested_absent = extract_description(report);
        }
        write_child(out, "describe_headless", &headless);
        let mut hosts = Command::new(binary);
        hosts.arg("--child-hosts");
        if let Some(flag) = scenario.flag() {
            hosts.arg(flag);
        }
        write_child(out, "child_hosts", &run_child(&mut hosts));
    }
    out.object_end();
    OtherConfiguration { discovered, requested_absent, found, binary, target_dir }
}

// ---------------------------------------------------------------------------
// The non-interactive child
// ---------------------------------------------------------------------------

/// The binary as a child with an empty environment (the loader's variables
/// kept on Windows), detached and without a console on Windows.
fn write_non_interactive(out: &mut Json, scenario: &Scenario, binary: &Path) {
    let mut command = Command::new(binary);
    command.arg("--child-hosts");
    if let Some(flag) = scenario.flag() {
        command.arg(flag);
    }
    command.env_clear();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        for name in ["SystemRoot", "windir", "TEMP", "TMP", "PATH"] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        const DETACHED_PROCESS: u32 = 0x8;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    out.key("non_interactive");
    out.object_start();
    out.string("binary", &binary.display().to_string());
    out.string("context", if cfg!(target_os = "windows") {
        "a detached child process without a console, its environment reduced to the loader's variables"
    } else {
        "a child process with an empty environment"
    });
    write_child(out, "child", &run_child(&mut command));
    out.object_end();
}

// ---------------------------------------------------------------------------
// Many worlds
// ---------------------------------------------------------------------------

struct References {
    scenario: Scenario,
    runs: Vec<Run>,
}

impl References {
    fn new(scenario: Scenario) -> References {
        References { scenario, runs: Vec::new() }
    }

    /// The reference of seed `DEFAULT_SEED + index`, computed once.
    fn get(&mut self, index: usize) -> &Run {
        while self.runs.len() <= index {
            let seed = DEFAULT_SEED + self.runs.len() as u64;
            self.runs.push(self.scenario.reference(seed));
        }
        &self.runs[index]
    }
}

/// N worlds, one per thread, each at one worker; then one world at N workers.
fn write_many_worlds(out: &mut Json, scenario: &Scenario, world_counts: &[usize], references: &mut References) {
    out.key("many_worlds");
    out.object_start();
    out.key("sequential_reference_ticks_per_second");
    out.array_start();
    for index in 0..world_counts.iter().copied().max().unwrap_or(1) {
        let run = references.get(index);
        out.object_start();
        out.hex("seed", run.parameters.seed);
        out.decimal("ticks_per_second", run.ticks_per_second());
        out.object_end();
    }
    out.array_end();
    let mut single_world_ticks_per_second = None;
    out.key("world_level");
    out.array_start();
    for &n in world_counts {
        let worlds: Vec<World> =
            (0..n).map(|i| World::new(WorldConfig::new(scenario.size, 1).seeded(DEFAULT_SEED + i as u64), Capabilities::default())).collect();
        let started = Instant::now();
        let worlds: Vec<World> = thread::scope(|scope| {
            let handles: Vec<_> = worlds
                .into_iter()
                .map(|mut world| {
                    scope.spawn(move || {
                        world.step(scenario.ticks).expect("a world steps");
                        world
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().expect("a world's thread completes")).collect()
        });
        let wall = started.elapsed();
        let total = ticks_per_second(scenario.ticks * n as u32, wall);
        let single = *single_world_ticks_per_second.get_or_insert(total);
        let ratio = if single > 0.0 { total / (single * n as f64) } else { 0.0 };
        let per_world: Vec<f64> = worlds.iter().map(|world| ticks_per_second(scenario.ticks, world.stepping())).collect();
        let matching = worlds.iter().enumerate().filter(|(i, world)| first_divergent_tick(&world.hashes(), &references.get(*i).tick_hashes).is_none()).count();
        out.object_start();
        out.u64("worlds", n as u64);
        out.u64("workers_per_world", 1);
        out.u64("worlds_matching_their_reference", matching as u64);
        out.bool("every_world_matches", matching == n);
        out.decimal("wall_ms", wall.as_secs_f64() * 1_000.0);
        out.decimal("total_ticks_per_second", total);
        out.decimal("per_world_ticks_per_second_mean", per_world.iter().sum::<f64>() / n as f64);
        out.decimal("per_world_ticks_per_second_min", per_world.iter().copied().fold(f64::INFINITY, f64::min));
        out.decimal("ratio_to_n_times_single_world", ratio);
        out.bool("within_15_percent", ratio >= 0.85);
        out.object_end();
    }
    out.array_end();
    out.key("intra_world");
    out.array_start();
    for &n in world_counts {
        let world = headless_host(scenario, n, DEFAULT_SEED, Capabilities::default());
        out.object_start();
        out.u64("worlds", 1);
        out.u64("workers_per_world", n as u64);
        out.bool("matches_reference", first_divergent_tick(&world.hashes(), &references.get(0).tick_hashes).is_none());
        out.decimal("ticks_per_second", ticks_per_second(scenario.ticks, world.stepping()));
        out.object_end();
    }
    out.array_end();
    out.object_end();
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// `self_binary` is this binary's path for the non-interactive child; the
/// self-checks, which run inside the test harness, pass none and the other
/// configuration's binary stands in.
fn report(cell: &str, scenario: &Scenario, worker_counts: &[usize], world_counts: &[usize], self_binary: Option<&Path>) -> String {
    let mut references = References::new(*scenario);
    let discovery = discover();
    let description = Description::new(PRESENTATION, discovery.found, false);
    let present = PRESENTATION && discovery.found;
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", cell);
    write_platform_and_build(&mut out);
    out.u64("workload_version", u64::from(reference_workload::VERSION));
    out.string("size", scenario.size.name());
    out.u64("ticks", u64::from(scenario.ticks));
    out.key("worker_counts");
    out.array_start();
    for &w in worker_counts {
        out.object_start();
        out.u64("workers", w as u64);
        out.object_end();
    }
    out.array_end();
    write_description(&mut out, &description, &discovery, false);
    write_session(&mut out);
    out.hex("reference_run_hash", references.get(0).run_hash);
    write_hosts(&mut out, scenario, worker_counts, references.get(0), present);
    write_imports(&mut out, "imports", &env::current_exe().expect("the running binary has a path"));
    let other = write_other_configuration(&mut out, scenario);
    out.key("descriptions");
    out.object_start();
    out.raw("this_build", &description.json());
    out.bool("this_build_capability_built", PRESENTATION);
    match &other.discovered {
        Some(text) => out.raw("other_build_discovered", text),
        None => out.string("other_build_discovered", "unavailable"),
    }
    out.string("other_build_discovery_found", &other.found.map_or("unavailable".to_string(), |f| f.to_string()));
    match &other.requested_absent {
        Some(text) => out.raw("other_build_requested_absent", text),
        None => out.string("other_build_requested_absent", "unavailable"),
    }
    out.bool("requested_absent_equals_this_build", other.requested_absent.as_deref() == Some(description.json().as_str()));
    out.bool("discovered_equals_this_build", other.discovered.as_deref() == Some(description.json().as_str()));
    out.object_end();
    match self_binary.or(other.binary.as_deref()) {
        Some(binary) => write_non_interactive(&mut out, scenario, binary),
        None => out.string("non_interactive", "skipped: no binary to run"),
    }
    drop(other);
    write_many_worlds(&mut out, scenario, world_counts, &mut references);
    out.object_end();
    out.finish()
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let headless = args.iter().any(|a| a == "--headless");
    let scenario = if args.iter().any(|a| a == "--small") { SMALL } else { MEASURED };
    match args.first().map(String::as_str) {
        Some("--describe") => println!("{}", describe_report(headless)),
        Some("--child-hosts") => println!("{}", child_hosts_report(&scenario, headless)),
        first => {
            let cell = first.unwrap_or("unspecified");
            let exe = env::current_exe().expect("the running binary has a path");
            println!("{}", report(cell, &MEASURED, &worker_counts(), &world_counts(), Some(&exe)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_runs_every_scenario_and_every_world_matches() {
        let text = report("self-check", &SMALL, &[1, 2, 3], &[1, 3], None);
        assert!(text.starts_with('{') && text.ends_with('}'));
        for key in ["\"hosts\"", "\"imports\"", "\"other_configuration\"", "\"descriptions\"", "\"non_interactive\"", "\"many_worlds\"", "\"session\""] {
            assert!(text.contains(key), "missing {key}");
        }
        assert!(!text.contains("\"matches_reference\":false"), "{text}");
        assert!(!text.contains("\"every_world_matches\":false"), "{text}");
        assert!(!text.contains("\"error\""), "{text}");
        // This build's own imports carry no windowing library unless the
        // positive control is linked.
        let own = text.find("\"imports\":{").unwrap();
        let own_end = text[own..].find("\"other_configuration\"").unwrap() + own;
        assert!(text[own..own_end].contains(&format!("\"imports_a_windowing_graphics_or_audio_library\":{PRESENTATION}")), "{}", &text[own..own_end]);
        // The other configuration built, and requesting absence describes
        // the host like a build without the capability.
        assert!(text.contains("\"succeeded\":true"), "{text}");
        let absent = Description::new(false, false, false).json();
        assert!(text.contains(&format!("\"other_build_requested_absent\":{absent}")), "{text}");
        if PRESENTATION {
            // The other build has no capability: discovered absent by construction.
            assert!(text.contains(&format!("\"other_build_discovered\":{absent}")), "{text}");
        } else {
            assert!(text.contains("\"requested_absent_equals_this_build\":true"), "{text}");
        }
        // The non-interactive child saw no session variable and matched.
        let child = text.find("\"non_interactive\":{").unwrap();
        let child_text = &text[child..text.find("\"many_worlds\"").unwrap()];
        assert!(child_text.contains("\"matches_reference\":true"), "{child_text}");
        for variable in ["DISPLAY", "SECURITYSESSIONID", "SESSIONNAME"] {
            assert!(!child_text.contains(&format!("\"{variable}\":true")), "{child_text}");
        }
    }

    #[test]
    fn the_child_modes_print_their_reports() {
        let describe = describe_report(true);
        assert!(describe.contains("\"headless_requested\":true"));
        assert!(describe.contains(&format!("\"description\":{}", Description::new(false, false, false).json())));
        assert_eq!(extract_description(&describe).as_deref(), Some(Description::new(false, false, false).json().as_str()));
        let hosts = child_hosts_report(&SMALL, false);
        assert!(hosts.contains("\"matches_reference\":true") && !hosts.contains("\"matches_reference\":false"), "{hosts}");
        assert!(hosts.contains("\"session\":{"));
        assert_eq!(configuration_name(), if PRESENTATION { "with-presentation" } else { "without-presentation" });
    }

    #[test]
    fn the_world_counts_cover_the_machine() {
        let counts = world_counts();
        assert!(counts.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(counts.contains(&1) && counts.contains(&available_parallelism()));
        assert!(worker_counts().contains(&available_parallelism()));
    }
}
