//! Structural changes deferred until the issuing system returns, then applied in issue
//! order. Commands on a dead entity are ignored.

use std::cell::RefCell;

use crate::observer::{CommandKind, CommandRecord};
use crate::system::{Accesses, Missing, SystemParam};
use crate::{Component, EcsError, Entity, World};

/// Applies itself to the world and says what it did; the second argument holds the
/// entities spawned so far by this queue, in spawn order.
type Command = Box<dyn FnOnce(&mut World, &mut Vec<Entity>) -> CommandRecord + Send>;

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
        if let Some(records) = records.as_deref_mut() {
            records.push(record);
        }
    }
}

fn record(kind: CommandKind, entity: Entity, component: Option<&'static str>) -> CommandRecord {
    CommandRecord {
        kind,
        entity: Some(entity),
        component,
    }
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
            record(CommandKind::Spawn, entity, None)
        }));
        SpawnCommands {
            queue: self.queue,
            spawn,
        }
    }

    pub fn despawn(&mut self, entity: Entity) {
        // Despawn fails only on a dead entity, which is ignored.
        self.push(move |world| {
            drop(world.despawn(entity));
            record(CommandKind::Despawn, entity, None)
        });
    }

    /// # Panics
    /// When applied, if `T` is not registered.
    pub fn insert<T: Component>(&mut self, entity: Entity, component: T) {
        self.push(move |world| {
            ignore_dead(world.insert(entity, component));
            record(CommandKind::Insert, entity, Some(T::NAME))
        });
    }

    /// # Panics
    /// When applied, if `T` is not registered.
    pub fn remove<T: Component>(&mut self, entity: Entity) {
        self.push(move |world| {
            ignore_dead(world.remove::<T>(entity).map(drop));
            record(CommandKind::Remove, entity, Some(T::NAME))
        });
    }

    fn push(&mut self, command: impl FnOnce(&mut World) -> CommandRecord + Send + 'static) {
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
                ignore_dead(world.insert(entity, component));
                record(CommandKind::Insert, entity, Some(T::NAME))
            }));
        self
    }
}

/// An unregistered component is a programming error, so it panics with its name.
fn ignore_dead(result: Result<(), EcsError>) {
    match result {
        Ok(()) | Err(EcsError::NoSuchEntity(_)) => {}
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
