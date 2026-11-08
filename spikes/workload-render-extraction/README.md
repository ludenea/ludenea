# Render extraction

Throwaway spike of the rendering research. It replaces the stub extraction node of the [runtime skeleton](../runtime-skeleton/README.md) spike by a real one on the [reference workload](../reference-workload/README.md): after every tick, a chunked, parallel pass reads the state after the tick and writes, per slot, the position and a sprite handle into one of two render buffers that the renderer owns, while the next tick's passes run on the other state buffer. It measures, instead of assuming, that the extraction leaves the simulation untouched, that the render state's content is a function of the simulation state alone, what the extraction costs per tick, and what a per-chunk change detection skip saves for chunks no system wrote.

## Layout

- `src/world.rs`: a trimmed copy of the runtime skeleton spike's core (the reference workload stepped on the dependency-counting tick graph, stepped by whole ticks; no snapshot, no injection) with a sixth node, the extraction. It is armed when the structural node of a tick completes, together with the hash of that tick; both read the state after the tick while the next tick's passes write the other buffer; the structural node of the next tick waits for the group of the two, as it waited for the hash alone. The render state is two buffers of three columns (`x`, `y`, `sprite`, the sprite handle 1, 2 or 3 for the masses 1, 2 and 4, 0 for a dead slot); the extraction of tick `t` writes buffer `t % 2` chunk by chunk through disjoint slices. Change detection is a flag per chunk and per tick parity, set by the motion pass that writes the chunk and by the structural changes for the chunks their kills and spawns touch, cleared by the structural node for the parity whose extraction is complete. With the skip on, a chunk whose flag is not set is copied from the previous tick's render buffer instead of gathered from the state. The static-half variant freezes the upper half of the chunks: their repulsion and motion passes copy the current values to the other buffer and mark nothing, so that only the structural changes ever write them; it is a different simulation, compared across worker counts and modes only, never with the reference.
- `src/main.rs`: the scenarios and the JSON report. Both variants (normal, static half), three modes (extraction off, the skeleton's world and the baseline; on, every chunk gathered; skip), at 1, 2, 4 and 8 workers and the machine's CPU count when different; every world warmed up by a throwaway world of ten ticks and stepped in batches of 100 ticks, the host hashing the render state after every batch as a renderer would read it between ticks.

## Hypotheses

With the reference workload, version 1, default size (10 000 entities, 40 chunks of 256 slots, 1 000 ticks), wander system off, compiled by one pinned Rust toolchain at default target features:

1. The extraction changes nothing in the simulation: in every mode and at every worker count the normal variant reproduces the sequential reference's hash at every tick, and the static-half variant reproduces, at every worker count and in every mode, the hashes it gives at one worker without extraction.
2. The render state's content is a function of the simulation state alone: the run hash of the render state hashes (one per batch of 100 ticks) is identical across worker counts, and identical with and without the skip, in both variants.
3. The extraction of 10 000 entities costs under 5 percent of the tick at 4 workers: measured as the processor time of its chunks per tick (the sum of the chunk durations) against the tick's wall time, and as the throughput of the world with the extraction against the baseline without it.
4. The skip removes the gather for the chunks no system wrote: in the static-half variant, about half of the chunks are copied from the previous render buffer every tick (less on the ticks whose kills and spawns touch a static chunk), and the extraction's processor time per tick falls accordingly; in the normal variant the skip copies nothing, since every chunk is written every tick.

## Method

The binary runs the thirty worlds of the matrix (two variants, three modes, five worker counts on a 10-core machine; four on a 4-CPU runner) and reports per run the run hash, the comparison with the reference or with the one-worker baseline of the variant, the first divergent tick if any, the run hash and the final hash of the render state, the ticks per second, the ratio to the baseline of the same variant and worker count, the extraction's processor time per tick and its share of the tick's wall time, and the chunks gathered and copied per tick. The render state hashing is done by the host between batches, outside the measured steps.

The self-checks (`cargo test -p workload-render-extraction`) verify on the small size over 40 ticks, stepped in batches of 7, that every mode reproduces the sequential hashes at 1, 2 and 3 workers; that the render run hash is identical across worker counts and skip settings in both variants; that the static-half variant is identical across worker counts and modes and differs from the reference; that with the skip in the static-half variant some chunks are copied and only the static ones ever are, while the normal variant copies nothing and the baseline extracts nothing; that every slot of the render state holds the position and the sprite handle of its entity (the population is full at every tick boundary, the spawns refilling the kills within the tick, so the dead form of a slot is never observable between steps); and that the report holds every run with every world matching.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=workload-render-extraction`. Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml), release profile, default target features of each target; plus a local run on an Apple M5 (10 cores, macOS 26) outside the matrix. Timings from hosted runners compare per processor only; the x86_64 runners' 4 virtual CPUs are two cores with hyperthreads.

## Result

Measured on 2026-11-08 by the research CI on the five cells (run 42518321484, the raw measurements and the environment of each cell in its artifacts), after a local validation run on 2026-11-03 on the founder's machine (Apple M5, 10 cores, macOS 26, taken while other builds ran). Every world matches on every cell: the normal variant reproduces the sequential reference at every tick in every mode at 1, 2, 4 and 8 workers (and 3 on the macOS runner), 24 of 24 runs per cell and 30 of 30 on macOS (hypothesis 1); the static-half variant reproduces its one-worker baseline everywhere; and the render state run hash is one value per variant across the five cells, every worker count and both skip settings (hypothesis 2: `0x2bb3dba263d87df3` normal, `0x94aa02d647c77acc` static half, the values of the local run).

| Cell | Variant | Workers | Baseline ticks per second | Extraction on: processor us per tick, share of the tick, ratio to baseline | Skip: processor us per tick, chunks copied per tick, ratio to baseline |
| --- | --- | --- | --- | --- | --- |
| linux-x86_64 | normal | 1 | 553 | 21.8 us, 1.2 percent, 0.98 | 21.3 us, 0, 0.99 |
| linux-x86_64 | normal | 4 | 1 324 | 58.2 us, 7.7 percent, 1.00 | 58.6 us, 0, 1.00 |
| linux-arm64 | normal | 1 | 741 | 23.7 us, 1.7 percent, 0.98 | 23.7 us, 0, 0.98 |
| linux-arm64 | normal | 4 | 2 213 | 29.6 us, 7.3 percent, 1.11 | 28.7 us, 0, 1.15 |
| windows-x86_64 | normal | 1 | 591 | 20.9 us, 1.2 percent, 0.98 | 20.8 us, 0, 0.98 |
| windows-x86_64 | normal | 4 | 1 464 | 55.2 us, 8.0 percent, 0.99 | 53.8 us, 0, 0.99 |
| windows-arm64 | normal | 1 | 730 | 23.5 us, 1.7 percent, 0.99 | 23.6 us, 0, 0.99 |
| windows-arm64 | normal | 4 | 2 532 | 29.0 us, 7.3 percent, 1.00 | 29.0 us, 0, 1.00 |
| macos-arm64 | normal | 1 | 601 | 19.6 us, 1.2 percent, 0.99 | 19.8 us, 0, 0.98 |
| macos-arm64 | normal | 3 | 1 668 | 21.8 us, 3.3 percent, 0.92 | 22.1 us, 0, 0.92 |
| linux-x86_64 | static half | 1 | 876 | 21.1 us, 1.8 percent, 0.98 | 14.7 us, 16.4 of 20 static chunks, 0.98 |
| linux-x86_64 | static half | 4 | 1 912 | 57.2 us, 10.8 percent, 0.99 | 39.6 us, 16.4, 0.99 |
| linux-arm64 | static half | 1 | 1 201 | 23.8 us, 2.8 percent, 0.97 | 16.2 us, 16.4, 0.98 |
| linux-arm64 | static half | 4 | 3 984 | 28.4 us, 11.3 percent, 1.00 | 23.5 us, 16.4, 1.00 |
| windows-arm64 | static half | 1 | 1 190 | 23.4 us, 2.7 percent, 0.97 | 16.1 us, 16.4, 0.98 |
| windows-x86_64 | static half | 4 | 2 214 | 54.2 us, 11.5 percent, 0.96 | 37.0 us, 16.4, 0.98 |

- Hypothesis 3 holds as throughput and not as the share stated: the world with the extraction runs within the noise of the baseline on every cell (ratios 0.98 to 1.15 on the Linux and Windows cells at 1 to 8 workers; the macOS runner's ratios spread from 0.47 to 1.53 across its runs, its known variance, 3 CPUs oversubscribed by 8 workers included). The extraction's processor time per tick is 20 to 24 microseconds at 1 and 2 workers on every cell (1.2 to 1.7 percent of the tick at 1 worker, 2.3 to 3.4 at 2), and 29 to 30 microseconds at 4 workers on the ARM64 cells (7.3 percent of a 0.4 ms tick) against 55 to 58 on the x86_64 cells (7.7 to 8.0 percent of a 0.7 ms tick), where the forty chunks spread over the two cores' hyperthreads cost 2.6 times their single-worker processor time. At 4 workers on the 4-CPU runners the share of processor time is above the 5 percent the hypothesis stated, while the throughput cost stays inside the noise because the extraction overlaps the next tick's passes. On the founder's 10-core machine the share was 5.2 percent at 4 workers.
- Hypothesis 4 holds on every cell: in the static-half variant with the skip, 16.4 of the 20 static chunks are copied per tick on average (the tick's kills and spawns touch 3.6 static chunks on average), and the extraction's processor time falls by about 30 percent (from 21 to 24 microseconds to 15 to 16 at 1 worker; from 57 to 40 at 4 workers on the x86_64 cells, from 28 to 24 on the ARM64 cells). The copy of a chunk's 3 KB render slice costs about a third of its gather. In the normal variant the skip copies nothing and costs nothing measurable beyond the flag read.

The result holds for the reference workload version 1 at this toolchain, on these runner images and the founder's machine.
