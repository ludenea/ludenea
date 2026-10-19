//! Regenerates the TypeScript types of the project protocol into the control client, and pins
//! the protocol's wire shape to its version.

use std::any::TypeId;
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
use ts_rs::{Config, TS, TypeVisitor};
use xxhash_rust::xxh3::xxh3_64;

const CHANGELOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");

#[test]
fn the_wire_shape_is_pinned_to_its_protocol_version() {
    let fingerprint = wire_fingerprint(&Config::from_env());
    assert!(
        fingerprint == PROJECT_PROTOCOL_FINGERPRINT.0,
        "the wire shape of project protocol {PROJECT_PROTOCOL_VERSION} changed (fingerprint \
         {fingerprint:016x}, pinned {:016x}): set PROJECT_PROTOCOL_FINGERPRINT to it, bump \
         PROJECT_PROTOCOL_VERSION unless this version is unreleased, and record the change in \
         CHANGELOG.md",
        PROJECT_PROTOCOL_FINGERPRINT.0
    );
    let entry =
        format!("Project protocol {PROJECT_PROTOCOL_VERSION}, fingerprint `{fingerprint:016x}`");
    let changelog = fs::read_to_string(CHANGELOG)
        .unwrap_or_else(|error| panic!("{CHANGELOG} cannot be read: {error}"));
    assert!(
        changelog.contains(&entry),
        "{CHANGELOG} does not name this wire shape: write {entry} in the entry of project \
         protocol {PROJECT_PROTOCOL_VERSION}"
    );
}

/// xxh3 of the TypeScript declarations of every type the messages reach, the control types
/// among them, in sorted order and without their documentation, so only a change to what
/// travels moves it.
fn wire_fingerprint(config: &Config) -> u64 {
    let mut declarations = Declarations {
        config,
        seen: BTreeSet::new(),
        texts: Vec::new(),
    };
    declarations.visit::<ProjectClientMessage>();
    declarations.visit::<ProjectServerMessage>();
    declarations.texts.sort();
    xxh3_64(declarations.texts.join("\n").as_bytes())
}

/// Walks the types `export_all` writes, collecting their declarations.
struct Declarations<'a> {
    config: &'a Config,
    seen: BTreeSet<TypeId>,
    texts: Vec<String>,
}

impl TypeVisitor for Declarations<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        self.texts.push(without_docs(&T::decl(self.config)));
        T::visit_dependencies(self);
    }
}

/// `declaration` without the JSDoc blocks ts-rs writes for documented fields and variants,
/// and with its whitespace collapsed, so editing a doc comment leaves it unchanged.
fn without_docs(declaration: &str) -> String {
    let mut rest = declaration;
    let mut text = String::new();
    while let Some(start) = rest.find("/**") {
        text.push_str(&rest[..start]);
        let end = rest[start..]
            .find("*/")
            .unwrap_or_else(|| panic!("an unterminated doc comment in {declaration}"));
        rest = &rest[start + end + 2..];
    }
    text.push_str(rest);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
