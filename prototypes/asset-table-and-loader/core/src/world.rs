//! The world of the asset table spike. Throwaway, never product code.
//!
//! A trimmed copy of the runtime skeleton spike's core (the reference workload
//! stepped by whole ticks on the dependency-counting tick graph, the hash of a
//! tick overlapping the next tick's passes on the other buffer) with an asset
//! table as world state. Every entity names an asset by a 128-bit identifier,
//! and the repulsion pass reads the parameters of the asset each entity names,
//! resolved in the table once per chunk and distinct asset. The table changes
//! at the input fixed point between two ticks and nowhere else: the pending
//! input record of the next tick (the scripted records due, the host's
//! commands, the arrivals drained from the queue, at most one per asset) is
//! assembled, the loading gate is evaluated on it, and it is applied in one
//! piece or not at all. A gated world runs no tick and applies nothing.

use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

pub use reference_workload::{DEFAULT_SEED, Parameters, Size, State};
use reference_workload::{
    CHUNK, Fnv1a, GRID_SIDE, Grid, KILLS_PER_TICK, SPAWNS_PER_TICK, SplitMix64, bounced, chunk_range, integrated, kinetic_energy,
    repelled_velocity,
};


use crate::table::{
    Applied, Arrival, ArrivalQueue, AssetId, Availability, Command, Event, Instance, Maker, Owner, Policy, Reader, Record, RepulsionTable, Resident,
    Entry, Residents, Rules, Table, View,
};

pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);

const NODE_SPATIAL: usize = 0;
const NODE_REPULSION: usize = 1;
const NODE_MOTION: usize = 2;
const NODE_STRUCTURAL: usize = 3;
const NODE_HASH: usize = 4;
const NODE_COUNT: usize = 5;
const NODE_NAMES: [&str; NODE_COUNT] = ["spatial_hashing", "repulsion", "motion", "structural_changes", "hash"];
const CLAIM_ORDER: [usize; NODE_COUNT] = [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_STRUCTURAL];

/// System 2 for one slot with the parameters of the asset the slot names in
/// place of the specification's constants: the same operations in the same
/// order, so that the specification's parameters give the reference kernel's bits.
#[inline]
pub fn repulsion_force(state: &State, grid: &Grid, i: usize, table: RepulsionTable) -> (f32, f32) {
    let (xi, yi) = (state.x[i], state.y[i]);
    let (cx, cy) = Grid::cell_of(xi, yi);
    let radius_squared = table.radius * table.radius;
    let mut fx = 0.0_f32;
    let mut fy = 0.0_f32;
    for b in -1_isize..=1 {
        for a in -1_isize..=1 {
            let (gx, gy) = (cx as isize + a, cy as isize + b);
            if gx < 0 || gy < 0 || gx >= GRID_SIDE as isize || gy >= GRID_SIDE as isize {
                continue;
            }
            for &j in grid.slots_in(gx as usize, gy as usize) {
                let j = j as usize;
                if j == i {
                    continue;
                }
                let dx = state.x[j] - xi;
                let dy = state.y[j] - yi;
                let d2 = (dx * dx) + (dy * dy);
                if d2 < radius_squared && d2 > 0.0 {
                    let d = d2.sqrt();
                    let s = (table.strength * (table.radius - d)) / d;
                    fx -= dx * s;
                    fy -= dy * s;
                }
            }
        }
    }
    (fx, fy)
}

// ---------------------------------------------------------------------------
// Configuration, results, failures
// ---------------------------------------------------------------------------

/// How the repulsion pass gets the parameters of a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// No asset at all: one runtime value the world holds, read per chunk.
    /// The parameterized world of the late asset arrival spike, the cost baseline.
    Parameterized,
    /// Every slot names an asset by its 128-bit identifier, resolved in the
    /// table once per chunk and distinct asset.
    Identifiers,
    /// Every slot holds a 64-bit generational handle into a dense table whose
    /// entry holds the identifier: the form recorded beside the identifier's.
    Indices,
}

#[derive(Clone, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub size: Size,
    pub workers: usize,
    pub spin_bound: Duration,
    pub mode: Mode,
    /// The assets the entities name: slot `i` names `pool[i % pool.len()]`.
    pub pool: Vec<AssetId>,
    /// The wants the composition's contributors make at creation, each
    /// under the world's own entry with the identity of its contributor.
    pub initial_wants: Vec<(u64, AssetId, Policy)>,
    /// The asset types the composition declares.
    pub asset_types: Vec<u32>,
    /// The hash of the content set the world stands under, where it names
    /// one: configuration, recorded in a save's header, and no state.
    pub content_set: Option<u64>,
    /// Control: the content set's hash is hashed with the state.
    pub set_under_the_hash: bool,
    pub rules: Rules,
}

impl WorldConfig {
    pub fn new(size: Size, workers: usize, mode: Mode, pool: Vec<AssetId>) -> WorldConfig {
        assert!(workers >= 1, "a world is stepped by at least the calling thread");
        assert!(mode == Mode::Parameterized || !pool.is_empty(), "the entities name an asset of the pool");
        WorldConfig {
            seed: DEFAULT_SEED,
            size,
            workers,
            spin_bound: DEFAULT_SPIN_BOUND,
            mode,
            pool,
            initial_wants: Vec::new(),
            asset_types: vec![1],
            content_set: None,
            set_under_the_hash: false,
            rules: Rules::default(),
        }
    }

    /// The wants of one contributor of the composition.
    pub fn wanting(mut self, contributor: u64, wants: &[(AssetId, Policy)]) -> WorldConfig {
        self.initial_wants.extend(wants.iter().map(|(asset, policy)| (contributor, *asset, *policy)));
        self
    }
}

/// What a world is connected to outside the domain: the resident store of the
/// process, its own arrival queue, and the loading capability's inbox.
#[derive(Clone)]
pub struct Links {
    /// The extension point through which the core reads the parts arrivals name.
    pub residents: Arc<dyn Residents>,
    pub queue: Arc<ArrivalQueue>,
    /// Where the wants leave the domain, as extracted state at a tick's end.
    pub extract: Option<Arc<dyn Fn(Extraction) + Send + Sync>>,
}

impl Links {
    pub fn new(residents: Arc<dyn Residents>) -> Links {
        Links { residents, queue: Arc::new(ArrivalQueue::default()), extract: None }
    }
}

