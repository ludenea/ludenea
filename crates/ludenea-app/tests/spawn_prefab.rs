use ludenea_app::replay::{ReplayOutcome, play_replay, record_replay};
use ludenea_app::{App, AssetTexts, EditLog, InputLog, Rng, Sim, Simulation, Time};
use ludenea_ecs::{
    AssetId, CommandKind, CommandRecord, Commands, Component, Entity, Handle, IntoSystemConfig,
    Prefab, Query, Res, ResMut, ScheduleObserver, World,
};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Tag(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Target(Entity);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Pos {
    x: f32,
    y: f32,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Aim {
    target: Entity,
    angle: f32,
    spread: Vec<f32>,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Pair(Entity, f32, Entity);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Spawner {
    bullet: Handle<Prefab>,
    every: u64,
}

const BULLET: u128 = 0xb;

fn spawn_bullets(
    spawners: Query<&Spawner>,
    time: Res<Time>,
    mut rng: ResMut<Rng>,
    mut commands: Commands,
) {
    for spawner in spawners.iter() {
        if time.tick.is_multiple_of(spawner.every) {
            let x = rng.range_u32(0, 100);
            commands.spawn_prefab(spawner.bullet, move |overrides| {
                overrides.set::<Pos>(0, move |pos| pos.x = x as f32);
            });
        }
    }
}

fn move_bullets(mut bullets: Query<&mut Pos>, mut rng: ResMut<Rng>) {
    for pos in bullets.iter_mut() {
        pos.y += rng.range_u32(1, 4) as f32;
    }
}

fn simulation(every: u64) -> Simulation {
    simulation_with(every, spawn_bullets)
}

fn simulation_with<M>(every: u64, spawn: impl IntoSystemConfig<M>) -> Simulation {
    let mut simulation = unloaded(spawn);
    let (scene, assets) = bullet_scene(every);
    simulation.load_scene_with(&scene, &assets).unwrap();
    simulation
}

fn unloaded<M>(spawn: impl IntoSystemConfig<M>) -> Simulation {
    let mut app = App::new();
    app.component::<Tag>()
        .component::<Target>()
        .component::<Pos>()
        .component::<Aim>()
        .component::<Pair>()
        .component::<Spawner>()
        .system(Sim, spawn)
        .system(Sim, move_bullets);
    app.build().unwrap()
}

/// A scene with one spawner of the bullet prefab, and its assets.
fn bullet_scene(every: u64) -> (String, AssetTexts) {
    let bullet = "Prefab(version: 1, next_id: 2, entities: [\
        (id: 0, components: {\"spawn_prefab::Tag\": (1), \"spawn_prefab::Pos\": (x: 0.0, y: 0.0), \
         \"spawn_prefab::Target\": ((index: 1, generation: 0))}),\
        (id: 1, components: {\"spawn_prefab::Tag\": (2), \"spawn_prefab::Target\": ((index: 0, generation: 0))}),\
        ], instances: [])";
    let assets: AssetTexts = [(AssetId(BULLET), bullet.to_string())]
        .into_iter()
        .collect();
    let scene = format!(
        "Scene(version: 2, next_id: 1, resources: {{}}, entities: [\
         (id: 0, components: {{\"spawn_prefab::Spawner\": (bullet: \"{}\", every: {every})}})], instances: [])",
        AssetId(BULLET)
    );
    (scene, assets)
}

fn tagged(world: &World, value: u32) -> Vec<Entity> {
    world
        .entities()
        .filter(|&entity| world.get::<Tag>(entity).is_some_and(|tag| tag.0 == value))
        .collect()
}

#[test]
fn spawn_prefab_spawns_entities_in_prefab_order() {
    let mut simulation = simulation(1000);
    simulation.step();
    let world = simulation.world();
    let (first, second) = (tagged(world, 1), tagged(world, 2));
    assert_eq!((first.len(), second.len()), (1, 1));
    assert_eq!(second[0].index(), first[0].index() + 1);
}

#[test]
fn spawned_entity_fields_point_to_new_entities() {
    let mut simulation = simulation(1000);
    simulation.step();
    let world = simulation.world();
    let (first, second) = (tagged(world, 1)[0], tagged(world, 2)[0]);
    assert_eq!(world.get::<Target>(first).unwrap().0, second);
    assert_eq!(world.get::<Target>(second).unwrap().0, first);
}

#[test]
fn overrides_apply_after_spawn() {
    fn spawn(spawners: Query<&Spawner>, mut commands: Commands) {
        for spawner in spawners.iter() {
            commands.spawn_prefab(spawner.bullet, |overrides| {
                overrides
                    .set::<Tag>(0, |tag| tag.0 += 10)
                    .insert(1, Pos { x: 5.0, y: 5.0 })
                    .set::<Pos>(1, |pos| pos.x += 1.0);
            });
        }
    }
    let mut simulation = simulation_with(1000, spawn);
    simulation.step();
    let world = simulation.world();
    let first = tagged(world, 11);
    assert_eq!(first.len(), 1);
    let second = world.get::<Target>(first[0]).unwrap().0;
    assert_eq!(world.get::<Tag>(second).unwrap().0, 2);
    assert_eq!(world.get::<Target>(second).unwrap().0, first[0]);
    assert_eq!(world.get::<Pos>(second).unwrap().x, 6.0);
}

#[test]
fn spawn_prefab_is_deterministic_across_thread_counts() {
    let hashes = |threads| {
        let mut simulation = simulation(7);
        simulation.set_threads(threads);
        (0..200)
            .map(|_| {
                simulation.step();
                simulation.hash()
            })
            .collect::<Vec<_>>()
    };
    let single = hashes(1);
    assert_eq!(single, hashes(8));
    let mut simulation = simulation(7);
    for _ in 0..200 {
        simulation.step();
    }
    assert_eq!(tagged(simulation.world(), 1).len(), 29);
}

#[test]
fn spawn_prefab_survives_snapshot_and_restore() {
    let mut simulation = simulation(5);
    for _ in 0..50 {
        simulation.step();
    }
    let snapshot = simulation.snapshot();
    let run = |simulation: &mut Simulation| {
        (0..50)
            .map(|_| {
                simulation.step();
                simulation.hash()
            })
            .collect::<Vec<_>>()
    };
    let first = run(&mut simulation);
    simulation.restore(&snapshot).unwrap();
    assert_eq!(run(&mut simulation), first);
}

/// A replay stores the prefab a component's handle names in flattened form.
#[test]
fn replay_of_a_scene_holding_a_prefab_handle_plays_back() {
    let (scene, assets) = bullet_scene(5);
    let mut recording = unloaded(spawn_bullets);
    let replay = record_replay(
        &mut recording,
        &scene,
        &assets,
        &InputLog::new(1),
        &EditLog::new(),
        50,
    )
    .unwrap();
    let report = play_replay(&mut unloaded(spawn_bullets), &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
}

#[derive(Default)]
struct Recorder(Vec<CommandRecord>);

impl ScheduleObserver for Recorder {
    fn before_system(&mut self, _: usize) {}

    fn after_system(&mut self, _: usize, commands: &[CommandRecord]) {
        self.0.extend_from_slice(commands);
    }

    fn tick_end(&mut self, _: &World) {}
}

#[test]
fn spawn_records_commands() {
    let mut simulation = simulation(1000);
    let mut recorder = Recorder::default();
    simulation.step_observed(None, &mut recorder).unwrap();
    let world = simulation.world();
    let spawned = vec![tagged(world, 1)[0], tagged(world, 2)[0]];
    let records: Vec<_> = recorder
        .0
        .iter()
        .filter(|record| record.kind == CommandKind::Spawn)
        .map(|record| record.entity.unwrap())
        .collect();
    assert_eq!(records, spawned);
}

#[test]
#[should_panic(expected = "has no entity with local identifier 9")]
fn unknown_local_identifier_panics() {
    fn spawn(spawners: Query<&Spawner>, mut commands: Commands) {
        for spawner in spawners.iter() {
            commands.spawn_prefab(spawner.bullet, |overrides| {
                overrides.set::<Tag>(9, |tag| tag.0 = 0);
            });
        }
    }
    simulation_with(1000, spawn).step();
}

#[test]
fn remap_keeps_non_finite_floats() {
    let mut simulation = simulation(1000);
    simulation.step();
    let world = simulation.world();
    let (entity, other) = (tagged(world, 1)[0], tagged(world, 2)[0]);
    let aim = Aim {
        target: entity,
        angle: f32::NAN,
        spread: vec![f32::INFINITY, f32::NEG_INFINITY],
    };
    let codec = world.component_codec("spawn_prefab::Aim").unwrap();
    let bytes = postcard::to_allocvec(&aim).unwrap();
    let remapped = (codec.remap_entities)(&bytes, &|_| other);
    let back: Aim = postcard::from_bytes(&remapped).unwrap();
    assert_eq!(back.target, other);
    assert_eq!(back.angle.to_bits(), f32::NAN.to_bits());
    assert_eq!(back.spread, aim.spread);
}

#[test]
fn spawn_records_each_inserted_component() {
    let mut simulation = simulation(1000);
    let mut recorder = Recorder::default();
    simulation.step_observed(None, &mut recorder).unwrap();
    let world = simulation.world();
    let (first, second) = (tagged(world, 1)[0], tagged(world, 2)[0]);
    let inserted: Vec<_> = recorder
        .0
        .iter()
        .filter(|record| record.kind == CommandKind::Insert)
        .map(|record| (record.entity.unwrap(), record.component.unwrap()))
        .collect();
    assert_eq!(
        inserted,
        [
            (first, "spawn_prefab::Tag"),
            (first, "spawn_prefab::Pos"),
            (first, "spawn_prefab::Target"),
            (second, "spawn_prefab::Tag"),
            (second, "spawn_prefab::Target"),
        ]
    );
}

#[test]
fn components_outside_entity_components_are_not_remapped() {
    let mut simulation = simulation(1000);
    let id = AssetId(BULLET);
    let (_, bytes) = simulation.world().asset_bytes(id).unwrap();
    let mut prefab: Prefab = postcard::from_bytes(&bytes).unwrap();
    prefab.entity_components.clear();
    let bytes = postcard::to_allocvec(&prefab).unwrap();
    simulation
        .world_mut()
        .insert_asset(id, "ludenea::Prefab", &bytes)
        .unwrap();
    simulation.step();
    let world = simulation.world();
    let first = tagged(world, 1)[0];
    // Position 1 is written as is: the raw entity 1v0, which is `first` itself here.
    assert_eq!(world.get::<Target>(first).unwrap().0, first);
    assert_ne!(tagged(world, 2)[0], first);
}

#[test]
fn remap_maps_each_entity_on_its_own_and_survives_a_panic() {
    let mut simulation = simulation(1000);
    simulation.step();
    let world = simulation.world();
    let (first, second) = (tagged(world, 1)[0], tagged(world, 2)[0]);
    let codec = world.component_codec("spawn_prefab::Pair").unwrap();
    let pair = Pair(first, f32::NAN, second);
    let bytes = postcard::to_allocvec(&pair).unwrap();
    let swap = |entity| if entity == first { second } else { first };

    let mut trailing = bytes.clone();
    trailing.push(0);
    let panicked = std::panic::catch_unwind(|| (codec.remap_entities)(&trailing, &swap));
    assert!(panicked.is_err());

    let remapped: Pair = postcard::from_bytes(&(codec.remap_entities)(&bytes, &swap)).unwrap();
    assert_eq!((remapped.0, remapped.2), (second, first));
    assert_eq!(remapped.1.to_bits(), f32::NAN.to_bits());
    let plain: Pair = postcard::from_bytes(&bytes).unwrap();
    assert_eq!((plain.0, plain.2), (first, second));
}
