use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use boxes::SpawnerStats;
use ludenea::app::prefab::InstanceInfo;
use ludenea::app::replay::{Replay, ReplayOutcome, play_replay};
use ludenea::app::trace::{TRACE_VERSION, Trace, TraceComparison};
use ludenea::app::{EntityRef, RawInput};
use ludenea::cli::assets::project_assets;
use ludenea::control::{
    BranchId, Bytes, Command, ComponentValue, Document, ErrorCode, Event, FieldTarget, Hash64,
    Host, InProcess, PROTOCOL_VERSION, PathSegment, ProtocolError, Reply, Timeline, Topic,
};
use ludenea::ecs::diff::Difference;
use ludenea::ecs::{Asset, AssetId, Entity, Snapshot};
use ludenea::prelude::{
    App, Component, Edit, EditLog, InputLog, Query, Res, ResMut, Rng, Sim, Time, TimedEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod support;

const SPAWNER_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/spawner.scene.ron"
);
const CRATE_PREFAB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/crate.prefab.ron"
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
    let mut host = Host::new(boxes::app).unwrap();
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
    assert_eq!(PROTOCOL_VERSION, 4);
    let Err(error) = InProcess::connect(Host::new(boxes::app).unwrap(), 2) else {
        panic!("a version 2 client connects");
    };
    assert_eq!(
        error,
        ProtocolError {
            code: ErrorCode::VersionMismatch,
            message: "client speaks protocol 2, engine speaks 4; rebuild the editor (pnpm -C \
                      editor build) and the game from the same checkout"
                .to_string(),
        }
    );
}

/// The `type` a command or a reply travels with.
fn type_of(value: &impl Serialize) -> String {
    let value = serde_json::to_value(value).unwrap();
    match value["type"].as_str() {
        Some(name) => name.to_owned(),
        None => panic!("{value} travels without a type"),
    }
}

/// A host whose commands must succeed with the reply `Command::reply_type` names.
struct Checked {
    host: Host,
    ran: BTreeSet<String>,
}

impl Checked {
    fn run(&mut self, command: Command) -> Reply {
        let name = type_of(&command);
        let expected = command.reply_type();
        let reply = self
            .host
            .handle(command)
            .unwrap_or_else(|error| panic!("{name} failed: {error:?}"));
        assert_eq!(type_of(&reply), expected, "the reply to {name}");
        self.ran.insert(name);
        reply
    }
}

