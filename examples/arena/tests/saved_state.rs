//! Saving the simulation every tick and loading a past tick resimulates the same states.

use std::path::Path;

use arena::ArenaInput;
use ludenea::app::SavedState;
use ludenea::app::project::project_assets;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/arena.scene.ron");
const TICKS: usize = 300;
const LOADED: usize = 150;
const PLAYERS: usize = 2;

/// Each player runs one way then the other and jumps now and then, so fighters move, land,
/// and meet.
fn inputs(tick: usize) -> Vec<ArenaInput> {
    (0..PLAYERS)
        .map(|player| {
            let phase = (tick + player * 40) % 120;
            ArenaInput {
                run: if phase < 60 { 1.0 } else { -1.0 },
                jump: (tick + player * 13) % 45 < 2,
            }
        })
        .collect()
}

fn saved_state_matches_snapshot_at(threads: usize) {
    let scene = std::fs::read_to_string(SCENE).unwrap();
    let assets = project_assets(Path::new(SCENE), None).unwrap();
    let mut simulation = arena::app().build().unwrap();
    simulation
        .load_scene_with(&scene, &assets)
        .unwrap_or_else(|error| panic!("{error}"));
    simulation.set_players(PLAYERS);
    simulation.set_threads(threads).unwrap();

    let mut states = Vec::with_capacity(TICKS);
    let mut straight = Vec::with_capacity(TICKS + 1);
    for tick in 0..TICKS {
        let mut state = SavedState::default();
        simulation.save_state(&mut state);
        states.push(state);
        straight.push(simulation.snapshot().encode());
        simulation.step_with(&inputs(tick)).unwrap();
    }
    straight.push(simulation.snapshot().encode());

    simulation.load_state(&states[LOADED]).unwrap();
    for (tick, expected) in straight.iter().enumerate().take(TICKS).skip(LOADED) {
        assert!(
            simulation.snapshot().encode() == *expected,
            "{threads} threads: tick {tick} differs after loading tick {LOADED}"
        );
        simulation.step_with(&inputs(tick)).unwrap();
    }
    assert!(
        simulation.snapshot().encode() == straight[TICKS],
        "{threads} threads: tick {TICKS} differs after loading tick {LOADED}"
    );
}

#[test]
fn saved_state_matches_snapshot() {
    for threads in [1, 8] {
        saved_state_matches_snapshot_at(threads);
    }
}
