//! The scenes of the Rapier half: three that scale with a body count (a pile
//! at rest, bodies in motion that seldom touch, chains of jointed bodies),
//! the churn that destroys and creates bodies, and the three 3D scenes the
//! Rapier snapshot continuation spike ran, adopted as entities.

use rapier3d::prelude::*;

use crate::sim::{
    BODY_DYNAMIC, BODY_FIXED, BODY_OTHER, Components, Definition, KIND_CAPSULE, KIND_CUBOID, KIND_FOREIGN,
    Pool, Random, Sim,
};

pub const SCENE_SEED: u64 = 0x4c75_6465_6e65_6132;
pub const INPUT_SEED: u64 = 0x696e_7075_7473_3031;
const PYRAMID_BASE: usize = 10;
const CHAIN_LINKS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SceneKind {
    Pile,
    Motion,
    Chains,
}

pub const SCENE_KINDS: [SceneKind; 3] = [SceneKind::Pile, SceneKind::Motion, SceneKind::Chains];

impl SceneKind {
    pub fn name(self) -> &'static str {
        match self {
            SceneKind::Pile => "pile_at_rest",
            SceneKind::Motion => "bodies_in_motion",
            SceneKind::Chains => "jointed_chains",
        }
    }

    pub fn impulse_speed(self) -> f32 {
        if self == SceneKind::Pile { 0.02 } else { 0.1 }
    }
}

fn fixed_cuboid(sim: &mut Sim, center: Vector, half: Vector, friction: f32, restitution: f32) {
    sim.world.insert(
        RigidBodyBuilder::fixed().translation(center),
        ColliderBuilder::cuboid(half.x, half.y, half.z).friction(friction).restitution(restitution),
    );
}

/// Pyramids of cubes resting on a ground, ten cubes at the base, built row by
/// row until the body count is reached; the pyramids stand in rows of eight.
fn build_pile(sim: &mut Sim, bodies: usize) {
    let pyramids = bodies.div_ceil(55);
    let pitch = PYRAMID_BASE as f32 + 2.0;
    fixed_cuboid(
        sim,
        Vector::new(0.0, -1.0, 0.0),
        Vector::new(8.0 * pitch + 20.0, 1.0, 4.0 * pyramids.div_ceil(8) as f32 + 20.0),
        0.6,
        0.0,
    );
    let cube = Definition {
        kind: KIND_CUBOID,
        body_type: BODY_DYNAMIC,
        collides: 1,
        a: 0.5,
        b: 0.5,
        density: 1.0,
        friction: 0.6,
        restitution: 0.0,
    };
    let a = 0.5f32;
    let mut created = 0;
    'pyramids: for p in 0..pyramids {
        let origin = pitch * ((p % 8) as f32 - 3.5);
        let z = 4.0 * (p / 8) as f32;
        for i in 0..PYRAMID_BASE {
            let y = (2.0 * i as f32 + 1.0) * a;
            for j in i..PYRAMID_BASE {
                if created == bodies {
                    break 'pyramids;
                }
                let x = origin + (i as f32 + 1.0) * a + 2.0 * (j - i) as f32 * a - a * PYRAMID_BASE as f32;
                sim.spawn(cube, Components::resting_at(x, y, z));
                created += 1;
            }
        }
    }
}

pub fn motion_definition(kind: u32) -> Definition {
    let (a, b) = if kind == KIND_CAPSULE { (0.3, 0.3) } else { (0.5, 0.5) };
    Definition { kind, body_type: BODY_DYNAMIC, collides: 1, a, b, density: 1.0, friction: 0.0, restitution: 1.0 }
}

pub fn ceil_cube_root(value: usize) -> usize {
    let mut side = 1;
    while side * side * side < value {
        side += 1;
    }
    side
}

/// The six walls that keep the bodies in motion inside a cube.
pub fn build_motion_walls(sim: &mut Sim, side: usize) {
    let extent = 4.0 * side as f32;
    let middle = 0.5 * extent - 2.0;
    let half = 0.5 * extent + 4.0;
    for axis in 0..3 {
        for far in [false, true] {
            let mut center = Vector::new(middle, middle, middle);
            let mut size = Vector::new(half, half, half);
            center[axis] = if far { extent } else { -4.0 };
            size[axis] = 1.0;
            fixed_cuboid(sim, center, size, 0.0, 1.0);
        }
    }
}

fn seeded_motion(random: &mut Random, c: &mut Components) {
    c.linear = [random.signed(), random.signed(), random.signed()];
    c.angular = [random.signed(), random.signed(), random.signed()];
}

