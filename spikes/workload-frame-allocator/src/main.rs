//! Frame allocator of the reference workload for the memory research.
//!
//! Throwaway spike, never product code. It runs the reference workload
//! (default size, wander system off) on the dependency-counting tick graph the
//! multithreading research measured, with per-chunk temporaries that the
//! earlier spikes did not have (a force buffer in the repulsion pass, an
//! energy buffer and a wall-bounce event buffer in the motion pass, the grid's
//! cursor in the spatial hashing), under four allocation modes:
//!
//! - `pre-sized`: buffers sized at creation and reused across ticks, per
//!   worker for the temporaries and per chunk for the events;
//! - `scratch`: a bump allocator per worker, reset at the tick boundary by
//!   the worker that owns it, with a counted heap fallback when a block is full;
//! - `heap`: every temporary allocated from the global allocator at chunk
//!   start and freed at chunk end, the event buffers freed at the fixed point
//!   (the contended mode), with the counting wrapper switched off;
//! - `heap-counted`: the same with the counting wrapper on.
//!
//! The program's global allocator is a wrapper around the system allocator
//! that counts allocations, frees and bytes per thread when switched on; the
//! fixed point reads the counters and records the allocations of every tick.
//! A growth mode starts the workload with a capacity equal to its population,
//! spawns 80 or 800 entities per tick without kills, and grows every table at
//! the fixed point when the spawns exceed the capacity; it is compared with
//! the same run preallocated at the final size. Every allocation site belongs
//! to a tag whose counters are summed at destruction: the leak oracle; one run
//! leaks a buffer on purpose.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, UnsafeCell};
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use reference_workload::{
    CHUNK, DEFAULT_TICK_COUNT, Fnv1a, GRID_SIDE, Grid, INTERACTION_RADIUS, Json, KILLS_PER_TICK, Parameters,
    REPULSION_STRENGTH, Run, SPAWNS_PER_TICK, Size, State, Timing, WARM_UP_TICKS, bounced, chunk_range,
    first_divergent_tick, integrated, kinetic_energy, repelled_velocity, run_hash, run_sequential,
    wandered_velocity, write_platform_and_build, write_run_core,
};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-frame-allocator";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
const GROWTH_WORKER_COUNTS: [usize; 2] = [1, 4];
const SPIN_BOUND: Duration = Duration::from_micros(50);
const STEADY_STATE_FROM_TICK: u32 = WARM_UP_TICKS + 1;
/// The initial block of a worker's scratch: small on purpose, so that the
/// growth to the high-water mark through the counted fallback is visible.
const SCRATCH_INITIAL_BLOCK: usize = 64 * 1024;
const GROWTH_RATES: [(usize, u32); 2] = [(80, 1_000), (800, 100)];
const MICROBENCHMARK_ITERATIONS: usize = 1_000_000;
const MICROBENCHMARK_BYTES: usize = 1_024;
const MICROBENCHMARK_ROUNDS: usize = 3;

const NODE_SPATIAL: usize = 0;
const NODE_REPULSION: usize = 1;
const NODE_MOTION: usize = 2;
const NODE_STRUCTURAL: usize = 3;
const NODE_HASH: usize = 4;
const NODE_COUNT: usize = 5;
const NODE_NAMES: [&str; NODE_COUNT] = ["spatial_hashing", "repulsion", "motion", "structural_changes", "hash"];
const CLAIM_ORDER: [usize; NODE_COUNT] = [NODE_SPATIAL, NODE_HASH, NODE_REPULSION, NODE_MOTION, NODE_STRUCTURAL];

// ---------------------------------------------------------------------------
// The counting global allocator
// ---------------------------------------------------------------------------

const SLOTS: usize = 64;
/// The slot of a thread that registered none: counted, since a spawned
/// thread frees a few blocks in the standard library's start-up code before
/// it registers.
const DEFAULT_SLOT: usize = SLOTS - 2;
/// A slot the totals leave out, for a thread whose allocations must not
/// count (a test's thread once its test is over).
const IGNORED_SLOT: usize = SLOTS - 1;

/// One thread's counters, on its own cache line.
#[repr(align(64))]
struct Counter {
    allocations: AtomicU64,
    frees: AtomicU64,
    reallocations: AtomicU64,
    bytes_allocated: AtomicU64,
    bytes_freed: AtomicU64,
}

impl Counter {
    const fn new() -> Counter {
        Counter {
            allocations: AtomicU64::new(0),
            frees: AtomicU64::new(0),
            reallocations: AtomicU64::new(0),
            bytes_allocated: AtomicU64::new(0),
            bytes_freed: AtomicU64::new(0),
        }
    }
}

static COUNTERS: [Counter; SLOTS] = [const { Counter::new() }; SLOTS];
static COUNTING: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The counter slot of the current thread: the worker index once a worker
    /// registered, the default slot for every other thread.
    static SLOT: Cell<usize> = const { Cell::new(DEFAULT_SLOT) };
}

fn register_thread(slot: usize) {
    SLOT.set(slot.min(IGNORED_SLOT));
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct HeapTotals {
    allocations: u64,
    frees: u64,
    reallocations: u64,
    bytes_allocated: u64,
    bytes_freed: u64,
}

/// The totals of every thread but those that asked to be ignored.
fn heap_totals() -> HeapTotals {
    let mut totals = HeapTotals::default();
    for counter in &COUNTERS[..IGNORED_SLOT] {
        totals.allocations += counter.allocations.load(Ordering::Relaxed);
        totals.frees += counter.frees.load(Ordering::Relaxed);
        totals.reallocations += counter.reallocations.load(Ordering::Relaxed);
        totals.bytes_allocated += counter.bytes_allocated.load(Ordering::Relaxed);
        totals.bytes_freed += counter.bytes_freed.load(Ordering::Relaxed);
    }
    totals
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded as is to the system allocator.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
            let counter = &COUNTERS[SLOT.get()];
            counter.allocations.fetch_add(1, Ordering::Relaxed);
            counter.bytes_allocated.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded as is to the system allocator.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
            let counter = &COUNTERS[SLOT.get()];
            counter.allocations.fetch_add(1, Ordering::Relaxed);
            counter.bytes_allocated.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarded as is to the system allocator.
        unsafe { System.dealloc(pointer, layout) };
        if COUNTING.load(Ordering::Relaxed) {
            let counter = &COUNTERS[SLOT.get()];
            counter.frees.fetch_add(1, Ordering::Relaxed);
            counter.bytes_freed.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded as is to the system allocator.
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() && COUNTING.load(Ordering::Relaxed) {
            let counter = &COUNTERS[SLOT.get()];
            counter.reallocations.fetch_add(1, Ordering::Relaxed);
            counter.bytes_allocated.fetch_add(new_size as u64, Ordering::Relaxed);
            counter.bytes_freed.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        moved
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;


// ---------------------------------------------------------------------------
// Accounting by tag
// ---------------------------------------------------------------------------

const TAG_TABLES: usize = 0;
const TAG_SCRATCH: usize = 1;
const TAG_BUFFERS: usize = 2;
const TAG_EVENTS: usize = 3;
const TAG_TEMPORARIES: usize = 4;
const TAG_COUNT: usize = 5;
const TAG_NAMES: [&str; TAG_COUNT] = ["tables", "scratch", "buffers", "events", "temporaries"];

struct TagCounters {
    bytes: AtomicI64,
    allocations: AtomicU64,
}

/// Per-worker counters per tag, updated by the worker that allocates or
/// frees, summed when a report asks: no counter is contended in a pass.
struct Accounting {
    per_worker: Vec<[TagCounters; TAG_COUNT]>,
}

impl Accounting {
    fn new(workers: usize) -> Accounting {
        Accounting {
            per_worker: (0..workers)
                .map(|_| std::array::from_fn(|_| TagCounters { bytes: AtomicI64::new(0), allocations: AtomicU64::new(0) }))
                .collect(),
        }
    }

    fn allocated(&self, worker: usize, tag: usize, bytes: usize) {
        let counters = &self.per_worker[worker][tag];
        counters.bytes.fetch_add(bytes as i64, Ordering::Relaxed);
        counters.allocations.fetch_add(1, Ordering::Relaxed);
    }

    fn freed(&self, worker: usize, tag: usize, bytes: usize) {
        self.per_worker[worker][tag].bytes.fetch_sub(bytes as i64, Ordering::Relaxed);
    }

    fn bytes(&self, tag: usize) -> i64 {
        self.per_worker.iter().map(|tags| tags[tag].bytes.load(Ordering::Relaxed)).sum()
    }

    fn allocations(&self, tag: usize) -> u64 {
        self.per_worker.iter().map(|tags| tags[tag].allocations.load(Ordering::Relaxed)).sum()
    }
}

// ---------------------------------------------------------------------------
// The per-worker scratch
// ---------------------------------------------------------------------------

struct Block {
    pointer: *mut u8,
    size: usize,
}

/// A bump allocator owned by one worker: blocks allocated from the heap and
/// kept, an offset reset at the tick boundary, no individual free. When the
/// current block is full the next one is used, and when none is left a new
/// block twice the last is allocated from the heap: the counted fallback,
/// after which the capacity stays at the high-water mark.
struct Scratch {
    worker: usize,
    blocks: Vec<Block>,
    current: usize,
    offset: usize,
    used: usize,
    high_water: usize,
    fallbacks: u64,
    tick_stamp: u32,
}

impl Scratch {
    fn new(worker: usize, accounting: &Accounting) -> Scratch {
        let mut scratch = Scratch {
            worker,
            blocks: Vec::with_capacity(16),
            current: 0,
            offset: 0,
            used: 0,
            high_water: 0,
            fallbacks: 0,
            tick_stamp: 0,
        };
        scratch.add_block(SCRATCH_INITIAL_BLOCK, accounting);
        scratch
    }

    fn add_block(&mut self, size: usize, accounting: &Accounting) {
        let layout = Layout::from_size_align(size, 64).expect("a valid block layout");
        // SAFETY: a non-zero size; the block is freed in `release`.
        let pointer = unsafe { std::alloc::alloc(layout) };
        assert!(!pointer.is_null(), "the heap served a scratch block");
        accounting.allocated(self.worker, TAG_SCRATCH, size);
        self.blocks.push(Block { pointer, size });
    }

    fn reset(&mut self) {
        self.high_water = self.high_water.max(self.used);
        self.current = 0;
        self.offset = 0;
        self.used = 0;
    }

    fn capacity(&self) -> usize {
        self.blocks.iter().map(|block| block.size).sum()
    }

    /// A buffer of `count` values, valid until the next reset.
    fn alloc<T: Copy>(&mut self, count: usize, accounting: &Accounting) -> *mut T {
        let bytes = count * size_of::<T>();
        let align = align_of::<T>();
        loop {
            let block = &self.blocks[self.current];
            let start = (self.offset + align - 1) & !(align - 1);
            if start + bytes <= block.size {
                self.offset = start + bytes;
                self.used += bytes;
                // SAFETY: in bounds of the block by the check above.
                return unsafe { block.pointer.add(start) }.cast::<T>();
            }
            if self.current + 1 < self.blocks.len() {
                self.current += 1;
                self.offset = 0;
                continue;
            }
            let size = (block.size * 2).max(bytes + align);
            self.add_block(size, accounting);
            self.fallbacks += 1;
            self.current = self.blocks.len() - 1;
            self.offset = 0;
        }
    }

    fn release(&mut self, accounting: &Accounting) {
        for block in self.blocks.drain(..) {
            let layout = Layout::from_size_align(block.size, 64).expect("a valid block layout");
            // SAFETY: allocated by `add_block` with this layout, freed once.
            unsafe { std::alloc::dealloc(block.pointer, layout) };
            accounting.freed(self.worker, TAG_SCRATCH, block.size);
        }
    }
}

// ---------------------------------------------------------------------------
// Modes and configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    PreSized,
    Scratch,
    Heap,
    HeapCounted,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::PreSized, Mode::Scratch, Mode::Heap, Mode::HeapCounted];

    fn name(self) -> &'static str {
        match self {
            Mode::PreSized => "pre-sized",
            Mode::Scratch => "scratch",
            Mode::Heap => "heap",
            Mode::HeapCounted => "heap-counted",
        }
    }

    fn counting(self) -> bool {
        self != Mode::Heap
    }

    fn uses_heap(self) -> bool {
        matches!(self, Mode::Heap | Mode::HeapCounted)
    }
}

/// The growth mode: `rate` spawns per tick and no kills, the tables grown at
/// the fixed point (`grow`) or preallocated at the final population.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Growth {
    rate: usize,
    grow: bool,
}

