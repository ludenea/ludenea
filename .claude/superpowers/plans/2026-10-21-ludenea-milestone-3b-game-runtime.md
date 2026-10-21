# Ludenea Milestone 3b: Game Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Ludenea game runs on its own: `play` opens it in a native window (fixed-rate simulation, display-rate interpolated rendering, keyboard and gamepads for up to four local players claimed before the match), `ludenea play` builds and launches it, and `ludenea build --web` turns it into a standalone static web page; the renderer keeps its geometry on the GPU between ticks so 10,000 sprites hold 144 frames per second.

**Architecture:** Every runtime drives the same in-process `ludenea_control::Host` through typed commands (`LoadScene`, `SetPlayers`, `SetLiveInput`, `Play`, `GetRenderFrame`, `ExportReplay`), so the native window, the web page and the editor are all clients of the control surface and record the same sessions. Device assignment (keyboard halves, gamepads, claims, the pre-match lobby) is GPU-free, window-free code in `ludenea-app::players`, shared by the host, the native runtime and the web page. A new crate `ludenea-runtime` holds the runner (host loop, frame pacing, statistics), the native window behind feature `native` (winit, gilrs, wgpu surface) and the page's exports behind feature `web`. The renderer splits into `set_frames` (uploads instances with both ticks' transforms, once per tick) and `draw(alpha)` (writes view uniforms only); the shader interpolates.

**Tech Stack:** unchanged (Rust 1.97.1, wgpu `=30.0.1`, png `=0.18.1`, pollster `=1.0.1`, clap, serde, postcard, wasm-bindgen `=0.2.129`; TypeScript, Playwright). New pinned dependencies, each the newest release at implementation time accepted by wgpu 30's `raw-window-handle` 0.6, pinned with `=` and justified in its commit: `winit` (window, keyboard, fullscreen) and `gilrs` (gamepads). gilrs needs `libudev-dev` to build on Linux. The standalone page is plain JavaScript modules embedded in `ludenea-project`: building a game for the web needs no Node.js.

