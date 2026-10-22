use std::collections::BTreeMap;
use std::fs;

use ludenea::cli::Trace;
use ludenea_app::path::PathSegment;
use ludenea_app::prefab::{InstanceInfo, OverrideInfo, resolve};
use ludenea_app::{App, AssetTexts, NamedAssetTexts, NoAssets, SceneError, Simulation};
use ludenea_ecs::{Asset, AssetId, Component, Entity, Handle, Prefab};
use serde::{Deserialize, Serialize};

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq)]
struct Stats(u32);

#[derive(Clone, Component, Serialize, Deserialize, Debug, PartialEq)]
struct Tag(u32);

#[derive(Clone, Component, Serialize, Deserialize, Debug, PartialEq)]
struct Target(Entity);

#[derive(Clone, Component, Serialize, Deserialize, Debug, PartialEq)]
struct Pos {
    x: f32,
    y: f32,
}

#[derive(Clone, Component, Serialize, Deserialize, Debug, PartialEq)]
enum Shape {
    Circle { radius: f32 },
    Square(f32),
}

#[derive(Clone, Component, Serialize, Deserialize, Debug, PartialEq)]
struct Spawner {
    bullet: Handle<Prefab>,
}

#[derive(Clone, Component, Serialize, Deserialize, Debug, PartialEq)]
struct Turret {
    stats: Handle<Stats>,
}

const A: u128 = 0xa;
const B: u128 = 0xb;
const C: u128 = 0xc;

fn simulation() -> Simulation {
    let mut app = App::new();
    app.component::<Tag>()
        .component::<Target>()
        .component::<Pos>()
        .component::<Shape>()
        .component::<Spawner>()
        .component::<Turret>()
        .asset::<Stats>();
    app.build().unwrap()
}

fn entities(entities: &[(u32, &str)]) -> String {
    entities
        .iter()
        .map(|(id, components)| format!("(id: {id}, components: {{{components}}}),"))
        .collect()
}

fn scene(next_id: u32, own: &[(u32, &str)], instances: &str) -> String {
    format!(
        "Scene(version: 2, next_id: {next_id}, resources: {{}}, entities: [{}], instances: [{instances}])",
        entities(own)
    )
}

fn prefab(next_id: u32, own: &[(u32, &str)], instances: &str) -> String {
    format!(
        "Prefab(version: 1, next_id: {next_id}, entities: [{}], instances: [{instances}])",
        entities(own)
    )
}

fn instance(id: u32, prefab: u128, overrides: &str, added: &str, removed: &str) -> String {
    format!(
        "(id: {id}, prefab: \"{}\", overrides: [{overrides}], added: [{added}], removed: [{removed}]),",
        AssetId(prefab)
    )
}

fn texts(assets: &[(u128, String)]) -> AssetTexts {
    assets
        .iter()
        .map(|(id, text)| (AssetId(*id), text.clone()))
        .collect()
}

fn tag(value: u32) -> String {
    format!("\"prefab::Tag\": ({value})")
}

fn target(local: u32) -> String {
    format!("\"prefab::Target\": ((index: {local}, generation: 0))")
}

fn tagged(simulation: &Simulation, value: u32) -> Entity {
    let world = simulation.world();
    world
        .entities()
        .find(|&entity| world.get::<Tag>(entity).is_some_and(|tag| tag.0 == value))
        .unwrap()
}

fn tag_of(simulation: &Simulation, entity: Entity) -> u32 {
    simulation.world().get::<Tag>(entity).unwrap().0
}

fn flattened(simulation: &Simulation, id: u128) -> Prefab {
    let (type_name, bytes) = simulation.world().asset_bytes(AssetId(id)).unwrap();
    assert_eq!(type_name, "ludenea::Prefab");
    postcard::from_bytes(&bytes).unwrap()
}

#[test]
fn scene_without_instances_resolves_to_itself() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let ron = fs::read_to_string(format!("{root}/examples/boxes/assets/boxes.scene.ron")).unwrap();
    let golden = Trace::decode(&fs::read(format!("{root}/golden/boxes.trace")).unwrap()).unwrap();
    let resolved = resolve(&ron, &NoAssets).unwrap();
    assert!(resolved.warnings.is_empty());
    assert!(
        resolved
            .entities
            .iter()
            .enumerate()
            .all(|(position, entity)| entity.address == [position as u32])
    );

    let mut simulation = boxes::app().build().unwrap();
    simulation.load_scene(&ron).unwrap();
    assert_eq!(simulation.hash(), golden.hashes[0]);
    let mut again = boxes::app().build().unwrap();
    again.load_scene(&resolved.to_scene_text()).unwrap();
    assert_eq!(again.hash(), golden.hashes[0]);
}

