use ludenea_app::{App, Simulation};
use ludenea_ecs::Entity;
use ludenea_math::{Rot, Transform, Vec2};
use ludenea_physics::coloring::{SOLVER_CHUNK, color_constraints};
use ludenea_physics::{
    Body, BodyKind, Capsule, Circle, Collider, LINEAR_SLOP, PhysicsPlugin, Polygon, Pose, Proxy,
    Shape, collide, find_pairs,
};

fn spawn(simulation: &mut Simulation, position: Vec2, body: Body, collider: Collider) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Pose {
                position,
                rotation: Rot::IDENTITY,
            },
        )
        .unwrap();
    world.insert(entity, body).unwrap();
    world.insert(entity, collider).unwrap();
    entity
}

fn boxed(half_width: f32, half_height: f32) -> Collider {
    Collider::new(Shape::Polygon(Polygon::make_box(half_width, half_height)))
}

/// 564 bodies: a static floor and walls, a kinematic paddle, and a pile of mixed shapes.
/// The 560 dynamic bodies exceed one solver chunk, so the body stages are split across
/// threads.
fn scene(threads: usize) -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin);
    let mut simulation = app.build().unwrap();
    simulation.set_threads(threads).unwrap();
    spawn(&mut simulation, Vec2::ZERO, Body::fixed(), boxed(12.0, 0.5));
    spawn(
        &mut simulation,
        Vec2::new(-12.0, 30.0),
        Body::fixed(),
        boxed(0.5, 30.0),
    );
    spawn(
        &mut simulation,
        Vec2::new(12.0, 30.0),
        Body::fixed(),
        boxed(0.5, 30.0),
    );
    let mut paddle = Body::kinematic();
    paddle.angular_velocity = 1.0;
    spawn(
        &mut simulation,
        Vec2::new(0.0, 3.0),
        paddle,
        boxed(3.0, 0.2),
    );
    for i in 0..560 {
        let (column, row) = (i % 14, i / 14);
        let position = Vec2::new(
            -9.1 + 1.4 * column as f32 + 0.1 * (row % 2) as f32,
            5.0 + 1.1 * row as f32,
        );
        let mut collider = match i % 3 {
            0 => boxed(0.45, 0.4),
            1 => Collider::new(Shape::Circle(Circle {
                center: Vec2::ZERO,
                radius: 0.45,
            })),
            _ => Collider::new(Shape::Capsule(Capsule {
                center1: Vec2::new(-0.3, 0.0),
                center2: Vec2::new(0.3, 0.0),
                radius: 0.25,
            })),
        };
        collider.restitution = if i % 4 == 0 { 0.5 } else { 0.0 };
        spawn(&mut simulation, position, Body::dynamic(), collider);
    }
    simulation
}

fn hashes(threads: usize) -> Vec<u64> {
    let mut simulation = scene(threads);
    (0..600)
        .map(|_| {
            simulation.step();
            simulation.hash()
        })
        .collect()
}

#[test]
fn hash_sequence_is_identical_for_one_two_and_eight_threads() {
    let one = hashes(1);
    assert_eq!(one, hashes(2), "2 threads diverged from 1");
    assert_eq!(one, hashes(8), "8 threads diverged from 1");
}

const ROW_BOXES: usize = 1100;

/// A static ground under one row of separated dynamic boxes resting on it. Static bodies
/// never block a color, so all ground contacts share one color larger than a solver chunk.
fn row(threads: usize) -> (Simulation, Vec<Entity>) {
    let mut app = App::new();
    app.plugin(PhysicsPlugin);
    let mut simulation = app.build().unwrap();
    simulation.set_threads(threads).unwrap();
    let mut entities = vec![spawn(
        &mut simulation,
        Vec2::ZERO,
        Body::fixed(),
        boxed(280.0, 0.5),
    )];
    for i in 0..ROW_BOXES {
        let position = Vec2::new(-274.75 + 0.5 * i as f32, 0.71);
        entities.push(spawn(
            &mut simulation,
            position,
            Body::dynamic(),
            boxed(0.2, 0.2),
        ));
    }
    (simulation, entities)
}

fn row_hashes(threads: usize) -> (Vec<u64>, Simulation, Vec<Entity>) {
    let (mut simulation, entities) = row(threads);
    let hashes = (0..60)
        .map(|_| {
            simulation.step();
            simulation.hash()
        })
        .collect();
    (hashes, simulation, entities)
}

/// Size of the largest color of touching contacts, recomputed from the public broad
/// phase, narrow phase, and coloring the step uses.
fn largest_color(simulation: &Simulation, entities: &[Entity]) -> usize {
    let world = simulation.world();
    let bodies: Vec<(Transform, BodyKind, Collider)> = entities
        .iter()
        .map(|&entity| {
            let pose = *world.get::<Pose>(entity).unwrap();
            let kind = world.get::<Body>(entity).unwrap().kind;
            let collider = world.get::<Collider>(entity).unwrap().clone();
            let transform = Transform {
                position: pose.position,
                rotation: pose.rotation,
            };
            (transform, kind, collider)
        })
        .collect();
    let speculative = 4.0 * LINEAR_SLOP;
    let proxies: Vec<Proxy> = bodies
        .iter()
        .enumerate()
        .map(|(index, (transform, kind, collider))| Proxy {
            aabb: collider
                .shape
                .compute_aabb(*transform)
                .fattened(speculative),
            body: index as u32,
            kind: *kind,
        })
        .collect();
    let touching: Vec<(u32, u32)> = find_pairs(&proxies)
        .into_iter()
        .filter(|&(a, b)| {
            let ((xf_a, _, a), (xf_b, _, b)) = (&bodies[a as usize], &bodies[b as usize]);
            collide(&a.shape, *xf_a, &b.shape, *xf_b, speculative).count > 0
        })
        .collect();
    let is_dynamic: Vec<bool> = bodies
        .iter()
        .map(|(_, kind, _)| *kind == BodyKind::Dynamic)
        .collect();
    color_constraints(&touching, &is_dynamic)
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
}

#[test]
fn parallel_color_path_is_identical_for_one_two_and_eight_threads() {
    let (one, simulation, entities) = row_hashes(1);
    let largest = largest_color(&simulation, &entities);
    assert!(
        largest > 2 * SOLVER_CHUNK,
        "largest color has {largest} contacts; the scene must split a color into at least three chunks of {SOLVER_CHUNK}"
    );
    assert_eq!(one, row_hashes(2).0, "2 threads diverged from 1");
    assert_eq!(one, row_hashes(8).0, "8 threads diverged from 1");
}
