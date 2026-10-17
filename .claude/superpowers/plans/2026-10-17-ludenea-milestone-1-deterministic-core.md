# Ludenea Milestone 1: Deterministic Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A headless Rust engine core (deterministic math, ECS, 2D physics, app loop, snapshot, hash tree, control protocol, CLI) whose reference scenario produces the same golden hash trace on macOS, Linux, Windows, and WebAssembly, with 1, 2, or 8 threads, in debug and release builds.

**Architecture:** Cargo workspace of small crates layered math, then ECS, then app, then physics, then control, then CLI, plus a facade crate. The simulation only touches `f32` through `+ - * / sqrt` and `ludenea-math` replacements, iterates entities in ascending index order, and splits parallel work by data (graph colors, fixed chunks), never by thread count. The CLI talks to the engine through the control protocol over an in-process transport.

**Tech Stack:** Rust 1.97.1 (edition 2024), serde, postcard (snapshot bytes), ron (scenes), xxhash-rust xxh3 (hashes), rayon (native threads only), thiserror, clap, base64, syn, quote, proc-macro-crate, wasmtime (local WebAssembly runs), GitHub Actions.

**Spec:** `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md` (milestone 1 = sections 5 to 12, 16).

## Global Constraints

- The repository is already a git repository with a `.gitignore` (`.superpowers/`, `/target/`, `node_modules/`, `dist/`). Task 1 adds `dump/`.
- Toolchain pinned in `rust-toolchain.toml`: channel `1.97.1`, components `clippy`, `rustfmt`, targets `wasm32-wasip1`.
- Edition 2024, `resolver = "3"`. Add dependencies with `cargo add` (latest compatible versions) through `[workspace.dependencies]`. **`Cargo.lock` is committed**, and every CI and golden command uses `--locked`, so a dependency update can never silently change encodings.
- Profiles: `[profile.release] overflow-checks = true` and `[profile.dev] opt-level = 1`, so debug and release agree on integer overflow and the debug golden tests stay fast. Integer arithmetic that is meant to wrap (RNG, hashing) uses explicit `wrapping_*` methods.
- Floating point in simulation code: `f32` only through `+ - * / sqrt abs floor ceil round trunc`, `as` casts, and `ludenea_math` functions. Forbidden through `clippy.toml` `disallowed-methods`, for both `f32` and `f64`: `sin, cos, tan, sin_cos, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, acosh, atanh, exp, exp2, exp_m1, ln, ln_1p, log, log2, log10, powf, powi, hypot, cbrt, mul_add, min, max, clamp`; plus `std::time::Instant::now`, `std::time::SystemTime::now`, `std::thread::available_parallelism`, `rayon::current_num_threads`. `min`, `max`, `clamp` are forbidden because their handling of `-0.0` versus `+0.0` is not specified across targets; use `ludenea_math::{min, max, clamp}`.
- Forbidden through `disallowed-types`: `std::collections::HashMap`, `std::collections::HashSet`, `std::hash::RandomState`, `std::hash::DefaultHasher`. Use `BTreeMap`, `BTreeSet`, or `Vec`.
- State hashing goes only through `ludenea_ecs::hash_value` (serde based, `usize` encoded as `u64`); never `std::hash::Hash`, whose `write_usize` width differs between wasm32 and 64-bit targets.
- `sort_unstable*` only with a total-order key that has no equal elements (for example `(key, entity)`); float keys compare with `f32::total_cmp`.
- Maps keyed by `TypeId` are lookup only and never iterated; every iteration over registered types goes through the registry sorted by stable name.
- Tests may use `f64` std transcendentals as a reference oracle with `#[allow(clippy::disallowed_methods)]` on the test function. Non-simulation code (benchmark) may use `Instant` the same way.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass at the end of every task.
- No `-C target-cpu`, no fast-math, no `RUSTFLAGS` changes.
- Stable names: engine components and resources use explicit names `ludenea::<Type>` (for example `ludenea::Pose`); game types default to `module_path!()::Type`. `TypeId` is never persisted. Registering two types under one name is an error.
- Every `thiserror` variant carries an `#[error("...")]` message that says what went wrong and, where possible, what to do.
- `Simulation` and `Host` must be `Send` (the desktop editor of milestone 2 holds them in a `Mutex`): storages, systems, commands, and decoders are boxed with `+ Send`. `World` is not `Sync`; closures passed to `Parallel` capture only extracted arrays, never `World`, `Query`, or a `Ref` guard.
- Edition 2024 warns on `unsafe_op_in_unsafe_fn`: write explicit `unsafe {}` blocks, each with a `// SAFETY:` comment.
- Code, comments, file content in English. Commits: gitmoji plus Conventional Commits, `<emoji> <type>(<scope>): <summary>`, no internal plan references, no AI attribution trailers.
- Physics constants: `pub const LINEAR_SLOP: f32 = 0.005;` (in `ludenea_physics::shape`, not configurable), speculative distance `4 * LINEAR_SLOP`. `PhysicsSettings` defaults (Box2D v3): gravity `(0, -10)`, 4 substeps, contact hertz `30`, contact damping ratio `10`, push max velocity `3` m/s, restitution threshold `1` m/s. Friction mixing `sqrt(a * b)`, restitution mixing `max(a, b)`. Tick rate 60 Hz (`dt = 1/60`).

## Deferred From The Spec

- Component field schemas for the editor inspector (spec 7.2) and TypeScript generation with `ts-rs` (spec 11.1) move to milestone 2, their first consumer. Field-level diffs use RON values instead.
- `derive(Input)` (spec 6.3) is replaced by a blanket `Input` trait (Task 7).
- Pending events and commands in snapshots (spec 7.1, 9.1): events and commands never outlive a tick (Task 6), so nothing is pending between ticks. Change detection (`Changed<T>`) is deferred until a feature needs it.
- Access conflicts between different systems (spec 7.1): systems run sequentially in this milestone, so only conflicts inside one system exist. System-level parallelism and its conflict checks come later.
- The protocol has no "write trace" command (spec 11.1): `Step` returns per-tick hashes and the CLI writes the trace on the client side.

## Review Focus

- **A despawned entity still referenced** by the contact cache, an event, or a stored `Entity` must never panic: the pair produces an `End` collision event, `Query::get` returns `Err`. Test in Task 13.
- **A scene file with an unknown component name or a malformed value** must fail with an error naming the entity index and the component, and leave the simulation unchanged. Test in Task 8.
- **An input log shorter than the requested tick count** uses default inputs for the missing ticks; an input log encoded for another input type fails with a decode error, not a panic (detection relies on postcard rejecting the bytes or leaving bytes unread, so it is best effort). Tests in Tasks 8 and 14.
- **A snapshot from another app or another snapshot version** fails with `SnapshotError` in both directions (extra or missing names) and leaves the world unchanged. Test in Task 4.
- **Degenerate geometry** (fewer than 3 distinct points, collinear points, non-positive radius) fails at construction or scene load with `ShapeError`; a dynamic body with zero density gets mass 1 and locked rotation instead of dividing by zero. Tests in Tasks 9 and 11.

---

### Task 1: Workspace, determinism lints, scalar math

**Files:**
- Create: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml`, `.gitattributes`, `crates/ludenea-math/Cargo.toml`, `crates/ludenea-math/src/lib.rs`, `crates/ludenea-math/src/scalar.rs`
- Modify: `.gitignore` (add `dump/`)
- Test: `crates/ludenea-math/src/scalar.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces (`ludenea_math`): `pub const PI: f32`, `pub fn min(a: f32, b: f32) -> f32`, `pub fn max(a: f32, b: f32) -> f32`, `pub fn clamp(x: f32, low: f32, high: f32) -> f32`, `pub fn unwind_angle(angle: f32) -> f32` (to `[-PI, PI]`, any number of turns), `pub fn sin_cos(angle: f32) -> (f32, f32)` (returns `(sin, cos)`, normalized so `sin² + cos² ≈ 1`), `pub fn sin(angle: f32) -> f32`, `pub fn cos(angle: f32) -> f32`, `pub fn atan2(y: f32, x: f32) -> f32` (`atan2(0, 0) == 0`).

- [ ] **Step 1: Create the workspace.** Root `Cargo.toml` with `members = ["crates/*", "examples/*"]` (cargo accepts a glob that matches nothing; do not create an empty `examples/`), `resolver = "3"`, `[workspace.package] edition = "2024"`, profiles per Global Constraints. `rust-toolchain.toml` and `clippy.toml` per Global Constraints (each `disallowed-methods` entry carries a `reason` pointing to `ludenea_math`). `.gitattributes`: `*.ron text eol=lf`, `*.inputs binary`, `*.trace binary`, `*.snap binary` (Windows checkouts would otherwise turn scene files into CRLF and change their hash).

- [ ] **Step 2: Check that the lint really fires on primitive methods.** Add a throwaway `let _ = 1.0f32.sin();` in `lib.rs`, run `cargo clippy -p ludenea-math -- -D warnings`. Expected: a `disallowed_methods` error. Remove the line. If it does not fire, fix the `clippy.toml` paths before continuing.

- [ ] **Step 3: Write the failing tests** in `scalar.rs`:

```rust
#[test] fn min_max_are_order_independent_for_signed_zero() {
    assert_eq!(min(-0.0, 0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(min(0.0, -0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(max(-0.0, 0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(max(0.0, -0.0).to_bits(), 0.0f32.to_bits());
}
#[test] fn min_max_return_canonical_nan() {
    let negative_nan = f32::from_bits(0xffc0_0000);
    assert_eq!(min(negative_nan, 1.0).to_bits(), 0x7fc0_0000);
    assert_eq!(max(1.0, f32::NAN).to_bits(), 0x7fc0_0000);
}
#[test] #[allow(clippy::disallowed_methods)]
fn sin_cos_error_is_below_two_thousandths() { /* sweep 100_000 angles in [-10, 10], compare to f64 sin/cos, max abs error <= 0.002 */ }
#[test] #[allow(clippy::disallowed_methods)]
fn atan2_error_is_below_one_ten_thousandth_radian() { /* 360 * 50 directions at magnitudes 0.01, 1, 1000, max abs error <= 1e-4 */ }
#[test] fn atan2_of_origin_is_zero() { assert_eq!(atan2(0.0, 0.0), 0.0); }
#[test] fn known_bit_patterns_are_stable() {
    assert_eq!(sin_cos(1.0), (RECORDED_SIN_1, RECORDED_COS_1));
    assert_eq!(atan2(1.0, 2.0), RECORDED_ATAN2_1_2);
}
```

