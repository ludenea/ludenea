# Ludenea Milestone 2b: The Editor That Changes The Game Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prefabs and data assets that the simulation reads deterministically, replays as self-contained verifiable assets, a timeline that forks into branches instead of discarding the future, and an editor that shows another branch as a ghost and explains, entity by entity, how and why two futures diverge, plus a viewer for CI traces and snapshots.

**Architecture:** `ludenea-ecs` gains typed handles, an asset store with its own conditional hash leaf, prefab spawning, and structured snapshot diffs. `ludenea-app` gains scene version 2 (stable local identifiers, prefab instances), resolution of scenes against asset texts, edit log version 2 (load addresses, asset edits), and the replay file. `ludenea-control` turns the linear session into a branch tree, adds a second simulation for the compare branch (ghost and causal report), and moves to protocol version 3. The project service learns the new asset kinds and stores sessions per document. The editor adds `prefabs` and `divergence` plugins and extends `assets`, `inspector`, `timeline`, and `viewport`, all on the public plugin API.

**Tech Stack:** unchanged from milestone 2a (Rust 1.97.1, serde, postcard, ron, xxhash, ts-rs, wasm-bindgen, wgpu, rolldown, axum, Tauri 2; TypeScript strict, React 19, Vite, pnpm, Vitest, Playwright, dockview, Zustand). No new dependency.

**Spec:** `.claude/superpowers/specs/2026-10-18-ludenea-editor-2b-design.md` (parents: `.claude/superpowers/specs/2026-10-17-ludenea-editor-2a-design.md`, `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md`). The 2a plan `.claude/superpowers/plans/2026-10-17-ludenea-milestone-2a-usable-editor.md` describes the existing code.

## Global Constraints

