# Reference workload frame allocator

Throwaway spike of the memory research. It runs the [reference workload](../reference-workload/README.md) on the dependency-counting tick graph the [scheduling overhead spike](../workload-scheduling-overhead/README.md) measured, with per-chunk temporaries the earlier spikes did not have, under four allocation modes, and measures, instead of assuming, whether the state hashes depend on allocation, what the general heap costs under parallel passes on the three system allocators of the desktop boundary, whether a per-worker scratch removes every steady-state heap allocation at no cost in throughput, what a counting allocator wrapper costs, whether table growth at the fixed point is invisible to the results, and whether per-tag accounting returns to zero at destruction and names a leaked buffer.

## Hypotheses

With the reference workload, version 1, default size (10 000 entities, 1 000 ticks), wander system off, compiled by one pinned Rust toolchain at default target features:

1. The workload reproduces the sequential reference's hashes at 1, 2, 4 and 8 workers on every cell in four allocation modes (`pre-sized`, `scratch`, `heap`, `heap-counted`, below), and the wall-bounce event log that the motion pass records into per-chunk buffers and the fixed point merges in chunk order gives one hash across modes and worker counts: allocation neutrality holds in every mode.
2. The scratch mode makes zero general-heap allocations per tick in steady state (counted by the wrapper from the eleventh tick on) and costs no throughput against the pre-sized baseline, while the heap mode costs a measured share of the 4-worker tick that differs per system allocator (glibc malloc, the macOS allocator, HeapAlloc).
3. A growth mode, in which the workload starts with a capacity equal to its population, spawns 80 entities per tick for 1 000 ticks or 800 per tick for 100 ticks without kills, and grows every table at the fixed point when the spawns exceed the capacity (1.5 times the capacity at least, rounded up to whole chunks), gives the same run hash and event hash as the same run preallocated at its final population, at every worker count, and the growth costs an amortized share of the run under 5 percent at 80 spawns per tick.
4. The counting wrapper costs under 1 percent of the tick: the heap-counted mode's throughput is within 1 percent of the heap mode's, and the microbenchmark of the wrapper against the system allocator reached directly gives the cost per allocation and free pair.
5. A world destroyed at the end of a run returns every tag's counters to zero and leaves no block on the heap (the allocation and free counts before creation and after destruction agree), and a run that leaks one pre-sized buffer on purpose reports it by its tag and as one block on the heap.

## Method

The graph is the counter variant of the scheduling overhead spike with its in-place chunk slices. Three temporaries per chunk were added so that the allocation modes have something to allocate: the repulsion pass gathers the forces of its chunk into a buffer before applying them to the velocities (the same arithmetic in the same order), the motion pass writes the energies of its chunk into a buffer before adding them in index order into the chunk's group sum, and records a wall-bounce event (slot and axis) for every reflection into a per-chunk event buffer that lives until the fixed point; the spatial hashing takes its cursor array from the same source instead of cloning it. The modes decide where these live:

- `pre-sized`: per-worker buffers for the forces and energies, per-chunk buffers for the events and one cursor, all sized at creation and reused across ticks.
- `scratch`: a bump allocator per worker (an initial block of 64 KB, then blocks twice the last from the heap when a block is full, the counted fallback, after which the capacity stays at the high-water mark), reset at the first task a worker runs in a new tick; a task uses the scratch of the worker that runs it, and the events of a chunk stay valid until the fixed point because the reset happens only after it.
- `heap`: every temporary allocated from the global allocator at chunk start and freed at chunk end, the event buffers freed at the fixed point by the thread that runs it, with the counting wrapper switched off.
- `heap-counted`: the heap mode with the counting wrapper on.

The program's global allocator is a wrapper around the system allocator that, when switched on, counts allocations, frees, reallocations and bytes in a counter per thread (each on its own cache line, updated with relaxed atomics by its own thread only) and whose totals the fixed point reads to record the allocations of every tick. Every allocation site of the spike's own classes belongs to a tag (tables, scratch, buffers, events, temporaries) with per-worker counters summed at destruction. The growth mode uses the scratch mode for the temporaries and compares the `grow` variant with the `preallocated` one at 1 and 4 workers (and the machine's core count when larger). The leak run uses the pre-sized mode at 4 workers for 50 ticks and forgets one event buffer at destruction. The microbenchmark allocates and frees 1 KB a million times per thread on 1 and 4 threads through the system allocator directly, through the wrapper with counting off and with counting on, in three interleaved rounds whose median and minimum are reported.

Every run reports the run hash, the first divergent tick against the sequential reference, the event hash and count, ticks per second, the busy time per node, the steady-state allocations and bytes per tick (mean and maximum from the eleventh tick on), those of the first ten ticks and the last tick in which the heap was reached, the heap's net blocks and bytes after destruction (when the wrapper counted), the scratch's high-water mark, capacity and fallback blocks, the tables' final population, capacity, bytes and growth events, and the tags' bytes and allocation counts at destruction.

The self-checks (`cargo test -p workload-frame-allocator`) verify on the small size that the four modes reproduce the sequential hashes at 1, 2, 3 and 8 workers with one event hash, that the wander system runs with the right tick, that the pre-sized and scratch modes allocate nothing in steady state while the heap mode allocates per chunk, that the scratch grows through counted fallbacks and restarts from its first block after a reset, that growth changes no hash against the preallocated variant and across worker counts, that a leaked buffer is reported by its tag and as one heap block, and that the wrapper counts what a thread allocates.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=workload-frame-allocator`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features of each target; plus a local run on an Apple M5 (10 cores, macOS 26) outside the matrix. Timings from hosted runners compare per processor only.

## Result

Not measured yet.
