# Ludenea Milestone 3c: Editor Rendering and the Arena Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The editor draws the game with `ludenea-render` (sprites, tilemaps, layers, the debug draw on top), interpolated from frames the host pushes during play; it previews textures, plays animation clips, paints tilemaps as recorded, undoable edits, picks up changed textures without a restart (as `play` does), recovers from a lost GPU with a retry; and an example arena with a LuizMelo fighter on a Gothicvania tilemap painted in the editor runs to the same hashes headless, in the native window, on the web page and in the editor.

**Architecture:** The editor loads the browser renderer (`WebRenderer`, built by `cargo xtask wasm-render` into a new package `@ludenea/render-wasm`) and deletes the old `ViewportRenderer` of `ludenea-draw`, so one renderer serves every runtime. A new subscription topic makes the host push the render frames of the ticks it runs; paused or seeking, the viewport asks for the tick on screen. What only the renderer knows (sprite sizes from texture metas, the grid of a texture) is answered by the renderer: picking and framing sprites, and the frames of a texture for previews and the tile palette. Tilemap painting is a new control command, `PaintTiles`, recorded as the same edit a `set_field` of the tiles would be, and driven in the viewport through a new public editor API, viewport tools. Texture hot reload never touches the session: the editor uploads the changed textures, and the native window repacks its atlas, from a file watcher.

**Tech Stack:** unchanged (Rust 1.97.1, wgpu `=30.0.1`, png, winit, gilrs, wasm-bindgen `=0.2.129`, notify `8.2.0` and notify-debouncer-full `0.7.0` from the workspace; TypeScript, React, Vitest, Playwright). No new dependency.

**Spec:** `.claude/superpowers/specs/2026-10-20-ludenea-rendering-3-design.md` (parent: `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md` section 14). Milestones 3a (`.claude/superpowers/plans/2026-10-20-ludenea-milestone-3a-render-core.md`) and 3b (`.claude/superpowers/plans/2026-10-21-ludenea-milestone-3b-game-runtime.md`) shipped everything else. This plan covers what 3b left: spec section 6.3 (hot reload, in the editor and in `play`), section 8's pushed frames, section 9 (viewport, previews, tilemap painting), the editor half of VPR-2 (the panel's failure alert, retry and device-lost recreation), sections 11 and 12 (the art and the example scene), and section 10's "same hashes everywhere" for that scene.

## Global Constraints

- Every Global Constraint of the milestone 1, 2a, 2b, 3a, 3b and quality-bar plans still applies (determinism lints for simulation code, target gates, committed generated files, `--locked`, commit style).
- The state hashes of `golden/` never move. `examples/boxes` and `examples/swarm` do not change their systems, scenes or input mappings.
- No release happened since protocol 5: protocol changes stay in `PROTOCOL_VERSION = 5`, update `PROTOCOL_FINGERPRINT`, regenerate the TypeScript, and extend the existing "Control protocol 5" CHANGELOG entry. Project protocol, input log, replay, edit log (`EDIT_LOG_VERSION = 3`) and undo (`UNDO_VERSION = 4`) formats do not change.
- Hot reload of a texture never sends a command that changes the session: no `replace_scene`, no edit, no hash change. A changed animation clip (a `SpriteSheet` data asset) keeps reloading as every data asset does (milestone 2b, section 5.4).
- One renderer: after Task 4 nothing in the editor or the workspace uses `ludenea_draw::render`, its `render` feature, `cargo xtask wasm-renderer` or `@ludenea/debug-draw-wasm`; all are deleted, not deprecated.
- No privileged editor API: tilemap painting, texture previews and clip playback are built-in plugins written only against `definePlugin` and the editor API a project plugin gets.
- Art: only the CC0 packs named in spec section 11, downloaded by the author, never anything generated; each committed file is listed in `examples/arena/CREDITS.md` with its artist, pack, licence and source URL. The Gothicvania music is never committed.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p ludenea-runtime --features native --all-targets -- -D warnings`, `cargo xtask check`, `cargo test --locked --workspace`, `cargo test --locked -p ludenea-render --features gpu`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- **Pushed frames come by subscription.** `Topic::RenderFrames { debug_layers }` makes `Host::advance` push `Event::RenderFrames { frames, progress, rate }`: the frames of the last two ticks the advance ran (one when it ran one), oldest first, so `WebRenderer::push_frame` blends consecutive ticks; `rate` is the effective tick rate, and the page extrapolates `alpha = progress + elapsed * rate` as the standalone page does. A client that does not subscribe pays nothing.
- **Sprites and tilemaps are picked and framed by the renderer.** A sprite's size comes from its texture meta, which the simulation never sees (spec section 6.1), so the host's `Pick` keeps picking colliders and `WebRenderer::pick` and `WebRenderer::bounds` answer for what it drew; the viewport merges both.
- **`PaintTiles` is a control command** (`{ entity, area, tile }`), not a client-side computation sent as `set_field`: the command line and any tool fill a rectangle the same way the editor does. It records exactly the edit a `set_field` of `Tilemap::tiles` records, so the edit log and undo do not change.
- **The arena fighter is a kinematic body** moved by a small controller (gravity, run, jump, raycasts against the static colliders) in the example's code: physics has no rotation lock, and fighter physics belongs to the fighter project (spec section 2).
- **Only the art the arena draws is committed:** the Fantasy Warrior and Martial Hero sheets for idle, run, jump and fall, and the Gothicvania Town tileset and its two background layers. The other packs of section 11 come with the fighter project that uses them.
- **The standalone web page does not hot reload:** it is a static build (spec section 7.2) with no project folder to watch.

## Review Focus

- **A PNG saved half-written, or a meta edited into an invalid one, while the editor or `play` is open:** the previous texture stays drawn, one warning names the file and the cause, and the next valid save draws the new one; the hash never changes. Tests in Tasks 6 and 7.
- **The GPU lost mid-play in the editor (driver reset, laptop switching GPUs):** the panel recreates the renderer once, uploads the textures again and redraws the tick on screen; a second loss, or failures on three draws in a row, shows "The viewport stopped rendering: <reason>" with Retry, while the session keeps playing. Tests in Task 5.
- **Painting outside the grid, on an entity that lost its tilemap (undo, another branch), or past tick 0 while playing:** `PaintTiles` is refused naming the entity, the area and the grid; past tick 0 the stroke is a recorded edit at the tick on screen and undoes in one step. Tests in Tasks 9 and 11.
- **A scene whose textures are missing on disk opened in the editor:** the viewport draws the 3a placeholders, the console lists each missing texture once (not every frame), and picking still selects the entity. Tests in Tasks 3 and 4.
- **The editor playing at a speed other than 1, or stepping several ticks per animation frame:** alpha stays within 0 to 1, frames after a gap draw without blending, and a seek while frames are in flight never draws a stale tick over the one on screen. Tests in Task 3.

---

### Task 1: Render frames pushed during play

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, worker.rs, in_process.rs}`, the protocol fingerprint, generated TypeScript (`editor/packages/control-client`), `CHANGELOG.md` (the protocol 5 entry)
- Test: `crates/ludenea-control/tests/render_frames.rs`

