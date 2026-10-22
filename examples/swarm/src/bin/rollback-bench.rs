//! Times the worst case of a rollback frame on swarm, a scene of 10,000 sprites and 4 players:
//! load the state eight ticks back, then replay nine ticks saving the state after each. Swarm
//! has no golden replay, so a straight run with seeded inputs gives the hash every frame must
//! end on. Prints the distribution over every such frame at 1 thread; it records numbers and
//! never fails on them.

use std::path::Path;
use std::process::ExitCode;

use ludenea::app::assets::asset_file;
use ludenea::app::{AssetTexts, Rng, Simulation};
use ludenea::ecs::AssetId;
use swarm::{BUG_SHEET, PLAYERS, SwarmInput};

const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
/// Ticks played straight.
const TICKS: u64 = 600;
const SEED: u64 = 0x5741_524d;

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

fn read(name: &str) -> Result<String, String> {
    let path = Path::new(ASSETS).join(name);
    std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// A swarm simulation at tick 0 with its scene loaded, 4 players, 1 thread.
fn loaded(scene: &str, sheet: &str) -> Result<Simulation, String> {
    let mut simulation = swarm::app()
        .build()
        .map_err(|error| format!("the swarm app does not build: {error}"))?;
    let id = AssetId(
        u128::from_str_radix(BUG_SHEET, 16).map_err(|error| format!("bad sheet id: {error}"))?,
    );
    let assets = AssetTexts::from([(id, asset_file("ludenea::SpriteSheet", sheet))]);
    simulation
        .load_scene_with(scene, &assets)
        .map_err(|error| format!("cannot load the swarm scene: {error}"))?;
    simulation.set_players(PLAYERS);
    simulation
        .set_threads(1)
        .map_err(|error| error.to_string())?;
    Ok(simulation)
}

/// One input per player per tick, each direction of length 1 at most.
fn seeded_inputs(ticks: u64) -> Vec<Vec<SwarmInput>> {
    let mut rng = Rng::new(SEED, 0);
    (0..ticks)
        .map(|_| {
            (0..PLAYERS)
                .map(|_| {
                    let mut direction = ludenea::prelude::Vec2::new(
                        rng.range_f32(-1.0, 1.0),
                        rng.range_f32(-1.0, 1.0),
                    );
                    if direction.length_squared() > 1.0 {
                        direction = direction.normalize();
                    }
                    SwarmInput { direction }
                })
                .collect()
        })
        .collect()
}

fn run() -> Result<(), String> {
    let scene = read("swarm.scene.ron")?;
    let sheet = read("bug.sheet.ron")?;
    let inputs = seeded_inputs(TICKS);
    let play = |simulation: &mut Simulation, tick: u64| {
        let tick = usize::try_from(tick).map_err(|error| error.to_string())?;
        simulation
            .step_with(&inputs[tick])
            .map_err(|error| error.to_string())
    };

    // The straight run: the hash after every tick.
    let mut straight = loaded(&scene, &sheet)?;
    let mut hashes = vec![straight.hash()];
    for tick in 0..TICKS {
        play(&mut straight, tick)?;
        hashes.push(straight.hash());
    }

    let mut simulation = loaded(&scene, &sheet)?;
    rollback_bench::run(&mut simulation, &hashes, "the straight run", play)?.print();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_inputs_are_canonical_inputs_of_swarm() {
        let ticks = seeded_inputs(300);
        assert_eq!(ticks.len(), 300);
        for inputs in &ticks {
            assert_eq!(inputs.len(), PLAYERS);
            let bytes = postcard::to_allocvec(inputs).expect("inputs encode");
            let decoded: Vec<SwarmInput> = postcard::from_bytes(&bytes).expect("inputs decode");
            assert_eq!(postcard::to_allocvec(&decoded).expect("re-encode"), bytes);
            for input in inputs {
                assert!(input.direction.length_squared() <= 1.0 + 1e-6);
            }
        }
    }
}
