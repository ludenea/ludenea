use std::collections::BTreeMap;

use ludenea_app::{App, InputLog, Inputs, Rng, Sim};
use ludenea_control::*;
use ludenea_ecs::{Component, Entity, HashTree, Query, ResMut, Runtime};
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

const SCENE: &str =
    r#"Scene(version: 1, resources: {}, entities: [{ "host::Score": (5) }])"#;

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
fn handshake_with_wrong_version_is_refused() {
    let error = InProcess::connect(Host::new(app()).unwrap(), 99)
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::VersionMismatch);
    assert_eq!(error.message, "client speaks protocol 99, engine speaks 1");
}

#[test]
fn step_with_record_hashes_returns_one_hash_per_tick() {
    let mut client = client();
    let hashes = step(&mut client, 5);
    assert_eq!(hashes.len(), 5);
    let Ok(Reply::Hash { tick, hash }) = client.request(Command::GetHash) else {
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
    let Ok(Reply::Entities { entities }) = client.request(Command::ListEntities) else {
        panic!()
    };
    assert_eq!(entities.len(), 1);
    let Ok(Reply::Entity { components, .. }) = client.request(Command::GetEntity {
        entity: entities[0],
    }) else {
        panic!()
    };
    assert_eq!(components["host::Score"], "(5)");
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
        Command::ListEntities,
        Command::GetEntity { entity },
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
        protocol_version: 1,
    };
    let json = serde_json::to_string(&hello).unwrap();
    assert_eq!(json, r#"{"type":"hello","protocol_version":1}"#);

    let tree = Host::new(app())
        .map(|mut host| host.handle(Command::GetHashTree))
        .unwrap();
    let Ok(Reply::HashTree { tree, .. }) = tree else {
        panic!()
    };
    let _: &HashTree = &tree;
    let replies = vec![
        Reply::Done,
        Reply::Stepped {
            tick: 1,
            hashes: vec![Hash64(0xab)],
        },
        Reply::Hash {
            tick: 1,
            hash: Hash64(u64::MAX),
        },
        Reply::HashTree { tick: 0, tree },
        Reply::Snapshot {
            tick: 2,
            snapshot: bytes,
        },
        Reply::Differences {
            lines: vec!["a".into()],
        },
        Reply::Entities {
            entities: vec![entity],
        },
        Reply::Entity {
            entity,
            components: BTreeMap::from([("n".to_string(), "(1)".to_string())]),
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
                protocol_version: 1,
                engine_version: "0.1.0".into(),
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
    let json = serde_json::to_string(&Reply::Hash {
        tick: 1,
        hash: Hash64(0xab),
    })
    .unwrap();
    assert_eq!(
        json,
        r#"{"type":"hash","tick":1,"hash":"00000000000000ab"}"#
    );
}

#[test]
fn host_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Host>();
    assert_send::<InProcess>();
}