`RECORDED_*` are frozen once (Step 5) as `f32::from_bits(0x...)`: any change to the approximation changes every golden trace.

- [ ] **Step 4: Run** `cargo test -p ludenea-math`. Expected: compile failure (functions missing).

- [ ] **Step 5: Implement `scalar.rs`.**
  - `min(a, b)`: canonical NaN `f32::from_bits(0x7fc0_0000)` if either is NaN; else `if a < b { a } else if b < a { b } else if a.is_sign_negative() { a } else { b }`. `max` mirrors it; `clamp(x, l, h) = max(l, min(x, h))`.
  - `unwind_angle(x)`: `if x < -PI { x - 2π * trunc((x - PI) / 2π) } else if x > PI { x - 2π * trunc((x + PI) / 2π) } else { x }` (Box2D v3.1).
  - `sin_cos` (Box2D v3 `b2ComputeCosSin`), with `x = unwind_angle(angle)`, `π2 = π * π`:
    - cosine: `x < -π/2`: `y = x + π`, `c = -(π2 - 4y²)/(π2 + y²)`; `x > π/2`: `y = x - π`, same formula; otherwise `c = (π2 - 4x²)/(π2 + x²)`;
    - sine: `x < 0`: `y = x + π`, `s = -16y(π - y)/(5π2 - 4y(π - y))`; otherwise `s = 16x(π - x)/(5π2 - 4x(π - x))`;
    - divide both by `sqrt(s² + c²)` (zero magnitude gives `(0, 0)`).
  - `atan2` (Box2D v3 `b2Atan2`): `a = min(|y|, |x|) / (max(|y|, |x|) + f32::MIN_POSITIVE)`, `s = a²`, `c = s*a`, `q = s²`, `r = ((0.024840285*q + 0.18681418)*s + (-0.094097948*q - 0.33213072))*c + a`; then `if |y| > |x| { r = 1.57079637 - r }`, `if x < 0 { r = 3.14159274 - r }`, `if y < 0 { r = -r }`.
  - Record the frozen constants: a throwaway test printing `println!("{:#010x}", v.to_bits())`, run with `-- --nocapture`, paste, delete the throwaway test.

- [ ] **Step 6: Run** `cargo test -p ludenea-math && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS, no warnings.

- [ ] **Step 7: Commit** (including `Cargo.lock`) `🎉 chore(workspace): bootstrap workspace with deterministic scalar math`.

---

### Task 2: Geometry types

**Files:**
- Create: `crates/ludenea-math/src/{vec2.rs, rot.rs, transform.rs, aabb.rs}`
- Modify: `crates/ludenea-math/src/lib.rs` (re-exports), `crates/ludenea-math/Cargo.toml` (`serde` with `derive`)
- Test: inline test modules in each file

**Interfaces:**
- Consumes: Task 1 scalar functions.
- Produces, all `#[derive(Copy, Clone, Debug, PartialEq, Default, Serialize, Deserialize)]`:
  - `Vec2 { pub x: f32, pub y: f32 }`: `ZERO`, `new`, `dot`, `cross(self, other) -> f32`, `cross_vs(v, s) -> Vec2` = `(s*v.y, -s*v.x)`, `cross_sv(s, v) -> Vec2` = `(-s*v.y, s*v.x)`, `left_perp` = `(-y, x)`, `right_perp` = `(y, -x)`, `length`, `length_squared`, `normalize(self) -> Vec2` (returns `ZERO` when length `< f32::EPSILON`), `distance`, `lerp(a, b, t)`, componentwise `min`, `max`, `abs`; operators `Add`, `Sub`, `Neg`, `Mul<f32>`, `f32 * Vec2`, `AddAssign`, `SubAssign`, `MulAssign<f32>`.
  - `Rot { pub cos: f32, pub sin: f32 }`: `IDENTITY`, `from_angle(angle)`, `angle(self) -> f32` (via `atan2`), `rotate(v)`, `inv_rotate(v)`, `mul(self, other)`, `inv_mul(self, other)`, `integrate(self, delta_angle: f32) -> Rot` (Box2D v3: `(c - d*s, s + d*c)` renormalized with `sqrt`), `normalize`.
  - `Transform { pub position: Vec2, pub rotation: Rot }`: `IDENTITY`, `apply(v)`, `inv_apply(v)`, `mul(self, other)`, `inv_mul(self, other)`.
  - `Aabb { pub min: Vec2, pub max: Vec2 }`: `union`, `overlaps`, `contains`, `perimeter`, `fattened(margin: f32)`.

- [ ] **Step 1: Write the failing tests**: `rotate_then_inv_rotate_is_identity_within_1e6`, `integrate_keeps_unit_length_within_1e6` (1000 integrations of `0.01`), `transform_mul_inv_mul_round_trip`, `aabb_overlap_touching_edges_counts_as_overlap` (`min.x == other.max.x` overlaps), `normalize_of_zero_is_zero`, `cross_helpers_match_definitions` (`cross_vs(Vec2::new(1.0, 2.0), 3.0) == Vec2::new(6.0, -3.0)`).
- [ ] **Step 2: Run** `cargo test -p ludenea-math`. Expected: FAIL (types missing).
- [ ] **Step 3: Implement** the four files; no `std` transcendentals, `min`/`max` from `scalar`.
- [ ] **Step 4: Run** `cargo test -p ludenea-math && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(math): add vectors, rotations, transforms, and bounding boxes`.

---

### Task 3: ECS world, components, resources, derive macros, facade

**Files:**
- Create: `crates/ludenea-ecs/{Cargo.toml, src/lib.rs, src/entity.rs, src/component.rs, src/world.rs, src/error.rs}`, `crates/ludenea-macros/{Cargo.toml, src/lib.rs}`, `crates/ludenea/{Cargo.toml, src/lib.rs}`
- Test: `crates/ludenea-ecs/tests/world.rs`

**Interfaces:**
- Produces (`ludenea_ecs`):
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] pub struct Entity { index: u32, generation: u32 }` with `index()`, `generation()`, `Display` as `"{index}v{generation}"`. `Ord` by `(index, generation)`.
  - `pub trait Component: Serialize + DeserializeOwned + Send + 'static { const NAME: &'static str; }`
  - `pub trait Resource: Serialize + DeserializeOwned + Send + 'static { const NAME: &'static str; }`
  - `World`: `new()`, `register_component::<T: Component>() -> Result<(), EcsError>`, `register_resource::<T: Resource>(value: T) -> Result<(), EcsError>` (registers and inserts), `insert_runtime::<T: Send + 'static>(value: T)` (never hashed or snapshotted), `spawn() -> Entity`, `despawn(e) -> Result<(), EcsError>`, `is_alive(e) -> bool`, `entities() -> impl Iterator<Item = Entity>` (ascending index), `insert::<T>(e, value) -> Result<(), EcsError>`, `remove::<T>(e) -> Result<Option<T>, EcsError>`, `get::<T>(e) -> Option<std::cell::Ref<'_, T>>`, `get_mut::<T>(&mut self, e) -> Option<&mut T>`, `resource::<T>() -> Option<Ref<'_, T>>`, `resource_mut::<T>(&mut self) -> Option<&mut T>`, `runtime::<T>() -> Option<Ref<'_, T>>`, `runtime_mut::<T>(&mut self) -> Option<&mut T>`.
  - `#[derive(Debug, thiserror::Error)] pub enum EcsError { NoSuchEntity(Entity), UnregisteredComponent(&'static str), UnregisteredResource(&'static str), DuplicateName { name: &'static str, first: &'static str, second: &'static str } }` (`first` and `second` are `type_name`s).
  - `pub struct EntitiesState { pub generations: Vec<u32>, pub alive: Vec<bool>, pub free: Vec<u32> }` (`Clone, Debug, PartialEq, Serialize, Deserialize`), the allocator state; Task 4 snapshots and hashes it.
- Produces (`ludenea_macros`): `#[derive(Component)]`, `#[derive(Resource)]`, optional `#[component(name = "...")]` / `#[resource(name = "...")]` override. Default name `concat!(module_path!(), "::", "<Type>")`.
- Produces (`ludenea` facade): `pub use ludenea_math as math; pub use ludenea_ecs as ecs; pub use serde;` and `pub mod prelude` re-exporting `Vec2, Rot, Transform, Aabb, Entity, World, Component, Resource` plus the derives. Later tasks extend the facade and the prelude with their crates.

Macro path resolution (works for integration tests and the facade):
- `ludenea-ecs/src/lib.rs` contains `extern crate self as ludenea_ecs;` and re-exports `pub use ludenea_macros::{Component, Resource};` (Task 5 adds `Query`).
- The macro asks `proc-macro-crate` for `ludenea-ecs` first: `Itself` or `Name(n)` gives `::ludenea_ecs` (or `::n`); only when `ludenea-ecs` is not a dependency does it look for `ludenea`, giving `::ludenea::ecs`.
- Items the generated code touches are `pub` with `#[doc(hidden)]`.

- [ ] **Step 1: Write the failing tests** in `tests/world.rs`: `spawn_reuses_freed_index_with_bumped_generation` (spawn a, b; despawn a; spawn c gives `c.index() == a.index()`, `c.generation() == a.generation() + 1`), `free_list_is_last_in_first_out` (despawn 1 then 2: next spawns return index 2 then 1), `entities_iterate_in_ascending_index`, `get_on_stale_entity_returns_none`, `insert_unregistered_component_errors_with_its_name`, `runtime_resources_are_accessible`, `derive_uses_module_path_by_default_and_honours_override` (`assert_eq!(<Health as Component>::NAME, concat!(module_path!(), "::Health"))`; `#[component(name = "game::Hp")]` gives `"game::Hp"`), `registering_two_types_with_one_name_is_duplicate_name`.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs`. Expected: compile failure.
- [ ] **Step 3: Implement.** Storage per component: `RefCell<Vec<Option<T>>>` indexed by entity index, behind a type-erased `Box<dyn Storage + Send>` reachable by `TypeId` (lookup only) and by `NAME` in a `BTreeMap<&'static str, ...>` registry (iteration). Query order is then a pure function of entity identifiers, so equal hashes imply equal iteration order. Add `// ponytail: iteration visits every index up to the highest alive one; switch to sparse sets with a presence bitset if profiling shows it`. Allocator: `generations`, `alive`, LIFO `free`.
- [ ] **Step 4: Run** `cargo test -p ludenea-ecs && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(ecs): add world with generational entities, components, and resources`.

