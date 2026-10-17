use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use ludenea_ecs::AssetId;
use serde::{Deserialize, Serialize};

/// What an asset is, as far as the editor cares.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

impl Meta {
    pub fn read(path: &Path) -> Result<Meta, String> {
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        ron::from_str(&text).map_err(|error| error.to_string())
    }

    /// Reads the meta, creating it with a random identifier only when it does not exist.
    /// A meta that exists but cannot be read or parsed is an error and is never overwritten.
    pub fn load_or_create(path: &Path, kind: AssetKind) -> Result<Meta, String> {
        match fs::read_to_string(path) {
            Ok(text) => return ron::from_str(&text).map_err(|error| error.to_string()),
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
