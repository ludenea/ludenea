# Agent instructions

Instructions for every contributor to this repository, human or AI agent.
Ludenea is a deterministic game engine written in Rust: the simulation is a
headless library, and the editor, the command line, tests and agents are all
clients of it.

## Repository layout

| Path                       | Contents                                                       |
| -------------------------- | -------------------------------------------------------------- |
| `crates/math`              | `ludenea_math`, deterministic math (core)                      |
| `crates/sim`               | `ludenea_sim`, the deterministic simulation (core)             |
| `crates/ecs`               | `ludenea_ecs`, the entity component system (core)              |
| `crates/test`              | `ludenea_test`, harness running simulations inside `cargo test` |
| `xtask`                    | Repository automation, run with `cargo xtask`                  |
| `editor`                   | The future editor, licensed under the AGPLv3                   |
| `.github/workflows`        | Continuous integration                                         |
| `deny.toml`                | Dependency policy (licenses, advisories, bans)                 |

The toolchain is pinned in `rust-toolchain.toml`; never switch to nightly.
Shared dependency versions and lints live in the root `Cargo.toml` under
`[workspace.dependencies]` and `[workspace.lints]`; a crate inherits them
instead of declaring its own.

## Commands

| Command                     | What it does                                                         |
| --------------------------- | -------------------------------------------------------------------- |
| `cargo xtask ci`            | Everything CI runs: boundaries, format, clippy, nextest, doc, cargo deny |
| `cargo xtask boundaries`    | Checks the hexagonal boundary of the core crates                     |
| `cargo xtask doc`           | Builds the API documentation into `target/doc`, warnings denied      |
| `cargo xtask commit-format` | Checks subjects read from standard input, one per line               |
| `cargo xtask golden-hash`   | Prints the golden hash; add `--target wasm32-wasip1` for WebAssembly |
| `cargo xtask golden-compare <directory> <target>...` | Compares each `<target>.txt` with the reference, failing on a missing target |
| `cargo nextest run`         | Runs the tests                                                       |
| `cargo test --workspace --exclude xtask --target wasm32-wasip1` | Runs the core tests under `wasmtime` |
| `cargo insta review`        | Reviews changed snapshots                                            |

Run `cargo xtask ci` before considering a change done: CI runs the same
command and must be green before merge. It needs `cargo-nextest` and
`cargo-deny` installed; the WebAssembly tests also need `wasmtime`.

On Linux, builds link with [mold](https://github.com/rui314/mold), set in
`.cargo/config.toml`; install it from your distribution or its releases, with
GCC 12.1 or later for `-fuse-ld=mold`. Without it, run cargo with `RUSTFLAGS=`
set to an empty value to fall back to the default linker. That setting
replaces any `build.rustflags` of your own cargo configuration; repeat yours
in `target.'cfg(all(target_os = "linux", target_env = "gnu"))'.rustflags`.

## Development loop

Work test-first: write a failing test, make it pass with the simplest code,
then refactor while the tests stay green. A bug fix starts with a test that
reproduces it. Use snapshot tests (`insta`) for structured output and
property tests (`proptest`) for invariants.

Clippy runs with warnings denied and the pedantic group enabled. Silence a
lint only with `#[expect(lint, reason = "...")]`, never with `#[allow]`.

## Architecture rules

- **Hexagonal boundary.** Core crates (every workspace member except
  `xtask`) never depend on adapter crates, I/O, an async runtime or OS
  randomness, directly or transitively. `cargo xtask boundaries` enforces
  it; the rules live in `xtask/src/boundaries.rs`.
- **Determinism.** The same inputs produce the same simulation on every run
  and every platform. In the simulation crates (`math`, `sim`, `ecs`),
  `clippy.toml` forbids hash maps with random seeds, the wall clock,
  threads, platform floating-point functions and fused multiply-add; each
  entry names the allowed alternative. The three `clippy.toml` copies must
  stay identical, a test in `xtask` fails otherwise. A new crate under
  `crates/` goes in `SIMULATION_CRATES`, with its copy of `clippy.toml`, or in
  `NON_SIMULATION_CRATES` in `xtask/src/determinism_lints.rs`.
- **Golden hash.** CI computes a fixed floating point computation on every
  target, build profile and thread count and compares the hashes with
  `crates/test/golden-hash.txt`. A change to that reference changes the
  numeric results of the engine: it needs its own reviewed commit
  explaining why.
- **Dependencies.** Add a dependency to `[workspace.dependencies]` first.
  `cargo deny check` rejects non-permissive licenses, duplicate versions and
  unmaintained crates.

## Commits and pull requests

Every commit subject and pull request title follows
`<gitmoji> <type>(<optional scope>): <summary>`, for example
`✨ feat(xtask): add the boundaries command`.

- Gitmoji from <https://gitmoji.dev/>, chosen for the intent.
- Type from Conventional Commits: `feat`, `fix`, `docs`, `style`,
  `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`. Append `!`
  before the colon for a breaking change.
- Scope names the subject of the change, such as `math`, `sim`, `ecs`,
  `test`, `xtask` or `github`; omit it for repository-wide changes.
- Summary in the imperative mood, lower case first word, no trailing
  period. The whole subject is at most 72 characters long.
- One change per commit. A message describes the change itself, never an
  internal plan, task or step.

CI rejects a pull request whose title or any commit subject breaks the
format, merge commits aside, and reports a push to `main` that breaks it. Check locally with
`git log --no-merges --format=%s origin/main..HEAD | cargo xtask commit-format`.