/// What a tick's input fixed point changed in the wants, as the availability
/// source reads it at the tick's end.
#[derive(Clone, Debug, Default)]
pub struct Extraction {
    pub tick: u32,
    /// The assets a command of the tick's record made a want of.
    pub wanted: Vec<AssetId>,
    /// The wanted assets that became unanswered at this tick, a dependency
    /// the core added a want of included.
    pub unanswered: Vec<AssetId>,
    /// Every asset a want names after the tick's record.
    pub named: Vec<AssetId>,
    /// The record held the answers to a restore under the content at hand.
    pub restore_applied: bool,
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

/// Whether a step waits for a closed gate.
#[derive(Clone, Copy, Debug)]
pub enum Wait {
    No,
    Until(Duration),
}

/// What a step answers: the ticks that ran, none included, and where the
/// world stopped at a closed gate, what is still unanswered. A required asset
/// that failed during the step is named with its reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepResult {
    pub ticks_ran: u32,
    pub gated: bool,
    pub unanswered: Vec<AssetId>,
    pub failed_required: Vec<(AssetId, u32)>,
}

/// One closing of the gate: the tick it stood before, what was unanswered,
/// and how long the world stood there once it opened.
#[derive(Clone, Debug)]
pub struct GateRecord {
    pub before_tick: u32,
    pub unanswered: Vec<AssetId>,
    pub waited: Option<Duration>,
}

/// A restore refused: the asset and the hash the table holds, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub asset: AssetId,
    pub hash: u64,
    pub reason: String,
}

/// How a saved world is read.
pub enum Restore<'a> {
    /// A restore under the content at hand: the world is created gated on
    /// its answers. `type_of` gives the asset type the manifest at hand holds
    /// for an identifier, the table holding none; an entry whose type the
    /// composition lacks refuses the restore by name unless `dropped` names
    /// it. `drop_in_silence` is a control: such an entry is dropped unnamed.
    ContentAtHand { type_of: &'a dyn Fn(AssetId) -> Option<u32>, dropped: &'a [AssetId], drop_in_silence: bool },
    /// A restore under the recorded content: every asset the table holds as
    /// available under a want is confirmed at the same hash, or the restore
    /// is refused.
    Continue(&'a dyn Fn(AssetId, u64) -> Result<Arc<Resident>, String>),
}

/// What became of the history a restored world belongs to, known once its
/// first tick has applied its answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum History {
    /// Not a restore under the content at hand, or its answers are not applied yet.
    Undecided,
    /// Every answer was at the recorded hash: the saved run continues.
    Continues,
    /// An answer differed, or the composition did: a new history, with the assets named.
    New(Vec<AssetId>),
}

// ---------------------------------------------------------------------------
// The tick graph (a trimmed copy of the runtime skeleton spike's core)
// ---------------------------------------------------------------------------

/// A raw view of one `f32` column, taken once while the column's owner is
/// exclusively borrowed and never resized afterwards. Chunks write disjoint
/// slices of it without forming a reference to the whole column.
#[derive(Clone, Copy)]
struct Column {
    ptr: *mut f32,
    len: usize,
}

unsafe impl Send for Column {}
unsafe impl Sync for Column {}

impl Column {
    fn of(values: &mut [f32]) -> Column {
        Column { ptr: values.as_mut_ptr(), len: values.len() }
    }

    /// # Safety
    ///
    /// No other thread reads or writes the slots of `range` in this column
    /// while the returned slice lives.
    #[allow(clippy::mut_from_ref)]
    unsafe fn slice(&self, range: Range<usize>) -> &mut [f32] {
        assert!(range.start <= range.end && range.end <= self.len, "chunk slice out of bounds");
        // SAFETY: in bounds by the assertion; disjointness is the caller's.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(range.start), range.len()) }
    }
}

#[derive(Clone, Copy)]
struct Columns {
    x: Column,
    y: Column,
    vx: Column,
    vy: Column,
}

impl Columns {
    fn of(state: &mut State) -> Columns {
        Columns {
            x: Column::of(&mut state.x),
            y: Column::of(&mut state.y),
            vx: Column::of(&mut state.vx),
            vy: Column::of(&mut state.vy),
        }
    }

    /// # Safety
    ///
    /// As for [`Column::slice`], for the four columns at once.
    #[allow(clippy::mut_from_ref)]
    unsafe fn slices(&self, range: Range<usize>) -> (&mut [f32], &mut [f32], &mut [f32], &mut [f32]) {
        // SAFETY: forwarded to the caller.
        unsafe { (self.x.slice(range.clone()), self.y.slice(range.clone()), self.vx.slice(range.clone()), self.vy.slice(range)) }
    }
}

/// One energy sum per group of `CHUNK` slots, written by the motion chunk
/// that owns the group and added in group order at the fixed point.
struct EnergyScratch {
    _groups: Vec<f32>,
    groups: Column,
}

impl EnergyScratch {
    fn new(entity_count: usize) -> EnergyScratch {
        let mut groups = vec![0.0_f32; entity_count.div_ceil(CHUNK)];
        let column = Column::of(&mut groups);
        EnergyScratch { _groups: groups, groups: column }
    }

    fn reduce(&self) -> f32 {
        // SAFETY: the fixed point runs alone; nothing else touches the scratch.
        let groups = unsafe { self.groups.slice(0..self.groups.len) };
        let mut total = 0.0_f32;
        for sum in groups.iter() {
            total += *sum;
        }
        total
    }
}

/// Systems 4 to 6 for one slot, in the specified order (the wander system is off).
#[inline]
fn motion_slot(x: f32, y: f32, vx: f32, vy: f32, mass: f32) -> (f32, f32, f32, f32, f32) {
    let (x, vx) = bounced(integrated(x, vx), vx);
    let (y, vy) = bounced(integrated(y, vy), vy);
    (x, y, vx, vy, kinetic_energy(vx, vy, mass))
}

struct GraphNode {
    chunks: usize,
    next: AtomicUsize,
    finished: AtomicUsize,
    pending: AtomicUsize,
}

impl GraphNode {
    fn new(chunks: usize) -> GraphNode {
        GraphNode { chunks, next: AtomicUsize::new(chunks), finished: AtomicUsize::new(0), pending: AtomicUsize::new(0) }
    }
}

/// The slots of a worker's resolution cache: twice a chunk's slots, so that a
/// chunk whose entities all name distinct assets fills half of it.
const CACHE_SLOTS: usize = CHUNK * 2;

#[derive(Clone, Copy)]
struct CacheSlot {
    id: AssetId,
    /// The chunk and tick the slot was filled for; a slot of another epoch is empty.
    epoch: u64,
    table: RepulsionTable,
}

