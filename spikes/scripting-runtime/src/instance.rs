//! What the world asks of one instance of the tick's systems, whichever way
//! they run, and the native baseline: the same systems linked into the host,
//! behind the same copies. Throwaway, never product code.

use std::sync::Arc;

use reference_workload::CHUNK;
use tick_systems::{CELLS, ChunkInput, ChunkOutput, CompiledIn, Population};

/// The arenas, by the index the host and the module agree on.
pub const GENERATION: usize = 0;
pub const ALIVE: usize = 1;
pub const X: usize = 2;
pub const Y: usize = 3;
pub const VX: usize = 4;
pub const VY: usize = 5;
pub const MASS: usize = 6;
pub const CELL_START: usize = 7;
pub const SLOTS: usize = 8;
pub const OUT_X: usize = 9;
pub const OUT_Y: usize = 10;
pub const OUT_VX: usize = 11;
pub const OUT_VY: usize = 12;
pub const ARENAS: usize = 13;

/// The largest population an instance holds: the module's arena capacity.
pub const CAPACITY: usize = 10_000;

/// The fuel given to a call whose budget is not the subject of a measurement.
pub const AMPLE_FUEL: u64 = 1 << 40;

/// How a call into an instance failed: the runtime's name for the trap, and
/// its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub kind: String,
    pub detail: String,
}

pub trait Instance {
    /// Copies `bytes` into an arena at a byte offset.
    fn write(&mut self, arena: usize, offset: usize, bytes: &[u8]);
    /// Copies an arena's bytes at a byte offset into `out`.
    fn read(&mut self, arena: usize, offset: usize, out: &mut [u8]);
    fn spatial_hashing(&mut self, count: u32) -> Result<(), Fault>;
    fn chunk_systems(&mut self, start: u32, count: u32, population: u32, tick: u32, wander: u32, fault: u32) -> Result<f32, Fault>;
    fn structural_changes(&mut self, count: u32, stream_state: u64) -> Result<u64, Fault>;
    fn call_nop(&mut self, times: u32) -> Result<(), Fault>;
    /// The bit patterns of four invalid results computed inside the instance.
    fn make_nans(&mut self) -> Result<[u32; 4], Fault>;
    fn dive(&mut self, depth: u32) -> Result<u32, Fault>;
    /// Sets the fuel of the next call; nothing where fuel is not metered.
    fn set_fuel(&mut self, fuel: u64);
    /// The fuel left, where fuel is metered.
    fn fuel(&self) -> Option<u64>;
    /// The size of the instance's linear memory, where it has one.
    fn memory_bytes(&self) -> u64;
}

pub trait Runtime: Send + Sync {
    fn name(&self) -> &'static str;
    fn metered(&self) -> bool;
    fn instantiate(&self) -> Result<Box<dyn Instance>, String>;
}

pub fn bytes<T: Copy>(values: &[T]) -> &[u8] {
    // SAFETY: the column types (u8, u32, f32) have no padding.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values)) }
}

pub fn bytes_mut<T: Copy>(values: &mut [T]) -> &mut [u8] {
    // SAFETY: as above, and every bit pattern is a valid u8, u32 or f32.
    unsafe { std::slice::from_raw_parts_mut(values.as_mut_ptr().cast::<u8>(), std::mem::size_of_val(values)) }
}

/// The native baseline: the systems called directly on host columns, with the
/// damping constant of the module it stands for.
pub struct Native {
    pub damping: f32,
}

impl Runtime for Native {
    fn name(&self) -> &'static str {
        "native"
    }

    fn metered(&self) -> bool {
        false
    }

    fn instantiate(&self) -> Result<Box<dyn Instance>, String> {
        Ok(Box::new(NativeInstance {
            damping: self.damping,
            generation: vec![0; CAPACITY],
            alive: vec![0; CAPACITY],
            columns: std::array::from_fn(|_| vec![0.0; CAPACITY]),
            cell_start: vec![0; CELLS + 1],
            cursor: vec![0; CELLS + 1],
            slots: vec![0; CAPACITY],
            out: std::array::from_fn(|_| vec![0.0; CHUNK]),
        }))
    }
}

