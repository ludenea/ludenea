//! Recording round trip spike: the orchestrator. Throwaway, never product code.
//!
//! It builds the suite of sixteen tests with the child `cargo`, then, as a
//! process that is never the one that ran a test:
//!
//! 1. runs the suite whole, assembles its report from the listing and the
//!    run's directory, and takes the recordings the failing test left;
//! 2. runs it again with the tests held after eight have started, ends the
//!    process from outside once four hold, assembles, then resumes the tests
//!    interrupted and not run and assembles again;
//! 3. lists every recording with the description-driven client and replays
//!    it, the ones this run wrote and the ones carried from the other cells
//!    (the files under `recordings/`);
//! 4. changes and cuts the files and replays them;
//! 5. measures what an invariant read after every one-tick step costs.
//!
//! One JSON document on standard output.

use std::env;
use std::fs;
use std::hint::black_box;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use reference_workload::{Json, fnv1a, write_platform_and_build};
use test_recording_round_trip::container;
use test_recording_round_trip::pool::{Executor, Runtime};
use test_recording_round_trip::recording::{Recording, Replay, Start, TAG_INPUTS, Verdict, replay, replay_recording};
use test_recording_round_trip::run::{Assembly, BinaryEnd, CELL_VARIABLE, Class, HOLD_VARIABLE, RUN_DIR_VARIABLE, assemble, parse_listing};
use test_recording_round_trip::world::{Size, World, WorldConfig};
use test_recording_round_trip::{Fixture, every_entity_is_inside_and_under_the_speed_limit, run_until_the_watched_group_thins};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "test-recording-round-trip";
const TEST_TARGET: &str = "suite";
const FAILING_TEST: &str = "t03_the_watched_group_keeps_its_members";
const TEST_COUNT: usize = 16;
const TEST_THREADS: usize = 4;
const HOLD_AFTER: usize = 8;
const REPLAY_WORKERS: [usize; 2] = [1, 4];
const HOLD_TIMEOUT: Duration = Duration::from_secs(600);

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the spike lives in the workspace").to_path_buf()
}

fn scratch(label: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("{SPIKE}-{}-{label}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory is creatable");
    dir
}

// ---------------------------------------------------------------------------
// Building and running the suite
// ---------------------------------------------------------------------------

/// Builds the integration test target in its own target directory (the
/// workspace's holds this running binary, which the build must not replace)
/// and finds the executable in the build tool's messages.
fn build_tests(release: bool) -> Result<PathBuf, String> {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .current_dir(workspace_root())
        .args(["test", "--no-run", "--locked", "--package", SPIKE, "--test", TEST_TARGET, "--message-format=json", "--target-dir"])
        .arg(env::temp_dir().join(format!("{SPIKE}-tests")));
    if release {
        command.arg("--release");
    }
    let output = command.stdin(Stdio::null()).output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(format!("exit {:?}: {tail}", output.status.code()));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("\"compiler-artifact\"") && line.contains(&format!("\"name\":\"{TEST_TARGET}\"")))
        .find_map(|line| json_string_value(line, "executable"))
        .map(PathBuf::from)
        .ok_or_else(|| "no compiler-artifact message names the test executable".to_string())
}

/// The value of `"key":"..."` in a flat run of JSON text, unescaped.
fn json_string_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = text.find(&needle)? + needle.len();
    let mut value = String::new();
    let mut chars = text[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'u' => {
                    let code: String = chars.by_ref().take(4).collect();
                    value.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
                }
                other => value.push(other),
            },
            other => value.push(other),
        }
    }
    None
}

fn listing(executable: &Path) -> Result<Vec<String>, String> {
    let output = Command::new(executable).args(["--list", "--format", "terse"]).stdin(Stdio::null()).output().map_err(|error| error.to_string())?;
    Ok(parse_listing(&String::from_utf8_lossy(&output.stdout)))
}

/// What the tests said on standard error, independently of the directory.
#[derive(Default)]
struct Signals {
    finished: Vec<(String, String)>,
    held: Vec<String>,
}

