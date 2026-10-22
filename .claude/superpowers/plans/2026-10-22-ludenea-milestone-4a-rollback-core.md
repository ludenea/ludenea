# Ludenea Milestone 4a: Rollback Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A rollback session that keeps two to four peers in sync under latency, loss, duplication and reordering, proven in one process, on top of a fast in-memory save and restore of the simulation, with SyncTest catching non-deterministic game code from the command line and naming the tick, entity, component and field.

**Architecture:** `Component` and `Resource` require `Clone`, so `ludenea-ecs` can save a world into a reusable `SavedWorld` and load it back without serializing; `Simulation` wraps it as `SavedState`. A new crate `ludenea-net` holds the sans-IO `Session` (input queues, prediction, input delay, rollback window, time synchronization, hash reports, the end of a match), its versioned wire format, the `Transport` trait with an in-memory network and a seeded link conditioner, an `Executor` that applies the session's requests to a `Simulation`, and `sync_test`. The per-entity half of the editor's divergence report moves from `ludenea-control` to `ludenea-app` so SyncTest reports in the same shape. Each game binary gains `synctest`. Nothing in the control protocol, the host, the runtime or the editor changes behaviour in this milestone.

**Tech Stack:** unchanged (Rust 1.97.1, serde, postcard, clap). No new external dependency: the link conditioner draws from the existing `ludenea_app::Rng`.

**Spec:** `.claude/superpowers/specs/2026-10-22-ludenea-networking-4-design.md`. This plan covers spec sections 4.1, 4.2, the in-memory and conditioner rows of 4.3, the detection half of 5 (hash reports, the retention of reported states, the mismatch event, SyncTest with the divergence report), 6 at the session level (the agreed last tick), 7.4 at the session level, and the session and benchmark parts of 8. Milestone 4b covers WebRTC, signaling, the host integration (4.4, 4.5), the native and web runtimes and the headless peer. Milestone 4c covers the editor, the exchange of hash trees and snapshots that turns a desync between peers into the full report, SyncTest through the host and the editor, the end-to-end cross-play test and the online golden.

## Global Constraints

- Every Global Constraint of the earlier milestone and quality-bar plans still applies (determinism lints, target gates, committed generated files, `--locked`, commit style).
- The state hashes of `golden/` never move, and no golden file changes in this milestone.
- `ludenea-net` reads no clock and uses no unordered collection: the caller passes the current time as `now_micros: u64` (monotonic microseconds; a value lower than the last one is treated as the last one). It uses no floating point at all, enforced by `#![deny(clippy::float_arithmetic)]` in its `lib.rs`: ping, jitter and frame advantage are integers. The spec allows floating point for statistics; integers are stricter and enough.
- Spec defaults: `MatchRules::default()` is `input_delay = 2` ticks, `rollback_window = 8` ticks, `hash_interval = 30` confirmed ticks; `Timeouts::default()` is `notice_after_micros = 750_000`, `disconnect_after_micros = 3_000_000`.
- Limits (builder's call): at most `MAX_PEERS = 4` peers and `MAX_PLAYERS = 4` players; `MAX_INPUT_BYTES = 32` per player per tick (the example games' inputs take a few bytes); `MAX_PACKET_BYTES = 1200` (a safe WebRTC payload); `MAX_TICKS_PER_MESSAGE = 32` bounds what a decoder accepts in one input message; `MAX_MESSAGES_PER_PACKET = 16`. A sender packs a packet by size, oldest ticks first, and stops before `MAX_PACKET_BYTES`; `SessionSettings::validate` encodes the worst-case packet of its own layout (every local player at `MAX_INPUT_BYTES` for `rollback_window + input_delay + 1` ticks, plus the acknowledgment, quality and hash messages) and refuses settings whose worst case does not fit, naming both sizes.
- New format: the network wire format, `NET_PROTOCOL_VERSION: u16 = 1`, added to the CHANGELOG formats table (current 1, oldest readable 1) in the task that declares it. Its two-byte little-endian prefix is frozen for every future version, so any version can name the version it received. `PROTOCOL_VERSION` and every other format constant stay unchanged.
- Anything decoded from a packet is untrusted. Every arithmetic on a received field is checked (`checked_*` or a range test before the operation; `overflow-checks` is on in release, so an unchecked overflow is a panic); queue positions are relative (`usize::try_from(tick - base)`), never `tick as usize`. An invalid packet is dropped, counted under a `DropReason` in `SessionStats`, and reported as `SessionEvent::Dropped { peer, reason }` for the host to log; a packet that proves the peer is broken or hostile disconnects that peer with `EndReason::Violation` naming the peer, the player and tick when there is one, and the `ViolationRule`.
- Control messages are never sent once: every packet a session sends carries what the receiver has not acknowledged yet (inputs, hash reports, a goodbye, an end notice), so loss delays them and never loses them.
- `ludenea-net` builds for `wasm32-unknown-unknown` and `wasm32-wasip1`; its tests run under `wasmtime` in CI (`CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime run --dir=."`). A test that needs to catch a panic lives in a file starting `#![cfg(not(target_family = "wasm"))]`, as `crates/ludenea-cli/tests/render.rs` does: WebAssembly aborts on panic.
- Every CI lint step has its mirror in `CHECKS` of `xtask/src/main.rs`, in the same order, so `cargo xtask check` stays the local replay of CI.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- `Asset` does not gain `Clone`. **This overturns an author decision of spec section 4.2 and needs the author's approval before Task 1 starts; on approval, Task 1 also updates that paragraph of the spec.** Loaded assets are stored as `Arc<dyn Any + Send + Sync>` (`crates/ludenea-ecs/src/assets.rs`) and only ever replaced, never written in place, so a saved world shares them instead of cloning them, and copies the asset map only when it changed since the last save. The breaking change covers `Component` and `Resource` only, which is all the saved state needs.
- The derive macros do not change. `#[derive(Component)]` cannot add `Clone` without conflicting with a `#[derive(Clone)]` the game writes itself; the compiler's error for a missing supertrait already names the `Clone` bound, and the project template already derives it.
- SyncTest is reachable from the game's command line and the `ludenea-net` API in this milestone; through the host and the editor in 4c.
- On a hash mismatch between peers, 4a ends the session with `EndReason::Desync` carrying the tick, the peer and both hashes, on every peer through the end notice. Until 4c exchanges hash trees and snapshots, a desync between peers says only that the hashes differ, which the anti-goal "a desync report that only says hash differs" forbids for a shipped feature: online play is not reachable by a game before 4b and 4c, and 4a already keeps the saved states of reported ticks (spec section 5) so 4c can build the report from them. SyncTest, where both states live in one process, already gives the full report.

## Review Focus

- **A hostile or broken packet** (truncated, oversized, a version other than `NET_PROTOCOL_VERSION`, a wrong match identifier, a player index the sender does not own, an input tick beyond the legitimate horizon, more than `MAX_TICKS_PER_MESSAGE` ticks, an acknowledgment of ticks never sent, a quality reply that answers no pending request, a hash for a tick no honest peer could have confirmed, fields near `u64::MAX`): dropped and counted by reason, or the peer disconnected with a named `ViolationRule`; never a panic, never an overflow. Tests in Tasks 5 and 10, fuzzing of the decoder in Task 5 and of `Session::receive` in Task 10.
- **A peer that sends a different input for a tick it already sent**: a violation that disconnects that peer naming the player and the tick; the confirmed input is never overwritten. Test in Task 10.
- **Remote input bytes that are not the canonical encoding of one input of the game's type**: refused when received, through the check the session is built with, naming the player and the tick, before they reach a step. Test in Task 10.
- **A saved state loaded into another simulation** (another game build, or different registrations): `StateError::OtherWorld` naming both fingerprints; the simulation is unchanged. Test in Task 2.
- **Game code that panics while the executor steps a tick**: the executor restores the state of that tick from its ring, tells the session through `Session::fail`, which ends with `EndReason::LocalFailure` and notifies the other peers, and returns `ExecuteError::Game` naming the tick. Test in Task 14.
- **A match that ends on purpose under loss**: every remaining peer stops at the same tick and holds the same confirmed inputs. Test in Task 13.

