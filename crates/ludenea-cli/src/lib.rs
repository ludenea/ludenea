//! The `ludenea` command line: `run`, `verify`, `synctest`, `diff`, `render`, `play`,
//! `host`, and `peer`. A game provides its own `main` and passes its app constructor to [`main`].

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
    /// Run a scene or replay next to a copy that mispredicts and rolls back every tick, and
    /// report the first tick, entities, and fields where they differ.
    Synctest(commands::SyncTestArgs),
    /// List the differences between two snapshots.
    Diff(commands::DiffArgs),
    /// Draw a tick of a scene or replay to a PNG, offscreen on a GPU adapter.
    #[cfg(not(target_family = "wasm"))]
    Render(commands::RenderArgs),
    /// Play the game in a window: a lobby where local players claim their keyboard half or
    /// gamepad (unless --players is given), then the match.
    #[cfg(not(target_family = "wasm"))]
    Play(commands::PlayArgs),
    /// Serve the game to the editor over stdin and stdout.
    #[cfg(not(target_family = "wasm"))]
    Host(commands::HostArgs),
    /// Play a match online without a window: host or join a room of a signaling listener
    /// (`ludenea rooms`), then play it in real time with no input or seeded random input.
    #[cfg(not(target_family = "wasm"))]
    Peer(commands::PeerArgs),
}

/// The name the game's command line runs under, as its messages name it: the file stem of
/// the first argument.
#[cfg(not(target_family = "wasm"))]
fn program_name<S: AsRef<str>>(args: &[S]) -> String {
    args.first()
        .and_then(|program| std::path::Path::new(program.as_ref()).file_stem())
        .map_or_else(
            || "game".to_string(),
            |stem| stem.to_string_lossy().into_owned(),
        )
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
        Cmd::Synctest(args) => commands::synctest(app, args, out),
        Cmd::Diff(args) => commands::diff(app, args, out),
        #[cfg(not(target_family = "wasm"))]
        Cmd::Render(args) => commands::render(app, args, out),
        #[cfg(not(target_family = "wasm"))]
        Cmd::Play(args) => commands::play(app, args, out),
        #[cfg(not(target_family = "wasm"))]
        Cmd::Host(args) => commands::host(app, args),
        #[cfg(not(target_family = "wasm"))]
        Cmd::Peer(peer) => commands::peer(app, peer, &program_name(args), out),
    };
    outcome.unwrap_or_else(|message| {
        let _ = writeln!(out, "error: {message}");
        2
    })
}
