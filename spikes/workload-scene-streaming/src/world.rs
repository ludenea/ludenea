//! The streamed world: a trimmed copy of the late asset arrival spike's world
//! (itself a trimmed copy of the runtime skeleton spike's core: the entities
//! stepped by whole ticks on the dependency-counting tick graph, double
//! buffered, the hash of a tick overlapping the next tick's passes) with the
//! workload's kills and spawns replaced by the scene loads and unloads.
//! Throwaway, never product code.
//!
//! A tick runs the spatial hashing (one task), the reference system (chunks,
//! beside the spatial hashing), the repulsion pass (chunks), the motion pass
//! (chunks: integration, boundary inside the cell, energy), then the
//! structural node alone: the lagging buffer learns the last structural
//! changes, the pending loads and unloads are applied in record order with
//! the spawn sliced by the world parameter, the point of interest advances
//! along its path, the buffers swap, the policy writes its commands for the
//! next tick, and the input fixed point of the next tick applies the records
//! (the commands, then the arrivals sorted by request) and logs them. The
//! hash of a tick runs beside the next tick's passes.

// The fixture's one unsafe construct, the raw column views split into disjoint
// chunk slices, with the graph's dependencies as the justification (see
// `Column` and the `Sync` implementation of `Graph`).
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use reference_workload::{chunk_range, integrated, kinetic_energy, repelled_velocity};

use crate::grid::{GridConfig, NO_CELL, Path, PayloadStore, SpatialHash, bounced_in, repulsion_force};
use crate::state::{Instances, Pending, ReadCounts, State, resolve, runtime_id, slot_of};
use crate::streaming::{Arrival, ArrivalQueue, Command, InputLog, LoadRequest, LoggedRecord, NO_REFERENCE, Record, StreamingParameters, decide};

pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);

const NODE_SPATIAL: usize = 0;
const NODE_REFERENCES: usize = 1;
const NODE_REPULSION: usize = 2;
const NODE_MOTION: usize = 3;
const NODE_STRUCTURAL: usize = 4;
const NODE_HASH: usize = 5;
const NODE_COUNT: usize = 6;
pub const NODE_NAMES: [&str; NODE_COUNT] = ["spatial_hashing", "references", "repulsion", "motion", "structural", "hash"];
/// The order in which a worker looks for claimable work: the next tick's
/// spatial hashing first (it is on the critical path), then the pending hash
/// (long, and the structural node waits for it), then the passes, the
/// reference system last since it has the whole tick to complete.
const CLAIM_ORDER: [usize; NODE_COUNT] = [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_REFERENCES, NODE_STRUCTURAL];

// ---------------------------------------------------------------------------
// Configuration, failures
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct WorldConfig {
    pub grid: GridConfig,
    pub streaming: StreamingParameters,
    pub path: Path,
    pub workers: usize,
    pub spin_bound: Duration,
    /// A replay: the policy is off and the records come from this log.
    pub replay: Option<InputLog>,
}

impl WorldConfig {
    pub fn new(grid: GridConfig, streaming: StreamingParameters, path: Path, workers: usize) -> WorldConfig {
        assert!(workers >= 1, "a world is stepped by at least the calling thread");
        assert!(path.fits(&grid), "the path stays inside the grid");
        WorldConfig { grid, streaming, path, workers, spin_bound: DEFAULT_SPIN_BOUND, replay: None }
    }

    pub fn replaying(self, log: InputLog, workers: usize) -> WorldConfig {
        WorldConfig { replay: Some(log), workers, ..self }
    }
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
// Raw column views (the fixture's one unsafe construct)
// ---------------------------------------------------------------------------

/// A raw view of one column, taken while the column's owner is exclusively
/// borrowed and re-taken whenever the structural node, which runs alone,
/// resizes it. Chunks write disjoint slices of it without forming a
/// reference to the whole column: the one `unsafe` construct of the graph,
/// the split of a column into disjoint mutable chunk slices.
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

/// The views of both buffers and of the energy scratch (one sum per chunk,
/// added in chunk order at the fixed point).
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
    /// Predecessors still to complete (the structural node waits for the
    /// motion pass and the reference system of its tick and, except for the
    /// first tick of a step, for the hash of the previous tick).
    pending: AtomicUsize,
}

