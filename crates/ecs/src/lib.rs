//! Entity component system for the Ludenea engine.
//!
//! A [`World`] stores entities and their components in archetype tables.

mod archetype;
mod bundle;
mod column;
mod entity;
mod error;
mod query;
mod registry;
mod world;

pub use bundle::Bundle;
pub use entity::Entity;
pub use error::EcsError;
pub use query::{Query, QueryData, ReadOnlyQuery};
pub use world::{World, WorldBuilder};

/// Marker for the types a [`World`] stores as components.
///
/// `Copy` is what makes a snapshot a memory copy. Implement it explicitly:
///
/// ```
/// use ludenea_ecs::Component;
///
/// #[derive(Clone, Copy)]
/// struct Position(f32, f32);
///
/// impl Component for Position {}
/// ```
pub trait Component: Copy + 'static {}
