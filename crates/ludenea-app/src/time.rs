use ludenea_ecs::Resource;
use serde::{Deserialize, Serialize};

/// The fixed simulation clock: `tick` counts completed steps, `dt` is the step length in
/// seconds (`1 / tick_rate`). Deserialization rejects a `dt` that is not finite, not above
/// 0, or above 1 second, the step of the slowest tick rate.
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
        if !(data.dt > 0.0 && data.dt <= 1.0) {
            return Err(format!(
                "dt must be above 0 and at most 1 second, got {:?}",
                data.dt
            ));
        }
        Ok(Time {
            tick: data.tick,
            dt: data.dt,
        })
    }
}
