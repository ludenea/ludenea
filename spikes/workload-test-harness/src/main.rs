//! Orchestrator of the test harness spike, for the game testing research.
//! Throwaway, never product code.
//!
//! The spike's tests are ordinary tests of the standard harness (see
//! `tests/divergence.rs`); this binary, which the research CI builds and
//! runs, measures the suite instead of assuming: it builds the test
//! executable in both configurations (without and with the presentation
//! capability) with the child `cargo`, runs it on the measured size alone
//! (one test at a time), in parallel at the machine's parallelism and with
//! every test at once, once with the failing test armed and once on the
//! build with the presentation capability, collects the report every test
//! writes, and checks that every test's hashes are the hashes it gives alone,
//! that the divergence reports name the injected ticks and the energy, that
//! the fixture restores under 2 milliseconds, and that the armed test fails
//! alone while the fifteen others pass.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Instant;

use reference_workload::{Json, write_platform_and_build};
use workload_test_harness::{PANIC_VARIABLE, REPORT_DIR_VARIABLE, SIZE_VARIABLE};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-test-harness";
const PACKAGE: &str = "workload-test-harness";
const TEST_TARGET: &str = "divergence";
const TEST_COUNT: u64 = 16;
const PANIC_TEST: &str = "a_panic_in_a_system_fails_alone";
const EVERY_TEST_AT_ONCE: usize = 16;

fn configuration_name(presentation: bool) -> &'static str {
    if presentation { "with-presentation" } else { "without-presentation" }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the spike lives in the workspace").to_path_buf()
}

fn cargo() -> Command {
    Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn available_parallelism() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get())
}

// ---------------------------------------------------------------------------
// Building the test executable
// ---------------------------------------------------------------------------

struct Built {
    configuration: &'static str,
    seconds: f64,
    executable: Option<PathBuf>,
    error: Option<String>,
}

fn target_dir(presentation: bool) -> PathBuf {
    env::temp_dir().join(format!("{SPIKE}-tests-{}", configuration_name(presentation)))
}

/// Builds the integration test target in its own target directory (the
/// workspace's holds this running binary, which the build must not replace)
/// and finds the executable in the compiler's messages.
fn build_tests(root: &Path, release: bool, presentation: bool) -> Built {
    let configuration = configuration_name(presentation);
    let mut command = cargo();
    command
        .current_dir(root)
        .args(["test", "--no-run", "--locked", "--package", PACKAGE, "--test", TEST_TARGET, "--message-format=json", "--target-dir"])
        .arg(target_dir(presentation));
    if release {
        command.arg("--release");
    }
    if presentation {
        command.args(["--features", "presentation"]);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let started = Instant::now();
    let output = command.output();
    let seconds = started.elapsed().as_secs_f64();
    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match executable_from_messages(&stdout) {
                Some(executable) => Built { configuration, seconds, executable: Some(executable), error: None },
                None => Built { configuration, seconds, executable: None, error: Some("no compiler-artifact message names the test executable".to_string()) },
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
            Built { configuration, seconds, executable: None, error: Some(format!("exit {:?}: {tail}", output.status.code())) }
        }
        Err(error) => Built { configuration, seconds, executable: None, error: Some(error.to_string()) },
    }
}

