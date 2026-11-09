//! Every gameplay system of the reference workload's tick, written once over
//! plain columns so that the same code runs natively in the host and inside a
//! WebAssembly module. Throwaway, never product code.
//!
//! The arithmetic follows the reference workload's specification operation by
//! operation; the kernels the specification fixes are the reference crate's
//! own. Two things are parameters here and constants there: the damping
//! factor, so that a second module can differ from the first by its code, and
//! the cosine and sine of the wander system, so that they can be the code
//! compiled with the systems or a function the host provides.

use reference_workload::{
    CELL_SIZE, GRID_SIDE, INTERACTION_RADIUS, KILLS_PER_TICK, REPULSION_STRENGTH, SPAWNS_PER_TICK, SPEED_LIMIT, SplitMix64, TIME_STEP, WANDER_ANGLE_PERIOD,
    WANDER_ANGLE_SCALE, WANDER_STRENGTH, WORLD_SIDE, bounced, integrated, kinetic_energy,
};

pub const CELLS: usize = GRID_SIDE * GRID_SIDE;
/// The reference workload's damping factor, 1 - 2^-8.
pub const DAMPING: f32 = reference_workload::DAMPING;
/// The damping factor of the second module, 1 - 2^-7: a one-constant change of
/// code that changes every tick's hash from the tick it is loaded at.
pub const DAMPING_VARIANT: f32 = 0.992_187_5;

/// Where the wander system takes its cosine and sine from.
pub trait Trig {
    fn cos(&self, angle: f32) -> f32;
    fn sin(&self, angle: f32) -> f32;
}

/// The functions compiled with the systems: the platform's library natively,
/// the software implementation linked into the module on WebAssembly.
pub struct CompiledIn;

impl Trig for CompiledIn {
    fn cos(&self, angle: f32) -> f32 {
        angle.cos()
    }

    fn sin(&self, angle: f32) -> f32 {
        angle.sin()
    }
}

fn cell_of(x: f32, y: f32) -> (usize, usize) {
    ((x / CELL_SIZE).floor() as usize, (y / CELL_SIZE).floor() as usize)
}

/// The spatial hashing system: the cell lists as a counting sort, the slots
/// of each cell in increasing index order. `cell_start` and `cursor` hold
/// `CELLS + 1` entries, `slots` one entry per slot.
pub fn spatial_hashing(alive: &[u8], x: &[f32], y: &[f32], cell_start: &mut [u32], cursor: &mut [u32], slots: &mut [u32]) {
    cell_start.fill(0);
    for slot in 0..alive.len() {
        if alive[slot] != 0 {
            let (cx, cy) = cell_of(x[slot], y[slot]);
            cell_start[cy * GRID_SIDE + cx + 1] += 1;
        }
    }
    for cell in 0..CELLS {
        cell_start[cell + 1] += cell_start[cell];
    }
    cursor.copy_from_slice(cell_start);
    for slot in 0..alive.len() {
        if alive[slot] != 0 {
            let (cx, cy) = cell_of(x[slot], y[slot]);
            let cell = cy * GRID_SIDE + cx;
            slots[cursor[cell] as usize] = slot as u32;
            cursor[cell] += 1;
        }
    }
}

