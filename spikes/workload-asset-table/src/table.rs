//! The asset identity core of the asset table spike. Throwaway, never product
//! code.
//!
//! An asset is named by a 128-bit identifier drawn outside the domain. A
//! component field holds an identifier and asks for nothing: what a world
//! loads, it loads because a want says so. The asset table is state of the
//! world: per asset that has a want, its wants in the order of their owners
//! and its availability (unavailable, available at a content hash, failed
//! with a reason code). Four rules make it a function of the inputs:
//!
//! - availability rises at the input fixed point alone, by an arrival;
//! - wanting an asset wants the dependencies its canonical part lists: as a
//!   batch of arrivals is applied, the core first adds a want of every
//!   dependency the batch's parts list, to any depth, owned by the parent and
//!   under its policy, then applies the arrivals in identifier order;
//! - an asset no want names falls to unavailable at the input fixed point of
//!   the tick after its last want ended, derived from the table and written
//!   nowhere in the input record;
//! - an arrival for an asset no want names is recorded and changes nothing.
//!
//! The canonical parts live outside the world, in a resident store shared by
//! the worlds of the process and keyed by content hash; a world that holds an
//! asset available holds a reference to its entry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use reference_workload::fnv1a;

/// A 128-bit asset identifier, drawn at random outside the domain.
pub type AssetId = u128;

pub const REASON_NOT_FOUND: u32 = 1;
pub const REASON_INVALID_PART: u32 = 2;
pub const REASON_HASH_MISMATCH: u32 = 3;

pub fn reason_text(code: u32) -> &'static str {
    match code {
        REASON_NOT_FOUND => "no artifact for the identifier",
        REASON_INVALID_PART => "the canonical part does not decode",
        REASON_HASH_MISMATCH => "the artifact's bytes do not hash to the announced content hash",
        _ => "unknown reason",
    }
}

// ---------------------------------------------------------------------------
// The canonical part and the resident store
// ---------------------------------------------------------------------------

/// The repulsion parameters a part carries: what the repulsion pass reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepulsionTable {
    pub strength: f32,
    pub radius: f32,
}

impl RepulsionTable {
    /// The declared default a read of an unavailable asset gives: no repulsion.
    pub const DEFAULT: RepulsionTable = RepulsionTable { strength: 0.0, radius: 8.0 };
    /// The specification's constants, with which a world gives the reference hashes.
    pub const SPECIFICATION: RepulsionTable = RepulsionTable { strength: 64.0, radius: 8.0 };
}

const PART_FORMAT: u32 = 1;

/// A decoded canonical part: its parameters and the assets it depends on. Its
/// opaque payload is validated and never copied out of the canonical bytes.
pub struct Part {
    pub table: RepulsionTable,
    pub dependencies: Vec<AssetId>,
}

/// The canonical bytes of a part: a format word, the strength and the radius,
/// the dependency count and the identifiers, the payload length and the
/// payload, little-endian. The payload is generated from its length and a
/// salt, so that two parts with the same parameters can be distinct content.
/// The buffer is allocated once at its exact size.
pub fn part_bytes(table: RepulsionTable, dependencies: &[AssetId], payload_len: usize, salt: u32) -> Vec<u8> {
    let len = part_len(dependencies.len(), payload_len);
    let mut bytes = Vec::with_capacity(len);
    bytes.extend_from_slice(&PART_FORMAT.to_le_bytes());
    bytes.extend_from_slice(&table.strength.to_le_bytes());
    bytes.extend_from_slice(&table.radius.to_le_bytes());
    bytes.extend_from_slice(&(dependencies.len() as u32).to_le_bytes());
    for dependency in dependencies {
        bytes.extend_from_slice(&dependency.to_le_bytes());
    }
    bytes.extend_from_slice(&(payload_len as u32).to_le_bytes());
    bytes.extend((0..payload_len).map(|k| (k as u32).wrapping_mul(31).wrapping_add(salt).to_le_bytes()[k % 4]));
    debug_assert_eq!(bytes.len(), len);
    bytes
}

pub fn part_len(dependency_count: usize, payload_len: usize) -> usize {
    20 + dependency_count * 16 + payload_len
}

impl Part {
    /// Validates and decodes canonical bytes; any defect is the invalid part reason.
    pub fn decode(bytes: &[u8]) -> Result<Part, u32> {
        let mut reader = Reader::new(bytes);
        let mut decode = || -> Result<Part, String> {
            if reader.u32()? != PART_FORMAT {
                return Err("unknown part format".to_string());
            }
            let table = RepulsionTable { strength: reader.f32()?, radius: reader.f32()? };
            let count = reader.u32()? as usize;
            let mut dependencies = Vec::new();
            for _ in 0..count {
                dependencies.push(reader.id()?);
            }
            let payload_len = reader.u32()? as usize;
            reader.take(payload_len)?;
            reader.end()?;
            Ok(Part { table, dependencies })
        };
        decode().map_err(|_| REASON_INVALID_PART)
    }
}

/// One entry of the resident store: canonical bytes, immutable once inserted,
/// with their decoded part.
pub struct Resident {
    pub bytes: Vec<u8>,
    pub part: Part,
}