impl Signals {
    fn read(&mut self, line: &str) {
        let mut words = line.split(' ');
        match (words.next(), words.next(), words.next()) {
            (Some("finished"), Some(name), Some(outcome)) => self.finished.push((name.to_string(), outcome.to_string())),
            (Some("held"), Some(name), None) => self.held.push(name.to_string()),
            _ => {}
        }
    }
}

struct SuiteRun {
    end: BinaryEnd,
    signals: Signals,
    /// The harness's own verdicts, `ok` or `FAILED` by test name.
    harness: Vec<(String, String)>,
    error: Option<String>,
}

fn suite_command(executable: &Path, dir: &Path, cell: &str, names: &[String]) -> Command {
    let mut command = Command::new(executable);
    command.args(["--test-threads", &TEST_THREADS.to_string()]);
    if !names.is_empty() {
        command.arg("--exact").args(names);
    }
    command.env(RUN_DIR_VARIABLE, dir).env(CELL_VARIABLE, cell).env_remove(HOLD_VARIABLE).stdin(Stdio::null());
    command
}

/// Runs the suite, or the named tests of it, to its end.
fn run_suite(executable: &Path, dir: &Path, cell: &str, names: &[String]) -> SuiteRun {
    let output = suite_command(executable, dir, cell, names).output();
    let mut signals = Signals::default();
    match output {
        Ok(output) => {
            String::from_utf8_lossy(&output.stderr).lines().for_each(|line| signals.read(line));
            let stdout = String::from_utf8_lossy(&output.stdout);
            let harness = stdout
                .lines()
                .filter_map(|line| line.strip_prefix("test ")?.split_once(" ... "))
                .map(|(name, verdict)| (name.to_string(), verdict.to_string()))
                .collect();
            let end = if output.status.success() { BinaryEnd::Succeeded } else { BinaryEnd::Failed };
            SuiteRun { end, signals, harness, error: None }
        }
        Err(error) => SuiteRun { end: BinaryEnd::Lost, signals, harness: Vec::new(), error: Some(error.to_string()) },
    }
}