impl GraphNode {
    fn new(chunks: usize) -> GraphNode {
        GraphNode { chunks: AtomicUsize::new(chunks), next: AtomicUsize::new(chunks), finished: AtomicUsize::new(0), pending: AtomicUsize::new(0) }
    }
}

/// The replayed records, in log order, and a cursor.
struct Replay {
    records: Vec<LoggedRecord>,
    next: usize,
    mismatches: u64,
}

/// The time the structural node spent, split by what it did.
#[derive(Default)]
struct FixedPointClocks {
    changes_ns: AtomicU64,
    spawn_ns: AtomicU64,
    despawn_ns: AtomicU64,
    policy_ns: AtomicU64,
    input_ns: AtomicU64,
    fetch_ns: AtomicU64,
    spawned_entities: AtomicU64,
    despawned_entities: AtomicU64,
}

struct Graph {
    config: WorldConfig,
    store: Arc<PayloadStore>,
    loader: Option<Sender<LoadRequest>>,
    created: Instant,
    states: [UnsafeCell<State>; 2],
    views: UnsafeCell<Views>,
    scratch: UnsafeCell<Vec<f32>>,
    spatial: UnsafeCell<SpatialHash>,
    /// The free slots, lowest first.
    free: UnsafeCell<BinaryHeap<Reverse<usize>>>,
    /// The slots whose static columns the last structural changes touched:
    /// the only slots the other buffer must learn about.
    changed: UnsafeCell<Vec<usize>>,
    instances: UnsafeCell<Instances>,
    /// The policy's commands for the next tick's record.
    commands: UnsafeCell<Vec<Command>>,
    /// The scripted loader's deliveries: (tick, request) to cell.
    scheduled: UnsafeCell<BTreeMap<(u32, u64), u32>>,
    replay: UnsafeCell<Replay>,
    queue: ArrivalQueue,
    reads: [AtomicU64; 3],
    current: AtomicUsize,
    nodes: [GraphNode; NODE_COUNT],
    ready: AtomicU32,
    tick: AtomicU32,
    hash_tick: AtomicU32,
    end: AtomicU32,
    done: AtomicBool,
    failed: AtomicBool,
    failure: Mutex<Option<Failure>>,
    tick_hashes: Vec<AtomicU64>,
    tick_start_ns: Vec<AtomicU64>,
    tick_end_ns: Vec<AtomicU64>,
    structural_ns: Vec<AtomicU64>,
    changes_per_tick_ns: Vec<AtomicU64>,
    spawned_per_tick: Vec<AtomicU64>,
    despawned_per_tick: Vec<AtomicU64>,
    loaded_per_tick: Vec<AtomicU64>,
    phase_ns: [AtomicU64; NODE_COUNT],
    clocks: FixedPointClocks,
    spawn_cell_ns: UnsafeCell<Vec<u64>>,
    despawn_cell_ns: UnsafeCell<Vec<u64>>,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise; the structural node runs alone and is the only writer of
// the instances, the remap table, the free list, the scratch's size and the
// views; the reference chunks read the remap table during the passes only.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(config: WorldConfig, store: Arc<PayloadStore>, loader: Option<Sender<LoadRequest>>) -> Graph {
        let point = config.path.position_at(&config.grid, 0);
        let mut a = State::empty(point);
        let mut b = State::empty(point);
        let mut scratch: Vec<f32> = Vec::new();
        let views = Views { buffers: [BufferViews::of(&mut a), BufferViews::of(&mut b)], scratch: Column::of(&mut scratch) };
        let initial_hash = a.hash();
        let instances = Instances::new(config.grid.cell_count());
        let replay = Replay { records: config.replay.clone().map(|log| log.records).unwrap_or_default(), next: 0, mismatches: 0 };
        // The record of tick 1: the policy's decision from the point at tick 0.
        let commands = if config.replay.is_none() { decide(&config.grid, &config.streaming, &point, &instances.residency) } else { Vec::new() };
        let bins_per_side = config.grid.bins_per_side();
        let workers = config.workers;
        Graph {
            config,
            store,
            loader,
            created: Instant::now(),
            states: [UnsafeCell::new(a), UnsafeCell::new(b)],
            views: UnsafeCell::new(views),
            scratch: UnsafeCell::new(scratch),
            spatial: UnsafeCell::new(SpatialHash::new(bins_per_side)),
            free: UnsafeCell::new(BinaryHeap::new()),
            changed: UnsafeCell::new(Vec::new()),
            instances: UnsafeCell::new(instances),
            commands: UnsafeCell::new(commands),
            scheduled: UnsafeCell::new(BTreeMap::new()),
            replay: UnsafeCell::new(replay),
            queue: Arc::new(Mutex::new(Vec::new())),
            reads: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            current: AtomicUsize::new(0),
            nodes: [GraphNode::new(1), GraphNode::new(0), GraphNode::new(0), GraphNode::new(0), GraphNode::new(1), GraphNode::new(1)],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(0),
            hash_tick: AtomicU32::new(0),
            end: AtomicU32::new(0),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            tick_hashes: vec![AtomicU64::new(initial_hash)],
            tick_start_ns: vec![AtomicU64::new(0)],
            tick_end_ns: vec![AtomicU64::new(0)],
            structural_ns: vec![AtomicU64::new(0)],
            changes_per_tick_ns: vec![AtomicU64::new(0)],
            spawned_per_tick: vec![AtomicU64::new(0)],
            despawned_per_tick: vec![AtomicU64::new(0)],
            loaded_per_tick: vec![AtomicU64::new(0)],
            phase_ns: Default::default(),
            clocks: FixedPointClocks::default(),
            spawn_cell_ns: UnsafeCell::new(Vec::new()),
            despawn_cell_ns: UnsafeCell::new(Vec::new()),
            parked: (0..workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
        }
    }

