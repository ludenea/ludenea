//! Most of these tests run real `cargo build`s of a generated project, so they are `#[ignore]`d;
//! run them with `cargo test -p ludenea-serve --test play -- --ignored`.

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
#[ignore]
fn play_builds_the_template_and_forwards_arguments() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let output = ludenea(&["play", project.root().to_str().unwrap(), "--", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout.contains("--players"), "{stdout}");
    assert!(stdout.contains("--fullscreen"), "{stdout}");
}

#[test]
fn play_without_a_scene_names_both_fixes() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let manifest = project.root().join("ludenea.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    let text: String = text
        .lines()
        .filter(|line| !line.starts_with("default_scene"))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&manifest, text).unwrap();
    let output = ludenea(&["play", project.root().to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr
            .contains("error: no scene to play: pass --scene or set default_scene in ludenea.toml"),
        "{stderr}"
    );
}

#[test]
#[ignore]
fn play_of_a_broken_build_exits_2_with_its_diagnostic() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    fs::write(project.root().join("src/lib.rs"), "fn broken( {").unwrap();
    let output = ludenea(&["play", project.root().to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("src/lib.rs"), "{stderr}");
    assert!(
        stderr.contains("this file contains an unclosed delimiter"),
        "{stderr}"
    );
}
