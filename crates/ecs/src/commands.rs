//! Commands: structural changes recorded by systems and applied later.

use std::fmt;

use crate::access::Access;
use crate::entity::Allocator;
use crate::registry::Registry;
use crate::system::{SystemParam, WorldParts};
use crate::{Bundle, Component, Entity, World};

/// One structural change waiting to be applied.
type Command = Box<dyn FnOnce(&mut World)>;

/// Commands waiting to be applied, in recording order.
#[doc(hidden)]
#[derive(Default)]
pub struct CommandBuffer {
    commands: Vec<Command>,
}

impl CommandBuffer {
    /// Applies every command in recording order, keeping the capacity of
    /// the buffer.
    pub(crate) fn apply(&mut self, world: &mut World) {
        for command in self.commands.drain(..) {
            command(world);
        }
    }
}

impl fmt::Debug for CommandBuffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandBuffer")
            .field("len", &self.commands.len())
            .finish()
    }
}

/// A system parameter recording structural changes: spawning, despawning,
/// inserting and removing components. The schedule applies them after the
/// system, in system execution order.
///
/// Commands never create a conflict between systems: they are recorded,
/// then applied later at a deterministic point.
///
/// A command on an entity that is no longer alive when the command is
/// applied does nothing.
pub struct Commands<'w> {
    registry: &'w Registry,
    allocator: &'w mut Allocator,
    buffer: &'w mut CommandBuffer,
}

impl Commands<'_> {
    /// Spawns an entity with `bundle` and returns it at once. The entity is
    /// reserved now and stored when the command is applied; until then,
    /// queries do not see it.
    ///
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        check_bundle::<B>(self.registry);
        let entity = self.allocator.allocate();
        self.buffer
            .commands
            .push(Box::new(move |world: &mut World| {
                world.spawn_at(entity, bundle);
            }));
        entity
    }

    /// Despawns `entity`.
    pub fn despawn(&mut self, entity: Entity) {
        self.buffer
            .commands
            .push(Box::new(move |world: &mut World| {
                // A stale entity is ignored, see the type documentation.
                world.despawn(entity).ok();
            }));
    }

    /// Adds the components of `bundle` to `entity`, overwriting those it
    /// already has.
    ///
    /// # Panics
    ///
    /// Panics when a component is not registered or appears twice.
    pub fn insert<B: Bundle>(&mut self, entity: Entity, bundle: B) {
        check_bundle::<B>(self.registry);
        self.buffer
            .commands
            .push(Box::new(move |world: &mut World| {
                world.insert(entity, bundle).ok();
            }));
    }

    /// Removes the `T` component of `entity`.
    ///
    /// # Panics
    ///
    /// Panics when `T` is not registered.
    pub fn remove<T: Component>(&mut self, entity: Entity) {
        self.registry.id::<T>();
        self.buffer
            .commands
            .push(Box::new(move |world: &mut World| {
                world.remove::<T>(entity).ok();
            }));
    }
}

/// Checks a bundle when its command is recorded, so a panic points at the
/// system recording it.
fn check_bundle<B: Bundle>(registry: &Registry) {
    let mut ids = Vec::new();
    B::component_ids(registry, &mut ids);
    registry.sorted_without_duplicates(ids, "bundle");
}

impl SystemParam for Commands<'_> {
    type State = ();
    type Item<'w, 's> = Commands<'w>;

    fn init(_: &World, access: &mut Access, system: &'static str) {
        access.add_commands(system);
    }

    fn get<'w>(_state: &mut (), parts: &mut WorldParts<'w>) -> Commands<'w> {
        let (allocator, buffer) = parts
            .commands
            .take()
            .expect("the access check allows one commands parameter per system");
        Commands {
            registry: parts.registry,
            allocator,
            buffer,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::system::tests::run;
    use crate::{Commands, Component, Entity, Query, World};

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Position(i32);
    impl Component for Position {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Velocity(i32);
    impl Component for Velocity {}

    fn world() -> World {
        let mut builder = World::builder();
        builder.register::<Position>().register::<Velocity>();
        builder.build()
    }

    #[test]
    fn a_spawned_entity_is_stored_when_its_command_is_applied() {
        let mut world = world();

        run(
            &mut world,
            |mut commands: Commands, positions: Query<&Position>| {
                let entity = commands.spawn(Position(1));
                assert_eq!(positions.get(entity), None);
            },
            1,
        );

        let stored: Vec<_> = world.query::<&Position>().iter().collect();
        assert_eq!(stored, [&Position(1)]);
    }

    #[test]
    fn commands_apply_in_recording_order() {
        let mut world = world();
        let entity = world.spawn(Position(1));

        run(
            &mut world,
            move |mut commands: Commands| {
                commands.insert(entity, Velocity(2));
                commands.remove::<Position>(entity);
                commands.insert(entity, Position(3));
            },
            1,
        );

        assert_eq!(world.get::<Position>(entity), Some(&Position(3)));
        assert_eq!(world.get::<Velocity>(entity), Some(&Velocity(2)));
    }

    #[test]
    fn a_command_on_a_stale_entity_is_ignored() {
        let mut world = world();
        let entity = world.spawn(Position(1));

        run(
            &mut world,
            move |mut commands: Commands| {
                commands.despawn(entity);
                commands.despawn(entity);
                commands.insert(entity, Velocity(1));
                commands.remove::<Position>(entity);
            },
            1,
        );

        assert!(!world.contains(entity));
    }

    #[test]
    fn spawning_then_despawning_in_one_run_frees_the_index() {
        let mut world = world();

        run(
            &mut world,
            |mut commands: Commands| {
                let entity = commands.spawn(Position(1));
                commands.despawn(entity);
            },
            1,
        );

        assert_eq!(world.query::<Entity>().iter().count(), 0);
        assert_eq!(world.spawn(()).to_string(), "0v1");
    }

    #[test]
    #[should_panic(expected = "is not registered")]
    fn recording_an_unregistered_component_panics() {
        #[derive(Clone, Copy)]
        struct Unknown;
        impl Component for Unknown {}

        run(
            &mut world(),
            |mut commands: Commands| {
                commands.spawn(Unknown);
            },
            1,
        );
    }
}