---

### Task 4: Canonical hashing, hash tree, snapshot, restore, diff

**Files:**
- Create: `crates/ludenea-ecs/src/{hash.rs, snapshot.rs, diff.rs}`
- Modify: `crates/ludenea-ecs/src/{component.rs, world.rs, lib.rs}`, `crates/ludenea-ecs/Cargo.toml` (`postcard` with `alloc`, `xxhash-rust` with `xxh3`, `ron`)
- Test: `crates/ludenea-ecs/tests/snapshot.rs`

**Interfaces:**
- Consumes: Task 3 `World`, registry, `EntitiesState`.
- Produces (`ludenea_ecs`):
  - `pub fn hash_value<T: Serialize>(value: &T) -> u64`: a `serde::Serializer` feeding `xxhash_rust::xxh3::Xxh3` with fixed-width little-endian integers (`usize`/`isize` as 64-bit), `u64` length prefixes for strings, bytes, sequences, and maps (`serialize_seq(None)` and `serialize_map(None)` return an error), `u32` variant indices, and floats as bits with every NaN replaced by `0x7fc0_0000` (`f32`) or `0x7ff8_0000_0000_0000` (`f64`).
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct HashTree { pub root: u64, pub resources: Vec<NamedHash>, pub entities: Vec<EntityHash> }`, `NamedHash { pub name: String, pub hash: u64 }`, `EntityHash { pub entity: Entity, pub hash: u64, pub components: Vec<NamedHash> }`.
  - `World::hash_tree(&self) -> HashTree`, `World::hash(&self) -> u64` (equals `hash_tree().root`).
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Snapshot { pub version: u32, pub entities: EntitiesState, pub resources: Vec<(String, Vec<u8>)>, pub components: Vec<(String, Vec<(Entity, Vec<u8>)>)> }`. `resources` and `components` list **every registered name** in name order, even with no instances. `pub const SNAPSHOT_VERSION: u32 = 1`, `Snapshot::encode(&self) -> Vec<u8>` (postcard), `Snapshot::decode(bytes: &[u8]) -> Result<Snapshot, SnapshotError>`.
  - `World::snapshot(&self) -> Snapshot`, `World::restore(&mut self, snapshot: &Snapshot) -> Result<(), SnapshotError>`: requires the snapshot's name sets to equal the world's registered sets exactly, decodes everything before mutating, leaves runtime resources untouched.
  - `#[derive(Debug, thiserror::Error)] pub enum SnapshotError { Version { found: u32, expected: u32 }, UnknownComponent(String), UnknownResource(String), MissingComponent(String), MissingResource(String), Decode { name: String, reason: String } }`.
  - `World::diff_snapshots(&self, a: &Snapshot, b: &Snapshot) -> Result<Vec<Difference>, SnapshotError>`; `pub struct Difference { pub subject: String, pub component: String, pub path: String, pub left: String, pub right: String }` with `Display` as `"{subject} {component} {path}: {left} != {right}"`. `subject` is `"entity 3v0"`, `"resource"`, or `"entities"`; a missing component shows `"absent"`.
  - Registry gains, per component and resource name, type-erased functions: encode to postcard bytes, decode bytes and insert, hash, encode to a RON string (`ron::to_string`), and `decode_ron(&str) -> Result<Box<dyn Any + Send>, String>` through `ron::from_str::<T>` (typed: `ron::Value` loses enum variants and rounds floats through `f64`, so it is never used to build values). Task 8 consumes the RON pair.
  - Component and resource types must not use `#[serde(flatten)]` or `skip_serializing_if` (postcard cannot decode them); document this on the traits.

Hash tree layout (spec section 9.2):
- component hash = `hash_value(component)`;
- entity hash = xxh3 over, for each component of the entity sorted by name: `name.len() as u64` LE, name bytes, component hash LE;
- resource leaves = each registered resource sorted by name, plus a leaf named `ludenea::Entities` = `hash_value(&EntitiesState)`;
- root = xxh3 over each resource leaf (`name.len()` LE, name, hash LE), then each alive entity ascending (`index` LE, `generation` LE, entity hash LE).

Diff approach: compare postcard bytes per component or resource first. Where bytes differ, render both sides with `ron::to_string`, parse with `ron::from_str::<ron::Value>` for display only, and walk recursively (`.field` for string map keys, `[i]` for sequences, `[<ron key>]` for other map keys). If the walk finds nothing (enum variants collapse in `ron::Value`), emit one `Difference` with `path` empty and the full RON strings.

