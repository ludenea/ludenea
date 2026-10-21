# Ludenea milestone 3: rendering

Date: 2026-10-20
Status: approved
Parent design: `2026-10-17-ludenea-engine-design.md` (section 14, the rendering
outline)

Marks used below: **(author)** means the author decided it in conversation or
in a parent design; **(builder's call)** means the designer chose it and the
author may revisit it. Unmarked text restates a parent design or the
project's rules in `AGENTS.md`.

## 1. Intent

Until now a Ludenea game is boxes drawn by the physics debug draw, inside the
editor or headless. Milestone 3 gives Ludenea a real renderer and a real game
runtime, so a game can look like a game and run on its own.

The milestone is driven by a concrete game **(author)**: a small local
two-player **platform fighter** in a fantasy setting, in the spirit of Smash
Bros or Brawlhalla. Fighters battle on an arena with platforms; damage raises
a percentage, knockback grows with it, each player has three stocks, and a
finished match is saved as a replay. That game is the requirements document
for this milestone, not part of it: the engine comes first, the fighter is the
next project, built on top **(author)**. The same fighter later becomes the
test bed for rollback netcode in milestone 4.

Why a real game drives it (builder's reading, confirmed by the author): it
surfaces real needs a demo would not, it is Ludenea's showcase, and versus
play is exactly the use case the engine exists for.

## 2. Goals, non-goals, anti-goals

### Goals

- A wgpu renderer for sprites, sprite animations, tilemaps, layers with
  parallax, and a 2D camera, with the physics debug draw on top.
- Smooth display at any refresh rate: the renderer interpolates between the
  last two ticks.
- A game runs on its own: a native window on macOS, Linux and Windows, and a
  standalone web page, launched by `ludenea play` without the editor.
- Up to four local players: gamepads and a shared keyboard, each assigned
  to a player.
- Texture and sprite sheet assets, packed into atlases at build time.
- Tilemaps painted in the editor **(author)**.
- Rendering reachable through the control protocol: any tick can be rendered
  to a PNG from the command line.
- The quality-bar items VPR-1 and VPR-2 (renderer pixel tests on WebGL2 and
  WebGPU, GPU errors and device limits) ship in this milestone **(author)**.

### Non-goals (milestone 3)

- Fighter gameplay: hitboxes, hurtboxes, one-way platforms, knockback rules,
  stocks, match flow. They belong to the fighter project, along with the
  physics they need (sensors, collision filters, one-way platforms).
- Custom shaders, lighting, particles, post-processing.
- Text rendering in the game, user interface widgets, audio.
- Networking.
- 3D, mobile, consoles, 32-bit targets.

### Anti-goals

A change that does any of these is a failure, even if it works:

- Rendering that writes simulation state, or any rendering change that moves
  a golden hash.
- Animation state that lives only in the renderer. A fighter's attack frame
  decides when it hits, so rollback would be wrong.
- A renderer that exists twice: one for the editor and one for games.
- A rendering capability only the editor can reach.
- Two position components for the same entity, or two ways to draw a sprite.

## 3. Constraints

- Determinism rules from `AGENTS.md` and `clippy.toml` apply to every
  simulated component this milestone adds (animation state, camera, tilemap).
  The renderer itself is not simulation: it may use floating point freely,
  but it never feeds anything back.
- wgpu is already pinned (`=30.0.1`); new dependencies (a window library, a
  gamepad library, a PNG decoder, an atlas packer) are pinned the same way and
  justified in their commit.
- Every new file format (texture meta, tilemap data) is versioned, decoded
  defensively, listed in the CHANGELOG formats table, and refused precisely
  when malformed. A game or a tool never panics on a bad PNG or a bad meta.
- Protocol changes follow the protocol policy: the first change bumps the
  control protocol version, later ones update the pinned fingerprint and the
  same CHANGELOG entry.
- Every behaviour ships with tests; rendering is tested with pixel checks on
  each backend.

## 4. Architecture

### 4.1 One renderer, fed by a render frame (builder's call)

A new crate `ludenea-render` grows from `ludenea-debug-draw` and replaces its
browser-only renderer. It runs natively (Metal, Vulkan, DirectX 12) and in the
browser (WebGPU, WebGL2 as fallback).

The renderer never sees the world. After each tick, the host **extracts** a
`RenderFrame` from it: a plain, serializable description of what to draw
(camera, layers, sprites with their atlas region, tilemap chunks, debug draw
primitives, teleport marks). This mirrors how the debug draw already works
today over the control protocol, and it is what lets one renderer serve every
place a game runs:

| Where the game runs | How the render frame reaches the renderer |
| --- | --- |
| Native window | In process, every tick |
| Standalone web page | From the Web Worker to the page, every tick |
| Editor (browser or desktop) | Over the control protocol for the tick on screen |
| Command line | In process, rendered offscreen to a PNG |

The renderer keeps the last two frames and interpolates between them by the
fraction of time elapsed since the last tick: positions, rotations and the
camera. Entities present in only one of the two frames are drawn without
interpolation, as are entities marked as teleported in the newer frame.

Extraction is engine code written against the public API (`AGENTS.md`: no
privileged API), registered by a render plugin the same way physics registers
its debug draw layers. A game can add its own extraction for its own
components.

The `RenderFrame` shape is part of the control protocol and versioned with
it.

### 4.2 Position

Sprites and tilemaps are placed by the same position component physics
already uses. If that component must move out of `ludenea-physics` so that an
entity without a body can be drawn, it moves; the milestone never adds a
second position component (builder's call on where it lives).

## 5. Simulated components (builder's call on exact shapes)

These live in the world, are deterministic, hashed, saved in scenes and
snapshots, and replayed:

- **Sprite**: a texture handle, a region or animation clip, a layer, an
  order within the layer, horizontal and vertical flip, a tint.
- **SpriteAnimation**: the playing clip, the tick it started on, looping or
  one-shot, speed. The current frame is a pure function of the tick and of
  the clip's timing, so game code can ask "which frame is showing" and the
  renderer computes the same answer. Systems can switch clips; the fighter
  will use this to activate a hitbox on an attack's frame 4.
- **Camera2d**: position, zoom, the minimum area in world units that must be
  visible, and which camera is active. Game code drives it; the fighter will
  frame all players and zoom accordingly. Because it is in the world, a
  replay replays the camera too. The window never enters the simulation: the
  renderer fits the minimum area into whatever aspect ratio it draws to and
  shows more of the world on the longer side, never stretching. A match
  played in a wide window and a tall one has the same hashes (builder's
  call).
- **Tilemap**: a tileset handle, a grid (tile size, width, height), tile
  indices, a layer. Rendered in chunks, one draw per chunk.
- **Teleported**: a one-tick mark that cuts interpolation, for a fighter
  respawning after being knocked out.

Layers are declared by the game with an explicit order and a parallax factor
per layer (background layers scroll slower than the arena).

## 6. Assets

### 6.1 Textures, sprite sheets and animation clips (builder's call on the formats)

Two kinds of data, split by whether the simulation depends on them:

- **Texture (rendering only).** A new asset kind for PNG images. Its `.meta`
  describes how the image is cut: a single image, or a grid of frames (frame
  size, margins, spacing), and an optional pivot point so feet stay on the
  ground when frames differ in size. Nearest filtering by default for pixel
  art. Nothing in it affects the simulation, so headless runs and `verify`
  never need the PNG.
- **Animation clips (simulated).** A data asset, exactly like the existing
  data assets of milestone 2b: named clips, each a texture handle, a range of
  frames, and a duration per frame in ticks. Because a fighter's hit timing
  depends on it, it follows every data asset rule: loaded headless and in
  WebAssembly, part of the `ludenea::Assets` hash leaf, embedded in snapshots
  and replays, and edited past tick 0 as a recorded `SetAsset` edit.

This matches the one-sheet-per-animation layout of the chosen art (section
11).

### 6.2 Atlases (builder's call)

Textures are packed into atlases at build time (`ludenea build`, the editor's
build), so the renderer binds few textures and draws sprites in batches. The
packing must be reproducible: the same inputs give the same atlas bytes. The
atlas is a build output, never committed.

### 6.3 Hot reload

When a PNG or its texture meta changes on disk, the editor and a running game
pick up the new texture without a restart. That never changes simulation
state, so it never affects the hash. A changed animation clip asset reloads
like any data asset change (milestone 2b, section 5.4), which does change the
simulation, deliberately and as a recorded edit.

## 7. Game runtime

### 7.1 Native window (builder's call on the window library and command names)

Today `run` is a headless subcommand of each game's own binary
(`ludenea_cli::main`), and the `ludenea` tool only has `new` and `serve`;
`ludenea-project` builds games for WebAssembly only. The names below avoid
the clash:

- The game binary gains `play`, which opens the window, and `render`
  (section 8). Its headless `run` stays as it is.
- The `ludenea` tool gains `ludenea play`, which builds the game natively and
  launches its `play`, and `ludenea build --web`, which produces the
  standalone page (section 7.2). Native builds become a build target of
  `ludenea-project` next to WebAssembly.

`play` opens the game in a native window: the simulation at its fixed
tick rate, the renderer at the display rate with interpolation, live input
from keyboard and gamepads. The window resizes, goes fullscreen, and honours
vsync. Closing it ends the game cleanly; a match can be saved as a replay.

### 7.2 Standalone web page

The same game builds to a web page with a canvas: the simulation in a Web
Worker as today, the renderer on the page. It runs without the editor and
without `ludenea serve`, from any static file host.

### 7.3 Local players (builder's call on the assignment rules)

- Gamepads and a shared keyboard (two halves of the keyboard) map to
  players, up to four. The game's existing `input_mapping` receives the raw
  input of one player; the runtime decides which raw devices feed which
  player.
- The player count stays fixed for a whole simulation, as `set_players` and
  the input log already require. Joining happens **before the match starts**,
  outside the simulation: the runtime shows which devices are claimed, a
  player presses a button to claim a slot, and the match starts with that
  many players. Changing the count means starting a new match.
- A gamepad that disconnects mid-match keeps its slot: its player receives
  the default input until it reconnects, and the input log records that like
  any other input.
- The editor's input capture gains the same multi-player support (today it
  captures one keyboard and the first gamepad), with the player count set by
  the session.
- Input log and replay formats do not change.

## 8. Control surface and command line (builder's call on exact shapes)

- A control command returns the `RenderFrame` of a given tick, replacing or
  generalizing today's `get_debug_draw`. While the session plays, the host
  pushes each tick's frame as an event, so the editor interpolates exactly
  like the native window; paused or seeking, it asks for the tick on screen.
- `render <scene or replay> --tick N --out frame.png` (a game binary
  subcommand, section 7.1) renders a tick offscreen. It serves CI pixel tests and future divergence reports with
  pictures.
- Headless rendering in CI needs a software adapter on each platform; the
  builder picks it and documents it.

## 9. Editor

- The viewport uses `ludenea-render` and draws sprites, tilemaps and layers,
  with the debug draw on top, for the tick on screen.
- The inspector previews a texture and plays an animation clip.
- **Tilemap painting (author)**: select a tilemap entity, pick a tile from the
  tileset palette, paint and erase on the grid in the viewport. Every stroke
  is an edit recorded as an input, undoable, and reachable through the control
  protocol like every other edit. A rectangle fill is enough; no brushes or
  auto-tiling.

## 10. Quality

- **VPR-1** and **VPR-2** from the quality-bar backlog ship here, extended to
  the native backends.
- Pixel tests: reference frames sampled at known pixels on every backend
  (WebGL2, WebGPU, and native through the software adapter), covering a
  sprite, a flipped sprite, an animation frame, a tilemap, layer order,
  parallax, and interpolation halfway between two ticks.
- Determinism: the state hashes in the golden traces do not move. A golden
  file whose bytes change for another reason (a schema fingerprint after new
  components are registered, a moved position component) is recorded again
  with the README commands and explained in its commit, and its trace hashes
  must stay identical. The same example scene runs to the same hashes
  headless, in the native window, on the web page and in the editor.
- Performance target (builder's call): 10,000 sprites at 144 frames per second
  on a mid-range laptop, which means geometry stays on the GPU between frames
  (the direction of quality-bar item VPR-4).
- Malformed PNGs, metas and tilemaps are refused with precise errors, and
  covered by tests.

## 11. Art

The art chosen by the author:

- Characters by LuizMelo, all CC0: Fantasy Warrior, Evil Wizard 2, Huntress,
  Martial Hero (https://luizmelo.itch.io). Each ships idle, run, jump, fall,
  two or three attacks, hit and death as one PNG sheet per animation.
- Arena by ansimuz, CC0: Gothicvania Town, with a tileset and two parallax
  layers (https://opengameart.org/content/gothicvania-town). Its bundled music
  is not by the artist and is not committed.

Assets are made by humans; nothing generated by AI is accepted **(author)**.
They are committed to the public repository (allowed by CC0) with a
`CREDITS.md` naming each artist and pack, even though CC0 does not require it.
The packs differ in scale; harmonizing them (the wizard is larger) is the
fighter project's concern, not the renderer's.

## 12. Definition of done

An example scene with one LuizMelo fighter, animated (idle, run, jump, fall),
flipped by direction, moved with a gamepad or the keyboard, on a Gothicvania
tilemap painted through the control surface (the same recorded edits the
editor's tile palette sends, so an agent builds the level without a human;
the editor's painting is proven by its own end-to-end test) **(author)**,
with parallax background layers and a camera
that follows the fighter. The ground and platforms are ordinary static
colliders placed by hand under the tiles; generating colliders from a
tilemap is left to the fighter project if it needs it (builder's call). It
runs identically in the native window, the standalone web page and the
editor, to the same hashes as headless, and `render` produces a PNG of any
tick. Two players can join on the same machine before the match, each
controlling a fighter.

## 13. Left to the builder

- Exact component, protocol and meta shapes.
- The window, gamepad, PNG and atlas packing libraries.
- The software adapter used for headless rendering in CI.
- Crate boundaries beyond `ludenea-render`, and the migration from
  `ludenea-debug-draw`.
- The order of work, as long as each step keeps the goldens and the existing
  editor working.
