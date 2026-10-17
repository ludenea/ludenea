//! Ludenea, a deterministic 2D game engine.

pub use ludenea_app as app;
pub use ludenea_cli as cli;
pub use ludenea_control as control;
pub use ludenea_ecs as ecs;
pub use ludenea_math as math;
pub use ludenea_physics as physics;
pub use ludenea_schema as schema;
pub use serde;

pub mod prelude {
    pub use crate::app::{App, Input, InputLog, Inputs, Plugin, Rng, Sim, Simulation, Time};
    pub use crate::ecs::{
        Commands, Component, Entity, EventWriter, Events, IntoSystemConfig, Query, Res, ResMut,
        Resource, Runtime, SystemLabel, With, Without, World,
    };
    pub use crate::math::{Aabb, Rot, Transform, Vec2};
    pub use crate::physics::{
        Body, BodyKind, Capsule, Circle, Collider, Collision, CollisionKind, PhysicsPlugin,
        PhysicsSettings, Polygon, Pose, RayHit, Shape, overlap_aabb, raycast,
    };
    pub use crate::schema::Schema;
}
