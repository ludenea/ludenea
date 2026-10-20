//! The prefab and data asset texts of a project folder, for loading scenes by identifier,
//! and its textures, for rendering.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ludenea_app::meta::{Meta, TextureMeta, is_png, read_assets};
use ludenea_app::{AssetTexts, NamedAssetTexts};
use ludenea_ecs::AssetId;

const MANIFEST: &str = "ludenea.toml";
/// The folders of a project root that hold assets.
const ASSET_DIRECTORIES: [&str; 2] = ["scenes", "assets"];

/// Every prefab and data asset of the project holding `scene`, keyed by meta identifier.
///
/// The root is `project`, else the nearest folder above the scene holding `ludenea.toml`,
/// else the scene's folder. A project root's `scenes/` and `assets/` are scanned, a plain
/// folder entirely, as [`read_assets`] reads them.
pub fn project_assets(scene: &Path, project: Option<&Path>) -> Result<NamedAssetTexts, String> {
    let files = project_files(scene, project)?
        .into_iter()
        .map(|file| (file.to_string_lossy().into_owned(), file))
        .collect();
    let mut assets = NamedAssetTexts {
        texts: AssetTexts::new(),
        paths: BTreeMap::new(),
    };
    for asset in read_assets(files)? {
        assets.texts.insert(asset.id, asset.text);
        assets.paths.insert(asset.id, asset.path);
    }
    Ok(assets)
}

/// A texture of a project: its identifier, path, PNG bytes, and meta.
pub type ProjectTexture = (AssetId, String, Vec<u8>, TextureMeta);

/// Every texture of the project holding `scene` (found as [`project_assets`] finds assets):
/// each `.png` whose meta makes it a texture ([`Meta::texture`]), with its identifier, path, bytes, and meta,
/// sorted by identifier. A `.png` without a meta is skipped; a meta that cannot be read, or
/// two textures with the same identifier, are refused naming the files.
pub fn project_textures(
    scene: &Path,
    project: Option<&Path>,
) -> Result<Vec<ProjectTexture>, String> {
    let mut textures: BTreeMap<AssetId, (String, Vec<u8>, TextureMeta)> = BTreeMap::new();
    for file in project_files(scene, project)? {
        let name = file.to_string_lossy().into_owned();
        let meta_file = PathBuf::from(format!("{name}.meta"));
        if !is_png(&name) || !meta_file.is_file() {
            continue;
        }
        let meta = Meta::read(&meta_file)
            .map_err(|reason| format!("cannot read {name}.meta: {reason}"))?;
        let Some(texture) = meta.texture(&name) else {
            continue;
        };
        if let Some((other, ..)) = textures.get(&meta.id) {
            return Err(format!(
                "{other} and {name} have the same identifier {}; give one of them a new meta",
                meta.id
            ));
        }
        let bytes = fs::read(&file).map_err(|error| format!("cannot read {name}: {error}"))?;
        textures.insert(meta.id, (name, bytes, texture));
    }
    Ok(textures
        .into_iter()
        .map(|(id, (name, bytes, texture))| (id, name, bytes, texture))
        .collect())
}

/// Every file of the project holding `scene`: under `project`, else under the nearest folder
/// above the scene holding `ludenea.toml`, its `scenes/` and `assets/`; else under the
/// scene's folder.
fn project_files(scene: &Path, project: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    // Absolute, so a bare file name has a folder and the manifest search climbs past the
    // current directory.
    let scene = std::path::absolute(scene)
        .map_err(|error| format!("cannot locate {}: {error}", scene.display()))?;
    let folder = scene.parent().expect("an absolute file path has a parent");
    let root = project.map(Path::to_path_buf).or_else(|| {
        folder
            .ancestors()
            .find(|dir| dir.join(MANIFEST).is_file())
            .map(Path::to_path_buf)
    });
    let mut files = Vec::new();
    match &root {
        Some(root) => {
            for directory in ASSET_DIRECTORIES {
                collect(&root.join(directory), &mut files)?;
            }
        }
        None => collect(folder, &mut files)?,
    }
    Ok(files)
}

