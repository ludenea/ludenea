use ludenea_app::{App, Simulation};
use ludenea_ecs::{Entity, Query};
use ludenea_math::{Aabb, Rot, Vec2};
use ludenea_physics::{
    Body, Capsule, Circle, Collider, PhysicsPlugin, Polygon, Pose, Shape, overlap_aabb, raycast,
};

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin);
    app.build().unwrap()
}

fn spawn(simulation: &mut Simulation, x: f32, y: f32, shape: Shape) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    let position = Vec2::new(x, y);
    world
        .insert(
            entity,
            Pose {
                position,
                rotation: Rot::IDENTITY,
            },
        )
        .unwrap();
    world.insert(entity, Body::fixed()).unwrap();
    world.insert(entity, Collider::new(shape)).unwrap();
    entity
}

fn square(half: f32) -> Shape {
    Shape::Polygon(Polygon::make_box(half, half))
}

#[test]
fn raycast_hits_nearest_box_with_expected_normal() {
    let mut simulation = simulation();
    let near = spawn(&mut simulation, 0.0, 0.0, square(1.0));
    spawn(&mut simulation, 0.0, -4.0, square(1.0));
    let query = Query::new(simulation.world()).unwrap();
    let hit = raycast(&query, Vec2::new(0.0, 5.0), Vec2::new(0.0, -10.0)).unwrap();
    assert_eq!(hit.entity, near);
    assert!((hit.fraction - 0.4).abs() < 1e-4);
    assert!(hit.normal.distance(Vec2::new(0.0, 1.0)) < 1e-4);
    assert!(hit.point.distance(Vec2::new(0.0, 1.0)) < 1e-4);
}

#[test]
fn raycast_ties_break_to_lower_entity() {
    let mut simulation = simulation();
    let lower = spawn(&mut simulation, 0.0, 0.0, square(1.0));
    spawn(&mut simulation, 0.0, 0.0, square(1.0));
    let query = Query::new(simulation.world()).unwrap();
    let hit = raycast(&query, Vec2::new(0.0, 5.0), Vec2::new(0.0, -10.0)).unwrap();
    assert_eq!(hit.entity, lower);
}

#[test]
fn raycast_misses_return_none() {
    let mut simulation = simulation();
    spawn(&mut simulation, 0.0, 0.0, square(1.0));
    let query = Query::new(simulation.world()).unwrap();
    assert!(raycast(&query, Vec2::new(5.0, 5.0), Vec2::new(0.0, -10.0)).is_none());
    // Too short to arrive.
    assert!(raycast(&query, Vec2::new(0.0, 5.0), Vec2::new(0.0, -1.0)).is_none());
    assert!(raycast(&query, Vec2::new(0.0, 5.0), Vec2::ZERO).is_none());
}

#[test]
fn raycast_starting_inside_polygon_ignores_it() {
    let mut simulation = simulation();
    spawn(&mut simulation, 0.0, 0.0, square(1.0));
    let rounded = Polygon::new(
        &[
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, 1.0),
        ],
        0.5,
    )
    .unwrap();
    spawn(&mut simulation, 10.0, 0.0, Shape::Polygon(rounded));
    let query = Query::new(simulation.world()).unwrap();
    assert!(raycast(&query, Vec2::new(0.5, 0.5), Vec2::new(0.0, -10.0)).is_none());
    // Inside the rounded rim only, and in the vertex disc zone.
    assert!(raycast(&query, Vec2::new(11.3, 0.0), Vec2::new(5.0, 0.0)).is_none());
    assert!(raycast(&query, Vec2::new(11.2, 1.2), Vec2::new(-5.0, -5.0)).is_none());
}

#[test]
fn raycast_hits_circle_capsule_and_rounded_polygon() {
    let mut simulation = simulation();
    let circle = spawn(
        &mut simulation,
        0.0,
        0.0,
        Shape::Circle(Circle {
            center: Vec2::ZERO,
            radius: 1.0,
        }),
    );
    let capsule = spawn(
        &mut simulation,
        10.0,
        0.0,
        Shape::Capsule(Capsule {
            center1: Vec2::new(-1.0, 0.0),
            center2: Vec2::new(1.0, 0.0),
            radius: 0.5,
        }),
    );
    let rounded = spawn(
        &mut simulation,
        20.0,
        0.0,
        Shape::Polygon(
            Polygon::new(
                &[
                    Vec2::new(-1.0, -1.0),
                    Vec2::new(1.0, -1.0),
                    Vec2::new(1.0, 1.0),
                    Vec2::new(-1.0, 1.0),
                ],
                0.5,
            )
            .unwrap(),
        ),
    );
    let query = Query::new(simulation.world()).unwrap();
    let down = Vec2::new(0.0, -10.0);
    let hit = raycast(&query, Vec2::new(0.0, 5.0), down).unwrap();
    assert_eq!(hit.entity, circle);
    assert!((hit.point.y - 1.0).abs() < 1e-4);
    let hit = raycast(&query, Vec2::new(10.0, 5.0), down).unwrap();
    assert_eq!(hit.entity, capsule);
    assert!((hit.point.y - 0.5).abs() < 1e-4);
    assert!(hit.normal.distance(Vec2::new(0.0, 1.0)) < 1e-4);
    let hit = raycast(&query, Vec2::new(20.0, 5.0), down).unwrap();
    assert_eq!(hit.entity, rounded);
    assert!((hit.point.y - 1.5).abs() < 1e-4);
    // Through the rounded corner: hits the vertex disc.
    let hit = raycast(&query, Vec2::new(21.3, 5.0), down).unwrap();
    assert_eq!(hit.entity, rounded);
    assert!(hit.point.y < 1.5 && hit.point.y > 1.0);
}

#[test]
fn overlap_aabb_returns_ascending_entities() {
    let mut simulation = simulation();
    let first = spawn(&mut simulation, 0.0, 0.0, square(1.0));
    spawn(&mut simulation, 50.0, 0.0, square(1.0));
    let third = spawn(&mut simulation, 1.5, 0.0, square(1.0));
    let query = Query::new(simulation.world()).unwrap();
    let found = overlap_aabb(
        &query,
        Aabb {
            min: Vec2::new(-0.5, -0.5),
            max: Vec2::new(0.5, 0.5),
        },
    );
    assert_eq!(found, [first, third]);
}
