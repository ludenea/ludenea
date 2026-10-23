# Ludenea Milestone 4b: Online Play Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two to four players play a Ludenea game online through a room code, in native windows, standalone web pages and headless command line peers, over WebRTC data channels with signaling from `ludenea host`. Every peer drives its match through the control host, and every finished match is an ordinary replay that `verify` accepts.

**Architecture:** `ludenea-net` gains a link layer (the `Link` trait: a reliable room channel and an unreliable match channel between peers known by `LinkId`, plus an in-memory mesh) and a room (a sans-IO agreement on the peer order, the players and the match identifier), driven together by `OnlinePeer`. A new crate `ludenea-webrtc` implements `Link` with `matchbox_socket`, natively on a background executor and in the browser page. A new crate `ludenea-signal` is the matchbox-compatible signaling listener that `ludenea host` runs beside the read-only web page. The control host owns the rollback session of an online match: packets enter through `net_receive` and leave in `net_send` events, live devices map per local player, and the confirmed inputs become the session's input log when the match ends. The runner, the native window, a headless `peer` command and the web page (which owns the socket and bridges packets to its worker) all drive a match through those host commands.

**Tech Stack:** Rust 1.97.1. New dependencies, pinned exactly and justified in their commit: `matchbox_socket =0.14.0` (WebRTC data channels; natively through webrtc-rs, in the browser through `RTCPeerConnection`), `matchbox_protocol =0.14.0` with `json` (the signaling messages the listener speaks, so it cannot drift from the client), `futures =0.3.34` with `executor` only (runs the native message loop; already in `Cargo.lock` through matchbox). Reused: axum, tokio and tungstenite (as `ludenea serve` uses them, moved to workspace dependencies), uuid, wasm-bindgen-futures.

**Spec:** `.claude/superpowers/specs/2026-10-22-ludenea-networking-4-design.md`. This plan covers spec sections 4.3 (WebRTC and signaling), 4.4, 4.5, 7.1, 7.2 (the headless peer), 7.4 as the runtimes show it, and the cross-play end-to-end test of section 8. Milestone 4c covers the editor (host and join in play mode, the network panel, the desync view, opening a finished match), the exchange of hash trees and snapshots that turns a desync between peers into the full report, SyncTest through the host and the editor, and the online golden.

## Global Constraints

