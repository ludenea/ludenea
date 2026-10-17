//! Access: what a system reads and writes, and when two accesses conflict.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "systems and the schedule use it")
)]

use std::collections::BTreeSet;

use crate::registry::{ComponentId, Registry, ResourceId};

/// What one query reads and writes, and the archetypes it can touch.
#[derive(Clone, Debug, Default)]
pub(crate) struct QueryAccess {
    pub(crate) reads: Vec<ComponentId>,
    pub(crate) writes: Vec<ComponentId>,
    /// Components every touched archetype has.
    pub(crate) required: Vec<ComponentId>,
    /// Components no touched archetype has.
    pub(crate) excluded: Vec<ComponentId>,
}

impl QueryAccess {
    /// Whether no archetype can be touched by both queries: one requires a
    /// component the other excludes.
    fn is_disjoint(&self, other: &Self) -> bool {
        self.required.iter().any(|id| other.excluded.contains(id))
            || other.required.iter().any(|id| self.excluded.contains(id))
    }

    /// Components one query writes and the other reads or writes, unless
    /// the queries are disjoint.
    fn conflicts(&self, other: &Self) -> BTreeSet<ComponentId> {
        if self.is_disjoint(other) {
            return BTreeSet::new();
        }
        let touches = |access: &Self, id: &ComponentId| {
            access.reads.contains(id) || access.writes.contains(id)
        };
        self.writes
            .iter()
            .filter(|&id| touches(other, id))
            .chain(other.writes.iter().filter(|&id| touches(self, id)))
            .copied()
            .collect()
    }
}

/// Everything one system accesses.
#[derive(Clone, Debug, Default)]
pub(crate) struct Access {
    pub(crate) queries: Vec<QueryAccess>,
    pub(crate) resource_reads: BTreeSet<ResourceId>,
    pub(crate) resource_writes: BTreeSet<ResourceId>,
    /// The system takes the whole world.
    pub(crate) exclusive: bool,
    /// The system records commands.
    pub(crate) commands: bool,
}

/// Why two systems cannot run in an arbitrary order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// One of the systems takes the whole world.
    Exclusive,
    /// Names of the components and resources one system writes and the
    /// other reads or writes, components first, each in identifier order.
    Data(Vec<&'static str>),
}

impl Access {
    /// Registers a query of this system.
    ///
    /// # Panics
    ///
    /// Panics, naming `system`, when the query writes a component another
    /// query of the system reads or writes in an overlapping archetype.
    pub(crate) fn add_query(&mut self, query: QueryAccess, registry: &Registry, system: &str) {
        for other in &self.queries {
            if let Some(&id) = query.conflicts(other).first() {
                panic!(
                    "system {system} accesses component {} in two conflicting queries",
                    registry.name(id)
                );
            }
        }
        self.queries.push(query);
    }

    /// Registers a resource read or write of this system.
    ///
    /// # Panics
    ///
    /// Panics, naming `system`, when the system writes a resource it also
    /// reads or writes through another parameter.
    pub(crate) fn add_resource(
        &mut self,
        id: ResourceId,
        write: bool,
        registry: &Registry,
        system: &str,
    ) {
        let conflicting =
            self.resource_writes.contains(&id) || (write && self.resource_reads.contains(&id));
        assert!(
            !conflicting,
            "system {system} accesses resource {} in two conflicting parameters",
            registry.resource_name(id)
        );
        if write {
            self.resource_writes.insert(id);
        } else {
            self.resource_reads.insert(id);
        }
    }

    /// Registers the commands parameter of this system.
    ///
    /// # Panics
    ///
    /// Panics, naming `system`, when the system already has one.
    pub(crate) fn add_commands(&mut self, system: &str) {
        assert!(
            !self.commands,
            "system {system} has more than one commands parameter"
        );
        self.commands = true;
    }

    /// Why `self` and `other` cannot run in an arbitrary order, or `None`
    /// when they can.
    pub(crate) fn conflict(&self, other: &Self, registry: &Registry) -> Option<Conflict> {
        if self.exclusive || other.exclusive {
            return Some(Conflict::Exclusive);
        }
        let components: BTreeSet<ComponentId> = self
            .queries
            .iter()
            .flat_map(|query| other.queries.iter().map(|other| query.conflicts(other)))
            .flatten()
            .collect();
        let resources: BTreeSet<ResourceId> = self
            .resource_writes
            .iter()
            .filter(|id| other.resource_reads.contains(id) || other.resource_writes.contains(id))
            .chain(
                other
                    .resource_writes
                    .iter()
                    .filter(|id| self.resource_reads.contains(id)),
            )
            .copied()
            .collect();
        let names: Vec<&'static str> = components
            .into_iter()
            .map(|id| registry.name(id))
            .chain(resources.into_iter().map(|id| registry.resource_name(id)))
            .collect();
        (!names.is_empty()).then_some(Conflict::Data(names))
    }
}

#[cfg(test)]
mod tests {
    use super::{Access, Conflict, QueryAccess};
    use crate::registry::{ComponentId, Registry};
    use crate::{Component, Resource};

    #[derive(Clone, Copy)]
    struct Position;
    impl Component for Position {}

