//! Writes the editor's performance scene to `target/editor-bench.scene.ron` (under
//! `CARGO_TARGET_DIR` when it is set): the container of
//! the reference scenario (4 walls) with 1,000 crates, plus 98,996 entities holding only a
//! `Pose` (on a grid under the ground): 100,000 entities. Engine components and resources
//! only, so the scene loads in a project made from the template.

use std::fs;
use std::path::{Path, PathBuf};

use boxes::spawn_container;
use ludenea::prelude::*;

const BODIES: usize = 1000;
const PLAIN_ENTITIES: usize = 100_000 - 4 - BODIES;
const COLUMNS: usize = 330;
const SPACING: f32 = 0.5;

fn main() {
    let mut app = App::new();
    app.plugin(PhysicsPlugin);
    let mut simulation = app.build().expect("the physics app builds");
    let world = simulation.world_mut();
    spawn_container(world, BODIES, 11);
    for index in 0..PLAIN_ENTITIES {
        let column = (index % COLUMNS) as f32;
        let row = (index / COLUMNS) as f32;
        let pose = Pose {
            position: Vec2::new(
                (column - 0.5 * COLUMNS as f32) * SPACING,
                -2.0 - row * SPACING,
            ),
            rotation: Rot::IDENTITY,
        };
        let entity = world.spawn();
        world.insert(entity, pose).expect("Pose is registered");
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let path = target.join("editor-bench.scene.ron");
    fs::create_dir_all(&target).expect("cannot create the target directory");
    let scene = simulation
        .save_scene()
        .expect("100,000 entities leave local identifiers to spare");
    fs::write(&path, scene).expect("cannot write the scene");
    println!("wrote {}", path.display());
}