#[test]
fn instance_expands_after_scene_entities_in_prefab_order() {
    let mut simulation = simulation();
    let assets = texts(&[(A, prefab(6, &[(5, &tag(10)), (2, &tag(11))], ""))]);
    let file = scene(
        4,
        &[(0, &tag(1))],
        &(instance(1, A, "", "", "") + &instance(3, A, "", "", "")),
    );
    let resolved = resolve(&file, &assets).unwrap();
    let addresses: Vec<_> = resolved
        .entities
        .iter()
        .map(|e| e.address.clone())
        .collect();
    assert_eq!(
        addresses,
        [vec![0], vec![1, 5], vec![1, 2], vec![3, 5], vec![3, 2]]
    );

    simulation.load_scene_with(&file, &assets).unwrap();
    let world = simulation.world();
    let tags: Vec<u32> = world.entities().map(|e| tag_of(&simulation, e)).collect();
    assert_eq!(tags, [1, 10, 11, 10, 11]);
    let order: Vec<Entity> = world.entities().collect();
    assert_eq!(simulation.entity_at_address(&[3, 2]), Some(order[4]));
    assert_eq!(
        simulation.instances(),
        [
            InstanceInfo {
                address: vec![1],
                prefab: AssetId(A),
                entities: order[1..3].to_vec(),
                overrides: vec![],
            },
            InstanceInfo {
                address: vec![3],
                prefab: AssetId(A),
                entities: order[3..5].to_vec(),
                overrides: vec![],
            },
        ]
    );
}

#[test]
fn override_sets_one_field() {
    let mut simulation = simulation();
    let components = "\"prefab::Pos\": (x: 1.0, y: 2.0), \"prefab::Shape\": Circle(radius: 1.0)";
    let assets = texts(&[(A, prefab(1, &[(0, components)], ""))]);
    let overrides = "(entity: [0], component: \"prefab::Pos\", path: [(type: \"field\", name: \"y\")], value: 7.5),\
        (entity: [0], component: \"prefab::Shape\", path: [(type: \"field\", name: \"Circle\"), (type: \"field\", name: \"radius\")], value: 3.0),";
    let file = scene(1, &[], &instance(0, A, overrides, "", ""));
    let load = simulation.load_scene_with(&file, &assets).unwrap();
    assert!(load.warnings.is_empty(), "{:?}", load.warnings);
    let entity = simulation.entity_at_address(&[0, 0]).unwrap();
    let world = simulation.world();
    assert_eq!(*world.get::<Pos>(entity).unwrap(), Pos { x: 1.0, y: 7.5 });
    assert_eq!(
        *world.get::<Shape>(entity).unwrap(),
        Shape::Circle { radius: 3.0 }
    );
}

#[test]
fn added_and_removed_components_apply() {
    let mut simulation = simulation();
    let components = format!("{}, \"prefab::Pos\": (x: 1.0, y: 2.0)", tag(1));
    let assets = texts(&[(A, prefab(1, &[(0, &components)], ""))]);
    let added = "(entity: [0], component: \"prefab::Shape\", value: Square(2.0)),";
    let removed = "(entity: [0], component: \"prefab::Pos\"),";
    let file = scene(1, &[], &instance(0, A, "", added, removed));
    let load = simulation.load_scene_with(&file, &assets).unwrap();
    assert!(load.warnings.is_empty(), "{:?}", load.warnings);
    let entity = tagged(&simulation, 1);
    let world = simulation.world();
    assert!(world.get::<Pos>(entity).is_none());
    assert_eq!(*world.get::<Shape>(entity).unwrap(), Shape::Square(2.0));
}

