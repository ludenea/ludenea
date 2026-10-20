use std::collections::BTreeMap;

use boxes::BoxesInput;
use ludenea::control::Event;
use ludenea::control::*;
use ludenea::prelude::*;
use ludenea_app::EntityRef;
use serde_json::{Value, json};

fn scene() -> String {
    boxes::generate_scene(30, 7)
}

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
}

/// A boxes host with the scene and `ticks` idle recorded inputs loaded.
fn loaded(ticks: u64) -> Host {
    let mut host = Host::new(boxes::app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: scene(),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let mut log = InputLog::new(1);
    for _ in 0..ticks {
        log.record(&[BoxesInput::default()]);
    }
    ok(
        &mut host,
        Command::LoadInputs {
            log: Bytes(log.encode()),
        },
    );
    host
}

fn step(host: &mut Host, ticks: u32) {
    ok(
        host,
        Command::Step {
            ticks,
            record_hashes: false,
        },
    );
}

fn timeline(host: &mut Host) -> Timeline {
    match ok(host, Command::GetTimeline) {
        Reply::Timeline(timeline) => timeline,
        other => panic!("{other:?}"),
    }
}

fn hash(host: &mut Host) -> Hash64 {
    match ok(host, Command::GetHash) {
        Reply::Hash { hash, .. } => hash,
        other => panic!("{other:?}"),
    }
}

fn entities(host: &mut Host) -> Vec<Entity> {
    match ok(
        host,
        Command::QueryEntities {
            filter: String::new(),
            offset: 0,
            limit: u32::MAX,
        },
    ) {
        Reply::Entities { rows, .. } => rows.into_iter().map(|row| row.entity).collect(),
        other => panic!("{other:?}"),
    }
}

/// The first crate of the scene, a circle.
fn first_crate(host: &mut Host) -> Entity {
    entities(host)[4]
}

fn components(host: &mut Host, entity: Entity) -> BTreeMap<String, ComponentValue> {
    match ok(host, Command::GetEntity { entity }) {
        Reply::Entity { components, .. } => components,
        other => panic!("{other:?}"),
    }
}

fn json_of(host: &mut Host, entity: Entity, component: &str) -> Value {
    match components(host, entity).remove(component) {
        Some(ComponentValue::Json { value }) => value,
        other => panic!("{other:?}"),
    }
}

fn resources(host: &mut Host) -> BTreeMap<String, ComponentValue> {
    match ok(host, Command::GetResources) {
        Reply::Resources { resources } => resources,
        other => panic!("{other:?}"),
    }
}

fn save_scene(host: &mut Host) -> String {
    match ok(host, Command::SaveScene) {
        Reply::Scene { ron } => ron,
        other => panic!("{other:?}"),
    }
}

fn field(name: &str) -> PathSegment {
    PathSegment::Field {
        name: name.to_string(),
    }
}

fn set_pose(entity: Entity, path: &[&str], value: Value) -> Command {
    Command::SetField {
        target: FieldTarget::Component {
            entity,
            component: "ludenea::Pose".to_string(),
        },
        path: path.iter().map(|name| field(name)).collect(),
        value,
    }
}

/// Sends `command` the way a remote client does: as JSON text, parsed back.
fn through_json_text(command: Command) -> Command {
    let text = serde_json::to_string(&command).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn recorded_edits(host: &mut Host) -> EditLog {
    match ok(host, Command::ExportSession) {
        Reply::Session { edits, .. } => EditLog::decode(&edits.0).unwrap(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn set_field_on_pose_position_records_insert_edit() {
    let mut host = loaded(20);
    step(&mut host, 10);
    let entity = first_crate(&mut host);
    let x = 0.1_f32;
    let command = through_json_text(set_pose(entity, &["position", "x"], json!(x)));
    assert_eq!(ok(&mut host, command), Reply::Done);

    let pose = json_of(&mut host, entity, "ludenea::Pose");
    assert_eq!(
        (pose["position"]["x"].as_f64().unwrap() as f32).to_bits(),
        x.to_bits()
    );
    let edits = recorded_edits(&mut host);
    assert_eq!(edits.edits.len(), 1);
    assert_eq!(edits.edits[0].tick, 10);
    let address = match ok(&mut host, Command::GetInstances) {
        Reply::Instances { load_addresses, .. } => load_addresses
            .into_iter()
            .find(|(loaded, _)| *loaded == entity)
            .map(|(_, address)| address)
            .unwrap(),
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        &edits.edits[0].edit,
        Edit::Insert { entity: EntityRef::Loaded(at), component, .. }
            if *at == address && component == "ludenea::Pose"
    ));
    assert_eq!(timeline(&mut host).edit_ticks, vec![10]);
}

#[test]
fn set_field_with_non_unit_rotation_is_invalid_edit_and_records_nothing() {
    let mut host = loaded(20);
    step(&mut host, 5);
    let entity = first_crate(&mut host);
    let before = hash(&mut host);
    let error = host
        .handle(set_pose(entity, &["rotation", "cos"], json!(2.0)))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidEdit);
    assert!(error.message.contains("ludenea::Pose"), "{}", error.message);
    assert!(error.message.contains("rotation"), "{}", error.message);
    assert!(
        error.message.contains(&entity.to_string()),
        "{}",
        error.message
    );
    assert_eq!(hash(&mut host), before);
    assert!(timeline(&mut host).edit_ticks.is_empty());
}

#[test]
fn set_field_on_dead_entity_is_no_such_entity() {
    let mut host = loaded(20);
    step(&mut host, 5);
    let entity = first_crate(&mut host);
    ok(&mut host, Command::Despawn { entity });
    let error = host
        .handle(set_pose(entity, &["position", "x"], json!(1.0)))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NoSuchEntity);
    assert!(
        error.message.contains(&entity.to_string()),
        "{}",
        error.message
    );
    assert_eq!(timeline(&mut host).edit_ticks, vec![5]);
}

#[test]
fn set_field_bad_path_is_invalid_path() {
    let mut host = loaded(20);
    let entity = first_crate(&mut host);
    let error = host
        .handle(set_pose(entity, &["position", "z"], json!(1.0)))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidPath);
    assert!(error.message.contains("position.z"), "{}", error.message);
    assert!(error.message.contains("ludenea::Pose"), "{}", error.message);
}

#[test]
fn set_field_through_newtype_variant_changes_circle_radius() {
    let mut host = loaded(20);
    let entity = first_crate(&mut host);
    let command = Command::SetField {
        target: FieldTarget::Component {
            entity,
            component: "ludenea::Collider".to_string(),
        },
        path: vec![field("shape"), field("Circle"), field("radius")],
        value: json!(0.75),
    };
    assert_eq!(ok(&mut host, command), Reply::Done);
    let collider = json_of(&mut host, entity, "ludenea::Collider");
    assert_eq!(collider["shape"]["Circle"]["radius"], json!(0.75));
    // At tick 0 the edit changes the scene.
    assert!(timeline(&mut host).scene_dirty);
}

#[test]
fn set_field_on_time_is_invalid_edit() {
    let mut host = loaded(20);
    step(&mut host, 3);
    let error = host
        .handle(Command::SetField {
            target: FieldTarget::Resource {
                resource: "ludenea::Time".to_string(),
            },
            path: vec![field("dt")],
            value: json!(0.5),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidEdit);
    assert!(error.message.contains("ludenea::Time"), "{}", error.message);
}

#[test]
fn set_field_with_huge_substeps_is_invalid_edit_and_records_nothing() {
    let mut host = loaded(20);
    step(&mut host, 3);
    let before = hash(&mut host);
    let error = host
        .handle(Command::SetField {
            target: FieldTarget::Resource {
                resource: "ludenea::PhysicsSettings".to_string(),
            },
            path: vec![field("substeps")],
            value: json!(400_000_000),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidEdit);
    assert!(
        error.message.contains("ludenea::PhysicsSettings")
            && error
                .message
                .contains("substeps must be within 1 to 64, got 400000000"),
        "{}",
        error.message
    );
    assert_eq!(hash(&mut host), before);
    assert!(timeline(&mut host).edit_ticks.is_empty());
}

#[test]
fn rng_state_round_trips_as_decimal_string() {
    let mut host = loaded(20);
    step(&mut host, 2);
    let rng = |host: &mut Host| match resources(host).remove("ludenea::Rng") {
        Some(ComponentValue::Json { value }) => value,
        other => panic!("{other:?}"),
    };
    assert!(rng(&mut host)["state"].is_string(), "{}", rng(&mut host));

    let state = u64::MAX - 12;
    let command = through_json_text(Command::SetField {
        target: FieldTarget::Resource {
            resource: "ludenea::Rng".to_string(),
        },
        path: vec![field("state")],
        value: json!(state.to_string()),
    });
    assert_eq!(ok(&mut host, command), Reply::Done);
    assert_eq!(rng(&mut host)["state"], json!(state.to_string()));
    // Small integers stay numbers.
    let time = match resources(&mut host).remove("ludenea::Time") {
        Some(ComponentValue::Json { value }) => value,
        other => panic!("{other:?}"),
    };
    assert_eq!(time["tick"], json!(2));
}

#[test]
fn spawn_from_json_components_returns_entity() {
    let mut host = loaded(20);
    step(&mut host, 3);
    let pose = json!({"position": {"x": 1.5, "y": 10.0}, "rotation": {"cos": 1.0, "sin": 0.0}});
    let first = first_crate(&mut host);
    let crate_body = json_of(&mut host, first, "ludenea::Body");
    let reply = ok(
        &mut host,
        Command::Spawn {
            components: BTreeMap::from([
                ("ludenea::Pose".to_string(), pose.clone()),
                ("ludenea::Body".to_string(), crate_body),
            ]),
        },
    );
    let Reply::Spawned { entity } = reply else {
        panic!("{reply:?}")
    };
    assert!(entities(&mut host).contains(&entity));
    assert_eq!(json_of(&mut host, entity, "ludenea::Pose"), pose);

    ok(
        &mut host,
        Command::RemoveComponent {
            entity,
            component: "ludenea::Body".to_string(),
        },
    );
    assert!(!components(&mut host, entity).contains_key("ludenea::Body"));
    let error = host
        .handle(Command::InsertComponent {
            entity,
            component: "game::Missing".to_string(),
            value: json!(null),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownComponent);
    assert!(error.message.contains("game::Missing"), "{}", error.message);
}

#[test]
fn contact_cache_is_ron_value() {
    let mut host = loaded(120);
    // Once crates touch, the cache has entity pair keys, which JSON objects cannot hold.
    step(&mut host, 120);
    match resources(&mut host).remove("ludenea::ContactCache") {
        Some(ComponentValue::Ron { text }) => assert!(text.contains("pairs"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn save_scene_returns_session_scene_at_any_tick() {
    let mut host = loaded(100);
    let at_zero = save_scene(&mut host);
    step(&mut host, 100);
    assert_eq!(save_scene(&mut host), at_zero);
    assert_eq!(at_zero, scene());
}

#[test]
fn save_scene_keeps_the_scene_dirty_until_marked_saved() {
    let mut host = loaded(0);
    let entity = first_crate(&mut host);
    ok(&mut host, set_pose(entity, &["position", "x"], json!(3.0)));
    let written = save_scene(&mut host);
    // The text is not on disk yet: a failed write leaves the edit unsaved.
    assert!(timeline(&mut host).scene_dirty);

    // An edit made while the text is written stays unsaved.
    resimulate(&mut host);
    ok(&mut host, set_pose(entity, &["position", "y"], json!(4.0)));
    ok(
        &mut host,
        Command::MarkSceneSaved {
            text: written.clone(),
        },
    );
    assert!(timeline(&mut host).scene_dirty);

    let latest = save_scene(&mut host);
    assert_ne!(latest, written);
    ok(&mut host, Command::MarkSceneSaved { text: latest });
    assert!(!timeline(&mut host).scene_dirty);
}

#[test]
fn a_mark_made_after_a_later_edit_still_records_the_written_text() {
    let mut host = loaded(0);
    let entity = first_crate(&mut host);
    ok(&mut host, set_pose(entity, &["position", "x"], json!(3.0)));
    let written = save_scene(&mut host);
    resimulate(&mut host);
    ok(&mut host, set_pose(entity, &["position", "y"], json!(4.0)));
    ok(
        &mut host,
        Command::MarkSceneSaved {
            text: written.clone(),
        },
    );

    // Back to the written text, which the file holds.
    resimulate(&mut host);
    ok(&mut host, Command::Undo);
    assert_eq!(save_scene(&mut host), written);
    assert!(!timeline(&mut host).scene_dirty);

    // Back to the text loaded before the save, which the file no longer holds.
    resimulate(&mut host);
    ok(&mut host, Command::Undo);
    assert_eq!(save_scene(&mut host), scene());
    assert!(timeline(&mut host).scene_dirty);
    match ok(&mut host, Command::ExportSession) {
        Reply::Session { saved_scene, .. } => assert_eq!(saved_scene, Some(written)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn apply_to_scene_copies_value_and_marks_dirty() {
    let mut host = loaded(30);
    step(&mut host, 20);
    let entity = first_crate(&mut host);
    let fallen = json_of(&mut host, entity, "ludenea::Pose");
    assert_ne!(fallen, json_of_scene(entity));
    ok(
        &mut host,
        Command::ApplyToScene {
            entity,
            component: "ludenea::Pose".to_string(),
        },
    );
    // The scene, shown at tick 0 while the recorded future resimulates.
    let after = timeline(&mut host);
    assert!(
        after.scene_dirty && after.can_undo && after.resimulating,
        "{after:?}"
    );
    assert_eq!(after.current, 0);
    assert_eq!(json_of(&mut host, entity, "ludenea::Pose"), fallen);

    resimulate(&mut host);
    assert_eq!(timeline(&mut host).current, 20);
    let applied = save_scene(&mut host);
    ok(
        &mut host,
        Command::MarkSceneSaved {
            text: applied.clone(),
        },
    );
    assert!(!timeline(&mut host).scene_dirty);
    let mut fresh = loaded(30);
    ok(
        &mut fresh,
        Command::ReplaceScene {
            scene: applied.clone(),
            assets: Vec::new(),
        },
    );
    resimulate(&mut fresh);
    step(&mut fresh, 20);
    assert_eq!(hash(&mut host), hash(&mut fresh));

    ok(&mut host, Command::Undo);
    assert_eq!(save_scene(&mut host), scene());
    resimulate(&mut host);
    assert_eq!(timeline(&mut host).current, 20);
    assert_eq!(json_of(&mut host, entity, "ludenea::Pose"), fallen);
    ok(&mut host, Command::Redo);
    assert_eq!(save_scene(&mut host), applied);
}

#[test]
fn apply_to_scene_of_an_unchanged_value_records_nothing() {
    let mut host = loaded(30);
    let entity = first_crate(&mut host);
    let apply = Command::ApplyToScene {
        entity,
        component: "ludenea::Pose".to_string(),
    };
    ok(&mut host, apply.clone());
    let at_zero = timeline(&mut host);
    assert!(!at_zero.can_undo && !at_zero.scene_dirty, "{at_zero:?}");
    step(&mut host, 1);
    // A static body has not moved after one tick.
    let ground = entities(&mut host)[0];
    ok(
        &mut host,
        Command::ApplyToScene {
            entity: ground,
            component: "ludenea::Pose".to_string(),
        },
    );
    let later = timeline(&mut host);
    assert!(!later.can_undo && !later.resimulating, "{later:?}");
    assert_eq!(later.current, 1);
}

#[test]
fn errors_name_the_entity_once() {
    let mut host = loaded(30);
    step(&mut host, 5);
    let ground = entities(&mut host)[0];
    let named = |message: &str| message.matches(&ground.to_string()).count();
    let missing = host
        .handle(Command::ApplyToScene {
            entity: ground,
            component: "boxes::Crate".to_string(),
        })
        .unwrap_err();
    assert_eq!(missing.code, ErrorCode::InvalidEdit);
    assert_eq!(named(&missing.message), 1, "{}", missing.message);
    let pose = json!({"position": {"x": 0.0, "y": 5.0}, "rotation": {"cos": 1.0, "sin": 0.0}});
    let Reply::Spawned { entity: spawned } = ok(
        &mut host,
        Command::Spawn {
            components: BTreeMap::from([("ludenea::Pose".to_string(), pose)]),
        },
    ) else {
        panic!()
    };
    let not_in_scene = host
        .handle(Command::ApplyToScene {
            entity: spawned,
            component: "ludenea::Pose".to_string(),
        })
        .unwrap_err();
    let count = not_in_scene.message.matches(&spawned.to_string()).count();
    assert_eq!(count, 1, "{}", not_in_scene.message);
}

#[test]
fn insert_component_refuses_to_replace_an_existing_one() {
    let mut host = loaded(30);
    step(&mut host, 5);
    let entity = first_crate(&mut host);
    let pose = json_of(&mut host, entity, "ludenea::Pose");
    let before = recorded_edits(&mut host);
    let error = host
        .handle(Command::InsertComponent {
            entity,
            component: "ludenea::Pose".to_string(),
            value: pose,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidEdit);
    assert!(error.message.contains("SetField"), "{}", error.message);
    assert_eq!(recorded_edits(&mut host), before);
}

/// Advances while paused until resimulation is done.
fn resimulate(host: &mut Host) {
    for _ in 0..1_000 {
        host.advance(0.0);
        if !timeline(host).resimulating {
            return;
        }
    }
    panic!("resimulation did not finish");
}

#[test]
fn apply_to_scene_rejects_an_entity_spawned_after_tick_zero() {
    let mut host = loaded(30);
    step(&mut host, 5);
    let pose = json!({"position": {"x": 0.0, "y": 5.0}, "rotation": {"cos": 1.0, "sin": 0.0}});
    let Reply::Spawned { entity } = ok(
        &mut host,
        Command::Spawn {
            components: BTreeMap::from([("ludenea::Pose".to_string(), pose)]),
        },
    ) else {
        panic!()
    };
    let error = host
        .handle(Command::ApplyToScene {
            entity,
            component: "ludenea::Pose".to_string(),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NoSuchEntity);
    assert!(error.message.contains("after tick 0"), "{}", error.message);
    assert_eq!(timeline(&mut host).current, 5);
}

/// The Pose of `entity` in the boxes scene, at tick 0.
fn json_of_scene(entity: Entity) -> Value {
    let mut host = loaded(0);
    json_of(&mut host, entity, "ludenea::Pose")
}

#[test]
fn ticked_after_an_edit_says_the_edit_is_pending() {
    let mut host = loaded(20);
    step(&mut host, 10);
    host.advance(0.0);
    let entity = first_crate(&mut host);
    ok(&mut host, set_pose(entity, &["position", "x"], json!(0.1)));
    let events = host.advance(0.0);
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Ticked {
                tick: 10,
                pending: true,
                ..
            }
        )),
        "{events:?}"
    );
    step(&mut host, 1);
    let events = host.advance(0.0);
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Ticked {
                tick: 11,
                pending: false,
                ..
            }
        )),
        "{events:?}"
    );
}

#[test]
fn a_spawn_that_needs_more_local_identifiers_than_remain_is_an_invalid_edit() {
    // The new entity alone would take the last `next_id`. In the second scene it fits, but
    // the cached contact names two stale entities, which need a local identifier each.
    let contact = r#""ludenea::ContactCache": (pairs: {((index: 5, generation: 0), (index: 6, generation: 0)): (touching: false, points: [])})"#;
    for (next_id, resources) in [(4294967294u32, ""), (4294967293, contact)] {
        let scene = format!(
            "Scene(version: 2, next_id: {next_id}, resources: {{{resources}}}, entities: [])"
        );
        let mut host = Host::new(boxes::app).unwrap();
        ok(
            &mut host,
            Command::LoadScene {
                scene: scene.clone(),
                assets: Vec::new(),
                document: Document::Scene,
            },
        );
        let before = hash(&mut host);
        let error = host
            .handle(Command::Spawn {
                components: BTreeMap::new(),
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidEdit, "{resources}");
        assert!(
            error.message.contains("renumber the scene"),
            "{}",
            error.message
        );
        assert_eq!(hash(&mut host), before);
        assert!(timeline(&mut host).edit_ticks.is_empty());
        match ok(&mut host, Command::SaveScene) {
            Reply::Scene { ron } => assert_eq!(ron, scene),
            other => panic!("{other:?}"),
        }
    }
}

/// The load address of each entity the last scene load spawned, sorted.
fn load_addresses(host: &mut Host) -> Vec<(Entity, Vec<u32>)> {
    match ok(host, Command::GetInstances) {
        Reply::Instances {
            mut load_addresses, ..
        } => {
            load_addresses.sort_by(|a, b| a.1.cmp(&b.1));
            load_addresses
        }
        other => panic!("{other:?}"),
    }
}

fn addresses(host: &mut Host) -> Vec<Vec<u32>> {
    load_addresses(host)
        .into_iter()
        .map(|(_, address)| address)
        .collect()
}

fn despawns(entities: &[Entity]) -> Command {
    Command::Batch {
        commands: entities
            .iter()
            .map(|&entity| Command::Despawn { entity })
            .collect(),
    }
}

#[test]
fn a_batch_of_tick_zero_despawns_after_play_is_one_undo_step() {
    let mut host = loaded(100);
    step(&mut host, 100);
    ok(&mut host, Command::Seek { tick: 0 });
    let before = load_addresses(&mut host);
    let crates = &before[4..7];
    let entities: Vec<Entity> = crates.iter().map(|(entity, _)| *entity).collect();

    assert_eq!(ok(&mut host, despawns(&entities)), Reply::Done);

    let after = addresses(&mut host);
    assert_eq!(after.len(), before.len() - 3);
    for (_, address) in crates {
        assert!(
            !after.contains(address),
            "{address:?} is still in the scene"
        );
    }
    let edited = timeline(&mut host);
    assert!(edited.scene_dirty && edited.resimulating);
    assert_eq!(edited.undo_label.as_deref(), Some("Undo scene edit"));
    resimulate(&mut host);

    ok(&mut host, Command::Undo);
    let undone = timeline(&mut host);
    assert!(!undone.can_undo, "one undo takes the batch back");
    assert!(!undone.scene_dirty);
    resimulate(&mut host);
    let restored: Vec<(Entity, Vec<u32>)> = load_addresses(&mut host);
    assert_eq!(restored, before);
}

#[test]
fn a_batch_past_tick_zero_is_one_timed_edit_and_one_undo_step() {
    let mut host = loaded(100);
    step(&mut host, 50);
    let crates = &entities(&mut host)[4..7];
    let moved = |host: &mut Host| -> Vec<Value> {
        crates
            .iter()
            .map(|&entity| json_of(host, entity, "ludenea::Pose")["position"]["y"].clone())
            .collect()
    };
    let before = moved(&mut host);
    let commands = crates
        .iter()
        .map(|&entity| set_pose(entity, &["position", "y"], json!(40.0)))
        .collect();

    let command = through_json_text(Command::Batch { commands });
    assert_eq!(ok(&mut host, command), Reply::Done);

    assert_eq!(moved(&mut host), vec![json!(40.0); 3]);
    let log = recorded_edits(&mut host);
    assert_eq!(log.edits.len(), 1, "{log:?}");
    assert_eq!(log.edits[0].tick, 50);
    let Edit::Group(edits) = &log.edits[0].edit else {
        panic!("{log:?}");
    };
    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| matches!(edit, Edit::Insert { .. })));
    assert_eq!(
        timeline(&mut host).undo_label.as_deref(),
        Some("Undo edit at tick 50 (removes Branch 2)")
    );

    ok(&mut host, Command::Undo);
    assert!(
        !timeline(&mut host).can_undo,
        "one undo takes the batch back"
    );
    assert_eq!(moved(&mut host), before);
}

#[test]
fn a_batch_of_one_command_records_that_command() {
    let mut host = loaded(20);
    step(&mut host, 10);
    let entity = first_crate(&mut host);
    let commands = vec![set_pose(entity, &["position", "y"], json!(12.0))];
    ok(&mut host, Command::Batch { commands });
    let log = recorded_edits(&mut host);
    assert!(
        matches!(
            &log.edits[..],
            [TimedEdit {
                edit: Edit::Insert { .. },
                ..
            }]
        ),
        "{log:?}"
    );
}

#[test]
fn a_batch_holds_edit_commands_only() {
    for tick in [0, 10] {
        let mut host = loaded(20);
        step(&mut host, 10);
        ok(&mut host, Command::Seek { tick });
        let entity = first_crate(&mut host);
        let before = (hash(&mut host), save_scene(&mut host));
        for other in [
            Command::Play,
            Command::Batch {
                commands: Vec::new(),
            },
        ] {
            let commands = vec![set_pose(entity, &["position", "y"], json!(12.0)), other];
            let error = host.handle(Command::Batch { commands }).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidCommand, "{}", error.message);
            assert!(
                error.message.contains("command 1 of the batch"),
                "{}",
                error.message
            );
            assert_eq!((hash(&mut host), save_scene(&mut host)), before);
            let timeline = timeline(&mut host);
            assert!(!timeline.can_undo && timeline.edit_ticks.is_empty());
        }
    }
}

#[test]
fn a_batch_whose_edit_fails_changes_nothing() {
    for tick in [0, 10] {
        let mut host = loaded(20);
        step(&mut host, 10);
        ok(&mut host, Command::Seek { tick });
        let entity = first_crate(&mut host);
        let before = (hash(&mut host), save_scene(&mut host), addresses(&mut host));
        // The second despawn finds the entity gone: at tick 0 after the scene loaded once.
        let error = host.handle(despawns(&[entity, entity])).unwrap_err();
        assert_eq!(error.code, ErrorCode::NoSuchEntity, "{}", error.message);
        assert_eq!(
            (hash(&mut host), save_scene(&mut host), addresses(&mut host)),
            before,
            "at tick {tick}"
        );
        let timeline = timeline(&mut host);
        assert!(!timeline.can_undo && !timeline.scene_dirty && timeline.edit_ticks.is_empty());
        assert!(!timeline.resimulating);
    }
}

#[test]
fn a_batch_refuses_a_value_changed_twice() {
    let mut host = loaded(20);
    step(&mut host, 10);
    let entity = first_crate(&mut host);
    let commands = vec![
        set_pose(entity, &["position", "x"], json!(1.0)),
        set_pose(entity, &["position", "y"], json!(2.0)),
    ];
    let error = host.handle(Command::Batch { commands }).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert_eq!(
        error.message,
        format!(
            "command 1 of the batch changes `ludenea::Pose` of entity {entity} again; a batch \
             changes each component and resource once, so send the whole change in one command"
        )
    );
    assert!(timeline(&mut host).edit_ticks.is_empty());
}

#[test]
fn a_tick_zero_batch_needing_more_local_identifiers_than_remain_changes_nothing() {
    // The first spawn takes the identifier before the last; the second has none left.
    let scene = "Scene(version: 2, next_id: 4294967293, resources: {}, entities: [])".to_string();
    let mut host = Host::new(boxes::app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: scene.clone(),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let spawn = || Command::Spawn {
        components: BTreeMap::new(),
    };
    let before = hash(&mut host);
    let error = host
        .handle(Command::Batch {
            commands: vec![spawn(), spawn()],
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidEdit);
    assert!(
        error.message.contains("renumber the scene"),
        "{}",
        error.message
    );
    assert_eq!((hash(&mut host), save_scene(&mut host)), (before, scene));
    assert!(!timeline(&mut host).can_undo);
    // One spawn alone fits.
    ok(&mut host, spawn());
}
