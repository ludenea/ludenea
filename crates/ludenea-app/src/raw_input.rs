//! Raw keyboard and gamepad state as the browser reports it, before a game's mapping
//! turns it into its own input type.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A button counts as pressed above this value.
const PRESSED: f32 = 0.5;
/// An axis counts as moved beyond this absolute value.
const DEAD_ZONE: f32 = 0.2;

/// What one player holds down at a moment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RawInput {
    /// `KeyboardEvent.code` values of the keys held down.
    pub keys: BTreeSet<String>,
    pub gamepads: Vec<RawGamepad>,
}

/// One gamepad in the Gamepad API standard mapping.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RawGamepad {
    pub buttons: Vec<f32>,
    pub axes: Vec<f32>,
}

impl RawInput {
    /// No key held, no button above 0.5, no axis beyond 0.2 in absolute value.
    pub fn is_idle(&self) -> bool {
        self.keys.is_empty()
            && self.gamepads.iter().all(|pad| {
                pad.buttons.iter().all(|&button| button <= PRESSED)
                    && pad.axes.iter().all(|&axis| axis.abs() <= DEAD_ZONE)
            })
    }
}
