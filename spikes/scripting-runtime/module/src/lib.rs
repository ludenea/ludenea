//! The tick's systems as a WebAssembly module. Throwaway, never product code.
//!
//! The columns live in static arenas, so the module's linear memory has its
//! final size when it is instantiated and never grows. The host copies columns
//! into the arenas, calls a system, and copies the written columns out; the
//! arena addresses come from `arena`. Three imports are the module's whole
//! view of the host: a cosine, a sine and a function that does nothing.

#![allow(clippy::missing_safety_doc)]

use reference_workload::CHUNK;
use tick_systems::{CELLS, ChunkInput, ChunkOutput, CompiledIn, Population, Trig};

/// The largest population the arenas hold: the reference workload's default size.
pub const CAPACITY: usize = 10_000;

#[cfg(not(feature = "variant"))]
const DAMPING: f32 = tick_systems::DAMPING;
#[cfg(feature = "variant")]
const DAMPING: f32 = tick_systems::DAMPING_VARIANT;

static mut GENERATION: [u32; CAPACITY] = [0; CAPACITY];
static mut ALIVE: [u8; CAPACITY] = [0; CAPACITY];
static mut X: [f32; CAPACITY] = [0.0; CAPACITY];
static mut Y: [f32; CAPACITY] = [0.0; CAPACITY];
static mut VX: [f32; CAPACITY] = [0.0; CAPACITY];
static mut VY: [f32; CAPACITY] = [0.0; CAPACITY];
static mut MASS: [f32; CAPACITY] = [0.0; CAPACITY];
static mut CELL_START: [u32; CELLS + 1] = [0; CELLS + 1];
static mut SLOTS: [u32; CAPACITY] = [0; CAPACITY];
static mut OUT_X: [f32; CHUNK] = [0.0; CHUNK];
static mut OUT_Y: [f32; CHUNK] = [0.0; CHUNK];
static mut OUT_VX: [f32; CHUNK] = [0.0; CHUNK];
static mut OUT_VY: [f32; CHUNK] = [0.0; CHUNK];
static mut CURSOR: [u32; CELLS + 1] = [0; CELLS + 1];

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "host")]
unsafe extern "C" {
    fn host_cos(angle: f32) -> f32;
    fn host_sin(angle: f32) -> f32;
    fn host_nop();
}

// The workspace also builds this crate for the host's own target, where no
// host provides the imports: stand-ins keep that build linking. Only the
// WebAssembly build is ever loaded.
#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_cos(angle: f32) -> f32 {
    angle.cos()
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_sin(angle: f32) -> f32 {
    angle.sin()
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_nop() {}

struct HostTrig;

impl Trig for HostTrig {
    fn cos(&self, angle: f32) -> f32 {
        // SAFETY: a plain host function over one value.
        unsafe { host_cos(angle) }
    }

    fn sin(&self, angle: f32) -> f32 {
        // SAFETY: as above.
        unsafe { host_sin(angle) }
    }
}

/// The address of an arena in the module's memory, by the index the host and
/// the module agree on: generation, alive, x, y, vx, vy, mass, cell start,
/// slots, then the four written columns of a chunk.
#[unsafe(no_mangle)]
pub extern "C" fn arena(index: u32) -> u32 {
    let address = match index {
        0 => (&raw const GENERATION) as usize,
        1 => (&raw const ALIVE) as usize,
        2 => (&raw const X) as usize,
        3 => (&raw const Y) as usize,
        4 => (&raw const VX) as usize,
        5 => (&raw const VY) as usize,
        6 => (&raw const MASS) as usize,
        7 => (&raw const CELL_START) as usize,
        8 => (&raw const SLOTS) as usize,
        9 => (&raw const OUT_X) as usize,
        10 => (&raw const OUT_Y) as usize,
        11 => (&raw const OUT_VX) as usize,
        12 => (&raw const OUT_VY) as usize,
        _ => 0,
    };
    address as u32
}

unsafe fn column<T>(arena: *const T, count: usize) -> &'static [T] {
    // SAFETY: the caller keeps `count` within the arena; the module is single threaded.
    unsafe { std::slice::from_raw_parts(arena, count) }
}

