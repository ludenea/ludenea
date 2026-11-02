# Reference workload structural changes

Throwaway spike of the ECS research. It runs the [reference workload](../reference-workload/README.md) over the three storage layouts of the [storage layouts spike](../workload-storage-layouts/README.md) and measures, instead of assuming, what structural changes (spawn, despawn, add and remove of a component) cost each layout at 8, 80 and 800 changes per tick, whether command buffers recorded by parallel chunks and applied at a fixed point keep the results independent of the worker count, what the two identifier allocation paths cost, and how far the free list policies are apart.

## Hypotheses

With the reference workload, version 1, default size (10 000 entities, 1 000 ticks), wander system off, compiled by one pinned Rust toolchain at default target features:

1. Command buffers per system and per chunk, applied at the fixed point in chunk order and record order, give identical hashes at 1 and 4 workers when despawns, spawns and component changes are recorded from a parallel system (a spike-local mode in which the motion pass records them by a deterministic rule on the slot index and the tick, since the specified kills consume the global stream serially).
2. Both identifier paths (allocation when the spawn command is applied; per-chunk reservation when it is recorded) are deterministic across worker counts.
3. The cost per structural change at 8, 80 and 800 changes per tick differs between layouts by the predicted order (a flag flip for identifier-indexed slots, a swap-remove per component for sparse sets, a row move for archetype tables) and the free list policies (lowest-index scan, lowest-index heap, stack) differ by orders of magnitude at 800 changes per tick.
4. The application at the fixed point stays under 5 percent of the tick at 80 changes per tick in every layout.

## Method

The three layouts (`slots`, `archetypes`, `sparse`) and the barrier pool are those of the storage layouts spike, with two additions: the slot table may grow, and an entity may gain or lose a spike-local `heat` component (a row move between the plain table and the heat table in the archetype layout, a flag in the slot layout, an insert or remove in a sparse set).

Recording modes:

- `main-thread`: the kills and spawns of the specification, drawn from the global stream at the fixed point, at the specified rate (8 kills and 8 spawns per tick) and at 80 and 800. Spawns reuse dead slots through the free list policy; as specified, a spawn with no dead slot is skipped and draws nothing, and the slot table never grows.
- `parallel`: the motion pass records, into a command buffer per chunk, in the storage order of its rows, a despawn and a spawn for every entity that a deterministic rule on the slot index and the tick selects (about the rate per tick), and a heat toggle (add if absent, remove otherwise) for every entity a second rule selects (about the rate per tick); the spawned entity's values are a function of the despawned slot and the tick. The fixed point applies the buffers in chunk order and record order. A spawn applied before the despawns recorded after it may find no free slot, in which case the slot table grows.

Identifier allocation, in the parallel mode: `at-application` (the spawn's slot is taken from the free list, or added, when the command is applied) or `reserved` (before the motion pass, every chunk reserves, in chunk order, the expected number of despawns per chunk plus one, and records its spawns with those slots; a chunk that runs out spawns through the application path, counted as an overflow; unused reservations are returned at the fixed point in chunk order).

Free list policies: `scan` (the lowest dead slot, found by a linear scan of the liveness column, as the specification states), `heap` (the lowest free slot, from a binary heap of freed slots), `stack` (the most recently freed slot). In the main-thread mode at the specified rate, `scan` and `heap` follow the specification and their hashes compare with the sequential reference; every other configuration compares its hashes across worker counts only.

Runs, all on the default size for 1 000 ticks with the wander system off, each after a warm-up instance of 10 ticks, at 1 and 4 workers: the main-thread mode with the three policies at the three rates on the three layouts; the parallel mode with the two allocation paths and the heap and stack policies at the three rates on the three layouts. Every run reports the run hash, the first tick at which it diverges from the sequential reference, the heat hash, ticks per second, the main thread's wall time per phase (canonical traversal, spatial hashing, reservation, repulsion, motion, which includes the recording, the energy reduction, apply, hash), the counts of despawns, spawns, adds, removes, reserved and overflow spawns, returned reservations and added slots, the microseconds per change spent at the fixed point (apply and reservation), the share of the tick those take, and the final slot count.

The self-checks (`cargo test -p workload-structural-changes`) verify on the small size that the specified configuration reproduces the sequential tick hashes on every layout with the scan and heap policies at 1 and 3 workers; that the stack policy and the higher rates diverge from the specification but not across worker counts; that the main-thread mode gives one hash across layouts; that the parallel mode gives one run hash, one heat hash and one slot count at 1, 2, 3 and 8 workers for every layout, allocation path and policy, with a constant population; that the archetype tables and the sparse sets stay consistent under parallel structural changes; and that the free list policies pick the slots they promise.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=workload-structural-changes`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features of each target; plus a local run on an Apple M5 (10 cores, macOS 26) outside the matrix. Timings from hosted runners compare per processor only.

## Result

Not yet measured. The result is recorded here after the research CI run.
