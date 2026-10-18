# Ludenea Milestone 2b: The Editor That Changes The Game Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prefabs and data assets that the simulation reads deterministically, replays as self-contained verifiable assets, a timeline that forks into branches instead of discarding the future, and an editor that shows another branch as a ghost and explains, entity by entity, how and why two futures diverge, plus a viewer for CI traces and snapshots.

**Architecture:** `ludenea-ecs` gains typed handles, an asset store with its own conditional hash leaf, prefab spawning, and structured snapshot diffs. `ludenea-app` gains scene version 2 (stable local identifiers, prefab instances), resolution of scenes against asset texts, prefab editing operations, edit log version 2 (load addresses, asset edits), metas, traces, and the replay file. `ludenea-control` turns the linear session into a branch tree, adds a second simulation for the compare branch (ghost and causal report), and moves to protocol version 3. The project service learns the new asset kinds and stores sessions per document. The editor adds `prefabs` and `divergence` plugins and extends `assets`, `inspector`, `timeline`, `explorer`, and `viewport`, all on the public plugin API.

**Tech Stack:** unchanged from milestone 2a (Rust 1.97.1, serde, serde_json, postcard, ron, xxhash-rust, ts-rs, wasm-bindgen, wgpu, rolldown, axum, Tauri 2; TypeScript strict, React 19, Vite, pnpm, Vitest, Playwright, dockview, Zustand). No new dependency; some crates gain an existing workspace dependency (`serde_json` for `ludenea-schema`, `xxhash-rust` and `uuid` for `ludenea-app`).

**Spec:** `.claude/superpowers/specs/2026-10-18-ludenea-editor-2b-design.md` (parents: `.claude/superpowers/specs/2026-10-17-ludenea-editor-2a-design.md`, `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md`). The 2a plan `.claude/superpowers/plans/2026-10-17-ludenea-milestone-2a-usable-editor.md` describes the existing code.

## Global Constraints

- Every Global Constraint of the milestone 1 and 2a plans still applies (determinism rules for simulation crates, clippy allowances outside the simulation, target gates, pinned `wasm-bindgen` and rolldown, no Node.js at runtime, committed generated files, `#[ignore]` on tests running a real `cargo build`, keys, commit style).
- `golden/boxes.trace` is never regenerated. The existing golden tests (`examples/boxes/tests/golden.rs`, `editor/e2e/golden.spec.ts`, the CI verify jobs) pass unchanged at the end of every task. `examples/boxes` registers no new resource (a registered resource is a hash leaf of every world); new `boxes` types are components and asset types only. A `boxes` system draws from `Rng` only inside its loop over matching entities, so worlds without them draw nothing.
- `ludenea::Assets` hash leaf: present in `HashTree.resources` (after `ludenea::Entities`) only when at least one asset is loaded.
- Trace scene hash: xxh3 of the scene file's raw bytes when no asset is loaded after the scene load (milestone 1 behaviour, so `golden/boxes.trace` matches); otherwise xxh3 of the resolved scene text followed by, in identifier order, each loaded asset's identifier and canonical bytes.
- Formats and versions: `SCENE_VERSION = 2` (version 1 still loads), `PREFAB_VERSION = 1`, `EDIT_LOG_VERSION = 2` (version 1 still decodes), `SNAPSHOT_VERSION = 2` (version 1 refused with `SnapshotError::Version`, the version read before the rest), undo history versioned (`UNDO_VERSION`, Tasks 6 and 12; unversioned 2a bytes still decode), `REPLAY_VERSION = 1`, `META_VERSION = 1` (2a metas still read), `PROTOCOL_VERSION = 3`, project protocol version 2. 2a session exports (no branches, no assets, 2a undo history) load through `LoadSession`.
- Stable names: `ludenea::Prefab` (asset type), `ludenea::Assets` (hash leaf), `ludenea::Collision` (the physics collision event, Task 7). Asset and event type names follow components: `module_path!()::Type` unless renamed with the same attribute components use.
- File kinds by extension: `.scene.ron` scene, `.prefab.ron` prefab, `.replay` replay, `.trace` trace, `.snap` snapshot. A data asset is any `.ron` file whose meta says `Data { type }`; only the meta decides it.
- Directory scans that build asset sets (CLI, project service) sort paths before reading and refuse two metas with the same identifier, naming both files.
- Constants: `REPLAY_CHECKPOINT_INTERVAL = 1024`, `CHECKPOINT_BUDGET = 256`, `COMPARE_BUDGET_PLAYING = 4`, `COMPARE_BUDGET_PAUSED = 240`, `DIVERGENCE_TICKS = 600`.
- Generated files: after any change to protocol types run `cargo test --locked -p ludenea-control --test typescript`; after project protocol changes `cargo test --locked -p ludenea-project --test typescript`; after any change to the public editor API (`editor-shell`, `control-client` exports) run `node editor/scripts/bundle-dts.mjs`. Commit the outputs in the same task; CI checks staleness.
- Front end tests live in each package's `test/` directory, as in 2a.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C editor test`, and `pnpm -C editor typecheck` pass at the end of every task. A task that changes a Rust type mirrored in TypeScript updates the typed fixtures that break (for example `editor/packages/editor-builtins/test/support.tsx`, `editor/apps/web/test/games.test.ts`, `editor/packages/editor-shell/test/timeline.test.ts`).

## Deviations From The Spec

- `Assets<T>` is its own system parameter (`assets: Assets<WeaponStats>`), not `Res<Assets<T>>`: assets are not resources (not in scenes, not edited as resources, own hash leaf).
- Reachability follows `Handle<T>` fields only. A bare `AssetId` field carries no type, so it cannot be loaded; games that want an asset loaded use a handle.
- `Edit::LoadAsset` carries the asset's type name and canonical bytes, not only its identifier, so an edit log stays self-contained like the rest of the run definition.
- The game host receives every prefab and data asset text of the project with `LoadScene` and keeps only those reachable from the scene; the project service does not know game schemas.
- The compare simulation is interleaved with the main one inside `Host::advance` on every target, under a per-advance budget, instead of running on its own thread on native. A thread comes if the ghost measurably slows play.
- In scene and prefab files version 2, an `Entity` value names a local identifier in `index` (written with `generation: 0`, generation ignored on load); loading remaps it. `Entity` values inside recorded edit bytes stay raw entity identifiers.
- Instances and scene entities share one local identifier space per file, so a load address is a plain path of identifiers.
- A replay stores raw entity identifiers in its edit log: recording rewrites every load address to the entity it resolved to, since a replay's scene never changes.
- There is no `ListBranches` command: the `Timeline` reply and `TimelineChanged` carry the branches.
- A prefab cycle names the chain by file path when the asset source knows paths (editor, CLI), by identifier otherwise.
- The trace viewer seeks only when the current session is one side of the comparison; to compare a replay, open it as the document first.
- Stored sessions are written when switching documents, when the editor closes, and on the 2a crash recovery schedule (every pause, every edit, every second while playing), reusing that export.

## Review Focus

- **A prefab file deleted (or its meta lost) while a scene references it**: the scene load is refused with `SceneError::MissingAsset` naming the instance identifier and the asset identifier; the scene file is untouched and the editor shows the error in the console and on the scene tree, with the previous session kept. Tests in Tasks 3 and 20.
- **A prefab edited so that an instance override targets an entity or component that no longer exists**: the scene loads, the console shows one warning per stale override, and saving the scene keeps the stale overrides byte for byte. Tests in Tasks 3 and 20.
- **Switching to a stale branch** (marked stale by a scene edit or rebuild) while another resimulation runs: the switch succeeds, the target branch resimulates from its nearest fresh ancestor with progress, and seeking past the resimulated point fails with `Resimulating` as in 2a. Test in Task 13.
- **Opening a replay recorded with different game types**: when everything decodes, it plays and reports the first divergent tick against its own hashes; when a value fails to decode (input type changed), `LoadReplay` fails with `ReplayError::Decode` naming the first value (scene component, asset, input of tick N, or edit of tick N) and the reason. Tests in Tasks 8, 15, and 21.
- **A data asset file broken on disk during a session** (RON error, wrong type): the reload is refused, the console shows the path, line, and column, and the session keeps the previous asset value. Tests in Tasks 18 and 19.

---

### Task 1: Handles, asset types, and the asset store

**Files:**
- Create: `crates/ludenea-ecs/src/assets.rs`
- Modify: `crates/ludenea-ecs/src/{asset.rs, world.rs, hash.rs, snapshot.rs, system.rs, error.rs, lib.rs}`, `crates/ludenea-schema/src/{lib.rs, typescript.rs}`, `crates/ludenea-macros/src/lib.rs` (`derive(Asset)`), `crates/ludenea-app/src/{app.rs, simulation.rs}`, facade prelude (`Asset`, `Assets`, `Handle`), callers of `typescript_declarations` (`crates/ludenea-project/src/types.rs`, `examples/boxes/tests/typescript.rs`, passing `&[]` for assets until Task 18), `crates/ludenea-ecs/tests/snapshot.rs` (snapshot fields)
- Test: `crates/ludenea-ecs/tests/assets.rs`, inline tests in `crates/ludenea-schema/src/typescript.rs`

