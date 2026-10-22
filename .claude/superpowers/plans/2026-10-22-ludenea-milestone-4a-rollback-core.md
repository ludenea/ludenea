# Ludenea Milestone 4a: Rollback Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A rollback session that keeps two to four peers in sync under latency, loss and reordering, proven in one process, on top of a fast in-memory save and restore of the simulation, with SyncTest catching non-deterministic game code from the command line.

**Architecture:** `Component` and `Resource` require `Clone`, so `ludenea-ecs` can save a world into a reusable `SavedWorld` and load it back without serializing; `Simulation` wraps it as `SavedState`. A new crate `ludenea-net` holds the sans-IO `Session` (input queues, prediction, input delay, rollback window, time synchronization, hash reports, disconnection), its versioned wire format, the `Transport` trait with an in-memory network and a seeded link conditioner, an executor that applies the session's requests to a `Simulation`, and `sync_test`. Each game binary gains `synctest`. Nothing in the control protocol, the host, the runtime or the editor changes in this milestone.

**Tech Stack:** unchanged (Rust 1.97.1, serde, postcard, clap). No new external dependency: the link conditioner's random source is a small seeded generator inside `ludenea-net`.

**Spec:** `.claude/superpowers/specs/2026-10-22-ludenea-networking-4-design.md`. This plan covers spec sections 4.1, 4.2, the in-memory and conditioner rows of 4.3, the detection half of 5 (hash reports, mismatch event, SyncTest with a field diff), 7.4 at the session level, and the session and benchmark parts of 8. Milestone 4b covers WebRTC, signaling, the host integration (4.4, 4.5), the native and web runtimes and the headless peer. Milestone 4c covers the editor, the full causal desync report exchanged between peers, SyncTest through the host and the editor, the end-to-end cross-play test and the online golden.

## Global Constraints

