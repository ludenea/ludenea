use std::collections::BTreeSet;

use ludenea_app::{App, Simulation};
use ludenea_debug_draw::Primitive;
use ludenea_ecs::Entity;
use ludenea_math::Vec2;
use ludenea_physics::{Body, Circle, Collider, PhysicsPlugin, Polygon, Pose, Shape};

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin);
    app.build().unwrap()
}

fn spawn_box(simulation: &mut Simulation, x: f32, y: f32) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Pose {
                position: Vec2::new(x, y),
                ..Pose::default()
            },
        )
        .unwrap();
    world.insert(entity, Body::fixed()).unwrap();
    world
        .insert(
            entity,
            Collider::new(Shape::Polygon(Polygon::make_box(1.0, 1.0))),
        )
        .unwrap();
    entity
}

fn layers(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

#[test]
fn colliders_layer_draws_one_item_per_collider_with_entity() {
    let mut simulation = simulation();
    let a = spawn_box(&mut simulation, 0.0, 0.0);
    let b = spawn_box(&mut simulation, 5.0, 0.0);
    let frame = simulation.debug_draw(&layers(&["physics.colliders"]));
    let items = &frame.layers["physics.colliders"];
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].entity, Some(a));
    assert_eq!(items[1].entity, Some(b));
    let Primitive::Polygon { points, fill, .. } = &items[1].primitive else {
        panic!("a box is drawn as a polygon");
    };
    assert!(!fill);
    assert!(points.contains(&Vec2::new(6.0, 1.0)));
    assert_eq!(
        simulation.debug_layers(),
        vec!["physics.aabbs", "physics.colliders", "physics.contacts"]
    );
}

#[test]
fn unrequested_layers_are_empty() {
    let mut simulation = simulation();
    spawn_box(&mut simulation, 0.0, 0.0);
    let frame = simulation.debug_draw(&layers(&["physics.aabbs"]));
    assert!(!frame.layers.contains_key("physics.colliders"));
    assert_eq!(frame.layers["physics.aabbs"].len(), 1);
    assert!(simulation.debug_draw(&layers(&[])).layers.is_empty());
}

#[test]
fn pick_inside_box_returns_it() {
    let mut simulation = simulation();
    let target = spawn_box(&mut simulation, 0.0, 0.0);
    spawn_box(&mut simulation, 10.0, 0.0);
    assert_eq!(simulation.pick(Vec2::new(0.5, -0.5)), vec![target]);
}

#[test]
fn pick_outside_everything_returns_empty() {
    let mut simulation = simulation();
    spawn_box(&mut simulation, 0.0, 0.0);
    // Inside the bounding box of the circle but outside the circle itself.
    let world = simulation.world_mut();
    let round = world.spawn();
    world.insert(round, Pose::default()).unwrap();
    world
        .insert(
            round,
            Collider::new(Shape::Circle(Circle {
                center: Vec2::new(20.0, 0.0),
                radius: 1.0,
            })),
        )
        .unwrap();
    assert!(simulation.pick(Vec2::new(3.0, 3.0)).is_empty());
    assert!(simulation.pick(Vec2::new(20.9, 0.9)).is_empty());
}

#[test]
fn pick_collider_less_pose_within_radius() {
    let mut simulation = simulation();
    let world = simulation.world_mut();
    let marker = world.spawn();
    world
        .insert(
            marker,
            Pose {
                position: Vec2::new(4.0, 4.0),
                ..Pose::default()
            },
        )
        .unwrap();
    assert_eq!(simulation.pick(Vec2::new(4.2, 4.0)), vec![marker]);
    assert!(simulation.pick(Vec2::new(4.3, 4.0)).is_empty());
}

#[test]
fn debug_draw_and_pick_do_not_change_world_hash() {
    let mut simulation = simulation();
    spawn_box(&mut simulation, 0.0, 0.0);
    let moving = spawn_box(&mut simulation, 1.5, 0.0);
    simulation
        .world_mut()
        .insert(moving, Body::dynamic())
        .unwrap();
    simulation.step();
    let before = simulation.hash();
    let all = layers(&["physics.colliders", "physics.contacts", "physics.aabbs"]);
    let frame = simulation.debug_draw(&all);
    assert!(!frame.layers["physics.contacts"].is_empty());
    simulation.pick(Vec2::new(0.0, 0.0));
    assert_eq!(simulation.hash(), before);
}