/// The resident store of a runtime instance: the canonical parts by content
/// hash, shared read-only by the worlds of the process.
#[derive(Default)]
pub struct ResidentStore {
    entries: Mutex<BTreeMap<u64, Arc<Resident>>>,
}

impl ResidentStore {
    pub fn get(&self, hash: u64) -> Option<Arc<Resident>> {
        self.entries.lock().expect("the store is never poisoned").get(&hash).cloned()
    }

    /// Verifies the bytes against the announced hash, validates the part and
    /// inserts it; the bytes are moved, never copied. An entry already there
    /// is returned as is.
    pub fn insert(&self, hash: u64, bytes: Vec<u8>) -> Result<Arc<Resident>, u32> {
        if fnv1a(&bytes) != hash {
            return Err(REASON_HASH_MISMATCH);
        }
        let part = Part::decode(&bytes)?;
        let mut entries = self.entries.lock().expect("the store is never poisoned");
        Ok(entries.entry(hash).or_insert_with(|| Arc::new(Resident { bytes, part })).clone())
    }

    pub fn hashes(&self) -> Vec<u64> {
        self.entries.lock().expect("the store is never poisoned").keys().copied().collect()
    }
}

// ---------------------------------------------------------------------------
// Wants, availability, arrivals, commands, the input record
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Policy {
    Optional,
    Required,
}

/// Who a want belongs to: the world's own entry, an entity, an instance
/// record, or (a want the core added) the asset whose part lists this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    World,
    Entity { slot: u32, generation: u32 },
    Instance(u32),
    Asset(AssetId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Unavailable,
    Available(u64),
    Failed(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hash(u64),
    Failure(u32),
}

/// What the loading capability posts per asset of a batch: the identifier and
/// the content hash of its canonical part, or a failure with its reason; and
/// whether it answers a restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub asset: AssetId,
    pub outcome: Outcome,
    pub marked: bool,
}

impl Arrival {
    pub fn at(asset: AssetId, hash: u64) -> Arrival {
        Arrival { asset, outcome: Outcome::Hash(hash), marked: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Want { asset: AssetId, owner: Owner, policy: Policy },
    Release { asset: AssetId, owner: Owner },
    /// Creates an instance record made from an asset, which wants it.
    Instantiate { asset: AssetId, first_slot: u32, count: u32, policy: Policy, reload: bool },
}

/// The input record of one tick: its commands in order, then its arrivals in
/// identifier order, at most one per asset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub tick: u32,
    pub commands: Vec<Command>,
    pub arrivals: Vec<Arrival>,
}

impl Record {
    pub fn commands(tick: u32, commands: Vec<Command>) -> Record {
        Record { tick, commands, arrivals: Vec::new() }
    }

    pub fn arrivals(tick: u32, mut arrivals: Vec<Arrival>) -> Record {
        arrivals.sort_by_key(|arrival| arrival.asset);
        Record { tick, commands: Vec::new(), arrivals }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.arrivals.is_empty()
    }
}

/// The arrival queue of a world. A batch is posted under one lock, so that a
/// drain never sees a part of it; at most one arrival per asset stands in the
/// queue between two drains, a later one replacing an earlier one.
#[derive(Default)]
pub struct ArrivalQueue {
    arrivals: Mutex<BTreeMap<AssetId, Arrival>>,
}

impl ArrivalQueue {
    pub fn post(&self, batch: &[Arrival]) {
        let mut arrivals = self.arrivals.lock().expect("the queue is never poisoned");
        for arrival in batch {
            arrivals.insert(arrival.asset, *arrival);
        }
    }

    pub fn drain_into(&self, pending: &mut BTreeMap<AssetId, Arrival>) {
        let mut arrivals = self.arrivals.lock().expect("the queue is never poisoned");
        if !arrivals.is_empty() {
            pending.append(&mut arrivals);
        }
    }
}

/// An instance record made from an asset: the consumer of this spike. It is
/// instantiated when its asset first arrives, instantiated again on a change
/// of hash where its reload setting is on, and failed when its asset fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instance {
    pub asset: AssetId,
    pub first_slot: u32,
    pub count: u32,
    pub policy: Policy,
    pub reload: bool,
    pub instantiations: u32,
    pub failed: bool,
}

/// What the core emits when it applies an arrival that is no answer to a
/// restore, and for every failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Arrived { asset: AssetId, hash: u64 },
    HashChanged { asset: AssetId, from: u64, to: u64 },
    Failed { asset: AssetId, code: u32 },
}

// ---------------------------------------------------------------------------
// Canonical bytes
// ---------------------------------------------------------------------------

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(len).filter(|end| *end <= self.bytes.len());
        let end = end.ok_or_else(|| format!("truncated at byte {}", self.at))?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    pub fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn id(&mut self) -> Result<AssetId, String> {
        Ok(u128::from_le_bytes(self.take(16)?.try_into().expect("sixteen bytes")))
    }

    pub fn flag(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(format!("flag {other} at byte {}", self.at - 1)),
        }
    }

    pub fn end(&self) -> Result<(), String> {
        if self.at == self.bytes.len() { Ok(()) } else { Err(format!("{} trailing bytes", self.bytes.len() - self.at)) }
    }
}

