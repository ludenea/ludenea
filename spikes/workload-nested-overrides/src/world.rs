//! The world: a trimmed copy of the scene streaming spike's world (itself a
//! trimmed copy of the runtime skeleton spike's core: whole ticks on the
//! dependency-counting tick graph, double buffered, the hash of a tick
//! overlapping the next tick's passes) with the grid of cells replaced by
//! families of nested instances. Throwaway, never product code.
//!
//! A tick runs the spatial hashing (one task), the reference system (chunks,
//! beside the spatial hashing), the repulsion pass (chunks), the motion pass
//! (chunks), then the structural node alone: the lagging buffer learns the
//! last structural changes, the instantiation reads the records in the
//! canonical order of their identities and unloads and spawns whole
//! families, the buffers swap, and the input fixed point of the next tick
//! applies the scripted commands and the arrivals and decides whether the
//! loading gate holds the world. The two hashes of a tick run beside the
//! next tick's passes.

// The fixture's one unsafe construct, the raw column views split into disjoint
// chunk slices, with the graph's dependencies as the justification (see
// `Column` and the `Sync` implementation of `Graph`).
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fmt;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use reference_workload::{chunk_range, integrated, kinetic_energy, repelled_velocity};

use crate::scene::{ApplyError, INSTANCING, Override, Payload, Prepared, Proto, REFERENCE, Store, descend, effective, lists_hash, own_items};
use crate::state::{
    Actual, DYNAMIC, Event, FailReason, InstancePath, Logged, NO_REFERENCE, NONE, ReadCounts, Record, SpatialHash, State, Substrate, apply_in_world, bounced_in, content_hash, repulsion_force,
    resolve, runtime_id, slot_of,
};

pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);
/// The side of the box a top-level family is anchored in, and the world's.
pub const FAMILY_SIDE: f32 = 256.0;
pub const WORLD_SIDE: f32 = 1024.0;
/// The scripted loader delivers a payload this many polls after its request
/// unless the script says otherwise.
pub const DEFAULT_DELAY: u32 = 2;

const NODE_SPATIAL: usize = 0;
const NODE_REFERENCES: usize = 1;
const NODE_REPULSION: usize = 2;
const NODE_MOTION: usize = 3;
const NODE_STRUCTURAL: usize = 4;
const NODE_HASH: usize = 5;
const NODE_COUNT: usize = 6;
pub const NODE_NAMES: [&str; NODE_COUNT] = ["spatial_hashing", "references", "repulsion", "motion", "structural", "hash"];
const CLAIM_ORDER: [usize; NODE_COUNT] = [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_REFERENCES, NODE_STRUCTURAL];

/// Where the overrides of an instance are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Application {
    /// To the payload, outside the tick: the members are prepared before the
    /// run, keyed by two content hashes, and the spawn writes them.
    Prepared,
    /// In the stated order after a spawn into a staging area, whose members
    /// then enter the world in the order of their local identifiers.
    Staged,
    /// In the stated order after a spawn into the world itself, as
    /// structural and field operations on its entities.
    InPlace,
}

impl Application {
    pub const ALL: [Application; 3] = [Application::Prepared, Application::Staged, Application::InPlace];

    pub fn name(self) -> &'static str {
        match self {
            Application::Prepared => "payload_prepared",
            Application::Staged => "ordered_in_staging",
            Application::InPlace => "ordered_in_world",
        }
    }
}

/// A scripted command, applied at the input fixed point of its tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Load { root: u64, asset: u64, required: bool },
    Unload { root: u64 },
}

#[derive(Clone, Debug)]
pub struct WorldConfig {
    pub script: Vec<(u32, Command)>,
    /// The polls between the request of a scene and its arrival, per scene.
    pub delays: BTreeMap<u64, u32>,
    pub application: Application,
    pub workers: usize,
    pub spin_bound: Duration,
}

impl WorldConfig {
    pub fn new(script: Vec<(u32, Command)>, delays: BTreeMap<u64, u32>, application: Application, workers: usize) -> WorldConfig {
        assert!(workers >= 1, "a world is stepped by at least the calling thread");
        assert!(script.windows(2).all(|pair| pair[0].0 <= pair[1].0) && script.first().is_none_or(|first| first.0 >= 1), "the script is in tick order from tick 1");
        WorldConfig { script, delays, application, workers, spin_bound: DEFAULT_SPIN_BOUND }
    }
}

/// The origin of a top-level family: its root key read as a cell of the world.
pub fn root_origin(root: u64) -> (f32, f32) {
    ((root % 4) as f32 * FAMILY_SIDE, ((root / 4) % 4) as f32 * FAMILY_SIDE)
}

/// A programming error caught at the task boundary, with its context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub tick: u32,
    pub node: &'static str,
    pub chunk: usize,
    pub worker: usize,
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "world failed at tick {} in {} chunk {} on worker {}: {}", self.tick, self.node, self.chunk, self.worker, self.message)
    }
}

impl std::error::Error for Failure {}

// ---------------------------------------------------------------------------
// The instance records
// ---------------------------------------------------------------------------

/// The record of one instance: it exists for every instance loaded or
/// referred to, and outlives its members.
#[derive(Clone, Debug)]
pub struct InstanceRecord {
    pub path: InstancePath,
    pub asset: Option<u64>,
    pub wanted: bool,
    pub required: bool,
    pub actual: Actual,
    pub parent: u32,
    pub origin: (f32, f32),
    /// The content hash the parent's payload names for this child.
    pub expected_hash: Option<u64>,
    /// The lists of the scenes above, inner first, their paths relative to
    /// this instance.
    pub inherited: Vec<Vec<Override>>,
    /// The table of the instance: local identifier to runtime identifier,
    /// sorted by local identifier.
    pub members: Vec<(u64, u64)>,
}

