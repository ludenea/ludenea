# Ludenea

Ludenea is a deterministic 2D game engine written in Rust. It combines an
entity component system, a Box2D-style physics engine, a versioned control
protocol, and a command line tool, all built so that a simulation can be
recorded, replayed, and verified bit for bit.

## Determinism guarantee

The same scene, the same input log, and the same engine version produce the
same state hash at every tick, on macOS, Linux, Windows, and WebAssembly, and
for any thread count. The guarantee rests on:

- a golden trace of 10,001 state hashes, which the `determinism` workflow
  verifies on macOS, Linux, and Windows at 1, 2, and 8 threads, and on
  WebAssembly under `wasmtime`;
- a golden replay (`golden/spawner.replay`) of a scene with prefabs, a data
  asset, and edits to both, verified the same way;
- a golden replay (`golden/arena.replay`) of two fighters running and jumping
  through the arena's level, which the arena's `build-level` tool built
  through the control protocol, verified the same way;
- a pinned hash of a value that holds every serde shape, and mutation testing
  that checks the tests catch every mutant that `cargo-mutants` generates, and
  that compiles, in the serializer the state hash is computed with
  (`crates/ludenea-ecs/src/hash.rs`);
- lints that forbid platform-dependent floating point functions, wall-clock
  time, and unordered collections in simulation code;
- a pinned toolchain and a committed `Cargo.lock`.

## Workspace layout

| Path                     | Purpose                                                      |
| ------------------------ | ------------------------------------------------------------ |
| `crates/ludenea-math`    | Deterministic vectors, rotations, and functions              |
| `crates/ludenea-ecs`     | World, queries, snapshots, and state hashing                 |
| `crates/ludenea-macros`  | Derive macros                                                |
| `crates/ludenea-app`     | Simulation loop, scenes, and input logs                      |
| `crates/ludenea-physics` | Rigid body physics                                           |
| `crates/ludenea-draw`    | Render frame and debug draw data, GPU free                   |
| `crates/ludenea-render`  | Render components, extraction, wgpu renderer                 |
| `crates/ludenea-net`     | Rollback session, wire format, and transports                |
| `crates/ludenea-control` | Versioned control protocol and host                          |
| `crates/ludenea-cli`     | `run`, `verify`, `diff`, `render`, `play`, and `synctest`    |
| `crates/ludenea-runtime` | Game runner, native window, web page exports                 |
| `crates/ludenea`         | Facade crate re-exporting the engine                         |
| `crates/ludenea-project` | Projects, builds, and plugin bundling                        |
| `crates/ludenea-serve`   | The `ludenea` tool: `new`, `serve`, `play`, `build`, `rooms` |
| `crates/ludenea-signal`  | Signaling listener of online matches, static files           |
| `crates/ludenea-webrtc`  | WebRTC link between online peers, native and browser         |
| `editor`                 | The editor: web and desktop applications                     |
| `examples/boxes`         | Reference scenario, golden trace, and benchmark              |
| `examples/swarm`         | Runtime test game: 10,000 sprites, 4 players                 |
| `examples/arena`         | Platform arena: two fighters, run and jump                   |
| `golden`                 | Golden hash traces                                           |

## Commands

Run the tests:

```sh
cargo test --locked --workspace
```

On a divergence, the golden tests write the snapshots of the first divergent tick and the
one before it into a folder per golden and thread count, such as `boxes-golden-2-threads`,
under the folder `LUDENEA_DUMP_DIR` names, else under the temporary directory. Give it an
absolute path: cargo runs the tests from `examples/boxes`, so a relative one resolves there.
CI sets it to `dump/` in the checkout and uploads the dumps of a failing run as artifacts,
one per operating system, target, and thread count.

On Linux, the native window's gamepad library (gilrs) builds against libudev: install its
development files first (`sudo apt-get install libudev-dev` on Debian and Ubuntu). The
native window's own tests run with its feature on:

```sh
cargo test --locked -p ludenea-runtime --features native
```

The WebRTC link's own tests need its feature too (the workspace tests turn it on through the
native window's):

```sh
cargo test --locked -p ludenea-webrtc --features link
```

Run the rollback networking tests under WebAssembly, as CI does (with `wasmtime` and the
`wasm32-wasip1` target installed):

```sh
CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime run --dir=." cargo test --locked --target wasm32-wasip1 -p ludenea-net
```

The arena's rollback test runs two, three, and four peers over a seeded lossy network, at 1 and
8 threads, and checks that every peer matches an offline run. It draws 8 seeds of network
conditions by default; CI runs 32 in release on Linux:

```sh
LUDENEA_NET_SEEDS=32 cargo test --locked --release -p arena --test rollback
```

Run the lint checks CI runs: formatting (the engine and the desktop backend), clippy on the
host (with and without the wgpu renderer and the native window) and on WebAssembly, and the
API documentation, with warnings denied:

```sh
cargo xtask check
```

Run the renderer's GPU tests, which draw offscreen and compare the pixels read back. They need
a GPU adapter, hardware or software: without one they fail naming what was tried, they never
skip. The command line tool's `render` tests (`crates/ludenea-cli/tests/render.rs`), part of
the workspace tests above, need one too. CI provides lavapipe on Linux (the
`mesa-vulkan-drivers` package), WARP on Windows (built in), and Metal on macOS.

