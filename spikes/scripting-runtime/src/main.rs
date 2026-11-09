//! The scripting runtime spike's scenarios and report, for the scripting and
//! gameplay model research. Throwaway, never product code.
//!
//! Builds the module twice (the systems, and the systems with another damping
//! constant), then measures, on the reference workload at its default size:
//! the tick hashes of every gameplay system run through the native baseline,
//! the compiling runtime and the interpreter at 1, 4 and 8 workers, with the
//! wander system off, on with the cosine and sine compiled into the module,
//! and on with the host's; the fuel each run consumes; the bits of invalid
//! results; where and how a failing module stops; how deep a recursion goes;
//! what a host call and an instantiation cost; and a module replaced by the
//! other one at a tick boundary, replayed from the record of what was loaded.

use std::env;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use reference_workload::{DEFAULT_TICK_COUNT, Json, Parameters, Size, first_divergent_tick, fnv1a, run_hash, run_sequential, write_platform_and_build};
use scripting_runtime::builds::{self, Build};
use scripting_runtime::compiled::Compiled;
use scripting_runtime::instance::{AMPLE_FUEL, Runtime, native};
use scripting_runtime::interpreted::Interpreted;
use scripting_runtime::world::{Injection, Wander, World};
use scripting_runtime::{run, run_fresh};
use tick_systems::{DAMPING, DAMPING_VARIANT};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "scripting-runtime";
const WORKER_COUNTS: [usize; 3] = [1, 4, 8];
const WANDERS: [Wander; 3] = [Wander::Off, Wander::CompiledIn, Wander::HostFunctions];
const FAULT_TICK: u32 = 300;
const FAULT_CHUNK: usize = 17;
const FAULT_WORKERS: usize = 4;
const RELOAD_TICK: u32 = 500;
const NOP_CALLS: u32 = 5_000_000;
const INSTANTIATIONS: usize = 200;
const DEPTH_CAP: u32 = 1 << 24;
const SHALLOW_DEPTHS: [u32; 2] = [500, 900];

type Shared = Arc<dyn Runtime>;

fn fuel_hash(fuel_per_tick: &[u64]) -> u64 {
    let bytes: Vec<u8> = fuel_per_tick.iter().flat_map(|fuel| fuel.to_le_bytes()).collect();
    fnv1a(&bytes)
}

fn write_failure(out: &mut Json, error: &str) {
    out.bool("ran", false);
    out.string("failure", error);
}

/// One run of the whole tick on one runtime, compared with a reference.
fn write_run(out: &mut Json, runtime: &Shared, parameters: &Parameters, workers: usize, wander: Wander, reference: Option<&[u64]>) {
    out.object_start();
    out.string("runtime", runtime.name());
    out.u64("workers", workers as u64);
    out.string("wander", wander.name());
    let started = Instant::now();
    match run_fresh(runtime, parameters, workers, wander) {
        Err(error) => write_failure(out, &error),
        Ok((hashes, world)) => {
            let seconds = started.elapsed().as_secs_f64();
            let ticks = f64::from(parameters.tick_count);
            out.bool("ran", true);
            out.hex("run_hash", run_hash(&hashes));
            match reference {
                Some(reference) => {
                    out.bool("matches_reference", hashes == reference);
                    out.i64("first_divergent_tick", first_divergent_tick(&hashes, reference).map_or(-1, |tick| tick as i64));
                }
                None => out.raw("matches_reference", "null"),
            }
            out.decimal("ticks_per_second", ticks / seconds);
            out.decimal("ms_per_tick", seconds * 1_000.0 / ticks);
            out.decimal("call_us_per_tick", world.counters.call_nanos.load(Ordering::Relaxed) as f64 / 1_000.0 / ticks);
            out.decimal("copy_us_per_tick", world.counters.copy_nanos.load(Ordering::Relaxed) as f64 / 1_000.0 / ticks);
            out.decimal("serial_us_per_tick", world.serial_nanos as f64 / 1_000.0 / ticks);
            out.bool("metered", runtime.metered());
            if runtime.metered() {
                out.u64("fuel_total", world.fuel_per_tick.iter().sum());
                out.hex("fuel_hash", fuel_hash(&world.fuel_per_tick));
                out.u64("largest_chunk_call_fuel", world.largest_chunk_fuel);
                out.u64("largest_serial_call_fuel", world.largest_serial_fuel);
            }
            out.u64("memory_bytes", world.memory_bytes());
        }
    }
    out.object_end();
}