#[derive(Debug, Default)]
pub struct Instances {
    pub records: Vec<InstanceRecord>,
    pub digests: Vec<u64>,
    /// The records in the canonical order of their identities.
    pub index: BTreeMap<InstancePath, u32>,
    pub available: BTreeMap<u64, Arc<Payload>>,
    requested: BTreeSet<u64>,
    due: BTreeSet<(u64, u64)>,
    poll: u64,
    cursor: usize,
    input_done: u32,
    pub log: Vec<Logged>,
    pub events: Vec<Event>,
    /// Polls at which the loading gate held the world.
    pub gate_polls: u64,
    /// Ticks completed with a required, wanted instance neither loaded nor failed.
    pub required_missing: u64,
    /// Spawns whose members were not prepared before the run.
    pub prepared_misses: u64,
}

impl Instances {
    /// The record of an identity, created absent when there is none.
    fn record_of(&mut self, path: &InstancePath) -> u32 {
        if let Some(index) = self.index.get(path) {
            return *index;
        }
        let index = self.records.len() as u32;
        self.records.push(InstanceRecord {
            path: path.clone(),
            asset: None,
            wanted: false,
            required: false,
            actual: Actual::Absent,
            parent: NONE,
            origin: (0.0, 0.0),
            expected_hash: None,
            inherited: Vec::new(),
            members: Vec::new(),
        });
        self.digests.push(path.digest());
        self.index.insert(path.clone(), index);
        index
    }

    /// The record of the instance a relative reference names: a record
    /// exists for every instance referred to.
    fn target_record(&mut self, own: u32, ups: u64, down: u64) -> u32 {
        match self.records[own as usize].path.relative(ups, down) {
            Some(path) => self.record_of(&path),
            None => NONE,
        }
    }

    pub fn loaded(&self) -> impl Iterator<Item = &InstanceRecord> {
        self.index.values().map(|index| &self.records[*index as usize]).filter(|record| record.actual == Actual::Loaded)
    }
}

// ---------------------------------------------------------------------------
// Raw column views (the fixture's one unsafe construct)
// ---------------------------------------------------------------------------

/// A raw view of one column, taken while the column's owner is exclusively
/// borrowed and re-taken whenever the structural node, which runs alone,
/// resizes it. Chunks write disjoint slices of it without forming a
/// reference to the whole column.
#[derive(Clone, Copy)]
struct Column<T> {
    ptr: *mut T,
    len: usize,
}

unsafe impl<T: Send> Send for Column<T> {}
unsafe impl<T: Send> Sync for Column<T> {}

impl<T: Copy> Column<T> {
    fn of(values: &mut [T]) -> Column<T> {
        Column { ptr: values.as_mut_ptr(), len: values.len() }
    }

    /// # Safety
    ///
    /// No other thread reads or writes the slots of `range` in this column
    /// while the returned slice lives, and the column was not resized since
    /// the view was taken.
    #[allow(clippy::mut_from_ref)]
    unsafe fn slice(&self, range: Range<usize>) -> &mut [T] {
        assert!(range.start <= range.end && range.end <= self.len, "chunk slice out of bounds");
        // SAFETY: in bounds by the assertion; disjointness is the caller's.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(range.start), range.len()) }
    }
}

#[derive(Clone, Copy)]
struct BufferViews {
    x: Column<f32>,
    y: Column<f32>,
    vx: Column<f32>,
    vy: Column<f32>,
    observed: Column<u64>,
}

impl BufferViews {
    fn of(state: &mut State) -> BufferViews {
        BufferViews {
            x: Column::of(&mut state.x),
            y: Column::of(&mut state.y),
            vx: Column::of(&mut state.vx),
            vy: Column::of(&mut state.vy),
            observed: Column::of(&mut state.observed),
        }
    }
}

#[derive(Clone, Copy)]
struct Views {
    buffers: [BufferViews; 2],
    scratch: Column<f32>,
}

// ---------------------------------------------------------------------------
// The tick graph
// ---------------------------------------------------------------------------

struct GraphNode {
    chunks: AtomicUsize,
    next: AtomicUsize,
    finished: AtomicUsize,
    pending: AtomicUsize,
}

impl GraphNode {
    fn new(chunks: usize) -> GraphNode {
        GraphNode { chunks: AtomicUsize::new(chunks), next: AtomicUsize::new(chunks), finished: AtomicUsize::new(0), pending: AtomicUsize::new(0) }
    }
}

struct Graph {
    config: WorldConfig,
    store: Arc<Store>,
    prepared: Arc<Prepared>,
    created: Instant,
    states: [UnsafeCell<State>; 2],
    views: UnsafeCell<Views>,
    scratch: UnsafeCell<Vec<f32>>,
    spatial: UnsafeCell<SpatialHash>,
    /// The free slots, lowest first.
    free: UnsafeCell<BinaryHeap<Reverse<usize>>>,
    /// The slots whose static columns the last structural changes touched.
    changed: UnsafeCell<Vec<usize>>,
    instances: UnsafeCell<Instances>,
    reads: [AtomicU64; 3],
    current: AtomicUsize,
    nodes: [GraphNode; NODE_COUNT],
    ready: AtomicU32,
    tick: AtomicU32,
    hash_tick: AtomicU32,
    end: AtomicU32,
    /// The loading gate holds the world before its next tick.
    gated: AtomicBool,
    done: AtomicBool,
    failed: AtomicBool,
    failure: Mutex<Option<Failure>>,
    tick_hashes: Vec<AtomicU64>,
    content_hashes: Vec<AtomicU64>,
    tick_start_ns: Vec<AtomicU64>,
    tick_end_ns: Vec<AtomicU64>,
    changes_per_tick_ns: Vec<AtomicU64>,
    spawned_per_tick: Vec<AtomicU64>,
    despawned_per_tick: Vec<AtomicU64>,
    loaded_per_tick: Vec<AtomicU64>,
    phase_ns: [AtomicU64; NODE_COUNT],
    changes_ns: AtomicU64,
    spawn_ns: AtomicU64,
    spawned_entities: AtomicU64,
    despawned_entities: AtomicU64,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise; the structural node runs alone and is the only writer of
// the instance records, their tables, the free list, the scratch's size and
// the views; the reference chunks and the hash read the records only.
unsafe impl Sync for Graph {}

/// One structural fixed point's instantiation: the unloads, then the spawns
/// of whole families, in the canonical order of the records.
struct Batch<'a> {
    graph: &'a Graph,
    tick: u32,
    cur: &'a mut State,
    next: &'a mut State,
    changed: &'a mut Vec<usize>,
    free: &'a mut BinaryHeap<Reverse<usize>>,
    instances: &'a mut Instances,
    spawned: u64,
    despawned: u64,
    spawn_ns: u64,
}

