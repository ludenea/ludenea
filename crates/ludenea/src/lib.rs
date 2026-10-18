//! Ludenea, a deterministic 2D game engine.

pub use ludenea_app as app;
pub use ludenea_cli as cli;
pub use ludenea_control as control;
pub use ludenea_debug_draw as debug_draw;
pub use ludenea_ecs as ecs;
pub use ludenea_math as math;
pub use ludenea_physics as physics;
pub use ludenea_schema as schema;
pub use serde;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use wasm_bindgen;

/// Exports `createHost`, the entry point of a game's browser build: a `WorkerHost` running
/// the given `fn() -> App`. Expands to nothing outside the browser.
#[macro_export]
macro_rules! worker_entry {
    ($app:path) => {
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        #[$crate::wasm_bindgen::prelude::wasm_bindgen(wasm_bindgen = $crate::wasm_bindgen, js_name = createHost)]
        pub fn create_host() -> $crate::control::worker::WorkerHost {
            $crate::control::worker::WorkerHost::new($app)
        }
    };
}

pub mod prelude {
    pub use crate::app::{
        App, Edit, EditLog, Input, InputLog, Inputs, Plugin, RawInput, Rng, Sim, Simulation, Time,
        TimedEdit,
    };
    pub use crate::ecs::{
        Asset, Assets, Commands, Component, Entity, EventWriter, Events, Handle, IntoSystemConfig,
        Prefab, Query, Res, ResMut, Resource, Runtime, SystemLabel, With, Without, World,
    };
    pub use crate::math::{Aabb, Rot, Transform, Vec2};
    pub use crate::physics::{
        Body, BodyKind, Capsule, Circle, Collider, Collision, CollisionKind, PhysicsPlugin,
        PhysicsSettings, Polygon, Pose, RayHit, Shape, overlap_aabb, raycast,
    };
    pub use crate::schema::Schema;
}
