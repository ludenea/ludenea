//! Platform threading constraints probe for the cross-platform compatibility
//! research.
//!
//! Throwaway spike, never product code. It answers one question per platform:
//! which windowing, event loop and graphics device operations the platform
//! binds to the main thread, which it allows from any thread, and which are
//! unavailable on a machine without a display (the research CI runners).
//!
//! Every probe runs in a child process, so that an assertion, an abort or a
//! hang inside a platform library is recorded as a fact ("crashed", "timed
//! out") instead of ending the measurement. The child prints one JSON line
//! with the probe's outcome and details; the parent collects the outcomes, the
//! exit status, the duration and the tail of the standard error stream (where
//! platform libraries print their warnings) into one fact sheet.
//!
//! Outcomes, in the child's vocabulary: `succeeded` (the operation under test
//! worked), `failed` (the platform refused it), `unavailable` (the resource the
//! probe needs does not exist on this machine: no display, no device, no
//! library), `unsupported` (the probe does not apply to this platform). The
//! parent adds `crashed`, `panicked`, `timed out` and `no report`.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

const SCHEMA_VERSION: u64 = 1;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const SELF_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const STDERR_TAIL_BYTES: usize = 600;

/// What one probe measures.
pub struct ProbeSpec {
    pub name: &'static str,
    pub question: &'static str,
    /// The probe needs a display server (Linux only: the parent starts a
    /// virtual one when the image has it and no display is set).
    pub needs_display: bool,
}

/// The child's report: the outcome of the operation under test and the facts
/// it observed on the way.
pub struct Report {
    pub outcome: &'static str,
    pub details: Vec<(String, String)>,
}

impl Report {
    pub fn new(outcome: &'static str) -> Self {
        Report {
            outcome,
            details: Vec::new(),
        }
    }

    pub fn detail(mut self, key: &str, value: impl ToString) -> Self {
        self.details.push((key.to_string(), value.to_string()));
        self
    }
}

/// Diagnostic probes shared by every platform; they exercise the runner itself
/// (the self-check) and never enter the fact sheet.
const DIAGNOSTIC_PROBES: [&str; 4] = ["noop", "crash", "hang", "panic"];

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--probe") => {
            let name = arguments.get(1).expect("--probe needs a probe name");
            let report = run_probe(name);
            println!("{}", report_json(&report));
        }
        Some("--self-check") => self_check(),
        _ => {
            let cell = arguments
                .first()
                .cloned()
                .unwrap_or_else(|| "unspecified".to_string());
            println!("{}", fact_sheet(&cell));
        }
    }
}

fn run_probe(name: &str) -> Report {
    match name {
        "noop" => Report::new("succeeded").detail("thread", "child main"),
        "crash" => std::process::abort(),
        "hang" => loop {
            thread::sleep(Duration::from_secs(1));
        },
        "panic" => panic!("deliberate panic of the diagnostic probe"),
        _ => platform::run(name),
    }
}

/// The parent side: runs every probe of the platform in a child process and
/// writes the fact sheet.
fn fact_sheet(cell: &str) -> String {
    let executable = std::env::current_exe().expect("the spike knows its own path");
    let session = platform::Session::open();
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", "platform-threading-constraints");
    out.string("cell", cell);
    out.key("platform");
    out.object_start();
    out.string("os", std::env::consts::OS);
    out.string("family", std::env::consts::FAMILY);
    out.string("arch", std::env::consts::ARCH);
    out.u64(
        "available_parallelism",
        thread::available_parallelism().map_or(0, |n| n.get() as u64),
    );
    out.object_end();
    out.key("build");
    out.object_start();
    out.string("package_version", env!("CARGO_PKG_VERSION"));
    out.bool("debug_assertions", cfg!(debug_assertions));
    out.object_end();
    out.key("session");
    out.object_start();
    for (key, value) in session.facts() {
        out.string(&key, &value);
    }
    out.object_end();
    out.key("probes");
    out.array_start();
    for spec in platform::PROBES {
        let environment = session.environment_for(spec);
        let run = run_in_child(&executable, spec.name, &environment, PROBE_TIMEOUT);
        out.separator();
        out.object_start();
        out.string("name", spec.name);
        out.string("question", spec.question);
        out.bool("needs_display", spec.needs_display);
        out.string("outcome", &run.outcome);
        out.string("exit", &run.exit);
        out.u64("duration_ms", run.duration_ms);
        out.key("details");
        out.object_start();
        for (key, value) in &run.details {
            out.string(key, value);
        }
        out.object_end();
        out.string("stderr_notes", &run.stderr_notes);
        out.string("stderr_tail", &run.stderr_tail);
        out.object_end();
    }
    out.array_end();
    out.object_end();
    drop(session);
    out.finish()
}

/// The outcome of one child run, as the parent recorded it.
struct ChildRun {
    outcome: String,
    details: Vec<(String, String)>,
    exit: String,
    duration_ms: u64,
    stderr_notes: String,
    stderr_tail: String,
}

