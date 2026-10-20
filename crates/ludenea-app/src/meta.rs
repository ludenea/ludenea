//! The `<file>.meta` sidecar that gives a project file a stable identifier and a kind.

use std::collections::BTreeMap;
use std::fs;
#[cfg(not(target_family = "wasm"))]
use std::fs::OpenOptions;
#[cfg(not(target_family = "wasm"))]
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use ludenea_ecs::AssetId;
use serde::{Deserialize, Serialize};

pub const META_VERSION: u32 = 2;

/// What a project file is. Only a meta makes a `.ron` file a data asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub enum AssetKind {
    Scene,
    Prefab,
    Data { r#type: String },
    Texture(TextureMeta),
    Replay,
    Trace,
    Snapshot,
    Other,
}

impl AssetKind {
    /// The kind a file's extension gives it; never `Data`, which only a meta declares.
    pub fn of(path: &str) -> Self {
        let kinds = [
            (".scene.ron", Self::Scene),
            (".prefab.ron", Self::Prefab),
            (".replay", Self::Replay),
            (".trace", Self::Trace),
            (".snap", Self::Snapshot),
        ];
        if is_png(path) {
            return Self::Texture(TextureMeta::default());
        }
        kinds
            .into_iter()
            .find(|(extension, _)| path.ends_with(extension))
            .map_or(Self::Other, |(_, kind)| kind)
    }
}

/// Whether `path` names a PNG, whatever the case of its extension.
pub fn is_png(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".png")
}

/// How a PNG is cut into frames and drawn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct TextureMeta {
    /// How many pixels span one world unit.
    pub pixels_per_unit: u32,
    /// The frame grid; `None` makes the whole image one frame.
    pub grid: Option<Grid>,
    /// The pivot in pixels from the frame's top-left corner; `None` is the frame's centre.
    pub pivot: Option<(u32, u32)>,
    pub filter: Filter,
}

impl Default for TextureMeta {
    fn default() -> Self {
        Self {
            pixels_per_unit: 16,
            grid: None,
            pivot: None,
            filter: Filter::Nearest,
        }
    }
}

impl TextureMeta {
    /// Refuses a meta no image can be cut by, naming the field.
    pub fn validate(&self) -> Result<(), String> {
        let fields = [
            ("pixels_per_unit", Some(self.pixels_per_unit)),
            (
                "grid.frame_width",
                self.grid.as_ref().map(|grid| grid.frame_width),
            ),
            (
                "grid.frame_height",
                self.grid.as_ref().map(|grid| grid.frame_height),
            ),
        ];
        match fields.into_iter().find(|(_, value)| *value == Some(0)) {
            Some((field, _)) => Err(format!("{field} must be at least 1")),
            None => Ok(()),
        }
    }
}

/// A grid of equal frames, `margin` pixels in from the image edge and `spacing` pixels apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct Grid {
    pub frame_width: u32,
    pub frame_height: u32,
    pub margin: u32,
    pub spacing: u32,
}

/// How a texture is sampled when scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub enum Filter {
    Nearest,
    Linear,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub version: u32,
    pub id: AssetId,
    pub kind: AssetKind,
}

/// How long a lister waits for another one to finish writing a meta it just created.
const CREATION_GRACE: Duration = Duration::from_millis(50);
/// An empty meta modified longer ago than this is not being written: it is just empty. Wide
/// enough for file systems that keep modification times to 1 or 2 seconds (HFS+, FAT, exFAT).
const CREATION_WINDOW: Duration = Duration::from_secs(3);

/// Whether the meta at `path` was modified so recently that another lister may still be
/// writing it.
fn may_be_being_written(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|age| age < CREATION_WINDOW)
}

impl Meta {
    /// A meta of `kind` with a new random identifier.
    #[cfg(not(target_family = "wasm"))]
    pub fn new(kind: AssetKind) -> Meta {
        Meta {
            version: META_VERSION,
            id: AssetId(uuid::Uuid::new_v4().as_u128()),
            kind,
        }
    }

    /// The texture meta of the file `path` this meta belongs to, if that file is a texture:
    /// a PNG whose meta is a texture meta, or `Other` (as a meta written before textures had
    /// settings is), which reads as the default texture meta. A texture meta on any other
    /// file makes no texture. The editor listing and the command line share this rule.
    pub fn texture(&self, path: &str) -> Option<TextureMeta> {
        match &self.kind {
            _ if !is_png(path) => None,
            AssetKind::Texture(texture) => Some(texture.clone()),
            AssetKind::Other => Some(TextureMeta::default()),
            _ => None,
        }
    }

