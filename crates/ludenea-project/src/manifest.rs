use serde::{Deserialize, Serialize};

/// The contents of `ludenea.toml`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    /// The engine version the project was created with.
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_scene: Option<String>,
}
