//! Times the worst case of a rollback frame on the arena's golden replay: load the state
//! eight ticks back, then replay nine ticks saving the state after each. Prints the
//! distribution over every such frame at 1 thread; it records numbers and never fails on them.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use ludenea::app::SavedState;
use ludenea::app::replay::{Replay, load_replay, play_tick};

const REPLAY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/arena.replay");
/// The rollback window: a frame loads the state this many ticks back.
const WINDOW: u64 = 8;
/// The saved states a session keeps: the window, the current tick, and one to spare.
const RING: usize = 10;
/// Half a 60 Hz frame, in microseconds.
const BUDGET_MICROS: u128 = 8_333;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "benchmark timing, never feeds the simulation"
)]
fn now() -> Instant {
    Instant::now()
}

fn slot(tick: u64) -> usize {
    (tick % RING as u64) as usize
}

fn run() -> Result<(), String> {
    let path = Path::new(REPLAY);
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let replay = Replay::decode(&bytes).map_err(|error| error.to_string())?;
    let total = (replay.hashes.len() as u64).saturating_sub(1);
    if total <= WINDOW {
        return Err(format!(
            "the replay records {total} ticks: a rollback frame needs more than {WINDOW}"
        ));
    }
    let mut simulation = arena::app()
        .build()
        .map_err(|error| format!("the arena app does not build: {error}"))?;
    simulation
        .set_threads(1)
        .map_err(|error| error.to_string())?;
    let (inputs, edits) =
        load_replay(&mut simulation, &replay).map_err(|error| error.to_string())?;

    let play = |simulation: &mut _, tick| {
        play_tick(simulation, &inputs, &edits, tick).map_err(|error| error.to_string())
    };
    // Every slot holds a state before the first measure, so the timed frames reuse
    // allocations as a running session does.
    let mut ring: Vec<SavedState> = (0..RING).map(|_| SavedState::default()).collect();
    for saved in &mut ring {
        simulation.save_state(saved);
    }
    let mut frames = Vec::new();
    for tick in 0..total {
        simulation.save_state(&mut ring[slot(tick)]);
        if tick < WINDOW {
            play(&mut simulation, tick)?;
            continue;
        }
        // The frame ends at tick + 1, where straight play would be after this tick.
        let start = now();
        simulation
            .load_state(&ring[slot(tick - WINDOW)])
            .map_err(|error| error.to_string())?;
        for replayed in tick - WINDOW..=tick {
            play(&mut simulation, replayed)?;
            simulation.save_state(&mut ring[slot(replayed + 1)]);
        }
        let elapsed = start.elapsed().as_micros();
        // A wrong load must not produce a timing: the frame ends on the golden hash.
        let golden = replay.hashes[(tick + 1) as usize];
        if simulation.hash() != golden {
            return Err(format!(
                "tick {} differs from the golden replay after a rollback frame",
                tick + 1
            ));
        }
        frames.push(elapsed);
    }

    frames.sort_unstable();
    let count = frames.len();
    let mean = frames.iter().sum::<u128>() / count as u128;
    let p99 = frames[(count * 99).div_ceil(100) - 1];
    let worst = frames[count - 1];
    println!("{count} frames (1 thread): mean {mean} us, p99 {p99} us, worst {worst} us");
    let verdict = if worst <= BUDGET_MICROS {
        "fits"
    } else {
        "exceeds"
    };
    println!("worst frame {verdict} half a 60 Hz frame ({BUDGET_MICROS} us)");
    Ok(())
}
