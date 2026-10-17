//! The reference scenario: a few hundred crates in a closed container, nudged, spawned,
//! and despawned by a recorded input log. Its golden trace proves that every platform
//! and thread count computes the same simulation.

mod scene;

use ludenea::physics::{self, apply_impulse, mass_data};
use ludenea::prelude::*;
use serde::{Deserialize, Serialize};

pub use scene::generate_scene;

/// Height at which `BoxesInput::spawn` drops a new crate.
const SPAWN_HEIGHT: f32 = 26.0;

#[derive(Serialize, Deserialize, Schema, Default, Clone)]
pub struct BoxesInput {
    /// Velocity change applied to one randomly chosen crate.
    pub velocity_change: Option<Vec2>,
    /// Spawns a random crate at this horizontal position.
    pub spawn: Option<f32>,
    /// Despawns one randomly chosen crate.
    pub despawn: bool,
}

/// Marks the dynamic bodies of the scenario, as opposed to the container walls.
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct Crate;

#[derive(Resource, Serialize, Deserialize, Default, Clone)]
pub struct ContactStats {
    pub begins: u64,
    pub ends: u64,
}

pub struct BoxesPlugin;

impl Plugin for BoxesPlugin {
    fn build(&self, app: &mut App) {
        app.input::<BoxesInput>()
            .component::<Crate>()
            .resource(ContactStats::default())
            .system(Sim, apply_inputs.before(physics::Step))
            .system(Sim, count_contacts.after(physics::Step));
    }
}

pub fn app() -> App {
    let mut app = App::new();
    app.plugin(PhysicsPlugin).plugin(BoxesPlugin);
    app
}

/// The crate shape for a variant (circle, box, capsule, cycling) and a size in meters.
pub(crate) fn crate_shape(variant: u32, size: f32) -> Shape {
    match variant % 3 {
        0 => Shape::Circle(Circle {
            center: Vec2::ZERO,
            radius: size,
        }),
        1 => Shape::Polygon(Polygon::make_box(size, size)),
        _ => Shape::Capsule(Capsule {
            center1: Vec2::new(-0.5 * size, 0.0),
            center2: Vec2::new(0.5 * size, 0.0),
            radius: 0.5 * size,
        }),
    }
}

fn apply_inputs(
    inputs: Runtime<Inputs<BoxesInput>>,
    mut rng: ResMut<Rng>,
    mut crates: Query<(Entity, &mut Body, &Collider), With<Crate>>,
    mut commands: Commands,
) {
    let input = inputs.player(0);
    let entities: Vec<Entity> = crates.iter_mut().map(|(entity, _, _)| entity).collect();
    if let Some(change) = input.velocity_change
        && let Some(target) = pick(&mut rng, &entities)
    {
        let (_, body, collider) = crates.get_mut(target).expect("listed by this query");
        let mass = mass_data(&collider.shape, collider.density).mass;
        apply_impulse(body, collider, change * mass);
    }
    if let Some(x) = input.spawn {
        let shape = crate_shape(rng.range_u32(0, 3), rng.range_f32(0.3, 0.6));
        commands
            .spawn()
            .insert(Pose {
                position: Vec2::new(x, SPAWN_HEIGHT),
                rotation: Rot::IDENTITY,
            })
            .insert(Body::dynamic())
            .insert(Collider::new(shape))
            .insert(Crate);
    }
    if input.despawn
        && let Some(victim) = pick(&mut rng, &entities)
    {
        commands.despawn(victim);
    }
}

/// One entity chosen uniformly, drawing from `rng` only when there is a choice to make.
fn pick(rng: &mut Rng, entities: &[Entity]) -> Option<Entity> {
    if entities.is_empty() {
        return None;
    }
    let count = u32::try_from(entities.len()).expect("fewer than 2^32 crates");
    Some(entities[rng.range_u32(0, count) as usize])
}

fn count_contacts(collisions: Events<Collision>, mut stats: ResMut<ContactStats>) {
    for collision in collisions.iter() {
        match collision.kind {
            CollisionKind::Begin => stats.begins += 1,
            CollisionKind::End => stats.ends += 1,
        }
    }
}
