//! The world of the editor-rate spike. Throwaway, never product code.
//!
//! A trimmed copy of the control surface toy spike's world: the reference
//! workload stepped on the dependency-counting tick graph (the hash of a tick
//! overlaps the next tick's passes on the other buffer), with the input fixed
//! point. Two things differ from the toy's copy. The inputs of a tick are
//! commands of three kinds (the agent's kick, an editor's `set` of one field,
//! and a transaction of field sets applied atomically with its inverse
//! computed at the boundary), resolved against the state at the tick boundary
//! in record order into one input velocity per touched slot. And the graph
//! owns no threads: a step runs on the calling thread plus the helpers a
//! shared persistent pool lends it (see `pool.rs`).
//!
//! The fields an input may set are the two velocity components: the tick
//! graph reads a slot's velocity in one place (the repulsion pass), so the
//! override stays local, where an override of the position would have to be
//! seen by the spatial hashing and by every neighbour's force.

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

pub use reference_workload::{DEFAULT_SEED, Parameters, Size, State};
use reference_workload::{
    CHUNK, Grid, KILLS_PER_TICK, SPAWNS_PER_TICK, SPEED_LIMIT, bounced, chunk_range, integrated, kinetic_energy,
    repelled_velocity, repulsion_force, wandered_velocity,
};

use crate::pool::{Job, Pool};

pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);

const NODE_SPATIAL: usize = 0;
const NODE_REPULSION: usize = 1;
const NODE_MOTION: usize = 2;
const NODE_STRUCTURAL: usize = 3;
const NODE_HASH: usize = 4;
const NODE_COUNT: usize = 5;
const NODE_NAMES: [&str; NODE_COUNT] = ["spatial_hashing", "repulsion", "motion", "structural_changes", "hash"];
const CLAIM_ORDER: [usize; NODE_COUNT] = [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_STRUCTURAL];

// ---------------------------------------------------------------------------
// Commands and the input fixed point
// ---------------------------------------------------------------------------

/// A field an input may set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    VelocityX,
    VelocityY,
}

/// One field of one entity set to a value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldSet {
    pub index: u32,
    pub generation: u32,
    pub field: Field,
    pub value: f32,
}

/// One element of a tick's input record.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// A velocity impulse, the stepped agent's input.
    Kick { index: u32, generation: u32, delta_vx: f32, delta_vy: f32 },
    /// One field set, the editor's input during a drag.
    Set(FieldSet),
    /// Field sets applied all together or not at all.
    Transaction { sets: Vec<FieldSet>, want_inverse: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// The index exists but the entity there is dead or of another generation.
    StaleHandle,
    /// The index is outside the identifier table.
    UnknownIndex,
    /// The value is not a finite number.
    InvalidValue,
}

/// What the fixed point decided about one command, deterministically.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Applied,
    Rejected(Rejection),
    /// The transaction applied; `inverse` restores the fields it wrote to the
    /// values they held at the boundary, when it was asked for.
    TransactionApplied { inverse: Vec<FieldSet> },
    /// Nothing of the transaction applied; `element` is the first set refused.
    TransactionRejected { element: usize, reason: Rejection },
}

/// The input velocity of one slot for the tick, replacing the state's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Override {
    pub slot: usize,
    pub vx: f32,
    pub vy: f32,
}

fn clamped(v: f32) -> f32 {
    v.clamp(-SPEED_LIMIT, SPEED_LIMIT)
}

fn check_handle(state: &State, index: u32, generation: u32) -> Result<usize, Rejection> {
    let slot = index as usize;
    if slot >= state.len() {
        Err(Rejection::UnknownIndex)
    } else if !state.alive[slot] || state.generation[slot] != generation {
        Err(Rejection::StaleHandle)
    } else {
        Ok(slot)
    }
}

fn check_set(state: &State, set: &FieldSet) -> Result<usize, Rejection> {
    let slot = check_handle(state, set.index, set.generation)?;
    if set.value.is_finite() { Ok(slot) } else { Err(Rejection::InvalidValue) }
}

/// The velocity a slot holds after the commands resolved so far.
fn pending(state: &State, overrides: &[Override], slot: usize) -> (f32, f32) {
    overrides.iter().find(|o| o.slot == slot).map_or((state.vx[slot], state.vy[slot]), |o| (o.vx, o.vy))
}

