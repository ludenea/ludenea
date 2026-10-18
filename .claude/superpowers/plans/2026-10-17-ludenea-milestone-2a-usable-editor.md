# Ludenea Milestone 2a: Usable Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One editor, on desktop (Tauri) and in the browser (`ludenea serve`), that creates and opens Ludenea projects, explores and edits the ECS, plays, pauses, steps, and seeks a timeline whose edits are recorded as inputs, with undo and redo, React plugins built by an embedded Rust bundler, and editor sessions the CLI replays to the same trace.

**Architecture:** The Rust side grows field schemas, an edit log, a session timeline with logarithmic checkpoints, editor hooks (debug draw, picking, raw input mapping), and protocol version 2, served by the game's own binary over stdio (desktop) or by the game compiled to WebAssembly in a Web Worker (browser). A new project service owns files, metas, cargo builds, and plugin bundles, embedded in the Tauri backend and in the `ludenea` tool (`new`, `serve`). The front end is a pnpm workspace: `control-client`, `editor-shell`, `editor-builtins`, and two apps, with every built-in panel written as a plugin on the public API.

**Tech Stack:** Rust 1.97.1, serde, serde_json, ts-rs, wasm-bindgen and wasm-bindgen-cli-support (pinned to one exact version, driven by an `xtask`), wgpu, rolldown (embedded, pinned), axum and tokio (WebSocket server), notify with notify-debouncer-full, uuid, cargo_metadata, Tauri 2; TypeScript strict, React 19, Vite, pnpm, Vitest, Playwright, dockview, Zustand, @tanstack/react-virtual, rollup-plugin-dts (development only).

**Spec:** `.claude/superpowers/specs/2026-10-17-ludenea-editor-2a-design.md` (parent: `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md`). The milestone 1 plan `.claude/superpowers/plans/2026-10-17-ludenea-milestone-1-deterministic-core.md` describes the existing code.

## Global Constraints

- Every Global Constraint of the milestone 1 plan still applies to Rust code, in particular the determinism rules for simulation crates (`ludenea-math`, `-schema`, `-ecs`, `-app`, `-physics`, `-debug-draw` without its `render` feature, and game code).
- Code outside the simulation (`ludenea-control` stdio and pacer, introspection timing, `ludenea-project`, `ludenea-serve`, `xtask`, the Tauri backend) may use wall clocks, `HashMap`, and float `min`/`max`; `clippy.toml` applies workspace-wide, so each such use carries `#[allow(clippy::disallowed_methods)]` or `#[allow(clippy::disallowed_types)]` on the smallest enclosing item.
- Toolchain: add target `wasm32-unknown-unknown` to `rust-toolchain.toml`.
- Target gates: browser-only code uses `cfg(all(target_arch = "wasm32", target_os = "unknown"))`; native-only code (threads, stdio, processes) uses `cfg(not(target_family = "wasm"))`, so the existing `wasm32-wasip1` CI build keeps compiling. On `wasm32-unknown-unknown` panics abort the instance and `std::time::Instant::now` panics: browser code never calls `Instant` and never relies on `catch_unwind`.
- `wasm-bindgen` and `wasm-bindgen-cli-support` are pinned to the same exact version (`=x.y.z`) in `[workspace.dependencies]`. WebAssembly packaging goes through the workspace binary `xtask` (`cargo xtask wasm-game <package> <out-dir>`, `cargo xtask wasm-renderer <out-dir>`), which calls `wasm-bindgen-cli-support` as a library; nothing installs the `wasm-bindgen` command.
- rolldown crates are pinned to one exact version (all `rolldown*` crates on the same `=x.y.z`), chosen in the first step of Task 17.
- Tauri: `tauri = "2"` and `@tauri-apps/cli@2` (Tauri 3 is in alpha). `editor/apps/desktop/src-tauri` is listed in `[workspace] exclude`, has its own `Cargo.lock` (committed), and is built only by its own CI job, so `cargo clippy --workspace` and `cargo test --workspace` never compile it.
- Protocol: `PROTOCOL_VERSION = 2`. Component and resource values on the wire are JSON values in serde's externally tagged form. Any JSON integer outside `±2^53` travels as a decimal string in both directions; the host converts with the type schema (Task 8). `u64` hashes stay 16-digit hex strings (`Hash64`); `Bytes` stays base64.
- ts-rs: feature `serde-json-impl`; `[env] TS_RS_LARGE_INT = "number"` in `.cargo/config.toml`; types with a hand-written `Serialize` (`Hash64`, `Bytes`, `AssetId`) carry `#[ts(type = "string")]` on the container, `hex_u64` fields carry `#[ts(type = "string")]`, and `#[serde(with = ...)]` fields carry `#[ts(as = ...)]`.
- Users never need Node.js: no runtime path (project service, `ludenea` tool, desktop app) may spawn `node`, `npm`, `npx`, or `pnpm`. Node 24 and pnpm 10 are development tools for building the editor itself. `pnpm-lock.yaml` is committed and CI uses `pnpm install --frozen-lockfile`.
- Front end: TypeScript `strict: true`, `moduleResolution: "bundler"`, ES modules only, React function components. Tests: Vitest (units), Playwright (end-to-end, Chromium launched with `--use-angle=swiftshader`). `pnpm -C editor build:wasm` (Task 22) must run before `pnpm -C editor typecheck` once the viewport exists.
- Shared runtime modules for plugins: `react`, `react/jsx-runtime`, `react-dom`, `@ludenea/editor-shell`, `@ludenea/control-client`. A separate Vite library build emits them as `shared/<name>.js` with explicit named-export lists; the editor's `index.html` declares a static import map to those files, and both the editor build and every plugin bundle keep these five specifiers external, so one instance of each exists (Task 24). `react-dom` joined the list so the dock, the shell's renderer, and the built-ins' list library share one copy.
- Generated files are committed and checked in CI for staleness: protocol TypeScript (`editor/packages/control-client/src/generated/`), project protocol TypeScript (`editor/packages/control-client/src/generated/project/`), the embedded `.d.ts` bundles (`crates/ludenea-project/types/`).
- Tests that run a real `cargo build` of a project are `#[ignore]` and run in CI with `-- --ignored`.
- Keys: `F5` toggles play and pause; `Space` is never an editor shortcut (games and viewport panning use it).
- Stable names: unchanged from milestone 1. Schema type names are `module_path!()::Type`.
- Commits: gitmoji plus Conventional Commits, no internal plan references, no AI attribution trailers.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and (from Task 13 on) `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- TypeScript types for engine and game components both come from the schema generator (Task 1), not from `ts-rs`; `ts-rs` covers protocol types only (Task 11). One generator, one source of truth.
- The edit log is folded into the trace's `inputs_hash` (xxh3 of the input log bytes followed by the edit log bytes when an edit log is given), so the trace format and the golden trace stay unchanged.
- `ListEntities` is replaced by `QueryEntities` (one way to list entities).
- Recorded edits store whole component or resource values in canonical postcard bytes; field paths exist only in the protocol (`SetField`), which the host turns into whole values.
- Values without an exact JSON form (maps with non-string keys such as `ludenea::ContactCache`, non-finite floats) are shown read-only as RON in the inspector.
- Panic recovery: on desktop the host recovers in process (nearest checkpoint, replay, pause). In the browser a panic aborts the WebAssembly instance; the web app recreates the worker from the last exported session (exported on every pause, every edit, and every second while playing), so up to one second of live play can be lost.
- Per-system timings are not measured in the browser (`micros: None`).
- While dragging a body in the viewport, only the viewport shows the move (a local outline); the inspector and the recorded state update on release, as one edit.
- `ludenea::Time` is read-only in the editor: the timeline drives it.

## Review Focus

- **A system panics during a tick** (game bug). Desktop: the host restores the last good state, pauses with `PauseReason::Panicked { tick, message }`, and keeps the session (Task 7). Browser: the worker is recreated from the last exported session and the console shows the panic message (Task 24).
- **A field edit with an invalid value** (non-unit rotation on `Pose`, a negative radius, a string where a number goes) fails with `InvalidEdit` naming the component and field path, records nothing, and leaves the preview unchanged. Test in Task 8.
- **An edit targeting an entity that is not alive at the current tick** (selection kept after seeking back before its spawn) fails with `NoSuchEntity` and records nothing; the inspector clears its selection. Tests in Tasks 8 and 21.
- **A scene file changed on disk while open with unsaved edits in the editor**: the editor keeps its version and shows a "changed on disk" banner with Reload and Keep mine; without unsaved edits it sends `ReplaceScene` and the session resimulates. Tests in Tasks 15 and 20.
- **A file moved or renamed outside the editor** without its `.meta`: the moved file gets a new identifier, the orphan meta is reported once as a console warning naming its identifier and old path, and nothing is deleted automatically. Test in Task 15.

---

### Task 1: Field schemas

**Files:**
- Create: `crates/ludenea-schema/{Cargo.toml, src/lib.rs, src/typescript.rs}`
- Modify: `crates/ludenea-macros/src/lib.rs` (`derive(Schema)`, `schema_path()`; `derive(Component)` and `derive(Resource)` also implement `Schema`), `crates/ludenea-ecs/src/{component.rs, world.rs, entity.rs, lib.rs}`, `crates/ludenea-math/src/{vec2.rs, rot.rs, transform.rs, aabb.rs}` and `Cargo.toml`, `crates/ludenea-physics/src/{components.rs, shape.rs, contacts.rs}` and `Cargo.toml`, `crates/ludenea-app/src/{time.rs, rng.rs, input.rs}`, every test type deriving `Component`, `Resource`, or used as an input in existing tests (for example `Pad` in `crates/ludenea-app/tests/simulation.rs`), `examples/boxes/src/lib.rs`, facade (`pub use ludenea_schema as schema;`, prelude `Schema`), workspace `Cargo.toml`
- Test: `crates/ludenea-schema/src/lib.rs` (inline), `crates/ludenea-ecs/tests/schema.rs`, `examples/boxes/tests/typescript.rs`

**Interfaces:**
- Produces (`ludenea_schema`), all types `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]`:
  - `pub trait Schema { fn schema() -> TypeSchema; }` (recursive types overflow the stack; documented), implemented for `bool`, `u8..u64`, `usize` (as `U64`), `i8..i64`, `f32`, `f64`, `String`, `Option<T>`, `Vec<T>`, `[T; N]`, `BTreeMap<K, V>`, `BTreeSet<T>`, tuples up to 4, `()`. The crate declares `extern crate self as ludenea_schema;`.
  - `#[serde(tag = "kind", rename_all = "snake_case")] pub enum TypeSchema { Bool, U8, U16, U32, U64, I8, I16, I32, I64, F32, F64, String, Unit, Option { item: Box<TypeSchema> }, List { item: Box<TypeSchema> }, Array { item: Box<TypeSchema>, len: u32 }, Map { key: Box<TypeSchema>, value: Box<TypeSchema> }, Tuple { items: Vec<TypeSchema> }, Struct { name: String, fields: Vec<FieldSchema> }, Newtype { name: String, item: Box<TypeSchema> }, TupleStruct { name: String, items: Vec<TypeSchema> }, UnitStruct { name: String }, Enum { name: String, variants: Vec<VariantSchema> }, Entity, AssetId }`. A one-field tuple struct is `Newtype` (serde serializes it as the bare inner value).
  - `pub struct FieldSchema { pub name: String, pub schema: TypeSchema }`, `pub struct VariantSchema { pub name: String, pub fields: VariantFields }`, `#[serde(tag = "kind", rename_all = "snake_case")] pub enum VariantFields { Unit, Newtype { item: Box<TypeSchema> }, Tuple { items: Vec<TypeSchema> }, Struct { fields: Vec<FieldSchema> } }`.
  - `pub fn typescript_declarations(types: &[(&str, &TypeSchema)]) -> String`: one `export type` per named type reachable from the given types (named by last path segment; on a collision, by the full path with `::` replaced by `_`), plus `export interface ComponentTypes { "<stable name>": <Type>; ... }` for the given pairs. Externally tagged enums map to `"Unit" | { Newtype: T } | { Tuple: [..] } | { Struct: {..} }`; `Newtype` structs map to their item; `u64`, `i64`, and `usize` map to `number | string` (large values travel as strings); `Entity` maps to `{ index: number; generation: number }`; `AssetId` to `string`.
