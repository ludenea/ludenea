use serde::{Deserialize, Serialize};

pub const TRACE_VERSION: u32 = 1;

/// A recorded run: the state hash after loading the scene and after every tick.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct Trace {
    pub version: u32,
    /// xxh3 of the raw scene file.
    pub scene_hash: u64,
    /// xxh3 of the raw input log file, 0 without one.
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
}
