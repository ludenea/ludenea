//! Errors returned by world operations and schedule builds.

use std::fmt;

use crate::Entity;
use crate::access::Conflict;

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

/// Two systems with conflicting access and no order between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ambiguity {
    /// The system inserted first.
    pub first: &'static str,
    /// The system inserted second.
    pub second: &'static str,
    /// Why their order matters.
    pub conflict: Conflict,
}

impl fmt::Display for Ambiguity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            first,
            second,
            conflict,
        } = self;
        match conflict {
            Conflict::Exclusive => write!(
                formatter,
                "{first} and {second} are unordered and one of them takes the whole world"
            )?,
            Conflict::Data(names) => write!(
                formatter,
                "{first} and {second} are unordered and conflict on {}",
                names.join(", ")
            )?,
        }
        write!(
            formatter,
            "; order them with `{first}.before({second})` or `{second}.before({first})`"
        )
    }
}

/// A schedule that cannot be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleError {
    /// The same function was added twice.
    DuplicateSystem(&'static str),
    /// An ordering constraint names a system missing from the schedule.
    UnknownSystem(&'static str),
    /// The ordering constraints form a cycle, listed from the system
    /// inserted first.
    Cycle(Vec<&'static str>),
    /// Conflicting systems are left unordered, every pair in insertion
    /// order.
    Ambiguous(Vec<Ambiguity>),
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSystem(name) => {
                write!(formatter, "{name} is added to the schedule twice")
            }
            Self::UnknownSystem(name) => write!(
                formatter,
                "an ordering constraint names {name}, which is not in the schedule"
            ),
            Self::Cycle(names) => write!(
                formatter,
                "the ordering constraints form a cycle: {} -> {}",
                names.join(" -> "),
                names.first().copied().unwrap_or_default()
            ),
            Self::Ambiguous(ambiguities) => {
                formatter.write_str("the schedule is ambiguous:")?;
                for ambiguity in ambiguities {
                    write!(formatter, "\n- {ambiguity}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ScheduleError {}

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
