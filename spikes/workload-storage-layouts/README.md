# Reference workload storage layouts

Throwaway spike of the ECS research. It runs the [reference workload](../reference-workload/README.md) over three storage layouts and measures, instead of assuming, whether each layout reproduces the hashes of the sequential reference at every worker count on every cell of the desktop boundary, what the layouts cost one another in iteration throughput and in canonical traversals, what a second archetype costs the layouts that have no notion of archetype, and what change detection costs per chunk and per entity.

## Hypotheses

With the reference workload, version 1, default size (10 000 entities, 1 000 ticks), wander system off, compiled by one pinned Rust toolchain at default target features:

1. The three layouts (identifier-indexed slots, chunked archetype tables with a slot table, sparse sets per component) reproduce the run hash of the sequential reference at 1, 2, 4 and 8 workers on every cell, with the systems writing disjoint chunk slices in place (no copy back).
2. On the workload as specified (one archetype), the iteration throughput of the three layouts is within 20 percent of one another, and the canonical traversal through a slot table (the archetype layout's hash and spatial hashing walk the slot table and read every row through its location) costs under 10 percent of the hash phase.
3. With a second archetype introduced by a spike-local component that half the entities carry and one extra system reads and writes without touching the specified state (so that the reference hashes are unchanged), the archetype layout matches the extra system's query once per tick while the slot and sparse layouts filter or look up per entity, and the measured difference is the price of the query model.
4. Per-chunk change detection costs under 2 percent of the tick and per-entity change detection under 10 percent.

## Method

Three layouts run the same systems on the barrier pool of the [worker sweep spike](../workload-worker-sweep/README.md), re-implemented with the passes writing in place (a persistent set of threads, the main thread counting as one worker, a barrier at both ends of each parallel pass, storage chunks claimed through one atomic counter, the serial phases on the main thread):

- `slots`: identifier-indexed slots, the layout of the reference implementation: one row per slot, dead slots included; the storage chunk is a range of 256 slots; the canonical order is the storage order.
- `archetypes`: a slot table (generation, liveness, location) plus one table per archetype cut into chunks of 256 rows, each chunk a structure of arrays (slot, position, velocity, mass, and heat in the heat archetype); rows are appended on spawn and swap-removed on despawn, so the row order is a function of the command history and only the last chunk of a table is partial; the storage chunk is a table chunk; a system iterates the chunks of the archetypes its query matches; the spatial hashing and the hash walk the slot table in slot order and read each row through its location.
- `sparse`: one sparse set per component (position, velocity, mass, heat), each a dense array per field with the slot of every dense index and a slot-indexed index; a query iterates the dense array of its first component and looks the others up per entity (the sets are kept in lockstep but the layout does not assume it); the storage chunk is a range of 256 dense indices; the spatial hashing and the hash walk the slots in order with one lookup per component.

The spatial grid stores the position next to each slot: it is built from a canonical traversal that lists the alive entities in slot order (timed as `canonical_traversal`), then a counting sort (timed as `spatial_hashing`) that keeps the slot order inside each cell, and the repulsion reads its neighbors from the grid, never through the layout, so that the visit order and the arithmetic of the specification hold whatever the layout. Every layout computes each entity's energy into a per-slot scratch that the fixed point sums in the specified group order.

Two variants. The second archetype: a spike-local `heat` component (`f32`, 0 at spawn) carried by the even slots, and a cooling system that runs after the motion pass and computes `heat = heat * (1 - 2^-8) + (vx * vx + vy * vy) * 2^-10` from the velocity without touching the specified state; the archetype layout gives these entities their own table and the cooling system iterates that table only, the slot layout tests a flag per slot, the sparse layout iterates the heat set and looks the velocity up per entity. A second hash over the heat values in slot order checks that the three layouts and every worker count agree. Change detection, in the one-archetype mode: per chunk (the tick of the last write of the position and velocity columns, one store per written column per chunk) or per entity (one store per written component per entity).

Runs, all on the default size for 1 000 ticks with the wander system off, each after a warm-up instance of 10 ticks: the sequential reference; the three layouts in the one-archetype mode at 1, 2, 4 and 8 workers plus the machine's available parallelism when it is not in that list; the same in the two-archetype mode; the three layouts with per-chunk and per-entity change detection at 4 workers and, locally, at 10. Every run reports the run hash, the first tick at which it diverges from the sequential run of the same cell, the heat hash, ticks per second and the main thread's wall time per phase (canonical traversal, spatial hashing, repulsion, motion, cooling, structural changes, hash).

The self-checks (`cargo test -p workload-storage-layouts`) verify on the small size that every layout reproduces the sequential tick hashes at 1, 2, 3 and 8 workers with the wander system off and on, that the two-archetype mode leaves the reference hashes unchanged and gives one heat hash across layouts and worker counts, that change detection changes no hash, that the archetype tables and the sparse sets stay consistent (every row's slot points back at the row, only the last chunk of a table is partial) over 200 ticks of structural changes, and that the grid lists each cell in slot order and gives the reference's forces.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=workload-storage-layouts`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features of each target; plus a local run on an Apple M5 (10 cores, macOS 26) outside the matrix. The runners have 4 virtual CPUs (3 on macOS), so the 8-worker runs oversubscribe them on purpose; timings from hosted runners compare per processor only.

## Result

Not yet measured. The result is recorded here after the research CI run.