- Every Global Constraint of the milestone 1 and 2a plans still applies (determinism rules for simulation crates, clippy allowances outside the simulation, target gates, pinned `wasm-bindgen` and rolldown, no Node.js at runtime, committed generated files, `#[ignore]` on tests running a real `cargo build`, keys, commit style).
- `golden/boxes.trace` is never regenerated. The existing golden tests (`examples/boxes/tests/golden.rs`, `editor/e2e/golden.spec.ts`, the CI verify jobs) pass unchanged at the end of every task. In particular `examples/boxes` registers no new resource (a registered resource is a hash leaf of every world); new `boxes` types are components and asset types only.
- `ludenea::Assets` hash leaf: present in `HashTree.resources` (after `ludenea::Entities`) only when at least one asset is loaded.
- Formats and versions: `SCENE_VERSION = 2` (version 1 still loads), `EDIT_LOG_VERSION = 2` (version 1 still decodes), `SNAPSHOT_VERSION = 2` (version 1 refused with a clear error), `REPLAY_VERSION = 1`, `PREFAB_VERSION = 1`, `PROTOCOL_VERSION = 3`, project protocol version 2. 2a session exports (no branches, no assets) load through `LoadSession`.
- Stable names: `ludenea::Prefab` (asset type), `ludenea::Assets` (hash leaf). Asset type names follow components: `module_path!()::Type`.
- File kinds by extension: `.scene.ron` scene, `.prefab.ron` prefab, `.replay` replay, `.trace` trace, `.snap` snapshot. A data asset is any `.ron` file whose meta says `Data { type }`; only the meta decides it.
- Replay checkpoints: every `REPLAY_CHECKPOINT_INTERVAL = 1024` ticks. Divergence report default range: `DIVERGENCE_TICKS = 600` ticks after the fork.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C editor test`, and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- `Assets<T>` is its own system parameter (`assets: Assets<WeaponStats>`), not `Res<Assets<T>>`: assets are not resources (they are not in scenes, not edited as resources, and have their own hash leaf).
- `Edit::LoadAsset` carries the asset's type name and canonical bytes, not only its identifier, so an edit log stays self-contained like the rest of the run definition and the CLI needs no asset lookup to replay it.
- The game host receives every prefab and data asset text of the project with `LoadScene` and keeps only those reachable from the scene; the project service does not know game schemas, so it cannot compute reachability.
- The compare simulation is interleaved with the main one inside `Host::advance` on every target, under a per-advance budget, instead of running on its own thread on native. A thread comes if the ghost measurably slows play.
- In scene and prefab files version 2, an `Entity` value names a local identifier in `index` (written with `generation: 0`, generation ignored on load); loading remaps it. `Entity` values inside recorded edit bytes stay raw entity identifiers.
- Instances and scene entities share one local identifier space per file, so a load address is a plain path of identifiers.

## Review Focus

- **A prefab file deleted (or its meta lost) while a scene references it**: the scene load is refused with `SceneError::MissingAsset` naming the instance identifier and the asset identifier; the scene file is untouched and the editor shows the error in the console and on the scene tree, with the previous session kept. Tests in Tasks 3 and 19.
- **A prefab edited so that an instance override targets an entity or component that no longer exists**: the scene loads, the console shows one warning per stale override, and saving the scene keeps the stale overrides byte for byte. Test in Task 3.
- **Switching to a stale branch** (marked stale by a scene edit or rebuild) while another resimulation runs: the switch succeeds, the target branch resimulates from its fork with progress, and seeking past the resimulated point fails with `Resimulating` as in 2a. Test in Task 12.
- **Opening a replay recorded with different game types**: when everything decodes, it plays and reports the first divergent tick against its own hashes; when a value fails to decode (input type changed), `LoadReplay` fails with `ReplayError::Decode` naming the first value (scene component, asset, input of tick N, or edit of tick N) and the reason. Tests in Tasks 7 and 14.
- **A data asset file broken on disk during a session** (RON error, wrong type): the reload is refused, the console shows the path, line, and column, and the session keeps the previous asset value. Test in Task 17.

---

### Task 1: Handles, asset types, and the asset store

**Files:**
- Create: `crates/ludenea-ecs/src/assets.rs`
- Modify: `crates/ludenea-ecs/src/{asset.rs, world.rs, hash.rs, snapshot.rs, system.rs, lib.rs}`, `crates/ludenea-schema/src/{lib.rs, typescript.rs}`, `crates/ludenea-macros/src/lib.rs` (`derive(Asset)`), `crates/ludenea-app/src/app.rs` (`App::asset`), facade prelude (`Asset`, `Assets`, `Handle`)
- Test: `crates/ludenea-ecs/tests/assets.rs`, inline tests in `crates/ludenea-schema/src/typescript.rs`

**Interfaces:**
- Produces (`ludenea_schema`): `TypeSchema::Handle { asset: String }` (the asset type's stable name); TypeScript maps it to `string`. `typescript_declarations` also emits `export interface AssetTypes { "<stable name>": <Type>; ... }` from a second list argument: `typescript_declarations(components: &[(&str, &TypeSchema)], assets: &[(&str, &TypeSchema)]) -> String` (update every caller).
- Produces (`ludenea_ecs`):
  - `pub trait Asset: Schema + Serialize + DeserializeOwned + Send + Sync + 'static` with the same stable-name mechanism as `Component`; `#[derive(Asset)]` implements it and `Schema`.
  - `pub struct Handle<T: Asset> { pub id: AssetId, .. }` (`Copy`, `Eq`, `Ord`, `Debug`), serialized exactly as its `AssetId` (so its bytes and hash equal the identifier's), `impl Schema` giving `TypeSchema::Handle { asset: T::NAME }`, `Handle::new(id) -> Self`.
  - `World::register_asset::<T: Asset>(&mut self) -> Result<(), EcsError>` (codec as for components: encode, decode, hash, RON, JSON, schema); `World::asset_schemas(&self) -> Vec<(&'static str, TypeSchema)>` sorted by name.
  - `World::insert_asset(&mut self, id: AssetId, type_name: &str, bytes: &[u8]) -> Result<(), EcsError>` (inserts or replaces), `World::insert_asset_ron(&mut self, id: AssetId, type_name: &str, ron: &str) -> Result<(), EcsError>` (`EcsError::InvalidAsset { id, type_name, reason }` with the RON position in `reason`), `World::asset_ids(&self) -> Vec<AssetId>`, `World::asset_bytes(&self, id: AssetId) -> Option<(&str, Vec<u8>)>`, `World::asset_ron(&self, id: AssetId) -> Option<String>`, `World::clear_assets(&mut self)`.
  - System parameter `pub struct Assets<'w, T: Asset>` with `get(&self, handle: Handle<T>) -> &T` (panics with "asset {id} of type {name} is not loaded") and `try_get(&self, handle: Handle<T>) -> Option<&T>`. Read-only access: it never conflicts with another system.
  - `pub const ASSETS_LEAF: &str = "ludenea::Assets";` In `hash_tree`, when the store is non-empty, a `NamedHash { name: ASSETS_LEAF, hash }` follows the `ludenea::Entities` leaf, `hash` being xxh3 over, in identifier order, each identifier's 16 little-endian bytes followed by the 8-byte hash of its canonical bytes.
  - `Snapshot` (version 2) gains `pub assets: Vec<(AssetId, String, Vec<u8>)>` (identifier order, type name, canonical bytes); `restore` replaces the store. Decoding version 1 fails with `SnapshotError::Version { found: 1, expected: 2 }`.
- Produces (`ludenea_app`): `App::asset::<T: Asset>(&mut self) -> &mut Self`; `Simulation::asset_schemas(&self) -> Vec<(&'static str, TypeSchema)>`.

- [ ] **Step 1: Write the failing tests**: `handle_encodes_and_hashes_like_its_asset_id`, `handle_schema_names_its_asset_type`, `world_without_assets_hashes_as_before` (hash tree of a world with a registered but empty asset type has no `ludenea::Assets` leaf and the same root as without registration), `loaded_asset_adds_the_assets_leaf_after_entities`, `asset_hash_ignores_ron_formatting` (two RON texts of the same value, same root), `assets_param_reads_loaded_asset`, `assets_param_panics_naming_missing_asset`, `snapshot_restores_assets`, `snapshot_version_1_is_refused`, `invalid_asset_ron_reports_position`; in `typescript.rs`: `asset_types_interface_lists_assets`.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs -p ludenea-schema`. Expected: compile failure.
- [ ] **Step 3: Implement** the schema variant, the derive, `assets.rs` (store and system parameter), the hash leaf, snapshot version 2, and `App::asset`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS, golden tests included.
- [ ] **Step 5: Commit** `✨ feat(ecs): add typed asset handles and a content-hashed asset store`.

---

### Task 2: Scene version 2 and asset loading

**Files:**
- Create: `crates/ludenea-app/src/assets.rs`
- Modify: `crates/ludenea-app/src/{scene.rs, simulation.rs, lib.rs}`, `crates/ludenea-control/src/session.rs` (calls `load_scene_with`)
- Test: `crates/ludenea-app/tests/scene_assets.rs`

**Interfaces:**
- Consumes: Task 1 asset store and `TypeSchema::Handle`.
- Produces (`ludenea_app::assets`):
  - `pub trait AssetSource { fn text(&self, id: AssetId) -> Option<&str>; }`; `pub struct NoAssets;` and `pub type AssetTexts = BTreeMap<AssetId, String>;`, both implementing it.
  - `pub fn reachable_handles(schema: &TypeSchema, value: &ron::Value, out: &mut BTreeSet<(AssetId, String)>)`: every `Handle` (identifier and asset type name) inside a value, walked by schema. A bare `AssetId` field is not a handle and is not followed.
- Produces (`ludenea_app::scene`):
  - Scene version 2 file shape: `Scene(version: 2, resources: {...}, entities: [(id: u32, components: {...})], instances: [...])`; `instances` is defined in Task 3 and defaults to empty. Version 1 still loads (entity identifiers become positions).
  - `pub type LoadAddress = Vec<u32>;` (a path of local identifiers; one element for a scene entity).
  - `Simulation::load_scene_with(&mut self, ron: &str, assets: &dyn AssetSource) -> Result<(), SceneError>`: decodes, then loads every asset reachable from the scene's components and resources, recursively through loaded assets, into the store (clearing it first); `load_scene(&mut self, ron)` becomes `load_scene_with(ron, &NoAssets)`.
  - `Entity` values in a version 2 file name local identifiers and are remapped to the spawned entities; an unknown identifier fails with `SceneError::UnknownLocal { entity: u32, component: String, local: u32 }`.
  - `Simulation::load_addresses(&self) -> &BTreeMap<Entity, LoadAddress>` and `Simulation::entity_at_address(&self, address: &[u32]) -> Option<Entity>`, from the last load; not part of the state or the hash.
  - `SceneError` gains `MissingAsset { referenced_by: String, id: AssetId }` (`referenced_by` such as `entity 7, ludenea::Turret.bullet` or `asset 3f2a...`), `WrongAssetType { id, expected: String, found: String }`, `InvalidAsset { id, reason }`, `DuplicateLocal(u32)`, `UnknownLocal {..}`.
  - `Simulation::save_scene(&self) -> String` writes version 2 with identifiers `0..n` and remapped `Entity` values; `Simulation::save_scene_keeping(&self, previous: &str) -> String` keeps the identifiers and instances of `previous` for entities that still have a load address (used by the session for tick 0 edits).
  - `Simulation::assets_ron(&self) -> AssetTexts`: the loaded assets in canonical RON.
  - Asset texts given to `load_scene_with` hold the asset's type name: `AssetFile(type: "my_game::WeaponStats", value: ...)` for data assets (written by the editor, Task 17); prefab files are typed by their `Prefab(..)` wrapper (Task 3).

- [ ] **Step 1: Write the failing tests** (test app with `Turret { stats: Handle<Stats> }` and `Stats` deriving `Asset`): `version_1_scene_loads_unchanged` (the `boxes` scene gives the golden hash at tick 0), `version_2_scene_round_trips_through_save`, `reachable_assets_are_loaded_and_unreachable_ignored` (two texts given, one referenced: `asset_ids()` has one), `assets_reached_through_assets_are_loaded`, `missing_asset_names_the_referencing_field`, `wrong_asset_type_is_refused`, `entity_values_name_local_identifiers`, `unknown_local_identifier_is_refused`, `duplicate_local_identifier_is_refused`, `load_addresses_map_spawned_entities`, `save_keeping_preserves_identifiers_after_despawn`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test scene_assets`. Expected: compile failure.
- [ ] **Step 3: Implement** `assets.rs`, the version 2 parser and writer, remapping, and the session call site.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): give scene entities stable identifiers and load reachable assets`.

---

### Task 3: Prefabs and scene resolution

**Files:**
- Create: `crates/ludenea-app/src/prefab.rs`, `crates/ludenea-ecs/src/prefab.rs`
- Modify: `crates/ludenea-app/src/{scene.rs, app.rs, lib.rs}`, `crates/ludenea-ecs/src/lib.rs`
- Test: `crates/ludenea-app/tests/prefab.rs`

**Interfaces:**
- Consumes: Task 2 scene version 2, `AssetSource`, `LoadAddress`.
- Produces (`ludenea_ecs::prefab`): `#[derive(Asset)] pub struct Prefab { pub entities: Vec<PrefabEntity>, pub entity_components: BTreeSet<String> }` with stable name `ludenea::Prefab`, `pub struct PrefabEntity { pub address: Vec<u32>, pub components: Vec<(String, Vec<u8>)> }`: the **flattened** prefab (nested instances expanded, overrides applied) in canonical bytes; `entity_components` names the component types whose schema contains `Entity`, so spawning remaps only those. `App::new()` registers `Prefab` as an asset type.
- Produces (`ludenea_app::prefab`):
  - Prefab file: `Prefab(version: 1, entities: [(id: u32, components: {...})], instances: [...])` (no resources). `PREFAB_VERSION = 1`.
  - Instance entry (scene and prefab): `(id: u32, prefab: AssetId, overrides: [(entity: [u32], component: String, path: [PathSegment], value: <RON>)], added: [(entity: [u32], component: String, value: <RON>)], removed: [(entity: [u32], component: String)])`. `PathSegment` is the RON form of `Field(name)` and `Index(n)`, reused from the control crate's path type moved to `ludenea-app` (re-export it from `ludenea-control::path`).
  - `pub fn resolve(file: &str, assets: &dyn AssetSource) -> Result<Resolved, SceneError>` with `pub struct Resolved { pub resources: Vec<(String, String)>, pub entities: Vec<ResolvedEntity>, pub warnings: Vec<String> }`, `pub struct ResolvedEntity { pub address: LoadAddress, pub components: Vec<(String, String)> }` (RON texts). Order: the file's own entities in file order, then each instance's entities in instance order, each in prefab order, recursively. Pure function; `load_scene_with` loads a `Resolved`.
  - Stale overrides (target entity, component, or path missing) produce one warning each, naming the instance identifier, the prefab, and the target, and are ignored at load; they stay in the file because saving never rewrites `instances` it did not change.
  - Cycles: `SceneError::PrefabCycle(Vec<String>)` naming the chain by asset identifier (and path when the source knows it).
  - The resolved scene's canonical text (`Resolved::to_scene_text(&self) -> String`, a version 2 scene without instances) is what the trace and replays hash as the scene.
  - Saving a prefab whose `Entity` value names an identifier outside the prefab fails with `SceneError::ForeignReference { entity: u32, component: String, path: String }`.
  - `Simulation::load_addresses` covers instance entities (`[instance id, entity id]`, deeper for nested instances); `Simulation::instances(&self) -> Vec<InstanceInfo>` with `pub struct InstanceInfo { pub address: LoadAddress, pub prefab: AssetId, pub entities: Vec<Entity> }`.
  - Every prefab reached by an instance is also loaded as a `Prefab` asset (flattened), so the same prefab can be spawned at runtime (Task 4).