**Interfaces:**
- Produces (`ludenea_schema`): `TypeSchema::Handle { asset: String }` (the asset type's stable name); TypeScript maps it to `string`. `typescript_declarations(components: &[(&str, &TypeSchema)], assets: &[(&str, &TypeSchema)]) -> String` also emits `export interface AssetTypes { "<stable name>": <Type>; ... }` (an `interface`, so declaration merging works, Task 19).
- Produces (`ludenea_ecs`):
  - `pub trait Asset: Schema + Serialize + DeserializeOwned + Send + Sync + 'static` with the same stable-name mechanism as `Component` (including its rename attribute); `#[derive(Asset)]` implements it and `Schema`.
  - `pub struct Handle<T: Asset> { pub id: AssetId, .. }` (`Copy`, `Eq`, `Ord`, `Debug`), serialized exactly as its `AssetId` (same bytes, same hash), `impl Schema` giving `TypeSchema::Handle { asset: T::NAME }`, `Handle::new(id) -> Self`.
  - `World::register_asset::<T: Asset>(&mut self) -> Result<(), EcsError>` and `World::register_asset_with_default::<T: Asset + Default>(&mut self)` (records a default constructor); codec as for components (encode, decode, hash, RON, JSON, schema). `World::asset_schemas(&self) -> Vec<(&'static str, TypeSchema)>` sorted by name; `World::default_asset_ron(&self, type_name: &str) -> Result<String, EcsError>` (`EcsError::NoDefault(String)` when registered without a default, `EcsError::UnknownAsset(String)` when not registered).
  - `World::insert_asset(&mut self, id: AssetId, type_name: &str, bytes: &[u8]) -> Result<(), EcsError>` (inserts or replaces), `World::insert_asset_ron(&mut self, id, type_name, ron: &str) -> Result<(), EcsError>` (`EcsError::InvalidAsset { id, type_name, reason }`, `reason` carrying the RON line and column), `World::asset_ids(&self) -> Vec<AssetId>`, `World::asset_bytes(&self, id) -> Option<(&str, Vec<u8>)>`, `World::asset_ron(&self, id) -> Option<String>`, `World::clear_assets(&mut self)`.
  - System parameter `pub struct Assets<'w, T: Asset>` with `get(&self, handle: Handle<T>) -> &T` (panics with "asset {id} of type {name} is not loaded") and `try_get(&self, handle) -> Option<&T>`. Read-only: never conflicts.
  - `pub const ASSETS_LEAF: &str = "ludenea::Assets";`. In `hash_tree`, when the store is non-empty, `NamedHash { name: ASSETS_LEAF, hash }` follows the `ludenea::Entities` leaf; `hash` is xxh3 over, in identifier order, each identifier's 16 little-endian bytes then the 8-byte `hash_value` of its canonical bytes.
  - `Snapshot` version 2 gains `pub assets: Vec<(AssetId, String, Vec<u8>)>` (identifier order, type name, canonical bytes) and `pub fingerprint: u64` (`World::snapshot` writes `0`, meaning unknown; `Simulation::snapshot` fills it from Task 8). `restore` replaces the store. `Snapshot::decode` reads `version` first and refuses anything but 2 with `SnapshotError::Version { found, expected: 2 }`.
- Produces (`ludenea_app`): `App::asset::<T: Asset>()`, `App::asset_with_default::<T: Asset + Default>()`; `Simulation::asset_schemas(&self)`.

- [ ] **Step 1: Write the failing tests**: `handle_encodes_and_hashes_like_its_asset_id`, `handle_schema_names_its_asset_type`, `world_without_assets_hashes_as_before` (a registered but empty asset type: no `ludenea::Assets` leaf, same root as without registration), `loaded_asset_adds_the_assets_leaf_after_entities`, `asset_hash_ignores_ron_formatting`, `assets_param_reads_loaded_asset`, `assets_param_panics_naming_missing_asset`, `snapshot_restores_assets`, `snapshot_version_1_is_refused_as_version`, `invalid_asset_ron_reports_line_and_column`, `default_asset_ron_requires_a_registered_default`; in `typescript.rs`: `asset_types_interface_lists_assets`.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs -p ludenea-schema`. Expected: compile failure.
- [ ] **Step 3: Implement** the schema variant, the derive, `assets.rs`, the hash leaf, snapshot version 2, `App::asset*`, and the caller updates.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS, golden tests included.
- [ ] **Step 5: Commit** `✨ feat(ecs): add typed asset handles and a content-hashed asset store`.

---

### Task 2: Scene version 2 and asset loading

**Files:**
- Create: `crates/ludenea-app/src/assets.rs`
- Modify: `crates/ludenea-app/src/{scene.rs, simulation.rs, lib.rs}`, `crates/ludenea-app/tests/scene.rs` (`scene_with_another_version_is_rejected` now uses version 3; the `SceneError::Version` message names the supported versions 1 and 2), `crates/ludenea-control/src/session.rs` (calls `load_scene_with(scene, &NoAssets)` until Task 13)
- Test: `crates/ludenea-app/tests/scene_assets.rs`

**Interfaces:**
- Consumes: Task 1 asset store and `TypeSchema::Handle`.
- Produces (`ludenea_app::assets`):
  - `pub trait AssetSource { fn text(&self, id: AssetId) -> Option<&str>; fn path(&self, id: AssetId) -> Option<&str> { None } }`; `pub struct NoAssets;`; `pub type AssetTexts = BTreeMap<AssetId, String>;` implements it (no paths); `pub struct NamedAssetTexts { pub texts: AssetTexts, pub paths: BTreeMap<AssetId, String> }` implements it with paths.
  - Data asset file form: `AssetFile(type: "my_game::WeaponStats", value: <RON>)`. `pub fn parse_asset_file(text: &str) -> Result<(String, String), String>` returns the type name and the value RON.
  - `pub fn reachable_handles(schema: &TypeSchema, value: &ron::Value, out: &mut BTreeSet<(AssetId, String)>)`: every `Handle` (identifier, asset type name) inside a value, walked by schema.
- Produces (`ludenea_app::scene`):
  - Scene version 2: `Scene(version: 2, next_id: u32, resources: {...}, entities: [(id: u32, components: {...})], instances: [...])`. `instances` (Task 3) defaults to empty. `next_id` is greater than every identifier ever assigned in this file; identifiers are never reused. Version 1 loads with identifiers equal to positions and `next_id` equal to the entity count.
  - `pub type LoadAddress = Vec<u32>;` (a path of local identifiers; one element for a scene entity).
  - `Simulation::load_scene_with(&mut self, ron: &str, assets: &dyn AssetSource) -> Result<SceneLoad, SceneError>` with `pub struct SceneLoad { pub warnings: Vec<String> }`: decodes, clears the asset store, then loads every asset reachable from the scene's components and resources, recursively through loaded assets (a visited set: cycles through handles are legal). `load_scene(&mut self, ron)` becomes `load_scene_with(ron, &NoAssets).map(|_| ())`.
  - `Entity` values in a version 2 file name local identifiers and are remapped to the spawned entities; an unknown identifier fails with `SceneError::UnknownLocal { entity: u32, component: String, local: u32 }`.
  - `Simulation::load_addresses(&self) -> &BTreeMap<Entity, LoadAddress>`, `Simulation::entity_at_address(&self, address: &[u32]) -> Option<Entity>`: from the last load; not part of the state or the hash.
  - `SceneError` gains `MissingAsset { referenced_by: String, id: AssetId }` (`referenced_by` such as `entity 7, ludenea::Turret.bullet` or `asset 3f2a...`), `WrongAssetType { id, path: Option<String>, referenced_by: String, expected: String, found: String }`, `InvalidAsset { id, path: Option<String>, reason: String }`, `DuplicateLocal(u32)`, `UnknownLocal {..}`.
  - `Simulation::save_scene(&self) -> String` writes version 2 with identifiers `0..n` and `next_id = n`; `Simulation::save_scene_keeping(&self, previous: &str) -> String` keeps the identifiers of `previous` for entities that still have a load address, gives new entities identifiers from `previous`'s `next_id`, and keeps `next_id` growing (Task 3 extends it to instances).
  - `Simulation::assets_ron(&self) -> AssetTexts`: the loaded assets in file form.

- [ ] **Step 1: Write the failing tests** (test app with `Turret { stats: Handle<Stats> }`, `Stats` deriving `Asset`): `version_1_scene_loads_unchanged` (the `boxes` scene gives the golden hash at tick 0), `version_2_scene_round_trips_through_save`, `reachable_assets_are_loaded_and_unreachable_ignored`, `assets_reached_through_assets_are_loaded`, `handle_cycle_between_assets_loads_once`, `missing_asset_names_the_referencing_field`, `wrong_asset_type_is_refused_with_path`, `entity_values_name_local_identifiers`, `unknown_local_identifier_is_refused`, `duplicate_local_identifier_is_refused`, `load_addresses_map_spawned_entities`, `save_keeping_preserves_identifiers_after_despawn`, `deleted_local_identifier_is_never_reused` (despawn the highest identifier at tick 0, spawn: the new entity gets `next_id`).
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test scene_assets`. Expected: compile failure.
- [ ] **Step 3: Implement** `assets.rs`, the version 2 parser and writer, remapping, and the call sites.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): give scene entities stable identifiers and load reachable assets`.

---

### Task 3: Prefabs and scene resolution

**Files:**
- Create: `crates/ludenea-app/src/prefab.rs`, `crates/ludenea-ecs/src/prefab.rs`, `crates/ludenea-app/src/path.rs` (moved whole from `crates/ludenea-control/src/path.rs`; `ludenea_control::path` becomes `pub use ludenea_app::path::*;`)
- Modify: `crates/ludenea-app/src/{scene.rs, assets.rs, app.rs, lib.rs}`, `crates/ludenea-ecs/src/lib.rs`, facade prelude (`Prefab`)
- Test: `crates/ludenea-app/tests/prefab.rs`

**Interfaces:**
- Consumes: Task 2 scene version 2, `AssetSource`, `LoadAddress`, `reachable_handles`.
- Produces (`ludenea_ecs::prefab`): `#[derive(Asset)] pub struct Prefab { pub entities: Vec<PrefabEntity>, pub entity_components: BTreeSet<String> }` with stable name `ludenea::Prefab`, registered by `App::new()`; `pub struct PrefabEntity { pub address: Vec<u32>, pub components: Vec<(String, Vec<u8>)> }`. This is the **flattened** prefab: nested instances expanded, overrides applied, canonical bytes, and every `Entity` value inside rewritten to the **position** of its target in `entities` (so nested prefabs reusing local identifiers cannot collide). `entity_components` names the component types whose schema contains `Entity`.
- Produces (`ludenea_app::prefab`):
  - Prefab file: `Prefab(version: 1, next_id: u32, entities: [(id: u32, components: {...})], instances: [...])` (no resources).
  - Instance entry (scene and prefab): `(id: u32, prefab: AssetId, overrides: [(entity: [u32], component: String, path: [PathSegment], value: <RON>)], added: [(entity: [u32], component: String, value: <RON>)], removed: [(entity: [u32], component: String)])`. `PathSegment` keeps its serde shape in files: `(type: "field", name: "x")`, `(type: "index", index: 2)`.
  - `pub fn resolve(file: &str, assets: &dyn AssetSource) -> Result<Resolved, SceneError>` with `pub struct Resolved { pub resources: Vec<(String, String)>, pub entities: Vec<ResolvedEntity>, pub warnings: Vec<String> }`, `pub struct ResolvedEntity { pub address: LoadAddress, pub components: Vec<(String, String)> }`. Order: the file's own entities in file order, then each instance's entities in instance order, each in prefab order, recursively. Pure; `load_scene_with` loads a `Resolved` and returns its warnings in `SceneLoad`.
  - Stale overrides (target entity, component, or path missing) produce one warning each naming the instance identifier, the prefab, and the target, and are ignored at load; saving never rewrites an instance entry it did not change, so they stay byte for byte.
  - Cycles through instance nesting: `SceneError::PrefabCycle(Vec<String>)` naming the chain by path when `AssetSource::path` knows it, else by identifier.
  - `Resolved::to_scene_text(&self) -> String`: a version 2 scene without instances, every entity written as `(address: [u32], components: {...})` (the address replaces `id` in resolved text, so addresses survive), used for the trace scene hash and replays.
  - Every prefab reached by an instance, **or by a `Handle<Prefab>`**, is resolved and loaded as a flattened `Prefab` asset; `reachable_handles` walks a flattened prefab's components through their schemas, so assets referenced inside a prefab are loaded too.
  - `Simulation::load_addresses` covers instance entities (`[instance id, entity id]`, deeper for nested instances); `Simulation::instances(&self) -> Vec<InstanceInfo>` with `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[cfg_attr(feature = "ts", derive(ts_rs::TS))] pub struct InstanceInfo { pub address: LoadAddress, pub prefab: AssetId, pub entities: Vec<Entity> }`.
  - `save_scene_keeping` also keeps `previous`'s instance entries for instances whose entities are all still alive and unchanged; Task 4 handles changed ones.

