use std::fs;
use std::path::Path;

use boxes::SpawnerStats;
use ludenea::cli::assets::project_assets;
use ludenea::control::{
    BranchId, Command, ComponentValue, Document, ErrorCode, Host, InProcess, PROTOCOL_VERSION,
    PathSegment, ProtocolError, Reply, Timeline,
};
use ludenea::ecs::{Asset, AssetId};
use ludenea::prelude::{App, Edit, EditLog};
use serde::{Deserialize, Serialize};
use serde_json::json;

const SPAWNER_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/spawner.scene.ron"
);
/// The identifiers in `crate.prefab.ron.meta` and `spawner.ron.meta`.
const PREFAB: AssetId = AssetId(0x38b3_6be4_f8a1_40d6_95e5_cdac_24c8_2f90);
const STATS: AssetId = AssetId(0xbef6_17c9_e94d_41c4_b0e0_7882_849c_ef18);

#[derive(Asset, Serialize, Deserialize, Clone, Default)]
struct Tuning {
    speed: u32,
}

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
}

fn assets() -> Vec<(AssetId, String)> {
    project_assets(Path::new(SPAWNER_SCENE), None)
        .unwrap()
        .texts
        .into_iter()
        .collect()
}

/// A boxes host with the spawner scene loaded with its assets, and its reply.
fn spawner_host(scene: String) -> (Host, Reply) {
    let mut host = Host::new(boxes::app()).unwrap();
    let reply = ok(
        &mut host,
        Command::LoadScene {
            scene,
            assets: assets(),
            document: Document::Scene,
        },
    );
    (host, reply)
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

#[test]
fn handshake_refuses_version_2() {
    assert_eq!(PROTOCOL_VERSION, 3);
    let Err(error) = InProcess::connect(Host::new(boxes::app()).unwrap(), 2) else {
        panic!("a version 2 client connects");
    };
    assert_eq!(
        error,
        ProtocolError {
            code: ErrorCode::VersionMismatch,
            message: "client speaks protocol 2, engine speaks 3".to_string(),
        }
    );
}

#[test]
fn game_info_lists_assets_events_and_fingerprint() {
    let host = Host::new(boxes::app()).unwrap();
    let game = host.game_info();
    let names = |schemas: &[ludenea::control::NamedSchema]| -> Vec<String> {
        schemas.iter().map(|named| named.name.clone()).collect()
    };
    assert!(names(&game.assets).contains(&SpawnerStats::NAME.to_string()));
    assert!(names(&game.events).contains(&"ludenea::Collision".to_string()));
    let sim = boxes::app().build().unwrap();
    assert_eq!(game.fingerprint.0, sim.schema_fingerprint());
}

#[test]
fn load_scene_with_assets_reports_instances_and_warnings() {
    // The override of instance 6 targets an entity the prefab does not have.
    let scene = fs::read_to_string(SPAWNER_SCENE)
        .unwrap()
        .replace("(entity: [0], component", "(entity: [9], component");
    let (mut host, reply) = spawner_host(scene.clone());
    let Reply::Loaded {
        warnings,
        instances,
        load_addresses,
    } = reply
    else {
        panic!("{reply:?}");
    };
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains('9'), "{warnings:?}");
    assert_eq!(
        instances
            .iter()
            .map(|instance| (instance.address.clone(), instance.prefab))
            .collect::<Vec<_>>(),
        [(vec![5], PREFAB), (vec![6], PREFAB)]
    );
    let addresses: Vec<Vec<u32>> = load_addresses.iter().map(|(_, a)| a.clone()).collect();
    for address in [vec![0], vec![4], vec![5, 0], vec![6, 0]] {
        assert!(addresses.contains(&address), "{addresses:?}");
    }

    let Reply::Loaded { warnings, .. } = ok(
        &mut host,
        Command::ReplaceScene {
            scene,
            assets: assets(),
        },
    ) else {
        panic!("replace_scene replies loaded");
    };
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn set_asset_field_records_set_asset_past_tick_0() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    step(&mut host, 10);
    ok(
        &mut host,
        Command::SetAssetField {
            id: STATS,
            path: vec![PathSegment::Field {
                name: "every_ticks".to_string(),
            }],
            value: json!(30),
        },
    );
    let Reply::Asset { asset, value } = ok(&mut host, Command::GetAsset { id: STATS }) else {
        panic!("get_asset replies asset");
    };
    assert_eq!(asset, SpawnerStats::NAME);
    let ComponentValue::Json { value } = value else {
        panic!("{value:?}");
    };
    assert_eq!(value["every_ticks"], json!(30));

    let Reply::Session { edits, .. } = ok(&mut host, Command::ExportSession) else {
        panic!("export_session replies session");
    };
    let log = EditLog::decode(&edits.0).unwrap();
    assert!(
        matches!(
            &log.edits[..],
            [edit] if edit.tick == 10 && matches!(edit.edit, Edit::SetAsset { id, .. } if id == STATS)
        ),
        "{:?}",
        log.edits
    );
    // The asset text keeps its saved value: the edit lives in the timeline.
    let Reply::Text { text } = ok(&mut host, Command::SaveAsset { id: STATS }) else {
        panic!("save_asset replies text");
    };
    assert!(text.contains("every_ticks: 60"), "{text}");
}

