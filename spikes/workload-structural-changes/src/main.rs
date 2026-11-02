//! Deferred structural changes on three storage layouts of the reference
//! workload for the ECS research.
//!
//! Throwaway spike, never product code. It runs the reference workload
//! (default size, wander system off) over the three layouts of the storage
//! layouts spike (identifier-indexed slots, chunked archetype tables with a
//! slot table, sparse sets per component) and measures what structural
//! changes cost and whether deferred command buffers keep the results
//! independent of the worker count:
//!
//! - recording `main-thread`: kills and spawns as the specification states
//!   them, drawn from the global stream at the fixed point, at 8 (the
//!   specified rate), 80 and 800 changes per tick;
//! - recording `parallel`: the motion pass records, into a command buffer per
//!   chunk, despawns and spawns by a deterministic rule on the slot index and
//!   the tick, and adds or removes of a spike-local `heat` component by a
//!   second rule; the fixed point applies the buffers in chunk order and
//!   record order;
//! - identifier allocation `at-application` (the spawn's slot is chosen when
//!   the command is applied) or `reserved` (each chunk reserves a range of
//!   slots before the pass, in chunk order, and returns the unused ones at the
//!   fixed point in chunk order);
//! - free list policies `scan` (the lowest dead slot by a linear scan, as the
//!   specification states), `heap` (the lowest free slot from a binary heap)
//!   and `stack` (the most recently freed slot).
//!
//! In the parallel recording the slot table may grow, so that spawns applied
//! before the despawns recorded after them in the same tick still find a
//! slot; those runs compare their hashes across worker counts, not with the
//! sequential reference.

use std::cell::UnsafeCell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Range;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::Barrier;
use std::thread;
use std::time::Instant;

use reference_workload::{
    CHUNK, DEFAULT_TICK_COUNT, Fnv1a, GRID_SIDE, INTERACTION_RADIUS, Json, Parameters,
    REPULSION_STRENGTH, Run, Size, SplitMix64, State, Timing, WARM_UP_TICKS, WORLD_SIDE, bounced,
    chunk_range, first_divergent_tick, integrated, kinetic_energy, repelled_velocity, run_hash,
    run_sequential, wandered_velocity, write_platform_and_build, write_run_core,
};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-structural-changes";
const RATES: [usize; 3] = [8, 80, 800];
const WORKER_COUNTS: [usize; 2] = [1, 4];
const NONE: u32 = u32::MAX;
const PHASES: [&str; 8] =
    ["canonical_traversal", "spatial_hashing", "reservation", "repulsion", "motion", "energy", "apply", "hash"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    Slots,
    Archetypes,
    Sparse,
}

impl Layout {
    const ALL: [Layout; 3] = [Layout::Slots, Layout::Archetypes, Layout::Sparse];