---

### Task 1: `Component` and `Resource` require `Clone`

**Files:**
- Modify: `crates/ludenea-ecs/src/component.rs` (bounds and their doc comments), `crates/ludenea-ecs/src/query.rs` (its doc-test types derive `Clone`, so its `compile_fail` blocks keep failing for their own reason), every `#[derive(Component)]` and `#[derive(Resource)]` type in `crates/`, `examples/` and their tests that lacks `Clone` (85 sites, all in tests and doc examples; the hand-serialized `Fuse`, `Gauge` and `Touchy` test types derive it like the others), `CHANGELOG.md` (Rust API: the breaking change and its migration), and, once the author approves the `Asset` deviation, the `Clone` paragraph of spec section 4.2.
- Test: a `compile_fail,E0277` doc test on `Component`.

**Interfaces:**
- Produces: `pub trait Component: Schema + Serialize + DeserializeOwned + Clone + Send + 'static` and the same `Clone` bound on `Resource`. The doc comment of each says a hand-written `Clone` must give the same value a postcard round trip gives: the saved state relies on it. Later tasks clone storages through these bounds.

- [ ] **Step 1: Write the doc test**: a `compile_fail,E0277` block on `Component` with a struct that derives `Component`, `Serialize`, `Deserialize` and `Schema`, everything except `Clone`, so the missing `Clone` is the only error.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs --doc component`. Expected: FAIL (the block compiles).
- [ ] **Step 3: Add the bounds** and derive `Clone` on every type the compiler names, `query.rs`'s doc-test types included. The project template (`crates/ludenea-project/template/src/lib.rs`) already derives it; check it still does.
- [ ] **Step 4: Run** `cargo test --locked --workspace` and the clippy gates. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `💥 feat(ecs): require Clone on components and resources`.

---

### Task 2: Save and load a world in memory

**Files:**
- Create: `crates/ludenea-ecs/src/saved.rs` (with a unit test module)
- Modify: `crates/ludenea-ecs/src/world.rs` (`Storage` and `ResourceCell` gain clone methods; the world counts asset changes), `crates/ludenea-ecs/src/assets.rs` (`LoadedAsset` cheap to clone: `bytes` behind `Arc<[u8]>`), `crates/ludenea-ecs/src/lib.rs`, `crates/ludenea-app/src/simulation.rs`, `CHANGELOG.md`
- Test: `crates/ludenea-ecs/tests/saved.rs`, `crates/ludenea-app/tests/saved_state.rs`, `examples/boxes/tests/saved_state.rs`, `examples/arena/tests/saved_state.rs` (each example's own scene, in its own crate)

**Interfaces:**
- Produces in `ludenea_ecs`:
  - `SavedWorld` (`Default`), `World::save_into(&self, into: &mut SavedWorld)`, `World::load(&mut self, saved: &SavedWorld) -> Result<(), SavedError>`.
  - `save_into` reuses the allocations already in `into` (`Vec::clone_from` per storage) and copies the asset map only when the world's asset change counter differs from the one recorded in `into`.
  - A `SavedWorld` holds what `Snapshot` holds: the entity allocator, every component storage, every resource, the loaded assets (shared `Arc`s).
  - `load` is all or nothing: it checks every storage and resource of `saved` against the world before it changes anything. It drops queued commands and events and keeps runtime resources, like `World::restore`.
  - `SavedError::{ Empty, MissingStorage { name }, UnexpectedStorage { name }, TypeMismatch { name } }`, each naming the stable type name; a downcast that fails is `TypeMismatch`, never a panic.
- Produces in `ludenea_app`: `SavedState` (`Default`), `Simulation::save_state(&self, into: &mut SavedState)`, `Simulation::load_state(&mut self, saved: &SavedState) -> Result<(), StateError>`; `StateError::OtherWorld { saved: u64, current: u64 }` (schema fingerprints, formatted as 16 hex digits), `StateError::Empty` for a `SavedState` never saved into, `StateError::World(SavedError)`. A `SavedState` is stamped with the fingerprint the simulation already caches (`self.initial.fingerprint`, not a recomputation per save); `load_state` checks it before touching the world.

- [ ] **Step 1: Write the failing tests**:
  - `saved_world_round_trips_snapshot` (`tests/saved.rs`): a property test over random worlds, 256 cases from a small seeded generator written in the test (`ludenea-ecs` cannot depend on `ludenea_app::Rng`): spawn, despawn, insert and remove across several component types, resources, assets; `save_into`, mutate randomly, `load`, then `world.snapshot().encode()` equals the encoding taken at the save, byte for byte.
  - `save_into_reuses_allocations` (unit test in `src/saved.rs`, where the storages are visible): saving twice into the same `SavedWorld` with the same shape keeps each storage's buffer pointer; an unchanged asset map is not copied again.
  - `load_keeps_runtime_resources_and_drops_commands`.
  - `load_refuses` one case per `SavedError` variant, the world unchanged after each.
  - `load_state_refuses_another_game`: a `SavedState` from an app with one more component type: `StateError::OtherWorld`, and `snapshot()` unchanged.
  - `load_state_refuses_empty`.
  - `saved_state_matches_snapshot` on boxes and on the arena: run 300 ticks of the example scene, saving every tick; load tick 150 and resimulate to 300: at every tick the encoded snapshot equals the straight run's, at 1 and 8 threads.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs --test saved`, `cargo test -p ludenea-ecs saved`, `cargo test -p ludenea-app --test saved_state`, `cargo test -p boxes --test saved_state` and `cargo test -p arena --test saved_state`. Expected: compile failure.
- [ ] **Step 3: Implement** `Storage::clone_into(&self, into: &mut Option<Box<dyn Storage + Send>>)`, `Storage::check_saved(&self, saved: &dyn Storage) -> Result<(), SavedError>` and `Storage::load_from(&mut self, saved: &dyn Storage)` (and the same on `ResourceCell`), downcasting through `as_any`; `World::load` calls every check first, then every load. `SavedWorld` keys storages by stable name in `BTreeMap`s, like `World`.
- [ ] **Step 4: Run** `cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): save and load the simulation state in memory`.

---

### Task 3: Rollback benchmark on the arena

**Files:**
- Create: `examples/arena/src/bin/rollback-bench.rs`
- Modify: `README.md` (the command, next to the boxes benchmark), `.github/workflows/determinism.yml` (a step on the Linux job that runs it in release and appends its output to the job summary; it records, it does not gate)

**Interfaces:**
- Consumes: `Simulation::save_state`, `Simulation::load_state` (Task 2).

