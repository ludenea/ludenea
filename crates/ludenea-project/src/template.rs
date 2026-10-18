use std::fs;
use std::path::Path;

use crate::{ProjectError, io_error};

const FILES: [(&str, &str); 10] = [
    ("ludenea.toml", include_str!("../template/ludenea.toml")),
    ("Cargo.toml", include_str!("../template/Cargo.toml")),
    (
        "rust-toolchain.toml",
        include_str!("../template/rust-toolchain.toml"),
    ),
    ("src/lib.rs", include_str!("../template/src/lib.rs")),
    ("src/main.rs", include_str!("../template/src/main.rs")),
    (
        "scenes/main.scene.ron",
        include_str!("../template/scenes/main.scene.ron"),
    ),
    (
        "editor/package.json",
        include_str!("../template/editor/package.json"),
    ),
    (
        "editor/index.ts",
        include_str!("../template/editor/index.ts"),
    ),
    (
        "editor/tsconfig.json",
        include_str!("../template/editor/tsconfig.json"),
    ),
    (".gitignore", include_str!("../template/gitignore")),
];

/// Whether `name` matches `^[a-z][a-z0-9_]*$`.
pub(crate) fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Writes the template into the existing directory `root`, with the toolchain of the engine.
pub(crate) fn write(root: &Path, name: &str, engine_path: &Path) -> Result<(), ProjectError> {
    let channel = toolchain_channel(engine_path)?;
    let mut engine_path = engine_path.to_string_lossy().into_owned();
    if cfg!(windows) {
        engine_path = engine_path.replace('\\', "/");
    }
    // Only TOML strings hold the path (`Cargo.toml`).
    let engine_path = toml_string_contents(&engine_path);
    for (path, contents) in FILES {
        let text = contents
            .replace("{{name}}", name)
            .replace("{{engine_path}}", &engine_path)
            .replace("{{toolchain_channel}}", &channel)
            .replace("{{engine_version}}", env!("CARGO_PKG_VERSION"));
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::write(&target, text).map_err(|error| io_error(&target, error))?;
    }
    Ok(())
}

/// The `toolchain.channel` of the engine's `rust-toolchain.toml`, so a game builds with the
/// toolchain its engine was written for.
fn toolchain_channel(engine_path: &Path) -> Result<String, ProjectError> {
    let path = engine_path.join("rust-toolchain.toml");
    let text = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
    let manifest = |message: String| ProjectError::Manifest {
        path: path.clone(),
        message,
    };
    let table: toml::Table = toml::from_str(&text).map_err(|error| manifest(error.to_string()))?;
    let channel = table
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("channel"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| manifest("no toolchain.channel".to_string()))?;
    Ok(toml_string_contents(channel))
}

/// `text` escaped to sit between the quotes of a TOML basic string, whose escapes are a
/// superset of those JSON strings use.
fn toml_string_contents(text: &str) -> String {
    let json = serde_json::to_string(text).expect("a string serializes");
    json[1..json.len() - 1].to_string()
}
