use crate::{AssetId, Entity};

#[derive(Debug, thiserror::Error)]
pub enum EcsError {
    #[error("entity {0} does not exist or was despawned; check `World::is_alive` before using it")]
    NoSuchEntity(Entity),
    #[error("component `{0}` is not registered; call `World::register_component` first")]
    UnregisteredComponent(String),
    #[error("resource `{0}` is not registered; call `World::register_resource` first")]
    UnregisteredResource(String),
    #[error(
        "component `{0}` is already borrowed by another query, or twice by one query with at least one `&mut`; drop the other query first or merge the accesses"
    )]
    ComponentBorrowed(&'static str),
    #[error(
        "name `{name}` is used by both `{first}` and `{second}`; give one of them another name with `#[component(name = \"...\")]`, `#[resource(name = \"...\")]`, or `#[asset(name = \"...\")]`"
    )]
    DuplicateName {
        name: &'static str,
        first: &'static str,
        second: &'static str,
    },
    #[error("asset type `{0}` is not registered; call `World::register_asset` first")]
    UnknownAsset(String),
    #[error(
        "asset type `{0}` has no default value; register it with `World::register_asset_with_default`"
    )]
    NoDefault(String),
    #[error("asset {id} is not a valid `{type_name}`: {reason}")]
    InvalidAsset {
        id: AssetId,
        type_name: String,
        reason: String,
    },
}