fn write_nans(out: &mut Json, runtime: &Shared) {
    out.object_start();
    out.string("runtime", runtime.name());
    match runtime.instantiate().and_then(|mut instance| instance.make_nans().map_err(|fault| fault.kind)) {
        Err(error) => write_failure(out, &error),
        Ok(bits) => {
            out.bool("ran", true);
            for (name, bits) in ["zero_over_zero", "infinity_minus_infinity", "square_root_of_negative", "zero_times_infinity"].into_iter().zip(bits) {
                out.string(name, &format!("0x{bits:08x}"));
            }
        }
    }
    out.object_end();
}

/// A world asked to fault at a tick: where it stops, how, and what is left.
fn write_injected_fault(out: &mut Json, runtime: &Shared, parameters: &Parameters, kind: u32, what: &str, reference: &[u64]) {
    out.object_start();
    out.string("runtime", runtime.name());
    out.string("fault", what);
    out.u64("workers", FAULT_WORKERS as u64);
    out.u64("injected_at_tick", u64::from(FAULT_TICK));
    out.u64("injected_in_chunk", FAULT_CHUNK as u64);
    match World::new(runtime, parameters, FAULT_WORKERS, Wander::Off) {
        Err(error) => write_failure(out, &error),
        Ok(mut world) => {
            world.injection = Some(Injection { tick: FAULT_TICK, chunk: FAULT_CHUNK, kind });
            let (hashes, failure) = run(&mut world, parameters.tick_count, |_, _| {});
            out.bool("ran", true);
            out.u64("last_completed_tick", u64::from(world.tick()));
            out.bool("hashes_match_reference_up_to_there", hashes[..] == reference[..hashes.len()]);
            out.bool("state_is_the_last_completed_tick", world.hash() == reference[world.tick() as usize]);
            out.bool("further_step_refused", world.step().is_err());
            match failure {
                None => out.bool("failed", false),
                Some(failure) => {
                    out.bool("failed", true);
                    out.u64("failure_tick", u64::from(failure.tick));
                    out.i64("failure_chunk", failure.chunk.map_or(-1, |chunk| chunk as i64));
                    out.string("trap", &failure.fault.kind);
                    out.string("message", &failure.fault.detail);
                    out.i64("fuel_consumed_by_the_failing_call", failure.fuel_consumed.map_or(-1, |fuel| fuel as i64));
                }
            }
        }
    }
    out.object_end();
}

/// The smallest chunk budget that completes the first tick, found by
/// bisection below the fuel the tick's largest chunk call consumes, and what
/// the budget one unit below it does. A runtime that checks fuel at every
/// instruction stops exactly at the consumption; one that checks at loops and
/// calls lets a call finish that is already past its last check.
fn write_exhaustion(out: &mut Json, runtime: &Shared, parameters: &Parameters, reference: &[u64]) {
    out.object_start();
    out.string("runtime", runtime.name());
    out.u64("workers", FAULT_WORKERS as u64);
    let outcome = (|| {
        let first_tick = |budget: u64| {
            let mut world = World::new(runtime, parameters, FAULT_WORKERS, Wander::Off)?;
            world.chunk_budget = budget;
            let failure = world.step().err();
            Ok::<_, String>((failure, world.hash(), world.largest_chunk_fuel))
        };
        let (_, _, consumed) = first_tick(AMPLE_FUEL)?;
        let (mut stops, mut completes) = (0_u64, consumed);
        while completes - stops > 1 {
            let middle = stops + (completes - stops) / 2;
            match first_tick(middle)?.0 {
                Some(_) => stops = middle,
                None => completes = middle,
            }
        }
        let (failure, stopped_hash, _) = first_tick(stops)?;
        let (unexpected, completed_hash, _) = first_tick(completes)?;
        Ok::<_, String>((consumed, completes, failure, stopped_hash == reference[0], unexpected.is_none() && completed_hash == reference[1]))
    })();
    match outcome {
        Err(error) => write_failure(out, &error),
        Ok((consumed, completes, failure, untouched, completed)) => {
            out.bool("ran", true);
            out.u64("largest_chunk_call_fuel_of_tick_1", consumed);
            out.u64("smallest_budget_that_completes_tick_1", completes);
            out.bool("one_unit_less_stops_the_tick", failure.is_some());
            if let Some(failure) = failure {
                out.string("trap", &failure.fault.kind);
                out.i64("failure_chunk", failure.chunk.map_or(-1, |chunk| chunk as i64));
                out.i64("fuel_consumed_by_the_failing_call", failure.fuel_consumed.map_or(-1, |fuel| fuel as i64));
            }
            out.bool("state_untouched_after_the_stop", untouched);
            out.bool("that_budget_gives_the_reference_tick", completed);
        }
    }
    out.object_end();
}