- [ ] **Step 1: Write the failing tests**: `hash_is_independent_of_insertion_order`, `hash_changes_when_a_field_changes`, `hash_changes_when_free_list_order_changes`, `nan_payloads_hash_equal` (`f32::from_bits(0x7fc0_0001)` versus `f32::NAN` in a component), `snapshot_restore_round_trip_preserves_hash_and_iteration_order`, `restore_rejects_other_version_and_leaves_world_unchanged`, `restore_rejects_unknown_component_and_leaves_world_unchanged` (snapshot from a world with an extra registered component), `restore_rejects_missing_resource_and_leaves_world_unchanged` (snapshot from a world with one resource fewer), `diff_reports_entity_component_and_field_path` (a `Difference` with `path == ".hp"`, `left == "10"`, `right == "9"`), `diff_reports_enum_variant_change` (`Static` versus `Dynamic` in a test enum field yields one difference), `golden_hash_value_of_known_struct` (frozen constant recorded with the Task 1 procedure).
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs --test snapshot`. Expected: compile failure.
- [ ] **Step 3: Implement** `hash.rs`, `snapshot.rs`, `diff.rs`.
- [ ] **Step 4: Run** `cargo test -p ludenea-ecs && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(ecs): add hash tree, snapshots, and field-level snapshot diff`.

---

### Task 5: Queries and named queries

**Files:**
- Create: `crates/ludenea-ecs/src/query.rs`
- Modify: `crates/ludenea-macros/src/lib.rs` (`derive(Query)`), `crates/ludenea-ecs/src/lib.rs` (re-export the derive), facade prelude (`Query, With, Without`)
- Test: `crates/ludenea-ecs/tests/query.rs`

**Interfaces:**
- Consumes: Task 3 world and storages.
- Produces (`ludenea_ecs`):
  - `pub trait QueryData { type Item<'a>; ... }`, `pub unsafe trait ReadOnlyQueryData: QueryData { type ReadItem<'a>; }` implemented for `&T`, `&mut T` (`QueryData` only), `Entity`, `Option<&T>`, `Option<&mut T>`, tuples up to 8. `pub trait QueryFilter` for `()`, `With<T>`, `Without<T>`, tuples up to 4. Each declares its accesses (`Read(name)` or `Write(name)`).
  - `pub struct Query<'w, D: QueryData + 'static, F: QueryFilter + 'static = ()>`: `iter(&self)` and `get(&self, e) -> Result<D::ReadItem<'_>, QueryError>` when `D: ReadOnlyQueryData`, plus `iter_mut(&mut self)` and `get_mut(&mut self, e) -> Result<D::Item<'_>, QueryError>`. Iteration ascending entity index. `QueryError::{NoSuchEntity(Entity), DoesNotMatch(Entity)}`.
  - `pub fn accesses<D: QueryData, F: QueryFilter>() -> Vec<Access>` and `pub fn check_accesses(accesses: &[Access]) -> Result<(), (&'static str, usize, usize)>`; a query whose own data conflicts (`(&mut A, &A)`, `(&mut A, Option<&A>)`) is rejected. Task 6 uses both for system parameters.
  - `#[derive(Query)]` on a struct with lifetime `'w` whose fields are `&'w T`, `&'w mut T`, `Entity`, or `Option<...>` of those; filter through a struct attribute `#[query(filter = Without<Dead>)]` (the spec's field-less `#[filter(...)]` is not valid Rust). Generated impl follows Bevy: `impl<'x> QueryData for Fighter<'x> { type Item<'a> = Fighter<'a>; }`, and `Query<Fighter>` in a system resolves to `Query<Fighter<'static>>`. The derive emits `ReadOnlyQueryData` syntactically, only when no field is `&mut` (a `where` bound on field types would be a trivial bound, rejected on stable).

Borrowing approach: a query holds `Ref`/`RefMut` guards on its storages. Mutable iteration derives a raw base pointer once when the query is created (`RefMut` to `as_mut_ptr()`) and never touches the guard again while items are alive; each index is visited once, so the yielded `&mut T` are disjoint.

- [ ] **Step 1: Write the failing tests**: `query_iterates_in_ascending_entity_order`, `with_and_without_filters_select_expected_entities`, `get_mut_on_stale_entity_is_no_such_entity`, `get_on_non_matching_entity_is_does_not_match`, `iter_mut_writes_every_matching_component`, `derived_query_with_filter_skips_filtered_entities`, `derived_read_only_query_supports_iter`, `query_with_conflicting_data_is_rejected` (`check_accesses(&accesses::<(&mut Health, &Health), ()>())` is `Err` naming `Health`).
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs --test query`. Expected: compile failure.
- [ ] **Step 3: Implement** `query.rs` and `derive(Query)`.
- [ ] **Step 4: Run** `cargo test -p ludenea-ecs && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS. Optional when nightly is installed: `cargo +nightly miri test -p ludenea-ecs --test query` (no undefined behaviour reported).
- [ ] **Step 5: Commit** `✨ feat(ecs): add typed queries with filters and named query structs`.

---

### Task 6: Systems, schedule, commands, events

**Files:**
- Create: `crates/ludenea-ecs/src/{system.rs, schedule.rs, commands.rs, event.rs}`
- Modify: `crates/ludenea-ecs/src/{world.rs, lib.rs}`, facade prelude (`Res, ResMut, Runtime, RuntimeMut, Events, EventWriter, Commands, SystemLabel`)
- Test: `crates/ludenea-ecs/tests/systems.rs`, `crates/ludenea-ecs/tests/schedule.rs`

**Interfaces:**
- Consumes: Task 5 `Query`, `accesses`, `check_accesses`.
- Produces (`ludenea_ecs`):
  - System parameters: `Query`, `Res<'w, T: Resource>`, `ResMut<'w, T: Resource>`, `Runtime<'w, T>` (read a runtime resource), `RuntimeMut<'w, T>`, `Events<'w, T>` (`iter()`), `EventWriter<'w, T>` (`send(T)`), `Commands<'w>`. Functions with up to 8 parameters implement `IntoSystem` (bound `for<'a> &'a mut F: FnMut(P0, ...) + FnMut(P0::Item<'a>, ...)`).
  - `Commands`: `spawn() -> SpawnCommands` (`.insert(c) -> SpawnCommands`), `despawn(e)`, `insert(e, c)`, `remove::<T>(e)`. Applied in issue order right after the issuing system returns. Despawn, insert, or remove on a dead entity is ignored; inserting an unregistered component panics with the component name (a programming error).
  - Events: `World::register_event::<T: Send + 'static>()`, `World::clear_events()`. Events live for the current tick only, so there are no pending events or commands between ticks.
  - `pub trait SystemLabel: 'static { fn name(&self) -> &'static str; }`. System configuration: `f.in_set(L)`, `f.after(L)`, `f.before(L)`, chainable.
  - `ScheduleBuilder::add<M>(&mut self, system: impl IntoSystemConfig<M>)`, `ScheduleBuilder::build(self, world: &World) -> Result<Schedule, BuildError>`, `Schedule::run(&mut self, world: &mut World)` (sequential; each system followed by its commands).
  - `#[derive(Debug, thiserror::Error)] pub enum BuildError { AccessConflict { system: &'static str, component: &'static str, first: usize, second: usize }, Unregistered { system: &'static str, kind: &'static str, name: &'static str }, OrderingCycle { systems: Vec<&'static str> }, EmptyLabel { label: &'static str } }`. Messages:
    - access conflict: ``"system `{system}` accesses `{component}` in parameters {first} and {second}, and at least one is mutable. Merge them into one query, or split the system in two."`` (`first == second` when one query conflicts with itself);
    - unregistered: ``"system `{system}` uses {kind} `{name}`, which is not registered. Register it on the app (component, resource, event, or runtime resource)."``;
    - empty label: ``"label `{label}` has no systems. Did you forget to add the plugin that provides it?"``.
    System names come from `std::any::type_name` (diagnostics only, never persisted).
  - Ordering: topological sort of `before` and `after` constraints, ties broken by registration index (Kahn's algorithm over a `BTreeSet` of ready indices).

Parameters declare accesses and required registrations at build time, so the `RefCell` borrows taken in `fetch` never fail at run time.

- [ ] **Step 1: Write the failing tests** in `systems.rs`: `commands_apply_in_issue_order_after_system` (system A spawns 3 entities with values 1, 2, 3; system B, after A, reads them in that order), `commands_on_dead_entity_are_ignored`, `events_are_visible_to_later_systems_and_cleared_by_clear_events`, `conflicting_parameters_fail_at_build_with_named_message` (`fn bad(_: Query<&mut Health>, _: Query<&Health>)`; the error string contains `Health` and `bad`), `unregistered_resource_fails_at_build`, `res_mut_changes_are_visible_to_next_system`.
- [ ] **Step 2: Write the failing tests** in `schedule.rs`: `unordered_systems_run_in_registration_order`, `after_constraint_reorders`, `cycle_is_reported_with_system_names`, `after_unknown_label_is_empty_label_error`.
- [ ] **Step 3: Run** `cargo test -p ludenea-ecs`. Expected: compile failure.
- [ ] **Step 4: Implement** the four modules.
- [ ] **Step 5: Run** `cargo test -p ludenea-ecs && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 6: Commit** `✨ feat(ecs): add function systems, deterministic schedule, commands, and events`.

---

### Task 7: App, simulation loop, time, RNG, parallelism

**Files:**
- Create: `crates/ludenea-app/{Cargo.toml, src/lib.rs, src/app.rs, src/simulation.rs, src/time.rs, src/rng.rs, src/input.rs, src/parallel.rs}`
- Modify: facade (`pub use ludenea_app as app;`, prelude: `App, Plugin, Sim, Simulation, Time, Rng, Inputs, Input`)
- Test: `crates/ludenea-app/tests/simulation.rs`, inline tests in `rng.rs` and `parallel.rs`

**Interfaces:**
- Consumes: Tasks 3 to 6.
- Produces (`ludenea_app`):
  - `pub trait Plugin { fn build(&self, app: &mut App); }`, `pub struct Sim;` (schedule marker).
  - `App`: `new()` (registers `Time` and `Rng::new(0, 0)`), `plugin(&mut self, p: impl Plugin) -> &mut Self`, `component::<T: Component>()`, `resource::<T: Resource>(value: T)`, `event::<T: Send + 'static>()`, `input::<I: Input>()`, `system<M>(&mut self, schedule: Sim, s: impl IntoSystemConfig<M>) -> &mut Self`, `tick_rate(&mut self, hertz: u32) -> &mut Self` (default 60), `build(self) -> Result<Simulation, AppError>`. Registration errors (duplicate names) are collected and returned by `build`. `#[derive(Debug, thiserror::Error)] pub enum AppError { Ecs(#[from] EcsError), Build(#[from] BuildError) }`.
  - `pub trait Input: Serialize + DeserializeOwned + Default + Clone + Send + 'static {}` with a blanket impl.
  - `pub struct Inputs<I> { players: Vec<I> }` runtime resource: `player(&self, index: usize) -> &I`, `len()`. Systems read it with `Runtime<Inputs<I>>`.
  - `#[derive(Resource)] #[resource(name = "ludenea::Time")] pub struct Time { pub tick: u64, pub dt: f32 }`.
  - `#[derive(Resource)] #[resource(name = "ludenea::Rng")] pub struct Rng { state: u64, increment: u64 }`: PCG32 XSH RR with `wrapping_*` arithmetic. `new(seed: u64, stream: u64)` equals reference `pcg32_srandom_r`, `next_u32()`, `range_u32(low, high) -> u32` in `[low, high)` (Lemire's unbiased bounded method with rejection; panics with a message when `high <= low`), `range_f32(low, high) -> f32` = `low + (high - low) * ((next_u32() >> 8) as f32 * (1.0 / 16_777_216.0))`.
  - `Simulation`: `tick() -> u64`, `players() -> usize` (initially 1), `set_players(&mut self, players: usize)`, `step(&mut self)` (default inputs for `players()`), `step_with<I: Input>(&mut self, inputs: &[I]) -> Result<(), SimError>`, `step_encoded(&mut self, inputs: Option<&[u8]>) -> Result<(), SimError>` (postcard `Vec<I>`; `None` = defaults), `world()`, `world_mut()`, `hash()`, `hash_tree()`, `snapshot()`, `restore(&Snapshot) -> Result<(), SnapshotError>`, `set_threads(&mut self, threads: usize)`, `threads() -> usize`. `build()` keeps the snapshot of the freshly built world as the initial state (Task 8 uses it for scenes).
  - Step order: write `Inputs` → run `Sim` schedule → `clear_events()` → `Time.tick += 1`.
  - `#[derive(Debug, thiserror::Error)] pub enum SimError { NoInputType, InputTypeMismatch { expected: &'static str }, InputDecode(String), InputLogVersion { found: u32, expected: u32 } }`. Decoding uses `postcard::take_from_bytes` and rejects leftover bytes.
  - `Parallel` runtime resource: `new(threads: usize)`, `threads()`, `map<T: Sync, R: Send>(&self, items: &[T], chunk: usize, f: impl Fn(&T) -> R + Sync + Send) -> Vec<R>` (output in item order), `for_each_mut<T: Send>(&self, items: &mut [T], chunk: usize, f: impl Fn(&mut T) + Sync + Send)`. Native: a private `rayon::ThreadPool` when `threads > 1`, sequential otherwise. `cfg(target_family = "wasm")`: always sequential, `rayon` excluded through a target-specific dependency.

- [ ] **Step 1: Write the failing tests**: `pcg32_matches_reference_sequence` (`Rng::new(42, 54)` yields `0xa15c02b7, 0x7b47f409, 0xba1d3330, 0x83d2f293, 0xbfa4784b, 0xcbed606e`), `range_u32_stays_in_bounds`, `step_advances_tick_and_runs_systems_in_order`, `same_inputs_same_hash_twice`, `snapshot_restore_then_replay_matches_uninterrupted_run` (100 ticks with RNG and commands; snapshot at 50), `step_with_wrong_input_type_is_type_mismatch`, `step_encoded_without_input_type_is_no_input_type`, `parallel_map_preserves_order_for_any_thread_count` (1, 2, 8 threads give equal vectors), `simulation_is_send` (`fn assert_send<T: Send>() {}` then `assert_send::<Simulation>()`).
- [ ] **Step 2: Run** `cargo test -p ludenea-app`. Expected: compile failure.
- [ ] **Step 3: Implement** the modules.
- [ ] **Step 4: Run** `cargo test -p ludenea-app && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): add simulation loop, seeded RNG, inputs, and thread pool`.

---

### Task 8: Input logs and RON scenes

**Files:**
- Create: `crates/ludenea-app/src/{input_log.rs, scene.rs}`
- Modify: `crates/ludenea-app/src/{simulation.rs, lib.rs}`, facade prelude (`InputLog`)
- Test: `crates/ludenea-app/tests/{input_log.rs, scene.rs}`

**Interfaces:**
- Consumes: Task 7 `Simulation`, Task 4 RON registry functions.
- Produces (`ludenea_app`):
  - `#[derive(Serialize, Deserialize)] pub struct InputLog { pub version: u32, pub players: u16, pub ticks: Vec<Vec<u8>> }` (each entry = postcard `Vec<I>`), `INPUT_LOG_VERSION = 1`, `new(players: u16)`, `record<I: Input>(&mut self, inputs: &[I])`, `encode() -> Vec<u8>`, `decode(&[u8]) -> Result<InputLog, SimError>`, `tick(&self, tick: u64) -> Option<&[u8]>`.
  - `Simulation::load_scene(&mut self, ron: &str) -> Result<(), SceneError>`, `Simulation::save_scene(&self) -> String`.
  - Scene RON format (version 1):

```ron
Scene(
    version: 1,
    resources: { "ludenea::Rng": (state: 1, increment: 3) },
    entities: [
        { "ludenea::Pose": (position: (x: 0.0, y: 1.0), rotation: (cos: 1.0, sin: 0.0)) },
    ],
)
```

  Parsed as `struct SceneFile { version: u32, resources: BTreeMap<String, Box<ron::value::RawValue>>, entities: Vec<BTreeMap<String, Box<ron::value::RawValue>>> }`; each value goes through the registry's typed `decode_ron(raw.get_ron())`. `load_scene` decodes everything first, then restores the initial snapshot from `build()`, spawns entities in file order (entity `i` of the file gets index `i`, guaranteed because the initial world has no entities and an empty free list), and overrides the listed resources. On error the simulation is unchanged.
  - `save_scene` writes alive entities in ascending order and all registered resources. It does not keep generations or the free list, so it renumbers entities: it is meant for freshly built worlds (scene generators), documented on the method.
  - `#[derive(Debug, thiserror::Error)] pub enum SceneError { Parse(String), Version(u32), UnknownComponent { entity: usize, name: String }, UnknownResource(String), InvalidValue { entity: Option<usize>, name: String, reason: String } }`. A duplicated component key inside one entity is `InvalidValue`.

- [ ] **Step 1: Write the failing tests**: `input_log_shorter_than_run_uses_defaults` (log of 3 ticks, run 5: ticks 4 and 5 see `I::default()`), `input_log_for_another_type_is_decode_error` (a log of `u64::MAX` values read as a `bool` input), `input_log_round_trips`, `scene_round_trip_save_then_load_gives_same_hash`, `scene_round_trip_is_bit_exact` (a component with `u64::MAX`, `-0.0`, a subnormal, `f32::MAX`, and 10,000 pseudo-random `f32` bit patterns excluding NaN; save then load gives identical snapshot bytes), `scene_with_enum_fields_loads` (a test component holding an enum with unit and tuple variants), `scene_unknown_component_names_entity_and_component_and_leaves_state` (error `UnknownComponent { entity: 1, name: "nope::Thing" }`, hash unchanged), `scene_malformed_value_is_invalid_value`, `scene_duplicate_component_key_is_invalid_value`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app`. Expected: compile failure.
- [ ] **Step 3: Implement** `input_log.rs` and `scene.rs`.
- [ ] **Step 4: Run** `cargo test -p ludenea-app && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): add input logs and RON scene loading`.

---

### Task 9: Physics components, shapes, mass properties

**Files:**
- Create: `crates/ludenea-physics/{Cargo.toml, src/lib.rs, src/components.rs, src/shape.rs, src/mass.rs}`
- Test: inline tests in `shape.rs` and `mass.rs`

**Interfaces:**
- Consumes: `ludenea_math`, `ludenea_ecs` derives.
- Produces (`ludenea_physics`):
  - Components: `Pose { pub position: Vec2, pub rotation: Rot }` (`ludenea::Pose`, the body origin), `Body { pub kind: BodyKind, pub linear_velocity: Vec2, pub angular_velocity: f32, pub linear_damping: f32, pub angular_damping: f32, pub gravity_scale: f32 }` (`ludenea::Body`; `Body::dynamic()`, `Body::fixed()` for static, `Body::kinematic()`; gravity scale 1, damping 0), `pub enum BodyKind { Static, Kinematic, Dynamic }`, `Collider { pub shape: Shape, pub density: f32, pub friction: f32, pub restitution: f32 }` (`ludenea::Collider`; `Collider::new(shape)` with density 1, friction 0.6, restitution 0). One collider per entity in this milestone.
  - Resource `PhysicsSettings { pub gravity: Vec2, pub substeps: u32, pub contact_hertz: f32, pub contact_damping_ratio: f32, pub contact_push_max_velocity: f32, pub restitution_threshold: f32 }` (`ludenea::PhysicsSettings`, `Default` = Global Constraints values).
  - `pub const LINEAR_SLOP: f32 = 0.005;`, `pub const MAX_POLYGON_VERTICES: usize = 8;`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub enum Shape { Circle(Circle), Capsule(Capsule), Polygon(Polygon) }`, `Circle { pub center: Vec2, pub radius: f32 }`, `Capsule { pub center1: Vec2, pub center2: Vec2, pub radius: f32 }`.
  - `Polygon` (`Clone, Debug, PartialEq`): private fields `vertices`, `normals` (arrays of `MAX_POLYGON_VERTICES`), `count`, `centroid`, `radius`. **All fields are serialized** (through a `PolygonData` mirror with `#[serde(try_from, into)]`); loading validates (`3..=8` vertices, CCW and strictly convex, unit normals matching edges within 1e-4, finite values) but never recomputes, so restore and scene load give bit-identical polygons and equal hashes imply equal behaviour.
  - One private `finish(vertices: &[Vec2], radius: f32) -> Polygon` computes normals and centroid; `Polygon::new`, `Polygon::make_box`, and the internal capsule conversion all go through it. `Polygon::new(points: &[Vec2], radius: f32) -> Result<Polygon, ShapeError>` computes a canonical hull (gift wrapping, start vertex = lowest x then lowest y, CCW, points closer than `0.5 * LINEAR_SLOP` welded), so it is idempotent on its own output. `Polygon::make_box(half_width: f32, half_height: f32) -> Polygon`. `Polygon::vertices(&self) -> &[Vec2]`, `Polygon::radius(&self) -> f32`. `pub(crate) fn Polygon::from_segment(c1: Vec2, c2: Vec2, radius: f32) -> Polygon` (2-vertex rounded polygon for capsules, Task 10).
  - `Shape::compute_aabb(&self, xf: Transform) -> Aabb`, `Shape::validate(&self) -> Result<(), ShapeError>` (radius > 0 for circles and capsules, distinct capsule centers), also run when a `Collider` deserializes.
  - `#[derive(Debug, thiserror::Error)] pub enum ShapeError { TooFewPoints, TooManyPoints, Collinear, NonPositiveRadius, InvalidPolygon(String) }`.
  - `#[derive(Copy, Clone, Debug, PartialEq)] pub struct MassData { pub mass: f32, pub center: Vec2, pub rotational_inertia: f32 }` (inertia about `center`, not about the body origin), `pub fn mass_data(shape: &Shape, density: f32) -> MassData`. Solver rule (Task 11): a dynamic body with `mass == 0` uses mass 1 and inverse inertia 0.

- [ ] **Step 1: Write the failing tests**:
  - `circle_mass`: r = 1, density 1: mass `PI` within 1e-6, inertia `0.5 * PI` within 1e-5.
  - `offset_circle_inertia_is_about_its_center`: center `(1, 2)`, r = 1: mass `PI`, center `(1, 2)`, inertia `0.5 * PI` (not `5.5 * PI`) within 1e-5.
  - `box_mass`: `make_box(1.0, 1.0)`, density 1: mass `4.0`, inertia `8.0 / 3.0` within 1e-5, center `ZERO`.
  - `capsule_mass`: centers `(-1, 0)` and `(1, 0)`, r = 0.5: mass `PI * 0.25 + 2.0` within 1e-5, center `ZERO`, inertia `2.05024` within 1e-4 (Box2D: `lc = 4r/(3π)`, `h = L/2`, `I = m_circle (0.5r² + h² + 2h·lc) + m_rect (4r² + L²)/12`).
  - `polygon_from_unordered_points_is_canonical_ccw_hull`, `polygon_new_is_idempotent` (`Polygon::new(p.vertices(), p.radius()) == p` bit for bit, also for `make_box`), `polygon_round_trip_is_bit_identical` (postcard and RON), `collinear_points_are_rejected`, `two_points_are_too_few`, `nine_hull_points_are_too_many`, `zero_radius_circle_is_rejected`, `polygon_deserialization_validates` (a RON polygon with collinear vertices fails).
- [ ] **Step 2: Run** `cargo test -p ludenea-physics`. Expected: compile failure.
- [ ] **Step 3: Implement** the three modules (polygon mass via triangle fan around the centroid, rounded polygons per Box2D v3 `b2ComputePolygonMass`, capsule per the formula above).
- [ ] **Step 4: Run** `cargo test -p ludenea-physics && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(physics): add bodies, colliders, convex shapes, and mass properties`.

---

### Task 10: Narrow phase contact manifolds

**Files:**
- Create: `crates/ludenea-physics/src/manifold.rs`
- Test: inline tests in `manifold.rs`

**Interfaces:**
- Consumes: Task 9 shapes, `LINEAR_SLOP`, `Polygon::from_segment`, `Transform`.
- Produces:
  - `#[derive(Copy, Clone, Debug, Default, PartialEq)] pub struct ManifoldPoint { pub point: Vec2, pub anchor_a: Vec2, pub anchor_b: Vec2, pub separation: f32, pub id: u16 }` (`point` in world space; anchors = `point - xf.position` for each body origin).
  - `#[derive(Copy, Clone, Debug, Default, PartialEq)] pub struct Manifold { pub normal: Vec2, pub points: [ManifoldPoint; 2], pub count: usize }` (normal from A to B).
  - `pub fn collide(a: &Shape, xf_a: Transform, b: &Shape, xf_b: Transform, speculative_distance: f32) -> Manifold`. Points with `separation > speculative_distance` are dropped.

Approach (Box2D v3 structure): dedicated `collide_circles`, `collide_capsule_circle`, `collide_polygon_circle`; capsules otherwise become `Polygon::from_segment` and go through `collide_polygons`. `collide_polygons`: SAT over both cores; B becomes the reference only if `separation_b > separation_a + 0.1 * LINEAR_SLOP` (hysteresis, so stacked boxes do not flip reference faces and lose warm starting). When the closest features fall outside the reference edge region and the core separation exceeds `0.1 * LINEAR_SLOP`, use segment-segment closest points (one point); otherwise clip the incident edge against the reference edge side planes (up to two points). Feature id = `(reference_edge << 8) | incident_vertex`, plus `0x8000` when B owns the reference edge. Swapped dispatch negates the normal and swaps the anchors.

- [ ] **Step 1: Write the failing tests**: `overlapping_circles` (r = 1 at `(0, 0)` and `(1.5, 0)`: `count == 1`, `normal == (1, 0)`, `separation == -0.5`, `point == (0.75, 0)` within 1e-6), `distant_circles_have_no_points` (gap 1.0, speculative 0.02), `box_resting_on_box_gives_two_points` (`make_box(0.5, 0.5)` at origin and at `(0, 0.9)`: `count == 2`, `normal ≈ (0, 1)`, each separation `≈ -0.1` within 1e-5, points sorted by id), `circle_on_box_face`, `capsule_lying_on_box_gives_two_points`, `rotated_box_corner_contact_gives_one_point`, `swapping_arguments_negates_normal` (100 pseudo-random configurations from a local xorshift; skip configurations where the two SAT separations are within `0.1 * LINEAR_SLOP`; equal counts, and when `count > 0`, `normal_ab == -normal_ba` within 1e-5), `ids_are_stable_under_small_motion` (shift B by 0.001: same ids).
- [ ] **Step 2: Run** `cargo test -p ludenea-physics manifold`. Expected: compile failure.
- [ ] **Step 3: Implement** `manifold.rs`.
- [ ] **Step 4: Run** `cargo test -p ludenea-physics && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(physics): add contact manifolds for circles, capsules, and polygons`.

---

### Task 11: Physics pipeline: broad phase, contact cache, soft step solver, plugin

**Files:**
- Create: `crates/ludenea-physics/src/{broad_phase.rs, contacts.rs, solver.rs, step.rs, plugin.rs}`
- Modify: `crates/ludenea-physics/Cargo.toml` (depend on `ludenea-app`), `crates/ludenea-physics/src/lib.rs` (re-export `PhysicsPlugin, Step, ContactCache, find_pairs, Proxy, apply_impulse, mass_data`), facade (`pub use ludenea_physics as physics;`, prelude: `Pose, Body, BodyKind, Collider, Shape, Circle, Capsule, Polygon, PhysicsPlugin, PhysicsSettings`)
- Test: `crates/ludenea-physics/tests/pipeline.rs`

**Interfaces:**
- Consumes: Tasks 7 to 10 (`App`, `Plugin`, `Time`, `Parallel`, `Runtime`).
- Produces:
  - `pub struct PhysicsPlugin;` registering `Pose`, `Body`, `Collider`, `PhysicsSettings::default()`, `ContactCache::default()`, and `physics_step` in `Sim` with `.in_set(Step)` (Task 13 adds the `Collision` event).
  - `pub struct Step;` implementing `SystemLabel` (name `"physics::Step"`).
  - `#[derive(Resource, Default, Serialize, Deserialize)] #[resource(name = "ludenea::ContactCache")] pub struct ContactCache { pairs: BTreeMap<(Entity, Entity), CachedContact> }`, `CachedContact { touching: bool, points: Vec<CachedPoint> }`, `CachedPoint { id: u16, normal_impulse: f32, tangent_impulse: f32 }` (both `Serialize, Deserialize`). Keys are `(lower, higher)` entity.
  - `pub struct Proxy { pub aabb: Aabb, pub body: u32, pub kind: BodyKind }`, `pub fn find_pairs(proxies: &[Proxy]) -> Vec<(u32, u32)>`: sort and sweep on `aabb.min.x` (`total_cmp`, ties by `body`), skip pairs with no dynamic body, output sorted `(lower, higher)`. `Proxy.aabb = shape.compute_aabb(xf).fattened(4 * LINEAR_SLOP)` so speculative contacts are found. Comment `// ponytail: sort and sweep rebuilt every tick; dynamic tree if body counts grow past a few thousand`.
  - `pub fn apply_impulse(body: &mut Body, collider: &Collider, impulse: Vec2)` (center of mass impulse; no effect on non-dynamic bodies).

Gather and write-back:
- Each tick, gather entities with `Pose` and `Body` (collider optional; colliders without `Body` are ignored) into a body array in ascending entity order. Static bodies get zero velocity in the array (a velocity typed into a scene must not act as a conveyor belt). Per body: `local_center`, `c0 = origin + rotate(q0, local_center)`, inverse mass and inertia (zero for static and kinematic), `delta_position = ZERO`, `delta_rotation = Rot::IDENTITY`.
- Write-back: `c = c0 + delta_position`, `q = delta_rotation.mul(q0).normalize()`, `origin = c - rotate(q, local_center)`, velocities into `Body` (static bodies untouched). `assert!` (release too) that positions and velocities are finite, with a message naming the entity.

Soft step algorithm (spec section 8, Box2D v3), `h = dt / substeps`:

```text
softness(hertz, zeta, h): if hertz == 0 { (bias_rate 0, mass_scale 1, impulse_scale 0) }
  omega = 2*PI*hertz; a1 = 2*zeta + h*omega; a2 = h*omega*a1; a3 = 1/(1 + a2)
  bias_rate = omega/a1; mass_scale = a2*a3; impulse_scale = a3
contact_hertz = min(settings.contact_hertz, 0.25/h)
per constraint softness: softness(2*contact_hertz, zeta, h) if either body has inv_mass == 0, else softness(contact_hertz, zeta, h)

prepare each contact point:
  rA = anchor_a - rotate(qA, local_center_a)   (relative to center of mass, fixed for the tick); same for rB
  normal_mass, tangent_mass; adjusted_separation = separation - dot(rB - rA, n)
  relative_velocity = dot(n, vB + cross_sv(wB, rB) - vA - cross_sv(wA, rA)); max_normal_impulse = 0
  warm-start impulses from ContactCache when the point id matches

for each substep:
  integrate velocities (dynamic only):
    v = h*gravity*gravity_scale + v / (1 + h*linear_damping); w = w / (1 + h*angular_damping)
    clamp |v| <= 400 m/s and |w| <= 0.25*PI/h (guards for the rotation integration)
  warm start: apply normal_impulse*n + tangent_impulse*t, t = right_perp(n)
  solve(use_bias = true)
  integrate positions (dynamic and kinematic): delta_position += h*v; delta_rotation = delta_rotation.integrate(h*w)
  solve(use_bias = false)    // relax
restitution (if > 0): skip points with relative_velocity > -threshold or max_normal_impulse == 0;
  impulse = -normal_mass*(vn + restitution*relative_velocity), accumulated clamp >= 0,
  max_normal_impulse = max(max_normal_impulse, applied impulse)
store impulses into ContactCache

solve constraint: points in order, normal impulses first, then friction, recomputing vn and vt from current velocities
  normal point: d = (dpB - dpA) + rotate(dqB, rB) - rotate(dqA, rA); s = dot(d, n) + adjusted_separation
    s > 0           : bias = s/h, mass_scale = 1, impulse_scale = 0       (speculative)
    use_bias        : bias = max(bias_rate*s, -push_max_velocity), constraint softness scales
    otherwise       : bias = 0, mass_scale = 1, impulse_scale = 0
    impulse = -normal_mass*mass_scale*(vn + bias) - impulse_scale*accumulated
    new = max(accumulated + impulse, 0); impulse = new - accumulated; accumulated = new
    max_normal_impulse = max(max_normal_impulse, impulse)
  friction point (both passes): impulse = -tangent_mass*vt; clamp accumulated to +-friction*accumulated_normal
```

In this task constraints are solved in pair order; Task 12 replaces that order with graph colors.

- [ ] **Step 1: Write the failing tests** in `pipeline.rs`:
  - `find_pairs_skips_static_static_and_is_sorted`.
  - `falling_box_lands_and_rests_on_ground`: box `make_box(0.5, 0.5)` falls from y = 5 onto a static ground `make_box(10, 0.5)` at y = 0; after 300 ticks `|v| < 0.01` and center height within 0.01 of 1.0.
  - `pyramid_of_ten_rows_is_stable`: 55 boxes of half size 0.5, friction 0.6, rows without gaps, stacked at exactly zero separation on the ground; the maximum displacement over all boxes between tick 300 and tick 600 is below 0.05.
  - `bouncing_ball_with_restitution_rebounds`: circle r = 0.25, restitution 0.8, center dropped 4 m above its resting height; apex = maximum center height after the first velocity reversal, measured from the resting height; between 50% and 80% of the drop height (expected about 64%).
  - `kinematic_body_moves_at_constant_velocity_and_ignores_gravity`.
  - `static_body_velocity_is_ignored`.
  - `zero_density_dynamic_body_does_not_produce_nan`.
  - `offset_center_of_mass_rotates_about_its_center` (a body whose collider circle is centered at `(1, 0)` spinning freely in zero gravity keeps its center of mass fixed within 1e-4).
  - `two_runs_give_identical_hash_sequences`.
- [ ] **Step 2: Run** `cargo test -p ludenea-physics --test pipeline`. Expected: compile failure.
- [ ] **Step 3: Implement** the five modules.
- [ ] **Step 4: Run** `cargo test -p ludenea-physics && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(physics): add soft step contact solver and physics plugin`.

---

### Task 12: Graph coloring and thread-count-independent parallel solve

**Files:**
- Create: `crates/ludenea-physics/src/coloring.rs`
- Modify: `crates/ludenea-physics/src/{solver.rs, step.rs}`
- Test: inline tests in `coloring.rs`, `crates/ludenea-physics/tests/threads.rs`

**Interfaces:**
- Consumes: Task 11 solver, Task 7 `Parallel`.
- Produces:
  - `pub const GRAPH_COLOR_COUNT: usize = 12;`, `pub const SOLVER_CHUNK: usize = 64;`
  - `pub fn color_constraints(pairs: &[(u32, u32)], is_dynamic: &[bool]) -> Vec<Vec<u32>>`: input = touching contacts only (`count > 0`), in pair order. Greedy: lowest color whose body set contains neither dynamic body; static and kinematic bodies never block a color. Constraints that fit no color go in a final overflow group. Returns `GRAPH_COLOR_COUNT + 1` groups of constraint indices, each ascending.

Approach: every solver stage that touches constraints (warm start, solve, relax, restitution) runs color by color, then the overflow group. Within a color, `Parallel::map` over `SOLVER_CHUNK`-sized chunks computes each constraint's new accumulated impulses and the new absolute velocities of its dynamic bodies from the current body state; a sequential scatter in constraint order then writes them. Coloring guarantees one constraint per dynamic body per color, and the scatter never writes static or kinematic bodies, so writes never collide and the result is identical for any thread count. The overflow group is solved in place, sequentially. Body integration stages use `Parallel::for_each_mut` with `SOLVER_CHUNK`. Comment `// ponytail: gather then scatter costs one copy per color; switch to disjoint unsafe writes if profiling shows it`.

- [ ] **Step 1: Write the failing tests**: `no_color_shares_a_dynamic_body`, `static_body_does_not_block_colors` (10 boxes on one static ground land in one color), `overflow_receives_constraints_beyond_twelve_colors` (one dynamic body touching 14 others), `coloring_is_deterministic`; in `threads.rs`: `hash_sequence_is_identical_for_one_two_and_eight_threads` (200 mixed bodies, 600 ticks, compare all per-tick hashes).
- [ ] **Step 2: Run** `cargo test -p ludenea-physics`. Expected: FAIL (coloring missing).
- [ ] **Step 3: Implement** `coloring.rs` and switch the solver to colored stages.
- [ ] **Step 4: Run** `cargo test -p ludenea-physics && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS (Task 11 behaviour tests still pass).
- [ ] **Step 5: Commit** `⚡️ feat(physics): solve contacts by graph color, independent of thread count`.

---

### Task 13: Collision events and scene queries

**Files:**
- Create: `crates/ludenea-physics/src/{events.rs, queries.rs}`
- Modify: `crates/ludenea-physics/src/{step.rs, contacts.rs, plugin.rs, lib.rs}`, facade prelude (`Collision, CollisionKind, raycast, overlap_aabb, RayHit`)
- Test: `crates/ludenea-physics/tests/events.rs`, `crates/ludenea-physics/tests/queries.rs`

**Interfaces:**
- Consumes: Tasks 11 and 12.
- Produces:
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub struct Collision { pub kind: CollisionKind, pub a: Entity, pub b: Entity }` (`a < b`), `pub enum CollisionKind { Begin, End }`. `PhysicsPlugin` registers the event; `physics_step` gains an `EventWriter<Collision>` and emits events sorted by `(a, b)`: `Begin` when a pair becomes touching (`count > 0`), `End` when it stops or when either entity is gone. Cache entries for dead entities are dropped in the same tick.
  - `#[derive(Copy, Clone, Debug, PartialEq)] pub struct RayHit { pub entity: Entity, pub point: Vec2, pub normal: Vec2, pub fraction: f32 }`.
  - `pub fn raycast(colliders: &Query<(Entity, &Pose, &Collider)>, origin: Vec2, translation: Vec2) -> Option<RayHit>`: closest hit, ties to the lower entity. Ray versus circle, capsule, and rounded polygon. A ray starting inside a shape does not hit that shape (Box2D behaviour).
  - `pub fn overlap_aabb(colliders: &Query<(Entity, &Pose, &Collider)>, aabb: Aabb) -> Vec<Entity>`: ascending entities whose shape AABB overlaps. Comment `// ponytail: brute force over all colliders; reuse the broad phase when queries get hot`.

- [ ] **Step 1: Write the failing tests**: `landing_box_emits_one_begin`, `separating_emits_end`, `despawn_while_touching_emits_end_and_does_not_panic` (despawn through `Commands` mid-contact; next tick emits `End` with the dead entity; `Query::get` on it returns `Err`), `events_are_sorted_by_pair`, `raycast_hits_nearest_box_with_expected_normal` (origin `(0, 5)`, translation `(0, -10)`, box top at y = 1: `fraction ≈ 0.4`, `normal ≈ (0, 1)` within 1e-4), `raycast_ties_break_to_lower_entity`, `raycast_misses_return_none`, `raycast_starting_inside_polygon_ignores_it`, `overlap_aabb_returns_ascending_entities`.
- [ ] **Step 2: Run** `cargo test -p ludenea-physics`. Expected: compile failure.
- [ ] **Step 3: Implement** `events.rs` and `queries.rs`, and wire event emission in `physics_step`.
- [ ] **Step 4: Run** `cargo test -p ludenea-physics && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(physics): add collision events, raycasts, and overlap queries`.

---

### Task 14: Control protocol, engine host, in-process transport

**Files:**
- Create: `crates/ludenea-control/{Cargo.toml, src/lib.rs, src/protocol.rs, src/host.rs, src/in_process.rs}`
- Modify: facade (`pub use ludenea_control as control;`)
- Test: `crates/ludenea-control/tests/host.rs`

**Interfaces:**
- Consumes: Task 7 `App`, `Simulation`; Task 8 `InputLog`, scenes; Task 4 `HashTree`, `Snapshot`, `diff_snapshots`.
- Produces (`ludenea_control`), all messages `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]`. `ClientMessage`, `ServerMessage`, `Command`, `Reply` use `#[serde(tag = "type", rename_all = "snake_case")]`; `ErrorCode` uses only `rename_all = "snake_case"`.
  - `pub const PROTOCOL_VERSION: u32 = 1;`
  - `pub enum ClientMessage { Hello { protocol_version: u32 }, Request { id: u64, command: Command } }`
  - `pub enum ServerMessage { Welcome { protocol_version: u32, engine_version: String }, Refused { error: ProtocolError }, Response { id: u64, result: Result<Reply, ProtocolError> } }` (`engine_version` = `env!("CARGO_PKG_VERSION")`; pushed engine events arrive in milestone 2).
  - `pub enum Command { LoadScene { ron: String }, LoadInputs { log: Bytes }, SetThreads { threads: u32 }, Step { ticks: u32, record_hashes: bool }, GetHash, GetHashTree, TakeSnapshot, Restore { snapshot: Bytes }, DiffSnapshots { a: Bytes, b: Bytes }, ListEntities, GetEntity { entity: Entity } }`
  - `pub enum Reply { Done, Stepped { tick: u64, hashes: Vec<Hash64> }, Hash { tick: u64, hash: Hash64 }, HashTree { tick: u64, tree: HashTree }, Snapshot { tick: u64, snapshot: Bytes }, Differences { lines: Vec<String> }, Entities { entities: Vec<Entity> }, Entity { entity: Entity, components: BTreeMap<String, String> } }` (component values as RON strings; snapshot bytes are exactly `Snapshot::encode()`).
  - `pub struct Bytes(pub Vec<u8>)`, serialized as a standard base64 string. `pub struct Hash64(pub u64)`, serialized as a 16-digit lowercase hexadecimal string (JavaScript numbers lose `u64` precision).
  - `pub struct ProtocolError { pub code: ErrorCode, pub message: String }`, `pub enum ErrorCode { VersionMismatch, NotConnected, InvalidScene, InvalidInputs, InvalidSnapshot, NoSuchEntity }`.
  - `pub struct Host`: `Host::new(app: App) -> Result<Host, AppError>`, `Host::handle(&mut self, command: Command) -> Result<Reply, ProtocolError>`. `LoadInputs` decodes the log and calls `set_players(log.players)`. `Step` feeds the log entry for the current tick, or defaults past its end; `record_hashes` returns the hash after each tick. `LoadScene` keeps the loaded input log.
  - `pub trait Transport { fn request(&mut self, command: Command) -> Result<Reply, ProtocolError>; }`
  - `pub struct InProcess`, `InProcess::connect(host: Host, protocol_version: u32) -> Result<InProcess, ProtocolError>` (handshake through `ClientMessage::Hello` and `ServerMessage::Welcome` or `Refused`; a mismatch gives `VersionMismatch` with message ``"client speaks protocol {client}, engine speaks {engine}"``). `impl Transport for InProcess` assigns increasing request ids and passes values without serializing.

- [ ] **Step 1: Write the failing tests** with a tiny test app (one component, one registered input type, one system that reads `Runtime<Inputs<TestInput>>` and uses `Rng`): `handshake_with_wrong_version_is_refused`, `step_with_record_hashes_returns_one_hash_per_tick`, `snapshot_then_restore_reproduces_hashes`, `step_past_input_log_end_uses_defaults`, `load_inputs_for_other_type_fails_with_invalid_inputs_on_step`, `invalid_scene_returns_invalid_scene_error`, `get_entity_on_dead_entity_returns_no_such_entity`, `diff_snapshots_returns_lines`, `every_command_and_reply_round_trips_through_json` (one value per variant through `serde_json`, a dev-dependency; `Hash64` appears as a hex string), `host_is_send`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control`. Expected: compile failure.
- [ ] **Step 3: Implement** the three modules.
- [ ] **Step 4: Run** `cargo test -p ludenea-control && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): add versioned control protocol with in-process host`.

---

### Task 15: CLI: run, verify, diff

**Files:**
- Create: `crates/ludenea-cli/{Cargo.toml, src/lib.rs, src/trace.rs, src/commands.rs}`
- Modify: facade (`pub use ludenea_cli as cli;`)
- Test: `crates/ludenea-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 14 `Host`, `InProcess`, `Transport`, `Command`, `Reply`.
- Produces (`ludenea_cli`):
  - `pub fn main(app: fn() -> App) -> std::process::ExitCode` (wraps `run` with `std::env::args` and stdout).
  - `pub fn run<S: AsRef<str>>(app: fn() -> App, args: &[S], out: &mut dyn std::io::Write) -> i32` (the first element of `args` is the program name).
  - `#[derive(Serialize, Deserialize, PartialEq, Debug)] pub struct Trace { pub version: u32, pub scene_hash: u64, pub inputs_hash: u64, pub hashes: Vec<u64> }`, `TRACE_VERSION = 1`, postcard encoding (about 10 bytes per tick with varints). `hashes[0]` = hash after loading the scene, `hashes[k]` = hash after `k` ticks (length `ticks + 1`). `scene_hash` and `inputs_hash` = xxh3 of the raw file bytes (0 without an input file).
  - Every command goes through `InProcess`. Output files create missing parent directories (`create_dir_all`).
  - Commands (clap derive), exact output lines:
    - `run <scene> [--inputs <file>] --ticks <n> [--threads <n>] [--hash] [--trace <file>]`; `--hash` prints `tick {n} hash {:016x}`; exit 0.
    - `verify <scene> --inputs <file> --golden <file> [--threads <n>] [--dump-dir <dir>]` (default dump dir `dump`): when all hashes match, prints `ok: {count} hashes match`, exit 0. When the scene or input hash differs from the golden, prints `golden trace was recorded for a different scene or input log`, exit 2. On the first mismatch at tick `t`, prints `mismatch at tick {t}: expected {:016x}, got {:016x}`, writes `{dir}/tick-{t-1}.snap` and `{dir}/tick-{t}.snap` (only `tick-0.snap` when `t == 0`; a rolling previous snapshot is kept while running), prints `wrote {paths joined by " and "}`, exit 1.
    - `diff <snapshot-a> <snapshot-b>` (through `Command::DiffSnapshots`): prints one difference per line, exit 1; or `snapshots are identical`, exit 0.
    - Any file or protocol error prints `error: {message}`, exit 2.

