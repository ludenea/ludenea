# Spikes

Throwaway experiments of the Ludenea research program. A spike answers one engineering question with one measurement. Spikes are evidence: they are never product code, never a prototype of the engine and never a direction the engine takes. Nothing moves from a spike into the engine without going through the architecture and implementation planning held in the project's steering documentation.

## Layout

- `<slug>/`: one spike, named after what it measures. Each spike has a README stating the research subject it serves, its hypothesis, its method, its environment and its result.
- `reference-workload/`: the specification of the synthetic simulation that the spikes of several research subjects measure against, so that their numbers are comparable, with its sequential reference implementation, the one crate other spikes build on, and the reference hashes measured on the cells.
- `ci/run-spike.sh`: builds one spike and runs it. The research CI calls it; it runs the same way on a developer machine. `ci/summarize.sh` prints the Markdown summary of a run's measurements across cells.
- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`: the Cargo workspace of the Rust spikes and its pinned toolchain. A spike that studies a C library is a C program with its own `CMakeLists.txt`, built with the platform's default C compiler, since the library under study is compiled that way too. Spikes share nothing else.

## Research CI

The workflow [`research-ci.yml`](../.github/workflows/research-ci.yml) builds and runs the spikes on standard GitHub-hosted runners for every platform and architecture of the engine's first compatibility boundary: Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only. Measurements (fact sheets, state hashes, timings) are published as workflow artifacts, one per cell plus a merged one, and a summary table appears on the run page. The interpretation of the measurements happens in the steering documentation, not here.

| Cell             | Runner label       | Image                     |
| ---------------- | ------------------ | ------------------------- |
| `linux-x86_64`   | `ubuntu-24.04`     | Ubuntu 24.04, x86_64      |
| `linux-arm64`    | `ubuntu-24.04-arm` | Ubuntu 24.04, ARM64       |
| `windows-x86_64` | `windows-2025`     | Windows Server 2025, x86_64 |
| `windows-arm64`  | `windows-11-arm`   | Windows 11, ARM64         |
| `macos-arm64`    | `macos-15`         | macOS 15, Apple silicon   |

The workflow runs on pushes and pull requests that touch `spikes/` (Markdown files excepted: a README edit is not a measurement) or the workflow, and on demand. Each cell is independent: one failing cell never cancels the others, because every cell is a measurement, and a later run never cancels an earlier one for the same reason.

On demand (`workflow_dispatch`), a run can pick another spike, pass extra `RUSTFLAGS` to a Rust spike, build a Rust spike with another Cargo profile (`dev` for an unoptimized build with debug assertions), and run on another set of cells given as a JSON list. Three uses of the cross-platform compatibility and determinism research:

```bash
# Rebuild the platform facts with a raised x86-64 instruction set baseline on the two x86_64 cells.
gh workflow run research-ci.yml -f rustflags='-C target-cpu=x86-64-v3' \
  -f cells='[{"cell":"linux-x86_64","runner":"ubuntu-24.04"},{"cell":"windows-x86_64","runner":"windows-2025"}]'

# Run on the oldest runner images that match the engine's proposed operating system baseline.
gh workflow run research-ci.yml \
  -f cells='[{"cell":"linux-x86_64-ubuntu-22.04","runner":"ubuntu-22.04"},{"cell":"linux-arm64-ubuntu-22.04","runner":"ubuntu-22.04-arm"},{"cell":"macos-arm64-macos-14","runner":"macos-14"}]'

# Check that an unoptimized build of the reference workload gives the same hashes as the release build.
gh workflow run research-ci.yml -f spike=reference-workload -f profile=dev \
  -f cells='[{"cell":"linux-x86_64-dev","runner":"ubuntu-24.04"}]'
```

A Linux cell may also name a container image (`container`): the job then runs inside it, which is how the headless execution research runs a spike in a minimal image without display, GPU or audio libraries:

```bash
# Run the headless modes spike inside a minimal container on the two Linux cells.
gh workflow run research-ci.yml -f spike=headless-modes \
  -f cells='[{"cell":"linux-x86_64-container","runner":"ubuntu-24.04","container":"rust:1.99.1-slim-bookworm"},{"cell":"linux-arm64-container","runner":"ubuntu-24.04-arm","container":"rust:1.99.1-slim-bookworm"}]'
