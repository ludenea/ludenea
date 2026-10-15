//! Repository automation, run with `cargo xtask <command>`.

use std::env;
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask ci";

fn main() -> ExitCode {
    if env::args().nth(1).as_deref() == Some("ci") {
        ci()
    } else {
        eprintln!("{USAGE}");
        ExitCode::FAILURE
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