- [ ] **Step 1: Write the failing tests** (tiny test app with RNG and a counter component; each test uses its own directory under `std::env::temp_dir()` named after the test and the process id): `run_prints_final_hash`, `run_then_verify_passes`, `verify_against_tampered_golden_reports_tick_and_writes_dumps` (flip `hashes[7]`: output contains `mismatch at tick 7`, both dump files exist), `verify_mismatch_at_tick_zero_writes_one_dump`, `verify_with_other_scene_reports_different_scene`, `diff_of_dumps_names_entity_component_and_field` (two scenes differing in one field), `diff_of_identical_snapshots`, `missing_file_prints_error_and_exits_2`, `trace_parent_directory_is_created`.
- [ ] **Step 2: Run** `cargo test -p ludenea-cli`. Expected: compile failure.
- [ ] **Step 3: Implement** the CLI on top of `InProcess`.
- [ ] **Step 4: Run** `cargo test -p ludenea-cli && cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(cli): add run, verify, and diff commands with hash traces`.

---

### Task 16: Reference scenario, golden trace, WebAssembly run

**Files:**
- Create: `examples/boxes/{Cargo.toml, src/lib.rs, src/scene.rs, src/main.rs, src/bin/generate.rs}`, `examples/boxes/assets/{boxes.scene.ron, boxes.inputs}` (generated), `golden/boxes.trace` (generated)
- Modify: `.claude/superpowers/specs/2026-10-17-ludenea-engine-design.md` section 7.3 example (add `Serialize, Deserialize` derives to `Health`, use `hit.a` / `hit.b` instead of `hit.target`, `#[query(filter = ...)]` on the struct) and section 10.2 (trace size about 10 bytes per tick)
- Test: `examples/boxes/tests/golden.rs`