- [ ] **Step 1: Write** the binary. It loads `golden/arena.replay` (600 ticks) and plays it straight, saving the state of every tick into a ring of 10 `SavedState`s. At every tick `t` from 8 to 599, it measures one worst-case rollback frame: `load_state` of tick `t - 8`, then 9 `play_tick` calls with a `save_state` after each (the spec's W+1 ticks), ending at tick `t + 1`. That is 592 measured frames. It prints the frame count, the mean, the 99th percentile and the worst frame in microseconds at 1 thread, and whether the worst frame fits in half a 60 Hz frame (8,333 microseconds). The bench is display code: timing uses `std::time::Instant` with the `#[expect(clippy::disallowed_methods, reason = "benchmark timing, never feeds the simulation")]` pattern the boxes bench uses.
- [ ] **Step 2: Run** `cargo run --locked --release -p arena --bin rollback-bench`. Expected: `592 frames` and the measures; record the numbers in the commit message body.
- [ ] **Step 3: Commit** `✨ feat(arena): add a rollback frame benchmark`. If the worst frame misses the target, stop and report the numbers before the next task: the save path needs work before the session is built on it.

---

### Task 4: `ludenea-net` crate and validated session settings

**Files:**
- Create: `crates/ludenea-net/{Cargo.toml, src/lib.rs, src/settings.rs}`
- Modify: root `Cargo.toml` (workspace member and `ludenea-net = { path = "crates/ludenea-net" }` in `[workspace.dependencies]`), `crates/ludenea/Cargo.toml` and `crates/ludenea/src/lib.rs` (re-export `ludenea::net`), `Cargo.lock`, `.github/workflows/determinism.yml` and `xtask/src/main.rs` `CHECKS` (both: add `-p ludenea-net` to the existing `wasm32-unknown-unknown` `--lib` clippy line; a new line `clippy --locked --target wasm32-wasip1 -p ludenea-net --all-targets -- -D warnings` after the existing wasip1 lines), the `wasm` job of `determinism.yml` (a step `cargo test --locked --target wasm32-wasip1 -p ludenea-net` with `CARGO_TARGET_WASM32_WASIP1_RUNNER: wasmtime run --dir=.` after the wasmtime setup)
- Test: `crates/ludenea-net/tests/settings.rs`

**Interfaces:**
- Produces:
  - `PeerId(pub u8)`.
  - `MatchRules { input_delay: u32, rollback_window: u32, hash_interval: u32 }` with `Default` giving the spec values.
  - `MatchIdentity { engine: String, fingerprint: u64, scene_and_assets: u64, tick_rate: u32, players: Vec<PeerId>, rules: MatchRules }`: everything the peers must agree on; `players` gives the owner of each global player index, in order.
  - `Timeouts { notice_after_micros: u64, disconnect_after_micros: u64 }` with `Default` giving the spec values.
  - `SessionSettings { local: PeerId, match_id: u64, identity: MatchIdentity, timeouts: Timeouts }`.
  - `SessionSettings::validate(&self) -> Result<(), SettingsError>`.
  - Constants `MAX_PEERS`, `MAX_PLAYERS`, `MAX_INPUT_BYTES`, `MAX_PACKET_BYTES`, `MAX_TICKS_PER_MESSAGE`, `MAX_MESSAGES_PER_PACKET` (values in Global Constraints). `NET_PROTOCOL_VERSION` is declared in Task 5 with the format it versions.
- Validation, checked in this order so no check overflows: `players` not empty and at most `MAX_PLAYERS`; the owners are exactly the peers `0..n` with `n <= MAX_PEERS`, each owning at least one player (spectators are out of scope; `[0, 2]` is refused); `local < n`; `tick_rate > 0`; `rollback_window >= 1`; `rollback_window + input_delay <= MAX_TICKS_PER_MESSAGE` (written as an addition, never a subtraction); `hash_interval >= 1`; `0 < notice_after_micros < disconnect_after_micros`. The packet worst-case check of Global Constraints joins in Task 5, once the wire format exists.

- [ ] **Step 1: Write the failing tests**: `defaults_are_the_spec_values`; `default_rules_validate` (a two-peer identity with the default rules and timeouts passes); `validate_refuses` one case per rule above, `input_delay = u32::MAX` included; each error message names the field and the value.
- [ ] **Step 2: Run** `cargo test -p ludenea-net`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate (depends on `ludenea-app`, `ludenea-ecs`, `serde`, `postcard`, `thiserror`, all workspace dependencies), with `#![deny(clippy::float_arithmetic)]`.
- [ ] **Step 4: Run** the workspace gates, `cargo xtask check`, and `CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime run --dir=." cargo test --locked --target wasm32-wasip1 -p ludenea-net`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): add the network crate with validated session settings`.

---

### Task 5: Wire format

**Files:**
- Create: `crates/ludenea-net/src/wire.rs`
- Modify: `crates/ludenea-net/src/settings.rs` (the worst-case packet check), `CHANGELOG.md` (formats table row `Network protocol | NET_PROTOCOL_VERSION | 1 | 1`, and a Formats entry describing it), `crates/ludenea/tests/formats_documented.rs` (the entry `("NET_PROTOCOL_VERSION", u32::from(ludenea::net::NET_PROTOCOL_VERSION))`)
- Test: `crates/ludenea-net/tests/wire.rs`

**Interfaces:**
- Produces: `pub const NET_PROTOCOL_VERSION: u16 = 1`; `Packet { match_id: u64, messages: Vec<Message> }`, `Packet::encode(&self) -> Vec<u8>`, `Packet::decode(bytes: &[u8]) -> Result<Packet, WireError>`, `Packet::version_of(bytes: &[u8]) -> Option<u16>` (reads the frozen prefix only). The encoding is `NET_PROTOCOL_VERSION` as a little-endian `u16`, then postcard. `Message`:
  - `Hello { identity: MatchIdentity }` and `Welcome` (handshake; the version is the packet prefix, not a field);
  - `Inputs { player: u8, first_tick: u64, inputs: Vec<Vec<u8>> }` (consecutive ticks from `first_tick`, at least one);
  - `Ack { players: Vec<(u8, u64)>, hashes_through: u64 }` (for each remote player of the receiver, the next tick the sender still needs; the highest hash tick received from the receiver, `0` for none);
  - `Quality { tick: u64, sent_micros: u64, advantage: i32 }` and `QualityReply { echo_micros: u64, tick: u64 }`;
  - `Hash { tick: u64, hash: u64 }`;
  - `Goodbye { last_tick: u64 }`;
  - `End { reason: EndNotice }`, `EndNotice::{ Desync { tick: u64, peer: PeerId }, Lost { peer: PeerId }, LocalFailure { tick: u64 } }` (an ending one peer decided, relayed so every peer ends).
- `WireError` variants: `TooLarge { len }`, `Version { found, expected }`, `Malformed(String)`, `TooManyMessages { count }`, `TooManyTicks { count }`, `NoTicks`, `InputTooLarge { len }`, `TooManyPlayers { count }`.

- [ ] **Step 1: Write the failing tests**:
  - `every_message_round_trips`.
  - `decode_refuses` one case per `WireError` variant, and trailing bytes (`Malformed`).
  - `version_of_reads_any_version`: a packet with prefix `7` decodes as `Version { found: 7, expected: 1 }` and `version_of` returns `Some(7)`.
  - `worst_case_packet_fits` for the default rules at 1, 2 and 3 local players, and `validate_refuses_a_layout_whose_worst_packet_does_not_fit` (for example 3 local players with `rollback_window = 24`), naming both sizes.
  - `fuzz_decode_never_panics`: the harness draws from `ludenea_app::Rng::new(seed, 0)` for seeds `0..100_000`: valid packets of every message encoded then mutated (random bit flips, truncation at a random length, a random varint length field set to `u32::MAX` or `u64::MAX`, random bytes appended), plus purely random byte strings of length `0..=1300`. It asserts only that `decode` returns, and that an `Ok` packet encodes back within `MAX_PACKET_BYTES`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test wire` and `cargo test -p ludenea --test formats_documented`. Expected: compile failure.