/// A worker's cache of the identifiers it resolved for the chunk it runs:
/// open addressing on the identifier's low bits, emptied by a change of epoch.
struct ChunkCache {
    slots: Box<[CacheSlot]>,
    resolutions: u64,
}

/// An entry of the dense table of the generational handle form.
#[derive(Clone, Copy)]
struct DenseEntry {
    id: AssetId,
    generation: u32,
    table: RepulsionTable,
}

/// The input record of the next tick, as it accumulates.
#[derive(Default)]
struct Pending {
    commands: Vec<Command>,
    arrivals: BTreeMap<AssetId, Arrival>,
}

struct Script {
    records: Vec<Record>,
    next: usize,
}

/// What the input fixed points did, outside the state: the input log, the
/// events, the gate's closings and the counters the report prints.
#[derive(Default)]
struct Journal {
    log: Vec<Record>,
    events: Vec<(u32, Event)>,
    gates: Vec<GateRecord>,
    gate_closed_at: Option<Instant>,
    failed_required: Vec<(AssetId, u32)>,
    unnamed_arrivals: u32,
    refused_commands: u32,
    table_hash: u64,
    history: History,
    /// The entries a restore dropped with the composition: a new history.
    dropped: Vec<AssetId>,
}

impl Default for History {
    fn default() -> History {
        History::Undecided
    }
}

/// The tick graph over double-buffered state. The structural node runs alone
/// (every other node of its tick and the previous hash completed, the next
/// tick not armed yet) and is the fixed point where the structural changes
/// are applied, the wants of the despawned entities end, the buffers swap
/// roles and, where the step goes on, the input fixed point of the next tick
/// runs.
struct Graph {
    config: WorldConfig,
    states: [UnsafeCell<State>; 2],
    columns: [Columns; 2],
    grid: UnsafeCell<Grid>,
    scratch: EnergyScratch,
    dead: UnsafeCell<Vec<usize>>,
    changed: UnsafeCell<Vec<usize>>,
    current: AtomicUsize,
    nodes: [GraphNode; NODE_COUNT],
    ready: AtomicU32,
    tick: AtomicU32,
    hash_tick: AtomicU32,
    end: AtomicU32,
    done: AtomicBool,
    failed: AtomicBool,
    failure: Mutex<Option<Failure>>,
    /// The component hashes and the table hashes of the ticks, by tick.
    tick_hashes: Vec<AtomicU64>,
    table_hashes: Vec<AtomicU64>,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
    // --- the assets ---
    /// The parameterized mode's value, passed through a black box at creation
    /// so that nothing of it is folded into the kernel.
    parameters: RepulsionTable,
    /// The identifier each slot names.
    handles: Vec<AssetId>,
    /// The generational handle each slot holds in the dense form: the index
    /// in the low word, the generation in the high one.
    index_handles: Vec<u64>,
    dense: UnsafeCell<Vec<DenseEntry>>,
    table: UnsafeCell<Table>,
    view: UnsafeCell<View>,
    caches: Vec<UnsafeCell<ChunkCache>>,
    pending: UnsafeCell<Pending>,
    script: UnsafeCell<Script>,
    journal: UnsafeCell<Journal>,
    host_commands: Mutex<Vec<Command>>,
    /// The entities a record despawned, taken out at the structural change
    /// fixed point of the tick.
    kills: UnsafeCell<Vec<(u32, u32)>>,
    links: Links,
    /// The tick whose input fixed point has been applied.
    applied_for: AtomicU32,
    /// The gate closed inside a step: the step ends at the tick before it.
    gated: AtomicBool,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise; the structural node runs alone; the table, its view, the
// dense table, the pending record, the script and the journal are written at
// the fixed points only (the structural node, or the calling thread between
// steps) and the view and the dense table are read by the repulsion chunks of
// the following tick; a worker's cache is touched by that worker alone.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(state: State, config: WorldConfig, mut script: Vec<Record>, tick: u32, table: Table, links: Links) -> Graph {
        let state_hash = state.hash();
        let mut a = state;
        let mut b = a.clone();
        let columns = [Columns::of(&mut a), Columns::of(&mut b)];
        let n = a.len();
        let chunk_count = n.div_ceil(CHUNK);
        let dead: Vec<usize> = (0..n).filter(|&slot| !a.alive[slot]).collect();
        script.sort_by_key(|record| record.tick);
        let pool = &config.pool;
        let handles: Vec<AssetId> = if pool.is_empty() { Vec::new() } else { (0..n).map(|slot| pool[slot % pool.len()]).collect() };
        let index_handles = if pool.is_empty() { Vec::new() } else { (0..n).map(|slot| (1_u64 << 32) | (slot % pool.len()) as u64).collect() };
        let dense = pool.iter().map(|id| DenseEntry { id: *id, generation: 1, table: RepulsionTable::DEFAULT }).collect();
        let empty = CacheSlot { id: 0, epoch: 0, table: RepulsionTable::DEFAULT };
        let caches = (0..config.workers)
            .map(|_| UnsafeCell::new(ChunkCache { slots: vec![empty; CACHE_SLOTS].into_boxed_slice(), resolutions: 0 }))
            .collect();
        let hashes = |last: u64| (0..=tick).map(|t| AtomicU64::new(if t == tick { last } else { 0 })).collect::<Vec<_>>();
        let journal = Journal { table_hash: table.hash(), ..Journal::default() };
        let graph = Graph {
            states: [UnsafeCell::new(a), UnsafeCell::new(b)],
            columns,
            grid: UnsafeCell::new(Grid::new()),
            scratch: EnergyScratch::new(n),
            dead: UnsafeCell::new(dead),
            changed: UnsafeCell::new(Vec::new()),
            current: AtomicUsize::new(0),
            nodes: [GraphNode::new(1), GraphNode::new(chunk_count), GraphNode::new(chunk_count), GraphNode::new(1), GraphNode::new(1)],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(tick),
            hash_tick: AtomicU32::new(tick),
            end: AtomicU32::new(tick),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            tick_hashes: hashes(state_hash),
            table_hashes: hashes(journal.table_hash),
            parked: (0..config.workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
            parameters: std::hint::black_box(RepulsionTable::SPECIFICATION),
            handles,
            index_handles,
            dense: UnsafeCell::new(dense),
            view: UnsafeCell::new(View::build(&table)),
            table: UnsafeCell::new(table),
            caches,
            pending: UnsafeCell::new(Pending::default()),
            script: UnsafeCell::new(Script { records: script, next: 0 }),
            journal: UnsafeCell::new(journal),
            host_commands: Mutex::new(Vec::new()),
            kills: UnsafeCell::new(Vec::new()),
            links,
            applied_for: AtomicU32::new(tick),
            gated: AtomicBool::new(false),
            config,
        };
        graph.rebuild_views();
        graph
    }

