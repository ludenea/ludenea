# Changelog

Every change to the public Rust API, the control protocol, the project protocol, the plugin
API, or a file format is recorded here, under [Unreleased](#unreleased), in the commit that
makes it. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the
rules for versions are in the [README](README.md#versioning).

## Formats and protocols

The version each format or protocol writes (Current) and the oldest version this build still
reads. A test checks the Current column against the version constants, so a bump updates
this table in the same commit.

| Format                 | Constant                   | Current | Oldest readable               |
| ---------------------- | -------------------------- | ------- | ----------------------------- |
| Control protocol       | `PROTOCOL_VERSION`         | 5       | 5                             |
| Project protocol       | `PROJECT_PROTOCOL_VERSION` | 4       | 4                             |
| Scene (`.scene.ron`)   | `SCENE_VERSION`            | 2       | 1                             |
| Prefab (`.prefab.ron`) | `PREFAB_VERSION`           | 1       | 1                             |
| Meta (`.meta`)         | `META_VERSION`             | 2       | 1                             |
| Atlas (`atlas.json`)   | `ATLAS_VERSION`            | 1       | 1                             |
| Web game (`game.json`) | `WEB_GAME_VERSION`         | 1       | 1                             |
| Replay (`.replay`)     | `REPLAY_VERSION`           | 1       | 1                             |
| Trace (`.trace`)       | `TRACE_VERSION`            | 1       | 1                             |
| Snapshot (`.snap`)     | `SNAPSHOT_VERSION`         | 3       | 2                             |
| Input log (`.inputs`)  | `INPUT_LOG_VERSION`        | 1       | 1                             |
| Edit log               | `EDIT_LOG_VERSION`         | 3       | 1                             |
| Undo history           | `UNDO_VERSION`             | 4       | the unversioned form before 2 |
| Stored session         | `STORED_SESSION_VERSION`   | 1       | the unversioned form before 1 |
| Network protocol       | `NET_PROTOCOL_VERSION`     | 1       | 1                             |

The stored session is the editor's copy of each open document's session, in
`.ludenea/sessions/`; its version is a TypeScript constant of `@ludenea/editor-shell/app`.
The meta reader does not check its version yet: it reads a meta of any version whose fields
parse.

Not versioned yet: data assets (`.ron` files holding a value of the game type their meta
names), the project manifest `ludenea.toml`, the branch tree of a session, the plugin API,
and the encoding the state hash is computed from: a change to it can move every recorded
hash, those of the golden trace and replays included.

## [Unreleased]

### Rust API

- Breaking: `Component` and `Resource` require `Clone`, so a saved state can clone a
  storage instead of encoding it. The derive macros do not add it: add `Clone` to the
  `#[derive(...)]` of every component and resource type (the compiler names the missing
  bound). A hand-written `Clone` must give the same value a postcard round trip gives, as
  the saved state relies on it. `Asset` keeps its bounds: loaded assets are shared behind
  `Arc`.
- `World::save_into(&mut SavedWorld)` saves the simulation state in memory (the entity
  allocator, every component storage and resource, and the loaded assets, shared rather than
  copied), reusing the allocations of the `SavedWorld` it saves into and copying the asset
  map only when it changed since the last save. `World::load(&SavedWorld)` loads it back,
  dropping queued commands and events and keeping runtime resources as `World::restore`
  does, all or nothing: it refuses with a `SavedError` (`Empty`, `MissingStorage`,
  `UnexpectedStorage`, `TypeMismatch`, each naming the stable name of the component,
  resource, or asset type that differs) before changing anything.
  `Simulation::save_state(&mut SavedState)` and `Simulation::load_state(&SavedState)` wrap
  them for a rollback that saves every tick; `load_state` refuses a state never saved into
  (`StateError::Empty`), one saved by another game build (`StateError::OtherWorld`, naming
  both schema fingerprints), and a world that does not match (`StateError::World`), leaving
  the simulation unchanged. A loaded state gives the same snapshot, byte for byte, as the
  one it was saved from.
- `Simulation::player_input_spec()` gives a `PlayerInputSpec` (`None` for an app without an
  input type): the encoding of one default input, and a `check` that accepts only the
  canonical postcard encoding of exactly one input (it decodes, encodes again and compares,
  so an overlong varint is refused with `SimError::InputDecode` naming both lengths).
- `ludenea_net::Session` is the sans-IO rollback session of one peer.
  `SessionSettings::validate` refuses a match where peer 0 owns every player
  (`SettingsError::SinglePeer`): a rollback session needs at least two peers, and a
  single-peer game steps its `Simulation` directly.
  `Session::new(SessionSettings, PlayerInputSpec)` validates the settings and refuses a
  default input over 24 bytes (`SettingsError::DefaultInputTooLarge`) or refused by the
  check (`SettingsError::DefaultInputRefused`). `receive(from, packet, now_micros)` and
  `outgoing(now_micros)` run the handshake: a `Hello` to every peer that has not welcomed
  this one, at most every `HANDSHAKE_RESEND_MICROS` (100 ms), a `Welcome` for every
  matching `Hello`, and `SessionState::Running` with `SessionEvent::Started` once every
  peer has welcomed it and sent a matching `Hello`. Another view of the match ends it with
  `EndReason::Mismatch`, naming the peer, the first differing field (`net_version` while
  connecting, then the identity's fields in order, then `match_id`) and both values; a
  peer whose handshake is unfinished and that stays silent for `disconnect_after_micros`
  while connecting ends it with `EndReason::Unreachable`. A packet that is not one of this
  match from another of its peers is dropped, counted per sender and `DropReason` in
  `Session::stats`, and reported as `SessionEvent::Dropped`.
- A running `ludenea_net::Session` predicts and rolls back. `add_local_input(player, input)`
  sets a local player's input for the next advanced tick (a second call replaces it), and
  refuses with a `SessionError` (`NotRunning`, `NotLocal`, `InputTooLarge`, `InvalidInput`).
  `advance(now_micros)` returns the `Request`s for the simulation (`Save { tick, hash }`
  right before every `Advance`, `Load { tick }`, `Advance { tick, inputs }` with one input
  per player): first a rollback from the first mispredicted tick, then at most one new tick,
  or a stall once the current tick is `rollback_window` ticks past the confirmed one. It
  refuses with `SessionError::MissingLocalInput` when a tick would advance without every
  local input, and `SessionError::LastTick`. A local input applies `input_delay` ticks later;
  the ticks before hold the default input. Remote players are predicted by repeating their
  last confirmed input. `SessionEvent::Confirmed { tick, inputs }` follows every tick whose
  inputs are confirmed for every player, in tick order. Every packet after an advance
  carries the local inputs a peer has not acknowledged, oldest first within one packet, and
  an `Ack`. `PeerStats` gains `rollbacks`, `deepest_rollback`, `prediction_misses` and
  `stalls`, and `DropReason` gains `Gap`. A connecting session reads the inputs and
  acknowledgments of a peer whose matching `Hello` it has, and takes them as that peer's
  `Welcome`: only a running peer sends them.
- A `ludenea_net::Session` disconnects a peer that breaks the protocol: it ends with
  `EndReason::Violation { peer, player, tick, rule }`, naming the player and tick when there
  is one, and the `ViolationRule`: `NotOwner` (an input of a player the sender does not own,
  or an acknowledgment of a player the receiver does not own), `ConflictingInput` (the
  confirmed input is kept), `InvalidInput` (refused by the input check), `BeyondHorizon` (an
  input past `h + 1 + rollback_window + input_delay`, where the horizon `h` is the highest
  tick a local player has an input for, or a tick range past the last tick a `u64` numbers),
  `AckNeverSent` (an acknowledgment of an input or hash report never sent) and `AckLagging`
  (an acknowledgment trailing `h` by more than `2 * (rollback_window + input_delay) + 1`
  ticks, or none for that long). These bound the remote and the unacknowledged local inputs
  a session keeps for a hostile peer. Inputs that start after the next expected tick are
  still dropped as `DropReason::Gap`.
- A running `ludenea_net::Session` keeps its ticks in step with the other peers. Every
  `QUALITY_INTERVAL_MICROS` (200 ms) it sends each peer a `Quality` request with its tick
  and frame advantage, and answers each request with a `QualityReply`. A reply that echoes
  no pending request (at most `QUALITY_HISTORY` = 16 kept) is dropped alone as
  `DropReason::UnknownEcho`, the rest of its packet read. `PeerStats` gains `ping_micros`
  (the mean of the latest 16 round trips), `jitter_micros` (their mean absolute deviation)
  and `frame_advantage`. When the mean correction `(local_advantage - remote_advantage) / 2`
  of the latest `SYNC_MIN_REPORTS` (4) fresh reports of a peer, rounded to the nearest tick,
  is at least one tick, `advance` skips that many ticks for the peer furthest behind, at
  most `rollback_window / 2`, counting only peers the session is ahead of by its own
  estimate, and stops once it no longer is: each skip returns no `Advance`, keeps the
  pending local inputs and counts in the new `SessionStats::skips`.
- A running `ludenea_net::Session` compares state hashes with the other peers. The `Save` of
  every positive multiple of `hash_interval` has `hash: true`: the caller answers with
  `Session::report_hash(tick, hash)` before the next `outgoing`
  (`SessionError::HashNotAsked` for a tick no `Save` waits on) and keeps that state until
  the new `Request::Release { tick }`. The hash of a tick is sent once every input below it
  is confirmed, the rollback those inputs call for has run and its `Save` is answered, so it
  is always the hash of a state built only from confirmed inputs; every packet carries every
  report `Ack.hashes_through` does not cover yet (acknowledging a report never sent stays
  `ViolationRule::AckNeverSent`). When every peer's hash matches, the session emits
  `SessionEvent::HashMatched { tick }` and releases the states kept for earlier reports. A
  difference ends it with `EndReason::Desync { tick, peer, local, remote }`, naming the peer
  and both hashes. A received hash past the local horizon plus one is
  `ViolationRule::HashBeyondHorizon`; one of tick 0 or off the interval
  (`DropReason::HashOffInterval`), received already with another hash
  (`HashAlreadyReceived`; the same hash resent until acknowledged is ignored), or beyond the
  `(rollback_window + input_delay) / hash_interval + 2` reports kept waiting for a local
  hash answered late (`HashBacklog`), or after a report of that peer still missing
  (`HashGap`, as reports arrive in tick order) is dropped alone, and reports match in tick
  order. `ViolationRule::HashLagging` disconnects a peer whose newest report trails the
  newest final local hash by more than `rollback_window + input_delay + hash_interval - 1`
  ticks, or whose acknowledgment of the local reports trails it by more than `2 *
  (rollback_window + input_delay) + hash_interval - 1`, which no honest peer does: this
  bounds the reports a session keeps and the states its caller keeps for them. The
  worst-case packet of `SessionSettings::validate` counts a `Welcome` and the
  `MatchRules::unacknowledged_hash_reports()` reports a peer may leave unacknowledged, `(2 *
  (rollback_window + input_delay) + hash_interval - 1) / hash_interval` (1 with the
  defaults); a worst packet of more than 16 messages is refused with
  `SettingsError::PacketTooManyMessages`, naming the peer and both counts (with the default
  window and delay and one player per peer, a `hash_interval` of 1). An ending a session
  decides alone that an end notice tells of sends that `End` notice, and nothing else, to
  every peer at most once a tick for `END_LINGER_MICROS` (500 ms); a received `End` ends the
  receiver with the same reason, the new `EndReason::Lost { peer }` and
  `EndReason::LocalFailure { tick }` included, a desync with the view of the peer that found
  it (its hash as `local`, the differing peer and that peer's hash). A notice no honest peer
  can send is `ViolationRule::ImplausibleEnd`: one naming a peer outside the match, a desync
  of a tick past the local horizon plus one, or a game failure past that plus
  `rollback_window - 1`. A mismatch found once running has no end notice and is not relayed.
- A `ludenea_net::Session` ends a match for every peer at a tick they agree on. A peer silent
  for `notice_after_micros` while the match runs raises `SessionEvent::PeerSilent { peer,
  for_micros }`, once per silence, and `SessionEvent::PeerBack { peer }` when it speaks again;
  silent for `disconnect_after_micros`, it ends the session with `EndReason::Lost { peer }`,
  whose end notice ends every other peer too (a notice of a peer lost that is its own sender
  is `ViolationRule::ImplausibleEnd`). `Session::leave()` (`SessionError::NotRunning` unless
  running) goes `SessionState::Leaving`: the last tick is the local input horizon, the
  session takes no more local input, runs up to that tick, and every packet carries a
  `Goodbye { last_tick }`. Every peer then confirms no tick past the lowest last tick heard,
  runs no tick past the one after it, and ignores inputs past it; it ends with
  `EndReason::Left { peer, last_tick }` (`peer` the lowest one naming that tick) once every
  player's inputs through that tick are confirmed and every peer acknowledged its own, so
  every peer stops at the same tick with the same confirmed inputs; it then sends every peer
  an `Ack`, and the leaver its `Goodbye`, at most once a tick for `END_LINGER_MICROS`. The
  leaver ends anyway `disconnect_after_micros` after `leave`. A peer holding every input
  through that tick ends with `Lost { peer }` when a peer still talking makes no progress
  acknowledging its inputs for `disconnect_after_micros`: that peer cannot hear it. A
  `Goodbye` below the receiver's last confirmed tick is `ViolationRule::GoodbyeBehind`, one
  past the furthest input an honest peer can have `ViolationRule::BeyondHorizon`. Whatever
  the ending, once the session ran, the next `advance` (or the one that ended it) returns
  the requests that land the simulation on the state the confirmed inputs give at the end
  tick: a `Load` of the latest tick built only from confirmed inputs, then a `Save` (asking
  no hash) and an `Advance` of every confirmed tick from it; no `Release` follows, and the
  caller may drop the states it keeps. Later calls are `SessionError::NotRunning`. Every
  `Load` is of a tick at most `rollback_window` before the current one.
  `Session::settings()` gives the settings a session was built with.
- `ludenea_net::Executor` drives a `Simulation` from a `Session`. `Executor::new(&mut
  Simulation, &SessionSettings)` sets the simulation's player count to the match's and
  refuses an app without an input type (`ExecuteError::NoInputType`). `execute(&mut
  Simulation, &mut Session, Vec<Request>)` applies an advance's requests in order: it checks
  that each `Save` and `Advance` is at the simulation's tick (`ExecuteError::OutOfStep`),
  answers each `Save { hash: true }` through `Session::report_hash` at once, steps with the
  inputs joined by `ludenea_app::join_player_inputs`, and loads the latest state saved for a
  tick (`ExecuteError::MissingState` when none is held, `ExecuteError::State` when the
  simulation refuses it). It holds at most `Executor::capacity()` states, tagged with their
  tick: `rollback_window + 2`, plus the reported states the hash lag bounds let a session
  keep until their `Release` (`ExecuteError::StatesExhausted` past that; a `Release` of a
  state held but not kept is `ExecuteError::NotKept`). When the game fails while stepping a
  tick, it ends the session with `Session::fail` (a session that had ended already, whose
  landing failed, keeps its reason), loads that tick's state, and returns
  `ExecuteError::Game { tick, error, recovery }`, where `recovery` names the first of those
  two steps that failed, if any.
  `Session::fail(tick)` ends the session with `EndReason::LocalFailure { tick }`, whose end
  notice ends every other peer, and lands nothing (the next `advance` is
  `SessionError::NotRunning`); it refuses a tick the session has not run
  (`SessionError::TickNotRun`), which the other peers would find implausible, and
  `NotRunning` while connecting. `ludenea_app::join_player_inputs(&[Vec<u8>])` gives the
  postcard encoding of a `Vec` of inputs from the encoding of each.
- The per-entity half of the divergence report moves to `ludenea_app::divergence`, so any
  two simulations can be compared in the shape of the editor's branch report.
  `EntityDivergence`, `DivergenceKind` and `Cause` move there unchanged and stay reachable
  from `ludenea_control::divergence`; the control protocol and its TypeScript bindings do not
  change. A `TickRecorder`, passed to `Simulation::step_observed`, gives the events and
  commands of one tick as a `TickRecord` (`into_record(tick)`).
  `EntityComparison::new(&Simulation)` starts a comparison of simulations of that app,
  taking its event schemas and system names from it. `compare([&Simulation; 2], records,
  explains_from)` gives the entities diverging for the first time at the tick both
  simulations stand at (`OnlyInCurrent` means only in the first, `OnlyInCompare` only in the
  second), each with its first difference and the events and commands of `records` (one
  slice per simulation) at that tick or the tick before that may have caused it; a
  divergence before `explains_from` has no cause. It refuses simulations at different ticks
  (`ComparisonError::DifferentTicks`, naming both). `first_divergent()` gives the first tick
  whose world hashes differ, and `take_warnings()` the problems found since the last call,
  such as a value that failed to decode for a diff.
- `ludenea_render` answers without a GPU what the renderer draws: `atlas::frames_of(png,
  meta)` gives the `TextureFrames` (the image size and each `FrameRect`, row-major, with its
  pivot) a meta cuts a PNG into, the same cut `pack` makes, or a `FramesError` (`Decode`,
  `Grid`) naming the cause; `camera::screen_to_world` is the inverse of `world_to_clip` for a
  layer, and `camera::world_to_screen` the inverse of `screen_to_world`; `pick::pick` lists
  the entities whose drawn sprite quad (a missing texture's 1 by 1 placeholder included) or
  painted tilemap cell lies under a screen point, topmost first; and `pick::bounds` gives the
  world box of entities drawn on layers of parallax 1.
  `gpu::Size` moves to `camera::Size` (still reachable as `gpu::Size`), and
  `Renderer::regions` gives the regions of the atlas set last. `Renderer::set_atlas` refuses
  an atlas with a region on a page it does not have (`RenderError::InvalidAtlas`).
- `ludenea_runtime::native::play` repacks the textures that change on disk while a game
  plays, at most once per frame: `PlayOptions::watch` names the scene and project folder
  whose textures `native::watch::TextureWatch` watches (debounced, `.png` and `.png.meta`
  files only), reading them again on its own thread so a frame never waits for the files. A
  texture whose new version cannot be read, whose meta is invalid, or that the atlas refuses
  (a PNG that cannot be decoded, cut into frames, or fitted on a page) keeps its previous
  version, with a warning (`<path>: <cause>, keeping the previous version`); a texture
  removed from the project is dropped. The simulation, its state hashes and the replay never
  see a reload. `RuntimeError::Watch` reports a watcher that cannot start, which `play`
  writes as a warning before playing without reloads. The command line tool's `play` watches
  unless `--no-watch` is given.
- Breaking: `ludenea_app::project::project_textures` reports a `.png` that cannot be read,
  or whose meta cannot be read or is invalid, as a `TextureFailure { path, cause }` (shown
  as `<path>: <cause>`, such as `<path>: cannot read its meta: <reason>`) in the new
  `ProjectTextures::failures`, sorted by path, instead of failing the whole call or a
  warning; `ProjectTextures::warnings` is removed, its warnings being these failures. Only
  a folder that cannot be listed, or two textures sharing an identifier, still fail it.
  `render` and `play` print each failure as a warning, as before, and a web build lists
  them among its texture errors. The new `project_folders(scene, project)` gives the
  folders `project_assets` and `project_textures` scan.
- `TextureError::cause` gives why a texture was refused without naming it: its message is
  the texture's name then that cause.
- Breaking: `TextureError::Grid` holds the `reason` (`FramesError::Grid`'s message) instead
  of the `image` size and the `grid`; its message is unchanged.
- `WebRenderer::push_frame_bytes(bytes, overlay)` pushes a frame in the control protocol's
  postcard encoding (`ludenea_control::protocol::encode_frame`), as the session's worker
  sends it. `push_frame` and `push_frame_bytes` return `{ tick, missing, debug }`: the
  frame's tick and debug draw beside what is drawn as a placeholder. `pick(x, y, camera)`,
  `bounds(entities)`, `screen_to_world(x, y, parallax, camera)` and its inverse
  `world_to_screen(x, y, parallax, camera)` answer for what the last
  `draw` showed (the frame pushed last, blended at that draw's alpha), in CSS pixels of the
  canvas's layout until the first `resize`; `pick` and `bounds` find nothing from new
  textures to the next push. `panicForTesting` exists only in a build with the `test-hooks`
  feature (`cargo xtask wasm-render --test-hooks`), for the renderer page's tests. The free
  functions `textureFrames(png, meta)`, `ghostFrame(frame)` (the ghost style of a debug
  draw, `DebugDrawFrame::ghost`) and `setPanicHandler(handler)` (called
  with the message of any panic in the module) join it.
- Breaking: every `WebRenderer` error is a JavaScript `Error` whose `name` says what failed
  (`NoAdapter`, `Gpu`, `Limit`, `DeviceLost`, `InvalidAtlas`, `InvalidInput`, `Surface`,
  `NoSurfaceTexture`) and whose `message` is the cause, instead of a string. A lost device is
  reported by `draw` as `DeviceLost` before the canvas is asked for a texture. The standalone
  web page tells a canvas without a texture this frame by the name `NoSurfaceTexture`.
- Breaking: `ludenea_draw::render` and its `render` feature are removed, with
  `cargo xtask wasm-renderer` and the editor's `pnpm build:wasm` and `@ludenea/debug-draw-wasm`:
  the editor's viewport draws, picks and frames with `WebRenderer`, and the divergence ghost
  is styled by `ghostFrame`. The size-optimized browser build profile is renamed
  `wasm-render`.
- `Claim::describe` names a claim's devices in words ("left keyboard half and gamepad 1");
  the native window's title and the web page's lobby (`WebLobby::describeClaims`, a JSON
  array of those texts in slot order) both show it.
- `WorkerHost::renderFrameJson(layers)` is the worker transport's raw encoding of
  `get_render_frame`: refused before the hello as a request is (`not_connected`), it returns
  the current tick's `RenderFrame` as JSON, so a page forwards the frame text without parsing
  it, and the events it causes come first in the next `handle` or `advance`.
- Breaking: `WebRenderer::push_frame(current, overlay)` replaces `WebRenderer::set_frames`,
  with no alias: it sets the next frame, blended from the frame pushed before it only when
  that is the tick before (`RenderFrame::follows`), so a page reads each frame once and a
  frame after a gap in ticks (a step of several ticks, a seek, frames left undrawn) draws as
  is.
- `ludenea build <project> --web --out <folder>` builds a game as a standalone static web page,
  which any web server serves without the editor or `ludenea serve`: the simulation runs in
  a Web Worker driving the control `WorkerHost`, the page draws with `WebRenderer` between
  the last two ticks, runs the lobby in the page (four slots naming their devices) or skips
  it on `?players=N`, captures keys and gamepads, stops at `?ticks=N`, offers "Save replay"
  (downloaded as `<name>.replay`), and says to serve the folder over HTTP when opened from a
  file. It exits with code 2 on any failure. `Project::build_web(out, output)` does the
  build: it refuses a game whose `Cargo.toml` has no `web` feature forwarding `ludenea/web`
  (`BuildError::NoWebFeature`, naming the line to add under `[features]`), builds the library
  for `wasm32-unknown-unknown` with that feature, packs the textures with pages of at most
  2048 pixels a side and fails listing every texture that cannot be read or packed
  (`BuildError::Textures`), and writes `index.html`, `page.js`, `worker.js`, `style.css`,
  `game.js`, `game_bg.wasm`, `game.json` and the atlas files (`atlas/atlas.json`,
  `atlas/page-<n>.png`) into a staging folder beside `out` before it replaces `out`, so a
  failed build leaves the previous one as it was. An existing `out` that is neither empty nor
  a previous web build is refused (`BuildError::NotAWebBuild`), as is a project without a
  `default_scene` (`BuildError::NoScene`) and game files that cannot be read
  (`BuildError::Assets`); a file of the build that cannot be read or written is named
  (`BuildError::File`). `game.json` (`WEB_GAME_VERSION` 1) holds `version`, `name`, the
  scene text (`scene`) and the asset texts (`assets`, `[identifier, text]` pairs); the page
  refuses another version. The project template declares `[features] web = ["ludenea/web"]`;
  a project made before needs that line. Known limit: the worker sends each tick's render
  frame to the page as JSON text (about 2 MB per tick for 10,000 sprites); a binary
  transfer is the follow-up, and the native window is the reference for the performance
  target.
- `ludenea_runtime::web` (feature `web`, `wasm32-unknown-unknown` only) exports the
  standalone web page's lobby: `WebLobby` (`new`, `pressKey(code)` and
  `pressButton(gamepad, button)` returning the `LobbyEvent` as JSON, `claimsJson()`,
  `started`) and `automaticClaimsJson(players)`, which refuses a count outside 1 to
  `MAX_LOCAL_PLAYERS`. It links the browser renderer, `WebRenderer`, beside them. The
  facade's new `web` feature turns it on and the facade re-exports the runtime as
  `ludenea::runtime`. `LobbyEvent` serializes tagged by `type` in snake case.
- `WorkerHost::tickProgress()` returns the fraction of the next tick that play owes, from 0
  to below 1, so a page drawing a game that runs in a Web Worker interpolates its last two
  ticks.
- `ludenea_render::atlas::Atlas::to_files` writes a packed atlas as an `AtlasFiles`: a JSON
  manifest (`atlas.json`, `ATLAS_VERSION` 1: `version`, `pages` of `{ file, width, height }`
  and `regions` of `{ id, frame, region }`) and one PNG per page (`page-<n>.png`), byte for
  byte reproducible (the PNGs use the `fdeflate` encoder, `Compression::Fast`, and a fixed
  `Sub` filter, which no cargo feature changes, and a test pins their bytes).
  `Atlas::from_files` reads them back, refusing a newer version, a missing page, a page of
  another size, a page file not named `page-<index>.png`, a region outside its page and an
  undecodable PNG with `AtlasFileError`. Textures left out of the atlas are not written.
  `WebRenderer::set_atlas(manifest, pages)` uploads such an atlas in the browser and, like
  `set_textures`, blanks the sprites until the next `push_frame`.
- `ludenea play <project> [--scene PATH] [-- args...]` builds a project natively and runs its
  binary's `play` with `--project` and the forwarded arguments, returning the game's exit
  code (2 when the build or the launch fails, or without a scene: pass `--scene` or set
  `default_scene`). The project template's input mapping reads the arrows, WASD, and
  gamepad 0.
- `ludenea_app::players` assigns local players their devices without a window or a GPU:
  `Claim` (at most one `KeyboardPart` of `Whole`, `Left` or `Right`, and one gamepad slot),
  `LiveDevices`, `automatic_claims`, `check_claims` (refusing a gamepad or a keyboard part
  held twice with `ClaimError`) and `split`, which gives each player its keys and its
  gamepad, or `None` when its only device is disconnected. `MAX_LOCAL_PLAYERS` is 4.
  `Simulation::map_players` takes an `Option<RawInput>` per player and encodes
  `I::default()` for `None`; `map_raw_input` delegates to it. The prelude exports `Claim`,
  `KeyboardPart` and `LiveDevices`.
- New crate `ludenea-runtime` runs a game on its own through the control host. `Runner`
  loads `GameFiles` (scene, assets, optional edit log), `start`s the match with one `Claim`
  per player, and per displayed `frame` sends the live devices when they changed, lets the
  host's pacer run the ticks due (at most its cap, however long the frame took) and fetches
  the render frame of the newest tick; `FrameUpdate` carries the interpolation `alpha`.
  `export_replay` records the match, after a failure too. A game that fails or panics is
  `RuntimeError::Game`, naming the tick. `take_messages` returns what the host wrote to its
  console, such as a loaded edit that no longer applies. `stats::FrameStats` summarizes
  frame times as percentiles. With the `native` feature, on native targets only,
  `native::keys::code_name` names winit keys as `KeyboardEvent.code` does, and
  `native::gamepads` turns gilrs gamepads into the Gamepad API standard layout (`standard`)
  and keeps each pad's slot while it is unplugged (`Slots`). Without the feature the crate
  depends on no window, gamepad or GPU crate. On Linux, the feature needs libudev's
  development files to build.
- `ludenea_runtime::native::play(app, options, out)` runs a game in a winit window: the
  lobby (unless `PlayOptions::players` is given), then the match, drawn with the wgpu
  renderer at the display's rate between the last two ticks, with the keyboard and up to
  four gamepads. `PlayOptions` names the files, textures, players, `ticks` to stop at, the
  replay to `record` (written when the window closes, after a failure too), fullscreen,
  vsync, `stats`, threads, debug layers, size and title; `PlayOutcome` holds the tick
  reached and the frame time summary. The window's warnings (textures left out of the atlas,
  a lost device, gamepads that cannot be read) go to `out`, the game's console to the
  standard error. F11 toggles borderless fullscreen. `native::surface::Presenter` presents
  to the window's surface: it skips drawing at zero size and while the window is hidden,
  configures the surface again when it is lost or outdated (failing once it stays so for 5
  seconds), skips a frame that timed out, and fails on a texture the GPU refuses. A lost
  device, found before a texture is asked for or by a refused texture, is replaced by a new
  device, renderer and atlas, and the frame drawn again; when that cannot open, or the new
  device is lost too, play fails with both causes. `native::lobby::frame` draws the lobby on
  the debug layer `runtime.lobby` and `native::lobby::title` names each claim and how to
  start. `native::gamepads::button_index` and `native::gamepads::state` read a gilrs pad in
  the standard layout. `RuntimeError::Record` reports a replay that cannot be written.
  `Runner::stop_at(tick)` stops play at a tick, so a replay holds exactly the ticks up to
  it.
- `ludenea_render::gpu::for_surface(window)` opens a surface on a window and a device that
  presents to it, made with the window's display handle, with the same fallback adapter and
  `RenderError::NoAdapter` as `headless`; it also returns the adapter, whose capabilities
  the surface is configured from. Not on WebAssembly. `Renderer::check_device` returns the
  device's loss or pending errors without drawing.
- The command line tool's new `play` plays a scene in a native window: `--players` (1 to 4,
  skipping the lobby), `--ticks`, `--record`, `--fullscreen`, `--no-vsync`, `--stats` (the
  frame time percentiles, printed when the window closes), `--threads`, `--debug-layer`,
  `--width` and `--height` (1280 by 720 logical pixels by default), with `--project` and
  `--edits` as for `run`. Texture warnings are as in `render`; the game is loaded before the
  window opens, and a failure exits with code 2. Not in WebAssembly builds, which stay
  headless.
- Breaking: `ludenea_cli::assets` moves to `ludenea_app::project` (`project_assets`,
  `project_textures`, `ProjectTextures`, `ProjectTexture`), with no alias, so the runtime
  and the project tools read a project's files without depending on the command line tool.
- Breaking: the `ludenea-debug-draw` crate is renamed `ludenea-draw`, and the facade's
  `ludenea::debug_draw` is now `ludenea::draw`, with no alias. Its types keep their names.
- `ludenea_draw::frame::RenderFrame` is what a renderer draws for one tick: the camera
  (its entity, centre and view), the render layers in draw order, sprites, tilemaps, the
  entities teleported this tick, the debug draw, and warnings. `Color` derives `Schema`, so
  components can hold it.
- `App::render_layer(name, parallax)` declares a render layer, drawn in declaration order;
  `build` refuses a name declared twice with the new `AppError::DuplicateLayer`, and a
  parallax that is NaN or infinite with the new `AppError::NonFiniteParallax`, naming the
  layer and the value, and more than 65,536 layers, past what a `u16` layer index holds,
  with the new `AppError::TooManyLayers`, naming the count and the limit.
  `App::extractor` registers an `ExtractFn` that adds a world's content to the frame.
- `Simulation::render_frame(debug_layers)` builds the frame of the current tick: its tick
  and layers, then every extractor in registration order, then the debug draw of
  `debug_layers`. A panic of an extractor comes back as `SimError::Panicked`, as a
  drawer's does, and building the frame never changes the state.
- `Pose` and `PoseError` move from `ludenea-physics` to `ludenea_app::pose`, so an entity
  without a body can be placed and drawn. `ludenea_physics::Pose` and the prelude still
  export them, with the same stable name `ludenea::Pose`, serde shape, validation and
  messages. Registering `Pose` from two plugins is a no-op the second time.
- The new `ludenea-render` crate (facade `ludenea::render`) holds the simulated render
  components, all in the state hash: `Sprite` (texture, frame, layer, order, flips, tint),
  `SpriteAnimation` (a clip of a `SpriteSheet` from a tick, looping or holding its last
  frame), `Camera2d` (centred on the entity's `Pose`, `view` the minimum visible area),
  `Tilemap` and `Teleported` (its tick is the render frame's, `Time.tick + 1` during a
  step, which `Teleported::this_step(&time)` writes, while `SpriteAnimation::started`
  takes `Time.tick`), plus the `SpriteSheet` data asset of named `Clip`s. The
  prelude exports `Sprite`, `SpriteAnimation`, `SpriteSheet`, `Clip`, `Camera2d`,
  `Tilemap`, `Teleported` and `RenderPlugin`.
  `RenderPlugin` registers them with `Pose`, the `animate` system (set `render::Animate`,
  after `physics::Step` when the app has it), which writes each animated sprite's texture
  and frame every tick (a system switching clips orders itself `.before(Animate)` to show
  the new clip in the same tick), and the extractor that
  fills the render frame: the camera of the lowest entity, sprites and tilemaps sorted by
  layer, order and entity, the entities teleported on the frame's tick (added to those
  earlier extractors listed, sorted by entity without duplicates), and a warning naming
  the entity for a second camera, a sprite, tilemap or camera without a `Pose`, an
  animation without a `Sprite`, an undeclared layer, or a clip the sheet lacks. Loading
  refuses a camera view or a tilemap cell size that is not finite and above 0, a tilemap
  whose tile count is not `width * height`, and a clip whose `count` or `ticks_per_frame`
  is 0. The crate depends on no GPU crate.
- `ludenea-render` also draws. `atlas::pack` decodes PNG textures, cuts each into frames by
  its `TextureMeta`, and packs the frames, extruded by 1 pixel, into reproducible
  `AtlasPage`s. It always returns an `Atlas`: a texture that does not decode, fit its
  grid, or fit a page, and every texture sharing its identifier with another
  (`TextureError::Duplicate`), is left out with a `TextureError` naming it and the
  numbers, kept in `Atlas::errors` sorted by identifier, and the others still pack.
  `TextureError::id` and `TextureError::path` name the refused texture.
  Behind feature `gpu`, the wgpu `Renderer` draws a frame's sprites, tilemaps, layers
  with parallax, camera and debug draw between two ticks, keeping the geometry on the GPU.
  `set_frames(previous, current, overlay)`, once per tick, uploads every sprite instance
  with both ticks' transforms, the tilemap chunks (kept while their tiles do not change)
  and the views' layout, reusing and growing its buffers, and returns a `DrawReport` naming
  what draws as a magenta placeholder (a missing texture or frame). `draw(target, size,
  alpha, camera)` draws to a target, and `render_rgba(size, alpha, camera)` offscreen,
  returning the RGBA pixels, `alpha` of the way between the ticks (clamped, NaN as 1)
  through `camera` (the editor's free camera) or the frame's: they write only the views,
  the shader blends positions and turns rotations along the shorter arc, teleported
  entities and a camera that changed entity are not blended, and the debug draw (the
  `Overlay`'s layers and selected entities) is rebuilt only when the target size or the
  current visible area changes. `set_atlas` drops the frames set before it.
  `Renderer::stats` returns `RenderStats`, the buffers created and the bytes written since
  the renderer was made. It splits atlases and batches below the device limits, and
  returns the device's errors and its loss as `RenderError` values (non-exhaustive, so
  later variants are not breaking): a GPU error pending before a call is named `"earlier
  GPU work"` and the call does not run, and an atlas page with a side of 0 or pixels that
  do not match its size is `RenderError::InvalidAtlas`, naming the page. `gpu::headless`
  opens a device without a window, on the fallback adapter when there is no hardware
  adapter or its device cannot be opened, and `gpu::headless_with_limits` does the same
  with adjusted device limits. Behind feature `web`, the same renderer draws into a
  browser canvas with WebGPU or WebGL2: `WebRenderer::push_frame(current, overlay)`
  returns the report as JSON, and `draw(alpha, camera)` draws.
- The command line tool's new `render` draws a tick of a scene (with its `--inputs` and
  `--edits`) or of a `.replay` offscreen and writes it to a PNG: `--tick`, `--out`,
  `--width` and `--height` (1280 by 720 by default), and `--debug-layer` for each debug
  draw layer to show. It prints the tick's state hash, which rendering never changes, and
  for a replay a warning when that hash differs from the one the replay recorded. The
  project's textures (each `.png` whose meta makes it a texture, found by the new
  `ludenea_app::project::project_textures`, which returns them with a warning for each
  texture whose meta cannot be read or is invalid) are packed into the atlas; a texture
  whose meta cannot be read or is invalid, or that does not pack, is a warning, is left out
  and draws as placeholders, and each sprite or tile drawn as a placeholder is a warning
  naming the entity, the texture and the frame. Two textures sharing an identifier, a tick
  past the end of a replay, an unknown debug layer, and a `--width` or `--height` past the
  device's `max_texture_dimension_2d` are refused with exit code 2; the textures and the
  GPU adapter are checked before the run. It needs a GPU adapter, hardware or software,
  and is not in WebAssembly builds.
- `Meta::texture(path)` is the one rule the editor listing and the command line share for
  whether a file is a texture: a PNG (its extension in any case, see the new
  `meta::is_png`) whose meta is a texture meta, or `Other` as metas written before
  textures had settings are, which reads as the default texture meta. A texture meta on
  any other file makes no texture.
- `ludenea_app::replay::play_tick` plays one tick of a replay `load_replay` loaded: its
  recorded edits, then its recorded input.
- `IntoSystemConfig::after_if_present(label)` orders a system after a set like `after`,
  but ignores a label with no systems instead of refusing it with `BuildError::EmptyLabel`,
  so a plugin can order itself after another plugin's set that the app may not add.
- `ludenea_app::scene::SCENE_VERSION` is public, like the other format versions.
- `ludenea_control::PROTOCOL_FINGERPRINT` and
  `ludenea_project::protocol::PROJECT_PROTOCOL_FINGERPRINT` hold the wire shape of each
  protocol version, and `ludenea_project::protocol::handshake` takes the client's
  fingerprint after its version.
- `Command::reply_type` names the reply each command gets when it succeeds, and
  `ProjectRequest::reply_types` the replies each project request may get.
- `Parallel::try_new` replaces `Parallel::new`, which panicked when the system refused the
  threads and ran 0 as 1: it returns a `ParallelError` for 0, for a count above the new
  `Parallel::MAX_THREADS` (256), and for threads the system refuses. `Parallel::check`
  refuses the first two before any thread starts.
- `Simulation::set_threads` returns `Result<(), ParallelError>` and keeps the previous
  pool on error. The command line tool's `--threads` takes 1 to 256, and its refusals
  exit with code 2 instead of a panic.
- `Rot::normalize` returns `Rot::IDENTITY` when the magnitude is zero or not finite, and
  `sin_cos` returns `(0, 1)` for an angle that is not finite or so large that its
  approximation overflows (1e30, for example), so `Rot::from_angle` and `Rot::integrate`
  give the identity there too. All of them returned `(0, 0)`, a rotation that loading a
  `Pose` refuses. A finite angle too large to reduce precisely, such as 1e12, still gives
  a unit rotation that does not follow the angle.
- The physics step clamps the angular velocity of a kinematic body as it does a dynamic
  one's, to an eighth of a turn per substep, and writes the clamped value back to `Body`.
- `Pose::validate` checks a pose as loading does and returns the new `PoseError`, which
  carries the refused position or rotation. The physics step panics with the entity, the
  field, and its value when it leaves a state that loading would refuse.
- `PhysicsSettings` refuses to load, from a scene, a snapshot, or a `set_field`, a gravity
  component that is not finite or whose magnitude is above the new `MAX_GRAVITY` (1e6),
  `substeps` outside 1 to the new `MAX_SUBSTEPS` (64), a contact damping ratio above the
  new `MAX_CONTACT_DAMPING_RATIO` (1000), and a contact hertz, damping ratio, push
  velocity, or restitution threshold that is not finite or below zero. NaN gravity or a
  huge damping ratio ended the game process at the next step, and 400,000,000 substeps
  hung it. The step brings substeps and a damping ratio that game code writes back within
  these limits, but the resource keeps the written value, so a snapshot of the world taken
  meanwhile, such as a session checkpoint, fails to restore.
- `Body` refuses a `gravity_scale` whose magnitude is above the new `MAX_GRAVITY_SCALE`
  (1000). `Collider` refuses a density above the new `MAX_DENSITY` (1e6), a friction or
  restitution above the new `MAX_FRICTION` and `MAX_RESTITUTION` (1000), and, unless its
  density is 0, a mass below the new `MIN_MASS` (1e-9 kg) or an infinite one. All of them
  overflowed the step.
- `Time` refuses to load a `dt` that is not finite, below 2^-32 second (the step of the
  fastest tick rate, `u32::MAX` Hz), or above 1 second (the step of the slowest). A scene
  with a NaN or subnormal `dt` ended the game process at the next physics step.
- The physics step limits the linear speed of a kinematic body to 400 m/s, as it does a
  dynamic one's, and writes the limited value back to `Body`. A velocity so large that its
  square overflows keeps its direction at 400 m/s; it dropped to zero.
- The physics step applies the speed limits before it prepares the contacts, so a body
  that starts a step faster than them bounces at the limited speed; the bounce impulse
  overflowed. A contact spring so soft that its stiffness underflows to zero (a contact
  hertz near the smallest float with a damping ratio of 0) no longer gives NaN at zero
  separation.
- The panic for an invalid state after the physics step also gives the step inputs: `dt`,
  gravity, and the body's gravity scale and velocities at the start of the step.
- `SessionError::Checkpoint` is returned when a checkpoint cannot be restored because a
  system wrote a value its type refuses to load; the session used to panic.
- A component that a snapshot cannot restore is named with its entity, and
  `ludenea_ecs::decode_exact` keeps the message of a type that refuses its value, where
  postcard gave "Serde Deserialization Error".
- `Session::resume` drops an undo history that does not decode or has a later version,
  instead of failing with `SessionError::Undo`, and the entries that name a branch the
  session does not have, name the root as the branch a fork made, or restore branches
  that do not fit the tree, such as one forked past its parent's run, and the fork entries
  whose tick is not their branch's fork tick, and the redo of an edit recorded before its
  branch forks. `Session::warnings`
  names each. Redo puts branches back only where they fork on their parent's run, and a
  fork never takes the identifier of a branch redo put back. A redo entry of a version 2
  history redoes within its branch only where its tick is not before the branch's fork and
  cutting the run after it leaves every child forking on it; otherwise it is dropped.
- `Session::redo` of a recorded edit puts back only its branch's own part of the run,
  after the run it forks from as it is now. After a tick 0 edit on a forked branch, it
  rebuilt the ticks before the fork from the other branches' stale hashes. A branch part
  recorded by a previous build resimulates from the branch's fork tick, not from tick 0,
  and a game panic while replaying up to that tick refuses the redo and changes nothing.
- `Session::resume` refuses a branch tree that holds the branch identifier `u32::MAX` with
  `SessionError::BranchesDecode` and the new `BranchesError::Exhausted`; it panicked. Once
  a session has handed out its last branch identifier, an edit or take-over that would
  fork is refused with the new `SessionError::CannotFork` and changes nothing; it
  panicked, or, built without overflow checks, reused an identifier. `BranchTree::fork`
  returns `Result<(), BranchesError>`, `BranchesError::Exhausted` once the identifiers run
  out.
- A type that contains itself, such as a tree node holding a `Vec` of nodes, has no schema
  yet. `#[derive(Schema)]`, and so the `Component`, `Resource`, `Asset` and `Event`
  derives, refuses a field that names the type itself or `Self` inside `Option`, `Vec`,
  `BTreeSet`, `BTreeMap`, an array or a tuple. Where a derived `schema()` comes back to a
  type it is describing through other types, it gives the new
  `TypeSchema::Recursive { name }` marker instead of overflowing the stack, and
  `TypeSchema::recursive_type` finds it. The `World` registrations, `App::input` (through
  `App::build`) and the new `ludenea_ecs::check_not_recursive` refuse a type holding one
  with the new `EcsError::RecursiveType`, which names both types and says to store
  children as `Entity` references. Listing the schemas of such a type ended the game
  process.
- `Asset` has a provided method, `validate(&self, world: &World) -> Result<(), String>`,
  that accepts every value by default. `World::insert_asset` and
  `World::insert_asset_ron` refuse a value it refuses with `EcsError::InvalidAsset`, and
  `World::restore` with `SnapshotError::Decode` naming the asset, both changing nothing.
  To give it, derive `Schema` and implement `Asset` by hand.
- `Prefab` implements `validate`: a prefab whose entity address is listed twice, or whose
  component is not registered or does not decode exactly, is refused with the entity and
  the component, such as ``entity [0]: component `other_game::Gone` is not registered``.
  Such a prefab loaded, and the next `spawn_prefab` ended the game process mid-tick. A
  replay whose prefab holds a component that changed since it was recorded is refused
  the same way, with `ReplayError::Decode` naming the asset.
- Removed: the field `Prefab::entity_components`. Each component type records whether its
  schema holds an `Entity` when it is registered, and `spawn_prefab` remaps the values of
  those types, so a prefab can no longer leave a reference unmapped by omitting its type.
  The new `Prefab::upgrade_version_2` turns the bytes of a prefab in the earlier layout
  into the current one.
- `TypeSchema::mentions` tells whether a schema or any type inside it satisfies a
  predicate, such as holding an `Entity`.
- `Codec::remap_entities` returns `Result`: bytes that do not decode, or a type that
  refuses a mapped entity as a `try_from` can, give an error instead of a panic.
  `spawn_prefab` still panics when a component's type refuses the entities spawned for
  it, and now names the prefab and the component.
- `Simulation::save_scene` and `Simulation::save_scene_keeping` return
  `Result<String, SceneError>`. When the new entities and stale references need more
  local identifiers than remain from `next_id`, they are refused with the new
  `SceneError::IdentifiersExhausted { next_id }`, which says to renumber the scene; they
  panicked. A written `next_id` is always below `u32::MAX`, so the scene loads back.
- `prefab::place_instance` refuses a `next_id` that leaves no identifier for the instance
  with `SceneError::IdentifiersExhausted`; it panicked, or, built without overflow checks,
  gave the identifier 0 again.
- `Simulation::override_value_ron` refuses a component the app does not register with
  `SceneError::UnknownComponent`; it panicked.
- `Host::new` returns the new `AppError::Scene` when the built world cannot be written as
  a scene, or when that scene does not load back; it panicked on the latter.
- `Simulation::save_scene`, `Simulation::save_scene_keeping`, `prefab::create_prefab`,
  and `Simulation::override_value_ron` refuse a value that does not decode, as when the
  game's own `try_from` refuses a value its systems or its build wrote, with
  `SceneError::InvalidValue` naming it; they panicked.
- The new `Session::create_prefab` writes the prefab file of scene entities as the scene
  holds them, whatever tick the session shows. It refuses an entity spawned after tick 0
  with `SessionError::NotInScene`, whose message now fits every command that refuses such
  an entity, one that does not exist with `EditError::NoSuchEntity`, and any while the
  session resimulates with `SessionError::Resimulating`. The new `Session::apply_to_prefab`
  moves an override into the session's text of the instance's prefab and out of the scene,
  as one tick 0 edit that one undo takes back;
  `ludenea_app::prefab::move_overrides_to_prefab` is its text operation.
- `Document::Prefab { id }` names the prefab's asset. Every load of a prefab session reads
  the document's text for `id`, through the now public `ludenea_app::prefab::Overlay`, so
  an edit that puts the prefab inside itself, directly or through another prefab, fails
  with `SceneError::PrefabCycle` and changes nothing, and so does starting or resuming a
  session on such a file. It was accepted, and the saved prefab then failed to load in
  every scene using it. The serde helper `protocol::outcome::Wire::Ok` holds a
  `Box<Reply>`, since a reply carrying the document outgrew an unboxed variant.
- `Session::mark_scene_saved` takes the text written to the scene's file, records it as the
  saved text, and marks the scene saved only while it still is that text; the new
  `Session::mark_asset_saved` does the same for an asset. The session tracks which prefab
  and data assets differ from their file, listed in the new `Timeline::unsaved_assets` and
  carried by the new `SessionExport::saved_assets`, and `Session::replace_scene` keeps the
  text of an unsaved asset over the new one.
- `Project::files` reports files whose metas give them one identifier, as a folder copied
  with its metas has, with the new `ProjectWarning::DuplicateId`: once, then again when
  the files that share it change.
- `Project::set_aside_stored_session` moves the stored session of a document to the first
  free `.ludenea/sessions/<id>.<n>.rejected.session`, from 1, and returns that path, or
  `None` when none is stored.
- `Edit::Group`, the last variant, holds several entity and resource edits made as one.
  `Simulation::apply_edit` checks each against the world before the group, as if the
  entities that earlier members despawn were dead already, then applies them all, so a
  failing one changes nothing and none can target an entity another one spawns. A group
  without edits is refused with the new `EditError::EmptyGroup`, and one holding an asset
  edit or another group with the new `EditError::NotGroupable`, which the new
  `Edit::check_groupable` gives for one edit. `Simulation::assets_introduced_by` and the
  checks of a replay cover the edits of a group.
- The new `Session::edit_scene` applies `SceneChange`s (an edit, or the override of a field
  of a prefab instance's entity) to the scene in turn as one tick 0 edit: the recorded
  future resimulates once, one undo takes them all back, and when one fails nothing
  changes. Like a group, it refuses an asset edit or a group among them with
  `EditError::NotGroupable`. `Session::edit` of an `Edit::Group` at tick 0 is such a scene
  edit of its edits; past tick 0 it records the group as one edit. `Session::edit` refuses
  a `LoadAsset` edit at tick 0, where the assets come with the scene, with the new
  `SessionError::LoadAtTickZero`; it changed nothing and returned no error.
- The `Simulation` methods that run game code catch its panics in native builds and return
  them as the new `SimError::Panicked` or `EditError::Panicked`: `step`, which now returns
  `Result<(), SimError>`, `step_with`, `step_encoded`, `step_observed`, `map_raw_input`,
  `debug_draw` and `pick`, which now return `Result`, `apply_edit` while it decodes the
  values (the world is then untouched; a value's `Drop` that panics while the edit
  changes the world unwinds to the caller), `encode_json_value`, and the new
  `encode_asset_json` and `default_asset_ron` (whose refusals are the new
  `SimError::AssetDefault`). A panic is the new `GamePanic`: the tick, the system that
  was running or whose commands were applying (the new `Schedule::running`), the message,
  and the source location as `file:line:column`, displayed as
  ``the game panicked at tick 3 in system `game::spawn`: ... (src/lib.rs:88:14)``. The
  message names an asset by the file the scene load's `AssetSource` gave for it, so a
  prefab spawn that fails names `assets/ball.prefab.ron`, not its identifier. The new
  `ludenea_app::catch_panic` is the one way game code is caught; its first call installs
  a panic hook that records the location, then runs the hook that was there before.
  WebAssembly builds still abort on a panic.
- The new `World::decode_asset` decodes and validates an asset without storing it, and
  `World::insert_decoded_asset` stores it: the type's own code runs before the world
  changes. `World::insert_asset` does both.
- The new `Session::recover` brings a session back after game code panicked anywhere in
  it: it ends a resimulation the panic interrupted, as a tick that fails ends it, then
  reloads to the current tick. It returns `Result<Option<SessionError>, SessionError>`:
  `Ok(Some(why))` when it came back to a consistent tick other than the current one, such
  as tick 0, and an error only when the world and the session no longer agree.
- `record_replay`, `record_loaded_replay`, `play_replay` and `load_replay` return the new
  `ReplayError::Panicked { panic, snapshot }` when the game panics, with the snapshot of
  the state a panicking tick ran from, where the panic used to unwind through them.
  `SessionError::Panicked` holds a `GamePanic`, and converting a `SimError` or
  `ReplayError` panic gives it; an `EditError::Panicked` stays an edit failure.
- The command line tool's `run` and `verify` report a game panic as
  ``error: the game panicked at tick 3 in system `game::boom`: ... (src/lib.rs:6:22)``
  and exit with code 2, where Rust's panic text and code 101 named neither the tick nor
  the system; `verify` also writes the state the panicking tick ran from to
  `tick-3.snap` in `--dump-dir`.
- Removed: `BranchesError::Tree`, which named no branch. A branch tree or a redo whose
  branches do not link up is refused with the new `BranchesError::MissingParent`,
  `YoungerParent`, `NoParent`, `Taken`, `NoRoot`, `NoCurrent`, or `NoSuchBranch`, each
  naming the branch, such as "branch 3 forks from branch 9, which the tree does not have".
- An undo or redo entry that cannot work on the session's branches, as only a crafted
  history holds, fails with the new `SessionError::Unfit`, which names the entry and why,
  as the warnings of a resume do, and drops it. A redo whose branches no longer fit failed
  with `SessionError::BranchesDecode`, "the branch tree cannot be decoded", though nothing
  was decoded, and an undo of a fork entry naming the root with
  `SessionError::NoSuchBranch`, "there is no branch 0".
- Removed: `SessionError::Undo`, which nothing returns since `Session::resume` drops an
  undo history that does not decode with a warning. `UndoError` stays, in those warnings.
- New: `ludenea_control::protocol::unreadable_hello`, which reads the protocol version of a
  hello of any shape, so a text transport refuses a hello from an older protocol version
  as a version mismatch. The project service uses it instead of its own copy.

### Control protocol

- Control protocol 5, fingerprint `cd1e75810002a446`. Breaking: `get_render_frame`
  (`debug_layers`) replaces `get_debug_draw` (`layers`), and its reply `render_frame`
  (`frame`) replaces `debug_draw` (`tick`, `frame`). The frame is the `RenderFrame` of the
  current tick: its tick, camera, render layers, sprites, tilemaps, the entities teleported
  this tick, and warnings, with the debug drawing of `debug_layers` in `frame.debug`.
  `get_ghost_draw` is unchanged.
  `set_live_input` takes `devices` (the keys held and a slot per gamepad, `null` for an
  empty one) in place of `players`, and the host splits them between the players by their
  claims. New `set_players` (`players`, `claims`) replaces the recorded inputs with an empty
  log of that many players and sets who holds which keyboard part and gamepad (the automatic
  claims when `claims` is `null`) and replies `players` with the claims it applied and
  `devices`, each claim's devices in words as every runtime shows them ("left keyboard half
  and gamepad 1", "no device"); it is refused with `invalid_inputs` for no players, a
  number of claims other than the players, a gamepad claimed twice, and overlapping parts
  of the keyboard. `load_inputs`,
  `load_session`, and `load_replay` reset the claims to the automatic ones. A player whose
  gamepad is disconnected gets the game's default input.
  New topic `render_frames`: while it is subscribed, each advance that plays ticks pushes
  `render_frames` (`frames`, `progress`) after its `ticked` events, with the frames of the
  last two ticks it played (one when it played one), oldest first, and `progress`, the
  fraction of the next tick that play owes. New `set_pushed_layers` (`debug_layers`) sets
  the debug layers those frames draw, none until set. A frame that fails to extract pushes
  nothing for that advance and writes `render frame of tick N: <cause>` to the console, once
  per cause (at its first tick) until a scene or a session loads; play goes on. The browser
  worker posts these frames postcard-encoded in transferred buffers; in JSON (`run --stdio`,
  which the desktop editor relays) each of `frames` is that encoding as a standard base64
  string, under a quarter of the frame's JSON at 10,000 sprites. The control client hands
  them to listeners as `Uint8Array`s whatever the transport.
  In the control client, `ControlClient.on` is typed on the new `ClientEvent`: the
  protocol's events with `render_frames` as the new `RenderFramesEvent`, whose `frames` are
  `Uint8Array`s, each a frame's postcard encoding.
  `MessageTransport` gains an optional `onFrames` for the frames a transport carries as
  bytes (the new `PushedFrames`), which `startWorkerGame` and `postMessageTransport` provide.
  New in the Rust API: `Host::tick_progress`, the fraction of the next tick that play owes,
  and `protocol::encode_frame`, a render frame's postcard encoding.
  A debug draw `Primitive`, and so a `RenderFrame`, now decodes from postcard: the formats
  that are not human readable tag it by variant index, JSON still with its `type` field.
  In those formats a sprite's `texture` and a tilemap's `tileset` are the identifier's 16
  little-endian bytes instead of their 33 bytes of text, saving about a quarter of a sprite's
  postcard encoding; JSON keeps their hexadecimal text.
  New `paint_tiles` (`entity`, `area`, `tile`) sets every cell of `area`, the new `TileArea`
  (`x`, `y`, `width`, `height`, in cells from the bottom-left cell), of the entity's
  `ludenea::Tilemap` to `tile` and replies `done`. `tile` is required: `null` erases, and a
  command without it does not decode. It applies and records exactly as the `set_field` of
  the tilemap's `tiles` with the same result (a scene edit at tick 0, an override on a
  prefab instance's entity, a recorded edit later, one undo step), batches as one, and a
  batch changing the same tilemap with both is refused. Every refusal starts with the
  entity, as in ``entity 4v1, component `ludenea::Tilemap`, paint_tiles:``, errors of the
  edit that follows included. A dead entity is refused with `no_such_entity`. With
  `invalid_command`: an entity without a tilemap (`entity 4v1 has no ludenea::Tilemap; is
  RenderPlugin added?`), a tilemap the game left without one `tiles` entry per cell (`the
  tilemap cannot be read:` and the cause), an empty area (its size), and an area outside
  the grid (`area x 30..34, y 0..2 is outside the 32 by 18 grid`; an area whose end
  overflows names its start, its size, and the grid). A tile past the tileset's frames is
  accepted.
- Over `host --stdio` and in a browser worker, the events a request causes come before its
  response, not after the response to a later request: a `load_session` sent right after
  `load_scene` answered could be followed by the scene's `timeline_changed` (head 0), which
  a client took for the session's.
- `hello` and `welcome` carry `protocol_fingerprint`, a hash of the TypeScript declarations
  of every type the messages reach. The host refuses a client of another version or
  fingerprint with `version_mismatch` and says to rebuild the editor
  (`pnpm -C editor build`) and the game from the same checkout; a `hello` of an earlier
  version, without the fingerprint, is refused the same way. A `hello` of this version
  that does not decode, such as one without `protocol_fingerprint`, is refused with
  `invalid_command`, its decoding error, and the fields a `hello` needs, instead of
  "send Hello before any request".
- `set_threads` refuses a count outside 1 to 256, or one the system cannot start, with
  `invalid_command`, and the previous count stays; it ended the game process before.
  `host --stdio` starts on every core up to 256, and on one thread with a console warning
  when the system refuses them.
- A command that has to restore a checkpoint the game's own types refuse, such as `seek`,
  fails with `invalid_snapshot` and names the tick, the entity, and why its type refuses
  the value; it ended the game process before. A seek, undo, or redo that fails this way
  changes nothing, but an undo or redo that shows another branch stays on that branch,
  where it switched first, and a branch switch stays on the current branch, or on the
  target at its first stale tick. When the session cannot come back to its tick after a game panic, a
  snapshot restore, a replay export, resimulation, or a failed apply to the scene, it goes
  to a tick it can show, tick 0 at worst. While the current state itself holds such a
  value, resimulation and an edit whose asset loads are taken back replay the current tick
  instead of ending the game process.
- `load_session` loads a session whose `undo` history does not decode or has a later
  version without that history, and drops the history entries that do not fit the branch
  tree; `loaded` lists each in its `warnings`. Undoing a fork entry that named the root
  ended the game process, redoing a branch forked past its parent's run exhausted its
  memory, and a fork after redoing one in a reloaded session could reuse a branch
  identifier, making a branch its own parent and hanging the game. A redo entry of a
  version 2 history redoes again where it cuts no branch off; it failed to decode. A fork
  entry whose tick is not its branch's fork tick is dropped too, since undoing or redoing
  it would replay up to that tick however far, and so is a redo of an edit recorded before
  its branch forks.
- `redo` of an edit on a branch keeps the recorded hashes of the ticks before the branch's
  fork true. After a tick 0 edit on a forked branch they were stale, so a later
  `load_session` on the same build reported a divergence that did not exist.
- `load_session` refuses a branch tree that holds the branch identifier 4294967295 with
  `invalid_command`; it ended the game process. An edit or take-over that would fork a
  branch after the session has handed out its last identifier fails with
  `invalid_command` and changes nothing.
- `TypeSchema` has a `recursive` kind, `{ "kind": "recursive", "name": ... }`, marking
  where a type comes back to the type `name` it is inside of. No game sends one: a game
  holding such a type fails to build with an error that names it, where it ended with a
  stack overflow.
- `restore` refuses a snapshot holding a prefab the game cannot spawn with
  `invalid_snapshot`, naming the asset, the entity and the component; the game process
  ended at the next spawn of that prefab.
- A tick 0 edit, such as `spawn`, or a `place_instance` that needs more local identifiers
  than the scene's `next_id` leaves fails with `invalid_edit` and changes nothing. With
  stale references in the scene it ended the game process; otherwise it failed with
  `invalid_scene`.
- `create_prefab` writes the entities as the scene holds them, whatever the current tick.
  Past tick 0 it wrote their simulated values, which `replace_with_instance` then put into
  the scene. It refuses an entity spawned after tick 0 with `no_such_entity`, naming it and
  saying it is not in the scene, as `replace_with_instance` now does too (it did not name
  the entity), and refuses while the session resimulates with `resimulating`.
- `apply_to_prefab` is a tick 0 edit: the instance's prefab takes the override's value as
  the scene writes it, in the session's asset text, which `save_asset` gives to write, and
  the override is removed; one `undo` ("Undo apply to prefab") takes both back, and an
  instance without that override is refused with `invalid_scene`. It still replies the new
  prefab text. It used to take the instance's value at the current tick, simulated past
  tick 0, and to change neither the scene nor the session's prefab text.
- The `document` of `load_scene`, `load_session` and the `session` reply names a prefab
  document's asset: `{ "Prefab": { "id": "<asset id>" } }` replaces `"Prefab"`. Its
  instances resolve against the document's text, so a `place_instance` or another tick 0
  edit that puts the prefab inside itself, directly or through another prefab, fails with
  `invalid_scene` and changes nothing; it was accepted, and the saved prefab then failed
  to load in every scene using it. `save_scene` checks a prefab document as a prefab file
  before it gives its text: an instance cycle or an entity reference outside the prefab
  fails with `invalid_scene`.
- Saving is two-phase. `save_scene` and `save_asset` give the text to write and leave the
  scene or asset unsaved; the new `mark_scene_saved { text }` and
  `mark_asset_saved { id, text }` (reply `done`) record the text the client has written as
  the file's, and mark the scene or asset saved only while the session still holds that
  text, so an edit made during the write stays unsaved until undone. `save_scene` marked
  the scene saved before the editor wrote the file, so a failed write lost the edits when
  the document opened again.
- The host owns which prefab and data assets differ from their file: `timeline` lists them
  in the new `unsaved_assets`, a tick 0 asset edit, "Apply to asset" and `apply_to_prefab`
  add one, undo and redo back to the saved text remove it, and the `session` reply and
  `load_session` carry the saved text of each in the new `saved_assets`, so they stay
  unsaved after a resume. `replace_scene` keeps the session's text of an unsaved asset over
  the new file text, unsaved while they differ.
- The new `batch { commands }` (reply `done`) applies edit commands (`spawn`, `despawn`,
  `insert_component`, `remove_component`, and `set_field`) as one edit, all or none, each
  read against the state before the batch. At tick 0 it is one scene edit: one undo step,
  and the recorded future resimulates once. Later it records one edit at the current tick,
  a group of their edits, which one undo takes back; a batch of one command records that
  command's edit. Any other command, a nested batch, or a component or resource changed
  twice is refused with `invalid_command`, and nothing applies. Sent one by one after
  playing, such edits at tick 0 applied the first only, the next ones refused with
  `resimulating`, and each was an undo step of its own.
- A game panic no longer ends the game process. An input mapping that panics fails
  `set_live_input`, and a type whose own code panics on a value fails `spawn`,
  `insert_component`, `set_field`, `set_asset_field` or `new_asset_value`, with
  `system_panicked`, changing nothing. Any other panic while the host handles a command
  or advances, such as a type that panics while a scene loads or while it is serialized,
  replies `system_panicked`, is written to the console, pauses with `panicked`, and
  reloads the session to its current tick; a resimulation it interrupted ends there, as
  when a tick panics, instead of panicking again on every advance. A panic in the compare
  or divergence simulation, as while it restores a checkpoint, stops only the ghost or
  the report, as a panic in one of their ticks does. A report that failed tries again
  only once the runs it compares change (a command that changes the state, or a
  resimulation), or on `get_divergence` or a new subscription, instead of rebuilding its
  simulations and logging the failure on every advance; playing does not retry it. When the
  session cannot come back,
  commands that change the state and `play` are refused with that error, and time stops,
  until `load_scene`, `load_session` or `load_replay` starts a new session, or a later
  panic brings it back. A session that comes back to tick 0 instead of its tick is consistent
  there: the message says "the session is back at tick 0" and why. A recorded edit whose
  type now panics on its value is dropped while resimulating and reported with
  `edit_dropped`, as an edit that no longer applies is.
- The `system_panicked` message and the console name the system and the source location,
  ``the game panicked at tick 3 in system `game::spawn`: ... (src/lib.rs:88:14)``, and
  the `panicked` pause reason gives the message as ``... (in system `game::spawn`,
  src/lib.rs:88:14)``. `pick` and `get_render_frame` give that form too, instead of "the
  game panicked while picking" and "while drawing debug layers". A browser game's crash
  message gives the location as `(file:line:column)` like a native one, not
  `at file:line`.

### Project protocol

- Project protocol 4, fingerprint `96f47cb59cf37239`: `AssetKind` gains `Texture(TextureMeta)`,
  so a listing tells a PNG and how it is cut into frames.
- `hello` and `welcome` carry `protocol_fingerprint`, as in the control protocol, and the
  service refuses a client of another version or fingerprint the same way, and a `hello`
  that does not decode with `invalid_request`, its decoding error, and the fields a
  `hello` needs.
- `game_output` events and the `line` events of a build carry a line that is not UTF-8
  with replacement characters. Such a line used to end the relay of its stream: on the
  game's standard output, every later protocol message was lost.
- `TypeSchema` in `GameInfo` (`write_game_types`) has the `recursive` kind of the control
  protocol; the game types declare it as the alias of the type it names, or `unknown`
  when that type is not among them.
- `set_aside_stored_session { document }` moves the stored session of a document, which the
  editor could not restore, where no later `store_session` overwrites it, and answers
  `set_aside` with its new path, or a null path when none is stored.
- A `warning` event names the files that share an identifier, usually a file or folder
  copied with its metas, and says to delete the `.meta` of each copy.
- `ludenea serve` takes messages up to 256 MiB, in one frame or several. It took frames up
  to 16 MiB and messages up to 64 MiB, and closed the connection without a word on a larger
  one. A larger message now gets `refused` (`invalid_request`) naming its size and the
  limit, then a close with code 1009, and the tool logs it.

### Plugin API

- `@ludenea/control-client` takes `PROTOCOL_VERSION` and `PROJECT_PROTOCOL_VERSION` from
  the bindings generated from Rust, with the new `PROTOCOL_FINGERPRINT`,
  `PROJECT_PROTOCOL_FINGERPRINT`, `ReplyTypes` and `ProjectReplyTypes`; `ReplyFor` and
  `ProjectReplyFor` are built from the last two. `handshake` takes a hello with both the
  version and the fingerprint, and refuses a welcome of another version or fingerprint
  with `version_mismatch`, so `ControlClient.connect` and `ProjectClient.connect` do too.
- `Editor.console` is the editor's log, which the Console panel shows: `log(level, text,
  location?)`, `report(error, context)` (logs `<context> failed: <message>`), and the
  entries through `list()` and `subscribe()`. Entries logged before the Console panel
  activates wait for it. `runReported(editor, id)` runs a command and reports its failure
  there under the command's title.
- A command that fails from the command palette, a shortcut, or a key in the viewport is
  reported in the editor console (the palette also shows it in an alert for a few
  seconds), and so are the failures of viewport gestures, plugin overlays (once per
  registration), divergence seeks, timeline refreshes, entity watches, settings, and
  plugin reloads; none of them reach only the browser's developer tools any more. Work
  repeated every frame or tick, such as fetching the debug draw or the ghost, is reported
  once until it works again, and the scene tree reports a label it cannot read, but not one
  of an entity gone before the read, as short-lived entities are while playing.
- Removed: `logToConsole` from `@ludenea/editor-builtins` and the `log` option of
  `createEditor` with its `DocumentLog` type; use `editor.console`.
- "Apply to prefab" (`Prefabs.apply`) no longer writes the prefab file. The game marks the
  prefab unsaved, the scene tree shows each group of an unsaved prefab with a "Save
  prefab" button, and undo clears the mark. The `inspector.save-asset` command saves the
  asset whose identifier it is given, a prefab included, and the selected data asset
  without one.
- "Save scene" (`scene.save`) and `inspector.save-asset` mark the scene or asset saved only
  after the file is written. A failed scene write fails the command with
  `Cannot save <path>: <reason>`, which the palette and Mod+S show in the editor console,
  and the scene stays unsaved. The asset browser knows the text the editor wrote, so the
  watcher reporting a saved asset before the game says it is saved shows no "changed on
  disk" banner.
- Removed: `DocumentState.unsavedAssets` and `SavedSession.unsavedAssets`. The game's
  timeline lists the unsaved assets (`unsaved_assets`), and a saved session carries them in
  `load.saved_assets`, so `DocumentSessions.restore` no longer marks them unsaved.
  `DocumentState.reloadAssets` takes the unsaved assets to discard, which take their file's
  text.
- `@ludenea/editor-shell/app` exports `STORED_SESSION_VERSION`. A stored session that the
  game refuses, and one that loads into a new game but cannot go back to its tick, are
  reported as warnings; the latter used to fail as `Cannot load <path> into the game`.
- `DocumentSessions.restore` is how an application restores what `DocumentSessions.own`
  gives: a stored session the game refuses is set aside with that warning, before any later
  store of its document, and the restore resolves to null instead of failing. Other sessions
  restore as with `restoreSession`.
- The asset browser shows a texture as its own kind, `Texture`, with its own icon and
  entry in the kind filter, instead of as a data asset the inspector would take.
- The asset browser keeps the open document open when its file is moved or renamed outside
  the editor: `DocumentState.current` follows the file's identifier to its new path, or,
  when the meta stayed behind, the file created with the document's text, under that file's
  identifier. It stays with its own file while that file keeps the identifier, and follows
  none of several files that have it, as copies made with their metas do, so a save never
  goes to a copy. A move the watcher reports in several batches, as a branch checkout does,
  is followed too, and a moved file whose text changed on the way is reloaded, or shows as
  changed on disk when the editor holds unsaved edits of it.
  When the file is deleted outside the editor, the document's session is stored and the
  document stays open, with a banner offering "Save again" (`scene.save`) and "Close" (the
  new `Assets.close`, which stores the session again first); it used to close and drop the
  session. "Save scene" without an open document no longer advises opening one.
- A `refused` from the project service after the handshake (a message over its size limit)
  fails the requests `ProjectClient` is waiting on with the service's message, instead of
  `connection closed: ...` once the connection closes.
- A plugin panel, custom inspector, field decoration, or explorer cell that throws while
  rendering no longer unmounts the whole editor. The panel shows `The "<title>" panel of
  plugin "<name>" failed: <message>` with a "Reload panel" button, a custom inspector falls
  back to the generic fields, and a decoration or cell shows a mark until what it shows
  changes; the editor console gets the failure once per registration, and a new
  registration (the plugin rebuilt) renders it again. An explorer column whose `subscribe`
  or unsubscribe throws is reported once and left unsubscribed, instead of failing the
  Explorer. The new `PluginBoundary` gives the same isolation to a plugin that renders other
  plugins' registrations (its `resetKeys` render a failed slot again once what it shows
  changes), and the new `Editor.pluginOf` names the plugin that registered a value.
  `mountShell` reports the other render errors in the editor console, and one that no
  boundary catches replaces the window with its message.
- `DocumentState.nameAssets(text)` follows each project asset that `text` names by
  identifier, as the game does, with the asset's file. The Console panel shows every entry
  that way, so a game panic on a prefab names `assets/ball.prefab.ron` too.
- Despawning the selection (`scene.despawn-selection`), nudging it, dragging it in the
  viewport, and editing it in the inspector change every selected entity in one `batch`:
  one undo step, all of them or none. A viewport drag moves every selected entity by the
  same offset and outlines each one; it moved the entity under the pointer only. With
  several entities selected, the inspector shows "N entities selected" and the components
  they all have, marks "mixed" the fields whose values differ (it shows the first entity's),
  and sets a field, adds a component to the entities lacking it, or removes one from all of
  them. Custom inspectors, field decorations, and "Apply to scene" stay with one entity
  selected: `inspector.apply-to-scene` refuses several.
- `handshake` takes the `ProtocolPeer` it is for after the hello, and its `version_mismatch`
  names that protocol and peer, as the game and the service do: `ControlClient.connect`
  says "the game speaks protocol ..." and `ProjectClient.connect` "the service speaks
  project protocol ...", where both said "the engine speaks protocol ...".
- `DocumentSessions.store` no longer stores a stored session the game refused through
  `restore`. Closing the page stored the seed of a session guard whose game refused it back
  over the document's session, and the next open set it aside again as another
  `.rejected.session`.
- `DocumentSessions.restore` treats an export of the open document that `own` gives back
  like a stored session: when the game refuses it (a game rebuilt without one of its
  components), the document's stored session is set aside with a warning, and the restore
  resolves to null. It failed with the game's refusal, and the next store overwrote the
  stored session with the unsaved edits it held. A restore the game never answers (its
  connection closed) still fails, and sets nothing aside.
- `Assets.reload` and `Assets.keepMine` take a `ChangedBanner`, and `Assets.close` a
  `RemovedBanner`, the two kinds of `Banner`: they took any banner, so `close` could close a
  document whose file was only changed on disk.
- The mark of a field decoration that failed is a button that renders the decoration again,
  for one that fails until state it reads loads while its field stays the same (a paused
  game); it stayed until the entity or the component changed.
- `Editor.inspectors.registerAsset(name, inspector)` adds an `AssetInspector` for the data
  assets of the type `name`, shown below their generic fields with the asset's value and an
  `onChange` editing a field, or for textures as `"Texture"`, shown alone without a game;
  `getAsset(name)` finds it, and `pluginOf` names its plugin.
- `Editor.textures` (`TextureAccess`): `list` and `subscribe` follow the project's textures
  whose meta can be read (`TextureInfo`, one object per texture until its file or meta
  changes), `find(id)` looks one up (or says the listing failed, listed again on the next
  `subscribe` or `frames`), `status()` tells how the latest listing went (still listing,
  failed, or the paths of the textures it left out for an unreadable meta), `bytes(path)`
  reads a texture's PNG, and `frames(path)`
  gives the `TextureFrames` its meta cuts it into with the PNG read for them (`TextureCut`),
  rejected with the renderer's reason. `createEditor` takes the cutting function as
  `textureFrames` (the applications pass the renderer's, exported by
  `@ludenea/editor-builtins/app`); without it, `frames` rejects saying so. `textureOf(entry)`
  tells what a listed file is as a texture. `@ludenea/control-client` exports
  `fromBase64(text)`, which decodes the base64 the project and text transports carry with the
  runtime's native decoder when it has one; `bytes` reads the PNGs with it.
- Breaking: `SelectionState.asset` is `{ id, kind }` (an `AssetKindName`) instead of the
  identifier alone, and `selectAsset(id, kind)` takes the kind: the inspector shows a
  selected texture as well as a data asset. Opening a texture in the asset browser shows its
  preview in the inspector, and the built-in sprite sheet inspector plays its clips.
- `Editor.edit(command)` sends an `EditCommand` (a command that edits the session as one
  undo step) to the running game, refreshes the timeline and hash, and resolves with the
  game's reply (`ReplyFor<C>`: a `spawn` gives the entity it made); it rejects without a
  game and while the game resimulates. `Editor.batched(commands)` sends several as one
  `batch` (one alone as itself, with its own reply; none not at all, resolving as an empty
  batch does); it takes only the `BatchableCommand`s a batch holds (spawn, despawn, insert
  or remove a component, set a field, paint tiles). `Editor.onEdited(listener)` hears every
  edit the game accepted. The built-in panels edit through them, and so do
  `Editor.entities.set` and `Editor.assets.setField`: their edits refresh the timeline,
  reach the `onEdited` listeners, and are refused while the game resimulates.
- `Editor.viewport.tools.register(tool)` adds a `ViewportTool`: while it is the active
  tool (`viewport.setTool(id)`, read from the `viewport.activeTool` store, `null` for the
  built-in select and move), the viewport gives it the pointer's presses, moves, and
  releases (`ToolPointer`, in CSS pixels and world units), or `pointerCancel` when a press
  ends without a release (the browser took the pointer, the tool stopped being active in
  the middle of it, or the viewport closed). Its `ToolContext` maps screen points onto a
  layer of any parallax and back (`screenToWorld`, `worldToScreen`) as the renderer draws
  it. The viewport shows the tool's `cursor` and draws its `overlay`; the right button is
  the tool's too, without the context menu. Its `available(editor)` is asked again on every
  selection, timeline, and game change, and a tool that answers false, throws, or is
  unregistered is deactivated. Escape returns to select and move; panning and zooming stay
  the viewport's.
- `OverlayContext.worldToScreen(point, parallax?)` takes the parallax of the layer the point
  is on (1 when omitted) and maps it as the viewport's renderer draws it.
- `Editor.entities.watch(entity, listener, despawned?)` takes a third listener, called when
  the watched entity is no longer alive at the tick shown (an undo of its spawn, a seek
  before it, another branch); alive again, the entity reports all its components as
  changed. Watches without it hear nothing of the despawn, as before.

### Formats

- Network protocol 1: the packets the peers of a match exchange,
  `ludenea_net::NET_PROTOCOL_VERSION` as a little-endian `u16`, then the postcard encoding
  of a `Packet` (`match_id` and `messages`). The two-byte prefix is frozen for every future
  version, so `Packet::version_of` names the version of a packet from any build. A `Message`
  is a handshake (`Hello` with the `MatchIdentity`, `Welcome`), a player's inputs for
  consecutive ticks (`Inputs`), an acknowledgment (`Ack`), a time synchronization request
  and its reply (`Quality`, `QualityReply`), a state hash report (`Hash`), a peer leaving
  (`Goodbye`), or an ending relayed to every peer (`End` with an `EndNotice`: `Desync` with
  the tick, the differing peer, the sender's hash and that peer's, `Lost` with the peer, or
  `LocalFailure` with the tick). `Packet::decode` refuses with a `WireError` naming the
  value: another version (whatever the packet's length), a packet over 1200 bytes, bytes
  that are not one packet (trailing bytes included), more than 16 messages, an input message
  with no tick or more than 32, or an input over 24 bytes (each naming the player), more
  than 4 players in an `Ack` or a `Hello` (naming which), or an engine string over 64 bytes.
  `SessionSettings::validate` refuses an engine string over 64 bytes
  (`SettingsError::EngineTooLong`) and a layout where the largest packet of any peer does
  not fit in 1200 bytes (`SettingsError::PacketTooLarge`, naming the peer and both sizes) or
  holds more than 16 messages (`SettingsError::PacketTooManyMessages`, naming the peer and
  both counts), so every peer refuses the same settings.
- Meta 2: `Texture(TextureMeta)` describes how a PNG is cut into frames (pixels per unit,
  an optional frame grid, a pivot, and a filter); version 1 metas still read. A `.png`
  without a meta gets one with the defaults (16 pixels per unit, no grid, centred pivot,
  `Nearest`), and a texture meta with a pixels per unit or frame size of 0 is refused,
  naming the field.
- Edit log 3: an edit can be a group (`Edit::Group`), last, so versions 2 and 1 still read;
  a version 2 log that holds a group is refused with `SimError::EditLogDecode`.
  The undo history (version 4) and replays embed edits, so they may hold groups too; a
  replay keeps its version, and its edit log names its own.
- Stored session 1: `.ludenea/sessions/<id>.session` holds `{ version: 1, saved }`, the
  `SavedSession` in an envelope. The unversioned form before it still reads: the assets it
  lists in `unsavedAssets` stay unsaved, and its `"Prefab"` document names the prefab's
  asset, its `document`, as the game now asks. A stored session that cannot be read
  (not base64, not JSON, a shape the editor does not know, or another version), or that
  the game refuses, no longer blocks opening its document and is never overwritten: the
  document opens from its file, a warning names the session file and the reason, and the
  session is set aside as `.ludenea/sessions/<id>.<n>.rejected.session`. Until it is, no
  session of that document is stored.
- Undo history 4: an "Apply to prefab" entry holds the prefab text and the scene text, each
  before and after it. Version 3 histories still read. A version 2 or 3 history holding a
  group or an "Apply to prefab" entry is refused with the new `UndoError::NotInVersion`,
  and dropped with a warning like any history that does not decode.
- Snapshot 3: a `ludenea::Prefab` asset holds only its entities; version 2 also listed the
  component types that hold an `Entity`. Version 2 snapshots still read, and so do the
  prefab loads of version 2 edit logs, which an edit past tick 0 records for a prefab it
  introduces, in replays, stored sessions, branches and undo histories. The schema of
  `ludenea::Prefab` changed with it, so every game's schema fingerprint changed, and the
  tick hashes of a replay holding a prefab, recorded before, no longer match from tick
  0: record such replays again. Replays keep their version.
- Scenes, prefabs, snapshots, replays and stored sessions of every version that hold a
  value beyond the new limits now fail to load, naming the value: physics values beyond the
  limits of `PhysicsSettings`, `Body` and `Collider`, a `Time` whose `dt` is not finite or
  outside 2^-32 to 1 second (see Rust API), and a prefab the game cannot spawn. Some of
  them loaded and then ended or hung the game process at the next step or spawn, such as
  NaN gravity, 400,000,000 substeps, a NaN `dt`, or a prefab whose component is not
  registered; others ran, such as 65 substeps or a `dt` of 2 seconds. Their encoding is
  unchanged, so their versions are too; correct the value in the file, or record the
  replay again.
- Scene 2: loading refuses an entity or instance identifier, or the first identifier of an
  entity's address, that is not below the scene's `next_id`, naming both. Such a scene
  loaded, and the next tick 0 edit could end the game process or be refused; raise
  `next_id` above every identifier. A scene whose entities have addresses now reports its
  own fault (an empty or repeated address, instances) instead of a missing `id`.
- Prefab 1: loading refuses an entity or instance identifier of a prefab file, nested ones
  included, that is not below the prefab's `next_id`, naming both, as scenes do. Such a
  prefab loaded, and an instance placed in it could reuse an identifier; raise `next_id`
  above every identifier.
