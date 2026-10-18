//! Replays: a self-contained, verifiable run definition (resolved scene, assets, input and
//! edit logs) with the hash of every tick and sparse checkpoints.

use ludenea_ecs::{AssetId, Snapshot};
use serde::{Deserialize, Serialize};

use crate::assets::{AssetSource, AssetTexts, parse_asset_file};
use crate::edit::{Edit, EditLog, EntityRef, TimedEdit};
use crate::{InputLog, Simulation, resolve};

pub const REPLAY_VERSION: u32 = 1;

/// Ticks between two checkpoints of a replay.
pub const REPLAY_CHECKPOINT_INTERVAL: u64 = 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Replay {
    pub version: u32,
    /// The `ludenea-app` version that recorded the replay.
    pub engine: String,
    /// `Simulation::schema_fingerprint` of the recording game.
    pub fingerprint: u64,
    /// The scene with every prefab instance resolved (`Resolved::to_scene_text`).
    pub scene: String,
    /// The assets loaded at tick 0, in file form.
    pub assets: Vec<(AssetId, String)>,
    /// An encoded `InputLog`.
    pub inputs: Vec<u8>,
    /// An encoded `EditLog` whose targets are all `EntityRef::Runtime`.
    pub edits: Vec<u8>,
    /// The hash of every tick, from tick 0.
    pub hashes: Vec<u64>,
    /// Encoded snapshots every `REPLAY_CHECKPOINT_INTERVAL` ticks; optional.
    pub checkpoints: Vec<(u64, Vec<u8>)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error(
        "replay version {found} is not supported (expected {expected}); record it again with this build"
    )]
    Version { found: u32, expected: u32 },
    #[error(
        "cannot decode the {what} of the replay: {reason}; was it recorded with other game types?"
    )]
    Decode { what: String, reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayReport {
    pub outcome: ReplayOutcome,
    /// Whether the replay was recorded with other game types.
    pub fingerprint_differs: bool,
    /// The replay's checkpoints up to where it was played, missing or invalid ones
    /// regenerated.
    pub checkpoints: Vec<(u64, Vec<u8>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReplayOutcome {
    Matched,
    /// The first tick whose hash differs from the recorded one.
    Diverged {
        tick: u64,
        expected: u64,
        got: u64,
    },
}

impl Replay {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("a replay always encodes")
    }

    /// Reads the version first, so a replay of another version fails as such.
    pub fn decode(bytes: &[u8]) -> Result<Replay, ReplayError> {
        let (version, _) =
            postcard::take_from_bytes::<u32>(bytes).map_err(decode_error("replay"))?;
        if version != REPLAY_VERSION {
            return Err(ReplayError::Version {
                found: version,
                expected: REPLAY_VERSION,
            });
        }
        let (replay, rest) = postcard::take_from_bytes(bytes).map_err(decode_error("replay"))?;
        if !rest.is_empty() {
            return Err(decode(
                "replay",
                format!("{} unexpected trailing bytes", rest.len()),
            ));
        }
        Ok(replay)
    }
}

/// Loads `scene` with `assets`, then runs `ticks` ticks of `inputs` and `edits`, recording
/// the replay of that run.
pub fn record_replay(
    sim: &mut Simulation,
    scene: &str,
    assets: &dyn AssetSource,
    inputs: &InputLog,
    edits: &EditLog,
    ticks: u64,
) -> Result<Replay, ReplayError> {
    let resolved = resolve(scene, assets).map_err(decode_error("scene"))?;
    sim.load_scene_with(scene, assets)
        .map_err(decode_error("scene"))?;
    sim.set_players(usize::from(inputs.players));
    let mut replay = Replay {
        version: REPLAY_VERSION,
        engine: env!("CARGO_PKG_VERSION").to_string(),
        fingerprint: sim.schema_fingerprint(),
        scene: resolved.to_scene_text(),
        assets: sim.assets_ron().into_iter().collect(),
        inputs: inputs.encode(),
        edits: Vec::new(),
        hashes: vec![sim.hash()],
        checkpoints: Vec::new(),
    };
    let mut recorded = EditLog::new();
    for tick in 0..ticks {
        for timed in edits.at(tick) {
            let edit = with_runtime_target(sim, &timed.edit).map_err(edit_error(tick))?;
            sim.apply_edit(&edit).map_err(edit_error(tick))?;
            recorded.edits.push(TimedEdit { tick, edit });
        }
        sim.step_encoded(inputs.tick(tick))
            .map_err(decode_error(&format!("input of tick {tick}")))?;
        replay.hashes.push(sim.hash());
        if (tick + 1) % REPLAY_CHECKPOINT_INTERVAL == 0 {
            replay.checkpoints.push((tick + 1, sim.snapshot().encode()));
        }
    }
    replay.edits = recorded.encode();
    Ok(replay)
}

/// Loads the replay's scene and assets into `sim`, then plays all of it against its hashes,
/// so that a value failing to decode is found even past the first divergence. Calls
/// `on_tick(tick, total)` after each tick.
pub fn play_replay(
    sim: &mut Simulation,
    replay: &Replay,
    mut on_tick: impl FnMut(u64, u64),
) -> Result<ReplayReport, ReplayError> {
    let assets: AssetTexts = replay.assets.iter().cloned().collect();
    sim.load_scene_with(&replay.scene, &assets)
        .map_err(decode_error("scene"))?;
    // The scene load keeps only the assets its handles reach; the replay holds them all.
    for (id, text) in &replay.assets {
        let what = format!("asset {id}");
        let (type_name, value) = parse_asset_file(text).map_err(decode_error(&what))?;
        sim.world_mut()
            .insert_asset_ron(*id, &type_name, &value)
            .map_err(decode_error(&what))?;
    }
    let inputs = InputLog::decode(&replay.inputs).map_err(decode_error("input log"))?;
    let edits = EditLog::decode(&replay.edits).map_err(decode_error("edit log"))?;
    sim.set_players(usize::from(inputs.players));

    let mut report = ReplayReport {
        outcome: ReplayOutcome::Matched,
        fingerprint_differs: replay.fingerprint != sim.schema_fingerprint(),
        checkpoints: Vec::new(),
    };
    if replay.hashes.is_empty() {
        return Err(decode(
            "hashes",
            "the replay has no hash of tick 0".to_string(),
        ));
    }
    let total = replay.hashes.len() as u64 - 1;
    let mut tick = 0;
    loop {
        let (expected, got) = (replay.hashes[tick as usize], sim.hash());
        if expected != got && report.outcome == ReplayOutcome::Matched {
            report.outcome = ReplayOutcome::Diverged {
                tick,
                expected,
                got,
            };
        }
        if tick > 0 && tick % REPLAY_CHECKPOINT_INTERVAL == 0 {
            let recorded = replay
                .checkpoints
                .iter()
                .find(|(at, _)| *at == tick)
                .map(|(_, bytes)| bytes.as_slice());
            report
                .checkpoints
                .push((tick, checkpoint(sim, recorded, expected)));
        }
        if tick == total {
            break;
        }
        for timed in edits.at(tick) {
            sim.apply_edit(&timed.edit).map_err(edit_error(tick))?;
        }
        sim.step_encoded(inputs.tick(tick))
            .map_err(decode_error(&format!("input of tick {tick}")))?;
        tick += 1;
        on_tick(tick, total);
    }
    Ok(report)
}

/// The recorded checkpoint of the current tick when it restores to `expected`, leaving the
/// world restored from it; otherwise a snapshot of the world.
fn checkpoint(sim: &mut Simulation, recorded: Option<&[u8]>, expected: u64) -> Vec<u8> {
    let live = sim.snapshot();
    if let Some(bytes) = recorded
        && let Ok(snapshot) = Snapshot::decode(bytes)
        && snapshot.fingerprint == live.fingerprint
        && sim.restore(&snapshot).is_ok()
    {
        if sim.hash() == expected {
            return bytes.to_vec();
        }
        sim.restore(&live)
            .expect("a snapshot restores into its own world");
    }
    live.encode()
}

/// `edit` with its target named by the entity it resolves to now.
fn with_runtime_target(sim: &Simulation, edit: &Edit) -> Result<Edit, crate::EditError> {
    let runtime = |target: &EntityRef| sim.resolve_ref(target).map(EntityRef::Runtime);
    Ok(match edit {
        Edit::Despawn { entity } => Edit::Despawn {
            entity: runtime(entity)?,
        },
        Edit::Insert {
            entity,
            component,
            value,
        } => Edit::Insert {
            entity: runtime(entity)?,
            component: component.clone(),
            value: value.clone(),
        },
        Edit::Remove { entity, component } => Edit::Remove {
            entity: runtime(entity)?,
            component: component.clone(),
        },
        other => other.clone(),
    })
}

fn decode(what: &str, reason: String) -> ReplayError {
    ReplayError::Decode {
        what: what.to_string(),
        reason,
    }
}

fn decode_error<E: ToString>(what: &str) -> impl FnOnce(E) -> ReplayError + '_ {
    move |error| decode(what, error.to_string())
}

fn edit_error<E: ToString>(tick: u64) -> impl FnOnce(E) -> ReplayError {
    move |error| decode(&format!("edit of tick {tick}"), error.to_string())
}
