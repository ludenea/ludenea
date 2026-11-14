//! The asset identity core of the prototype. Throwaway, never product code.
//!
//! It stands on the table of the asset table spike, copied and then changed
//! where the design text states a rule the spike did not hold:
//!
//! - a want of the world's own entry carries the identity of what made it,
//!   so that two makers never share a want;
//! - the order at an input fixed point is the record first (its commands,
//!   then its arrivals), then the falls of the assets whose last want ended
//!   during the tick before and that still have no want;
//! - a despawn is an element of the record;
//! - an answer to a restore that fails is marked like any other;
//! - the parts are read through an extension point, the core holding no
//!   store of its own.
//!
//! Every rule a scenario's control changes on purpose is a field of `Rules`,
//! and so is every place where the text was read one way among several.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use reference_workload::fnv1a;

/// A 128-bit asset identifier, drawn at random outside the domain.
pub type AssetId = u128;

pub const REASON_UNKNOWN_ASSET: u32 = 1;
pub const REASON_INVALID_PART: u32 = 2;
/// The asset table spike's own code for bytes that do not hash to their name;
/// the prototype's loading capability reports it as an invalid part.
pub const REASON_HASH_MISMATCH: u32 = 3;
pub const REASON_ARTIFACT_GONE: u32 = 4;

pub fn reason_text(code: u32) -> &'static str {
    match code {
        REASON_UNKNOWN_ASSET => "unknown asset: the manifest the world stands under does not hold the identifier",
        REASON_INVALID_PART => "the bytes do not hash to their name or the canonical part does not validate",
        REASON_HASH_MISMATCH => "the artifact's bytes do not hash to the announced content hash",
        REASON_ARTIFACT_GONE => "the store holds no bytes under the hash the manifest names",
        _ => "unknown reason",
    }
}

// ---------------------------------------------------------------------------
// The canonical part and the extension point through which parts are read
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

/// A decoded canonical part: its parameters and the assets it depends on.
pub struct Part {
    pub table: RepulsionTable,
    pub dependencies: Vec<AssetId>,
}

/// The canonical bytes of a part: a format word, the strength and the radius,
/// the dependency count and the identifiers, the payload length and the
/// payload, little-endian. The payload is generated from its length and a
/// salt, so that two parts with the same parameters can be distinct content.
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

/// One resident canonical part: its bytes, immutable, with their decoded form.
pub struct Resident {
    pub bytes: Vec<u8>,
    pub part: Part,
}

/// The extension point through which the core reads a part an arrival names.
/// The loading capability's resident store fills it; a harness that fixes
/// availability by hand fills it itself. The core reads no file.
pub trait Residents: Send + Sync {
    fn get(&self, hash: u64) -> Option<Arc<Resident>>;
}

// ---------------------------------------------------------------------------
// Wants, availability, arrivals, commands, the input record
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Policy {
    Optional,
    Required,
}

/// What made a want of the world's own entry: a contributor of the
/// composition, the source of a command, or the harness's stand-in for a
/// scene instance, each with a numeric identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Maker {
    Contributor(u64),
    Source(u64),
    Instance(u32),
}

impl Maker {
    fn kind(self) -> u8 {
        match self {
            Maker::Contributor(_) => 0,
            Maker::Source(_) => 1,
            Maker::Instance(_) => 2,
        }
    }

    fn identity(self) -> u64 {
        match self {
            Maker::Contributor(identity) | Maker::Source(identity) => identity,
            Maker::Instance(index) => u64::from(index),
        }
    }
}

/// Who a want belongs to: the world's own entry (with what made the want), an
/// entity, or (a want the core added) the asset whose part lists this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    World(Maker),
    Entity { slot: u32, generation: u32 },
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

