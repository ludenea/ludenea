//! Traces: the state hash of every tick of a run, with hashes of what the run was made of.

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::{Xxh3, xxh3_64};

use crate::Simulation;
use crate::replay::Replay;

pub const TRACE_VERSION: u32 = 1;

/// A recorded run: the state hash after loading the scene and after every tick.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct Trace {
    pub version: u32,
    /// See [`scene_hash`].
    pub scene_hash: u64,
    /// See [`inputs_hash`].
    pub inputs_hash: u64,
    /// `hashes[k]` is the hash after `k` ticks, so the length is `ticks + 1`.
    pub hashes: Vec<u64>,
}

impl Trace {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("a trace always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<Trace, String> {
        let (trace, rest): (Trace, _) = postcard::take_from_bytes(bytes)
            .map_err(|error| format!("golden file is not a trace: {error}"))?;
        if !rest.is_empty() {
            return Err("golden file is not a trace: trailing bytes".into());
        }
        if trace.version != TRACE_VERSION {
            return Err(format!(
                "golden trace has version {}, this tool reads version {TRACE_VERSION}",
                trace.version
            ));
        }
        Ok(trace)
    }

    /// The trace of a replay. Without the game's codecs the assets are hashed as their
    /// file text, after the resolved scene, in identifier order.
    pub fn from_replay(replay: &Replay) -> Trace {
        let scene_hash = if replay.assets.is_empty() {
            xxh3_64(replay.scene.as_bytes())
        } else {
            let mut hasher = Xxh3::new();
            hasher.update(replay.scene.as_bytes());
            for (id, text) in &replay.assets {
                hasher.update(&id.0.to_le_bytes());
                hasher.update(text.as_bytes());
            }
            hasher.digest()
        };
        Trace {
            version: TRACE_VERSION,
            scene_hash,
            inputs_hash: inputs_hash(Some(&replay.inputs), Some(&replay.edits)),
            hashes: replay.hashes.clone(),
        }
    }
}

/// The scene hash of a trace, for `sim` just after loading the scene `scene_bytes`: xxh3 of
/// the raw bytes when no asset is loaded (so traces of scenes without assets keep their
/// milestone 1 hash); otherwise xxh3 of the bytes followed by each loaded asset's identifier
/// and canonical bytes, in identifier order. The loaded prefabs are flattened, so with the
/// scene's own overrides they determine its resolved form.
pub fn scene_hash(scene_bytes: &[u8], sim: &Simulation) -> u64 {
    let world = sim.world();
    let ids = world.asset_ids();
    if ids.is_empty() {
        return xxh3_64(scene_bytes);
    }
    let mut hasher = Xxh3::new();
    hasher.update(scene_bytes);
    for id in ids {
        let (_, bytes) = world.asset_bytes(id).expect("a loaded identifier");
        hasher.update(&id.0.to_le_bytes());
        hasher.update(&bytes);
    }
    hasher.digest()
}

/// xxh3 of the input log followed by the edit log; 0 when there is neither.
pub fn inputs_hash(inputs: Option<&[u8]>, edits: Option<&[u8]>) -> u64 {
    match (inputs, edits) {
        (None, None) => 0,
        (inputs, edits) => {
            xxh3_64(&[inputs.unwrap_or_default(), edits.unwrap_or_default()].concat())
        }
    }
}

/// How two traces' hashes compare tick by tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TraceComparison {
    /// Ticks both traces have.
    pub common: u64,
    /// The first of the common ticks whose hashes differ.
    pub first_divergent: Option<u64>,
    /// Ticks only the longer trace has.
    pub only_in_longer: u64,
}

pub fn compare_traces(a: &[u64], b: &[u64]) -> TraceComparison {
    let common = a.len().min(b.len());
    TraceComparison {
        common: common as u64,
        first_divergent: a
            .iter()
            .zip(b)
            .position(|(a, b)| a != b)
            .map(|tick| tick as u64),
        only_in_longer: a.len().abs_diff(b.len()) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_traces_have_no_divergence() {
        let hashes = [1, 2, 3];
        assert_eq!(
            compare_traces(&hashes, &hashes),
            TraceComparison {
                common: 3,
                first_divergent: None,
                only_in_longer: 0,
            }
        );
    }

    #[test]
    fn first_divergent_tick_is_reported() {
        assert_eq!(
            compare_traces(&[1, 2, 3, 4, 5], &[1, 2, 9, 4]),
            TraceComparison {
                common: 4,
                first_divergent: Some(2),
                only_in_longer: 1,
            }
        );
    }
}
