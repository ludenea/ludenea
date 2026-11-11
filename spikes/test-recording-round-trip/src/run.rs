//! The run's directory: the start marker and the record every test writes,
//! and the assembly of a report from a listing and the directory. Throwaway,
//! never product code.
//!
//! A test of the standard harness passes its name and its body to [`entry`],
//! which writes the start marker before the body runs, runs the body, writes
//! the record whole under another name and puts it in place by one rename,
//! then removes the marker. A test started and never finished is therefore
//! told from a test never started, whatever ended its process.
//!
//! The assembly is an ordered list of rules over a listing and the directory:
//! for each listed test the first rule that fits applies, so exactly one does.

use std::fs;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

/// The directory the tests write their markers and records in.
pub const RUN_DIR_VARIABLE: &str = "RECORDING_ROUND_TRIP_RUN_DIR";
/// When set to a count, the tests that start after that many have started
/// say so on standard error and hold, their markers written and no record:
/// the state a process ended from outside leaves.
pub const HOLD_VARIABLE: &str = "RECORDING_ROUND_TRIP_HOLD_AFTER";
/// The label of the machine, written in a recording's header.
pub const CELL_VARIABLE: &str = "RECORDING_ROUND_TRIP_CELL";

const MARKER_SUFFIX: &str = ".started";
const RECORD_SUFFIX: &str = ".record";

/// What a test's body reports when it returns: where its world stood and the
/// files it left in the run's directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub final_tick: u32,
    pub final_hash: u64,
    pub artifacts: Vec<String>,
}

/// A body's failure: its message and what the test reports all the same.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    pub message: String,
    pub report: Report,
}

pub fn run_dir() -> PathBuf {
    let dir = std::env::var_os(RUN_DIR_VARIABLE)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("test-recording-round-trip-run-{}", std::process::id())));
    fs::create_dir_all(&dir).expect("the run's directory is creatable");
    dir
}

pub fn cell() -> String {
    std::env::var(CELL_VARIABLE).unwrap_or_else(|_| "unnamed".to_string())
}

pub fn profile() -> &'static str {
    if cfg!(debug_assertions) { "dev" } else { "release" }
}

/// One line on standard error, written in one call and outside the harness's
/// capture of a test's output: what the process that started this one reads
/// to know, independently of the directory, what every test did.
fn signal(line: &str) {
    let _ = std::io::stderr().lock().write_all(format!("{line}\n").as_bytes());
}

/// Writes a file whole under a temporary name and puts it in place by one
/// rename, so that a reader never sees half of it.
fn put_in_place(path: &Path, content: &str) {
    let temporary = path.with_extension("writing");
    fs::write(&temporary, content).expect("the record is writable");
    fs::rename(&temporary, path).expect("the record is put in place");
}

static STARTED: AtomicUsize = AtomicUsize::new(0);

/// The entry function of a test: the marker, the body, the record. A body
/// that fails or panics leaves a record that says so, and the test then
/// panics with the body's message so that the standard harness reports it.
pub fn entry(name: &str, body: impl FnOnce(&Path) -> Result<Report, Failed>) {
    let dir = run_dir();
    let marker = dir.join(format!("{name}{MARKER_SUFFIX}"));
    put_in_place(&marker, &format!("name={name}\nprocess={}\n", std::process::id()));
    let index = STARTED.fetch_add(1, Ordering::SeqCst);
    if let Some(hold_after) = std::env::var(HOLD_VARIABLE).ok().and_then(|value| value.parse::<usize>().ok())
        && index >= hold_after
    {
        signal(&format!("held {name}"));
        loop {
            thread::park();
        }
    }
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| body(&dir))).unwrap_or_else(|payload| {
        let message =
            payload.downcast_ref::<&str>().map(|s| (*s).to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_default();
        Err(Failed { message: format!("the body panicked: {message}"), report: Report::default() })
    });
    let (outcome, message, report) = match &result {
        Ok(report) => ("passed", "", report),
        Err(failed) => ("failed", failed.message.as_str(), &failed.report),
    };
    let mut record = format!(
        "name={name}\noutcome={outcome}\nmessage={}\nduration_ms={:.3}\nfinal_tick={}\nfinal_hash={:016x}\n",
        message.replace('\n', " "),
        started.elapsed().as_secs_f64() * 1e3,
        report.final_tick,
        report.final_hash
    );
    for artifact in &report.artifacts {
        record.push_str(&format!("artifact={artifact}\n"));
    }
    put_in_place(&dir.join(format!("{name}{RECORD_SUFFIX}")), &record);
    let _ = fs::remove_file(&marker);
    signal(&format!("finished {name} {outcome}"));
    if let Err(failed) = result {
        panic!("{}", failed.message);
    }
}

// ---------------------------------------------------------------------------
// The assembly
// ---------------------------------------------------------------------------

/// How the test binary of a run ended, as the runner saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryEnd {
    Succeeded,
    Failed,
    /// Ended from outside, or never started.
    Lost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Class {
    /// A completed record: its outcome, with the hash it reports.
    Recorded { outcome: String, final_hash: String },
    /// A marker and no record: an error of the lost process kind.
    Interrupted,
    /// Neither, its binary lost or never started.
    NotRun,
    /// Neither, its binary failed: an error of unknown outcome.
    UnknownOutcome,
    /// Neither, its binary succeeded.
    PassedUnrecorded,
}

