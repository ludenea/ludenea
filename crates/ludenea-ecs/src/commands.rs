//! Structural changes deferred until the issuing system returns, then applied in issue
//! order. Commands on a dead entity are ignored, and observers do not see them.

use std::cell::RefCell;

use crate::observer::{CommandKind, CommandRecord};
use crate::system::{Accesses, Missing, SystemParam};
use crate::{Component, EcsError, Entity, World};

/// Applies itself to the world and says what it did, or `None` when it was ignored; the
/// second argument holds the entities spawned so far by this queue, in spawn order.
type Command = Box<dyn FnOnce(&mut World, &mut Vec<Entity>) -> Option<CommandRecord> + Send>;

#[derive(Default)]
pub(crate) struct CommandQueue {
    commands: Vec<Command>,
    spawns: usize,
}

/// Applies and drops every queued command, pushing what each did to `records` if given.
pub(crate) fn apply(world: &mut World, mut records: Option<&mut Vec<CommandRecord>>) {
    let queue = std::mem::take(world.commands.get_mut());
    let mut spawned = Vec::with_capacity(queue.spawns);
    for command in queue.commands {
        let record = command(world, &mut spawned);
        if let (Some(records), Some(record)) = (records.as_deref_mut(), record) {
            records.push(record);
        }
    }
}

/// The record of a command, or `None` when `applied` is false.
fn record(
    applied: bool,
    kind: CommandKind,
    entity: Entity,
    component: Option<&'static str>,
) -> Option<CommandRecord> {
    applied.then_some(CommandRecord {
        kind,
        entity: Some(entity),
        component,
    })
}

/// Queues structural changes. Every call borrows the queue only while it pushes, so a
/// system may take several `Commands`.
pub struct Commands<'w> {
    queue: &'w RefCell<CommandQueue>,
}

impl<'w> Commands<'w> {
    /// Queues the spawn of an entity, whose components `SpawnCommands::insert` adds.
    pub fn spawn(&mut self) -> SpawnCommands<'w> {
        let mut queue = self.queue.borrow_mut();
        let spawn = queue.spawns;
        queue.spawns += 1;
        queue.commands.push(Box::new(|world, spawned| {
            let entity = world.spawn();
            spawned.push(entity);
            record(true, CommandKind::Spawn, entity, None)
        }));
        SpawnCommands {
            queue: self.queue,
            spawn,
        }
    }

    pub fn despawn(&mut self, entity: Entity) {
        // Despawn fails only on a dead entity, which is ignored.
        self.push(move |world| {
            let applied = world.despawn(entity).is_ok();
            record(applied, CommandKind::Despawn, entity, None)
        });
    }

    /// # Panics
    /// When applied, if `T` is not registered.
    pub fn insert<T: Component>(&mut self, entity: Entity, component: T) {
        self.push(move |world| {
            let applied = ignore_dead(world.insert(entity, component));
            record(applied, CommandKind::Insert, entity, Some(T::NAME))
        });
    }

    /// # Panics
    /// When applied, if `T` is not registered.
    pub fn remove<T: Component>(&mut self, entity: Entity) {
        self.push(move |world| {
            let applied = ignore_dead(world.remove::<T>(entity).map(drop));
            record(applied, CommandKind::Remove, entity, Some(T::NAME))
        });
    }

    fn push(&mut self, command: impl FnOnce(&mut World) -> Option<CommandRecord> + Send + 'static) {
        self.queue
            .borrow_mut()
            .commands
            .push(Box::new(move |world, _| command(world)));
    }
}

/// Adds components to an entity queued by `Commands::spawn`.
pub struct SpawnCommands<'w> {
    queue: &'w RefCell<CommandQueue>,
    /// Position of the entity among the spawns of the queue.
    spawn: usize,
}

impl SpawnCommands<'_> {
    /// # Panics
    /// When applied, if `T` is not registered.
    pub fn insert<T: Component>(self, component: T) -> Self {
        let spawn = self.spawn;
        self.queue
            .borrow_mut()
            .commands
            .push(Box::new(move |world, spawned| {
                let entity = spawned[spawn];
                let applied = ignore_dead(world.insert(entity, component));
                record(applied, CommandKind::Insert, entity, Some(T::NAME))
            }));
        self
    }
}

/// Whether the command applied: a dead entity is ignored. An unregistered component is a
/// programming error, so it panics with its name.
fn ignore_dead(result: Result<(), EcsError>) -> bool {
    match result {
        Ok(()) => true,
        Err(EcsError::NoSuchEntity(_)) => false,
        Err(error) => panic!("{error}"),
    }
}

impl SystemParam for Commands<'_> {
    type Item<'w> = Commands<'w>;

    fn declare(_: &World, _: &mut Accesses) -> Result<(), Missing> {
        Ok(())
    }

    fn fetch(world: &World) -> Self::Item<'_> {
        Commands {
            queue: &world.commands,
        }
    }
}
