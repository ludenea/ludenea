//! The asset identity core of the prototype. Throwaway, never product code.
//!
//! It stands on the table of the asset table spike, copied and then changed
//! where the design text states a rule the spike did not hold:
//!
//! - a want of the world's own entry carries the kind and the identity of
//!   what made it, so that two makers never share a want;
//! - the order at an input fixed point is the record first (its commands,
//!   then its arrivals), then the falls of the assets whose last want ended
//!   during the tick before and that still have no want;
//! - a despawn is an element of the record;
//! - an entry holds its asset type, stated by its first want and carried by
//!   the canonical bytes as the stable name of the type's schema entry; a
//!   want states a type, and one that states another type than its entry
//!   holds is a recorded refusal, but on an entry failed for that reason;
//! - a want added on a failed entry returns it to unavailable;
//! - the wants of dependencies are added and recomputed wherever a want is
//!   applied on, or released from, a parent in force;
//! - an arrival that leaves an entry as it was emits no event, and an answer
//!   to a restore that fails emits its event as any failure does;
//! - the parts are read through an extension point, the core holding no
//!   store of its own.
//!
//! Then changed again where a reading of the design text was taken:
//!
//! - the gate is evaluated in an incremental form: the table keeps the set
//!   of the required assets it holds unanswered, derived and no part of the
//!   canonical form, and the gate reads the pending arrivals and the pending
//!   releases against it, and against the count of the entries a restore
//!   still awaits; it copies nothing, walks no closure, and does not see the
//!   wants a pending element would end through the recomputation;
//! - each system is the maker of the want it makes, a third kind of maker
//!   between contributors and command sources, known by its stable name;
//! - an arrival to a hash names the size of what it puts in force and the
//!   entry holds it, both in the canonical bytes; the count of a budget
//!   reads those sizes;
//! - the unit of a budget is the group of a root, the groups of the roots
//!   that hold a required want tested first;
//! - on an entry failed for its type, a want the core adds takes the type
//!   its parent's part lists.
//!
//! Every rule a scenario's control changes on purpose is a field of `Rules`,
//! and so is every place where the text was read one way among several, and
//! every former default of the build that a reading taken replaced.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reference_workload::fnv1a;

/// A 128-bit asset identifier, drawn at random outside the domain.
pub type AssetId = u128;

// ---------------------------------------------------------------------------
// The asset types and the reasons of a failure: entries of the schema
// ---------------------------------------------------------------------------

/// An asset type: an entry of the schema, known by its stable name. The
/// prototype's schema holds three synthetic types: the parameters every part
/// of the stand-in world carries, a second one so that a composition can
/// lack a type, and a third of which no asset exists, which a composition
/// or a request alone names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssetType {
    Parameters,
    Second,
    Third,
}

impl AssetType {
    pub const ALL: [AssetType; 3] = [AssetType::Parameters, AssetType::Second, AssetType::Third];

    /// The stable name of the type's schema entry: a namespace and a local
    /// name. It is what the canonical forms write.
    pub fn stable_name(self) -> &'static str {
        match self {
            AssetType::Parameters => "prototype/parameters",
            AssetType::Second => "prototype/second",
            AssetType::Third => "prototype/third",
        }
    }

    pub fn named(name: &str) -> Option<AssetType> {
        AssetType::ALL.into_iter().find(|asset_type| asset_type.stable_name() == name)
    }
}

pub const REASON_UNKNOWN_ASSET: u32 = 1;
pub const REASON_INVALID_PART: u32 = 2;
/// The asset table spike's own code for bytes that do not hash to their name;
/// the prototype's loading capability reports it as an invalid part.
pub const REASON_HASH_MISMATCH: u32 = 3;
pub const REASON_ARTIFACT_GONE: u32 = 4;
pub const REASON_ANOTHER_TYPE: u32 = 5;
pub const REASON_DEPENDENCIES_DIFFER: u32 = 6;
pub const REASON_ANOTHER_SCHEMA_ENTRY: u32 = 7;
pub const REASON_ADMISSION_REFUSED: u32 = 8;
pub const REASON_PREPARATION_FAILED: u32 = 9;
/// The reason of an arrival rejected over a world's budget, where the table
/// does not hold the asset available. The design text names no such reason:
/// it exists for the comparison of the candidates of a budget.
pub const REASON_OVER_BUDGET: u32 = 10;
/// The reason of a first load that the host's ceiling on a decoder at load
/// stops: the artifact declares more than the ceiling lets the decoder hold.
/// The design text lists no reason that fits it.
pub const REASON_OVER_CEILING: u32 = 11;

/// One reason of a failure: its code in the canonical form, a short name and
/// what it says.
pub struct Reason {
    pub code: u32,
    pub name: &'static str,
    pub text: &'static str,
}

/// The reasons of a failure: a list in the schema entry of the arrival
/// transition. Two of them are named by the design text and raised by
/// nothing this prototype builds: another schema entry, a preparation. The
/// reason of an admission check is raised by the loading capability where
/// the check of a world's composition refuses a part on a first load; the
/// design text lists it and gives it no name, and `not-admitted` is the
/// name the scenario sheets give it.
pub const REASONS: [Reason; 11] = [
    Reason { code: REASON_UNKNOWN_ASSET, name: "unknown-asset", text: "unknown asset: the manifest the world stands under does not hold the identifier" },
    Reason { code: REASON_INVALID_PART, name: "invalid-part", text: "the bytes do not hash to their name or the canonical part does not validate" },
    Reason { code: REASON_HASH_MISMATCH, name: "hash-mismatch", text: "the artifact's bytes do not hash to the announced content hash" },
    Reason { code: REASON_ARTIFACT_GONE, name: "artifact-gone", text: "the store holds no bytes under the hash the manifest names" },
    Reason { code: REASON_ANOTHER_TYPE, name: "other-type", text: "the manifest holds the asset under another type than the one its entry holds" },
    Reason { code: REASON_DEPENDENCIES_DIFFER, name: "dependencies-differ", text: "the dependencies the part lists are not those the manifest lists for it" },
    Reason { code: REASON_ANOTHER_SCHEMA_ENTRY, name: "another-schema-entry", text: "the package was built under another schema entry of the asset's type" },
    Reason { code: REASON_ADMISSION_REFUSED, name: "not-admitted", text: "the admission check of the world's composition refused the part, on a first load" },
    Reason { code: REASON_PREPARATION_FAILED, name: "preparation-failed", text: "the preparation of a form failed on a first load" },
    Reason { code: REASON_OVER_BUDGET, name: "over-budget", text: "the arrival was rejected over the world's budget for its asset data" },
    Reason { code: REASON_OVER_CEILING, name: "over-ceiling", text: "the artifact declares more than the host's ceiling lets a decoder at load hold" },
];

pub fn reason(code: u32) -> Option<&'static Reason> {
    REASONS.iter().find(|reason| reason.code == code)
}

pub fn reason_named(name: &str) -> Option<&'static Reason> {
    REASONS.iter().find(|reason| reason.name == name)
}

pub fn reason_text(code: u32) -> &'static str {
    reason(code).map_or("unknown reason", |reason| reason.text)
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

/// A domain dependency as a canonical part lists it: the identifier and the
/// dependency's own asset type, as the domain manifest holds it.
pub type Dependency = (AssetId, AssetType);

/// A decoded canonical part: its parameters and the assets it depends on.
pub struct Part {
    pub table: RepulsionTable,
    pub dependencies: Vec<Dependency>,
}

fn put_type(out: &mut Vec<u8>, asset_type: AssetType) {
    let name = asset_type.stable_name().as_bytes();
    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
    out.extend_from_slice(name);
}

/// The canonical bytes of a part: a format word, the strength and the radius,
/// the dependency count and, per dependency, its identifier and the stable
/// name of its asset type, the payload length and the payload, little-endian.
/// The payload is generated from its length and a salt, so that two parts
/// with the same parameters can be distinct content. A part that lists no
/// dependency has the bytes it had before a dependency stated its type.
pub fn part_bytes(table: RepulsionTable, dependencies: &[Dependency], payload_len: usize, salt: u32) -> Vec<u8> {
    let len = part_len(dependencies, payload_len);
    let mut bytes = Vec::with_capacity(len);
    bytes.extend_from_slice(&PART_FORMAT.to_le_bytes());
    bytes.extend_from_slice(&table.strength.to_le_bytes());
    bytes.extend_from_slice(&table.radius.to_le_bytes());
    bytes.extend_from_slice(&(dependencies.len() as u32).to_le_bytes());
    for (dependency, asset_type) in dependencies {
        bytes.extend_from_slice(&dependency.to_le_bytes());
        put_type(&mut bytes, *asset_type);
    }
    bytes.extend_from_slice(&(payload_len as u32).to_le_bytes());
    bytes.extend((0..payload_len).map(|k| (k as u32).wrapping_mul(31).wrapping_add(salt).to_le_bytes()[k % 4]));
    debug_assert_eq!(bytes.len(), len);
    bytes
}

pub fn part_len(dependencies: &[Dependency], payload_len: usize) -> usize {
    let listed: usize = dependencies.iter().map(|(_, asset_type)| 20 + asset_type.stable_name().len()).sum();
    20 + listed + payload_len
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
                dependencies.push((reader.id()?, reader.asset_type()?));
            }
            let payload_len = reader.u32()? as usize;
            reader.take(payload_len)?;
            reader.end()?;
            Ok(Part { table, dependencies })
        };
        decode().map_err(|_| REASON_INVALID_PART)
    }
}

/// An admission check: what the schema entry of an asset type may carry,
/// supplied by the capability that declares the type. The loading
/// capability runs it before an arrival is posted, and the core never does.
/// No asset type of the prototype has one: this is the stand-in a scenario
/// sheet defines for the comparison alone, which a composition that carries
/// it gives to the entry of the type of parameters, with one number, the
/// bound. It reads one canonical part of that type and nothing else, neither
/// the table, the manifest, the store nor another part, and refuses the part
/// where the first parameter the type's layout declares is greater than the
/// bound. The bound stands inside the values the type's validator accepts,
/// so a part the check refuses is a valid part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdmissionCheck {
    pub bound: f32,
}