- Every Global Constraint of the earlier milestone and quality-bar plans still applies (determinism lints, target gates, committed generated files, `--locked`, commit style). The state hashes of `golden/` never move.
- `ludenea-net` stays without clock, floating point (`#![deny(clippy::float_arithmetic)]`) or unordered collection, builds for `wasm32-unknown-unknown` and `wasm32-wasip1`, and its tests run under `wasmtime`. Its new modules take the time as `now_micros: u64`.
- Runtime code that reads a clock (`Instant`) does so at each call site under `#[expect(clippy::disallowed_methods, reason = "network pacing, never feeds the simulation")]`, as the native window does.
- Everything from another machine is untrusted: room messages, signaling messages and packets are size-bounded and decoded defensively, and a bad one is dropped with a counted or logged reason or ends the connection, never a panic.
- Formats: the room messages join Network protocol 1, which is unreleased, so its CHANGELOG entry is updated and `NET_PROTOCOL_VERSION` stays 1. The control protocol stays 5: each change updates `PROTOCOL_FINGERPRINT`, the generated TypeScript bindings (`editor/packages/control-client/src/generated`), the bundled declarations (`node editor/scripts/bundle-dts.mjs`) and the protocol 5 CHANGELOG entry, per the README's versioning rules.
- Limits (builder's call): a room code is 6 characters of `23456789ABCDEFGHJKMNPQRSTUVWXYZ`; `MAX_ROOM_MESSAGE_BYTES = 512`; the listener holds at most `max_rooms = 64` rooms, `MAX_PEERS` (4) peers per room, signaling messages of at most 16 KiB, and closes a connection silent for 30 s (matchbox_socket sends a keep-alive every 10 s). Default listener port 3536, matchbox's own.
- URLs (builder's call): a room is `<signaling>/<code>`, and the peer that creates it appends `?host`. The native default signaling is `ws://127.0.0.1:3536/rooms`; the web page defaults to `ws://` or `wss://` on its own host, `/rooms`. The default STUN server is `stun:stun.l.google.com:19302`, a public Google server, documented as such; `--no-stun` and `?stun=none` use none.
- WebRTC channels: channel 0 unreliable and unordered carries match packets (`Channel::Match`), channel 1 reliable and ordered carries room messages (`Channel::Room`).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, `cargo xtask check`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task. Every CI lint step has its mirror in `CHECKS` of `xtask/src/main.rs`.
- No em-dash in any text this milestone writes (code, messages, titles, documentation).

## Deviations From The Spec

- A room is forgotten when its last peer leaves the listener, not once its peers are connected: matchbox_socket reports a peer that leaves the signaling server as disconnected, so peers stay connected to it for the whole match. The listener stops admitting peers once a room holds `MAX_PEERS`, and the hosting peer refuses peers beyond the room's size.
- The link conditioner is a launch setting of each runtime (command line flags, page parameters) applied to that peer's own sends, not a field of a host command: the transport it wraps belongs to the runtime. Conditioning both peers impairs both directions.
- The match rules are `MatchRules::default()` on every peer; no runtime setting changes them yet.
- While a match runs, the host keeps its confirmed inputs in a log of its own and the editor session stays parked at tick 0; when the match ends the log becomes the session's input log, which resimulates in the background as after any change at tick 0. Commands that read or change the session's timeline are refused meanwhile. Milestone 4c may show the confirmed head on the timeline.
- The cross-play end-to-end test of spec section 8 moves from 4c to this milestone: it is the first proof that the native and browser WebRTC stacks connect, a risk worth retiring early.

## Review Focus

- **A room that never fills or never meshes** (a peer that joins and never readies, two joiners that cannot reach each other): the host keeps waiting and shows how many peers it has; the headless peer gives up after `--room-timeout` naming what it waited for; nothing hangs without saying why. Tests in Tasks 5 and 11.
- **A Chromium peer whose host candidates are mDNS names, against a native peer**: they connect without browser flags. Test in Task 13; if webrtc-rs cannot resolve the names, stop and report before working around it.
- **Packets before the host joins the match, or after it ended** (a fast peer's `Hello` reaching a host still in its lobby): ignored without an error; the session's resends make up for them. Tests in Tasks 6 and 7.
- **A window closed in the middle of a match**: the peer leaves, so the others end with `Left` at once rather than `Lost` after 3 s. Test in Task 12. A closed browser tab gets the same attempt through `pagehide`, best effort, documented.
- **A game whose mapped input is longer than `MAX_INPUT_BYTES`**: `set_live_input` is refused naming the player, the size and the limit, and the previous input stays; the match never stalls on it. Test in Task 7.

---

### Task 1: What each advance did, and why a match ended

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`, `crates/ludenea-net/src/lib.rs`, `CHANGELOG.md` (Rust API)
- Test: `crates/ludenea-net/tests/rollback.rs`, `crates/ludenea-net/tests/time_sync.rs`, `crates/ludenea-net/tests/session_end.rs`

**Interfaces:**
- Produces:
  - `pub enum Advanced { Ran { tick: u64 }, Stalled, Skipped, AtLastTick, Landed }` and `Session::last_advance(&self) -> Option<Advanced>`: the outcome of the latest `advance` that returned `Ok`, `None` before any. `Ran { tick }` names the tick its new `Advance` stepped, so the simulation then stands at `tick + 1`; `Stalled` is a stall at the window (the caller keeps the time it owes); `Skipped` a time-sync skip (the time is spent); `AtLastTick` a match ending on purpose that ran every tick through its agreed last tick and waits for the other peers; `Landed` the advance that landed an ended session.
  - `Session::confirmed_tick(&self) -> u64`, public: the first tick some player lacks a confirmed input for.
  - `impl fmt::Display for EndReason`, with these messages (`PeerId` displays as `peer N`):
    - `Mismatch`: `{peer} plays another match: its {field} is {remote}, this peer's is {local}`
    - `Unreachable`: `{peer} never answered while the match was starting`
    - `Violation`: `{peer} {rule phrase}{suffix}, so the match ended`, the suffix ` (player P, tick T)` with whichever is known
    - `Desync`: `the game states diverged at tick {tick}: {found_by} has {local:016x}, {peer} has {remote:016x}`
    - `Lost`: `{peer} stopped answering, so the match ended`
    - `Left`: `{peer} left; the match ended at tick {last_tick}`
    - `LocalFailure`: `the game of {peer} failed at tick {tick}, so the match ended`
  - `impl fmt::Display for ViolationRule`: `NotOwner` "sent an input of a player it does not own", `ConflictingInput` "sent two different inputs for one tick", `InvalidInput` "sent an input the game cannot read", `BeyondHorizon` "sent an input past the furthest tick it can have reached", `AckNeverSent` "acknowledged an input or a hash this peer never sent", `AckLagging` "fell too far behind acknowledging this peer's inputs", `HashBeyondHorizon` "reported a state hash of a tick it cannot have confirmed", `HashLagging` "fell too far behind exchanging state hashes", `ImplausibleEnd` "announced an ending no honest peer can announce", `GoodbyeBehind` "left at a tick before one already confirmed".

- [ ] **Step 1: Write the failing tests**:
  - `last_advance_reports_ran_and_stalled` (`rollback.rs`): `Ran { tick }` with the advanced tick; with the remote silent, `Stalled` once at the window.
  - `skips_are_reported_as_skipped` (`time_sync.rs`, on the `faster_peer_slows_down` scenario): the number of `Skipped` outcomes equals `stats().skips`.
  - `last_advance_reports_the_end` (`session_end.rs`): a leaver at its last tick reports `AtLastTick`; the advance after an ending reports `Landed`.
  - `confirmed_tick_follows_the_confirmed_inputs`.
  - `end_reasons_name_the_peer_and_the_cause`: one exact message per `EndReason` variant, and one per `ViolationRule`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test rollback --test time_sync --test session_end`. Expected: compile failure.
- [ ] **Step 3: Implement**: `advance` records its outcome before returning; the messages above.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): tell the caller what each advance did and why a match ended`.

---

### Task 2: Map one player's raw input

**Files:**
- Modify: `crates/ludenea-app/src/app.rs` (`RawMapping` gains `encode_one`), `crates/ludenea-app/src/simulation.rs`, `CHANGELOG.md` (Rust API)
- Test: `crates/ludenea-app/tests/players.rs`

**Interfaces:**
- Produces: `Simulation::map_player(&self, player: usize, raw: Option<&RawInput>) -> Result<Vec<u8>, SimError>`: the postcard encoding of one input of global player `player`, mapped with that index as `map_players` maps it; `None` encodes `I::default()`. New `SimError::NoSuchPlayer { player: usize, players: usize }`, "player {player} does not exist: the game has {players} players". `SimError::NoInputMapping` without a mapping. A panic in the mapping is guarded like `map_players`.

- [ ] **Step 1: Write the failing tests**: `map_player_matches_map_players` (arena, 1 to 4 players, `Some` and `None` raw inputs: `join_player_inputs` of each `map_player` equals `map_players`), `map_player_output_passes_the_input_check` (`player_input_spec().check`), `map_player_refuses_a_missing_player`, `map_player_without_a_mapping_is_an_error`.
- [ ] **Step 2: Run** `cargo test -p ludenea-app --test players`. Expected: compile failure.
- [ ] **Step 3: Implement** `encode_one: Box<dyn Fn(Option<&RawInput>, usize) -> Vec<u8> + Send + Sync>` next to `encode` in `App::input_mapping`.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(app): map the raw input of one player`.

---

### Task 3: Links and the in-memory mesh

**Files:**
- Create: `crates/ludenea-net/src/link.rs`
- Modify: `crates/ludenea-net/src/transport.rs` (`TransportError::Unreachable`), `crates/ludenea-net/src/lib.rs`, `CHANGELOG.md`
- Test: `crates/ludenea-net/tests/link.rs`

**Interfaces:**
- Produces:
  - `LinkId(pub [u8; 16])`: `Copy`, `Ord`, serde as its 16 bytes, `Display` as a UUID (8-4-4-4-12 lowercase hexadecimal), as the signaling server names peers.
  - `Channel::{ Room, Match }`; `MAX_ROOM_MESSAGE_BYTES: usize = 512`.
  - `LinkEvent::{ Assigned(LinkId), Connected(LinkId), Disconnected(LinkId), Closed(String) }`: this peer's own id, a peer whose channels opened, a peer gone, the link itself gone with the reason.
  - `LinkError::{ UnknownLink { link }, TooLarge { len, max }, Closed }`.
  - `trait Link { fn poll(&mut self) -> Vec<LinkEvent>; fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError>; fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)>; }`. `Room` delivers reliably and in order per peer, at most `MAX_ROOM_MESSAGE_BYTES`; `Match` promises nothing, at most `MAX_PACKET_BYTES`. `impl<L: Link + ?Sized> Link for Box<L>`.
  - `MemoryMesh::new() -> MemoryMesh` (a shared, single-threaded handle): `join(&self) -> MemoryLink` connects to every member; `join_with(&self, peers: &[LinkId]) -> MemoryLink` only to those; `connect(&self, a: LinkId, b: LinkId)` connects two members later. Ids are assigned from a counter. A new link first reports `Assigned`, then `Connected` for each peer it connects to, which reports `Connected` back. Dropping a `MemoryLink` reports `Disconnected` to its peers.
  - `TransportError::Unreachable { peer }`: a peer of the match whose link is not connected; the packet is lost, as the network loses packets.
  - `LinkTransport<L: Link>::new(link: L, links: Vec<LinkId>) -> LinkTransport<L>`, a `Transport` over `Channel::Match` where `PeerId(i)` is `links[i]`: `UnknownPeer` beyond the list, `Unreachable` for `LinkError::UnknownLink`, `TooLarge` passed on. `receive` drops packets from links outside the list and counts them (`strays(&self) -> u64`). `link_mut(&mut self) -> &mut L`.

- [ ] **Step 1: Write the failing tests**: `mesh_assigns_then_connects_every_member`, `room_channel_keeps_the_order`, `dropping_a_link_disconnects_it`, `a_partial_mesh_connects_only_the_listed_members`, `sending_to_an_unknown_link_is_refused`, `oversized_messages_are_refused_per_channel`, `link_transport_maps_peers_to_links`, `link_transport_drops_and_counts_strangers`, `link_transport_reports_unreachable_peers`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test link`. Expected: compile failure.
- [ ] **Step 3: Implement** with `Rc<RefCell<_>>` queues in `BTreeMap`s keyed by `LinkId`.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): carry room and match messages over links`.

---

### Task 4: Room codes and room messages

**Files:**
- Create: `crates/ludenea-net/src/code.rs`
- Modify: `crates/ludenea-net/src/wire.rs`, `crates/ludenea-net/src/lib.rs`, `CHANGELOG.md` (the Network protocol 1 entry under Formats describes the room messages)
- Test: `crates/ludenea-net/tests/wire.rs`, `crates/ludenea-net/tests/code.rs`

**Interfaces:**
- Produces:
  - `ROOM_CODE_ALPHABET: &str = "23456789ABCDEFGHJKMNPQRSTUVWXYZ"`, `ROOM_CODE_LENGTH: usize = 6`.
  - `RoomCode` (`Clone`, `Eq`, `Ord`, `Display`, `as_str`): `RoomCode::from_entropy(entropy: u64) -> RoomCode` writes `entropy % 31^6` in base 31, most significant digit first; `RoomCode::parse(text: &str) -> Result<RoomCode, RoomCodeError>` trims ASCII whitespace and uppercases.
  - `RoomCodeError::{ Length { found: usize }, Character { character: char, position: usize } }`: "a room code has 6 characters, this one has {found}"; "'{character}' (character {position}) is not in a room code, which uses the digits 2 to 9 and the letters except I, L and O", `position` from 1.
  - `RoomMessage::{ Host { peers: u8 }, Ready { players: u8, connected: Vec<LinkId> }, Layout { match_id: u64, links: Vec<LinkId>, players: Vec<u8> }, Refuse { reason: RoomRefusal } }`, `RoomRefusal::{ Full, TooManyPlayers { total: u8 } }`.
  - `RoomMessage::encode(&self) -> Vec<u8>` (the `NET_PROTOCOL_VERSION` prefix, then postcard) and `RoomMessage::decode(bytes: &[u8]) -> Result<RoomMessage, WireError>`: `TooLarge` above `MAX_ROOM_MESSAGE_BYTES`, `Version`, `Malformed` (trailing bytes included, through `decode_exact`), and a new `WireError::TooManyPeers { count }` for a `connected` or `links` list longer than `MAX_PEERS` or a `players` list longer than `MAX_PEERS`.

- [ ] **Step 1: Write the failing tests**: `room_code_from_entropy_uses_the_alphabet` (6 characters; 1,000 consecutive entropies give 1,000 codes), `room_code_parse_accepts_lowercase_and_spaces`, `room_code_parse_refuses` (5 and 7 characters, `0`, `O`, `1`, `I`, `L`, `-`, each message as above), `room_messages_round_trip`, `room_decode_refuses` (one case per error, five links included), `fuzz_room_decode_never_panics` (the shape of `fuzz_decode_never_panics`, seeds `0..20_000`).
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test wire --test code`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): add room codes and the room messages`.

---

### Task 5: The room

**Files:**
- Create: `crates/ludenea-net/src/room.rs`
- Modify: `crates/ludenea-net/src/lib.rs`, `CHANGELOG.md`
- Test: `crates/ludenea-net/tests/room.rs` (a small driver: each `Room` over a `MemoryLink`, pumping `outgoing` into `Channel::Room` and link events and messages back in)

**Interfaces:**
- Consumes: `Link`, `MemoryMesh`, `LinkId` (Task 3); `RoomMessage` (Task 4); `ludenea_app::players::MAX_LOCAL_PLAYERS`.
- Produces:
  - `RoomRole::{ Host { peers: u8, match_id: u64 }, Join }`; `Room::new(role: RoomRole) -> Result<Room, RoomError>` (`peers` in `2..=MAX_PEERS`).
  - `Room::assigned(&mut self, me: LinkId)`, `connected(&mut self, link: LinkId)`, `disconnected(&mut self, link: LinkId)`, `receive(&mut self, from: LinkId, bytes: &[u8])`, `set_ready(&mut self, players: u8) -> Result<(), RoomError>`, `outgoing(&mut self) -> Vec<(LinkId, Vec<u8>)>`, `state(&self) -> &RoomState`, `view(&self) -> RoomView`, `events(&mut self) -> Vec<RoomEvent>`.
  - `RoomState::{ Waiting, Ready(MatchLayout), Failed(RoomError) }`; `RoomView { host: bool, peers: Option<u8>, connected: u8, ready: bool }` (`peers` is the room's size once known; `connected` counts this peer and those it is connected to).
  - `MatchLayout { local: PeerId, match_id: u64, players: Vec<PeerId>, links: Vec<LinkId> }` (`players`: the owner of each global player; `links`: each peer's link) and `MatchLayout::local_players(&self) -> Vec<u8>`.
  - `RoomError::{ Peers { peers }, LocalPlayers { players }, AlreadyReady, Full, TooManyPlayers { total }, TwoHosts { first: LinkId, second: LinkId }, HostLeft, InvalidLayout(String) }`, each message naming its values.
  - `RoomEvent::Dropped { link: LinkId, reason: RoomDrop }`, `RoomDrop::{ Wire(&'static str), Unexpected(&'static str) }` (the message kind a link may not send).
- Rules (in the module documentation):
  - The host sends `Host { peers }` to each link that connects. One that connects while `peers - 1` joiners are connected gets `Refuse { Full }` and is ignored.
  - A joiner takes the first link that sends `Host` as the host; a second one fails it with `TwoHosts`, and a host that receives `Host` fails the same way. Once ready and the host known, a joiner sends `Ready { players, connected }` (its other connected links, sorted) to the host, and again whenever that set changes before the layout.
  - The host lays out the match once it is ready, `peers - 1` joiners are connected and ready, and each joiner's `connected` holds every other joiner. Order: the host, then the joiners in the order they connected. Above `MAX_PLAYERS` players in total it sends `Refuse { TooManyPlayers }` to every joiner and fails; otherwise it sends `Layout` to every joiner and is `Ready` as `PeerId(0)`.
  - A joiner accepts a `Layout` from the host only when: both lists have one entry per peer, from 2 to `MAX_PEERS`; the links are distinct; `links[0]` is the host; its own link (from `assigned`) appears once, with the players it declared; each count is 1 to `MAX_LOCAL_PLAYERS` and the total at most `MAX_PLAYERS`. Otherwise it fails with `InvalidLayout` naming the broken rule.
  - Before the layout, the host forgets a joiner that disconnects, and a joiner whose host disconnects fails with `HostLeft`. A message a link may not send (a `Layout` or `Refuse` from a joiner, `Ready` to a joiner) is dropped as `Unexpected`; bytes that do not decode as `Wire`. After `Ready` or `Failed` the room ignores everything.
  - `set_ready` takes 1 to `MAX_LOCAL_PLAYERS`, once.

- [ ] **Step 1: Write the failing tests**: `two_three_and_four_peers_agree_on_one_layout` (same match id, links and players everywhere, distinct local ids, host first, joiners in connection order), `players_follow_each_declaration` (host 2, joiner 1: players `[0, 0, 1]`), `the_host_waits_for_a_complete_mesh` (two joiners joined with `join_with`, no layout until `connect`), `the_host_waits_for_its_own_ready`, `an_extra_peer_is_refused_as_full`, `too_many_players_fail_every_peer`, `two_hosts_fail`, `a_leaving_host_fails_the_joiners`, `a_joiner_leaving_before_the_layout_is_forgotten` (a new joiner takes its place), `invalid_layouts_fail_the_joiner` (one case per rule, sent from a fake host link), `unexpected_and_malformed_messages_are_dropped`, `set_ready_refuses` (0, 5, twice).
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test room`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): agree on the peers of a match in a room`.

---

### Task 6: One peer: its room, then its match, over one link

**Files:**
- Create: `crates/ludenea-net/src/online.rs`
- Modify: `crates/ludenea-net/src/lib.rs`, `CHANGELOG.md`
- Test: `crates/ludenea-net/tests/online.rs` (reuses `tests/support` for the model game and its input spec)

**Interfaces:**
- Consumes: `Room`, `MatchLayout` (Task 5); `LinkTransport` (Task 3); `Conditioned`, `LinkConditions`.
- Produces:
  - `OnlinePeer<L: Link>::new(link: L, role: RoomRole, conditions: LinkConditions) -> Result<OnlinePeer<L>, OnlineError>`, `OnlineError::{ Room(RoomError), Conditions(ConditionsError) }`. All-zero conditions add nothing.
  - `poll(&mut self, now_micros: u64) -> OnlineStatus`: drains the link's events into the room (`Closed(reason)` before the layout fails it: "the connection to the room closed: {reason}"), reads `Channel::Room`, sends the room's outgoing. Once the room is ready, the link moves into a `Conditioned<LinkTransport<L>>` over `layout.links`. Match packets that arrive before the layout are dropped.
  - `OnlineStatus::{ Room(RoomView), Ready(MatchLayout), Failed(String) }`: once `Ready`, it stays `Ready`; a link lost afterwards is the session's silence to handle.
  - `set_ready(&mut self, players: u8) -> Result<(), RoomError>`; `layout(&self) -> Option<&MatchLayout>`.
  - `receive(&mut self, now_micros: u64) -> Vec<(PeerId, Vec<u8>)>` and `send(&mut self, to: PeerId, packet: Vec<u8>, now_micros: u64)`: nothing before the layout; send failures counted (`failed_sends(&self) -> u64`), never returned.
  - `events(&mut self) -> Vec<RoomEvent>`.

- [ ] **Step 1: Write the failing tests**: `two_three_and_four_peers_reach_running_sessions` (each peer builds `SessionSettings` from its layout and a `Session` with the model input spec, then exchanges packets through `receive` and `send` while advancing, 1 ms steps: every session runs within 1 s and confirms 100 ticks), `conditions_delay_the_sends` (50 ms of latency: nothing arrives earlier), `a_closed_link_fails_the_room`, `match_packets_before_the_layout_are_dropped`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test online`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): drive a room and its match over one link`.

---

### Task 7: The host plays an online match

**Files:**
- Create: `crates/ludenea-control/src/online.rs` (the host's online match: session, executor, local players, confirmed log, clock)
- Modify: `crates/ludenea-control/Cargo.toml` (`ludenea-net`; dev-dependency `arena = { path = "../../examples/arena" }`), `crates/ludenea-control/src/{host.rs, protocol.rs, session.rs, lib.rs}`, `crates/ludenea-control/tests/support/mod.rs` (`every_command` gains the new commands), the generated bindings and bundled declarations, the editor's exhaustive matches over `Event` (the new events are ignored until 4c), `CHANGELOG.md` (control protocol 5 entry, Rust API)
- Test: `crates/ludenea-control/tests/online.rs` (the arena's `tests/fixtures/arena-four.scene.ron` with its project assets; N hosts over `MemoryNetwork`, behind `Conditioned` when a test says so, stepped 1/120 s per frame, each fed scripted `set_live_input` devices from a seeded `Rng`: a gamepad's left stick and jump button)

**Interfaces:**
- Consumes: `Session`, `Executor`, `Advanced`, `EndReason` Display (Task 1); `Simulation::map_player` (Task 2).
- Produces in the protocol:
  - `Command::JoinMatch { settings: OnlineSettings, claims: Vec<Claim> }` with `OnlineSettings { local: u8, match_id: Hash64, players: Vec<u8> }` (`players`: the owner of each global player); `Command::NetReceive { from: u8, packet: Bytes }`; `Command::LeaveMatch`. All three reply `done`.
  - `Event::Online { state: OnlineState }`, `OnlineState::{ Connecting, Running, Leaving, Ended { end: MatchEnd } }` (tagged `type`, snake case), `MatchEnd { kind: MatchEndKind, message: String, peer: Option<u8>, tick: Option<u64> }`, `MatchEndKind::{ Left, Lost, Desync, Violation, Mismatch, Unreachable, LocalFailure }`; `message` is the `EndReason` Display.
  - `Event::NetSend { packets: Vec<NetPacket> }`, `NetPacket { to: u8, packet: Bytes }`: after each advance that has packets.
  - `Event::RolledBack { from: u64, to: u64 }`: the ticks from `from` up to `to`, excluded, ran again with corrected inputs.
  - `ErrorCode::OnlineMatch`.
- Produces in `ludenea_control::Session`: `adopt_inputs(&mut self, sim: &mut Simulation, log: InputLog) -> Result<(), SessionError>`: replaces the inputs with `log`, drops the edits after tick 0 and the branches, and resimulates from tick 0 in the background, coming back to the end of `log` (`resimulate_from(sim, 0, end)`).
- Rules:
  - `join_match` is refused with `online_match` while a match is connecting, running or leaving; with `invalid_inputs` for a claim count other than the local players or claims `check_claims` refuses; with `invalid_command` when the session holds edits after tick 0 ("the session holds edits after tick 0, which the other peers do not have; load the scene without them") or the settings fail `SessionSettings::validate` (its message); with `no_input_mapping` for a game without one.
  - It then loads an empty input log of the match's players (tick 0), and builds the identity: `engine` is `env!("CARGO_PKG_VERSION")` (as `Welcome` names it), `fingerprint` is the schema fingerprint, `scene_and_assets` is the state hash at tick 0, plus the tick rate, the players, `MatchRules::default()` and `Timeouts::default()`. It builds the `Session` and the `Executor`, stops the pacer, and emits `Online { Connecting }`.
  - The clock: the host adds each `advance`'s elapsed time to `now_micros` in whole microseconds, carrying the fraction; a negative or non-finite time adds nothing.
  - `net_receive` passes the packet to the session at the current time; without a match, or after it ended, it is dropped and the reply is still `done`.
  - While online, `set_live_input` maps each local player's devices (`split` over the local claims) with `map_player` at its global index. An input longer than `MAX_INPUT_BYTES` is refused with `invalid_inputs`, "player P's input takes N bytes, a network match carries at most 24", and the previous inputs stay.
  - `advance` while online:
    - While connecting it only exchanges packets.
    - Once `Started`, it sets play on, emits `Online { Running }` and starts the pacer afresh.
    - For each tick the pacer owes, it adds every local player's input, calls `Session::advance`, then has the `Executor` execute the requests. A first request `Load { tick: m }` emits `RolledBack { from: m, to }`, where `to` is the tick the simulation stood at when that advance began.
    - `Ran` emits `Ticked` with the state hash, and the frames as local play pushes them. `Stalled` gives the rest of the owed ticks back (`Pacer::not_run`) and stops. `Skipped` goes on. `AtLastTick` and `Landed` stop.
    - Then `Confirmed` events append `join_player_inputs(inputs)` to the confirmed log, and the outgoing packets go out as one `NetSend`.
  - `leave_match` calls `Session::leave` and emits `Online { Leaving }`; outside a running match it is `invalid_command` naming the state.
  - When the session ends, the next advance lands it, then emits `Online { Ended }`, stops play and calls `adopt_inputs` with the confirmed log. An `ExecuteError::Game` also emits `Paused { Failed { tick, message } }`. A game panic the host catches while online first ends the match with `Session::fail`, then recovers as today.
  - While a match is connecting, running or leaving, every command `changes_state` names is refused with `online_match`, and so are `play`, `pause`, `seek`, `set_speed`, `step`, `export_replay`, `export_session`, `set_compare_branch` and `join_match`: "`{command}` is refused while an online match runs: it would change this peer's game alone; leave the match first". Reads, subscriptions, `set_live_input`, `set_threads`, `net_receive` and `leave_match` stay allowed.

- [ ] **Step 1: Write the failing tests**:
  - `two_hosts_play_and_end_at_the_same_state`, at 1 and 8 threads: both run, play 300 ticks, host 1 leaves; both end `left` at the same tick, and once each session resimulated (advance by zero while `get_timeline` says resimulating), `export_replay` gives byte-identical replays that `play_replay` verifies, holding exactly the confirmed inputs.
  - `three_hosts_under_latency_and_loss_record_the_same_replay`: behind `Conditioned`, 100 ms latency and 50 per mille loss.
  - `rollbacks_are_announced_and_ticks_stay_in_order`: under latency at least one `rolled_back`, and the `ticked` ticks rise by one.
  - `join_match_refuses`, one case per rule.
  - `packets_without_a_match_are_ignored`: before `join_match` and after the match ended, `net_receive` replies `done` and changes nothing.
  - `an_input_too_large_for_the_network_is_refused` (a test app whose input holds 30 bytes).
  - `local_changes_are_refused_during_a_match`, one command of each refused kind, then the same command accepted after the end.
  - `an_ended_match_becomes_the_session` (`seek` and `export_session` work after the resimulation).
  - Protocol: the round trip of `every_command`, and the fingerprint test.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test online --test protocol_v3 --test typescript`. Expected: compile failure.
