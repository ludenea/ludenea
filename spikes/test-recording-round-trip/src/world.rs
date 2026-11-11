//! The world of the recording round trip spike. Throwaway, never product code.
//!
//! A copy of the persistent pool prototype's world (the reference workload on
//! the dependency-counting tick graph, a job that an executor runs, stepped by
//! whole ticks, snapshot and restore into a new world, a panic caught at the
//! task boundary failing its world) with what a recording needs added:
//!
//! - every tick takes an input record, an ordered list of kicks validated
//!   against the state at the tick boundary and applied in the repulsion
//!   pass, as the control surface toy does, so that a kick's effect is a
//!   function of the tick's record and never of the stepping;
//! - the state hash is computed at a cadence (every tick, or every sixtieth),
//!   so that the cost of an invariant read between one-tick steps is measured
//!   with and without a hash to wait for;
//! - the state at the current tick boundary is readable between steps, which
//!   is what a query and an invariant are served from.
//!
//! The injection of a panic is dropped: no scenario of this spike uses it.

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub use reference_workload::{DEFAULT_SEED, Parameters, Size, State};
use reference_workload::{
    CHUNK, Grid, KILLS_PER_TICK, SPAWNS_PER_TICK, SPEED_LIMIT, VERSION, bounced, chunk_range, integrated, kinetic_energy,
    repelled_velocity, repulsion_force, wandered_velocity,
};

use crate::pool::{Executor, Job, Signal, Task};

const NODE_SPATIAL: usize = 0;
const NODE_REPULSION: usize = 1;
const NODE_MOTION: usize = 2;
const NODE_STRUCTURAL: usize = 3;
const NODE_HASH: usize = 4;
const NODE_COUNT: usize = 5;
const NODE_NAMES: [&str; NODE_COUNT] = ["spatial_hashing", "repulsion", "motion", "structural_changes", "hash"];
/// The order in which a worker looks for claimable work: the next tick's
/// spatial hashing first, then the pending hash, then the passes.
const CLAIM_ORDER: [usize; NODE_COUNT] = [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_STRUCTURAL];

const SNAPSHOT_MAGIC: &[u8; 4] = b"LDRT";
pub const SNAPSHOT_FORMAT: u32 = 1;

/// A velocity impulse on one entity, the one domain input of the spike.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kick {
    pub index: u32,
    pub generation: u32,
    pub delta_vx: f32,
    pub delta_vy: f32,
}

/// The kick applied as the specification would: added, then clamped.
#[inline]
pub fn kicked_velocity(v: f32, delta: f32) -> f32 {
    (v + delta).clamp(-SPEED_LIMIT, SPEED_LIMIT)
}

/// Validates the kicks of one tick against a state at a tick boundary, in
/// record order, and lists the applied ones by slot: a kick whose index is
/// outside the table, or whose entity is dead or of another generation, is
/// dropped, deterministically. Shared by the graph and the sequential oracle
/// so that both decide alike.
pub fn applied_kicks(state: &State, kicks: &[Kick]) -> Vec<(usize, f32, f32)> {
    kicks
        .iter()
        .filter(|kick| {
            let slot = kick.index as usize;
            slot < state.len() && state.alive[slot] && state.generation[slot] == kick.generation
        })
        .map(|kick| (kick.index as usize, kick.delta_vx, kick.delta_vy))
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub size: Size,
    pub wander: bool,
    /// The state hash is computed for the ticks that are multiples of the
    /// cadence, and for the tick a world starts at.
    pub hash_cadence: u32,
}

impl WorldConfig {
    pub fn new(size: Size) -> WorldConfig {
        WorldConfig { seed: DEFAULT_SEED, size, wander: false, hash_cadence: 1 }
    }

    pub fn seeded(self, seed: u64) -> WorldConfig {
        WorldConfig { seed, ..self }
    }

    pub fn hashing_every(self, hash_cadence: u32) -> WorldConfig {
        assert!(hash_cadence >= 1, "a cadence is at least one tick");
        WorldConfig { hash_cadence, ..self }
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
/// values, then the energy and the stream state.
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
            return Err(SnapshotError("not a snapshot of this spike's world".to_string()));
        }
        let format = reader.u32()?;
        if format != SNAPSHOT_FORMAT {
            return Err(SnapshotError(format!("snapshot format {format}, this world reads {SNAPSHOT_FORMAT}")));
        }
        let workload = reader.u32()?;
        if workload != VERSION {
            return Err(SnapshotError(format!("workload version {workload}, this world runs {VERSION}")));
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
/// slices of it: the one `unsafe` construct of the world.
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
        Columns { x: Column::of(&mut state.x), y: Column::of(&mut state.y), vx: Column::of(&mut state.vx), vy: Column::of(&mut state.vy) }
    }

