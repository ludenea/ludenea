//! Storage layouts of the reference workload for the ECS research.
//!
//! Throwaway spike, never product code. It runs the reference workload
//! (default size, wander system off) over three storage layouts and compares
//! their tick hashes with the sequential reference and their throughput with
//! one another:
//!
//! - `slots`: identifier-indexed slots, the layout of the reference
//!   implementation: one row per slot, dead slots included, the canonical order
//!   is the storage order;
//! - `archetypes`: a slot table (generation, liveness, location) plus one
//!   table per archetype cut into chunks of 256 rows, each chunk a structure
//!   of arrays; rows are appended on spawn and swap-removed on despawn, so the
//!   row order is a function of the command history; canonical traversals walk
//!   the slot table and read each row through its location;
//! - `sparse`: one sparse set per component (position, velocity, mass, heat),
//!   each a dense array plus a slot-indexed index; a query iterates the dense
//!   array of its first component and looks the others up per entity.
//!
//! The systems run on a barrier pool (the pool of the worker sweep spike with
//! the passes writing in place) over the storage chunks of each layout. The
//! spatial grid stores the position next to each slot, so that the repulsion
//! reads its neighbors from the grid and never through the layout: the grid is
//! a derived structure, built from a canonical traversal in slot order, which
//! keeps the visit order of the specification whatever the layout.
//!
//! Two variants: a second archetype, introduced by a spike-local `heat`
//! component that the even slots carry and that one extra system reads and
//! writes from the velocity without touching the specified state (the
//! reference hashes are unchanged, and a second hash over the heat values must
//! agree across layouts and worker counts); and change detection, per chunk
//! (one store per written column per chunk) or per entity (one store per
//! written component per entity).

use std::cell::UnsafeCell;
use std::ops::Range;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::Barrier;
use std::thread;
use std::time::Instant;