- Produces (`ludenea_macros`): `#[derive(Schema)]` for structs (named, tuple, unit) and enums. Paths resolve through `schema_path()`: `ludenea_schema`, else `ludenea_ecs::schema`, else `ludenea::schema` (by `proc-macro-crate`). It accepts the container attributes `#[serde(try_from, from, into)]` (documented: the mirror type must have identical fields) and fails to compile, naming the attribute, on `rename`, `rename_all`, `tag`, `content`, `untagged`, `flatten`, `skip`, `skip_serializing`, `skip_deserializing`, `default`, `with` at container, field, or variant level, so a schema always matches the serde shape.
- Produces (`ludenea_ecs`): `pub trait Component: Schema + ...` and `pub trait Resource: Schema + ...` (the derives implement both); `impl Schema for Entity` (`TypeSchema::Entity`); `#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)] pub struct AssetId(pub u128)` in `asset.rs`, serialized as 32 lowercase hex digits, `impl Schema` (`TypeSchema::AssetId`); `Codec` gains `pub schema: fn() -> TypeSchema`; `World::component_schemas(&self) -> Vec<(&'static str, TypeSchema)>` and `World::resource_schemas(&self) -> Vec<(&'static str, TypeSchema)>`, sorted by stable name.
- Produces (`ludenea_app`): `pub trait Input: Schema + ...`; `Simulation::input_schema(&self) -> Option<TypeSchema>`.

