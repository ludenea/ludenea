use crate::Entity;

#[derive(Debug, thiserror::Error)]
pub enum EcsError {
    #[error("entity {0} does not exist or was despawned; check `World::is_alive` before using it")]
    NoSuchEntity(Entity),
    #[error("component `{0}` is not registered; call `World::register_component` first")]
    UnregisteredComponent(&'static str),
    #[error("resource `{0}` is not registered; call `World::register_resource` first")]
    UnregisteredResource(&'static str),
    #[error(
        "name `{name}` is used by both `{first}` and `{second}`; give one of them another name with `#[component(name = \"...\")]` or `#[resource(name = \"...\")]`"
    )]
    DuplicateName {
        name: &'static str,
        first: &'static str,
        second: &'static str,
    },
}