- [ ] **Step 1: Write the failing tests**: `scene_without_instances_resolves_to_itself` (golden hash at tick 0 unchanged), `instance_expands_after_scene_entities_in_prefab_order`, `override_sets_one_field`, `added_and_removed_components_apply`, `nested_instance_override_addresses_by_path`, `entity_values_are_remapped_per_instance` (two turret instances target their own barrel), `stale_override_warns_and_is_kept_on_save` (Review Focus), `missing_prefab_is_refused_naming_the_instance` (Review Focus), `prefab_cycle_is_refused_with_the_chain`, `foreign_reference_is_refused_on_prefab_save`, `resolved_scene_text_is_stable` (resolving twice gives the same text), `instance_prefabs_are_loaded_as_assets`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test prefab`. Expected: compile failure.
- [ ] **Step 3: Implement** `prefab.rs` in both crates and wire resolution into `load_scene_with`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): resolve prefab instances with overrides into scene entities`.

---

### Task 4: Spawning prefabs at runtime

**Files:**
- Modify: `crates/ludenea-ecs/src/{commands.rs, prefab.rs, observer.rs}`
- Test: `crates/ludenea-app/tests/spawn_prefab.rs`

**Interfaces:**
- Consumes: Task 3 `Prefab`, Task 1 `Handle`.
- Produces (`ludenea_ecs`):
  - `Commands::spawn_prefab(&mut self, prefab: Handle<Prefab>, overrides: impl FnOnce(&mut PrefabOverrides) + Send + 'static)`, applied at the next sync point in issue order like every command: spawns the prefab's entities in prefab order, remaps their `Entity` fields to the new entities, then applies the overrides.
  - `pub struct PrefabOverrides` with `set<C: Component>(&mut self, entity: u32, edit: impl FnOnce(&mut C) + Send + 'static) -> &mut Self` (`entity` is the prefab-local identifier of a top-level entity) and `insert<C: Component>(&mut self, entity: u32, value: C) -> &mut Self`.
  - Each spawned entity produces a `CommandRecord { kind: Spawn, .. }` so the explorer and the causal report see it.
  - A missing prefab or an unknown local identifier panics with the prefab identifier and the identifier (a game bug, caught by the host's panic recovery).

- [ ] **Step 1: Write the failing tests**: `spawn_prefab_spawns_entities_in_prefab_order`, `spawned_entity_fields_point_to_new_entities`, `overrides_apply_after_spawn`, `spawn_prefab_is_deterministic_across_thread_counts` (1 and 8 threads, same hashes over 200 ticks), `spawn_prefab_survives_snapshot_and_restore`, `spawn_records_commands`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test spawn_prefab`. Expected: compile failure.
- [ ] **Step 3: Implement** the command and the remapping (only components listed as containing `Entity` go through the JSON codec; `// ponytail:` note on the JSON round trip per remapped component, precompiled field offsets if spawning shows in profiles).
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(ecs): spawn prefabs from game code with overrides`.

---

### Task 5: Edit log version 2: load addresses and asset edits

**Files:**
- Modify: `crates/ludenea-app/src/edit.rs`, `crates/ludenea-control/src/session.rs` (records `EntityRef`), `crates/ludenea-cli/src/commands.rs` (decodes either version)
- Test: `crates/ludenea-app/tests/edit.rs`

**Interfaces:**
- Consumes: Task 2 `LoadAddress`, `entity_at_address`; Task 1 asset store.
- Produces (`ludenea_app::edit`):
  - `pub enum EntityRef { Loaded(LoadAddress), Runtime(Entity) }`; `Simulation::entity_ref(&self, entity: Entity) -> EntityRef` (`Loaded` when the entity is in the load map with the generation it was loaded with, else `Runtime`); `Simulation::resolve_ref(&self, target: &EntityRef) -> Result<Entity, EditError>` (`EditError::NoSuchAddress(LoadAddress)` when the address no longer resolves).
  - `Edit::{Despawn, Insert, Remove}` take `entity: EntityRef`. New variants `SetAsset { id: AssetId, value: Vec<u8> }` (whole value, canonical bytes of the asset's type) and `LoadAsset { id: AssetId, asset: String, value: Vec<u8> }`.
  - `EDIT_LOG_VERSION = 2`; `EditLog::decode` accepts version 1 and maps its `Entity` targets to `EntityRef::Runtime`.
  - `Simulation::assets_introduced_by(&self, edit: &Edit, sources: &dyn AssetSource) -> Result<Vec<Edit>, EditError>`: the `LoadAsset` edits needed before `edit` (handles in its values not yet loaded, recursively); `EditError::MissingAsset(AssetId)` when a source lacks one.
- Produces (`ludenea_control::session`): `Session::edit` records, before the edit, the `LoadAsset` edits returned by `assets_introduced_by` against the session's asset texts, at the same tick.

- [ ] **Step 1: Write the failing tests**: `edit_on_loaded_entity_records_its_address`, `edit_on_runtime_entity_records_its_identifier`, `address_survives_an_entity_added_earlier_in_a_prefab` (record an edit on instance 2's entity, add an entity to the prefab, reload, replay: the edit lands on the same instance entity), `unresolved_address_is_no_such_address`, `set_asset_replaces_value_and_changes_hash`, `load_asset_precedes_edit_introducing_a_handle`, `version_1_edit_log_decodes`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test edit`. Expected: compile failure.
- [ ] **Step 3: Implement** `EntityRef`, the new variants, version 1 decoding, and the session recording.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS (the 2a session end-to-end test regenerates its logs at run time).
- [ ] **Step 5: Commit** `✨ feat(app): address recorded edits by load address and record asset edits`.

---

### Task 6: Event schemas

**Files:**
- Modify: `crates/ludenea-app/src/app.rs`, `crates/ludenea-ecs/src/{event.rs, world.rs}`, `crates/ludenea-physics/src/*` (`Collision` and other events derive `Schema`), `examples/boxes/src/lib.rs`, every test event type, the project template
- Create: `crates/ludenea-schema/src/entities.rs`
- Test: inline tests in `entities.rs`, `crates/ludenea-ecs/tests/schema.rs`

**Interfaces:**
- Produces: `App::event::<T: Schema + Serialize + Send + 'static>()` (breaking: the `Schema` bound); `World::event_schemas(&self) -> Vec<(&'static str, TypeSchema)>`; `GameInfo` (control) gains `events: Vec<NamedSchema>` in Task 14.
- Produces (`ludenea_schema::entities`): `pub fn entity_refs(schema: &TypeSchema, value: &serde_json::Value) -> Vec<EntityRefAt>` with `pub struct EntityRefAt { pub path: String, pub index: u32, pub generation: u32 }`: every `Entity` inside a JSON value with its field path (such as `hits[2]`), walked by schema. The schema crate does not depend on the ECS, hence plain numbers.

- [ ] **Step 1: Write the failing tests**: `entity_refs_finds_nested_entities_with_paths` (struct with `a: Entity`, `hits: Vec<Entity>`, an enum variant holding an entity), `entity_refs_ignores_numbers_shaped_like_entities` (a `{index, generation}` struct that is not `Entity` is not reported), `world_lists_event_schemas`.
- [ ] **Step 2: Run** `cargo test -p ludenea-schema -p ludenea-ecs`. Expected: compile failure.
- [ ] **Step 3: Implement** the bound, `event_schemas`, `entity_refs`, and the `Schema` derives on every event.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `💥 feat(app): require a schema on every registered event`.

---

### Task 7: The replay file

**Files:**
- Create: `crates/ludenea-app/src/replay.rs`
- Modify: `crates/ludenea-app/src/{simulation.rs, lib.rs}`, facade
- Test: `crates/ludenea-app/tests/replay.rs`

**Interfaces:**
- Consumes: Task 3 `resolve`, Task 5 `EditLog` version 2, milestone 1 `InputLog`.
- Produces (`ludenea_app::replay`):
  - `pub const REPLAY_VERSION: u32 = 1; pub const REPLAY_CHECKPOINT_INTERVAL: u64 = 1024;`
  - `pub struct Replay { pub version: u32, pub engine: String, pub fingerprint: u64, pub scene: String, pub assets: Vec<(AssetId, String)>, pub inputs: Vec<u8>, pub edits: Vec<u8>, pub hashes: Vec<u64>, pub checkpoints: Vec<(u64, Vec<u8>)> }`: `scene` is the resolved scene text, `assets` the loaded assets as canonical RON wrapped in `AssetFile(..)`, `checkpoints` snapshot bytes (optional, may be empty). Postcard; `encode`, `decode` (other versions: `ReplayError::Version`).
  - `Simulation::schema_fingerprint(&self) -> u64`: `hash_value` of the sorted component, resource, asset, event, and input schemas. `engine` is `env!("CARGO_PKG_VERSION")` of `ludenea-app`.
  - `pub fn record_replay(sim: &mut Simulation, scene: &str, assets: &dyn AssetSource, inputs: &InputLog, edits: &EditLog, ticks: u64) -> Result<Replay, ReplayError>` (runs from tick 0, collects hashes and checkpoints).
  - `pub fn play_replay(sim: &mut Simulation, replay: &Replay, mut on_tick: impl FnMut(u64, u64)) -> Result<ReplayOutcome, ReplayError>` with `pub enum ReplayOutcome { Matched, Diverged { tick: u64, expected: u64, got: u64 } }`; checkpoints are checked against the hashes at their tick when used.
  - `pub enum ReplayError` (`thiserror`): `Version { found, expected }`, `Decode { what: String, reason: String }` (`what` such as `input of tick 120`, `edit of tick 300`, `asset 3f2a...`, `scene`), `FingerprintMismatch { recorded, current }` (a warning value returned alongside, not an error, when everything decodes: `ReplayOutcome` gains `fingerprint_differs: bool`).

- [ ] **Step 1: Write the failing tests**: `replay_round_trips`, `recorded_replay_plays_back_matched`, `replay_is_self_contained` (play with `NoAssets` available elsewhere: works), `altered_hash_reports_first_divergence`, `changed_input_type_fails_with_decode_naming_the_tick` (Review Focus), `added_component_field_with_default_still_plays_and_flags_fingerprint`, `checkpoints_every_1024_ticks`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test replay`. Expected: compile failure.
- [ ] **Step 3: Implement** `replay.rs` and `schema_fingerprint`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): record and play self-contained replays`.

---

### Task 8: CLI: project assets, replays, structured diff output

**Files:**
- Create: `crates/ludenea-cli/src/assets.rs`
- Modify: `crates/ludenea-cli/src/{commands.rs, lib.rs}`, `crates/ludenea-ecs/src/diff.rs`
- Test: `crates/ludenea-cli/tests/cli.rs` (extend), inline tests in `assets.rs`

**Interfaces:**
- Consumes: Tasks 2, 3, 7.
- Produces (`ludenea_ecs::diff`): `Difference { subject, component, path: Vec<String>, left: Option<serde_json::Value>, right: Option<serde_json::Value> }` (JSON through the codec; values without a JSON form fall back to `Value::String` of their RON); enum variant changes report `left`/`right` as the variant names. `Display` keeps the milestone 1 line format.
- Produces (`ludenea_cli::assets`): `pub fn project_assets(scene: &Path, project: Option<&Path>) -> Result<AssetTexts, String>`: the project root is `project`, else the nearest ancestor of the scene holding `ludenea.toml`, else the scene's folder; reads every `.prefab.ron` and every `.ron` whose `.meta` says `Data`, keyed by the meta identifier. A broken meta is an error naming its path.
- Produces (CLI): `run` and `verify` gain `--project <dir>`; `run --replay <file>` writes a replay of the run; `verify <file.replay>` (detected by extension, no `--inputs` or `--golden`) plays it and exits 1 on divergence with the milestone 1 snapshot dumps; `--threads` applies to both.

- [ ] **Step 1: Write the failing tests**: `project_root_is_nearest_manifest`, `scene_folder_is_used_without_manifest`, `data_assets_are_found_by_meta`, `run_writes_a_replay_that_verifies`, `verify_replay_fails_on_altered_hash_and_dumps_snapshots`, `diff_output_lines_unchanged` (milestone 1 expected text), `structured_difference_reports_variant_names`.
- [ ] **Step 2: Run** `cargo test -p ludenea-cli -p ludenea-ecs`. Expected: compile failure.
- [ ] **Step 3: Implement** `assets.rs`, the flags, replay verification, and structured differences.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(cli): resolve project assets and verify replays`.

---

### Task 9: The `boxes` prefab scenario and its cross-platform replay

**Files:**
- Create: `examples/boxes/assets/{spawner.scene.ron, crate.prefab.ron, crate.prefab.ron.meta, spawner.ron, spawner.ron.meta, spawner.scene.ron.meta}`, `examples/boxes/src/bin/record-replay.rs`, `golden/spawner.replay`
- Modify: `examples/boxes/src/lib.rs` (component `Spawner { prefab: Handle<Prefab>, stats: Handle<SpawnerStats> }`, asset `SpawnerStats { every_ticks: u32, impulse: Vec2 }`, system spawning the prefab every `every_ticks` with an impulse drawn from `Rng`), `examples/boxes/tests/golden.rs`, `.github/workflows/determinism.yml`, `README.md`
- Test: `examples/boxes/tests/golden.rs`

**Interfaces:**
- Consumes: Tasks 1 to 8.
- Produces: `golden/spawner.replay`: 3,000 ticks of `spawner.scene.ron` (one `Spawner` entity and two prefab instances with one override), with an edit log holding one `SetAsset` on `spawner.ron` at tick 1,000 and one `Insert` on an instance entity at tick 2,000. Produced once by `record-replay` (deterministic, no `Rng` outside the simulation) and committed; tests never run the generator.

- [ ] **Step 1: Write the failing tests**: `spawner_replay_matches_at_1_2_and_8_threads`, `spawner_replay_survives_snapshot_restore_at_1500` (restore the checkpoint at 1,024, step to 1,500, snapshot, restore, play to the end: hashes match), `boxes_golden_is_unchanged` (existing test kept as is).
- [ ] **Step 2: Run** `cargo test -p boxes --test golden`. Expected: FAIL (missing files).
- [ ] **Step 3: Implement** the types, the scene, the prefab, the asset, and the generator; run it once (`cargo run --release -p boxes --bin record-replay -- golden/spawner.replay`) and commit the output.
- [ ] **Step 4: Add CI steps** verifying `golden/spawner.replay` natively at 1, 2, and 8 threads on the three operating systems and under `wasmtime` (`verify golden/spawner.replay`), next to the existing golden steps; document both commands in `README.md`.
- [ ] **Step 5: Run** `cargo test --locked --workspace` and the wasmtime verify locally. Expected: PASS.
- [ ] **Step 6: Commit** `✅ test(boxes): add a prefab and data asset scenario as a cross-platform replay`.

---

### Task 10: Timeline branches in the session

**Files:**
- Create: `crates/ludenea-control/src/branches.rs`
- Modify: `crates/ludenea-control/src/{session.rs, checkpoints.rs}`
- Test: `crates/ludenea-control/tests/branches.rs`

**Interfaces:**
- Consumes: 2a `Session`, `Checkpoints`.
- Produces (`ludenea_control::branches`):
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)] pub struct BranchId(pub u32);` (root is `BranchId(0)`, identifiers never reused in a session).
  - `pub struct BranchInfo { pub id: BranchId, pub name: String, pub parent: Option<BranchId>, pub fork_tick: u64, pub head: u64, pub stale: bool }` (Serialize, ts).
  - `pub(crate) struct BranchTree`: the stashed segments of non-current branches. A **segment** is a branch's inputs, edits, hashes, and checkpoints strictly after its fork tick (from tick 0 for the root). The current branch lives flattened in the session's existing linear fields; switching splits the flattened run back into the current branch's segment and composes the target's ancestors' segments up to each fork tick plus its own.
- Produces (`ludenea_control::session`):
  - **Fork rule** (comment it in `session.rs`): an edit past tick 0, or a take-over, at tick `N` with recorded ticks after `N` on the current branch creates a child branch forked at `N`, names it `Branch {k}` (`k` = next number), makes it current, and leaves the parent's future intact; with nothing recorded after `N`, it extends the current branch.
  - `Session::branches(&self) -> Vec<BranchInfo>` (identifier order), `current_branch(&self) -> BranchId`, `switch_branch(&mut self, sim, id: BranchId) -> Result<(), SessionError>` (keeps the tick when the target covers it, else goes to the target's head), `rename_branch(&mut self, id, name: String)`, `delete_branch(&mut self, sim, id) -> Result<Vec<BranchId>, SessionError>` (returns the deleted descendants; deleting the current branch switches to its parent at the same tick; the root is `SessionError::RootBranch`).
  - `Timeline` gains `branch: BranchId`, `branches: Vec<BranchInfo>`.
  - Checkpoint budget: `pub const CHECKPOINT_BUDGET: usize = 256;` session-wide; when exceeded, checkpoints of stashed branches other than the current branch's parent are evicted first, oldest first, never tick 0 or a fork tick.
  - `SessionExport` gains `branches: Vec<u8>` (postcard of the tree, current branch, and names; empty for a linear session); `Session::resume` takes it; a 2a export (empty) resumes as one root branch.
  - `SessionError::NoSuchBranch(BranchId)`, `RootBranch`.

- [ ] **Step 1: Write the failing tests** (with the `boxes` app): `playing_forward_never_creates_branches`, `past_edit_forks_and_keeps_parent_future` (record 400 ticks, seek 200, edit: two branches, parent head 400 with its hashes intact, child head 200), `take_over_forks`, `edit_at_head_extends_current_branch`, `switching_keeps_tick_when_covered`, `switching_goes_to_head_when_not_covered`, `sibling_switch_reproduces_each_branch_hashes` (three branches from one parent; switching in any order gives each branch's recorded hashes), `nested_branch_composes_ancestors`, `delete_removes_descendants_and_switches_to_parent`, `root_cannot_be_deleted`, `checkpoint_budget_evicts_stashed_branches_first` (20 branches of 2,000 ticks: total checkpoints at most `CHECKPOINT_BUDGET`, every branch still seeks to every tick), `export_resume_keeps_the_tree`, `export_from_2a_resumes_as_root`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test branches`. Expected: compile failure.
- [ ] **Step 3: Implement** `branches.rs`, the fork rule replacing `discard_after` on these two paths, switching, deletion, the budget, and export.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): fork the timeline into branches instead of discarding the future`.

---

### Task 11: Undo and redo across branches

**Files:**
- Modify: `crates/ludenea-control/src/{undo.rs, session.rs, branches.rs}`
- Test: `crates/ludenea-control/tests/undo.rs` (extend)

**Interfaces:**
- Consumes: Task 10.
- Produces:
  - `UndoEntry::Run { branch: BranchId, edit: TimedEdit }` and `UndoEntry::Fork { branch: BranchId, cause: ForkCause, name: String }` with `pub(crate) enum ForkCause { Edit(TimedEdit), TakeOver { tick: u64, input: Vec<u8> } }` (what created `branch`). Both entries also hold `removed: Vec<u8>`, the postcard segments of branches the undo removed, empty until undone. One history per session; `UndoEntry::Scene` is unchanged.
  - Undo first switches to the entry's branch. Undoing `Fork` deletes the branch (and its descendants) and switches to the parent at the same tick; redo recreates it with the same identifier and name, restoring the deleted descendants from the entry.
  - Undoing `Run` removes the edit, discards that branch's future after its tick, and deletes descendant branches forked after that tick; the entry keeps the removed descendants' segments so redo restores them unchanged.
  - `Timeline` gains `undo_label: Option<String>` and `redo_label: Option<String>`, such as `Undo edit at tick 300 (removes Branch 3, Branch 4)`.

- [ ] **Step 1: Write the failing tests**: `undo_fork_returns_to_parent_with_future`, `redo_fork_recreates_same_branch`, `undo_switches_to_entry_branch_first`, `undo_run_removes_later_descendants_and_redo_restores_them`, `undo_run_keeps_descendants_forked_before_its_tick`, `undo_label_names_removed_branches`, `undo_survives_export_and_resume_with_branches`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test undo`. Expected: compile failure.
- [ ] **Step 3: Implement** the entries and their undo and redo.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): undo forks and edits across timeline branches`.

---

### Task 12: Stale branches, scene and asset changes, documents

**Files:**
- Modify: `crates/ludenea-control/src/{session.rs, branches.rs}`
- Test: `crates/ludenea-control/tests/branches.rs` (extend), `crates/ludenea-control/tests/session.rs` (extend)

**Interfaces:**
- Consumes: Tasks 2, 3, 5, 10.
- Produces:
  - The session holds asset texts beside the scene: `Session::start(sim, scene: String, assets: AssetTexts)`, `Session::assets(&self) -> &AssetTexts`, `replace_scene(&mut self, sim, scene: String, assets: AssetTexts)` (scene, prefab, or data asset changed on disk), `Session::save_asset(&self, id: AssetId) -> Option<String>` (the asset's RON text in its file form, for the editor to write).
  - Asset edits: at tick 0 an asset edit changes the session's asset text and replays like a scene edit; past tick 0 it records `Edit::SetAsset` (Task 5). `Session::apply_to_asset(&mut self, sim, id: AssetId) -> Result<(), SessionError>` copies the current value into the asset text, mirroring `apply_to_scene`.
  - Document kind: `pub enum Document { Scene, Prefab }` given to `start`; with `Prefab`, `scene()` returns prefab file text (no resources) and tick 0 edits refuse resources (`SessionError::PrefabHasNoResources`).
  - Staleness: a tick 0 edit, `replace_scene`, or `resume` after a rebuild resimulates the current branch (2a behaviour) and marks every other branch `stale`. Switching to a stale branch resimulates it from its fork tick (its ancestors first when they are stale too), with the same `Resimulation` reports, `EditDropped` per branch included.

- [ ] **Step 1: Write the failing tests**: `scene_edit_marks_other_branches_stale`, `switching_to_stale_branch_resimulates_it` (Review Focus: also while another resimulation runs; seeking past the resimulated point is `Resimulating`), `stale_ancestor_resimulates_first`, `dropped_edit_is_reported_per_branch`, `asset_edit_at_tick_0_replays_session`, `asset_edit_past_tick_0_is_recorded`, `apply_to_asset_copies_value`, `prefab_document_saves_prefab_text`, `prefab_document_refuses_resources`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** asset texts, documents, and stale branches.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): resimulate stale branches and edit data assets in sessions`.

---

### Task 13: Trace comparison

**Files:**
- Modify: `crates/ludenea-cli/src/trace.rs` (move `Trace` to `crates/ludenea-app/src/trace.rs` and re-export it from the CLI), `crates/ludenea-app/src/lib.rs`
- Test: inline tests in `crates/ludenea-app/src/trace.rs`

**Interfaces:**
- Produces (`ludenea_app::trace`): `Trace` unchanged in format; `pub fn compare_traces(a: &[u64], b: &[u64]) -> TraceComparison` with `pub struct TraceComparison { pub common: u64, pub first_divergent: Option<u64>, pub only_in_longer: u64 }`; `Trace::from_replay(&Replay) -> Trace`.

- [ ] **Step 1: Write the failing tests**: `identical_traces_have_no_divergence`, `first_divergent_tick_is_reported`, `prefix_trace_reports_only_length`, `golden_trace_still_decodes`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app trace`. Expected: compile failure.
- [ ] **Step 3: Implement** the move and `compare_traces`.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `♻️ refactor(app): share traces and compare them tick by tick`.

---

### Task 14: Control protocol version 3

**Files:**
- Modify: `crates/ludenea-control/src/{protocol.rs, host.rs, lib.rs}`, `crates/ludenea-control/tests/*` (handshake version), `editor/packages/control-client/src/generated/*` (regenerated), `crates/ludenea-cli/src/commands.rs`
- Test: `crates/ludenea-control/tests/protocol_v3.rs`

**Interfaces:**
- Consumes: Tasks 1 to 13.
- Produces (`PROTOCOL_VERSION = 3`); binary payloads are base64 `Bytes`:
  - `GameInfo` gains `assets: Vec<NamedSchema>`, `events: Vec<NamedSchema>`, `fingerprint: Hash64`.
  - `LoadScene { scene: String, assets: Vec<(AssetId, String)>, document: Document }` (replaces the 2a shape); the reply `Loaded { warnings: Vec<String>, instances: Vec<InstanceInfo>, load_addresses: Vec<(Entity, Vec<u32>)> }`.
  - Assets: `GetAsset { id } -> Asset { asset: String, value: ComponentValue }`, `SetAssetField { id, path, value }`, `SaveAsset { id } -> Text { text }`, `ApplyToAsset { id }`, `NewAssetValue { asset: String } -> Text { text }` (the `Default` value in file form, or `AssetError::NoDefault`), `ReplaceScene` gains `assets`.
  - Prefabs: `CreatePrefab { entities: Vec<Entity> } -> Text { text }` (the prefab file text; `ForeignReference` refused with its message), `ApplyOverride`, `RevertOverride { instance: Vec<u32>, entity: Vec<u32>, component: String, path: Vec<PathSegment> }`, `ApplyToPrefab { instance, entity, component, path } -> Text { text }` (the updated prefab file text, written by the editor), `GetInstances -> Instances { instances: Vec<InstanceInfo> }`.
  - Branches: `SwitchBranch { branch }`, `RenameBranch { branch, name }`, `DeleteBranch { branch } -> Deleted { branches }`; the `Timeline` reply carries the branches.
  - Replays: `ExportReplay { branch: Option<BranchId> } -> Bytes`, `LoadReplay { replay: Bytes } -> ReplayLoaded { fingerprint_differs: bool }` (errors `ReplayError::Decode` with its message).
  - Divergence: `DiffSnapshots` replies `Differences { differences: Vec<Difference> }` (structured, Task 8); `CompareTraces { a: Bytes, b: Option<Bytes> }` (`None` compares with the current branch) `-> TraceComparison`.
  - `ExportSession` and `LoadSession` carry `assets` and `branches` (serde defaults, so 2a exports load).
  - The handshake refuses version 2 clients with the existing message.
- Regenerate TypeScript with `cargo test --locked -p ludenea-control --test typescript` (and `-p ludenea-project --test typescript` in Task 17) and commit the generated files.

- [ ] **Step 1: Write the failing tests**: `handshake_refuses_version_2`, `load_scene_with_assets_reports_instances_and_warnings`, `set_asset_field_records_set_asset_past_tick_0`, `new_asset_value_uses_default`, `create_prefab_returns_prefab_text`, `apply_to_prefab_returns_updated_prefab`, `switch_and_delete_branch_over_protocol`, `export_and_load_replay_round_trip`, `load_replay_with_broken_input_names_the_tick` (Review Focus), `diff_snapshots_is_structured`, `compare_traces_against_current_branch`, `session_export_from_2a_loads`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test protocol_v3`. Expected: compile failure.
- [ ] **Step 3: Implement** the commands in `host.rs`, regenerate the types, update the CLI and the 2a tests for the new `LoadScene` shape.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS (fix front end call sites of `LoadScene` minimally here; features come in later tasks).
- [ ] **Step 5: Commit** `✨ feat(control): add assets, prefabs, branches, and replays to protocol version 3`.

---

### Task 15: The compare simulation and the ghost

**Files:**
- Create: `crates/ludenea-control/src/compare.rs`
- Modify: `crates/ludenea-control/src/{host.rs, protocol.rs, session.rs, worker.rs, stdio.rs}`, `crates/ludenea-cli/src/commands.rs`, every `Host::new` caller, generated TypeScript
- Test: `crates/ludenea-control/tests/compare.rs`

**Interfaces:**
- Consumes: Task 10 branches, Task 14 protocol.
- Produces:
  - `Host::new(build: fn() -> App) -> Result<Host, AppError>` (was `App`): the host builds a second `Simulation` lazily for the compare branch.
  - `pub(crate) struct CompareRun`: owns the second simulation, the compare branch, and its position; `follow(&mut self, session: &Session, tick: u64, budget: u32) -> CompareState` runs at most `budget` ticks toward `tick` using the compare branch's flattened run and checkpoints. `pub const COMPARE_BUDGET: u32 = 4;` ticks per `advance` while playing, `240` while paused.
  - `Command::SetCompareBranch { branch: Option<BranchId> }`; `Timeline` gains `compare: Option<BranchId>`; forking sets the compare branch to the parent.
  - `Command::GetGhostDraw { tick, layers } -> GhostDraw { frame: DebugDrawFrame, tick: u64, lagging: bool }`: the compare branch's debug draw at `tick`, or the latest tick it reached with `lagging: true`. A tick outside the compare branch's range returns an empty frame.
  - `Event::GhostAdvanced { tick }` when the compare run reaches a new tick while subscribed (`Topic::Ghost`).

- [ ] **Step 1: Write the failing tests**: `ghost_draws_compare_branch_at_current_tick` (fork at 200 by moving a crate: the ghost frame at 250 differs from the main frame for that crate only), `ghost_reports_lagging_then_catches_up`, `compare_defaults_to_parent_on_fork`, `ghost_never_changes_main_hashes` (main hashes with and without a compare branch are equal over 500 ticks), `ghost_outside_range_is_empty`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test compare`. Expected: compile failure.
- [ ] **Step 3: Implement** `compare.rs`, the builder signature change, and the commands.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): run the compare branch beside the timeline and draw its ghost`.

---

### Task 16: The causal divergence report

**Files:**
- Create: `crates/ludenea-control/src/divergence.rs`
- Modify: `crates/ludenea-control/src/{host.rs, protocol.rs, compare.rs, subscriptions.rs}`, generated TypeScript
- Test: `crates/ludenea-control/tests/divergence.rs`

**Interfaces:**
- Consumes: Task 15 `CompareRun`, Task 6 `entity_refs`, Task 8 structured `Difference`, 2a introspection recording.
- Produces (`ludenea_control::divergence`), all Serialize and ts:
  - `pub const DIVERGENCE_TICKS: u64 = 600;`
  - `pub struct DivergenceReport { pub fork_tick: u64, pub up_to: u64, pub complete: bool, pub first_divergent: Option<u64>, pub root_cause: Option<RootCause>, pub entities: Vec<EntityDivergence> }` (entities ordered by first divergent tick, then entity).
  - `pub struct EntityDivergence { pub entity: Entity, pub kind: DivergenceKind, pub first_tick: u64, pub first_difference: Option<Difference>, pub causes: Vec<Cause> }`; `pub enum DivergenceKind { Changed, OnlyInCurrent, OnlyInCompare, Replaced }` (`Replaced` when the component sets differ).
  - `pub enum Cause { Edit { tick: u64, description: String }, Event { tick: u64, event: String, path: String, other: Entity }, Command { tick: u64, system: String, kind: String } }`: events at the entity's first divergent tick or the tick before that reference the entity and an entity already divergent at that point (through `entity_refs`); commands targeting the entity at that tick; the forking edit or take-over for the earliest entities (`RootCause`).
  - `Command::GetDivergence { up_to: Option<u64> } -> Divergence(DivergenceReport)`: computed incrementally in the background inside `advance` (both simulations step from the fork with introspection recording), `complete: false` with `up_to` progress until done; `Event::DivergenceProgress { up_to }` while subscribed (`Topic::Divergence`). Default range `fork_tick + DIVERGENCE_TICKS`, extendable by passing `up_to`. Nothing runs while no client asked or subscribed.

- [ ] **Step 1: Write the failing tests** (with `boxes`: fork at tick 200 by setting one crate's velocity): `edited_entity_diverges_first_with_the_edit_as_root_cause`, `collided_entity_lists_the_collision_event_as_cause` (a crate the edited one later hits has a `Cause::Event` naming `ludenea::Collision` and the edited entity), `prefab_spawn_shows_as_command_cause`, `entity_only_in_one_branch_is_reported`, `different_component_set_is_replaced`, `report_is_incremental_and_completes`, `no_work_without_request` (no compare stepping after 1,000 played ticks when nothing asked).
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test divergence`. Expected: compile failure.
- [ ] **Step 3: Implement** `divergence.rs` (per tick: compare entity hash trees of both simulations; for each newly divergent entity, diff its components restricted to that entity, then scan the recorded events and commands for causes) and the command.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings`. Also run `cargo test --release -p ludenea-control --test divergence -- --ignored report_over_600_ticks_under_one_second` (an `#[ignore]` timing test: under 1 s on native). Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): explain divergence between branches entity by entity`.

---

### Task 17: Project service version 2

**Files:**
- Modify: `crates/ludenea-project/src/{meta.rs, files.rs, protocol.rs, service.rs, types.rs, template.rs}`, `crates/ludenea-project/template/{scenes/main.scene.ron, src/lib.rs}`, create `crates/ludenea-project/template/assets/{ball.prefab.ron, ball.prefab.ron.meta, settings.ron, settings.ron.meta}`, generated project TypeScript
- Test: `crates/ludenea-project/tests/assets.rs`

**Interfaces:**
- Produces:
  - `AssetKind::{Scene, Prefab, Data { r#type: String }, Replay, Trace, Snapshot, Other}`; `AssetKind::of` by extension (Global Constraints); `Data` only from the meta. Metas written by 2a (`Scene`, `Other`) still read.
  - Project protocol version 2 requests: `ReadBinary { path } -> Bytes`, `WriteBinary { path, bytes }`, `CreateAsset { path, kind: AssetKind, text: String } -> FileEntry` (writes the file and its meta atomically), `AssetTexts -> AssetTexts { assets: Vec<(AssetId, String)> }` (every prefab and data asset, data assets wrapped as `AssetFile(type: .., value: ..)` from their meta), `StoreSession { document: AssetId, bytes }` and `LoadStoredSession { document } -> Option<Bytes>` (files under `.ludenea/sessions/<id>.session`).
  - A change on disk to a prefab or data asset emits the existing file change event with its kind, so the editor sends `ReplaceScene` with fresh asset texts. A broken data asset is reported by the host's `ReplaceScene` error (path, line, column), and the session keeps the previous value (Review Focus; host side covered here with an in-process host).
  - Generated `AssetTypes` TypeScript for the game's asset types, beside the component types (`WriteGameTypes`).
  - Template: one `ball.prefab.ron` (a dynamic circle body), placed twice in `main.scene.ron` with one override, and `settings.ron` (`Data { type: "<template crate>::Settings" }`, a gravity scale read by one system through a handle on a component).

- [ ] **Step 1: Write the failing tests**: `kinds_follow_extensions_and_data_follows_meta`, `metas_from_2a_still_read`, `create_asset_writes_file_and_meta`, `asset_texts_wrap_data_assets_with_their_type`, `stored_session_round_trips`, `broken_data_asset_is_refused_and_previous_value_kept` (Review Focus), `template_project_loads_with_prefab_and_data_asset` (`#[ignore]`, real build).
- [ ] **Step 2: Run** `cargo test -p ludenea-project --test assets`. Expected: compile failure.
- [ ] **Step 3: Implement** the kinds, requests, template files, and type generation.
- [ ] **Step 4: Run** `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(project): know prefabs, data assets, replays, traces, and stored sessions`.

---

### Task 18: Editor shell: documents, branches, assets, inspector decorations

**Files:**
- Create: `editor/packages/editor-shell/src/stores/{document.ts, branches.ts}`, `editor/packages/editor-shell/src/assets.ts`
- Modify: `editor/packages/editor-shell/src/{editor.ts, app.ts, index.ts, plugin.ts}`, `editor/packages/control-client/src/*` (typed wrappers for the new commands), `editor/packages/editor-builtins/src/{project, scene-tree, inspector}/*` (open documents through the store), the browser crash recovery in `editor/apps/web/src/*` (exports branches and assets)
- Test: `editor/packages/editor-shell/src/stores/{document.test.ts, branches.test.ts}`

**Interfaces:**
- Consumes: Tasks 14, 15, 17.
- Produces (`Editor` interface additions):
  - `document: DocumentStore`: `{ current: { id: AssetId; kind: "scene" | "prefab" | "replay"; path: string } | null; open(id: AssetId): Promise<void> }`. `open` stores the current session (`ExportSession` then `StoreSession`), loads the target (`AssetTexts`, then `LoadScene` or `LoadReplay`, then `LoadStoredSession` and `LoadSession` when one exists). One document at a time.
  - `branches: BranchStore` (Zustand): `{ list: BranchInfo[]; current: BranchId; compare: BranchId | null; switch(id); rename(id, name); remove(id); setCompare(id | null) }`, kept from `TimelineChanged`.
  - `assets: { get<N extends keyof AssetTypes>(id: AssetId): Promise<AssetTypes[N]>; setField(id: AssetId, path: PathSegment[], value: unknown): Promise<void>; create(type: keyof AssetTypes, path: string): Promise<FileEntry> }` (`create` uses `NewAssetValue` then `CreateAsset`).
  - `inspectors.decorate(decoration: FieldDecoration): Disposable` with `FieldDecoration = { id: string; render(props: { editor: Editor; entity: Entity; component: string; path: PathSegment[] }): ReactNode }`: one registration path for marks and actions beside inspector fields; the built-in inspector renders every registered decoration.

- [ ] **Step 1: Write the failing tests**: `opening_a_document_stores_the_previous_session`, `reopening_restores_branches_and_tick`, `branch_store_follows_timeline_events`, `asset_create_writes_default_value`, `decorations_render_beside_fields`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the stores, the editor additions, and the call site changes.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): open one document at a time and expose branches and assets to plugins`.

---

### Task 19: The prefabs plugin

**Files:**
- Create: `editor/packages/editor-builtins/src/prefabs/{index.tsx, InstanceGroups.tsx, OverrideMark.tsx}`
- Modify: `editor/packages/editor-builtins/src/{index.ts, scene-tree/SceneTree.tsx, inspector/fields/*}` (a `Handle` field editor: asset picker filtered by asset type)
- Test: `editor/packages/editor-builtins/src/prefabs/prefabs.test.tsx`

**Interfaces:**
- Consumes: Task 18 `document`, `inspectors.decorate`, Task 14 prefab commands.
- Produces: `prefabsPlugin` (registered in `index.ts`):
  - Scene tree: instances as collapsible groups named after their prefab file, from `Loaded.instances` and `GetInstances`; a scene load error (missing prefab) shows on the tree with the message (Review Focus).
  - Inspector decoration: overridden fields marked; actions "Revert to prefab" and "Apply to prefab" per field and per component (`RevertOverride`, `ApplyToPrefab` then `WriteText` of the returned prefab text).
  - Commands: `prefabs.create-from-selection` ("Create prefab from selection": `CreatePrefab`, `CreateAsset` of kind `Prefab` under `assets/`, then a tick 0 edit replacing the selection with an instance), `prefabs.open` (opens the selected instance's prefab as the document).
  - Saving a prefab document reloads the open scene's session on return (Task 18 `open` path) and shows the first divergent tick.

- [ ] **Step 1: Write the failing tests**: `instances_render_as_groups`, `override_mark_shows_on_overridden_field_only`, `revert_sends_revert_override`, `apply_writes_returned_prefab_text`, `create_from_selection_creates_prefab_asset`, `missing_prefab_error_shows_on_tree`, `handle_field_lists_assets_of_its_type`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the plugin and the handle field.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): place, override, and create prefabs`.

---

### Task 20: Data assets, replays, traces, and snapshots in the asset browser

**Files:**
- Modify: `editor/packages/editor-builtins/src/assets/*`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx` (inspects a selected data asset)
- Test: `editor/packages/editor-builtins/src/assets/assets.test.tsx` (extend)

**Interfaces:**
- Consumes: Task 18 `assets`, `document`.
- Produces:
  - Asset browser: icons and filters per kind; "New data asset" (picks a type from `GameInfo.assets`, then `editor.assets.create`); opening a scene, prefab, or replay goes through `editor.document.open`; opening a trace or snapshot opens the divergence viewer (Task 22) through the command `divergence.open-file` with the file's path.
  - Selection gains `{ kind: "asset", id }`; the inspector edits a selected data asset through its schema (`GetAsset`, `SetAssetField`), with "Apply to asset" past tick 0 and the unsaved marker plus explicit save (`SaveAsset` then `WriteText`) at tick 0.

- [ ] **Step 1: Write the failing tests**: `new_data_asset_lists_game_asset_types`, `opening_replay_uses_document_store`, `opening_trace_runs_divergence_command`, `inspector_edits_selected_data_asset`, `apply_to_asset_shown_past_tick_0`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the browser and inspector changes.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): create and edit data assets and open replays from the asset browser`.

---

### Task 21: Branches in the timeline

**Files:**
- Create: `editor/packages/editor-builtins/src/timeline/BranchLanes.tsx`
- Modify: `editor/packages/editor-builtins/src/timeline/{TimelinePanel.tsx, index.tsx, editing.tsx}`
- Test: `editor/packages/editor-builtins/src/timeline/branches.test.tsx`

**Interfaces:**
- Consumes: Task 18 `branches`, Task 14 `ExportReplay`.
- Produces:
  - Branch lanes: one lane per branch with fork points and edit markers, the current lane highlighted, the compare lane marked; clicking a lane switches; stale branches marked.
  - Commands (palette, shortcuts optional): `timeline.switch-branch` (picker), `timeline.rename-branch`, `timeline.delete-branch` (confirmation naming the descendants), `timeline.set-compare-branch`, `timeline.save-replay` (`ExportReplay`, then `WriteBinary` and `CreateAsset` of kind `Replay` under `assets/replays/`).
  - Undo and redo labels come from `Timeline.undo_label` and `redo_label` in the palette.

- [ ] **Step 1: Write the failing tests**: `lanes_render_one_per_branch_with_fork_points`, `clicking_lane_switches_branch`, `delete_confirmation_names_descendants`, `save_replay_writes_replay_asset`, `undo_label_names_removed_branches`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the lanes and commands.
- [ ] **Step 4: Run** `pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): show, switch, and save timeline branches`.

---

### Task 22: The divergence plugin and the ghost overlay

**Files:**
- Create: `editor/packages/editor-builtins/src/divergence/{index.tsx, TraceViewer.tsx, SnapshotDiff.tsx, CausalReport.tsx}`
- Modify: `editor/packages/editor-builtins/src/{index.ts, viewport/*, explorer/*}`, `crates/ludenea-debug-draw/src/*` (ghost style: outline only, reduced opacity, same primitives)
- Test: `editor/packages/editor-builtins/src/divergence/divergence.test.tsx`

**Interfaces:**
- Consumes: Tasks 14, 15, 16, 18, 20.
- Produces: `divergencePlugin`:
  - Trace viewer: a strip of ticks with the first divergent tick (`CompareTraces`), for trace against trace or against the current branch; selecting the tick seeks there when the current session is one side and opens the snapshot comparison against the current tick.
  - Snapshot diff: a tree entity, component, field with left and right values formatted through the schemas (`DiffSnapshots` structured); variant changes show variant names; a snapshot that does not decode with the running game's types shows the host's refusal message.
  - Causal report panel: from the root cause outward, ordered by tick (`GetDivergence`, progress from `DivergenceProgress`, "Extend" button passing a larger `up_to`); selecting an entry seeks to its tick and selects the entity. Computed only while the panel is open.
  - ECS explorer column "Diverged at": the explorer has no column extension point, so this task adds one to the public API, `editor.explorer.columns.register({ id, title, width, value(entity: Entity): string | null }): Disposable` (in `editor-shell`, rendered by the explorer), and the divergence plugin registers the column through it; the cell is empty while no report exists.
  - Viewport overlay `divergence.ghost` (registered with `editor.overlays.register`): draws `GetGhostDraw` frames in the ghost style while a compare branch is set; a toggle command `divergence.toggle-ghost`; a "lagging" badge.
  - Command `divergence.open-file { path }` used by the asset browser.

- [ ] **Step 1: Write the failing tests**: `trace_strip_marks_first_divergent_tick`, `snapshot_diff_groups_by_entity_component_field`, `causal_report_orders_from_root_cause`, `selecting_cause_seeks_and_selects`, `explorer_column_shows_divergence_tick`, `ghost_overlay_draws_only_with_compare_branch`, `lagging_badge_shows_when_ghost_lags`.
- [ ] **Step 2: Run** `pnpm -C editor test`. Expected: FAIL.
- [ ] **Step 3: Implement** the plugin, the overlay, and the ghost style in the renderer.
- [ ] **Step 4: Run** `pnpm -C editor build:wasm && pnpm -C editor test && pnpm -C editor typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(editor): view trace divergence, explain it causally, and draw the ghost branch`.

---

### Task 23: End-to-end proofs, continuous integration, README

**Files:**
- Create: `editor/e2e/{prefabs.spec.ts, branches.spec.ts, divergence.spec.ts, replay.spec.ts}`, `editor/e2e/fixtures/divergence/{a.trace, b.trace, a.snap, b.snap}` (produced by a small test helper from the `boxes` app with one altered velocity, committed)
- Modify: `editor/e2e/golden.spec.ts` (also verifies `golden/spawner.replay` in the browser worker), `editor/e2e/performance.spec.ts` (ghost on), `.github/workflows/determinism.yml`, `README.md`
- Test: the files above

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Write the end-to-end tests** on the template project in the browser editor:
  - `prefabs.spec.ts`: `place_override_and_edit_prefab` (place a `ball` instance, override its radius, open the prefab, change its mass, return: the override is kept, the session replayed, the first divergent tick shown).
  - `branches.spec.ts`: `fork_switch_and_keep_parent_future` (play 300 ticks, seek 100, edit a ball: two lanes; switching back and forth shows both futures; undo removes the branch, redo restores it).
  - `divergence.spec.ts`: `ghost_and_causal_report` (after the fork, the ghost is visible; the report lists the edited ball at the fork tick and the other ball it later hits, with `ludenea::Collision` as cause), `trace_and_snapshot_viewer` (open the fixtures: first divergent tick, entity, component, and field as expected).
  - `replay.spec.ts`: `saved_replay_verifies_in_cli` (save the branch as a replay, run the CLI `verify` on it: exit 0), `replay_from_changed_types_is_explained` (a fixture replay with a broken input shows the decode message).
  - `golden.spec.ts`: `spawner_replay_matches_in_browser`.
  - `performance.spec.ts`: the 2a benchmark with a branch and the ghost on keeps p95 frame time at or under 17 ms (under `PERF_STRICT=1`); switching branches on a 10,000-tick session under 200 ms native (`#[ignore]` Rust timing test `switch_branch_under_200_ms` in `crates/ludenea-control/tests/branches.rs`).
- [ ] **Step 2: Run** `pnpm -C editor e2e`. Expected: PASS.
- [ ] **Step 3: Update CI**: the editor job runs the new specs; the Rust job runs the `#[ignore]` timing tests in release on macOS only (non-blocking on shared runners: `continue-on-error: true`, results printed).
- [ ] **Step 4: Update** `README.md`: prefabs, data assets, replays (`verify golden/spawner.replay`), branches, and the divergence tools, one short section each.
- [ ] **Step 5: Run** the full local check: `cargo test --locked --workspace && cargo clippy --workspace --all-targets -- -D warnings && pnpm -C editor test && pnpm -C editor typecheck && pnpm -C editor e2e`. Expected: PASS.
- [ ] **Step 6: Commit** `✅ test(e2e): prove prefabs, branches, divergence, and replays end to end`.
