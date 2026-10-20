//! Test helpers shared by the protocol tests.

use std::collections::BTreeMap;

use ludenea_app::{RawGamepad, RawInput};
use ludenea_control::{
    BranchId, Bytes, Command, ComponentValue, Document, FieldTarget, Hash64, PathSegment, Topic,
};
use ludenea_ecs::{AssetId, Entity};

/// Every type of command, several times for some, with arbitrary arguments.
pub fn every_command() -> Vec<Command> {
    let entity: Entity = serde_json::from_str(r#"{"index":1,"generation":2}"#).unwrap();
    let bytes = Bytes(vec![0, 1, 254, 255]);
    vec![
        Command::LoadScene {
            scene: "x".into(),
            assets: Vec::new(),
            document: Document::Scene,
        },
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
            b: Some(bytes.clone()),
        },
        Command::DiffSnapshots {
            a: bytes.clone(),
            b: None,
        },
        Command::CompareTraces {
            a: bytes.clone(),
            b: None,
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
        Command::Batch {
            commands: vec![
                Command::Despawn { entity },
                Command::RemoveComponent {
                    entity,
                    component: "n".into(),
                },
            ],
        },
        Command::SaveScene,
        Command::MarkSceneSaved { text: "s".into() },
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
        Command::ReplaceScene {
            scene: "y".into(),
            assets: Vec::new(),
        },
        Command::LoadSession {
            scene: "z".into(),
            saved_scene: Some("w".into()),
            inputs: bytes.clone(),
            edits: bytes.clone(),
            undo: bytes.clone(),
            expected: vec![Hash64(1)],
            assets: vec![(AssetId(7), "a".into())],
            saved_assets: vec![(AssetId(7), "b".into())],
            branches: bytes.clone(),
            document: Document::Prefab { id: AssetId(7) },
            keeps_assets: false,
        },
        Command::GetAsset { id: AssetId(7) },
        Command::SetAssetField {
            id: AssetId(7),
            path: vec![PathSegment::Index { index: 1 }],
            value: serde_json::json!(2),
        },
        Command::SaveAsset { id: AssetId(7) },
        Command::MarkAssetSaved {
            id: AssetId(7),
            text: "t".into(),
        },
        Command::ApplyToAsset { id: AssetId(7) },
        Command::NewAssetValue { asset: "a".into() },
        Command::SwitchBranch {
            branch: BranchId(1),
        },
        Command::RenameBranch {
            branch: BranchId(1),
            name: "b".into(),
        },
        Command::DeleteBranch {
            branch: BranchId(1),
        },
        Command::CreatePrefab {
            entities: vec![entity],
        },
        Command::PlaceInstance { prefab: AssetId(7) },
        Command::ReplaceWithInstance {
            entities: vec![entity],
            prefab: AssetId(7),
            prefab_text: "p".into(),
        },
        Command::ApplyOverride {
            instance: vec![5],
            entity: vec![0],
            component: "n".into(),
            path: vec![PathSegment::Field { name: "a".into() }],
            value: ComponentValue::Json {
                value: serde_json::json!(1),
            },
        },
        Command::RevertOverride {
            instance: vec![5, 1],
            entity: vec![0],
            component: "n".into(),
            path: vec![],
        },
        Command::ApplyToPrefab {
            instance: vec![5],
            entity: vec![0],
            component: "n".into(),
            path: vec![],
        },
        Command::GetInstances,
        Command::ExportReplay {
            branch: Some(BranchId(1)),
        },
        Command::ExportReplay { branch: None },
        Command::LoadReplay {
            replay: bytes.clone(),
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
            topic: Topic::Resource {
                name: "ludenea::Time".into(),
            },
        },
        Command::Unsubscribe {
            topic: Topic::Introspection,
        },
        Command::SetCompareBranch {
            branch: Some(BranchId(2)),
        },
        Command::SetCompareBranch { branch: None },
        Command::GetGhostDraw {
            tick: 9,
            layers: vec!["physics.colliders".into()],
        },
        Command::Subscribe {
            topic: Topic::Ghost,
        },
        Command::GetDivergence { up_to: Some(30) },
        Command::GetDivergence { up_to: None },
    ]
}