- Every Global Constraint of the earlier milestone and quality-bar plans still applies (determinism lints, target gates, committed generated files, `--locked`, commit style).
- The state hashes of `golden/` never move, and no golden file changes in this milestone.
- `ludenea-net` reads no clock and uses no unordered collection: the caller passes the current time as `now_micros: u64` (monotonic microseconds). It uses no floating point at all: ping, jitter and frame advantage are integers.
- Spec defaults, as `SessionSettings::default()` values: `input_delay = 2` ticks, `rollback_window = 8` ticks, `hash_interval = 30` confirmed ticks, `notice_after_micros = 750_000`, `disconnect_after_micros = 3_000_000`.
- Limits (builder's call): at most 4 peers and 4 players; `MAX_INPUT_BYTES = 128` per player per tick; `MAX_PACKET_BYTES = 1200` (a safe WebRTC payload); an input message carries at most `MAX_TICKS_PER_MESSAGE = 32` ticks.
- New format: the network wire format, `NET_PROTOCOL_VERSION = 1`, added to the CHANGELOG formats table (current 1, oldest readable 1). `PROTOCOL_VERSION` and every other format constant stay unchanged.
- Anything decoded from a packet is untrusted: it never panics, and an invalid packet is dropped and counted under a named reason in `SessionStats`, or, when it proves the peer is broken or hostile, disconnects that peer with an error naming the peer, the tick and the rule broken.
- `ludenea-net` builds for `wasm32-unknown-unknown` and `wasm32-wasip1`; its tests run under `wasmtime` in CI.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task.

## Deviations From The Spec

- `Asset` does not gain `Clone`. Loaded assets are already shared as `Arc` values and are never written by systems, so a saved world shares them instead of cloning them. The breaking change covers `Component` and `Resource` only, which is all the saved state needs.
- SyncTest is reachable from the game's command line and the `ludenea-net` API in this milestone; through the host and the editor in 4c.
- On a hash mismatch between peers, 4a emits `SessionEvent::Desync` with the tick and both hashes and stops the session; the exchange of hash trees and snapshots that turns it into a full report is 4c. SyncTest already produces the field-level diff, since both states live in one process.

## Review Focus

- **A hostile or broken packet** (truncated, oversized, wrong `NET_PROTOCOL_VERSION`, wrong match identifier, a player index the sender does not own, a tick beyond the window, more than `MAX_TICKS_PER_MESSAGE` ticks): dropped and counted by reason, or the peer disconnected with a named rule; never a panic. Tests in Tasks 5 and 7, fuzzing in Task 5.
- **A peer that sends a different input for a tick it already sent**: a protocol violation that disconnects that peer naming the player and the tick; the confirmed input is never overwritten. Test in Task 7.
- **Remote input bytes that do not decode as the game's input type**: refused when received, through the validator the session is built with, naming the player and the tick, before they reach a step. Test in Task 7.
- **A saved state loaded into another simulation** (another game build, or different registrations): `StateError::OtherWorld` naming both fingerprints; the simulation is unchanged. Test in Task 2.
- **Game code that panics while resimulating after a rollback**: the executor returns `SimError::Panicked` naming the tick, the session reports it, and the simulation is restored to the last saved state rather than left halfway. Test in Task 9.

---

### Task 1: `Component` and `Resource` require `Clone`

**Files:**
- Modify: `crates/ludenea-ecs/src/component.rs` (bounds and their doc comments), every `#[derive(Component)]` and `#[derive(Resource)]` type in `crates/`, `examples/` and their tests that lacks `Clone` (about 85 sites, most in tests), `CHANGELOG.md` (Rust API: the breaking change and its migration).
- Test: a `compile_fail` doc test on `Component` showing a type without `Clone` is refused.

**Interfaces:**
- Produces: `pub trait Component: Schema + Serialize + DeserializeOwned + Clone + Send + 'static` and the same `Clone` bound on `Resource`. Later tasks clone storages through these bounds.

- [ ] **Step 1: Write the doc test**: a `compile_fail` block deriving `Component` on a struct without `Clone`.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs --doc component`. Expected: FAIL (the block compiles).
- [ ] **Step 3: Add the bounds** and derive `Clone` on every type the compiler names. The project template (`crates/ludenea-project/template/src/lib.rs`) already derives it; check it still does.
- [ ] **Step 4: Run** `cargo test --locked --workspace` and the clippy gates. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `💥 feat(ecs): require Clone on components and resources`.

---

### Task 2: Save and load a world in memory

**Files:**
- Create: `crates/ludenea-ecs/src/saved.rs`
- Modify: `crates/ludenea-ecs/src/world.rs` (`Storage` and `ResourceCell` gain clone methods), `crates/ludenea-ecs/src/assets.rs` (`LoadedAsset` cheap to clone: `bytes` behind `Arc<[u8]>`), `crates/ludenea-ecs/src/lib.rs`, `crates/ludenea-app/src/simulation.rs`, `CHANGELOG.md`
- Test: `crates/ludenea-ecs/tests/saved.rs`, `crates/ludenea-app/tests/saved_state.rs`, `examples/boxes/tests/saved_state.rs`, `examples/arena/tests/saved_state.rs` (tests that need an example game live in that example's crate: a dev-dependency from an engine crate on an example would compile the engine crate twice)

**Interfaces:**
- Produces in `ludenea_ecs`: `SavedWorld` (`Default`), `World::save_into(&self, into: &mut SavedWorld)`, `World::load(&mut self, saved: &SavedWorld) -> Result<(), SavedError>`. `save_into` reuses the allocations already in `into` (`Vec::clone_from` per storage). A `SavedWorld` holds what `Snapshot` holds: the entity allocator, every component storage, every resource, the loaded assets (shared `Arc`s). `load` drops queued commands and events and keeps runtime resources, like `World::restore`.
- Produces in `ludenea_app`: `SavedState` (`Default`), `Simulation::save_state(&self, into: &mut SavedState)`, `Simulation::load_state(&mut self, saved: &SavedState) -> Result<(), StateError>`; `StateError::OtherWorld { saved: u64, current: u64 }` (schema fingerprints, formatted as 16 hex digits), `StateError::Empty` for a `SavedState` never saved into. A `SavedState` is stamped with `schema_fingerprint()`; `load_state` checks it before touching the world.

- [ ] **Step 1: Write the failing tests**:
  - `saved_world_round_trips_snapshot`: a property test over random worlds (seeded, 256 cases; spawn, despawn, insert and remove across several component types, resources, assets): `save_into`, step or mutate randomly, `load`, then `world.snapshot()` equals the snapshot taken at the save, byte for byte after `encode()`.
  - `save_into_reuses_allocations`: saving twice into the same `SavedWorld` with the same shape keeps each storage's capacity (assert through a test-only capacity accessor or by `Vec` pointer equality).
  - `load_keeps_runtime_resources_and_drops_commands`.
  - `load_state_refuses_another_game`: a `SavedState` from an app with one more component type: `StateError::OtherWorld`, and `snapshot()` unchanged.
  - `load_state_refuses_empty`.
  - `saved_state_matches_snapshot_on_boxes_and_arena`: run 300 ticks of each example scene, saving every tick; load tick 150 and resimulate to 300: hashes equal the straight run at 1 and 8 threads.
- [ ] **Step 2: Run** `cargo test -p ludenea-ecs --test saved`, `cargo test -p ludenea-app --test saved_state`, `cargo test -p boxes --test saved_state` and `cargo test -p arena --test saved_state`. Expected: compile failure.
- [ ] **Step 3: Implement** `Storage::clone_into(&self, into: &mut Option<Box<dyn Storage + Send>>)` and `Storage::load_from(&mut self, saved: &dyn Storage)` (and the same pair on `ResourceCell`), downcasting through `as_any`; `SavedWorld` keys storages by stable name in `BTreeMap`s, like `World`.
- [ ] **Step 4: Run** `cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): save and load the simulation state in memory`.

---

### Task 3: Rollback benchmark on the arena

**Files:**
- Create: `examples/arena/src/bin/rollback-bench.rs`
- Modify: `README.md` (the command, next to the boxes benchmark), `.github/workflows/determinism.yml` (a step on the Linux job that runs it in release and appends its output to the job summary; it records, it does not gate)

**Interfaces:**
- Consumes: `Simulation::save_state`, `Simulation::load_state` (Task 2).

- [ ] **Step 1: Write** the binary: loads `golden/arena.replay`, plays it, and at every tick from 600 on measures one worst-case rollback frame: `load_state` of the state 8 ticks back, then 9 `play_tick` calls with a `save_state` after each (the spec's W+1 ticks). Prints the mean, the 99th percentile and the worst frame in microseconds, at 1 thread, and whether the worst frame fits in half a 60 Hz frame (8,333 microseconds). The bench is display code: timing uses `std::time::Instant` with the `#[expect(clippy::disallowed_methods, reason = "benchmark timing, never feeds the simulation")]` pattern the boxes bench uses.
- [ ] **Step 2: Run** `cargo run --locked --release -p arena --bin rollback-bench`. Expected: a report line per measure; record the numbers in the commit message.
- [ ] **Step 3: Commit** `⏱️ perf(arena): measure the worst rollback frame`. If the worst frame misses the target, stop and report the numbers before Task 4: the save path needs work before the session is built on it.

