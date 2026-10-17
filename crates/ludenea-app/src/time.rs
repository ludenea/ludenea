use ludenea_ecs::Resource;
use serde::{Deserialize, Serialize};

/// The fixed simulation clock: `tick` counts completed steps, `dt` is the step length in
/// seconds (`1 / tick_rate`).
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[resource(name = "ludenea::Time")]
pub struct Time {
    pub tick: u64,
    pub dt: f32,
}