    // --- the input fixed point -----------------------------------------------

    /// The input fixed point for `tick`: the records of the tick applied in
    /// order and logged. Live and scripted worlds apply the policy's
    /// commands (a load posts its request to the loader or schedules its
    /// scripted arrival) then the arrivals due, sorted by request; a replay
    /// applies the log's records of the tick in their order, fetching each
    /// arrival's payload from the shared store. Runs alone: in the
    /// structural node, or on the calling thread between steps.
    fn input_fixed_point(&self, tick: u32) {
        let started = Instant::now();
        // SAFETY: the fixed point runs alone (see the type's documentation).
        let instances = unsafe { &mut *self.instances.get() };
        if self.config.replay.is_some() {
            let replay = unsafe { &mut *self.replay.get() };
            while let Some(logged) = replay.records.get(replay.next) {
                if logged.tick > tick {
                    break;
                }
                let accepted = match logged.record {
                    Record::Load { cell, request } => instances.apply_load(tick, cell, Some(request)).is_some(),
                    Record::Unload { cell } => instances.apply_unload(tick, cell),
                    Record::Arrival { request, cell, content_hash } => {
                        let payload = self.fetch(cell);
                        instances.apply_arrival(tick, request, cell, payload, content_hash)
                    }
                };
                if accepted != logged.accepted || logged.tick != tick {
                    replay.mismatches += 1;
                }
                replay.next += 1;
            }
        } else {
            let commands = unsafe { &mut *self.commands.get() };
            for command in commands.drain(..) {
                match command {
                    Command::Load(cell) => {
                        if let Some(request) = instances.apply_load(tick, cell, None) {
                            self.request(tick, request, cell);
                        }
                    }
                    Command::Unload(cell) => {
                        instances.apply_unload(tick, cell);
                    }
                }
            }
            let mut arrivals = std::mem::take(&mut *self.queue.lock().expect("the queue is never poisoned"));
            let scheduled = unsafe { &mut *self.scheduled.get() };
            while let Some((&(due, request), &cell)) = scheduled.first_key_value() {
                if due > tick {
                    break;
                }
                scheduled.pop_first();
                arrivals.push(Arrival { request, cell, payload: self.fetch(cell) });
            }
            arrivals.sort_by_key(|arrival| arrival.request);
            for arrival in arrivals {
                let announced = arrival.payload.content_hash;
                instances.apply_arrival(tick, arrival.request, arrival.cell, arrival.payload, announced);
            }
        }
        self.clocks.input_ns.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// Hands an accepted load to the loader: the live thread, or the
    /// scripted delivery a fixed number of ticks later.
    fn request(&self, tick: u32, request: u64, cell: u32) {
        match self.config.streaming.scripted_delay_ticks {
            Some(delay) => {
                // SAFETY: the fixed point runs alone.
                let scheduled = unsafe { &mut *self.scheduled.get() };
                scheduled.insert((tick + delay, request), cell);
            }
            None => {
                let loader = self.loader.as_ref().expect("a live world has a loader");
                loader.send(LoadRequest { request, cell, queue: self.queue.clone() }).expect("the loader outlives the world's steps");
            }
        }
    }

    /// The payload of a cell from the shared store, imported on first use;
    /// the time is charged to the fetch clock, outside the spawn's.
    fn fetch(&self, cell: u32) -> Arc<crate::grid::Payload> {
        let started = Instant::now();
        let payload = self.store.fetch(cell);
        self.clocks.fetch_ns.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        payload
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
        spatial.build(&cur.alive, &cur.x, &cur.y);
    }

    fn references_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's observed words of the next buffer are written by
        // this task only; the remap table is read only during the passes.
        let views = self.views();
        let observed = unsafe { views.buffers[next].observed.slice(range.clone()) };
        let remap = unsafe { &(*self.instances.get()).remap };
        let mut counts = ReadCounts::default();
        for (k, i) in range.enumerate() {
            if !cur.alive[i] {
                continue;
            }
            let target = cur.reference_cell[i];
            observed[k] = if target == NO_CELL { NO_REFERENCE } else { counts.record(resolve(cur, remap, target, cur.reference_local[i])) };
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
            if cur.alive[i] {
                let (fx, fy) = repulsion_force(&cur.x, &cur.y, spatial, i);
                vx[k] = repelled_velocity(cur.vx[i], fx, cur.mass[i]);
                vy[k] = repelled_velocity(cur.vy[i], fy, cur.mass[i]);
            }
        }
    }