fn run_in_child(
    executable: &std::path::Path,
    probe: &str,
    environment: &[(String, String)],
    timeout: Duration,
) -> ChildRun {
    let start = Instant::now();
    let mut command = Command::new(executable);
    command
        .arg("--probe")
        .arg(probe)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in environment {
        command.env(key, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ChildRun {
                outcome: "no report".to_string(),
                details: vec![("spawn_error".to_string(), error.to_string())],
                exit: "not started".to_string(),
                duration_ms: 0,
                stderr_notes: String::new(),
                stderr_tail: String::new(),
            };
        }
    };
    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let (status, timed_out) = wait_with_timeout(&mut child, timeout);
    let stdout = stdout.recv().unwrap_or_default();
    let stderr = stderr.recv().unwrap_or_default();
    let duration_ms = start.elapsed().as_millis() as u64;
    let exit = match (&status, timed_out) {
        (_, true) => format!("killed after {} ms", timeout.as_millis()),
        (Some(status), false) => describe_exit(status),
        (None, false) => "unknown".to_string(),
    };
    let (outcome, details) = if timed_out {
        ("timed out".to_string(), Vec::new())
    } else {
        match parse_report(&stdout) {
            Some((outcome, details)) => (outcome, details),
            None => match status {
                Some(status) if status.code() == Some(101) => ("panicked".to_string(), Vec::new()),
                Some(status) if status.success() => ("no report".to_string(), Vec::new()),
                _ => ("crashed".to_string(), Vec::new()),
            },
        }
    };
    ChildRun {
        outcome,
        details,
        exit,
        duration_ms,
        stderr_notes: notable_lines(&stderr),
        stderr_tail: tail(&stderr, STDERR_TAIL_BYTES),
    }
}

/// The lines of the child's standard error that name an exception, an
/// assertion or a warning: platform libraries state their thread rules there,
/// and the tail alone may hold only a stack trace.
fn notable_lines(stderr: &str) -> String {
    let markers = ["exception", "reason", "assert", "warning", "only", "thread"];
    let mut notes = String::new();
    for line in stderr.lines() {
        let lower = line.to_ascii_lowercase();
        if markers.iter().any(|marker| lower.contains(marker)) {
            if !notes.is_empty() {
                notes.push('\n');
            }
            notes.push_str(line.trim());
            if notes.len() > STDERR_TAIL_BYTES {
                break;
            }
        }
    }
    notes
}

fn drain(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> (Option<std::process::ExitStatus>, bool) {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (Some(status), false),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let status = child.wait().ok();
                return (status, true);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => return (None, false),
        }
    }
}

#[cfg(unix)]
fn describe_exit(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "unknown".to_string(),
    }
}

#[cfg(windows)]
fn describe_exit(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("code {code} (0x{:08x})", code as u32),
        None => "unknown".to_string(),
    }
}

/// Parses the child's report line: `{"outcome":"...","details":{"k":"v",...}}`.
/// The writer is ours, so the parser handles that shape and nothing else.
fn parse_report(stdout: &str) -> Option<(String, Vec<(String, String)>)> {
    let line = stdout.lines().find(|line| line.starts_with("{\"outcome\":"))?;
    let mut fields = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let (key, after_key) = read_json_string(&rest[start..])?;
        let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
        if after_colon.starts_with('{') {
            rest = &after_colon[1..];
            continue;
        }
        let (value, after_value) = read_json_string(after_colon)?;
        fields.push((key, value));
        rest = after_value;
    }
    let outcome = fields.iter().find(|(key, _)| key == "outcome")?.1.clone();
    let details = fields.into_iter().filter(|(key, _)| key != "outcome").collect();
    Some((outcome, details))
}

/// Reads one JSON string starting at the opening quote; returns it unescaped
/// with the remainder of the text.
fn read_json_string(text: &str) -> Option<(String, &str)> {
    let mut characters = text.strip_prefix('"')?.char_indices();
    let mut value = String::new();
    while let Some((index, character)) = characters.next() {
        match character {
            '"' => return Some((value, &text[index + 2..])),
            '\\' => {
                let (_, escaped) = characters.next()?;
                match escaped {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    'u' => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            let (_, digit) = characters.next()?;
                            code = code * 16 + digit.to_digit(16)?;
                        }
                        value.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    other => value.push(other),
                }
            }
            other => value.push(other),
        }
    }
    None
}

fn report_json(report: &Report) -> String {
    let mut out = Json::new();
    out.object_start();
    out.string("outcome", report.outcome);
    out.key("details");
    out.object_start();
    for (key, value) in &report.details {
        out.string(key, value);
    }
    out.object_end();
    out.object_end();
    out.finish()
}

fn tail(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.trim_end().to_string();
    }
    let mut start = text.len() - bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", text[start..].trim_end())
}