impl Batch<'_> {
    fn substrate(&mut self) -> Substrate<'_> {
        Substrate { state: &mut *self.next, lagging: Some(&mut *self.cur), free: &mut *self.free, changed: Some(&mut *self.changed) }
    }

    fn run(&mut self) {
        let order: Vec<u32> = self.instances.index.values().copied().collect();
        for &index in &order {
            let record = &self.instances.records[index as usize];
            if record.parent == NONE && !record.wanted && record.actual != Actual::Absent {
                self.unload(index);
            }
        }
        for &index in &order {
            self.spawn(index);
        }
        self.instances.required_missing += self.instances.records.iter().filter(|record| record.wanted && record.required && !matches!(record.actual, Actual::Loaded | Actual::Failed(_))).count() as u64;
    }

    /// Despawns the members of an instance and of every instance nested
    /// below it, the nested instances first (the reverse of the canonical
    /// order), each instance's members in the order of their local
    /// identifiers; the records stay, absent.
    fn unload(&mut self, index: u32) {
        let root = self.instances.records[index as usize].path.clone();
        let subtree: Vec<u32> = self.instances.index.range(root.clone()..).take_while(|(path, _)| path.starts_with(&root)).map(|(_, index)| *index).collect();
        for &nested in subtree.iter().rev() {
            let record = &mut self.instances.records[nested as usize];
            let was_loaded = record.actual == Actual::Loaded;
            let members = std::mem::take(&mut record.members);
            record.actual = Actual::Absent;
            if nested != index {
                record.wanted = false;
            }
            if was_loaded {
                let path = record.path.clone();
                let mut substrate = self.substrate();
                for (_, id) in &members {
                    substrate.release(slot_of(*id));
                }
                self.despawned += members.len() as u64;
                self.instances.events.push(Event::Despawned { tick: self.tick, path, members: members.len() as u32 });
            }
        }
    }

    fn fail(&mut self, index: u32, reason: FailReason) {
        let record = &mut self.instances.records[index as usize];
        record.actual = Actual::Failed(reason);
        let path = record.path.clone();
        self.instances.events.push(Event::Failed { tick: self.tick, path, reason });
    }

    /// Spawns a wanted instance whose payload is available and, in the same
    /// batch, every instance nested below it whose payload is available: a
    /// family appears at one fixed point whatever its depth.
    fn spawn(&mut self, index: u32) {
        let record = &self.instances.records[index as usize];
        if !record.wanted || matches!(record.actual, Actual::Loaded | Actual::Failed(_)) {
            return;
        }
        let Some(asset) = record.asset else { return };
        if record.parent != NONE && self.instances.records[record.parent as usize].actual != Actual::Loaded {
            return;
        }
        let Some(payload) = self.instances.available.get(&asset).cloned() else {
            self.instances.records[index as usize].actual = Actual::Requested;
            return;
        };
        if record.expected_hash.is_some_and(|hash| hash != payload.content_hash) {
            // A child at another content hash than the one its parent's
            // payload names is not spawned.
            self.fail(index, FailReason::AnotherContentHash);
            return;
        }
        let (path, origin, required, inherited) = (record.path.clone(), record.origin, record.required, record.inherited.clone());
        let lists: Vec<Vec<Override>> = inherited.iter().map(|list| own_items(list)).collect();
        let started = Instant::now();
        let members = match self.graph.config.application {
            Application::Prepared => self.spawn_prepared(index, &payload, &lists, origin),
            Application::Staged => self.spawn_staged(index, &payload, &lists, origin),
            Application::InPlace => self.spawn_in_place(index, &payload, &lists, origin),
        };
        let Ok(members) = members else {
            self.fail(index, FailReason::OverrideRefused);
            return;
        };
        self.spawn_ns += started.elapsed().as_nanos() as u64;
        let digest = self.next.members_digest(&members, &self.instances.digests);
        let children: Vec<(u64, u64, f32, f32)> = members
            .iter()
            .map(|(local, id)| (*local, slot_of(*id)))
            .filter(|(_, slot)| self.next.has(*slot, INSTANCING))
            .map(|(local, slot)| (local, self.next.asset[slot], self.next.anchor_x[slot], self.next.anchor_y[slot]))
            .collect();
        self.spawned += members.len() as u64;
        self.instances.events.push(Event::Spawned { tick: self.tick, path: path.clone(), members: members.len() as u32, digest });
        let record = &mut self.instances.records[index as usize];
        record.members = members;
        record.actual = Actual::Loaded;
        for (entity, child_asset, anchor_x, anchor_y) in children {
            let Some(nested) = payload.nested_of(entity) else { continue };
            let child = self.instances.record_of(&path.child(entity));
            let record = &mut self.instances.records[child as usize];
            record.asset = Some(child_asset);
            record.parent = index;
            record.origin = (origin.0 + anchor_x, origin.1 + anchor_y);
            record.expected_hash = Some(nested.child_hash);
            record.inherited = descend(&nested.overrides, &inherited, entity);
            record.wanted = true;
            record.required = required;
            self.spawn(child);
        }
    }

    /// Writes prototypes as members of a record, in their order, each into
    /// the lowest free slot.
    fn write_members(&mut self, index: u32, protos: &[Proto], origin: (f32, f32)) -> Vec<(u64, u64)> {
        let mut members = Vec::with_capacity(protos.len());
        for proto in protos {
            let reference = if proto.has(REFERENCE) { self.instances.target_record(index, proto.get(REFERENCE, 0), proto.get(REFERENCE, 1)) } else { NONE };
            let slot = self.substrate().allocate();
            self.next.write_proto(slot, index, proto, origin, reference);
            members.push((proto.local, runtime_id(slot, self.next.generation[slot])));
        }
        members
    }

    fn spawn_prepared(&mut self, index: u32, payload: &Payload, lists: &[Vec<Override>], origin: (f32, f32)) -> Result<Vec<(u64, u64)>, ApplyError> {
        let key = (payload.content_hash, lists_hash(lists));
        let protos = match self.graph.prepared.get(key) {
            Some(protos) => protos,
            None => {
                self.instances.prepared_misses += 1;
                self.graph.prepared.insert(key, effective(payload, lists)?)
            }
        };
        Ok(self.write_members(index, &protos, origin))
    }

    fn spawn_staged(&mut self, index: u32, payload: &Payload, lists: &[Vec<Override>], origin: (f32, f32)) -> Result<Vec<(u64, u64)>, ApplyError> {
        let mut staging = State::default();
        let mut free = BinaryHeap::new();
        let mut staged = Vec::with_capacity(payload.protos.len());
        let mut substrate = Substrate { state: &mut staging, lagging: None, free: &mut free, changed: None };
        for proto in &payload.protos {
            let slot = substrate.allocate();
            substrate.state.write_proto(slot, NONE, proto, (0.0, 0.0), NONE);
            staged.push((proto.local, runtime_id(slot, 0)));
        }
        for list in lists {
            apply_in_world(&mut substrate, &mut staged, NONE, (0.0, 0.0), list, &mut |_, _| NONE)?;
        }
        let protos: Vec<Proto> = staged.iter().map(|(_, id)| staging.read_proto(slot_of(*id))).collect();
        Ok(self.write_members(index, &protos, origin))
    }

    fn spawn_in_place(&mut self, index: u32, payload: &Payload, lists: &[Vec<Override>], origin: (f32, f32)) -> Result<Vec<(u64, u64)>, ApplyError> {
        let mut members = self.write_members(index, &payload.protos, origin);
        for list in lists {
            let instances = &mut *self.instances;
            let mut target_record = |ups: u64, down: u64| instances.target_record(index, ups, down);
            let mut substrate = Substrate { state: &mut *self.next, lagging: Some(&mut *self.cur), free: &mut *self.free, changed: Some(&mut *self.changed) };
            if let Err(refused) = apply_in_world(&mut substrate, &mut members, index, origin, list, &mut target_record) {
                for (_, id) in &members {
                    substrate.release(slot_of(*id));
                }
                return Err(refused);
            }
        }
        Ok(members)
    }
}