/// What the availability source posts per asset of a batch: the identifier
/// and the content hash of its canonical part, or a failure with its reason;
/// and whether it answers a restore.
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

    pub fn failed(asset: AssetId, code: u32) -> Arrival {
        Arrival { asset, outcome: Outcome::Failure(code), marked: false }
    }

    pub fn marked(mut self) -> Arrival {
        self.marked = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Want { asset: AssetId, owner: Owner, policy: Policy },
    Release { asset: AssetId, owner: Owner },
    /// Despawns an entity; the wants it owns end.
    Despawn { slot: u32, generation: u32 },
    /// Creates the stand-in for a scene instance made from an asset, which
    /// wants it under the world's own entry with an identity of its own.
    Instantiate { asset: AssetId, policy: Policy, reload: bool },
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

/// The harness's stand-in for a scene instance made from an asset. It is
/// instantiated when its asset first arrives, instantiated again on a change
/// of hash where its reload setting is on, and failed when its asset fails.
/// It is a consumer's state, serialized after the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instance {
    pub asset: AssetId,
    pub policy: Policy,
    pub reload: bool,
    pub instantiations: u32,
    pub failed: bool,
}

/// What the core emits when it applies an arrival that is no answer to a
/// restore; and the one event that records a restore whose answers differed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Arrived { asset: AssetId, hash: u64 },
    HashChanged { asset: AssetId, from: u64, to: u64 },
    Failed { asset: AssetId, code: u32 },
    /// A restore under the content at hand started a new history: the assets
    /// whose answer differed from the recorded hash.
    NewHistory { assets: Vec<AssetId> },
}

/// The rules a control changes on purpose, and the readings taken where the
/// text gave two behaviors or none. The default is the build's reading of the
/// text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Control: the arrivals of a record are applied before its commands.
    pub arrivals_before_commands: bool,
    /// Control: the falls are applied before the record (the spike's order).
    pub falls_before_record: bool,
    /// Control: the gate counts a pending command that adds a want.
    pub gate_counts_added_wants: bool,
    /// Control: the gate sees a pending despawn of an owner.
    pub gate_sees_despawns: bool,
    /// Control: an entry's wants stand in the order of making.
    pub wants_in_making_order: bool,
    /// Control: a dependency holds one want per want of its parent.
    pub dependency_want_per_parent_want: bool,
    /// Control: the wants the core added are left as they were when the
    /// parent's hash changes.
    pub core_wants_left: bool,
    /// Control: every maker shares one want under the world's own entry.
    pub one_want_under_the_world: bool,
    /// Control: a second want from one maker is added beside the first.
    pub second_want_added: bool,
    /// Control: a restore under the content at hand awaits an answer for an
    /// entry that holds no want.
    pub restore_awaits_entries_without_a_want: bool,
    /// Reading: the makers under the world's own entry are ordered by their
    /// numeric identity alone, the kind breaking a tie. The default orders
    /// contributors, then command sources, then instances, each by identity.
    pub makers_by_identity_alone: bool,
    /// Reading: a despawn of the record ends its entity's wants at the
    /// structural change fixed point of its tick, after the tick's arrivals.
    /// The default ends them where the command stands in the record.
    pub despawn_ends_wants_at_the_structural_point: bool,
    /// Reading: an answer to a restore that fails emits its event and fails
    /// the instance made from the asset. The default emits nothing on a
    /// marked transition.
    pub marked_failure_emits: bool,
    /// Reading: an answer to a restore that fails emits no event and still
    /// fails the instance made from the asset.
    pub marked_failure_fails_the_instance: bool,
    /// Reading: the event that records a restore that started a new history
    /// stands before the events of its tick's record. The default places it
    /// after them.
    pub restore_event_first: bool,
}