pub fn native(damping: f32) -> Arc<dyn Runtime> {
    Arc::new(Native { damping })
}

struct NativeInstance {
    damping: f32,
    generation: Vec<u32>,
    alive: Vec<u8>,
    /// x, y, vx, vy, mass.
    columns: [Vec<f32>; 5],
    cell_start: Vec<u32>,
    cursor: Vec<u32>,
    slots: Vec<u32>,
    /// The four written columns of a chunk.
    out: [Vec<f32>; 4],
}

impl NativeInstance {
    fn arena(&mut self, arena: usize) -> &mut [u8] {
        match arena {
            GENERATION => bytes_mut(&mut self.generation),
            ALIVE => &mut self.alive,
            X..=MASS => bytes_mut(&mut self.columns[arena - X]),
            CELL_START => bytes_mut(&mut self.cell_start),
            SLOTS => bytes_mut(&mut self.slots),
            OUT_X..=OUT_VY => bytes_mut(&mut self.out[arena - OUT_X]),
            _ => panic!("no arena {arena}"),
        }
    }
}

impl Instance for NativeInstance {
    fn write(&mut self, arena: usize, offset: usize, bytes: &[u8]) {
        self.arena(arena)[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    fn read(&mut self, arena: usize, offset: usize, out: &mut [u8]) {
        out.copy_from_slice(&self.arena(arena)[offset..offset + out.len()]);
    }

    fn spatial_hashing(&mut self, count: u32) -> Result<(), Fault> {
        let n = count as usize;
        tick_systems::spatial_hashing(&self.alive[..n], &self.columns[0][..n], &self.columns[1][..n], &mut self.cell_start, &mut self.cursor, &mut self.slots[..n]);
        Ok(())
    }

    fn chunk_systems(&mut self, start: u32, count: u32, population: u32, tick: u32, wander: u32, fault: u32) -> Result<f32, Fault> {
        if fault != 0 {
            return Err(Fault { kind: "Injected".to_string(), detail: "the native baseline does not trap".to_string() });
        }
        let n = population as usize;
        let [x, y, vx, vy, mass] = &self.columns;
        let input =
            ChunkInput { alive: &self.alive[..n], x: &x[..n], y: &y[..n], vx: &vx[..n], vy: &vy[..n], mass: &mass[..n], cell_start: &self.cell_start, slots: &self.slots[..n] };
        let [out_x, out_y, out_vx, out_vy] = &mut self.out;
        let count = count as usize;
        let mut out = ChunkOutput { x: &mut out_x[..count], y: &mut out_y[..count], vx: &mut out_vx[..count], vy: &mut out_vy[..count] };
        let wander = (wander != 0).then_some(&CompiledIn);
        Ok(tick_systems::chunk_systems(&input, &mut out, start as usize, tick, wander, self.damping))
    }

    fn structural_changes(&mut self, count: u32, stream_state: u64) -> Result<u64, Fault> {
        let n = count as usize;
        let [x, y, vx, vy, mass] = &mut self.columns;
        let mut population =
            Population { generation: &mut self.generation[..n], alive: &mut self.alive[..n], x: &mut x[..n], y: &mut y[..n], vx: &mut vx[..n], vy: &mut vy[..n], mass: &mut mass[..n] };
        Ok(tick_systems::structural_changes(&mut population, stream_state))
    }

    fn call_nop(&mut self, _times: u32) -> Result<(), Fault> {
        Ok(())
    }

    fn make_nans(&mut self) -> Result<[u32; 4], Fault> {
        let (zero, infinity, negative) = (std::hint::black_box(0.0_f32), std::hint::black_box(f32::INFINITY), std::hint::black_box(-1.0_f32));
        Ok([(zero / zero).to_bits(), (infinity - infinity).to_bits(), negative.sqrt().to_bits(), (zero * infinity).to_bits()])
    }

    fn dive(&mut self, _depth: u32) -> Result<u32, Fault> {
        Ok(0)
    }

    fn set_fuel(&mut self, _fuel: u64) {}

    fn fuel(&self) -> Option<u64> {
        None
    }

    fn memory_bytes(&self) -> u64 {
        0
    }
}