#[derive(Clone, Copy, Debug)]
struct Config {
    mode: Mode,
    workers: usize,
    growth: Option<Growth>,
    leak_one_buffer: bool,
}

impl Config {
    fn new(mode: Mode, workers: usize) -> Config {
        assert!(workers >= 1 && workers < DEFAULT_SLOT);
        Config { mode, workers, growth: None, leak_one_buffer: false }
    }

    fn growing(self, growth: Growth) -> Config {
        Config { growth: Some(growth), ..self }
    }

    fn leaking(self) -> Config {
        Config { leak_one_buffer: true, ..self }
    }
}

// ---------------------------------------------------------------------------
// Columns, the local grid and the kernels
// ---------------------------------------------------------------------------

/// A raw view of one `f32` column, refreshed at the fixed point whenever the
/// column's vector is reallocated. Chunks write disjoint slices of it: the
/// one `unsafe` construct the earlier spikes also had.
#[derive(Clone, Copy)]
struct Column {
    pointer: *mut f32,
    len: usize,
}

unsafe impl Send for Column {}
unsafe impl Sync for Column {}

impl Column {
    fn of(values: &mut [f32]) -> Column {
        Column { pointer: values.as_mut_ptr(), len: values.len() }
    }

    /// # Safety
    ///
    /// No other thread reads or writes the slots of `range` in this column
    /// while the returned slice lives.
    unsafe fn slice(&self, range: Range<usize>) -> &mut [f32] {
        assert!(range.start <= range.end && range.end <= self.len, "chunk slice out of bounds");
        // SAFETY: in bounds by the assertion; disjointness is the caller's.
        unsafe { std::slice::from_raw_parts_mut(self.pointer.add(range.start), range.len()) }
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
        unsafe { (self.x.slice(range.clone()), self.y.slice(range.clone()), self.vx.slice(range.clone()), self.vy.slice(range)) }
    }
}

/// The spatial hashing of the specification in pre-sized storage: the same
/// counting sort as the reference implementation's, with the cursor array
/// handed in by the caller instead of cloned per tick.
struct LocalGrid {
    cell_start: Vec<u32>,
    slots: Vec<u32>,
}

impl LocalGrid {
    const CELLS: usize = GRID_SIDE * GRID_SIDE;

    fn new(capacity: usize) -> LocalGrid {
        LocalGrid { cell_start: vec![0; LocalGrid::CELLS + 1], slots: vec![0; capacity] }
    }

    fn bytes(&self) -> usize {
        (self.cell_start.capacity() + self.slots.capacity()) * size_of::<u32>()
    }

    fn build(&mut self, state: &State, cursor: &mut [u32]) {
        let cells = LocalGrid::CELLS;
        self.cell_start.fill(0);
        for slot in 0..state.len() {
            if state.alive[slot] {
                let (cx, cy) = Grid::cell_of(state.x[slot], state.y[slot]);
                self.cell_start[cy * GRID_SIDE + cx + 1] += 1;
            }
        }
        for cell in 0..cells {
            self.cell_start[cell + 1] += self.cell_start[cell];
        }
        cursor[..cells].copy_from_slice(&self.cell_start[..cells]);
        for slot in 0..state.len() {
            if state.alive[slot] {
                let (cx, cy) = Grid::cell_of(state.x[slot], state.y[slot]);
                let cell = cy * GRID_SIDE + cx;
                self.slots[cursor[cell] as usize] = slot as u32;
                cursor[cell] += 1;
            }
        }
    }

    fn slots_in(&self, cx: usize, cy: usize) -> &[u32] {
        let cell = cy * GRID_SIDE + cx;
        &self.slots[self.cell_start[cell] as usize..self.cell_start[cell + 1] as usize]
    }
}

/// System 2 for one slot, the specification's visit order, on the local grid.
fn repulsion_force(state: &State, grid: &LocalGrid, i: usize) -> (f32, f32) {
    let (xi, yi) = (state.x[i], state.y[i]);
    let (cx, cy) = Grid::cell_of(xi, yi);
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
                if d2 < INTERACTION_RADIUS * INTERACTION_RADIUS && d2 > 0.0 {
                    let d = d2.sqrt();
                    let s = (REPULSION_STRENGTH * (INTERACTION_RADIUS - d)) / d;
                    fx -= dx * s;
                    fy -= dy * s;
                }
            }
        }
    }
    (fx, fy)
}

/// Systems 3 to 6 for one slot, in the specified order, plus whether each
/// axis bounced on a wall (the event the motion pass records).
#[inline]
fn motion_slot(x: f32, y: f32, mut vx: f32, mut vy: f32, mass: f32, tick: u32, i: usize, wander: bool) -> (f32, f32, f32, f32, f32, bool, bool) {
    if wander {
        (vx, vy) = wandered_velocity(vx, vy, tick, i);
    }
    let ix = integrated(x, vx);
    let iy = integrated(y, vy);
    let (x, vx) = bounced(ix, vx);
    let (y, vy) = bounced(iy, vy);
    (x, y, vx, vy, kinetic_energy(vx, vy, mass), x != ix, y != iy)
}

// ---------------------------------------------------------------------------
// The graph
// ---------------------------------------------------------------------------