/// Adds every file under `directory`, if it exists. Links to folders are not followed, so
/// a link cycle cannot loop.
fn collect(directory: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot list {}: {error}", directory.display())),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect(&path, out)?;
        } else if !path.is_dir() {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ludenea_app::AssetSource;

    use super::*;

    /// A fresh folder, removed when the returned guard drops.
    fn dir() -> (tempfile::TempDir, PathBuf) {
        let guard = tempfile::tempdir().unwrap();
        let path = guard.path().to_path_buf();
        (guard, path)
    }

    fn put(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn meta(path: &Path, id: u128, kind: &str) {
        put(
            &PathBuf::from(format!("{}.meta", path.display())),
            &format!("Meta(version: 1, id: \"{:032x}\", kind: {kind})", id),
        );
    }

    #[test]
    fn project_root_is_nearest_manifest() {
        let (_guard, root) = dir();
        put(&root.join("ludenea.toml"), "");
        let scene = root.join("scenes/levels/main.scene.ron");
        put(&scene, "");
        let prefab = root.join("assets/crate.prefab.ron");
        put(&prefab, "Prefab(version: 1)");
        meta(&prefab, 1, "Prefab");
        let stats = root.join("assets/stats.ron");
        put(&stats, "(damage: 3)");
        meta(&stats, 2, "Data(type: \"game::Stats\")");
        let plain = root.join("assets/plain.ron");
        put(&plain, "()");
        meta(&plain, 3, "Other");
        // Outside `scenes/` and `assets/`: not scanned.
        let outside = root.join("other/far.prefab.ron");
        put(&outside, "Prefab(version: 1)");
        meta(&outside, 4, "Prefab");

        let assets = project_assets(&scene, None).unwrap();
        assert_eq!(
            assets.texts.keys().copied().collect::<Vec<_>>(),
            [AssetId(1), AssetId(2)]
        );
        assert_eq!(assets.text(AssetId(1)), Some("Prefab(version: 1)"));
        assert_eq!(
            assets.text(AssetId(2)),
            Some("AssetFile(type: \"game::Stats\", value: (damage: 3))")
        );
        assert_eq!(assets.path(AssetId(1)), Some(prefab.to_str().unwrap()));
    }

    #[test]
    fn scene_folder_is_used_without_manifest() {
        let (_guard, folder) = dir();
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let prefab = folder.join("nested/crate.prefab.ron");
        put(&prefab, "Prefab(version: 1)");
        meta(&prefab, 7, "Prefab");
        let assets = project_assets(&scene, None).unwrap();
        assert_eq!(
            assets.texts.keys().copied().collect::<Vec<_>>(),
            [AssetId(7)]
        );
    }

    #[test]
    fn duplicate_asset_identifiers_are_refused() {
        let (_guard, folder) = dir();
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let a = folder.join("a.prefab.ron");
        let b = folder.join("b.prefab.ron");
        for prefab in [&a, &b] {
            put(prefab, "Prefab(version: 1)");
            meta(prefab, 9, "Prefab");
        }
        let error = project_assets(&scene, None).err().expect("refused");
        assert!(
            error.contains(a.to_str().unwrap()) && error.contains(b.to_str().unwrap()),
            "{error}"
        );
    }

    #[test]
    fn broken_meta_is_refused_naming_it() {
        let (_guard, folder) = dir();
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let prefab = folder.join("a.prefab.ron");
        put(&prefab, "Prefab(version: 1)");
        put(&folder.join("a.prefab.ron.meta"), "Meta(");
        let error = project_assets(&scene, None).err().expect("refused");
        assert!(error.contains("a.prefab.ron.meta"), "{error}");
    }

    fn texture_meta(path: &Path, id: u128) {
        put(
            &PathBuf::from(format!("{}.meta", path.display())),
            &format!(
                "Meta(version: 2, id: \"{id:032x}\", kind: Texture((pixels_per_unit: 8, grid: None, pivot: None, filter: Linear)))"
            ),
        );
    }

    #[test]
    fn textures_are_the_pngs_with_a_texture_meta_sorted_by_identifier() {
        let (_guard, folder) = dir();
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let (a, b) = (folder.join("a.png"), folder.join("nested/b.png"));
        put(&a, "a");
        texture_meta(&a, 2);
        put(&b, "b");
        texture_meta(&b, 1);
        put(&folder.join("no-meta.png"), "c");
        let prefab = folder.join("prefab.png");
        put(&prefab, "d");
        meta(&prefab, 3, "Prefab");
        let textures = project_textures(&scene, None).unwrap();
        let meta = TextureMeta {
            pixels_per_unit: 8,
            filter: ludenea_app::meta::Filter::Linear,
            ..TextureMeta::default()
        };
        assert_eq!(
            textures,
            [
                (
                    AssetId(1),
                    b.display().to_string(),
                    b"b".to_vec(),
                    meta.clone()
                ),
                (AssetId(2), a.display().to_string(), b"a".to_vec(), meta),
            ]
        );
    }

    #[test]
    fn a_png_with_a_version_1_other_meta_is_a_default_texture() {
        let (_guard, folder) = dir();
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let old = folder.join("old.png");
        put(&old, "a");
        meta(&old, 4, "Other");
        let misplaced = folder.join("misplaced.ron");
        put(&misplaced, "()");
        texture_meta(&misplaced, 5);
        assert_eq!(
            project_textures(&scene, None).unwrap(),
            [(
                AssetId(4),
                old.display().to_string(),
                b"a".to_vec(),
                TextureMeta::default()
            )]
        );
    }

    #[test]
    fn textures_with_the_same_identifier_are_refused() {
        let (_guard, folder) = dir();
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let (a, b) = (folder.join("a.png"), folder.join("b.png"));
        for texture in [&a, &b] {
            put(texture, "");
            texture_meta(texture, 9);
        }
        let error = project_textures(&scene, None).expect_err("refused");
        assert!(
            error.contains(a.to_str().unwrap()) && error.contains(b.to_str().unwrap()),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_to_folders_are_not_followed() {
        let (_guard, folder) = dir();
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let prefab = folder.join("nested/crate.prefab.ron");
        put(&prefab, "Prefab(version: 1)");
        meta(&prefab, 7, "Prefab");
        std::os::unix::fs::symlink(&folder, folder.join("nested/loop")).unwrap();
        // A link to a file is kept: with a meta of its own, it is an asset.
        let (_outside_guard, outside) = dir();
        let target = outside.join("far.prefab.ron");
        put(&target, "Prefab(version: 1, next_id: 0)");
        let linked = folder.join("linked.prefab.ron");
        std::os::unix::fs::symlink(&target, &linked).unwrap();
        meta(&linked, 8, "Prefab");
        let assets = project_assets(&scene, None).unwrap();
        assert_eq!(
            assets.texts.keys().copied().collect::<Vec<_>>(),
            [AssetId(7), AssetId(8)]
        );
        assert_eq!(
            assets.text(AssetId(8)),
            Some("Prefab(version: 1, next_id: 0)")
        );
    }
}
