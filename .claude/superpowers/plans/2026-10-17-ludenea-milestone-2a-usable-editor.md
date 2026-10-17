# Ludenea Milestone 2a: Usable Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One editor, on desktop (Tauri) and in the browser (`ludenea serve`), that creates and opens Ludenea projects, explores and edits the ECS, plays, pauses, steps, and seeks a timeline whose edits are recorded as inputs, with undo and redo, React plugins built by an embedded Rust bundler, and editor sessions the CLI replays to the same trace.

**Architecture:** The Rust side grows field schemas, an edit log, a session timeline with logarithmic checkpoints, editor hooks (debug draw, picking, raw input mapping), and protocol version 2, served by the game's own binary over stdio (desktop) or by the game compiled to WebAssembly in a Web Worker (browser). A new project service owns files, metas, cargo builds, and plugin bundles, embedded in the Tauri backend and in the `ludenea` tool (`new`, `serve`). The front end is a pnpm workspace: `control-client`, `editor-shell`, `editor-builtins`, and two apps, with every built-in panel written as a plugin on the public API.

**Tech Stack:** Rust 1.97.1, serde, serde_json, ts-rs, wasm-bindgen and wasm-bindgen-cli-support (pinned to one exact version), wgpu, rolldown (embedded), axum and tokio (WebSocket server), notify (file watching), uuid, cargo_metadata, Tauri 2; TypeScript strict, React 19, Vite, pnpm, Vitest, Playwright, dockview, Zustand, @tanstack/react-virtual.

**Spec:** `.claude/superpowers/specs/2026-10-17-ludenea-editor-2a-design.md` (parent: `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md`). The milestone 1 plan `.claude/superpowers/plans/2026-10-17-ludenea-milestone-1-deterministic-core.md` describes the existing code.

## Global Constraints

- Every Global Constraint of the milestone 1 plan still applies to Rust code, in particular the determinism rules for simulation crates (`ludenea-math`, `-schema`, `-ecs`, `-app`, `-physics`, `-debug-draw` without its `render` feature, and game code).
- Code outside the simulation (`ludenea-control` runner and pacer, `ludenea-project`, `ludenea-serve`, the Tauri backend) may use wall clocks, `HashMap`, and float `min`/`max`; `clippy.toml` applies workspace-wide, so each such use carries `#[allow(clippy::disallowed_methods)]` or `#[allow(clippy::disallowed_types)]` on the smallest enclosing item.
- Toolchain: add target `wasm32-unknown-unknown` to `rust-toolchain.toml`.
- `wasm-bindgen` and `wasm-bindgen-cli-support` are pinned to the same exact version (`=x.y.z`) in `[workspace.dependencies]`; a mismatch breaks the browser build.
- Protocol: `PROTOCOL_VERSION = 2`. Component and resource values on the wire are JSON values in serde's externally tagged form (`Codec::to_json`). `u64` hashes stay 16-digit hex strings (`Hash64`); `Bytes` stays base64.
- Users never need Node.js: no runtime path (project service, `ludenea` tool, desktop app) may spawn `node`, `npm`, `npx`, or `pnpm`. Node and pnpm are development tools for building the editor itself. `pnpm-lock.yaml` is committed and CI uses `pnpm install --frozen-lockfile`.
- Front end: TypeScript `strict: true`, ES modules only, React function components. Tests: Vitest (units), Playwright (end-to-end, Chromium).
- Shared runtime modules for plugins: `react`, `react/jsx-runtime`, `@ludenea/editor-shell`, `@ludenea/control-client`. The editor declares a static import map for them in its `index.html`, its own build marks them external, and Vite emits them as stable entry files `shared/<name>.js`, so the editor and every plugin share one instance.
- Generated files are committed and checked in CI for staleness: protocol TypeScript (`editor/packages/control-client/src/generated/`), the embedded `.d.ts` bundles (`crates/ludenea-project/types/`).
- Stable names: unchanged from milestone 1. Schema type names are `module_path!()::Type`.
- Commits: gitmoji plus Conventional Commits, no internal plan references, no AI attribution trailers.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and (from Task 11 on) `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- TypeScript types for engine and game components both come from the schema generator (Task 1), not from `ts-rs`; `ts-rs` covers protocol types only (Task 9). One generator, one source of truth.
- The edit log is folded into the trace's `inputs_hash` (xxh3 of the input log bytes followed by the edit log bytes when an edit log is given), so the trace format and the golden trace stay unchanged.
- `ListEntities` is replaced by `QueryEntities` (one way to list entities).
- Recorded edits store whole component or resource values in canonical postcard bytes; field paths exist only in the protocol (`SetField`), which the host turns into whole values.
- Resources whose value has no JSON form (maps with non-string keys, such as `ludenea::ContactCache`) are shown read-only as RON in the inspector.

## Review Focus

- **A system panics during a tick** (game bug). The host catches it, restores the last good state (nearest checkpoint, then replay), pauses with `PauseReason::Panicked { tick, message }`, and the session is kept. Test in Task 7.
- **A field edit with an invalid value** (non-unit rotation on `Pose`, a negative radius, a string where a number goes) fails with `InvalidEdit` naming the component and field path, records nothing, and leaves the preview unchanged. Test in Task 7.
- **An edit targeting an entity that is not alive at the current tick** (selection kept after seeking back before its spawn) fails with `NoSuchEntity` and records nothing; the inspector clears its selection. Tests in Tasks 7 and 19.
- **A scene file changed on disk while open with unsaved edits in the editor**: the editor keeps its version and shows a "changed on disk" banner with Reload and Keep mine; without unsaved edits it reloads silently and the session resimulates. Tests in Tasks 13 and 18.
- **A file moved or renamed outside the editor** without its `.meta`: the moved file gets a new identifier, the orphan meta is reported once as a console warning naming its identifier and old path, and nothing is deleted automatically. Test in Task 13.

---

### Task 1: Field schemas

**Files:**
- Create: `crates/ludenea-schema/{Cargo.toml, src/lib.rs, src/typescript.rs}`
- Modify: `crates/ludenea-macros/src/lib.rs` (`derive(Schema)`; `derive(Component)` and `derive(Resource)` also implement `Schema`), `crates/ludenea-ecs/src/{component.rs, world.rs, entity.rs, lib.rs}`, `crates/ludenea-math/src/{vec2.rs, rot.rs, transform.rs, aabb.rs}`, `crates/ludenea-physics/src/{components.rs, shape.rs, contacts.rs}`, `crates/ludenea-app/src/{time.rs, rng.rs, input.rs}`, `examples/boxes/src/lib.rs`, facade (`pub use ludenea_schema as schema;`, prelude `Schema`), workspace `Cargo.toml`
- Test: `crates/ludenea-schema/src/lib.rs` (inline), `crates/ludenea-ecs/tests/schema.rs`

**Interfaces:**
- Produces (`ludenea_schema`), all types `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]`:
  - `pub trait Schema { fn schema() -> TypeSchema; }`, implemented for `bool`, `u8..u64`, `i8..i64`, `f32`, `f64`, `String`, `Option<T>`, `Vec<T>`, `[T; N]`, `BTreeMap<K, V>`, `BTreeSet<T>`, tuples up to 4, `()`.
  - `#[serde(tag = "kind", rename_all = "snake_case")] pub enum TypeSchema { Bool, U8, U16, U32, U64, I8, I16, I32, I64, F32, F64, String, Unit, Option { item: Box<TypeSchema> }, List { item: Box<TypeSchema> }, Array { item: Box<TypeSchema>, len: u32 }, Map { key: Box<TypeSchema>, value: Box<TypeSchema> }, Tuple { items: Vec<TypeSchema> }, Struct { name: String, fields: Vec<FieldSchema> }, TupleStruct { name: String, items: Vec<TypeSchema> }, UnitStruct { name: String }, Enum { name: String, variants: Vec<VariantSchema> }, Entity, AssetId }`
  - `pub struct FieldSchema { pub name: String, pub schema: TypeSchema }`, `pub struct VariantSchema { pub name: String, pub fields: VariantFields }`, `#[serde(tag = "kind", rename_all = "snake_case")] pub enum VariantFields { Unit, Tuple { items: Vec<TypeSchema> }, Struct { fields: Vec<FieldSchema> } }`.
  - `pub fn typescript_declarations(types: &[(&str, &TypeSchema)]) -> String`: one `export type` per named struct or enum reachable from the given types (named by last path segment; on a collision, by the full path with `::` replaced by `_`), plus `export interface ComponentTypes { "<stable name>": <Type>; ... }` for the given pairs. Externally tagged enums map to `"Unit" | { Tuple: [..] } | { Struct: {..} }`; `u64`/`i64` map to `number` (serde_json emits numbers); `Entity` maps to `{ index: number; generation: number }`; `AssetId` to `string`.
- Produces (`ludenea_macros`): `#[derive(Schema)]` for structs (named, tuple, unit) and enums. It reads only `#[serde(...)]` container attributes `try_from`, `from`, `into` (accepted, documented: the mirror type must have identical fields) and fails to compile, naming the attribute, on `rename`, `rename_all`, `tag`, `content`, `untagged`, `flatten`, `skip`, `skip_serializing`, `skip_deserializing`, `default`, `with`, so a schema always matches the serde shape. Recursive types are not supported (stack overflow is documented on the trait).
- Produces (`ludenea_ecs`): `pub trait Component: Schema + ...` and `pub trait Resource: Schema + ...` (the derives implement both); `impl Schema for Entity` (`TypeSchema::Entity`); `#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)] pub struct AssetId(pub u128)` in `asset.rs`, serialized as 32 lowercase hex digits, `impl Schema` (`TypeSchema::AssetId`); `Codec` gains `pub schema: fn() -> TypeSchema`; `World::component_schemas(&self) -> Vec<(&'static str, TypeSchema)>` and `World::resource_schemas(&self) -> Vec<(&'static str, TypeSchema)>`, sorted by stable name.
- Produces (`ludenea_app`): `pub trait Input: Schema + ...`; `Simulation::input_schema(&self) -> Option<TypeSchema>`.