/// Where a chunk left the events of its slots, read at the fixed point in
/// chunk order. `buffer` holds the chunk's own vector in the pre-sized mode
/// and the tick's vector in the heap modes; in the scratch mode the pointer
/// points into the scratch of the worker that ran the chunk.
struct EventSlot {
    buffer: Option<Vec<u32>>,
    pointer: *const u32,
    len: usize,
}

impl EventSlot {
    fn empty() -> EventSlot {
        EventSlot { buffer: None, pointer: std::ptr::null(), len: 0 }
    }

    fn pre_sized() -> EventSlot {
        EventSlot { buffer: Some(Vec::with_capacity(2 * CHUNK)), pointer: std::ptr::null(), len: 0 }
    }
}

/// The reused temporaries of one worker in the pre-sized mode.
struct WorkerBuffers {
    forces: Vec<f32>,
    energies: Vec<f32>,
}

impl WorkerBuffers {
    fn new() -> WorkerBuffers {
        WorkerBuffers { forces: vec![0.0; 2 * CHUNK], energies: vec![0.0; CHUNK] }
    }

    fn bytes(&self) -> usize {
        (self.forces.capacity() + self.energies.capacity()) * size_of::<f32>()
    }
}

#[derive(Clone, Copy, Debug)]
struct GrowthEvent {
    tick: u32,
    from: usize,
    to: usize,
    nanos: u64,
}

#[derive(Clone, Debug)]
struct Failure {
    tick: u32,
    node: usize,
    chunk: usize,
    worker: usize,
    message: String,
}

#[derive(Clone, Copy, Default, Debug)]
struct WorkerStats {
    busy_ns: [u64; NODE_COUNT],
    tasks: [u64; NODE_COUNT],
    parks: u64,
}

impl WorkerStats {
    fn add(&mut self, other: &WorkerStats) {
        for node in 0..NODE_COUNT {
            self.busy_ns[node] += other.busy_ns[node];
            self.tasks[node] += other.tasks[node];
        }
        self.parks += other.parks;
    }
}

struct GraphNode {
    chunks: AtomicUsize,
    next: AtomicUsize,
    finished: AtomicUsize,
    pending: AtomicUsize,
}

impl GraphNode {
    fn new(chunks: usize) -> GraphNode {
        GraphNode {
            chunks: AtomicUsize::new(chunks),
            next: AtomicUsize::new(chunks),
            finished: AtomicUsize::new(0),
            pending: AtomicUsize::new(0),
        }
    }

    fn chunks(&self) -> usize {
        self.chunks.load(Ordering::Relaxed)
    }
}

/// The tables of a world that grow together: both state buffers, the energy
/// groups, the grid's slot list and the per-chunk event slots. `capacity` is
/// the number of slots reserved; the vectors never reallocate below it.
struct Tables {
    states: [State; 2],
    columns: [Columns; 2],
    capacity: usize,
    groups: Vec<f32>,
    groups_column: Column,
    grid: LocalGrid,
    event_slots: Vec<UnsafeCell<EventSlot>>,
    bytes: usize,
}

impl Tables {
    fn new(mut state: State, capacity: usize, mode: Mode) -> Tables {
        let capacity = capacity.max(state.len()).div_ceil(CHUNK) * CHUNK;
        Tables::reserve(&mut state, capacity);
        let mut other = state.clone();
        Tables::reserve(&mut other, capacity);
        let columns = [Columns::of(&mut state), Columns::of(&mut other)];
        let mut groups = vec![0.0_f32; capacity / CHUNK];
        let groups_column = Column::of(&mut groups);
        let event_slots = (0..capacity / CHUNK)
            .map(|_| UnsafeCell::new(if mode == Mode::PreSized { EventSlot::pre_sized() } else { EventSlot::empty() }))
            .collect();
        let mut tables = Tables {
            states: [state, other],
            columns,
            capacity,
            groups,
            groups_column,
            grid: LocalGrid::new(capacity),
            event_slots,
            bytes: 0,
        };
        tables.bytes = tables.measure_bytes();
        tables
    }

    fn reserve(state: &mut State, capacity: usize) {
        let more = capacity - state.len();
        state.generation.reserve_exact(more);
        state.alive.reserve_exact(more);
        state.x.reserve_exact(more);
        state.y.reserve_exact(more);
        state.vx.reserve_exact(more);
        state.vy.reserve_exact(more);
        state.mass.reserve_exact(more);
    }

    fn state_bytes(state: &State) -> usize {
        state.generation.capacity() * 4
            + state.alive.capacity()
            + (state.x.capacity() + state.y.capacity() + state.vx.capacity() + state.vy.capacity() + state.mass.capacity()) * 4
    }

    /// The bytes of the tables proper (the pre-sized event buffers are
    /// accounted as buffers).
    fn measure_bytes(&self) -> usize {
        Tables::state_bytes(&self.states[0])
            + Tables::state_bytes(&self.states[1])
            + self.groups.capacity() * 4
            + self.grid.bytes()
            + self.event_slots.capacity() * size_of::<UnsafeCell<EventSlot>>()
    }

    /// Grows every table to hold `needed` slots: 1.5 times the capacity at
    /// least, rounded up to whole chunks. Runs at the fixed point only.
    fn grow(&mut self, needed: usize, mode: Mode) -> usize {
        let target = needed.max(self.capacity * 3 / 2).div_ceil(CHUNK) * CHUNK;
        for state in &mut self.states {
            Tables::reserve(state, target);
        }
        self.columns = [Columns::of(&mut self.states[0]), Columns::of(&mut self.states[1])];
        self.groups.resize(target / CHUNK, 0.0);
        self.groups_column = Column::of(&mut self.groups);
        self.grid.slots.resize(target, 0);
        while self.event_slots.len() < target / CHUNK {
            self.event_slots.push(UnsafeCell::new(if mode == Mode::PreSized { EventSlot::pre_sized() } else { EventSlot::empty() }));
        }
        self.capacity = target;
        target
    }

    fn pre_sized_event_bytes(&self) -> usize {
        self.event_slots
            .iter()
            .map(|slot| {
                // SAFETY: called at creation and at destruction, when no task runs.
                unsafe { &*slot.get() }.buffer.as_ref().map_or(0, |buffer| buffer.capacity() * 4)
            })
            .sum()
    }
}

/// The tick graph over the tables, the per-worker scratches and buffers.
/// The structural node runs alone and is the fixed point where the energy is
/// reduced, the events are merged, the structural changes (or the growth
/// mode's spawns and table growth) are applied, the heap counters are read
/// and the buffers swap roles.
struct Graph {
    config: Config,
    parameters: Parameters,
    tables: UnsafeCell<Tables>,
    scratches: Vec<UnsafeCell<Scratch>>,
    buffers: Vec<UnsafeCell<WorkerBuffers>>,
    cursor: UnsafeCell<Vec<u32>>,
    dead: UnsafeCell<Vec<usize>>,
    changed: UnsafeCell<Vec<usize>>,
    current: AtomicUsize,
    nodes: [GraphNode; NODE_COUNT],
    ready: AtomicU32,
    tick: AtomicU32,
    hash_tick: AtomicU32,
    done: AtomicBool,
    failed: AtomicBool,
    failure: Mutex<Option<Failure>>,
    tick_hashes: Vec<AtomicU64>,
    event_hashes: Vec<AtomicU64>,
    event_counts: Vec<AtomicU64>,
    allocations_per_tick: Vec<AtomicU64>,
    bytes_per_tick: Vec<AtomicU64>,
    last_heap: UnsafeCell<HeapTotals>,
    growth_events: UnsafeCell<Vec<GrowthEvent>>,
    accounting: Accounting,
    parked: Vec<AtomicBool>,
    threads: OnceLock<Vec<Thread>>,
    started: OnceLock<Instant>,
    finished_ns: AtomicU64,
}

// SAFETY: the accesses to the cells follow the graph's dependencies, which
// the atomics order: a buffer is written during a tick only through disjoint
// chunk slices of the passes, each event slot by the chunk that owns it, each
// scratch and buffer set by the worker that owns it, and the structural node
// runs alone.
unsafe impl Sync for Graph {}

