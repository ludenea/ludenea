# Rapier snapshot continuation

Throwaway spike of the physics research, whose second stage compares the 2D and 3D physics libraries the engine could integrate behind a boundary of its own. It measures, instead of assuming, whether Rapier's serialized world gives identical continuation: a world restored from the bytes must step exactly like the world that was never interrupted, which is what rollback, replay and deterministic save games need from a physics library inside the deterministic domain. Box2D v3's measured continuation (the Box2D snapshot continuation spike) is the control it compares with. A second mode gives the pool sharing question its Rapier figure: the library has no task interface and runs its parallel parts on a Rayon pool, so the spike measures a handed-in pool against the step on one thread.

## Hypothesis

With Rapier 0.36.1 (`rapier2d` and `rapier3d`) built with the `enhanced-determinism`, `parallel` and `serde-serialize` features, on three scenes per dimension built from the library's own examples:

1. A world restored from the bytes reproduces the uninterrupted run at every step after the snapshot: the same world restored in place (its state replaced, its pipelines kept), a new world from the bytes on a thread pool of the same size, and a new world on a pool of another size (1 and 4 threads), and the world that was serialized keeps stepping like the uninterrupted one.
2. Two worlds serialized after the same number of steps give identical bytes, on one cell and across the five cells of the desktop boundary (Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only).
3. The hashes are identical across the five cells and across pool sizes.

Refuting thresholds: any divergent step; any byte difference between the two serializations.

