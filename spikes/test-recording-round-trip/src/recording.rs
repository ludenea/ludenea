//! A recording of one world, its two forms, and its replay. Throwaway, never
//! product code.
//!
//! A recording holds the header of its starting state (the workload version,
//! the snapshot format, the world's creation values, the starting tick and the
//! hash of the starting state), the starting state in one of two forms, the
//! input record of every tick after it, and the hash of every tick:
//!
//! - the self-contained form carries the snapshot of the starting state;
//! - the reference form carries the recipe instead, the creation values of the
//!   header and the input records of the fixture's preparation, and no state.
//!
//! A replay creates a world from the file, checks the starting state's hash
//! before any record of the run is applied, steps every record in one
//! multi-tick step and compares the recorded hashes with the replayed ones.

use reference_workload::VERSION;

use crate::container::{self, Container, ContainerError, Kind, Record, RecordType, Section, Value, record_type};
use crate::pool::Executor;
use crate::world::{Kick, SNAPSHOT_FORMAT, Size, Snapshot, World, WorldConfig};

const TYPE_KICK: u32 = 0;
const TYPE_HEADER: u32 = 1;
const TYPE_INPUT: u32 = 2;
const TYPE_HASH: u32 = 3;
const TYPE_SNAPSHOT: u32 = 4;

pub const TAG_HEADER: [u8; 4] = *b"head";
pub const TAG_SNAPSHOT: [u8; 4] = *b"snap";
pub const TAG_PREPARATION: [u8; 4] = *b"prep";
pub const TAG_INPUTS: [u8; 4] = *b"inpt";
pub const TAG_HASHES: [u8; 4] = *b"hash";

const FORM_SELF_CONTAINED: u8 = 0;
const FORM_REFERENCE: u8 = 1;

/// The description every recording of this spike carries.
pub fn description() -> Vec<RecordType> {
    vec![
        record_type("kick", &[("index", Kind::U32), ("generation", Kind::U32), ("delta_vx", Kind::F32), ("delta_vy", Kind::F32)]),
        record_type("header", &[
            ("workload_version", Kind::U32),
            ("snapshot_format", Kind::U32),
            ("seed", Kind::U64),
            ("entity_count", Kind::U32),
            ("wander", Kind::U8),
            ("form", Kind::U8),
            ("start_tick", Kind::U32),
            ("start_hash", Kind::U64),
            ("end_tick", Kind::U32),
            ("test", Kind::Text),
            ("writer_cell", Kind::Text),
            ("writer_profile", Kind::Text),
        ]),
        record_type("input_record", &[("tick", Kind::U32), ("source", Kind::U32), ("kicks", Kind::Sequence(TYPE_KICK))]),
        record_type("tick_hash", &[("tick", Kind::U32), ("hash", Kind::U64)]),
        record_type("snapshot", &[("bytes", Kind::Bytes)]),
    ]
}

/// The input record of one tick: its tick, the identity of its source and
/// the kicks in the order the source gave them.
#[derive(Clone, Debug, PartialEq)]
pub struct InputRecord {
    pub tick: u32,
    pub source: u32,
    pub kicks: Vec<Kick>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    /// The self-contained form: the snapshot of the starting state.
    Snapshot(Vec<u8>),
    /// The reference form: the input records of the fixture's preparation,
    /// applied to the world the header's creation values create.
    Recipe(Vec<InputRecord>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub workload_version: u32,
    pub snapshot_format: u32,
    pub seed: u64,
    pub entity_count: u32,
    pub wander: bool,
    pub start_tick: u32,
    pub start_hash: u64,
    pub end_tick: u32,
    pub test: String,
    pub writer_cell: String,
    pub writer_profile: String,
    pub start: Start,
    pub inputs: Vec<InputRecord>,
    pub hashes: Vec<(u32, u64)>,
}

fn input_values(record: &InputRecord) -> Record {
    let kicks = record
        .kicks
        .iter()
        .map(|kick| vec![Value::U32(kick.index), Value::U32(kick.generation), Value::F32(kick.delta_vx), Value::F32(kick.delta_vy)])
        .collect();
    vec![Value::U32(record.tick), Value::U32(record.source), Value::Sequence(kicks)]
}

impl Recording {
    pub fn form(&self) -> &'static str {
        match self.start {
            Start::Snapshot(_) => "self-contained",
            Start::Recipe(_) => "reference",
        }
    }

