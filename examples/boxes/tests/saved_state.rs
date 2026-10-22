//! Saving the simulation every tick and loading a past tick resimulates the same states.

use std::fs;

use ludenea::app::SavedState;
use ludenea::prelude::*;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.scene.ron");
const INPUTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.inputs");
const TICKS: usize = 300;
const LOADED: usize = 150;

fn saved_state_matches_snapshot_at(threads: usize) {
    let mut simulation = boxes::app().build().unwrap();
    simulation
        .load_scene(&fs::read_to_string(SCENE).unwrap())
        .unwrap();
    simulation.set_threads(threads).unwrap();
    let log = InputLog::decode(&fs::read(INPUTS).unwrap()).unwrap();
    let inputs = |tick: usize| log.tick(u64::try_from(tick).unwrap());

    let mut states = Vec::with_capacity(TICKS);
    let mut straight = Vec::with_capacity(TICKS + 1);
    for tick in 0..TICKS {
        let mut state = SavedState::default();
        simulation.save_state(&mut state);
        states.push(state);
        straight.push(simulation.snapshot().encode());
        simulation.step_encoded(inputs(tick)).unwrap();
    }
    straight.push(simulation.snapshot().encode());

    simulation.load_state(&states[LOADED]).unwrap();
    for (tick, expected) in straight.iter().enumerate().take(TICKS).skip(LOADED) {
        assert!(
            simulation.snapshot().encode() == *expected,
            "{threads} threads: tick {tick} differs after loading tick {LOADED}"
        );
        simulation.step_encoded(inputs(tick)).unwrap();
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
