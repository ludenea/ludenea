//! Deterministic entity component system: generational entities, components stored by
//! entity index, and resources, all registered under stable names.

extern crate self as ludenea_ecs;

pub mod asset;
pub mod assets;
pub mod commands;
pub mod component;
pub mod diff;
pub mod entity;
pub mod error;
pub mod event;
pub mod filter;
pub mod hash;
pub mod observer;
pub mod prefab;
pub mod query;
pub mod saved;
pub mod schedule;
pub mod snapshot;
pub mod system;
pub mod world;

pub use asset::{Asset, AssetId, Handle};
pub use assets::Assets;
pub use commands::{Commands, SpawnCommands};
pub use component::{Codec, Component, DecodeResult, Resource, decode_exact};
pub use diff::Difference;
pub use entity::{EntitiesState, Entity};
pub use error::EcsError;
pub use event::{Event, EventWriter, Events};
pub use filter::{EntityFilter, FilterError};
pub use hash::{EntityHash, HashTree, NamedHash, hash_value};
pub use ludenea_macros::{Asset, Component, Event, Query, Resource};
pub use ludenea_schema as schema;
pub use observer::{CommandKind, CommandRecord, ScheduleObserver};
pub use prefab::{Prefab, PrefabEntity, PrefabOverrides};
pub use query::{
    Access, Query, QueryData, QueryError, QueryFilter, ReadOnlyQueryData, With, Without, accesses,
    check_accesses,
};
pub use saved::{SavedError, SavedWorld};
pub use schedule::{
    BuildError, IntoSystemConfig, Schedule, ScheduleBuilder, SystemConfig, SystemInfo, SystemLabel,
};
pub use snapshot::{ComponentValues, SNAPSHOT_VERSION, Snapshot, SnapshotError};
pub use system::{IntoSystem, Res, ResMut, Runtime, System, SystemParam};
pub use world::{World, check_not_recursive};
