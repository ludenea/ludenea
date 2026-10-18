use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use ludenea_project::{AssetKind, FileChange, Project, ProjectError};
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

/// The template's toolchain is the engine's, read when the project is created.
#[test]
fn create_uses_the_engine_toolchain() {
    let dir = TempDir::new().unwrap();
    new_project(&dir);
    let toolchain = |path: &Path| -> toml::Table {
        toml::from_str(&fs::read_to_string(path.join("rust-toolchain.toml")).unwrap()).unwrap()
    };
    assert_eq!(
        toolchain(&dir.path().join("demo"))["toolchain"]["channel"],
        toolchain(&engine_path())["toolchain"]["channel"]
    );
}

/// A quote or a backslash in the engine path must not break the generated `Cargo.toml`.
#[cfg(unix)]
#[test]
fn engine_path_with_quotes_stays_valid_toml() {
    let dir = TempDir::new().unwrap();
    let engine = dir.path().join("my \"engine\" \\ copy");
    fs::create_dir_all(&engine).unwrap();
    for file in ["crates", "vendor", "rust-toolchain.toml"] {
        std::os::unix::fs::symlink(
            engine_path().join(file).canonicalize().unwrap(),
            engine.join(file),
        )
        .unwrap();
    }
    let project = Project::create(dir.path(), "demo", &engine).unwrap();
    let cargo: toml::Table =
        toml::from_str(&fs::read_to_string(project.root().join("Cargo.toml")).unwrap()).unwrap();
    let ludenea = cargo["dependencies"]["ludenea"]["path"].as_str().unwrap();
    assert_eq!(
        Path::new(ludenea),
        engine.canonicalize().unwrap().join("crates/ludenea")
    );
}

#[test]
fn create_refuses_a_directory_that_is_not_the_engine() {
    let dir = TempDir::new().unwrap();
    let error = Project::create(dir.path(), "demo", dir.path()).unwrap_err();
    assert!(matches!(error, ProjectError::NotAnEngine(_)), "{error:?}");
    assert!(!dir.path().join("demo").exists());
}

/// A failure halfway leaves nothing behind, so creating again with a fixed engine works.
#[test]
fn failed_create_leaves_no_directory() {
    let dir = TempDir::new().unwrap();
    let engine = dir.path().join("engine");
    fs::create_dir_all(engine.join("crates/ludenea")).unwrap();
    fs::write(engine.join("crates/ludenea/Cargo.toml"), "").unwrap();
    // No rust-toolchain.toml: the template cannot be written.
    assert!(Project::create(dir.path(), "demo", &engine).is_err());
    assert!(!dir.path().join("demo").exists());
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

/// A reader holding the old file keeps the old text: the new text replaced the file in one
/// rename instead of being written over it, so a crash mid-write cannot leave half a scene.
#[cfg(unix)]
#[test]
fn write_text_replaces_the_file_in_one_step() {
    use std::io::Read;

    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    let scene = project.root().join("scenes/main.scene.ron");
    let old = fs::read_to_string(&scene).unwrap();
    let mut reader = fs::File::open(&scene).unwrap();
    project
        .write_text("scenes/main.scene.ron", "new text")
        .unwrap();
    let mut held = String::new();
    reader.read_to_string(&mut held).unwrap();
    assert_eq!(held, old);
    assert_eq!(fs::read_to_string(&scene).unwrap(), "new text");
    let scenes: Vec<_> = fs::read_dir(project.root().join("scenes"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(scenes, ["main.scene.ron"], "no temporary file is left");
    let leftovers = fs::read_dir(project.root().join(".ludenea"))
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().path().is_file())
        .count();
    assert_eq!(leftovers, 0, "no temporary file is left");
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
        &FileChange::Changed {
            path: "scenes/main.scene.ron".into(),
            kind: AssetKind::of("scenes/main.scene.ron"),
        },
    );
}

/// Only the meta makes a `.ron` file a data asset, and its changes say so.
#[test]
fn watch_reports_data_asset_changes_with_their_kind() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    fs::create_dir_all(project.root().join("assets")).unwrap();
    fs::write(project.root().join("assets/settings.ron"), "()").unwrap();
    fs::write(
        project.root().join("assets/settings.ron.meta"),
        "Meta(version: 1, id: \"0000000000000000000000000000002a\", kind: Data(type: \"demo::Settings\"))",
    )
    .unwrap();
    let changes = project.watch().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    fs::write(
        project.root().join("assets/settings.ron"),
        "(changed: true)",
    )
    .unwrap();
    let kind = AssetKind::Data {
        r#type: "demo::Settings".to_string(),
    };
    let path = "assets/settings.ron".to_string();
    expect_change(
        &changes,
        &FileChange::Changed {
            path: path.clone(),
            kind: kind.clone(),
        },
    );
    fs::remove_file(project.root().join("assets/settings.ron")).unwrap();
    fs::remove_file(project.root().join("assets/settings.ron.meta")).unwrap();
    expect_change(&changes, &FileChange::Removed { path, kind });
}

/// The changes that arrive until none has come for a second.
fn settled_changes(changes: &Receiver<FileChange>) -> Vec<FileChange> {
    std::iter::from_fn(|| changes.recv_timeout(Duration::from_secs(1)).ok()).collect()
}

fn path_of(change: &FileChange) -> &str {
    match change {
        FileChange::Changed { path, .. }
        | FileChange::Created { path, .. }
        | FileChange::Removed { path, .. } => path,
    }
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
    let scene_changes: Vec<_> = settled_changes(&changes)
        .into_iter()
        .filter(|change| path_of(change) == "scenes/main.scene.ron")
        .collect();
    assert_eq!(
        scene_changes,
        [FileChange::Changed {
            path: "scenes/main.scene.ron".into(),
            kind: AssetKind::of("scenes/main.scene.ron")
        }],
        "never a removal or a creation of the saved file"
    );
}