use reference_workload::{
    CHUNK, DEFAULT_TICK_COUNT, Fnv1a, GRID_SIDE, INTERACTION_RADIUS, Json, KILLS_PER_TICK,
    Parameters, REPULSION_STRENGTH, Run, SPAWNS_PER_TICK, Size, SplitMix64, State, Timing,
    WARM_UP_TICKS, WORLD_SIDE, bounced, chunk_range, first_divergent_tick, integrated,
    kinetic_energy, repelled_velocity, run_hash, run_sequential, wandered_velocity,
    write_platform_and_build, write_run_core,
};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "workload-storage-layouts";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
const VARIANT_WORKERS: usize = 4;
const HEAT_DECAY: f32 = 0.996_093_75; // 1 - 2^-8
const HEAT_GAIN: f32 = 0.000_976_562_5; // 2^-10
const NONE: u32 = u32::MAX;
const PHASES: [&str; 7] = [
    "canonical_traversal",
    "spatial_hashing",
    "repulsion",
    "motion",
    "cooling",
    "structural_changes",
    "hash",
];

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
enum Mode {
    /// The workload as specified: every entity has position, velocity and mass.
    OneArchetype,
    /// The even slots also carry `heat`, read and written by the cooling system.
    TwoArchetypes,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::OneArchetype => "one-archetype",
            Mode::TwoArchetypes => "two-archetypes",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChangeDetection {
    None,
    PerChunk,
    PerEntity,
}

impl ChangeDetection {
    fn name(self) -> &'static str {
        match self {
            ChangeDetection::None => "none",
            ChangeDetection::PerChunk => "per-chunk",
            ChangeDetection::PerEntity => "per-entity",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Config {
    layout: Layout,
    mode: Mode,
    change_detection: ChangeDetection,
    workers: usize,
}

fn has_heat(slot: usize, mode: Mode) -> bool {
    mode == Mode::TwoArchetypes && slot % 2 == 0
}

/// The spike-local cooling system for one entity.
#[inline]
fn cooled(heat: f32, vx: f32, vy: f32) -> f32 {
    (heat * HEAT_DECAY) + (((vx * vx) + (vy * vy)) * HEAT_GAIN)
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

/// The draws of a spawn or of the initialization, in the specified order.
fn draw(rng: &mut SplitMix64) -> (f32, f32, f32, f32, f32) {
    let x = rng.u01() * WORLD_SIDE;
    let y = rng.u01() * WORLD_SIDE;
    let vx = ((rng.u01() * 2.0) - 1.0) * 32.0;
    let vy = ((rng.u01() * 2.0) - 1.0) * 32.0;
    let mass = rng.mass();
    (x, y, vx, vy, mass)
}

/// The two globals of the workload, kept outside the layouts.
#[derive(Clone, Copy)]
struct Globals {
    rng: SplitMix64,
    energy: f32,
}

/// The hash of one slot, in the specified byte layout.
fn hash_slot(hasher: &mut Fnv1a, generation: u32, alive: bool, row: Option<(f32, f32, f32, f32, f32)>) {
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
// Raw column views and the spatial grid
// ---------------------------------------------------------------------------

/// A raw view of one column, refreshed by the main thread before the passes
/// (a layout may reallocate its columns at the fixed point). Tasks write
/// disjoint slots of it without forming a reference to the whole column: the
/// one `unsafe` construct of the spike.
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

/// One alive entity in slot order: what the spatial grid and the hash need.
#[derive(Clone, Copy, Default)]
struct Entry {
    slot: u32,
    x: f32,
    y: f32,
}

/// System 1 as a counting sort of slot-ordered entries, so that each cell
/// lists its entities in increasing slot order with their positions.
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

// ---------------------------------------------------------------------------
// The layouts
// ---------------------------------------------------------------------------

/// What the pool needs from a layout. Tasks run one storage chunk each; the
/// main thread owns the storage between passes.
trait Storage: Sync {
    fn entity_count(&self) -> usize;
    fn alive(&self) -> &[bool];
    /// The number of storage chunks the main systems iterate.
    fn main_chunk_count(&self) -> usize;
    /// The number of storage chunks the cooling system iterates.
    fn cooling_chunk_count(&self) -> usize;
    /// Refreshes the raw views before the passes of a tick.
    fn prepare_passes(&mut self);
    /// Appends the alive entities in slot order: the canonical traversal.
    fn canonical_entries(&self, out: &mut Vec<Entry>);
    /// # Safety
    ///
    /// One task per chunk per pass; the chunk's rows are written by that task only.
    unsafe fn repulsion_chunk(&self, chunk: usize, grid: &Grid, tick: u32, detection: ChangeDetection);
    /// # Safety
    ///
    /// As above; `energy` receives one value per slot.
    unsafe fn motion_chunk(&self, chunk: usize, tick: u32, wander: bool, detection: ChangeDetection, energy: &Raw<f32>);
    /// # Safety
    ///
    /// As above, over the chunks of the entities carrying heat.
    unsafe fn cooling_chunk(&self, chunk: usize);
    fn structural_changes(&mut self, globals: &mut Globals, mode: Mode);
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
    changed_position: Vec<u32>,
    changed_velocity: Vec<u32>,
    /// Per chunk: the tick of the last write of the position and velocity columns.
    chunk_changed: Vec<[u32; 2]>,
    raw: SlotsRaw,
}

#[derive(Clone, Copy)]
struct SlotsRaw {
    x: Raw<f32>,
    y: Raw<f32>,
    vx: Raw<f32>,
    vy: Raw<f32>,
    heat: Raw<f32>,
    changed_position: Raw<u32>,
    changed_velocity: Raw<u32>,
    chunk_changed: Raw<[u32; 2]>,
}

impl Slots {
    fn new(state: &State, mode: Mode) -> Slots {
        let n = state.len();
        let mut slots = Slots {
            generation: state.generation.clone(),
            alive: state.alive.clone(),
            x: state.x.clone(),
            y: state.y.clone(),
            vx: state.vx.clone(),
            vy: state.vy.clone(),
            mass: state.mass.clone(),
            with_heat: (0..n).map(|slot| has_heat(slot, mode)).collect(),
            heat: vec![0.0; n],
            changed_position: vec![0; n],
            changed_velocity: vec![0; n],
            chunk_changed: vec![[0; 2]; n.div_ceil(CHUNK)],
            raw: SlotsRaw {
                x: Raw::empty(),
                y: Raw::empty(),
                vx: Raw::empty(),
                vy: Raw::empty(),
                heat: Raw::empty(),
                changed_position: Raw::empty(),
                changed_velocity: Raw::empty(),
                chunk_changed: Raw::empty(),
            },
        };
        slots.prepare_passes();
        slots
    }
}

impl Storage for Slots {
    fn entity_count(&self) -> usize {
        self.alive.len()
    }

    fn alive(&self) -> &[bool] {
        &self.alive
    }

    fn main_chunk_count(&self) -> usize {
        self.alive.len().div_ceil(CHUNK)
    }

    fn cooling_chunk_count(&self) -> usize {
        self.main_chunk_count()
    }

    fn prepare_passes(&mut self) {
        self.raw = SlotsRaw {
            x: Raw::of(&mut self.x),
            y: Raw::of(&mut self.y),
            vx: Raw::of(&mut self.vx),
            vy: Raw::of(&mut self.vy),
            heat: Raw::of(&mut self.heat),
            changed_position: Raw::of(&mut self.changed_position),
            changed_velocity: Raw::of(&mut self.changed_velocity),
            chunk_changed: Raw::of(&mut self.chunk_changed),
        };
    }

    fn canonical_entries(&self, out: &mut Vec<Entry>) {
        for slot in 0..self.alive.len() {
            if self.alive[slot] {
                out.push(Entry { slot: slot as u32, x: self.x[slot], y: self.y[slot] });
            }
        }
    }

    unsafe fn repulsion_chunk(&self, chunk: usize, grid: &Grid, tick: u32, detection: ChangeDetection) {
        let range = chunk_range(chunk, self.alive.len());
        // SAFETY: this chunk's slots of the velocity columns are written by this task only.
        let (vx, vy) = unsafe { (self.raw.vx.slice(range.clone()), self.raw.vy.slice(range.clone())) };
        for (k, i) in range.enumerate() {
            if self.alive[i] {
                let (fx, fy) = repulsion_force(self.x[i], self.y[i], i as u32, grid);
                vx[k] = repelled_velocity(vx[k], fx, self.mass[i]);
                vy[k] = repelled_velocity(vy[k], fy, self.mass[i]);
                if detection == ChangeDetection::PerEntity {
                    // SAFETY: own slot.
                    unsafe { self.raw.changed_velocity.write(i, tick) };
                }
            }
        }
        if detection == ChangeDetection::PerChunk {
            // SAFETY: own chunk.
            unsafe {
                let mut changed = self.raw.chunk_changed.read(chunk);
                changed[1] = tick;
                self.raw.chunk_changed.write(chunk, changed);
            }
        }
    }

    unsafe fn motion_chunk(&self, chunk: usize, tick: u32, wander: bool, detection: ChangeDetection, energy: &Raw<f32>) {
        let range = chunk_range(chunk, self.alive.len());
        // SAFETY: this chunk's slots of the four columns and of the energy scratch are
        // read and written by this task only.
        let (x, y, vx, vy, e) = unsafe {
            (
                self.raw.x.slice(range.clone()),
                self.raw.y.slice(range.clone()),
                self.raw.vx.slice(range.clone()),
                self.raw.vy.slice(range.clone()),
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
                if detection == ChangeDetection::PerEntity {
                    // SAFETY: own slot.
                    unsafe {
                        self.raw.changed_position.write(i, tick);
                        self.raw.changed_velocity.write(i, tick);
                    }
                }
            }
        }
        if detection == ChangeDetection::PerChunk {
            // SAFETY: own chunk.
            unsafe { self.raw.chunk_changed.write(chunk, [tick, tick]) };
        }
    }

    unsafe fn cooling_chunk(&self, chunk: usize) {
        let range = chunk_range(chunk, self.alive.len());
        // SAFETY: this chunk's slots of the heat column are written by this task only.
        let heat = unsafe { self.raw.heat.slice(range.clone()) };
        for (k, i) in range.enumerate() {
            if self.alive[i] && self.with_heat[i] {
                heat[k] = cooled(heat[k], self.vx[i], self.vy[i]);
            }
        }
    }

    fn structural_changes(&mut self, globals: &mut Globals, mode: Mode) {
        let n = self.alive.len();
        for _ in 0..KILLS_PER_TICK {
            let slot = (globals.rng.next() % n as u64) as usize;
            self.alive[slot] = false;
        }
        for _ in 0..SPAWNS_PER_TICK {
            let Some(slot) = self.alive.iter().position(|alive| !alive) else { return };
            let (x, y, vx, vy, mass) = draw(&mut globals.rng);
            self.x[slot] = x;
            self.y[slot] = y;
            self.vx[slot] = vx;
            self.vy[slot] = vy;
            self.mass[slot] = mass;
            self.generation[slot] += 1;
            self.alive[slot] = true;
            self.with_heat[slot] = has_heat(slot, mode);
            self.heat[slot] = 0.0;
        }
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

/// One chunk of an archetype table: a structure of arrays of up to `CHUNK`
/// rows, with the slot of each row.
struct Chunk {
    slot: Vec<u32>,
    x: Vec<f32>,
    y: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    mass: Vec<f32>,
    heat: Vec<f32>,
    changed_position: Vec<u32>,
    changed_velocity: Vec<u32>,
    column_changed: [u32; 2],
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
            changed_position: Vec::with_capacity(CHUNK),
            changed_velocity: Vec::with_capacity(CHUNK),
            column_changed: [0; 2],
        }
    }

    fn len(&self) -> usize {
        self.slot.len()
    }

    fn push(&mut self, slot: u32, row: (f32, f32, f32, f32, f32), heat: Option<f32>) {
        self.slot.push(slot);
        self.x.push(row.0);
        self.y.push(row.1);
        self.vx.push(row.2);
        self.vy.push(row.3);
        self.mass.push(row.4);
        if let Some(heat) = heat {
            self.heat.push(heat);
        }
        self.changed_position.push(0);
        self.changed_velocity.push(0);
    }

    fn row(&self, row: usize) -> (f32, f32, f32, f32, f32) {
        (self.x[row], self.y[row], self.vx[row], self.vy[row], self.mass[row])
    }

    fn pop(&mut self) -> (u32, (f32, f32, f32, f32, f32), Option<f32>) {
        let slot = self.slot.pop().expect("pop from a non-empty chunk");
        let row = (
            self.x.pop().unwrap(),
            self.y.pop().unwrap(),
            self.vx.pop().unwrap(),
            self.vy.pop().unwrap(),
            self.mass.pop().unwrap(),
        );
        let heat = self.heat.pop();
        self.changed_position.pop();
        self.changed_velocity.pop();
        (slot, row, heat)
    }

    fn overwrite(&mut self, row: usize, slot: u32, values: (f32, f32, f32, f32, f32), heat: Option<f32>) {
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

/// One archetype table: dense chunks, the last one possibly partial.
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

    /// Appends a row and returns its location within the table.
    fn insert(&mut self, slot: u32, row: (f32, f32, f32, f32, f32), heat: Option<f32>) -> (u32, u32) {
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
    /// the slot that moved, if any, so that the caller updates its location.
    fn swap_remove(&mut self, chunk: u32, row: u32) -> Option<u32> {
        let last_chunk = self.chunks.len() - 1;
        let (moved_slot, moved_row, moved_heat) = self.chunk_mut(last_chunk).pop();
        let moved = if (chunk as usize, row as usize) == (last_chunk, self.chunk(last_chunk).len()) {
            None
        } else {
            self.chunk_mut(chunk as usize).overwrite(row as usize, moved_slot, moved_row, moved_heat);
            Some(moved_slot)
        };
        if self.chunk(last_chunk).len() == 0 {
            let empty = self.chunks.pop().expect("the last chunk exists").into_inner();
            self.spare.push(empty);
        }
        moved
    }
}

struct Archetypes {
    generation: Vec<u32>,
    alive: Vec<bool>,
    location: Vec<Location>,
    tables: [Table; 2],
    /// The storage chunks the main systems iterate, in archetype then chunk order.
    main_chunks: Vec<(u8, u32)>,
    /// The storage chunks of the heat archetype.
    cooling_chunks: Vec<(u8, u32)>,
}

impl Archetypes {
    fn new(state: &State, mode: Mode) -> Archetypes {
        let n = state.len();
        let mut archetypes = Archetypes {
            generation: state.generation.clone(),
            alive: state.alive.clone(),
            location: vec![Location::default(); n],
            tables: [Table::new(false), Table::new(true)],
            main_chunks: Vec::new(),
            cooling_chunks: Vec::new(),
        };
        for slot in 0..n {
            if state.alive[slot] {
                let row = (state.x[slot], state.y[slot], state.vx[slot], state.vy[slot], state.mass[slot]);
                archetypes.insert(slot, row, mode);
            }
        }
        archetypes.rebuild_chunk_lists();
        archetypes
    }

    fn insert(&mut self, slot: usize, row: (f32, f32, f32, f32, f32), mode: Mode) {
        let archetype = u8::from(has_heat(slot, mode));
        let heat = (archetype == 1).then_some(0.0);
        let (chunk, index) = self.tables[archetype as usize].insert(slot as u32, row, heat);
        self.location[slot] = Location { archetype, chunk, row: index };
    }

    fn remove(&mut self, slot: usize) {
        let location = self.location[slot];
        if let Some(moved) = self.tables[location.archetype as usize].swap_remove(location.chunk, location.row) {
            self.location[moved as usize] = location;
        }
    }

    fn rebuild_chunk_lists(&mut self) {
        self.main_chunks.clear();
        self.cooling_chunks.clear();
        for (archetype, table) in self.tables.iter().enumerate() {
            for chunk in 0..table.chunks.len() {
                self.main_chunks.push((archetype as u8, chunk as u32));
                if table.with_heat {
                    self.cooling_chunks.push((archetype as u8, chunk as u32));
                }
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
            for (index, cell) in table.chunks.iter().enumerate() {
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
                let _ = cell;
            }
        }
        assert_eq!(rows, self.alive.iter().filter(|&&alive| alive).count());
    }
}

impl Storage for Archetypes {
    fn entity_count(&self) -> usize {
        self.alive.len()
    }

    fn alive(&self) -> &[bool] {
        &self.alive
    }

    fn main_chunk_count(&self) -> usize {
        self.main_chunks.len()
    }

    fn cooling_chunk_count(&self) -> usize {
        self.cooling_chunks.len()
    }

    fn prepare_passes(&mut self) {}

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

    unsafe fn repulsion_chunk(&self, task: usize, grid: &Grid, tick: u32, detection: ChangeDetection) {
        // SAFETY: one task per chunk per pass.
        let chunk = unsafe { self.owned_chunk(self.main_chunks[task]) };
        for row in 0..chunk.len() {
            let slot = chunk.slot[row];
            let (fx, fy) = repulsion_force(chunk.x[row], chunk.y[row], slot, grid);
            chunk.vx[row] = repelled_velocity(chunk.vx[row], fx, chunk.mass[row]);
            chunk.vy[row] = repelled_velocity(chunk.vy[row], fy, chunk.mass[row]);
            if detection == ChangeDetection::PerEntity {
                chunk.changed_velocity[row] = tick;
            }
        }
        if detection == ChangeDetection::PerChunk {
            chunk.column_changed[1] = tick;
        }
    }

    unsafe fn motion_chunk(&self, task: usize, tick: u32, wander: bool, detection: ChangeDetection, energy: &Raw<f32>) {
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
            if detection == ChangeDetection::PerEntity {
                chunk.changed_position[row] = tick;
                chunk.changed_velocity[row] = tick;
            }
        }
        if detection == ChangeDetection::PerChunk {
            chunk.column_changed = [tick, tick];
        }
    }

    unsafe fn cooling_chunk(&self, task: usize) {
        // SAFETY: one task per chunk per pass.
        let chunk = unsafe { self.owned_chunk(self.cooling_chunks[task]) };
        for row in 0..chunk.len() {
            chunk.heat[row] = cooled(chunk.heat[row], chunk.vx[row], chunk.vy[row]);
        }
    }

    fn structural_changes(&mut self, globals: &mut Globals, mode: Mode) {
        let n = self.alive.len();
        for _ in 0..KILLS_PER_TICK {
            let slot = (globals.rng.next() % n as u64) as usize;
            if self.alive[slot] {
                self.alive[slot] = false;
                self.remove(slot);
            }
        }
        for _ in 0..SPAWNS_PER_TICK {
            let Some(slot) = self.alive.iter().position(|alive| !alive) else { break };
            let row = draw(&mut globals.rng);
            self.generation[slot] += 1;
            self.alive[slot] = true;
            self.insert(slot, row, mode);
        }
        self.rebuild_chunk_lists();
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

/// A sparse set of one component with `COLUMNS` `f32` fields: a dense array
/// per field, the slot of each dense index, and a slot-indexed index.
struct SparseSet<const COLUMNS: usize> {
    sparse: Vec<u32>,
    dense_slot: Vec<u32>,
    columns: [Vec<f32>; COLUMNS],
    changed: Vec<u32>,
    raw_columns: [Raw<f32>; COLUMNS],
    raw_changed: Raw<u32>,
}

impl<const COLUMNS: usize> SparseSet<COLUMNS> {
    fn new(entity_count: usize) -> SparseSet<COLUMNS> {
        SparseSet {
            sparse: vec![NONE; entity_count],
            dense_slot: Vec::new(),
            columns: std::array::from_fn(|_| Vec::new()),
            changed: Vec::new(),
            raw_columns: [Raw::empty(); COLUMNS],
            raw_changed: Raw::empty(),
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
        self.changed.push(0);
    }

    fn remove(&mut self, slot: usize) {
        let index = self.index_of(slot);
        let last = self.dense_slot.len() - 1;
        let moved_slot = self.dense_slot[last];
        self.dense_slot.swap_remove(index);
        for column in &mut self.columns {
            column.swap_remove(index);
        }
        self.changed.swap_remove(index);
        if index != last {
            self.sparse[moved_slot as usize] = index as u32;
        }
        self.sparse[slot] = NONE;
    }

    fn refresh(&mut self) {
        for (raw, column) in self.raw_columns.iter_mut().zip(self.columns.iter_mut()) {
            *raw = Raw::of(column);
        }
        self.raw_changed = Raw::of(&mut self.changed);
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
    /// Per chunk of the position's dense array: the tick of the last write of
    /// the position and velocity columns.
    chunk_changed: Vec<[u32; 2]>,
    raw_chunk_changed: Raw<[u32; 2]>,
}

impl Sparse {
    fn new(state: &State, mode: Mode) -> Sparse {
        let n = state.len();
        let mut sparse = Sparse {
            generation: state.generation.clone(),
            alive: state.alive.clone(),
            position: SparseSet::new(n),
            velocity: SparseSet::new(n),
            mass: SparseSet::new(n),
            heat: SparseSet::new(n),
            chunk_changed: vec![[0; 2]; n.div_ceil(CHUNK)],
            raw_chunk_changed: Raw::empty(),
        };
        for slot in 0..n {
            if state.alive[slot] {
                let row = (state.x[slot], state.y[slot], state.vx[slot], state.vy[slot], state.mass[slot]);
                sparse.insert(slot, row, mode);
            }
        }
        sparse.prepare_passes();
        sparse
    }

    fn insert(&mut self, slot: usize, row: (f32, f32, f32, f32, f32), mode: Mode) {
        self.position.insert(slot, [row.0, row.1]);
        self.velocity.insert(slot, [row.2, row.3]);
        self.mass.insert(slot, [row.4]);
        if has_heat(slot, mode) {
            self.heat.insert(slot, [0.0]);
        }
    }

    fn remove(&mut self, slot: usize) {
        self.position.remove(slot);
        self.velocity.remove(slot);
        self.mass.remove(slot);
        if self.heat.contains(slot) {
            self.heat.remove(slot);
        }
    }

    fn row(&self, slot: usize) -> (f32, f32, f32, f32, f32) {
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
    fn entity_count(&self) -> usize {
        self.alive.len()
    }

    fn alive(&self) -> &[bool] {
        &self.alive
    }

    fn main_chunk_count(&self) -> usize {
        self.position.len().div_ceil(CHUNK)
    }

    fn cooling_chunk_count(&self) -> usize {
        self.heat.len().div_ceil(CHUNK)
    }

    fn prepare_passes(&mut self) {
        self.position.refresh();
        self.velocity.refresh();
        self.mass.refresh();
        self.heat.refresh();
        self.chunk_changed.resize(self.main_chunk_count().max(self.chunk_changed.len()), [0; 2]);
        self.raw_chunk_changed = Raw::of(&mut self.chunk_changed);
    }

    fn canonical_entries(&self, out: &mut Vec<Entry>) {
        for slot in 0..self.alive.len() {
            if self.alive[slot] {
                let p = self.position.index_of(slot);
                out.push(Entry { slot: slot as u32, x: self.position.columns[0][p], y: self.position.columns[1][p] });
            }
        }
    }

    unsafe fn repulsion_chunk(&self, chunk: usize, grid: &Grid, tick: u32, detection: ChangeDetection) {
        let range = chunk_range(chunk, self.position.len());
        let [vx, vy] = self.velocity.raw_columns;
        for d in range {
            let slot = self.position.dense_slot[d] as usize;
            let xi = self.position.columns[0][d];
            let yi = self.position.columns[1][d];
            let (fx, fy) = repulsion_force(xi, yi, slot as u32, grid);
            let v = self.velocity.index_of(slot);
            let mass = self.mass.columns[0][self.mass.index_of(slot)];
            // SAFETY: the entity's velocity is written by the one task that iterates
            // its position row.
            unsafe {
                vx.write(v, repelled_velocity(vx.read(v), fx, mass));
                vy.write(v, repelled_velocity(vy.read(v), fy, mass));
                if detection == ChangeDetection::PerEntity {
                    self.velocity.raw_changed.write(v, tick);
                }
            }
        }
        if detection == ChangeDetection::PerChunk {
            // SAFETY: own chunk.
            unsafe {
                let mut changed = self.raw_chunk_changed.read(chunk);
                changed[1] = tick;
                self.raw_chunk_changed.write(chunk, changed);
            }
        }
    }

    unsafe fn motion_chunk(&self, chunk: usize, tick: u32, wander: bool, detection: ChangeDetection, energy: &Raw<f32>) {
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
                if detection == ChangeDetection::PerEntity {
                    self.position.raw_changed.write(d, tick);
                    self.velocity.raw_changed.write(v, tick);
                }
            }
        }
        if detection == ChangeDetection::PerChunk {
            // SAFETY: own chunk.
            unsafe { self.raw_chunk_changed.write(chunk, [tick, tick]) };
        }
    }

    unsafe fn cooling_chunk(&self, chunk: usize) {
        let range = chunk_range(chunk, self.heat.len());
        // SAFETY: this chunk of the heat's dense array is written by this task only.
        let heat = unsafe { self.heat.raw_columns[0].slice(range.clone()) };
        for (k, d) in range.enumerate() {
            let slot = self.heat.dense_slot[d] as usize;
            let v = self.velocity.index_of(slot);
            heat[k] = cooled(heat[k], self.velocity.columns[0][v], self.velocity.columns[1][v]);
        }
    }

    fn structural_changes(&mut self, globals: &mut Globals, mode: Mode) {
        let n = self.alive.len();
        for _ in 0..KILLS_PER_TICK {
            let slot = (globals.rng.next() % n as u64) as usize;
            if self.alive[slot] {
                self.alive[slot] = false;
                self.remove(slot);
            }
        }
        for _ in 0..SPAWNS_PER_TICK {
            let Some(slot) = self.alive.iter().position(|alive| !alive) else { break };
            let row = draw(&mut globals.rng);
            self.generation[slot] += 1;
            self.alive[slot] = true;
            self.insert(slot, row, mode);
        }
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
// The pool
// ---------------------------------------------------------------------------

const PASS_IDLE: u8 = 0;
const PASS_REPULSION: u8 = 1;
const PASS_MOTION: u8 = 2;
const PASS_COOLING: u8 = 3;
const PASS_STOP: u8 = 4;

/// The barrier pool of the worker sweep spike, the passes writing in place.
struct Pool<S: Storage> {
    config: Config,
    parameters: Parameters,
    storage: UnsafeCell<S>,
    globals: UnsafeCell<Globals>,
    grid: UnsafeCell<Grid>,
    entries: UnsafeCell<Vec<Entry>>,
    _energy: UnsafeCell<Vec<f32>>,
    energy: Raw<f32>,
    chunk_count: AtomicUsize,
    next_chunk: AtomicUsize,
    pass: AtomicU8,
    barrier: Barrier,
    tick: AtomicU32,
}

// SAFETY: the storage is read by every worker during a pass and written only
// through the chunk each task owns; between passes, the barrier makes the
// main thread the only thread touching anything.
unsafe impl<S: Storage> Sync for Pool<S> {}

impl<S: Storage> Pool<S> {
    fn new(parameters: &Parameters, config: Config, storage: S, rng: SplitMix64) -> Pool<S> {
        let n = storage.entity_count();
        let mut energy = vec![0.0_f32; n];
        let raw_energy = Raw::of(&mut energy);
        Pool {
            config,
            parameters: *parameters,
            storage: UnsafeCell::new(storage),
            globals: UnsafeCell::new(Globals { rng, energy: 0.0 }),
            grid: UnsafeCell::new(Grid::new()),
            entries: UnsafeCell::new(Vec::with_capacity(n)),
            _energy: UnsafeCell::new(energy),
            energy: raw_energy,
            chunk_count: AtomicUsize::new(0),
            next_chunk: AtomicUsize::new(0),
            pass: AtomicU8::new(PASS_IDLE),
            barrier: Barrier::new(config.workers),
            tick: AtomicU32::new(0),
        }
    }

    fn work(&self, pass: u8) {
        // SAFETY: shared reads during a pass; writes go through the owned chunks.
        let storage = unsafe { &*self.storage.get() };
        let grid = unsafe { &*self.grid.get() };
        let tick = self.tick.load(Ordering::Relaxed);
        let chunk_count = self.chunk_count.load(Ordering::Acquire);
        loop {
            let chunk = self.next_chunk.fetch_add(1, Ordering::AcqRel);
            if chunk >= chunk_count {
                break;
            }
            // SAFETY: the counter hands every chunk to exactly one task.
            unsafe {
                match pass {
                    PASS_REPULSION => storage.repulsion_chunk(chunk, grid, tick, self.config.change_detection),
                    PASS_MOTION => storage.motion_chunk(chunk, tick, self.parameters.wander, self.config.change_detection, &self.energy),
                    PASS_COOLING => storage.cooling_chunk(chunk),
                    _ => unreachable!("workers run the three passes only"),
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

    /// One tick, driven by the main thread; the hash is taken by the caller.
    fn tick(&self, tick: u32, timing: &mut Timing) {
        self.tick.store(tick, Ordering::Relaxed);
        // SAFETY: between passes, the main thread is alone.
        let storage = unsafe { &mut *self.storage.get() };
        let globals = unsafe { &mut *self.globals.get() };
        let grid = unsafe { &mut *self.grid.get() };
        let entries = unsafe { &mut *self.entries.get() };
        storage.prepare_passes();
        timing.charge("canonical_traversal", || {
            entries.clear();
            storage.canonical_entries(entries);
        });
        timing.charge("spatial_hashing", || grid.build(entries));
        timing.charge("repulsion", || self.run_pass(PASS_REPULSION, storage.main_chunk_count()));
        timing.charge("motion", || self.run_pass(PASS_MOTION, storage.main_chunk_count()));
        if self.config.mode == Mode::TwoArchetypes {
            timing.charge("cooling", || self.run_pass(PASS_COOLING, storage.cooling_chunk_count()));
        }
        timing.charge("structural_changes", || {
            let alive = storage.alive();
            let n = alive.len();
            // SAFETY: the fixed point runs alone.
            let energy = unsafe { self.energy.slice(0..n) };
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
            storage.structural_changes(globals, self.config.mode);
        });
    }

    fn hash(&self) -> u64 {
        // SAFETY: between passes, the main thread is alone.
        unsafe { (*self.storage.get()).hash(&*self.globals.get()) }
    }

    fn heat_hash(&self) -> u64 {
        // SAFETY: between passes, the main thread is alone.
        unsafe { (*self.storage.get()).heat_hash() }
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
        let heat_hash = self.heat_hash();
        // SAFETY: the pool is quiescent.
        let main_chunks = unsafe { &*self.storage.get() }.main_chunk_count();
        let run_hash = run_hash(&tick_hashes);
        Outcome {
            run: Run { parameters: self.parameters, tick_hashes, run_hash, timing },
            heat_hash,
            main_chunks,
        }
    }
}

struct Outcome {
    run: Run,
    heat_hash: u64,
    main_chunks: usize,
}

fn run_once(parameters: &Parameters, config: Config) -> Outcome {
    let state = State::initialize(parameters);
    match config.layout {
        Layout::Slots => Pool::new(parameters, config, Slots::new(&state, config.mode), state.rng).run(),
        Layout::Archetypes => Pool::new(parameters, config, Archetypes::new(&state, config.mode), state.rng).run(),
        Layout::Sparse => Pool::new(parameters, config, Sparse::new(&state, config.mode), state.rng).run(),
    }
}

/// A warm-up instance of `WARM_UP_TICKS` ticks, then the measured instance.
fn measure(parameters: &Parameters, config: Config) -> Outcome {
    let warm_up = Parameters { tick_count: WARM_UP_TICKS, ..*parameters };
    run_once(&warm_up, config);
    run_once(parameters, config)
}

fn write_run(out: &mut Json, config: Config, outcome: &Outcome, reference: &Run) {
    out.object_start();
    out.string("layout", config.layout.name());
    out.string("mode", config.mode.name());
    out.string("change_detection", config.change_detection.name());
    out.u64("workers", config.workers as u64);
    write_run_core(out, &outcome.run);
    let divergence = first_divergent_tick(&outcome.run.tick_hashes, &reference.tick_hashes);
    out.bool("matches_sequential", divergence.is_none());
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    out.hex("heat_hash", outcome.heat_hash);
    out.u64("main_chunks", outcome.main_chunks as u64);
    out.object_end();
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut worker_counts = BASE_WORKER_COUNTS.to_vec();
    if !worker_counts.contains(&available) {
        worker_counts.push(available);
        worker_counts.sort_unstable();
    }
    let mut variant_workers = vec![VARIANT_WORKERS];
    if available > BASE_WORKER_COUNTS[BASE_WORKER_COUNTS.len() - 1] {
        variant_workers.push(available);
    }
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
    out.object_start();
    out.string("layout", "sequential");
    out.string("mode", Mode::OneArchetype.name());
    out.string("change_detection", ChangeDetection::None.name());
    out.u64("workers", 1);
    write_run_core(&mut out, &reference);
    out.bool("matches_sequential", true);
    out.i64("first_divergent_tick", -1);
    out.hex("heat_hash", 0);
    out.u64("main_chunks", Size::Default.entity_count().div_ceil(CHUNK) as u64);
    out.object_end();
    for mode in [Mode::OneArchetype, Mode::TwoArchetypes] {
        for layout in Layout::ALL {
            for &workers in &worker_counts {
                let config = Config { layout, mode, change_detection: ChangeDetection::None, workers };
                write_run(&mut out, config, &measure(&parameters, config), &reference);
            }
        }
    }
    for change_detection in [ChangeDetection::PerChunk, ChangeDetection::PerEntity] {
        for layout in Layout::ALL {
            for &workers in &variant_workers {
                let config = Config { layout, mode: Mode::OneArchetype, change_detection, workers };
                write_run(&mut out, config, &measure(&parameters, config), &reference);
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

    fn small(wander: bool) -> Parameters {
        Parameters::new(Size::Small, 40, wander)
    }

    fn assert_matches(config: Config, parameters: &Parameters, reference: &Run) -> Outcome {
        let outcome = run_once(parameters, config);
        assert_eq!(
            first_divergent_tick(&outcome.run.tick_hashes, &reference.tick_hashes),
            None,
            "{config:?} wander {}",
            parameters.wander
        );
        assert_eq!(outcome.run.run_hash, reference.run_hash);
        outcome
    }

    #[test]
    fn every_layout_reproduces_the_sequential_hashes_at_every_worker_count() {
        for wander in [false, true] {
            let reference = run_sequential(&small(wander));
            for layout in Layout::ALL {
                for workers in [1, 2, 3, 8] {
                    let config = Config { layout, mode: Mode::OneArchetype, change_detection: ChangeDetection::None, workers };
                    let outcome = assert_matches(config, &small(wander), &reference);
                    assert_eq!(outcome.heat_hash, Fnv1a::new().finish(), "no heat in the one-archetype mode");
                }
            }
        }
    }

    #[test]
    fn the_second_archetype_leaves_the_specified_state_unchanged_and_agrees_across_layouts() {
        let reference = run_sequential(&small(false));
        let mut heat_hashes = Vec::new();
        for layout in Layout::ALL {
            for workers in [1, 3, 8] {
                let config = Config { layout, mode: Mode::TwoArchetypes, change_detection: ChangeDetection::None, workers };
                let outcome = assert_matches(config, &small(false), &reference);
                heat_hashes.push(outcome.heat_hash);
            }
        }
        assert_ne!(heat_hashes[0], Fnv1a::new().finish(), "the heat hash covers entities");
        assert!(heat_hashes.iter().all(|&h| h == heat_hashes[0]), "{heat_hashes:x?}");
    }

    #[test]
    fn change_detection_does_not_change_the_hashes() {
        let reference = run_sequential(&small(false));
        for layout in Layout::ALL {
            for change_detection in [ChangeDetection::PerChunk, ChangeDetection::PerEntity] {
                let config = Config { layout, mode: Mode::OneArchetype, change_detection, workers: 3 };
                assert_matches(config, &small(false), &reference);
            }
        }
    }

    #[test]
    fn the_archetype_tables_and_the_sparse_sets_stay_consistent_under_structural_changes() {
        let parameters = small(false);
        let state = State::initialize(&parameters);
        let mut globals = Globals { rng: state.rng, energy: 0.0 };
        let mut archetypes = Archetypes::new(&state, Mode::TwoArchetypes);
        let mut sparse = Sparse::new(&state, Mode::TwoArchetypes);
        let mut sparse_globals = globals;
        archetypes.check_consistency();
        sparse.check_consistency();
        for _ in 0..200 {
            archetypes.structural_changes(&mut globals, Mode::TwoArchetypes);
            archetypes.check_consistency();
            sparse.structural_changes(&mut sparse_globals, Mode::TwoArchetypes);
            sparse.check_consistency();
        }
        assert_eq!(globals.rng.state, sparse_globals.rng.state);
        assert_eq!(archetypes.hash(&globals), sparse.hash(&sparse_globals));
        assert!(archetypes.generation.iter().any(|&g| g > 0));
        assert!(!archetypes.tables[0].spare.is_empty() || !archetypes.tables[1].spare.is_empty() || archetypes.tables[0].chunks.len() + archetypes.tables[1].chunks.len() >= 4);
    }

    #[test]
    fn the_grid_keeps_the_slot_order_inside_each_cell() {
        let state = State::initialize(&small(false));
        let entries: Vec<Entry> =
            (0..state.len()).map(|slot| Entry { slot: slot as u32, x: state.x[slot], y: state.y[slot] }).collect();
        let mut grid = Grid::new();
        grid.build(&entries);
        let mut reference = reference_workload::Grid::new();
        reference.build(&state);
        for cy in 0..GRID_SIDE {
            for cx in 0..GRID_SIDE {
                let slots: Vec<u32> = grid.entries_in(cx, cy).iter().map(|e| e.slot).collect();
                assert_eq!(slots, reference.slots_in(cx, cy));
            }
        }
        let (fx, fy) = repulsion_force(state.x[7], state.y[7], 7, &grid);
        assert_eq!((fx, fy), reference_workload::repulsion_force(&state, &reference, 7));
    }
}
