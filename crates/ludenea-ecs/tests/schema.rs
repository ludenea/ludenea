use ludenea_ecs::schema::{Schema, TypeSchema};
use ludenea_ecs::{AssetId, Component, Entity, Event, Resource, World};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize)]
#[component(name = "test::Pose")]
struct Pose {
    position: f32,
    rotation: f32,
}

#[derive(Component, Serialize, Deserialize)]
#[component(name = "test::Alpha")]
struct Alpha(u8);

#[derive(Resource, Serialize, Deserialize)]
#[resource(name = "test::Score")]
struct Score(i64);

#[derive(Event, Serialize)]
#[event(name = "test::Hit")]
struct Hit {
    target: Entity,
}

#[derive(Event, Serialize)]
#[event(name = "test::Alert")]
struct Alert(u8);

fn field_names(schema: &TypeSchema) -> Vec<&str> {
    match schema {
        TypeSchema::Struct { fields, .. } => fields.iter().map(|f| f.name.as_str()).collect(),
        other => panic!("expected a struct, got {other:?}"),
    }
}

#[test]
fn component_derive_also_derives_schema() {
    assert_eq!(field_names(&Pose::schema()), ["position", "rotation"]);
}

#[test]
fn world_lists_component_schemas_sorted_by_stable_name() {
    let mut world = World::new();
    world.register_component::<Pose>().unwrap();
    world.register_component::<Alpha>().unwrap();
    world.register_resource(Score(0)).unwrap();

    let names: Vec<_> = world
        .component_schemas()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(names, ["test::Alpha", "test::Pose"]);
    let resources = world.resource_schemas();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].0, "test::Score");
    assert!(matches!(resources[0].1, TypeSchema::Newtype { .. }));
}

#[test]
fn entity_and_asset_id_have_their_own_schemas() {
    assert_eq!(Entity::schema(), TypeSchema::Entity);
    assert_eq!(AssetId::schema(), TypeSchema::AssetId);
}

#[test]
fn asset_id_serializes_as_32_lowercase_hex_digits() {
    let id = AssetId(0xAB);
    let ron_text = ron::to_string(&id).unwrap();
    assert_eq!(ron_text, "\"000000000000000000000000000000ab\"");
    assert_eq!(ron::from_str::<AssetId>(&ron_text).unwrap(), id);
    assert!(ron::from_str::<AssetId>("\"AB\"").is_err());
    assert!(ron::from_str::<AssetId>("\"0000000000000000000000000000000G\"").is_err());
}

#[test]
fn world_lists_event_schemas_by_stable_name() {
    let mut world = World::new();
    world.register_event::<Hit>().unwrap();
    world.register_event::<Alert>().unwrap();

    let schemas = world.event_schemas();
    let names: Vec<_> = schemas.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["test::Alert", "test::Hit"]);
    assert_eq!(field_names(&schemas[1].1), ["target"]);
    assert!(matches!(schemas[0].1, TypeSchema::Newtype { .. }));
}

#[test]
fn events_sharing_a_stable_name_are_refused() {
    #[derive(Event, Serialize)]
    #[event(name = "test::Hit")]
    struct OtherHit;

    let mut world = World::new();
    world.register_event::<Hit>().unwrap();
    world.register_event::<Hit>().unwrap();
    assert!(matches!(
        world.register_event::<OtherHit>(),
        Err(ludenea_ecs::EcsError::DuplicateName {
            name: "test::Hit",
            ..
        })
    ));
}

/// A dialogue line, whose replies lead to further lines.
#[derive(ludenea_ecs::schema::Schema, Serialize, Deserialize, Default)]
struct Line {
    replies: Vec<Reply>,
}

#[derive(ludenea_ecs::schema::Schema, Serialize, Deserialize, Default)]
struct Reply {
    next: Vec<Line>,
}

fn assert_recursive<T>(result: Result<(), ludenea_ecs::EcsError>) {
    let error = result.expect_err("a type holding a recursive type is refused");
    assert!(
        matches!(
            &error,
            ludenea_ecs::EcsError::RecursiveType { name, recursive }
                if *name == std::any::type_name::<T>() && recursive == "schema::Line"
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "`{}` cannot be registered: `schema::Line` contains itself, and recursive types \
             have no schema yet; store children as `Entity` references until recursive \
             schemas are supported",
            std::any::type_name::<T>()
        )
    );
}

#[test]
fn every_registration_refuses_a_type_holding_a_recursive_type() {
    #[derive(Component, Serialize, Deserialize)]
    #[component(name = "test::Speaker")]
    struct Speaker(Line);

    #[derive(Resource, Serialize, Deserialize)]
    #[resource(name = "test::Script")]
    struct Script(Line);

    #[derive(ludenea_ecs::Asset, Serialize, Deserialize, Default)]
    #[asset(name = "test::Dialogue")]
    struct Dialogue(Line);

    #[derive(Event, Serialize)]
    #[event(name = "test::Said")]
    struct Said(Line);

    let mut world = World::new();
    assert_recursive::<Speaker>(world.register_component::<Speaker>());
    assert_recursive::<Script>(world.register_resource(Script(Line::default())));
    assert_recursive::<Dialogue>(world.register_asset::<Dialogue>());
    assert_recursive::<Dialogue>(world.register_asset_with_default::<Dialogue>());
    assert_recursive::<Said>(world.register_event::<Said>());

    assert!(world.component_schemas().is_empty());
    assert!(world.resource_schemas().is_empty());
    assert!(world.asset_schemas().is_empty());
    assert!(world.event_schemas().is_empty());
}