- [ ] **Step 3: Implement**: check the length before decoding; postcard through `ludenea_ecs::decode_exact` (which refuses trailing bytes); check every vector length after decoding; move the worst-case encoding into `SessionSettings::validate`.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): define the versioned network wire format`.

---

### Task 6: Transports: in-memory network and link conditioner

**Files:**
- Create: `crates/ludenea-net/src/transport.rs`
- Test: `crates/ludenea-net/tests/transport.rs`

**Interfaces:**
- Produces:
  - `trait Transport { fn send(&mut self, to: PeerId, packet: Vec<u8>, now_micros: u64) -> Result<(), TransportError>; fn receive(&mut self, now_micros: u64) -> Option<(PeerId, Vec<u8>)>; }`. `send` is fallible now so the WebRTC transport of 4b fits without breaking the trait; `TransportError::{ UnknownPeer { peer }, TooLarge { len } }`.
  - `MemoryNetwork::new(peers: u8) -> Vec<MemoryTransport>` (shared queues, single-threaded, in send order).
  - `LinkConditions { latency_micros: u32, jitter_micros: u32, loss_per_mille: u16, duplicate_per_mille: u16, reorder_per_mille: u16, reorder_extra_micros: u32, seed: u64 }` (integers only; 5% loss is `50`), validated by `Conditioned::new` (each per-mille value at most 1000, `jitter_micros < u32::MAX`).
  - `Conditioned<T: Transport>::new(inner: T, conditions: LinkConditions) -> Result<Self, ConditionsError>`. Each destination draws from its own `ludenea_app::Rng::new(seed, u64::from(to.0))`, so links are not correlated. At `send`, a packet is lost per mille, or gets the due time `now + latency + range_u32(0, jitter + 1)`, plus `reorder_extra_micros` per mille; a duplicate is a second copy with its own draw. `receive(now)` first passes every packet due at or before `now` to the inner transport, in due-time order, then receives from the inner transport.

- [ ] **Step 1: Write the failing tests**: `memory_delivers_in_order`; `memory_refuses_an_unknown_peer`; `conditioned_is_deterministic_for_a_seed` (same seed, same schedule of sends and times, same deliveries); `conditioned_loss_rate_is_close` (100,000 packets at 50 per mille: between 4.5% and 5.5% lost); `conditioned_delays_by_latency` (a packet sent at `now` is not delivered at `now + latency - 1` and is at `now + latency`); `jitter_reorders`; `reorder_reorders_without_jitter`; `duplicates_arrive_twice`; `links_are_independent` (two destinations with the same seed do not lose the same packets).
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test transport`. Expected: compile failure.
- [ ] **Step 3: Implement**: pending packets in a `BTreeMap<(u64 due, u64 sequence), (PeerId, Vec<u8>)>`.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): add an in-memory network and a seeded link conditioner`.

---

### Task 7: The input queue

**Files:**
- Create: `crates/ludenea-net/src/queue.rs` (crate-private, with a unit test module)

**Interfaces:**
- Produces (crate-private): `InputQueue::new(default_input: Vec<u8>)`, holding for one player the confirmed inputs from a base tick (a `VecDeque`, indexed relative to the base), the last confirmed input, and the input each simulated tick last ran with.
  - `confirm(&mut self, tick: u64, input: Vec<u8>) -> Result<Confirmed, QueueError>`: `Confirmed::{ New, Same }` for a repeat of an equal input; `QueueError::Conflict { tick }` for a different input at a confirmed tick; `QueueError::Gap { next }` for a tick after the next expected one.
  - `next_confirmed(&self) -> u64` (the first tick without a confirmed input).
  - `input_for(&self, tick: u64) -> Vec<u8>`: the confirmed input, or the prediction (the last confirmed input, or `default_input` before any).
  - `record_used(&mut self, tick: u64, input: &[u8])` when a tick is simulated.
  - `first_misprediction(&self, below: u64) -> Option<u64>`: the first tick below `below` whose used input differs from its confirmed one. Ticks not simulated yet (at or after `below`) never count.
  - `release_before(&mut self, tick: u64)` drops what no rollback can need anymore.

- [ ] **Step 1: Write the failing unit tests**: confirm in order, repeat (`Same`), conflict, gap; prediction repeats the last confirmed input; `first_misprediction` compares with the input last used, so a tick re-predicted after a correction is compared with the new prediction; an input confirmed for a future, unsimulated tick is not a misprediction; `release_before` keeps positions relative so ticks near `u64::MAX` never overflow.
- [ ] **Step 2: Run** `cargo test -p ludenea-net queue`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): queue confirmed and predicted inputs per player`.

---

### Task 8: The session: handshake

**Files:**
- Create: `crates/ludenea-net/src/session.rs`, `crates/ludenea-net/tests/support/mod.rs` (the harness: N sessions over `MemoryNetwork`, optionally behind `Conditioned`, driven in steps of 1 ms of `now_micros`, delivering every received packet before each `advance`, one `advance` per frame at the tick rate; and a model game that executes requests by folding inputs into a running hash per tick, with saves and loads of that hash)
- Modify: `crates/ludenea-app/src/{input.rs, simulation.rs, lib.rs}` (`PlayerInputSpec`)
- Test: `crates/ludenea-net/tests/handshake.rs`, `crates/ludenea-app/tests/player_input.rs`

**Interfaces:**
- Consumes: `SessionSettings`, `Packet`, `Message` (Tasks 4, 5).
- Produces:
  - `PlayerInputSpec { default: Vec<u8>, check: fn(&[u8]) -> Result<(), SimError> }` is consumed from `ludenea_app` (added in this task to `crates/ludenea-app/src/{input.rs, simulation.rs}`): `Simulation::player_input_spec(&self) -> Option<PlayerInputSpec>`, `None` for an app without an input type. `check` accepts only the canonical postcard encoding of exactly one input: it decodes, re-encodes and compares the bytes, so two peers never record different bytes for one value. `InputType` gains the matching `encode_default_one` and `check_one` functions.
  - `Session::new(settings: SessionSettings, input: PlayerInputSpec) -> Result<Session, SettingsError>`: validates the settings, and that `input.default` is at most `MAX_INPUT_BYTES` and passes `input.check`.
  - `Session::receive(&mut self, from: PeerId, packet: &[u8], now_micros: u64)`.
  - `Session::outgoing(&mut self, now_micros: u64) -> Vec<(PeerId, Vec<u8>)>`: at most one packet per peer per call, packed by size.
  - `Session::events(&mut self) -> Vec<SessionEvent>`; `Session::state(&self) -> SessionState` (`Connecting`, `Running`, `Ended(EndReason)`); `Session::stats(&self) -> &SessionStats` (`dropped: BTreeMap<DropReason, u64>` per peer, filled further by later tasks).
  - `SessionEvent::{ Started, Dropped { peer: PeerId, reason: DropReason } }`, extended by later tasks.
  - `EndReason::{ Mismatch { peer: PeerId, field: &'static str, local: String, remote: String }, Unreachable { peer: PeerId } }`, extended by later tasks.
  - `DropReason` (an enum, `Ord`): `Wire(&'static str)` named after the `WireError` variant, `WrongMatch`, `UnknownPeer`, `BeforeRunning`, extended by later tasks.