    // --- the assets ----------------------------------------------------------

    /// Rebuilds what the passes read from the table. Runs alone.
    fn rebuild_views(&self) {
        // SAFETY: the fixed point runs alone (see the type's documentation).
        let (table, view, dense) = unsafe { (&*self.table.get(), &mut *self.view.get(), &mut *self.dense.get()) };
        *view = View::build(table);
        if self.config.mode == Mode::Indices {
            for entry in dense.iter_mut() {
                entry.table = view.resolve(entry.id);
            }
        }
    }

    /// The input fixed point of `tick`: assembles the pending record (the
    /// scripted records due, the host's commands, the arrivals drained from
    /// the queue), evaluates the gate on it, and applies it in one piece if
    /// the gate is open. Returns whether it was applied; a closed gate leaves
    /// the state untouched and the record pending. Runs alone.
    fn input_fixed_point(&self, tick: u32) -> bool {
        // SAFETY: the fixed point runs alone (see the type's documentation).
        let (pending, script, journal, table) =
            unsafe { (&mut *self.pending.get(), &mut *self.script.get(), &mut *self.journal.get(), &mut *self.table.get()) };
        while let Some(record) = script.records.get(script.next) {
            if record.tick > tick {
                break;
            }
            pending.commands.extend_from_slice(&record.commands);
            for arrival in &record.arrivals {
                pending.arrivals.insert(arrival.asset, *arrival);
            }
            script.next += 1;
        }
        pending.commands.append(&mut self.host_commands.lock().expect("the host's commands are never poisoned"));
        self.links.queue.drain_into(&mut pending.arrivals);
        let empty = pending.commands.is_empty() && pending.arrivals.is_empty();
        if empty && !table.has_orphans() {
            // Nothing to apply: the gate stands as the state has it.
            return self.gate_verdict(tick, !table.blocked(), table, journal);
        }
        let record = Record { tick, commands: std::mem::take(&mut pending.commands), arrivals: pending.arrivals.values().copied().collect() };
        if !table.gate_open(&record, self.links.residents.as_ref()) {
            pending.commands = record.commands;
            return self.gate_verdict(tick, false, table, journal);
        }
        pending.arrivals.clear();
        // SAFETY: the current buffer is only read here, and nothing else runs.
        let state = unsafe { &*self.states[self.current.load(Ordering::Relaxed)].get() };
        // SAFETY: the fixed point runs alone.
        let kills = unsafe { &mut *self.kills.get() };
        let alive = |slot: u32, generation: u32| {
            state.alive.get(slot as usize) == Some(&true) && state.generation[slot as usize] == generation && !kills.contains(&(slot, generation))
        };
        let unanswered_before = table.unanswered();
        let named_before = table.wanted();
        let restore_pending = !table.awaiting.is_empty();
        let applied: Applied = table.apply(&record, self.links.residents.as_ref(), &alive);
        journal.unnamed_arrivals += applied.unnamed_arrivals;
        journal.refused_commands += applied.refused_commands;
        journal.failed_required.extend_from_slice(&applied.failed_required);
        // A restore under the content at hand is decided by the tick that
        // applies its answers: the saved run continues where every answer is
        // at the recorded hash, and a new history starts where one differs.
        // The event that records it stands after the events of the tick's
        // record, or before them under the other reading.
        let restore_applied = restore_pending && table.awaiting.is_empty();
        let mut restore_event = None;
        if restore_applied {
            let mut differed = std::mem::take(&mut journal.dropped);
            differed.extend_from_slice(&applied.differed);
            differed.sort_unstable();
            differed.dedup();
            if differed.is_empty() {
                journal.history = History::Continues;
            } else {
                restore_event = Some((tick, Event::NewHistory { assets: differed.clone() }));
                journal.history = History::New(differed);
            }
        }
        if table.rules.restore_event_first {
            journal.events.extend(restore_event.take());
        }
        for event in applied.events {
            journal.events.push((tick, event));
        }
        journal.events.extend(restore_event);
        kills.extend_from_slice(&applied.despawned);
        if !record.is_empty() {
            journal.log.push(record);
        }
        journal.table_hash = table.hash();
        // The wants leave the domain for the availability source.
        if let Some(extract) = &self.links.extract {
            let unanswered: Vec<AssetId> = table.unanswered().difference(&unanswered_before).copied().collect();
            let named = table.wanted();
            if !applied.wanted.is_empty() || !unanswered.is_empty() || named != named_before || restore_applied {
                extract(Extraction { tick, wanted: applied.wanted, unanswered, named: named.into_iter().collect(), restore_applied });
            }
        }
        self.rebuild_views();
        self.gate_verdict(tick, true, table, journal)
    }

    /// Records the closing and the opening of the gate before `tick`.
    fn gate_verdict(&self, tick: u32, open: bool, table: &Table, journal: &mut Journal) -> bool {
        match (open, journal.gate_closed_at) {
            (false, None) => {
                journal.gate_closed_at = Some(Instant::now());
                journal.gates.push(GateRecord { before_tick: tick, unanswered: table.required_unanswered(), waited: None });
            }
            (true, Some(closed_at)) => {
                journal.gate_closed_at = None;
                if let Some(gate) = journal.gates.last_mut() {
                    gate.waited = Some(closed_at.elapsed());
                }
            }
            _ => {}
        }
        open
    }

    /// Resolves the parameters of the alive slots of a chunk from the
    /// identifiers they name: once per distinct identifier of the chunk, in
    /// the worker's cache, then from the cache.
    fn resolve_identifiers(&self, chunk: usize, worker: usize, range: Range<usize>, cur: &State, tables: &mut [RepulsionTable; CHUNK]) {
        // SAFETY: a worker's cache is touched by that worker alone; the view
        // is written at the fixed point only, which never overlaps a pass.
        let (cache, view) = unsafe { (&mut *self.caches[worker].get(), &*self.view.get()) };
        let epoch = (u64::from(self.tick.load(Ordering::Relaxed)) << 32) | (chunk as u64 + 1);
        let mut last_id: AssetId = 0;
        let mut last = RepulsionTable::DEFAULT;
        for (k, i) in range.enumerate() {
            if !cur.alive[i] {
                continue;
            }
            let id = self.handles[i];
            if id != last_id {
                let mut at = id as usize & (CACHE_SLOTS - 1);
                loop {
                    let slot = &mut cache.slots[at];
                    if slot.epoch != epoch {
                        *slot = CacheSlot { id, epoch, table: view.resolve(id) };
                        cache.resolutions += 1;
                        break;
                    }
                    if slot.id == id {
                        break;
                    }
                    at = (at + 1) & (CACHE_SLOTS - 1);
                }
                last_id = id;
                last = cache.slots[at].table;
            }
            tables[k] = last;
        }
    }

