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

Research CI [run](https://github.com/ludenea/ludenea/actions/runs/41492150925) of 2026-11-02 on the five cells, the [rebuild](https://github.com/ludenea/ludenea/actions/runs/41492168053) with `-C target-cpu=x86-64-v3` on the two x86_64 cells, the [unoptimized build](https://github.com/ludenea/ludenea/actions/runs/41492203328) on Linux x86_64, and a local run the same day on an Apple M5 (10 cores, macOS 26) outside the matrix. Ticks per second of the default size with the wander system off (the wander-on figures are 5 to 15 percent lower):

| Cell | Machine | Sequential | Deterministic at 1, 2, 4, 8 workers | Free-running at 1, 2, 4, 8 workers |
| --- | --- | --- | --- | --- |
| `linux-x86_64` | AMD EPYC 9V74, 4 vCPU | 495 | 473, 671, 806, 729 | 468, 680, 800, 705 |
| `linux-arm64` | Neoverse N2, 4 vCPU | 756 | 711, 1 100, 1 508, 1 344 | 710, 1 105, 1 502, 1 336 |
| `windows-x86_64` | AMD EPYC 9V74, 4 vCPU | 585 | 566, 831, 937, 799 | 570, 821, 926, 799 |
| `windows-arm64` | Cobalt 100, 4 vCPU | 751 | 707, 1 096, 1 274, 1 410 | 706, 1 097, 1 503, 1 406 |
| `macos-arm64` | Apple M1 virtual, 3 vCPU | 567 | 476, 695, 856, 874 | 542, 828, 884, 954 |
| local | Apple M5, 10 cores | 1 033 | 986, 1 481, 1 918, 2 007 | 974, 1 476, 1 883, 1 959 |

- Hypothesis 1 holds: with the wander system off, the sequential run hash is `0x9f50cc4763ae733c` on the five cells, on the x86-64-v3 rebuilds, on the unoptimized build and on the local machine. The other sizes are in [`reference-hashes.md`](../reference-workload/reference-hashes.md).
- Hypothesis 2 holds: every deterministic run reproduces the sequential run of its cell at every tick, at 1, 2, 4 and 8 workers, with the wander system off and on: 40 of 40 runs on the five cells, 16 of 16 on the x86-64-v3 rebuilds, 8 of 8 on the unoptimized build, 8 of 8 on the local machine. With the wander system off, all of them share the single hash above.
- Hypothesis 3 settles with a surprise: with the wander system on, the hashes split three ways, not two. Linux x86_64, Linux ARM64 and Windows ARM64 agree (`0x756937193f1eb200`); macOS differs (`0xd4f14b2dd11228d1`, which the local Apple M5 reproduces); Windows x86_64 differs from both (`0x66f88ba45f145858`), Windows ARM64 included. Over the 4 096 distinct arguments the wander system feeds to the `f32` cosine and sine, the platform math library differs per architecture within Windows, which the eight `f32` probes of the platform facts spike had not shown.
- Hypothesis 4 holds: the x86-64-v3 rebuild and the unoptimized build reproduce every hash of the default release build of their cell, wander on included.
- Hypothesis 5 does not hold as stated: the deterministic reduction costs nothing measurable. Deterministic and free-running runs are within the runners' timing noise of each other at every worker count (the macOS runner, the noisiest, shows the free-running runs faster by up to 9 percent; the Windows ARM64 runner shows one free-running run slower by half). The free-running runs never match the sequential run: their energy diverges at tick 1, and the 20 runs per wander setting give 16 and 18 distinct hashes (the single-worker free-running runs are deterministic, with one hash per math library class).
- Scaling is bounded by the sequential phases. The repulsion pass scales by 3.3 from 1 to 4 workers on the ARM64 Linux cell, by 2.4 to 2.7 on the x86_64 and Windows ARM64 cells and by 2.1 on the 3-vCPU macOS cell, while the per-tick hash of the whole state (210 KB through FNV-1a) costs 224 ms per 1 000 ticks on the ARM64 cells, 275 to 320 ms on the x86_64 cells and 410 ms on the macOS cell: 30 to 40 percent of the deterministic run at 4 workers. The spatial hashing costs 164 to 190 ms on Linux x86_64 against 28 to 32 ms on the ARM64 cells and 95 ms on Windows x86_64, a difference the spike does not explain. Eight workers on 4 virtual CPUs cost up to 11 percent against 4 workers on three cells and nothing on the two others; unlike the spin-waiting workers of the Box2D worker sweep, this pool sleeps at its barrier, and the hosted macOS runner scales with it.

The raw measurements and the environment of each cell are the artifacts of the runs. The result holds for the reference workload version 1 at this toolchain and on these images.
