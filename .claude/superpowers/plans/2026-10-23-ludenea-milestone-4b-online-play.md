# Ludenea Milestone 4b: Online Play Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two to four players play a Ludenea game online through a room code, in native windows, standalone web pages and headless command line peers, over WebRTC data channels with signaling from `ludenea rooms`. Every peer drives its match through the control host, and every finished match is an ordinary replay that `verify` accepts.

**Architecture:** `ludenea-net` gains a link layer and a room:
- the `Link` trait: a reliable room channel and an unreliable match channel between peers known by `LinkId`, plus an in-memory mesh;
- the room: a sans-IO agreement on the peer order, the players and the match identifier;
- `OnlinePeer`, which drives both.

A new crate `ludenea-webrtc` implements `Link` with `matchbox_socket` (vendored with a small patch), natively on a background executor and in the browser page. A new crate `ludenea-signal` is the matchbox-compatible signaling listener; `ludenea rooms` runs it beside the read-only web page.

The control host owns the rollback session of an online match:
- packets enter through `net_receive` and leave in `net_send` events;
- live devices map per local player;
- the confirmed inputs become the session's input log when the match ends.

The runner, the native window, a headless `peer` command and the web page all drive a match through those host commands. The page owns the socket and bridges packets to its worker.

**Tech Stack:** Rust 1.97.1. New dependencies, pinned exactly and justified in their commit:
- `matchbox_socket =0.14.0`: WebRTC data channels, natively through webrtc-rs, in the browser through `RTCPeerConnection`. Vendored under `vendor/matchbox_socket` with a patch.
- `matchbox_protocol =0.14.0` with `json`: the signaling messages the listener speaks, so it cannot drift from the client.
- `futures =0.3.34` with `executor` only: the native message loop.
- `async-tungstenite =0.32.1` without default features, native only: the type of a refused WebSocket upgrade, as matchbox reports it.

Reused: axum, tokio and tungstenite (as `ludenea serve` uses them, moved to workspace dependencies), hyper-util (already under axum), uuid, wasm-bindgen-futures.

**Spec:** `.claude/superpowers/specs/2026-10-22-ludenea-networking-4-design.md`. This plan covers spec sections:
- 4.3 (WebRTC and signaling), 4.4, 4.5, 6, 7.1;
- 7.2 (the headless peer);
- 7.4 as the runtimes show it;
- the cross-play end-to-end test of section 8.

Milestone 4c covers:
- the editor: host and join in play mode, the network panel, the desync view, opening a finished match;
- the exchange of hash trees and snapshots that turns a desync between peers into the full report;
- SyncTest through the host and the editor;
- the online golden.

## Global Constraints

- Every Global Constraint of the earlier milestone and quality-bar plans still applies (determinism lints, target gates, committed generated files, `--locked`, commit style). The state hashes of `golden/` never move.
- `ludenea-net` stays without clock, floating point (`#![deny(clippy::float_arithmetic)]`) or unordered collection. It builds for `wasm32-unknown-unknown` and `wasm32-wasip1`, and its tests run under `wasmtime`. Its new modules take the time as `now_micros: u64`.
- Runtime code that reads a clock (`Instant`) does so at each call site under `#[expect(clippy::disallowed_methods, reason = "network pacing, never feeds the simulation")]`, as the native window does.
- Everything from another machine is untrusted: room messages, signaling messages, SDP and ICE candidates, and packets. Each is size-bounded and decoded defensively. A bad one is dropped with a counted or logged reason, or ends that connection. Never a panic, in the native process or in the browser's WebAssembly module.
- Formats:
  - The room messages join Network protocol 1, which is unreleased: its CHANGELOG entry is updated and `NET_PROTOCOL_VERSION` stays 1.
  - The control protocol stays 5. Each change updates `PROTOCOL_FINGERPRINT`, the generated TypeScript bindings (`editor/packages/control-client/src/generated`), the bundled declarations (`node editor/scripts/bundle-dts.mjs`) and the protocol 5 CHANGELOG entry, per the README's versioning rules.
  - The editor's exhaustive matches over `Event` and `Reply` ignore the new variants until 4c.