impl Graph {
    fn new(parameters: &Parameters, config: Config, tick_count: u32) -> Graph {
        let state = State::initialize(parameters);
        let n = state.len();
        let capacity = match config.growth {
            Some(growth) if !growth.grow => n + growth.rate * tick_count as usize,
            _ => n,
        };
        let tables = Tables::new(state, capacity, config.mode);
        let chunk_count = n.div_ceil(CHUNK);
        let w = config.workers;
        let accounting = Accounting::new(w);
        accounting.allocated(0, TAG_TABLES, tables.bytes);
        if config.mode == Mode::PreSized {
            accounting.allocated(0, TAG_BUFFERS, tables.pre_sized_event_bytes());
        }
        let scratches: Vec<UnsafeCell<Scratch>> = (0..w).map(|worker| UnsafeCell::new(Scratch::new(worker, &accounting))).collect();
        let buffers: Vec<UnsafeCell<WorkerBuffers>> = (0..w).map(|_| UnsafeCell::new(WorkerBuffers::new())).collect();
        for buffer in &buffers {
            // SAFETY: construction, no task runs.
            accounting.allocated(0, TAG_BUFFERS, unsafe { &*buffer.get() }.bytes());
        }
        let cursor = vec![0_u32; LocalGrid::CELLS];
        accounting.allocated(0, TAG_BUFFERS, cursor.capacity() * 4);
        let per_tick = || (0..=tick_count).map(|_| AtomicU64::new(0)).collect::<Vec<_>>();
        Graph {
            config,
            parameters: *parameters,
            tables: UnsafeCell::new(tables),
            scratches,
            buffers,
            cursor: UnsafeCell::new(cursor),
            dead: UnsafeCell::new(Vec::with_capacity(64)),
            changed: UnsafeCell::new(Vec::with_capacity(64 + config.growth.map_or(0, |g| g.rate))),
            current: AtomicUsize::new(0),
            nodes: [GraphNode::new(1), GraphNode::new(chunk_count), GraphNode::new(chunk_count), GraphNode::new(1), GraphNode::new(1)],
            ready: AtomicU32::new(0),
            tick: AtomicU32::new(0),
            hash_tick: AtomicU32::new(0),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            failure: Mutex::new(None),
            tick_hashes: per_tick(),
            event_hashes: per_tick(),
            event_counts: per_tick(),
            allocations_per_tick: per_tick(),
            bytes_per_tick: per_tick(),
            last_heap: UnsafeCell::new(HeapTotals::default()),
            growth_events: UnsafeCell::new(Vec::with_capacity(64)),
            accounting,
            parked: (0..w).map(|_| AtomicBool::new(false)).collect(),
            threads: OnceLock::new(),
            started: OnceLock::new(),
            finished_ns: AtomicU64::new(0),
        }
    }

    fn tables(&self) -> &Tables {
        // SAFETY: the tables' vectors are mutated at the fixed point only, where no other task runs.
        unsafe { &*self.tables.get() }
    }

    fn current_state(&self) -> &State {
        &self.tables().states[self.current.load(Ordering::Relaxed)]
    }

    fn scratch(&self, worker: usize) -> &mut Scratch {
        // SAFETY: a scratch is touched by the thread of the worker that owns it only.
        unsafe { &mut *self.scratches[worker].get() }
    }

    fn worker_buffers(&self, worker: usize) -> &mut WorkerBuffers {
        // SAFETY: as for the scratch.
        unsafe { &mut *self.buffers[worker].get() }
    }

    /// The worker resets its scratch at its first task of a tick: the events
    /// of the previous tick were consumed by that tick's fixed point, which
    /// completed before any task of this tick was armed.
    fn touch_scratch(&self, worker: usize) {
        let tick = self.tick.load(Ordering::Relaxed);
        let scratch = self.scratch(worker);
        if scratch.tick_stamp != tick {
            scratch.reset();
            scratch.tick_stamp = tick;
        }
    }

    // --- the kernels ------------------------------------------------------

    fn spatial(&self, worker: usize) {
        let state = self.current_state();
        // SAFETY: the spatial node is the only user of the grid until it completes.
        let tables = unsafe { &mut *self.tables.get() };
        let mut heap_cursor: Vec<u32> = Vec::new();
        let cursor: &mut [u32] = match self.config.mode {
            // SAFETY: the cursor is used by the spatial node only, which runs one at a time.
            Mode::PreSized => unsafe { &mut *self.cursor.get() },
            Mode::Scratch => {
                let pointer = self.scratch(worker).alloc::<u32>(LocalGrid::CELLS, &self.accounting);
                // SAFETY: freshly bump-allocated for this task.
                unsafe { std::slice::from_raw_parts_mut(pointer, LocalGrid::CELLS) }
            }
            Mode::Heap | Mode::HeapCounted => {
                heap_cursor = vec![0; LocalGrid::CELLS];
                self.accounting.allocated(worker, TAG_TEMPORARIES, heap_cursor.capacity() * 4);
                &mut heap_cursor[..]
            }
        };
        tables.grid.build(state, cursor);
        if self.config.mode.uses_heap() {
            self.accounting.freed(worker, TAG_TEMPORARIES, heap_cursor.capacity() * 4);
        }
    }