- Rules: while `Connecting`, `outgoing` sends `Hello` to every peer whose `Welcome` it lacks, at most every `HANDSHAKE_RESEND_MICROS = 100_000`. A `Hello` whose identity equals the local one is answered with `Welcome`, in any state, so a lost `Welcome` is answered again. A different identity, or a `Hello` with another `match_id`, ends the session with `Mismatch` naming the first differing field (`engine`, `fingerprint`, `scene_and_assets`, `tick_rate`, `players`, `input_delay`, `rollback_window`, `hash_interval`, `match_id`) and both values. A packet from an expected peer whose prefix is another version (`WireError::Version`, read through `Packet::version_of`) ends the session with `Mismatch { field: "net_version", .. }` while `Connecting`, and is dropped and counted once `Running`. The session is `Running`, and emits `Started`, once it holds a `Welcome` and a matching `Hello` from every peer. With no packet from some peer for `disconnect_after_micros` while `Connecting`, it ends with `Unreachable` naming that peer. `Inputs` before `Running` are dropped as `BeforeRunning`.

- [ ] **Step 1: Write the failing tests**: `handshake_starts_two_three_and_four_peers`; `handshake_survives_loss` (behind `Conditioned` at 300 per mille loss); `handshake_refuses_mismatch` one case per field of the list above, each naming the field and both values; `other_version_names_net_version` (a peer whose packets carry prefix 2); `unreachable_peer_ends_the_handshake`; `wrong_match_id_after_start_is_dropped_and_counted`; `player_input_spec_refuses_non_canonical_bytes` (an overlong varint of a valid value).
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test handshake` and `cargo test -p ludenea-app --test player_input`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates, and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): agree on the match before it starts`.

---

### Task 9: The session: inputs, prediction, rollback and stalls

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`
- Test: `crates/ludenea-net/tests/rollback.rs`

**Interfaces:**
- Produces:
  - `Session::add_local_input(&mut self, player: u8, input: Vec<u8>) -> Result<(), SessionError>`: sets the pending input of a local player for the next tick the session will advance; a second call before that tick advances replaces it, so a caller that adds one input per displayed frame never errs during a stall. Refused: a remote player (`NotLocal`), more than `MAX_INPUT_BYTES` (`InputTooLarge`), bytes `check` refuses (`InvalidInput`), a session not `Running` (`NotRunning`).
  - `Session::advance(&mut self, now_micros: u64) -> Result<Vec<Request>, SessionError>`: at most one new tick per call, after any rollback. `SessionError::MissingLocalInput { player }` when a local player has no pending input and the tick would advance.
  - `Request::{ Save { tick: u64, hash: bool }, Load { tick: u64 }, Advance { tick: u64, inputs: Vec<Vec<u8>> } }` (`hash` is always `false` until Task 12).
  - `SessionEvent::Confirmed { tick: u64, inputs: Vec<Vec<u8>> }`, emitted by `advance` in tick order, once per tick, when every player's input of that tick is confirmed.
  - `SessionStats` per peer gains `rollbacks`, `deepest_rollback`, `prediction_misses`, `stalls`.
- Tick convention: `Advance { tick: t, inputs }` steps the state of tick `t` to `t + 1` with the inputs of tick `t`, one per global player. `Save { tick: t }` saves the state of tick `t`, before its inputs, and always comes right before the first `Advance { tick: t }` after it. The pending local input taken when the session advances tick `t` applies at tick `t + input_delay`; each local player's ticks `0..input_delay` hold `default_input`, sent and confirmed like any other input, so every peer knows them.
- Rules:
  - The confirmed tick is the first tick some player lacks a confirmed input for.
  - `advance` first looks for the earliest misprediction below the current tick across all players; if there is one at tick `m`, it returns `Load { tick: m }`, then `Save` and `Advance` for each tick from `m` up to the current tick, with the inputs now known.
  - It then advances the current tick, unless the current tick is `rollback_window` ticks past the confirmed tick: then it stalls (no new `Advance`, `stalls` counted, the pending inputs kept).
  - Every `advance`, stalled or not, leaves for `outgoing`, for each remote peer, the inputs of every local player from that peer's acknowledged tick on, and an `Ack` for that peer's players. Acknowledgments only move forward: an older one, arriving reordered, is ignored. A packet that leaves a gap after the next expected tick is dropped as `DropReason::Gap`, since a later packet resends from the acknowledged tick.
  - Invariant: a `Load { tick: t }` loads the latest `Save { tick: t }`, and no input below `t` was corrected since that save.

- [ ] **Step 1: Write the failing tests**:
  - `two_peers_without_loss_never_roll_back`: with 0 latency both peers' confirmed inputs and model hashes match tick by tick for 600 ticks.
  - `local_input_applies_after_the_input_delay`, at `input_delay` 0 and 2: the input added when tick `t` advances is in `Advance { tick: t + input_delay }`, and ticks `0..input_delay` carry `default_input`.
  - `late_input_rolls_back_to_the_first_misprediction`: a scripted remote input that differs from the prediction at tick 10, arriving while the local peer is at tick 13, makes the next `advance` return exactly `Load { 10 }, Save { 10 }, Advance { 10 }, Save { 11 }, Advance { 11 }, Save { 12 }, Advance { 12 }, Save { 13 }, Advance { 13 }`, with the real input in each.
  - `correct_prediction_does_not_roll_back`.
  - `future_input_is_not_a_misprediction`: an input for a tick not simulated yet, different from what would be predicted, causes no `Load`.
  - `repredicted_tick_is_compared_with_its_new_prediction`: after a correction, a later misprediction rolls back only to the tick that now differs.
  - `stalls_at_the_window`: with the remote peer silent, `advance` returns no new `Advance` once the current tick is `rollback_window` ticks past the confirmed tick, keeps sending its inputs, and `stats` counts each stall; `add_local_input` during the stall succeeds and replaces the pending input.
  - `advance_without_local_input_is_an_error`.
  - `redundant_inputs_survive_loss`: drop every other packet; all inputs are confirmed anyway.
  - `two_local_players_and_one_remote`: two peers, three players (peer 0 owns players 0 and 1), behind `Conditioned` with loss; every peer confirms the same inputs for all three.
  - `save_precedes_every_load`: over a conditioned run, the invariant above holds for every request.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test rollback`. Expected: compile failure.
- [ ] **Step 3: Implement** around `InputQueue`.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): predict remote inputs and roll back on misprediction`.

---

### Task 10: The session: hostile and broken peers

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`
- Test: `crates/ludenea-net/tests/violations.rs`

**Interfaces:**
- Produces: `EndReason::Violation { peer: PeerId, player: Option<u8>, tick: Option<u64>, rule: ViolationRule }`; `ViolationRule::{ NotOwner, ConflictingInput, InvalidInput, BeyondHorizon, AckNeverSent, AckLagging }`; the matching `DropReason` additions.
- Rules, with their derivation in the doc comment of each check:
  - The local input horizon `h` is the highest tick a local player has an input for (the current tick plus `input_delay`). A peer A cannot hold an input beyond its own confirmed tick plus `rollback_window + input_delay`, and its confirmed tick is at most `h + 1` here, so a remote input for a tick above `h + 1 + rollback_window + input_delay` is `BeyondHorizon`.
  - An `Ack` for a tick the local peer never sent is `AckNeverSent`.
  - The newest `Ack` of a peer can trail the local horizon by at most `2 × (rollback_window + input_delay) + 1` ticks, since each peer stalls at its window. Beyond that it is `AckLagging`, which also bounds the unacknowledged inputs kept per peer.
  - An input for a player the sender does not own is `NotOwner`, and an input `check` refuses is `InvalidInput`, naming the player and the tick.
  - Every computation on a received tick is checked first: `first_tick + inputs.len()` that overflows is `BeyondHorizon`, never a panic.

