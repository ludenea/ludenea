# Ludenea milestone 2a: the usable editor

Date: 2026-10-17
Status: approved
Parent design: `2026-10-17-ludenea-engine-design.md` (milestone 2, split into
2a and 2b)

Marks used below: **(author)** means the author decided it in conversation;
**(builder's call)** means the designer chose it and the author may revisit
it. Unmarked text restates the parent design.

## 1. Intent

Milestone 1 proved a deterministic core driven from a CLI. Milestone 2a gives
it an editor a game developer can live in: create and open a project, browse
its scenes and files, see and edit the ECS, play, pause, step, and move back
and forth in time. (author)

Two things make it Ludenea's editor rather than a generic one:

- **Extensibility without a privileged API.** Engine plugins (physics) and
  game projects extend the editor with React plugins through one typed API,
  and the built-in panels are themselves plugins on that API. (author)
- **The editor is just a client of the control surface.** A studio can extend
  the standard editor or assemble its own from the same packages. (author)

The deterministic timeline features that build on this (branches, ghost
future, causal diff) are milestone 2b; 2a lays their foundation by recording
editor edits as inputs.

### 1.1 The milestone 2 split (author)

- **2a, the usable editor (this document):** projects, editor shell and
  plugins, the three transports, scenes, ECS explorer and inspector, debug
  draw viewport, console, asset foundation, play, pause, step, linear
  timeline with edits recorded as inputs.
- **2b, the editor that changes the game:** prefabs, data assets
  (`#[derive(Asset)]`), replays as assets, trace and snapshot diff viewer,
  timeline branches, ghost future with causal divergence. Outline in
  section 14; it gets its own design document.

Each is usable on its own; 2b depends on 2a.

## 2. Goals, non-goals, anti-goals

### Goals

- One installed editor that opens any Ludenea project, Godot or Unity style.
  (author)
- Desktop (Tauri, engine native and multithreaded) and browser (engine in
  WebAssembly in a Web Worker, project served by `ludenea serve`). (author)
- React plugins from the engine and from game projects: panels, component
  inspectors, commands, shortcuts. (author)
- A real ECS explorer (section 8). (author)
- Light and fluid: performance targets in section 11. (author)
- Undo and redo for every edit (section 9.5). (author)
- No Node.js or package manager required on the user's machine: the editor
  and `ludenea serve` embed a bundler written in Rust (section 12.1).
  (author)
- Packages a studio can reuse to build its own editor. (author)

### Non-goals (2a)

- Real-time multi-user editing. The remote mode makes it possible later.
  (builder's call)
- Everything listed under 2b.
- Sprites, images, atlases, animation, audio (milestone 3 and later).
  (author)
- Hot reload of game Rust code (milestone 5). Rebuilding the game restarts
  its process (section 9.4).
- Sandboxing editor plugins. A project's plugin is as trusted as the
  project's game code. (builder's call)
- Documentation and templates dedicated to custom editors: 2a makes them
  possible by structure only. (builder's call)
- Third-party npm dependencies in project plugins: in 2a a plugin imports
  only React, the editor packages, and its own files (section 12.1).
  (builder's call, a consequence of needing no Node.js)

### Anti-goals (author, from the pains of other editors)

- An extension API that is not clean: two ways to add a panel, untyped
  component access, or a built-in panel that needs something plugins cannot
  get.
- An ECS shown as a flat entity list with no way to see queries, systems,
  events, or commands.
- A heavy, slow editor: a UI that stutters while the game runs, a blocking
  splash screen while cargo compiles, or anything only reachable by mouse.
- Anything the editor does that bypasses the control surface (parent
  design anti-goal).
- An editor session that breaks determinism: the same session replayed by
  the CLI must give the same trace.

## 3. Constraints

- Everything in the parent design's determinism contract still holds. The
  editor and the hosts live outside the simulation; they may use wall clocks
  and hash maps, the simulation crates may not.
- Rust stable (toolchain pinned by the repository). Front end: TypeScript in
  strict mode, React, Vite, pnpm workspace, used to develop and build the
  editor itself, never required by users. (builder's call)
- Docking panels: dockview. State: Zustand. (builder's call)
- Rust to TypeScript types are generated with `ts-rs` and checked in CI, so
  protocol and component types cannot drift (parent design 11.1).
- Sandbox project: relaxed craft standards, but determinism and protocol
  compatibility are enforced by automated tests.

## 4. Architecture

### 4.1 Processes and roles

Two Rust roles, kept separate because they have different lifetimes and
trust: (builder's call)

- **Project service:** owns a project folder. Lists and watches files, reads
  and writes scenes and `.meta` files, runs `cargo build`, starts and stops
  the game host, serves project plugins to the front end.
- **Game host:** the game's own binary, owning one `Simulation` and executing
  protocol commands (the milestone 1 `Host`, extended). It knows the game's
  components and input type; the project service does not.

```text
 Desktop (Tauri)                         Browser
 +--------------------------+            +-----------------------------+
 | webview: editor (React)  |            | tab: editor (React)         |
 |   |  Tauri IPC           |            |   |  WebSocket   | postMessage
 | backend: project service |            |   v              v          |
 |   |  stdio (protocol)    |            | ludenea serve   Web Worker: |
 |   v                      |            | (project        game host   |
 | game host process        |            |  service)       (wasm)      |
 | (native, multithreaded)  |            +-----------------------------+
 +--------------------------+
```

- Desktop: the Tauri backend embeds the project service as a library and
  talks to the game host over stdio, one JSON message per line.
  (builder's call)
- Browser: `ludenea serve` embeds the same project service behind a
  WebSocket and serves the editor's static files. The project service builds
  the game for `wasm32-unknown-unknown`; the editor loads it into a Web
  Worker, which is the game host. (author)
- Remote: `ludenea serve` on another machine; identical to the browser case.

The front end talks to two endpoints with the same client: the project
service (project protocol) and the game host (control protocol). In the
browser both reach it through different transports (WebSocket and
`postMessage`).

### 4.2 Crates and packages (builder's call)

New Rust crates:

```text
crates/
  ludenea-project/   project service: files, metas, cargo, game host process
  ludenea-serve/     `ludenea serve` binary: project service over WebSocket,
                     static editor files
  ludenea-debug-draw/ debug draw primitives and wgpu renderer (wasm and native)
```

Extended crates: `ludenea-ecs` (field schemas), `ludenea-macros`
(schema derive), `ludenea-control` (protocol version 2, stdio and Web Worker
transports, event stream), `ludenea-cli` (`host` subcommand),
`ludenea-physics` (debug draw output).

Front end, under `editor/`:

```text
editor/
  packages/
    control-client/  typed client, handshake, transports (Tauri IPC,
                     WebSocket, postMessage), generated protocol types
    editor-shell/    layout (dockview), plugin registry, command palette,
                     shortcuts, selection and timeline state
    editor-builtins/ built-in plugins: scene tree, ECS explorer, inspector,
                     viewport, timeline, console, asset browser, project
  apps/
    desktop/         Tauri application (src-tauri embeds ludenea-project)
    web/             static build served by ludenea serve
```

The Ludenea editor is `editor-shell` plus `editor-builtins` plus the engine
and project plugins. A custom studio editor depends on the same packages
and picks its own set.

### 4.3 How a game plugs in

Milestone 1: a game binary calls `ludenea::cli::main(app)`. 2a adds a `host`
subcommand to that CLI that runs the game host over stdio, so the editor
needs nothing extra from a native game. For the browser, the project
template contains a small `wasm32-unknown-unknown` entry (one call,
`ludenea::worker::main(app)`) exposing the game host to a Web Worker.
(builder's call)

## 5. Projects

### 5.1 Layout (builder's call)

```text
my-game/
  ludenea.toml        project manifest: name, engine version, default scene
  Cargo.toml          the game crate (lib with `app()`, bin, wasm entry)
  src/
  scenes/             *.scene.ron
  assets/             any other files
  editor/             the project's editor plugins (TypeScript, React)
    package.json
    index.ts
  .ludenea/           local editor state, ignored by git (layout, caches,
                      build output)
```

Every file under `scenes/` and `assets/` has a sibling `.meta` file
(section 7).

### 5.2 Creating a project

"New project" in the editor (and `ludenea new` in the CLI, pulled forward
from milestone 5 in minimal form) generates the layout above from a
template: one scene with a static floor and a few bodies, the game crate
depending on the engine, an empty project plugin. While the engine is not
published, the template depends on the engine through a path configured in
the editor settings. (builder's call)

### 5.3 Opening a project

1. The project service reads `ludenea.toml` and indexes `scenes/` and
   `assets/`. The editor UI is interactive at this point (section 11).
2. In the background it runs `cargo build` for the target in use (native on
   desktop, `wasm32-unknown-unknown` in the browser) and builds the project
   plugins with the embedded bundler (section 12.1). Build output streams to the console with progress.
3. When the build finishes, the game host starts, handshakes, and sends the
   game's schemas (section 6.2). Panels that need the game (inspector,
   viewport, timeline) show a clear "building" state until then; the file
   browser and the scene files are usable before.
4. A build error shows in the console with clickable file and line, and the
   editor keeps the previous game host running if there was one.

## 6. Control surface changes

### 6.1 Protocol version 2

The handshake still refuses mismatched versions. `ServerMessage` gains an
`Event` variant for pushed events. Additions (exact shapes are the builder's
call):

- **Editing:** spawn, despawn, insert component, remove component, set
  component field, set resource field, save scene (returns RON).
- **Running:** play, pause, step, seek to tick, set playback speed.
- **Reading:** schemas, query entities with a filter (section 8), get
  entity, get resources, schedule (systems, order, accesses), events of a
  tick, applied commands of a tick, debug draw of a tick, pick entities at a
  point (using the physics AABB overlap query, falling back to an entity
  position for bodies without colliders).
- **Subscriptions:** subscribe to tick events (tick, hash), to entity and
  component changes for a set of entities or a query, to console output.
  Changes are pushed as deltas, never as full snapshots every tick.

### 6.2 Field schemas

Deferred from milestone 1 to here, their first consumer. `derive(Component)`
(and a resource equivalent) emits a schema: stable name, fields with names
and types, enumerations with variants. The handshake sends every registered
component, resource, and input schema. The inspector renders from schemas;
`ts-rs` generates TypeScript types for the engine's own components, and the
project service generates them for the game's components at build time so
project plugins get typed access. (builder's call: generation from the
schemas the game binary prints, not from parsing Rust source)

### 6.3 Transports

| Transport | Endpoint | Framing |
|---|---|---|
| In-process | tests, CLI | Rust values |
| stdio | desktop backend to game host | one JSON message per line |
| Tauri IPC | desktop front end to backend | Tauri commands and events |
| WebSocket | browser to `ludenea serve` | one JSON message per frame |
| `postMessage` | browser to Web Worker game host | structured clone of JSON |

On desktop the backend relays control protocol messages between the
webview and the game host unchanged. (builder's call)

## 7. Assets foundation (2a scope)

- Each file under `scenes/` and `assets/` has a `.meta` file holding a
  stable identifier (random 128-bit, generated once and committed) and the
  asset type. The project service creates missing metas and keeps them next
  to their file when the editor renames or moves it. (author, the shape is
  the builder's call)
- Scenes reference other assets by identifier, not by path. (author)
- The asset browser lists, filters, renames, moves, and deletes files, and
  opens scenes. Files changed on disk outside the editor are picked up
  (file watching) and open scenes reload.
- In 2a the only asset read by the simulation is the scene, which is
  already part of the run definition. Content hashing of simulation assets
  comes with data assets in 2b. (builder's call)

## 8. ECS explorer and inspector (author, contents are the builder's call)

- **Entities:** filtered by a query written like the code
  (`With<Body>, Without<Dead>`), parsed by the game host against component
  stable names; virtualized list sized for 100,000 entities; each row shows
  the entity hash.
- **Inspector:** components and resources rendered from schemas, editable
  field by field; add and remove components. A plugin can register a custom
  inspector for one component.
- **Schedule:** systems in run order, their read and write accesses, and
  their time per tick (measured by the host outside the simulation).
- **Events:** events emitted during the selected tick.
- **Commands:** commands applied at each sync point of the selected tick.

Events and commands are recorded by the host only while a client is
subscribed, so the simulation pays nothing otherwise. Recording reads them,
never changes them.

## 9. Running and the timeline

### 9.1 Edit and play (builder's call)

- At tick 0 the editor edits the scene: changes apply to the scene and are
  saved to its file.
- Past tick 0, edits are part of the run (9.2), not of the scene file. An
  explicit "apply to scene" action copies a component value back to the
  scene.
- Editing the scene at tick 0 (or the scene file changing on disk) while a
  session has recorded ticks keeps the recorded inputs and edits and replays
  them on the new scene up to the current tick, like a rebuild (9.4). The
  timeline shows the first tick whose hash differs. (builder's call)

### 9.2 Edits are inputs (author)

An edit made at tick N is recorded as a timestamped command in an **edit
log** and applied at the start of tick N + 1, before any system runs. The
run definition becomes scene plus input log plus edit log, so an editor
session is replayable and hash-verifiable exactly like a game. The CLI
`run` and `verify` accept an edit log. The edit log format is canonical and
versioned, like the input log.

While paused at tick N, an edit shows immediately as a preview: the
inspector and viewport display tick N with the pending edits applied, and
the hash is marked as pending. The recorded state of tick N is unchanged;
the edits take effect for real when tick N + 1 runs. Dragging a transform in
the viewport updates the preview continuously and records one edit on
release. (builder's call)

### 9.3 Playing, stepping, seeking

- Play runs the fixed tick rate in real time inside the game host (the wall
  clock paces the host loop, never the simulation). Pause, step one tick,
  step N ticks, playback speed.
- Player inputs while playing: the editor captures keyboard and gamepad
  state per tick and sends it raw; the game registers a mapping from raw
  device state to its input type (`app.input_mapping(...)`), and the mapped,
  encoded input is what goes into the input log. (builder's call)
- Timeline: the logarithmic checkpoints of the parent design (9.3). Seeking
  back restores the nearest earlier checkpoint and replays inputs and edits.
  In 2a the timeline is linear: editing at a past tick discards the future
  after it (2b turns this into a branch).
- Playing from a past tick replays the recorded inputs and edits, like a
  replay. The future is discarded only when the user produces something new:
  an edit, or a live input that differs from the recorded one for that tick
  (the user "takes over"). (builder's call)
- The timeline shows the tick, the hash, and markers for edits.

### 9.4 Rebuilding the game

A rebuild restarts the game host and replays the current session from tick
0 with the new code up to the tick the user was on. Since the code changed,
the hashes may differ; the timeline shows the first tick whose hash differs
from the previous build. (builder's call)

### 9.5 Undo and redo (author)

- Every edit can be undone and redone, through the command palette and the
  usual shortcuts.
- At tick 0, undo walks back a history of scene versions kept by the
  editor for the session; saving is explicit and the scene shows an unsaved
  marker. (builder's call: explicit save)
- Past tick 0, undoing an edit removes it from the edit log and resimulates
  from the nearest checkpoint before it, so the state returns exactly to
  what it would have been without the edit. Redo puts it back. In 2a,
  undoing an edit also discards the future after it, consistent with the
  linear timeline. (builder's call)
- Undo history is per session and not persisted. (builder's call)

### 9.6 Game panics (builder's call)

- Desktop: a panic in a game system is caught by the game host, which
  restores the last good state, pauses, and reports the panic in the
  console. The session is kept intact.
- Browser: a panic aborts the WebAssembly instance, which cannot be caught.
  The editor recreates the worker from the last exported session (exported
  on every pause, every edit, and every second while playing) and reports
  the panic. Up to one second of live play can be lost.

## 10. Viewport

- Rendered by `ludenea-debug-draw`: wgpu compiled to WebAssembly in the
  front end (WebGPU, WebGL2 fallback), the same in Tauri and in the browser.
  It is the seed of the milestone 3 renderer. (builder's call, consistent
  with the parent design 14)
- Engine plugins provide primitives (lines, circles, polygons, capsules,
  text labels) per tick; physics draws colliders, contacts, and AABBs with
  toggles. The game host sends primitives only for the ticks the viewport
  displays.
- Interactions: pan, zoom, click to select (pick command), drag to move the
  selected entity's transform, keyboard nudging. Selection is shared with
  the scene tree and the inspector.

## 11. Performance and fluidity (author, numbers are the builder's call)

- The editor UI stays at 60 frames per second while the simulation plays:
  the simulation never runs on the UI thread (separate process or Web
  Worker).
- The editor window is interactive in under 2 seconds after opening a
  project, before the game is built.
- The entity list stays smooth at 100,000 entities (virtualized).
- State reaches the UI as deltas.
- Every action is reachable from the command palette and can have a
  shortcut; nothing is mouse-only.

## 12. Editor plugin API (author, exact shapes are the builder's call)

A plugin is a TypeScript module exporting one definition:

```ts
export default definePlugin({
  name: "combat",
  activate(editor) {
    editor.panels.register({ id: "combat.teams", title: "Teams", component: TeamsPanel });
    editor.inspectors.register("my_game::Health", HealthInspector);
    editor.commands.register({ id: "combat.heal-all", title: "Heal all", run: healAll });
  },
});
```

- One registration path per kind: panels, component inspectors, commands,
  shortcuts, viewport overlays, settings.
- `editor` exposes the typed control client, selection, timeline, and
  project service. Built-in panels use exactly this object.
- Component values are typed with the generated TypeScript types.
- Engine plugins ship their editor plugin alongside the crate (for example
  physics debug draw toggles); project plugins live in `editor/` of the
  project. Plugins are hot reloaded without restarting the game host or
  losing the timeline.

### 12.1 Building plugins without Node.js (author)

- The project service embeds rolldown (a JavaScript bundler written in
  Rust) as a library. It bundles each project plugin, and each engine
  plugin shipped with a crate, into one ES module. (the choice of rolldown
  is the builder's call; embedding a Rust bundler is the author's)
- React, the editor packages, and the generated types are provided by the
  editor at runtime (import map) and marked external in plugin bundles, so
  there is exactly one React instance.
- Hot reload is Ludenea's own: the project service watches plugin sources,
  rebuilds the changed bundle, and pushes a reload event; the shell
  deactivates the old plugin and activates the new one, keeping editor
  state outside plugins. No React fast refresh in 2a. (builder's call)
- TypeScript is stripped, not type-checked, by the bundler. Type checking
  happens in the developer's own editor tooling against the generated
  types. (builder's call)

## 13. Proof of done

- **Determinism in the browser:** the `boxes` golden trace verified in a
  headless browser, game host in a Web Worker, single-threaded (parent
  design 10.3).
- **Editor sessions replay:** an end-to-end test drives the browser editor
  (Playwright, builder's call) on the template project: open, play, pause,
  edit a component at a past tick, seek, play again, export the session. The
  CLI `verify` reproduces the same trace from the exported scene, input log,
  and edit log.
- **Plugin API parity:** a test project plugin adds a panel and a custom
  inspector with no access beyond the plugin object.
- **Protocol types:** CI fails if generated TypeScript types are stale.
- **Performance:** a benchmark scene of 100,000 entities, of which 1,000 are
  dynamic bodies (the parent design's physics target) and the rest are
  plain entities, keeps the UI at 60 frames per second while playing,
  measured in the end-to-end suite. The criterion is UI fluidity, not
  simulation speed: when the simulation cannot keep the tick rate (likely in
  single-threaded WebAssembly), the host runs slower than real time and the
  timeline shows the effective rate; it never skips ticks. (builder's call)

## 14. Milestone 2b outline

- Prefabs: reusable entity templates with field overrides.
- Data assets: game types with `#[derive(Asset)]`, stored as RON, edited in
  the inspector through schemas, content hashed because the simulation reads
  them.
- Replays as assets: scene, input log, edit log, checkpoints.
- Trace and snapshot viewer: open a CI trace or snapshot, see the divergence
  by entity, component, and field.
- Timeline branches: editing at a past tick forks the timeline instead of
  discarding the future; switch between branches; no merging.
- Ghost future: one other branch drawn as outlines in the viewport, plus
  the first divergent tick, entity, and field from the hash tree.

## 15. Left to the builder

- Exact protocol message shapes and the project protocol.
- Edit log encoding, within "canonical and versioned".
- Query filter grammar beyond `With` and `Without`.
- Checkpoint spacing constants and memory cap.
- Visual design of the editor.
