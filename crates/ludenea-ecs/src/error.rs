use crate::Entity;

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
        "name `{name}` is used by both `{first}` and `{second}`; give one of them another name with `#[component(name = \"...\")]` or `#[resource(name = \"...\")]`"
    )]
    DuplicateName {
        name: &'static str,
        first: &'static str,
        second: &'static str,
    },
}