    /// # Safety
    ///
    /// As for [`Column::slice`], for the four columns at once.
    unsafe fn slices(&self, range: Range<usize>) -> (&mut [f32], &mut [f32], &mut [f32], &mut [f32]) {
        // SAFETY: forwarded to the caller.
        unsafe { (self.x.slice(range.clone()), self.y.slice(range.clone()), self.vx.slice(range.clone()), self.vy.slice(range)) }
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

/// The inputs of the ticks of the step in progress, installed one tick at a
/// time at the fixed point.
#[derive(Default)]
struct StepInputs {
    /// The kicks of every tick of the step, by tick offset from the first.
    records: Vec<Vec<Kick>>,
    /// The applied kicks of the armed tick, in record order.
    applied: Vec<(usize, f32, f32)>,
    /// Whether a slot has a kick in the armed tick (sparse clears).
    kicked: Vec<bool>,
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

/// The tick graph over double-buffered state. The state after tick `t` lives
/// in `states[current]`; the next tick's passes read it and write the other
/// buffer, while the hash of tick `t` reads it too. The structural node runs
/// alone and is the fixed point where the structural changes are applied and
/// the buffers swap roles.
///
/// The graph is a job: an executor's workers claim its tasks through
/// [`Job::try_claim`] and run them through [`Job::execute`], which tells the
/// executor what to wake. The graph never touches a thread itself.
struct Graph {
    config: WorldConfig,
    states: [UnsafeCell<State>; 2],
    columns: [Columns; 2],
    grid: UnsafeCell<Grid>,
    inputs: UnsafeCell<StepInputs>,
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
    history_start: u32,
    tick_hashes: Vec<AtomicU64>,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise, and the structural node runs alone.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(state: State, config: WorldConfig, history_start: u32, initial_hash: u64) -> Graph {
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
            inputs: UnsafeCell::new(StepInputs { kicked: vec![false; n], ..StepInputs::default() }),
            scratch: EnergyScratch::new(n),
            dead: UnsafeCell::new(dead),
            changed: UnsafeCell::new(Vec::new()),
            current: AtomicUsize::new(0),
            nodes: [GraphNode::new(1), GraphNode::new(chunk_count), GraphNode::new(chunk_count), GraphNode::new(1), GraphNode::new(1)],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(history_start),
            hash_tick: AtomicU32::new(history_start),
            end: AtomicU32::new(history_start),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            history_start,
            tick_hashes: vec![AtomicU64::new(initial_hash)],
        }
    }

    // --- the kernels ------------------------------------------------------

    fn current_state(&self) -> &State {
        // SAFETY: the current buffer is only read until the structural node runs alone.
        unsafe { &*self.states[self.current.load(Ordering::Relaxed)].get() }
    }

    /// The input fixed point for `tick`: validates its record against the
    /// state at the boundary (the current buffer) and installs the applied
    /// kicks for the repulsion pass. Runs alone: before the step starts, or
    /// when the structural node completes, after the swap.
    fn install_inputs(&self, tick: u32) {
        // SAFETY: runs alone (see the type's documentation).
        let inputs = unsafe { &mut *self.inputs.get() };
        for &(slot, _, _) in &inputs.applied {
            inputs.kicked[slot] = false;
        }
        inputs.applied.clear();
        let first = self.end.load(Ordering::Relaxed) + 1 - inputs.records.len() as u32;
        let Some(record) = inputs.records.get((tick - first) as usize) else { return };
        let applied = applied_kicks(self.current_state(), record);
        for &(slot, _, _) in &applied {
            inputs.kicked[slot] = true;
        }
        inputs.applied = applied;
    }

    fn spatial(&self) {
        // SAFETY: the spatial node is the only user of the grid until it completes.
        let grid = unsafe { &mut *self.grid.get() };
        grid.build(self.current_state());
    }

    fn repulsion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        // SAFETY: the grid was built by the completed spatial node and is read only;
        // the inputs were installed before this node was armed and are read only.
        let grid = unsafe { &*self.grid.get() };
        let inputs = unsafe { &*self.inputs.get() };
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's slots of the next buffer are written by this task only.
        let (_, _, vx, vy) = unsafe { self.columns[next].slices(range.clone()) };
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                let (fx, fy) = repulsion_force(cur, grid, i);
                let (mut vx_in, mut vy_in) = (cur.vx[i], cur.vy[i]);
                if inputs.kicked[i] {
                    for &(slot, dvx, dvy) in &inputs.applied {
                        if slot == i {
                            vx_in = kicked_velocity(vx_in, dvx);
                            vy_in = kicked_velocity(vy_in, dvy);
                        }
                    }
                }
                vx[k] = repelled_velocity(vx_in, fx, cur.mass[i]);
                vy[k] = repelled_velocity(vy_in, fy, cur.mass[i]);
            }
        }
    }

    fn motion_chunk(&self, chunk: usize) {
        let cur = self.current_state();
        let tick = self.tick.load(Ordering::Relaxed);
        let wander = self.config.wander;
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's slots of the next buffer are read and written by
        // this task only, and the chunk owns its energy group.
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
    /// spawns and swaps the buffers.
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
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed);
        let index = (tick - self.history_start) as usize;
        // A tick outside the cadence keeps its node, which computes nothing:
        // the graph's shape does not depend on the cadence.
        let hashed = tick % self.config.hash_cadence == 0 || tick == self.history_start;
        let hash = if hashed { self.current_state().hash() } else { 0 };
        self.tick_hashes[index].store(hash, Ordering::Relaxed);
    }

    fn node_tick(&self, node: usize) -> u32 {
        if node == NODE_HASH { self.hash_tick.load(Ordering::Relaxed) } else { self.tick.load(Ordering::Relaxed) }
    }

    fn run_kernel(&self, node: usize, chunk: usize) {
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
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.run_kernel(node, chunk))) {
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

    fn release_structural(&self) -> Signal {
        if self.nodes[NODE_STRUCTURAL].pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.arm(NODE_STRUCTURAL);
        }
        Signal::Nothing
    }

    fn finish_step(&self) -> Signal {
        self.done.store(true, Ordering::SeqCst);
        Signal::Finished
    }

    /// The last task of `node` has finished: arms what the node releases and
    /// says how many workers the executor should wake for it.
    fn complete(&self, node: usize) -> Signal {
        self.ready.fetch_and(!(1 << node), Ordering::SeqCst);
        match node {
            NODE_SPATIAL => {
                self.arm(NODE_REPULSION);
                Signal::Armed(self.nodes[NODE_REPULSION].chunks)
            }
            NODE_REPULSION => {
                self.arm(NODE_MOTION);
                Signal::Armed(self.nodes[NODE_MOTION].chunks)
            }
            NODE_MOTION => self.release_structural(),
            NODE_HASH => {
                let last = self.hash_tick.load(Ordering::Relaxed) == self.end.load(Ordering::Relaxed);
                if last || self.failed.load(Ordering::SeqCst) { self.finish_step() } else { self.release_structural() }
            }
            NODE_STRUCTURAL => {
                let tick = self.tick.load(Ordering::Relaxed);
                self.hash_tick.store(tick, Ordering::Relaxed);
                if tick < self.end.load(Ordering::Relaxed) && !self.failed.load(Ordering::SeqCst) {
                    self.tick.store(tick + 1, Ordering::Relaxed);
                    self.install_inputs(tick + 1);
                    self.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
                    self.arm(NODE_SPATIAL);
                    self.arm(NODE_HASH);
                    // The completer claims one of the two armed tasks itself.
                    Signal::Armed(1)
                } else {
                    self.arm(NODE_HASH);
                    Signal::Nothing
                }
            }
            _ => unreachable!("five nodes"),
        }
    }

    /// Prepares the ticks after `hash_tick`, one per record, and runs them on
    /// the executor, the calling thread taking part as worker 0 of the step.
    fn run_step(&mut self, records: Vec<Vec<Kick>>, executor: &dyn Executor) {
        let from = self.hash_tick.load(Ordering::Relaxed);
        let end = from + records.len() as u32;
        assert!(end > from, "a step runs at least one tick");
        self.inputs.get_mut().records = records;
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        self.tick_hashes.resize_with((end - self.history_start) as usize + 1, || AtomicU64::new(0));
        // The hash of `from` is already known: the first structural node
        // waits for its motion pass only.
        self.tick.store(from + 1, Ordering::Relaxed);
        self.install_inputs(from + 1);
        self.nodes[NODE_STRUCTURAL].pending.store(1, Ordering::Relaxed);
        self.arm(NODE_SPATIAL);
        executor.run(self);
    }
}

