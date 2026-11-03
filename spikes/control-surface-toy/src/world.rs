//! The world of the control surface toy spike. Throwaway, never product code.
//!
//! A trimmed copy of the runtime skeleton spike's core: the reference workload
//! stepped on the dependency-counting tick graph (the hash of a tick overlaps
//! the next tick's passes on the other buffer), with a time control API (step
//! by whole ticks, read any completed tick's hash). What this copy adds is the
//! input fixed point: every tick of a step carries an
//! ordered record of kicks, validated against the state at the tick boundary
//! (an alive entity whose generation matches) and applied before the tick's
//! first system, in record order, as the specification's velocity clamp would
//! apply them. A kick's effect is therefore a function of the tick's record
//! and the state, never of the thread that delivered it.

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
    CHUNK, Grid, KILLS_PER_TICK, SPAWNS_PER_TICK, SPEED_LIMIT, bounced, chunk_range, integrated, kinetic_energy,
    repelled_velocity, repulsion_force, wandered_velocity,
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


/// A velocity impulse on one entity, the one domain input of the toy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kick {
    pub index: u32,
    pub generation: u32,
    pub delta_vx: f32,
    pub delta_vy: f32,
}

/// What the fixed point decided about one kick, deterministically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KickOutcome {
    Applied,
    /// The index exists but the entity there is dead or of another generation.
    StaleHandle,
    /// The index is outside the identifier table.
    UnknownIndex,
}

/// The kick applied as the specification would: added, then clamped.
#[inline]
pub fn kicked_velocity(v: f32, delta: f32) -> f32 {
    (v + delta).clamp(-SPEED_LIMIT, SPEED_LIMIT)
}

