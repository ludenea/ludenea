# Box2D worker sweep

Throwaway spike of the physics research, whose first stage studies Box2D v3 as a study object, not as an integration choice. It measures, instead of assuming, what Box2D's documentation claims about determinism under multithreading and across platforms, and it exercises the task interface through which an engine-owned scheduler would drive the library.

## Hypothesis

With Box2D v3.2.0 built as its author prescribes (contraction into fused multiply-add disabled, no fast math):

1. Driving the library through its task callbacks from an external scheduler yields bit-identical body transforms after every step at 1, 2, 4 and 8 workers, and identical to the transforms produced by the library's own task system at the same worker counts.
2. The transforms are identical across the five cells of the desktop boundary (Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only), the AVX2 path and the SSE2 fallback of x86_64 included: the cross-platform determinism the library claims since version 3.1 holds for its own benchmark scenes.
3. The author's own determinism scenario, the falling hinges, reproduces the hash and the sleep step the upstream test suite expects, on every cell.

Informational, not a hypothesis: how step time scales with the worker count on the research CI runners (4 virtual CPUs, 3 on macOS), compared with the numbers the author publishes in the repository's `benchmark/` directory.

## Method

A C17 driver fetches Box2D at tag `v3.2.0` and enkiTS at tag `v1.12` through CMake, and compiles the library's own shared scenes (`shared/benchmarks.c`, `shared/determinism.c`) from the fetched tree. It runs seven of the author's benchmark scenes (`joint_grid`, `large_pyramid`, `many_pyramids`, `rain`, `smash`, `spinner`, `tumbler`) with the step counts of the author's benchmark program, at 1/60 s with 4 substeps, for every combination of:

- scheduler: external (enkiTS behind `b2WorldDef.enqueueTask` and `finishTask`; Box2D v3.2 asks the task system to run each callback exactly once on a worker thread and distributes the work itself, so a task set of size one carries each task) and native (no callbacks: the library's own task system at more than one worker, its serial path at one);
- workers: 1, 2, 4, 8 (`b2WorldDef.workerCount`; enkiTS counts the calling thread as one of them, as Box2D does);
- SIMD width: the detected one, plus the SSE2 fallback (`b2World_EnableSSE2Fallback`) on machines where AVX2 is dispatched.

After every step the driver enumerates every shape with a broadphase query covering the scene, reduces them to bodies, sorts the bodies by their stored id (the creation slot comes first in it) and hashes position and rotation field by field with FNV-1a 64, so that the hash depends on neither the query's traversal order nor struct padding. It records the final hash, a chain hash of all step hashes, the first step at which the run diverges from the reference run (external scheduler, 1 worker, same scene and width), the body count of the world next to the number of bodies hashed (a body without a shape is not reached), the number of tasks Box2D enqueued, and the step times (total, median, maximum; the hashing happens outside the timed region). The falling hinges scenario runs with the same combinations until the bodies sleep, and its hash and sleep step are compared with the constants of the upstream test suite.

The driver prints one JSON document; `ci/run-spike.sh box2d-worker-sweep <cell>` builds it with the platform's default C compiler, runs it and saves `facts.json`, `environment.txt` and the CMake configure output. `--self-check` verifies the driver's own logic (hash vectors, two identical serial runs, the external scheduler running the enqueued tasks, the hinges completing) and `--quick` runs a tenth of the steps.

Relationship to the external components: Box2D v3.2.0 (MIT) and enkiTS v1.12 (zlib) are the subjects of the measurement; nothing of them is vendored or reused beyond this spike.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)), run on demand with `-f spike=box2d-worker-sweep`. Release build with the default C compiler of each image (GCC on Linux, Apple Clang on macOS, MSVC on Windows), `-ffp-contract=off` on GCC and Clang for every target of the build, Box2D's own CMake flags otherwise (its MSVC build compiles the eight-wide contact solver with `/arch:AVX2` and dispatches to it at run time). Asserts are off, as in a release build, so that step times compare with the author's numbers.

## Result

Local run on 2026-11-02, Apple M5 (10 cores, macOS, Apple Clang 21, NEON path, no AVX2 so one SIMD width), full step counts: the seven scenes give one final hash each across the 8 combinations of scheduler and worker count, no run diverges from its reference at any step, and the falling hinges give the upstream hash `0x465f3df4` at sleep step 299 in every combination. Step time at 1, 2, 4 and 8 workers (external scheduler, total milliseconds): `large_pyramid` 492, 272, 170, 254; `joint_grid` 1067, 581, 384, 379; `rain` 2987, 1777, 1311, 1343; 8 workers is slower than 4 on this machine, which mixes performance and efficiency cores. The research CI result on the five cells is recorded below when the run completes.