    #[derive(Clone, Copy)]
    struct Velocity;
    impl Component for Velocity {}

    #[derive(Clone, Copy)]
    struct Player;
    impl Component for Player {}

    #[derive(Clone, Copy)]
    struct Score;
    impl Resource for Score {}

    /// The registry, then the identifiers of `Position`, `Velocity` and
    /// `Player`.
    fn registry() -> (Registry, [ComponentId; 3]) {
        let mut registry = Registry::default();
        registry.register::<Position>();
        registry.register::<Velocity>();
        registry.register::<Player>();
        registry.register_resource::<Score>();
        let ids = [
            registry.id::<Position>(),
            registry.id::<Velocity>(),
            registry.id::<Player>(),
        ];
        (registry, ids)
    }

    /// One query reading `reads`, writing `writes`, requiring `with` and
    /// excluding `without`.
    fn query(
        reads: &[ComponentId],
        writes: &[ComponentId],
        with: &[ComponentId],
        without: &[ComponentId],
    ) -> QueryAccess {
        let mut required = [reads, writes, with].concat();
        required.sort_unstable();
        QueryAccess {
            reads: reads.to_vec(),
            writes: writes.to_vec(),
            required,
            excluded: without.to_vec(),
        }
    }

    fn system(query: QueryAccess) -> Access {
        Access {
            queries: vec![query],
            ..Access::default()
        }
    }

    fn resource(registry: &Registry, write: bool) -> Access {
        let mut access = Access::default();
        access.add_resource(registry.resource_id::<Score>(), write, registry, "system");
        access
    }

    #[test]
    fn two_reads_do_not_conflict() {
        let (registry, [position, ..]) = registry();
        let read = system(query(&[position], &[], &[], &[]));

        assert_eq!(read.conflict(&read, &registry), None);
    }

    #[test]
    fn a_write_conflicts_with_a_read_or_a_write() {
        let (registry, [position, velocity, _]) = registry();
        let write = system(query(&[], &[position], &[], &[]));
        let read = system(query(&[position, velocity], &[], &[], &[]));

        let expected = Some(Conflict::Data(vec![std::any::type_name::<Position>()]));
        assert_eq!(write.conflict(&read, &registry), expected);
        assert_eq!(read.conflict(&write, &registry), expected);
        assert_eq!(write.conflict(&write, &registry), expected);
    }

    #[test]
    fn with_and_without_make_queries_disjoint() {
        let (registry, [position, _, player]) = registry();
        let players = system(query(&[], &[position], &[player], &[]));
        let others = system(query(&[], &[position], &[], &[player]));

        assert_eq!(players.conflict(&others, &registry), None);
    }

    #[test]
    fn an_accessed_component_also_makes_queries_disjoint() {
        let (registry, [position, _, player]) = registry();
        let players = system(query(&[player], &[position], &[], &[]));
        let others = system(query(&[], &[position], &[], &[player]));

        assert_eq!(players.conflict(&others, &registry), None);
    }

    #[test]
    fn resources_conflict_like_components() {
        let (registry, _) = registry();
        let read = resource(&registry, false);
        let write = resource(&registry, true);
        let expected = Some(Conflict::Data(vec![std::any::type_name::<Score>()]));

        assert_eq!(read.conflict(&read, &registry), None);
        assert_eq!(write.conflict(&read, &registry), expected);
        assert_eq!(read.conflict(&write, &registry), expected);
    }

    #[test]
    fn an_exclusive_system_conflicts_with_everything() {
        let (registry, _) = registry();
        let exclusive = Access {
            exclusive: true,
            ..Access::default()
        };

        assert_eq!(
            exclusive.conflict(&Access::default(), &registry),
            Some(Conflict::Exclusive)
        );
        assert_eq!(
            Access::default().conflict(&exclusive, &registry),
            Some(Conflict::Exclusive)
        );
    }

    #[test]
    #[should_panic(expected = "system movement accesses component")]
    fn two_overlapping_queries_writing_a_component_panic() {
        let (registry, [position, ..]) = registry();
        let mut access = system(query(&[], &[position], &[], &[]));

        access.add_query(query(&[position], &[], &[], &[]), &registry, "movement");
    }

    #[test]
    fn two_disjoint_queries_writing_a_component_are_accepted() {
        let (registry, [position, _, player]) = registry();
        let mut access = system(query(&[], &[position], &[player], &[]));

        access.add_query(
            query(&[], &[position], &[], &[player]),
            &registry,
            "movement",
        );

        assert_eq!(access.queries.len(), 2);
    }

    #[test]
    #[should_panic(expected = "system scoring accesses resource")]
    fn reading_and_writing_a_resource_in_one_system_panics() {
        let (registry, _) = registry();
        let mut access = Access::default();
        let id = registry.resource_id::<Score>();

        access.add_resource(id, false, &registry, "scoring");
        access.add_resource(id, true, &registry, "scoring");
    }

    #[test]
    #[should_panic(expected = "system spawner has more than one commands parameter")]
    fn two_commands_parameters_panic() {
        let mut access = Access::default();

        access.add_commands("spawner");
        access.add_commands("spawner");
    }
}
