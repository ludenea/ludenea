//! The world of the late asset arrival spike. Throwaway, never product code.
//!
//! A trimmed copy of the runtime skeleton spike's core (the reference workload
//! stepped by whole ticks on the dependency-counting tick graph, the hash of a
//! tick overlapping the next tick's passes on the other buffer; no snapshot,
//! no injection) with one asset the simulation depends on: the repulsion
//! parameters table, domain data identified by a handle and by the content
//! hash of its canonical bytes. Its payload is produced outside the domain (a
//! loader thread posts it to the arrival queue whenever it is ready) and
//! becomes domain state at one point only, the input fixed point between two
//! ticks, where the queue is drained, sorted by request identifier and
//! applied; every transition is recorded in the input log with the tick it
//! takes effect at. Scripted transitions (the replay of a recorded log, a hot
//! reload at a chosen tick) enter through the same point. While the asset is
//! unavailable the repulsion pass reads the declared default. A world may
//! require the asset: it then refuses to step until the asset is available
//! (the loading gate). The control mode removes the fence: the pass reads a
//! shared slot the loader writes directly, whenever it lands.

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
    CHUNK, GRID_SIDE, Grid, KILLS_PER_TICK, SPAWNS_PER_TICK, bounced, chunk_range, fnv1a, integrated, kinetic_energy,
    repelled_velocity, wandered_velocity,
};

pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);

const NODE_SPATIAL: usize = 0;
const NODE_REPULSION: usize = 1;
const NODE_MOTION: usize = 2;
const NODE_STRUCTURAL: usize = 3;
const NODE_HASH: usize = 4;
const NODE_COUNT: usize = 5;
const NODE_NAMES: [&str; NODE_COUNT] = ["spatial_hashing", "repulsion", "motion", "structural_changes", "hash"];
/// The order in which a worker looks for claimable work: the next tick's
/// spatial hashing first (it is on the critical path), then the pending hash
/// (long, and the structural changes wait for it), then the passes.
const CLAIM_ORDER: [usize; NODE_COUNT] = [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_STRUCTURAL];

// ---------------------------------------------------------------------------
// The asset: identity, payload, availability, arrivals and the input log
// ---------------------------------------------------------------------------

/// The handle of the one asset of this spike. A handle is the stable
/// identity the domain speaks; the content hash identifies a version.
pub const REPULSION_TABLE: u32 = 1;

/// The repulsion parameters: domain data the repulsion pass reads every tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepulsionTable {
    pub strength: f32,
    pub radius: f32,
}

impl RepulsionTable {
    /// The declared default the pass reads while the asset is unavailable: no repulsion.
    pub const DEFAULT: RepulsionTable = RepulsionTable { strength: 0.0, radius: 8.0 };
    /// Version 1: the specification's constants, with which a world gives the reference hashes.
    pub const VERSION_1: RepulsionTable = RepulsionTable { strength: 64.0, radius: 8.0 };
    /// Version 2, the hot reload: a weaker repulsion.
    pub const VERSION_2: RepulsionTable = RepulsionTable { strength: 48.0, radius: 8.0 };

    /// The canonical bytes: strength then radius, little-endian.
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8);
        bytes.extend_from_slice(&self.strength.to_le_bytes());
        bytes.extend_from_slice(&self.radius.to_le_bytes());
        bytes
    }

    pub fn content_hash(&self) -> u64 {
        fnv1a(&self.bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<RepulsionTable, String> {
        if bytes.len() != 8 {
            return Err(format!("payload of {} bytes, 8 expected", bytes.len()));
        }
        Ok(RepulsionTable {
            strength: f32::from_le_bytes(bytes[0..4].try_into().expect("four bytes")),
            radius: f32::from_le_bytes(bytes[4..8].try_into().expect("four bytes")),
        })
    }

    /// The table as one word, for the shared slot of the control mode.
    pub fn packed(&self) -> u64 {
        (u64::from(self.strength.to_bits()) << 32) | u64::from(self.radius.to_bits())
    }

    pub fn unpacked(bits: u64) -> RepulsionTable {
        RepulsionTable { strength: f32::from_bits((bits >> 32) as u32), radius: f32::from_bits(bits as u32) }
    }

    /// The arrival a loader posts for this version of the table.
    pub fn arrival(&self, request: u64) -> Arrival {
        Arrival { request, asset: REPULSION_TABLE, content_hash: self.content_hash(), bytes: self.bytes() }
    }
}

/// System 2 for one slot with the table's parameters in place of the
/// specification's constants: the same operations in the same order, so that
/// version 1 of the table gives the reference kernel's bits.
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

/// What a loader posts to the arrival queue: the request it answers, the
/// asset, the content hash it announces and the payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub request: u64,
    pub asset: u32,
    pub content_hash: u64,
    pub bytes: Vec<u8>,
}

