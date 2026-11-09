//! The world the spike steps: the reference workload's columns in the host,
//! every gameplay system of the tick run through an instance, the per-chunk
//! systems in parallel on worker threads that each own one instance. Throwaway,
//! never product code.
//!
//! One tick: the serial instance builds the cell lists; the workers claim
//! chunks, each call reading the columns copied into its instance and writing
//! four columns the host copies out; the host applies the written columns and
//! adds the chunk energies in chunk order; the serial instance applies the
//! structural changes. The state hash is the host's.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use reference_workload::{CHUNK, Fnv1a, Parameters, State};
use tick_systems::CELLS;

use crate::instance::{ALIVE, AMPLE_FUEL, CAPACITY, CELL_START, Fault, GENERATION, Instance, MASS, OUT_VX, OUT_VY, OUT_X, OUT_Y, Runtime, SLOTS, VX, VY, X, Y, bytes, bytes_mut};

/// Where the wander system takes its cosine and sine from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wander {
    Off,
    CompiledIn,
    HostFunctions,
}

impl Wander {
    pub fn name(self) -> &'static str {
        match self {
            Wander::Off => "off",
            Wander::CompiledIn => "compiled into the module",
            Wander::HostFunctions => "host functions",
        }
    }

    fn code(self) -> u32 {
        match self {
            Wander::Off => 0,
            Wander::CompiledIn => 1,
            Wander::HostFunctions => 2,
        }
    }
}

/// A fault the host asks the module to raise: at `tick`, in `chunk`, of the
/// module's kind 1 (a read outside the memory) or 2 (a panic).
#[derive(Clone, Copy, Debug)]
pub struct Injection {
    pub tick: u32,
    pub chunk: usize,
    pub kind: u32,
}

/// Why a world stopped: the tick it could not complete, the lowest failing
/// chunk (none for a serial system), the fault, and the fuel the failing call
/// had consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub tick: u32,
    pub chunk: Option<usize>,
    pub fault: Fault,
    pub fuel_consumed: Option<u64>,
}

struct ChunkResult {
    x: [f32; CHUNK],
    y: [f32; CHUNK],
    vx: [f32; CHUNK],
    vy: [f32; CHUNK],
    energy: f32,
    fuel: u64,
    fault: Option<Fault>,
}

struct TickJob {
    population: usize,
    tick: u32,
    wander: u32,
    budget: u64,
    injection: Option<Injection>,
    alive: Vec<u8>,
    columns: [Vec<f32>; 5],
    cell_start: Vec<u32>,
    slots: Vec<u32>,
    next: AtomicUsize,
    results: Vec<Mutex<ChunkResult>>,
}

enum Command {
    Tick(Arc<TickJob>),
    Load(Arc<dyn Runtime>),
    Stop,
}

/// Processor time the workers spent, summed over them.
#[derive(Default)]
pub struct Counters {
    pub copy_nanos: AtomicU64,
    pub call_nanos: AtomicU64,
}

fn worker(runtime: Arc<dyn Runtime>, commands: Receiver<Command>, done: Sender<Result<(), String>>, counters: Arc<Counters>) {
    let mut instance = runtime.instantiate();
    let _ = done.send(instance.as_ref().map(|_| ()).map_err(Clone::clone));
    for command in commands {
        match command {
            Command::Stop => break,
            Command::Load(runtime) => {
                instance = runtime.instantiate();
                let _ = done.send(instance.as_ref().map(|_| ()).map_err(Clone::clone));
            }
            Command::Tick(job) => {
                if let Ok(instance) = instance.as_mut() {
                    run_chunks(instance.as_mut(), &job, &counters);
                }
                let _ = done.send(Ok(()));
            }
        }
    }
}

fn run_chunks(instance: &mut dyn Instance, job: &TickJob, counters: &Counters) {
    let chunks = job.results.len();
    let mut copied = 0_u64;
    let mut called = 0_u64;
    let mut first = true;
    loop {
        let chunk = job.next.fetch_add(1, Ordering::Relaxed);
        if chunk >= chunks {
            break;
        }
        let started = Instant::now();
        if first {
            // The columns every chunk of this tick reads, copied once per worker.
            instance.write(ALIVE, 0, &job.alive);
            for (arena, column) in [X, Y, VX, VY, MASS].into_iter().zip(&job.columns) {
                instance.write(arena, 0, bytes(column));
            }
            instance.write(CELL_START, 0, bytes(&job.cell_start));
            instance.write(SLOTS, 0, bytes(&job.slots));
            first = false;
        }
        let start = chunk * CHUNK;
        let count = CHUNK.min(job.population - start);
        let fault = match job.injection {
            Some(injection) if injection.tick == job.tick && injection.chunk == chunk => injection.kind,
            _ => 0,
        };
        instance.set_fuel(job.budget);
        copied += started.elapsed().as_nanos() as u64;
        let calling = Instant::now();
        let outcome = instance.chunk_systems(start as u32, count as u32, job.population as u32, job.tick, job.wander, fault);
        called += calling.elapsed().as_nanos() as u64;
        let copying = Instant::now();
        let mut result = job.results[chunk].lock().expect("no worker panics while holding a result");
        result.fuel = instance.fuel().map_or(0, |left| job.budget - left);
        match outcome {
            Ok(energy) => {
                result.energy = energy;
                instance.read(OUT_X, 0, bytes_mut(&mut result.x[..count]));
                instance.read(OUT_Y, 0, bytes_mut(&mut result.y[..count]));
                instance.read(OUT_VX, 0, bytes_mut(&mut result.vx[..count]));
                instance.read(OUT_VY, 0, bytes_mut(&mut result.vy[..count]));
            }
            Err(fault) => result.fault = Some(fault),
        }
        drop(result);
        copied += copying.elapsed().as_nanos() as u64;
    }
    counters.copy_nanos.fetch_add(copied, Ordering::Relaxed);
    counters.call_nanos.fetch_add(called, Ordering::Relaxed);
}