/// Runs the suite with the tests held after [`HOLD_AFTER`] have started, and
/// ends the process from outside once every test thread holds a test.
fn run_suite_and_end_it(executable: &Path, dir: &Path, cell: &str) -> SuiteRun {
    let mut command = suite_command(executable, dir, cell, &[]);
    command.env(HOLD_VARIABLE, HOLD_AFTER.to_string()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut signals = Signals::default();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return SuiteRun { end: BinaryEnd::Lost, signals, harness: Vec::new(), error: Some(error.to_string()) },
    };
    let stderr = child.stderr.take().expect("standard error is piped");
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + HOLD_TIMEOUT;
    let mut error = None;
    while signals.held.len() < TEST_THREADS {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => signals.read(&line),
            Err(_) => {
                error = Some(format!("{} tests held when the wait ended", signals.held.len()));
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    receiver.try_iter().for_each(|line| signals.read(&line));
    SuiteRun { end: BinaryEnd::Lost, signals, harness: Vec::new(), error }
}

// ---------------------------------------------------------------------------
// The assembly's checks
// ---------------------------------------------------------------------------

fn count(assembly: &Assembly, wanted: impl Fn(&Class) -> bool) -> u64 {
    assembly.names(wanted).len() as u64
}

fn recorded(class: &Class, outcome: &str) -> bool {
    matches!(class, Class::Recorded { outcome: o, .. } if o == outcome)
}

/// Whether every listed test is in the assembly exactly once, in order.
fn lists_every_test_once(assembly: &Assembly, listing: &[String]) -> bool {
    assembly.entries.iter().map(|(name, _)| name).eq(listing.iter()) && assembly.unmatched.is_empty()
}

/// Whether the assembly classes every test as the signals of the process say:
/// a test that said it finished has its outcome, a test that said it held is
/// interrupted, and every other one is `otherwise`.
fn agrees_with_the_signals(assembly: &Assembly, signals: &Signals, otherwise: &Class) -> bool {
    assembly.entries.iter().all(|(name, class)| {
        if let Some((_, outcome)) = signals.finished.iter().find(|(n, _)| n == name) {
            recorded(class, outcome)
        } else if signals.held.contains(name) {
            *class == Class::Interrupted
        } else {
            class == otherwise
        }
    })
}

fn write_assembly(out: &mut Json, key: &str, assembly: &Assembly) {
    out.key(key);
    out.array_start();
    for (name, class) in &assembly.entries {
        out.object_start();
        out.string("test", name);
        out.string("class", &class.name());
        if let Class::Recorded { final_hash, .. } = class {
            out.string("final_hash", final_hash);
        }
        out.object_end();
    }
    out.array_end();
}

// ---------------------------------------------------------------------------
// Recordings
// ---------------------------------------------------------------------------

fn write_replay(out: &mut Json, outcome: &Replay) {
    out.string("verdict", outcome.verdict.name());
    out.u64("records_applied", u64::from(outcome.records_applied));
    match &outcome.verdict {
        Verdict::Verified { last_tick, compared } => {
            out.u64("last_tick", u64::from(*last_tick));
            out.u64("hashes_compared", u64::from(*compared));
        }
        Verdict::Diverged { tick, recorded, replayed } => {
            out.u64("first_divergent_tick", u64::from(*tick));
            out.hex("recorded", *recorded);
            out.hex("replayed", *replayed);
        }
        Verdict::NothingCompared { last_tick } => out.u64("last_tick", u64::from(*last_tick)),
        Verdict::Refused { reason } => out.string("reason", reason),
    }
}

/// One file listed by the client and replayed at every worker count. Returns
/// whether the client listed it and every replay verified it.
fn write_file(out: &mut Json, origin: &str, name: &str, bytes: &[u8], runtimes: &[Runtime]) -> bool {
    out.object_start();
    out.string("origin", origin);
    out.string("file", name);
    out.u64("bytes", bytes.len() as u64);
    out.hex("digest", fnv1a(bytes));
    let mut holds = true;
    match container::list(bytes) {
        Ok(sections) => {
            out.bool("listed_from_the_description", true);
            out.key("sections");
            out.array_start();
            for section in &sections {
                out.object_start();
                out.string("tag", &section.tag);
                out.string("type", &section.type_name);
                out.u64("records", u64::from(section.records));
                out.u64("bytes", section.bytes as u64);
                out.hex("digest", section.digest);
                out.object_end();
            }
            out.array_end();
        }
        Err(error) => {
            holds = false;
            out.bool("listed_from_the_description", false);
            out.string("listing_error", &error.0);
        }
    }
    if let Ok(recording) = Recording::decode(bytes) {
        out.string("form", recording.form());
        out.string("writer_cell", &recording.writer_cell);
        out.string("writer_profile", &recording.writer_profile);
        out.string("test", &recording.test);
        out.u64("entity_count", u64::from(recording.entity_count));
        out.u64("start_tick", u64::from(recording.start_tick));
        out.hex("start_hash", recording.start_hash);
        out.u64("end_tick", u64::from(recording.end_tick));
        out.hex("last_recorded_hash", recording.hashes.last().map_or(0, |&(_, hash)| hash));
    }
    out.key("replays");
    out.array_start();
    for runtime in runtimes {
        let outcome = replay(bytes, runtime.pool());
        holds &= matches!(outcome.verdict, Verdict::Verified { .. });
        out.object_start();
        out.u64("workers", runtime.pool().workers() as u64);
        write_replay(out, &outcome);
        out.object_end();
    }
    out.array_end();
    out.bool("verified_at_every_worker_count", holds);
    out.object_end();
    holds
}

/// The files of a directory with the recording extension, by name.
fn recordings_in(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "rec")
            && let (Some(name), Ok(bytes)) = (path.file_name().and_then(|name| name.to_str()), fs::read(&path))
        {
            files.push((name.to_string(), bytes));
        }
    }
    files.sort();
    files
}

