//! Core crate of the runtime skeleton spike. Throwaway, never product code.
//!
//! A [`World`] steps the reference workload on the dependency-counting tick
//! graph the multithreading research measured (a trimmed copy of its counter
//! variant: the hash of a tick runs while the next tick's passes proceed on
//! the other buffer), under a time control API: step a number of ticks, read
//! the hash of any completed tick, take a snapshot, restore one into a new
//! world. The thread that calls `step` is worker 0 of that step; the other
//! workers are spawned for the step and joined before it returns, so a world
//! may be stepped by different threads over its life and two worlds never
//! share a thread at the same time.
//!
//! A panic inside a task is caught at the task boundary, recorded with its
//! tick, node, chunk and worker, counted as a completion so that nothing waits
//! forever, and fails the world: `step` returns the failure, the hashes stop
//! at the last good tick, and the world is never stepped again.
//!
//! The core names no presentation type. The [`Extraction`] extension point is
//! a trait that a capability implements; when a capability is present the
//! fixed point calls it after every tick with the state after the tick, and
//! when none is present nothing runs there.

pub mod edit_marker;

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

pub use reference_workload::{DEFAULT_SEED, Parameters, Size, State};
use reference_workload::{
    CHUNK, Grid, KILLS_PER_TICK, SPAWNS_PER_TICK, VERSION, bounced, chunk_range, integrated,
    kinetic_energy, repelled_velocity, repulsion_force, wandered_velocity,
};

pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);

const NODE_SPATIAL: usize = 0;
const NODE_REPULSION: usize = 1;
const NODE_MOTION: usize = 2;
const NODE_STRUCTURAL: usize = 3;
const NODE_HASH: usize = 4;
const NODE_COUNT: usize = 5;
const NODE_NAMES: [&str; NODE_COUNT] =
    ["spatial_hashing", "repulsion", "motion", "structural_changes", "hash"];
/// The order in which a worker looks for claimable work: the next tick's
/// spatial hashing first (it is on the critical path), then the pending hash
/// (long, and the structural changes wait for it), then the passes.
const CLAIM_ORDER: [usize; NODE_COUNT] =
    [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_STRUCTURAL];

const SNAPSHOT_MAGIC: &[u8; 4] = b"LDSK";
const SNAPSHOT_FORMAT: u32 = 1;

/// The nodes of the tick graph, named for the injection of a programming error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    SpatialHashing,
    Repulsion,
    Motion,
    StructuralChanges,
    Hash,
}

impl Node {
    fn index(self) -> usize {
        match self {
            Node::SpatialHashing => NODE_SPATIAL,
            Node::Repulsion => NODE_REPULSION,
            Node::Motion => NODE_MOTION,
            Node::StructuralChanges => NODE_STRUCTURAL,
            Node::Hash => NODE_HASH,
        }
    }

    pub fn name(self) -> &'static str {
        NODE_NAMES[self.index()]
    }
}

/// A programming error injected in one task: a panic in `chunk` of `node` at `tick`.
#[derive(Clone, Copy, Debug)]
pub struct Injection {
    pub tick: u32,
    pub node: Node,
    pub chunk: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub size: Size,
    pub wander: bool,
    pub workers: usize,
    pub spin_bound: Duration,
    pub injection: Option<Injection>,
}

impl WorldConfig {
    pub fn new(size: Size, workers: usize) -> WorldConfig {
        assert!(workers >= 1, "a world is stepped by at least the calling thread");
        WorldConfig {
            seed: DEFAULT_SEED,
            size,
            wander: false,
            workers,
            spin_bound: DEFAULT_SPIN_BOUND,
            injection: None,
        }
    }

    pub fn seeded(self, seed: u64) -> WorldConfig {
        WorldConfig { seed, ..self }
    }

    pub fn injecting(self, injection: Injection) -> WorldConfig {
        WorldConfig { injection: Some(injection), ..self }
    }
}