pub struct World {
    population: usize,
    tick: u32,
    wander: Wander,
    /// The fuel given to every chunk call.
    pub chunk_budget: u64,
    pub injection: Option<Injection>,
    generation: Vec<u32>,
    alive: Vec<u8>,
    /// x, y, vx, vy, mass.
    columns: [Vec<f32>; 5],
    energy: f32,
    stream_state: u64,
    cell_start: Vec<u32>,
    slots: Vec<u32>,
    serial: Box<dyn Instance>,
    commands: Vec<Sender<Command>>,
    done: Receiver<Result<(), String>>,
    threads: Vec<JoinHandle<()>>,
    pub counters: Arc<Counters>,
    /// The fuel every call of a tick consumed, one total per tick.
    pub fuel_per_tick: Vec<u64>,
    pub largest_chunk_fuel: u64,
    pub largest_serial_fuel: u64,
    /// Processor time of the serial systems and of the host's copies around them.
    pub serial_nanos: u64,
    failure: Option<Failure>,
}

impl World {
    pub fn new(runtime: &Arc<dyn Runtime>, parameters: &Parameters, workers: usize, wander: Wander) -> Result<World, String> {
        let state = State::initialize(parameters);
        let population = state.len();
        assert!(population <= CAPACITY, "the instances hold {CAPACITY} slots");
        let counters = Arc::new(Counters::default());
        let (done_sender, done) = channel();
        let mut commands = Vec::new();
        let mut threads = Vec::new();
        for _ in 0..workers {
            let (sender, receiver) = channel();
            let (runtime, done_sender, counters) = (Arc::clone(runtime), done_sender.clone(), Arc::clone(&counters));
            threads.push(std::thread::spawn(move || worker(runtime, receiver, done_sender, counters)));
            commands.push(sender);
        }
        let mut world = World {
            population,
            tick: 0,
            wander,
            chunk_budget: AMPLE_FUEL,
            injection: None,
            generation: state.generation,
            alive: state.alive.iter().map(|alive| u8::from(*alive)).collect(),
            columns: [state.x, state.y, state.vx, state.vy, state.mass],
            energy: state.energy,
            stream_state: state.rng.state,
            cell_start: vec![0; CELLS + 1],
            slots: vec![0; population],
            serial: runtime.instantiate()?,
            commands,
            done,
            threads,
            counters,
            fuel_per_tick: Vec::new(),
            largest_chunk_fuel: 0,
            largest_serial_fuel: 0,
            serial_nanos: 0,
            failure: None,
        };
        world.wait_for_workers()?;
        Ok(world)
    }

    fn wait_for_workers(&mut self) -> Result<(), String> {
        let mut outcome = Ok(());
        for _ in 0..self.commands.len() {
            if let Err(error) = self.done.recv().map_err(|error| format!("a worker stopped: {error}"))? {
                outcome = Err(error);
            }
        }
        outcome
    }

    /// Replaces the module: the serial instance and every worker's instance
    /// are created again from `runtime` before the next tick.
    pub fn load(&mut self, runtime: &Arc<dyn Runtime>) -> Result<(), String> {
        self.serial = runtime.instantiate()?;
        for sender in &self.commands {
            sender.send(Command::Load(Arc::clone(runtime))).map_err(|error| format!("a worker stopped: {error}"))?;
        }
        self.wait_for_workers()
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    pub fn memory_bytes(&self) -> u64 {
        self.serial.memory_bytes()
    }

    fn serial_call<T>(&mut self, call: impl FnOnce(&mut dyn Instance) -> Result<T, Fault>) -> Result<(T, u64), Failure> {
        self.serial.set_fuel(AMPLE_FUEL);
        let outcome = call(self.serial.as_mut());
        let fuel = self.serial.fuel().map_or(0, |left| AMPLE_FUEL - left);
        self.largest_serial_fuel = self.largest_serial_fuel.max(fuel);
        match outcome {
            Ok(value) => Ok((value, fuel)),
            Err(fault) => Err(Failure { tick: self.tick + 1, chunk: None, fault, fuel_consumed: self.serial.fuel().map(|_| fuel) }),
        }
    }

    /// Steps one tick. A fault leaves the world at the last completed tick and
    /// every later step is refused with the same failure.
    pub fn step(&mut self) -> Result<(), Failure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        match self.try_step() {
            Ok(()) => Ok(()),
            Err(failure) => {
                self.failure = Some(failure.clone());
                Err(failure)
            }
        }
    }

