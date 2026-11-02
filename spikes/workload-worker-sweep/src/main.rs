//! Worker sweep of the reference workload for the determinism research.
//!
//! Throwaway spike, never product code. It runs the reference workload three
//! ways and compares their tick hashes:
//!
//! - `sequential`: the reference implementation, the comparison point;
//! - `deterministic`: a parallel implementation whose constructs are
//!   deterministic by construction: slots are processed in whatever order the
//!   workers claim their chunks, each slot's result depends on its own fixed
//!   visit order only, and the energy is reduced per chunk, the chunk sums
//!   being added in chunk order on the main thread;
//! - `free-running`: the same parallel implementation with the one construct
//!   a non-deterministic scheduler would use instead: every worker adds the
//!   energies it meets in arrival order, and the worker totals are merged in
//!   completion order.
//!
//! Each parallel mode runs at 1, 2, 4 and 8 workers, with the wander system
//! off and on. The report carries, per run, the run hash, the first tick at
//! which it diverges from the sequential run with the same parameters, the
//! throughput after warm-up and the time spent per phase.
//!
//! The pool is a persistent set of threads (the main thread counts as one
//! worker) synchronized by a barrier at both ends of each parallel pass;
//! chunks of 256 slots are claimed through an atomic counter; a pass writes
//! into per-chunk output buffers that the main thread copies back between
//! passes, so the state itself is only ever written by one thread at a time.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex, RwLock};
use std::time::Instant;

use reference_workload::{
    CHUNK, DEFAULT_TICK_COUNT, Grid, Json, Parameters, Run, Size, State, Timing, WARM_UP_TICKS,
    bounced, chunk_range, first_divergent_tick, integrated, kinetic_energy, repelled_velocity,
    repulsion_force, run_hash, run_sequential, wandered_velocity, write_platform_and_build,
    write_run_core,
};

const SCHEMA_VERSION: u64 = 1;
const WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
const PHASES: [&str; 6] = [
    "spatial_hashing",
    "repulsion_pass",
    "motion_pass",
    "copy_back",
    "structural_changes",
    "hash",
];

const PASS_IDLE: u8 = 0;
const PASS_REPULSION: u8 = 1;
const PASS_MOTION: u8 = 2;
const PASS_STOP: u8 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reduction {
    Deterministic,
    FreeRunning,
}

impl Reduction {
    fn name(self) -> &'static str {
        match self {
            Reduction::Deterministic => "deterministic",
            Reduction::FreeRunning => "free-running",
        }
    }
}

/// The outputs one chunk produces in a pass; copied back by the main thread.
struct ChunkOutput {
    x: Vec<f32>,
    y: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    energy: f32,
}

struct Shared {
    state: RwLock<State>,
    grid: RwLock<Grid>,
    outputs: Vec<Mutex<ChunkOutput>>,
    next_chunk: AtomicUsize,
    pass: AtomicU8,
    barrier: Barrier,
    tick: AtomicUsize,
    wander: bool,
    reduction: Reduction,
    free_running_energy: Mutex<f32>,
}

impl Shared {
    fn new(state: State, workers: usize, wander: bool, reduction: Reduction) -> Shared {
        let outputs = (0..state.chunk_count())
            .map(|_| {
                Mutex::new(ChunkOutput {
                    x: vec![0.0; CHUNK],
                    y: vec![0.0; CHUNK],
                    vx: vec![0.0; CHUNK],
                    vy: vec![0.0; CHUNK],
                    energy: 0.0,
                })
            })
            .collect();
        Shared {
            state: RwLock::new(state),
            grid: RwLock::new(Grid::new()),
            outputs,
            next_chunk: AtomicUsize::new(0),
            pass: AtomicU8::new(PASS_IDLE),
            barrier: Barrier::new(workers),
            tick: AtomicUsize::new(0),
            wander,
            reduction,
            free_running_energy: Mutex::new(0.0),
        }
    }

