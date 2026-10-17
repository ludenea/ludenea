//! Deterministic entity component system: generational entities, components stored by
//! entity index, and resources, all registered under stable names.

extern crate self as ludenea_ecs;

pub mod component;
pub mod entity;
pub mod error;
pub mod world;

pub use component::{Component, Resource};
pub use entity::{EntitiesState, Entity};
pub use error::EcsError;
pub use ludenea_macros::{Component, Resource};
pub use world::World;