impl Job for Graph {
    fn has_work(&self) -> bool {
        let mask = self.ready.load(Ordering::SeqCst);
        mask != 0
            && (0..NODE_COUNT)
                .any(|node| mask & (1 << node) != 0 && self.nodes[node].next.load(Ordering::Relaxed) < self.nodes[node].chunks)
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    fn try_claim(&self) -> Option<Task> {
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
                    return Some(Task { node, chunk });
                }
            }
        }
        None
    }

    fn execute(&self, task: Task, worker: usize) -> Signal {
        self.run_task(task.node, task.chunk, worker);
        let n = &self.nodes[task.node];
        if n.finished.fetch_add(1, Ordering::AcqRel) + 1 == n.chunks { self.complete(task.node) } else { Signal::Nothing }
    }
}

// ---------------------------------------------------------------------------
// The world and its time control API
// ---------------------------------------------------------------------------

/// A world: the reference workload's state, its tick graph and its history of
/// tick hashes, stepped by whole ticks on the executor each step names.
pub struct World {
    graph: Graph,
    tick_done: u32,
    failure: Option<Failure>,
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World")
            .field("tick", &self.tick_done)
            .field("history_start", &self.graph.history_start)
            .field("failure", &self.failure)
            .finish()
    }
}

impl World {
    /// A world at tick 0, initialized by the specification from the seed.
    pub fn new(config: WorldConfig) -> World {
        let parameters = Parameters { seed: config.seed, size: config.size, tick_count: 0, wander: config.wander };
        let state = State::initialize(&parameters);
        let initial_hash = state.hash();
        World { graph: Graph::new(state, config, 0, initial_hash), tick_done: 0, failure: None }
    }