impl Graph {
    fn new(config: WorldConfig, store: Arc<Store>, prepared: Arc<Prepared>) -> Graph {
        let mut a = State::default();
        let mut b = State::default();
        let mut scratch: Vec<f32> = Vec::new();
        let views = Views { buffers: [BufferViews::of(&mut a), BufferViews::of(&mut b)], scratch: Column::of(&mut scratch) };
        let instances = Instances::default();
        let initial_hash = a.hash(&instances.digests);
        let initial_content = content_hash(&a, &instances.digests, std::iter::empty());
        let workers = config.workers;
        Graph {
            config,
            store,
            prepared,
            created: Instant::now(),
            states: [UnsafeCell::new(a), UnsafeCell::new(b)],
            views: UnsafeCell::new(views),
            scratch: UnsafeCell::new(scratch),
            spatial: UnsafeCell::new(SpatialHash::new(WORLD_SIDE)),
            free: UnsafeCell::new(BinaryHeap::new()),
            changed: UnsafeCell::new(Vec::new()),
            instances: UnsafeCell::new(instances),
            reads: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            current: AtomicUsize::new(0),
            nodes: [GraphNode::new(1), GraphNode::new(0), GraphNode::new(0), GraphNode::new(0), GraphNode::new(1), GraphNode::new(1)],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(0),
            hash_tick: AtomicU32::new(0),
            end: AtomicU32::new(0),
            gated: AtomicBool::new(false),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            tick_hashes: vec![AtomicU64::new(initial_hash)],
            content_hashes: vec![AtomicU64::new(initial_content)],
            tick_start_ns: vec![AtomicU64::new(0)],
            tick_end_ns: vec![AtomicU64::new(0)],
            changes_per_tick_ns: vec![AtomicU64::new(0)],
            spawned_per_tick: vec![AtomicU64::new(0)],
            despawned_per_tick: vec![AtomicU64::new(0)],
            loaded_per_tick: vec![AtomicU64::new(0)],
            phase_ns: Default::default(),
            changes_ns: AtomicU64::new(0),
            spawn_ns: AtomicU64::new(0),
            spawned_entities: AtomicU64::new(0),
            despawned_entities: AtomicU64::new(0),
            parked: (0..workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
        }
    }

    // --- the input fixed point -----------------------------------------------

    /// One poll of the input fixed point for `tick`: on the first poll the
    /// scripted commands of the tick are applied and logged; on every poll
    /// the arrivals that are due are applied and logged, each asking for the
    /// scenes its payload nests; then the loading gate is decided. Runs
    /// alone: in the structural node, or on the calling thread between steps.
    fn input_fixed_point(&self, tick: u32) {
        // SAFETY: the fixed point runs alone (see the type's documentation).
        let instances = unsafe { &mut *self.instances.get() };
        instances.poll += 1;
        if instances.input_done != tick {
            instances.input_done = tick;
            while let Some((at, command)) = self.config.script.get(instances.cursor) {
                if *at > tick {
                    break;
                }
                instances.cursor += 1;
                self.apply_command(instances, tick, *command);
            }
        }
        while let Some(&(due, asset)) = instances.due.first() {
            if due > instances.poll {
                break;
            }
            instances.due.pop_first();
            let payload = self.store.current(asset).expect("a requested scene is in the store");
            instances.log.push(Logged { tick, record: Record::Arrival { asset, content_hash: payload.content_hash }, accepted: true });
            for nested in &payload.nested {
                self.request(instances, nested.asset);
            }
            instances.available.insert(asset, payload);
        }
        // The loading gate: a required family is in the world's required set
        // with everything it nests, known from the dependencies the import
        // recorded, before any record of a child exists.
        let gated = instances.records.iter().any(|record| {
            record.parent == NONE
                && record.wanted
                && record.required
                && !matches!(record.actual, Actual::Loaded | Actual::Failed(_))
                && record.asset.is_some_and(|asset| !instances.available.contains_key(&asset) || self.store.dependencies(asset).iter().any(|nested| !instances.available.contains_key(nested)))
        });
        if gated {
            instances.gate_polls += 1;
        }
        self.gated.store(gated, Ordering::SeqCst);
    }

    fn request(&self, instances: &mut Instances, asset: u64) {
        if !instances.available.contains_key(&asset) && instances.requested.insert(asset) {
            let delay = self.config.delays.get(&asset).copied().unwrap_or(DEFAULT_DELAY);
            instances.due.insert((instances.poll + u64::from(delay), asset));
        }
    }

    /// A command validated against the records as they stand: a load of a
    /// scene the store lacks, of an identity whose record names another
    /// scene, or of an instance already wanted, and an unload of an instance
    /// that is not wanted, change nothing and are logged as such.
    fn apply_command(&self, instances: &mut Instances, tick: u32, command: Command) {
        let (record, accepted) = match command {
            Command::Load { root, asset, required } => {
                let path = InstancePath::root(root);
                let existing = instances.index.get(&path).map(|index| &instances.records[*index as usize]);
                let accepted = self.store.current(asset).is_some() && existing.is_none_or(|record| !record.wanted && record.asset.is_none_or(|held| held == asset));
                if accepted {
                    let index = instances.record_of(&path);
                    let record = &mut instances.records[index as usize];
                    record.asset = Some(asset);
                    record.wanted = true;
                    record.required = required;
                    record.origin = root_origin(root);
                    self.request(instances, asset);
                    if required {
                        for nested in self.store.dependencies(asset) {
                            self.request(instances, nested);
                        }
                    }
                }
                (Record::Load { root, asset, required }, accepted)
            }
            Command::Unload { root } => {
                let wanted = instances.index.get(&InstancePath::root(root)).copied().filter(|index| instances.records[*index as usize].wanted);
                if let Some(index) = wanted {
                    instances.records[index as usize].wanted = false;
                }
                (Record::Unload { root }, wanted.is_some())
            }
        };
        instances.log.push(Logged { tick, record, accepted });
    }

    // --- the kernels ------------------------------------------------------

    fn current_state(&self) -> &State {
        // SAFETY: the current buffer is only read until the structural node runs alone.
        unsafe { &*self.states[self.current.load(Ordering::Relaxed)].get() }
    }

    fn views(&self) -> Views {
        // SAFETY: written at the fixed point only, which never overlaps a pass.
        unsafe { *self.views.get() }
    }

    fn spatial(&self) {
        self.tick_start_ns[self.tick.load(Ordering::Relaxed) as usize].store(self.created.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let cur = self.current_state();
        // SAFETY: the spatial node is the only user of the hash until it completes,
        // and the repulsion pass that reads it is armed by its completion.
        let spatial = unsafe { &mut *self.spatial.get() };
        spatial.build(&cur.spatial, &cur.x, &cur.y);
    }

    fn references_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's observed words of the next buffer are written by
        // this task only; the records are read only during the passes.
        let views = self.views();
        let observed = unsafe { views.buffers[next].observed.slice(range.clone()) };
        let records = unsafe { &(*self.instances.get()).records };
        let mut counts = ReadCounts::default();
        for (k, i) in range.enumerate() {
            if !cur.alive[i] {
                continue;
            }
            observed[k] = if !cur.has(i, REFERENCE) {
                NO_REFERENCE
            } else {
                let target = cur.reference_record[i];
                let resolution = match records.get(target as usize) {
                    Some(record) => resolve(cur, record.actual == Actual::Loaded, &record.members, target, cur.reference_local[i]),
                    None => crate::state::Resolution::Absent,
                };
                counts.record(resolution)
            };
        }
        self.reads[0].fetch_add(counts.absent, Ordering::Relaxed);
        self.reads[1].fetch_add(counts.present, Ordering::Relaxed);
        self.reads[2].fetch_add(counts.stale, Ordering::Relaxed);
    }

    fn repulsion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        // SAFETY: the hash was built by the completed spatial node and is read only.
        let spatial = unsafe { &*self.spatial.get() };
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        let views = self.views().buffers[next];
        // SAFETY: this chunk's slots of the next buffer are written by this task only.
        let (vx, vy) = unsafe { (views.vx.slice(range.clone()), views.vy.slice(range.clone())) };
        for (k, i) in range.enumerate() {
            if !cur.alive[i] {
                continue;
            }
            if cur.mask[i] & DYNAMIC == DYNAMIC {
                let (fx, fy) = repulsion_force(&cur.x, &cur.y, spatial, i);
                vx[k] = repelled_velocity(cur.vx[i], fx, cur.mass[i]);
                vy[k] = repelled_velocity(cur.vy[i], fy, cur.mass[i]);
            } else {
                vx[k] = cur.vx[i];
                vy[k] = cur.vy[i];
            }
        }
    }