/// The `executable` of the `compiler-artifact` message whose target is the
/// test target, from the compiler messages Cargo prints one per line.
fn executable_from_messages(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .filter(|line| line.contains("\"compiler-artifact\"") && line.contains(&format!("\"name\":\"{TEST_TARGET}\"")))
        .find_map(|line| json_string_value(line, "executable"))
        .map(PathBuf::from)
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

/// The raw token after `"key":` in a flat JSON object: a quoted string
/// without its quotes, or a number or boolean.
fn field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    if let Some(quoted) = rest.strip_prefix('"') {
        quoted.find('"').map(|end| &quoted[..end])
    } else {
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

fn number(json: &str, key: &str) -> Option<f64> {
    field(json, key)?.parse().ok()
}

// ---------------------------------------------------------------------------
// Running the suite
// ---------------------------------------------------------------------------

struct TestRun {
    configuration: &'static str,
    label: &'static str,
    threads: usize,
    panic_armed: bool,
    wall_ms: f64,
    exit_code: Option<i32>,
    success: bool,
    passed: u64,
    failed: u64,
    summary_line: String,
    outcomes: Vec<(String, String)>,
    failure_messages: Vec<(String, String)>,
    /// The per-test reports, by test name, as the tests wrote them.
    reports: Vec<(String, String)>,
}

impl TestRun {
    fn report(&self, name: &str) -> Option<&str> {
        self.reports.iter().find(|(n, _)| n == name).map(|(_, json)| json.as_str())
    }

    fn passed_test(&self, name: &str) -> bool {
        self.outcomes.iter().any(|(n, outcome)| n == name && outcome == "ok")
    }

    fn setup_ms(&self) -> f64 {
        self.reports.iter().filter_map(|(_, json)| number(json, "shared_setup_ms")).fold(0.0, f64::max)
    }
}

/// Runs the test executable once with the given thread count, collecting the
/// harness's output and the reports the tests wrote in a fresh directory.
fn run_tests(executable: &Path, configuration: &'static str, label: &'static str, threads: usize, panic_armed: bool, size: &str) -> TestRun {
    let report_dir = env::temp_dir().join(format!("{SPIKE}-reports-{}-{label}-{}", std::process::id(), configuration));
    let _ = fs::remove_dir_all(&report_dir);
    fs::create_dir_all(&report_dir).expect("the report directory is creatable");
    let mut command = Command::new(executable);
    command
        .args(["--test-threads", &threads.to_string()])
        .env(SIZE_VARIABLE, size)
        .env(REPORT_DIR_VARIABLE, &report_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if panic_armed {
        command.env(PANIC_VARIABLE, "1");
    } else {
        command.env_remove(PANIC_VARIABLE);
    }
    let started = Instant::now();
    let output = command.output();
    let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let (stdout, exit_code, success) = match &output {
        Ok(output) => (String::from_utf8_lossy(&output.stdout).into_owned(), output.status.code(), output.status.success()),
        Err(error) => (format!("could not run the test executable: {error}"), None, false),
    };
    let parsed = parse_harness_output(&stdout);
    let mut reports: Vec<(String, String)> = fs::read_dir(&report_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    let name = path.file_stem()?.to_str()?.to_string();
                    fs::read_to_string(&path).ok().map(|json| (name, json))
                })
                .collect()
        })
        .unwrap_or_default();
    reports.sort();
    let _ = fs::remove_dir_all(&report_dir);
    TestRun {
        configuration,
        label,
        threads,
        panic_armed,
        wall_ms,
        exit_code,
        success,
        passed: parsed.passed,
        failed: parsed.failed,
        summary_line: parsed.summary_line,
        outcomes: parsed.outcomes,
        failure_messages: parsed.failure_messages,
        reports,
    }
}

struct Parsed {
    outcomes: Vec<(String, String)>,
    summary_line: String,
    passed: u64,
    failed: u64,
    failure_messages: Vec<(String, String)>,
}

/// Reads the standard harness's output: one `test <name> ... <outcome>` line
/// per test, the `failures:` section with each failed test's captured output
/// (the panic message), and the `test result:` summary line.
fn parse_harness_output(stdout: &str) -> Parsed {
    let mut outcomes = Vec::new();
    let mut summary_line = String::new();
    let mut failure_messages = Vec::new();
    let lines: Vec<&str> = stdout.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("test ")
            && let Some((name, outcome)) = rest.split_once(" ... ")
            && !name.contains(' ')
        {
            outcomes.push((name.to_string(), outcome.trim().to_string()));
        } else if line.starts_with("test result:") {
            summary_line = line.to_string();
        } else if let Some(name) = line.strip_prefix("---- ").and_then(|rest| rest.strip_suffix(" stdout ----")) {
            let mut block = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && !lines[j].starts_with("---- ") && lines[j] != "failures:" && !lines[j].starts_with("test result:") {
                block.push(lines[j]);
                j += 1;
            }
            // The message follows the "panicked at" line and precedes the backtrace note.
            let message: Vec<&str> = block
                .iter()
                .copied()
                .skip_while(|l| !l.contains("panicked at"))
                .skip(1)
                .take_while(|l| !l.starts_with("note: run with"))
                .filter(|l| !l.trim().is_empty())
                .collect();
            let message = if message.is_empty() { block.join("\n").trim().to_string() } else { message.join("\n") };
            failure_messages.push((name.to_string(), message.chars().take(600).collect()));
            i = j;
            continue;
        }
        i += 1;
    }
    let count = |word: &str| -> u64 {
        summary_line
            .split([';', '.'])
            .filter_map(|part| {
                let part = part.trim();
                part.strip_suffix(word).and_then(|n| n.trim().parse().ok())
            })
            .next()
            .unwrap_or(0)
    };
    Parsed { outcomes, summary_line: summary_line.clone(), passed: count(" passed"), failed: count(" failed"), failure_messages }
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