fn write(overrides: &mut Vec<Override>, slot: usize, vx: f32, vy: f32) {
    match overrides.iter_mut().find(|o| o.slot == slot) {
        Some(existing) => (existing.vx, existing.vy) = (vx, vy),
        None => overrides.push(Override { slot, vx, vy }),
    }
}

fn apply_set(state: &State, overrides: &mut Vec<Override>, slot: usize, set: &FieldSet) -> f32 {
    let (vx, vy) = pending(state, overrides, slot);
    match set.field {
        Field::VelocityX => {
            write(overrides, slot, clamped(set.value), vy);
            vx
        }
        Field::VelocityY => {
            write(overrides, slot, vx, clamped(set.value));
            vy
        }
    }
}

/// The input fixed point: resolves the commands of one tick against the
/// state at the tick boundary, in record order, into the outcome of each
/// command and one input velocity per touched slot. A kick adds and clamps as
/// the specification's velocity clamp would; a set replaces and clamps; a
/// transaction is validated whole before any of its sets is written. Shared
/// by the graph and the sequential oracle so that both decide alike.
pub fn resolve_commands(state: &State, record: &[Command]) -> (Vec<Outcome>, Vec<Override>) {
    let mut outcomes = Vec::with_capacity(record.len());
    let mut overrides: Vec<Override> = Vec::new();
    for command in record {
        let outcome = match command {
            Command::Kick { index, generation, delta_vx, delta_vy } => match check_handle(state, *index, *generation) {
                Ok(slot) => {
                    let (vx, vy) = pending(state, &overrides, slot);
                    write(&mut overrides, slot, clamped(vx + delta_vx), clamped(vy + delta_vy));
                    Outcome::Applied
                }
                Err(reason) => Outcome::Rejected(reason),
            },
            Command::Set(set) => match check_set(state, set) {
                Ok(slot) => {
                    apply_set(state, &mut overrides, slot, set);
                    Outcome::Applied
                }
                Err(reason) => Outcome::Rejected(reason),
            },
            Command::Transaction { sets, want_inverse } => {
                let refused = sets.iter().enumerate().find_map(|(element, set)| check_set(state, set).err().map(|reason| (element, reason)));
                match refused {
                    Some((element, reason)) => Outcome::TransactionRejected { element, reason },
                    None => {
                        let mut inverse = Vec::with_capacity(sets.len());
                        for set in sets {
                            let previous = apply_set(state, &mut overrides, set.index as usize, set);
                            inverse.push(FieldSet { value: previous, ..*set });
                        }
                        // Applied in order, the inverse undoes the last write first.
                        inverse.reverse();
                        Outcome::TransactionApplied { inverse: if *want_inverse { inverse } else { Vec::new() } }
                    }
                }
            }
        };
        outcomes.push(outcome);
    }
    (outcomes, overrides)
}

#[derive(Clone, Copy, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub size: Size,
    pub wander: bool,
    pub workers: usize,
    pub spin_bound: Duration,
}

impl WorldConfig {
    pub fn new(size: Size, workers: usize) -> WorldConfig {
        assert!(workers >= 1, "a world is stepped by at least the calling thread");
        WorldConfig { seed: DEFAULT_SEED, size, wander: false, workers, spin_bound: DEFAULT_SPIN_BOUND }
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

#[derive(Clone, Debug)]
pub struct WorldFailed(pub Failure);

impl fmt::Display for WorldFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "world failed at tick {} in {} chunk {} on worker {}: {}", self.0.tick, self.0.node, self.0.chunk, self.0.worker, self.0.message)
    }
}

impl std::error::Error for WorldFailed {}

// ---------------------------------------------------------------------------
// The tick graph
// ---------------------------------------------------------------------------

/// A raw view of one `f32` column, split into disjoint mutable chunk slices
/// by the passes: the one `unsafe` construct of the world's data.
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
        Columns { x: Column::of(&mut state.x), y: Column::of(&mut state.y), vx: Column::of(&mut state.vx), vy: Column::of(&mut state.vy) }
    }

    /// # Safety
    ///
    /// As for [`Column::slice`], for the four columns at once.
    #[allow(clippy::type_complexity)]
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

#[inline]
#[allow(clippy::too_many_arguments)]
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
    pending: AtomicUsize,
}

impl GraphNode {
    fn new(chunks: usize) -> GraphNode {
        GraphNode { chunks, next: AtomicUsize::new(chunks), finished: AtomicUsize::new(0), pending: AtomicUsize::new(0) }
    }
}

