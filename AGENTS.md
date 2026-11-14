# Agent instructions for Ludenea

Ludenea is an original 2D and 3D game engine, primarily in Rust, at its foundations stage. This public repository receives the engine's code and its released documentation; the project is steered from a private repository. Until the founder releases something here, there is no engine code in this repository, and no engine code, scaffolding or dependency is added without the founder's explicit instruction. The only code is the throwaway research spikes under `spikes/` and the throwaway prototypes under `prototypes/`, governed by the rules below.

## Conventions

- All files are in English. Markdown: ATX headings, one top-level heading per file, `-` list markers, fenced code blocks with a language tag, prose not hard-wrapped. No em-dashes.
- Plain words, no codes. Nothing in this repository is designated by an identifier of the steering documentation: no track numbers, no requirement, question or decision keys, no phase or wave labels. A spike, a directory, a workflow or a README names what it measures and the research subject it serves in words ("the cross-platform compatibility research"). A reader of this repository must understand everything without the steering documentation.
- Commit messages: gitmoji plus Conventional Commits (`<emoji> <type>(<scope>): <summary>`), one logical change per commit, in English, without any reference to internal working labels or codes and without AI attribution trailers.
- Licensing: MIT or Apache 2.0 at the licensee's option; contributions are accepted under the same terms.

## Spikes

`spikes/` holds the throwaway experiments of the research program and the research CI that runs them (`.github/workflows/research-ci.yml`). Read [`spikes/README.md`](spikes/README.md) first.

- A spike answers one engineering question with one measurement. It lives in `spikes/<slug>/`, named after what it measures, with a README stating in plain words the research subject it serves, its hypothesis, its method, its environment and its result.
- Spikes are evidence, never product code and never a direction of the engine. Nothing moves from a spike into engine code: that path goes through the architecture and implementation planning held in the steering repository.
- Do not refactor spikes into shared or reusable code. The only shared parts are the Cargo workspace, the pinned toolchain and the reference workload (`spikes/reference-workload/`).
- A spike adds a third-party dependency only when that dependency is the subject of the measurement; its README names the version and the license. No copyleft code is vendored.
- Measurements are published as CI artifacts. Their interpretation is not written in this repository.
- The research CI stays thin: the workflow calls `spikes/ci/run-spike.sh`, which must keep running unchanged on Linux, macOS and Windows (Git Bash) and must not depend on GitHub Actions features.
- Spikes build warning-free and pass `cargo test` at the pinned toolchain. A spike whose logic is not trivial carries one runnable self-check.

## Prototypes

`prototypes/` holds the throwaway prototypes of the research program. Read [`prototypes/README.md`](prototypes/README.md) first.

- A prototype is larger than a spike: it takes several claims of one design at once, to check how its mechanisms meet. It lives in `prototypes/<slug>/`, named after what it builds, as a Cargo workspace of its own, with a README stating in plain words what it measures, its hypothesis, its method, its environment and its result.
- A prototype is throwaway: never product code, never a first version of the engine, never a direction it takes. Nothing moves from a prototype into engine code: that path goes through the architecture and implementation planning held in the steering repository. A prototype is built only on the founder's instruction.
- A prototype shares the pinned toolchain of `spikes/`, the reference workload and the research CI, and nothing else. It may stand on the code of earlier spikes by copying it, and says so in its README; the spikes stay as measured.
- The rules of the spikes on dependencies, licensing, plain words and published measurements hold for a prototype too. It builds warning-free and passes `cargo test` at the pinned toolchain, run from `spikes/` with `--manifest-path`.
- The research CI runs a prototype by its name through `spikes/ci/run-spike.sh`, as it runs a spike.