---

### Task 4: `ludenea-net` crate, settings and the link conditioner's random source

**Files:**
- Create: `crates/ludenea-net/{Cargo.toml, src/lib.rs, src/settings.rs, src/random.rs}`
- Modify: `crates/ludenea/src/lib.rs` (re-export `ludenea::net`), `.github/workflows/determinism.yml` (clippy for `wasm32-unknown-unknown` and `wasm32-wasip1`, and `cargo test -p ludenea-net` under `wasmtime`, next to the existing wasm steps)
- Test: `crates/ludenea-net/tests/settings.rs`

**Interfaces:**
- Produces: `PeerId(pub u8)`; `MatchIdentity { engine: String, fingerprint: u64, scene_and_assets: u64, tick_rate: u32, players: Vec<PeerId> }` (the owner of each global player index, in order); `SessionSettings { local: PeerId, identity: MatchIdentity, match_id: u64, input_delay: u32, rollback_window: u32, hash_interval: u32, notice_after_micros: u64, disconnect_after_micros: u64 }` with `Default` giving the spec values; `SessionSettings::validate(&self) -> Result<(), SettingsError>`; constants `MAX_PEERS = 4`, `MAX_PLAYERS = 4`, `MAX_INPUT_BYTES = 128`, `MAX_PACKET_BYTES = 1200`, `MAX_TICKS_PER_MESSAGE = 32`, `NET_PROTOCOL_VERSION = 1`; `SplitMix64::new(seed: u64)` with `next_u64` and `below(n: u64) -> u64`.

