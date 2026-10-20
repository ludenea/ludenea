# Ludenea Milestone 3a: Render Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A wgpu renderer that draws sprites, sprite animations, tilemaps, layers with parallax, a 2D camera and the debug draw, interpolated between ticks, on native backends, in the browser and offscreen, fed by a serializable render frame that the simulation extracts and the control protocol and the command line expose.

**Architecture:** `ludenea-debug-draw` becomes `ludenea-draw`, the GPU-free data model: debug primitives, their mesh, and the new `RenderFrame`. `ludenea-app` gains render layers and extractors as app hooks and `Simulation::render_frame`; `Pose` moves into `ludenea-app` so an entity without a body can be drawn. A new crate `ludenea-render` holds the simulated render components (`Sprite`, `SpriteAnimation`, `SpriteSheet`, `Camera2d`, `Tilemap`, `Teleported`), the `RenderPlugin` and its extraction, atlas packing, interpolation and camera fitting (all GPU-free), and behind feature `gpu` the one wgpu renderer, with a browser wrapper behind feature `web`. The control protocol replaces `GetDebugDraw` with `GetRenderFrame`, and each game binary gains `render`, which writes a tick to a PNG.

**Tech Stack:** unchanged (Rust 1.97.1, wgpu `=30.0.1`, serde, postcard, ron, wasm-bindgen, clap; TypeScript, Playwright). New pinned dependency: `png` (decode and encode, pure Rust, builds for wasm32). `pollster` (pinned) to block on adapter requests in native tests and the `render` command.