impl Class {
    pub fn name(&self) -> String {
        match self {
            Class::Recorded { outcome, .. } => outcome.clone(),
            Class::Interrupted => "interrupted".to_string(),
            Class::NotRun => "not run".to_string(),
            Class::UnknownOutcome => "error of unknown outcome".to_string(),
            Class::PassedUnrecorded => "passed, unrecorded".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assembly {
    /// One entry per listed test inside the selection, in the listing's order.
    pub entries: Vec<(String, Class)>,
    /// The records and markers of the directory that match no listed test: an
    /// error of the report.
    pub unmatched: Vec<String>,
}

impl Assembly {
    pub fn class(&self, name: &str) -> Option<&Class> {
        self.entries.iter().find(|(n, _)| n == name).map(|(_, class)| class)
    }

    pub fn names(&self, wanted: impl Fn(&Class) -> bool) -> Vec<String> {
        self.entries.iter().filter(|(_, class)| wanted(class)).map(|(name, _)| name.clone()).collect()
    }
}

/// The value of `key=` in a record.
pub fn record_value<'a>(record: &'a str, key: &str) -> Option<&'a str> {
    record.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

/// The names of the tests in the standard harness's terse listing, in its order.
pub fn parse_listing(stdout: &str) -> Vec<String> {
    stdout.lines().filter_map(|line| line.strip_suffix(": test")).map(str::to_string).collect()
}

/// Assembles the report of a run. For each listed test the first rule that
/// fits applies: outside the selection, left out; a completed record, its
/// outcome; a marker and no record, interrupted; neither, by how the binary
/// ended. The rules on ignored tests and on a binary ended by the runner's
/// own bound are not built here.
pub fn assemble(listing: &[String], selection: Option<&[String]>, dir: &Path, end: BinaryEnd) -> Assembly {
    let mut entries = Vec::with_capacity(listing.len());
    for name in listing {
        if selection.is_some_and(|selection| !selection.contains(name)) {
            continue;
        }
        let record = fs::read_to_string(dir.join(format!("{name}{RECORD_SUFFIX}"))).ok();
        let marker = dir.join(format!("{name}{MARKER_SUFFIX}")).exists();
        let class = match (record, marker, end) {
            (Some(record), _, _) => Class::Recorded {
                outcome: record_value(&record, "outcome").unwrap_or("unreadable").to_string(),
                final_hash: record_value(&record, "final_hash").unwrap_or("").to_string(),
            },
            (None, true, _) => Class::Interrupted,
            (None, false, BinaryEnd::Lost) => Class::NotRun,
            (None, false, BinaryEnd::Failed) => Class::UnknownOutcome,
            (None, false, BinaryEnd::Succeeded) => Class::PassedUnrecorded,
        };
        entries.push((name.clone(), class));
    }
    let mut unmatched: Vec<String> = fs::read_dir(dir)
        .map(|read| {
            read.filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter_map(|file| file.strip_suffix(RECORD_SUFFIX).or_else(|| file.strip_suffix(MARKER_SUFFIX)).map(str::to_string))
                .filter(|name| !listing.contains(name))
                .collect()
        })
        .unwrap_or_default();
    unmatched.sort();
    unmatched.dedup();
    Assembly { entries, unmatched }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_rule_applies_to_every_listed_test() {
        let dir = std::env::temp_dir().join(format!("test-recording-round-trip-assembly-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let listing: Vec<String> = ["done", "both", "started", "absent", "outside"].iter().map(|s| (*s).to_string()).collect();
        fs::write(dir.join("done.record"), "name=done\noutcome=passed\nfinal_hash=00ff\n").unwrap();
        // A record and a marker left by an end between the rename and the removal: the record wins.
        fs::write(dir.join("both.record"), "name=both\noutcome=failed\nfinal_hash=0a\n").unwrap();
        fs::write(dir.join("both.started"), "name=both\n").unwrap();
        fs::write(dir.join("started.started"), "name=started\n").unwrap();
        fs::write(dir.join("stranger.record"), "name=stranger\noutcome=passed\n").unwrap();
        fs::write(dir.join("half.writing"), "name=half\n").unwrap();
        let selection: Vec<String> = listing[..4].to_vec();
        let assembly = assemble(&listing, Some(&selection), &dir, BinaryEnd::Lost);
        assert_eq!(assembly.entries.len(), 4, "the test outside the selection is left out");
        assert_eq!(assembly.class("done"), Some(&Class::Recorded { outcome: "passed".to_string(), final_hash: "00ff".to_string() }));
        assert_eq!(assembly.class("both").map(Class::name).as_deref(), Some("failed"));
        assert_eq!(assembly.class("started"), Some(&Class::Interrupted));
        assert_eq!(assembly.class("absent"), Some(&Class::NotRun));
        assert_eq!(assembly.unmatched, ["stranger"]);
        assert_eq!(assemble(&listing, None, &dir, BinaryEnd::Failed).class("absent"), Some(&Class::UnknownOutcome));
        assert_eq!(assemble(&listing, None, &dir, BinaryEnd::Succeeded).class("outside"), Some(&Class::PassedUnrecorded));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_terse_listing_gives_the_test_names_in_order() {
        let stdout = "t00_a: test\nt01_b: test\nsome_bench: benchmark\n\n2 tests, 1 benchmark\n";
        assert_eq!(parse_listing(stdout), ["t00_a", "t01_b"]);
    }
}
