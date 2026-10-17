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

Run the benchmark:

```sh
cargo run --locked --release -p boxes --bin bench
```

## Editor

The editor opens a game project, runs the game, and records everything that happens
while it plays: scrub the timeline back to any tick, change a component in the inspector
at that tick, play again from there, and undo or redo the change. The session (scene,
input log, and edit log) replays in the command line tool to the same hashes, tick for
tick. The game always runs apart from the interface (a Web Worker in the browser, a
separate process on the desktop), so the interface stays responsive while it simulates.
Panels: scene tree, inspector, viewport with physics debug drawing, timeline, asset
browser, entity explorer, physics, and console, all reachable from the command palette.

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

Built with `--features embed-editor`, the `ludenea` binary carries the editor and needs no
`--editor-dir`. The desktop application (Tauri) runs the same editor with the game as a
native process:

```sh
LUDENEA_ENGINE_PATH=$PWD pnpm -C editor/apps/desktop tauri dev -- -- ~/games/my_game
```

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
`ludenea serve`: the golden trace in a browser, a session replayed by the command line tool,
plugin parity, and fluidity with 100,000 entities. `PERF_STRICT=1` turns the frame time
report into assertions, on the reference machine (an Apple M-series):

```sh
cargo xtask wasm-game boxes editor/e2e/golden/pkg
cargo run --locked -p boxes --bin editor-bench
cargo build --locked -p ludenea-serve
pnpm -C editor e2e
```

After changing protocol types, regenerate the TypeScript bindings and the declarations
project plugins check against (CI fails when they are stale):

```sh
cargo test --locked -p ludenea-control --test typescript
cargo test --locked -p ludenea-project --test typescript
node editor/scripts/bundle-dts.mjs
```
