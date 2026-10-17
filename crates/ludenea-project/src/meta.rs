use std::path::Path;
use std::{fs, io};

use ludenea_ecs::AssetId;
use serde::Deserialize;

/// What an asset is, as far as the editor cares.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn read(path: &Path) -> Option<Meta> {
        ron::from_str(&fs::read_to_string(path).ok()?).ok()
    }

    /// Writes a fresh meta with a random identifier.
    pub fn create(path: &Path, kind: AssetKind) -> io::Result<Meta> {
        let id = AssetId(uuid::Uuid::new_v4().as_u128());
        fs::write(
            path,
            format!("Meta(version: 1, id: \"{id}\", kind: {kind:?})\n"),
        )?;
        Ok(Meta {
            version: 1,
            id,
            kind,
        })
    }
}