/// The availability state of the asset, domain state read by the systems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Unavailable,
    Available(u64),
    Failed(String),
}

/// One element of the input log: a transition applied at the fixed point
/// before `tick`, the first tick that reads its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub tick: u32,
    pub request: u64,
    pub asset: u32,
    pub content_hash: u64,
    pub failed: bool,
    pub reason: String,
}

/// The input log of a world: its availability transitions in order, and the
/// tick at which its loading gate opened, if it had one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputLog {
    pub transitions: Vec<Transition>,
    pub gate_opened_at: Option<u32>,
}

impl InputLog {
    /// The tick at which the version with this content hash took effect.
    pub fn arrival_tick(&self, content_hash: u64) -> Option<u32> {
        self.transitions.iter().find(|t| !t.failed && t.content_hash == content_hash).map(|t| t.tick)
    }

    /// The scripted arrivals that replay this log: one per transition, with
    /// the payload the host knows for the content hash (empty when unknown,
    /// which fails at application as the recorded failure did).
    pub fn script(&self, known: &[RepulsionTable]) -> Vec<(u32, Arrival)> {
        self.transitions
            .iter()
            .map(|t| {
                let bytes = known.iter().find(|k| k.content_hash() == t.content_hash).map_or_else(Vec::new, |k| k.bytes());
                (t.tick, Arrival { request: t.request, asset: t.asset, content_hash: t.content_hash, bytes })
            })
            .collect()
    }

    /// The canonical bytes of the log: the gate tick (all ones for none), the
    /// transition count, then per transition the tick, the request, the asset,
    /// the content hash and the failure flag, little-endian.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + self.transitions.len() * 25);
        bytes.extend_from_slice(&self.gate_opened_at.unwrap_or(u32::MAX).to_le_bytes());
        bytes.extend_from_slice(&(self.transitions.len() as u32).to_le_bytes());
        for t in &self.transitions {
            bytes.extend_from_slice(&t.tick.to_le_bytes());
            bytes.extend_from_slice(&t.request.to_le_bytes());
            bytes.extend_from_slice(&t.asset.to_le_bytes());
            bytes.extend_from_slice(&t.content_hash.to_le_bytes());
            bytes.push(u8::from(t.failed));
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<InputLog, String> {
        let read = |at: usize, len: usize| -> Result<&[u8], String> {
            bytes.get(at..at + len).ok_or_else(|| format!("log truncated at byte {at}"))
        };
        let u32_at = |at: usize| read(at, 4).map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")));
        let u64_at = |at: usize| read(at, 8).map(|b| u64::from_le_bytes(b.try_into().expect("eight bytes")));
        let gate = u32_at(0)?;
        let count = u32_at(4)? as usize;
        let mut transitions = Vec::with_capacity(count);
        let mut at = 8;
        for _ in 0..count {
            let failed = match read(at + 24, 1)?[0] {
                0 => false,
                1 => true,
                other => return Err(format!("failure flag {other} at byte {}", at + 24)),
            };
            transitions.push(Transition {
                tick: u32_at(at)?,
                request: u64_at(at + 4)?,
                asset: u32_at(at + 12)?,
                content_hash: u64_at(at + 16)?,
                failed,
                reason: String::new(),
            });
            at += 25;
        }
        if at != bytes.len() {
            return Err(format!("{} trailing bytes", bytes.len() - at));
        }
        Ok(InputLog { transitions, gate_opened_at: (gate != u32::MAX).then_some(gate) })
    }
}

/// The sequential oracle: the reference implementation's systems in the
/// specification's order, the repulsion reading the table the schedule names
/// for the tick (the last entry whose tick is at most the current one; the
/// default before the first). The schedule with version 1 at tick 1 is the
/// reference run.
pub fn oracle(parameters: &Parameters, schedule: &[(u32, RepulsionTable)]) -> Vec<u64> {
    let mut state = State::initialize(parameters);
    let mut grid = Grid::new();
    let mut hashes = Vec::with_capacity(parameters.tick_count as usize + 1);
    hashes.push(state.hash());
    for t in 1..=parameters.tick_count {
        let table = schedule.iter().rfind(|(tick, _)| *tick <= t).map_or(RepulsionTable::DEFAULT, |(_, table)| *table);
        grid.build(&state);
        for i in 0..state.len() {
            if state.alive[i] {
                let (fx, fy) = repulsion_force(&state, &grid, i, table);
                state.vx[i] = repelled_velocity(state.vx[i], fx, state.mass[i]);
                state.vy[i] = repelled_velocity(state.vy[i], fy, state.mass[i]);
            }
        }
        if parameters.wander {
            reference_workload::wander(&mut state, t);
        }
        reference_workload::integration(&mut state);
        reference_workload::boundary(&mut state);
        reference_workload::energy(&mut state);
        state.structural_changes();
        hashes.push(state.hash());
    }
    hashes
}

// ---------------------------------------------------------------------------
// Configuration, failures
// ---------------------------------------------------------------------------

/// How the repulsion pass gets its table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The skeleton's kernel with the specification's constants: no asset at all, the cost baseline.
    Plain,
    /// The parameterized kernel with no asset at all: the table is a runtime
    /// value the world holds, read per chunk, and the fixed point does nothing.
    /// Separates what the kernel's parameterization costs from what the fence costs.
    Parameterized,
    /// The fence: the table is domain state, changed at the input fixed point only.
    Fenced,
    /// The control: the pass reads the shared slot the loader writes directly.
    FenceRemoved,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldConfig {
    pub seed: u64,
    pub size: Size,
    pub wander: bool,
    pub workers: usize,
    pub spin_bound: Duration,
    pub mode: Mode,
    /// Whether the asset is in the world's required set (the loading gate).
    pub required: bool,
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
            mode: Mode::Plain,
            required: false,
        }
    }

    pub fn parameterized(self) -> WorldConfig {
        WorldConfig { mode: Mode::Parameterized, required: false, ..self }
    }

    pub fn fenced(self, required: bool) -> WorldConfig {
        WorldConfig { mode: Mode::Fenced, required, ..self }
    }

    pub fn fence_removed(self) -> WorldConfig {
        WorldConfig { mode: Mode::FenceRemoved, required: false, ..self }
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

/// Why a world did not step: its loading gate is closed, or it failed.
#[derive(Clone, Debug)]
pub enum StepError {
    Loading,
    Failed(Failure),
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepError::Loading => write!(f, "the world is loading: a required asset is not available"),
            StepError::Failed(failure) => write!(
                f,
                "world failed at tick {} in {} chunk {} on worker {}: {}",
                failure.tick, failure.node, failure.chunk, failure.worker, failure.message
            ),
        }
    }
}

