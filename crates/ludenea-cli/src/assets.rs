//! The prefab and data asset texts of a project folder, for loading scenes by identifier.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ludenea_app::assets::asset_file;
use ludenea_app::meta::{AssetKind, Meta};
use ludenea_app::{AssetTexts, NamedAssetTexts};

const MANIFEST: &str = "ludenea.toml";
/// The folders of a project root that hold assets.
const ASSET_DIRECTORIES: [&str; 2] = ["scenes", "assets"];

/// Every prefab and data asset of the project holding `scene`, keyed by meta identifier.
///
/// The root is `project`, else the nearest folder above the scene holding `ludenea.toml`,
/// else the scene's folder. A project root's `scenes/` and `assets/` are scanned, a plain
/// folder entirely, in sorted path order. Files without a meta are skipped.
pub fn project_assets(scene: &Path, project: Option<&Path>) -> Result<NamedAssetTexts, String> {
    let folder = scene.parent().unwrap_or(Path::new("."));
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
    files.sort();

    let mut assets = NamedAssetTexts {
        texts: AssetTexts::new(),
        paths: BTreeMap::new(),
    };
    let mut seen = BTreeMap::new();
    for file in files {
        let name = file.to_string_lossy();
        if !name.ends_with(".ron") {
            continue;
        }
        let meta_file = PathBuf::from(format!("{name}.meta"));
        if !meta_file.is_file() {
            continue;
        }
        let meta = Meta::read(&meta_file)
            .map_err(|reason| format!("cannot read {}: {reason}", meta_file.display()))?;
        if let Some(other) = seen.insert(meta.id, name.to_string()) {
            return Err(format!(
                "{other} and {name} have the same identifier {}; give one of them a new meta",
                meta.id
            ));
        }
        let text = match meta.kind {
            _ if AssetKind::of(&name) == AssetKind::Prefab => read(&file)?,
            AssetKind::Data { r#type } => asset_file(&r#type, &read(&file)?),
            _ => continue,
        };
        assets.texts.insert(meta.id, text);
        assets.paths.insert(meta.id, name.into_owned());
    }
    Ok(assets)
}

/// Adds every file under `directory`, if it exists.
fn collect(directory: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot list {}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            collect(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use ludenea_app::AssetSource;
    use ludenea_ecs::AssetId;

    use super::*;

    fn dir(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludenea-assets-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
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
        let root = dir("nearest_manifest");
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
        let folder = dir("without_manifest");
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
        let folder = dir("duplicates");
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
        let folder = dir("broken_meta");
        let scene = folder.join("main.scene.ron");
        put(&scene, "");
        let prefab = folder.join("a.prefab.ron");
        put(&prefab, "Prefab(version: 1)");
        put(&folder.join("a.prefab.ron.meta"), "Meta(");
        let error = project_assets(&scene, None).err().expect("refused");
        assert!(error.contains("a.prefab.ron.meta"), "{error}");
    }
}