```sh
cargo test --locked -p ludenea-render --features gpu
```

The renderer uploads a frame's geometry once per tick and keeps it on the GPU: a draw between
two ticks writes only the views, and the shader interpolates. Its frame budget test draws
10,000 sprites 144 times per pair of ticks and reports the CPU time of one draw;
`PERF_STRICT=1` asserts a 95th percentile below 1 ms, which holds on the reference machine (an
Apple M-series; about 0.2 ms on an Apple M5):

```sh
PERF_STRICT=1 cargo test --release --locked -p ludenea-render --features gpu --test perf -- --ignored --nocapture
```

Check that the tests catch every mutant of the canonical state hash serializer, as the
`mutants` workflow does when the serializer or the `ludenea-ecs` tests change (install the
tool with `cargo install --locked cargo-mutants@27.1.0`):

```sh
cargo mutants --file crates/ludenea-ecs/src/hash.rs --cargo-arg=--locked
```

Verify the golden trace natively (use `--threads 1`, `2`, or `8`):

```sh
cargo run --locked --release -p boxes -- verify examples/boxes/assets/boxes.scene.ron \
  --inputs examples/boxes/assets/boxes.inputs --golden golden/boxes.trace --threads 8
```

Verify the golden trace under WebAssembly:

```sh
cargo build --locked --release --target wasm32-wasip1 -p boxes --bin boxes
wasmtime run --dir=. target/wasm32-wasip1/release/boxes.wasm verify \
  examples/boxes/assets/boxes.scene.ron --inputs examples/boxes/assets/boxes.inputs \
  --golden golden/boxes.trace
```

Verify the golden replay natively (use `--threads 1`, `2`, or `8`) and under WebAssembly:

```sh
cargo run --locked --release -p boxes -- verify golden/spawner.replay --threads 8
wasmtime run --dir=. target/wasm32-wasip1/release/boxes.wasm verify golden/spawner.replay
```

Record it again after a deliberate change to the simulation (its hashes change with it):

```sh
cargo run --locked --release -p boxes --bin record-replay -- golden/spawner.replay
```

The arena's golden replay (600 ticks of scripted input for two players) is verified and
recorded the same way:

```sh
cargo run --locked --release -p arena -- verify golden/arena.replay --threads 8
cargo build --locked --release --target wasm32-wasip1 -p arena --bin arena
wasmtime run --dir=. target/wasm32-wasip1/release/arena.wasm verify golden/arena.replay
cargo run --locked --release -p arena --bin record-replay -- golden/arena.replay
```

Catch game code that a rollback makes diverge: `synctest` runs a scene or replay straight next to
a copy that mispredicts and rolls back every tick (`--window` ticks back, 8 by default), and
compares their hashes at every tick. A mismatch names the tick, the entities, components and
fields that differ with their candidate causes, and the resources that differ; it writes both
snapshots of that tick, `tick-K-straight.snap` and `tick-K-rollback.snap`, into `--dump-dir`
(`dump` by default) and exits with code 1. A replay holds its own run; a scene needs `--ticks`,
and takes `--inputs` or `--random-inputs SEED`, seeded keys and gamepad input mapped by the
game's own input mapping. CI runs these at 1 and 8 threads, and the arena's under `wasmtime`:

```sh
cargo run --locked --release -p arena -- synctest golden/arena.replay --threads 8
cargo run --locked --release -p swarm -- synctest examples/swarm/assets/swarm.scene.ron \
  --ticks 600 --random-inputs 1 --threads 8
cargo run --locked --release -p boxes -- synctest golden/spawner.replay --threads 8
wasmtime run --dir=. target/wasm32-wasip1/release/arena.wasm synctest golden/arena.replay
```

Render a tick of a scene or replay to a PNG (1280 by 720 unless `--width` and `--height`
say otherwise; repeat `--debug-layer` to draw debug layers over it). A scene runs with its
`--inputs` and `--edits`; the textures are the project's PNGs with a texture meta, and one
whose meta cannot be read or is invalid, or that cannot be packed, is a warning: it is left
out and drawn as a magenta placeholder. Two textures sharing an identifier are refused. Like
the GPU tests, it needs a GPU adapter, hardware or software, and fails naming what was tried
without one; the textures and the adapter are checked before the run:

```sh
cargo run --locked --release -p boxes -- render golden/spawner.replay --tick 300 \
  --out spawner.png --debug-layer physics.colliders
```

Play a game in a window. It opens on the lobby: each local player (up to four) claims a slot
with a key of a keyboard half or a gamepad button, Escape or Select gives it back, and Enter or
Start begins the match; the window title names each claim. `--players N` skips the lobby: one
player holds the keyboard and the first gamepad, two share the keyboard's halves. F11 toggles
borderless fullscreen (`--fullscreen` starts there), `--record` writes the replay when the
window closes, after a failure of the game too, and `--ticks` closes it once the match reaches
that tick. Saving a texture's PNG or meta while the game plays repacks it at the next frame,
without touching the simulation; a version that cannot be read or decoded is a warning and the
previous one stays drawn. `--no-watch` keeps the textures of the launch. A failure exits with
code 2:

```sh
cargo run --locked --release -p swarm -- play examples/swarm/assets/swarm.scene.ron \
  --record swarm.replay
cargo run --locked --release -p swarm -- verify swarm.replay
```

