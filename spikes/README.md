# Spikes

Throwaway experiments of the Ludenea research program. A spike answers one engineering question with one measurement. Spikes are evidence: they are never product code, never a prototype of the engine and never a direction the engine takes. Nothing moves from a spike into the engine without going through the architecture and implementation planning held in the project's steering documentation.

## Layout

- `<slug>/`: one spike, named after what it measures. Each spike has a README stating the research subject it serves, its hypothesis, its method, its environment and its result.
- `reference-workload/`: the specification of the synthetic simulation that the spikes of several research subjects measure against, so that their numbers are comparable.
- `ci/run-spike.sh`: builds one spike and runs it. The research CI calls it; it runs the same way on a developer machine.
- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`: the spikes' Cargo workspace and its pinned toolchain. Spikes share nothing else.

## Research CI

The workflow [`research-ci.yml`](../.github/workflows/research-ci.yml) builds and runs the spikes on standard GitHub-hosted runners for every platform and architecture of the engine's first compatibility boundary: Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only. Measurements (fact sheets, state hashes, timings) are published as workflow artifacts, one per cell plus a merged one, and a summary table appears on the run page. The interpretation of the measurements happens in the steering documentation, not here.

| Cell             | Runner label       | Image                     |
| ---------------- | ------------------ | ------------------------- |
| `linux-x86_64`   | `ubuntu-24.04`     | Ubuntu 24.04, x86_64      |
| `linux-arm64`    | `ubuntu-24.04-arm` | Ubuntu 24.04, ARM64       |
| `windows-x86_64` | `windows-2025`     | Windows Server 2025, x86_64 |
| `windows-arm64`  | `windows-11-arm`   | Windows 11, ARM64         |
| `macos-arm64`    | `macos-15`         | macOS 15, Apple silicon   |

The workflow runs on pushes and pull requests that touch `spikes/` or the workflow, and on demand. Each cell is independent: one failing cell never cancels the others, because every cell is a measurement.

## Running locally

```bash
cd spikes
ci/run-spike.sh platform-facts my-machine
```

The script installs the pinned toolchain if needed, builds the spike in release mode, runs it, and writes `out/my-machine/facts.json` and `out/my-machine/environment.txt`. `cargo test` runs the spikes' self-checks.

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