#[test]
fn every_command_names_the_reply_it_gets() {
    let scene = fs::read_to_string(SPAWNER_SCENE).unwrap();
    let mut checked = Checked {
        host: Host::new(boxes::app).unwrap(),
        ran: BTreeSet::new(),
    };
    let Reply::Loaded { load_addresses, .. } = checked.run(Command::LoadScene {
        scene: scene.clone(),
        assets: assets(),
        document: Document::Scene,
    }) else {
        unreachable!("checked by run");
    };
    let loaded = |id: u32| {
        load_addresses
            .iter()
            .find(|(_, address)| address == &[id])
            .unwrap()
            .0
    };
    let ground = loaded(0);
    checked.run(Command::SetThreads { threads: 1 });
    checked.run(Command::LoadInputs {
        log: Bytes(InputLog::new(1).encode()),
    });
    checked.run(Command::LoadEdits {
        log: Bytes(EditLog::new().encode()),
    });

    // Tick 0: scene, prefab and asset edits.
    checked.run(Command::GetHash);
    checked.run(Command::GetHashTree);
    let Reply::Snapshot {
        snapshot: start, ..
    } = checked.run(Command::TakeSnapshot)
    else {
        unreachable!("checked by run");
    };
    let Reply::Entity { components, .. } = checked.run(Command::GetEntity { entity: ground })
    else {
        unreachable!("checked by run");
    };
    let Some(ComponentValue::Json { value: pose }) = components.get("ludenea::Pose").cloned()
    else {
        panic!("{components:?}");
    };
    checked.run(Command::GetResources);
    checked.run(Command::QueryEntities {
        filter: "With<boxes::Crate>".to_string(),
        offset: 0,
        limit: 10,
    });
    checked.run(Command::CreatePrefab {
        entities: vec![ground],
    });
    checked.run(Command::PlaceInstance { prefab: PREFAB });
    let pose_y = vec![field("position"), field("y")];
    checked.run(Command::ApplyOverride {
        instance: vec![5],
        entity: vec![0],
        component: "ludenea::Pose".to_string(),
        path: pose_y.clone(),
        value: ComponentValue::Json { value: json!(3.0) },
    });
    checked.run(Command::RevertOverride {
        instance: vec![5],
        entity: vec![0],
        component: "ludenea::Pose".to_string(),
        path: pose_y.clone(),
    });
    checked.run(Command::ApplyToPrefab {
        instance: vec![6],
        entity: vec![0],
        component: "ludenea::Pose".to_string(),
        path: vec![field("position"), field("x")],
    });
    checked.run(Command::GetInstances);
    checked.run(Command::ReplaceWithInstance {
        entities: vec![loaded(1)],
        prefab: AssetId(7),
        prefab_text: fs::read_to_string(CRATE_PREFAB).unwrap(),
    });
    checked.run(Command::GetAsset { id: STATS });
    checked.run(Command::SetAssetField {
        id: STATS,
        path: vec![field("every_ticks")],
        value: json!(30),
    });
    checked.run(Command::SaveAsset { id: STATS });
    checked.run(Command::ApplyToAsset { id: STATS });
    checked.run(Command::SaveScene);

    // Past tick 0: recorded edits, snapshots and traces.
    let Reply::Stepped { .. } = checked.run(Command::Step {
        ticks: 20,
        record_hashes: true,
    }) else {
        unreachable!("checked by run");
    };
    checked.run(Command::DiffSnapshots { a: start, b: None });
    let Reply::Snapshot { snapshot, .. } = checked.run(Command::TakeSnapshot) else {
        unreachable!("checked by run");
    };
    checked.run(Command::Restore { snapshot });
    let Reply::Session {
        scene: session_scene,
        saved_scene,
        inputs,
        edits,
        undo,
        hashes,
        assets: session_assets,
        branches,
        document,
        keeps_assets,
    } = checked.run(Command::ExportSession)
    else {
        unreachable!("checked by run");
    };
    let trace = Trace {
        version: TRACE_VERSION,
        scene_hash: 0,
        inputs_hash: 0,
        hashes: hashes.iter().map(|hash| hash.0).collect(),
    };
    checked.run(Command::CompareTraces {
        a: Bytes(trace.encode()),
        b: None,
    });
    let Reply::Spawned { entity } = checked.run(Command::Spawn {
        components: [("boxes::Crate".to_string(), json!(null))].into(),
    }) else {
        unreachable!("checked by run");
    };
    checked.run(Command::InsertComponent {
        entity,
        component: "ludenea::Pose".to_string(),
        value: pose,
    });
    checked.run(Command::SetField {
        target: FieldTarget::Component {
            entity,
            component: "ludenea::Pose".to_string(),
        },
        path: pose_y,
        value: json!(5.0),
    });
    checked.run(Command::RemoveComponent {
        entity,
        component: "ludenea::Pose".to_string(),
    });
    checked.run(Command::Despawn { entity });
    checked.run(Command::Undo);
    checked.run(Command::Redo);
    checked.run(Command::ApplyToScene {
        entity: ground,
        component: "ludenea::Pose".to_string(),
    });
    settle(&mut checked.host);

    // Branches, and what compares them.
    checked.run(Command::Seek { tick: 10 });
    checked.run(Command::Spawn {
        components: [("boxes::Crate".to_string(), json!(null))].into(),
    });
    let Reply::Timeline(forked) = checked.run(Command::GetTimeline) else {
        unreachable!("checked by run");
    };
    let child = forked.branch;
    assert_ne!(child, BranchId::ROOT);
    checked.run(Command::RenameBranch {
        branch: child,
        name: "spawned".to_string(),
    });
    checked.run(Command::SwitchBranch {
        branch: BranchId::ROOT,
    });
    checked.run(Command::SetCompareBranch {
        branch: Some(child),
    });
    checked.run(Command::GetGhostDraw {
        tick: 10,
        layers: Vec::new(),
    });
    checked.run(Command::GetDivergence { up_to: None });
    checked.run(Command::DeleteBranch { branch: child });
    let Reply::Replay { replay } = checked.run(Command::ExportReplay { branch: None }) else {
        unreachable!("checked by run");
    };

    // Introspection and drawing.
    checked.run(Command::Subscribe {
        topic: Topic::Introspection,
    });
    let Reply::Stepped { tick, .. } = checked.run(Command::Step {
        ticks: 1,
        record_hashes: false,
    }) else {
        unreachable!("checked by run");
    };
    checked.run(Command::GetTickEvents { tick });
    checked.run(Command::GetTickCommands { tick });
    checked.run(Command::Unsubscribe {
        topic: Topic::Introspection,
    });
    checked.run(Command::GetSchedule);
    checked.run(Command::GetDebugDraw { layers: Vec::new() });
    checked.run(Command::Pick {
        point: serde_json::from_value(json!({"x": 0.0, "y": 0.0})).unwrap(),
    });

    // Play.
    checked.run(Command::Play);
    checked.run(Command::SetSpeed { speed: 2.0 });
    checked.run(Command::SetLiveInput {
        players: vec![RawInput::default()],
    });
    checked.run(Command::Pause);

    // Whole sessions.
    checked.run(Command::ReplaceScene {
        scene,
        assets: assets(),
    });
    checked.run(Command::LoadSession {
        scene: session_scene,
        saved_scene,
        inputs,
        edits,
        undo,
        expected: hashes,
        assets: session_assets,
        branches,
        document,
        keeps_assets,
    });
    settle(&mut checked.host);
    checked.run(Command::LoadReplay { replay });

    fn tuning_app() -> App {
        let mut app = App::new();
        app.asset_with_default::<Tuning>();
        app
    }
    checked.host = Host::new(tuning_app).unwrap();
    checked.run(Command::NewAssetValue {
        asset: Tuning::NAME.to_string(),
    });

    let every_type: BTreeSet<String> = support::every_command().iter().map(type_of).collect();
    assert_eq!(checked.ran, every_type, "the commands this test runs");
}

