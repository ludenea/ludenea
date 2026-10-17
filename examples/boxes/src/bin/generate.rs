//! Writes the scenario assets: the scene and the 10,000-tick input log.

use std::fs;
use std::path::Path;

use boxes::{BoxesInput, generate_scene};
use ludenea::prelude::{InputLog, Vec2};

const TICKS: u32 = 10_000;

fn input(tick: u32) -> BoxesInput {
    BoxesInput {
        velocity_change: tick.is_multiple_of(120).then_some(Vec2::new(1.0, 4.0)),
        spawn: tick
            .is_multiple_of(500)
            .then_some(((tick / 500) % 30) as f32 - 15.0),
        despawn: tick.is_multiple_of(700),
    }
}

fn main() {
    let assets = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));
    fs::create_dir_all(assets).expect("cannot create the assets directory");
    fs::write(assets.join("boxes.scene.ron"), generate_scene(300, 7))
        .expect("cannot write the scene");
    let mut log = InputLog::new(1);
    for tick in 0..TICKS {
        log.record(&[input(tick)]);
    }
    fs::write(assets.join("boxes.inputs"), log.encode()).expect("cannot write the input log");
}