- [ ] **Step 1: Write the failing tests**: `scene_without_instances_resolves_to_itself` (golden hash at tick 0 unchanged), `instance_expands_after_scene_entities_in_prefab_order`, `override_sets_one_field`, `added_and_removed_components_apply`, `nested_instance_override_addresses_by_path`, `entity_values_are_remapped_per_instance`, `nested_prefabs_reusing_local_identifiers_do_not_collide` (a sub-prefab's entity 0 follows its entity 1; instantiated inside a parent whose entity 1 differs, it follows its own), `stale_override_warns_and_is_kept_on_save` (Review Focus), `missing_prefab_is_refused_naming_the_instance` (Review Focus), `prefab_cycle_is_refused_with_paths`, `resolved_scene_text_keeps_addresses`, `prefabs_reached_by_handle_are_flattened_and_loaded`, `assets_inside_prefabs_are_reached`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test prefab`. Expected: compile failure.
- [ ] **Step 3: Implement** both `prefab.rs` files, the `path.rs` move, and resolution in `load_scene_with`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): resolve prefab instances with overrides into scene entities`.

---

### Task 4: Prefab editing operations

**Files:**
- Modify: `crates/ludenea-app/src/{prefab.rs, scene.rs}`
- Test: `crates/ludenea-app/tests/prefab_editing.rs`

**Interfaces:**
- Consumes: Task 3 file shapes, `resolve`, `InstanceInfo`.
- Produces (`ludenea_app::prefab`), all pure functions on file texts, each returning the new text(s) or a `SceneError`:
  - `pub fn create_prefab(sim: &Simulation, entities: &[Entity]) -> Result<String, SceneError>`: a prefab file of the entities' current values with identifiers `0..n`; an `Entity` value pointing outside the set fails with `SceneError::ForeignReference { entity: u32, component: String, path: String }`.
  - `pub fn place_instance(file: &str, prefab: AssetId) -> Result<(String, u32), SceneError>`: appends an instance with a fresh identifier from `next_id`, returns the new text and the identifier.
  - `pub fn replace_with_instance(file: &str, entities: &[u32], prefab: AssetId) -> Result<String, SceneError>`: removes the given scene entities and places the instance (for "create prefab from selection").
  - `pub fn set_override(file: &str, instance: &[u32], entity: &[u32], component: &str, path: &[PathSegment], value: &str) -> Result<String, SceneError>`, `pub fn revert_override(file: &str, instance: &[u32], entity: &[u32], component: &str, path: &[PathSegment]) -> Result<String, SceneError>` (an empty `path` reverts every override of that component), `pub fn add_component_override(file, instance, entity, component, value: &str) -> Result<String, SceneError>` and `pub fn remove_component_override(file, instance, entity, component) -> Result<String, SceneError>` writing `added` and `removed`.
  - `pub fn apply_to_prefab(prefab_file: &str, entity: &[u32], component: &str, path: &[PathSegment], value: &str) -> Result<String, SceneError>`: writes the value into the prefab file (into a nested instance's override when `entity` has more than one element).
  - `pub fn save_prefab_checked(file: &str, assets: &dyn AssetSource) -> Result<(), SceneError>`: refuses a foreign reference and an instance cycle before a prefab is written.
  - Tick 0 edit rule (used by the session in Task 13): an edit on an entity whose load address lies inside an instance becomes `set_override`, `add_component_override`, or `remove_component_override` on the scene text; despawning such an entity is refused with `SceneError::InstanceEntity` ("remove the instance or override its components").

- [ ] **Step 1: Write the failing tests**: `create_prefab_numbers_entities_from_zero`, `foreign_reference_is_refused_on_create`, `place_instance_uses_next_id`, `replace_with_instance_removes_entities`, `set_then_revert_override_restores_text`, `revert_component_reverts_every_field`, `added_and_removed_overrides_round_trip`, `apply_to_prefab_writes_nested_override`, `prefab_cycle_is_refused_on_save`, `despawning_instance_entity_is_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test prefab_editing`. Expected: compile failure.
- [ ] **Step 3: Implement** the functions (edits through the file structs, re-serialized with the writer of Tasks 2 and 3; untouched entries copied verbatim).
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): create prefabs and edit instance overrides`.

---

### Task 5: Spawning prefabs at runtime

**Files:**
- Modify: `crates/ludenea-ecs/src/{commands.rs, prefab.rs, observer.rs, component.rs}`
- Test: `crates/ludenea-app/tests/spawn_prefab.rs`

**Interfaces:**
- Consumes: Task 3 `Prefab`, Task 1 `Handle`.
- Produces (`ludenea_ecs`):
  - `Commands::spawn_prefab(&mut self, prefab: Handle<Prefab>, overrides: impl FnOnce(&mut PrefabOverrides) + Send + 'static)`, applied at the next sync point in issue order: spawns the prefab's entities in prefab order, remaps `Entity` values (positions) to the new entities, then applies the overrides.
  - `pub struct PrefabOverrides` with `set<C: Component>(&mut self, entity: u32, edit: impl FnOnce(&mut C) + Send + 'static) -> &mut Self` (`entity` is the top-level local identifier) and `insert<C: Component>(&mut self, entity: u32, value: C) -> &mut Self`.
  - `Codec` gains `pub remap_entities: fn(&[u8], &dyn Fn(Entity) -> Entity) -> Vec<u8>`: decodes the concrete type, rewrites its `Entity` values through a serde visitor guided by the type (no JSON, so non-finite floats survive), re-encodes. Only components in `entity_components` go through it.
  - Each spawned entity produces a `CommandRecord { kind: Spawn, .. }`.
  - A missing prefab or an unknown local identifier panics with the prefab identifier and the identifier.

- [ ] **Step 1: Write the failing tests**: `spawn_prefab_spawns_entities_in_prefab_order`, `spawned_entity_fields_point_to_new_entities`, `overrides_apply_after_spawn`, `spawn_prefab_is_deterministic_across_thread_counts` (1 and 8 threads, same hashes over 200 ticks), `spawn_prefab_survives_snapshot_and_restore`, `spawn_records_commands`, `remap_keeps_non_finite_floats`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test spawn_prefab`. Expected: compile failure.
- [ ] **Step 3: Implement** the command, `remap_entities`, and the overrides.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(ecs): spawn prefabs from game code with overrides`.

---

### Task 6: Edit log version 2 and versioned undo history

**Files:**
- Modify: `crates/ludenea-app/src/edit.rs`, `crates/ludenea-control/src/{session.rs, host.rs, undo.rs}` (records `EntityRef`; every `Edit::{Despawn, Insert, Remove}` construction), `crates/ludenea-control/tests/{session.rs, undo.rs, editing.rs, host.rs}`, `crates/ludenea-cli/src/commands.rs`
- Create: `crates/ludenea-control/tests/fixtures/session-2a.json` (a real 2a `ExportSession` reply with a non-empty undo history, produced from the commit before this task and committed)
- Test: `crates/ludenea-app/tests/edit.rs`, `crates/ludenea-control/tests/undo.rs`

**Interfaces:**
- Consumes: Task 2 `LoadAddress`, `entity_at_address`; Task 1 asset store.
- Produces (`ludenea_app::edit`):
  - `pub enum EntityRef { Loaded(LoadAddress), Runtime(Entity) }`; `Simulation::entity_ref(&self, entity: Entity) -> EntityRef` (`Loaded` when the entity is in the load map with the generation it was loaded with); `Simulation::resolve_ref(&self, target: &EntityRef) -> Result<Entity, EditError>` (`EditError::NoSuchAddress(LoadAddress)`).
  - `Edit::{Despawn, Insert, Remove}` take `entity: EntityRef`. New variants `SetAsset { id: AssetId, value: Vec<u8> }` and `LoadAsset { id: AssetId, asset: String, value: Vec<u8> }`.
  - `EDIT_LOG_VERSION = 2`; `EditLog::decode` accepts version 1 through mirror types and maps its `Entity` targets to `EntityRef::Runtime`.
  - `Simulation::assets_introduced_by(&self, edit: &Edit, sources: &dyn AssetSource) -> Result<Vec<Edit>, EditError>`: the `LoadAsset` edits needed before `edit` (handles in its values not yet loaded, recursively); `EditError::MissingAsset(AssetId)`. The session starts calling it in Task 13.
