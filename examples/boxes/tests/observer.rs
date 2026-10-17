//! Observation must never change the simulation: hashes with and without an observer match.

use std::fs;

use ludenea::ecs::{CommandRecord, ScheduleObserver};
use ludenea::prelude::*;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.scene.ron");
const INPUTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.inputs");
const TICKS: u64 = 600;

#[derive(Default)]
struct Counting {
    systems: usize,
    commands: usize,
    ticks: usize,
}

impl ScheduleObserver for Counting {
    fn before_system(&mut self, _: usize) {
        self.systems += 1;
    }

    fn after_system(&mut self, _: usize, commands: &[CommandRecord]) {
        self.commands += commands.len();
    }

    fn tick_end(&mut self, world: &World) {
        // Reading events and hashing mid-tick must not disturb anything either.
        let _ = world.events_json();
        let _ = world.hash();
        self.ticks += 1;
    }
}

fn hashes(mut observer: Option<&mut Counting>) -> Vec<u64> {
    let mut simulation = boxes::app().build().unwrap();
    simulation
        .load_scene(&fs::read_to_string(SCENE).unwrap())
        .unwrap();
    let log = InputLog::decode(&fs::read(INPUTS).unwrap()).unwrap();
    (0..TICKS)
        .map(|tick| {
            match observer.as_deref_mut() {
                Some(observer) => simulation.step_observed(log.tick(tick), observer),
                None => simulation.step_encoded(log.tick(tick)),
            }
            .unwrap();
            simulation.hash()
        })
        .collect()
}

#[test]
fn unobserved_run_hashes_equal_observed_run_hashes() {
    let mut observer = Counting::default();
    let observed = hashes(Some(&mut observer));
    assert_eq!(hashes(None), observed);
    assert_eq!(observer.ticks, TICKS as usize);
    assert!(observer.systems > 0 && observer.commands > 0);
}