**Interfaces:**
- Consumes: `Simulation::render_frame`, `Host::tick_progress`, the existing subscription machinery.
- Produces:
  - `Topic::RenderFrames { debug_layers: Vec<String> }` (one subscription per host; subscribing again replaces the layers).
  - `Event::RenderFrames { frames: Vec<RenderFrame>, progress: f64, rate: f32 }`, pushed by `Host::advance` when it ran at least one tick while subscribed: at most the last two ticks run, oldest first. No event when no tick ran.
  - The worker transport encodes this event's frames with the same JSON fast path as `render_frame_json`.

- [ ] **Step 1: Write the failing tests**: `subscribed_advance_pushes_the_tick_it_ran`, `advance_of_three_ticks_pushes_the_last_two_oldest_first`, `no_tick_no_frames`, `unsubscribed_host_pushes_nothing`, `pushed_frames_hold_only_the_subscribed_debug_layers`, `progress_and_rate_match_the_pacer` (`progress == host.tick_progress()`, `rate` equals `Ticked::effective_rate`), `pushed_frames_never_change_the_hash` (same hashes at tick 300 with and without the subscription).
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test render_frames`. Expected: compile failure.
- [ ] **Step 3: Implement**; regenerate TypeScript and the fingerprint.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-control --test typescript && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(protocol): push the render frames of the ticks a subscribed host runs`.

---

### Task 2: What the browser renderer answers for the editor

**Files:**
- Modify: `crates/ludenea-render/src/{web.rs, atlas.rs}`, `crates/ludenea-render/src/gpu/{mod.rs, sprites.rs, tilemaps.rs}`, `xtask/src/main.rs` (`wasm-render` also writes a `package.json` named `@ludenea/render-wasm`), `CHANGELOG.md` (Rust API)
- Create: `editor/packages/render-wasm/.gitkeep`, `.gitignore` entry for its generated files (as `debug-draw-wasm` is today), `editor/package.json` script `build:render` (`cargo xtask wasm-render packages/render-wasm`)
- Test: `crates/ludenea-render/tests/gpu.rs`, `crates/ludenea-render/tests/frames.rs`, `editor/e2e/renderer.spec.ts`

**Interfaces:**
- Consumes: 3a/3b `Renderer`, `Atlas`, `TextureMeta`, `DebugDrawFrame::ghost`.
- Produces (`ludenea_render`, GPU-free):
  - `atlas::frames_of(png: &[u8], meta: &TextureMeta) -> Result<TextureFrames, TextureError>`; `TextureFrames { width: u32, height: u32, frames: Vec<FrameRect> }`, `FrameRect { x, y, width, height, pivot: (u32, u32) }`: the exact rectangles `pack` cuts, in image pixels, from the same code path (`pack` calls it).