fn write_run(out: &mut Json, run: &TestRun) {
    out.object_start();
    out.string("configuration", run.configuration);
    out.string("label", run.label);
    out.u64("test_threads", run.threads as u64);
    out.bool("panic_armed", run.panic_armed);
    out.decimal("wall_ms", run.wall_ms);
    out.decimal("setup_ms", run.setup_ms());
    out.i64("exit_code", run.exit_code.map_or(-1, i64::from));
    out.bool("success", run.success);
    out.u64("passed", run.passed);
    out.u64("failed", run.failed);
    out.u64("reports", run.reports.len() as u64);
    out.string("summary_line", &run.summary_line);
    out.key("outcomes");
    out.object_start();
    for (name, outcome) in &run.outcomes {
        out.string(name, outcome);
    }
    out.object_end();
    out.key("failure_messages");
    out.object_start();
    for (name, message) in &run.failure_messages {
        out.string(name, message);
    }
    out.object_end();
    out.key("tests");
    out.object_start();
    for (name, json) in &run.reports {
        out.raw(name, json);
    }
    out.object_end();
    out.object_end();
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let release = !cfg!(debug_assertions);
    let size = if release { "default" } else { "small" };
    let ticks: u64 = if release { 1_000 } else { 40 };
    let cores = available_parallelism();
    let root = workspace_root();

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.string("size", size);
    out.u64("ticks", ticks);
    out.u64("test_count", TEST_COUNT);
    out.string("profile", if release { "release" } else { "dev" });

    let builds = [build_tests(&root, release, false), build_tests(&root, release, true)];
    out.key("builds");
    out.array_start();
    for built in &builds {
        out.object_start();
        out.string("configuration", built.configuration);
        out.decimal("seconds", built.seconds);
        out.bool("succeeded", built.executable.is_some());
        if let Some(error) = &built.error {
            out.string("error", error);
        }
        out.object_end();
    }
    out.array_end();

    let mut runs = Vec::new();
    if let Some(plain) = &builds[0].executable {
        runs.push(run_tests(plain, configuration_name(false), "solo", 1, false, size));
        runs.push(run_tests(plain, configuration_name(false), "parallel-at-the-machine-parallelism", cores, false, size));
        runs.push(run_tests(plain, configuration_name(false), "parallel-every-test-at-once", EVERY_TEST_AT_ONCE, false, size));
        runs.push(run_tests(plain, configuration_name(false), "panic-armed", cores, true, size));
    }
    if let Some(with) = &builds[1].executable {
        runs.push(run_tests(with, configuration_name(true), "with-presentation", cores, false, size));
    }
    out.key("runs");
    out.array_start();
    for run in &runs {
        write_run(&mut out, run);
    }
    out.array_end();

    // Every test's hashes are the hashes it gives alone, on both builds: both
    // run hashes of a test without an injection equal the solo run's; a test
    // with an injection stops at a first divergent tick that varies with the
    // timing, so its world A must match the reference over the ticks it ran
    // (its world B carries the timing-dependent reduction on purpose).
    let solo = runs.iter().find(|run| run.label == "solo");
    let names: Vec<String> = solo.map(|run| run.reports.iter().map(|(name, _)| name.clone()).collect()).unwrap_or_default();
    let mut mismatching = Vec::new();
    for name in &names {
        let Some(reference) = solo.and_then(|run| run.report(name)) else { continue };
        let injected = number(reference, "injection_tick").is_some_and(|t| t >= 0.0);
        let expected = (field(reference, "run_hash_a"), field(reference, "run_hash_b"));
        for run in &runs {
            if !run.passed_test(name) {
                continue;
            }
            let Some(json) = run.report(name) else { continue };
            let same = if injected {
                field(json, "a_matches_reference") == Some("true")
            } else {
                (field(json, "run_hash_a"), field(json, "run_hash_b")) == expected
            };
            if !same {
                mismatching.push(format!("{name} in {} {}", run.configuration, run.label));
            }
        }
    }
    out.bool("hashes_identical_across_runs", mismatching.is_empty() && !names.is_empty());
    out.key("hashes_mismatching");
    out.array_start();
    for entry in &mismatching {
        out.object_start();
        out.string("test", entry);
        out.object_end();
    }
    out.array_end();
    out.bool(
        "every_run_passed_except_the_armed_one",
        runs.iter().all(|run| if run.panic_armed { !run.success } else { run.success && run.passed == TEST_COUNT && run.failed == 0 }),
    );

    // World-level parallelism: the suite's wall time against the tests alone,
    // read three ways: the speedup over the solo run, the ratio to the ideal
    // of the solo wall times spread over the cores (an ideal the tests cannot
    // reach, since a test at 4 workers already uses several cores alone), and
    // the ratio to the ideal of the worker time (every world's stepping time
    // times its worker count, an upper bound of the processor time) spread
    // over the cores.
    let solo_walls: Vec<(String, f64)> =
        solo.map(|run| run.reports.iter().filter_map(|(name, json)| number(json, "wall_ms").map(|w| (name.clone(), w))).collect()).unwrap_or_default();
    let sum_solo: f64 = solo_walls.iter().map(|(_, w)| w).sum();
    let (longest_name, longest_solo) = solo_walls.iter().fold((String::new(), 0.0_f64), |best, (name, w)| if *w > best.1 { (name.clone(), *w) } else { best });
    let worker_time: f64 = solo
        .map(|run| {
            run.reports
                .iter()
                .filter_map(|(_, json)| {
                    Some(number(json, "stepping_ms_a")? * number(json, "workers_a")? + number(json, "stepping_ms_b")? * number(json, "workers_b")?)
                })
                .sum()
        })
        .unwrap_or(0.0);
    let ratio = |numerator: f64, denominator: f64| if denominator > 0.0 { numerator / denominator } else { 0.0 };
    out.key("parallelism");
    out.object_start();
    out.u64("cores", cores as u64);
    out.decimal("sum_of_solo_tests_ms", sum_solo);
    out.decimal("longest_solo_test_ms", longest_solo);
    out.string("longest_solo_test", &longest_name);
    out.decimal("worker_time_estimate_ms", worker_time);
    out.key("suites");
    out.array_start();
    for run in &runs {
        let effective = run.threads.min(cores) as f64;
        let minus_setup = run.wall_ms - run.setup_ms();
        out.object_start();
        out.string("configuration", run.configuration);
        out.string("label", run.label);
        out.u64("test_threads", run.threads as u64);
        out.bool("panic_armed", run.panic_armed);
        out.decimal("suite_wall_ms", run.wall_ms);
        out.decimal("setup_ms", run.setup_ms());
        out.decimal("suite_wall_minus_setup_ms", minus_setup);
        out.decimal("speedup_over_solo", ratio(sum_solo, minus_setup));
        out.decimal("wall_ideal_ms", ratio(sum_solo, effective));
        out.decimal("ratio_to_wall_ideal", ratio(minus_setup, ratio(sum_solo, effective)));
        out.decimal("worker_time_ideal_ms", ratio(worker_time, effective));
        out.decimal("ratio_to_worker_time_ideal", ratio(minus_setup, ratio(worker_time, effective)));
        out.decimal("ratio_to_longest_solo", ratio(minus_setup, longest_solo));
        out.object_end();
    }
    out.array_end();
    out.object_end();

    // The fixture: restore times over every world, per run (alone, and under
    // the load of the parallel runs).
    out.key("fixture");
    out.array_start();
    for run in &runs {
        let restores: Vec<f64> =
            run.reports.iter().flat_map(|(_, json)| [number(json, "restore_us_a"), number(json, "restore_us_b")]).flatten().collect();
        out.object_start();
        out.string("configuration", run.configuration);
        out.string("label", run.label);
        out.u64("test_threads", run.threads as u64);
        out.u64("restores", restores.len() as u64);
        out.decimal("restore_us_max", restores.iter().copied().fold(0.0, f64::max));
        out.decimal("restore_us_mean", if restores.is_empty() { 0.0 } else { restores.iter().sum::<f64>() / restores.len() as f64 });
        out.bool("under_2_ms", !restores.is_empty() && restores.iter().all(|&us| us < 2_000.0));
        out.object_end();
    }
    out.array_end();

    // The divergence reports of the injected tests, per run.
    out.key("divergence");
    out.array_start();
    for run in &runs {
        let injected: Vec<&str> = run.reports.iter().map(|(_, json)| json.as_str()).filter(|json| number(json, "injection_tick").is_some_and(|t| t >= 0.0)).collect();
        let found = injected.iter().filter(|json| field(json, "divergence_found") == Some("true")).count();
        let equal = injected.iter().filter(|json| field(json, "reported_tick_equals_injected") == Some("true")).count();
        let energy_first = injected.iter().filter(|json| field(json, "first_divergent_group") == Some("energy")).count();
        let only_energy = injected.iter().filter(|json| field(json, "groups_differing") == Some("energy")).count();
        let snapshots_differ = injected.iter().filter(|json| field(json, "divergence_found") == Some("true") && field(json, "snapshots_identical") == Some("false")).count();
        let max_lag = injected
            .iter()
            .filter_map(|json| Some(number(json, "first_divergent_tick")? - number(json, "injection_tick")?))
            .filter(|lag| *lag >= 0.0)
            .fold(0.0, f64::max);
        out.object_start();
        out.string("configuration", run.configuration);
        out.string("label", run.label);
        out.u64("injected_tests", injected.len() as u64);
        out.u64("divergence_found", found as u64);
        out.u64("reported_tick_equals_injected", equal as u64);
        out.u64("energy_first", energy_first as u64);
        out.u64("only_energy_differs", only_energy as u64);
        out.u64("snapshots_differ", snapshots_differ as u64);
        out.decimal("max_lag_ticks", max_lag);
        out.key("tests");
        out.array_start();
        for json in &injected {
            out.object_start();
            out.string("name", field(json, "name").unwrap_or(""));
            out.i64("injected_tick", number(json, "injection_tick").unwrap_or(-1.0) as i64);
            out.i64("reported_tick", number(json, "first_divergent_tick").unwrap_or(-1.0) as i64);
            out.string("first_divergent_group", field(json, "first_divergent_group").unwrap_or(""));
            out.string("first_divergent_system", field(json, "first_divergent_system").unwrap_or(""));
            out.string("groups_differing", field(json, "groups_differing").unwrap_or(""));
            out.bool("snapshots_identical", field(json, "snapshots_identical") == Some("true"));
            out.i64("first_differing_byte", number(json, "first_differing_byte").unwrap_or(-1.0) as i64);
            out.u64("snapshot_bytes", number(json, "snapshot_bytes_a").unwrap_or(0.0) as u64);
            out.object_end();
        }
        out.array_end();
        out.object_end();
    }
    out.array_end();

    // The armed run: one test fails with its report, the others pass.
    out.key("panic_run");
    out.object_start();
    match runs.iter().find(|run| run.panic_armed) {
        Some(run) => {
            let failed: Vec<&str> = run.outcomes.iter().filter(|(_, outcome)| outcome != "ok").map(|(name, _)| name.as_str()).collect();
            out.bool("ran", true);
            out.bool("exit_failure", !run.success);
            out.u64("passed", run.passed);
            out.u64("failed", run.failed);
            out.string("failed_tests", &failed.join(","));
            out.bool("failed_alone", failed == [PANIC_TEST] && run.passed == TEST_COUNT - 1);
            out.string("message", run.failure_messages.iter().find(|(name, _)| name == PANIC_TEST).map_or("", |(_, message)| message.as_str()));
            if let Some(json) = run.report(PANIC_TEST) {
                out.bool("failure_reported", field(json, "failure_reported") == Some("true"));
                out.i64("failure_tick", number(json, "failure_tick").unwrap_or(-1.0) as i64);
                out.string("failure_node", field(json, "failure_node").unwrap_or(""));
                out.i64("failure_chunk", number(json, "failure_chunk").unwrap_or(-1.0) as i64);
                out.i64("failure_worker", number(json, "failure_worker").unwrap_or(-1.0) as i64);
                out.bool("world_a_matches_reference", field(json, "a_matches_reference") == Some("true"));
            }
        }
        None => out.bool("ran", false),
    }
    out.object_end();

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
    fn the_test_executable_is_read_from_the_compiler_messages() {
        let messages = concat!(
            "{\"reason\":\"compiler-artifact\",\"target\":{\"kind\":[\"lib\"],\"name\":\"workload-test-harness\"},\"executable\":null}\n",
            "{\"reason\":\"compiler-artifact\",\"target\":{\"kind\":[\"test\"],\"crate_types\":[\"bin\"],\"name\":\"divergence\"},",
            "\"executable\":\"C:\\\\tmp\\\\deps\\\\divergence-1a2b.exe\",\"fresh\":false}\n",
            "{\"reason\":\"build-finished\",\"success\":true}\n"
        );
        assert_eq!(executable_from_messages(messages), Some(PathBuf::from("C:\\tmp\\deps\\divergence-1a2b.exe")));
        assert_eq!(executable_from_messages("{\"reason\":\"build-finished\",\"success\":true}"), None);
    }

    #[test]
    fn the_harness_output_is_read_into_outcomes_messages_and_counts() {
        let output = concat!(
            "\nrunning 3 tests\n",
            "test no_injection_1_vs_4_workers ... ok\n",
            "test a_panic_in_a_system_fails_alone ... FAILED\n",
            "test injection_at_500_1_vs_4_workers ... ok\n",
            "\nfailures:\n\n---- a_panic_in_a_system_fails_alone stdout ----\n\n",
            "thread 'a_panic_in_a_system_fails_alone' (1234) panicked at tests/divergence.rs:127:9:\n",
            "world B failed at tick 300 in repulsion chunk 2 on worker 1: injected panic\n",
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n\n",
            "failures:\n    a_panic_in_a_system_fails_alone\n\n",
            "test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.23s\n"
        );
        let parsed = parse_harness_output(output);
        assert_eq!(parsed.outcomes.len(), 3);
        assert_eq!(parsed.outcomes[1], ("a_panic_in_a_system_fails_alone".to_string(), "FAILED".to_string()));
        assert_eq!(parsed.passed, 2);
        assert_eq!(parsed.failed, 1);
        assert_eq!(parsed.failure_messages, vec![("a_panic_in_a_system_fails_alone".to_string(), "world B failed at tick 300 in repulsion chunk 2 on worker 1: injected panic".to_string())]);
        assert!(parsed.summary_line.starts_with("test result: FAILED."));
    }

    #[test]
    fn flat_json_fields_are_read_as_tokens() {
        let json = r#"{"name":"x_1","wall_ms":12.500,"divergence_found":true,"group":"energy","last":-1}"#;
        assert_eq!(field(json, "name"), Some("x_1"));
        assert_eq!(number(json, "wall_ms"), Some(12.5));
        assert_eq!(field(json, "divergence_found"), Some("true"));
        assert_eq!(field(json, "group"), Some("energy"));
        assert_eq!(number(json, "last"), Some(-1.0));
        assert_eq!(field(json, "missing"), None);
        assert_eq!(json_string_value(r#"{"a":"x\"y\\z"}"#, "a"), Some("x\"y\\z".to_string()));
    }
}
