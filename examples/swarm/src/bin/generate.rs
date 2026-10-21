//! Writes the committed assets: the scene and the bug sprite sheet with its metadata.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let assets = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));
    match write(assets) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn write(assets: &Path) -> Result<(), String> {
    fs::create_dir_all(assets)
        .map_err(|error| format!("cannot create {}: {error}", assets.display()))?;
    for (name, bytes) in swarm::generated_files() {
        let path = assets.join(name);
        fs::write(&path, bytes)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    Ok(())
}