- [ ] **Step 1: Write the failing tests**: `defaults_are_the_spec_values`; `validate_refuses` for each of: no players, more than `MAX_PLAYERS`, a player owned by a peer index `>= MAX_PEERS`, a local peer that owns no player (spectators are out of scope), `rollback_window == 0` or `> MAX_TICKS_PER_MESSAGE - input_delay`, `hash_interval == 0`, `notice_after_micros >= disconnect_after_micros`, `tick_rate == 0`; each error message names the field and the value. `splitmix_sequence_is_pinned`: the first five values for seed 0 equal the reference SplitMix64 outputs.
- [ ] **Step 2: Run** `cargo test -p ludenea-net`. Expected: compile failure.
- [ ] **Step 3: Implement** the crate (depends on `ludenea-app`, `ludenea-ecs`, `serde`, `postcard`, `thiserror`, all workspace dependencies).
- [ ] **Step 4: Run** the workspace gates and `cargo clippy --locked --target wasm32-unknown-unknown -p ludenea-net -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): add the network crate with validated session settings`.

---

### Task 5: Wire format

**Files:**
- Create: `crates/ludenea-net/src/wire.rs`
- Modify: `CHANGELOG.md` (formats table row `Network protocol | NET_PROTOCOL_VERSION | 1 | 1`, and a Formats entry describing it)
- Test: `crates/ludenea-net/tests/wire.rs`

**Interfaces:**
- Produces: `Packet { match_id: u64, message: Message }`, `Packet::encode(&self) -> Vec<u8>`, `Packet::decode(bytes: &[u8]) -> Result<Packet, WireError>`. The encoding is `NET_PROTOCOL_VERSION` as a little-endian `u16`, then postcard. `Message`:
  - `Hello { identity: MatchIdentity, net_version: u16 }` and `Welcome` (handshake);
  - `Inputs { player: u8, first_tick: u64, inputs: Vec<Vec<u8>>, ack: u64 }` (consecutive ticks from `first_tick`; `ack` is the next tick the sender still needs from the receiver);
  - `Quality { tick: u64, sent_micros: u64 }` and `QualityReply { echo_micros: u64, tick: u64 }`;
  - `Hash { tick: u64, hash: u64 }`;
  - `Goodbye { last_tick: u64 }`.
- `WireError` variants: `TooLarge { len }`, `Version { found, expected }`, `Malformed(String)`, `TooManyTicks { count }`, `InputTooLarge { len }`.

- [ ] **Step 1: Write the failing tests**: `every_message_round_trips`; `decode_refuses` one case per `WireError` variant; `encoded_inputs_message_fits_a_packet` (one message carries one player: `MAX_TICKS_PER_MESSAGE` ticks of `MAX_INPUT_BYTES` each must encode within `MAX_PACKET_BYTES`; if it cannot, lower `MAX_TICKS_PER_MESSAGE` in Task 4 and say so in the commit); `fuzz_decode_never_panics`: 100,000 seeded random and mutated packets (bit flips, truncations, length fields set to `u32::MAX`) through `Packet::decode`, run in the test suite like the existing fuzz tests.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test wire`. Expected: compile failure.
- [ ] **Step 3: Implement**: check the length before decoding; postcard with `take_from_bytes` and refuse trailing bytes; check vector lengths after decoding.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): define the versioned network wire format`.

---

### Task 6: Transports: in-memory network and link conditioner

**Files:**
- Create: `crates/ludenea-net/src/transport.rs`
- Test: `crates/ludenea-net/tests/transport.rs`

**Interfaces:**
- Produces: `trait Transport { fn send(&mut self, to: PeerId, packet: Vec<u8>); fn receive(&mut self, now_micros: u64) -> Option<(PeerId, Vec<u8>)>; }`; `MemoryNetwork::new(peers: u8) -> Vec<MemoryTransport>` (shared queues, single-threaded, in send order); `LinkConditions { latency_micros: u64, jitter_micros: u64, loss_per_mille: u16, duplicate_per_mille: u16, seed: u64 }` (integers only; 5% loss is `50`); `Conditioned<T: Transport>::new(inner: T, conditions: LinkConditions)`, which delays each outgoing packet by `latency + below(jitter + 1)`, drops and duplicates per mille, and delivers a packet only once `now_micros` passes its due time, in due-time order (jitter reorders).