Measure the frame times: `--no-vsync` draws as fast as the GPU allows, and `--stats` prints the
50th, 95th and 99th percentiles of the frames displayed when the window closes. A frame's time is
the work of the frame (input, ticks, drawing), without its wait for the screen's next texture:
a frame made faster than the display refreshes waits there, which says nothing of the engine.
The GPU's own time counts only once it delays that texture. The target is a
95th percentile at or below 6.94 ms (144 frames per second) on the reference machine; the
window must be on screen, since a hidden window (another desktop, a locked screen) displays no
frame:

```sh
cargo run --locked --release -p swarm -- play examples/swarm/assets/swarm.scene.ron \
  --players 1 --no-vsync --ticks 1200 --stats
```

On an Apple M5 laptop the 95th percentile is about 5 ms (4.76 and 5.24 ms over two runs).

Play online in the window: `play` takes the online options of `peer` below (`--host` with
`--peers` and `--room`, or `--join CODE`, `--signaling`, `--stun` or `--no-stun`, and the link
conditions). The window opens on the room: a host prints `room CODE: give this code to the
other players; signaling at URL` once the room exists, and Enter (or `--players`) readies the
room with the lobby's claims; the match starts once every peer is ready. The title names the
room's state, then `online, peer P of N` and any peer silent, then `match ended:` and why.
Closing the window during the match leaves it: the window goes, and the match plays on until
it ended and the others were told (at most 3.5 seconds: the silence that disconnects a peer,
then the linger; past 4.5 seconds the session fails), so they end it as left at the same tick
and `--record` writes a complete replay. `--ticks N` leaves at tick N, and closes once the
match ended. A desync writes this peer's snapshot into `--dump-dir`, as `peer` does, and
`--edits` is refused online. It exits with code 0 for a match ended because a peer left (or a
window closed before any match), 1 for any other ending (the line `match ended:` says why),
and 2 for a failure of this peer: its game, its files, or its room.

Play a match online without a window: `peer` hosts (`--host`) or joins (`--join CODE`) a room
of a signaling listener (`ludenea rooms`, see the editor section below), readies it with
`--players` local players (1 by default, with the devices `play --players` gives them), and
plays in real time, a frame every 4 ms. The players hold nothing, or seeded random devices
with `--random-inputs SEED`: the keys `synctest` draws from, held one frame in four, and one
gamepad per local player with random buttons and axes. The host's room holds `--peers` peers
(2 to 4, 2 by default) under a random code, or `--room CODE`; it prints `room CODE` once the
room exists on the listener. `--signaling` gives the listener's URL
(`ws://127.0.0.1:3536/rooms` by default), `--stun` a STUN server (repeatable,
`stun:stun.l.google.com:19302` by default) and `--no-stun` none, as on a local network.
`--latency-ms`, `--jitter-ms` and `--loss-percent` impair this peer's own sends. Each
change of the room's state prints a line, then `match started: peer P of N, local players
[..]`, a line when a peer falls silent or is back, `match ended at tick T:` and why, and the
statistics of each other peer and of the link. `--ticks N` leaves the match at tick N; the
peer stops once the match ended and it told the others why. `--record` writes the replay,
and a desync writes this peer's snapshot of the divergent tick into `--dump-dir` (`dump` by
default) as `desync-tick-T-peer-P.snap`: compare it with the other peer's using `diff`. It
exits with code 0 for a match ended because a peer left, 1 for any other ending, and 2 for a
failure of this peer: its game, its files, its room (such as an unknown code), or a room not
ready within `--room-timeout` seconds (60 by default). Ctrl-C ends the process at once: the
other peers end the match as lost after 3 seconds without its packets. One terminal per
command, on one machine (which needs a network interface besides loopback, as below):

```sh
cargo run --locked -p ludenea-serve -- rooms --listen 127.0.0.1
cargo run --locked --release -p arena -- peer examples/arena/assets/arena.scene.ron \
  --host --room 23456A --no-stun --random-inputs 1 --ticks 600 --record host.replay
cargo run --locked --release -p arena -- peer examples/arena/assets/arena.scene.ron \
  --join 23456A --no-stun --random-inputs 2 --record join.replay
