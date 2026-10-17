//! The world: every entity, its components, and the operations on them.

use std::collections::BTreeMap;

use crate::archetype::{Archetype, ArchetypeId};
use crate::entity::Allocator;
use crate::registry::{ComponentId, Registry};
use crate::{Bundle, Component, EcsError, Entity};

/// Registers the components of a [`World`] before building it.
#[derive(Default)]
pub struct WorldBuilder {
    registry: Registry,
}

impl WorldBuilder {
    /// Registers `T`. Component identifiers follow registration order, so
    /// every peer of a session must register the same components in the same
    /// order. Registering a component twice keeps its first identifier.
    pub fn register<T: Component>(&mut self) -> &mut Self {
        self.registry.register::<T>();
        self
    }

    /// Builds the world, freezing its registry.
    #[must_use]
    pub fn build(self) -> World {
        let empty = Archetype::new(Vec::new(), &self.registry);
        World {
            registry: self.registry,
            state: State {
                allocator: Allocator::default(),
                locations: Vec::new(),
                archetypes: vec![empty],
                archetype_ids: BTreeMap::from([(Vec::new(), ArchetypeId::EMPTY)]),
            },
        }
    }
}

/// Where a live entity is stored.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Location {
    pub(crate) archetype: ArchetypeId,
    pub(crate) row: usize,
}

/// The mutable state of a world.
#[derive(Debug)]
pub(crate) struct State {
    pub(crate) allocator: Allocator,
    /// Location of every entity index, `None` when no entity uses it.
    pub(crate) locations: Vec<Option<Location>>,
    /// Every archetype, in creation order.
    pub(crate) archetypes: Vec<Archetype>,
    pub(crate) archetype_ids: BTreeMap<Vec<ComponentId>, ArchetypeId>,
}

/// Every entity and its components, stored in archetype tables.
///
/// Every operation is deterministic: the same sequence of operations on two
/// worlds with the same registry yields the same entity identifiers and the
/// same iteration order.
pub struct World {
    pub(crate) registry: Registry,
    pub(crate) state: State,
}

impl World {
    #[must_use]
    pub fn builder() -> WorldBuilder {
        WorldBuilder::default()
    }