**Spec:** `.claude/superpowers/specs/2026-10-20-ludenea-rendering-3-design.md` (parent: `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md` section 14). Milestone 3a (`.claude/superpowers/plans/2026-10-20-ludenea-milestone-3a-render-core.md`) shipped spec sections 4, 5, 6.1, the packing half of 6.2, 8 without pushed frames, and 10 without the performance target. This plan covers spec section 7 (7.1, 7.2, 7.3 including the editor's multi-player capture), the build-time half of 6.2 for the web page, and the performance target of section 10. Milestone 3c keeps: the editor viewport on `ludenea-render`, pushed frames during play, texture previews, tilemap painting, hot reload of textures (section 6.3, in the editor and in `play`), the panel's device-lost retry, and the example scene with the art (sections 9, 11 and 12).

## Global Constraints

- Every Global Constraint of the milestone 1, 2a, 2b, 3a and quality-bar plans still applies (determinism lints for simulation code, target gates, committed generated files, `--locked`, commit style).
- The state hashes of `golden/` never move. `examples/boxes` does not change its systems or its input mapping.
- Nothing here changes the input log or replay formats (`INPUT_LOG_VERSION = 1`, `REPLAY_VERSION = 1`).
- No release happened since protocol 5: protocol changes stay in `PROTOCOL_VERSION = 5`, update `PROTOCOL_FINGERPRINT` and extend the existing "Control protocol 5" CHANGELOG entry with the new fingerprint. The project protocol does not change.
- Window, gamepad and surface code (`ludenea-runtime::native`) and the page (`ludenea-runtime::web`, the page scripts) are runtime code, not simulation: they may read the clock and use floats freely, and they never write simulation state except through host commands. Wall-clock reads carry `#[expect(clippy::disallowed_methods, reason = "runtime only: ... never feeds the simulation except through recorded inputs")]` (or the type lint's equivalent), as the editor bench does.
- `ludenea-runtime` without features depends on no window, gamepad or GPU crate. `native` adds winit, gilrs and `ludenea-render/gpu`, only on non-wasm targets; `web` adds `ludenea-render/web` and wasm-bindgen, only on `wasm32-unknown-unknown`. `ludenea-cli` depends on `ludenea-runtime` with `native` only for `cfg(not(target_family = "wasm"))`, so `wasm32-wasip1` game builds stay headless.
- Local players: at most `MAX_LOCAL_PLAYERS = 4`. The host accepts any player count of at least 1; only the lobby and `--players` are capped.
- Key names are `KeyboardEvent.code` strings everywhere (native keys are translated to them), so a game's `input_mapping` reads the same names in every runtime.
- A runtime never panics on a lost device, a lost or outdated surface, a zero-size window, a disconnected gamepad, a failing game, or a bad file; each is handled or reported with its cause, and `play` exits with code 2 on a failure (as `run` does).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p ludenea-runtime --features native --all-targets -- -D warnings`, `cargo xtask check`, `cargo test --locked --workspace`, `cargo test --locked -p ludenea-render --features gpu`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- **The runtimes drive the control host.** The native window and the web page load, play, feed input and export replays through `Host` commands, as the editor does, instead of stepping `Simulation` directly. One recording path, and the window is a client of the control surface (`AGENTS.md`).
- **Claims rules (builder's call, section 7.3).** A claim is at most one keyboard part (`Whole`, `Left`, `Right`) and at most one gamepad slot. Left half: `Backquote`, `Digit1` to `Digit5`, `Tab`, `KeyQ` `KeyW` `KeyE` `KeyR` `KeyT`, `CapsLock`, `KeyA` `KeyS` `KeyD` `KeyF` `KeyG`, `ShiftLeft`, `KeyZ` `KeyX` `KeyC` `KeyV` `KeyB`, `ControlLeft`, `AltLeft`, `MetaLeft`, `Space`; the right half is every other key. Automatic claims for `n` players: one player gets the whole keyboard and gamepad 0; otherwise player 0 gets the left half and gamepad 0, player 1 the right half and gamepad 1, and player `k >= 2` gamepad `k`.
- **Lobby keys (builder's call).** Any key of a keyboard half, or any gamepad button except Start and Select, claims the next free player slot for that device; `Escape` releases the keyboard's claims, Select releases its gamepad; `Enter` or Start begins the match once at least one slot is claimed. When the match begins with exactly one keyboard half claimed, that player gets the whole keyboard. `--players N` (native) and `?players=N` (web) skip the lobby with the automatic claims.
- **Default input on disconnect.** A player whose only device is a disconnected gamepad gets `I::default()`, not the mapping of an idle raw input: `Simulation::map_players` takes `Option<RawInput>` per player. A new gamepad connecting while a slot is orphaned takes the lowest orphaned slot, so a pad that comes back under another identifier reclaims its player.
- **Atlases of a web build** are packed with pages of at most 2048 pixels (the side every WebGL2 device supports), and a texture that cannot be packed fails the build: a shipped build never draws placeholders. `play` packs at launch from the project's PNGs, like `render`.
- **Replays.** `play --record <path>` writes the replay when the window closes, including after a game failure (up to the last tick that ran). The web page offers "Save replay", which downloads it.

## Review Focus

- **A gamepad unplugged mid-match, then plugged back in** (possibly under another identifier): the slot stays, the player gets `I::default()` while no claimed device is connected, the input log records it, and the returning pad takes its slot back. Tests in Tasks 1 and 6.
- **A long stall** (the window dragged, the laptop asleep for a minute): no burst of thousands of ticks; the host's pacer drops the debt beyond its cap, play slows down, and alpha stays within 0 to 1. Test in Task 6.
- **The game panics or fails mid-match**: the window closes, `play` prints the tick and message, exits 2, and `--record` still writes the replay of the ticks that ran, which `verify` accepts. Tests in Tasks 6 and 7.
- **A minimized (zero-size) window, a lost or outdated surface, a lost device**: drawing is skipped at zero size while the simulation keeps its pace; the surface is configured again on `Lost` or `Outdated`; a lost device rebuilds the device, the renderer and the atlas once, then fails with the cause if that fails too. Tests in Task 7.
- **`ludenea build --web` on a project made before this milestone** (no `web` feature in its `Cargo.toml`), with a texture that does not pack, or the page opened from `file://`: the build is refused naming the line to add, or every texture error; the page says to serve the folder over HTTP. Tests in Task 10.

---

### Task 1: Player devices, claims and the split

**Files:**
- Create: `crates/ludenea-app/src/players.rs`
- Modify: `crates/ludenea-app/src/{lib.rs, app.rs, simulation.rs, input.rs}` (`RawMapping::encode` takes `&[Option<RawInput>]`), `crates/ludenea/src/lib.rs` (prelude: `Claim`, `KeyboardPart`, `LiveDevices`), `CHANGELOG.md` (Rust API)
- Test: `crates/ludenea-app/tests/players.rs`

**Interfaces:**
- Produces (`ludenea_app::players`), all `Clone, Debug, PartialEq, Serialize, Deserialize` with the `ts` derive like `RawInput`:
  - `pub const MAX_LOCAL_PLAYERS: usize = 4;`
  - `pub enum KeyboardPart { Whole, Left, Right }`; `KeyboardPart::of(code: &str) -> KeyboardPart` returns `Left` or `Right` from the table in Deviations; `KeyboardPart::contains(&self, code: &str) -> bool`.
  - `pub struct Claim { pub keyboard: Option<KeyboardPart>, pub gamepad: Option<u32> }`
  - `pub struct LiveDevices { pub keys: BTreeSet<String>, pub gamepads: Vec<Option<RawGamepad>> }` (index = gamepad slot, `None` = disconnected)
  - `pub fn automatic_claims(players: usize) -> Vec<Claim>` (rules in Deviations).
  - `pub fn check_claims(claims: &[Claim]) -> Result<(), ClaimError>`; `ClaimError::{GamepadTwice { gamepad: u32, players: (usize, usize) }, KeyboardOverlap { players: (usize, usize) }}` (`Whole` overlaps anything; `Left` twice overlaps).
  - `pub fn split(devices: &LiveDevices, claims: &[Claim]) -> Vec<Option<RawInput>>`: player `i` gets the keys its part contains and its gamepad as `gamepads[0]`; `None` when it claims only a gamepad and that slot is disconnected or absent.
- Produces (`ludenea_app::Simulation`): `pub fn map_players(&self, players: &[Option<RawInput>]) -> Result<Vec<u8>, SimError>`: `None` encodes `I::default()`; `map_raw_input` stays and delegates.

- [ ] **Step 1: Write the failing tests**: `left_half_holds_wasd_and_space`, `right_half_holds_arrows_and_enter`, `automatic_claims_for_one_two_and_four_players`, `split_gives_each_player_its_half_and_its_gamepad`, `whole_keyboard_gets_every_key`, `disconnected_gamepad_only_player_gets_none` (Review Focus), `disconnected_gamepad_with_a_keyboard_half_keeps_the_keys`, `gamepad_claimed_twice_is_refused_naming_both_players`, `map_players_encodes_default_for_none` (asserts the bytes equal `encoded_default_inputs` for an all-`None` call of two players).
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test players`. Expected: compile failure.
- [ ] **Step 3: Implement** `players.rs` and `map_players`.
- [ ] **Step 4: Run** `cargo test --locked --workspace`. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `✨ feat(app): split live devices between local players by claims`.

---

### Task 2: The lobby

**Files:**
- Modify: `crates/ludenea-app/src/players.rs`
- Test: `crates/ludenea-app/tests/players.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces (`ludenea_app::players`):
  - `pub enum LobbyPress<'a> { Key(&'a str), Button { gamepad: u32, button: usize } }` (`button` in the Gamepad API standard order: 8 is Select, 9 is Start)
  - `pub enum LobbyEvent { Claimed { player: usize }, Released { players: Vec<usize> }, Full, Started { claims: Vec<Claim> }, Ignored }`
  - `pub struct Lobby`; `Lobby::new() -> Lobby`; `press(&mut self, press: LobbyPress<'_>) -> LobbyEvent`; `claims(&self) -> &[Claim]`; `started(&self) -> bool`. Rules in Deviations; a device already claimed pressing again is `Ignored`; releasing compacts the slots in claim order; a fifth claim is `Full`; presses after `Started` are `Ignored`.

- [ ] **Step 1: Write the failing tests**: `key_claims_its_half`, `same_half_twice_claims_once`, `gamepad_button_claims_its_slot`, `start_without_claims_is_ignored`, `enter_starts_with_the_claims_in_order`, `lone_keyboard_half_becomes_whole_at_start`, `fifth_claim_is_full`, `escape_releases_both_halves_and_compacts`, `select_releases_its_gamepad`, `presses_after_start_are_ignored`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test players`. Expected: FAIL.
- [ ] **Step 3: Implement** `Lobby`.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-app`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): claim local player slots in a pre-match lobby`.

---

### Task 3: Players in the control protocol and the editor's capture

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, pacer.rs}`, the protocol fingerprint, generated TypeScript, `CHANGELOG.md` (the protocol 5 entry), `editor/packages/control-client`, `editor/packages/editor-builtins/src/input/capture.ts` and its registration of commands, README (Editor: local players)
- Test: `crates/ludenea-control/tests/players.rs`, `editor/packages/editor-builtins/test/capture.test.ts`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces (protocol):
  - `Command::SetPlayers { players: u16, claims: Option<Vec<Claim>> }`, replies `Done`: replaces the recorded inputs with an empty log of `players` (`Session::load_inputs`, which seeks to tick 0) and stores the claims (`automatic_claims` when `None`). Refused with `InvalidInputs`: `players == 0`, a claims length other than `players` (naming both), a `ClaimError`.
  - `Command::SetLiveInput { devices: LiveDevices }` replaces `{ players }`: the host splits by its claims and maps with `map_players`; live input is `None` when every player is `None` or idle.
  - Any load that sets the player count from a log (`LoadInputs`, `LoadSession`, `LoadReplay`) resets the claims to `automatic_claims`.
- Produces (Rust only): `Pacer::progress(&self) -> f64` (owed fraction of the next tick, 0 to below 1); `Host::tick_progress(&self) -> f64` returns it.
- Produces (editor): `captureInput` sends every key and every slot of `navigator.getGamepads()` (`null` for empty slots) as `set_live_input { devices }`; a palette command "Set the player count" offers 1 to 4, each option labelled as replacing the recorded inputs, and sends `set_players { players, claims: null }`; the console logs the resulting assignment.

- [ ] **Step 1: Write the failing tests**: `players.rs`: `set_players_records_that_many_players`, `set_players_zero_is_refused`, `claims_of_the_wrong_length_are_refused_naming_both_counts`, `two_players_on_keyboard_halves_record_their_own_input`, `disconnected_gamepad_records_the_default_input` (Review Focus), `loading_a_session_resets_automatic_claims`, `tick_progress_grows_between_ticks`; `capture.test.ts`: `sends_every_gamepad_slot_with_nulls_for_empty_ones`, `set_player_count_sends_set_players`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test players`. Expected: compile failure.
- [ ] **Step 3: Implement**; regenerate the TypeScript, the declarations bundle and the fingerprint; update the editor tests that send `set_live_input`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-control --test typescript && pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(protocol): set the player count and split live devices in the host`.

---

### Task 4: Geometry stays on the GPU between ticks

**Files:**
- Modify: `crates/ludenea-render/src/{gpu/mod.rs, gpu/sprites.rs, gpu/tilemaps.rs, gpu/debug.rs, gpu/shaders/*.wgsl, interpolate.rs, web.rs}`, `crates/ludenea-cli/src/commands.rs` (`render`), `editor/e2e/fixtures/renderer/main.js`, `CHANGELOG.md` (Rust API, breaking), README (performance)
- Test: `crates/ludenea-render/tests/gpu.rs`, `crates/ludenea-render/tests/perf.rs` (`#[ignore]`), `editor/e2e/renderer.spec.ts`

**Interfaces:**
- Consumes: the 3a renderer.
- Produces (`ludenea_render::gpu`), replacing `Frames` and the frame arguments of `draw` and `render_rgba` (one way to draw):
  - `Renderer::set_frames(&mut self, previous: Option<&RenderFrame>, current: &RenderFrame, overlay: &Overlay) -> Result<DrawReport, RenderError>`: builds and uploads every sprite instance with both ticks' transforms (previous centre, current centre, previous rotation, arc to the current one; equal transforms and a zero arc for an entity new in `current` or teleported), the tilemap chunks (unchanged items keep their buffers), the debug vertices; instance buffers are reused and grown, never recreated when they fit.
  - `Renderer::draw(&mut self, target: &wgpu::TextureView, size: Size, alpha: f32, camera: Option<CameraView>) -> Result<(), RenderError>` and `render_rgba(&mut self, size: Size, alpha: f32, camera: Option<CameraView>) -> Result<Vec<u8>, RenderError>`: write the view uniforms (camera and tilemap positions blended on the CPU with the 3a rules, `alpha` clamped, NaN as 1) and the alpha uniform, then encode. `camera` is the editor's free camera, moved out of `Overlay`; `Frames` and `Rendered` are removed. Debug vertices are rebuilt only when the target size or the current frame's visible area changes, never for `alpha` alone.
  - `Renderer::stats(&self) -> RenderStats`, `pub struct RenderStats { pub buffers_created: u64, pub bytes_written: u64 }` (diagnostics: counts since creation).
  - `interpolate::interpolate` is no longer public; the shader rotates the previous rotation by `arc * alpha`, matching the 3a slerp.
  - `WebRenderer::set_frames(&mut self, previous_json: Option<String>, current_json: &str, overlay_json: &str) -> Result<String, JsValue>` (the `DrawReport` as JSON) and `WebRenderer::draw(&mut self, alpha: f32, camera_json: Option<String>) -> Result<(), JsValue>`.

- [ ] **Step 1: Write the failing tests**: `gpu.rs`: the 3a tests ported to `set_frames` then `render_rgba`, plus `draw_between_ticks_creates_no_buffer` (two `render_rgba` calls at alphas 0.25 and 0.75 after one `set_frames` of 10,000 sprites: `buffers_created` unchanged and `bytes_written` below 4 KiB between them), `halfway_rotation_matches_the_shorter_arc`, `teleported_sprite_is_not_blended`, `free_camera_moves_the_view_without_set_frames`; `perf.rs`: `ten_thousand_sprites_draw_144_frames_per_tick_pair` (release, `PERF_STRICT=1` asserts a 95th percentile CPU time per `draw` below 1 ms on the reference machine); `renderer.spec.ts`: the reference frame through `set_frames` and `draw`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --features gpu`. Expected: compile failure.
- [ ] **Step 3: Implement**; port `render` (`set_frames`, then `render_rgba(size, 1.0, None)`) and the fixture page.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-render --features gpu && cargo test --locked --workspace && cargo xtask wasm-render editor/e2e/fixtures/renderer/pkg && pnpm -C editor e2e renderer.spec.ts`. Expected: PASS, the `render` command's tests included.
- [ ] **Step 5: Commit** `⚡️ perf(render): keep sprite geometry on the GPU and interpolate in the shader`.

---

### Task 5: `examples/swarm`, the runtime's test game

**Files:**
- Create: `examples/swarm/{Cargo.toml, ludenea.toml, src/lib.rs, src/main.rs, src/bin/generate.rs, assets/swarm.scene.ron, assets/bug.png, assets/bug.png.meta, assets/bug.sheet.ron, assets/bug.sheet.ron.meta}`, `examples/swarm/README.md` (what it is, how to regenerate)
- Modify: README (workspace layout)
- Test: `examples/swarm/tests/swarm.rs`

**Interfaces:**
- Produces: a workspace member and a project (`ludenea.toml`: `name = "swarm"`, `default_scene = "assets/swarm.scene.ron"`), `[lib] crate-type = ["cdylib", "rlib"]`, `ludenea::worker_entry!(app)`, `pub fn app() -> App` with `RenderPlugin`, layers `background` (parallax 0.5) and `world`, a `Camera2d` showing 64 by 36 units, 10,000 animated sprites (`SpriteAnimation` looping, 4 frames) moving on deterministic paths, and four player sprites (`Player { index: u8 }`, one hidden per absent player) moved by `inputs.player(i)`.
  - `SwarmInput { direction: Vec2 }`; `map_raw_input(raw, _player)`: arrows and WASD, gamepad 0's left stick and d-pad, normalized.
  - `generate` writes the scene and a 4-frame 16 by 16 sheet drawn procedurally (no art, no generated imagery beyond flat shapes).

- [ ] **Step 1: Write the failing tests**: `swarm_spawns_ten_thousand_animated_sprites`, `player_two_moves_with_the_right_half`, `same_hashes_at_one_and_eight_threads` (600 ticks), `render_frame_lists_every_sprite`.
- [ ] **Step 2: Run** `cargo test -p swarm`. Expected: compile failure.
- [ ] **Step 3: Implement**; run `cargo run --locked -p swarm --bin generate` and commit the assets.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo run --locked --release -p swarm -- render examples/swarm/assets/swarm.scene.ron --tick 60 --out swarm.png` (inspect the PNG). Expected: PASS and a drawn swarm.
- [ ] **Step 5: Commit** `✨ feat(examples): add a sprite swarm to exercise the game runtime`.

---

### Task 6: `ludenea-runtime`: the runner, keys and gamepads

**Files:**
- Create: `crates/ludenea-runtime/{Cargo.toml, src/lib.rs, src/runner.rs, src/stats.rs, src/native/mod.rs, src/native/keys.rs, src/native/gamepads.rs}`; workspace member; `winit` and `gilrs` pinned in the workspace; every Linux CI job that builds the workspace installs `libudev-dev`
- Modify: README (Linux build dependency), `CHANGELOG.md`
- Test: `crates/ludenea-runtime/tests/runner.rs`, inline tests in `stats.rs`, `keys.rs`, `gamepads.rs`

**Interfaces:**
- Consumes: Tasks 1 to 3, `Host`, `record_replay`.
- Produces (`ludenea_runtime`, no feature):
  - `pub struct GameFiles { pub scene: String, pub assets: Vec<(AssetId, String)>, pub edits: Option<Vec<u8>> }`
  - `pub struct Runner`; `Runner::load(app: fn() -> App, files: &GameFiles, threads: Option<usize>, debug_layers: Vec<String>) -> Result<Runner, RuntimeError>` (`LoadScene`, then `LoadEdits`, `SetThreads`); `start(&mut self, claims: Vec<Claim>) -> Result<(), RuntimeError>` (`SetPlayers`, `Play`); `frame(&mut self, elapsed_seconds: f64, devices: &LiveDevices) -> Result<FrameUpdate, RuntimeError>` (sends `SetLiveInput` only when the devices changed, `advance`, `GetRenderFrame` once if any `Ticked` arrived); `frames(&self) -> (Option<&RenderFrame>, Option<&RenderFrame>)`; `export_replay(&mut self) -> Result<Vec<u8>, RuntimeError>`; `tick(&self) -> u64`.
  - `pub struct FrameUpdate { pub new_frames: bool, pub alpha: f32, pub tick: u64 }` (`alpha = Host::tick_progress`).
  - `pub enum RuntimeError { Load(String), Game { tick: u64, message: String }, Gpu(String), Window(String) }`; a `Paused { reason: Failed | Panicked }` event becomes `Game`.
  - `stats::FrameStats`: `record(&mut self, frame_seconds: f64)`, `summary(&self) -> String` ("frames N, frame time p50 X ms, p95 Y ms, p99 Z ms").
- Produces (`ludenea_runtime::native`, feature `native`):
  - `keys::code_name(key: winit::keyboard::KeyCode) -> Option<&'static str>`: an explicit match to `KeyboardEvent.code` names; unmapped keys are `None`.
  - `gamepads::standard(buttons: impl IntoIterator<Item = (gilrs::Button, f32)>, axes: impl IntoIterator<Item = (gilrs::Axis, f32)>) -> RawGamepad`: 17 buttons in the standard order (South 0, East 1, West 2, North 3, LeftTrigger 4, RightTrigger 5, LeftTrigger2 6, RightTrigger2 7, Select 8, Start 9, LeftThumb 10, RightThumb 11, DPadUp 12, DPadDown 13, DPadLeft 14, DPadRight 15, Mode 16), 4 axes with both Y axes negated (down is positive, as in browsers), d-pad axes folded into buttons 12 to 15.
  - `gamepads::Slots`: `connect(&mut self, id: usize) -> u32` (its previous slot if known, else the lowest orphaned slot, else a new one), `disconnect(&mut self, id: usize)`, `devices(&self, states: &BTreeMap<usize, RawGamepad>) -> Vec<Option<RawGamepad>>`.

- [ ] **Step 1: Write the failing tests**: `runner.rs` (swarm): `runner_matches_headless_hashes` (600 ticks of scripted devices at 1/144 s frames; the exported replay verifies with `ludenea_app::replay` and equals `record_replay` of its own inputs), `two_players_record_their_halves`, `devices_unchanged_send_no_command`, `stall_of_a_minute_runs_at_most_the_pacer_cap` (Review Focus), `alpha_stays_between_zero_and_one`, `game_panic_is_a_game_error_naming_the_tick` (Review Focus; a test app whose system panics at tick 30), `replay_after_a_failure_holds_the_ticks_that_ran`; `stats.rs`: `percentiles_of_known_frames`; `keys.rs`: `letters_arrows_and_modifiers_use_dom_codes`; `gamepads.rs`: `south_is_button_zero`, `stick_y_is_negated`, `dpad_axes_press_dpad_buttons`, `reconnected_pad_keeps_its_slot`, `new_pad_takes_the_orphaned_slot` (Review Focus).
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --features native`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-runtime --features native && cargo clippy -p ludenea-runtime --features native --all-targets -- -D warnings && cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(runtime): run a game through the control host with local devices`.

---

### Task 7: `play`: the native window

**Files:**
- Move: `crates/ludenea-cli/src/assets.rs` to `crates/ludenea-app/src/project.rs` (`ludenea_app::project::{project_assets, project_textures, ProjectTextures, ProjectTexture}`), callers updated, so the runtime and `ludenea-project` read project files without depending on the command line crate
- Create: `crates/ludenea-runtime/src/native/{window.rs, surface.rs, lobby.rs}`
- Modify: `crates/ludenea-render/src/gpu/device.rs` (a device for a surface), `crates/ludenea-cli/{Cargo.toml, src/lib.rs, src/commands.rs}`, `.github/workflows/determinism.yml` (a Linux step: `xvfb-run` of `swarm play` with `--players 1 --ticks 120 --record`, then `verify` of that replay), `xtask` `CHECKS` if it lists workflow steps, README (the command, measuring frame times), `CHANGELOG.md`
- Test: `crates/ludenea-runtime/tests/surface.rs`, inline tests in `lobby.rs`, `crates/ludenea-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 2, 4, 6; `project_assets`, `project_textures`, `atlas::pack`.
- Produces (`ludenea_render::gpu`, native only): `pub async fn for_surface(window: impl wgpu::WindowHandle + 'static) -> Result<(wgpu::Surface<'static>, wgpu::Device, wgpu::Queue), RenderError>`: the instance is built with the window's display handle and the adapter must be compatible with its surface, with the same fallback and `NoAdapter` message as `headless`. `ludenea-render` does not depend on winit.
- Produces (`ludenea_runtime::native`):
  - `pub struct PlayOptions { pub files: GameFiles, pub textures: Vec<ProjectTexture>, pub players: Option<u8>, pub ticks: Option<u64>, pub record: Option<PathBuf>, pub fullscreen: bool, pub vsync: bool, pub stats: bool, pub threads: Option<usize>, pub debug_layers: Vec<String>, pub size: (u32, u32), pub title: String }`
  - `pub fn play(app: fn() -> App, options: PlayOptions) -> Result<PlayOutcome, RuntimeError>`; `PlayOutcome { pub ticks: u64, pub stats: Option<String> }`. Runs the winit event loop: lobby (unless `players`), then the match; each redraw polls gilrs, builds `LiveDevices`, calls `Runner::frame`, then `set_frames` on new frames and `draw(alpha)`. Present mode `Fifo` with `vsync`, else `AutoNoVsync`. `F11` toggles borderless fullscreen. Closing the window, or reaching `ticks`, ends the loop; `record` is written in every case, failure included.
  - `surface::Presenter`: `resize(&mut self, width: u32, height: u32)` (zero skips drawing), `present(&mut self, draw: impl FnOnce(&wgpu::TextureView, Size) -> Result<(), RenderError>) -> Result<(), RuntimeError>`: reconfigures on `Lost` or `Outdated`, skips the frame on `Timeout`, fails on `OutOfMemory`; on `RenderError::DeviceLost` the caller rebuilds the device, renderer and atlas once.
  - `lobby::frame(lobby: &Lobby, connected: &[u32]) -> RenderFrame` (debug layer `runtime.lobby`: four slot outlines, filled when claimed, a wide rectangle for a keyboard part, a circle for a gamepad) and `lobby::title(game: &str, lobby: &Lobby) -> String` naming each claim and how to start.
- Produces (`ludenea_cli`, not on wasm): `Cmd::Play(PlayArgs)`: `PlayArgs { files: RunFiles, players: Option<u8> (1..=4), ticks: Option<u64>, record: Option<PathBuf>, fullscreen: bool, no_vsync: bool, stats: bool, threads: Option<usize>, debug_layers: Vec<String>, width: u32 (1280), height: u32 (720) }`; texture warnings as in `render`; exit 0, or 2 with `error: ...`.

- [ ] **Step 1: Write the failing tests**: `lobby.rs`: `lobby_frame_fills_claimed_slots`, `title_names_claims_and_how_to_start`; `surface.rs` (offscreen fakes of the surface outcomes): `zero_size_skips_drawing_and_keeps_ticking`, `lost_surface_is_configured_again`, `timeout_skips_one_frame`, `lost_device_rebuilds_once_then_fails_with_the_cause` (Review Focus); `cli.rs`: `play_help_lists_its_flags`, `play_refuses_five_players`, `play_with_a_bad_scene_exits_2_before_opening_a_window`.
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --features native && cargo test -p ludenea-cli --test cli`. Expected: FAIL.
- [ ] **Step 3: Implement**; wire the CI step.
- [ ] **Step 4: Run** `cargo test --locked --workspace`, then by hand on the reference machine (Apple M-series laptop): `cargo run --locked --release -p swarm -- play examples/swarm/assets/swarm.scene.ron` (lobby, two players on the keyboard halves, a gamepad unplugged and plugged back, resize, F11, close with `--record` and `verify` the replay), and `cargo run --locked --release -p swarm -- play examples/swarm/assets/swarm.scene.ron --players 1 --no-vsync --ticks 1200 --stats`. Expected: PASS, and a 95th percentile frame time at or below 6.94 ms (144 frames per second), quoted in the commit message and the README.
- [ ] **Step 5: Commit** `✨ feat(cli): play a game in a native window with local players`.

---

### Task 8: `ludenea play`

**Files:**
- Modify: `crates/ludenea-serve/src/main.rs`, `crates/ludenea-project/template/src/lib.rs` (the template's mapping reads arrows, WASD and gamepad 0, like swarm's), README (the `ludenea` tool table and commands), `CHANGELOG.md`
- Test: `crates/ludenea-serve/tests/play.rs` (`#[ignore]`, run in CI with the project build tests)

**Interfaces:**
- Consumes: `Project::open`, `Project::build(BuildTarget::Native, ...)`, `BuildArtifact::Native { binary }`, Task 7.
- Produces: `Command::Play { project: PathBuf, #[arg(long)] scene: Option<PathBuf>, #[arg(last = true)] args: Vec<String> }`: builds natively, printing build lines and diagnostics to stderr; runs `<binary> play <scene> --project <root> <args...>` with inherited standard streams and returns its exit code. The scene defaults to the manifest's `default_scene`; without either: `error: no scene to play: pass --scene or set default_scene in ludenea.toml`, exit 2. A failed build exits 2 after its diagnostics.

- [ ] **Step 1: Write the failing tests**: `play_builds_the_template_and_forwards_arguments` (`ludenea play <new project> -- --help` prints the game's `play` help, exit 0), `play_without_a_scene_names_both_fixes`, `play_of_a_broken_build_exits_2_with_its_diagnostic`.
- [ ] **Step 2: Run** `cargo test -p ludenea-serve --test play -- --ignored`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-serve --test play -- --ignored && cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(serve): build a project natively and play it with ludenea play`.

---

### Task 9: Atlas files

**Files:**
- Modify: `crates/ludenea-render/src/{atlas.rs, web.rs}`, `crates/ludenea/tests/formats_documented.rs`, `CHANGELOG.md` (formats table: Atlas (`atlas.json`), `ATLAS_VERSION`, current 1, oldest readable 1)
- Test: `crates/ludenea-render/tests/atlas.rs`

**Interfaces:**
- Consumes: the 3a `Atlas`.
- Produces (`ludenea_render::atlas`):
  - `pub const ATLAS_VERSION: u32 = 1;`
  - `pub struct AtlasFiles { pub manifest: String, pub pages: Vec<Vec<u8>> }` (manifest: JSON `{ version, pages: [{ file, width, height }], regions: [{ id, frame, region }] }`; pages: PNG bytes, files `page-<n>.png`)
  - `Atlas::to_files(&self) -> AtlasFiles` (byte-for-byte reproducible); `Atlas::from_files(manifest: &str, pages: &[&[u8]]) -> Result<Atlas, AtlasFileError>`; `AtlasFileError::{Version { found: u32, supported: u32 }, Json(String), MissingPage { index: usize }, PageSize { index, expected: (u32, u32), found: (u32, u32) }, RegionOutside { id: AssetId, frame: u32, page: u32 }, Decode { index, reason: String }}`, version read first.
  - `WebRenderer::set_atlas(&mut self, manifest: &str, pages: Vec<Uint8Array>) -> Result<(), JsValue>`.

- [ ] **Step 1: Write the failing tests**: `atlas_round_trips_through_files`, `to_files_is_reproducible`, `newer_version_is_refused_as_a_version`, `missing_page_is_refused`, `page_of_another_size_is_refused_naming_both_sizes`, `region_outside_its_page_is_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-render --test atlas`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo xtask check`. Expected: PASS, the formats table test included.
- [ ] **Step 5: Commit** `✨ feat(render): write packed atlases to versioned files`.

---

### Task 10: `ludenea build --web`: the standalone page

**Files:**
- Create: `crates/ludenea-project/src/web.rs`, `crates/ludenea-project/web/{index.html, page.js, worker.js, style.css}`, `crates/ludenea-runtime/src/web.rs`, `editor/e2e/standalone.spec.ts`
- Modify: `crates/ludenea/Cargo.toml` (feature `web = ["ludenea-runtime/web"]`), `crates/ludenea-runtime/Cargo.toml`, `crates/ludenea-control/src/worker.rs` (`tickProgress`), `crates/ludenea-project/template/Cargo.toml` (`[features] web = ["ludenea/web"]`), `examples/swarm/Cargo.toml` (same feature), `crates/ludenea-serve/src/main.rs`, `editor/e2e/{vite.config.ts, playwright.config.ts}` (mount `/standalone`), `.github/workflows/determinism.yml` (editor job: `cargo build --locked -p swarm` and `ludenea build --web examples/swarm --out editor/e2e/fixtures/standalone` before the e2e run), README, `CHANGELOG.md`
- Test: `crates/ludenea-project/tests/web.rs` (`#[ignore]`, with the build tests), `editor/e2e/standalone.spec.ts`

**Interfaces:**
- Consumes: Tasks 2, 3, 4, 9; `ludenea_app::project::project_textures` (Task 7); `Project::build`'s cargo invocation, `wasm-bindgen-cli-support`.
- Produces (`ludenea_runtime::web`, wasm-bindgen): `WebLobby` (`new()`, `pressKey(code)`, `pressButton(gamepad, button)`, returning the `LobbyEvent` as JSON, `claimsJson()`, `started`), and `automaticClaimsJson(players: u32) -> String`.
- Produces (`WorkerHost`): `tickProgress() -> f64`.
- Produces (`ludenea_project`): `Project::build_web(&self, out: &Path, output: Sender<BuildEvent>) -> JoinHandle<Result<PathBuf, BuildError>>`: refuses a game without a `web` feature forwarding `ludenea/web` (`BuildError::NoWebFeature`, message naming the `[features] web = ["ludenea/web"]` line); builds the library for `wasm32-unknown-unknown` with `--features web` and binds it (`game.js`, `game_bg.wasm`); packs the project's textures with pages of 2048 and fails with every `TextureError` (`BuildError::Textures(Vec<String>)`); writes into `out`: `index.html`, `page.js`, `worker.js`, `style.css`, `game.js`, `game_bg.wasm`, `game.json` (`{ "version": 1, "name", "scene", "assets": [[id, text]] }`), `atlas/atlas.json`, `atlas/page-<n>.png`. `out` is replaced only after everything is written to a sibling staging folder.
- Produces (page): `worker.js` creates the `WorkerHost`, loads `game.json` (`load_scene`), then on `start` sends `set_players` and `play`, forwards `devices` as `set_live_input`, calls `advance` every 4 ms, and after each `ticked` posts `{ type: "frame", frame, tick, hash, progress }`; it stops at `?ticks=N`. `page.js` loads the same `game.js` for `WebRenderer` and `WebLobby`, captures keys and gamepads, runs the lobby in the DOM (four slots naming their devices) or skips it on `?players=N`, calls `set_frames` on each frame and `draw(alpha)` on each animation frame with `alpha = progress + (now - arrival) * tick_rate` clamped to 0..1, sets `document.body.dataset.tick` and `.hash`, offers "Save replay" (`export_replay`, downloaded as `<name>.replay`), and shows failures and a page opened from `file://` (no worker) in `#status`.
- Produces (`ludenea` tool): `Command::Build { project: PathBuf, #[arg(long)] web: bool (required), #[arg(long)] out: PathBuf }`, exit 0 or 2.

- [ ] **Step 1: Write the failing tests**: `web.rs`: `web_build_writes_a_complete_page` (every file listed above), `web_build_without_the_feature_names_the_line_to_add` (Review Focus), `broken_texture_fails_the_web_build_listing_it` (Review Focus), `web_build_is_reproducible_but_for_the_wasm` (atlas and `game.json` bytes equal across two builds); `standalone.spec.ts`: `draws_the_swarm`, `players_query_matches_headless_hashes` (`?players=1&ticks=300`: the page's hash at tick 300 equals `swarm run <scene> --ticks 300 --hash`), `lobby_claims_two_keyboard_halves_and_starts`, `save_replay_downloads_a_replay_that_verifies`, `file_url_says_to_serve_over_http` (Review Focus).
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test web -- --ignored`. Expected: FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-project --test web -- --ignored && cargo test --locked --workspace && cargo build --locked -p swarm -p ludenea-serve && cargo run --locked -p ludenea-serve -- build examples/swarm --web --out editor/e2e/fixtures/standalone && pnpm -C editor e2e standalone.spec.ts`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(serve): build a game as a standalone web page`.