/// The fuel of the first tick of two worlds created one after the other on a
/// runtime made for this measurement alone: equal when nothing but the tick is
/// paid for, different when the first world also pays for translating the
/// functions it is the first to call.
fn write_translation_fuel(out: &mut Json, create: impl FnOnce() -> Result<Shared, String>, parameters: &Parameters) {
    out.object_start();
    let outcome = create().and_then(|runtime| {
        let mut fuel = [0_u64; 2];
        for fuel in &mut fuel {
            let mut world = World::new(&runtime, parameters, 1, Wander::Off)?;
            world.step().map_err(|failure| failure.fault.kind)?;
            *fuel = world.fuel_per_tick[0];
        }
        Ok((runtime.name(), fuel))
    });
    match outcome {
        Err(error) => write_failure(out, &error),
        Ok((name, [first, second])) => {
            out.string("runtime", name);
            out.bool("ran", true);
            out.u64("first_world_fuel_of_tick_1", first);
            out.u64("second_world_fuel_of_tick_1", second);
            out.bool("equal", first == second);
        }
    }
    out.object_end();
}

/// How deep the module's recursion goes before the runtime stops it, and the
/// fuel a level consumes.
fn write_stack(out: &mut Json, runtime: &Shared) {
    out.object_start();
    out.string("runtime", runtime.name());
    let outcome = (|| {
        let mut instance = runtime.instantiate()?;
        let mut dive = |depth: u32| {
            instance.set_fuel(AMPLE_FUEL);
            let outcome = instance.dive(depth);
            (outcome, instance.fuel().map_or(0, |left| AMPLE_FUEL - left))
        };
        let (_, fuel_shallow) = dive(SHALLOW_DEPTHS[0]);
        let (_, fuel_deeper) = dive(SHALLOW_DEPTHS[1]);
        let mut deepest = 0_u32;
        let mut shallowest_failure = None;
        let mut depth = 512_u32;
        while depth <= DEPTH_CAP {
            match dive(depth).0 {
                Ok(_) => deepest = depth,
                Err(fault) => {
                    shallowest_failure = Some((depth, fault));
                    break;
                }
            }
            depth *= 2;
        }
        if let Some((mut high, mut fault)) = shallowest_failure.clone() {
            while high - deepest > 1 {
                let middle = deepest + (high - deepest) / 2;
                match dive(middle).0 {
                    Ok(_) => deepest = middle,
                    Err(found) => (high, fault) = (middle, found),
                }
            }
            shallowest_failure = Some((high, fault));
        }
        let (_, fuel_at_deepest) = dive(deepest);
        Ok::<_, String>((fuel_shallow, fuel_deeper, deepest, shallowest_failure, fuel_at_deepest))
    })();
    match outcome {
        Err(error) => write_failure(out, &error),
        Ok((fuel_shallow, fuel_deeper, deepest, failure, fuel_at_deepest)) => {
            out.bool("ran", true);
            out.u64("fuel_at_depth_500", fuel_shallow);
            out.u64("fuel_at_depth_900", fuel_deeper);
            out.decimal("fuel_per_level", (fuel_deeper - fuel_shallow) as f64 / f64::from(SHALLOW_DEPTHS[1] - SHALLOW_DEPTHS[0]));
            out.u64("deepest_recursion_that_returns", u64::from(deepest));
            out.u64("fuel_at_that_depth", fuel_at_deepest);
            match failure {
                Some((_, fault)) => {
                    out.bool("stopped_below_the_cap", true);
                    out.string("trap", &fault.kind);
                }
                None => out.bool("stopped_below_the_cap", false),
            }
            out.u64("depth_cap_of_the_search", u64::from(DEPTH_CAP));
        }
    }
    out.object_end();
}

