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