```

The cell label is free text that names the artifact and the output directory; the environment file records the machine and the image the run landed on, and, for a cell inside a container, the image as named with the digests of its manifest index and of its per-platform manifests, resolved at the start of the run so that the environment is identified by digest and not by a tag that moves.

## Running locally

```bash
cd spikes
ci/run-spike.sh platform-facts my-machine
```

The script installs the pinned toolchain if needed (Rust spikes), builds the spike in release mode (or in the Cargo profile named by `SPIKE_PROFILE`), runs it, and writes `out/my-machine/facts.json` and `out/my-machine/environment.txt` (plus `configure.txt` for a C spike). `cargo test` runs the Rust spikes' self-checks; a C spike runs its self-check when invoked with `--self-check`. `ci/summarize.sh <spike> out` prints the summary table.

## Rules

- A spike lives in its own directory, stays small (a few files), and states its hypothesis before it runs.
- A spike uses a third-party library only when that library is the subject of the measurement, for example a physics library under study; the spike's README names the library, its version and its license. Copyleft code is never vendored.
- Spikes are licensed like the rest of this repository: MIT or Apache 2.0, at your option.
- A spike's conclusion lives in its README and is updated when a measurement changes it; raw measurements live in the CI artifacts.
- Spikes are not maintained as software. A spike that has served its purpose is left as is or deleted, never refactored into reusable code. The only shared parts are the workspace, the toolchain and the reference workload.
- Spikes, directories and READMEs name things in plain words; they carry no identifier of the steering documentation.

## Index

| Spike                                   | Research subject               | Question                                                                                                 | Status                                 |
| --------------------------------------- | ------------------------------ | -------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| [`platform-facts`](platform-facts/)     | Cross-platform compatibility   | Which floating-point results are bit-identical across the five cells of the boundary, and which differ? | Measured on 2026-11-02, see its README |
| [`platform-threading-constraints`](platform-threading-constraints/) | Cross-platform compatibility | Which windowing, event loop and graphics device operations does each platform of the boundary bind to the main thread, which does it allow from any thread, and which are unavailable on a machine without a display? | Measured on 2026-11-02, see its README |
| [`box2d-worker-sweep`](box2d-worker-sweep/) | Physics (the Box2D v3 study) | Does Box2D v3.2.0, driven through its task interface by an external scheduler, give identical state at 1, 2, 4 and 8 workers and across the five cells? | Measured on 2026-11-02, see its README |
| [`box2d-snapshot-continuation`](box2d-snapshot-continuation/) | Physics (the Box2D v3 study) | Does a Box2D v3.2.0 world restored from one of its snapshots, in place or as a new world at the same or another worker count, step exactly like the uninterrupted world, on the five cells? | Measured on 2026-11-02, see its README |
| [`reference-workload`](reference-workload/) | Deterministic simulation (the fixture of several research subjects) | Does the sequential reference implementation of the reference workload give identical hashes on the five cells, with and without the platform math library, in release and unoptimized builds? | Measured on 2026-11-02, see `reference-hashes.md` |
| [`workload-worker-sweep`](workload-worker-sweep/) | Deterministic simulation | Do deterministic-by-construction parallel constructs give the sequential hashes of the reference workload at 1, 2, 4 and 8 workers on the five cells, and what do they cost against a free-running reduction? | Measured on 2026-11-02, see its README |
| [`workload-scheduling-overhead`](workload-scheduling-overhead/) | Multithreading | Does a dependency-counting task graph that overlaps a tick's hash with the next tick's parallel work keep the sequential hashes of the reference workload at every worker count on the five cells, and what do the barrier, the copy back, the chunk size, work stealing and the waiting policy cost? | Measured on 2026-11-02, see its README |
| [`workload-storage-layouts`](workload-storage-layouts/) | ECS | Do identifier-indexed slots, chunked archetype tables with a slot table and sparse sets reproduce the sequential hashes of the reference workload at every worker count on the five cells, and what do they cost one another in iteration, canonical traversals, a second archetype and change detection? | Measured on 2026-11-02, see its README |
| [`workload-structural-changes`](workload-structural-changes/) | ECS | Do command buffers recorded by parallel chunks and applied at a fixed point keep the reference workload's results independent of the worker count on three storage layouts, and what do spawns, despawns, component changes, the identifier paths and the free list policies cost at 8, 80 and 800 changes per tick? | Measured on 2026-11-02, see its README |
| [`runtime-skeleton`](runtime-skeleton/) | Runtime architecture | Does a minimal three-layer crate set build with and without a presentation capability behind the facade's feature flag, with no dependency on the capability crate when absent, what does that cost in build time, and does a time control API that steps, snapshots and restores a world reproduce the reference hashes, with two worlds in one process and a failure confined to one? | Measured on 2026-11-02, see its README |
| [`workload-frame-allocator`](workload-frame-allocator/) | Memory | Do the reference workload's hashes stay independent of allocation under four allocation modes, what does the general heap cost under parallel passes on the three system allocators, does a per-worker scratch remove every steady-state heap allocation, what does a counting wrapper cost, is table growth invisible to the results, and does per-tag accounting return to zero at destruction? | Measured on 2026-11-02, see its README |
| [`workload-schema`](workload-schema/) | ECS | Does a schema served as data describe the reference workload's components completely to a client that knows nothing of the storage, so that the client recomputes every tick hash from its view, and does a field added with a default leave an old client reading correctly while a new client reads the field? | Measured on 2026-11-03, see its README |
| [`control-surface-toy`](control-surface-toy/) | Control surface | Does a scripted agent that discovers the contract through introspection step the reference workload, kick, query and observe through a stepped session with the oracle's hashes, in process and over local sockets, does the input log replay, does a session survive a disconnection with every event once and a retried command applied once, is a field added with a default a compatible change, and what does the surface cost? | Measured on 2026-11-03, see its README |
| [`workload-history-capture`](workload-history-capture/) | Control surface | Does a world restored from any snapshot of a cadence and replayed from the per-tick input log reach the live run's hashes and events at every worker count, what do the input log, the hash, the event log and the snapshots cost per tick, how long does a causal query on one entity take by instrumented replay beside a live world, and is a tick outside retention a structured error? | Measured on 2026-11-03, see its README |
| [`headless-modes`](headless-modes/) | Headless execution | Do the headless and harness hosts run in a minimal Linux container with the reference hashes, does the binary built without the presentation capability import any windowing, graphics or audio library on the five cells, does it run from a non-interactive context, how many worlds does one machine step at once, and does a capability not built, discovered absent or requested absent describe the host identically? | Measured on 2026-11-03, see its README |
| [`workload-test-harness`](workload-test-harness/) | Game testing | Does the reference workload run as tests of the standard harness, each test two isolated worlds of one process from one fixture at two worker counts, report no divergence when there is none and the first divergent tick and system when a worker-count-dependent reduction is injected, do sixteen such tests in parallel give the hashes each gives alone and at what wall time, what does the fixture's restore cost, does a test whose world panics fail alone with its report, and do the same tests pass on the build without and with the presentation capability? | Measured on 2026-11-03, see its README |
| [`workload-lockstep`](workload-lockstep/) | Networking and multiplayer | Do two reference workload worlds in deterministic lockstep over a simulated link with latency, jitter and loss keep identical per-tick hashes at every worker count for input delays of 2 and 4 ticks and a loss rate up to 10 percent, is a desync injected in one peer detected by the hash exchange at its tick and named by its first divergent system, how many frames does a peer stall per link setting, and what does a rollback variant cost per rollback? | Measured on 2026-11-03, see its README |
| [`workload-render-extraction`](workload-render-extraction/) | Rendering | Does a chunked extraction of positions and sprite handles into a double-buffered render state after every tick of the reference workload leave the simulation's hashes unchanged at every worker count, give one render state whatever the worker count, what does it cost per tick, and what does a per-chunk change detection skip save for chunks no system wrote? | Measured on 2026-11-08, see its README |
| [`plugin-tiers`](plugin-tiers/) | Plugin architecture | Does the reference workload's motion pass reached through each candidate plugin tier (a crate linked at compile time, a WebAssembly module under Wasmtime, a dynamic library with a C ABI built in Rust and in C by the cell's compiler with its default and with the permitted floating-point flags) give the sequential hashes at every worker count on the five cells, what does each tier cost per chunk against the kernel inlined in the world, does a reload between two ticks change a hash, and what does an out-of-process importer's round trip cost? | Measured on 2026-11-08, see its README |
| [`workload-asset-arrival`](workload-asset-arrival/) | Assets and content pipelines | Does the reference workload, depending on a domain data asset that a loader thread delivers after a real delay, give the reference hashes at every worker count behind a loading gate, does a world that reads a declared default until the arrival tick replay from its recorded input log at every worker count and agree with a sequential oracle switched at that tick, do runs diverge once the fence is removed, is a hot reload at a scripted tick deterministic, and what does the fence cost? | Measured on 2026-11-08, see its README |
| [`render-offscreen`](render-offscreen/) | Rendering | Does a fixed scene from the reference workload render offscreen, with no window and no display server, through a software Vulkan device on the Linux cells, the WARP device on Windows and the Metal device on macOS, read back as byte-identical images across two runs of one cell, and how do the images compare across cells and APIs? | Measured on 2026-11-08, see its README |
| [`workload-persistent-pool`](workload-persistent-pool/) | Fundamental architecture (the concurrency model) | Does the harness host, stepping the reference workload one tick per call on a worker pool owned by the runtime instance and persistent across steps, give the reference hashes at every worker count, alone, with two worlds alternating and with sixteen worlds stepped concurrently from sixteen threads on one pool, what does a one-tick step cost against the step-scoped pool's spawn, what do a step after a pause and the idle pool cost, and does a failed world leave the shared pool serving the others? | Measured on 2026-11-09, see its README |
| [`asset-import-parallel`](asset-import-parallel/) | Assets and content pipelines (the background pool of the multithreading research) | Do a few hundred synthetic source assets of a plain and a geometric kind, imported in parallel on a background pool with dynamic claiming into canonical content-hashed bytes, give byte-identical outputs across runs, worker counts and the five cells, does a re-import after one source changes import that asset and its dependents only, and how does the import's throughput scale with the workers? | Measured on 2026-11-09, see its README |
| [`workload-scene-streaming`](workload-scene-streaming/) | Scenes and levels | Do the reference workload's entities partitioned into a grid of scene cells, loaded and unloaded by a scripted streaming policy from a moving point of interest with references across neighbouring cells resolved through a per-instance remap table, give one history at 1, 4 and 8 workers equal to a sequential oracle's and replayed from the recorded input log, does every reference to an unloaded cell read the deterministic "not present" value and never a stale slot, do the remap table's size and the cost of a load and an unload follow the loaded cells rather than the world's size, what does a leaving distance larger than the entering distance save in loads, what does the sliced spawn cost at a fast traversal, and do two worlds on one shared payload store each reproduce their oracle? | Measured locally on 2026-11-09, see its README |
