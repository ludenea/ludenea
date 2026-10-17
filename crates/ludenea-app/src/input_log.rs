use serde::{Deserialize, Serialize};

use crate::SimError;
use crate::input::{self, Input};

pub const INPUT_LOG_VERSION: u32 = 1;

/// The inputs of a run, one entry per tick, for replay with `Simulation::step_encoded`.
/// A tick past the end of the log has no entry, which replays as default inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputLog {
    pub version: u32,
    pub players: u16,
    /// Per tick, the postcard bytes of a `Vec<I>` with one input per player.
    pub ticks: Vec<Vec<u8>>,
}

impl InputLog {
    pub fn new(players: u16) -> Self {
        Self {
            version: INPUT_LOG_VERSION,
            players,
            ticks: Vec::new(),
        }
    }

    /// Appends the inputs of the next tick.
    pub fn record<I: Input>(&mut self, inputs: &[I]) {
        self.ticks
            .push(postcard::to_allocvec(inputs).expect("inputs always encode"));
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("an input log always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<InputLog, SimError> {
        let log: InputLog = input::decode_exact(bytes)?;
        if log.version != INPUT_LOG_VERSION {
            return Err(SimError::InputLogVersion {
                found: log.version,
                expected: INPUT_LOG_VERSION,
            });
        }
        if log.players == 0 {
            return Err(SimError::NoPlayers);
        }
        Ok(log)
    }

    /// The recorded inputs of `tick`, or `None` past the end of the log.
    pub fn tick(&self, tick: u64) -> Option<&[u8]> {
        let index = usize::try_from(tick).ok()?;
        self.ticks.get(index).map(Vec::as_slice)
    }
}
