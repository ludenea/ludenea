use std::collections::BTreeMap;
use std::fs;

use ludenea::cli::Trace;
use ludenea_app::{App, AssetTexts, NamedAssetTexts, NoAssets, SceneError, Simulation};
use ludenea_ecs::{Asset, AssetId, Component, Entity, Handle};
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

fn simulation() -> Simulation {
    let mut app = App::new();
    app.component::<Turret>()
        .component::<Target>()
        .component::<Tag>()
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
    let target = "\"scene_assets::Target\": ((index: 4, generation: 0))";
    let error = simulation()
        .load_scene(&scene(6, &[(2, target), (5, &tag(1))]))
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::UnknownLocal { entity: 2, component, local: 4 }
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
