use ludenea_ecs::hash::ASSETS_LEAF;
use ludenea_ecs::schema::{Schema, TypeSchema};
use ludenea_ecs::{
    Asset, AssetId, Assets, Component, EcsError, Handle, ResMut, Resource, ScheduleBuilder,
    Snapshot, SnapshotError, World, hash_value,
};
use serde::{Deserialize, Serialize};

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq, Default)]
#[asset(name = "test::Stats")]
struct Stats {
    damage: u32,
    cooldown: u32,
}

#[derive(Asset, Serialize, Deserialize, Debug, PartialEq)]
struct NoDefault(u8);

#[derive(Component, Serialize, Deserialize)]
struct Weapon(u32);

#[derive(Resource, Serialize, Deserialize)]
struct Read(Option<u32>);

const ID: AssetId = AssetId(7);

fn world() -> World {
    let mut world = World::new();
    world.register_component::<Weapon>().unwrap();
    world.register_resource(Read(None)).unwrap();
    let entity = world.spawn();
    world.insert(entity, Weapon(3)).unwrap();
    world
}

fn with_assets() -> World {
    let mut world = world();
    world.register_asset_with_default::<Stats>().unwrap();
    world.register_asset::<NoDefault>().unwrap();
    world
}

fn run(world: &mut World, system: fn(Assets<Stats>, ResMut<Read>)) {
    let mut builder = ScheduleBuilder::default();
    builder.add(system);
    let Ok(mut schedule) = builder.build(world) else {
        panic!("schedule should build");
    };
    schedule.run(world);
}

#[test]
fn handle_encodes_and_hashes_like_its_asset_id() {
    let handle = Handle::<Stats>::new(ID);
    assert_eq!(handle.id, ID);
    assert_eq!(
        postcard::to_allocvec(&handle).unwrap(),
        postcard::to_allocvec(&ID).unwrap()
    );
    assert_eq!(hash_value(&handle), hash_value(&ID));
    assert_eq!(
        ron::to_string(&handle).unwrap(),
        ron::to_string(&ID).unwrap()
    );
    let back: Handle<Stats> = ron::from_str(&ron::to_string(&ID).unwrap()).unwrap();
    assert_eq!(back, handle);
}

#[test]
fn asset_id_serializes_as_its_display_text() {
    for id in [0, 7, 0xfedc_ba98_7654_3210, u128::MAX, 1 << 127] {
        let id = AssetId(id);
        let text = format!("{id}");
        assert_eq!(text.len(), 32);
        assert_eq!(ron::to_string(&id).unwrap(), format!("\"{text}\""));
        assert_eq!(hash_value(&id), hash_value(&text));
        assert_eq!(
            postcard::to_allocvec(&id).unwrap(),
            postcard::to_allocvec(&text).unwrap()
        );
    }
}

#[test]
fn world_hash_is_the_root_of_its_tree() {
    let mut world = with_assets();
    let other = world.spawn();
    world.insert(other, Weapon(5)).unwrap();
    let gone = world.spawn();
    world.despawn(gone).unwrap();
    world.spawn();
    assert_eq!(world.hash(), world.hash_tree().root);
    world
        .insert_asset_ron(ID, "test::Stats", "(damage: 4, cooldown: 2)")
        .unwrap();
    assert_eq!(world.hash(), world.hash_tree().root);
}

#[test]
fn handle_schema_names_its_asset_type() {
    assert_eq!(
        Handle::<Stats>::schema(),
        TypeSchema::Handle {
            asset: "test::Stats".to_string()
        }
    );
    assert!(NoDefault::NAME.ends_with("assets::NoDefault"));
}

#[test]
fn world_without_assets_hashes_as_before() {
    let plain = world();
    let registered = with_assets();
    assert_eq!(registered.hash_tree(), plain.hash_tree());
    assert!(
        registered
            .hash_tree()
            .resources
            .iter()
            .all(|leaf| leaf.name != ASSETS_LEAF)
    );
}

