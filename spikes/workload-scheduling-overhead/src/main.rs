//! Scheduling overhead of the reference workload for the multithreading
//! research.
//!
//! Throwaway spike, never product code. It runs the reference workload
//! (default size, wander system off) under four schedulers, compares their
//! tick hashes with the sequential reference and their throughput with one
//! another, and measures what each scheduler construct costs:
//!
//! - `barrier-copy-back`: the pool of the worker sweep spike: a barrier at
//!   both ends of each parallel pass, chunks claimed from one atomic counter,
//!   per-chunk output buffers copied back by the main thread between passes;
//! - `barrier-in-place`: the same pool, the passes writing disjoint chunk
//!   slices of the state in place, so that the copy back disappears;
//! - `graph-counter`: a dependency-counting task graph per tick over
//!   double-buffered state: a node is armed when its predecessors complete,
//!   the chunks of a node are claimed from one atomic counter, and the hash of
//!   a tick runs while the next tick's spatial hashing, repulsion and motion
//!   proceed on the other buffer;
//! - `graph-stealing`: the same graph, the chunks of a node pre-partitioned
//!   into per-worker ranges: the owner claims from the front of its range and
//!   idle workers steal one chunk at a time from the back of another's.
//!
//! The graph runs under three waiting policies (park at once, spin for a
//! bound then park, spin without bound), with a chunk size sweep, a spin bound
//! sweep, an empty-kernel variant that measures the scheduler alone, and one
//! run with a panic injected in a chunk to check that the pool neither
//! deadlocks nor loses the failure's context.
//!
//! Every worker records the busy time of each task it executes and the time
//! it spends waiting, so that a run reports its utilization per node.

