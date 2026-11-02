# Runtime skeleton

Throwaway spike of the runtime architecture research. It builds a minimal crate set in three layers around the [reference workload](../reference-workload/README.md) and measures, instead of assuming, whether a capability can be present or absent at build time without a second code path in the core, what that costs in build time, and whether a time control API that steps a world by whole ticks, snapshots it and restores it reproduces the sequential reference, with two worlds in one process and a failure confined to one of them.

## Layout

Four crates, in the three layers of the proposal under study:

- `core/` (`skeleton-core`): the core layer. A `World` steps the reference workload on the dependency-counting tick graph the [scheduling overhead spike](../workload-scheduling-overhead/README.md) measured (a trimmed copy of its counter variant, with the hash of a tick overlapping the next tick's passes), under a time control API: `step(ticks)`, `tick_hash(tick)`, `snapshot()`, `World::restore(snapshot)`. The thread that calls `step` is worker 0 of that step; the other workers are spawned for the step and joined before it returns. A panic in a task is caught at the task boundary, recorded with its tick, node, chunk and worker, and fails the world, which is never stepped again. The core names no presentation type: `Extraction` is a trait the fixed point calls after every tick when a capability implements it. `src/edit_marker.rs` is the file the spike edits to measure an incremental build.
- `presentation/` (`skeleton-presentation`): the capability layer, one crate. A stub extraction node that reads the state after the tick (it counts the alive entities) and writes nothing but its own counters.
- `facade/` (`skeleton-facade`): the facade. It re-exports the core and, when its `presentation` feature is on, the presentation crate. The feature flag lives here and nowhere else; the capability crate has no flag for its own presence.
- `src/main.rs` (`runtime-skeleton`): the host executable the research CI builds and runs. Its own `presentation` feature forwards to the facade's. Without the feature it is the headless and harness host; with it, the same hosts plus the extraction capability registered at world creation.

## Hypotheses

With the reference workload, version 1, default size (10 000 entities, 1 000 ticks), wander system off, compiled by one pinned Rust toolchain at default target features:

1. The crate set builds in both configurations, and the configuration without the flag has no dependency on the presentation crate: the crate is absent from the host's dependency graph as `cargo tree` resolves it, and no library of it is built.
2. Stepping under test time control reproduces the sequential reference's per-tick hashes, in both configurations and at 1 and 4 workers (and at the machine's core count): 1 000 ticks stepped back to back by a headless host; 1 000 ticks stepped by a harness host in batches of 1, 7 and 100 with a 1 ms pause after every batch (and once in batches of 1 without pauses), the batches of 100 alternating between two threads; a snapshot taken at tick 500 and restored into a new world that is stepped to tick 1 000, byte-identical when taken again, with the restored world and the original giving identical snapshots at tick 1 000.
3. Two worlds with different seeds stepped alternately in one process, in batches of 10, give the hashes each gives alone, and a panic injected in one world at tick 300 (in the second chunk of its repulsion pass) fails that world, whose history then stops at tick 299 and matches its reference up to there, whose further steps and snapshots are refused, and leaves the other world stepping to its reference hash.
4. The clean and incremental build times of both configurations are measured (release and dev profiles, the incremental ones after a one-line change in the core, three clean and four incremental samples per profile) on the five cells and the founder's machine, and the difference between the two configurations is the presentation crate's cost alone.
5. The spike runs with no display, GPU or audio device on every cell: it links the standard library only.

## Method

The binary runs its own configuration's time control scenarios in process, then measures the builds of both configurations in temporary target directories by timing `cargo build` as a child process (the clean builds from an empty target directory, the incremental ones after rewriting the marker constant in `core/src/edit_marker.rs`, which it restores), checks the dependency graphs with `cargo tree --edges normal`, and runs the release binary of the other configuration with `--time-control` to collect that configuration's scenarios too. Every world's tick hashes are compared with the sequential reference of its seed (the default seed, and a second seed for the second world); a comparison reports the first divergent tick, if any. The snapshot is the canonical bytes of the state at a tick boundary (a header, then per slot the generation, the alive flag and the five values of an alive slot, then the energy and the stream state), so that it never depends on which buffer of the double-buffered graph is current.

The self-checks (`cargo test -p skeleton-core -p skeleton-presentation -p runtime-skeleton`, and the host with `--features presentation`) verify on the small size that stepping by batches at 1, 2, 3 and 8 workers, a world stepped from another thread, a snapshot round trip and the restored world's continuation, two isolated worlds, a failed world (a panic injected in each of four nodes) and the extraction capability all reproduce the sequential hashes, that a corrupt snapshot is rejected with a reason, and that the edit marker guard restores the file.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=runtime-skeleton`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile for the measured binary, release and dev profiles for the timed builds, default target features of each target; plus a local run on an Apple M5 (10 cores, macOS 26) outside the matrix. Build times from hosted runners compare per processor and per image only; the crates are small, so the clean build times are dominated by the compiler's fixed costs and are reported with three samples.

## Result

Not measured yet.
