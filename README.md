# Ludenea

Ludenea is a deterministic 2D game engine written in Rust. It combines an
entity component system, a Box2D-style physics engine, a versioned control
protocol, and a command line tool, all built so that a simulation can be
recorded, replayed, and verified bit for bit.

## Determinism guarantee

The same scene, the same input log, and the same engine version produce the
same state hash at every tick, on macOS, Linux, Windows, and WebAssembly, and
for any thread count. The guarantee rests on:

- a golden trace of 10,001 state hashes, which the `determinism` workflow
  verifies on macOS, Linux, and Windows at 1, 2, and 8 threads, and on
  WebAssembly under `wasmtime`;
- a golden replay (`golden/spawner.replay`) of a scene with prefabs, a data
  asset, and edits to both, verified the same way;
- a pinned hash of a value that holds every serde shape, and mutation testing
  that proves the tests notice any field dropping out of the state hash;
- lints that forbid platform-dependent floating point functions, wall-clock
  time, and unordered collections in simulation code;
- a pinned toolchain and a committed `Cargo.lock`.

## Workspace layout

| Path                     | Purpose                                         |
| ------------------------ | ----------------------------------------------- |
| `crates/ludenea-math`    | Deterministic vectors, rotations, and functions |
| `crates/ludenea-ecs`     | World, queries, snapshots, and state hashing    |
| `crates/ludenea-macros`  | Derive macros                                   |
| `crates/ludenea-app`     | Simulation loop, scenes, and input logs         |
| `crates/ludenea-physics` | Rigid body physics                              |
| `crates/ludenea-control` | Versioned control protocol and host             |
| `crates/ludenea-cli`     | `run`, `verify`, and `diff` commands            |
| `crates/ludenea`         | Facade crate re-exporting the engine            |
| `crates/ludenea-project` | Projects, builds, and plugin bundling           |
| `crates/ludenea-serve`   | The `ludenea` tool: `new` and `serve`           |
| `editor`                 | The editor: web and desktop applications        |
| `examples/boxes`         | Reference scenario, golden trace, and benchmark |
| `golden`                 | Golden hash traces                              |

## Commands

Run the tests:

```sh
cargo test --locked --workspace
```

On a divergence, the golden tests write the snapshots of the first divergent tick and the
one before it into the folder `LUDENEA_DUMP_DIR` names, else into the temporary directory.
CI sets it to `dump/` and uploads the dumps of a failing run as artifacts, one per
operating system, target, and thread count.

Run the lint checks CI runs: formatting (the engine and the desktop backend), clippy on the
host and on WebAssembly, and the API documentation, with warnings denied:

```sh
cargo xtask check
```

Check that the tests catch every mutant of the canonical state hash serializer, as the
`mutants` workflow does when the serializer or the `ludenea-ecs` tests change (install the
tool with `cargo install --locked cargo-mutants@27.1.0`):

```sh
cargo mutants --file crates/ludenea-ecs/src/hash.rs --cargo-arg=--locked
```

Verify the golden trace natively (use `--threads 1`, `2`, or `8`):

```sh
cargo run --locked --release -p boxes -- verify examples/boxes/assets/boxes.scene.ron \
  --inputs examples/boxes/assets/boxes.inputs --golden golden/boxes.trace --threads 8
```

Verify the golden trace under WebAssembly:

```sh
cargo build --locked --release --target wasm32-wasip1 -p boxes --bin boxes
wasmtime run --dir=. target/wasm32-wasip1/release/boxes.wasm verify \
  examples/boxes/assets/boxes.scene.ron --inputs examples/boxes/assets/boxes.inputs \
  --golden golden/boxes.trace
```

Verify the golden replay natively (use `--threads 1`, `2`, or `8`) and under WebAssembly:

```sh
cargo run --locked --release -p boxes -- verify golden/spawner.replay --threads 8
wasmtime run --dir=. target/wasm32-wasip1/release/boxes.wasm verify golden/spawner.replay
```

Record it again after a deliberate change to the simulation (its hashes change with it):

```sh
cargo run --locked --release -p boxes --bin record-replay -- golden/spawner.replay
```

Run the benchmark:

```sh
cargo run --locked --release -p boxes --bin bench
```

## Versioning

Every file format and protocol has a version, except those [`CHANGELOG.md`](CHANGELOG.md)
lists as not versioned yet. The changelog lists each one's current version and the oldest
version this build reads, and records every change to them, to the public Rust API, and to
the plugin API, in the commit that makes it. The rules:

- A reader reads the version before anything else, so a version it does not support is
  refused as such, not as a malformed file.
