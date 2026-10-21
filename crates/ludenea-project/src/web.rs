//! `Project::build_web`: the game as a standalone static web page. The simulation runs in a
//! Web Worker driving the control `WorkerHost`, the page draws with `WebRenderer`, runs the
//! lobby with `WebLobby`, and captures keys and gamepads. The page's scripts are embedded here
//! as plain JavaScript modules, so building a game for the web needs no Node.js.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use ludenea_app::meta::TextureMeta;
use ludenea_app::project::{project_assets, project_textures};
use ludenea_control::{ClientMessage, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION};
use ludenea_ecs::AssetId;
use ludenea_render::atlas::{self, TextureSource};
use serde::Serialize;

use crate::Project;
use crate::build::{BuildError, BuildEvent, BuildTarget, cargo_build, generate};

/// The version of `game.json`, the page's game: its name, scene, and asset texts. The page
/// refuses any other.
pub const WEB_GAME_VERSION: u32 = 1;

/// The largest side of an atlas page in a web build: every WebGL2 device supports it.
const MAX_PAGE_SIDE: u32 = 2048;

/// What the build writes in the worker's script in place of this marker: the protocol hello
/// of this engine.
const HELLO_MARKER: &str = "\"__LUDENEA_HELLO__\"";

/// The page's own files.
const PAGE_FILES: [(&str, &str); 4] = [
    ("index.html", include_str!("../web/index.html")),
    ("page.js", include_str!("../web/page.js")),
    ("worker.js", include_str!("../web/worker.js")),
    ("style.css", include_str!("../web/style.css")),
];

/// `game.json`.
#[derive(Serialize)]
struct WebGame<'a> {
    version: u32,
    name: &'a str,
    /// The scene text.
    scene: &'a str,
    /// The prefab and data asset texts the scene may reach, as `[identifier, text]`.
    assets: Vec<(AssetId, &'a str)>,
}

impl Project {
    /// Builds the game as a standalone web page into `out` on a background thread, reporting
    /// on `output`; the last event is always `Finished`. Returns `out`.
    ///
    /// The game needs a `web` feature forwarding `ludenea/web`. Its textures are packed with
    /// pages of at most 2048 pixels, and one that cannot be read or packed fails the build.
    /// Everything is written into a staging folder beside `out` first, so a failed build
    /// leaves the previous `out` as it was; an existing `out` is replaced only if it is empty
    /// or a previous web build (it holds `game.json`).
    pub fn build_web(
        &self,
        out: &Path,
        output: Sender<BuildEvent>,
    ) -> JoinHandle<Result<PathBuf, BuildError>> {
        let root = self.root.clone();
        let name = self.manifest.name.clone();
        let scene = self.manifest.default_scene.clone();
        let out = out.to_path_buf();
        thread::spawn(move || {
            let result = build(&root, &name, scene.as_deref(), &out, &output);
            let _ = output.send(BuildEvent::Finished { ok: result.is_ok() });
            result
        })
    }
}

