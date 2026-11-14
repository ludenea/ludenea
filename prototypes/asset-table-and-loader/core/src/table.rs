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
//! Every rule a scenario's control changes on purpose is a field of `Rules`,
//! and so is every place where the text was read one way among several.

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

/// One reason of a failure: its code in the canonical form, a short name and
/// what it says.
pub struct Reason {
    pub code: u32,
    pub name: &'static str,
    pub text: &'static str,
}

/// The reasons of a failure: a list in the schema entry of the arrival
/// transition. Three of them are named by the design text and raised by
/// nothing this prototype builds yet: another schema entry, an admission
/// check, a preparation.
pub const REASONS: [Reason; 10] = [
    Reason { code: REASON_UNKNOWN_ASSET, name: "unknown-asset", text: "unknown asset: the manifest the world stands under does not hold the identifier" },
    Reason { code: REASON_INVALID_PART, name: "invalid-part", text: "the bytes do not hash to their name or the canonical part does not validate" },
    Reason { code: REASON_HASH_MISMATCH, name: "hash-mismatch", text: "the artifact's bytes do not hash to the announced content hash" },
    Reason { code: REASON_ARTIFACT_GONE, name: "artifact-gone", text: "the store holds no bytes under the hash the manifest names" },
    Reason { code: REASON_ANOTHER_TYPE, name: "other-type", text: "the manifest holds the asset under another type than the one its entry holds" },
    Reason { code: REASON_DEPENDENCIES_DIFFER, name: "dependencies-differ", text: "the dependencies the part lists are not those the manifest lists for it" },
    Reason { code: REASON_ANOTHER_SCHEMA_ENTRY, name: "another-schema-entry", text: "the package was built under another schema entry of the asset's type" },
    Reason { code: REASON_ADMISSION_REFUSED, name: "admission-refused", text: "the admission check of the asset's type refused a first load" },
    Reason { code: REASON_PREPARATION_FAILED, name: "preparation-failed", text: "the preparation of a form failed on a first load" },
    Reason { code: REASON_OVER_BUDGET, name: "over-budget", text: "the arrival was rejected over the world's budget for its asset data" },
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
/// and whether it answers a restore. Under the candidate unit of a budget
/// that rejects the closure of one want, and under it alone, an arrival also
/// names a root: the asset of the want it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub asset: AssetId,
    pub outcome: Outcome,
    pub marked: bool,
    pub root: Option<AssetId>,
}

impl Arrival {
    pub fn at(asset: AssetId, hash: u64) -> Arrival {
        Arrival { asset, outcome: Outcome::Hash(hash), marked: false, root: None }
    }

    pub fn failed(asset: AssetId, code: u32) -> Arrival {
        Arrival { asset, outcome: Outcome::Failure(code), marked: false, root: None }
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
    /// What an excess over a world's budget rejects: one of the three
    /// candidate units the design text names and leaves open.
    pub budget_unit: BudgetUnit,
    // --- the readings the amended text did not take ---
    /// Not taken: a want added on a failed entry leaves it failed until the
    /// arrival of the retry. The default returns it to unavailable.
    pub failed_entry_stays_failed_on_a_want: bool,
    /// Not taken: the first want decides an entry's type even when wrong, a
    /// want of another type being refused on an entry failed for its type.
    pub first_type_holds: bool,
    /// Not taken: the makers under the world's own entry are ordered by
    /// their numeric identity alone, the kind breaking a tie. The default
    /// orders contributors, then command sources, then instances, each by
    /// identity.
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
    /// The wants the core added for the dependencies of a parent that fails
    /// are kept until the parent's last want ends. The default ends them
    /// with the failure: a failed parent has no part in force.
    pub core_wants_kept_under_a_failed_parent: bool,
    /// The wants of a parent's dependencies are recomputed at the parent's
    /// own arrival, in identifier order. The default recomputes them once
    /// the batch's arrivals are applied, so that an arrival of the batch for
    /// a former dependency is applied whatever the order of the identifiers.
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
    /// A step on a gated world names what its pending record leaves
    /// unanswered. The default names what the state holds unanswered, the
    /// pending answers not being applied.
    pub step_names_what_the_pending_record_leaves: bool,
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
}

/// What an excess over a world's budget rejects. The design text names
/// three candidates and leaves the choice open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BudgetUnit {
    /// Each arrival to a hash is tested alone, in identifier order.
    #[default]
    SingleArrival,
    /// The arrivals that answer one want, which name one root, are accepted
    /// together or rejected together.
    ClosureOfAWant,
    /// The arrivals to a hash of one record are one group.
    WholeDrain,
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
    fn place(&self, wants: &[(Owner, Policy)], owner: &Owner) -> usize {
        if self.wants_in_making_order {
            return wants.len();
        }
        wants.iter().position(|(other, _)| self.owner_order(other, owner) == Ordering::Greater).unwrap_or(wants.len())
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
/// the kind of its maker, then its identity.
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
            Outcome::Hash(hash) => {
                out.push(0);
                out.extend_from_slice(&hash.to_le_bytes());
            }
            Outcome::Failure(code) => {
                out.push(1);
                out.extend_from_slice(&u64::from(code).to_le_bytes());
            }
        }
        // One byte of marks: an answer to a restore, and the root an arrival
        // names under the candidate of a budget that writes it. An arrival
        // that names no root has the bytes it had before the mark existed.
        out.push(u8::from(arrival.marked) | (u8::from(arrival.root.is_some()) << 1));
        if let Some(root) = arrival.root {
            out.extend_from_slice(&root.to_le_bytes());
        }
    }
}