- [ ] **Step 1: Write the failing tests**: `struct_schema_lists_fields_in_declaration_order`, `enum_schema_covers_unit_tuple_and_struct_variants`, `containers_map_to_option_list_array_map_tuple` (`[Vec2; 8]` gives `Array { len: 8 }`), `component_derive_also_derives_schema` (`Pose` schema is a struct with fields `position` and `rotation`), `world_lists_component_schemas_sorted_by_stable_name`, `typescript_declarations_for_boxes_components` (contains `export type Shape =` and `"ludenea::Pose": Pose`), `typescript_names_collide_to_full_paths`; a `compile_fail` doc test on `derive(Schema)` with `#[serde(rename_all = "camelCase")]`.
- [ ] **Step 2: Run** `cargo test -p ludenea-schema -p ludenea-ecs`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate, the derive, the supertraits, and `#[derive(Schema)]` on every nested engine type (`Vec2`, `Rot`, `Transform`, `Aabb`, `BodyKind`, `Shape`, `Circle`, `Capsule`, `Polygon`, `CachedContact`, and the `boxes` input and components).
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS, golden tests included (schemas change no encoding).
- [ ] **Step 5: Commit** `✨ feat(schema): derive field schemas for components, resources, and inputs`.

---

### Task 2: JSON values and the edit log

**Files:**
- Create: `crates/ludenea-app/src/edit.rs`
- Modify: `crates/ludenea-ecs/src/{component.rs, world.rs}` (JSON codec, removal by name), `crates/ludenea-app/src/lib.rs`, facade prelude (`Edit`, `EditLog`, `TimedEdit`)
- Test: `crates/ludenea-app/tests/edit.rs`

**Interfaces:**
- Consumes: Task 1 `Codec`.
- Produces (`ludenea_ecs`): `Codec` gains `pub to_json: fn(&dyn Any) -> Result<serde_json::Value, String>` and `pub from_json: fn(serde_json::Value) -> DecodeResult` (through the concrete type, so validation such as `Pose`'s `try_from` runs). `World::component_json(&self, entity: Entity, name: &str) -> Option<Result<serde_json::Value, String>>`, `World::entity_components_json(&self, entity: Entity) -> Option<BTreeMap<String, Result<serde_json::Value, String>>>`, `World::resource_json(&self, name: &str) -> Option<Result<serde_json::Value, String>>`, `World::remove_by_name(&mut self, entity: Entity, name: &str) -> Result<(), EcsError>`.
- Produces (`ludenea_app::edit`), all `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]`:
  - `pub enum Edit { Spawn { components: Vec<(String, Vec<u8>)> }, Despawn { entity: Entity }, Insert { entity: Entity, component: String, value: Vec<u8> }, Remove { entity: Entity, component: String }, SetResource { resource: String, value: Vec<u8> } }`. Values are postcard bytes of the concrete type (`Codec::encode`), so the log is canonical. `Insert` inserts or replaces.
  - `pub struct TimedEdit { pub tick: u64, pub edit: Edit }`: made while the simulation was at `tick`, applied before tick `tick + 1` runs.
  - `pub const EDIT_LOG_VERSION: u32 = 1;` `pub struct EditLog { pub version: u32, pub edits: Vec<TimedEdit> }` (ordered by tick, then record order), `EditLog::new() -> Self`, `EditLog::encode(&self) -> Vec<u8>` (postcard), `EditLog::decode(bytes: &[u8]) -> Result<EditLog, SimError>` (rejects other versions with `SimError::EditLogVersion { found, expected }` and trailing bytes), `EditLog::at(&self, tick: u64) -> &[TimedEdit]`.
  - `pub enum EditError` (`thiserror`): `NoSuchEntity(Entity)`, `UnknownComponent(String)`, `UnknownResource(String)`, `InvalidValue { name: String, reason: String }`; messages say what to do.
  - `Simulation::apply_edit(&mut self, edit: &Edit) -> Result<Option<Entity>, EditError>`: atomic (validate everything, then mutate); returns the spawned entity for `Spawn`.
  - `Simulation::encode_json_value(&self, kind: ValueKind, name: &str, value: serde_json::Value) -> Result<Vec<u8>, EditError>` with `pub enum ValueKind { Component, Resource }`: JSON to canonical bytes through the codec.

- [ ] **Step 1: Write the failing tests**: `insert_replaces_existing_component`, `spawn_returns_next_free_entity_deterministically`, `despawn_of_dead_entity_is_no_such_entity`, `invalid_pose_rotation_is_rejected_and_world_unchanged` (hash before equals hash after), `unknown_component_name_is_rejected`, `edit_log_round_trips_and_rejects_other_versions`, `edit_log_at_returns_edits_of_one_tick_in_record_order`, `json_round_trip_is_exact_for_f32_fields` (encode `Pose` to JSON and back, bytes equal), `contact_cache_has_no_json_form_but_ron_works`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test edit`. Expected: compile failure.
- [ ] **Step 3: Implement** the codec additions, `remove_by_name`, and `edit.rs`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): record editor edits as a canonical edit log`.

---

### Task 3: Session timeline with logarithmic checkpoints

**Files:**
- Create: `crates/ludenea-control/src/{checkpoints.rs, session.rs}`
- Test: inline tests in `checkpoints.rs`, `crates/ludenea-control/tests/session.rs`

**Interfaces:**
- Consumes: Task 2 `EditLog`, `Simulation::apply_edit`; milestone 1 `InputLog`, `Snapshot`.
- Produces (`ludenea_control::checkpoints`):
  - `pub const CHECKPOINT_INTERVAL: u64 = 16;` `pub const CHECKPOINTS_PER_LEVEL: usize = 2;`
  - `pub struct Checkpoints`: `new(initial: Snapshot) -> Self` (tick 0, never dropped), `wants(&self, tick: u64) -> bool` (`tick % CHECKPOINT_INTERVAL == 0` and not stored), `insert(&mut self, tick: u64, snapshot: Snapshot)` then thins relative to the new head: level `k` covers distances `[INTERVAL * 2^k, INTERVAL * 2^(k+1))` from the head and keeps at most `CHECKPOINTS_PER_LEVEL` checkpoints; `nearest_at_or_before(&self, tick: u64) -> (u64, &Snapshot)`, `truncate_after(&mut self, tick: u64)`, `ticks(&self) -> Vec<u64>`.
- Produces (`ludenea_control::session`):
  - `pub struct Session` holding the scene RON, `InputLog`, `EditLog`, `hashes: Vec<u64>` (`hashes[t]` = hash after `t` ticks, before edits made at `t`), `Checkpoints`, and `head: u64` (last recorded tick).
  - `pub struct Timeline { pub head: u64, pub current: u64, pub checkpoints: Vec<u64>, pub edit_ticks: Vec<u64>, pub can_undo: bool, pub can_redo: bool }` (Serialize).
  - Methods taking `&mut Simulation` as their first argument, so `Host` owns both:
    - `Session::start(sim: &mut Simulation, scene: String) -> Result<Session, SceneError>`.
    - `step(&mut self, sim, live: Option<&[u8]>) -> Result<u64, SessionError>`: from `current < head`, applies recorded edits of `current`, then the recorded input (or `live` when it takes over, below); at `head`, records `live` or default inputs. Records the hash and a checkpoint when wanted. Returns the new tick.
    - **Take over:** while `current < head`, a `live` input counts only when the caller passes it (the host passes it only when the raw input is not idle, Task 6) and its bytes differ from the recorded input of `current`; then everything after `current` is discarded (inputs, edits, hashes, checkpoints) and recording continues live.
    - `edit(&mut self, sim, edit: Edit) -> Result<Option<Entity>, SessionError>`: at tick 0, applies to the world and replaces the scene with `sim.save_scene()`, keeps the recorded future and marks it stale (below); past tick 0, discards the future after `current`, records `TimedEdit { tick: current }`, applies it now (the preview).
    - `seek(&mut self, sim, tick: u64) -> Result<(), SessionError>`: `tick <= head`, restore the nearest checkpoint, replay inputs and edits up to `tick`, then apply the edits recorded at `tick` (preview). `tick > head` fails with `SessionError::InvalidTick { tick, head }`.
    - `is_pending(&self) -> bool`: edits recorded at `current` exist.
    - **Stale future:** after a tick 0 edit or `replace_scene(&mut self, sim, scene: String)` (scene file reloaded, game rebuilt), `resimulate(&mut self, sim, budget: u32) -> Resimulation` replays at most `budget` recorded ticks on the new state, comparing with the old hashes, and returns `pub enum Resimulation { Idle, Progress { up_to: u64 }, Diverged { tick: u64, expected: u64, got: u64 }, Done }`; `Diverged` is reported once, then resimulation continues and the new hashes replace the old. The simulation's visible tick stays where the user is (resimulation runs on restored snapshots and restores the current state when it yields).
    - `export(&self) -> (String, Vec<u8>, Vec<u8>)`: scene, input log bytes, edit log bytes.
    - `timeline(&self) -> Timeline`.
  - `pub enum SessionError` (`thiserror`): `InvalidTick { tick, head }`, `Edit(EditError)`, `Sim(SimError)`, `Scene(SceneError)`.

- [ ] **Step 1: Write the failing tests**: in `checkpoints.rs`, `tick_zero_is_always_kept`, `count_grows_logarithmically` (after 100,000 ticks, at most `CHECKPOINTS_PER_LEVEL * (log2(100000 / 16) + 1) + 1` checkpoints), `seek_cost_is_bounded_by_distance` (for every target `t` in a sample, `t - nearest_at_or_before(t).0 <= max(CHECKPOINT_INTERVAL, head - t)`); in `tests/session.rs` with the `boxes` app (dev-dependency): `replaying_recorded_ticks_matches_original_hashes`, `seek_back_then_forward_reproduces_hashes`, `edit_in_past_discards_future_and_records_edit`, `pending_edit_shows_in_world_before_next_tick` (`is_pending`, world hash differs from `hashes[current]`), `idle_replay_keeps_future`, `live_input_differing_from_recorded_takes_over`, `seek_beyond_head_is_invalid_tick`, `export_then_cli_style_replay_matches` (fresh `Simulation`, load exported scene, apply inputs and edits tick by tick, same hashes), `tick_zero_edit_resimulates_and_reports_first_divergence`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** `checkpoints.rs` and `session.rs`.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-control && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): add a session timeline with logarithmic checkpoints`.

---

### Task 4: Undo and redo

**Files:**
- Create: `crates/ludenea-control/src/undo.rs`
- Modify: `crates/ludenea-control/src/session.rs`
- Test: `crates/ludenea-control/tests/undo.rs`

**Interfaces:**
- Consumes: Task 3 `Session`.
- Produces:
  - `pub(crate) enum UndoEntry { Scene { before: String, after: String }, Run { edit: TimedEdit } }`; the session keeps an undo stack and a redo stack for its lifetime (not persisted); a new edit clears the redo stack.
  - `Session::undo(&mut self, sim) -> Result<(), SessionError>`: `Scene` restores `before` as the scene and marks the future stale; `Run` removes that edit from the log, discards the future after its tick, seeks to its tick. `SessionError::NothingToUndo` when empty.
  - `Session::redo(&mut self, sim) -> Result<(), SessionError>`: reapplies the entry (for `Run`, seeks to its tick, discards the future after it, records the edit again). `SessionError::NothingToRedo` when empty.
  - `Session::is_scene_dirty(&self) -> bool` and `Session::mark_scene_saved(&mut self)`: the scene differs from its last saved text (the unsaved marker; saving stays explicit).

- [ ] **Step 1: Write the failing tests**: `undo_run_edit_restores_exact_hash` (edit at tick 300, play to 400, undo: at tick 300 with `hashes[300]` and no pending edit, head 300), `redo_reapplies_same_edit_and_hash`, `undo_scene_edit_restores_scene_text`, `new_edit_clears_redo`, `undo_on_empty_stack_is_nothing_to_undo`, `scene_dirty_until_saved`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test undo`. Expected: compile failure.
- [ ] **Step 3: Implement** `undo.rs` and wire it into `Session::edit`.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-control && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): undo and redo edits by resimulating the timeline`.

---

### Task 5: Debug draw primitives and picking

**Files:**
- Create: `crates/ludenea-debug-draw/{Cargo.toml, src/lib.rs}`, `crates/ludenea-physics/src/editor.rs`
- Modify: `crates/ludenea-app/src/{app.rs, simulation.rs}` (editor hooks), `crates/ludenea-physics/src/{plugin.rs, lib.rs}`, facade (`pub use ludenea_debug_draw as debug_draw;`)
- Test: `crates/ludenea-physics/tests/editor.rs`

**Interfaces:**
- Produces (`ludenea_debug_draw`, no wgpu in this task), all `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]`:
  - `pub struct Color(pub u32)` (`0xRRGGBBAA`).
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum Primitive { Line { a: Vec2, b: Vec2, color: Color }, Circle { center: Vec2, radius: f32, color: Color, fill: bool }, Polygon { points: Vec<Vec2>, color: Color, fill: bool }, Capsule { a: Vec2, b: Vec2, radius: f32, color: Color }, Label { at: Vec2, text: String, color: Color } }`
  - `pub struct Item { pub entity: Option<Entity>, pub primitive: Primitive }`, `pub struct DebugDrawFrame { pub layers: BTreeMap<String, Vec<Item>> }` with `push(&mut self, layer: &str, entity: Option<Entity>, primitive: Primitive)`.