- Any change to an encoded type bumps its version; nothing is appended silently. The
  first change after a release bumps it, and later changes before the next release go into
  that same version, so a release moves each format by one version at most.
- User files (scenes, prefabs, metas, data assets, the project manifest `ludenea.toml`,
  replays, traces, snapshots, input logs, and edit logs) stay readable one release back
  (N-1). A pure conversion from each older version to the next one migrates them, like the
  edit log's `version_1` module.
- Editor state (the stored session, its undo history, and its branch tree) may drop what
  is older than N-1. A stored file the editor refuses is set aside, never overwritten.
- The control and project protocols require the same version on both sides, and the same
  fingerprint: a hash of the TypeScript declarations of every type their messages reach,
  so builds from different revisions of one version refuse each other. Their changes
  accumulate under the next version, which is bumped once per release; its changelog entry
  names its fingerprint and lists the commands, messages, and fields that changed.

## Editor

The editor opens a game project, runs the game, and records everything that happens
while it plays: scrub the timeline back to any tick, change a component in the inspector
at that tick, play again from there, and undo or redo the change. The session (scene,
input log, and edit log) replays in the command line tool to the same hashes, tick for
tick. The game always runs apart from the interface (a Web Worker in the browser, a
separate process on the desktop), so the interface stays responsive while it simulates.
Panels: scene tree, inspector, viewport with physics debug drawing, timeline, asset
browser, entity explorer, physics, and console, all reachable from the command palette.
A project opens with the document last open in it, or else with the `default_scene` of its
`ludenea.toml`; without one (or with its file missing) nothing opens and the console says
why. Double-click a scene, prefab, or replay in the asset browser to open another.

Create a project from the template (the game depends on the engine at `--engine-path`):

```sh
cargo run --locked -p ludenea-serve -- new ~/games/my_game --engine-path .
```

Serve it with the editor and open `http://127.0.0.1:7878` in a browser; the editor is
usable at once, and the game appears when its first build finishes:

```sh
pnpm -C editor build
cargo run --locked -p ludenea-serve -- serve ~/games/my_game --editor-dir editor/apps/web/dist
```

`ludenea serve` listens on `127.0.0.1` by default. Remote mode (`--host 0.0.0.0` with
`--allow-host <name>`) is for trusted networks only: there is no authentication, and anyone
who can reach the port can change and build the project, which runs its code. The tool
prints a warning when it listens beyond this machine.

Only pages served from the address the browser used may open the project connection. A
development server on another port that proxies it (Vite on `http://localhost:5173`, for
example) needs `--allow-origin http://localhost:5173` (repeatable).

The project connection takes messages up to 256 MiB, in one frame or several. The largest
message is the session the editor stores after each edit: for a scene of 100,000 entities
(about 11 MB of text), about 68 MB after one edit at tick 0 and 39 MB more for each further
one. A larger message is refused with its size and the limit, which the editor shows in its
console and the tool logs, then the connection closes; reload the editor to connect again.

Built with `--features embed-editor`, the `ludenea` binary carries the editor and needs no
`--editor-dir`. The desktop application (Tauri) runs the same editor with the game as a
native process:

```sh
LUDENEA_ENGINE_PATH=$PWD pnpm -C editor/apps/desktop tauri dev -- -- ~/games/my_game
```

### Prefabs

A prefab (`.prefab.ron`) is a group of entities a scene places as instances. The scene tree
groups each instance under its prefab file; "Place prefab" adds one, and "Create prefab
from selection" turns selected entities, as the scene holds them whatever the tick shown,
into a new prefab. A field changed on an instance in the scene (at tick 0) is an override
of that one field, marked in the inspector with "Revert to prefab" and "Apply to prefab";
the instance keeps following the prefab elsewhere. "Apply to prefab" moves the override's
value into the prefab, which the scene tree marks unsaved until its "Save prefab" writes
it; one undo takes both back. Opening the prefab ("Open the selected instance's prefab")
edits it as a document; once saved, every scene using it replays with the change and keeps
its overrides.

### Data assets

A data asset is a `.ron` file whose meta names its type (`Data { type }`), such as the
template's `assets/settings.ron`. Systems read it through a `Handle<T>` field and an
`Assets<T>` parameter, so it is part of the simulation's state and of its hash. Select it in
the asset browser to edit it in the inspector: the session replays with the new value from
the tick of the change, and "Save the selected data asset" writes it back to its file.

### Replays

