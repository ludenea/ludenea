# Ludenea Milestone 3c: Editor Rendering and the Arena Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The editor draws the game with `ludenea-render` (sprites, tilemaps, layers, the debug draw on top), interpolated from frames the host pushes during play; it previews textures, plays animation clips, paints tilemaps as recorded, undoable edits, picks up changed textures without a restart (as `play` does), recovers from a lost GPU with a retry; and an example arena with LuizMelo fighters on a Gothicvania tilemap painted in the editor runs to the same hashes headless, in the native window, on the web page and in the editor, on every platform of the CI matrix.

**Architecture:** The editor loads the browser renderer (`WebRenderer`, built by `cargo xtask wasm-render` into `editor/packages/render-wasm`) and deletes the old `ViewportRenderer` of `ludenea-draw`, so one renderer serves every runtime. A new subscription topic makes the host push the render frames of the ticks it runs (as postcard bytes in transferred buffers through the Web Worker); paused or seeking, the viewport asks for the tick on screen. What only texture metas know (a sprite's drawn size, a texture's frames) is answered by GPU-free functions of `ludenea-render` that any tool can call, and exposed to the page by `WebRenderer`. Tilemap painting is a new control command, `PaintTiles`, recorded as the same edit a `set_field` of the tiles would be, driven in the viewport through a new public editor API, viewport tools. Texture hot reload never touches the session: the editor uploads the changed textures, and the native window repacks its atlas from a file watcher, keeping the last valid version of a texture that fails.

**Tech Stack:** unchanged (Rust 1.97.1, wgpu `=30.0.1`, png, postcard, winit, gilrs, wasm-bindgen `=0.2.129`, notify `8.2.0` and notify-debouncer-full `0.7.0` from the workspace; TypeScript, React, Zustand, Vitest, Playwright). No new dependency.

**Spec:** `.claude/superpowers/specs/2026-10-20-ludenea-rendering-3-design.md` (parent: `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md` section 14). Milestones 3a (`.claude/superpowers/plans/2026-10-20-ludenea-milestone-3a-render-core.md`) and 3b (`.claude/superpowers/plans/2026-10-21-ludenea-milestone-3b-game-runtime.md`) shipped everything else; their Deviations still bind. This plan covers what 3b left: spec section 6.3 (hot reload, in the editor and in `play`), section 8's pushed frames, section 9 (viewport, previews, tilemap painting), the editor half of VPR-2 (`.claude/superpowers/plans/2026-10-19-quality-bar-backlog.md`, VPR-2: the panic hook, the panel's failure alert, retry and device-lost recreation), sections 11 and 12 (the art and the example scene), and section 10's "same hashes everywhere" for that scene.

## Global Constraints

- Every Global Constraint of the milestone 1, 2a, 2b, 3a, 3b and quality-bar plans still applies (determinism lints for simulation code, target gates, committed generated files, `--locked`, commit style).
- The state hashes of `golden/` never move. `examples/boxes` and `examples/swarm` do not change their systems, scenes or input mappings.
- No release happened since protocol 5: protocol changes stay in `PROTOCOL_VERSION = 5`, update `PROTOCOL_FINGERPRINT`, regenerate the TypeScript, and extend the existing "Control protocol 5" CHANGELOG entry. Project protocol, input log, replay, edit log (`EDIT_LOG_VERSION = 3`), undo (`UNDO_VERSION = 4`) and sprite sheet formats do not change.
- Hot reload of a texture never sends a command that changes the session: no `replace_scene`, no edit, no hash change. A changed `SpriteSheet` data asset keeps reloading in the editor as every data asset does (milestone 2b, section 5.4).
- One renderer: after Task 4 nothing in the workspace, the editor or CI uses `ludenea_draw::render`, its `render` feature, `cargo xtask wasm-renderer`, `pnpm build:wasm` or `@ludenea/debug-draw-wasm`; all are deleted, not deprecated.
- No privileged editor API: tilemap painting, texture previews and clip playback are built-in plugins using only the API a project plugin gets (`editor.*` registries and stores, the modules in `editor/shared/plugin.ts` `SHARED_MODULES`). Whatever they need that is internal today (editing, texture frames) is made public in the task that needs it.
- Simulation code added here (the arena) follows `clippy.toml`: `f32::max`, `min`, `clamp` and friends go through `ludenea::math` (`max`, `min`, `clamp`); division and `sqrt` are allowed.
- Art: only the CC0 packs named in spec section 11, downloaded by the author, never anything generated; each committed file is listed in `examples/arena/CREDITS.md` with its artist, pack, licence and source URL. The Gothicvania music is never committed.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p ludenea-runtime --features native --all-targets -- -D warnings`, `cargo xtask check`, `cargo test --locked --workspace`, `cargo test --locked -p ludenea-render --features gpu`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- **One texture per sprite sheet (kept from 3a).** Spec 6.1 gives each clip a texture handle; 3a gave the sheet one (`SpriteSheet { texture, clips }`). The art is one PNG per animation, so the arena has one sheet per animation and switches `SpriteAnimation::sheet` with the clip. No format change.
- **Pushed frames come by subscription.** `Topic::RenderFrames` (no payload; the layers are set by `SetPushedLayers`) makes `Host::advance` push `Event::RenderFrames { frames, progress }` after the `Ticked` events of the same advance: the frames of the last two ticks it ran (one when it ran one), oldest first, so `push_frame` blends consecutive ticks. The rate comes from `Ticked::effective_rate`; the page extrapolates `alpha = progress + elapsed * rate` as the standalone page does. The Web Worker transport carries these frames as postcard bytes in transferred buffers (JSON would cost about 2 MB per tick at 10,000 sprites, the limit 3b recorded); the WebSocket transport keeps JSON, a known limit for the desktop editor stated in the README.
- **Sprites are picked and framed from the drawn frame.** A sprite's size comes from its texture meta, which the simulation never sees (spec 6.1). `ludenea_render::pick` (GPU-free: a `RenderFrame`, the atlas regions, a screen point, the view) picks sprites and tilemaps on any layer, parallax included; any tool with the textures calls it, `WebRenderer` exposes it, and the host's `Pick` keeps picking colliders. The editor merges both.
- **`PaintTiles` is a control command** (`{ entity, area, tile }`): the host fills the rectangle atomically on its own copy of the tiles, so a stroke never races play or other edits from a stale client copy, and the command line fills a rectangle the same way the editor does. It records exactly the edit a `set_field` of `Tilemap::tiles` records (scene edit or prefab override at tick 0, timed edit later), so the edit log and undo do not change; each stroke stores the whole tiles array, so the log grows with the map's size. The host names `ludenea::Tilemap` by its stable name, as it names `ludenea::Pose` for dragging.
- **The arena fighter is a kinematic body** whose `Pose` the example's controller writes (gravity, run, jump, raycasts against colliders): physics has no rotation lock, and fighter physics belongs to the fighter project (spec section 2). Fighters standing on each other is accepted.
- **Only the art the arena draws is committed** (builder's call, the author may widen it): Fantasy Warrior and Martial Hero (idle, run, jump, fall) and Gothicvania Town (tileset and two background layers). Evil Wizard 2 and Huntress come with the fighter project.
- **Hot reload in `play` covers textures only.** A changed `SpriteSheet` in `play` needs a restart: a reload there changes the simulation, and `play` records no edits. The editor reloads it as a recorded edit, as the spec says. The standalone web page, a static build (spec 7.2), does not hot reload.

## Review Focus

- **A PNG saved half-written, or a meta edited into an invalid one, while the editor or `play` is open:** the previous version of that texture stays drawn, one warning names the file and the cause, the next valid save draws the new one, and the hash never changes. Tests in Tasks 6 and 7.
- **The GPU lost mid-play in the editor (driver reset, laptop switching GPUs):** the panel recreates the renderer once, uploads the textures and pushes the last frame again; a second loss within a minute, a failing creation, or three failing draws in a row show "The viewport stopped rendering: <reason>" with Retry, while the session keeps playing. A hidden tab (no surface texture) is not a failure. Tests in Task 5.
- **Painting outside the grid, on an entity that lost its tilemap (undo, another branch), with a tile index past the tileset, or paused past tick 0:** out of grid (overflow included), no tilemap and a dead entity are refused naming the entity, the area and the grid; an unknown tile index is accepted and drawn as the placeholder with `DrawReport::missing`; paused past tick 0 the stroke is a timed edit that undoes in one step. Tests in Tasks 9 and 11.
- **Frames out of order:** a `get_render_frame` reply older than the newest frame on screen (sent before a seek, a layer change or a reload) is dropped, while playing and while paused; a selection change pushes the frame on screen again locally. Tests in Task 3.
- **Playing a heavy scene in the editor:** the swarm (10,000 sprites) plays in the browser editor with a 95th percentile frame time under 20 ms, the VPR-4 bar. Test in Task 3.

---

### Task 1: Render frames pushed during play

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, subscriptions.rs, worker.rs}`, `editor/packages/control-client/src/{client.ts, worker/host-worker.ts, transports/post-message.ts}`, the protocol fingerprint, generated TypeScript, `CHANGELOG.md` (the protocol 5 entry)
- Test: `crates/ludenea-control/tests/render_frames.rs`, `editor/packages/control-client/test/render-frames.test.ts`