- Produces (`ludenea_control::undo`): `UndoHistory::encode` writes `UNDO_VERSION: u32 = 2` first; `decode` reads unversioned 2a bytes through mirror types (old `Edit`, targets as `Runtime`) and version 2 natively.

- [ ] **Step 1: Write the failing tests**: `edit_on_loaded_entity_records_its_address`, `edit_on_runtime_entity_records_its_identifier`, `address_survives_an_entity_added_earlier_in_a_prefab`, `unresolved_address_is_no_such_address`, `set_asset_replaces_value_and_changes_hash`, `assets_introduced_by_lists_missing_handles`, `version_1_edit_log_decodes`; in `undo.rs`: `session_export_from_2a_with_undo_history_resumes` (loads the fixture, undo works).
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test edit && cargo test -p ludenea-control --test undo`. Expected: compile failure.
- [ ] **Step 3: Implement** `EntityRef`, the variants, both version 1 decoders, and the call sites.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): address recorded edits by load address and record asset edits`.

---

### Task 7: Event schemas and stable event names

**Files:**
- Create: `crates/ludenea-schema/src/entities.rs`
- Modify: `crates/ludenea-ecs/src/{event.rs, world.rs, lib.rs}`, `crates/ludenea-macros/src/lib.rs` (`derive(Event)`), `crates/ludenea-app/src/app.rs`, `crates/ludenea-schema/Cargo.toml` (`serde_json` to normal dependencies), `crates/ludenea-physics/src/*` (events derive `Event`; the collision event is named `ludenea::Collision`), `crates/ludenea-physics/editor/index.tsx` (event name), `examples/boxes/src/lib.rs`, every test event type, `crates/ludenea-project/template/src/lib.rs`, the editor fixtures naming events
- Test: inline tests in `entities.rs`, `crates/ludenea-ecs/tests/schema.rs`

**Interfaces:**
- Produces (`ludenea_ecs`): `pub trait Event: Schema + Serialize + Send + 'static` with the stable-name mechanism of `Component`; `#[derive(Event)]` implements it and `Schema`. `App::event::<T: Event>()` (breaking: replaces the `Serialize` bound). Events are keyed by stable name instead of `type_name` everywhere (`World` registry, introspection). `World::event_schemas(&self) -> Vec<(&'static str, TypeSchema)>`.
- Produces (`ludenea_schema::entities`): `pub fn entity_refs(schema: &TypeSchema, value: &serde_json::Value) -> Vec<EntityRefAt>` with `pub struct EntityRefAt { pub path: String, pub index: u32, pub generation: u32 }` (paths such as `hits[2]`). The schema crate does not depend on the ECS, hence plain numbers.

- [ ] **Step 1: Write the failing tests**: `entity_refs_finds_nested_entities_with_paths`, `entity_refs_ignores_structs_shaped_like_entities`, `world_lists_event_schemas_by_stable_name`, `physics_collision_is_named_ludenea_collision`.
- [ ] **Step 2: Run** `cargo test -p ludenea-schema -p ludenea-ecs`. Expected: compile failure.
- [ ] **Step 3: Implement** the trait, derive, registry keys, `entity_refs`, and every event type change.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor test`. Expected: PASS.
- [ ] **Step 5: Commit** `💥 feat(ecs): give events stable names and schemas`.

---

### Task 8: The replay file

**Files:**
- Create: `crates/ludenea-app/src/replay.rs`
- Modify: `crates/ludenea-app/src/{simulation.rs, lib.rs}`, `crates/ludenea-app/Cargo.toml` (`xxhash-rust`), facade
- Test: `crates/ludenea-app/tests/replay.rs`

**Interfaces:**
- Consumes: Task 3 `resolve`, Task 6 `EditLog`, `EntityRef`, milestone 1 `InputLog`.
- Produces (`ludenea_app`): `Simulation::schema_fingerprint(&self) -> u64` (`hash_value` of the sorted component, resource, asset, event, and input schemas); `Simulation::snapshot` fills `Snapshot.fingerprint`.
- Produces (`ludenea_app::replay`):
  - `pub struct Replay { pub version: u32, pub engine: String, pub fingerprint: u64, pub scene: String, pub assets: Vec<(AssetId, String)>, pub inputs: Vec<u8>, pub edits: Vec<u8>, pub hashes: Vec<u64>, pub checkpoints: Vec<(u64, Vec<u8>)> }`: `scene` is `Resolved::to_scene_text`; `assets` the assets loaded **at tick 0**, in file form; `edits` an edit log with every `EntityRef::Loaded` rewritten to `Runtime` of the entity it resolved to when recorded; `checkpoints` snapshot bytes every `REPLAY_CHECKPOINT_INTERVAL` ticks (optional). Postcard; `encode`, `decode` (`ReplayError::Version`). `engine` is `env!("CARGO_PKG_VERSION")` of `ludenea-app`.
  - `pub fn record_replay(sim: &mut Simulation, scene: &str, assets: &dyn AssetSource, inputs: &InputLog, edits: &EditLog, ticks: u64) -> Result<Replay, ReplayError>`.
  - `pub fn play_replay(sim: &mut Simulation, replay: &Replay, on_tick: impl FnMut(u64, u64)) -> Result<ReplayReport, ReplayError>` (`on_tick(tick, total)`): loads the resolved scene, inserts the recorded assets as they are (no reachability), replays; with `pub struct ReplayReport { pub outcome: ReplayOutcome, pub fingerprint_differs: bool }` and `pub enum ReplayOutcome { Matched, Diverged { tick: u64, expected: u64, got: u64 } }`. Missing checkpoints are regenerated while playing; existing ones are checked against the hashes at their tick when used.
  - `pub enum ReplayError` (`thiserror`): `Version { found, expected }`, `Decode { what: String, reason: String }` (`what` such as `input of tick 120`, `edit of tick 300`, `asset 3f2a...`, `scene`).

- [ ] **Step 1: Write the failing tests**: `replay_round_trips`, `recorded_replay_plays_back_matched`, `replay_with_instance_edit_plays_back` (an `Insert` on an instance entity at tick 50), `replay_tick_0_assets_include_instance_prefabs` (a scene with two instances and no handles: matched from tick 0), `asset_loaded_by_a_later_edit_is_not_in_tick_0_assets`, `altered_hash_reports_first_divergence`, `changed_input_type_fails_with_decode_naming_the_tick` (Review Focus), `added_optional_component_field_still_plays_and_flags_fingerprint` (a new `Option` field; `Schema` refuses `#[serde(default)]`), `replay_without_checkpoints_regenerates_them`, `checkpoints_every_1024_ticks`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test replay`. Expected: compile failure.
- [ ] **Step 3: Implement** `replay.rs` and the fingerprint.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): record and play self-contained replays`.

---

### Task 9: Metas, traces, CLI assets and replays, structured differences

**Files:**
- Create: `crates/ludenea-app/src/{meta.rs, trace.rs}` (`Trace` moved from `crates/ludenea-cli/src/trace.rs`, re-exported there), `crates/ludenea-cli/src/assets.rs`
- Modify: `crates/ludenea-project/src/{meta.rs, files.rs}` (use the shared `Meta` and `AssetKind`), `crates/ludenea-cli/src/{commands.rs, lib.rs}`, `crates/ludenea-ecs/src/diff.rs`, `crates/ludenea-ecs/tests/snapshot.rs`, `crates/ludenea-control/src/host.rs` (the `Differences` reply keeps its display lines until Task 15), `crates/ludenea-app/Cargo.toml` (`uuid`)
- Test: inline tests in `meta.rs`, `trace.rs`, `assets.rs`; `crates/ludenea-cli/tests/cli.rs` (extend)

**Interfaces:**
- Produces (`ludenea_app::meta`): `#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] #[cfg_attr(feature = "ts", derive(ts_rs::TS))] pub enum AssetKind { Scene, Prefab, Data { r#type: String }, Replay, Trace, Snapshot, Other }` (no `Copy`); `AssetKind::of(path: &str) -> AssetKind` by extension (never `Data`); `pub struct Meta { pub version: u32, pub id: AssetId, pub kind: AssetKind }` written with `ron::to_string` (a valid RON struct variant for `Data`), read with `ron::from_str`; 2a metas (`Meta(version: 1, id: "...", kind: Scene)`) still read. `Meta::read(path)`, `Meta::load_or_create(path, kind)` move here from the project crate unchanged in behaviour.
- Produces (`ludenea_app::trace`): `Trace` unchanged in format; `pub fn compare_traces(a: &[u64], b: &[u64]) -> TraceComparison` with `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[cfg_attr(feature = "ts", derive(ts_rs::TS))] pub struct TraceComparison { pub common: u64, pub first_divergent: Option<u64>, pub only_in_longer: u64 }`; `Trace::from_replay(&Replay) -> Trace`; `pub fn scene_hash(scene_bytes: &[u8], sim: &Simulation) -> u64` implementing the Global Constraints rule.
- Produces (`ludenea_ecs::diff`): `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[cfg_attr(feature = "ts", derive(ts_rs::TS))] pub struct Difference { pub subject: String, pub component: String, pub path: Vec<String>, pub left: Option<serde_json::Value>, pub right: Option<serde_json::Value>, pub left_text: String, pub right_text: String }`. `left_text` and `right_text` are the milestone 1 RON renderings (`absent` when missing) used by `Display`, which joins `path` with `.`; `left` and `right` come from the codec's JSON (a RON string when there is no JSON form). The walk goes through JSON guided by the schema, so an enum variant change reports the variant names. Assets are compared too (`subject` is `asset <id>`). Snapshots whose `fingerprint` are both non-zero and differ fail with `SnapshotError::Fingerprint { left, right }`.
- Produces (`ludenea_cli::assets`): `pub fn project_assets(scene: &Path, project: Option<&Path>) -> Result<NamedAssetTexts, String>`: root is `project`, else the nearest ancestor holding `ludenea.toml`, else the scene's folder; scans `scenes/` and `assets/` (or the folder itself without a manifest) in sorted path order, reads every `.prefab.ron` and every `.ron` whose meta says `Data` (wrapped as `AssetFile(type: .., value: ..)`), keyed by meta identifier, with paths; a broken meta or a duplicate identifier is an error naming the files.
- Produces (CLI): `run` and `verify` gain `--project <dir>` and load scenes with `load_scene_with`; traces use `scene_hash`; `run --replay <file>` writes a replay; `verify <file.replay>` (detected by extension) plays it, exits 1 on divergence with the milestone 1 snapshot dumps; `--threads` applies to both.