**Interfaces:**
- Consumes: the facade crate `ludenea` (`ludenea::cli`, `ludenea::control`, prelude) plus a direct `serde` dependency with `derive` (serde derives reference the `serde` crate by name).
- Produces (`boxes`), `Cargo.toml` with `default-run = "boxes"`:
  - `pub fn app() -> App` (`PhysicsPlugin` + `BoxesPlugin`; `BoxesPlugin` calls `app.input::<BoxesInput>()`).
  - `#[derive(Serialize, Deserialize, Default, Clone)] pub struct BoxesInput { pub velocity_change: Option<Vec2>, pub spawn: Option<f32>, pub despawn: bool }`.
  - Component `Crate` (marker), resource `ContactStats { pub begins: u64, pub ends: u64 }`.
  - Systems: `apply_inputs` (`.before(physics::Step)`, reads `Runtime<Inputs<BoxesInput>>`): for player 0, `velocity_change` applies `apply_impulse(mass * change)` to one crate chosen with `Rng::range_u32` over crates in query order; `spawn: Some(x)` spawns a crate at `(x, 26)` whose shape (circle, box, or capsule) and size come from `Rng`; `despawn` removes one `Rng`-chosen crate. No `Rng` draw happens when there are no crates. `count_contacts` (`.after(physics::Step)`) adds `Begin` and `End` events into `ContactStats`.
  - `pub fn generate_scene(crates: usize, seed: u64) -> String`: closed static container (ground at y = 0, ceiling at y = 30, walls at x = ±20, all boxes of half thickness 0.5), then `crates` bodies on a 20-column grid with spacing 1.4 and jitter up to 0.1, starting at y = 2 (300 crates fill 15 rows, top near y = 23), shapes cycling circle, box, capsule, sizes 0.3 to 0.6, friction 0.2 to 0.9, restitution 0 to 0.4, all from `Rng::new(seed, 1)`. Built through `Simulation::save_scene`.
  - `src/bin/generate.rs`: writes `concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.scene.ron")` (`generate_scene(300, 7)`) and `.../assets/boxes.inputs` (10,000 ticks, 1 player: `velocity_change: Some((1, 4))` every 120 ticks, `spawn: Some(((t / 500) % 30) as f32 - 15.0)` every 500 ticks, `despawn: true` every 700 ticks), creating `assets/` if missing.

