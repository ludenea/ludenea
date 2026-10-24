# Ludenea

Ludenea is a general-purpose 2D game engine written in Rust, built around one
guarantee: **the simulation is bit-for-bit deterministic across platforms and
across thread counts.** The same scene, the same seed, and the same inputs
produce the same state hash on macOS, Linux, Windows, and WebAssembly, whether
the physics runs on 1 thread or 16.

Read `README.md` for the workspace layout, the commands, and the editor.

## Quality bar

Ludenea is professional-grade software: stable, fully tested, and deployable
in real-world games. No hacks, no shortcuts.

- Every behaviour change comes with tests; a fix starts with a test that
  reproduces the bug.
- Fix root causes, not symptoms. No workarounds, disabled tests, silenced
  lints, or `unwrap` on paths a user can reach.
- Errors are precise and actionable; a game or a tool never panics on bad
  input.
- Public APIs and file formats are versioned; a breaking change is deliberate
  and documented.

## Why determinism

- **Testability.** A failing run replays exactly, a test asserts a hash, and a
  regression is caught at the first divergent tick.
- **Competitive multiplayer.** Rollback netcode, lockstep, replays, and
  server-side re-verification (anti-cheat) all require it.

Studios keep rebuilding deterministic simulation and multiplayer on the side of
their engine, after the fact. Ludenea makes both native.

## What makes it Ludenea

- **No privileged API.** Engine features (physics, rendering, networking) are
  plugins written against the same public API a game uses. The editor's
  built-in panels are plugins on the same editor API as a project's plugins.
- **The editor is a client of the control surface.** Everything the editor
  does goes through the versioned control protocol, so the command line tool
  and other tools can do it too, and a studio can assemble its own editor.
- **Determinism as editor features.** Edits are recorded as inputs, so a
  session replays to the same hashes. Editing the past forks a branch instead
  of destroying the future; a replay is an asset you open, share, and verify
  in CI; a divergence report points at the first divergent tick, entity,
  component, field, and likely cause.
- **Developer experience first.** A consistent, typed API that is hard to
  misuse, precise errors, and precise desync diagnostics.

## Roadmap

1. Deterministic core proven cross-platform: math, ECS, physics, snapshots,
   hash tree, control protocol, command line tool. Done.
2. Editor (Tauri and React, desktop and browser): projects, inspector,
   timeline, plugins, then prefabs, data assets, replays, branches, ghost
   future, and divergence tools. Done.
3. Rendering: wgpu sprites, atlases, camera, animations, interpolation
   between ticks.
4. Networking: rollback netcode (GGPO style) first, lockstep afterwards.
5. Developer tooling: hot reload of game code, packaged lints.

Not planned for now: 3D, scripting languages (game logic is Rust; sandboxed
WebAssembly later), audio, mobile and console platforms, 32-bit targets.

## Anti-goals

A change that does any of these is a failure, even if it works:

- Determinism that holds "most of the time", on one platform only, or only
  single-threaded.
- A feature that bypasses the public API, or a privileged internal API.
- Two ways to do the same thing.
- An editor-only capability that the control surface cannot reach.
- A desync report that only says "hash differs".

## Rules for agents

- Determinism is the product. Simulation code must not use platform-dependent
  floating point functions, wall-clock time, or unordered collections; the
  lints in `clippy.toml` enforce this. Never weaken them to make code compile.
- The golden traces and replays in `golden/` are the proof. A change that
  alters their hashes must be deliberate, recorded again with the commands in
  `README.md`, and explained in the commit.
- Keep the toolchain pinned and `Cargo.lock` committed; build with `--locked`.
- Every agent and the IDE share one `target/`. Never set `CARGO_INCREMENTAL`,
  `RUSTFLAGS`, or another profile setting for a single command: a difference
  rechecks every workspace crate.
- `cargo xtask check` already runs the workspace clippy; do not run it again
  separately.
- Cargo never deletes stale artifacts, and builds slow down severalfold once
  `target/debug/deps` holds a few hundred thousand files. Run `cargo clean`
  between milestones, never while another session builds.
- Code, comments, and documentation are in English.
