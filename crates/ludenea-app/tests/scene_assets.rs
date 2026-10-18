use std::collections::BTreeMap;
use std::f32::consts::PI;
use std::fs;

use ludenea::cli::Trace;
use ludenea_app::{App, AssetTexts, NamedAssetTexts, NoAssets, SceneError, Simulation};
use ludenea_ecs::schema::Schema;
use ludenea_ecs::{Asset, AssetId, Component, Entity, Handle, Resource};
use serde::{Deserialize, Serialize};

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq)]
struct Stats {
    damage: u32,
    next: Option<Handle<Stats>>,
}

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq)]
struct Sound(u8);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Turret {
    stats: Handle<Stats>,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Target(Entity);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Tag(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
enum Slot {
    Armed(Handle<Stats>),
    Label(String),
}

fn simulation() -> Simulation {
    let mut app = App::new();
    app.component::<Turret>()
        .component::<Target>()
        .component::<Tag>()
        .component::<Slot>()
        .asset::<Stats>()
        .asset::<Sound>();
    app.build().unwrap()
}

fn stats(damage: u32, next: Option<u128>) -> String {
    let next = match next {
        Some(id) => format!("Some(\"{}\")", AssetId(id)),
        None => "None".to_string(),
    };
    format!("AssetFile(type: \"scene_assets::Stats\", value: (damage: {damage}, next: {next}))")
}

fn texts(assets: &[(u128, String)]) -> AssetTexts {
    assets
        .iter()
        .map(|(id, text)| (AssetId(*id), text.clone()))
        .collect()
}

/// A version 2 scene: one entity per `(id, components)` pair.
fn scene(next_id: u32, entities: &[(u32, &str)]) -> String {
    let entities: String = entities
        .iter()
        .map(|(id, components)| format!("(id: {id}, components: {{{components}}}),"))
        .collect();
    format!("Scene(version: 2, next_id: {next_id}, resources: {{}}, entities: [{entities}])")
}

fn turret(id: u128) -> String {
    format!("\"scene_assets::Turret\": (stats: \"{}\")", AssetId(id))
}

fn tag(value: u32) -> String {
    format!("\"scene_assets::Tag\": ({value})")
}

fn tagged(simulation: &Simulation, value: u32) -> Entity {
    let world = simulation.world();
    world
        .entities()
        .find(|&entity| world.get::<Tag>(entity).is_some_and(|tag| tag.0 == value))
        .unwrap()
}

fn addresses(simulation: &Simulation) -> Vec<Vec<u32>> {
    simulation.load_addresses().values().cloned().collect()
}

#[test]
fn version_1_scene_loads_unchanged() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let ron = fs::read_to_string(format!("{root}/examples/boxes/assets/boxes.scene.ron")).unwrap();
    let golden = Trace::decode(&fs::read(format!("{root}/golden/boxes.trace")).unwrap()).unwrap();
    let mut simulation = boxes::app().build().unwrap();
    simulation.load_scene(&ron).unwrap();
    assert_eq!(simulation.hash(), golden.hashes[0]);
}

#[test]
fn version_2_scene_round_trips_through_save() {
    let mut first = simulation();
    let ron = scene(
        12,
        &[
            (3, &tag(1)),
            (
                9,
                &format!(
                    "{}, \"scene_assets::Target\": ((index: 3, generation: 0))",
                    tag(2)
                ),
            ),
        ],
    );
    first.load_scene(&ron).unwrap();
    let saved = first.save_scene();
    assert!(
        saved.contains("version: 2") && saved.contains("next_id: 2"),
        "{saved}"
    );
    let mut second = simulation();
    second.load_scene(&saved).unwrap();
    assert_eq!(first.hash(), second.hash());
    assert_eq!(second.save_scene(), saved);
}

#[test]
fn reachable_assets_are_loaded_and_unreachable_ignored() {
    let mut simulation = simulation();
    let assets = texts(&[(1, stats(5, None)), (2, stats(6, None))]);
    let load = simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &assets)
        .unwrap();
    assert!(load.warnings.is_empty());
    assert_eq!(simulation.world().asset_ids(), [AssetId(1)]);
}

