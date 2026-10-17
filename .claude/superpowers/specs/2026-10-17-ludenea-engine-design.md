# Ludenea: deterministic 2D game engine

Date: 2026-10-17
Status: draft, awaiting approval

## 1. Intent

Ludenea is a general-purpose 2D game engine written in Rust, built around
one guarantee: **the simulation is bit-for-bit deterministic across
platforms and across thread counts.** The same scene, the same seed, and
the same inputs produce the same state hash on macOS, Linux, Windows, and in
the browser (WebAssembly), whether the physics runs on 1 thread or 16.

Why it matters:

- **Testability.** Gameplay and physics become reproducible: a failing run
  can be replayed exactly, a test asserts a hash, a regression is caught at
  the first divergent tick.
- **Competitive multiplayer.** Rollback netcode, lockstep, replays, and
  server-side re-verification (anti-cheat) all require determinism.

Positioning: studios repeatedly rebuild deterministic simulation and
multiplayer on the side of their engine, after the fact. Ludenea makes both
native. Engine features (physics, rendering, networking) are plugins written
against the same public API a game developer uses, so nothing is "glued on
next to the engine".

Developer experience (DX) is a first-class goal. The main pain the author
has felt with other engines is an unclean API for building features; the
API must be consistent, typed, and hard to misuse.

## 2. Goals, non-goals, anti-goals

### Goals

- Deterministic, headless-by-default simulation core: ECS plus 2D rigid
  body physics.
- Cross-platform determinism: macOS (arm64), Linux (x86_64), Windows
  (x86_64), WebAssembly (browser and WASI).
- Multithreaded physics whose result does not depend on the thread count
  (approach inspired by Box2D v3 research by Erin Catto).
- Snapshot and restore of the full simulation state, the foundation for
  rollback, replays, and the editor timeline.
- A control surface: a versioned message protocol between clients (editor,
  CLI, future tools) and the engine, over several transports.
- An editor built with Tauri and React, available as a desktop application
  and in the browser.
- A command-line interface (CLI).
- Final game rendering (milestone 3), outside the simulation.
- Native multiplayer (milestone 4): rollback first, lockstep later.
- Excellent DX: clean API, precise errors, precise desync diagnostics.

### Non-goals (for now)

- 3D.
- Scripting languages. Game logic is written in Rust. Scripting comes later
  (likely sandboxed WebAssembly modules, deterministic by construction).
- Audio (later; it lives outside the simulation like rendering, so it
  carries no determinism risk).
- Mobile and console platforms.
- 32-bit targets.

### Anti-goals (failure even if it technically works)

- Determinism that holds "most of the time", or only on one platform, or
  only single-threaded.
- Features that bypass the public API, or a privileged internal API.
- Two ways to do the same thing.
- Editor-only capabilities: everything the editor does goes through the
  control surface, so the CLI and other tools can do it too.
- A desync report that only says "hash differs".

## 3. Constraints

- Language: Rust (stable toolchain, currently 1.97). Editor front end:
  React and TypeScript inside Tauri.
- Game logic: Rust crates that depend on Ludenea and register components
  and systems as plugins.
- Sandbox project: standards are relaxed (no mandatory TDD or hexagonal
  architecture), but the determinism guarantees are enforced by automated
  tests because they are the product.
- File content, code, and comments in English.

## 4. Milestones

Each milestone after the first gets its own design document and plan when
it starts. This document is detailed for milestone 1 and sets the
architecture the later ones must fit.

1. **Deterministic core, proven cross-platform.** Deterministic math, ECS,
   physics, app and tick loop, snapshot and restore, hash tree, control
   protocol core with in-process transport, CLI. Proof: identical golden
   hashes on the four platforms and across thread counts.
2. **Editor and control surface**, split into two milestones with their own
   design documents:
   - **2a, the usable editor** (`2026-10-17-ludenea-editor-2a-design.md`):
     projects, Tauri plus React editor on desktop and in the browser,
     React editor plugins, Tauri IPC, Web Worker, and WebSocket transports,
     scene editing, ECS explorer, inspector, play, pause, step, linear
     timeline backed by logarithmic checkpoints with edits recorded as
     inputs, undo and redo, asset foundation, minimal wgpu debug-draw
     viewport.
   - **2b**: prefabs, data assets, replays, trace and diff viewer, timeline
     branches, ghost future with causal divergence.
