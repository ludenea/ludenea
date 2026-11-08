//! The plugin tiers spike's scenarios and report, for the plugin architecture
//! research. Throwaway, never product code.
//!
//! Started with `--importer`, the binary is the out-of-process importer (see
//! `importer.rs`). Otherwise it runs the measurement: the reference workload
//! (default size, 1 000 ticks, wander off) under the built-in motion pass and
//! under each tier's implementation of the same pass, at 1, 2, 4 and 8 workers
//! (and the machine's CPU count when different), every run compared tick by
//! tick with the sequential reference; the processor time of the motion node
//! per tier against the built-in pass; a reload of the module and of the
//! dynamic libraries between two steps; and the importer's round trip.

use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use plugin_tiers::builds::{self, BuildOutcome};
use plugin_tiers::importer;
use plugin_tiers::module::WasmModule;
use plugin_tiers::tiers::{CompileTime, DynamicLibrary};
use plugin_tiers::world::{Motion, MotionPass, Size, World, WorldConfig};
use reference_workload::{DEFAULT_TICK_COUNT, Json, Parameters, Run, first_divergent_tick, run_hash, run_sequential, write_platform_and_build};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "plugin-tiers";
const BASE_WORKER_COUNTS: [usize; 4] = [1, 2, 4, 8];
const RELOAD_TICK: u32 = 500;

fn worker_counts() -> Vec<usize> {
    let available = thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = BASE_WORKER_COUNTS.to_vec();
    if !counts.contains(&available) {
        counts.push(available);
        counts.sort_unstable();
    }
    counts
}

/// A tier's loaded implementation, with what the host can do to it besides
/// running it.
enum Loaded {
    BuiltIn,
    CompileTime,
    Module(Arc<WasmModule>),
    Library(Arc<DynamicLibrary>),
}

struct Tier {
    tier: &'static str,
    form: &'static str,
    loaded: Loaded,
}

impl Tier {
    fn motion(&self) -> Motion {
        match &self.loaded {
            Loaded::BuiltIn => Motion::BuiltIn,
            Loaded::CompileTime => Motion::Pass(Arc::new(CompileTime)),
            Loaded::Module(module) => Motion::Pass(module.clone() as Arc<dyn MotionPass>),
            Loaded::Library(library) => Motion::Pass(library.clone() as Arc<dyn MotionPass>),
        }
    }

    fn reload(&self) -> Option<Result<Duration, String>> {
        match &self.loaded {
            Loaded::Module(module) => Some(module.reload()),
            Loaded::Library(library) => Some(library.reload()),
            _ => None,
        }
    }
}

struct Measured {
    run_hash: u64,
    first_divergent_tick: Option<usize>,
    ticks_per_second: f64,
    motion_us_per_tick: f64,
    motion_us_per_chunk: f64,
}

fn measure(world: &World, reference: &Run, ticks: u32) -> Measured {
    let hashes = world.hashes();
    let (motion, chunks) = world.motion_time();
    let stepping = world.stepping().as_secs_f64();
    Measured {
        run_hash: run_hash(&hashes),
        first_divergent_tick: first_divergent_tick(&hashes, &reference.tick_hashes),
        ticks_per_second: if stepping > 0.0 { f64::from(ticks) / stepping } else { 0.0 },
        motion_us_per_tick: motion.as_secs_f64() * 1e6 / f64::from(ticks),
        motion_us_per_chunk: if chunks > 0 { motion.as_secs_f64() * 1e6 / chunks as f64 } else { 0.0 },
    }
}

fn write_measured(out: &mut Json, measured: &Measured, ticks: u32, builtin_us_per_chunk: Option<f64>) {
    out.hex("run_hash", measured.run_hash);
    out.bool("matches_reference", measured.first_divergent_tick.is_none());
    out.i64("first_divergent_tick", measured.first_divergent_tick.map_or(-1, |t| t as i64));
    out.u64("ticks", u64::from(ticks));
    out.decimal("ticks_per_second", measured.ticks_per_second);
    out.decimal("motion_us_per_tick", measured.motion_us_per_tick);
    out.decimal("motion_us_per_chunk", measured.motion_us_per_chunk);
    match builtin_us_per_chunk {
        Some(builtin) if builtin > 0.0 => out.decimal("ratio_to_builtin", measured.motion_us_per_chunk / builtin),
        _ => out.string("ratio_to_builtin", "n/a"),
    }
}

