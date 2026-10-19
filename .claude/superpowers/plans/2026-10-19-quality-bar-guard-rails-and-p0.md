# Quality bar: guard rails and P0 fixes

> For agentic workers: execute with superpowers:subagent-driven-development, one task at a time, test first.

**Goal:** land the six guard-rail items and the twenty P0 items of the quality-bar backlog, so that no malformed file, message or game-code panic crashes a tool, no editor action loses data, and the CI gates and policies that protect later work are in place.

**Spec:** `.claude/superpowers/plans/2026-10-19-quality-bar-backlog.md` (the verified audit and its backlog) and `AGENTS.md`. Each task below carries its backlog item verbatim plus the full text of the audit findings it resolves, with the verifiers' notes; the verifiers' scope corrections are binding.

## Global Constraints

- The AGENTS.md quality bar binds every task: every behaviour change comes with tests and a fix starts with a test that reproduces the bug (red first, then green, then refactor); fix root causes, never symptoms; no workarounds, disabled tests, silenced lints, or unwrap or expect on a path a user can reach; errors are precise and actionable (what, where: file, line, field path, entity, tick; and what to do); public APIs and file formats are versioned and a breaking change is deliberate and documented.
- The project's ambition binds every design choice: no privileged API (built-ins use the same public API as games and project plugins), the editor is a client of the control surface (anything the editor can do, the protocol and the command line tool can do), one way to do each thing, determinism as the product.
- Determinism: simulation code never uses platform-dependent float functions, wall-clock time, or unordered collections; never weaken clippy.toml. A lint opt-out is `#[expect(lint, reason = "...")]` with a real reason, never a bare allow.
- Golden policy: a task whose golden impact is "certain" re-records the goldens with the README commands and explains why in its commit; a task marked "possible" runs the golden verification (`cargo test --locked -p boxes --test golden`) and treats a moved hash as a deliberate decision to justify, never as noise. "none" still runs the golden test when it touches engine crates.
- Protocol policy: the first task that changes the control or project wire shape bumps the version (control protocol 3 to 4, project protocol 2 to 3) and records it in the CHANGELOG once the CHANGELOG exists; later tasks on this branch update the pinned fingerprint and the same changelog entry instead of bumping again. After changing protocol types, regenerate the TypeScript bindings and declarations (`cargo test --locked -p ludenea-control --test typescript`, `cargo test --locked -p ludenea-project --test typescript`, `node editor/scripts/bundle-dts.mjs`) and commit them.
- Toolchain: pinned in rust-toolchain.toml; build and test with `--locked`; keep Cargo.lock committed; add a dependency only when the standard library and existing dependencies cannot do the job, and say why in the report.
- Verification before every commit: `cargo fmt --all --check`; `cargo clippy --locked --workspace --all-targets -- -D warnings`; the tests of every touched crate (`cargo test --locked -p <crate>`); the golden test for engine changes; for editor changes `pnpm -C editor typecheck` and `pnpm -C editor test`. Once the task that adds `cargo xtask check` has landed, run it too. Test output must be pristine.
- Code, comments, documentation, and commit messages are in English. Match the surrounding code's style, naming, and comment density. Avoid unexplained abbreviations; no em dashes.
- Commits: gitmoji plus Conventional Commits, `<emoji> <type>(<scope>): <summary>`, subject at most 50 characters when possible and body wrapped at 72. Never mention backlog item identifiers (such as FND-1 or P0), tasks, phases, plans, audits, or findings in commit messages, code, comments, or documentation: state the objective in plain words. Never add Co-Authored-By or any tool attribution. Never override the git identity. One task may make several focused commits.
- Work on the current branch in the repository root. Never switch branches, rebase, amend commits from earlier tasks, push, or touch remotes. Never run `git clean`. Do not commit AGENTS.md, CLAUDE.md, or anything under .superpowers/.
- Preserve the strengths listed below; changes in those areas lean on the tests that prove them.

## Strengths to preserve (from the audit)

What the auditors judged solid. Changes in these areas should keep these properties and lean on the tests that prove them.

- Determinism core: math uses only exactly rounded operations and sqrt; clippy.toml bans transcendental functions, min and max, mul_add, clocks and hash collections; the hash serializer is canonical by construction (fixed-width little-endian, canonical NaN) with a pinned value; Parallel works in fixed chunks with results in item order; graph coloring and gather-then-scatter make the solve independent of thread count (proven at 1 to 16 threads, including a color larger than two chunks). Do not trade these for speed.
- Goldens: a 10,001-hash trace and a replay with prefab, data asset and edits verified on macOS, Linux and Windows at 1, 2 and 8 threads in release, in debug inside cargo test, under wasmtime and in Chromium; an editor session replays hash for hash in the native command line tool; verify re-runs to tell nondeterminism from a platform change.
- Decoders and restore: Snapshot, Replay and EditLog read their version first and reject trailing bytes; World::restore validates allocator state and liveness before mutating; replay decoding checks every input and edit before playing; tens of thousands of fuzzed snapshots, traces, scenes, replays and protocol batches produced no panic other than the thread count.
- Type-level guards: the Schema derive refuses shape-changing serde attributes and generic types with spanned errors; HashMap and HashSet have no Schema; derive(Query) checks read-only access syntactically and at type level; unsafe code is confined to query.rs with SAFETY arguments; access conflicts are refused at build with positions and a fix.
- Scenes and prefabs: loading is transactional; local identifiers are never reused and stale references survive; override text splicing keeps formatting and comments byte for byte (4,000 fuzzed edits, no panic); prefab cycles are detected with their chain; data asset errors give the file's line and column.
- Sessions and branches: each branch stores only its own segment and checkpoints are Arc-shared across branches; checkpoint spacing is logarithmic with tested seek bounds; game panics during step, seek, resimulation, restore, compare, divergence and picking are isolated per tick; undo history migrations from earlier versions are tested; the divergence report is incremental and pays nothing unless requested.
- Control protocol: the envelope and handshake survive invalid JSON, non-UTF-8 lines and unknown variants; the wire format is pinned by a golden JSON test and an exhaustive error-code list; 64-bit integers travel as decimal strings; generated TypeScript bindings are regenerated from scratch in CI; work per advance is bounded (resimulation, ghost, report, resource pushes, pacer) as back-pressure.
- Projects and serving: path confinement through symlinks with O_NOFOLLOW; crash-safe atomic writes; a watcher that does not trust event kinds; content-hashed wasm builds that keep the previous game; a careful rebuild handover; late-joining editors catch up without missed or repeated events; ludenea serve checks Host against DNS rebinding and Origin against cross-site WebSocket hijacking.
- Editor architecture: the plugin loader charges registrations to their plugin, rolls back a failed activate, swaps a reload in one step and restores the previous plugin on failure; one React instance across plugins, checked end to end; plugins bundle without Node.js with one failing plugin never blocking others; the control client correlates by id and reference-counts topic subscriptions; browser crash recovery restarts the worker with the latest session and a crash budget.
- Editor quality signals: strict TypeScript with noUncheckedIndexedAccess and no any; about 90 percent of editor lines covered by behaviour-named tests; accessibility roles throughout with no unlabelled control in the live audit; virtualized entity lists measured at 60 fps on 100,000 entities in the scene tree.

### Task 1: Enforce rustfmt, rustdoc and WebAssembly clippy in CI and in a local cargo xtask check

Backlog item FND-1 (do not cite this identifier in commits or code). Workstream: FND: Tooling and CI gates (grade C).

**Workstream context.**

Subsystem: cross-cutting: CI, lints, documentation gates, coverage.

**Problem.** CI proves determinism well but enforces only native clippy and tests. There is no rustfmt or rustdoc gate (the docs build fails today), WebAssembly targets are never linted and are not lint-clean, the editor has no linter, formatter or coverage, divergence dumps never reach the CI artifact in the realistic failure path, project and game-process tests run on Linux or unix only, the determinism lints miss several sources and some opt-outs carry no reason, and the public Rust API is barely documented.

**Target state.** One local entry point (cargo xtask check) and CI run rustfmt, rustdoc with -D warnings, clippy on native and WebAssembly targets, ESLint and Prettier, coverage, and the cross-platform project tests. Every lint opt-out is an #[expect] with a reason. Dumps from any failing platform reach an artifact. The facade has a compiled getting-started guide and missing_docs is enforced on game-facing crates.

**Item.**

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: delivery-12, delivery-7.
- Depends on: nothing.
- Locations and files: `.github/workflows/determinism.yml`, `xtask/src/main.rs`, `crates/ludenea-control/src/protocol.rs:108`, `crates/ludenea-control/src/in_process.rs:76-91`, `crates/ludenea-serve/src/main.rs:41`, `crates/ludenea-serve/Cargo.toml:6-8`.
- Acceptance: CI runs cargo fmt --all --check (engine and src-tauri), RUSTDOCFLAGS=-D warnings cargo doc --locked --no-deps --workspace, and clippy -D warnings for wasm32-unknown-unknown (ludenea, ludenea-control, boxes lib; ludenea-debug-draw with render) and wasm32-wasip1 (boxes bins); all green. cargo xtask check runs the same locally. protocol.rs:108 uses a pointer-width-aware expect, answer and malformed in in_process.rs compile only where used, serve main.rs:41 writes [::1] as code, and the ludenea-serve binary sets doc = false.
- First failing test: The new CI steps: today cargo clippy --locked --target wasm32-wasip1 -p boxes --bins -- -D warnings and the rustdoc step fail; both must pass.

**Audit findings resolved by this task (full text).**

#### Finding delivery-12: No formatter or linter for TypeScript; rustfmt and rustdoc not enforced, and the docs build fails

- Verified status: confirmed; final severity: medium; rule: tooling; feature: code quality tooling.
- Locations: editor/package.json:9-15, rust-toolchain.toml:3, .github/workflows/determinism.yml:22, crates/ludenea-serve/src/main.rs:41, crates/ludenea-serve/Cargo.toml:6-8, editor/shared/src/editor-shell.js, editor/shared/scripts/export-lists.mjs
- Evidence: The editor has 87 non-test TypeScript files (about 15,000 lines of React and protocol code) and no ESLint, Prettier or Biome configuration or script, so floating promises and React hook dependency mistakes go unchecked. rustfmt is installed by rust-toolchain.toml, and `cargo fmt --all --check` passes today, but CI never runs it. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --locked` fails with 'unresolved link to `::1`' at crates/ludenea-serve/src/main.rs:41 and warns 'output filename collision at target/doc/ludenea/index.html': the `ludenea` binary of ludenea-serve shares a doc path with the engine facade crate game developers read. editor/shared/src/*.js is generated by export-lists.mjs and committed, but the staleness check (determinism.yml:83-93) does not cover it.
- Reachability: Contributors, and game developers reading the facade's API docs.
- Recommendation: Add CI steps for `cargo fmt --all --check` and the same for src-tauri. Add an ESLint flat config with typescript-eslint recommended-type-checked (no-floating-promises, no-misused-promises) and eslint-plugin-react-hooks, plus Prettier or Biome configured to the current line width, as `pnpm -C editor lint` and `format:check` in CI. Add `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --workspace --locked`, set `doc = false` on the ludenea-serve [[bin]], and write `[::1]` as code. Either gitignore editor/shared/src/*.js or include it in the staleness check. Provide one local entry point that runs these checks (for example `cargo xtask check`).
- First test suggested by the auditor: The new CI lint, format and doc steps (the doc step is red today)
- Verifier notes (binding scope corrections): Reproduced: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p ludenea-serve -p ludenea --locked` fails with 'unresolved link to `::1`' at crates/ludenea-serve/src/main.rs:41:25. It also warns 'output filename collision at target/doc/ludenea/index.html': the ludenea-serve bin `ludenea` (Cargo.toml:6-8) collides with the facade lib. editor/package.json:9-15 has no lint or format script, and there i...

#### Finding delivery-7: WebAssembly targets are never linted, and they are not lint-clean