3. **Rendering.** wgpu renderer: sprites, atlases, 2D camera, frame
   animations, interpolation between ticks, native window and web canvas.
4. **Networking.** Rollback netcode (GGPO style): input prediction, rollback
   ring buffer, input delay, desync detection between peers. Lockstep
   afterwards.
5. **DX tooling.** `ludenea new` project template, hot reload of game code
   (spike `subsecond` from Dioxus first), packaged lints.

## 5. Architecture

### 5.1 Repository layout

Cargo workspace at the repository root:

```text
crates/
  ludenea-math/          deterministic scalar and vector math
  ludenea-ecs/           world, entities, components, resources, events,
                         queries, schedule, snapshot, hash tree
  ludenea-macros/        derive(Component), derive(Query), derive(Input)
  ludenea-physics/       2D rigid body physics plugin
  ludenea-app/           App, Plugin, fixed tick loop, inputs, RNG,
                         checkpoints
  ludenea-control/       protocol types, engine host, transports
  ludenea-cli/           CLI as a library (games provide their own main)
  ludenea/               facade crate: re-exports and prelude
examples/
  boxes/                 milestone 1 reference scenario
golden/                  committed golden hash traces
editor/                  Tauri + React app (milestone 2)
```

### 5.2 Layers

```text
 clients:   editor (React)        CLI          future tools / agents
              |                    |                |
 transport: Tauri IPC | Web Worker | in-process | WebSocket
              \___________________ | ____________/
 control:            protocol (versioned commands, events)
                                   |
 host:               engine host: owns the App, executes commands
                                   |
 simulation: App = ECS world + schedule + plugins (physics, game)
 presentation (outside simulation, read-only): renderer, audio
```

The simulation never reads anything that is not part of its state or of the
per-tick input. Presentation layers read state and never write it.

### 5.3 How a game plugs in

A game is a crate exposing a function that builds its `App` (engine plugins
plus game plugins). Hosts are libraries, so each game has a tiny `main` per
target (CLI, Tauri backend, Web Worker), all calling the same app builder.
For milestone 1, `examples/boxes` plays this role.

## 6. Determinism contract

These rules apply to every crate that runs inside the simulation
(`ludenea-math`, `ludenea-ecs`, `ludenea-physics`, `ludenea-app`, and game
simulation code).

### 6.1 Floating point

- Use `f32` with IEEE 754 semantics. Rust never contracts `a * b + c` into a
  fused multiply-add unless asked, and `+ - * / sqrt` are correctly rounded
  on all supported targets (including WebAssembly), so they are
  deterministic.
- Forbidden in the simulation: `mul_add`, and every transcendental function
  from `std` (`sin`, `cos`, `tan`, `atan2`, `exp`, `ln`, `powf`, `hypot`,
  and friends), because their results vary by platform library.
- `ludenea-math` provides replacements implemented only with `+ - * / sqrt`
  (polynomial approximations, Box2D v3 style): `sin`, `cos`, `sin_cos`,
  `atan2`, plus vectors, rotations (stored as cosine and sine pair),
  transforms, and AABB.
- No reliance on denormal flushing modes; none of the targets enable it by
  default and Ludenea never changes it.
- NaN must never enter the state: physics asserts finiteness in debug
  builds; the hasher canonicalizes NaN bit patterns so a NaN, if one slips
  through, at least produces the same hash everywhere.
- Target CPU features stay at the defaults; no `-C target-cpu=native`.

### 6.2 Data structures and ordering

- No iteration over randomly seeded hash maps in the simulation. Use
  `BTreeMap`, or `IndexMap` with a fixed hasher.
- Entity identifiers: generational indices allocated by a deterministic free
  list.
- Query iteration order is a deterministic function of the world state, and
  a snapshot preserves it: after a restore, iteration order is identical.
