//! Saving the simulation every tick and loading a past tick resimulates the same states.

use std::path::Path;

use arena::ArenaInput;
use ludenea::app::SavedState;
use ludenea::app::project::project_assets;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/arena.scene.ron");
const TICKS: usize = 300;
const LOADED: usize = 150;
const RING: usize = 8;
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

    // A rollback ring reused every tick, as a session keeps it, and tick LOADED in a slot
    // of its own so it survives the later ticks.
    let mut ring: Vec<SavedState> = (0..RING).map(|_| SavedState::default()).collect();
    let mut loaded = SavedState::default();
    let mut straight = Vec::with_capacity(TICKS + 1);
    for tick in 0..TICKS {
        let slot = if tick == LOADED {
            &mut loaded
        } else {
            &mut ring[tick % RING]
        };
        simulation.save_state(slot);
        straight.push(simulation.snapshot().encode());
        simulation.step_with(&inputs(tick)).unwrap();
    }
    straight.push(simulation.snapshot().encode());

    // The last ring slot was saved into many times before tick TICKS - 1.
    simulation.load_state(&ring[(TICKS - 1) % RING]).unwrap();
    assert!(
        simulation.snapshot().encode() == straight[TICKS - 1],
        "{threads} threads: the reused ring slot of tick {} differs",
        TICKS - 1
    );

    simulation.load_state(&loaded).unwrap();
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