- [ ] **Step 1: Write the failing tests**: `struct_schema_lists_fields_in_declaration_order`, `enum_schema_covers_unit_newtype_tuple_and_struct_variants` (`Shape::Circle` is `VariantFields::Newtype`), `one_field_tuple_struct_is_newtype`, `containers_map_to_option_list_array_map_tuple` (`[Vec2; 8]` gives `Array { len: 8 }`, `usize` gives `U64`), `component_derive_also_derives_schema` (`Pose` schema is a struct with fields `position` and `rotation`), `world_lists_component_schemas_sorted_by_stable_name`; in `examples/boxes/tests/typescript.rs`: `typescript_declarations_for_boxes_components` (contains `export type Shape =`, `{ Circle: Circle }`, and `"ludenea::Pose": Pose`), `typescript_names_collide_to_full_paths`; `compile_fail` doc tests on `derive(Schema)` with `#[serde(rename_all = "camelCase")]` on the container and with `#[serde(skip)]` on a field.
- [ ] **Step 2: Run** `cargo test -p ludenea-schema -p ludenea-ecs -p boxes`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate, the derive, the supertraits, and `#[derive(Schema)]` on every nested engine type (`Vec2`, `Rot`, `Transform`, `Aabb`, `BodyKind`, `Shape`, `Circle`, `Capsule`, `Polygon`, `CachedContact`, `CachedPoint`, the `boxes` input and components, and the test types).
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
- Produces (`ludenea_ecs`): `Codec` gains `pub to_json: fn(&dyn Any) -> Result<serde_json::Value, String>` (fails unless decoding the JSON back gives the same postcard bytes, which rejects non-finite floats and non-string map keys) and `pub from_json: fn(serde_json::Value) -> DecodeResult` (through the concrete type, so validation such as `Pose`'s `try_from` runs). `World::component_json(&self, entity: Entity, name: &str) -> Option<Result<serde_json::Value, String>>`, `World::entity_components_json(&self, entity: Entity) -> Option<BTreeMap<String, Result<serde_json::Value, String>>>`, `World::resource_json(&self, name: &str) -> Option<Result<serde_json::Value, String>>`, `World::remove_by_name(&mut self, entity: Entity, name: &str) -> Result<(), EcsError>`.
- Produces (`ludenea_app::edit`), all `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]`:
  - `pub enum Edit { Spawn { components: Vec<(String, Vec<u8>)> }, Despawn { entity: Entity }, Insert { entity: Entity, component: String, value: Vec<u8> }, Remove { entity: Entity, component: String }, SetResource { resource: String, value: Vec<u8> } }`. Values are postcard bytes of the concrete type (`Codec::encode`), so the log is canonical. `Insert` inserts or replaces.
  - `pub struct TimedEdit { pub tick: u64, pub edit: Edit }`: made while the simulation was at `tick`, part of the state at `tick` from then on, so it takes effect when tick `tick + 1` runs.
  - `pub const EDIT_LOG_VERSION: u32 = 1;` `pub struct EditLog { pub version: u32, pub edits: Vec<TimedEdit> }` (ordered by tick, then record order), `EditLog::new() -> Self`, `EditLog::encode(&self) -> Vec<u8>` (postcard), `EditLog::decode(bytes: &[u8]) -> Result<EditLog, SimError>` (rejects other versions with `SimError::EditLogVersion { found, expected }` and trailing bytes), `EditLog::at(&self, tick: u64) -> &[TimedEdit]`, `EditLog::last_tick(&self) -> Option<u64>`.
  - `pub enum EditError` (`thiserror`): `NoSuchEntity(Entity)`, `UnknownComponent(String)`, `UnknownResource(String)`, `ReadOnly(String)` (for `ludenea::Time`), `InvalidValue { name: String, reason: String }`; messages say what to do.
  - `Simulation::apply_edit(&mut self, edit: &Edit) -> Result<Option<Entity>, EditError>`: atomic (validate everything, then mutate); returns the spawned entity for `Spawn`; `SetResource` on `ludenea::Time` is `ReadOnly`.
  - `pub enum ValueKind { Component, Resource }`; `Simulation::encode_json_value(&self, kind: ValueKind, name: &str, value: serde_json::Value) -> Result<Vec<u8>, EditError>`: JSON to canonical bytes through the codec.

- [ ] **Step 1: Write the failing tests**: `insert_replaces_existing_component`, `spawn_returns_next_free_entity_deterministically`, `despawn_of_dead_entity_is_no_such_entity`, `invalid_pose_rotation_is_rejected_and_world_unchanged` (hash before equals hash after), `unknown_component_name_is_rejected`, `time_resource_is_read_only`, `edit_log_round_trips_and_rejects_other_versions`, `edit_log_at_returns_edits_of_one_tick_in_record_order`, `json_round_trip_is_exact_for_f32_fields` (encode `Pose` to JSON and back, bytes equal), `non_finite_float_has_no_json_form`, `contact_cache_has_no_json_form_but_ron_works`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test edit`. Expected: compile failure.
- [ ] **Step 3: Implement** the codec additions, `remove_by_name`, and `edit.rs`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): record editor edits as a canonical edit log`.

---

### Task 3: Session timeline with logarithmic checkpoints

**Files:**
- Create: `crates/ludenea-control/src/{checkpoints.rs, session.rs}`
- Modify: `crates/ludenea-ecs/src/{snapshot.rs, world.rs}` (`World::restore` also clears the command queue and this tick's events)
- Test: inline tests in `checkpoints.rs`, `crates/ludenea-control/tests/session.rs`, `crates/ludenea-ecs/tests/snapshot.rs`

**Interfaces:**
- Consumes: Task 2 `EditLog`, `Simulation::apply_edit`; milestone 1 `InputLog`, `Snapshot`, `load_scene`, `save_scene`.
- Produces (`ludenea_ecs`): `World::restore` leaves no queued command and no event behind (they are always empty between ticks, so the golden trace is unchanged; after an aborted tick they are not).
- Produces (`ludenea_control::checkpoints`):
  - `pub const CHECKPOINT_INTERVAL: u64 = 16;` `pub const CHECKPOINTS_PER_LEVEL: usize = 2;`
  - `pub struct Checkpoints`: `new(initial: Snapshot) -> Self` (tick 0, never dropped), `wants(&self, tick: u64) -> bool` (`tick % CHECKPOINT_INTERVAL == 0` and not stored), `insert(&mut self, tick: u64, snapshot: Snapshot)` then thins relative to the new head: the newest checkpoint is always kept, and level `k` covers distances `[INTERVAL * 2^k, INTERVAL * 2^(k+1))` from the head and keeps at most `CHECKPOINTS_PER_LEVEL` checkpoints; `nearest_at_or_before(&self, tick: u64) -> (u64, &Snapshot)`, `truncate_after(&mut self, tick: u64)`, `ticks(&self) -> Vec<u64>`.
- Produces (`ludenea_control::session`):
  - **State invariant** (comment it at the top of `session.rs`): the world at tick `t` always includes the edits recorded at `t`. `hashes[t]` is the hash after `t` ticks, *before* the edits recorded at `t`; the checkpoint at `t` is taken at that same moment. Running tick `t + 1` therefore never applies edits of `t` again; after it runs, the session records `hashes[t + 1]` and the checkpoint, then applies the edits recorded at `t + 1`. Seeking to `t` restores the nearest checkpoint `c <= t`, applies the edits of `c`, and repeats "run tick, apply its edits" up to `t`.
  - `pub struct Session` holding the scene RON, `InputLog`, `EditLog`, `hashes: Vec<u64>`, `Checkpoints`, `head: u64` (last tick with a recorded input or edit), and `current: u64`. Ticks in `(simulated, head]` with `simulated = hashes.len() - 1` are the recorded future not yet simulated (after `LoadInputs` or `LoadEdits`).
  - `pub struct Timeline { pub head: u64, pub simulated: u64, pub current: u64, pub checkpoints: Vec<u64>, pub edit_ticks: Vec<u64>, pub can_undo: bool, pub can_redo: bool, pub scene_dirty: bool, pub resimulating: bool }` (Serialize).
  - Every method takes `sim: &mut Simulation` as its first argument after `self`, so `Host` owns both. Every tick runs through one private `run_tick` that wraps `Simulation::step_encoded` in `std::panic::catch_unwind(AssertUnwindSafe(..))` *before* the session touches its logs; on a panic it restores the state of `current` (nearest checkpoint plus replay) and returns `SessionError::Panicked { tick, message }`, where `tick` is `sim.tick()` before the failing step (the recovered state is at that tick). Native only: on `wasm32-unknown-unknown` the panic aborts the instance (Deviations).
    - `Session::start(sim, scene: String) -> Result<Session, SessionError>`: loads the scene, tick 0, empty logs.
    - `load_inputs(&mut self, sim, log: InputLog)` and `load_edits(&mut self, sim, log: EditLog)`: replace the recorded future after tick 0 and seek to 0; `head` becomes the last recorded tick.
    - `step(&mut self, sim, live: Option<&[u8]>) -> Result<u64, SessionError>`: below `head`, runs the recorded input of the next tick (or `live` when it takes over, below); at or past `head`, runs and records `live`, else the recorded input if any, else default inputs (milestone 1 behaviour past the end of a log; an app without an input type records nothing). Returns the new tick.
    - **Take over:** below `head`, `live` counts only when the caller passes it (the host passes it only when the raw input is not idle, Task 6) and its bytes differ from the recorded input of that tick; then everything after `current` is discarded (inputs, edits, hashes, checkpoints, and redo entries) and recording continues live.
    - `edit(&mut self, sim, edit: Edit) -> Result<Option<Entity>, SessionError>`: at tick 0, applies to the world, saves the scene text with `sim.save_scene()`, then reloads it with `sim.load_scene` (so entity indices and generations match what any later load gives), records `hashes[0]` again, keeps the recorded future, and marks it stale (below); the returned entity is the one in the reloaded world. Past tick 0, discards the future after `current`, records `TimedEdit { tick: current }`, and applies it now.
    - `seek(&mut self, sim, tick: u64) -> Result<(), SessionError>`: `tick <= simulated` (or `<= head`, simulating the recorded future on the way); beyond, `SessionError::InvalidTick { tick, head }`.
    - `is_pending(&self) -> bool`: edits recorded at `current` exist.
    - `scene(&self) -> &str`: the session's scene text (tick 0 state), which `SaveScene` returns at any tick.
    - **Stale future:** after a tick 0 edit, `replace_scene(&mut self, sim, scene: String)` (scene file changed on disk), or `Session::resume(sim, scene, inputs: InputLog, edits: EditLog, expected: Vec<u64>) -> Result<Session, SessionError>` (host restarted after a rebuild), checkpoints after 0 are dropped and `resimulating` is true. `resimulate(&mut self, sim, budget: u32) -> Resimulation` replays at most `budget` recorded ticks on the new state from the last resimulated point, compares with the old hashes, and returns `pub enum Resimulation { Idle, Progress { up_to: u64 }, Diverged { tick: u64, expected: u64, got: u64 }, EditDropped { tick: u64, reason: String }, Done }`; `Diverged` is reported once, then resimulation continues and the new hashes replace the old; a recorded edit that fails on the new state (dead entity, unregistered component) is removed from the log and reported as `EditDropped`. The visible state stays at `current` (resimulation works on restored snapshots and restores `current` before returning). While `resimulating`, `edit`, `seek` past the resimulated point, `undo`, `redo`, and `export` fail with `SessionError::Resimulating { up_to }`.
    - `export(&self) -> Result<SessionExport, SessionError>` with `pub struct SessionExport { pub scene: String, pub inputs: Vec<u8>, pub edits: Vec<u8>, pub hashes: Vec<u64> }`.
    - `timeline(&self) -> Timeline`.
  - `pub enum SessionError` (`thiserror`): `InvalidTick { tick, head }`, `Resimulating { up_to }`, `Panicked { tick, message }`, `NothingToUndo`, `NothingToRedo` (used by Task 4), `Edit(EditError)`, `Sim(SimError)`, `Scene(SceneError)`.

- [ ] **Step 1: Write the failing tests**: in `checkpoints.rs`, `tick_zero_and_head_are_always_kept`, `count_grows_logarithmically` (after 100,000 ticks, at most `CHECKPOINTS_PER_LEVEL * (log2(100000 / 16) + 1) + 2` checkpoints), `seek_cost_is_bounded_by_distance` (for every target `t` in a sample, `t - nearest_at_or_before(t).0 <= max(CHECKPOINT_INTERVAL, head - t)`); in `snapshot.rs`, `restore_clears_queued_commands_and_events`; in `tests/session.rs` with the `boxes` app (dev-dependency): `golden_inputs_through_session_match_golden_trace` (`load_inputs` with `boxes.inputs`, 10,000 steps, every hash equals `golden/boxes.trace`), `replaying_recorded_ticks_matches_original_hashes`, `seek_back_then_forward_reproduces_hashes`, `past_spawn_survives_seek_back_and_forth_without_duplicates` (spawn at 200, seek 100, seek 300, step: same hash and same crate count as recorded), `edit_in_past_discards_future_and_records_edit`, `pending_edit_shows_in_world_before_next_tick` (`is_pending`, world hash differs from `hashes[current]`), `idle_replay_keeps_future`, `live_input_differing_from_recorded_takes_over`, `seek_beyond_head_is_invalid_tick`, `tick_zero_despawn_reloads_scene_with_matching_indices` (the hash after the edit equals a fresh `load_scene` of `scene()`), `tick_zero_edit_resimulates_and_reports_first_divergence`, `edits_are_refused_while_resimulating`, `failed_recorded_edit_is_dropped_during_resimulation`, `export_then_fresh_replay_matches` (fresh `Simulation`, load exported scene, apply inputs and edits tick by tick, same hashes), `panic_restores_state_and_keeps_logs` (test app panicking when `Time.tick == 50`: `Panicked { tick: 50 }`, `current == 50`, hash equals `hashes[50]`, logs unchanged).
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** the restore change, `checkpoints.rs`, and `session.rs`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
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
  - `pub(crate) enum UndoEntry { Scene { before: String, after: String }, Run { edit: TimedEdit } }`; the session keeps an undo stack and a redo stack for its lifetime. A new edit clears the redo stack. Every discard of the future after tick `t` (edit in the past, take over) also removes the `Run` entries whose tick is after `t` from both stacks. `SessionExport` gains `undo: Vec<u8>` (postcard of both stacks) and `Session::resume` takes it back, so a rebuild keeps undo history.
  - `Session::undo(&mut self, sim) -> Result<(), SessionError>`: `Scene` restores `before` as the scene (reloaded like a tick 0 edit) and marks the future stale; `Run` removes that edit from the log, discards the future after its tick, seeks to its tick. `SessionError::NothingToUndo` when empty.
  - `Session::redo(&mut self, sim) -> Result<(), SessionError>`: reapplies the entry (for `Run`, seeks to its tick, discards the future after it, records the edit again). `SessionError::NothingToRedo` when empty.
  - `Session::mark_scene_saved(&mut self)`; `Timeline.scene_dirty` is true while the scene differs from its last saved text (the unsaved marker; saving stays explicit).

- [ ] **Step 1: Write the failing tests**: `undo_run_edit_restores_exact_hash` (edit at tick 300, play to 400, undo: at tick 300 with `hashes[300]`, no pending edit, head 300), `redo_reapplies_same_edit_and_hash`, `undo_scene_edit_restores_scene_text`, `new_edit_clears_redo`, `discarded_future_removes_its_undo_entries` (edit at 300, seek 100, edit at 100, undo twice: the second undo is `NothingToUndo`), `undo_survives_export_and_resume`, `undo_on_empty_stack_is_nothing_to_undo`, `scene_dirty_until_saved`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test undo`. Expected: compile failure.
- [ ] **Step 3: Implement** `undo.rs` and wire it into the session.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-control && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): undo and redo edits by resimulating the timeline`.

---

### Task 5: Debug draw primitives and picking

**Files:**
- Create: `crates/ludenea-debug-draw/{Cargo.toml, src/lib.rs, src/mesh.rs}`, `crates/ludenea-physics/src/editor.rs`
- Modify: `crates/ludenea-app/src/{app.rs, simulation.rs}` (editor hooks), `crates/ludenea-physics/src/{plugin.rs, lib.rs}`, facade (`pub use ludenea_debug_draw as debug_draw;`)
- Test: `crates/ludenea-physics/tests/editor.rs`, inline tests in `mesh.rs`

**Interfaces:**
- Produces (`ludenea_debug_draw`, no wgpu in this task), all `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]`:
  - `pub struct Color(pub u32)` (`0xRRGGBBAA`).
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum Primitive { Line { a: Vec2, b: Vec2, color: Color }, Circle { center: Vec2, radius: f32, color: Color, fill: bool }, Polygon { points: Vec<Vec2>, color: Color, fill: bool }, Capsule { a: Vec2, b: Vec2, radius: f32, color: Color }, Label { at: Vec2, text: String, color: Color } }`
  - `pub struct Item { pub entity: Option<Entity>, pub primitive: Primitive }`, `pub struct DebugDrawFrame { pub layers: BTreeMap<String, Vec<Item>> }` with `push(&mut self, layer: &str, entity: Option<Entity>, primitive: Primitive)`.
  - `mesh.rs` (pure Rust, tested natively, used by the renderer in Task 22): `pub const CIRCLE_SEGMENTS: usize = 32;` `pub struct Vertex { pub position: [f32; 2], pub color: u32 }`, `pub fn triangulate(frame: &DebugDrawFrame, layers: &BTreeSet<String>, selected: &BTreeSet<Entity>, line_width: f32) -> Vec<Vertex>` (triangle list; lines and outlines become quads of `line_width`; selected entities use a highlight color; labels are skipped, drawn by the overlay canvas).
- Produces (`ludenea_app`): `App::debug_draw(&mut self, layers: &[&'static str], draw: fn(&World, &BTreeSet<String>, &mut DebugDrawFrame)) -> &mut Self`, `App::picker(&mut self, pick: fn(&World, Vec2) -> Vec<Entity>) -> &mut Self`; `Simulation::debug_layers(&self) -> Vec<&'static str>` (sorted), `Simulation::debug_draw(&self, layers: &BTreeSet<String>) -> DebugDrawFrame` (calls every drawer, which skips unrequested layers), `Simulation::pick(&self, point: Vec2) -> Vec<Entity>` (union of all pickers, ascending, no duplicates).
- Produces (`ludenea_physics`): layers `"physics.colliders"`, `"physics.contacts"`, `"physics.aabbs"`; colliders drawn as outlines in their world pose (static gray, kinematic blue, dynamic green), contacts as points and normals, AABBs as rectangles. Picker: entities whose collider contains `point` (reuse `overlap_aabb` on a 1 cm box, then an exact point test), plus entities with `Pose` and no `Collider` within `PICK_RADIUS = 0.25` m.

- [ ] **Step 1: Write the failing tests**: `colliders_layer_draws_one_item_per_collider_with_entity`, `unrequested_layers_are_empty`, `pick_inside_box_returns_it`, `pick_outside_everything_returns_empty`, `pick_collider_less_pose_within_radius`, `debug_draw_and_pick_do_not_change_world_hash`; in `mesh.rs`: `filled_circle_has_32_triangles`, `capsule_outline_is_closed`, `selected_entity_uses_highlight_color`, `labels_produce_no_vertices`.
- [ ] **Step 2: Run** `cargo test -p ludenea-physics --test editor -p ludenea-debug-draw`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate, the hooks, and `physics/src/editor.rs`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(physics): draw colliders, contacts, and bounds, and pick bodies`.

---

### Task 6: Raw input mapping and the real-time pacer

**Files:**
- Create: `crates/ludenea-app/src/raw_input.rs`, `crates/ludenea-control/src/pacer.rs`
- Modify: `crates/ludenea-app/src/{app.rs, simulation.rs, lib.rs}`, `examples/boxes/src/lib.rs` (mapping: arrow keys give `velocity_change`, `KeyS` spawns at x = 0, `Delete` despawns)
- Test: `crates/ludenea-app/tests/raw_input.rs`, inline tests in `pacer.rs`

**Interfaces:**
- Produces (`ludenea_app`), `#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]`:
  - `pub struct RawInput { pub keys: BTreeSet<String>, pub gamepads: Vec<RawGamepad> }` (`keys` are `KeyboardEvent.code` values), `pub struct RawGamepad { pub buttons: Vec<f32>, pub axes: Vec<f32> }` (Gamepad API standard mapping).
  - `RawInput::is_idle(&self) -> bool`: no key, no button above 0.5, no axis beyond 0.2 in absolute value.
  - `App::input_mapping<I: Input>(&mut self, map: fn(&RawInput, usize) -> I) -> &mut Self` (the `usize` is the player index); a mapping for a type other than the app's input type is an `AppError` at build.
  - `Simulation::map_raw_input(&self, players: &[RawInput]) -> Result<Vec<u8>, SimError>`: the encoded `Vec<I>`, one per player; `SimError::NoInputMapping` without a mapping (message: register one with `App::input_mapping`).
- Produces (`ludenea_control::pacer`, no wall clock inside: callers pass elapsed time): `pub const MAX_TICKS_PER_FRAME: u32 = 4;` `pub struct Pacer` with `new(tick_rate: u32) -> Self`, `set_speed(&mut self, speed: f32)` (clamped to `[0.1, 8.0]`), `advance(&mut self, elapsed_seconds: f64) -> u32`: ticks due, at most `MAX_TICKS_PER_FRAME`; debt beyond that is dropped, so a slow simulation runs slower than real time and never skips ticks. `effective_rate(&self) -> f32`: ticks per second over the last second of passed time.

- [ ] **Step 1: Write the failing tests**: `boxes_mapping_turns_arrow_keys_into_velocity_change`, `idle_raw_input_maps_to_default`, `map_without_mapping_is_no_input_mapping`, `is_idle_ignores_small_axes`; `pacer_runs_one_tick_per_sixtieth_second`, `pacer_caps_ticks_and_drops_debt`, `speed_two_doubles_ticks`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** `raw_input.rs`, the app hook, and `pacer.rs`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(input): map raw keyboard and gamepad state and pace real-time play`.

---

### Task 7: Protocol version 2: the host on the session

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, in_process.rs, lib.rs}`, `crates/ludenea-cli/src/commands.rs` (only if a command shape changes)
- Test: `crates/ludenea-control/tests/{host.rs, running.rs}`

**Interfaces:**
- Consumes: Tasks 3, 4, 5, 6.
- Produces (`ludenea_control`), same serde conventions as milestone 1:
  - `PROTOCOL_VERSION = 2`. `ServerMessage::Welcome` gains `game: GameInfo`; new variant `ServerMessage::Event { event: Event }`.
  - `pub struct GameInfo { pub components: Vec<NamedSchema>, pub resources: Vec<NamedSchema>, pub input: Option<TypeSchema>, pub tick_rate: u32, pub debug_layers: Vec<String> }`, `pub struct NamedSchema { pub name: String, pub schema: TypeSchema }`.
  - `Host::new(app: App)` starts a session on an empty scene (the built world, no entities). Command semantics on the session: `LoadScene { ron }` = `Session::start`; `LoadInputs { log }` = `load_inputs`; new `LoadEdits { log: Bytes }` = `load_edits`; `Step { ticks, record_hashes }` = `ticks` × `Session::step(None)`; `Restore { snapshot }` restores the world, sets `current` to the snapshot's tick, drops hashes and checkpoints after it, and keeps the recorded inputs and edits as the future (so milestone 1's restore-and-replay stays identical); `SetThreads` is unchanged; `GetHash` replies `Hash { tick, hash, pending: bool }`.
  - New commands: `Play`, `Pause`, `Seek { tick: u64 }`, `SetSpeed { speed: f32 }`, `SetLiveInput { players: Vec<RawInput> }`, `Undo`, `Redo`, `GetTimeline` → `Timeline(Timeline)`, `ExportSession` → `Session { scene: String, inputs: Bytes, edits: Bytes, undo: Bytes, hashes: Vec<Hash64> }`, `ReplaceScene { ron: String }` (`Session::replace_scene`), `LoadSession { scene: String, inputs: Bytes, edits: Bytes, undo: Bytes, expected: Vec<Hash64> }` (`Session::resume`, used after a rebuild or a worker crash).
  - New `ErrorCode` variants: `InvalidTick`, `Resimulating`, `NothingToUndo`, `NothingToRedo`, `NoInputMapping`, `SystemPanicked`.
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum Event { Ticked { tick: u64, hash: Hash64, playing: bool, effective_rate: f32 }, Paused { reason: PauseReason }, TimelineChanged { timeline: Timeline }, Resimulated { up_to: u64 }, Diverged { tick: u64, expected: Hash64, got: Hash64 }, EditDropped { tick: u64, reason: String }, Console { level: ConsoleLevel, text: String } }` (Task 10 adds more), `pub enum PauseReason { User, Panicked { tick: u64, message: String } }`, `pub enum ConsoleLevel { Info, Warning, Error }`. The host emits `Console` for panics, dropped edits, and divergences.
  - `Host::handle(&mut self, command: Command) -> Result<Reply, ProtocolError>` stays synchronous; `Host::advance(&mut self, elapsed_seconds: f64) -> Vec<Event>` runs the pacer's due ticks while playing (live input is the mapped `SetLiveInput` value, passed to the session only when not idle), or a resimulation budget of 240 ticks while paused and resimulating, and returns the events. `SessionError::Panicked` from any path (`Step`, `Seek`, `Undo`, `Redo`, `advance`, resimulation) pauses with `PauseReason::Panicked` and replies `SystemPanicked`.

- [ ] **Step 1: Write the failing tests** in `running.rs` (boxes app): `cli_style_run_through_host_matches_golden` (`LoadScene`, `LoadInputs`, `Step { ticks: 10000, record_hashes: true }`, all hashes equal `golden/boxes.trace`), `restore_then_step_reproduces_hashes` (milestone 1 semantics), `play_then_advance_runs_ticks_and_emits_ticked`, `panicking_system_pauses_with_panicked_and_keeps_session` (system panicking when `Time.tick == 50`: `Paused { reason: Panicked { tick: 50, .. } }`, `GetTimeline` current 50, recorded inputs intact), `undo_and_redo_through_protocol`, `export_session_replays_in_fresh_host`, `load_session_with_changed_code_reports_diverged` (expected hashes with one value flipped at tick 30, then `advance` while paused: `Diverged { tick: 30, .. }`), `replace_scene_keeps_recorded_inputs`, `seek_while_resimulating_is_resimulating`; update `host.rs` for version 2 (`handshake_with_version_1_is_refused`, `welcome_carries_game_schemas`, every command, reply, and event round-trips through JSON).
- [ ] **Step 2: Run** `cargo test -p ludenea-control`. Expected: FAIL.
- [ ] **Step 3: Implement** the protocol additions and the host on top of `Session`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS, including the CLI and golden tests.
- [ ] **Step 5: Commit** `✨ feat(control): play, seek, and undo through protocol version 2`.

---

### Task 8: Editing commands

**Files:**
- Create: `crates/ludenea-control/src/{path.rs, json.rs}`
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs}`
- Test: `crates/ludenea-control/tests/editing.rs`

**Interfaces:**
- Consumes: Task 7 host, Task 2 JSON codec, Task 1 schemas.
- Produces (`ludenea_control`), `JsonValue` = `serde_json::Value`:
  - `json.rs`: `pub fn to_wire(value: JsonValue, schema: &TypeSchema) -> JsonValue` and `pub fn from_wire(value: JsonValue, schema: &TypeSchema) -> Result<JsonValue, String>`: integers outside `±2^53` become decimal strings on the way out and are parsed back on the way in, guided by the schema; every value the host sends or receives goes through them.
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum PathSegment { Field { name: String }, Index { index: u32 } }`; `pub fn set_at_path(root: &mut JsonValue, path: &[PathSegment], value: JsonValue) -> Result<(), String>` in `path.rs`. An enum variant's content is the object key of its name; a newtype variant's content is that key's value directly (`Collider.shape` as `{"Circle": {"center": .., "radius": ..}}` takes the path `[shape, Circle, radius]`).
  - `pub enum FieldTarget { Component { entity: Entity, component: String }, Resource { resource: String } }`.
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum ComponentValue { Json { value: JsonValue }, Ron { text: String } }` (RON when `to_json` fails).
  - Commands: `GetEntity { entity }` → `Entity { entity, components: BTreeMap<String, ComponentValue> }`; `GetResources` → `Resources { resources: BTreeMap<String, ComponentValue> }`; `Spawn { components: BTreeMap<String, JsonValue> }` → `Spawned { entity }`; `Despawn { entity }`; `InsertComponent { entity, component: String, value: JsonValue }` (adding a component; editing goes through `SetField`); `RemoveComponent { entity, component: String }`; `SetField { target: FieldTarget, path: Vec<PathSegment>, value: JsonValue }` (reads the exact current JSON, applies `set_at_path`, encodes through `Simulation::encode_json_value`, records an `Insert` or `SetResource` edit); `SaveScene` → `Scene { ron: String }` (`Session::scene()` at any tick, then `mark_scene_saved`); `ApplyToScene { entity, component: String }` (copies the current value of that component into the session's scene when the entity exists in it at tick 0, as an undoable scene edit; otherwise `NoSuchEntity` with a message saying the entity was spawned after tick 0).
  - New `ErrorCode` variants: `InvalidEdit`, `UnknownComponent`, `UnknownResource`, `InvalidPath`. Every error message names the entity, component, and path involved.

- [ ] **Step 1: Write the failing tests** in `editing.rs` (boxes app): `set_field_on_pose_position_records_insert_edit`, `set_field_with_non_unit_rotation_is_invalid_edit_and_records_nothing` (message contains `ludenea::Pose` and `rotation`; world hash unchanged), `set_field_on_dead_entity_is_no_such_entity`, `set_field_bad_path_is_invalid_path`, `set_field_through_newtype_variant_changes_circle_radius`, `set_field_on_time_is_invalid_edit`, `rng_state_round_trips_as_decimal_string` (`GetResources` shows `ludenea::Rng.state` as a string; `SetField` with a string sets it exactly), `spawn_from_json_components_returns_entity`, `contact_cache_is_ron_value`, `save_scene_returns_session_scene_at_any_tick` (after 100 ticks, equals the scene at tick 0), `apply_to_scene_copies_value_and_marks_dirty`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test editing`. Expected: FAIL.
- [ ] **Step 3: Implement** `json.rs`, `path.rs`, and the commands.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): edit components and resources through protocol version 2`.

---

### Task 9: ECS introspection hooks

**Files:**
- Create: `crates/ludenea-ecs/src/{filter.rs, observer.rs}`
- Modify: `crates/ludenea-ecs/src/{schedule.rs, system.rs, commands.rs, world.rs, lib.rs}`, `crates/ludenea-app/src/{app.rs, simulation.rs}` (`event::<T>` requires `T: Serialize`), `crates/ludenea-physics/src/events.rs` (`Collision` and `CollisionKind` derive `Serialize`), event types in existing ECS and app tests
- Test: `crates/ludenea-ecs/tests/{filter.rs, observer.rs}`

**Interfaces:**
- Produces (`ludenea_ecs`):
  - `pub struct EntityFilter`; `EntityFilter::parse(text: &str, world: &World) -> Result<EntityFilter, FilterError>`: comma-separated `With<Name>` and `Without<Name>`, where `Name` is a stable name or its last segment when unique; empty text matches every entity. `FilterError` names the unknown or ambiguous component and lists candidates. `World::filter_entities(&self, filter: &EntityFilter) -> Vec<Entity>` (ascending).
  - `pub struct SystemInfo { pub name: &'static str, pub reads: Vec<(&'static str, &'static str)>, pub writes: Vec<(&'static str, &'static str)> }` (kind, name); `Schedule::systems(&self, world: &World) -> Vec<SystemInfo>` in run order.
  - `pub trait ScheduleObserver { fn before_system(&mut self, index: usize); fn after_system(&mut self, index: usize, commands: &[CommandRecord]); fn tick_end(&mut self, world: &World); }`, `pub struct CommandRecord { pub kind: CommandKind, pub entity: Option<Entity>, pub component: Option<&'static str> }`, `pub enum CommandKind { Spawn, Despawn, Insert, Remove }`. `Schedule::run_observed(&mut self, world: &mut World, observer: &mut dyn ScheduleObserver)`; plain `run` behaves as before. Records are built only when observed.
  - `World::events_json(&self) -> Vec<(&'static str, Vec<serde_json::Value>)>` (event type name, events of this tick).
- Produces (`ludenea_app`): `Simulation::step_observed(&mut self, inputs: Option<&[u8]>, observer: &mut dyn ScheduleObserver) -> Result<(), SimError>`; `Simulation::systems(&self) -> Vec<SystemInfo>`.

- [ ] **Step 1: Write the failing tests**: `filter_with_and_without_by_short_and_full_name`, `ambiguous_short_name_lists_candidates`, `empty_filter_matches_all`; `observer_sees_systems_in_order_and_their_commands`, `observer_sees_this_tick_events_before_they_clear`, `unobserved_run_hashes_equal_observed_run_hashes` (600 boxes ticks, in an app test).
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs -p ludenea-app`. Expected: compile failure.
- [ ] **Step 3: Implement** filters and the observer.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(ecs): filter entities and observe systems, commands, and events`.

---

### Task 10: Introspection commands and subscriptions

**Files:**
- Create: `crates/ludenea-control/src/subscriptions.rs`
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs}`
- Test: `crates/ludenea-control/tests/introspection.rs`

**Interfaces:**
- Consumes: Task 9 filters and observer, Task 5 debug draw and pick, Task 8 `ComponentValue` and `json.rs`.
- Produces (`ludenea_control`):
  - Commands: `QueryEntities { filter: String, offset: u32, limit: u32 }` → `Entities { total: u32, rows: Vec<EntityRow> }` with `pub struct EntityRow { pub entity: Entity, pub hash: Hash64 }` (replaces `ListEntities`); `GetSchedule` → `Schedule { systems: Vec<SystemRow> }` with `pub struct SystemRow { pub name: String, pub reads: Vec<String>, pub writes: Vec<String>, pub micros: Option<f32> }` (`"kind name"` strings; `micros` = mean over the last 60 observed ticks, measured with `Instant` on native, always `None` on `wasm32-unknown-unknown`); `GetTickEvents { tick }` → `TickEvents { events: Vec<RecordedEvent> }` with `pub struct RecordedEvent { pub event_type: String, pub value: JsonValue }`; `GetTickCommands { tick }` → `TickCommands { sync_points: Vec<SyncPoint> }` with `pub struct SyncPoint { pub after_system: String, pub commands: Vec<String> }` (for example `"insert ludenea::Body on 12v0"`); `GetDebugDraw { layers: Vec<String> }` → `DebugDraw { tick, frame: DebugDrawFrame }` (the current tick); `Pick { point: Vec2 }` → `Picked { entities: Vec<Entity> }`; `Subscribe { topic: Topic }`, `Unsubscribe { topic: Topic }`.
  - `#[serde(tag = "type", rename_all = "snake_case")] pub enum Topic { Entities { filter: String }, Entity { entity: Entity }, Resources, Introspection }`. `Introspection` turns on the observer and keeps events, commands, and timings for the last `INTROSPECTION_TICKS = 600` ticks run while subscribed; outside that window `GetTickEvents` and `GetTickCommands` fail with `InvalidTick`.
  - New `ErrorCode` variant: `InvalidFilter` (message from `FilterError`, with candidates).
  - New events: `EntitiesChanged { filter: String, total: u32 }`, `EntityChanged { entity, components: BTreeMap<String, ComponentValue>, removed: Vec<String> }` (only components whose hash changed since the last push), `EntityDespawned { entity }`, `ResourcesChanged { resources: BTreeMap<String, ComponentValue> }`. Deltas are computed at most once per `Host::advance` call and once after each editing or timeline command, never per tick, from per-component hashes.

- [ ] **Step 1: Write the failing tests**: `query_entities_pages_and_reports_total`, `invalid_filter_lists_candidates`, `schedule_lists_accesses_and_timings_after_introspection`, `tick_events_contain_collision_begin`, `tick_commands_name_spawn_after_apply_inputs`, `tick_outside_window_is_invalid_tick`, `entity_subscription_pushes_only_changed_components`, `seek_pushes_entity_despawned_for_entities_not_yet_spawned`, `debug_draw_and_pick_through_protocol`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test introspection`. Expected: compile failure.
- [ ] **Step 3: Implement** the commands and the subscriptions.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): explore entities, systems, events, and commands per tick`.

---

### Task 11: TypeScript protocol types

**Files:**
- Create: `.cargo/config.toml` (`[env]` `TS_RS_EXPORT_DIR = { value = "editor/packages/control-client/src/generated", relative = true }`, `TS_RS_LARGE_INT = "number"`), `editor/packages/control-client/src/generated/` (generated), `crates/ludenea-control/tests/typescript.rs`
- Modify: `crates/ludenea-control/{Cargo.toml, src/protocol.rs}` (`outcome::Wire` becomes public with a `TS` derive, `Response.result` carries `#[ts(as = "outcome::Wire")]`), and an optional `ts` feature enabling `ts_rs::TS` derives in `ludenea-schema`, `ludenea-ecs` (`Entity`, `AssetId`, `HashTree`, `NamedHash`, `EntityHash`), `ludenea-debug-draw`, `ludenea-app` (`RawInput`, `RawGamepad`), `ludenea-math` (`Vec2`); `ludenea-control` enables them

**Interfaces:**
- Produces: one `.ts` file per exported type under `editor/packages/control-client/src/generated/`, plus `index.ts` re-exporting them (written by the test). Annotations follow the ts-rs Global Constraint.
- `cargo test -p ludenea-control --test typescript` regenerates the directory.

- [ ] **Step 1: Write the test** `export_typescript_bindings`: exports every protocol type, writes `index.ts`, then asserts that `ClientMessage.ts`, `ServerMessage.ts`, `Command.ts`, `Event.ts`, and `TypeSchema.ts` exist, that `Command.ts` contains `"type": "set_field"`, and that `Hash64` is declared as `string`.
- [ ] **Step 2: Run** it. Expected: compile failure (no derives).
- [ ] **Step 3: Implement** the derives, annotations, and the `index.ts` writer.
- [ ] **Step 4: Run** `cargo test --locked -p ludenea-control --test typescript && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS, files generated.
- [ ] **Step 5: Commit** `✨ feat(control): generate TypeScript types for the protocol`.

---

### Task 12: Native game host over stdio, CLI edit logs

**Files:**
- Create: `crates/ludenea-control/src/stdio.rs` (`cfg(not(target_family = "wasm"))`)
- Modify: `crates/ludenea-cli/src/{lib.rs, commands.rs}`
- Test: `crates/ludenea-control/tests/stdio.rs`, `crates/ludenea-cli/tests/cli.rs`

**Interfaces:**
- Produces (`ludenea_control::stdio`): `pub fn serve_stdio(app: fn() -> App, input: impl BufRead + Send + 'static, output: impl Write) -> std::io::Result<()>` (callers pass `BufReader<Stdin>`, not a lock): a reader thread parses one `ClientMessage` JSON per line into a channel; the main loop waits at most 1/60 s for messages, handles them, calls `Host::advance` with the elapsed time measured by `Instant`, and writes one `ServerMessage` JSON per line, flushing after each batch. A malformed line produces `Refused` with code `NotConnected` before the handshake, or a `Response` error when an id can be read; it never panics. End of input ends the loop with `Ok`. Protocol lines always start with `{"type":`; anything else a game prints on stdout is treated as console output by the reader on the other side (Task 16).
- Produces (`ludenea_cli`): subcommand `host --stdio` (native only, the clap variant is gated too) calling `serve_stdio` on stdin and stdout; `run` and `verify` accept `--edits <file>`; when given, `inputs_hash` = xxh3 of the input log bytes followed by the edit log bytes (Deviations).

- [ ] **Step 1: Write the failing tests**: `stdio_handshake_and_step_over_pipes` (in-memory pipes, `Hello` then `Step`), `malformed_line_gets_error_not_panic`, `playing_emits_ticked_events_in_real_time` (play for 0.5 s: between 10 and 60 ticks); in `cli.rs`: `verify_with_edits_replays_exported_session` (build a session with edits through `InProcess`, export, write files, `run --trace` then `verify --golden` both with `--edits` pass, and the trace hashes equal the exported `hashes`), `golden_trace_still_verifies_without_edits`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control -p ludenea-cli`. Expected: compile failure.
- [ ] **Step 3: Implement** `stdio.rs` and the CLI changes.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo build --locked --release --target wasm32-wasip1 -p boxes --bin boxes`. Expected: PASS (the WASI build still compiles).
- [ ] **Step 5: Commit** `✨ feat(cli): host the game over stdio and replay edit logs`.

---

### Task 13: Editor workspace and the control client

**Files:**
- Create: `editor/{package.json, pnpm-workspace.yaml, tsconfig.base.json, vitest.workspace.ts}`, `editor/packages/control-client/{package.json, tsconfig.json, src/index.ts, src/client.ts, src/project.ts, src/transports/{transport.ts, websocket.ts, post-message.ts, tauri.ts, fake.ts}}`, `editor/packages/control-client/test/{client.test.ts, project.test.ts}`
- Modify: `.gitignore` (`editor/**/dist/`), `README.md` (editor development commands)

**Interfaces:**
- Consumes: Task 11 generated types.
- Produces (`@ludenea/control-client`):
  - `export interface MessageTransport { send(message: string): void; onMessage(listener: (message: string) => void): () => void; onClose(listener: (reason: string) => void): () => void; close(): void }`; implementations `webSocketTransport(url: string): Promise<MessageTransport>`, `postMessageTransport(port: MessagePort | Worker): MessageTransport`, `tauriTransport(channel: "control" | "project"): Promise<MessageTransport>` (outgoing through `invoke("send", { channel, message })`, incoming through a `Channel<string>` passed to `invoke("subscribe", { channel, onMessage })`), `fakeTransport(handler: (message: string) => string[]): MessageTransport` (tests).
  - `export class ControlClient { static connect(transport: MessageTransport): Promise<ControlClient>` (sends `Hello` with `PROTOCOL_VERSION = 2`, rejects with the `Refused` message) `; readonly game: GameInfo; request<C extends Command>(command: C): Promise<ReplyFor<C>>; on<T extends Event["type"]>(type: T, listener: (event: Extract<Event, { type: T }>) => void): () => void; close(): void }`. Requests carry increasing ids; responses resolve by id; `{ error }` results reject with `ProtocolFailure extends Error { code: ErrorCode }`. `ReplyFor<C>` maps each command type to its reply variant.
  - `export class ProjectClient` with the same shape over the project protocol (types arrive in Task 18; until then `src/project.ts` is typed against a local placeholder `ProjectRequest = { type: string }`, replaced in Task 18).
- Scripts in `editor/package.json`: `test` (Vitest), `typecheck` (`tsc -b`), `build`, `e2e` (Playwright, configured in Task 14).

- [ ] **Step 1: Write the failing tests** in `client.test.ts`: `connect_rejects_version_mismatch_with_message`, `responses_resolve_by_id_out_of_order`, `error_result_rejects_with_code`, `events_reach_typed_listeners_and_unsubscribe_works`, `close_rejects_pending_requests`, `transport_close_rejects_pending_requests_with_reason`.
- [ ] **Step 2: Run** `pnpm -C editor install && pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the clients and transports.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add the typed control client and its transports`.

---

### Task 14: WebAssembly game host in a Web Worker, browser golden check

**Files:**
- Create: `crates/ludenea-control/src/worker.rs` (`cfg(all(target_arch = "wasm32", target_os = "unknown"))`), `xtask/{Cargo.toml, src/main.rs}` (workspace member), `.cargo/config.toml` alias `xtask = "run --locked -p xtask --"`, `editor/packages/control-client/src/worker/{host-worker.ts, supervisor.ts}`, `editor/e2e/{playwright.config.ts, golden.spec.ts, golden/index.html, golden/main.ts, trace.ts}`
- Modify: facade (`pub use wasm_bindgen;` and `macro_rules! worker_entry` under the browser gate), `examples/boxes/{Cargo.toml (crate-type ["cdylib", "rlib"]), src/lib.rs (ludenea::worker_entry!(app);)}`, `editor/package.json` (`e2e` script, `@playwright/test`)
- Test: `editor/e2e/golden.spec.ts`, `editor/packages/control-client/test/supervisor.test.ts`

**Interfaces:**
- Produces (`ludenea_control::worker`): `#[wasm_bindgen] pub struct WorkerHost` with `handle(&mut self, message: &str) -> String` (JSON array of `ServerMessage`) and `advance(&mut self, elapsed_ms: f64) -> String` (JSON array of `Event` messages); `WorkerHost::new(app: fn() -> App) -> WorkerHost` (Rust only). It installs a panic hook that posts `{"type":"crashed","tick":<tick>,"message":"..."}` to the worker scope before the instance aborts.
- Produces (facade): `ludenea::worker_entry!(path::to::app)` expands to `#[ludenea::wasm_bindgen::prelude::wasm_bindgen(crate = "ludenea::wasm_bindgen", js_name = createHost)] pub fn create_host() -> WorkerHost` under the browser gate and to nothing elsewhere.
- Produces (`xtask`): `wasm-game <package> <out-dir>`: `cargo build --locked --release --target wasm32-unknown-unknown -p <package> --lib`, then `wasm-bindgen-cli-support` with the `web` target into `<out-dir>` (`game.js`, `game_bg.wasm`). `wasm-renderer <out-dir>` is added in Task 22.
- Produces (`host-worker.ts`): a module worker; first message `{ type: "init", glueUrl: string }` imports the glue, calls its default init, then `createHost()`; later messages are protocol JSON strings passed to `handle`; a `setInterval` of 16 ms calls `advance` with the elapsed time from `performance.now()` and posts each resulting message; a thrown `WebAssembly.RuntimeError` posts `{ type: "crashed" }` if the hook did not.
- Produces (`supervisor.ts`): `export function startWorkerGame(options: { workerUrl: URL; glueUrl: string }): WorkerGame` with `interface WorkerGame { transport: MessageTransport; onCrashed(listener: (crash: { tick?: number; message: string }) => void): () => void; terminate(): void }`. Recovery policy lives in the app (Task 24).
- Playwright: `editor/e2e/playwright.config.ts` starts a `webServer` (Vite serving `editor/e2e/golden/`, with `examples/boxes/assets/` and `golden/` mounted as static directories), Chromium with `--use-angle=swiftshader`, timeouts of 10 minutes for tests that build. Install with `pnpm -C editor exec playwright install chromium`.
- `trace.ts`: `export function decodeTrace(bytes: Uint8Array): { version: number; sceneHash: bigint; inputsHash: bigint; hashes: bigint[] }` (postcard varints, decoded as `BigInt`), reused in Task 25.

- [ ] **Step 1: Write the failing tests**: `golden.spec.ts` (`boxes in a headless browser matches the golden trace`): the page loads the worker with the boxes glue, sends `LoadScene`, `LoadInputs`, `Step { ticks: 10000, record_hashes: true }`, decodes `golden/boxes.trace`, and asserts all 10,001 hashes (initial `GetHash` plus 10,000 recorded) equal the golden; `supervisor.test.ts` (fake worker): `crash_message_reaches_listener`, `runtime_error_without_hook_reports_crash`.
- [ ] **Step 2: Run** `cargo xtask wasm-game boxes editor/e2e/golden/pkg && pnpm -C editor e2e golden`. Expected: FAIL (no worker host).
- [ ] **Step 3: Implement** `worker.rs`, the macro, `xtask`, the worker module, and the supervisor.
- [ ] **Step 4: Run** the same commands plus `pnpm -C editor test`. Expected: PASS. On a hash mismatch, stop and debug with snapshots and `diff` (systematic-debugging skill); never re-record the golden.
- [ ] **Step 5: Commit** `✅ test(boxes): verify the golden trace in a headless browser worker`.

---

### Task 15: Projects, metas, and `ludenea new`

**Files:**
- Create: `crates/ludenea-project/{Cargo.toml, src/lib.rs, src/manifest.rs, src/meta.rs, src/files.rs, src/template.rs, template/**}`, `crates/ludenea-serve/{Cargo.toml, src/main.rs}` (binary `ludenea`, subcommand `new` only in this task)
- Test: `crates/ludenea-project/tests/{project.rs, metas.rs}`

**Interfaces:**
- Produces (`ludenea_project`):
  - `ludenea.toml`: `pub struct Manifest { pub name: String, pub engine: String /* version */, pub default_scene: Option<String> }` (TOML, `toml` crate).
  - `pub struct Project` with `Project::open(root: &Path) -> Result<Project, ProjectError>` (canonicalizes `root`), `Project::create(parent: &Path, name: &str, engine_path: &Path) -> Result<Project, ProjectError>` writing the template: `ludenea.toml`; `Cargo.toml` with `crate-type = ["cdylib", "rlib"]` and `ludenea = { path = <engine_path>/crates/ludenea }`; `rust-toolchain.toml` (same channel as the engine, targets `wasm32-unknown-unknown`); `src/lib.rs` with `pub fn app()` (physics plugin, an input type with a mapping) and `ludenea::worker_entry!(app);`; `src/main.rs` calling `ludenea::cli::main`; `scenes/main.scene.ron` with a static floor and three dynamic boxes; `editor/package.json` (name and version only, no dependencies); `editor/index.ts` with an empty `definePlugin`; `editor/tsconfig.json` pointing at `.ludenea/types/`; `.gitignore` with `.ludenea/` and `target/`. Names must match `^[a-z][a-z0-9_]*$` (`ProjectError::InvalidName` explains the rule).
  - Metas: each file under `scenes/` and `assets/` has `<file>.meta` (RON `Meta(version: 1, id: "<32 hex>", kind: Scene | Other)`); `id` = `AssetId(uuid::Uuid::new_v4().as_u128())`. `Project::files(&self) -> Vec<FileEntry>` with `pub struct FileEntry { pub path: String /* relative, forward slashes */, pub id: AssetId, pub kind: AssetKind }`; listing creates missing metas. `Project::rename(&mut self, from: &str, to: &str)`, `Project::delete(&mut self, path: &str)` move or delete the meta with its file. An orphan meta (no sibling file) produces `ProjectWarning::OrphanMeta { id, path }` once and is never deleted automatically.
  - `Project::read_text(&self, path: &str) -> Result<String, ProjectError>`, `write_text(&mut self, path: &str, text: &str)`; every path is checked to stay inside the project root (rejects `..` and absolute paths with `ProjectError::OutsideProject`).
  - `Project::watch(&self) -> Result<Receiver<FileChange>, ProjectError>` (notify-debouncer-full, 100 ms): `pub enum FileChange { Changed(String), Created(String), Removed(String) }` with paths relative to the canonical root, ignoring `.ludenea/` and `target/`; a remove followed by a create of the same path within the debounce window (atomic save) is reported as `Changed`.
- Produces (binary `ludenea`): `ludenea new <dir> --engine-path <path>`.

- [ ] **Step 1: Write the failing tests**: `create_writes_template_that_opens`, `invalid_name_is_rejected_with_rule`, `listing_creates_missing_metas_and_keeps_ids_stable`, `rename_moves_meta_with_file`, `delete_removes_meta_with_file`, `file_moved_outside_editor_gets_new_id_and_orphan_warning_once`, `paths_outside_project_are_rejected`, `watch_reports_scene_change_on_disk`, `atomic_save_is_reported_as_changed`, `template_builds_and_runs` (`#[ignore]`: `cargo build` in a temp project created from this repository, then `cargo run -- run scenes/main.scene.ron --ticks 60 --hash`).
- [ ] **Step 2: Run** `cargo test -p ludenea-project`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate and `ludenea new`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-project -- --ignored && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(project): create and open projects with stable asset identifiers`.

---

### Task 16: Builds, game host processes, and game types

**Files:**
- Create: `crates/ludenea-project/src/{build.rs, game.rs, types.rs}`, `crates/ludenea-project/types/{editor-shell.d.ts, control-client.d.ts, react.d.ts}` (placeholders replaced in Task 19)
- Test: `crates/ludenea-project/tests/build.rs` (all `#[ignore]`)

**Interfaces:**
- Consumes: Task 15 `Project`, Task 12 `host --stdio`, Task 1 `typescript_declarations`, Task 7 `GameInfo`, Task 14 `worker_entry!`.
- Produces:
  - `pub enum BuildTarget { Native, Wasm }`. `Project::build(&self, target: BuildTarget, output: Sender<BuildEvent>) -> JoinHandle<Result<BuildArtifact, BuildError>>`: `Native` runs `cargo build --release --bin <package> --message-format=json-render-diagnostics`; `Wasm` runs `cargo build --release --lib --target wasm32-unknown-unknown` with the same message format, then `wasm-bindgen-cli-support` as a library (`web` target). Messages are parsed with `cargo_metadata`. `pub enum BuildEvent { Line { text: String }, Progress { done: u32, total: Option<u32> }, Diagnostic(Diagnostic), Finished { ok: bool } }` (`total` = artifacts of the previous build of this target, kept in `.ludenea/build/<target>.count`, `None` the first time); `pub struct Diagnostic { pub level: String, pub message: String, pub file: Option<String>, pub line: Option<u32>, pub column: Option<u32> }`. `pub enum BuildArtifact { Native { binary: PathBuf }, Wasm { dir: PathBuf } }` where the wasm `dir` is `.ludenea/build/wasm/<hex of xxh3 of game_bg.wasm>/` holding `game.js` and `game_bg.wasm`.
  - `pub struct GameProcess`: `GameProcess::start(binary: &Path) -> Result<GameProcess, ProjectError>` spawns `<binary> host --stdio`; `send(&mut self, line: &str)`; `take_messages(&mut self) -> Receiver<String>` (stdout lines starting with `{"type":`); `take_output(&mut self) -> Receiver<GameOutput>` with `pub struct GameOutput { pub stream: OutputStream /* Stdout | Stderr */, pub line: String }` (other stdout lines and all stderr lines); `wait(&mut self) -> Option<i32>` (exit code once exited; the message receiver disconnects at exit); `stop(self)`.
  - `Project::write_game_types(&self, info: &GameInfo) -> Result<(), ProjectError>` writes `.ludenea/types/game.d.ts` (`typescript_declarations` wrapped in `declare module "@ludenea/editor-shell" { interface ComponentTypes {...} }`) plus copies of the embedded `editor-shell.d.ts`, `control-client.d.ts`, and `react.d.ts`.
  - Every type crossing the project protocol (`BuildEvent`, `Diagnostic`, `GameOutput`, `FileEntry`, `FileChange`, `AssetKind`) derives `Serialize`, `Deserialize`, and `TS` (ts feature).

- [ ] **Step 1: Write the failing tests**: `native_build_produces_binary_and_reports_progress`, `build_error_reports_file_and_line` (template with a type error in `src/lib.rs`), `wasm_build_produces_glue_and_wasm_in_hashed_dir`, `game_process_handshakes_over_stdio`, `game_println_is_output_not_protocol`, `game_types_declare_template_components`.
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test build -- --ignored`. Expected: compile failure.
- [ ] **Step 3: Implement** `build.rs`, `game.rs`, `types.rs`.
- [ ] **Step 4: Run** the same command plus `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(project): build games natively and to WebAssembly and run their hosts`.

---

### Task 17: Embedded plugin bundler and hot reload

**Files:**
- Create: `crates/ludenea-project/src/plugins.rs`, `crates/ludenea-project/tests/fixtures/engine-plugin/{Cargo.toml, src/lib.rs, editor/index.ts}`
- Test: `crates/ludenea-project/tests/plugins.rs`

**Interfaces:**
- Consumes: Task 15 `Project::watch`.
- Produces:
  - `pub struct PluginBundle { pub name: String, pub url_path: String /* plugins/<name>.<hash>.js */, pub source: PluginSource }`, `pub enum PluginSource { Project, Engine { crate_name: String } }` (Serialize, TS).
  - `Project::bundle_plugins(&self) -> Result<Vec<PluginBundle>, Vec<Diagnostic>>`: entries are `editor/index.ts` or `editor/index.tsx` of the project (name = project name) and of every dependency found through `cargo metadata` that has one (name = crate name). rolldown runs embedded on a current-thread tokio runtime: ESM output, TypeScript stripped without type checking, JSX with the automatic runtime and `development: false` (imports `react/jsx-runtime`, never `react/jsx-dev-runtime`), the shared specifiers external, and a `resolve_id` plugin that rejects any other bare import with a diagnostic naming the import and stating that 2a plugins may import only React, the editor packages, and their own files (so a local `node_modules` is never bundled silently). Bundles are written to `.ludenea/plugins/`.
  - `Project::watch_plugins(&self) -> Receiver<PluginEvent>`: `pub enum PluginEvent { Rebuilt(PluginBundle), Failed { name: String, diagnostics: Vec<Diagnostic> } }`, rebundling only the plugin whose sources changed.

- [ ] **Step 1: Spike** (at most one hour): a scratch test that bundles a two-file TSX entry with `react` external through the rolldown crates, pinning every `rolldown*` crate to the same exact version. If the API cannot do externals, TSX, and a resolve hook, stop and report back with the failing piece; the fallback decided in advance is `oxc` (transform only, one file per plugin entry, relative imports rejected) and requires the author's approval because it narrows the plugin API.
- [ ] **Step 2: Write the failing tests**: `bundles_tsx_plugin_with_shared_imports_external` (output contains `from "react/jsx-runtime"` and no React source), `index_ts_and_index_tsx_are_both_entries`, `third_party_import_is_rejected_with_rule`, `syntax_error_reports_file_and_line`, `engine_plugin_of_dependency_is_discovered` (the fixture crate as a path dependency of a temp project), `editing_plugin_source_emits_rebuilt_with_new_hash`.
- [ ] **Step 3: Run** `cargo test -p ludenea-project --test plugins`. Expected: FAIL.
- [ ] **Step 4: Implement** `plugins.rs`.
- [ ] **Step 5: Run** `cargo test --locked -p ludenea-project && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 6: Commit** `✨ feat(project): bundle editor plugins without Node.js and rebuild on change`.

---

### Task 18: Project protocol and `ludenea serve`

**Files:**
- Create: `crates/ludenea-project/src/{protocol.rs, service.rs}`, `crates/ludenea-project/tests/typescript.rs`, `crates/ludenea-serve/src/serve.rs`
- Modify: `crates/ludenea-serve/src/main.rs`, `editor/packages/control-client/src/project.ts` (generated types replace the placeholder)
- Test: `crates/ludenea-project/tests/service.rs`, `crates/ludenea-serve/tests/serve.rs`

**Interfaces:**
- Produces (`ludenea_project::protocol`), envelope shaped like the control protocol: `PROJECT_PROTOCOL_VERSION = 1`, `ProjectClientMessage { Hello { protocol_version }, Request { id, request: ProjectRequest } }`, `ProjectServerMessage { Welcome { protocol_version }, Refused { error }, Response { id, result }, Event { event: ProjectEvent } }`.
  - `ProjectRequest`: `Create { parent: String, name: String, engine_path: Option<String> }` (falls back to the service's configured engine path, else `ProjectError::NoEnginePath` saying to set the editor setting `ludenea.engine-path`), `Open { path: String }`, `Info` → `ProjectInfo { name, root, default_scene, mode: ServiceMode /* Desktop | Browser */ }`, `ListFiles` → `Files { entries: Vec<FileEntry> }`, `ReadText { path }` → `Text { text }`, `WriteText { path, text }`, `Rename { from, to }`, `Delete { path }`, `Build` (target chosen by the mode), `ListPlugins` → `Plugins { plugins: Vec<PluginBundle> }`, `GameEndpoint` → `Native` (desktop: the backend relays the control channel) or `Wasm { glue_url: String }`, `WriteGameTypes { info: GameInfo }` (sent by the front end after each control handshake, in both modes).
  - `ProjectEvent`: `FilesChanged { changes: Vec<FileChange> }`, `Build(BuildEvent)`, `GameReady`, `GameExited { code: Option<i32> }`, `GameOutput(GameOutput)`, `PluginRebuilt(PluginBundle)`, `PluginFailed { name, diagnostics }`, `Warning { message }`.
  - `tests/typescript.rs` exports these types to `editor/packages/control-client/src/generated/project/` (the same staleness check covers them).
- Produces (`ludenea_project::service`): `pub struct ProjectService` with `new(mode: ServiceMode, engine_path: Option<PathBuf>) -> Self`, `handle(&mut self, request: ProjectRequest) -> Result<ProjectReply, ProjectFailure>`, `subscribe(&self) -> tokio::sync::broadcast::Receiver<ProjectEvent>` (several clients); after `Open`, it builds in the background and, in desktop mode, starts the `GameProcess` when the build succeeds, keeping the previous game on failure in both modes (browser mode keeps serving the previous wasm directory); `take_control_channel(&mut self) -> Option<(Sender<String>, Receiver<String>)>` (desktop, for the relay).
- Produces (binary `ludenea`): `ludenea serve <project> [--port 7878] [--host 127.0.0.1] [--editor-dir <dir>] [--engine-path <path>]` (axum with the `ws` feature, tokio): `GET /` and assets from the editor build (embedded with `include_dir` from `editor/apps/web/dist` behind feature `embed-editor`, otherwise `--editor-dir`), `GET /ws/project` (WebSocket, project protocol), `GET /game/<hash>/<file>` (wasm build output, `application/wasm` and `text/javascript`), `GET /plugins/<file>` (`text/javascript`).

- [ ] **Step 1: Write the failing tests**: in `service.rs`: `list_and_read_without_build`, `requests_outside_project_are_rejected`, `create_without_engine_path_names_the_setting`, `open_builds_in_background_and_reports_game_ready` (`#[ignore]`), `failed_rebuild_keeps_previous_game_in_both_modes` (`#[ignore]`); in `serve.rs`: `ws_handshake_and_list_files`, `plugins_and_game_files_are_served_with_js_and_wasm_content_types`, `two_clients_both_receive_file_events`.
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test service && cargo test -p ludenea-serve`. Expected: compile failure.
- [ ] **Step 3: Implement** the protocol, the service, the server, and the TypeScript export.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo test --locked -p ludenea-project -- --ignored && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(serve): serve projects and the editor over WebSocket`.

---

### Task 19: Editor shell and the plugin API

**Files:**
- Create: `editor/packages/editor-shell/{package.json (exports: "." only), src/index.ts, src/plugin.ts, src/registry.ts, src/editor.ts, src/entities.ts, src/layout/Shell.tsx, src/palette/CommandPalette.tsx, src/shortcuts.ts, src/stores/{selection.ts, timeline.ts, project.ts}, src/loader.ts}`, `editor/packages/editor-shell/test/{registry.test.ts, loader.test.ts, palette.test.tsx, entities.test.ts}`, `editor/scripts/bundle-dts.mjs`
- Modify: `crates/ludenea-project/types/*.d.ts` (generated by `bundle-dts.mjs`)

**Interfaces:**
- Consumes: Task 13 `ControlClient`, `ProjectClient`.
- Produces (`@ludenea/editor-shell`), the only module path the package exports:
  - `export interface Disposable { dispose(): void }`, `export function definePlugin(plugin: PluginDefinition): PluginDefinition`, `export interface PluginDefinition { name: string; activate(editor: Editor): void | (() => void) }`.
  - `export interface Editor { control: ControlClient | null; project: ProjectClient; entities: EntityAccess; panels: { register(panel: PanelDefinition): Disposable }; inspectors: { register<N extends keyof ComponentTypes>(component: N, inspector: InspectorComponent<ComponentTypes[N]>): Disposable }; commands: { register(command: CommandDefinition): Disposable; run(id: string): Promise<void>; list(): CommandDefinition[] }; shortcuts: { register(shortcut: { key: string; command: string }): Disposable }; overlays: { register(overlay: ViewportOverlay): Disposable }; settings: { register<T>(setting: SettingDefinition<T>): Disposable; get<T>(id: string): T; set<T>(id: string, value: T): void }; selection: SelectionStore; timeline: TimelineStore; onGameChanged(listener: (control: ControlClient | null) => void): Disposable }`.
  - `export interface ComponentTypes {}` (augmented by the generated `game.d.ts`). `export interface EntityAccess { get<N extends keyof ComponentTypes>(entity: Entity, component: N): Promise<ComponentTypes[N] | undefined>; set<N extends keyof ComponentTypes>(entity: Entity, component: N, path: PathSegment[], value: unknown): Promise<void>; watch(entity: Entity, listener: (components: Partial<ComponentTypes>, removed: string[]) => void): Disposable }` (typed wrappers over `GetEntity`, `SetField`, and the `Entity` topic).
  - `PanelDefinition { id: string; title: string; component: React.ComponentType<{ editor: Editor }>; defaultPosition?: "left" | "right" | "bottom" | "center" }`, `CommandDefinition { id: string; title: string; run(editor: Editor): void | Promise<void> }`, `InspectorComponent<T> = React.ComponentType<{ value: T; onChange(path: PathSegment[], value: unknown): void; entity: Entity }>`, `SettingDefinition<T> { id: string; title: string; default: T; kind: "boolean" | "number" | "string" | "path" }` (persisted in `localStorage` per installation, wrapped in try/catch), `ViewportOverlay { id: string; draw(context: OverlayContext): void }` with `OverlayContext { canvas: CanvasRenderingContext2D; worldToScreen(point: Vec2): { x: number; y: number }; tick: number; selection: readonly Entity[] }`.
  - `SelectionStore` (Zustand): `{ entities: Entity[]; select(entities: Entity[]): void; add(entity: Entity): void; clear(): void }`; `TimelineStore`: `{ timeline: Timeline | null; hash: string | null; pending: boolean; playing: boolean; effectiveRate: number; divergence: { tick: number } | null }`, fed by the shell from `Ticked`, `TimelineChanged`, `Paused`, and `Diverged` events.
  - Every registration made during `activate` is collected per plugin; `loader.deactivate(name)` disposes them and calls the returned cleanup. `loader.load(url: string)` imports the module and activates its default export; `loader.reload(name, url)` deactivates then loads, keeping stores (selection, timeline, layout) intact.
  - Registering a second panel, command, setting, or shortcut with an existing id throws `DuplicateRegistration` naming both plugins.
  - Shell layout with dockview; the command palette (`Ctrl+K` and `Cmd+K`) lists every command, fuzzy filtered; shortcuts are global unless focus is in a text field.
  - `editor/scripts/bundle-dts.mjs` (rollup-plugin-dts, development only) rolls the public `.d.ts` of `editor-shell` and `control-client` and the React types they reference into `crates/ludenea-project/types/`, deterministically (sorted, no timestamps).

- [ ] **Step 1: Write the failing tests**: `registrations_are_disposed_on_deactivate`, `duplicate_command_id_names_both_plugins`, `reload_keeps_selection_and_layout`, `palette_lists_and_runs_registered_commands`, `shortcut_ignored_inside_text_input`, `built_in_and_project_plugins_receive_the_same_editor_object` (identity check), `entities_get_returns_typed_value_from_get_entity`, `entities_set_sends_set_field_with_path`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the shell package.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck && node editor/scripts/bundle-dts.mjs`. Expected: tests PASS and `crates/ludenea-project/types/*.d.ts` regenerated with the real declarations (part of this commit).
- [ ] **Step 5: Commit** `✨ feat(editor): add the editor shell and plugin API`.

---

### Task 20: Built-in plugins: project, assets, scene tree, console

**Files:**
- Create: `editor/packages/editor-builtins/{package.json, src/index.ts, src/project/*.tsx, src/assets/*.tsx, src/scene-tree/*.tsx, src/console/*.tsx}`, `editor/packages/editor-builtins/test/*.test.tsx`, `editor/packages/editor-builtins/test/imports.test.ts`

**Interfaces:**
- Consumes: Task 19 `Editor`; project protocol (Task 18); `QueryEntities` and subscriptions (Task 10); `SaveScene`, `ReplaceScene` (Tasks 7 and 8).
- Produces: `export const builtinPlugins: PluginDefinition[]`, each a `definePlugin` using only the `Editor` object. `imports.test.ts` fails if any source file under `editor-builtins/src` imports anything but `react`, `react/jsx-runtime`, `@ludenea/editor-shell`, `@ludenea/control-client`, `@tanstack/react-virtual`, or a relative path (parity with project plugins, plus the list library).
  - `ludenea.project`: commands `project.new` (name, parent folder, engine path defaulting to the setting `ludenea.engine-path`), `project.open`, `project.build`; recent projects in settings; build progress and "building" states.
  - `ludenea.assets`: file tree of `scenes/` and `assets/` with filter; commands `assets.rename`, `assets.move`, `assets.delete` (confirmation inside the panel, never a browser dialog), `assets.open`, also reachable by drag and context menu; opening a scene sends `ReadText` then `LoadScene`. On `FilesChanged` for the open scene: with `Timeline.scene_dirty` true, shows the banner "changed on disk" with Reload and Keep mine; otherwise sends `ReplaceScene` with the new text. Orphan meta warnings go to the console. An unsaved marker (from `Timeline.scene_dirty`) shows next to the open scene.
  - `ludenea.scene-tree`: entities of the current tick (virtualized), label = first component short names; click selects (shared `selection` store); commands `scene.despawn-selection` (`Delete`), `scene.save` (`Ctrl+S` / `Cmd+S`: `SaveScene`, then `WriteText`).
  - `ludenea.console`: build lines and diagnostics (clickable `file:line` opens the asset browser at that file), `GameOutput` lines, host `Console` events (panics, dropped edits, divergences), warnings; filter by level.

- [ ] **Step 1: Write the failing tests** (fake transports): `opening_scene_loads_it_into_the_game`, `disk_change_with_unsaved_edits_shows_banner_and_keeps_mine`, `disk_change_without_unsaved_edits_sends_replace_scene`, `unsaved_marker_follows_scene_dirty`, `rename_through_command_sends_rename`, `scene_tree_selection_updates_store`, `console_shows_build_diagnostic_with_location`, `console_shows_game_output_and_host_console`, `builtins_import_only_public_modules`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the four plugins.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add project, asset, scene tree, and console panels`.

---

### Task 21: Built-in plugins: ECS explorer, inspector, physics panel

**Files:**
- Create: `editor/packages/editor-builtins/src/{explorer/*.tsx, inspector/*.tsx, inspector/fields/*.tsx, inspector/default-value.ts}`, `crates/ludenea-physics/editor/index.tsx` (engine plugin), tests next to them

**Interfaces:**
- Consumes: `GameInfo` schemas, `QueryEntities`, `GetEntity`, `GetResources`, `SetField`, `InsertComponent`, `RemoveComponent`, `ApplyToScene`, `GetSchedule`, `GetTickEvents`, `GetTickCommands`, `Subscribe` (Tasks 7, 8, 10).
- Produces:
  - `ludenea.explorer`: tabs Entities (query input with the code syntax, `InvalidFilter` errors shown inline with candidates; `@tanstack/react-virtual` list paging `QueryEntities` by 200 rows; each row shows the entity and its hash), Schedule (run order, reads, writes, microseconds per tick or "not measured in the browser"), Events (selected tick), Commands (per sync point). Subscribes to `Introspection` only while the Schedule, Events, or Commands tab is visible.
  - `ludenea.inspector`: components and resources of the selection rendered from `TypeSchema` (number fields for integers and floats, large integers edited as text, checkbox, text, enum variant select with nested fields, lists and arrays with indexed rows, `Entity` as a link that selects it, `AssetId` as a picker over `ListFiles`); each change sends `SetField` with its path; `ComponentValue::Ron` values are read-only text; `ludenea::Time` is read-only; a registered custom inspector replaces the generic one for its component. Errors (`InvalidEdit`, `NoSuchEntity`) show under the field; `NoSuchEntity` also clears the selection. Commands `inspector.add-component` (component picker; `default-value.ts` builds a value from the schema: zeros, `false`, empty strings and lists, first enum variant; an `InvalidEdit` reply opens the value form so the user fixes it before inserting), `inspector.remove-component`, `inspector.apply-to-scene` (`ApplyToScene` for the focused component).
  - Engine plugin `ludenea_physics` (`crates/ludenea-physics/editor/index.tsx`, bundled by Task 17's discovery): a "Physics" panel showing body counts by kind and the contact begin and end events of the current tick, using only `editor.control` and `editor.entities`.

- [ ] **Step 1: Write the failing tests**: `filter_error_shows_candidates`, `list_renders_only_visible_rows_for_100000_entities` (fewer than 100 row elements in the DOM), `editing_newtype_variant_field_sends_set_field_with_path` (`Collider.shape` circle radius gives path `[shape, Circle, radius]`), `large_integer_is_edited_as_text_and_sent_as_string`, `invalid_edit_message_shows_under_field`, `no_such_entity_clears_selection`, `custom_inspector_replaces_generic`, `default_value_from_schema_for_body`, `introspection_subscribed_only_while_tab_visible`, `physics_panel_counts_bodies_by_kind`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the plugins.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add the ECS explorer and the schema-driven inspector`.

---

### Task 22: Debug draw renderer and the viewport

**Files:**
- Modify: `crates/ludenea-debug-draw/{Cargo.toml, src/lib.rs}` (feature `render`: wgpu with `webgpu` and `webgl`, wasm-bindgen, web-sys), `xtask/src/main.rs` (`wasm-renderer`), `editor/package.json` (`build:wasm` runs `cargo xtask wasm-renderer editor/packages/debug-draw-wasm`)
- Create: `crates/ludenea-debug-draw/src/render.rs` (browser gate), `editor/packages/editor-builtins/src/viewport/*.tsx`, tests next to them

**Interfaces:**
- Consumes: Task 5 `DebugDrawFrame` and `mesh::triangulate`, Task 10 `GetDebugDraw` and `Pick`.
- Produces (`ludenea_debug_draw::render`): `#[wasm_bindgen] pub struct ViewportRenderer` with `async create(canvas: HtmlCanvasElement) -> Result<ViewportRenderer, JsValue>` (instance from `wgpu::util::new_instance_with_webgpu_detection`, surface created before requesting the adapter, so WebKitGTK and headless Chromium fall back to WebGL2), `resize(&mut self, width: u32, height: u32, pixel_ratio: f32)`, `set_camera(&mut self, center_x: f32, center_y: f32, pixels_per_meter: f32)`, `draw(&mut self, frame_json: &str, layers_json: &str, selected_json: &str)` (one vertex buffer from `mesh::triangulate` per frame). The generated package `editor/packages/debug-draw-wasm/` is gitignored and built by `pnpm -C editor build:wasm`.
- Produces (`ludenea.viewport` plugin): canvas sized to its panel plus a 2D overlay canvas above it (labels from `Label` primitives and `editor.overlays`); pan (middle drag, or `Space` held plus drag), zoom (wheel, anchored at the cursor); keyboard: commands `viewport.pan-left/right/up/down` (`Shift` plus arrows) and `viewport.zoom-in/out` (`+`, `-`), `viewport.frame-selection` (`F`); click to `Pick` and select (`Shift` adds); drag a selected entity with a `Pose`: a local outline follows the cursor and release sends one `SetField` on `Pose.position` (Deviations); arrow keys nudge the selection by 0.1 m through `SetField`; one toggle command per `GameInfo.debug_layers` entry (`viewport.toggle-layer.<layer>`), the only layer toggle path; frames requested once per animation frame while playing, on change otherwise.

- [ ] **Step 1: Write the failing tests** (renderer mocked): `screen_to_world_inverts_world_to_screen`, `zoom_keeps_point_under_cursor`, `click_sends_pick_and_selects`, `drag_sends_one_set_field_on_release`, `arrow_nudges_by_tenth_meter`, `layer_toggle_commands_follow_game_info`, `labels_are_drawn_on_overlay`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the renderer and the plugin.
- [ ] **Step 4: Run** `pnpm -C editor build:wasm && pnpm -C editor test && pnpm -C editor typecheck && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): render debug draw in the viewport and move bodies by hand`.

---

### Task 23: Timeline, live input, and undo in the editor

**Files:**
- Create: `editor/packages/editor-builtins/src/timeline/*.tsx`, `editor/packages/editor-builtins/src/input/capture.ts`, `editor/packages/editor-builtins/test/keyboard.test.ts`, tests next to them

**Interfaces:**
- Consumes: Task 7 `Play`, `Pause`, `Step`, `Seek`, `SetSpeed`, `SetLiveInput`, `Undo`, `Redo`, `GetTimeline`, events `Ticked`, `TimelineChanged`, `Paused`, `Resimulated`, `Diverged`, `EditDropped`.
- Produces:
  - `ludenea.timeline` panel: current tick, head, hash (with a "pending" marker when `pending`), effective rate when below the tick rate, a scrubber over `[0, head]` (drag seeks, throttled to one `Seek` in flight), checkpoint ticks as faint marks, edit ticks as markers, a divergence marker from `Diverged`, resimulation progress; while `Timeline.resimulating`, editing panels show a "resimulating" state and editing commands are disabled.
  - Commands with default shortcuts: `timeline.play-pause` (`F5`), `timeline.step` (`.`), `timeline.step-back` (`,`: seek to current minus 1), `timeline.speed-up` (`]`), `timeline.speed-down` (`[`), `edit.undo` (`Ctrl+Z` / `Cmd+Z`), `edit.redo` (`Ctrl+Shift+Z` / `Cmd+Shift+Z`).
  - `capture.ts`: while playing and the viewport has focus, sends `SetLiveInput` once per animation frame with pressed `KeyboardEvent.code` values and the Gamepad API state of the first gamepad, for player 0 (one player in 2a); sends an idle state on blur.
  - `keyboard.test.ts`: loads all built-in plugins and asserts that `commands.list()` contains every id in this list, so nothing is mouse-only: `project.new`, `project.open`, `project.build`, `assets.rename`, `assets.move`, `assets.delete`, `assets.open`, `scene.save`, `scene.despawn-selection`, `inspector.add-component`, `inspector.remove-component`, `inspector.apply-to-scene`, `viewport.pan-left`, `viewport.zoom-in`, `viewport.frame-selection`, `timeline.play-pause`, `timeline.step`, `timeline.step-back`, `edit.undo`, `edit.redo`.

- [ ] **Step 1: Write the failing tests**: `scrubbing_sends_at_most_one_seek_in_flight`, `pending_marker_follows_hash_reply`, `undo_shortcut_runs_undo_and_refreshes_timeline`, `divergence_marker_appears_on_diverged_event`, `editing_disabled_while_resimulating`, `capture_sends_pressed_codes_and_idle_on_blur`, `every_action_is_a_command`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the panel, commands, and capture.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): add the timeline with play, step, seek, and undo`.

---

### Task 24: Web and desktop applications

**Files:**
- Create: `editor/shared/{package.json, vite.config.ts, scripts/export-lists.mjs, src/*.js}` (the shared modules build), `editor/apps/web/{package.json, index.html, vite.config.ts, src/main.tsx, src/session-guard.ts}`, `editor/apps/desktop/{package.json, index.html, vite.config.ts, src/main.tsx, src-tauri/{Cargo.toml, Cargo.lock, tauri.conf.json, src/main.rs, capabilities/default.json}}`, `editor/apps/web/test/session-guard.test.ts`
- Modify: workspace `Cargo.toml` (`exclude = ["editor/apps/desktop/src-tauri"]`), `crates/ludenea-serve` (feature `embed-editor`)

**Interfaces:**
- Consumes: Tasks 13, 14, 18 to 23.
- Produces:
  - Shared modules: `editor/shared` is a separate Vite library build producing `dist/shared/react.js`, `react-jsx-runtime.js`, `editor-shell.js`, `control-client.js`. Each entry re-exports an explicit named-export list written by `export-lists.mjs` (from `Object.keys(require("react"))` and the packages' public exports; React is CommonJS, so `export *` would export nothing), and keeps the other three bare specifiers external so `react/jsx-runtime` and the editor packages share the single `react`. Both apps copy `dist/shared/` into their output, declare `<script type="importmap">` mapping the four specifiers to `/shared/*.js` in `index.html`, and mark them external in their own Vite builds. In development, a small Vite plugin in each app serves `/shared/*` from the shared build, so `vite dev` and `tauri dev` also run one React instance.
  - Both apps' `main.tsx`: mount the shell, load `builtinPlugins`, then every `ListPlugins` bundle through `loader.load`, and reload on `PluginRebuilt`; after each control handshake send `WriteGameTypes`.
  - Web: project client over `webSocketTransport("/ws/project")`; on `GameEndpoint::Wasm { glue_url }`, `startWorkerGame` and connect the control client. `session-guard.ts`: `export function guardSession(control: ControlClient, game: WorkerGame, restart: () => Promise<ControlClient>): Disposable` keeps the latest `ExportSession` (taken on every `Paused`, after every editing command, and every second while playing); on `onCrashed`, it restarts the worker, sends `LoadSession` with the latest export, seeks to the crash tick when known (else the export's current tick), and writes the panic message to the console. A rebuild uses the same path: export, restart with the new glue, `LoadSession`, seek back once resimulation reaches the previous tick; `Diverged` is reported if the new code changes hashes.
  - Desktop: `src-tauri` (Tauri 2, own lockfile) embeds `ProjectService` in desktop mode with the engine path from the setting `ludenea.engine-path` or the `LUDENEA_ENGINE_PATH` variable. Tauri command `send(channel, message)` routes `project` messages to the service and `control` lines to the game process unchanged; command `subscribe(channel, onMessage: Channel<String>)` streams replies and events in batches (one `Channel` message per `Host::advance` batch, never one Tauri event per protocol message). Plugin bundles load through a custom URI scheme `ludenea-plugin://` registered with `register_asynchronous_uri_scheme_protocol`, answering `text/javascript` with `Access-Control-Allow-Origin: *`; the CSP allows `script-src 'self' ludenea-plugin: 'wasm-unsafe-eval'`. On rebuild the backend restarts the game process and the front end restores the session as the web app does (`ExportSession` before, `LoadSession` after).
  - Both windows show the shell before any build (spec target: interactive in under 2 seconds).

- [ ] **Step 1: Write the failing tests**: `session-guard.test.ts` (fake worker and client): `crash_restarts_worker_and_loads_latest_export`, `export_taken_on_pause_and_after_edits`; `editor/e2e/smoke.spec.ts` (`web editor opens the template project`): start `ludenea serve` on a temp project created with `ludenea new`, open `/`, expect the asset browser to list `scenes/main.scene.ron` within 2 seconds, the viewport to leave the building state after the build, and `window.__ludeneaReactInstances` (set by a test-only probe in the shared React entry) to equal 1 after a project plugin loads.
- [ ] **Step 2: Run** `pnpm -C editor build && cargo build --locked -p ludenea-serve && pnpm -C editor test && pnpm -C editor e2e smoke`. Expected: FAIL.
- [ ] **Step 3: Implement** the shared build and both apps.
- [ ] **Step 4: Run** the same, plus `cargo build --manifest-path editor/apps/desktop/src-tauri/Cargo.toml --locked` after `pnpm -C editor/apps/desktop build`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): ship the editor in the browser and on the desktop`.

---

### Task 25: End-to-end proofs, continuous integration, README

**Files:**
- Create: `editor/e2e/{session.spec.ts, plugins.spec.ts, performance.spec.ts, fixtures/test-plugin/index.tsx}`, `examples/boxes/src/bin/editor-bench.rs` (writes a scene of 100,000 entities using only `ludenea::` components and resources: 1,000 dynamic bodies laid out like `generate_scene`, plus 99,000 entities with only a `Pose`, so it loads in the template project)
- Modify: `.github/workflows/determinism.yml` (jobs `editor` and `desktop`), `README.md`

**Interfaces:**
- Consumes: everything above; Task 14 `decodeTrace`.

- [ ] **Step 1: Write the end-to-end tests** (web app against `ludenea serve`, template project):
  - `session.spec.ts` (`an editor session replays in the CLI`): open the main scene, play 2 seconds, pause, seek back 60 ticks, change a box's `Pose.position.x` in the inspector, seek forward 30 ticks then back to the edit tick, play 1 second, undo, redo, `ExportSession` written to files; then the project's binary `run <scene> --inputs <file> --edits <file> --ticks <head> --trace <file>`; `decodeTrace` of that trace must equal the exported `hashes` at every tick (editor and CLI computed them independently), and `verify --golden` against that trace passes.
  - `plugins.spec.ts` (`a project plugin has the same API as built-ins`): copy `fixtures/test-plugin` into the project's `editor/`, expect its panel and its custom inspector for `ludenea::Body` to appear without restarting the game (timeline tick unchanged), its typed `editor.entities.get` read to return the body, and the plugin to reach nothing beyond its `editor` argument and the shared modules (a probe on `globalThis` records no other access).
  - `performance.spec.ts` (`the UI stays at 60 fps with 100,000 entities`): open the bench scene, play 5 seconds, measure frame times with `requestAnimationFrame` in the page; assert the 95th percentile frame time is at most 17 ms and the shell was interactive in under 2 seconds, when `PERF_STRICT=1` (local runs on the reference machine, an Apple M-series); otherwise report the numbers without failing. Report the simulation's effective rate (never a pass criterion).
- [ ] **Step 2: Run** `cargo run --locked -p boxes --bin editor-bench && pnpm -C editor build && cargo build --locked -p ludenea-serve && PERF_STRICT=1 pnpm -C editor e2e`. Expected: PASS (fix what fails in the owning task's code).
- [ ] **Step 3: Extend CI** (existing jobs unchanged):
  - Job `editor` on `ubuntu-24.04`: toolchain, Node 24 and pnpm 10, `pnpm -C editor install --frozen-lockfile`, `pnpm -C editor exec playwright install --with-deps chromium`, `pnpm -C editor build:wasm`, `pnpm -C editor typecheck`, `pnpm -C editor test`, staleness checks (`cargo test --locked -p ludenea-control --test typescript && cargo test --locked -p ludenea-project --test typescript && node editor/scripts/bundle-dts.mjs && git diff --exit-code`), `cargo test --locked -p ludenea-project -- --ignored`, `cargo xtask wasm-game boxes editor/e2e/golden/pkg`, `cargo run --locked -p boxes --bin editor-bench`, `pnpm -C editor build`, `cargo build --locked -p ludenea-serve`, then `pnpm -C editor e2e` (performance in report mode).
  - Job `desktop`, matrix `macos-14`, `ubuntu-24.04` (installs `libwebkit2gtk-4.1-dev`, `libsoup-3.0-dev`, `librsvg2-dev`), `windows-2025`: `pnpm -C editor/apps/desktop build`, then `cargo clippy --manifest-path editor/apps/desktop/src-tauri/Cargo.toml --locked -- -D warnings` and `cargo build --manifest-path editor/apps/desktop/src-tauri/Cargo.toml --locked`.
- [ ] **Step 4: Update `README.md`**: the editor (what it does, `ludenea new`, `ludenea serve`, the desktop app), the plugin API in five lines, and the development commands.
- [ ] **Step 5: Run** `cargo clippy --locked --workspace --all-targets -- -D warnings && cargo test --locked --workspace && pnpm -C editor test`. Expected: PASS.
- [ ] **Step 6: Commit** `✅ test(editor): prove session replay, plugin parity, and UI fluidity end to end`.