- [ ] **Step 1: Implement the example crate and generate the assets:** `cargo run --locked -p boxes --bin generate`. Expected: both files in `examples/boxes/assets/`.
- [ ] **Step 2: Record the golden trace on macOS arm64 in release:** `cargo run --locked --release -p boxes -- run examples/boxes/assets/boxes.scene.ron --inputs examples/boxes/assets/boxes.inputs --ticks 10000 --trace golden/boxes.trace --hash`. Expected: a `tick 10000 hash ...` line and the trace file.
- [ ] **Step 3: Write the tests** in `tests/golden.rs` (debug profile, which also proves debug and release agree). Paths come from `env!("CARGO_MANIFEST_DIR")` (`assets/...`, `../../golden/boxes.trace`); each verify uses its own `--dump-dir` under `std::env::temp_dir()`:
  - `golden_matches_with_1_thread`, `golden_matches_with_2_threads`, `golden_matches_with_8_threads`: each calls `ludenea::cli::run(boxes::app, &["boxes", "verify", ...], &mut Vec::new())` and asserts exit 0.
  - `snapshot_at_5000_restore_replays_identically`: through `InProcess`, step 5,000, take snapshot, step 5,000 with hashes A, restore, step 5,000 with hashes B; `A == B` and `A` equals the golden tail.
  - `scenario_exercises_spawn_despawn_and_contacts`: after 10,000 ticks, `ContactStats.begins > 0`, `ends > 0`, crate count differs from 300, and every crate is still inside the container.
