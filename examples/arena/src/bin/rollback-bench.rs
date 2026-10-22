//! Times the worst case of a rollback frame on the arena's golden replay: load the state
//! eight ticks back, then replay nine ticks saving the state after each. Prints the
//! distribution over every such frame at 1 thread; it records numbers and never fails on them.

use std::path::Path;
use std::process::ExitCode;

use ludenea::app::replay::{Replay, load_replay, play_tick};

const REPLAY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/arena.replay");

#[path = "../../../rollback_bench.rs"]
mod rollback_bench;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let path = Path::new(REPLAY);
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let replay = Replay::decode(&bytes).map_err(|error| error.to_string())?;
    let mut simulation = arena::app()
        .build()
        .map_err(|error| format!("the arena app does not build: {error}"))?;
    simulation
        .set_threads(1)
        .map_err(|error| error.to_string())?;
    let (inputs, edits) =
        load_replay(&mut simulation, &replay).map_err(|error| error.to_string())?;
    let report = rollback_bench::run(
        &mut simulation,
        &replay.hashes,
        "the golden replay",
        |simulation, tick| {
            play_tick(simulation, &inputs, &edits, tick).map_err(|error| error.to_string())
        },
    )?;
    report.print();
    Ok(())
}
