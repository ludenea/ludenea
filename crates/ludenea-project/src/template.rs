use std::path::Path;
use std::{fs, io};

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

/// Writes the template into the existing directory `root`.
pub(crate) fn write(root: &Path, name: &str, engine_path: &Path) -> io::Result<()> {
    let engine_path = engine_path.to_string_lossy().replace('\\', "/");
    for (path, contents) in FILES {
        let text = contents
            .replace("{{name}}", name)
            .replace("{{engine_path}}", &engine_path)
            .replace("{{engine_version}}", env!("CARGO_PKG_VERSION"));
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, text)?;
    }
    Ok(())
}