Informational, not a hypothesis: the bytes per body, the time to serialize and to restore, the step time on the global pool restricted to one thread against handed-in pools of 1, 2, 4 and 8 threads (and the machine's own count when larger), and, in a build without the `parallel` feature, the step time of the sequential library.

## Method

A Rust program, a member of the spikes' Cargo workspace, depends on `rapier2d` and `rapier3d` at exactly 0.36.1 and on `bincode` 1.3.3 as the serde format that turns the world into bytes. Three scenes per dimension: `large_pyramid` (Rapier's port of Box2D's and Box3D's `large_pyramid` benchmarks, with a base of 100 boxes in 2D and 60 cubes in 3D so that the run stays bounded on the research CI runners), `joint_grid` (the ports of the `joint_grid` benchmarks, a 50 by 50 grid of balls linked by revolute joints in 2D and spherical joints in 3D), and `mixed_shape_rain` (balls, boxes and capsules placed at staggered heights over a ground box so that they land over the run, with sleeping allowed; the bodies exist from the start because Rapier's own `rain` spawns ragdolls over time and a world rebuilt from the bytes needs no per-step driver). Gravity is 10 downwards, the time step 1/60 s and the solver iterations 4, Rapier's defaults, as the Box2D scenes.

For each scene and each pool size (1 and 4 threads):

- the uninterrupted run steps 300 times and hashes every body's position and rotation after every step (bodies sorted by handle, components hashed field by field with FNV-1a 64), the reference;
- a second world runs the same first 100 steps, is serialized (`PhysicsWorld` derives `Serialize`; its solver and collision pipelines and its CCD solver are workspaces skipped by the derive) and continues for 200 steps (the first path);
- a third world runs the same first 100 steps and is serialized, so that its bytes compare with the second world's;
- the second world is restored in place from the bytes (its state fields replaced, its pipelines kept) and steps 200 times (the second path);
- a new world is deserialized from the bytes, given a pool of the same size and steps 200 times (the third path), and another one is given a pool of the other size (the fourth path).

Every path records whether the restore succeeded, whether the hash right after the restore equals the hash at the snapshot instant, the first step at which it diverges from the reference (or none), its final hash, and the time to restore. The pool sharing mode steps every scene 120 times on the global Rayon pool restricted to one thread, then on handed-in pools of 1, 2, 4, 8 threads and the machine's own count, hashing after every step; the final hashes must agree and the step times are the figure. `cargo test` runs the driver's own checks (hash vectors, order independence of the hash, two identical runs, the bytes giving back the snapshot instant in place and as a new world, a quick report). `ci/run-spike.sh rapier-snapshot-continuation <cell>` builds it with the pinned toolchain and saves `facts.json` and `environment.txt`; `--quick` runs a tenth of the steps. A build with `--no-default-features` drops the `parallel` feature and measures the sequential library in the pool sharing mode.

Relationship to the external components: Rapier 0.36.1 (Apache 2.0) is the subject of the measurement; `bincode` 1.3.3 (MIT) is the serializer the measurement needs and `rayon` 1 the pool the library accepts; nothing of them is vendored or reused beyond this spike.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=rapier-snapshot-continuation`. Release build at the pinned toolchain (Rust 1.99.1), default target features (no AVX2 or FMA selected at compile time; the `enhanced-determinism` feature forces the library's transcendental functions through `libm`). Plus a local run on the founder's machine (AMD Ryzen 9 9950X3D, 16 cores and 32 logical processors, Windows 11) outside the matrix.

## Result

Local run of 2026-11-09 on the founder's machine, the parallel build: the 48 paths (2 dimensions, 3 scenes, 2 pool sizes, 4 paths) all restore, give back the hash of the snapshot instant and reproduce the uninterrupted run at every one of the 200 steps that follow; the third world's bytes equal the second world's in the 12 runs; the uninterrupted final hashes are the same at 1 and 4 threads for every scene, and the pool sharing mode gives one final hash per scene on the one-thread global pool and on every handed-in pool size up to 32 threads. Hypotheses 1 and 2 hold on this machine; hypothesis 3 holds across pool sizes here and awaits the cells.

| Dimension | Scene | Bodies | Snapshot bytes | Bytes per body | Serialize ms | Restore in place ms | New world ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2D | `large_pyramid` | 5 051 | 10 017 241 | 1 983 | 3.9 | 13 | 9 |
| 2D | `joint_grid` | 2 500 | 2 913 891 | 1 166 | 0.9 | 3.3 | 2.7 |
| 2D | `mixed_shape_rain` | 1 201 | 768 940 | 640 | 0.3 | 1.5 | 0.8 |
| 3D | `large_pyramid` | 1 831 | 7 102 072 | 3 879 | 2.4 | 6.3 | 5.6 |
| 3D | `joint_grid` | 2 500 | 4 207 196 | 1 683 | 1.4 | 3.8 | 3.5 |
| 3D | `mixed_shape_rain` | 1 441 | 1 511 895 | 1 049 | 0.5 | 1.7 | 1.2 |

Step times of the pool sharing mode, total milliseconds for 120 steps, on the one-thread global pool then on handed-in pools of 1, 2, 4, 8 and 32 threads: 2D `large_pyramid` 388, 378, 339, 274, 219, 274; 2D `joint_grid` 97, 88, 79, 52, 66, 115; 2D `mixed_shape_rain` 31, 24, 30, 34, 41, 63; 3D `large_pyramid` 314, 312, 261, 153, 135, 157; 3D `joint_grid` 256, 264, 183, 154, 116, 152; 3D `mixed_shape_rain` 93, 74, 70, 64, 78, 102. The sequential build (`--no-default-features`, run on the same machine the same day) gives the same final hash as the parallel build for every scene (the `parallel` feature changes no result) and these totals for the same 120 steps: 2D `large_pyramid` 393, 2D `joint_grid` 94, 2D `mixed_shape_rain` 26, 3D `large_pyramid` 293, 3D `joint_grid` 190, 3D `mixed_shape_rain` 61. Reading: the parallel build on one thread costs nothing over the sequential build on the 2D scenes and 7 to 35 percent on the 3D scenes, single runs on a desktop machine, noise included; a handed-in pool of one thread costs nothing over the one-thread global pool; the large scenes gain 1.8 to 2.3 times at 8 threads and lose part of it at 32; the small rain scenes gain nothing and lose with more threads. The five cells' results are recorded below when measured.

The raw measurements and the environment of each cell are the artifacts of the run. The result holds for Rapier 0.36.1 built with the pinned toolchain on these machines.