    /// A new world at the snapshot's tick, with the snapshot's state and the
    /// hash cadence of `config` (whose seed, size and wander flag the snapshot's
    /// header overrides).
    pub fn restore(snapshot: &Snapshot, config: WorldConfig) -> Result<World, SnapshotError> {
        let (header, state) = snapshot.decode()?;
        let size = Size::ALL
            .iter()
            .copied()
            .find(|size| size.entity_count() == header.entity_count)
            .ok_or_else(|| SnapshotError(format!("{} entities is not a workload size", header.entity_count)))?;
        let config = WorldConfig { seed: header.seed, size, wander: header.wander, ..config };
        let hash = state.hash();
        Ok(World { graph: Graph::new(state, config, header.tick, hash), tick_done: header.tick, failure: None })
    }

    /// The last completed tick.
    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    /// The first tick whose hash this world knows: 0, or the restored snapshot's tick.
    pub fn history_start(&self) -> u32 {
        self.graph.history_start
    }

    /// The hash of the state after `tick`, for a completed tick of the
    /// history that the cadence hashed (the starting tick always is).
    pub fn tick_hash(&self, tick: u32) -> Option<u64> {
        let start = self.graph.history_start;
        if tick < start || tick > self.tick_done || (tick != start && tick % self.graph.config.hash_cadence != 0) {
            return None;
        }
        Some(self.graph.tick_hashes[(tick - start) as usize].load(Ordering::Relaxed))
    }

    /// The hashed ticks of the history with their hashes, in tick order.
    pub fn history(&self) -> Vec<(u32, u64)> {
        (self.graph.history_start..=self.tick_done).filter_map(|t| self.tick_hash(t).map(|hash| (t, hash))).collect()
    }

    /// The state at the current tick boundary, readable between steps: what
    /// a query and an invariant are served from.
    pub fn state(&self) -> &State {
        self.graph.current_state()
    }

    /// `ticks` ticks without inputs.
    pub fn step_empty(&mut self, ticks: u32, executor: &dyn Executor) -> Result<(), WorldFailed> {
        self.step(vec![Vec::new(); ticks as usize], executor)
    }

    /// Advances the world by one tick per record, each tick taking its record
    /// of kicks as inputs at its boundary, on the executor, the calling thread
    /// taking part as worker 0. A failed world is never stepped again: the
    /// call returns the failure at once.
    pub fn step(&mut self, records: Vec<Vec<Kick>>, executor: &dyn Executor) -> Result<(), WorldFailed> {
        if let Some(failure) = &self.failure {
            return Err(WorldFailed(failure.clone()));
        }
        if records.is_empty() {
            return Ok(());
        }
        let end = self.tick_done + records.len() as u32;
        self.graph.run_step(records, executor);
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


/// The reference workload stepped sequentially from tick 0 with the same
/// records of kicks, by the reference crate's systems: the ground truth the
/// graph's hashes are compared with. Returns the tick hashes from tick 0.
pub fn oracle(config: &WorldConfig, records: &[Vec<Kick>]) -> Vec<u64> {
    let parameters = Parameters { seed: config.seed, size: config.size, tick_count: records.len() as u32, wander: config.wander };
    let mut state = State::initialize(&parameters);
    let mut grid = Grid::new();
    let mut timing = reference_workload::Timing::new(&reference_workload::SEQUENTIAL_PHASES);
    let mut hashes = vec![state.hash()];
    for (offset, record) in records.iter().enumerate() {
        for (slot, dvx, dvy) in applied_kicks(&state, record) {
            state.vx[slot] = kicked_velocity(state.vx[slot], dvx);
            state.vy[slot] = kicked_velocity(state.vy[slot], dvy);
        }
        reference_workload::tick(&mut state, &mut grid, offset as u32 + 1, config.wander, &mut timing);
        hashes.push(state.hash());
    }
    hashes
}