- Produces (`gpu::Renderer`):
  - `pick(&self, point: Vec2) -> Vec<Entity>`: entities whose drawn sprite quad (after flip, pivot and pixels per unit) or tilemap non-empty cell contains the world point in the current frame, topmost first (layer, then order, then entity).
  - `bounds(&self, entities: &BTreeSet<Entity>) -> Option<(Vec2, Vec2)>`: world bounding box of their drawn sprites and tilemaps.
- Produces (`WebRenderer`, wasm-bindgen):
  - `pick(&self, x: f32, y: f32) -> String` (JSON `Entity[]`), `bounds(&self, entities_json: &str) -> Result<Option<String>, JsValue>` (JSON `{ min, max }`).
  - Errors thrown as a JS `Error` whose `name` is the `RenderError` variant (`NoAdapter`, `Gpu`, `Limit`, `DeviceLost`, `InvalidAtlas`) and whose `message` is its text.
  - Free functions `textureFrames(png: Uint8Array, meta_json: &str) -> Result<String, JsValue>` (JSON `TextureFrames`, or the `TextureError` message) and `ghostFrame(frame_json: &str) -> Result<String, JsValue>` (moved from `ludenea_draw::render::ghost_frame`).

- [ ] **Step 1: Write the failing tests**: `frames.rs`: `grid_frames_match_what_pack_cuts`, `single_image_is_one_frame_with_the_meta_pivot`, `frames_of_a_broken_png_name_the_cause`; `gpu.rs`: `pick_hits_a_sprite_by_its_drawn_quad`, `pick_respects_flip_and_pivot`, `pick_returns_the_topmost_first`, `pick_hits_a_painted_tile_and_misses_an_empty_cell`, `missing_texture_placeholder_is_pickable` (Review Focus), `bounds_cover_sprites_and_tilemaps`; `renderer.spec.ts`: `device_lost_error_is_named_DeviceLost` (webgpu project), `texture_frames_from_the_page`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --features gpu`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-render --features gpu && cargo test --locked --workspace && cargo xtask wasm-render editor/e2e/fixtures/renderer/pkg && pnpm -C editor e2e renderer.spec.ts`. Expected: PASS on webgl2 and webgpu.
- [ ] **Step 5: Commit** `✨ feat(render): pick, frame and list the frames of what the renderer draws`.

---

### Task 3: The viewport draws with `ludenea-render`

**Files:**
- Create: `editor/packages/editor-builtins/src/viewport/{textures.ts, frames.ts}`
- Modify: `editor/packages/editor-builtins/src/viewport/{ViewportPanel.tsx, state.ts, camera.ts, index.tsx}`, `editor/packages/editor-builtins/test/fake-renderer.ts` (fakes `WebRenderer`), `editor/apps/{web,desktop}/vite.config.ts` and `editor/vitest.config.ts` aliases, `README` (Editor: viewport)
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`

**Interfaces:**
- Consumes: Tasks 1 and 2; the project protocol's `list_files` and `read_binary`.
- Produces:
  - `textures.ts`: `loadTextures(project, files): Promise<{ sources: TextureSourceJson[]; pngs: Uint8Array[] }>` reading every `Texture` asset of the project with its meta; `ViewportPanel` passes them to `set_textures` when the renderer is created and when the open document changes, and logs each refused texture once in the editor console (`<path>: <message>`).
  - `frames.ts`: `FrameFeed`: `subscribe()` sends `subscribe { topic: render_frames }` with the visible debug layers while `timeline.playing`, `unsubscribe` when paused; on `render_frames` it calls `push_frame` for each frame in order and records `{ progress, rate, arrival }`; `alpha(now): number` returns `progress + (now - arrival) / 1000 * rate` clamped to 0..1 (0 when paused). Paused, after a seek, an edit or a layer change, it requests `get_render_frame` for the tick on screen and pushes it; a reply or event for a tick other than the one on screen is dropped.
  - `camera.ts`: `toCameraView(camera, size): CameraView` (`view = size / pixelsPerMeter`); `ViewportState.gameCamera: boolean`, command `viewport.toggle-game-camera` ("View through the game camera", shortcut `C`), which draws with `camera: null`.
  - The animation frame loop calls `draw(alpha, camera)` every frame and nothing else; `resize` on size or pixel ratio change. `frame.debug` still feeds labels and the drag outline (`overlay.ts` unchanged).

- [ ] **Step 1: Write the failing tests** (`viewport.test.tsx`; port the existing tests to the fake `WebRenderer`): `textures_are_uploaded_when_the_renderer_is_created`, `a_refused_texture_is_logged_once` (Review Focus), `playing_subscribes_and_pushes_each_frame_in_order`, `pausing_unsubscribes_and_draws_the_tick_on_screen`, `alpha_is_extrapolated_and_clamped_at_any_speed` (Review Focus, speeds 0.25 and 4), `a_frame_for_another_tick_after_a_seek_is_dropped` (Review Focus), `game_camera_toggle_draws_without_a_camera`, `drawing_between_frames_sends_no_request`.
- [ ] **Step 2: Run** `pnpm -C editor test viewport`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor build:render && pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor e2e smoke.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): draw the viewport with the game renderer from pushed frames`.