    /// The single file of the recording.
    pub fn encode(&self) -> Vec<u8> {
        let form = match self.start {
            Start::Snapshot(_) => FORM_SELF_CONTAINED,
            Start::Recipe(_) => FORM_REFERENCE,
        };
        let header = vec![
            Value::U32(self.workload_version),
            Value::U32(self.snapshot_format),
            Value::U64(self.seed),
            Value::U32(self.entity_count),
            Value::U8(u8::from(self.wander)),
            Value::U8(form),
            Value::U32(self.start_tick),
            Value::U64(self.start_hash),
            Value::U32(self.end_tick),
            Value::Text(self.test.clone()),
            Value::Text(self.writer_cell.clone()),
            Value::Text(self.writer_profile.clone()),
        ];
        let start = match &self.start {
            Start::Snapshot(bytes) => Section { tag: TAG_SNAPSHOT, type_index: TYPE_SNAPSHOT, records: vec![vec![Value::Bytes(bytes.clone())]] },
            Start::Recipe(records) => Section { tag: TAG_PREPARATION, type_index: TYPE_INPUT, records: records.iter().map(input_values).collect() },
        };
        let sections = [
            Section { tag: TAG_HEADER, type_index: TYPE_HEADER, records: vec![header] },
            start,
            Section { tag: TAG_INPUTS, type_index: TYPE_INPUT, records: self.inputs.iter().map(input_values).collect() },
            Section {
                tag: TAG_HASHES,
                type_index: TYPE_HASH,
                records: self.hashes.iter().map(|&(tick, hash)| vec![Value::U32(tick), Value::U64(hash)]).collect(),
            },
        ];
        container::write(&description(), &sections)
    }

    /// A recording from its file. The values are found by the names the
    /// file's own description gives its fields, never by their positions.
    pub fn decode(bytes: &[u8]) -> Result<Recording, ContainerError> {
        let container = container::read(bytes)?;
        let fields = Fields { container: &container };
        let head = fields.section(TAG_HEADER)?;
        let [header] = head.records.as_slice() else {
            return Err(ContainerError("the header section holds another count of records than one".to_string()));
        };
        let h = |name: &str| fields.value(head.type_index, header, name);
        let start = match h("form")?.u8()? {
            FORM_SELF_CONTAINED => {
                let section = fields.section(TAG_SNAPSHOT)?;
                let [snapshot] = section.records.as_slice() else {
                    return Err(ContainerError("the snapshot section holds another count of records than one".to_string()));
                };
                Start::Snapshot(fields.value(section.type_index, snapshot, "bytes")?.bytes()?.to_vec())
            }
            FORM_REFERENCE => Start::Recipe(fields.inputs(TAG_PREPARATION)?),
            other => return Err(ContainerError(format!("the header names the unknown form {other}"))),
        };
        let hash_section = fields.section(TAG_HASHES)?;
        let mut hashes = Vec::with_capacity(hash_section.records.len());
        for record in &hash_section.records {
            hashes.push((
                fields.value(hash_section.type_index, record, "tick")?.u32()?,
                fields.value(hash_section.type_index, record, "hash")?.u64()?,
            ));
        }
        Ok(Recording {
            workload_version: h("workload_version")?.u32()?,
            snapshot_format: h("snapshot_format")?.u32()?,
            seed: h("seed")?.u64()?,
            entity_count: h("entity_count")?.u32()?,
            wander: match h("wander")?.u8()? {
                0 => false,
                1 => true,
                other => return Err(ContainerError(format!("the header's wander flag is {other}"))),
            },
            start_tick: h("start_tick")?.u32()?,
            start_hash: h("start_hash")?.u64()?,
            end_tick: h("end_tick")?.u32()?,
            test: h("test")?.text()?.to_string(),
            writer_cell: h("writer_cell")?.text()?.to_string(),
            writer_profile: h("writer_profile")?.text()?.to_string(),
            start,
            inputs: fields.inputs(TAG_INPUTS)?,
            hashes,
        })
    }
}

/// The values of a container reached by section tag and field name.
struct Fields<'a> {
    container: &'a Container,
}