cargo run --locked --release -p arena -- verify host.replay
```

Both replays are byte-identical: a replay holds the confirmed inputs only.

Run the benchmark:

```sh
cargo run --locked --release -p boxes --bin bench
```

Run the rollback benchmark, which times the worst case of a rollback frame (load the state
eight ticks back, replay nine ticks saving after each) over the arena's golden replay, and
reports whether the 99th percentile of the frames fits in half a 120 Hz frame (4,167
microseconds); the slowest frame is printed too, but not judged, as it mostly measures
operating system scheduling noise:

```sh
cargo run --locked --release -p arena --bin rollback-bench
```

Run the same benchmark on swarm (10,000 sprites, 4 players), a heavy scene that has no golden
replay: a straight run with seeded inputs gives the hash every rollback frame must end on:

```sh
cargo run --locked --release -p swarm --bin rollback-bench
```

## Versioning

Every file format and protocol has a version, except those [`CHANGELOG.md`](CHANGELOG.md)
lists as not versioned yet. The changelog lists each one's current version and the oldest
version this build reads, and records every change to them, to the public Rust API, and to
the plugin API, in the commit that makes it. The rules:

- A reader reads the version before anything else, so a version it does not support is
  refused as such, not as a malformed file.
- Any change to an encoded type bumps its version; nothing is appended silently. The
  first change after a release bumps it, and later changes before the next release go into
  that same version, so a release moves each format by one version at most.
- User files (scenes, prefabs, metas, data assets, the project manifest `ludenea.toml`,
  replays, traces, snapshots, input logs, and edit logs) stay readable one release back
  (N-1). A pure conversion from each older version to the next one migrates them, like the
  edit log's `version_1` module. Data assets and `ludenea.toml` have no version yet, so
  this holds for them only once they have one.
- Editor state (the stored session, its undo history, and its branch tree) may drop what
  is older than N-1. A stored file the editor refuses is set aside, never overwritten.
- The control and project protocols require the same version on both sides, and the same
  fingerprint: a hash of the TypeScript declarations of every type their messages reach,
  so builds from different revisions of one version whose declarations differ refuse each
  other. A change the declarations do not show, such as a new serde default on a field or
  another encoding of a type declared as a string (`Hash64`, `Bytes`, `AssetId`), leaves
  the fingerprint as it is, so builds that differ only by it still connect. Their changes
  accumulate under the next version, which is bumped once per release; its changelog entry
  names its fingerprint and lists the commands, messages, and fields that changed.

## Editor

The editor opens a game project, runs the game, and records everything that happens
while it plays: scrub the timeline back to any tick, change a component in the inspector
at that tick, play again from there, and undo or redo the change. With several entities
selected, Delete, the arrow keys, a drag in the viewport, and the inspector change them all
as one edit, which one undo takes back; the inspector shows the components they share and
marks the fields whose values differ. The session (scene, input log, and edit log) replays
in the command line tool to the same hashes, tick for tick. The game always runs apart from
the interface (a Web Worker in the browser, a separate process on the desktop), so the
interface stays responsive while it simulates.
Panels: scene tree, inspector, viewport (the game as its renderer draws it, with the physics
debug drawing on top), timeline, asset browser, entity explorer, physics, tile palette, and
console, all reachable from the command palette.
A project opens with the document last open in it, or else with the `default_scene` of its
`ludenea.toml`; without one (or with its file missing) nothing opens and the console says
why. Double-click a scene, prefab, or replay in the asset browser to open another.

Create a project from the template (the game depends on the engine at `--engine-path`):

```sh
cargo run --locked -p ludenea-serve -- new ~/games/my_game --engine-path .
```

Play it in a window: `ludenea play` builds the game natively and runs its `play` command; the
scene defaults to the `default_scene` of `ludenea.toml` (relative to the project root; a
`--scene` path is relative to the working directory), and everything after `--` goes to the
game (`--players`, `--record`, `--fullscreen`, ...). It exits with the game's code, or 2 when
the build or the launch fails:

```sh
cargo run --locked -p ludenea-serve -- play ~/games/my_game -- --players 2
```

Build it as a standalone web page, a static folder any web server serves without the editor:
`ludenea build --web` builds the game for the browser (with its `web` feature, which the
template declares; a project made before needs `web = ["ludenea/web"]` under `[features]` in
its `Cargo.toml`) and writes the page, the game, its scene, and its textures packed in pages
of at most 2048 pixels a side into `--out`. A texture that cannot be read or packed fails the
build, and a failed build leaves the previous page as it was. The page opens on the lobby
(`?players=N` skips it, `?ticks=N` stops play at that tick) and offers "Save replay"; it does
not run from a `file:` address, so serve the folder:

```sh
cargo run --locked -p ludenea-serve -- build ~/games/my_game --web --out ~/games/my_game-web
python3 -m http.server --directory ~/games/my_game-web
```

A known limit of the page: the worker sends the render frame of every tick to the page as
JSON text, about 2 MB per tick for 10,000 sprites, which the page then parses. A binary
transfer of the frame is the planned follow-up. Until then the native window (`play`) is
the reference for the performance target of 144 frames per second.

Online matches meet through a signaling listener: `ludenea rooms` runs one, and serves a
web build beside it with `--page` (the folder `ludenea build --web` wrote; one without
`game.json` is refused). It listens on this machine's network address and on `127.0.0.1`,
both on `--port` (3536 by default, matchbox's own), and prints one
`signaling ws://ADDRESS:PORT/rooms` line per address, then one `page http://ADDRESS:PORT/`
line each with a page. `--listen ADDRESS` listens there only; `--port 0` picks a free port
and needs `--listen`. Without a network it listens on `127.0.0.1` only, and says so:

```sh
cargo run --locked -p ludenea-serve -- rooms --page ~/games/my_game-web
```

Play a game online in two windows: run `ludenea rooms`, host from one terminal, and join with
the code the host printed from another, or from another machine with the `signaling` line the
listener printed for its network address:

```sh
cargo run --locked -p ludenea-serve -- rooms
cargo run --locked -p ludenea-serve -- play ~/games/my_game -- --host
cargo run --locked -p ludenea-serve -- play ~/games/my_game -- --join CODE \
  --signaling ws://ADDRESS:3536/rooms
```

