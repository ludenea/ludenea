use ludenea_ecs::schema::{Schema, TypeSchema};
use ludenea_ecs::{AssetId, Component, Entity, Resource, World};
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
    let json = ron::to_string(&id).unwrap();
    assert_eq!(json, "\"000000000000000000000000000000ab\"");
    assert_eq!(ron::from_str::<AssetId>(&json).unwrap(), id);
    assert!(ron::from_str::<AssetId>("\"AB\"").is_err());
    assert!(ron::from_str::<AssetId>("\"0000000000000000000000000000000G\"").is_err());
}