    fn name(self) -> &'static str {
        match self {
            Layout::Slots => "slots",
            Layout::Archetypes => "archetypes",
            Layout::Sparse => "sparse",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Recording {
    /// Kills and spawns from the global stream at the fixed point, as specified.
    MainThread,
    /// Despawns, spawns and heat toggles recorded by the motion pass into
    /// per-chunk command buffers.
    Parallel,
}

impl Recording {
    fn name(self) -> &'static str {
        match self {
            Recording::MainThread => "main-thread",
            Recording::Parallel => "parallel",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Allocation {
    AtApplication,
    Reserved,
}

impl Allocation {
    fn name(self) -> &'static str {
        match self {
            Allocation::AtApplication => "at-application",
            Allocation::Reserved => "reserved",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Policy {
    Scan,
    Heap,
    Stack,
}

impl Policy {
    fn name(self) -> &'static str {
        match self {
            Policy::Scan => "scan",
            Policy::Heap => "heap",
            Policy::Stack => "stack",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Config {
    layout: Layout,
    recording: Recording,
    allocation: Allocation,
    policy: Policy,
    rate: usize,
    workers: usize,
}

impl Config {
    /// Whether the run follows the specification, so that its hashes compare
    /// with the sequential reference: the specified rate, kills and spawns on
    /// the main thread, the lowest dead slot reused.
    fn follows_the_specification(&self) -> bool {
        self.recording == Recording::MainThread && self.rate == 8 && self.policy != Policy::Stack
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

type Row = (f32, f32, f32, f32, f32);

/// The draws of a spawn or of the initialization, in the specified order.
fn draw(rng: &mut SplitMix64) -> Row {
    let x = rng.u01() * WORLD_SIDE;
    let y = rng.u01() * WORLD_SIDE;
    let vx = ((rng.u01() * 2.0) - 1.0) * 32.0;
    let vy = ((rng.u01() * 2.0) - 1.0) * 32.0;
    let mass = rng.mass();
    (x, y, vx, vy, mass)
}

/// A 64-bit mix of a slot and a tick, for the recording rules and the values
/// of the spawns recorded in parallel: a function of the entity and the tick,
/// never of the worker.
fn mix(slot: usize, tick: u32, salt: u64) -> u64 {
    let mut stream = SplitMix64::new((slot as u64) ^ (u64::from(tick) << 32) ^ salt);
    stream.next()
}

fn despawn_rule(slot: usize, tick: u32, rate: usize, slot_count: usize) -> bool {
    (mix(slot, tick, 0x5e5a_1e5c) % slot_count as u64) < rate as u64
}

fn toggle_rule(slot: usize, tick: u32, rate: usize, slot_count: usize) -> bool {
    (mix(slot, tick, 0x7e47_0a9d) % slot_count as u64) < rate as u64
}

/// The values of an entity spawned by the parallel recording.
fn parallel_spawn_values(slot: usize, tick: u32) -> Row {
    draw(&mut SplitMix64::new(mix(slot, tick, 0x5a7d_a9e5)))
}

#[derive(Clone, Copy)]
struct Globals {
    rng: SplitMix64,
    energy: f32,
}

fn hash_slot(hasher: &mut Fnv1a, generation: u32, alive: bool, row: Option<Row>) {
    hasher.write(&generation.to_le_bytes());
    hasher.write(&[u8::from(alive)]);
    if let Some((x, y, vx, vy, mass)) = row {
        hasher.write(&x.to_le_bytes());
        hasher.write(&y.to_le_bytes());
        hasher.write(&vx.to_le_bytes());
        hasher.write(&vy.to_le_bytes());
        hasher.write(&mass.to_le_bytes());
    }
}

fn hash_globals(hasher: &mut Fnv1a, globals: &Globals) -> u64 {
    hasher.write(&globals.energy.to_le_bytes());
    hasher.write(&globals.rng.state.to_le_bytes());
    hasher.finish()
}

fn hash_heat(hasher: &mut Fnv1a, slot: usize, heat: f32) {
    hasher.write(&(slot as u32).to_le_bytes());
    hasher.write(&heat.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Raw column views, the spatial grid, the free list and the command buffers
// ---------------------------------------------------------------------------

/// A raw view of one column, refreshed by the main thread before the passes.
/// Tasks write disjoint slots of it without forming a reference to the whole
/// column: the one `unsafe` construct of the spike.
#[derive(Clone, Copy)]
struct Raw<T> {
    ptr: *mut T,
    len: usize,
}

unsafe impl<T> Send for Raw<T> {}
unsafe impl<T> Sync for Raw<T> {}

impl<T: Copy> Raw<T> {
    fn of(values: &mut [T]) -> Raw<T> {
        Raw { ptr: values.as_mut_ptr(), len: values.len() }
    }

    fn empty() -> Raw<T> {
        Raw { ptr: std::ptr::null_mut(), len: 0 }
    }

    /// # Safety
    ///
    /// No other thread reads or writes the slots of `range` while the slice lives.
    unsafe fn slice(&self, range: Range<usize>) -> &mut [T] {
        assert!(range.start <= range.end && range.end <= self.len, "chunk slice out of bounds");
        // SAFETY: in bounds by the assertion; disjointness is the caller's.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(range.start), range.len()) }
    }

    /// # Safety
    ///
    /// No other thread reads or writes slot `index` during the pass.
    unsafe fn write(&self, index: usize, value: T) {
        assert!(index < self.len, "element out of bounds");
        // SAFETY: in bounds by the assertion; disjointness is the caller's.
        unsafe { *self.ptr.add(index) = value };
    }

    /// # Safety
    ///
    /// No other thread writes slot `index` during the pass.
    unsafe fn read(&self, index: usize) -> T {
        assert!(index < self.len, "element out of bounds");
        // SAFETY: in bounds by the assertion; disjointness is the caller's.
        unsafe { *self.ptr.add(index) }
    }
}

#[derive(Clone, Copy, Default)]
struct Entry {
    slot: u32,
    x: f32,
    y: f32,
}

/// System 1 as a counting sort of slot-ordered entries.
struct Grid {
    cell_start: Vec<u32>,
    entries: Vec<Entry>,
}

impl Grid {
    fn new() -> Grid {
        Grid { cell_start: vec![0; GRID_SIDE * GRID_SIDE + 1], entries: Vec::new() }
    }

    fn build(&mut self, ordered: &[Entry]) {
        let cells = GRID_SIDE * GRID_SIDE;
        self.cell_start.clear();
        self.cell_start.resize(cells + 1, 0);
        for entry in ordered {
            let (cx, cy) = reference_workload::Grid::cell_of(entry.x, entry.y);
            self.cell_start[cy * GRID_SIDE + cx + 1] += 1;
        }
        for cell in 0..cells {
            self.cell_start[cell + 1] += self.cell_start[cell];
        }
        let mut cursor = self.cell_start.clone();
        self.entries.clear();
        self.entries.resize(ordered.len(), Entry::default());
        for entry in ordered {
            let (cx, cy) = reference_workload::Grid::cell_of(entry.x, entry.y);
            let cell = cy * GRID_SIDE + cx;
            self.entries[cursor[cell] as usize] = *entry;
            cursor[cell] += 1;
        }
    }

    fn entries_in(&self, cx: usize, cy: usize) -> &[Entry] {
        let cell = cy * GRID_SIDE + cx;
        &self.entries[self.cell_start[cell] as usize..self.cell_start[cell + 1] as usize]
    }
}

/// System 2 for one slot, reading the neighbors' positions from the grid in
/// the specified visit order; the arithmetic is the reference's.
fn repulsion_force(xi: f32, yi: f32, i: u32, grid: &Grid) -> (f32, f32) {
    let (cx, cy) = reference_workload::Grid::cell_of(xi, yi);
    let mut fx = 0.0_f32;
    let mut fy = 0.0_f32;
    for b in -1_isize..=1 {
        for a in -1_isize..=1 {
            let (gx, gy) = (cx as isize + a, cy as isize + b);
            if gx < 0 || gy < 0 || gx >= GRID_SIDE as isize || gy >= GRID_SIDE as isize {
                continue;
            }
            for entry in grid.entries_in(gx as usize, gy as usize) {
                if entry.slot == i {
                    continue;
                }
                let dx = entry.x - xi;
                let dy = entry.y - yi;
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

/// The free slots, under one of the three policies. The scan policy keeps no
/// structure: it finds the lowest dead slot in the liveness column.
struct FreeList {
    policy: Policy,
    heap: BinaryHeap<Reverse<u32>>,
    stack: Vec<u32>,
}

impl FreeList {
    fn new(policy: Policy) -> FreeList {
        FreeList { policy, heap: BinaryHeap::new(), stack: Vec::new() }
    }

    fn free(&mut self, slot: usize) {
        match self.policy {
            Policy::Scan => {}
            Policy::Heap => self.heap.push(Reverse(slot as u32)),
            Policy::Stack => self.stack.push(slot as u32),
        }
    }

    /// The next free slot, or none; `alive` is scanned under the scan policy.
    fn take(&mut self, alive: &[bool]) -> Option<usize> {
        match self.policy {
            Policy::Scan => alive.iter().position(|alive| !alive),
            Policy::Heap => self.heap.pop().map(|Reverse(slot)| slot as usize),
            Policy::Stack => self.stack.pop().map(|slot| slot as usize),
        }
    }
}

/// One deferred structural change, as the motion pass records it.
#[derive(Clone, Copy, Debug)]
enum Command {
    Despawn(u32),
    /// A spawn with the slot reserved at record time, or none.
    Spawn { reserved: u32, values: Row },
    /// Add the heat component if absent, remove it otherwise.
    Toggle(u32),
}

/// The command buffer and the slot reservations of one chunk.
#[derive(Default)]
struct ChunkCommands {
    commands: Vec<Command>,
    reserved: Vec<u32>,
    reserved_used: usize,
}

/// What the fixed point counted while applying the commands of a run.
#[derive(Clone, Copy, Default, Debug)]
struct Counts {
    despawns: u64,
    spawns: u64,
    adds: u64,
    removes: u64,
    /// Spawns recorded with a reservation.
    reserved_spawns: u64,
    /// Spawns recorded after a chunk ran out of reservations.
    overflow_spawns: u64,
    /// Reservations returned unused at the fixed point.
    returned_reservations: u64,
    /// Slots added to the slot table.
    grown_slots: u64,
}

impl Counts {
    fn changes(&self) -> u64 {
        self.despawns + self.spawns + self.adds + self.removes
    }
}

// ---------------------------------------------------------------------------
// The layouts
// ---------------------------------------------------------------------------

/// What the pool and the fixed point need from a layout.
trait Storage: Sync {
    fn slot_count(&self) -> usize;
    fn alive(&self) -> &[bool];
    fn generation(&self) -> &[u32];
    /// Adds one dead slot to the slot table and returns its index.
    fn grow(&mut self) -> usize;
    fn main_chunk_count(&self) -> usize;
    fn prepare_passes(&mut self);
    fn canonical_entries(&self, out: &mut Vec<Entry>);
    /// # Safety
    ///
    /// One task per chunk per pass; the chunk's rows are written by that task only.
    unsafe fn repulsion_chunk(&self, chunk: usize, grid: &Grid);
    /// # Safety
    ///
    /// As above; `energy` receives one value per slot; `record` is called for
    /// each row in storage order with its slot, when recording is on.
    unsafe fn motion_chunk(&self, chunk: usize, tick: u32, wander: bool, energy: &Raw<f32>, record: Option<&mut dyn FnMut(usize)>);
    /// Marks `slot` dead and removes its row.
    fn despawn(&mut self, slot: usize);
    /// Marks `slot` alive with the given row (no heat) and bumps its generation.
    fn spawn(&mut self, slot: usize, row: Row);
    fn has_heat(&self, slot: usize) -> bool;
    fn add_heat(&mut self, slot: usize, heat: f32);
    fn remove_heat(&mut self, slot: usize);
    fn hash(&self, globals: &Globals) -> u64;
    fn heat_hash(&self) -> u64;
}

// --- slots ------------------------------------------------------------------

struct Slots {
    generation: Vec<u32>,
    alive: Vec<bool>,
    x: Vec<f32>,
    y: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    mass: Vec<f32>,
    with_heat: Vec<bool>,
    heat: Vec<f32>,
    raw: [Raw<f32>; 4],
}

impl Slots {
    fn new(state: &State) -> Slots {
        let n = state.len();
        let mut slots = Slots {
            generation: state.generation.clone(),
            alive: state.alive.clone(),
            x: state.x.clone(),
            y: state.y.clone(),
            vx: state.vx.clone(),
            vy: state.vy.clone(),
            mass: state.mass.clone(),
            with_heat: vec![false; n],
            heat: vec![0.0; n],
            raw: [Raw::empty(); 4],
        };
        slots.prepare_passes();
        slots
    }
}

impl Storage for Slots {
    fn slot_count(&self) -> usize {
        self.alive.len()
    }

    fn alive(&self) -> &[bool] {
        &self.alive
    }

    fn generation(&self) -> &[u32] {
        &self.generation
    }

    fn grow(&mut self) -> usize {
        self.generation.push(0);
        self.alive.push(false);
        self.x.push(0.0);
        self.y.push(0.0);
        self.vx.push(0.0);
        self.vy.push(0.0);
        self.mass.push(0.0);
        self.with_heat.push(false);
        self.heat.push(0.0);
        self.alive.len() - 1
    }

    fn main_chunk_count(&self) -> usize {
        self.alive.len().div_ceil(CHUNK)
    }

    fn prepare_passes(&mut self) {
        self.raw = [Raw::of(&mut self.x), Raw::of(&mut self.y), Raw::of(&mut self.vx), Raw::of(&mut self.vy)];
    }

    fn canonical_entries(&self, out: &mut Vec<Entry>) {
        for slot in 0..self.alive.len() {
            if self.alive[slot] {
                out.push(Entry { slot: slot as u32, x: self.x[slot], y: self.y[slot] });
            }
        }
    }

    unsafe fn repulsion_chunk(&self, chunk: usize, grid: &Grid) {
        let range = chunk_range(chunk, self.alive.len());
        // SAFETY: this chunk's slots of the velocity columns are written by this task only.
        let (vx, vy) = unsafe { (self.raw[2].slice(range.clone()), self.raw[3].slice(range.clone())) };
        for (k, i) in range.enumerate() {
            if self.alive[i] {
                let (fx, fy) = repulsion_force(self.x[i], self.y[i], i as u32, grid);
                vx[k] = repelled_velocity(vx[k], fx, self.mass[i]);
                vy[k] = repelled_velocity(vy[k], fy, self.mass[i]);
            }
        }
    }

    unsafe fn motion_chunk(&self, chunk: usize, tick: u32, wander: bool, energy: &Raw<f32>, mut record: Option<&mut dyn FnMut(usize)>) {
        let range = chunk_range(chunk, self.alive.len());
        // SAFETY: this chunk's slots of the four columns and of the energy scratch are
        // read and written by this task only.
        let (x, y, vx, vy, e) = unsafe {
            (
                self.raw[0].slice(range.clone()),
                self.raw[1].slice(range.clone()),
                self.raw[2].slice(range.clone()),
                self.raw[3].slice(range.clone()),
                energy.slice(range.clone()),
            )
        };
        for (k, i) in range.enumerate() {
            if self.alive[i] {
                let (nx, ny, nvx, nvy, ne) = motion_slot(x[k], y[k], vx[k], vy[k], self.mass[i], tick, i, wander);
                x[k] = nx;
                y[k] = ny;
                vx[k] = nvx;
                vy[k] = nvy;
                e[k] = ne;
                if let Some(record) = record.as_mut() {
                    record(i);
                }
            }
        }
    }

    fn despawn(&mut self, slot: usize) {
        self.alive[slot] = false;
    }

    fn spawn(&mut self, slot: usize, row: Row) {
        self.x[slot] = row.0;
        self.y[slot] = row.1;
        self.vx[slot] = row.2;
        self.vy[slot] = row.3;
        self.mass[slot] = row.4;
        self.generation[slot] += 1;
        self.alive[slot] = true;
        self.with_heat[slot] = false;
    }

    fn has_heat(&self, slot: usize) -> bool {
        self.with_heat[slot]
    }

    fn add_heat(&mut self, slot: usize, heat: f32) {
        self.with_heat[slot] = true;
        self.heat[slot] = heat;
    }

    fn remove_heat(&mut self, slot: usize) {
        self.with_heat[slot] = false;
    }

    fn hash(&self, globals: &Globals) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.alive.len() {
            let row = self.alive[slot].then(|| (self.x[slot], self.y[slot], self.vx[slot], self.vy[slot], self.mass[slot]));
            hash_slot(&mut hasher, self.generation[slot], self.alive[slot], row);
        }
        hash_globals(&mut hasher, globals)
    }

    fn heat_hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.alive.len() {
            if self.alive[slot] && self.with_heat[slot] {
                hash_heat(&mut hasher, slot, self.heat[slot]);
            }
        }
        hasher.finish()
    }
}

// --- archetypes -------------------------------------------------------------

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Location {
    archetype: u8,
    chunk: u32,
    row: u32,
}

struct Chunk {
    slot: Vec<u32>,
    x: Vec<f32>,
    y: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    mass: Vec<f32>,
    heat: Vec<f32>,
}

impl Chunk {
    fn new(with_heat: bool) -> Chunk {
        Chunk {
            slot: Vec::with_capacity(CHUNK),
            x: Vec::with_capacity(CHUNK),
            y: Vec::with_capacity(CHUNK),
            vx: Vec::with_capacity(CHUNK),
            vy: Vec::with_capacity(CHUNK),
            mass: Vec::with_capacity(CHUNK),
            heat: Vec::with_capacity(if with_heat { CHUNK } else { 0 }),
        }
    }

    fn len(&self) -> usize {
        self.slot.len()
    }

    fn push(&mut self, slot: u32, row: Row, heat: Option<f32>) {
        self.slot.push(slot);
        self.x.push(row.0);
        self.y.push(row.1);
        self.vx.push(row.2);
        self.vy.push(row.3);
        self.mass.push(row.4);
        if let Some(heat) = heat {
            self.heat.push(heat);
        }
    }

    fn row(&self, row: usize) -> Row {
        (self.x[row], self.y[row], self.vx[row], self.vy[row], self.mass[row])
    }

    fn pop(&mut self) -> (u32, Row, Option<f32>) {
        let slot = self.slot.pop().expect("pop from a non-empty chunk");
        let row = (
            self.x.pop().unwrap(),
            self.y.pop().unwrap(),
            self.vx.pop().unwrap(),
            self.vy.pop().unwrap(),
            self.mass.pop().unwrap(),
        );
        (slot, row, self.heat.pop())
    }

    fn overwrite(&mut self, row: usize, slot: u32, values: Row, heat: Option<f32>) {
        self.slot[row] = slot;
        self.x[row] = values.0;
        self.y[row] = values.1;
        self.vx[row] = values.2;
        self.vy[row] = values.3;
        self.mass[row] = values.4;
        if let Some(heat) = heat {
            self.heat[row] = heat;
        }
    }
}

struct Table {
    with_heat: bool,
    chunks: Vec<UnsafeCell<Chunk>>,
    spare: Vec<Chunk>,
}

// SAFETY: a chunk is written during a pass by the one task that owns it, and
// by the main thread between passes.
unsafe impl Sync for Table {}

impl Table {
    fn new(with_heat: bool) -> Table {
        Table { with_heat, chunks: Vec::new(), spare: Vec::new() }
    }

    fn chunk(&self, index: usize) -> &Chunk {
        // SAFETY: shared reads between passes, or of a chunk the caller owns.
        unsafe { &*self.chunks[index].get() }
    }

    fn chunk_mut(&mut self, index: usize) -> &mut Chunk {
        self.chunks[index].get_mut()
    }

    fn insert(&mut self, slot: u32, row: Row, heat: Option<f32>) -> (u32, u32) {
        let last_full = self.chunks.last().is_none_or(|chunk| {
            // SAFETY: the main thread owns the table between passes.
            unsafe { &*chunk.get() }.len() == CHUNK
        });
        if last_full {
            let chunk = self.spare.pop().unwrap_or_else(|| Chunk::new(self.with_heat));
            self.chunks.push(UnsafeCell::new(chunk));
        }
        let index = self.chunks.len() - 1;
        let chunk = self.chunk_mut(index);
        chunk.push(slot, row, heat);
        (index as u32, (chunk.len() - 1) as u32)
    }

    /// Removes a row by moving the table's last row into its place; returns
    /// the removed row and the slot that moved, if any.
    fn swap_remove(&mut self, chunk: u32, row: u32) -> (Row, Option<f32>, Option<u32>) {
        let last_chunk = self.chunks.len() - 1;
        let (moved_slot, moved_row, moved_heat) = self.chunk_mut(last_chunk).pop();
        let (removed, moved) = if (chunk as usize, row as usize) == (last_chunk, self.chunk(last_chunk).len()) {
            ((moved_row, moved_heat), None)
        } else {
            let target = self.chunk_mut(chunk as usize);
            let removed = (target.row(row as usize), target.heat.get(row as usize).copied());
            target.overwrite(row as usize, moved_slot, moved_row, moved_heat);
            (removed, Some(moved_slot))
        };
        if self.chunk(last_chunk).len() == 0 {
            let empty = self.chunks.pop().expect("the last chunk exists").into_inner();
            self.spare.push(empty);
        }
        (removed.0, removed.1, moved)
    }
}

struct Archetypes {
    generation: Vec<u32>,
    alive: Vec<bool>,
    location: Vec<Location>,
    tables: [Table; 2],
    main_chunks: Vec<(u8, u32)>,
}

impl Archetypes {
    fn new(state: &State) -> Archetypes {
        let n = state.len();
        let mut archetypes = Archetypes {
            generation: state.generation.clone(),
            alive: state.alive.clone(),
            location: vec![Location::default(); n],
            tables: [Table::new(false), Table::new(true)],
            main_chunks: Vec::new(),
        };
        for slot in 0..n {
            if state.alive[slot] {
                let row = (state.x[slot], state.y[slot], state.vx[slot], state.vy[slot], state.mass[slot]);
                archetypes.insert(slot, 0, row, None);
            }
        }
        archetypes.rebuild_chunk_list();
        archetypes
    }

    fn insert(&mut self, slot: usize, archetype: u8, row: Row, heat: Option<f32>) {
        let (chunk, index) = self.tables[archetype as usize].insert(slot as u32, row, heat);
        self.location[slot] = Location { archetype, chunk, row: index };
    }

    /// Removes the row of `slot` and returns it.
    fn remove(&mut self, slot: usize) -> (Row, Option<f32>) {
        let location = self.location[slot];
        let (row, heat, moved) = self.tables[location.archetype as usize].swap_remove(location.chunk, location.row);
        if let Some(moved) = moved {
            self.location[moved as usize] = location;
        }
        (row, heat)
    }

    fn rebuild_chunk_list(&mut self) {
        self.main_chunks.clear();
        for (archetype, table) in self.tables.iter().enumerate() {
            for chunk in 0..table.chunks.len() {
                self.main_chunks.push((archetype as u8, chunk as u32));
            }
        }
    }

    /// # Safety
    ///
    /// The caller is the one task owning this chunk during the pass.
    unsafe fn owned_chunk(&self, (archetype, chunk): (u8, u32)) -> &mut Chunk {
        // SAFETY: forwarded to the caller.
        unsafe { &mut *self.tables[archetype as usize].chunks[chunk as usize].get() }
    }

    fn row_of(&self, slot: usize) -> &Chunk {
        let location = self.location[slot];
        self.tables[location.archetype as usize].chunk(location.chunk as usize)
    }

    #[cfg(test)]
    fn check_consistency(&self) {
        let mut rows = 0;
        for (archetype, table) in self.tables.iter().enumerate() {
            for index in 0..table.chunks.len() {
                let chunk = table.chunk(index);
                assert!(chunk.len() > 0 && chunk.len() <= CHUNK);
                assert!(index + 1 == table.chunks.len() || chunk.len() == CHUNK, "only the last chunk is partial");
                assert_eq!(chunk.heat.len(), if table.with_heat { chunk.len() } else { 0 });
                for row in 0..chunk.len() {
                    let slot = chunk.slot[row] as usize;
                    assert!(self.alive[slot]);
                    assert_eq!(self.location[slot], Location { archetype: archetype as u8, chunk: index as u32, row: row as u32 });
                    rows += 1;
                }
            }
        }
        assert_eq!(rows, self.alive.iter().filter(|&&alive| alive).count());
    }
}

impl Storage for Archetypes {
    fn slot_count(&self) -> usize {
        self.alive.len()
    }

    fn alive(&self) -> &[bool] {
        &self.alive
    }

    fn generation(&self) -> &[u32] {
        &self.generation
    }

    fn grow(&mut self) -> usize {
        self.generation.push(0);
        self.alive.push(false);
        self.location.push(Location::default());
        self.alive.len() - 1
    }

    fn main_chunk_count(&self) -> usize {
        self.main_chunks.len()
    }

    fn prepare_passes(&mut self) {
        self.rebuild_chunk_list();
    }

    fn canonical_entries(&self, out: &mut Vec<Entry>) {
        for slot in 0..self.alive.len() {
            if self.alive[slot] {
                let location = self.location[slot];
                let chunk = self.tables[location.archetype as usize].chunk(location.chunk as usize);
                let row = location.row as usize;
                out.push(Entry { slot: slot as u32, x: chunk.x[row], y: chunk.y[row] });
            }
        }
    }

    unsafe fn repulsion_chunk(&self, task: usize, grid: &Grid) {
        // SAFETY: one task per chunk per pass.
        let chunk = unsafe { self.owned_chunk(self.main_chunks[task]) };
        for row in 0..chunk.len() {
            let slot = chunk.slot[row];
            let (fx, fy) = repulsion_force(chunk.x[row], chunk.y[row], slot, grid);
            chunk.vx[row] = repelled_velocity(chunk.vx[row], fx, chunk.mass[row]);
            chunk.vy[row] = repelled_velocity(chunk.vy[row], fy, chunk.mass[row]);
        }
    }

    unsafe fn motion_chunk(&self, task: usize, tick: u32, wander: bool, energy: &Raw<f32>, mut record: Option<&mut dyn FnMut(usize)>) {
        // SAFETY: one task per chunk per pass; each slot's energy is written by the
        // one row that carries the slot.
        let chunk = unsafe { self.owned_chunk(self.main_chunks[task]) };
        for row in 0..chunk.len() {
            let slot = chunk.slot[row] as usize;
            let (nx, ny, nvx, nvy, e) =
                motion_slot(chunk.x[row], chunk.y[row], chunk.vx[row], chunk.vy[row], chunk.mass[row], tick, slot, wander);
            chunk.x[row] = nx;
            chunk.y[row] = ny;
            chunk.vx[row] = nvx;
            chunk.vy[row] = nvy;
            // SAFETY: see above.
            unsafe { energy.write(slot, e) };
            if let Some(record) = record.as_mut() {
                record(slot);
            }
        }
    }

    fn despawn(&mut self, slot: usize) {
        self.alive[slot] = false;
        self.remove(slot);
    }

    fn spawn(&mut self, slot: usize, row: Row) {
        self.generation[slot] += 1;
        self.alive[slot] = true;
        self.insert(slot, 0, row, None);
    }

    fn has_heat(&self, slot: usize) -> bool {
        self.location[slot].archetype == 1
    }

    /// The row moves from the plain table to the heat table.
    fn add_heat(&mut self, slot: usize, heat: f32) {
        let (row, _) = self.remove(slot);
        self.insert(slot, 1, row, Some(heat));
    }

    fn remove_heat(&mut self, slot: usize) {
        let (row, _) = self.remove(slot);
        self.insert(slot, 0, row, None);
    }

    fn hash(&self, globals: &Globals) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.alive.len() {
            let row = self.alive[slot].then(|| self.row_of(slot).row(self.location[slot].row as usize));
            hash_slot(&mut hasher, self.generation[slot], self.alive[slot], row);
        }
        hash_globals(&mut hasher, globals)
    }

    fn heat_hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.alive.len() {
            if self.alive[slot] && self.location[slot].archetype == 1 {
                hash_heat(&mut hasher, slot, self.row_of(slot).heat[self.location[slot].row as usize]);
            }
        }
        hasher.finish()
    }
}

// --- sparse sets ------------------------------------------------------------

struct SparseSet<const COLUMNS: usize> {
    sparse: Vec<u32>,
    dense_slot: Vec<u32>,
    columns: [Vec<f32>; COLUMNS],
    raw_columns: [Raw<f32>; COLUMNS],
}

impl<const COLUMNS: usize> SparseSet<COLUMNS> {
    fn new(slot_count: usize) -> SparseSet<COLUMNS> {
        SparseSet {
            sparse: vec![NONE; slot_count],
            dense_slot: Vec::new(),
            columns: std::array::from_fn(|_| Vec::new()),
            raw_columns: [Raw::empty(); COLUMNS],
        }
    }

    fn len(&self) -> usize {
        self.dense_slot.len()
    }

    fn contains(&self, slot: usize) -> bool {
        self.sparse[slot] != NONE
    }

    fn index_of(&self, slot: usize) -> usize {
        self.sparse[slot] as usize
    }

    fn insert(&mut self, slot: usize, values: [f32; COLUMNS]) {
        self.sparse[slot] = self.dense_slot.len() as u32;
        self.dense_slot.push(slot as u32);
        for (column, value) in self.columns.iter_mut().zip(values) {
            column.push(value);
        }
    }

    fn remove(&mut self, slot: usize) {
        let index = self.index_of(slot);
        let last = self.dense_slot.len() - 1;
        let moved_slot = self.dense_slot[last];
        self.dense_slot.swap_remove(index);
        for column in &mut self.columns {
            column.swap_remove(index);
        }
        if index != last {
            self.sparse[moved_slot as usize] = index as u32;
        }
        self.sparse[slot] = NONE;
    }

    fn refresh(&mut self) {
        for (raw, column) in self.raw_columns.iter_mut().zip(self.columns.iter_mut()) {
            *raw = Raw::of(column);
        }
    }

    #[cfg(test)]
    fn check_consistency(&self, alive: &[bool]) {
        for (index, &slot) in self.dense_slot.iter().enumerate() {
            assert_eq!(self.sparse[slot as usize], index as u32);
            assert!(alive[slot as usize]);
        }
        for column in &self.columns {
            assert_eq!(column.len(), self.dense_slot.len());
        }
        assert_eq!(self.sparse.iter().filter(|&&index| index != NONE).count(), self.dense_slot.len());
    }
}

struct Sparse {
    generation: Vec<u32>,
    alive: Vec<bool>,
    position: SparseSet<2>,
    velocity: SparseSet<2>,
    mass: SparseSet<1>,
    heat: SparseSet<1>,
}

impl Sparse {
    fn new(state: &State) -> Sparse {
        let n = state.len();
        let mut sparse = Sparse {
            generation: state.generation.clone(),
            alive: state.alive.clone(),
            position: SparseSet::new(n),
            velocity: SparseSet::new(n),
            mass: SparseSet::new(n),
            heat: SparseSet::new(n),
        };
        for slot in 0..n {
            if state.alive[slot] {
                sparse.insert(slot, (state.x[slot], state.y[slot], state.vx[slot], state.vy[slot], state.mass[slot]));
            }
        }
        sparse.prepare_passes();
        sparse
    }

    fn insert(&mut self, slot: usize, row: Row) {
        self.position.insert(slot, [row.0, row.1]);
        self.velocity.insert(slot, [row.2, row.3]);
        self.mass.insert(slot, [row.4]);
    }

    fn row(&self, slot: usize) -> Row {
        let p = self.position.index_of(slot);
        let v = self.velocity.index_of(slot);
        let m = self.mass.index_of(slot);
        (
            self.position.columns[0][p],
            self.position.columns[1][p],
            self.velocity.columns[0][v],
            self.velocity.columns[1][v],
            self.mass.columns[0][m],
        )
    }

    #[cfg(test)]
    fn check_consistency(&self) {
        self.position.check_consistency(&self.alive);
        self.velocity.check_consistency(&self.alive);
        self.mass.check_consistency(&self.alive);
        self.heat.check_consistency(&self.alive);
        assert_eq!(self.position.len(), self.alive.iter().filter(|&&alive| alive).count());
    }
}

impl Storage for Sparse {
    fn slot_count(&self) -> usize {
        self.alive.len()
    }

    fn alive(&self) -> &[bool] {
        &self.alive
    }

    fn generation(&self) -> &[u32] {
        &self.generation
    }

    fn grow(&mut self) -> usize {
        self.generation.push(0);
        self.alive.push(false);
        for sparse in [&mut self.position.sparse, &mut self.velocity.sparse, &mut self.mass.sparse, &mut self.heat.sparse] {
            sparse.push(NONE);
        }
        self.alive.len() - 1
    }

    fn main_chunk_count(&self) -> usize {
        self.position.len().div_ceil(CHUNK)
    }

    fn prepare_passes(&mut self) {
        self.position.refresh();
        self.velocity.refresh();
        self.mass.refresh();
        self.heat.refresh();
    }

    fn canonical_entries(&self, out: &mut Vec<Entry>) {
        for slot in 0..self.alive.len() {
            if self.alive[slot] {
                let p = self.position.index_of(slot);
                out.push(Entry { slot: slot as u32, x: self.position.columns[0][p], y: self.position.columns[1][p] });
            }
        }
    }

    unsafe fn repulsion_chunk(&self, chunk: usize, grid: &Grid) {
        let range = chunk_range(chunk, self.position.len());
        let [vx, vy] = self.velocity.raw_columns;
        for d in range {
            let slot = self.position.dense_slot[d] as usize;
            let (fx, fy) = repulsion_force(self.position.columns[0][d], self.position.columns[1][d], slot as u32, grid);
            let v = self.velocity.index_of(slot);
            let mass = self.mass.columns[0][self.mass.index_of(slot)];
            // SAFETY: the entity's velocity is written by the one task that iterates
            // its position row.
            unsafe {
                vx.write(v, repelled_velocity(vx.read(v), fx, mass));
                vy.write(v, repelled_velocity(vy.read(v), fy, mass));
            }
        }
    }

    unsafe fn motion_chunk(&self, chunk: usize, tick: u32, wander: bool, energy: &Raw<f32>, mut record: Option<&mut dyn FnMut(usize)>) {
        let range = chunk_range(chunk, self.position.len());
        let [vx, vy] = self.velocity.raw_columns;
        // SAFETY: this chunk of the position's dense array is written by this task only.
        let (x, y) = unsafe { (self.position.raw_columns[0].slice(range.clone()), self.position.raw_columns[1].slice(range.clone())) };
        for (k, d) in range.enumerate() {
            let slot = self.position.dense_slot[d] as usize;
            let v = self.velocity.index_of(slot);
            let mass = self.mass.columns[0][self.mass.index_of(slot)];
            // SAFETY: as in the repulsion pass; each slot's energy is written once.
            unsafe {
                let (nx, ny, nvx, nvy, e) = motion_slot(x[k], y[k], vx.read(v), vy.read(v), mass, tick, slot, wander);
                x[k] = nx;
                y[k] = ny;
                vx.write(v, nvx);
                vy.write(v, nvy);
                energy.write(slot, e);
            }
            if let Some(record) = record.as_mut() {
                record(slot);
            }
        }
    }

    fn despawn(&mut self, slot: usize) {
        self.alive[slot] = false;
        self.position.remove(slot);
        self.velocity.remove(slot);
        self.mass.remove(slot);
        if self.heat.contains(slot) {
            self.heat.remove(slot);
        }
    }

    fn spawn(&mut self, slot: usize, row: Row) {
        self.generation[slot] += 1;
        self.alive[slot] = true;
        self.insert(slot, row);
    }

    fn has_heat(&self, slot: usize) -> bool {
        self.heat.contains(slot)
    }

    fn add_heat(&mut self, slot: usize, heat: f32) {
        self.heat.insert(slot, [heat]);
    }

    fn remove_heat(&mut self, slot: usize) {
        self.heat.remove(slot);
    }

    fn hash(&self, globals: &Globals) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.alive.len() {
            let row = self.alive[slot].then(|| self.row(slot));
            hash_slot(&mut hasher, self.generation[slot], self.alive[slot], row);
        }
        hash_globals(&mut hasher, globals)
    }

    fn heat_hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        for slot in 0..self.alive.len() {
            if self.alive[slot] && self.heat.contains(slot) {
                hash_heat(&mut hasher, slot, self.heat.columns[0][self.heat.index_of(slot)]);
            }
        }
        hasher.finish()
    }
}

// ---------------------------------------------------------------------------
// The pool and the fixed point
// ---------------------------------------------------------------------------

const PASS_IDLE: u8 = 0;
const PASS_REPULSION: u8 = 1;
const PASS_MOTION: u8 = 2;
const PASS_STOP: u8 = 3;

struct Pool<S: Storage> {
    config: Config,
    parameters: Parameters,
    storage: UnsafeCell<S>,
    globals: UnsafeCell<Globals>,
    grid: UnsafeCell<Grid>,
    entries: UnsafeCell<Vec<Entry>>,
    energy: UnsafeCell<Vec<f32>>,
    raw_energy: UnsafeCell<Raw<f32>>,
    buffers: Vec<UnsafeCell<ChunkCommands>>,
    free: UnsafeCell<FreeList>,
    counts: UnsafeCell<Counts>,
    chunk_count: AtomicUsize,
    next_chunk: AtomicUsize,
    pass: AtomicU8,
    barrier: Barrier,
    tick: AtomicU32,
}

// SAFETY: the storage is read by every worker during a pass and written only
// through the chunk each task owns, each task records into its own chunk's
// buffer; between passes, the barrier makes the main thread the only thread
// touching anything.
unsafe impl<S: Storage> Sync for Pool<S> {}

impl<S: Storage> Pool<S> {
    fn new(parameters: &Parameters, config: Config, storage: S, rng: SplitMix64) -> Pool<S> {
        let n = storage.slot_count();
        // Room for the slot table to grow without reallocating during a run.
        let buffer_count = (2 * n).div_ceil(CHUNK) + 1;
        Pool {
            config,
            parameters: *parameters,
            storage: UnsafeCell::new(storage),
            globals: UnsafeCell::new(Globals { rng, energy: 0.0 }),
            grid: UnsafeCell::new(Grid::new()),
            entries: UnsafeCell::new(Vec::with_capacity(n)),
            energy: UnsafeCell::new(vec![0.0_f32; n]),
            raw_energy: UnsafeCell::new(Raw::empty()),
            buffers: (0..buffer_count).map(|_| UnsafeCell::new(ChunkCommands::default())).collect(),
            free: UnsafeCell::new(FreeList::new(config.policy)),
            counts: UnsafeCell::new(Counts::default()),
            chunk_count: AtomicUsize::new(0),
            next_chunk: AtomicUsize::new(0),
            pass: AtomicU8::new(PASS_IDLE),
            barrier: Barrier::new(config.workers),
            tick: AtomicU32::new(0),
        }
    }

    fn work(&self, pass: u8) {
        // SAFETY: shared reads during a pass; writes go through the owned chunks and
        // the owned buffers.
        let storage = unsafe { &*self.storage.get() };
        let grid = unsafe { &*self.grid.get() };
        let raw_energy = unsafe { *self.raw_energy.get() };
        let tick = self.tick.load(Ordering::Relaxed);
        let chunk_count = self.chunk_count.load(Ordering::Acquire);
        let slot_count = storage.slot_count();
        let rate = self.config.rate;
        loop {
            let chunk = self.next_chunk.fetch_add(1, Ordering::AcqRel);
            if chunk >= chunk_count {
                break;
            }
            // SAFETY: the counter hands every chunk to exactly one task.
            unsafe {
                match pass {
                    PASS_REPULSION => storage.repulsion_chunk(chunk, grid),
                    PASS_MOTION => {
                        if self.config.recording == Recording::Parallel {
                            let buffer = &mut *self.buffers[chunk].get();
                            let mut record = |slot: usize| {
                                if despawn_rule(slot, tick, rate, slot_count) {
                                    buffer.commands.push(Command::Despawn(slot as u32));
                                    let reserved = if buffer.reserved_used < buffer.reserved.len() {
                                        buffer.reserved_used += 1;
                                        buffer.reserved[buffer.reserved_used - 1]
                                    } else {
                                        NONE
                                    };
                                    buffer.commands.push(Command::Spawn { reserved, values: parallel_spawn_values(slot, tick) });
                                }
                                if toggle_rule(slot, tick, rate, slot_count) {
                                    buffer.commands.push(Command::Toggle(slot as u32));
                                }
                            };
                            storage.motion_chunk(chunk, tick, self.parameters.wander, &raw_energy, Some(&mut record));
                        } else {
                            storage.motion_chunk(chunk, tick, self.parameters.wander, &raw_energy, None);
                        }
                    }
                    _ => unreachable!("workers run the two passes only"),
                }
            }
        }
    }

    fn worker_loop(&self) {
        loop {
            self.barrier.wait();
            let pass = self.pass.load(Ordering::Acquire);
            if pass == PASS_STOP {
                return;
            }
            self.work(pass);
            self.barrier.wait();
        }
    }

    fn run_pass(&self, pass: u8, chunk_count: usize) {
        self.chunk_count.store(chunk_count, Ordering::Release);
        self.next_chunk.store(0, Ordering::Relaxed);
        self.pass.store(pass, Ordering::Release);
        self.barrier.wait();
        self.work(pass);
        self.barrier.wait();
    }

    fn stop(&self) {
        self.pass.store(PASS_STOP, Ordering::Release);
        self.barrier.wait();
    }

    /// Takes a free slot, growing the slot table when none is free.
    fn allocate(storage: &mut S, free: &mut FreeList, counts: &mut Counts) -> usize {
        free.take(storage.alive()).unwrap_or_else(|| {
            counts.grown_slots += 1;
            storage.grow()
        })
    }

    /// Reserves, in chunk order, the slots the chunks of the motion pass may
    /// spawn into: the expected number of despawns per chunk, plus one.
    fn reserve(&self, storage: &mut S, free: &mut FreeList, counts: &mut Counts, chunk_count: usize) {
        let slot_count = storage.slot_count();
        let per_chunk = (self.config.rate * CHUNK).div_ceil(slot_count) + 1;
        for chunk in 0..chunk_count {
            // SAFETY: between passes, the main thread is alone.
            let buffer = unsafe { &mut *self.buffers[chunk].get() };
            buffer.reserved.clear();
            buffer.reserved_used = 0;
            for _ in 0..per_chunk {
                let slot = Self::allocate(storage, free, counts);
                buffer.reserved.push(slot as u32);
            }
        }
    }

    /// The fixed point: the specified kills and spawns on the main thread, or
    /// the recorded commands applied in chunk order and record order, then
    /// the unused reservations returned in chunk order.
    fn apply(&self, storage: &mut S, globals: &mut Globals, free: &mut FreeList, counts: &mut Counts, tick: u32, chunk_count: usize) {
        match self.config.recording {
            Recording::MainThread => {
                let slot_count = storage.slot_count();
                for _ in 0..self.config.rate {
                    let slot = (globals.rng.next() % slot_count as u64) as usize;
                    if storage.alive()[slot] {
                        storage.despawn(slot);
                        free.free(slot);
                        counts.despawns += 1;
                    }
                }
                for _ in 0..self.config.rate {
                    // As specified: no dead slot, no spawn and no draw; the table never grows here.
                    let Some(slot) = free.take(storage.alive()) else { break };
                    let row = draw(&mut globals.rng);
                    storage.spawn(slot, row);
                    counts.spawns += 1;
                }
            }
            Recording::Parallel => {
                for chunk in 0..chunk_count {
                    // SAFETY: between passes, the main thread is alone.
                    let buffer = unsafe { &mut *self.buffers[chunk].get() };
                    for command in buffer.commands.drain(..) {
                        match command {
                            Command::Despawn(slot) => {
                                let slot = slot as usize;
                                if storage.alive()[slot] {
                                    storage.despawn(slot);
                                    free.free(slot);
                                    counts.despawns += 1;
                                }
                            }
                            Command::Spawn { reserved, values } => {
                                let slot = if reserved == NONE {
                                    counts.overflow_spawns += u64::from(self.config.allocation == Allocation::Reserved);
                                    Self::allocate(storage, free, counts)
                                } else {
                                    counts.reserved_spawns += 1;
                                    reserved as usize
                                };
                                storage.spawn(slot, values);
                                counts.spawns += 1;
                            }
                            Command::Toggle(slot) => {
                                let slot = slot as usize;
                                if storage.alive()[slot] {
                                    if storage.has_heat(slot) {
                                        storage.remove_heat(slot);
                                        counts.removes += 1;
                                    } else {
                                        storage.add_heat(slot, tick as f32);
                                        counts.adds += 1;
                                    }
                                }
                            }
                        }
                    }
                    for &slot in &buffer.reserved[buffer.reserved_used..] {
                        free.free(slot as usize);
                        counts.returned_reservations += 1;
                    }
                    buffer.reserved.clear();
                    buffer.reserved_used = 0;
                }
            }
        }
    }

    fn tick(&self, tick: u32, timing: &mut Timing) {
        self.tick.store(tick, Ordering::Relaxed);
        // SAFETY: between passes, the main thread is alone.
        let storage = unsafe { &mut *self.storage.get() };
        let globals = unsafe { &mut *self.globals.get() };
        let grid = unsafe { &mut *self.grid.get() };
        let entries = unsafe { &mut *self.entries.get() };
        let energy = unsafe { &mut *self.energy.get() };
        let free = unsafe { &mut *self.free.get() };
        let counts = unsafe { &mut *self.counts.get() };
        if self.config.recording == Recording::Parallel && self.config.allocation == Allocation::Reserved {
            // Reservations may grow the slot table; a chunk the growth adds gets none
            // and spawns through the overflow path.
            let reserved_chunks = storage.main_chunk_count();
            timing.charge("reservation", || self.reserve(storage, free, counts, reserved_chunks));
        }
        storage.prepare_passes();
        let chunk_count = storage.main_chunk_count();
        assert!(chunk_count <= self.buffers.len(), "the slot table outgrew the command buffers");
        energy.resize(storage.slot_count(), 0.0);
        // SAFETY: the main thread is alone; the raw view is refreshed before the passes.
        unsafe { *self.raw_energy.get() = Raw::of(energy) };
        timing.charge("canonical_traversal", || {
            entries.clear();
            storage.canonical_entries(entries);
        });
        timing.charge("spatial_hashing", || grid.build(entries));
        timing.charge("repulsion", || self.run_pass(PASS_REPULSION, chunk_count));
        timing.charge("motion", || self.run_pass(PASS_MOTION, chunk_count));
        timing.charge("energy", || {
            let alive = storage.alive();
            let n = alive.len();
            let mut total = 0.0_f32;
            for group in 0..n.div_ceil(CHUNK) {
                let mut sum = 0.0_f32;
                for i in chunk_range(group, n) {
                    if alive[i] {
                        sum += energy[i];
                    }
                }
                total += sum;
            }
            globals.energy = total;
        });
        timing.charge("apply", || self.apply(storage, globals, free, counts, tick, chunk_count));
    }

    fn hash(&self) -> u64 {
        // SAFETY: between passes, the main thread is alone.
        unsafe { (*self.storage.get()).hash(&*self.globals.get()) }
    }

    fn run(&self) -> Outcome {
        let tick_count = self.parameters.tick_count;
        let mut tick_hashes = Vec::with_capacity(tick_count as usize + 1);
        let mut timing = Timing::new(&PHASES);
        thread::scope(|scope| {
            let handles: Vec<_> = (1..self.config.workers).map(|_| scope.spawn(|| self.worker_loop())).collect();
            let started = Instant::now();
            tick_hashes.push(timing.charge("hash", || self.hash()));
            for t in 1..=tick_count {
                self.tick(t, &mut timing);
                tick_hashes.push(timing.charge("hash", || self.hash()));
            }
            timing.total = started.elapsed();
            self.stop();
            for handle in handles {
                handle.join().expect("a worker never panics");
            }
        });
        // SAFETY: the pool is quiescent.
        let storage = unsafe { &*self.storage.get() };
        let run_hash = run_hash(&tick_hashes);
        Outcome {
            run: Run { parameters: self.parameters, tick_hashes, run_hash, timing },
            heat_hash: storage.heat_hash(),
            counts: unsafe { *self.counts.get() },
            slot_count: storage.slot_count(),
            alive_count: storage.alive().iter().filter(|&&alive| alive).count(),
            max_generation: storage.generation().iter().copied().max().unwrap_or(0),
        }
    }
}

struct Outcome {
    run: Run,
    heat_hash: u64,
    counts: Counts,
    slot_count: usize,
    alive_count: usize,
    max_generation: u32,
}

fn run_once(parameters: &Parameters, config: Config) -> Outcome {
    let state = State::initialize(parameters);
    match config.layout {
        Layout::Slots => Pool::new(parameters, config, Slots::new(&state), state.rng).run(),
        Layout::Archetypes => Pool::new(parameters, config, Archetypes::new(&state), state.rng).run(),
        Layout::Sparse => Pool::new(parameters, config, Sparse::new(&state), state.rng).run(),
    }
}

fn measure(parameters: &Parameters, config: Config) -> Outcome {
    let warm_up = Parameters { tick_count: WARM_UP_TICKS, ..*parameters };
    run_once(&warm_up, config);
    run_once(parameters, config)
}

fn write_run(out: &mut Json, config: Config, outcome: &Outcome, reference: &Run) {
    out.object_start();
    out.string("layout", config.layout.name());
    out.string("recording", config.recording.name());
    out.string("allocation", config.allocation.name());
    out.string("policy", config.policy.name());
    out.u64("rate", config.rate as u64);
    out.u64("workers", config.workers as u64);
    out.bool("follows_the_specification", config.follows_the_specification());
    write_run_core(out, &outcome.run);
    let divergence = first_divergent_tick(&outcome.run.tick_hashes, &reference.tick_hashes);
    out.bool("matches_sequential", divergence.is_none());
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    out.hex("heat_hash", outcome.heat_hash);
    let counts = &outcome.counts;
    out.key("changes");
    out.object_start();
    out.u64("despawns", counts.despawns);
    out.u64("spawns", counts.spawns);
    out.u64("adds", counts.adds);
    out.u64("removes", counts.removes);
    out.u64("reserved_spawns", counts.reserved_spawns);
    out.u64("overflow_spawns", counts.overflow_spawns);
    out.u64("returned_reservations", counts.returned_reservations);
    out.u64("grown_slots", counts.grown_slots);
    out.u64("total", counts.changes());
    out.object_end();
    let apply_ms = outcome.run.timing.phases.iter().find(|(name, _)| *name == "apply").map_or(0, |(_, ns)| *ns) as f64 / 1_000_000.0;
    let reservation_ms = outcome.run.timing.phases.iter().find(|(name, _)| *name == "reservation").map_or(0, |(_, ns)| *ns) as f64 / 1_000_000.0;
    let changes = counts.changes().max(1) as f64;
    out.decimal("microseconds_per_change", (apply_ms + reservation_ms) * 1_000.0 / changes);
    out.decimal("apply_share_of_tick", (apply_ms + reservation_ms) / (outcome.run.timing.total.as_secs_f64() * 1_000.0));
    out.u64("final_slot_count", outcome.slot_count as u64);
    out.u64("final_alive_count", outcome.alive_count as u64);
    out.u64("max_generation", u64::from(outcome.max_generation));
    out.object_end();
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let parameters = Parameters::new(Size::Default, DEFAULT_TICK_COUNT, false);
    let reference = run_sequential(&parameters);

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.key("runs");
    out.array_start();
    for layout in Layout::ALL {
        for policy in [Policy::Scan, Policy::Heap, Policy::Stack] {
            for rate in RATES {
                for workers in WORKER_COUNTS {
                    let config = Config { layout, recording: Recording::MainThread, allocation: Allocation::AtApplication, policy, rate, workers };
                    write_run(&mut out, config, &measure(&parameters, config), &reference);
                }
            }
        }
        for allocation in [Allocation::AtApplication, Allocation::Reserved] {
            for policy in [Policy::Heap, Policy::Stack] {
                for rate in RATES {
                    for workers in WORKER_COUNTS {
                        let config = Config { layout, recording: Recording::Parallel, allocation, policy, rate, workers };
                        write_run(&mut out, config, &measure(&parameters, config), &reference);
                    }
                }
            }
        }
    }
    out.array_end();
    out.object_end();
    println!("{}", out.finish());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Parameters {
        Parameters::new(Size::Small, 40, false)
    }

    fn config(layout: Layout, recording: Recording, allocation: Allocation, policy: Policy, rate: usize, workers: usize) -> Config {
        Config { layout, recording, allocation, policy, rate, workers }
    }

    #[test]
    fn the_specified_configuration_reproduces_the_sequential_hashes_on_every_layout() {
        let reference = run_sequential(&small());
        for layout in Layout::ALL {
            for policy in [Policy::Scan, Policy::Heap] {
                for workers in [1, 3] {
                    let config = config(layout, Recording::MainThread, Allocation::AtApplication, policy, 8, workers);
                    assert!(config.follows_the_specification());
                    let outcome = run_once(&small(), config);
                    assert_eq!(first_divergent_tick(&outcome.run.tick_hashes, &reference.tick_hashes), None, "{config:?}");
                    assert_eq!(outcome.counts.adds + outcome.counts.removes, 0);
                    assert_eq!(outcome.counts.grown_slots, 0);
                    assert_eq!(outcome.slot_count, Size::Small.entity_count());
                }
            }
        }
    }

    #[test]
    fn the_stack_policy_and_the_higher_rates_diverge_from_the_specification_but_not_across_worker_counts() {
        let reference = run_sequential(&small());
        for layout in Layout::ALL {
            for (policy, rate) in [(Policy::Stack, 8), (Policy::Scan, 80), (Policy::Heap, 80)] {
                let one = run_once(&small(), config(layout, Recording::MainThread, Allocation::AtApplication, policy, rate, 1));
                let three = run_once(&small(), config(layout, Recording::MainThread, Allocation::AtApplication, policy, rate, 3));
                assert_eq!(one.run.run_hash, three.run.run_hash, "{layout:?} {policy:?} {rate}");
                assert_ne!(one.run.run_hash, reference.run_hash);
                assert!(one.counts.spawns >= 40 * rate as u64 / 2, "{:?}", one.counts);
            }
        }
    }

    #[test]
    fn the_main_thread_recording_gives_one_hash_across_layouts() {
        for (policy, rate) in [(Policy::Heap, 80), (Policy::Stack, 800)] {
            let hashes: Vec<u64> = Layout::ALL
                .iter()
                .map(|&layout| run_once(&small(), config(layout, Recording::MainThread, Allocation::AtApplication, policy, rate, 3)).run.run_hash)
                .collect();
            assert!(hashes.iter().all(|&h| h == hashes[0]), "{policy:?} {rate}: {hashes:x?}");
        }
    }

    #[test]
    fn the_parallel_recording_is_independent_of_the_worker_count() {
        for layout in Layout::ALL {
            for allocation in [Allocation::AtApplication, Allocation::Reserved] {
                for policy in [Policy::Heap, Policy::Stack] {
                    let rate = 80;
                    let outcomes: Vec<Outcome> = [1, 2, 3, 8]
                        .iter()
                        .map(|&workers| run_once(&small(), config(layout, Recording::Parallel, allocation, policy, rate, workers)))
                        .collect();
                    for outcome in &outcomes[1..] {
                        assert_eq!(outcome.run.run_hash, outcomes[0].run.run_hash, "{layout:?} {allocation:?} {policy:?}");
                        assert_eq!(outcome.heat_hash, outcomes[0].heat_hash);
                        assert_eq!(outcome.slot_count, outcomes[0].slot_count);
                    }
                    let counts = outcomes[0].counts;
                    assert!(counts.despawns > 0 && counts.spawns == counts.despawns, "{counts:?}");
                    assert!(counts.adds > 0 && counts.removes > 0, "{counts:?}");
                    if allocation == Allocation::Reserved {
                        assert!(counts.reserved_spawns > 0, "{counts:?}");
                        assert!(counts.returned_reservations > 0, "{counts:?}");
                    } else {
                        assert_eq!(counts.reserved_spawns + counts.returned_reservations, 0);
                    }
                    assert_eq!(outcomes[0].alive_count, Size::Small.entity_count(), "the population is constant");
                }
            }
        }
    }

    #[test]
    fn the_layouts_stay_consistent_under_parallel_structural_changes() {
        let parameters = small();
        let state = State::initialize(&parameters);
        for allocation in [Allocation::AtApplication, Allocation::Reserved] {
            let config = config(Layout::Archetypes, Recording::Parallel, allocation, Policy::Stack, 80, 3);
            let pool = Pool::new(&parameters, config, Archetypes::new(&state), state.rng);
            let mut timing = Timing::new(&PHASES);
            thread::scope(|scope| {
                let handles: Vec<_> = (1..3).map(|_| scope.spawn(|| pool.worker_loop())).collect();
                for t in 1..=30 {
                    pool.tick(t, &mut timing);
                    // SAFETY: between passes, the test is the main thread.
                    unsafe { &*pool.storage.get() }.check_consistency();
                }
                pool.stop();
                for handle in handles {
                    handle.join().unwrap();
                }
            });
            let config = config_with(Layout::Sparse, config);
            let pool = Pool::new(&parameters, config, Sparse::new(&state), state.rng);
            let mut timing = Timing::new(&PHASES);
            thread::scope(|scope| {
                let handles: Vec<_> = (1..3).map(|_| scope.spawn(|| pool.worker_loop())).collect();
                for t in 1..=30 {
                    pool.tick(t, &mut timing);
                    // SAFETY: between passes, the test is the main thread.
                    unsafe { &*pool.storage.get() }.check_consistency();
                }
                pool.stop();
                for handle in handles {
                    handle.join().unwrap();
                }
            });
        }
    }

    fn config_with(layout: Layout, config: Config) -> Config {
        Config { layout, ..config }
    }

    #[test]
    fn the_free_list_policies_pick_the_slots_they_promise() {
        let alive = vec![true, false, true, false];
        let mut scan = FreeList::new(Policy::Scan);
        assert_eq!(scan.take(&alive), Some(1));
        let mut heap = FreeList::new(Policy::Heap);
        heap.free(3);
        heap.free(1);
        assert_eq!(heap.take(&alive), Some(1));
        assert_eq!(heap.take(&alive), Some(3));
        assert_eq!(heap.take(&alive), None);
        let mut stack = FreeList::new(Policy::Stack);
        stack.free(3);
        stack.free(1);
        assert_eq!(stack.take(&alive), Some(1));
        assert_eq!(stack.take(&alive), Some(3));
        assert_eq!(stack.take(&alive), None);
    }
}