- [ ] **Step 3: Implement**. Then regenerate the bindings and declarations as the README says, update the fingerprint and the protocol 5 entry, and make the editor ignore the new events.
- [ ] **Step 4: Run** the gates and `cargo test --locked -p ludenea-control`. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `✨ feat(control): play an online match through the host`.

---

### Task 8: Network statistics and silent peers on the control surface

**Files:**
- Modify: `crates/ludenea-control/src/{host.rs, online.rs, protocol.rs}` (`PROTOCOL_FINGERPRINT` included), `tests/support/mod.rs`, the generated bindings and declarations, `CHANGELOG.md` (control protocol 5 entry)
- Test: `crates/ludenea-control/tests/online.rs`

**Interfaces:**
- Produces:
  - `Command::GetNetStats` replying `net_stats`: `NetStats { tick: u64, confirmed: u64, skips: u64, peers: Vec<PeerNetStats> }`, `PeerNetStats { peer: u8, ping_micros: u64, jitter_micros: u64, frame_advantage: i32, rollbacks: u64, deepest_rollback: u64, prediction_misses: u64, stalls: u64, dropped: Vec<DroppedPackets> }`, `DroppedPackets { reason: String, count: u64 }` (the `DropReason` in words, such as `wire: too_large`). The statistics of an ended match stay readable until another match or session loads; with none, `invalid_command` "no online match ran".
  - `Event::PeerSilent { peer: u8, for_ms: u64 }` and `Event::PeerBack { peer: u8 }` from the session's events.
  - A dropped packet writes a console warning the first time each peer and reason occur in a match: "dropped a packet from peer N: {reason}"; later ones are only counted.

