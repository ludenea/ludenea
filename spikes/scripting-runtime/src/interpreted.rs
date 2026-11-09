//! The interpreter: the same module run by wasmi, built with its
//! deterministic profile. Throwaway, never product code.

use wasmi::{CompilationMode, Config, Engine, Instance as WasmInstance, Linker, Memory, Module, Store, TypedFunc};

use crate::instance::{AMPLE_FUEL, ARENAS, Fault, Instance, OUT_X, Runtime};

pub struct Interpreted {
    engine: Engine,
    module: Module,
    linker: Linker<()>,
    metered: bool,
    eager: bool,
}

impl Interpreted {
    /// Translates `wasm`. `eager` translates every function when the module
    /// is created; otherwise a function is translated at its first call, and
    /// that call pays the translation in fuel.
    pub fn new(wasm: &[u8], metered: bool, eager: bool) -> Result<Interpreted, String> {
        let mut config = Config::default();
        config.consume_fuel(metered);
        config.compilation_mode(if eager { CompilationMode::Eager } else { CompilationMode::LazyTranslation });
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm).map_err(|error| format!("translating the module: {error}"))?;
        let mut linker = Linker::new(&engine);
        let wrapped = linker
            .func_wrap("host", "host_cos", |angle: f32| angle.cos())
            .and_then(|linker| linker.func_wrap("host", "host_sin", |angle: f32| angle.sin()))
            .and_then(|linker| linker.func_wrap("host", "host_nop", || {}))
            .map(|_| ());
        wrapped.map_err(|error| format!("defining the host functions: {error}"))?;
        Ok(Interpreted { engine, module, linker, metered, eager })
    }
}

impl Runtime for Interpreted {
    fn name(&self) -> &'static str {
        match (self.metered, self.eager) {
            (true, true) => "wasmi, fuel on",
            (false, true) => "wasmi, fuel off",
            (true, false) => "wasmi, fuel on, functions translated at their first call",
            (false, false) => "wasmi, fuel off, functions translated at their first call",
        }
    }

    fn metered(&self) -> bool {
        self.metered
    }

    fn instantiate(&self) -> Result<Box<dyn Instance>, String> {
        let mut store = Store::new(&self.engine, ());
        if self.metered {
            store.set_fuel(AMPLE_FUEL).map_err(|error| format!("setting fuel: {error}"))?;
        }
        let instance = self.linker.instantiate_and_start(&mut store, &self.module).map_err(|error| format!("instantiating: {error}"))?;
        InterpretedInstance::new(store, instance, self.metered).map(|instance| Box::new(instance) as Box<dyn Instance>)
    }
}

struct InterpretedInstance {
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

fn fault(error: wasmi::Error) -> Fault {
    let kind = match error.as_trap_code() {
        Some(code) => format!("{code:?}"),
        None => "Other".to_string(),
    };
    Fault { kind, detail: format!("{error}").lines().next().unwrap_or_default().to_string() }
}

impl InterpretedInstance {
    fn new(mut store: Store<()>, instance: WasmInstance, metered: bool) -> Result<InterpretedInstance, String> {
        let memory = instance.get_memory(&store, "memory").ok_or("the module exports no memory")?;
        let arena = instance.get_typed_func::<u32, u32>(&store, "arena").map_err(|error| format!("resolving arena: {error}"))?;
        let mut arenas = [0; ARENAS];
        for (index, address) in arenas.iter_mut().enumerate() {
            *address = arena.call(&mut store, index as u32).map_err(|error| format!("calling arena: {error}"))? as usize;
        }
        macro_rules! export {
            ($name:literal) => {
                instance.get_typed_func(&store, $name).map_err(|error| format!("resolving {}: {error}", $name))?
            };
        }
        Ok(InterpretedInstance {
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

impl Instance for InterpretedInstance {
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
        self.memory.data(&self.store).len() as u64
    }
}