fn put_policy(out: &mut Vec<u8>, policy: Policy) {
    out.push(u8::from(policy == Policy::Required));
}

fn read_policy(reader: &mut Reader) -> Result<Policy, String> {
    Ok(if reader.flag()? { Policy::Required } else { Policy::Optional })
}

fn put_owner(out: &mut Vec<u8>, owner: Owner) {
    match owner {
        Owner::World => out.push(0),
        Owner::Entity { slot, generation } => {
            out.push(1);
            out.extend_from_slice(&slot.to_le_bytes());
            out.extend_from_slice(&generation.to_le_bytes());
        }
        Owner::Instance(index) => {
            out.push(2);
            out.extend_from_slice(&index.to_le_bytes());
        }
        Owner::Asset(parent) => {
            out.push(3);
            out.extend_from_slice(&parent.to_le_bytes());
        }
    }
}

fn read_owner(reader: &mut Reader) -> Result<Owner, String> {
    match reader.u8()? {
        0 => Ok(Owner::World),
        1 => Ok(Owner::Entity { slot: reader.u32()?, generation: reader.u32()? }),
        2 => Ok(Owner::Instance(reader.u32()?)),
        3 => Ok(Owner::Asset(reader.id()?)),
        other => Err(format!("owner tag {other}")),
    }
}