- [ ] **Step 1: Write the failing tests**: `memory_delivers_in_order`; `conditioned_is_deterministic_for_a_seed` (same seed, same schedule of sends and times, same deliveries); `conditioned_loss_rate_is_close` (100,000 packets at 50 per mille: between 4.5% and 5.5% lost); `conditioned_delays_by_latency`; `jitter_reorders`; `duplicates_arrive_twice`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test transport`. Expected: compile failure.
- [ ] **Step 3: Implement**: pending packets in a `BTreeMap<(u64 due, u64 sequence), (PeerId, Vec<u8>)>`.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): add an in-memory network and a seeded link conditioner`.

---

### Task 7: The session: handshake, inputs, prediction and rollback

**Files:**
- Create: `crates/ludenea-net/src/session.rs`, `crates/ludenea-net/src/queue.rs`
- Test: `crates/ludenea-net/tests/session.rs` (scripted, no simulation: inputs are opaque bytes, and a test model of the game folds inputs into a running hash)

**Interfaces:**
- Consumes: `SessionSettings`, `Packet`, `Message` (Tasks 4, 5).
- Produces:
  - `Session::new(settings: SessionSettings, default_input: Vec<u8>, check_input: Box<dyn Fn(&[u8]) -> Result<(), String>>) -> Result<Session, SettingsError>`;
  - `Session::add_local_input(&mut self, player: u8, input: Vec<u8>) -> Result<(), SessionError>` (once per local player per tick; refused for a remote player, a second call in the same tick, or more than `MAX_INPUT_BYTES`);
  - `Session::receive(&mut self, from: PeerId, packet: &[u8], now_micros: u64)`;
  - `Session::advance(&mut self, now_micros: u64) -> Vec<Request>`;
  - `Session::outgoing(&mut self) -> Vec<(PeerId, Vec<u8>)>`;
  - `Session::events(&mut self) -> Vec<SessionEvent>`;
  - `Session::state(&self) -> SessionState` (`Connecting`, `Running`, `Ended(EndReason)`);
  - `Session::stats(&self) -> SessionStats` (per peer: dropped packets by `WireError` variant name and by session rule, rollbacks, deepest rollback, prediction misses, stalls; filled further by Tasks 8 and 9);
  - `Request::{ Save { tick: u64, hash: bool }, Load { tick: u64 }, Advance { tick: u64, inputs: Vec<Vec<u8>> } }`; `Session::report_hash(&mut self, tick: u64, hash: u64)`;
  - `SessionEvent::{ Started, Confirmed { tick: u64, inputs: Vec<Vec<u8>> }, Ended(EndReason) }`, extended by Tasks 8 and 9;
  - `EndReason::{ Mismatch { peer: PeerId, field: &'static str }, Violation { peer: PeerId, rule: String } }`, extended by Tasks 8 and 9.
- Tick convention: `Advance { tick: t, inputs }` steps the state of tick `t` to `t + 1` with the inputs of tick `t`, one per global player. `Save { tick: t }` saves the state of tick `t`, before its inputs. Local inputs added during tick `t` apply at tick `t + input_delay`; ticks below `input_delay` use `default_input`.

- [ ] **Step 1: Write the failing tests**:
  - `handshake_starts_both_peers` and `handshake_refuses_mismatch`: one test per `MatchIdentity` field and for `net_version`, each ending the session with `EndReason::Mismatch` naming the field.
  - `two_peers_without_loss_never_roll_back`: with 0 latency both peers' confirmed inputs and model hashes match tick by tick for 600 ticks.
  - `late_input_rolls_back_to_the_first_misprediction`: a scripted remote input that differs from the prediction at tick 10 arriving at tick 13 produces `Load { tick: 10 }` then `Advance` for ticks 10 to 13 with the real input.
  - `correct_prediction_does_not_roll_back`.
  - `stalls_at_the_window`: with the remote peer silent, `advance` returns no `Advance` once the local tick is `rollback_window` ticks past the last confirmed tick, and `stats` counts the stall.
  - `redundant_inputs_survive_loss`: drop every other packet; all inputs are confirmed anyway.
  - `conflicting_input_disconnects_the_peer`, `undecodable_input_disconnects_the_peer` (through `check_input`), `input_for_a_player_the_peer_does_not_own_is_a_violation`, `input_beyond_the_window_is_a_violation`, `wrong_match_id_is_dropped_and_counted`: the Review Focus cases.
  - `save_requests_are_issued_for_every_tick_that_may_be_loaded`: no `Load { tick }` without an earlier `Save { tick }` since the last load before it.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test session`. Expected: compile failure.
- [ ] **Step 3: Implement** the input queue per player (confirmed inputs from the last confirmed tick, the last confirmed input as the prediction, the first incorrect prediction tick) in `queue.rs`, and the session around it. Keep saved-state bookkeeping to the ticks within `rollback_window + 2`.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): predict remote inputs and roll back on misprediction`.

