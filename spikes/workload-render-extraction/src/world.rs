//! The world of the render extraction spike. Throwaway, never product code.
//!
//! A trimmed copy of the runtime skeleton spike's core (the reference workload
//! stepped on the dependency-counting tick graph, the hash of a tick
//! overlapping the next tick's passes on the other buffer, stepped by whole
//! ticks; no snapshot, no injection) with one node more: the extraction. After
//! the structural node of a tick it runs chunk by chunk, reads the state after
//! the tick while the next tick's passes run on the other buffer, and writes
//! per slot the position and a sprite handle into one of two render buffers
//! the renderer owns. The structural node of the next tick waits for it, as it
//! waits for the hash. With the change detection skip, a chunk no pass wrote
//! this tick is copied from the previous render buffer instead of gathered
//! from the state. The static-half variant freezes the upper half of the
//! chunks (their passes copy the values and mark nothing) so that the skip has
//! chunks to skip; it is a different simulation, compared across worker counts
//! and modes only.

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
    CHUNK, Fnv1a, Grid, KILLS_PER_TICK, SPAWNS_PER_TICK, bounced, chunk_range, integrated, kinetic_energy,
    repelled_velocity, repulsion_force, wandered_velocity,
};

pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);

const NODE_SPATIAL: usize = 0;
const NODE_REPULSION: usize = 1;
const NODE_MOTION: usize = 2;
const NODE_STRUCTURAL: usize = 3;
const NODE_HASH: usize = 4;
const NODE_EXTRACTION: usize = 5;
const NODE_COUNT: usize = 6;
const NODE_NAMES: [&str; NODE_COUNT] =
    ["spatial_hashing", "repulsion", "motion", "structural_changes", "hash", "extraction"];
/// The order in which a worker looks for claimable work: the next tick's
/// spatial hashing first (it is on the critical path), then the hash and the
/// extraction of the completed tick (the structural changes wait for them),
/// then the passes.
const CLAIM_ORDER: [usize; NODE_COUNT] =
    [NODE_SPATIAL, NODE_HASH, NODE_EXTRACTION, NODE_REPULSION, NODE_MOTION, NODE_STRUCTURAL];

/// The extraction mode of a world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extraction {
    /// No extraction node: the skeleton's world, the baseline.
    Off,
    /// Every chunk gathered from the state after every tick.
    On,
    /// A chunk no pass wrote this tick is copied from the previous render buffer.
    Skip,
}

impl Extraction {
    #[cfg(test)]
    pub const ALL: [Extraction; 3] = [Extraction::Off, Extraction::On, Extraction::Skip];

    pub fn name(self) -> &'static str {
        match self {
            Extraction::Off => "off",
            Extraction::On => "on",
            Extraction::Skip => "skip",
        }
    }
}

/// The population variant of a world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// The reference workload as specified.
    Normal,
    /// The upper half of the chunks never moves: their repulsion and motion
    /// passes copy the values to the other buffer and mark nothing, so that
    /// only the structural changes ever write them.
    StaticHalf,
}

impl Variant {
    pub const ALL: [Variant; 2] = [Variant::Normal, Variant::StaticHalf];