#[test]
fn game_info_lists_assets_events_and_fingerprint() {
    let host = Host::new(boxes::app).unwrap();
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
    fn app() -> App {
        let mut app = App::new();
        app.asset_with_default::<Tuning>();
        app
    }
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
    let mut host = Host::new(boxes::app).unwrap();
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
    let mut host = Host::new(boxes::app).unwrap();
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
        ..
    } = serde_json::from_str(fixture).unwrap()
    else {
        panic!("the fixture is a session export");
    };
    assert!(assets.is_empty() && branches.0.is_empty());
    assert_eq!(document, Document::Scene);
    let mut host = Host::new(boxes::app).unwrap();
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
            keeps_assets: false,
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

fn spawn_crate(host: &mut Host) {
    ok(
        host,
        Command::Spawn {
            components: [("boxes::Crate".to_string(), json!(null))].into(),
        },
    );
}

fn hash(host: &mut Host) -> Hash64 {
    match ok(host, Command::GetHash) {
        Reply::Hash { hash, .. } => hash,
        other => panic!("{other:?}"),
    }
}

/// The branches without their stale flag, which a resume sets on the other branches.
fn branches(timeline: &Timeline) -> Vec<(BranchId, String, Option<BranchId>, u64, u64)> {
    timeline
        .branches
        .iter()
        .map(|b| (b.id, b.name.clone(), b.parent, b.fork_tick, b.head))
        .collect()
}

#[test]
fn export_and_load_session_round_trips_assets_branches_and_document() {
    let mut host = Host::new(boxes::app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: fs::read_to_string(CRATE_PREFAB).unwrap(),
            assets: assets(),
            document: Document::Prefab,
        },
    );
    step(&mut host, 5);
    spawn_crate(&mut host);
    step(&mut host, 15);
    ok(&mut host, Command::Seek { tick: 10 });
    spawn_crate(&mut host);
    let b = timeline(&mut host).branch;
    // Below B's fork tick: C inherits the root's edit at tick 5.
    ok(&mut host, Command::Seek { tick: 5 });
    spawn_crate(&mut host);
    step(&mut host, 3);
    ok(
        &mut host,
        Command::RenameBranch {
            branch: b,
            name: "B".to_string(),
        },
    );
    let before = timeline(&mut host);
    let c = before.branch;
    assert_eq!(before.branches.len(), 3);
    let fork = before
        .branches
        .iter()
        .find(|branch| branch.id == c)
        .unwrap();
    assert_eq!((fork.parent, fork.fork_tick), (Some(b), 5));
    let expected = hash(&mut host);
    let Reply::Session {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        hashes,
        assets,
        branches: tree,
        document,
        ..
    } = ok(&mut host, Command::ExportSession)
    else {
        panic!("export_session replies session");
    };
    assert_eq!(document, Document::Prefab);
    assert!(!assets.is_empty() && !tree.0.is_empty());

    let mut loaded = Host::new(boxes::app).unwrap();
    ok(
        &mut loaded,
        Command::LoadSession {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            expected: hashes.clone(),
            assets: assets.clone(),
            branches: tree,
            document,
            keeps_assets: false,
        },
    );
    while timeline(&mut loaded).resimulating {
        loaded.advance(0.0);
    }
    ok(
        &mut loaded,
        Command::Seek {
            tick: before.current,
        },
    );
    let after = timeline(&mut loaded);
    assert_eq!(after.branch, c);
    assert_eq!(after.current, before.current);
    assert_eq!(branches(&after), branches(&before));
    assert_eq!(hash(&mut loaded), expected);
    let Reply::Session {
        hashes: reloaded,
        assets: reloaded_assets,
        document,
        ..
    } = ok(&mut loaded, Command::ExportSession)
    else {
        panic!("export_session replies session");
    };
    assert_eq!(reloaded, hashes);
    assert_eq!(reloaded_assets, assets);
    assert_eq!(document, Document::Prefab);

    // Deleting B deletes C, its descendant, too.
    assert_eq!(
        ok(&mut loaded, Command::DeleteBranch { branch: b }),
        Reply::Deleted {
            branches: vec![b, c]
        }
    );
    assert_eq!(timeline(&mut loaded).branch, BranchId::ROOT);
}

#[derive(Serialize)]
struct HostileTree {
    current: BranchId,
    next: u32,
    branches: Vec<HostileBranch>,
}

/// The stored branch layout of a session export.
#[derive(Serialize)]
struct HostileBranch {
    id: BranchId,
    name: String,
    parent: Option<BranchId>,
    fork_tick: u64,
    fork_edits: u64,
    stale: bool,
    inputs: Vec<u8>,
    edits: Vec<u8>,
    hashes: Vec<u64>,
}