- Produces (`ludenea_app`): `App::debug_draw(&mut self, layers: &[&'static str], draw: fn(&World, &BTreeSet<String>, &mut DebugDrawFrame)) -> &mut Self`, `App::picker(&mut self, pick: fn(&World, Vec2) -> Vec<Entity>) -> &mut Self`; `Simulation::debug_layers(&self) -> Vec<&'static str>` (sorted), `Simulation::debug_draw(&self, layers: &BTreeSet<String>) -> DebugDrawFrame` (calls every drawer, which skips unrequested layers), `Simulation::pick(&self, point: Vec2) -> Vec<Entity>` (union of all pickers, ascending, no duplicates).
- Produces (`ludenea_physics`): layers `"physics.colliders"`, `"physics.contacts"`, `"physics.aabbs"`; colliders drawn as outlines in their world pose (static gray, kinematic blue, dynamic green), contacts as points and normals, AABBs as rectangles. Picker: entities whose collider contains `point` (reuse `overlap_aabb` on a 1 cm box, then an exact point test), plus entities with `Pose` and no `Collider` within `PICK_RADIUS = 0.25` m.

- [ ] **Step 1: Write the failing tests**: `colliders_layer_draws_one_item_per_collider_with_entity`, `unrequested_layers_are_empty`, `pick_inside_box_returns_it`, `pick_outside_everything_returns_empty`, `pick_collider_less_pose_within_radius`, `debug_draw_and_pick_do_not_change_world_hash`.
- [ ] **Step 2: Run** `cargo test -p ludenea-physics --test editor`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate, the hooks, and `physics/src/editor.rs`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(physics): draw colliders, contacts, and bounds, and pick bodies`.

---

### Task 6: Raw input mapping and the real-time pacer

**Files:**
- Create: `crates/ludenea-app/src/raw_input.rs`, `crates/ludenea-control/src/pacer.rs`
- Modify: `crates/ludenea-app/src/{app.rs, simulation.rs, lib.rs}`, `examples/boxes/src/lib.rs` (mapping: arrow keys give `velocity_change`, `Space` spawns at x = 0, `Delete` despawns)
- Test: `crates/ludenea-app/tests/raw_input.rs`, inline tests in `pacer.rs`

**Interfaces:**
- Produces (`ludenea_app`), `#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]`:
  - `pub struct RawInput { pub keys: BTreeSet<String>, pub gamepads: Vec<RawGamepad> }` (`keys` are `KeyboardEvent.code` values), `pub struct RawGamepad { pub buttons: Vec<f32>, pub axes: Vec<f32> }` (Gamepad API standard mapping).
  - `RawInput::is_idle(&self) -> bool`: no key, no button above 0.5, no axis beyond 0.2 in absolute value.
  - `App::input_mapping<I: Input>(&mut self, map: fn(&RawInput, usize) -> I) -> &mut Self` (the `usize` is the player index); registering a mapping for a type other than the app's input type is an `AppError` at build.
  - `Simulation::map_raw_input(&self, players: &[RawInput]) -> Result<Vec<u8>, SimError>`: the encoded `Vec<I>`, one per player; `SimError::NoInputMapping` without a mapping (message: register one with `App::input_mapping`).
- Produces (`ludenea_control::pacer`): `pub const MAX_TICKS_PER_FRAME: u32 = 4;` `pub struct Pacer` with `new(tick_rate: u32) -> Self`, `set_speed(&mut self, speed: f32)` (clamped to `[0.1, 8.0]`), `advance(&mut self, elapsed_seconds: f64) -> u32`: ticks due, at most `MAX_TICKS_PER_FRAME`; debt beyond that is dropped, so a slow simulation runs slower than real time and never skips ticks. `effective_rate(&self) -> f32`: ticks per second over the last second.

- [ ] **Step 1: Write the failing tests**: `boxes_mapping_turns_arrow_keys_into_velocity_change`, `idle_raw_input_maps_to_default`, `map_without_mapping_is_no_input_mapping`, `is_idle_ignores_small_axes`; `pacer_runs_one_tick_per_sixtieth_second`, `pacer_caps_ticks_and_drops_debt`, `speed_two_doubles_ticks`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** `raw_input.rs`, the app hook, and `pacer.rs`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): map raw keyboard and gamepad state to game inputs`.

---

### Task 7: Protocol version 2: editing, running, and the timeline

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, in_process.rs, lib.rs}`, `crates/ludenea-cli/src/commands.rs` (follow the renamed commands)
- Create: `crates/ludenea-control/src/path.rs`
- Test: `crates/ludenea-control/tests/{host.rs, editing.rs}`