impl std::error::Error for StepError {}

// ---------------------------------------------------------------------------
// The tick graph (a trimmed copy of the runtime skeleton spike's core)
// ---------------------------------------------------------------------------

/// A raw view of one `f32` column, taken once while the column's owner is
/// exclusively borrowed and never resized afterwards. Chunks write disjoint
/// slices of it without forming a reference to the whole column: the one
/// `unsafe` construct of the graph, the split of a column into disjoint
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
    /// Predecessors still to complete (the structural node waits for the
    /// motion pass of its tick and, except for the first tick of a step, for
    /// the hash of the previous tick).
    pending: AtomicUsize,
}

impl GraphNode {
    fn new(chunks: usize) -> GraphNode {
        GraphNode { chunks, next: AtomicUsize::new(chunks), finished: AtomicUsize::new(0), pending: AtomicUsize::new(0) }
    }
}

/// The asset as the domain sees it: its availability and the current table
/// (the domain-resident asset data; with one asset and one reader, the
/// current table is the whole store).
struct Assets {
    availability: Availability,
    table: RepulsionTable,
}

/// The scripted arrivals, sorted by the tick they take effect at, and a cursor.
struct Script {
    entries: Vec<(u32, Arrival)>,
    next: usize,
}

/// The tick graph over double-buffered state. The state after tick `t` lives
/// in `states[current]`; the next tick's passes read it and write the other
/// buffer, while the hash of tick `t` reads it too. The structural node runs
/// alone (every other node of its tick and the previous hash completed, the
/// next tick not armed yet) and is the fixed point where the structural
/// changes are applied, the buffers swap roles and the input fixed point of
/// the next tick runs.
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
    /// The hashes of the ticks from 0 on; grown before each step.
    tick_hashes: Vec<AtomicU64>,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
    // --- the asset ---
    /// The parameterized mode's table: version 1 passed through a black box at
    /// creation, so that the compiler treats it as the runtime value a data-driven
    /// engine would hold and folds nothing of it into the kernel.
    parameters: RepulsionTable,
    assets: UnsafeCell<Assets>,
    script: UnsafeCell<Script>,
    log: UnsafeCell<InputLog>,
    /// The arrival queue the loader thread posts to; drained at the input fixed point.
    queue: Arc<Mutex<Vec<Arrival>>>,
    /// The control mode's shared slot: the packed table the loader writes directly.
    slot: Arc<AtomicU64>,
    /// The last packed table a repulsion chunk read from the slot, and the
    /// tick during which a chunk first read a value other than the initial one.
    observed: AtomicU64,
    first_observed_tick: AtomicU32,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise; the structural node runs alone; the asset cells are written