---

### Task 4: Picking, framing and the ghost on the new renderer; the old renderer deleted

**Files:**
- Modify: `editor/packages/editor-builtins/src/viewport/{ViewportPanel.tsx, camera.ts, index.tsx}`, `editor/packages/editor-builtins/src/divergence/index.tsx`, `editor/package.json`, `xtask/src/main.rs` (drop `wasm-renderer` and its clippy line), `README`, `CHANGELOG.md` (Rust API, breaking: `ludenea_draw::render` removed)
- Delete: `crates/ludenea-draw/src/render.rs`, the `render` feature of `crates/ludenea-draw/Cargo.toml`, `editor/packages/debug-draw-wasm/`
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/e2e/divergence.spec.ts`

**Interfaces:**
- Consumes: Task 2 (`pick`, `bounds`, `ghostFrame`), Task 3.
- Produces: a click picks with both the host's `pick` (colliders) and `renderer.pick` (sprites, tilemaps), union in that order without duplicates; `viewport.frame-selection` fits the union of `renderer.bounds` and `bounds(frame.debug)`; the divergence ghost uses `ghostFrame` from `@ludenea/render-wasm`.

- [ ] **Step 1: Write the failing tests**: `click_on_a_sprite_without_a_collider_selects_it`, `click_merges_collider_and_sprite_hits_without_duplicates`, `frame_selection_fits_a_sprite_only_entity`, `missing_texture_entity_is_still_selectable` (Review Focus); the existing divergence e2e keeps passing on the moved `ghostFrame`.
- [ ] **Step 2: Run** `pnpm -C editor test viewport`. Expected: FAIL.
- [ ] **Step 3: Implement**, then delete the old renderer and every reference (`rg -n "ViewportRenderer|ghost_frame|debug-draw-wasm|wasm-renderer"` returns nothing outside the CHANGELOG).
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo xtask check && pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor e2e divergence.spec.ts smoke.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `♻️ refactor(editor-builtins): pick and frame with the game renderer and delete the old viewport renderer`.

---

### Task 5: Viewport failures, retry and a lost device

**Files:**
- Modify: `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx`, `editor/packages/editor-builtins/test/fake-renderer.ts` (scripted failures and `DeviceLost`), `README` (Editor: viewport)
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/e2e/renderer-webgpu.spec.ts`

**Interfaces:**
- Consumes: Task 2's named errors, Task 3.
- Produces (in `ViewportPanel`): a `DeviceLost` error frees the renderer, replaces the `<canvas>` element (a lost WebGPU context cannot be reconfigured), creates a new renderer, uploads the textures and pushes the last frame, once per 60 seconds; a second loss within that time, a failing creation, or three failing draws in a row stop the loop and render `<div role="alert">The viewport stopped rendering: {message}</div>` with a `Retry` button, which recreates the renderer and resumes. The session keeps playing throughout. The plain `<p>` creation message is replaced by the same alert.