fn build(
    root: &Path,
    name: &str,
    scene: Option<&str>,
    out: &Path,
    output: &Sender<BuildEvent>,
) -> Result<PathBuf, BuildError> {
    check_web_feature(root)?;
    let scene = scene.ok_or(BuildError::NoScene)?;
    let out = std::path::absolute(out).map_err(at(out))?;
    check_replaceable(&out)?;
    let parent = out
        .parent()
        .ok_or_else(|| at(&out)(io::Error::other("it has no parent folder")))?;
    fs::create_dir_all(parent).map_err(at(parent))?;
    let staging = sibling(&out, "staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging).map_err(at(&staging))?;
    let written = write_page(root, name, &root.join(scene), &staging, output)
        .and_then(|()| replace(&staging, &out).map_err(at(&out)));
    if written.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    written?;
    let _ = output.send(BuildEvent::Line {
        text: format!("wrote the web page to {}", out.display()),
    });
    Ok(out)
}

/// The error of a file operation on `path`, naming it.
fn at(path: &Path) -> impl FnOnce(io::Error) -> BuildError {
    let path = path.to_path_buf();
    move |source| BuildError::File { path, source }
}

/// Refuses a game whose `Cargo.toml` has no `web` feature holding `ludenea/web`.
fn check_web_feature(root: &Path) -> Result<(), BuildError> {
    let path = root.join("Cargo.toml");
    let text = fs::read_to_string(&path).map_err(at(&path))?;
    let manifest: toml::Table = toml::from_str(&text).map_err(|error| {
        at(&path)(io::Error::new(
            io::ErrorKind::InvalidData,
            error.message().to_string(),
        ))
    })?;
    let forwards = manifest
        .get("features")
        .and_then(|features| features.get("web"))
        .and_then(toml::Value::as_array)
        .is_some_and(|web| web.iter().any(|item| item.as_str() == Some("ludenea/web")));
    if forwards {
        Ok(())
    } else {
        Err(BuildError::NoWebFeature)
    }
}

/// Refuses to replace an `out` that is a file or a folder holding something other than a
/// web build, which replacing would delete.
fn check_replaceable(out: &Path) -> Result<(), BuildError> {
    let Ok(metadata) = fs::symlink_metadata(out) else {
        return Ok(());
    };
    let refuse = || BuildError::NotAWebBuild(out.to_path_buf());
    if !metadata.is_dir() {
        return Err(refuse());
    }
    let empty = fs::read_dir(out).map_err(at(out))?.next().is_none();
    if empty || out.join("game.json").is_file() {
        Ok(())
    } else {
        Err(refuse())
    }
}

/// `.<name of out>.<what>-<process id>` beside `out`.
fn sibling(out: &Path, what: &str) -> PathBuf {
    let name = out
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    out.with_file_name(format!(".{name}.{what}-{}", std::process::id()))
}

/// Writes every file of the page into `dir`: the atlas and `game.json` first, which fail fast,
/// then the game's module and glue, then the page's own files.
fn write_page(
    root: &Path,
    name: &str,
    scene: &Path,
    dir: &Path,
    output: &Sender<BuildEvent>,
) -> Result<(), BuildError> {
    write_atlas(root, scene, &dir.join("atlas"), output)?;
    write_game(name, scene, root, dir)?;
    let module = cargo_build(root, name, BuildTarget::Wasm, &["web"], "web", output)?;
    generate(&module, dir)?;
    let hello = ClientMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        protocol_fingerprint: PROTOCOL_FINGERPRINT,
    };
    let hello = serde_json::to_string(&hello).expect("a hello serializes");
    for (file, text) in PAGE_FILES {
        let text = text.replace(HELLO_MARKER, &hello);
        let path = dir.join(file);
        fs::write(&path, text).map_err(at(&path))?;
    }
    Ok(())
}

/// Packs the project's textures and writes `atlas.json` and its pages into `dir`.
fn write_atlas(
    root: &Path,
    scene: &Path,
    dir: &Path,
    output: &Sender<BuildEvent>,
) -> Result<(), BuildError> {
    let found =
        project_textures(scene, Some(root)).map_err(|error| BuildError::Textures(vec![error]))?;
    let mut errors = found.warnings;
    let relative = |path: &str| -> String {
        Path::new(path).strip_prefix(root).map_or_else(
            |_| path.to_string(),
            |path| path.to_string_lossy().into_owned(),
        )
    };
    let named: Vec<(AssetId, String, &[u8], &TextureMeta)> = found
        .textures
        .iter()
        .map(|(id, path, png, meta)| (*id, relative(path), png.as_slice(), meta))
        .collect();
    let sources: Vec<TextureSource<'_>> = named
        .iter()
        .map(|(id, path, png, meta)| TextureSource {
            id: *id,
            path: Some(path.as_str()),
            png,
            meta,
        })
        .collect();
    let packed = atlas::pack(&sources, MAX_PAGE_SIDE);
    errors.extend(packed.errors.iter().map(ToString::to_string));
    if !errors.is_empty() {
        return Err(BuildError::Textures(errors));
    }
    let files = packed.to_files();
    fs::create_dir(dir).map_err(at(dir))?;
    let manifest = dir.join("atlas.json");
    fs::write(&manifest, files.manifest).map_err(at(&manifest))?;
    for (index, page) in files.pages.iter().enumerate() {
        let path = dir.join(format!("page-{index}.png"));
        fs::write(&path, page).map_err(at(&path))?;
    }
    let _ = output.send(BuildEvent::Line {
        text: format!(
            "packed {} into {} of at most {MAX_PAGE_SIDE} pixels a side",
            counted(sources.len(), "texture"),
            counted(files.pages.len(), "atlas page")
        ),
    });
    Ok(())
}