/// Balls, cubes and capsules on a grid four units apart, each with a seeded
/// velocity, with no gravity, no friction and elastic contacts.
fn build_motion(sim: &mut Sim, bodies: usize) {
    let side = ceil_cube_root(bodies);
    build_motion_walls(sim, side);
    let mut random = Random::new(SCENE_SEED);
    for i in 0..bodies {
        let mut c = Components::resting_at(
            4.0 * (i % side) as f32,
            4.0 * ((i / side) % side) as f32,
            4.0 * (i / (side * side)) as f32,
        );
        seeded_motion(&mut random, &mut c);
        sim.spawn(motion_definition((i % 3) as u32), c);
    }
}

/// Chains of ten cuboids linked by spherical joints, each hung by its first
/// link from a fixed anchor and released horizontal; the links collide with
/// nothing, so the work is the joints'.
fn build_chains(sim: &mut Sim, bodies: usize) {
    let anchor = sim.world.insert_body(RigidBodyBuilder::fixed());
    let link = Definition {
        kind: KIND_CUBOID,
        body_type: BODY_DYNAMIC,
        collides: 0,
        a: 0.5,
        b: 0.125,
        density: 1.0,
        friction: 0.6,
        restitution: 0.0,
    };
    let mut created = 0;
    let mut chain = 0;
    while created < bodies {
        let hook = Vector::new(12.0 * (chain % 16) as f32, 12.0 * ((chain / 16) % 16) as f32, 2.0 * (chain / 256) as f32);
        let mut previous = anchor;
        for k in 0..CHAIN_LINKS {
            if created == bodies {
                break;
            }
            let index = sim.spawn(link, Components::resting_at(hook.x + 0.5 + k as f32, hook.y, hook.z));
            let handle = sim.entities[index].handle;
            let anchor1 = if k == 0 { hook } else { Vector::new(0.5, 0.0, 0.0) };
            let joint = SphericalJointBuilder::new().local_anchor1(anchor1).local_anchor2(Vector::new(-0.5, 0.0, 0.0));
            sim.world.insert_impulse_joint(previous, handle, joint);
            previous = handle;
            created += 1;
        }
        chain += 1;
    }
}

pub fn build_scene(scene: SceneKind, bodies: usize, pool: &Pool, sleep: bool, inputs: bool) -> Sim {
    let gravity = if scene == SceneKind::Motion { Vector::ZERO } else { Vector::new(0.0, -10.0, 0.0) };
    let mut sim = Sim::new(gravity, pool, sleep, INPUT_SEED, if inputs { scene.impulse_speed() } else { 0.0 });
    match scene {
        SceneKind::Pile => build_pile(&mut sim, bodies),
        SceneKind::Motion => build_motion(&mut sim, bodies),
        SceneKind::Chains => build_chains(&mut sim, bodies),
    }
    sim.refresh_pushable();
    sim.sync();
    sim
}

/// One tick of the churn: one body in a hundred is destroyed and as many are
/// created, at seeded places inside the walls, the count held constant.
pub fn churn_tick(sim: &mut Sim, random: &mut Random, side: usize) {
    let changes = sim.count() / 100;
    let extent = 4.0 * side as f32;
    let middle = 0.5 * extent - 2.0;
    let reach = 0.5 * extent - 2.0;
    for _ in 0..changes {
        let index = (random.next() % sim.count() as u64) as usize;
        sim.despawn(index);
    }
    for _ in 0..changes {
        let definition = motion_definition((random.next() % 3) as u32);
        let mut c = Components::resting_at(
            middle + reach * random.signed(),
            middle + reach * random.signed(),
            middle + reach * random.signed(),
        );
        seeded_motion(random, &mut c);
        sim.spawn(definition, c);
    }
    sim.refresh_pushable();
}

// --- The scenes of the Rapier snapshot continuation spike, adopted ------------

pub struct LibraryScene {
    pub name: &'static str,
    build: fn(&mut PhysicsWorld),
}

/// The three 3D scenes the Rapier snapshot continuation spike ran, at their
/// sizes.
pub const LIBRARY_SCENES: [LibraryScene; 3] = [
    LibraryScene { name: "large_pyramid", build: large_pyramid },
    LibraryScene { name: "joint_grid", build: joint_grid },
    LibraryScene { name: "mixed_shape_rain", build: mixed_shape_rain },
];