fn read_record(reader: &mut Reader) -> Result<Record, String> {
    let tick = reader.u32()?;
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
        let outcome = match reader.u8()? {
            0 => Outcome::Hash(reader.u64()?),
            1 => {
                // A failure names a reason of the list its schema entry holds.
                let code = reader.u64()?;
                let listed = u32::try_from(code).ok().filter(|code| reason(*code).is_some());
                Outcome::Failure(listed.ok_or_else(|| format!("a failure names the reason {code}, which the list does not hold"))?)
            }
            other => return Err(format!("outcome tag {other}")),
        };
        let marks = reader.u8()?;
        if marks > 3 {
            return Err(format!("arrival marks {marks}"));
        }
        let root = if marks & 2 != 0 { Some(reader.id()?) } else { None };
        arrivals.push(Arrival { asset, outcome, marked: marks & 1 != 0, root });
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
    /// The asset type the entry holds: the one its first want stated.
    pub asset_type: AssetType,
    /// The wants of the asset, in their total order.
    pub wants: Vec<(Owner, Policy)>,
    pub availability: Availability,
    /// The reference to the resident part a world holds while the asset is available.
    pub resident: Option<Arc<Resident>>,
}

impl Entry {
    fn unavailable(asset_type: AssetType) -> Entry {
        Entry { asset_type, wants: Vec::new(), availability: Availability::Unavailable, resident: None }
    }