**Spec:** `.claude/superpowers/specs/2026-10-20-ludenea-rendering-3-design.md` (parent: `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md` section 14). This plan covers spec sections 4, 5, 6.1, the packing half of 6.2, 8 (without pushed frames) and 10 (without the performance target). The game runtime (spec section 7, `ludenea play`, `build --web` with atlases packed at build time, local players, and the 10,000 sprites at 144 frames per second target, measured in the native window) is milestone 3b; the editor (section 9, the viewport on the new renderer, previews, tilemap painting, pushed frames during play, hot reload, the panel's retry) and the example scene with the art (sections 11 and 12) are milestone 3c.

## Global Constraints

- Every Global Constraint of the milestone 1, 2a, 2b and quality-bar plans still applies (determinism lints for simulation code, target gates, committed generated files, `--locked`, commit style).
- The state hashes of `golden/` never move. `examples/boxes` does not add `RenderPlugin` and registers nothing new. A golden file whose bytes change for another reason is recorded again with the README commands and explained in its commit, and its trace hashes stay identical (spec section 10).
- The simulation never pulls wgpu: `ludenea-render` without features depends on no GPU crate. `gpu` adds wgpu (native backends on native targets, WebGPU and WebGL2 on `wasm32-unknown-unknown`); `web` adds `gpu` plus wasm-bindgen and web-sys.
- Rendering code (`ludenea-render::gpu`, `::web`, `::interpolate`, `::camera`) is display only. Any allowed float function carries `#[expect(clippy::disallowed_methods, reason = "display only: ... never feeds back into the simulation")]`, as in today's `render.rs`. Simulation code (`components`, `animate`, `extract`) follows every determinism lint.
- Stable names: `ludenea::Pose` (unchanged), `ludenea::Sprite`, `ludenea::SpriteAnimation`, `ludenea::SpriteSheet` (asset), `ludenea::Camera2d`, `ludenea::Tilemap`, `ludenea::Teleported`.
- Versions: `PROTOCOL_VERSION = 5` (bumped once, Task 10), `META_VERSION = 2` (version 1 still reads, Task 4), project protocol bumped once to 4 (Task 4). Every bump updates the CHANGELOG formats table and the pinned fingerprints in the same task.
- Texture defaults: `pixels_per_unit = 16`, filter `Nearest`, pivot at the frame centre. Atlas page side at most `min(4096, device max_texture_dimension_2d)`; every frame is extruded by 1 pixel.
- Malformed PNGs, metas, sheets, cameras and tilemaps are refused with errors naming the file or the entity and the field; nothing reachable by a user panics.
- A test that needs a GPU never skips silently: with no adapter it fails naming the missing adapter. CI provides a software adapter (Task 6).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p ludenea-render --features gpu --all-targets -- -D warnings`, `cargo test --locked --workspace`, `cargo test --locked -p ludenea-render --features gpu`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- `SpriteAnimation` has no speed field: a clip's `ticks_per_frame` sets its pace, and a game wanting a faster attack defines a faster clip. One way to set timing.
- `Camera2d` has no position or zoom of its own: its centre is the entity's `Pose`, and its zoom is its `view` (the minimum visible area). One position component, one way to zoom.
- An animation is applied by a simulation system (`animate`) that writes `Sprite::frame` and `Sprite::texture` every tick, so `Sprite` is the only thing the renderer and game code read.
- Until milestone 3c moves the editor viewport, the old `ViewportRenderer` in `ludenea-draw` (formerly `ludenea-debug-draw`) keeps drawing the editor; 3c deletes it. The new renderer is exercised by its own tests and fixture page.
- A tilemap's `tiles` travel whole in each render frame; chunking is a renderer concern (one draw per 32 by 32 chunk).

## Review Focus

- **A broken PNG or a grid that does not fit the image** (truncated file, wrong signature, frame size larger than the image, margins and spacing that leave a partial frame): `TextureError` naming the asset identifier, the path when known, and the numbers; the other textures still pack. Tests in Task 5.
- **A sprite whose texture is not loaded, or whose frame does not exist**: drawn as a magenta placeholder of one world unit, and `DrawReport::missing` names the entity, texture and frame; the rest of the frame draws. Test in Task 6.
- **An animation naming a clip its sheet does not have** (a typo, or a clip renamed in the sheet during a session): the sprite keeps its last frame, and the next render frame carries a warning naming the entity, the clip and the clips the sheet has. Tests in Task 3.
- **A malformed simulated component in a scene** (a camera view that is zero, negative or not finite; tilemap `tiles.len() != width * height`; a clip with `count == 0` or `ticks_per_frame == 0`): the scene load is refused naming the entity, component and field. Tests in Task 3.
- **A frame larger than the device allows** (an atlas past `max_texture_dimension_2d`, a sprite batch past `max_buffer_size`): atlases split into pages and batches split below the limit; nothing is refused at ordinary sizes, and only a single texture larger than a page is refused with `TextureError::TooLarge`. Tests in Tasks 5 and 8.

---

### Task 1: `Pose` moves into `ludenea-app`

**Files:**
- Create: `crates/ludenea-app/src/pose.rs` (moved from `crates/ludenea-physics/src/components.rs`: `Pose`, `PoseError`, `PoseData`, `UNIT_ROTATION_TOLERANCE`)
- Modify: `crates/ludenea-physics/src/{components.rs, lib.rs}` (`pub use ludenea_app::pose::{Pose, PoseError};`), `crates/ludenea-app/src/lib.rs`, `crates/ludenea/src/lib.rs` (prelude takes `Pose` from `app`), `CHANGELOG.md`. `PhysicsPlugin` keeps registering `Pose`; `RenderPlugin` (Task 3) registers it too, so registering the same component twice must be a no-op.
- Test: `crates/ludenea-app/tests/pose.rs` (the pose tests moved from physics)

**Interfaces:**
- Produces: `ludenea_app::pose::{Pose, PoseError}` with the same stable name, serde shape, validation and messages; `ludenea_physics::Pose` stays a re-export, so game code compiles unchanged.

- [ ] **Step 1: Move the tests**: the existing pose tests into `crates/ludenea-app/tests/pose.rs`, plus `pose_registered_by_two_plugins_is_registered_once`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test pose`. Expected: compile failure.
- [ ] **Step 3: Move** the code; check `World::register` already tolerates the same type twice, and make it do so if not.
- [ ] **Step 4: Run** `cargo test --locked --workspace`. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `♻️ refactor(app): move Pose out of physics so any entity can be placed`.

---

### Task 2: `ludenea-draw` and the render frame

**Files:**
- Rename: `crates/ludenea-debug-draw` to `crates/ludenea-draw` (package `ludenea-draw`), every dependent `Cargo.toml`, `xtask/src/main.rs`, the facade (`ludenea::draw` replaces `ludenea::debug_draw`, no alias), README, CHANGELOG (breaking rename)
- Create: `crates/ludenea-draw/src/frame.rs`
- Modify: `crates/ludenea-draw/src/lib.rs` (`Color` derives `Schema`), `crates/ludenea-app/src/{app.rs, simulation.rs}`
- Test: `crates/ludenea-draw/src/frame.rs` (inline), `crates/ludenea-app/tests/render_frame.rs`

**Interfaces:**
- Produces (`ludenea_draw::frame`), all `Clone, Debug, PartialEq, Serialize, Deserialize`, `ts` derive like `DebugDrawFrame`:
  - `pub struct RenderFrame { pub tick: u64, pub camera: Option<CameraView>, pub layers: Vec<LayerInfo>, pub sprites: Vec<SpriteItem>, pub tilemaps: Vec<TilemapItem>, pub teleported: Vec<Entity>, pub debug: DebugDrawFrame, pub warnings: Vec<String> }`
  - `pub struct CameraView { pub center: Vec2, pub view: Vec2 }` (view: minimum visible width and height in world units)
  - `pub struct LayerInfo { pub name: String, pub parallax: f32 }` (draw order is the index; parallax 1.0 moves with the world, 0.0 is fixed to the screen)
  - `pub struct SpriteItem { pub entity: Entity, pub layer: u16, pub order: i32, pub texture: AssetId, pub frame: u32, pub position: Vec2, pub rotation: Rot, pub flip_x: bool, pub flip_y: bool, pub tint: Color }`
  - `pub struct TilemapItem { pub entity: Entity, pub layer: u16, pub order: i32, pub tileset: AssetId, pub position: Vec2, pub cell_size: Vec2, pub width: u32, pub height: u32, pub tiles: Vec<Option<u32>> }` (row-major from the bottom-left cell at `position`)
  - `RenderFrame::layer_index(&self, name: &str) -> Option<u16>`
- Produces (`ludenea_app`): `pub type ExtractFn = fn(&World, &mut RenderFrame);` `App::render_layer(&mut self, name: &'static str, parallax: f32) -> &mut Self` (declaration order is draw order; a repeated name is an `AppError::DuplicateLayer(String)` at `build`), `App::extractor(&mut self, extract: ExtractFn) -> &mut Self`, `Simulation::render_frame(&self, debug_layers: &BTreeSet<String>) -> Result<RenderFrame, SimError>`: fills `tick`, `layers`, then runs the extractors in registration order, then the debug drawers into `debug`. Game panics inside an extractor are caught like debug drawers'.

- [ ] **Step 1: Write the failing tests**: in `frame.rs`: `render_frame_round_trips_through_json`, `layer_index_finds_declared_layers`; in `render_frame.rs`: `layers_come_in_declaration_order`, `duplicate_layer_is_refused_at_build`, `extractors_run_in_registration_order`, `render_frame_carries_the_debug_draw`, `extractor_panic_is_reported_like_a_drawer_panic`, `render_frame_does_not_change_the_hash`.
- [ ] **Step 2: Run** `cargo test -p ludenea-draw -p ludenea-app`. Expected: compile failure.
- [ ] **Step 3: Implement** the rename, `frame.rs`, and the app hooks.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-control --test typescript && pnpm -C editor typecheck`. Expected: PASS; regenerated TypeScript committed.
- [ ] **Step 5: Commit** `✨ feat(draw): add the render frame a simulation extracts for the renderer`.

---

### Task 3: Render components, sprite sheets, animation and extraction

**Files:**
- Create: `crates/ludenea-render/{Cargo.toml, src/lib.rs, src/components.rs, src/sheet.rs, src/animate.rs, src/extract.rs}`; workspace member; facade `ludenea::render` and prelude (`RenderPlugin`, `Sprite`, `SpriteAnimation`, `SpriteSheet`, `Clip`, `Camera2d`, `Tilemap`, `Teleported`)
- Test: `crates/ludenea-render/tests/{components.rs, animate.rs, extract.rs}`

**Interfaces:**
- Consumes: Task 1 `Pose`, Task 2 `RenderFrame`, `App::render_layer`, `App::extractor`.
- Produces (`ludenea_render`), components validated through serde mirrors like `Pose`:
  - `Sprite { pub texture: AssetId, pub frame: u32, pub layer: String, pub order: i32, pub flip_x: bool, pub flip_y: bool, pub tint: Color }` (`Default`: tint `0xFFFFFFFF`, layer `""` meaning the first declared layer)
  - `SpriteAnimation { pub sheet: Handle<SpriteSheet>, pub clip: String, pub started: u64, pub looping: bool }`
  - asset `SpriteSheet { pub texture: AssetId, pub clips: BTreeMap<String, Clip> }`, `Clip { pub first: u32, pub count: u32, pub ticks_per_frame: u32 }`; `count` and `ticks_per_frame` at least 1. `texture` is a bare `AssetId` on purpose: a handle would make the simulation load the PNG.
  - `Clip::frame_at(&self, elapsed: u64, looping: bool) -> u32`: `first + (elapsed / ticks_per_frame) % count` when looping, else clamped to the last frame.
  - `SpriteAnimation::frame(&self, sheet: &SpriteSheet, tick: u64) -> Result<u32, AnimationError>`, `AnimationError::UnknownClip { clip: String, known: Vec<String> }`; a tick before `started` counts as elapsed 0.
  - `Camera2d { pub view: Vec2 }` (both components finite and above 0)
  - `Tilemap { pub tileset: AssetId, pub cell_size: Vec2, pub width: u32, pub height: u32, pub tiles: Vec<Option<u32>>, pub layer: String, pub order: i32 }` (`tiles.len() == width * height`, `cell_size` finite and above 0)
  - `Teleported { pub tick: u64 }`: interpolation is cut for this entity in the frame of that tick.
  - `pub struct RenderPlugin;` registers every component above, `Pose`, the asset, the `animate` system (label `render::Animate`, after `physics::Step` when present, else at the end of the tick), and the extractor.
  - `animate`: for each `(SpriteAnimation, Sprite)`, writes `Sprite::texture = sheet.texture` and `Sprite::frame`; on `UnknownClip` leaves the sprite unchanged.
  - Extraction: the entity with a `Camera2d` and a `Pose` with the lowest `Entity` gives `camera` (more than one adds a warning naming them); every `Sprite` with a `Pose` becomes a `SpriteItem`; every `Tilemap` with a `Pose` a `TilemapItem`; `Teleported { tick }` equal to the frame's tick lists the entity in `teleported`. Warnings, each naming the entity: a sprite or tilemap without `Pose`, an undeclared layer (listing the declared ones; the item is skipped), an animation with an unknown clip. Items are sorted by `(layer, order, entity)`.

- [ ] **Step 1: Write the failing tests**: `components.rs`: `zero_camera_view_is_refused_at_load`, `tilemap_with_wrong_tile_count_is_refused_naming_the_counts`, `clip_with_zero_ticks_per_frame_is_refused` (Review Focus), `render_components_round_trip_through_a_scene`; `animate.rs`: `looping_clip_wraps`, `one_shot_clip_holds_its_last_frame`, `animation_writes_sprite_frame_and_texture_each_tick`, `unknown_clip_keeps_the_last_frame` (Review Focus), `animation_is_deterministic_across_thread_counts` (1 and 8 threads, same hashes over 600 ticks); `extract.rs`: `sprites_are_extracted_sorted_by_layer_order_and_entity`, `camera_comes_from_pose_and_view`, `two_cameras_warn_and_keep_the_lowest_entity`, `undeclared_layer_is_skipped_with_a_warning_listing_layers`, `sprite_without_pose_warns`, `unknown_clip_warns_naming_known_clips` (Review Focus), `teleported_is_listed_only_on_its_tick`, `plugin_does_not_change_boxes_hashes` (boxes plus `RenderPlugin`, no render component spawned: same trace hashes).
- [ ] **Step 2: Run** `cargo test -p ludenea-render`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(render): add sprite, animation, camera and tilemap components`.

---

### Task 4: Texture metas

**Files:**
- Modify: `crates/ludenea-app/src/meta.rs`, `crates/ludenea-project` (asset listing, TypeScript), `crates/ludenea/tests/formats_documented.rs`, `CHANGELOG.md` (formats table: Meta current 2, oldest readable 1; project protocol 4), generated TypeScript
- Test: inline tests in `meta.rs`

**Interfaces:**
- Produces (`ludenea_app::meta`):
  - `META_VERSION = 2`; version 1 metas still read.
  - `AssetKind::Texture(TextureMeta)`; `AssetKind::of` gives `Texture(TextureMeta::default())` for `.png`.
  - `pub struct TextureMeta { pub pixels_per_unit: u32, pub grid: Option<Grid>, pub pivot: Option<(u32, u32)>, pub filter: Filter }` (`Default`: 16, `None`, `None`, `Nearest`); `pub struct Grid { pub frame_width: u32, pub frame_height: u32, pub margin: u32, pub spacing: u32 }`; `pub enum Filter { Nearest, Linear }`. `pivot` is in pixels from the frame's top-left; `None` is the centre.
  - `TextureMeta::validate(&self) -> Result<(), String>`: `pixels_per_unit` and frame sizes at least 1.
  - Meta file form: `Meta(version: 2, id: "...", kind: Texture((pixels_per_unit: 16, grid: Some((frame_width: 100, frame_height: 55, margin: 0, spacing: 0)), pivot: None, filter: Nearest)))`.

- [ ] **Step 1: Write the failing tests**: `version_1_meta_still_reads`, `png_gets_a_default_texture_meta`, `texture_meta_round_trips`, `texture_meta_with_zero_frame_width_is_refused_naming_the_field`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app meta`. Expected: FAIL.
- [ ] **Step 3: Implement**; bump the project protocol and its fingerprint.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-project --test typescript && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(meta): describe how a PNG is cut into frames`.

---

### Task 5: PNG decoding and atlas packing

**Files:**
- Create: `crates/ludenea-render/src/atlas.rs`; `png` pinned in the workspace
- Test: `crates/ludenea-render/tests/atlas.rs`, fixtures in `crates/ludenea-render/tests/fixtures/` (small generated PNGs, committed)

**Interfaces:**
- Consumes: Task 4 `TextureMeta`.
- Produces (`ludenea_render::atlas`, GPU-free):
  - `pub struct TextureSource<'a> { pub id: AssetId, pub path: Option<&'a str>, pub png: &'a [u8], pub meta: &'a TextureMeta }`
  - `pub fn pack(sources: &[TextureSource<'_>], max_page: u32) -> Result<Atlas, Vec<TextureError>>`: decodes, cuts frames (row-major, left to right, top to bottom), extrudes each frame by 1 pixel, packs by shelves in a fixed order (height descending, then `(id, frame)`), opening a new page when one is full. Errors are per texture; textures without errors still pack, and the caller gets both (`Atlas::errors`).
  - `pub struct Atlas { pub pages: Vec<AtlasPage>, pub regions: BTreeMap<(AssetId, u32), Region>, pub errors: Vec<TextureError> }`, `pub struct AtlasPage { pub width: u32, pub height: u32, pub rgba: Vec<u8> }`, `pub struct Region { pub page: u32, pub x: u32, pub y: u32, pub width: u32, pub height: u32, pub pivot: (u32, u32), pub pixels_per_unit: u32, pub filter: Filter }` (rectangle without the extrusion).
  - `pub enum TextureError { Decode { id, path: Option<String>, reason: String }, Grid { id, path, image: (u32, u32), grid: Grid }, TooLarge { id, path, size: (u32, u32), max: u32 }, Meta { id, path, reason: String } }` with messages naming all fields.

- [ ] **Step 1: Write the failing tests**: `single_image_is_one_frame`, `grid_cuts_frames_row_major`, `margins_and_spacing_are_honoured`, `frames_are_extruded_by_one_pixel`, `packing_is_reproducible_byte_for_byte` (same sources in another order give identical pages), `full_page_opens_a_second_page`, `texture_larger_than_a_page_is_too_large`, `truncated_png_is_refused_and_others_still_pack` (Review Focus), `grid_larger_than_image_is_refused_naming_sizes` (Review Focus), `partial_last_frame_is_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --test atlas`. Expected: compile failure.
- [ ] **Step 3: Implement** `atlas.rs`.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-render`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(render): pack sprite sheets into reproducible atlases`.

---

### Task 6: The wgpu renderer: device, sprites, camera, offscreen pixels

**Files:**
- Create: `crates/ludenea-render/src/{camera.rs, gpu/mod.rs, gpu/device.rs, gpu/sprites.rs, gpu/shaders/sprite.wgsl}`; feature `gpu`; `pollster` pinned (dev and `gpu`)
- Modify: `.github/workflows/determinism.yml` (a `render` job on macOS, Linux and Windows running `cargo test --locked -p ludenea-render --features gpu`; Linux installs `mesa-vulkan-drivers` for lavapipe, Windows uses the WARP fallback adapter, macOS uses Metal), README (how to run the GPU tests)
- Test: `crates/ludenea-render/tests/gpu.rs` (`#![cfg(feature = "gpu")]`), `camera.rs` inline

**Interfaces:**
- Consumes: Tasks 2, 3, 5.
- Produces (`ludenea_render::camera`, GPU-free): `pub fn fit(view: Vec2, aspect: f32) -> Vec2` (the visible area: `view` widened on the longer side to match `aspect = width / height`, never narrowed, never stretched); `pub fn world_to_clip(point: Vec2, camera: CameraView, visible: Vec2, parallax: f32) -> [f32; 2]` (parallax scales the camera offset: a layer at `p` sees the camera at `center * p`). A frame without a camera uses centre zero and a view of 20 by 11.25 world units.
- Produces (`ludenea_render::gpu`):
  - `pub async fn headless() -> Result<(wgpu::Device, wgpu::Queue), RenderError>`: requests a hardware adapter, then the fallback adapter; `RenderError::NoAdapter(String)` lists what was tried.
  - `pub struct Renderer`; `Renderer::new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Renderer`; `set_atlas(&mut self, atlas: &Atlas) -> Result<(), RenderError>` (uploads pages, one sampler per filter); `draw(&mut self, target: &wgpu::TextureView, size: Size, frames: Frames<'_>, overlay: &Overlay) -> Result<DrawReport, RenderError>`; `render_rgba(&mut self, size: Size, frames: Frames<'_>, overlay: &Overlay) -> Result<Vec<u8>, RenderError>` (offscreen, tightly packed RGBA rows, top row first).
  - `pub struct Size { pub width: u32, pub height: u32 }` (physical pixels), `pub struct Frames<'a> { pub previous: Option<&'a RenderFrame>, pub current: &'a RenderFrame, pub alpha: f32 }`, `pub struct Overlay { pub debug_layers: BTreeSet<String>, pub selected: BTreeSet<Entity>, pub camera: Option<CameraView> }` (`camera` overrides the frame's, for the editor's free camera), `pub struct DrawReport { pub missing: Vec<(Entity, AssetId, u32)> }`.
  - `pub enum RenderError { NoAdapter(String), Gpu { operation: &'static str, message: String }, Limit { operation: &'static str, limit: &'static str, requested: u64, allowed: u64 }, DeviceLost(String) }`.
  - Sprite drawing: one instanced quad per sprite, batched by `(layer, page, filter)` in item order; size in world units = region size / `pixels_per_unit`; the pivot sits on the entity's position; flips mirror around the pivot; tint multiplies the texel; premultiplied alpha blending. A missing region draws a magenta quad of one world unit and is reported.
  - In this task `draw` uses `frames.current` only; Task 8 adds interpolation.

- [ ] **Step 1: Write the failing tests**: `camera.rs`: `fit_widens_the_longer_side`, `fit_never_narrows_the_view`, `parallax_zero_ignores_the_camera`; `gpu.rs` (each renders 64 by 64 offscreen and samples pixels): `clears_to_the_background`, `draws_a_sprite_at_its_pose`, `pivot_sits_on_the_position`, `flip_x_mirrors_the_texels`, `tint_multiplies`, `nearest_filter_keeps_hard_edges`, `animation_frame_selects_its_region`, `missing_texture_draws_a_placeholder_and_reports_it` (Review Focus), `wide_and_tall_targets_show_the_same_minimum_view`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --features gpu`. Expected: compile failure.
- [ ] **Step 3: Implement** `camera.rs`, the device, the sprite pipeline and offscreen read-back.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-render --features gpu && cargo clippy -p ludenea-render --features gpu --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(render): draw sprites with wgpu through a fitted 2D camera`.

---

### Task 7: Tilemaps, layers with parallax, and the debug draw

**Files:**
- Create: `crates/ludenea-render/src/gpu/{tilemaps.rs, debug.rs, shaders/debug.wgsl}`
- Modify: `crates/ludenea-render/src/gpu/mod.rs`
- Test: `crates/ludenea-render/tests/gpu.rs`

**Interfaces:**
- Consumes: Task 6 `Renderer`, `ludenea_draw::mesh::triangulate`.
- Produces: `draw` draws layer by layer in `frame.layers` order; within a layer, tilemaps and sprites interleave by `(order, entity)`. Tilemaps are cut into 32 by 32 cell chunks, each one instanced draw, rebuilt only when the tilemap item changes. The debug draw of `overlay.debug_layers` is drawn last, with parallax 1.0, selected entities highlighted as the editor does today (`mesh::triangulate`).

- [ ] **Step 1: Write the failing tests**: `tilemap_draws_tiles_at_their_cells`, `empty_cells_stay_transparent`, `tilemap_row_zero_is_at_the_bottom`, `later_layers_draw_over_earlier_ones`, `order_sorts_within_a_layer`, `background_layer_scrolls_slower`, `debug_draw_is_on_top`, `debug_layers_not_requested_are_hidden`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --features gpu`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-render --features gpu`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(render): draw tilemaps, parallax layers and the debug draw`.

---

### Task 8: Interpolation, teleports, GPU errors and device limits

**Files:**
- Create: `crates/ludenea-render/src/interpolate.rs`
- Modify: `crates/ludenea-render/src/gpu/{mod.rs, device.rs, sprites.rs}`
- Test: `interpolate.rs` inline, `crates/ludenea-render/tests/gpu.rs`

**Interfaces:**
- Produces (`ludenea_render::interpolate`, GPU-free): `pub fn interpolate(previous: &RenderFrame, current: &RenderFrame, alpha: f32) -> RenderFrame`: `current` with sprite and tilemap positions, sprite rotations (shortest arc, normalized) and the camera centre and view blended from `previous` by `alpha` (clamped to 0..=1); entities missing from `previous` or listed in `current.teleported` are not blended. `draw` and `render_rgba` call it when `frames.previous` is present.
- Produces (VPR-2, renderer side): the device's uncaptured errors are stored and returned by the next `set_atlas`, `draw` or `render_rgba` as `RenderError::Gpu { operation, message }`; sprite instance buffers split below `max_buffer_size`; targets larger than `max_texture_dimension_2d` are refused with `RenderError::Limit` naming `max_texture_dimension_2d`; a lost device is reported once as `DeviceLost`, and the caller recreates the renderer (3c wires the panel's retry).

- [ ] **Step 1: Write the failing tests**: `interpolate.rs`: `halfway_blends_positions`, `rotation_takes_the_shorter_arc`, `teleported_entity_is_not_blended`, `new_entity_appears_at_its_current_position`, `camera_is_blended`; `gpu.rs`: `previous_frame_and_alpha_draw_in_between`, `sixty_thousand_sprites_then_one_both_draw` (Review Focus, from VPR-2), `oversized_target_is_refused_naming_the_limit`, `a_validation_error_is_returned_by_the_next_draw`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --features gpu`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-render --features gpu`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(render): interpolate between ticks and report GPU errors as values`.

---

### Task 9: The renderer in the browser, pixel-tested on WebGL2 and WebGPU

**Files:**
- Create: `crates/ludenea-render/src/web.rs` (feature `web`), `editor/e2e/fixtures/renderer/` (a page loading the wasm renderer and drawing a reference frame), `editor/e2e/renderer.spec.ts`
- Modify: `xtask/src/main.rs` (`wasm-render <out-dir>` builds `ludenea-render` with `web`, next to `wasm-renderer` until 3c), `editor/e2e/playwright.config.ts` (projects `webgl2` and `webgpu`; the WebGPU project asserts an adapter exists), `.github/workflows/determinism.yml` (clippy of `ludenea-render --features web` for `wasm32-unknown-unknown`, the renderer spec in the browser job)
- Test: `editor/e2e/renderer.spec.ts`

**Interfaces:**
- Produces (`ludenea_render::web`, wasm-bindgen): `WebRenderer::create(canvas: HtmlCanvasElement) -> Promise<WebRenderer>`; `set_textures(&mut self, sources_json: &str, pngs: Vec<Uint8Array>) -> Result<String, JsValue>` (sources: `[{ id, path, meta }]` in the order of `pngs`; returns the `TextureError`s as JSON); `resize(&mut self, width: u32, height: u32, pixel_ratio: f32)`; `draw(&mut self, previous_json: Option<String>, current_json: &str, alpha: f32, overlay_json: &str) -> Result<String, JsValue>` (returns the `DrawReport` as JSON; errors carry the `RenderError` message).

- [ ] **Step 1: Write the failing tests**: `renderer.spec.ts`, run on both projects: `draws_the_reference_frame` (samples the background, a sprite texel, a flipped sprite, a tile, a parallax layer, a debug outline and a selected highlight), `sixty_thousand_sprites_then_one_both_draw`, `webgpu_project_has_a_webgpu_adapter`.
- [ ] **Step 2: Run** `pnpm -C editor exec playwright test e2e/renderer.spec.ts`. Expected: FAIL.
- [ ] **Step 3: Implement** `web.rs`, the xtask task, the fixture page and the projects.
- [ ] **Step 4: Run** the spec on both projects. Expected: PASS.
- [ ] **Step 5: Commit** `✅ test(render): check the renderer's pixels on WebGL2 and WebGPU`.

---

### Task 10: `GetRenderFrame` in the control protocol

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, worker.rs}`, the protocol fingerprint, `CHANGELOG.md` (protocol 5: `GetDebugDraw` and `Reply::DebugDraw` replaced), generated TypeScript, `editor/packages/control-client`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx` (draws `frame.debug` with the old renderer until 3c), the editor tests that fake `get_debug_draw`
- Test: `crates/ludenea-control/tests/introspection.rs` (`debug_draw_and_pick_through_protocol` becomes `render_frame_and_pick_through_protocol`), editor viewport tests

