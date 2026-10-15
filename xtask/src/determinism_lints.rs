//! Proves that the determinism lints of the simulation crates fire: a fixture
//! using every forbidden item is checked against their clippy configuration.

use std::env;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cargo_metadata::Message;
use cargo_metadata::diagnostic::Diagnostic;

/// Directories under `crates/` holding a simulation crate.
const SIMULATION_CRATES: [&str; 3] = ["math", "sim", "ecs"];

/// Directories under `crates/` holding a crate free to use the forbidden
/// items, such as a test harness running simulations on several threads.
const NON_SIMULATION_CRATES: [&str; 1] = ["test"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_owned()
}

fn configuration_of(crate_directory: &str) -> String {
    let path = workspace_root()
        .join("crates")
        .join(crate_directory)
        .join("clippy.toml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Extracts `(path, reason)` from each `{ path = "...", reason = "..." }`
/// entry, one entry per line.
fn forbidden_items(configuration: &str) -> Vec<(&str, &str)> {
    configuration
        .lines()
        .filter_map(|line| Some((string_value(line, "path")?, string_value(line, "reason")?)))
        .collect()
}

/// The string assigned to `key` in `line`, as in `key = "value"`.
fn string_value<'line>(line: &'line str, key: &str) -> Option<&'line str> {
    let prefix = format!("{key} = \"");
    let start = line.find(&prefix)? + prefix.len();
    let length = line[start..].find('"')?;
    Some(&line[start..start + length])
}

/// Runs clippy on the fixture with the configuration of `crate_directory`.
fn fixture_diagnostics(crate_directory: &str) -> Vec<Diagnostic> {
    let root = workspace_root();
    let mut child = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
        .args([
            "clippy",
            "--quiet",
            "--message-format=json",
            "--manifest-path",
        ])
        .arg(root.join("xtask/fixtures/determinism/Cargo.toml"))
        .arg("--target-dir")
        .arg(root.join("target/determinism-fixture"))
        .env("CLIPPY_CONF_DIR", root.join("crates").join(crate_directory))
        .current_dir(&root)
        .stdout(Stdio::piped())
        .spawn()
        .expect("cargo clippy starts");
    let stdout = child.stdout.take().expect("stdout is piped");
    let diagnostics = Message::parse_stream(BufReader::new(stdout))
        .filter_map(|message| match message.expect("cargo emits valid JSON") {
            Message::CompilerMessage(message) => Some(message.message),
            _ => None,
        })
        .collect();
    assert!(
        child.wait().expect("cargo clippy runs").success(),
        "the fixture must compile, only lint warnings are expected"
    );
    diagnostics
}

#[test]
fn every_crate_is_classified() {
    let unclassified: Vec<String> = fs::read_dir(workspace_root().join("crates"))
        .expect("crates/ is readable")
        .map(|entry| entry.expect("crates/ is readable").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| {
            !SIMULATION_CRATES.contains(&name.as_str())
                && !NON_SIMULATION_CRATES.contains(&name.as_str())
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "add each new crate to SIMULATION_CRATES, with a copy of clippy.toml, or to NON_SIMULATION_CRATES: {unclassified:?}"
    );
}

#[test]
fn simulation_crates_share_one_configuration() {
    let reference = configuration_of(SIMULATION_CRATES[0]);
    for crate_directory in &SIMULATION_CRATES[1..] {
        assert!(
            configuration_of(crate_directory) == reference,
            "crates/{crate_directory}/clippy.toml differs from crates/{}/clippy.toml",
            SIMULATION_CRATES[0]
        );
    }
}

#[test]
fn every_forbidden_item_is_reported_with_its_alternative() {
    let configuration = configuration_of(SIMULATION_CRATES[0]);
    let forbidden = forbidden_items(&configuration);
    assert!(!forbidden.is_empty(), "no forbidden item found");
    assert_eq!(
        forbidden.len(),
        configuration.matches("path = ").count(),
        "every entry must stay on one line to be checked"
    );

    let diagnostics = fixture_diagnostics(SIMULATION_CRATES[0]);

    let unreported: Vec<_> = forbidden
        .iter()
        .filter(|(path, reason)| {
            !diagnostics.iter().any(|diagnostic| {
                diagnostic.message.ends_with(&format!("`{path}`"))
                    && diagnostic
                        .children
                        .iter()
                        .any(|note| note.message == *reason)
            })
        })
        .map(|(path, _)| *path)
        .collect();
    assert!(
        unreported.is_empty(),
        "forbidden but not reported on the fixture (unknown path or unused in the fixture): {unreported:?}"
    );
}
