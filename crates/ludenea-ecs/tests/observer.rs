use ludenea_ecs::{
    CommandKind, CommandRecord, Commands, Component, EventWriter, IntoSystemConfig, Res, ResMut,
    Resource, ScheduleBuilder, ScheduleObserver, SystemInfo, SystemLabel, World,
};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize)]
struct Marker;

#[derive(Resource, Serialize, Deserialize, Default)]
struct Counter(u32);

#[derive(Serialize)]
struct Hit(u32);

struct Early;

impl SystemLabel for Early {
    fn name(&self) -> &'static str {
        "early"
    }
}

fn spawner(mut commands: Commands, mut hits: EventWriter<Hit>) {
    commands.spawn().insert(Marker);
    hits.send(Hit(7));
}

fn counter(mut counter: ResMut<Counter>) {
    counter.0 += 1;
}

fn reader(_: Res<Counter>) {}

fn world() -> World {
    let mut world = World::new();
    world.register_component::<Marker>().unwrap();
    world.register_resource(Counter::default()).unwrap();
    world.register_event::<Hit>();
    world
}

#[derive(Default)]
struct Recorder {
    steps: Vec<String>,
    events: Vec<(&'static str, Vec<serde_json::Value>)>,
}

impl ScheduleObserver for Recorder {
    fn before_system(&mut self, index: usize) {
        self.steps.push(format!("before {index}"));
    }

    fn after_system(&mut self, index: usize, commands: &[CommandRecord]) {
        let kinds: Vec<_> = commands
            .iter()
            .map(|record| (record.kind, record.entity.is_some(), record.component))
            .collect();
        self.steps.push(format!("after {index} {kinds:?}"));
    }

    fn tick_end(&mut self, world: &World) {
        self.events = world.events_json();
    }
}

#[test]
fn observer_sees_systems_in_order_and_their_commands() {
    let mut world = world();
    let mut builder = ScheduleBuilder::default();
    builder.add(reader.after(Early));
    builder.add(counter.in_set(Early));
    builder.add(spawner);
    let mut schedule = builder.build(&world).unwrap();

    let infos: Vec<SystemInfo> = schedule.systems(&world);
    let names: Vec<_> = infos.iter().map(|info| info.name).collect();
    assert!(names[0].ends_with("counter") && names[1].ends_with("reader"));
    assert!(names[2].ends_with("spawner"));
    assert_eq!(infos[0].writes, [("resource", Counter::NAME)]);
    assert_eq!(infos[1].reads, [("resource", Counter::NAME)]);
    assert_eq!(infos[2].writes, [("event", std::any::type_name::<Hit>())]);

    let mut recorder = Recorder::default();
    schedule.run_observed(&mut world, &mut recorder);

    let spawn_and_insert = [
        (CommandKind::Spawn, true, None),
        (CommandKind::Insert, true, Some(Marker::NAME)),
    ];
    assert_eq!(
        recorder.steps,
        [
            "before 0".to_string(),
            "after 0 []".to_string(),
            "before 1".to_string(),
            "after 1 []".to_string(),
            "before 2".to_string(),
            format!("after 2 {spawn_and_insert:?}"),
        ]
    );
    assert_eq!(world.entities().count(), 1);
    assert_eq!(world.resource::<Counter>().unwrap().0, 1);
}

#[test]
fn observer_sees_this_tick_events_before_they_clear() {
    let mut world = world();
    let mut builder = ScheduleBuilder::default();
    builder.add(spawner);
    let mut schedule = builder.build(&world).unwrap();

    let mut recorder = Recorder::default();
    schedule.run_observed(&mut world, &mut recorder);
    assert_eq!(
        recorder.events,
        [(std::any::type_name::<Hit>(), vec![serde_json::json!(7)])]
    );

    world.clear_events();
    assert!(
        world
            .events_json()
            .iter()
            .all(|(_, events)| events.is_empty())
    );
}