#[test]
fn instances_report_their_overrides_stale_ones_flagged() {
    let mut simulation = simulation();
    let pos = "\"prefab::Pos\": (x: 1.0, y: 2.0)";
    let assets = texts(&[
        (B, prefab(2, &[(1, pos)], "")),
        (
            A,
            prefab(
                4,
                &[(0, pos)],
                &instance(
                    3,
                    B,
                    "(entity: [1], component: \"prefab::Pos\", path: [], value: (x: 0.0, y: 0.0)),",
                    "",
                    "",
                ),
            ),
        ),
    ]);
    // Comments between fields, as hand-edited files keep them.
    let overrides = "(entity: [0], /* own */ component: \"prefab::Pos\", // x only\n path: [(type: \"field\", name: \"x\")], value: 5.0),\
        (entity: [3, 1], component: \"prefab::Pos\", path: [(type: \"field\", name: \"y\")], value: 6.0),\
        (entity: [9], component: \"prefab::Pos\", path: [], value: (x: 0.0, y: 0.0)),";
    let file = scene(8, &[], &instance(7, A, overrides, "", ""));
    let load = simulation.load_scene_with(&file, &assets).unwrap();
    assert_eq!(load.warnings.len(), 1, "{:?}", load.warnings);

    let field = |name: &str| PathSegment::Field {
        name: name.to_string(),
    };
    let target = |entity: Vec<u32>, path: Vec<PathSegment>, stale: bool| OverrideInfo {
        entity,
        component: "prefab::Pos".to_string(),
        path,
        stale,
    };
    let instances = simulation.instances();
    assert_eq!(
        instances[0].overrides,
        [
            target(vec![0], vec![field("x")], false),
            target(vec![3, 1], vec![field("y")], false),
            target(vec![9], vec![], true),
        ]
    );
    // The nested instance's own override belongs to the prefab file, not to the scene.
    assert_eq!(instances[1].address, [7, 3]);
    assert!(instances[1].overrides.is_empty());
}

#[test]
fn nested_instance_override_addresses_by_path() {
    let mut simulation = simulation();
    let assets = texts(&[
        (B, prefab(2, &[(1, &tag(20))], "")),
        (A, prefab(4, &[(0, &tag(30))], &instance(3, B, "", "", ""))),
    ]);
    let overrides = "(entity: [3, 1], component: \"prefab::Tag\", path: [], value: (99)),";
    let file = scene(8, &[], &instance(7, A, overrides, "", ""));
    let resolved = resolve(&file, &assets).unwrap();
    let addresses: Vec<_> = resolved
        .entities
        .iter()
        .map(|e| e.address.clone())
        .collect();
    assert_eq!(addresses, [vec![7, 0], vec![7, 3, 1]]);

    simulation.load_scene_with(&file, &assets).unwrap();
    let nested = simulation.entity_at_address(&[7, 3, 1]).unwrap();
    assert_eq!(tag_of(&simulation, nested), 99);
    let addresses: Vec<_> = simulation
        .instances()
        .into_iter()
        .map(|info| info.address)
        .collect();
    assert_eq!(addresses, [vec![7], vec![7, 3]]);
}

#[test]
fn entity_values_are_remapped_per_instance() {
    let mut simulation = simulation();
    let first = format!("{}, {}", tag(1), target(1));
    let assets = texts(&[(A, prefab(2, &[(0, &first), (1, &tag(2))], ""))]);
    let file = scene(
        6,
        &[],
        &(instance(4, A, "", "", "") + &instance(5, A, "", "", "")),
    );
    simulation.load_scene_with(&file, &assets).unwrap();
    for id in [4, 5] {
        let source = simulation.entity_at_address(&[id, 0]).unwrap();
        let expected = simulation.entity_at_address(&[id, 1]).unwrap();
        assert_eq!(
            simulation.world().get::<Target>(source).unwrap().0,
            expected
        );
    }
}

#[test]
fn nested_prefabs_reusing_local_identifiers_do_not_collide() {
    let mut simulation = simulation();
    let follower = format!("{}, {}", tag(100), target(1));
    let assets = texts(&[
        (B, prefab(2, &[(0, &follower), (1, &tag(101))], "")),
        (A, prefab(3, &[(1, &tag(200))], &instance(2, B, "", "", ""))),
    ]);
    let file = scene(1, &[], &instance(0, A, "", "", ""));
    simulation.load_scene_with(&file, &assets).unwrap();
    let source = tagged(&simulation, 100);
    let followed = simulation.world().get::<Target>(source).unwrap().0;
    assert_eq!(tag_of(&simulation, followed), 101);

    let flat = flattened(&simulation, A);
    let addresses: Vec<_> = flat.entities.iter().map(|e| e.address.clone()).collect();
    assert_eq!(addresses, [vec![1], vec![2, 0], vec![2, 1]]);
    let (_, bytes) = flat.entities[1]
        .components
        .iter()
        .find(|(name, _)| name == "prefab::Target")
        .unwrap();
    let value: Target = postcard::from_bytes(bytes).unwrap();
    assert_eq!((value.0.index(), value.0.generation()), (2, 0));
}