- [ ] **Step 1: Write the failing tests**: `data_meta_round_trips_as_ron`, `metas_from_2a_still_read`, `identical_traces_have_no_divergence`, `first_divergent_tick_is_reported`, `scene_hash_of_assetless_scene_is_raw_bytes` (equals the golden trace's `scene_hash`), `project_root_is_nearest_manifest`, `scene_folder_is_used_without_manifest`, `duplicate_asset_identifiers_are_refused`, `run_writes_a_replay_that_verifies`, `verify_replay_fails_on_altered_hash_and_dumps_snapshots`, `diff_output_lines_unchanged` (milestone 1 expected text), `structured_difference_reports_variant_names`, `asset_only_divergence_is_listed`, `snapshots_with_other_fingerprints_are_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app -p ludenea-cli -p ludenea-ecs -p ludenea-project`. Expected: compile failure.
- [ ] **Step 3: Implement** the moves, `assets.rs`, the flags, replay verification, and structured differences.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(cli): resolve project assets, verify replays, and diff structurally`.

---

### Task 10: The `boxes` spawner scenario and its cross-platform replay

**Files:**
- Create: `examples/boxes/assets/{spawner.scene.ron, crate.prefab.ron, spawner.ron}` and their `.meta` files, `examples/boxes/src/bin/record-replay.rs`, `golden/spawner.replay`
- Modify: `examples/boxes/src/lib.rs` (component `Spawner { prefab: Handle<Prefab>, stats: Handle<SpawnerStats> }`, asset `SpawnerStats { every_ticks: u32, impulse: Vec2 }`, system `spawn_crates` that, for each `Spawner`, spawns the prefab every `every_ticks` ticks and draws its impulse from `Rng` inside that loop), `examples/boxes/tests/golden.rs`, `.github/workflows/determinism.yml`, `README.md`
- Test: `examples/boxes/tests/golden.rs`

**Interfaces:**
- Consumes: Tasks 1 to 9.
- Produces: `golden/spawner.replay`: 3,000 ticks of `spawner.scene.ron` (one `Spawner` entity and two `crate` instances, one override), with one `SetAsset` on `spawner.ron` at tick 1,000 and one `Insert` on an instance entity at tick 2,000. Produced once by `record-replay` and committed; tests never run the generator.

- [ ] **Step 1: Write the failing tests**: `spawner_replay_matches_at_1_2_and_8_threads`, `spawner_replay_survives_snapshot_restore_at_1500`, and keep `boxes_golden` unchanged.
- [ ] **Step 2: Run** `cargo test -p boxes --test golden`. Expected: FAIL (missing files).
- [ ] **Step 3: Implement** the types, files, and generator; run `cargo run --release -p boxes --bin record-replay -- golden/spawner.replay` once and commit the output.
- [ ] **Step 4: Add CI steps** verifying `golden/spawner.replay` natively at 1, 2, and 8 threads on the three operating systems and under `wasmtime`; document both commands in `README.md`.
- [ ] **Step 5: Run** `cargo test --locked --workspace` and the wasmtime verify locally. Expected: PASS.
- [ ] **Step 6: Commit** `✅ test(boxes): add a prefab and data asset scenario as a cross-platform replay`.

---

### Task 11: Timeline branches in the session

**Files:**
- Create: `crates/ludenea-control/src/branches.rs`
- Modify: `crates/ludenea-control/src/{session.rs, checkpoints.rs, host.rs}` (the `Timeline` literal), `crates/ludenea-control/tests/host.rs`, generated TypeScript and the typed editor fixtures
- Test: `crates/ludenea-control/tests/branches.rs`

**Interfaces:**
- Consumes: 2a `Session`, `Checkpoints`.
- Produces (`ludenea_control::branches`):
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)] #[cfg_attr(feature = "ts", derive(ts_rs::TS))] pub struct BranchId(pub u32);` (root `BranchId(0)`, never reused).
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[cfg_attr(feature = "ts", derive(ts_rs::TS))] pub struct BranchInfo { pub id: BranchId, pub name: String, pub parent: Option<BranchId>, pub fork_tick: u64, pub head: u64, pub stale: bool }`.
  - `pub(crate) struct BranchTree`: the stashed segments of non-current branches. A **fork point** is `(fork_tick, parent_edits_at_fork)`: the number of the parent's edits recorded at `fork_tick` that the child inherits. A **segment** is a branch's edits at `fork_tick` after those inherited ones, plus its inputs, edits, hashes, and checkpoints strictly after `fork_tick` (from tick 0 for the root). The current branch lives flattened in the session's linear fields; switching splits the flattened run back into the current branch's segment and composes the target's ancestors up to each fork point plus its own segment.
- Produces (`ludenea_control::session`):
  - **Fork rule** (comment it in `session.rs`): an edit past tick 0, or a take-over, at tick `N` with recorded ticks after `N` on the current branch creates a child forked at `(N, edits already recorded at N)`, named `Branch {k}`, made current; the parent's future stays intact. With nothing recorded after `N`, it extends the current branch. Until Task 12, a fork records the existing `UndoEntry::Run`, and undoing it deletes the child branch.
  - `Session::branches(&self) -> Vec<BranchInfo>`, `current_branch(&self) -> BranchId`, `switch_branch(&mut self, sim, id) -> Result<(), SessionError>` (keeps the tick when covered, else the target's head), `rename_branch(&mut self, id, name: String) -> Result<(), SessionError>`, `delete_branch(&mut self, sim, id) -> Result<Vec<BranchId>, SessionError>` (returns deleted descendants; deleting the current branch switches to its parent at the same tick).
  - `Timeline` gains `branch: BranchId`, `branches: Vec<BranchInfo>`.
  - Checkpoint budget `CHECKPOINT_BUDGET` session-wide: when exceeded, evict checkpoints of stashed branches other than the current branch's parent first, oldest first; never tick 0, a fork tick, or a pinned checkpoint (`Checkpoints::pin`, used by `Session::restore`).
  - `SessionExport` gains `branches: Vec<u8>` (postcard of the tree, current branch, names; empty for a linear session); a 2a export resumes as one root branch.
  - `SessionError::NoSuchBranch(BranchId)`, `RootBranch`.

- [ ] **Step 1: Write the failing tests** (`boxes` app): `playing_forward_never_creates_branches`, `past_edit_forks_and_keeps_parent_future`, `forking_edit_survives_switching_away_and_back` (edit at 200 creates the fork; switch to the parent and back: the edit is applied, the parent's hashes at 200 and after are unchanged), `edits_recorded_before_the_fork_at_the_same_tick_stay_in_the_parent`, `take_over_forks`, `edit_at_head_extends_current_branch`, `switching_keeps_tick_when_covered`, `switching_goes_to_head_when_not_covered`, `sibling_switch_reproduces_each_branch_hashes`, `nested_branch_composes_ancestors`, `rename_branch_keeps_identifier`, `delete_removes_descendants_and_switches_to_parent`, `root_cannot_be_deleted`, `pinned_checkpoints_are_never_evicted`, `export_resume_keeps_the_tree`, `export_from_2a_resumes_as_root`; `#[ignore]` release test `checkpoint_budget_with_20_branches_of_10000_ticks` (at most `CHECKPOINT_BUDGET` checkpoints, every branch seeks to sampled ticks with its recorded hashes).
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test branches`. Expected: compile failure.
- [ ] **Step 3: Implement** `branches.rs`, the fork rule replacing `discard_after` on these two paths, switching, deletion, the budget, export; regenerate TypeScript and fix the fixtures.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): fork the timeline into branches instead of discarding the future`.

---

### Task 12: Undo and redo across branches

**Files:**
- Modify: `crates/ludenea-control/src/{undo.rs, session.rs, branches.rs}`, generated TypeScript
- Test: `crates/ludenea-control/tests/undo.rs` (extend)

**Interfaces:**
- Consumes: Task 11, Task 6 versioned undo history.
- Produces:
  - `UndoEntry::Run { branch: BranchId, edit: TimedEdit, removed: Vec<u8> }`, `UndoEntry::Fork { branch: BranchId, cause: ForkCause, name: String, removed: Vec<u8> }` with `pub(crate) enum ForkCause { Edit(TimedEdit), TakeOver { tick: u64, input: Vec<u8> } }`; `removed` holds the postcard segments of branches the undo removed (empty until undone). `UndoEntry::Scene` unchanged. `UNDO_VERSION = 3`; versions 2 and unversioned 2a bytes decode with `branch: BranchId(0)`.
  - One history per session; undo and redo first switch to the entry's branch.
  - Undoing `Fork` deletes the branch and its descendants and switches to the parent at the same tick; redo recreates them with the same identifiers and names.
  - Undoing `Run` removes the edit, discards that branch's future after its tick, and deletes descendant branches forked **at or after** that tick (they inherit the edit); redo restores them unchanged. Branches forked before it are untouched.
  - `Timeline` gains `undo_label: Option<String>` and `redo_label: Option<String>`, such as `Undo edit at tick 300 (removes Branch 3, Branch 4)`.

- [ ] **Step 1: Write the failing tests**: `undo_fork_returns_to_parent_with_future`, `redo_fork_recreates_same_branch`, `undo_switches_to_entry_branch_first`, `undo_run_removes_descendants_forked_at_or_after_and_redo_restores_them` (including a child forked exactly at the edit's tick), `undo_run_keeps_descendants_forked_before_its_tick`, `undo_label_names_removed_branches`, `undo_survives_export_and_resume_with_branches`, `undo_history_version_2_decodes`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test undo`. Expected: compile failure.
- [ ] **Step 3: Implement** the entries, their undo and redo, and the version 3 decoder; regenerate TypeScript.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): undo forks and edits across timeline branches`.

---

### Task 13: Session assets, documents, instance edits, and stale branches

**Files:**
- Modify: `crates/ludenea-control/src/{session.rs, branches.rs, host.rs}` (callers of `start`, `resume`, `replace_scene`), `crates/ludenea-control/tests/{session.rs, undo.rs, branches.rs}`, generated TypeScript
- Test: `crates/ludenea-control/tests/session_assets.rs`, `crates/ludenea-control/tests/branches.rs` (extend)

**Interfaces:**
- Consumes: Tasks 2 to 6, 11, 12.
- Produces (`ludenea_control::session`):
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] #[cfg_attr(feature = "ts", derive(ts_rs::TS))] pub enum Document { Scene, Prefab }`.
  - `Session::start(sim, scene: String, assets: AssetTexts, document: Document) -> Result<Session, SessionError>`; `Session::resume(sim, export: SessionExport, document: Document) -> Result<Session, SessionError>` (replaces the positional arguments; the expected hashes come from `export.hashes`); `SessionExport` gains `assets: Vec<(AssetId, String)>` (serde default empty). `Session::assets(&self) -> &AssetTexts`; `replace_scene(&mut self, sim, scene: String, assets: AssetTexts)`.
  - `Session::edit` records, before the edit and at the same tick, the `LoadAsset` edits from `assets_introduced_by` against the session's asset texts.
  - Tick 0 edits on instance entities follow the Task 4 rule (override on the scene text, then reload); `Session::place_instance(&mut self, sim, prefab: AssetId) -> Result<u32, SessionError>` and `Session::replace_with_instance(&mut self, sim, entities: &[Entity], prefab: AssetId, prefab_text: String) -> Result<u32, SessionError>` are tick 0 scene edits with undo (`UndoEntry::Scene`). Past tick 0, edits on instance entities are recorded like any edit.
  - Asset edits: at tick 0 an asset edit changes the session's asset text and replays like a scene edit (undo through a new `UndoEntry::Asset { id: AssetId, before: String, after: String }`); past tick 0 it records `Edit::SetAsset`. `Session::apply_to_asset(&mut self, sim, id: AssetId) -> Result<(), SessionError>`, `Session::save_asset(&self, id: AssetId) -> Option<String>`.
  - With `Document::Prefab`, `scene()` returns prefab file text and tick 0 resource edits fail with `SessionError::PrefabHasNoResources`.
  - Staleness: a tick 0 edit, `replace_scene`, or `resume` after a rebuild resimulates the current branch (2a behaviour) and marks every other branch `stale`, dropping its stashed checkpoints (tick 0 excepted). Switching to a stale branch resimulates it from its nearest fresh ancestor's fork point, with `Resimulation` reports, `EditDropped` per branch included.

- [ ] **Step 1: Write the failing tests**: `edit_introducing_a_handle_records_load_asset_first`, `tick_0_edit_on_instance_entity_becomes_override`, `place_instance_appends_and_undoes`, `asset_edit_at_tick_0_replays_session_and_undoes`, `asset_edit_past_tick_0_is_recorded`, `apply_to_asset_copies_value`, `prefab_document_saves_prefab_text`, `prefab_document_refuses_resources`; in `branches.rs`: `scene_edit_marks_other_branches_stale_and_drops_their_checkpoints`, `switching_to_stale_branch_resimulates_it` (Review Focus, also while another resimulation runs), `stale_ancestor_resimulates_first`, `dropped_edit_is_reported_per_branch`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** the signatures, asset texts, documents, instance edits, and stale branches; regenerate TypeScript.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): edit assets and prefab instances in sessions and resimulate stale branches`.

---

### Task 14: Protocol version 3: scenes, assets, branches, sessions

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, lib.rs}`, `crates/ludenea-control/tests/*` (handshake version, `LoadScene` shape), `crates/ludenea-cli/src/commands.rs`, `editor/packages/control-client/src/{client.ts, *}` (`PROTOCOL_VERSION = 3`), `editor/packages/editor-builtins/src/assets/index.tsx` (`load_scene` and `replace_scene` call sites send `assets` and `document`), generated TypeScript, `crates/ludenea-project/types/*.d.ts` (regenerated)
- Test: `crates/ludenea-control/tests/protocol_v3.rs`

