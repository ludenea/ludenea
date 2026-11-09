//! The compiling runtime: the module compiled and run by Wasmtime, with the
//! settings that keep its execution deterministic. Throwaway, never product
//! code.

use wasmtime::{Config, Engine, Instance as WasmInstance, InstanceAllocationStrategy, Linker, Memory, Module, PoolingAllocationConfig, Store, Trap, TypedFunc};

use crate::instance::{AMPLE_FUEL, ARENAS, Fault, Instance, OUT_X, Runtime};

pub struct Compiled {
    engine: Engine,
    module: Module,
    linker: Linker<()>,
    metered: bool,
    name: &'static str,
}

impl Compiled {
    /// Compiles `wasm`. `metered` turns fuel on; `pooling` selects the pooling
    /// instance allocator, sized for a few hundred small instances.
    pub fn new(wasm: &[u8], metered: bool, pooling: bool) -> Result<Compiled, String> {
        let mut config = Config::new();
        config.consume_fuel(metered);
        // The pinned settings: one NaN bit pattern, no relaxed SIMD. Threads
        // are off because the feature that would provide them is not built.
        config.cranelift_nan_canonicalization(true);
        config.wasm_relaxed_simd(false);
        if pooling {
            let mut pool = PoolingAllocationConfig::default();
            pool.total_core_instances(256);
            pool.total_memories(256);
            pool.total_tables(256);
            pool.max_memory_size(64 << 20);
            config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        }
        let engine = Engine::new(&config).map_err(|error| format!("creating the engine: {error}"))?;
        let module = Module::new(&engine, wasm).map_err(|error| format!("compiling the module: {error}"))?;
        let mut linker = Linker::new(&engine);
        let wrapped = linker
            .func_wrap("host", "host_cos", |angle: f32| angle.cos())
            .and_then(|linker| linker.func_wrap("host", "host_sin", |angle: f32| angle.sin()))
            .and_then(|linker| linker.func_wrap("host", "host_nop", || {}))
            .map(|_| ());
        wrapped.map_err(|error| format!("defining the host functions: {error}"))?;
        let name = match (metered, pooling) {
            (true, false) => "wasmtime, fuel on",
            (false, false) => "wasmtime, fuel off",
            (true, true) => "wasmtime, fuel on, pooling allocator",
            (false, true) => "wasmtime, fuel off, pooling allocator",
        };
        Ok(Compiled { engine, module, linker, metered, name })
    }
}

impl Runtime for Compiled {
    fn name(&self) -> &'static str {
        self.name
    }

    fn metered(&self) -> bool {
        self.metered
    }

    fn instantiate(&self) -> Result<Box<dyn Instance>, String> {
        let mut store = Store::new(&self.engine, ());
        if self.metered {
            store.set_fuel(AMPLE_FUEL).map_err(|error| format!("setting fuel: {error}"))?;
        }
        let instance = self.linker.instantiate(&mut store, &self.module).map_err(|error| format!("instantiating: {error}"))?;
        CompiledInstance::new(store, instance, self.metered).map(|instance| Box::new(instance) as Box<dyn Instance>)
    }
}

struct CompiledInstance {
    store: Store<()>,
    memory: Memory,
    arenas: [usize; ARENAS],
    metered: bool,
    spatial_hashing: TypedFunc<u32, ()>,
    chunk_systems: TypedFunc<(u32, u32, u32, u32, u32, u32), f32>,
    structural_changes: TypedFunc<(u32, u64), u64>,
    call_nop: TypedFunc<u32, ()>,
    make_nans: TypedFunc<(f32, f32, f32), ()>,
    dive: TypedFunc<(u32, u32), u32>,
}

fn fault(error: wasmtime::Error) -> Fault {
    let kind = match error.downcast_ref::<Trap>() {
        Some(trap) => format!("{trap:?}"),
        None => "Other".to_string(),
    };
    Fault { kind, detail: format!("{}", error.root_cause()).lines().next().unwrap_or_default().to_string() }
}

impl CompiledInstance {
    fn new(mut store: Store<()>, instance: WasmInstance, metered: bool) -> Result<CompiledInstance, String> {
        let memory = instance.get_memory(&mut store, "memory").ok_or("the module exports no memory")?;
        let arena = instance.get_typed_func::<u32, u32>(&mut store, "arena").map_err(|error| format!("resolving arena: {error}"))?;
        let mut arenas = [0; ARENAS];
        for (index, address) in arenas.iter_mut().enumerate() {
            *address = arena.call(&mut store, index as u32).map_err(|error| format!("calling arena: {error}"))? as usize;
        }
        macro_rules! export {
            ($name:literal) => {
                instance.get_typed_func(&mut store, $name).map_err(|error| format!("resolving {}: {error}", $name))?
            };
        }
        Ok(CompiledInstance {
            spatial_hashing: export!("spatial_hashing"),
            chunk_systems: export!("chunk_systems"),
            structural_changes: export!("structural_changes"),
            call_nop: export!("call_nop"),
            make_nans: export!("make_nans"),
            dive: export!("dive"),
            store,
            memory,
            arenas,
            metered,
        })
    }
}

impl Instance for CompiledInstance {
    fn write(&mut self, arena: usize, offset: usize, bytes: &[u8]) {
        let at = self.arenas[arena] + offset;
        self.memory.data_mut(&mut self.store)[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn read(&mut self, arena: usize, offset: usize, out: &mut [u8]) {
        let at = self.arenas[arena] + offset;
        out.copy_from_slice(&self.memory.data(&self.store)[at..at + out.len()]);
    }

    fn spatial_hashing(&mut self, count: u32) -> Result<(), Fault> {
        self.spatial_hashing.call(&mut self.store, count).map_err(fault)
    }

    fn chunk_systems(&mut self, start: u32, count: u32, population: u32, tick: u32, wander: u32, fault_kind: u32) -> Result<f32, Fault> {
        self.chunk_systems.call(&mut self.store, (start, count, population, tick, wander, fault_kind)).map_err(fault)
    }

    fn structural_changes(&mut self, count: u32, stream_state: u64) -> Result<u64, Fault> {
        self.structural_changes.call(&mut self.store, (count, stream_state)).map_err(fault)
    }

    fn call_nop(&mut self, times: u32) -> Result<(), Fault> {
        self.call_nop.call(&mut self.store, times).map_err(fault)
    }

    fn make_nans(&mut self) -> Result<[u32; 4], Fault> {
        self.make_nans.call(&mut self.store, (0.0, f32::INFINITY, -1.0)).map_err(fault)?;
        let mut raw = [0_u8; 16];
        self.read(OUT_X, 0, &mut raw);
        Ok(std::array::from_fn(|index| u32::from_le_bytes(raw[index * 4..index * 4 + 4].try_into().expect("four bytes"))))
    }

    fn dive(&mut self, depth: u32) -> Result<u32, Fault> {
        self.dive.call(&mut self.store, (depth, 1)).map_err(fault)
    }

    fn set_fuel(&mut self, fuel: u64) {
        if self.metered {
            self.store.set_fuel(fuel).expect("fuel is metered");
        }
    }

    fn fuel(&self) -> Option<u64> {
        self.metered.then(|| self.store.get_fuel().expect("fuel is metered"))
    }

    fn memory_bytes(&self) -> u64 {
        self.memory.data_size(&self.store) as u64
    }
}