#[test]
fn stale_override_warns_and_is_kept_on_save() {
    let mut simulation = simulation();
    let components = format!("{}, \"prefab::Pos\": (x: 1.0, y: 2.0)", tag(1));
    let assets = NamedAssetTexts {
        texts: texts(&[(A, prefab(2, &[(1, &components)], ""))]),
        paths: BTreeMap::from([(AssetId(A), "turret.prefab.ron".to_string())]),
    };
    let overrides = "(entity: [9], component: \"prefab::Tag\", path: [], value: (5)),\
        (entity: [1], component: \"prefab::Shape\", path: [], value: Square(1.0)),\
        (entity: [1], component: \"prefab::Pos\", path: [(type: \"field\", name: \"z\")], value: 1.0),\
        (entity: [1], component: \"prefab::Pos\", path: [(type: \"field\", name: \"x\")], value: 4.0),";
    let removed = "(entity: [1], component: \"prefab::Target\"),";
    let entry = instance(2, A, overrides, "", removed);
    let entry = entry.trim_end_matches(',');
    let file = format!(
        "Scene(\n    version: 2,\n    next_id: 3,\n    resources: {{}},\n    entities: [\n        (id: 0, components: {{{}}}),\n    ],\n    instances: [\n        {entry},\n    ],\n)\n",
        tag(7)
    );
    let load = simulation.load_scene_with(&file, &assets).unwrap();
    assert_eq!(load.warnings.len(), 4, "{:?}", load.warnings);
    for warning in &load.warnings {
        assert!(
            warning.contains("instance 2") && warning.contains("turret.prefab.ron"),
            "{warning}"
        );
    }
    assert!(
        load.warnings[0].contains("prefab::Target"),
        "{:?}",
        load.warnings
    );
    assert!(load.warnings.iter().any(|w| w.contains("entity 9")));
    let entity = tagged(&simulation, 1);
    assert_eq!(
        *simulation.world().get::<Pos>(entity).unwrap(),
        Pos { x: 4.0, y: 2.0 }
    );

    let saved = simulation.save_scene_keeping(&file).unwrap();
    assert!(saved.contains(entry), "{saved}");
    assert_eq!(saved.matches("prefab::Pos").count(), 2, "{saved}");
    let mut reloaded = self::simulation();
    reloaded.load_scene_with(&saved, &assets).unwrap();
    assert_eq!(reloaded.hash(), simulation.hash());
}

#[test]
fn missing_prefab_is_refused_naming_the_instance() {
    let mut simulation = simulation();
    simulation
        .load_scene(&scene(1, &[(0, &tag(1))], ""))
        .unwrap();
    let before = simulation.hash();
    let error = simulation
        .load_scene_with(&scene(5, &[], &instance(4, C, "", "", "")), &NoAssets)
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::MissingAsset { referenced_by, id: AssetId(C) }
            if referenced_by == "instance 4"),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("instance 4") && message.contains(&AssetId(C).to_string()),
        "{message}"
    );
    assert_eq!(simulation.hash(), before);
}

#[test]
fn prefab_identifier_not_below_its_next_id_is_refused() {
    for (own, instances) in [
        (
            vec![(0, tag(1).as_str()), (2, tag(2).as_str())],
            String::new(),
        ),
        (vec![(0, tag(1).as_str())], instance(2, B, "", "", "")),
    ] {
        let mut simulation = simulation();
        let assets = texts(&[(A, prefab(2, &own, &instances)), (B, prefab(0, &[], ""))]);
        let error = simulation
            .load_scene_with(&scene(1, &[], &instance(0, A, "", "", "")), &assets)
            .unwrap_err();
        assert!(
            matches!(&error, SceneError::InvalidAsset { id: AssetId(A), reason, .. }
                if reason.contains("local identifier 2 is not below `next_id` 2")),
            "{error:?}"
        );
    }
}

#[test]
fn prefab_cycle_is_refused_with_paths() {
    let mut simulation = simulation();
    let cyclic = texts(&[
        (A, prefab(1, &[], &instance(0, B, "", "", ""))),
        (B, prefab(1, &[], &instance(0, A, "", "", ""))),
    ]);
    let file = scene(1, &[], &instance(0, A, "", "", ""));
    let named = NamedAssetTexts {
        texts: cyclic.clone(),
        paths: BTreeMap::from([
            (AssetId(A), "a.prefab.ron".to_string()),
            (AssetId(B), "b.prefab.ron".to_string()),
        ]),
    };
    let error = simulation.load_scene_with(&file, &named).unwrap_err();
    assert!(
        matches!(&error, SceneError::PrefabCycle(chain)
            if chain == &["a.prefab.ron", "b.prefab.ron", "a.prefab.ron"]),
        "{error:?}"
    );
    assert!(error.to_string().contains("a.prefab.ron"), "{error}");
    let error = simulation.load_scene_with(&file, &cyclic).unwrap_err();
    let (a, b) = (AssetId(A).to_string(), AssetId(B).to_string());
    assert!(
        matches!(&error, SceneError::PrefabCycle(chain) if *chain == [a.clone(), b, a]),
        "{error:?}"
    );
}

