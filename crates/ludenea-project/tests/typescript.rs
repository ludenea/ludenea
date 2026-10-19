//! Regenerates the TypeScript types of the project protocol into the control client, and pins
//! the protocol's wire shape to its version.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ludenea_control::{Bytes, GameInfo, Hash64};
use ludenea_ecs::AssetId;
use ludenea_project::AssetKind;
use ludenea_project::protocol::{
    PROJECT_PROTOCOL_FINGERPRINT, PROJECT_PROTOCOL_VERSION, ProjectClientMessage, ProjectRequest,
    ProjectServerMessage,
};
use ts_rs::{Config, TS};

#[path = "../../ludenea-control/tests/support/fingerprint.rs"]
mod fingerprint;

#[test]
fn the_wire_shape_is_pinned_to_its_protocol_version() {
    // The control types the messages reach are part of the shape.
    fingerprint::assert_wire_shape_is_pinned::<ProjectClientMessage, ProjectServerMessage>(
        "Project protocol",
        PROJECT_PROTOCOL_VERSION,
        PROJECT_PROTOCOL_FINGERPRINT,
        "PROJECT_PROTOCOL",
    );
}

#[test]
fn export_typescript_bindings() {
    // The project types go to `generated/project/`; the control types they use (`GameInfo`)
    // are written where the control protocol export puts them.
    let config = Config::from_env();
    ProjectClientMessage::export_all(&config).unwrap();
    ProjectServerMessage::export_all(&config).unwrap();

    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../editor/packages/control-client/src/generated/project");
    write_version(&directory, &config);
    let mut names: Vec<String> = fs::read_dir(&directory)
        .unwrap()
        .filter_map(|entry| {
            let file = entry.unwrap().file_name().into_string().unwrap();
            file.strip_suffix(".ts").map(str::to_owned)
        })
        .filter(|name| name != "index")
        .collect();
    names.sort();
    let index: String = names
        .iter()
        .map(|name| format!("export * from \"./{name}\";\n"))
        .collect();
    fs::write(directory.join("index.ts"), index).unwrap();

    for expected in [
        "ProjectClientMessage",
        "ProjectServerMessage",
        "ProjectRequest",
        "ProjectEvent",
    ] {
        assert!(
            directory.join(format!("{expected}.ts")).exists(),
            "{expected}.ts"
        );
    }
    let request = fs::read_to_string(directory.join("ProjectRequest.ts")).unwrap();
    assert!(
        request.contains("\"type\": \"write_game_types\""),
        "{request}"
    );
}

/// One request of each type, with arbitrary arguments.
fn every_request() -> Vec<ProjectRequest> {
    let path = || "scenes/main.scene.ron".to_string();
    vec![
        ProjectRequest::Create {
            parent: "projects".to_string(),
            name: "demo".to_string(),
            engine_path: None,
        },
        ProjectRequest::Open { path: path() },
        ProjectRequest::Info,
        ProjectRequest::ListFiles,
        ProjectRequest::ReadText { path: path() },
        ProjectRequest::WriteText {
            path: path(),
            text: "()".to_string(),
        },
        ProjectRequest::ReadBinary { path: path() },
        ProjectRequest::WriteBinary {
            path: path(),
            bytes: Bytes(vec![1]),
        },
        ProjectRequest::CreateAsset {
            path: path(),
            kind: AssetKind::Scene,
            text: "()".to_string(),
        },
        ProjectRequest::ReadAssetTexts,
        ProjectRequest::StoreSession {
            document: AssetId(7),
            bytes: Bytes(vec![1]),
        },
        ProjectRequest::LoadStoredSession {
            document: AssetId(7),
        },
        ProjectRequest::SetAsideStoredSession {
            document: AssetId(7),
        },
        ProjectRequest::Rename {
            from: path(),
            to: path(),
        },
        ProjectRequest::Delete { path: path() },
        ProjectRequest::Build,
        ProjectRequest::ListPlugins,
        ProjectRequest::GameEndpoint,
        ProjectRequest::WriteGameTypes {
            info: GameInfo {
                components: Vec::new(),
                resources: Vec::new(),
                assets: Vec::new(),
                events: Vec::new(),
                fingerprint: Hash64(0),
                input: None,
                tick_rate: 60,
                debug_layers: Vec::new(),
            },
        },
        ProjectRequest::RestartGame,
    ]
}

/// Writes `version.ts`: the protocol version and fingerprint, and the reply types of each
/// request type, which the client types its replies with.
fn write_version(directory: &Path, config: &Config) {
    let reply_types: BTreeMap<String, &[&str]> = every_request()
        .iter()
        .map(|request| {
            let value = serde_json::to_value(request).unwrap();
            (
                value["type"].as_str().unwrap().to_owned(),
                request.reply_types(),
            )
        })
        .collect();
    let declared: BTreeSet<String> = ProjectRequest::decl(config)
        .split("\"type\": \"")
        .skip(1)
        .map(|rest| rest[..rest.find('"').unwrap()].to_owned())
        .collect();
    assert_eq!(
        reply_types.keys().cloned().collect::<BTreeSet<_>>(),
        declared,
        "`every_request` (left) misses a request type (right)"
    );
    let entries: String = reply_types
        .iter()
        .map(|(request, replies)| {
            let replies: Vec<String> = replies.iter().map(|reply| format!("\"{reply}\"")).collect();
            format!("  {request}: {};\n", replies.join(" | "))
        })
        .collect();
    let version = format!(
        "// This file was generated by crates/ludenea-project/tests/typescript.rs. Do not edit this file manually.\n\
         \n\
         /** The project protocol version this build speaks: the editor and the service connect only when theirs are equal. */\n\
         export const PROJECT_PROTOCOL_VERSION = {PROJECT_PROTOCOL_VERSION};\n\
         \n\
         /** The wire shape of `PROJECT_PROTOCOL_VERSION`, a hash of the protocol's types: the editor and the service connect only when theirs are equal too. */\n\
         export const PROJECT_PROTOCOL_FINGERPRINT = \"{:016x}\";\n\
         \n\
         /** The `type` of the replies each request type may get when it succeeds. */\n\
         export type ProjectReplyTypes = {{\n\
         {entries}}};\n",
        PROJECT_PROTOCOL_FINGERPRINT.0
    );
    fs::write(directory.join("version.ts"), version).unwrap();
}