**Interfaces:**
- Consumes: Tasks 1 to 13.
- Produces (`PROTOCOL_VERSION = 3`):
  - `GameInfo` gains `assets: Vec<NamedSchema>`, `events: Vec<NamedSchema>`, `fingerprint: Hash64`.
  - `LoadScene { scene: String, assets: Vec<(AssetId, String)>, document: Document }` replies `Loaded { warnings: Vec<String>, instances: Vec<InstanceInfo>, load_addresses: Vec<(Entity, Vec<u32>)> }`; `ReplaceScene { scene, assets }` replies the same.
  - Assets: `GetAsset { id } -> Asset { asset: String, value: ComponentValue }`, `SetAssetField { id, path, value }`, `SaveAsset { id } -> Text { text }`, `ApplyToAsset { id }`, `NewAssetValue { asset: String } -> Text { text }` (the default in file form; `NoDefault` as a protocol error naming the type).
  - Branches: `SwitchBranch { branch }`, `RenameBranch { branch, name }`, `DeleteBranch { branch } -> Deleted { branches }`.
  - `ExportSession` and `LoadSession` carry `assets` and `branches` (serde defaults: 2a exports load).
  - The handshake refuses version 2 clients with the existing message.

- [ ] **Step 1: Write the failing tests**: `handshake_refuses_version_2`, `load_scene_with_assets_reports_instances_and_warnings`, `set_asset_field_records_set_asset_past_tick_0`, `new_asset_value_uses_default`, `new_asset_value_without_default_is_an_error`, `switch_rename_and_delete_branch_over_protocol`, `session_export_from_2a_loads` (the Task 6 fixture).
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test protocol_v3`. Expected: compile failure.
- [ ] **Step 3: Implement** the commands, regenerate the types and `.d.ts`, update the CLI, the TypeScript client constant, and the call sites.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): load scenes with assets and drive branches through protocol version 3`.

---