#[test]
fn resolved_scene_text_keeps_addresses() {
    let first = format!("{}, {}", tag(1), target(1));
    let assets = texts(&[(A, prefab(2, &[(0, &first), (1, &tag(2))], ""))]);
    let file = scene(3, &[(0, &tag(0))], &instance(2, A, "", "", ""));
    let resolved = resolve(&file, &assets).unwrap();
    let text = resolved.to_scene_text();
    assert!(text.contains("(address: [2, 1], components:"), "{text}");
    assert!(!text.contains("instances"), "{text}");

    let mut original = simulation();
    original.load_scene_with(&file, &assets).unwrap();
    let mut reloaded = simulation();
    reloaded.load_scene(&text).unwrap();
    assert_eq!(reloaded.load_addresses(), original.load_addresses());
    let world = reloaded.world();
    for entity in world.entities() {
        assert_eq!(
            world.entity_components_ron(entity),
            original.world().entity_components_ron(entity)
        );
    }
}

#[test]
fn prefabs_reached_by_handle_are_flattened_and_loaded() {
    let mut simulation = simulation();
    let first = format!("{}, {}", tag(1), target(1));
    let assets = texts(&[(A, prefab(2, &[(0, &first), (1, &tag(2))], ""))]);
    let spawner = format!("\"prefab::Spawner\": (bullet: \"{}\")", AssetId(A));
    simulation
        .load_scene_with(&scene(1, &[(0, &spawner)], ""), &assets)
        .unwrap();
    assert_eq!(simulation.world().entities().count(), 1);
    let flat = flattened(&simulation, A);
    let addresses: Vec<_> = flat.entities.iter().map(|e| e.address.clone()).collect();
    assert_eq!(addresses, [vec![0], vec![1]]);
    let names: Vec<_> = flat.entities[0]
        .components
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["prefab::Tag", "prefab::Target"]);
    let target: Target = postcard::from_bytes(&flat.entities[0].components[1].1).unwrap();
    assert_eq!((target.0.index(), target.0.generation()), (1, 0));
}

#[test]
fn assets_inside_prefabs_are_reached() {
    let mut simulation = simulation();
    let turret = format!("\"prefab::Turret\": (stats: \"{}\")", AssetId(C));
    let assets = texts(&[
        (A, prefab(1, &[(0, &turret)], "")),
        (
            C,
            "AssetFile(type: \"prefab::Stats\", value: (3))".to_string(),
        ),
    ]);
    let spawner = format!("\"prefab::Spawner\": (bullet: \"{}\")", AssetId(A));
    simulation
        .load_scene_with(&scene(1, &[(0, &spawner)], ""), &assets)
        .unwrap();
    assert_eq!(simulation.world().asset_ids(), [AssetId(A), AssetId(C)]);
}

#[test]
fn resolved_scene_with_an_empty_address_is_refused() {
    let mut simulation = simulation();
    let text = format!(
        "Scene(version: 2, next_id: 1, resources: {{}}, entities: [(address: [], components: {{{}}})])",
        tag(1)
    );
    let error = simulation.load_scene(&text).unwrap_err();
    assert!(matches!(error, SceneError::Parse(_)), "{error:?}");
}

#[test]
fn errors_in_instance_entities_name_the_whole_address() {
    let mut simulation = simulation();
    let assets = texts(&[(A, prefab(4, &[(3, "\"nope::Thing\": ()")], ""))]);
    let file = scene(8, &[], &instance(7, A, "", "", ""));
    let error = simulation.load_scene_with(&file, &assets).unwrap_err();
    assert!(
        matches!(&error, SceneError::UnknownComponent { entity, .. } if *entity == [7, 3]),
        "{error:?}"
    );
    assert!(error.to_string().contains("entity 7/3"), "{error}");
}

