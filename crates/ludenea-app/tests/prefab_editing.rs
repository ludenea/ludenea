use ludenea_app::path::PathSegment;
use ludenea_app::prefab::{
    add_component_override, apply_to_prefab, check_despawn, create_prefab, place_instance,
    remove_component_override, replace_with_instance, revert_override, save_prefab_checked,
    set_override,
};
use ludenea_app::{App, AssetTexts, Resolved, SceneError, Simulation, resolve};
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
    save_prefab_checked(&text, &AssetTexts::new()).unwrap();
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
        save_prefab_checked(&a, &assets),
        Err(SceneError::PrefabCycle(chain)) if chain.len() == 3
    ));

    let foreign = "Prefab(version: 1, next_id: 2, entities: [\
        (id: 0, components: {\"prefab_editing::Link\": (to: (index: 1, generation: 0))}),\
        ])";
    assert!(matches!(
        save_prefab_checked(foreign, &AssetTexts::new()),
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