#[test]
fn loaded_asset_adds_the_assets_leaf_after_entities() {
    let mut world = with_assets();
    let before = world.hash();
    world
        .insert_asset_ron(ID, "test::Stats", "(damage: 4, cooldown: 2)")
        .unwrap();
    let tree = world.hash_tree();
    let names: Vec<_> = tree
        .resources
        .iter()
        .map(|leaf| leaf.name.as_str())
        .collect();
    assert_eq!(names[names.len() - 2..], ["ludenea::Entities", ASSETS_LEAF]);
    assert_ne!(tree.root, before);

    let (type_name, bytes) = world.asset_bytes(ID).unwrap();
    assert_eq!(type_name, "test::Stats");
    let mut expected = xxhash_rust::xxh3::Xxh3::new();
    expected.update(&ID.0.to_le_bytes());
    expected.update(&hash_value(&bytes[..]).to_le_bytes());
    assert_eq!(tree.resources.last().unwrap().hash, expected.digest());

    world.clear_assets();
    assert_eq!(world.hash(), before);
}

#[test]
fn asset_hash_ignores_ron_formatting() {
    let mut compact = with_assets();
    compact
        .insert_asset_ron(ID, "test::Stats", "(damage:4,cooldown:2)")
        .unwrap();
    let mut spaced = with_assets();
    spaced
        .insert_asset_ron(
            ID,
            "test::Stats",
            "// tuned\n(\n    damage: 4,\n    cooldown: 2,\n)\n",
        )
        .unwrap();
    assert_eq!(compact.hash_tree(), spaced.hash_tree());
    assert_eq!(compact.asset_ron(ID), spaced.asset_ron(ID));
    assert_eq!(compact.asset_ids(), vec![ID]);
}

fn read_damage(assets: Assets<Stats>, mut read: ResMut<Read>) {
    let handle = Handle::new(ID);
    read.0 = Some(assets.get(handle).damage);
    assert!(assets.try_get(Handle::new(AssetId(8))).is_none());
}

#[test]
fn assets_param_reads_loaded_asset() {
    let mut world = with_assets();
    let bytes = postcard::to_allocvec(&Stats {
        damage: 9,
        cooldown: 1,
    })
    .unwrap();
    world.insert_asset(ID, "test::Stats", &bytes).unwrap();
    run(&mut world, read_damage);
    assert_eq!(world.resource::<Read>().unwrap().0, Some(9));
}

#[test]
#[should_panic(
    expected = "asset 00000000000000000000000000000007 of type test::Stats is not loaded"
)]
fn assets_param_panics_naming_missing_asset() {
    let mut world = with_assets();
    run(&mut world, read_damage);
}

#[test]
fn snapshot_restores_assets() {
    let mut source = with_assets();
    source
        .insert_asset_ron(ID, "test::Stats", "(damage: 4, cooldown: 2)")
        .unwrap();
    let snapshot = source.snapshot();
    assert_eq!(snapshot.fingerprint, 0);
    assert_eq!(snapshot.assets.len(), 1);
    let decoded = Snapshot::decode(&snapshot.encode()).unwrap();
    assert_eq!(decoded, snapshot);

    let mut target = with_assets();
    target
        .insert_asset_ron(AssetId(99), "test::Stats", "(damage: 1, cooldown: 1)")
        .unwrap();
    target.restore(&decoded).unwrap();
    assert_eq!(target.hash_tree(), source.hash_tree());
    assert_eq!(target.asset_ids(), vec![ID]);
}

#[test]
fn snapshot_version_1_is_refused_as_version() {
    let mut bytes = postcard::to_allocvec(&1u32).unwrap();
    // A version 1 snapshot continues with fields this build does not read.
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    let error = Snapshot::decode(&bytes).unwrap_err();
    assert!(
        matches!(
            error,
            SnapshotError::Version {
                found: 1,
                expected: 3
            }
        ),
        "{error}"
    );
}

#[test]
fn invalid_asset_ron_reports_line_and_column() {
    let mut world = with_assets();
    let error = world
        .insert_asset_ron(ID, "test::Stats", "(\n  damage: oops,\n)")
        .unwrap_err();
    let EcsError::InvalidAsset {
        id,
        type_name,
        reason,
    } = error
    else {
        panic!("expected InvalidAsset, got {error}");
    };
    assert_eq!(id, ID);
    assert_eq!(type_name, "test::Stats");
    assert!(reason.contains("2:"), "{reason}");
    assert!(world.asset_ids().is_empty());
}