**Interfaces:**
- Consumes: `Simulation::render_frame` (`&self`, guarded), `Host::tick_progress`, the subscription machinery.
- Produces:
  - `Topic::RenderFrames` (no fields). `Command::SetPushedLayers { debug_layers: Vec<String> }`, replies `Done`: the debug layers of pushed frames (none until set).
  - `Event::RenderFrames { frames: Vec<RenderFrame>, progress: f64 }`, pushed by `Host::advance` after its `Ticked` events when it ran at least one tick while subscribed. In the `for ran in 0..due` loop the frame of tick `due - 1` is extracted right after that step only when `due >= 2`, and the last tick's after the loop: one extraction per advance otherwise. No tick, no event.
  - A failed extraction pushes nothing and sends one `Console` error per distinct message (`render frame of tick N: <cause>`); play continues.
  - `WorkerHost` posts this event as `{ type: "render_frames", progress, frames: ArrayBuffer[] }`, each frame `postcard`-encoded, transferred; the control client exposes it to listeners as `{ type: "render_frames", progress, frames: Uint8Array[] }` on the worker transport and as JSON frames on the WebSocket transport (`frames: RenderFrame[]`), typed as a union.

- [ ] **Step 1: Write the failing tests**: `render_frames.rs`: `subscribed_advance_pushes_the_tick_it_ran`, `advance_of_three_ticks_pushes_the_last_two_oldest_first`, `frames_follow_the_ticked_events_of_the_same_advance`, `no_tick_no_frames`, `unsubscribed_host_pushes_nothing`, `pushed_frames_hold_only_the_set_layers`, `progress_matches_tick_progress`, `a_failing_extraction_logs_once_and_play_continues` (a test app whose extractor panics on tick 5), `pushed_frames_never_change_the_hash` (hashes at tick 300 equal with and without the subscription), `worker_frames_decode_to_the_same_frames`; `render-frames.test.ts`: `worker_frames_arrive_as_bytes`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test render_frames`. Expected: compile failure.
- [ ] **Step 3: Implement**; regenerate TypeScript and the fingerprint.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-control --test typescript && pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(protocol): push the render frames of the ticks a subscribed host runs`.

---

### Task 2: What the renderer answers without a GPU, and the browser renderer for the editor

**Files:**
- Create: `crates/ludenea-render/src/pick.rs`
- Modify: `crates/ludenea-render/src/{lib.rs, atlas.rs, camera.rs, web.rs}`, `xtask/src/main.rs` (`wasm-render` unchanged in output; README names `editor/packages/render-wasm`), `.gitignore` (`editor/packages/render-wasm/*` except `.gitkeep`), `editor/package.json` (script `build:render`: `cargo xtask wasm-render packages/render-wasm`), `CHANGELOG.md` (Rust API)
- Create: `editor/packages/render-wasm/.gitkeep` (no `package.json`: resolved by alias and `paths`, as `debug-draw-wasm` is, so the pnpm lockfile does not depend on a build)
- Test: `crates/ludenea-render/tests/{frames.rs, pick.rs}`, `editor/e2e/renderer.spec.ts`, `editor/e2e/renderer-webgpu.spec.ts`

