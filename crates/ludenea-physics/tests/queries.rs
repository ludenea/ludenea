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
    spawn_rotated(simulation, x, y, 0.0, shape)
}

fn spawn_rotated(simulation: &mut Simulation, x: f32, y: f32, angle: f32, shape: Shape) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    let position = Vec2::new(x, y);
    world
        .insert(
            entity,
            Pose {
                position,
                rotation: Rot::from_angle(angle),
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
    assert!(hit.normal.distance(Vec2::new(0.0, 1.0)) < 1e-4);
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
    // The normal points from the vertex (21, 1) through the hit point.
    let radial = (hit.point - Vec2::new(21.0, 1.0)).normalize();
    assert!(hit.normal.distance(radial) < 1e-4);
    assert!(((hit.point - Vec2::new(21.0, 1.0)).length() - 0.5).abs() < 1e-4);
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

fn assert_hit(hit: ludenea_physics::RayHit, point: Vec2, normal: Vec2, fraction: f32) {
    assert!(hit.point.distance(point) < 1e-3, "point {:?}", hit.point);
    assert!(
        hit.normal.distance(normal) < 1e-3,
        "normal {:?}",
        hit.normal
    );
    assert!(
        (hit.fraction - fraction).abs() < 1e-4,
        "fraction {}",
        hit.fraction
    );
}

fn capsule() -> Shape {
    Shape::Capsule(Capsule {
        center1: Vec2::new(-1.0, 0.0),
        center2: Vec2::new(1.0, 0.0),
        radius: 0.5,
    })
}

#[test]
fn raycast_respects_body_rotation() {
    let quarter = std::f32::consts::FRAC_PI_2;
    let sixth = std::f32::consts::FRAC_PI_6;
    let up = Vec2::new(0.0, 1.0);
    let down = Vec2::new(0.0, -10.0);

    // Rotated a quarter turn: the 2 x 1 box (half extents) is 1 wide and 4 tall.
    let mut simulation = simulation();
    let wide = Shape::Polygon(Polygon::make_box(2.0, 0.5));
    spawn_rotated(&mut simulation, 0.0, 0.0, quarter, wide.clone());
    let query = Query::new(simulation.world()).unwrap();
    let hit = raycast(&query, Vec2::new(0.0, 5.0), down).unwrap();
    assert_hit(hit, Vec2::new(0.0, 2.0), up, 0.3);
    drop(query);

    // Rotated 30 degrees: the top face normal is (-sin 30, cos 30).
    let mut simulation = self::simulation();
    spawn_rotated(&mut simulation, 0.0, 0.0, sixth, wide);
    let query = Query::new(simulation.world()).unwrap();
    let y = 0.5 / 0.866_025_4;
    let normal = Vec2::new(-0.5, 0.866_025_4);
    let hit = raycast(&query, Vec2::new(0.0, 10.0), Vec2::new(0.0, -20.0)).unwrap();
    assert_hit(hit, Vec2::new(0.0, y), normal, (10.0 - y) / 20.0);
    drop(query);

    // Capsule a quarter turn: vertical, its cap reaches y = 1.5, its side x = 0.5.
    let mut simulation = self::simulation();
    spawn_rotated(&mut simulation, 0.0, 0.0, quarter, capsule());
    let query = Query::new(simulation.world()).unwrap();
    let hit = raycast(&query, Vec2::new(0.0, 5.0), down).unwrap();
    assert_hit(hit, Vec2::new(0.0, 1.5), up, 0.35);
    let hit = raycast(&query, Vec2::new(5.0, 0.0), Vec2::new(-10.0, 0.0)).unwrap();
    assert_hit(hit, Vec2::new(0.5, 0.0), Vec2::new(1.0, 0.0), 0.45);
    drop(query);

    // Capsule at 30 degrees, hit on its side.
    let mut simulation = self::simulation();
    spawn_rotated(&mut simulation, 0.0, 0.0, sixth, capsule());
    let query = Query::new(simulation.world()).unwrap();
    let hit = raycast(&query, Vec2::new(0.0, 10.0), Vec2::new(0.0, -20.0)).unwrap();
    assert_hit(hit, Vec2::new(0.0, y), normal, (10.0 - y) / 20.0);
}
