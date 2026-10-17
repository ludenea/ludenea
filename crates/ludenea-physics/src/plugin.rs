use ludenea_app::{App, Plugin, Sim};
use ludenea_ecs::{IntoSystemConfig, SystemLabel};

use crate::components::{Body, Collider, PhysicsSettings, Pose};
use crate::contacts::ContactCache;
use crate::step::physics_step;

/// Rigid body physics: registers the physics components and resources and runs one
/// physics step per tick in the `Step` set.
pub struct PhysicsPlugin;

/// The set holding the physics step, for ordering game systems around it.
pub struct Step;

impl SystemLabel for Step {
    fn name(&self) -> &'static str {
        "physics::Step"
    }
}

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.component::<Pose>()
            .component::<Body>()
            .component::<Collider>()
            .resource(PhysicsSettings::default())
            .resource(ContactCache::default())
            .system(Sim, physics_step.in_set(Step));
    }
}