- Limits (builder's call):
  - A room code is 6 characters of `23456789ABCDEFGHJKMNPQRSTUVWXYZ`. `MAX_ROOM_MESSAGE_BYTES = 512`.
  - The listener holds at most `max_rooms = 64` rooms of at most the size their host announced (2 to `MAX_PEERS`).
  - Signaling messages are at most 16 KiB, with at most 64 queued per member.
  - The listener closes a connection silent for 30 s (matchbox_socket sends a keep-alive every 10 s), and a request whose headers are not in after 10 s.
  - Per client address: 4 hosted rooms, 30 refused joins a minute.
  - The default listener port is 3536, matchbox's own.
- URLs (builder's call):
  - A room is `<signaling>/<code>`; the peer that creates it appends `?host=N`, N being its size.
  - The native default signaling is `ws://127.0.0.1:3536/rooms`. The web page defaults to `ws://` or `wss://` on its own host, `/rooms`.
  - The default STUN server is `stun:stun.l.google.com:19302`, a public Google server, documented as such. `--no-stun` and `?stun=none` use none.
- WebRTC channels: channel 0, unreliable and unordered, carries match packets (`Channel::Match`). Channel 1, reliable and ordered, carries room messages (`Channel::Room`).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, `cargo xtask check`, `pnpm -C editor test` and `pnpm -C editor typecheck` pass at the end of every task. Every CI lint step has its mirror in `CHECKS` of `xtask/src/main.rs`.
- No em-dash in any text this milestone writes (code, messages, titles, documentation).

## Deviations From The Spec

- **The room's lifetime.** A room is forgotten when its last peer leaves the listener, not once its peers are connected. matchbox_socket closes every data channel when its signaling connection ends, so peers stay connected to the listener for the whole match: stopping `ludenea rooms` ends the matches it serves, as the README says. The listener caps each room at the size its host announced, and the host refuses peers beyond it.
- **The signaling command is `ludenea rooms`**, not `ludenea host`: a game's command line already has a `host` subcommand (the editor's process), and `--host` hosts a room.
- **The link conditioner is a launch setting of each runtime**, applied to that peer's own sends, not a field of a host command: the transport it wraps belongs to the runtime. Conditioning both peers impairs both directions.
  - The runtime flags set latency, jitter and loss.
  - Duplication and extra reordering stay at zero there (jitter already reorders); the in-process tests drive them.
  - The seed comes from the peer's entropy.
- **The match rules** are `MatchRules::default()` on every peer; no runtime setting changes them yet.
- **The session during a match.** Joining replaces the session's inputs, edits and branches with an empty log of the match's players, as `set_players` does. While the match runs, the host keeps the confirmed inputs in a log of its own, and the session stays parked at tick 0.
  - Commands that read or change the session's recorded run are refused meanwhile; reads of the world are not.
  - When the match ends, the log becomes the session's input log, which resimulates in the background, as after any change at tick 0.
  - The local match and the online match still share the host's pacer, claims and session: the online match only defers the session's recording to its end.
  - Milestone 4c may show the confirmed head on the timeline.
- **A desync between peers** ends the match naming the tick, both peers and both hashes. The automatic exchange that names the entity, component and field is 4c's. Until then, each runtime writes this peer's snapshot of the divergent tick, `desync-tick-T-peer-P.snap`, taken from the adopted session. The end message tells the player to compare it with the other peer's using `<game> diff`, which names the entity, component and field.
- **The cross-play end-to-end test** of spec section 8 moves from 4c to this milestone: it is the first proof that the native and browser WebRTC stacks connect, a risk worth retiring early.

## Review Focus

- **A room that never fills or never meshes** (a joiner that never readies, two joiners that cannot reach each other, a peer that brings too many players): the host lays out with the joiners that qualify and keeps saying what it waits for. The headless peer gives up after `--room-timeout`, quoting that. Tests in Tasks 5 and 11.
- **A hostile room member or hostile signaling data** (a lying `Ready`, a `Layout` naming links never connected, malformed SDP or ICE candidates, room message floods): the peer never panics and never grows without bound. Tests in Tasks 5, 6 and 10.
- **A match that ends in any state**: a version mismatch while connecting, an unreachable peer, a local game failure, a desync, a lost peer, a leaver. Every peer reaches `Ended`, keeps telling the others for the linger time, and keeps a replay that `verify` accepts. Tests in Tasks 7 and 11.
- **A window closed in the middle of a match**: the peer leaves and pumps until its session ends, so the others end with `Left` at the same tick and its own replay is complete. Test in Task 12. A closed browser tab gets the same attempt through `pagehide`, best effort, documented.
- **A game whose mapped input is longer than `MAX_INPUT_BYTES`**: `set_live_input` is refused naming the player, the size and the limit, and the previous input stays; the match never stalls on it. Test in Task 7.

---

### Task 1: What each advance did, and why a match ended

**Files:**
- Modify: `crates/ludenea-net/src/session.rs`, `crates/ludenea-net/src/lib.rs`, `CHANGELOG.md` (Rust API)
- Test: `crates/ludenea-net/tests/rollback.rs`, `crates/ludenea-net/tests/time_sync.rs`, `crates/ludenea-net/tests/session_end.rs`

**Interfaces:**
- Produces:
  - `pub enum Advanced { Ran { tick: u64 }, Stalled, Skipped, AtLastTick, Landed { tick: u64 } }` and `Session::last_advance(&self) -> Option<Advanced>`: the outcome of the latest `advance` that returned `Ok`, `None` before any.
    - `Ran { tick }`: its new `Advance` stepped `tick`, so the simulation then stands at `tick + 1`.
    - `Stalled`: a stall at the window; the caller keeps the time it owes, the stalled tick included.
    - `Skipped`: a time-sync skip; the time is spent.
    - `AtLastTick`: a match ending on purpose that ran every tick through its agreed last tick and waits for the other peers.
    - `Landed { tick }`: the call landed an ended session at `tick`. A call that ran a tick and then landed reports `Landed`.
  - `Session::confirmed_tick(&self) -> u64` (the first tick some player lacks a confirmed input for) and `Session::next_tick(&self) -> u64` (the next tick the session advances), both public.
  - `impl fmt::Display for EndReason`, with these messages (`PeerId` displays as `peer N`):
    - `Mismatch`: `{peer} plays another match: its {field} is {remote}, this peer's is {local}`
    - `Unreachable`: `{peer} never answered while the match was starting`
    - `Violation`: `{peer} {rule phrase}{suffix}, so the match ended`, the suffix ` (player P, tick T)` with whichever is known
    - `Desync`: `the game states diverged at tick {tick}: {found_by} has {local:016x}, {peer} has {remote:016x}`
    - `Lost`: `{peer} stopped answering, so the match ended`
    - `Left`: `{peer} left; the match ended at tick {last_tick}`
    - `LocalFailure`: `the game of {peer} failed at tick {tick}, so the match ended`
  - `impl fmt::Display for ViolationRule`, one phrase per variant:

    | Variant | Phrase |
    | --- | --- |
    | `NotOwner` | sent an input of a player it does not own |
    | `ConflictingInput` | sent two different inputs for one tick |
    | `InvalidInput` | sent an input the game cannot read |
    | `BeyondHorizon` | sent an input past the furthest tick it can have reached |
    | `AckNeverSent` | acknowledged an input or a hash this peer never sent |
    | `AckLagging` | fell too far behind acknowledging this peer's inputs |
    | `HashBeyondHorizon` | reported a state hash of a tick it cannot have confirmed |
    | `HashLagging` | fell too far behind exchanging state hashes |
    | `ImplausibleEnd` | announced an ending no honest peer can announce |
    | `GoodbyeBehind` | left at a tick before one already confirmed |

- [ ] **Step 1: Write the failing tests**:
  - `last_advance_reports_ran_and_stalled` (`rollback.rs`): `Ran { tick }` with the advanced tick; with the remote silent, `Stalled` once at the window.
  - `skips_are_reported_as_skipped` (`time_sync.rs`, on the `faster_peer_slows_down` scenario): the number of `Skipped` outcomes equals `stats().skips`.
  - `last_advance_reports_the_end` (`session_end.rs`): a leaver at its last tick reports `AtLastTick`; the advance that lands reports `Landed` with the confirmed tick, also when it ran a tick first. A session ended while connecting, or by `fail`, returns `Err(NotRunning)` from `advance` and never lands.
  - `confirmed_and_next_ticks_follow_the_session`.
  - `end_reasons_name_the_peer_and_the_cause`: one exact message per `EndReason` variant, and one per `ViolationRule`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test rollback --test time_sync --test session_end`. Expected: compile failure.
- [ ] **Step 3: Implement**: `advance` records its outcome before returning; the messages above.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): tell the caller what each advance did and why a match ended`.

---

### Task 2: Map one player's raw input

**Files:**
- Modify: `crates/ludenea-app/src/app.rs` (`RawMapping` gains `encode_one`), `crates/ludenea-app/src/simulation.rs`, `crates/ludenea-app/Cargo.toml` (dev-dependency `arena = { path = "../../examples/arena" }`), `CHANGELOG.md` (Rust API)
- Test: `crates/ludenea-app/tests/players.rs`

**Interfaces:**
- Produces:
  - `Simulation::map_player(&self, player: usize, raw: Option<&RawInput>) -> Result<Vec<u8>, SimError>`: the postcard encoding of one input of global player `player`, mapped with that index as `map_players` maps it. `None` encodes `I::default()`.
  - New `SimError::NoSuchPlayer { player: usize, players: usize }`: "player {player} does not exist: the game has {players} players".
  - Without a mapping: `SimError::NoInputMapping`. A panic in the mapping is guarded like `map_players`.

- [ ] **Step 1: Write the failing tests**:
  - `map_player_matches_map_players`: arena, 1 to 4 players, `Some` and `None` raw inputs. `join_player_inputs` of each `map_player` equals `map_players`.
  - `map_player_output_passes_the_input_check` (`player_input_spec().check`).
  - `map_player_refuses_a_missing_player`.
  - `map_player_without_a_mapping_is_an_error`.
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
  - `LinkId(pub [u8; 16])`: `Copy`, `Ord`, serde as its 16 bytes. `Display` as a UUID (8-4-4-4-12 lowercase hexadecimal), as the signaling server names peers.
  - `Channel::{ Room, Match }`; `MAX_ROOM_MESSAGE_BYTES: usize = 512`.
  - `LinkEvent::{ Assigned(LinkId), Connected(LinkId), Disconnected(LinkId), Closed(String) }`: this peer's own id, a peer whose channels opened, a peer gone, the link itself gone with the reason.
  - `LinkError::{ UnknownLink { link }, TooLarge { len, max }, Closed }`.
  - `trait Link`, with `impl<L: Link + ?Sized> Link for Box<L>`:
    - `fn poll(&mut self) -> Vec<LinkEvent>`;
    - `fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError>`;
    - `fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)>`.

    `Room` delivers reliably and in order per peer, at most `MAX_ROOM_MESSAGE_BYTES`. `Match` promises nothing, at most `MAX_PACKET_BYTES`. A send to a link not currently `Connected` is `UnknownLink`.
  - `MemoryMesh::new() -> MemoryMesh`, a shared, single-threaded handle:
    - `join(&self) -> MemoryLink` connects to every member;
    - `join_with(&self, peers: &[LinkId]) -> MemoryLink` connects only to those;
    - `connect(&self, a: LinkId, b: LinkId)` connects two members later;
    - `disconnect(&self, a: LinkId, b: LinkId)` cuts two members apart.

    Ids come from a counter. A new link first reports `Assigned`, then `Connected` for each peer it connects to, which reports `Connected` back. Dropping a `MemoryLink` reports `Disconnected` to its peers.
  - `TransportError::Unreachable { peer }`: a peer of the match whose link is not connected; the packet is lost, as the network loses packets.
  - `LinkTransport<L: Link>::new(link: L, links: Vec<LinkId>) -> LinkTransport<L>`, a `Transport` over `Channel::Match` where `PeerId(i)` is `links[i]`:
    - a peer beyond the list is `UnknownPeer`, `LinkError::UnknownLink` becomes `Unreachable`, and `TooLarge` is passed on;
    - `receive` drops packets from links outside the list and counts them (`strays(&self) -> u64`);
    - `link_mut(&mut self) -> &mut L`.

- [ ] **Step 1: Write the failing tests**: `mesh_assigns_then_connects_every_member`, `room_channel_keeps_the_order`, `dropping_a_link_disconnects_it`, `a_partial_mesh_connects_only_the_listed_members`, `disconnect_cuts_two_members_apart`, `sending_to_an_unconnected_link_is_refused`, `oversized_messages_are_refused_per_channel`, `link_transport_maps_peers_to_links`, `link_transport_drops_and_counts_strangers`, `link_transport_reports_unreachable_peers`.
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
  - `RoomCode` (`Clone`, `Eq`, `Ord`, `Display`, `as_str`):
    - `RoomCode::from_entropy(entropy: u64) -> RoomCode` writes `entropy % 31^6` in base 31, most significant digit first;
    - `RoomCode::parse(text: &str) -> Result<RoomCode, RoomCodeError>` trims ASCII whitespace and uppercases.
  - `RoomCodeError::{ Length { found: usize }, Character { character: char, position: usize } }`:
    - "a room code has 6 characters, this one has {found}";
    - "'{character}' (character {position}) is not in a room code, which uses the digits 2 to 9 and the letters except I, L and O", `position` counted from 1.
  - `RoomMessage::{ Host { peers: u8 }, Ready { players: u8, connected: Vec<LinkId> }, Layout { match_id: u64, links: Vec<LinkId>, counts: Vec<u8> }, Refuse { reason: RoomRefusal } }`. `counts` is the local players of each peer, in peer order.
  - `RoomRefusal::{ Full, TooManyPlayers { total: u8 }, Unplayable }`.
  - `RoomMessage::encode(&self) -> Vec<u8>`: the `NET_PROTOCOL_VERSION` prefix, then postcard.
  - `RoomMessage::decode(bytes: &[u8]) -> Result<RoomMessage, WireError>`:
    - `TooLarge` above `MAX_ROOM_MESSAGE_BYTES`;
    - `Version`;
    - `Malformed`, trailing bytes included, through `decode_exact`;
    - a new `WireError::TooManyPeers { count }` for a `connected`, `links` or `counts` list longer than `MAX_PEERS`.
  - `WireError::TooLarge` becomes `{ len, max }`, so its message names the limit of what was decoded (1200 for a packet, 512 for a room message). `WireError::variant` names `TooManyPeers`.

- [ ] **Step 1: Write the failing tests**:
  - `room_code_from_entropy_uses_the_alphabet`: 6 characters, and 1,000 consecutive entropies give 1,000 codes.
  - `room_code_parse_accepts_lowercase_and_spaces`.
  - `room_code_parse_refuses`: 5 and 7 characters, `0`, `O`, `1`, `I`, `L`, `-`, each message as above.
  - `room_messages_round_trip`.
  - `room_decode_refuses`: one case per error, five links included.
  - `fuzz_room_decode_never_panics`: the shape of `fuzz_decode_never_panics`, seeds `0..20_000`.
  - In `wire.rs`, the existing `TooLarge` cases updated to name 1200.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test wire --test code`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): add room codes and the room messages`.

---

### Task 5: The room

**Files:**
- Create: `crates/ludenea-net/src/room.rs`
- Modify: `crates/ludenea-net/src/lib.rs`, `CHANGELOG.md`
- Test: `crates/ludenea-net/tests/room.rs` (a small driver: each `Room` over a `MemoryLink`, pumping `outgoing` into `Channel::Room`, and link events and messages back in)

**Interfaces:**
- Consumes: `Link`, `MemoryMesh`, `LinkId` (Task 3); `RoomMessage` (Task 4); `SessionSettings::validate`; `ludenea_app::players::MAX_LOCAL_PLAYERS`.
- Produces:
  - `RoomRole::{ Host { peers: u8, match_id: u64 }, Join }`.
  - `Room::new(role: RoomRole, rules: MatchRules) -> Result<Room, RoomError>`, with `peers` in `2..=MAX_PEERS`. `rules` serve only to check the layout; `OnlinePeer` passes `MatchRules::default()`, the rules every peer's host uses.
  - `Room` methods:
    - `assigned(&mut self, me: LinkId)`, `connected(&mut self, link: LinkId)`, `disconnected(&mut self, link: LinkId)`;
    - `receive(&mut self, from: LinkId, bytes: &[u8])`;
    - `set_ready(&mut self, players: u8) -> Result<(), RoomError>`;
    - `outgoing(&mut self) -> Vec<(LinkId, Vec<u8>)>`, `state(&self) -> &RoomState`, `view(&self) -> RoomView`, `events(&mut self) -> Vec<RoomEvent>`.
  - `RoomState::{ Waiting, Ready(MatchLayout), Failed(RoomError) }`.
  - `RoomView { host: bool, peers: Option<u8>, connected: u8, ready: u8, unmeshed: u8, unfit: u8, local_ready: bool }`, counting this peer in `connected` and `ready`:
    - `peers`: the room's size, once known;
    - `ready`: the peers ready (the host counts itself and its ready joiners);
    - `unmeshed` (host only): pairs of joiners whose `Ready` reports they are not connected to each other;
    - `unfit` (host only): ready joiners whose players do not fit.

    `RoomView::describe(&self) -> String` says what the room waits for, the one text every runtime shows and every timeout quotes. Host: "2 of 3 peers here, 1 not ready, 1 pair of peers not connected to each other". Joiner: "connected to 2 of 3 peers, waiting for the host to start".
  - `MatchLayout { local: PeerId, match_id: u64, players: Vec<PeerId>, links: Vec<LinkId> }` (`players`: the owner of each global player; `links`: each peer's link) and `MatchLayout::local_players(&self) -> Vec<u8>`.
  - `RoomError`, each message naming its values: `Peers { peers }`, `LocalPlayers { players }`, `AlreadyReady`, `Full`, `TooManyPlayers { total }`, `Unplayable`, `TwoHosts { first: LinkId, second: LinkId }`, `HostLeft`, `InvalidLayout(String)`.
  - `RoomEvent::Dropped { link: LinkId, reason: RoomDrop }`, `RoomDrop::{ Wire(&'static str), Unexpected(&'static str), Invalid(&'static str) }`. `Unexpected` is a message kind this link may not send; `Invalid` a field out of range.
- Rules (in the module documentation):
  - **The host's announcement.**
    - The host sends `Host { peers }` to each link that connects, in every state, before anything else it sends that link.
    - After the layout, or while `peers - 1` joiners are already in the layout candidates, a newcomer gets `Refuse { Full }` right after `Host`.
  - **Joiners and their host.**
    - A joiner takes the first link that sends `Host` as its host. A repeat from the same link is ignored.
    - A `Host` from another link fails the joiner with `TwoHosts`. A host drops a `Host` it receives as `Unexpected`, so one joiner cannot fail it.
    - A joiner accepts `Layout` and `Refuse` only from its host; from anyone else they are `Unexpected`.
  - **Readiness.**
    - Once ready and its host known, a joiner sends `Ready { players, connected }` to the host, `connected` being its other connected links, sorted. It sends it again whenever that set changes before the layout.
    - The host drops a `Ready` whose `players` is outside `1..=MAX_LOCAL_PLAYERS`, or whose `connected` lists more than `MAX_PEERS - 1` links, as `Invalid`.
    - A `Ready` whose players can never fit (its players plus the host's plus one per other missing peer above `MAX_PLAYERS`) gets `Refuse { TooManyPlayers }` at once.
  - **The layout.**
    - Once the host is ready, it picks joiners in connection order. A joiner qualifies when it is ready, is connected to every joiner already picked, those report being connected to it, and its players still fit in `MAX_PLAYERS`.
    - When it holds `peers - 1` of them, it builds the layout: the host, then the picked joiners in that order.
    - It checks the layout with `SessionSettings::validate`, on settings built from it with the room's rules, the default timeouts and placeholder identity values. With the default rules every layout of at most `MAX_PLAYERS` players passes; the check guards other rules. A refusal sends `Refuse { Unplayable }` to the picked joiners and fails the host with `Unplayable`, naming the settings error.
    - Otherwise it sends `Layout` to the picked joiners, `Refuse { Full }` to every other connected joiner, and is `Ready` as `PeerId(0)`.
  - **The joiner's checks.** A joiner accepts a `Layout` only when:
    - both lists have the size its host announced;
    - the links are distinct and `links[0]` is the host;
    - its own link (from `assigned`, or the layout is `InvalidLayout("own link unknown")`) appears once, with the players it declared;
    - every other link of the layout is connected to it;
    - each count is 1 to `MAX_LOCAL_PLAYERS`, and the total at most `MAX_PLAYERS`.

    Otherwise it fails with `InvalidLayout`, naming the broken rule.
  - **Departures.** Before the layout, the host forgets a joiner that disconnects, and a joiner whose host disconnects fails with `HostLeft`.
  - **Bad messages.** Bytes that do not decode are dropped as `Wire`.
  - **After the end.** After `Ready` or `Failed`, the room still answers newcomers as above and drops everything else, counted.
  - **`set_ready`** takes 1 to `MAX_LOCAL_PLAYERS`, once.

- [ ] **Step 1: Write the failing tests**:
  - `two_three_and_four_peers_agree_on_one_layout`: the same match id, links and players everywhere, distinct local ids, the host first, joiners in connection order.
  - `players_follow_each_declaration`: host 2, joiner 1, so players `[0, 0, 1]`; host 1, joiner 2, so `[0, 1, 1]`.
  - `the_host_waits_for_a_complete_mesh`: two joiners joined with `join_with`; no layout until `connect`, and `describe` names the unconnected pair.
  - `the_host_waits_for_its_own_ready`.
  - `a_joiner_that_never_readies_is_passed_over`: three joiners in a 3-peer room, the first never readies. The other two are laid out, and the first gets `Full`.
  - `an_extra_peer_is_refused_as_full`, before and after the layout.
  - `a_joiner_that_cannot_fit_is_refused` and `the_host_picks_joiners_that_fit`.
  - `an_unplayable_layout_fails_every_peer`: rules with `rollback_window = 24` and a joiner with 3 local players, whose worst packet does not fit.
  - `a_joiners_host_message_is_dropped_by_the_host`, `two_hosts_fail_their_joiners`, `a_repeated_host_message_is_ignored`.
  - `a_leaving_host_fails_the_joiners`.
  - `a_joiner_leaving_before_the_layout_is_forgotten`: a new joiner takes its place.
  - `invalid_layouts_fail_the_joiner`: one case per check, sent from the host link.
  - `layouts_and_refusals_from_another_link_are_dropped`.
  - `invalid_ready_messages_are_dropped`.
  - `malformed_messages_are_dropped`.
  - `set_ready_refuses`: 0, 5, twice.
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
- Consumes: `Room`, `MatchLayout`, `RoomView` (Task 5); `LinkTransport` (Task 3); `Conditioned`, `LinkConditions`.
- Produces:
  - `OnlinePeer<L: Link>::new(link: L, role: RoomRole, conditions: LinkConditions) -> Result<OnlinePeer<L>, OnlineError>`, with `OnlineError::{ Room(RoomError), Conditions(ConditionsError) }`. All-zero conditions add nothing.
  - `poll(&mut self, now_micros: u64) -> OnlineStatus`:
    - It drains the link's events into the room. Before the layout, `Closed(reason)` fails the room: "the connection to the room closed: {reason}".
    - It reads every `Channel::Room` message, before and after the layout, so a flood never accumulates, and sends the room's outgoing.
    - Once the room is ready, the link moves into a `Conditioned<LinkTransport<L>>` over `layout.links`. The room keeps answering newcomers through `link_mut`.
    - Match packets that arrive before the layout are dropped.
    - After the layout, `Closed(reason)` is kept: `closed(&self) -> Option<&str>`, for the runtimes to add to the end's message.
  - `OnlineStatus::{ Room(RoomView), Ready(MatchLayout), Failed(String) }`. Once `Ready`, it stays `Ready`; a link lost afterwards is the session's silence to handle.
  - `set_ready(&mut self, players: u8) -> Result<(), RoomError>`; `layout(&self) -> Option<&MatchLayout>`.
  - `receive(&mut self, now_micros: u64) -> Vec<(PeerId, Vec<u8>)>` and `send(&mut self, to: PeerId, packet: Vec<u8>, now_micros: u64)`: nothing before the layout. Send failures are counted, never returned.
  - Counters for the runtimes to report: `failed_sends(&self) -> u64`, `strays(&self) -> u64`, `room_drops(&mut self) -> Vec<RoomEvent>`.

- [ ] **Step 1: Write the failing tests**:
  - `two_three_and_four_peers_reach_running_sessions`: each peer builds `SessionSettings` from its layout and a `Session` with the model input spec, then exchanges packets through `receive` and `send` while advancing, 1 ms steps. Every session runs within 1 s and confirms 100 ticks.
  - `conditions_delay_the_sends`: 50 ms of latency, nothing arrives earlier.
  - `a_closed_link_fails_the_room`, `a_link_closed_after_the_layout_is_kept_for_the_message`.
  - `match_packets_before_the_layout_are_dropped`.
  - `a_room_flood_after_the_layout_is_drained`: 10,000 room messages, nothing kept, all counted.
  - `a_newcomer_after_the_layout_is_refused_as_full`.
- [ ] **Step 2: Run** `cargo test -p ludenea-net --test online`. Expected: compile failure.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** the gates and the `ludenea-net` tests under `wasmtime`. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(net): drive a room and its match over one link`.

---

### Task 7: The host plays an online match

**Files:**
- Create: `crates/ludenea-control/src/online.rs` (the host's online match: session, executor, local players and their mapped inputs, confirmed log, clock)
- Modify:
  - `crates/ludenea-control/Cargo.toml`: `ludenea-net`, and the dev-dependency `arena = { path = "../../examples/arena" }`.
  - `crates/ludenea-control/src/{host.rs, protocol.rs, session.rs, lib.rs}`.
  - `crates/ludenea-control/tests/support/mod.rs`: `every_command` gains the new commands.
  - The generated bindings and bundled declarations; the editor's exhaustive matches.
  - `CHANGELOG.md`: the control protocol 5 entry, Rust API.
- Test: `crates/ludenea-control/tests/online.rs`:
  - the arena's `tests/fixtures/arena-four.scene.ron` with its project assets;
  - N hosts over `MemoryNetwork`, behind `Conditioned` when a test says so, stepped 1/120 s per frame;
  - each fed scripted `set_live_input` devices from a seeded `Rng`: a gamepad's left stick and jump button.

**Interfaces:**
- Consumes: `Session`, `Executor`, `Advanced`, `EndReason` Display (Task 1); `Simulation::map_player` (Task 2).
- Produces in the protocol:
  - `Command::JoinMatch { settings: OnlineSettings, claims: Vec<Claim> }`, with `OnlineSettings { local: u8, match_id: Hash64, players: Vec<u8> }` (`players`: the owner of each global player).
  - `Command::NetReceive { from: u8, packet: Bytes }` and `Command::LeaveMatch`.
  - All three reply `done`.
  - `Event::Online { state: OnlineState }`, with:
    - `OnlineState::{ Connecting, Running, Leaving, Ended { end: MatchEnd } }`, tagged `type`, snake case;
    - `MatchEnd { kind: MatchEndKind, message: String, peer: Option<u8>, tick: Option<u64> }`;
    - `MatchEndKind::{ Left, Lost, Desync, Violation, Mismatch, Unreachable, LocalFailure }`.

    `message` is the `EndReason` Display. `peer` and `tick` come from the reason: `Mismatch`, `Unreachable` and `Lost` give the peer; `Violation` the peer and its tick; `Desync` the peer whose hash differed and the tick; `Left` the peer and its last tick; `LocalFailure` the peer and the tick.
  - `Event::NetSend { packets: Vec<NetPacket> }`, `NetPacket { to: u8, packet: Bytes }`: after each advance that has packets, the ended session's lingering included.
  - `Event::RolledBack { from: u64, to: u64 }`: the ticks from `from` up to `to`, excluded, ran again with corrected inputs.
  - `ErrorCode::OnlineMatch`.
- Produces in `ludenea_control::Session`: `adopt_inputs(&mut self, sim: &mut Simulation, log: InputLog) -> Result<(), SessionError>`.
  - It replaces the inputs with `log`, and resimulates them from tick 0 within the host's resimulation budget per advance, never all at once whatever the log's length.
  - Until it gets back to tick `log.ticks.len()`, it shows tick 0. There every tick has its recorded hash.
  - It sets the internal state this needs explicitly: the stale run from tick 0, the tick to come back to, and an end of the session that reaches `log.ticks.len()`.
- Rules:
  - **Joining.**
    - `join_match` is refused:
      - with `online_match` while a match is connecting, running or leaving;
      - with `invalid_inputs` for a claim count other than the local players, or claims `check_claims` refuses;
      - with `invalid_command` when the session holds edits after tick 0 ("the session holds edits after tick 0, which the other peers do not have; load the scene without them"), or the settings fail `SessionSettings::validate`, or `Session::new` or `Executor::new` refuses (each with its message);
      - with `no_input_mapping` for a game without one.
    - It then stops play, sets the speed to 1, loads an empty input log of the match's players (tick 0, as `set_players` does), and builds the identity:
      - `engine`: `env!("CARGO_PKG_VERSION")`, as `Welcome` names it;
      - `fingerprint`: the schema fingerprint;
      - `scene_and_assets`: the state hash at tick 0;
      - the tick rate, the players, `MatchRules::default()` and `Timeouts::default()`.
    - It builds the `Session` and the `Executor`, starts the pacer afresh, and emits `Online { Connecting }`. Each local player's input starts as `map_player(global, None)`.
  - **The clock.** The host adds each `advance`'s elapsed time to `now_micros` in whole microseconds, carrying the fraction. A negative or non-finite time adds nothing.
  - **Receiving.** `net_receive` passes the packet to the session at the current time. Without a match, it is dropped and the reply is still `done`; an ended session ignores it itself.
  - **Live devices.** While online, `set_live_input` maps each local player's devices (`split` over the local claims) with `map_player` at its global index. An input longer than `MAX_INPUT_BYTES` is refused with `invalid_inputs`, "player P's input takes N bytes, a network match carries at most 24", and the previous inputs stay.
  - **`changes_state`** gains `net_receive`, `leave_match` and `get_net_stats` among its exclusions; `join_match` stays a change.
  - **`advance` while a match is online**, until it is finished:
    1. It advances the clock.
    2. While running or leaving, for each tick the pacer owes:
       - while running, it adds every local player's input (not while leaving, where the session takes none);
       - it calls `Session::advance` and has the `Executor` execute the requests;
       - a first request `Load { tick: m }` emits `RolledBack { from: m, to }`, where `to` is the tick the simulation stood at when that advance began.

       Then by `last_advance`:
       - `Ran` emits `Ticked` with the state hash, and the frames as local play pushes them;
       - `Stalled` gives the stalled tick and the rest back (`Pacer::not_run`) and stops;
       - `Skipped` goes on;
       - `AtLastTick` stops;
       - `Landed { tick }` emits `Ticked` for `tick` and stops.

       `MissingLocalInput` or `LastTick` from `Session::advance` is a host bug: a console error, then the match fails at the latest tick it ran.
    3. When the session has ended, and no `Landed` came in this advance, it calls `Session::advance` once more, whatever the pacer owes. `Ok` is the landing: execute it and emit `Ticked` for the landed tick. `Err(NotRunning)` means nothing lands: an ending while connecting, or through `fail`.
    4. It drains the session's events. `Started` emits `Online { Running }`, sets play on and starts the pacer afresh. `Confirmed` appends `join_player_inputs(inputs)` to the confirmed log.
    5. It calls `Session::outgoing` on every advance, the ended session's included (it lingers), and emits one `NetSend` when there are packets.
    6. Once the session has ended, it finishes the match:
       - it emits `Online { Ended }` and stops play;
       - `adopt_inputs` takes the confirmed log;
       - the claims go back to the automatic ones for the match's players (`reset_claims`);
       - the net session stays, for its lingering `outgoing` and its statistics, until another match or session loads.
  - **Executor errors.** An `ExecuteError::Game` also emits `Paused { Panicked }` when its `SimError` is a panic, `Paused { Failed }` otherwise. Any other `ExecuteError` writes a console error. Either way the executor ended the session, and the next step finishes the match.
  - **Leaving.**
    - `leave_match` while running calls `Session::leave` and emits `Online { Leaving }`.
    - While connecting, it drops the match locally: `Ended` of kind `left`, "peer P left before the match started". The other peers end `unreachable`.
    - Without a match, or after it ended, it is `invalid_command` naming the state.
  - **Panics.** A game panic the host catches while a match is online and not finished:
    - while running or leaving, with `next_tick() > 0`, the session fails at `next_tick() - 1`;
    - otherwise the match is dropped locally as `local_failure`, "the game of peer P failed while the match was starting";
    - the host finishes the match before it recovers as today, so loading another scene is never blocked.
  - **Refused during a match.** While a match is connecting, running or leaving, the host refuses with `online_match`:
    - every command `changes_state` names;
    - `play`, `pause`, `seek`, `set_speed`, `step`;
    - the reads of the session's recorded run: `get_timeline`, `get_tick_events`, `get_tick_commands`, `compare_traces`, `get_ghost_draw`, `get_divergence`, `export_replay`, `export_session`, `set_compare_branch`.

    The message: "`{command}` is refused while an online match runs: it would change this peer's game alone or read the session the match records at its end; leave the match first".
  - **Allowed during a match**: the reads of the world (`get_hash`, `get_hash_tree`, `take_snapshot`, `diff_snapshots`, `query_entities`, `get_entity`, `get_resources`, `get_asset`, `get_render_frame`, `pick`, `get_schedule`, `save_scene`, `save_asset`, `new_asset_value`), subscriptions, `set_pushed_layers`, `set_live_input`, `set_threads`, `net_receive`, `leave_match` and `get_net_stats`. The compare and divergence runs do not follow while online.

- [ ] **Step 1: Write the failing tests**:
  - `two_hosts_play_and_record_the_same_replay`, at 1 and 8 threads:
    - both run, play 300 ticks, and host 1 leaves;
    - both end `left` at the same tick, keep sending `net_send` while lingering, then their sessions resimulate (advance by zero while `get_timeline` says resimulating);
    - `export_replay` gives byte-identical replays, which `play_replay` verifies and which hold exactly the confirmed inputs.
  - `the_adopted_session_resimulates_in_the_background`: a match of 600 ticks, more than `RESIMULATION_BUDGET`.
    - Right after the end, `get_timeline` says resimulating, and the world is at tick 0.
    - The session reaches tick 600 within `600 / RESIMULATION_BUDGET + 2` advances.
    - Its hash there equals the hash `Ticked` gave for the landed tick.
  - `three_hosts_under_latency_and_loss_record_the_same_replay`: behind `Conditioned`, 100 ms latency and 50 per mille loss.
  - `two_local_players_and_one_remote_play`: players `[0, 0, 1]`, then `[0, 1, 1]`.
  - `rollbacks_are_announced_and_ticks_stay_in_order`: under latency at least one `rolled_back`, and the `ticked` ticks rise by one, apart from the landing.
  - `a_mismatched_scene_ends_both_hosts_while_connecting`: kind `mismatch`, naming `scene_and_assets`. Both finish, and loading another scene works.
  - `an_unreachable_peer_ends_connecting`.
  - `leaving_while_connecting_drops_the_match`.
  - `a_lost_peer_leaves_each_host_a_verifiable_replay`: one host stops for good; the other ends `lost` and its replay verifies.
  - `a_failing_game_ends_both_hosts`:
    - a test app whose system fails on an input byte `0xFF`;
    - the failing host ends `local_failure` with `paused` failed;
    - the other ends `local_failure` naming it, through the lingering end notice.
  - `join_match_refuses`: one case per rule.
  - `packets_without_a_match_are_ignored`: before `join_match`, `net_receive` replies `done` and changes nothing.
  - `an_input_too_large_for_the_network_is_refused`: a test app whose input holds a `Vec<u8>`, empty by default, that the mapping fills with 30 bytes.
  - `local_changes_are_refused_during_a_match`: one command of each refused kind, each accepted again after the end.
  - `live_input_after_the_end_uses_the_full_claims`.
  - Protocol: the round trip of `every_command`, and the fingerprint test.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test online --test protocol_v3 --test typescript`. Expected: compile failure.
- [ ] **Step 3: Implement**. Then regenerate the bindings and declarations as the README says, update the fingerprint and the protocol 5 entry, and make the editor ignore the new events.
- [ ] **Step 4: Run** the gates and `cargo test --locked -p ludenea-control`. Expected: PASS, golden tests unchanged.
- [ ] **Step 5: Commit** `✨ feat(control): play an online match through the host`.

---

### Task 8: Network statistics and silent peers on the control surface

**Files:**
- Modify: `crates/ludenea-control/src/{host.rs, online.rs, protocol.rs}` (`PROTOCOL_FINGERPRINT` included), `tests/support/mod.rs`, the generated bindings and declarations, the editor's exhaustive matches (the new events and the new reply), `CHANGELOG.md` (control protocol 5 entry)
- Test: `crates/ludenea-control/tests/online.rs`

**Interfaces:**
- Produces:
  - `Command::GetNetStats`, replying `net_stats`:
    - `NetStats { tick: u64, confirmed: u64, skips: u64, peers: Vec<PeerNetStats> }`, where `confirmed` is `Session::confirmed_tick`;
    - `PeerNetStats { peer: u8, ping_micros: u64, jitter_micros: u64, frame_advantage: i32, rollbacks: u64, deepest_rollback: u64, prediction_misses: u64, stalls: u64, dropped: Vec<DroppedPackets> }`;
    - `DroppedPackets { reason: String, count: u64 }`, the `DropReason` in words, such as `wire: TooLarge`.

    The statistics of an ended match stay readable until another match or session loads. With none: `invalid_command`, "no online match ran".
  - `Event::PeerSilent { peer: u8, for_ms: u64 }` and `Event::PeerBack { peer: u8 }`, from the session's events.
  - A dropped packet writes a console warning the first time each peer and reason occur in a match: "dropped a packet from peer N: {reason}". Later ones are only counted.

- [ ] **Step 1: Write the failing tests**:
  - `net_stats_follow_the_match`: pings near the conditioned latency; `confirmed` at most the local tick plus `input_delay`; rollbacks counted.
  - `a_silent_peer_is_announced_then_back`: a host stops delivering one peer's packets for 1 s.
  - `dropped_packets_warn_once_per_reason`.
  - `net_stats_without_a_match_are_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-control --test online`. Expected: compile failure.
- [ ] **Step 3: Implement**, regenerate, update the fingerprint and the entry.
- [ ] **Step 4: Run** the gates. Expected: PASS.
- [ ] **Step 5: Commit** `✨ feat(control): report network statistics and silent peers`.

---

### Task 9: The signaling listener and `ludenea rooms`

**Files:**
- Create: `crates/ludenea-signal/{Cargo.toml, src/lib.rs, src/rooms.rs, src/limits.rs, src/files.rs}`. It depends on `ludenea-net` for `RoomCode` and `MAX_PEERS`.
- Modify:
  - root `Cargo.toml`: `axum`, `tungstenite`, `matchbox_protocol` and `ludenea-signal` as workspace dependencies; `Cargo.lock`.
  - `crates/ludenea-serve/{Cargo.toml, src/main.rs, src/serve.rs}`: the static file helpers `serve_file`, `plain_name` and `content_type` move to `ludenea_signal::files`, used by both.
  - `crates/ludenea-serve/src/rooms.rs`, new.
  - `README.md`: a section on `ludenea rooms`, its exposure, that it must stay up for every match it serves, that anyone with a room's code can join or disrupt that room, and how to use a stock `matchbox_server` (its root URL as `--signaling`, which then admits anyone to any room).
  - `CHANGELOG.md`: Rust API.
- Test: `crates/ludenea-signal/tests/signal.rs` (the server on `127.0.0.1:0` in a background tokio runtime, `tungstenite` clients on threads), `crates/ludenea-serve/tests/rooms.rs` (the binary)

**Interfaces:**
- Produces:
  - `ludenea_signal::Limits { max_rooms: usize, rooms_per_address: usize, refused_joins_per_minute: usize, idle: Duration, max_message_bytes: usize, queue: usize, header_timeout: Duration }`. `Default` gives 64, 4, 30, 30 s, 16 KiB, 64 and 10 s.
  - `ludenea_signal::Options { page: Option<PathBuf>, limits: Limits, allowed_origins: Vec<String> }`.
  - `ludenea_signal::router(options: Options) -> axum::Router`.
  - `ludenea_signal::serve(listeners: Vec<tokio::net::TcpListener>, options: Options) -> Result<(), String>`: one router on every listener, each connection under `header_timeout` through hyper-util's server builder.
- Rules:
  - **The route.** `GET /rooms/{code}` upgrades to a WebSocket speaking matchbox's JSON (`matchbox_protocol::{JsonPeerEvent, JsonPeerRequest}`). `?host=N` (N from 2 to `MAX_PEERS`) creates the room with N places. The upgrade sets `max_message_size` and `max_frame_size` to `max_message_bytes`.
  - **Origins.** A request with an `Origin` other than its own `Host`, or one of `allowed_origins`, is 403: "origin X may not open rooms here: pass --allow-origin X". A request without `Origin` (a native client) passes.
  - **Refused before the upgrade.**
    - 400: a code `RoomCode::parse` refuses (with its message), or a bad `host` value.
    - 409: "room CODE is in use" for `?host` on a room that exists.
    - 404: "no room CODE" to join a room that does not exist.
    - 410: "the host of room CODE left".
    - 409: "room CODE is full" at its size.
    - 503: "this server holds its maximum of N rooms".
    - 429: "this address hosts N rooms already", and "too many refused joins from this address; wait a minute" after `refused_joins_per_minute` refusals (404, 409 and 410) from one address.
  - **Admission.** The place, and the room for `?host`, is reserved under the lock in the handler, by a guard moved into the `on_upgrade` closure whose `Drop` releases it if the upgrade never completes. On upgrade, the member is inserted and `NewPeer` broadcast in one locked section, so no `Offer` to the newcomer can be lost.
  - **Like `matchbox_server`'s full mesh.**
    - The server sends `IdAssigned` (a v4 UUID) first.
    - It relays `Signal` only to a member of the same room, and drops others.
    - It sends `PeerLeft` to the remaining members when a connection ends.
    - It forgets a room left empty.
  - **Every member's queue is bounded** at `queue` messages. A member whose queue is full is closed with 1013.
  - **Closing.**
    - A message over the limit closes with 1009.
    - A text that is not a JSON `PeerRequest` closes with 1008.
    - A connection without any frame (text, keep-alive, ping or pong, all count) for `idle` closes with 1001.
  - **The page.** Every other path serves the page folder read-only: GET and HEAD, `/` as `index.html`. Any `..` or separator in a segment is 400, other methods 405. Without a page it answers 404, "no page here: pass the folder `ludenea build --web` wrote".
  - **`ludenea rooms [--page DIR] [--listen ADDRESS] [--port PORT] [--allow-origin ORIGIN]... [--idle-seconds N] [--max-rooms N]`.**
    - By default it listens on the machine's LAN address and on `127.0.0.1`, both on `--port` (3536). The LAN address is the local address of a UDP socket connected to `192.0.2.1`; nothing is sent. Without a network it listens on `127.0.0.1` only, and says so.
    - `--listen ADDRESS` listens there only. `--port 0` picks a free port and needs `--listen`: "--port 0 needs --listen: the default listens on two addresses, which share one port".
    - A page folder without `game.json` is refused at launch: "DIR holds no web build: write one with `ludenea build <project> --web --out DIR`".
    - It prints one `signaling ws://ADDRESS:PORT/rooms` line per address it listens on, then `page http://ADDRESS:PORT/` lines with a page. It warns that anyone on the network can reach it.

- [ ] **Step 1: Write the failing tests** (`signal.rs`):
  - `a_host_and_a_joiner_meet`: `IdAssigned`, `NewPeer` to the host, `Signal` relayed both ways with the sender's id.
  - `a_leaving_peer_is_announced`.
  - `joining_a_missing_room_is_404`, `a_second_host_is_409`, `a_full_room_is_409`, `a_room_whose_host_left_is_410`, `an_invalid_code_or_size_is_400`.
  - `rooms_are_bounded` (`max_rooms: 2`), `an_address_hosts_a_bounded_number_of_rooms`, `refused_joins_are_rate_limited`.
  - `concurrent_joins_never_overfill_a_room`: 8 joiners at once to a room of 2, exactly one admitted.
  - `a_foreign_origin_is_403`, and an allowed one passes.
  - `an_oversized_message_closes`, `a_malformed_message_closes`.
  - `an_idle_connection_closes` and `keep_alives_keep_a_connection_open` (`idle` 1 s, a keep-alive every 300 ms for 3 s).
  - `a_member_that_never_reads_is_closed`.
  - `signals_to_another_room_are_dropped`, `an_empty_room_is_forgotten`.
  - `page_files_are_served_read_only`.

  In `rooms.rs`: `rooms_prints_its_urls_and_serves_the_page`, `port_zero_needs_listen`, `a_folder_without_a_web_build_is_refused`.
- [ ] **Step 2: Run** `cargo test -p ludenea-signal` and `cargo test -p ludenea-serve --test rooms`. Expected: compile failure.
- [ ] **Step 3: Implement**: rooms in a `BTreeMap` behind a `Mutex`, one bounded sender per member.
- [ ] **Step 4: Run** the gates. `cargo test -p ludenea-serve` passes unchanged. Expected: PASS.
- [ ] **Step 5: Commit**, in order:
  - `♻️ refactor(serve): share static file serving with a new signaling crate`;
  - `✨ feat(signal): relay WebRTC signaling between the peers of a room`, the body justifying `matchbox_protocol`;
  - `✨ feat(serve): add ludenea rooms`.

---

### Task 10: The WebRTC link

**Files:**
- Create:
  - `vendor/matchbox_socket/`: crates.io 0.14.0 with the patch below.
  - `crates/ludenea-webrtc/{Cargo.toml, src/lib.rs, src/settings.rs, src/native.rs, src/web.rs}`.
- Modify:
  - root `Cargo.toml`: `matchbox_socket = "=0.14.0"`, `futures = { version = "=0.3.34", default-features = false, features = ["executor"] }`, `async-tungstenite = { version = "=0.32.1", default-features = false }`, and a `[patch.crates-io]` entry for `vendor/matchbox_socket`, commented like the others, with "drop it once upstream" and what changed. Also `Cargo.lock`.
  - `xtask/src/main.rs` `CHECKS` and `.github/workflows/determinism.yml`, both: `-p ludenea-webrtc` on the existing `wasm32-unknown-unknown --lib` clippy line.
  - `README.md`: STUN; the strict NAT limitation (no TURN relay); a network that drops UDP to the STUN server stalls browser handshakes about 40 s (`?stun=none` on a LAN); native peers need a network interface other than loopback (webrtc-rs leaves loopback out); native `wss://` trusts a 2022 certificate root store (webpki-roots 0.22).
  - `CHANGELOG.md`: Rust API.
- Test: `crates/ludenea-webrtc/tests/webrtc.rs` (native; dev-dependencies `ludenea-signal`, `tokio`, `tungstenite`)

**Interfaces:**
- Consumes: `Link`, `LinkEvent`, `Channel`, `LinkError`, `RoomCode`, `OnlinePeer`.
- The vendor patch, and nothing else:
  - An empty ICE URL list gives no ICE server, natively (`native.rs`, where it builds `ice_servers`) and in the browser (`wasm.rs`, where it builds `iceServers`). Chromium refuses a server without URLs.
  - Every `unwrap` or `expect` reachable from signaling or peer data drops that peer's handshake with a `warn!` instead of panicking. That covers a malformed SDP or ICE candidate, a data channel looked up for a peer without one, and a peer leaving mid-handshake. Start from `native.rs` near lines 187 and 258, `wasm.rs` near 261 and 413, and `mod.rs` near 277, and list in the patch comment every site changed.
- Produces:
  - `DEFAULT_STUN: &str = "stun:stun.l.google.com:19302"`.
  - `LinkSettings::new(signaling: &str, code: RoomCode, host: Option<u8>, stun: &[String]) -> Result<LinkSettings, LinkSettingsError>`. `host` is `Some(room size)` for the host.
    - It validates the signaling URL (`ws://` or `wss://` with a host) and each STUN URL (`stun:` or `stuns:`, a host, an optional port). A malformed URL panics both matchbox stacks.
    - Errors name the URL and what is wrong.
    - `LinkSettings::room_url(&self) -> String`: the signaling URL without its trailing slash, `/`, the code, and `?host=N` for the host.
  - `WebRtcLink::open(settings: LinkSettings) -> WebRtcLink`, which implements `Link`:
    - A matchbox socket with channel 0 `ChannelConfig::unreliable()` as `Channel::Match`, channel 1 `ChannelConfig::reliable()` as `Channel::Room`, the STUN URLs as its one ICE server, and one connection attempt (a refused room is final).
    - It uses only `try_update_peers` and the channels' `try_send`. It keeps the set of peers reported `Connected`, and refuses a send to any other with `UnknownLink`, without calling matchbox.
    - `poll` reports:
      - `Assigned` once the socket has its id;
      - `Connected` and `Disconnected` from `try_update_peers` (a `Disconnected` for a peer never connected is ignored);
      - `Closed` when the message loop ended, or when no `Assigned` came within 10 s: "no answer from the signaling server at {signaling} within 10 s".
    - When the loop ends other than through the link's own drop, `poll` first reports `Disconnected` for every connected peer, then `Closed`: "lost the signaling server at {signaling}" unless the error says more.
    - `LinkId` is the matchbox `PeerId`'s UUID bytes.
  - Natively the message loop runs on a thread named `ludenea-webrtc`, under `futures::executor::block_on` inside `catch_unwind`. A panic, or a result never delivered, becomes `Closed` with the panic's message. Dropping the link closes the socket, which ends the loop. In the browser the loop runs through `wasm_bindgen_futures::spawn_local`.
  - `Closed` reasons, natively. A refused upgrade arrives as `matchbox_socket::Error::ConnectionFailed(SignalingError::WebSocket(async_tungstenite::tungstenite::Error::Http(response)))`, a closed port as `Io(ConnectionRefused)`:
    - 404: "no room {code} at {signaling}: check the code, or there is no Ludenea listener there";
    - 409 as host: "room {code} is in use at {signaling}: host again for a new code";
    - 409 as joiner: "room {code} is full";
    - 410: "the host of room {code} left";
    - 400: "the signaling server at {signaling} refused the room code {code}";
    - 403: "the signaling server at {signaling} refused this origin";
    - 429: "the signaling server at {signaling} refused: too many rooms or attempts from this address";
    - 503: "the signaling server at {signaling} holds too many rooms";
    - otherwise: "cannot reach the signaling server at {signaling}: {error}".
  - In the browser, which hides the status: "cannot join room {code} at {signaling}: the server refused or cannot be reached (an unknown, full or closed room, a code in use, or no server there)".

- [ ] **Step 1: Write the failing tests** (real WebRTC on this machine, no STUN, each bounded at 30 s with a message saying what it waited for):
  - `two_links_connect_and_carry_both_channels`:
    - a host and a joiner through an in-process listener; both are assigned, then connected;
    - 100 room messages arrive in order;
    - 100 match packets arrive, all of them, since loopback loses none; on a timeout the failure says how many did.
  - `three_online_peers_agree_on_a_layout`.
  - `a_leaving_peer_disconnects`.
  - `a_stopped_listener_disconnects_and_closes`.
  - `a_send_to_a_peer_never_connected_is_refused_and_the_link_lives_on`.
  - `a_malformed_offer_does_not_kill_the_link`: a `tungstenite` client in the room sends a malformed `Offer` to a link, which keeps carrying traffic with another peer.
  - `an_unknown_room_closes_naming_it`, `a_room_in_use_closes_naming_it`, `an_unreachable_server_closes_naming_it` (the port of a closed listener), `a_silent_server_closes_after_10_s` (a TCP listener that never answers).
  - Unit tests of `LinkSettings::new` (each refusal) and `room_url` (trailing slash, host query).
- [ ] **Step 2: Run** `cargo test -p ludenea-webrtc`. Expected: compile failure.
- [ ] **Step 3: Implement** the patch, then the crate.
- [ ] **Step 4: Run** the gates, `cargo xtask check` and `cargo clippy --locked --target wasm32-unknown-unknown -p ludenea-webrtc --lib -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit**:
  - `🩹 fix(webrtc): patch matchbox_socket to take no ICE server and never panic on peer data`;
  - then `✨ feat(webrtc): connect peers over WebRTC data channels`. Its body justifies `matchbox_socket` (one stack for native and browser peers, as the spec requires; natively webrtc-rs, with async-std, discontinued upstream, and rustls 0.21), `futures`, and `async-tungstenite`.

---

### Task 11: The runner online, and a headless peer

**Files:**
- Create: `crates/ludenea-runtime/src/online.rs`
- Modify:
  - `crates/ludenea-runtime/{Cargo.toml, src/lib.rs, src/runner.rs}`:
    - `ludenea-net`;
    - `ludenea-webrtc` under the `native` feature on native targets and the `web` feature in the browser, never on `wasm32-wasip1`;
    - `uuid` under `native` only, since the page passes its own entropy.
  - `crates/ludenea-cli/src/{lib.rs, commands.rs}`: `peer`; the random devices of `--random-inputs` share `RANDOM_KEYS` with `synctest`.
  - `README.md`, `CHANGELOG.md`.
- Test: `crates/ludenea-runtime/tests/online.rs` (runners over a `MemoryMesh`), `examples/arena/tests/online.rs` (two `arena peer` processes through an in-process listener; dev-dependencies `ludenea-signal`, `tokio`), `crates/ludenea-cli/tests/peer.rs`

**Interfaces:**
- Consumes: `OnlinePeer`, `MatchLayout`, `RoomView`, `RoomCode` (Tasks 4 to 6); the host commands and events (Tasks 7 and 8); `WebRtcLink`, `LinkSettings` (Task 10).
- Produces on `Runner`:
  - `join_match(&mut self, settings: OnlineSettings, claims: Vec<Claim>) -> Result<(), RuntimeError>`.
  - `net_receive(&mut self, from: u8, packet: Vec<u8>)`; `take_net_packets(&mut self) -> Vec<(u8, Vec<u8>)>`.
  - `leave_match(&mut self) -> Result<(), RuntimeError>`.
  - `online(&self) -> Option<&OnlineState>`; `silent_peers(&self) -> &BTreeMap<u8, u64>`.
  - `frame` keeps the `net_send` packets and the online events. An ended match is not an error, and once a match ended `frame` sends no more live input.
  - `export_replay` and `snapshot_at(&mut self, tick: u64) -> Result<Vec<u8>, RuntimeError>` (`seek` then `take_snapshot`) first advance by zero while the host answers `resimulating`. They do so at most `ticks / RESIMULATION_BUDGET + 2` times, else return a `Record` error naming the tick reached.
- Produces in `ludenea_runtime`:
  - `RuntimeError::Online(String)`: "the online match cannot go on: {0}".
  - `online::OnlineOptions { role: OnlineRole, signaling: String, stun: Vec<String>, conditions: LinkConditions }`; `OnlineRole::{ Host { peers: u8, code: Option<RoomCode> }, Join { code: RoomCode } }`.
  - `OnlineOptions::plan(&self, entropy: u128) -> Result<OnlinePlan, RuntimeError>`, with `OnlinePlan { role: RoomRole, code: RoomCode, link: LinkSettings, conditions: LinkConditions }`:
    - the room code comes from bits 0 to 63 of the entropy, unless the host gave one;
    - the match identifier from bits 64 to 127;
    - the conditioner's seed from bits 0 to 63;
    - URLs are validated through `LinkSettings::new`.

    Native callers pass `uuid::Uuid::new_v4().as_u128()`, the page its own entropy. One derivation for every runtime.
  - `online::open_link(settings: LinkSettings) -> Box<dyn Link>` (native and browser): a `WebRtcLink`.
  - `online::settings(layout: &MatchLayout) -> OnlineSettings`.
  - `online::OnlineMatch`, which owns an `OnlinePeer<Box<dyn Link>>`, so tests pass a boxed `MemoryLink`:
    - `new(link: Box<dyn Link>, plan: &OnlinePlan) -> Result<OnlineMatch, RuntimeError>`, `code(&self) -> &RoomCode`;
    - `ready(&mut self, claims: Vec<Claim>) -> Result<(), RuntimeError>`, which keeps those claims to join with: the lobby is frozen once ready;
    - `before_frame(&mut self, runner: &mut Runner, now_micros: u64) -> OnlinePhase`: polls the room, joins the runner to the match once laid out, and delivers the received packets;
    - `after_frame(&mut self, runner: &mut Runner, now_micros: u64)`: sends the runner's packets, also after the end;
    - `leave(&mut self, runner: &mut Runner)`;
    - `lingering(&self, now_micros: u64) -> bool`: true until `END_LINGER_MICROS` after the end was seen;
    - `messages(&mut self) -> Vec<String>`: warnings for the room's dropped messages (once per link and reason), and the link's `Closed` reason, which is appended to a `lost` ending's message.
  - `OnlinePhase::{ Room(RoomView), Assigned, Playing, Ended(MatchEnd), Failed(String) }`. `Assigned` is reported once, when the link has its id: the room now exists on the listener, so a host shows its code from then on, not before.
  - `online::write_desync_snapshot(runner: &mut Runner, end: &MatchEnd, local: u8, dump_dir: &Path) -> Result<PathBuf, RuntimeError>`, for a `desync` ending. It writes `snapshot_at(tick)` as `desync-tick-T-peer-P.snap` and returns its path. The runtimes append "wrote PATH: compare it with the other peer's snapshot using `<game> diff`".
- Command: `<game> peer <SCENE> [--project P] [--threads T] [--players K] [--random-inputs SEED] [--ticks N] [--record FILE] [--dump-dir D] [--room-timeout SECONDS] <online options>`.
  - The online options are shared with `play` in Task 12, as a flattened `OnlineArgs`:
    - `--host` with `--peers N` (2 to 4, default 2) and `--room CODE` (instead of a random code), or `--join CODE`;
    - `--signaling URL`;
    - `--stun URL` (repeatable, default `DEFAULT_STUN`) or `--no-stun`;
    - `--latency-ms`, `--jitter-ms`, `--loss-percent` (0 to 100).
  - Exactly one of `--host` and `--join` is required. `--players` defaults to 1, with the automatic claims. `--room-timeout` defaults to 60 and `--dump-dir` to `dump`.
  - It runs headless in real time, stepping every 4 ms. `--random-inputs` plays seeded devices each frame: the keys of `RANDOM_KEYS` held one frame in four, and one gamepad per local player with random buttons and axes. Without it, the players hold nothing.
  - It leaves once the match reaches tick N, and stops once the match ended and stopped lingering.
  - Output, one line each:
    - `room CODE` as host, at `Assigned`, flushed;
    - the room's `describe` whenever it changes;
    - `match started: peer P of N, local players [..]`;
    - `peer Q silent for S s` and `peer Q is back`;
    - `match ended at tick T: {message}`;
    - the per-peer statistics (`get_net_stats`), with the link's failed sends and strays;
    - `wrote FILE` for the replay and the desync snapshot.
  - Exit codes:
    - 0 when the match ended as `left`;
    - 1 for another peer's failure or any other ending;
    - 2 for an error: this peer's game failing, a load failure, a room failure (its message), or a room not ready within the timeout ("room CODE was not ready within S s: {describe}").
  - Ctrl-C ends the process abruptly; the other peers end `lost` after 3 s, as documented.

- [ ] **Step 1: Write the failing tests**:
  - `two_runners_play_online_and_export_the_same_replay` (`runtime/tests/online.rs`, the arena fixture scene, 1/120 s frames).
  - `a_desync_writes_each_peers_snapshot`:
    - two runners on two apps that register the same types (the schema fingerprint covers types, not systems) but whose system moves one entity differently from tick 50;
    - both end `desync` at tick 60;
    - `diff_snapshots` of the two written snapshots names the entity, the component and the field.
  - `a_host_shows_its_code_only_once_assigned`.
  - `two_arena_peers_play_and_verify` (`examples/arena/tests/online.rs`):
    - `CARGO_BIN_EXE_arena peer` as host with `--room`, the joiner started once the host printed `room CODE`;
    - `--no-stun --random-inputs 1 and 2 --ticks 300 --threads 1 and 8 --record`;
    - both exit 0, the replays are byte-identical, and `arena verify` accepts them.
  - `a_peer_that_disappears_ends_the_other_lost`: the joiner is killed; the host exits 1 with `lost` and a replay `arena verify` accepts.
  - `peer_refuses_bad_options` (`cli/tests/peer.rs`): no role, both roles, `--peers 5`, `--join 0O0O0O`, `--stun http://x`, each naming the problem.
  - `peer_reports_an_unknown_room`: exit 2, the 404 message.
  - `peer_gives_up_on_an_empty_room`: `--room-timeout 1`, the message quoting `describe`.
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --test online`, `cargo test -p arena --test online` and `cargo test -p ludenea-cli --test peer`. Expected: compile failure.
- [ ] **Step 3: Implement**. Document `peer` in `README.md`, with a two-terminal example through `ludenea rooms`.
- [ ] **Step 4: Run** the gates and `cargo xtask check`. Expected: PASS. The `wasm32-wasip1` builds of the runtime and the games link no WebRTC.
- [ ] **Step 5: Commit** `✨ feat(runtime): drive an online match from the runner`, then `✨ feat(cli): add a headless online peer`.

---

### Task 12: The native window online

**Files:**
- Modify:
  - `crates/ludenea-runtime/src/native/{window.rs, lobby.rs}`.
  - `crates/ludenea-cli/src/commands.rs`: `play` takes `OnlineArgs` and `--dump-dir`; `--edits` conflicts with `--host` and `--join`.
  - `README.md`: playing online with `ludenea rooms`, then `ludenea play <project> -- --host`, and `-- --join CODE --signaling ws://ADDRESS:3536/rooms`.
  - `.github/workflows/determinism.yml`: the Linux step "Play and verify a match in the native window" gains an online match.
- Test: unit tests in `window.rs` with an `OnlineMatch` over boxed `MemoryLink`s in place of the WebRTC one

**Interfaces:**
- Consumes: `OnlineMatch`, `OnlinePhase`, `OnlineOptions::plan`, `open_link`, `write_desync_snapshot` (Task 11).
- Produces:
  - `PlayOptions::online: Option<OnlineOptions>`, `PlayOptions::dump_dir: PathBuf`, `PlayOutcome::ended: Option<MatchEnd>`.
  - The window opens on the lobby as today. As host, at `Assigned`, it prints `room CODE: give this code to the other players; signaling at URL`.
  - The title, through `lobby::room_title(title, code, view)`:
    - in the room: "{title}: room CODE, {describe}, press Enter when your players have joined";
    - during the match: "{title}: online, peer P of N", plus ", peer Q silent for S s" while one is;
    - after the end: "{title}: match ended: {message}".
  - Enter (or `--players`) readies the room with the claims, instead of starting the match.
  - Each frame runs `before_frame`, the runner's frame, then `after_frame`. The match's first frame is fetched once the runner joined. After the end the window keeps the match's last frame, and `after_frame` keeps running while `lingering`.
  - Online, `--ticks N` does not call `Runner::stop_at`, which would stop the host and its packets. The window leaves at tick N, and closes once the match ended and stopped lingering.
  - Closing the window during a match leaves it, then pumps until the match ended and stopped lingering. That is bounded by `disconnect_after_micros` plus `END_LINGER_MICROS` plus one frame, so the others end with `Left` and this peer's replay is complete.
  - `--record` writes the replay after that. A `desync` ending also writes the snapshot into `--dump-dir`.
  - Room drop warnings and the link's messages go to `out`.
  - The command exits:
    - 0 for a match ended as `left`, or a window closed before any match;
    - 1 for another peer's failure or any other ending (the message printed);
    - 2 for this peer's game failure and other failures, as today.

- [ ] **Step 1: Write the failing tests**:
  - `the_online_lobby_readies_the_room_then_plays`: two window sessions over one mesh, without windows, as the existing tests drive `Session`.
  - `closing_the_window_leaves_the_match`: the other side ends `left` at the same tick, and both replays are byte-identical.
  - `ticks_online_leaves_at_that_tick`.
  - `the_title_follows_the_room_and_the_match`.
  - `an_online_match_writes_its_replay_after_the_end`.
  - In `cli/tests/cli.rs`: `play_refuses_edits_online`.
- [ ] **Step 2: Run** `cargo test -p ludenea-runtime --features native` and `cargo test -p ludenea-cli --test cli`. Expected: compile failure.
- [ ] **Step 3: Implement**. Then extend the CI step:
  - `cargo build --locked --release -p ludenea-serve -p arena`;
  - start `target/release/ludenea rooms --listen 127.0.0.1` in the background, and wait for its `signaling` line;
  - under one `xvfb-run -a bash -c '... & ... & wait'`, run two `arena play examples/arena/assets/arena.scene.ron --players 1 --ticks 600 --no-stun --signaling ws://127.0.0.1:3536/rooms`, one `--host --room 23456A`, the other `--join 23456A` started once the host printed its `room` line, each with its own `--record`;
  - run `arena verify` on both replays and `cmp` them;
  - stop the listener.
- [ ] **Step 4: Run** the gates, `cargo xtask check`, and the manual check below. Expected: PASS.
  - Manual check: run `ludenea rooms`, then two windows on this machine, one hosting and one joining. Play, close the joining window, and see the host's title end with "peer 1 left".
- [ ] **Step 5: Commit** `✨ feat(runtime): host and join online matches in the native window`.

---

### Task 13: The standalone web page online, and cross-play

**Files:**
- Modify: `crates/ludenea-runtime/src/web.rs` (`WebOnline`), `crates/ludenea-control/src/worker.rs` (`netReceive`, `takeNetPackets`), `crates/ludenea-project/web/{index.html, page.js, worker.js, style.css}`, `README.md` (page parameters, the Leave button, the limits of a closed tab), `CHANGELOG.md` (Rust API: `WebOnline`, the `WorkerHost` methods)
- Create: `editor/e2e/online.spec.ts`
- Test: `editor/e2e/online.spec.ts` (Chromium):
  - the arena page that `cargo run --locked -p ludenea-serve -- build examples/arena --web --out editor/e2e/fixtures/arena` writes;
  - `target/debug/ludenea rooms --listen 127.0.0.1 --port 0 --allow-origin <the test server's origin>`, spawned per file, its URL read from its output;
  - `target/debug/arena`.

**Interfaces:**
- Consumes: `OnlineOptions::plan`, `open_link`, `settings`, `OnlinePeer`, `RoomView::describe` (Tasks 5, 6, 11); the host commands and events (Tasks 7 and 8).
- Produces:
  - `WebOnline` (wasm-bindgen, page side), a thin binding over `OnlineOptions::plan`, `open_link` and `OnlinePeer`:
    - `new WebOnline(options: string, entropy: Uint32Array)`. `options` is JSON:

      ```json
      {
        "role": { "host": peers } | { "join": code },
        "signaling": url,
        "stun": [urls],
        "conditions": { "latency_ms", "jitter_ms", "loss_percent" }
      }
      ```

      Bad options throw a message naming the field.
    - getter `code`.
    - `poll(now_ms: f64) -> string`: JSON, one of:
      - `{ "state": "room", "assigned", "describe", "view" }`;
      - `{ "state": "ready", "settings": OnlineSettings, "local": peer }`;
      - `{ "state": "failed", "message" }`.
    - `setReady(players: u32)`.
    - `receive(now_ms: f64) -> Array` of `{ from, packet: ArrayBuffer }`.
    - `send(to: u8, packet: Uint8Array, now_ms: f64)`.
    - `messages() -> Array` of strings, from `room_drops` and the link's `closed`.
  - `WorkerHost` additions:
    - `net_receive(&mut self, from: u8, packet: &[u8]) -> Result<(), JsValue>` (`netReceive`), which sends `Command::NetReceive` to the host without the JSON text;
    - `take_net_packets(&mut self) -> Option<Array>` (`takeNetPackets`): `{ to, packet: ArrayBuffer }`, taken out of `advance`'s events like the render frames.
  - The worker:
    - takes `{ type: "join", settings, claims }`, `{ type: "net", packets }`, `{ type: "leave" }` and `{ type: "snapshot", tick }`;
    - posts `{ type: "net", packets }` (transferred) after every step that has packets, the lingering included;
    - posts `{ type: "online", state }`, `{ type: "silent", peer, ms }`, `{ type: "back", peer }` and `{ type: "snapshot", tick, bytes }`;
    - on `stop` while online, sends `leave_match` instead of `pause`;
    - on `export_replay` and `snapshot` while the host answers `resimulating`, advances by zero and asks again, at most `ticks / 240 + 2` times.
  - The page:
    - **Parameters.** `?host=N` (2 to 4) or `?join=CODE`; `?signaling=URL` (default `ws(s)://<its host>/rooms`); `?stun=URL` (repeatable, `none` for none); `?latency=MS&jitter=MS&loss=PERCENT`. A bad one fails the page naming it.
    - **The room.** The lobby shows the room in `#room`: the code once `assigned` (with `data-room` on the body from then on), and `describe`.
    - **States.** `data-online` takes `room`, `connecting`, `running`, `leaving` and `ended`; `data-end` takes the end's kind.
    - **The status** shows a silent peer ("peer Q silent for S s") until it is back, and the end's message after the end.
    - Readying the room replaces starting the match; `?players=N` readies it at once.
    - **Leaving.** A `#leave` button leaves the match. `pagehide` sends `leave`, best effort.
    - **Pumping.** The page pumps the link whenever a worker message arrives, and every 4 ms (`setInterval`). The worker's timer drives the worker's messages, which a hidden page still receives, so a hidden tab keeps its match going.
    - **After the end**, "Save replay" saves the match. A `desync` ending also offers "Save desync snapshot" (`desync-tick-T-peer-P.snap`).

- [ ] **Step 1: Write the failing tests** (`online.spec.ts`):
  - `two pages play the arena and save the same replay`:
    - page A opens with `?host=2&players=1&stun=none&signaling=...`; page B joins with A's `data-room`, read once set;
    - both reach `running`; A holds ArrowRight for a second, both reach tick 300, and A clicks Leave;
    - both end `left` at the same `data-tick`;
    - the saved replays are byte-identical, and `arena verify` accepts them.
  - `a native peer and a page play under bad conditions` (spec section 8):
    - `arena peer --host --room <random> --players 1 --random-inputs 7 --ticks 600 --no-stun --latency-ms 50 --jitter-ms 10 --loss-percent 5 --record`, against a page `?join=CODE&players=1&stun=none&latency=50&jitter=10&loss=5` opened once the peer printed its `room` line;
    - the peer exits 0, and the page ends `left`;
    - the replays are byte-identical, and `arena verify` accepts both;
    - no browser flag disables mDNS candidates.
  - `an unknown room says so`.
  - `bad parameters fail the page naming them`: `?host=9`, `?join=0O0O0O`, `?loss=150`, `?stun=http://x`.
- [ ] **Step 2: Run** the e2e prerequisites of the README, then `pnpm -C editor e2e online.spec.ts`. Expected: FAIL.
- [ ] **Step 3: Implement**. List `online.spec.ts` in the README's end-to-end paragraph. If the page-to-page test fails on the Linux runner because Chromium cannot resolve its own mDNS names without STUN, stop and report before working around it.
- [ ] **Step 4: Run** the gates, `cargo xtask check`, and `pnpm -C editor e2e standalone.spec.ts online.spec.ts`. Expected: PASS; the local page tests unchanged.
- [ ] **Step 5: Commit** `✨ feat(web): host and join online matches from the standalone page`, then `✅ test(web): play the arena between a native peer and a browser`.
