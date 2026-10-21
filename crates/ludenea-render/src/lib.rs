//! Rendering: the simulated render components (sprites, their animation, the camera and
//! tilemaps), the `animate` system, the extraction of a world into a render frame, atlas
//! packing and the frames a texture's meta cuts, the camera's math, and picking (what a point
//! on screen hits, what a selection spans). All of it depends on no GPU crate, so the
//! simulation never pulls one; the wgpu renderer is `gpu`, behind the feature of the same
//! name, which interpolates between two frames on the GPU, and `web` (feature `web`, browser
//! only) draws with it into a canvas.

pub mod animate;
pub mod atlas;
pub mod camera;
pub mod components;
pub mod extract;
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "gpu")]
mod interpolate;
pub mod pick;
pub mod sheet;
#[cfg(all(feature = "web", target_arch = "wasm32", target_os = "unknown"))]
pub mod web;

use ludenea_app::pose::Pose;
use ludenea_app::{App, Plugin, Sim};
use ludenea_ecs::{IntoSystemConfig, SystemLabel};

pub use animate::animate;
pub use components::{
    Camera2d, CameraError, Sprite, SpriteAnimation, Teleported, Tilemap, TilemapError,
};
pub use extract::extract;
pub use sheet::{AnimationError, Clip, SheetError, SpriteSheet};

/// Registers the render components, `Pose`, the `SpriteSheet` asset, the `animate` system in
/// the `Animate` set, and the extractor that fills each render frame. `animate` runs after
/// `physics::Step` when the app has it; without it, `animate` runs at its registration slot.
pub struct RenderPlugin;

/// The set holding the `animate` system. A system that switches a `SpriteAnimation`'s clip
/// declares `.before(Animate)` so the sprite shows the new clip in the same tick; ordered
/// after it, the sprite shows the new clip one tick later.
pub struct Animate;

impl SystemLabel for Animate {
    fn name(&self) -> &'static str {
        "render::Animate"
    }
}

/// The physics step's set, matched by its name so this crate does not depend on the physics.
struct PhysicsStep;

impl SystemLabel for PhysicsStep {
    fn name(&self) -> &'static str {
        "physics::Step"
    }
}

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.component::<Pose>()
            .component::<Sprite>()
            .component::<SpriteAnimation>()
            .component::<Camera2d>()
            .component::<Tilemap>()
            .component::<Teleported>()
            .asset::<SpriteSheet>()
            .system(Sim, animate.in_set(Animate).after_if_present(PhysicsStep))
            .extractor(extract);
    }
}
