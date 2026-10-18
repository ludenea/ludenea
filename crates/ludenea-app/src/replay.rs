//! Replays: a self-contained, verifiable run definition (resolved scene, assets, input and
//! edit logs) with the hash of every tick and sparse checkpoints.

use std::collections::BTreeMap;

use ludenea_ecs::{AssetId, Snapshot};
use serde::{Deserialize, Serialize};

use crate::assets::{AssetSource, AssetTexts, parse_asset_file};
use crate::edit::{Edit, EditError, EditLog, EntityRef, TimedEdit};
use crate::{InputLog, Simulation};

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
    /// A recorded edit no longer applies to the world it was recorded on, before any
    /// divergence: the replay is inconsistent.
    #[error("the edit of tick {tick} does not apply: {reason}; record the replay again")]
    Edit { tick: u64, reason: String },
    #[error("cannot record the replay: the {what} fails: {reason}; fix it and record again")]
    Record { what: String, reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayReport {
    pub outcome: ReplayOutcome,
    /// Whether the replay was recorded with other game types.
    pub fingerprint_differs: bool,
    /// The replay's checkpoints up to where it was played, missing or invalid ones
    /// regenerated.
    pub checkpoints: Vec<(u64, Vec<u8>)>,
    /// On divergence, the encoded snapshots of the tick before it (none at tick 0) and of
    /// the divergent tick.
    pub diverged_snapshots: Option<(Option<Vec<u8>>, Vec<u8>)>,
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
    sim.load_scene_with(scene, assets)
        .map_err(record_error("scene"))?;
    sim.set_players(usize::from(inputs.players));
    record_loaded_replay(sim, inputs, edits, ticks)
}

/// `record_replay` on the scene and assets `sim` just loaded, with its players set: the
/// replay holds every loaded asset, reached by the scene or not.
pub fn record_loaded_replay(
    sim: &mut Simulation,
    inputs: &InputLog,
    edits: &EditLog,
    ticks: u64,
) -> Result<Replay, ReplayError> {
    let scene = sim.resolved_scene().ok_or_else(|| ReplayError::Record {
        what: "scene".to_string(),
        reason: "no scene is loaded".to_string(),
    })?;
    // A replay holds the inputs of the ticks it records only.
    let mut inputs = inputs.clone();
    inputs
        .ticks
        .truncate(usize::try_from(ticks).unwrap_or(usize::MAX));
    let mut replay = Replay {
        version: REPLAY_VERSION,
        engine: env!("CARGO_PKG_VERSION").to_string(),
        fingerprint: sim.schema_fingerprint(),
        scene: scene.to_string(),
        assets: sim.assets_ron().into_iter().collect(),
        inputs: inputs.encode(),
        edits: Vec::new(),
        hashes: vec![sim.hash()],
        checkpoints: Vec::new(),
    };
    let mut recorded = EditLog::new();
    for tick in 0..ticks {
        let what = format!("edit of tick {tick}");
        for timed in edits.at(tick) {
            let edit = with_runtime_target(sim, &timed.edit).map_err(record_error(&what))?;
            sim.apply_edit(&edit).map_err(record_error(&what))?;
            recorded.edits.push(TimedEdit { tick, edit });
        }
        sim.step_encoded(inputs.tick(tick))
            .map_err(record_error(&format!("input of tick {tick}")))?;
        replay.hashes.push(sim.hash());
        if (tick + 1) % REPLAY_CHECKPOINT_INTERVAL == 0 {
            replay.checkpoints.push((tick + 1, sim.snapshot().encode()));
        }
    }
    replay.edits = recorded.encode();
    Ok(replay)
}

/// Loads the replay's scene and assets into `sim` and checks that every recorded value
/// decodes, then plays it against its hashes, calling `on_tick(tick, total)` after each
/// tick. Stops at the first divergent tick, leaving `sim` there.
pub fn play_replay(
    sim: &mut Simulation,
    replay: &Replay,
    mut on_tick: impl FnMut(u64, u64),
) -> Result<ReplayReport, ReplayError> {
    let total = ticks(replay)?;
    let (inputs, edits) = load(sim, replay, total)?;

    let mut report = ReplayReport {
        outcome: ReplayOutcome::Matched,
        fingerprint_differs: replay.fingerprint != sim.schema_fingerprint(),
        checkpoints: Vec::new(),
        diverged_snapshots: None,
    };
    // The latest snapshot of this run, to re-simulate the tick before a divergence from.
    let mut held = (0, sim.snapshot());
    let mut tick = 0;
    loop {
        let (expected, got) = (replay.hashes[tick as usize], sim.hash());
        if expected != got {
            report.outcome = ReplayOutcome::Diverged {
                tick,
                expected,
                got,
            };
            let previous = match tick {
                0 => None,
                _ => Some(snapshot_before(sim, &held, &inputs, &edits)?),
            };
            report.diverged_snapshots = Some((previous, sim.snapshot().encode()));
            break;
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
            held = (tick, sim.snapshot());
        }
        if tick == total {
            break;
        }
        step(sim, &inputs, &edits, tick)?;
        tick += 1;
        on_tick(tick, total);
    }
    Ok(report)
}

/// Loads the replay's scene and assets into `sim` and checks that every recorded value
/// decodes, without playing it; returns its input and edit logs.
pub fn load_replay(
    sim: &mut Simulation,
    replay: &Replay,
) -> Result<(InputLog, EditLog), ReplayError> {
    load(sim, replay, ticks(replay)?)
}

/// The number of ticks the replay records.
fn ticks(replay: &Replay) -> Result<u64, ReplayError> {
    match replay.hashes.len() {
        0 => Err(decode(
            "hashes",
            "the replay has no hash of tick 0".to_string(),
        )),
        hashes => Ok(hashes as u64 - 1),
    }
}

/// Inserts every asset of `assets` into `sim` after a scene load, which keeps only the
/// assets the scene's handles reach (a resolved scene reaches no prefab).
pub fn keep_assets<'a>(
    sim: &mut Simulation,
    assets: impl IntoIterator<Item = (AssetId, &'a str)>,
) -> Result<(), ReplayError> {
    let parsed = parse_assets(sim, assets)?;
    insert_assets(sim, &parsed)
}

/// Each asset's identifier, type name, and RON value, checked to decode.
fn parse_assets<'a>(
    sim: &Simulation,
    assets: impl IntoIterator<Item = (AssetId, &'a str)>,
) -> Result<Vec<(AssetId, String, String)>, ReplayError> {
    let mut parsed = Vec::new();
    for (id, text) in assets {
        let what = format!("asset {id}");
        let (type_name, value) = parse_asset_file(text).map_err(decode_error(&what))?;
        let codec = sim
            .world()
            .asset_codec(&type_name)
            .ok_or_else(|| decode(&what, format!("asset type `{type_name}` is not registered")))?;
        (codec.decode_ron)(&value).map_err(decode_error(&what))?;
        parsed.push((id, type_name, value));
    }
    Ok(parsed)
}

fn insert_assets(
    sim: &mut Simulation,
    parsed: &[(AssetId, String, String)],
) -> Result<(), ReplayError> {
    for (id, type_name, value) in parsed {
        sim.world_mut()
            .insert_asset_ron(*id, type_name, value)
            .map_err(decode_error(&format!("asset {id}")))?;
    }
    Ok(())
}

/// Loads the scene and assets, sets the players, and checks that every value played up to
/// `total` decodes, in file order: assets, scene, inputs and edits by tick. Inputs and edits
/// past the recorded ticks are refused: nothing would check them.
fn load(
    sim: &mut Simulation,
    replay: &Replay,
    total: u64,
) -> Result<(InputLog, EditLog), ReplayError> {
    let texts = replay.assets.iter().map(|(id, text)| (*id, text.as_str()));
    let parsed = parse_assets(sim, texts)?;
    let assets: AssetTexts = replay.assets.iter().cloned().collect();
    sim.load_scene_with(&replay.scene, &assets)
        .map_err(decode_error("scene"))?;
    insert_assets(sim, &parsed)?;
    let inputs = InputLog::decode(&replay.inputs).map_err(decode_error("input log"))?;
    let edits = EditLog::decode(&replay.edits).map_err(decode_error("edit log"))?;
    let past = |what: String| decode(&what, format!("the replay records {total} ticks only"));
    if inputs.ticks.len() as u64 > total {
        return Err(past(format!("input of tick {total}")));
    }
    if let Some(edit) = edits.edits.iter().find(|edit| edit.tick >= total) {
        return Err(past(format!("edit of tick {}", edit.tick)));
    }
    sim.set_players(usize::from(inputs.players));

    let mut asset_types: BTreeMap<AssetId, String> = parsed
        .into_iter()
        .map(|(id, type_name, _)| (id, type_name))
        .collect();
    for tick in 0..total {
        for timed in edits.at(tick) {
            sim.check_edit_values(&timed.edit, &mut asset_types)
                .map_err(decode_error(&format!("edit of tick {tick}")))?;
        }
        if let Some(bytes) = inputs.tick(tick) {
            sim.check_encoded_inputs(bytes)
                .map_err(decode_error(&format!("input of tick {tick}")))?;
        }
    }
    Ok((inputs, edits))
}

/// Applies the edits of `tick`, then steps with its inputs.
fn step(
    sim: &mut Simulation,
    inputs: &InputLog,
    edits: &EditLog,
    tick: u64,
) -> Result<(), ReplayError> {
    for timed in edits.at(tick) {
        sim.apply_edit(&timed.edit).map_err(|error| match error {
            EditError::UnknownComponent(_)
            | EditError::UnknownResource(_)
            | EditError::InvalidValue { .. } => {
                decode(&format!("edit of tick {tick}"), error.to_string())
            }
            EditError::NoSuchEntity(_)
            | EditError::NoSuchAddress(_)
            | EditError::MissingAsset(_)
            | EditError::ReadOnly(_) => ReplayError::Edit {
                tick,
                reason: error.to_string(),
            },
        })?;
    }
    sim.step_encoded(inputs.tick(tick))
        .map_err(decode_error(&format!("input of tick {tick}")))
}

/// The encoded snapshot of the tick before the current one, re-simulated from `held`;
/// leaves `sim` as it was.
fn snapshot_before(
    sim: &mut Simulation,
    held: &(u64, Snapshot),
    inputs: &InputLog,
    edits: &EditLog,
) -> Result<Vec<u8>, ReplayError> {
    let current = sim.snapshot();
    let before = sim.tick() - 1;
    sim.restore(&held.1)
        .expect("a snapshot restores into its own world");
    for tick in held.0..before {
        step(sim, inputs, edits, tick)?;
    }
    let snapshot = sim.snapshot().encode();
    sim.restore(&current)
        .expect("a snapshot restores into its own world");
    Ok(snapshot)
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
fn with_runtime_target(sim: &Simulation, edit: &Edit) -> Result<Edit, EditError> {
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
        Edit::Spawn { .. }
        | Edit::SetResource { .. }
        | Edit::SetAsset { .. }
        | Edit::LoadAsset { .. } => edit.clone(),
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

fn record_error<E: ToString>(what: &str) -> impl FnOnce(E) -> ReplayError + '_ {
    move |error| ReplayError::Record {
        what: what.to_string(),
        reason: error.to_string(),
    }
}
