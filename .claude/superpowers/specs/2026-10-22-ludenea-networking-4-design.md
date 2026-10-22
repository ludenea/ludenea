# Ludenea milestone 4: rollback netcode

Date: 2026-10-22
Status: approved
Parent design: `2026-10-17-ludenea-engine-design.md` (section 4, item 4, the
networking outline)

Marks used below: **(author)** means the author decided it in conversation or
in a parent design; **(builder's call)** means the designer chose it and the
author may revisit it. Unmarked text restates a parent design or the
project's rules in `AGENTS.md`.

## 1. Intent

A Ludenea game today plays on one machine: up to four local players, in the
native window, the standalone web page or the editor. Milestone 4 lets the
same game play online with GGPO-style rollback netcode: each machine runs
its local inputs at once, predicts the remote ones, and corrects itself when
the real inputs arrive, so a fighting game feels local over a real network.

Rollback is the reason Ludenea is deterministic. The milestone turns that
guarantee into multiplayer features a studio would otherwise build after the
fact: a cross-platform match between a native player and a browser player, a
precise report when two peers disagree, and an online match that becomes a
replay any machine can verify (the basis of server-side anti-cheat).

The arena of milestone 3 (two fighters running and jumping through a
tilemap level) is the test bed **(author)**, as milestone 3 planned. The
fighter game remains a separate project.

## 2. Goals, non-goals, anti-goals

### Goals

- A rollback session written in Ludenea **(author)**: input prediction,
  configurable input delay, a configurable rollback window, time
  synchronization between peers, and disconnection handling.
- Up to four players in total, any mix of local and remote, over up to four
  peers connected as a mesh (builder's call on the mesh; GGPO's model).
- A fast in-memory save and restore of the simulation state, distinct from
  the serialized snapshot, sized for a save every tick **(author)**.
- One transport for native and browser peers: WebRTC data channels through
  `matchbox_socket` **(author)**, with signaling hosted by the `ludenea` tool
  and rooms joined by code.
- An in-memory transport and a link conditioner (latency, jitter, loss,
  reordering, duplication), both driven by a seed, so network tests are
  deterministic **(author)**.
- A SyncTest mode that rolls back every tick and compares hashes, to catch
  non-deterministic game code before any online play **(author)**.
- Desync detection between peers through periodic hash exchange, and on a
  mismatch a full divergence report: first divergent tick, entity,
  component, field **(author)**.
- An online match recorded as an ordinary replay of its confirmed inputs,
  verifiable by `verify` on any machine and in CI **(author)**.
- A network panel in the editor (ping, frame advantage, rollbacks,
  prediction misses), fed through the control surface **(author)**.

### Non-goals (milestone 4)

- Lockstep: it comes after rollback **(author)**.
- A TURN relay server and a public matchmaking service **(author)**. Two
  peers behind strict NATs cannot connect; this is a documented limitation.
  STUN is used, so ordinary home NATs work.
- Live spectators **(author)**. The replay covers watching a match after it
  ends.
- Reconnecting a peer to a running match (builder's call): a disconnection
  ends the match for everyone, cleanly (section 7.4).
- Visual smoothing of rollback corrections (blending a corrected position
  over several frames). The renderer shows the corrected state at once.
- Fighter gameplay and its match flow.

### Anti-goals

A change that does any of these is a failure, even if it works:

- A rollback that changes a golden hash, or a game that hashes differently
  online than offline for the same confirmed inputs.
- A networked game that plays differently on one platform, or that only
  holds with one thread.
- Netcode that reaches into engine internals a game cannot reach. The
  session is written against the public API; the host and the runtime drive
  it through host commands like everything else.
- A desync report that only says "hash differs".
- A peer that panics, freezes or corrupts its state on a malformed,
  oversized, duplicated or hostile packet.
- A second replay format, or a second way to run a match (the local match
  and the online match share the host's pacer, claims and session).

## 3. Constraints

- Determinism rules from `AGENTS.md` and `clippy.toml` apply to the session
  logic that decides which ticks run with which inputs. The session reads no
  clock: the caller passes elapsed time, as with the pacer today. Network
  statistics (ping, jitter) may use floating point because they never feed
  the simulation; only the integer decisions derived from them (how many
  ticks to run, whether to wait a frame) affect pacing, and pacing never
  affects hashes.
- Everything that arrives from the network is untrusted: size-bounded,
  decoded defensively, validated against the session (player indexes, tick
  ranges, input sizes), and dropped with a counted, logged reason when
  invalid. A peer never panics on bad input.
- Peers refuse to start a match unless their engine version, schema
  fingerprint, network protocol version, scene and assets (by hash), tick
  rate and player layout match, with an error that names what differs.
- New dependencies (`matchbox_socket`, the signaling side of matchbox or an
  equivalent in the `ludenea` tool) are pinned exactly and justified
  in their commit, like wgpu.
- The network wire format is versioned, listed in the CHANGELOG formats
  table, and has its own version constant. Control protocol changes follow
  the protocol policy (first change bumps the version, later ones update the
  pinned fingerprint and the same CHANGELOG entry).
- In the browser the simulation runs in a Web Worker, and Chromium does not
  expose WebRTC to workers. The page owns the socket and forwards packets to
  the worker (builder's call on the exact bridge).

## 4. Architecture

### 4.1 The rollback session: sans IO (builder's call)

A new crate `ludenea-net` holds the rollback session as a state machine with
no IO and no clock. It is fed events (local inputs for a tick, packets
received, time elapsed) and returns requests (save tick N, load tick N,
advance with these inputs, send these packets, report this event). This is
GGRS's proven shape, written in Ludenea so desync detection, replays and
diagnostics are native **(author)**.

Responsibilities:

- the input queue per player: confirmed inputs, predicted inputs (repeat the
  last confirmed input), the first incorrect prediction;
- the input delay: local inputs apply N ticks after they are read;
- the rollback window: the session never predicts more than W ticks ahead
  of the last tick confirmed by every peer, and waits (stalls) instead;
- time synchronization: each peer computes its frame advantage over the
  others and runs slightly slower when ahead, so neither peer stalls
  constantly;
- the confirmed tick: the latest tick whose inputs are known for every
  player, which drives the replay, the hash exchange and memory release;
- the wire protocol: input packets carry redundant unacknowledged inputs
  (so a lost packet costs nothing while a later one arrives), acknowledgments,
  quality reports for ping and frame advantage, hash reports, and the
  handshake.

Defaults (builder's call, tuned on the arena): input delay 2 ticks, rollback
window 8 ticks, hash report every 30 confirmed ticks, disconnection notice
after 750 ms of silence and disconnection after 3 s. All are session
settings, validated when the match starts.

### 4.2 Fast state: save and restore in memory

`Snapshot` serializes every component and resource to postcard. It stays
the format of files, checkpoints, the hash and the divergence report, but it
is too slow for a save every tick and a restore plus up to W+1 ticks of
resimulation inside one displayed frame.

The milestone adds a second way to capture state that never leaves memory:
each registered component and resource storage gains a typed clone, and
loaded assets are shared.
A saved state holds exactly what `Snapshot` holds: the entity allocator,
every component, resource and asset (the RNG is a resource). Like
`World::restore`, restoring drops any queued command and event, is all or
nothing, and keeps runtime resources.

**The `Clone` bound (author).** `Component` and `Resource` require
`Clone` from this milestone on, for every game, online or not: one save
path, no opt-in. This is a deliberate breaking change of the public API,
documented in the CHANGELOG with its migration (derive `Clone` on each
type), and the derive macros and the project template are updated so a new
game gets it by default. A type that cannot be cloned is refused at compile
time with an error that names the bound. `Asset` does not require `Clone`
(author): loaded assets are stored behind `Arc` and only ever replaced, never
written in place, so a saved state shares them instead of cloning them, and
copies the asset map only when it changed since the last save.

This is not a second way to do the same thing: the saved state is the
in-memory counterpart of the snapshot, and a property test proves that
`save, run, restore` gives a world whose snapshot is byte-identical to the
snapshot taken at the save, across random worlds and every registered type.

The session keeps saved states in a ring of W+2 entries; their memory is
reused, not reallocated, every tick (builder's call on pooling).

A benchmark on the arena measures the worst frame (restore, then W+1 ticks
with a save each) and CI records it; the target is that the worst frame fits
in half of a 60 Hz frame on the reference machine, single-threaded.

### 4.3 Transports

The session talks to a transport trait (send to peer, receive) with three
implementations:

| Transport | Where | Purpose |
| --- | --- | --- |
| `matchbox_socket` WebRTC data channels, unreliable and unordered | native and browser | real matches, cross-play |
| In-memory | tests, the command line | deterministic multi-peer tests in one process |
| Link conditioner | wraps any transport | latency, jitter, loss, reordering, duplication from a seed |

The link conditioner wraps the real transport too, so a developer can try a
match under bad conditions on a LAN (a session setting, off by default).

Signaling (builder's call): `ludenea serve` is bound to localhost and its
project service reads and writes files, so it is never exposed to the
network. Instead, a separate listener, `ludenea host` (name builder's call),
serves only two things, read-only: the matchbox-compatible signaling
endpoint and the game's built standalone web page (the output of `ludenea
build`). It binds to an address the developer gives (the LAN by default),
carries no project service and no file write, and bounds rooms, peers per
room, message sizes and idle time. A room is a short code with a fixed
number of peers, forgotten once its peers are connected. The Chrome player
of the definition of done opens the page from this listener. A game's
native build and web page point at a signaling URL given in their launch
settings, so a studio can host signaling elsewhere. STUN servers are a setting too, with a public
default documented as such.

### 4.4 Fitting into the host

The host already drives every local match: the runner (native window) and
the worker (web page) send it live devices, the pacer turns elapsed time
into ticks, and the session records inputs, edits and branches. An online
match runs through the same host **(author: one way to run a match)**:

- the host owns the rollback session when a match is online; the pacer's
  tick count goes through the session, which may stall a tick, run extra
  ticks to catch up, or roll back first;
- local devices are mapped to the local players' inputs exactly as today
  (claims, `map_players`); remote players' inputs arrive encoded and are
  checked against the input schema before use;
- packets enter and leave the host through host commands and events, so the
  native runner, the web worker bridge, the editor and the command line all
  drive a match the same way (builder's call on command shapes);
- while a match is online, the host records only confirmed inputs into its
  session's input log, so its replay never holds a prediction;
- edits, branches, undo, seeking the timeline, pause and speed changes are
  refused while a match is online, with a precise error (builder's call):
  each would be a change one peer makes alone. After the match ends, its
  recording is an ordinary session the editor can open, fork and edit;
- the control surface shows the current state, predicted or not (builder's
  call): per-tick events keep their shape, and a new rollback event says
  which ticks were replaced, so the timeline and the inspector redraw them.
  The confirmed tick is part of the network statistics, so a tool that
  needs only confirmed state can wait for it.

### 4.5 Players across peers

Before an online match, each peer declares how many local players it brings
(through the existing lobby and claims), and the host of the room orders
the peers, which fixes the global player index of every player. The layout
is part of the handshake, and the game sees `Inputs` with one entry per
player as today, so a game written for local play runs online unchanged
**(author: the game code does not change)**.

## 5. Desync detection and the divergence report

- Every 30 confirmed ticks (a setting), each peer sends the state hash of
  that tick. Peers keep the saved state of the last reported ticks until all
  peers have confirmed them.
- On a mismatch (builder's call on the procedure): the confirmed inputs are
  identical on both peers by construction, so each peer replays its own
  trajectory from the last matching report with introspection recording,
  and the peers exchange the per-tick entity hash trees of that interval
  (`Simulation::hash_tree`) over a reliable, chunked channel. The first tick
  whose trees differ is the first divergent tick; the peers then exchange
  the snapshots of that tick and the one before, and the field diff names
  the entity, component and field. The report has the shape of the
  editor's branch divergence report, with the possible causes (events and
  commands) from each peer's own introspection shown side by side.
- The match ends with a precise error, and the report goes out as a host
  event: the editor shows it in the divergence view, the native runner and
  the command line write it and both snapshots to a folder (like the golden
  dumps), and the standalone web page offers them as a download. A desync is a bug in
  the engine or the game, so the milestone never tries to resynchronize by
  copying state (builder's call: copying would hide the bug and is a cheat
  vector).
- SyncTest mode runs a match alone: every tick, it rolls back W ticks and
  resimulates, and compares each resimulated hash with the first run. On a
  mismatch it produces the same report. The `ludenea` tool holds no game
  code, so SyncTest is a subcommand of the game's own command line, like
  `verify` (`synctest`, builder's call on the name), and is also reachable
  through the host and the editor. CI runs it on the arena and the swarm
  (the example projects) and on the boxes scenario through its tests.

## 6. Replays and verification

An online match produces an ordinary replay (same format, same version) from
its confirmed inputs: no new format **(author)**. Every peer records it.
For a match that ends normally, the peers agree on the last tick (a leaving
peer's goodbye and the end of a match carry the tick, and every peer stops
there), so their replays are byte-identical, which a test asserts. `verify` on any
machine checks the replay's hashes, which is the server-side re-verification
an anti-cheat needs: a server that receives a match's inputs replays them
and rejects a result the inputs do not produce.

A match that ends abruptly (a lost peer, a desync) still produces, on each
peer, the replay of the ticks that peer confirmed. Those replays are valid
and verifiable but may stop at different ticks, since no agreement is
possible with a peer that is gone (builder's call). Why the match ended is
not stored in the replay (its format and version stay unchanged): it is
reported as a host event and logged.

## 7. Runtime, command line and editor

### 7.1 Native window and standalone web page

`ludenea play` gains host and join modes (builder's call on the flags, such
as `--host` which prints a room code and `--join <code>`), and the native
window and the standalone web page show the room code, the waiting peers,
and the connection state. The lobby of milestone 3 stays the way local
players join; a room adds remote peers to it.

### 7.2 Command line

- `ludenea synctest` (section 5).
- A headless peer, so CI and a developer can run a match between a native
  process and a browser, or between two processes, with scripted inputs
  (builder's call on the shape: a mode of `play` or of the game's CLI).

### 7.3 Editor

- The editor can host and join a match in play mode, through the control
  surface.
- A network panel, a built-in plugin on the public editor API: ping and
  jitter per peer, frame advantage, rollbacks per second and their depth,
  prediction misses, stalls, packets dropped and why, and the hash reports.
- A desync report opens in the existing divergence view.
- A finished online match opens as a session, like a local one.

### 7.4 Disconnections

After 750 ms without packets, every peer shows a notice naming the peer.
After 3 s, the match ends for everyone with a reason, and each peer keeps
the replay of the confirmed ticks. A peer that leaves on purpose sends a
goodbye so the others end at once.

## 8. Quality

- The rollback session is tested without IO: scripted packet orders, loss,
  duplication, reordering, late inputs, stalls at the window, time
  synchronization, and disconnection, with property tests over random
  network conditions that assert the confirmed hashes equal an offline run
  of the same confirmed inputs.
- Every network packet decoder is fuzzed (seeded, in the test suite like the
  existing fuzzing) and never panics.
- Multi-peer tests run two, three and four peers in one process over the
  in-memory transport behind the link conditioner, with 1 and 8 threads,
  and on WebAssembly under `wasmtime` for the session logic.
- An end-to-end test runs a native peer against a browser peer in headless
  Chromium over real WebRTC on the local machine, with the link conditioner
  on both sides.
- A golden online replay, `golden/arena-online.replay` (builder's call on
  how it is produced): a replay holds only confirmed inputs, so network
  conditions leave no trace in it. It is recorded by a seeded in-process
  match of the arena (scripted inputs, two peers, link conditioner with 100
  ms of latency and 5% loss), with a re-recording command in `README.md`
  like the other goldens. What it proves: the online path records a replay
  identical to the offline one for the same inputs, and the `determinism`
  workflow verifies it on every runtime. The real cross-play path is proven
  by the end-to-end test above, which asserts both peers' replays are equal
  and pass `verify`.

## 9. Definition of done

Two players play the arena online, one in the native window on macOS and
one in Chrome, each on its own machine, through a room code from the signaling
listener of section 4.3, with 100 ms of added latency and 5% loss, without a desync; the
match feels local (inputs apply after the input delay, corrections are
rare). A machine can bring two local players into a match with a remote
one. The recorded match is a replay that `verify` accepts on macOS, Linux,
Windows and WebAssembly. SyncTest passes on every example game. A desync
forced by a test (a deliberately non-deterministic system) produces a report
that names the tick, entity, component and field. The editor hosts and joins
a match and shows the network panel.

## 10. Left to the builder

- Exact session settings, wire format, host command and event shapes.
- The mesh topology details and the room ordering rule.
- The signaling implementation, as long as it lives in the `ludenea` tool, apart from `serve`, and
  talks to `matchbox_socket`.
- The worker bridge in the browser.
- The order of work, as long as each step keeps the goldens, the local
  match and the editor working. A suggested split, as for milestone 3:
  4a the fast state, the session and SyncTest over the in-memory transport;
  4b WebRTC, signaling, the native and web runtimes and the command line;
  4c the editor, desync reports end to end and the online golden.
