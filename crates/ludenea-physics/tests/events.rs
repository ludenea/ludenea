use ludenea_app::{App, Sim, Simulation, Time};
use ludenea_ecs::{Commands, Entity, Events, IntoSystemConfig, Query, Res, ResMut, Resource};
use ludenea_math::{Rot, Vec2};
use ludenea_physics::{
    Body, Collider, Collision, CollisionKind, PhysicsPlugin, Polygon, Pose, Shape, Step,
};
use serde::{Deserialize, Serialize};

/// Collisions seen per tick, and the tick at which to despawn the entity `victim`.
#[derive(Resource, Serialize, Deserialize, Default)]
struct Log {
    seen: Vec<(bool, Entity, Entity)>,
    victim: Option<Entity>,
    despawn_at: u64,
}

fn record(events: Events<Collision>, mut log: ResMut<Log>) {
    for event in events.iter() {
        log.seen
            .push((event.kind == CollisionKind::Begin, event.a, event.b));
    }
}

fn despawner(time: Res<Time>, log: Res<Log>, mut commands: Commands) {
    if let Some(victim) = log.victim
        && time.tick == log.despawn_at
    {
        commands.despawn(victim);
    }
}

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin)
        .resource(Log::default())
        .system(Sim, despawner.before(Step))
        .system(Sim, record.after(Step));
    app.build().unwrap()
}

fn spawn(simulation: &mut Simulation, y: f32, body: Body) -> Entity {
    let world = simulation.world_mut();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Pose {
                position: Vec2::new(0.0, y),
                rotation: Rot::IDENTITY,
            },
        )
        .unwrap();
    world.insert(entity, body).unwrap();
    world
        .insert(
            entity,
            Collider::new(Shape::Polygon(Polygon::make_box(0.5, 0.5))),
        )
        .unwrap();
    entity
}

fn seen(simulation: &Simulation) -> Vec<(bool, Entity, Entity)> {
    simulation.world().resource::<Log>().unwrap().seen.clone()
}

#[test]
fn landing_box_emits_one_begin() {
    let mut simulation = simulation();
    let ground = spawn(&mut simulation, 0.0, Body::fixed());
    let falling = spawn(&mut simulation, 3.0, Body::dynamic());
    for _ in 0..200 {
        simulation.step();
    }
    assert_eq!(seen(&simulation), [(true, ground, falling)]);
}

#[test]
fn separating_emits_end() {
    let mut simulation = simulation();
    let ground = spawn(&mut simulation, 0.0, Body::fixed());
    let body = Body {
        linear_velocity: Vec2::new(0.0, 20.0),
        ..Body::dynamic()
    };
    let launched = spawn(&mut simulation, 0.9, body);
    for _ in 0..30 {
        simulation.step();
    }
    assert_eq!(
        seen(&simulation),
        [(true, ground, launched), (false, ground, launched)]
    );
}

#[test]
fn despawn_while_touching_emits_end_and_does_not_panic() {
    let mut simulation = simulation();
    let ground = spawn(&mut simulation, 0.0, Body::fixed());
    let resting = spawn(&mut simulation, 0.95, Body::dynamic());
    {
        let log = simulation.world_mut().resource_mut::<Log>().unwrap();
        log.victim = Some(resting);
        log.despawn_at = 20;
    }
    for _ in 0..40 {
        simulation.step();
    }
    assert_eq!(
        seen(&simulation),
        [(true, ground, resting), (false, ground, resting)]
    );
    let query = Query::<&Pose>::new(simulation.world()).unwrap();
    assert!(query.get(resting).is_err());
}

#[test]
fn events_are_sorted_by_pair() {
    let mut simulation = simulation();
    let ground = spawn(&mut simulation, 0.0, Body::fixed());
    // Spawned in reverse height order, so the pairs do not start in entity order.
    let upper = spawn(&mut simulation, 0.9, Body::dynamic());
    let far = spawn(&mut simulation, 50.0, Body::fixed());
    let other = spawn(&mut simulation, 50.9, Body::dynamic());
    simulation.step();
    assert_eq!(
        seen(&simulation),
        [(true, ground, upper), (true, far, other)]
    );
}