// at the input fixed point only (the structural node, or the calling thread
// between steps) and read by the repulsion chunks of the following tick.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(state: State, config: WorldConfig, mut script: Vec<(u32, Arrival)>) -> Graph {
        let initial_hash = state.hash();
        let mut a = state;
        let mut b = a.clone();
        let columns = [Columns::of(&mut a), Columns::of(&mut b)];
        let n = a.len();
        let chunk_count = n.div_ceil(CHUNK);
        let dead: Vec<usize> = (0..n).filter(|&slot| !a.alive[slot]).collect();
        script.sort_by_key(|(tick, _)| *tick);
        Graph {
            config,
            states: [UnsafeCell::new(a), UnsafeCell::new(b)],
            columns,
            grid: UnsafeCell::new(Grid::new()),
            scratch: EnergyScratch::new(n),
            dead: UnsafeCell::new(dead),
            changed: UnsafeCell::new(Vec::new()),
            current: AtomicUsize::new(0),
            nodes: [GraphNode::new(1), GraphNode::new(chunk_count), GraphNode::new(chunk_count), GraphNode::new(1), GraphNode::new(1)],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(0),
            hash_tick: AtomicU32::new(0),
            end: AtomicU32::new(0),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            tick_hashes: vec![AtomicU64::new(initial_hash)],
            parked: (0..config.workers).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
            parameters: std::hint::black_box(RepulsionTable::VERSION_1),
            assets: UnsafeCell::new(Assets { availability: Availability::Unavailable, table: RepulsionTable::DEFAULT }),
            script: UnsafeCell::new(Script { entries: script, next: 0 }),
            log: UnsafeCell::new(InputLog::default()),
            queue: Arc::new(Mutex::new(Vec::new())),
            slot: Arc::new(AtomicU64::new(RepulsionTable::DEFAULT.packed())),
            observed: AtomicU64::new(RepulsionTable::DEFAULT.packed()),
            first_observed_tick: AtomicU32::new(u32::MAX),
        }
    }

    // --- the asset -----------------------------------------------------------

    /// The input fixed point for `tick`: the arrivals that landed in the
    /// queue, sorted by request identifier, then the scripted arrivals due,
    /// applied as transitions that take effect at `tick` and recorded in the
    /// log. Runs alone: in the structural node, or on the calling thread
    /// between steps.
    fn input_fixed_point(&self, tick: u32) {
        if self.config.mode != Mode::Fenced {
            return;
        }
        let mut arrivals = std::mem::take(&mut *self.queue.lock().expect("the queue is never poisoned"));
        arrivals.sort_by_key(|arrival| arrival.request);
        // SAFETY: the fixed point runs alone (see the type's documentation).
        let script = unsafe { &mut *self.script.get() };
        while let Some((due, arrival)) = script.entries.get(script.next) {
            if *due > tick {
                break;
            }
            arrivals.push(arrival.clone());
            script.next += 1;
        }
        for arrival in arrivals {
            self.apply(tick, arrival);
        }
    }

    /// Applies one transition: a payload for the known asset whose bytes
    /// decode and match the announced content hash makes the asset available
    /// at that hash with the decoded table; anything else makes it failed,
    /// with the default table, deterministic from then on. Both are logged.
    fn apply(&self, tick: u32, arrival: Arrival) {
        // SAFETY: the fixed point runs alone.
        let assets = unsafe { &mut *self.assets.get() };
        let log = unsafe { &mut *self.log.get() };
        let outcome = if arrival.asset != REPULSION_TABLE {
            Err(format!("unknown asset {}", arrival.asset))
        } else {
            RepulsionTable::decode(&arrival.bytes).and_then(|table| {
                if table.content_hash() == arrival.content_hash {
                    Ok(table)
                } else {
                    Err(format!("content hash {:#018x} announced, {:#018x} computed", arrival.content_hash, table.content_hash()))
                }
            })
        };
        let (failed, reason) = match outcome {
            Ok(table) => {
                assets.table = table;
                assets.availability = Availability::Available(arrival.content_hash);
                (false, String::new())
            }
            Err(reason) => {
                assets.table = RepulsionTable::DEFAULT;
                assets.availability = Availability::Failed(reason.clone());
                (true, reason)
            }
        };
        log.transitions.push(Transition {
            tick,
            request: arrival.request,
            asset: arrival.asset,
            content_hash: arrival.content_hash,
            failed,
            reason,
        });
    }

    /// The table a repulsion chunk reads: none in the plain mode (the
    /// reference kernel's constants), the world's runtime value in the
    /// parameterized mode, the domain state in the fenced mode, the shared
    /// slot's current value in the control.
    fn table_for_chunk(&self) -> Option<RepulsionTable> {
        match self.config.mode {
            Mode::Plain => None,
            Mode::Parameterized => Some(self.parameters),
            // SAFETY: written at the fixed point only, which never overlaps a pass.
            Mode::Fenced => Some(unsafe { (*self.assets.get()).table }),
            Mode::FenceRemoved => {
                let packed = self.slot.load(Ordering::Relaxed);
                if packed != self.observed.load(Ordering::Relaxed) {
                    self.observed.store(packed, Ordering::Relaxed);
                    self.first_observed_tick.fetch_min(self.tick.load(Ordering::Relaxed), Ordering::Relaxed);
                }
                Some(RepulsionTable::unpacked(packed))
            }
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
        let table = self.table_for_chunk();
        let range = chunk_range(chunk, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's slots of the next buffer are written by this task only.
        let (_, _, vx, vy) = unsafe { self.columns[next].slices(range.clone()) };
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                let (fx, fy) = match table {
                    None => reference_workload::repulsion_force(cur, grid, i),
                    Some(table) => repulsion_force(cur, grid, i, table),
                };
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
    /// spawns, records which slots they touched, swaps the buffers, then runs
    /// the input fixed point of the next tick.
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
        self.input_fixed_point(self.tick.load(Ordering::Relaxed) + 1);
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

/// A world: the reference workload's state, its tick graph, its asset and its
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
            .field("mode", &self.graph.config.mode)
            .field("availability", &self.availability())
            .field("failure", &self.failure)
            .finish()
    }
}

impl World {
    /// A world at tick 0, initialized by the specification from the seed,
    /// with its scripted arrivals (each applied at the fixed point before the
    /// tick it names).
    pub fn new(config: WorldConfig, script: Vec<(u32, Arrival)>) -> World {
        let parameters = Parameters { seed: config.seed, size: config.size, tick_count: 0, wander: config.wander };
        let state = State::initialize(&parameters);
        World { graph: Graph::new(state, config, script), tick_done: 0, stepping: Duration::ZERO, failure: None }
    }

    /// The arrival queue a loader posts to.
    pub fn queue(&self) -> Arc<Mutex<Vec<Arrival>>> {
        self.graph.queue.clone()
    }

    /// The control mode's shared slot a loader writes the packed table to.
    pub fn slot(&self) -> Arc<AtomicU64> {
        self.graph.slot.clone()
    }

    /// The last completed tick.
    pub fn tick(&self) -> u32 {
        self.tick_done
    }

    /// The hash of the state after `tick`, for a completed tick.
    pub fn tick_hash(&self, tick: u32) -> Option<u64> {
        (tick <= self.tick_done).then(|| self.graph.tick_hashes[tick as usize].load(Ordering::Relaxed))
    }

    /// The hashes of the ticks 0 to the last completed one.
    pub fn hashes(&self) -> Vec<u64> {
        (0..=self.tick_done).map(|t| self.tick_hash(t).expect("in the history")).collect()
    }

    /// The wall time spent inside `step` so far.
    pub fn stepping(&self) -> Duration {
        self.stepping
    }

    pub fn availability(&self) -> Availability {
        // SAFETY: no step is in progress (`step` joins its workers before returning).
        unsafe { (*self.graph.assets.get()).availability.clone() }
    }

    /// The table the repulsion pass reads at the next tick.
    #[cfg(test)]
    pub fn table(&self) -> RepulsionTable {
        // SAFETY: as for `availability`.
        unsafe { (*self.graph.assets.get()).table }
    }

    pub fn log(&self) -> &InputLog {
        // SAFETY: as for `availability`.
        unsafe { &*self.graph.log.get() }
    }

    /// In the control mode, the tick during which a repulsion chunk first
    /// read a table other than the initial one from the shared slot.
    pub fn first_observed_tick(&self) -> Option<u32> {
        let tick = self.graph.first_observed_tick.load(Ordering::Relaxed);
        (tick != u32::MAX).then_some(tick)
    }

    /// Runs the input fixed point of the next tick on the calling thread and
    /// says whether the loading gate is closed: the asset is required and not
    /// available. The gate's opening tick is recorded in the log.
    pub fn loading(&mut self) -> bool {
        let next = self.tick_done + 1;
        self.graph.input_fixed_point(next);
        if !self.graph.config.required {
            return false;
        }
        let available = matches!(self.availability(), Availability::Available(_));
        if available {
            // SAFETY: nothing runs between steps.
            let log = unsafe { &mut *self.graph.log.get() };
            if log.gate_opened_at.is_none() {
                log.gate_opened_at = Some(next);
            }
        }
        !available
    }

    /// Advances the world by `ticks` ticks on the calling thread and the
    /// configured workers, unless its loading gate is closed. A failed world
    /// is never stepped again: the call returns the failure at once.
    pub fn step(&mut self, ticks: u32) -> Result<(), StepError> {
        if let Some(failure) = &self.failure {
            return Err(StepError::Failed(failure.clone()));
        }
        if ticks == 0 {
            return Ok(());
        }
        if self.loading() {
            return Err(StepError::Loading);
        }
        let end = self.tick_done + ticks;
        self.stepping += self.graph.run_step(end);
        let failure = self.graph.failure.lock().unwrap().clone();
        if let Some(failure) = failure {
            // The failing tick's state is not a result: the history stops before it.
            self.tick_done = failure.tick.saturating_sub(1);
            self.graph.tick_hashes.truncate(self.tick_done as usize + 1);
            self.failure = Some(failure.clone());
            return Err(StepError::Failed(failure));
        }
        self.tick_done = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reference_workload::{first_divergent_tick, run_hash, run_sequential};

    const TICKS: u32 = 40;

    fn parameters() -> Parameters {
        Parameters::new(Size::Small, TICKS, false)
    }

    fn assert_equal_histories(ours: &[u64], theirs: &[u64], what: &str) {
        assert_eq!(first_divergent_tick(ours, theirs), None, "{what}");
    }

    fn step_in_batches(world: &mut World, batch: u32, pause: Duration) {
        while world.tick() < TICKS {
            world.step(batch.min(TICKS - world.tick())).expect("the world steps");
            if !pause.is_zero() {
                thread::sleep(pause);
            }
        }
    }

    #[test]
    fn the_parameterized_kernel_with_the_specification_table_equals_the_reference_kernel() {
        let mut state = State::initialize(&parameters());
        let mut grid = Grid::new();
        let mut timing = reference_workload::Timing::new(&reference_workload::SEQUENTIAL_PHASES);
        for t in 1..=5 {
            reference_workload::tick(&mut state, &mut grid, t, false, &mut timing);
            grid.build(&state);
            for i in 0..state.len() {
                if state.alive[i] {
                    let (fx, fy) = repulsion_force(&state, &grid, i, RepulsionTable::VERSION_1);
                    let (rx, ry) = reference_workload::repulsion_force(&state, &grid, i);
                    assert_eq!((fx.to_bits(), fy.to_bits()), (rx.to_bits(), ry.to_bits()), "slot {i} at tick {t}");
                }
            }
        }
        let reference = run_sequential(&parameters());
        assert_eq!(oracle(&parameters(), &[(1, RepulsionTable::VERSION_1)]), reference.tick_hashes);
        assert_ne!(oracle(&parameters(), &[]), reference.tick_hashes, "the default table changes the results");
    }

    #[test]
    fn the_gate_mode_equals_the_sequential_reference_at_every_worker_count() {
        let reference = run_sequential(&parameters());
        for workers in [1, 2, 3] {
            let mut world = World::new(WorldConfig::new(Size::Small, workers).fenced(true), Vec::new());
            let queue = world.queue();
            let loader = thread::spawn(move || {
                thread::sleep(Duration::from_millis(30));
                queue.lock().unwrap().push(RepulsionTable::VERSION_1.arrival(1));
            });
            assert!(world.loading(), "the gate is closed before the arrival");
            assert!(matches!(world.step(1), Err(StepError::Loading)), "a closed gate refuses to step");
            assert_eq!(world.tick(), 0);
            while world.loading() {
                thread::sleep(Duration::from_millis(1));
            }
            loader.join().unwrap();
            assert_eq!(world.log().gate_opened_at, Some(1));
            assert_eq!(world.log().arrival_tick(RepulsionTable::VERSION_1.content_hash()), Some(1));
            step_in_batches(&mut world, 7, Duration::ZERO);
            assert_equal_histories(&world.hashes(), &reference.tick_hashes, &format!("{workers} workers"));
            assert_eq!(world.availability(), Availability::Available(RepulsionTable::VERSION_1.content_hash()));
        }
    }

    #[test]
    fn the_default_mode_equals_the_oracle_of_its_arrival_tick_and_its_replay_equals_it() {
        let mut world = World::new(WorldConfig::new(Size::Small, 2).fenced(false), Vec::new());
        let queue = world.queue();
        let loader = thread::spawn(move || {
            thread::sleep(Duration::from_millis(8));
            queue.lock().unwrap().push(RepulsionTable::VERSION_1.arrival(1));
        });
        step_in_batches(&mut world, 5, Duration::from_millis(2));
        loader.join().unwrap();
        let arrival = world.log().arrival_tick(RepulsionTable::VERSION_1.content_hash());
        let schedule: Vec<(u32, RepulsionTable)> = arrival.map(|t| vec![(t, RepulsionTable::VERSION_1)]).unwrap_or_default();
        let oracle_hashes = oracle(&parameters(), &schedule);
        assert_equal_histories(&world.hashes(), &oracle_hashes, &format!("live against the oracle, arrival {arrival:?}"));
        let log = InputLog::decode(&world.log().encode()).unwrap();
        assert_eq!(log.transitions.len(), world.log().transitions.len());
        let mut replay = World::new(WorldConfig::new(Size::Small, 3).fenced(false), log.script(&[RepulsionTable::VERSION_1]));
        step_in_batches(&mut replay, 40, Duration::ZERO);
        assert_equal_histories(&replay.hashes(), &world.hashes(), "the replay against the live run");
        assert_eq!(replay.log().transitions.len(), world.log().transitions.len());
    }

    #[test]
    fn two_scripted_arrival_ticks_give_two_run_hashes_each_equal_to_its_oracle() {
        let mut hashes = Vec::new();
        for tick in [10, 20] {
            let mut world = World::new(WorldConfig::new(Size::Small, 2).fenced(false), vec![(tick, RepulsionTable::VERSION_1.arrival(1))]);
            step_in_batches(&mut world, 3, Duration::ZERO);
            assert_eq!(world.log().arrival_tick(RepulsionTable::VERSION_1.content_hash()), Some(tick));
            assert_equal_histories(&world.hashes(), &oracle(&parameters(), &[(tick, RepulsionTable::VERSION_1)]), "scripted arrival");
            hashes.push(run_hash(&world.hashes()));
        }
        assert_ne!(hashes[0], hashes[1], "different arrival ticks are different inputs");
    }

    #[test]
    fn the_hot_reload_at_a_scripted_tick_equals_the_oracle_and_replays() {
        let schedule = [(1, RepulsionTable::VERSION_1), (20, RepulsionTable::VERSION_2)];
        let oracle_hashes = oracle(&parameters(), &schedule);
        for workers in [1, 3] {
            let script = vec![(1, RepulsionTable::VERSION_1.arrival(1)), (20, RepulsionTable::VERSION_2.arrival(2))];
            let mut world = World::new(WorldConfig::new(Size::Small, workers).fenced(true), script);
            assert!(!world.loading(), "the scripted version 1 opens the gate at tick 1");
            step_in_batches(&mut world, 7, Duration::ZERO);
            assert_equal_histories(&world.hashes(), &oracle_hashes, &format!("hot reload at {workers} workers"));
            assert_eq!(world.availability(), Availability::Available(RepulsionTable::VERSION_2.content_hash()));
            assert_eq!(world.table(), RepulsionTable::VERSION_2);
            let log = InputLog::decode(&world.log().encode()).unwrap();
            assert_eq!(log.gate_opened_at, Some(1));
            let mut replay = World::new(WorldConfig::new(Size::Small, 2).fenced(true), log.script(&[RepulsionTable::VERSION_1, RepulsionTable::VERSION_2]));
            step_in_batches(&mut replay, 40, Duration::ZERO);
            assert_equal_histories(&replay.hashes(), &world.hashes(), "the replay of the hot reload");
        }
    }

    #[test]
    fn an_unknown_asset_or_a_corrupt_payload_is_reported_failed_and_the_world_keeps_stepping_with_the_default() {
        let mut corrupt = RepulsionTable::VERSION_1.arrival(2);
        corrupt.bytes.truncate(5);
        let mut wrong_hash = RepulsionTable::VERSION_1.arrival(3);
        wrong_hash.content_hash ^= 1;
        let unknown = Arrival { request: 1, asset: 7, content_hash: 0, bytes: RepulsionTable::VERSION_1.bytes() };
        let script = vec![(5, unknown), (10, corrupt), (15, wrong_hash), (30, RepulsionTable::VERSION_1.arrival(4))];
        let mut world = World::new(WorldConfig::new(Size::Small, 2).fenced(false), script);
        step_in_batches(&mut world, 4, Duration::ZERO);
        let log = world.log();
        assert_eq!(log.transitions.len(), 4);
        assert!(log.transitions[0].failed && log.transitions[0].reason.contains("unknown asset 7"));
        assert!(log.transitions[1].failed && log.transitions[1].reason.contains("5 bytes"));
        assert!(log.transitions[2].failed && log.transitions[2].reason.contains("content hash"));
        assert!(!log.transitions[3].failed);
        assert_eq!(log.transitions.iter().map(|t| t.tick).collect::<Vec<_>>(), vec![5, 10, 15, 30]);
        assert_equal_histories(&world.hashes(), &oracle(&parameters(), &[(30, RepulsionTable::VERSION_1)]), "default until tick 30");
        assert_eq!(world.availability(), Availability::Available(RepulsionTable::VERSION_1.content_hash()));
    }

    #[test]
    fn the_fence_removed_control_reads_the_shared_slot() {
        let reference = run_sequential(&parameters());
        let mut world = World::new(WorldConfig::new(Size::Small, 2).fence_removed(), Vec::new());
        world.slot().store(RepulsionTable::VERSION_1.packed(), Ordering::Relaxed);
        step_in_batches(&mut world, 10, Duration::ZERO);
        assert_equal_histories(&world.hashes(), &reference.tick_hashes, "the slot holds version 1 from the start");
        assert_eq!(world.first_observed_tick(), Some(1));
        let mut world = World::new(WorldConfig::new(Size::Small, 2).fence_removed(), Vec::new());
        step_in_batches(&mut world, 10, Duration::ZERO);
        assert_equal_histories(&world.hashes(), &oracle(&parameters(), &[]), "the slot holds the default");
        assert_eq!(world.first_observed_tick(), None);
        assert_eq!(RepulsionTable::unpacked(RepulsionTable::VERSION_2.packed()), RepulsionTable::VERSION_2);
    }

    #[test]
    fn the_parameterized_mode_reproduces_the_reference_with_no_asset() {
        let reference = run_sequential(&parameters());
        let mut world = World::new(WorldConfig::new(Size::Small, 3).parameterized(), Vec::new());
        assert!(!world.loading());
        step_in_batches(&mut world, 9, Duration::ZERO);
        assert_equal_histories(&world.hashes(), &reference.tick_hashes, "parameterized");
        assert!(world.log().transitions.is_empty());
        assert_eq!(world.availability(), Availability::Unavailable);
    }

    #[test]
    fn the_plain_mode_is_the_skeleton_and_the_log_round_trips() {
        let reference = run_sequential(&parameters());
        let mut world = World::new(WorldConfig::new(Size::Small, 3), Vec::new());
        assert!(!world.loading());
        step_in_batches(&mut world, 9, Duration::ZERO);
        assert_equal_histories(&world.hashes(), &reference.tick_hashes, "plain");
        assert!(world.log().transitions.is_empty());
        let log = InputLog {
            transitions: vec![
                Transition { tick: 1, request: 7, asset: 1, content_hash: 0x0102, failed: false, reason: String::new() },
                Transition { tick: 500, request: 8, asset: 9, content_hash: 0xff, failed: true, reason: String::new() },
            ],
            gate_opened_at: Some(1),
        };
        let bytes = log.encode();
        assert_eq!(bytes.len(), 8 + 2 * 25);
        assert_eq!(InputLog::decode(&bytes).unwrap(), log);
        assert!(InputLog::decode(&bytes[..20]).unwrap_err().contains("truncated"));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(InputLog::decode(&trailing).unwrap_err().contains("trailing"));
        let mut bad_flag = bytes;
        bad_flag[8 + 24] = 2;
        assert!(InputLog::decode(&bad_flag).unwrap_err().contains("failure flag"));
    }
}