- [ ] **Step 1: Write the failing tests**:
  - `conflicting_input_disconnects_the_peer` (naming player and tick; the confirmed input is kept).
  - `undecodable_input_disconnects_the_peer`.
  - `input_for_a_player_the_peer_does_not_own_is_a_violation`.
  - `input_beyond_the_horizon`: at the bound accepted, one tick above disconnected.
  - `honest_peer_at_the_horizon_under_jitter_is_not_disconnected`: a conditioned run with 150 ms latency and 30 ms jitter, where one peer runs at the edge of its window.
  - `ack_never_sent_is_a_violation`.
  - `ack_lagging_is_a_violation`: at the bound accepted, one above disconnected.
  - `ticks_near_u64_max_never_panic`.
  - `fuzz_receive_never_panics`: for seeds `0..10_000` drawn from `ludenea_app::Rng`, two running sessions; one receives sequences of random well-formed `Message`s (random fields, ticks near the horizon and near `u64::MAX`, random players, random acknowledgments, quality echoes in the future) interleaved with valid traffic. Every call returns, and the session ends only with `Violation` or stays `Running`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test violations`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates, and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): disconnect peers that break the protocol`.

---

### Task 11: Time synchronization

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`
- Test: `crates/ludenea-net/tests/time_sync.rs`

**Interfaces:**
- Produces: `PeerStats { ping_micros: u64, jitter_micros: u64, frame_advantage: i32 }` per peer in `SessionStats`, and a `skips` counter, apart from `stalls`.
- Rules (builder's call, named constants in `session.rs`):
  - `QUALITY_INTERVAL_MICROS = 200_000`: send `Quality { tick, sent_micros, advantage }`. The receiver answers with `QualityReply`.
  - A reply is used only if its `echo_micros` matches a request still pending (at most 16 kept; an unknown, duplicate or future echo is dropped and counted).
  - The ping is the mean of the last 16 round trips, and the jitter is the mean absolute deviation from that mean.
  - The tick length is `1_000_000 / tick_rate` microseconds, an integer.
  - The local advantage over a peer is the local tick minus the peer's estimated tick: its last reported tick, plus the ticks elapsed since that report, plus half the ping in ticks.
  - Like GGPO, the correction is half the difference of the two views, `(local_advantage - remote_advantage) / 2`. Over 3 or 4 peers, the session corrects for the largest one.
  - When the mean correction over at least `SYNC_MIN_REPORTS = 4` fresh reports is at least 1 tick, the session skips that many ticks, capped at `rollback_window / 2`: one `advance` per skipped tick returns no new `Advance`, and each counts in `skips`. It then clears the report history and waits for 4 fresh reports, so a correction never acts twice on the same evidence.

- [ ] **Step 1: Write the failing tests**:
  - `ping_converges_to_the_conditioned_latency`: 100 ms latency each way, time stepped by 1 ms, ping between 195 and 205 ms after 5 s.
  - `duplicate_or_unknown_replies_do_not_skew_the_ping`.
  - `faster_peer_slows_down`: one peer's frames 2% shorter. From 10 s to 60 s, neither peer stalls, the tick gap stays within 2, and the faster peer's skips per second stay within 1.2 ± 0.4 (no oscillation).
  - `three_peers_converge` with one fast and one slow peer.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test time_sync`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): synchronize peer clocks`.

---

### Task 12: Hash reports, desync and end notices

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`
- Test: `crates/ludenea-net/tests/hashes.rs`

**Interfaces:**
- Produces:
  - `Session::report_hash(&mut self, tick: u64, hash: u64)`.
  - `Request::Release { tick: u64 }`: the executor may drop the state it kept for that tick.
  - `SessionEvent::HashMatched { tick: u64 }`.
  - `EndReason::Desync { tick: u64, peer: PeerId, local: u64, remote: u64 }`.
  - `DropReason` additions for a hash at a tick that is not a multiple of `hash_interval`, or already received.
  - `ViolationRule::HashBeyondHorizon`.
- Rules:
  - Every `Save` of a tick that is a multiple of `hash_interval` asks for the hash (`hash: true`), and the state saved there is kept for that tick until `Release`.
  - The session keeps, per tick, the hash of the latest save. It sends the hash of tick `T` only once:
    - every input below `T` is confirmed;
    - the `advance` that emitted the rollback for those inputs has run;
    - `report_hash` has answered that rollback's save.

    The hash sent is therefore always the hash of a state built only from confirmed inputs, whether a rollback crossed `T` or the prediction was right and nothing was resaved.
  - Hash reports are resent in every packet until the peer's `Ack.hashes_through` covers them.
  - A received hash is accepted only for a multiple of `hash_interval` at or below the local horizon plus 1, since no honest peer can have confirmed beyond that; above it is `HashBeyondHorizon`. At most `(rollback_window + input_delay) / hash_interval + 2` received hashes are kept waiting for a local one, so the memory stays bounded.
  - Once every peer's hash of `T` matches the local one, the session emits `HashMatched { tick: T }` and `Release` for the kept states of reported ticks before `T`. The state of `T`, the last matching report, is kept until the next one matches.
  - A difference ends the session with `Desync`. Every packet then carries `End { Desync }` for `END_LINGER_MICROS = 500_000`, so a peer whose own hash report was lost also ends with `Desync` rather than `Lost`. Any received `End` notice ends the receiver with the same reason. The same linger applies to every ending the session decides alone (`Lost` in Task 13, `LocalFailure` in Task 14).

- [ ] **Step 1: Write the failing tests**:
  - `hashes_agree_without_desync`.
  - `hash_is_sent_under_latency_without_a_rollback`: 100 ms latency, correct predictions across tick 30; the hash of 30 is sent and matched.
  - `hash_report_uses_the_state_resimulated_after_a_correction`: a rollback crossing tick 30 sends the hash of the resimulated state, not the predicted one.
  - `hash_reports_survive_loss` at 300 per mille.
  - `diverging_hash_ends_every_peer_with_desync`: the test model flips one peer's hash at tick 90, with that peer's outgoing hash packets dropped. Both peers, and a third one, end with `Desync { tick: 90 }`.
  - `reported_state_is_kept_until_every_peer_matches`: the `Release` of a reported tick comes only after the next report matched on every peer.
  - `hash_beyond_horizon_is_a_violation`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test hashes`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): compare state hashes between peers`.

---