#[test]
fn new_asset_value_uses_default() {
    let mut app = App::new();
    app.asset_with_default::<Tuning>();
    let mut host = Host::new(app).unwrap();
    let reply = ok(
        &mut host,
        Command::NewAssetValue {
            asset: Tuning::NAME.to_string(),
        },
    );
    assert_eq!(
        reply,
        Reply::Text {
            text: "(speed:0)".to_string()
        }
    );
}

#[test]
fn new_asset_value_without_default_is_an_error() {
    let mut host = Host::new(boxes::app()).unwrap();
    let error = host
        .handle(Command::NewAssetValue {
            asset: SpawnerStats::NAME.to_string(),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert!(error.message.contains(SpawnerStats::NAME), "{error:?}");
}

#[test]
fn switch_rename_and_delete_branch_over_protocol() {
    let mut host = Host::new(boxes::app()).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(30, 7),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    step(&mut host, 20);
    ok(&mut host, Command::Seek { tick: 10 });
    ok(
        &mut host,
        Command::Spawn {
            components: [("boxes::Crate".to_string(), json!(null))].into(),
        },
    );
    let forked = timeline(&mut host);
    assert_eq!(forked.branches.len(), 2);
    let child = forked.branch;
    assert_ne!(child, BranchId::ROOT);

    ok(
        &mut host,
        Command::RenameBranch {
            branch: child,
            name: "Spawned".to_string(),
        },
    );
    ok(
        &mut host,
        Command::SwitchBranch {
            branch: BranchId::ROOT,
        },
    );
    let switched = timeline(&mut host);
    assert_eq!(switched.branch, BranchId::ROOT);
    let renamed = switched.branches.iter().find(|branch| branch.id == child);
    assert_eq!(renamed.unwrap().name, "Spawned");

    assert_eq!(
        ok(&mut host, Command::DeleteBranch { branch: child }),
        Reply::Deleted {
            branches: vec![child]
        }
    );
    assert_eq!(timeline(&mut host).branches.len(), 1);
    let error = host
        .handle(Command::DeleteBranch {
            branch: BranchId::ROOT,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidCommand);
}

#[test]
fn session_export_from_2a_loads() {
    let fixture = include_str!("fixtures/session-2a.json");
    let Reply::Session {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        hashes,
        assets,
        branches,
        document,
    } = serde_json::from_str(fixture).unwrap()
    else {
        panic!("the fixture is a session export");
    };
    assert!(assets.is_empty() && branches.0.is_empty());
    assert_eq!(document, Document::Scene);
    let mut host = Host::new(boxes::app()).unwrap();
    ok(
        &mut host,
        Command::LoadSession {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            expected: hashes,
            assets,
            branches,
            document,
        },
    );
    while timeline(&mut host).resimulating {
        host.advance(0.0);
    }
    let loaded = timeline(&mut host);
    assert_eq!(loaded.branch, BranchId::ROOT);
    assert_eq!(loaded.branches.len(), 1);
    assert!(loaded.head > 0);
}