    fn repulsion_chunk(&self, chunk: usize, worker: usize) {
        let tables = self.tables();
        let cur = self.current_state();
        let range = chunk_range(chunk, cur.len());
        let len = range.len();
        let next = 1 - self.current.load(Ordering::Relaxed);
        let mut heap_forces: Vec<f32> = Vec::new();
        let forces: &mut [f32] = match self.config.mode {
            Mode::PreSized => &mut self.worker_buffers(worker).forces[..2 * len],
            Mode::Scratch => {
                let pointer = self.scratch(worker).alloc::<f32>(2 * len, &self.accounting);
                // SAFETY: freshly bump-allocated for this task.
                unsafe { std::slice::from_raw_parts_mut(pointer, 2 * len) }
            }
            Mode::Heap | Mode::HeapCounted => {
                heap_forces = vec![0.0; 2 * len];
                self.accounting.allocated(worker, TAG_TEMPORARIES, heap_forces.capacity() * 4);
                &mut heap_forces[..]
            }
        };
        for (k, i) in range.clone().enumerate() {
            if cur.alive[i] {
                let (fx, fy) = repulsion_force(cur, &tables.grid, i);
                forces[2 * k] = fx;
                forces[2 * k + 1] = fy;
            }
        }
        // SAFETY: this chunk's slots of the next buffer's velocity columns are written by this task only.
        let (_, _, vx, vy) = unsafe { tables.columns[next].slices(range.clone()) };
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                vx[k] = repelled_velocity(cur.vx[i], forces[2 * k], cur.mass[i]);
                vy[k] = repelled_velocity(cur.vy[i], forces[2 * k + 1], cur.mass[i]);
            }
        }
        if self.config.mode.uses_heap() {
            self.accounting.freed(worker, TAG_TEMPORARIES, heap_forces.capacity() * 4);
        }
    }

    fn motion_chunk(&self, chunk: usize, worker: usize) {
        let tables = self.tables();
        let cur = self.current_state();
        let tick = self.tick.load(Ordering::Relaxed);
        let wander = self.parameters.wander;
        let range = chunk_range(chunk, cur.len());
        let len = range.len();
        let next = 1 - self.current.load(Ordering::Relaxed);
        let mut heap_energies: Vec<f32> = Vec::new();
        let mut heap_events: Vec<u32> = Vec::new();
        let (energies, events): (&mut [f32], &mut [u32]) = match self.config.mode {
            Mode::PreSized => {
                let buffers = self.worker_buffers(worker);
                // SAFETY: the chunk's own event buffer, written by this task only.
                let slot = unsafe { &mut *tables.event_slots[chunk].get() };
                let buffer = slot.buffer.as_mut().expect("a pre-sized event buffer per chunk");
                buffer.clear();
                buffer.resize(2 * len, 0);
                (&mut buffers.energies[..len], &mut buffer[..])
            }
            Mode::Scratch => {
                let scratch = self.scratch(worker);
                let energies = scratch.alloc::<f32>(len, &self.accounting);
                let events = scratch.alloc::<u32>(2 * len, &self.accounting);
                // SAFETY: freshly bump-allocated for this task; the events live until the fixed point.
                unsafe { (std::slice::from_raw_parts_mut(energies, len), std::slice::from_raw_parts_mut(events, 2 * len)) }
            }
            Mode::Heap | Mode::HeapCounted => {
                heap_energies = vec![0.0; len];
                heap_events = vec![0; 2 * len];
                self.accounting.allocated(worker, TAG_TEMPORARIES, heap_energies.capacity() * 4);
                self.accounting.allocated(worker, TAG_EVENTS, heap_events.capacity() * 4);
                (&mut heap_energies[..], &mut heap_events[..])
            }
        };
        // SAFETY: this chunk's slots of the next buffer are read and written by this task only.
        let (x, y, vx, vy) = unsafe { tables.columns[next].slices(range.clone()) };
        let mut event_count = 0;
        for (k, i) in range.clone().enumerate() {
            if cur.alive[i] {
                let (nx, ny, nvx, nvy, e, bx, by) = motion_slot(cur.x[i], cur.y[i], vx[k], vy[k], cur.mass[i], tick, i, wander);
                x[k] = nx;
                y[k] = ny;
                vx[k] = nvx;
                vy[k] = nvy;
                energies[k] = e;
                if bx {
                    events[event_count] = (i as u32) << 1;
                    event_count += 1;
                }
                if by {
                    events[event_count] = ((i as u32) << 1) | 1;
                    event_count += 1;
                }
            }
        }
        // The group sum in index order, as the specification adds it.
        let mut sum = 0.0_f32;
        for (k, i) in range.enumerate() {
            if cur.alive[i] {
                sum += energies[k];
            }
        }
        // SAFETY: the chunk owns its energy group.
        unsafe { tables.groups_column.slice(chunk..chunk + 1)[0] = sum };
        // SAFETY: the chunk's own event slot, written by this task only; read at the fixed point.
        let slot = unsafe { &mut *tables.event_slots[chunk].get() };
        slot.pointer = events.as_ptr();
        slot.len = event_count;
        if self.config.mode.uses_heap() {
            self.accounting.freed(worker, TAG_TEMPORARIES, heap_energies.capacity() * 4);
            slot.buffer = Some(heap_events);
        }
    }

    /// The fixed point.
    fn structural(&self, worker: usize) {
        let tick = self.tick.load(Ordering::Relaxed) as usize;
        let cur_index = self.current.load(Ordering::Relaxed);
        let next_index = 1 - cur_index;
        // SAFETY: the structural node runs alone (see the type's documentation).
        let tables = unsafe { &mut *self.tables.get() };
        let changed = unsafe { &mut *self.changed.get() };
        let dead = unsafe { &mut *self.dead.get() };
        let n = tables.states[cur_index].len();
        let chunk_count = n.div_ceil(CHUNK);

        // The events of the tick, merged in chunk order, hashed; the heap mode's buffers freed here.
        let mut hasher = Fnv1a::new();
        let mut count = 0_u64;
        for chunk in 0..chunk_count {
            // SAFETY: the slot's task completed before this node was armed.
            let slot = unsafe { &mut *tables.event_slots[chunk].get() };
            // SAFETY: `len` events were written at `pointer` by the chunk's task and are still live.
            let events = unsafe { std::slice::from_raw_parts(slot.pointer, slot.len) };
            for event in events {
                hasher.write(&event.to_le_bytes());
            }
            count += slot.len as u64;
            slot.len = 0;
            if self.config.mode.uses_heap()
                && let Some(buffer) = slot.buffer.take()
            {
                self.accounting.freed(worker, TAG_EVENTS, buffer.capacity() * 4);
            }
        }
        self.event_hashes[tick].store(hasher.finish(), Ordering::Relaxed);
        self.event_counts[tick].store(count, Ordering::Relaxed);

        let (cur, next) = if cur_index == 0 {
            let (a, b) = tables.states.split_at_mut(1);
            (&a[0], &mut b[0])
        } else {
            let (a, b) = tables.states.split_at_mut(1);
            (&b[0], &mut a[0])
        };
        for &slot in changed.iter() {
            next.alive[slot] = cur.alive[slot];
            next.generation[slot] = cur.generation[slot];
            next.mass[slot] = cur.mass[slot];
        }
        next.rng = cur.rng;
        // SAFETY: no motion chunk runs; the groups of this tick's chunks are final.
        let groups = unsafe { tables.groups_column.slice(0..chunk_count) };
        let mut total = 0.0_f32;
        for sum in groups.iter() {
            total += *sum;
        }
        next.energy = total;
        changed.clear();
        match self.config.growth {
            None => {
                let mut peek = next.rng;
                for _ in 0..KILLS_PER_TICK {
                    changed.push((peek.next() % n as u64) as usize);
                }
                next.structural_changes();
                dead.extend_from_slice(changed);
                dead.sort_unstable();
                dead.dedup();
                let spawned = SPAWNS_PER_TICK.min(dead.len());
                changed.extend(dead.drain(..spawned));
            }
            Some(growth) => {
                let needed = n + growth.rate;
                if needed > tables.capacity {
                    assert!(growth.grow, "the preallocated variant never grows");
                    let growing = Instant::now();
                    let from = tables.capacity;
                    let old_bytes = tables.bytes;
                    let to = tables.grow(needed, self.config.mode);
                    tables.bytes = tables.measure_bytes();
                    self.accounting.allocated(worker, TAG_TABLES, tables.bytes.saturating_sub(old_bytes));
                    if self.config.mode == Mode::PreSized {
                        let added = (to - from) / CHUNK * 2 * CHUNK * 4;
                        self.accounting.allocated(worker, TAG_BUFFERS, added);
                    }
                    // SAFETY: the fixed point runs alone.
                    unsafe { &mut *self.growth_events.get() }.push(GrowthEvent {
                        tick: tick as u32,
                        from,
                        to,
                        nanos: growing.elapsed().as_nanos() as u64,
                    });
                }
                // The tables were possibly reallocated: take the state references again.
                let (cur, next) = if cur_index == 0 {
                    let (a, b) = tables.states.split_at_mut(1);
                    (&mut a[0], &mut b[0])
                } else {
                    let (a, b) = tables.states.split_at_mut(1);
                    (&mut b[0], &mut a[0])
                };
                for _ in 0..growth.rate {
                    let slot = next.len();
                    let x = next.rng.u01() * reference_workload::WORLD_SIDE;
                    let y = next.rng.u01() * reference_workload::WORLD_SIDE;
                    let vx = ((next.rng.u01() * 2.0) - 1.0) * 32.0;
                    let vy = ((next.rng.u01() * 2.0) - 1.0) * 32.0;
                    let mass = next.rng.mass();
                    next.generation.push(0);
                    next.alive.push(true);
                    next.x.push(x);
                    next.y.push(y);
                    next.vx.push(vx);
                    next.vy.push(vy);
                    next.mass.push(mass);
                    cur.generation.push(0);
                    cur.alive.push(false);
                    cur.x.push(0.0);
                    cur.y.push(0.0);
                    cur.vx.push(0.0);
                    cur.vy.push(0.0);
                    cur.mass.push(0.0);
                    changed.push(slot);
                }
                let new_n = next.len();
                assert!(new_n <= tables.capacity);
                tables.columns = [Columns::of(&mut tables.states[0]), Columns::of(&mut tables.states[1])];
                let new_chunks = new_n.div_ceil(CHUNK);
                self.nodes[NODE_REPULSION].chunks.store(new_chunks, Ordering::Relaxed);
                self.nodes[NODE_MOTION].chunks.store(new_chunks, Ordering::Relaxed);
            }
        }
        // The heap counters since the previous fixed point: this tick's allocations.
        let totals = heap_totals();
        // SAFETY: the fixed point runs alone.
        let last = unsafe { &mut *self.last_heap.get() };
        self.allocations_per_tick[tick].store(totals.allocations + totals.reallocations - last.allocations - last.reallocations, Ordering::Relaxed);
        self.bytes_per_tick[tick].store(totals.bytes_allocated - last.bytes_allocated, Ordering::Relaxed);
        *last = totals;
        self.current.store(next_index, Ordering::Relaxed);
    }

    fn hash(&self) {
        let tick = self.hash_tick.load(Ordering::Relaxed) as usize;
        self.tick_hashes[tick].store(self.current_state().hash(), Ordering::Relaxed);
    }

    fn execute(&self, node: usize, chunk: usize, worker: usize) {
        match node {
            NODE_SPATIAL => self.spatial(worker),
            NODE_REPULSION => self.repulsion_chunk(chunk, worker),
            NODE_MOTION => self.motion_chunk(chunk, worker),
            NODE_STRUCTURAL => self.structural(worker),
            NODE_HASH => self.hash(),
            _ => unreachable!("five nodes"),
        }
    }

    fn run_task(&self, node: usize, chunk: usize, worker: usize) {
        self.touch_scratch(worker);
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.execute(node, chunk, worker))) {
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

    // --- the scheduler (the counter graph of the scheduling overhead spike) ---

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

    fn finish_run(&self) {
        let started = self.started.get().expect("the run started");
        self.finished_ns.store(started.elapsed().as_nanos() as u64, Ordering::Release);
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
                let last = self.hash_tick.load(Ordering::Relaxed) == self.parameters.tick_count;
                if last || self.failed.load(Ordering::SeqCst) {
                    self.finish_run();
                } else {
                    self.release_structural();
                }
            }
            NODE_STRUCTURAL => {
                let tick = self.tick.load(Ordering::Relaxed);
                self.hash_tick.store(tick, Ordering::Relaxed);
                if tick < self.parameters.tick_count && !self.failed.load(Ordering::SeqCst) {
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
        mask != 0 && (0..NODE_COUNT).any(|node| mask & (1 << node) != 0 && self.nodes[node].next.load(Ordering::Relaxed) < self.nodes[node].chunks())
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
            let chunks = n.chunks();
            if n.next.load(Ordering::Relaxed) < chunks {
                let chunk = n.next.fetch_add(1, Ordering::AcqRel);
                if chunk < chunks {
                    return Some((node, chunk));
                }
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

    fn wait(&self, worker: usize, stats: &mut WorkerStats) {
        let started = Instant::now();
        let mut spins = 0_u32;
        loop {
            if self.has_work() || self.done.load(Ordering::SeqCst) {
                break;
            }
            std::hint::spin_loop();
            spins += 1;
            if spins % 32 == 0 && started.elapsed() >= SPIN_BOUND {
                self.park(worker, stats);
                break;
            }
        }
    }

    fn worker_loop(&self, worker: usize) -> WorkerStats {
        register_thread(worker);
        let mut stats = WorkerStats::default();
        loop {
            match self.try_claim() {
                Some((node, chunk)) => {
                    let started = Instant::now();
                    self.run_task(node, chunk, worker);
                    stats.busy_ns[node] += started.elapsed().as_nanos() as u64;
                    stats.tasks[node] += 1;
                    let n = &self.nodes[node];
                    if n.finished.fetch_add(1, Ordering::AcqRel) + 1 == n.chunks() {
                        self.complete(node);
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

    /// Runs the ticks and fills `outcome`, whose vectors the caller allocated
    /// before the graph existed (so that the heap counters can tell what the
    /// world itself left behind).
    fn run(&self, outcome: &mut Outcome) {
        COUNTING.store(self.config.mode.counting(), Ordering::SeqCst);
        // SAFETY: nothing runs yet.
        unsafe { *self.last_heap.get() = heap_totals() };
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
            self.tick.store(1, Ordering::Relaxed);
            self.hash_tick.store(0, Ordering::Relaxed);
            self.nodes[NODE_STRUCTURAL].pending.store(2, Ordering::Relaxed);
            self.arm(NODE_SPATIAL);
            self.arm(NODE_HASH);
            self.wake(1);
            stats = self.worker_loop(0);
            for handle in handles {
                stats.add(&handle.join().expect("a worker never panics outside a task"));
            }
        });
        COUNTING.store(true, Ordering::SeqCst);
        outcome.total = Duration::from_nanos(self.finished_ns.load(Ordering::Acquire));
        let last = self.hash_tick.load(Ordering::Relaxed) as usize;
        outcome.tick_hashes.clear();
        outcome.tick_hashes.extend((0..=last).map(|t| self.tick_hashes[t].load(Ordering::Relaxed)));
        outcome.event_hashes.clear();
        outcome.event_hashes.extend((1..=last).map(|t| self.event_hashes[t].load(Ordering::Relaxed)));
        outcome.event_count = (1..=last).map(|t| self.event_counts[t].load(Ordering::Relaxed)).sum();
        outcome.allocations_per_tick.clear();
        outcome.allocations_per_tick.extend((1..=last).map(|t| self.allocations_per_tick[t].load(Ordering::Relaxed)));
        outcome.bytes_per_tick.clear();
        outcome.bytes_per_tick.extend((1..=last).map(|t| self.bytes_per_tick[t].load(Ordering::Relaxed)));
        outcome.stats = stats;
        outcome.failure = self.failure.lock().unwrap().clone();
        for worker in 0..self.config.workers {
            let scratch = self.scratch(worker);
            scratch.reset();
            outcome.scratch_high_water = outcome.scratch_high_water.max(scratch.high_water);
            outcome.scratch_capacity = outcome.scratch_capacity.max(scratch.capacity());
            outcome.scratch_fallbacks += scratch.fallbacks;
        }
        // SAFETY: the run is over.
        let growth = unsafe { &*self.growth_events.get() };
        outcome.growth_events.clear();
        outcome.growth_events.extend_from_slice(growth);
        outcome.final_population = self.current_state().len();
        outcome.final_capacity = self.tables().capacity;
        outcome.table_bytes = self.tables().bytes;
    }

    /// Frees what the classes own and returns the counters to zero, except
    /// for the one buffer the leak run keeps.
    fn destroy(mut self) -> [(i64, u64); TAG_COUNT] {
        let config = self.config;
        for worker in 0..config.workers {
            // SAFETY: the run is over; this thread is the only one left.
            unsafe { &mut *self.scratches[worker].get() }.release(&self.accounting);
            let buffers = unsafe { &*self.buffers[worker].get() };
            self.accounting.freed(0, TAG_BUFFERS, buffers.bytes());
        }
        self.accounting.freed(0, TAG_BUFFERS, self.cursor.get_mut().capacity() * 4);
        let tables = self.tables.get_mut();
        self.accounting.freed(0, TAG_TABLES, tables.bytes);
        if config.mode == Mode::PreSized {
            let mut bytes = tables.pre_sized_event_bytes();
            if config.leak_one_buffer {
                let leaked = tables.event_slots[0].get_mut().buffer.take().expect("a pre-sized buffer to leak");
                bytes -= leaked.capacity() * 4;
                std::mem::forget(leaked);
            }
            self.accounting.freed(0, TAG_BUFFERS, bytes);
        }
        std::array::from_fn(|tag| (self.accounting.bytes(tag), self.accounting.allocations(tag)))
    }
}

/// What a run leaves behind, in vectors the caller sized beforehand.
struct Outcome {
    tick_hashes: Vec<u64>,
    event_hashes: Vec<u64>,
    event_count: u64,
    allocations_per_tick: Vec<u64>,
    bytes_per_tick: Vec<u64>,
    growth_events: Vec<GrowthEvent>,
    total: Duration,
    stats: WorkerStats,
    failure: Option<Failure>,
    scratch_high_water: usize,
    scratch_capacity: usize,
    scratch_fallbacks: u64,
    final_population: usize,
    final_capacity: usize,
    table_bytes: usize,
}

impl Outcome {
    fn sized(tick_count: u32) -> Outcome {
        let ticks = tick_count as usize + 1;
        Outcome {
            tick_hashes: Vec::with_capacity(ticks),
            event_hashes: Vec::with_capacity(ticks),
            event_count: 0,
            allocations_per_tick: Vec::with_capacity(ticks),
            bytes_per_tick: Vec::with_capacity(ticks),
            growth_events: Vec::with_capacity(64),
            total: Duration::ZERO,
            stats: WorkerStats::default(),
            failure: None,
            scratch_high_water: 0,
            scratch_capacity: 0,
            scratch_fallbacks: 0,
            final_population: 0,
            final_capacity: 0,
            table_bytes: 0,
        }
    }

    fn run(&self, parameters: &Parameters) -> Run {
        let phases = NODE_NAMES.iter().enumerate().map(|(node, name)| (*name, self.stats.busy_ns[node])).collect();
        Run {
            parameters: *parameters,
            tick_hashes: self.tick_hashes.clone(),
            run_hash: run_hash(&self.tick_hashes),
            timing: Timing { phases, total: self.total },
        }
    }

    fn event_hash(&self) -> u64 {
        run_hash(&self.event_hashes)
    }

    /// Allocations per tick once the scratch has grown to its high-water
    /// mark: the mean and the maximum over the ticks after the first ten.
    fn steady_state(&self, values: &[u64]) -> (f64, u64, u64) {
        let from = (STEADY_STATE_FROM_TICK as usize - 1).min(values.len());
        let steady = &values[from..];
        let first: u64 = values[..from].iter().sum();
        if steady.is_empty() {
            return (0.0, 0, first);
        }
        let total: u64 = steady.iter().sum();
        (total as f64 / steady.len() as f64, *steady.iter().max().unwrap_or(&0), first)
    }
}

/// One measured run: the graph is created, run and destroyed, and the heap
/// counters before and after tell what the world left on the heap.
struct Measured {
    outcome: Outcome,
    tags: [(i64, u64); TAG_COUNT],
    heap_net_blocks: i64,
    heap_net_bytes: i64,
    heap_counted: bool,
}

fn run_once(parameters: &Parameters, config: Config) -> Measured {
    let mut outcome = Outcome::sized(parameters.tick_count);
    COUNTING.store(true, Ordering::SeqCst);
    let before = heap_totals();
    let graph = Graph::new(parameters, config, parameters.tick_count);
    graph.run(&mut outcome);
    let tags = graph.destroy();
    let after = heap_totals();
    Measured {
        outcome,
        tags,
        heap_net_blocks: (after.allocations as i64 - after.frees as i64) - (before.allocations as i64 - before.frees as i64),
        heap_net_bytes: (after.bytes_allocated as i64 - after.bytes_freed as i64) - (before.bytes_allocated as i64 - before.bytes_freed as i64),
        heap_counted: config.mode.counting(),
    }
}

/// A warm-up instance of `WARM_UP_TICKS` ticks, then the measured instance.
fn measure(parameters: &Parameters, config: Config) -> Measured {
    let warm_up = Parameters { tick_count: WARM_UP_TICKS, ..*parameters };
    run_once(&warm_up, config);
    run_once(parameters, config)
}

// ---------------------------------------------------------------------------
// The allocator microbenchmark
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    SystemDirect,
    WrapperOff,
    WrapperOn,
}

impl Path {
    fn name(self) -> &'static str {
        match self {
            Path::SystemDirect => "system-direct",
            Path::WrapperOff => "wrapper-off",
            Path::WrapperOn => "wrapper-on",
        }
    }
}

/// Nanoseconds per allocation and free pair of `MICROBENCHMARK_BYTES`, on
/// `threads` threads at once, through the system allocator directly, the
/// global allocator with counting off, and with counting on.
fn microbenchmark(path: Path, threads: usize) -> f64 {
    COUNTING.store(path == Path::WrapperOn, Ordering::SeqCst);
    let layout = Layout::from_size_align(MICROBENCHMARK_BYTES, 16).expect("a valid layout");
    let started = Instant::now();
    thread::scope(|scope| {
        for worker in 0..threads {
            scope.spawn(move || {
                register_thread(worker);
                for _ in 0..MICROBENCHMARK_ITERATIONS {
                    if path == Path::SystemDirect {
                        // SAFETY: a valid layout, freed at once.
                        unsafe {
                            let pointer = System.alloc(layout);
                            assert!(!pointer.is_null());
                            std::ptr::write_volatile(pointer, 1);
                            System.dealloc(pointer, layout);
                        }
                    } else {
                        let mut buffer: Vec<u8> = Vec::with_capacity(MICROBENCHMARK_BYTES);
                        buffer.push(1);
                        std::hint::black_box(&buffer);
                    }
                }
            });
        }
    });
    let elapsed = started.elapsed();
    COUNTING.store(true, Ordering::SeqCst);
    elapsed.as_secs_f64() * 1e9 / MICROBENCHMARK_ITERATIONS as f64
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

fn write_config(out: &mut Json, config: Config) {
    out.string("mode", config.mode.name());
    out.u64("workers", config.workers as u64);
    match config.growth {
        Some(growth) => {
            out.string("variant", if growth.grow { "grow" } else { "preallocated" });
            out.u64("rate", growth.rate as u64);
        }
        None => {
            out.string("variant", "specified");
            out.u64("rate", 0);
        }
    }
    out.bool("leak_one_buffer", config.leak_one_buffer);
}

fn write_measured(out: &mut Json, parameters: &Parameters, config: Config, measured: &Measured, reference: Option<&Run>) {
    let outcome = &measured.outcome;
    let run = outcome.run(parameters);
    out.object_start();
    write_config(out, config);
    write_run_core(out, &run);
    out.string("phase_ms_meaning", "busy time per node summed over the workers");
    match reference {
        Some(reference) => {
            let divergence = first_divergent_tick(&run.tick_hashes, &reference.tick_hashes);
            out.bool("matches_sequential", divergence.is_none());
            out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
        }
        None => {
            out.bool("matches_sequential", false);
            out.i64("first_divergent_tick", -2);
        }
    }
    out.hex("event_hash", outcome.event_hash());
    out.u64("events", outcome.event_count);
    out.bool("failure_reported", outcome.failure.is_some());
    if let Some(failure) = &outcome.failure {
        out.string("failure", &format!("tick {} {} chunk {} worker {}: {}", failure.tick, NODE_NAMES[failure.node], failure.chunk, failure.worker, failure.message));
    }
    out.u64("tasks", outcome.stats.tasks.iter().sum());
    out.u64("parks", outcome.stats.parks);
    out.key("heap");
    out.object_start();
    out.bool("counted", measured.heap_counted);
    let (mean_allocations, max_allocations, first_allocations) = outcome.steady_state(&outcome.allocations_per_tick);
    let (mean_bytes, max_bytes, first_bytes) = outcome.steady_state(&outcome.bytes_per_tick);
    out.decimal("steady_state_allocations_per_tick_mean", mean_allocations);
    out.u64("steady_state_allocations_per_tick_max", max_allocations);
    out.decimal("steady_state_bytes_per_tick_mean", mean_bytes);
    out.u64("steady_state_bytes_per_tick_max", max_bytes);
    out.u64("first_ten_ticks_allocations", first_allocations);
    out.u64("first_ten_ticks_bytes", first_bytes);
    let last_tick_with_allocation = outcome.allocations_per_tick.iter().rposition(|&count| count > 0).map_or(0, |index| index + 1);
    out.u64("last_tick_with_allocation", last_tick_with_allocation as u64);
    if measured.heap_counted {
        out.i64("net_blocks_after_destruction", measured.heap_net_blocks);
        out.i64("net_bytes_after_destruction", measured.heap_net_bytes);
    }
    out.object_end();
    out.key("scratch");
    out.object_start();
    out.u64("high_water_bytes", outcome.scratch_high_water as u64);
    out.u64("capacity_bytes", outcome.scratch_capacity as u64);
    out.u64("fallback_blocks", outcome.scratch_fallbacks);
    out.object_end();
    out.key("tables");
    out.object_start();
    out.u64("final_population", outcome.final_population as u64);
    out.u64("final_capacity", outcome.final_capacity as u64);
    out.u64("bytes", outcome.table_bytes as u64);
    out.u64("growth_events", outcome.growth_events.len() as u64);
    let growth_ns: u64 = outcome.growth_events.iter().map(|event| event.nanos).sum();
    out.decimal("growth_ms", growth_ns as f64 / 1e6);
    out.decimal("growth_share_of_run", if outcome.total.is_zero() { 0.0 } else { growth_ns as f64 / outcome.total.as_nanos() as f64 });
    out.key("growth");
    out.array_start();
    for event in &outcome.growth_events {
        out.object_start();
        out.u64("tick", u64::from(event.tick));
        out.u64("from", event.from as u64);
        out.u64("to", event.to as u64);
        out.decimal("microseconds", event.nanos as f64 / 1e3);
        out.object_end();
    }
    out.array_end();
    out.object_end();
    out.key("tags_at_destruction");
    out.object_start();
    for (tag, name) in TAG_NAMES.iter().enumerate() {
        out.key(name);
        out.object_start();
        out.i64("bytes", measured.tags[tag].0);
        out.u64("allocations", measured.tags[tag].1);
        out.object_end();
    }
    out.object_end();
    out.bool("leak_free", measured.tags.iter().all(|(bytes, _)| *bytes == 0));
    out.object_end();
}

fn write_sequential(out: &mut Json, reference: &Run) {
    out.object_start();
    out.string("mode", "sequential");
    out.u64("workers", 1);
    out.string("variant", "specified");
    out.u64("rate", 0);
    out.bool("leak_one_buffer", false);
    write_run_core(out, reference);
    out.string("phase_ms_meaning", "main-thread wall time per phase");
    out.bool("matches_sequential", true);
    out.i64("first_divergent_tick", -1);
    out.object_end();
}

fn main() {
    register_thread(0);
    COUNTING.store(true, Ordering::SeqCst);
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let available = thread::available_parallelism().map_or(1, |n| n.get()).min(DEFAULT_SLOT - 1);
    let mut worker_counts = BASE_WORKER_COUNTS.to_vec();
    if !worker_counts.contains(&available) {
        worker_counts.push(available);
        worker_counts.sort_unstable();
    }
    let mut growth_workers = GROWTH_WORKER_COUNTS.to_vec();
    if available > 4 {
        growth_workers.push(available);
    }
    let parameters = Parameters::new(Size::Default, DEFAULT_TICK_COUNT, false);
    let reference = run_sequential(&parameters);
    // The main thread's one-time initializations (its thread handle, the first
    // scope) are paid by a throwaway run, so that every measured run's heap
    // net after destruction counts the world's blocks only.
    run_once(&Parameters::new(Size::Small, 2, false), Config::new(Mode::PreSized, 2));

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.string("system_allocator", if cfg!(windows) { "HeapAlloc" } else if cfg!(target_os = "macos") { "macOS malloc" } else { "glibc malloc" });
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
    write_sequential(&mut out, &reference);
    for mode in Mode::ALL {
        for &workers in &worker_counts {
            let config = Config::new(mode, workers);
            let measured = measure(&parameters, config);
            write_measured(&mut out, &parameters, config, &measured, Some(&reference));
        }
    }
    out.array_end();

    out.key("growth_runs");
    out.array_start();
    for (rate, ticks) in GROWTH_RATES {
        let growth_parameters = Parameters { tick_count: ticks, ..parameters };
        for grow in [true, false] {
            for &workers in &growth_workers {
                let config = Config::new(Mode::Scratch, workers).growing(Growth { rate, grow });
                let measured = run_once(&growth_parameters, config);
                write_measured(&mut out, &growth_parameters, config, &measured, None);
            }
        }
    }
    out.array_end();

    out.key("leak_run");
    let leak_parameters = Parameters { tick_count: 50, ..parameters };
    let config = Config::new(Mode::PreSized, 4.min(available.max(1))).leaking();
    let measured = run_once(&leak_parameters, config);
    write_measured(&mut out, &leak_parameters, config, &measured, None);

    // Three rounds over the three paths, interleaved, so that the order of
    // measurement and the allocator's warm-up favour no path; the median and
    // the minimum of each path are reported.
    out.key("allocator_microbenchmark");
    out.array_start();
    for threads in [1, 4] {
        let paths = [Path::SystemDirect, Path::WrapperOff, Path::WrapperOn];
        let mut samples = [[0.0_f64; MICROBENCHMARK_ROUNDS]; 3];
        for round in 0..MICROBENCHMARK_ROUNDS {
            for (index, path) in paths.iter().enumerate() {
                samples[index][round] = microbenchmark(*path, threads);
            }
        }
        for (index, path) in paths.iter().enumerate() {
            let mut sorted = samples[index];
            sorted.sort_by(f64::total_cmp);
            out.object_start();
            out.string("path", path.name());
            out.u64("threads", threads as u64);
            out.u64("bytes", MICROBENCHMARK_BYTES as u64);
            out.u64("iterations_per_thread", MICROBENCHMARK_ITERATIONS as u64);
            out.u64("rounds", MICROBENCHMARK_ROUNDS as u64);
            out.decimal("nanoseconds_per_pair_median", sorted[MICROBENCHMARK_ROUNDS / 2]);
            out.decimal("nanoseconds_per_pair_min", sorted[0]);
            out.object_end();
        }
    }
    out.array_end();

    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    /// The counters and the counting switch are process-wide: the tests that
    /// read them run one at a time, and a test's thread leaves the counted
    /// slots before it releases the lock, so that what the harness does on
    /// that thread afterwards is not counted against the next test.
    struct Serial(#[allow(dead_code)] MutexGuard<'static, ()>);

    impl Drop for Serial {
        fn drop(&mut self) {
            register_thread(IGNORED_SLOT);
        }
    }

    fn serial() -> Serial {
        static LOCK: Mutex<()> = Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        register_thread(0);
        COUNTING.store(true, Ordering::SeqCst);
        // The thread's one-time initializations (its handle, the first scope)
        // are paid here so that the tests' heap nets see the world's blocks only.
        run_once(&Parameters::new(Size::Small, 2, false), Config::new(Mode::PreSized, 2));
        Serial(guard)
    }

    fn small(ticks: u32) -> Parameters {
        Parameters::new(Size::Small, ticks, false)
    }

    #[test]
    fn every_mode_reproduces_the_sequential_hashes_at_every_worker_count() {
        let _serial = serial();
        let parameters = small(40);
        let reference = run_sequential(&parameters);
        let mut event_hashes = Vec::new();
        for mode in Mode::ALL {
            for workers in [1, 2, 3, 8] {
                let measured = run_once(&parameters, Config::new(mode, workers));
                let run = measured.outcome.run(&parameters);
                assert_eq!(first_divergent_tick(&run.tick_hashes, &reference.tick_hashes), None, "{mode:?} at {workers} workers");
                assert!(measured.outcome.failure.is_none());
                assert!(measured.outcome.event_count > 0, "bounces happen on the small size");
                event_hashes.push(measured.outcome.event_hash());
                assert!(measured.tags.iter().all(|(bytes, _)| *bytes == 0), "{mode:?}: {:?}", measured.tags);
            }
        }
        event_hashes.dedup();
        assert_eq!(event_hashes.len(), 1, "one event hash across modes and worker counts");
    }

    #[test]
    fn the_wander_system_runs_with_the_right_tick() {
        let _serial = serial();
        let parameters = Parameters::new(Size::Small, 30, true);
        let reference = run_sequential(&parameters);
        for mode in [Mode::PreSized, Mode::Scratch, Mode::Heap] {
            let measured = run_once(&parameters, Config::new(mode, 3));
            assert_eq!(measured.outcome.run(&parameters).run_hash, reference.run_hash, "{mode:?}");
        }
    }

    /// The test harness's own thread allocates while a test runs (it prints
    /// the results of the other tests), in the default slot the totals count:
    /// the heap nets are therefore asserted on runs without spawned threads,
    /// where a mismatch is re-measured once before it counts as a failure.
    fn heap_net_blocks(parameters: &Parameters, config: Config) -> i64 {
        let first = run_once(parameters, config).heap_net_blocks;
        if first == 0 { first } else { run_once(parameters, config).heap_net_blocks }
    }

    #[test]
    fn the_pre_sized_and_scratch_modes_allocate_nothing_in_steady_state() {
        let _serial = serial();
        let parameters = small(60);
        for mode in [Mode::PreSized, Mode::Scratch] {
            for workers in [1, 3] {
                let measured = run_once(&parameters, Config::new(mode, workers));
                let (mean, max, _) = measured.outcome.steady_state(&measured.outcome.allocations_per_tick);
                assert_eq!((mean, max), (0.0, 0), "{mode:?} at {workers} workers: {:?}", measured.outcome.allocations_per_tick);
                assert!(measured.tags.iter().all(|(bytes, _)| *bytes == 0), "{mode:?}: {:?}", measured.tags);
            }
            assert_eq!(heap_net_blocks(&parameters, Config::new(mode, 1)), 0, "{mode:?}");
        }
        let measured = run_once(&parameters, Config::new(Mode::HeapCounted, 3));
        let (mean, _, _) = measured.outcome.steady_state(&measured.outcome.allocations_per_tick);
        assert!(mean >= 3.0 * 4.0, "the heap mode allocates per chunk: {mean}");
        assert_eq!(heap_net_blocks(&parameters, Config::new(Mode::HeapCounted, 1)), 0);
    }

    #[test]
    fn the_scratch_grows_to_its_high_water_mark_through_counted_fallbacks() {
        let _serial = serial();
        let accounting = Accounting::new(1);
        let mut scratch = Scratch::new(0, &accounting);
        assert_eq!(accounting.bytes(TAG_SCRATCH), SCRATCH_INITIAL_BLOCK as i64);
        let first = scratch.alloc::<f32>(1_000, &accounting);
        let second = scratch.alloc::<u32>(1_000, &accounting);
        assert_ne!(first as usize, second as usize);
        assert_eq!(scratch.fallbacks, 0);
        let large = scratch.alloc::<u8>(SCRATCH_INITIAL_BLOCK, &accounting);
        assert!(!large.is_null());
        assert_eq!(scratch.fallbacks, 1);
        assert_eq!(scratch.blocks.len(), 2);
        scratch.reset();
        assert_eq!(scratch.high_water, 8_000 + SCRATCH_INITIAL_BLOCK);
        let again = scratch.alloc::<f32>(1_000, &accounting);
        assert_eq!(again as usize, first as usize, "the reset starts over from the first block");
        scratch.release(&accounting);
        assert_eq!(accounting.bytes(TAG_SCRATCH), 0);
        assert_eq!(accounting.allocations(TAG_SCRATCH), 2);
    }

    #[test]
    fn growth_changes_no_hash_and_the_preallocated_variant_never_grows() {
        let _serial = serial();
        let parameters = small(30);
        let mut hashes = Vec::new();
        for rate in [8, 80] {
            let mut rate_hashes = Vec::new();
            for grow in [true, false] {
                for workers in [1, 3] {
                    let config = Config::new(Mode::Scratch, workers).growing(Growth { rate, grow });
                    let measured = run_once(&parameters, config);
                    assert!(measured.outcome.failure.is_none(), "{:?}", measured.outcome.failure);
                    assert_eq!(measured.outcome.final_population, 1_000 + 30 * rate);
                    assert_eq!(measured.outcome.growth_events.is_empty(), !grow, "rate {rate} grow {grow}");
                    assert!(measured.tags.iter().all(|(bytes, _)| *bytes == 0), "{:?}", measured.tags);
                    rate_hashes.push((measured.outcome.run(&parameters).run_hash, measured.outcome.event_hash()));
                }
            }
            rate_hashes.dedup();
            assert_eq!(rate_hashes.len(), 1, "one hash pair per rate across variants and worker counts");
            hashes.push(rate_hashes[0]);
        }
        assert_ne!(hashes[0], hashes[1]);
        let grown = run_once(&parameters, Config::new(Mode::PreSized, 2).growing(Growth { rate: 80, grow: true }));
        assert!(grown.outcome.growth_events.len() >= 2);
        assert!(grown.tags.iter().all(|(bytes, _)| *bytes == 0));
    }

    #[test]
    fn a_leaked_buffer_is_reported_by_its_tag() {
        let _serial = serial();
        let measured = run_once(&small(5), Config::new(Mode::PreSized, 2).leaking());
        assert_eq!(measured.tags[TAG_BUFFERS].0, 2 * CHUNK as i64 * 4);
        assert!(measured.tags.iter().enumerate().all(|(tag, (bytes, _))| tag == TAG_BUFFERS || *bytes == 0));
        let net = {
            let first = run_once(&small(5), Config::new(Mode::PreSized, 1).leaking());
            if first.heap_net_blocks == 1 { first } else { run_once(&small(5), Config::new(Mode::PreSized, 1).leaking()) }
        };
        assert_eq!(net.heap_net_blocks, 1);
        assert_eq!(net.heap_net_bytes, 2 * CHUNK as i64 * 4);
    }

    #[test]
    fn the_counting_wrapper_counts_what_this_thread_allocates() {
        let _serial = serial();
        let before = heap_totals();
        let buffer: Vec<u8> = Vec::with_capacity(4_096);
        let after_allocation = heap_totals();
        drop(buffer);
        let after_free = heap_totals();
        assert_eq!(after_allocation.allocations - before.allocations, 1);
        assert_eq!(after_allocation.bytes_allocated - before.bytes_allocated, 4_096);
        assert_eq!(after_free.frees - before.frees, 1);
        COUNTING.store(false, Ordering::SeqCst);
        let _silent: Vec<u8> = Vec::with_capacity(4_096);
        assert_eq!(heap_totals().allocations, after_free.allocations);
        COUNTING.store(true, Ordering::SeqCst);
    }
}
