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
//!
//! Systems are functions whose parameters borrow parts of the world. A
//! [`Schedule`] runs them in a deterministic order and rejects, when it is
//! built, two systems with conflicting access left unordered:
//!
//! ```
//! use ludenea_ecs::{
//!     Commands, Component, Entity, IntoSystemConfig, Query, Res, Resource, Schedule,
//!     ScheduleError, World,
//! };
//!
//! #[derive(Clone, Copy, Debug, PartialEq)]
//! struct Position(i32);
//! impl Component for Position {}
//!
//! #[derive(Clone, Copy)]
//! struct Velocity(i32);
//! impl Component for Velocity {}
//!
//! #[derive(Clone, Copy)]
//! struct Gravity(i32);
//! impl Resource for Gravity {}
//!
//! fn apply_gravity(mut bodies: Query<&mut Velocity>, gravity: Res<Gravity>) {
//!     for velocity in bodies.iter_mut() {
//!         velocity.0 += gravity.0;
//!     }
//! }
//!
//! fn integrate(mut bodies: Query<(&mut Position, &Velocity)>) {
//!     for (position, velocity) in bodies.iter_mut() {
//!         position.0 += velocity.0;
//!     }
//! }
//!
//! fn remove_fallen(mut commands: Commands, bodies: Query<(Entity, &Position)>) {
//!     for (entity, position) in bodies.iter() {
//!         if position.0 < 0 {
//!             commands.despawn(entity);
//!         }
//!     }
//! }
//!
//! let mut builder = World::builder();
//! builder
//!     .register::<Position>()
//!     .register::<Velocity>()
//!     .insert_resource(Gravity(-1));
//! let mut world = builder.build();
//! let body = world.spawn((Position(1), Velocity(0)));
//!
//! let mut ambiguous = Schedule::builder();
//! ambiguous.add(apply_gravity).add(integrate);
//! assert!(matches!(
//!     ambiguous.build(&world),
//!     Err(ScheduleError::Ambiguous(_))
//! ));
//!
//! let mut schedule = Schedule::builder();
//! schedule
//!     .add(apply_gravity)
//!     .add(integrate.after(apply_gravity))
//!     .add(remove_fallen.after(integrate));
//! let mut schedule = schedule.build(&world).unwrap();
//!
//! schedule.run(&mut world);
//! assert_eq!(world.get::<Position>(body), Some(&Position(0)));
//! schedule.run(&mut world);
//! assert!(!world.contains(body));
//! ```

mod access;
mod archetype;
mod bundle;
mod column;
mod commands;
mod entity;
mod error;
mod query;
mod query_state;
mod registry;
mod resource;
mod schedule;
mod snapshot;
mod system;
mod world;

pub use access::Conflict;
pub use bundle::Bundle;
pub use commands::Commands;
pub use entity::Entity;
pub use error::{Ambiguity, EcsError, ScheduleError};
pub use query::{QueryData, QueryFilter, ReadOnlyQuery, With, Without, WorldQuery};
pub use query_state::Query;
pub use resource::{Res, ResMut, Resource};
pub use schedule::{
    IntoSystemConfig, Schedule, ScheduleBuilder, SetOrder, SystemConfig, SystemSet,
};
pub use snapshot::Snapshot;
pub use system::{IntoSystem, SystemParam};
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