impl<'a> Fields<'a> {
    fn section(&self, tag: [u8; 4]) -> Result<&'a Section, ContainerError> {
        self.container.section(tag).ok_or_else(|| ContainerError(format!("the file holds no section {}", container::tag_text(tag))))
    }

    fn value(&self, type_index: u32, record: &'a Record, name: &str) -> Result<Typed<'a>, ContainerError> {
        let position =
            self.container.field(type_index, name).ok_or_else(|| ContainerError(format!("the description holds no field {name} where one is read")))?;
        Ok(Typed { value: &record[position], name: name.to_string() })
    }

    fn inputs(&self, tag: [u8; 4]) -> Result<Vec<InputRecord>, ContainerError> {
        let section = self.section(tag)?;
        let mut records = Vec::with_capacity(section.records.len());
        for record in &section.records {
            let kicks_value = self.value(section.type_index, record, "kicks")?;
            let Value::Sequence(kick_records) = kicks_value.value else {
                return Err(kicks_value.mismatch("a sequence"));
            };
            let kick_type = match self.container.types[section.type_index as usize].fields.iter().find(|field| field.name == "kicks").map(|field| &field.kind)
            {
                Some(Kind::Sequence(index)) => *index,
                _ => return Err(kicks_value.mismatch("a sequence")),
            };
            let mut kicks = Vec::with_capacity(kick_records.len());
            for kick in kick_records {
                kicks.push(Kick {
                    index: self.value(kick_type, kick, "index")?.u32()?,
                    generation: self.value(kick_type, kick, "generation")?.u32()?,
                    delta_vx: self.value(kick_type, kick, "delta_vx")?.f32()?,
                    delta_vy: self.value(kick_type, kick, "delta_vy")?.f32()?,
                });
            }
            records.push(InputRecord {
                tick: self.value(section.type_index, record, "tick")?.u32()?,
                source: self.value(section.type_index, record, "source")?.u32()?,
                kicks,
            });
        }
        Ok(records)
    }
}

struct Typed<'a> {
    value: &'a Value,
    name: String,
}

impl<'a> Typed<'a> {
    fn mismatch(&self, wanted: &str) -> ContainerError {
        ContainerError(format!("field {} is read as {wanted} and the description gives it another kind", self.name))
    }

    fn u8(&self) -> Result<u8, ContainerError> {
        if let Value::U8(v) = self.value { Ok(*v) } else { Err(self.mismatch("an 8-bit integer")) }
    }

    fn u32(&self) -> Result<u32, ContainerError> {
        if let Value::U32(v) = self.value { Ok(*v) } else { Err(self.mismatch("a 32-bit integer")) }
    }

    fn u64(&self) -> Result<u64, ContainerError> {
        if let Value::U64(v) = self.value { Ok(*v) } else { Err(self.mismatch("a 64-bit integer")) }
    }

    fn f32(&self) -> Result<f32, ContainerError> {
        if let Value::F32(v) = self.value { Ok(*v) } else { Err(self.mismatch("a float")) }
    }

    fn text(&self) -> Result<&'a str, ContainerError> {
        if let Value::Text(v) = self.value { Ok(v) } else { Err(self.mismatch("a text")) }
    }

    fn bytes(&self) -> Result<&'a [u8], ContainerError> {
        if let Value::Bytes(v) = self.value { Ok(v) } else { Err(self.mismatch("bytes")) }
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every recorded hash up to the last tick equals the replayed one.
    Verified { last_tick: u32, compared: u32 },
    /// The first tick whose recorded hash differs from the replayed one.
    Diverged { tick: u32, recorded: u64, replayed: u64 },
    /// The recording holds no hash after its starting tick: the records were
    /// applied and nothing was compared.
    NothingCompared { last_tick: u32 },
    /// The file, its header or its starting state was refused.
    Refused { reason: String },
}

impl Verdict {
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Verified { .. } => "verified",
            Verdict::Diverged { .. } => "diverged",
            Verdict::NothingCompared { .. } => "replayed, nothing compared",
            Verdict::Refused { .. } => "refused",
        }
    }
}

/// What a replay did: its verdict and how many input records of the run it
/// applied. Every refusal of the file, of its header or of its starting state
/// returns before the step that applies the run's records, so its count is
/// zero by the order of the code; a world that fails during that step is
/// refused with the records applied before the failing tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    pub verdict: Verdict,
    pub records_applied: u32,
}

fn refuse(reason: impl Into<String>) -> Replay {
    Replay { verdict: Verdict::Refused { reason: reason.into() }, records_applied: 0 }
}

fn consecutive(records: &[InputRecord], first: u32) -> Result<(), String> {
    for (offset, record) in records.iter().enumerate() {
        let expected = first + offset as u32;
        if record.tick != expected {
            return Err(format!("an input record is of tick {} where tick {expected} follows the one before", record.tick));
        }
    }
    Ok(())
}