    fn motion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        let views = self.views();
        let buffer = views.buffers[next];
        let side = self.config.grid.cell_side;
        // SAFETY: this chunk's slots of the next buffer (velocities written by the
        // completed repulsion pass, positions stale) are read and written by this
        // task only, and the chunk owns its energy group.
        let (x, y, vx, vy) = unsafe { (buffer.x.slice(range.clone()), buffer.y.slice(range.clone()), buffer.vx.slice(range.clone()), buffer.vy.slice(range.clone())) };
        let sum_slot = unsafe { &mut views.scratch.slice(chunk..chunk + 1)[0] };
        let mut sum = 0.0_f32;
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                let (ox, oy) = self.config.grid.origin(cur.cell[i]);
                let (nx, nvx) = bounced_in(integrated(cur.x[i], vx[k]), vx[k], ox, ox + side);
                let (ny, nvy) = bounced_in(integrated(cur.y[i], vy[k]), vy[k], oy, oy + side);
                x[k] = nx;
                y[k] = ny;
                vx[k] = nvx;
                vy[k] = nvy;
                sum += kinetic_energy(nvx, nvy, cur.mass[i]);
            }
        }
        *sum_slot = sum;
    }

    /// The structural fixed point of `tick`: see the module documentation.
    fn structural(&self) {
        let started = Instant::now();
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

        let changes_started = Instant::now();
        let (spawned, despawned) = self.apply_pending(cur, next, changed, free, instances);
        let changes_ns = changes_started.elapsed().as_nanos() as u64;
        self.clocks.changes_ns.fetch_add(changes_ns, Ordering::Relaxed);
        self.clocks.spawned_entities.fetch_add(spawned, Ordering::Relaxed);
        self.clocks.despawned_entities.fetch_add(despawned, Ordering::Relaxed);

        next.point = self.config.path.position_at(&self.config.grid, tick);

        // The buffers may have grown: the views and the chunk counts follow.
        let chunk_count = next.chunk_count();
        scratch.resize(chunk_count, 0.0);
        // SAFETY: the views are rewritten while every pass is complete and
        // before the next tick is armed.
        unsafe {
            *self.views.get() = Views { buffers: [BufferViews::of(&mut *self.states[0].get()), BufferViews::of(&mut *self.states[1].get())], scratch: Column::of(scratch) };
        }
        for node in [NODE_REFERENCES, NODE_REPULSION, NODE_MOTION] {
            self.nodes[node].chunks.store(chunk_count, Ordering::Relaxed);
        }
        self.current.store(next_index, Ordering::Relaxed);

        if self.config.replay.is_none() {
            let policy_started = Instant::now();
            let commands = unsafe { &mut *self.commands.get() };
            debug_assert!(commands.is_empty(), "the previous record was consumed");
            *commands = decide(&self.config.grid, &self.config.streaming, &next.point, &instances.residency);
            self.clocks.policy_ns.fetch_add(policy_started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        self.input_fixed_point(tick + 1);

        let t = tick as usize;
        self.structural_ns[t].store(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.changes_per_tick_ns[t].store(changes_ns, Ordering::Relaxed);
        self.spawned_per_tick[t].store(spawned, Ordering::Relaxed);
        self.despawned_per_tick[t].store(despawned, Ordering::Relaxed);
        self.loaded_per_tick[t].store(instances.loaded_cells() as u64, Ordering::Relaxed);
        self.tick_end_ns[t].store(self.created.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// The pending structural changes in record order, the spawn sliced by
    /// the world parameter: a spawn takes the lowest free slots (growing
    /// both buffers when none is free), fills its partition and enters the
    /// remap table at its last slice; a despawn frees its slots in local
    /// order and leaves the table. Returns the entities spawned and
    /// despawned.
    fn apply_pending(&self, cur: &mut State, next: &mut State, changed: &mut Vec<usize>, free: &mut BinaryHeap<Reverse<usize>>, instances: &mut Instances) -> (u64, u64) {
        let mut budget = self.config.streaming.spawn_slice.map_or(u32::MAX, |slice| slice);
        let (mut spawned, mut despawned) = (0_u64, 0_u64);
        // SAFETY: the fixed point runs alone.
        let spawn_cell_ns = unsafe { &mut *self.spawn_cell_ns.get() };
        let despawn_cell_ns = unsafe { &mut *self.despawn_cell_ns.get() };
        loop {
            let Some(front) = instances.pending.front() else { break };
            match front {
                Pending::Spawn { cell, request, .. } => {
                    let (cell, request) = (*cell, *request);
                    if !instances.spawn_expected(cell, request) {
                        instances.drop_pending();
                        continue;
                    }
                    if budget == 0 {
                        break;
                    }
                    let payload = instances.payloads.get(&cell).expect("an arrived cell keeps its payload").clone();
                    let started = Instant::now();
                    let Some(Pending::Spawn { next_local, partition, elapsed, .. }) = instances.pending.front_mut() else { unreachable!() };
                    let from = *next_local;
                    let to = from.saturating_add(budget).min(payload.count);
                    for local in from..to {
                        let entity = payload.entity(local);
                        let slot = match free.pop() {
                            Some(Reverse(slot)) => slot,
                            None => {
                                cur.push_slot();
                                next.push_slot()
                            }
                        };
                        next.write_authored(slot, cell, &entity);
                        partition.push(runtime_id(slot, next.generation[slot]));
                        changed.push(slot);
                    }
                    spawned += u64::from(to - from);
                    budget -= to - from;
                    *next_local = to;
                    *elapsed += started.elapsed();
                    let slice_ns = started.elapsed().as_nanos() as u64;
                    self.clocks.spawn_ns.fetch_add(slice_ns, Ordering::Relaxed);
                    if to == payload.count {
                        let Some(Pending::Spawn { partition, elapsed, .. }) = instances.pending.pop_front() else { unreachable!() };
                        spawn_cell_ns.push(elapsed.as_nanos() as u64);
                        instances.complete_spawn(cell, partition);
                    } else {
                        instances.mark_spawning(cell, request);
                        break;
                    }
                }
                Pending::Despawn { cell } => {
                    let cell = *cell;
                    if instances.residency[cell as usize] != crate::streaming::Residency::Unloading {
                        instances.drop_pending();
                        continue;
                    }
                    let Some(partition) = instances.take_partition(cell) else {
                        instances.drop_pending();
                        continue;
                    };
                    let started = Instant::now();
                    for id in &partition {
                        let slot = slot_of(*id);
                        debug_assert_eq!(next.generation[slot], crate::state::generation_of(*id));
                        next.free_slot(slot);
                        free.push(Reverse(slot));
                        changed.push(slot);
                    }
                    despawned += partition.len() as u64;
                    instances.pending.pop_front();
                    instances.complete_despawn(cell);
                    let ns = started.elapsed().as_nanos() as u64;
                    self.clocks.despawn_ns.fetch_add(ns, Ordering::Relaxed);
                    despawn_cell_ns.push(ns);
                }
            }
        }
        (spawned, despawned)
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed);
        self.tick_hashes[tick as usize].store(self.current_state().hash(), Ordering::Relaxed);
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

    /// Called once per node per tick, by the worker that finished its last
    /// chunk. After a failure nothing new is armed except the hash the
    /// structural node owes, and the step ends at the first hash that completes.
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
                let last = self.hash_tick.load(Ordering::Relaxed) == self.end.load(Ordering::Relaxed);
                if last || self.failed.load(Ordering::SeqCst) {
                    self.finish_step();
                } else {
                    self.release_structural();
                }
            }
            NODE_STRUCTURAL => {
                let tick = self.tick.load(Ordering::Relaxed);
                self.hash_tick.store(tick, Ordering::Relaxed);
                if tick < self.end.load(Ordering::Relaxed) && !self.failed.load(Ordering::SeqCst) {
                    self.tick.store(tick + 1, Ordering::Relaxed);
                    self.nodes[NODE_STRUCTURAL].pending.store(3, Ordering::Relaxed);
                    self.arm(NODE_SPATIAL);
                    self.arm(NODE_HASH);
                    self.arm(NODE_REFERENCES);
                    self.wake(self.config.workers);
                } else {
                    self.arm(NODE_HASH);
                }
            }
            _ => unreachable!("six nodes"),
        }
    }

    fn has_work(&self) -> bool {
        let mask = self.ready.load(Ordering::SeqCst);
        mask != 0
            && (0..NODE_COUNT).any(|node| {
                mask & (1 << node) != 0 && self.nodes[node].next.load(Ordering::Relaxed) < self.nodes[node].chunks.load(Ordering::Relaxed)
            })
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

    /// Runs the ticks after `hash_tick` up to `end` on the calling thread and
    /// `workers - 1` spawned threads, joined before returning.
    fn run_step(&mut self, end: u32) -> Duration {
        let from = self.hash_tick.load(Ordering::Relaxed);
        assert!(end > from, "a step runs at least one tick");
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        let len = end as usize + 1;
        for instrument in [
            &mut self.tick_hashes,
            &mut self.tick_start_ns,
            &mut self.tick_end_ns,
            &mut self.structural_ns,
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
    /// The time of the spawns and despawns at the fixed point (the structural changes proper).
    pub changes_ns: u64,
    pub spawn_ns: u64,
    pub despawn_ns: u64,
    pub policy_ns: u64,
    pub input_ns: u64,
    pub fetch_ns: u64,
    pub spawned_entities: u64,
    pub despawned_entities: u64,
    /// Per completed cell: the time of its spawn (all slices) and of its despawn.
    pub spawn_cell_ns: Vec<u64>,
    pub despawn_cell_ns: Vec<u64>,
    /// Per tick from 1: the structural node's time, the structural changes'
    /// time, the tick's wall time from its spatial hashing to its structural
    /// node's end, the entities spawned and despawned, the loaded cells.
    pub structural_ns: Vec<u64>,
    pub changes_per_tick_ns: Vec<u64>,
    pub tick_wall_ns: Vec<u64>,
    pub spawned_per_tick: Vec<u64>,
    pub despawned_per_tick: Vec<u64>,
    pub loaded_per_tick: Vec<u64>,
    /// The wall time inside `step`.
    pub stepping: Duration,
}

/// A world: the entity columns, the tick graph, the instance records and the
/// history of tick hashes, stepped by whole ticks.
pub struct World {
    graph: Graph,
    tick_done: u32,
    stepping: Duration,
    failure: Option<Failure>,
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World")
            .field("tick", &self.tick_done)
            .field("workers", &self.graph.config.workers)
            .field("loaded_cells", &self.instances().loaded_cells())
            .field("failure", &self.failure)
            .finish()
    }
}

impl World {
    /// An empty world at tick 0 on the grid of `store`, with the policy's
    /// first commands written for tick 1 (none in a replay). A live world
    /// (no scripted delay, no replay) needs the loader's sender.
    pub fn new(config: WorldConfig, store: Arc<PayloadStore>, loader: Option<Sender<LoadRequest>>) -> World {
        assert_eq!(*store.config(), config.grid, "the store serves this grid");
        assert!(
            loader.is_some() || config.streaming.scripted_delay_ticks.is_some() || config.replay.is_some(),
            "a live world needs the loader"
        );
        World { graph: Graph::new(config, store, loader), tick_done: 0, stepping: Duration::ZERO, failure: None }
    }

    /// The last completed tick.
    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    pub fn tick_hash(&self, tick: u32) -> Option<u64> {
        (tick <= self.tick_done).then(|| self.graph.tick_hashes[tick as usize].load(Ordering::Relaxed))
    }

    /// The hashes of the ticks 0 to the last completed one.
    pub fn hashes(&self) -> Vec<u64> {
        (0..=self.tick_done).map(|t| self.tick_hash(t).expect("in the history")).collect()
    }

    pub fn instances(&self) -> &Instances {
        // SAFETY: no step is in progress (`step` joins its workers before returning).
        unsafe { &*self.graph.instances.get() }
    }

    pub fn log(&self) -> &InputLog {
        &self.instances().log
    }

    #[cfg(test)]
    pub fn state(&self) -> &State {
        self.graph.current_state()
    }

    /// The reference system's counts: reads while absent, reads while
    /// present, stale slots.
    pub fn reads(&self) -> ReadCounts {
        ReadCounts {
            absent: self.graph.reads[0].load(Ordering::Relaxed),
            present: self.graph.reads[1].load(Ordering::Relaxed),
            stale: self.graph.reads[2].load(Ordering::Relaxed),
        }
    }

    /// In a replay, the records whose acceptance or tick differed from the log.
    pub fn replay_mismatches(&self) -> u64 {
        // SAFETY: as for `instances`.
        unsafe { (*self.graph.replay.get()).mismatches }
    }

    pub fn instruments(&self) -> Instruments {
        let g = &self.graph;
        let per_tick = |values: &Vec<AtomicU64>| (1..=self.tick_done as usize).map(|t| values[t].load(Ordering::Relaxed)).collect::<Vec<u64>>();
        let starts = per_tick(&g.tick_start_ns);
        let ends = per_tick(&g.tick_end_ns);
        // SAFETY: as for `instances`.
        let (spawn_cell_ns, despawn_cell_ns) = unsafe { ((*g.spawn_cell_ns.get()).clone(), (*g.despawn_cell_ns.get()).clone()) };
        Instruments {
            phase_ns: std::array::from_fn(|node| g.phase_ns[node].load(Ordering::Relaxed)),
            changes_ns: g.clocks.changes_ns.load(Ordering::Relaxed),
            spawn_ns: g.clocks.spawn_ns.load(Ordering::Relaxed),
            despawn_ns: g.clocks.despawn_ns.load(Ordering::Relaxed),
            policy_ns: g.clocks.policy_ns.load(Ordering::Relaxed),
            input_ns: g.clocks.input_ns.load(Ordering::Relaxed),
            fetch_ns: g.clocks.fetch_ns.load(Ordering::Relaxed),
            spawned_entities: g.clocks.spawned_entities.load(Ordering::Relaxed),
            despawned_entities: g.clocks.despawned_entities.load(Ordering::Relaxed),
            spawn_cell_ns,
            despawn_cell_ns,
            structural_ns: per_tick(&g.structural_ns),
            changes_per_tick_ns: per_tick(&g.changes_per_tick_ns),
            tick_wall_ns: starts.iter().zip(&ends).map(|(start, end)| end.saturating_sub(*start)).collect(),
            spawned_per_tick: per_tick(&g.spawned_per_tick),
            despawned_per_tick: per_tick(&g.despawned_per_tick),
            loaded_per_tick: per_tick(&g.loaded_per_tick),
            stepping: self.stepping,
        }
    }

    /// Advances the world by `ticks` ticks on the calling thread and the
    /// configured workers, after the input fixed point of the next tick has
    /// picked up what landed between steps. A failed world is never stepped
    /// again: the call returns the failure at once.
    pub fn step(&mut self, ticks: u32) -> Result<(), Failure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if ticks == 0 {
            return Ok(());
        }
        self.graph.input_fixed_point(self.tick_done + 1);
        let end = self.tick_done + ticks;
        self.stepping += self.graph.run_step(end);
        let failure = self.graph.failure.lock().unwrap().clone();
        if let Some(failure) = failure {
            // The failing tick's state is not a result: the history stops before it.
            self.tick_done = failure.tick.saturating_sub(1);
            self.graph.tick_hashes.truncate(self.tick_done as usize + 1);
            self.failure = Some(failure.clone());
            return Err(failure);
        }
        self.tick_done = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::GridConfig;
    use crate::streaming::Residency;

    fn grid() -> GridConfig {
        GridConfig::new(3, 50, 0x5ce)
    }

    fn path() -> Path {
        Path::from_moves((1, 1), &[(1, 0), (-1, 0), (1, 0), (-1, 0)], 10)
    }

    fn scripted(workers: usize) -> WorldConfig {
        let streaming = StreamingParameters { entering: 1.0, leaving: 1.5, spawn_slice: Some(20), scripted_delay_ticks: Some(3) };
        WorldConfig::new(grid(), streaming, path(), workers)
    }

    #[test]
    fn an_empty_world_steps_and_the_first_record_is_the_policy_s() {
        let store = PayloadStore::new(grid());
        let mut world = World::new(scripted(2), store, None);
        assert_eq!(world.hashes().len(), 1);
        world.step(1).unwrap();
        assert_eq!(world.tick(), 1);
        let loads = world.log().accepted_loads();
        assert_eq!(loads, 5, "the centre and its four edge neighbours at the entering distance of one cell");
        assert!(world.log().records.iter().all(|r| r.tick == 1));
        assert_eq!(world.instances().residency[5], Residency::Requested(1), "the cell ahead (east) is requested first");
        assert!(matches!(world.instances().residency[4], Residency::Requested(_)));
        world.step(3).unwrap();
        assert_eq!(world.instances().loaded_cells(), 0, "the arrivals of tick 4 spawn their first slice at the fixed point of tick 4");
        assert_eq!(world.instances().residency[5], Residency::Spawning(1));
        assert_eq!(world.state().alive_count(), 20, "one slice");
        world.step(2).unwrap();
        assert_eq!(world.state().alive_count(), 60, "20 entities per tick, the cells pending in record order");
        assert_eq!(world.instances().loaded_cells(), 1);
        assert_eq!(world.instances().residency[5], Residency::Loaded);
        while world.tick() < 40 {
            world.step(7.min(40 - world.tick())).unwrap();
        }
        assert_eq!(world.instances().dropped_pending, 0);
        assert!(world.reads().stale == 0 && world.reads().present > 0 && world.reads().absent > 0);
        assert_eq!(world.replay_mismatches(), 0);
        let instruments = world.instruments();
        assert_eq!(instruments.structural_ns.len(), 40);
        assert_eq!(instruments.spawn_cell_ns.len(), world.log().accepted_arrivals());
        assert!(instruments.spawned_entities >= 250);
    }

    #[test]
    fn the_scripted_world_gives_one_history_at_one_and_three_workers() {
        let store = PayloadStore::new(grid());
        let mut first = World::new(scripted(1), store.clone(), None);
        first.step(40).unwrap();
        let mut second = World::new(scripted(3), store.clone(), None);
        for _ in 0..8 {
            second.step(5).unwrap();
        }
        assert_eq!(first.hashes(), second.hashes());
        assert_eq!(first.log(), second.log());
        assert_eq!(first.reads(), second.reads());
        assert!(first.log().accepted_unloads() > 0, "the path leaves cells behind");
    }

    #[test]
    fn a_replay_of_the_log_reproduces_the_run_at_another_worker_count() {
        let store = PayloadStore::new(grid());
        let mut live = World::new(scripted(2), store.clone(), None);
        live.step(40).unwrap();
        let log = InputLog::decode(&live.log().encode()).unwrap();
        let mut replay = World::new(scripted(2).replaying(log, 3), store, None);
        replay.step(40).unwrap();
        assert_eq!(replay.hashes(), live.hashes());
        assert_eq!(replay.log(), live.log());
        assert_eq!(replay.replay_mismatches(), 0);
    }

    #[test]
    fn the_unsliced_spawn_differs_only_by_its_timing_of_presence() {
        let store = PayloadStore::new(grid());
        let mut sliced = World::new(scripted(2), store.clone(), None);
        sliced.step(40).unwrap();
        let mut config = scripted(2);
        config.streaming.spawn_slice = None;
        let mut unsliced = World::new(config, store, None);
        unsliced.step(40).unwrap();
        assert_ne!(sliced.hashes(), unsliced.hashes(), "the slice is a world parameter and changes the result");
        assert_eq!(sliced.log().accepted_loads(), unsliced.log().accepted_loads());
        assert!(unsliced.instruments().spawned_per_tick.iter().all(|&n| n.is_multiple_of(50)), "whole cells: {:?}", unsliced.instruments().spawned_per_tick);
        assert!(unsliced.instruments().spawned_per_tick.iter().any(|&n| n > 50), "several cells in one fixed point");
    }
}
