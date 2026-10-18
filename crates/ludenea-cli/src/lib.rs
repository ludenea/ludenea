//! The `ludenea` command line: `run`, `verify`, `diff`, and `host`. A game provides its own
//! `main` and passes its app constructor to [`main`].

pub mod assets;
mod commands;
mod trace;

use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ludenea_app::App;

pub use trace::{TRACE_VERSION, Trace};

#[derive(Parser)]
#[command(about = "Run, verify, and diff deterministic simulations")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a scene for a number of ticks.
    Run(commands::RunArgs),
    /// Replay a run and compare its hashes with a golden trace, or verify a replay.
    Verify(commands::VerifyArgs),
    /// List the differences between two snapshots.
    Diff(commands::DiffArgs),
    /// Serve the game to the editor over stdin and stdout.
    #[cfg(not(target_family = "wasm"))]
    Host(commands::HostArgs),
}

pub fn main(app: fn() -> App) -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let code = run(app, &args, &mut std::io::stdout());
    ExitCode::from(u8::try_from(code).unwrap_or(2))
}

/// Runs the command line and returns the exit code; the first argument is the program name.
pub fn run<S: AsRef<str>>(app: fn() -> App, args: &[S], out: &mut dyn Write) -> i32 {
    let cli = match Cli::try_parse_from(args.iter().map(AsRef::as_ref)) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = write!(out, "{error}");
            return error.exit_code();
        }
    };
    let outcome = match &cli.command {
        Cmd::Run(args) => commands::run(app, args, out),
        Cmd::Verify(args) => commands::verify(app, args, out),
        Cmd::Diff(args) => commands::diff(app, args, out),
        #[cfg(not(target_family = "wasm"))]
        Cmd::Host(args) => commands::host(app, args),
    };
    outcome.unwrap_or_else(|message| {
        let _ = writeln!(out, "error: {message}");
        2
    })
}