fn write_host_call(out: &mut Json, runtime: &Shared) {
    out.object_start();
    out.string("runtime", runtime.name());
    let outcome = (|| {
        let mut instance = runtime.instantiate()?;
        instance.set_fuel(AMPLE_FUEL);
        instance.call_nop(1_000).map_err(|fault| fault.kind)?;
        instance.set_fuel(AMPLE_FUEL);
        let started = Instant::now();
        instance.call_nop(NOP_CALLS).map_err(|fault| fault.kind)?;
        let nanos = started.elapsed().as_nanos() as f64;
        Ok::<_, String>((nanos / f64::from(NOP_CALLS), instance.fuel().map(|left| (AMPLE_FUEL - left) as f64 / f64::from(NOP_CALLS))))
    })();
    match outcome {
        Err(error) => write_failure(out, &error),
        Ok((nanos, fuel)) => {
            out.bool("ran", true);
            out.u64("calls", u64::from(NOP_CALLS));
            out.decimal("ns_per_call_and_loop_iteration", nanos);
            if let Some(fuel) = fuel {
                out.decimal("fuel_per_call_and_loop_iteration", fuel);
            }
        }
    }
    out.object_end();
}

fn write_instantiation(out: &mut Json, what: &str, create: impl FnOnce() -> Result<Shared, String>) {
    out.object_start();
    out.string("runtime", what);
    let started = Instant::now();
    let outcome = create().and_then(|runtime| {
        let compile_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let mut micros = Vec::with_capacity(INSTANTIATIONS);
        let mut memory = 0;
        for _ in 0..INSTANTIATIONS {
            let started = Instant::now();
            let instance = runtime.instantiate()?;
            micros.push(started.elapsed().as_secs_f64() * 1_000_000.0);
            memory = instance.memory_bytes();
        }
        micros.sort_by(f64::total_cmp);
        Ok((compile_ms, micros, memory))
    });
    match outcome {
        Err(error) => write_failure(out, &error),
        Ok((compile_ms, micros, memory)) => {
            out.bool("ran", true);
            out.decimal("module_compilation_ms", compile_ms);
            out.u64("instantiations", INSTANTIATIONS as u64);
            out.decimal("median_us", micros[INSTANTIATIONS / 2]);
            out.decimal("mean_us", micros.iter().sum::<f64>() / INSTANTIATIONS as f64);
            out.decimal("slowest_us", micros[INSTANTIATIONS - 1]);
            out.u64("memory_bytes", memory);
        }
    }
    out.object_end();
}

/// The record of what a world loaded: the tick each module takes effect at
/// and the module's content hash. A replay resolves the hashes again.
type LoadLog = Vec<(u32, u64)>;