impl Rules {
    fn owner_order(&self, a: &Owner, b: &Owner) -> Ordering {
        let rank = |owner: &Owner| match owner {
            Owner::World(_) => 0,
            Owner::Entity { .. } => 1,
            Owner::Asset(_) => 2,
        };
        match (a, b) {
            (Owner::World(a), Owner::World(b)) => {
                if self.makers_by_identity_alone {
                    (a.identity(), a.kind()).cmp(&(b.identity(), b.kind()))
                } else {
                    (a.kind(), a.identity()).cmp(&(b.kind(), b.identity()))
                }
            }
            (Owner::Entity { slot: a, generation: ga }, Owner::Entity { slot: b, generation: gb }) => (a, ga).cmp(&(b, gb)),
            (Owner::Asset(a), Owner::Asset(b)) => a.cmp(b),
            _ => rank(a).cmp(&rank(b)),
        }
    }

    /// The owner a want is held under: under the control that shares one
    /// want under the world's own entry, whichever maker already holds it.
    fn owner(&self, wants: &[(Owner, Policy)], owner: Owner) -> Owner {
        match owner {
            Owner::World(_) if self.one_want_under_the_world => {
                wants.iter().map(|(held, _)| *held).find(|held| matches!(held, Owner::World(_))).unwrap_or(owner)
            }
            other => other,
        }
    }
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
        Owner::World(maker) => {
            out.push(0);
            out.push(maker.kind());
            out.extend_from_slice(&maker.identity().to_le_bytes());
        }
        Owner::Entity { slot, generation } => {
            out.push(1);
            out.extend_from_slice(&slot.to_le_bytes());
            out.extend_from_slice(&generation.to_le_bytes());
        }
        Owner::Asset(parent) => {
            out.push(2);
            out.extend_from_slice(&parent.to_le_bytes());
        }
    }
}