/// Runs one tier at one worker count for `ticks` ticks in one step and
/// measures it; a world that fails (a trap in a module, a panic in a library)
/// is reported as such.
fn run_tier(out: &mut Json, tier: &Tier, workers: usize, size: Size, ticks: u32, reference: &Run, builtin_us_per_chunk: Option<f64>) -> Option<f64> {
    let mut world = World::new(WorldConfig::new(size, workers), tier.motion());
    // The module's counters accumulate over its life: this run's share is the difference.
    let module_before = match &tier.loaded {
        Loaded::Module(module) => Some((module.times(), module.instantiations())),
        _ => None,
    };
    let outcome = world.step(ticks);
    out.object_start();
    out.string("tier", tier.tier);
    out.string("form", tier.form);
    out.bool("available", true);
    out.u64("workers", workers as u64);
    let per_chunk = match outcome {
        Ok(()) => {
            let measured = measure(&world, reference, ticks);
            write_measured(out, &measured, ticks, builtin_us_per_chunk);
            Some(measured.motion_us_per_chunk)
        }
        Err(failure) => {
            out.bool("matches_reference", false);
            out.string("failure", &failure.to_string());
            None
        }
    };
    if let (Loaded::Module(module), Some(((copy_before, call_before), instantiated_before))) = (&tier.loaded, module_before) {
        let (copy, call) = module.times();
        out.decimal("copy_us_per_tick", (copy - copy_before).as_secs_f64() * 1e6 / f64::from(ticks));
        out.decimal("call_us_per_tick", (call - call_before).as_secs_f64() * 1e6 / f64::from(ticks));
        out.u64("instantiations", module.instantiations() - instantiated_before);
    }
    out.object_end();
    per_chunk
}

/// Steps to the reload tick, reloads the tier's code, steps to the end, and
/// compares with the reference.
fn run_reload(out: &mut Json, tier: &Tier, workers: usize, size: Size, ticks: u32, reference: &Run) {
    let mut world = World::new(WorldConfig::new(size, workers), tier.motion());
    out.object_start();
    out.string("tier", tier.tier);
    out.string("form", tier.form);
    out.u64("workers", workers as u64);
    out.u64("reload_tick", u64::from(RELOAD_TICK.min(ticks)));
    let first = world.step(RELOAD_TICK.min(ticks));
    let reloaded = tier.reload().expect("a reloadable tier");
    let second = if first.is_ok() && ticks > RELOAD_TICK { world.step(ticks - RELOAD_TICK) } else { first.clone() };
    match reloaded {
        Ok(duration) => {
            out.bool("reloaded", true);
            out.decimal("reload_ms", duration.as_secs_f64() * 1e3);
        }
        Err(error) => {
            out.bool("reloaded", false);
            out.string("reload_error", &error);
        }
    }
    match second {
        Ok(()) => {
            let measured = measure(&world, reference, ticks);
            out.hex("run_hash", measured.run_hash);
            out.bool("matches_reference", measured.first_divergent_tick.is_none());
            out.bool("reload_changes_hash", measured.first_divergent_tick.is_some());
            out.i64("first_divergent_tick", measured.first_divergent_tick.map_or(-1, |t| t as i64));
        }
        Err(failure) => {
            out.bool("matches_reference", false);
            out.string("failure", &failure.to_string());
        }
    }
    out.object_end();
}

