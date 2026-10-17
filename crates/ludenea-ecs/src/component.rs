use serde::Serialize;
use serde::de::DeserializeOwned;

/// Per-entity simulation data. `NAME` is the stable identifier used for registration,
/// snapshots, and scene files; derive it with `#[derive(Component)]`.
pub trait Component: Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
}

/// Global simulation data, one value per type. `NAME` is its stable identifier;
/// derive it with `#[derive(Resource)]`.
pub trait Resource: Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
}
