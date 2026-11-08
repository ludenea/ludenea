//! Self-checks of the plugin tiers spike on the small size over 40 ticks:
//! every tier that builds on this machine reproduces the sequential reference
//! at 1, 2 and 3 workers, a reload changes no hash, and the importer answers
//! the expected bytes. The module and native tiers are built here as the
//! spike builds them at run time; a machine without the WebAssembly target
//! (offline) or without CMake and a C compiler skips those tiers with a
//! printed reason.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use plugin_tiers::builds;
use plugin_tiers::importer;
use plugin_tiers::module::WasmModule;
use plugin_tiers::tiers::{CompileTime, DynamicLibrary};
use plugin_tiers::world::{Motion, MotionPass, Parameters, Size, World, WorldConfig};
use reference_workload::{first_divergent_tick, run_sequential};

const TICKS: u32 = 40;
const WORKERS: [usize; 3] = [1, 2, 3];

fn reference() -> Vec<u64> {
    run_sequential(&Parameters::new(Size::Small, TICKS, false)).tick_hashes
}

fn divergence(motion: &Motion, workers: usize, reload_at: Option<u32>, reload: &dyn Fn()) -> Option<usize> {
    let mut world = World::new(WorldConfig::new(Size::Small, workers), motion.clone());
    match reload_at {
        Some(tick) => {
            world.step(tick).unwrap();
            reload();
            world.step(TICKS - tick).unwrap();
        }
        None => world.step(TICKS).unwrap(),
    }
    first_divergent_tick(&world.hashes(), &reference())
}

#[test]
fn the_built_in_pass_and_the_compile_time_crate_reproduce_the_reference() {
    for workers in WORKERS {
        assert_eq!(divergence(&Motion::BuiltIn, workers, None, &|| {}), None, "built-in, {workers} workers");
        let crate_tier = Motion::Pass(Arc::new(CompileTime));
        assert_eq!(divergence(&crate_tier, workers, None, &|| {}), None, "crate, {workers} workers");
    }
}

#[test]
fn the_module_reproduces_the_reference_and_survives_a_reload() {
    let dir = builds::scratch("test-module");
    let build = builds::build_module(&builds::workspace_root(), &dir);
    let Some(artifact) = &build.artifact else {
        eprintln!("skipped: the module did not build here: {}", build.stderr_tail);
        return;
    };
    let module = Arc::new(WasmModule::load(artifact).unwrap());
    let motion = Motion::Pass(module.clone() as Arc<dyn MotionPass>);
    for workers in WORKERS {
        assert_eq!(divergence(&motion, workers, None, &|| {}), None, "module, {workers} workers");
    }
    assert_eq!(divergence(&motion, 3, Some(17), &|| { module.reload().unwrap(); }), None, "module reloaded at tick 17");
    assert_eq!(module.reloads(), 1);
    let (copy, call) = module.times();
    assert!(copy.as_nanos() > 0 && call.as_nanos() > 0);
    drop(motion);
    drop(module);
    let _ = fs::remove_dir_all(dir);
}

fn check_library(build: &builds::BuildOutcome, expect_reference: bool) {
    let Some(artifact) = &build.artifact else {
        eprintln!("skipped: {} did not build here: {}", build.what, build.stderr_tail);
        return;
    };
    let library = Arc::new(DynamicLibrary::load(artifact).unwrap());
    let motion = Motion::Pass(library.clone() as Arc<dyn MotionPass>);
    let outcomes: Vec<Option<usize>> = WORKERS.iter().map(|&workers| divergence(&motion, workers, None, &|| {})).collect();
    if expect_reference {
        assert!(outcomes.iter().all(Option::is_none), "{}: {outcomes:?}", build.what);
    } else {
        // The default C build may or may not contract on this machine; what it
        // gives must at least be the same at every worker count.
        let hashes: Vec<Vec<u64>> = WORKERS
            .iter()
            .map(|&workers| {
                let mut world = World::new(WorldConfig::new(Size::Small, workers), motion.clone());
                world.step(TICKS).unwrap();
                world.hashes()
            })
            .collect();
        assert!(hashes.windows(2).all(|pair| pair[0] == pair[1]), "{}: worker counts disagree", build.what);
        eprintln!("{}: first divergent tick from the reference {:?} ({})", build.what, outcomes[0], build.flags);
    }
    let reloaded = divergence(&motion, 3, Some(17), &|| { library.reload().unwrap(); });
    assert_eq!(reloaded, outcomes[2], "{}: a reload changes the result", build.what);
    assert_eq!(library.reloads(), 1);
}

#[test]
fn the_dynamic_libraries_reproduce_the_reference_and_survive_a_reload() {
    let root = builds::workspace_root();
    let native_dir = builds::scratch("test-native");
    check_library(&builds::build_native(&root, &native_dir), true);
    let _ = fs::remove_dir_all(native_dir);
    let permitted_dir = builds::scratch("test-c-permitted");
    check_library(&builds::build_c(&root, &permitted_dir, true), true);
    let _ = fs::remove_dir_all(permitted_dir);
    let default_dir = builds::scratch("test-c-default");
    check_library(&builds::build_c(&root, &default_dir, false), false);
    let _ = fs::remove_dir_all(default_dir);
}

#[test]
fn the_importer_process_answers_the_expected_bytes() {
    let dir = builds::scratch("test-importer");
    let source = dir.join("repulsion.txt");
    fs::write(&source, importer::SOURCE).unwrap();
    let exe = Path::new(env!("CARGO_BIN_EXE_plugin-tiers"));
    let (bytes, elapsed) = importer::round_trip(exe, &source).unwrap();
    assert_eq!(bytes, importer::import(importer::SOURCE).unwrap());
    assert!(elapsed.as_micros() > 0);
    let missing = importer::round_trip(exe, &dir.join("absent.txt")).unwrap_err();
    assert!(missing.contains("absent.txt"), "{missing}");
    let _ = fs::remove_dir_all(dir);
}