    /// The policy the asset is under: required if any want is.
    pub fn policy(&self) -> Option<Policy> {
        self.wants.iter().map(|(_, policy)| *policy).max()
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

    /// The query of the table at a boundary where no tick has ended: the
    /// wants whose asset is unanswered, each with the type its entry holds.
    pub fn unanswered(&self) -> Vec<(AssetId, AssetType)> {
        let wanted = |entry: &&Entry| !entry.wants.is_empty() && entry.availability == Availability::Unavailable;
        self.entries.iter().filter(|(_, entry)| wanted(entry)).map(|(id, entry)| (*id, entry.asset_type)).collect()
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
                entry.resident = None;
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
                let at = rules.place(&entry.wants, &owner);
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
        true
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
            if !self.rules.dependency_wants_with_a_batch_alone {
                self.dependency_wants(&BTreeMap::new(), true);
            }
            self.core_added.clear();
            if self.rules.unrisen_entry_leaves_at_once {
                self.entries.retain(|_, entry| !entry.wants.is_empty() || matches!(entry.availability, Availability::Available(_)));
            }
            self.refresh();
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
    fn dependency_wants(&mut self, arriving: &BTreeMap<AssetId, Arc<Resident>>, prune: bool) {
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
                if let Some(entry) = self.entries.get(parent) {
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
                    continue;
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
            for (id, entry) in self.entries.iter_mut() {
                entry.wants.retain(|(owner, _)| match owner {
                    Owner::Asset(parent) => desired.contains_key(&(*id, *parent)) || kept.contains(parent),
                    _ => true,
                });
            }
        }
        for ((dependency, parent), (asked, asset_type)) in desired {
            // Under the control, the dependency's entry takes its parent's type.
            let parents_type = self.entries.get(&parent).map(|entry| entry.asset_type).filter(|_| rules.dependency_takes_its_parents_type);
            self.want_dependency(dependency, parents_type.unwrap_or(asset_type), parent, &asked);
        }
    }

    /// A want the core adds for a dependency: owned by the parent, with the
    /// policies asked. It states a type only where it is the dependency's
    /// first, and is never refused. Where it is added on a failed entry, it
    /// is the new load that returns the entry to unavailable; where the
    /// parent already held it, only its policy follows.
    fn want_dependency(&mut self, dependency: AssetId, asset_type: AssetType, parent: AssetId, asked: &[Policy]) {
        let rules = self.rules;
        let owner = Owner::Asset(parent);
        // The control that refuses a want of the core for its type.
        if rules.core_wants_are_refused_for_their_type && self.entries.get(&dependency).is_some_and(|entry| entry.asset_type != asset_type) {
            return;
        }
        let entry = self.entries.entry(dependency).or_insert_with(|| Entry::unavailable(asset_type));
        let held: Vec<Policy> = entry.wants.iter().filter(|(other, _)| *other == owner).map(|(_, policy)| *policy).collect();
        if held == asked {
            return;
        }
        entry.wants.retain(|(other, _)| *other != owner);
        let at = rules.place(&entry.wants, &owner);
        for (k, policy) in asked.iter().enumerate() {
            entry.wants.insert(at + k, (owner, *policy));
        }
        let left_failed = rules.core_want_leaves_a_failed_entry_failed && matches!(entry.availability, Availability::Failed(_));
        if held.is_empty() && !left_failed && entry.unanswered_for_a_new_want(&rules) {
            self.core_added.push((dependency, parent));
        }
    }

    fn take_core_added(&mut self, leaving: &mut Leaving) {
        leaving.extend(self.core_added.drain(..).map(|(dependency, parent)| (dependency, Owner::Asset(parent))));
    }

    fn apply_arrival(&mut self, arrival: &Arrival, residents: &dyn Residents, applied: &mut Applied) {
        let rules = self.rules;
        applied.answered_a_restore |= arrival.marked;
        self.awaiting.remove(&arrival.asset);
        let Some(entry) = self.entries.get_mut(&arrival.asset) else {
            applied.unnamed_arrivals += 1;
            return;
        };
        let before = entry.availability;
        // An arrival that leaves an entry as it was: at the same hash, or
        // failed for the same reason.
        let as_it_was = match (before, arrival.outcome) {
            (Availability::Available(held), Outcome::Hash(hash)) => held == hash,
            (Availability::Failed(held), Outcome::Failure(code)) => held == code,
            _ => false,
        };
        let named = !entry.wants.is_empty();
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
                // An answer to a restore at a hash is no arrival and no reload: no event.
                if (as_it_was && !rules.unchanged_arrival_emits) || arrival.marked {
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
                entry.availability = Availability::Failed(code);
                entry.resident = None;
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
        match *command {
            Command::Want { asset, asset_type, owner, policy } if adding => {
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
                let mut ended = false;
                for entry in self.entries.values_mut() {
                    let before = entry.wants.len();
                    entry.wants.retain(|(owner, _)| *owner != Owner::Entity { slot, generation });
                    ended |= entry.wants.len() != before;
                }
                ended
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
                true
            }
        }
    }

    /// The batch of a record's arrivals: the wants its parts list first, to
    /// any depth, then the arrivals in identifier order, then the wants of
    /// dependencies recomputed. A want the core added with the batch leaves
    /// the domain where no arrival of the batch answers it.
    fn apply_arrivals(&mut self, record: &Record, residents: &dyn Residents, purpose: Purpose, applied: &mut Applied, leaving: &mut Leaving) {
        if record.arrivals.is_empty() {
            return;
        }
        let rules = self.rules;
        let counts_core_wants = !(purpose == Purpose::Gate && rules.gate_leaves_out_core_wants);
        // Under one control of the gate the wants of the pending parts were
        // added before the pending releases, and follow nothing after.
        let added_early = purpose == Purpose::Gate && rules.gate_leaves_out_releases_for_core_wants;
        let mut brought = Leaving::new();
        if counts_core_wants && !added_early {
            self.want_the_dependencies_of(record, residents, rules.former_dependency_wants_end_before_the_batch);
            self.take_core_added(&mut brought);
        }
        // An arrival to a hash is tested against the world's budget where it
        // is applied: after the commands of its record, after the wants the
        // parts of the batch list are added, and before the falls.
        let rejected = self.over_budget(&record.arrivals, residents);
        for arrival in &record.arrivals {
            match arrival.outcome {
                Outcome::Hash(hash) if rejected.contains(&arrival.asset) => self.reject(arrival, hash, residents, applied),
                _ => self.apply_arrival(arrival, residents, applied),
            }
            if rules.core_wants_recomputed_at_each_arrival && counts_core_wants && !rules.core_wants_left {
                self.end_wants_no_longer_listed_by(arrival.asset);
            }
        }
        if counts_core_wants && !rules.core_wants_left && !added_early {
            self.dependency_wants(&BTreeMap::new(), true);
            self.take_core_added(&mut brought);
        }
        let answered = |asset: &AssetId| record.arrivals.iter().any(|arrival| arrival.asset == *asset);
        leaving.extend(brought.into_iter().filter(|(asset, _)| !answered(asset)));
    }

    /// The first half of a batch: the wants its parts list, to any depth.
    /// With `prune`, a control, the wants a new part no longer lists end here,
    /// before any arrival of the batch is applied.
    fn want_the_dependencies_of(&mut self, record: &Record, residents: &dyn Residents, prune: bool) {
        let mut arriving = BTreeMap::new();
        for arrival in &record.arrivals {
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
        self.dependency_wants(&arriving, prune);
    }

    /// The arrivals of a record that the world's budget rejects, under the
    /// candidate unit the rules name. The count of a world is the sum of the
    /// sizes of the canonical parts its table holds available, an entry that
    /// holds no want included. The budget is exceeded where the count would
    /// stand above it. An arrival at another hash of an asset the table holds
    /// available counts the new size in place of the former one. An arrival
    /// to failed has no size and is not tested; an arrival that would change
    /// nothing, for an asset no want names or at the hash the entry holds,
    /// adds nothing and is not tested either.
    fn over_budget(&self, arrivals: &[Arrival], residents: &dyn Residents) -> BTreeSet<AssetId> {
        let mut rejected = BTreeSet::new();
        let Some(budget) = self.budget else { return rejected };
        let size = |resident: &Arc<Resident>| resident.bytes.len() as u64;
        let mut count: u64 = self.entries.values().filter_map(|entry| entry.resident.as_ref()).map(size).sum();
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
            let added = residents.get(hash).as_ref().map_or(0, size);
            let replaced = entry.and_then(|entry| entry.resident.as_ref()).map_or(0, size);
            let root = match self.rules.budget_unit {
                BudgetUnit::SingleArrival => arrival.asset,
                BudgetUnit::ClosureOfAWant => arrival.root.unwrap_or(arrival.asset),
                BudgetUnit::WholeDrain => 0,
            };
            tested.push((arrival.asset, root, added, replaced));
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

    /// A rejected arrival is not applied, and the result of the step names
    /// it. Where the table holds the asset available, the entry stays as it
    /// was and no event is emitted. Where it does not, the entry becomes
    /// failed for the reason of the budget, as by a failed arrival, with the
    /// event of a failure. Under the control that only reports, the arrival
    /// is named and applied all the same.
    fn reject(&mut self, arrival: &Arrival, hash: u64, residents: &dyn Residents, applied: &mut Applied) {
        applied.rejected.push((arrival.asset, hash));
        if self.rules.budget_only_reports {
            return self.apply_arrival(arrival, residents, applied);
        }
        self.awaiting.remove(&arrival.asset);
        let Some(entry) = self.entries.get_mut(&arrival.asset).filter(|entry| !entry.wants.is_empty()) else { return };
        if matches!(entry.availability, Availability::Available(_)) {
            return;
        }
        entry.availability = Availability::Failed(REASON_OVER_BUDGET);
        entry.resident = None;
        applied.events.push(Event::Failed { asset: arrival.asset, code: REASON_OVER_BUDGET });
        if entry.policy() == Some(Policy::Required) {
            applied.failed_required.push((arrival.asset, REASON_OVER_BUDGET));
        }
    }

    /// Under the reading that recomputes at a parent's own arrival: the
    /// wants `parent` owns on assets its part in force does not list end.
    fn end_wants_no_longer_listed_by(&mut self, parent: AssetId) {
        let Some(entry) = self.entries.get(&parent) else { return };
        if matches!(entry.availability, Availability::Failed(_)) && self.rules.core_wants_kept_under_a_failed_parent {
            return;
        }
        let listed: Vec<AssetId> = entry.resident.as_ref().map_or_else(Vec::new, |resident| resident.part.dependencies.iter().map(|(id, _)| *id).collect());
        for (id, entry) in self.entries.iter_mut() {
            entry.wants.retain(|(owner, _)| *owner != Owner::Asset(parent) || listed.contains(id));
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
        self.core_added.clear();
        // The entries whose last want ended during the tick before.
        let candidates: Vec<AssetId> = self.entries.iter().filter(|(_, entry)| entry.wants.is_empty()).map(|(id, _)| *id).collect();
        if self.rules.falls_before_record {
            applied.fallen = self.fall(&candidates, &BTreeSet::new());
        }
        // A candidate a want names while the record is applied did not end
        // its last want during the tick before, even if that want is ended
        // again by a later element of the same record.
        let mut wanted_since = BTreeSet::new();
        let mut leaving = Leaving::new();
        if self.rules.arrivals_before_commands {
            self.apply_arrivals(record, residents, purpose, &mut applied, &mut leaving);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        // One control of the gate: the wants of the pending parts are added
        // with the wants the table holds, before the pending releases.
        let added_early = purpose == Purpose::Gate && self.rules.gate_leaves_out_releases_for_core_wants;
        if added_early && !self.rules.gate_leaves_out_core_wants {
            self.want_the_dependencies_of(record, residents, false);
        }
        self.apply_commands(&record.commands, purpose, alive, !added_early, &mut applied, &mut leaving, &candidates, &mut wanted_since);
        if !self.rules.arrivals_before_commands {
            self.apply_arrivals(record, residents, purpose, &mut applied, &mut leaving);
            self.note_wanted(&candidates, &mut wanted_since);
        }
        if !self.rules.falls_before_record {
            applied.fallen = self.fall(&candidates, &wanted_since);
        }
        self.finish(&mut applied, leaving);
        applied
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
    ) {
        let follow = dependencies_follow && !self.rules.dependency_wants_with_a_batch_alone;
        // The copy the gate is evaluated on needs the wants of dependencies
        // once, after the last pending command: nothing reads them between.
        let once = purpose == Purpose::Gate;
        let mut changed = false;
        for (index, command) in commands.iter().enumerate() {
            if self.apply_command(index, command, purpose, alive, applied, leaving) {
                changed = true;
                if follow && !once {
                    self.dependency_wants(&BTreeMap::new(), true);
                    self.take_core_added(leaving);
                }
            }
            self.note_wanted(candidates, wanted_since);
        }
        if follow && once && changed {
            self.dependency_wants(&BTreeMap::new(), true);
            self.take_core_added(leaving);
        }
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
        if self.rules.unrisen_entry_leaves_at_once {
            self.entries.retain(|_, entry| !entry.wants.is_empty() || matches!(entry.availability, Availability::Available(_)));
        }
        self.refresh();
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
        self.apply_commands(commands, Purpose::Tick, alive, true, &mut applied, &mut leaving, &[], &mut BTreeSet::new());
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
    /// by a pending arrival, a pending command that releases a required want
    /// counting as its answer. A pending command that adds a want is left
    /// out, and a pending despawn of an owner is not seen. A want the core
    /// would add from the part of a pending arrival is counted, with the
    /// policy the wants the table holds give its parent.
    pub fn gate_open(&self, record: &Record, residents: &dyn Residents) -> bool {
        self.gate(record, residents).is_empty()
    }

    /// What keeps the gate closed once the pending record is counted: the
    /// assets still unanswered, none where the gate is open.
    pub fn gate(&self, record: &Record, residents: &dyn Residents) -> Vec<AssetId> {
        self.gate_in_two_parts(record, residents).0
    }

    /// The same evaluation, with the time of its two parts, for the
    /// measurement of its cost: the copy of the table, then the pending
    /// record applied on the copy and the verdict read from it. Both are
    /// zero where no copy is made.
    pub fn gate_in_two_parts(&self, record: &Record, residents: &dyn Residents) -> (Vec<AssetId>, Duration, Duration) {
        // A world that is not gated steps: its pending record is not read.
        if !self.blocked && !self.rules.gate_read_on_a_world_that_is_not_gated {
            return (Vec::new(), Duration::ZERO, Duration::ZERO);
        }
        if record.is_empty() {
            return (self.required_unanswered(), Duration::ZERO, Duration::ZERO);
        }
        // How the table the gate is evaluated on is obtained: a copy.
        let started = Instant::now();
        let mut after = self.clone();
        let copied = started.elapsed();
        after.evaluating_the_gate = true;
        after.apply_for(record, residents, Purpose::Gate, &|_, _| true);
        let awaited = after.required_unanswered();
        (awaited, copied, started.elapsed() - copied)
    }

    /// Recomputes what is derived from the entries. Called after any change.
    pub fn refresh(&mut self) {
        self.orphans = self.entries.values().any(|entry| entry.wants.is_empty());
        self.blocked = !self.required_unanswered().is_empty();
        let owned_by_entity = |(owner, _): &&(Owner, Policy)| matches!(owner, Owner::Entity { .. });
        self.entity_wants = self.entries.values().map(|entry| entry.wants.iter().filter(owned_by_entity).count()).sum();
    }

    /// The canonical bytes of the table: per asset in identifier order, its
    /// asset type as the stable name of the type's schema entry, its wants
    /// in their total order, and its availability with the hash or the
    /// reason code; then the instance records in creation order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        if !self.rules.type_left_out_of_the_bytes {
            return self.typed_bytes();
        }
        // The control: the bytes the table had before an entry held a type.
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, entry) in &self.entries {
            out.extend_from_slice(&id.to_le_bytes());
            put_wants_and_availability(&mut out, entry.availability, &entry.wants);
        }
        put_instances(&mut out, &self.instances);
        out
    }

    /// The canonical bytes with the type of every entry, whatever the rules:
    /// what a save holds and what `decode` reads back.
    pub fn typed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (id, entry) in &self.entries {
            put_entry(&mut out, *id, entry.asset_type, entry.availability, &entry.wants);
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
            let availability = match tag {
                0 => Availability::Unavailable,
                1 => Availability::Available(value),
                2 => Availability::Failed(value as u32),
                other => return Err(format!("availability tag {other}")),
            };
            table.entries.insert(id, Entry { asset_type, wants, availability, resident: None });
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
/// availability. Public so that a harness can write a table stated by hand
/// through the same bytes.
pub fn put_entry(out: &mut Vec<u8>, id: AssetId, asset_type: AssetType, availability: Availability, wants: &[(Owner, Policy)]) {
    out.extend_from_slice(&id.to_le_bytes());
    put_type(out, asset_type);
    put_wants_and_availability(out, availability, wants);
}

fn put_wants_and_availability(out: &mut Vec<u8>, availability: Availability, wants: &[(Owner, Policy)]) {
    out.extend_from_slice(&(wants.len() as u32).to_le_bytes());
    for (owner, policy) in wants {
        put_owner(out, *owner);
        put_policy(out, *policy);
    }
    let (tag, value) = match availability {
        Availability::Unavailable => (0_u8, 0_u64),
        Availability::Available(hash) => (1, hash),
        Availability::Failed(code) => (2, u64::from(code)),
    };
    out.push(tag);
    out.extend_from_slice(&value.to_le_bytes());
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