- Equal hashes must imply equal future behaviour. If iteration order depends
  on storage layout (for example dense array order), that order is part of
  the canonical snapshot and covered by the world hash (an ordering leaf in
  the hash tree). (Builder's call.)
- Systems run in a stable order (registration order, refined by explicit
  `before` and `after` constraints). Parallel execution of systems is only
  allowed between systems with no conflicting access, so it cannot change
  results.
- Commands (spawn, despawn, insert, remove) are applied at defined sync
  points, in the order they were issued, with the issuing system's position
  in the schedule as the primary key.

### 6.3 Inputs, time, randomness

- Time is a tick counter with a fixed timestep set at app build time (default
  60 Hz). No wall clock in the simulation.
- Randomness comes from an `Rng` resource seeded at world creation, part of
  the snapshot (a small, specified algorithm such as PCG32, so it can be
  reimplemented and audited).
- Inputs: a game-defined input type (`#[derive(Input)]`), provided per player
  per tick. The input log plus the initial snapshot fully define a run.

### 6.4 Multithreading

- Work is split into partitions whose boundaries depend only on the data
  (fixed chunk sizes, graph colors), never on the number of threads.
- Results are written to preassigned slots and combined in index order.
  No floating-point reduction whose order depends on scheduling.
- Thread pool: rayon on native. WebAssembly runs single-threaded by default
  and must produce the same hash.

### 6.5 Enforcement

- `clippy.toml` with `disallowed-methods` (std transcendentals, `mul_add`,
  `Instant::now`, `SystemTime::now`) and `disallowed-types`
  (`std::collections::HashMap`, `HashSet`) applied to simulation crates,
  with CI failing on violations.
- The cross-platform golden test (section 10) is the final arbiter.

## 7. ECS

### 7.1 Inspired by Bevy, minus its weaknesses

The shape follows Bevy: systems are plain functions; parameters are
`Query`, `Res`, `ResMut`, `Events`, `Commands`; filters `With` and
`Without`. Corrections:

| Bevy weakness | Ludenea behaviour |
|---|---|
| Unordered systems run in a nondeterministic order | Stable order guaranteed; parallelism only between non-conflicting systems |
| Access conflicts (`&mut T` in two queries) panic at startup | Detected when the app is built; the error names both systems and the conflicting component and suggests a fix |
| Long tuple signatures are unreadable | Named query structs (`#[derive(Query)]`) are the primary path; tuples remain for small cases |
| Iteration order depends on archetype history | Iteration order specified and restored by snapshots |
| Change detection and deferred commands are not rollback aware | Change ticks, pending events, and commands are part of the snapshot |

### 7.2 Components

`#[derive(Component)]` generates:

- a stable component name (`module_path!()` plus the type name), used for
  hashing, snapshots, scene files, and the protocol. `TypeId` is never
  persisted because it differs across builds;
- canonical serialization (snapshot bytes, RON for scenes);
- hashing over the canonical little-endian encoding;
- a field schema (names and types) consumed by the editor inspector and by
  field-level desync diffs.

Storage layout is left to the builder, provided the ordering contract in
section 6.2 holds and snapshots stay cheap (milestone 4 snapshots every
tick).

### 7.3 Target API

```rust
#[derive(Component, Serialize, Deserialize)]
struct Health(u32);

#[derive(Query)]
#[query(filter = Without<Dead>)]
struct Fighter<'w> {
    health: &'w mut Health,
    team: &'w Team,
}

fn damage_on_hit(hits: Events<Collision>, mut fighters: Query<Fighter>) {
    for hit in hits.iter() {
        for entity in [hit.a, hit.b] {
            if let Ok(fighter) = fighters.get_mut(entity) {
                fighter.health.0 = fighter.health.0.saturating_sub(10);
            }
        }
    }
}

pub struct Combat;

impl Plugin for Combat {
    fn build(&self, app: &mut App) {
        app.component::<Health>()
            .system(Sim, damage_on_hit.after(physics::Step));
    }
}
```

Registering a component is enough for it to be serializable, hashable,
rollback-safe, and editable in the inspector.

API principles: one way to do each thing; consistent naming across engine
plugins; the engine's own plugins use only the public API; errors explain
what to do.

## 8. Physics (milestone 1 scope)

Plugin `ludenea-physics`, architecture inspired by Box2D v3. Written from
scratch, not ported: Box2D v3 is a reference for ideas (soft step solver,
graph coloring, determinism techniques), not code to translate. The ECS is
also written from scratch.

In scope:

- Bodies: static, kinematic, dynamic. Components for body, collider,
  velocity, mass properties, material (friction, restitution).
- Shapes: circle, convex polygon (boxes included), capsule.
- Broad phase: deterministic (dynamic AABB tree or sort and sweep; builder's
  choice) with deterministic pair ordering.
- Narrow phase: contact manifolds with persistent contact identifiers
  (warm starting).
- Solver: soft step (substepping with soft constraints and relax
  iterations), restitution, friction.
- Parallelism: constraint graph coloring; colors solved in parallel, within
  the rules of section 6.4.
- Collision events: contact begin and end, exposed as `Events<Collision>`.
- Scene queries: raycast and AABB overlap.

Deferred to later work: joints (revolute, prismatic, distance, weld), sleeping
and islands, continuous collision detection, sensors.

## 9. Snapshot, hash, checkpoints

### 9.1 Snapshot and restore

The full simulation state (entities, components, resources including `Rng`
and tick, pending events and commands, change ticks, physics caches such as
contact warm-start data) serializes to a canonical byte buffer. Restoring it
and running N ticks with the same inputs yields the same state as never
having snapshotted.

### 9.2 Hash tree

- Component hash: hash of its canonical encoding.
- Entity hash: combination of its component hashes ordered by stable
  component name.
- World hash: combination of entity hashes in entity identifier order, plus
  resource hashes ordered by name.
- Hash function: a fixed, platform-independent 64-bit hash (for example
  xxHash3) over little-endian bytes.

The tree powers the desync diagnosis: comparing two traces finds the first
divergent tick, then the entity, the component, and the field (fields are
compared through the canonical decoded values).

### 9.3 Checkpoints (milestone 2 and 4)

- Editor timeline and replays: logarithmic checkpoints (dense near the
  current tick, spaced at distances 1, 2, 4, 8, and so on going back) plus
  the full input log. Seeking restores the nearest earlier checkpoint and
  replays inputs. Memory is O(log n).
- Rollback netcode: a ring buffer of the last N ticks (8 to 16).

## 10. Milestone 1 proof: cross-platform golden test

### 10.1 Run definition: scene plus input log

- A **scene** is a RON file listing entities with their components (by
  stable name) and resources, including the `Rng` seed. RON loading and
  saving is part of milestone 1, because the CLI and the protocol load
  scenes. (Builder's call.)
- An **input log** is a file holding, for each tick, the inputs of each
  player, in the canonical encoding of the game's input type. Scene plus
  input log fully define a run, which milestone 1 demonstrates. (Builder's
  call.)
- The `boxes` scenario commits its scene and its input log. The input log is
  produced once by a small deterministic generator in the example and then
  committed, so the test never depends on generator code. (Builder's call.)

### 10.2 Trace and diff

- A **trace** stores the scene identifier, a hash of the input log, and the
  world hash for every tick. It stays small (about 10 bytes per tick, hashes
  are postcard varints), so the golden trace is cheap to commit. (Builder's call.)
- On mismatch, `verify` writes a full snapshot at the first divergent tick
  and at the tick before. In CI this becomes a build artifact.
- `diff` compares two snapshots taken at the same tick (for example the CI
  artifact against a local run on another platform) and reports entity,
  component, and field. (Builder's call.)

### 10.3 Golden test

- Reference scenario `examples/boxes`: a few hundred mixed bodies (boxes,
  circles, capsules) dropped into a static container, with a scripted input
  stream that applies impulses, spawns and despawns bodies, and uses `Rng`,
  so the test exercises commands, events, and randomness, not only stacking.
- Run 10,000 ticks. Record the world hash every tick into a trace file and
  commit a golden trace (`golden/boxes.trace`).
- The test passes when every platform matches the golden trace exactly:
  - native macOS arm64, Linux x86_64, Windows x86_64, each with 1, 2, and 8
    threads;
  - WebAssembly through `wasm32-wasip1` under wasmtime (same CLI code);
  - a headless browser run is added in milestone 2.
- A second test snapshots at tick 5,000, restores, and checks the remaining
  trace is identical.
- CI: GitHub Actions matrix with the targets above. Locally, macOS native
  and wasmtime are runnable; Linux and Windows are verified in CI.

## 11. Control surface

### 11.1 Protocol

- Messages are Rust types serialized as JSON (serde), with TypeScript types
  generated from Rust (`ts-rs`) for the editor, so the two cannot drift.
- Every connection starts with a handshake carrying the protocol version;
  incompatible versions are refused with a clear error.
- Requests with identifiers and responses; plus a stream of events pushed by
  the engine.
- Milestone 1 commands: load scene, step N ticks, get hash (world or
  subtree), snapshot, restore, query entities and components, write trace.
- Milestone 2 adds: spawn, despawn, set component field, play, pause, seek,
  save scene, subscribe to state changes.

### 11.2 Transports

| Transport | Used by | Milestone |
|---|---|---|
| In-process | CLI, tests | 1 |
| Tauri IPC | desktop editor (engine native, multithreaded) | 2 |
| Web Worker (`postMessage`) | browser editor (engine in WebAssembly) | 2 |
| WebSocket | remote engine (`ludenea serve`), CLI remote mode | 2 |

The React editor has one control client with interchangeable transports.

## 12. CLI (milestone 1)

Implemented on top of the control protocol through the in-process
transport (clap for argument parsing).

- `run <scene> [--inputs <file>] --ticks <n> [--threads <n>] [--hash] [--trace <file>]`
- `diff <snapshot-a> <snapshot-b>`: divergent entities, components, and
  fields (see section 10.2).
- `verify <scene> --inputs <file> --golden <file>`: run and compare against a golden trace;
  non-zero exit code on mismatch (used by CI).

## 13. Editor (milestone 2, outline)

- One React application. Desktop: Tauri hosts it, engine in the Rust
  backend. Browser: the same app served statically, engine compiled to
  WebAssembly in a Web Worker.
- Panels: entity tree, inspector (driven by component schemas), viewport,
  timeline (play, pause, step, seek back), console.
- Scenes stored as RON files, diffable in git, read and written by both
  editor and CLI.

## 14. Rendering (milestone 3, outline)

- `ludenea-render` on wgpu (Metal, Vulkan, DirectX natively; WebGPU and
  WebGL2 in the browser).
- Reads simulation state, never writes it; interpolates between the last two
  ticks.
- Sprites, atlases, 2D camera, frame animations, physics debug draw.
- The editor viewport embeds the same renderer compiled to WebAssembly.

## 15. Networking (milestone 4, outline)

- Rollback: local input prediction, rollback ring buffer, resimulation,
  configurable input delay, per-tick hash exchange between peers with the
  hash tree for desync reports.
- Lockstep afterwards, on the same snapshot and input foundations.
- Transport choices (UDP natively, WebRTC data channels in the browser) are
  decided in that milestone's design.

## 16. Performance targets

Initial targets, to keep milestone 4 (snapshot every tick, resimulating up to
16 ticks per frame) reachable without rewriting the ECS. Measured on an Apple
M-series machine, release build. (Builder's call; revisit in milestone 4.)

- 1,000 dynamic bodies with active contacts: one simulation tick under 2 ms on
  8 threads, under 6 ms on 1 thread.
- Snapshot and restore of that world: each under 0.5 ms.
- A benchmark of these numbers lives next to the golden test so regressions
  are visible.

## 17. Left to the builder

- ECS storage layout (archetypes or sparse sets), within section 6.2.
- Broad phase algorithm, within section 8.
- Exact polynomial approximations in `ludenea-math`, as long as they only
  use `+ - * / sqrt` and keep accuracy suitable for physics.
- Snapshot byte format, as long as it is canonical and versioned.
- Exact RNG algorithm, as long as it is small and specified.
