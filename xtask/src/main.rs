//! Repository automation, run with `cargo xtask <command>`.

mod boundaries;
mod commit_format;
#[cfg(test)]
mod determinism_lints;
mod golden_compare;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::Path;
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <ci|boundaries|commit-format|doc|golden-hash [cargo run arguments]|golden-compare <directory> <target>...>";

/// The golden hash every target must produce, relative to the workspace root.
const GOLDEN_REFERENCE: &str = "crates/test/golden-hash.txt";

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("ci") => ci(),
        Some("boundaries") => to_exit_code(check_boundaries()),
        Some("commit-format") => to_exit_code(check_commit_format()),
        Some("doc") => to_exit_code(cargo(&DOC)),
        Some("golden-hash") => to_exit_code(golden_hash()),
        Some("golden-compare") => to_exit_code(golden_compare()),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the same checks as the continuous integration workflow.
fn ci() -> ExitCode {
    let steps: [&[&str]; 5] = [
        &["fmt", "--all", "--check"],
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        &["nextest", "run", "--workspace"],
        &DOC,
        &["deny", "check"],
    ];
    if !check_boundaries() {
        return ExitCode::FAILURE;
    }
    for arguments in steps {
        if !cargo(arguments) {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Builds the API documentation of the library crates, failing on any
/// rustdoc warning such as a broken intra-documentation link.
const DOC: [&str; 7] = [
    "doc",
    "--workspace",
    "--exclude",
    "xtask",
    "--lib",
    "--no-deps",
    "--config=build.rustdocflags=[\"-D\", \"warnings\"]",
];

fn cargo(arguments: &[&str]) -> bool {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!("$ cargo {}", arguments.join(" "));
    Command::new(cargo)
        .args(arguments)
        .status()
        .is_ok_and(|status| status.success())
}

/// Fails when a core crate reaches a crate the hexagonal boundary forbids.
fn check_boundaries() -> bool {
    eprintln!("$ cargo xtask boundaries");
    let (graph, members) = match boundaries::load() {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("error: cannot read the workspace metadata: {error}");
            return false;
        }
    };
    let violations = boundaries::violations(&graph, &members);
    for violation in &violations {
        eprintln!("error: {violation}");
    }
    violations.is_empty()
}

/// Fails when a subject read from standard input, one per line, breaks the
/// commit format. Used on commit subjects and pull request titles.
fn check_commit_format() -> bool {
    let mut success = true;
    for line in io::stdin().lock().lines() {
        let subject = match line {
            Ok(subject) => subject,
            Err(error) => {
                eprintln!("error: cannot read standard input: {error}");
                return false;
            }
        };
        if subject.is_empty() {
            continue;
        }
        if let Some(problem) = commit_format::problem(&subject) {
            eprintln!("error: \"{subject}\": {problem}");
            success = false;
        }
    }
    success
}

/// Prints the golden hash of the target selected by the cargo run arguments,
/// such as `--target wasm32-wasip1`, the host by default.
fn golden_hash() -> bool {
    let mut arguments = vec![
        "run",
        "--quiet",
        "--package",
        "ludenea_test",
        "--bin",
        "golden-hash",
    ];
    let forwarded: Vec<String> = env::args().skip(2).collect();
    arguments.extend(forwarded.iter().map(String::as_str));
    cargo(&arguments)
}

/// Fails when a report in the directory given as argument, one `<target>.txt`
/// file per target, holds a hash differing from the committed reference, or
/// when a target given after the directory has no report.
fn golden_compare() -> bool {
    let arguments: Vec<String> = env::args().skip(2).collect();
    let Some((directory, expected)) = arguments
        .split_first()
        .filter(|(_, expected)| !expected.is_empty())
    else {
        eprintln!("{USAGE}");
        return false;
    };
    let directory = Path::new(directory);
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace");
    let read = |path: &Path| match fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", path.display());
            None
        }
    };
    let Some(reference) = read(&workspace_root.join(GOLDEN_REFERENCE)) else {
        return false;
    };
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", directory.display());
            return false;
        }
    };
    let mut reports = BTreeMap::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                eprintln!("error: cannot read {}: {error}", directory.display());
                return false;
            }
        };
        if path.extension().is_some_and(|extension| extension == "txt") {
            let Some(report) = read(&path) else {
                return false;
            };
            let target = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            eprintln!(
                "{target}: {}",
                report.split_whitespace().collect::<Vec<_>>().join(" ")
            );
            reports.insert(target, report);
        }
    }
    let problems = golden_compare::problems(reference.trim(), expected, &reports);
    for problem in &problems {
        eprintln!("error: {problem}");
    }
    problems.is_empty()
}

fn to_exit_code(success: bool) -> ExitCode {
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