The web page plays online too. `?host=N` (2 to 4 peers) opens a room and `?join=CODE` joins
one, through `?signaling=URL` (by default `ws://` or `wss://` on the page's own host, `/rooms`,
where `ludenea rooms --page` serves it), with the STUN servers of `?stun=URL` (repeatable,
`?stun=none` for none) and `?latency=MS&jitter=MS&loss=PERCENT` on this page's own sends; a
bad parameter fails the page, naming it. The lobby shows the room, its code once the room
exists, and what it waits for; Enter (or `?players=N`) readies the room instead of starting a
match, which starts once every peer is ready. The status names any peer silent, and how the
match ended. The Leave button leaves the match, which every peer ends as left at the same
tick; after the end, "Save replay" saves the match, and a desync also offers "Save desync
snapshot" (`desync-tick-T-peer-P.snap`, to compare with the other peer's using `<game>
diff`). A hidden tab keeps its match going. Half a second after the match ended, the time
it keeps telling the others why, the page closes its link: its room leaves the listener and
no longer counts against the listener's limits. Closing the tab leaves the match on a
best-effort basis only: the page and its link go at once, so the others usually end the
match as lost after 3 seconds of silence, and the closed tab keeps no replay. Leave first to
keep both.
A page and a native peer play in the same rooms:

```sh
cargo run --locked -p ludenea-serve -- rooms --page ~/games/my_game-web
cargo run --locked -p ludenea-serve -- play ~/games/my_game -- --host \
  --signaling ws://ADDRESS:3536/rooms
# then open http://ADDRESS:3536/?join=CODE in a browser
```

A room is `<signaling>/<code>`; the peer that creates it appends `?host=N`, N being its size
(2 to 4). Peers stay connected to the listener for the whole match, so it must stay up for
every match it serves: stopping it ends them. Anyone who can reach it can open rooms, and
anyone who knows a room's code can join that room or disrupt it; there is no authentication,
and it warns when it listens beyond this machine. It bounds what each client may do: 64
rooms (`--max-rooms`), 4 hosted per client address, 30 refused joins a minute per address,
messages of 16 KiB, 64 messages waiting per peer, 10 seconds for a request's headers, 512
connections at once (further clients wait), 32 of them per client address (further ones are
closed at once, so one address never holds them all), and 30 seconds of silence before a
connection closes (`--idle-seconds`; matchbox_socket sends a keep-alive every 10 seconds),
as does a client that stops reading for as long. The limits per address count the TCP peer:
an IPv4 address, or an IPv6 address by its /64 network. Behind a reverse proxy every client
shares the proxy's address, so they share its limits too. Only pages served from the address
in the Host header may open rooms; a page served elsewhere needs `--allow-origin` with its
origin (repeatable). The Host header itself is not checked, so a page proxied under any name
(`wss://` through a reverse proxy) works. A page on a name rebound to the listener passes
the origin check too: any remote site a person on the network opens can reach the listener
through their browser, and open, join or disrupt rooms as anyone on the network can. Its
guesses of room codes are rate-limited like everyone's.

A stock `matchbox_server` works too: pass its root URL as `--signaling`
(`ws://ADDRESS:3536`). It knows nothing of hosts, sizes, or limits, so it admits anyone to
any room.

Peers talk over WebRTC data channels (`ludenea-webrtc`, through matchbox_socket, patched in
`vendor/matchbox_socket`): webrtc-rs natively, the browser's own WebRTC in a page, so native
and browser peers meet in the same rooms. To find each other across a home router, peers ask
a STUN server for their public address: `stun:stun.l.google.com:19302` by default, a public
server of Google's. There is no TURN relay, so two peers that are both behind a strict NAT
(symmetric, as some mobile and corporate networks are) cannot connect. A network that drops
UDP to the STUN server stalls a browser's handshake for about 40 seconds before it falls
back to local addresses; on a local network, `?stun=none` on the page and `--no-stun` of a
native peer skip STUN. Native peers need a network interface other than loopback, even to
play on one machine: webrtc-rs leaves loopback out of its candidates. Native `wss://`
signaling trusts the certificate roots of 2022 (webpki-roots 0.22, which matchbox_socket's
TLS uses), so a server whose certificate chains to a newer root is refused natively;
`ws://`, or a certificate from an older root, works.

Serve it with the editor and open `http://127.0.0.1:7878` in a browser; the editor is
usable at once, and the game appears when its first build finishes:

```sh
pnpm -C editor build
cargo run --locked -p ludenea-serve -- serve ~/games/my_game --editor-dir editor/apps/web/dist
```

`ludenea serve` listens on `127.0.0.1` by default. Remote mode (`--host 0.0.0.0` with
`--allow-host <name>`) is for trusted networks only: there is no authentication, and anyone
who can reach the port can change and build the project, which runs its code. The tool
prints a warning when it listens beyond this machine.

Only pages served from the address the browser used may open the project connection. A
development server on another port that proxies it (Vite on `http://localhost:5173`, for
example) needs `--allow-origin http://localhost:5173` (repeatable).

The project connection takes messages up to 256 MiB, in one frame or several. The largest
message is the session the editor stores after each edit: for a scene of 100,000 entities
(about 11 MB of text), about 68 MB after one edit at tick 0 and 39 MB more for each further
one. A larger message is refused with its size and the limit, which the editor shows in its
console and the tool logs, then the connection closes; reload the editor to connect again.