use std::cell::UnsafeCell;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use reference_workload::{
    CHUNK, DEFAULT_TICK_COUNT, Grid, Json, KILLS_PER_TICK, Parameters, Run, SPAWNS_PER_TICK, Size,
    State, Timing, WARM_UP_TICKS, bounced, chunk_range, first_divergent_tick, integrated,
    kinetic_energy, repelled_velocity, repulsion_force, run_hash, run_sequential,
    wandered_velocity, write_platform_and_build, write_run_core,
};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-scheduling-overhead";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
const SWEEP_WORKERS: usize = 4;
const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);
const SPIN_BOUND_SWEEP_US: [u64; 4] = [5, 20, 200, 1_000];
const CHUNK_SIZE_SWEEP: [usize; 4] = [64, 128, 512, 1_024];
const EMPTY_TICKS: u32 = 10_000;

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
const POOL_PHASES: [&str; 6] =
    ["spatial_hashing", "repulsion", "motion", "copy_back", "structural_changes", "hash"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scheduler {
    BarrierCopyBack,
    BarrierInPlace,
    GraphCounter,
    GraphStealing,
}

impl Scheduler {
    const ALL: [Scheduler; 4] = [
        Scheduler::BarrierCopyBack,
        Scheduler::BarrierInPlace,
        Scheduler::GraphCounter,
        Scheduler::GraphStealing,
    ];

    fn name(self) -> &'static str {
        match self {
            Scheduler::BarrierCopyBack => "barrier-copy-back",
            Scheduler::BarrierInPlace => "barrier-in-place",
            Scheduler::GraphCounter => "graph-counter",
            Scheduler::GraphStealing => "graph-stealing",
        }
    }

    fn is_graph(self) -> bool {
        matches!(self, Scheduler::GraphCounter | Scheduler::GraphStealing)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Waiting {
    /// Park on the operating system at once when no work is claimable.
    Park,
    /// Spin for at most the bound, then park.
    SpinThenPark(Duration),
    /// Spin until work appears, never park.
    Spin,
}

impl Waiting {
    fn name(self) -> &'static str {
        match self {
            Waiting::Park => "park",
            Waiting::SpinThenPark(_) => "spin-then-park",
            Waiting::Spin => "spin",
        }
    }

    /// The spin bound in microseconds: 0 for park, -1 for an unbounded spin.
    fn spin_bound_us(self) -> i64 {
        match self {
            Waiting::Park => 0,
            Waiting::SpinThenPark(bound) => bound.as_micros() as i64,
            Waiting::Spin => -1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kernel {
    Workload,
    Empty,
}

#[derive(Clone, Copy, Debug)]
struct Injection {
    tick: u32,
    node: usize,
    chunk: usize,
}

#[derive(Clone, Copy, Debug)]
struct Config {
    scheduler: Scheduler,
    waiting: Waiting,
    chunk_size: usize,
    workers: usize,
    kernel: Kernel,
    injection: Option<Injection>,
}

impl Config {
    fn new(scheduler: Scheduler, waiting: Waiting, chunk_size: usize, workers: usize) -> Config {
        assert!(workers >= 1);
        assert!(
            chunk_size.is_power_of_two() && (64..=4_096).contains(&chunk_size),
            "chunk sizes are powers of two between 64 and 4096 so that chunks never straddle an energy group"
        );
        assert!(
            scheduler != Scheduler::BarrierCopyBack || chunk_size == CHUNK,
            "the copy-back pool keeps the worker sweep's chunk size"
        );
        Config { scheduler, waiting, chunk_size, workers, kernel: Kernel::Workload, injection: None }
    }

    fn empty(self) -> Config {
        Config { kernel: Kernel::Empty, ..self }
    }

    fn injecting(self, injection: Injection) -> Config {
        Config { injection: Some(injection), ..self }
    }
}

/// What one worker did during a run.
#[derive(Clone, Copy, Default, Debug)]
struct WorkerStats {
    busy_ns: [u64; NODE_COUNT],
    tasks: [u64; NODE_COUNT],
    copy_back_ns: u64,
    wait_ns: u64,
    parks: u64,
    steals: u64,
}

impl WorkerStats {
    fn add(&mut self, other: &WorkerStats) {
        for node in 0..NODE_COUNT {
            self.busy_ns[node] += other.busy_ns[node];
            self.tasks[node] += other.tasks[node];
        }
        self.copy_back_ns += other.copy_back_ns;
        self.wait_ns += other.wait_ns;
        self.parks += other.parks;
        self.steals += other.steals;
    }

    fn busy_total_ns(&self) -> u64 {
        self.busy_ns.iter().sum::<u64>() + self.copy_back_ns
    }

    fn task_total(&self) -> u64 {
        self.tasks.iter().sum()
    }
}

#[derive(Clone, Debug)]
struct Failure {
    tick: u32,
    node: usize,
    chunk: usize,
    worker: usize,
    message: String,
}

/// The result of one run under one configuration.
struct Outcome {
    parameters: Parameters,
    tick_hashes: Vec<u64>,
    total: Duration,
    /// Main-thread wall time per phase for the barrier pools; busy time per
    /// node summed over the workers for the graphs.
    phases: Vec<(&'static str, u64)>,
    stats: WorkerStats,
    workers: usize,
    tasks_per_tick: u64,
    failure: Option<Failure>,
}

impl Outcome {
    fn run(&self) -> Run {
        Run {
            parameters: self.parameters,
            tick_hashes: self.tick_hashes.clone(),
            run_hash: run_hash(&self.tick_hashes),
            timing: Timing { phases: self.phases.clone(), total: self.total },
        }
    }

    fn utilization(&self) -> f64 {
        let capacity = self.workers as f64 * self.total.as_nanos() as f64;
        if capacity == 0.0 { 0.0 } else { self.stats.busy_total_ns() as f64 / capacity }
    }
}

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

/// Where a motion chunk leaves the energies: one sum per group of `CHUNK`
/// slots when the chunk covers whole groups (the reduction per chunk, then in
/// chunk order, of the specification), one value per slot when the chunk is
/// smaller than a group (the fixed point then sums the groups in slot order).
/// Both reproduce the specified order of additions.
struct EnergyScratch {
    _groups: Vec<f32>,
    _slots: Vec<f32>,
    groups: Column,
    slots: Column,
}

impl EnergyScratch {
    fn new(entity_count: usize) -> EnergyScratch {
        let mut groups = vec![0.0_f32; entity_count.div_ceil(CHUNK)];
        let mut slots = vec![0.0_f32; entity_count];
        let groups_column = Column::of(&mut groups);
        let slots_column = Column::of(&mut slots);
        EnergyScratch { _groups: groups, _slots: slots, groups: groups_column, slots: slots_column }
    }

    /// The tick's energy, added in the specified order, at the fixed point
    /// where no chunk writes the scratch any more.
    fn reduce(&self, alive: &[bool], chunk_size: usize) -> f32 {
        let n = alive.len();
        let group_count = n.div_ceil(CHUNK);
        let mut total = 0.0_f32;
        if chunk_size >= CHUNK {
            // SAFETY: the fixed point runs alone; nothing else touches the scratch.
            let groups = unsafe { self.groups.slice(0..group_count) };
            for sum in groups.iter() {
                total += *sum;
            }
        } else {
            // SAFETY: as above.
            let slots = unsafe { self.slots.slice(0..n) };
            for group in 0..group_count {
                let mut sum = 0.0_f32;
                for i in chunk_range(group, n) {
                    if alive[i] {
                        sum += slots[i];
                    }
                }
                total += sum;
            }
        }
        total
    }
}

fn chunk_range_sized(chunk: usize, chunk_size: usize, entity_count: usize) -> Range<usize> {
    let start = chunk * chunk_size;
    start..(start + chunk_size).min(entity_count)
}

/// Systems 3 to 6 for one slot, in the specified order: the new position and
/// velocity, and the slot's kinetic energy.
#[inline]
fn motion_slot(
    x: f32,
    y: f32,
    mut vx: f32,
    mut vy: f32,
    mass: f32,
    tick: u32,
    i: usize,
    wander: bool,
) -> (f32, f32, f32, f32, f32) {
    if wander {
        (vx, vy) = wandered_velocity(vx, vy, tick, i);
    }
    let (x, vx) = bounced(integrated(x, vx), vx);
    let (y, vy) = bounced(integrated(y, vy), vy);
    (x, y, vx, vy, kinetic_energy(vx, vy, mass))
}

/// Runs `body` on the alive slots of one motion chunk and sinks their
/// energies into the scratch in the group structure the specification fixes.
/// `body(i)` performs the motion of slot `i` and returns its energy.
fn motion_chunk(
    range: Range<usize>,
    chunk_size: usize,
    alive: &[bool],
    scratch: &EnergyScratch,
    mut body: impl FnMut(usize) -> f32,
) {
    let n = alive.len();
    if chunk_size >= CHUNK {
        let groups = (range.start / CHUNK)..range.end.div_ceil(CHUNK);
        // SAFETY: the chunk owns these groups: chunk sizes are multiples of the group size.
        let sums = unsafe { scratch.groups.slice(groups.clone()) };
        for (group, sum_slot) in groups.zip(sums.iter_mut()) {
            let mut sum = 0.0_f32;
            for i in chunk_range(group, n) {
                if alive[i] {
                    sum += body(i);
                }
            }
            *sum_slot = sum;
        }
    } else {
        // SAFETY: the chunk owns these slots.
        let slots = unsafe { scratch.slots.slice(range.clone()) };
        for (k, i) in range.enumerate() {
            if alive[i] {
                slots[k] = body(i);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The barrier pools
// ---------------------------------------------------------------------------

const PASS_IDLE: u8 = 0;
const PASS_REPULSION: u8 = 1;
const PASS_MOTION: u8 = 2;
const PASS_STOP: u8 = 3;

/// The outputs one chunk produces in a pass of the copy-back pool.
struct ChunkOutput {
    x: Vec<f32>,
    y: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    energy: f32,
}

/// The pool of the worker sweep spike: a persistent set of threads (the main
/// thread counts as one worker) synchronized by a barrier at both ends of
/// each parallel pass; the main thread runs the serial phases between passes.
struct Pool {
    config: Config,
    parameters: Parameters,
    state: UnsafeCell<State>,
    columns: Columns,
    grid: UnsafeCell<Grid>,
    scratch: EnergyScratch,
    outputs: Vec<Mutex<ChunkOutput>>,
    chunk_count: usize,
    next_chunk: AtomicUsize,
    pass: AtomicU8,
    barrier: Barrier,
    tick: AtomicU32,
}

// SAFETY: the state and the grid are read by every worker during a pass and
// written during a pass only through disjoint chunk slices (in place) or into
// the per-chunk outputs (copy back); between passes, the barrier makes the
// main thread the only thread touching them.
unsafe impl Sync for Pool {}

impl Pool {
    fn new(parameters: &Parameters, config: Config) -> Pool {
        let mut state = State::initialize(parameters);
        let columns = Columns::of(&mut state);
        let n = state.len();
        let chunk_count = n.div_ceil(config.chunk_size);
        let outputs = if config.scheduler == Scheduler::BarrierCopyBack {
            (0..chunk_count)
                .map(|_| {
                    Mutex::new(ChunkOutput {
                        x: vec![0.0; CHUNK],
                        y: vec![0.0; CHUNK],
                        vx: vec![0.0; CHUNK],
                        vy: vec![0.0; CHUNK],
                        energy: 0.0,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        Pool {
            config,
            parameters: *parameters,
            state: UnsafeCell::new(state),
            columns,
            grid: UnsafeCell::new(Grid::new()),
            scratch: EnergyScratch::new(n),
            outputs,
            chunk_count,
            next_chunk: AtomicUsize::new(0),
            pass: AtomicU8::new(PASS_IDLE),
            barrier: Barrier::new(config.workers),
            tick: AtomicU32::new(0),
        }
    }

    /// Claims chunks until none is left and processes each one.
    fn work(&self, pass: u8, stats: &mut WorkerStats) {
        // SAFETY: see the `Sync` implementation: shared reads during a pass.
        let state = unsafe { &*self.state.get() };
        let grid = unsafe { &*self.grid.get() };
        let tick = self.tick.load(Ordering::Relaxed);
        let wander = self.parameters.wander;
        let n = state.len();
        let node = if pass == PASS_REPULSION { NODE_REPULSION } else { NODE_MOTION };
        loop {
            let chunk = self.next_chunk.fetch_add(1, Ordering::AcqRel);
            if chunk >= self.chunk_count {
                break;
            }
            let started = Instant::now();
            let range = chunk_range_sized(chunk, self.config.chunk_size, n);
            if self.config.kernel == Kernel::Workload {
                match (self.config.scheduler, pass) {
                    (Scheduler::BarrierCopyBack, PASS_REPULSION) => {
                        let mut out = self.outputs[chunk].lock().unwrap();
                        for i in range {
                            if state.alive[i] {
                                let (fx, fy) = repulsion_force(state, grid, i);
                                out.vx[i % CHUNK] = repelled_velocity(state.vx[i], fx, state.mass[i]);
                                out.vy[i % CHUNK] = repelled_velocity(state.vy[i], fy, state.mass[i]);
                            }
                        }
                    }
                    (Scheduler::BarrierCopyBack, PASS_MOTION) => {
                        let mut out = self.outputs[chunk].lock().unwrap();
                        let mut sum = 0.0_f32;
                        for i in range {
                            if state.alive[i] {
                                let (x, y, vx, vy, e) = motion_slot(
                                    state.x[i], state.y[i], state.vx[i], state.vy[i], state.mass[i], tick, i,
                                    wander,
                                );
                                let k = i % CHUNK;
                                out.x[k] = x;
                                out.y[k] = y;
                                out.vx[k] = vx;
                                out.vy[k] = vy;
                                sum += e;
                            }
                        }
                        out.energy = sum;
                    }
                    (Scheduler::BarrierInPlace, PASS_REPULSION) => {
                        // SAFETY: this chunk's slots of the velocity columns are written by this
                        // task only; the pass reads positions, which nobody writes.
                        let (_, _, vx, vy) = unsafe { self.columns.slices(range.clone()) };
                        for (k, i) in range.enumerate() {
                            if state.alive[i] {
                                let (fx, fy) = repulsion_force(state, grid, i);
                                vx[k] = repelled_velocity(vx[k], fx, state.mass[i]);
                                vy[k] = repelled_velocity(vy[k], fy, state.mass[i]);
                            }
                        }
                    }
                    (Scheduler::BarrierInPlace, PASS_MOTION) => {
                        // SAFETY: this chunk's slots of the four columns are read and written
                        // by this task only.
                        let (x, y, vx, vy) = unsafe { self.columns.slices(range.clone()) };
                        let start = range.start;
                        motion_chunk(range, self.config.chunk_size, &state.alive, &self.scratch, |i| {
                            let k = i - start;
                            let (nx, ny, nvx, nvy, e) =
                                motion_slot(x[k], y[k], vx[k], vy[k], state.mass[i], tick, i, wander);
                            x[k] = nx;
                            y[k] = ny;
                            vx[k] = nvx;
                            vy[k] = nvy;
                            e
                        });
                    }
                    _ => unreachable!("the pool runs the barrier schedulers only"),
                }
            }
            stats.busy_ns[node] += started.elapsed().as_nanos() as u64;
            stats.tasks[node] += 1;
        }
    }

    fn worker_loop(&self) -> WorkerStats {
        let mut stats = WorkerStats::default();
        loop {
            let waiting = Instant::now();
            self.barrier.wait();
            stats.wait_ns += waiting.elapsed().as_nanos() as u64;
            let pass = self.pass.load(Ordering::Acquire);
            if pass == PASS_STOP {
                return stats;
            }
            self.work(pass, &mut stats);
            let waiting = Instant::now();
            self.barrier.wait();
            stats.wait_ns += waiting.elapsed().as_nanos() as u64;
        }
    }

    /// Runs one parallel pass on every worker, the calling thread included.
    fn run_pass(&self, pass: u8, stats: &mut WorkerStats) {
        self.next_chunk.store(0, Ordering::Relaxed);
        self.pass.store(pass, Ordering::Release);
        self.barrier.wait();
        self.work(pass, stats);
        let waiting = Instant::now();
        self.barrier.wait();
        stats.wait_ns += waiting.elapsed().as_nanos() as u64;
    }

    fn stop(&self) {
        self.pass.store(PASS_STOP, Ordering::Release);
        self.barrier.wait();
    }

    fn copy_back(&self, pass: u8) {
        // SAFETY: between passes, the main thread is the only thread touching the state.
        let state = unsafe { &mut *self.state.get() };
        for (chunk, output) in self.outputs.iter().enumerate() {
            let out = output.lock().unwrap();
            for i in chunk_range(chunk, state.len()) {
                if state.alive[i] {
                    let k = i % CHUNK;
                    state.vx[i] = out.vx[k];
                    state.vy[i] = out.vy[k];
                    if pass == PASS_MOTION {
                        state.x[i] = out.x[k];
                        state.y[i] = out.y[k];
                    }
                }
            }
        }
    }

    /// One tick, driven by the main thread; the hash is taken by the caller.
    fn tick(&self, tick: u32, timing: &mut Timing, stats: &mut WorkerStats) {
        self.tick.store(tick, Ordering::Relaxed);
        let copy_back = self.config.scheduler == Scheduler::BarrierCopyBack;
        if self.config.kernel == Kernel::Workload {
            timing.charge("spatial_hashing", || {
                // SAFETY: between passes, the main thread is alone.
                let state = unsafe { &*self.state.get() };
                let grid = unsafe { &mut *self.grid.get() };
                grid.build(state);
            });
        }
        timing.charge("repulsion", || self.run_pass(PASS_REPULSION, stats));
        if copy_back && self.config.kernel == Kernel::Workload {
            timing.charge("copy_back", || self.copy_back(PASS_REPULSION));
        }
        timing.charge("motion", || self.run_pass(PASS_MOTION, stats));
        if copy_back && self.config.kernel == Kernel::Workload {
            timing.charge("copy_back", || self.copy_back(PASS_MOTION));
        }
        if self.config.kernel == Kernel::Workload {
            timing.charge("structural_changes", || {
                // SAFETY: between passes, the main thread is alone.
                let state = unsafe { &mut *self.state.get() };
                state.energy = if copy_back {
                    let mut total = 0.0_f32;
                    for output in &self.outputs {
                        total += output.lock().unwrap().energy;
                    }
                    total
                } else {
                    self.scratch.reduce(&state.alive, self.config.chunk_size)
                };
                state.structural_changes();
            });
        }
    }

    fn hash(&self) -> u64 {
        // SAFETY: between passes, the main thread is alone.
        unsafe { &*self.state.get() }.hash()
    }

    fn run(&self) -> Outcome {
        let tick_count = self.parameters.tick_count;
        let workload = self.config.kernel == Kernel::Workload;
        let mut tick_hashes = Vec::with_capacity(tick_count as usize + 1);
        let mut timing = Timing::new(&POOL_PHASES);
        let mut main = WorkerStats::default();
        let mut stats = WorkerStats::default();
        thread::scope(|scope| {
            let handles: Vec<_> = (1..self.config.workers).map(|_| scope.spawn(|| self.worker_loop())).collect();
            let started = Instant::now();
            if workload {
                tick_hashes.push(timing.charge("hash", || self.hash()));
            }
            for t in 1..=tick_count {
                self.tick(t, &mut timing, &mut main);
                if workload {
                    tick_hashes.push(timing.charge("hash", || self.hash()));
                }
            }
            timing.total = started.elapsed();
            self.stop();
            for handle in handles {
                stats.add(&handle.join().expect("a pool worker never panics"));
            }
        });
        // The main thread's serial phases are its busy time outside the passes.
        let phase = |name: &str| timing.phases.iter().find(|(n, _)| *n == name).map_or(0, |(_, ns)| *ns);
        main.busy_ns[NODE_SPATIAL] += phase("spatial_hashing");
        main.tasks[NODE_SPATIAL] += u64::from(tick_count) * u64::from(workload);
        main.copy_back_ns += phase("copy_back");
        main.busy_ns[NODE_STRUCTURAL] += phase("structural_changes");
        main.tasks[NODE_STRUCTURAL] += u64::from(tick_count) * u64::from(workload);
        main.busy_ns[NODE_HASH] += phase("hash");
        main.tasks[NODE_HASH] += (u64::from(tick_count) + 1) * u64::from(workload);
        stats.add(&main);
        Outcome {
            parameters: self.parameters,
            tick_hashes,
            total: timing.total,
            phases: timing.phases,
            stats,
            workers: self.config.workers,
            tasks_per_tick: 3 + 2 * self.chunk_count as u64,
            failure: None,
        }
    }
}

// ---------------------------------------------------------------------------
// The dependency-counting task graph
// ---------------------------------------------------------------------------

/// One node of the tick graph: a serial phase (one chunk) or a parallel pass
/// (one chunk per slice of the population).
struct Node {
    chunks: usize,
    /// The central counter: the next chunk to claim.
    next: AtomicUsize,
    finished: AtomicUsize,
    /// Predecessors still to complete (used by the structural node, which
    /// waits for the motion pass of its tick and the hash of the previous one).
    pending: AtomicUsize,
    /// The stealing variant: per worker, the packed `(next, end)` range of
    /// chunks it owns; the owner claims from the front, thieves from the back.
    ranges: Vec<AtomicU64>,
}

impl Node {
    fn new(chunks: usize, workers: usize) -> Node {
        Node {
            chunks,
            next: AtomicUsize::new(chunks),
            finished: AtomicUsize::new(0),
            pending: AtomicUsize::new(0),
            ranges: (0..workers).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

fn pack(next: usize, end: usize) -> u64 {
    ((next as u64) << 32) | end as u64
}

fn unpack(packed: u64) -> (usize, usize) {
    ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize)
}

/// The owner claims the front chunk of its range.
fn claim_own(range: &AtomicU64) -> Option<usize> {
    let mut packed = range.load(Ordering::Acquire);
    loop {
        let (next, end) = unpack(packed);
        if next >= end {
            return None;
        }
        match range.compare_exchange_weak(packed, pack(next + 1, end), Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(next),
            Err(current) => packed = current,
        }
    }
}

/// A thief takes the back chunk of a victim's range, one chunk per steal as a
/// work-stealing deque does. (Stealing a half and keeping it as the thief's
/// own range needs a store into that range, which races with the arming of
/// the node when a worker holding a stale view of the ready nodes steals
/// while the ranges are being written: the chunks it took were then lost.)
fn steal_one(range: &AtomicU64) -> Option<usize> {
    let mut packed = range.load(Ordering::Acquire);
    loop {
        let (next, end) = unpack(packed);
        if next >= end {
            return None;
        }
        match range.compare_exchange_weak(packed, pack(next, end - 1), Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(end - 1),
            Err(current) => packed = current,
        }
    }
}

/// A tick graph over double-buffered state. The state after tick `t` lives
/// in `states[current]`; the next tick's passes read it and write the other
/// buffer, while the hash of tick `t` reads it too. The structural node runs
/// alone (every other node of its tick and the previous hash completed, the
/// next tick is not armed yet) and is the fixed point where structural
/// changes are applied and the roles of the buffers swap.
struct Graph {
    config: Config,
    parameters: Parameters,
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
    nodes: [Node; NODE_COUNT],
    ready: AtomicU32,
    /// The tick whose spatial hashing, passes and structural changes are armed.
    tick: AtomicU32,
    /// The tick whose hash is armed.
    hash_tick: AtomicU32,
    done: AtomicBool,
    failed: AtomicBool,
    failure: Mutex<Option<Failure>>,
    tick_hashes: Vec<AtomicU64>,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
    started: OnceLock<Instant>,
    finished_ns: AtomicU64,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, read by the passes and the hash of the previous
// tick otherwise, and the structural node runs alone.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(parameters: &Parameters, config: Config) -> Graph {
        assert!(config.scheduler.is_graph());
        let mut a = State::initialize(parameters);
        let mut b = State::initialize(parameters);
        let columns = [Columns::of(&mut a), Columns::of(&mut b)];
        let n = a.len();
        let chunk_count = n.div_ceil(config.chunk_size);
        let w = config.workers;
        Graph {
            config,
            parameters: *parameters,
            states: [UnsafeCell::new(a), UnsafeCell::new(b)],
            columns,
            grid: UnsafeCell::new(Grid::new()),
            scratch: EnergyScratch::new(n),
            dead: UnsafeCell::new(Vec::new()),
            changed: UnsafeCell::new(Vec::new()),
            current: AtomicUsize::new(0),
            nodes: [
                Node::new(1, w),
                Node::new(chunk_count, w),
                Node::new(chunk_count, w),
                Node::new(1, w),
                Node::new(1, w),
            ],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(0),
            hash_tick: AtomicU32::new(0),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            tick_hashes: (0..=parameters.tick_count).map(|_| AtomicU64::new(0)).collect(),
            parked: (0..w).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
            started: OnceLock::new(),
            finished_ns: AtomicU64::new(0),
        }
    }

    fn tasks_per_tick(&self) -> u64 {
        3 + 2 * self.nodes[NODE_REPULSION].chunks as u64
    }

    /// The scheduler's state in one line, for the watchdog of the self-checks.
    #[cfg(test)]
    fn describe(&self) -> String {
        let nodes: Vec<String> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(node, n)| {
                let ranges: Vec<String> = n.ranges[..self.config.workers]
                    .iter()
                    .map(|r| {
                        let (a, b) = unpack(r.load(Ordering::SeqCst));
                        format!("{a}..{b}")
                    })
                    .collect();
                format!(
                    "{}: next {} finished {} of {} pending {} ranges [{}]",
                    NODE_NAMES[node],
                    n.next.load(Ordering::SeqCst),
                    n.finished.load(Ordering::SeqCst),
                    n.chunks,
                    n.pending.load(Ordering::SeqCst),
                    ranges.join(", ")
                )
            })
            .collect();
        let parked: Vec<bool> = self.parked.iter().map(|p| p.load(Ordering::SeqCst)).collect();
        format!(
            "ready {:05b} tick {} hash_tick {} done {} failed {} parked {:?}\n  {}",
            self.ready.load(Ordering::SeqCst),
            self.tick.load(Ordering::SeqCst),
            self.hash_tick.load(Ordering::SeqCst),
            self.done.load(Ordering::SeqCst),
            self.failed.load(Ordering::SeqCst),
            parked,
            nodes.join("\n  ")
        )
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
        let range = chunk_range_sized(chunk, self.config.chunk_size, cur.len());
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
        let wander = self.parameters.wander;
        let range = chunk_range_sized(chunk, self.config.chunk_size, cur.len());
        let next = 1 - self.current.load(Ordering::Relaxed);
        // SAFETY: this chunk's slots of the next buffer (velocities written by the
        // completed repulsion pass, positions garbage) are read and written by this
        // task only.
        let (x, y, vx, vy) = unsafe { self.columns[next].slices(range.clone()) };
        let start = range.start;
        motion_chunk(range, self.config.chunk_size, &cur.alive, &self.scratch, |i| {
            let k = i - start;
            let (nx, ny, nvx, nvy, e) = motion_slot(cur.x[i], cur.y[i], vx[k], vy[k], cur.mass[i], tick, i, wander);
            x[k] = nx;
            y[k] = ny;
            vx[k] = nvx;
            vy[k] = nvy;
            e
        });
    }

    /// The fixed point: brings the next buffer's liveness, generations,
    /// masses and stream up to date, reduces the energy, applies the kills and
    /// spawns, records which slots they touched, and swaps the buffers.
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
        next.energy = self.scratch.reduce(&cur.alive, self.config.chunk_size);
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
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed) as usize;
        self.tick_hashes[tick].store(self.current_state().hash(), Ordering::Relaxed);
    }

    fn execute(&self, node: usize, chunk: usize) {
        if let Some(injection) = self.config.injection {
            let tick = if node == NODE_HASH { self.hash_tick.load(Ordering::Relaxed) } else { self.tick.load(Ordering::Relaxed) };
            if injection.node == node && injection.chunk == chunk && injection.tick == tick {
                panic!("injected panic in {} chunk {chunk} of tick {tick}", NODE_NAMES[node]);
            }
        }
        if self.config.kernel == Kernel::Empty {
            return;
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
            let tick = if node == NODE_HASH { self.hash_tick.load(Ordering::Relaxed) } else { self.tick.load(Ordering::Relaxed) };
            let mut failure = self.failure.lock().unwrap();
            if failure.is_none() {
                *failure = Some(Failure { tick, node, chunk, worker, message });
            }
            self.failed.store(true, Ordering::SeqCst);
        }
    }

    // --- the scheduler ----------------------------------------------------

    fn arm(&self, node: usize, worker: usize) {
        let n = &self.nodes[node];
        n.finished.store(0, Ordering::Relaxed);
        match self.config.scheduler {
            Scheduler::GraphCounter => n.next.store(0, Ordering::Release),
            Scheduler::GraphStealing => {
                let w = self.config.workers;
                for v in 0..w {
                    let range = if n.chunks == 1 {
                        if v == worker { pack(0, 1) } else { pack(0, 0) }
                    } else {
                        pack(v * n.chunks / w, (v + 1) * n.chunks / w)
                    };
                    n.ranges[v].store(range, Ordering::Release);
                }
            }
            _ => unreachable!(),
        }
        self.ready.fetch_or(1 << node, Ordering::SeqCst);
    }

    fn release_structural(&self, worker: usize) {
        if self.nodes[NODE_STRUCTURAL].pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.arm(NODE_STRUCTURAL, worker);
        }
    }

    fn finish_run(&self) {
        let started = self.started.get().expect("the run started");
        self.finished_ns.store(started.elapsed().as_nanos() as u64, Ordering::Release);
        self.done.store(true, Ordering::SeqCst);
        self.wake(self.config.workers);
    }

    /// Called once per node per tick, by the worker that finished its last
    /// chunk. After a failure nothing new is armed except the hash the
    /// structural node owes, and the run ends at the first hash that completes.
    fn complete(&self, node: usize, worker: usize) {
        self.ready.fetch_and(!(1 << node), Ordering::SeqCst);
        match node {
            NODE_SPATIAL => {
                self.arm(NODE_REPULSION, worker);
                self.wake(self.config.workers);
            }
            NODE_REPULSION => {
                self.arm(NODE_MOTION, worker);
                self.wake(self.config.workers);
            }
            NODE_MOTION => self.release_structural(worker),
            NODE_HASH => {
                let last = self.hash_tick.load(Ordering::Relaxed) == self.parameters.tick_count;
                if last || self.failed.load(Ordering::SeqCst) {
                    self.finish_run();
                } else {
                    self.release_structural(worker);
                }
            }
            NODE_STRUCTURAL => {
                let tick = self.tick.load(Ordering::Relaxed);
                self.hash_tick.store(tick, Ordering::Relaxed);
                if tick < self.parameters.tick_count && !self.failed.load(Ordering::SeqCst) {
                    self.tick.store(tick + 1, Ordering::Relaxed);
                    self.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
                    self.arm(NODE_SPATIAL, worker);
                    self.arm(NODE_HASH, worker);
                    // The arming worker takes one of the two serial nodes; one more worker is needed.
                    self.wake(1);
                } else {
                    self.arm(NODE_HASH, worker);
                }
            }
            _ => unreachable!("five nodes"),
        }
    }

    fn node_has_work(&self, node: usize) -> bool {
        let n = &self.nodes[node];
        match self.config.scheduler {
            Scheduler::GraphCounter => n.next.load(Ordering::Relaxed) < n.chunks,
            Scheduler::GraphStealing => n.ranges[..self.config.workers].iter().any(|range| {
                let (next, end) = unpack(range.load(Ordering::Relaxed));
                next < end
            }),
            _ => unreachable!(),
        }
    }

    fn has_work(&self) -> bool {
        let mask = self.ready.load(Ordering::SeqCst);
        mask != 0 && (0..NODE_COUNT).any(|node| mask & (1 << node) != 0 && self.node_has_work(node))
    }

    fn try_claim(&self, worker: usize, stats: &mut WorkerStats) -> Option<(usize, usize)> {
        let mask = self.ready.load(Ordering::SeqCst);
        if mask == 0 {
            return None;
        }
        for node in CLAIM_ORDER {
            if mask & (1 << node) == 0 {
                continue;
            }
            let n = &self.nodes[node];
            match self.config.scheduler {
                Scheduler::GraphCounter => {
                    if n.next.load(Ordering::Relaxed) < n.chunks {
                        let chunk = n.next.fetch_add(1, Ordering::AcqRel);
                        if chunk < n.chunks {
                            return Some((node, chunk));
                        }
                    }
                }
                Scheduler::GraphStealing => {
                    if let Some(chunk) = claim_own(&n.ranges[worker]) {
                        return Some((node, chunk));
                    }
                    let w = self.config.workers;
                    for offset in 1..w {
                        let victim = (worker + offset) % w;
                        if let Some(chunk) = steal_one(&n.ranges[victim]) {
                            stats.steals += 1;
                            return Some((node, chunk));
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        None
    }

    fn park(&self, worker: usize, stats: &mut WorkerStats) {
        self.parked[worker].store(true, Ordering::SeqCst);
        if !self.has_work() && !self.done.load(Ordering::SeqCst) {
            stats.parks += 1;
            thread::park();
        }
        self.parked[worker].store(false, Ordering::SeqCst);
    }

    /// Wakes up to `count` parked workers.
    fn wake(&self, count: usize) {
        if count == 0 || self.config.waiting == Waiting::Spin {
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

    fn wait(&self, worker: usize, stats: &mut WorkerStats) {
        let started = Instant::now();
        match self.config.waiting {
            Waiting::Spin => {
                while !self.has_work() && !self.done.load(Ordering::SeqCst) {
                    std::hint::spin_loop();
                }
            }
            Waiting::Park => self.park(worker, stats),
            Waiting::SpinThenPark(bound) => {
                let mut spins = 0_u32;
                loop {
                    if self.has_work() || self.done.load(Ordering::SeqCst) {
                        break;
                    }
                    std::hint::spin_loop();
                    spins += 1;
                    if spins % 32 == 0 && started.elapsed() >= bound {
                        self.park(worker, stats);
                        break;
                    }
                }
            }
        }
        stats.wait_ns += started.elapsed().as_nanos() as u64;
    }

    fn worker_loop(&self, worker: usize) -> WorkerStats {
        let mut stats = WorkerStats::default();
        loop {
            match self.try_claim(worker, &mut stats) {
                Some((node, chunk)) => {
                    let started = Instant::now();
                    self.run_task(node, chunk, worker);
                    stats.busy_ns[node] += started.elapsed().as_nanos() as u64;
                    stats.tasks[node] += 1;
                    let n = &self.nodes[node];
                    if n.finished.fetch_add(1, Ordering::AcqRel) + 1 == n.chunks {
                        self.complete(node, worker);
                    }
                }
                None => {
                    if self.done.load(Ordering::SeqCst) {
                        return stats;
                    }
                    self.wait(worker, &mut stats);
                }
            }
        }
    }

    fn run(&self) -> Outcome {
        let mut stats = WorkerStats::default();
        thread::scope(|scope| {
            let handles: Vec<_> = (1..self.config.workers)
                .map(|worker| {
                    thread::Builder::new()
                        .name(format!("worker-{worker}"))
                        .spawn_scoped(scope, move || self.worker_loop(worker))
                        .expect("spawn a worker")
                })
                .collect();
            let mut threads = vec![thread::current()];
            threads.extend(handles.iter().map(|handle| handle.thread().clone()));
            self.threads.set(threads).ok().expect("threads are published once");
            self.started.set(Instant::now()).ok().expect("the run starts once");
            // Tick 1 and the hash of the initial state start together.
            self.tick.store(1, Ordering::Relaxed);
            self.hash_tick.store(0, Ordering::Relaxed);
            self.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
            self.arm(NODE_SPATIAL, 0);
            self.arm(NODE_HASH, 0);
            self.wake(1);
            stats = self.worker_loop(0);
            for handle in handles {
                stats.add(&handle.join().expect("a worker never panics outside a task"));
            }
        });
        let total = Duration::from_nanos(self.finished_ns.load(Ordering::Acquire));
        let tick_hashes = if self.config.kernel == Kernel::Workload {
            let last = self.hash_tick.load(Ordering::Relaxed) as usize;
            (0..=last).map(|t| self.tick_hashes[t].load(Ordering::Relaxed)).collect()
        } else {
            Vec::new()
        };
        let phases = NODE_NAMES.iter().enumerate().map(|(node, name)| (*name, stats.busy_ns[node])).collect();
        Outcome {
            parameters: self.parameters,
            tick_hashes,
            total,
            phases,
            stats,
            workers: self.config.workers,
            tasks_per_tick: self.tasks_per_tick(),
            failure: self.failure.lock().unwrap().clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Runs and reports
// ---------------------------------------------------------------------------

fn run_once(parameters: &Parameters, config: Config) -> Outcome {
    if config.scheduler.is_graph() {
        Graph::new(parameters, config).run()
    } else {
        Pool::new(parameters, config).run()
    }
}

/// A warm-up instance of `WARM_UP_TICKS` ticks, then the measured instance.
fn measure(parameters: &Parameters, config: Config) -> Outcome {
    let warm_up = Parameters { tick_count: WARM_UP_TICKS, ..*parameters };
    run_once(&warm_up, config);
    run_once(parameters, config)
}

fn write_config(out: &mut Json, config: Config) {
    out.string("scheduler", config.scheduler.name());
    out.string("waiting", config.waiting.name());
    out.i64("spin_bound_us", config.waiting.spin_bound_us());
    out.u64("chunk_size", config.chunk_size as u64);
    out.u64("workers", config.workers as u64);
    out.string("kernel", if config.kernel == Kernel::Workload { "workload" } else { "empty" });
}

fn write_stats(out: &mut Json, outcome: &Outcome) {
    out.u64("tasks_per_tick", outcome.tasks_per_tick);
    out.u64("tasks", outcome.stats.task_total());
    out.decimal("utilization", outcome.utilization());
    out.key("busy_ms");
    out.object_start();
    for (node, name) in NODE_NAMES.iter().enumerate() {
        out.decimal(name, outcome.stats.busy_ns[node] as f64 / 1_000_000.0);
    }
    out.decimal("copy_back", outcome.stats.copy_back_ns as f64 / 1_000_000.0);
    out.object_end();
    out.decimal("wait_ms", outcome.stats.wait_ns as f64 / 1_000_000.0);
    out.u64("parks", outcome.stats.parks);
    out.u64("steals", outcome.stats.steals);
}

fn write_workload_run(out: &mut Json, config: Config, outcome: &Outcome, reference: &Run) {
    let run = outcome.run();
    out.object_start();
    write_config(out, config);
    write_run_core(out, &run);
    out.string(
        "phase_ms_meaning",
        if config.scheduler.is_graph() {
            "busy time per node summed over the workers"
        } else {
            "main-thread wall time per phase"
        },
    );
    let divergence = first_divergent_tick(&run.tick_hashes, &reference.tick_hashes);
    out.bool("matches_sequential", divergence.is_none());
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    write_stats(out, outcome);
    out.object_end();
}

fn write_sequential_run(out: &mut Json, reference: &Run) {
    out.object_start();
    out.string("scheduler", "sequential");
    out.string("waiting", "none");
    out.i64("spin_bound_us", 0);
    out.u64("chunk_size", CHUNK as u64);
    out.u64("workers", 1);
    out.string("kernel", "workload");
    write_run_core(out, reference);
    out.string("phase_ms_meaning", "main-thread wall time per phase");
    out.bool("matches_sequential", true);
    out.i64("first_divergent_tick", -1);
    out.object_end();
}

fn write_empty_run(out: &mut Json, config: Config, outcome: &Outcome) {
    out.object_start();
    write_config(out, config);
    let ticks = u64::from(outcome.parameters.tick_count);
    let total_us = outcome.total.as_secs_f64() * 1_000_000.0;
    out.u64("ticks", ticks);
    out.decimal("total_ms", total_us / 1_000.0);
    out.decimal("microseconds_per_tick", total_us / ticks as f64);
    out.decimal("microseconds_per_task", total_us / (ticks * outcome.tasks_per_tick) as f64);
    write_stats(out, outcome);
    out.object_end();
}

fn write_injection_run(out: &mut Json, config: Config, outcome: &Outcome, reference: &Run) {
    let injection = config.injection.expect("an injection run");
    out.object_start();
    write_config(out, config);
    out.string("size", outcome.parameters.size.name());
    out.u64("tick_count", u64::from(outcome.parameters.tick_count));
    out.key("injected");
    out.object_start();
    out.u64("tick", u64::from(injection.tick));
    out.string("node", NODE_NAMES[injection.node]);
    out.u64("chunk", injection.chunk as u64);
    out.object_end();
    out.bool("pool_terminated", true);
    out.bool("failure_reported", outcome.failure.is_some());
    if let Some(failure) = &outcome.failure {
        out.key("failure");
        out.object_start();
        out.u64("tick", u64::from(failure.tick));
        out.string("node", NODE_NAMES[failure.node]);
        out.u64("chunk", failure.chunk as u64);
        out.u64("worker", failure.worker as u64);
        out.string("message", &failure.message);
        out.object_end();
    }
    out.u64("ticks_hashed", outcome.tick_hashes.len().saturating_sub(1) as u64);
    let before_failure = outcome.tick_hashes.len().saturating_sub(1).min(reference.tick_hashes.len());
    out.bool(
        "hashes_before_the_failure_match",
        outcome.tick_hashes[..before_failure] == reference.tick_hashes[..before_failure],
    );
    out.decimal("total_ms", outcome.total.as_secs_f64() * 1_000.0);
    out.object_end();
}

/// Reads a comma-separated list of worker counts from the environment
/// variable, sorted and without duplicates; `None` when the variable is
/// absent or empty. A value that is not a positive integer is a usage error
/// and stops the run with the offending value named.
fn worker_counts_from_env(variable: &str) -> Option<Vec<usize>> {
    let value = std::env::var(variable).ok()?;
    if value.trim().is_empty() {
        return None;
    }
    let mut counts: Vec<usize> = value
        .split(',')
        .map(|item| match item.trim().parse::<usize>() {
            Ok(count) if count > 0 => count,
            _ => panic!("{variable} holds {item:?}, not a positive worker count"),
        })
        .collect();
    counts.sort_unstable();
    counts.dedup();
    Some(counts)
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut worker_counts = BASE_WORKER_COUNTS.to_vec();
    if !worker_counts.contains(&available) {
        worker_counts.push(available);
        worker_counts.sort_unstable();
    }
    let mut sweep_workers = vec![SWEEP_WORKERS];
    if available > BASE_WORKER_COUNTS[BASE_WORKER_COUNTS.len() - 1] {
        sweep_workers.push(available);
    }
    // A machine outside the matrix may measure the scaling curve at every
    // worker count of a range (for example 1 to 16 on a 16-core machine)
    // instead of the base counts plus its own parallelism.
    let worker_counts = worker_counts_from_env("SPIKE_WORKER_COUNTS").unwrap_or(worker_counts);
    let sweep_workers = worker_counts_from_env("SPIKE_SWEEP_WORKERS").unwrap_or(sweep_workers);
    let default_waiting = Waiting::SpinThenPark(DEFAULT_SPIN_BOUND);
    let parameters = Parameters::new(Size::Default, DEFAULT_TICK_COUNT, false);
    let reference = run_sequential(&parameters);

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.key("worker_counts");
    out.array_start();
    for &w in &worker_counts {
        out.object_start();
        out.u64("workers", w as u64);
        out.object_end();
    }
    out.array_end();

    out.key("runs");
    out.array_start();
    write_sequential_run(&mut out, &reference);
    for scheduler in Scheduler::ALL {
        for &workers in &worker_counts {
            let config = Config::new(scheduler, default_waiting, CHUNK, workers);
            write_workload_run(&mut out, config, &measure(&parameters, config), &reference);
        }
    }
    for waiting in [Waiting::Park, Waiting::Spin] {
        for &workers in &worker_counts {
            let config = Config::new(Scheduler::GraphCounter, waiting, CHUNK, workers);
            write_workload_run(&mut out, config, &measure(&parameters, config), &reference);
        }
    }
    out.array_end();

    out.key("chunk_size_sweep");
    out.array_start();
    for scheduler in [Scheduler::BarrierInPlace, Scheduler::GraphCounter] {
        for chunk_size in CHUNK_SIZE_SWEEP {
            for &workers in &sweep_workers {
                let config = Config::new(scheduler, default_waiting, chunk_size, workers);
                write_workload_run(&mut out, config, &measure(&parameters, config), &reference);
            }
        }
    }
    out.array_end();

    out.key("spin_bound_sweep");
    out.array_start();
    for bound_us in SPIN_BOUND_SWEEP_US {
        for &workers in &sweep_workers {
            let waiting = Waiting::SpinThenPark(Duration::from_micros(bound_us));
            let config = Config::new(Scheduler::GraphCounter, waiting, CHUNK, workers);
            write_workload_run(&mut out, config, &measure(&parameters, config), &reference);
        }
    }
    out.array_end();

    out.key("empty_graph");
    out.array_start();
    let empty_parameters = Parameters { tick_count: EMPTY_TICKS, ..parameters };
    for scheduler in [Scheduler::BarrierInPlace, Scheduler::GraphCounter, Scheduler::GraphStealing] {
        for &workers in &worker_counts {
            let config = Config::new(scheduler, default_waiting, CHUNK, workers).empty();
            write_empty_run(&mut out, config, &measure(&empty_parameters, config));
        }
    }
    out.array_end();

    out.key("panic_injection");
    let small = Parameters::new(Size::Small, 20, false);
    let small_reference = run_sequential(&small);
    let injection = Injection { tick: 3, node: NODE_REPULSION, chunk: 2 };
    let config = Config::new(Scheduler::GraphCounter, default_waiting, CHUNK, SWEEP_WORKERS.min(available.max(2))).injecting(injection);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = run_once(&small, config);
    std::panic::set_hook(previous_hook);
    write_injection_run(&mut out, config, &outcome, &small_reference);

    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_counts_from_the_environment_are_sorted_and_distinct() {
        // SAFETY: the tests of this module that read the environment run on
        // one variable each, named after the test, so no other thread reads
        // or writes it.
        unsafe { std::env::set_var("SPIKE_TEST_WORKER_COUNTS", " 8, 1,16,1, 4 ") };
        assert_eq!(worker_counts_from_env("SPIKE_TEST_WORKER_COUNTS"), Some(vec![1, 4, 8, 16]));
        unsafe { std::env::set_var("SPIKE_TEST_WORKER_COUNTS_EMPTY", "  ") };
        assert_eq!(worker_counts_from_env("SPIKE_TEST_WORKER_COUNTS_EMPTY"), None);
        assert_eq!(worker_counts_from_env("SPIKE_TEST_WORKER_COUNTS_ABSENT"), None);
    }

    #[test]
    #[should_panic(expected = "not a positive worker count")]
    fn a_worker_count_that_is_not_a_positive_integer_is_refused() {
        // SAFETY: see the test above.
        unsafe { std::env::set_var("SPIKE_TEST_WORKER_COUNTS_BAD", "1,0,4") };
        worker_counts_from_env("SPIKE_TEST_WORKER_COUNTS_BAD");
    }

    fn small(wander: bool) -> Parameters {
        Parameters::new(Size::Small, 40, wander)
    }

    fn assert_matches(config: Config, parameters: &Parameters, reference: &Run) {
        let outcome = run_once(parameters, config);
        assert_eq!(
            first_divergent_tick(&outcome.tick_hashes, &reference.tick_hashes),
            None,
            "{config:?} wander {}",
            parameters.wander
        );
        assert_eq!(outcome.run().run_hash, reference.run_hash);
        assert!(outcome.failure.is_none());
        assert!(outcome.total > Duration::ZERO);
    }

    #[test]
    fn every_scheduler_reproduces_the_sequential_hashes_at_every_worker_count() {
        let reference = run_sequential(&small(false));
        for scheduler in Scheduler::ALL {
            for workers in [1, 2, 3, 8] {
                let config = Config::new(scheduler, Waiting::SpinThenPark(DEFAULT_SPIN_BOUND), CHUNK, workers);
                assert_matches(config, &small(false), &reference);
            }
        }
    }

    #[test]
    fn the_waiting_policies_do_not_change_the_hashes() {
        let reference = run_sequential(&small(false));
        for waiting in [Waiting::Park, Waiting::Spin, Waiting::SpinThenPark(Duration::from_micros(5))] {
            for scheduler in [Scheduler::GraphCounter, Scheduler::GraphStealing] {
                assert_matches(Config::new(scheduler, waiting, CHUNK, 3), &small(false), &reference);
            }
        }
    }

    #[test]
    fn the_wander_system_runs_with_the_right_tick_under_every_scheduler() {
        let reference = run_sequential(&small(true));
        for scheduler in Scheduler::ALL {
            let config = Config::new(scheduler, Waiting::SpinThenPark(DEFAULT_SPIN_BOUND), CHUNK, 3);
            assert_matches(config, &small(true), &reference);
        }
    }

    #[test]
    fn every_chunk_size_reproduces_the_hashes() {
        let reference = run_sequential(&small(false));
        for scheduler in [Scheduler::BarrierInPlace, Scheduler::GraphCounter, Scheduler::GraphStealing] {
            for chunk_size in [64, 128, 512, 1_024] {
                let config = Config::new(scheduler, Waiting::SpinThenPark(DEFAULT_SPIN_BOUND), chunk_size, 3);
                assert_matches(config, &small(false), &reference);
            }
        }
    }

    #[test]
    fn stealing_takes_the_back_chunk_and_the_owner_claims_the_front() {
        let range = AtomicU64::new(pack(0, 4));
        assert_eq!(claim_own(&range), Some(0));
        assert_eq!(steal_one(&range), Some(3));
        assert_eq!(unpack(range.load(Ordering::Relaxed)), (1, 3));
        assert_eq!(claim_own(&range), Some(1));
        assert_eq!(steal_one(&range), Some(2));
        assert_eq!(steal_one(&range), None);
        assert_eq!(claim_own(&range), None);
        let single = AtomicU64::new(pack(7, 8));
        assert_eq!(steal_one(&single), Some(7));
        assert_eq!(claim_own(&single), None);
    }

    /// Reproduces a hang of the empty graph, where every task is instant and
    /// the race windows are widest; a watchdog dumps the scheduler's state.
    #[test]
    fn the_empty_graph_never_hangs() {
        let parameters = Parameters::new(Size::Default, 2_000, false);
        for scheduler in [Scheduler::GraphCounter, Scheduler::GraphStealing] {
            for workers in [4, 8] {
                for waiting in [Waiting::SpinThenPark(Duration::from_micros(5)), Waiting::Park] {
                    for _ in 0..3 {
                        let config = Config::new(scheduler, waiting, CHUNK, workers).empty();
                        let graph = Graph::new(&parameters, config);
                        let stop = AtomicBool::new(false);
                        thread::scope(|scope| {
                            scope.spawn(|| {
                                let started = Instant::now();
                                while !stop.load(Ordering::SeqCst) {
                                    thread::sleep(Duration::from_millis(20));
                                    if started.elapsed() > Duration::from_secs(20) {
                                        eprintln!("HANG {config:?}\n{}", graph.describe());
                                        std::process::abort();
                                    }
                                }
                            });
                            let outcome = graph.run();
                            stop.store(true, Ordering::SeqCst);
                            assert_eq!(outcome.stats.tasks[NODE_HASH], 2_001, "{config:?}");
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn the_empty_graph_counts_its_tasks() {
        let parameters = Parameters::new(Size::Small, 50, false);
        let chunks = Size::Small.entity_count().div_ceil(CHUNK) as u64;
        for scheduler in [Scheduler::GraphCounter, Scheduler::GraphStealing] {
            let config = Config::new(scheduler, Waiting::SpinThenPark(DEFAULT_SPIN_BOUND), CHUNK, 3).empty();
            let outcome = run_once(&parameters, config);
            assert_eq!(outcome.tasks_per_tick, 3 + 2 * chunks);
            assert_eq!(outcome.stats.task_total(), 50 * (2 + 2 * chunks) + 51, "{scheduler:?}");
            assert_eq!(outcome.stats.tasks[NODE_HASH], 51);
            assert!(outcome.tick_hashes.is_empty());
        }
        let config = Config::new(Scheduler::BarrierInPlace, Waiting::Park, CHUNK, 3).empty();
        let outcome = run_once(&parameters, config);
        assert_eq!(outcome.stats.task_total(), 50 * 2 * chunks);
    }

    #[test]
    fn an_injected_panic_fails_the_tick_without_deadlocking_the_pool() {
        let parameters = Parameters::new(Size::Small, 20, false);
        let reference = run_sequential(&parameters);
        for scheduler in [Scheduler::GraphCounter, Scheduler::GraphStealing] {
            for (node, chunk) in [(NODE_REPULSION, 1), (NODE_HASH, 0), (NODE_STRUCTURAL, 0)] {
                let injection = Injection { tick: 3, node, chunk };
                let config = Config::new(scheduler, Waiting::Park, CHUNK, 3).injecting(injection);
                let outcome = run_once(&parameters, config);
                let failure = outcome.failure.expect("the failure is reported");
                assert_eq!((failure.tick, failure.node, failure.chunk), (3, node, chunk));
                assert!(failure.message.contains("injected panic"));
                assert!(failure.worker < 3);
                // The run ends at the first hash completing after the failure: the hash
                // of tick 2, when it was still running, or the hash of the failed tick.
                assert!((3..=4).contains(&outcome.tick_hashes.len()), "{} hashes", outcome.tick_hashes.len());
                assert_eq!(outcome.tick_hashes[..3], reference.tick_hashes[..3]);
            }
        }
    }
}