/// The checks on changed and cut files, on the two forms one test left.
/// Returns whether every one came out as the hypotheses state.
fn write_damage(out: &mut Json, self_contained: &[u8], reference: &[u8], runtime: &Runtime) -> bool {
    let executor = runtime.pool();
    let mut holds = true;
    out.key("damage");
    out.object_start();

    // One byte of one input record changed in place, the digests left as they
    // were: the container's integrity finds it, without a world.
    out.key("one_byte_changed_in_place");
    out.object_start();
    let inputs = container::list(self_contained).ok().and_then(|sections| sections.into_iter().find(|s| s.tag == container::tag_text(TAG_INPUTS)));
    if let Some(section) = inputs {
        let mut changed = self_contained.to_vec();
        changed[section.offset + section.bytes / 2] ^= 0x80;
        let listed = container::list(&changed);
        let outcome = replay(&changed, executor);
        let found = listed.is_err() && matches!(outcome.verdict, Verdict::Refused { .. }) && outcome.records_applied == 0;
        holds &= found;
        out.string("client", &listed.err().map_or("listed".to_string(), |error| error.0));
        write_replay(out, &outcome);
        out.bool("refused_before_any_step", found);
    } else {
        holds = false;
    }
    out.object_end();

    // The same change in a file that is whole again (the digests computed
    // over the changed bytes): only a replay finds it, at its tick.
    out.key("one_byte_changed_and_the_file_made_whole");
    out.object_start();
    match Recording::decode(self_contained) {
        Ok(mut recording) => {
            let index = recording.inputs.len() / 2;
            let tick = recording.inputs[index].tick;
            let delta = recording.inputs[index].kicks[0].delta_vx;
            recording.inputs[index].kicks[0].delta_vx = f32::from_bits(delta.to_bits() ^ 0x8000_0000);
            let changed = recording.encode();
            let differing = changed.iter().zip(self_contained).filter(|(a, b)| a != b).count();
            let listed = container::list(&changed).is_ok();
            let outcome = replay(&changed, executor);
            let found = listed && changed.len() == self_contained.len() && matches!(outcome.verdict, Verdict::Diverged { tick: reported, .. } if reported == tick);
            holds &= found;
            out.u64("changed_record_tick", u64::from(tick));
            out.u64("bytes_that_differ_digests_included", differing as u64);
            out.bool("client_lists_it_as_whole", listed);
            write_replay(out, &outcome);
            out.bool("reported_at_the_changed_tick", found);
        }
        Err(error) => {
            holds = false;
            out.string("error", &error.0);
        }
    }
    out.object_end();

    // The reference form with one record of its recipe's preparation changed,
    // each record in turn, the file whole: the replay rebuilds another
    // starting state and is refused before any record of the run, unless the
    // change's effect has died out by the starting tick and the state is the
    // same one.
    out.key("one_preparation_record_changed");
    out.object_start();
    match Recording::decode(reference) {
        Ok(recording) => {
            let (mut refused, mut same_state, mut wrong) = (0_u64, 0_u64, 0_u64);
            let mut same_state_ticks = Vec::new();
            let preparation_len = if let Start::Recipe(preparation) = &recording.start { preparation.len() } else { 0 };
            for index in 0..preparation_len {
                let mut changed = recording.clone();
                let Start::Recipe(preparation) = &mut changed.start else { unreachable!("the reference form") };
                let delta = preparation[index].kicks[0].delta_vx;
                preparation[index].kicks[0].delta_vx = f32::from_bits(delta.to_bits() ^ 0x8000_0000);
                let outcome = replay(&changed.encode(), executor);
                match outcome.verdict {
                    Verdict::Refused { ref reason } if reason.contains("starting state's hash") && outcome.records_applied == 0 => refused += 1,
                    Verdict::Verified { .. } => {
                        same_state += 1;
                        same_state_ticks.push((index + 1).to_string());
                    }
                    _ => wrong += 1,
                }
            }
            holds &= preparation_len > 0 && wrong == 0 && refused > 0;
            out.u64("preparation_records", preparation_len as u64);
            out.u64("refused_at_the_starting_hash_with_no_record_applied", refused);
            out.u64("same_starting_state_and_verified", same_state);
            out.string("ticks_of_the_changes_that_left_the_starting_state_the_same", &same_state_ticks.join(" "));
            out.u64("any_other_outcome", wrong);
        }
        Err(error) => {
            holds = false;
            out.string("error", &error.0);
        }
    }
    out.object_end();

    // The file cut at several lengths: refused by the client and by the
    // replay, before any step.
    out.key("cut_files");
    out.array_start();
    for (form, bytes) in [("self-contained", self_contained), ("reference", reference)] {
        for len in [0, 7, 20, 100, bytes.len() / 2, bytes.len() - 1] {
            let cut = &bytes[..len.min(bytes.len())];
            let listed = container::list(cut);
            let outcome = replay(cut, executor);
            let found = listed.is_err() && matches!(outcome.verdict, Verdict::Refused { .. }) && outcome.records_applied == 0;
            holds &= found;
            out.object_start();
            out.string("form", form);
            out.u64("bytes_kept", cut.len() as u64);
            out.u64("bytes_of_the_file", bytes.len() as u64);
            out.bool("refused_before_any_step", found);
            out.string("reason", &listed.err().map_or(String::new(), |error| error.0));
            out.object_end();
        }
    }
    out.array_end();

    // A recording whose hashes were trimmed away replays and is never called verified.
    out.key("hashes_trimmed");
    out.object_start();
    if let Ok(mut recording) = Recording::decode(reference) {
        recording.hashes.clear();
        let outcome = replay_recording(&recording, executor);
        holds &= matches!(outcome.verdict, Verdict::NothingCompared { .. });
        write_replay(out, &outcome);
    } else {
        holds = false;
    }
    out.object_end();

    out.bool("holds", holds);
    out.object_end();
    holds
}

