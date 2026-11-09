//! The spike's self-checks: the world steps the reference workload to the
//! sequential reference's hashes through the native baseline, and through the
//! module under both runtimes. Throwaway, never product code.

use std::fs;
use std::sync::Arc;

use reference_workload::{Parameters, Size, run_sequential};
use scripting_runtime::builds;
use scripting_runtime::compiled::Compiled;
use scripting_runtime::instance::{Runtime, native};
use scripting_runtime::interpreted::Interpreted;
use scripting_runtime::run_fresh;
use scripting_runtime::world::Wander;

#[test]
fn the_native_baseline_gives_the_reference_hashes() {
    let baseline = native(tick_systems::DAMPING);
    for (wander, wander_on) in [(Wander::Off, false), (Wander::HostFunctions, true), (Wander::CompiledIn, true)] {
        let parameters = Parameters::new(Size::Small, 30, wander_on);
        let reference = run_sequential(&parameters);
        for workers in [1, 3] {
            let (hashes, _) = run_fresh(&baseline, &parameters, workers, wander).expect("the native baseline runs");
            assert_eq!(hashes, reference.tick_hashes, "{} workers, wander {}", workers, wander.name());
        }
    }
}

#[test]
fn the_module_gives_the_reference_hashes_under_both_runtimes() {
    let scratch = builds::scratch("self-check");
    let build = builds::build_module(&builds::workspace_root(), &scratch, false);
    let wasm = build.wasm.expect("the module builds");
    let compiled: Arc<dyn Runtime> = Arc::new(Compiled::new(&wasm, true, false).expect("the module compiles"));
    let interpreted: Arc<dyn Runtime> = Arc::new(Interpreted::new(&wasm, true, true).expect("the module translates"));

    let parameters = Parameters::new(Size::Small, 20, false);
    let reference = run_sequential(&parameters);
    for runtime in [&compiled, &interpreted] {
        let (hashes, world) = run_fresh(runtime, &parameters, 2, Wander::Off).expect("the module runs");
        assert_eq!(hashes, reference.tick_hashes, "{}", runtime.name());
        assert!(world.fuel_per_tick.iter().all(|fuel| *fuel > 0), "{} meters fuel", runtime.name());
    }

    // The module's own cosine and sine are one implementation under both runtimes.
    let parameters = Parameters::new(Size::Small, 20, true);
    let (from_compiled, _) = run_fresh(&compiled, &parameters, 2, Wander::CompiledIn).expect("the module runs");
    let (from_interpreted, _) = run_fresh(&interpreted, &parameters, 2, Wander::CompiledIn).expect("the module runs");
    assert_eq!(from_compiled, from_interpreted);
    let _ = fs::remove_dir_all(scratch);
}
