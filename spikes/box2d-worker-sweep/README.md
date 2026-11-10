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

Research CI [run](https://github.com/ludenea/ludenea/actions/runs/41434748466) of 2026-11-02 on the five cells, plus a local run the same day on an Apple M5 (10 cores, macOS, Apple Clang 21) outside the matrix:

| Cell | Machine | Compiler | SIMD widths | Final hashes per scene across 8 or 16 combinations | Falling hinges |
| --- | --- | --- | --- | --- | --- |
| `linux-x86_64` | AMD EPYC 7763, 4 vCPU | GCC 13.3 | 8 (AVX2 dispatched) and 4 (SSE2 fallback) | one per scene | `0x465f3df4` at step 299, every combination |
| `linux-arm64` | Neoverse N2, 4 vCPU | GCC 13.3 | 4 (NEON) | one per scene | same |
| `windows-x86_64` | AMD EPYC 9V74, 4 vCPU | MSVC 19.51 | 8 and 4 | one per scene | same |
| `windows-arm64` | Cobalt 100, 4 vCPU | MSVC 19.51 | 4 (NEON) | one per scene | same |
| `macos-arm64` | Apple M1 (virtual), 3 vCPU | Apple Clang 17 | 4 (NEON) | one per scene | same |
| local | Apple M5, 10 cores | Apple Clang 21 | 4 (NEON) | one per scene | same |

- Hypothesis 1 holds: every scene has one final hash across the external and native schedulers at 1, 2, 4 and 8 workers, and no run diverges from its single-worker reference at any step.
- Hypothesis 2 holds: the hash of each scene is the same on the six machines, across the three compilers, between the AVX2 path and the SSE2 fallback, and between x86_64 and ARM64: `joint_grid` `0x9c1e03dad6409a09`, `large_pyramid` `0xc3af4b9fd4b96e77`, `many_pyramids` `0xb1c91fd6de0df6d9`, `rain` `0x07e3c76d31f4bc47`, `smash` `0x752022fc74c9c592`, `spinner` `0xc43a20d0bfd4107d`, `tumbler` `0x67b938db701c26ce`. Every body was hashed except one body without a shape in `tumbler` (2026 of 2027), identically everywhere.
- Hypothesis 3 holds: the falling hinges reproduce the upstream hash and sleep step on every cell in every combination.
- Step times, total milliseconds with the external scheduler at 1, 2, 4 and 8 workers (the native scheduler is within a few percent): `large_pyramid` 1075, 600, 528, 655 on Linux x86_64 with AVX2 (1269, 730, 622, 761 with SSE2), 1138, 684, 606, 753 on Windows x86_64, 1414, 882, 527, 628 on Linux ARM64, 1508, 896, 535, 780 on Windows ARM64, 1321, 1199, 1035, 1670 on the macOS runner, 492, 272, 170, 254 on the local M5; `joint_grid` 3037, 1609, 1381, 1476 on Linux x86_64, 3440, 1870, 1040, 1174 on Linux ARM64, 3179, 4921, 4820, 5934 on the macOS runner. The ARM64 runners scale by about 3 from 1 to 4 workers, the x86_64 runners by about 2; 8 workers on 4 virtual CPUs cost 10 to 25 percent over 4 and never deadlock; the hosted macOS runner does not scale at all, unlike the local M5, which says something about that runner and nothing about the library. Box2D enqueued between 3 and 6 tasks per step at 1 worker and up to 41 per step at 8 workers, below the 256 it promises as a ceiling.

The raw measurements, environments and configure outputs of each cell are the artifacts of the run. The result holds for Box2D v3.2.0 built with these compilers on these images.

### The Windows ARM64 cell built with Clang

Two research CI runs of 2026-11-10 on the Windows ARM64 cell (Cobalt 100, 4 vCPU, image `win11-vs2026-arm64`) with `clang-cl` (Clang 22.1.8, the image's LLVM release, Ninja), to learn whether a rule that builds foreign code with Clang on this cell, where MSVC is the default, keeps this library's results.

- **As released, Box2D v3.2.0 does not compile with `clang-cl` for ARM64** ([run](https://github.com/ludenea/ludenea/actions/runs/42768648439), `-f cxx=clang-cl`): `src/platform.h` reads the floating-point control register through `_ReadStatusReg( ARM64_FPCR )` whenever `_MSC_VER` is defined, which `clang-cl` defines, and the identifier is declared by the Windows SDK's headers, which that file does not include on the Clang path (`error: use of undeclared identifier 'ARM64_FPCR'`, in every translation unit that includes the header). Whether a later upstream revision compiles was not checked.
- **With the identifier supplied from the command line, it builds and every result is the reference** ([run](https://github.com/ludenea/ludenea/actions/runs/42769080324), `-f cxx='clang-cl /FIintrin.h /DARM64_FPCR=0x5A20'`, the flags recorded in the run's environment file; `0x5A20` is the register's encoding as the Windows SDK defines it, and the forced include declares the intrinsic). No source file of the library or of the spike is changed. The 56 runs (seven scenes, the external and native schedulers at 1, 2, 4 and 8 workers, NEON) give the seven final hashes of hypothesis 2 above, none diverges from its single-worker reference at any step, and the falling hinges reproduce the upstream hash `0x465f3df4` and sleep step 299 in the eight combinations. Step times with the external scheduler at 1, 2, 4 and 8 workers, total milliseconds: `joint_grid` 3313, 1829, 1032, 1162.

So Clang changes no result of Box2D v3.2.0 on this cell, where MSVC already gave the reference; what a Clang prescription costs for this library on Windows ARM64 is a two-flag workaround or a one-line patch of the library's header, to carry until upstream compiles there.
