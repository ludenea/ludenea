use std::collections::BTreeMap;

use ludenea_app::{App, InputLog, Inputs, RawGamepad, RawInput, Rng, Sim};
use ludenea_control::*;
use ludenea_ecs::schema::{Schema, TypeSchema};
use ludenea_ecs::{Component, Entity, EntityHash, HashTree, NamedHash, Query, ResMut, Runtime};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize)]
struct Score(u32);

fn grow(mut scores: Query<&mut Score>, mut rng: ResMut<Rng>, inputs: Runtime<Inputs<u8>>) {
    let bonus = u32::from(*inputs.player(0));
    for score in scores.iter_mut() {
        score.0 += rng.range_u32(1, 4) + bonus;
    }
}

fn app() -> App {
    let mut app = App::new();
    app.input::<u8>().component::<Score>().system(Sim, grow);
    app
}

const SCENE: &str = r#"Scene(version: 1, resources: {}, entities: [{ "host::Score": (5) }])"#;

fn client() -> InProcess {
    let mut client = InProcess::connect(Host::new(app()).unwrap(), PROTOCOL_VERSION).unwrap();
    client
        .request(Command::LoadScene { ron: SCENE.into() })
        .unwrap();
    client
}

fn step(client: &mut InProcess, ticks: u32) -> Vec<Hash64> {
    match client.request(Command::Step {
        ticks,
        record_hashes: true,
    }) {
        Ok(Reply::Stepped { hashes, .. }) => hashes,
        other => panic!("{other:?}"),
    }
}

fn snapshot(client: &mut InProcess) -> Bytes {
    match client.request(Command::TakeSnapshot) {
        Ok(Reply::Snapshot { snapshot, .. }) => snapshot,
        other => panic!("{other:?}"),
    }
}

fn log(ticks: &[u8]) -> Bytes {
    let mut log = InputLog::new(1);
    for tick in ticks {
        log.record(&[*tick]);
    }
    Bytes(log.encode())
}

#[test]
fn handshake_with_version_1_is_refused() {
    let error = InProcess::connect(Host::new(app()).unwrap(), 1)
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::VersionMismatch);
    assert_eq!(error.message, "client speaks protocol 1, engine speaks 2");
}

#[test]
fn welcome_carries_game_schemas() {
    let client = InProcess::connect(Host::new(app()).unwrap(), PROTOCOL_VERSION).unwrap();
    let game = client.game();
    let score = game
        .components
        .iter()
        .find(|named| named.name == "host::Score")
        .unwrap();
    assert_eq!(score.schema, Score::schema());
    let resources: Vec<&str> = game.resources.iter().map(|r| r.name.as_str()).collect();
    assert!(resources.contains(&"ludenea::Time"), "{resources:?}");
    assert_eq!(game.input, Some(TypeSchema::U8));
    assert_eq!(game.tick_rate, 60);
    assert!(game.debug_layers.is_empty());
}