#[test]
fn load_session_refuses_a_hostile_branch_tree() {
    let branch = |id, parent, fork_tick| HostileBranch {
        id: BranchId(id),
        name: String::new(),
        parent,
        fork_tick,
        fork_edits: 0,
        stale: false,
        inputs: InputLog::new(1).encode(),
        edits: EditLog::new().encode(),
        hashes: Vec::new(),
    };
    let far_fork = postcard::to_allocvec(&HostileTree {
        current: BranchId::ROOT,
        next: 2,
        branches: vec![
            branch(0, None, 0),
            branch(1, Some(BranchId::ROOT), u64::MAX),
        ],
    })
    .unwrap();
    for tree in [far_fork, vec![0xff; 16]] {
        let mut host = Host::new(boxes::app).unwrap();
        let error = host
            .handle(Command::LoadSession {
                scene: boxes::generate_scene(30, 7),
                saved_scene: None,
                inputs: Bytes(Vec::new()),
                edits: Bytes(Vec::new()),
                undo: Bytes(Vec::new()),
                expected: Vec::new(),
                assets: Vec::new(),
                branches: Bytes(tree),
                document: Document::Scene,
                keeps_assets: false,
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidCommand);
        assert!(
            error
                .message
                .starts_with("the branch tree cannot be decoded"),
            "{error:?}"
        );
    }
}

#[test]
fn load_session_refuses_a_stored_edit_past_any_run() {
    // A stored edit at the last tick lets a grandchild fork there; its inputs would end
    // past the last tick.
    let edits = EditLog {
        edits: vec![TimedEdit {
            tick: u64::MAX,
            edit: Edit::Despawn {
                entity: EntityRef::Loaded(vec![0]),
            },
        }],
        ..EditLog::new()
    };
    let mut inputs = InputLog::new(1);
    inputs.ticks.push(vec![0]);
    let branch = |id, parent, fork_tick, inputs: &InputLog, edits: &EditLog| HostileBranch {
        id: BranchId(id),
        name: String::new(),
        parent,
        fork_tick,
        fork_edits: 0,
        stale: false,
        inputs: inputs.encode(),
        edits: edits.encode(),
        hashes: Vec::new(),
    };
    let empty = (InputLog::new(1), EditLog::new());
    let tree = postcard::to_allocvec(&HostileTree {
        current: BranchId::ROOT,
        next: 3,
        branches: vec![
            branch(0, None, 0, &empty.0, &empty.1),
            branch(1, Some(BranchId(0)), 0, &empty.0, &edits),
            branch(2, Some(BranchId(1)), u64::MAX, &inputs, &empty.1),
        ],
    })
    .unwrap();
    let mut host = Host::new(boxes::app).unwrap();
    let error = host
        .handle(Command::LoadSession {
            scene: boxes::generate_scene(30, 7),
            saved_scene: None,
            inputs: Bytes(Vec::new()),
            edits: Bytes(Vec::new()),
            undo: Bytes(Vec::new()),
            expected: Vec::new(),
            assets: Vec::new(),
            branches: Bytes(tree),
            document: Document::Scene,
            keeps_assets: false,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert!(
        error
            .message
            .starts_with("the branch tree cannot be decoded"),
        "{error:?}"
    );
}

fn text(reply: Reply) -> String {
    match reply {
        Reply::Text { text } => text,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_refused_load_session_keeps_the_session() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    step(&mut host, 10);
    spawn_crate(&mut host);
    step(&mut host, 5);
    let before = (
        ok(&mut host, Command::ExportSession),
        timeline(&mut host),
        hash(&mut host),
    );
    let Reply::Session {
        scene,
        inputs,
        edits,
        hashes,
        ..
    } = before.0.clone()
    else {
        panic!("export_session replies session");
    };
    let load =
        |scene: String, assets: Vec<(AssetId, String)>, branches: Vec<u8>| Command::LoadSession {
            scene,
            saved_scene: None,
            inputs: inputs.clone(),
            edits: edits.clone(),
            undo: Bytes(Vec::new()),
            expected: hashes.clone(),
            assets,
            branches: Bytes(branches),
            document: Document::Scene,
            keeps_assets: false,
        };
    let refused = [
        // A scene that does not parse, one whose prefab is missing, and a broken branch tree.
        load("(not a scene".to_string(), assets(), Vec::new()),
        load(scene.clone(), Vec::new(), Vec::new()),
        load(scene, assets(), vec![0xff; 16]),
    ];
    for command in refused {
        host.handle(command).unwrap_err();
        let after = (
            ok(&mut host, Command::ExportSession),
            timeline(&mut host),
            hash(&mut host),
        );
        assert_eq!(after, before);
    }
    // The kept session goes on.
    step(&mut host, 5);
    assert_eq!(timeline(&mut host).current, before.1.current + 5);
}

#[test]
fn load_session_replies_what_the_scene_load_reports() {
    let (mut host, loaded) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
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
        keeps_assets,
    } = ok(&mut host, Command::ExportSession)
    else {
        panic!("export_session replies session");
    };
    let mut resumed = Host::new(boxes::app).unwrap();
    let reply = ok(
        &mut resumed,
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
            keeps_assets,
        },
    );
    assert_eq!(reply, loaded);
}

fn instances(host: &mut Host) -> Vec<InstanceInfo> {
    match ok(host, Command::GetInstances) {
        Reply::Instances { instances, .. } => instances,
        other => panic!("{other:?}"),
    }
}

/// The entity of `instance` that its prefab gives the local identifier 0.
fn instance_entity(host: &mut Host, instance: u32) -> Entity {
    instances(host)
        .into_iter()
        .find(|info| info.address == [instance])
        .unwrap()
        .entities[0]
}

fn pose_position(host: &mut Host, entity: Entity) -> serde_json::Value {
    let Reply::Entity { components, .. } = ok(host, Command::GetEntity { entity }) else {
        panic!("get_entity replies entity");
    };
    let Some(ComponentValue::Json { value }) = components.get("ludenea::Pose") else {
        panic!("{components:?}");
    };
    value["position"].clone()
}

fn field(name: &str) -> PathSegment {
    PathSegment::Field {
        name: name.to_string(),
    }
}

fn scene_text(host: &mut Host) -> String {
    match ok(host, Command::SaveScene) {
        Reply::Scene { ron } => ron,
        other => panic!("{other:?}"),
    }
}

#[test]
fn create_prefab_returns_prefab_text() {
    let (mut host, reply) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    let Reply::Loaded { load_addresses, .. } = reply else {
        panic!("{reply:?}");
    };
    let (ground, _) = load_addresses
        .into_iter()
        .find(|(_, address)| address == &[0])
        .unwrap();
    let prefab = text(ok(
        &mut host,
        Command::CreatePrefab {
            entities: vec![ground],
        },
    ));
    assert!(prefab.starts_with("Prefab("), "{prefab}");
    assert!(prefab.contains("ludenea::Collider"), "{prefab}");
}

#[test]
fn place_instance_over_protocol() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    assert_eq!(
        ok(&mut host, Command::PlaceInstance { prefab: PREFAB }),
        Reply::Placed { instance: 7 }
    );
    let placed = instances(&mut host);
    assert!(
        placed
            .iter()
            .any(|info| info.address == [7] && info.prefab == PREFAB)
    );
    assert!(scene_text(&mut host).contains("(id: 7, prefab: \"38b36be4f8a140d695e5cdac24c82f90\""));
    // The new instance's entities have load addresses at once, without another scene load.
    let Reply::Instances { load_addresses, .. } = ok(&mut host, Command::GetInstances) else {
        panic!("get_instances replies instances");
    };
    assert!(load_addresses.iter().any(|(_, address)| address == &[7, 0]));

    // Instance entities are refused as the entities of a new prefab instance.
    let entity = instance_entity(&mut host, 7);
    let error = host
        .handle(Command::ReplaceWithInstance {
            entities: vec![entity],
            prefab: AssetId(7),
            prefab_text: fs::read_to_string(CRATE_PREFAB).unwrap(),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidScene);
    assert!(
        error
            .message
            .starts_with("entity 7/0 belongs to a prefab instance"),
        "{error:?}"
    );
}

#[test]
fn apply_override_then_revert() {
    let scene = fs::read_to_string(SPAWNER_SCENE).unwrap();
    let (mut host, _) = spawner_host(scene.clone());
    let path = vec![field("position"), field("y")];
    ok(
        &mut host,
        Command::ApplyOverride {
            instance: vec![5],
            entity: vec![0],
            component: "ludenea::Pose".to_string(),
            path: path.clone(),
            value: ComponentValue::Json { value: json!(3.0) },
        },
    );
    let entity = instance_entity(&mut host, 5);
    assert_eq!(
        pose_position(&mut host, entity),
        json!({"x": 0.0, "y": 3.0})
    );
    assert!(scene_text(&mut host).contains("value: 3.0)"));

    ok(
        &mut host,
        Command::RevertOverride {
            instance: vec![5],
            entity: vec![0],
            component: "ludenea::Pose".to_string(),
            path,
        },
    );
    let entity = instance_entity(&mut host, 5);
    assert_eq!(
        pose_position(&mut host, entity),
        json!({"x": 0.0, "y": 26.0})
    );
    let instance_5 = "(id: 5, prefab: \"38b36be4f8a140d695e5cdac24c82f90\"";
    assert_eq!(
        scene_text(&mut host),
        scene.replace(
            &format!("{instance_5})"),
            &format!("{instance_5}, overrides: [])")
        )
    );
}

#[test]
fn set_field_on_instance_entity_at_tick_0_overrides_that_field() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    let entity = instance_entity(&mut host, 5);
    ok(
        &mut host,
        Command::SetField {
            target: FieldTarget::Component {
                entity,
                component: "ludenea::Pose".to_string(),
            },
            path: vec![field("position"), field("y")],
            value: json!(3.0),
        },
    );
    let entity = instance_entity(&mut host, 5);
    assert_eq!(
        pose_position(&mut host, entity),
        json!({"x": 0.0, "y": 3.0})
    );
    let scene = scene_text(&mut host);
    assert!(
        scene.contains(
            r#"(entity: [0], component: "ludenea::Pose", path: [(type:"field",name:"position"),(type:"field",name:"y")], value: 3.0)"#
        ),
        "{scene}"
    );
    assert!(!scene.contains("path: []"), "{scene}");
    // A scene edit, not a recorded one.
    assert!(timeline(&mut host).edit_ticks.is_empty());
}

#[test]
fn apply_to_prefab_returns_updated_prefab() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    // Instance 6 overrides the position's `x` with 5.0.
    let prefab = text(ok(
        &mut host,
        Command::ApplyToPrefab {
            instance: vec![6],
            entity: vec![0],
            component: "ludenea::Pose".to_string(),
            path: vec![field("position"), field("x")],
        },
    ));
    let original = fs::read_to_string(CRATE_PREFAB).unwrap();
    assert_eq!(
        prefab,
        original.replace("position:(x:0.0,y:26.0)", "position:(x:5.0,y:26.0)")
    );
    assert_ne!(prefab, original);
}