fn run_from_log(log: &LoadLog, resolve: &dyn Fn(u64) -> Shared, parameters: &Parameters, workers: usize) -> Result<(Vec<u64>, f64), String> {
    let mut world = World::new(&resolve(log[0].1), parameters, workers, Wander::Off)?;
    let mut reload_ms = 0.0;
    let mut load_error = None;
    let (hashes, failure) = run(&mut world, parameters.tick_count, |world, tick| {
        if let Some((_, content)) = log.iter().skip(1).find(|(at, _)| *at == tick) {
            let started = Instant::now();
            if let Err(error) = world.load(&resolve(*content)) {
                load_error = Some(error);
            }
            reload_ms = started.elapsed().as_secs_f64() * 1_000.0;
        }
    });
    match (load_error, failure) {
        (Some(error), _) => Err(error),
        (None, Some(failure)) => Err(format!("tick {} failed: {}", failure.tick, failure.fault.kind)),
        (None, None) => Ok((hashes, reload_ms)),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_reload(out: &mut Json, name: &str, first: &Shared, second: &Shared, contents: (u64, u64), parameters: &Parameters, workers: usize, reference: &[u64], oracle: &[u64]) {
    out.object_start();
    out.string("runtime", name);
    out.u64("workers", workers as u64);
    out.u64("second_module_takes_effect_at_tick", u64::from(RELOAD_TICK));
    let log: LoadLog = vec![(1, contents.0), (RELOAD_TICK, contents.1)];
    let resolve = |content: u64| if content == contents.0 { Arc::clone(first) } else { Arc::clone(second) };
    let outcome = run_from_log(&log, &resolve, parameters, workers).and_then(|live| run_from_log(&log, &resolve, parameters, workers).map(|replay| (live, replay)));
    match outcome {
        Err(error) => write_failure(out, &error),
        Ok(((hashes, reload_ms), (replayed, _))) => {
            out.bool("ran", true);
            out.hex("run_hash", run_hash(&hashes));
            out.i64("first_divergent_tick_from_the_reference", first_divergent_tick(&hashes, reference).map_or(-1, |tick| tick as i64));
            out.bool("matches_the_native_oracle", hashes == oracle);
            out.bool("replay_from_the_load_record_matches", replayed == hashes);
            out.decimal("reload_ms", reload_ms);
        }
    }
    out.object_end();
}

fn write_build(out: &mut Json, build: &Build) {
    out.object_start();
    out.string("what", build.what);
    out.decimal("seconds", build.seconds);
    match &build.wasm {
        Ok(wasm) => {
            out.bool("succeeded", true);
            out.u64("module_bytes", wasm.len() as u64);
            out.hex("content_hash", fnv1a(wasm));
        }
        Err(error) => {
            out.bool("succeeded", false);
            out.string("failure", error);
        }
    }
    out.object_end();
}

fn main() {
    let cell = env::args().nth(1).unwrap_or_else(|| "local".to_string());
    let parameters_off = Parameters::new(Size::Default, DEFAULT_TICK_COUNT, false);
    let parameters_on = Parameters::new(Size::Default, DEFAULT_TICK_COUNT, true);
    let reference_off = run_sequential(&parameters_off);
    let reference_on = run_sequential(&parameters_on);

    let root = builds::workspace_root();
    let (scratch_first, scratch_second) = (builds::scratch("first"), builds::scratch("second"));
    let first = builds::build_module(&root, &scratch_first, false);
    let second = builds::build_module(&root, &scratch_second, true);

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    write_platform_and_build(&mut out);
    out.string("compiling_runtime", "wasmtime 50.0.1");
    out.string("interpreter", "wasmi 2.0.0");
    out.hex("reference_run_hash_wander_off", reference_off.run_hash);
    out.hex("reference_run_hash_wander_on", reference_on.run_hash);
    out.decimal("reference_ticks_per_second_wander_off", reference_off.ticks_per_second());
    out.decimal("reference_ticks_per_second_wander_on", reference_on.ticks_per_second());
    out.key("builds");
    out.array_start();
    write_build(&mut out, &first);
    write_build(&mut out, &second);
    out.array_end();

    if let (Ok(wasm), Ok(variant)) = (&first.wasm, &second.wasm) {
        let contents = (fnv1a(wasm), fnv1a(variant));
        let compiled = |wasm: &[u8], metered: bool, pooling: bool| Compiled::new(wasm, metered, pooling).map(|runtime| Arc::new(runtime) as Shared);
        let interpreted = |wasm: &[u8], metered: bool| Interpreted::new(wasm, metered, true).map(|runtime| Arc::new(runtime) as Shared);
        let created = (|| {
            Ok::<_, String>((
                compiled(wasm, false, false)?,
                compiled(wasm, true, false)?,
                interpreted(wasm, false)?,
                interpreted(wasm, true)?,
                compiled(variant, true, false)?,
                interpreted(variant, true)?,
            ))
        })();
        match created {
            Err(error) => out.string("runtimes_failure", &error),
            Ok((compiled_off, compiled_on, interpreted_off, interpreted_on, compiled_variant, interpreted_variant)) => {
                let baseline = native(DAMPING);

                out.key("runs");
                out.array_start();
                for runtime in [&baseline, &compiled_off, &compiled_on, &interpreted_on] {
                    for workers in WORKER_COUNTS {
                        for wander in WANDERS {
                            // The module's own cosine and sine are not the platform's: a
                            // module run with them compiled in has no platform reference.
                            let (parameters, reference) = match wander {
                                Wander::Off => (&parameters_off, Some(&reference_off.tick_hashes[..])),
                                Wander::HostFunctions => (&parameters_on, Some(&reference_on.tick_hashes[..])),
                                Wander::CompiledIn if Arc::ptr_eq(runtime, &baseline) => (&parameters_on, Some(&reference_on.tick_hashes[..])),
                                Wander::CompiledIn => (&parameters_on, None),
                            };
                            write_run(&mut out, runtime, parameters, workers, wander, reference);
                        }
                    }
                }
                write_run(&mut out, &interpreted_off, &parameters_off, 4, Wander::Off, Some(&reference_off.tick_hashes));
                out.array_end();

                out.key("invalid_results");
                out.array_start();
                for runtime in [&baseline, &compiled_on, &interpreted_on] {
                    write_nans(&mut out, runtime);
                }
                out.array_end();

                out.key("injected_faults");
                out.array_start();
                for runtime in [&compiled_on, &interpreted_on] {
                    write_injected_fault(&mut out, runtime, &parameters_off, 1, "a read outside the memory", &reference_off.tick_hashes);
                    write_injected_fault(&mut out, runtime, &parameters_off, 2, "a panic", &reference_off.tick_hashes);
                }
                out.array_end();

                out.key("fuel_exhaustion");
                out.array_start();
                for runtime in [&compiled_on, &interpreted_on] {
                    write_exhaustion(&mut out, runtime, &parameters_off, &reference_off.tick_hashes);
                }
                out.array_end();

                out.key("fuel_of_a_first_and_a_second_world");
                out.array_start();
                write_translation_fuel(&mut out, || compiled(wasm, true, false), &parameters_off);
                write_translation_fuel(&mut out, || interpreted(wasm, true), &parameters_off);
                write_translation_fuel(&mut out, || Interpreted::new(wasm, true, false).map(|runtime| Arc::new(runtime) as Shared), &parameters_off);
                out.array_end();

                out.key("recursion");
                out.array_start();
                for runtime in [&compiled_on, &interpreted_on] {
                    write_stack(&mut out, runtime);
                }
                out.array_end();

                out.key("host_calls");
                out.array_start();
                for runtime in [&compiled_off, &compiled_on, &interpreted_off, &interpreted_on] {
                    write_host_call(&mut out, runtime);
                }
                out.array_end();

                out.key("instantiation");
                out.array_start();
                write_instantiation(&mut out, "wasmtime, fuel on", || compiled(wasm, true, false));
                write_instantiation(&mut out, "wasmtime, fuel on, pooling allocator", || compiled(wasm, true, true));
                write_instantiation(&mut out, "wasmi, fuel on", || interpreted(wasm, true));
                out.array_end();

                out.key("reloads");
                out.array_start();
                let variant_baseline = native(DAMPING_VARIANT);
                let oracle_log: LoadLog = vec![(1, contents.0), (RELOAD_TICK, contents.1)];
                let oracle_resolve = |content: u64| if content == contents.0 { Arc::clone(&baseline) } else { Arc::clone(&variant_baseline) };
                match run_from_log(&oracle_log, &oracle_resolve, &parameters_off, 1) {
                    Err(error) => {
                        out.object_start();
                        out.string("runtime", "native oracle");
                        write_failure(&mut out, &error);
                        out.object_end();
                    }
                    Ok((oracle, _)) => {
                        for workers in WORKER_COUNTS {
                            write_reload(&mut out, "native", &baseline, &variant_baseline, contents, &parameters_off, workers, &reference_off.tick_hashes, &oracle);
                            write_reload(&mut out, "wasmtime, fuel on", &compiled_on, &compiled_variant, contents, &parameters_off, workers, &reference_off.tick_hashes, &oracle);
                            write_reload(&mut out, "wasmi, fuel on", &interpreted_on, &interpreted_variant, contents, &parameters_off, workers, &reference_off.tick_hashes, &oracle);
                        }
                    }
                }
                out.array_end();
            }
        }
    }
    out.object_end();
    println!("{}", out.finish());
    let _ = fs::remove_dir_all(scratch_first);
    let _ = fs::remove_dir_all(scratch_second);
}