### Task 15: Protocol version 3: prefabs, replays, differences, traces

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs}`, generated TypeScript, `.d.ts`
- Test: `crates/ludenea-control/tests/protocol_v3.rs` (extend)

**Interfaces:**
- Consumes: Tasks 4, 8, 9, 13, 14.
- Produces:
  - Prefabs: `CreatePrefab { entities: Vec<Entity> } -> Text { text }`, `PlaceInstance { prefab: AssetId } -> Placed { instance: u32 }`, `ReplaceWithInstance { entities: Vec<Entity>, prefab: AssetId, prefab_text: String } -> Placed { instance }`, `ApplyOverride { instance: Vec<u32>, entity: Vec<u32>, component: String, path: Vec<PathSegment>, value: ComponentValue }`, `RevertOverride { instance, entity, component, path }`, `ApplyToPrefab { instance, entity, component, path } -> Text { text }` (the updated prefab file text), `GetInstances -> Instances { instances }`. `ForeignReference`, `PrefabCycle`, and `InstanceEntity` come back as protocol errors with their messages.
  - Replays: `ExportReplay { branch: Option<BranchId> } -> Bytes`, `LoadReplay { replay: Bytes } -> ReplayLoaded { fingerprint_differs: bool }`; the session keeps the replay's recorded hashes as its expected hashes, so a replay recorded with other types reports `Diverged` like a rebuild.
  - `DiffSnapshots { a: Bytes, b: Option<Bytes> }` (`None` is the current tick) replies `Differences { differences: Vec<Difference> }`; a fingerprint mismatch is an error naming both fingerprints.
  - `CompareTraces { a: Bytes, b: Option<Bytes> }` (`None` is the current branch) `-> TraceComparison`.

- [ ] **Step 1: Write the failing tests**: `create_prefab_returns_prefab_text`, `place_instance_over_protocol`, `apply_override_then_revert`, `apply_to_prefab_returns_updated_prefab`, `export_and_load_replay_round_trip`, `load_replay_with_broken_input_names_the_tick` (Review Focus), `load_replay_with_changed_fingerprint_reports_first_divergent_tick` (Review Focus), `diff_snapshots_is_structured`, `diff_snapshot_against_current_tick`, `snapshot_with_other_fingerprint_is_refused`, `compare_traces_against_current_branch`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test protocol_v3`. Expected: compile failure.
- [ ] **Step 3: Implement** the commands; regenerate the types and `.d.ts`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): edit prefabs, exchange replays, and compare runs over the protocol`.

---

### Task 16: The compare simulation and the ghost

**Files:**
- Create: `crates/ludenea-control/src/compare.rs`
- Modify: `crates/ludenea-control/src/{host.rs, protocol.rs, session.rs, worker.rs, stdio.rs}`, `crates/ludenea-cli/src/commands.rs`, every `Host::new` caller: `crates/ludenea-control/tests/{editing.rs, introspection.rs, host.rs, running.rs}` (the `panic_at_50` app and the `loaded_with` helper become named `fn() -> App` builders), `crates/ludenea-cli/tests/cli.rs`, `examples/boxes/tests/golden.rs`; generated TypeScript, `.d.ts`
- Test: `crates/ludenea-control/tests/compare.rs`

**Interfaces:**
- Consumes: Tasks 11, 13, 14.
- Produces:
  - `Host::new(build: fn() -> App) -> Result<Host, AppError>` (was `App`); the host builds a second `Simulation` lazily for the compare branch.
  - `pub(crate) struct CompareRun`: owns the second simulation, the compare branch, and its position. Before restoring any checkpoint it loads the session's scene and assets with `load_scene_with`, so load addresses resolve. `follow(&mut self, session: &Session, tick: u64, budget: u32) -> CompareState` (with `pub(crate) struct CompareState { pub tick: u64, pub lagging: bool }`) runs at most `budget` ticks toward `tick` on the compare branch's flattened run and checkpoints; `COMPARE_BUDGET_PLAYING` per `advance` while playing, `COMPARE_BUDGET_PAUSED` while paused.
  - `Command::SetCompareBranch { branch: Option<BranchId> }`; `Timeline` gains `compare: Option<BranchId>`; forking sets the compare branch to the parent.
  - `Command::GetGhostDraw { tick, layers } -> GhostDraw { frame: DebugDrawFrame, tick: u64, lagging: bool }`; outside the compare branch's range, an empty frame.
  - `Topic::Ghost`; `Event::GhostAdvanced { tick }`.

- [ ] **Step 1: Write the failing tests**: `ghost_draws_compare_branch_at_current_tick`, `ghost_reports_lagging_then_catches_up`, `compare_defaults_to_parent_on_fork`, `ghost_never_changes_main_hashes`, `ghost_outside_range_is_empty`, `compare_branch_with_loaded_address_edit_matches_its_recorded_hashes`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test compare`. Expected: compile failure.
- [ ] **Step 3: Implement** `compare.rs`, the builder signature change, and the commands.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): run the compare branch beside the timeline and draw its ghost`.

---

### Task 17: The causal divergence report

**Files:**
- Create: `crates/ludenea-control/src/divergence.rs`
- Modify: `crates/ludenea-control/src/{host.rs, protocol.rs, compare.rs, subscriptions.rs}`, generated TypeScript, `.d.ts`
- Test: `crates/ludenea-control/tests/divergence.rs`

**Interfaces:**
- Consumes: Task 16 `CompareRun`, Task 7 `entity_refs` and event schemas, Task 9 `Difference`, 2a introspection recording.
- Produces (`ludenea_control::divergence`), all `Serialize` and ts:
  - `pub struct DivergenceReport { pub fork_tick: u64, pub up_to: u64, pub complete: bool, pub first_divergent: Option<u64>, pub root_cause: Option<RootCause>, pub entities: Vec<EntityDivergence> }` (entities ordered by first divergent tick, then entity).
  - `pub struct RootCause { pub tick: u64, pub kind: RootCauseKind, pub entities: Vec<Entity> }`, `pub enum RootCauseKind { Edit { description: String }, TakeOver }`: the fork's edit or take-over, and the entities divergent at the first divergent tick.
  - `pub struct EntityDivergence { pub entity: Entity, pub kind: DivergenceKind, pub first_tick: u64, pub first_difference: Option<Difference>, pub causes: Vec<Cause> }`; `pub enum DivergenceKind { Changed, OnlyInCurrent, OnlyInCompare, Replaced }` (`Replaced` when the component sets differ).
  - `pub enum Cause { Event { tick: u64, event: String, path: String, other: Entity }, Command { tick: u64, system: String, kind: String } }`: events at the entity's first divergent tick or the tick before that reference the entity and an entity already divergent then (through `entity_refs`, event named by its stable name); commands targeting the entity at that tick, `system` being the name of the system at the observer's index in `Schedule::systems`.
  - `Command::GetDivergence { up_to: Option<u64> } -> Divergence(DivergenceReport)`: incremental inside `advance` (both simulations step from the fork with introspection recording), `complete: false` until done; `Topic::Divergence`, `Event::DivergenceProgress { up_to }`. Default range `fork_tick + DIVERGENCE_TICKS`. Nothing runs unless asked or subscribed.

- [ ] **Step 1: Write the failing tests** (`boxes`, fork at 200 by setting one crate's velocity): `edited_entity_diverges_first_with_the_edit_as_root_cause`, `take_over_is_root_cause`, `collided_entity_lists_the_collision_event_as_cause` (`event == "ludenea::Collision"`, `other` the edited crate), `prefab_spawn_shows_as_command_cause` (spawner scene), `entity_only_in_one_branch_is_reported`, `different_component_set_is_replaced`, `report_is_incremental_and_completes`, `no_work_without_request`; `#[ignore]` release test `report_over_600_ticks_under_one_second`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test divergence`. Expected: compile failure.
- [ ] **Step 3: Implement** `divergence.rs` (per tick: compare entity hash trees; for each newly divergent entity, diff its components restricted to that entity, then scan recorded events and commands) and the command.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`, then `cargo test --release -p ludenea-control --test divergence -- --ignored`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): explain divergence between branches entity by entity`.

---

### Task 18: Project service version 2

**Files:**
- Modify: `crates/ludenea-project/src/{files.rs, protocol.rs, service.rs, types.rs, template.rs}`, `crates/ludenea-project/template/{scenes/main.scene.ron, src/lib.rs}`, create `crates/ludenea-project/template/assets/{ball.prefab.ron, settings.ron}` and their metas, `editor/packages/control-client/src/project.ts` (`PROJECT_PROTOCOL_VERSION = 2`), generated project TypeScript, `.d.ts`
- Test: `crates/ludenea-project/tests/assets.rs`

**Interfaces:**
- Consumes: Task 9 shared `Meta` and `AssetKind`, Task 2 `NamedAssetTexts`, Task 14 host.
- Produces:
  - Project protocol version 2 requests: `ReadBinary { path } -> Bytes`, `WriteBinary { path, bytes }`, `CreateAsset { path, kind: AssetKind, text: String } -> FileEntry` (file and meta written atomically), `ReadAssetTexts -> AssetTexts { assets: Vec<(AssetId, String)>, paths: Vec<(AssetId, String)> }` (every prefab and data asset in sorted path order, data assets in file form; duplicate identifiers are an error naming both files), `StoreSession { document: AssetId, bytes }`, `LoadStoredSession { document } -> Option<Bytes>` (`.ludenea/sessions/<id>.session`).
  - Changes on disk to prefabs and data assets emit the file change event with their kind.
  - `WriteGameTypes` also writes the game's `AssetTypes` declarations (augmenting `editor-shell`'s interface, Task 19).
  - Template: `ball.prefab.ron` (a dynamic circle body), placed twice in `main.scene.ron` with one override; `settings.ron` (`Data { type: "<template crate>::Settings" }`, a gravity scale read through a handle on a component).