**Interfaces:**
- Consumes: Tasks 2 to 6.
- Produces (`ludenea_control`), same serde conventions as milestone 1, `JsonValue` = `serde_json::Value`:
  - `PROTOCOL_VERSION = 2`. `ServerMessage::Welcome` gains `game: GameInfo`; new variant `ServerMessage::Event { event: Event }`.
  - `pub struct GameInfo { pub components: Vec<NamedSchema>, pub resources: Vec<NamedSchema>, pub input: Option<TypeSchema>, pub tick_rate: u32, pub debug_layers: Vec<String> }`, `pub struct NamedSchema { pub name: String, pub schema: TypeSchema }`.
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum PathSegment { Field { name: String }, Index { index: u32 } }`; `pub fn set_at_path(root: &mut JsonValue, path: &[PathSegment], value: JsonValue) -> Result<(), String>` in `path.rs` (an enum variant's content is the object key of its name).
  - `pub enum FieldTarget { Component { entity: Entity, component: String }, Resource { resource: String } }`.
  - New commands: `LoadEdits { log: Bytes }`, `GetEntity { entity }` now replies `Entity { entity, components: BTreeMap<String, ComponentValue> }` with `#[serde(tag = "type", rename_all = "snake_case")] pub enum ComponentValue { Json { value: JsonValue }, Ron { text: String } }` (RON when no JSON form), `GetResources` → `Resources { resources: BTreeMap<String, ComponentValue> }`, `Spawn { components: BTreeMap<String, JsonValue> }` → `Spawned { entity }`, `Despawn { entity }`, `InsertComponent { entity, component: String, value: JsonValue }`, `RemoveComponent { entity, component: String }`, `SetField { target: FieldTarget, path: Vec<PathSegment>, value: JsonValue }`, `SaveScene` → `Scene { ron: String }` (and marks it saved), `Play`, `Pause`, `Seek { tick: u64 }`, `SetSpeed { speed: f32 }`, `SetLiveInput { players: Vec<RawInput> }`, `Undo`, `Redo`, `GetTimeline` → `Timeline(Timeline)`, `ExportSession` → `Session { scene: String, inputs: Bytes, edits: Bytes, hashes: Vec<Hash64> }`, `ReplaceScene { ron: String }` (scene file changed on disk: `Session::replace_scene`, the recorded future is kept and resimulated), `LoadSession { scene: String, inputs: Bytes, edits: Bytes, expected: Vec<Hash64> }` (a restarted host after a rebuild: starts at tick 0 with the recorded future marked stale against `expected`, so resimulation reports `Diverged`; backed by `Session::resume(sim, scene, inputs, edits, expected) -> Result<Session, SessionError>` added in this task). `GetHash` replies `Hash { tick, hash, pending: bool }`.
  - New `ErrorCode` variants: `InvalidEdit`, `UnknownComponent`, `UnknownResource`, `InvalidPath`, `InvalidTick`, `NothingToUndo`, `NothingToRedo`, `NoInputMapping`, `SystemPanicked`. Every error message names the entity, component, and path involved.
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum Event { Ticked { tick: u64, hash: Hash64, playing: bool, effective_rate: f32 }, Paused { reason: PauseReason }, TimelineChanged { timeline: Timeline }, Resimulated { up_to: u64 }, Diverged { tick: u64, expected: Hash64, got: Hash64 } }` (Task 8 adds more), `pub enum PauseReason { User, Panicked { tick: u64, message: String } }`.
  - `Host` now owns `Simulation` plus `Session`. `Host::handle(&mut self, command: Command) -> Result<Reply, ProtocolError>` stays synchronous; `Host::advance(&mut self, elapsed_seconds: f64) -> Vec<Event>` runs due ticks while playing (live input is the mapped `SetLiveInput` value, passed to the session only when not idle), or a resimulation budget of 240 ticks while paused, and returns the events. Every tick runs under `std::panic::catch_unwind(AssertUnwindSafe(..))`; on a panic the host seeks to the last good tick and pauses with `PauseReason::Panicked`.
  - `SetField` reads the current JSON value, applies `set_at_path`, encodes through `Simulation::encode_json_value`, and records an `Insert` or `SetResource` edit.
  - The CLI keeps working (`ListEntities` usages move to `QueryEntities` in Task 8; until then the CLI does not use it).

- [ ] **Step 1: Write the failing tests** in `editing.rs` (boxes app): `set_field_on_pose_position_records_insert_edit`, `set_field_with_non_unit_rotation_is_invalid_edit_and_records_nothing` (message contains `ludenea::Pose` and `rotation`), `set_field_on_dead_entity_is_no_such_entity`, `set_field_bad_path_is_invalid_path`, `spawn_from_json_components_returns_entity`, `save_scene_round_trips_through_load_scene`, `play_then_advance_runs_ticks_and_emits_ticked`, `panicking_system_pauses_with_panicked_and_keeps_session` (test app with a system that panics at tick 50: `Paused { reason: Panicked { tick: 50, .. } }`, current tick 49, `GetTimeline` head 49), `undo_and_redo_through_protocol`, `export_session_replays_in_fresh_host`, `load_session_with_changed_code_reports_diverged` (expected hashes with one value flipped at tick 30: `Diverged { tick: 30, .. }`), `replace_scene_keeps_recorded_inputs`; update `host.rs` for version 2 (`handshake_with_version_1_is_refused`, every new command and reply round-trips through JSON).
- [ ] **Step 2: Run** `cargo test -p ludenea-control`. Expected: FAIL.
- [ ] **Step 3: Implement** the protocol additions, `path.rs`, and the host on top of `Session`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS (golden tests unchanged).
- [ ] **Step 5: Commit** `✨ feat(control): edit, play, seek, and undo through protocol version 2`.

---

### Task 8: ECS introspection and subscriptions

**Files:**
- Create: `crates/ludenea-ecs/src/{filter.rs, observer.rs}`, `crates/ludenea-control/src/subscriptions.rs`
- Modify: `crates/ludenea-ecs/src/{schedule.rs, system.rs, commands.rs, world.rs, lib.rs}`, `crates/ludenea-app/src/{app.rs, simulation.rs}` (`event::<T>` requires `T: Serialize`), `crates/ludenea-physics/src/events.rs` (`Collision` derives `Serialize`), `crates/ludenea-control/src/{protocol.rs, host.rs}`, `crates/ludenea-cli/src/commands.rs`
- Test: `crates/ludenea-ecs/tests/{filter.rs, observer.rs}`, `crates/ludenea-control/tests/introspection.rs`

**Interfaces:**
- Produces (`ludenea_ecs`):
  - `pub struct EntityFilter`; `EntityFilter::parse(text: &str, world: &World) -> Result<EntityFilter, FilterError>`: comma-separated `With<Name>` and `Without<Name>`, where `Name` is a stable name or its last segment when unique; empty text matches every entity. `FilterError` names the unknown or ambiguous component and lists candidates. `World::filter_entities(&self, filter: &EntityFilter) -> Vec<Entity>` (ascending).
  - `pub struct SystemInfo { pub name: &'static str, pub reads: Vec<(&'static str, &'static str)>, pub writes: Vec<(&'static str, &'static str)> }` (kind, name); `Schedule::systems(&self, world: &World) -> Vec<SystemInfo>` in run order.
  - `pub trait ScheduleObserver { fn before_system(&mut self, index: usize); fn after_system(&mut self, index: usize, commands: &[CommandRecord]); fn tick_end(&mut self, world: &World); }`, `pub struct CommandRecord { pub kind: CommandKind, pub entity: Option<Entity>, pub component: Option<&'static str> }`, `pub enum CommandKind { Spawn, Despawn, Insert, Remove }`. `Schedule::run_observed(&mut self, world: &mut World, observer: &mut dyn ScheduleObserver)`; plain `run` behaves as before. Records are built only when observed.
  - `World::events_json(&self) -> Vec<(&'static str, Vec<serde_json::Value>)>` (event type name, events of this tick).
- Produces (`ludenea_app`): `Simulation::step_observed(&mut self, inputs: Option<&[u8]>, observer: &mut dyn ScheduleObserver) -> Result<(), SimError>`; `Simulation::systems(&self) -> Vec<SystemInfo>`.
- Produces (`ludenea_control`):
  - Commands: `QueryEntities { filter: String, offset: u32, limit: u32 }` → `Entities { total: u32, rows: Vec<EntityRow> }` with `pub struct EntityRow { pub entity: Entity, pub hash: Hash64 }` (replaces `ListEntities`); `GetSchedule` → `Schedule { systems: Vec<SystemRow> }` with `pub struct SystemRow { pub name: String, pub reads: Vec<String>, pub writes: Vec<String>, pub micros: Option<f32> }` (`"kind name"` strings; `micros` = mean over the last 60 observed ticks, measured with `Instant` in the host); `GetTickEvents { tick }` → `TickEvents { events: Vec<RecordedEvent> }` with `pub struct RecordedEvent { pub event_type: String, pub value: JsonValue }`; `GetTickCommands { tick }` → `TickCommands { sync_points: Vec<SyncPoint> }` with `pub struct SyncPoint { pub after_system: String, pub commands: Vec<String> }` (for example `"insert ludenea::Body on 12v0"`); `GetDebugDraw { layers: Vec<String> }` → `DebugDraw { tick, frame: DebugDrawFrame }`; `Pick { point: Vec2 }` → `Picked { entities: Vec<Entity> }`; `Subscribe { topic: Topic }`, `Unsubscribe { topic: Topic }`.
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum Topic { Entities { filter: String }, Entity { entity: Entity }, Resources, Introspection }`. `Introspection` turns on the observer and keeps events, commands, and timings for the last `INTROSPECTION_TICKS = 600` ticks; `GetTickEvents` and `GetTickCommands` outside that window fail with `InvalidTick`.
  - New `ErrorCode` variant: `InvalidFilter` (message from `FilterError`, with candidates).
  - New events: `EntitiesChanged { filter: String, total: u32 }`, `EntityChanged { entity, components: BTreeMap<String, ComponentValue>, removed: Vec<String> }` (only components whose hash changed since the last push), `EntityDespawned { entity }`, `ResourcesChanged { resources: BTreeMap<String, ComponentValue> }`. Deltas are computed at most once per `Host::advance` call, never per tick, from per-component hashes.

- [ ] **Step 1: Write the failing tests**: `filter_with_and_without_by_short_and_full_name`, `ambiguous_short_name_lists_candidates`, `empty_filter_matches_all`; `observer_sees_systems_in_order_and_their_commands`, `unobserved_run_hashes_equal_observed_run_hashes` (600 boxes ticks); `query_entities_pages_and_reports_total`, `schedule_lists_accesses_and_timings_after_introspection`, `tick_events_contain_collision_begin`, `tick_commands_name_spawn_after_apply_inputs`, `entity_subscription_pushes_only_changed_components`, `debug_draw_and_pick_through_protocol`.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** filters, the observer, the commands, and the subscriptions.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): explore entities, systems, events, and commands per tick`.

---

### Task 9: TypeScript protocol types

**Files:**
- Modify: `crates/ludenea-control/{Cargo.toml, src/protocol.rs}`, `crates/ludenea-schema`, `crates/ludenea-ecs` (`Entity`, `AssetId`, `HashTree`, `NamedHash`, `EntityHash`), `crates/ludenea-debug-draw`, `crates/ludenea-app` (`RawInput`, `RawGamepad`), `crates/ludenea-math` (`Vec2`): each behind an optional `ts` feature enabling `ts_rs::TS` derives; `ludenea-control` enables them
- Create: `editor/packages/control-client/src/generated/` (generated), `crates/ludenea-control/tests/typescript.rs`

**Interfaces:**
- Produces: one `.ts` file per exported type under `editor/packages/control-client/src/generated/`, plus `index.ts` re-exporting them. `Hash64` and `Bytes` export as `string`, `JsonValue` as ts-rs's `JsonValue`, `u64` fields in protocol types as `number` except hashes.
- `cargo test -p ludenea-control --test typescript` regenerates the directory (`TS_RS_EXPORT_DIR` set in `.cargo/config.toml` to the absolute path through `[env]` with `relative = true`).

- [ ] **Step 1: Write the test** `export_typescript_bindings`: exports every protocol type, then asserts that `ClientMessage.ts`, `ServerMessage.ts`, `Command.ts`, `Event.ts`, and `TypeSchema.ts` exist and that `Command.ts` contains `"type": "set_field"`.
- [ ] **Step 2: Run** it. Expected: compile failure (no derives).
- [ ] **Step 3: Implement** the derives and the `index.ts` writer.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-control --test typescript && git status --porcelain editor/packages/control-client/src/generated`. Expected: PASS, files listed as new.
- [ ] **Step 5: Commit** `✨ feat(control): generate TypeScript types for the protocol`.

---

### Task 10: Native game host over stdio, CLI edit logs

**Files:**
- Create: `crates/ludenea-control/src/stdio.rs` (native only)
- Modify: `crates/ludenea-cli/src/{lib.rs, commands.rs}`
- Test: `crates/ludenea-control/tests/stdio.rs`, `crates/ludenea-cli/tests/cli.rs`

**Interfaces:**
- Produces (`ludenea_control::stdio`): `pub fn serve_stdio(app: fn() -> App, input: impl BufRead + Send + 'static, output: impl Write) -> std::io::Result<()>`: a reader thread parses one `ClientMessage` JSON per line into a channel; the main loop waits at most 1/60 s for messages, handles them, calls `Host::advance` with the measured elapsed time, and writes one `ServerMessage` JSON per line, flushing after each batch. A malformed line produces `Refused` with code `NotConnected` before the handshake and a `Response` error afterwards only when an id can be read; it never panics. End of input ends the loop with `Ok`.
- Produces (`ludenea_cli`): subcommand `host --stdio` (calls `serve_stdio` on stdin and stdout); `run` and `verify` accept `--edits <file>`; when given, `inputs_hash` = xxh3 of the input log bytes followed by the edit log bytes (Deviations above).

- [ ] **Step 1: Write the failing tests**: `stdio_handshake_and_step_over_pipes` (in-memory pipes, `Hello` then `Step`), `malformed_line_gets_error_not_panic`, `playing_emits_ticked_events_in_real_time` (play for 0.5 s: between 20 and 40 ticks); in `cli.rs`: `verify_with_edits_replays_exported_session` (build a session through `InProcess`, export, write files, `verify` passes), `golden_trace_still_verifies_without_edits`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control -p ludenea-cli`. Expected: compile failure.
- [ ] **Step 3: Implement** `stdio.rs` and the CLI changes.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(cli): host the game over stdio and replay edit logs`.

---

### Task 11: Editor workspace and the control client

**Files:**
- Create: `editor/{package.json, pnpm-workspace.yaml, tsconfig.base.json, vitest.workspace.ts}`, `editor/packages/control-client/{package.json, tsconfig.json, src/index.ts, src/client.ts, src/transports/{transport.ts, websocket.ts, post-message.ts, tauri.ts, fake.ts}}`, `editor/packages/control-client/test/client.test.ts`
- Modify: `.gitignore` (`editor/**/dist/`), `README.md` (editor development commands)

**Interfaces:**
- Consumes: Task 9 generated types.
- Produces (`@ludenea/control-client`):
  - `export interface MessageTransport { send(message: string): void; onMessage(listener: (message: string) => void): () => void; close(): void }`; implementations `webSocketTransport(url: string): Promise<MessageTransport>`, `postMessageTransport(port: MessagePort | Worker): MessageTransport`, `tauriTransport(channel: "control" | "project"): Promise<MessageTransport>` (Tauri `invoke("send", { channel, message })` and event `"<channel>-message"`), `fakeTransport(handler: (message: string) => string[]): MessageTransport` (tests).
  - `export class ControlClient { static connect(transport: MessageTransport): Promise<ControlClient>` (sends `Hello` with `PROTOCOL_VERSION = 2`, rejects with the `Refused` message) `; readonly game: GameInfo; request<C extends Command>(command: C): Promise<ReplyFor<C>>; on<T extends Event["type"]>(type: T, listener: (event: Extract<Event, { type: T }>) => void): () => void; close(): void }`. Requests carry increasing ids; responses resolve by id; `{ error }` results reject with `ProtocolFailure extends Error { code: ErrorCode }`. `ReplyFor<C>` maps each command type to its reply variant.
- Scripts in `editor/package.json`: `test` (Vitest), `typecheck` (`tsc -b`), `build`, `e2e` (Playwright, Task 23).

- [ ] **Step 1: Write the failing tests** in `client.test.ts`: `connect_rejects_version_mismatch_with_message`, `responses_resolve_by_id_out_of_order`, `error_result_rejects_with_code`, `events_reach_typed_listeners_and_unsubscribe_works`, `close_rejects_pending_requests`.
- [ ] **Step 2: Run** `pnpm -C editor install && pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the client and transports.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add the typed control client and its transports`.

---

### Task 12: WebAssembly game host in a Web Worker, browser golden check

**Files:**
- Create: `crates/ludenea-control/src/worker.rs` (`cfg(all(target_arch = "wasm32", target_os = "unknown"))`), `editor/packages/control-client/src/worker/host-worker.ts`, `editor/e2e/{playwright.config.ts, golden.spec.ts, golden/index.html}`, `editor/scripts/build-game-wasm.sh`
- Modify: facade (`pub use wasm_bindgen;` and `macro_rules! worker_entry` on wasm targets), `examples/boxes/{Cargo.toml (crate-type ["cdylib", "rlib"]), src/lib.rs (ludenea::worker_entry!(app);)}`
- Test: `editor/e2e/golden.spec.ts`

**Interfaces:**
- Produces (`ludenea_control::worker`): `#[wasm_bindgen] pub struct WorkerHost` with `handle(&mut self, message: &str) -> String` (JSON array of `ServerMessage`) and `advance(&mut self, elapsed_ms: f64) -> String` (JSON array of `Event` messages); `WorkerHost::new(app: fn() -> App) -> WorkerHost` (Rust only).
- Produces (facade): `ludenea::worker_entry!(path::to::app)` expands to `#[wasm_bindgen(crate = "ludenea::wasm_bindgen", js_name = createHost)] pub fn create_host() -> WorkerHost` on wasm targets and to nothing elsewhere.
- Produces (`host-worker.ts`): a module worker; first message `{ type: "init", glueUrl: string }` imports the wasm-bindgen glue (`--target web`), calls its default init, then `createHost()`; later messages are protocol JSON strings passed to `handle`; a `setInterval` of 16 ms calls `advance` with the measured elapsed time and posts each resulting message.
- `build-game-wasm.sh <package> <out-dir>`: `cargo build --locked --release --target wasm32-unknown-unknown -p <package> --lib`, then `wasm-bindgen --target web` (the CLI matching the pinned version) into `<out-dir>`. Development only; the project service uses the library in Task 14.

- [ ] **Step 1: Write the failing test** `golden.spec.ts` (`boxes in a headless browser matches the golden trace`): the page loads the worker with the boxes glue, sends `LoadScene`, `LoadInputs`, `Step { ticks: 10000, record_hashes: true }`, decodes `golden/boxes.trace` (postcard: varint `version`, `scene_hash`, `inputs_hash`, then varint length and varint hashes) in TypeScript, and asserts all 10,001 hashes (initial `GetHash` plus 10,000 recorded) equal the golden.
- [ ] **Step 2: Run** `editor/scripts/build-game-wasm.sh boxes editor/e2e/golden/pkg && pnpm -C editor e2e golden`. Expected: FAIL (no worker host).
- [ ] **Step 3: Implement** `worker.rs`, the macro, and the worker module.
- [ ] **Step 4: Run** the same commands. Expected: PASS. On a mismatch, stop and debug with snapshots and `diff` (systematic-debugging skill); never re-record the golden.
- [ ] **Step 5: Commit** `✅ test(boxes): verify the golden trace in a headless browser worker`.

---

### Task 13: Projects, metas, and `ludenea new`

**Files:**
- Create: `crates/ludenea-project/{Cargo.toml, src/lib.rs, src/manifest.rs, src/meta.rs, src/files.rs, src/template.rs, template/**}`, `crates/ludenea-serve/{Cargo.toml, src/main.rs}` (binary `ludenea`, subcommand `new` only in this task)
- Test: `crates/ludenea-project/tests/{project.rs, metas.rs}`

**Interfaces:**
- Produces (`ludenea_project`):
  - `ludenea.toml`: `pub struct Manifest { pub name: String, pub engine: String /* version */, pub default_scene: Option<String> }` (TOML; add `toml` crate).
  - `pub struct Project` with `Project::open(root: &Path) -> Result<Project, ProjectError>`, `Project::create(parent: &Path, name: &str, engine_path: &Path) -> Result<Project, ProjectError>` (writes the template: `ludenea.toml`, `Cargo.toml` depending on `ludenea = { path = <engine_path>/crates/ludenea }`, `src/lib.rs` with `pub fn app()` and `ludenea::worker_entry!(app);`, `src/main.rs` calling `ludenea::cli::main`, `scenes/main.scene.ron` with a static floor and three dynamic boxes, `editor/index.ts` with an empty `definePlugin`, `editor/tsconfig.json` pointing at `.ludenea/types/`, `.gitignore` with `.ludenea/` and `target/`). Names must match `^[a-z][a-z0-9_]*$` (`ProjectError::InvalidName` explains the rule).
  - Metas: each file under `scenes/` and `assets/` has `<file>.meta` (RON `Meta(version: 1, id: "<32 hex>", kind: Scene | Other)`); `id` = `AssetId(uuid::Uuid::new_v4().as_u128())`. `Project::files(&self) -> Vec<FileEntry>` with `pub struct FileEntry { pub path: String /* relative, forward slashes */, pub id: AssetId, pub kind: AssetKind }`; listing creates missing metas. `Project::rename(&mut self, from: &str, to: &str)`, `Project::delete(&mut self, path: &str)` move or delete the meta with its file. An orphan meta (no sibling file) produces `ProjectWarning::OrphanMeta { id, path }` once and is never deleted automatically.
  - `Project::read_text(&self, path: &str) -> Result<String, ProjectError>`, `write_text(&mut self, path: &str, text: &str)`; every path is checked to stay inside the project root (rejects `..` and absolute paths with `ProjectError::OutsideProject`).
  - `Project::watch(&self) -> Result<Receiver<FileChange>, ProjectError>` (notify, debounced 100 ms): `pub enum FileChange { Changed(String), Created(String), Removed(String) }`, ignoring `.ludenea/` and `target/`.
- Produces (binary `ludenea`): `ludenea new <dir> --engine-path <path>`.

- [ ] **Step 1: Write the failing tests**: `create_writes_template_that_opens`, `invalid_name_is_rejected_with_rule`, `listing_creates_missing_metas_and_keeps_ids_stable`, `rename_moves_meta_with_file`, `file_moved_outside_editor_gets_new_id_and_orphan_warning_once`, `paths_outside_project_are_rejected`, `watch_reports_scene_change_on_disk`, `template_builds_and_verifies` (marked `#[ignore]` by default, run in CI: `cargo build` in a temp project created from this repository, then `cargo run -- run scenes/main.scene.ron --ticks 60 --hash`).
- [ ] **Step 2: Run** `cargo test -p ludenea-project`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate and `ludenea new`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-project -- --ignored && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(project): create and open projects with stable asset identifiers`.

---

### Task 14: Builds, game host processes, and game types

**Files:**
- Create: `crates/ludenea-project/src/{build.rs, game.rs, types.rs}`, `crates/ludenea-project/types/{editor-shell.d.ts, control-client.d.ts}` (placeholders replaced in Task 17)
- Test: `crates/ludenea-project/tests/build.rs` (`#[ignore]`, CI only: compiles the template)

**Interfaces:**
- Consumes: Task 13 `Project`, Task 10 `host --stdio`, Task 1 `typescript_declarations`, Task 12 `worker_entry!`.
- Produces:
  - `pub enum BuildTarget { Native, Wasm }`. `Project::build(&self, target: BuildTarget, output: Sender<BuildEvent>) -> JoinHandle<Result<BuildArtifact, BuildError>>`: runs `cargo build --release --message-format=json-render-diagnostics` (plus `--target wasm32-unknown-unknown --lib` for `Wasm`), parsing messages with `cargo_metadata`. `pub enum BuildEvent { Line { text: String }, Progress { done: u32, total: u32 }, Diagnostic(Diagnostic), Finished { ok: bool } }`, `pub struct Diagnostic { pub level: String, pub message: String, pub file: Option<String>, pub line: Option<u32>, pub column: Option<u32> }`. `pub enum BuildArtifact { Native { binary: PathBuf }, Wasm { dir: PathBuf /* .ludenea/build/<hash>/ with game.js and game_bg.wasm */ } }`; the wasm step runs `wasm-bindgen-cli-support` as a library (`--target web`), never an external tool.
  - `pub struct GameProcess`: `GameProcess::start(binary: &Path) -> Result<GameProcess, ProjectError>` spawns `<binary> host --stdio`; `send(&mut self, line: &str)`, `messages(&self) -> Receiver<String>`, `stop(self)`; `stderr(&self) -> Receiver<String>` carries its stderr lines for the console; `wait(&mut self) -> Option<i32>` returns the exit code once it exits (the `messages` receiver disconnects at exit).
  - `Project::write_game_types(&self, info: &GameInfo) -> Result<(), ProjectError>` writes `.ludenea/types/game.d.ts` (`typescript_declarations` with `declare module "@ludenea/editor-shell" { interface ComponentTypes {...} }`) plus copies of the embedded `editor-shell.d.ts` and `control-client.d.ts`.

- [ ] **Step 1: Write the failing tests**: `native_build_produces_binary_and_reports_progress`, `build_error_reports_file_and_line` (template with a type error in `src/lib.rs`), `wasm_build_produces_glue_and_wasm`, `game_process_handshakes_over_stdio`, `game_types_declare_template_components`.
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test build -- --ignored`. Expected: compile failure.
- [ ] **Step 3: Implement** `build.rs`, `game.rs`, `types.rs`.
- [ ] **Step 4: Run** the same command plus the workspace clippy. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(project): build games natively and to WebAssembly and run their hosts`.

---

### Task 15: Embedded plugin bundler and hot reload

**Files:**
- Create: `crates/ludenea-project/src/plugins.rs`
- Test: `crates/ludenea-project/tests/plugins.rs`

**Interfaces:**
- Consumes: Task 13 `Project::watch`.
- Produces:
  - `pub struct PluginBundle { pub name: String, pub url_path: String /* plugins/<name>.<hash>.js */, pub source: PluginSource }`, `pub enum PluginSource { Project, Engine { crate_name: String } }`.
  - `Project::bundle_plugins(&self) -> Result<Vec<PluginBundle>, Vec<Diagnostic>>`: bundles `editor/index.ts` (name = project name) and every engine plugin found through `cargo metadata` as `<crate root>/editor/index.ts` of a dependency, with rolldown embedded as a library: ESM output, TypeScript and JSX (automatic runtime) stripped without type checking, the four shared specifiers external, any other bare import an error naming the import and stating that 2a plugins may import only React, the editor packages, and their own files. Bundles are written to `.ludenea/plugins/`.
  - `Project::watch_plugins(&self) -> Receiver<PluginEvent>`: `pub enum PluginEvent { Rebuilt(PluginBundle), Failed { name: String, diagnostics: Vec<Diagnostic> } }`, rebundling only the plugin whose sources changed.
- Physics ships `crates/ludenea-physics/editor/index.ts` (a plugin registering the three debug layer toggles as settings), so engine plugin discovery has a real case.

- [ ] **Step 1: Write the failing tests**: `bundles_tsx_plugin_with_shared_imports_external` (output contains `from "react"` and no React source), `third_party_import_is_rejected_with_rule`, `syntax_error_reports_file_and_line`, `engine_plugin_of_physics_is_discovered`, `editing_plugin_source_emits_rebuilt_with_new_hash`.
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test plugins`. Expected: compile failure.
- [ ] **Step 3: Implement** `plugins.rs` (add the rolldown crates, pinned to one exact version).
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-project && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(project): bundle editor plugins without Node.js and rebuild on change`.

---

### Task 16: Project protocol and `ludenea serve`

**Files:**
- Create: `crates/ludenea-project/src/{protocol.rs, service.rs}`, `crates/ludenea-serve/src/serve.rs`
- Modify: `crates/ludenea-serve/src/main.rs`, Task 9 export (project protocol types into `editor/packages/control-client/src/generated/project/`)
- Test: `crates/ludenea-project/tests/service.rs`, `crates/ludenea-serve/tests/serve.rs`

**Interfaces:**
- Produces (`ludenea_project::protocol`), envelope shaped like the control protocol: `PROJECT_PROTOCOL_VERSION = 1`, `ProjectClientMessage { Hello { protocol_version }, Request { id, request: ProjectRequest } }`, `ProjectServerMessage { Welcome { protocol_version }, Refused { error }, Response { id, result }, Event { event: ProjectEvent } }`.
  - `ProjectRequest`: `Create { parent: String, name: String }`, `Open { path: String }`, `Info` → `ProjectInfo { name, root, default_scene, mode: ServiceMode /* Desktop | Browser */ }`, `ListFiles` → `Files { entries: Vec<FileEntry> }`, `ReadText { path }` → `Text { text }`, `WriteText { path, text }`, `Rename { from, to }`, `Delete { path }`, `Build` (target chosen by the mode), `ListPlugins` → `Plugins { plugins: Vec<PluginBundle> }`, `GameEndpoint` → `Native` (desktop: the backend relays the control channel) or `Wasm { glue_url: String }`.
  - `ProjectEvent`: `FilesChanged { changes: Vec<FileChange> }`, `Build(BuildEvent)`, `GameReady`, `GameExited { code: Option<i32> }`, `PluginRebuilt(PluginBundle)`, `PluginFailed { name, diagnostics }`, `Warning { message }`.
- Produces (`ludenea_project::service`): `pub struct ProjectService` with `new(mode: ServiceMode, engine_path: Option<PathBuf>) -> Self`, `handle(&mut self, request: ProjectRequest) -> Result<ProjectReply, ProjectFailure>`, `events(&self) -> Receiver<ProjectEvent>`; after `Open`, it builds in the background and, in desktop mode, starts the `GameProcess` when the build succeeds (keeping the previous one on failure) and exposes `control_channel(&self) -> Option<(Sender<String>, Receiver<String>)>`.
- Produces (binary `ludenea`): `ludenea serve <project> [--port 7878] [--editor-dir <dir>] [--engine-path <path>]` (axum, tokio): `GET /` and assets from the editor build (embedded with `include_dir` from `editor/apps/web/dist` behind feature `embed-editor`, otherwise `--editor-dir`), `GET /ws/project` (WebSocket, project protocol), `GET /game/<hash>/<file>` (wasm build output), `GET /plugins/<file>` (bundles), all bound to `127.0.0.1` unless `--host` is passed.

- [ ] **Step 1: Write the failing tests**: `open_lists_files_and_builds_in_background` (events `Build(Finished { ok: true })` then `GameReady`), `failed_rebuild_keeps_previous_game`, `ws_handshake_and_list_files`, `plugins_and_game_files_are_served_with_js_and_wasm_content_types`, `requests_outside_project_are_rejected`.
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test service -p ludenea-serve`. Expected: compile failure.
- [ ] **Step 3: Implement** the protocol, the service, and the server.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(serve): serve projects and the editor over WebSocket`.

---

### Task 17: Editor shell and the plugin API

**Files:**
- Create: `editor/packages/editor-shell/{package.json, src/index.ts, src/plugin.ts, src/registry.ts, src/editor.ts, src/layout/Shell.tsx, src/palette/CommandPalette.tsx, src/shortcuts.ts, src/stores/{selection.ts, timeline.ts, project.ts}, src/loader.ts}`, `editor/packages/editor-shell/test/{registry.test.ts, loader.test.ts, palette.test.tsx}`, `editor/scripts/bundle-dts.sh`
- Modify: `crates/ludenea-project/types/*.d.ts` (generated by `bundle-dts.sh`)

**Interfaces:**
- Consumes: Task 11 `ControlClient`; a `ProjectClient` built the same way on the project protocol (add `editor/packages/control-client/src/project.ts`).
- Produces (`@ludenea/editor-shell`):
  - `export interface Disposable { dispose(): void }`, `export function definePlugin(plugin: PluginDefinition): PluginDefinition`, `export interface PluginDefinition { name: string; activate(editor: Editor): void | (() => void) }`.
  - `export interface Editor { control: ControlClient | null; project: ProjectClient; panels: { register(panel: PanelDefinition): Disposable }; inspectors: { register<N extends keyof ComponentTypes>(component: N, inspector: InspectorComponent<ComponentTypes[N]>): Disposable }; commands: { register(command: CommandDefinition): Disposable; run(id: string): Promise<void>; list(): CommandDefinition[] }; shortcuts: { register(shortcut: { key: string; command: string }): Disposable }; overlays: { register(overlay: ViewportOverlay): Disposable }; settings: { register<T>(setting: SettingDefinition<T>): Disposable; get<T>(id: string): T; set<T>(id: string, value: T): void }; selection: SelectionStore; timeline: TimelineStore; onGameChanged(listener: (control: ControlClient | null) => void): Disposable }`.
  - `export interface ComponentTypes {}` (augmented by the generated `game.d.ts`).
  - `PanelDefinition { id: string; title: string; component: React.ComponentType<{ editor: Editor }>; defaultPosition?: "left" | "right" | "bottom" | "center" }`, `CommandDefinition { id: string; title: string; run(editor: Editor): void | Promise<void> }`, `InspectorComponent<T> = React.ComponentType<{ value: T; onChange(value: T): void; entity: Entity }>`, `ViewportOverlay { id: string; draw(context: OverlayContext): void }`.
  - Every registration made during `activate` is collected per plugin; `loader.deactivate(name)` disposes them and calls the returned cleanup. `loader.load(url: string)` imports the module and activates its default export; `loader.reload(name, url)` deactivates then loads, keeping stores (selection, timeline, layout) intact.
  - Registering a second panel, command, or shortcut with an existing id throws `DuplicateRegistration` naming both plugins.
  - Shell layout with dockview; the command palette (`Ctrl+K` and `Cmd+K`) lists every command, fuzzy filtered; shortcuts are global unless focus is in a text field.
  - `editor/scripts/bundle-dts.sh` rolls the public `.d.ts` of `editor-shell` and `control-client` into `crates/ludenea-project/types/` (checked for staleness in CI).

- [ ] **Step 1: Write the failing tests**: `registrations_are_disposed_on_deactivate`, `duplicate_command_id_names_both_plugins`, `reload_keeps_selection_and_layout`, `palette_lists_and_runs_registered_commands`, `shortcut_ignored_inside_text_input`, `built_in_and_project_plugins_receive_the_same_editor_object` (identity check).
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the shell package.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck && editor/scripts/bundle-dts.sh`. Expected: tests PASS and `crates/ludenea-project/types/*.d.ts` regenerated with the real declarations (they are part of this commit).
- [ ] **Step 5: Commit** `✨ feat(editor): add the editor shell and plugin API`.

---

### Task 18: Built-in plugins: project, assets, scene tree, console

**Files:**
- Create: `editor/packages/editor-builtins/{package.json, src/index.ts, src/project/*.tsx, src/assets/*.tsx, src/scene-tree/*.tsx, src/console/*.tsx}`, `editor/packages/editor-builtins/test/*.test.tsx`

**Interfaces:**
- Consumes: Task 17 `Editor`; project protocol (Task 16); `QueryEntities` and subscriptions (Task 8).
- Produces: `export const builtinPlugins: PluginDefinition[]`, each a `definePlugin` using only the `Editor` object:
  - `ludenea.project`: commands `project.new` (name and parent folder form) and `project.open`; recent projects in settings; build progress and "building" states.
  - `ludenea.assets`: file tree of `scenes/` and `assets/` with filter, rename, move (drag), delete (confirmation in the panel, never a browser dialog), open scene (`ReadText`, then `LoadScene`). On `FilesChanged` for the open scene: unsaved session (`is_scene_dirty`) shows the banner "changed on disk" with Reload and Keep mine; otherwise sends `ReplaceScene` with the new text (the recorded session is kept and resimulated). Orphan meta warnings go to the console.
  - `ludenea.scene-tree`: entities of the open scene (virtualized), label = first component short names, click selects (shared `selection` store), `Delete` despawns the selection, `scene.save` command (`SaveScene`, then `WriteText`).
  - `ludenea.console`: build lines and diagnostics (clickable `file:line` opens the asset browser at that file), game stderr, host events (`Paused` with a panic, `Diverged`), warnings; filter by level.

- [ ] **Step 1: Write the failing tests** (fake transports): `opening_scene_loads_it_into_the_game`, `disk_change_with_unsaved_edits_shows_banner_and_keeps_mine`, `disk_change_without_unsaved_edits_sends_replace_scene`, `rename_through_panel_sends_rename`, `scene_tree_selection_updates_store`, `console_shows_build_diagnostic_with_location`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the four plugins.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add project, asset, scene tree, and console panels`.

---

### Task 19: Built-in plugins: ECS explorer and inspector

**Files:**
- Create: `editor/packages/editor-builtins/src/{explorer/*.tsx, inspector/*.tsx, inspector/fields/*.tsx}`, tests next to them

**Interfaces:**
- Consumes: `GameInfo` schemas, `QueryEntities`, `GetEntity`, `GetResources`, `SetField`, `InsertComponent`, `RemoveComponent`, `GetSchedule`, `GetTickEvents`, `GetTickCommands`, `Subscribe` (Tasks 7 and 8).
- Produces:
  - `ludenea.explorer`: tabs Entities (query input with the code syntax, errors from `InvalidFilter` shown inline with candidates; `@tanstack/react-virtual` list paging `QueryEntities` by 200 rows; each row shows the entity and its hash), Schedule (run order, reads, writes, microseconds per tick), Events (selected tick), Commands (per sync point). Subscribes to `Introspection` only while the Schedule, Events, or Commands tab is visible.
  - `ludenea.inspector`: components and resources of the selection rendered from `TypeSchema` (number fields for integers and floats, checkbox, text, enum variant select with nested fields, lists and arrays with indexed rows, `Entity` as a link that selects it, `AssetId` as a picker over `ListFiles`); each change sends `SetField` with its path; `ComponentValue::Ron` values are read-only text; a registered custom inspector replaces the generic one for its component. Errors (`InvalidEdit`, `NoSuchEntity`) show under the field; `NoSuchEntity` also clears the selection. Add and remove component actions.
  - Generic rendering is itself `editor.inspectors`' fallback, so a plugin overriding one component sees the same props.

- [ ] **Step 1: Write the failing tests**: `filter_error_shows_candidates`, `list_renders_only_visible_rows_for_100000_entities` (fewer than 100 row elements in the DOM), `editing_nested_enum_field_sends_set_field_with_path` (`Collider.shape.Circle.radius` gives path `[shape, Circle, radius]`), `invalid_edit_message_shows_under_field`, `no_such_entity_clears_selection`, `custom_inspector_replaces_generic`, `introspection_subscribed_only_while_tab_visible`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** both plugins.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add the ECS explorer and the schema-driven inspector`.

---

### Task 20: Debug draw renderer and the viewport

**Files:**
- Modify: `crates/ludenea-debug-draw/{Cargo.toml, src/lib.rs}` (feature `render`: wgpu, wasm-bindgen, web-sys)
- Create: `crates/ludenea-debug-draw/src/render.rs`, `editor/scripts/build-renderer-wasm.sh`, `editor/packages/editor-builtins/src/viewport/*.tsx`, tests next to them

**Interfaces:**
- Consumes: Task 5 `DebugDrawFrame`, Task 8 `GetDebugDraw` and `Pick`.
- Produces (`ludenea_debug_draw::render`, wasm only): `#[wasm_bindgen] pub struct ViewportRenderer` with `async create(canvas: HtmlCanvasElement) -> Result<ViewportRenderer, JsValue>` (WebGPU, WebGL2 fallback through wgpu's `webgl` feature), `resize(&mut self, width: u32, height: u32, pixel_ratio: f32)`, `set_camera(&mut self, center_x: f32, center_y: f32, pixels_per_meter: f32)`, `draw(&mut self, frame_json: &str, selected_json: &str)` (selected entities drawn highlighted). Rendering triangulates circles (32 segments), capsules, and polygon fills on the CPU into one vertex buffer per frame. `build-renderer-wasm.sh` writes the package to `editor/packages/debug-draw-wasm/` (gitignored, built by `pnpm -C editor build`).
- Produces (`ludenea.viewport` plugin): canvas sized to its panel; pan (middle drag or Space plus drag), zoom (wheel, anchored at the cursor), click to `Pick` and select (Shift adds), drag a selected entity with a `Pose` to move it (while dragging, the plugin draws the moved outline in a local overlay; release sends one `SetField` on `Pose.position`, so one edit is recorded), arrow keys nudge the selection by 0.1 m (Shift: 1 m); layer toggles from `GameInfo.debug_layers`; frames requested once per animation frame while playing, on change otherwise; overlays from `editor.overlays` draw on a 2D canvas above.

- [ ] **Step 1: Write the failing tests**: Rust `cargo test -p ludenea-debug-draw` (`triangulated_circle_has_32_segments`, `capsule_mesh_is_closed`); TypeScript (renderer mocked): `screen_to_world_inverts_world_to_screen`, `zoom_keeps_point_under_cursor`, `click_sends_pick_and_selects`, `drag_sends_one_set_field_on_release`, `arrow_nudges_by_tenth_meter`.
- [ ] **Step 2: Run** `cargo test -p ludenea-debug-draw && pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the renderer and the plugin.
- [ ] **Step 4: Run** the same plus `editor/scripts/build-renderer-wasm.sh` and `pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): render debug draw in the viewport and move bodies by hand`.

---

### Task 21: Timeline, live input, and undo in the editor

**Files:**
- Create: `editor/packages/editor-builtins/src/timeline/*.tsx`, `editor/packages/editor-builtins/src/input/capture.ts`, tests next to them

**Interfaces:**
- Consumes: Task 7 `Play`, `Pause`, `Step`, `Seek`, `SetSpeed`, `SetLiveInput`, `Undo`, `Redo`, `GetTimeline`, events `Ticked`, `TimelineChanged`, `Paused`, `Resimulated`, `Diverged`.
- Produces:
  - `ludenea.timeline` panel: current tick, head, hash (with a "pending" marker when `pending`), effective rate when below the tick rate, a scrubber over `[0, head]` (drag seeks, throttled to one `Seek` in flight), checkpoint ticks as faint ticks, edit ticks as markers, a divergence marker from `Diverged`, resimulation progress.
  - Commands with default shortcuts: `timeline.play-pause` (`Space` when the viewport has focus, `F5` anywhere), `timeline.step` (`.`), `timeline.step-back` (`,`: seek to current minus 1), `timeline.speed-up`, `timeline.speed-down`, `edit.undo` (`Ctrl+Z` / `Cmd+Z`), `edit.redo` (`Ctrl+Shift+Z` / `Cmd+Shift+Z`).
  - `capture.ts`: while playing and the viewport has focus, sends `SetLiveInput` once per animation frame with pressed `KeyboardEvent.code` values and the Gamepad API state of the first gamepad, for player 0 (one player in 2a); sends an idle state on blur.

- [ ] **Step 1: Write the failing tests**: `scrubbing_sends_at_most_one_seek_in_flight`, `pending_marker_follows_hash_reply`, `undo_shortcut_runs_undo_and_refreshes_timeline`, `divergence_marker_appears_on_diverged_event`, `capture_sends_pressed_codes_and_idle_on_blur`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the panel, commands, and capture.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add the timeline with play, step, seek, and undo`.

---

### Task 22: Web and desktop applications

**Files:**
- Create: `editor/apps/web/{package.json, index.html, vite.config.ts, src/main.tsx}`, `editor/apps/desktop/{package.json, index.html, vite.config.ts, src/main.tsx, src-tauri/{Cargo.toml, tauri.conf.json, src/main.rs, capabilities/default.json}}`
- Modify: workspace `Cargo.toml` members (`editor/apps/desktop/src-tauri`), `crates/ludenea-serve` (feature `embed-editor`)

**Interfaces:**
- Consumes: Tasks 16 to 21.
- Produces:
  - Both apps: `index.html` with the static import map for the four shared specifiers (Global Constraints); Vite config marks them external and emits `shared/<name>.js` entries with `preserveEntrySignatures: "strict"`; `main.tsx` mounts the shell, loads `builtinPlugins`, then every `ListPlugins` bundle through `loader.load`, and reloads on `PluginRebuilt`.
  - Web: project client over `webSocketTransport("/ws/project")`; on `GameEndpoint::Wasm { glue_url }`, starts `host-worker.ts` with `postMessageTransport` and connects the control client; a rebuild exports the session (`ExportSession`), restarts the worker, sends `LoadSession` with the exported values, then seeks to the previous tick once resimulation reaches it; `Diverged` is reported if the new code changes hashes.
  - Desktop: `src-tauri` embeds `ProjectService` in desktop mode; Tauri command `send(channel, message)` routes `project` messages to the service and `control` lines to the game process unchanged; events `project-message` and `control-message` carry replies and events; on rebuild the backend restarts the game process and the front end restores the session the same way as the web app (`ExportSession` before, `LoadSession` after).
  - Window shows the shell before any build (spec target: interactive in under 2 seconds).

- [ ] **Step 1: Write the failing test** `editor/e2e/smoke.spec.ts` (`web editor opens the template project`): start `ludenea serve` on a temp project created with `ludenea new`, open `/`, expect the asset browser to list `scenes/main.scene.ron` within 2 seconds and the viewport to leave the building state after the build.
- [ ] **Step 2: Run** `pnpm -C editor build && cargo build --locked -p ludenea-serve && pnpm -C editor e2e smoke`. Expected: FAIL.
- [ ] **Step 3: Implement** both apps.
- [ ] **Step 4: Run** the same, plus `pnpm -C editor/apps/desktop tauri build --debug` (the desktop app builds). Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): ship the editor in the browser and on the desktop`.

---

### Task 23: End-to-end proofs, continuous integration, README

**Files:**
- Create: `editor/e2e/{session.spec.ts, plugins.spec.ts, performance.spec.ts, fixtures/test-plugin/index.tsx}`, `examples/boxes/src/bin/editor-bench.rs` (writes a scene of 100,000 entities: the 1,000-body `generate_scene` plus 99,000 entities with only a `Pose`)
- Modify: `.github/workflows/determinism.yml` (job `editor`), `README.md`

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Write the end-to-end tests** (web app against `ludenea serve`, template project):
  - `session.spec.ts` (`an editor session replays in the CLI`): open the main scene, play 2 seconds, pause, seek back 60 ticks, change a box's `Pose.position.x` in the inspector, play 1 second, undo, redo, `ExportSession` written to files, then `cargo run -- verify` with `--inputs` and `--edits` against a trace recorded from the exported files with `run --trace` passes, and its final hash equals the editor's final hash.
  - `plugins.spec.ts` (`a project plugin has the same API as built-ins`): copy `fixtures/test-plugin` into the project's `editor/`, expect its panel and its custom inspector for `ludenea::Body` to appear without restarting the game (timeline tick unchanged), and the plugin to have no access beyond its `editor` argument (it reads nothing from `globalThis` except shared modules).
  - `performance.spec.ts` (`the UI stays at 60 fps with 100,000 entities`): open the bench scene, play 5 seconds, measure frame times with `requestAnimationFrame` in the page; the 95th percentile frame time is at most 20 ms and the shell was interactive in under 2 seconds. Report the simulation's effective rate (not a pass criterion).
- [ ] **Step 2: Run** `pnpm -C editor e2e`. Expected: PASS (fix what fails in the owning task's code).
- [ ] **Step 3: Extend CI**: job `editor` on `ubuntu-24.04` installs the toolchain, Node 24 and pnpm, Playwright Chromium, runs `pnpm -C editor install --frozen-lockfile`, `pnpm -C editor typecheck`, `pnpm -C editor test`, the TypeScript and `.d.ts` staleness checks (`cargo test -p ludenea-control --test typescript && editor/scripts/bundle-dts.sh && git diff --exit-code`), `cargo test --locked -p ludenea-project -- --ignored`, then `pnpm -C editor e2e`. The existing jobs are unchanged.
- [ ] **Step 4: Update `README.md`**: the editor (what it does, `ludenea new`, `ludenea serve`, the desktop app), the plugin API in five lines, and the development commands.
- [ ] **Step 5: Run** `cargo clippy --locked --workspace --all-targets -- -D warnings && cargo test --locked --workspace && pnpm -C editor test`. Expected: PASS.
- [ ] **Step 6: Commit** `✅ test(editor): prove session replay, plugin parity, and UI fluidity end to end`.