#[test]
fn assets_reached_through_assets_are_loaded() {
    let mut simulation = simulation();
    let assets = texts(&[
        (1, stats(5, Some(2))),
        (2, stats(6, None)),
        (3, stats(7, None)),
    ]);
    simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &assets)
        .unwrap();
    assert_eq!(simulation.world().asset_ids(), [AssetId(1), AssetId(2)]);
}

#[test]
fn handle_cycle_between_assets_loads_once() {
    let mut simulation = simulation();
    let assets = texts(&[(1, stats(5, Some(2))), (2, stats(6, Some(1)))]);
    let ron = scene(2, &[(0, &turret(1)), (1, &turret(2))]);
    simulation.load_scene_with(&ron, &assets).unwrap();
    assert_eq!(simulation.world().asset_ids(), [AssetId(1), AssetId(2)]);
    let mut reloaded = self::simulation();
    reloaded
        .load_scene_with(&ron, &simulation.assets_ron())
        .unwrap();
    assert_eq!(reloaded.hash(), simulation.hash());
}

#[test]
fn loading_a_scene_replaces_the_loaded_assets() {
    let mut simulation = simulation();
    let assets = texts(&[(1, stats(5, None))]);
    simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &assets)
        .unwrap();
    simulation.load_scene(&scene(1, &[(0, &tag(1))])).unwrap();
    assert!(simulation.world().asset_ids().is_empty());
}

#[test]
fn missing_asset_names_the_referencing_field() {
    let mut simulation = simulation();
    simulation.load_scene(&scene(1, &[(0, &tag(1))])).unwrap();
    let before = simulation.hash();
    let error = simulation
        .load_scene_with(&scene(8, &[(7, &turret(5))]), &NoAssets)
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::MissingAsset { referenced_by, id: AssetId(5) }
            if referenced_by == "entity 7, scene_assets::Turret.stats"),
        "{error:?}"
    );
    assert_eq!(simulation.hash(), before);

    let assets = texts(&[(1, stats(5, Some(4)))]);
    let error = simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &assets)
        .unwrap_err();
    let expected = format!("asset {}, scene_assets::Stats.next", AssetId(1));
    assert!(
        matches!(&error, SceneError::MissingAsset { referenced_by, id: AssetId(4) }
            if *referenced_by == expected),
        "{error:?}"
    );
    assert_eq!(simulation.hash(), before);
}

#[test]
fn wrong_asset_type_is_refused_with_path() {
    let mut simulation = simulation();
    let assets = NamedAssetTexts {
        texts: texts(&[(
            1,
            "AssetFile(type: \"scene_assets::Sound\", value: (3))".into(),
        )]),
        paths: BTreeMap::from([(AssetId(1), "sounds/boom.ron".to_string())]),
    };
    let error = simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &assets)
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::WrongAssetType { id: AssetId(1), path: Some(path), referenced_by, expected, found }
            if path == "sounds/boom.ron"
                && referenced_by == "entity 0, scene_assets::Turret.stats"
                && expected == "scene_assets::Stats"
                && found == "scene_assets::Sound"),
        "{error:?}"
    );
    assert!(error.to_string().contains("sounds/boom.ron"), "{error}");
}

#[test]
fn invalid_asset_value_is_refused_and_leaves_state() {
    let mut simulation = simulation();
    simulation.load_scene(&scene(1, &[(0, &tag(1))])).unwrap();
    let before = simulation.hash();
    let assets = texts(&[(
        1,
        "AssetFile(type: \"scene_assets::Stats\", value: (damage: \"x\", next: None))".into(),
    )]);
    let error = simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &assets)
        .unwrap_err();
    assert!(
        matches!(
            &error,
            SceneError::InvalidAsset {
                id: AssetId(1),
                path: None,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(simulation.hash(), before);
}

#[test]
fn entity_values_name_local_identifiers() {
    let mut simulation = simulation();
    let target = format!(
        "{}, \"scene_assets::Target\": ((index: 5, generation: 0))",
        tag(2)
    );
    simulation
        .load_scene(&scene(6, &[(2, &target), (5, &tag(1))]))
        .unwrap();
    let pointing = tagged(&simulation, 2);
    let target = simulation.world().get::<Target>(pointing).unwrap().0;
    assert_eq!(target, tagged(&simulation, 1));
}

#[test]
fn unknown_local_identifier_is_refused() {
    let target = "\"scene_assets::Target\": ((index: 6, generation: 0))";
    let error = simulation()
        .load_scene(&scene(6, &[(2, target), (5, &tag(1))]))
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::UnknownLocal { entity: 2, component, local: 6 }
            if component == "scene_assets::Target"),
        "{error:?}"
    );
}

