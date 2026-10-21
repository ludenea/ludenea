//! `ludenea build --web`. The refusals come before any cargo build, so these run with the
//! workspace tests; a full build is tested in `ludenea-project`'s web tests and the editor's
//! end-to-end tests.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use ludenea_project::Project;
use tempfile::TempDir;

fn new_project(dir: &TempDir) -> Project {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Project::create(dir.path(), "demo", &engine).expect("the project is created")
}

fn ludenea(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludenea"))
        .args(args)
        .output()
        .expect("ludenea runs")
}

#[test]
fn build_of_a_game_without_the_web_feature_exits_2_naming_the_line() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let manifest = project.root().join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replace("[features]\nweb = [\"ludenea/web\"]\n", ""),
    )
    .unwrap();
    let out = dir.path().join("site");
    let output = ludenea(&[
        "build",
        project.root().to_str().unwrap(),
        "--web",
        "--out",
        out.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("error: the game has no web feature: add `web = [\"ludenea/web\"]`"),
        "{stderr}"
    );
    assert!(!out.exists());
}

#[test]
fn build_needs_the_web_flag() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let out = dir.path().join("site");
    let output = ludenea(&[
        "build",
        project.root().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--web"), "{stderr}");
    assert!(!out.exists());
}
