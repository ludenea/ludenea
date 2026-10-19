use std::panic::AssertUnwindSafe;

use ludenea_ecs::schema::Schema;
use ludenea_ecs::{
    Commands, Component, Difference, EcsError, Entity, Event, EventWriter, Events, ResMut,
    Resource, SNAPSHOT_VERSION, ScheduleBuilder, Snapshot, SnapshotError, World, hash_value,
};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Health {
    hp: u32,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Speed(f32);

#[derive(Serialize, Deserialize, Schema, Debug, PartialEq, Clone)]
enum BodyKind {
    Static,
    Dynamic,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Body {
    kind: BodyKind,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Extra;

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Score(u64);

/// Refuses a negative amount on load, as a game component validating with `try_from` does.
#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(try_from = "i32")]
struct Fuel(i32);

impl TryFrom<i32> for Fuel {
    type Error = String;

    fn try_from(amount: i32) -> Result<Self, String> {
        if amount < 0 {
            return Err(format!("fuel must be zero or more, got {amount}"));
        }
        Ok(Fuel(amount))
    }
}

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Gravity(f32);

fn base_world() -> World {
    let mut world = World::new();
    world.register_component::<Health>().unwrap();
    world.register_component::<Speed>().unwrap();
    world.register_component::<Body>().unwrap();
    world.register_resource(Score(7)).unwrap();
    world.register_resource(Gravity(-10.0)).unwrap();
    world
}

/// Three entities, the middle one despawned so the free list is not empty.
fn populated_world() -> (World, Vec<Entity>) {
    let mut world = base_world();
    let a = world.spawn();
    let b = world.spawn();
    let c = world.spawn();
    world.insert(a, Health { hp: 10 }).unwrap();
    world.insert(a, Speed(1.5)).unwrap();
    world
        .insert(
            c,
            Body {
                kind: BodyKind::Static,
            },
        )
        .unwrap();
    world.despawn(b).unwrap();
    (world, vec![a, c])
}

#[test]
fn hash_is_independent_of_insertion_order() {
    let mut first = base_world();
    let a = first.spawn();
    first.insert(a, Health { hp: 3 }).unwrap();
    first.insert(a, Speed(2.0)).unwrap();

    let mut second = base_world();
    let b = second.spawn();
    second.insert(b, Speed(2.0)).unwrap();
    second.insert(b, Health { hp: 3 }).unwrap();

    assert_eq!(first.hash(), second.hash());
    assert_eq!(first.hash_tree(), second.hash_tree());
    assert_eq!(first.hash(), first.hash_tree().root);
}

#[test]
fn single_entity_and_resource_hashes_match_the_tree() {
    let (mut world, entities) = populated_world();
    let dead = world.spawn();
    world.despawn(dead).unwrap();
    let tree = world.hash_tree();
    for (entity, leaf) in entities.iter().zip(&tree.entities) {
        assert_eq!(world.entity_hash(*entity).as_ref(), Some(leaf));
    }
    assert!(world.entity_hash(dead).is_none());
    assert_eq!(world.resource_hashes()[..], tree.resources[..2]);
    for leaf in &tree.resources[..2] {
        assert_eq!(world.resource_hash(&leaf.name), Some(leaf.hash));
    }
    assert_eq!(world.resource_hash("no::Such"), None);
}

#[test]
fn single_component_accessors_read_one_value() {
    let (world, entities) = populated_world();
    let health = Health::NAME;
    assert_eq!(
        world.component_bytes(entities[0], health),
        Some(postcard::to_allocvec(&Health { hp: 10 }).unwrap())
    );
    assert_eq!(
        world.component_ron(entities[0], health).as_deref(),
        Some("(hp:10)")
    );
    assert_eq!(world.component_bytes(entities[1], health), None);
    assert_eq!(world.component_ron(entities[0], "unknown"), None);
    assert!(world.has_component_named(entities[0], health));
    assert!(!world.has_component_named(entities[1], health));
    assert!(!world.has_component_named(entities[0], "unknown"));
}

#[test]
fn hash_changes_when_a_field_changes() {
    let (mut world, entities) = populated_world();
    let before = world.hash_tree();
    world.get_mut::<Health>(entities[0]).unwrap().hp = 11;
    let after = world.hash_tree();
    assert_ne!(before.root, after.root);
    assert_ne!(before.entities[0].hash, after.entities[0].hash);
    assert_eq!(before.entities[1], after.entities[1]);
}

#[test]
fn hash_changes_when_free_list_order_changes() {
    let mut first = base_world();
    let mut second = base_world();
    for world in [&mut first, &mut second] {
        for _ in 0..3 {
            world.spawn();
        }
    }
    let entities: Vec<Entity> = first.entities().collect();
    first.despawn(entities[0]).unwrap();
    first.despawn(entities[1]).unwrap();
    second.despawn(entities[1]).unwrap();
    second.despawn(entities[0]).unwrap();
    assert_ne!(first.hash(), second.hash());
}

#[test]
fn nan_payloads_hash_equal() {
    let mut first = base_world();
    let a = first.spawn();
    first.insert(a, Speed(f32::from_bits(0x7fc0_0001))).unwrap();
    let mut second = base_world();
    let b = second.spawn();
    second.insert(b, Speed(f32::NAN)).unwrap();
    assert_eq!(first.hash(), second.hash());
}

#[test]
fn snapshot_restore_round_trip_preserves_hash_and_iteration_order() {
    let (world, _) = populated_world();
    let snapshot = world.snapshot();
    assert_eq!(snapshot.version, SNAPSHOT_VERSION);
    let decoded = Snapshot::decode(&snapshot.encode()).unwrap();
    assert_eq!(decoded, snapshot);

    let mut restored = base_world();
    restored.spawn();
    restored.register_resource(Score(0)).unwrap();
    restored.restore(&decoded).unwrap();

    assert_eq!(restored.hash_tree(), world.hash_tree());
    assert_eq!(
        restored.entities().collect::<Vec<_>>(),
        world.entities().collect::<Vec<_>>()
    );
    assert_eq!(*restored.resource::<Score>().unwrap(), Score(7));
    // The free list survives, so the next spawn reuses the same slot in both worlds.
    let mut world = world;
    assert_eq!(restored.spawn(), world.spawn());
}

#[test]
fn restore_rejects_other_version_and_leaves_world_unchanged() {
    let (world, _) = populated_world();
    let mut snapshot = world.snapshot();
    snapshot.version = SNAPSHOT_VERSION + 1;
    let mut target = base_world();
    let before = target.hash_tree();
    let error = target.restore(&snapshot).unwrap_err();
    assert!(matches!(
        error,
        SnapshotError::Version { found, expected } if found == SNAPSHOT_VERSION + 1 && expected == SNAPSHOT_VERSION
    ));
    assert_eq!(target.hash_tree(), before);
}

#[test]
fn restore_rejects_unknown_component_and_leaves_world_unchanged() {
    let (mut source, _) = populated_world();
    source.register_component::<Extra>().unwrap();
    let snapshot = source.snapshot();
    let (mut target, _) = populated_world();
    let before = target.hash_tree();
    let error = target.restore(&snapshot).unwrap_err();
    assert!(matches!(&error, SnapshotError::UnknownComponent(name) if name == Extra::NAME));
    assert_eq!(target.hash_tree(), before);
}

#[test]
fn restore_rejects_missing_resource_and_leaves_world_unchanged() {
    let mut source = World::new();
    source.register_component::<Health>().unwrap();
    source.register_component::<Speed>().unwrap();
    source.register_component::<Body>().unwrap();
    source.register_resource(Score(1)).unwrap();
    let snapshot = source.snapshot();
    let (mut target, _) = populated_world();
    let before = target.hash_tree();
    let error = target.restore(&snapshot).unwrap_err();
    assert!(matches!(&error, SnapshotError::MissingResource(name) if name == Gravity::NAME));
    assert_eq!(target.hash_tree(), before);
}

#[test]
fn diff_reports_entity_component_and_field_path() {
    let (mut world, entities) = populated_world();
    let left = world.snapshot();
    world.get_mut::<Health>(entities[0]).unwrap().hp = 9;
    let right = world.snapshot();
    let differences = world.diff_snapshots(&left, &right).unwrap();
    assert_eq!(differences.len(), 1);
    let difference = &differences[0];
    assert_eq!(difference.subject, format!("entity {}", entities[0]));
    assert_eq!(difference.component, Health::NAME);
    assert_eq!(difference.path, ["hp"]);
    assert_eq!(difference.left, Some(serde_json::json!(10)));
    assert_eq!(difference.right, Some(serde_json::json!(9)));
    assert_eq!(
        (
            difference.left_text.as_str(),
            difference.right_text.as_str()
        ),
        ("10", "9")
    );
    assert_eq!(
        difference.to_string(),
        format!("entity 0v0 {} .hp: 10 != 9", Health::NAME)
    );
}

#[test]
fn diff_reports_enum_variant_change() {
    let (mut world, entities) = populated_world();
    let left = world.snapshot();
    world.get_mut::<Body>(entities[1]).unwrap().kind = BodyKind::Dynamic;
    let right = world.snapshot();
    let differences = world.diff_snapshots(&left, &right).unwrap();
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].component, Body::NAME);
    assert_eq!(differences[0].path, ["kind"]);
    assert_eq!(
        (
            differences[0].left_text.as_str(),
            differences[0].right_text.as_str()
        ),
        ("Static", "Dynamic")
    );
}

#[derive(Serialize)]
struct Known {
    flag: bool,
    small: i8,
    count: usize,
    ratio: f32,
    precise: f64,
    name: String,
    items: Vec<u16>,
    maybe: Option<u32>,
    kind: (char, i64),
}

#[test]
fn golden_hash_value_of_known_struct() {
    let value = Known {
        flag: true,
        small: -3,
        count: 42,
        ratio: 0.5,
        precise: -1.25,
        name: "ludenea".to_string(),
        items: vec![1, 2, 3],
        maybe: Some(9),
        kind: ('z', -7),
    };
    assert_eq!(hash_value(&value), 13_400_747_774_808_448_267);
}

#[test]
fn restore_rejects_trailing_component_bytes_and_leaves_world_unchanged() {
    let (world, _) = populated_world();
    let mut snapshot = world.snapshot();
    let (_, values) = snapshot
        .components
        .iter_mut()
        .find(|(name, _)| name == Health::NAME)
        .unwrap();
    values[0].1.push(0);
    let mut target = base_world();
    let before = target.hash_tree();
    let error = target.restore(&snapshot).unwrap_err();
    assert!(matches!(&error, SnapshotError::Decode { name, .. } if name == Health::NAME));
    assert_eq!(target.hash_tree(), before);
}

#[test]
fn snapshot_decode_rejects_trailing_bytes() {
    let (world, _) = populated_world();
    let mut bytes = world.snapshot().encode();
    bytes.push(0);
    assert!(matches!(
        Snapshot::decode(&bytes),
        Err(SnapshotError::Decode { .. })
    ));
}

#[test]
fn insert_boxed_inserts_a_ron_decoded_component_by_name() {
    let mut world = base_world();
    let entity = world.spawn();
    let codec = world.component_codec(Health::NAME).unwrap();
    let value = (codec.decode_ron)("(hp: 4)").unwrap();
    world.insert_boxed(entity, Health::NAME, value).unwrap();
    assert_eq!(*world.get::<Health>(entity).unwrap(), Health { hp: 4 });

    let value = (codec.decode_ron)("(hp: 5)").unwrap();
    let error = world
        .insert_boxed(entity, "game::Unknown", value)
        .unwrap_err();
    assert!(matches!(&error, EcsError::UnregisteredComponent(name) if name == "game::Unknown"));
}

#[test]
fn insert_resource_boxed_replaces_a_resource_by_name() {
    let mut world = base_world();
    let codec = world.resource_codec(Score::NAME).unwrap();
    let value = (codec.decode_ron)("(42)").unwrap();
    world.insert_resource_boxed(Score::NAME, value).unwrap();
    assert_eq!(*world.resource::<Score>().unwrap(), Score(42));

    let value = (codec.decode_ron)("(1)").unwrap();
    let error = world
        .insert_resource_boxed("game::Unknown", value)
        .unwrap_err();
    assert!(matches!(&error, EcsError::UnregisteredResource(name) if name == "game::Unknown"));
}

#[derive(Event, Serialize)]
struct Ping;

#[derive(Resource, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Seen(usize);

fn queue_then_panic(mut commands: Commands, mut pings: EventWriter<Ping>) {
    commands.spawn().insert(Health { hp: 1 });
    pings.send(Ping);
    panic!("aborted tick");
}

fn count_pings(pings: Events<Ping>, mut seen: ResMut<Seen>) {
    seen.0 = pings.iter().count();
}

#[test]
fn restore_clears_queued_commands_and_events() {
    let (mut world, _) = populated_world();
    world.register_event::<Ping>().unwrap();
    world.register_resource(Seen(usize::MAX)).unwrap();
    let before = world.snapshot();

    let mut builder = ScheduleBuilder::default();
    builder.add(queue_then_panic);
    let mut aborting = builder.build(&world).unwrap();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| aborting.run(&mut world)));
    std::panic::set_hook(hook);
    assert!(outcome.is_err());

    world.restore(&before).unwrap();
    let mut builder = ScheduleBuilder::default();
    builder.add(count_pings);
    builder.build(&world).unwrap().run(&mut world);
    assert_eq!(
        world.resource::<Seen>().unwrap().0,
        0,
        "a stale event survived"
    );
    assert_eq!(
        world.snapshot().entities,
        before.entities,
        "a stale command was applied"
    );
}

