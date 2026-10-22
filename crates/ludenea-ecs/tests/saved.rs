use std::panic::AssertUnwindSafe;

use ludenea_ecs::{
    Asset, AssetId, Commands, Component, Entity, Event, EventWriter, Events, ResMut, Resource,
    SavedError, SavedWorld, ScheduleBuilder, World,
};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Health(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Label(String);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Target(Entity);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Marker;

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Score(u64);

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct History(Vec<u32>);

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq)]
struct Stats(u32);

/// SplitMix64: a small seeded generator, so every case replays exactly.
struct Generator(u64);

impl Generator {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value below `bound`, which is not zero.
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn pick(&mut self, entities: &[Entity]) -> Option<Entity> {
        let count = u64::try_from(entities.len()).unwrap();
        (count > 0).then(|| entities[usize::try_from(self.below(count)).unwrap()])
    }
}

fn registered() -> World {
    let mut world = World::new();
    world.register_component::<Health>().unwrap();
    world.register_component::<Label>().unwrap();
    world.register_component::<Target>().unwrap();
    world.register_component::<Marker>().unwrap();
    world.register_resource(Score(0)).unwrap();
    world.register_resource(History(Vec::new())).unwrap();
    world.register_asset::<Stats>().unwrap();
    world
}

/// Applies `steps` random changes: spawns, despawns, inserts and removes across every
/// component type, resource writes, and asset loads and unloads.
fn mutate(world: &mut World, generator: &mut Generator, steps: u32) {
    for _ in 0..steps {
        let alive: Vec<Entity> = world.entities().collect();
        let value = u32::try_from(generator.below(1_000)).unwrap();
        match generator.below(12) {
            0 | 1 => {
                world.spawn();
            }
            2 => {
                if let Some(entity) = generator.pick(&alive) {
                    world.despawn(entity).unwrap();
                }
            }
            3 => {
                if let Some(entity) = generator.pick(&alive) {
                    world.insert(entity, Health(value)).unwrap();
                }
            }
            4 => {
                if let Some(entity) = generator.pick(&alive) {
                    world.insert(entity, Label(format!("l{value}"))).unwrap();
                }
            }
            5 => {
                if let (Some(entity), Some(target)) =
                    (generator.pick(&alive), generator.pick(&alive))
                {
                    world.insert(entity, Target(target)).unwrap();
                }
            }
            6 => {
                if let Some(entity) = generator.pick(&alive) {
                    world.insert(entity, Marker).unwrap();
                }
            }
            7 => {
                if let Some(entity) = generator.pick(&alive) {
                    world.remove::<Health>(entity).unwrap();
                    world.remove::<Marker>(entity).unwrap();
                }
            }
            8 => {
                if let Some(entity) = generator.pick(&alive) {
                    world.remove::<Label>(entity).unwrap();
                    world.remove::<Target>(entity).unwrap();
                }
            }
            9 => {
                world.resource_mut::<Score>().unwrap().0 += u64::from(value);
                world.resource_mut::<History>().unwrap().0.push(value);
            }
            10 => {
                let id = AssetId(u128::from(value % 4));
                let bytes = postcard::to_allocvec(&Stats(value)).unwrap();
                world.insert_asset(id, Stats::NAME, &bytes).unwrap();
            }
            _ => {
                if value % 4 == 0 {
                    world.clear_assets();
                } else {
                    world.resource_mut::<History>().unwrap().0.clear();
                }
            }
        }
    }
}

#[test]
fn saved_world_round_trips_snapshot() {
    for case in 0..256 {
        let mut generator = Generator(case);
        let mut world = registered();
        let mut saved = SavedWorld::default();
        // A first save of another shape, so the second one reuses and reshapes it.
        mutate(&mut world, &mut generator, 30);
        world.save_into(&mut saved);
        mutate(&mut world, &mut generator, 30);
        world.save_into(&mut saved);
        let expected = world.snapshot().encode();

        mutate(&mut world, &mut generator, 30);
        world.load(&saved).unwrap();
        assert_eq!(world.snapshot().encode(), expected, "case {case}");

        // The saved world does not share state with the world it was loaded into.
        mutate(&mut world, &mut generator, 30);
        world.load(&saved).unwrap();
        assert_eq!(
            world.snapshot().encode(),
            expected,
            "case {case}, second load"
        );
    }
}

#[derive(Event, Serialize)]
struct Ping;

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Seen(usize);

fn queue_then_panic(mut commands: Commands, mut pings: EventWriter<Ping>) {
    commands.spawn().insert(Health(1));
    pings.send(Ping);
    panic!("aborted tick");
}

fn count_pings(pings: Events<Ping>, mut seen: ResMut<Seen>) {
    seen.0 = pings.iter().count();
}