    fn try_step(&mut self) -> Result<(), Failure> {
        let tick = self.tick + 1;
        let n = self.population;
        let mut tick_fuel = 0_u64;

        let serial = Instant::now();
        self.serial.write(ALIVE, 0, &self.alive);
        self.serial.write(X, 0, bytes(&self.columns[0]));
        self.serial.write(Y, 0, bytes(&self.columns[1]));
        let ((), fuel) = self.serial_call(|instance| instance.spatial_hashing(n as u32))?;
        tick_fuel += fuel;
        self.serial.read(CELL_START, 0, bytes_mut(&mut self.cell_start));
        self.serial.read(SLOTS, 0, bytes_mut(&mut self.slots));
        self.serial_nanos += serial.elapsed().as_nanos() as u64;

        let chunks = n.div_ceil(CHUNK);
        let job = Arc::new(TickJob {
            population: n,
            tick,
            wander: self.wander.code(),
            budget: self.chunk_budget,
            injection: self.injection,
            alive: self.alive.clone(),
            columns: self.columns.clone(),
            cell_start: self.cell_start.clone(),
            slots: self.slots.clone(),
            next: AtomicUsize::new(0),
            results: (0..chunks)
                .map(|_| Mutex::new(ChunkResult { x: [0.0; CHUNK], y: [0.0; CHUNK], vx: [0.0; CHUNK], vy: [0.0; CHUNK], energy: 0.0, fuel: 0, fault: None }))
                .collect(),
        });
        for sender in &self.commands {
            sender.send(Command::Tick(Arc::clone(&job))).expect("the workers live as long as the world");
        }
        for _ in 0..self.commands.len() {
            self.done.recv().expect("the workers live as long as the world").expect("a tick reports no instantiation");
        }

        // The lowest failing chunk is the tick's failure, whichever worker ran it.
        for (chunk, result) in job.results.iter().enumerate() {
            let result = result.lock().expect("no worker panics while holding a result");
            if let Some(fault) = &result.fault {
                let metered = self.serial.fuel().is_some();
                return Err(Failure { tick, chunk: Some(chunk), fault: fault.clone(), fuel_consumed: metered.then_some(result.fuel) });
            }
        }
        let mut energy = 0.0_f32;
        for (chunk, result) in job.results.iter().enumerate() {
            let result = result.lock().expect("no worker panics while holding a result");
            let start = chunk * CHUNK;
            let count = CHUNK.min(n - start);
            self.columns[0][start..start + count].copy_from_slice(&result.x[..count]);
            self.columns[1][start..start + count].copy_from_slice(&result.y[..count]);
            self.columns[2][start..start + count].copy_from_slice(&result.vx[..count]);
            self.columns[3][start..start + count].copy_from_slice(&result.vy[..count]);
            energy += result.energy;
            tick_fuel += result.fuel;
            self.largest_chunk_fuel = self.largest_chunk_fuel.max(result.fuel);
        }
        self.energy = energy;

        let serial = Instant::now();
        self.serial.write(GENERATION, 0, bytes(&self.generation));
        self.serial.write(ALIVE, 0, &self.alive);
        for (arena, column) in [X, Y, VX, VY, MASS].into_iter().zip(&self.columns) {
            self.serial.write(arena, 0, bytes(column));
        }
        let stream_state = self.stream_state;
        let (stream_state, fuel) = self.serial_call(|instance| instance.structural_changes(n as u32, stream_state))?;
        tick_fuel += fuel;
        self.stream_state = stream_state;
        self.serial.read(GENERATION, 0, bytes_mut(&mut self.generation));
        self.serial.read(ALIVE, 0, &mut self.alive);
        for (arena, column) in [X, Y, VX, VY, MASS].into_iter().zip(&mut self.columns) {
            self.serial.read(arena, 0, bytes_mut(column));
        }
        self.serial_nanos += serial.elapsed().as_nanos() as u64;

        self.fuel_per_tick.push(tick_fuel);
        self.tick = tick;
        Ok(())
    }

    /// The tick hash of the reference workload, over the same canonical bytes.
    pub fn hash(&self) -> u64 {
        let mut hasher = Fnv1a::new();
        let [x, y, vx, vy, mass] = &self.columns;
        for slot in 0..self.population {
            hasher.write(&self.generation[slot].to_le_bytes());
            hasher.write(&[self.alive[slot]]);
            if self.alive[slot] != 0 {
                for column in [x, y, vx, vy, mass] {
                    hasher.write(&column[slot].to_le_bytes());
                }
            }
        }
        hasher.write(&self.energy.to_le_bytes());
        hasher.write(&self.stream_state.to_le_bytes());
        hasher.finish()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        for sender in &self.commands {
            let _ = sender.send(Command::Stop);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
