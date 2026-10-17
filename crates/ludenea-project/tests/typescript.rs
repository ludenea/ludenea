//! Regenerates the TypeScript types of the project protocol into the control client.

use std::fs;
use std::path::PathBuf;

use ludenea_project::protocol::{ProjectClientMessage, ProjectServerMessage};
use ts_rs::{Config, TS};

#[test]
fn export_typescript_bindings() {
    // The project types go to `generated/project/`; the control types they use (`GameInfo`)
    // are written where the control protocol export puts them.
    let config = Config::from_env();
    ProjectClientMessage::export_all(&config).unwrap();
    ProjectServerMessage::export_all(&config).unwrap();

    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../editor/packages/control-client/src/generated/project");
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