#[test]
fn diff_output_lines_unchanged() {
    let (mut world, entities) = populated_world();
    let left = world.snapshot();
    world.get_mut::<Health>(entities[0]).unwrap().hp = 9;
    world.insert(entities[1], Speed(2.5)).unwrap();
    world.resource_mut::<Score>().unwrap().0 = 8;
    world.spawn();
    let right = world.snapshot();
    let lines: Vec<String> = world
        .diff_snapshots(&left, &right)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        lines,
        [
            "entities ludenea::Entities .alive[1]: false != true",
            "entities ludenea::Entities .free[0]: 1 != absent",
            "resource snapshot::Score [0]: 7 != 8",
            "entity 0v0 snapshot::Health .hp: 10 != 9",
            "entity 2v0 snapshot::Speed : absent != (2.5)",
        ]
    );
}

#[derive(Serialize, Deserialize, Schema, Debug, PartialEq, Clone)]
struct Slot {
    weight: u32,
    amount: u32,
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Inventory {
    counts: std::collections::BTreeMap<String, u32>,
    slots: Vec<Slot>,
}

/// String map keys and a whole struct inside a list, as milestone 1 printed them.
#[test]
fn diff_output_lines_unchanged_for_maps_and_list_items() {
    let mut world = World::new();
    world.register_component::<Inventory>().unwrap();
    let entity = world.spawn();
    let slot = |weight, amount| Slot { weight, amount };
    let counts = |pairs: &[(&str, u32)]| {
        pairs
            .iter()
            .map(|(name, count)| (name.to_string(), *count))
            .collect()
    };
    let inventory = Inventory {
        counts: counts(&[("apple", 1), ("pear", 2)]),
        slots: vec![slot(5, 1)],
    };
    world.insert(entity, inventory).unwrap();
    let left = world.snapshot();
    let inventory = Inventory {
        counts: counts(&[("apple", 3)]),
        slots: vec![slot(5, 1), slot(6, 2)],
    };
    world.insert(entity, inventory).unwrap();
    let lines: Vec<String> = world
        .diff_snapshots(&left, &world.snapshot())
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        lines,
        [
            "entity 0v0 snapshot::Inventory .counts.apple: 1 != 3",
            "entity 0v0 snapshot::Inventory .counts.pear: 2 != absent",
            "entity 0v0 snapshot::Inventory .slots[1]: absent != {\"amount\":2,\"weight\":6}",
        ]
    );
}