/// The presentation extraction extension point. The fixed point calls it
/// after every tick with the state after the tick; an implementation reads
/// the state and writes its own storage, never the domain.
pub trait Extraction: Send + Sync {
    fn extract(&self, tick: u32, state: &State);
}

/// The capabilities a world is created with. Absent, a capability adds nothing.
#[derive(Clone, Default)]
pub struct Capabilities {
    pub extraction: Option<Arc<dyn Extraction>>,
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

/// The error of stepping or snapshotting a failed world.
#[derive(Clone, Debug)]
pub struct WorldFailed(pub Failure);

impl fmt::Display for WorldFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "world failed at tick {} in {} chunk {} on worker {}: {}",
            self.0.tick, self.0.node, self.0.chunk, self.0.worker, self.0.message
        )
    }
}

impl std::error::Error for WorldFailed {}

/// A snapshot that cannot be restored, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotError(pub String);

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "snapshot rejected: {}", self.0)
    }
}

impl std::error::Error for SnapshotError {}

/// The canonical bytes of a world at a tick boundary: a header (magic,
/// format, workload version, seed, entity count, tick, wander flag), then
/// per slot the generation, the alive flag and, for an alive slot, its five
/// values, then the energy and the stream state. Dead slots carry no values,
/// as in the specification's hash, so that the bytes never depend on which
/// buffer of the double-buffered graph is current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub bytes: Vec<u8>,
}

struct SnapshotHeader {
    seed: u64,
    entity_count: usize,
    tick: u32,
    wander: bool,
}

const HEADER_LEN: usize = 4 + 4 + 4 + 8 + 4 + 4 + 1;

