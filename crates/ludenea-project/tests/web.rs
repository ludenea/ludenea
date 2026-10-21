//! `Project::build_web`. The tests that build run real `cargo build`s of a generated project for
//! the browser, so every test here is `#[ignore]`d with the other build tests; run them with
//! `cargo test -p ludenea-project --test web -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc;

use ludenea_project::{BuildError, BuildEvent, Project, WEB_GAME_VERSION};
use tempfile::TempDir;

const SWARM_ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/swarm/assets");

fn new_project(dir: &TempDir) -> Project {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Project::create(dir.path(), "demo", &engine).expect("the project is created")
}

/// A project holding the swarm's bug texture, so its atlas has a page.
fn project_with_texture(dir: &TempDir) -> Project {
    let mut project = new_project(dir);
    for file in ["bug.png", "bug.png.meta"] {
        let bytes = fs::read(Path::new(SWARM_ASSETS).join(file)).unwrap();
        fs::write(project.root().join("assets").join(file), bytes).unwrap();
    }
    // The project lists its files afresh.
    project = Project::open(project.root()).unwrap();
    project
}

fn build_web(project: &Project, out: &Path) -> (Result<PathBuf, BuildError>, Vec<BuildEvent>) {
    let (sender, receiver) = mpsc::channel();
    let handle = project.build_web(out, sender);
    let result = handle.join().expect("the build thread does not panic");
    (result, receiver.try_iter().collect())
}

/// One project built for the web once, shared by the tests that read its page.
fn built() -> &'static (TempDir, PathBuf) {
    static BUILT: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = TempDir::new().unwrap();
        let project = project_with_texture(&dir);
        let out = dir.path().join("site");
        let (result, events) = build_web(&project, &out);
        assert_eq!(result.as_deref().ok(), Some(out.as_path()), "{result:?}");
        assert!(matches!(
            events.last(),
            Some(BuildEvent::Finished { ok: true })
        ));
        (dir, out)
    })
}

/// Every file under `dir`, relative to it with `/` separators, sorted.
fn files(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
#[ignore]
fn web_build_writes_a_complete_page() {
    let (dir, out) = built();
    assert_eq!(
        files(out),
        [
            "atlas/atlas.json",
            "atlas/page-0.png",
            "game.js",
            "game.json",
            "game_bg.wasm",
            "index.html",
            "page.js",
            "style.css",
            "worker.js",
        ]
    );
    let game: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("game.json")).unwrap()).unwrap();
    assert_eq!(game["version"], WEB_GAME_VERSION);
    assert_eq!(game["name"], "demo");
    let scene = fs::read_to_string(dir.path().join("demo/scenes/main.scene.ron")).unwrap();
    assert_eq!(game["scene"], scene);
    // The template's prefab and data asset, as [identifier, text] pairs.
    let assets = game["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2, "{assets:?}");
    assert!(
        assets
            .iter()
            .all(|pair| pair[0].is_string() && pair[1].is_string())
    );
    let atlas = fs::read_to_string(out.join("atlas/atlas.json")).unwrap();
    assert!(
        atlas.contains("00000000000000000000000000005b01"),
        "{atlas}"
    );
    // The game's glue exports the worker's host, the renderer, and the lobby.
    let glue = fs::read_to_string(out.join("game.js")).unwrap();
    for export in [
        "createHost",
        "WebRenderer",
        "WebLobby",
        "automaticClaimsJson",
    ] {
        assert!(glue.contains(export), "game.js does not export {export}");
    }
    // Nothing is left beside the page.
    let siblings: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        siblings
            .iter()
            .filter(|name| name.as_str() != "demo")
            .count(),
        1,
        "{siblings:?}"
    );
}

#[test]
#[ignore]
fn web_build_is_reproducible_but_for_the_wasm() {
    let (dir, first) = built();
    let project = Project::open(&dir.path().join("demo")).unwrap();
    let second = TempDir::new().unwrap();
    let second = second.path().join("again");
    let (result, _) = build_web(&project, &second);
    result.unwrap();
    for file in [
        "game.json",
        "atlas/atlas.json",
        "atlas/page-0.png",
        "index.html",
        "page.js",
        "worker.js",
        "style.css",
    ] {
        assert!(
            fs::read(first.join(file)).unwrap() == fs::read(second.join(file)).unwrap(),
            "{file} differs between two builds"
        );
    }
}

#[test]
#[ignore]
fn web_build_without_the_feature_names_the_line_to_add() {
    let dir = TempDir::new().unwrap();
    let mut project = new_project(&dir);
    // A project made before games had a web feature.
    let manifest = project.read_text("Cargo.toml").unwrap();
    let old = manifest.replace("[features]\nweb = [\"ludenea/web\"]\n", "");
    assert_ne!(old, manifest, "the template declares the web feature");
    project.write_text("Cargo.toml", &old).unwrap();
    let out = dir.path().join("site");
    let (result, events) = build_web(&project, &out);
    let Err(error) = result else {
        panic!("a game without the web feature is built: {result:?}");
    };
    assert!(matches!(error, BuildError::NoWebFeature), "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("[features]") && message.contains("web = [\"ludenea/web\"]"),
        "{message}"
    );
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished { ok: false })
    ));
    assert!(!out.exists());
}

#[test]
#[ignore]
fn broken_texture_fails_the_web_build_listing_it() {
    let dir = TempDir::new().unwrap();
    let mut project = project_with_texture(&dir);
    let texture_meta = |id: &str| {
        format!(
            "Meta(version: 2, id: \"{id}\", kind: Texture((pixels_per_unit: 16, grid: None, pivot: None, filter: Nearest)))\n"
        )
    };
    for (file, id) in [
        ("assets/broken.png", "000000000000000000000000000000b1"),
        ("assets/also-broken.png", "000000000000000000000000000000b2"),
    ] {
        fs::write(project.root().join(file), b"not a png").unwrap();
        fs::write(
            project.root().join(format!("{file}.meta")),
            texture_meta(id),
        )
        .unwrap();
    }
    project = Project::open(project.root()).unwrap();
    // A previous build, which a failed one leaves as it was.
    let out = dir.path().join("site");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("game.json"), "previous").unwrap();
    let (result, events) = build_web(&project, &out);
    let Err(BuildError::Textures(errors)) = &result else {
        panic!("{result:?}");
    };
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].contains("broken.png"), "{errors:?}");
    assert!(errors[1].contains("also-broken.png"), "{errors:?}");
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("broken.png") && message.contains("also-broken.png"),
        "{message}"
    );
    assert!(matches!(
        events.last(),
        Some(BuildEvent::Finished { ok: false })
    ));
    assert_eq!(files(&out), ["game.json"]);
    assert_eq!(
        fs::read_to_string(out.join("game.json")).unwrap(),
        "previous"
    );
    // No staging folder is left behind.
    let mut siblings: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    siblings.sort();
    assert_eq!(siblings, ["demo", "site"]);
}

#[test]
#[ignore]
fn web_build_refuses_to_replace_a_folder_that_is_not_a_build() {
    let dir = TempDir::new().unwrap();
    let project = new_project(&dir);
    let out = dir.path().join("documents");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("notes.txt"), "keep me").unwrap();
    let (result, _) = build_web(&project, &out);
    let Err(error) = result else {
        panic!("a folder that is not a web build is replaced");
    };
    assert!(matches!(error, BuildError::NotAWebBuild(_)), "{error:?}");
    assert!(error.to_string().contains("documents"), "{error}");
    assert_eq!(
        fs::read_to_string(out.join("notes.txt")).unwrap(),
        "keep me"
    );
}