### Task 13: Disconnection and the agreed end

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`
- Test: `crates/ludenea-net/tests/session_end.rs`

**Interfaces:**
- Produces:
  - `Session::leave(&mut self)`.
  - `SessionState::Leaving`.
  - `SessionEvent::{ PeerSilent { peer: PeerId, for_micros: u64 }, PeerBack { peer: PeerId } }`.
  - `EndReason::{ Left { peer: PeerId, last_tick: u64 }, Lost { peer: PeerId } }`.
  - `ViolationRule::GoodbyeBehind`.
- Rules:
  - **Silence.** `PeerSilent` once a peer has been silent for `notice_after_micros`, and `PeerBack` when it speaks again. After `disconnect_after_micros` the session ends with `Lost`, and the `End { Lost }` notice ends every other peer too (spec section 7.4: "ends for everyone").
  - **The leaving peer.** `leave` sets `last_tick` to the leaver's input horizon, the last tick it has its own inputs for. No peer can confirm beyond that tick. The session goes `Leaving`: every packet carries `Goodbye { last_tick }` and the unacknowledged inputs up to `last_tick`. It ends with `Left { peer: local }` once every peer acknowledges every local player through `last_tick`, or after `disconnect_after_micros`.
  - **The remaining peers.** On a `Goodbye`, each remaining peer keeps running and exchanging inputs until every player's inputs through `last_tick` are confirmed, then ends with `Left`.
    - Inputs of later ticks are ignored, and the session emits no `Confirmed` beyond `last_tick`.
    - When several peers leave, the lowest `last_tick` wins.
    - A `Goodbye` below the receiver's own confirmed tick cannot come from an honest peer: it is `GoodbyeBehind`.
  - **Final state.** On every ending, `advance` returns the requests that bring the simulation to the state of the end tick built only from confirmed inputs:
    - a `Load` of the latest tick whose state was built only from confirmed inputs;
    - then `Save` and `Advance` with confirmed inputs.

    So the simulation each peer holds at the end is the state its confirmed inputs give.

- [ ] **Step 1: Write the failing tests**:
  - `silent_peer_raises_a_notice_then_ends`: notice after 750 ms of silence, `PeerBack` when packets resume, `Lost` after 3 s of silence.
  - `lost_peer_ends_the_match_for_everyone`: three peers, one goes silent; both others end with `Lost` naming it.
  - `goodbye_ends_everyone_at_the_same_tick`: behind `Conditioned` with 100 ms latency, 100 per mille loss and 20 per mille duplication, for 2 and 3 peers. Every remaining peer ends with the same `last_tick`, and the `Confirmed` events of all peers are identical, byte for byte once encoded.
  - `goodbye_behind_is_a_violation`.
  - `ended_session_lands_on_the_confirmed_state`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test session_end`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): end matches at a tick every peer agrees on`.

---

### Task 14: The executor and multi-peer property tests

**Files:**
- Create: `crates/ludenea-net/src/execute.rs`, `examples/arena/tests/fixtures/arena-four.scene.ron` (the arena scene with spawn points for players 2 and 3, so all four players act)
- Modify: `crates/ludenea-net/src/session.rs`, `crates/ludenea-app/src/{input.rs, simulation.rs, lib.rs}` (`join_player_inputs`), `.github/workflows/determinism.yml` (a Linux step that runs the arena property test in release with `LUDENEA_NET_SEEDS=32`)
- Test: `crates/ludenea-net/tests/execute.rs` (with a small test app), `crates/ludenea-net/tests/execute_panic.rs` (`#![cfg(not(target_family = "wasm"))]`), `crates/ludenea-net/tests/model_peers.rs` (the model property test, which runs under `wasmtime`), `examples/arena/tests/rollback.rs` (the arena property test)

**Interfaces:**
- Produces in `ludenea_app`: `join_player_inputs(inputs: &[Vec<u8>]) -> Vec<u8>`, the postcard encoding of `Vec<I>` from one encoded input per player (the varint length, then the inputs concatenated). The executor steps with `step_encoded(Some(&joined))`, and 4b's input log records the same joined bytes: one way to step with per-player inputs.
- Produces in `ludenea_net`:
  - `Session::fail(&mut self, tick: u64)`: ends with `EndReason::LocalFailure { tick }` and the end notice of Task 12.
  - `Executor::new(sim: &mut Simulation, settings: &SessionSettings) -> Result<Executor, ExecuteError>`: sets `sim.set_players(players.len())`, and refuses an app without an input type (`ExecuteError::NoInputType`). It sizes its `StateRing` from the rules: `rollback_window + 2` states, plus the states kept for reported ticks until `Release`. Every entry is tagged with its tick.
  - `Executor::execute(&mut self, sim: &mut Simulation, session: &mut Session, requests: Vec<Request>) -> Result<(), ExecuteError>` applies the requests in order:
    - for each `Save` and `Advance`, it first checks `sim.tick() == tick` (`ExecuteError::OutOfStep { expected, found }`);
    - it calls `report_hash` for `Save { hash: true }`;
    - a `Load` of a tick the ring does not hold, or holds for another tick, is `ExecuteError::MissingState { tick }`;
    - a `StateError` is `ExecuteError::State`;
    - on a game error or panic in `Advance { tick }`, it loads the state of `tick` (always in the ring, since its `Save` came first), calls `session.fail(tick)`, and returns `ExecuteError::Game { tick, error: SimError }`.