- [ ] **Step 1: Write the failing tests**: `net_stats_follow_the_match` (pings near the conditioned latency, the confirmed tick at most the current one, rollbacks counted), `a_silent_peer_is_announced_then_back` (a host stops delivering one peer's packets for 1 s), `dropped_packets_warn_once_per_reason`, `net_stats_without_a_match_are_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test online`. Expected: compile failure.
- [ ] **Step 3: Implement**, regenerate, update the fingerprint and the entry.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): report network statistics and silent peers`.

---

### Task 9: The signaling listener and `ludenea host`

**Files:**
- Create: `crates/ludenea-signal/{Cargo.toml, src/lib.rs, src/rooms.rs, src/files.rs}`
- Modify: root `Cargo.toml` (`axum`, `tungstenite`, `matchbox_protocol` and `ludenea-signal` as workspace dependencies), `Cargo.lock`, `crates/ludenea-serve/{Cargo.toml, src/main.rs, src/serve.rs}` (the static file helpers `serve_file`, `plain_name` and `content_type` move to `ludenea_signal::files`, used by both), `crates/ludenea-serve/src/host.rs` (new), `README.md` (a section on `ludenea host`), `CHANGELOG.md` (Rust API)
- Test: `crates/ludenea-signal/tests/signal.rs` (the server on `127.0.0.1:0` in a background tokio runtime, `tungstenite` clients on threads), `crates/ludenea-serve/tests/host.rs` (the binary)

**Interfaces:**
- Produces:
  - `ludenea_signal::Limits { max_rooms: usize, idle: Duration, max_message_bytes: usize }`, with `Default` giving 64, 30 s and 16 KiB.
  - `ludenea_signal::router(page: Option<PathBuf>, limits: Limits) -> axum::Router`.
  - `ludenea_signal::serve(listener: tokio::net::TcpListener, page: Option<PathBuf>, limits: Limits) -> Result<(), String>`.
- Rules:
  - `GET /rooms/{code}` upgrades to a WebSocket speaking matchbox's JSON (`matchbox_protocol::{JsonPeerEvent, JsonPeerRequest}`). `?host` creates the room.
  - Refused before the upgrade:
    - 400 for a code `RoomCode::parse` refuses, with its message;
    - 409 "room CODE is in use" for `?host` on a room that exists;
    - 404 "no room CODE" to join a room that does not exist;
    - 409 "room CODE is full" at `MAX_PEERS`;
    - 503 "this server holds its maximum of N rooms".
  - Like `matchbox_server`'s full mesh:
    - the server sends `IdAssigned` (a v4 UUID) first, then `NewPeer` of the newcomer to every member already there;
    - it relays `Signal` only to a member of the same room, and drops others;
    - it ignores `KeepAlive`;
    - it sends `PeerLeft` to the remaining members when a connection ends;
    - it forgets a room left empty.
  - A message over `max_message_bytes` closes the connection with code 1009, one that is not a JSON `PeerRequest` with 1008, and a connection silent for `idle` with 1001.
  - Every other path serves the page folder read-only (GET and HEAD; `/` is `index.html`; any `..` or separator in a segment is 400; other methods 405). Without a page it answers 404 "no page here: pass the folder `ludenea build --web` wrote".
  - `ludenea host [--page DIR] [--listen ADDRESS] [--port PORT] [--idle-seconds N] [--max-rooms N]`:
    - `--listen` defaults to `0.0.0.0` (the LAN) and `--port` to 3536; 0 picks a free port.
    - A page folder without `game.json` is refused at launch: "DIR holds no web build: write one with `ludenea build <project> --web --out DIR`".
    - It prints `signaling ws://ADDRESS:PORT/rooms`, then `page http://ADDRESS:PORT/` with a page. ADDRESS is the machine's LAN address, taken from the local address of a UDP socket connected to `192.0.2.1` (nothing is sent), or `127.0.0.1` without a network.
    - It warns that anyone on the network can reach it, and that it serves the page read-only and relays room signaling.

