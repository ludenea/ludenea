//! Structural changes deferred until the issuing system returns, then applied in issue
//! order. Commands on a dead entity are ignored, and observers do not see them.

use std::cell::RefCell;

use crate::observer::{CommandKind, CommandRecord};
use crate::prefab::PrefabOverrides;
use crate::system::{Accesses, Missing, SystemParam};
use crate::{Component, EcsError, Entity, Handle, Prefab, World};

/// Applies itself to the world, reporting what it did to the third argument (nothing when
/// it was ignored); the second argument holds the entities spawned so far by this queue, in
/// spawn order.
type Command = Box<dyn FnOnce(&mut World, &mut Vec<Entity>, &mut dyn FnMut(CommandRecord)) + Send>;

#[derive(Default)]
pub(crate) struct CommandQueue {
    commands: Vec<Command>,
    spawns: usize,
}

/// Applies and drops every queued command, pushing what each did to `records` if given.
pub(crate) fn apply(world: &mut World, mut records: Option<&mut Vec<CommandRecord>>) {
    let queue = std::mem::take(world.commands.get_mut());
    let mut spawned = Vec::with_capacity(queue.spawns);
    let mut report = |record| {
        if let Some(records) = records.as_deref_mut() {
            records.push(record);
        }
    };
    for command in queue.commands {
        command(world, &mut spawned, &mut report);
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
        queue.commands.push(Box::new(|world, spawned, report| {
            let entity = world.spawn();
            spawned.push(entity);
            report(CommandRecord {
                kind: CommandKind::Spawn,
                entity: Some(entity),
                component: None,
            });
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

    /// Queues the spawn of every entity of `prefab`, in prefab order, with its `Entity`
    /// values pointing to the new entities, then applies `overrides` to them.
    ///
    /// # Panics
    /// When applied, if the prefab is not loaded, a component's type refuses the entities
    /// spawned for it, or an override names a local identifier it does not have.
    pub fn spawn_prefab(
        &mut self,
        prefab: Handle<Prefab>,
        overrides: impl FnOnce(&mut PrefabOverrides) + Send + 'static,
    ) {
        self.queue
            .borrow_mut()
            .commands
            .push(Box::new(move |world, _, report| {
                crate::prefab::spawn(world, prefab, overrides, report);
            }));
    }

    fn push(&mut self, command: impl FnOnce(&mut World) -> Option<CommandRecord> + Send + 'static) {
        self.queue
            .borrow_mut()
            .commands
            .push(Box::new(move |world, _, report| {
                if let Some(record) = command(world) {
                    report(record);
                }
            }));
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
            .push(Box::new(move |world, spawned, report| {
                let entity = spawned[spawn];
                if ignore_dead(world.insert(entity, component)) {
                    report(CommandRecord {
                        kind: CommandKind::Insert,
                        entity: Some(entity),
                        component: Some(T::NAME),
                    });
                }
            }));
        self
    }
}

/// Whether the command applied: a dead entity is ignored. An unregistered component is a
/// programming error, so it panics with its name.
pub(crate) fn ignore_dead(result: Result<(), EcsError>) -> bool {
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