#[test]
fn diff_quotes_map_keys_that_read_as_an_index_or_nothing() {
    let mut world = World::new();
    world.register_component::<Inventory>().unwrap();
    let entity = world.spawn();
    let inventory = |count| Inventory {
        counts: [("[0]", count), ("", count)]
            .map(|(name, count)| (name.to_string(), count))
            .into(),
        slots: Vec::new(),
    };
    world.insert(entity, inventory(1)).unwrap();
    let left = world.snapshot();
    world.insert(entity, inventory(2)).unwrap();
    let lines: Vec<String> = world
        .diff_snapshots(&left, &world.snapshot())
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        lines,
        [
            "entity 0v0 snapshot::Inventory .counts[\"\"]: 1 != 2",
            "entity 0v0 snapshot::Inventory .counts[\"[0]\"]: 1 != 2",
        ]
    );
}

#[derive(Serialize, Deserialize, Schema, Debug, PartialEq, Clone)]
enum Shape {
    Circle { radius: u32 },
    Square(u32),
}

#[derive(Component, Serialize, Deserialize, Debug, PartialEq, Clone)]
struct Outline {
    shape: Shape,
    kind: BodyKind,
}

#[test]
fn structured_difference_reports_variant_names() {
    let mut world = World::new();
    world.register_component::<Outline>().unwrap();
    let entity = world.spawn();
    let outline = |shape, kind| Outline { shape, kind };
    world
        .insert(
            entity,
            outline(Shape::Circle { radius: 1 }, BodyKind::Static),
        )
        .unwrap();
    let left = world.snapshot();
    world
        .insert(entity, outline(Shape::Square(2), BodyKind::Dynamic))
        .unwrap();
    let right = world.snapshot();
    let differences = world.diff_snapshots(&left, &right).unwrap();
    let difference = |path: &str, left: &str, right: &str| Difference {
        subject: "entity 0v0".to_string(),
        component: Outline::NAME.to_string(),
        path: vec![path.to_string()],
        left: Some(serde_json::json!(left)),
        right: Some(serde_json::json!(right)),
        left_text: left.to_string(),
        right_text: right.to_string(),
    };
    assert_eq!(
        differences,
        [
            difference("kind", "Static", "Dynamic"),
            difference("shape", "Circle", "Square"),
        ]
    );
    assert_eq!(
        differences[0].to_string(),
        format!("entity 0v0 {} .kind: Static != Dynamic", Outline::NAME)
    );

    // Within one variant, the walk goes on into its fields.
    world
        .insert(
            entity,
            outline(Shape::Circle { radius: 3 }, BodyKind::Static),
        )
        .unwrap();
    let differences = world.diff_snapshots(&left, &world.snapshot()).unwrap();
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].path, ["shape", "radius"]);
    assert_eq!(
        (differences[0].left.clone(), differences[0].right.clone()),
        (Some(serde_json::json!(1)), Some(serde_json::json!(3)))
    );
}

