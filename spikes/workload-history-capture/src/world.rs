//! The world of the history capture spike. Throwaway, never product code.
//!
//! The control surface toy spike's world (the reference workload on the
//! dependency-counting tick graph, with the input fixed point that applies a
//! tick's record of kicks before its first system) plus what the capture
//! needs: the snapshot of the runtime skeleton spike (the canonical bytes of a
//! tick boundary, restored into a new world), the wall-bounce events of the
//! frame allocator spike (recorded by the motion chunks, merged in chunk order
//! at the fixed point, returned per tick), and an instrumentation of one
//! watched entity for the harness world of a causal query: after every node
//! of every tick, the watched entity's row is read once, with the kicks that
//! reached it, the neighbors that repelled it and the walls it bounced on.

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

pub use reference_workload::{DEFAULT_SEED, Parameters, Size, State};
use reference_workload::{
    CHUNK, GRID_SIDE, Grid, INTERACTION_RADIUS, KILLS_PER_TICK, SPAWNS_PER_TICK, SPEED_LIMIT, VERSION, WORLD_SIDE,
    bounced, chunk_range, integrated, kinetic_energy, repelled_velocity, repulsion_force, wandered_velocity,
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

const SNAPSHOT_MAGIC: &[u8; 4] = b"LDHC";
const SNAPSHOT_FORMAT: u32 = 1;

/// A velocity impulse on one entity, the one domain input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kick {
    pub index: u32,
    pub generation: u32,
    pub delta_vx: f32,
    pub delta_vy: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KickOutcome {
    Applied,
    StaleHandle,
    UnknownIndex,
}

#[inline]
pub fn kicked_velocity(v: f32, delta: f32) -> f32 {
    (v + delta).clamp(-SPEED_LIMIT, SPEED_LIMIT)
}

pub fn validate_kicks(state: &State, kicks: &[Kick]) -> (Vec<KickOutcome>, Vec<(usize, f32, f32)>) {
    let mut outcomes = Vec::with_capacity(kicks.len());
    let mut applied = Vec::with_capacity(kicks.len());
    for kick in kicks {
        let slot = kick.index as usize;
        let outcome = if slot >= state.len() {
            KickOutcome::UnknownIndex
        } else if !state.alive[slot] || state.generation[slot] != kick.generation {
            KickOutcome::StaleHandle
        } else {
            applied.push((slot, kick.delta_vx, kick.delta_vy));
            KickOutcome::Applied
        };
        outcomes.push(outcome);
    }
    (outcomes, applied)
}

/// A wall bounce: the slot and the axis (0 for x, 1 for y), packed as the
/// frame allocator spike packed it.
pub type BounceEvent = u32;

pub fn bounce_event(slot: usize, axis: u32) -> BounceEvent {
    ((slot as u32) << 1) | axis
}

#[derive(Clone, Copy, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub size: Size,
    pub wander: bool,
    pub workers: usize,
    pub spin_bound: Duration,
    /// The slot whose row the instrumentation reads after every node.
    pub watch: Option<usize>,
}

impl WorldConfig {
    pub fn new(size: Size, workers: usize) -> WorldConfig {
        assert!(workers >= 1, "a world is stepped by at least the calling thread");
        WorldConfig { seed: DEFAULT_SEED, size, wander: false, workers, spin_bound: DEFAULT_SPIN_BOUND, watch: None }
    }

    #[cfg(test)]
    pub fn watching(self, slot: usize) -> WorldConfig {
        WorldConfig { watch: Some(slot), ..self }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub tick: u32,
    pub node: &'static str,
    pub chunk: usize,
    pub worker: usize,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct WorldFailed(pub Failure);

impl fmt::Display for WorldFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "world failed at tick {} in {} chunk {} on worker {}: {}", self.0.tick, self.0.node, self.0.chunk, self.0.worker, self.0.message)
    }
}

impl std::error::Error for WorldFailed {}

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
        let mut reader = ByteReader { bytes: &self.bytes, at: 0 };
        if reader.take(4)? != SNAPSHOT_MAGIC {
            return Err(SnapshotError("not a history capture snapshot".to_string()));
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

    #[cfg(test)]
    pub fn tick(&self) -> Result<u32, SnapshotError> {
        self.decode().map(|(header, _)| header.tick)
    }
}