#[test]
fn default_asset_ron_requires_a_registered_default() {
    let world = with_assets();
    let ron = world.default_asset_ron("test::Stats").unwrap();
    assert_eq!(ron::from_str::<Stats>(&ron).unwrap(), Stats::default());
    assert!(matches!(
        world.default_asset_ron(NoDefault::NAME),
        Err(EcsError::NoDefault(name)) if name == NoDefault::NAME
    ));
    assert!(matches!(
        world.default_asset_ron("missing"),
        Err(EcsError::UnknownAsset(name)) if name == "missing"
    ));
    let names: Vec<_> = world
        .asset_schemas()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(names.len(), 2);
}

#[test]
fn insert_asset_of_unknown_type_is_refused() {
    let mut world = with_assets();
    assert!(matches!(
        world.insert_asset(ID, "missing", &[]),
        Err(EcsError::UnknownAsset(name)) if name == "missing"
    ));
    assert!(world.asset_ids().is_empty());
}

#[test]
fn insert_asset_replaces_the_same_identifier() {
    let mut world = with_assets();
    world
        .insert_asset_ron(ID, "test::Stats", "(damage: 1, cooldown: 1)")
        .unwrap();
    world
        .insert_asset_ron(ID, "test::Stats", "(damage: 5, cooldown: 1)")
        .unwrap();
    assert_eq!(world.asset_ids(), vec![ID]);
    assert_eq!(
        world.asset_ron(ID).unwrap(),
        ron::to_string(&Stats {
            damage: 5,
            cooldown: 1
        })
        .unwrap()
    );
}

fn stats_snapshot() -> Snapshot {
    let mut source = with_assets();
    source
        .insert_asset_ron(ID, "test::Stats", "(damage: 4, cooldown: 2)")
        .unwrap();
    source.snapshot()
}

#[test]
fn restore_refuses_an_unregistered_asset_type_and_keeps_the_world() {
    let mut target = world();
    let before = target.hash_tree();
    let error = target.restore(&stats_snapshot()).unwrap_err();
    assert!(
        matches!(&error, SnapshotError::Decode { name, reason }
            if name == &format!("asset {ID}") && reason.contains("test::Stats")),
        "{error}"
    );
    assert_eq!(target.hash_tree(), before);
}

#[test]
fn restore_refuses_unsorted_or_duplicate_asset_identifiers() {
    let sorted = stats_snapshot();
    let (_, type_name, bytes) = sorted.assets[0].clone();
    for ids in [[ID, ID], [AssetId(8), ID]] {
        let mut snapshot = sorted.clone();
        snapshot.assets = ids
            .iter()
            .map(|&id| (id, type_name.clone(), bytes.clone()))
            .collect();
        let mut target = with_assets();
        let error = target.restore(&snapshot).unwrap_err();
        assert!(
            matches!(&error, SnapshotError::Decode { reason, .. } if reason.contains("ascending")),
            "{error}"
        );
        assert!(target.asset_ids().is_empty());
    }
}

#[test]
fn handle_field_appears_in_the_component_schema() {
    #[derive(Component, Serialize, Deserialize)]
    #[component(name = "test::Armed")]
    struct Armed {
        stats: Handle<Stats>,
    }

    let mut world = World::new();
    world.register_component::<Armed>().unwrap();
    let (_, schema) = world.component_schemas().remove(0);
    let TypeSchema::Struct { fields, .. } = schema else {
        panic!("expected a struct, got {schema:?}");
    };
    assert_eq!(
        fields[0].schema,
        TypeSchema::Handle {
            asset: "test::Stats".to_string()
        }
    );
}

#[test]
fn diff_of_an_asset_whose_type_changed_names_both_types() {
    let mut left = with_assets();
    left.insert_asset_ron(ID, "test::Stats", "(damage: 4, cooldown: 2)")
        .unwrap();
    let mut right = with_assets();
    right.insert_asset_ron(ID, NoDefault::NAME, "(3)").unwrap();
    let differences = left
        .diff_snapshots(&left.snapshot(), &right.snapshot())
        .unwrap();
    assert_eq!(differences.len(), 1);
    let difference = &differences[0];
    assert!(
        difference.left_text.starts_with("test::Stats "),
        "{difference}"
    );
    assert!(
        difference
            .right_text
            .starts_with(&format!("{} ", NoDefault::NAME)),
        "{difference}"
    );
}
