use ludenea_app::path::PathSegment;
use ludenea_app::prefab::{
    add_component_override, apply_to_prefab, check_despawn, create_prefab, place_instance,
    remove_component_override, replace_with_instance, revert_override, save_prefab_checked,
    set_override,
};
use ludenea_app::{App, AssetTexts, NamedAssetTexts, Resolved, SceneError, Simulation, resolve};
use ludenea_ecs::{AssetId, Component, Entity};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Tag(u32);

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Link {
    to: Entity,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq)]
struct Pos {
    x: f32,
    y: f32,
}

const A: u128 = 0xa;
const B: u128 = 0xb;

fn simulation() -> Simulation {
    let mut app = App::new();
    app.component::<Tag>()
        .component::<Link>()
        .component::<Pos>();
    app.build().unwrap()
}

fn field(name: &str) -> PathSegment {
    PathSegment::Field {
        name: name.to_string(),
    }
}

fn texts(assets: &[(u128, &str)]) -> AssetTexts {
    assets
        .iter()
        .map(|(id, text)| (AssetId(*id), text.to_string()))
        .collect()
}

/// A prefab of two entities: `Pos` and `Tag(1)`, then `Tag(2)`.
const PREFAB: &str = "Prefab(version: 1, next_id: 2, entities: [\
    (id: 0, components: {\"prefab_editing::Pos\": (x: 1.0, y: 2.0), \"prefab_editing::Tag\": (1)}),\
    (id: 1, components: {\"prefab_editing::Tag\": (2)}),\
    ])";

fn scene_with_instance() -> String {
    format!(
        "Scene(\n    version: 2,\n    next_id: 4,\n    resources: {{}},\n    entities: [\n        (id: 1, components: {{\"prefab_editing::Tag\": (7)}}),\n    ],\n    instances: [\n        (id: 3, prefab: \"{}\", overrides: [], added: [], removed: []),\n    ],\n)\n",
        AssetId(A)
    )
}

fn resolved(scene: &str) -> Resolved {
    resolve(scene, &texts(&[(A, PREFAB)])).unwrap()
}

fn component(resolved: &Resolved, address: &[u32], name: &str) -> Option<String> {
    let entity = resolved.entities.iter().find(|e| e.address == address)?;
    let name = format!("prefab_editing::{name}");
    entity
        .components
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, value)| value.clone())
}

#[test]
fn create_prefab_numbers_entities_from_zero() {
    let mut simulation = simulation();
    simulation
        .load_scene(
            "Scene(version: 2, next_id: 10, resources: {}, entities: [\
            (id: 5, components: {\"prefab_editing::Tag\": (1)}),\
            (id: 7, components: {\"prefab_editing::Tag\": (2)}),\
            (id: 9, components: {\"prefab_editing::Link\": (to: (index: 7, generation: 0))}),\
            ])",
        )
        .unwrap();
    let at = |local| simulation.entity_at_address(&[local]).unwrap();
    let text = create_prefab(&simulation, &[at(9), at(7)]).unwrap();
    assert_eq!(
        text,
        "Prefab(\n    version: 1,\n    next_id: 2,\n    entities: [\n        (id: 0, components: {\"prefab_editing::Link\": (to:(index:1,generation:0))}),\n        (id: 1, components: {\"prefab_editing::Tag\": (2)}),\n    ],\n)\n"
    );
    save_prefab_checked(AssetId(A), &text, &AssetTexts::new()).unwrap();
}

#[test]
fn foreign_reference_is_refused_on_create() {
    let mut simulation = simulation();
    simulation
        .load_scene(
            "Scene(version: 2, next_id: 2, resources: {}, entities: [\
            (id: 0, components: {\"prefab_editing::Tag\": (1)}),\
            (id: 1, components: {\"prefab_editing::Link\": (to: (index: 0, generation: 0))}),\
            ])",
        )
        .unwrap();
    let linked = simulation.entity_at_address(&[1]).unwrap();
    let error = create_prefab(&simulation, &[linked]).unwrap_err();
    assert!(
        matches!(
            &error,
            SceneError::ForeignReference { entity, component, path }
                if *entity == linked.index()
                    && component == "prefab_editing::Link"
                    && path == "to"
        ),
        "{error}"
    );
}

#[test]
fn place_instance_uses_next_id() {
    let scene = "Scene(version: 2, next_id: 4, resources: {}, entities: [(id: 1, components: {})])";
    let (text, id) = place_instance(scene, AssetId(A)).unwrap();
    assert_eq!(id, 4);
    let (text, again) = place_instance(&text, AssetId(A)).unwrap();
    assert_eq!(again, 5);
    let resolved = resolved(&text);
    let addresses: Vec<_> = resolved
        .entities
        .iter()
        .map(|e| e.address.clone())
        .collect();
    assert_eq!(
        addresses,
        [vec![1], vec![4, 0], vec![4, 1], vec![5, 0], vec![5, 1]]
    );
    assert!(text.contains("next_id: 6"));
}

