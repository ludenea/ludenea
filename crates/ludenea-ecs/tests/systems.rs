use ludenea_ecs::{
    BuildError, Commands, Component, Entity, EventWriter, Events, IntoSystemConfig, Query, Res,
    ResMut, Resource, Runtime, ScheduleBuilder, SystemLabel, World,
};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Value(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Health(u32);

#[derive(Resource, Serialize, Deserialize, Default)]
struct Seen {
    during: usize,
    values: Vec<u32>,
}

#[derive(Resource, Serialize, Deserialize)]
struct Counter(u32);

#[derive(Resource, Serialize, Deserialize)]
struct Target(Entity);

struct Hit(u32);

struct Spawner;

impl SystemLabel for Spawner {
    fn name(&self) -> &'static str {
        "spawner"
    }
}

fn world() -> World {
    let mut world = World::new();
    world.register_component::<Value>().unwrap();
    world.register_component::<Health>().unwrap();
    world.register_resource(Seen::default()).unwrap();
    world
}

fn run(world: &mut World, systems: impl FnOnce(&mut ScheduleBuilder)) {
    let mut builder = ScheduleBuilder::default();
    systems(&mut builder);
    let Ok(mut schedule) = builder.build(world) else {
        panic!("schedule should build");
    };
    schedule.run(world);
}

fn build_error(world: &World, systems: impl FnOnce(&mut ScheduleBuilder)) -> BuildError {
    let mut builder = ScheduleBuilder::default();
    systems(&mut builder);
    builder.build(world).err().expect("build should fail")
}

fn spawn_three(mut commands: Commands, values: Query<&Value>, mut seen: ResMut<Seen>) {
    for value in 1..=3 {
        commands.spawn().insert(Value(value));
    }
    seen.during = values.iter().count();
}

fn record(values: Query<&Value>, mut seen: ResMut<Seen>) {
    seen.values = values.iter().map(|value| value.0).collect();
}

#[test]
fn commands_apply_in_issue_order_after_system() {
    let mut world = world();
    run(&mut world, |builder| {
        builder.add(spawn_three.in_set(Spawner));
        builder.add(record.after(Spawner));
    });
    let seen = world.resource::<Seen>().unwrap();
    assert_eq!(seen.during, 0);
    assert_eq!(seen.values, [1, 2, 3]);
}

fn poke_target(target: Res<Target>, mut commands: Commands) {
    commands.insert(target.0, Value(9));
    commands.remove::<Value>(target.0);
    commands.despawn(target.0);
}

#[test]
fn commands_on_dead_entity_are_ignored() {
    let mut world = world();
    let dead = world.spawn();
    world.despawn(dead).unwrap();
    let reused = world.spawn();
    world.insert(reused, Value(5)).unwrap();
    world.register_resource(Target(dead)).unwrap();

    run(&mut world, |builder| builder.add(poke_target));

    assert!(world.is_alive(reused));
    assert_eq!(*world.get::<Value>(reused).unwrap(), Value(5));
    assert_eq!(world.entities().count(), 1);
}

fn send_hits(mut hits: EventWriter<Hit>) {
    hits.send(Hit(1));
    hits.send(Hit(2));
}

fn read_hits(hits: Events<Hit>, mut seen: ResMut<Seen>) {
    seen.values = hits.iter().map(|hit| hit.0).collect();
}

#[test]
fn events_are_visible_to_later_systems_and_cleared_by_clear_events() {
    let mut world = world();
    world.register_event::<Hit>();
    run(&mut world, |builder| {
        builder.add(send_hits);
        builder.add(read_hits);
    });
    assert_eq!(world.resource::<Seen>().unwrap().values, [1, 2]);

    world.clear_events();
    run(&mut world, |builder| builder.add(read_hits));
    assert!(world.resource::<Seen>().unwrap().values.is_empty());
}

fn bad(_: Query<&mut Health>, _: Query<&Health>) {}

#[test]
fn conflicting_parameters_fail_at_build_with_named_message() {
    let error = build_error(&world(), |builder| builder.add(bad));
    assert!(matches!(
        error,
        BuildError::AccessConflict {
            first: 1,
            second: 2,
            ..
        }
    ));
    let message = error.to_string();
    assert!(message.contains("Health"), "{message}");
    assert!(message.contains("bad"), "{message}");
}

fn bad_resource(_: Res<Counter>, _: ResMut<Counter>) {}

#[test]
fn resource_conflict_message_suggests_a_single_res_mut() {
    let mut world = world();
    world.register_resource(Counter(0)).unwrap();
    let message = build_error(&world, |builder| builder.add(bad_resource)).to_string();
    assert!(message.contains("bad_resource"), "{message}");
    assert!(
        message.contains("Use a single ResMut parameter instead of both Res and ResMut."),
        "{message}"
    );
}

fn needs_counter(_: Res<Counter>) {}

#[test]
fn unregistered_resource_fails_at_build() {
    let error = build_error(&world(), |builder| builder.add(needs_counter));
    assert!(matches!(
        error,
        BuildError::Unregistered {
            kind: "resource",
            ..
        }
    ));
    let message = error.to_string();
    assert!(message.contains("Counter"), "{message}");
    assert!(message.contains("needs_counter"), "{message}");
}

fn increment(mut counter: ResMut<Counter>) {
    counter.0 += 1;
}

fn double(mut counter: ResMut<Counter>) {
    counter.0 *= 2;
}

#[test]
fn res_mut_changes_are_visible_to_next_system() {
    let mut world = world();
    world.register_resource(Counter(1)).unwrap();
    run(&mut world, |builder| {
        builder.add(increment);
        builder.add(double);
    });
    assert_eq!(world.resource::<Counter>().unwrap().0, 4);
}

struct Speed(u32);

#[derive(Query)]
struct Fighter<'w> {
    health: &'w mut Health,
}

fn heal(mut fighters: Query<Fighter>, speed: Runtime<Speed>) {
    for fighter in fighters.iter_mut() {
        fighter.health.0 += speed.0;
    }
}

#[test]
fn runtime_resources_and_derived_queries_are_parameters() {
    let mut world = world();
    world.insert_runtime(Speed(3));
    let entity = world.spawn();
    world.insert(entity, Health(1)).unwrap();
    run(&mut world, |builder| builder.add(heal));
    assert_eq!(*world.get::<Health>(entity).unwrap(), Health(4));
}