- [ ] **Step 4: Run** `cargo test --locked -p boxes`. Expected: PASS.
- [ ] **Step 5: Run the WebAssembly check:** `cargo build --locked --release --target wasm32-wasip1 -p boxes --bin boxes && wasmtime run --dir=. target/wasm32-wasip1/release/boxes.wasm verify examples/boxes/assets/boxes.scene.ron --inputs examples/boxes/assets/boxes.inputs --golden golden/boxes.trace`. Expected: `ok: 10001 hashes match`. On mismatch, stop and debug with `--dump-dir` and `diff` (systematic-debugging skill); never re-record the golden to make a platform pass.
- [ ] **Step 6: Update the spec** sections 7.3 and 10.2 as listed under Files.
- [ ] **Step 7: Commit** `✅ test(boxes): add reference scenario with cross-platform golden trace`.

---

### Task 17: Continuous integration, benchmark, README

**Files:**
- Create: `.github/workflows/determinism.yml`, `examples/boxes/src/bin/bench.rs`, `README.md`

**Interfaces:**
- Consumes: Task 16 `generate_scene`, the `boxes` binary.

- [ ] **Step 1: Write the workflow.**
  - Job `native`, matrix `os: [macos-14, ubuntu-24.04, windows-2025]`: checkout, toolchain from `rust-toolchain.toml`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, then for `threads` in 1, 2, 8: `cargo run --locked --release -p boxes -- verify examples/boxes/assets/boxes.scene.ron --inputs examples/boxes/assets/boxes.inputs --golden golden/boxes.trace --threads $threads`. On failure, upload `dump/` as an artifact.
  - Job `wasm` on `ubuntu-24.04`: toolchain, `bytecodealliance/actions/wasmtime/setup@v1`, then the two commands of Task 16 Step 5.
- [ ] **Step 2: Write `bench.rs`.** Builds `generate_scene(1000, 11)` in memory, warms 120 ticks, measures 600 ticks with 1 and 8 threads, plus 100 snapshots and 100 restores. Prints `ticks: {ms:.3} ms/tick (1 thread), {ms:.3} ms/tick (8 threads); snapshot {ms:.3} ms; restore {ms:.3} ms`, compared against the spec targets (2 ms on 8 threads, 6 ms on 1 thread, 0.5 ms snapshot and restore).
- [ ] **Step 3: Run** `cargo run --locked --release -p boxes --bin bench`. Expected: the line above. Report the numbers; missing a target is reported, not hidden, and does not block the milestone.
- [ ] **Step 4: Write `README.md`**: what Ludenea is (one paragraph), the determinism guarantee, the workspace layout, and the commands to run tests, the golden check, the WebAssembly check, and the benchmark.
- [ ] **Step 5: Run** `cargo clippy --locked --workspace --all-targets -- -D warnings && cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 6: Commit** `👷 ci: verify golden traces on macOS, Linux, Windows, and WebAssembly`.