/// Validates the kicks of one tick against a state at a tick boundary, in
/// record order, and lists the applied ones by slot. Shared by the graph and
/// the sequential oracle so that both decide alike.
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
/// by the passes: the one `unsafe` construct of the world.
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
    /// The kicks of every tick of the step, by tick offset from the first.
    records: Vec<Vec<Kick>>,
    /// The outcomes per tick, filled as the ticks are installed.
    outcomes: Vec<Vec<KickOutcome>>,
    /// The applied kicks of the armed tick, in record order.
    applied: Vec<(usize, f32, f32)>,
    /// Whether a slot has a kick in the armed tick (sparse clears).
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
// tick otherwise, the inputs are written by the structural node alone and
// read by the repulsion pass it armed, and the structural node runs alone.
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

    /// The input fixed point for `tick`: validates its record against the
    /// state at the boundary (the current buffer) and installs the applied
    /// kicks for the repulsion pass. Runs alone: before the step starts, or in
    /// the structural node after the swap.
    fn install_inputs(&self, tick: u32) {
        // SAFETY: runs alone (see the type's documentation).
        let inputs = unsafe { &mut *self.inputs.get() };
        for &(slot, _, _) in &inputs.applied {
            inputs.kicked[slot] = false;
        }
        inputs.applied.clear();
        let offset = (tick - self.tick_start()) as usize;
        let Some(record) = inputs.records.get(offset) else { return };
        let (outcomes, applied) = validate_kicks(self.current_state(), record);
        for &(slot, _, _) in &applied {
            inputs.kicked[slot] = true;
        }
        inputs.applied = applied;
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

    /// Runs the ticks after `hash_tick` up to `end`, with `records` as the
    /// inputs of those ticks in order, on the calling thread and `workers - 1`
    /// spawned threads, joined before returning.
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

// ---------------------------------------------------------------------------
// The world and its time control API
// ---------------------------------------------------------------------------

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

    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    /// The first tick whose hash this world knows.
    #[cfg(test)]
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

    /// The state at the current tick boundary, readable between steps: what
    /// a query is served from.
    pub fn state(&self) -> &State {
        self.graph.current_state()
    }

    /// Advances the world by `records.len()` ticks, each tick taking its
    /// record of kicks as inputs at its boundary. Returns the outcomes per
    /// tick, in record order. A failed world is never stepped again.
    pub fn step(&mut self, records: Vec<Vec<Kick>>) -> Result<Vec<Vec<KickOutcome>>, WorldFailed> {
        if let Some(failure) = &self.failure {
            return Err(WorldFailed(failure.clone()));
        }
        if records.is_empty() {
            return Ok(Vec::new());
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
        Ok(std::mem::take(&mut self.graph.inputs.get_mut().outcomes))
    }

    /// `ticks` ticks without inputs.
    #[cfg(test)]
    pub fn step_empty(&mut self, ticks: u32) -> Result<(), WorldFailed> {
        self.step(vec![Vec::new(); ticks as usize]).map(drop)
    }
}

// ---------------------------------------------------------------------------
// The sequential oracle
// ---------------------------------------------------------------------------

/// The reference workload stepped sequentially with the same records of kicks,
/// by the reference crate's systems: the ground truth the graph's hashes are
/// compared with. Returns the tick hashes from tick 0 and the outcomes per tick.
#[cfg(test)]
pub fn oracle(config: &WorldConfig, records: &[Vec<Kick>]) -> (Vec<u64>, Vec<Vec<KickOutcome>>) {
    let parameters = Parameters { seed: config.seed, size: config.size, tick_count: records.len() as u32, wander: config.wander };
    let mut state = State::initialize(&parameters);
    let mut grid = Grid::new();
    let mut timing = reference_workload::Timing::new(&reference_workload::SEQUENTIAL_PHASES);
    let mut hashes = vec![state.hash()];
    let mut outcomes = Vec::with_capacity(records.len());
    for (offset, record) in records.iter().enumerate() {
        let (tick_outcomes, applied) = validate_kicks(&state, record);
        for (slot, dvx, dvy) in applied {
            state.vx[slot] = kicked_velocity(state.vx[slot], dvx);
            state.vy[slot] = kicked_velocity(state.vy[slot], dvy);
        }
        reference_workload::tick(&mut state, &mut grid, offset as u32 + 1, config.wander, &mut timing);
        hashes.push(state.hash());
        outcomes.push(tick_outcomes);
    }
    (hashes, outcomes)
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
                world.step_empty(7.min(TICKS - world.tick())).unwrap();
            }
            assert_eq!(first_divergent_tick(&hashes(&world), &reference.tick_hashes), None);
        }
        let (oracle_hashes, _) = oracle(&WorldConfig::new(Size::Small, 1), &vec![Vec::new(); TICKS as usize]);
        assert_eq!(oracle_hashes, reference.tick_hashes);
    }

    #[test]
    fn kicks_change_the_hashes_and_the_graph_agrees_with_the_oracle_at_every_worker_count_and_batch() {
        let config = WorldConfig::new(Size::Small, 1);
        let (oracle_hashes, oracle_outcomes) = oracle(&config, &records(0, TICKS));
        let reference = run_sequential(&Parameters::new(Size::Small, TICKS, false));
        assert_eq!(first_divergent_tick(&oracle_hashes, &reference.tick_hashes), Some(5), "the first kicks land at tick 5");
        assert_eq!(oracle_outcomes[4][12], KickOutcome::UnknownIndex);
        assert_eq!(oracle_outcomes[4][0], KickOutcome::Applied);
        for workers in [1, 2, 3, 8] {
            for batch in [1, 7, 40] {
                let mut world = World::new(WorldConfig::new(Size::Small, workers));
                let mut outcomes = Vec::new();
                while world.tick() < TICKS {
                    let to = (world.tick() + batch).min(TICKS);
                    outcomes.extend(world.step(records(world.tick(), to)).unwrap());
                }
                assert_eq!(first_divergent_tick(&hashes(&world), &oracle_hashes), None, "{workers} workers, batch {batch}");
                assert_eq!(outcomes, oracle_outcomes, "{workers} workers, batch {batch}");
            }
        }
    }

    #[test]
    fn a_stale_handle_is_rejected_deterministically_and_two_kicks_add_in_record_order() {
        let mut state = State::initialize(&Parameters::new(Size::Small, 0, false));
        state.alive[3] = false;
        state.generation[4] = 2;
        let kicks = [
            Kick { index: 3, generation: 0, delta_vx: 1.0, delta_vy: 0.0 },
            Kick { index: 4, generation: 0, delta_vx: 1.0, delta_vy: 0.0 },
            Kick { index: 4, generation: 2, delta_vx: 1.0, delta_vy: 0.0 },
            Kick { index: 4, generation: 2, delta_vx: 100.0, delta_vy: 0.0 },
        ];
        let (outcomes, applied) = validate_kicks(&state, &kicks);
        assert_eq!(outcomes, [KickOutcome::StaleHandle, KickOutcome::StaleHandle, KickOutcome::Applied, KickOutcome::Applied]);
        assert_eq!(applied.len(), 2);
        let v = kicked_velocity(kicked_velocity(10.0, 1.0), 100.0);
        assert_eq!(v, SPEED_LIMIT);
    }
}