    fn motion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        let views = self.views();
        let buffer = views.buffers[next];
        // SAFETY: this chunk's slots of the next buffer (velocities written by the
        // completed repulsion pass, positions stale) are read and written by this
        // task only, and the chunk owns its energy group.
        let (x, y, vx, vy) = unsafe { (buffer.x.slice(range.clone()), buffer.y.slice(range.clone()), buffer.vx.slice(range.clone()), buffer.vy.slice(range.clone())) };
        let sum_slot = unsafe { &mut views.scratch.slice(chunk..chunk + 1)[0] };
        let mut sum = 0.0_f32;
        for (k, i) in range.enumerate() {
            if !cur.alive[i] {
                continue;
            }
            if cur.mask[i] & DYNAMIC == DYNAMIC {
                let (nx, nvx) = bounced_in(integrated(cur.x[i], vx[k]), vx[k], WORLD_SIDE);
                let (ny, nvy) = bounced_in(integrated(cur.y[i], vy[k]), vy[k], WORLD_SIDE);
                x[k] = nx;
                y[k] = ny;
                vx[k] = nvx;
                vy[k] = nvy;
                sum += kinetic_energy(nvx, nvy, cur.mass[i]);
            } else {
                x[k] = cur.x[i];
                y[k] = cur.y[i];
            }
        }
        *sum_slot = sum;
    }

    /// The structural fixed point of `tick`: see the module documentation.
    fn structural(&self) {
        let tick = self.tick.load(Ordering::Relaxed);
        let cur_index = self.current.load(Ordering::Relaxed);
        let next_index = 1 - cur_index;
        // SAFETY: the structural node runs alone (see the type's documentation).
        let cur = unsafe { &mut *self.states[cur_index].get() };
        let next = unsafe { &mut *self.states[next_index].get() };
        let changed = unsafe { &mut *self.changed.get() };
        let free = unsafe { &mut *self.free.get() };
        let instances = unsafe { &mut *self.instances.get() };
        let scratch = unsafe { &mut *self.scratch.get() };
        for &slot in changed.iter() {
            next.copy_static_from(cur, slot);
        }
        changed.clear();
        let mut total = 0.0_f32;
        for sum in scratch.iter().take(cur.chunk_count()) {
            total += *sum;
        }
        next.energy = total;

        let started = Instant::now();
        let mut batch = Batch { graph: self, tick, cur, next, changed, free, instances, spawned: 0, despawned: 0, spawn_ns: 0 };
        batch.run();
        let (spawned, despawned, spawn_ns) = (batch.spawned, batch.despawned, batch.spawn_ns);
        let changes_ns = started.elapsed().as_nanos() as u64;
        self.changes_ns.fetch_add(changes_ns, Ordering::Relaxed);
        self.spawn_ns.fetch_add(spawn_ns, Ordering::Relaxed);
        self.spawned_entities.fetch_add(spawned, Ordering::Relaxed);
        self.despawned_entities.fetch_add(despawned, Ordering::Relaxed);

        // The buffers may have grown: the views and the chunk counts follow.
        // SAFETY: as above; the views are rewritten while every pass is
        // complete and before the next tick is armed.
        let (a, b) = unsafe { (&mut *self.states[0].get(), &mut *self.states[1].get()) };
        let chunk_count = a.chunk_count();
        scratch.resize(chunk_count, 0.0);
        unsafe {
            *self.views.get() = Views { buffers: [BufferViews::of(a), BufferViews::of(b)], scratch: Column::of(scratch) };
        }
        for node in [NODE_REFERENCES, NODE_REPULSION, NODE_MOTION] {
            self.nodes[node].chunks.store(chunk_count, Ordering::Relaxed);
        }
        self.current.store(next_index, Ordering::Relaxed);

        self.input_fixed_point(tick + 1);

        let t = tick as usize;
        // SAFETY: as above.
        let loaded = unsafe { (*self.instances.get()).loaded().count() as u64 };
        self.changes_per_tick_ns[t].store(changes_ns, Ordering::Relaxed);
        self.spawned_per_tick[t].store(spawned, Ordering::Relaxed);
        self.despawned_per_tick[t].store(despawned, Ordering::Relaxed);
        self.loaded_per_tick[t].store(loaded, Ordering::Relaxed);
        self.tick_end_ns[t].store(self.created.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed) as usize;
        let state = self.current_state();
        // SAFETY: the records are written by the structural node alone, which
        // waits for this hash.
        let instances = unsafe { &*self.instances.get() };
        self.tick_hashes[tick].store(state.hash(&instances.digests), Ordering::Relaxed);
        let records = instances.index.values().map(|index| &instances.records[*index as usize]).map(|record| (&record.path, record.actual, &record.members[..]));
        self.content_hashes[tick].store(content_hash(state, &instances.digests, records), Ordering::Relaxed);
    }

    fn node_tick(&self, node: usize) -> u32 {
        if node == NODE_HASH { self.hash_tick.load(Ordering::Relaxed) } else { self.tick.load(Ordering::Relaxed) }
    }

    fn execute(&self, node: usize, chunk: usize) {
        match node {
            NODE_SPATIAL => self.spatial(),
            NODE_REFERENCES => self.references_chunk(chunk),
            NODE_REPULSION => self.repulsion_chunk(chunk),
            NODE_MOTION => self.motion_chunk(chunk),
            NODE_STRUCTURAL => self.structural(),
            NODE_HASH => self.hash(),
            _ => unreachable!("six nodes"),
        }
    }

    /// Runs one task at the task boundary: a panic is caught, recorded with
    /// its tick, node, chunk and worker, and the task still counts as
    /// finished so that nothing waits forever.
    fn run_task(&self, node: usize, chunk: usize, worker: usize) {
        let started = Instant::now();
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.execute(node, chunk))) {
            let message = payload.downcast_ref::<&str>().map(|s| (*s).to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_default();
            let tick = self.node_tick(node);
            let mut failure = self.failure.lock().unwrap();
            if failure.is_none() {
                *failure = Some(Failure { tick, node: NODE_NAMES[node], chunk, worker, message });
            }
            self.failed.store(true, Ordering::SeqCst);
        }
        self.phase_ns[node].fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    // --- the scheduler ----------------------------------------------------

    /// Arms a node; a node with no chunk (an empty world) completes at once.
    fn arm(&self, node: usize) {
        let n = &self.nodes[node];
        if n.chunks.load(Ordering::Relaxed) == 0 {
            self.complete(node);
            return;
        }
        n.finished.store(0, Ordering::Relaxed);
        n.next.store(0, Ordering::Release);
        self.ready.fetch_or(1 << node, Ordering::SeqCst);
    }

    fn release_structural(&self) {
        if self.nodes[NODE_STRUCTURAL].pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.arm(NODE_STRUCTURAL);
        }
    }

    fn finish_step(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.wake(self.config.workers);
    }

    /// Whether the step stops after the tick whose structural node ran last:
    /// its last tick, a failure, or the loading gate holding the next tick.
    fn stops_after(&self, tick: u32) -> bool {
        tick >= self.end.load(Ordering::Relaxed) || self.failed.load(Ordering::SeqCst) || self.gated.load(Ordering::SeqCst)
    }

    /// Called once per node per tick, by the worker that finished its last chunk.
    fn complete(&self, node: usize) {
        self.ready.fetch_and(!(1 << node), Ordering::SeqCst);
        match node {
            NODE_SPATIAL => {
                self.arm(NODE_REPULSION);
                self.wake(self.config.workers);
            }
            NODE_REPULSION => {
                self.arm(NODE_MOTION);
                self.wake(self.config.workers);
            }
            NODE_MOTION | NODE_REFERENCES => self.release_structural(),
            NODE_HASH => {
                // The hash of the tick the step stops after ends the step;
                // any other releases the structural node of the tick running.
                if self.hash_tick.load(Ordering::Relaxed) == self.tick.load(Ordering::Relaxed) {
                    self.finish_step();
                } else {
                    self.release_structural();
                }
            }
            NODE_STRUCTURAL => {
                let tick = self.tick.load(Ordering::Relaxed);
                self.hash_tick.store(tick, Ordering::Relaxed);
                if self.stops_after(tick) {
                    self.arm(NODE_HASH);
                } else {
                    self.tick.store(tick + 1, Ordering::Relaxed);
                    self.nodes[NODE_STRUCTURAL].pending.store(3, Ordering::Relaxed);
                    self.arm(NODE_SPATIAL);
                    self.arm(NODE_HASH);
                    self.arm(NODE_REFERENCES);
                    self.wake(self.config.workers);
                }
            }
            _ => unreachable!("six nodes"),
        }
    }

    fn has_work(&self) -> bool {
        let mask = self.ready.load(Ordering::SeqCst);
        mask != 0 && (0..NODE_COUNT).any(|node| mask & (1 << node) != 0 && self.nodes[node].next.load(Ordering::Relaxed) < self.nodes[node].chunks.load(Ordering::Relaxed))
    }

    fn try_claim(&self) -> Option<(usize, usize)> {
        let mask = self.ready.load(Ordering::SeqCst);
        if mask == 0 {
            return None;
        }
        for node in CLAIM_ORDER {
            if mask & (1 << node) == 0 {
                continue;
            }
            let n = &self.nodes[node];
            let chunks = n.chunks.load(Ordering::Relaxed);
            if n.next.load(Ordering::Relaxed) < chunks {
                let chunk = n.next.fetch_add(1, Ordering::AcqRel);
                if chunk < chunks {
                    return Some((node, chunk));
                }
            }
        }
        None
    }

    fn park(&self, worker: usize) {
        self.parked[worker].store(true, Ordering::SeqCst);
        if !self.has_work() && !self.done.load(Ordering::SeqCst) {
            thread::park();
        }
        self.parked[worker].store(false, Ordering::SeqCst);
    }

    /// Wakes up to `count` parked workers.
    fn wake(&self, count: usize) {
        if count == 0 {
            return;
        }
        let Some(threads) = self.threads.get() else { return };
        let mut woken = 0;
        for (worker, thread) in threads.iter().enumerate() {
            if self.parked[worker].load(Ordering::SeqCst) {
                thread.unpark();
                woken += 1;
                if woken == count {
                    return;
                }
            }
        }
    }

    /// The measured waiting policy: spin for a bound, then park.
    fn wait(&self, worker: usize) {
        let started = Instant::now();
        let mut spins = 0_u32;
        loop {
            if self.has_work() || self.done.load(Ordering::SeqCst) {
                break;
            }
            std::hint::spin_loop();
            spins += 1;
            if spins.is_multiple_of(32) && started.elapsed() >= self.config.spin_bound {
                self.park(worker);
                break;
            }
        }
    }

    fn worker_loop(&self, worker: usize) {
        loop {
            match self.try_claim() {
                Some((node, chunk)) => {
                    self.run_task(node, chunk, worker);
                    let n = &self.nodes[node];
                    if n.finished.fetch_add(1, Ordering::AcqRel) + 1 == n.chunks.load(Ordering::Relaxed) {
                        self.complete(node);
                    }
                }
                None => {
                    if self.done.load(Ordering::SeqCst) {
                        return;
                    }
                    self.wait(worker);
                }
            }
        }
    }

    /// Runs the ticks after `hash_tick` up to `end`, or up to the tick the
    /// loading gate holds the world after, on the calling thread and
    /// `workers - 1` spawned threads, joined before returning.
    fn run_step(&mut self, end: u32) -> Duration {
        let from = self.hash_tick.load(Ordering::Relaxed);
        assert!(end > from, "a step runs at least one tick");
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        let len = end as usize + 1;
        for instrument in [
            &mut self.tick_hashes,
            &mut self.content_hashes,
            &mut self.tick_start_ns,
            &mut self.tick_end_ns,
            &mut self.changes_per_tick_ns,
            &mut self.spawned_per_tick,
            &mut self.despawned_per_tick,
            &mut self.loaded_per_tick,
        ] {
            instrument.resize_with(len, || AtomicU64::new(0));
        }
        self.threads.take();
        let graph: &Graph = self;
        let started = Instant::now();
        thread::scope(|scope| {
            let handles: Vec<_> =
                (1..graph.config.workers).map(|worker| thread::Builder::new().name(format!("worker-{worker}")).spawn_scoped(scope, move || graph.worker_loop(worker)).expect("spawn a worker")).collect();
            let mut threads = vec![thread::current()];
            threads.extend(handles.iter().map(|handle| handle.thread().clone()));
            assert!(graph.threads.set(threads).is_ok(), "threads are published once per step");
            // The hash of `from` is already known: the first structural node
            // waits for its motion pass and its reference system only.
            graph.tick.store(from + 1, Ordering::Relaxed);
            graph.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
            graph.arm(NODE_SPATIAL);
            graph.arm(NODE_REFERENCES);
            graph.worker_loop(0);
            for handle in handles {
                handle.join().expect("a worker never panics outside a task");
            }
        });
        started.elapsed()
    }
}

