use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use ludenea_ecs::AssetId;
use serde::{Deserialize, Serialize};

/// What an asset is, as far as the editor cares.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub enum AssetKind {
    Scene,
    Other,
}

impl AssetKind {
    pub(crate) fn of(path: &str) -> Self {
        if path.ends_with(".scene.ron") {
            Self::Scene
        } else {
            Self::Other
        }
    }
}

/// The `<file>.meta` sidecar that gives a file a stable identifier.
#[derive(Deserialize, Debug)]
pub(crate) struct Meta {
    #[allow(dead_code)]
    version: u32,
    pub id: AssetId,
    #[allow(dead_code)]
    pub kind: AssetKind,
}

/// How long a lister waits for another one to finish writing a meta it just created.
const CREATION_GRACE: Duration = Duration::from_millis(50);

impl Meta {
    /// Reads the meta. An empty one is read once more after a moment: another lister may have
    /// created it and not written it yet.
    pub fn read(path: &Path) -> Result<Meta, String> {
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(path, text)
    }

    fn parse(path: &Path, text: String) -> Result<Meta, String> {
        let text = if text.is_empty() {
            thread::sleep(CREATION_GRACE);
            fs::read_to_string(path).map_err(|error| error.to_string())?
        } else {
            text
        };
        ron::from_str(&text).map_err(|error| error.to_string())
    }

    /// Reads the meta, creating it with a random identifier only when it does not exist.
    /// A meta that exists but cannot be read or parsed is an error and is never overwritten.
    pub fn load_or_create(path: &Path, kind: AssetKind) -> Result<Meta, String> {
        match fs::read_to_string(path) {
            Ok(text) => return Self::parse(path, text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let id = AssetId(uuid::Uuid::new_v4().as_u128());
        let created = OpenOptions::new().write(true).create_new(true).open(path);
        match created {
            Ok(mut file) => {
                let text = format!("Meta(version: 1, id: \"{id}\", kind: {kind:?})\n");
                file.write_all(text.as_bytes())
                    .map_err(|error| format!("cannot write the meta: {error}"))?;
                Ok(Meta {
                    version: 1,
                    id,
                    kind,
                })
            }
            // Another lister created it first: use its identifier.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Self::read(path),
            Err(error) => Err(format!("cannot create the meta: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
