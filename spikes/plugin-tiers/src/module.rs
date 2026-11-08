//! The sandboxed tier: the motion pass as a WebAssembly module run by
//! Wasmtime, one instance per worker thread, the chunk's columns copied into
//! the module's memory and back around every call. Throwaway, never product
//! code.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reference_workload::CHUNK;
use wasmtime::{Engine, Instance, Memory, Module, Store, TypedFunc};

use crate::world::MotionPass;

type Params = (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32);
const PAGE: u32 = 65_536;
/// One column of `CHUNK` values of 4 bytes.
const COLUMN: u32 = (CHUNK * 4) as u32;

/// One instance of the module with its store, the exported pass, its memory
/// and the page the host grew for the columns: x, y, mass, alive, then the
/// four `next` columns, each `COLUMN` bytes apart.
struct ModuleInstance {
    store: Store<()>,
    func: TypedFunc<Params, f32>,
    memory: Memory,
    base: u32,
}

impl ModuleInstance {
    fn new(engine: &Engine, module: &Module) -> Result<ModuleInstance, String> {
        let mut store = Store::new(engine, ());
        let instance = Instance::new(&mut store, module, &[]).map_err(|error| format!("instantiating: {error}"))?;
        let memory = instance.get_memory(&mut store, "memory").ok_or("the module exports no memory")?;
        let func = instance.get_typed_func::<Params, f32>(&mut store, "motion").map_err(|error| format!("resolving motion: {error}"))?;
        let pages = memory.grow(&mut store, 1).map_err(|error| format!("growing the memory: {error}"))?;
        let base = u32::try_from(pages).map_err(|_| "memory too large")? * PAGE;
        Ok(ModuleInstance { store, func, memory, base })
    }
}

/// The hosts are little-endian on every cell of the boundary, like the
/// module's memory: a column's bytes are copied as they are.
fn bytes(values: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding; reading its bytes is always valid.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * 4) }
}

fn bytes_mut(values: &mut [f32]) -> &mut [u8] {
    // SAFETY: as above, and every bit pattern is a valid f32.
    unsafe { std::slice::from_raw_parts_mut(values.as_mut_ptr().cast::<u8>(), values.len() * 4) }
}

