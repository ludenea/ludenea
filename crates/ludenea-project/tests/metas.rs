use std::fs;
use std::path::Path;

use ludenea_project::{AssetKind, Project, ProjectError, ProjectWarning};
use tempfile::TempDir;

fn new_project(dir: &TempDir) -> Project {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Project::create(dir.path(), "demo", &engine).expect("the project is created")
}

#[test]
fn listing_creates_missing_metas_and_keeps_ids_stable() {
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    project.write_text("assets/note.txt", "hello").unwrap();
    let first = project.files();
    let paths: Vec<_> = first.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["assets/note.txt", "scenes/main.scene.ron"]);
    assert_eq!(first[0].kind, AssetKind::Other);
    assert_eq!(first[1].kind, AssetKind::Scene);
    assert_ne!(first[0].id, first[1].id);
    assert!(dir.path().join("demo/assets/note.txt.meta").is_file());
    assert!(dir.path().join("demo/scenes/main.scene.ron.meta").is_file());
    assert_eq!(project.files(), first);
}

#[test]
fn rename_moves_meta_with_file() {
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    let before = project.files()[0].clone();
    project
        .rename("scenes/main.scene.ron", "scenes/level/one.scene.ron")
        .unwrap();
    let after = project.files();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].path, "scenes/level/one.scene.ron");
    assert_eq!(after[0].id, before.id);
    assert!(!dir.path().join("demo/scenes/main.scene.ron.meta").exists());
    assert!(project.take_warnings().is_empty());
}

#[test]
fn delete_removes_meta_with_file() {
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    project.files();
    project.delete("scenes/main.scene.ron").unwrap();
    assert!(project.files().is_empty());
    assert!(!dir.path().join("demo/scenes/main.scene.ron").exists());
    assert!(!dir.path().join("demo/scenes/main.scene.ron.meta").exists());
    assert!(project.take_warnings().is_empty());
    assert!(matches!(
        project.delete("scenes/main.scene.ron"),
        Err(ProjectError::Io { .. })
    ));
}

#[test]
fn file_moved_outside_editor_gets_new_id_and_orphan_warning_once() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let old_id = project.files()[0].id;
    let root = dir.path().join("demo");
    fs::rename(
        root.join("scenes/main.scene.ron"),
        root.join("scenes/moved.scene.ron"),
    )
    .unwrap();

    let after = project.files();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].path, "scenes/moved.scene.ron");
    assert_ne!(after[0].id, old_id);
    assert_eq!(
        project.take_warnings(),
        [ProjectWarning::OrphanMeta {
            id: old_id,
            path: "scenes/main.scene.ron".into()
        }]
    );

    project.files();
    assert!(project.take_warnings().is_empty(), "reported once");
    assert!(
        root.join("scenes/main.scene.ron.meta").is_file(),
        "never deleted"
    );
}