#[test]
fn replace_with_instance_removes_entities() {
    let scene = "Scene(\n    version: 2,\n    next_id: 3,\n    resources: {},\n    entities: [\n        (id: 0, components: {\"prefab_editing::Tag\": (10)}),\n        (id: 1, components: {\"prefab_editing::Tag\": (11)}),\n        (id: 2, components: {\"prefab_editing::Tag\": (12)}),\n    ],\n)\n";
    let text = replace_with_instance(scene, &[0, 2], AssetId(A)).unwrap();
    let resolved = resolved(&text);
    let addresses: Vec<_> = resolved
        .entities
        .iter()
        .map(|e| e.address.clone())
        .collect();
    assert_eq!(addresses, [vec![1], vec![3, 0], vec![3, 1]]);
    assert!(matches!(
        replace_with_instance(&text, &[3], AssetId(A)),
        Err(SceneError::InstanceEntity(address)) if address == [3]
    ));
    assert!(replace_with_instance(&text, &[8], AssetId(A)).is_err());
}

#[test]
fn set_then_revert_override_restores_text() {
    let scene = scene_with_instance();
    let y = [field("y")];
    let text = set_override(&scene, &[3], &[0], "prefab_editing::Pos", &y, "7.5").unwrap();
    let text = set_override(&text, &[3], &[0], "prefab_editing::Pos", &y, "8.5").unwrap();
    assert_eq!(text.matches("value:").count(), 1);
    assert_eq!(
        component(&resolved(&text), &[3, 0], "Pos").unwrap(),
        "(x: 1.0, y: 8.5)"
    );
    let reverted = revert_override(&text, &[3], &[0], "prefab_editing::Pos", &y).unwrap();
    assert_eq!(reverted, scene);
    assert!(set_override(&scene, &[9], &[0], "prefab_editing::Pos", &y, "1.0").is_err());
}

#[test]
fn revert_component_reverts_every_field() {
    let scene = scene_with_instance();
    let pos = "prefab_editing::Pos";
    let text = set_override(&scene, &[3], &[0], pos, &[field("x")], "5.0").unwrap();
    let text = set_override(&text, &[3], &[0], pos, &[field("y")], "6.0").unwrap();
    let text = set_override(&text, &[3], &[0], "prefab_editing::Tag", &[], "(9)").unwrap();
    let reverted = revert_override(&text, &[3], &[0], pos, &[]).unwrap();
    let resolved = resolved(&reverted);
    assert_eq!(
        component(&resolved, &[3, 0], "Pos").unwrap(),
        "(x: 1.0, y: 2.0)"
    );
    assert_eq!(component(&resolved, &[3, 0], "Tag").unwrap(), "(9)");
    let all = revert_override(&reverted, &[3], &[0], "prefab_editing::Tag", &[]).unwrap();
    assert_eq!(all, scene);
}

#[test]
fn added_and_removed_overrides_round_trip() {
    let scene = scene_with_instance();
    let link = "prefab_editing::Link";
    let tag = "prefab_editing::Tag";
    let value = "(to: (index: 0, generation: 0))";
    let text = add_component_override(&scene, &[3], &[1], link, value).unwrap();
    let text = remove_component_override(&text, &[3], &[1], tag).unwrap();
    let edited = resolved(&text);
    assert_eq!(component(&edited, &[3, 1], "Link").unwrap(), value);
    assert_eq!(component(&edited, &[3, 1], "Tag"), None);
    assert!(edited.warnings.is_empty());

    let text = add_component_override(&text, &[3], &[1], tag, "(4)").unwrap();
    let text = remove_component_override(&text, &[3], &[1], link).unwrap();
    let edited = resolved(&text);
    assert_eq!(component(&edited, &[3, 1], "Tag").unwrap(), "(4)");
    assert_eq!(component(&edited, &[3, 1], "Link"), None);
    assert!(!text.contains("removed: [("));
}

#[test]
fn removing_a_readded_prefab_component_removes_it_again() {
    let scene = scene_with_instance();
    let tag = "prefab_editing::Tag";
    let text = remove_component_override(&scene, &[3], &[1], tag).unwrap();
    let text = add_component_override(&text, &[3], &[1], tag, "(4)").unwrap();
    assert!(!text.contains("added: [("));
    assert_eq!(component(&resolved(&text), &[3, 1], "Tag").unwrap(), "(4)");
    let text = remove_component_override(&text, &[3], &[1], tag).unwrap();
    let edited = resolved(&text);
    assert_eq!(component(&edited, &[3, 1], "Tag"), None);
    assert!(edited.warnings.is_empty());
}