    // --- the kernels ------------------------------------------------------

    fn current_state(&self) -> &State {
        // SAFETY: the current buffer is only read until the structural node runs alone.
        unsafe { &*self.states[self.current.load(Ordering::Relaxed)].get() }
    }

    fn spatial(&self) {
        // SAFETY: the spatial node is the only user of the grid until it completes,
        // and the repulsion pass that reads the grid is armed by its completion.
        let grid = unsafe { &mut *self.grid.get() };
        grid.build(self.current_state());
    }

    fn repulsion_chunk(&self, chunk: usize, worker: usize) {
        let cur = self.current_state();
        // SAFETY: the grid was built by the completed spatial node and is read only.
        let grid = unsafe { &*self.grid.get() };
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's slots of the next buffer are written by this task only.
        let (_, _, vx, vy) = unsafe { self.columns[next].slices(range.clone()) };
        let mut tables = [RepulsionTable::DEFAULT; CHUNK];
        match self.config.mode {
            Mode::Parameterized => {
                // One runtime value for the chunk, as the late asset arrival spike's world read it.
                let table = self.parameters;
                for (k, i) in range.enumerate() {
                    if cur.alive[i] {
                        let (fx, fy) = repulsion_force(cur, grid, i, table);
                        vx[k] = repelled_velocity(cur.vx[i], fx, cur.mass[i]);
                        vy[k] = repelled_velocity(cur.vy[i], fy, cur.mass[i]);
                    }
                }
                return;
            }
            Mode::Identifiers => self.resolve_identifiers(chunk, worker, range.clone(), cur, &mut tables),
            Mode::Indices => {
                // SAFETY: written at the fixed point only, which never overlaps a pass.
                let dense = unsafe { &*self.dense.get() };
                for (k, i) in range.clone().enumerate() {
                    let handle = self.index_handles[i];
                    let entry = &dense[(handle & 0xffff_ffff) as usize];
                    tables[k] = if entry.generation == (handle >> 32) as u32 { entry.table } else { RepulsionTable::DEFAULT };
                }
            }
        }
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                let (fx, fy) = repulsion_force(cur, grid, i, tables[k]);
                vx[k] = repelled_velocity(cur.vx[i], fx, cur.mass[i]);
                vy[k] = repelled_velocity(cur.vy[i], fy, cur.mass[i]);
            }
        }
    }

    fn motion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's slots of the next buffer (velocities written by the
        // completed repulsion pass, positions stale) are read and written by this
        // task only, and the chunk owns its energy group.
        let (x, y, vx, vy) = unsafe { self.columns[next].slices(range.clone()) };
        let sum_slot = unsafe { &mut self.scratch.groups.slice(chunk..chunk + 1)[0] };
        let mut sum = 0.0_f32;
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                let (nx, ny, nvx, nvy, e) = motion_slot(cur.x[i], cur.y[i], vx[k], vy[k], cur.mass[i]);
                x[k] = nx;
                y[k] = ny;
                vx[k] = nvx;
                vy[k] = nvy;
                sum += e;
            }
        }
        *sum_slot = sum;
    }

    /// The fixed point: brings the next buffer's liveness, generations,
    /// masses and stream up to date, reduces the energy, applies the kills
    /// and spawns, ends the wants of the entities despawned, swaps the
    /// buffers, records the table's hash for the tick, then, where the step
    /// goes on, runs the input fixed point of the next tick.
    fn structural(&self) {
        let cur_index = self.current.load(Ordering::Relaxed);
        let next_index = 1 - cur_index;
        // SAFETY: the structural node runs alone (see the type's documentation).
        let cur = unsafe { &*self.states[cur_index].get() };
        let next = unsafe { &mut *self.states[next_index].get() };
        let changed = unsafe { &mut *self.changed.get() };
        let dead = unsafe { &mut *self.dead.get() };
        let (table, journal) = unsafe { (&mut *self.table.get(), &mut *self.journal.get()) };
        for &slot in changed.iter() {
            next.alive[slot] = cur.alive[slot];
            next.generation[slot] = cur.generation[slot];
            next.mass[slot] = cur.mass[slot];
        }
        next.rng = cur.rng;
        next.energy = self.scratch.reduce();
        let n = next.len();
        let mut peek = next.rng;
        changed.clear();
        for _ in 0..KILLS_PER_TICK {
            changed.push((peek.next() % n as u64) as usize);
        }
        // The entities the tick's record despawned leave here, before the
        // workload's own kills and spawns.
        let kills = unsafe { &mut *self.kills.get() };
        let mut ended = false;
        for &(slot, generation) in kills.iter() {
            next.alive[slot as usize] = false;
            changed.push(slot as usize);
            // Under the other reading of a despawn, its wants end here.
            ended |= table.end_wants_of_entity(slot, Some(generation));
        }
        kills.clear();
        next.structural_changes();
        // A want ends when its owner is despawned.
        for &slot in changed.iter() {
            ended |= table.end_wants_of_entity(slot as u32, None);
        }
        if ended {
            journal.table_hash = table.hash();
        }
        dead.extend_from_slice(changed);
        dead.sort_unstable();
        dead.dedup();
        let spawned = SPAWNS_PER_TICK.min(dead.len());
        changed.extend(dead.drain(..spawned));
        self.current.store(next_index, Ordering::Relaxed);
        let tick = self.tick.load(Ordering::Relaxed);
        self.table_hashes[tick as usize].store(journal.table_hash, Ordering::Relaxed);
        // The record of the tick after a step's last one is left to the next
        // step, so that what the host posts between two steps is part of it.
        if tick < self.end.load(Ordering::Relaxed) && !self.failed.load(Ordering::SeqCst) {
            if self.input_fixed_point(tick + 1) {
                self.applied_for.store(tick + 1, Ordering::Relaxed);
            } else {
                self.gated.store(true, Ordering::SeqCst);
            }
        }
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed);
        self.tick_hashes[tick as usize].store(self.current_state().hash(), Ordering::Relaxed);
    }

    fn node_tick(&self, node: usize) -> u32 {
        if node == NODE_HASH { self.hash_tick.load(Ordering::Relaxed) } else { self.tick.load(Ordering::Relaxed) }
    }

    fn execute(&self, node: usize, chunk: usize, worker: usize) {
        match node {
            NODE_SPATIAL => self.spatial(),
            NODE_REPULSION => self.repulsion_chunk(chunk, worker),
            NODE_MOTION => self.motion_chunk(chunk),
            NODE_STRUCTURAL => self.structural(),
            NODE_HASH => self.hash(),
            _ => unreachable!("five nodes"),
        }
    }

    /// Runs one task at the task boundary: a panic is caught, recorded with
    /// its tick, node, chunk and worker, and the task still counts as
    /// finished so that nothing waits forever.
    fn run_task(&self, node: usize, chunk: usize, worker: usize) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.execute(node, chunk, worker))) {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            let tick = self.node_tick(node);
            let mut failure = self.failure.lock().unwrap();
            if failure.is_none() {
                *failure = Some(Failure { tick, node: NODE_NAMES[node], chunk, worker, message });
            }
            self.failed.store(true, Ordering::SeqCst);
        }
    }

    // --- the scheduler ----------------------------------------------------

    fn arm(&self, node: usize) {
        let n = &self.nodes[node];
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

    /// Whether the step stops after the tick whose structural node completed:
    /// its last tick, a failure, or a gate that closed before the next tick.
    fn stops(&self, tick: u32) -> bool {
        tick == self.end.load(Ordering::Relaxed) || self.failed.load(Ordering::SeqCst) || self.gated.load(Ordering::SeqCst)
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
            NODE_MOTION => self.release_structural(),
            NODE_HASH => {
                if self.stops(self.hash_tick.load(Ordering::Relaxed)) {
                    self.finish_step();
                } else {
                    self.release_structural();
                }
            }
            NODE_STRUCTURAL => {
                let tick = self.tick.load(Ordering::Relaxed);
                self.hash_tick.store(tick, Ordering::Relaxed);
                if self.stops(tick) {
                    self.arm(NODE_HASH);
                } else {
                    self.tick.store(tick + 1, Ordering::Relaxed);
                    self.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
                    self.arm(NODE_SPATIAL);
                    self.arm(NODE_HASH);
                    // The arming worker takes one of the two serial nodes; one more worker is needed.
                    self.wake(1);
                }
            }
            _ => unreachable!("five nodes"),
        }
    }

    fn has_work(&self) -> bool {
        let mask = self.ready.load(Ordering::SeqCst);
        mask != 0
            && (0..NODE_COUNT)
                .any(|node| mask & (1 << node) != 0 && self.nodes[node].next.load(Ordering::Relaxed) < self.nodes[node].chunks)
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
            if n.next.load(Ordering::Relaxed) < n.chunks {
                let chunk = n.next.fetch_add(1, Ordering::AcqRel);
                if chunk < n.chunks {
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
                    if n.finished.fetch_add(1, Ordering::AcqRel) + 1 == n.chunks {
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

    /// Runs the ticks after `hash_tick` up to `end`, or up to a gate that
    /// closes on the way, on the calling thread and `workers - 1` spawned
    /// threads, joined before returning.
    fn run_step(&mut self, end: u32) -> Duration {
        let from = self.hash_tick.load(Ordering::Relaxed);
        assert!(end > from, "a step runs at least one tick");
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        self.gated.store(false, Ordering::SeqCst);
        self.tick_hashes.resize_with(end as usize + 1, || AtomicU64::new(0));
        self.table_hashes.resize_with(end as usize + 1, || AtomicU64::new(0));
        self.threads.take();
        let graph: &Graph = self;
        let started = Instant::now();
        thread::scope(|scope| {
            let handles: Vec<_> = (1..graph.config.workers)
                .map(|worker| {
                    thread::Builder::new()
                        .name(format!("worker-{worker}"))
                        .spawn_scoped(scope, move || graph.worker_loop(worker))
                        .expect("spawn a worker")
                })
                .collect();
            let mut threads = vec![thread::current()];
            threads.extend(handles.iter().map(|handle| handle.thread().clone()));
            assert!(graph.threads.set(threads).is_ok(), "threads are published once per step");
            // The hash of `from` is already known: the first structural node
            // waits for its motion pass only.
            graph.tick.store(from + 1, Ordering::Relaxed);
            graph.nodes[NODE_STRUCTURAL].pending.store(1, Ordering::Relaxed);
            graph.arm(NODE_SPATIAL);
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

const SAVE_FORMAT: u32 = 2;

/// A world: the reference workload's state, its tick graph, its asset table
/// and its history of hashes, stepped by whole ticks.
pub struct World {
    graph: Graph,
    first_tick: u32,
    tick_done: u32,
    stepping: Duration,
    failure: Option<Failure>,
}

/// The header of a save: what is configuration and no state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveHeader {
    pub tick: u32,
    /// The hash of the content set the saved world stood under, where it named one.
    pub content_set: Option<u64>,
    /// The asset types its composition declared.
    pub asset_types: Vec<u32>,
}

impl World {
    /// A world at tick 0, initialized by the specification from the seed,
    /// with the wants of its composition's contributors and its scripted
    /// input records (each part of the pending record of the tick it names).
    /// No tick has ended: the availability source reads the wants by a query
    /// at the boundary (`unanswered`).
    pub fn new(config: WorldConfig, script: Vec<Record>, links: Links) -> World {
        let parameters = Parameters { seed: config.seed, size: config.size, tick_count: 0, wander: false };
        let state = State::initialize(&parameters);
        let mut table = Table::with_rules(config.rules);
        for (contributor, asset, policy) in &config.initial_wants {
            table.want(*asset, Owner::World(Maker::Contributor(*contributor)), *policy);
        }
        table.refresh();
        World { graph: Graph::new(state, config, script, 0, table, links), first_tick: 0, tick_done: 0, stepping: Duration::ZERO, failure: None }
    }

    fn table(&self) -> &Table {
        // SAFETY: no step is in progress (`step` joins its workers before returning).
        unsafe { &*self.graph.table.get() }
    }

    fn journal(&self) -> &Journal {
        // SAFETY: as for `table`.
        unsafe { &*self.graph.journal.get() }
    }

    /// The last completed tick.
    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    /// The component hashes (the reference workload's tick hashes) from the
    /// world's first tick to the last completed one.
    pub fn component_hashes(&self) -> Vec<u64> {
        (self.first_tick..=self.tick_done).map(|t| self.graph.tick_hashes[t as usize].load(Ordering::Relaxed)).collect()
    }

    /// The hash of the asset table after `tick`.
    pub fn table_hash(&self, tick: u32) -> u64 {
        assert!((self.first_tick..=self.tick_done).contains(&tick), "a tick of the history");
        self.graph.table_hashes[tick as usize].load(Ordering::Relaxed)
    }

    /// The state hashes: per tick, the component hash and the hash of the
    /// asset table's canonical bytes hashed together, so that two worlds that
    /// differ in the table have diverged. The content set is configuration
    /// and is not hashed, but under the control that hashes it.
    pub fn state_hashes(&self) -> Vec<u64> {
        (self.first_tick..=self.tick_done)
            .map(|t| {
                let mut hasher = Fnv1a::new();
                hasher.write(&self.graph.tick_hashes[t as usize].load(Ordering::Relaxed).to_le_bytes());
                hasher.write(&self.table_hash(t).to_le_bytes());
                if self.graph.config.set_under_the_hash {
                    hasher.write(&self.graph.config.content_set.unwrap_or(0).to_le_bytes());
                }
                hasher.finish()
            })
            .collect()
    }

    /// The wall time spent stepping ticks, the waits at a closed gate excluded.
    pub fn stepping(&self) -> Duration {
        self.stepping
    }

    /// The availability and the wants of an asset, if the table holds it.
    pub fn entry(&self, asset: AssetId) -> Option<(Availability, Vec<(Owner, Policy)>)> {
        self.table().entries.get(&asset).map(|entry| (entry.availability, entry.wants.clone()))
    }

    pub fn table_bytes(&self) -> Vec<u8> {
        self.table().canonical_bytes()
    }

    pub fn instances(&self) -> &[Instance] {
        &self.table().instances
    }

    /// The input log: the record of every tick that had one, in order.
    pub fn log(&self) -> &[Record] {
        &self.journal().log
    }

    pub fn events(&self) -> &[(u32, Event)] {
        &self.journal().events
    }

    pub fn gates(&self) -> &[GateRecord] {
        &self.journal().gates
    }

    pub fn unnamed_arrivals(&self) -> u32 {
        self.journal().unnamed_arrivals
    }

    pub fn refused_commands(&self) -> u32 {
        self.journal().refused_commands
    }

    /// What became of the history a restored world belongs to.
    pub fn history(&self) -> &History {
        &self.journal().history
    }

    pub fn content_set(&self) -> Option<u64> {
        self.graph.config.content_set
    }

    /// Whether an entity is alive between two steps.
    pub fn alive(&self, slot: u32, generation: u32) -> bool {
        let state = self.graph.current_state();
        state.alive[slot as usize] && state.generation[slot as usize] == generation
    }

    /// The identifiers the passes resolved in the table so far, over every worker.
    pub fn resolutions(&self) -> u64 {
        // SAFETY: no step is in progress.
        self.graph.caches.iter().map(|cache| unsafe { (*cache.get()).resolutions }).sum()
    }

    /// The wanted assets that are unanswered: what the availability source
    /// reads by a query at a boundary where no tick has ended.
    pub fn unanswered(&self) -> Vec<AssetId> {
        self.table().unanswered().into_iter().collect()
    }

    /// Every asset a want names.
    pub fn named(&self) -> Vec<AssetId> {
        self.table().wanted().into_iter().collect()
    }

    /// The assets a restore under the content at hand awaits an answer for,
    /// with the hash the table holds.
    pub fn restore_query(&self) -> Vec<(AssetId, u64)> {
        let table = self.table();
        let held = |id: &AssetId| match table.entries.get(id)?.availability {
            Availability::Available(hash) => Some((*id, hash)),
            _ => None,
        };
        table.awaiting.iter().filter_map(held).collect()
    }

    /// Every entry of the table, with its availability and the count of its
    /// wants: what a control that answers too much reads.
    pub fn entries(&self) -> Vec<(AssetId, Availability, usize)> {
        self.table().entries.iter().map(|(id, entry)| (*id, entry.availability, entry.wants.len())).collect()
    }

    /// A command of the host: part of the pending record of the next tick.
    pub fn post(&self, command: Command) {
        self.graph.host_commands.lock().expect("the host's commands are never poisoned").push(command);
    }

    fn result(&mut self, from: u32, gated: bool) -> StepResult {
        // SAFETY: no step is in progress.
        let journal = unsafe { &mut *self.graph.journal.get() };
        StepResult {
            ticks_ran: self.tick_done - from,
            gated,
            unanswered: if gated { self.table().required_unanswered() } else { Vec::new() },
            failed_required: std::mem::take(&mut journal.failed_required),
        }
    }

    /// Advances the world by up to `ticks` ticks on the calling thread and
    /// the configured workers. Where the gate is closed before a tick, the
    /// step waits as asked, then answers with the ticks that ran and what is
    /// still unanswered. A failed world is never stepped again.
    pub fn step(&mut self, ticks: u32, wait: Wait) -> Result<StepResult, Failure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        let from = self.tick_done;
        let target = from + ticks;
        let started = Instant::now();
        while self.tick_done < target {
            let next = self.tick_done + 1;
            if self.graph.applied_for.load(Ordering::Relaxed) < next {
                loop {
                    if self.graph.input_fixed_point(next) {
                        break;
                    }
                    match wait {
                        Wait::Until(limit) if started.elapsed() < limit => thread::sleep(Duration::from_micros(200)),
                        _ => return Ok(self.result(from, true)),
                    }
                }
                self.graph.applied_for.store(next, Ordering::Relaxed);
            }
            self.stepping += self.graph.run_step(target);
            let failure = self.graph.failure.lock().unwrap().clone();
            if let Some(failure) = failure {
                // The failing tick's state is not a result: the history stops before it.
                self.tick_done = failure.tick.saturating_sub(1);
                self.failure = Some(failure.clone());
                return Err(failure);
            }
            self.tick_done = self.graph.hash_tick.load(Ordering::Relaxed);
        }
        Ok(self.result(from, false))
    }

    /// The saved form of the world between two steps: a header that holds
    /// what is configuration (the content set, the composition's asset
    /// types), then its components and the canonical bytes of its asset table.
    pub fn save(&self) -> Vec<u8> {
        let state = self.graph.current_state();
        let config = &self.graph.config;
        let mut out = Vec::new();
        out.extend_from_slice(&SAVE_FORMAT.to_le_bytes());
        out.extend_from_slice(&self.tick_done.to_le_bytes());
        out.push(u8::from(config.content_set.is_some()));
        out.extend_from_slice(&config.content_set.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&(config.asset_types.len() as u32).to_le_bytes());
        for asset_type in &config.asset_types {
            out.extend_from_slice(&asset_type.to_le_bytes());
        }
        out.extend_from_slice(&(state.len() as u32).to_le_bytes());
        for slot in 0..state.len() {
            out.extend_from_slice(&state.generation[slot].to_le_bytes());
            out.push(u8::from(state.alive[slot]));
            for value in [state.x[slot], state.y[slot], state.vx[slot], state.vy[slot], state.mass[slot]] {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        out.extend_from_slice(&state.rng.state.to_le_bytes());
        out.extend_from_slice(&state.energy.to_le_bytes());
        let table = self.table_bytes();
        out.extend_from_slice(&(table.len() as u32).to_le_bytes());
        out.extend_from_slice(&table);
        out
    }

    /// Reads a saved world. The request states the content set the new world
    /// stands under and its composition, in `config`, in place of the save's.
    ///
    /// Under the content at hand, the world is created gated on its answers:
    /// it runs no tick until every asset its table holds as available under
    /// a want has been answered, in its first tick's record, by an arrival
    /// marked as an answer to a restore. An entry that holds no want is not
    /// answered, a failed entry stays failed, and an unanswered want is
    /// loaded as any want is.
    ///
    /// Under the recorded content, every asset the table holds as available
    /// under a want is confirmed at the same hash before the first tick,
    /// outside the input record, or the restore is refused, naming the hash.
    pub fn restore(config: WorldConfig, bytes: &[u8], script: Vec<Record>, links: Links, restore: Restore) -> Result<World, Refusal> {
        let (header, state, mut table) = read_save(bytes, config.rules).expect("the save decodes");
        assert_eq!(state.len(), config.size.entity_count(), "the save is of the configured size");
        let mut history = History::Undecided;
        let mut dropped_entries = Vec::new();
        match restore {
            Restore::ContentAtHand { type_of, dropped, drop_in_silence } => {
                let lacking: Vec<(AssetId, u32)> = table
                    .entries
                    .keys()
                    .filter_map(|id| Some((*id, type_of(*id)?)))
                    .filter(|(_, asset_type)| !config.asset_types.contains(asset_type))
                    .collect();
                for (asset, asset_type) in lacking {
                    let hash = match table.entries[&asset].availability {
                        Availability::Available(hash) => hash,
                        _ => 0,
                    };
                    if drop_in_silence {
                        table.entries.remove(&asset);
                    } else if dropped.contains(&asset) {
                        table.entries.remove(&asset);
                        dropped_entries.push(asset);
                    } else {
                        return Err(Refusal { asset, hash, reason: format!("the composition lacks the asset type {asset_type}") });
                    }
                }
                let without_a_want = config.rules.restore_awaits_entries_without_a_want;
                let awaited = |entry: &Entry| matches!(entry.availability, Availability::Available(_)) && (without_a_want || !entry.wants.is_empty());
                table.awaiting = table.entries.iter().filter(|(_, entry)| awaited(entry)).map(|(id, _)| *id).collect();
                if table.awaiting.is_empty() {
                    history = if dropped_entries.is_empty() { History::Continues } else { History::New(dropped_entries.clone()) };
                }
            }
            Restore::Continue(confirm) => {
                let held: Vec<(AssetId, u64)> = table
                    .entries
                    .iter()
                    .filter(|(_, entry)| !entry.wants.is_empty())
                    .filter_map(|(id, entry)| match entry.availability {
                        Availability::Available(hash) => Some((*id, hash)),
                        _ => None,
                    })
                    .collect();
                for (asset, hash) in held {
                    let resident = confirm(asset, hash).map_err(|reason| Refusal { asset, hash, reason })?;
                    table.entries.get_mut(&asset).expect("an entry of the table").resident = Some(resident);
                }
            }
        }
        table.refresh();
        let tick = header.tick;
        let graph = Graph::new(state, config, script, tick, table, links);
        let mut world = World { graph, first_tick: tick, tick_done: tick, stepping: Duration::ZERO, failure: None };
        let journal = world.graph.journal.get_mut();
        if let History::New(assets) = &history {
            journal.events.push((tick, Event::NewHistory { assets: assets.clone() }));
        }
        journal.history = history;
        journal.dropped = dropped_entries;
        Ok(world)
    }
}

/// Reads the header of a save alone.
pub fn save_header(bytes: &[u8]) -> Result<SaveHeader, String> {
    read_header(&mut Reader::new(bytes))
}

fn read_header(reader: &mut Reader) -> Result<SaveHeader, String> {
    if reader.u32()? != SAVE_FORMAT {
        return Err("unknown save format".to_string());
    }
    let tick = reader.u32()?;
    let (named, set) = (reader.flag()?, reader.u64()?);
    let mut asset_types = Vec::new();
    for _ in 0..reader.u32()? {
        asset_types.push(reader.u32()?);
    }
    Ok(SaveHeader { tick, content_set: named.then_some(set), asset_types })
}

fn read_save(bytes: &[u8], rules: Rules) -> Result<(SaveHeader, State, Table), String> {
    let mut reader = Reader::new(bytes);
    let header = read_header(&mut reader)?;
    let n = reader.u32()? as usize;
    let mut state = State {
        generation: Vec::with_capacity(n),
        alive: Vec::with_capacity(n),
        x: Vec::with_capacity(n),
        y: Vec::with_capacity(n),
        vx: Vec::with_capacity(n),
        vy: Vec::with_capacity(n),
        mass: Vec::with_capacity(n),
        rng: SplitMix64::new(0),
        energy: 0.0,
    };
    for _ in 0..n {
        state.generation.push(reader.u32()?);
        state.alive.push(reader.flag()?);
        state.x.push(reader.f32()?);
        state.y.push(reader.f32()?);
        state.vx.push(reader.f32()?);
        state.vy.push(reader.f32()?);
        state.mass.push(reader.f32()?);
    }
    state.rng = SplitMix64::new(reader.u64()?);
    state.energy = reader.f32()?;
    let table_len = reader.u32()? as usize;
    let table = Table::decode(reader.take(table_len)?, rules)?;
    reader.end()?;
    Ok((header, state, table))
}