impl Snapshot {
    fn encode(state: &State, seed: u64, tick: u32, wander: bool) -> Snapshot {
        let mut bytes = Vec::with_capacity(HEADER_LEN + state.len() * 25 + 12);
        bytes.extend_from_slice(SNAPSHOT_MAGIC);
        bytes.extend_from_slice(&SNAPSHOT_FORMAT.to_le_bytes());
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&seed.to_le_bytes());
        bytes.extend_from_slice(&(state.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&tick.to_le_bytes());
        bytes.push(u8::from(wander));
        for slot in 0..state.len() {
            bytes.extend_from_slice(&state.generation[slot].to_le_bytes());
            bytes.push(u8::from(state.alive[slot]));
            if state.alive[slot] {
                for value in [state.x[slot], state.y[slot], state.vx[slot], state.vy[slot], state.mass[slot]] {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        bytes.extend_from_slice(&state.energy.to_le_bytes());
        bytes.extend_from_slice(&state.rng.state.to_le_bytes());
        Snapshot { bytes }
    }

    fn decode(&self) -> Result<(SnapshotHeader, State), SnapshotError> {
        let mut reader = Reader { bytes: &self.bytes, at: 0 };
        if reader.take(4)? != SNAPSHOT_MAGIC {
            return Err(SnapshotError("not a skeleton snapshot".to_string()));
        }
        let format = reader.u32()?;
        if format != SNAPSHOT_FORMAT {
            return Err(SnapshotError(format!("snapshot format {format}, this core reads {SNAPSHOT_FORMAT}")));
        }
        let workload = reader.u32()?;
        if workload != VERSION {
            return Err(SnapshotError(format!("workload version {workload}, this core runs {VERSION}")));
        }
        let seed = reader.u64()?;
        let entity_count = reader.u32()? as usize;
        let tick = reader.u32()?;
        let wander = match reader.u8()? {
            0 => false,
            1 => true,
            other => return Err(SnapshotError(format!("wander flag {other}"))),
        };
        let mut state = State {
            generation: vec![0; entity_count],
            alive: vec![false; entity_count],
            x: vec![0.0; entity_count],
            y: vec![0.0; entity_count],
            vx: vec![0.0; entity_count],
            vy: vec![0.0; entity_count],
            mass: vec![0.0; entity_count],
            rng: reference_workload::SplitMix64::new(0),
            energy: 0.0,
        };
        for slot in 0..entity_count {
            state.generation[slot] = reader.u32()?;
            state.alive[slot] = match reader.u8()? {
                0 => false,
                1 => true,
                other => return Err(SnapshotError(format!("alive flag {other} at slot {slot}"))),
            };
            if state.alive[slot] {
                state.x[slot] = reader.f32()?;
                state.y[slot] = reader.f32()?;
                state.vx[slot] = reader.f32()?;
                state.vy[slot] = reader.f32()?;
                state.mass[slot] = reader.f32()?;
            }
        }
        state.energy = reader.f32()?;
        state.rng.state = reader.u64()?;
        if reader.at != self.bytes.len() {
            return Err(SnapshotError(format!("{} trailing bytes", self.bytes.len() - reader.at)));
        }
        Ok((SnapshotHeader { seed, entity_count, tick, wander }, state))
    }

    /// The tick the snapshot was taken at, read from the header.
    pub fn tick(&self) -> Result<u32, SnapshotError> {
        self.decode().map(|(header, _)| header.tick)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], SnapshotError> {
        let end = self.at.checked_add(len).filter(|&end| end <= self.bytes.len());
        let Some(end) = end else {
            return Err(SnapshotError(format!("truncated at byte {}", self.at)));
        };
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    fn f32(&mut self) -> Result<f32, SnapshotError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }
}

// ---------------------------------------------------------------------------
// The tick graph (a trimmed copy of the scheduling overhead spike's counter graph)
// ---------------------------------------------------------------------------

/// A raw view of one `f32` column, taken once while the column's owner is
/// exclusively borrowed and never resized afterwards. Chunks write disjoint
/// slices of it without forming a reference to the whole column: this is the
/// one `unsafe` construct of the spike, the split of a column into disjoint
/// mutable chunk slices.
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
    unsafe fn slices(&self, range: Range<usize>) -> (&mut [f32], &mut [f32], &mut [f32], &mut [f32]) {
        // SAFETY: forwarded to the caller.
        unsafe {
            (
                self.x.slice(range.clone()),
                self.y.slice(range.clone()),
                self.vx.slice(range.clone()),
                self.vy.slice(range),
            )
        }
    }
}

/// One energy sum per group of `CHUNK` slots, written by the motion chunk
/// that owns the group and added in group order at the fixed point: the
/// reduction order of the specification.
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

/// Systems 3 to 6 for one slot, in the specified order.
#[inline]
fn motion_slot(x: f32, y: f32, mut vx: f32, mut vy: f32, mass: f32, tick: u32, i: usize, wander: bool) -> (f32, f32, f32, f32, f32) {
    if wander {
        (vx, vy) = wandered_velocity(vx, vy, tick, i);
    }
    let (x, vx) = bounced(integrated(x, vx), vx);
    let (y, vy) = bounced(integrated(y, vy), vy);
    (x, y, vx, vy, kinetic_energy(vx, vy, mass))
}

struct GraphNode {
    chunks: usize,
    next: AtomicUsize,
    finished: AtomicUsize,
    /// Predecessors still to complete (the structural node waits for the
    /// motion pass of its tick and, except for the first tick of a step, for
    /// the hash of the previous tick).
    pending: AtomicUsize,
}

impl GraphNode {
    fn new(chunks: usize) -> GraphNode {
        GraphNode {
            chunks,
            next: AtomicUsize::new(chunks),
            finished: AtomicUsize::new(0),
            pending: AtomicUsize::new(0),
        }
    }
}

/// The tick graph over double-buffered state. The state after tick `t` lives
/// in `states[current]`; the next tick's passes read it and write the other
/// buffer, while the hash of tick `t` reads it too. The structural node runs
/// alone (every other node of its tick and the previous hash completed, the
/// next tick not armed yet) and is the fixed point where the structural
/// changes are applied, the extraction runs and the buffers swap roles.
struct Graph {
    config: WorldConfig,
    states: [UnsafeCell<State>; 2],
    columns: [Columns; 2],
    grid: UnsafeCell<Grid>,
    scratch: EnergyScratch,
    /// The dead slots, sorted, maintained at the fixed point.
    dead: UnsafeCell<Vec<usize>>,
    /// The slots whose liveness, generation or mass the last structural
    /// changes touched: the only slots the other buffer must learn about.
    changed: UnsafeCell<Vec<usize>>,
    current: AtomicUsize,
    nodes: [GraphNode; NODE_COUNT],
    ready: AtomicU32,
    /// The tick whose spatial hashing, passes and structural changes are armed.
    tick: AtomicU32,
    /// The tick whose hash is armed or was the last computed.
    hash_tick: AtomicU32,
    /// The last tick of the step in progress.
    end: AtomicU32,
    done: AtomicBool,
    failed: AtomicBool,
    failure: Mutex<Option<Failure>>,
    /// The hashes of the ticks from `history_start` on; grown before each step.
    history_start: u32,
    tick_hashes: Vec<AtomicU64>,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
    extraction: Option<Arc<dyn Extraction>>,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise, and the structural node runs alone.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(state: State, config: WorldConfig, extraction: Option<Arc<dyn Extraction>>, history_start: u32, initial_hash: u64) -> Graph {
        let mut a = state;
        let mut b = a.clone();
        let columns = [Columns::of(&mut a), Columns::of(&mut b)];
        let n = a.len();
        let chunk_count = n.div_ceil(CHUNK);
        let dead: Vec<usize> = (0..n).filter(|&slot| !a.alive[slot]).collect();
        Graph {
            config,
            states: [UnsafeCell::new(a), UnsafeCell::new(b)],
            columns,
            grid: UnsafeCell::new(Grid::new()),
            scratch: EnergyScratch::new(n),
            dead: UnsafeCell::new(dead),
            changed: UnsafeCell::new(Vec::new()),
            current: AtomicUsize::new(0),
            nodes: [
                GraphNode::new(1),
                GraphNode::new(chunk_count),
                GraphNode::new(chunk_count),
                GraphNode::new(1),
                GraphNode::new(1),
            ],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(history_start),
            hash_tick: AtomicU32::new(history_start),
            end: AtomicU32::new(history_start),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            history_start,
            tick_hashes: vec![AtomicU64::new(initial_hash)],
            parked: (0..config.workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
            extraction,
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

    fn repulsion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        // SAFETY: the grid was built by the completed spatial node and is read only.
        let grid = unsafe { &*self.grid.get() };
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's slots of the next buffer are written by this task only.
        let (_, _, vx, vy) = unsafe { self.columns[next].slices(range.clone()) };
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                let (fx, fy) = repulsion_force(cur, grid, i);
                vx[k] = repelled_velocity(cur.vx[i], fx, cur.mass[i]);
                vy[k] = repelled_velocity(cur.vy[i], fy, cur.mass[i]);
            }
        }
    }

    fn motion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        let tick = self.tick.load(Ordering::Relaxed);
        let wander = self.config.wander;
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
                let (nx, ny, nvx, nvy, e) = motion_slot(cur.x[i], cur.y[i], vx[k], vy[k], cur.mass[i], tick, i, wander);
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
    /// masses and stream up to date, reduces the energy, applies the kills and
    /// spawns, records which slots they touched, swaps the buffers and hands
    /// the state after the tick to the extraction capability, if any.
    fn structural(&self) {
        let cur_index = self.current.load(Ordering::Relaxed);
        let next_index = 1 - cur_index;
        // SAFETY: the structural node runs alone (see the type's documentation).
        let cur = unsafe { &*self.states[cur_index].get() };
        let next = unsafe { &mut *self.states[next_index].get() };
        let changed = unsafe { &mut *self.changed.get() };
        let dead = unsafe { &mut *self.dead.get() };
        for &slot in changed.iter() {
            next.alive[slot] = cur.alive[slot];
            next.generation[slot] = cur.generation[slot];
            next.mass[slot] = cur.mass[slot];
        }
        next.rng = cur.rng;
        next.energy = self.scratch.reduce();
        // The kills are the next draws of the stream; the spawns fill the
        // lowest dead slots after them. Both are known before the call.
        let n = next.len();
        let mut peek = next.rng;
        changed.clear();
        for _ in 0..KILLS_PER_TICK {
            changed.push((peek.next() % n as u64) as usize);
        }
        next.structural_changes();
        dead.extend_from_slice(changed);
        dead.sort_unstable();
        dead.dedup();
        let spawned = SPAWNS_PER_TICK.min(dead.len());
        changed.extend(dead.drain(..spawned));
        self.current.store(next_index, Ordering::Relaxed);
        if let Some(extraction) = &self.extraction {
            extraction.extract(self.tick.load(Ordering::Relaxed), next);
        }
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed);
        let index = (tick - self.history_start) as usize;
        self.tick_hashes[index].store(self.current_state().hash(), Ordering::Relaxed);
    }

    fn node_tick(&self, node: usize) -> u32 {
        if node == NODE_HASH { self.hash_tick.load(Ordering::Relaxed) } else { self.tick.load(Ordering::Relaxed) }
    }

    fn execute(&self, node: usize, chunk: usize) {
        if let Some(injection) = self.config.injection
            && injection.node.index() == node
            && injection.chunk == chunk
            && injection.tick == self.node_tick(node)
        {
            panic!("injected panic in {} chunk {chunk} of tick {}", NODE_NAMES[node], injection.tick);
        }
        match node {
            NODE_SPATIAL => self.spatial(),
            NODE_REPULSION => self.repulsion_chunk(chunk),
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
            NODE_MOTION => self.release_structural(),
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
                    self.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
                    self.arm(NODE_SPATIAL);
                    self.arm(NODE_HASH);
                    // The arming worker takes one of the two serial nodes; one more worker is needed.
                    self.wake(1);
                } else {
                    self.arm(NODE_HASH);
                }
            }
            _ => unreachable!("five nodes"),
        }
    }

    fn has_work(&self) -> bool {
        let mask = self.ready.load(Ordering::SeqCst);
        mask != 0
            && (0..NODE_COUNT).any(|node| {
                mask & (1 << node) != 0 && self.nodes[node].next.load(Ordering::Relaxed) < self.nodes[node].chunks
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
            if spins % 32 == 0 && started.elapsed() >= self.config.spin_bound {
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

    /// Runs the ticks after `hash_tick` up to `end` on the calling thread and
    /// `workers - 1` spawned threads, joined before returning.
    fn run_step(&mut self, end: u32) -> Duration {
        let from = self.hash_tick.load(Ordering::Relaxed);
        assert!(end > from, "a step runs at least one tick");
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        self.tick_hashes.resize_with((end - self.history_start) as usize + 1, || AtomicU64::new(0));
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
            graph.threads.set(threads).ok().expect("threads are published once per step");
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
// The world and its time control API
// ---------------------------------------------------------------------------

/// A world: the reference workload's state, its tick graph and its history of
/// tick hashes, stepped by whole ticks.
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
            .field("history_start", &self.graph.history_start)
            .field("workers", &self.graph.config.workers)
            .field("failure", &self.failure)
            .finish()
    }
}

impl World {
    /// A world at tick 0, initialized by the specification from the seed.
    pub fn new(config: WorldConfig, capabilities: Capabilities) -> World {
        let parameters = Parameters { seed: config.seed, size: config.size, tick_count: 0, wander: config.wander };
        let state = State::initialize(&parameters);
        let initial_hash = state.hash();
        World {
            graph: Graph::new(state, config, capabilities.extraction, 0, initial_hash),
            tick_done: 0,
            stepping: Duration::ZERO,
            failure: None,
        }
    }

    /// A new world at the snapshot's tick, with the snapshot's state.
    pub fn restore(snapshot: &Snapshot, workers: usize, capabilities: Capabilities) -> Result<World, SnapshotError> {
        let (header, state) = snapshot.decode()?;
        let size = Size::ALL
            .iter()
            .copied()
            .find(|size| size.entity_count() == header.entity_count)
            .ok_or_else(|| SnapshotError(format!("{} entities is not a workload size", header.entity_count)))?;
        let config = WorldConfig { seed: header.seed, size, wander: header.wander, ..WorldConfig::new(size, workers) };
        let hash = state.hash();
        Ok(World {
            graph: Graph::new(state, config, capabilities.extraction, header.tick, hash),
            tick_done: header.tick,
            stepping: Duration::ZERO,
            failure: None,
        })
    }

    pub fn config(&self) -> &WorldConfig {
        &self.graph.config
    }

    /// The last completed tick.
    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    /// The first tick whose hash this world knows: 0, or the restored snapshot's tick.
    pub fn history_start(&self) -> u32 {
        self.graph.history_start
    }

    /// The hash of the state after `tick`, for a completed tick in the history.
    pub fn tick_hash(&self, tick: u32) -> Option<u64> {
        if tick < self.graph.history_start || tick > self.tick_done {
            return None;
        }
        Some(self.graph.tick_hashes[(tick - self.graph.history_start) as usize].load(Ordering::Relaxed))
    }

    /// The hash of the current state.
    pub fn hash(&self) -> u64 {
        self.tick_hash(self.tick_done).expect("the current tick is in the history")
    }

    /// The wall time spent inside `step` so far.
    pub fn stepping(&self) -> Duration {
        self.stepping
    }

    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Advances the world by `ticks` ticks on the calling thread and the
    /// configured workers. A failed world is never stepped again: the call
    /// returns the failure at once.
    pub fn step(&mut self, ticks: u32) -> Result<(), WorldFailed> {
        if let Some(failure) = &self.failure {
            return Err(WorldFailed(failure.clone()));
        }
        if ticks == 0 {
            return Ok(());
        }
        let end = self.tick_done + ticks;
        self.stepping += self.graph.run_step(end);
        let failure = self.graph.failure.lock().unwrap().clone();
        if let Some(failure) = failure {
            // The failing tick's state is not a result: the history stops before it.
            self.tick_done = failure.tick.saturating_sub(1).max(self.graph.history_start);
            self.graph.tick_hashes.truncate((self.tick_done - self.graph.history_start) as usize + 1);
            self.failure = Some(failure.clone());
            return Err(WorldFailed(failure));
        }
        self.tick_done = end;
        Ok(())
    }

    /// The canonical bytes of the current state, at the current tick boundary.
    pub fn snapshot(&self) -> Result<Snapshot, WorldFailed> {
        if let Some(failure) = &self.failure {
            return Err(WorldFailed(failure.clone()));
        }
        let config = &self.graph.config;
        Ok(Snapshot::encode(self.graph.current_state(), config.seed, self.tick_done, config.wander))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reference_workload::{Run, first_divergent_tick, run_sequential};
    use std::sync::atomic::AtomicU64;

    const TICKS: u32 = 40;

    /// A panic hook that keeps the injected panics out of the test output and
    /// prints every other one, the test's own assertions included.
    fn silence_injected_panics(info: &std::panic::PanicHookInfo<'_>) {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("");
        if !message.contains("injected panic") {
            eprintln!("{info}");
        }
    }

    fn reference(seed: u64) -> Run {
        run_sequential(&Parameters { seed, ..Parameters::new(Size::Small, TICKS, false) })
    }

    fn hashes(world: &World) -> Vec<u64> {
        (world.history_start()..=world.tick()).map(|t| world.tick_hash(t).unwrap()).collect()
    }

    /// The world's history, from its first known tick to its last completed
    /// one, equals the reference's over the same ticks.
    fn assert_matches(world: &World, reference: &Run) {
        let from = world.history_start() as usize;
        let to = world.tick() as usize;
        let ours = hashes(world);
        assert_eq!(first_divergent_tick(&ours, &reference.tick_hashes[from..=to]), None, "from tick {from}");
    }

    #[test]
    fn stepping_by_batches_reproduces_the_sequential_hashes_at_every_worker_count() {
        let reference = reference(DEFAULT_SEED);
        for workers in [1, 2, 3, 8] {
            for batch in [1, 7, 100] {
                let mut world = World::new(WorldConfig::new(Size::Small, workers), Capabilities::default());
                while world.tick() < TICKS {
                    world.step(batch.min(TICKS - world.tick())).unwrap();
                }
                assert_eq!(world.tick(), TICKS);
                assert_matches(&world, &reference);
                assert_eq!(world.hash(), reference.final_hash());
            }
        }
    }

    #[test]
    fn a_world_may_be_stepped_from_another_thread() {
        let reference = reference(DEFAULT_SEED);
        let mut world = World::new(WorldConfig::new(Size::Small, 3), Capabilities::default());
        world.step(10).unwrap();
        thread::scope(|scope| {
            scope.spawn(|| world.step(10).unwrap());
        });
        world.step(TICKS - 20).unwrap();
        assert_matches(&world, &reference);
    }

    #[test]
    fn a_snapshot_restores_into_a_world_that_continues_identically() {
        let reference = reference(DEFAULT_SEED);
        let mut original = World::new(WorldConfig::new(Size::Small, 3), Capabilities::default());
        original.step(17).unwrap();
        let snapshot = original.snapshot().unwrap();
        assert_eq!(snapshot.tick().unwrap(), 17);
        let mut restored = World::restore(&snapshot, 2, Capabilities::default()).unwrap();
        assert_eq!(restored.tick(), 17);
        assert_eq!(restored.history_start(), 17);
        assert_eq!(restored.hash(), original.hash());
        assert_eq!(restored.hash(), reference.tick_hashes[17]);
        assert_eq!(restored.snapshot().unwrap(), snapshot, "the round trip is byte-identical");
        restored.step(TICKS - 17).unwrap();
        original.step(TICKS - 17).unwrap();
        assert_matches(&restored, &reference);
        assert_matches(&original, &reference);
        assert_eq!(restored.snapshot().unwrap(), original.snapshot().unwrap());
        assert_eq!(restored.tick_hash(16), None, "a restored world knows no earlier tick");
    }

    #[test]
    fn a_corrupt_snapshot_is_rejected_with_a_reason() {
        let mut world = World::new(WorldConfig::new(Size::Small, 1), Capabilities::default());
        world.step(3).unwrap();
        let snapshot = world.snapshot().unwrap();
        let mut truncated = snapshot.clone();
        truncated.bytes.truncate(100);
        assert!(World::restore(&truncated, 1, Capabilities::default()).unwrap_err().0.contains("truncated"));
        let mut wrong_magic = snapshot.clone();
        wrong_magic.bytes[0] = b'X';
        assert!(World::restore(&wrong_magic, 1, Capabilities::default()).is_err());
        let mut trailing = snapshot.clone();
        trailing.bytes.push(0);
        assert!(World::restore(&trailing, 1, Capabilities::default()).unwrap_err().0.contains("trailing"));
        let mut bad_flag = snapshot;
        bad_flag.bytes[HEADER_LEN + 4] = 2;
        assert!(World::restore(&bad_flag, 1, Capabilities::default()).unwrap_err().0.contains("alive flag"));
    }

    #[test]
    fn two_worlds_stepped_alternately_are_isolated() {
        let seed_b = 0x4c75_6465_6e65_6132;
        let reference_a = reference(DEFAULT_SEED);
        let reference_b = reference(seed_b);
        let mut a = World::new(WorldConfig::new(Size::Small, 3), Capabilities::default());
        let mut b = World::new(WorldConfig::new(Size::Small, 2).seeded(seed_b), Capabilities::default());
        while a.tick() < TICKS {
            a.step(5).unwrap();
            b.step(5).unwrap();
        }
        assert_matches(&a, &reference_a);
        assert_matches(&b, &reference_b);
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn a_panic_fails_its_world_and_leaves_the_other_stepping() {
        let reference = reference(DEFAULT_SEED);
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(silence_injected_panics));
        for (node, chunk) in [(Node::Repulsion, 1), (Node::Hash, 0), (Node::StructuralChanges, 0), (Node::SpatialHashing, 0)] {
            let injection = Injection { tick: 13, node, chunk };
            let mut healthy = World::new(WorldConfig::new(Size::Small, 3), Capabilities::default());
            let mut failing = World::new(WorldConfig::new(Size::Small, 3).injecting(injection), Capabilities::default());
            let mut failed_at = None;
            while healthy.tick() < TICKS {
                healthy.step(5).unwrap();
                if failed_at.is_none()
                    && let Err(WorldFailed(failure)) = failing.step(5)
                {
                    assert_eq!((failure.tick, failure.node, failure.chunk), (13, node.name(), chunk));
                    assert!(failure.message.contains("injected panic"), "{failure:?}");
                    assert!(failure.worker < 3);
                    failed_at = Some(healthy.tick());
                }
            }
            assert_eq!(failed_at, Some(15), "{node:?}");
            assert_eq!(failing.tick(), 12, "the history stops at the last good tick");
            assert_matches(&failing, &reference);
            assert!(failing.step(1).is_err(), "a failed world is never stepped again");
            assert!(failing.snapshot().is_err());
            assert_eq!(failing.tick(), 12);
            assert_matches(&healthy, &reference);
        }
        std::panic::set_hook(previous_hook);
    }

    #[test]
    fn the_extraction_runs_at_every_fixed_point_and_changes_nothing() {
        struct Counter(AtomicU64, AtomicU64);
        impl Extraction for Counter {
            fn extract(&self, tick: u32, state: &State) {
                self.0.fetch_add(1, Ordering::Relaxed);
                self.1.fetch_max(u64::from(tick), Ordering::Relaxed);
                assert_eq!(state.len(), Size::Small.entity_count());
            }
        }
        let reference = reference(DEFAULT_SEED);
        let counter = Arc::new(Counter(AtomicU64::new(0), AtomicU64::new(0)));
        let capabilities = Capabilities { extraction: Some(counter.clone()) };
        let mut world = World::new(WorldConfig::new(Size::Small, 3), capabilities);
        world.step(TICKS).unwrap();
        assert_eq!(counter.0.load(Ordering::Relaxed), u64::from(TICKS));
        assert_eq!(counter.1.load(Ordering::Relaxed), u64::from(TICKS));
        assert_matches(&world, &reference);
    }

    #[test]
    fn the_wander_system_runs_with_the_right_tick_when_stepped_by_batches() {
        let reference = run_sequential(&Parameters::new(Size::Small, TICKS, true));
        let config = WorldConfig { wander: true, ..WorldConfig::new(Size::Small, 3) };
        let mut world = World::new(config, Capabilities::default());
        while world.tick() < TICKS {
            world.step(7.min(TICKS - world.tick())).unwrap();
        }
        assert_matches(&world, &reference);
        let snapshot = world.snapshot().unwrap();
        let restored = World::restore(&snapshot, 1, Capabilities::default()).unwrap();
        assert!(restored.config().wander);
    }
}
