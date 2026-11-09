//! The scripting runtime spike, for the scripting and gameplay model research:
//! every gameplay system of the reference workload's tick run as a WebAssembly
//! module under a compiling runtime and an interpreter. Throwaway, never
//! product code.

pub mod builds;
pub mod compiled;
pub mod instance;
pub mod interpreted;
pub mod world;

use reference_workload::Parameters;

use crate::instance::Runtime;
use crate::world::{Failure, Wander, World};

/// The tick hashes `h_0` to `h_T` of a world stepped `ticks` times, or the
/// hashes up to its failure with the failure. `at` is called before each tick
/// with the tick about to run, so that a scenario can load a module or change
/// a budget at a tick boundary.
pub fn run(world: &mut World, ticks: u32, mut at: impl FnMut(&mut World, u32)) -> (Vec<u64>, Option<Failure>) {
    let mut hashes = vec![world.hash()];
    for tick in 1..=ticks {
        at(world, tick);
        if let Err(failure) = world.step() {
            return (hashes, Some(failure));
        }
        hashes.push(world.hash());
    }
    (hashes, None)
}

/// A fresh world on `runtime`, stepped `ticks` times without interference.
pub fn run_fresh(runtime: &std::sync::Arc<dyn Runtime>, parameters: &Parameters, workers: usize, wander: Wander) -> Result<(Vec<u64>, World), String> {
    let mut world = World::new(runtime, parameters, workers, wander)?;
    let (hashes, failure) = run(&mut world, parameters.tick_count, |_, _| {});
    match failure {
        Some(failure) => Err(format!("tick {} failed: {} ({})", failure.tick, failure.fault.kind, failure.fault.detail)),
        None => Ok((hashes, world)),
    }
}