- [ ] **Step 1: Write the failing tests**: `a_draw_that_keeps_failing_offers_a_retry` (the VPR-2 test), `retry_recreates_the_renderer_and_draws_again`, `a_lost_device_recreates_the_renderer_once_with_textures_and_frame` (Review Focus), `a_second_loss_within_a_minute_shows_the_alert`, `a_failed_creation_shows_the_alert_with_retry`, `the_session_keeps_playing_while_the_viewport_is_stopped`; `renderer-webgpu.spec.ts`: `editor_viewport_recovers_from_a_destroyed_device` (calls `device.destroy()` through the fixture's test hook).
- [ ] **Step 2: Run** `pnpm -C editor test viewport`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor e2e renderer-webgpu.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `🐛 fix(editor-builtins): recover the viewport from a lost GPU and offer a retry`.

---

### Task 6: Hot reload of textures in the editor

**Files:**
- Modify: `editor/packages/editor-shell/src/stores/document.ts` (route `Texture` changes away from `reloadAssets`), `editor/packages/editor-builtins/src/viewport/{textures.ts, ViewportPanel.tsx}`, `README` (Editor: textures)
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/packages/editor-shell/test/document.test.ts`

**Interfaces:**
- Consumes: `files_changed` (`Texture` kind for a PNG, the PNG's change for its `.meta`), Task 3's `loadTextures`.
- Produces: `textures.ts` `reloadChanged(changes): Promise<TextureReload>`: re-reads the changed and created textures and drops removed ones, then calls `set_textures` with the full set (the renderer packs again) and pushes the frame on screen again; a texture that fails keeps its previous bytes in the set and logs one warning naming the file and cause. No control command is sent.

- [ ] **Step 1: Write the failing tests**: `a_changed_png_is_uploaded_again_and_redrawn`, `a_changed_meta_reloads_its_texture`, `a_removed_png_drops_its_texture`, `a_broken_png_keeps_the_previous_texture_and_warns_once` (Review Focus), `texture_reload_sends_no_control_command` (asserts the fake control saw only `get_render_frame`), `data_asset_changes_still_reload_the_session`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): draw a changed texture without reloading the session`.

---

### Task 7: Hot reload of textures in `play`

**Files:**
- Create: `crates/ludenea-runtime/src/native/watch.rs`
- Modify: `crates/ludenea-runtime/{Cargo.toml (notify, notify-debouncer-full under native), src/native/{mod.rs, window.rs, surface.rs}}`, `crates/ludenea-cli/src/commands.rs` (`PlayArgs::no_watch`), `README` (play), `CHANGELOG.md`
- Test: inline tests in `watch.rs`, `crates/ludenea-runtime/tests/surface.rs`, `crates/ludenea-cli/tests/cli.rs`

**Interfaces:**
- Consumes: `ludenea_app::project::project_textures`, `atlas::pack`, `Display` and `Presenter` from 3b.
- Produces (`ludenea_runtime::native::watch`):
  - `pub struct TextureWatch`; `TextureWatch::start(scene: &Path, project: Option<&Path>) -> Result<TextureWatch, RuntimeError>` watching the folders `project_textures` scans; `poll(&mut self) -> Option<ProjectTextures>`: after a debounced change to a `.png` or `.png.meta`, the textures read again (`None` when nothing changed).
  - `Display::replace_textures(&mut self, textures: &[ProjectTexture]) -> Vec<String>`: packs and calls `set_atlas`; a texture that fails keeps its previous entry, and the returned warnings name each failure.
  - `PlayOptions.watch: Option<(PathBuf, Option<PathBuf>)>` (scene and project; `None` with `--no-watch`); the window polls it once per frame and writes warnings to `out`.

- [ ] **Step 1: Write the failing tests**: `watch.rs`: `a_written_png_is_reported_once_after_the_debounce`, `an_unrelated_file_is_ignored`; `surface.rs`: `replaced_texture_changes_the_drawn_pixels` (offscreen display, sample a pixel before and after), `broken_replacement_keeps_the_previous_texture` (Review Focus); `cli.rs`: `play_help_lists_no_watch`.
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --features native`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-runtime --features native && cargo clippy -p ludenea-runtime --features native --all-targets -- -D warnings && cargo test --locked --workspace`, then by hand: `cargo run --locked -p swarm -- play examples/swarm/assets/swarm.scene.ron --players 1`, edit `bug.png` in an image editor and save. Expected: PASS, and the bugs change without a restart.
- [ ] **Step 5: Commit** `✨ feat(runtime): repack changed textures while a game plays`.

---

### Task 8: Texture previews and animation clips in the inspector

**Files:**
- Create: `editor/packages/editor-builtins/src/textures/{index.tsx, TexturePreview.tsx, ClipPlayer.tsx}`
- Modify: `editor/packages/editor-builtins/src/{index.ts, assets/index.tsx, assets/AssetsPanel.tsx, inspector/InspectorPanel.tsx}` (a texture asset is selectable; an asset type with a registered inspector uses it, as components do), `editor/packages/editor-shell/src/plugin.ts` (`inspectors` documented as keyed by component or asset type name), `README` (Editor: textures and clips)
- Test: `editor/packages/editor-builtins/test/textures.test.tsx`, `editor/packages/editor-builtins/test/assets.test.tsx`

**Interfaces:**
- Consumes: Task 2's `textureFrames`, the project protocol's `read_binary`, `editor.inspectors`.
- Produces:
  - Selecting a texture in the asset browser shows `TexturePreview` in the inspector: the image at an integer zoom with nearest filtering, each frame outlined and numbered, the pivot marked, the size, frame count and pixels per unit; a refused texture shows the `TextureError` message as `role="alert"`.
  - An inspector for `ludenea::SpriteSheet`: the generic fields, then a `ClipPlayer` per clip: play and pause, a frame index, playing at the session's tick rate times `ticks_per_frame` from its texture's frames; a clip whose range exceeds the texture's frame count says so and does not play.
  - Both are a built-in plugin (`texturesPlugin`) registered through `definePlugin` only.

- [ ] **Step 1: Write the failing tests**: `selecting_a_texture_previews_its_frames`, `preview_marks_the_pivot`, `broken_texture_preview_shows_the_error`, `clip_player_advances_one_frame_every_ticks_per_frame`, `clip_past_the_texture_frames_is_reported`, `asset_inspector_registered_by_type_name_replaces_the_generic_one`.
- [ ] **Step 2: Run** `pnpm -C editor test textures assets`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): preview textures and play animation clips in the inspector`.

---

### Task 9: `PaintTiles` in the control protocol

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs}`, the fingerprint, generated TypeScript, `CHANGELOG.md` (the protocol 5 entry)
- Test: `crates/ludenea-control/tests/paint_tiles.rs`