**Interfaces:**
- Consumes: 3a/3b `Atlas`, `Region`, `TextureMeta`, `camera::world_to_clip`, `DebugDrawFrame::ghost`, `WebRenderer`'s kept `pushed` frame.
- Produces (`ludenea_render`, GPU-free):
  - `atlas::frames_of(png: &[u8], meta: &TextureMeta) -> Result<TextureFrames, FramesError>`; `TextureFrames { width: u32, height: u32, frames: Vec<FrameRect> }`, `FrameRect { x, y, width, height, pivot: (u32, u32) }`; `FramesError::{Decode { reason: String }, Grid { reason: String }}`. Both `frames_of` and `pack` call one private `cut(image, meta) -> Result<Vec<FrameRect>, FramesError>` after their own decode (no double decode); `TextureError::{Decode, Grid}` wrap the same reasons with the id and path.
  - `camera::screen_to_world(screen: Vec2, size: Size, camera: CameraView, parallax: f32) -> Vec2`, the inverse of `world_to_clip` for a layer.
  - `pick::pick(frame: &RenderFrame, regions: &BTreeMap<(AssetId, u32), Region>, screen: Vec2, size: Size, camera: CameraView) -> Vec<Entity>`: sprites whose drawn quad (pivot, pixels per unit, flip, rotation; a missing region is the 1 by 1 placeholder) and tilemaps whose non-empty cell contain the point in their layer's view; topmost first (layer, order, entity).
  - `pick::bounds(frame, regions, entities: &BTreeSet<Entity>) -> Option<(Vec2, Vec2)>`: world box of their drawn sprites and tilemaps on layers of parallax 1 (other layers are left out: framing them has no single world box).
- Produces (`WebRenderer`, wasm-bindgen):
  - `push_frame_bytes(&mut self, bytes: &[u8], overlay_json: &str) -> Result<String, JsValue>` (same as `push_frame`, postcard input), both returning `{ tick, missing, debug }` as JSON (`debug` is the frame's `DebugDrawFrame`, which the page's labels and drag outline read).
  - `pick(&self, x: f32, y: f32, camera_json: Option<String>) -> String` (CSS pixels; `None` = the game camera; JSON `Entity[]`), `bounds(&self, entities_json: &str) -> Result<Option<String>, JsValue>` (`{ min, max }`), `screen_to_world(&self, x: f32, y: f32, parallax: f32, camera_json: Option<String>) -> String`.
  - Every error is a JS `Error` whose `name` is one of `NoAdapter`, `Gpu`, `Limit`, `DeviceLost`, `InvalidAtlas` (the `RenderError` variants), `InvalidInput` (bad JSON or postcard, PNG count mismatch), `Surface` (`create_surface`), `NoSurfaceTexture` (the canvas has no texture this frame: hidden or occluded), with the cause as `message`.
  - Free functions `textureFrames(png: Uint8Array, meta_json: &str) -> Result<String, JsValue>`, `ghostFrame(frame_json: &str) -> Result<String, JsValue>` (moved from `ludenea_draw::render::ghost_frame`) and `setPanicHandler(handler: js_sys::Function)`: a panic hook forwarding the panic message to `handler` (VPR-2).

- [ ] **Step 1: Write the failing tests**: `frames.rs`: `grid_frames_match_what_pack_cuts`, `single_image_is_one_frame_with_the_meta_pivot`, `frames_of_a_broken_png_name_the_cause`; `pick.rs`: `pick_hits_a_sprite_by_its_drawn_quad`, `pick_respects_flip_pivot_and_rotation`, `pick_returns_the_topmost_first`, `pick_on_a_parallax_layer_uses_its_view`, `pick_hits_a_painted_tile_and_misses_an_empty_cell`, `missing_texture_placeholder_is_pickable`, `bounds_cover_sprites_and_tilemaps_of_parallax_one`, `screen_to_world_inverts_world_to_clip`; `renderer.spec.ts`: `push_frame_bytes_draws_like_push_frame`, `texture_frames_from_the_page`, `bad_json_is_named_InvalidInput`, `a_panic_reaches_the_handler`; `renderer-webgpu.spec.ts`: `device_lost_error_is_named_DeviceLost`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --features gpu`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-render --features gpu && cargo test --locked --workspace && cargo xtask wasm-render editor/e2e/fixtures/renderer/pkg && pnpm -C editor e2e renderer.spec.ts renderer-webgpu.spec.ts`. Expected: PASS on webgl2 and webgpu.
- [ ] **Step 5: Commit** `✨ feat(render): pick, frame and cut textures without a GPU, and name browser errors`.

---

### Task 3: The viewport draws with `ludenea-render`