#[test]
fn live_input_without_mapping_is_refused() {
    let error = client()
        .request(Command::SetLiveInput {
            players: vec![RawInput::default()],
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NoInputMapping);
}

#[test]
fn hash_reports_pending_edits_and_tick() {
    let mut client = client();
    step(&mut client, 2);
    let Ok(Reply::Hash { tick, pending, .. }) = client.request(Command::GetHash) else {
        panic!()
    };
    assert_eq!((tick, pending), (2, false));
}

#[test]
fn step_with_record_hashes_returns_one_hash_per_tick() {
    let mut client = client();
    let hashes = step(&mut client, 5);
    assert_eq!(hashes.len(), 5);
    let Ok(Reply::Hash { tick, hash, .. }) = client.request(Command::GetHash) else {
        panic!()
    };
    assert_eq!((tick, hash), (5, hashes[4]));
    let Ok(Reply::Stepped { hashes, .. }) = client.request(Command::Step {
        ticks: 2,
        record_hashes: false,
    }) else {
        panic!()
    };
    assert!(hashes.is_empty());
}

#[test]
fn snapshot_then_restore_reproduces_hashes() {
    let mut client = client();
    step(&mut client, 3);
    let saved = snapshot(&mut client);
    let first = step(&mut client, 4);
    client
        .request(Command::Restore { snapshot: saved })
        .unwrap();
    assert_eq!(step(&mut client, 4), first);
}

#[test]
fn step_past_input_log_end_uses_defaults() {
    let mut with_log = client();
    with_log
        .request(Command::LoadInputs { log: log(&[1, 1]) })
        .unwrap();
    let mut explicit = client();
    explicit
        .request(Command::LoadInputs {
            log: log(&[1, 1, 0, 0]),
        })
        .unwrap();
    assert_eq!(step(&mut with_log, 4), step(&mut explicit, 4));
}

#[test]
fn load_inputs_for_other_type_fails_with_invalid_inputs_on_step() {
    let mut other = InputLog::new(1);
    other.record(&[1u64 << 40]);
    let mut client = client();
    client
        .request(Command::LoadInputs {
            log: Bytes(other.encode()),
        })
        .unwrap();
    let error = client
        .request(Command::Step {
            ticks: 1,
            record_hashes: false,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInputs);
}

#[test]
fn invalid_scene_returns_invalid_scene_error() {
    let error = client()
        .request(Command::LoadScene {
            ron: "nonsense".into(),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidScene);
}

#[test]
fn list_and_get_entity_return_components_and_reject_dead_ones() {
    let mut client = client();
    let Ok(Reply::Entities { total, rows }) = client.request(Command::QueryEntities {
        filter: String::new(),
        offset: 0,
        limit: 10,
    }) else {
        panic!()
    };
    assert_eq!((total, rows.len()), (1, 1));
    let Ok(Reply::Entity { components, .. }) = client.request(Command::GetEntity {
        entity: rows[0].entity,
    }) else {
        panic!()
    };
    assert_eq!(
        components["host::Score"],
        ComponentValue::Json {
            value: serde_json::json!(5)
        }
    );
    let dead: Entity = serde_json::from_str(r#"{"index":9,"generation":0}"#).unwrap();
    let error = client
        .request(Command::GetEntity { entity: dead })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NoSuchEntity);
}

#[test]
fn diff_snapshots_returns_lines() {
    let mut client = client();
    let before = snapshot(&mut client);
    step(&mut client, 1);
    let after = snapshot(&mut client);
    let Ok(Reply::Differences { lines }) = client.request(Command::DiffSnapshots {
        a: before,
        b: after,
    }) else {
        panic!()
    };
    assert!(lines.iter().any(|line| line.contains("Score")), "{lines:?}");
}

#[test]
fn every_command_and_reply_round_trips_through_json() {
    let entity: Entity = serde_json::from_str(r#"{"index":1,"generation":2}"#).unwrap();
    let bytes = Bytes(vec![0, 1, 254, 255]);
    let commands = vec![
        Command::LoadScene { ron: "x".into() },
        Command::LoadInputs { log: bytes.clone() },
        Command::SetThreads { threads: 4 },
        Command::Step {
            ticks: 3,
            record_hashes: true,
        },
        Command::GetHash,
        Command::GetHashTree,
        Command::TakeSnapshot,
        Command::Restore {
            snapshot: bytes.clone(),
        },
        Command::DiffSnapshots {
            a: bytes.clone(),
            b: bytes.clone(),
        },
        Command::QueryEntities {
            filter: "With<Score>".into(),
            offset: 2,
            limit: 3,
        },
        Command::GetEntity { entity },
        Command::GetResources,
        Command::Spawn {
            components: BTreeMap::from([("n".to_string(), serde_json::json!(1))]),
        },
        Command::Despawn { entity },
        Command::InsertComponent {
            entity,
            component: "n".into(),
            value: serde_json::json!({"x": 0.1}),
        },
        Command::RemoveComponent {
            entity,
            component: "n".into(),
        },
        Command::SetField {
            target: FieldTarget::Component {
                entity,
                component: "n".into(),
            },
            path: vec![
                PathSegment::Field { name: "a".into() },
                PathSegment::Index { index: 2 },
            ],
            value: serde_json::json!(null),
        },
        Command::SetField {
            target: FieldTarget::Resource {
                resource: "r".into(),
            },
            path: vec![],
            value: serde_json::json!("1"),
        },
        Command::SaveScene,
        Command::ApplyToScene {
            entity,
            component: "n".into(),
        },
        Command::LoadEdits { log: bytes.clone() },
        Command::Play,
        Command::Pause,
        Command::Seek { tick: 9 },
        Command::SetSpeed { speed: 1.5 },
        Command::SetLiveInput {
            players: vec![RawInput {
                keys: ["KeyA".to_string()].into(),
                gamepads: vec![RawGamepad {
                    buttons: vec![0.25],
                    axes: vec![-0.5],
                }],
            }],
        },
        Command::Undo,
        Command::Redo,
        Command::GetTimeline,
        Command::ExportSession,
        Command::ReplaceScene { ron: "y".into() },
        Command::LoadSession {
            scene: "z".into(),
            saved_scene: Some("w".into()),
            inputs: bytes.clone(),
            edits: bytes.clone(),
            undo: bytes.clone(),
            expected: vec![Hash64(1)],
        },
        Command::GetSchedule,
        Command::GetTickEvents { tick: 3 },
        Command::GetTickCommands { tick: 4 },
        Command::GetDebugDraw {
            layers: vec!["physics.colliders".into()],
        },
        Command::Pick {
            point: serde_json::from_str(r#"{"x":1.5,"y":-2.0}"#).unwrap(),
        },
        Command::Subscribe {
            topic: Topic::Entities {
                filter: "With<Score>".into(),
            },
        },
        Command::Subscribe {
            topic: Topic::Entity { entity },
        },
        Command::Subscribe {
            topic: Topic::Resources,
        },
        Command::Unsubscribe {
            topic: Topic::Introspection,
        },
    ];
    for command in commands {
        let message = ClientMessage::Request { id: 7, command };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&json).unwrap(),
            message
        );
    }
    let hello = ClientMessage::Hello {
        protocol_version: 2,
    };
    let json = serde_json::to_string(&hello).unwrap();
    assert_eq!(json, r#"{"type":"hello","protocol_version":2}"#);

    let tree = Host::new(app())
        .map(|mut host| host.handle(Command::GetHashTree))
        .unwrap();
    let Ok(Reply::HashTree { tree, .. }) = tree else {
        panic!()
    };
    let _: &HashTree = &tree;
    let timeline = Timeline {
        head: 9,
        simulated: 8,
        current: 3,
        checkpoints: vec![0],
        edit_ticks: vec![2],
        can_undo: true,
        can_redo: false,
        scene_dirty: true,
        resimulating: false,
    };
    let replies = vec![
        Reply::Done,
        Reply::Stepped {
            tick: 1,
            hashes: vec![Hash64(0xab)],
        },
        Reply::Hash {
            tick: 1,
            hash: Hash64(u64::MAX),
            pending: true,
        },
        Reply::HashTree { tick: 0, tree },
        Reply::Snapshot {
            tick: 2,
            snapshot: bytes.clone(),
        },
        Reply::Differences {
            lines: vec!["a".into()],
        },
        Reply::Entities {
            total: 4,
            rows: vec![EntityRow {
                entity,
                hash: Hash64(9),
            }],
        },
        Reply::Schedule {
            systems: vec![SystemRow {
                name: "s".into(),
                reads: vec!["component n".into()],
                writes: vec![],
                micros: Some(1.5),
            }],
        },
        Reply::TickEvents {
            events: vec![RecordedEvent {
                event_type: "e".into(),
                value: serde_json::json!({"k": "Begin"}),
            }],
        },
        Reply::TickCommands {
            sync_points: vec![SyncPoint {
                after_system: "s".into(),
                commands: vec!["spawn 1v2".into()],
            }],
        },
        Reply::DebugDraw {
            tick: 3,
            frame: Default::default(),
        },
        Reply::Picked {
            entities: vec![entity],
        },
        Reply::Entity {
            entity,
            components: BTreeMap::from([
                (
                    "n".to_string(),
                    ComponentValue::Json {
                        value: serde_json::json!({"a": [1.5, "18446744073709551615"]}),
                    },
                ),
                (
                    "r".to_string(),
                    ComponentValue::Ron {
                        text: "(1)".to_string(),
                    },
                ),
            ]),
        },
        Reply::Resources {
            resources: BTreeMap::new(),
        },
        Reply::Spawned { entity },
        Reply::Scene { ron: "s".into() },
        Reply::Timeline(timeline.clone()),
        Reply::Session {
            scene: "s".into(),
            saved_scene: None,
            inputs: bytes.clone(),
            edits: bytes.clone(),
            undo: bytes.clone(),
            hashes: vec![Hash64(3)],
        },
    ];
    for reply in replies {
        let messages = [
            ServerMessage::Response {
                id: 1,
                result: Ok(reply),
            },
            ServerMessage::Response {
                id: 2,
                result: Err(ProtocolError {
                    code: ErrorCode::NoSuchEntity,
                    message: "m".into(),
                }),
            },
            ServerMessage::Welcome {
                protocol_version: 2,
                engine_version: "0.1.0".into(),
                game: game(),
            },
        ];
        for message in messages {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<ServerMessage>(&json).unwrap(),
                message
            );
        }
    }
    let events = vec![
        Event::Ticked {
            tick: 4,
            hash: Hash64(5),
            pending: false,
            playing: true,
            effective_rate: 59.5,
        },
        Event::Paused {
            reason: PauseReason::User,
        },
        Event::Paused {
            reason: PauseReason::Panicked {
                tick: 50,
                message: "boom".into(),
            },
        },
        Event::Paused {
            reason: PauseReason::Failed {
                tick: 3,
                message: "bad input".into(),
            },
        },
        Event::TimelineChanged { timeline },
        Event::Resimulated { up_to: 7 },
        Event::Diverged {
            tick: 30,
            expected: Hash64(1),
            got: Hash64(2),
        },
        Event::EditDropped {
            tick: 3,
            reason: "r".into(),
        },
        Event::Console {
            level: ConsoleLevel::Warning,
            text: "t".into(),
        },
        Event::EntitiesChanged {
            filter: "f".into(),
            total: 3,
        },
        Event::EntityChanged {
            entity,
            components: BTreeMap::from([(
                "n".to_string(),
                ComponentValue::Json {
                    value: serde_json::json!(1),
                },
            )]),
            removed: vec!["r".into()],
        },
        Event::EntityDespawned { entity },
        Event::ResourcesChanged {
            resources: BTreeMap::new(),
        },
    ];
    for event in events {
        let message = ServerMessage::Event { event };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&json).unwrap(),
            message
        );
    }
    for code in every_error_code() {
        let message = ServerMessage::Response {
            id: 3,
            result: Err(ProtocolError {
                code,
                message: "m".into(),
            }),
        };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&json).unwrap(),
            message
        );
    }
    let json = serde_json::to_string(&Reply::Hash {
        tick: 1,
        hash: Hash64(0xab),
        pending: false,
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"type":"hash","tick":1,"hash":"00000000000000ab","pending":false}"#
    );
}

