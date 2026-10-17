use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use ludenea_project::{FileChange, Project, ProjectError};
use tempfile::TempDir;

fn engine_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn new_project(dir: &TempDir) -> Project {
    Project::create(dir.path(), "demo", &engine_path()).expect("the project is created")
}

/// Waits until `wanted` arrives (other changes are skipped) or fails after a generous timeout.
fn expect_change(changes: &Receiver<FileChange>, wanted: &FileChange) {
    let mut seen = Vec::new();
    while let Ok(change) = changes.recv_timeout(Duration::from_secs(15)) {
        if &change == wanted {
            return;
        }
        seen.push(change);
    }
    panic!("never saw {wanted:?}; saw {seen:?}");
}

#[test]
fn create_writes_template_that_opens() {
    let dir = TempDir::new().unwrap();
    let created = new_project(&dir);
    let root = dir.path().join("demo");
    for file in [
        "ludenea.toml",
        "Cargo.toml",
        "rust-toolchain.toml",
        "src/lib.rs",
        "src/main.rs",
        "scenes/main.scene.ron",
        "editor/package.json",
        "editor/index.ts",
        "editor/tsconfig.json",
        ".gitignore",
    ] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("crate-type = [\"cdylib\", \"rlib\"]"));
    assert!(cargo.contains("/crates/ludenea\""));
    let opened = Project::open(&root).unwrap();
    assert_eq!(opened.manifest().name, "demo");
    assert_eq!(
        opened.manifest().default_scene.as_deref(),
        Some("scenes/main.scene.ron")
    );
    assert_eq!(opened.root(), created.root());
}

#[test]
fn invalid_name_is_rejected_with_rule() {
    let dir = TempDir::new().unwrap();
    for name in ["Demo", "1demo", "my-game", "", "a b"] {
        let error = Project::create(dir.path(), name, &engine_path()).unwrap_err();
        assert!(matches!(error, ProjectError::InvalidName(_)), "{name}");
        assert!(error.to_string().contains("^[a-z][a-z0-9_]*$"));
    }
}

#[test]
fn paths_outside_project_are_rejected() {
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    for path in ["../secret.txt", "scenes/../../x", "/etc/passwd"] {
        assert!(matches!(
            project.read_text(path),
            Err(ProjectError::OutsideProject(_))
        ));
        assert!(matches!(
            project.write_text(path, "x"),
            Err(ProjectError::OutsideProject(_))
        ));
        assert!(matches!(
            project.delete(path),
            Err(ProjectError::OutsideProject(_))
        ));
        assert!(matches!(
            project.rename("scenes/main.scene.ron", path),
            Err(ProjectError::OutsideProject(_))
        ));
    }
}

#[test]
fn watch_reports_scene_change_on_disk() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let changes = project.watch().unwrap();
    // Let the watcher settle, then write in place.
    std::thread::sleep(Duration::from_millis(500));
    fs::write(project.root().join("scenes/main.scene.ron"), "changed").unwrap();
    expect_change(
        &changes,
        &FileChange::Changed("scenes/main.scene.ron".into()),
    );
}

#[test]
fn atomic_save_is_reported_as_changed() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let changes = project.watch().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let scene = project.root().join("scenes/main.scene.ron");
    let temporary = project.root().join("scenes/.main.scene.ron.tmp");
    fs::write(&temporary, "saved atomically").unwrap();
    fs::rename(&temporary, &scene).unwrap();
    expect_change(
        &changes,
        &FileChange::Changed("scenes/main.scene.ron".into()),
    );
}

#[test]
#[ignore = "runs a real cargo build"]
fn template_builds_and_runs() {
    let dir = TempDir::new_in(engine_path().join("target")).unwrap();
    let project = new_project(&dir);
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO"))
            .args(args)
            .current_dir(project.root())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cargo {args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    run(&["build"]);
    let output = run(&[
        "run",
        "--",
        "run",
        "scenes/main.scene.ron",
        "--ticks",
        "60",
        "--hash",
    ]);
    assert!(!output.trim().is_empty(), "the run prints a hash");
}
