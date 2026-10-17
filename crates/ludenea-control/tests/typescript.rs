//! Regenerates the TypeScript types of the protocol into the control client.

use std::fs;
use std::path::PathBuf;

use ludenea_control::protocol::{ClientMessage, ServerMessage};
use ts_rs::{Config, TS};

#[test]
fn export_typescript_bindings() {
    let config = Config::from_env();
    ClientMessage::export_all(&config).unwrap();
    ServerMessage::export_all(&config).unwrap();

    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../editor/packages/control-client/src/generated");
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