**Interfaces:**
- Produces: `PROTOCOL_VERSION = 5`; `Command::GetRenderFrame { debug_layers: Vec<String> }` replies `Reply::RenderFrame { frame: RenderFrame }` for the current tick; `GetGhostDraw` is unchanged.

- [ ] **Step 1: Write the failing tests**: `render_frame_and_pick_through_protocol`, `render_frame_carries_sprites_of_a_render_plugin_game`, `an_old_client_is_refused_naming_both_versions`; in the editor, the viewport test draws `frame.debug`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test introspection`. Expected: compile failure.
- [ ] **Step 3: Implement**; regenerate the TypeScript and the fingerprint.
- [ ] **Step 4: Run** `cargo test --locked --workspace && pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `💥 feat(protocol): replace the debug draw request with the render frame`.

---

### Task 11: `render`: any tick to a PNG from the command line

**Files:**
- Modify: `crates/ludenea-cli/{Cargo.toml, src/lib.rs, src/commands.rs, src/assets.rs}` (`ludenea-render` with `gpu` on non-wasm targets), README (the command), `CHANGELOG.md`
- Test: `crates/ludenea-cli/tests/render.rs`, a fixture project under `crates/ludenea-cli/tests/fixtures/render/` (a scene with a camera, a sprite sheet, a tilemap; generated PNGs)