/// Every `ErrorCode`; the match fails to compile when a variant is added without listing it.
fn every_error_code() -> Vec<ErrorCode> {
    use ErrorCode::*;
    let all = vec![
        VersionMismatch,
        NotConnected,
        InvalidScene,
        InvalidInputs,
        InvalidSnapshot,
        NoSuchEntity,
        InvalidEdit,
        UnknownComponent,
        UnknownResource,
        InvalidPath,
        InvalidCommand,
        InvalidTick,
        Resimulating,
        NothingToUndo,
        NothingToRedo,
        NoInputMapping,
        SystemPanicked,
        InvalidFilter,
    ];
    for code in &all {
        match code {
            VersionMismatch | NotConnected | InvalidScene | InvalidInputs | InvalidSnapshot
            | NoSuchEntity | InvalidEdit | UnknownComponent | UnknownResource | InvalidPath
            | InvalidCommand | InvalidTick | Resimulating | NothingToUndo | NothingToRedo
            | NoInputMapping | SystemPanicked | InvalidFilter => {}
        }
    }
    all
}

fn game() -> GameInfo {
    GameInfo {
        components: vec![NamedSchema {
            name: "host::Score".into(),
            schema: Score::schema(),
        }],
        resources: Vec::new(),
        input: Some(TypeSchema::U8),
        tick_rate: 60,
        debug_layers: vec!["physics.colliders".into()],
    }
}