// The instances live in a thread-local slot of each worker thread, so that a
// chunk reuses the instance its thread used last (one instance per worker, its
// memory warm in that core's cache, no lock on the hot path); the world's
// workers live for one step, so a step instantiates once per worker. A reload
// compiles the module again and bumps the generation: every thread replaces
// its instance on its next chunk. (A first version pooled the instances behind
// a lock: at 8 and 10 workers the lock's parking and the instances migrating
// between cores cost ten times the pass.)
thread_local! {
    static INSTANCES: RefCell<Vec<(u64, u64, ModuleInstance)>> = const { RefCell::new(Vec::new()) };
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct WasmModule {
    id: u64,
    engine: Engine,
    path: PathBuf,
    module: Mutex<Module>,
    generation: AtomicU64,
    copy_nanos: AtomicU64,
    call_nanos: AtomicU64,
    instantiations: AtomicU64,
    reloads: AtomicU64,
}

impl WasmModule {
    /// Compiles the module at `path` and checks that it instantiates.
    pub fn load(path: &Path) -> Result<WasmModule, String> {
        let engine = Engine::default();
        let module = Module::from_file(&engine, path).map_err(|error| format!("compiling {}: {error}", path.display()))?;
        ModuleInstance::new(&engine, &module)?;
        Ok(WasmModule {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            engine,
            path: path.to_path_buf(),
            module: Mutex::new(module),
            generation: AtomicU64::new(0),
            copy_nanos: AtomicU64::new(0),
            call_nanos: AtomicU64::new(0),
            instantiations: AtomicU64::new(0),
            reloads: AtomicU64::new(0),
        })
    }

    /// Compiles the module again from its file and retires every instance:
    /// each worker instantiates the new module on its next chunk. Returns the
    /// time the compilation took.
    pub fn reload(&self) -> Result<Duration, String> {
        let started = Instant::now();
        let module = Module::from_file(&self.engine, &self.path).map_err(|error| format!("compiling {}: {error}", self.path.display()))?;
        *self.module.lock().unwrap() = module;
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.reloads.fetch_add(1, Ordering::Relaxed);
        Ok(started.elapsed())
    }

    pub fn reloads(&self) -> u64 {
        self.reloads.load(Ordering::Relaxed)
    }

    /// How many instances the worker threads created so far.
    pub fn instantiations(&self) -> u64 {
        self.instantiations.load(Ordering::Relaxed)
    }

    /// The time spent copying columns in and out, and the time spent inside
    /// the exported function.
    pub fn times(&self) -> (Duration, Duration) {
        (
            Duration::from_nanos(self.copy_nanos.load(Ordering::Relaxed)),
            Duration::from_nanos(self.call_nanos.load(Ordering::Relaxed)),
        )
    }

    /// Runs `body` on this thread's instance of the current generation,
    /// instantiating it first when the thread has none or an old one.
    fn with_instance<T>(&self, body: impl FnOnce(&mut ModuleInstance) -> T) -> T {
        let generation = self.generation.load(Ordering::SeqCst);
        INSTANCES.with(|slot| {
            let mut instances = slot.borrow_mut();
            let index = match instances.iter().position(|(id, _, _)| *id == self.id) {
                Some(index) if instances[index].1 == generation => index,
                found => {
                    let module = self.module.lock().unwrap().clone();
                    let instance = ModuleInstance::new(&self.engine, &module).unwrap_or_else(|error| panic!("module instance: {error}"));
                    self.instantiations.fetch_add(1, Ordering::Relaxed);
                    match found {
                        Some(index) => {
                            instances[index] = (self.id, generation, instance);
                            index
                        }
                        None => {
                            instances.push((self.id, generation, instance));
                            instances.len() - 1
                        }
                    }
                }
            };
            body(&mut instances[index].2)
        })
    }
}

impl MotionPass for WasmModule {
    fn motion(
        &self,
        x: &[f32],
        y: &[f32],
        mass: &[f32],
        alive: &[bool],
        next_x: &mut [f32],
        next_y: &mut [f32],
        next_vx: &mut [f32],
        next_vy: &mut [f32],
        tick: u32,
    ) -> f32 {
        let count = alive.len();
        assert!(count <= CHUNK, "a chunk fits the module's columns");
        self.with_instance(|instance| {
            let base = instance.base;
            let copying = Instant::now();
            {
                let memory = instance.memory.data_mut(&mut instance.store);
                let columns = [bytes(x), bytes(y), bytes(mass), crate::tiers::alive_bytes(alive), bytes(next_vx), bytes(next_vy)];
                for (index, column) in [0, 1, 2, 3, 6, 7].into_iter().zip(columns) {
                    let at = (base + index * COLUMN) as usize;
                    memory[at..at + column.len()].copy_from_slice(column);
                }
            }
            let copied_in = copying.elapsed();
            let calling = Instant::now();
            let energy = instance
                .func
                .call(
                    &mut instance.store,
                    (
                        base,
                        base + COLUMN,
                        base + 2 * COLUMN,
                        base + 3 * COLUMN,
                        base + 4 * COLUMN,
                        base + 5 * COLUMN,
                        base + 6 * COLUMN,
                        base + 7 * COLUMN,
                        count as u32,
                        tick,
                    ),
                )
                .unwrap_or_else(|error| panic!("module call: {error}"));
            let called = calling.elapsed();
            let copying = Instant::now();
            {
                let memory = instance.memory.data(&instance.store);
                for (index, column) in [4, 5, 6, 7].into_iter().zip([next_x, next_y, next_vx, next_vy]) {
                    let at = (base + index * COLUMN) as usize;
                    bytes_mut(column).copy_from_slice(&memory[at..at + count * 4]);
                }
            }
            let copied = copied_in + copying.elapsed();
            self.copy_nanos.fetch_add(copied.as_nanos() as u64, Ordering::Relaxed);
            self.call_nanos.fetch_add(called.as_nanos() as u64, Ordering::Relaxed);
            energy
        })
    }
}