---

### Task 8: Time synchronization and hash reports

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`
- Test: `crates/ludenea-net/tests/session_sync.rs`

**Interfaces:**
- Produces: `PeerStats { ping_micros: u64, jitter_micros: u64, frame_advantage: i32 }` in `SessionStats`; `SessionEvent::Desync { tick: u64, local: u64, remote: u64, peer: PeerId }`, after which the session ends with `EndReason::Desync { tick, peer }`.
- Rules (builder's call, named constants in `session.rs`): send `Quality` every 200 ms; ping is the mean of the last 16 round trips; the frame advantage of the local peer is its tick minus the remote's tick estimated at the current time (its last reported tick plus half the ping in ticks); when the mean advantage over the last 16 reports is at least 2 ticks, `advance` skips one tick (returns no `Advance`), at most once every 10 ticks. Hashes: each peer sends `Hash` for every tick that is a multiple of `hash_interval`, once the inputs of every tick before it are confirmed and that tick's latest `Save` happened after them; the session asks for it with `Save { hash: true }` and gets it through `report_hash`.

- [ ] **Step 1: Write the failing tests**: `ping_converges_to_the_conditioned_latency` (in-memory network, 100 ms latency each way: ping between 190 and 210 ms after 5 s); `faster_peer_slows_down` (one peer's `advance` called 2% more often: after 60 s neither peer stalls more than once per second and the tick gap stays within 3); `hashes_agree_without_desync`; `diverging_hash_ends_both_peers_with_desync` (the test model flips one peer's hash at tick 90: both peers get `Desync { tick: 90 }`); `hash_report_uses_the_state_resimulated_after_a_correction` (a rollback crossing tick 30 sends the hash of the resimulated state, not the predicted one).
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test session_sync`. Expected: compile failure.
- [ ] **Step 3: Implement** the rules above.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): synchronize peer clocks and compare state hashes`.

---

### Task 9: Disconnection, the agreed end, and the executor

**Files:**
- Create: `crates/ludenea-net/src/execute.rs`
- Modify: `crates/ludenea-net/src/session.rs`, `crates/ludenea-app/src/simulation.rs` (`Simulation::step_player_inputs`, `Simulation::encoded_default_input`, `Simulation::check_player_input`)
- Test: `crates/ludenea-net/tests/session_end.rs`, `crates/ludenea-net/tests/execute.rs` (with a small test app), `examples/arena/tests/rollback.rs` (the multi-peer property test on the arena)

**Interfaces:**
- Produces in `ludenea_app`: `Simulation::step_player_inputs(&mut self, inputs: &[Vec<u8>]) -> Result<(), SimError>` (one postcard-encoded input per player, each checked to decode exactly as the input type; the same result as `step_encoded` of the joined encoding); `Simulation::encoded_default_input(&self) -> Option<Vec<u8>>`; `Simulation::check_player_input(&self, bytes: &[u8]) -> Result<(), SimError>`.
- Produces in `ludenea_net`:
  - `Session::leave(&mut self)` (sends `Goodbye { last_tick }`, where `last_tick` is the local confirmed tick);
  - `SessionEvent::{ PeerSilent { peer: PeerId, for_micros: u64 }, PeerBack { peer: PeerId } }`;
  - `EndReason::{ Left { peer: PeerId, last_tick: u64 }, Lost { peer: PeerId } }`.

    On a `Goodbye`, every peer confirms up to `last_tick` and stops there, so the confirmed inputs of a normal end are identical everywhere. On a loss there is no agreement, and each peer ends at its own confirmed tick.
  - `StateRing::new(len: usize)` holding `SavedState`s by tick;
  - `execute(sim: &mut Simulation, ring: &mut StateRing, session: &mut Session, requests: Vec<Request>) -> Result<(), ExecuteError>`. It applies the requests in order, calls `report_hash` for `Save { hash: true }`, and on a game panic loads the last saved state, then returns `ExecuteError::Game { tick, error: SimError }`.

- [ ] **Step 1: Write the failing tests**:
  - `silent_peer_raises_a_notice_then_ends`: notice after 750 ms of silence, `Lost` after 3 s.
  - `goodbye_ends_everyone_at_the_same_tick`: the confirmed inputs are equal on both peers.
  - `step_player_inputs_equals_step_encoded`.
  - `two_three_and_four_peers_match_an_offline_run`: a property test over 32 seeds. Each case uses `MemoryNetwork` behind `Conditioned` (latency 0 to 150 ms, jitter 0 to 30 ms, loss 0 to 100 per mille, duplication 0 to 20 per mille), the arena app, and seeded random inputs per player, at 1 and 8 threads. The confirmed inputs of every peer are equal. Replaying them offline with `step_player_inputs` gives the same hash at every confirmed tick as each peer's executed state.
  - `panic_during_resimulation_is_reported_and_restores` (a test app whose system panics when an input byte is 0xFF, sent as a late remote input).
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test session_end --test execute` and `cargo test -p arena --test rollback`. Expected: compile failure.
- [ ] **Step 3: Implement** the end rules and the executor.
- [ ] **Step 4: Run** the gates, and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): end matches cleanly and drive a simulation from a session`.

---

### Task 10: SyncTest

**Files:**
- Create: `crates/ludenea-net/src/sync_test.rs`
- Modify: `crates/ludenea-cli/src/{lib.rs, commands.rs}` (`synctest` subcommand), `crates/ludenea-cli/Cargo.toml`, `.github/workflows/determinism.yml`, `README.md`, `CHANGELOG.md`
- Test: `crates/ludenea-net/tests/sync_test.rs` (test apps), `examples/arena/tests/rollback.rs` and `examples/boxes/tests/synctest.rs` (the example games), `crates/ludenea-cli/tests/synctest.rs`

**Interfaces:**
- Produces:
  - `sync_test(sim: &mut Simulation, inputs: &InputLog, edits: &EditLog, ticks: u64, window: u32) -> Result<SyncTestReport, SyncTestError>`. For every tick `t` from `window` on, it saves, runs ahead, loads the state of tick `t - window`, resimulates to `t` with `play_tick`, and compares each resimulated hash with the first run's.
  - `SyncTestReport { ticks: u64, rollbacks: u64 }`.
  - `SyncTestError::Mismatch { tick: u64, first: u64, again: u64, differences: Vec<Difference> }`, where `differences` is `World::diff_snapshots` of the two states of that tick. Its message lists the first 10 differences (entity, component, field) and the count of the rest.
  - `SyncTestError::Game { tick, error }`.
- Command: `<game> synctest <scene or .replay> [--project] [--edits] [--inputs] [--ticks N] [--window W] [--threads T] [--dump-dir D]`. The flags come from `RunFiles`, like `verify`. `--ticks` is required for a scene and defaults to the replay's length. `--window` defaults to 8. Output: `synctest passed: N ticks, R rollbacks`, exit 0. A mismatch prints the error, writes both snapshots to `--dump-dir` like `verify`, and exits 1.

- [ ] **Step 1: Write the failing tests**:
  - `boxes_and_arena_pass_synctest`.
  - `state_kept_outside_the_world_fails_synctest`: a test app whose system increments a runtime resource (never saved) and writes it into a component. The report names the tick, the entity, the component and the field.
  - `synctest_command_passes_on_the_arena_replay`.
  - `synctest_command_reports_a_mismatch_and_dumps_both_snapshots`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test sync_test`, `cargo test -p arena --test rollback`, `cargo test -p boxes --test synctest` and `cargo test -p ludenea-cli --test synctest`. Expected: compile failure.
- [ ] **Step 3: Implement**, then add CI steps on every operating system's job, at 1 and 8 threads, and under `wasmtime` for the arena:
  - `arena synctest golden/arena.replay`;
  - `swarm synctest` on its scene with `--ticks 600`;
  - `boxes synctest golden/spawner.replay`.

  Document the command in `README.md`, and add a Rust API and command line entry to the CHANGELOG.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): catch non-deterministic game code with synctest`.