impl AdmissionCheck {
    /// Whether the check accepts a part: it refuses one whose first
    /// parameter is greater than the bound, and accepts every other.
    pub fn admits(&self, part: &Part) -> bool {
        part.table.strength.partial_cmp(&self.bound) != Some(std::cmp::Ordering::Greater)
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

    /// The bytes of the prepared form of a part, as a system of the
    /// capability that declares the form reads it inside the domain: the
    /// part by its content hash, and the hash of the form the entry holds
    /// beside it, where it holds one. None where no form can be handed. A
    /// store that keeps no form hands none.
    fn form(&self, _part: u64, _named: Option<u64>) -> Option<Arc<Vec<u8>>> {
        None
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

/// The stable name of a system: its identity as a maker of wants, the one a
/// world derives the system's random stream from. A name is kept for the
/// life of the process, so that a maker stays a plain value; two names are
/// ordered as their bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SystemName(&'static str);

impl SystemName {
    pub fn new(name: &str) -> SystemName {
        static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
        let mut names = NAMES.lock().expect("the names are never poisoned");
        if let Some(held) = names.get(name) {
            return SystemName(held);
        }
        let held: &'static str = Box::leak(name.to_string().into_boxed_str());
        names.insert(held);
        SystemName(held)
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// What made a want of the world's own entry: a contributor of the
/// composition, a system, the source of a command, or the harness's stand-in
/// for a scene instance. A system is known by its stable name and the three
/// others by a numeric identity: the kinds of identity have no common order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Maker {
    Contributor(u64),
    System(SystemName),
    Source(u64),
    Instance(u32),
}

/// The identity of a maker inside its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identity {
    Number(u64),
    Name(SystemName),
}

impl Maker {
    /// Where the kind stands in the total order of wants: the wants of
    /// contributors first, those of systems next, those of command sources
    /// after, and the stand-in for a scene instance last.
    fn rank(self) -> u8 {
        match self {
            Maker::Contributor(_) => 0,
            Maker::System(_) => 1,
            Maker::Source(_) => 2,
            Maker::Instance(_) => 3,
        }
    }

    /// The kind as the canonical bytes carry it. The three kinds that stood
    /// before a system was a maker keep the byte they had.
    fn kind(self) -> u8 {
        match self {
            Maker::Contributor(_) => 0,
            Maker::Source(_) => 1,
            Maker::Instance(_) => 2,
            Maker::System(_) => 3,
        }
    }

    fn identity(self) -> Identity {
        match self {
            Maker::Contributor(identity) | Maker::Source(identity) => Identity::Number(identity),
            Maker::Instance(index) => Identity::Number(u64::from(index)),
            Maker::System(name) => Identity::Name(name),
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
/// and the content hash of its canonical part with the size of what the
/// arrival puts in force, or a failure with its reason, which names no size;
/// and whether it answers a restore. The size is what the domain manifest
/// gives, the canonical part with the prepared forms built once that the
/// arrival names: the core has no manifest, and counts a world's asset data
/// from the sizes its arrivals named. Under the candidate unit of a budget
/// that rejects the closure of one want, and under it alone, an arrival also
/// names a root: the asset of the want it answers. Where the world's
/// composition declares a prepared form built once for the asset's type, an
/// arrival to a hash also names the hash of that form; the stand-in declares
/// at most one such form for a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub asset: AssetId,
    pub outcome: Outcome,
    /// The size of what an arrival to a hash puts in force, in bytes; zero
    /// for a failure.
    pub size: u64,
    pub marked: bool,
    pub root: Option<AssetId>,
    /// The hash of the prepared form built once that an arrival to a hash
    /// names beside the hash of the canonical part; none for a failure, and
    /// none where the world's composition declares no such form.
    pub form: Option<u64>,
}

impl Arrival {
    /// An arrival to a hash that names no size yet: `sized` names it.
    pub fn at(asset: AssetId, hash: u64) -> Arrival {
        Arrival { asset, outcome: Outcome::Hash(hash), size: 0, marked: false, root: None, form: None }
    }

    pub fn failed(asset: AssetId, code: u32) -> Arrival {
        Arrival { asset, outcome: Outcome::Failure(code), size: 0, marked: false, root: None, form: None }
    }

    /// Names the hash of the prepared form built once that an arrival to a
    /// hash puts in force with its part. An arrival to failed names none.
    pub fn with_form(mut self, form: u64) -> Arrival {
        if matches!(self.outcome, Outcome::Hash(_)) {
            self.form = Some(form);
        }
        self
    }

    /// Names the size of what an arrival to a hash puts in force. An arrival
    /// to failed names none.
    pub fn sized(mut self, size: u64) -> Arrival {
        if matches!(self.outcome, Outcome::Hash(_)) {
            self.size = size;
        }
        self
    }

    pub fn marked(mut self) -> Arrival {
        self.marked = true;
        self
    }

    pub fn for_root(mut self, root: AssetId) -> Arrival {
        self.root = Some(root);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Makes a want. It states the asset type of the asset it names, which
    /// the handle type of the field or of the argument it is made from gives.
    Want { asset: AssetId, asset_type: AssetType, owner: Owner, policy: Policy },
    Release { asset: AssetId, owner: Owner },
    /// Despawns an entity; the wants it owns end.
    Despawn { slot: u32, generation: u32 },
    /// Creates the stand-in for a scene instance made from an asset, which
    /// wants it under the world's own entry with an identity of its own.
    Instantiate { asset: AssetId, asset_type: AssetType, policy: Policy, reload: bool },
}

/// A world event: an input that changes a field of the world's
/// configuration at a tick boundary, so that a replay reproduces the change
/// at its tick. The one field this prototype changes so is the world's
/// budget for its asset data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldEvent {
    /// Sets the world's budget for its asset data, in bytes.
    Budget(u64),
}

/// The input record of one tick: its world events, its commands in order,
/// then its arrivals in identifier order, at most one per asset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub tick: u32,
    /// The world events of the tick's boundary, in the order issued.
    pub events: Vec<WorldEvent>,
    pub commands: Vec<Command>,
    pub arrivals: Vec<Arrival>,
}

impl Record {
    pub fn commands(tick: u32, commands: Vec<Command>) -> Record {
        Record { tick, commands, ..Record::default() }
    }

    pub fn arrivals(tick: u32, mut arrivals: Vec<Arrival>) -> Record {
        arrivals.sort_by_key(|arrival| arrival.asset);
        Record { tick, arrivals, ..Record::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.commands.is_empty() && self.arrivals.is_empty()
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
    /// Posts a batch. A later arrival of an asset replaces an earlier one;
    /// where both name a root, the one that stands first in identifier order
    /// is kept: an arrival in the closures of several wants answered between
    /// two drains names the first of the assets those wants name.
    pub fn post(&self, batch: &[Arrival]) {
        let mut arrivals = self.arrivals.lock().expect("the queue is never poisoned");
        for arrival in batch {
            let mut arrival = *arrival;
            if let (Some(root), Some(earlier)) = (arrival.root, arrivals.get(&arrival.asset).and_then(|earlier| earlier.root)) {
                arrival.root = Some(root.min(earlier));
            }
            arrivals.insert(arrival.asset, arrival);
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

/// What the one event of a restore that starts a new history names: each
/// asset whose answer differed, the asset of each want the save held
/// unanswered, and each entry the request dropped. Each list is in
/// identifier order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewHistory {
    pub differed: Vec<AssetId>,
    pub unanswered: Vec<AssetId>,
    pub dropped: Vec<AssetId>,
}

impl NewHistory {
    /// Every asset the event names, in identifier order.
    pub fn named(&self) -> Vec<AssetId> {
        let mut named: Vec<AssetId> = self.differed.iter().chain(&self.unanswered).chain(&self.dropped).copied().collect();
        named.sort_unstable();
        named.dedup();
        named
    }
}

/// What the core emits: one event per asset that became available, whose
/// hash changed, or that failed; and the one event that records a restore
/// that started a new history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Arrived { asset: AssetId, hash: u64 },
    HashChanged { asset: AssetId, from: u64, to: u64 },
    Failed { asset: AssetId, code: u32 },
    NewHistory(NewHistory),
}

/// The rules a control changes on purpose, and the readings taken where the
/// text gave two behaviors or none. The default is the design text as
/// amended, and the build's reading of it where it had to take one; every
/// field set runs another behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    // --- controls: a mechanism deliberately changed ---
    /// Control: the arrivals of a record are applied before its commands.
    pub arrivals_before_commands: bool,
    /// Control: the falls are applied before the record (the spike's order).
    pub falls_before_record: bool,
    /// Control: the gate counts a pending command that adds a want.
    pub gate_counts_added_wants: bool,
    /// Control: the gate sees a pending despawn of an owner.
    pub gate_sees_despawns: bool,
    /// Control: the gate leaves out a want the core would add from the part
    /// of a pending arrival (the reading of the gate the text does not take).
    pub gate_leaves_out_core_wants: bool,
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
    /// Control: a want that states another type than its entry holds is
    /// applied as any want is (the table as it stood before it held a type).
    pub types_not_checked: bool,
    /// Control: a restore neither confirms nor awaits an answer for an entry
    /// that holds no want (the text as it stood before its amendments).
    pub restore_leaves_entries_without_a_want: bool,
    /// Control: an arrival that fails an entry already failed for the same
    /// reason emits its event again (the text as it stood).
    pub repeated_failure_emits: bool,
    /// Control: a save that held an unanswered want continues the saved run
    /// under the content at hand where every answer is at the recorded hash
    /// (the text as it stood).
    pub save_with_an_unanswered_want_continues: bool,
    /// Control: the entries of the table are written without their asset
    /// type in the canonical bytes the state hash covers.
    pub type_left_out_of_the_bytes: bool,
    /// Control: an entry that never rose, or that failed, leaves the table
    /// at once when its last want ends.
    pub unrisen_entry_leaves_at_once: bool,
    /// Control: the wants of dependencies are added and recomputed with a
    /// batch alone, and not where a want is applied on, or released from, a
    /// parent in force.
    pub dependency_wants_with_a_batch_alone: bool,
    /// Control: a want its maker already holds, made again with another
    /// policy, counts as a want added: on a failed entry it is a new load.
    pub policy_change_counts_as_a_want_added: bool,
    /// Control: a want the core adds on a failed entry leaves it failed.
    pub core_want_leaves_a_failed_entry_failed: bool,
    /// Control: a want of another type replaces the type of an entry failed
    /// for any reason.
    pub any_failure_lets_a_type_be_replaced: bool,
    /// Control: the entry of a dependency takes the type its parent's entry
    /// holds, and not the type the parent's part lists for it.
    pub dependency_takes_its_parents_type: bool,
    /// Control: a want the core adds states the type its parent's part
    /// lists, and is refused where the entry holds another.
    pub core_wants_are_refused_for_their_type: bool,
    /// Control: the wants of the dependencies a new part no longer lists end
    /// before any arrival of the batch is applied.
    pub former_dependency_wants_end_before_the_batch: bool,
    /// Control: an arrival is applied to an entry that no want names.
    pub unnamed_arrivals_applied: bool,
    /// Control: an arrival that leaves an entry as it was, at the same hash
    /// or failed for the same reason, emits its event.
    pub unchanged_arrival_emits: bool,
    /// Control: the gate counts a want the core would add from the part of a
    /// pending arrival as required, whatever the wants of its parent.
    pub gate_counts_core_wants_as_required: bool,
    /// Control: a pending command that releases a want of a parent is left
    /// out of the policy of the want the core would add from its part.
    pub gate_leaves_out_releases_for_core_wants: bool,
    /// Control: a want of the world's own entry carries the identity of its
    /// maker without its kind, so that two makers of one identity share it.
    pub makers_known_by_identity_alone: bool,
    /// Control: what a system does in a tick is applied at the input fixed
    /// point of the next tick, before that tick's record.
    pub system_applied_at_the_next_input_fixed_point: bool,
    /// Control: what a system does in a tick is applied as commands of the
    /// record of its tick, before the arrivals and the falls.
    pub system_applied_with_its_ticks_record: bool,
    /// Control: a type the request names as dropped, of which the table
    /// holds no entry, refuses the restore.
    pub dropped_type_without_an_entry_refuses: bool,
    /// Control: a type the request names as dropped that the composition
    /// has drops nothing and refuses nothing.
    pub dropped_declared_type_ignored: bool,
    /// Control: a type is dropped whatever kept entry lists a dependency of it.
    pub type_dropped_under_a_kept_parent: bool,
    /// Control: the want a dropped parent owned is left on its dependency.
    pub dropped_parents_wants_left: bool,
    /// Control: the budget only reports, and every arrival is applied.
    pub budget_only_reports: bool,
    /// Control: under the unit the design text proposes, the count holds the
    /// entries that fall at the same fixed point, as it does under the three
    /// candidates.
    pub budget_count_holds_the_falling_entries: bool,
    /// Control: under the unit the design text proposes, the wants that a
    /// rejected part lists are added before the arrivals are applied, as
    /// those of every part of a batch are without a budget.
    pub wants_of_rejected_parts_added: bool,
    /// What an excess over a world's budget rejects: one of the three
    /// candidate units the design text names, or the unit it proposes.
    pub budget_unit: BudgetUnit,
    // --- the readings the amended text did not take ---
    /// Not taken: a want added on a failed entry leaves it failed until the
    /// arrival of the retry. The default returns it to unavailable.
    pub failed_entry_stays_failed_on_a_want: bool,
    /// Not taken: the first want decides an entry's type even when wrong, a
    /// want of another type being refused on an entry failed for its type.
    pub first_type_holds: bool,
    /// Not taken: the makers under the world's own entry are ordered by
    /// their numeric identity alone, the kind breaking a tie, a system, which
    /// has a name and no number, standing after them. The default orders
    /// contributors, then systems, then command sources, then instances,
    /// each kind by its identity.
    pub makers_by_identity_alone: bool,
    /// Not taken: a despawn of the record ends its entity's wants at the
    /// structural change fixed point of its tick, after the tick's arrivals.
    /// The default ends them where the command stands in the record.
    pub despawn_ends_wants_at_the_structural_point: bool,
    /// Not taken: an answer to a restore that fails emits no event. The
    /// default emits its event, which fails the instance made from the asset.
    pub marked_failure_emits_nothing: bool,
    /// Not taken, with the field above: such an answer still fails the
    /// instance made from the asset.
    pub marked_failure_fails_the_instance: bool,
    /// Not taken: the event that records a restore that started a new
    /// history stands after the events of its tick's record. The default
    /// places it first.
    pub restore_event_after_the_record: bool,
    // --- the readings the build took where the amended text gave two
    // behaviors or none; each field runs the other one ---
    /// An entry that holds no want takes the type of a want that states
    /// another type, and is unanswered again. The default refuses the want:
    /// the entry holds its type until it leaves the table.
    pub entry_without_a_want_takes_a_new_type: bool,
    /// A want that replaces the type of an entry failed for its type ends
    /// the wants the entry held under the former type. The default keeps
    /// them.
    pub type_replacement_ends_the_former_wants: bool,
    /// On an entry failed for its type, a want of another type from a maker
    /// that already holds a want of the asset is refused: the maker releases
    /// and wants again. The default lets it replace the type and be the new
    /// load.
    pub type_corrected_by_a_new_maker_alone: bool,
    /// Not taken, run as a control: the wants the core added for the
    /// dependencies of a parent that fails, or whose arrival is rejected,
    /// are kept until the parent's last want ends. The default ends them
    /// with the failure: a failed parent has no part in force.
    pub core_wants_kept_under_a_failed_parent: bool,
    /// Not taken, run as a control: the wants of a parent's dependencies are
    /// recomputed at the parent's own arrival, in identifier order. The
    /// default recomputes them once the batch's arrivals are applied, so
    /// that an arrival of the batch for a former dependency is applied
    /// whatever the order of the identifiers.
    pub core_wants_recomputed_at_each_arrival: bool,
    /// An answer to a restore for an entry no want names never counts as an
    /// answer that differed. The default counts it.
    pub unwanted_answer_never_differs: bool,
    /// A restore that awaits no answer records its new history at the
    /// restore, before any tick. The default records it as the first event
    /// of the first tick the restored world runs.
    pub restore_event_at_the_restore_where_nothing_is_awaited: bool,
    /// A restore into another composition that drops no entry, and in which
    /// nothing else differs, continues the saved run. The default starts a
    /// new history, with an event that names nothing.
    pub another_composition_without_a_drop_continues: bool,
    /// A want added and ended inside one record still leaves the domain. The
    /// default leaves nothing for it: it is not in the state extracted at
    /// the tick's end.
    pub a_want_ended_in_its_record_still_leaves: bool,
    /// Not taken, run as a control: a step on a gated world names what the
    /// state holds unanswered, the pending answers not being applied. The
    /// default names the dependencies that keep the gate closed, the pending
    /// record counted.
    pub step_names_what_the_state_holds_unanswered: bool,
    /// A type the request names as dropped that the composition has, and of
    /// which the table holds no entry, refuses nothing. The default refuses
    /// the restore by the type's name, whatever the table holds.
    pub dropped_declared_type_without_an_entry_refuses_nothing: bool,
    /// Under a budget, an arrival for an asset no want names counts in the
    /// group it stands in. The default leaves it out: it is not applied, so
    /// it adds nothing to what the table holds available.
    pub budget_counts_unnamed_arrivals: bool,
    /// The gate is evaluated on the pending record of a world that is not
    /// gated too: a pending arrival whose part lists a dependency the table
    /// does not hold available closes it before its record is applied. The
    /// default evaluates the gate of a gated world alone, as the sentence on
    /// a gated world reads: a world whose state holds every required asset
    /// answered steps, its record is applied, and a dependency it leaves
    /// unanswered under a required want gates from the next boundary.
    pub gate_read_on_a_world_that_is_not_gated: bool,
    // --- the readings taken on the same-harness stage: each field runs the
    // former default of the build, the reading not taken ---
    /// Not taken, kept for the comparison of costs: the gate is evaluated on
    /// a copy of the table on which the whole pending record is applied. It
    /// sees the wants that a pending element ends through the recomputation,
    /// and applies the world's budget to the pending arrivals. The default
    /// reads the pending elements against the set of the required assets the
    /// table holds unanswered, copies nothing and walks no closure.
    pub gate_copies_the_table: bool,
    /// The table keeps no set of the required assets it holds unanswered:
    /// what the gate reads is derived by one scan of the table where it is
    /// read. A world run so pays nothing where a want or an availability
    /// changes, which is what the cost the kept set moves there is measured
    /// against. The default keeps the set, brought up to date at each change.
    pub required_set_not_kept: bool,
    /// Control: a pending release answers the required asset it names
    /// whatever wants are left on it. The default applies the release to the
    /// wants of the asset, which is answered only where no required want is
    /// left.
    pub gate_takes_a_pending_release_as_an_answer: bool,
    /// Control: the gate sees a pending command that changes the policy of a
    /// want its maker already holds. The default leaves it out: it is neither
    /// a pending arrival nor a pending release.
    pub gate_sees_pending_policy_changes: bool,
    /// The other reading of the policy of a want the core would add from a
    /// pending part: the wants the table holds on the parent give it, and
    /// they alone, so that the part of a pending parent that the table does
    /// not hold under a want adds nothing. The default follows the parts of
    /// the pending arrivals downwards, to any depth, as the first step of a
    /// tick does: a want the core would add on a pending parent gives that
    /// parent's part its policy.
    pub gate_counts_core_wants_from_the_tables_wants_alone: bool,
    /// Not taken, run as a control: a want a system makes under the world's
    /// own entry is made under the contributor that brings the system, so
    /// that two systems of one contributor share one want. The default makes
    /// each system the maker of the want it makes, a kind of its own.
    pub system_wants_under_their_contributor: bool,
    /// The other reading of the order in which what several systems did in
    /// one tick is applied at its structural change fixed point: the order
    /// of their stable names. The default applies them in the order the
    /// composition lists them, the stand-in for the order in which the
    /// schedule runs them.
    pub systems_applied_in_name_order: bool,
    /// Not taken, run as a control: the groups of a budget are tested in the
    /// identifier order of their roots alone. The default tests the groups
    /// of the roots that hold a required want first, then the others, each
    /// set in identifier order.
    pub budget_groups_in_identifier_order_alone: bool,
    /// Control: the count of a budget holds an entry in force that an
    /// arrival to failed of the same record fails. The default leaves it
    /// out: its room is free for the arrivals of the same record.
    pub budget_count_holds_the_failing_entries: bool,
    /// Control: a group that does not raise the count is tested against the
    /// budget as any group is. The default accepts it whatever the count, so
    /// that a world whose count stands over its budget still takes the
    /// replacement that brings it down.
    pub budget_tests_a_group_that_does_not_raise_the_count: bool,
    /// Not taken, the stand-in the build read before: the count of a budget
    /// reads the length of each canonical part, of the part in force and of
    /// the part an arrival names. The default reads the size the entry
    /// holds, which its arrival named, and the size an arrival names.
    pub budget_counts_the_length_of_the_part: bool,
    /// Control: the size an entry in force holds is left out of the
    /// canonical bytes the state hash covers (the bytes as they stood before
    /// an entry held a size).
    pub size_left_out_of_the_bytes: bool,
    /// The other reading of an arrival at the hash in force that names
    /// another size than the entry holds: the entry takes the size named,
    /// with no event. The default leaves the entry as it was, its size
    /// included: an arrival at the same hash changes nothing.
    pub arrival_at_the_hash_in_force_brings_its_size: bool,
    /// Control: an answer to a restore is compared with the recorded hash of
    /// the canonical part alone. The default compares it with the hashes the
    /// entry holds, of its canonical part and of its prepared form built
    /// once: an answer at the recorded hash of the part that names another
    /// form differs, the entry takes that form, and the restore starts a new
    /// history.
    pub answer_compared_with_the_part_alone: bool,
    /// Control: the confirmation of a restore under the recorded content
    /// reads the hash of the canonical part alone. The default confirms each
    /// available entry at the hashes it holds, of its canonical part and of
    /// its prepared form built once, and refuses the restore where the form
    /// cannot be confirmed, naming the asset and the recorded hash of the
    /// form.
    pub confirmation_reads_the_part_alone: bool,
    /// Not taken, run as a control: on an entry failed for the reason of
    /// another type, a want the core adds leaves the entry's type as it is:
    /// the load starts again under the type that failed. The default has the
    /// core's want take the type its parent's part lists and replace the
    /// entry's type.
    pub core_want_keeps_the_type_of_a_failed_entry: bool,
    /// The build as it stood: the state leaves the domain at the input fixed
    /// point of a tick whose record changed the table, and again where a
    /// system acted, so that the wants a despawn ends at the structural
    /// change fixed point reach the availability source with the next change
    /// alone, a view older than one tick. The default extracts the state
    /// once, at the end of every tick in which the table changed.
    pub extracted_at_the_input_fixed_point: bool,
    // --- the controls of the scenarios of the stand-in stage ---
    /// Control: a required want that the core added for a dependency gates
    /// no boundary: the required set holds the assets that a want of an
    /// entity or of the world's own entry requires, and no other.
    pub core_wants_gate_no_boundary: bool,
    /// Control: at the first step of a record's arrivals the part of every
    /// arrival to a hash adds its wants, whatever wants its asset holds, a
    /// parent that holds none giving an optional want. The default starts
    /// from the part of an asset that holds a want at that step.
    pub first_step_follows_every_arriving_part: bool,
    /// Control: the wants of the first step start from an asset that held a
    /// want before the commands of the record were applied. The default
    /// reads the wants once those commands are applied.
    pub first_step_starts_from_the_wants_held_before_the_commands: bool,
    /// Control: a pending release answers a required asset only where it
    /// ends the last want of that asset, whatever its policy. The default
    /// counts the asset as answered where no required want is left on it.
    pub gate_release_answers_where_no_want_is_left: bool,
    /// Control: under a budget, a root holds a required want where it stood
    /// in the world's required set before the commands of the record were
    /// applied. The default reads the set once those commands are applied.
    pub budget_required_roots_read_before_the_commands: bool,
    /// Control: under a budget, a root holds a required want where a maker
    /// other than the core made one. The default reads the world's required
    /// set, whichever maker made the want, the core included.
    pub budget_required_roots_by_their_own_wants: bool,
    /// Control: a want the core added keeps its policy until the arrivals of
    /// the record are applied, whatever a command of that record does to the
    /// wants of its parent. The default recomputes it where the command is
    /// applied.
    pub core_wants_keep_their_policy_until_the_arrivals: bool,
    /// Control: the groups of each of the two sets of a budget's test are
    /// tested in the order in which the wants of their roots were made. The
    /// default tests each set in the identifier order of its roots.
    pub budget_groups_in_the_order_their_wants_were_made: bool,
    /// Control: the room of an entry in force is free wherever the record
    /// holds an arrival to failed for it, whether or not that arrival fails
    /// it. The default leaves out of the count an entry in force that holds
    /// a want once the commands are applied, and no other.
    pub budget_count_leaves_out_every_entry_an_arrival_to_failed_names: bool,
    /// Control: the room of the dependencies of a parent that fails is free
    /// at the fixed point that fails it. The default counts them: the wants
    /// the core added end at the recomputation, after the arrivals.
    pub budget_count_leaves_out_the_dependencies_of_a_failing_parent: bool,
    /// Control: a group is accepted whatever the count only where it lowers
    /// the count. The default accepts one that does not raise it.
    pub budget_accepts_over_the_count_only_a_group_that_lowers_it: bool,
    /// Control: each arrival of a group is judged alone, in identifier
    /// order, one that raises the count being rejected where the count would
    /// stand over the budget. The default accepts or rejects a group whole.
    pub budget_judges_each_arrival_of_a_group_alone: bool,
    /// Control: a budget that a record changes is in force from the tick
    /// after it. The default has the world event change the configuration at
    /// the boundary of its tick, so that the arrivals of the record that
    /// holds it are tested against the budget it sets.
    pub budget_event_in_force_from_the_next_tick: bool,
    /// Control: a world event that sets a budget is applied and is no
    /// element of the input log, so that a replay stands under the budget of
    /// its creation. The default records it in the record of its tick.
    pub budget_events_left_out_of_the_log: bool,
    /// Control, for a world that replays a log: the core derives no
    /// rejection, and every arrival to a hash is applied whatever the count.
    pub no_rejection_derived: bool,
    /// Control: the count of a restored table starts from nothing and holds
    /// the sizes of the arrivals applied since the restore alone. The
    /// default counts the size every available entry holds, a restored one
    /// included.
    pub restored_count_starts_from_nothing: bool,
    /// Control: a save holds the budget its world was created under, so that
    /// a restored world stands under it. The default holds the budget in
    /// force at the save, a world event that lowered it counted.
    pub save_holds_the_budget_of_creation: bool,
    /// Control: an answer to a restore at another hash changes the hash of
    /// its entry and leaves the size the entry held. The default has the
    /// entry take the size the answer names.
    pub marked_answer_leaves_the_size: bool,
    /// Control: the wants of systems stand after those of command sources
    /// in the total order of an entry's wants. The default places them
    /// after those of contributors and before those of command sources.
    pub system_wants_after_command_sources: bool,
    /// Control: the wants of systems stand in the order of the identities
    /// of the contributors that bring them, then by name. The default
    /// orders them by their stable names alone.
    pub system_wants_ordered_by_their_contributor: bool,
    /// Control: an entry that fails for its type while a want of the core
    /// stands on it takes, at that failure, the type its parent's part
    /// lists, and its load starts again. The default leaves it failed under
    /// the type it holds: the design text does not reach the case.
    pub entry_failed_for_its_type_under_a_core_want_takes_the_type: bool,
    /// Control: a second want on a parent in force is a want added on each
    /// of its dependencies, which retries those the table holds failed. The
    /// default retries nothing: a dependency holds one want per parent.
    pub second_want_on_a_parent_retries_its_dependencies: bool,
    /// Control: an arrival that leaves an entry failed for the reason it
    /// held when its record was drained emits no event and is named by no
    /// step, whatever want the same record added on the entry. The default
    /// judges an arrival against the entry as it stands when it is applied:
    /// an entry a want of the record returned to unavailable fails anew.
    pub failure_for_the_reason_held_at_the_drain_changes_nothing: bool,
    /// Control: the arrival of an entry that held no want when its record
    /// was drained changes nothing, a want the core adds from a part of the
    /// same record not counted. The default applies it where a want applied
    /// before it names the asset again, the core's of the first step
    /// included.
    pub arrival_for_an_entry_unwanted_at_the_drain_changes_nothing: bool,
    /// Control: an entry leaves the table at the fixed point that ends its
    /// last want. The default keeps it, with no want, until the input fixed
    /// point of the next tick.
    pub entry_leaves_at_the_fixed_point_that_ends_its_last_want: bool,
    // --- a variant of the build for one measurement ---
    /// The host does not ask the gate: the world steps at every boundary,
    /// and the count of the entries a restore awaits is not kept. With the
    /// rule that keeps no set, it is the variant of the build that keeps
    /// neither the set nor the count, against which the cost the incremental
    /// form moves to every change of a want or of an availability is
    /// measured. It stands only on a world whose state holds every required
    /// asset answered at every boundary and that no restore made: it is a
    /// measurement's, never a reading of the design text.
    pub gate_not_asked: bool,
}

/// What an excess over a world's budget rejects. The design text named
/// three candidates, each of which showed a defect, and takes a fourth
/// unit, the group of a root, which is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BudgetUnit {
    /// Each arrival to a hash is tested alone, in identifier order.
    SingleArrival,
    /// The arrivals that answer one want, which name one root, are accepted
    /// together or rejected together.
    ClosureOfAWant,
    /// The arrivals to a hash of one record are one group.
    WholeDrain,
    /// The unit the design text takes: the group of a root, the arrivals of
    /// a record that stand in the closure of one root, its bounds derived by
    /// the core from the parts in force and the parts of the batch. No
    /// arrival names a root; every group is decided before any want of the
    /// batch's parts is added, and the wants a rejected part lists are not
    /// added; the count leaves out the entries that fall or fail at the same
    /// fixed point whatever the test gives.
    #[default]
    DerivedClosure,
}

impl Rules {
    fn owner_order(&self, a: &Owner, b: &Owner, brought_by: &BTreeMap<SystemName, u64>) -> Ordering {
        let rank = |owner: &Owner| match owner {
            Owner::World(_) => 0,
            Owner::Entity { .. } => 1,
            Owner::Asset(_) => 2,
        };
        match (a, b) {
            (Owner::World(a), Owner::World(b)) => {
                // Two controls on the place of a system's want. Under one
                // the wants of systems stand after those of command sources.
                // Under the other, two systems are ordered by the identity
                // of the contributor that brings each, then by name.
                let rank = |maker: &Maker| match maker {
                    Maker::System(_) if self.system_wants_after_command_sources => 2 * Maker::Source(0).rank() + 1,
                    other => 2 * other.rank(),
                };
                if let (Maker::System(one), Maker::System(other), true) = (a, b, self.system_wants_ordered_by_their_contributor) {
                    let contributor = |name: &SystemName| brought_by.get(name).copied().unwrap_or(u64::MAX);
                    return (contributor(one), one).cmp(&(contributor(other), other));
                }
                // Under the reading that orders the makers by their numeric
                // identity, a system, which has none, stands after them.
                if self.makers_by_identity_alone {
                    (a.identity(), rank(a)).cmp(&(b.identity(), rank(b)))
                } else {
                    (rank(a), a.identity()).cmp(&(rank(b), b.identity()))
                }
            }
            (Owner::Entity { slot: a, generation: ga }, Owner::Entity { slot: b, generation: gb }) => (a, ga).cmp(&(b, gb)),
            (Owner::Asset(a), Owner::Asset(b)) => a.cmp(b),
            _ => rank(a).cmp(&rank(b)),
        }
    }

    /// The owner a want is held under: under the control that shares one
    /// want under the world's own entry, whichever maker already holds it;
    /// under the control that knows a maker by its identity alone, the maker
    /// of that identity that already holds it.
    fn owner(&self, wants: &[(Owner, Policy)], owner: Owner) -> Owner {
        match owner {
            Owner::World(_) if self.one_want_under_the_world => {
                wants.iter().map(|(held, _)| *held).find(|held| matches!(held, Owner::World(_))).unwrap_or(owner)
            }
            Owner::World(maker) if self.makers_known_by_identity_alone => {
                let same = |held: &Owner| matches!(held, Owner::World(other) if other.identity() == maker.identity());
                wants.iter().map(|(held, _)| *held).find(same).unwrap_or(owner)
            }
            other => other,
        }
    }

    /// Where a want of `owner` stands among the wants of an entry.
    fn place(&self, wants: &[(Owner, Policy)], owner: &Owner, brought_by: &BTreeMap<SystemName, u64>) -> usize {
        if self.wants_in_making_order {
            return wants.len();
        }
        wants.iter().position(|(other, _)| self.owner_order(other, owner, brought_by) == Ordering::Greater).unwrap_or(wants.len())
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

    /// A string: a length, then UTF-8 bytes.
    pub fn text(&mut self) -> Result<&'a str, String> {
        let len = self.u32()? as usize;
        let at = self.at;
        std::str::from_utf8(self.take(len)?).map_err(|_| format!("a string that is not UTF-8 at byte {at}"))
    }

    /// An asset type, as the stable name of its schema entry.
    pub fn asset_type(&mut self) -> Result<AssetType, String> {
        let name = self.text()?;
        AssetType::named(name).ok_or_else(|| format!("the schema holds no asset type named {name}"))
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

/// An owner in the canonical form. A want of the world's own entry carries
/// the kind of its maker, then its identity: a number, or for a system its
/// stable name, a length then UTF-8 bytes.
fn put_owner(out: &mut Vec<u8>, owner: Owner) {
    match owner {
        Owner::World(maker) => {
            out.push(0);
            out.push(maker.kind());
            match maker.identity() {
                Identity::Number(identity) => out.extend_from_slice(&identity.to_le_bytes()),
                Identity::Name(name) => {
                    out.extend_from_slice(&(name.as_str().len() as u32).to_le_bytes());
                    out.extend_from_slice(name.as_str().as_bytes());
                }
            }
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
        0 => Ok(Owner::World(match reader.u8()? {
            0 => Maker::Contributor(reader.u64()?),
            1 => Maker::Source(reader.u64()?),
            2 => Maker::Instance(reader.u64()? as u32),
            3 => Maker::System(SystemName::new(reader.text()?)),
            other => return Err(format!("maker kind {other}")),
        })),
        1 => Ok(Owner::Entity { slot: reader.u32()?, generation: reader.u32()? }),
        2 => Ok(Owner::Asset(reader.id()?)),
        other => Err(format!("owner tag {other}")),
    }
}

fn put_record(out: &mut Vec<u8>, record: &Record, sized: bool) {
    out.extend_from_slice(&record.tick.to_le_bytes());
    out.extend_from_slice(&(record.events.len() as u32).to_le_bytes());
    for event in &record.events {
        match *event {
            WorldEvent::Budget(bytes) => {
                out.push(0);
                out.extend_from_slice(&bytes.to_le_bytes());
            }
        }
    }
    out.extend_from_slice(&(record.commands.len() as u32).to_le_bytes());
    for command in &record.commands {
        match *command {
            Command::Want { asset, asset_type, owner, policy } => {
                out.push(0);
                out.extend_from_slice(&asset.to_le_bytes());
                put_type(out, asset_type);
                put_owner(out, owner);
                put_policy(out, policy);
            }
            Command::Release { asset, owner } => {
                out.push(1);
                out.extend_from_slice(&asset.to_le_bytes());
                put_owner(out, owner);
            }
            Command::Instantiate { asset, asset_type, policy, reload } => {
                out.push(2);
                out.extend_from_slice(&asset.to_le_bytes());
                put_type(out, asset_type);
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
            // An arrival to a hash names the size of what it puts in force;
            // a failure names none. One control leaves the size out.
            Outcome::Hash(hash) => {
                out.push(0);
                out.extend_from_slice(&hash.to_le_bytes());
                if sized {
                    out.extend_from_slice(&arrival.size.to_le_bytes());
                }
            }
            Outcome::Failure(code) => {
                out.push(1);
                out.extend_from_slice(&u64::from(code).to_le_bytes());
            }
        }
        // One byte of marks: an answer to a restore, the root an arrival
        // names under the candidate of a budget that writes it, and the
        // prepared form built once it names. An arrival that names neither
        // a root nor a form has the bytes it had before those marks existed.
        out.push(u8::from(arrival.marked) | (u8::from(arrival.root.is_some()) << 1) | (u8::from(arrival.form.is_some()) << 2));
        if let Some(root) = arrival.root {
            out.extend_from_slice(&root.to_le_bytes());
        }
        if let Some(form) = arrival.form {
            out.extend_from_slice(&form.to_le_bytes());
        }
    }
}

fn read_record(reader: &mut Reader, sized: bool) -> Result<Record, String> {
    let tick = reader.u32()?;
    let mut events = Vec::new();
    for _ in 0..reader.u32()? {
        events.push(match reader.u8()? {
            0 => WorldEvent::Budget(reader.u64()?),
            other => return Err(format!("world event tag {other}")),
        });
    }
    let mut commands = Vec::new();
    for _ in 0..reader.u32()? {
        commands.push(match reader.u8()? {
            0 => Command::Want { asset: reader.id()?, asset_type: reader.asset_type()?, owner: read_owner(reader)?, policy: read_policy(reader)? },
            1 => Command::Release { asset: reader.id()?, owner: read_owner(reader)? },
            2 => Command::Instantiate { asset: reader.id()?, asset_type: reader.asset_type()?, policy: read_policy(reader)?, reload: reader.flag()? },
            3 => Command::Despawn { slot: reader.u32()?, generation: reader.u32()? },
            other => return Err(format!("command tag {other}")),
        });
    }
    let mut arrivals = Vec::new();
    for _ in 0..reader.u32()? {
        let asset = reader.id()?;
        let (outcome, size) = match reader.u8()? {
            0 => (Outcome::Hash(reader.u64()?), if sized { reader.u64()? } else { 0 }),
            1 => {
                // A failure names a reason of the list its schema entry holds.
                let code = reader.u64()?;
                let listed = u32::try_from(code).ok().filter(|code| reason(*code).is_some());
                (Outcome::Failure(listed.ok_or_else(|| format!("a failure names the reason {code}, which the list does not hold"))?), 0)
            }
            other => return Err(format!("outcome tag {other}")),
        };
        let marks = reader.u8()?;
        if marks > 7 {
            return Err(format!("arrival marks {marks}"));
        }
        let root = if marks & 2 != 0 { Some(reader.id()?) } else { None };
        let form = if marks & 4 != 0 { Some(reader.u64()?) } else { None };
        if form.is_some() && !matches!(outcome, Outcome::Hash(_)) {
            return Err("an arrival to failed names a prepared form".to_string());
        }
        arrivals.push(Arrival { asset, outcome, size, marked: marks & 1 != 0, root, form });
    }
    Ok(Record { tick, events, commands, arrivals })
}

/// The canonical bytes of an input log: the record count, then the records.
pub fn encode_log(records: &[Record]) -> Vec<u8> {
    encode_log_under(records, &Rules::default())
}

pub fn decode_log(bytes: &[u8]) -> Result<Vec<Record>, String> {
    decode_log_under(bytes, &Rules::default())
}

/// The bytes of an input log under the rules of the world that wrote it:
/// the control that leaves the size out of the canonical bytes leaves it out
/// of the bytes of an arrival, so that a log read back names no size.
pub fn encode_log_under(records: &[Record], rules: &Rules) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for record in records {
        put_record(&mut out, record, !rules.size_left_out_of_the_bytes);
    }
    out
}

pub fn decode_log_under(bytes: &[u8], rules: &Rules) -> Result<Vec<Record>, String> {
    let mut reader = Reader::new(bytes);
    let mut records = Vec::new();
    for _ in 0..reader.u32()? {
        records.push(read_record(&mut reader, !rules.size_left_out_of_the_bytes)?);
    }
    reader.end()?;
    Ok(records)
}

// ---------------------------------------------------------------------------
// The asset table
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Entry {
    /// The asset type the entry holds: the one its first want stated.
    pub asset_type: AssetType,
    /// The wants of the asset, in their total order.
    pub wants: Vec<(Owner, Policy)>,
    pub availability: Availability,
    /// The size the entry holds beside its hash while the asset is
    /// available: the one the arrival that put it in force named. Zero
    /// otherwise.
    pub size: u64,
    /// The reference to the resident part a world holds while the asset is available.
    pub resident: Option<Arc<Resident>>,
    /// The hash of the prepared form built once that the entry holds beside
    /// the hash of its canonical part while the asset is available: the one
    /// the arrival that put it in force named. None otherwise, and none
    /// where the world's composition declares no such form for the type.
    pub form: Option<u64>,
}

impl Entry {
    fn unavailable(asset_type: AssetType) -> Entry {
        Entry { asset_type, wants: Vec::new(), availability: Availability::Unavailable, size: 0, resident: None, form: None }
    }

    /// The policy the asset is under: required if any want is.
    pub fn policy(&self) -> Option<Policy> {
        self.wants.iter().map(|(_, policy)| *policy).max()
    }

    /// Whether the entry closes the gate: under a required want and
    /// unanswered, a failure being an answer. Under one control a want the
    /// core added is no such want.
    fn required_and_unanswered(&self, rules: &Rules) -> bool {
        if self.availability != Availability::Unavailable {
            return false;
        }
        if rules.core_wants_gate_no_boundary {
            return self.wants.iter().any(|(owner, policy)| *policy == Policy::Required && !matches!(owner, Owner::Asset(_)));
        }
        self.policy() == Some(Policy::Required)
    }

    /// The test of a want that leaves the domain, made when the want is
    /// applied: the asset it names is unanswered, or failed, in which case
    /// the want is the new load and the entry returns to unavailable.
    fn unanswered_for_a_new_want(&mut self, rules: &Rules) -> bool {
        match self.availability {
            Availability::Unavailable => true,
            Availability::Failed(_) => {
                if !rules.failed_entry_stays_failed_on_a_want {
                    self.availability = Availability::Unavailable;
                }
                true
            }
            Availability::Available(_) => false,
        }
    }
}

/// An entry of the table as the availability source reads it, in the state
/// extracted at a tick's end or by a query at a boundary where no tick has
/// ended: the asset, the type its entry holds, its availability and whether
/// a want names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryView {
    pub asset: AssetId,
    pub asset_type: AssetType,
    pub availability: Availability,
    pub wanted: bool,
}

/// What became of a want a command, a system or the composition makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wanted {
    /// It states another type than its entry holds: a recorded refusal that
    /// changes nothing.
    Refused,
    /// Its maker already held a want of the asset: the policy alone changed.
    /// No want was added, and nothing leaves the domain.
    PolicyOnly,
    /// The want was added, or is the new load of an entry failed for its
    /// type. It leaves the domain where the asset it names was unanswered or
    /// failed when it was applied.
    Applied { leaves: bool },
}

/// Why a command of a record was refused. A refusal is recorded and changes
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The entity that would own the want, or that a despawn names, is not alive.
    OwnerNotAlive,
    /// The want states another asset type than its entry holds.
    AnotherType,
}

impl Refused {
    pub const ALL: [Refused; 2] = [Refused::OwnerNotAlive, Refused::AnotherType];

    /// The short name of the reason.
    pub fn name(self) -> &'static str {
        match self {
            Refused::OwnerNotAlive => "owner-not-alive",
            Refused::AnotherType => "other-type",
        }
    }
}

/// What applying one input record did.
#[derive(Default)]
pub struct Applied {
    pub events: Vec<Event>,
    /// Arrivals for an asset no want names: recorded, no change.
    pub unnamed_arrivals: u32,
    /// The commands refused, each by its place in the record, with the
    /// reason: recorded, no change.
    pub refusals: Vec<(usize, Refused)>,
    pub fallen: Vec<AssetId>,
    /// The entities the record despawns, for the world to take out.
    pub despawned: Vec<(u32, u32)>,
    /// The assets a command of the record made a want of, a want that only
    /// changed its policy included, in order, each with the type stated.
    pub wanted: Vec<(AssetId, AssetType)>,
    /// The wants that leave the domain: each asset a want was added on while
    /// it was unanswered or failed, with the type its entry holds. A want
    /// the core added with a batch is among them where no arrival of that
    /// batch answers it.
    pub left: Vec<(AssetId, AssetType)>,
    /// The assets that failed under a required want, with the reason.
    pub failed_required: Vec<(AssetId, u32)>,
    /// The answers to a restore that differed from what the table held.
    pub differed: Vec<AssetId>,
    /// Whether the record held an answer to a restore.
    pub answered_a_restore: bool,
    /// The arrivals rejected over the world's budget, in the order applied:
    /// the asset and the hash the arrival named.
    pub rejected: Vec<(AssetId, u64)>,
}

impl Applied {
    pub fn refused_commands(&self) -> u32 {
        self.refusals.len() as u32
    }
}

/// What a record is applied for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Tick,
    Gate,
}

/// The wants added while a record is applied on an asset that was unanswered
/// or failed: the asset and the owner of the want.
type Leaving = Vec<(AssetId, Owner)>;

/// Where the wants of the first step of a record's arrivals start from,
/// under the two controls on it. The default starts from the part of every
/// asset that holds a want at that step.
#[derive(Default)]
struct Starts {
    /// The assets whose own wants are not read: under one control, those
    /// that held no want before the commands of the record were applied.
    not_from: BTreeSet<AssetId>,
    /// Under the other control: the part of every arrival adds its wants.
    from_every_arrival: bool,
}

/// What a record found when it was drained, kept while it is applied for
/// the controls that read the table as it stood before the record's
/// commands. Each part is filled under the control that reads it alone.
#[derive(Clone, Default)]
struct Found {
    /// The assets that held a want.
    wanted: BTreeSet<AssetId>,
    /// The assets that stood under a required want.
    required: BTreeSet<AssetId>,
    /// The entries held failed, each with its reason.
    failed: BTreeMap<AssetId, u32>,
    /// The entries that held no want and that no command of the record
    /// names: filled once the commands are applied.
    unnamed: BTreeSet<AssetId>,
}

#[derive(Clone, Default)]
pub struct Table {
    pub entries: BTreeMap<AssetId, Entry>,
    pub instances: Vec<Instance>,
    /// A restore under the content at hand: the entries whose answer the
    /// world still awaits. Its length is the count the gate of a restored
    /// world reads. Derived, and no part of the canonical form.
    pub awaiting: BTreeSet<AssetId>,
    pub rules: Rules,
    /// An entry has no want left: it falls at the next input fixed point.
    orphans: bool,
    /// The required assets the table holds unanswered, brought up to date
    /// where a want or an availability changes. Derived from the entries,
    /// and no part of the canonical form: the gate reads the pending
    /// elements against it. Empty under the rule that keeps no set.
    required: BTreeSet<AssetId>,
    /// How many times the table followed a change of a want or of an
    /// availability: each is a place where the incremental form of the gate
    /// brings its set up to date. Counted whatever the rules, and no part of
    /// the canonical form: a measurement divides a cost by it.
    changes: u64,
    entity_wants: usize,
    /// The wants the core added on an asset that was unanswered or failed,
    /// since the list was last taken: the dependency and its parent.
    core_added: Vec<(AssetId, AssetId)>,
    /// The world's budget for its asset data, in bytes, where it has one:
    /// configuration of the world, and no state.
    pub budget: Option<u64>,
    /// The table is a copy the gate is evaluated on.
    evaluating_the_gate: bool,
    /// On that copy: the parents whose part a pending arrival brings.
    pending_parents: BTreeSet<AssetId>,
    /// While a record is applied: what it found when it was drained.
    found: Found,
    /// The order in which the assets a maker other than the core wants were
    /// first wanted: read by one control of a budget's test, derived, and no
    /// part of the canonical form.
    made: BTreeMap<AssetId, u64>,
    /// Under the control that starts the count of a restored table from
    /// nothing: the entries restored in force to which no arrival has been
    /// applied since, which count for nothing.
    uncounted: BTreeSet<AssetId>,
    /// The contributor of the composition that brings each system: the
    /// world's configuration, read by one control on the place of a
    /// system's want, and no part of the canonical form.
    pub systems: BTreeMap<SystemName, u64>,
}

impl Table {
    pub fn with_rules(rules: Rules) -> Table {
        Table { rules, ..Table::default() }
    }

    /// Whether the state gates the world: a required asset is unanswered, or
    /// an answer to a restore is awaited.
    pub fn blocked(&self) -> bool {
        if self.rules.gate_not_asked {
            return false;
        }
        if !self.awaiting.is_empty() {
            return true;
        }
        if self.rules.required_set_not_kept { self.entries.values().any(|entry| entry.required_and_unanswered(&self.rules)) } else { !self.required.is_empty() }
    }

    pub fn has_orphans(&self) -> bool {
        self.orphans
    }

    /// The required assets the table holds unanswered, by one scan of it:
    /// what the kept set is derived from, and what stands in its place
    /// under the rule that keeps none.
    fn scanned_required(&self) -> BTreeSet<AssetId> {
        self.entries.iter().filter(|(_, entry)| entry.required_and_unanswered(&self.rules)).map(|(id, _)| *id).collect()
    }

    /// The set of the required assets the table holds unanswered: the kept
    /// one, or under the rule that keeps none the one a scan gives.
    fn required_set(&self) -> std::borrow::Cow<'_, BTreeSet<AssetId>> {
        if self.rules.required_set_not_kept { std::borrow::Cow::Owned(self.scanned_required()) } else { std::borrow::Cow::Borrowed(&self.required) }
    }

    /// The entry of an asset changed its wants or its availability, or left
    /// the table: the kept set follows. This is the cost the incremental
    /// form of the gate moves to every such change.
    fn follow(&mut self, asset: AssetId) {
        self.changes += 1;
        if self.rules.required_set_not_kept {
            return;
        }
        if self.entries.get(&asset).is_some_and(|entry| entry.required_and_unanswered(&self.rules)) {
            self.required.insert(asset);
        } else {
            self.required.remove(&asset);
        }
    }

    /// How many times the table followed a change of a want or of an
    /// availability since it was made: a want applied, released or ended, a
    /// want the core added, changed or ended, an arrival applied, a
    /// rejection that fails an entry, a fall.
    pub fn changes_followed(&self) -> u64 {
        self.changes
    }

    /// For the measurement of what the incremental form costs at one
    /// change: brings the kept set up to date for each asset given, as a
    /// change of its wants or of its availability does, and returns how long
    /// that took. The set follows the entries, so nothing changes, and the
    /// count of the changes followed is left as it was. Under the rule that
    /// keeps no set, nothing is brought up to date.
    pub fn follow_again(&mut self, assets: &[AssetId]) -> Duration {
        let counted = self.changes;
        let started = Instant::now();
        for asset in assets {
            self.follow(*asset);
        }
        let elapsed = started.elapsed();
        self.changes = counted;
        elapsed
    }

    /// An entry a restore awaited has its answer, or left the table: the
    /// count of the awaited entries follows. Nothing is kept under the
    /// variant of the build whose gate the host does not ask.
    fn no_longer_awaited(&mut self, asset: &AssetId) {
        if !self.rules.gate_not_asked {
            self.awaiting.remove(asset);
        }
    }

    /// The assets that close the gate on the state alone: under a required
    /// want and unanswered (a failure is an answer), or awaited by a restore
    /// under the content at hand.
    pub fn required_unanswered(&self) -> Vec<AssetId> {
        let mut set: BTreeSet<AssetId> = self.awaiting.clone();
        set.extend(self.required_set().iter().copied());
        set.into_iter().collect()
    }

    /// The query of the table at a boundary where no tick has ended: the
    /// wants whose asset is unanswered, each with the type its entry holds.
    pub fn unanswered(&self) -> Vec<(AssetId, AssetType)> {
        let wanted = |entry: &&Entry| !entry.wants.is_empty() && entry.availability == Availability::Unavailable;
        self.entries.iter().filter(|(_, entry)| wanted(entry)).map(|(id, entry)| (*id, entry.asset_type)).collect()
    }

    /// The same query, for a restore under the content at hand: for each
    /// entry the table does not hold available, the wants that stand on it
    /// with their owners. Without them an availability source could not tell
    /// an entry on which the part of an answer adds a want from one that its
    /// parent's own want already holds.
    pub fn standing_wants(&self) -> Vec<(AssetId, Vec<(Owner, Policy)>)> {
        let not_available = |entry: &&Entry| !matches!(entry.availability, Availability::Available(_));
        self.entries.iter().filter(|(_, entry)| not_available(entry)).map(|(id, entry)| (*id, entry.wants.clone())).collect()
    }

    /// The table as the availability source reads it.
    pub fn view(&self) -> Vec<EntryView> {
        let view = |(id, entry): (&AssetId, &Entry)| EntryView {
            asset: *id,
            asset_type: entry.asset_type,
            availability: entry.availability,
            wanted: !entry.wants.is_empty(),
        };
        self.entries.iter().map(view).collect()
    }

    /// Whether `owner` holds a want of `asset`.
    fn holds(&self, asset: AssetId, owner: Owner) -> bool {
        self.entries.get(&asset).is_some_and(|entry| {
            let owner = self.rules.owner(&entry.wants, owner);
            entry.wants.iter().any(|(other, _)| *other == owner)
        })
    }

    /// Applies the want of an owner on an asset. The first want of an asset
    /// writes the type it states into the asset's entry; a later want that
    /// states another type is refused, but on an entry failed for the reason
    /// that the manifest holds the asset under another type, where it
    /// replaces the entry's type and is the new load. An owner, or a maker
    /// under the world's own entry, holds at most one want of an asset: a
    /// second one from the same changes its policy and adds no want. A want
    /// added on a failed entry returns it to unavailable.
    pub fn want(&mut self, asset: AssetId, asset_type: AssetType, owner: Owner, policy: Policy) -> Wanted {
        let wanted = self.want_on_the_entry(asset, asset_type, owner, policy);
        self.follow(asset);
        wanted
    }

    fn want_on_the_entry(&mut self, asset: AssetId, asset_type: AssetType, owner: Owner, policy: Policy) -> Wanted {
        let rules = self.rules;
        let mut new_load = false;
        if let Some(entry) = self.entries.get_mut(&asset)
            && entry.asset_type != asset_type
            && !rules.types_not_checked
        {
            let held_under = rules.owner(&entry.wants, owner);
            let held = entry.wants.iter().any(|(other, _)| *other == held_under);
            let failed_for_its_type = (entry.availability == Availability::Failed(REASON_ANOTHER_TYPE)
                || (rules.any_failure_lets_a_type_be_replaced && matches!(entry.availability, Availability::Failed(_))))
                && !rules.first_type_holds
                && !(held && rules.type_corrected_by_a_new_maker_alone);
            let unwanted = entry.wants.is_empty() && rules.entry_without_a_want_takes_a_new_type;
            if !failed_for_its_type && !unwanted {
                return Wanted::Refused;
            }
            entry.asset_type = asset_type;
            if !failed_for_its_type {
                entry.availability = Availability::Unavailable;
                entry.size = 0;
                entry.resident = None;
                entry.form = None;
            }
            if rules.type_replacement_ends_the_former_wants {
                entry.wants.retain(|(other, _)| *other == held_under);
            }
            new_load = failed_for_its_type;
        }
        let entry = self.entries.entry(asset).or_insert_with(|| Entry::unavailable(asset_type));
        let owner = rules.owner(&entry.wants, owner);
        let held = if rules.second_want_added { None } else { entry.wants.iter().position(|(other, _)| *other == owner) };
        match held {
            Some(at) => {
                let changed = entry.wants[at].1 != policy;
                entry.wants[at].1 = policy;
                if !new_load && !(changed && rules.policy_change_counts_as_a_want_added) {
                    return Wanted::PolicyOnly;
                }
            }
            None => {
                // The order of making, for the control that reads it: the
                // place of the first want a maker other than the core makes
                // of the asset, while one stands.
                if !entry.wants.iter().any(|(other, _)| !matches!(other, Owner::Asset(_))) {
                    let next = self.made.values().max().map_or(0, |last| last + 1);
                    self.made.insert(asset, next);
                }
                let at = rules.place(&entry.wants, &owner, &self.systems);
                entry.wants.insert(at, (owner, policy));
            }
        }
        Wanted::Applied { leaves: entry.unanswered_for_a_new_want(&rules) }
    }

    /// Ends the want of an owner on an asset. Returns whether one ended.
    pub fn release(&mut self, asset: AssetId, owner: Owner) -> bool {
        let rules = self.rules;
        let Some(entry) = self.entries.get_mut(&asset) else { return false };
        let owner = rules.owner(&entry.wants, owner);
        let Some(at) = entry.wants.iter().position(|(other, _)| *other == owner) else { return false };
        entry.wants.remove(at);
        self.follow(asset);
        true
    }

    /// Ends, on every entry, the wants an owner test names. Returns whether
    /// any ended.
    fn end_wants_where(&mut self, ends: impl Fn(&Owner) -> bool) -> bool {
        let mut changed = Vec::new();
        for (id, entry) in self.entries.iter_mut() {
            let before = entry.wants.len();
            entry.wants.retain(|(owner, _)| !ends(owner));
            if entry.wants.len() != before {
                changed.push(*id);
            }
        }
        for id in &changed {
            self.follow(*id);
        }
        !changed.is_empty()
    }

    /// Ends the wants an entity owned, when it is despawned. With a
    /// generation, the wants of that entity alone; without, of whatever
    /// entity holds the slot. Returns whether any ended.
    pub fn end_wants_of_entity(&mut self, slot: u32, generation: Option<u32>) -> bool {
        if self.entity_wants == 0 {
            return false;
        }
        let ended = self.end_wants_where(|owner| match owner {
            Owner::Entity { slot: other, generation: held } => *other == slot && generation.is_none_or(|generation| generation == *held),
            _ => false,
        });
        if ended {
            if !self.rules.dependency_wants_with_a_batch_alone {
                self.dependency_wants(&BTreeMap::new(), true, &Starts::default());
            }
            self.core_added.clear();
            self.leave_at_once();
            if self.rules.unrisen_entry_leaves_at_once {
                self.entries.retain(|_, entry| !entry.wants.is_empty() || matches!(entry.availability, Availability::Available(_)));
            }
            self.recount();
        }
        ended
    }

    /// The wants the core holds for dependencies, brought in step with the
    /// wants of their parents: a dependency holds one want per parent whose
    /// part lists it, required if any want of the parent is, which ends with
    /// the parent's last want and follows the parent's part.
    ///
    /// The part of a parent is the one `arriving` brings for it, where a
    /// batch about to be applied brings one, and the one in force otherwise.
    /// With `prune`, the wants no part lists any more end; without, wants are
    /// added and no want ends, which is the first half of a batch: the wants
    /// its parts list, to any depth, before its arrivals are applied.
    ///
    /// A want whose parent's part is not at hand (a restored table awaiting
    /// its answers) is kept as it is. The wants are computed from the wants
    /// that are not the core's, so that two parts that list each other do
    /// not keep each other wanted.
    fn dependency_wants(&mut self, arriving: &BTreeMap<AssetId, Arc<Resident>>, prune: bool, starts: &Starts) {
        let rules = self.rules;
        let as_required = self.evaluating_the_gate && rules.gate_counts_core_wants_as_required;
        let lists = |resident: &Arc<Resident>| !resident.part.dependencies.is_empty();
        let core_owned = |entry: &Entry| entry.wants.iter().any(|(owner, _)| matches!(owner, Owner::Asset(_)));
        if !arriving.values().any(lists) && !self.entries.values().any(|entry| entry.resident.as_ref().is_some_and(lists) || core_owned(entry)) {
            return;
        }
        // The parents whose wants on their dependencies are kept as they are.
        let kept: BTreeSet<AssetId> = self
            .entries
            .iter()
            .filter(|(id, entry)| {
                let awaits_its_part = matches!(entry.availability, Availability::Available(_)) && entry.resident.is_none() && !arriving.contains_key(id);
                let failed_and_wanted =
                    rules.core_wants_kept_under_a_failed_parent && matches!(entry.availability, Availability::Failed(_)) && !entry.wants.is_empty();
                awaits_its_part || failed_and_wanted
            })
            .map(|(id, _)| *id)
            .collect();
        let part_of = |table: &Table, id: &AssetId| arriving.get(id).or_else(|| table.entries.get(id).and_then(|entry| entry.resident.as_ref())).cloned();
        let parents: Vec<(AssetId, Arc<Resident>)> = {
            let ids: BTreeSet<AssetId> = self.entries.keys().chain(arriving.keys()).copied().collect();
            ids.into_iter().filter_map(|id| Some((id, part_of(self, &id)?))).filter(|(_, part)| lists(part)).collect()
        };
        // The wants the parts ask for: per dependency and parent, the
        // policies and the type the parent's part lists for the dependency.
        let mut desired: BTreeMap<(AssetId, AssetId), (Vec<Policy>, AssetType)> = BTreeMap::new();
        loop {
            let mut changed = false;
            for (parent, part) in &parents {
                let mut policies: Vec<Policy> = Vec::new();
                let mut counted: BTreeSet<AssetId> = BTreeSet::new();
                if let Some(entry) = self.entries.get(parent).filter(|_| !starts.not_from.contains(parent)) {
                    for (owner, policy) in &entry.wants {
                        match owner {
                            Owner::Asset(above) => {
                                if let Some((asked, _)) = desired.get(&(*parent, *above)) {
                                    if counted.insert(*above) {
                                        policies.extend_from_slice(asked);
                                    }
                                } else if !prune || kept.contains(above) {
                                    policies.push(*policy);
                                }
                            }
                            _ => policies.push(*policy),
                        }
                    }
                }
                for ((_, above), (asked, _)) in desired.range((*parent, AssetId::MIN)..=(*parent, AssetId::MAX)) {
                    if counted.insert(*above) {
                        policies.extend_from_slice(asked);
                    }
                }
                if policies.is_empty() {
                    // The part of an asset no want names adds nothing, but
                    // under the control that follows every arriving part.
                    if !(starts.from_every_arrival && arriving.contains_key(parent)) {
                        continue;
                    }
                    policies.push(Policy::Optional);
                }
                // Under the control, the want a pending parent's part asks
                // for is required, before its arrival is counted and after.
                let asked: Vec<Policy> = if as_required && (arriving.contains_key(parent) || self.pending_parents.contains(parent)) {
                    vec![Policy::Required]
                } else if rules.dependency_want_per_parent_want {
                    policies
                } else {
                    policies.into_iter().max().into_iter().collect()
                };
                for (dependency, asset_type) in &part.part.dependencies {
                    if desired.get(&(*dependency, *parent)).is_none_or(|(held, _)| *held != asked) {
                        desired.insert((*dependency, *parent), (asked.clone(), *asset_type));
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        if prune {
            let mut changed = Vec::new();
            for (id, entry) in self.entries.iter_mut() {
                let before = entry.wants.len();
                entry.wants.retain(|(owner, _)| match owner {
                    Owner::Asset(parent) => desired.contains_key(&(*id, *parent)) || kept.contains(parent),
                    _ => true,
                });
                if entry.wants.len() != before {
                    changed.push(*id);
                }
            }
            for id in changed {
                self.follow(id);
            }
        }
        for ((dependency, parent), (asked, asset_type)) in desired {
            // Under the control, the dependency's entry takes its parent's type.
            let parents_type = self.entries.get(&parent).map(|entry| entry.asset_type).filter(|_| rules.dependency_takes_its_parents_type);
            // Under one control the first half of a batch adds wants and
            // leaves the policy of a want the core already holds as it is.
            let keeps_held = !prune && rules.core_wants_keep_their_policy_until_the_arrivals;
            self.want_dependency(dependency, parents_type.unwrap_or(asset_type), parent, &asked, keeps_held);
        }
    }

    /// A want the core adds for a dependency: owned by the parent, with the
    /// policies asked. It is never refused, and it states a type in two
    /// cases: where it is the dependency's first, and where it is added on
    /// an entry failed for the reason of another type, on which it takes the
    /// type its parent's part lists and replaces the entry's type, whatever
    /// other want stands on the entry, the wants made under the former type
    /// staying. Where it is added on a failed entry, it is the new load that
    /// returns the entry to unavailable; where the parent already held it,
    /// only its policy follows.
    fn want_dependency(&mut self, dependency: AssetId, asset_type: AssetType, parent: AssetId, asked: &[Policy], keeps_held: bool) {
        let rules = self.rules;
        let owner = Owner::Asset(parent);
        // The control that refuses a want of the core for its type.
        if rules.core_wants_are_refused_for_their_type && self.entries.get(&dependency).is_some_and(|entry| entry.asset_type != asset_type) {
            return;
        }
        let entry = self.entries.entry(dependency).or_insert_with(|| Entry::unavailable(asset_type));
        let held: Vec<Policy> = entry.wants.iter().filter(|(other, _)| *other == owner).map(|(_, policy)| *policy).collect();
        if held == asked || (keeps_held && !held.is_empty()) {
            return;
        }
        entry.wants.retain(|(other, _)| *other != owner);
        let at = rules.place(&entry.wants, &owner, &self.systems);
        for (k, policy) in asked.iter().enumerate() {
            entry.wants.insert(at + k, (owner, *policy));
        }
        let left_failed = rules.core_want_leaves_a_failed_entry_failed && matches!(entry.availability, Availability::Failed(_));
        if held.is_empty() && !left_failed {
            if entry.availability == Availability::Failed(REASON_ANOTHER_TYPE) && !rules.core_want_keeps_the_type_of_a_failed_entry {
                entry.asset_type = asset_type;
            }
            if entry.unanswered_for_a_new_want(&rules) {
                self.core_added.push((dependency, parent));
            }
        }
        self.follow(dependency);
    }

    fn take_core_added(&mut self, leaving: &mut Leaving) {
        leaving.extend(self.core_added.drain(..).map(|(dependency, parent)| (dependency, Owner::Asset(parent))));
    }

    fn apply_arrival(&mut self, arrival: &Arrival, residents: &dyn Residents, applied: &mut Applied) {
        self.apply_arrival_to_the_entry(arrival, residents, applied);
        self.follow(arrival.asset);
    }

    fn apply_arrival_to_the_entry(&mut self, arrival: &Arrival, residents: &dyn Residents, applied: &mut Applied) {
        let rules = self.rules;
        applied.answered_a_restore |= arrival.marked;
        self.no_longer_awaited(&arrival.asset);
        let Some(entry) = self.entries.get_mut(&arrival.asset) else {
            applied.unnamed_arrivals += 1;
            return;
        };
        let before = entry.availability;
        // An arrival that leaves an entry as it was: at the same hash, or
        // failed for the same reason. It changes nothing, the size the entry
        // holds included: a size is a function of the content an arrival
        // names, so an arrival at the hash in force names the size in force.
        // The same hashes are those of the canonical part and of the
        // prepared form built once the entry holds beside it: an arrival
        // that names the part in force with another form does not leave the
        // entry as it was. Under one control an answer to a restore is
        // compared with the hash of the canonical part alone.
        let same_form = entry.form == arrival.form || (arrival.marked && rules.answer_compared_with_the_part_alone);
        let as_it_was = match (before, arrival.outcome) {
            (Availability::Available(held), Outcome::Hash(hash)) => held == hash && same_form,
            (Availability::Failed(held), Outcome::Failure(code)) => held == code,
            _ => false,
        };
        // Under one control an entry that held no want when the record was
        // drained, and that no command of the record names, stays unnamed
        // whatever want the core added from a part of the record.
        let named = !entry.wants.is_empty() && !self.found.unnamed.contains(&arrival.asset);
        if arrival.marked && !as_it_was && (named || !rules.unwanted_answer_never_differs) {
            applied.differed.push(arrival.asset);
        }
        // An arrival for an asset no want names is recorded and changes
        // nothing. The test is made once the wants of the batch are added.
        if !named && !rules.unnamed_arrivals_applied {
            applied.unnamed_arrivals += 1;
            return;
        }
        match arrival.outcome {
            Outcome::Hash(hash) => {
                entry.availability = Availability::Available(hash);
                entry.resident = Some(resident(residents, hash));
                // Under one control an answer to a restore at another hash
                // leaves the size the entry held.
                let size_left = arrival.marked && rules.marked_answer_leaves_the_size && matches!(before, Availability::Available(_));
                if (!as_it_was && !size_left) || rules.arrival_at_the_hash_in_force_brings_its_size {
                    entry.size = arrival.size;
                }
                if !as_it_was {
                    entry.form = arrival.form;
                    self.uncounted.remove(&arrival.asset);
                }
                // An answer to a restore at a hash is no arrival and no reload: no event.
                if (as_it_was && !rules.unchanged_arrival_emits) || arrival.marked {
                    return;
                }
                // An event names the hash of a canonical part: an arrival
                // that changes the form an entry holds and leaves its part
                // as it was has no event to emit. The design text has no
                // sentence on such an arrival, and no scenario holds one.
                if !as_it_was && before == Availability::Available(hash) {
                    return;
                }
                applied.events.push(match before {
                    Availability::Available(old) => Event::HashChanged { asset: arrival.asset, from: old, to: hash },
                    _ => Event::Arrived { asset: arrival.asset, hash },
                });
            }
            Outcome::Failure(code) => {
                if as_it_was && !rules.repeated_failure_emits && !rules.unchanged_arrival_emits {
                    return;
                }
                // Under one control an entry that the record found failed
                // for this reason is left so in silence, whatever want of
                // the record returned it to unavailable meanwhile.
                if self.found.failed.get(&arrival.asset) == Some(&code) {
                    entry.availability = Availability::Failed(code);
                    return;
                }
                entry.availability = Availability::Failed(code);
                entry.size = 0;
                entry.resident = None;
                entry.form = None;
                // A failure is a failure as any other, an answer to a restore included.
                if !(arrival.marked && rules.marked_failure_emits_nothing) {
                    applied.events.push(Event::Failed { asset: arrival.asset, code });
                } else if rules.marked_failure_fails_the_instance {
                    for instance in self.instances.iter_mut().filter(|instance| instance.asset == arrival.asset) {
                        instance.failed = true;
                    }
                }
                if entry.policy() == Some(Policy::Required) {
                    applied.failed_required.push((arrival.asset, code));
                }
            }
        }
    }

    /// Applies one command of a record. Returns whether a want was added,
    /// changed or ended, so that the wants of dependencies follow.
    fn apply_command(&mut self, index: usize, command: &Command, purpose: Purpose, alive: &dyn Fn(u32, u32) -> bool, applied: &mut Applied, leaving: &mut Leaving) -> bool {
        let adding = purpose == Purpose::Tick || self.rules.gate_counts_added_wants;
        let despawning = purpose == Purpose::Tick || self.rules.gate_sees_despawns;
        // Under one control the copy the gate is evaluated on takes a
        // pending command that changes the policy of a want its maker holds.
        let changing = |table: &Table, asset: AssetId, owner: Owner| table.rules.gate_sees_pending_policy_changes && table.holds(asset, owner);
        match *command {
            Command::Want { asset, asset_type, owner, policy } if adding || changing(self, asset, owner) => {
                // A command that stands after the despawn of its entity in
                // one record finds it not alive. Under the reading that
                // applies a despawn at the structural change point of its
                // tick, the entity is alive until then.
                let seen_despawned = !self.rules.despawn_ends_wants_at_the_structural_point;
                if let Owner::Entity { slot, generation } = owner
                    && (!alive(slot, generation) || (seen_despawned && applied.despawned.contains(&(slot, generation))))
                {
                    applied.refusals.push((index, Refused::OwnerNotAlive));
                    return false;
                }
                self.apply_want(index, asset, asset_type, owner, policy, applied, leaving)
            }
            Command::Instantiate { asset, asset_type, policy, reload } if adding => {
                let maker = Owner::World(Maker::Instance(self.instances.len() as u32 + 1));
                let wanted = self.apply_want(index, asset, asset_type, maker, policy, applied, leaving);
                // An instance whose want is refused is not made.
                if wanted {
                    self.instances.push(Instance { asset, policy, reload, instantiations: 0, failed: false });
                }
                wanted
            }
            Command::Release { asset, owner } => self.release(asset, owner),
            Command::Despawn { slot, generation } if despawning => {
                if !alive(slot, generation) || applied.despawned.contains(&(slot, generation)) {
                    applied.refusals.push((index, Refused::OwnerNotAlive));
                    return false;
                }
                applied.despawned.push((slot, generation));
                if self.rules.despawn_ends_wants_at_the_structural_point {
                    return false;
                }
                self.end_wants_where(|owner| *owner == Owner::Entity { slot, generation })
            }
            Command::Want { .. } | Command::Instantiate { .. } | Command::Despawn { .. } => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_want(&mut self, index: usize, asset: AssetId, asset_type: AssetType, owner: Owner, policy: Policy, applied: &mut Applied, leaving: &mut Leaving) -> bool {
        match self.want(asset, asset_type, owner, policy) {
            Wanted::Refused => {
                applied.refusals.push((index, Refused::AnotherType));
                false
            }
            Wanted::PolicyOnly => {
                applied.wanted.push((asset, asset_type));
                true
            }
            Wanted::Applied { leaves } => {
                applied.wanted.push((asset, asset_type));
                if leaves {
                    leaving.push((asset, owner));
                }
                if self.rules.second_want_on_a_parent_retries_its_dependencies {
                    self.retry_the_failed_dependencies_of(asset, leaving);
                }
                true
            }
        }
    }

    /// Under one control: a want added on a parent in force is a want added
    /// on each dependency its part lists, so that one the table holds failed
    /// returns to unavailable and its load leaves the domain again.
    fn retry_the_failed_dependencies_of(&mut self, parent: AssetId, leaving: &mut Leaving) {
        let rules = self.rules;
        let part = self.entries.get(&parent).and_then(|entry| entry.resident.as_ref());
        let listed: Vec<AssetId> = part.map_or_else(Vec::new, |resident| resident.part.dependencies.iter().map(|(dependency, _)| *dependency).collect());
        for dependency in listed {
            let Some(entry) = self.entries.get_mut(&dependency) else { continue };
            if matches!(entry.availability, Availability::Failed(_)) && entry.unanswered_for_a_new_want(&rules) {
                leaving.push((dependency, Owner::Asset(parent)));
            }
            self.follow(dependency);
        }
    }

    /// Under one control: an entry that an arrival has just failed for its
    /// type, and on which a want of the core stands, takes the type the part
    /// of that parent lists for it, the part in force or the one the record
    /// brings, and returns to unavailable: its load starts again, the want
    /// leaving the domain with the type taken. The failure keeps its event.
    fn retry_under_the_type_its_parent_lists(&mut self, asset: AssetId, record: &Record, residents: &dyn Residents, leaving: &mut Leaving) {
        let Some(entry) = self.entries.get(&asset).filter(|entry| entry.availability == Availability::Failed(REASON_ANOTHER_TYPE)) else { return };
        let part_of = |parent: &AssetId| {
            let in_force = self.entries.get(parent).and_then(|entry| entry.resident.clone());
            let brought = record.arrivals.iter().find(|arrival| arrival.asset == *parent).and_then(|arrival| match arrival.outcome {
                Outcome::Hash(hash) => residents.get(hash),
                Outcome::Failure(_) => None,
            });
            brought.or(in_force)
        };
        let listed = entry.wants.iter().find_map(|(owner, _)| {
            let Owner::Asset(parent) = owner else { return None };
            let part = part_of(parent)?;
            part.part.dependencies.iter().find(|(dependency, _)| *dependency == asset).map(|(_, asset_type)| (*parent, *asset_type))
        });
        // An entry that already holds the type its parent's part lists has
        // nothing to take: it stays failed.
        let Some((parent, asset_type)) = listed.filter(|(_, asset_type)| *asset_type != entry.asset_type) else { return };
        let entry = self.entries.get_mut(&asset).expect("the entry was read above");
        entry.asset_type = asset_type;
        entry.availability = Availability::Unavailable;
        leaving.push((asset, Owner::Asset(parent)));
        self.follow(asset);
    }

    /// The batch of a record's arrivals: the wants its parts list first, to
    /// any depth, then the arrivals in identifier order, then the wants of
    /// dependencies recomputed. A want the core added with the batch leaves
    /// the domain where no arrival of the batch answers it.
    ///
    /// Under the unit of a rejection the design text proposes, the first
    /// step is another: every group is decided first, once the commands of
    /// the record are applied and before any want of the batch's parts is
    /// added; the wants the batch's parts list are then added, the parts of
    /// the rejected arrivals left out. `candidates` are the entries that
    /// held no want before the record, and `wanted_since` those of them a
    /// want named while its commands were applied, which the count of that
    /// unit reads.
    #[allow(clippy::too_many_arguments)]
    fn apply_arrivals(
        &mut self,
        record: &Record,
        residents: &dyn Residents,
        purpose: Purpose,
        applied: &mut Applied,
        leaving: &mut Leaving,
        candidates: &[AssetId],
        wanted_since: &BTreeSet<AssetId>,
    ) {
        if record.arrivals.is_empty() {
            return;
        }
        let rules = self.rules;
        let counts_core_wants = !(purpose == Purpose::Gate && rules.gate_leaves_out_core_wants);
        // Under one control of the gate the wants of the pending parts were
        // added before the pending releases, and follow nothing after.
        let added_early = purpose == Purpose::Gate && rules.gate_leaves_out_releases_for_core_wants;
        let proposed_unit = rules.budget_unit == BudgetUnit::DerivedClosure;
        let decided_first = if proposed_unit { self.over_budget_by_derived_closures(&record.arrivals, residents, candidates, wanted_since) } else { BTreeSet::new() };
        // The arrivals whose part gives no want: those the proposed unit
        // rejected, so that an asset a rejected part alone lists is named by
        // no want. A budget that only reports rejects nothing.
        let none = BTreeSet::new();
        let no_want_from = if rules.budget_only_reports || rules.wants_of_rejected_parts_added { &none } else { &decided_first };
        let mut brought = Leaving::new();
        if counts_core_wants && !added_early {
            self.want_the_dependencies_of(record, residents, rules.former_dependency_wants_end_before_the_batch, no_want_from);
            self.take_core_added(&mut brought);
        }
        // Under each of the three candidates, an arrival to a hash is tested
        // against the world's budget where it is applied: after the commands
        // of its record, after the wants the parts of the batch list are
        // added, and before the falls.
        let rejected = if proposed_unit { decided_first } else { self.over_budget(&record.arrivals, residents) };
        // The arrivals in identifier order, each applied but the rejected
        // ones: a rejected arrival that fails its asset does so at its own
        // place in that order, which is the place of its event.
        for arrival in &record.arrivals {
            match arrival.outcome {
                Outcome::Hash(hash) if rejected.contains(&arrival.asset) => self.reject(arrival, hash, residents, applied),
                _ => self.apply_arrival(arrival, residents, applied),
            }
            if rules.entry_failed_for_its_type_under_a_core_want_takes_the_type {
                self.retry_under_the_type_its_parent_lists(arrival.asset, record, residents, leaving);
            }
            if rules.core_wants_recomputed_at_each_arrival && counts_core_wants && !rules.core_wants_left {
                self.end_wants_no_longer_listed_by(arrival.asset);
            }
        }
        if counts_core_wants && !rules.core_wants_left && !added_early {
            self.dependency_wants(&BTreeMap::new(), true, &Starts::default());
            self.take_core_added(&mut brought);
        }
        let answered = |asset: &AssetId| record.arrivals.iter().any(|arrival| arrival.asset == *asset);
        leaving.extend(brought.into_iter().filter(|(asset, _)| !answered(asset)));
    }

    /// The first half of a batch: the wants its parts list, to any depth.
    /// With `prune`, a control, the wants a new part no longer lists end here,
    /// before any arrival of the batch is applied. The part of an arrival in
    /// `no_want_from` is not read: it will not be put in force.
    fn want_the_dependencies_of(&mut self, record: &Record, residents: &dyn Residents, prune: bool, no_want_from: &BTreeSet<AssetId>) {
        let mut arriving = BTreeMap::new();
        for arrival in record.arrivals.iter().filter(|arrival| !no_want_from.contains(&arrival.asset)) {
            let Outcome::Hash(hash) = arrival.outcome else { continue };
            let in_force = self.entries.get(&arrival.asset).is_some_and(|entry| matches!(entry.availability, Availability::Available(_)));
            // A part that is not resident is asked for only where its
            // arrival is applied, which a want must name.
            if !(self.rules.core_wants_left && in_force)
                && let Some(part) = residents.get(hash)
            {
                arriving.insert(arrival.asset, part);
            }
        }
        if self.evaluating_the_gate {
            self.pending_parents.extend(arriving.keys().copied());
        }
        // The two controls on where the first step starts from.
        let mut starts = Starts { from_every_arrival: self.rules.first_step_follows_every_arriving_part, ..Starts::default() };
        if self.rules.first_step_starts_from_the_wants_held_before_the_commands {
            let newly_wanted = |(id, entry): (&AssetId, &Entry)| (!entry.wants.is_empty() && !self.found.wanted.contains(id)).then_some(*id);
            starts.not_from = self.entries.iter().filter_map(newly_wanted).collect();
        }
        self.dependency_wants(&arriving, prune, &starts);
    }

    /// The size an entry counts for against the world's budget: the size it
    /// holds while its asset is available, which its arrival named. Under
    /// the stand-in the build read before, the length of the part in force.
    fn size_in_force(&self, asset: &AssetId, entry: &Entry) -> u64 {
        if self.uncounted.contains(asset) {
            return 0;
        }
        if self.rules.budget_counts_the_length_of_the_part {
            entry.resident.as_ref().map_or(0, |resident| resident.bytes.len() as u64)
        } else if matches!(entry.availability, Availability::Available(_)) {
            entry.size
        } else {
            0
        }
    }

    /// The size an arrival to a hash would put in force: the one it names.
    /// Under the stand-in the build read before, the length of its part.
    fn size_arriving(&self, arrival: &Arrival, residents: &dyn Residents) -> u64 {
        match arrival.outcome {
            Outcome::Hash(hash) if self.rules.budget_counts_the_length_of_the_part => residents.get(hash).map_or(0, |resident| resident.bytes.len() as u64),
            Outcome::Hash(_) => arrival.size,
            Outcome::Failure(_) => 0,
        }
    }

    /// The arrivals of a record that the world's budget rejects, under the
    /// candidate unit the rules name. The count of a world is the sum of the
    /// sizes its table's available entries hold, an entry that holds no want
    /// included. The budget is exceeded where the count would
    /// stand above it. An arrival at another hash of an asset the table holds
    /// available counts the new size in place of the former one. An arrival
    /// to failed has no size and is not tested; an arrival that would change
    /// nothing, for an asset no want names or at the hash the entry holds,
    /// adds nothing and is not tested either.
    fn over_budget(&self, arrivals: &[Arrival], residents: &dyn Residents) -> BTreeSet<AssetId> {
        let mut rejected = BTreeSet::new();
        let Some(budget) = self.budget.filter(|_| !self.rules.no_rejection_derived) else { return rejected };
        // The root of the group an arrival stands in, under each candidate.
        // The unit the design text proposes is decided apart, before the
        // wants of the batch's parts are added.
        let root_of: fn(&Arrival) -> AssetId = match self.rules.budget_unit {
            BudgetUnit::SingleArrival => |arrival| arrival.asset,
            BudgetUnit::ClosureOfAWant => |arrival| arrival.root.unwrap_or(arrival.asset),
            BudgetUnit::WholeDrain => |_| 0,
            BudgetUnit::DerivedClosure => return rejected,
        };
        let mut count: u64 = self.entries.iter().map(|(asset, entry)| self.size_in_force(asset, entry)).sum();
        // Per tested arrival: the asset, the root of its group, the size it
        // adds and the size it takes the place of.
        let mut tested: Vec<(AssetId, AssetId, u64, u64)> = Vec::new();
        for arrival in arrivals {
            let Outcome::Hash(hash) = arrival.outcome else { continue };
            let entry = self.entries.get(&arrival.asset);
            let named = entry.is_some_and(|entry| !entry.wants.is_empty());
            if (!named && !self.rules.budget_counts_unnamed_arrivals) || entry.is_some_and(|entry| entry.availability == Availability::Available(hash)) {
                continue;
            }
            let added = self.size_arriving(arrival, residents);
            let replaced = entry.map_or(0, |entry| self.size_in_force(&arrival.asset, entry));
            tested.push((arrival.asset, root_of(arrival), added, replaced));
        }
        // The groups are tested whole, in the identifier order of their
        // roots, each against the count as the groups accepted before it
        // leave it. Every group is decided before any arrival is applied.
        let roots: BTreeSet<AssetId> = tested.iter().map(|(_, root, _, _)| *root).collect();
        for root in roots {
            let group = || tested.iter().filter(move |(_, other, _, _)| *other == root);
            let added: u64 = group().map(|(_, _, added, _)| added).sum();
            let replaced: u64 = group().map(|(_, _, _, replaced)| replaced).sum();
            let with = count + added - replaced.min(count + added);
            if with > budget {
                rejected.extend(group().map(|(asset, _, _, _)| *asset));
            } else {
                count = with;
            }
        }
        rejected
    }

    /// The arrivals of a record that the world's budget rejects under the
    /// unit the design text takes: the group of a root, its bounds derived
    /// by the core. It is decided once the commands of the record are
    /// applied, before any want of the batch's parts is added and before any
    /// arrival is applied.
    ///
    /// A root is an asset under a want the core did not add. Its closure is
    /// the root with every asset reached from it through the lists of
    /// dependencies, read for each asset from the part in force and from the
    /// part of its arrival to a hash in this record, from both where it has
    /// both; it stops at an asset that has neither. Its group is the
    /// arrivals to a hash of this record whose asset stands in its closure.
    /// An arrival to failed stands in no group, and neither does an arrival
    /// at the hash the entry holds, nor one whose asset stands in the
    /// closure of no root: none of them is tested.
    ///
    /// The count is the sum of the sizes the table's available entries hold,
    /// each the size its arrival named, less what leaves those entries at
    /// this fixed point whatever the test gives: an entry that holds no want
    /// once the commands are applied, whose last want ended during the tick
    /// before, and that the closure of no root reaches; and an entry in
    /// force under a want that an arrival to failed of this record fails.
    ///
    /// The groups of the roots that hold a required want are tested before
    /// the others, each of the two sets in the identifier order of its
    /// roots. A root holds a required want where it stands in the world's
    /// required set once the commands of the record are applied, whichever
    /// maker made that want, the core included. A group is accepted where
    /// the count stays within the budget once every arrival of the group
    /// that no earlier group accepted is counted, an arrival at another hash
    /// of an asset in force counting the new size in place of the former
    /// one; its arrivals are then part of the count. A group that does not
    /// raise the count is accepted whatever the count. An arrival that
    /// stands in a group is rejected where every group that holds it is
    /// rejected.
    fn over_budget_by_derived_closures(
        &self,
        arrivals: &[Arrival],
        residents: &dyn Residents,
        candidates: &[AssetId],
        wanted_since: &BTreeSet<AssetId>,
    ) -> BTreeSet<AssetId> {
        self.groups_derived_and_decided(arrivals, residents, candidates, wanted_since).0
    }

    /// The same derivation and decision, with the count of the roots whose
    /// closure it read: what the measurement of its cost reports beside its
    /// duration.
    fn groups_derived_and_decided(
        &self,
        arrivals: &[Arrival],
        residents: &dyn Residents,
        candidates: &[AssetId],
        wanted_since: &BTreeSet<AssetId>,
    ) -> (BTreeSet<AssetId>, usize) {
        let rules = self.rules;
        let mut rejected = BTreeSet::new();
        let Some(budget) = self.budget.filter(|_| !rules.no_rejection_derived) else { return (rejected, 0) };
        let in_force = |asset: &AssetId| self.entries.get(asset).and_then(|entry| entry.resident.as_ref());
        // The arrival and the part of each arrival to a hash of the record.
        let mut arriving: BTreeMap<AssetId, (&Arrival, Option<Arc<Resident>>)> = BTreeMap::new();
        for arrival in arrivals {
            if let Outcome::Hash(hash) = arrival.outcome {
                arriving.insert(arrival.asset, (arrival, residents.get(hash)));
            }
        }
        if arriving.is_empty() {
            return (rejected, 0);
        }
        let listed_by = |asset: &AssetId| -> Vec<AssetId> {
            let parts = [in_force(asset), arriving.get(asset).and_then(|(_, part)| part.as_ref())];
            parts.into_iter().flatten().flat_map(|part| part.part.dependencies.iter().map(|(dependency, _)| *dependency)).collect()
        };
        // The closure of each root, and whether the root holds a required
        // want, in the identifier order of the roots.
        let is_a_root = |entry: &Entry| entry.wants.iter().any(|(owner, _)| !matches!(owner, Owner::Asset(_)));
        let mut closures: Vec<(bool, AssetId, BTreeSet<AssetId>)> = Vec::new();
        for (root, entry) in self.entries.iter().filter(|(_, entry)| is_a_root(entry)) {
            let mut closure = BTreeSet::new();
            let mut open = vec![*root];
            while let Some(asset) = open.pop() {
                if closure.insert(asset) {
                    open.extend(listed_by(&asset));
                }
            }
            // A root holds a required want where it stands in the world's
            // required set once the commands of the record are applied,
            // whichever maker made that want, the core included. Two
            // controls read it otherwise: as the record found the set, or
            // from the wants a maker other than the core made.
            let own = |(owner, policy): &(Owner, Policy)| *policy == Policy::Required && !matches!(owner, Owner::Asset(_));
            let required = if rules.budget_required_roots_read_before_the_commands {
                self.found.required.contains(root)
            } else if rules.budget_required_roots_by_their_own_wants {
                entry.wants.iter().any(own)
            } else {
                entry.policy() == Some(Policy::Required)
            };
            closures.push((required, *root, closure));
        }
        // Under one control each set is tested in the order in which the
        // wants of its roots were made.
        if rules.budget_groups_in_the_order_their_wants_were_made {
            closures.sort_by_key(|(_, root, _)| self.made.get(root).copied().unwrap_or(0));
        }
        // The groups of the roots that hold a required want are tested
        // first: in identifier order alone an optional closure whose root
        // stands first takes the room of a required one. The sort is stable,
        // so each of the two sets keeps the identifier order of its roots.
        if !rules.budget_groups_in_identifier_order_alone {
            closures.sort_by_key(|(required, _, _)| !required);
        }
        let closures: Vec<BTreeSet<AssetId>> = closures.into_iter().map(|(_, _, closure)| closure).collect();
        // The entries whose room is free: they leave the table's available
        // entries at this fixed point whatever the test gives. An entry due
        // to fall that a closure reaches stays counted. Each control counts
        // what it names.
        let reached = |asset: &AssetId| closures.iter().any(|closure| closure.contains(asset));
        let falls = |asset: &AssetId| {
            let unwanted = !wanted_since.contains(asset) && self.entries.get(asset).is_some_and(|entry| entry.wants.is_empty());
            unwanted && !reached(asset) && !rules.budget_count_holds_the_falling_entries
        };
        let mut leaving: BTreeSet<AssetId> = candidates.iter().copied().filter(falls).collect();
        if !rules.budget_count_holds_the_failing_entries {
            // An entry in force that holds a want once the commands are
            // applied and that an arrival to failed of the record fails. One
            // that holds no want stays counted, though the record holds its
            // arrival to failed: that arrival fails it only where a want of
            // the first step names it, which rests on the test. One control
            // frees its room all the same.
            let whatever_it_holds = rules.budget_count_leaves_out_every_entry_an_arrival_to_failed_names;
            let in_force = |entry: &Entry| matches!(entry.availability, Availability::Available(_));
            let fails = |arrival: &&Arrival| {
                let failed = |entry: &Entry| in_force(entry) && (whatever_it_holds || !entry.wants.is_empty());
                matches!(arrival.outcome, Outcome::Failure(_)) && self.entries.get(&arrival.asset).is_some_and(failed)
            };
            let mut failing: BTreeSet<AssetId> = arrivals.iter().filter(fails).map(|arrival| arrival.asset).collect();
            // The wants the core added end at the recomputation alone, after
            // the arrivals: the dependencies of a parent that fails stay
            // counted. One control frees their room too: an entry in force
            // whose every want is one a failing parent owns, to any depth.
            let mut more = rules.budget_count_leaves_out_the_dependencies_of_a_failing_parent;
            while more {
                let freed = |(asset, entry): (&AssetId, &Entry)| {
                    let owned_by_the_failing = |(owner, _): &(Owner, Policy)| matches!(owner, Owner::Asset(parent) if failing.contains(parent));
                    (!failing.contains(asset) && in_force(entry) && !entry.wants.is_empty() && entry.wants.iter().all(owned_by_the_failing)).then_some(*asset)
                };
                let freed: Vec<AssetId> = self.entries.iter().filter_map(freed).collect();
                more = !freed.is_empty();
                failing.extend(freed);
            }
            leaving.extend(failing);
        }
        let mut count: u64 = self.entries.iter().filter(|(asset, _)| !leaving.contains(*asset)).map(|(asset, entry)| self.size_in_force(asset, entry)).sum();
        let mut accepted: BTreeSet<AssetId> = BTreeSet::new();
        // A group that does not raise the count is accepted whatever the
        // count. Two controls: such a group is tested as any other, or it is
        // accepted only where it lowers the count.
        let within = |with: u64, count: u64| {
            let spared = if rules.budget_accepts_over_the_count_only_a_group_that_lowers_it { with < count } else { with <= count };
            with <= budget || (spared && !rules.budget_tests_a_group_that_does_not_raise_the_count)
        };
        for closure in &closures {
            // The group: per arrival, the asset, the size it adds and the
            // size it takes the place of.
            let mut group: Vec<(AssetId, u64, u64)> = Vec::new();
            for asset in closure {
                let Some((arrival, _)) = arriving.get(asset) else { continue };
                let entry = self.entries.get(asset);
                let as_it_was = entry.is_some_and(|entry| matches!((entry.availability, arrival.outcome), (Availability::Available(held), Outcome::Hash(hash)) if held == hash));
                if as_it_was {
                    continue;
                }
                group.push((*asset, self.size_arriving(arrival, residents), entry.map_or(0, |entry| self.size_in_force(asset, entry))));
            }
            if rules.budget_judges_each_arrival_of_a_group_alone {
                // The control that knows no group: each arrival alone, in
                // identifier order.
                for (asset, added, replaced) in group.iter().filter(|(asset, _, _)| !accepted.contains(asset)) {
                    let with = count + added - (*replaced).min(count + added);
                    if within(with, count) {
                        count = with;
                        rejected.remove(asset);
                    } else {
                        rejected.insert(*asset);
                    }
                }
                accepted.extend(group.iter().map(|(asset, _, _)| *asset).filter(|asset| !rejected.contains(asset)));
                continue;
            }
            let counted = || group.iter().filter(|(asset, _, _)| !accepted.contains(asset));
            let added: u64 = counted().map(|(_, added, _)| added).sum();
            let replaced: u64 = counted().map(|(_, _, replaced)| replaced).sum();
            let with = count + added - replaced.min(count + added);
            if within(with, count) {
                count = with;
                accepted.extend(group.iter().map(|(asset, _, _)| *asset));
            } else {
                rejected.extend(group.iter().map(|(asset, _, _)| *asset));
            }
        }
        // An arrival is applied where at least one group that holds it is
        // accepted.
        rejected.retain(|asset| !accepted.contains(asset));
        (rejected, closures.len())
    }

    /// The entries that hold no want: those the input fixed point of the
    /// next record takes as due to fall, before it applies its commands.
    pub fn entries_without_a_want(&self) -> Vec<AssetId> {
        self.entries.iter().filter(|(_, entry)| entry.wants.is_empty()).map(|(id, _)| *id).collect()
    }

    /// For the measurement of its cost: the derivation and the decision of
    /// the groups of a record's arrivals, alone, as the input fixed point
    /// makes them once the commands of the record are applied, the record
    /// holding none, and before any want is added from a part or any
    /// arrival is applied. `without_a_want` is what `entries_without_a_want`
    /// gave before. It changes nothing. Returns the arrivals rejected and
    /// the count of the roots whose closure the derivation read.
    pub fn groups_decided(&self, record: &Record, residents: &dyn Residents, without_a_want: &[AssetId]) -> (BTreeSet<AssetId>, usize) {
        self.groups_derived_and_decided(&record.arrivals, residents, without_a_want, &BTreeSet::new())
    }

    /// How the groups of a rejection over a budget are derived, in words.
    pub fn groups_form(&self) -> &'static str {
        "from every root of the table downwards: the closure of each asset under a want the core did not add is walked through the parts in force and the parts of the record, whatever the record holds"
    }

    /// A rejected arrival is not applied, and the result of the step names
    /// it. Where the table holds the asset available, the entry stays as it
    /// was and no event is emitted. Where it does not, the entry becomes
    /// failed for the reason of the budget, as by a failed arrival, with the
    /// event of a failure. An asset no want names keeps no trace: under the
    /// unit the design text proposes, an asset that a rejected part alone
    /// lists has no entry, no failure and no event. Under the control that
    /// only reports, the arrival is named and applied all the same.
    fn reject(&mut self, arrival: &Arrival, hash: u64, residents: &dyn Residents, applied: &mut Applied) {
        applied.rejected.push((arrival.asset, hash));
        if self.rules.budget_only_reports {
            return self.apply_arrival(arrival, residents, applied);
        }
        self.no_longer_awaited(&arrival.asset);
        let Some(entry) = self.entries.get_mut(&arrival.asset).filter(|entry| !entry.wants.is_empty()) else { return };
        if matches!(entry.availability, Availability::Available(_)) {
            return;
        }
        entry.availability = Availability::Failed(REASON_OVER_BUDGET);
        entry.size = 0;
        entry.resident = None;
        entry.form = None;
        applied.events.push(Event::Failed { asset: arrival.asset, code: REASON_OVER_BUDGET });
        if entry.policy() == Some(Policy::Required) {
            applied.failed_required.push((arrival.asset, REASON_OVER_BUDGET));
        }
        self.follow(arrival.asset);
    }

    /// Under the reading that recomputes at a parent's own arrival: the
    /// wants `parent` owns on assets its part in force does not list end.
    fn end_wants_no_longer_listed_by(&mut self, parent: AssetId) {
        let Some(entry) = self.entries.get(&parent) else { return };
        if matches!(entry.availability, Availability::Failed(_)) && self.rules.core_wants_kept_under_a_failed_parent {
            return;
        }
        let listed: Vec<AssetId> = entry.resident.as_ref().map_or_else(Vec::new, |resident| resident.part.dependencies.iter().map(|(id, _)| *id).collect());
        let mut changed = Vec::new();
        for (id, entry) in self.entries.iter_mut() {
            let before = entry.wants.len();
            entry.wants.retain(|(owner, _)| *owner != Owner::Asset(parent) || listed.contains(id));
            if entry.wants.len() != before {
                changed.push(*id);
            }
        }
        for id in changed {
            self.follow(id);
        }
    }

    /// Removes the entries that had no want before the record and that no
    /// want named while it was applied: the fall to unavailable.
    fn fall(&mut self, candidates: &[AssetId], wanted_since: &BTreeSet<AssetId>) -> Vec<AssetId> {
        let mut fallen = Vec::new();
        for id in candidates {
            if !wanted_since.contains(id) && self.entries.get(id).is_some_and(|entry| entry.wants.is_empty()) {
                self.entries.remove(id);
                self.no_longer_awaited(id);
                self.made.remove(id);
                self.follow(*id);
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
        self.core_added.clear();
        // The entries whose last want ended during the tick before.
        let candidates: Vec<AssetId> = self.entries.iter().filter(|(_, entry)| entry.wants.is_empty()).map(|(id, _)| *id).collect();
        // What the controls that read the table as the record found it need.
        self.found = Found::default();
        if self.rules.first_step_starts_from_the_wants_held_before_the_commands {
            self.found.wanted = self.entries.iter().filter(|(_, entry)| !entry.wants.is_empty()).map(|(id, _)| *id).collect();
        }
        if self.rules.budget_required_roots_read_before_the_commands {
            self.found.required = self.entries.iter().filter(|(_, entry)| entry.policy() == Some(Policy::Required)).map(|(id, _)| *id).collect();
        }
        if self.rules.failure_for_the_reason_held_at_the_drain_changes_nothing {
            let failed = |(id, entry): (&AssetId, &Entry)| match entry.availability {
                Availability::Failed(code) => Some((*id, code)),
                _ => None,
            };
            self.found.failed = self.entries.iter().filter_map(failed).collect();
        }
        // A world event changes the configuration at the boundary of its
        // tick: the budget it sets is the one the arrivals of its record are
        // tested against. Under one control it is in force from the tick
        // after, and is applied once the record is.
        let events_last = self.rules.budget_event_in_force_from_the_next_tick;
        if !events_last {
            self.apply_events(&record.events);
        }
        if self.rules.falls_before_record {
            applied.fallen = self.fall(&candidates, &BTreeSet::new());
        }
        // A candidate a want names while the record is applied did not end
        // its last want during the tick before, even if that want is ended
        // again by a later element of the same record.
        let mut wanted_since = BTreeSet::new();
        let mut leaving = Leaving::new();
        if self.rules.arrivals_before_commands {
            self.apply_arrivals(record, residents, purpose, &mut applied, &mut leaving, &candidates, &wanted_since);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        // One control of the gate: the wants of the pending parts are added
        // with the wants the table holds, before the pending releases.
        let added_early = purpose == Purpose::Gate && self.rules.gate_leaves_out_releases_for_core_wants;
        if added_early && !self.rules.gate_leaves_out_core_wants {
            self.want_the_dependencies_of(record, residents, false, &BTreeSet::new());
        }
        let late = self.apply_commands(&record.commands, purpose, alive, !added_early, &mut applied, &mut leaving, &candidates, &mut wanted_since);
        if self.rules.arrival_for_an_entry_unwanted_at_the_drain_changes_nothing {
            self.found.unnamed = candidates.iter().copied().filter(|id| !wanted_since.contains(id)).collect();
        }
        if !self.rules.arrivals_before_commands {
            self.apply_arrivals(record, residents, purpose, &mut applied, &mut leaving, &candidates, &wanted_since);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        if late {
            self.follow_the_commands_late(&mut leaving);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        if !self.rules.falls_before_record {
            applied.fallen = self.fall(&candidates, &wanted_since);
        }
        if events_last {
            self.apply_events(&record.events);
        }
        self.finish(&mut applied, leaving);
        applied
    }

    /// Applies the world events of a record: each sets a field of the
    /// world's configuration, which is no state and stands in no canonical
    /// byte of the table. What a budget lowered under the count does to what
    /// is in force is left open by the design text: nothing here.
    fn apply_events(&mut self, events: &[WorldEvent]) {
        for event in events {
            match *event {
                WorldEvent::Budget(bytes) => self.budget = Some(bytes),
            }
        }
    }

    /// For a restored table, under the control that starts its count from
    /// nothing: every entry in force counts for nothing until an arrival is
    /// applied to it.
    pub fn count_from_nothing(&mut self) {
        let in_force = |(asset, entry): (&AssetId, &Entry)| matches!(entry.availability, Availability::Available(_)).then_some(*asset);
        self.uncounted = self.entries.iter().filter_map(in_force).collect();
    }

    /// Applies commands in order. The core adds and recomputes the wants of
    /// dependencies wherever a want is applied on, or released from, a parent
    /// in force, and not with a batch alone.
    #[allow(clippy::too_many_arguments)]
    fn apply_commands(
        &mut self,
        commands: &[Command],
        purpose: Purpose,
        alive: &dyn Fn(u32, u32) -> bool,
        dependencies_follow: bool,
        applied: &mut Applied,
        leaving: &mut Leaving,
        candidates: &[AssetId],
        wanted_since: &mut BTreeSet<AssetId>,
    ) -> bool {
        // Under one control the wants the core added follow the commands of
        // a record once its arrivals are applied, and not before.
        let deferred = purpose == Purpose::Tick && self.rules.core_wants_keep_their_policy_until_the_arrivals;
        let follow = dependencies_follow && !self.rules.dependency_wants_with_a_batch_alone && !deferred;
        // The copy the gate is evaluated on needs the wants of dependencies
        // once, after the last pending command: nothing reads them between.
        let once = purpose == Purpose::Gate;
        let mut changed = false;
        for (index, command) in commands.iter().enumerate() {
            if self.apply_command(index, command, purpose, alive, applied, leaving) {
                changed = true;
                if follow && !once {
                    self.dependency_wants(&BTreeMap::new(), true, &Starts::default());
                    self.take_core_added(leaving);
                }
            }
            self.note_wanted(candidates, wanted_since);
        }
        if follow && once && changed {
            self.dependency_wants(&BTreeMap::new(), true, &Starts::default());
            self.take_core_added(leaving);
        }
        changed && deferred && dependencies_follow && !self.rules.dependency_wants_with_a_batch_alone
    }

    /// Under the control that defers it: the recomputation that the commands
    /// of a record asked for, made once its arrivals are applied.
    fn follow_the_commands_late(&mut self, leaving: &mut Leaving) {
        self.dependency_wants(&BTreeMap::new(), true, &Starts::default());
        self.take_core_added(leaving);
    }

    /// Under one control: an entry leaves the table at the fixed point that
    /// ends its last want, whatever its availability, and not at the input
    /// fixed point of the next tick.
    fn leave_at_once(&mut self) {
        if !self.rules.entry_leaves_at_the_fixed_point_that_ends_its_last_want {
            return;
        }
        self.entries.retain(|_, entry| !entry.wants.is_empty());
        let entries = &self.entries;
        self.awaiting.retain(|asset| entries.contains_key(asset));
        self.made.retain(|asset, _| entries.contains_key(asset));
    }

    /// What ends the application of a record, or of what a system did: the
    /// consumers read the events, and the wants that leave the domain are
    /// those added on an asset unanswered or failed that stand at the end.
    fn finish(&mut self, applied: &mut Applied, leaving: Leaving) {
        for event in &applied.events {
            self.consume(event);
        }
        // What leaves the domain is extracted at the tick's end: a want
        // added and ended inside the record is not in that state.
        for (asset, owner) in leaving {
            let stands = self.holds(asset, owner) || self.rules.a_want_ended_in_its_record_still_leaves;
            if let Some(entry) = self.entries.get(&asset).filter(|_| stands)
                && !applied.left.iter().any(|(other, _)| *other == asset)
            {
                applied.left.push((asset, entry.asset_type));
            }
        }
        self.leave_at_once();
        if self.rules.unrisen_entry_leaves_at_once {
            self.entries.retain(|_, entry| !entry.wants.is_empty() || matches!(entry.availability, Availability::Available(_)));
        }
        self.recount();
    }

    /// Applies what a system did inside a tick, through its declared access
    /// to the wants of the table: the wants it made, those it released and
    /// the despawns it asked for, in the order given. It is no element of an
    /// input record: a replay runs the system again. It is applied at the
    /// structural change fixed point of the system's tick, so that no arrival
    /// and no fall of that tick's input fixed point follows it.
    pub fn apply_system(&mut self, commands: &[Command], alive: &dyn Fn(u32, u32) -> bool) -> Applied {
        let mut applied = Applied::default();
        if commands.is_empty() {
            return applied;
        }
        self.core_added.clear();
        let mut leaving = Leaving::new();
        if self.apply_commands(commands, Purpose::Tick, alive, true, &mut applied, &mut leaving, &[], &mut BTreeSet::new()) {
            self.follow_the_commands_late(&mut leaving);
        }
        self.finish(&mut applied, leaving);
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
    /// by a pending arrival or by a pending release, and every entry a
    /// restore awaits has its answer pending.
    pub fn gate_open(&self, record: &Record, residents: &dyn Residents) -> bool {
        self.gate(record, residents).is_empty()
    }

    /// What keeps the gate closed once the pending record is counted: the
    /// assets still unanswered, none where the gate is open.
    pub fn gate(&self, record: &Record, residents: &dyn Residents) -> Vec<AssetId> {
        self.gate_in_two_parts(record, residents).0
    }

    /// The form that evaluates the gate under the rules, in words.
    pub fn gate_form(&self) -> &'static str {
        match (self.rules.gate_copies_the_table, self.rules.required_set_not_kept) {
            (true, _) => "a copy of the whole table, on which the pending record is applied",
            (false, false) => "the pending elements read against the kept set of the required assets the table holds unanswered, with no copy",
            (false, true) => "the pending elements read against the set of the required assets the table holds unanswered, derived by one scan of the table, no set being kept",
        }
    }

    /// The same evaluation, with the time of its two parts, for the
    /// measurement of its cost: the copy of the table, then the rest, the
    /// pending elements read and the verdict. The first is zero where no
    /// copy is made, and both where the pending record is not read.
    pub fn gate_in_two_parts(&self, record: &Record, residents: &dyn Residents) -> (Vec<AssetId>, Duration, Duration) {
        // A world that is not gated steps: its pending record is not read.
        if !self.blocked() && !self.rules.gate_read_on_a_world_that_is_not_gated {
            return (Vec::new(), Duration::ZERO, Duration::ZERO);
        }
        if record.is_empty() {
            return (self.required_unanswered(), Duration::ZERO, Duration::ZERO);
        }
        let started = Instant::now();
        if !self.rules.gate_copies_the_table {
            let awaited = self.gate_on_the_pending_elements(record, residents);
            return (awaited, Duration::ZERO, started.elapsed());
        }
        // The form kept for the comparison of costs: a copy of the table.
        let mut after = self.clone();
        let copied = started.elapsed();
        after.evaluating_the_gate = true;
        after.apply_for(record, residents, Purpose::Gate, &|_, _| true);
        let awaited = after.required_unanswered();
        (awaited, copied, started.elapsed() - copied)
    }

    /// The incremental form of the gate: the pending elements read against
    /// the set of the required assets the table holds unanswered and against
    /// the count of the entries a restore still awaits. Nothing of the table
    /// is copied but the entries a pending command names, no closure is
    /// walked, and the cost follows the pending record and that set.
    ///
    /// Two kinds of pending element are applied, and no other. A pending
    /// arrival answers the asset it names. A pending release is applied to
    /// the wants of the asset it names, which counts as answered where no
    /// required want is left on it. A pending command that adds a want, or
    /// that changes the policy of a want its maker holds, is left out, and a
    /// pending despawn is not seen.
    ///
    /// A want the core would add from the part of a pending arrival is
    /// counted: where the wants the table holds on the parent, a pending
    /// release applied, make it required, a dependency its part lists that
    /// the table does not hold available keeps the gate closed until its own
    /// arrival is pending. The parts of the pending arrivals are followed
    /// downwards, to any depth, as the first step of a tick follows them.
    ///
    /// The gate does not see the wants that a pending element would end
    /// through the recomputation: a pending release of a parent's last
    /// required want, a pending failure of the parent, or a pending
    /// replacement whose part no longer lists a dependency each leave the
    /// gate closed on a dependency the world is gated on until its own
    /// answer is pending. And it does not apply the world's budget to the
    /// pending arrivals, which would ask for the closure of every root.
    fn gate_on_the_pending_elements(&self, record: &Record, residents: &dyn Residents) -> Vec<AssetId> {
        self.gate_read(record, residents, true).0
    }

    /// The same evaluation, with the count of the entries a restore still
    /// awaits once the pending record is counted. Without `naming`, the
    /// entries a restore awaits are counted and not named: the verdict reads
    /// the count, and no awaited entry is walked.
    fn gate_read(&self, record: &Record, residents: &dyn Residents, naming: bool) -> (Vec<AssetId>, usize) {
        let rules = self.rules;
        let required = self.required_set();
        debug_assert!(rules.required_set_not_kept || *required == self.scanned_required(), "the kept set is the one a scan of the table gives");
        let pending: BTreeMap<AssetId, &Arrival> = record.arrivals.iter().map(|arrival| (arrival.asset, arrival)).collect();
        let mut closed: BTreeSet<AssetId> = BTreeSet::new();

        // The count of a restore: every awaited entry has its answer
        // pending, or the gate is closed on those that have none. The
        // awaited entries are walked only to name them.
        let answered = pending.keys().filter(|asset| self.awaiting.contains(*asset)).count();
        let awaited = self.awaiting.len() - answered;
        if awaited > 0 && naming {
            closed.extend(self.awaiting.iter().filter(|asset| !pending.contains_key(*asset)).copied());
        }

        // The entries a pending command the gate applies names, each copied
        // alone and brought to what those commands leave of its wants.
        let mut named = Table::with_rules(Rules { required_set_not_kept: true, ..rules });
        let mut released: BTreeSet<AssetId> = BTreeSet::new();
        let mut despawned: Vec<(u32, u32)> = Vec::new();
        let mut instances = self.instances.len() as u32;
        for command in &record.commands {
            match *command {
                Command::Release { asset, owner } => {
                    self.name_for_the_gate(&mut named, asset);
                    if named.release(asset, owner) {
                        released.insert(asset);
                    }
                }
                Command::Want { asset, asset_type, owner, policy } => {
                    // The controls that apply more of the pending record: a
                    // want added, or the policy of a want its maker holds.
                    let held = if named.entries.contains_key(&asset) { named.holds(asset, owner) } else { self.holds(asset, owner) };
                    let seen = rules.gate_counts_added_wants || (held && rules.gate_sees_pending_policy_changes);
                    let gone = matches!(owner, Owner::Entity { slot, generation } if despawned.contains(&(slot, generation)));
                    if seen && !gone {
                        self.name_for_the_gate(&mut named, asset);
                        named.want(asset, asset_type, owner, policy);
                    }
                }
                Command::Instantiate { asset, asset_type, policy, .. } => {
                    if rules.gate_counts_added_wants {
                        self.name_for_the_gate(&mut named, asset);
                        if named.want(asset, asset_type, Owner::World(Maker::Instance(instances + 1)), policy) != Wanted::Refused {
                            instances += 1;
                        }
                    }
                }
                Command::Despawn { slot, generation } => {
                    // The control that sees a pending despawn reads the
                    // whole table for the wants of the entity.
                    if rules.gate_sees_despawns && !despawned.contains(&(slot, generation)) {
                        despawned.push((slot, generation));
                        let owner = Owner::Entity { slot, generation };
                        let owned = |entry: &Entry| entry.wants.iter().any(|(other, _)| *other == owner);
                        let assets: Vec<AssetId> = self.entries.iter().filter(|(_, entry)| owned(entry)).map(|(id, _)| *id).chain(named.entries.keys().copied()).collect();
                        for asset in assets {
                            self.name_for_the_gate(&mut named, asset);
                            named.release(asset, owner);
                        }
                    }
                }
            }
        }
        let entry_of = |asset: &AssetId| named.entries.get(asset).or_else(|| self.entries.get(asset));

        // The required assets the table holds unanswered, and the entries a
        // pending command named: each is answered by a pending arrival, or
        // by a pending release that leaves no required want on it.
        for asset in required.iter().chain(named.entries.keys()) {
            let answered_by_a_release = rules.gate_takes_a_pending_release_as_an_answer && released.contains(asset);
            // Under one control a release answers only where it leaves no
            // want at all on an asset the table holds required and unanswered.
            let unanswered = if rules.gate_release_answers_where_no_want_is_left && released.contains(asset) {
                let wanted = named.entries.get(asset).is_some_and(|entry| !entry.wants.is_empty());
                wanted && self.entries.get(asset).is_some_and(|entry| entry.required_and_unanswered(&rules))
            } else {
                entry_of(asset).is_some_and(|entry| entry.required_and_unanswered(&rules))
            };
            if !pending.contains_key(asset) && !answered_by_a_release && unanswered {
                closed.insert(*asset);
            }
        }

        // The wants the core would add from the parts of the pending
        // arrivals to a hash.
        if !rules.gate_leaves_out_core_wants {
            let lists = |resident: &Arc<Resident>| !resident.part.dependencies.is_empty();
            let mut parts: BTreeMap<AssetId, Arc<Resident>> = BTreeMap::new();
            for arrival in &record.arrivals {
                if let Outcome::Hash(hash) = arrival.outcome
                    && let Some(part) = residents.get(hash).filter(lists)
                {
                    parts.insert(arrival.asset, part);
                }
            }
            // The policy the wants the table holds give each pending parent,
            // a pending release applied. Under one control the release is
            // left out; under another the want is required whatever the
            // wants of its parent, where the parent holds one.
            let held_policy = |asset: &AssetId| {
                let entry = if rules.gate_leaves_out_releases_for_core_wants { self.entries.get(asset) } else { entry_of(asset) };
                let policy = entry.and_then(Entry::policy);
                if rules.gate_counts_core_wants_as_required { policy.map(|_| Policy::Required) } else { policy }
            };
            let mut policies: BTreeMap<AssetId, Policy> = parts.keys().filter_map(|parent| Some((*parent, held_policy(parent)?))).collect();
            // A want the core would add on a pending parent gives that
            // parent's part its policy, to any depth through the parts of
            // the pending arrivals and through them alone.
            let mut followed = rules.gate_counts_core_wants_from_the_tables_wants_alone;
            while !followed {
                followed = true;
                for (parent, part) in &parts {
                    let Some(policy) = policies.get(parent).copied() else { continue };
                    for (dependency, _) in &part.part.dependencies {
                        if parts.contains_key(dependency) && policies.get(dependency).is_none_or(|held| *held < policy) {
                            policies.insert(*dependency, policy);
                            followed = false;
                        }
                    }
                }
            }
            for (parent, part) in &parts {
                if policies.get(parent) != Some(&Policy::Required) {
                    continue;
                }
                for (dependency, asset_type) in &part.part.dependencies {
                    if pending.contains_key(dependency) {
                        continue;
                    }
                    // What the want would leave of the dependency's entry: a
                    // new entry is unanswered, an unanswered one stays so
                    // under a required want, and a failed one returns to
                    // unavailable where the parent does not hold the want
                    // already.
                    let unanswered = match entry_of(dependency) {
                        None => true,
                        Some(entry) if rules.core_wants_are_refused_for_their_type && entry.asset_type != *asset_type => false,
                        Some(entry) => match entry.availability {
                            Availability::Available(_) => false,
                            Availability::Unavailable => true,
                            Availability::Failed(_) => {
                                let held = entry.wants.iter().any(|(owner, _)| *owner == Owner::Asset(*parent));
                                !held && !rules.core_want_leaves_a_failed_entry_failed && !rules.failed_entry_stays_failed_on_a_want
                            }
                        },
                    };
                    if unanswered {
                        closed.insert(*dependency);
                    }
                }
            }
        }
        (closed.into_iter().collect(), awaited)
    }

    /// The verdict of the gate alone for the tick whose pending record is
    /// given: whether it is open, and the count of the entries a restore
    /// still awaits once that record is counted. It is the evaluation of
    /// `gate` without the names of what a restore awaits: the count of the
    /// awaited entries is read against the pending arrivals, and none of
    /// them is walked. A required asset the table holds unanswered is read
    /// as `gate` reads it.
    pub fn gate_verdict(&self, record: &Record, residents: &dyn Residents) -> (bool, usize) {
        if !self.blocked() && !self.rules.gate_read_on_a_world_that_is_not_gated {
            return (true, 0);
        }
        if record.is_empty() {
            return (!self.blocked(), self.awaiting.len());
        }
        if self.rules.gate_copies_the_table {
            // The form kept for the comparison of costs has no verdict apart
            // from its names.
            let closed = self.gate(record, residents);
            let awaited = closed.iter().filter(|asset| self.awaiting.contains(*asset)).count();
            return (closed.is_empty(), awaited);
        }
        let (closed, awaited) = self.gate_read(record, residents, false);
        (closed.is_empty() && awaited == 0, awaited)
    }

    /// Copies the entry of an asset, where the table holds one, into the
    /// entries a pending command of the gate names.
    fn name_for_the_gate(&self, named: &mut Table, asset: AssetId) {
        if !named.entries.contains_key(&asset)
            && let Some(entry) = self.entries.get(&asset)
        {
            named.entries.insert(asset, entry.clone());
        }
    }

    /// Recomputes what is derived from the entries, the kept set of the
    /// required assets the table holds unanswered included, by a scan of the
    /// table. For a table read from its bytes or changed entry by entry from
    /// outside; a record applied keeps the set up to date by itself.
    pub fn refresh(&mut self) {
        self.required = if self.rules.required_set_not_kept { BTreeSet::new() } else { self.scanned_required() };
        self.recount();
    }

    /// Recomputes the two counts the fixed points read: whether an entry has
    /// no want left, and how many wants entities own.
    fn recount(&mut self) {
        self.orphans = self.entries.values().any(|entry| entry.wants.is_empty());
        let owned_by_entity = |(owner, _): &&(Owner, Policy)| matches!(owner, Owner::Entity { .. });
        self.entity_wants = self.entries.values().map(|entry| entry.wants.iter().filter(owned_by_entity).count()).sum();
        debug_assert!(self.rules.required_set_not_kept || self.required == self.scanned_required(), "the kept set is the one a scan of the table gives");
    }

    /// The canonical bytes of the table: per asset in identifier order, its
    /// asset type as the stable name of the type's schema entry, its wants
    /// in their total order, and its availability with the hash and the size
    /// its arrival named, or the reason code; then the instance records in
    /// creation order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let (typed, sized) = (!self.rules.type_left_out_of_the_bytes, !self.rules.size_left_out_of_the_bytes);
        if typed && sized {
            return self.typed_bytes();
        }
        // The controls: the bytes the table had before an entry held a type,
        // or before it held a size.
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, entry) in &self.entries {
            out.extend_from_slice(&id.to_le_bytes());
            if typed {
                put_type(&mut out, entry.asset_type);
            }
            put_wants_and_availability(&mut out, entry.availability, entry.form, sized.then_some(entry.size), &entry.wants);
        }
        put_instances(&mut out, &self.instances);
        out
    }

    /// The canonical bytes with the type and the size of every entry,
    /// whatever the rules: what a save holds and what `decode` reads back.
    pub fn typed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, entry) in &self.entries {
            put_entry_with_its_form(&mut out, *id, entry.asset_type, entry.availability, entry.form, entry.size, &entry.wants);
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
            let asset_type = reader.asset_type()?;
            let mut wants = Vec::new();
            for _ in 0..reader.u32()? {
                wants.push((read_owner(&mut reader)?, read_policy(&mut reader)?));
            }
            let (tag, value) = (reader.u8()?, reader.u64()?);
            let (availability, form, size) = match tag {
                0 => (Availability::Unavailable, None, 0),
                1 => (Availability::Available(value), None, reader.u64()?),
                2 => (Availability::Failed(value as u32), None, 0),
                3 => (Availability::Available(value), Some(reader.u64()?), reader.u64()?),
                other => return Err(format!("availability tag {other}")),
            };
            table.entries.insert(id, Entry { asset_type, wants, availability, size, resident: None, form });
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

/// One entry in the canonical form: the identifier, the asset type as the
/// stable name of its schema entry, the wants in the order given, and the
/// availability, an available entry with its hash and the size it holds.
/// Public so that a harness can write a table stated by hand through the
/// same bytes.
pub fn put_entry(out: &mut Vec<u8>, id: AssetId, asset_type: AssetType, availability: Availability, size: u64, wants: &[(Owner, Policy)]) {
    put_entry_with_its_form(out, id, asset_type, availability, None, size, wants);
}

/// The same, for an entry that may hold, while it is available, the hash of
/// a prepared form built once beside the hash of its canonical part.
pub fn put_entry_with_its_form(out: &mut Vec<u8>, id: AssetId, asset_type: AssetType, availability: Availability, form: Option<u64>, size: u64, wants: &[(Owner, Policy)]) {
    out.extend_from_slice(&id.to_le_bytes());
    put_type(out, asset_type);
    put_wants_and_availability(out, availability, form, Some(size), wants);
}

/// The wants and the availability of an entry. An available entry holds its
/// hash, then the hash of its prepared form built once where it holds one,
/// then the size its arrival named, which a control leaves out. An entry
/// that holds no form has the bytes it had before an entry could hold one:
/// the availability of one that does is written under a tag of its own.
fn put_wants_and_availability(out: &mut Vec<u8>, availability: Availability, form: Option<u64>, size: Option<u64>, wants: &[(Owner, Policy)]) {
    out.extend_from_slice(&(wants.len() as u32).to_le_bytes());
    for (owner, policy) in wants {
        put_owner(out, *owner);
        put_policy(out, *policy);
    }
    let (tag, value) = match (availability, form) {
        (Availability::Unavailable, _) => (0_u8, 0_u64),
        (Availability::Available(hash), None) => (1, hash),
        (Availability::Available(hash), Some(_)) => (3, hash),
        (Availability::Failed(code), _) => (2, u64::from(code)),
    };
    out.push(tag);
    out.extend_from_slice(&value.to_le_bytes());
    if let (Availability::Available(_), Some(form)) = (availability, form) {
        out.extend_from_slice(&form.to_le_bytes());
    }
    if let (Availability::Available(_), Some(size)) = (availability, size) {
        out.extend_from_slice(&size.to_le_bytes());
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod magnitude_tests;

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
