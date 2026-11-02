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

The cell label is free text that names the artifact and the output directory; the environment file records the machine and the image the run landed on.

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
| [`box2d-worker-sweep`](box2d-worker-sweep/) | Physics (the Box2D v3 study) | Does Box2D v3.2.0, driven through its task interface by an external scheduler, give identical state at 1, 2, 4 and 8 workers and across the five cells? | Measured on 2026-11-02, see its README |
| [`box2d-snapshot-continuation`](box2d-snapshot-continuation/) | Physics (the Box2D v3 study) | Does a Box2D v3.2.0 world restored from one of its snapshots, in place or as a new world at the same or another worker count, step exactly like the uninterrupted world, on the five cells? | Measured on 2026-11-02, see its README |
| [`reference-workload`](reference-workload/) | Deterministic simulation (the fixture of several research subjects) | Does the sequential reference implementation of the reference workload give identical hashes on the five cells, with and without the platform math library, in release and unoptimized builds? | Measured on 2026-11-02, see `reference-hashes.md` |
| [`workload-worker-sweep`](workload-worker-sweep/) | Deterministic simulation | Do deterministic-by-construction parallel constructs give the sequential hashes of the reference workload at 1, 2, 4 and 8 workers on the five cells, and what do they cost against a free-running reduction? | Measured on 2026-11-02, see its README |
| [`workload-scheduling-overhead`](workload-scheduling-overhead/) | Multithreading | Does a dependency-counting task graph that overlaps a tick's hash with the next tick's parallel work keep the sequential hashes of the reference workload at every worker count on the five cells, and what do the barrier, the copy back, the chunk size, work stealing and the waiting policy cost? | Built on 2026-11-02, run pending |
| [`workload-storage-layouts`](workload-storage-layouts/) | ECS | Do identifier-indexed slots, chunked archetype tables with a slot table and sparse sets reproduce the sequential hashes of the reference workload at every worker count on the five cells, and what do they cost one another in iteration, canonical traversals, a second archetype and change detection? | Built on 2026-11-02, run pending |
| [`workload-structural-changes`](workload-structural-changes/) | ECS | Do command buffers recorded by parallel chunks and applied at a fixed point keep the reference workload's results independent of the worker count on three storage layouts, and what do spawns, despawns, component changes, the identifier paths and the free list policies cost at 8, 80 and 800 changes per tick? | Built on 2026-11-02, run pending |