- [ ] **Step 1: Write the failing tests**: `create_asset_writes_file_and_meta`, `asset_texts_wrap_data_assets_with_their_type_in_path_order`, `duplicate_identifiers_are_refused_naming_both_files`, `stored_session_round_trips`, `broken_data_asset_is_refused_and_previous_value_kept` (Review Focus, with an in-process host: `ReplaceScene` fails with `InvalidAsset` carrying the path, line, and column; the session's asset is unchanged), `template_project_loads_with_prefab_and_data_asset` (`#[ignore]`, real build).
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test assets`. Expected: compile failure.
- [ ] **Step 3: Implement** the requests, template files, and type generation; regenerate TypeScript and `.d.ts`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(project): know prefabs, data assets, replays, traces, and stored sessions`.

---

### Task 19: Editor shell: documents, branches, assets, inspector decorations, explorer columns

**Files:**
- Create: `editor/packages/editor-shell/src/stores/{document.ts, branches.ts}`, `editor/packages/editor-shell/src/assets.ts`
- Modify: `editor/packages/editor-shell/src/{editor.ts, app.ts, index.ts, plugin.ts}`, `editor/packages/control-client/src/*` (typed wrappers for the new commands and requests), `editor/packages/editor-builtins/src/{project, scene-tree, inspector, explorer, console}/*` (open documents through the store; render decorations and columns; asset errors logged with their path), `editor/apps/web/src/*` and `editor/apps/desktop/src/*` (crash recovery and close export branches and assets, then `StoreSession`), `.d.ts`
- Test: `editor/packages/editor-shell/test/{document.test.ts, branches.test.ts, decorations.test.tsx}`

**Interfaces:**
- Consumes: Tasks 14, 15, 16, 18.
- Produces (`Editor` additions):
  - `document: DocumentStore`: `{ current: { id: AssetId; kind: "scene" | "prefab" | "replay"; path: string } | null; open(id: AssetId): Promise<void> }`. `open` stores the current session (`ExportSession`, `StoreSession`), loads the target (`ReadAssetTexts`, then `LoadScene` or `LoadReplay`, then `LoadStoredSession` and `LoadSession` when stored). A stored session whose scene or assets changed meanwhile resimulates and shows the first divergent tick (2a behaviour). Sessions are also stored on close and on the 2a crash recovery schedule.
  - `branches: BranchStore` (Zustand): `{ list: BranchInfo[]; current: BranchId; compare: BranchId | null; switch(id: BranchId): Promise<void>; rename(id: BranchId, name: string): Promise<void>; remove(id: BranchId): Promise<BranchId[]>; setCompare(id: BranchId | null): Promise<void> }`, kept from `TimelineChanged`.
  - `assets: { get<N extends keyof AssetTypes>(id: AssetId): Promise<AssetTypes[N]>; setField(id: AssetId, path: PathSegment[], value: unknown): Promise<void>; create(type: string, path: string): Promise<{ kind: "created"; entry: FileEntry } | { kind: "needs-value"; schema: TypeSchema }> }`; `create` sends `NewAssetValue` and returns `needs-value` on `NoDefault`, which the asset browser turns into a schema form (Task 21). `editor-shell` exports `export interface AssetTypes {}` that generated game declarations augment.
  - `inspectors.decorate(decoration: FieldDecoration): Disposable` with `FieldDecoration = { id: string; render(props: { editor: Editor; entity: Entity; component: string; path: PathSegment[] }): ReactNode }`.
  - `explorer.columns.register(column: { id: string; title: string; width: number; value(entity: Entity): string | null }): Disposable`, rendered by the explorer.
  - Asset errors from the host are logged with the asset's path (looked up in the project file list), line, and column.

- [ ] **Step 1: Write the failing tests**: `opening_a_document_stores_the_previous_session`, `reopening_restores_branches_and_tick`, `stored_session_replays_when_prefab_changed_meanwhile`, `branches_survive_editor_restart`, `branch_store_follows_timeline_events`, `asset_create_writes_default_value`, `asset_create_without_default_needs_value`, `decorations_render_beside_fields`, `explorer_columns_render`, `broken_asset_error_names_path_line_and_column` (Review Focus).
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the stores, editor additions, and call sites; regenerate `.d.ts`.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): open one document at a time and expose branches and assets to plugins`.

---

### Task 20: The prefabs plugin

**Files:**
- Create: `editor/packages/editor-builtins/src/prefabs/{index.tsx, InstanceGroups.tsx, OverrideMark.tsx}`
- Modify: `editor/packages/editor-builtins/src/{index.ts, scene-tree/SceneTree.tsx, inspector/fields/*}` (a `Handle` field editor: asset picker filtered by asset type)
- Test: `editor/packages/editor-builtins/test/prefabs.test.tsx`

**Interfaces:**
- Consumes: Task 19 `document`, `inspectors.decorate`; Task 15 prefab commands.
- Produces: `prefabsPlugin`:
  - Scene tree: instances as collapsible groups named after their prefab file (`Loaded.instances`, `GetInstances`); a scene load error shows on the tree with its message (Review Focus); `Loaded.warnings` go to the console, one line per stale override (Review Focus).
  - Inspector decoration: overridden fields marked; "Revert to prefab" and "Apply to prefab" per field and per component (`RevertOverride`; `ApplyToPrefab` then `WriteText` of the returned prefab text).
  - Commands: `prefabs.place` ("Place prefab": picker, `PlaceInstance`), `prefabs.create-from-selection` (`CreatePrefab`, `CreateAsset` of kind `Prefab` under `assets/`, `ReplaceWithInstance`), `prefabs.open` (the selected instance's prefab as the document).

- [ ] **Step 1: Write the failing tests**: `instances_render_as_groups`, `override_mark_shows_on_overridden_field_only`, `revert_sends_revert_override`, `apply_writes_returned_prefab_text`, `place_prefab_sends_place_instance`, `create_from_selection_creates_prefab_asset`, `missing_prefab_error_shows_on_tree`, `stale_override_warnings_reach_console`, `handle_field_lists_assets_of_its_type`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the plugin and the handle field.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): place, override, and create prefabs`.

---

### Task 21: Data assets, replays, traces, and snapshots in the asset browser

**Files:**
- Modify: `editor/packages/editor-builtins/src/assets/*`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx`, `editor/packages/editor-builtins/src/timeline/TimelinePanel.tsx` (replay divergence banner)
- Test: `editor/packages/editor-builtins/test/assets.test.tsx` (extend)

**Interfaces:**
- Consumes: Task 19 `assets`, `document`.
- Produces:
  - Asset browser: icons and filters per kind; "New data asset" (type from `GameInfo.assets`, then `editor.assets.create`; on `needs-value`, a form built from the schema with every field required, written with `CreateAsset` when complete); scenes, prefabs, and replays open through `editor.document.open`; traces and snapshots run `divergence.open-file` with the path.
  - Selection gains `{ kind: "asset", id }`; the inspector edits a selected data asset through its schema (`GetAsset`, `SetAssetField`), with "Apply to asset" past tick 0 and the unsaved marker plus explicit save (`SaveAsset`, `WriteText`) at tick 0.
  - A replay loaded with `fingerprint_differs` shows a banner on the timeline ("recorded with other game types") and the first divergent tick when reported.

- [ ] **Step 1: Write the failing tests**: `new_data_asset_lists_game_asset_types`, `new_data_asset_without_default_requires_every_field`, `opening_replay_uses_document_store`, `opening_trace_runs_divergence_command`, `inspector_edits_selected_data_asset`, `apply_to_asset_shown_past_tick_0`, `replay_with_other_types_shows_banner_and_divergence` (Review Focus).
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the browser, inspector, and banner changes.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): create and edit data assets and open replays from the asset browser`.

---

### Task 22: Branches in the timeline

**Files:**
- Create: `editor/packages/editor-builtins/src/timeline/BranchLanes.tsx`
- Modify: `editor/packages/editor-builtins/src/timeline/{TimelinePanel.tsx, index.tsx, editing.tsx}`
- Test: `editor/packages/editor-builtins/test/branches.test.tsx`

**Interfaces:**
- Consumes: Task 19 `branches`, Task 15 `ExportReplay`.
- Produces:
  - Branch lanes: one lane per branch with fork points and edit markers, current lane highlighted, compare lane marked, stale branches marked; clicking a lane switches.
  - Commands: `timeline.switch-branch` (picker), `timeline.rename-branch`, `timeline.delete-branch` (confirmation naming the descendants), `timeline.set-compare-branch`, `timeline.save-replay` (`ExportReplay`, `WriteBinary`, `CreateAsset` of kind `Replay` under `assets/replays/`).
  - Undo and redo labels from `Timeline.undo_label` and `redo_label` in the palette.

- [ ] **Step 1: Write the failing tests**: `lanes_render_one_per_branch_with_fork_points`, `clicking_lane_switches_branch`, `delete_confirmation_names_descendants`, `save_replay_writes_replay_asset`, `undo_label_names_removed_branches`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the lanes and commands.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): show, switch, and save timeline branches`.

---

### Task 23: The divergence plugin and the ghost overlay

**Files:**
- Create: `editor/packages/editor-builtins/src/divergence/{index.tsx, TraceViewer.tsx, SnapshotDiff.tsx, CausalReport.tsx}`
- Modify: `editor/packages/editor-builtins/src/{index.ts, viewport/*}`, `crates/ludenea-debug-draw/src/*` (ghost style: outlines only, reduced opacity, same primitives)
- Test: `editor/packages/editor-builtins/test/divergence.test.tsx`

**Interfaces:**
- Consumes: Tasks 15, 16, 17, 19, 21.
- Produces: `divergencePlugin`:
  - Trace viewer: a strip of ticks with the first divergent tick (`CompareTraces`), trace against trace or against the current branch; selecting the tick seeks there when the current session is one side and opens the snapshot comparison against the current tick.
  - Snapshot diff: a tree entity, component, field with left and right values formatted through the schemas (`DiffSnapshots`); variant changes show variant names; a fingerprint refusal shows its message.
  - Causal report panel: root cause first, then entities by tick (`GetDivergence`, progress from `DivergenceProgress`, "Extend" passing a larger `up_to`); selecting an entry seeks and selects. Computed only while the panel is open.
  - Explorer column `divergence.diverged-at` through `editor.explorer.columns.register`; empty while no report exists.
  - Viewport overlay `divergence.ghost` through `editor.overlays.register`: draws `GetGhostDraw` frames in the ghost style while a compare branch is set; command `divergence.toggle-ghost`; a "lagging" badge.
  - Command `divergence.open-file { path }`.

- [ ] **Step 1: Write the failing tests**: `trace_strip_marks_first_divergent_tick`, `snapshot_diff_groups_by_entity_component_field`, `fingerprint_refusal_is_shown`, `causal_report_starts_from_root_cause`, `selecting_cause_seeks_and_selects`, `explorer_column_shows_divergence_tick`, `ghost_overlay_draws_only_with_compare_branch`, `lagging_badge_shows_when_ghost_lags`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the plugin, the overlay, and the ghost style.
- [ ] **Step 4: Run** `pnpm -C editor build:wasm && pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): view trace divergence, explain it causally, and draw the ghost branch`.

---

### Task 24: End-to-end proofs, continuous integration, README

**Files:**
- Create: `editor/e2e/{prefabs.spec.ts, branches.spec.ts, divergence.spec.ts, replay.spec.ts}`, `editor/e2e/fixtures/divergence/{a.trace, b.trace, a.snap, b.snap, broken-input.replay}` (produced by a small test helper from the `boxes` app, committed)
- Modify: `editor/e2e/golden.spec.ts`, `editor/e2e/performance.spec.ts`, `.github/workflows/determinism.yml`, `README.md`
- Test: the files above

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Write the end-to-end tests** on the template project in the browser editor:
  - `prefabs.spec.ts`: `place_override_and_edit_prefab` (place a `ball` instance, override its radius, open the prefab, change its mass, return: the override is kept, the session replayed, the first divergent tick shown).
  - `branches.spec.ts`: `fork_switch_and_keep_parent_future` (play 300 ticks, seek 100, edit a ball: two lanes; switching shows both futures; undo removes the branch, redo restores it).
  - `divergence.spec.ts`: `ghost_and_causal_report` (the ghost is visible; the report lists the edited ball at the fork tick and the ball it later hits, with `ludenea::Collision` as cause), `trace_and_snapshot_viewer` (the fixtures show the expected tick, entity, component, and field).
  - `replay.spec.ts`: `saved_replay_verifies_in_cli` (save the branch as a replay; CLI `verify` exits 0), `replay_from_changed_types_is_explained` (the broken fixture shows the decode message).
  - `golden.spec.ts`: `spawner_replay_matches_in_browser`.
  - `performance.spec.ts`: the 2a benchmark with a branch and the ghost on keeps p95 frame time at or under 17 ms (under `PERF_STRICT=1`); `#[ignore]` Rust test `switch_branch_under_200_ms` in `crates/ludenea-control/tests/branches.rs` (10,000-tick session, a checkpoint within 64 ticks).
- [ ] **Step 2: Run** `pnpm -C editor e2e`. Expected: PASS.
- [ ] **Step 3: Update CI**: the editor job runs the new specs; a macOS job runs the `#[ignore]` timing and memory tests in release with `continue-on-error: true`, printing results.
- [ ] **Step 4: Update** `README.md`: prefabs, data assets, replays (`verify golden/spawner.replay`), branches, and the divergence tools, one short section each.
- [ ] **Step 5: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor e2e`. Expected: PASS.
- [ ] **Step 6: Commit** `✅ test(e2e): prove prefabs, branches, divergence, and replays end to end`.
