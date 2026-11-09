# Scripting runtime

Throwaway spike of the scripting and gameplay model research. It takes every gameplay system of the [reference workload](../reference-workload/README.md)'s tick (spatial hashing, repulsion, wander, integration, boundary, the chunk's kinetic energy, the structural changes), writes them once in Rust, builds them as a WebAssembly module, and runs the tick through that module under two runtimes: one that compiles the module to machine code and one that interprets it. It measures, instead of assuming, whether the scripted tick gives the reference hashes at every worker count on every cell of the boundary, whether the two runtimes agree with each other on the results and on the fuel they consume, what a failing module does to the world, how deep a module may recurse, what the scripted tick, a host call and an instantiation cost, and whether replacing the module by another one at a tick boundary is an input a replay reproduces.

It extends the [plugin tiers](../plugin-tiers/README.md) spike, which ran one system of the tick under one runtime with unchanged code, no fuel, no host call and no invalid floating-point result.

## Layout

- `systems/`: the tick's gameplay systems over plain columns, with the reference workload's own kernels where the specification fixes them. Two things are parameters: the damping constant, so that a second module can differ from the first by its code, and where the wander system takes its cosine and sine from.
- `module/`: the systems exported as a WebAssembly module (a `cdylib` built for `wasm32-unknown-unknown`). The columns live in static arenas, so the module's linear memory has its final size at instantiation and never grows. Its three imports are its whole view of the host: a cosine, a sine and a function that does nothing. The `variant` feature builds the second module, with another damping constant. Three more exports exist for the measurements: invalid results computed at run time, a recursion the compiler cannot turn into a loop, and a loop of calls to the empty host function.
- `src/instance.rs`: what the world asks of one instance, and the native baseline: the same systems linked into the host behind the same copies, so that a ratio between a runtime and the baseline compares the execution of the systems and nothing else.
- `src/compiled.rs`: the compiling runtime, Wasmtime, with fuel on or off, NaN canonicalization on, relaxed SIMD off and no thread support built. `src/interpreted.rs`: the interpreter, wasmi, built with its deterministic profile, every function translated when the module is created.
- `src/world.rs`: the world. The columns are the host's. One tick: a serial instance builds the cell lists; worker threads, each owning one instance, claim chunks of 256 slots, each call reading the columns copied into the instance once per tick and writing four columns the host copies out; the host applies them and adds the chunk energies in chunk order; the serial instance applies the structural changes. The state hash is the host's. A call that faults fails the world at that tick: the lowest failing chunk is the failure, nothing of the tick is applied, and later steps are refused.
- `src/builds.rs`: the two run-time builds of the module (`rustup target add wasm32-unknown-unknown`, then `cargo build --release --locked --target wasm32-unknown-unknown -p tick-module`, once with `--features variant`), in temporary directories.
- `src/main.rs`: the scenarios and the report. `tests/self_check.rs`: the self-checks.

Third-party components, the two subjects of the measurement and nothing else: Wasmtime 50.0.1 (crate `wasmtime`, Apache 2.0 with the LLVM exception, features `runtime`, `cranelift`, `std` and `pooling-allocator`) and wasmi 2.0.0 (crate `wasmi`, MIT or Apache 2.0, features `std`, `validate` and `deterministic`).

## Hypotheses

With the reference workload, version 1, default size (10 000 entities, 1 000 ticks), compiled by one pinned Rust toolchain at default target features, at 1, 4 and 8 workers:

1. The whole tick as a module gives the sequential reference's tick hashes, under both runtimes, with the wander system off, and with it on when the cosine and sine are the host's functions (the reference uses the platform's on purpose, so that run compares with the same cell's reference).
2. With the cosine and sine compiled into the module, the module's results no longer depend on the platform: the two runtimes give one run hash, the same on every cell, and it differs from each cell's own reference.
3. The two runtimes consume the same fuel for the same tick, and each consumes the same fuel at every worker count and on every cell.
4. An invalid floating-point result has one bit pattern under both runtimes on every cell.
5. A module that reads outside its memory or panics in one chunk of one tick stops the world at that tick, in that chunk, with the same trap under both runtimes; the world keeps the state of the last completed tick and refuses further steps. A chunk budget one unit below the smallest budget that completes a tick stops the tick at the same chunk everywhere.
6. The depth at which a recursion is stopped depends on the runtime and, for the compiling runtime, on the architecture; the fuel a level consumes does not.
7. The scripted tick under the compiling runtime with fuel on, at 4 workers, costs less than 4.2 ms (a quarter of a sixtieth of a second). The interpreter, fuel's overhead, a host call and an instantiation are measured without a threshold, except the instantiation, expected under 1 ms.
8. Replacing the module by the second one before tick 500 changes the tick hashes from tick 500 on and not before, identically under both runtimes, at every worker count and on every cell, equal to the native baseline switched at the same tick; a second run from the record of what was loaded (the tick and the content hash of each module) reproduces every hash.

## Method

The binary runs the sequential reference with the wander system off and on; builds the two modules; then runs each scenario and prints one JSON report.

- **Runs**: a fresh world per runtime (the native baseline, the compiling runtime with fuel off and on, the interpreter with fuel on), per worker count and per wander form, 1 000 ticks, every tick hash compared with the reference where one applies; the wall time per tick, the workers' processor time inside the systems and in the copies, the serial systems' time, the fuel per tick as a total and as a hash of the per-tick totals, and the largest fuel of one chunk call. One more run gives the interpreter with fuel off.
- **Invalid results**: zero over zero, infinity minus infinity, the square root of a negative number and zero times infinity, computed inside an instance from arguments, their bits read back.
- **Injected faults**: a world at 4 workers asked to read outside the memory, then to panic, in chunk 17 of tick 300.
- **Fuel exhaustion**: the smallest chunk budget that completes the first tick, found by bisection, and the tick under one unit less.
- **Fuel of a first and a second world**: the first tick's fuel of two worlds created one after the other on a runtime made for that measurement, for the compiling runtime, the interpreter, and the interpreter translating each function at its first call.
- **Recursion**: the deepest recursion that returns, by doubling then bisection, and the fuel at two depths.
- **Host calls**: five million calls of the empty host function from a loop inside the module.
- **Instantiation**: two hundred instances ready to call, per runtime, the compiling runtime also with its pooling allocator; the module's compilation or translation time.
- **Reloads**: per runtime and worker count, the second module loaded before tick 500, compared with the native baseline switched at the same tick, and run a second time from the load record.

## Environment

The five cells of the research CI and the developer's machine. The report carries the platform, the build facts and the durations of the two module builds.

## Result

Recorded after the runs on the five cells.