#[test]
fn duplicate_local_identifier_is_refused() {
    let error = simulation()
        .load_scene(&scene(4, &[(3, &tag(1)), (3, &tag(2))]))
        .unwrap_err();
    assert!(matches!(error, SceneError::DuplicateLocal(3)), "{error:?}");
}

#[test]
fn load_addresses_map_spawned_entities() {
    let mut simulation = simulation();
    simulation
        .load_scene(&scene(5, &[(4, &tag(4)), (1, &tag(1))]))
        .unwrap();
    let four = tagged(&simulation, 4);
    let one = tagged(&simulation, 1);
    assert_eq!(
        simulation.load_addresses(),
        &BTreeMap::from([(four, vec![4]), (one, vec![1])])
    );
    assert_eq!(simulation.entity_at_address(&[1]), Some(one));
    assert_eq!(simulation.entity_at_address(&[9]), None);
}

#[test]
fn save_keeping_preserves_identifiers_after_despawn() {
    let mut simulation = simulation();
    let previous = scene(10, &[(4, &tag(4)), (1, &tag(1)), (6, &tag(6))]);
    simulation.load_scene(&previous).unwrap();
    let one = simulation.entity_at_address(&[1]).unwrap();
    simulation.world_mut().despawn(one).unwrap();

    let saved = simulation.save_scene_keeping(&previous);
    assert!(saved.contains("next_id: 10"), "{saved}");
    simulation.load_scene(&saved).unwrap();
    assert_eq!(addresses(&simulation), [vec![4], vec![6]]);
    assert_eq!(
        simulation.entity_at_address(&[6]),
        Some(tagged(&simulation, 6))
    );
}

#[test]
fn deleted_local_identifier_is_never_reused() {
    let mut simulation = simulation();
    let previous = scene(3, &[(0, &tag(0)), (1, &tag(1)), (2, &tag(2))]);
    simulation.load_scene(&previous).unwrap();
    let world = simulation.world_mut();
    let highest = world.entities().last().unwrap();
    world.despawn(highest).unwrap();
    let spawned = world.spawn();
    world.insert(spawned, Tag(99)).unwrap();

    let saved = simulation.save_scene_keeping(&previous);
    assert!(saved.contains("next_id: 4"), "{saved}");
    simulation.load_scene(&saved).unwrap();
    assert_eq!(
        simulation.entity_at_address(&[3]),
        Some(tagged(&simulation, 99))
    );
    assert_eq!(simulation.entity_at_address(&[2]), None);
}

#[test]
fn asset_value_error_gives_the_file_line_and_column() {
    let mut simulation = simulation();
    let text = "AssetFile(\n    type: \"scene_assets::Stats\",\n    value: (\n        damage: \"x\",\n        next: None,\n    ),\n)\n";
    let assets = NamedAssetTexts {
        texts: texts(&[(1, text.to_string())]),
        paths: BTreeMap::from([(AssetId(1), "stats/broken.ron".to_string())]),
    };
    let error = simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &assets)
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::InvalidAsset { id: AssetId(1), path: Some(path), reason }
            if path == "stats/broken.ron" && reason.starts_with("4:16-4:17")),
        "{error:?}"
    );
    assert!(error.to_string().contains("stats/broken.ron"), "{error}");
}

#[test]
fn asset_syntax_error_in_value_gives_the_file_line() {
    let mut simulation = simulation();
    let text =
        "AssetFile(\n    type: \"scene_assets::Stats\",\n    value: (damage: 1, next: None),\n)\n";
    let broken = text.replace("next: None", "next: None,,");
    let error = simulation
        .load_scene_with(&scene(1, &[(0, &turret(1))]), &texts(&[(1, broken)]))
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::InvalidAsset { reason, .. } if reason.starts_with("3:")),
        "{error:?}"
    );
}