/// Runs the diagnostic probes through the runner and checks that the parent
/// classifies a clean report, an abort, a hang and a panic correctly.
fn self_check() {
    let executable = std::env::current_exe().expect("the spike knows its own path");
    let expectations = [
        ("noop", "succeeded"),
        ("crash", "crashed"),
        ("hang", "timed out"),
        ("panic", "panicked"),
    ];
    assert_eq!(DIAGNOSTIC_PROBES.len(), expectations.len());
    let mut failures = 0;
    for (probe, expected) in expectations {
        let run = run_in_child(&executable, probe, &[], SELF_CHECK_TIMEOUT);
        let verdict = if run.outcome == expected { "ok" } else { "MISMATCH" };
        if verdict != "ok" {
            failures += 1;
        }
        println!(
            "self-check: {probe}: outcome {} (expected {expected}), exit {}, {} ms: {verdict}",
            run.outcome, run.exit, run.duration_ms
        );
    }
    let noop = run_in_child(&executable, "noop", &[], SELF_CHECK_TIMEOUT);
    if noop.details != vec![("thread".to_string(), "child main".to_string())] {
        failures += 1;
        println!("self-check: noop details MISMATCH: {:?}", noop.details);
    }
    if failures == 0 {
        println!("self-check: ok");
    } else {
        println!("self-check: {failures} failure(s)");
        std::process::exit(1);
    }
}

/// Minimal JSON writer, enough for the fact sheet (copied, as every spike
/// carries its own; spikes share nothing but the workspace).
struct Json {
    text: String,
    needs_separator: Vec<bool>,
}

impl Json {
    fn new() -> Self {
        Json {
            text: String::new(),
            needs_separator: vec![false],
        }
    }

    fn object_start(&mut self) {
        self.text.push('{');
        self.needs_separator.push(false);
    }

    fn object_end(&mut self) {
        self.text.push('}');
        self.needs_separator.pop();
        self.mark_separator();
    }

    fn array_start(&mut self) {
        self.text.push('[');
        self.needs_separator.push(false);
    }

    fn array_end(&mut self) {
        self.text.push(']');
        self.needs_separator.pop();
        self.mark_separator();
    }

    fn key(&mut self, key: &str) {
        self.separator();
        self.quoted(key);
        self.text.push(':');
        if let Some(last) = self.needs_separator.last_mut() {
            *last = false;
        }
    }

    fn string(&mut self, key: &str, value: &str) {
        self.key(key);
        self.quoted(value);
        self.mark_separator();
    }

    fn u64(&mut self, key: &str, value: u64) {
        self.key(key);
        self.text.push_str(&value.to_string());
        self.mark_separator();
    }

    fn bool(&mut self, key: &str, value: bool) {
        self.key(key);
        self.text.push_str(if value { "true" } else { "false" });
        self.mark_separator();
    }

    fn separator(&mut self) {
        if self.needs_separator.last().copied().unwrap_or(false) {
            self.text.push(',');
        }
    }

    fn mark_separator(&mut self) {
        if let Some(last) = self.needs_separator.last_mut() {
            *last = true;
        }
    }

    fn quoted(&mut self, text: &str) {
        self.text.push('"');
        for character in text.chars() {
            match character {
                '"' => self.text.push_str("\\\""),
                '\\' => self.text.push_str("\\\\"),
                '\n' => self.text.push_str("\\n"),
                '\r' => self.text.push_str("\\r"),
                '\t' => self.text.push_str("\\t"),
                c if (c as u32) < 0x20 => self.text.push_str(&format!("\\u{:04x}", c as u32)),
                c => self.text.push(c),
            }
        }
        self.text.push('"');
    }

    fn finish(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_round_trips_through_the_child_protocol() {
        let report = Report::new("succeeded")
            .detail("width", 640)
            .detail("note", "quote \" and \\ backslash\nnew line");
        let line = report_json(&report);
        let (outcome, details) = parse_report(&format!("noise before\n{line}\n")).expect("parsed");
        assert_eq!(outcome, "succeeded");
        assert_eq!(details[0], ("width".to_string(), "640".to_string()));
        assert_eq!(
            details[1],
            ("note".to_string(), "quote \" and \\ backslash\nnew line".to_string())
        );
    }

    #[test]
    fn a_report_without_the_marker_is_no_report() {
        assert!(parse_report("thread 'main' panicked at ...").is_none());
        assert!(parse_report("").is_none());
    }

    #[test]
    fn json_writer_separates_and_escapes() {
        let mut out = Json::new();
        out.object_start();
        out.string("a", "x\"y");
        out.u64("b", 2);
        out.key("c");
        out.array_start();
        out.separator();
        out.object_start();
        out.bool("d", true);
        out.object_end();
        out.array_end();
        out.object_end();
        assert_eq!(out.finish(), "{\"a\":\"x\\\"y\",\"b\":2,\"c\":[{\"d\":true}]}");
    }

    #[test]
    fn tail_keeps_the_end_on_a_character_boundary() {
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(tail("héllo world", 5), "...world");
    }
}