// ---------------------------------------------------------------------------
// The world
// ---------------------------------------------------------------------------

/// What the instruments of a world measured.
#[derive(Clone, Debug, Default)]
pub struct Instruments {
    /// The sum of the task durations per node, nanoseconds.
    pub phase_ns: [u64; NODE_COUNT],
    /// The time of the instantiation at the fixed point, and of its spawns.
    pub changes_ns: u64,
    pub spawn_ns: u64,
    pub spawned_entities: u64,
    pub despawned_entities: u64,
    /// Per tick from 1: the instantiation's time, the tick's wall time, the
    /// entities spawned and despawned, the loaded instances.
    pub changes_per_tick_ns: Vec<u64>,
    pub tick_wall_ns: Vec<u64>,
    pub spawned_per_tick: Vec<u64>,
    pub despawned_per_tick: Vec<u64>,
    pub loaded_per_tick: Vec<u64>,
    /// The wall time inside `step`.
    pub stepping: Duration,
}

/// A world: the entity columns, the tick graph, the instance records and the
/// history of its two hashes per tick, stepped by whole ticks.
pub struct World {
    graph: Graph,
    tick_done: u32,
    stepping: Duration,
    failure: Option<Failure>,
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World").field("tick", &self.tick_done).field("workers", &self.graph.config.workers).field("failure", &self.failure).finish()
    }
}