- [ ] **Step 1: Write the failing tests** (`signal.rs`): `a_host_and_a_joiner_meet` (`IdAssigned`, `NewPeer` to the host, `Signal` relayed both ways with the sender's id), `a_leaving_peer_is_announced`, `joining_a_missing_room_is_404`, `a_second_host_is_409`, `a_fifth_peer_is_409`, `an_invalid_code_is_400`, `rooms_are_bounded` (`max_rooms: 2`), `an_oversized_message_closes`, `a_malformed_message_closes`, `an_idle_connection_closes` (`idle` 1 s), `signals_to_another_room_are_dropped`, `an_empty_room_is_forgotten`, `page_files_are_served_read_only`. In `host.rs`: `host_prints_its_urls_and_serves_the_page`, `a_folder_without_a_web_build_is_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-signal` and `cargo test -p ludenea-serve --test host`. Expected: compile failure.
- [ ] **Step 3: Implement**: rooms in a `BTreeMap` behind a `Mutex`, one unbounded sender per member.
- [ ] **Step 4: Run** the gates; `cargo test -p ludenea-serve` passes unchanged. Expected: PASS.
- [ ] **Step 5: Commit** `♻️ refactor(serve): share static file serving with a new signaling crate`, then `✨ feat(signal): relay WebRTC signaling between the peers of a room` (the body justifies `matchbox_protocol`), then `✨ feat(serve): add ludenea host`.

---

### Task 10: The WebRTC link

**Files:**
- Create: `crates/ludenea-webrtc/{Cargo.toml, src/lib.rs, src/native.rs, src/web.rs}`
- Modify: root `Cargo.toml` (`matchbox_socket = "=0.14.0"`, `futures = { version = "=0.3.34", default-features = false, features = ["executor"] }`), `Cargo.lock`, `xtask/src/main.rs` `CHECKS` and `.github/workflows/determinism.yml` (both: `-p ludenea-webrtc` on the existing `wasm32-unknown-unknown --lib` clippy line), `README.md` (STUN, and the strict NAT limitation: no TURN relay), `CHANGELOG.md` (Rust API)
- Test: `crates/ludenea-webrtc/tests/webrtc.rs` (native; dev-dependencies `ludenea-signal`, `tokio`)

**Interfaces:**
- Consumes: `Link`, `LinkEvent`, `Channel`, `LinkError`, `RoomCode`, `OnlinePeer`.
- Produces:
  - `DEFAULT_STUN: &str = "stun:stun.l.google.com:19302"`.
  - `LinkSettings { signaling: String, code: RoomCode, host: bool, ice_urls: Vec<String> }` and `LinkSettings::room_url(&self) -> String`: the signaling URL without its trailing slash, `/`, the code, and `?host` for the host.
  - `WebRtcLink::open(settings: LinkSettings) -> WebRtcLink`: a matchbox socket with channel 0 `ChannelConfig::unreliable()` as `Channel::Match`, channel 1 `ChannelConfig::reliable()` as `Channel::Room`, the ICE URLs as its one ICE server, and one connection attempt (a refused room is final). It implements `Link`:
    - `poll` reports `Assigned` once the socket has its id, `Connected` and `Disconnected` from `update_peers`, and `Closed` once the message loop ended;
    - `LinkId` is the matchbox `PeerId`'s UUID bytes.
  - Natively the message loop runs on a thread named `ludenea-webrtc` under `futures::executor::block_on`; dropping the link closes the socket, which ends the loop. In the browser it runs through `wasm_bindgen_futures::spawn_local`.
  - `Closed` reasons, natively, from the HTTP status of the refused upgrade (match `SignalingError::NegotiationFailed` holding `SignalingError::WebSocket`; depend on the `async-tungstenite` version matchbox_socket 0.14.0 uses, natively only, so the type matches):
    - 404: "no room {code} at {signaling}: check the code";
    - 409 as host: "room {code} is in use at {signaling}: host again for a new code";
    - 409 as joiner: "room {code} is full";
    - 400: "the signaling server at {signaling} refused the room code {code}";
    - 503: "the signaling server at {signaling} holds too many rooms";
    - otherwise: "cannot reach the signaling server at {signaling}: {error}".
  - In the browser, which hides the status: "cannot join room {code} at {signaling}: the server refused or cannot be reached (an unknown or full room, a code in use, or no server there)".

- [ ] **Step 1: Write the failing tests** (real WebRTC on this machine, no STUN, each bounded at 30 s with a message saying what it waited for):
  - `two_links_connect_and_carry_both_channels`: a host and a joiner through an in-process listener. Both are assigned, then connected. 100 room messages arrive in order. 100 match packets arrive, all of them, since loopback loses none; on a timeout the failure says how many did.
  - `three_online_peers_agree_on_a_layout`.
  - `a_leaving_peer_disconnects`.
  - `an_unknown_room_closes_naming_it`, `a_room_in_use_closes_naming_it`, `an_unreachable_server_closes_naming_it` (the port of a closed listener).
  - A unit test of `room_url` (trailing slash, host query).
- [ ] **Step 2: Run** `cargo test -p ludenea-webrtc`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates, `cargo xtask check` and `cargo clippy --locked --target wasm32-unknown-unknown -p ludenea-webrtc --lib -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(webrtc): connect peers over WebRTC data channels`, the body justifying `matchbox_socket` (one stack for native and browser peers, as the spec requires; its native WebRTC is webrtc-rs) and `futures`.

---

### Task 11: The runner online, and a headless peer

**Files:**
- Create: `crates/ludenea-runtime/src/online.rs`
- Modify: `crates/ludenea-runtime/{Cargo.toml, src/lib.rs, src/runner.rs}` (`ludenea-net`; `ludenea-webrtc` under the `native` feature on native targets and the `web` feature in the browser, never on `wasm32-wasip1`; `uuid` under `native` only, since the page passes its own entropy), `crates/ludenea-cli/src/{lib.rs, commands.rs}` (`peer`; the random devices of `--random-inputs` share `RANDOM_KEYS` with `synctest`), `README.md`, `CHANGELOG.md`
- Test: `crates/ludenea-runtime/tests/online.rs` (runners over a `MemoryMesh`), `examples/arena/tests/online.rs` (two `arena peer` processes through an in-process listener; dev-dependencies `ludenea-signal`, `tokio`), `crates/ludenea-cli/tests/peer.rs`

**Interfaces:**
- Consumes: `OnlinePeer`, `MatchLayout`, `RoomCode` (Tasks 4 to 6); the host commands and events (Tasks 7 and 8); `WebRtcLink` (Task 10).
- Produces on `Runner`:
  - `join_match(&mut self, settings: OnlineSettings, claims: Vec<Claim>) -> Result<(), RuntimeError>`.
  - `net_receive(&mut self, from: u8, packet: Vec<u8>)`.
  - `take_net_packets(&mut self) -> Vec<(u8, Vec<u8>)>`.
  - `leave_match(&mut self) -> Result<(), RuntimeError>`.
  - `online(&self) -> Option<&OnlineState>` and `silent_peers(&self) -> &BTreeMap<u8, u64>`.
  - `frame` keeps the `net_send` packets and the online events; an ended match is not an error.
  - `export_replay` advances by zero while the host answers `resimulating`, at most `ticks / RESIMULATION_BUDGET + 2` times, else a `Record` error naming the tick reached.
- Produces in `ludenea_runtime::online`:
  - `OnlineOptions { role: OnlineRole, signaling: String, ice_urls: Vec<String>, conditions: LinkConditions }`.
  - `OnlineRole::{ Host { peers: u8, code: Option<RoomCode> }, Join { code: RoomCode } }`.
  - `settings(layout: &MatchLayout) -> OnlineSettings`.
  - `OnlineMatch<L: Link>`:
    - `new(link: L, role: RoomRole, code: RoomCode, conditions: LinkConditions) -> Result<_, RuntimeError>`, `code(&self) -> &RoomCode`;
    - `ready(&mut self, claims: Vec<Claim>) -> Result<(), RuntimeError>`;
    - `before_frame(&mut self, runner: &mut Runner, now_micros: u64) -> OnlinePhase`: polls the room, joins the runner to the match once laid out, and delivers the received packets;
    - `after_frame(&mut self, runner: &mut Runner, now_micros: u64)`: sends the runner's packets;
    - `leave(&mut self, runner: &mut Runner)`.
  - `OnlinePhase::{ Room(RoomView), Playing, Ended(MatchEnd), Failed(String) }`.
  - `open_online(options: &OnlineOptions, entropy: u128) -> Result<OnlineMatch<WebRtcLink>, RuntimeError>` (native and browser): the room code from the low 64 bits (unless the host gave one), the match identifier from the high 64. Native callers pass `uuid::Uuid::new_v4().as_u128()`.
- Command: `<game> peer <SCENE> [--project P] [--threads T] [--players K] [--random-inputs SEED] [--ticks N] [--record FILE] [--room-timeout SECONDS] <online options>`.
  - The online options are shared with `play` in Task 12 (a flattened `OnlineArgs`): `--host`, `--peers N` (2 to 4, default 2, with `--host`), `--room CODE` (with `--host`, instead of a random code), `--join CODE`, `--signaling URL`, `--stun URL` (repeatable, default `DEFAULT_STUN`), `--no-stun`, `--latency-ms`, `--jitter-ms`, `--loss-percent` (0 to 100).
  - Exactly one of `--host` and `--join` is required. `--players` defaults to 1, with the automatic claims. `--room-timeout` defaults to 60.
  - It runs headless in real time, stepping every 4 ms. `--random-inputs` plays seeded devices each frame: the keys of `RANDOM_KEYS` held one frame in four, and one gamepad per local player with random buttons and axes. Without it, the players hold nothing.
  - It leaves once the match reaches tick N, and ends when the match ends.
  - Output, one line each: `room CODE` (as host, at once, flushed); `match started: peer P of N, local players [..]`; `match ended at tick T: {message}`; the per-peer statistics (`get_net_stats`); `wrote FILE`.
  - Exit 0 when the match ended as `left`, 1 for any other ending, and 2 for an error: a load failure, a room failure (its message), or a room not ready within the timeout ("room CODE was not ready within S s: C of N peers connected").

- [ ] **Step 1: Write the failing tests**:
  - `two_runners_play_online_and_export_the_same_replay` (`runtime/tests/online.rs`, the arena fixture scene, 1/120 s frames).
  - `two_arena_peers_play_and_verify` (`examples/arena/tests/online.rs`): `CARGO_BIN_EXE_arena peer` as host with `--room` and as joiner, `--no-stun --random-inputs 1 and 2 --ticks 300 --threads 1 and 8 --record`. Both exit 0, the replays are byte-identical, and `arena verify` accepts them.
  - `peer_refuses_bad_options` (`cli/tests/peer.rs`): no role, both roles, `--peers 5`, `--join 0O0O0O`, each naming the problem.
  - `peer_reports_an_unknown_room` (exit 2, the 404 message).
  - `peer_gives_up_on_an_empty_room` (`--room-timeout 1`).
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --test online`, `cargo test -p arena --test online` and `cargo test -p ludenea-cli --test peer`. Expected: compile failure.
- [ ] **Step 3: Implement**. Document `peer` in `README.md` with a two-terminal example through `ludenea host`.
- [ ] **Step 4: Run** the gates and `cargo xtask check`. Expected: PASS; `wasm32-wasip1` builds of the runtime and the games link no WebRTC.
- [ ] **Step 5: Commit** `✨ feat(runtime): drive an online match from the runner`, then `✨ feat(cli): add a headless online peer`.

---

### Task 12: The native window online

**Files:**
- Modify: `crates/ludenea-runtime/src/native/{window.rs, lobby.rs}`, `crates/ludenea-cli/src/commands.rs` (`play` takes `OnlineArgs`; `--edits` conflicts with `--host` and `--join`), `README.md` (playing online: `ludenea host`, then `ludenea play <project> -- --host` and `-- --join CODE --signaling ws://ADDRESS:3536/rooms`), `.github/workflows/determinism.yml` (the Linux step "Play and verify a match in the native window" gains an online match)
- Test: unit tests in `window.rs` with an `OnlineMatch<MemoryLink>` in place of the WebRTC one

**Interfaces:**
- Consumes: `OnlineMatch`, `OnlinePhase`, `open_online` (Task 11).
- Produces:
  - `PlayOptions::online: Option<OnlineOptions>` and `PlayOutcome::ended: Option<MatchEnd>`. The window's session holds an `OnlineMatch<Box<dyn Link>>`.
  - The window opens on the lobby as today. As host it prints `room CODE: give this code to the other players; signaling at URL`.
  - The title shows the room through `lobby::room_title(title, code, view)`: "{title}: room CODE, C of N peers, press Enter when everyone has joined", then "waiting for the other peers" once ready. During the match it shows "{title}: online, peer P of N", plus "peer Q silent for S s" while one is. After the end it shows "{title}: match ended: {message}".
  - Enter (or `--players`) readies the room with the claims instead of starting the match. Each frame runs `before_frame`, the runner's frame, then `after_frame`. The match's first frame is fetched once the runner joined.
  - Closing the window during a match leaves it, then keeps pumping until the match ends or one second passes, so the others end with `Left`.
  - `--ticks N` leaves at tick N and closes the window once the match ended. `--record` writes the replay after the end.
  - The command exits 0 for a match ended as `left` or a window closed before any match, 1 for any other ending (the message printed), and 2 for failures as today.

- [ ] **Step 1: Write the failing tests**: `the_online_lobby_readies_the_room_then_plays` (two window sessions over one mesh, without windows, as the existing tests drive `Session`), `closing_the_window_leaves_the_match` (the other side ends `left`), `the_title_follows_the_room_and_the_match`, `an_online_match_writes_its_replay_after_the_end`, and in `cli/tests/cli.rs` `play_refuses_edits_online`.
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --features native` and `cargo test -p ludenea-cli --test cli`. Expected: compile failure.
- [ ] **Step 3: Implement**. Then add to the CI step, under Xvfb:
  - `cargo build --locked --release -p ludenea-serve -p arena`;
  - `target/release/ludenea host --listen 127.0.0.1 &`;
  - two `arena play examples/arena/assets/arena.scene.ron --players 1 --ticks 600 --no-stun`, one `--host --room 23456A`, the other `--join 23456A`, each with its own `--record`;
  - `arena verify` on both replays, and `cmp` of the two.
- [ ] **Step 4: Run** the gates, `cargo xtask check`, and the manual check below. Expected: PASS.
  - Manual check: `ludenea host`, then two windows on this machine, one hosting and one joining. Play, close the joining window, and see the host end with "peer 1 left".
- [ ] **Step 5: Commit** `✨ feat(runtime): host and join online matches in the native window`.

---

### Task 13: The standalone web page online, and cross-play

**Files:**
- Modify: `crates/ludenea-runtime/src/web.rs` (`WebOnline`), `crates/ludenea-control/src/worker.rs` (`netReceive`, `takeNetPackets`), `crates/ludenea-project/web/{index.html, page.js, worker.js, style.css}`, `README.md` (page parameters, the Leave button, the limits of a hidden tab and of a closed tab), `CHANGELOG.md` (Rust API: `WebOnline`, the `WorkerHost` methods)
- Create: `editor/e2e/online.spec.ts`
- Test: `editor/e2e/online.spec.ts` (Chromium; the arena page that `cargo run --locked -p ludenea-serve -- build examples/arena --web --out editor/e2e/fixtures/arena` writes; `target/debug/ludenea host --listen 127.0.0.1 --port 0` spawned per file, its URL read from its output; `target/debug/arena`)

**Interfaces:**
- Consumes: `open_online`, `settings` (Task 11); the host commands and events (Tasks 7 and 8).
- Produces:
  - `WebOnline` (wasm-bindgen, page side):
    - `new WebOnline(options: string, entropy: Uint32Array)`. `options` is JSON: `{ "role": { "host": peers } | { "join": code }, "signaling": url, "stun": [urls], "conditions": { "latency_ms", "jitter_ms", "loss_percent" } }`. Bad options throw a message naming the field.
    - getter `code`;
    - `poll(now_ms: f64) -> string`: JSON, one of `{ "state": "room", "peers", "connected", "ready" }`, `{ "state": "ready", "settings": OnlineSettings }` and `{ "state": "failed", "message" }`;
    - `setReady(players: u32)`;
    - `receive(now_ms: f64) -> Array` of `{ from, packet: ArrayBuffer }`;
    - `send(to: u8, packet: Uint8Array, now_ms: f64)`.
  - `WorkerHost::net_receive(&mut self, from: u8, packet: &[u8]) -> Result<(), JsValue>` (`netReceive`, no JSON) and `take_net_packets(&mut self) -> Option<Array>` (`takeNetPackets`: `{ to, packet: ArrayBuffer }`, taken out of `advance`'s events like the render frames).
  - The worker:
    - takes `{ type: "join", settings, claims }`, `{ type: "net", packets }` and `{ type: "leave" }`;
    - posts `{ type: "net", packets }` (transferred), `{ type: "online", state }`, `{ type: "silent", peer, ms }` and `{ type: "back", peer }`;
    - on `export_replay` while the host answers `resimulating`, advances by zero and asks again.
  - The page:
    - Parameters: `?host=N` (2 to 4) or `?join=CODE`; `?signaling=URL` (default `ws(s)://<its host>/rooms`); `?stun=URL` (repeatable, `none` for none); `?latency=MS&jitter=MS&loss=PERCENT`. A bad one fails the page naming it.
    - The lobby shows the room in `#room`, with `data-room` on the body. `data-online` takes `room`, `connecting`, `running`, `leaving` and `ended`, and `data-end` the end's kind.
    - Readying the room replaces starting the match; `?players=N` readies it at once.
    - A `#leave` button leaves the match. `pagehide` sends `leave`, best effort.
    - The link is pumped every 4 ms (`setInterval`), apart from the animation frames.
    - After the end, the status shows the end's message and "Save replay" saves the match.

- [ ] **Step 1: Write the failing tests** (`online.spec.ts`):
  - `two pages play the arena and save the same replay`: page A with `?host=2&players=1&stun=none&signaling=...`, page B joining with A's `data-room`. Both reach `running`; A holds ArrowRight for a second, both reach tick 300, and A clicks Leave. Both end `left` at the same `data-tick`. The saved replays are byte-identical, and `arena verify` accepts them.
  - `a native peer and a page play under bad conditions` (spec section 8): `arena peer --host --room <random> --players 1 --random-inputs 7 --ticks 600 --no-stun --latency-ms 50 --jitter-ms 10 --loss-percent 5 --record` against a page `?join=CODE&players=1&stun=none&latency=50&jitter=10&loss=5`. The peer exits 0, the page ends `left`, the replays are byte-identical, and `arena verify` accepts both. No browser flag disables mDNS candidates.
  - `an unknown room says so`.
  - `bad parameters fail the page naming them` (`?host=9`, `?join=0O0O0O`, `?loss=150`).
- [ ] **Step 2: Run** the e2e prerequisites of the README, then `pnpm -C editor e2e online.spec.ts`. Expected: FAIL.
- [ ] **Step 3: Implement**. List `online.spec.ts` in the README's end-to-end paragraph.
- [ ] **Step 4: Run** the gates, `cargo xtask check`, and `pnpm -C editor e2e standalone.spec.ts online.spec.ts`. Expected: PASS; the local page tests unchanged.
- [ ] **Step 5: Commit** `✨ feat(web): host and join online matches from the standalone page`, then `✅ test(web): play the arena between a native peer and a browser`.