    /// Reads the meta. An empty one just created is read once more after a moment: another
    /// lister may have created it and not written it yet.
    pub fn read(path: &Path) -> Result<Meta, String> {
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(path, text)
    }

    fn parse(path: &Path, text: String) -> Result<Meta, String> {
        let text = if text.is_empty() && may_be_being_written(path) {
            thread::sleep(CREATION_GRACE);
            fs::read_to_string(path).map_err(|error| error.to_string())?
        } else {
            text
        };
        let meta: Meta = ron::from_str(&text).map_err(|error| error.to_string())?;
        if let AssetKind::Texture(texture) = &meta.kind {
            texture.validate()?;
        }
        Ok(meta)
    }

    /// Reads the meta, creating it with a random identifier only when it does not exist.
    /// A meta that exists but cannot be read or parsed is an error and is never overwritten.
    #[cfg(not(target_family = "wasm"))]
    pub fn load_or_create(path: &Path, kind: AssetKind) -> Result<Meta, String> {
        match fs::read_to_string(path) {
            Ok(text) => return Self::parse(path, text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let meta = Self::new(kind);
        let created = OpenOptions::new().write(true).create_new(true).open(path);
        match created {
            Ok(mut file) => {
                let text = ron::to_string(&meta).expect("a meta always encodes") + "\n";
                file.write_all(text.as_bytes())
                    .map_err(|error| format!("cannot write the meta: {error}"))?;
                Ok(meta)
            }
            // Another lister created it first: use its identifier.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Self::read(path),
            Err(error) => Err(format!("cannot create the meta: {error}")),
        }
    }
}

/// One prefab or data asset found by [`read_assets`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundAsset {
    pub id: AssetId,
    pub path: String,
    /// In file form: a data asset is wrapped as `AssetFile(type: .., value: ..)`.
    pub text: String,
}

/// The prefab and data assets among `files` (each named as errors and paths show it, and
/// where it is on disk), read in name order. Files without a meta are skipped; two metas with
/// the same identifier are refused, naming both files. The CLI and the project service share
/// it, so both build the same asset set from the same folder.
pub fn read_assets(mut files: Vec<(String, PathBuf)>) -> Result<Vec<FoundAsset>, String> {
    files.sort();
    let mut found = Vec::new();
    let mut seen = BTreeMap::new();
    for (name, file) in files {
        if !name.ends_with(".ron") {
            continue;
        }
        let mut meta_file = file.clone().into_os_string();
        meta_file.push(".meta");
        let meta_file = PathBuf::from(meta_file);
        if !meta_file.is_file() {
            continue;
        }
        let meta = Meta::read(&meta_file)
            .map_err(|reason| format!("cannot read {name}.meta: {reason}"))?;
        if let Some(other) = seen.insert(meta.id, name.clone()) {
            return Err(format!(
                "{other} and {name} have the same identifier {}; give one of them a new meta",
                meta.id
            ));
        }
        let read =
            || fs::read_to_string(&file).map_err(|error| format!("cannot read {name}: {error}"));
        let text = match meta.kind {
            _ if AssetKind::of(&name) == AssetKind::Prefab => read()?,
            AssetKind::Data { r#type } => crate::assets::asset_file(&r#type, &read()?),
            _ => continue,
        };
        found.push(FoundAsset {
            id: meta.id,
            path: name,
            text,
        });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_meta_round_trips_as_ron() {
        let meta = Meta {
            version: META_VERSION,
            id: AssetId(42),
            kind: AssetKind::Data {
                r#type: "game::WeaponStats".to_string(),
            },
        };
        let text = ron::to_string(&meta).unwrap();
        assert!(
            text.contains("kind:Data(type:\"game::WeaponStats\")"),
            "{text}"
        );
        assert_eq!(ron::from_str::<Meta>(&text).unwrap(), meta);
    }

    #[test]
    fn metas_from_2a_still_read() {
        let text = "Meta(version: 1, id: \"0000000000000000000000000000002a\", kind: Scene)\n";
        assert_eq!(
            ron::from_str::<Meta>(text).unwrap(),
            Meta {
                version: 1,
                id: AssetId(42),
                kind: AssetKind::Scene,
            }
        );
    }

    #[test]
    fn version_1_meta_still_reads() {
        let text = "Meta(version: 1, id: \"0000000000000000000000000000002a\", kind: Other)\n";
        assert_eq!(ron::from_str::<Meta>(text).unwrap().version, 1);
        assert_eq!(META_VERSION, 2);
    }

    #[test]
    fn png_gets_a_default_texture_meta() {
        assert_eq!(
            AssetKind::of("assets/hero.png"),
            AssetKind::Texture(TextureMeta {
                pixels_per_unit: 16,
                grid: None,
                pivot: None,
                filter: Filter::Nearest,
            })
        );
    }

    fn meta_of(kind: AssetKind) -> Meta {
        Meta {
            version: 1,
            id: AssetId(42),
            kind,
        }
    }

    #[test]
    fn an_other_meta_on_a_png_is_a_default_texture() {
        let meta = meta_of(AssetKind::Other);
        assert_eq!(meta.texture("hero.png"), Some(TextureMeta::default()));
        assert_eq!(meta.texture("HERO.PNG"), Some(TextureMeta::default()));
        assert_eq!(meta.texture("hero.ron"), None);
    }

    #[test]
    fn a_texture_meta_counts_only_on_a_png() {
        let meta = sheet_meta();
        let AssetKind::Texture(texture) = &meta.kind else {
            unreachable!()
        };
        assert_eq!(meta.texture("hero.Png"), Some(texture.clone()));
        assert_eq!(meta.texture("hero.ron"), None);
        assert_eq!(meta_of(AssetKind::Prefab).texture("hero.png"), None);
    }

    #[test]
    fn png_extension_is_matched_in_any_case() {
        assert_eq!(
            AssetKind::of("assets/HERO.PNG"),
            AssetKind::Texture(TextureMeta::default())
        );
    }

    fn sheet_meta() -> Meta {
        Meta {
            version: META_VERSION,
            id: AssetId(42),
            kind: AssetKind::Texture(TextureMeta {
                grid: Some(Grid {
                    frame_width: 100,
                    frame_height: 55,
                    margin: 0,
                    spacing: 0,
                }),
                ..TextureMeta::default()
            }),
        }
    }

    #[test]
    fn texture_meta_round_trips() {
        let text = "Meta(version: 2, id: \"0000000000000000000000000000002a\", kind: Texture((pixels_per_unit: 16, grid: Some((frame_width: 100, frame_height: 55, margin: 0, spacing: 0)), pivot: None, filter: Nearest)))";
        assert_eq!(ron::from_str::<Meta>(text).unwrap(), sheet_meta());
        let written = ron::to_string(&sheet_meta()).unwrap();
        assert_eq!(ron::from_str::<Meta>(&written).unwrap(), sheet_meta());
    }

    #[test]
    fn texture_meta_with_zero_frame_width_is_refused_naming_the_field() {
        let mut meta = sheet_meta();
        let AssetKind::Texture(texture) = &mut meta.kind else {
            unreachable!()
        };
        texture.grid.as_mut().unwrap().frame_width = 0;
        let text = ron::to_string(&meta).unwrap();
        let error = Meta::parse(Path::new("hero.png.meta"), text).unwrap_err();
        assert!(error.contains("grid.frame_width"), "{error}");
    }

    #[test]
    fn kind_comes_from_the_extension() {
        assert_eq!(AssetKind::of("scenes/a.scene.ron"), AssetKind::Scene);
        assert_eq!(AssetKind::of("assets/b.prefab.ron"), AssetKind::Prefab);
        assert_eq!(AssetKind::of("assets/c.ron"), AssetKind::Other);
        assert_eq!(AssetKind::of("runs/d.replay"), AssetKind::Replay);
        assert_eq!(AssetKind::of("runs/e.trace"), AssetKind::Trace);
        assert_eq!(AssetKind::of("runs/f.snap"), AssetKind::Snapshot);
    }

    /// An empty meta left on disk long ago is not waited for at every listing.
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test bounds a wait with wall-clock time"
    )]
    fn only_a_recently_created_empty_meta_may_still_be_written() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("main.scene.ron.meta");
        fs::write(&path, "").unwrap();
        assert!(may_be_being_written(&path));
        let long_ago = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        assert!(!may_be_being_written(&path));
        let start = std::time::Instant::now();
        assert!(Meta::read(&path).is_err());
        assert!(
            start.elapsed() < CREATION_GRACE,
            "an old empty meta fails without waiting"
        );
    }

    /// Another lister created the meta and writes it a moment later.
    #[test]
    fn a_meta_being_written_by_another_lister_is_read_once_written() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("main.scene.ron.meta");
        fs::write(&path, "").unwrap();
        let writer = {
            let path = path.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(5));
                fs::write(
                    path,
                    "Meta(version: 1, id: \"0000000000000000000000000000002a\", kind: Scene)\n",
                )
                .unwrap();
            })
        };
        let meta = Meta::load_or_create(&path, AssetKind::Scene).unwrap();
        writer.join().unwrap();
        assert_eq!(meta.id, AssetId(42));
    }
}
