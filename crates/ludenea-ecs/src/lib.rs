//! Deterministic entity component system: generational entities, components stored by
//! entity index, and resources, all registered under stable names.

extern crate self as ludenea_ecs;

pub mod commands;
pub mod component;
pub mod diff;
pub mod entity;
pub mod error;
pub mod event;
pub mod hash;
pub mod query;
pub mod schedule;
pub mod snapshot;
pub mod system;
pub mod world;

pub use commands::{Commands, SpawnCommands};
pub use component::{Codec, Component, DecodeResult, Resource};
pub use diff::Difference;
pub use entity::{EntitiesState, Entity};
pub use error::EcsError;
pub use event::{EventWriter, Events};
pub use hash::{EntityHash, HashTree, NamedHash, hash_value};
pub use ludenea_macros::{Component, Query, Resource};
pub use query::{
    Access, Query, QueryData, QueryError, QueryFilter, ReadOnlyQueryData, With, Without, accesses,
    check_accesses,
};
pub use schedule::{
    BuildError, IntoSystemConfig, Schedule, ScheduleBuilder, SystemConfig, SystemLabel,
};
pub use snapshot::{ComponentValues, SNAPSHOT_VERSION, Snapshot, SnapshotError};
pub use system::{IntoSystem, Res, ResMut, Runtime, RuntimeMut, System, SystemParam};
pub use world::World;