fn export_replay(host: &mut Host, branch: Option<BranchId>) -> Bytes {
    match ok(host, Command::ExportReplay { branch }) {
        Reply::Replay { replay } => replay,
        other => panic!("{other:?}"),
    }
}

/// A spawner session with a crate spawned at tick 10, 40 ticks long.
fn recorded_host() -> Host {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    step(&mut host, 10);
    spawn_crate(&mut host);
    step(&mut host, 30);
    host
}

/// Advances `host` until its resimulation ends, and returns the events.
fn settle(host: &mut Host) -> Vec<Event> {
    let mut events = host.advance(0.0);
    while timeline(host).resimulating {
        events.extend(host.advance(0.0));
    }
    events
}

#[test]
fn export_and_load_replay_round_trip() {
    let mut host = recorded_host();
    let expected = hash(&mut host);
    let replay = export_replay(&mut host, None);
    // Exporting leaves the session where it was.
    assert_eq!(hash(&mut host), expected);
    assert_eq!(timeline(&mut host).current, 40);

    let mut loaded = Host::new(boxes::app).unwrap();
    assert_eq!(
        ok(&mut loaded, Command::LoadReplay { replay }),
        Reply::ReplayLoaded {
            fingerprint_differs: false
        }
    );
    let events = settle(&mut loaded);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Diverged { .. })),
        "{events:?}"
    );
    ok(&mut loaded, Command::Seek { tick: 40 });
    assert_eq!(hash(&mut loaded), expected);
}

