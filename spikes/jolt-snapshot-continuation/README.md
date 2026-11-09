# Jolt snapshot continuation

Throwaway spike of the physics research, whose second stage compares the 3D physics libraries the engine could integrate behind a boundary of its own. It measures, instead of assuming, whether Jolt's save state gives identical continuation: a world restored from the saved state must step exactly like the world that was never interrupted, which is what rollback, replay and deterministic save games need from a physics library inside the deterministic domain. Box2D v3's measured continuation (the Box2D snapshot continuation spike) is the control it compares with. The library runs on a job system the spike implements over a fixed pool of threads, the way an engine with a pool of its own would drive it.

## Hypothesis

With Jolt Physics v5.6.0 built with its cross-platform determinism option (which also disables fused multiply-adds and selects precise floating point in the library's own flags), the spike's own code built with contraction off, on three scenes from the library's performance tests and samples:

1. A world restored from the saved state reproduces the uninterrupted run at every step after the save: the same world restored in place, and a world built from the same scene description in the same creation order then restored, on a job system of the same thread count and of another thread count (1 and 4), and the world that was saved keeps stepping like the uninterrupted one.
2. Two worlds saved after the same number of steps give identical bytes, on one cell and across the five cells of the desktop boundary (Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only).
3. The hashes are identical across the five cells and across thread counts.
4. The save state restored into a world built from the same scene description in the same creation order continues identically, which is what the samples' determinism check exercises and the documentation does not promise (it says the body identifiers must match, which the same creation order gives).

Refuting thresholds: any divergent step; any byte difference between the two saves; a restore refused, or a continuation that diverges at the first step, marks the candidate for the recreation path.

Informational, not a hypothesis: the state bytes per body, the time to save and restore, and the step time at 1 and 4 threads on the spike's job system.

## Method

A C++17 program fetches Jolt at tag `v5.6.0` through CMake with `CROSS_PLATFORM_DETERMINISTIC` on, link-time optimization off (the library asks consumers to match it otherwise), the dynamic MSVC runtime, and none of the extras the measurement does not need (debug renderer, profiler, object stream, GPU compute, floating-point exceptions, warnings as errors). Its job system derives from Jolt's barrier base class (`JobSystemWithBarrier`, whose waiting thread runs the barrier's jobs while it waits) and implements the rest over a fixed pool: a thread count that counts the stepping thread, so that it compares with Box2D's and Rapier's worker counts (four threads start three workers; one thread starts none and every job runs on the stepping thread inside the barrier's wait), one queue under a mutex and a condition variable that the workers claim from, and the job slots in Jolt's fixed-size free list.

Three scenes, in the creation order of the library's own code: `pyramid` (`PerformanceTest/PyramidScene.h`, a pyramid of 1 240 boxes without convex radius on a floor, sleeping disabled), `convex_vs_mesh` (`PerformanceTest/ConvexVsMeshScene.h`, 1 764 boxes, spheres, capsules and convex hulls dropped on a sloping mesh terrain, with the scene's reduced solver iteration counts), and `point_constraint_grid` (the construction of `Samples/Tests/Constraints/PointConstraintTest.cpp`, bodies created then added and a point constraint between neighbours at their shared point, laid out as the joint grid of the Box2D and Rapier scenes: a 30 by 30 grid of spheres, a column of static anchors, the grid's bodies colliding with the non-moving layer only). Gravity is 9.81 downwards, the time step 1/60 s with one collision step, Jolt's defaults.

For each scene and each thread count (1 and 4):

- the uninterrupted run steps 300 times and hashes every body's position and rotation after every step (bodies sorted by identifier, which the same creation order makes stable across worlds, components hashed field by field with FNV-1a 64), the reference;
- a second world runs the same first 100 steps, saves its whole state through the state recorder (`PhysicsSystem::SaveState`, global state, bodies, contacts and constraints) and continues for 200 steps (the first path);
- a third world runs the same first 100 steps and saves its state, so that its bytes compare with the second world's;
- the second world is restored in place (`PhysicsSystem::RestoreState`) and steps 200 times (the second path);
- a new world is built from the same scene description in the same creation order, restored from the bytes and stepped 200 times on a job system of the same thread count (the third path), and another one on a job system of the other thread count (the fourth path).

Every path records whether the restore succeeded, whether the hash right after the restore equals the hash at the save instant, the first step at which it diverges from the reference (or none), its final hash, the update errors the library reported and the time to restore. `--self-check` verifies the driver's own logic (hash vectors; the job system running jobs with dependencies through a barrier at one, two and four threads, every job exactly once; two identical runs; a state restored in place and into a rebuilt world giving back the save instant). `ci/run-spike.sh jolt-snapshot-continuation <cell>` builds it with the platform's default C++ compiler and saves `facts.json`, `environment.txt` and the CMake configure output; `--quick` runs a tenth of the steps.

Relationship to the external component: Jolt Physics v5.6.0 (MIT) is the subject of the measurement; nothing of it is vendored or reused beyond this spike.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=jolt-snapshot-continuation`. Release build with the default C++ compiler of each image, Jolt's own flags for the library (precise floating point, contraction off, AVX2 with F16C, LZCNT and TZCNT on x86_64, NEON on ARM64), `-ffp-contract=off` or `/fp:precise` for the spike's code. Plus a local run on the founder's machine (AMD Ryzen 9 9950X3D, 16 cores and 32 logical processors) outside the matrix, under WSL Ubuntu 24.04 with GCC 13.3 and CMake 3.28: the machine's Windows application control policy (Smart App Control) refused to run the rebuilt MSVC executables of this spike after the first one, so the Windows figures of this machine stop at the first build's quick run, and the Windows x86_64 cell carries the MSVC measurement.

## Result

Local run of 2026-11-09 on the founder's machine under WSL Ubuntu 24.04 (GCC 13.3, the library's configuration string "Single precision x86 64-bit with instructions: SSE2 SSE4.1 SSE4.2 AVX AVX2 F16C LZCNT TZCNT (Cross Platform Deterministic)"): the 24 paths (3 scenes, 2 thread counts, 4 paths) all restore, give back the hash of the save instant and reproduce the uninterrupted run at every one of the 200 steps that follow, with no update error; the third world's bytes equal the second world's in the 6 runs; the uninterrupted final hashes are the same at 1 and 4 threads for every scene. Hypotheses 1, 2 and 4 hold on this machine; hypothesis 3 holds across thread counts here and awaits the cells. The first MSVC build's quick run on Windows (a tenth of the steps) gave the same outcome: 24 of 24 paths identical, bytes identical.

| Scene | Bodies | Constraints | State bytes | Bytes per body | Save ms | Restore ms | Step ms total at 1 and 4 threads (300 steps) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `pyramid` | 1 241 | 0 | 1 697 334 | 1 368 | 1.7 to 3.5 | 1.7 to 2.0 | 2 154, 878 |
| `convex_vs_mesh` | 1 765 | 0 | 338 116 | 192 | 0.3 | 0.4 to 0.5 | 663, 264 |
| `point_constraint_grid` | 900 | 1 740 | 147 183 | 164 | 0.1 | 0.15 to 0.2 | 142, 145 |

Reading: the state is the library's dynamic state (positions, velocities, contacts, constraint impulses), not a copy of the world, so its size follows the contacts (the pyramid's many resting contacts weigh most) rather than the body count, and a world built from the same description then restored continues identically, which makes the save state usable for rollback as long as the world's description is rebuilt in the same order. The first MSVC build's `--self-check` on Windows stopped in the job system's own check with the free list exhausted ("no job slot left"), an outcome not reproduced by the GCC build, whose self-check passes at one, two and four threads; the cause was not identified because the application control policy refused every rebuilt MSVC executable, and the measurement path shows no exhaustion on either compiler (the quick run on Windows and the full run under WSL completed with the same 2 048-slot pool).

Research CI [run](https://github.com/ludenea/ludenea/actions/runs/42663672200) of 2026-11-09 on the five cells:

| Cell | Machine | Compiler | Instruction set | Paths identical | Second saves identical | `pyramid` final hash (300 steps) | `convex_vs_mesh` final hash | `point_constraint_grid` final hash |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `linux-x86_64` | AMD EPYC 7763, 4 vCPU | GCC 13.3 | AVX2 | 24 of 24 | 6 of 6 | `0x7df8a2e3b64262c1` | `0xe855fe5eaad06161` | `0xf4e1adb7401b1209` |
| `linux-arm64` | Neoverse N2, 4 vCPU | GCC 13.3 | NEON | 24 of 24 | 6 of 6 | same | same | same |
| `windows-x86_64` | AMD EPYC 9V74, 4 vCPU | MSVC 19.51 | AVX2 | 24 of 24 | 6 of 6 | same | same | same |
| `windows-arm64` | Cobalt 100, 4 vCPU | MSVC 19.51 | NEON | 24 of 24 | 6 of 6 | `0x4edaaf1853a3ef69` | `0xb5f9dd33653b2981` | same |
| `macos-arm64` | Apple M1 (virtual), 3 vCPU | Apple Clang 17 | NEON | 24 of 24 | 6 of 6 | same as Linux | same as Linux | same |
| local, outside the matrix | AMD Ryzen 9 9950X3D under WSL | GCC 13.3 | AVX2 | 24 of 24 | 6 of 6 | same as Linux | same as Linux | same |

- Hypotheses 1, 2 and 4 hold on every cell: every path restores, gives back the save instant and reproduces its own cell's uninterrupted run at every step, at one and four threads, in place and in a world rebuilt in the same creation order, and two saves after the same steps are byte-identical; no update error was reported.
- Hypothesis 3 holds on four cells and is refuted on one: the final hashes and the state sizes (1 697 334, 338 116 and 147 183 bytes) are identical across Linux x86_64 and ARM64 with GCC, Windows x86_64 with MSVC and macOS ARM64 with Apple Clang, so the library's cross-platform determinism option holds across two instruction sets and three compilers there; the Windows ARM64 cell, MSVC 19.51 compiling for ARM64 with NEON, gives other hashes and other state sizes (1 699 678 and 338 052 bytes) on the two scenes with contacts (`pyramid` and `convex_vs_mesh`) and the same hash on the constraint grid without contacts, while its own continuation is identical. MSVC for ARM64 is absent from the configurations the library's documentation lists as verified for cross-platform determinism; MSVC's documentation states that `/fp:precise` generates no contraction since Visual Studio 2022, so the cause is not the documented contraction default and was not identified by this spike. A build of the same cell with another compiler (Clang, which the library's own cross-platform deterministic configurations use on Windows) is the measurement that would separate the compiler from the platform.
- Step times, total milliseconds for 300 steps at 1 and 4 threads: `pyramid` 4 237 and 1 852 on Linux x86_64, 4 108 and 1 680 on Linux ARM64, 3 328 and 1 533 on Windows x86_64, 5 238 and 2 125 on Windows ARM64, 4 071 and 6 232 on the macOS runner (which does not scale, as every multithreaded spike found), 2 154 and 878 on the local machine; `convex_vs_mesh` 1 178 and 445, 904 and 366, 897 and 346, 1 004 and 417, 1 060 and 1 169, 663 and 264. Saving the pyramid's state costs 3 to 12 ms on the runners and restoring it 3 to 5 ms (one outlier of 21 ms on the macOS runner).

The raw measurements, environments and configure outputs of each cell are the artifacts of the run.

The raw measurements, environments and configure outputs of each cell are the artifacts of the run. The result holds for Jolt v5.6.0 built with these compilers on these machines.