#[test]
fn adding_then_removing_a_missing_component_restores_text() {
    let scene = scene_with_instance();
    let link = "prefab_editing::Link";
    let text = add_component_override(&scene, &[3], &[1], link, "(to: (index: 0, generation: 0))")
        .unwrap();
    assert!(text.contains("added: [\n"));
    assert_eq!(
        remove_component_override(&text, &[3], &[1], link).unwrap(),
        scene
    );
}

#[test]
fn field_overrides_win_over_an_earlier_whole_override() {
    let scene = scene_with_instance();
    let pos = "prefab_editing::Pos";
    let text = set_override(&scene, &[3], &[0], pos, &[field("y")], "5.0").unwrap();
    let text = set_override(&text, &[3], &[0], pos, &[], "(x: 3.0, y: 4.0)").unwrap();
    assert!(!text.contains("5.0"));
    let text = set_override(&text, &[3], &[0], pos, &[field("x")], "9.0").unwrap();
    assert_eq!(
        component(&resolved(&text), &[3, 0], "Pos").unwrap(),
        "(x: 9.0, y: 4.0)"
    );
}

#[test]
fn apply_to_prefab_writes_nested_override() {
    let outer = format!(
        "Prefab(version: 1, next_id: 4, entities: [\
        (id: 0, components: {{\"prefab_editing::Pos\": (x: 0.0, y: 0.0)}}),\
        ], instances: [(id: 3, prefab: \"{}\")])",
        AssetId(A)
    );
    let outer = apply_to_prefab(&outer, &[0], "prefab_editing::Pos", &[field("y")], "2.0").unwrap();
    let outer = apply_to_prefab(&outer, &[3, 1], "prefab_editing::Tag", &[], "(5)").unwrap();
    let scene = format!(
        "Scene(version: 2, next_id: 1, resources: {{}}, entities: [], instances: [(id: 0, prefab: \"{}\")])",
        AssetId(B)
    );
    let resolved = resolve(&scene, &texts(&[(A, PREFAB), (B, &outer)])).unwrap();
    assert!(resolved.warnings.is_empty());
    assert_eq!(
        component(&resolved, &[0, 0], "Pos").unwrap(),
        "(x: 0.0, y: 2.0)"
    );
    assert_eq!(component(&resolved, &[0, 3, 1], "Tag").unwrap(), "(5)");
    assert!(apply_to_prefab(&outer, &[0], "prefab_editing::Tag", &[], "(1)").is_err());
}

#[test]
fn prefab_cycle_is_refused_on_save() {
    let instance_of = |id| format!("(id: 5, prefab: \"{}\")", AssetId(id));
    let a = format!(
        "Prefab(version: 1, next_id: 6, entities: [], instances: [{}])",
        instance_of(B)
    );
    let b = format!(
        "Prefab(version: 1, next_id: 6, entities: [], instances: [{}])",
        instance_of(A)
    );
    let assets = texts(&[(A, &a), (B, &b)]);
    assert!(matches!(
        save_prefab_checked(AssetId(A), &a, &assets),
        Err(SceneError::PrefabCycle(chain))
            if chain == [AssetId(A), AssetId(B), AssetId(A)].map(|id| id.to_string())
    ));

    // `assets` holds the old A, without the instance of B, or no A at all (a new prefab).
    let old_a = "Prefab(version: 1, next_id: 0, entities: [])".to_string();
    let named = |a: Option<&String>| NamedAssetTexts {
        texts: a
            .map(|a| (AssetId(A), a.clone()))
            .into_iter()
            .chain([(AssetId(B), b.clone())])
            .collect(),
        paths: [(AssetId(A), "a.prefab.ron"), (AssetId(B), "b.prefab.ron")]
            .map(|(id, path)| (id, path.to_string()))
            .into(),
    };
    for stale in [named(Some(&old_a)), named(None)] {
        assert!(matches!(
            save_prefab_checked(AssetId(A), &a, &stale),
            Err(SceneError::PrefabCycle(chain))
                if chain == ["a.prefab.ron", "b.prefab.ron", "a.prefab.ron"]
        ));
    }

    let foreign = "Prefab(version: 1, next_id: 2, entities: [\
        (id: 0, components: {\"prefab_editing::Link\": (to: (index: 1, generation: 0))}),\
        ])";
    assert!(matches!(
        save_prefab_checked(AssetId(A), foreign, &AssetTexts::new()),
        Err(SceneError::ForeignReference { entity: 0, component, path })
            if component == "prefab_editing::Link" && path == "to"
    ));
}