- [ ] **Step 1: Write the failing tests**:
  - `joined_inputs_equal_the_postcard_vec`.
  - `execute_refuses_out_of_step_and_missing_states`.
  - `executor_sets_the_player_count`.
  - `panic_during_resimulation_restores_and_ends`: a test app whose system panics when an input byte is `0xFF`, sent as a late remote input. The simulation is at the state of that tick, `ExecuteError::Game` names it, and the other peer ends with `LocalFailure`.
  - `model_peers_match_an_offline_run` (`model_peers.rs`): the property test on the model game. 2, 3 and 4 peers, 32 seeds, 300 ticks then `leave`, conditions drawn per seed: latency 0 to 150 ms, jitter 0 to 30 ms, loss 0 to 100 per mille, duplication 0 to 20 per mille, reordering 0 to 50 per mille. Every peer's confirmed inputs are equal, and folding them offline gives every peer's hash at every confirmed tick. Over all cases, at least one rollback and one prediction miss occur, so the test cannot pass vacuously.
  - `two_three_and_four_peers_match_an_offline_run` (`examples/arena/tests/rollback.rs`):
    - the same shape on the arena app with `Executor`, the four-spawn fixture scene, and seeded random canonical inputs per player (run from `{-1, -0.5, 0, 0.5, 1}`, jump or not);
    - at 1 and 8 threads;
    - `LUDENEA_NET_SEEDS` seeds (8 by default, 32 in CI release), 300 ticks per match;
    - the confirmed inputs, joined with `join_player_inputs` and stepped offline, give each peer's executed state hash at every confirmed tick;
    - the test asserts the same non-vacuity.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test execute --test execute_panic --test model_peers` and `cargo test -p arena --test rollback`. Expected: compile failure.
- [ ] **Step 3: Implement** the executor.
- [ ] **Step 4: Run** the gates, the `ludenea-net` tests under `wasmtime`, and `LUDENEA_NET_SEEDS=32 cargo test --locked --release -p arena --test rollback`. Expected: PASS; the debug run of the arena test stays under a minute.
- [ ] **Step 5: Commit** `✨ feat(net): drive a simulation from a rollback session`.

---

### Task 15: Share the per-entity divergence report

**Files:**
- Create: `crates/ludenea-app/src/divergence.rs`
- Modify: `crates/ludenea-app/src/lib.rs`, `crates/ludenea-control/src/divergence.rs` (uses the shared half and re-exports its types under their current paths), `CHANGELOG.md` (Rust API)
- Test: the existing tests of `crates/ludenea-control` (the branch divergence report, unchanged behaviour), the generated TypeScript bindings (unchanged: the types keep their names and `cfg_attr(feature = "ts")` derives), new unit tests in `crates/ludenea-app/src/divergence.rs`

**Interfaces:**
- Moves to `ludenea_app::divergence`, unchanged: `EntityDivergence`, `DivergenceKind`, `Cause`. `ludenea_control::divergence` re-exports them.
- Produces:
  - `TickRecorder` (the current `Recorder`): a `ScheduleObserver` whose `into_record(self, tick: u64) -> TickRecord` gives the events and commands of one tick.
  - `EntityComparison::new(skeleton_from: &Simulation)`.
  - `EntityComparison::compare(&mut self, sims: [&Simulation; 2], tick: u64, records: &[TickRecord], explains_from: u64, event_schemas: &BTreeMap<&'static str, TypeSchema>, systems: &[&'static str]) -> Vec<EntityDivergence>` gives the entities diverging for the first time at `tick`, with their first difference and their causes.
    - It holds the per-entity state `Progress` holds today: which entities already diverged, the snapshot skeleton, the warnings, the first divergent tick.
    - `explains_from` is the branch report's fork rule: records of ticks below it explain nothing.
  - `EntityComparison::{ first_divergent, warnings }` accessors.
- Branch-specific parts stay in `ludenea-control`: the fork point, the root cause, the runs, and `DivergenceReport` itself.

- [ ] **Step 1: Write the failing tests**: unit tests in `ludenea_app::divergence` on two boxes simulations, one with an entity moved: `Changed` with its first difference; an entity spawned on one side only: `OnlyInCurrent`; a command recorded on that entity: a `Command` cause.
- [ ] **Step 2: Run** `cargo test -p ludenea-app divergence`. Expected: compile failure.
- [ ] **Step 3: Move** the code. `ludenea-control` calls `EntityComparison` from `Progress`.
- [ ] **Step 4: Run** the gates. `cargo test -p ludenea-control` passes unchanged, and regenerating the TypeScript bindings leaves `git status` clean. Expected: PASS.
- [ ] **Step 5: Commit** `♻️ refactor(app): share the per-entity divergence report`.

---

### Task 16: SyncTest

**Files:**
- Create: `crates/ludenea-net/src/sync_test.rs`
- Modify: `crates/ludenea-app/src/replay.rs` (`play_tick_observed`), `crates/ludenea-cli/src/{lib.rs, commands.rs}` (`synctest` subcommand), `crates/ludenea-cli/Cargo.toml`, `Cargo.lock`, `.github/workflows/determinism.yml`, `README.md`, `CHANGELOG.md` (the Rust API section: `sync_test`, `play_tick_observed`, and the `synctest` command, next to the existing `run` and `verify` entries there)
- Test: `crates/ludenea-net/tests/sync_test.rs` (test apps), `examples/arena/tests/synctest.rs`, `examples/boxes/tests/synctest.rs`, `crates/ludenea-cli/tests/synctest.rs`

**Interfaces:**
- Produces in `ludenea_app`: `play_tick_observed(sim, inputs, edits, tick, observer: &mut dyn ScheduleObserver) -> Result<(), ReplayError>`, which is `play_tick` with `step_observed`.
- Produces in `ludenea_net`:
  - `sync_test(sim: &mut Simulation, other: &mut Simulation, inputs: &InputLog, edits: &EditLog, ticks: u64, window: u32) -> Result<SyncTestReport, SyncTestError>`.
    - The caller loads the same scene, or replay, into both simulations first (`load_replay` for a replay) and builds `other` from the same app.
    - `1 <= window` and `window <= ticks` are checked first (`SyncTestError::Window`).
  - The algorithm, for each tick `t` from 0 to `ticks - 1`:
    1. `other` runs the straight line: it saves the state of `t` into its own ring of `window + 2`, plays tick `t` with `play_tick`, and records the hash `H[t + 1]`.
    2. `sim`, at tick `t`, saves the state of `t` into its ring. It plays tick `t` once with a wrong input, the way a misprediction would: the input of tick `t - 1`, or the default inputs when that equals the real input, through `step_encoded`.
    3. It then loads the state of `max(0, t + 1 - window)` from its ring and replays every tick from there to `t` with `play_tick`, saving each state again.
    4. It compares each resimulated hash with `H`.

    This catches state outside the world, non-deterministic resimulation, and state leaking from a mispredicted branch.
  - `SyncTestReport { ticks: u64, rollbacks: u64 }`.
  - `SyncTestError::Mismatch { tick: u64, straight: u64, rollback: u64, entities: Vec<EntityDivergence>, snapshots: [Snapshot; 2] }`, for the first tick `k` whose hashes differ.
    - Each side loads its state of `k - 1` and plays tick `k - 1` observed with a `TickRecorder`.
    - Each side then loads its state of `k`. `EntityComparison` compares them, with the records as causes, and `snapshots` holds both encoded states of `k`.
    - The causes come from re-running tick `k - 1`. For game code that is not deterministic they are candidates, as in the branch report, and the doc comment says so.
    - Its message lists the first 10 entities (entity, kind, first differing component and field, causes) and the count of the rest.
  - `SyncTestError::{ Game { tick, error: ReplayError }, Window { window, ticks } }`.
- Command: `<game> synctest <scene or .replay> [--project P] [--edits E] [--inputs I] [--random-inputs SEED] [--ticks N] [--window W] [--threads T] [--dump-dir D]`.
  - For a replay, like `verify`, `--inputs`, `--edits` and `--project` are refused.
  - For a scene, `--ticks` is required. `--random-inputs SEED`, refused with `--inputs`, plays seeded `RawInput`s mapped by the game's own `map_raw_input`: keys held from a fixed set of common codes, and one gamepad with random axes and buttons, drawn from `ludenea_app::Rng`. Any game then gets inputs that are valid for it.
  - `--window` defaults to 8, `--dump-dir` to `dump`, as for `verify`.
  - Output: `synctest passed: N ticks, R rollbacks`, exit 0.
  - A mismatch prints the error, writes `tick-K-straight.snap` and `tick-K-rollback.snap` to the dump folder, and exits 1.
  - The command builds `other` through the same `fn() -> App` the game passes to `ludenea::cli::main`.

- [ ] **Step 1: Write the failing tests**:
  - `boxes_passes_synctest` and `arena_passes_synctest`, each in its own example crate's `tests/synctest.rs`.
  - `state_kept_outside_the_world_fails_synctest`. A test app inserts `Runtime<AtomicU32>` through `world_mut().insert_runtime`; runtime resources are never saved. Its system increments the counter and writes it into a component. The error names the tick, the entity, the component and the field.
  - `window_out_of_range_is_refused`.
  - `synctest_command_passes_on_the_arena_replay`.
  - `synctest_command_reports_a_mismatch_and_dumps_both_snapshots`.
  - `synctest_command_refuses_inputs_for_a_replay`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test sync_test`, `cargo test -p arena --test synctest`, `cargo test -p boxes --test synctest` and `cargo test -p ludenea-cli --test synctest`. Expected: compile failure.
- [ ] **Step 3: Implement**. Then add CI steps in release on every operating system's job, at 1 and 8 threads, and under `wasmtime` for the arena:
  - `arena synctest golden/arena.replay`;
  - `swarm synctest examples/swarm/assets/swarm.scene.ron --ticks 600 --random-inputs 1`;
  - `boxes synctest golden/spawner.replay`.

  Document the command in `README.md`, next to `verify`.
- [ ] **Step 4: Run** the gates and `cargo xtask check`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): catch non-deterministic game code with synctest`.