impl World {
    /// An empty world at tick 0.
    pub fn new(config: WorldConfig, store: Arc<Store>, prepared: Arc<Prepared>) -> World {
        World { graph: Graph::new(config, store, prepared), tick_done: 0, stepping: Duration::ZERO, failure: None }
    }

    /// The state hashes of the ticks 0 to the last completed one.
    pub fn hashes(&self) -> Vec<u64> {
        (0..=self.tick_done as usize).map(|t| self.graph.tick_hashes[t].load(Ordering::Relaxed)).collect()
    }

    /// The content hashes of the ticks 0 to the last completed one.
    pub fn content_hashes(&self) -> Vec<u64> {
        (0..=self.tick_done as usize).map(|t| self.graph.content_hashes[t].load(Ordering::Relaxed)).collect()
    }

    pub fn instances(&self) -> &Instances {
        // SAFETY: no step is in progress (`step` joins its workers before returning).
        unsafe { &*self.graph.instances.get() }
    }

    pub fn state(&self) -> &State {
        self.graph.current_state()
    }

    /// The reference system's counts: reads while absent, reads while
    /// present, stale slots.
    pub fn reads(&self) -> ReadCounts {
        ReadCounts { absent: self.graph.reads[0].load(Ordering::Relaxed), present: self.graph.reads[1].load(Ordering::Relaxed), stale: self.graph.reads[2].load(Ordering::Relaxed) }
    }