A replay (`.replay`) holds everything a run needs to be checked again: the resolved scene,
its assets, the input and edit logs, and the hash of every tick. "Save as replay" in the
timeline writes the current branch to `assets/replays/`. Opening a replay in the editor
plays it and reports the first tick that no longer matches; a replay whose values no longer
decode (its game's types changed) is refused with the value at fault. The command line tool
verifies one:

```sh
cargo run --locked --release -p boxes -- verify golden/spawner.replay
```

### Branches

Editing at a past tick no longer discards the recorded future: it forks a branch from that
tick, and the timeline shows one lane per branch under its parent. Click a lane (or "Switch
branch") to go to that branch's future; "Rename branch" and "Delete branch" manage them.
Undoing the edit that forked a branch removes it, and redoing it brings it back. Each
document keeps its session, branches included, when another document opens.

### Divergence tools

"Compare with" picks a second branch: the viewport draws it as a ghost over the current one
("Toggle the ghost of the compare branch" hides it), and the "Divergence report" panel lists,
entity by entity, the first tick each one diverges and its likely causes (the edit at the
fork, or a recorded interaction such as a `ludenea::Collision` with an entity that diverged
before it). A trace (`.trace`) or snapshot (`.snap`) opened from the asset browser shows in
the "Divergence" panel: two traces compare tick by tick to their first divergent tick, and
two snapshots compare entity by entity, component by component, and field by field.

### Plugins

Built-in panels and project plugins use the same API. A project plugin lives in the
project's `editor/index.ts` (or `index.tsx`); the editor bundles it without Node.js and
reloads it on save, without restarting the game. Component values are typed from the
game's schemas (`.ludenea/types/`, written by the editor):

```ts
export default definePlugin({
  name: "my_game",
  activate(editor) {
    editor.panels.register({ id: "my_game.stats", title: "Stats", component: StatsPanel });
    editor.inspectors.register("ludenea::Body", BodyInspector);
    editor.commands.register({ id: "my_game.reset", title: "Reset", run: reset });
  },
});
```

`editor.console` writes to the Console panel: `log` shows a message, `report` a failure.
A command that fails from the palette or a shortcut is reported there under its title.

A panel, custom inspector, field decoration, or explorer column that throws while rendering
costs only its own place: a panel shows the failure with a "Reload panel" button, a custom
inspector gives way to the generic fields, and a decoration or cell shows a mark. The failure
is reported in the Console once, naming the plugin (`editor.pluginOf`), and a new
registration (the plugin rebuilt) renders it again. A plugin that renders other plugins'
registrations wraps each in `PluginBoundary` for the same isolation.

### Development

The front end lives in `editor/` (pnpm workspace; Node.js 24 and the pnpm version in
`editor/package.json`, for building the editor only):

```sh
pnpm -C editor install --frozen-lockfile
pnpm -C editor build:wasm              # viewport renderer, before typecheck and build
pnpm -C editor typecheck
pnpm -C editor test
pnpm -C editor build
```

End-to-end tests (Playwright, Chromium) drive the built web editor against
`ludenea serve`: the golden trace and replay in a browser, a session replayed by the command
line tool, plugin parity, prefabs, branches, the divergence tools, saved replays, and
fluidity with 100,000 entities, a branch, and the ghost. `PERF_STRICT=1` turns the frame
time report into assertions, on the reference machine (an Apple M-series):

```sh
cargo xtask wasm-game boxes editor/e2e/golden/pkg
cargo run --locked -p boxes --bin editor-bench
cargo build --locked -p ludenea-serve
pnpm -C editor e2e
```

The divergence viewer's fixtures (`editor/e2e/fixtures/divergence/`) come from the `boxes`
app. The tests never regenerate them; after a change to the trace, snapshot, or replay
formats, write them again and commit them:

```sh
cargo run --locked -p boxes --bin divergence-fixtures editor/e2e/fixtures/divergence
```

The timing and memory budgets (switching branches, the divergence report, checkpoints of 20
branches) are `#[ignore]`d tests, run in release:

```sh
cargo test --locked --release -p ludenea-control --test branches --test divergence -- --ignored --nocapture
```

After changing protocol types, regenerate the TypeScript bindings and the declarations
project plugins check against (CI fails when they are stale). The `typescript` tests also
pin each protocol's fingerprint: when the shape changes they fail with the new value, which
goes into `PROTOCOL_FINGERPRINT` (or `PROJECT_PROTOCOL_FINGERPRINT`) and into the
changelog entry of that protocol version.

```sh
cargo test --locked -p ludenea-control --test typescript
cargo test --locked -p ludenea-project --test typescript
node editor/scripts/bundle-dts.mjs
```