#[test]
fn removing_then_recreating_a_file_is_reported_in_order() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let changes = project.watch().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let scene = project.root().join("scenes/main.scene.ron");
    fs::remove_file(&scene).unwrap();
    expect_change(
        &changes,
        &FileChange::Removed {
            path: "scenes/main.scene.ron".into(),
            kind: AssetKind::of("scenes/main.scene.ron"),
        },
    );
    fs::write(&scene, "back").unwrap();
    expect_change(
        &changes,
        &FileChange::Created {
            path: "scenes/main.scene.ron".into(),
            kind: AssetKind::of("scenes/main.scene.ron"),
        },
    );
}

/// Metas, version control, installed packages, and build outputs are not the user's files.
#[test]
fn watch_ignores_metas_git_node_modules_and_target() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    project.files();
    let changes = project.watch().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let root = project.root();
    for (path, text) in [
        (".git/HEAD", "ref: refs/heads/main"),
        ("editor/node_modules/react/index.js", "export {};"),
        ("target/debug/demo", "binary"),
        (
            "scenes/main.scene.ron.meta",
            "Meta(version: 1, id: \"0\", kind: Scene)",
        ),
        ("scenes/sentinel.scene.ron", "(entities: [])"),
    ] {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), text).unwrap();
    }
    let seen = settled_changes(&changes);
    let paths: Vec<_> = seen.iter().map(path_of).collect();
    assert_eq!(paths, ["scenes/sentinel.scene.ron"], "{seen:?}");
}

/// A directory created at the top of the project after watching started is watched too.
#[test]
fn watch_follows_new_top_level_directories() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let changes = project.watch().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    fs::create_dir_all(project.root().join("levels/one")).unwrap();
    fs::write(project.root().join("levels/one/a.txt"), "a").unwrap();
    expect_change(
        &changes,
        &FileChange::Created {
            path: "levels/one/a.txt".into(),
            kind: AssetKind::of("levels/one/a.txt"),
        },
    );
    std::thread::sleep(Duration::from_millis(500));
    fs::write(project.root().join("levels/one/a.txt"), "changed").unwrap();
    expect_change(
        &changes,
        &FileChange::Changed {
            path: "levels/one/a.txt".into(),
            kind: AssetKind::of("levels/one/a.txt"),
        },
    );
}

/// A directory created inside `editor/` after watching started is watched too (`editor/` is
/// watched apart from its installed packages).
#[test]
fn watch_follows_new_directories_in_the_editor_directory() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    fs::create_dir_all(project.root().join("editor/node_modules")).unwrap();
    let changes = project.watch().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    fs::create_dir_all(project.root().join("editor/panels")).unwrap();
    fs::write(project.root().join("editor/panels/a.ts"), "a").unwrap();
    expect_change(
        &changes,
        &FileChange::Created {
            path: "editor/panels/a.ts".into(),
            kind: AssetKind::of("editor/panels/a.ts"),
        },
    );
    std::thread::sleep(Duration::from_millis(500));
    fs::write(project.root().join("editor/panels/a.ts"), "changed").unwrap();
    expect_change(
        &changes,
        &FileChange::Changed {
            path: "editor/panels/a.ts".into(),
            kind: AssetKind::of("editor/panels/a.ts"),
        },
    );
}

#[cfg(unix)]
#[test]
fn write_text_keeps_the_permissions_of_the_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    let scene = project.root().join("scenes/main.scene.ron");
    fs::set_permissions(&scene, fs::Permissions::from_mode(0o640)).unwrap();
    project.write_text("scenes/main.scene.ron", "new").unwrap();
    let mode = fs::metadata(&scene).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

/// Writing through a symbolic link inside the project replaces its target and keeps the link.
#[cfg(unix)]
#[test]
fn write_text_writes_through_a_symbolic_link() {
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    let link = project.root().join("scenes/link.scene.ron");
    std::os::unix::fs::symlink("main.scene.ron", &link).unwrap();
    project
        .write_text("scenes/link.scene.ron", "through")
        .unwrap();
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(
        project.read_text("scenes/main.scene.ron").unwrap(),
        "through"
    );
    assert_eq!(
        project.read_text("scenes/link.scene.ron").unwrap(),
        "through"
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
    let hash = output
        .trim()
        .strip_prefix("tick 60 hash ")
        .unwrap_or_else(|| panic!("the run prints a hash: {output:?}"));
    assert_eq!(hash.len(), 16, "{hash}");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
}

#[cfg(unix)]
#[test]
fn symlinks_leading_outside_the_project_are_rejected() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    let mut project = new_project(&dir);
    // The template's own assets make way for the link.
    fs::remove_dir_all(project.root().join("assets")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.root().join("assets")).unwrap();

    let is_outside = |result: Result<(), ProjectError>| {
        assert!(
            matches!(result, Err(ProjectError::OutsideProject(_))),
            "{result:?}"
        );
    };
    assert!(matches!(
        project.read_text("assets/secret.txt"),
        Err(ProjectError::OutsideProject(_))
    ));
    is_outside(project.write_text("assets/new.txt", "x"));
    is_outside(project.delete("assets/secret.txt"));
    is_outside(project.rename("scenes/main.scene.ron", "assets/moved.ron"));
    is_outside(project.rename("assets/secret.txt", "scenes/secret.txt"));
    assert!(outside.path().join("secret.txt").is_file());
    assert!(!outside.path().join("new.txt").exists());
}