#[test]
fn export_replay_of_another_branch() {
    let mut host = recorded_host();
    ok(&mut host, Command::Seek { tick: 20 });
    spawn_crate(&mut host);
    let forked = timeline(&mut host);
    assert_ne!(forked.branch, BranchId::ROOT);
    let replay = export_replay(&mut host, Some(BranchId::ROOT));
    let after = timeline(&mut host);
    assert_eq!((after.branch, after.current), (forked.branch, 20));

    let mut loaded = Host::new(boxes::app).unwrap();
    ok(&mut loaded, Command::LoadReplay { replay });
    settle(&mut loaded);
    ok(
        &mut host,
        Command::SwitchBranch {
            branch: BranchId::ROOT,
        },
    );
    ok(&mut host, Command::Seek { tick: 40 });
    ok(&mut loaded, Command::Seek { tick: 40 });
    assert_eq!(hash(&mut loaded), hash(&mut host));
}

fn decode_replay(bytes: &Bytes) -> Replay {
    Replay::decode(&bytes.0).unwrap()
}

#[test]
fn load_replay_with_broken_input_names_the_tick() {
    let mut host = recorded_host();
    let mut replay = decode_replay(&export_replay(&mut host, None));
    let mut inputs = InputLog::decode(&replay.inputs).unwrap();
    inputs.ticks[3] = vec![0xff; 9];
    replay.inputs = inputs.encode();

    let mut loaded = Host::new(boxes::app).unwrap();
    let error = loaded
        .handle(Command::LoadReplay {
            replay: Bytes(replay.encode()),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidReplay);
    assert!(
        error
            .message
            .starts_with("cannot decode the input of tick 3 of the replay: "),
        "{error:?}"
    );
    // The previous session stays.
    assert_eq!(timeline(&mut loaded).head, 0);
    assert_eq!(timeline(&mut host).head, 39);
}

#[test]
fn load_replay_with_changed_fingerprint_reports_first_divergent_tick() {
    let mut host = recorded_host();
    let mut replay = decode_replay(&export_replay(&mut host, None));
    // As recorded by a build whose types and simulation differ from tick 12.
    replay.fingerprint ^= 1;
    let got = replay.hashes[12];
    replay.hashes[12] ^= 1;
    replay.hashes[20] ^= 1;

    let mut loaded = Host::new(boxes::app).unwrap();
    assert_eq!(
        ok(
            &mut loaded,
            Command::LoadReplay {
                replay: Bytes(replay.encode()),
            }
        ),
        Reply::ReplayLoaded {
            fingerprint_differs: true
        }
    );
    let diverged: Vec<Event> = settle(&mut loaded)
        .into_iter()
        .filter(|event| matches!(event, Event::Diverged { .. }))
        .collect();
    assert_eq!(
        diverged,
        [Event::Diverged {
            tick: 12,
            expected: Hash64(got ^ 1),
            got: Hash64(got),
        }]
    );
}

fn snapshot(host: &mut Host) -> Bytes {
    match ok(host, Command::TakeSnapshot) {
        Reply::Snapshot { snapshot, .. } => snapshot,
        other => panic!("{other:?}"),
    }
}

fn differences(host: &mut Host, a: Bytes, b: Option<Bytes>) -> Vec<Difference> {
    match ok(host, Command::DiffSnapshots { a, b }) {
        Reply::Differences { differences } => differences,
        other => panic!("{other:?}"),
    }
}

#[test]
fn diff_snapshots_is_structured() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    let a = snapshot(&mut host);
    step(&mut host, 10);
    let b = snapshot(&mut host);
    let found = differences(&mut host, a, Some(b));
    let fall = found
        .iter()
        .find(|difference| {
            difference.component == "ludenea::Pose" && difference.path == ["position", "y"]
        })
        .unwrap_or_else(|| panic!("{found:?}"));
    assert_eq!(fall.left, Some(json!(26.0)));
    assert!(
        fall.right.as_ref().unwrap().as_f64().unwrap() != 26.0,
        "{fall:?}"
    );
    assert_eq!(fall.left_text, "26.0");
    assert!(fall.subject.starts_with("entity "), "{fall:?}");
}

#[test]
fn diff_snapshot_against_current_tick() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    let a = snapshot(&mut host);
    step(&mut host, 10);
    let b = snapshot(&mut host);
    let current = differences(&mut host, a.clone(), None);
    assert!(!current.is_empty());
    assert_eq!(current, differences(&mut host, a, Some(b)));
}