/// `count` and `thing`, plural unless `count` is 1.
fn counted(count: usize, thing: &str) -> String {
    if count == 1 {
        format!("1 {thing}")
    } else {
        format!("{count} {thing}s")
    }
}

/// Writes `game.json`: the scene text and the asset texts it may reach.
fn write_game(name: &str, scene: &Path, root: &Path, dir: &Path) -> Result<(), BuildError> {
    let text = fs::read_to_string(scene)
        .map_err(|error| BuildError::Assets(format!("{}: {error}", scene.display())))?;
    let assets = project_assets(scene, Some(root)).map_err(BuildError::Assets)?;
    let game = WebGame {
        version: WEB_GAME_VERSION,
        name,
        scene: &text,
        assets: assets
            .texts
            .iter()
            .map(|(id, text)| (*id, text.as_str()))
            .collect(),
    };
    let json = serde_json::to_string(&game).expect("the game serializes");
    let path = dir.join("game.json");
    fs::write(&path, json).map_err(at(&path))?;
    Ok(())
}

/// Puts `staging` in place of `out`: the old `out` is moved aside first and removed once
/// `staging` took its place, or moved back if it could not.
fn replace(staging: &Path, out: &Path) -> io::Result<()> {
    if fs::symlink_metadata(out).is_err() {
        return fs::rename(staging, out);
    }
    let old = sibling(out, "old");
    let _ = fs::remove_dir_all(&old);
    fs::rename(out, &old)?;
    if let Err(error) = fs::rename(staging, out) {
        let _ = fs::rename(&old, out);
        return Err(error);
    }
    // Best effort: the old build left behind only takes space.
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_file_holds_no_marker_but_the_worker() {
        for (file, text) in PAGE_FILES {
            assert_eq!(
                text.contains(HELLO_MARKER),
                file == "worker.js",
                "{file} and the hello marker"
            );
        }
    }

    #[test]
    fn replace_swaps_a_previous_build_and_leaves_nothing_beside() {
        let dir = tempfile::TempDir::new().unwrap();
        let out = dir.path().join("site");
        let staging = sibling(&out, "staging");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("game.json"), "old").unwrap();
        fs::write(out.join("stale.js"), "old").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("game.json"), "new").unwrap();
        replace(&staging, &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("game.json")).unwrap(), "new");
        assert!(!out.join("stale.js").exists());
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn only_an_empty_folder_or_a_web_build_is_replaceable() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(check_replaceable(&dir.path().join("missing")).is_ok());
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(check_replaceable(&empty).is_ok());
        let build = dir.path().join("build");
        fs::create_dir(&build).unwrap();
        fs::write(build.join("game.json"), "{}").unwrap();
        assert!(check_replaceable(&build).is_ok());
        let other = dir.path().join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("notes.txt"), "keep").unwrap();
        assert!(matches!(
            check_replaceable(&other),
            Err(BuildError::NotAWebBuild(_))
        ));
        let file = dir.path().join("file");
        fs::write(&file, "keep").unwrap();
        assert!(matches!(
            check_replaceable(&file),
            Err(BuildError::NotAWebBuild(_))
        ));
    }
}