**Interfaces:**
- Consumes: `ludenea_render::Tilemap` (read through the world's component JSON, so the host does not depend on `ludenea-render`), the `SetField` edit path.
- Produces:
  - `Command::PaintTiles { entity: Entity, area: TileArea, tile: Option<u32> }`, replies `Done`; `TileArea { x: u32, y: u32, width: u32, height: u32 }` (cells, origin at the grid's first cell, row-major as `Tilemap::tiles`). `None` erases.
  - Applied and recorded exactly as `SetField { target: Component { entity, component: "ludenea::Tilemap" }, path: [tiles], value }` with the filled array: at tick 0 one scene edit, later one timed edit, one undo step either way. Allowed in a `Batch`.
  - Refused with `InvalidCommand` (the existing edit context names `paint_tiles`): no `ludenea::Tilemap` on the entity ("entity 4v1 has no ludenea::Tilemap"), an empty area, an area outside the grid ("area 30..34 x 0..2 is outside the 32 by 18 grid").

- [ ] **Step 1: Write the failing tests**: `paint_fills_the_rectangle_at_tick_0_as_one_scene_edit`, `erase_clears_the_rectangle`, `paint_undoes_in_one_step`, `paint_past_tick_0_records_a_timed_edit_that_replays` (Review Focus), `paint_records_the_same_edit_as_set_field` (edit logs equal), `area_outside_the_grid_is_refused_naming_both` (Review Focus), `entity_without_a_tilemap_is_refused` (Review Focus), `empty_area_is_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test paint_tiles`. Expected: compile failure.
- [ ] **Step 3: Implement**; regenerate TypeScript and the fingerprint.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-control --test typescript && pnpm -C editor typecheck`. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `✨ feat(protocol): fill and erase tilemap rectangles as recorded edits`.

---

### Task 10: Viewport tools in the editor API

**Files:**
- Modify: `editor/packages/editor-shell/src/{plugin.ts, editor.ts}`, `editor/packages/editor-builtins/src/viewport/{ViewportPanel.tsx, state.ts, index.tsx}`, `README` (Editor: plugins)
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/packages/editor-shell/test/plugins.test.ts`

**Interfaces:**
- Produces (`plugin.ts`):
  - `interface ViewportTool { id: string; title: string; cursor?: string; available(editor: Editor): boolean; pointerDown?(event: ToolPointer, context: ToolContext): void; pointerMove?(event: ToolPointer, context: ToolContext): void; pointerUp?(event: ToolPointer, context: ToolContext): void; overlay?(context: OverlayContext): void }`
  - `interface ToolPointer { world: Vec2; button: number; shift: boolean; alt: boolean }`, `interface ToolContext { editor: Editor; screenToWorld(point: { x: number; y: number }): Vec2; redraw(): void }`
  - `definePlugin({ viewportTools: ViewportTool[] })`; `editor.viewport.activeTool` (`string | null`, `null` is the built-in select and move) and `editor.viewport.setTool(id | null)`; `Escape` returns to `null`; a tool whose `available` turns false is deactivated. Pan (Space-drag, middle-drag) and zoom stay with the panel whatever the tool.

- [ ] **Step 1: Write the failing tests**: `an_active_tool_receives_pointer_events_in_world_units`, `escape_returns_to_select`, `a_tool_that_becomes_unavailable_is_deactivated`, `pan_and_zoom_work_with_a_tool_active`, `a_project_plugin_registers_a_viewport_tool` (`plugins.test.ts`).
- [ ] **Step 2: Run** `pnpm -C editor test viewport plugins`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-shell): let plugins add viewport tools`.

---

### Task 11: Tilemap painting

**Files:**
- Create: `editor/packages/editor-builtins/src/tilemap/{index.tsx, TilePalette.tsx, paint.ts}`, `editor/e2e/tilemap.spec.ts`
- Modify: `editor/packages/editor-builtins/src/index.ts`, `README` (Editor: tilemaps)
- Test: `editor/packages/editor-builtins/test/tilemap.test.tsx`, `editor/e2e/tilemap.spec.ts`

**Interfaces:**
- Consumes: Tasks 2 (`textureFrames`), 8 (texture bytes), 9, 10.
- Produces (`tilemapPlugin`):
  - Panel `ludenea.tile-palette`: the selected tilemap's tileset frames as a grid of buttons; the chosen tile is plugin state.
  - Tools `tilemap.paint` and `tilemap.erase` (shortcuts `B` and `E`), available while exactly one entity with `ludenea::Tilemap` is selected and the timeline is not playing; a drag selects a rectangle of cells (snapped to the grid from the entity's `Pose` and `cell_size`), previewed in the overlay, sent on release as one `paint_tiles` through the editor's `edit()` path (so it is undoable and refreshes the timeline); a right drag with `paint` erases.
  - `paint.ts`: `cellAt(world: Vec2, pose: Pose, tilemap: Tilemap): { x: number; y: number } | null`, `areaBetween(a, b): TileArea` clamped to the grid.

- [ ] **Step 1: Write the failing tests**: `tilemap.test.tsx`: `cell_at_maps_world_points_to_cells`, `area_between_is_clamped_to_the_grid`, `dragging_sends_one_paint_tiles_on_release`, `right_drag_erases`, `tools_are_unavailable_while_playing`, `a_refused_paint_is_shown_in_the_console` (Review Focus); `tilemap.spec.ts`: `paint_a_rectangle_undo_and_save` (opens a project with a tilemap, paints, sees the tiles drawn, undoes, paints again, saves, reopens and sees them).
- [ ] **Step 2: Run** `pnpm -C editor test tilemap`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor e2e tilemap.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): paint and erase tilemaps in the viewport`.

---

### Task 12: The arena's art

**Files:**
- Create: `examples/arena/{Cargo.toml, src/lib.rs}` (the crate, with `RenderPlugin` only; Task 13 completes it), workspace member, `examples/arena/assets/art/{warrior,hero}/{idle,run,jump,fall}.png` and their `.meta`, `examples/arena/assets/art/town/{tileset,background,middleground}.png` and their `.meta`, `examples/arena/assets/{warrior,hero}.sheet.ron` and their `.meta`, `examples/arena/CREDITS.md`
- Test: `examples/arena/tests/art.rs`

**Interfaces:**
- Produces: texture metas with `pixels_per_unit` chosen so a fighter stands about 2 world units tall and a tile is 1 unit, `filter: Nearest`, the grid measured from each sheet (frame width = image width / frame count), and a pivot at the fighter's feet measured on the idle sheet; sheets `warrior.sheet.ron` and `hero.sheet.ron` with clips `idle`, `run`, `jump`, `fall` (frame counts from the images; `ticks_per_frame` 5 for idle and run, 4 for jump and fall). `CREDITS.md`: one entry per pack (artist, pack, CC0, URL, the files taken, the SHA-256 of each downloaded archive).

- [ ] **Step 1: Author step**: download Fantasy Warrior and Martial Hero from https://luizmelo.itch.io and Gothicvania Town from https://opengameart.org/content/gothicvania-town, and leave the three archives in a folder outside the repository. Wait for the author to give its path.
- [ ] **Step 2: Write the failing tests**: `every_texture_packs_without_errors` (`project_textures` then `atlas::pack`: no `TextureError`), `every_clip_fits_its_texture` (`frames_of` count ≥ `first + count`), `credits_list_every_committed_art_file`.
- [ ] **Step 3: Run** `cargo test -p arena --test art`. Expected: FAIL (no art yet).
- [ ] **Step 4: Import** the files named above, write the metas and sheets, check each preview in the editor's texture inspector (Task 8), and write `CREDITS.md`.
- [ ] **Step 5: Run** `cargo test --locked -p arena --test art && cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 6: Commit** `🍱 chore(arena): add the CC0 fighter and town art with credits`.

---

### Task 13: The arena game

**Files:**
- Create: `examples/arena/{ludenea.toml, README.md, src/lib.rs, src/main.rs, src/fighter.rs, src/camera.rs, src/bin/record-replay.rs, assets/arena.scene.ron, assets/arena.scene.ron.meta}`
- Modify: `examples/arena/{Cargo.toml, src/lib.rs}`, README (workspace layout)
- Test: `examples/arena/tests/arena.rs`

**Interfaces:**
- Consumes: Task 12's sheets, `RenderPlugin`, `PhysicsPlugin` (`raycast`), `inputs.player(i)`.
- Produces: a project (`name = "arena"`, `default_scene = "assets/arena.scene.ron"`, `[features] web = ["ludenea/web"]`, `ludenea::worker_entry!(app)`), `pub fn app() -> App`:
  - Layers: `sky` (parallax 0.1), `town` (0.4), `tiles` (1.0), `fighters` (1.0).
  - `ArenaInput { run: f32, jump: bool }`; `map_raw_input(raw, _player)`: arrows or A and D, gamepad 0's left stick and d-pad; jump on `Space`, `KeyW`, `ArrowUp` or gamepad button 0.
  - `Fighter { player: u8, velocity: Vec2, grounded: bool }` on a kinematic `Body` with a capsule `Collider`: constants `RUN_SPEED = 0.12` units per tick, `JUMP_SPEED = 0.35`, `GRAVITY = 0.02` per tick², `MAX_FALL = 0.5`; `grounded` from a downward `raycast` against the other colliders; horizontal moves stop at walls by a sideways raycast. A fighter whose player is absent is despawned on the first tick.
  - `animate_fighters`: clip `idle`, `run`, `jump` (rising) or `fall` (falling, not grounded), switching `SpriteAnimation::clip` and `started` only when the clip changes; `Sprite::flip_x` follows the last non-zero run direction.
  - `follow_camera`: the `Camera2d` entity's `Pose` centres on the present fighters' midpoint and its `view` is their bounding box plus 6 units, at least 24 by 13.5.
  - The scene: the camera, a fighter per player (warrior for player 0, hero for player 1), a `Tilemap` entity on `tiles` using the town tileset (32 by 18 cells of 1 unit, all empty: painted in Task 14), sprites on `sky` and `town`, and no colliders yet (Task 14).
  - `record-replay <out>`: plays 600 ticks of scripted input for two players and writes the replay.

- [ ] **Step 1: Write the failing tests**: `fighter_runs_and_turns_its_sprite`, `fighter_lands_on_a_collider_and_idles`, `jump_then_fall_clips_follow_velocity`, `absent_player_fighter_is_despawned`, `camera_frames_both_fighters_and_keeps_its_minimum_view`, `same_hashes_at_one_and_eight_threads` (600 ticks, two players, a test scene with a floor collider).
- [ ] **Step 2: Run** `cargo test -p arena`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p arena && cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(examples): add an arena where local players run and jump as LuizMelo fighters`.

---

### Task 14: The arena level, built in the editor, and the same hashes everywhere

**Files:**
- Modify: `examples/arena/assets/arena.scene.ron` (saved by the editor), `examples/arena/README.md` (how the level was made)
- Create: `golden/arena.replay`, `editor/e2e/arena.spec.ts`
- Modify: `README` (golden files and the command to record `golden/arena.replay`), `.github/workflows/determinism.yml` (Linux: `xvfb-run` of `arena play --players 2 --ticks 600 --record arena-native.replay` with scripted absence of input, then `verify`; `arena render golden/arena.replay --tick 300 --out arena.png`; the editor job builds `ludenea build --web examples/arena --out editor/e2e/fixtures/arena`), `crates/ludenea/tests/golden.rs` (the arena replay verifies)
- Test: `examples/arena/tests/level.rs`, `editor/e2e/arena.spec.ts`, `editor/e2e/standalone.spec.ts`

**Interfaces:**
- Consumes: every task above.
- Produces: a level painted in the editor with the tilemap tools (ground and two platforms), the static box colliders placed by hand under them with the viewport, both fighters standing on the ground at tick 0, saved by the editor; `golden/arena.replay` recorded with `cargo run --locked --release -p arena --bin record-replay -- golden/arena.replay`.

- [ ] **Step 1: Author step, in the editor** (`cargo run --locked -p ludenea-serve -- serve examples/arena`): paint the ground and two platforms with the tile palette, place a fixed box collider under each, check both fighters land, save the scene. The executor prepares the session and waits for the author to finish.
- [ ] **Step 2: Write the failing tests**: `level.rs`: `both_fighters_stand_on_the_ground_after_60_ticks`, `every_platform_tile_row_has_a_collider_under_it`, `render_of_tick_300_draws_a_fighter_over_the_tiles` (offscreen `render`, sample a fighter pixel and a tile pixel against `BACKGROUND`); `arena.spec.ts`: `editor_play_matches_headless_hashes` (one player, 300 ticks of no input: the timeline's hash equals `arena run <scene> --ticks 300 --hash`), `editor_two_player_replay_verifies_headless` (two players, scripted keys, `export_replay` then `arena verify` exits 0), `editor_draws_the_painted_level`; `standalone.spec.ts`: `arena_page_matches_headless_hashes` (`?players=1&ticks=300` against `arena run --hash`), `arena_page_two_player_replay_verifies` (`?players=2`, scripted keys, "Save replay" then `arena verify`).
- [ ] **Step 3: Run** `cargo test -p arena --test level`. Expected: FAIL until the level is saved.
- [ ] **Step 4: Record** `golden/arena.replay`; wire CI.
- [ ] **Step 5: Run** `cargo test --locked --workspace && cargo run --locked --release -p arena -- verify golden/arena.replay && pnpm -C editor e2e arena.spec.ts standalone.spec.ts`, then by hand on the reference machine: `cargo run --locked --release -p ludenea-serve -- play examples/arena` (two players join, run, jump, the camera follows, the parallax layers scroll), and edit `tileset.png` while it plays (hot reload). Expected: PASS and the definition of done of spec section 12, checked item by item in the commit message.
- [ ] **Step 6: Commit** `✨ feat(examples): build the arena level in the editor and verify it on every runtime`.