#[test]
fn stale_reference_survives_save_and_reload() {
    let mut simulation = simulation();
    let target = format!(
        "{}, \"scene_assets::Target\": ((index: 1, generation: 0))",
        tag(0)
    );
    simulation
        .load_scene(&scene(2, &[(0, &target), (1, &tag(1))]))
        .unwrap();
    let world = simulation.world_mut();
    let despawned = tagged_in(world, 1);
    world.despawn(despawned).unwrap();
    let spawned = world.spawn();
    world.insert(spawned, Tag(2)).unwrap();
    world.insert(spawned, Target(despawned)).unwrap();

    for saved in [
        simulation.save_scene(),
        simulation.save_scene_keeping(&scene(2, &[])),
    ] {
        let mut reloaded = self::simulation();
        reloaded.load_scene(&saved).unwrap();
        let world = reloaded.world();
        let first = world.get::<Target>(tagged(&reloaded, 0)).unwrap().0;
        let second = world.get::<Target>(tagged(&reloaded, 2)).unwrap().0;
        assert!(!world.is_alive(first), "{saved}");
        assert_eq!(first, second, "one stale entity stays one: {saved}");
        let again = reloaded.save_scene();
        self::simulation().load_scene(&again).unwrap();
    }
}

#[test]
fn deleted_local_identifier_below_next_id_loads_as_a_dead_entity() {
    let mut simulation = simulation();
    let target = format!(
        "{}, \"scene_assets::Target\": ((index: 3, generation: 0))",
        tag(0)
    );
    simulation.load_scene(&scene(5, &[(0, &target)])).unwrap();
    let world = simulation.world();
    let target = world.get::<Target>(tagged(&simulation, 0)).unwrap().0;
    assert!(!world.is_alive(target));
}

fn tagged_in(world: &ludenea_ecs::World, value: u32) -> Entity {
    world
        .entities()
        .find(|&entity| world.get::<Tag>(entity).is_some_and(|tag| tag.0 == value))
        .unwrap()
}

#[derive(Serialize, Deserialize, Schema, Clone, Debug, PartialEq)]
enum Mood {
    Calm,
    Angry(u8, f32),
    Watching { name: String, at: Entity },
}

/// `Entity` values after and inside every shape the postcard walker crosses.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Mixed {
    label: String,
    scale: f32,
    precise: f64,
    signed: i64,
    calm: Mood,
    angry: Mood,
    after: Entity,
    watching: Mood,
    maybe: Option<Entity>,
    none: Option<Entity>,
    list: Vec<Entity>,
    named: BTreeMap<String, Entity>,
    keyed: BTreeMap<Entity, u16>,
    pair: (u16, Entity),
    array: [Entity; 2],
    stats: Handle<Stats>,
    last: Entity,
}

#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Boss(Option<Entity>);

fn mixed_simulation() -> Simulation {
    let mut app = App::new();
    app.component::<Mixed>()
        .component::<Tag>()
        .asset::<Stats>()
        .resource(Boss(None));
    app.build().unwrap()
}

