//! Errors returned by world operations.

use std::fmt;

use crate::Entity;

/// An operation on a [`World`](crate::World) that could not be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EcsError {
    /// The entity was despawned or was never alive here.
    StaleEntity(Entity),
    /// The snapshot was taken from a world with a different component
    /// registry.
    IncompatibleSnapshot,
}

impl fmt::Display for EcsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleEntity(entity) => write!(formatter, "entity {entity} is not alive"),
            Self::IncompatibleSnapshot => formatter
                .write_str("the snapshot comes from a world with a different component registry"),
        }
    }
}

impl std::error::Error for EcsError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Allocator;

    #[test]
    fn stale_entity_names_the_entity() {
        let entity = Allocator::default().allocate();

        assert_eq!(
            EcsError::StaleEntity(entity).to_string(),
            "entity 0v0 is not alive"
        );
    }
}