/// Rapier's port of Box3D's `large_pyramid` benchmark, with a base of 60 cubes.
fn large_pyramid(world: &mut PhysicsWorld) {
    let base_count = 60i32;
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)),
        ColliderBuilder::cuboid(400.0, 1.0, 400.0),
    );
    let h = 0.5f32;
    let shift = 1.0 * h;
    for i in 0..base_count {
        let y = (2.0 * i as f32 + 1.0) * shift;
        for j in i..base_count {
            let x = (i as f32 + 1.0) * shift + 2.0 * (j - i) as f32 * shift - h * base_count as f32;
            let body = RigidBodyBuilder::dynamic().translation(Vector::new(x, y, 0.0)).can_sleep(false);
            world.insert(body, ColliderBuilder::cuboid(h, h, h).density(100.0));
        }
    }
}

/// Rapier's port of Box3D's `joint_grid` benchmark, a 50 by 50 grid.
fn joint_grid(world: &mut PhysicsWorld) {
    let n = 50usize;
    let mut bodies = vec![RigidBodyHandle::invalid(); n * n];
    let mut index = 0usize;
    for k in 0..n {
        for i in 0..n {
            let body = if i == 0 { RigidBodyBuilder::fixed() } else { RigidBodyBuilder::dynamic().can_sleep(false) }
                .translation(Vector::new(k as f32, -(i as f32), 0.0));
            let handle = world.insert_body(body);
            world.insert_collider(ColliderBuilder::ball(0.4), Some(handle));
            if i > 0 {
                let joint = SphericalJointBuilder::new()
                    .local_anchor1(Vector::new(0.0, -0.5, 0.0))
                    .local_anchor2(Vector::new(0.0, 0.5, 0.0));
                world.insert_impulse_joint(bodies[index - 1], handle, joint);
            }
            if k > 0 {
                let joint = SphericalJointBuilder::new()
                    .local_anchor1(Vector::new(0.5, 0.0, 0.0))
                    .local_anchor2(Vector::new(-0.5, 0.0, 0.0));
                world.insert_impulse_joint(bodies[index - n], handle, joint);
            }
            bodies[index] = handle;
            index += 1;
        }
    }
}

/// A grid of columns of balls, boxes and capsules at staggered heights over a
/// ground box, sleeping allowed.
fn mixed_shape_rain(world: &mut PhysicsWorld) {
    let mut random = reference_workload::SplitMix64::new(0x5261_7069_6572_3231);
    let mut jitter = |amplitude: f32| (random.u01() - 0.5) * 2.0 * amplitude;
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(0.0, -1.0, 0.0)),
        ColliderBuilder::cuboid(200.0, 1.0, 200.0),
    );
    let side = 12usize;
    let rows = 10usize;
    for row in 0..rows {
        for ix in 0..side {
            for iz in 0..side {
                let x = -33.0 + 6.0 * ix as f32 + jitter(0.5);
                let z = -33.0 + 6.0 * iz as f32 + jitter(0.5);
                let y = 5.0 + 3.0 * row as f32 + jitter(0.5);
                let body = RigidBodyBuilder::dynamic().translation(Vector::new(x, y, z));
                let collider = match (row + ix + iz) % 3 {
                    0 => ColliderBuilder::ball(0.5),
                    1 => ColliderBuilder::cuboid(0.5, 0.5, 0.5),
                    _ => ColliderBuilder::capsule_y(0.5, 0.3),
                };
                world.insert(body, collider.density(1.0));
            }
        }
    }
}

/// Builds one of the continuation spike's scenes and adopts every body as an
/// entity, in the order of the handles, which is the order that spike hashed
/// them in.
pub fn build_library_scene(scene: &LibraryScene, pool: &Pool) -> Sim {
    let mut sim = Sim::new(Vector::new(0.0, -10.0, 0.0), pool, true, INPUT_SEED, 0.5);
    (scene.build)(&mut sim.world);
    let mut rows: Vec<((u32, u32), RigidBodyHandle, Definition)> = sim
        .world
        .bodies
        .iter()
        .map(|(handle, body)| {
            let body_type = if body.is_dynamic() {
                BODY_DYNAMIC
            } else if body.is_fixed() {
                BODY_FIXED
            } else {
                BODY_OTHER
            };
            let definition = Definition {
                kind: KIND_FOREIGN,
                body_type,
                collides: 1,
                a: body.mass(),
                b: 0.0,
                density: 0.0,
                friction: 0.0,
                restitution: 0.0,
            };
            (handle.into_raw_parts(), handle, definition)
        })
        .collect();
    rows.sort_by_key(|(key, _, _)| *key);
    for (_, handle, definition) in rows {
        sim.adopt(handle, definition);
    }
    sim.refresh_pushable();
    sim.sync();
    sim
}

