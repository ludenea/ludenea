//! Deterministic entity component system: generational entities, components stored by
//! entity index, and resources, all registered under stable names.

extern crate self as ludenea_ecs;

pub mod component;
pub mod diff;
pub mod entity;
pub mod error;
pub mod hash;
pub mod snapshot;
pub mod world;

pub use component::{Codec, Component, DecodeResult, Resource};
pub use diff::Difference;
pub use entity::{EntitiesState, Entity};
pub use error::EcsError;
pub use hash::{EntityHash, HashTree, NamedHash, hash_value};
pub use ludenea_macros::{Component, Resource};
pub use snapshot::{ComponentValues, SNAPSHOT_VERSION, Snapshot, SnapshotError};
pub use world::World;
