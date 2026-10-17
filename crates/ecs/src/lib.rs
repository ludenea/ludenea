//! Entity component system for the Ludenea engine.
//!
//! A [`World`] stores entities and their components in archetype tables. Its
//! whole state can be captured in a [`Snapshot`] and restored with memory
//! copies, and a restored world yields the same entity identifiers and the
//! same iteration order as the original.
//!
//! ```
//! use ludenea_ecs::{Component, Entity, World};
//!
//! #[derive(Clone, Copy, Debug, PartialEq)]
//! struct Position(i32);
//! impl Component for Position {}
//!
//! #[derive(Clone, Copy)]
//! struct Velocity(i32);
//! impl Component for Velocity {}
//!
//! let mut builder = World::builder();
//! builder.register::<Position>().register::<Velocity>();
//! let mut world = builder.build();
//!
//! let player = world.spawn((Position(0), Velocity(2)));
//!
//! for (position, velocity) in world.query_mut::<(&mut Position, &Velocity)>().iter() {
//!     position.0 += velocity.0;
//! }
//! let entities: Vec<Entity> = world.query::<Entity>().iter().collect();
//! assert_eq!(entities, [player]);
//!
//! let snapshot = world.snapshot();
//! world.despawn(player).unwrap();
//! world.restore(&snapshot).unwrap();
//! assert_eq!(world.get::<Position>(player), Some(&Position(2)));
//! ```

mod archetype;
mod bundle;
mod column;
mod entity;
mod error;
mod query;
mod registry;
mod resource;
mod snapshot;
mod world;

pub use bundle::Bundle;
pub use entity::Entity;
pub use error::EcsError;
pub use query::{Query, QueryData, ReadOnlyQuery};
pub use resource::Resource;
pub use snapshot::Snapshot;
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