Built with `--features embed-editor`, the `ludenea` binary carries the editor and needs no
`--editor-dir`. The desktop application (Tauri) runs the same editor with the game as a
native process:

```sh
LUDENEA_ENGINE_PATH=$PWD pnpm -C editor/apps/desktop tauri dev -- -- ~/games/my_game
```

Measure the desktop editor's frame times, unattended: the command builds the renderer and the
desktop application in release, opens a copy of `examples/swarm` in it (made with `ludenea new` in a
temporary directory, so the example gains no files), plays the scene for 10 seconds with the
viewport shown through the game camera, prints the 50th, 95th and 99th percentiles of the time
between animation frames, and fails when the 95th percentile is above 20 ms, when the game
pushed no render frames, when the editor console reports an error during play (such as a frame
the viewport cannot draw), or when the scene cannot load. The window must stay on screen,
since a hidden window draws no frames: the command fails at once on a locked screen, and the measurement fails after 5
seconds without an animation frame. The editor measures only when launched with
`LUDENEA_FRAME_TIMES` set to the file its report goes to, then exits. Interrupting the command
stops the editor, with the game build and the game it started, and removes the temporary
project:

```sh
pnpm -C editor measure:desktop
```

### Viewport

The viewport draws the game with the renderer the game runs with (`ludenea-render`, on WebGPU,
or WebGL2 where the browser offers no WebGPU adapter): its sprites, tilemaps, and layers, with
the debug layers shown on top, the selected entities' shapes highlighted. While the game plays,
the game pushes the frame of each tick it runs (as bytes: transferred from the browser's
worker, base64 in the lines of the desktop game's standard output) and the
viewport draws between the last two, as the game's own window does; paused, it shows the tick
on screen, fetched again after a step, a seek, an undo, an edit, a reload, or a change of debug
layers. The project's textures are loaded when the viewport opens; one that cannot be read or
packed is logged once in the console and drawn as a magenta placeholder. A texture changed on
disk (its PNG or its `.meta`), created, or removed while the editor runs is read again and drawn
at once, with no command to the game; a new version that cannot be read or packed is logged once
and the previous version stays on screen. "View through the game camera" (`C`) shows what the
game's camera sees instead of the editor's free camera; picking and dragging follow it, and
panning, zooming, or framing the selection leaves it for the editor's camera, starting from the
same view. The desktop editor on Linux (WebKitGTK) has no
WebGPU: its viewport draws with WebGL2. When the GPU device is lost (a driver reset, a GPU
removed), the viewport creates its renderer again on a new canvas, with the project's textures
and the frame on screen, and logs a warning. When it cannot render (no GPU adapter, a second lost
device within a minute, three failing draws in a row, or a panic in the renderer), it stops and
shows "The viewport stopped rendering" with the reason and a Retry button; the session keeps
playing meanwhile. A panic aborts the renderer's WebAssembly module: the next renderer, and the
divergence ghost, use a new instance of it.

### Prefabs

A prefab (`.prefab.ron`) is a group of entities a scene places as instances. The scene tree
groups each instance under its prefab file; "Place prefab" adds one, and "Create prefab
from selection" turns selected entities, as the scene holds them whatever the tick shown,
into a new prefab. A field changed on an instance in the scene (at tick 0) is an override
of that one field, marked in the inspector with "Revert to prefab" and "Apply to prefab";
the instance keeps following the prefab elsewhere. "Apply to prefab" moves the override's
value into the prefab, which the scene tree marks unsaved until its "Save prefab" writes
it; one undo takes both back. Opening the prefab ("Open the selected instance's prefab")
edits it as a document; once saved, every scene using it replays with the change and keeps
its overrides.

### Data assets

A data asset is a `.ron` file whose meta names its type (`Data { type }`), such as the
template's `assets/settings.ron`. Systems read it through a `Handle<T>` field and an
`Assets<T>` parameter, so it is part of the simulation's state and of its hash. Select it in
the asset browser to edit it in the inspector: the session replays with the new value from
the tick of the change, and "Save the selected data asset" writes it back to its file.

### Textures and clips

Select a texture (a PNG with its `.meta`) in the asset browser to preview it in the inspector,
with or without a running game: the image zoomed by a whole factor with nearest filtering, each
frame its meta cuts outlined and numbered, each pivot marked, and its size, frame count, and
pixels per unit. A texture its meta cannot cut shows why; one whose meta cannot be read points
at the project's warning in the console. The preview follows changes on disk. A sprite sheet
(`ludenea::SpriteSheet` data asset) shows a player under its fields for each clip: play and
pause, the frame shown and its index, one frame every `ticks_per_frame` ticks at the game's
tick rate, looping. A clip reaching past its texture's frames says so and does not play.

### Tilemaps

Select one entity with a `ludenea::Tilemap` (and its `ludenea::Pose`) and the "Tile palette"
panel shows its tileset's frames; click one to choose the tile to paint (a chosen tile the
tileset lacks is named as such). With the game paused, "Paint tiles" (`B`) and "Erase tiles"
(`E`) turn the viewport's pointer into a brush: a drag selects a rectangle of cells, previewed
over the viewport and clamped to the grid, and paints it on release as one `paint_tiles` edit,
which one undo takes back; a right drag erases. The cells follow the tilemap's layer and its
parallax. A paint at a past tick forks a branch as any edit does, and "Save scene" writes the
tiles. A refused paint is reported in the Console with the game's reason. The tools and the
palette are a plugin on the public editor API.