#[test]
fn entities_inside_mixed_values_are_remapped_and_round_trip() {
    let entity = |local: u32| format!("(index: {local}, generation: 0)");
    let mixed = format!(
        "\"scene_assets::Mixed\": (label: \"héllo\", scale: {PI}, precise: -0.1, signed: -300, \
         calm: Calm, angry: Angry(7, -2.5), after: {a}, \
         watching: Watching(name: \"w\", at: {b}), maybe: Some({b}), none: None, \
         list: [{a}, {b}, {a}], named: {{\"x\": {b}, \"y\": {a}}}, keyed: {{{b}: 1, {a}: 2}}, \
         pair: (9, {b}), array: ({a}, {b}), stats: \"{stats}\", last: {b})",
        a = entity(40),
        b = entity(300),
        stats = AssetId(1),
    );
    let ron = format!(
        "Scene(version: 2, next_id: 301, resources: {{\"scene_assets::Boss\": (Some({}))}}, \
         entities: [(id: 300, components: {{{}}}), (id: 40, components: {{{mixed}}})])",
        entity(40),
        tag(300),
    );
    let assets = texts(&[(1, stats(5, None))]);
    let mut first = mixed_simulation();
    first.load_scene_with(&ron, &assets).unwrap();

    let world = first.world();
    let a = first.entity_at_address(&[40]).unwrap();
    let b = first.entity_at_address(&[300]).unwrap();
    let value = world.get::<Mixed>(a).unwrap().clone();
    let expected = Mixed {
        label: "héllo".into(),
        scale: PI,
        precise: -0.1,
        signed: -300,
        calm: Mood::Calm,
        angry: Mood::Angry(7, -2.5),
        after: a,
        watching: Mood::Watching {
            name: "w".into(),
            at: b,
        },
        maybe: Some(b),
        none: None,
        list: vec![a, b, a],
        named: BTreeMap::from([("x".into(), b), ("y".into(), a)]),
        keyed: BTreeMap::from([(b, 1), (a, 2)]),
        pair: (9, b),
        array: [a, b],
        stats: Handle::new(AssetId(1)),
        last: b,
    };
    assert_eq!(value, expected);
    assert_eq!(world.resource::<Boss>().unwrap().0, Some(a));

    let saved = first.save_scene();
    let mut second = mixed_simulation();
    second.load_scene_with(&saved, &assets).unwrap();
    assert_eq!(second.hash(), first.hash());
    assert_eq!(second.save_scene(), saved);
}

#[test]
fn handles_are_found_in_the_enum_variant_written_only() {
    let id = AssetId(5);
    let mut simulation = simulation();
    let label = format!("\"scene_assets::Slot\": Label(\"{id}\")");
    simulation
        .load_scene_with(&scene(1, &[(0, &label)]), &NoAssets)
        .unwrap();
    assert!(simulation.world().asset_ids().is_empty());

    let armed = format!("\"scene_assets::Slot\": Armed(\"{id}\")");
    let error = simulation
        .load_scene_with(&scene(1, &[(0, &armed)]), &NoAssets)
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::MissingAsset { referenced_by, id: AssetId(5) }
            if referenced_by == "entity 0, scene_assets::Slot.Armed"),
        "{error:?}"
    );
}

#[test]
fn a_resource_and_a_component_may_share_a_stable_name() {
    #[derive(Resource, Serialize, Deserialize)]
    #[resource(name = "test::Same")]
    struct Pair(Option<Entity>, Option<Entity>);

    #[derive(Component, Serialize, Deserialize)]
    #[component(name = "test::Same")]
    struct One(Entity);

    let mut app = App::new();
    app.component::<One>()
        .component::<Tag>()
        .resource(Pair(None, None));
    let mut simulation = app.build().unwrap();
    let pair = "Some((index: 0, generation: 0))";
    let ron = format!(
        "Scene(version: 2, next_id: 2, resources: {{\"test::Same\": ({pair}, {pair})}}, entities: [(id: 0, components: {{{}}}), (id: 1, components: {{\"test::Same\": ((index: 1, generation: 0))}})])",
        tag(1)
    );
    simulation.load_scene(&ron).unwrap();
    let world = simulation.world();
    let first = tagged(&simulation, 1);
    assert_eq!(world.resource::<Pair>().unwrap().1, Some(first));
    let holder = world.entities().find(|&e| e != first).unwrap();
    assert_eq!(world.get::<One>(holder).unwrap().0, holder);
}

#[test]
fn a_next_id_that_cannot_grow_is_refused_at_load() {
    let mut simulation = simulation();
    let error = simulation
        .load_scene(&scene(u32::MAX, &[(0, &tag(1))]))
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::Parse(reason) if reason.contains("next_id")),
        "{error}"
    );
}

#[test]
#[should_panic(expected = "fewer than u32::MAX local identifiers")]
fn saving_past_the_last_local_identifier_panics_rather_than_reuse_one() {
    let mut simulation = simulation();
    let ron = scene(u32::MAX - 1, &[(u32::MAX - 2, &tag(1))]);
    simulation.load_scene(&ron).unwrap();
    simulation.world_mut().spawn();
    simulation.world_mut().spawn();
    simulation.save_scene_keeping(&ron);
}