struct ByteReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl ByteReader<'_> {
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
// The instrumentation of one watched entity
// ---------------------------------------------------------------------------

/// The watched entity's row at a point of a tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Row {
    pub generation: u32,
    pub alive: bool,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

/// What one replayed tick did to the watched entity, as the instrumented
/// harness world records it after every node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    pub tick: u32,
    /// The row at the tick boundary before the tick.
    pub before: Row,
    /// The kicks the input fixed point applied to the entity, in record order.
    pub kicks: Vec<(f32, f32)>,
    /// The slots within the interaction radius after the spatial hashing: the
    /// entities whose positions the repulsion read for this one.
    pub neighbors: Vec<u32>,
    /// The velocity the repulsion wrote.
    pub after_repulsion: (f32, f32),
    /// The row the motion pass wrote, and the walls it bounced on.
    pub after_motion: Row,
    pub bounced_x: bool,
    pub bounced_y: bool,
    /// The row at the tick boundary after the fixed point (the entity may
    /// have been killed or the slot respawned there).
    pub after: Row,
}

// ---------------------------------------------------------------------------
// The tick graph
// ---------------------------------------------------------------------------

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

/// Systems 3 to 6 for one slot, and whether each axis bounced on a wall.
#[inline]
fn motion_slot(x: f32, y: f32, mut vx: f32, mut vy: f32, mass: f32, tick: u32, i: usize, wander: bool) -> (f32, f32, f32, f32, f32, bool, bool) {
    if wander {
        (vx, vy) = wandered_velocity(vx, vy, tick, i);
    }
    let ix = integrated(x, vx);
    let iy = integrated(y, vy);
    let bx = ix < 0.0 || ix >= WORLD_SIDE;
    let by = iy < 0.0 || iy >= WORLD_SIDE;
    let (x, vx) = bounced(ix, vx);
    let (y, vy) = bounced(iy, vy);
    (x, y, vx, vy, kinetic_energy(vx, vy, mass), bx, by)
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

#[derive(Default)]
struct StepInputs {
    records: Vec<Vec<Kick>>,
    outcomes: Vec<Vec<KickOutcome>>,
    applied: Vec<(usize, f32, f32)>,
    kicked: Vec<bool>,
}


struct Graph {
    config: WorldConfig,
    states: [UnsafeCell<State>; 2],
    columns: [Columns; 2],
    grid: UnsafeCell<Grid>,
    scratch: EnergyScratch,
    dead: UnsafeCell<Vec<usize>>,
    changed: UnsafeCell<Vec<usize>>,
    inputs: UnsafeCell<StepInputs>,
    /// The event buffer of each chunk for the tick in progress, written by
    /// the motion task of that chunk.
    chunk_events: Vec<UnsafeCell<Vec<BounceEvent>>>,
    /// The merged events of the completed ticks of the step.
    tick_events: UnsafeCell<Vec<Vec<BounceEvent>>>,
    traces: UnsafeCell<Vec<Trace>>,
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
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise; the inputs are written by the structural node alone and
// read by the repulsion pass it armed; each chunk's event buffer is written
// by the motion task of that chunk and read at the fixed point; the traces
// are written at the completion of a node, before its successor is armed;
// and the structural node runs alone.
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
            scratch: EnergyScratch::new(n),
            dead: UnsafeCell::new(dead),
            changed: UnsafeCell::new(Vec::new()),
            inputs: UnsafeCell::new(StepInputs { kicked: vec![false; n], ..StepInputs::default() }),
            chunk_events: (0..chunk_count).map(|_| UnsafeCell::new(Vec::with_capacity(2 * CHUNK))).collect(),
            tick_events: UnsafeCell::new(Vec::new()),
            traces: UnsafeCell::new(Vec::new()),
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
            parked: (0..config.workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
        }
    }

    fn current_state(&self) -> &State {
        // SAFETY: the current buffer is only read until the structural node runs alone.
        unsafe { &*self.states[self.current.load(Ordering::Relaxed)].get() }
    }

    fn row(state: &State, slot: usize) -> Row {
        Row { generation: state.generation[slot], alive: state.alive[slot], x: state.x[slot], y: state.y[slot], vx: state.vx[slot], vy: state.vy[slot] }
    }

    /// The input fixed point for `tick`, run alone; when an entity is watched,
    /// the trace of the tick starts here with the row before the tick and the
    /// kicks that reached it.
    fn install_inputs(&self, tick: u32) {
        // SAFETY: runs alone (see the type's documentation).
        let inputs = unsafe { &mut *self.inputs.get() };
        for &(slot, _, _) in &inputs.applied {
            inputs.kicked[slot] = false;
        }
        inputs.applied.clear();
        let offset = (tick - self.tick_start()) as usize;
        if let Some(record) = inputs.records.get(offset) {
            let (outcomes, applied) = validate_kicks(self.current_state(), record);
            for &(slot, _, _) in &applied {
                inputs.kicked[slot] = true;
            }
            inputs.applied = applied;
            inputs.outcomes.push(outcomes);
        }
        if let Some(watch) = self.config.watch {
            let traces = unsafe { &mut *self.traces.get() };
            let state = self.current_state();
            traces.push(Trace {
                tick,
                before: Graph::row(state, watch),
                kicks: inputs.applied.iter().filter(|(slot, _, _)| *slot == watch).map(|&(_, dvx, dvy)| (dvx, dvy)).collect(),
                ..Trace::default()
            });
        }
    }

    fn tick_start(&self) -> u32 {
        // SAFETY: `inputs.records` is sized at the step start and read only after.
        let inputs = unsafe { &*self.inputs.get() };
        self.end.load(Ordering::Relaxed) - inputs.records.len() as u32 + 1
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
        // this task only, the chunk owns its energy group and its event buffer.
        let (x, y, vx, vy) = unsafe { self.columns[next].slices(range.clone()) };
        let sum_slot = unsafe { &mut self.scratch.groups.slice(chunk..chunk + 1)[0] };
        let events = unsafe { &mut *self.chunk_events[chunk].get() };
        events.clear();
        let mut sum = 0.0_f32;
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                let (nx, ny, nvx, nvy, e, bx, by) = motion_slot(cur.x[i], cur.y[i], vx[k], vy[k], cur.mass[i], tick, i, wander);
                x[k] = nx;
                y[k] = ny;
                vx[k] = nvx;
                vy[k] = nvy;
                sum += e;
                if bx {
                    events.push(bounce_event(i, 0));
                }
                if by {
                    events.push(bounce_event(i, 1));
                }
            }
        }
        *sum_slot = sum;
    }

    /// The fixed point: merges the tick's events in chunk order, brings the
    /// next buffer up to date, reduces the energy, applies the kills and
    /// spawns, swaps the buffers and installs the next tick's inputs.
    fn structural(&self) {
        let cur_index = self.current.load(Ordering::Relaxed);
        let next_index = 1 - cur_index;
        // SAFETY: the structural node runs alone (see the type's documentation).
        let cur = unsafe { &*self.states[cur_index].get() };
        let next = unsafe { &mut *self.states[next_index].get() };
        let changed = unsafe { &mut *self.changed.get() };
        let dead = unsafe { &mut *self.dead.get() };
        let tick_events = unsafe { &mut *self.tick_events.get() };
        let mut merged = Vec::new();
        for chunk in &self.chunk_events {
            merged.extend_from_slice(unsafe { &*chunk.get() });
        }
        tick_events.push(merged);
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
        if let Some(watch) = self.config.watch {
            let traces = unsafe { &mut *self.traces.get() };
            if let Some(trace) = traces.last_mut() {
                trace.after = Graph::row(next, watch);
            }
        }
        let tick = self.tick.load(Ordering::Relaxed);
        if tick < self.end.load(Ordering::Relaxed) {
            self.install_inputs(tick + 1);
        }
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed);
        let index = (tick - self.history_start) as usize;
        self.tick_hashes[index].store(self.current_state().hash(), Ordering::Relaxed);
    }

    /// The instrumentation, at the completion of a node and before its
    /// successor is armed: the watched entity's neighbors after the spatial
    /// hashing, its velocity after the repulsion, its row and bounces after
    /// the motion.
    fn observe(&self, node: usize) {
        let Some(watch) = self.config.watch else { return };
        // SAFETY: the node completed and its successor is not armed yet: the
        // buffers it wrote are stable, and the traces are this thread's alone.
        let traces = unsafe { &mut *self.traces.get() };
        let Some(trace) = traces.last_mut() else { return };
        let cur = self.current_state();
        let next = 1 - self.current.load(Ordering::Relaxed);
        match node {
            NODE_SPATIAL => {
                if cur.alive[watch] {
                    let grid = unsafe { &*self.grid.get() };
                    trace.neighbors = neighbors(cur, grid, watch);
                }
            }
            NODE_REPULSION => {
                let (_, _, vx, vy) = unsafe { self.columns[next].slices(watch..watch + 1) };
                trace.after_repulsion = (vx[0], vy[0]);
            }
            NODE_MOTION => {
                let (x, y, vx, vy) = unsafe { self.columns[next].slices(watch..watch + 1) };
                trace.after_motion = Row { generation: cur.generation[watch], alive: cur.alive[watch], x: x[0], y: y[0], vx: vx[0], vy: vy[0] };
                let events = unsafe { &*self.chunk_events[watch / CHUNK].get() };
                trace.bounced_x = events.contains(&bounce_event(watch, 0));
                trace.bounced_y = events.contains(&bounce_event(watch, 1));
            }
            _ => {}
        }
    }

    fn node_tick(&self, node: usize) -> u32 {
        if node == NODE_HASH { self.hash_tick.load(Ordering::Relaxed) } else { self.tick.load(Ordering::Relaxed) }
    }

    fn execute(&self, node: usize, chunk: usize) {
        match node {
            NODE_SPATIAL => self.spatial(),
            NODE_REPULSION => self.repulsion_chunk(chunk),
            NODE_MOTION => self.motion_chunk(chunk),
            NODE_STRUCTURAL => self.structural(),
            NODE_HASH => self.hash(),
            _ => unreachable!("five nodes"),
        }
    }

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

    fn complete(&self, node: usize) {
        self.ready.fetch_and(!(1 << node), Ordering::SeqCst);
        self.observe(node);
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
        mask != 0 && (0..NODE_COUNT).any(|node| mask & (1 << node) != 0 && self.nodes[node].next.load(Ordering::Relaxed) < self.nodes[node].chunks)
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

    fn run_step(&mut self, end: u32, records: Vec<Vec<Kick>>) -> Duration {
        let from = self.hash_tick.load(Ordering::Relaxed);
        assert!(end > from, "a step runs at least one tick");
        assert_eq!(records.len() as u32, end - from, "one input record per tick of the step");
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        self.tick_hashes.resize_with((end - self.history_start) as usize + 1, || AtomicU64::new(0));
        self.threads.take();
        {
            let inputs = self.inputs.get_mut();
            inputs.records = records;
            inputs.outcomes.clear();
            self.tick_events.get_mut().clear();
            self.traces.get_mut().clear();
        }
        self.install_inputs(from + 1);
        let graph: &Graph = self;
        let started = Instant::now();
        thread::scope(|scope| {
            let handles: Vec<_> = (1..graph.config.workers)
                .map(|worker| thread::Builder::new().name(format!("worker-{worker}")).spawn_scoped(scope, move || graph.worker_loop(worker)).expect("spawn a worker"))
                .collect();
            let mut threads = vec![thread::current()];
            threads.extend(handles.iter().map(|handle| handle.thread().clone()));
            graph.threads.set(threads).ok().expect("threads are published once per step");
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

/// The slots within the interaction radius of `i`, in the visit order of the
/// repulsion (the y offset outer, the x offset inner, slots in index order).
fn neighbors(state: &State, grid: &Grid, i: usize) -> Vec<u32> {
    let (xi, yi) = (state.x[i], state.y[i]);
    let (cx, cy) = Grid::cell_of(xi, yi);
    let mut found = Vec::new();
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
                if d2 < INTERACTION_RADIUS * INTERACTION_RADIUS && d2 > 0.0 {
                    found.push(j as u32);
                }
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------
// The world and its time control API
// ---------------------------------------------------------------------------

/// What a step returns: the outcomes of every tick's kicks and the wall
/// bounces of every tick, in tick order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepOutput {
    pub outcomes: Vec<Vec<KickOutcome>>,
    pub events: Vec<Vec<BounceEvent>>,
}

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
    pub fn new(config: WorldConfig) -> World {
        let parameters = Parameters { seed: config.seed, size: config.size, tick_count: 0, wander: config.wander };
        let state = State::initialize(&parameters);
        let initial_hash = state.hash();
        World { graph: Graph::new(state, config, 0, initial_hash), tick_done: 0, stepping: Duration::ZERO, failure: None }
    }

    /// A new world at the snapshot's tick, with the snapshot's state, stepped
    /// by `workers` and watching `watch` when the caller asks.
    pub fn restore(snapshot: &Snapshot, workers: usize, watch: Option<usize>) -> Result<World, SnapshotError> {
        let (header, state) = snapshot.decode()?;
        let size = Size::ALL
            .iter()
            .copied()
            .find(|size| size.entity_count() == header.entity_count)
            .ok_or_else(|| SnapshotError(format!("{} entities is not a workload size", header.entity_count)))?;
        if let Some(slot) = watch
            && slot >= header.entity_count
        {
            return Err(SnapshotError(format!("slot {slot} is outside the {} entities", header.entity_count)));
        }
        let config = WorldConfig { seed: header.seed, size, wander: header.wander, watch, ..WorldConfig::new(size, workers) };
        let hash = state.hash();
        Ok(World { graph: Graph::new(state, config, header.tick, hash), tick_done: header.tick, stepping: Duration::ZERO, failure: None })
    }

    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    pub fn history_start(&self) -> u32 {
        self.graph.history_start
    }

    pub fn tick_hash(&self, tick: u32) -> Option<u64> {
        if tick < self.graph.history_start || tick > self.tick_done {
            return None;
        }
        Some(self.graph.tick_hashes[(tick - self.graph.history_start) as usize].load(Ordering::Relaxed))
    }

    pub fn hash(&self) -> u64 {
        self.tick_hash(self.tick_done).expect("the current tick is in the history")
    }

    pub fn stepping(&self) -> Duration {
        self.stepping
    }

    pub fn state(&self) -> &State {
        self.graph.current_state()
    }

    /// Advances the world by `records.len()` ticks, each tick taking its
    /// record of kicks as inputs at its boundary.
    pub fn step(&mut self, records: Vec<Vec<Kick>>) -> Result<StepOutput, WorldFailed> {
        if let Some(failure) = &self.failure {
            return Err(WorldFailed(failure.clone()));
        }
        if records.is_empty() {
            return Ok(StepOutput::default());
        }
        let end = self.tick_done + records.len() as u32;
        self.stepping += self.graph.run_step(end, records);
        let failure = self.graph.failure.lock().unwrap().clone();
        if let Some(failure) = failure {
            self.tick_done = failure.tick.saturating_sub(1).max(self.graph.history_start);
            self.graph.tick_hashes.truncate((self.tick_done - self.graph.history_start) as usize + 1);
            self.failure = Some(failure.clone());
            return Err(WorldFailed(failure));
        }
        self.tick_done = end;
        Ok(StepOutput { outcomes: std::mem::take(&mut self.graph.inputs.get_mut().outcomes), events: std::mem::take(self.graph.tick_events.get_mut()) })
    }

    /// The traces of the watched entity for the ticks of the last step.
    pub fn take_traces(&mut self) -> Vec<Trace> {
        std::mem::take(self.graph.traces.get_mut())
    }

    pub fn snapshot(&self) -> Result<Snapshot, WorldFailed> {
        if let Some(failure) = &self.failure {
            return Err(WorldFailed(failure.clone()));
        }
        let config = &self.graph.config;
        Ok(Snapshot::encode(self.graph.current_state(), config.seed, self.tick_done, config.wander))
    }
}

// ---------------------------------------------------------------------------
// The sequential oracle
// ---------------------------------------------------------------------------

/// The reference workload stepped sequentially with the same records of kicks
/// by the reference crate's systems, with the wall bounces of every tick
/// listed in slot order: the ground truth the graph is compared with.
#[cfg(test)]
pub fn oracle(config: &WorldConfig, records: &[Vec<Kick>]) -> (Vec<u64>, Vec<Vec<BounceEvent>>) {
    let parameters = Parameters { seed: config.seed, size: config.size, tick_count: records.len() as u32, wander: config.wander };
    let mut state = State::initialize(&parameters);
    let mut grid = Grid::new();
    let mut timing = reference_workload::Timing::new(&reference_workload::SEQUENTIAL_PHASES);
    let mut hashes = vec![state.hash()];
    let mut events = Vec::with_capacity(records.len());
    for (offset, record) in records.iter().enumerate() {
        let (_, applied) = validate_kicks(&state, record);
        for (slot, dvx, dvy) in applied {
            state.vx[slot] = kicked_velocity(state.vx[slot], dvx);
            state.vy[slot] = kicked_velocity(state.vy[slot], dvy);
        }
        let tick = offset as u32 + 1;
        // The bounces of the tick: which alive slots leave the world after
        // the repulsion and the integration, before the boundary system.
        timing.charge("spatial_hashing", || grid.build(&state));
        timing.charge("repulsion", || reference_workload::repulsion(&mut state, &grid));
        if config.wander {
            reference_workload::wander(&mut state, tick);
        }
        let mut tick_events = Vec::new();
        for slot in 0..state.len() {
            if state.alive[slot] {
                if !(0.0..WORLD_SIDE).contains(&integrated(state.x[slot], state.vx[slot])) {
                    tick_events.push(bounce_event(slot, 0));
                }
                if !(0.0..WORLD_SIDE).contains(&integrated(state.y[slot], state.vy[slot])) {
                    tick_events.push(bounce_event(slot, 1));
                }
            }
        }
        reference_workload::integration(&mut state);
        reference_workload::boundary(&mut state);
        reference_workload::energy(&mut state);
        state.structural_changes();
        hashes.push(state.hash());
        events.push(tick_events);
    }
    (hashes, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reference_workload::{first_divergent_tick, run_sequential};

    const TICKS: u32 = 40;

    fn kicks_for(tick: u32) -> Vec<Kick> {
        if tick % 5 == 0 {
            (0..12).map(|i| Kick { index: i, generation: 0, delta_vx: 8.0, delta_vy: -8.0 }).chain([Kick { index: 5_000, generation: 0, delta_vx: 1.0, delta_vy: 1.0 }]).collect()
        } else {
            Vec::new()
        }
    }

    fn records(from: u32, to: u32) -> Vec<Vec<Kick>> {
        (from + 1..=to).map(kicks_for).collect()
    }

    fn hashes(world: &World) -> Vec<u64> {
        (world.history_start()..=world.tick()).map(|t| world.tick_hash(t).unwrap()).collect()
    }

    #[test]
    fn without_kicks_the_world_reproduces_the_sequential_reference() {
        let reference = run_sequential(&Parameters::new(Size::Small, TICKS, false));
        for workers in [1, 3] {
            let mut world = World::new(WorldConfig::new(Size::Small, workers));
            while world.tick() < TICKS {
                world.step(vec![Vec::new(); 7.min(TICKS - world.tick()) as usize]).unwrap();
            }
            assert_eq!(first_divergent_tick(&hashes(&world), &reference.tick_hashes), None);
        }
        let (oracle_hashes, _) = oracle(&WorldConfig::new(Size::Small, 1), &vec![Vec::new(); TICKS as usize]);
        assert_eq!(oracle_hashes, reference.tick_hashes);
    }

    #[test]
    fn the_graph_agrees_with_the_oracle_on_hashes_and_events_at_every_worker_count_and_batch() {
        let config = WorldConfig::new(Size::Small, 1);
        let (oracle_hashes, oracle_events) = oracle(&config, &records(0, TICKS));
        assert!(oracle_events.iter().map(Vec::len).sum::<usize>() > 0, "some entity bounces in 40 ticks");
        for workers in [1, 2, 3, 8] {
            for batch in [1, 7, 40] {
                let mut world = World::new(WorldConfig::new(Size::Small, workers));
                let mut events = Vec::new();
                while world.tick() < TICKS {
                    let to = (world.tick() + batch).min(TICKS);
                    events.extend(world.step(records(world.tick(), to)).unwrap().events);
                }
                assert_eq!(first_divergent_tick(&hashes(&world), &oracle_hashes), None, "{workers} workers, batch {batch}");
                assert_eq!(events, oracle_events, "{workers} workers, batch {batch}");
            }
        }
    }

    #[test]
    fn a_snapshot_restores_into_a_world_that_continues_identically_with_kicks() {
        let config = WorldConfig::new(Size::Small, 3);
        let (oracle_hashes, _) = oracle(&config, &records(0, TICKS));
        let mut original = World::new(config);
        original.step(records(0, 17)).unwrap();
        let snapshot = original.snapshot().unwrap();
        assert_eq!(snapshot.tick().unwrap(), 17);
        let mut restored = World::restore(&snapshot, 2, None).unwrap();
        assert_eq!(restored.hash(), oracle_hashes[17]);
        assert_eq!(restored.snapshot().unwrap(), snapshot);
        restored.step(records(17, TICKS)).unwrap();
        original.step(records(17, TICKS)).unwrap();
        assert_eq!(hashes(&restored), oracle_hashes[17..]);
        assert_eq!(hashes(&original), oracle_hashes);
        let mut truncated = snapshot.clone();
        truncated.bytes.truncate(50);
        assert!(World::restore(&truncated, 1, None).unwrap_err().0.contains("truncated"));
        assert!(World::restore(&snapshot, 1, Some(5_000)).unwrap_err().0.contains("outside"));
    }

    #[test]
    fn a_watched_entity_is_traced_after_every_node_without_changing_the_hashes() {
        let config = WorldConfig::new(Size::Small, 3);
        let (oracle_hashes, _) = oracle(&config, &records(0, TICKS));
        let mut plain = World::new(config);
        plain.step(records(0, TICKS)).unwrap();
        let mut watched = World::new(config.watching(3));
        watched.step(records(0, TICKS)).unwrap();
        assert_eq!(hashes(&watched), hashes(&plain));
        assert_eq!(hashes(&watched), oracle_hashes);
        let traces = watched.take_traces();
        assert_eq!(traces.len(), TICKS as usize);
        assert_eq!(traces[0].tick, 1);
        assert!(plain.take_traces().is_empty());
        let kicked: Vec<&Trace> = traces.iter().filter(|t| !t.kicks.is_empty()).collect();
        assert_eq!(kicked.len(), 8, "slot 3 is kicked at every fifth tick while alive with generation 0");
        assert_eq!(kicked[0].kicks, vec![(8.0, -8.0)]);
        for trace in &traces {
            if trace.before.alive {
                // The repulsion wrote the velocity from the kicked velocity and the
                // neighbors' positions; the motion wrote the position from it.
                let expected_x = bounced(integrated(trace.before.x, trace.after_repulsion.0), trace.after_repulsion.0).0;
                assert_eq!(trace.after_motion.x, expected_x, "tick {}", trace.tick);
                assert_eq!(trace.after_motion.generation, trace.before.generation);
            }
            assert_eq!(trace.after.alive || trace.after.generation > trace.before.generation || !trace.before.alive || trace.after.generation == trace.before.generation, true);
        }
        let state = watched.state();
        let last = traces.last().unwrap();
        assert_eq!(last.after, Row { generation: state.generation[3], alive: state.alive[3], x: state.x[3], y: state.y[3], vx: state.vx[3], vy: state.vy[3] });
        // Slot 3 meets nobody in 40 ticks at this density: a slot with a
        // neighbor at tick 0 checks the neighbor list against the grid.
        let initial = State::initialize(&Parameters::new(Size::Small, 0, false));
        let mut grid = Grid::new();
        grid.build(&initial);
        let (crowded, expected) = (0..initial.len()).map(|slot| (slot, neighbors(&initial, &grid, slot))).find(|(_, n)| !n.is_empty()).expect("some entity has a neighbor at tick 0");
        let mut crowded_world = World::new(config.watching(crowded));
        crowded_world.step(records(0, 1)).unwrap();
        let first = &crowded_world.take_traces()[0];
        assert_eq!(first.neighbors, expected);
        assert_ne!(first.after_repulsion, (first.before.vx, first.before.vy), "a neighbor changes the velocity");
    }
}
