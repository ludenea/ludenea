# Reference workload worker sweep

Throwaway spike of the deterministic simulation research. It runs the [reference workload](../reference-workload/README.md) in parallel at several worker counts and measures, instead of assuming, whether constructs that are deterministic by construction (per-slot results independent of the processing order, a reduction in fixed chunk order) give the same state hashes as the sequential reference on every cell of the desktop boundary, and what they cost against a free-running reduction.

## Hypothesis

With the reference workload, version 1, compiled by one pinned Rust toolchain at default target features:

1. The sequential reference implementation produces identical run hashes on the five cells (Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only) with the wander system off: the IEEE-exact core is reproducible across the boundary.
2. The parallel implementation with deterministic constructs produces the hashes of the sequential reference at 1, 2, 4 and 8 workers, on every cell, whatever order the workers claim their work in.
3. With the wander system on, which adds the platform's `f32` cosine and sine to every velocity, the hashes are either identical on the five cells or split per operating system family: a measurement of math library variance on a realistic workload.
4. Rebuilding with `-C target-cpu=x86-64-v3` on the x86_64 cells, and building without optimizations (the `dev` profile), leave every hash unchanged.
5. The deterministic reduction costs a measurable but bounded share of throughput against the free-running reduction, which accumulates in arrival order and merges worker totals in completion order.

## Method

The spike runs the default size (10 000 entities, 1 000 ticks) with the wander system off and on, three ways:

- `sequential`: the reference implementation of `reference-workload`, the comparison point of the cell.
- `deterministic`: a persistent pool of threads (the main thread counts as one worker) synchronized by a barrier at both ends of each parallel pass. The tick runs the spatial hashing on the main thread, then a repulsion pass and a motion pass (wander, integration, boundary, energy) over chunks of 256 slots claimed through an atomic counter, so that the assignment of chunks to workers changes from run to run. A pass writes into per-chunk output buffers that the main thread copies back between passes; each slot's result depends on its own fixed neighbor visit order only. The energy of each chunk is summed in slot order inside the chunk, and the chunk sums are added in chunk order on the main thread. Structural changes and the hash run on the main thread.
- `free-running`: the same pool and passes, with the energy reduction a non-deterministic scheduler would use: each worker adds the energies it meets in arrival order across the chunks it claimed, and the worker totals are merged in completion order.

Each parallel mode runs at 1, 2, 4 and 8 workers. Every run reports the run hash, the first tick at which it diverges from the sequential run of the same cell and wander setting, the throughput as ticks per second after a warm-up instance of 10 ticks, and the time spent per phase (spatial hashing, repulsion pass, motion pass, copy back, structural changes, hash). The research CI's compare job counts the distinct run hashes per mode and wander setting across cells and worker counts.

The self-checks (`cargo test -p workload-worker-sweep`) verify the spike's own logic on the small size: the deterministic runs at 1, 2, 3 and 8 workers reproduce the sequential tick hashes with the wander system off and on; a free-running run has the same positions, velocities, slots and stream as the reference after every tick and differs through its energy only; a pool with more workers than chunks works.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=workload-worker-sweep`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features of each target; plus a rebuild with `-f rustflags='-C target-cpu=x86-64-v3'` on the two x86_64 cells, a build with `-f profile=dev` on Linux x86_64, and a local run on an Apple M5 (10 cores, macOS 26) outside the matrix. The runners have 4 virtual CPUs (3 on macOS), so the 8-worker runs oversubscribe them on purpose.

## Result

Local run of 2026-11-02 on the Apple M5, release profile:

| Mode | Workers | Wander off: run hash | Wander on: run hash | Matches the sequential run | Ticks per second, wander off |
| --- | --- | --- | --- | --- | --- |
| sequential | 1 | `0x9f50cc4763ae733c` | `0xd4f14b2dd11228d1` | reference | 1 033 |
| deterministic | 1, 2, 4, 8 | `0x9f50cc4763ae733c` | `0xd4f14b2dd11228d1` | yes, at every worker count | 986, 1 481, 1 918, 2 007 |
| free-running | 1, 2, 4, 8 | four distinct values | four distinct values | no: the energy diverges at tick 1 | 974, 1 476, 1 883, 1 959 |

The deterministic constructs cost nothing measurable against the free-running reduction (the two modes are within 2 percent at every worker count). The scaling is bounded by the sequential phases: at 10 000 entities the per-tick hash of the whole state (210 KB through FNV-1a, 0.22 ms) costs as much as 30 percent of the sequential tick and more than the parallel repulsion pass at 8 workers (0.18 ms), with the spatial hashing, the copy back and the structural changes adding 0.05 ms.

The research CI result on the five cells is recorded below once the run completes; the raw measurements and the environment of each cell are the artifacts of the run.
