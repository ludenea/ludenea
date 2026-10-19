//! Regenerates the TypeScript types of the protocol into the control client, and pins the
//! protocol's wire shape to its version.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ludenea_control::protocol::{
    ClientMessage, Command, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION, ServerMessage,
};
use ts_rs::{Config, TS, TypeVisitor};
use xxhash_rust::xxh3::xxh3_64;

mod support;

const CHANGELOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");

#[test]
fn the_wire_shape_is_pinned_to_its_protocol_version() {
    let fingerprint = wire_fingerprint(&Config::from_env());
    assert!(
        fingerprint == PROTOCOL_FINGERPRINT.0,
        "the wire shape of control protocol {PROTOCOL_VERSION} changed (fingerprint \
         {fingerprint:016x}, pinned {:016x}): set PROTOCOL_FINGERPRINT to it, bump \
         PROTOCOL_VERSION unless this version is unreleased, and record the change in \
         CHANGELOG.md",
        PROTOCOL_FINGERPRINT.0
    );
    let entry = format!("Control protocol {PROTOCOL_VERSION}, fingerprint `{fingerprint:016x}`");
    let changelog = fs::read_to_string(CHANGELOG)
        .unwrap_or_else(|error| panic!("{CHANGELOG} cannot be read: {error}"));
    assert!(
        changelog.contains(&entry),
        "{CHANGELOG} does not name this wire shape: write {entry} in the entry of control \
         protocol {PROTOCOL_VERSION}"
    );
}

/// xxh3 of the TypeScript declarations of every type the messages reach, in sorted order and
/// without their documentation, so only a change to what travels moves it.
fn wire_fingerprint(config: &Config) -> u64 {
    let mut declarations = Declarations {
        config,
        seen: BTreeSet::new(),
        texts: Vec::new(),
    };
    declarations.visit::<ClientMessage>();
    declarations.visit::<ServerMessage>();
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
    let config = Config::from_env();
    ClientMessage::export_all(&config).unwrap();
    ServerMessage::export_all(&config).unwrap();

    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../editor/packages/control-client/src/generated");
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
    // `serde_json::Value` is exported into a subdirectory.
    let index = index + "export * from \"./serde_json/JsonValue\";\n";
    fs::write(directory.join("index.ts"), index).unwrap();

    for expected in [
        "ClientMessage",
        "ServerMessage",
        "Command",
        "Event",
        "TypeSchema",
    ] {
        assert!(
            directory.join(format!("{expected}.ts")).exists(),
            "{expected}.ts"
        );
    }
    let command = fs::read_to_string(directory.join("Command.ts")).unwrap();
    assert!(command.contains("\"type\": \"set_field\""), "{command}");
    let hash = fs::read_to_string(directory.join("Hash64.ts")).unwrap();
    assert!(hash.contains("export type Hash64 = string;"), "{hash}");
}

/// Writes `version.ts`: the protocol version and fingerprint, and the reply type of each
/// command type, which the client types its replies with.
fn write_version(directory: &Path, config: &Config) {
    let reply_types: BTreeMap<String, &str> = support::every_command()
        .iter()
        .map(|command| {
            let value = serde_json::to_value(command).unwrap();
            (
                value["type"].as_str().unwrap().to_owned(),
                command.reply_type(),
            )
        })
        .collect();
    let declared: BTreeSet<String> = Command::decl(config)
        .split("\"type\": \"")
        .skip(1)
        .map(|rest| rest[..rest.find('"').unwrap()].to_owned())
        .collect();
    assert_eq!(
        reply_types.keys().cloned().collect::<BTreeSet<_>>(),
        declared,
        "`every_command` in tests/support (left) misses a command type (right)"
    );
    let entries: String = reply_types
        .iter()
        .map(|(command, reply)| format!("  {command}: \"{reply}\";\n"))
        .collect();
    let version = format!(
        "// This file was generated by crates/ludenea-control/tests/typescript.rs. Do not edit this file manually.\n\
         \n\
         /** The control protocol version this build speaks: a client and a host connect only when theirs are equal. */\n\
         export const PROTOCOL_VERSION = {PROTOCOL_VERSION};\n\
         \n\
         /** The wire shape of `PROTOCOL_VERSION`, a hash of the protocol's types: a client and a host connect only when theirs are equal too. */\n\
         export const PROTOCOL_FINGERPRINT = \"{:016x}\";\n\
         \n\
         /** The `type` of the reply each command type gets when it succeeds. */\n\
         export type ReplyTypes = {{\n\
         {entries}}};\n",
        PROTOCOL_FINGERPRINT.0
    );
    fs::write(directory.join("version.ts"), version).unwrap();
}