    /// Spawns an entity with the components of `bundle`.
    ///
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice, or when
    /// entity indices are exhausted.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let mut ids = Vec::new();
        B::component_ids(&self.registry, &mut ids);
        let archetype = self.archetype_for(self.sorted_key(ids));
        let entity = self.state.allocator.allocate();
        let row = self.state.archetypes[archetype.0].push_entity(entity);
        bundle.write(&self.registry, &mut self.state.archetypes[archetype.0], row);
        if entity.index() == self.state.locations.len() {
            self.state.locations.push(None);
        }
        self.state.locations[entity.index()] = Some(Location { archetype, row });
        entity
    }

    /// Despawns `entity` with all its components.
    ///
    /// # Errors
    ///
    /// Returns [`EcsError::StaleEntity`] when `entity` is not alive.
    pub fn despawn(&mut self, entity: Entity) -> Result<(), EcsError> {
        let location = self.locate(entity).ok_or(EcsError::StaleEntity(entity))?;
        let archetype = &mut self.state.archetypes[location.archetype.0];
        if let Some(moved) = archetype.swap_remove(location.row) {
            self.state.locations[moved.index()] = Some(location);
        }
        self.state.locations[entity.index()] = None;
        self.state.allocator.free(entity);
        Ok(())
    }

    /// Returns the `T` component of `entity`, or `None` when the entity is
    /// not alive or has no `T`.
    ///
    /// # Panics
    ///
    /// Panics when `T` is not registered.
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        let id = self.registry.id::<T>();
        let location = self.locate(entity)?;
        self.state.archetypes[location.archetype.0]
            .column::<T>(id)
            .map(|column| &column[location.row])
    }

    /// Returns the `T` component of `entity` for writing, or `None` when the
    /// entity is not alive or has no `T`.
    ///
    /// # Panics
    ///
    /// Panics when `T` is not registered.
    #[must_use]
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        let id = self.registry.id::<T>();
        let location = self.locate(entity)?;
        self.state.archetypes[location.archetype.0]
            .column_mut::<T>(id)
            .map(|column| &mut column[location.row])
    }

    /// Whether `entity` is alive in this world.
    #[must_use]
    pub fn contains(&self, entity: Entity) -> bool {
        self.locate(entity).is_some()
    }

    /// Finds a live entity. The entity stored at the location must be
    /// `entity` itself, which rules out stale generations and identifiers
    /// from another world.
    fn locate(&self, entity: Entity) -> Option<Location> {
        let location = (*self.state.locations.get(entity.index())?)?;
        let stored = self.state.archetypes[location.archetype.0].entities()[location.row];
        (stored == entity).then_some(location)
    }

    /// Sorts the component identifiers of a bundle into an archetype key.
    ///
    /// # Panics
    ///
    /// Panics when a component appears twice.
    fn sorted_key(&self, mut ids: Vec<ComponentId>) -> Vec<ComponentId> {
        ids.sort_unstable();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            panic!(
                "component {} appears twice in the bundle",
                self.registry.name(pair[0])
            );
        }
        ids
    }

    /// Returns the archetype of `key`, creating it when needed.
    // ponytail: one map lookup per structural change, add a transition cache
    // between archetypes when benchmarks ask for it.
    fn archetype_for(&mut self, key: Vec<ComponentId>) -> ArchetypeId {
        if let Some(&id) = self.state.archetype_ids.get(&key) {
            return id;
        }
        let id = ArchetypeId(self.state.archetypes.len());
        self.state
            .archetypes
            .push(Archetype::new(key.clone(), &self.registry));
        self.state.archetype_ids.insert(key, id);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Position(i32);
    impl Component for Position {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Velocity(i32);
    impl Component for Velocity {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Frozen;
    impl Component for Frozen {}

    fn world() -> World {
        let mut builder = World::builder();
        builder
            .register::<Position>()
            .register::<Velocity>()
            .register::<Frozen>();
        builder.build()
    }

    #[test]
    fn spawn_stores_the_components_of_the_bundle() {
        let mut world = world();

        let entity = world.spawn((Position(1), Velocity(2)));

        assert_eq!(world.get::<Position>(entity), Some(&Position(1)));
        assert_eq!(world.get::<Velocity>(entity), Some(&Velocity(2)));
        assert_eq!(world.get::<Frozen>(entity), None);
    }

    #[test]
    fn spawn_accepts_a_single_component_and_the_empty_bundle() {
        let mut world = world();

        let single = world.spawn(Position(1));
        let empty = world.spawn(());

        assert_eq!(world.get::<Position>(single), Some(&Position(1)));
        assert!(world.contains(empty));
    }

    #[test]
    fn zero_sized_components_are_stored_like_any_other() {
        let mut world = world();

        let entity = world.spawn((Frozen, Position(1)));

        assert_eq!(world.get::<Frozen>(entity), Some(&Frozen));
    }

    #[test]
    #[should_panic(expected = "Position appears twice in the bundle")]
    fn a_bundle_with_a_duplicate_component_panics() {
        world().spawn((Position(1), Position(2)));
    }

    #[test]
    #[should_panic(expected = "is not registered")]
    fn spawning_an_unregistered_component_panics() {
        #[derive(Clone, Copy)]
        struct Unknown;
        impl Component for Unknown {}

        world().spawn(Unknown);
    }

    #[test]
    fn get_mut_writes_the_component_in_place() {
        let mut world = world();
        let entity = world.spawn(Position(1));

        *world
            .get_mut::<Position>(entity)
            .expect("the entity has a position") = Position(5);

        assert_eq!(world.get::<Position>(entity), Some(&Position(5)));
    }

    #[test]
    fn despawn_makes_the_entity_stale() {
        let mut world = world();
        let entity = world.spawn(Position(1));

        world.despawn(entity).expect("the entity is alive");

        assert!(!world.contains(entity));
        assert_eq!(world.get::<Position>(entity), None);
        assert_eq!(world.despawn(entity), Err(EcsError::StaleEntity(entity)));
    }

    #[test]
    fn despawn_keeps_the_entity_moved_into_the_hole_reachable() {
        let mut world = world();
        let first = world.spawn(Position(1));
        let last = world.spawn(Position(2));

        world.despawn(first).expect("the entity is alive");

        assert_eq!(world.get::<Position>(last), Some(&Position(2)));
    }

    #[test]
    fn a_reused_index_does_not_revive_the_stale_entity() {
        let mut world = world();
        let stale = world.spawn(Position(1));
        world.despawn(stale).expect("the entity is alive");

        let reused = world.spawn(Position(2));

        assert_ne!(reused, stale);
        assert_eq!(world.get::<Position>(stale), None);
    }

    #[test]
    fn an_entity_from_a_larger_world_is_not_alive_here() {
        let mut larger = world();
        let entities: Vec<Entity> = (0..3).map(|_| larger.spawn(())).collect();
        let mut smaller = world();
        smaller.spawn(());

        assert!(!smaller.contains(entities[2]));
        assert_eq!(smaller.get::<Position>(entities[2]), None);
    }
}