/// Replays a file on the executor: decodes it, then [`replay_recording`].
pub fn replay(bytes: &[u8], executor: &dyn Executor) -> Replay {
    match Recording::decode(bytes) {
        Ok(recording) => replay_recording(&recording, executor),
        Err(error) => refuse(error.0),
    }
}

/// Replays a recording to its last tick. In order: the header against this
/// build, the starting state (restored from the snapshot, or rebuilt from the
/// recipe) and its hash against the recorded one, each record's tick against
/// the one before, then every record in one multi-tick step, and the recorded
/// hashes against the replayed ones.
pub fn replay_recording(recording: &Recording, executor: &dyn Executor) -> Replay {
    if recording.workload_version != VERSION {
        return refuse(format!("workload version {}, this build runs {VERSION}", recording.workload_version));
    }
    if recording.snapshot_format != SNAPSHOT_FORMAT {
        return refuse(format!("snapshot format {}, this build reads {SNAPSHOT_FORMAT}", recording.snapshot_format));
    }
    let Some(size) = Size::ALL.iter().copied().find(|size| size.entity_count() == recording.entity_count as usize) else {
        return refuse(format!("{} entities is not a workload size", recording.entity_count));
    };
    if recording.end_tick < recording.start_tick {
        return refuse("the recording ends before it starts");
    }
    // The tick after the last one is computed below and by the world.
    if recording.end_tick == u32::MAX {
        return refuse("the recording's last tick is the largest a tick counts to");
    }
    if recording.inputs.len() as u64 != u64::from(recording.end_tick - recording.start_tick) {
        return refuse(format!("{} input records for the ticks {} to {}", recording.inputs.len(), recording.start_tick + 1, recording.end_tick));
    }
    if let Err(reason) = consecutive(&recording.inputs, recording.start_tick + 1) {
        return refuse(reason);
    }
    let mut previous = None;
    for &(tick, _) in &recording.hashes {
        if tick < recording.start_tick || tick > recording.end_tick || previous.is_some_and(|p| tick <= p) {
            return refuse(format!("the hash of tick {tick} is out of the recording's ticks or out of order"));
        }
        previous = Some(tick);
    }
    let config = WorldConfig { seed: recording.seed, size, wander: recording.wander, hash_cadence: 1 };
    let mut world = match &recording.start {
        Start::Snapshot(bytes) => {
            let snapshot = Snapshot { bytes: bytes.clone() };
            match World::restore(&snapshot, config) {
                Ok(world) => world,
                Err(error) => return refuse(error.0),
            }
        }
        Start::Recipe(preparation) => {
            if preparation.len() as u32 != recording.start_tick {
                return refuse(format!("{} preparation records for a starting tick of {}", preparation.len(), recording.start_tick));
            }
            if let Err(reason) = consecutive(preparation, 1) {
                return refuse(reason);
            }
            let mut world = World::new(config);
            if let Err(failed) = world.step(preparation.iter().map(|record| record.kicks.clone()).collect(), executor) {
                return refuse(failed.to_string());
            }
            world
        }
    };
    if world.tick() != recording.start_tick {
        return refuse(format!("the starting state is of tick {} and the header says {}", world.tick(), recording.start_tick));
    }
    let start_hash = world.tick_hash(recording.start_tick).expect("the starting tick is hashed");
    if start_hash != recording.start_hash {
        return refuse(format!(
            "the starting state's hash is {start_hash:016x} and the recording's is {:016x}: no record of the run is applied",
            recording.start_hash
        ));
    }
    if let Err(failed) = world.step(recording.inputs.iter().map(|record| record.kicks.clone()).collect(), executor) {
        // The one refusal after records of the run were applied: the world
        // stands at the last tick before the failing one.
        let records_applied = world.tick().saturating_sub(recording.start_tick);
        return Replay { verdict: Verdict::Refused { reason: failed.to_string() }, records_applied };
    }
    let records_applied = recording.inputs.len() as u32;
    let mut compared = 0;
    for &(tick, recorded) in &recording.hashes {
        let replayed = world.tick_hash(tick).expect("every tick of a replay is hashed");
        if recorded != replayed {
            return Replay { verdict: Verdict::Diverged { tick, recorded, replayed }, records_applied };
        }
        if tick > recording.start_tick {
            compared += 1;
        }
    }
    let verdict = if compared == 0 {
        Verdict::NothingCompared { last_tick: recording.end_tick }
    } else {
        Verdict::Verified { last_tick: recording.end_tick, compared }
    };
    Replay { verdict, records_applied }
}