// ---------------------------------------------------------------------------
// The cost of an invariant read after every tick
// ---------------------------------------------------------------------------

struct Cost {
    workers: usize,
    cadence: u32,
    /// Microseconds per tick, the best of the rounds: one multi-tick step,
    /// one-tick steps, one-tick steps each followed by the invariant.
    one_step: f64,
    tick_steps: f64,
    tick_steps_with_invariant: f64,
    same_state: bool,
}

fn timed_world(size: Size, cadence: u32, runtime: &Runtime) -> World {
    let mut world = World::new(WorldConfig::new(size).hashing_every(cadence));
    world.step_empty(10, runtime.pool()).expect("the warm-up does not fail");
    world
}

fn measure_cost(size: Size, ticks: u32, rounds: usize, workers: usize, cadence: u32) -> Cost {
    let runtime = Runtime::new(workers);
    let executor = runtime.pool();
    let per_tick = |started: Instant| started.elapsed().as_secs_f64() * 1e6 / f64::from(ticks);
    let (mut one_step, mut tick_steps, mut with_invariant) = (f64::MAX, f64::MAX, f64::MAX);
    let mut same_state = true;
    for _ in 0..rounds {
        let mut world = timed_world(size, cadence, &runtime);
        let started = Instant::now();
        world.step_empty(ticks, executor).expect("the batch does not fail");
        one_step = one_step.min(per_tick(started));
        let reference = world.snapshot().expect("a stepped world snapshots");

        let mut world = timed_world(size, cadence, &runtime);
        let started = Instant::now();
        for _ in 0..ticks {
            world.step_empty(1, executor).expect("a one-tick step does not fail");
        }
        tick_steps = tick_steps.min(per_tick(started));
        same_state &= world.snapshot().expect("a stepped world snapshots") == reference;

        let mut world = timed_world(size, cadence, &runtime);
        let mut held = true;
        let started = Instant::now();
        for _ in 0..ticks {
            world.step_empty(1, executor).expect("a one-tick step does not fail");
            held &= black_box(every_entity_is_inside_and_under_the_speed_limit(world.state()));
        }
        with_invariant = with_invariant.min(per_tick(started));
        same_state &= held && world.snapshot().expect("a stepped world snapshots") == reference;
    }
    Cost { workers, cadence, one_step, tick_steps, tick_steps_with_invariant: with_invariant, same_state }
}

