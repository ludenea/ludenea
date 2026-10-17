use ludenea::prelude::*;

use crate::{Crate, app, crate_shape};

const HALF_THICKNESS: f32 = 0.5;
const HALF_WIDTH: f32 = 20.0;
const HEIGHT: f32 = 30.0;
const COLUMNS: usize = 20;
const SPACING: f32 = 1.4;
const FIRST_ROW: f32 = 2.0;
const JITTER: f32 = 0.1;

/// The RON scene of the scenario: a closed static container (ground at y = 0, ceiling at
/// y = 30, walls at x = -20 and x = 20) holding `crates` crates on a jittered grid, all
/// randomness drawn from `Rng::new(seed, 1)`.
pub fn generate_scene(crates: usize, seed: u64) -> String {
    let mut simulation = app().build().expect("the boxes app builds");
    let world = simulation.world_mut();
    for entity in spawn_container(world, crates, seed) {
        world.insert(entity, Crate).expect("Crate is registered");
    }
    simulation.save_scene()
}

/// The bodies of `generate_scene` with engine components only (no `Crate` marker); returns
/// the crates.
pub fn spawn_container(world: &mut World, crates: usize, seed: u64) -> Vec<Entity> {
    let outer = HALF_WIDTH + HALF_THICKNESS;
    let half_height = 0.5 * HEIGHT;
    for (center, half_extents) in [
        (Vec2::new(0.0, 0.0), Vec2::new(outer, HALF_THICKNESS)),
        (Vec2::new(0.0, HEIGHT), Vec2::new(outer, HALF_THICKNESS)),
        (
            Vec2::new(-HALF_WIDTH, half_height),
            Vec2::new(HALF_THICKNESS, half_height),
        ),
        (
            Vec2::new(HALF_WIDTH, half_height),
            Vec2::new(HALF_THICKNESS, half_height),
        ),
    ] {
        let shape = Shape::Polygon(Polygon::make_box(half_extents.x, half_extents.y));
        spawn(world, center, Body::fixed(), Collider::new(shape));
    }

    let mut rng = Rng::new(seed, 1);
    let first_column = -0.5 * (COLUMNS - 1) as f32 * SPACING;
    (0..crates)
        .map(|index| {
            let column = (index % COLUMNS) as f32;
            let row = (index / COLUMNS) as f32;
            let position = Vec2::new(
                first_column + column * SPACING + rng.range_f32(-JITTER, JITTER),
                FIRST_ROW + row * SPACING + rng.range_f32(-JITTER, JITTER),
            );
            let shape = crate_shape(index as u32, rng.range_f32(0.3, 0.6));
            let collider = Collider {
                friction: rng.range_f32(0.2, 0.9),
                restitution: rng.range_f32(0.0, 0.4),
                ..Collider::new(shape)
            };
            spawn(world, position, Body::dynamic(), collider)
        })
        .collect()
}

fn spawn(world: &mut World, position: Vec2, body: Body, collider: Collider) -> Entity {
    let entity = world.spawn();
    let pose = Pose {
        position,
        rotation: Rot::IDENTITY,
    };
    world.insert(entity, pose).expect("Pose is registered");
    world.insert(entity, body).expect("Body is registered");
    world
        .insert(entity, collider)
        .expect("Collider is registered");
    entity
}