#[test]
fn snapshot_with_other_fingerprint_is_refused() {
    let (mut host, _) = spawner_host(fs::read_to_string(SPAWNER_SCENE).unwrap());
    let a = snapshot(&mut host);
    let mut other = Snapshot::decode(&a.0).unwrap();
    let fingerprint = other.fingerprint;
    other.fingerprint ^= 1;
    let error = host
        .handle(Command::DiffSnapshots {
            a,
            b: Some(Bytes(other.encode())),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidSnapshot);
    for named in [fingerprint, fingerprint ^ 1] {
        assert!(
            error.message.contains(&format!("{named:016x}")),
            "{error:?}"
        );
    }
}

#[test]
fn compare_traces_against_current_branch() {
    let mut host = recorded_host();
    let Reply::Session { hashes, .. } = ok(&mut host, Command::ExportSession) else {
        panic!("export_session replies session");
    };
    let mut hashes: Vec<u64> = hashes.into_iter().map(|hash| hash.0).collect();
    hashes[15] ^= 1;
    hashes.truncate(31);
    let trace = Trace {
        version: TRACE_VERSION,
        scene_hash: 0,
        inputs_hash: 0,
        hashes,
    };
    assert_eq!(
        ok(
            &mut host,
            Command::CompareTraces {
                a: Bytes(trace.encode()),
                b: None,
            }
        ),
        Reply::TraceComparison(TraceComparison {
            common: 31,
            first_divergent: Some(15),
            only_in_longer: 10,
        })
    );
}

#[test]
fn replay_of_a_replay_keeps_prefabs_reached_only_by_instances() {
    // Without the spawner, only instances 5 and 6 reach the crate prefab.
    let scene: String = fs::read_to_string(SPAWNER_SCENE)
        .unwrap()
        .lines()
        .filter(|line| !line.contains("boxes::Spawner"))
        .map(|line| format!("{line}\n"))
        .collect();
    let (mut host, _) = spawner_host(scene);
    step(&mut host, 20);
    let a = export_replay(&mut host, None);

    let mut loaded = Host::new(boxes::app).unwrap();
    ok(&mut loaded, Command::LoadReplay { replay: a.clone() });
    settle(&mut loaded);
    let b = export_replay(&mut loaded, None);
    let (a, b) = (decode_replay(&a), decode_replay(&b));
    assert_eq!(a.assets.len(), 1);
    assert_eq!(b.assets, a.assets);
    assert_eq!(b.hashes, a.hashes);
    let mut sim = boxes::app().build().unwrap();
    let report = play_replay(&mut sim, &b, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);

    // Exporting leaves the replay session's tick 0 as recorded.
    ok(&mut loaded, Command::Seek { tick: 0 });
    assert_eq!(hash(&mut loaded), Hash64(a.hashes[0]));
}

fn panic_at_30(time: Res<Time>) {
    if time.tick == 30 {
        panic!("boom at thirty");
    }
}

#[test]
fn export_replay_contains_a_panic_and_keeps_the_session() {
    // The root runs to tick 40; the current branch forks at tick 20.
    let mut host = recorded_host();
    ok(&mut host, Command::Seek { tick: 20 });
    spawn_crate(&mut host);
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
        ..
    } = ok(&mut host, Command::ExportSession)
    else {
        panic!("export_session replies session");
    };
    // A build that panics at tick 30, which only the stored root reaches.
    fn app() -> App {
        let mut app = boxes::app();
        app.system(Sim, panic_at_30);
        app
    }
    let mut panicking = Host::new(app).unwrap();
    ok(
        &mut panicking,
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
            keeps_assets: false,
        },
    );
    settle(&mut panicking);
    let before = (hash(&mut panicking), timeline(&mut panicking).current);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let error = panicking
        .handle(Command::ExportReplay {
            branch: Some(BranchId::ROOT),
        })
        .unwrap_err();
    std::panic::set_hook(hook);
    assert_eq!(error.code, ErrorCode::SystemPanicked);
    assert!(error.message.contains("boom at thirty"), "{error:?}");
    assert_eq!(
        (hash(&mut panicking), timeline(&mut panicking).current),
        before
    );
    // The current branch still exports.
    export_replay(&mut panicking, None);
}

