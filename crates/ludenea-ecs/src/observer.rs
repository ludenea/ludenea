//! Read-only hooks into a schedule run, for the editor's ECS explorer. An observer never
//! receives a mutable world, so observing cannot change the simulation or its hashes.

use crate::{Entity, World};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CommandKind {
    Spawn,
    Despawn,
    Insert,
    Remove,
}

/// One structural command a system issued, as applied right after it returned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CommandRecord {
    pub kind: CommandKind,
    /// The entity spawned, despawned, or changed.
    pub entity: Option<Entity>,
    /// The stable name of the inserted or removed component.
    pub component: Option<&'static str>,
}

/// Watches `Schedule::run_observed`. `index` is the system's position in run order, as in
/// `Schedule::systems`.
pub trait ScheduleObserver {
    fn before_system(&mut self, index: usize);
    /// Called after the system ran and its commands were applied.
    fn after_system(&mut self, index: usize, commands: &[CommandRecord]);
    /// Called after the last system, while this tick's events are still readable.
    fn tick_end(&mut self, world: &World);
}