/// The inputs of the ticks of the step in progress, installed one tick at a
/// time at the fixed point, and the outcomes the fixed point decided.
#[derive(Default)]
struct StepInputs {
    /// The commands of every tick of the step, by tick offset from the first.
    records: Vec<Vec<Command>>,
    /// The outcomes per tick, filled as the ticks are installed.
    outcomes: Vec<Vec<Outcome>>,
    /// The input velocities of the armed tick.
    overrides: Vec<Override>,
    /// Whether a slot has an input velocity in the armed tick (sparse clears).
    overridden: Vec<bool>,
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
    /// The thread of each worker of the step in progress, registered by the
    /// worker itself; read only to unpark a worker that announced its park.
    threads: Mutex<Vec<Option<Thread>>>,
    helpers_active: AtomicUsize,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise, the inputs are written by the structural node alone and
// read by the repulsion pass it armed, and the structural node runs alone.
unsafe impl Sync for Graph {}

impl Job for Graph {
    fn helper_joined(&self) {
        self.helpers_active.fetch_add(1, Ordering::SeqCst);
    }

    fn help(&self, worker: usize) {
        self.threads.lock().unwrap()[worker] = Some(thread::current());
        self.worker_loop(worker);
        self.helpers_active.fetch_sub(1, Ordering::SeqCst);
    }