    pub fn instruments(&self) -> Instruments {
        let g = &self.graph;
        let per_tick = |values: &Vec<AtomicU64>| (1..=self.tick_done as usize).map(|t| values[t].load(Ordering::Relaxed)).collect::<Vec<u64>>();
        let starts = per_tick(&g.tick_start_ns);
        let ends = per_tick(&g.tick_end_ns);
        Instruments {
            phase_ns: std::array::from_fn(|node| g.phase_ns[node].load(Ordering::Relaxed)),
            changes_ns: g.changes_ns.load(Ordering::Relaxed),
            spawn_ns: g.spawn_ns.load(Ordering::Relaxed),
            spawned_entities: g.spawned_entities.load(Ordering::Relaxed),
            despawned_entities: g.despawned_entities.load(Ordering::Relaxed),
            changes_per_tick_ns: per_tick(&g.changes_per_tick_ns),
            tick_wall_ns: starts.iter().zip(&ends).map(|(start, end)| end.saturating_sub(*start)).collect(),
            spawned_per_tick: per_tick(&g.spawned_per_tick),
            despawned_per_tick: per_tick(&g.despawned_per_tick),
            loaded_per_tick: per_tick(&g.loaded_per_tick),
            stepping: self.stepping,
        }
    }

    /// Advances the world by up to `ticks` ticks on the calling thread and
    /// the configured workers, and returns how many ran: fewer than asked,
    /// none included, when the loading gate holds the world before a tick,
    /// in which case the call polled the input fixed point once more. A
    /// failed world is never stepped again.
    pub fn step(&mut self, ticks: u32) -> Result<u32, Failure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if ticks == 0 {
            return Ok(0);
        }
        let next = self.tick_done + 1;
        if self.instances().input_done != next || self.graph.gated.load(Ordering::SeqCst) {
            self.graph.input_fixed_point(next);
        }
        if self.graph.gated.load(Ordering::SeqCst) {
            return Ok(0);
        }
        let before = self.tick_done;
        self.stepping += self.graph.run_step(before + ticks);
        let failure = self.graph.failure.lock().unwrap().clone();
        if let Some(failure) = failure {
            // The failing tick's state is not a result: the history stops before it.
            self.tick_done = failure.tick.saturating_sub(1);
            self.failure = Some(failure.clone());
            return Err(failure);
        }
        self.tick_done = self.graph.hash_tick.load(Ordering::Relaxed);
        Ok(self.tick_done - before)
    }

    /// Steps until `tick` is completed, polling through the loading gate.
    pub fn advance_to(&mut self, tick: u32) -> Result<(), Failure> {
        let mut held = 0_u32;
        while self.tick_done < tick {
            if self.step(tick - self.tick_done)? == 0 {
                held += 1;
                assert!(held < 100_000, "the loading gate never opened");
            }
        }
        Ok(())
    }
}