unsafe fn column_mut<T>(arena: *mut T, count: usize) -> &'static mut [T] {
    // SAFETY: as above, and no other reference to the arena is alive during a call.
    unsafe { std::slice::from_raw_parts_mut(arena, count) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn spatial_hashing(count: u32) {
    let n = (count as usize).min(CAPACITY);
    // SAFETY: every range is inside its arena.
    unsafe {
        tick_systems::spatial_hashing(
            column((&raw const ALIVE).cast(), n),
            column((&raw const X).cast(), n),
            column((&raw const Y).cast(), n),
            column_mut((&raw mut CELL_START).cast(), CELLS + 1),
            column_mut((&raw mut CURSOR).cast(), CELLS + 1),
            column_mut((&raw mut SLOTS).cast(), n),
        );
    }
}

/// The chunk's systems. `wander`: 0 off, 1 the cosine and sine compiled into
/// the module, 2 the host's. `fault`: 0 none, 1 a read outside the memory,
/// 2 a panic; the two faults exist so that the host can measure where and how
/// a failing module stops.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chunk_systems(start: u32, count: u32, population: u32, tick: u32, wander: u32, fault: u32) -> f32 {
    if fault == 1 {
        // SAFETY: none on purpose; the address is beyond the memory and the read traps.
        let value = unsafe { std::ptr::read_volatile(0xffff_fff0_usize as *const u32) };
        return value as f32;
    }
    if fault == 2 {
        panic!("the module was asked to panic");
    }
    let n = (population as usize).min(CAPACITY);
    let start = (start as usize).min(n);
    let count = (count as usize).min(CHUNK).min(n - start);
    // SAFETY: every range is inside its arena.
    unsafe {
        let input = ChunkInput {
            alive: column((&raw const ALIVE).cast(), n),
            x: column((&raw const X).cast(), n),
            y: column((&raw const Y).cast(), n),
            vx: column((&raw const VX).cast(), n),
            vy: column((&raw const VY).cast(), n),
            mass: column((&raw const MASS).cast(), n),
            cell_start: column((&raw const CELL_START).cast(), CELLS + 1),
            slots: column((&raw const SLOTS).cast(), n),
        };
        let mut out = ChunkOutput {
            x: column_mut((&raw mut OUT_X).cast(), count),
            y: column_mut((&raw mut OUT_Y).cast(), count),
            vx: column_mut((&raw mut OUT_VX).cast(), count),
            vy: column_mut((&raw mut OUT_VY).cast(), count),
        };
        match wander {
            0 => tick_systems::chunk_systems::<CompiledIn>(&input, &mut out, start, tick, None, DAMPING),
            1 => tick_systems::chunk_systems(&input, &mut out, start, tick, Some(&CompiledIn), DAMPING),
            _ => tick_systems::chunk_systems(&input, &mut out, start, tick, Some(&HostTrig), DAMPING),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn structural_changes(count: u32, stream_state: u64) -> u64 {
    let n = (count as usize).min(CAPACITY);
    // SAFETY: every range is inside its arena.
    unsafe {
        let mut population = Population {
            generation: column_mut((&raw mut GENERATION).cast(), n),
            alive: column_mut((&raw mut ALIVE).cast(), n),
            x: column_mut((&raw mut X).cast(), n),
            y: column_mut((&raw mut Y).cast(), n),
            vx: column_mut((&raw mut VX).cast(), n),
            vy: column_mut((&raw mut VY).cast(), n),
            mass: column_mut((&raw mut MASS).cast(), n),
        };
        tick_systems::structural_changes(&mut population, stream_state)
    }
}

/// Calls the host's empty function `times` times: the cost of a host call.
#[unsafe(no_mangle)]
pub extern "C" fn call_nop(times: u32) {
    for _ in 0..times {
        // SAFETY: a plain host function without arguments.
        unsafe { host_nop() };
    }
}

/// Writes four invalid results into the first written column, computed at run
/// time from the arguments: zero over zero, infinity minus infinity, the
/// square root of a negative number and zero times infinity.
// The operands are equal on purpose: the results must be invalid.
#[allow(clippy::eq_op)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn make_nans(zero: f32, infinity: f32, negative: f32) {
    // SAFETY: four entries of a 256-entry arena.
    let out = unsafe { column_mut((&raw mut OUT_X).cast::<f32>(), 4) };
    out[0] = zero / zero;
    out[1] = infinity - infinity;
    out[2] = negative.sqrt();
    out[3] = zero * infinity;
}

/// A recursion the compiler cannot turn into a loop: each level's result
/// depends on the level below it. No local has its address taken, so a level
/// uses the runtime's call stack and none of the module's memory.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn dive(depth: u32, seed: u32) -> u32 {
    if depth == 0 {
        return seed;
    }
    let below = dive(depth - 1, seed.wrapping_mul(31).wrapping_add(depth));
    below ^ depth.rotate_left(below & 31)
}