**Files:**
- Create: `editor/packages/editor-builtins/src/viewport/{textures.ts, frames.ts}`, `editor/packages/editor-builtins/test/fake-render-wasm.ts` (fakes `WebRenderer` and the free functions; the old `fake-renderer.ts` stays until Task 4, for the divergence ghost)
- Modify: `editor/packages/editor-builtins/src/viewport/{ViewportPanel.tsx, state.ts, camera.ts, overlay.ts, index.tsx}`, aliases and `paths` for `@ludenea/render-wasm` in `editor/apps/web/vite.config.ts`, `editor/apps/desktop/vite.config.ts`, `editor/apps/web/vitest.config.ts`, `editor/packages/editor-builtins/vitest.config.ts`, `editor/packages/editor-builtins/tsconfig.json`; `editor/packages/editor-builtins/test/imports.test.ts` (`ALLOWED`), `editor/e2e/performance.spec.ts`, `.github/workflows/determinism.yml` (editor jobs also run `pnpm -C editor build:render`), README (Editor: viewport; the desktop editor's JSON frames limit; WebKitGTK has no WebGPU and falls back to WebGL2, or shows the failure alert without it)
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/e2e/performance.spec.ts`

**Interfaces:**
- Consumes: Tasks 1 and 2; the project protocol's `list_files` and `read_binary`; `editor.project.on("files_changed")`; `editor.timeline.subscribe`; `onGameChanged`.
- Produces:
  - `textures.ts`: `loadTextures(project): Promise<LoadedTextures>`, `LoadedTextures { sources: { id: string; path: string; meta: TextureMeta }[]; pngs: Uint8Array[] }`, every `Texture` asset of the project (`list_files`) with its bytes (`read_binary`); an entry whose id is `0` (an unreadable meta) is left out and the project's warning for it logged once. Textures belong to the project: loaded when the renderer is created, not per document. `set_textures` refusals are logged once each in the editor console as `<path>: <message>`.
  - `frames.ts`: `class FrameFeed`: on `play` it sends `set_pushed_layers` then `subscribe { topic: render_frames }`; on pause `unsubscribe`; a layer change while playing sends only `set_pushed_layers`; it subscribes again on `onGameChanged`. On `render_frames` it pushes each frame (`push_frame_bytes` or `push_frame`) in order and records `{ progress, rate: lastTicked.effective_rate, arrival }`. After any timeline change while paused (step, seek, undo, edit, reload) or a layer change, it requests `get_render_frame` for the tick on screen. Any frame or reply whose tick is older than the newest pushed tick, or other than the tick on screen while paused, is dropped. `alpha(now)` is `progress + (now - arrival) / 1000 * rate` clamped to 0..1 while playing and `1` while paused (1 draws the current tick). `lastFrame(): { json?: string; bytes?: Uint8Array } | null` keeps the newest pushed frame; a selection change pushes it again with the new overlay.
  - `camera.ts`: `toCameraView(camera, size): CameraView` (`view = size / pixelsPerMeter`); `fromCameraItem(item, size): Camera` (the editor camera equivalent to the game camera); `ViewportState.gameCamera: boolean`, command `viewport.toggle-game-camera` ("View through the game camera", shortcut `C`): draws with `camera: null`, and the overlay, picking and tools use `fromCameraItem` of the frame on screen so they stay aligned.
  - The animation frame loop calls `draw(alpha, camera)` and draws the overlay; `resize` on size or pixel ratio change. A `NoSurfaceTexture` error skips the frame and draws again on the next one.

- [ ] **Step 1: Write the failing tests** (`viewport.test.tsx`, ported to `fake-render-wasm.ts`, including `a_skipped_draw_is_drawn_again_while_paused` on `NoSurfaceTexture`): `textures_are_uploaded_when_the_renderer_is_created`, `a_refused_texture_is_logged_once`, `a_texture_with_an_unreadable_meta_is_left_out_with_its_warning`, `playing_subscribes_and_pushes_each_frame_in_order`, `pausing_unsubscribes_and_draws_the_tick_on_screen_at_alpha_1`, `stepping_while_paused_draws_the_new_tick_at_alpha_1`, `alpha_is_extrapolated_and_clamped_at_any_speed` (speeds 0.25 and 4), `toggling_a_layer_while_playing_keeps_frames_coming`, `a_reply_older_than_the_newest_frame_is_dropped`, `selecting_while_paused_pushes_the_frame_again`, `game_camera_toggle_keeps_overlay_and_pick_aligned`, `a_new_game_subscribes_again`, `drawing_between_frames_sends_no_request`; `performance.spec.ts`: `playing_the_swarm_keeps_p95_under_20_ms`.
- [ ] **Step 2: Run** `pnpm -C editor test viewport`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor build:render && pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor build && pnpm -C editor e2e smoke.spec.ts performance.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): draw the viewport with the game renderer from pushed frames`.

---

### Task 4: Picking, framing and the ghost on the new renderer; the old renderer deleted

**Files:**
- Modify: `editor/packages/editor-builtins/src/viewport/{ViewportPanel.tsx, camera.ts, index.tsx}`, `editor/packages/editor-builtins/src/divergence/index.tsx`, `editor/package.json` (drop `build:wasm`), the aliases and `paths` listed in Task 3 (drop `@ludenea/debug-draw-wasm`), `test/imports.test.ts`, `.gitignore` (drop the `debug-draw-wasm` entry), `xtask/src/main.rs` (drop `wasm-renderer` and the `ludenea-draw --features render` clippy check), `.github/workflows/determinism.yml` (drop the `render` clippy line and both `pnpm -C editor build:wasm` steps), README (the development commands), `CHANGELOG.md` (Rust API, breaking: `ludenea_draw::render` and its feature removed)
- Delete: `crates/ludenea-draw/src/render.rs`, the `render` feature and its dependencies in `crates/ludenea-draw/Cargo.toml`, `editor/packages/debug-draw-wasm/`, `editor/packages/editor-builtins/test/fake-renderer.ts`
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/e2e/divergence.spec.ts`

**Interfaces:**
- Consumes: Task 2 (`pick`, `bounds`, `ghostFrame`), Task 3.
- Produces: a click picks with the host's `pick` (colliders, at the world point of parallax 1) and `renderer.pick` (sprites and tilemaps), host hits first, without duplicates; `viewport.frame-selection` fits the union of `renderer.bounds` and the debug bounds; the divergence ghost uses `ghostFrame`.

- [ ] **Step 1: Write the failing tests**: `click_on_a_sprite_without_a_collider_selects_it`, `click_merges_collider_and_sprite_hits_without_duplicates`, `frame_selection_fits_a_sprite_only_entity`, `missing_texture_entity_is_still_selectable`; the divergence e2e passes on `ghostFrame`.
- [ ] **Step 2: Run** `pnpm -C editor test viewport`. Expected: FAIL.
- [ ] **Step 3: Implement**, then delete the old renderer; `rg -n "ViewportRenderer|ghost_frame|debug-draw-wasm|wasm-renderer|build:wasm"` finds nothing outside the CHANGELOG.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo xtask check && pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor build && pnpm -C editor e2e divergence.spec.ts smoke.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `♻️ refactor(editor-builtins): pick and frame with the game renderer and delete the old viewport renderer`.

---

### Task 5: Viewport failures, retry and a lost device

**Files:**
- Modify: `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx`, `editor/packages/editor-builtins/test/fake-render-wasm.ts` (scripted failures by name), `editor/e2e/smoke.spec.ts` (asserts no "stopped rendering"), `editor/e2e/playwright.config.ts` (`viewport-webgpu.spec.ts` in the `webgpu` project's `testMatch` and the `chromium` project's `testIgnore`), `.claude/superpowers/plans/2026-10-19-quality-bar-backlog.md` (VPR-2 marked done, naming the commits of Tasks 2 and 5), README (Editor: viewport)
- Create: `editor/e2e/viewport-webgpu.spec.ts`
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/e2e/viewport-webgpu.spec.ts`

**Interfaces:**
- Consumes: Task 2's named errors and `setPanicHandler`, Task 3's `FrameFeed.lastFrame` and `loadTextures`.
- Produces (in `ViewportPanel`):
  - `DeviceLost` frees the renderer, replaces the `<canvas>` element (a lost WebGPU context cannot be configured again), creates a renderer, uploads the textures and pushes `lastFrame()`: at most once per 60 seconds.
  - A second loss within that time, a failing creation, a panic reported by the handler, or three failing draws in a row (`NoSurfaceTexture` excluded) stop the loop and render `<div role="alert">The viewport stopped rendering: {message}</div>` with a `Retry` button that recreates the renderer and resumes. The session keeps playing throughout. This alert replaces the "cannot render" paragraph.

- [ ] **Step 1: Write the failing tests**: `a_draw_that_keeps_failing_offers_a_retry` (the VPR-2 test), `retry_recreates_the_renderer_and_draws_again`, `a_lost_device_recreates_the_renderer_once_with_textures_and_frame`, `a_second_loss_within_a_minute_shows_the_alert`, `a_failed_creation_shows_the_alert_with_retry`, `a_panic_shows_the_alert`, `missing_surface_textures_never_show_the_alert`, `the_session_keeps_playing_while_the_viewport_is_stopped`; `viewport-webgpu.spec.ts`: `editor_viewport_recovers_from_a_destroyed_device` (serves the template project; an `addInitScript` wraps `GPUAdapter.prototype.requestDevice` to keep the device on `window`, the test calls `destroy()` and waits for the viewport to draw a non-background pixel again).
- [ ] **Step 2: Run** `pnpm -C editor test viewport`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor build && pnpm -C editor e2e viewport-webgpu.spec.ts smoke.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `🐛 fix(editor-builtins): recover the viewport from a lost GPU and offer a retry`.

---

### Task 6: Hot reload of textures in the editor

**Files:**
- Modify: `editor/packages/editor-builtins/src/viewport/{textures.ts, ViewportPanel.tsx, index.tsx}`, README (Editor: textures)
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`

**Interfaces:**
- Consumes: `editor.project.on("files_changed")` (`Texture` changes; a `.meta` change arrives as its PNG's change), Task 3's `loadTextures`, Task 2's `textureFrames`.
- Produces: `textures.ts` `reloadChanged(current: LoadedTextures, changes: FileChange[], project): Promise<LoadedTextures>`: reads the changed and created textures again, drops removed ones, and checks each new version with `textureFrames`; a version that fails (unreadable bytes, `textureFrames` error, id `0` after `list_files`) keeps the previous version in the set and logs one warning naming the file and the cause. The panel then calls `set_textures` with the whole set and pushes `lastFrame()` again. No control command is sent. `document.ts` is not touched: it already reloads only prefabs and data assets.

- [ ] **Step 1: Write the failing tests**: `a_changed_png_is_uploaded_again_and_redrawn`, `a_changed_meta_reloads_its_texture`, `a_removed_png_drops_its_texture`, `a_broken_png_keeps_the_previous_texture_and_warns_once`, `an_invalid_meta_keeps_the_previous_texture_and_warns_once`, `texture_reload_sends_no_control_command` (the fake control sees only `get_render_frame`), `data_asset_changes_still_reload_the_session`.
- [ ] **Step 2: Run** `pnpm -C editor test viewport`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): draw a changed texture without reloading the session`.

---

### Task 7: Hot reload of textures in `play`

**Files:**
- Create: `crates/ludenea-runtime/src/native/watch.rs`
- Modify: `crates/ludenea-app/src/project.rs` (per-file failures), its callers in `crates/ludenea-cli/src/commands.rs` and `crates/ludenea-project`, `crates/ludenea-runtime/{Cargo.toml (notify, notify-debouncer-full under native), src/native/{mod.rs, window.rs, surface.rs}}` (both `PlayOptions` literals of the window tests included), `crates/ludenea-cli/src/commands.rs` (`PlayArgs::no_watch`), README (play), `CHANGELOG.md`
- Test: inline tests in `watch.rs` and `project.rs`, `crates/ludenea-runtime/tests/surface.rs`, `crates/ludenea-cli/tests/cli.rs`

**Interfaces:**
- Consumes: `project_textures`, `atlas::pack`, `Display` and its `reopen` from 3b.
- Produces:
  - `ProjectTextures { textures: Vec<ProjectTexture>, failures: Vec<TextureFailure>, warnings: Vec<String> }`, `TextureFailure { path: String, cause: String }`: a PNG that cannot be read or a meta that does not parse is a failure of that file, no longer an error of the whole call (only an unreadable folder is). Existing callers report failures as they reported warnings.
  - `native::watch::TextureWatch::start(scene: &Path, project: Option<&Path>) -> Result<TextureWatch, RuntimeError>` watching the folders `project_textures` scans (a second watcher beside `ludenea-project`'s, as the runtime does not depend on that crate); `poll(&mut self) -> Option<ProjectTextures>` returns the textures read again after a debounced change to a `.png` or `.png.meta`.
  - `Display` keeps the last valid `ProjectTexture` per path; `Display::replace_textures(&mut self, update: &ProjectTextures) -> Vec<String>`: takes the new valid ones, keeps the previous version of each failed path, drops paths gone from both lists, packs, `set_atlas`, stores the atlas `reopen` uses, marks the frame stale, and returns one warning per failure (`<path>: <cause>, keeping the previous version`).
  - `PlayOptions.watch: Option<(PathBuf, Option<PathBuf>)>` (`None` with `--no-watch`); the window polls once per frame and writes warnings to `out`.

- [ ] **Step 1: Write the failing tests**: `project.rs`: `an_unreadable_png_is_a_failure_of_that_file`, `an_invalid_meta_is_a_failure_of_that_file`; `watch.rs`: `a_written_png_is_reported_once_after_the_debounce`, `an_unrelated_file_is_ignored`; `surface.rs`: `replaced_texture_changes_the_drawn_pixels` (offscreen display, a pixel sampled before and after), `broken_replacement_keeps_the_previous_texture`, `invalid_meta_keeps_the_previous_texture`, `replaced_textures_survive_a_reopen`; `cli.rs`: `play_help_lists_no_watch`.
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --features native && cargo test -p ludenea-app`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-runtime --features native && cargo clippy -p ludenea-runtime --features native --all-targets -- -D warnings && cargo test --locked --workspace`. Expected: PASS. Optionally, by hand: edit `examples/swarm/assets/bug.png` while `swarm play` runs.
- [ ] **Step 5: Commit** `✨ feat(runtime): repack changed textures while a game plays`.

---

### Task 8: Texture previews and animation clips in the inspector

**Files:**
- Create: `editor/packages/editor-builtins/src/textures/{index.tsx, TexturePreview.tsx, ClipPlayer.tsx}`
- Modify: `editor/packages/editor-shell/src/{plugin.ts, editor.ts, registry.ts, stores/selection.ts}`, `editor/shared/plugin.ts` (types), `editor/packages/editor-builtins/src/{index.ts, assets/AssetsPanel.tsx, assets/index.tsx, inspector/InspectorPanel.tsx}`, README (Editor: textures and clips; Editor: plugins)
- Test: `editor/packages/editor-builtins/test/{textures.test.tsx, assets.test.tsx, inspector.test.tsx}`, `editor/packages/editor-shell/test/loader.test.ts`

**Interfaces:**
- Consumes: Task 2's `textureFrames`, Task 3's texture loading.
- Produces (public editor API):
  - `selectAsset(id: AssetId, kind: AssetKindName)` (the selection holds the kind; existing callers pass `"Data"`); the asset browser selects textures as well as data assets.
  - `editor.textures`: `list(): Observed<TextureInfo[]>` (`{ id, path, meta }`), `bytes(path): Promise<Uint8Array>`, `frames(path): Promise<TextureFrames>` (rejects with the `textureFrames` message).
  - `editor.inspectors.registerAsset(name: string, inspector: AssetInspector): Disposable`, `AssetInspector = ComponentType<{ id: AssetId; value: unknown | null; onChange(path: PathSegment[], value: unknown): void }>`, keyed by a data type name (`"ludenea::SpriteSheet"`) or `"Texture"`; rendered below the generic fields of a data asset, and alone for a texture (`value: null`). A texture's inspector needs no running game; a data asset's still says "No game is running."
- Produces (`texturesPlugin`, a built-in written on that API):
  - Inspector `"Texture"`: the image at an integer zoom with nearest filtering, each frame outlined and numbered, the pivot marked, size, frame count and pixels per unit; a refused texture shows its message as `role="alert"`.
  - Inspector `"ludenea::SpriteSheet"`: a `ClipPlayer` per clip (play and pause, the frame index) advancing one frame every `ticks_per_frame` ticks at the game's tick rate; a clip past its texture's frame count says so and does not play.

- [ ] **Step 1: Write the failing tests**: `textures.test.tsx`: `selecting_a_texture_previews_its_frames`, `preview_marks_the_pivot`, `broken_texture_preview_shows_the_error`, `texture_preview_works_without_a_game`, `clip_player_advances_one_frame_every_ticks_per_frame`, `clip_past_the_texture_frames_is_reported`; `assets.test.tsx`: `textures_are_selectable`; `inspector.test.tsx`: `an_asset_inspector_renders_below_the_generic_fields`; `loader.test.ts`: `a_project_plugin_registers_an_asset_inspector_and_reads_texture_frames`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): preview textures and play animation clips in the inspector`.

---

### Task 9: `PaintTiles` in the control protocol

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs}`, the fingerprint, generated TypeScript, `CHANGELOG.md` (the protocol 5 entry)
- Test: `crates/ludenea-control/tests/paint_tiles.rs`

**Interfaces:**
- Consumes: the world's component JSON (`component_json`; the host does not depend on `ludenea-render`), the `SetField` paths of the host.
- Produces:
  - `Command::PaintTiles { entity: Entity, area: TileArea, tile: Option<u32> }`, replies `Done`; `TileArea { x: u32, y: u32, width: u32, height: u32 }` in cells, `(0, 0)` the bottom-left cell, row-major as `Tilemap::tiles`; `None` erases.
  - The host reads the entity's `ludenea::Tilemap`, fills the area, and turns the command into `SetField { target: Component { entity, component: "ludenea::Tilemap" }, path: [Field { name: "tiles" }], value }` before `edit_command`, `scene_change_of`, `edit_of` and `batch`, so it applies and records exactly as that `set_field` (scene edit, or prefab override on an instance, at tick 0; one timed edit later; one undo step). `changes_state`, `edit_context` (`paint_tiles`) and `changed_value` (same key as a `set_field` of the tilemap, so both in one batch are refused) include it.
  - Refused with `InvalidCommand`: a dead entity (`NoSuchEntity`), no `ludenea::Tilemap` ("entity 4v1 has no ludenea::Tilemap; is RenderPlugin added?"), a tilemap whose `tiles.len() != width * height`, an empty area, an area outside the grid or whose end overflows (checked additions; "area x 30..34, y 0..2 is outside the 32 by 18 grid"). A tile index past the tileset is accepted (the host cannot know the tileset), drawn as the placeholder and listed in `DrawReport::missing`.

- [ ] **Step 1: Write the failing tests**: `paint_fills_the_rectangle_at_tick_0_as_one_scene_edit`, `erase_clears_the_rectangle`, `paint_undoes_in_one_step`, `paint_on_a_prefab_instance_is_an_override`, `paint_past_tick_0_records_a_timed_edit_that_replays`, `paint_records_the_same_edit_as_set_field` (edit logs equal), `paint_and_set_field_of_one_tilemap_in_a_batch_are_refused`, `area_outside_the_grid_is_refused_naming_both`, `overflowing_area_is_refused`, `entity_without_a_tilemap_is_refused`, `dead_entity_is_refused`, `empty_area_is_refused`, `unknown_tile_index_is_accepted`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test paint_tiles`. Expected: compile failure.
- [ ] **Step 3: Implement**; regenerate TypeScript and the fingerprint.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-control --test typescript && pnpm -C editor typecheck`. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `✨ feat(protocol): fill and erase tilemap rectangles as recorded edits`.

---

### Task 10: Viewport tools and editing in the public editor API

**Files:**
- Modify: `editor/packages/editor-shell/src/{plugin.ts, editor.ts, registry.ts}`, `editor/shared/plugin.ts` (types), `editor/packages/editor-builtins/src/timeline/editing.tsx` (implemented on the public `editor.edit`), `editor/packages/editor-builtins/src/viewport/{ViewportPanel.tsx, state.ts, index.tsx}`, README (Editor: plugins)
- Test: `editor/packages/editor-builtins/test/viewport.test.tsx`, `editor/packages/editor-shell/test/loader.test.ts`

**Interfaces:**
- Produces (public editor API):
  - `editor.edit(command: EditCommand): Promise<void>` and `editor.batched(commands: EditCommand[]): Promise<void>`: today's internal `edit()` and `batched()` (editability check, `onEdited` listeners, timeline refresh), now public; the built-ins call them.
  - `editor.viewport.tools.register(tool: ViewportTool): Disposable` (beside `editor.overlays`), `editor.viewport.activeTool` (a store: `string | null`, `null` is the built-in select and move), `editor.viewport.setTool(id: string | null)`.
  - `interface ViewportTool { id: string; title: string; cursor?: string; available(editor: Editor): boolean; pointerDown?(event: ToolPointer, context: ToolContext): void; pointerMove?(event: ToolPointer, context: ToolContext): void; pointerUp?(event: ToolPointer, context: ToolContext): void; overlay?(context: OverlayContext): void }`, `ToolPointer { screen: { x: number; y: number }; world: Vec2; button: number; shift: boolean; alt: boolean }`, `ToolContext { editor: Editor; screenToWorld(screen: { x: number; y: number }, parallax: number): Vec2; redraw(): void }` (`screenToWorld` is the renderer's, so a tool on a parallax layer maps correctly).
  - `available` is evaluated again on every selection, timeline and game change; a tool that becomes unavailable is deactivated; `Escape` returns to `null`. Pan (Space-drag, middle-drag) and zoom stay with the panel whatever the tool.

- [ ] **Step 1: Write the failing tests**: `an_active_tool_receives_pointer_events_in_world_units`, `escape_returns_to_select`, `a_tool_that_becomes_unavailable_is_deactivated`, `pan_and_zoom_work_with_a_tool_active`; `loader.test.ts`: `a_project_plugin_registers_a_viewport_tool_and_edits`.
- [ ] **Step 2: Run** `pnpm -C editor test viewport loader`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-shell): let plugins add viewport tools and send edits`.

---

### Task 11: Tilemap painting

**Files:**
- Create: `editor/packages/editor-builtins/src/tilemap/{index.tsx, TilePalette.tsx, paint.ts}`, `editor/e2e/tilemap.spec.ts`, `editor/e2e/fixtures/tilemap-project/` (a copy of the project template with `examples/swarm/assets/bug.png` and its meta as the tileset, and a scene holding a 16 by 9 `Tilemap` of cell size 1 at the origin on layer `world`)
- Modify: `editor/packages/editor-builtins/src/index.ts`, README (Editor: tilemaps)
- Test: `editor/packages/editor-builtins/test/tilemap.test.tsx`, `editor/e2e/tilemap.spec.ts`

**Interfaces:**
- Consumes: Tasks 8 (`editor.textures`), 9, 10; `editor.entities.watch`.
- Produces (`tilemapPlugin`, built on the public API only):
  - Panel `ludenea.tile-palette`: the selected tilemap's tileset frames as a grid of buttons; the chosen tile is plugin state.
  - Commands `tilemap.paint` and `tilemap.erase` (shortcuts `B` and `E`) calling `setTool`; tools `tilemap.paint` and `tilemap.erase` available while exactly one entity with `ludenea::Tilemap` is selected and the timeline is paused (the tilemap is cached from `editor.entities.watch`, so `available` stays synchronous). A drag selects a rectangle of cells, previewed in the overlay, sent on release as one `paint_tiles` through `editor.edit`; a right drag with `paint` erases. A refused command is shown in the editor console.
  - `paint.ts`: `cellAt(world: Vec2, pose: Pose, tilemap: Tilemap): { x: number; y: number } | null` (world point taken with the tilemap layer's parallax), `areaBetween(a, b, tilemap): TileArea` clamped to the grid.

- [ ] **Step 1: Write the failing tests**: `tilemap.test.tsx`: `cell_at_maps_world_points_to_cells`, `cell_at_follows_the_pose`, `area_between_is_clamped_to_the_grid`, `dragging_sends_one_paint_tiles_on_release`, `right_drag_erases`, `tools_are_unavailable_while_playing`, `painting_paused_past_tick_0_sends_the_same_command`, `a_refused_paint_is_shown_in_the_console`; `tilemap.spec.ts`: `paint_a_rectangle_undo_and_save` (serves the fixture project, selects the tilemap, picks tile 2, drags a rectangle, samples a canvas pixel inside it against the background, undoes and sees the background again, paints, saves, reopens and samples it again).
- [ ] **Step 2: Run** `pnpm -C editor test tilemap`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor build && pnpm -C editor e2e tilemap.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor-builtins): paint and erase tilemaps in the viewport`.

---

### Task 12: The arena's art

**Files:**
- Create: `examples/arena/{Cargo.toml, src/lib.rs}` (the crate with `RenderPlugin` only; Task 13 completes it), the workspace member, `examples/arena/assets/art/{warrior,hero}/{idle,run,jump,fall}.png` and their `.meta`, `examples/arena/assets/art/town/{tileset,background,middleground}.png` and their `.meta`, `examples/arena/assets/{warrior,hero}-{idle,run,jump,fall}.sheet.ron` and their `.meta`, `examples/arena/CREDITS.md`
- Test: `examples/arena/tests/art.rs`

**Interfaces:**
- Produces: texture metas with `pixels_per_unit` chosen so a fighter stands about 2 world units tall and a tile is 1 unit, `filter: Nearest`, the grid measured from each image (frame width = image width / frame count), and a pivot at the fighter's feet measured on the idle image and used for all four. Eight sheets, one per animation PNG, each with one clip named after it (`idle`, `run`, `jump`, `fall`; `ticks_per_frame` 5 for idle and run, 4 for jump and fall). `pub const` asset ids for each sheet in `src/lib.rs`. `CREDITS.md`: one entry per pack (artist, pack, CC0, URL, the files taken, the SHA-256 of each downloaded archive).

- [ ] **Step 1: Author step**: the author downloads Fantasy Warrior and Martial Hero from https://luizmelo.itch.io and Gothicvania Town from https://opengameart.org/content/gothicvania-town into a folder outside the repository. Wait for its path.
- [ ] **Step 2: Write the failing tests**: `every_texture_packs_without_errors` (`project_textures` then `atlas::pack`: no failure, no `TextureError`), `every_clip_fits_its_texture` (`frames_of` count ≥ `first + count` for each sheet's own texture), `credits_list_every_committed_art_file`.
- [ ] **Step 3: Run** `cargo test -p arena --test art`. Expected: FAIL (no art yet).
- [ ] **Step 4: Import** the files named above, write the metas and sheets, and check each in the editor's texture inspector (Task 8).
- [ ] **Step 5: Run** `cargo test --locked -p arena --test art && cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 6: Commit** `🍱 chore(arena): add the CC0 fighter and town art with credits`.

---

### Task 13: The arena game

**Files:**
- Create: `examples/arena/{ludenea.toml, README.md, src/main.rs, src/fighter.rs, src/camera.rs, src/bin/record-replay.rs, assets/arena.scene.ron, assets/arena.scene.ron.meta}`
- Modify: `examples/arena/{Cargo.toml, src/lib.rs}`, README (workspace layout)
- Test: `examples/arena/tests/arena.rs`

**Interfaces:**
- Consumes: Task 12's sheets, `RenderPlugin`, `PhysicsPlugin` (`raycast`, `Body::kinematic`, capsule `Collider`), `inputs.len()` and `inputs.player(i)`, `Commands::spawn`.
- Produces: a project (`name = "arena"`, `default_scene = "assets/arena.scene.ron"`, `[features] web = ["ludenea/web"]`, `ludenea::worker_entry!(app)`), `pub fn app() -> App`:
  - Layers: `sky` (parallax 0.1), `town` (0.4), `tiles` (1.0), `fighters` (1.0).
  - `ArenaInput { run: f32, jump: bool }`; `map_raw_input(raw, _player)` reads the player's own raw input: arrows or A and D, the left stick and d-pad of its gamepad (`raw.gamepads[0]`), jump on `Space`, `KeyW`, `ArrowUp` or button 0.
  - `SpawnPoint { player: u8 }` entities in the scene; on the first tick `spawn_fighters` spawns one fighter per player present (`inputs.len()`, fixed for the whole simulation; `SetPlayers` returns to tick 0), at its spawn point: warrior for player 0, hero for player 1, a kinematic `Body` with `linear_velocity` zero, a capsule `Collider`, a `Sprite` on `fighters`, a `SpriteAnimation` on its idle sheet.
  - `Fighter { player: u8, velocity: Vec2, grounded: bool, facing: i8 }`. Two systems ordered after `physics::Step`: `sense_fighters` reads `(Entity, &Pose, &Collider)` and writes `Fighter` (`grounded` from a downward `raycast` from the capsule's centre, `0.05` past its bottom; the horizontal move allowed by a sideways `raycast`; the ray starts inside its own capsule, so it ignores it); `move_fighters` writes `Pose` from `velocity`. Constants: `RUN_SPEED = 0.12` units per tick, `JUMP_SPEED = 0.35`, `GRAVITY = 0.02` per tick², `MAX_FALL = 0.5`, clamped with `ludenea::math`.
  - `animate_fighters`: `idle`, `run`, `jump` (rising) or `fall` (falling and not grounded), switching `SpriteAnimation::sheet`, `clip` and `started` only when the animation changes; `Sprite::flip_x` follows `facing`, the sign of the last non-zero run.
  - `follow_camera`: the `Camera2d` entity's `Pose` centres on the fighters' midpoint and its `view` is their bounding box plus 6 units, at least 24 by 13.5 (`ludenea::math::max`).
  - The scene: the camera, two spawn points, a `Tilemap` on `tiles` using the town tileset (32 by 18 cells of 1 unit, all empty: painted in Task 14), the town backgrounds as sprites on `sky` and `town`, no colliders yet (Task 14).
  - `record-replay <out>`: plays 600 ticks of scripted input for two players and writes the replay.

- [ ] **Step 1: Write the failing tests** (on a test scene with a floor collider): `one_player_spawns_one_fighter`, `two_players_spawn_two_fighters`, `fighter_runs_and_turns_its_sprite`, `fighter_lands_on_a_collider_and_idles`, `jump_then_fall_switch_sheet_and_clip`, `fighter_stops_at_a_wall`, `camera_frames_both_fighters_and_keeps_its_minimum_view`, `same_hashes_at_one_two_and_eight_threads` (600 ticks, two players).
- [ ] **Step 2: Run** `cargo test -p arena`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p arena && cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(examples): add an arena where local players run and jump as LuizMelo fighters`.

---

### Task 14: The arena level, built in the editor, and the same hashes everywhere

**Files:**
- Modify: `examples/arena/assets/arena.scene.ron` (saved by the editor), `examples/arena/README.md` (how the level was made), README (golden files, the command recording `golden/arena.replay`), `xtask/src/main.rs` and `.github/workflows/determinism.yml` (wasm32 clippy gains `-p arena`)
- Create: `golden/arena.replay`, `examples/arena/tests/{golden.rs, level.rs, runtime.rs}`, `editor/e2e/arena.spec.ts`
- Modify: `.github/workflows/determinism.yml`: the native matrix (macOS, Ubuntu, Windows) verifies `golden/arena.replay` at each thread count with its own divergence upload step, as for the boxes goldens; the wasm32-wasip1 job verifies it under wasmtime; Linux runs `xvfb-run cargo run --locked --release -p arena -- play examples/arena/assets/arena.scene.ron --players 2 --ticks 600 --record arena-native.replay` then `verify` of it, and `render golden/arena.replay --tick 300 --out arena.png` uploaded as an artifact; the editor job builds `cargo run --locked -p ludenea-serve -- build examples/arena --web --out editor/e2e/fixtures/arena`
- Test: `examples/arena/tests/{golden.rs, level.rs, runtime.rs}`, `editor/e2e/arena.spec.ts`, `editor/e2e/standalone.spec.ts`

**Interfaces:**
- Consumes: every task above; `ludenea_runtime::Runner` and `Lobby` (3b).
- Produces: a level painted in the editor with the tilemap tools (ground and two platforms), fixed box colliders placed by hand under them, saved by the editor; `golden/arena.replay` recorded with `cargo run --locked --release -p arena --bin record-replay -- golden/arena.replay`.

- [ ] **Step 1: Author step, in the editor** (`cargo run --locked -p ludenea-serve -- serve examples/arena`): paint the ground and two platforms with the tile palette, place a fixed box collider under each, check both fighters land, save. The executor starts the editor and waits for the author.
- [ ] **Step 2: Write the failing tests**:
  - `level.rs`: `both_fighters_stand_on_the_ground_after_60_ticks`; `every_tile_run_has_a_collider_under_it` (for each maximal horizontal run of non-empty cells, a fixed collider's top edge equals the run's bottom edge within 0.01 and spans its width); `render_of_tick_300_draws_a_fighter_over_the_tiles` (offscreen `render`, a fighter pixel and a tile pixel differ from `BACKGROUND`).
  - `golden.rs`: `arena_replay_verifies_at_one_two_and_eight_threads`.
  - `runtime.rs`: `lobby_claims_two_keyboard_halves_and_spawns_two_fighters` (`Lobby` presses `KeyA` then `ArrowLeft` then `Enter`, `Runner::start` with its claims), `scripted_keys_move_both_fighters_and_the_replay_verifies` (`Runner::frame` with scripted `LiveDevices` for 600 frames; `export_replay` verifies headless).
  - `arena.spec.ts`: `editor_play_matches_headless_hashes` (one player, 300 ticks of no input: the timeline's hash equals `arena run <scene> --ticks 300 --hash`), `editor_two_player_replay_verifies_headless` (`set_players 2` before `play`, scripted keys, `export_replay`, then `arena verify` exits 0), `editor_draws_the_painted_level` (a tile pixel differs from the background).
  - `standalone.spec.ts`: `arena_page_matches_headless_hashes` (`?players=1&ticks=300` against `arena run --hash`), `arena_page_two_player_replay_verifies` (`?players=2`, scripted keys, "Save replay", `arena verify`).
- [ ] **Step 3: Run** `cargo test -p arena --test level --test runtime`. Expected: FAIL until the level is saved.
- [ ] **Step 4: Record** `golden/arena.replay`; wire CI.
- [ ] **Step 5: Run** `cargo test --locked --workspace && cargo run --locked --release -p arena -- verify golden/arena.replay && cargo xtask check && pnpm -C editor e2e arena.spec.ts standalone.spec.ts`. Expected: PASS. Then by hand, as a supplement to the tests: `cargo run --locked --release -p ludenea-serve -- play examples/arena` with a gamepad and the keyboard. Every item of spec section 12 is named in the commit message with the test that proves it.
- [ ] **Step 6: Commit** `✨ feat(examples): build the arena level in the editor and verify it on every runtime`.
