//! Writes the committed assets: the scene and the bug sprite sheet with its metadata.

use std::fs;
use std::path::Path;

fn main() {
    let assets = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));
    fs::create_dir_all(assets).expect("cannot create the assets directory");
    for (name, bytes) in swarm::generated_files() {
        fs::write(assets.join(name), bytes)
            .unwrap_or_else(|error| panic!("cannot write {name}: {error}"));
    }
}
