//! Ludenea, a deterministic 2D game engine.

pub use ludenea_app as app;
pub use ludenea_ecs as ecs;
pub use ludenea_math as math;
pub use serde;

pub mod prelude {
    pub use crate::app::{App, Input, InputLog, Inputs, Plugin, Rng, Sim, Simulation, Time};
    pub use crate::ecs::{
        Commands, Component, Entity, EventWriter, Events, IntoSystemConfig, Query, Res, ResMut,
        Resource, Runtime, RuntimeMut, SystemLabel, With, Without, World,
    };
    pub use crate::math::{Aabb, Rot, Transform, Vec2};
}
