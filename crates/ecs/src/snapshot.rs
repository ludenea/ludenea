//! Snapshots: the whole state of a world, restored with memory copies.

use crate::registry::RegistryFingerprint;
use crate::world::{State, next_archetype_epoch};
use crate::{EcsError, World};

/// The whole state of a [`World`]: entities, allocator, components and
/// resources.
///
/// Restoring it yields the same entity identifiers and the same iteration
/// order as when it was taken.
#[derive(Clone)]
pub struct Snapshot {
    fingerprint: RegistryFingerprint,
    state: State,
}

impl World {
    /// Captures the whole state of the world.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            fingerprint: self.registry.fingerprint(),
            state: self.state.clone(),
        }
    }

    /// Restores the state captured by `snapshot`, reusing the allocations of
    /// the world. The snapshot may come from another world built with the
    /// same registry.
    ///
    /// # Errors
    ///
    /// Returns [`EcsError::IncompatibleSnapshot`] when the snapshot comes
    /// from a world with a different registry; the world is left untouched.
    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<(), EcsError> {
        if !self.registry.has_fingerprint(&snapshot.fingerprint) {
            return Err(EcsError::IncompatibleSnapshot);
        }
        self.state.clone_from(&snapshot.state);
        self.archetype_epoch = next_archetype_epoch();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Component, EcsError, Entity, Resource, World};

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Position(i32);
    impl Component for Position {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Velocity(i32);
    impl Component for Velocity {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Score(u32);
    impl Resource for Score {}

    fn world() -> World {
        let mut builder = World::builder();
        builder
            .register::<Position>()
            .register::<Velocity>()
            .insert_resource(Score(0));
        builder.build()
    }

    #[test]
    fn restore_brings_back_the_resources() {
        let mut world = world();
        let snapshot = world.snapshot();

        world.resource_mut::<Score>().0 = 7;
        world.restore(&snapshot).expect("same registry");

        assert_eq!(world.resource::<Score>(), &Score(0));
    }

    #[test]
    fn a_snapshot_from_a_world_with_other_resources_is_rejected() {
        let snapshot = world().snapshot();
        let mut builder = World::builder();
        builder.register::<Position>().register::<Velocity>();
        let mut without_resources = builder.build();

        assert_eq!(
            without_resources.restore(&snapshot),
            Err(EcsError::IncompatibleSnapshot)
        );
    }

    #[test]
    fn restore_reuses_the_resource_allocations() {
        let mut world = world();
        let snapshot = world.snapshot();
        world.restore(&snapshot).expect("same registry");
        let before = std::ptr::from_ref(world.resource::<Score>());

        world.resource_mut::<Score>().0 = 3;
        world.restore(&snapshot).expect("same registry");

        assert_eq!(std::ptr::from_ref(world.resource::<Score>()), before);
    }

    #[test]
    fn restore_brings_back_the_components() {
        let mut world = world();
        let entity = world.spawn(Position(1));
        let snapshot = world.snapshot();

        *world
            .get_mut::<Position>(entity)
            .expect("the entity has a position") = Position(9);
        world.restore(&snapshot).expect("same registry");

        assert_eq!(world.get::<Position>(entity), Some(&Position(1)));
    }

    #[test]
    fn an_entity_despawned_after_the_snapshot_is_alive_again() {
        let mut world = world();
        let entity = world.spawn(Position(1));
        let snapshot = world.snapshot();

        world.despawn(entity).expect("the entity is alive");
        world.restore(&snapshot).expect("same registry");

        assert_eq!(world.get::<Position>(entity), Some(&Position(1)));
    }

    #[test]
    fn an_entity_spawned_after_the_snapshot_is_stale() {
        let mut world = world();
        let snapshot = world.snapshot();

        let entity = world.spawn((Position(1), Velocity(2)));
        world.restore(&snapshot).expect("same registry");

        assert!(!world.contains(entity));
        assert_eq!(world.get::<Position>(entity), None);
    }

    #[test]
    fn restore_hands_out_the_same_identifiers_again() {
        let mut world = world();
        let snapshot = world.snapshot();
        let first_run = world.spawn(Position(1));

        world.restore(&snapshot).expect("same registry");

        assert_eq!(world.spawn(Position(2)), first_run);
    }

    #[test]
    fn a_snapshot_restores_into_another_world_with_the_same_registry() {
        let mut source = world();
        let entity = source.spawn(Position(1));
        let mut peer = world();
        peer.spawn((Position(5), Velocity(5)));

        peer.restore(&source.snapshot()).expect("same registry");

        assert_eq!(peer.get::<Position>(entity), Some(&Position(1)));
        assert_eq!(peer.query::<&Position>().iter().count(), 1);
    }

    #[test]
    fn a_snapshot_from_another_registry_is_rejected() {
        let snapshot = world().snapshot();
        let mut builder = World::builder();
        builder.register::<Velocity>().register::<Position>();
        let mut other = builder.build();

        assert_eq!(
            other.restore(&snapshot),
            Err(EcsError::IncompatibleSnapshot)
        );
    }

    #[test]
    fn restore_reuses_the_column_allocations() {
        let mut world = world();
        let entities: Vec<_> = (0..3).map(|value| world.spawn(Position(value))).collect();
        let snapshot = world.snapshot();
        world.restore(&snapshot).expect("same registry");
        let id = world.registry.id::<Position>();
        let allocation = |world: &World| {
            world.state.archetypes[1]
                .column::<Position>(id)
                .expect("the archetype stores positions")
                .as_ptr()
        };
        let before = allocation(&world);

        world.despawn(entities[0]).expect("the entity is alive");
        world.restore(&snapshot).expect("same registry");

        assert_eq!(allocation(&world), before);
    }

    #[test]
    fn an_archetype_created_after_the_snapshot_is_recreated_on_demand() {
        let mut world = world();
        let snapshot = world.snapshot();
        world.spawn((Position(1), Velocity(1)));
        world.restore(&snapshot).expect("same registry");

        let entity = world.spawn((Position(2), Velocity(2)));

        let items: Vec<_> = world.query::<(Entity, &Velocity)>().iter().collect();
        assert_eq!(items, [(entity, &Velocity(2))]);
    }
}