#[test]
fn despawning_instance_entity_is_refused() {
    let error = check_despawn(&[3, 1]).unwrap_err();
    assert!(matches!(&error, SceneError::InstanceEntity(address) if *address == [3, 1]));
    assert!(
        error
            .to_string()
            .contains("remove the instance or override its components")
    );
    check_despawn(&[1]).unwrap();
}

#[test]
fn values_that_are_not_one_ron_value_are_refused() {
    let scene = scene_with_instance();
    let tag = "prefab_editing::Tag";
    for value in ["(9)), (entity: [1]", "", "(9) (8)"] {
        assert!(matches!(
            set_override(&scene, &[3], &[0], tag, &[], value),
            Err(SceneError::Parse(_))
        ));
        assert!(matches!(
            add_component_override(&scene, &[3], &[0], tag, value),
            Err(SceneError::Parse(_))
        ));
        assert!(matches!(
            apply_to_prefab(PREFAB, &[0], tag, &[], value),
            Err(SceneError::Parse(_))
        ));
    }
    let shaped = "Circle(radius: Some([1.0, 2.0]))";
    assert!(set_override(&scene, &[3], &[0], tag, &[], shaped).is_ok());
}

#[test]
fn create_prefab_refuses_dead_entities_and_writes_duplicates_once() {
    let mut simulation = simulation();
    simulation
        .load_scene(
            "Scene(version: 2, next_id: 2, resources: {}, entities: [\
            (id: 0, components: {\"prefab_editing::Tag\": (1)}),\
            (id: 1, components: {\"prefab_editing::Tag\": (2)}),\
            ])",
        )
        .unwrap();
    let first = simulation.entity_at_address(&[0]).unwrap();
    let second = simulation.entity_at_address(&[1]).unwrap();
    let text = create_prefab(&simulation, &[first, first]).unwrap();
    assert!(text.contains("next_id: 1"), "{text}");
    simulation.world_mut().despawn(second).unwrap();
    assert!(matches!(
        create_prefab(&simulation, &[first, second]),
        Err(SceneError::MissingTarget(_))
    ));
}

#[test]
fn stale_entries_are_kept_byte_for_byte_while_a_neighbour_is_edited() {
    let stale = "(entity: [9], component: \"prefab_editing::Tag\", path: [], value: (5))";
    let scene = scene_with_instance().replace("overrides: []", &format!("overrides: [{stale}]"));
    let text = set_override(
        &scene,
        &[3],
        &[0],
        "prefab_editing::Pos",
        &[field("y")],
        "4.0",
    )
    .unwrap();
    assert!(text.contains(stale), "{text}");
    let resolved = resolve(&text, &texts(&[(A, PREFAB)])).unwrap();
    assert_eq!(resolved.warnings.len(), 1, "{:?}", resolved.warnings);
    assert_eq!(
        component(&resolved, &[3, 0], "Pos").unwrap(),
        "(x: 1.0, y: 4.0)"
    );
}

#[test]
fn overrides_append_to_inline_and_multiline_lists() {
    let first = "(entity: [1], component: \"prefab_editing::Tag\", path: [], value: (5))";
    let inline = scene_with_instance().replace("overrides: []", &format!("overrides: [{first}]"));
    let multiline = scene_with_instance().replace(
        "overrides: []",
        &format!("overrides: [\n            {first},\n        ]"),
    );
    for scene in [inline, multiline] {
        let text = set_override(&scene, &[3], &[0], "prefab_editing::Tag", &[], "(6)").unwrap();
        let resolved = resolved(&text);
        assert_eq!(
            component(&resolved, &[3, 1], "Tag").unwrap(),
            "(5)",
            "{text}"
        );
        assert_eq!(
            component(&resolved, &[3, 0], "Tag").unwrap(),
            "(6)",
            "{text}"
        );
    }
}

#[test]
fn save_prefab_checked_refuses_nested_foreign_references_and_missing_prefabs() {
    let nested = "Prefab(version: 1, next_id: 1, entities: [\
        (id: 0, components: {\"x::Many\": (all: [Some((index: 0, generation: 0)), Some((index: 4, generation: 0))])}),\
        ])";
    assert!(matches!(
        save_prefab_checked(AssetId(A), nested, &AssetTexts::new()),
        Err(SceneError::ForeignReference { entity: 0, path, .. }) if path == "all[1]"
    ));
    let missing = format!(
        "Prefab(version: 1, next_id: 1, entities: [], instances: [(id: 0, prefab: \"{}\")])",
        AssetId(B)
    );
    assert!(matches!(
        save_prefab_checked(AssetId(A), &missing, &AssetTexts::new()),
        Err(SceneError::MissingAsset { id, .. }) if id == AssetId(B)
    ));
}