fn write_build(out: &mut Json, build: &BuildOutcome) {
    out.object_start();
    out.string("what", &build.what);
    out.decimal("seconds", build.seconds);
    out.bool("succeeded", build.succeeded);
    if !build.succeeded {
        out.string("stderr_tail", &build.stderr_tail);
    }
    if let Some(artifact) = &build.artifact {
        out.string("artifact", &artifact.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    }
    out.string("compiler", &build.compiler);
    out.string("flags", &build.flags);
    out.object_end();
}

/// A tier whose build failed or whose library did not load: reported once,
/// unavailable, with the reason.
fn write_unavailable(out: &mut Json, tier: &str, form: &str, reason: &str) {
    out.object_start();
    out.string("tier", tier);
    out.string("form", form);
    out.bool("available", false);
    out.string("reason", reason);
    out.object_end();
}

fn load_library(build: &BuildOutcome) -> Result<Arc<DynamicLibrary>, String> {
    let path = build.artifact.as_ref().ok_or_else(|| format!("build failed: {}", build.stderr_tail))?;
    DynamicLibrary::load(path).map(Arc::new)
}

fn importer_round_trips(out: &mut Json, scratch: &Path) {
    let exe = env::current_exe().expect("the running executable has a path");
    let source = scratch.join("repulsion.txt");
    fs::write(&source, importer::SOURCE).expect("the source file is writable");
    let expected = importer::import(importer::SOURCE).expect("the source imports");
    out.key("importer");
    out.object_start();
    out.string("source", importer::SOURCE);
    out.u64("payload_bytes", expected.len() as u64);
    out.hex("content_hash", importer::content_hash(&expected).unwrap_or(0));
    out.u64("recorded_before_tick", 1);
    out.key("round_trips");
    out.array_start();
    let mut all_match = true;
    for sample in 1..=5_u64 {
        out.object_start();
        out.u64("sample", sample);
        match importer::round_trip(&exe, &source) {
            Ok((bytes, elapsed)) => {
                let matches = bytes == expected;
                all_match &= matches;
                out.bool("matches_expected", matches);
                out.decimal("spawn_and_round_trip_us", elapsed.as_secs_f64() * 1e6);
            }
            Err(error) => {
                all_match = false;
                out.bool("matches_expected", false);
                out.string("error", &error);
            }
        }
        out.object_end();
    }
    out.array_end();
    out.bool("matches_expected", all_match);
    out.object_end();
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--importer") {
        if let Err(error) = importer::serve() {
            eprintln!("importer: {error}");
            std::process::exit(1);
        }
        return;
    }
    let cell = args.first().cloned().unwrap_or_else(|| "unspecified".to_string());
    let size = Size::Default;
    let ticks = DEFAULT_TICK_COUNT;
    let counts = worker_counts();
    let root = builds::workspace_root();
    let reference = run_sequential(&Parameters::new(size, ticks, false));

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.string("size", size.name());
    out.u64("ticks", u64::from(ticks));
    out.hex("reference_run_hash", reference.run_hash);
    out.string("wasmtime_version", "50.0.1");

    // The run-time builds, each in its own temporary directory.
    let module_dir = builds::scratch("module");
    let native_dir = builds::scratch("native");
    let c_default_dir = builds::scratch("c-default");
    let c_permitted_dir = builds::scratch("c-permitted");
    let module_build = builds::build_module(&root, &module_dir);
    let native_build = builds::build_native(&root, &native_dir);
    let c_default_build = builds::build_c(&root, &c_default_dir, false);
    let c_permitted_build = builds::build_c(&root, &c_permitted_dir, true);
    out.key("builds");
    out.array_start();
    for build in [&module_build, &native_build, &c_default_build, &c_permitted_build] {
        write_build(&mut out, build);
    }
    out.array_end();

    // The tiers, loaded once; the module pool holds one instance per worker
    // of the largest worker count.
    let max_workers = counts.iter().copied().max().unwrap_or(1);
    let mut tiers = vec![
        Tier { tier: "built-in", form: "inlined kernel", loaded: Loaded::BuiltIn },
        Tier { tier: "compile-time crate", form: "static crate", loaded: Loaded::CompileTime },
    ];
    let mut unavailable: Vec<(&str, &str, String)> = Vec::new();
    match module_build.artifact.as_ref().ok_or_else(|| format!("build failed: {}", module_build.stderr_tail)).and_then(|path| WasmModule::load(path)) {
        Ok(module) => tiers.push(Tier { tier: "sandboxed module", form: "wasmtime", loaded: Loaded::Module(Arc::new(module)) }),
        Err(reason) => unavailable.push(("sandboxed module", "wasmtime", reason)),
    }
    for (form, build) in [("rust cdylib", &native_build), ("c default flags", &c_default_build), ("c permitted flags", &c_permitted_build)] {
        match load_library(build) {
            Ok(library) => tiers.push(Tier { tier: "dynamic native library", form, loaded: Loaded::Library(library) }),
            Err(reason) => unavailable.push(("dynamic native library", form, reason)),
        }
    }

    out.key("tiers");
    out.array_start();
    for &workers in &counts {
        let mut builtin_us_per_chunk = None;
        for tier in &tiers {
            let per_chunk = run_tier(&mut out, tier, workers, size, ticks, &reference, builtin_us_per_chunk);
            if matches!(tier.loaded, Loaded::BuiltIn) {
                builtin_us_per_chunk = per_chunk;
            }
        }
    }
    for (tier, form, reason) in &unavailable {
        write_unavailable(&mut out, tier, form, reason);
    }
    out.array_end();

    out.key("reloads");
    out.array_start();
    for tier in tiers.iter().filter(|tier| matches!(tier.loaded, Loaded::Module(_) | Loaded::Library(_))) {
        run_reload(&mut out, tier, 4.min(max_workers), size, ticks, &reference);
    }
    out.array_end();

    importer_round_trips(&mut out, &module_dir);

    drop(tiers);
    for dir in [module_dir, native_dir, c_default_dir, c_permitted_dir] {
        let _ = fs::remove_dir_all(dir);
    }
    out.object_end();
    println!("{}", out.finish());
}