fn read_owner(reader: &mut Reader) -> Result<Owner, String> {
    match reader.u8()? {
        0 => {
            let (kind, identity) = (reader.u8()?, reader.u64()?);
            Ok(Owner::World(match kind {
                0 => Maker::Contributor(identity),
                1 => Maker::Source(identity),
                2 => Maker::Instance(identity as u32),
                other => return Err(format!("maker kind {other}")),
            }))
        }
        1 => Ok(Owner::Entity { slot: reader.u32()?, generation: reader.u32()? }),
        2 => Ok(Owner::Asset(reader.id()?)),
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
            Command::Instantiate { asset, policy, reload } => {
                out.push(2);
                out.extend_from_slice(&asset.to_le_bytes());
                put_policy(out, policy);
                out.push(u8::from(reload));
            }
            Command::Despawn { slot, generation } => {
                out.push(3);
                out.extend_from_slice(&slot.to_le_bytes());
                out.extend_from_slice(&generation.to_le_bytes());
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
            2 => Command::Instantiate { asset: reader.id()?, policy: read_policy(reader)?, reload: reader.flag()? },
            3 => Command::Despawn { slot: reader.u32()?, generation: reader.u32()? },
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
    /// The wants of the asset, in their total order.
    pub wants: Vec<(Owner, Policy)>,
    pub availability: Availability,
    /// The reference to the resident part a world holds while the asset is available.
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
    /// The entities the record despawns, for the world to take out.
    pub despawned: Vec<(u32, u32)>,
    /// The assets a command of the record made a want of, in order.
    pub wanted: Vec<AssetId>,
    /// The assets that failed under a required want, with the reason.
    pub failed_required: Vec<(AssetId, u32)>,
    /// The answers to a restore that differed from the hash the table held.
    pub differed: Vec<AssetId>,
    /// Whether the record held an answer to a restore.
    pub answered_a_restore: bool,
}

/// What a record is applied for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Tick,
    Gate,
}

#[derive(Clone, Default)]
pub struct Table {
    pub entries: BTreeMap<AssetId, Entry>,
    pub instances: Vec<Instance>,
    /// A restore under the content at hand: the assets whose answer the world awaits.
    pub awaiting: BTreeSet<AssetId>,
    pub rules: Rules,
    /// An entry has no want left: it falls at the next input fixed point.
    orphans: bool,
    /// A required asset is unanswered, or an answer to a restore is awaited.
    blocked: bool,
    entity_wants: usize,
}

impl Table {
    pub fn with_rules(rules: Rules) -> Table {
        Table { rules, ..Table::default() }
    }

    pub fn blocked(&self) -> bool {
        self.blocked
    }

    pub fn has_orphans(&self) -> bool {
        self.orphans
    }

    /// The assets that close the gate: under a required want and unanswered
    /// (a failure is an answer), or awaited by a restore under the content at hand.
    pub fn required_unanswered(&self) -> Vec<AssetId> {
        let mut set: BTreeSet<AssetId> = self.awaiting.clone();
        for (id, entry) in &self.entries {
            if entry.policy() == Some(Policy::Required) && entry.availability == Availability::Unavailable {
                set.insert(*id);
            }
        }
        set.into_iter().collect()
    }

    /// The wanted assets that are unavailable.
    pub fn unanswered(&self) -> BTreeSet<AssetId> {
        let wanted = |entry: &&Entry| !entry.wants.is_empty() && entry.availability == Availability::Unavailable;
        self.entries.iter().filter(|(_, entry)| wanted(entry)).map(|(id, _)| *id).collect()
    }

    /// The assets a want names.
    pub fn wanted(&self) -> BTreeSet<AssetId> {
        self.entries.iter().filter(|(_, entry)| !entry.wants.is_empty()).map(|(id, _)| *id).collect()
    }

    fn place(&self, wants: &[(Owner, Policy)], owner: &Owner) -> usize {
        if self.rules.wants_in_making_order {
            return wants.len();
        }
        wants.iter().position(|(other, _)| self.rules.owner_order(other, owner) == Ordering::Greater).unwrap_or(wants.len())
    }

    /// Adds the want of an owner on an asset, or changes its policy: an
    /// owner, or a maker under the world's own entry, holds at most one.
    pub fn want(&mut self, asset: AssetId, owner: Owner, policy: Policy) {
        let rules = self.rules;
        let entry = self.entries.entry(asset).or_insert_with(Entry::unavailable);
        let owner = rules.owner(&entry.wants, owner);
        if !rules.second_want_added
            && let Some(at) = entry.wants.iter().position(|(other, _)| *other == owner)
        {
            entry.wants[at].1 = policy;
            return;
        }
        let at = if rules.wants_in_making_order {
            entry.wants.len()
        } else {
            entry.wants.iter().position(|(other, _)| rules.owner_order(other, &owner) == Ordering::Greater).unwrap_or(entry.wants.len())
        };
        entry.wants.insert(at, (owner, policy));
    }

    pub fn release(&mut self, asset: AssetId, owner: Owner) {
        let rules = self.rules;
        if let Some(entry) = self.entries.get_mut(&asset) {
            let owner = rules.owner(&entry.wants, owner);
            if let Some(at) = entry.wants.iter().position(|(other, _)| *other == owner) {
                entry.wants.remove(at);
            }
        }
    }

    /// Ends the wants an entity owned, when it is despawned. With a
    /// generation, the wants of that entity alone; without, of whatever
    /// entity holds the slot. Returns whether any ended.
    pub fn end_wants_of_entity(&mut self, slot: u32, generation: Option<u32>) -> bool {
        if self.entity_wants == 0 {
            return false;
        }
        let mut ended = false;
        for entry in self.entries.values_mut() {
            let before = entry.wants.len();
            entry.wants.retain(|(owner, _)| match owner {
                Owner::Entity { slot: other, generation: held } => *other != slot || generation.is_some_and(|generation| generation != *held),
                _ => true,
            });
            ended |= entry.wants.len() != before;
        }
        if ended {
            self.recompute_dependency_wants();
            self.refresh();
        }
        ended
    }

    /// The wants the core holds for dependencies, recomputed from the wanted
    /// assets whose part is at hand: a dependency holds one want per parent,
    /// required if any want of the parent is, which ends with the parent's
    /// last want and follows the parent's part when its hash changes. A want
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
            let parents: Vec<(AssetId, Vec<Policy>, Arc<Resident>)> = self
                .entries
                .iter()
                .filter(|(_, entry)| !entry.wants.is_empty())
                .filter_map(|(id, entry)| Some((*id, entry.wants.iter().map(|(_, policy)| *policy).collect(), entry.resident.clone()?)))
                .filter(|(_, _, resident)| !resident.part.dependencies.is_empty())
                .collect();
            let mut changed = false;
            for (parent, policies, resident) in parents {
                for dependency in &resident.part.dependencies {
                    changed |= self.want_dependency(*dependency, parent, &policies);
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// A want the core adds for a dependency: owned by the parent, required
    /// if any want of the parent is. Returns whether anything changed.
    fn want_dependency(&mut self, dependency: AssetId, parent: AssetId, parent_policies: &[Policy]) -> bool {
        let owner = Owner::Asset(parent);
        let wanted: Vec<Policy> = if self.rules.dependency_want_per_parent_want {
            parent_policies.to_vec()
        } else {
            parent_policies.iter().copied().max().into_iter().collect()
        };
        let held: Vec<Policy> = self.entries.get(&dependency).map_or(Vec::new(), |entry| {
            entry.wants.iter().filter(|(other, _)| *other == owner).map(|(_, policy)| *policy).collect()
        });
        if held == wanted {
            return false;
        }
        let at = {
            let entry = self.entries.entry(dependency).or_insert_with(Entry::unavailable);
            entry.wants.retain(|(other, _)| *other != owner);
            entry.wants.clone()
        };
        let at = self.place(&at, &owner);
        let entry = self.entries.get_mut(&dependency).expect("the entry was made above");
        for (k, policy) in wanted.into_iter().enumerate() {
            entry.wants.insert(at + k, (owner, policy));
        }
        true
    }

    /// The first half of a batch: a want of every dependency its parts list,
    /// to any depth, so that the order of the identifiers never leaves a
    /// dependency without its want.
    fn want_dependencies_of(&mut self, arrivals: &[Arrival], residents: &dyn Residents) {
        loop {
            let mut changed = false;
            for arrival in arrivals {
                let Outcome::Hash(hash) = arrival.outcome else { continue };
                let Some(entry) = self.entries.get(&arrival.asset).filter(|entry| !entry.wants.is_empty()) else { continue };
                if self.rules.core_wants_left && matches!(entry.availability, Availability::Available(_)) {
                    continue;
                }
                let policies: Vec<Policy> = entry.wants.iter().map(|(_, policy)| *policy).collect();
                for dependency in &resident(residents, hash).part.dependencies {
                    changed |= self.want_dependency(*dependency, arrival.asset, &policies);
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn apply_arrival(&mut self, arrival: &Arrival, residents: &dyn Residents, applied: &mut Applied) {
        applied.answered_a_restore |= arrival.marked;
        let emits = !arrival.marked;
        let failure_emits = emits || self.rules.marked_failure_emits;
        self.awaiting.remove(&arrival.asset);
        let Some(entry) = self.entries.get_mut(&arrival.asset).filter(|entry| !entry.wants.is_empty()) else {
            applied.unnamed_arrivals += 1;
            return;
        };
        let before = entry.availability;
        match arrival.outcome {
            Outcome::Hash(hash) => {
                entry.availability = Availability::Available(hash);
                entry.resident = Some(resident(residents, hash));
                match before {
                    Availability::Available(old) if old == hash => {}
                    // An answer to a restore is no arrival and no reload: no event.
                    Availability::Available(old) if emits => applied.events.push(Event::HashChanged { asset: arrival.asset, from: old, to: hash }),
                    _ if emits => applied.events.push(Event::Arrived { asset: arrival.asset, hash }),
                    _ => {}
                }
                if arrival.marked && before != Availability::Available(hash) {
                    applied.differed.push(arrival.asset);
                }
            }
            Outcome::Failure(code) => {
                entry.availability = Availability::Failed(code);
                entry.resident = None;
                if failure_emits {
                    applied.events.push(Event::Failed { asset: arrival.asset, code });
                } else if self.rules.marked_failure_fails_the_instance {
                    for instance in self.instances.iter_mut().filter(|instance| instance.asset == arrival.asset) {
                        instance.failed = true;
                    }
                }
                if entry.policy() == Some(Policy::Required) {
                    applied.failed_required.push((arrival.asset, code));
                }
                if arrival.marked {
                    applied.differed.push(arrival.asset);
                }
            }
        }
    }

    fn apply_command(&mut self, command: &Command, purpose: Purpose, alive: &dyn Fn(u32, u32) -> bool, applied: &mut Applied) {
        let adding = purpose == Purpose::Tick || self.rules.gate_counts_added_wants;
        let despawning = purpose == Purpose::Tick || self.rules.gate_sees_despawns;
        match *command {
            Command::Want { asset, owner, policy } if adding => {
                if let Owner::Entity { slot, generation } = owner
                    && (!alive(slot, generation) || applied.despawned.contains(&(slot, generation)))
                {
                    applied.refused_commands += 1;
                    return;
                }
                self.want(asset, owner, policy);
                applied.wanted.push(asset);
            }
            Command::Instantiate { asset, policy, reload } if adding => {
                self.instances.push(Instance { asset, policy, reload, instantiations: 0, failed: false });
                self.want(asset, Owner::World(Maker::Instance(self.instances.len() as u32)), policy);
                applied.wanted.push(asset);
            }
            Command::Release { asset, owner } => self.release(asset, owner),
            Command::Despawn { slot, generation } if despawning => {
                if !alive(slot, generation) || applied.despawned.contains(&(slot, generation)) {
                    applied.refused_commands += 1;
                    return;
                }
                applied.despawned.push((slot, generation));
                if !self.rules.despawn_ends_wants_at_the_structural_point {
                    for entry in self.entries.values_mut() {
                        entry.wants.retain(|(owner, _)| *owner != Owner::Entity { slot, generation });
                    }
                }
            }
            Command::Want { .. } | Command::Instantiate { .. } | Command::Despawn { .. } => {}
        }
        self.recompute_dependency_wants();
    }

    fn apply_arrivals(&mut self, record: &Record, residents: &dyn Residents, applied: &mut Applied) {
        if record.arrivals.is_empty() {
            return;
        }
        self.want_dependencies_of(&record.arrivals, residents);
        for arrival in &record.arrivals {
            self.apply_arrival(arrival, residents, applied);
        }
        if !self.rules.core_wants_left {
            self.recompute_dependency_wants();
        }
    }

    /// Removes the entries that had no want before the record and that no
    /// want named while it was applied: the fall to unavailable.
    fn fall(&mut self, candidates: &[AssetId], wanted_since: &BTreeSet<AssetId>) -> Vec<AssetId> {
        let mut fallen = Vec::new();
        for id in candidates {
            if !wanted_since.contains(id) && self.entries.get(id).is_some_and(|entry| entry.wants.is_empty()) {
                self.entries.remove(id);
                self.awaiting.remove(id);
                fallen.push(*id);
            }
        }
        fallen
    }

    /// Applies the input record of a tick at its input fixed point, in one
    /// piece: the record in canonical order (its commands, then the batch of
    /// its arrivals: the wants of the batch's dependencies first, then the
    /// arrivals in identifier order), then the falls of the assets whose last
    /// want ended during the tick before and that still have no want.
    pub fn apply(&mut self, record: &Record, residents: &dyn Residents, alive: &dyn Fn(u32, u32) -> bool) -> Applied {
        self.apply_for(record, residents, Purpose::Tick, alive)
    }

    fn apply_for(&mut self, record: &Record, residents: &dyn Residents, purpose: Purpose, alive: &dyn Fn(u32, u32) -> bool) -> Applied {
        let mut applied = Applied::default();
        if record.is_empty() && !self.orphans {
            return applied;
        }
        // The entries whose last want ended during the tick before.
        let candidates: Vec<AssetId> = self.entries.iter().filter(|(_, entry)| entry.wants.is_empty()).map(|(id, _)| *id).collect();
        if self.rules.falls_before_record {
            applied.fallen = self.fall(&candidates, &BTreeSet::new());
        }
        // A candidate a want names while the record is applied did not end
        // its last want during the tick before, even if that want is ended
        // again by a later element of the same record.
        let mut wanted_since = BTreeSet::new();
        if self.rules.arrivals_before_commands {
            self.apply_arrivals(record, residents, &mut applied);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        for command in &record.commands {
            self.apply_command(command, purpose, alive, &mut applied);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        if !self.rules.arrivals_before_commands {
            self.apply_arrivals(record, residents, &mut applied);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        if !self.rules.falls_before_record {
            applied.fallen = self.fall(&candidates, &wanted_since);
        }
        for event in &applied.events {
            self.consume(event);
        }
        self.refresh();
        applied
    }

    fn note_wanted(&self, candidates: &[AssetId], wanted_since: &mut BTreeSet<AssetId>) {
        for id in candidates {
            if self.entries.get(id).is_some_and(|entry| !entry.wants.is_empty()) {
                wanted_since.insert(*id);
            }
        }
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
    /// by a pending arrival, a pending command that releases a required want
    /// counting as its answer. A pending command that adds a want is left
    /// out, and a pending despawn of an owner is not seen.
    pub fn gate_open(&self, record: &Record, residents: &dyn Residents) -> bool {
        if !self.blocked {
            return true;
        }
        if record.is_empty() {
            return false;
        }
        let mut after = self.clone();
        after.apply_for(record, residents, Purpose::Gate, &|_, _| true);
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
    /// availability with the hash or the reason code, and its wants in their
    /// total order; then the instance records in creation order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, entry) in &self.entries {
            put_entry(&mut out, *id, entry.availability, &entry.wants);
        }
        put_instances(&mut out, &self.instances);
        out
    }

    /// Reads a table back from its canonical bytes. No entry holds a
    /// reference to a resident part: the reader decides how they are had.
    pub fn decode(bytes: &[u8], rules: Rules) -> Result<Table, String> {
        let mut reader = Reader::new(bytes);
        let mut table = Table::with_rules(rules);
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

/// One entry in the canonical form. Public so that a harness can write a
/// table stated by hand through the same bytes.
pub fn put_entry(out: &mut Vec<u8>, id: AssetId, availability: Availability, wants: &[(Owner, Policy)]) {
    out.extend_from_slice(&id.to_le_bytes());
    let (tag, value) = match availability {
        Availability::Unavailable => (0_u8, 0_u64),
        Availability::Available(hash) => (1, hash),
        Availability::Failed(code) => (2, u64::from(code)),
    };
    out.push(tag);
    out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(&(wants.len() as u32).to_le_bytes());
    for (owner, policy) in wants {
        put_owner(out, *owner);
        put_policy(out, *policy);
    }
}

pub fn put_instances(out: &mut Vec<u8>, instances: &[Instance]) {
    out.extend_from_slice(&(instances.len() as u32).to_le_bytes());
    for instance in instances {
        out.extend_from_slice(&instance.asset.to_le_bytes());
        put_policy(out, instance.policy);
        out.push(u8::from(instance.reload));
        out.extend_from_slice(&instance.instantiations.to_le_bytes());
        out.push(u8::from(instance.failed));
    }
}

fn resident(residents: &dyn Residents, hash: u64) -> Arc<Resident> {
    residents.get(hash).unwrap_or_else(|| panic!("an arrival names the content hash {hash:#018x}, which is not resident"))
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
/// fixed point whenever the table changes. An asset that is not available
/// resolves to the declared default.
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