- Verified status: confirmed; final severity: medium; rule: tooling; feature: CI lints.
- Locations: .github/workflows/determinism.yml:22, .github/workflows/determinism.yml:51, crates/ludenea-control/src/protocol.rs:108, crates/ludenea-control/src/in_process.rs:76, crates/ludenea-control/src/in_process.rs:91, crates/ludenea-control/src/lib.rs:20, crates/ludenea-control/src/worker.rs, crates/ludenea-debug-draw/src/render.rs, crates/ludenea/src/lib.rs:12-20
- Evidence: CI runs clippy for the host target only, so code under `cfg(all(target_arch = "wasm32", target_os = "unknown"))` (worker.rs, 99 lines; render.rs, 279 lines; the facade's wasm entry) never sees the determinism lints. `cargo clippy --locked --target wasm32-unknown-unknown -p ludenea-control -p ludenea -p boxes --lib -- -D warnings` fails: 'this lint expectation is unfulfilled --> crates/ludenea-control/src/protocol.rs:108' (#[expect(clippy::large_enum_variant)] depends on pointer width). `cargo clippy --locked --target wasm32-wasip1 -p boxes --bins -- -D warnings` fails with 3 errors: function `answer` is never used (in_process.rs:76), function `malformed` is never used (:91), and the same expectation. The wasm job's `cargo build` shows these as warnings that nobody denies.
- Reachability: Engine and plugin contributors: a wasm-only regression, including a forbidden float function in the worker, passes CI.
- Recommendation: Make the expectation pointer-width-aware with `#[cfg_attr(target_pointer_width = "64", expect(clippy::large_enum_variant))]`, or box the large variant. Compile `answer` and `malformed` only on targets that use them, or have the worker use them. Add three CI steps: `cargo clippy --locked --target wasm32-unknown-unknown -p ludenea -p ludenea-control -p boxes --lib -- -D warnings`, `cargo clippy --locked --target wasm32-unknown-unknown -p ludenea-debug-draw --features render --lib -- -D warnings`, and `cargo clippy --locked --target wasm32-wasip1 -p boxes --bins -- -D warnings`.
- First test suggested by the auditor: The new CI clippy steps, red today on protocol.rs:108 and in_process.rs:76/91
- Verifier notes (binding scope corrections): Reproduced. `cargo clippy --locked --target wasm32-unknown-unknown -p ludenea-control -p ludenea -p boxes --lib -- -D warnings` exits 101 with 'this lint expectation is unfulfilled --> crates/ludenea-control/src/protocol.rs:108:10'. `cargo clippy --locked --target wasm32-wasip1 -p boxes --bins -- -D warnings` exits 101 with three errors: `answer` never used (in_process.rs:76), `malformed` never us...

### Task 2: Divergence dumps reach a CI artifact on every platform and target

Backlog item FND-4 (do not cite this identifier in commits or code). Workstream: FND: Tooling and CI gates (grade C).

**Workstream context.**

Subsystem: cross-cutting: CI, lints, documentation gates, coverage.

**Problem.** CI proves determinism well but enforces only native clippy and tests. There is no rustfmt or rustdoc gate (the docs build fails today), WebAssembly targets are never linted and are not lint-clean, the editor has no linter, formatter or coverage, divergence dumps never reach the CI artifact in the realistic failure path, project and game-process tests run on Linux or unix only, the determinism lints miss several sources and some opt-outs carry no reason, and the public Rust API is barely documented.

**Target state.** One local entry point (cargo xtask check) and CI run rustfmt, rustdoc with -D warnings, clippy on native and WebAssembly targets, ESLint and Prettier, coverage, and the cross-platform project tests. Every lint opt-out is an #[expect] with a reason. Dumps from any failing platform reach an artifact. The facade has a compiled getting-started guide and missing_docs is enforced on game-facing crates.

**Item.**

- Priority P1, kind fix, effort S, golden impact none.
- Findings: delivery-1.
- Depends on: nothing.
- Locations and files: `.github/workflows/determinism.yml:23-54`, `examples/boxes/tests/golden.rs:20,127`.
- Acceptance: examples/boxes/tests/golden.rs reads LUDENEA_DUMP_DIR (falling back to the temporary directory); the workflow sets it to the workspace dump folder; the verify steps run with if: success() || failure(); the wasm job uploads dump/ on failure; artifacts are named per operating system, target and thread count and include the failing run's trace.
- First failing test: examples/boxes/tests/golden.rs dump_dir_follows_ludenea_dump_dir; then a throwaway branch perturbing one golden hash shows tick-N.snap and tick-(N-1).snap in every artifact.

**Audit findings resolved by this task (full text).**

#### Finding delivery-1: Divergence dumps never reach the CI artifact in the realistic failure path, and the wasm job uploads nothing

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: CI determinism workflow.
- Locations: .github/workflows/determinism.yml:23, .github/workflows/determinism.yml:24-35, .github/workflows/determinism.yml:36-42, .github/workflows/determinism.yml:43-54, examples/boxes/tests/golden.rs:20-34, examples/boxes/tests/golden.rs:127-135, crates/ludenea-cli/src/commands.rs:214-215
- Evidence: The native job runs `cargo test --locked --workspace` (line 23) before the verify loops (lines 24-35). The golden tests in that run pass `--dump-dir std::env::temp_dir()/boxes-golden-N-threads` (golden.rs:20, :127). A determinism break therefore fails `cargo test` first: its snapshots land in the runner's temp directory, the verify steps that would write `dump/` (the default at commands.rs:214) are skipped, and `Upload the divergence dump` (path: dump/, if-no-files-found: ignore) uploads nothing. The wasm job (lines 43-54) has no upload step, so a wasmtime divergence leaves no snapshots either. The engine spec (10.2) promises 'In CI this becomes a build artifact', and the 2b spec promises 'a CI trace or snapshot opens in the editor'.
- Reachability: Any determinism regression on any CI platform: the developer gets a red job and no snapshot to open in the editor's Divergence panel or pass to `diff`.
- Recommendation: Make the golden tests honour a dump directory CI controls: read `LUDENEA_DUMP_DIR` in examples/boxes/tests/golden.rs (falling back to temp_dir) and set `LUDENEA_DUMP_DIR: ${{ github.workspace }}/dump` in the workflow env. Alternatively, run the verify steps with `if: success() || failure()` so they always produce dump/. Add an `upload-artifact` step with `if: failure()` to the wasm job (wasmtime writes dump/ through --dir=.). Also write the failing run's trace (`run --trace dump/<os>.trace`) so the artifact opens directly in the trace viewer. Name artifacts per OS and target.
- First test suggested by the auditor: n/a (CI): prove it once on a throwaway branch that perturbs one golden hash, then check that every OS's dump artifact contains tick-N.snap and tick-(N-1).snap, plus a wasm artifact.
- Verifier notes (binding scope corrections): The finding holds as written. determinism.yml:23 runs `cargo test --locked --workspace` before the verify loops at :24-35, and golden.rs:20 and :127 pass `--dump-dir std::env::temp_dir()/...`. So a divergence that also shows in the debug profile (any platform-level divergence) fails the test step, the verify steps that write `dump/` (default at commands.rs:214) are skipped, and the upload at :36-4...

### Task 3: Versioning policy and a CHANGELOG with a formats table, checked by a test

Backlog item VER-1 (do not cite this identifier in commits or code). Workstream: VER: Versioning and compatibility (grade C).

**Workstream context.**

Subsystem: cross-cutting: file formats, protocols, public APIs.

**Problem.** Binary formats mostly read their version first and two real migrations exist, but meta versions are never checked, traces and input logs decode in full before checking, text formats silently drop unknown or misspelled fields, the branch tree and the stored session have no version (and a stored session that does not decode blocks its document while a refused one gets overwritten), the protocol wire shape changed several times under the same number, version errors give wrong advice, the engine identity is a constant nobody reads, the plugin API has no version, and no policy or changelog exists.

**Target state.** A written policy with a CHANGELOG formats table checked against the constants; one versioned-decode helper and one VersionError; every format, editor state included, versioned, strict and pinned by frozen fixtures; the protocol shape pinned by a fingerprint; the engine identity and the plugin API version recorded and reported.

**Item.**

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: versioning-9.
- Depends on: nothing.
- Locations and files: `CHANGELOG.md (new)`, `AGENTS.md`, `README.md`, `crates/ludenea/tests/formats_documented.rs (new)`.
- Acceptance: CHANGELOG.md (Keep a Changelog) holds a Formats and protocols table (current and oldest readable version of each format and protocol) and sections for the Rust API, control protocol, project protocol and plugin API. AGENTS.md and the README state the rules: the version is read first; any change to an encoded type bumps it; user files stay readable at N-1; editor state may drop to N-1 but a refused file is set aside, never overwritten; protocol changes accumulate under the next version, which is bumped once per release. A test parses the table and compares it with every version constant.
- First failing test: crates/ludenea/tests/formats_documented.rs every_format_version_is_in_the_changelog.

**Audit findings resolved by this task (full text).**

#### Finding versioning-9: No versioning policy, changelog, or protocol reference: breaking changes are written down only in commit messages

- Verified status: partially; final severity: low; rule: versioning; feature: cross-cutting: formats, protocols, public APIs.
- Locations: AGENTS.md:22-23, README.md, crates/ludenea-control/src/protocol.rs:21, crates/ludenea-project/src/protocol.rs:13, crates/ludenea-app/src/scene.rs:23, crates/ludenea-app/src/prefab.rs:19, crates/ludenea-app/src/meta.rs:16, crates/ludenea-app/src/replay.rs:13, crates/ludenea-app/src/trace.rs:9, crates/ludenea-ecs/src/snapshot.rs:12, crates/ludenea-app/src/input_log.rs:6, crates/ludenea-app/src/edit.rs:69, crates/ludenea-control/src/undo.rs:11
- Evidence: AGENTS.md requires that "a breaking change is deliberate and documented", yet the repository has no CHANGELOG, no table of format versions, and no protocol reference. Eleven version constants are scattered over five crates. The branch tree, the stored session, ludenea.toml, the generated types, and the plugin API have none. What changed from protocol 2 to 3, or from scene 1 to 2, is described only in commit messages and in the internal specs under .claude/superpowers. A studio writing its own client of the control surface (an AGENTS.md ambition) has generated TypeScript types but no changelog, and no rule says which versions a reader must still accept: the 2b spec leaves it to "builder's call" (2026-10-18 spec section 3). Today's compatibility comes from discipline: version_1 and version_2a modules exist, but undo variants were appended without a bump and metas are never checked.
- Reachability: Every developer changing a format, and every user or tool author upgrading.
- Recommendation: Adopt and write down this policy (CHANGELOG.md at the root, Keep a Changelog style, with a "Formats and protocols" table at the top listing each format's current and oldest readable version): (1) every persisted or exchanged format starts with its version, read before anything else; (2) any change to an encoded type bumps its version, with no silent append; (3) readers accept every version back to the oldest supported one, at least N-1 for user files (scene, prefab, meta, data asset, ludenea.toml, replay, trace, snapshot), with migrations as pure version_N to version_N+1 modules (the existing edit and undo pattern); editor state (stored session, undo, branches) may drop to N-1, but a refused file is set aside, never deleted; (4) newer versions are refused with "update Ludenea", older unsupported ones with the last engine that reads them; (5) text formats deny unknown fields; (6) protocols keep exact-match handshakes before 1.0, constants are generated for TypeScript, and every bump gets a changelog entry listing the commands and fields that changed; (7) one workspace engine version plus STATE_HASH_VERSION, with the minor version bumped at every format, protocol, or hash change; (8) every version keeps a frozen fixture forever. A test keeps the table honest.
- First test suggested by the auditor: crates/ludenea/tests/formats_documented.rs: fn every_format_version_is_in_the_changelog(): parse the "Formats and protocols" table of CHANGELOG.md and assert each row's current version equals its constant (PROTOCOL_VERSION, PROJECT_PROTOCOL_VERSION, SCENE_VERSION, PREFAB_VERSION, META_VERSION, REPLAY_VERSION, TRACE_VERSION, SNAPSHOT_VERSION, INPUT_LOG_VERSION, EDIT_LOG_VERSION, UNDO_VERSION, BRANCHES_VERSION, STATE_HASH_VERSION), so a bump without its documentation fails CI.
- Verifier notes (binding scope corrections): What holds: the repository has no CHANGELOG, no table of format versions and no protocol reference outside the generated types and Rust doc comments. The branch tree has no version of its own (branches.rs:539-557 StoredTree/StoredBranch, encoded raw at :440 and :584); only the logs inside it carry one. The stored session is unversioned JSON of SavedSession (editor-shell/src/stores/document.ts:155-...

### Task 4: Pin the protocol wire shape and generate the versions and the reply map from Rust

Backlog item VER-3 (do not cite this identifier in commits or code). Workstream: VER: Versioning and compatibility (grade C).

**Workstream context.**

Subsystem: cross-cutting: file formats, protocols, public APIs.

**Problem.** Binary formats mostly read their version first and two real migrations exist, but meta versions are never checked, traces and input logs decode in full before checking, text formats silently drop unknown or misspelled fields, the branch tree and the stored session have no version (and a stored session that does not decode blocks its document while a refused one gets overwritten), the protocol wire shape changed several times under the same number, version errors give wrong advice, the engine identity is a constant nobody reads, the plugin API has no version, and no policy or changelog exists.

**Target state.** A written policy with a CHANGELOG formats table checked against the constants; one versioned-decode helper and one VersionError; every format, editor state included, versioned, strict and pinned by frozen fixtures; the protocol shape pinned by a fingerprint; the engine identity and the plugin API version recorded and reported.

**Item.**

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: protocol-8, versioning-10, protocol-12, ts-quality-8.
- Depends on: VER-1.
- Locations and files: `crates/ludenea-control/src/protocol.rs:21`, `crates/ludenea-project/src/protocol.rs:13`, `crates/ludenea-control/tests/typescript.rs`, `crates/ludenea-project/tests/typescript.rs`, `crates/ludenea-control/src/in_process.rs:51-66`, `editor/packages/control-client/src/client.ts:22-187`, `editor/packages/control-client/src/project.ts:12-33`.
- Acceptance: A fingerprint of the generated ClientMessage and ServerMessage declarations (and the project protocol's) is pinned per protocol version by a test, so a shape change without a deliberate bump and changelog entry fails CI. The fingerprint travels in Hello and Welcome; a mismatch is refused with rebuild the editor (pnpm -C editor build). PROTOCOL_VERSION, PROJECT_PROTOCOL_VERSION and an exhaustive Command::reply_type map are generated into the bindings; client.ts and project.ts import them; handshake() refuses a welcome of another version with version_mismatch.
- First failing test: crates/ludenea-control/tests/typescript.rs the_wire_shape_is_pinned_to_protocol_version_3, plus control-client connect_rejects_a_welcome_of_another_protocol_version.

**Audit findings resolved by this task (full text).**

#### Finding protocol-8: Protocol version 3 does not pin the wire shape: stale editor builds and newer engines both say 3 and misbehave without warning

- Verified status: partially; final severity: medium; rule: versioning; feature: control protocol / versioning.
- Locations: crates/ludenea-control/src/protocol.rs:21, crates/ludenea-control/src/protocol.rs:283-301, crates/ludenea-control/src/protocol.rs:505-512, crates/ludenea-control/src/in_process.rs:51-59, editor/packages/control-client/src/client.ts:22, editor/packages/control-client/src/client.ts:174-176, README.md:109
- Evidence: PROTOCOL_VERSION = 3 was set in 2118b65. Later commits changed the wire under the same number: 6c9dd4a added `load_addresses` to Loaded, 05e43fd added `keeps_assets` with `#[serde(default)]` to LoadSession and Session, and ef4b603 added Topic::Resource; a993da6 and 5a3c70d added commands. serde ignores unknown fields and fills defaulted ones. A client built before 05e43fd therefore loads a replay's session with keeps_assets=false and no error, and a newer TypeScript client reading an older Reply gets `undefined` fields. `engine_version` is always CARGO_PKG_VERSION "0.1.0". The client never checks `welcome.protocol_version`: editor-builtins/test/support.tsx:42 even welcomes with version 2. The TypeScript PROTOCOL_VERSION is a hand copy. README.md:109 serves `--editor-dir editor/apps/web/dist`, which nothing rebuilds when the engine changes.
- Reachability: Pulling engine changes and running `ludenea serve` with a dist folder that was not rebuilt, or a `ludenea` built with embed-editor that builds a game from a newer engine checkout; also third-party clients written against an earlier v3.
- Recommendation: Generate a protocol fingerprint, for example an xxh3 of the concatenated ts-rs declarations of ClientMessage and ServerMessage written by tests/typescript.rs into generated/ProtocolFingerprint.ts and exposed in Rust. Send it in Hello and Welcome. The host refuses a mismatch with VersionMismatch: "this editor was built for another revision of protocol 3; rebuild the editor (pnpm -C editor build)". Add a test that pins the fingerprint per PROTOCOL_VERSION (`assert_eq!(fingerprint, PROTOCOL_3_FINGERPRINT)`), so a shape change without a deliberate version bump fails CI. Export PROTOCOL_VERSION into the bindings and make the client reject a welcome with another version. Document version changes in a protocol changelog.
- First test suggested by the auditor: crates/ludenea-control/tests/typescript.rs (or protocol_v3.rs): `the_wire_shape_is_pinned_to_protocol_version_3`, asserting the computed fingerprint equals a committed constant. Add tests/stdio.rs `a_hello_with_another_fingerprint_is_refused`.
- Verifier notes (binding scope corrections): The facts are correct. PROTOCOL_VERSION is still 3 (protocol.rs:21) while 6c9dd4a, 05e43fd, ef4b603, a993da6 and 5a3c70d changed the wire. engine_version is always CARGO_PKG_VERSION (in_process.rs:56). Nothing pins the wire shape per version: protocol_v3.rs:84 only asserts the number. I downgraded it from high for three reasons. First, every one of those commits landed on 2026-10-18, the day 2118b...

#### Finding versioning-10: Protocol versions are copied by hand into TypeScript, and the client never checks the version in the welcome

- Verified status: confirmed; final severity: low; rule: versioning; feature: control client and project client.
- Locations: editor/packages/control-client/src/client.ts:22, editor/packages/control-client/src/project.ts:12, editor/packages/control-client/src/client.ts:162-187, crates/ludenea-control/tests/typescript.rs, editor/packages/editor-builtins/test/support.tsx:39-42, crates/ludenea-project/src/protocol.rs:305-320
- Evidence: PROTOCOL_VERSION = 3 and PROJECT_PROTOCOL_VERSION = 2 are hand-written TypeScript constants. The generated-files CI check covers types but not these constants, and only the end-to-end run would notice a drift. `handshake` resolves on any `welcome` without comparing `protocol_version` (client.ts:174-176). The editor-builtins test fakes still answer with project protocol 1 and control protocol 2 (support.tsx:39 and 42), and nothing notices. On the Rust side, the project service's refusal of another protocol version has no test: ludenea-project/src/protocol.rs is at 59% line coverage, and the only service test sends the matching version (service.rs:813). The control handshake does have Rust tests (host.rs:69, protocol_v3.rs:83).
- Reachability: A tool or game host that answers with another version, or a developer who bumps one constant but not the other.
- Recommendation: Have crates/ludenea-control/tests/typescript.rs (and the ludenea-project one) also write generated/version.ts with `export const PROTOCOL_VERSION = 3;` (and project/version.ts likewise), import them in client.ts and project.ts, and let the existing "generated files are current" CI step guard them. In `handshake`'s accept, refuse a welcome whose protocol_version differs: close the transport and reject with ProtocolFailure("version_mismatch", ...). Fix the stale fakes. Add a Rust test of the project refusal.
- First test suggested by the auditor: editor/packages/control-client/test/client.test.ts: it("connect_rejects_a_welcome_of_another_protocol_version"): a fake transport answers `{type: "welcome", protocol_version: 2, ...}`; expect connect to reject with code "version_mismatch" and the transport to be closed. Also crates/ludenea-project/src/service.rs tests: hello_with_another_project_protocol_version_is_refused.
- Verifier notes (binding scope corrections): Confirmed by reading the code. PROTOCOL_VERSION = 3 (client.ts:22) and PROJECT_PROTOCOL_VERSION = 2 (project.ts:12) are typed by hand. crates/ludenea-control/tests/typescript.rs exports types only, no constants, so the 'generated files are current' check does not cover them (the bundled .d.ts at crates/ludenea-project/types/control-client.d.ts:1293/1475 copies the TypeScript source). `handshake` r...

#### Finding protocol-12: TypeScript reply typing and protocol version are written by hand next to the generated bindings

- Verified status: partially; final severity: low; rule: dx; feature: generated TypeScript bindings.
- Locations: editor/packages/control-client/src/client.ts:22, editor/packages/control-client/src/client.ts:35-91, editor/packages/control-client/src/project.ts:12-33, editor/packages/control-client/test/client.test.ts:24, editor/packages/control-client/test/client.test.ts:78
- Evidence: `ReplyFor<C>` is a 57-line nested conditional mapping 40 command types to their replies, maintained by hand (its indentation is already broken at lines 67-69). A new Rust command with a new reply silently types as `{type: "done"}` in TypeScript, so plugin authors get wrong types and must cast. Nothing checks the map against what Host::handle returns. The same goes for ProjectReplyFor and both PROTOCOL_VERSION constants. The client tests use error codes that do not exist (`unsupported_version`, `unknown_entity`), which `ProtocolFailure<C extends string>` accepts, so they do not exercise the real ErrorCode contract.
- Reachability: Every plugin and tool author using `control.request(...)`.
- Recommendation: Make the mapping a single source of truth in Rust: an exhaustive `Command::reply_type(&self) -> &'static str`, checked by a test that runs every command through Host and compares the reply's tag. Have tests/typescript.rs export it as `ReplyTypes` (`{ get_hash: "hash", ... }`) along with PROTOCOL_VERSION, and define `ReplyFor<C> = Extract<Reply, { type: ReplyTypes[C["type"]] }>`. Use real ErrorCode values in the client tests.
- First test suggested by the auditor: crates/ludenea-control/tests/host.rs: `every_command_names_the_reply_it_gets`. For each command of the round-trip list run on a loaded host, assert that a successful reply's tag equals `command.reply_type()`.
- Verifier notes (binding scope corrections): ReplyFor (client.ts:35-91), ProjectReplyFor (project.ts:15-33) and both PROTOCOL_VERSION constants are written by hand, and nothing checks them against Rust. The client tests use error codes that do not exist (client.test.ts:24 `unsupported_version`, :78 `unknown_entity`). However, the map is complete today: all 28 non-Done reply variants returned by Host::execute (Asset, DebugDraw, Deleted, Diffe...

#### Finding ts-quality-8: Hand-maintained TypeScript copies of Rust contracts (reply map, versions, RON writer, input log decoder)

- Verified status: partially; final severity: medium; rule: versioning; feature: control client / Rust and TypeScript duplication.
- Locations: editor/packages/control-client/src/client.ts:22, editor/packages/control-client/src/client.ts:35-91, editor/packages/control-client/src/client.ts:162-187, editor/packages/control-client/src/project.ts:12-33, editor/packages/editor-builtins/src/assets/value.ts:76-121, editor/apps/web/src/session-guard.ts:29-44, editor/packages/editor-builtins/test/support.tsx:39-42
- Evidence: Several engine contracts are copied by hand into TypeScript:
- ReplyFor and ProjectReplyFor are long conditional types written by hand, and the fallback branch types any unmapped command as `done`. A new command with a new reply is therefore mistyped without a compile error.
- PROTOCOL_VERSION = 3 and PROJECT_PROTOCOL_VERSION = 2 are copied from crates/ludenea-control/src/protocol.rs:21 and crates/ludenea-project/src/protocol.rs:13.
- handshake() accepts any welcome without checking its protocol_version. The test fakes welcome with versions 1 and 2 and still pass.
- toRon re-implements the RON writer. Strings go through JSON.stringify, whose escapes \b, \f, and \u0007 ron 0.12.2 rejects ('Unknown escape character' and 'Missing { in Unicode escape', ron-0.12.2/src/parse.rs). create_asset does not validate the text (crates/ludenea-project/src/files.rs:191-216), so the file is written invalid.
- withoutTicks hand-decodes the postcard InputLog prefix (version and player-count varints), which is pinned only by a Rust test.
- Reachability: Engine developers adding a command or changing a format (mistyped replies, crash recovery that silently breaks on an InputLog version bump); users filling in a string field with a control character in the 'New data asset' form.
- Recommendation: Generate rather than copy:
- Emit the command-to-reply map from Rust (a table next to Command, exported by the ts-rs test into generated/ReplyFor.ts).
- Export both protocol versions from the generated index.
- Make handshake() refuse a welcome whose protocol_version differs, with version_mismatch.
- Replace toRon with a host command (for example encode_asset_value { asset, value } -> text), so Rust writes RON.
- Replace withoutTicks with a protocol option (export_session { scene_only: true } or load_session { keep_future: false }).
- First test suggested by the auditor: editor/packages/control-client/test/client.test.ts: a_welcome_with_another_protocol_version_is_refused_with_version_mismatch; and a type-level test (vitest expectTypeOf) that ReplyFor<{ type: "get_instances" }> is the instances reply, generated from Rust.
- Verifier notes (binding scope corrections): What holds:
- ReplyFor and ProjectReplyFor are hand-written, with a 'done' fallback (client.ts:35-91, project.ts:15-33). I checked every current Reply variant and all are mapped, so nothing is mistyped today, but a new command would silently type as done. That makes the protocol module harder to change safely: medium.
- toRon writes strings through JSON.stringify (assets/value.ts:84), and the resu...

### Task 5: Public editor log; every command dispatcher reports failures there

Backlog item ERR-1 (do not cite this identifier in commits or code). Workstream: ERR: Error architecture (grade C).

**Workstream context.**

Subsystem: cross-cutting: error types, protocol errors, editor error channel, command line errors.

**Problem.** Rust errors are typed and mostly actionable, but the editor has no public error channel (palette, shortcut, viewport and divergence failures reach only the devtools console), protocol errors flatten into a code plus text that the editor parses with regular expressions, value errors give positions inside the value instead of the file, binary decode errors blame game types for truncated files, the command line tool prints errors on stdout and drops scene warnings, and malformed handshakes are told to send Hello first.

**Target state.** One public editor.console used by every dispatcher; ProtocolError carries specific codes and a typed detail; every file error names the file with its line and column; every decode error separates a damaged container from a value mismatch; the command line tool writes errors and warnings to stderr with documented exit codes.

**Item.**

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: shell-2, errors-3, sessions-8, plugins-13, ts-quality-5, ux-live-2, ambition-4, branches-11.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-shell/src/editor.ts:23-125`, `editor/packages/editor-shell/src/palette/CommandPalette.tsx:58`, `editor/packages/editor-shell/src/shortcuts.ts:107`, `editor/packages/editor-builtins/src/console/index.tsx:41-51`, `editor/packages/editor-builtins/src/shared/index.ts:95-99`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:39`, `editor/packages/editor-builtins/src/viewport/overlay.ts:57`, `editor/packages/editor-builtins/src/divergence/CausalReport.tsx:16,26`, `editor/packages/editor-builtins/src/divergence/TraceViewer.tsx:44`, `editor/packages/editor-builtins/src/divergence/index.tsx:45,63,238`, `editor/packages/editor-shell/src/stores/timeline.ts:40-42`.
- Acceptance: Editor exposes console: { log(level, text, location?), report(error, context) } backed by a shell store buffered before the Console plugin activates; it replaces the createEditor log option and logToConsole's private WeakMap. The palette, shortcuts, showPanel, viewport gestures, overlays, divergence actions and timeline refresh report <command title> failed: <message> there; the palette also shows a transient role=alert line. console.error remains only before an editor exists.
- First failing test: editor/packages/editor-shell/test/palette.test.tsx a_failing_command_run_from_the_palette_is_logged_in_the_editor_console, and the same for a shortcut.

**Audit findings resolved by this task (full text).**

#### Finding shell-2: Failures of commands run from the palette, shortcuts, and viewport gestures reach only the browser devtools, never the Console panel

- Verified status: confirmed; final severity: high; rule: errors; feature: console, command palette, shortcuts, viewport.
- Locations: editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-shell/src/palette/CommandPalette.tsx:58, editor/packages/editor-builtins/src/shared/index.ts:95-99, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:39, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:94, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:115, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:180, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:212-221, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:247, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:254, editor/packages/editor-builtins/src/viewport/overlay.ts:57, editor/packages/editor-shell/src/layout/Shell.tsx:101, editor/packages/editor-shell/src/loader.ts:36, editor/packages/editor-shell/src/loader.ts:73, editor/packages/editor-shell/src/loader.ts:82, editor/packages/editor-shell/src/entities.ts:46, editor/packages/editor-shell/src/stores/timeline.ts:41, editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx:91, editor/packages/editor-builtins/src/input/capture.ts:31
- Evidence: The palette and global shortcuts end every run with .catch(console.error) (CommandPalette.tsx:58, shortcuts.ts:107). The viewport does the same through report() for picking, drawing, fetching the debug draw, moving, despawning, and nudging. So 'Despawn selected entities' with nothing despawnable, 'Nudge the selection right' while playing ('pause the game to nudge the selection'), a refused drag, a refused Delete in the viewport, a throwing plugin overlay, and a plugin that does not deactivate cleanly all show nothing in the editor. Panel buttons, by contrast, show their error inline (InspectorPanel.tsx:47, TimelinePanel.tsx:54, project/index.tsx:141). The same command therefore reports its failure when clicked and stays silent when typed. Reproduced: a failing Mod+S appears only in console.error (scratchpad/shell-audit/audit.test.tsx). The Editor interface (editor.ts:23-62) has no logging member, so the shell itself cannot reach the Console panel.
- Reachability: Any command from the palette or a shortcut that throws in ordinary use (no selection, playing, resimulating, a refused edit), and every viewport click, drag, Delete, or arrow key the host refuses.
- Recommendation: Give the shell its own log: editor.console.log(level, text, location?) backed by an observable store, buffered before the Console plugin activates, and replacing the createEditor log option. The Console plugin renders that store instead of its private sink. The palette and installShortcuts report a failure as '<command title> failed: <message>'. The viewport's report() and overlay failures log there too. Keep console.error only as a fallback when no editor exists (startup).
- First test suggested by the auditor: palette.test.tsx: a_failing_command_run_from_the_palette_is_logged_in_the_editor_console. Register a command that throws 'nothing selected', run it through the palette (Enter), and expect editor.console entries to contain 'Despawn selected entities failed: nothing selected'. Add the same for a shortcut, and in viewport.test.tsx a refused drag logs 'viewport: moving failed: ...'.
- Verifier notes (binding scope corrections): Code matches the finding. CommandPalette.tsx:58 and shortcuts.ts:107 end with .catch(console.error). ViewportPanel.tsx:39 report() sends picking, moving, despawning and nudging failures to console.error, and overlay.ts:57 does the same for a throwing overlay. The Editor interface (editor.ts:23-62) has no log member, and logToConsole lives only in editor-builtins. Panels show the same failures inli...

#### Finding errors-3: Command failures from the palette and shortcuts, and several plugin-facing failures, reach only the devtools console; project plugins have no public way to write to the Console panel

- Verified status: confirmed; final severity: high; rule: ambition; feature: editor shell, plugin API.
- Locations: editor/packages/editor-shell/src/palette/CommandPalette.tsx:58, editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-builtins/src/shared/index.ts:98, editor/packages/editor-builtins/src/divergence/TraceViewer.tsx:44, editor/packages/editor-builtins/src/input/capture.ts:31, editor/packages/editor-shell/src/entities.ts:46, editor/packages/editor-shell/src/layout/Shell.tsx:101, editor/packages/editor-shell/src/loader.ts:36, editor/packages/editor-shell/src/loader.ts:73, editor/packages/editor-shell/src/loader.ts:82, editor/packages/editor-shell/src/editor.ts:125, editor/packages/editor-builtins/src/console/index.tsx:42-51, crates/ludenea-project/src/plugins.rs:28-34
- Evidence: `editor.commands.run(command).catch((error) => console.error(...))` is how the palette (CommandPalette.tsx:58) and global shortcuts (shortcuts.ts:107) run every command. Built-in commands such as edit.undo (timeline/index.tsx:105-110), timeline.play-pause, and timeline.step do not catch, so Mod+Z with nothing to undo (code nothing_to_undo), any command before the first build ('no game is connected', from game()), or stepping while resimulating gives the user no feedback in the editor. The README says every panel and action is 'reachable from the command palette'. `editor.entities.watch` subscription failures go to console.warn (entities.ts:46), so the listener just never fires. The Console panel's writer is `logToConsole`, a module-level WeakMap sink exported from @ludenea/editor-builtins (console/index.tsx:42-51). Project plugins may import only react, @ludenea/editor-shell, and @ludenea/control-client (SHARED_MODULES, plugins.rs:28-34), so built-ins can report to the Console and project plugins cannot. That is a privileged API, an AGENTS.md anti-goal. createEditor defaults its document log to the devtools console (editor.ts:125).
- Reachability: Every editor user pressing a shortcut or using the palette when the command fails, and every project plugin author who wants to report an error.
- Recommendation: Add `console: { log(level, text, location?): void; report(error: unknown, context: string): void }` to the public Editor interface in editor-shell. The shell owns the entry list (or a sink the console plugin subscribes to), and the console plugin renders it. Have the palette and shortcuts route failures to `editor.console.report(error, `Command "${title}" failed`)`. Do the same for entities.watch subscription failures (also reject or notify the watcher), loader cleanup failures, and panel id conflicts. Replace logToConsole in the built-ins with the public API and delete the WeakMap, so built-ins use exactly what project plugins use.
- First test suggested by the auditor: editor/packages/editor-shell/test/shortcuts.test.ts: `a failing command run from a shortcut is reported in the editor console`. Register a command whose run rejects with ProtocolFailure('nothing_to_undo', 'there is nothing to undo'), dispatch Mod+Z, and assert that an editor.console entry at level error contains the command title and the message. Add a plugin test that a project-style plugin importing only @ludenea/editor-shell can call editor.console.log.
- Verifier notes (binding scope corrections): The palette (CommandPalette.tsx:58), global shortcuts (shortcuts.ts:107) and showPanel (shared/index.ts:98) all send command failures to console.error. edit.undo and edit.redo (timeline/index.tsx:105-116) use edit(), which throws on resimulating (editing.tsx:6) and does not catch host errors. timeline.play-pause and timeline.step do not catch either. A failing Mod+Z or palette command therefore gi...

#### Finding sessions-8: Failures of commands run from keyboard shortcuts or the command palette only reach the browser devtools

- Verified status: confirmed; final severity: medium; rule: errors; feature: timeline / error display.
- Locations: editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-shell/src/palette/CommandPalette.tsx:58, editor/packages/editor-builtins/src/shared/index.ts:98, editor/packages/editor-shell/src/stores/timeline.ts:40-42
- Evidence: `installShortcuts` and the palette both end in `.catch((error) => console.error(...))`. Mod+Z with nothing to undo, while resimulating ('the game is resimulating; edit once it finishes'), or on an entry of a stale branch shows nothing in the editor. In the last case the session also switches branch and asks to undo again (see the separate finding). Step '.' and step back ',' fail the same way. The same commands run from the timeline buttons show the error (TimelinePanel.tsx:53-57). The README says every panel is 'reachable from the command palette', and the shell already has a console log (`options.log`, editor.ts:125). `refresh()` failures also go to `console.warn` only.
- Reachability: Every keyboard or palette use of undo, redo, step, speed, branch, and inspector commands.
- Recommendation: Report command failures in one place. Give the shell a `runReported(id)` that logs `Cannot <command title>: <message>` to the editor's console (the same `DocumentLog`), and use it from shortcuts, the palette, and `showPanel`. Log timeline refresh failures there too.
- First test suggested by the auditor: editor-shell/test/shell.test.tsx: a_failing_shortcut_command_is_reported_in_the_console
- Verifier notes (binding scope corrections): Both entry points end in console.error only: shortcuts.ts:107 and CommandPalette.tsx:58. showPanel does the same (shared/index.ts:98), and refresh failures go to console.warn (stores/timeline.ts:40-42). commands.run (editor.ts:148-152) adds no reporting, and the timeline commands do not log on their own: edit.undo and edit.redo go through edit(), which only rethrows (editing.tsx:20-29). So every f...

#### Finding plugins-13: Command failures run from the palette or a shortcut go to the devtools console only, hiding the built-ins' actionable messages

- Verified status: confirmed; final severity: medium; rule: ux; feature: commands and shortcuts.
- Locations: editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-shell/src/palette/CommandPalette.tsx:58, editor/packages/editor-builtins/src/viewport/index.tsx:45-47, editor/packages/editor-builtins/src/inspector/index.tsx:33-41
- Evidence: Both keyboard paths end in `.catch((error) => console.error(`command "${id}" failed:`, error))` (shortcuts.ts:107, CommandPalette.tsx:58). The built-ins throw 31 user-facing guidance errors from commands, for example 'pause the game to nudge the selection' (viewport/index.tsx:47), 'select an entity first' and 'focus a component in the inspector first' (inspector/index.tsx:35-41). From the palette or a shortcut, which is the keyboard path the spec promises for every action, the user sees nothing happen. Buttons inside panels do show the same errors inline (InspectorPanel Section `run`). A shortcut bound to an unregistered command (`shortcuts.register` does not check the command exists) fails the same invisible way. No palette test covers failure.
- Reachability: Any user running a command from the palette or a shortcut in a state where it refuses.
- Recommendation: Make `editor.commands.run` failures from the palette and shortcuts visible. Log them to the Console panel through the public console API (see the side channel finding), with the command title: `Nudge the selection left: pause the game to nudge the selection`. Optionally show a short inline notice in the palette. Warn at registration when a shortcut's command is unknown, and once more when it becomes registered.
- First test suggested by the auditor: editor-shell test/palette.test.tsx `a_command_that_fails_from_the_palette_reports_its_message_to_the_user`: register a command throwing 'select an entity first', run it with Enter, and expect the message in the console sink (or the palette notice) rather than only in console.error.
- Verifier notes (binding scope corrections): Both keyboard paths send failures to the devtools console only. The shortcut path ends at shortcuts.ts:107 `.catch((error) => console.error(...))`, and the palette path at CommandPalette.tsx:58. No global handler routes them to the Console panel: there is no console.error patch, unhandledrejection or onerror handler in the editor packages or apps. Built-in commands throw user guidance, for example...

#### Finding ts-quality-5: Failures of commands run from the palette, shortcuts, and viewport gestures reach only the devtools console

- Verified status: confirmed; final severity: high; rule: ux; feature: commands / error feedback.
- Locations: editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-shell/src/palette/CommandPalette.tsx:58, editor/packages/editor-builtins/src/shared/index.ts:98, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:39, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:180-254, editor/packages/editor-builtins/src/divergence/CausalReport.tsx:16, editor/packages/editor-builtins/src/divergence/TraceViewer.tsx:44, editor/packages/editor-builtins/src/divergence/index.tsx:45, editor/packages/editor-builtins/src/input/capture.ts:31, editor/packages/editor-builtins/src/scene-tree/index.tsx:24-35
- Evidence: Every command run from a shortcut or from the palette ends in `.catch((error) => console.error(...))`. The viewport reports picking, moving, despawning, and nudging failures through `report()`, which is console.error. Seeks from the causal report and the trace strip do the same. Panel buttons show their errors; keyboard paths do not.

Example: Mod+S runs scene.save, which throws 'X is a replay: its file is never overwritten' on a replay (scene-tree/index.tsx:30) and rethrows any write_text failure (permission, disk full). The user gets no message, only an unsaved dot that stays in the asset browser. Dragging an entity while resimulating fails assertEditable and logs only to devtools.
- Reachability: Ordinary keyboard-driven use: Mod+S, Delete, arrow nudges, Mod+Z, and any palette command that fails. The design's anti-goal is anything reachable only by mouse, yet only the mouse paths show their errors.
- Recommendation: Route command failures to an editor-visible surface: the Console sink through a public editor.console, plus a transient role='alert' status line in the shell naming the command title and the error. Use one helper (for example editor.commands.runReported(id, ...args)) in the palette, shortcuts, showPanel, and the viewport/divergence handlers instead of console.error.
- First test suggested by the auditor: editor/packages/editor-shell/test/palette.test.tsx: a_failing_command_run_from_a_shortcut_shows_its_title_and_error_in_the_editor (register a command that throws, press its key, expect an alert containing the title and the message).
- Verifier notes (binding scope corrections): Shortcut failures go only to console.error (shortcuts.ts:107), and so do palette failures (CommandPalette.tsx:58), showPanel (shared/index.ts:98) and every viewport gesture (report at ViewportPanel.tsx:39, used at 94,115,180,212,221,247,254). Neither app forwards console.error to the Console panel (apps/web/src/app.ts:29-34). scene.save throws on a replay (scene-tree/index.tsx:30), throws 'no game...

#### Finding ux-live-2: Commands that fail from the palette or a shortcut give the user no feedback

- Verified status: confirmed; final severity: high; rule: errors; feature: command palette / shortcuts.
- Locations: editor/packages/editor-shell/src/palette/CommandPalette.tsx:62, editor/packages/editor-shell/src/shortcuts.ts:107
- Evidence: Both catch handlers only call `console.error` (browser devtools). Live: 'Create prefab from selection' with nothing selected did nothing visible; the only trace was the devtools line `command "prefabs.create-from-selection" failed: Error: select the entities of the new prefab first` (shots/34-command-fails-silently.png; browser-console.log). Panel buttons that run the same commands do show errors, so behaviour depends on the entry point.
- Reachability: Any palette command or keyboard shortcut whose precondition is not met (no selection, no game, replay open, editing locked during resimulation).
- Recommendation: Route command failures to the user: log them with `logToConsole(editor, 'error', ...)` and show a transient notice (a status region in the shell) naming the command and the error. Do it once in `editor.commands.run` callers in the shell (palette and shortcuts), not per command.
- First test suggested by the auditor: palette.test.tsx: register a command whose run rejects with 'select the entities first', run it from the palette, expect a role=status/alert element with that text (and a Console entry when the console plugin is active).
- Verifier notes (binding scope corrections): The palette (CommandPalette.tsx:58) and the shortcuts (shortcuts.ts:107) only call console.error. The Console panel does not capture console.error: console/index.tsx only forwards logToConsole calls to the devtools. Some commands can only be reached from the palette, and their precondition errors are never shown: 'Create prefab from selection' throws 'select the entities of the new prefab first' (...

#### Finding ambition-4: Command failures from the palette and from shortcuts reach only the devtools console

- Verified status: confirmed; final severity: medium; rule: ux; feature: editor shell (command palette, shortcuts).
- Locations: editor/packages/editor-shell/src/palette/CommandPalette.tsx:58, editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-builtins/src/inspector/index.tsx:33-41, editor/packages/editor-builtins/src/viewport/index.tsx:45-47, editor/packages/editor-builtins/src/timeline/editing.tsx:6, editor/packages/editor-builtins/src/divergence/index.tsx:152, editor/packages/editor-builtins/src/prefabs/index.tsx:151-177
- Evidence: Both dispatchers do `editor.commands.run(id).catch((error) => console.error(...))`. About 30 guidance errors thrown by built-in commands never reach the user when invoked this way, for example: 'select an entity first' and 'focus a component in the inspector first' (inspector/index.tsx:35,41), 'pause the game to nudge the selection' (viewport/index.tsx:47, bound to the arrow keys), 'the game is resimulating; edit once it finishes' (editing.tsx:6), 'choose a trace or snapshot from the asset browser' (divergence/index.tsx:152: the command always fails from the palette), and 'select the entities of the new prefab first' (prefabs/index.tsx:151). A few panels catch errors themselves (assets `onSelected`, scene tree Delete key), but the palette and shortcut paths do not. This contradicts 'every action is reachable from the command palette' and 'errors are precise and actionable'. palette.test.tsx has no failing-command test.
- Reachability: Any editor user running a command from the palette or a shortcut in a state where it fails (no selection, playing, resimulating, no game).
- Recommendation: Route every dispatcher failure through the public log introduced for the privileged-channel finding (`editor.log('error', ...)`), shown in the Console and as a transient status line near the palette. Make `divergence.open-file` usable without an argument (fall back to the asset browser's selection or a file picker).
- First test suggested by the auditor: editor/packages/editor-shell/test/palette.test.tsx: `a_failing_command_shows_its_error`. Register a command that throws 'select an entity first', run it from the palette, and expect the message in the shell's visible log or status, not only in a console.error spy.
- Verifier notes (binding scope corrections): Confirmed. shortcuts.ts:107 and CommandPalette.tsx:58 both do .catch(console.error), as does showPanel in shared/index.ts:98. palette.test.tsx has no failing-command test. Global shortcuts include Mod+S (scene save, which makes a failed write invisible: see ambition-1), Mod+Z / Mod+Shift+Z, '.', ',' and F5. divergence.open-file always throws when run from the palette (divergence/index.tsx:152). On...

#### Finding branches-11: Divergence UI failures go only to the browser console

- Verified status: confirmed; final severity: medium; rule: ux; feature: divergence report.
- Locations: editor/packages/editor-builtins/src/divergence/CausalReport.tsx:16, editor/packages/editor-builtins/src/divergence/CausalReport.tsx:26, editor/packages/editor-builtins/src/divergence/TraceViewer.tsx:44, editor/packages/editor-builtins/src/divergence/index.tsx:45, editor/packages/editor-builtins/src/divergence/index.tsx:63, editor/packages/editor-builtins/src/divergence/index.tsx:238
- Evidence: Selecting a report entry or a cause seeks the game. That seek fails while resimulating, or for an entity only in the compare branch, and the failure ends in console.error('divergence: seeking failed:'). The trace strip's seek, subscribing to the divergence and ghost topics, and fetching the ghost all go to the same console-only `report` helper. The user clicks and nothing happens. The timeline panel, by contrast, shows its errors (TimelinePanel.tsx:137).
- Reachability: Clicking an entry in the Divergence report or the trace strip while the game resimulates or the tick is out of range; a ghost fetch failure.
- Recommendation: Send these failures to the panels' visible error state (reportState.error, Viewer stores, and a lagging or error badge on the ghost overlay) and to the editor console through logToConsole, as the timeline does, with the failing action named ('Cannot seek to tick 501: ...').
- First test suggested by the auditor: vitest divergence.test.tsx: a_failed_seek_from_the_report_shows_an_alert. Make the fake control reject `seek`, click an entity entry, and assert a role=alert with the rejection text appears in the report panel.
- Verifier notes (binding scope corrections): Confirmed at every cited line. Seek failures from report entries (CausalReport.tsx:16 and :26) and from the trace strip (TraceViewer.tsx:44) end in console.error. So do the `report` helper (index.tsx:45), the subscribe failure (index.tsx:63) and the ghost fetch failure (index.tsx:238). Nothing forwards console.error into the editor console: the console plugin only writes to the browser console (co...

### Task 6: Pin the canonical hash of every serde shape

Backlog item DET-1 (do not cite this identifier in commits or code). Workstream: DET: Determinism enforcement and proof (grade B).

**Workstream context.**

Subsystem: state hash, type-level guards, golden traces and replays.

**Problem.** The guarantee is well enforced and proven, but some holes remain: the hash of several serde shapes is not pinned (mutants survive), the assets hash leaf ignores each asset's type, usize can enter the state, a shared &World can mutate the simulation from an observer or drawer, the goldens never reach the parallel colored solve, and most features (kinematic bodies, rounded polygons, five of seven edit kinds, nested prefabs, several players) have no golden.

**Target state.** Every serde shape pinned; the state hash covers types; the type system forbids pointer-width types and mutation outside systems; a stress golden and a feature corpus verified on three operating systems, WebAssembly and three browser engines.

**Item.**

- Priority P1, kind fix, effort S, golden impact none.
- Findings: test-strategy-1.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/tests/hash.rs (new)`, `crates/ludenea-ecs/src/hash.rs:208-528`, `.github/workflows/determinism.yml`.
- Acceptance: crates/ludenea-ecs/tests/hash.rs holds an all-shapes fixture with a pinned hash_value and a sensitivity test that changes each field in turn. cargo mutants on hash.rs leaves no surviving non-equivalent mutant, and CI runs it when hash.rs changes.
- First failing test: every_field_of_every_shape_changes_the_hash (fails today under the serialize_i32 returning Ok(()) mutant).

**Audit findings resolved by this task (full text).**

#### Finding test-strategy-1: Seven serde shapes can silently drop out of the canonical state hash: no test in the workspace notices

- Verified status: partially; final severity: medium; rule: tests; feature: state hash (ludenea-ecs).
- Locations: crates/ludenea-ecs/src/hash.rs:257, crates/ludenea-ecs/src/hash.rs:262, crates/ludenea-ecs/src/hash.rs:272, crates/ludenea-ecs/src/hash.rs:297, crates/ludenea-ecs/src/hash.rs:459, crates/ludenea-ecs/src/hash.rs:472, crates/ludenea-ecs/src/hash.rs:519, crates/ludenea-ecs/tests/snapshot.rs:273, crates/ludenea-ecs/tests/snapshot.rs:286, .claude/superpowers/specs/2026-10-17-ludenea-engine-design.md:196
- Evidence: cargo mutants on hash.rs, then the 21 survivors re-run with --test-workspace=true (23 minutes, scratchpad/mutants-hash/mutants.out): still MISSED workspace-wide are serialize_i16, serialize_i32, serialize_i128 and serialize_u128 replaced with Ok(()), plus SerializeTupleStruct, SerializeTupleVariant and SerializeStructVariant serialize_field replaced with Ok(()), and the HashError Display. With the crate's own tests, serialize_u8, serialize_none, the unit and newtype variants, newtype structs and map keys/values also survive. The only pinned hash value (`Known`, snapshot.rs:273-298) covers bool, i8, usize, f32, f64, String, Vec<u16>, Some and a tuple. The spec requires that equal hashes imply equal future behaviour.
- Reachability: Any game component or resource with an i16/i32/i128/u128 field, a tuple struct, or an enum with tuple or struct variants: a regression in the hasher would make a desync in that field invisible to replays, golden traces, divergence reports and future rollback netcode.
- Recommendation: Add an all-shapes fixture `Shapes` in crates/ludenea-ecs/tests/hash.rs. It derives Serialize with one field of every serde shape: u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, char, bool, f32, f64, String, bytes, None, Some, unit, unit struct, newtype struct, tuple struct with 2 fields, tuple, unit, newtype, tuple and struct variants, Vec and a BTreeMap with string and integer keys. Two tests: (1) a golden value of hash_value(&Shapes) pinned as a constant, which documents the cross-platform format; (2) a sensitivity property: changing any single field changes hash_value. Run cargo mutants on hash.rs in CI on changes to that file, or at least once per release.
- First test suggested by the auditor: crates/ludenea-ecs/tests/hash.rs: `every_field_of_every_shape_changes_the_hash` builds Shapes, mutates each field in turn (i32 field from 1 to 2, the struct variant's inner field, the tuple struct's second item) and asserts hash_value differs. It fails today under the serialize_i32 -> Ok(()) mutant.
- Verifier notes (binding scope corrections): The code and the mutation evidence hold. scratchpad/mutants-hash/mutants.out/missed.txt lists 8 survivors with --test-workspace: serialize_i16/i32/i128/u128 (hash.rs:258,263,273,298), the serialize_field of SerializeTupleStruct, SerializeTupleVariant and SerializeStructVariant (hash.rs:464,477,528), and HashError Display (hash.rs:208). The only pinned value, `Known` (tests/snapshot.rs:273-298), co...

### Task 7: Thread counts validated and pool creation fallible end to end

Backlog item RUN-1 (do not cite this identifier in commits or code). Workstream: RUN: Simulation loop, inputs and game-code isolation (grade C).

**Workstream context.**

Subsystem: ludenea-app simulation, inputs, parallel runtime; game panics across host, command line tool and worker.

**Problem.** Input validation, the pacer, the deterministic RNG and Parallel's ordering are solid, but an oversized thread count panics every tool, game code panics are caught call site by call site (an input mapping or a value codec kills the native game process, the command line tool reports a bare panic), live input is judged idle on raw thresholds (unmapped keys fork the future), edits on runtime-spawned entities can retarget another entity, the player count cannot be set, the tick contract is hand-written in nine loops, and the template never reads its input.

**Target state.** Pool creation fallible end to end; one panic boundary around game code with tick and system; the game's mapping defines idleness; runtime entities addressed stably; a Run type owns the logs and the tick contract; a logging API for game code.

**Item.**

- Priority P0, kind fix, effort S, golden impact none.
- Findings: runtime-4, replays-2, protocol-1, untrusted-input-1, determinism-2, test-strategy-2, errors-5, rust-quality-3, dx-live-17.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/parallel.rs:14-30`, `crates/ludenea-app/src/simulation.rs:264-267`, `crates/ludenea-cli/src/commands.rs:84-90`, `crates/ludenea-control/src/host.rs:371-374`, `crates/ludenea-control/src/protocol.rs:180-182`, `crates/ludenea-control/src/stdio.rs:29-32`.
- Acceptance: Parallel::try_new returns ParallelError for 0, for counts above a documented MAX_THREADS (for example 256) and for operating system refusals; Simulation::set_threads returns Result and keeps the previous pool. The command line tool validates --threads with a clap range and prints cannot start N threads: ...; use 1 to 256, exiting 2. SetThreads replies InvalidCommand and the session stays usable. serve_stdio falls back to one thread with a console warning. The panicking Parallel::new is removed. (The other APIs rust-quality-3 lists are covered by RUN-5 and SCN-3.)
- First failing test: crates/ludenea-control/tests/host.rs set_threads_beyond_the_limit_is_refused_and_the_session_goes_on, and crates/ludenea-cli/tests/cli.rs too_many_threads_is_refused.

**Audit findings resolved by this task (full text).**

#### Finding runtime-4: An oversized thread count panics: `--threads 9000` and a `set_threads` message kill the CLI and the native game

- Verified status: confirmed; final severity: high; rule: no-panic; feature: thread pool configuration.
- Locations: crates/ludenea-app/src/parallel.rs:14-30, crates/ludenea-app/src/simulation.rs:264-267, crates/ludenea-control/src/host.rs:371-374, crates/ludenea-control/src/protocol.rs:180-182, crates/ludenea-cli/src/commands.rs:84-90
- Evidence: `target/debug/boxes run examples/boxes/assets/boxes.scene.ron --ticks 1 --threads 9000 --hash` fails with "thread 'main' panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 9000 threads: Resource temporarily unavailable (os error 35)" and exit code 101 (on macOS, kern.num_taskthreads is 8192). Sending `{"type":"request","id":1,"command":{"type":"set_threads","threads":9000}}` to `boxes host --stdio` (simloop/threads.jsonl) panics the same way, and the process exits with 101. The protocol field is a u32 with no bound, and Parallel::new documents the panic but offers no fallible alternative.
- Reachability: (a) a typo in the CLI flag or a value in a CI matrix; any client of the stdio protocol (the desktop transport).
- Recommendation: Add `Parallel::try_new(threads) -> Result<Parallel, ThreadsError>`, and make `Simulation::set_threads` return `Result<(), SimError>` with a `Threads { requested, reason }` variant, keeping the previous pool when it fails. The host answers InvalidCommand with 'cannot start 9000 threads (Resource temporarily unavailable); ask for fewer, this machine has N cores'. The CLI prints the same message and exits with code 2.
- First test suggested by the auditor: crates/ludenea-control/tests/host.rs: `set_threads_beyond_the_system_limit_is_refused_and_keeps_the_pool`: SetThreads with u32::MAX returns an error, then a Step succeeds and the thread count has not changed.
- Verifier notes (binding scope corrections): Reproduced. `boxes run ... --threads 9000 --hash` panics at parallel.rs:27 with 'cannot start 9000 threads: Resource temporarily unavailable (os error 35)' (kern.num_taskthreads is 8192 on this machine). Host SetThreads (host.rs:371-374) passes the unbounded u32 straight to Simulation::set_threads, which calls Parallel::new and panics, and no fallible alternative exists. The editor never sends set...

#### Finding replays-2: Too many threads panics the CLI and the host (Parallel::new)

- Verified status: confirmed; final severity: high; rule: no-panic; feature: CLI arguments / control protocol SetThreads.
- Locations: crates/ludenea-app/src/parallel.rs:14-30, crates/ludenea-app/src/simulation.rs:264-267, crates/ludenea-cli/src/commands.rs:84-90, crates/ludenea-control/src/host.rs:371-374
- Evidence: `target/debug/boxes run empty.scene.ron --ticks 1 --threads 10000 --hash` prints "thread 'main' panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 10000 threads: Resource temporarily unavailable (os error 35)" and exits 101. Simulation::set_threads has no fallible variant, so `Command::SetThreads { threads }` from any protocol client (stdio, WebSocket, worker) reaches the same panic in the game host. `--threads 0` is accepted silently as 1.
- Reachability: Any CLI user passing a large --threads value (a typo, or `$(nproc)` multiplied in a script); any protocol client sending SetThreads with a large u32.
- Recommendation: Add `Parallel::try_new(threads) -> Result<Parallel, ParallelError>` and make `Simulation::set_threads` return `Result<(), SimError::Threads { requested, reason }>`, with a message like 'cannot start 10000 threads: ...; use at most <available_parallelism * k>'. Keep `Parallel::new` only for internal callers whose count is known to be valid, or drop it. Map the error to exit 2 in the CLI and to an InvalidCommand error reply in the host. Reject 0 with a clear message instead of silently clamping it.
- First test suggested by the auditor: crates/ludenea-cli/tests/cli.rs: `too_many_threads_is_refused`: cli(&dir, ["run", "{}/scene.ron", "--ticks", "1", "--threads", "100000"]) returns code 2 and output starting with 'error: cannot start 100000 threads'. Plus a ludenea-control host test: SetThreads { threads: 100_000 } gives an error reply, and the next GetHash still answers.
- Verifier notes (binding scope corrections): Reproduced: `boxes run empty.scene.ron --ticks 1 --threads 10000 --hash` panics at parallel.rs:27:45 ('cannot start 10000 threads: Resource temporarily unavailable (os error 35)') with exit 101. `--threads 0` is silently clamped to 1 (parallel.rs:19) and prints a hash. Simulation::set_threads (simulation.rs:264-267) has no fallible form. Host handles Command::SetThreads with a plain sim.set_thread...

#### Finding protocol-1: A thread count the OS cannot start panics the host process (SetThreads message and CLI --threads)

- Verified status: partially; final severity: high; rule: no-panic; feature: control protocol / host.
- Locations: crates/ludenea-control/src/host.rs:371-374, crates/ludenea-app/src/parallel.rs:18-29, crates/ludenea-app/src/simulation.rs:265-267, crates/ludenea-control/src/stdio.rs:29-32, crates/ludenea-cli/src/commands.rs:84-88
- Evidence: `Command::SetThreads { threads }` calls `sim.set_threads(threads as usize)`, which calls `Parallel::new`, which does `rayon::ThreadPoolBuilder::build().unwrap_or_else(|error| panic!("cannot start {threads} threads: {error}"))`. Reproduced: piping hello + `{"type":"request","id":1,"command":{"type":"set_threads","threads":50000}}` into `target/debug/boxes host --stdio` exits 101 with `panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 50000 threads: Resource temporarily unavailable (os error 35)`. Stdout is empty: even the Welcome computed in the same batch is lost. `boxes run examples/boxes/assets/boxes.scene.ron --ticks 1 --threads 50000` panics the same way (exit 101). `serve_stdio` also holds `.expect("SetThreads always succeeds")`, an invariant that is false.
- Reachability: Any protocol client (a third-party tool over stdio, a buggy plugin calling `editor.control.request({type: "set_threads", ...})`) and any CLI user who mistypes `--threads` (run, verify).
- Recommendation: Make the failure a value at its root: add `Parallel::try_new(threads) -> Result<Parallel, ParallelError>` (keep or remove `new`) and `Simulation::set_threads(&mut self, threads) -> Result<(), SimError>` with `SimError::Threads { requested, reason }`. On failure the previous pool stays. Host maps the error to `InvalidCommand` with a message such as "cannot start 50000 threads: Resource temporarily unavailable (os error 35); this machine has 12 cores, ask for at most that". The CLI prints it and exits 2. serve_stdio replaces its expect with a fallback to one thread plus a console warning.
- First test suggested by the auditor: tests/host.rs: `set_threads_beyond_what_the_machine_starts_is_an_invalid_command`. Request SetThreads { threads: 1_000_000 }, assert Err(InvalidCommand) whose message names the count, then assert `Step { ticks: 3 }` still succeeds. Add a cli test asserting that `run --threads 1000000` exits 2 with "cannot start".
- Verifier notes (binding scope corrections): I reproduced it. A hello followed by set_threads 50000, piped into target/debug/boxes host --stdio, exits 101 with the panic at crates/ludenea-app/src/parallel.rs:27 and nothing on stdout. `boxes run ... --threads 50000` also exits 101. The root cause is as described: host.rs:371-374 -> Simulation::set_threads (simulation.rs:265-267) -> Parallel::new, which panics. Parallel::new documents its pani...

#### Finding untrusted-input-1: A thread count the system cannot start panics the CLI, the stdio host, and the public API

- Verified status: confirmed; final severity: high; rule: no-panic; feature: parallelism (CLI --threads, SetThreads command, Simulation::set_threads).
- Locations: crates/ludenea-app/src/parallel.rs:18-31, crates/ludenea-app/src/simulation.rs:265, crates/ludenea-control/src/host.rs:371-374, crates/ludenea-cli/src/commands.rs:84-90, crates/ludenea-control/src/stdio.rs:29-32
- Evidence: Parallel::new builds a rayon pool with `.unwrap_or_else(|error| panic!("cannot start {threads} threads: {error}"))`. Reproduced: `target/debug/boxes run examples/boxes/assets/boxes.scene.ron --ticks 1 --threads 10000` spins for about 10 s at 860% CPU, then exits 101 with "panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 10000 threads: Resource temporarily unavailable (os error 35)" (macOS kern.num_taskthreads is 8192). Over the desktop transport, piping hello, get_hash, then `{"type":"request","id":2,"command":{"type":"set_threads","threads":9000}}` into `boxes host --stdio` kills the game with exit 101. Only the welcome was ever written: the get_hash reply in the same batch is lost too, because a batch is written only after every line in it is answered. SetThreads is a public protocol command (any plugin can send it through ControlClient.request), and `--threads` is a documented CLI flag.
- Reachability: CLI `run`/`verify --threads N` with N above the OS thread limit; any control client (project plugin, custom tool) sending SetThreads; game code calling Simulation::set_threads.
- Recommendation: Make pool creation fallible end to end: `Parallel::try_new(threads) -> Result<Parallel, ParallelError>`, `Simulation::set_threads -> Result<(), SimError>` with a variant naming the count and the OS error, a Host reply of InvalidCommand ("cannot start 9000 threads: ...; use at most N"), and CLI exit 2 with the same text. Check the range up front too (a clap value_parser range on --threads and the same bound in the host), because spawning thousands of threads before failing costs seconds.
- First test suggested by the auditor: In crates/ludenea-control/tests: `set_threads_the_system_cannot_start_is_refused`. Send Command::SetThreads { threads: u32::MAX } to a Host, assert Err with code InvalidCommand, then assert GetHash still answers. Add a CLI test asserting that `run --threads 4294967295` returns 2 with "cannot start" in its output.
- Verifier notes (binding scope corrections): Reproduced: `target/debug/boxes run examples/boxes/assets/boxes.scene.ron --ticks 1 --threads 10000` spins for 10.07 s at 860% CPU, then exits 101 with "panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 10000 threads: Resource temporarily unavailable (os error 35)". The code matches the finding. Parallel::new (parallel.rs:18-31) panics. Simulation::set_threads (simulation.rs:265) ...

#### Finding determinism-2: Parallel::new panics when the OS refuses the threads, reachable from `--threads` and from the SetThreads protocol message

- Verified status: confirmed; final severity: high; rule: no-panic; feature: thread pool / control protocol.
- Locations: crates/ludenea-app/src/parallel.rs:14-29, crates/ludenea-app/src/simulation.rs:264-267, crates/ludenea-cli/src/commands.rs:84-90, crates/ludenea-control/src/host.rs:371-374, crates/ludenea-control/src/protocol.rs:180-182
- Evidence: `target/debug/boxes run examples/boxes/assets/boxes.scene.ron --ticks 1 --threads 9000 --hash` prints "thread 'main' panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 9000 threads: Resource temporarily unavailable (os error 35)" (macOS kern.num_taskthreads = 8192). Over stdio, the lines {"type":"hello","protocol_version":3} then {"type":"request","id":1,"command":{"type":"set_threads","threads":9000}} make `boxes host --stdio` panic with the same message and no reply (input at scratchpad core-guarantee/threads.jsonl). SetThreads takes any u32 and is not inside the host's catch_unwind, so the desktop game process dies. The panic is documented (parallel.rs:15-16), but no fallible alternative exists.
- Reachability: (a) a CLI user passing a large --threads; any client of the control protocol (WebSocket, stdio, Tauri IPC) sending set_threads with a large value.
- Recommendation: Add `Parallel::try_new(threads) -> Result<Parallel, ParallelError>`. It rejects counts above a documented `Parallel::MAX_THREADS` (for example 256; results never depend on the count, so the cap costs nothing) and returns ThreadPoolBuilder::build errors instead of panicking. Make `Simulation::set_threads` return `Result<(), SimError>` and keep the previous pool on error. Make the CLI print "cannot start 9000 threads: ...; pass --threads between 1 and 256" and exit 2. Make the host reply ProtocolError with a dedicated ErrorCode::InvalidThreads, leaving the session usable. Delete the panicking `new`, so only one way remains.
- First test suggested by the auditor: crates/ludenea-cli/tests/cli.rs: `run_rejects_a_thread_count_above_the_maximum`: `run <scene> --ticks 1 --threads 100000` returns a non-zero code and the output names the maximum, without a panic. crates/ludenea-control/tests/host.rs: `set_threads_above_the_maximum_is_an_error_and_the_session_survives`: SetThreads { threads: u32::MAX } returns Err, and the next GetHash still replies.
- Verifier notes (binding scope corrections): Reproduced. `target/debug/boxes run ... --threads 9000 --hash` panics at parallel.rs:27:45 with 'cannot start 9000 threads: Resource temporarily unavailable (os error 35)' and exits 101. `boxes host --stdio` fed threads.jsonl panics the same way and never replies. Simulation::set_threads (simulation.rs:265) calls Parallel::new directly. Host::execute SetThreads (host.rs:371-374) is not wrapped in ...

#### Finding test-strategy-2: A large thread count panics the CLI and the game host (`--threads 1000000`, `SetThreads`)

- Verified status: confirmed; final severity: high; rule: no-panic; feature: simulation loop / control protocol / CLI.
- Locations: crates/ludenea-app/src/parallel.rs:18, crates/ludenea-app/src/parallel.rs:27, crates/ludenea-app/src/simulation.rs:265, crates/ludenea-control/src/host.rs:371, crates/ludenea-control/src/protocol.rs:180, crates/ludenea-cli/src/commands.rs:86
- Evidence: The protocol fuzz target panicked on a mutated `{"type":"set_threads","threads":18551616}` after 50 seconds of thread spawning: "cannot start 18551616 threads: Resource temporarily unavailable (os error 35) at crates/ludenea-app/src/parallel.rs:27" (input saved at scratchpad/sweep-fuzz/out/protocol/protocol-1689.bin). Reproduced with the built binary: `target/debug/boxes run examples/boxes/assets/boxes.scene.ron --ticks 5 --threads 1000000` exits 101 with the same panic (scratchpad/sweep-repro/threads.err). Parallel::new documents the panic but there is no fallible alternative, and SetThreads accepts any u32.
- Reachability: A CLI user who passes a large --threads value to run or verify; any control-protocol client over stdio (the desktop game binary, `host --stdio`) that sends SetThreads.
- Recommendation: Add `Parallel::try_new(threads) -> Result<Parallel, ThreadsError>` and make `Simulation::set_threads` return Result. Bound the count at the trust boundaries: CLI argument validation with a value parser and a stated maximum, for example 256 or available_parallelism times 4, and a ProtocolError InvalidCommand from SetThreads with the accepted range in the message. Keep the session running after a refused SetThreads.
- First test suggested by the auditor: crates/ludenea-control/tests/host.rs: `set_threads_beyond_the_limit_is_refused_and_the_session_goes_on` sends SetThreads { threads: u32::MAX }, asserts Err with ErrorCode::InvalidCommand naming the limit, then Step succeeds. Plus crates/ludenea-cli/tests/cli.rs: run --threads 1000000 exits 2 with `error: ...threads...`.
- Verifier notes (binding scope corrections): Parallel::new (parallel.rs:18-28) panics with `unwrap_or_else(panic!)` when the rayon pool cannot be built. The panic is documented, but there is no fallible variant. Simulation::set_threads (simulation.rs:265-267) passes the value through unchanged. Host handles Command::SetThreads with any u32 and does not check it (host.rs:371-374). The CLI's `threads: Option<u32>` goes straight to set_threads ...

#### Finding errors-5: `--threads` and SetThreads panic on a thread count the OS cannot provide; there is no fallible thread pool constructor

- Verified status: confirmed; final severity: high; rule: no-panic; feature: simulation loop, CLI, control protocol.
- Locations: crates/ludenea-app/src/parallel.rs:14-30, crates/ludenea-cli/src/commands.rs:84-90, crates/ludenea-control/src/host.rs:371-374
- Evidence: `boxes run solo/s3.scene.ron --ticks 1 --threads 20000` prints `thread 'main' panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 20000 threads: Resource temporarily unavailable (os error 35)` and exits 101. Command::SetThreads reaches the same `unwrap_or_else(|error| panic!(...))` (host.rs:372). Parallel::new documents the panic but offers no alternative.
- Reachability: A CLI user or CI script passing a large --threads, and any control protocol client sending SetThreads (the desktop host sends available_parallelism, which is safe, but the protocol accepts any u32).
- Recommendation: Add `Parallel::try_new(threads) -> Result<Parallel, ThreadPoolError>` and `Simulation::try_set_threads`. Keep `new` only if a documented panic is still wanted. The CLI validates `--threads` with a clap range (1..=1024) and maps a pool failure to `error: cannot start N threads: ...; use a smaller --threads` (exit 2). The host answers SetThreads with InvalidCommand naming the count and the OS error.
- First test suggested by the auditor: crates/ludenea-cli/tests/cli.rs: `an_unstartable_thread_count_is_an_error_not_a_panic`. Run `run <scene> --ticks 1 --threads 100000` and assert exit code 2 and output containing 'cannot start 100000 threads'. Add a host test that SetThreads { threads: u32::MAX } returns Err(InvalidCommand).
- Verifier notes (binding scope corrections): Reproduced: `boxes run spawner.scene.ron --ticks 1 --threads 20000` exits 101 with `thread 'main' panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 20000 threads: Resource temporarily unavailable (os error 35)`. Parallel::new documents the panic and has no fallible alternative (parallel.rs:14-30). Simulation::set_threads calls it directly (simulation.rs:265-267). The CLI passes --...

#### Finding rust-quality-3: Public APIs panic on values their types allow, with no fallible alternative: thread counts, player counts, unknown components

- Verified status: confirmed; final severity: high; rule: no-panic; feature: simulation runtime API and transports.
- Locations: crates/ludenea-app/src/parallel.rs:14-29, crates/ludenea-app/src/simulation.rs:264-267, crates/ludenea-app/src/simulation.rs:94-97, crates/ludenea-control/src/host.rs:371-374, crates/ludenea-control/src/stdio.rs:28-32, crates/ludenea-control/src/stdio.rs:83-88, crates/ludenea-cli/src/commands.rs:84-89, crates/ludenea-control/src/session.rs:258, crates/ludenea-control/src/session.rs:301, crates/ludenea-app/src/scene.rs:627-639
- Evidence: Thread counts: `Parallel::new` panics with "cannot start N threads" whenever rayon cannot build the pool. `Simulation::set_threads` does not document that panic, and both the host and the CLI pass a u32 straight through.
- Repro: sending `{"type":"set_threads","threads":9000}` after hello to target/debug/boxes host --stdio prints "panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 9000 threads: Resource temporarily unavailable (os error 35)" and exits 101 (/private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/rust-quality/threads.jsonl).
- `boxes verify golden/spawner.replay --threads 9000` panics the same way, instead of printing `error:` and exiting 2.
- At startup, `serve_stdio` runs `.expect("SetThreads always succeeds")` with `available_parallelism`. It fails the same way wherever the process thread limit is below the core count (containers).

Player counts: `Simulation::set_players(usize)` accepts 0 and values above 65535, while logs hold a u16 and `SimError::NoPlayers` forbids 0. `Session::start` and `Session::resume` then call `expect("fewer than 2^16 players")`.

Unknown components: `Simulation::override_value_ron` already returns a Result, yet panics on an unregistered component (its documented # Panics section).
- Reachability: (a) Any protocol client sending set_threads, and any CLI user passing --threads (run, verify). (b) Game or tool developers calling set_threads, set_players or override_value_ron with values their types allow.
- Recommendation: - Add `Parallel::try_new(threads) -> Result<Parallel, ParallelError>`, with an explicit upper bound (for example a MAX_THREADS constant) checked before building a pool.
- Make `Simulation::set_threads` fallible, taking a `NonZeroU32` and returning `Result<(), SimError>`.
- The host maps the error to InvalidCommand "cannot start 9000 threads: <os error>; ask for at most <limit>". The CLI prints `error: --threads 9000: …` and exits 2. serve_stdio falls back to 1 thread with a console warning.
- `set_players(NonZeroU16)`, which removes the expects and in-memory `NoPlayers` states.
- `override_value_ron` returns `SceneError::UnknownComponent` instead of panicking.
- First test suggested by the auditor: crates/ludenea-control/tests/host.rs: `SetThreads { threads: 100_000 }` returns Err(InvalidCommand) naming the limit, and a following GetHash still answers. Today the process panics. In crates/ludenea-cli/tests/cli.rs, `verify golden/spawner.replay --threads 100000` exits 2 with an `error:` line.
- Verifier notes (binding scope corrections): Reproduced. Piping threads.jsonl into `target/debug/boxes host --stdio` panics at parallel.rs:27 ("cannot start 9000 threads: Resource temporarily unavailable (os error 35)"). `boxes verify golden/spawner.replay --threads 9000` panics the same way and exits 101 instead of printing an error and exiting 2. `Parallel::new` (parallel.rs:18-29) documents the panic. `Simulation::set_threads` (simulation...

#### Finding dx-live-17: Thread counts are unvalidated: `--threads 9000` hangs; pool failure panics; SetThreads takes any u32

- Verified status: confirmed; final severity: medium; rule: no-panic; feature: parallel physics / CLI / protocol.
- Locations: crates/ludenea-app/src/parallel.rs:18-29, crates/ludenea-cli/src/commands.rs:84-90, crates/ludenea-control/src/host.rs:371-374, crates/ludenea-control/src/protocol.rs:180
- Evidence: - `game run scene --ticks 2 --threads 9000` (above macOS's 8192 threads per task) printed nothing and was still running when killed after 20 s (exit 142).
- `--threads 100000` ran at 700% CPU with 450 GB of virtual memory until killed.
- `--threads 0` is silently treated as 1.
- `Parallel::new` panics when the pool cannot start ("cannot start {threads} threads"), and the protocol's `SetThreads { threads }` reaches it with any client value.
- Reachability: A CLI typo, or any control-protocol client.
- Recommendation: Clamp or reject at the boundary: parse `--threads` with a clap range of 1 to 256 (or `available_parallelism() * 4`), and make `Command::SetThreads` return `InvalidCommand` outside that range. Make `Parallel::new` fallible (`Result<Parallel, ParallelError>`) so pool creation failure becomes an error, not a panic.
- First test suggested by the auditor: In ludenea-cli tests, `run scene --ticks 1 --threads 0` and `--threads 100000` exit 2 quickly with an error naming the accepted range. In a ludenea-control test, `SetThreads { threads: u32::MAX }` returns an error and leaves the host usable.
- Verifier notes (binding scope corrections): Reproduced with target/debug/boxes (kern.num_taskthreads is 8192). `run spawner.scene.ron --ticks 2 --hash --threads 9000` printed nothing for 25 s in one run (killed, exit 142). In a second run it panicked within 18 s: "thread 'main' panicked at crates/ludenea-app/src/parallel.rs:27:45: cannot start 9000 threads: Resource temporarily unavailable (os error 35)". So it stalls while spawning threads...

### Task 8: Game and cargo output read as bytes

Backlog item CTL-3 (do not cite this identifier in commits or code). Workstream: CTL: Control protocol, host and transports (grade C).

**Workstream context.**

Subsystem: ludenea-control host and transports, control-client, game process supervision.

**Problem.** The host resists hostile commands and the wire format is pinned, but a native game that exits or hangs is never reported (requests hang, the queue jams, only a rebuild restarts it), one non-UTF-8 byte on the game's output cuts the control channel, one throwing listener drops other responses, a lost connection to ludenea serve is never shown or recovered, a second Hello inherits stale state, and Host::execute is a 571-line match with topic state in four places.

**Target state.** Every game exit or hang is reported and recovered from the last export; output is read as bytes; listeners are isolated; the project connection reconnects; Host is split by area with one owner for topic state.

**Item.**

- Priority P0, kind fix, effort S, golden impact none.
- Findings: protocol-5, desktop-7, untrusted-input-3, readiness-12.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/game.rs:50-65`, `crates/ludenea-project/src/build.rs:139`, `crates/ludenea-project/src/service.rs:563-584`.
- Acceptance: Both game streams and cargo's stderr are split on newline bytes, a trailing carriage return trimmed, and converted with String::from_utf8_lossy; only an I/O error ends a relay; a non-UTF-8 line is never taken for a protocol message.
- First failing test: game.rs a_line_that_is_not_utf8_is_output_and_protocol_lines_still_arrive (stdout and stderr cases).

**Audit findings resolved by this task (full text).**

#### Finding protocol-5: One non-UTF-8 line that a game prints makes the project service stop reading the protocol; the game then dies of a broken pipe with no reason given

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: stdio transport / game process output.
- Locations: crates/ludenea-project/src/game.rs:50-65, crates/ludenea-project/src/service.rs:565-583, crates/ludenea-control/src/stdio.rs:74-75
- Evidence: Both readers use `BufReader::new(stdout).lines().map_while(Result::ok)`. A line that is not UTF-8 is an `Err(InvalidData)`, so the loop stops and the pipe is dropped. Reproduced with a scratch game that writes `b"caf\xe9\n"` at tick 2, run through the real `GameProcess` (probe: /private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/control/panicgame/src/bin/probe.rs). Results: "protocol lines received: 0" (even the Welcome, which followed the printed line), "message channel: disconnected", later sends fail with BrokenPipe, and the game exits with status 2 because serve_stdio's write fails. Without the garbage line, all 8 messages arrive. The editor only shows "Game exited with code 2." Non-UTF-8 on stderr likewise stops forwarding, and the game's next `eprintln!` panics on the closed pipe.
- Reachability: Desktop editor: a game or a C dependency that prints Latin-1 text, raw bytes, or a truncated multi-byte sequence on stdout or stderr.
- Recommendation: Read both streams as bytes until EOF (`BufRead::split(b'\n')`). Protocol lines are always UTF-8, since the host writes serde_json. Convert printed lines with `String::from_utf8_lossy`, and never stop reading before EOF, so a stray byte can never close the pipe.
- First test suggested by the auditor: crates/ludenea-project/src/game.rs: `a_line_that_is_not_utf8_is_output_and_protocol_lines_still_arrive`. A fake game script runs `printf 'caf\351\n{"type":"event","event":{"type":"resimulated","up_to":3}}\n'`. Expect a GameOutput line "caf\u{FFFD}" and the protocol message on the messages receiver.
- Verifier notes (binding scope corrections): I confirmed it by reading the code. game.rs:51 and game.rs:62 use BufReader::lines().map_while(Result::ok). lines() returns InvalidData for a line that is not UTF-8, so the reader thread ends and drops the pipe. The game's next stdout write then fails: serve_stdio returns Err (stdio.rs:74-75) and the game exits with code 2. On stderr, eprintln! panics once the pipe is closed. The project already k...

#### Finding desktop-7: Game output that is not UTF-8 ends the relay, announces a false exit, and makes the game panic on its next print

- Verified status: confirmed; final severity: medium; rule: no-panic; feature: desktop game supervision.
- Locations: crates/ludenea-project/src/game.rs:51, crates/ludenea-project/src/game.rs:62, crates/ludenea-project/src/service.rs:564-584, crates/ludenea-project/src/build.rs:139, crates/ludenea-control/src/stdio.rs:35-41
- Evidence: The stdout and stderr readers use `BufReader::lines().map_while(Result::ok)` (game.rs:51, 62). `lines()` returns an InvalidData error for a line that is not UTF-8, and map_while stops at the first error. The reader thread then exits and closes its end of the pipe. On stdout, the message channel disconnects, and after 100 polls the watcher announces `GameExited { code: None }` (service.rs:581-583) while the game still runs. The editor loses every later protocol message. The game's next write gets EPIPE, because Rust ignores SIGPIPE, so a later `println!` or `eprintln!` in game code panics with 'failed printing to stdout/stderr'. The host side already avoids this exact trap: it reads bytes, with the comment 'a line that is not UTF-8 gets an error instead of ending input' (stdio.rs:35-41). Cargo's stderr reader has the same pattern (build.rs:139).
- Reachability: A game, or a native library it links, that writes bytes that are not UTF-8 to stdout or stderr (raw bytes via write_all, or a C dependency printing in a legacy code page).
- Recommendation: Read both streams with `split(b'\n')`, trim a trailing '\r', and convert with String::from_utf8_lossy before split_stdout_line and forward. Stop only on an I/O error, not on bad encoding. Apply the same change to the cargo stderr reader in build.rs.
- First test suggested by the auditor: Unit test in game.rs (unix): a fake game `printf '\377\n{"type":"pong"}\n'; read -r line`. Expect a GameOutput line containing U+FFFD, then the `{"type":"pong"}` message on take_messages(), and no disconnect while the game runs
- Verifier notes (binding scope corrections): The finding holds as stated. game.rs:51 and :62 use `BufReader::lines().map_while(Result::ok)`. lines() yields an InvalidData error for a line that is not UTF-8, and map_while stops at the first error. The reader thread then drops ChildStdout or ChildStderr, which closes the pipe. On stdout, message_sender is dropped, so `for line in messages` ends (service.rs:566-568). After 100 polls of 10 ms th...

#### Finding untrusted-input-3: One non-UTF-8 byte on the game's stdout or stderr silently cuts the desktop control channel

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: desktop game process relay.
- Locations: crates/ludenea-project/src/game.rs:51, crates/ludenea-project/src/game.rs:62, crates/ludenea-project/src/service.rs:565-585, crates/ludenea-project/src/build.rs:139
- Evidence: Both relay threads read with `BufReader::new(stdout).lines().map_while(Result::ok)`. `lines()` yields Err(InvalidData) on a non-UTF-8 line, and map_while ends the loop for good. Demonstrated with the same loop compiled on its own (scratchpad sweep/lines.rs): the input `loading\n\xffbinary\n{"type":"response","id":1}\nmore\n` prints only `got "loading"`, then stops. On stdout, the messages channel disconnects, so relay() at service.rs:565 assumes the game is exiting: it polls for 1 s, then broadcasts GameExited { code: None } while the process is still running. Every later reply is dropped, so editor requests never resolve. Once the unread pipe fills (64 KiB), the game blocks in its next print. On stderr, the game blocks once it has written another pipe buffer. The stdio host already reads bytes for exactly this reason (stdio.rs:34: "Bytes, not text: a line that is not UTF-8 gets an error instead of ending input"); the direction from the game to the editor does not. build.rs:139 uses the same pattern for cargo's stderr.
- Reachability: A desktop game, or one of its native dependencies, printing raw bytes or text in a legacy code page (common with C libraries on Windows), through println!, eprintln!, or write_all.
- Recommendation: Read lines as bytes (`BufRead::split(b'\n')` or `read_until`) and convert them with `String::from_utf8_lossy`. A bad line then shows as console output with replacement characters, and reading goes on; only an I/O error ends the relay. Apply this to both streams in game.rs and to build.rs:139. Never take a non-UTF-8 line for a protocol message.
- First test suggested by the auditor: In crates/ludenea-project/src/game.rs tests (unix): start a fake game script `printf '\377\n{"type":"x"}\n'; sleep 5` with GameProcess::start. Assert that take_messages() yields `{"type":"x"}` and that take_output() yields a Stdout line containing U+FFFD. Write a second case for stderr, followed by more than 64 KiB of output, to prove the game does not block.
- Verifier notes (binding scope corrections): Code as described: game.rs:51 and :62, and build.rs:139, use lines().map_while(Result::ok). The stdio direction deliberately reads bytes (stdio.rs:34-36). No test covers non-UTF-8 output. One correction to the mechanism. When the loop ends, the Lines iterator is dropped and with it the ChildStdout or ChildStderr, so the pipe is closed. The game does not block once the buffer fills; its next write ...

#### Finding readiness-12: One non-UTF-8 line from the game or from cargo ends the relay of that stream

- Verified status: confirmed; final severity: low; rule: no-panic; feature: desktop game process and builds.
- Locations: crates/ludenea-project/src/game.rs:51, crates/ludenea-project/src/game.rs:62, crates/ludenea-project/src/build.rs:139, crates/ludenea-control/src/stdio.rs:35-37
- Evidence: The relays read with BufReader::lines().map_while(Result::ok), which stops at the first line that is not UTF-8 and drops the pipe's read end. On stderr, the game's next eprintln! then hits a broken pipe and panics with 'failed printing to stderr'. On stdout, the protocol messages stop, the editor announces GameExited while the process still runs, and the host exits on its next write. The stdio host already reads bytes for exactly this reason (stdio.rs:35-37).
- Reachability: A desktop game, or one of its native dependencies, writing raw bytes to stdout or stderr.
- Recommendation: In the three relays, read with split(b'\n') and convert with String::from_utf8_lossy.
- First test suggested by the auditor: A unix service test with a script game that prints byte 0xFF on stderr and on stdout, then answers a line. Assert that the protocol line still arrives and that the output is relayed with a replacement character.
- Verifier notes (binding scope corrections): All three relays use BufReader::lines().map_while(Result::ok): the game's stdout and stderr at crates/ludenea-project/src/game.rs:51 and :62, and cargo's stderr at crates/ludenea-project/src/build.rs:139. The first non-UTF-8 line yields an InvalidData error, which ends the thread and drops the read end of the pipe. After that, the game's eprintln!/println! hit a broken pipe (Rust ignores SIGPIPE, ...

### Task 9: A Pose rotation is always unit length, and checkpoint restore never panics

Backlog item PHY-1 (do not cite this identifier in commits or code). Workstream: PHY: Math and physics (grade C).

**Workstream context.**

Subsystem: ludenea-math, ludenea-physics, physics editor plugin.

**Problem.** The narrow phase, the solver's thread independence and the coverage are excellent, but finite inputs still crash the editor host (a zero rotation the snapshot decoder rejects; NaN settings; huge densities and substep counts), public constructors build degenerate shapes, a collider without a Body is drawn and raycast as ground but ignored by the solver, polygons and rotations cannot be authored or edited, the physics panel floods the host with one request per body per tick, and deep stacks and large mass ratios fail.

**Target state.** Finite in, finite out, by validation at every boundary; one definition of what takes part in physics; polygons authored from their vertices with physics inspectors; the panel reads in bulk; stacking scenarios covered by regression tests.

**Item.**

- Priority P0, kind fix, effort M, golden impact possible.
- Findings: physics-1.
- Depends on: nothing.
- Locations and files: `crates/ludenea-math/src/rot.rs:62-80`, `crates/ludenea-math/src/scalar.rs:51-73`, `crates/ludenea-physics/src/solver.rs:262-289`, `crates/ludenea-physics/src/step.rs:217-237`, `crates/ludenea-physics/src/components.rs:35-52`, `crates/ludenea-control/src/session.rs:1692`.
- Acceptance: Every moving body's per-substep rotation is clamped, kinematic bodies included. Rot::normalize and sin_cos return IDENTITY for a zero or non-finite magnitude (documented). The post-step check reuses the Pose validation and names the entity and field. The session turns the checkpoint expect at session.rs:1692 into a SessionError that keeps the decode reason.
- First failing test: crates/ludenea-physics/tests/pipeline.rs kinematic_spin_keeps_a_unit_rotation_and_restores (angular velocity 1e30, one step, unit rotation, the snapshot restores into a fresh simulation), plus a ludenea-control session test that seeks back afterwards without a panic.

**Audit findings resolved by this task (full text).**

#### Finding physics-1: Physics can store a zero rotation that its own snapshot decoder rejects, and the editor host then panics on the next seek

- Verified status: confirmed; final severity: critical; rule: no-panic; feature: physics step / math rotations / snapshots.
- Locations: crates/ludenea-math/src/rot.rs:62-80, crates/ludenea-math/src/scalar.rs:51-73, crates/ludenea-physics/src/solver.rs:283-289, crates/ludenea-physics/src/step.rs:217-237, crates/ludenea-physics/src/components.rs:39-55, crates/ludenea-control/src/session.rs:1689-1692
- Evidence: Kinematic bodies' angular velocity is never clamped (integrate_velocity returns early for non-dynamic bodies; integrate_position calls Rot::integrate(h*w)). For |w| above about 4.4e21, sin² overflows. Rot::normalize then returns (0, 0) because inv = 0 when mag is not greater than 0, and also when mag is infinite. Probe result: "kinematic w=1e30 rotation after 1 tick: Rot { cos: 0.0, sin: 0.0 }". The post-step check only tests is_finite, so (0, 0) passes. Pose deserialization requires cos²+sin²=1, so restoring fails with "cannot decode `ludenea::Pose` from the snapshot: Serde Deserialization Error" (the reason is lost). Over stdio with boxes host --stdio: load scratchpad/physics/kin_spin.scene.ron (angular_velocity 1e30, which passes scene validation), step 40, seek 20. Result: "panicked at crates/ludenea-control/src/session.rs:1692:14: a checkpoint restores into the world it was taken from: Decode { name: \"ludenea::Pose\" ... }", exit 101. Game code reaches the same state through Rot::from_angle(NaN | inf | 1e30), which returns (0, 0) (sin_cos falls back to (0.0, 0.0) at scalar.rs:71).
- Reachability: (a) A scene or inspector edit giving a kinematic body a large finite angular velocity, then scrubbing the timeline. (b) Game code calling Rot::from_angle with a non-finite or huge angle, or writing Rot fields directly.
- Recommendation: Enforce one invariant: a Pose rotation is always unit length. (1) Clamp the per-substep rotation of every moving body, kinematic included, as integrate_velocity already does for dynamic ones (0.25*PI*inv_h). (2) Make Rot::normalize and sin_cos never return a non-unit rotation: return Rot::IDENTITY when the magnitude is zero or not finite, and document it. (3) Have the post-step check reuse the decoder's validation (move the Pose check into a Pose::validate used by both) and name the entity and the offending field. (4) In ludenea-control, turn the checkpoint `expect` into a SessionError, and keep the decode reason instead of postcard's generic text.
- First test suggested by the auditor: crates/ludenea-physics/tests/pipeline.rs: kinematic_spin_keeps_a_unit_rotation_and_restores. A kinematic box with angular_velocity 1e30; step once; assert |cos²+sin²-1| < 1e-4; restore simulation.snapshot() into a fresh simulation and expect Ok. Plus a ludenea-control session test: load that scene, step 40, seek 20, expect no panic.
- Verifier notes (binding scope corrections): Reproduced exactly. `boxes host --stdio` < scratchpad/physics/host_in.jsonl (kinematic angular_velocity 1e30, then step 40, then seek 20) exits 101 with "panicked at crates/ludenea-control/src/session.rs:1692:14: a checkpoint restores into the world it was taken from: Decode { name: "ludenea::Pose", reason: "Serde Deserialization Error" }". Here is the chain. solver.rs:262-265 returns early for an...

### Task 10: Every physics input validated so the post-step assert is a true invariant

Backlog item PHY-2 (do not cite this identifier in commits or code). Workstream: PHY: Math and physics (grade C).

**Workstream context.**

Subsystem: ludenea-math, ludenea-physics, physics editor plugin.

**Problem.** The narrow phase, the solver's thread independence and the coverage are excellent, but finite inputs still crash the editor host (a zero rotation the snapshot decoder rejects; NaN settings; huge densities and substep counts), public constructors build degenerate shapes, a collider without a Body is drawn and raycast as ground but ignored by the solver, polygons and rotations cannot be authored or edited, the physics panel floods the host with one request per body per tick, and deep stacks and large mass ratios fail.

**Target state.** Finite in, finite out, by validation at every boundary; one definition of what takes part in physics; polygons authored from their vertices with physics inspectors; the panel reads in bulk; stacking scenarios covered by regression tests.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: physics-2.
- Depends on: nothing.
- Locations and files: `crates/ludenea-physics/src/components.rs:221-243`, `crates/ludenea-physics/src/step.rs:224-237`, `crates/ludenea-physics/src/solver.rs:105-152`, `crates/ludenea-app/src/time.rs`.
- Acceptance: PhysicsSettings has a try_from mirror (finite gravity; substeps in a documented range such as 1 to 64; finite, non-negative hertz, damping, push velocity and thresholds). Time.dt is validated finite and positive. Kinematic linear speed is clamped like dynamic speed. Density and gravity times gravity_scale are capped with precise errors. The post-step message names the entity, the field and the step inputs. set_field of substeps 400000000 is refused instead of hanging the host.
- First failing test: crates/ludenea-physics/tests/pipeline.rs scene_with_nan_gravity_is_rejected, scene_with_huge_substeps_is_rejected and kinematic_body_at_huge_finite_speed_stays_finite.

**Audit findings resolved by this task (full text).**

#### Finding physics-2: Unvalidated or huge inputs reach the physics step and trip a release-mode assert! panic or a hang

- Verified status: partially; final severity: critical; rule: no-panic; feature: physics step / PhysicsSettings / scene loading.
- Locations: crates/ludenea-physics/src/components.rs:221-243, crates/ludenea-physics/src/step.rs:224-237, crates/ludenea-physics/src/solver.rs:105-130, crates/ludenea-physics/src/solver.rs:152, crates/ludenea-physics/src/solver.rs:262-289, crates/ludenea-physics/src/components.rs:100-121, crates/ludenea-physics/src/components.rs:202-217
- Evidence: PhysicsSettings derives Deserialize with no try_from, and Time.dt is not validated either. Scenes carry both as resources, and the inspector edits resources through set_field. `boxes run` on scenes generated in scratchpad/physics/ panics with "physics produced a non-finite state for entity 1v0" at step.rs:234 for each of these: gravity (NaN, -10); dt NaN; a kinematic body with linear velocity (3e38, 0), which is finite and accepted but overflows to inf; collider density 3e38, which is accepted; gravity -1e38 with gravity_scale 3e38. substeps 4000000000 in a scene, or 400000000 set through the host's set_field on ludenea::PhysicsSettings, never finishes one tick (killed after 15 to 20 s). Negative contact_hertz and substeps 0 are accepted silently. The panic message does not name the cause (gravity, dt or velocity). The spec says finiteness is asserted in debug builds; the code uses assert! in release too. In the browser the game worker aborts on it.
- Reachability: (a) Any malformed scene file, or a typo in the inspector's PhysicsSettings or Time fields. (b) Game code writing the public fields.
- Recommendation: Validate every boundary so the assert becomes a true internal invariant. Give PhysicsSettings a serde try_from mirror like Body: finite gravity; substeps within a documented range, e.g. 1..=64; contact_hertz, damping, push velocity and restitution threshold finite and >= 0. Validate Time.dt as finite and > 0 in ludenea-app. Bound finite inputs so finite in gives finite out: clamp kinematic linear speed like dynamic bodies, cap density and gravity*gravity_scale at load with a documented maximum, and reject larger values with a precise error. Keep the post-step check, but make its message name the entity, the non-finite field, and the step inputs (dt, gravity, velocity) so it is actionable.
- First test suggested by the auditor: crates/ludenea-physics/tests/pipeline.rs: scene_with_nan_gravity_is_rejected and scene_with_huge_substeps_is_rejected. Load ONE_BODY_SCENE with resources {"ludenea::PhysicsSettings": (... gravity:(x:NaN ...))} and expect a SceneError naming ludenea::PhysicsSettings. Plus kinematic_body_at_huge_finite_speed_stays_finite_or_is_rejected.
- Verifier notes (binding scope corrections): The core holds. I reproduced `boxes run` panicking at step.rs:234 ("non-finite state for entity 1v0", exit 101) for nan_gravity, nan_dt, gs_huge, kin_fast (after more than 5 ticks) and huge_density (after more than 5 ticks). A set_field of substeps=400000000 hung the host for more than 12 s without answering. PhysicsSettings (components.rs:221-230) has no validation. A scene's Time.dt is honored o...

### Task 11: Undo history and branch restore keep the tree's invariants

Backlog item SES-1 (do not cite this identifier in commits or code). Workstream: SES: Editing sessions, undo and redo, timeline (grade C).

**Workstream context.**

Subsystem: ludenea-control session, undo history, checkpoints, timeline.

**Problem.** Session invariants are documented, coverage is high and panics in game code are isolated, but a malformed undo history panics the host or exhausts memory, branch identifiers can be reused into a cycle, redo can rebuild a branch from a stale parent (false recorded hashes) or silently discard play recorded since the undo, restore and edit logs are unbounded, the undo history stores two full scene texts per tick-0 edit, scrubbing far behind the head replays thousands of ticks per step, the host rebuilds the timeline several times per frame, every tick-0 edit is blamed on the game code, and session.rs is a 2,249-line god object.

**Target state.** The undo history and branch tree validate on resume and keep their invariants; redo never loses or falsifies recorded work; work per command is bounded; storage and seek cost follow the change, not the run; Session is split by concern under a seeded sequence test.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: sessions-1, sessions-3, branches-5.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/branches.rs:88-95,201-235,291-305,446-474`, `crates/ludenea-control/src/session.rs:289-334,648-661,1310-1314`, `crates/ludenea-control/src/undo.rs:211-234`.
- Acceptance: put_back raises next above every restored identifier and runs the decode fork checks. resume drops, with a load warning, entries naming an unknown branch or the root as a fork, and removed branches whose fork fails check_forks. The expect at session.rs:654 becomes SessionError::NoSuchBranch, handled by is_dropped. An undecodable history resumes empty with a warning. fork() debug-asserts that the identifier is new.
- First failing test: tests/undo.rs a_fork_entry_naming_the_root_is_dropped_not_a_panic, a_redo_entry_forking_past_its_parent_is_refused, and a_redone_fork_after_a_resume_keeps_branch_identifiers_unique.

**Audit findings resolved by this task (full text).**

#### Finding sessions-1: Branch identifiers are reused after a session round trip and a redo: the tree gets a cycle, the host hangs and runs out of memory

- Verified status: partially; final severity: high; rule: feature-robustness; feature: branches / undo and redo.
- Locations: crates/ludenea-control/src/branches.rs:201-225, crates/ludenea-control/src/branches.rs:291-305, crates/ludenea-control/src/branches.rs:228-235, crates/ludenea-control/src/session.rs:1310-1314, crates/ludenea-control/src/session.rs:323-325, crates/ludenea-control/src/session.rs:709-719
- Evidence: `fork()` takes its new id from `self.next` (branches.rs:207). `export()` writes no tree for a linear session (session.rs:1310-1314), so `resume` starts from `BranchTree::default()` with `next = 1`. A redo of a Fork entry `put_back`s branch 1 without moving `next` (branches.rs:291-305). The next fork then inserts a child with id 1 over branch 1, and the child's parent is branch 1 itself. After that, `path()` loops forever and keeps allocating (branches.rs:228-235). The host calls `timeline()` on every command and every frame (host.rs:136, 193). Reproduction (scratchpad/editing-audit/probe/tests/branch_id_reuse.rs): edit at tick 50 (forks), undo, export, resume, redo, seek 20, edit. Output: `after a second fork: [(BranchId(0), None), (BranchId(1), Some(BranchId(1)))]`, then `timeline() did not answer within 3 s`. The randomized probe hit it in 3 of 500 seeds, and an unguarded run was SIGKILLed for memory. The original branch 1 and its recorded future are overwritten without notice.
- Reachability: Editor: edit at a past tick (it forks), undo, then anything that exports and reloads the session (switching documents and back, a prefab or data asset change on disk that triggers reloadAssets, crash recovery), then redo, then edit at a past tick again. The game process or worker hangs.
- Recommendation: Make 'identifiers are never reused' hold whatever the history brings back: in `BranchTree::put_back`, set `self.next = self.next.max(id.0 + 1)` for every restored branch. Optionally make `resume` raise `next` above every id the decoded undo history names. Add a debug assertion in `fork()` that the id is not present.
- First test suggested by the auditor: tests/undo.rs: a_redone_fork_after_a_resume_keeps_branch_identifiers_unique (edit at 50, undo, export, resume, redo, seek 20, edit; assert every branch's parent id is lower than its own and the two branches are distinct)
- Verifier notes (binding scope corrections): The bug is real, but the claim that the editor reaches it is wrong. Code and reproduction hold: fork() takes self.next (branches.rs:207). put_back() never raises next (branches.rs:291-305). export() writes no tree when is_branched() is false (session.rs:1310-1314). The probe tests/branch_id_reuse.rs reproduces the cycle (BranchId(1) with parent BranchId(1)) and timeline() hangs. However, is_branch...

#### Finding sessions-3: A malformed undo history is trusted on load: undo panics, and redo exhausts memory

- Verified status: confirmed; final severity: critical; rule: no-panic; feature: session persistence / undo and redo.
- Locations: crates/ludenea-control/src/session.rs:654, crates/ludenea-control/src/branches.rs:291-305, crates/ludenea-control/src/branches.rs:446-474, crates/ludenea-control/src/branches.rs:88-95, crates/ludenea-control/src/undo.rs:211-234, crates/ludenea-control/src/session.rs:294-299, crates/ludenea-control/src/host.rs:587-631
- Evidence: `UndoHistory::decode` checks only the encoding. `resume` validates the branch tree but not the entries that name branches. (1) A Fork entry naming the root is accepted by LoadSession; its label already reads 'Undo take-over at tick 0 (removes Branch 1)'. Undo then panics at session.rs:654: `undo PANICKED: Some("a forked branch has a parent")` (probe tests/bad_history.rs). The host does not catch panics around commands, so the game process or worker dies. (2) A redo Fork entry whose stored branch forks at tick 2^36 passes `put_back`, which skips the `check_forks` bound that `BranchTree::decode` applies (branches.rs:472-480). `append_to` then resizes the input log to the fork tick (branches.rs:93): the probe process was SIGKILLed for memory after 50 s (exit 137). In the other direction, a history that fails to decode makes `resume` refuse the whole session (inputs, edits, branches), although the history is only a convenience.
- Reachability: Any LoadSession message (WebSocket, stdio, postMessage) and every stored session the editor reads back from .ludenea/sessions/<document>: a corrupted, hand-edited, or crafted file, or one from another editor build.
- Recommendation: Validate the history against the tree at resume: drop, with a load warning, every entry naming an unknown branch or the root as a fork, and every removed branch whose fork point fails `check_forks` against its parent. Make `put_back` run the same fork check as decode. Replace the `expect` at session.rs:654 with `.ok_or(SessionError::NoSuchBranch(*branch))?`, which `is_dropped` already handles. When the history bytes do not decode, resume with an empty history and a warning instead of refusing the session.
- First test suggested by the auditor: tests/undo.rs: a_fork_entry_naming_the_root_is_dropped_not_a_panic, and a_redo_entry_forking_past_its_parent_is_refused (craft the bytes with postcard as tests/undo.rs already does for version 2)
- Verifier notes (binding scope corrections): Reproduced (probe tests/bad_history.rs): LoadSession accepts a Fork entry that names the root, the undo label reads 'Undo take-over at tick 0 (removes Branch 1)', and Undo panics at session.rs:654 ('a forked branch has a parent'). No command path catches panics: catch_unwind is used only around pick, replay export, compare and divergence. UndoHistory::decode (undo.rs:211-234) checks only the encod...

#### Finding branches-5: Undoing a fork entry that names the root panics the game process

- Verified status: confirmed; final severity: critical; rule: no-panic; feature: branches undo.
- Locations: crates/ludenea-control/src/session.rs:648-661, crates/ludenea-control/src/undo.rs:211-234, crates/ludenea-control/src/session.rs:289-334
- Evidence: Reproduced with scratchpad/branches-divergence/undo_root.py: LoadSession with undo bytes [3,1,2,0,1,5,0,0,0,0], meaning version 3 and one Fork { branch: 0, cause: TakeOver { tick: 5 } }. The timeline already showed 'Undo take-over at tick 5 (removes Branch 1)'. Undo then killed the process: "panicked at crates/ludenea-control/src/session.rs:654:44: a forked branch has a parent". UndoHistory::decode and Session::resume never check history entries against the tree, and undo_entry relies on `forked.parent.expect(...)`. Host::handle has no catch_unwind, so the stdio host dies; in the browser worker the panic hook aborts the instance.
- Reachability: Any stored session in .ludenea/, browser crash recovery, or a LoadSession over the protocol whose undo history is corrupt or hand-edited, followed by Undo. Every file the tools read counts as user input.
- Recommendation: In undo_entry, replace the expect with `forked.parent.ok_or(SessionError::NoSuchBranch(*branch))?`, or a new error such as NotAFork. Add it to is_dropped so the entry is dropped with a precise message. Also validate the history on resume and drop or refuse Fork entries naming the root, so the timeline never shows a label that removes the root.
- First test suggested by the auditor: session.rs unit test: a_fork_entry_naming_the_root_is_dropped. Resume a session whose undo bytes are [3,1,2,0,1,5,0,0,0,0], assert that session.undo() returns an error and that timeline().can_undo is false afterwards, with no panic.
- Verifier notes (binding scope corrections): Reproduced with undo_root.py against the current binary. LoadSession with undo bytes [3,1,2,0,1,5,0,0,0,0] loads, and the timeline shows 'Undo take-over at tick 5 (removes Branch 1)'. Undo then kills the host: 'panicked at crates/ludenea-control/src/session.rs:654:44: a forked branch has a parent'. UndoHistory::decode (undo.rs:211-234) and Session::resume (session.rs:289-334) never check entries a...

### Task 12: Redo rebuilds only the branch's own segment

Backlog item SES-2 (do not cite this identifier in commits or code). Workstream: SES: Editing sessions, undo and redo, timeline (grade C).

**Workstream context.**

Subsystem: ludenea-control session, undo history, checkpoints, timeline.

**Problem.** Session invariants are documented, coverage is high and panics in game code are isolated, but a malformed undo history panics the host or exhausts memory, branch identifiers can be reused into a cycle, redo can rebuild a branch from a stale parent (false recorded hashes) or silently discard play recorded since the undo, restore and edit logs are unbounded, the undo history stores two full scene texts per tick-0 edit, scrubbing far behind the head replays thousands of ticks per step, the host rebuilds the timeline several times per frame, every tick-0 edit is blamed on the game code, and session.rs is a 2,249-line god object.

**Target state.** The undo history and branch tree validate on resume and keep their invariants; redo never loses or falsifies recorded work; work per command is bounded; storage and seek cost follow the change, not the run; Session is split by concern under a seeded sequence test.

**Item.**

- Priority P0, kind fix, effort S, golden impact none.
- Findings: sessions-2.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/session.rs:720-739,1556-1587,2089-2095`, `crates/ludenea-control/src/branches.rs:342-349`.
- Acceptance: Redo of a Run entry splices the branch's own segment onto the current run (or resimulates from the first stale tick), so recorded hashes stay true and a resume reports no divergence.
- First failing test: undo.rs redo_after_a_scene_edit_on_a_fork_keeps_recorded_hashes_true (a fresh replay of export() matches every hash).

**Audit findings resolved by this task (full text).**

#### Finding sessions-2: Redo rebuilds the shared ticks of a branch from a stale parent: recorded hashes go wrong and a resume reports a false divergence

- Verified status: confirmed; final severity: high; rule: determinism; feature: undo and redo / determinism.
- Locations: crates/ludenea-control/src/session.rs:720-739, crates/ludenea-control/src/session.rs:1556-1587, crates/ludenea-control/src/branches.rs:342-349, crates/ludenea-control/src/session.rs:2089-2095
- Evidence: A tick 0 edit on a forked branch marks every stored branch stale, the root included, from its fork tick (branches.rs:342-349). Resimulation refreshes only the current flattened run. Redoing a recorded edit on that branch calls `self.compose(&segment, ..)` (session.rs:735), which rebuilds ticks 0..fork from the root's stale stored segment. It resimulates only when the branch's own segment is stale (session.rs:736-738). Reproduction (scratchpad/editing-audit/probe/tests/redo_stale_prefix.rs): fork at 40, play 20, tick 0 edit, edit at 60, undo, redo. Output: `after redo: first wrong hash Some(1)`; `resume on the same build reports: Diverged { tick: 1, ... }`; `seek 20: world c2387b3378226297, recorded 244f4c36f20fcfdd`. The host then logs 'tick 1 diverged from the recorded session ...; the game code changes the simulation' (host.rs:1206-1209) on the same build. That is a false desync report. `CompareTraces` against the session (host.rs:415-419) and the divergence tools read these hashes too.
- Reachability: Editor, ordinary flow: edit at a past tick (fork), change something at tick 0 (a field, Apply to scene, a prefab override), edit again on the branch, then undo and redo. Any later document switch, asset reload, or crash recovery reports a divergence that does not exist.
- Recommendation: Redo of a `Run` entry happens on the current branch (it switched there first), whose flattened run before the fork is fresh. Splice only the branch's own segment back onto the current run (`own.append_to(self.tree.current_branch().fork(), &mut self.inputs, &mut self.edits, &mut self.hashes, &mut self.checkpoints, defaults)`) instead of recomposing from the tree. Alternatively, compute the first stale tick on the path as `branch_run` does and `resimulate_from` it.
- First test suggested by the auditor: tests/undo.rs: redo_after_a_scene_edit_on_a_fork_keeps_recorded_hashes_true (the reproduction above, asserting a fresh replay of `export()` matches every hash and that resuming reports no Diverged)
- Verifier notes (binding scope corrections): Reproduced exactly (probe tests/redo_stale_prefix.rs). Output: 'after redo: first wrong hash Some(1)'; resume on the same build reports Diverged { tick: 1 }; seek 20 shows world c2387b3378226297 against recorded 244f4c36f20fcfdd. Root cause as described: mark_stale (branches.rs:342-349) clears only the checkpoints of the stored root segment and keeps its old hashes. redo of a Run entry calls compo...

### Task 13: Recursive types refused cleanly instead of aborting the process

Backlog item ECS-1 (do not cite this identifier in commits or code). Workstream: ECS: ECS, schemas, snapshots and hashing (grade B).

**Workstream context.**

Subsystem: ludenea-ecs, ludenea-schema, ludenea-macros, App registration.

**Problem.** Snapshot decoding and restore are robust and well covered, but several type-level traps remain: a recursive type aborts every tool with a stack overflow, restore accepts prefab assets the game cannot spawn, #[serde(default)] is refused so adding a field breaks every saved file, serde from and into mirrors let a schema disagree with its data while two separate remapping mechanisms depend on that agreement, misused systems produce generic trait errors, a reader ordered before every writer silently sees no events, and World::hash rebuilds the whole tree twice per tick.

**Target state.** Every registered type has a sound schema by construction; restore validates everything it accepts; component types can evolve; one fallible schema-guided walker rewrites references; compile errors and build errors name the fix; hashing streams once per tick.

**Item.**

- Priority P0, kind fix, effort S, golden impact none.
- Findings: ecs-8.
- Depends on: nothing.
- Locations and files: `crates/ludenea-schema/src/lib.rs:14-84`, `crates/ludenea-macros/src/lib.rs:90-201`, `crates/ludenea-app/src/simulation.rs:83,236-249`, `crates/ludenea-app/src/app.rs`.
- Acceptance: Derived schema() tracks the types in progress on a thread-local stack and, on re-entry, returns a TypeSchema::Recursive { name } marker; App::build and every registration path return an error naming the type and the advice (store children as Entity references until recursive schemas are supported). A direct self-reference fails at compile time with a spanned error.
- First failing test: compile_fail doctest in ludenea-schema/src/lib.rs for #[derive(Schema)] struct Node { children: Vec<Node> }, plus an app test that an indirectly recursive asset type makes build() return an error instead of overflowing the stack.

**Audit findings resolved by this task (full text).**

#### Finding ecs-8: A recursive component type compiles, then aborts the game with a stack overflow when schemas are listed

- Verified status: confirmed; final severity: medium; rule: no-panic; feature: schemas.
- Locations: crates/ludenea-schema/src/lib.rs:17-21, crates/ludenea-schema/src/lib.rs:26-84, crates/ludenea-macros/src/lib.rs:104-134
- Evidence: Probe /private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/ecs-audit/probe/src/bin/recursive.rs: `#[derive(Component)] struct Node { value: u8, children: Vec<Node> }` compiles and registers. World::component_schemas(), which the host calls for the handshake and schema_fingerprint calls on every snapshot, then aborts with "thread 'main' has overflowed its stack / fatal runtime error: stack overflow, aborting". The only warning is in the Schema trait docs (lib.rs:17-18), not on the Component derive that game developers actually use, and TypeSchema has no way to refer back to a type it is already describing.
- Reachability: (b) A game developer modelling a tree (a dialogue tree, a behaviour tree) as one component; the game process dies on the editor's first connection.
- Recommendation: In expand_schema, reject any field type whose path mentions the deriving type's own ident, with a spanned compile_error ("recursive types have no schema yet; store children as Entity references"). Later, add TypeSchema::Ref { name }, emitted when schema() re-enters a type already in progress (tracked with a thread-local stack of names), so trees are supported and TypeScript emits a recursive alias.
- First test suggested by the auditor: compile_fail doctest in ludenea-schema/src/lib.rs: `#[derive(Schema)] struct Node { children: Vec<Node> }`.
- Verifier notes (binding scope corrections): Reproduced: #[derive(Component)] struct Node { value: u8, children: Vec<Node> } compiles and registers. component_schemas() then aborts the process with a stack overflow, which cannot be caught the way the host catches tick panics. The host calls it at the handshake, and schema_fingerprint calls it on every Simulation::snapshot (simulation.rs:233-249). The only warning is on the Schema trait (lude...

### Task 14: Restore and asset insertion validate prefab assets

Backlog item ECS-3 (do not cite this identifier in commits or code). Workstream: ECS: ECS, schemas, snapshots and hashing (grade B).

**Workstream context.**

Subsystem: ludenea-ecs, ludenea-schema, ludenea-macros, App registration.

**Problem.** Snapshot decoding and restore are robust and well covered, but several type-level traps remain: a recursive type aborts every tool with a stack overflow, restore accepts prefab assets the game cannot spawn, #[serde(default)] is refused so adding a field breaks every saved file, serde from and into mirrors let a schema disagree with its data while two separate remapping mechanisms depend on that agreement, misused systems produce generic trait errors, a reader ordered before every writer silently sees no events, and World::hash rebuilds the whole tree twice per tick.

**Target state.** Every registered type has a sound schema by construction; restore validates everything it accepts; component types can evolve; one fallible schema-guided walker rewrites references; compile errors and build errors name the fix; hashing streams once per tick.

**Item.**

- Priority P0, kind fix, effort M, golden impact certain.
- Findings: ecs-3.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/snapshot.rs:125-238`, `crates/ludenea-ecs/src/assets.rs:153-161`, `crates/ludenea-ecs/src/prefab.rs:20-30,88-120`, `crates/ludenea-ecs/src/world.rs`.
- Acceptance: AssetType gains an optional validate hook; the Prefab hook checks that every component name is registered, every value decodes exactly and addresses are unique. insert_asset, insert_asset_ron and restore refuse with SnapshotError::Decode naming the asset and the component, leaving the world unchanged. Whether a type mentions Entity is computed once at register_component and no longer read from the snapshot.
- First failing test: crates/ludenea-ecs/tests/snapshot.rs restoring_a_prefab_asset_with_an_unregistered_component_is_refused (hash_tree unchanged).

**Audit findings resolved by this task (full text).**

#### Finding ecs-3: Restore accepts a prefab asset this game cannot spawn; the next spawn_prefab panics mid-tick

- Verified status: confirmed; final severity: high; rule: no-panic; feature: snapshots and prefabs at ECS level.
- Locations: crates/ludenea-ecs/src/snapshot.rs:156-167, crates/ludenea-ecs/src/assets.rs:153-161, crates/ludenea-ecs/src/prefab.rs:20-21, crates/ludenea-ecs/src/prefab.rs:88-92, crates/ludenea-ecs/src/prefab.rs:108-123, crates/ludenea-ecs/src/component.rs:56-62, crates/ludenea-control/src/host.rs:392-398, crates/ludenea-control/src/worker.rs:4
- Evidence: Probe /private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/ecs-audit/probe/src/bin/prefab_restore.rs: a snapshot holding a `ludenea::Prefab` asset whose entity carries `other_game::Gone` restores with Ok. The next system calling commands.spawn_prefab then panics at prefab.rs:113 with "prefab 0000...01 has the unregistered component other_game::Gone". Restore only decodes the asset as a Prefab struct (assets.rs:153-161); the component bytes inside stay unchecked. Bytes that do not decode panic the same way (prefab.rs:121, component.rs:148). An incomplete `entity_components` set, which is redundant data stored in the asset (prefab.rs:20-21), silently leaves references as prefab positions with generation 0, so they point at unrelated live entities. The control protocol's Restore { snapshot } passes client bytes straight into this path (host.rs:392-398). In the browser a panic aborts the game's WebAssembly instance (worker.rs:4).
- Reachability: (a) A restore protocol message (CLI, a tool, or the editor) carrying a snapshot from another build of the game or written by hand. Also any caller of World::insert_asset with ludenea::Prefab bytes.
- Recommendation: Validate prefabs where they enter the world. Give AssetType an optional `validate: fn(&World, &AssetValue) -> Result<(), String>` and register one for Prefab that checks every component name is registered, every value's bytes decode exactly, and addresses are unique. Run it in `loaded()` so insert_asset, insert_asset_ron and restore all refuse with SnapshotError::Decode { name: "asset <id>", reason: "entity [0]: component `x` is not registered" }. Drop `entity_components` and compute "its schema mentions Entity" once per type in register_component, stored in ComponentEntry, so a snapshot or replay cannot lie about it. This changes prefab asset bytes, so record golden/spawner.replay again and say so in the commit. The remaining panics in prefab::spawn then become expects on a validated invariant.
- First test suggested by the auditor: tests/snapshot.rs: restoring a snapshot that holds a prefab asset with an unregistered component returns SnapshotError::Decode naming the asset and the component, and the world's hash_tree() is unchanged.
- Verifier notes (binding scope corrections): Confirmed by reading and by running the probe: restore returns Ok, then prefab.rs:113 panics. World::restore (snapshot.rs:125-185) checks names and versions but never the fingerprint, and it decodes a Prefab asset only as opaque (String, Vec<u8>) pairs (assets.rs decode_asset/loaded, prefab.rs:26-30). There is also a realistic case beyond hand-crafted bytes, from a different build. Component Foo c...

### Task 15: Local identifier exhaustion and edge inputs return errors, never panic

Backlog item SCN-3 (do not cite this identifier in commits or code). Workstream: SCN: Scenes, prefabs and data assets (grade C).

**Workstream context.**

Subsystem: ludenea-app scene, prefab, assets and meta modules; prefab and data-asset session operations.

**Problem.** Loading is transactional, identifiers are never reused and override text splicing keeps files byte for byte, but editor prefab operations capture the live world at the current tick and write simulated values into the tick-0 scene and the prefab file, a prefab document can contain itself, identifier exhaustion panics, scenes persist the engine clock (overriding App::tick_rate), data asset saves drop comments, instances cannot be removed, references into a new prefab silently die, and scene.rs and prefab.rs mix many concerns in about 1,400 lines each.

**Target state.** Every prefab and asset operation works from the tick-0 state as one undoable session operation; files keep their text when edited; engine-owned resources stay out of scene files; scene and prefab code is split by concern with a tested RON span parser.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: untrusted-input-2, scenes-23.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/scene.rs:228-252,581-640,700-740,1132-1150`, `crates/ludenea-app/src/prefab.rs:422-430`, `crates/ludenea-control/src/session.rs:1988-1997`.
- Acceptance: parse_scene refuses entity ids at or above next_id; after returns Option; save_scene_keeping and write_scene return Result with SceneError::IdentifiersExhausted { next_id } (renumber the scene); place_instance uses checked_add; override_value_ron returns UnknownComponent; a Spawn that would exhaust identifiers is an InvalidEdit.
- First failing test: crates/ludenea-app/tests/scene.rs a_save_that_needs_more_local_identifiers_than_remain_is_refused, and prefab_editing.rs place_instance_at_the_last_identifier_is_refused.

**Audit findings resolved by this task (full text).**

#### Finding untrusted-input-2: Running out of local identifiers panics the game on an ordinary tick 0 spawn

- Verified status: confirmed; final severity: high; rule: no-panic; feature: scenes (save_scene_keeping) and editor scene edits.
- Locations: crates/ludenea-app/src/scene.rs:1132-1150, crates/ludenea-app/src/scene.rs:581-626, crates/ludenea-app/src/scene.rs:700-740, crates/ludenea-control/src/session.rs:1988-1997
- Evidence: growable() refuses only next_id == u32::MAX, so a scene with `next_id: 4294967294` loads. A tick 0 Spawn goes scene_after -> save_scene_keeping. The new entity pushes next_id to u32::MAX, and a stale Entity value then needs one more fresh identifier: write_scene calls `after(next_id, stale.len())`, whose expect fires. Reproduced over the desktop transport. I sent LoadScene of `Scene(version: 2, next_id: 4294967294, resources: {"ludenea::ContactCache": (pairs: {((index:5,generation:0),(index:6,generation:0)): (touching: false, points: [])})}, entities: [])`, then `{"type":"spawn","components":{}}`. `boxes host --stdio` exits 101 with "panicked at crates/ludenea-app/src/scene.rs:1149:10: a scene has fewer than u32::MAX local identifiers" and writes nothing, not even the welcome of the same batch. The panic is outside run_tick's catch_unwind, so a native game dies, and a browser worker crashes and restarts into the same scene.
- Reachability: A scene file whose next_id is close to u32::MAX (hand-edited, generated, or merged), followed in the editor by any tick 0 edit that spawns an entity.
- Recommendation: Report identifier exhaustion as a SceneError, not a panic. Make `after` return Option, and make `save_scene_keeping` and `write_scene` return `Result<String, SceneError>` with a variant such as `IdentifiersExhausted { next_id }` and "renumber the scene" advice. scene_after then propagates it as a refused edit. Reserving head room in growable is not enough on its own, because stale references can push next_id up by any count.
- First test suggested by the auditor: In crates/ludenea-app/tests/scene.rs: `a_save_that_needs_more_local_identifiers_than_remain_is_refused`. Load the scene above, spawn an entity, and assert that save_scene_keeping returns an Err instead of panicking. Add a host test asserting that Spawn after this LoadScene returns an InvalidEdit error and that GetHash still answers.
- Verifier notes (binding scope corrections): True, and easier to trigger than the finding says. When a plain spawn exhausts next_id, the refusal is already clean: next_id 4294967294 followed by two spawns gives the invalid_scene error "`next_id` is 4294967295 ... renumber the scene". But parse_scene (scene.rs:228-252) never checks entity ids against next_id. A scene `Scene(version: 2, next_id: 0, resources: {}, entities: [(id: 4294967295, co...

#### Finding scenes-23: Public prefab and scene APIs panic on reachable edge inputs

- Verified status: confirmed; final severity: medium; rule: no-panic; feature: prefab and scene public API.
- Locations: crates/ludenea-app/src/prefab.rs:426-428, crates/ludenea-app/src/scene.rs:631-639, crates/ludenea-app/src/scene.rs:1141-1150
- Evidence: `place_instance` computes `id + 1` on the parsed `next_id`. Probe: a scene with `next_id: 4294967295` panics, by overflow in debug and by wrapping to a duplicate identifier 0 in release. The session path is guarded by the load-time `growable` check, but the public function is not. `override_value_ron` returns `Result` yet panics with `expect("registered")` on an unregistered component. `after` panics when stale references push past `u32::MAX`, from a hand-written `next_id` near the limit.
- Reachability: A developer calling the public `ludenea_app::prefab` or `Simulation` API (user (b)); a hand-edited `next_id` near `u32::MAX`.
- Recommendation: Use `checked_add` in `place_instance` and return the `growable` error. Return `SceneError::UnknownComponent` from `override_value_ron`. Make `write_scene` fallible, or refuse at load any `next_id` that leaves too little room for stale references.
- First test suggested by the auditor: crates/ludenea-app/tests/prefab_editing.rs: `place_instance_at_the_last_identifier_is_refused`.
- Verifier notes (binding scope corrections): Verified. place_instance (prefab.rs:422-430) parses `next_id` as u32 and computes `id + 1` unchecked. A text with next_id 4294967295, which the type allows, panics in debug. A game crate built from the template (no [profile.release] override) wraps it to a duplicate local identifier 0; only the engine workspace sets overflow-checks in release (Cargo.toml:9-10). The function returns Result and docu...

### Task 16: Create prefab and Apply to prefab work from the tick-0 state; Apply is one undoable operation

Backlog item SCN-1 (do not cite this identifier in commits or code). Workstream: SCN: Scenes, prefabs and data assets (grade C).

**Workstream context.**

Subsystem: ludenea-app scene, prefab, assets and meta modules; prefab and data-asset session operations.

**Problem.** Loading is transactional, identifiers are never reused and override text splicing keeps files byte for byte, but editor prefab operations capture the live world at the current tick and write simulated values into the tick-0 scene and the prefab file, a prefab document can contain itself, identifier exhaustion panics, scenes persist the engine clock (overriding App::tick_rate), data asset saves drop comments, instances cannot be removed, references into a new prefab silently die, and scene.rs and prefab.rs mix many concerns in about 1,400 lines each.

**Target state.** Every prefab and asset operation works from the tick-0 state as one undoable session operation; files keep their text when edited; engine-owned resources stay out of scene files; scene and prefab code is split by concern with a tested RON span parser.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: scenes-1, scenes-9, ux-live-6.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/host.rs:681-683,739-765,951-1006`, `crates/ludenea-control/src/session.rs:1029-1096`, `editor/packages/editor-builtins/src/prefabs/index.tsx:121-130`.
- Acceptance: Session::create_prefab and Session::apply_to_prefab replace the host's live-world code. Create restores checkpoint 0, writes the prefab and replays to the current tick. Apply reads the override's value from the scene text, updates the prefab text in the session's assets (marked unsaved, written by Save), removes the override, all under one undo entry; undo restores both.
- First failing test: crates/ludenea-control/tests/protocol_v3.rs apply_to_prefab_after_playing_writes_the_override_value, plus session_assets.rs create_prefab_after_playing_keeps_the_tick_0_values and apply_to_prefab_moves_the_override_into_the_prefab.

**Audit findings resolved by this task (full text).**

#### Finding scenes-1: Create prefab from selection and Apply to prefab capture the live world at the current tick, writing simulated values into the tick 0 scene and the prefab file

- Verified status: partially; final severity: high; rule: feature-robustness; feature: prefabs (host and session).
- Locations: crates/ludenea-control/src/host.rs:681-683, crates/ludenea-control/src/host.rs:739-765, crates/ludenea-control/src/host.rs:951-1006, crates/ludenea-control/src/session.rs:1065-1096, editor/packages/editor-builtins/src/prefabs/index.tsx:121-130, editor/packages/editor-builtins/src/prefabs/index.tsx:145-168
- Evidence: `Command::CreatePrefab` runs `prefab::create_prefab(sim, ..)` on the live simulation, and `ApplyToPrefab` gets its value from `override_ron(.., None)`, which reads `sim.world().component_bytes(entity, component)` (host.rs:972-976). Neither checks the tick. Reproduced with the scratch probe: in a version 2 boxes scene, after 120 ticks, create-from-selection on entity [10] changed its tick 0 Pose from (x:-4.9207296,y:1.9778125,cos:1.0) to (x:-5.569748,y:0.96759176,cos:0.158). On the spawner scene after `Step{ticks:120}`, `ApplyToPrefab` for instance 6 Pose.position.x wrote `x:6.756054` into crate.prefab.ron instead of the override's 5.0. The editor writes that text to disk, outside undo.
- Reachability: Editor: pause or scrub to any tick past 0, then run "Create prefab from selection" or click "Apply to prefab" on an override mark (shown at every tick).
- Recommendation: Move both operations into `Session` and base them on the tick 0 state, never the live world. `Session::create_prefab(sim, entities)`: restore checkpoint 0 as `edit_scene` does (session.rs:1955-1960), run `write_prefab`, then `replay_to(current)`. `Session::apply_to_prefab`: take the value from the instance's override entry in the scene text (`ron_at_path` on the matching `overrides` entry), not from the world. The host then only calls the session. Until then, refuse both past tick 0 with "seek to tick 0 first".
- First test suggested by the auditor: crates/ludenea-control/tests/protocol_v3.rs: `apply_to_prefab_after_playing_writes_the_override_value`: load the spawner, Step 120 ticks, ApplyToPrefab instance 6 Pose.position.x, assert the text has `position:(x:5.0,y:26.0)`. Also session_assets.rs `create_prefab_after_playing_keeps_the_tick_0_values`.
- Verifier notes (binding scope corrections): I reproduced it (probe copy at /private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/verify-scenes-adv8/probe, bins tickn and applyn). After 120 ticks, create-from-selection moved entity [10]'s tick 0 Pose to its simulated value. ApplyToPrefab on instance 6 wrote x:6.756054 into the prefab instead of the override's 5.0. host....

#### Finding scenes-9: After "Apply to prefab" the instance keeps its override and the change is not undoable

- Verified status: confirmed; final severity: medium; rule: feature-robustness; feature: prefabs (apply).
- Locations: editor/packages/editor-builtins/src/prefabs/index.tsx:121-130, crates/ludenea-control/src/host.rs:739-765, crates/ludenea-control/src/session.rs:1029-1048
- Evidence: `apply` writes the returned prefab text and never reverts the override in the scene. The field stays marked "overridden" with the same value, so later edits of that prefab field never reach this instance. The prefab change arrives as a disk write, and `replace_scene` records an undo entry only when the scene text changed (session.rs:1039-1044), so undo does not take the apply back.
- Reachability: Editor: click "Apply to prefab" on an override mark.
- Recommendation: Make apply one session operation, `Session::apply_to_prefab(instance, entity, component, path)`. It reads the override's value from the scene (see the wrong-tick finding), updates `assets[prefab]` with `prefab::apply_to_prefab`, removes the override with `prefab::revert_override`, and records both under one undo entry. Its reply carries the prefab text for the editor to write.
- First test suggested by the auditor: crates/ludenea-control/tests/session_assets.rs: `apply_to_prefab_moves_the_override_into_the_prefab`. Afterwards the scene has no override for that field, the prefab text holds the value, and undo restores both.
- Verifier notes (binding scope corrections): Holds as described. The editor's `apply` (editor/packages/editor-builtins/src/prefabs/index.tsx:121-130) only sends ApplyToPrefab and writes the returned text. Host ApplyToPrefab (host.rs:739-765) builds the prefab text and never touches the session scene. The override mark comes from the scene file's override list (prefab.rs:54-58, OverrideMark.tsx:21-23), so it stays and the redundant override k...

#### Finding ux-live-6: 'Apply to prefab' writes the file at once, keeps the override, and Undo does not revert the prefab

- Verified status: confirmed; final severity: medium; rule: ux; feature: prefabs.
- Locations: editor/packages/editor-builtins/src/prefabs/index.tsx:121-130, crates/ludenea-control/src/host.rs:739
- Evidence: Live:
- Overriding position.x = 0 on instance 6v0 and clicking 'Apply position.x to prefab' rewrote assets/ball.prefab.ron immediately (diff x:1.5 -> x:0.0). It asked no confirmation and set no unsaved marker.
- The instance still showed 'overridden' with Revert/Apply after reselection (shots/31-applied-to-prefab.png), so it no longer follows the prefab for that field.
- The Undo button ('Undo scene edit') then removed the override, but the prefab file still has x:0.0 (grep after undo).
- Every other instance moved with no way back except editing the file.
- Reachability: Any designer using the per-field or per-component 'Apply to prefab' action.
- Recommendation: Make apply one undoable edit:
- Put the new prefab text in the session's assets and mark it unsaved, like a data asset at tick 0, saved by Save.
- Drop the applied override in the same step.
- Undo then restores both the prefab text and the override.
If writing at once stays the design, at least drop the override, confirm before writing, and leave no undo entry that only half-reverts.
- First test suggested by the auditor: editor-builtins prefabs.test.tsx (fake control): apply(target) leaves overridesOf(instance) empty. Session test: an apply followed by an undo serves the previous prefab text.
- Verifier notes (binding scope corrections): prefabs/index.tsx:121-130 gets the text and calls write_text at once. There is no confirmation, no unsaved marker and no session edit. The ApplyToPrefab handler (host.rs:739-764) only returns the spliced text and never touches the session scene, so the instance's override stays. After the file write, the shell calls replace_scene (document.ts:355) with the same scene text, and session.rs:1038-1043...

### Task 17: Prefab documents are cycle-checked against themselves

Backlog item SCN-2 (do not cite this identifier in commits or code). Workstream: SCN: Scenes, prefabs and data assets (grade C).

**Workstream context.**

Subsystem: ludenea-app scene, prefab, assets and meta modules; prefab and data-asset session operations.

**Problem.** Loading is transactional, identifiers are never reused and override text splicing keeps files byte for byte, but editor prefab operations capture the live world at the current tick and write simulated values into the tick-0 scene and the prefab file, a prefab document can contain itself, identifier exhaustion panics, scenes persist the engine clock (overriding App::tick_rate), data asset saves drop comments, instances cannot be removed, references into a new prefab silently die, and scene.rs and prefab.rs mix many concerns in about 1,400 lines each.

**Target state.** Every prefab and asset operation works from the tick-0 state as one undoable session operation; files keep their text when edited; engine-owned resources stay out of scene files; scene and prefab code is split by concern with a tested RON span parser.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: scenes-2.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/session.rs:89-93,1051-1060,2147-2165`, `crates/ludenea-control/src/host.rs:518-522`, `crates/ludenea-app/src/prefab.rs:664-682`.
- Acceptance: Document::Prefab carries its AssetId (protocol change, changelog entry); a prefab session loads through the overlay so its own text replaces its id; every edit is checked for cycles; SaveScene of a prefab runs save_prefab_checked before marking it saved.
- First failing test: crates/ludenea-control/tests/session_assets.rs placing_a_prefab_inside_itself_is_refused (SceneError::PrefabCycle, text unchanged).

**Audit findings resolved by this task (full text).**

#### Finding scenes-2: A prefab document accepts an instance of itself; its save is never cycle-checked, which breaks every scene using it

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: prefabs (prefab documents).
- Locations: crates/ludenea-control/src/session.rs:89-93, crates/ludenea-control/src/session.rs:1051-1060, crates/ludenea-control/src/session.rs:2147-2165, crates/ludenea-control/src/host.rs:518-522, crates/ludenea-app/src/prefab.rs:631-682
- Evidence: `Document::Prefab` carries no asset id, so a prefab document resolves against the prefab's saved text in `session.assets`, not the text being edited. Probe `selfplace`: `Session::start(crate.prefab.ron, Document::Prefab)` then `place_instance(PREFAB)` returned Ok(1). `save_prefab_checked` on that text returned "prefab instances form a cycle: 38b3.. -> 38b3..", and loading spawner.scene.ron with it failed the same way. `Command::SaveScene` returns `session.scene()` without `save_prefab_checked`, though the spec (2b section 4.4) says "Cycles are refused at save and at load".
- Reachability: Editor: open a prefab, run "Place prefab" and pick that same prefab (or one that contains it), then save.
- Recommendation: Make `Document::Prefab` carry its `AssetId`, as a deliberate, versioned protocol change. In `load`/`load_kept`, load a prefab document through the existing `Overlay` (prefab.rs:664-682) so its current text replaces its own id: every edit is then checked against cycles through itself. For prefab documents, run `save_prefab_checked` in `SaveScene` before marking the document saved.
- First test suggested by the auditor: crates/ludenea-control/tests/session_assets.rs: `placing_a_prefab_inside_itself_is_refused`. Start a prefab session on crate.prefab.ron with its id, `place_instance(PREFAB)` returns `SceneError::PrefabCycle`, and the scene text is unchanged.
- Verifier notes (binding scope corrections): Reproduced (bin selfplace): place_instance of the open prefab into itself returns Ok(1), save_prefab_checked on that text reports the cycle, and loading spawner.scene.ron then fails. Document::Prefab (session.rs:89-93) has no asset id, and load() (session.rs:2147-2165) resolves nested instances against session.assets, the saved text. So both self-placement and an indirect cycle (open B, place A, w...

### Task 18: Save scene is two-phase: the scene stays unsaved until the write succeeds

Backlog item SHL-1 (do not cite this identifier in commits or code). Workstream: SHL: Editor shell and panels (grade D).

**Workstream context.**

Subsystem: editor-shell layout, palette, shortcuts and stores; built-in panels (console, inspector, scene tree, viewport, timeline).

**Problem.** The registry, palette accessibility, virtualized lists and document store are solid, but Save marks the scene saved before the file is written (a failed write loses the edits silently), the palette runs alphabetically first matches (typing Undo nudges the selection), the layout is never saved, empty states ignore the build, the Console neither virtualizes nor follows its tail, shortcuts are swallowed by text fields and Delete does nothing on Mac laptops, entity lists have no keyboard navigation, inspector inputs collapse to 18px on overridden instances and show f32 noise, and maps and entity references cannot be edited.

**Target state.** Saving is two-phase; commands are found by quality and their failures shown; layout persists; panels say what the build is doing; lists are keyboard listboxes; the inspector edits every value shape legibly.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: shell-1, ambition-1.
- Depends on: VER-3, ERR-1.
- Locations and files: `crates/ludenea-control/src/host.rs:518-522`, `crates/ludenea-control/src/session.rs:955-959,1305`, `crates/ludenea-control/src/protocol.rs`, `editor/packages/editor-builtins/src/scene-tree/index.tsx:23-36`, `editor/packages/editor-builtins/src/inspector/index.tsx:87-107`.
- Acceptance: SaveScene returns the text and leaves the scene dirty; MarkSceneSaved { text } clears it only when the text still equals the session scene, so an edit made during the write stays unsaved. scene.save marks saved only after write_text succeeds and reports a failed write as Cannot save <path>: <reason>. The host tracks data asset dirtiness the same way (unsaved assets in Timeline).
- First failing test: crates/ludenea-control/tests/editing.rs save_scene_keeps_the_scene_dirty_until_marked_saved, and scene-tree.test.tsx a_failed_write_keeps_the_scene_unsaved_and_says_why.

**Audit findings resolved by this task (full text).**

#### Finding shell-1: Save scene marks the scene saved before the file is written; a failed write is silent and the edits are dropped when the document reopens

- Verified status: confirmed; final severity: critical; rule: feature-robustness; feature: scene tree: save scene.
- Locations: crates/ludenea-control/src/host.rs:518-521, crates/ludenea-control/src/session.rs:955-958, crates/ludenea-control/src/session.rs:1305, editor/packages/editor-builtins/src/scene-tree/index.tsx:23-36, editor/packages/editor-shell/src/stores/document.ts:244-249, editor/packages/editor-shell/src/stores/document.ts:305-306, editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-shell/src/palette/CommandPalette.tsx:58
- Evidence: Command::SaveScene calls session.mark_scene_saved(), which sets scene_dirty to false, and only then returns the RON (host.rs:518-521). The editor writes the file afterwards with project.request(write_text) (scene-tree/index.tsx:33). When that write fails, the error goes only to devtools: Mod+S goes through shortcuts.ts:107 and the palette through CommandPalette.tsx:58. Reproduced in scratchpad/shell-audit/audit.test.tsx with write_text answering an error: save_scene was sent, devtools received 'command "scene.save" failed: ProtocolFailure: scenes/main.scene.ron: permission denied', and the Console panel received nothing. The unsaved dot (AssetsPanel.tsx:34 reads timeline.scene_dirty) disappears even though the file is unchanged. The next export has saved_scene: null (session.rs:1305). When the document is opened again (after visiting a prefab, or after restarting the editor), refreshed() replaces the session's scene with the file text on disk because saved_scene is null (document.ts:246). The edits are lost without any message.
- Reachability: Ordinary Mod+S or 'Save scene' from the palette whenever the project service refuses the write: a full disk, a read-only file on Windows or a read-only folder, the scene's folder removed meanwhile, or the connection to ludenea serve lost. Then switch documents or reopen the editor.
- Recommendation: Make saving two-phase. SaveScene returns the RON and leaves the scene dirty. After write_text succeeds, the editor sends a new MarkSceneSaved { scene } command, and the host marks the scene saved only if its current scene still equals that text, so an edit made during the write stays unsaved. This is a control protocol change: bump the version, regenerate the TypeScript bindings, and record it in the changelog. In scene.save, report a failed write in the Console panel as 'Cannot save <path>: <reason>' (see the error-channel finding) and leave editor.document.text unchanged.
- First test suggested by the auditor: Rust, in crates/ludenea-control/tests: save_scene_leaves_the_scene_dirty_until_marked_saved (SaveScene, then GetTimeline: scene_dirty is still true; MarkSceneSaved with the returned text clears it; MarkSceneSaved with a stale text does not). Editor, in scene-tree.test.tsx: a_save_whose_write_fails_is_reported_and_marks_nothing (write_text answers an error; no mark_scene_saved is sent; the Console panel shows 'Cannot save scenes/main.scene.ron: permission denied').
- Verifier notes (binding scope corrections): Holds as stated. host.rs:518-521 calls mark_scene_saved() (session.rs:955-958 sets scene_dirty=false) before the editor writes the file. scene-tree/index.tsx:33 writes afterwards, and a rejected write_text leaves the host clean. Because handle() calls report_timeline (host.rs:136), TimelineChanged clears the unsaved dot. export() then sends saved_scene: null (session.rs:1305), and refreshed() swap...

#### Finding ambition-1: SaveScene clears the dirty flag before the project write: a failed save silently loses unsaved scene edits

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: scene saving (control + project protocols).
- Locations: crates/ludenea-control/src/host.rs:518-522, crates/ludenea-control/src/session.rs:956-959, editor/packages/editor-builtins/src/scene-tree/index.tsx:23-35, editor/packages/editor-shell/src/shortcuts.ts:107, editor/packages/editor-shell/src/stores/document.ts:243-248, editor/packages/editor-builtins/src/assets/index.tsx:165-171
- Evidence: Command::SaveScene returns the text and calls `self.session.mark_scene_saved()` (host.rs:519-520), which sets `saved_scene = scene` and `scene_dirty = false` (session.rs:956-959). Only afterwards does the editor send `write_text` to the project service (scene-tree/index.tsx:32-33). If that write fails (read-only file, permission, disk full), the host already believes the scene is saved. Save scene is reachable only from the palette and Mod+S, and both swallow the error into `console.error` (shortcuts.ts:107, CommandPalette.tsx:58), so nothing tells the user. From then on, ExportSession has `saved_scene: None`, so on the next reopen `refreshed()` substitutes the disk text for the session scene (document.ts:245: `saved.load.saved_scene === null && text !== null ? text : saved.load.scene`), and the edits are gone. Any change to the file on disk also reloads it with no banner, because `scene_dirty` is false (assets/index.tsx:169-170). Data assets do it the other way: the editor marks them saved, writes, and marks them unsaved again on failure (inspector/index.tsx:97-105). Two different dirty-tracking schemes. No test covers a failed scene write (scene-tree.test.tsx:93-107 only tests success).
- Reachability: Editor user: make the scene file read-only (or lose write permission), edit at tick 0, press Mod+S, then reopen the document or let any tool touch the file.
- Recommendation: Make saving two-phase. SaveScene returns the text without marking it saved. A new `MarkSceneSaved { text }` (applied only when `text` equals the session scene) is sent after `write_text` succeeds. Track data asset dirtiness in the host the same way (`unsaved_assets` in Timeline), so scene and assets share one mechanism and any client sees it. Report command failures to the user (see the palette finding).
- First test suggested by the auditor: crates/ludenea-control/tests/editing.rs: `save_scene_keeps_the_scene_dirty_until_marked_saved`. Edit at tick 0, send SaveScene, assert `GetTimeline.scene_dirty == true`, send MarkSceneSaved with the returned text, assert false. Editor side, scene-tree.test.tsx: `a_failed_write_keeps_the_scene_unsaved_and_reports_it`, with `write_text` rejecting, expecting no `mark_scene_saved` and a visible error.
- Verifier notes (binding scope corrections): Confirmed as stated. host.rs:518-521 builds the text and calls session.mark_scene_saved() (session.rs:956-959: saved_scene = scene, scene_dirty = false) before the editor sends write_text (scene-tree/index.tsx:32-33). If the write fails, the command throws, and the only callers (Mod+S at shortcuts.ts:107 and the palette at CommandPalette.tsx:58) send the error to console.error. No Save button exis...

### Task 19: Stored sessions: versioned, decoded defensively, never overwritten when refused

Backlog item VER-5 (do not cite this identifier in commits or code). Workstream: VER: Versioning and compatibility (grade C).

**Workstream context.**

Subsystem: cross-cutting: file formats, protocols, public APIs.

**Problem.** Binary formats mostly read their version first and two real migrations exist, but meta versions are never checked, traces and input logs decode in full before checking, text formats silently drop unknown or misspelled fields, the branch tree and the stored session have no version (and a stored session that does not decode blocks its document while a refused one gets overwritten), the protocol wire shape changed several times under the same number, version errors give wrong advice, the engine identity is a constant nobody reads, the plugin API has no version, and no policy or changelog exists.

**Target state.** A written policy with a CHANGELOG formats table checked against the constants; one versioned-decode helper and one VersionError; every format, editor state included, versioned, strict and pinned by frozen fixtures; the protocol shape pinned by a fingerprint; the engine identity and the plugin API version recorded and reported.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: versioning-1, sessions-12, shell-12, untrusted-input-5, ts-quality-2, readiness-13.
- Depends on: VER-3.
- Locations and files: `editor/packages/editor-shell/src/stores/document.ts:70-78,154-160,236-249,300-306`, `crates/ludenea-project/src/protocol.rs:118-125`, `crates/ludenea-project/src/files.rs:251-267`, `crates/ludenea-project/src/service.rs:245-251`.
- Acceptance: The stored session is { version: 1, saved } read through a codec returning a result. An unreadable, other-version or game-refused session never blocks opening: the document opens from its file, a warning names .ludenea/sessions/<id>.session and the reason, and a new project request sets the file aside as <id>.<n>.rejected.session before any later store of that document (project protocol bump and changelog entry). Stored bytes are encoded without per-byte string concatenation.
- First failing test: editor/packages/editor-shell/test/document.test.ts a_stored_session_that_does_not_decode_opens_the_document_from_its_file_and_is_set_aside (cases: not base64, not JSON, older shape, newer version, refused by the game).

**Audit findings resolved by this task (full text).**

#### Finding versioning-1: A stored session that does not decode blocks its document, and one the game refuses gets overwritten

- Verified status: partially; final severity: high; rule: feature-robustness; feature: stored sessions (.ludenea/sessions/<id>.session).
- Locations: editor/packages/editor-shell/src/stores/document.ts:71-78, editor/packages/editor-shell/src/stores/document.ts:154-160, editor/packages/editor-shell/src/stores/document.ts:238-249, editor/packages/editor-shell/src/stores/document.ts:305-306, editor/packages/editor-shell/src/stores/document.ts:319-336, editor/packages/editor-shell/src/stores/document.ts:461-467, crates/ludenea-project/src/files.rs:251-262
- Evidence: The stored session is the JSON of the TypeScript `SavedSession` (document.ts:71-78), base64-encoded (154-160), with no version field. `decode` calls `JSON.parse` with no guard, and `refreshed` reads `saved.load.keeps_assets` with no shape check (244-249). Both run inside `open` before any fallback (305-306), so any failure lands in `.catch(reported("Cannot open ..."))`. Reproduced with a scratch vitest that uses the repository's own test support (/private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/versioning/vitest/stored.test.ts): with stored bytes btoa("{truncated"), the open fails with "Cannot open scenes/main.scene.ron: Expected property name or '}' in JSON at position 1" and current stays null. With an older shape `{tick: 3}`, it fails with "Cannot read properties of undefined (reading 'keeps_assets')". The file is never moved or named, so the document cannot open again until the user finds and deletes the file by hand; the last-document reopen at project start hits the same failure. Second path: when the game refuses a stored session (319-336, for example an undo or branch tree it cannot decode, or a scene with a component the rebuilt game dropped), the log says the document "opened from its file". The next export (on a pause or a timeline change) then overwrites the stored file through `storeIn`, so the unsaved scene edits, unsaved data assets, and branches it held are lost. The existing test a_stored_session_that_fails_to_restore_leaves_the_document_open_from_its_file (document.test.ts:154) checks only the message, not that the session survives.
- Reachability: Any user who opens a project after a hand edit, a disk problem, or an editor upgrade that changes SavedSession or LoadSession. The refusal path is ordinary: rebuild the game after removing a component type, then reopen a document whose stored session uses it.
- Recommendation: 1. Version the file: store `{ version: 1, protocol: PROTOCOL_VERSION, saved }`, and read it through a `parseStoredSession(bytes): SavedSession | { unreadable: string }` that checks the shape (load.type === "load_session", numeric tick) and the version. 2. When it is unreadable or another version, open the document from its file and log a warning that names the file and the reason. 3. Never overwrite a session the editor could not restore: add a project request `SetAsideStoredSession { document }`. The service renames `.ludenea/sessions/<id>.session` to `<id>.<n>.rejected.session` and answers with the path, and the editor calls it before any later store of that document, then says "kept as <path>" in the console. Both the decode failure and the game refusal go through this one path.
- First test suggested by the auditor: editor/packages/editor-shell/test/document.test.ts: it("a_stored_session_that_does_not_decode_lets_the_document_open_and_is_set_aside"). The fake project answers load_stored_session with btoa("{truncated"). Expect current.id === ids.main, a warning naming .ludenea/sessions/<id>.session, and a set_aside_stored_session request. Then it("a_refused_stored_session_is_not_overwritten_by_the_next_store"): after the refusal, `await host.sessions.store()` leaves the original bytes reachable under the set-aside path.
- Verifier notes (binding scope corrections): Both paths hold up in the code. (a) A stored session that does not decode blocks its document. I re-ran the auditor's scratch vitest (/private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/versioning/vitest/stored.test.ts) and both cases fail: 'Expected property name...' for the bad JSON and 'Cannot read properties of undefine...

#### Finding sessions-12: Stored sessions are an unversioned JSON wrapper, and one that does not decode blocks opening the document

- Verified status: confirmed; final severity: medium; rule: versioning; feature: session persistence.
- Locations: editor/packages/editor-shell/src/stores/document.ts:154-160, editor/packages/editor-shell/src/stores/document.ts:238-241, editor/packages/editor-shell/src/stores/document.ts:300-306, editor/packages/editor-shell/src/stores/document.ts:461-467
- Evidence: `stored()` runs `JSON.parse(atob(...))` inside `open`'s try block (document.ts:305), before the document loads. A stored session that is not JSON, or lacks `load`, makes `refreshed` read `saved.load.keeps_assets` of undefined. The whole open is then refused with 'Cannot open <path>: Unexpected token ...', and the document cannot be opened until the user finds and deletes .ludenea/sessions/<id> by hand. Crash recovery (`own`) fails the same way. The wrapper has no version, unlike the inputs, edits, undo, and branch blobs it carries. There is a test for a session the game refuses (document.test.ts:154), none for one that does not decode.
- Reachability: A corrupted, hand-edited, or merged .ludenea/sessions file, or one written by an editor with another SavedSession shape.
- Recommendation: Write `{ version: 1, ... }` and check it on read. On any decode failure, log a warning naming the file ('the stored session of scenes/main.scene.ron cannot be read and was set aside as <file>.unreadable'), move the file aside, and open the document from its file.
- First test suggested by the auditor: editor-shell/test/document.test.ts: a_stored_session_that_does_not_decode_opens_the_document_from_its_file
- Verifier notes (binding scope corrections): `decode` (document.ts:159) runs atob and JSON.parse with no guard. `stored(document)` (document.ts:238-241) is awaited inside `open`'s try/finally (document.ts:305) before `set({ current: document ... })`. Any decode error therefore propagates to `.catch(reported(`Cannot open ${name}`))` and the document does not open. Valid JSON without `load` throws a TypeError in `refreshed` (document.ts:245). ...

#### Finding shell-12: The stored session file is unversioned, and one that does not decode blocks opening its document with a raw parser error

- Verified status: confirmed; final severity: medium; rule: versioning; feature: document store sessions.
- Locations: editor/packages/editor-shell/src/stores/document.ts:70-78, editor/packages/editor-shell/src/stores/document.ts:154-160, editor/packages/editor-shell/src/stores/document.ts:238-241, editor/packages/editor-shell/src/stores/document.ts:244-249, editor/packages/editor-shell/src/stores/document.ts:305-306, crates/ludenea-project/src/files.rs:554-556
- Evidence: SavedSession, persisted in .ludenea/sessions/<id>.session across editor versions, has no version field (document.ts:71-78). Its shape already changed (document, unsavedAssets, and keeps_assets were added). decode is a bare JSON.parse (line 160) and stored() is called inside open (line 305). Reproduced in scratchpad/shell-audit/audit.test.tsx: a stored session '{truncated' makes open() reject with 'SyntaxError: Expected property name or '}' in JSON at position 1'. The document does not open (current stays null), and the console line does not name .ludenea/sessions/<id>.session or say that deleting it recovers. A JSON object missing load fails in refreshed() with 'Cannot read properties of undefined'. A session the game refuses is handled gracefully (lines 318-336), but a session the editor itself cannot read is not.
- Reachability: A stored session written by another editor version, a hand-edited or synced .ludenea folder, or a partial copy.
- Recommendation: Wrap the stored bytes in a versioned envelope ({ version: 1, saved }). Validate the shape on decode, accept the previous unversioned form explicitly, and on any decode or validation failure log a warning 'The stored session of <path> (.ludenea/sessions/<id>.session) cannot be read (<reason>); the document opened from its file', then open from the file as the game-refusal path already does.
- First test suggested by the auditor: document.test.ts: a_stored_session_that_does_not_decode_opens_the_document_from_its_file. load_stored_session returns base64 of '{truncated': expect current set to the scene, a warning naming .ludenea/sessions/<id>.session, and load_scene sent.
- Verifier notes (binding scope corrections): SavedSession has no version field (document.ts:71-78). decode is a bare atob plus JSON.parse (line 160), and stored() runs inside open before the guarded restore (line 305). Any decode failure therefore rejects the whole open through reported() (line 343). The console then shows 'Cannot open <path>: Expected property name...'. It names the document but not .ludenea/sessions/<id>.session (files.rs:...

#### Finding untrusted-input-5: A stored session that does not decode blocks opening its document; the session file format has no version

- Verified status: partially; final severity: medium; rule: versioning; feature: editor document store (stored sessions).
- Locations: editor/packages/editor-shell/src/stores/document.ts:154-160, editor/packages/editor-shell/src/stores/document.ts:238-245, editor/packages/editor-shell/src/stores/document.ts:305-306, editor/packages/editor-shell/src/stores/document.ts:343, crates/ludenea-control/src/branches.rs:539-543
- Evidence: A stored session is `btoa(JSON.stringify(SavedSession))` with no version field, and decode is a bare JSON.parse. open() awaits `stored(document)` and `refreshed(saved, ...)` before loading. Both run inside the serial block whose catch turns any error into "Cannot open <path>". Reproduced these two functions in node (sweep/stored.mjs): a session `{}` gives "Cannot open scenes/main.scene.ron: Cannot read properties of undefined (reading 'keeps_assets')", and a truncated one gives "Cannot open scenes/main.scene.ron: Expected ':' after property name in JSON at position 7". The document cannot be opened at all, and that includes startup, when the project opens its last or default scene. The message names neither .ludenea/sessions/<id>.session nor a fix (deleting that file). The code means to avoid exactly this: document.ts:319-320 says "a stored session it cannot take back is reported, ..., not a refused open", but that covers only a refusal by the game. SavedSession mirrors the load_session command, so a protocol change also breaks every stored session. The branch tree bytes inside it (StoredTree) have no version either, while UndoHistory has UNDO_VERSION. document.test.ts has no case for an unreadable stored session.
- Reachability: Opening any scene or prefab whose stored session is corrupt, hand-edited, or written by another editor version, including on startup.
- Recommendation: Wrap the stored session in a versioned envelope (`{ version: 1, saved }`) and decode it defensively: check the version and the shape. On any failure, open the document from its file and log a warning naming the session file and saying it was ignored (optionally move it aside as <id>.session.bad). Add a version to StoredTree in branches.rs, as UndoHistory already has.
- First test suggested by the auditor: In editor/packages/editor-shell/test/document.test.ts: `a_stored_session_that_does_not_decode_leaves_the_document_open_from_its_file`. Make the project double return btoa("{}"), then btoa("{\"load\""), as stored bytes. Assert that open resolves, that current is the document, and that a warning names .ludenea/sessions/<id>.session.
- Verifier notes (binding scope corrections): Code as described. decode is a bare JSON.parse (document.ts:154-160). stored() and refreshed() run before the restore try block (document.ts:305-306), so `{}` throws on saved.load.keeps_assets. The open fails as "Cannot open <path>: ...", which names neither the .session file nor the fix. document.test.ts has no unreadable-session case. StoredTree has no version, while UndoHistory does (undo.rs:11...

#### Finding ts-quality-2: A stored session that does not decode makes its document unopenable, with a non-actionable error

- Verified status: partially; final severity: medium; rule: errors; feature: documents / stored sessions.
- Locations: editor/packages/editor-shell/src/stores/document.ts:71-78, editor/packages/editor-shell/src/stores/document.ts:154-160, editor/packages/editor-shell/src/stores/document.ts:238-241, editor/packages/editor-shell/src/stores/document.ts:305-306, editor/packages/editor-shell/src/stores/document.ts:461-467
- Evidence: Stored sessions are the base64 JSON of SavedSession with no version field, decoded by an unchecked `JSON.parse(...)` cast (document.ts:160). The decode runs inside open() with no handling of its own, so any failure refuses the whole open. Scratch reproduction scratchpad/editor-ts/stored.test.ts answers load_stored_session with '%%%', btoa('{trunc'), and an older-shape session ({ tick, load: { type, scene } }). In every case open rejects and current stays null. Logged messages: 'Cannot open scenes/main.scene.ron: Invalid character' and 'TypeError: Cannot read properties of undefined (reading 'filter')'. sessions.own (crash recovery and rebuilds) decodes the same way.
- Reachability: Any session file under .ludenea/sessions/ written by an older editor build (SavedSession gained branches, document, and keeps_assets in 2b), edited by hand, or copied between machines. The project's start document then never opens either (project/index.tsx:55 swallows the error), and nothing tells the user which file to delete.
- Recommendation: Give the stored format a version ({ format: 1, saved }) and decode it through a validating codec that returns a result instead of throwing. When a stored session cannot be read, open the document from its file. Log a warning naming the file and the reason ('The stored session of scenes/main.scene.ron (.ludenea/sessions/<id>.session) cannot be read: <reason>; the document opened from its file'), and set the bad file aside as <id>.session.unreadable. Apply the same path in sessions.own.
- First test suggested by the auditor: editor/packages/editor-shell/test/document.test.ts: a_stored_session_that_does_not_decode_opens_the_document_from_its_file_with_a_warning, parametrized over not base64, not JSON, an older shape, and a future format version.
- Verifier notes (binding scope corrections): The mechanism is real. I re-ran scratchpad/editor-ts/stored.test.ts and all three cases refuse the open. decode at document.ts:160 is an unchecked cast. stored() is called at document.ts:305 inside open, before the load, so a decode failure (atob, JSON.parse, or `refreshed` reading saved.load.assets at :247) rejects the whole open with 'Cannot open <path>: Invalid character'. The open's own commen...

#### Finding readiness-13: A stored session that does not decode blocks opening its document

- Verified status: confirmed; final severity: low; rule: versioning; feature: stored sessions.
- Locations: editor/packages/editor-shell/src/stores/document.ts:154-160, editor/packages/editor-shell/src/stores/document.ts:238-241, editor/packages/editor-shell/src/stores/document.ts:305, crates/ludenea-project/src/files.rs:251-267
- Evidence: SavedSession is stored as base64 of JSON, with no version field. stored() decodes it inside open() without a catch (document.ts:305). A file that is not valid base64 or JSON (written by an editor version with another encoding, or edited by hand) therefore makes 'Cannot open <scene>' fail entirely, with a raw atob or JSON error. By contrast, a session the game refuses already falls back to opening the document from its file (document.ts:318-336). Here the user has to find and delete .ludenea/sessions/<id>.session.
- Reachability: An editor update that changes the stored session encoding, or a damaged file.
- Recommendation: Add a version field to SavedSession. On a decode failure or an unknown version, log a warning naming the stored session file and open the document from its file, as is already done for a refused session.
- First test suggested by the auditor: In editor/packages/editor-shell/test: load_stored_session answers with the bytes 'not base64!'. Assert that open() resolves, the document is open from its file, and the console has a warning naming the stored session.
- Verifier notes (binding scope corrections): SavedSession (editor/packages/editor-shell/src/stores/document.ts:70-78) has no version field. decode (:159) runs atob and JSON.parse unguarded, and open() awaits stored(document) inside its try (:305) with no catch. A bad file, or one with another shape (refreshed() reads saved.load.keeps_assets at :252), therefore rejects the whole open with 'Cannot open <path>' and a raw error. A session the ga...

### Task 20: Project WebSocket limits deliberate; oversized messages refused precisely

Backlog item PRJ-3 (do not cite this identifier in commits or code). Workstream: PRJ: Projects, builds, serving and the template (grade C).

**Workstream context.**

Subsystem: ludenea-project, ludenea-serve, the project template.

**Problem.** Path confinement, atomic writes, the file watcher, build diagnostics and serve's host and origin checks are strong, but the Vite dev proxy forges Origin and voids serve's cross-site protection, opening another project loads its start document into the previous project's game, a project message above 16 MiB silently drops the connection, a closed project's build keeps broadcasting, ludenea.toml is unversioned and unlocated, serve refusals are unexplained, new projects get none of the engine's determinism enforcement, and the game's TypeScript types exist only after an editor connects to a running game.

**Target state.** The development setup keeps the security model; project switches end the previous game and build; message limits are deliberate and explained; the manifest is strict; new projects inherit the engine's lints, line-ending rules, overflow checks and lockfile; types exist from project creation.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: projects-4.
- Depends on: nothing.
- Locations and files: `crates/ludenea-serve/src/serve.rs:94-104,188-193`, `editor/packages/editor-shell/src/stores/document.ts:155-159`.
- Acceptance: max_frame_size and max_message_size are set deliberately for the 100,000-entity target and documented; a refused message gets a precise error naming its size and the limit before the close, plus a server log line; the editor shows it (CTL-5 then reconnects).
- First failing test: crates/ludenea-serve/tests/serve.rs a_large_write_is_accepted_and_an_oversized_one_is_refused_precisely.

**Audit findings resolved by this task (full text).**

#### Finding projects-4: Browser editor: a project message above 16 MiB silently drops the connection (session stores after a tick-0 edit of a medium scene)

- Verified status: confirmed; final severity: critical; rule: feature-robustness; feature: serving the editor (project WebSocket).
- Locations: crates/ludenea-serve/src/serve.rs:94-104, crates/ludenea-serve/src/serve.rs:188-193, editor/packages/editor-shell/src/stores/document.ts:155-159, crates/ludenea-control/src/undo.rs:30, editor/apps/web/src/session-guard.ts:77-98
- Evidence: project_socket upgrades with axum's defaults, which are tungstenite 0.29's max_frame_size 16 MiB and max_message_size 64 MiB (tungstenite protocol/mod.rs:101-102). Browsers send each message as one frame. A larger frame makes socket.recv() return Err, and session() just returns (serve.rs:191), with nothing logged and no error sent. StoreSession carries base64(JSON(session)) (document.ts:155-159). The session holds the scene, the saved scene when dirty, and an undo history whose every tick-0 entry stores two full scene texts (undo.rs:30). guardSession stores after every edit (session-guard.ts:77-98). The e2e bench scene of 100,000 entities is 10.8 MB (target/editor-bench.scene.ron), so one tick-0 edit makes a session of about four scene copies, roughly 58 MB after base64. A scene of about 30,000 entities already crosses 16 MiB. The desktop (Tauri IPC) has no such limit, so the two editors behave differently.
- Reachability: Browser or remote editor users editing a medium or large scene at tick 0, saving a large scene, or saving a large replay (WriteBinary).
- Recommendation: Set deliberate limits on the WebSocketUpgrade (`.max_frame_size(..).max_message_size(..)`, sized for the 100,000-entity target with headroom) and document them. When a message is refused, answer with a precise error naming the size and the limit before closing, and log it on the server. Longer term, stop shipping full scene texts in every session store: store the undo history as diffs (the ponytail note in undo.rs) or send sessions as binary frames.
- First test suggested by the auditor: tests/serve.rs 'a_large_write_is_accepted_and_an_oversized_one_is_refused_precisely': send a 20 MiB WriteText in one frame and expect Done, then send a message above the documented limit and expect a Response or Refused naming the limit (not a silent close).
- Verifier notes (binding scope corrections): The defect holds, with one correction to the mechanism. axum 0.8.9 uses tungstenite 0.29 defaults: max_frame_size 16 MiB and max_message_size 64 MiB (protocol/mod.rs:101-102). An error from socket.recv() makes session() return silently (serve.rs:191). The client then sees only 'connection closed: closed (1006)', and the project connection never comes back (projects-5). Correction: Chromium splits ...

### Task 21: The open scene follows renames and removals on disk

Backlog item AST-2 (do not cite this identifier in commits or code). Workstream: AST: Project assets and changes made outside the editor (grade C).

**Workstream context.**

Subsystem: metas, asset listing, file watching, open documents.

**Problem.** Writes are atomic and the watcher is robust, but one duplicated asset (file and meta copied together) makes every document unopenable, the command line tool and the editor build the asset set with two different walkers, renaming the open scene outside the editor drops its live session, files moved or metas deleted outside the editor silently get new identities, meta-only changes emit no event, Finder and editor temporary files get metas, and Save overwrites a pending disk change without a decision.

**Target state.** One asset-set policy shared by every tool, where a problem file costs only itself; identities survive moves, copies and lost metas; the open document follows its file; Save never silently overwrites a change made on disk.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: gap-2-4, readiness-10.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/assets/index.tsx:174-188`, `editor/packages/editor-builtins/src/scene-tree/index.tsx:28-29`, `editor/packages/editor-shell/src/stores/document.ts:292-297,454-460`, `editor/packages/editor-builtins/test/assets.test.tsx:251-262`.
- Acceptance: On files_changed the asset browser refreshes and, when the listing holds the open identifier at a new path, the document follows it. When a created file holds the document's text, it follows that file (reconnecting the orphan meta once AST-3 lands). When the file is really gone, the session is stored first and the document stays open as deleted on disk with Save again and Close. The Save message never advises an action that throws the session away.
- First failing test: editor-builtins assets.test.tsx an_open_scene_renamed_outside_with_its_meta_stays_open_at_its_new_path and a_dirty_scene_removed_on_disk_stays_open_with_a_banner.

**Audit findings resolved by this task (full text).**

#### Finding gap-2-4: Renaming the open scene outside the editor drops its live session: Save refuses, and reopening the renamed file loses unsaved edits, recorded ticks, undo history and branches

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: editor documents and sessions.
- Locations: editor/packages/editor-builtins/src/assets/index.tsx:184-187, editor/packages/editor-builtins/src/scene-tree/index.tsx:28-29, editor/packages/editor-shell/src/stores/document.ts:292-297, editor/packages/editor-shell/src/stores/document.ts:454-460, editor/packages/editor-builtins/test/assets.test.tsx:251
- Evidence: Live: I placed a prefab instance (unsaved marker shown), played to tick 92 and paused, then ran `mv scenes/main.scene.ron scenes/level.scene.ron`, a Finder rename that leaves the meta behind. The '(open)' marker disappeared, while the game still ran the session (7 dynamic bodies, tick 92). 'Save scene' answered 'Nothing saved: no document is open (the open one was removed or moved); open a document first.' Following that advice and double-clicking level.scene.ron gave 'Tick 0 of 0', 6 bodies and a disabled Undo: the edit, the 92 recorded ticks and the history were gone, with no message. The cause: document.ts:292-297 stores the previous session only when a document is current, and sessions.store keeps only the current document's session (document.ts:459). When the meta moves too, so the identifier is unchanged, the document still closes. The editor recovers only because reopening finds the session stored at the last pause; edits made between the rename and the reopen are not stored. This is not the removal case: the file still exists, and the listing shows where it went. assets.test.tsx:251 pins the close; nothing tests following a rename.
- Reachability: Renaming or moving the open scene or prefab in Finder, VS Code, a terminal, or through a git checkout while the editor is open.
- Recommendation: In the files_changed handler (assets/index.tsx:174), refresh the listing, then follow the document instead of closing it. If the listing holds current.id at a new path, set current.path to it, as the in-editor rename does at index.tsx:142-143. Otherwise, if a file created in the same batch holds documents.text, reconnect the orphan meta (AdoptMeta, from the move finding) and follow that file. Only when the file is really gone: store the session first, keep it as a detached document, and offer 'Save scene as' instead of dropping current. Reword the Save message so it never advises an action that throws the session away.
- First test suggested by the auditor: editor/packages/editor-builtins/test/assets.test.tsx an_open_scene_renamed_outside_with_its_meta_stays_open_at_its_new_path: a batch with Removed main and Created level, a listing that returns the same id at scenes/level.scene.ron, and scene.save writes to scenes/level.scene.ron.
- Verifier notes (binding scope corrections): Confirmed by code. The files_changed handler closes the document on any removal of the open path that is not an in-editor rename (editor/packages/editor-builtins/src/assets/index.tsx:184-187). The test removing_the_open_scene_on_disk_closes_it (editor/packages/editor-builtins/test/assets.test.tsx:251) even sends Removed plus Created moved.scene.ron in one batch and pins the close, so it is deliber...

#### Finding readiness-10: Removing the open scene on disk discards its unsaved edits without a word

- Verified status: partially; final severity: medium; rule: feature-robustness; feature: asset browser and documents.
- Locations: editor/packages/editor-builtins/src/assets/index.tsx:174-188, editor/packages/editor-builtins/test/assets.test.tsx:251-262
- Evidence: When a files_changed batch reports the open scene as removed (a git checkout or stash, a file manager), the asset plugin sets the current document to null (index.tsx:184-186), even when the scene is dirty. It neither stores the session nor offers a choice. The test removing_the_open_scene_on_disk_closes_it asserts exactly this with unsaved changes, and that Save then writes nothing. A changed file gets a banner with 'keep mine'; a removed one gets nothing.
- Reachability: Any external removal or move of the open scene while it has unsaved edits.
- Recommendation: Treat removal like an external change. With unsaved edits, keep the document open as 'deleted on disk', with a banner offering 'Save it again' (write_text to the same path) and 'Close'. Without unsaved edits, close it as today. In both cases, store its session before closing.
- First test suggested by the auditor: Change removing_the_open_scene_on_disk_closes_it in assets.test.tsx: with scene_dirty true, assert that a banner names the removed file and that scene.save writes the scene back to its path.
- Verifier notes (binding scope corrections): The code is as described. On a Removed change for the open path, editor/packages/editor-builtins/src/assets/index.tsx:183-187 sets current to null even when the scene is dirty, shows nothing, and does not store the session. The test at editor/packages/editor-builtins/test/assets.test.tsx:251-262 pins this behaviour. The finding overstates the loss, though. In the web app, the session guard (editor...

### Task 22: Error boundaries around every plugin-rendered slot

Backlog item PLG-1 (do not cite this identifier in commits or code). Workstream: PLG: Editor plugin system (grade D).

**Workstream context.**

Subsystem: editor-shell plugin API, loader and registry; plugin bundling.

**Problem.** The loader's rollback, the registry's ownership and the bundler are good, but one plugin component that throws while rendering unmounts the whole editor, built-in plugins coordinate through private WeakMap channels project plugins cannot reach (the 'no privileged API' anti-goal), inspectors and overlays are not observable so hot reload leaves stale ones, registrations made after an await leak, runtime errors carry no source location, settings are half done, and the typed API covers reads only.

**Target state.** Every plugin slot is isolated by an error boundary; built-ins use only the public API (edits, console, viewport, assets, build state, scene tree groups); every registration kind is observable; registrations belong to their plugin whatever the time of the call; errors point at source lines; settings have a panel; writes are typed.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: plugins-1, errors-4, ts-quality-1.
- Depends on: ERR-1.
- Locations and files: `editor/packages/editor-shell/src/layout/Shell.tsx:15-27`, `editor/packages/editor-shell/src/layout/mount.tsx:9-12`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:257-275`, `editor/packages/editor-builtins/src/inspector/fields/Field.tsx:26-46`, `editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:38-44`, `editor/packages/editor-builtins/src/viewport/overlay.ts:53-59`.
- Acceptance: A PluginBoundary in editor-shell wraps every panel, custom inspector (falling back to the schema form), decoration and explorer cell. The fallback names the panel and the plugin, shows the error, offers Reload panel and resets on a new registration. Failures reach editor.console once. mountShell passes onUncaughtError and onCaughtError. A failing overlay is reported once, not on every frame.
- First failing test: shell.test.tsx a_panel_that_throws_while_rendering_leaves_the_other_panels_and_the_palette_working.

**Audit findings resolved by this task (full text).**

#### Finding plugins-1: A plugin component that throws while rendering unmounts the whole editor

- Verified status: confirmed; final severity: critical; rule: feature-robustness; feature: editor plugin isolation.
- Locations: editor/packages/editor-shell/src/layout/Shell.tsx:15-27, editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:257-274, editor/packages/editor-builtins/src/inspector/fields/Field.tsx:26-36, editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:38-44, editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:83-86, editor/packages/editor-shell/src/layout/mount.tsx:9-12
- Evidence: No React error boundary exists anywhere in editor/ (grep for ErrorBoundary, getDerivedStateFromError and componentDidCatch finds nothing). Plugin code renders directly inside the shell's single createRoot:
- PluginPanel renders `<panel.component editor=.../>` (Shell.tsx:26).
- The inspector renders `<Custom value=... />` (InspectorPanel.tsx:274).
- Decorations render beside every field (Field.tsx:35-36).
- Explorer columns call `column.value(row.entity)` during render (ExplorerPanel.tsx:84-85), and `column.subscribe` runs in an effect.
Reproduced in scratchpad/plugins/vt/repro.test.tsx: a Shell holds a healthy panel, then a plugin registers a panel that throws. Afterwards the healthy panel is gone and document.body is reduced to `<div></div>` (11 characters).
The Console panel and the command palette live in the same root, so the user sees a blank page with no explanation. If the panel is visible at start, the editor is unusable until the plugin source is fixed.
- Reachability: Ordinary use. A project or engine plugin whose panel or custom inspector has any bug: reading a field the game renamed, `value.kind.toUpperCase()` on undefined, a column throwing on an entity without the component.
- Recommendation: Add one `PluginBoundary` error boundary in editor-shell. It catches the error, renders an inline message with the plugin name and the error, and offers a Retry button that resets on a new registration. Report the error once to the Console panel. Wrap every place that renders plugin-provided code in it:
- PluginPanel, keyed by the registration so a reload resets it;
- the custom inspector in Section, with a fallback to the schema-driven Field;
- each Decoration;
- each explorer column cell (try/catch around `value`, showing an error marker).
Change the overlay path to report a failing overlay once rather than at every frame. The target state: a broken plugin component costs only its own slot.
- First test suggested by the auditor: editor-shell test/shell.test.tsx `a_panel_that_throws_while_rendering_leaves_the_other_panels_and_the_palette_working`. Activate a healthy panel plugin and a throwing one, render Shell, then expect the healthy panel's text, an error message naming the throwing plugin, and that Ctrl+K still opens the palette. Add the same test for a custom inspector in editor-builtins/test/inspector.test.tsx (expect the generic fields as the fallback).
- Verifier notes (binding scope corrections): No error boundary exists anywhere in editor/: grep for ErrorBoundary, getDerivedStateFromError, componentDidCatch and the createRoot onUncaughtError/onCaughtError options finds nothing. mountShell (mount.tsx:9-12) creates a single root. Plugin code renders unguarded in that root: PluginPanel (Shell.tsx:26), the custom inspector (InspectorPanel.tsx:257/274), decorations (Field.tsx:35-45), and explo...

#### Finding errors-4: No React error boundary: a plugin panel or custom inspector that throws while rendering unmounts the whole editor

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: editor shell, plugins.
- Locations: editor/packages/editor-shell/src/layout/Shell.tsx:15-27, editor/packages/editor-shell/src/layout/mount.tsx:9-12, editor/packages/editor-shell/src/plugin.ts:29, editor/packages/editor-shell/src/plugin.ts:72-73
- Evidence: `grep ErrorBoundary|componentDidCatch|getDerivedStateFromError` over editor/packages and editor/apps finds nothing. The shell is a single `createRoot(element).render(<Shell/>)` (mount.tsx:10-11). PluginPanel renders `<panel.component editor={host.editor} />` directly (Shell.tsx:26), and dockview panels portal into that same root. Custom inspectors are rendered components too (plugin.ts:73). With React 19.3.1, a render error that no boundary catches unmounts the whole root. Project plugins are re-bundled and hot-reloaded on save (README 'Plugins'), so one typo in a plugin panel's render blanks every panel, including the timeline and scene tree, while the game keeps running in the worker.
- Reachability: A project plugin author saving a panel or inspector with a render-time bug (ordinary plugin development), or a plugin meeting an unexpected component value from the game.
- Recommendation: Wrap each `<panel.component>` in PluginPanel, and each registered inspector or decoration render, in a small class ErrorBoundary. Its fallback names the panel and plugin, shows the message, and offers 'Reload panel' (reset key). componentDidCatch reports to `editor.console.report` (see the console API finding). Reset the boundary when the plugin reloads (key on the registration identity).
- First test suggested by the auditor: editor/packages/editor-shell/test/shell.test.tsx: `a panel that throws while rendering shows a fallback and leaves the other panels mounted`. Register panel A whose component throws `new Error("boom")` and panel B with a text. Mount the shell and assert that B's text is still in the document, that A's fallback contains 'boom', and that a console entry is reported.
- Verifier notes (binding scope corrections): `grep ErrorBoundary|componentDidCatch|getDerivedStateFromError|onUncaughtError` over editor/packages, editor/apps and editor/e2e finds nothing. mountShell is a single createRoot().render(<Shell/>) (mount.tsx:10-11). PluginPanel renders <panel.component> directly (Shell.tsx:26). dockview-react 8.6.0 renders panels through createPortal into the same root, and custom inspectors render as <Custom/> in...

#### Finding ts-quality-1: No error boundary: one throwing plugin panel, inspector, decoration, or explorer column blanks the whole editor

- Verified status: confirmed; final severity: critical; rule: no-panic; feature: editor shell / plugin isolation.
- Locations: editor/packages/editor-shell/src/layout/Shell.tsx:15-27, editor/packages/editor-shell/src/layout/mount.tsx:9-12, editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:257-275, editor/packages/editor-builtins/src/inspector/fields/Field.tsx:35-46, editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:83-86, editor/packages/editor-builtins/src/viewport/overlay.ts:53-59
- Evidence: No ErrorBoundary, getDerivedStateFromError, or onUncaughtError exists in the editor (rg over packages and apps). PluginPanel renders `<panel.component editor={host.editor} />` unguarded, and createRoot is called without onUncaughtError. Scratch reproduction scratchpad/editor-ts/boundary.test.tsx registers a panel rendering <p>good panel</p> and one that throws, then renders Shell: render throws 'plugin bug', the container's innerHTML length becomes 0, and the good panel is gone. Custom inspectors (InspectorPanel.tsx:274), decorations (Field.tsx:45), and explorer column values (ExplorerPanel.tsx:85) run plugin code during render in the same unguarded way. Only viewport overlays are isolated (overlay.ts:54-58). Built-in panels also cast game JSON by schema during render (for example Field.tsx:86 and 94), so a value that does not match its schema reaches the same failure.
- Reachability: A project plugin author saves a panel or custom inspector with a render bug: hot reload activates it, the whole UI unmounts, and the fixed rebuild cannot remount it because the React root is gone; the page must be reloaded (in the browser up to one second of play is lost).
- Recommendation: Add a PluginBoundary class component in editor-shell that wraps each PluginPanel, each custom inspector, each decoration, and each explorer cell. The registry already knows the owner. The boundary shows 'The <title> panel of plugin <owner> failed: <message>' with a 'Reload panel' button, reports to the console sink (see the privileged-API finding for editor.console), and resets itself when the registration changes, so a fixed hot reload remounts the panel. Pass onUncaughtError and onCaughtError to createRoot in mountShell so nothing reaches the root unreported.
- First test suggested by the auditor: editor/packages/editor-shell/test/shell.test.tsx: a_throwing_plugin_panel_shows_its_error_and_leaves_the_other_panels_alive (register 'good' and 'bad' panels, render Shell, expect 'good panel' visible and an alert naming 'bad' and its plugin; re-register 'bad' with a fixed component and expect it renders).
- Verifier notes (binding scope corrections): The claim holds. I re-ran scratchpad/editor-ts/boundary.test.tsx and it fails: after one panel throws, the container is empty and the good panel is gone. The codebase has no ErrorBoundary, getDerivedStateFromError or onUncaughtError. Shell.tsx:26 renders `<panel.component>` unguarded, and mount.tsx:10 calls createRoot with no error options (React 19.3.1, where an uncaught render error unmounts the...

### Task 23: Explorer lists virtualized in the real dock

Backlog item INT-1 (do not cite this identifier in commits or code). Workstream: INT: Explorer and introspection (grade D).

**Workstream context.**

Subsystem: Explorer panel, introspection protocol and subscriptions.

**Problem.** Subscriptions are reference-counted, records are hash-validated and the filter parser is fully covered, but in the real dock the Entities list is not virtualized (it renders every row and freezes the UI at 100,000 entities), Events and Commands answer only from a passive 600-tick ring so ordinary states show impossible advice, events render as raw JSON with entities that cannot be clicked, an Entities watch rehashes every matching entity every frame even when paused, and most of these states have no test.

**Target state.** The Explorer scales in the real layout; any simulated tick can be introspected by re-running it; events read as fields with clickable entities; watches cost nothing when nothing changed; every state is tested.

**Item.**

- Priority P0, kind fix, effort M, golden impact none.
- Findings: gap-3-1, ux-live-18.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/panels.css:34-67`, `editor/packages/editor-builtins/src/shared/EntityList.tsx:36-78`, `editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:27,46-89`, `editor/e2e/performance.spec.ts`.
- Acceptance: The explorer tabpanel is a column flex child (flex 1, min-height 0) and .ludenea-entities uses flex 1 and min-height 0 instead of height 100 percent; rows stay on one line with ellipsis; EntityList caps the pages one refresh may request to the visible range plus overscan; the listbox is wider than half the panel.
- First failing test: editor/e2e/performance.spec.ts the_explorer_lists_100000_entities_with_only_the_visible_rows_while_playing (fewer than 100 options, frames within budget).

**Audit findings resolved by this task (full text).**

#### Finding gap-3-1: The Explorer's Entities list is not virtualized in the real dock: at 100,000 entities it renders every row and freezes the UI while playing

- Verified status: confirmed; final severity: critical; rule: feature-robustness; feature: explorer entities.
- Locations: editor/packages/editor-builtins/src/panels.css:34-49, editor/packages/editor-builtins/src/panels.css:64-67, editor/packages/editor-builtins/src/shared/EntityList.tsx:36-39, editor/packages/editor-builtins/src/shared/EntityList.tsx:62-78, editor/packages/editor-builtins/test/explorer.test.tsx:10-13
- Evidence: Root cause in the CSS. panels.css:42 puts `.ludenea-explorer [role="tabpanel"]` in the same rule as the entity rows (display:flex in a row, with a gap). The tabpanel gets neither flex:1 nor min-height:0, so `.ludenea-entities { height:100% }` (panels.css:34-40) resolves against a parent with automatic height, and `.ludenea-entities-list` (64-67) grows to the full content height. Measured in the web editor (ludenea serve, port 7933) on target/editor-bench.scene.ron with scratchpad/introspection-explorer/ux/steps/10.js: the scroller is 2,200,000 px tall and the DOM holds 100,000 role=option rows. The virtualizer therefore spans pages 0 to 499 (EntityList.tsx:38-39), and every refresh sends 500 query_entities requests of 200 rows each (EntityList.tsx:64-65). A refresh happens on every animation frame while playing (EntityList.tsx:46-53). Results from ux/steps/09.js: playing with Explorer > Entities shown gave 14 frames in 6.3 s with a worst frame of 2,050 ms, and the UI stayed frozen after Pause (3 frames in 2.1 s). The same EntityList in the scene tree gave 301 frames in 5 s, worst 17 ms. The unit test list_renders_only_visible_rows_for_100000_entities passes only because explorer.test.tsx:10-13 gives every element an offsetHeight of 400, and no Playwright spec opens the Explorer (grep of editor/e2e). This is a different symptom from the existing 'rows overlap' finding, though they may share this CSS cause.
- Reachability: Open the bench scene (or any large scene), show the Explorer (bottom dock by default), stay on Entities, and press Play.
- Recommendation: Take the tabpanel out of the row-flex rule and make it a column flex child: `.ludenea-explorer [role="tabpanel"] { flex: 1; min-height: 0; display: flex; flex-direction: column; }`, and give `.ludenea-entities` flex:1 and min-height:0 instead of height:100%. As a guard, have EntityList cap how many pages one refresh may request (the visible range plus overscan), so a layout bug cannot turn into 500 requests per frame.
- First test suggested by the auditor: Playwright test in editor/e2e/performance.spec.ts: 'the explorer lists 100,000 entities with only the visible rows while playing'. Open bench.scene.ron, show Explorer > Entities, assert `.ludenea-explorer [role=option]` count < 100 and the list's clientHeight smaller than the viewport, then play 5 s and assert p95 frame <= 17 ms under PERF_STRICT. It fails today with 100,000 options.
- Verifier notes (binding scope corrections): The code matches the finding. panels.css:42-48 gives `.ludenea-explorer [role="tabpanel"]` the same row-flex rule as the entity rows, with no flex:1 and no min-height:0. ExplorerPanel.tsx:27 wraps Entities in that tabpanel, so `.ludenea-entities { height:100% }` (panels.css:34-40) resolves against an automatic height. The built dist CSS (editor/apps/web/dist/assets/index-COFZz92H.css) has the same...

#### Finding ux-live-18: Explorer entity rows overlap: the list collapses to about 180px

- Verified status: confirmed; final severity: medium; rule: ux; feature: explorer.
- Locations: editor/packages/editor-builtins/src/panels.css:42-48, editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:46-89
- Evidence: `.ludenea-explorer [role="tabpanel"]` is grouped with the row-flex rule (display:flex, row direction), and its child `.ludenea-entities` does not grow. The list is about 180px wide in a 1440px panel, so each 22px virtual row wraps the entity id and hash onto two lines that overlap the next row. The 'Diverged at' header sits over the hashes (shots/48-explorer.png).
- Reachability: Anyone opening the Explorer panel.
- Recommendation: Give the tabpanel `flex-direction: column; flex: 1; min-height: 0` and its child `flex: 1; min-width: 0`. Keep row content on one line (white-space: nowrap with ellipsis).
- First test suggested by the auditor: editor/e2e/panels.spec.ts: open Explorer, assert the 'Matching entities' listbox is wider than half the panel and the first option's height equals the row height.
- Verifier notes (binding scope corrections): panels.css:42-48 puts `.ludenea-explorer [role="tabpanel"]` in the same rule as the row flex layout (display:flex, row direction). The tabpanel gets no flex:1, and its child `.ludenea-entities` (panels.css:34-40) gets no flex-grow and no min-width:0. The virtualised rows are absolutely positioned (EntityList.tsx:109) and add no width, so the column shrinks to the width of the search input, about 1...

### Task 24: Multi-entity edits are one batch

Backlog item AUT-1 (do not cite this identifier in commits or code). Workstream: AUT: Scene authoring in the editor (grade D).

**Workstream context.**

Subsystem: the level-building workflow across scene tree, inspector, viewport and documents.

**Problem.** Tick-0 edits, undo and the scene format are sound, but a designer cannot build a level comfortably: multi-entity commands send one edit per entity (after playing, only the first applies, and each entity is its own undo step), no command creates an entity, Add component inserts schema zeros the game refuses (Pose and Collider) or that misbehave (Body without gravity), the selection holds runtime handles that point at another entity after a reload, there is no New scene, Save as, copy, paste or duplicate, entities have no names, and no test builds a scene the way a designer does.

**Target state.** Multi-entity edits are atomic batches; creating, naming, duplicating and saving entities and scenes are first-class commands; components start from the game's defaults; the selection follows scene entities by load address; an end-to-end test builds a level from nothing.

**Item.**

- Priority P0, kind deep-refactor, effort L, golden impact none.
- Findings: gap-1-1.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/protocol.rs`, `crates/ludenea-control/src/host.rs`, `crates/ludenea-control/src/session.rs:565-577,1950-1985`, `crates/ludenea-app/src/edit.rs`, `editor/packages/editor-builtins/src/scene-tree/index.tsx:19`, `editor/packages/editor-builtins/src/viewport/index.tsx:49`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:215-219`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:26`.
- Acceptance: Command::Batch { commands } applies as one scene edit at tick 0 (one undo entry, one resimulation, all or nothing) and as one timed edit later. Despawn selection, nudge, viewport drag and inspector edits over a multi-selection send one batch. The inspector shows N entities selected with their shared components (mixed values marked) and edits all of them through one batch.
- First failing test: crates/ludenea-control/tests/editing.rs a_batch_of_tick_zero_despawns_after_play_is_one_undo_step, and scene-tree.test.tsx despawning_three_selected_entities_sends_one_batch.

**Audit findings resolved by this task (full text).**

#### Finding gap-1-1: Multi-entity commands send one edit per entity: after playing, only the first applies, and each entity is its own undo step

- Verified status: confirmed; final severity: critical; rule: root-cause; feature: scene tree, viewport, inspector (multi-selection editing).
- Locations: editor/packages/editor-builtins/src/scene-tree/index.tsx:19, editor/packages/editor-builtins/src/viewport/index.tsx:49, editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:215-219, editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:26, editor/packages/editor-builtins/src/inspector/index.tsx:31-37, crates/ludenea-control/src/session.rs:1648-1653, crates/ludenea-control/src/session.rs:2089-2119
- Evidence: `Despawn selected entities` and `Nudge the selection` both send `Promise.all(entities.map(edit))`, one request per entity. At tick 0 with a recorded future, the first edit calls start_resimulation, which sets `resimulating = Some(0)`; every later edit then fails ensure_resimulated. Live, after playing 181 ticks and seeking to 0 with 3 crates selected: Nudge up moved only 1v0 (y 3 -> 3.1), and 2v0 failed with "the session is resimulated up to tick 0 only; wait for resimulation to finish" (steps/a08.js). Delete despawned 1 of 3 (steps/a10.js). With no recorded future, one nudge of 3 entities left 3 separate undo steps (steps/a24.js). The inspector edits `selection.entities[0]` only and shows no selection count: with 3 crates selected, setting gravity_scale changed 2v0 only, while 1v0 and 3v0 stayed at 1 (steps/a06.js, shots/a04-three-selected.png). A viewport drag moves only the entity under the pointer, while nudge moves them all.
- Reachability: Ordinary use: select several entities in the scene tree or viewport after playing the game, return to tick 0, then press Delete, an arrow key, or edit a field.
- Recommendation: Make a multi-entity operation one edit at protocol level. Add `Command::Batch { commands: Vec<Command> }`, or an `Edit::Group(Vec<Edit>)` the host applies at tick 0 as one scene edit: one UndoEntry::Scene, one resimulation, and all-or-nothing on failure. Past tick 0 it becomes one TimedEdit. Send it from despawn-selection, nudge, viewport drag (move every selected entity by the same offset), and the inspector. The inspector should show "N entities selected" with the components they share (mixed values shown as such); a field change sends one batch of SetField over every selected entity. Bump the protocol version and regenerate the bindings.
- First test suggested by the auditor: crates/ludenea-control/tests/editing.rs `a_batch_of_tick_zero_despawns_after_play_is_one_undo_step`: load the boxes scene, step 100, seek 0, send one Batch of three Despawn, then assert all three are gone and the timeline offers a single undo that restores them. Then editor-builtins/test/scene-tree.test.tsx `despawning_a_multi_selection_sends_one_batch`, with a fake game that answers a second edit request with code `resimulating`.
- Verifier notes (binding scope corrections): The code matches the finding, and the most common case is worse than reported. scene-tree/index.tsx:19 and viewport/index.tsx:49 send one edit per entity through Promise.all, and timeline/editing.tsx:20-29 sends each one at once. session.rs:565-577 sends every tick-0 edit to record_scene_edit, and edit_scene (session.rs:1950-1985) reloads the scene, so indices follow file order. With a recorded fu...

### Task 25: The desktop app builds on Windows: full icon set

Backlog item DSK-1 (do not cite this identifier in commits or code). Workstream: DSK: Desktop application (grade D).

**Workstream context.**

Subsystem: editor/apps/desktop (Tauri backend and entry).

**Problem.** The backend is a minimal relay with a tight attack surface and careful rebuild handover, but it does not build for Windows (icon.ico is missing), has no test at any level, cannot replace a running game's executable on Windows (rebuilds fail, consoles flash), is never built or packaged in production form, loses startup diagnostics, and closes without storing the session.

**Target state.** A tested backend library green on three operating systems; games run from private copies; production bundles built in CI; startup failures and window closing handled.

**Item.**

- Priority P0, kind fix, effort S, golden impact none.
- Findings: desktop-1.
- Depends on: nothing.
- Locations and files: `editor/apps/desktop/src-tauri/icons/`, `editor/apps/desktop/src-tauri/tauri.conf.json:18`.
- Acceptance: Icons generated with tauri icon from one source (icon.ico, icon.icns, PNG sizes) are committed and listed in bundle.icon; the desktop job is green on macos-14, ubuntu-24.04 and windows-2025 (locally through act while the repository has no remote).
- First failing test: n/a (tooling): the windows-2025 desktop clippy step fails today and must pass.

**Audit findings resolved by this task (full text).**

#### Finding desktop-1: The desktop editor does not build for Windows: icons/icon.ico is missing

- Verified status: confirmed; final severity: high; rule: feature-robustness; feature: desktop packaging.
- Locations: editor/apps/desktop/src-tauri/icons/icon.png, editor/apps/desktop/src-tauri/tauri.conf.json:18, editor/apps/desktop/src-tauri/build.rs:2, ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-build-2.7.3/src/lib.rs:793-870, .github/workflows/determinism.yml:125-161
- Evidence: Only icons/icon.png is tracked (git ls-files), and tauri.conf.json sets no bundle.icon. For a Windows target, tauri-build looks for the first .ico in bundle.icon, falls back to icons/icon.ico, and returns an error when that file is missing (tauri-build lib.rs:797-870). Reproduced: I ran the crate's own compiled build script (src-tauri/target/debug/build/ludenea-desktop-3f21997c89c3fdf2/build-script-build) on a scratch copy of the crate. With TARGET=aarch64-apple-darwin it exits 0. With TARGET=x86_64-pc-windows-msvc it exits 1 with: "`.../icons/icon.ico` not found; required for generating a Windows Resource file during tauri-build". The desktop CI job lists windows-2025 and runs cargo clippy and cargo build on this crate, so it would fail there. It has never run: `git remote -v` is empty. The README and AGENTS.md list Windows as a supported platform, and main.tsx:7 already contains Windows-specific code.
- Reachability: Anyone who builds or runs `tauri dev` for the desktop editor on Windows: the build fails before anything compiles.
- Recommendation: Generate the full icon set from one source image with `pnpm -C editor/apps/desktop tauri icon <source.png>` (icon.ico, icon.icns, PNG sizes). Commit the set and list it in tauri.conf.json bundle.icon. Then get the desktop CI job green on all three operating systems by pushing to a remote or running it locally with act. Keep the job required, so a missing platform resource can never return silently.
- First test suggested by the auditor: n/a (tooling): the windows-2025 leg of the desktop CI job (`cargo clippy --manifest-path editor/apps/desktop/src-tauri/Cargo.toml --locked -- -D warnings`) fails today and must pass after the fix
- Verifier notes (binding scope corrections): The finding holds as stated. git ls-files shows only icons/icon.png (151 bytes) under src-tauri/icons, and tauri.conf.json has no bundle.icon (line 18 is just `"bundle": { "active": false }`). For a Windows target, tauri-build-2.7.3/src/lib.rs:793-866 looks for the first .ico in bundle.icon, falls back to icons/icon.ico, and returns an error when that file is missing. So the build script fails on ...

### Task 26: One panic boundary around game code

Backlog item RUN-2 (do not cite this identifier in commits or code). Workstream: RUN: Simulation loop, inputs and game-code isolation (grade C).

**Workstream context.**

Subsystem: ludenea-app simulation, inputs, parallel runtime; game panics across host, command line tool and worker.

**Problem.** Input validation, the pacer, the deterministic RNG and Parallel's ordering are solid, but an oversized thread count panics every tool, game code panics are caught call site by call site (an input mapping or a value codec kills the native game process, the command line tool reports a bare panic), live input is judged idle on raw thresholds (unmapped keys fork the future), edits on runtime-spawned entities can retarget another entity, the player count cannot be set, the tick contract is hand-written in nine loops, and the template never reads its input.

**Target state.** Pool creation fallible end to end; one panic boundary around game code with tick and system; the game's mapping defines idleness; runtime entities addressed stably; a Run type owns the logs and the tick contract; a logging API for game code.

**Item.**

- Priority P0, kind deep-refactor, effort L, golden impact none.
- Findings: runtime-3, protocol-2, rust-quality-2, replays-10, errors-12, dx-live-11.
- Depends on: RUN-1.
- Locations and files: `crates/ludenea-app/src/simulation.rs:159-168,298-306`, `crates/ludenea-ecs/src/schedule.rs:218-237`, `crates/ludenea-app/src/replay.rs:39-55`, `crates/ludenea-control/src/host.rs:123-138,552-556,654-659,848-860`, `crates/ludenea-control/src/session.rs:393-405,1663-1680,2189-2195`, `crates/ludenea-control/src/compare.rs:75,201`, `crates/ludenea-control/src/divergence.rs:250`, `crates/ludenea-cli/src/commands.rs:71-103`, `crates/ludenea-control/src/worker.rs:44-56`.
- Acceptance: The Simulation methods that run game code (stepping, map_raw_input, debug_draw, pick, value codecs and asset defaults reached through the host) catch unwinding natively and return SimError::Panicked { tick, system, message, location }, the schedule recording the running system. Host::handle and Host::advance have one boundary that replies SystemPanicked and reloads the session to a consistent tick. play_replay and record_replay return ReplayError::Panicked, so the command line tool prints the game panicked at tick 3 in system boom: ... and writes dumps (exit 2). The per-site wrappers in session, compare and divergence are deleted. Prefab panics show the prefab path, not a hexadecimal id.
- First failing test: crates/ludenea-control/tests/running.rs a_panicking_input_mapping_fails_set_live_input_and_keeps_the_host, host.rs a_panicking_try_from_in_set_field_is_reported_and_the_hash_is_unchanged, and cli.rs game_panic_names_the_tick_and_system.

**Audit findings resolved by this task (full text).**

#### Finding runtime-3: Game-code panics are caught call site by call site: the input mapping kills the game process, and the CLI reports a bare panic without tick or system

- Verified status: confirmed; final severity: high; rule: no-panic; feature: failure of game code (systems, input mapping).
- Locations: crates/ludenea-control/src/host.rs:552-553, crates/ludenea-app/src/simulation.rs:159-168, crates/ludenea-app/src/simulation.rs:298-306, crates/ludenea-control/src/session.rs:398, crates/ludenea-control/src/session.rs:1662-1678, crates/ludenea-control/src/host.rs:849, crates/ludenea-control/src/compare.rs:75, crates/ludenea-control/src/compare.rs:196-206, crates/ludenea-control/src/divergence.rs:250, crates/ludenea-control/src/stdio.rs:55-66, crates/ludenea-cli/src/commands.rs:84-90, editor/apps/web/src/games.ts:84-86, editor/apps/web/src/games.ts:119-121
- Evidence: Probe (simloop/probe/src/main.rs) with the mapping `Move { x: raw.gamepads[0].axes[0] }`: `Host::handle(SetLiveInput { keyboard only })` raises 'index out of bounds: the len is 0 but the index is 0', and the panic escapes Host::handle. serve_stdio has no guard, so the native game process exits. games.ts:120 installs no restart on the desktop ('the native host recovers from panics itself'), so the game stays dead until the next rebuild. In the browser, the worker crashes. CLI probe (simloop/probe/src/bin/panics.rs: a system that indexes an empty Vec at tick 3): `run --ticks 10 --hash` prints only "thread 'main' panicked at src/bin/panics.rs:6:22: index out of bounds" and exits with 101: no tick, no system name, no dump. The editor reports the tick (SessionError::Panicked) but never the system. Six separate catch_unwind sites each wrap one entry point; map_raw_input and the CLI loops are not covered.
- Reachability: (b) a game developer whose mapping indexes `gamepads[0]` or `buttons[i]` (RawInput has no safe accessors), followed by (a) any key press in the editor; (a) any CLI run, or CI verify, of a game with a system that panics.
- Recommendation: Guard game code once, at the point where it is called. Simulation's methods that run game code (step_with, step_encoded, step_observed, map_raw_input, debug_draw, pick) catch unwinding in native builds and return `SimError::Panicked { tick, system: Option<&'static str>, message }`; the schedule records the name of the system it is running. Delete the per-site wrappers in session, host, compare and divergence; the session still restores its checkpoint when it gets that error. The CLI then prints 'the game panicked at tick 3 in system `boom`: index out of bounds' and writes dump snapshots as it does for a mismatch. Add `RawInput::key(code) -> bool`, `button(pad, index) -> f32` and `axis(pad, index) -> f32`, which return 0 when the pad or entry is absent, so mappings never need to index.
- First test suggested by the auditor: crates/ludenea-control/tests/host.rs: `panicking_input_mapping_is_refused_and_the_game_keeps_running`: SetLiveInput returns an error with code invalid_inputs that names the panic, and GetHash still answers afterwards. crates/ludenea-cli/tests: `run_reports_the_tick_and_system_of_a_panicking_system`.
- Verifier notes (binding scope corrections): Reproduced. In target/debug/simloop-probe, a mapping that indexes gamepads[0] panics inside Host::handle(SetLiveInput), and the panic escapes. map_raw_input (host.rs:553) is not under catch_unwind. The only guarded sites are session.rs:398 and 1670, compare.rs:75 and 201, divergence.rs:250, and host.rs:849. serve_stdio (stdio.rs:55-66) has no guard, so the native game process dies. games.ts:120 in...

#### Finding protocol-2: Game code that commands call outside catch_unwind kills the native host (input mapping, asset defaults, value codecs)

- Verified status: confirmed; final severity: high; rule: no-panic; feature: host.
- Locations: crates/ludenea-control/src/host.rs:552-556, crates/ludenea-app/src/simulation.rs:160-168, crates/ludenea-app/src/app.rs:156, crates/ludenea-control/src/host.rs:654-659, crates/ludenea-control/src/host.rs:1411-1424, crates/ludenea-control/src/host.rs:843-860, crates/ludenea-control/src/stdio.rs:54-75
- Evidence: `SetLiveInput` calls `sim.map_raw_input`, which runs the game's `App::input_mapping` function with no catch_unwind. `NewAssetValue` runs the asset type's `Default`, and Spawn/SetField/InsertComponent run the codec's `from_json`, also unguarded. Meanwhile `Pick` (host.rs:848-860), `GetDebugDraw` (caught_draw), and stepping (session.rs:1670) do catch game panics. Reproduced with a scratch game (/private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/control/panicgame/src/main.rs) whose mapper reads `raw.gamepads[0].axes[0]`: hello, play, then set_live_input with one player and no gamepad makes `host --stdio` exit 101 with `index out of bounds: the len is 0 but the index is 0`. The Play response in the same batch was never written, because a batch is written only after every line in it has been handled (stdio.rs:54-75).
- Reachability: Ordinary use: a game developer whose mapper assumes a gamepad presses a key while playing in the editor. On desktop the game process dies (see the desktop finding); in the browser every key press crashes the worker until the restart budget stops the game.
- Recommendation: Add one `Host::guarded<T>(&mut self, what: &str, f: impl FnOnce(&mut Simulation) -> Result<T, ProtocolError>) -> Result<T, ProtocolError>` helper that wraps calls into game code in `catch_unwind(AssertUnwindSafe(..))` and maps a panic to `SystemPanicked` with `panic_message`, plus a console error event. Route through it every command path that reaches game code before it mutates the session: map_raw_input, encode_value/encode_json_value, default_asset_ron, create_prefab, override_ron, and asset from_json. Pick and caught_draw then use the same helper, so there is one way to do it.
- First test suggested by the auditor: tests/running.rs: `a_panicking_input_mapping_fails_set_live_input_and_keeps_the_host`. The app maps input with `|raw, _| Input(raw.gamepads[0].axes[0])`; SetLiveInput { players: [RawInput { keys: vec![], gamepads: vec![] }] } returns Err(SystemPanicked) naming "index out of bounds", and a following GetHash and Step succeed.
- Verifier notes (binding scope corrections): I confirmed it by reading the code. SetLiveInput (host.rs:552-556) calls Simulation::map_raw_input (simulation.rs:160-168), which calls the game's mapper through the closure built in app.rs:156, with no catch_unwind. Host::handle (host.rs:123-138) has no outer guard either. Pick (host.rs:848), caught_draw (compare.rs:201) and stepping (session.rs:1670) do catch game panics, so the policy is applie...

#### Finding rust-quality-2: A panic in game type code during an edit, load or inspection escapes Host::handle and kills the game process, along with the unexported session

- Verified status: partially; final severity: high; rule: no-panic; feature: control host (panic isolation).
- Locations: crates/ludenea-control/src/host.rs:123-138, crates/ludenea-control/src/host.rs:343-914, crates/ludenea-control/src/session.rs:1663-1678, crates/ludenea-control/src/session.rs:398, crates/ludenea-control/src/compare.rs:75, crates/ludenea-control/src/compare.rs:201, crates/ludenea-control/src/divergence.rs:250, crates/ludenea-control/src/host.rs:849, crates/ludenea-ecs/src/component.rs:66-75, crates/ludenea-control/src/stdio.rs:57, editor/packages/editor-shell/src/stores/document.ts:265-268
- Evidence: Panic boundaries exist only around running a tick, drawing, picking, following the compare branch, advancing the divergence report, and exporting a replay. `Host::handle` runs `execute` unguarded. Codec::encode and encode_ron panic on serializer errors (component.rs:66-75), and game Deserialize/TryFrom code runs inside apply_edit, scene loads and to_json.

Repro (/private/tmp/claude-501/-Users-jh3ady-Personal-Sandbox-game-engine-determinism/974fa27c-3eba-4973-a2f7-f54680a8691c/scratchpad/rust-quality/boundary/src/main.rs): a component `Health` whose `TryFrom` asserts `>= 0`. After LoadScene, `SetField` with -5 makes the panic unwind out of `Host::handle` (the program prints "the panic escaped Host::handle"). In `host --stdio` and the browser worker, that ends the game.

The editor exports the session only when switching documents or reloading assets (document.ts:265-268, 400). Inputs, edits, branches and undo history recorded since then are lost. The same game panicking inside a system is caught as SystemPanicked, with the session intact.
- Reachability: (a) An editor user types a value in the inspector (SetField, InsertComponent, Spawn, SetAssetField, ApplyOverride), loads a scene, session or replay, or inspects an entity (GetEntity → to_json) whose game-side serde or TryFrom code panics. That is a game in development, the editor's main use case.
- Recommendation: Put one panic boundary in Host::handle (and Host::advance) around `execute`. On a caught panic:
- reply ErrorCode::SystemPanicked "the game panicked while handling `set_field`: <message>; the session is back at tick N";
- write it to the console;
- restore consistency with `self.session.reload(&mut self.simulation)`, as the LoadReplay failure path already does (host.rs:791-797).
If the reload fails, refuse further state-changing commands with a precise error ("reload the document") rather than continue on an inconsistent state. Keep the inner boundaries only where they add a tick number to the message.
- First test suggested by the auditor: crates/ludenea-control/tests/host.rs: a game whose component's TryFrom panics on negative values. After LoadScene, `SetField` with -5 returns Err with code SystemPanicked, and a following GetHash returns the hash from before the command. Today the panic unwinds out of `handle`.
- Verifier notes (binding scope corrections): Holds: `Host::handle` (host.rs:123-138) calls `execute` with no panic guard. Re-running the boundary repro prints "the panic escaped Host::handle". `serve_stdio` (stdio.rs:58) has no guard either, so a panicking TryFrom or Deserialize in game code during SetField, LoadScene or GetEntity kills the stdio host. The only panic guards are around ticks, drawing and picking (session.rs:1670, compare.rs:7...

#### Finding replays-10: CLI run and verify do not catch game panics, so CI gets no tick

- Verified status: confirmed; final severity: medium; rule: no-panic; feature: run / verify (CLI).
- Locations: crates/ludenea-cli/src/commands.rs:71-81, crates/ludenea-cli/src/commands.rs:95-103, crates/ludenea-control/src/session.rs:393-405, crates/ludenea-control/src/session.rs:1670
- Evidence: crates/ludenea-cli/src has no catch_unwind. A panic in game code while recording or playing (for example Assets::get panics on an unloaded handle, ecs/src/assets.rs:222-225) aborts the CLI with Rust's panic text and exit 101, and says nothing about the tick or the replay. The control host catches the same panics and reports `SessionError::Panicked { tick, message }` (session.rs:398-405, 1670). The editor is therefore more precise than the CI tool on the same replay.
- Reachability: Verifying in CI a replay recorded before a refactor that now hits a panic path in game code; running any scene that makes a system panic.
- Recommendation: Wrap record_replay and play_replay in the CLI with catch_unwind(AssertUnwindSafe(..)) and report 'error: the game panicked at tick N: <message>' with exit 2, reusing panic_message from ludenea-control. Better, move the wrapper into ludenea-app so play_replay returns ReplayError::Panicked { tick, message } and both tools share it.
- First test suggested by the auditor: crates/ludenea-cli/tests/cli.rs: `game_panic_names_the_tick`: an app whose system panics when Time.tick == 3. `run {}/scene.ron --ticks 5` returns 2 and output containing 'panicked at tick 3'.
- Verifier notes (binding scope corrections): There is no catch_unwind anywhere in crates/ludenea-cli/src or in ludenea-app/src/replay.rs. The only ones are in ludenea-control: session.rs:398, 1670, host.rs:849, compare.rs, divergence.rs. Run::record (commands.rs:71-81) and play (95-103) call record_replay and play_replay bare, and ReplayError has no Panicked variant (replay.rs:39-55). A game panic therefore falls through to Rust's default ho...

#### Finding errors-12: System panic reports lack the system name, and natively the source location

- Verified status: partially; final severity: low; rule: dx; feature: simulation loop, editor console.
- Locations: crates/ludenea-control/src/session.rs:1663-1677, crates/ludenea-control/src/session.rs:2189-2195, crates/ludenea-control/src/worker.rs:44-56, crates/ludenea-control/src/session.rs:45
- Evidence: SessionError::Panicked { tick, message } has only the payload text (`panic_message`, session.rs:2189). The console shows 'the game panicked while running tick N: called Option::unwrap() on a None value', with no system and, natively, no file:line. The wasm worker hook does add ` at file:line` (worker.rs:47-50), so the browser and desktop disagree. On desktop the location only appears as separate stderr 'game output' lines. Neither path names the system that was running, although the schedule knows it.
- Reachability: Every game developer whose system panics while playing or resimulating in the editor.
- Recommendation: Have the schedule record the running system's name (a field on the Simulation or a thread-local set around each system call) and include it in SessionError::Panicked { tick, system, location, message }. Install a panic hook in Host::new (natively too, as worker.rs does) that stores `info.location()` for the session to read. Show 'system boxes::spawn panicked at tick 42 (src/lib.rs:88:14): ...' in PauseReason::Panicked and the console, with the Location detail making it clickable.
- First test suggested by the auditor: crates/ludenea-control/tests/running.rs: `a_panicking_system_is_named_with_its_source_location`. An app whose system `panicky` panics at tick 3: after play, assert that PauseReason::Panicked's message contains 'panicky' and 'tests/running.rs:'.
- Verifier notes (binding scope corrections): The facts are right: SessionError::Panicked { tick, message } (session.rs:45) is built from the payload alone (session.rs:1663-1677, panic_message at 2189-2195), and Host::fail (host.rs:1232-1238) sends that text to the console. The wasm hook appends ` at file:line` (worker.rs:44-56). No production code installs a native hook, so no report names the running system. The finding's weight is too high...

#### Finding dx-live-11: Game panics do not name the system, and prefab ids appear as raw hexadecimal

- Verified status: confirmed; final severity: medium; rule: errors; feature: runtime diagnostics.
- Locations: crates/ludenea-ecs/src/prefab.rs:88-92, crates/ludenea-ecs/src/prefab.rs:113, crates/ludenea-ecs/src/prefab.rs:141, crates/ludenea-ecs/src/assets.rs:222-225, crates/ludenea-ecs/src/commands.rs:165-171, crates/ludenea-ecs/src/schedule.rs:218-237, crates/ludenea-control/src/session.rs:44
- Evidence: With `overrides.set::<Pose>(5, ..)` (variants/p1_bad_local.py), `run` panics at engine source prefab.rs:141 with "prefab 6c90cde358504084bfb84f6d1d83c6d9 has no entity with local identifier 5" (exit 101). The host reports "the game panicked while running tick 0: prefab 6c90... has no entity with local identifier 5". Neither names `game::spawn_balls` (the system whose commands failed) nor `assets/ball.prefab.ron`, so the developer must grep for the hexadecimal id. `PrefabOverrides::set` and `Commands::insert` of an unregistered component panic the same way, and none has a fallible variant.
- Reachability: A game developer passing a wrong local id, a missing component, or an unloaded prefab handle.
- Recommendation: Have `Schedule::run`/`run_observed` record the running system's name (a field on World or a thread-local set before each system and its commands::apply), and append "in system `{name}`" to `SessionError::Panicked` and to the CLI panic output. Let the host map asset ids to paths in messages, as editor-shell/assets.ts:46 already does for asset errors. Validate `PrefabOverrides` local ids when queued against the loaded prefab, returning an error the system can handle.
- First test suggested by the auditor: In a ludenea-control test, a system queuing `spawn_prefab` with a bad local id makes `Step` fail with a message containing the system's type name and the prefab's path.
- Verifier notes (binding scope corrections): The panic sites match the finding: prefab.rs:92 ('prefab {id} is not loaded'), :113, :141 ('has no entity with local identifier'), PrefabOverrides::set at :57, and commands.rs:165-171 (ignore_dead panics on any error other than a dead entity). Schedule::run and run_observed (schedule.rs:218-237) record no system name. SessionError::Panicked (session.rs:44-45) carries only the tick and the message....