#[test]
fn host_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Host>();
    assert_send::<InProcess>();
}

#[test]
fn failing_step_names_the_tick_and_keeps_earlier_ticks() {
    let mut bad = InputLog::new(1);
    bad.record(&[1u8]);
    bad.ticks.push(vec![0xff, 0xff, 0xff]);
    let mut client = client();
    client
        .request(Command::LoadInputs {
            log: Bytes(bad.encode()),
        })
        .unwrap();
    let error = client
        .request(Command::Step {
            ticks: 3,
            record_hashes: false,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInputs);
    assert!(error.message.starts_with("tick 1: "), "{}", error.message);
    let Ok(Reply::Hash { tick, .. }) = client.request(Command::GetHash) else {
        panic!()
    };
    assert_eq!(tick, 1);
}

#[test]
fn hash64_rejects_uppercase_and_plus_sign() {
    for bad in ["\"00000000000000AB\"", "\"+00000000000000a\"", "\"ab\""] {
        assert!(serde_json::from_str::<Hash64>(bad).is_err(), "{bad}");
    }
}

#[test]
fn wire_format_matches_golden_json() {
    use serde_json::{Value, json};
    let to = |value: &dyn erased::Ser| value.to_value();
    let entity: Entity = serde_json::from_str(r#"{"index":1,"generation":2}"#).unwrap();

    assert_eq!(
        to(&ClientMessage::Request {
            id: 3,
            command: Command::Step {
                ticks: 5,
                record_hashes: true
            }
        }),
        json!({"type":"request","id":3,"command":{"type":"step","ticks":5,"record_hashes":true}})
    );
    assert_eq!(
        to(&Command::GetEntity { entity }),
        json!({"type":"get_entity","entity":{"index":1,"generation":2}})
    );
    assert_eq!(
        to(&Command::LoadInputs {
            log: Bytes(vec![0, 1, 254, 255])
        }),
        json!({"type":"load_inputs","log":"AAH+/w=="})
    );
    assert_eq!(
        to(&Reply::Entity {
            entity,
            components: BTreeMap::from([
                ("n".to_string(), ComponentValue::Json { value: json!(1) }),
                (
                    "r".to_string(),
                    ComponentValue::Ron {
                        text: "(1)".to_string()
                    }
                ),
            ])
        }),
        json!({"type":"entity","entity":{"index":1,"generation":2},"components":{
            "n":{"type":"json","value":1},
            "r":{"type":"ron","text":"(1)"},
        }})
    );
    assert_eq!(
        to(&Command::SetField {
            target: FieldTarget::Component {
                entity,
                component: "ludenea::Collider".into()
            },
            path: vec![
                PathSegment::Field {
                    name: "shape".into()
                },
                PathSegment::Index { index: 0 }
            ],
            value: json!(0.5),
        }),
        json!({"type":"set_field",
            "target":{"type":"component","entity":{"index":1,"generation":2},"component":"ludenea::Collider"},
            "path":[{"type":"field","name":"shape"},{"type":"index","index":0}],
            "value":0.5})
    );
    let error = ProtocolError {
        code: ErrorCode::NoSuchEntity,
        message: "m".into(),
    };
    assert_eq!(to(&error), json!({"code":"no_such_entity","message":"m"}));
    assert_eq!(
        to(&ServerMessage::Response {
            id: 1,
            result: Ok(Reply::Done)
        }),
        json!({"type":"response","id":1,"result":{"ok":{"type":"done"}}})
    );
    assert_eq!(
        to(&ServerMessage::Response {
            id: 2,
            result: Err(error.clone())
        }),
        json!({"type":"response","id":2,"result":{"error":{"code":"no_such_entity","message":"m"}}})
    );
    assert_eq!(
        to(&ServerMessage::Welcome {
            protocol_version: 2,
            engine_version: "0.1.0".into(),
            game: game()
        }),
        json!({"type":"welcome","protocol_version":2,"engine_version":"0.1.0","game":{
            "components":[{"name":"host::Score","schema":to(&Score::schema())}],
            "resources":[],"input":{"kind":"u8"},"tick_rate":60,
            "debug_layers":["physics.colliders"]}})
    );
    assert_eq!(
        to(&ServerMessage::Event {
            event: Event::Paused {
                reason: PauseReason::Panicked {
                    tick: 50,
                    message: "m".into()
                }
            }
        }),
        json!({"type":"event","event":{"type":"paused","reason":{"type":"panicked","tick":50,"message":"m"}}})
    );
    assert_eq!(
        to(&Command::SetSpeed { speed: 2.0 }),
        json!({"type":"set_speed","speed":2.0})
    );
    assert_eq!(
        to(&ServerMessage::Refused { error }),
        json!({"type":"refused","error":{"code":"no_such_entity","message":"m"}})
    );
    let big = u64::MAX - 1;
    let tree = HashTree {
        root: big,
        resources: vec![NamedHash {
            name: "r".into(),
            hash: 1,
        }],
        entities: vec![EntityHash {
            entity,
            hash: big,
            components: vec![NamedHash {
                name: "c".into(),
                hash: big,
            }],
        }],
    };
    let value: Value = to(&Reply::HashTree { tick: 0, tree });
    assert_eq!(
        value,
        json!({"type":"hash_tree","tick":0,"tree":{
            "root":"fffffffffffffffe",
            "resources":[{"name":"r","hash":"0000000000000001"}],
            "entities":[{"entity":{"index":1,"generation":2},"hash":"fffffffffffffffe",
                "components":[{"name":"c","hash":"fffffffffffffffe"}]}]}})
    );
}

mod erased {
    pub trait Ser {
        fn to_value(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn to_value(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap()
        }
    }
}

#[test]
fn step_hashes_exclude_the_edits_get_hash_includes() {
    let mut client = client();
    let entity = match client.request(Command::QueryEntities {
        filter: String::new(),
        offset: 0,
        limit: 1,
    }) {
        Ok(Reply::Entities { rows, .. }) => rows[0].entity,
        other => panic!("{other:?}"),
    };
    let insert = |tick, score: u32| ludenea_app::TimedEdit {
        tick,
        edit: ludenea_app::Edit::Insert {
            entity,
            component: "host::Score".to_string(),
            value: postcard::to_allocvec(&Score(score)).unwrap(),
        },
    };
    let mut edits = ludenea_app::EditLog::new();
    edits.edits = vec![insert(0, 100), insert(2, 7)];
    client
        .request(Command::LoadEdits {
            log: Bytes(edits.encode()),
        })
        .unwrap();
    while client
        .request(Command::GetTimeline)
        .map(|reply| match reply {
            Reply::Timeline(timeline) => timeline.resimulating,
            other => panic!("{other:?}"),
        })
        == Ok(true)
    {
        client.advance(0.0);
    }
    let get_hash = |client: &mut InProcess| match client.request(Command::GetHash) {
        Ok(Reply::Hash { hash, pending, .. }) => (hash, pending),
        other => panic!("{other:?}"),
    };
    let recorded = match client.request(Command::ExportSession) {
        Ok(Reply::Session { hashes, .. }) => hashes,
        other => panic!("{other:?}"),
    };
    let (at_zero, pending) = get_hash(&mut client);
    assert!(pending);
    assert_ne!(at_zero, recorded[0]);
    let stepped = step(&mut client, 2);
    assert_eq!(stepped, recorded[1..=2]);
    let (at_two, pending) = get_hash(&mut client);
    assert!(pending);
    assert_ne!(at_two, stepped[1]);
}

#[test]
fn a_failing_play_tick_pauses_with_failed() {
    let mut bad = InputLog::new(1);
    bad.record(&[1u8]);
    bad.ticks.push(vec![0xff, 0xff, 0xff]);
    let mut client = client();
    client
        .request(Command::LoadInputs {
            log: Bytes(bad.encode()),
        })
        .unwrap();
    client.request(Command::Play).unwrap();
    let events = client.advance(1.0);
    let paused: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Paused { reason } => Some(reason),
            _ => None,
        })
        .collect();
    let [PauseReason::Failed { tick: 1, message }] = paused.as_slice() else {
        panic!("{events:?}");
    };
    assert!(message.contains("input"), "{message}");
}

#[test]
fn drain_events_returns_what_commands_queued() {
    let mut client = client();
    client
        .request(Command::Step {
            ticks: 2,
            record_hashes: false,
        })
        .unwrap();
    assert!(!client.drain_events().is_empty());
    assert!(client.drain_events().is_empty());
}
