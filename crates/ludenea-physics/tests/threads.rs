use ludenea_app::{App, Simulation};
use ludenea_math::{Rot, Vec2};
use ludenea_physics::{Body, Capsule, Circle, Collider, PhysicsPlugin, Polygon, Pose, Shape};

fn spawn(simulation: &mut Simulation, position: Vec2, body: Body, collider: Collider) {
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
}

fn boxed(half_width: f32, half_height: f32) -> Collider {
    Collider::new(Shape::Polygon(Polygon::make_box(half_width, half_height)))
}

/// 200 bodies: a static floor and walls, a kinematic paddle, and a pile of mixed shapes.
fn scene(threads: usize) -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin);
    let mut simulation = app.build().unwrap();
    simulation.set_threads(threads);
    spawn(&mut simulation, Vec2::ZERO, Body::fixed(), boxed(12.0, 0.5));
    spawn(
        &mut simulation,
        Vec2::new(-12.0, 10.0),
        Body::fixed(),
        boxed(0.5, 10.0),
    );
    spawn(
        &mut simulation,
        Vec2::new(12.0, 10.0),
        Body::fixed(),
        boxed(0.5, 10.0),
    );
    let mut paddle = Body::kinematic();
    paddle.angular_velocity = 1.0;
    spawn(
        &mut simulation,
        Vec2::new(0.0, 3.0),
        paddle,
        boxed(3.0, 0.2),
    );
    for i in 0..196 {
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