    /// Claims chunks until none is left and processes each one.
    fn work(&self, pass: u8) {
        let state = self.state.read().unwrap();
        let grid = self.grid.read().unwrap();
        let tick = self.tick.load(Ordering::Relaxed) as u32;
        let mut arrival_energy = 0.0_f32;
        loop {
            let chunk = self.next_chunk.fetch_add(1, Ordering::Relaxed);
            if chunk >= self.outputs.len() {
                break;
            }
            let range = chunk_range(chunk, state.len());
            let mut out = self.outputs[chunk].lock().unwrap();
            match pass {
                PASS_REPULSION => {
                    for i in range {
                        if state.alive[i] {
                            let (fx, fy) = repulsion_force(&state, &grid, i);
                            out.vx[i % CHUNK] = repelled_velocity(state.vx[i], fx, state.mass[i]);
                            out.vy[i % CHUNK] = repelled_velocity(state.vy[i], fy, state.mass[i]);
                        }
                    }
                }
                PASS_MOTION => {
                    let mut chunk_energy = 0.0_f32;
                    for i in range {
                        if !state.alive[i] {
                            continue;
                        }
                        let (mut vx, mut vy) = (state.vx[i], state.vy[i]);
                        if self.wander {
                            (vx, vy) = wandered_velocity(vx, vy, tick, i);
                        }
                        let (x, vx) = bounced(integrated(state.x[i], vx), vx);
                        let (y, vy) = bounced(integrated(state.y[i], vy), vy);
                        let k = i % CHUNK;
                        out.x[k] = x;
                        out.y[k] = y;
                        out.vx[k] = vx;
                        out.vy[k] = vy;
                        let e = kinetic_energy(vx, vy, state.mass[i]);
                        match self.reduction {
                            Reduction::Deterministic => chunk_energy += e,
                            Reduction::FreeRunning => arrival_energy += e,
                        }
                    }
                    out.energy = chunk_energy;
                }
                _ => unreachable!("workers only run the repulsion and motion passes"),
            }
        }
        if pass == PASS_MOTION && self.reduction == Reduction::FreeRunning {
            *self.free_running_energy.lock().unwrap() += arrival_energy;
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

    /// Runs one parallel pass on every worker, the calling thread included.
    fn run_pass(&self, pass: u8) {
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

    fn copy_back(&self, pass: u8) {
        let mut state = self.state.write().unwrap();
        for (chunk, output) in self.outputs.iter().enumerate() {
            let out = output.lock().unwrap();
            for i in chunk_range(chunk, state.len()) {
                if state.alive[i] {
                    let k = i % CHUNK;
                    state.vx[i] = out.vx[k];
                    state.vy[i] = out.vy[k];
                    if pass == PASS_MOTION {
                        state.x[i] = out.x[k];
                        state.y[i] = out.y[k];
                    }
                }
            }
        }
    }

    /// One tick, driven by the main thread; the hash is taken by the caller.
    fn tick(&self, tick: u32, timing: &mut Timing) {
        self.tick.store(tick as usize, Ordering::Relaxed);
        timing.charge("spatial_hashing", || {
            let state = self.state.read().unwrap();
            self.grid.write().unwrap().build(&state);
        });
        timing.charge("repulsion_pass", || self.run_pass(PASS_REPULSION));
        timing.charge("copy_back", || self.copy_back(PASS_REPULSION));
        *self.free_running_energy.lock().unwrap() = 0.0;
        timing.charge("motion_pass", || self.run_pass(PASS_MOTION));
        timing.charge("copy_back", || self.copy_back(PASS_MOTION));
        timing.charge("structural_changes", || {
            let mut state = self.state.write().unwrap();
            state.energy = match self.reduction {
                Reduction::Deterministic => {
                    let mut total = 0.0_f32;
                    for output in &self.outputs {
                        total += output.lock().unwrap().energy;
                    }
                    total
                }
                Reduction::FreeRunning => *self.free_running_energy.lock().unwrap(),
            };
            state.structural_changes();
        });
    }

    fn hash(&self) -> u64 {
        self.state.read().unwrap().hash()
    }
}

/// A parallel run at `workers` workers: a warm-up instance of
/// `WARM_UP_TICKS` ticks, then the measured instance of `T` ticks.
fn run_parallel(parameters: &Parameters, workers: usize, reduction: Reduction) -> Run {
    let warm_up = Shared::new(State::initialize(parameters), workers, parameters.wander, reduction);
    std::thread::scope(|scope| {
        for _ in 1..workers {
            scope.spawn(|| warm_up.worker_loop());
        }
        let mut discarded = Timing::new(&PHASES);
        for t in 1..=WARM_UP_TICKS {
            warm_up.tick(t, &mut discarded);
        }
        warm_up.stop();
    });

    let shared = Shared::new(State::initialize(parameters), workers, parameters.wander, reduction);
    let mut timing = Timing::new(&PHASES);
    let mut tick_hashes = Vec::with_capacity(parameters.tick_count as usize + 1);
    std::thread::scope(|scope| {
        for _ in 1..workers {
            scope.spawn(|| shared.worker_loop());
        }
        let start = Instant::now();
        tick_hashes.push(timing.charge("hash", || shared.hash()));
        for t in 1..=parameters.tick_count {
            shared.tick(t, &mut timing);
            tick_hashes.push(timing.charge("hash", || shared.hash()));
        }
        timing.total = start.elapsed();
        shared.stop();
    });
    let run_hash = run_hash(&tick_hashes);
    Run { parameters: *parameters, tick_hashes, run_hash, timing }
}

fn write_run(out: &mut Json, mode: &str, workers: usize, run: &Run, reference: &Run) {
    out.object_start();
    out.string("mode", mode);
    out.u64("workers", workers as u64);
    write_run_core(out, run);
    let divergence = first_divergent_tick(&run.tick_hashes, &reference.tick_hashes);
    out.bool("matches_sequential", divergence.is_none());
    out.i64("first_divergent_tick", divergence.map_or(-1, |t| t as i64));
    out.object_end();
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", "workload-worker-sweep");
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.key("runs");
    out.array_start();
    for wander in [false, true] {
        let parameters = Parameters::new(Size::Default, DEFAULT_TICK_COUNT, wander);
        let reference = run_sequential(&parameters);
        write_run(&mut out, "sequential", 1, &reference, &reference);
        for reduction in [Reduction::Deterministic, Reduction::FreeRunning] {
            for workers in WORKER_COUNTS {
                let run = run_parallel(&parameters, workers, reduction);
                write_run(&mut out, reduction.name(), workers, &run, &reference);
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

    #[test]
    fn deterministic_runs_match_the_sequential_reference_at_every_worker_count() {
        for wander in [false, true] {
            let reference = run_sequential(&small(wander));
            for workers in [1, 2, 3, 8] {
                let run = run_parallel(&small(wander), workers, Reduction::Deterministic);
                assert_eq!(
                    first_divergent_tick(&run.tick_hashes, &reference.tick_hashes),
                    None,
                    "{workers} workers, wander {wander}"
                );
                assert_eq!(run.run_hash, reference.run_hash);
            }
        }
    }

    #[test]
    fn a_free_running_run_differs_from_the_reference_only_through_its_energy() {
        let reference = run_sequential(&small(false));
        let run = run_parallel(&small(false), 3, Reduction::FreeRunning);
        let mut reference_state = State::initialize(&small(false));
        let shared = Shared::new(State::initialize(&small(false)), 1, false, Reduction::FreeRunning);
        let mut grid = Grid::new();
        let mut timing = Timing::new(&reference_workload::SEQUENTIAL_PHASES);
        let mut parallel_timing = Timing::new(&PHASES);
        for t in 1..=5 {
            reference_workload::tick(&mut reference_state, &mut grid, t, false, &mut timing);
            shared.tick(t, &mut parallel_timing);
            let state = shared.state.read().unwrap();
            assert_eq!(state.x, reference_state.x);
            assert_eq!(state.vx, reference_state.vx);
            assert_eq!(state.alive, reference_state.alive);
            assert_eq!(state.rng.state, reference_state.rng.state);
        }
        assert_eq!(run.initial_hash(), reference.initial_hash());
        assert_eq!(run.tick_hashes.len(), reference.tick_hashes.len());
    }

    #[test]
    fn the_pool_handles_more_workers_than_chunks() {
        let parameters = Parameters::new(Size::Small, 3, false);
        assert_eq!(State::initialize(&parameters).chunk_count(), 4);
        let reference = run_sequential(&parameters);
        let run = run_parallel(&parameters, 16, Reduction::Deterministic);
        assert_eq!(run.run_hash, reference.run_hash);
    }
}