    pub fn name(self) -> &'static str {
        match self {
            Variant::Normal => "normal",
            Variant::StaticHalf => "static-half",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub size: Size,
    pub wander: bool,
    pub workers: usize,
    pub spin_bound: Duration,
    pub extraction: Extraction,
    pub variant: Variant,
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
            extraction: Extraction::Off,
            variant: Variant::Normal,
        }
    }

    pub fn extracting(self, extraction: Extraction) -> WorldConfig {
        WorldConfig { extraction, ..self }
    }

    pub fn variant(self, variant: Variant) -> WorldConfig {
        WorldConfig { variant, ..self }
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

/// The error of stepping a failed world.
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

/// The sprite handle of an entity: the mass 1, 2 and 4 map to the handles 1,
/// 2 and 3; a dead slot carries 0.
pub fn sprite_of(mass: f32) -> u32 {
    if mass == 1.0 {
        1
    } else if mass == 2.0 {
        2
    } else {
        3
    }
}

// ---------------------------------------------------------------------------
// The tick graph (a trimmed copy of the scheduling overhead spike's counter graph)
// ---------------------------------------------------------------------------

/// A raw view of one column, taken once while the column's owner is
/// exclusively borrowed and never resized afterwards. Chunks write disjoint
/// slices of it without forming a reference to the whole column: this is the
/// one `unsafe` construct of the spike, the split of a column into disjoint
/// mutable chunk slices, used for the state columns and the render columns.
struct RawColumn<T> {
    ptr: *mut T,
    len: usize,
}

impl<T> Clone for RawColumn<T> {
    fn clone(&self) -> RawColumn<T> {
        *self
    }
}

impl<T> Copy for RawColumn<T> {}

unsafe impl<T> Send for RawColumn<T> {}
unsafe impl<T> Sync for RawColumn<T> {}

impl<T> RawColumn<T> {
    fn of(values: &mut [T]) -> RawColumn<T> {
        RawColumn { ptr: values.as_mut_ptr(), len: values.len() }
    }

    /// # Safety
    ///
    /// No other thread reads or writes the slots of `range` in this column
    /// while the returned slice lives.
    #[allow(clippy::mut_from_ref, reason = "the raw split of a column into disjoint chunk slices is the construct under test")]
    unsafe fn slice(&self, range: Range<usize>) -> &mut [T] {
        assert!(range.start <= range.end && range.end <= self.len, "chunk slice out of bounds");
        // SAFETY: in bounds by the assertion; disjointness is the caller's.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(range.start), range.len()) }
    }
}

#[derive(Clone, Copy)]
struct Columns {
    x: RawColumn<f32>,
    y: RawColumn<f32>,
    vx: RawColumn<f32>,
    vy: RawColumn<f32>,
}

impl Columns {
    fn of(state: &mut State) -> Columns {
        Columns {
            x: RawColumn::of(&mut state.x),
            y: RawColumn::of(&mut state.y),
            vx: RawColumn::of(&mut state.vx),
            vy: RawColumn::of(&mut state.vy),
        }
    }

    /// # Safety
    ///
    /// As for [`RawColumn::slice`], for the four columns at once.
    #[allow(clippy::mut_from_ref, reason = "the raw split of a column into disjoint chunk slices is the construct under test")]
    unsafe fn slices(&self, range: Range<usize>) -> (&mut [f32], &mut [f32], &mut [f32], &mut [f32]) {
        // SAFETY: forwarded to the caller.
        unsafe {
            (self.x.slice(range.clone()), self.y.slice(range.clone()), self.vx.slice(range.clone()), self.vy.slice(range))
        }
    }
}

/// One render buffer: what the renderer reads per slot, in the layout the
/// renderer chooses (here three columns), never a reference into the state.
struct RenderBuffer {
    x: Vec<f32>,
    y: Vec<f32>,
    sprite: Vec<u32>,
}

impl RenderBuffer {
    fn new(entity_count: usize) -> RenderBuffer {
        RenderBuffer { x: vec![0.0; entity_count], y: vec![0.0; entity_count], sprite: vec![0; entity_count] }
    }
}

#[derive(Clone, Copy)]
struct RenderColumns {
    x: RawColumn<f32>,
    y: RawColumn<f32>,
    sprite: RawColumn<u32>,
}

impl RenderColumns {
    fn of(buffer: &mut RenderBuffer) -> RenderColumns {
        RenderColumns {
            x: RawColumn::of(&mut buffer.x),
            y: RawColumn::of(&mut buffer.y),
            sprite: RawColumn::of(&mut buffer.sprite),
        }
    }

    /// # Safety
    ///
    /// As for [`RawColumn::slice`], for the three columns at once.
    #[allow(clippy::mut_from_ref, reason = "the raw split of a column into disjoint chunk slices is the construct under test")]
    unsafe fn slices(&self, range: Range<usize>) -> (&mut [f32], &mut [f32], &mut [u32]) {
        // SAFETY: forwarded to the caller.
        unsafe { (self.x.slice(range.clone()), self.y.slice(range.clone()), self.sprite.slice(range)) }
    }
}

/// One energy sum per group of `CHUNK` slots, written by the motion chunk
/// that owns the group and added in group order at the fixed point: the
/// reduction order of the specification.
struct EnergyScratch {
    _groups: Vec<f32>,
    groups: RawColumn<f32>,
}

impl EnergyScratch {
    fn new(entity_count: usize) -> EnergyScratch {
        let mut groups = vec![0.0_f32; entity_count.div_ceil(CHUNK)];
        let column = RawColumn::of(&mut groups);
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
#[allow(clippy::too_many_arguments, reason = "the slot's five values, the tick, the index and the wander flag")]
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
    /// the hash and the extraction of the previous tick, counted as one).
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
/// buffer, while the hash and the extraction of tick `t` read it too. The
/// structural node runs alone (every other node of its tick, the previous
/// hash and the previous extraction completed, the next tick not armed yet)
/// and is the fixed point where the structural changes are applied and the
/// buffers swap roles. The extraction of tick `t` writes render buffer `t % 2`
/// and, with the skip, reads render buffer `(t - 1) % 2`, whose writer
/// completed before the structural node of tick `t` ran.
struct Graph {
    config: WorldConfig,
    states: [UnsafeCell<State>; 2],
    columns: [Columns; 2],
    render: [UnsafeCell<RenderBuffer>; 2],
    render_columns: [RenderColumns; 2],
    /// Per tick parity, per chunk: whether a pass or the structural changes
    /// wrote the chunk this tick. The structural node of tick `t` clears the
    /// flags of parity `(t + 1) % 2` (tick `t - 1`'s, whose extraction is
    /// complete) before the passes of tick `t + 1` set them.
    written: [Vec<AtomicBool>; 2],
    /// The first static chunk: the chunk count in the normal variant.
    static_from: usize,
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
    /// The tick whose hash and extraction are armed or were the last computed.
    hash_tick: AtomicU32,
    /// The hash and extraction tasks of the completed tick still to finish:
    /// their group releases the structural node once, or ends the step.
    post_pending: AtomicUsize,
    /// The last tick of the step in progress.
    end: AtomicU32,
    done: AtomicBool,
    failed: AtomicBool,
    failure: Mutex<Option<Failure>>,
    /// The hashes of every tick from 0; grown before each step.
    tick_hashes: Vec<AtomicU64>,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
    extraction_nanos: AtomicU64,
    chunks_gathered: AtomicU64,
    chunks_copied: AtomicU64,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a state buffer is written during a tick only through
// disjoint chunk slices of the passes, read by the passes, the hash and the
// extraction of the previous tick otherwise; a render buffer is written only
// through disjoint chunk slices of one tick's extraction and read by the next
// tick's extraction after that one completed; the structural node runs alone.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(state: State, config: WorldConfig, initial_hash: u64) -> Graph {
        let mut a = state;
        let mut b = a.clone();
        let columns = [Columns::of(&mut a), Columns::of(&mut b)];
        let n = a.len();
        let chunk_count = n.div_ceil(CHUNK);
        let mut render_a = RenderBuffer::new(n);
        let mut render_b = RenderBuffer::new(n);
        let render_columns = [RenderColumns::of(&mut render_a), RenderColumns::of(&mut render_b)];
        let dead: Vec<usize> = (0..n).filter(|&slot| !a.alive[slot]).collect();
        let flags = || (0..chunk_count).map(|_| AtomicBool::new(false)).collect();
        Graph {
            config,
            states: [UnsafeCell::new(a), UnsafeCell::new(b)],
            columns,
            render: [UnsafeCell::new(render_a), UnsafeCell::new(render_b)],
            render_columns,
            written: [flags(), flags()],
            static_from: match config.variant {
                Variant::Normal => chunk_count,
                Variant::StaticHalf => chunk_count / 2,
            },
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
                GraphNode::new(chunk_count),
            ],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(0),
            hash_tick: AtomicU32::new(0),
            post_pending: AtomicUsize::new(0),
            end: AtomicU32::new(0),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            tick_hashes: vec![AtomicU64::new(initial_hash)],
            parked: (0..config.workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
            extraction_nanos: AtomicU64::new(0),
            chunks_gathered: AtomicU64::new(0),
            chunks_copied: AtomicU64::new(0),
        }
    }

    fn extracting(&self) -> bool {
        self.config.extraction != Extraction::Off
    }

    /// The hash and extraction tasks that follow a tick: the hash alone
    /// without extraction.
    fn post_nodes(&self) -> usize {
        if self.extracting() { 2 } else { 1 }
    }

    fn is_static(&self, chunk: usize) -> bool {
        chunk >= self.static_from
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
        if self.is_static(chunk) {
            vx.copy_from_slice(&cur.vx[range.clone()]);
            vy.copy_from_slice(&cur.vy[range]);
            return;
        }
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
        if self.is_static(chunk) {
            // The velocities were copied by the repulsion pass; the positions
            // follow, and the energy of the unchanged velocities is summed as
            // the specification states.
            x.copy_from_slice(&cur.x[range.clone()]);
            y.copy_from_slice(&cur.y[range.clone()]);
            for (k, i) in range.enumerate() {
                if cur.alive[i] {
                    sum += kinetic_energy(vx[k], vy[k], cur.mass[i]);
                }
            }
        } else {
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
            if self.extracting() {
                self.written[tick as usize % 2][chunk].store(true, Ordering::Relaxed);
            }
        }
        *sum_slot = sum;
    }

    /// The fixed point: brings the next buffer's liveness, generations,
    /// masses and stream up to date, reduces the energy, applies the kills and
    /// spawns, records which slots they touched (and marks their chunks as
    /// written for this tick's extraction), clears the previous tick's marks,
    /// and swaps the buffers.
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
        if self.extracting() {
            let tick = self.tick.load(Ordering::Relaxed) as usize;
            let marks = &self.written[tick % 2];
            for &slot in changed.iter() {
                marks[slot / CHUNK].store(true, Ordering::Relaxed);
            }
            // The previous tick's extraction completed before this node ran;
            // the next tick's passes set these flags after it completes.
            for flag in &self.written[(tick + 1) % 2] {
                flag.store(false, Ordering::Relaxed);
            }
        }
        self.current.store(next_index, Ordering::Relaxed);
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed);
        self.tick_hashes[tick as usize].store(self.current_state().hash(), Ordering::Relaxed);
    }

    /// The extraction of one chunk of the completed tick: gathered from the
    /// state after the tick, or, with the skip and no write to the chunk this
    /// tick, copied from the previous tick's render buffer.
    fn extraction_chunk(&self, chunk: usize) {
        let started = Instant::now();
        let tick = self.hash_tick.load(Ordering::Relaxed) as usize;
        let cur = self.current_state();
        let range = chunk_range(chunk, cur.len());
        let (to, from) = (tick % 2, (tick + 1) % 2);
        // SAFETY: this chunk's slots of render buffer `to` are written by this
        // task only; render buffer `from` was completed by the previous tick's
        // extraction and is not written until this tick's structural node ran.
        let (x, y, sprite) = unsafe { self.render_columns[to].slices(range.clone()) };
        if self.config.extraction == Extraction::Skip && !self.written[to][chunk].load(Ordering::Relaxed) {
            let (px, py, ps) = unsafe { self.render_columns[from].slices(range) };
            x.copy_from_slice(px);
            y.copy_from_slice(py);
            sprite.copy_from_slice(ps);
            self.chunks_copied.fetch_add(1, Ordering::Relaxed);
        } else {
            for (k, i) in range.enumerate() {
                if cur.alive[i] {
                    x[k] = cur.x[i];
                    y[k] = cur.y[i];
                    sprite[k] = sprite_of(cur.mass[i]);
                } else {
                    x[k] = 0.0;
                    y[k] = 0.0;
                    sprite[k] = 0;
                }
            }
            self.chunks_gathered.fetch_add(1, Ordering::Relaxed);
        }
        self.extraction_nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    fn node_tick(&self, node: usize) -> u32 {
        if node == NODE_HASH || node == NODE_EXTRACTION {
            self.hash_tick.load(Ordering::Relaxed)
        } else {
            self.tick.load(Ordering::Relaxed)
        }
    }

    fn execute(&self, node: usize, chunk: usize) {
        match node {
            NODE_SPATIAL => self.spatial(),
            NODE_REPULSION => self.repulsion_chunk(chunk),
            NODE_MOTION => self.motion_chunk(chunk),
            NODE_STRUCTURAL => self.structural(),
            NODE_HASH => self.hash(),
            NODE_EXTRACTION => self.extraction_chunk(chunk),
            _ => unreachable!("six nodes"),
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

    /// Arms the hash and, when extracting, the extraction of the tick the
    /// structural node just completed.
    fn arm_post_nodes(&self) {
        self.post_pending.store(self.post_nodes(), Ordering::Relaxed);
        self.arm(NODE_HASH);
        if self.extracting() {
            self.arm(NODE_EXTRACTION);
        }
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
    /// chunk. After a failure nothing new is armed except the hash and the
    /// extraction the structural node owes, and the step ends when the group
    /// of those two completes.
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
            NODE_HASH | NODE_EXTRACTION => {
                if self.post_pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                    let last = self.hash_tick.load(Ordering::Relaxed) == self.end.load(Ordering::Relaxed);
                    if last || self.failed.load(Ordering::SeqCst) {
                        self.finish_step();
                    } else {
                        self.release_structural();
                    }
                }
            }
            NODE_STRUCTURAL => {
                let tick = self.tick.load(Ordering::Relaxed);
                self.hash_tick.store(tick, Ordering::Relaxed);
                if tick < self.end.load(Ordering::Relaxed) && !self.failed.load(Ordering::SeqCst) {
                    self.tick.store(tick + 1, Ordering::Relaxed);
                    // The motion pass of the next tick, and the group of this
                    // tick's hash and extraction.
                    self.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
                    self.arm(NODE_SPATIAL);
                }
                self.arm_post_nodes();
                self.wake(self.config.workers);
            }
            _ => unreachable!("six nodes"),
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

    /// Runs the ticks after `hash_tick` up to `end` on the calling thread and
    /// `workers - 1` spawned threads, joined before returning.
    fn run_step(&mut self, end: u32) -> Duration {
        let from = self.hash_tick.load(Ordering::Relaxed);
        assert!(end > from, "a step runs at least one tick");
        self.end.store(end, Ordering::Relaxed);
        self.done.store(false, Ordering::SeqCst);
        self.tick_hashes.resize_with(end as usize + 1, || AtomicU64::new(0));
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
            graph.threads.set(threads).expect("threads are published once per step");
            // The hash and the extraction of `from` are already done: the
            // first structural node waits for its motion pass only.
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

/// A world: the reference workload's state, its tick graph, its render
/// buffers and its history of tick hashes, stepped by whole ticks.
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
            .field("extraction", &self.graph.config.extraction)
            .field("variant", &self.graph.config.variant)
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
        World { graph: Graph::new(state, config, initial_hash), tick_done: 0, stepping: Duration::ZERO, failure: None }
    }

    /// The last completed tick.
    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    #[cfg(test)]
    pub fn chunk_count(&self) -> usize {
        self.graph.nodes[NODE_EXTRACTION].chunks
    }

    /// The hash of the state after `tick`, for a completed tick.
    pub fn tick_hash(&self, tick: u32) -> Option<u64> {
        if tick > self.tick_done {
            return None;
        }
        Some(self.graph.tick_hashes[tick as usize].load(Ordering::Relaxed))
    }

    pub fn hashes(&self) -> Vec<u64> {
        (0..=self.tick_done).map(|t| self.tick_hash(t).expect("in the history")).collect()
    }

    /// The state after the last completed tick.
    #[cfg(test)]
    pub fn state(&self) -> &State {
        self.graph.current_state()
    }

    /// The wall time spent inside `step` so far.
    pub fn stepping(&self) -> Duration {
        self.stepping
    }

    /// The processor time the extraction chunks took so far, summed over chunks.
    pub fn extraction_time(&self) -> Duration {
        Duration::from_nanos(self.graph.extraction_nanos.load(Ordering::Relaxed))
    }

    pub fn chunks_gathered(&self) -> u64 {
        self.graph.chunks_gathered.load(Ordering::Relaxed)
    }

    pub fn chunks_copied(&self) -> u64 {
        self.graph.chunks_copied.load(Ordering::Relaxed)
    }

    /// The render buffer of the last completed tick, valid between steps.
    fn render_buffer(&self) -> &RenderBuffer {
        // SAFETY: no step is in progress: the graph's threads are joined.
        unsafe { &*self.graph.render[self.tick_done as usize % 2].get() }
    }

    /// The render state of one slot after the last completed tick: position
    /// and sprite handle, 0 for a dead slot.
    #[cfg(test)]
    pub fn render_slot(&self, slot: usize) -> (f32, f32, u32) {
        let buffer = self.render_buffer();
        (buffer.x[slot], buffer.y[slot], buffer.sprite[slot])
    }

    /// FNV-1a over the render state after the last completed tick: per slot
    /// the position and the sprite handle, little-endian, in slot order.
    pub fn render_hash(&self) -> u64 {
        let buffer = self.render_buffer();
        let mut hasher = Fnv1a::new();
        for slot in 0..buffer.x.len() {
            hasher.write(&buffer.x[slot].to_le_bytes());
            hasher.write(&buffer.y[slot].to_le_bytes());
            hasher.write(&buffer.sprite[slot].to_le_bytes());
        }
        hasher.finish()
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
            self.tick_done = failure.tick.saturating_sub(1);
            self.graph.tick_hashes.truncate(self.tick_done as usize + 1);
            self.failure = Some(failure.clone());
            return Err(WorldFailed(failure));
        }
        self.tick_done = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reference_workload::{Run, first_divergent_tick, run_hash, run_sequential};

    const TICKS: u32 = 40;
    const WORKERS: [usize; 3] = [1, 2, 3];

    fn reference() -> Run {
        run_sequential(&Parameters::new(Size::Small, TICKS, false))
    }

    /// Steps a world to `TICKS` in batches of 7, reading the render hash after
    /// every batch; returns the world and the run hash of those render hashes.
    fn run(variant: Variant, extraction: Extraction, workers: usize) -> (World, u64) {
        let mut world = World::new(WorldConfig::new(Size::Small, workers).extracting(extraction).variant(variant));
        let mut render_hashes = Vec::new();
        while world.tick() < TICKS {
            world.step(7.min(TICKS - world.tick())).unwrap();
            render_hashes.push(world.render_hash());
        }
        (world, run_hash(&render_hashes))
    }

    #[test]
    fn every_mode_reproduces_the_sequential_hashes_at_every_worker_count() {
        let reference = reference();
        for extraction in Extraction::ALL {
            for workers in WORKERS {
                let (world, _) = run(Variant::Normal, extraction, workers);
                assert_eq!(world.tick(), TICKS);
                assert_eq!(
                    first_divergent_tick(&world.hashes(), &reference.tick_hashes),
                    None,
                    "{extraction:?} at {workers} workers"
                );
            }
        }
    }

    #[test]
    fn the_render_state_is_a_function_of_the_state_alone() {
        for variant in Variant::ALL {
            let (_, expected) = run(variant, Extraction::On, 1);
            for extraction in [Extraction::On, Extraction::Skip] {
                for workers in WORKERS {
                    let (_, render) = run(variant, extraction, workers);
                    assert_eq!(render, expected, "{variant:?} {extraction:?} at {workers} workers");
                }
            }
        }
    }

    #[test]
    fn the_static_half_variant_is_identical_across_worker_counts_and_modes() {
        let reference = reference();
        let (baseline, _) = run(Variant::StaticHalf, Extraction::Off, 1);
        assert_ne!(baseline.hashes(), reference.tick_hashes, "a different simulation");
        for extraction in Extraction::ALL {
            for workers in WORKERS {
                let (world, _) = run(Variant::StaticHalf, extraction, workers);
                assert_eq!(first_divergent_tick(&world.hashes(), &baseline.hashes()), None, "{extraction:?} at {workers} workers");
            }
        }
    }

    #[test]
    fn the_skip_copies_the_static_chunks_no_change_touched() {
        let (on, _) = run(Variant::StaticHalf, Extraction::On, 2);
        let chunks = on.chunk_count() as u64;
        assert_eq!(chunks, 4);
        assert_eq!((on.chunks_gathered(), on.chunks_copied()), (chunks * u64::from(TICKS), 0));
        let (skip, _) = run(Variant::StaticHalf, Extraction::Skip, 2);
        assert_eq!(skip.chunks_gathered() + skip.chunks_copied(), chunks * u64::from(TICKS));
        assert!(skip.chunks_copied() > 0, "the static chunks are copied on the ticks no change touched them");
        assert!(skip.chunks_copied() <= chunks / 2 * u64::from(TICKS), "only the static chunks are ever copied");
        assert!(skip.chunks_gathered() >= chunks / 2 * u64::from(TICKS), "the moving chunks are gathered every tick");
        let (normal, _) = run(Variant::Normal, Extraction::Skip, 2);
        assert_eq!(normal.chunks_copied(), 0, "every chunk of the normal variant is written every tick");
        let (off, _) = run(Variant::StaticHalf, Extraction::Off, 2);
        assert_eq!((off.chunks_gathered(), off.chunks_copied(), off.extraction_time()), (0, 0, Duration::ZERO));
    }

    /// The population is full at every tick boundary (the spawns of a tick
    /// refill the slots its kills emptied), so the dead form of a render slot
    /// (position 0, sprite 0) is never observable between steps; the alive
    /// form is checked slot by slot against the state.
    #[test]
    fn the_render_state_reads_the_position_and_the_sprite_handle_of_every_slot() {
        for variant in Variant::ALL {
            for extraction in [Extraction::On, Extraction::Skip] {
                let (world, _) = run(variant, extraction, 3);
                let state = world.state();
                assert!(state.alive.iter().all(|&alive| alive), "the population is full at a tick boundary");
                let handles: std::collections::BTreeSet<u32> = (0..state.len()).map(|slot| world.render_slot(slot).2).collect();
                assert_eq!(handles.into_iter().collect::<Vec<_>>(), vec![1, 2, 3], "every sprite handle is in use");
                for slot in 0..state.len() {
                    let expected = if state.alive[slot] {
                        (state.x[slot], state.y[slot], sprite_of(state.mass[slot]))
                    } else {
                        (0.0, 0.0, 0)
                    };
                    assert_eq!(world.render_slot(slot), expected, "{variant:?} {extraction:?} slot {slot}");
                }
            }
        }
    }
}