fn repulsion_force(x: &[f32], y: &[f32], cell_start: &[u32], slots: &[u32], i: usize) -> (f32, f32) {
    let (xi, yi) = (x[i], y[i]);
    let (cx, cy) = cell_of(xi, yi);
    let mut fx = 0.0_f32;
    let mut fy = 0.0_f32;
    for b in -1_isize..=1 {
        for a in -1_isize..=1 {
            let (gx, gy) = (cx as isize + a, cy as isize + b);
            if gx < 0 || gy < 0 || gx >= GRID_SIDE as isize || gy >= GRID_SIDE as isize {
                continue;
            }
            let cell = gy as usize * GRID_SIDE + gx as usize;
            for &j in &slots[cell_start[cell] as usize..cell_start[cell + 1] as usize] {
                let j = j as usize;
                if j == i {
                    continue;
                }
                let dx = x[j] - xi;
                let dy = y[j] - yi;
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

fn repelled_velocity(v: f32, force: f32, mass: f32, damping: f32) -> f32 {
    let acceleration = force / mass;
    ((v + (acceleration * TIME_STEP)) * damping).clamp(-SPEED_LIMIT, SPEED_LIMIT)
}

/// The columns one chunk's systems read: the whole population's positions,
/// liveness and cell lists (repulsion reads neighbors in other chunks), and
/// the velocities and masses, of which only the chunk's range is read.
pub struct ChunkInput<'a> {
    pub alive: &'a [u8],
    pub x: &'a [f32],
    pub y: &'a [f32],
    pub vx: &'a [f32],
    pub vy: &'a [f32],
    pub mass: &'a [f32],
    pub cell_start: &'a [u32],
    pub slots: &'a [u32],
}

/// The columns one chunk's systems write, one entry per slot of the chunk.
pub struct ChunkOutput<'a> {
    pub x: &'a mut [f32],
    pub y: &'a mut [f32],
    pub vx: &'a mut [f32],
    pub vy: &'a mut [f32],
}

/// Repulsion, wander (when `wander` is given), integration, boundary and the
/// chunk's kinetic energy for the slots `start..start + out.x.len()`, each
/// system applied to a slot in the specification's order. Returns the chunk's
/// energy, the alive slots' energies added in index order.
pub fn chunk_systems<T: Trig>(input: &ChunkInput, out: &mut ChunkOutput, start: usize, tick: u32, wander: Option<&T>, damping: f32) -> f32 {
    let mut energy = 0.0_f32;
    for k in 0..out.x.len() {
        let i = start + k;
        let (mut x, mut y, mut vx, mut vy) = (input.x[i], input.y[i], input.vx[i], input.vy[i]);
        if input.alive[i] != 0 {
            let mass = input.mass[i];
            let (fx, fy) = repulsion_force(input.x, input.y, input.cell_start, input.slots, i);
            vx = repelled_velocity(vx, fx, mass, damping);
            vy = repelled_velocity(vy, fy, mass, damping);
            if let Some(trig) = wander {
                let theta = (((tick as usize + i) % WANDER_ANGLE_PERIOD) as f32) * WANDER_ANGLE_SCALE;
                vx = (vx + (trig.cos(theta) * WANDER_STRENGTH)).clamp(-SPEED_LIMIT, SPEED_LIMIT);
                vy = (vy + (trig.sin(theta) * WANDER_STRENGTH)).clamp(-SPEED_LIMIT, SPEED_LIMIT);
            }
            x = integrated(x, vx);
            y = integrated(y, vy);
            (x, vx) = bounced(x, vx);
            (y, vy) = bounced(y, vy);
            energy += kinetic_energy(vx, vy, mass);
        }
        out.x[k] = x;
        out.y[k] = y;
        out.vx[k] = vx;
        out.vy[k] = vy;
    }
    energy
}

/// The whole population's columns, as the structural changes system writes them.
pub struct Population<'a> {
    pub generation: &'a mut [u32],
    pub alive: &'a mut [u8],
    pub x: &'a mut [f32],
    pub y: &'a mut [f32],
    pub vx: &'a mut [f32],
    pub vy: &'a mut [f32],
    pub mass: &'a mut [f32],
}

/// The structural changes system: the kills then the spawns of one tick, the
/// global stream consumed in the specification's order. Returns the stream's
/// state after them.
pub fn structural_changes(population: &mut Population, stream_state: u64) -> u64 {
    let n = population.alive.len();
    let mut stream = SplitMix64::new(stream_state);
    for _ in 0..KILLS_PER_TICK {
        let slot = (stream.next() % n as u64) as usize;
        population.alive[slot] = 0;
    }
    for _ in 0..SPAWNS_PER_TICK {
        let Some(slot) = population.alive.iter().position(|alive| *alive == 0) else {
            break;
        };
        population.x[slot] = stream.u01() * WORLD_SIDE;
        population.y[slot] = stream.u01() * WORLD_SIDE;
        population.vx[slot] = ((stream.u01() * 2.0) - 1.0) * 32.0;
        population.vy[slot] = ((stream.u01() * 2.0) - 1.0) * 32.0;
        population.mass[slot] = stream.mass();
        population.generation[slot] += 1;
        population.alive[slot] = 1;
    }
    stream.state
}