#[test]
fn load_keeps_runtime_resources_and_drops_commands() {
    let mut world = registered();
    world.register_event::<Ping>().unwrap();
    world.register_resource(Seen(usize::MAX)).unwrap();
    world.insert_runtime(7_u32);
    let entity = world.spawn();
    world.insert(entity, Health(3)).unwrap();
    let mut saved = SavedWorld::default();
    world.save_into(&mut saved);
    let before = world.snapshot();
    *world.runtime_mut::<u32>().unwrap() = 9;

    let mut builder = ScheduleBuilder::default();
    builder.add(queue_then_panic);
    let mut aborting = builder.build(&world).unwrap();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| aborting.run(&mut world)));
    std::panic::set_hook(hook);
    assert!(outcome.is_err());

    world.load(&saved).unwrap();
    assert_eq!(
        *world.runtime::<u32>().unwrap(),
        9,
        "a runtime resource was saved"
    );
    let mut builder = ScheduleBuilder::default();
    builder.add(count_pings);
    builder.build(&world).unwrap().run(&mut world);
    assert_eq!(
        world.resource::<Seen>().unwrap().0,
        0,
        "a stale event survived"
    );
    assert_eq!(
        world.snapshot().entities,
        before.entities,
        "a stale command was applied"
    );
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[component(name = "test::Shared")]
struct SharedA(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[component(name = "test::Shared")]
struct SharedB(u32);

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[resource(name = "test::Setting")]
struct SettingA(u32);

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[resource(name = "test::Setting")]
struct SettingB(u32);

/// A populated world built by `register`, saved.
fn saved_from(register: impl FnOnce(&mut World)) -> SavedWorld {
    let mut world = World::new();
    register(&mut world);
    world.spawn();
    let mut saved = SavedWorld::default();
    world.save_into(&mut saved);
    saved
}

/// Loads `saved` into a populated world built by `register`, expecting `expected` and the
/// world unchanged.
fn assert_refused(saved: &SavedWorld, register: impl FnOnce(&mut World), expected: SavedError) {
    let mut world = World::new();
    register(&mut world);
    let entity = world.spawn();
    world.spawn();
    world.despawn(entity).unwrap();
    let before = world.snapshot().encode();
    assert_eq!(world.load(saved), Err(expected));
    assert_eq!(
        world.snapshot().encode(),
        before,
        "a refused load changed the world"
    );
}

#[test]
fn load_refuses() {
    assert_refused(&SavedWorld::default(), |_| {}, SavedError::Empty);

    let health = |world: &mut World| world.register_component::<Health>().unwrap();
    let health_and_label = |world: &mut World| {
        world.register_component::<Health>().unwrap();
        world.register_component::<Label>().unwrap();
    };
    assert_refused(
        &saved_from(health),
        health_and_label,
        SavedError::MissingStorage {
            name: Label::NAME.to_string(),
        },
    );
    assert_refused(
        &saved_from(health_and_label),
        health,
        SavedError::UnexpectedStorage {
            name: Label::NAME.to_string(),
        },
    );
    assert_refused(
        &saved_from(|world| world.register_component::<SharedA>().unwrap()),
        |world| world.register_component::<SharedB>().unwrap(),
        SavedError::TypeMismatch {
            name: "test::Shared".to_string(),
        },
    );

    assert_refused(
        &saved_from(|_| {}),
        |world| world.register_resource(Score(1)).unwrap(),
        SavedError::MissingStorage {
            name: Score::NAME.to_string(),
        },
    );
    assert_refused(
        &saved_from(|world| world.register_resource(Score(1)).unwrap()),
        |_| {},
        SavedError::UnexpectedStorage {
            name: Score::NAME.to_string(),
        },
    );
    assert_refused(
        &saved_from(|world| world.register_resource(SettingA(1)).unwrap()),
        |world| world.register_resource(SettingB(2)).unwrap(),
        SavedError::TypeMismatch {
            name: "test::Setting".to_string(),
        },
    );
}

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq)]
#[asset(name = "test::Table")]
struct TableA(u32);

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq)]
#[asset(name = "test::Table")]
struct TableB(u32);

#[test]
fn load_refuses_other_asset_types() {
    let with_stats = |world: &mut World| {
        world.register_asset::<Stats>().unwrap();
        let bytes = postcard::to_allocvec(&Stats(4)).unwrap();
        world.insert_asset(AssetId(1), Stats::NAME, &bytes).unwrap();
    };
    assert_refused(
        &saved_from(with_stats),
        |_| {},
        SavedError::UnexpectedStorage {
            name: Stats::NAME.to_string(),
        },
    );
    assert_refused(
        &saved_from(|_| {}),
        |world| world.register_asset::<Stats>().unwrap(),
        SavedError::MissingStorage {
            name: Stats::NAME.to_string(),
        },
    );
    assert_refused(
        &saved_from(|world| world.register_asset::<TableA>().unwrap()),
        |world| world.register_asset::<TableB>().unwrap(),
        SavedError::TypeMismatch {
            name: "test::Table".to_string(),
        },
    );
}

#[test]
fn saved_error_messages_name_the_type() {
    let message = SavedError::TypeMismatch {
        name: "test::Shared".to_string(),
    }
    .to_string();
    assert!(message.contains("`test::Shared`"), "{message}");
}