fn put_record(out: &mut Vec<u8>, record: &Record) {
    out.extend_from_slice(&record.tick.to_le_bytes());
    out.extend_from_slice(&(record.commands.len() as u32).to_le_bytes());
    for command in &record.commands {
        match *command {
            Command::Want { asset, owner, policy } => {
                out.push(0);
                out.extend_from_slice(&asset.to_le_bytes());
                put_owner(out, owner);
                put_policy(out, policy);
            }
            Command::Release { asset, owner } => {
                out.push(1);
                out.extend_from_slice(&asset.to_le_bytes());
                put_owner(out, owner);
            }
            Command::Instantiate { asset, first_slot, count, policy, reload } => {
                out.push(2);
                out.extend_from_slice(&asset.to_le_bytes());
                out.extend_from_slice(&first_slot.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
                put_policy(out, policy);
                out.push(u8::from(reload));
            }
        }
    }
    out.extend_from_slice(&(record.arrivals.len() as u32).to_le_bytes());
    for arrival in &record.arrivals {
        out.extend_from_slice(&arrival.asset.to_le_bytes());
        match arrival.outcome {
            Outcome::Hash(hash) => {
                out.push(0);
                out.extend_from_slice(&hash.to_le_bytes());
            }
            Outcome::Failure(code) => {
                out.push(1);
                out.extend_from_slice(&u64::from(code).to_le_bytes());
            }
        }
        out.push(u8::from(arrival.marked));
    }
}

fn read_record(reader: &mut Reader) -> Result<Record, String> {
    let tick = reader.u32()?;
    let mut commands = Vec::new();
    for _ in 0..reader.u32()? {
        commands.push(match reader.u8()? {
            0 => Command::Want { asset: reader.id()?, owner: read_owner(reader)?, policy: read_policy(reader)? },
            1 => Command::Release { asset: reader.id()?, owner: read_owner(reader)? },
            2 => Command::Instantiate {
                asset: reader.id()?,
                first_slot: reader.u32()?,
                count: reader.u32()?,
                policy: read_policy(reader)?,
                reload: reader.flag()?,
            },
            other => return Err(format!("command tag {other}")),
        });
    }
    let mut arrivals = Vec::new();
    for _ in 0..reader.u32()? {
        let asset = reader.id()?;
        let outcome = match reader.u8()? {
            0 => Outcome::Hash(reader.u64()?),
            1 => Outcome::Failure(reader.u64()? as u32),
            other => return Err(format!("outcome tag {other}")),
        };
        arrivals.push(Arrival { asset, outcome, marked: reader.flag()? });
    }
    Ok(Record { tick, commands, arrivals })
}

/// The canonical bytes of an input log: the record count, then the records.
pub fn encode_log(records: &[Record]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for record in records {
        put_record(&mut out, record);
    }
    out
}

pub fn decode_log(bytes: &[u8]) -> Result<Vec<Record>, String> {
    let mut reader = Reader::new(bytes);
    let mut records = Vec::new();
    for _ in 0..reader.u32()? {
        records.push(read_record(&mut reader)?);
    }
    reader.end()?;
    Ok(records)
}

// ---------------------------------------------------------------------------
// The asset table
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Entry {
    /// The wants of the asset, in the order of their owners.
    pub wants: Vec<(Owner, Policy)>,
    pub availability: Availability,
    /// The reference to the resident entry a world holds while the asset is available.
    pub resident: Option<Arc<Resident>>,
}

impl Entry {
    fn unavailable() -> Entry {
        Entry { wants: Vec::new(), availability: Availability::Unavailable, resident: None }
    }

    /// The policy the asset is under: required if any want is.
    pub fn policy(&self) -> Option<Policy> {
        self.wants.iter().map(|(_, policy)| *policy).max()
    }
}

/// What applying one input record did.
#[derive(Default)]
pub struct Applied {
    pub events: Vec<Event>,
    /// Arrivals for an asset no want names: recorded, no change.
    pub unnamed_arrivals: u32,
    /// Commands whose owner entity was not alive: recorded, no change.
    pub refused_commands: u32,
    pub fallen: Vec<AssetId>,
}

#[derive(Clone, Default)]
pub struct Table {
    pub entries: BTreeMap<AssetId, Entry>,
    pub instances: Vec<Instance>,
    /// A save loaded as a new run: the assets whose answer the world awaits.
    pub awaiting: BTreeSet<AssetId>,
    /// An entry has no want left: it falls at the next input fixed point.
    orphans: bool,
    /// A required asset is unanswered, or an answer to a restore is awaited.
    blocked: bool,
    entity_wants: usize,
}

impl Table {
    pub fn blocked(&self) -> bool {
        self.blocked
    }

    pub fn has_orphans(&self) -> bool {
        self.orphans
    }

    /// The assets that close the gate: under a required want and unanswered
    /// (a failure is an answer), or awaited by a save loaded as a new run.
    pub fn required_unanswered(&self) -> Vec<AssetId> {
        let mut set: BTreeSet<AssetId> = self.awaiting.clone();
        for (id, entry) in &self.entries {
            if entry.policy() == Some(Policy::Required) && entry.availability == Availability::Unavailable {
                set.insert(*id);
            }
        }
        set.into_iter().collect()
    }

    /// The wanted assets that are unavailable: what the loading capability reads.
    pub fn unanswered(&self) -> BTreeSet<AssetId> {
        let wanted = |entry: &&Entry| !entry.wants.is_empty() && entry.availability == Availability::Unavailable;
        self.entries.iter().filter(|(_, entry)| wanted(entry)).map(|(id, _)| *id).collect()
    }

    /// Adds or replaces the want of an owner on an asset.
    pub fn want(&mut self, asset: AssetId, owner: Owner, policy: Policy) {
        let entry = self.entries.entry(asset).or_insert_with(Entry::unavailable);
        match entry.wants.binary_search_by(|(other, _)| other.cmp(&owner)) {
            Ok(at) => entry.wants[at].1 = policy,
            Err(at) => entry.wants.insert(at, (owner, policy)),
        }
    }

    /// A want the core adds for a dependency: owned by the parent, under its
    /// policy. Returns whether anything changed (a new want, or a raised policy).
    fn want_dependency(&mut self, dependency: AssetId, parent: AssetId, policy: Policy) -> bool {
        let entry = self.entries.entry(dependency).or_insert_with(Entry::unavailable);
        let owner = Owner::Asset(parent);
        match entry.wants.binary_search_by(|(other, _)| other.cmp(&owner)) {
            Ok(at) if entry.wants[at].1 >= policy => false,
            Ok(at) => {
                entry.wants[at].1 = policy;
                true
            }
            Err(at) => {
                entry.wants.insert(at, (owner, policy));
                true
            }
        }
    }

    pub fn release(&mut self, asset: AssetId, owner: Owner) {
        if let Some(entry) = self.entries.get_mut(&asset) {
            entry.wants.retain(|(other, _)| *other != owner);
        }
    }

    /// Ends the wants an entity owned, when it is despawned. Returns whether any ended.
    pub fn end_wants_of_entity(&mut self, slot: u32) -> bool {
        if self.entity_wants == 0 {
            return false;
        }
        let mut ended = false;
        for entry in self.entries.values_mut() {
            let before = entry.wants.len();
            entry.wants.retain(|(owner, _)| !matches!(owner, Owner::Entity { slot: other, .. } if *other == slot));
            ended |= entry.wants.len() != before;
        }
        if ended {
            self.recompute_dependency_wants();
            self.refresh();
        }
        ended
    }

    /// The wants the core holds for dependencies, recomputed from the wanted
    /// assets that are available: a want the core added ends with its
    /// parent's, and follows the parent's part when its hash changes. A want
    /// whose parent's part is not at hand (a restored table awaiting its
    /// answers) is kept as it is.
    fn recompute_dependency_wants(&mut self) {
        let unknown: BTreeSet<AssetId> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.availability, Availability::Available(_)) && entry.resident.is_none())
            .map(|(id, _)| *id)
            .collect();
        for entry in self.entries.values_mut() {
            entry.wants.retain(|(owner, _)| match owner {
                Owner::Asset(parent) => unknown.contains(parent),
                _ => true,
            });
        }
        loop {
            let parents: Vec<(AssetId, Policy, Arc<Resident>)> = self
                .entries
                .iter()
                .filter_map(|(id, entry)| Some((*id, entry.policy()?, entry.resident.clone()?)))
                .filter(|(_, _, resident)| !resident.part.dependencies.is_empty())
                .collect();
            let mut changed = false;
            for (parent, policy, resident) in parents {
                for dependency in &resident.part.dependencies {
                    changed |= self.want_dependency(*dependency, parent, policy);
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// The first half of a batch: a want of every dependency its parts list,
    /// to any depth, so that the order of the identifiers never leaves a
    /// dependency without its want.
    fn want_dependencies_of(&mut self, arrivals: &[Arrival], store: &ResidentStore) {
        loop {
            let mut changed = false;
            for arrival in arrivals {
                let Outcome::Hash(hash) = arrival.outcome else { continue };
                let Some(policy) = self.entries.get(&arrival.asset).and_then(Entry::policy) else { continue };
                for dependency in &resident(store, hash).part.dependencies {
                    changed |= self.want_dependency(*dependency, arrival.asset, policy);
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn apply_arrival(&mut self, arrival: &Arrival, store: &ResidentStore, applied: &mut Applied) {
        let Some(entry) = self.entries.get_mut(&arrival.asset).filter(|entry| !entry.wants.is_empty()) else {
            applied.unnamed_arrivals += 1;
            return;
        };
        let before = entry.availability;
        match arrival.outcome {
            Outcome::Hash(hash) => {
                entry.availability = Availability::Available(hash);
                entry.resident = Some(resident(store, hash));
                // An answer to a restore is no arrival and no reload: no event.
                if !arrival.marked {
                    match before {
                        Availability::Available(old) if old == hash => {}
                        Availability::Available(old) => {
                            applied.events.push(Event::HashChanged { asset: arrival.asset, from: old, to: hash });
                        }
                        _ => applied.events.push(Event::Arrived { asset: arrival.asset, hash }),
                    }
                }
            }
            Outcome::Failure(code) => {
                entry.availability = Availability::Failed(code);
                entry.resident = None;
                applied.events.push(Event::Failed { asset: arrival.asset, code });
            }
        }
        self.awaiting.remove(&arrival.asset);
    }

    /// Removes the entries no want names: the fall to unavailable.
    fn fall(&mut self) -> Vec<AssetId> {
        if !self.orphans {
            return Vec::new();
        }
        let fallen: Vec<AssetId> = self.entries.iter().filter(|(_, entry)| entry.wants.is_empty()).map(|(id, _)| *id).collect();
        for id in &fallen {
            self.entries.remove(id);
            self.awaiting.remove(id);
        }
        fallen
    }

    /// Applies the input record of a tick at its input fixed point, in one
    /// piece: the falls the state owes, the commands in order, then the batch
    /// of arrivals (the wants of its dependencies first, then the arrivals in
    /// identifier order). With `adding` off, the commands that add a want are
    /// skipped: the form the gate is evaluated with.
    pub fn apply(&mut self, record: &Record, store: &ResidentStore, adding: bool, alive: &dyn Fn(u32, u32) -> bool) -> Applied {
        let mut applied = Applied::default();
        if record.is_empty() && !self.orphans {
            return applied;
        }
        applied.fallen = self.fall();
        for command in &record.commands {
            match *command {
                Command::Want { asset, owner, policy } if adding => {
                    if let Owner::Entity { slot, generation } = owner
                        && !alive(slot, generation)
                    {
                        applied.refused_commands += 1;
                        continue;
                    }
                    self.want(asset, owner, policy);
                }
                Command::Instantiate { asset, first_slot, count, policy, reload } if adding => {
                    let index = self.instances.len() as u32;
                    self.instances.push(Instance { asset, first_slot, count, policy, reload, instantiations: 0, failed: false });
                    self.want(asset, Owner::Instance(index), policy);
                }
                Command::Release { asset, owner } => self.release(asset, owner),
                Command::Want { .. } | Command::Instantiate { .. } => {}
            }
        }
        if !record.commands.is_empty() {
            self.recompute_dependency_wants();
        }
        if !record.arrivals.is_empty() {
            self.want_dependencies_of(&record.arrivals, store);
            for arrival in &record.arrivals {
                self.apply_arrival(arrival, store, &mut applied);
            }
            self.recompute_dependency_wants();
        }
        for event in &applied.events {
            self.consume(event);
        }
        self.refresh();
        applied
    }

    /// The consumer's rule, read from an event: an instance is instantiated
    /// when its asset first arrives, again on a change of hash where its
    /// reload setting is on, and failed when its asset fails.
    fn consume(&mut self, event: &Event) {
        for instance in &mut self.instances {
            match *event {
                Event::Arrived { asset, .. } if asset == instance.asset && instance.instantiations == 0 => instance.instantiations = 1,
                Event::HashChanged { asset, .. } if asset == instance.asset && instance.reload => instance.instantiations += 1,
                Event::Failed { asset, .. } if asset == instance.asset => instance.failed = true,
                _ => {}
            }
        }
    }

    /// Whether the gate is open for the tick whose pending record is given:
    /// every required asset of the state is answered already, or is answered
    /// by the record (an arrival of its batch, or a command that releases the
    /// required want). The wants the record adds gate from the next boundary.
    pub fn gate_open(&self, record: &Record, store: &ResidentStore) -> bool {
        if !self.blocked {
            return true;
        }
        if record.is_empty() {
            return false;
        }
        let mut after = self.clone();
        after.apply(record, store, false, &|_, _| true);
        !after.blocked
    }

    /// Recomputes what is derived from the entries. Called after any change.
    pub fn refresh(&mut self) {
        self.orphans = self.entries.values().any(|entry| entry.wants.is_empty());
        self.blocked = !self.required_unanswered().is_empty();
        let owned_by_entity = |(owner, _): &&(Owner, Policy)| matches!(owner, Owner::Entity { .. });
        self.entity_wants = self.entries.values().map(|entry| entry.wants.iter().filter(owned_by_entity).count()).sum();
    }

    /// The canonical bytes of the table: per asset in identifier order, its
    /// availability with the hash or the reason code, and its wants in the
    /// order of their owners; then the instance records in creation order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, entry) in &self.entries {
            out.extend_from_slice(&id.to_le_bytes());
            let (tag, value) = match entry.availability {
                Availability::Unavailable => (0_u8, 0_u64),
                Availability::Available(hash) => (1, hash),
                Availability::Failed(code) => (2, u64::from(code)),
            };
            out.push(tag);
            out.extend_from_slice(&value.to_le_bytes());
            out.extend_from_slice(&(entry.wants.len() as u32).to_le_bytes());
            for (owner, policy) in &entry.wants {
                put_owner(&mut out, *owner);
                put_policy(&mut out, *policy);
            }
        }
        out.extend_from_slice(&(self.instances.len() as u32).to_le_bytes());
        for instance in &self.instances {
            out.extend_from_slice(&instance.asset.to_le_bytes());
            out.extend_from_slice(&instance.first_slot.to_le_bytes());
            out.extend_from_slice(&instance.count.to_le_bytes());
            put_policy(&mut out, instance.policy);
            out.push(u8::from(instance.reload));
            out.extend_from_slice(&instance.instantiations.to_le_bytes());
            out.push(u8::from(instance.failed));
        }
        out
    }

    /// Reads a table back from its canonical bytes. No entry holds a
    /// reference to a resident part: the reader decides how they are had.
    pub fn decode(bytes: &[u8]) -> Result<Table, String> {
        let mut reader = Reader::new(bytes);
        let mut table = Table::default();
        for _ in 0..reader.u32()? {
            let id = reader.id()?;
            let (tag, value) = (reader.u8()?, reader.u64()?);
            let availability = match tag {
                0 => Availability::Unavailable,
                1 => Availability::Available(value),
                2 => Availability::Failed(value as u32),
                other => return Err(format!("availability tag {other}")),
            };
            let mut wants = Vec::new();
            for _ in 0..reader.u32()? {
                wants.push((read_owner(&mut reader)?, read_policy(&mut reader)?));
            }
            table.entries.insert(id, Entry { wants, availability, resident: None });
        }
        for _ in 0..reader.u32()? {
            table.instances.push(Instance {
                asset: reader.id()?,
                first_slot: reader.u32()?,
                count: reader.u32()?,
                policy: read_policy(&mut reader)?,
                reload: reader.flag()?,
                instantiations: reader.u32()?,
                failed: reader.flag()?,
            });
        }
        reader.end()?;
        table.refresh();
        Ok(table)
    }

    pub fn hash(&self) -> u64 {
        fnv1a(&self.canonical_bytes())
    }
}

fn resident(store: &ResidentStore, hash: u64) -> Arc<Resident> {
    store.get(hash).unwrap_or_else(|| panic!("an arrival names the content hash {hash:#018x}, which is not resident"))
}

// ---------------------------------------------------------------------------
// The view the passes resolve identifiers in
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct ViewSlot {
    /// The identifier as two words; both zero marks an empty slot.
    id: [u64; 2],
    table: RepulsionTable,
}

/// The table as the passes read it: an open-addressed index from the 128-bit
/// identifier to the parameters of the asset's part, rebuilt at the input
/// fixed point whenever the table changes. An identifier is random, so its
/// low bits are the hash. An asset that is not available resolves to the
/// declared default.
pub struct View {
    mask: usize,
    slots: Vec<ViewSlot>,
}

impl View {
    pub fn build(table: &Table) -> View {
        let capacity = (table.entries.len() * 2).next_power_of_two().max(16);
        let empty = ViewSlot { id: [0, 0], table: RepulsionTable::DEFAULT };
        let mut view = View { mask: capacity - 1, slots: vec![empty; capacity] };
        for (id, entry) in &table.entries {
            let Some(resident) = &entry.resident else { continue };
            let words = [*id as u64, (*id >> 64) as u64];
            let mut at = words[0] as usize & view.mask;
            while view.slots[at].id != [0, 0] {
                at = (at + 1) & view.mask;
            }
            view.slots[at] = ViewSlot { id: words, table: resident.part.table };
        }
        view
    }

    #[inline]
    pub fn resolve(&self, id: AssetId) -> RepulsionTable {
        let words = [id as u64, (id >> 64) as u64];
        let mut at = words[0] as usize & self.mask;
        loop {
            let slot = &self.slots[at];
            if slot.id == words {
                return slot.table;
            }
            if slot.id == [0, 0] {
                return RepulsionTable::DEFAULT;
            }
            at = (at + 1) & self.mask;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: AssetId = 0x30;
    const MIDDLE: AssetId = 0x20;
    const LEAF: AssetId = 0x10;
    const OTHER: AssetId = 0x40;

    fn alive(_: u32, _: u32) -> bool {
        true
    }

    /// A store with a closure of three levels: the root lists the middle, the middle lists the leaf.
    fn store() -> (ResidentStore, [u64; 4]) {
        let store = ResidentStore::default();
        let mut hashes = [0; 4];
        let parts = [
            part_bytes(RepulsionTable::DEFAULT, &[MIDDLE], 0, 0),
            part_bytes(RepulsionTable::DEFAULT, &[LEAF], 0, 0),
            part_bytes(RepulsionTable::SPECIFICATION, &[], 0, 0),
            part_bytes(RepulsionTable { strength: 48.0, radius: 8.0 }, &[], 0, 0),
        ];
        for (hash, bytes) in hashes.iter_mut().zip(parts) {
            *hash = fnv1a(&bytes);
            store.insert(*hash, bytes).expect("a valid part");
        }
        (store, hashes)
    }

    fn closure(hashes: &[u64; 4], tick: u32) -> Record {
        Record::arrivals(tick, vec![Arrival::at(ROOT, hashes[0]), Arrival::at(MIDDLE, hashes[1]), Arrival::at(LEAF, hashes[2])])
    }

    #[test]
    fn a_batch_wants_its_dependencies_to_any_depth_and_answers_them() {
        let (store, hashes) = store();
        let mut table = Table::default();
        table.want(ROOT, Owner::World, Policy::Required);
        table.refresh();
        assert!(table.blocked());
        // The leaf has the lowest identifier: applied first, it must already be wanted.
        let applied = table.apply(&closure(&hashes, 1), &store, true, &alive);
        assert_eq!(applied.unnamed_arrivals, 0);
        assert_eq!(applied.events.len(), 3);
        assert!(!table.blocked());
        assert_eq!(table.entries[&MIDDLE].wants, vec![(Owner::Asset(ROOT), Policy::Required)]);
        assert_eq!(table.entries[&LEAF].wants, vec![(Owner::Asset(MIDDLE), Policy::Required)]);
        assert_eq!(table.entries[&LEAF].availability, Availability::Available(hashes[2]));
    }

    #[test]
    fn an_arrival_no_want_names_changes_nothing() {
        let (store, hashes) = store();
        let mut table = Table::default();
        let before = table.canonical_bytes();
        let applied = table.apply(&Record::arrivals(1, vec![Arrival::at(LEAF, hashes[2])]), &store, true, &alive);
        assert_eq!(applied.unnamed_arrivals, 1);
        assert!(applied.events.is_empty());
        assert_eq!(table.canonical_bytes(), before);
    }

    #[test]
    fn an_asset_falls_at_the_fixed_point_after_its_last_want_ended() {
        let (store, hashes) = store();
        let mut table = Table::default();
        let owners = [Owner::Entity { slot: 3, generation: 0 }, Owner::Entity { slot: 9, generation: 0 }];
        let wants = owners.iter().map(|owner| Command::Want { asset: LEAF, owner: *owner, policy: Policy::Optional }).collect();
        table.apply(&Record { tick: 1, commands: wants, arrivals: vec![Arrival::at(LEAF, hashes[2])] }, &store, true, &alive);
        assert!(table.end_wants_of_entity(3));
        table.apply(&Record::default(), &store, true, &alive);
        assert_eq!(table.entries[&LEAF].availability, Availability::Available(hashes[2]), "one owner is left");
        assert!(table.end_wants_of_entity(9));
        assert_eq!(table.entries[&LEAF].availability, Availability::Available(hashes[2]), "available until the next fixed point");
        let applied = table.apply(&Record::default(), &store, true, &alive);
        assert_eq!(applied.fallen, vec![LEAF]);
        assert!(table.entries.is_empty());
    }

    #[test]
    fn a_released_parent_takes_its_dependencies_with_it() {
        let (store, hashes) = store();
        let mut table = Table::default();
        table.want(ROOT, Owner::World, Policy::Optional);
        table.apply(&closure(&hashes, 1), &store, true, &alive);
        table.apply(&Record::commands(2, vec![Command::Release { asset: ROOT, owner: Owner::World }]), &store, true, &alive);
        assert_eq!(table.entries.len(), 3, "still there during the tick of the release");
        let applied = table.apply(&Record::default(), &store, true, &alive);
        assert_eq!(applied.fallen, vec![LEAF, MIDDLE, ROOT]);
    }

    #[test]
    fn the_gate_reads_the_pending_record() {
        let (store, hashes) = store();
        let mut table = Table::default();
        table.want(ROOT, Owner::World, Policy::Required);
        table.refresh();
        assert!(!table.gate_open(&Record::default(), &store));
        // The root without its closure leaves a required dependency unanswered.
        let partial = Record::arrivals(1, vec![Arrival::at(ROOT, hashes[0])]);
        assert!(!table.gate_open(&partial, &store));
        assert!(table.gate_open(&closure(&hashes, 1), &store));
        // A failure is an answer, and so is a command that releases the required want.
        let failure = Record::arrivals(1, vec![Arrival { asset: ROOT, outcome: Outcome::Failure(REASON_NOT_FOUND), marked: false }]);
        assert!(table.gate_open(&failure, &store));
        let release = Record::commands(1, vec![Command::Release { asset: ROOT, owner: Owner::World }]);
        assert!(table.gate_open(&release, &store));
        // A want the record adds gates from the next boundary, not this one.
        let mut open = Table::default();
        let want = Record::commands(1, vec![Command::Want { asset: OTHER, owner: Owner::World, policy: Policy::Required }]);
        assert!(open.gate_open(&want, &store));
        open.apply(&want, &store, true, &alive);
        assert!(open.blocked());
    }

    #[test]
    fn a_change_of_hash_is_an_event_and_an_answer_to_a_restore_is_none() {
        let (store, hashes) = store();
        let mut table = Table::default();
        let instantiate = Command::Instantiate { asset: LEAF, first_slot: 0, count: 4, policy: Policy::Required, reload: true };
        table.apply(&Record { tick: 1, commands: vec![instantiate], arrivals: vec![Arrival::at(LEAF, hashes[2])] }, &store, true, &alive);
        assert_eq!(table.instances[0].instantiations, 1);
        let applied = table.apply(&Record::arrivals(2, vec![Arrival::at(LEAF, hashes[3])]), &store, true, &alive);
        assert_eq!(applied.events, vec![Event::HashChanged { asset: LEAF, from: hashes[2], to: hashes[3] }]);
        assert_eq!(table.instances[0].instantiations, 2);
        let marked = Arrival { asset: LEAF, outcome: Outcome::Hash(hashes[2]), marked: true };
        let applied = table.apply(&Record::arrivals(3, vec![marked]), &store, true, &alive);
        assert!(applied.events.is_empty());
        assert_eq!(table.instances[0].instantiations, 2);
        assert_eq!(table.entries[&LEAF].availability, Availability::Available(hashes[2]));
    }

    #[test]
    fn a_command_of_an_entity_that_is_not_alive_is_refused() {
        let (store, _) = store();
        let mut table = Table::default();
        let want = Command::Want { asset: LEAF, owner: Owner::Entity { slot: 1, generation: 0 }, policy: Policy::Optional };
        let applied = table.apply(&Record::commands(1, vec![want]), &store, true, &|_, _| false);
        assert_eq!(applied.refused_commands, 1);
        assert!(table.entries.is_empty());
    }

    #[test]
    fn the_table_and_the_log_round_trip_through_their_canonical_bytes() {
        let (store, hashes) = store();
        let mut table = Table::default();
        let commands = vec![
            Command::Want { asset: ROOT, owner: Owner::World, policy: Policy::Required },
            Command::Want { asset: OTHER, owner: Owner::Entity { slot: 7, generation: 2 }, policy: Policy::Optional },
            Command::Instantiate { asset: LEAF, first_slot: 8, count: 16, policy: Policy::Optional, reload: false },
            Command::Release { asset: OTHER, owner: Owner::World },
        ];
        let failure = Arrival { asset: OTHER, outcome: Outcome::Failure(REASON_INVALID_PART), marked: true };
        let mut record = closure(&hashes, 5);
        record.commands = commands;
        record.arrivals.push(failure);
        table.apply(&record, &store, true, &alive);
        let bytes = table.canonical_bytes();
        let decoded = Table::decode(&bytes).expect("the table decodes");
        assert_eq!(decoded.canonical_bytes(), bytes);
        assert!(Table::decode(&bytes[..bytes.len() - 1]).is_err());
        let log = vec![record, Record::commands(9, Vec::new())];
        let encoded = encode_log(&log);
        assert_eq!(decode_log(&encoded).expect("the log decodes"), log);
        assert!(decode_log(&encoded[..encoded.len() - 2]).is_err());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode_log(&trailing).is_err());
    }

    #[test]
    fn a_part_is_validated_and_the_view_resolves_identifiers() {
        let (store, hashes) = store();
        assert_eq!(store.insert(hashes[2] ^ 1, part_bytes(RepulsionTable::SPECIFICATION, &[], 0, 0)).err(), Some(REASON_HASH_MISMATCH));
        let cut = part_bytes(RepulsionTable::SPECIFICATION, &[LEAF], 8, 0)[..30].to_vec();
        assert_eq!(store.insert(fnv1a(&cut), cut).err(), Some(REASON_INVALID_PART));
        assert_eq!(part_bytes(RepulsionTable::DEFAULT, &[LEAF, ROOT], 100, 1).len(), part_len(2, 100));
        assert_ne!(part_bytes(RepulsionTable::DEFAULT, &[], 4, 1), part_bytes(RepulsionTable::DEFAULT, &[], 4, 2));
        let mut table = Table::default();
        table.want(ROOT, Owner::World, Policy::Optional);
        table.apply(&closure(&hashes, 1), &store, true, &alive);
        let view = View::build(&table);
        assert_eq!(view.resolve(LEAF), RepulsionTable::SPECIFICATION);
        assert_eq!(view.resolve(OTHER), RepulsionTable::DEFAULT);
    }
}