### Replays

A replay (`.replay`) holds everything a run needs to be checked again: the resolved scene,
its assets, the input and edit logs, and the hash of every tick. "Save as replay" in the
timeline writes the current branch to `assets/replays/`. Opening a replay in the editor
plays it and reports the first tick that no longer matches; a replay whose values no longer
decode (its game's types changed) is refused with the value at fault. The command line tool
verifies one:

```sh
cargo run --locked --release -p boxes -- verify golden/spawner.replay
```

### Local players

While the game plays and the viewport has focus, the editor sends every key and every
gamepad slot, and the control host splits them between the players. With one player, the
whole keyboard and gamepad 1 are that player's. With two or more, player 1 holds the left
half of the keyboard (the keys under the left hand, up to `Space`) and gamepad 1, player 2
the right half and gamepad 2, and every later player the gamepad of their number. A player
whose gamepad is unplugged gets the game's default input, and takes it back when a gamepad
connects. "Set the player count to N" (1 to 4) in the command palette replaces the recorded
inputs with an empty log of N players, goes to tick 0, and logs who holds what; a game that
reads its own `RawInput` per player sees the same split on every runtime.

### Branches

Editing at a past tick no longer discards the recorded future: it forks a branch from that
tick, and the timeline shows one lane per branch under its parent. Click a lane (or "Switch
branch") to go to that branch's future; "Rename branch" and "Delete branch" manage them.
Undoing the edit that forked a branch removes it, and redoing it brings it back. Each
document keeps its session, branches included, when another document opens.

### Divergence tools

"Compare with" picks a second branch: the viewport draws it as a ghost over the current one
("Toggle the ghost of the compare branch" hides it), and the "Divergence report" panel lists,
entity by entity, the first tick each one diverges and its likely causes (the edit at the
fork, or a recorded interaction such as a `ludenea::Collision` with an entity that diverged
before it). A trace (`.trace`) or snapshot (`.snap`) opened from the asset browser shows in
the "Divergence" panel: two traces compare tick by tick to their first divergent tick, and
two snapshots compare entity by entity, component by component, and field by field.

### Plugins

Built-in panels and project plugins use the same API. A project plugin lives in the
project's `editor/index.ts` (or `index.tsx`); the editor bundles it without Node.js and
reloads it on save, without restarting the game. Component values are typed from the
game's schemas (`.ludenea/types/`, written by the editor):

```ts
export default definePlugin({
  name: "my_game",
  activate(editor) {
    editor.panels.register({ id: "my_game.stats", title: "Stats", component: StatsPanel });
    editor.inspectors.register("ludenea::Body", BodyInspector);
    editor.commands.register({ id: "my_game.reset", title: "Reset", run: reset });
  },
});
```

`editor.inspectors.registerAsset` adds an inspector for one type of asset: a data type name
(shown below the asset's generic fields, with its value and an `onChange` that edits a field)
or `"Texture"` (shown alone, with no game needed). `editor.textures` lists the project's
textures (`list` and `subscribe`, following changes on disk) and reads a texture's PNG
(`bytes`), or the frames its meta cuts it into with that PNG (`frames`, rejected with the
renderer's reason). `find(id)` returns the texture's `TextureInfo`, or `"listing"` before the
first listing answers, `"missing"` when no texture has that identifier, `"unreadable"` when
its meta cannot be read, or `{ failed }` with the reason the listing failed.
The built-in texture previews and clip players are a plugin on this API.

`editor.edit(command)` sends an editing command to the running game, as one undo step, and
resolves with the game's reply (a `spawn` gives the entity it made). `editor.batched(commands)`
sends several entity edits (spawn, despawn, components, fields, tiles) as one, and
`editor.onEdited` hears each accepted edit.
`editor.entities.watch(entity, listener, despawned)` follows one entity's components, and
`despawned` hears when it is no longer alive at the tick shown.

`editor.viewport.tools.register` adds a viewport tool. Once activated with
`editor.viewport.setTool(id)`, it receives the pointer events over the viewport in world
units (`pointerCancel` when a press ends without its release), maps points onto a layer of
any parallax both ways (`screenToWorld`, `worldToScreen`), and draws its own overlay, until
Escape or until its `available(editor)` answers false. Panning and zooming stay the
viewport's whatever the tool:

```ts
editor.viewport.tools.register({
  id: "my_game.spawn",
  title: "Spawn a crate",
  cursor: "crosshair",
  available: (editor) => editor.control !== null,
  pointerDown: ({ world }, { editor }) =>
    editor
      .edit({ type: "spawn", components: { "ludenea::Pose": { position: world, rotation: { cos: 1, sin: 0 } } } })
      // The reply names the new entity: select it.
      .then(({ entity }) => editor.selection.getState().select([entity]))
      .catch((error) => editor.console.report(error, "Spawning a crate")),
});
```

`editor.console` writes to the Console panel: `log` shows a message, `report` a failure.
A command that fails from the palette or a shortcut is reported there under its title.

A panel, custom inspector, field decoration, or explorer column that throws while rendering
costs only its own place: a panel shows the failure with a "Reload panel" button, a custom
inspector gives way to the generic fields, and a decoration or cell shows a mark until what it
shows (its entity, component, or column values) changes. An explorer column whose `subscribe`
or unsubscribe throws is left unsubscribed. The failure is reported in the Console once,
naming the plugin (`editor.pluginOf`), and a new registration (the plugin rebuilt) renders it
again. A plugin that renders other plugins' registrations wraps each in `PluginBoundary` for
the same isolation, with `resetKeys` to render a failed slot again once what it shows changes.

### Development

The front end lives in `editor/` (pnpm workspace; Node.js 24 and the pnpm version in
`editor/package.json`, for building the editor only):

```sh
pnpm -C editor install --frozen-lockfile
pnpm -C editor build:render            # the renderer, into editor/packages/render-wasm, before typecheck and build
pnpm -C editor typecheck
pnpm -C editor test
pnpm -C editor build
```

End-to-end tests (Playwright, Chromium) drive the built web editor against
`ludenea serve`: the golden trace and replay in a browser, a session replayed by the command
line tool, plugin parity, prefabs, branches, the divergence tools, saved replays,
fluidity with 100,000 entities, a branch, and the ghost, and the swarm's 10,000 sprites
playing in the viewport from frames the game's worker pushes as bytes. `PERF_STRICT=1` turns
the frame time reports into assertions on the reference machine (an Apple M-series): 17 ms at
the 95th percentile for the editor's panels, and 20 ms for the swarm, which then draws with
the machine's GPU:

```sh
cargo xtask wasm-game boxes editor/e2e/golden/pkg
cargo xtask wasm-render --test-hooks editor/e2e/fixtures/renderer/pkg
cargo run --locked -p boxes --bin editor-bench
cargo build --locked -p ludenea-serve -p swarm -p arena
cargo run --locked -p ludenea-serve -- build examples/swarm --web --out editor/e2e/fixtures/standalone
cargo run --locked -p ludenea-serve -- build examples/arena --web --out editor/e2e/fixtures/arena
pnpm -C editor e2e
```

The standalone page's tests (`editor/e2e/standalone.spec.ts`) open the swarm built as a web
page: it draws, its hash at tick 300 matches `swarm run --hash`, the lobby claims keyboard
halves, a saved replay verifies with `swarm verify`, and a `file:` address says to serve the
folder. They open the arena's page too: its hash at tick 300 matches `arena run --hash`, and
a two-player match played with keys saves a replay that `arena verify` accepts. The arena's
editor tests (`editor/e2e/arena.spec.ts`) play it in the editor to the hash of `arena run`,
record a two-player match with keys whose saved replay verifies, and check the viewport draws
the painted level. The online tests (`editor/e2e/online.spec.ts`) start `ludenea rooms` on a free
port: two arena pages play each other until one leaves, and save the same replay, which
`arena verify` accepts; a native `arena peer` plays a page under 50 ms of latency, 10 ms of
jitter and 5 percent of loss, and both save the same replay; a match that loses its listener
ends on both pages naming why the link closed; a join the game refuses fails the page, which
leaves the room; an unknown room and bad parameters fail the page, naming them.

The renderer's pixel tests (`editor/e2e/renderer.spec.ts`) draw a reference frame with the
browser renderer (`ludenea-render` with its `web` feature) and run twice, once per backend,
with no GPU: the `webgl2` project on WebGL2 (SwiftShader through ANGLE), and the `webgpu`
project on WebGPU, where headless Chromium offers SwiftShader's adapter with
`--enable-unsafe-webgpu`, on macOS and Linux alike. The `webgpu` project fails, never skips,
when the browser offers no WebGPU adapter. They also check what the page asks the renderer
without drawing (picking, bounds, a texture's frames) and that its errors are named. The page
pushes `fixtures/renderer/frame.json` both as JSON and in postcard (`frame.postcard`, which a
Rust test checks against it); after a change to either, write it again with
`LUDENEA_WRITE_FIXTURES=1 cargo test -p ludenea-render --test web_fixture`. Run them alone with:

```sh
pnpm -C editor e2e renderer.spec.ts
```

The divergence viewer's fixtures (`editor/e2e/fixtures/divergence/`) come from the `boxes`
app. The tests never regenerate them; after a change to the trace, snapshot, or replay
formats, write them again and commit them:

```sh
cargo run --locked -p boxes --bin divergence-fixtures editor/e2e/fixtures/divergence
```

The timing and memory budgets (switching branches, the divergence report, checkpoints of 20
branches) are `#[ignore]`d tests, run in release:

```sh
cargo test --locked --release -p ludenea-control --test branches --test divergence -- --ignored --nocapture
```

After changing protocol types, regenerate the TypeScript bindings and the declarations
project plugins check against (CI fails when they are stale). The `typescript` tests also
pin each protocol's fingerprint: when the shape changes they fail with the new value, which
goes into `PROTOCOL_FINGERPRINT` (or `PROJECT_PROTOCOL_FINGERPRINT`) and into the
changelog entry of that protocol version.

```sh
cargo test --locked -p ludenea-control --test typescript
cargo test --locked -p ludenea-project --test typescript
node editor/scripts/bundle-dts.mjs
```
