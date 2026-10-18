use ludenea_app::{App, SceneError, Simulation};
use ludenea_ecs::schema::Schema;
use ludenea_ecs::{Component, Resource};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Bulk {
    big: u64,
    negative_zero: f32,
    subnormal: f32,
    max: f32,
    noise: Vec<f32>,
}

#[derive(Serialize, Deserialize, Schema, Clone, Debug, PartialEq)]
enum Mood {
    Calm,
    Angry(u8, f32),
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Feeling(Mood);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Tag(u32);

#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Score(i64);

fn simulation() -> Simulation {
    let mut app = App::new();
    app.component::<Bulk>()
        .component::<Feeling>()
        .component::<Tag>()
        .resource(Score(0));
    app.build().unwrap()
}

fn bytes(simulation: &Simulation) -> Vec<u8> {
    simulation.snapshot().encode()
}

fn noise() -> Vec<f32> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut values = Vec::new();
    while values.len() < 10_000 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let value = f32::from_bits((state >> 32) as u32);
        if !value.is_nan() {
            values.push(value);
        }
    }
    values
}

const SCENE: &str = r#"Scene(
    version: 1,
    resources: { "ludenea::Rng": (state: 1, increment: 3), "scene::Score": (-5) },
    entities: [
        { "scene::Tag": (7) },
        { "scene::Tag": (8), "scene::Feeling": (Angry(2, 0.5)) },
    ],
)"#;

fn loaded() -> Simulation {
    let mut simulation = simulation();
    simulation.load_scene(SCENE).unwrap();
    simulation
}

#[test]
fn scene_round_trip_save_then_load_gives_same_hash() {
    let first = loaded();
    let mut second = simulation();
    second.load_scene(&first.save_scene()).unwrap();
    assert_eq!(first.hash(), second.hash());
}

#[test]
fn scene_round_trip_is_bit_exact() {
    let mut first = simulation();
    let world = first.world_mut();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Bulk {
                big: u64::MAX,
                negative_zero: -0.0,
                subnormal: f32::from_bits(1),
                max: f32::MAX,
                noise: noise(),
            },
        )
        .unwrap();

    let mut second = simulation();
    second.load_scene(&first.save_scene()).unwrap();
    assert_eq!(bytes(&first), bytes(&second));
}

#[test]
fn scene_with_enum_fields_loads() {
    let loaded = loaded();
    let world = loaded.world();
    let second = world.entities().nth(1).unwrap();
    assert_eq!(world.get::<Feeling>(second).unwrap().0, Mood::Angry(2, 0.5));
    assert_eq!(world.resource::<Score>().unwrap().0, -5);
    let calm = r#"Scene(version: 1, resources: {}, entities: [{"scene::Feeling": (Calm)}])"#;
    let mut other = simulation();
    other.load_scene(calm).unwrap();
    let entity = other.world().entities().next().unwrap();
    assert_eq!(other.world().get::<Feeling>(entity).unwrap().0, Mood::Calm);
}

#[test]
fn scene_unknown_component_names_entity_and_component_and_leaves_state() {
    let mut simulation = loaded();
    let before = simulation.hash();
    let scene = r#"Scene(version: 1, resources: {}, entities: [
        { "scene::Tag": (1) },
        { "nope::Thing": (1) },
    ])"#;
    let error = simulation.load_scene(scene).unwrap_err();
    assert!(
        matches!(&error, SceneError::UnknownComponent { entity, name } if *entity == [1] && name == "nope::Thing"),
        "{error:?}"
    );
    assert_eq!(simulation.hash(), before);
}

#[test]
fn scene_unknown_resource_is_named() {
    let mut simulation = simulation();
    let error = simulation
        .load_scene(r#"Scene(version: 1, resources: {"nope::Res": (1)}, entities: [])"#)
        .unwrap_err();
    assert!(matches!(&error, SceneError::UnknownResource(name) if name == "nope::Res"));
}

#[test]
fn scene_with_another_version_is_rejected() {
    let error = simulation()
        .load_scene("Scene(version: 3, resources: {}, entities: [])")
        .unwrap_err();
    assert!(matches!(error, SceneError::Version(3)));
    assert!(error.to_string().contains("expected 1 or 2"), "{error}");

    let error = simulation()
        .load_scene("Scene(version: 3, objects: {}, next_id: \"a\")")
        .unwrap_err();
    assert!(matches!(error, SceneError::Version(3)), "{error}");
}

#[test]
fn scene_malformed_value_is_invalid_value() {
    let mut simulation = loaded();
    let before = simulation.hash();
    let scene = r#"Scene(version: 1, resources: {}, entities: [{ "scene::Tag": ("text") }])"#;
    let error = simulation.load_scene(scene).unwrap_err();
    assert!(
        matches!(&error, SceneError::InvalidValue { entity: Some(entity), name, .. } if *entity == [0] && name == "scene::Tag"),
        "{error:?}"
    );
    assert_eq!(simulation.hash(), before);
}

#[test]
fn scene_duplicate_component_key_is_invalid_value() {
    let scene =
        r#"Scene(version: 1, resources: {}, entities: [{ "scene::Tag": (1), "scene::Tag": (2) }])"#;
    let error = simulation().load_scene(scene).unwrap_err();
    assert!(
        matches!(&error, SceneError::InvalidValue { entity: Some(entity), name, .. } if *entity == [0] && name == "scene::Tag"),
        "{error:?}"
    );
}

#[test]
fn scene_that_is_not_ron_is_a_parse_error() {
    let error = simulation().load_scene("not a scene").unwrap_err();
    assert!(matches!(error, SceneError::Parse(_)));
}