#[test]
fn nested_missing_prefab_names_the_nested_instance() {
    let mut simulation = simulation();
    let assets = texts(&[(A, prefab(4, &[], &instance(3, C, "", "", "")))]);
    let file = scene(8, &[], &instance(7, A, "", "", ""));
    let error = simulation.load_scene_with(&file, &assets).unwrap_err();
    assert!(
        matches!(&error, SceneError::MissingAsset { referenced_by, id: AssetId(C) }
            if referenced_by == "instance 7/3"),
        "{error:?}"
    );
}

#[test]
fn self_instancing_prefab_is_a_cycle() {
    let mut simulation = simulation();
    let assets = texts(&[(A, prefab(1, &[], &instance(0, A, "", "", "")))]);
    let file = scene(1, &[], &instance(0, A, "", "", ""));
    let error = simulation.load_scene_with(&file, &assets).unwrap_err();
    let a = AssetId(A).to_string();
    assert!(
        matches!(&error, SceneError::PrefabCycle(chain) if *chain == [a.clone(), a]),
        "{error:?}"
    );
}

#[test]
fn override_entity_values_name_entities_of_the_prefab() {
    let mut simulation = simulation();
    let first = format!("{}, {}", tag(1), target(0));
    let assets = texts(&[(A, prefab(2, &[(0, &first), (1, &tag(2))], ""))]);
    let overrides = "(entity: [0], component: \"prefab::Target\", path: [], value: ((index: 1, generation: 0))),";
    let file = scene(8, &[(1, &tag(9))], &instance(7, A, overrides, "", ""));
    let load = simulation.load_scene_with(&file, &assets).unwrap();
    assert!(load.warnings.is_empty(), "{:?}", load.warnings);
    let source = simulation.entity_at_address(&[7, 0]).unwrap();
    let expected = simulation.entity_at_address(&[7, 1]).unwrap();
    assert_eq!(
        simulation.world().get::<Target>(source).unwrap().0,
        expected
    );
}

#[test]
fn resolved_scene_with_instances_is_refused() {
    let mut simulation = simulation();
    let text = format!(
        "Scene(version: 2, next_id: 1, resources: {{}}, entities: [(address: [0], components: {{{}}})], instances: [{}])",
        tag(1),
        instance(3, A, "", "", "")
    );
    let error = simulation.load_scene(&text).unwrap_err();
    assert!(matches!(error, SceneError::Parse(_)), "{error:?}");
}

#[test]
fn data_asset_behind_a_prefab_handle_is_the_wrong_type() {
    let mut simulation = simulation();
    let assets = texts(&[(
        A,
        "AssetFile(type: \"prefab::Stats\", value: (3))".to_string(),
    )]);
    let spawner = format!("\"prefab::Spawner\": (bullet: \"{}\")", AssetId(A));
    let error = simulation
        .load_scene_with(&scene(1, &[(0, &spawner)], ""), &assets)
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::WrongAssetType { expected, found, .. }
            if expected == "ludenea::Prefab" && found == "prefab::Stats"),
        "{error:?}"
    );
}

#[test]
fn flattened_prefab_in_file_form_reaches_its_assets_and_is_validated() {
    let turret = format!("\"prefab::Turret\": (stats: \"{}\")", AssetId(C));
    let stats = "AssetFile(type: \"prefab::Stats\", value: (3))".to_string();
    let spawner = format!("\"prefab::Spawner\": (bullet: \"{}\")", AssetId(A));
    let file = scene(1, &[(0, &spawner)], "");
    let mut source = simulation();
    source
        .load_scene_with(
            &file,
            &texts(&[(A, prefab(1, &[(0, &turret)], "")), (C, stats.clone())]),
        )
        .unwrap();
    let flat = source.assets_ron()[&AssetId(A)].clone();

    let mut simulation = simulation();
    simulation
        .load_scene_with(&file, &texts(&[(A, flat.clone()), (C, stats)]))
        .unwrap();
    assert_eq!(simulation.world().asset_ids(), [AssetId(A), AssetId(C)]);

    let error = simulation
        .load_scene_with(&file, &texts(&[(A, flat)]))
        .unwrap_err();
    assert!(
        matches!(&error, SceneError::MissingAsset { referenced_by, id: AssetId(C) }
            if referenced_by.ends_with("entity 0, prefab::Turret.stats")),
        "{error:?}"
    );
    let broken = "AssetFile(type: \"ludenea::Prefab\", value: (entities: 3))".to_string();
    let error = simulation
        .load_scene_with(&file, &texts(&[(A, broken)]))
        .unwrap_err();
    assert!(
        matches!(error, SceneError::InvalidAsset { .. }),
        "{error:?}"
    );
}