#[test]
fn load_replay_refuses_an_edit_past_the_recorded_ticks() {
    let mut host = recorded_host();
    let mut replay = decode_replay(&export_replay(&mut host, None));
    let mut edits = EditLog::decode(&replay.edits).unwrap();
    edits.edits.push(TimedEdit {
        tick: u64::MAX,
        edit: Edit::Despawn {
            entity: EntityRef::Loaded(vec![0]),
        },
    });
    replay.edits = edits.encode();
    let mut loaded = Host::new(boxes::app).unwrap();
    let error = loaded
        .handle(Command::LoadReplay {
            replay: Bytes(replay.encode()),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidReplay);
    assert!(
        error.message.starts_with(&format!(
            "cannot decode the edit of tick {} of the replay: ",
            u64::MAX
        )),
        "{error:?}"
    );
    assert_eq!(timeline(&mut loaded).head, 0);
}

#[derive(Component, Serialize, Deserialize)]
struct Score(u32);

fn grow(mut scores: Query<&mut Score>, mut rng: ResMut<Rng>) {
    for score in scores.iter_mut() {
        score.0 += rng.range_u32(1, 4);
    }
}

fn score_app() -> App {
    let mut app = App::new();
    app.component::<Score>().system(Sim, grow);
    app
}

#[test]
fn branch_restored_before_its_edits_still_reloads() {
    // A game without inputs records no input: its edits lie past its hashes once a
    // restore drops them.
    let mut host = Host::new(score_app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: r#"Scene(version: 1, resources: {}, entities: [{ "protocol_v3::Score": (5) }])"#
                .to_string(),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let set_score = |host: &mut Host, score: u32| {
        let Reply::Entities { rows, .. } = ok(
            host,
            Command::QueryEntities {
                filter: String::new(),
                offset: 0,
                limit: 1,
            },
        ) else {
            panic!("query_entities replies entities");
        };
        ok(
            host,
            Command::SetField {
                target: FieldTarget::Component {
                    entity: rows[0].entity,
                    component: "protocol_v3::Score".to_string(),
                },
                path: Vec::new(),
                value: json!(score),
            },
        );
    };
    step(&mut host, 20);
    set_score(&mut host, 100);
    step(&mut host, 10);
    ok(&mut host, Command::Seek { tick: 5 });
    let at_5 = snapshot(&mut host);
    ok(&mut host, Command::Restore { snapshot: at_5 });
    ok(&mut host, Command::Seek { tick: 3 });
    set_score(&mut host, 7);
    let before = timeline(&mut host);
    assert_eq!(before.branches.len(), 2);

    let Reply::Session {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        hashes,
        assets,
        branches: tree,
        document,
        ..
    } = ok(&mut host, Command::ExportSession)
    else {
        panic!("export_session replies session");
    };
    let mut loaded = Host::new(score_app).unwrap();
    ok(
        &mut loaded,
        Command::LoadSession {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            expected: hashes,
            assets,
            branches: tree,
            document,
            keeps_assets: false,
        },
    );
    settle(&mut loaded);
    assert_eq!(branches(&timeline(&mut loaded)), branches(&before));
}

#[test]
fn replay_of_a_replay_keeps_prefabs_reached_only_by_handles() {
    // Without instances 5 and 6, only the spawner's handle reaches the crate prefab.
    let scene: String = fs::read_to_string(SPAWNER_SCENE)
        .unwrap()
        .lines()
        .filter(|line| !line.contains("(id: 5, prefab") && !line.contains("(id: 6, prefab"))
        .map(|line| format!("{line}\n"))
        .collect();
    let (mut host, _) = spawner_host(scene);
    // The spawner spawns the prefab at tick 60.
    step(&mut host, 70);
    let a = export_replay(&mut host, None);

    let mut loaded = Host::new(boxes::app).unwrap();
    ok(&mut loaded, Command::LoadReplay { replay: a.clone() });
    settle(&mut loaded);
    let b = export_replay(&mut loaded, None);
    let (a, b) = (decode_replay(&a), decode_replay(&b));
    let ids: Vec<AssetId> = a.assets.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, [PREFAB, STATS]);
    assert_eq!(b.assets, a.assets);
    assert_eq!(b.hashes, a.hashes);
    let mut sim = boxes::app().build().unwrap();
    let report = play_replay(&mut sim, &b, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
}

#[test]
fn compare_traces_against_the_session_waits_for_resimulation() {
    let mut host = recorded_host();
    let Reply::Scene { ron } = ok(&mut host, Command::SaveScene) else {
        panic!("save_scene replies scene");
    };
    ok(
        &mut host,
        Command::ReplaceScene {
            scene: ron,
            assets: assets(),
        },
    );
    assert!(timeline(&mut host).resimulating);
    let trace = Trace {
        version: TRACE_VERSION,
        scene_hash: 0,
        inputs_hash: 0,
        hashes: vec![1],
    };
    let error = host
        .handle(Command::CompareTraces {
            a: Bytes(trace.encode()),
            b: None,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Resimulating);
}

/// How many more times `panic_from_33` panics.
static PANICS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn panic_from_33(time: Res<Time>) {
    use std::sync::atomic::Ordering;
    let left = PANICS.load(Ordering::Relaxed);
    if time.tick >= 33 && left > 0 {
        PANICS.store(left - 1, Ordering::Relaxed);
        panic!("boom {left}");
    }
}

#[test]
fn export_replay_reports_a_failed_return_on_the_console() {
    fn app() -> App {
        let mut app = boxes::app();
        app.system(Sim, panic_from_33);
        app
    }
    let mut host = Host::new(app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(10, 3),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    step(&mut host, 35);
    host.advance(0.0);

    // Recording panics at tick 33, then so does replaying from the checkpoint at 32 back
    // to tick 35.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    PANICS.store(2, std::sync::atomic::Ordering::Relaxed);
    let error = host
        .handle(Command::ExportReplay { branch: None })
        .unwrap_err();
    std::panic::set_hook(hook);
    assert_eq!(error.code, ErrorCode::SystemPanicked);
    assert!(error.message.contains("boom 2"), "{error:?}");
    let events = host.advance(0.0);
    let logged = events
        .iter()
        .any(|event| matches!(event, Event::Console { text, .. } if text.contains("boom 1")));
    assert!(logged, "{events:?}");
    assert_eq!(timeline(&mut host).current, 35);
}