    fn helpers_active(&self) -> usize {
        self.helpers_active.load(Ordering::SeqCst)
    }
}

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
            inputs: UnsafeCell::new(StepInputs { overridden: vec![false; n], ..StepInputs::default() }),
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
            threads: Mutex::new(vec![None; config.workers]),
            helpers_active: AtomicUsize::new(0),
        }
    }

    fn current_state(&self) -> &State {
        // SAFETY: the current buffer is only read until the structural node runs alone.
        unsafe { &*self.states[self.current.load(Ordering::Relaxed)].get() }
    }

    /// The input fixed point for `tick`: resolves its record against the
    /// state at the boundary (the current buffer) and installs the input
    /// velocities for the repulsion pass. Runs alone: before the step starts,
    /// or in the structural node after the swap. The current buffer itself is
    /// not written: the hash of the previous tick still reads it.
    fn install_inputs(&self, tick: u32) {
        // SAFETY: runs alone (see the type's documentation).
        let inputs = unsafe { &mut *self.inputs.get() };
        for o in &inputs.overrides {
            inputs.overridden[o.slot] = false;
        }
        inputs.overrides.clear();
        let offset = (tick - self.tick_start()) as usize;
        let Some(record) = inputs.records.get(offset) else { return };
        let (outcomes, overrides) = resolve_commands(self.current_state(), record);
        for o in &overrides {
            inputs.overridden[o.slot] = true;
        }
        inputs.overrides = overrides;
        inputs.outcomes.push(outcomes);
    }

    /// The first tick of the step in progress.
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
                if inputs.overridden[i]
                    && let Some(o) = inputs.overrides.iter().find(|o| o.slot == i)
                {
                    (vx_in, vy_in) = (o.vx, o.vy);
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

    /// The fixed point: brings the next buffer up to date, reduces the
    /// energy, applies the kills and spawns, swaps the buffers and installs
    /// the next tick's inputs against the state after this tick.
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

    /// Unparks up to `count` workers that announced a park. The lock on the
    /// thread table is taken only when a worker is parked.
    fn wake(&self, count: usize) {
        if count == 0 {
            return;
        }
        let mut woken = 0;
        for (worker, parked) in self.parked.iter().enumerate() {
            if parked.load(Ordering::SeqCst) {
                if let Some(thread) = &self.threads.lock().unwrap()[worker] {
                    thread.unpark();
                }
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

    /// Runs the ticks after `hash_tick` up to `end`, with `records` as the
    /// inputs of those ticks in order, on the calling thread and the helpers
    /// the pool lends, all gone from the graph before returning.
    fn run_step(&mut self, end: u32, records: Vec<Vec<Command>>, pool: &Pool) -> Duration {
        let from = self.hash_tick.load(Ordering::Relaxed);
        assert!(end > from, "a step runs at least one tick");
        assert_eq!(records.len() as u32, end - from, "one input record per tick of the step");
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        self.tick_hashes.resize_with((end - self.history_start) as usize + 1, || AtomicU64::new(0));
        for parked in &self.parked {
            parked.store(false, Ordering::SeqCst);
        }
        {
            let threads = self.threads.get_mut().unwrap();
            threads.fill(None);
            threads[0] = Some(thread::current());
            let inputs = self.inputs.get_mut();
            inputs.records = records;
            inputs.outcomes.clear();
        }
        self.install_inputs(from + 1);
        let graph: &Graph = self;
        let started = Instant::now();
        graph.tick.store(from + 1, Ordering::Relaxed);
        graph.nodes[NODE_STRUCTURAL].pending.store(1, Ordering::Relaxed);
        graph.arm(NODE_SPATIAL);
        // SAFETY: the graph stays borrowed, hence alive and in place, until
        // `retire` below has seen every ticket withdrawn and every helper gone.
        unsafe { pool.offer(graph, graph.config.workers - 1) };
        graph.worker_loop(0);
        pool.retire(graph);
        started.elapsed()
    }
}

// ---------------------------------------------------------------------------
// The world and its time control API
// ---------------------------------------------------------------------------

pub struct World {
    graph: Graph,
    pool: Arc<Pool>,
    tick_done: u32,
    failure: Option<Failure>,
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World").field("tick", &self.tick_done).field("workers", &self.graph.config.workers).field("failure", &self.failure).finish()
    }
}

impl World {
    /// A world of `config.workers` workers at most per step: the calling
    /// thread and up to `config.workers - 1` helpers of `pool`.
    pub fn new(config: WorldConfig, pool: Arc<Pool>) -> World {
        let parameters = Parameters { seed: config.seed, size: config.size, tick_count: 0, wander: config.wander };
        let state = State::initialize(&parameters);
        let initial_hash = state.hash();
        World { graph: Graph::new(state, config, 0, initial_hash), pool, tick_done: 0, failure: None }
    }

    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    pub fn tick_hash(&self, tick: u32) -> Option<u64> {
        if tick < self.graph.history_start || tick > self.tick_done {
            return None;
        }
        Some(self.graph.tick_hashes[(tick - self.graph.history_start) as usize].load(Ordering::Relaxed))
    }

    /// The hashes of every tick from the first to the current one.
    pub fn tick_hashes(&self) -> Vec<u64> {
        (self.graph.history_start..=self.tick_done).map(|tick| self.tick_hash(tick).expect("a completed tick")).collect()
    }

    /// The state at the current tick boundary, readable between steps: what
    /// the extraction and a stepped session's query are served from.
    pub fn state(&self) -> &State {
        self.graph.current_state()
    }

    /// Advances the world by `records.len()` ticks, each tick taking its
    /// record of commands as inputs at its boundary. Returns the outcomes per
    /// tick, in record order. A failed world is never stepped again.
    pub fn step(&mut self, records: Vec<Vec<Command>>) -> Result<Vec<Vec<Outcome>>, WorldFailed> {
        if let Some(failure) = &self.failure {
            return Err(WorldFailed(failure.clone()));
        }
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let end = self.tick_done + records.len() as u32;
        self.graph.run_step(end, records, &self.pool);
        let failure = self.graph.failure.lock().unwrap().clone();
        if let Some(failure) = failure {
            self.tick_done = failure.tick.saturating_sub(1).max(self.graph.history_start);
            self.graph.tick_hashes.truncate((self.tick_done - self.graph.history_start) as usize + 1);
            self.failure = Some(failure.clone());
            return Err(WorldFailed(failure));
        }
        self.tick_done = end;
        Ok(std::mem::take(&mut self.graph.inputs.get_mut().outcomes))
    }
}

// ---------------------------------------------------------------------------
// The sequential oracle
// ---------------------------------------------------------------------------

/// The reference workload stepped sequentially by the reference crate's
/// systems with records of commands resolved by the same fixed point: the
/// ground truth the graph's hashes and outcomes are compared with.
pub struct Oracle {
    state: State,
    grid: Grid,
    timing: reference_workload::Timing,
    tick: u32,
    wander: bool,
    pub tick_hashes: Vec<u64>,
}

impl Oracle {
    pub fn new(config: &WorldConfig) -> Oracle {
        let parameters = Parameters { seed: config.seed, size: config.size, tick_count: 0, wander: config.wander };
        let state = State::initialize(&parameters);
        let tick_hashes = vec![state.hash()];
        Oracle { state, grid: Grid::new(), timing: reference_workload::Timing::new(&reference_workload::SEQUENTIAL_PHASES), tick: 0, wander: config.wander, tick_hashes }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// One tick with its record; returns the outcomes of the record.
    pub fn step(&mut self, record: &[Command]) -> Vec<Outcome> {
        let (outcomes, overrides) = resolve_commands(&self.state, record);
        for o in overrides {
            self.state.vx[o.slot] = o.vx;
            self.state.vy[o.slot] = o.vy;
        }
        self.tick += 1;
        reference_workload::tick(&mut self.state, &mut self.grid, self.tick, self.wander, &mut self.timing);
        self.tick_hashes.push(self.state.hash());
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reference_workload::{first_divergent_tick, run_sequential};

    const TICKS: u32 = 40;

    fn set(index: u32, generation: u32, field: Field, value: f32) -> FieldSet {
        FieldSet { index, generation, field, value }
    }

    fn commands_for(tick: u32) -> Vec<Command> {
        let mut record = Vec::new();
        if tick % 5 == 0 {
            record.extend((0..12).map(|index| Command::Kick { index, generation: 0, delta_vx: 8.0, delta_vy: -8.0 }));
            record.push(Command::Kick { index: 5_000, generation: 0, delta_vx: 1.0, delta_vy: 1.0 });
        }
        if tick % 3 == 0 {
            record.push(Command::Set(set(20 + tick, 0, Field::VelocityX, tick as f32)));
            record.push(Command::Set(set(3, 0, Field::VelocityY, -(tick as f32))));
        }
        if tick == 1 {
            record.push(Command::Transaction { sets: vec![set(40, 0, Field::VelocityX, 5.0), set(41, 0, Field::VelocityY, 500.0)], want_inverse: true });
            record.push(Command::Transaction { sets: vec![set(42, 0, Field::VelocityX, 5.0), set(42, 9, Field::VelocityY, 5.0)], want_inverse: true });
        }
        record
    }

    fn records(from: u32, to: u32) -> Vec<Vec<Command>> {
        (from + 1..=to).map(commands_for).collect()
    }

    fn oracle_run(ticks: u32) -> (Vec<u64>, Vec<Vec<Outcome>>) {
        let mut oracle = Oracle::new(&WorldConfig::new(Size::Small, 1));
        let outcomes = (1..=ticks).map(|tick| oracle.step(&commands_for(tick))).collect();
        (oracle.tick_hashes, outcomes)
    }

    #[test]
    fn without_commands_the_world_reproduces_the_sequential_reference() {
        let reference = run_sequential(&Parameters::new(Size::Small, TICKS, false));
        for workers in [1, 3] {
            let mut world = World::new(WorldConfig::new(Size::Small, workers), Pool::new(workers - 1));
            while world.tick() < TICKS {
                let ticks = 7.min(TICKS - world.tick());
                world.step(vec![Vec::new(); ticks as usize]).unwrap();
            }
            assert_eq!(first_divergent_tick(&world.tick_hashes(), &reference.tick_hashes), None);
        }
        let mut oracle = Oracle::new(&WorldConfig::new(Size::Small, 1));
        for _ in 0..TICKS {
            oracle.step(&[]);
        }
        assert_eq!(oracle.tick_hashes, reference.tick_hashes);
    }

    #[test]
    fn commands_change_the_hashes_and_the_graph_agrees_with_the_oracle_at_every_worker_count_and_batch() {
        let (oracle_hashes, oracle_outcomes) = oracle_run(TICKS);
        let reference = run_sequential(&Parameters::new(Size::Small, TICKS, false));
        assert_eq!(first_divergent_tick(&oracle_hashes, &reference.tick_hashes), Some(1), "the transaction lands at tick 1");
        assert_eq!(oracle_outcomes[4][12], Outcome::Rejected(Rejection::UnknownIndex));
        assert!(matches!(oracle_outcomes[0][0], Outcome::TransactionApplied { .. }));
        assert_eq!(oracle_outcomes[0][1], Outcome::TransactionRejected { element: 1, reason: Rejection::StaleHandle });
        for workers in [1, 2, 3, 8] {
            for batch in [1, 7, 40] {
                let mut world = World::new(WorldConfig::new(Size::Small, workers), Pool::new(workers - 1));
                let mut outcomes = Vec::new();
                while world.tick() < TICKS {
                    let to = (world.tick() + batch).min(TICKS);
                    outcomes.extend(world.step(records(world.tick(), to)).unwrap());
                }
                assert_eq!(first_divergent_tick(&world.tick_hashes(), &oracle_hashes), None, "{workers} workers, batch {batch}");
                assert_eq!(outcomes, oracle_outcomes, "{workers} workers, batch {batch}");
            }
        }
    }

    #[test]
    fn two_worlds_sharing_one_pool_from_two_threads_each_match_the_oracle() {
        let (oracle_hashes, _) = oracle_run(TICKS);
        let pool = Pool::new(2);
        let runs: Vec<Vec<u64>> = thread::scope(|scope| {
            let handles: Vec<_> = [1_u32, 10]
                .into_iter()
                .map(|batch| {
                    let pool = pool.clone();
                    scope.spawn(move || {
                        let mut world = World::new(WorldConfig::new(Size::Small, 3), pool);
                        while world.tick() < TICKS {
                            let to = (world.tick() + batch).min(TICKS);
                            world.step(records(world.tick(), to)).unwrap();
                        }
                        world.tick_hashes()
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap()).collect()
        });
        for hashes in runs {
            assert_eq!(first_divergent_tick(&hashes, &oracle_hashes), None);
        }
    }

    #[test]
    fn a_stale_handle_and_a_value_that_is_not_finite_are_rejected_and_kicks_add_in_record_order() {
        let mut state = State::initialize(&Parameters::new(Size::Small, 0, false));
        state.alive[3] = false;
        state.generation[4] = 2;
        state.vx[4] = 10.0;
        let record = [
            Command::Kick { index: 3, generation: 0, delta_vx: 1.0, delta_vy: 0.0 },
            Command::Kick { index: 4, generation: 0, delta_vx: 1.0, delta_vy: 0.0 },
            Command::Kick { index: 4, generation: 2, delta_vx: 1.0, delta_vy: 0.0 },
            Command::Kick { index: 4, generation: 2, delta_vx: 100.0, delta_vy: 0.0 },
            Command::Set(set(5, 0, Field::VelocityX, f32::NAN)),
            Command::Set(set(9_999, 0, Field::VelocityX, 1.0)),
        ];
        let (outcomes, overrides) = resolve_commands(&state, &record);
        assert_eq!(
            outcomes,
            [
                Outcome::Rejected(Rejection::StaleHandle),
                Outcome::Rejected(Rejection::StaleHandle),
                Outcome::Applied,
                Outcome::Applied,
                Outcome::Rejected(Rejection::InvalidValue),
                Outcome::Rejected(Rejection::UnknownIndex),
            ]
        );
        assert_eq!(overrides, [Override { slot: 4, vx: SPEED_LIMIT, vy: state.vy[4] }]);
    }

    #[test]
    fn a_transaction_is_atomic_and_its_inverse_restores_the_fields() {
        let state = State::initialize(&Parameters::new(Size::Small, 0, false));
        // One stale set among three: nothing applies, the element is named.
        let refused = Command::Transaction {
            sets: vec![set(1, 0, Field::VelocityX, 1.0), set(2, 7, Field::VelocityX, 1.0), set(3, 0, Field::VelocityX, 1.0)],
            want_inverse: true,
        };
        let (outcomes, overrides) = resolve_commands(&state, &[refused]);
        assert_eq!(outcomes, [Outcome::TransactionRejected { element: 1, reason: Rejection::StaleHandle }]);
        assert!(overrides.is_empty(), "a refused transaction writes nothing");

        // The same field written twice and another entity: the inverse, as
        // one transaction after it, restores the velocities of the boundary.
        let sets = vec![set(1, 0, Field::VelocityX, 3.0), set(1, 0, Field::VelocityX, 4.0), set(2, 0, Field::VelocityY, 900.0)];
        let applied = Command::Transaction { sets, want_inverse: true };
        let (outcomes, overrides) = resolve_commands(&state, std::slice::from_ref(&applied));
        let Outcome::TransactionApplied { inverse } = &outcomes[0] else { panic!("the transaction applies") };
        assert_eq!(overrides, [Override { slot: 1, vx: 4.0, vy: state.vy[1] }, Override { slot: 2, vx: state.vx[2], vy: SPEED_LIMIT }]);
        assert_eq!(inverse.len(), 3);
        let undo = Command::Transaction { sets: inverse.clone(), want_inverse: false };
        let (outcomes, overrides) = resolve_commands(&state, &[applied, undo]);
        assert_eq!(outcomes[1], Outcome::TransactionApplied { inverse: Vec::new() });
        assert_eq!(overrides, [Override { slot: 1, vx: state.vx[1], vy: state.vy[1] }, Override { slot: 2, vx: state.vx[2], vy: state.vy[2] }]);
    }
}
