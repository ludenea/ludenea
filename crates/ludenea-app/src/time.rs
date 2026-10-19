use ludenea_ecs::Resource;
use serde::{Deserialize, Serialize};

/// Shortest step, in seconds: the step of the fastest tick rate, `u32::MAX` Hz. A step far
/// shorter rounds the physics substep to zero.
const MIN_DT: f32 = 1.0 / u32::MAX as f32;

/// The fixed simulation clock: `tick` counts completed steps, `dt` is the step length in
/// seconds (`1 / tick_rate`). Deserialization rejects a `dt` that is not finite or outside
/// the steps of the fastest and slowest tick rates: `2^-32` and 1 second.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[resource(name = "ludenea::Time")]
#[serde(try_from = "TimeData", into = "TimeData")]
pub struct Time {
    pub tick: u64,
    pub dt: f32,
}

/// Serialization mirror of `Time` so loading can validate.
#[derive(Serialize, Deserialize)]
struct TimeData {
    tick: u64,
    dt: f32,
}

impl From<Time> for TimeData {
    fn from(time: Time) -> Self {
        TimeData {
            tick: time.tick,
            dt: time.dt,
        }
    }
}

impl TryFrom<TimeData> for Time {
    type Error = String;

    fn try_from(data: TimeData) -> Result<Self, String> {
        // NaN fails both comparisons.
        if !(MIN_DT <= data.dt && data.dt <= 1.0) {
            return Err(format!(
                "dt must be at least {MIN_DT:?} and at most 1 second, got {:?}",
                data.dt
            ));
        }
        Ok(Time {
            tick: data.tick,
            dt: data.dt,
        })
    }
}