**Interfaces:**
- Consumes: Tasks 5, 6, 8; `RunFiles`, `project_assets`.
- Produces: `Cmd::Render(commands::RenderArgs)` (not on wasm): `RenderArgs { files: RunFiles, inputs: Option<PathBuf>, tick: u64, out: PathBuf, width: u32 (default 1280), height: u32 (default 720), debug_layers: Vec<String> }`. Runs to `tick`, extracts the frames of `tick - 1` and `tick`, draws with `alpha = 1`, writes the PNG. Textures: every `.png` under the project folder with a texture meta, packed with `atlas::pack`; texture errors are printed as warnings and do not stop the render. A `.replay` given as the scene renders its own run. Exit codes as `run`.
- Produces (`ludenea_cli::assets`): `pub fn project_textures(scene: &Path, project: Option<&Path>) -> Result<Vec<(AssetId, String, Vec<u8>, TextureMeta)>, String>` (sorted by identifier).

- [ ] **Step 1: Write the failing tests**: `renders_a_tick_to_a_png` (decodes the output, samples a sprite and a tile), `render_of_a_replay_matches_render_of_its_scene_and_inputs`, `broken_texture_warns_and_still_renders`, `tick_past_a_replay_end_is_refused`, `render_does_not_change_the_run_hashes`.
- [ ] **Step 2: Run** `cargo test -p ludenea-cli --test render`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS, the golden tests included.
- [ ] **Step 5: Commit** `✨ feat(cli): render any tick of a scene or replay to a PNG`.