/// The invariant alone on a state at rest, in nanoseconds per evaluation.
fn invariant_alone_ns(size: Size) -> f64 {
    let world = World::new(WorldConfig::new(size));
    let evaluations = 2_000_u32;
    let started = Instant::now();
    let mut held = true;
    for _ in 0..evaluations {
        held &= black_box(every_entity_is_inside_and_under_the_speed_limit(black_box(world.state())));
    }
    assert!(held, "the invariant holds on the initial state");
    started.elapsed().as_secs_f64() * 1e9 / f64::from(evaluations)
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

struct Settings {
    cell: String,
    release: bool,
    /// The size of the cost measurement and of the in-process recording.
    measured_size: Size,
    cost_ticks: u32,
    cost_rounds: usize,
}

fn measure(settings: &Settings) -> (String, bool) {
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &settings.cell);
    out.string("profile", if settings.release { "release" } else { "dev" });
    write_platform_and_build(&mut out);
    out.u64("available_parallelism", thread::available_parallelism().map_or(1, |n| n.get()) as u64);
    out.u64("test_threads", TEST_THREADS as u64);
    let mut every_check = true;

    let executable = match build_tests(settings.release) {
        Ok(executable) => executable,
        Err(error) => {
            out.string("build_error", &error);
            out.bool("every_check_holds", false);
            out.object_end();
            return (out.finish(), false);
        }
    };
    let listing = listing(&executable).unwrap_or_default();
    out.u64("listed_tests", listing.len() as u64);
    every_check &= listing.len() == TEST_COUNT;

    // 1. The whole run.
    let complete_dir = scratch("complete");
    let complete = run_suite(&executable, &complete_dir, &settings.cell, &[]);
    let complete_assembly = assemble(&listing, None, &complete_dir, complete.end);
    let harness_agrees = complete_assembly.entries.iter().all(|(name, class)| {
        let verdict = complete.harness.iter().find(|(n, _)| n == name).map(|(_, verdict)| verdict.as_str());
        matches!((class, verdict), (Class::Recorded { outcome, .. }, Some("ok")) if outcome == "passed")
            || matches!((class, verdict), (Class::Recorded { outcome, .. }, Some("FAILED")) if outcome == "failed")
    });
    let complete_holds = complete.error.is_none()
        && lists_every_test_once(&complete_assembly, &listing)
        && count(&complete_assembly, |c| recorded(c, "passed")) == TEST_COUNT as u64 - 1
        && recorded(complete_assembly.class(FAILING_TEST).unwrap_or(&Class::NotRun), "failed")
        && agrees_with_the_signals(&complete_assembly, &complete.signals, &Class::NotRun)
        && harness_agrees;
    every_check &= complete_holds;
    out.key("complete_run");
    out.object_start();
    out.u64("passed", count(&complete_assembly, |c| recorded(c, "passed")));
    out.u64("failed", count(&complete_assembly, |c| recorded(c, "failed")));
    out.bool("every_listed_test_once", lists_every_test_once(&complete_assembly, &listing));
    out.bool("agrees_with_the_harness_verdicts", harness_agrees);
    out.bool("holds", complete_holds);
    write_assembly(&mut out, "report", &complete_assembly);
    out.object_end();

    // 2. The run ended from outside, then resumed.
    let ended_dir = scratch("ended");
    let ended = run_suite_and_end_it(&executable, &ended_dir, &settings.cell);
    let ended_assembly = assemble(&listing, None, &ended_dir, ended.end);
    let ended_holds = ended.error.is_none()
        && lists_every_test_once(&ended_assembly, &listing)
        && ended.signals.finished.len() == HOLD_AFTER
        && ended.signals.held.len() == TEST_THREADS
        && agrees_with_the_signals(&ended_assembly, &ended.signals, &Class::NotRun)
        && count(&ended_assembly, |c| matches!(c, Class::Recorded { .. })) == HOLD_AFTER as u64
        && count(&ended_assembly, |c| *c == Class::Interrupted) == TEST_THREADS as u64
        && count(&ended_assembly, |c| *c == Class::NotRun) == (TEST_COUNT - HOLD_AFTER - TEST_THREADS) as u64;
    let to_resume = ended_assembly.names(|c| matches!(c, Class::Interrupted | Class::NotRun));
    let resumed = run_suite(&executable, &ended_dir, &settings.cell, &to_resume);
    let resumed_assembly = assemble(&listing, None, &ended_dir, resumed.end);
    let resumed_holds = resumed.error.is_none()
        && lists_every_test_once(&resumed_assembly, &listing)
        && resumed.signals.finished.len() == to_resume.len()
        && resumed.signals.finished.iter().all(|(name, _)| to_resume.contains(name))
        && resumed_assembly.entries == complete_assembly.entries;
    every_check &= ended_holds && resumed_holds;
    out.key("ended_run");
    out.object_start();
    if let Some(error) = &ended.error {
        out.string("error", error);
    }
    out.u64("tests_that_said_they_finished", ended.signals.finished.len() as u64);
    out.u64("tests_that_said_they_held", ended.signals.held.len() as u64);
    out.u64("with_an_outcome", count(&ended_assembly, |c| matches!(c, Class::Recorded { .. })));
    out.u64("interrupted", count(&ended_assembly, |c| *c == Class::Interrupted));
    out.u64("not_run", count(&ended_assembly, |c| *c == Class::NotRun));
    out.bool("every_listed_test_once", lists_every_test_once(&ended_assembly, &listing));
    out.bool("classes_agree_with_what_the_tests_said", agrees_with_the_signals(&ended_assembly, &ended.signals, &Class::NotRun));
    out.bool("holds", ended_holds);
    write_assembly(&mut out, "report", &ended_assembly);
    out.object_end();
    out.key("resumed_run");
    out.object_start();
    out.u64("tests_resumed", to_resume.len() as u64);
    out.u64("tests_that_said_they_finished", resumed.signals.finished.len() as u64);
    out.bool("every_listed_test_once", lists_every_test_once(&resumed_assembly, &listing));
    out.bool("same_outcomes_and_hashes_as_the_complete_run", resumed_assembly.entries == complete_assembly.entries);
    out.bool("holds", resumed_holds);
    out.object_end();

    // 3. The recordings: this run's, then the ones carried from other cells.
    let runtimes: Vec<Runtime> = REPLAY_WORKERS.iter().map(|&workers| Runtime::new(workers)).collect();
    let local = recordings_in(&complete_dir);
    let published = workspace_root().join("out").join(&settings.cell).join("recordings");
    fs::create_dir_all(&published).expect("the output directory is creatable");
    let mut portable = local.len() == 2;
    out.key("recordings");
    out.array_start();
    for (name, bytes) in &local {
        let form = name.trim_start_matches(FAILING_TEST).trim_start_matches('.');
        fs::write(published.join(format!("{}.{form}", settings.cell)), bytes).expect("the recording is publishable");
        portable &= write_file(&mut out, "this run, written by the test process", name, bytes, &runtimes);
    }
    let carried = recordings_in(&workspace_root().join(SPIKE).join("recordings"));
    for (name, bytes) in &carried {
        portable &= write_file(&mut out, "carried from another run", name, bytes, &runtimes);
    }
    out.array_end();
    out.u64("recordings_carried_from_other_runs", carried.len() as u64);
    out.bool("every_recording_verified", portable);
    every_check &= portable;

    // 4. Changed and cut files.
    let find = |form: &str| local.iter().find(|(name, _)| name.contains(form)).map(|(_, bytes)| bytes.as_slice());
    if let (Some(self_contained), Some(reference)) = (find("self-contained"), find("reference")) {
        every_check &= write_damage(&mut out, self_contained, reference, &runtimes[1]);
    } else {
        every_check = false;
    }

    // The same failing run at the measured size, recorded in this process:
    // its files are compared across cells by their digests.
    let executor = runtimes[1].pool();
    let fixture = Fixture::build(settings.measured_size, executor);
    let (session, failure) = run_until_the_watched_group_thins(&fixture, 3, 20_000, executor);
    out.key("measured_size_recording");
    out.object_start();
    out.string("size", settings.measured_size.name());
    out.bool("the_invariant_failed", failure.is_some());
    out.key("files");
    out.array_start();
    let mut measured_holds = failure.is_some();
    for (reference_form, form) in [(false, "self-contained"), (true, "reference")] {
        let bytes = session.recording(&fixture, reference_form, "the_same_run_in_the_orchestrator", &settings.cell).encode();
        let name = format!("{}.{}-size.{form}.rec", settings.cell, settings.measured_size.name());
        fs::write(published.join(&name), &bytes).expect("the recording is publishable");
        measured_holds &= write_file(&mut out, "this run, written by this process", &name, &bytes, &runtimes);
    }
    out.array_end();
    out.bool("holds", measured_holds);
    out.object_end();
    every_check &= measured_holds;

    // 5. The cost of an invariant.
    drop(runtimes);
    out.key("invariant_cost");
    out.object_start();
    out.string("size", settings.measured_size.name());
    out.u64("ticks", u64::from(settings.cost_ticks));
    out.u64("rounds", settings.cost_rounds as u64);
    out.decimal("invariant_alone_ns", invariant_alone_ns(settings.measured_size));
    out.key("rows");
    out.array_start();
    for workers in [1, 4] {
        for cadence in [1, 60] {
            let cost = measure_cost(settings.measured_size, settings.cost_ticks, settings.cost_rounds, workers, cadence);
            every_check &= cost.same_state;
            out.object_start();
            out.u64("workers", cost.workers as u64);
            out.u64("hash_cadence", u64::from(cost.cadence));
            out.decimal("one_multi_tick_step_us_per_tick", cost.one_step);
            out.decimal("one_tick_steps_us_per_tick", cost.tick_steps);
            out.decimal("one_tick_steps_with_the_invariant_us_per_tick", cost.tick_steps_with_invariant);
            out.decimal("invariant_us_per_tick", cost.tick_steps_with_invariant - cost.tick_steps);
            out.decimal("one_tick_steps_with_the_invariant_over_one_step", cost.tick_steps_with_invariant / cost.one_step);
            out.bool("same_final_state", cost.same_state);
            out.object_end();
        }
    }
    out.array_end();
    out.object_end();

    out.bool("every_check_holds", every_check);
    out.object_end();
    let _ = fs::remove_dir_all(&complete_dir);
    let _ = fs::remove_dir_all(&ended_dir);
    (out.finish(), every_check)
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "local".to_string());
    let release = !cfg!(debug_assertions);
    // An unoptimized build measures the small size, as the spikes before this
    // one do, and the report says which size ran.
    let settings = if release {
        Settings { cell, release, measured_size: Size::Default, cost_ticks: 600, cost_rounds: 5 }
    } else {
        Settings { cell, release, measured_size: Size::Small, cost_ticks: 120, cost_rounds: 3 }
    };
    let (report, _) = measure(&settings);
    println!("{report}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_executable_is_read_from_the_build_tool_messages() {
        let line = "{\"reason\":\"compiler-artifact\",\"target\":{\"kind\":[\"test\"],\"name\":\"suite\"},\"executable\":\"C:\\\\tmp\\\\deps\\\\suite-1a2b.exe\"}";
        assert_eq!(json_string_value(line, "executable").as_deref(), Some("C:\\tmp\\deps\\suite-1a2b.exe"));
        assert_eq!(json_string_value("{\"executable\":null}", "executable"), None);
    }

    #[test]
    fn the_signals_of_a_process_are_read_by_their_first_word() {
        let mut signals = Signals::default();
        for line in ["finished t00 passed", "held t09", "thread 't03' panicked at src/run.rs", "finished t03 failed", "held"] {
            signals.read(line);
        }
        assert_eq!(signals.finished, [("t00".to_string(), "passed".to_string()), ("t03".to_string(), "failed".to_string())]);
        assert_eq!(signals.held, ["t09"]);
    }

    /// The whole report on the small size, with the build of the suite, the
    /// run ended from outside and every check. The costs are measured, never
    /// asserted.
    #[test]
    fn the_whole_report_holds_on_the_small_size() {
        let settings = Settings { cell: "self-check".to_string(), release: !cfg!(debug_assertions), measured_size: Size::Small, cost_ticks: 20, cost_rounds: 1 };
        let (report, holds) = measure(&settings);
        let _ = fs::remove_dir_all(workspace_root().join("out").join("self-check"));
        assert!(holds, "{report}");
    }
}