#[derive(ludenea_ecs::Asset, Serialize, Deserialize, Debug, PartialEq)]
#[asset(name = "test::Stats")]
struct Stats {
    damage: u32,
}

#[test]
fn asset_only_divergence_is_listed() {
    let mut world = base_world();
    world.register_asset::<Stats>().unwrap();
    let id = ludenea_ecs::AssetId(7);
    world
        .insert_asset_ron(id, "test::Stats", "(damage: 1)")
        .unwrap();
    let left = world.snapshot();
    world
        .insert_asset_ron(id, "test::Stats", "(damage: 2)")
        .unwrap();
    let right = world.snapshot();
    let lines: Vec<String> = world
        .diff_snapshots(&left, &right)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(lines, [format!("asset {id} test::Stats .damage: 1 != 2")]);

    world.clear_assets();
    let lines: Vec<String> = world
        .diff_snapshots(&left, &world.snapshot())
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        lines,
        [format!("asset {id} test::Stats : (damage:1) != absent")]
    );
}

#[test]
fn snapshots_with_other_fingerprints_are_refused() {
    let (world, _) = populated_world();
    let mut left = world.snapshot();
    let mut right = world.snapshot();
    (left.fingerprint, right.fingerprint) = (1, 2);
    assert!(matches!(
        world.diff_snapshots(&left, &right),
        Err(SnapshotError::Fingerprint { left: 1, right: 2 })
    ));
    // An unknown fingerprint (0) compares with any.
    right.fingerprint = 0;
    assert!(world.diff_snapshots(&left, &right).unwrap().is_empty());
}

#[test]
fn restore_names_the_entity_and_why_its_component_refuses_the_value() {
    let fuel_world = || {
        let mut world = World::new();
        world.register_component::<Fuel>().unwrap();
        world
    };
    let mut source = fuel_world();
    let tank = source.spawn();
    source.insert(tank, Fuel(-3)).unwrap();
    let mut target = fuel_world();
    let before = target.hash_tree();

    let error = target.restore(&source.snapshot()).unwrap_err();

    assert!(
        matches!(&error, SnapshotError::Decode { name, reason } if name == Fuel::NAME
            && *reason == format!("entity {tank}: fuel must be zero or more, got -3")),
        "{error}"
    );
    assert_eq!(target.hash_tree(), before);
}
