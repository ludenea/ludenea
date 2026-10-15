//! Repository automation, run with `cargo xtask <command>`.

mod boundaries;

use std::env;
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <ci|boundaries>";

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("ci") => ci(),
        Some("boundaries") => to_exit_code(check_boundaries()),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the same checks as the continuous integration workflow.
fn ci() -> ExitCode {
    let steps: [&[&str]; 3] = [
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

fn to_exit_code(success: bool) -> ExitCode {
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
