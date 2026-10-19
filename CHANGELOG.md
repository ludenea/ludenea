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
| Control protocol       | `PROTOCOL_VERSION`         | 4       | 4                             |
| Project protocol       | `PROJECT_PROTOCOL_VERSION` | 3       | 3                             |
| Scene (`.scene.ron`)   | `SCENE_VERSION`            | 2       | 1                             |
| Prefab (`.prefab.ron`) | `PREFAB_VERSION`           | 1       | 1                             |
| Meta (`.meta`)         | `META_VERSION`             | 1       | 1                             |
| Replay (`.replay`)     | `REPLAY_VERSION`           | 1       | 1                             |
| Trace (`.trace`)       | `TRACE_VERSION`            | 1       | 1                             |
| Snapshot (`.snap`)     | `SNAPSHOT_VERSION`         | 2       | 2                             |
| Input log (`.inputs`)  | `INPUT_LOG_VERSION`        | 1       | 1                             |
| Edit log               | `EDIT_LOG_VERSION`         | 2       | 1                             |
| Undo history           | `UNDO_VERSION`             | 3       | the unversioned form before 2 |

Not versioned yet: data assets (`.ron` files holding a value of the game type their meta
names), the project manifest `ludenea.toml`, the branch tree of a session, the stored
session (the editor's copy of each open document's session), and the plugin API.

## [Unreleased]

### Rust API

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
  `sin_cos` returns `(0, 1)` for an angle that is not finite or too large to reduce, so
  `Rot::from_angle` and `Rot::integrate` give the identity there too. All of them returned
  `(0, 0)`, a rotation that loading a `Pose` refuses.
- The physics step clamps the angular velocity of a kinematic body as it does a dynamic
  one's, to an eighth of a turn per substep, and writes the clamped value back to `Body`.
- `Pose::validate` checks a pose as loading does and returns the new `PoseError`, which
  carries the refused position or rotation. The physics step panics with the entity, the
  field, and its value when it leaves a state that loading would refuse.
- A component that a snapshot cannot restore is named with its entity, and
  `ludenea_ecs::decode_exact` keeps the message of a type that refuses its value, where
  postcard gave "Serde Deserialization Error".

### Control protocol

- Control protocol 4, fingerprint `3e9ffb591ec1ae7f`.
- `hello` and `welcome` carry `protocol_fingerprint`, a hash of the TypeScript declarations
  of every type the messages reach. The host refuses a client of another version or
  fingerprint with `version_mismatch` and says to rebuild the editor
  (`pnpm -C editor build`) and the game from the same checkout; a `hello` of an earlier
  version, without the fingerprint, is refused the same way.
- `set_threads` refuses a count outside 1 to 256, or one the system cannot start, with
  `invalid_command`, and the previous count stays; it ended the game process before.
  `host --stdio` starts on every core up to 256, and on one thread with a console warning
  when the system refuses them.

### Project protocol

- Project protocol 3, fingerprint `227a1de4ad22d2f3`.
- `hello` and `welcome` carry `protocol_fingerprint`, as in the control protocol, and the
  service refuses a client of another version or fingerprint the same way.
- `game_output` events and the `line` events of a build carry a line that is not UTF-8
  with replacement characters. Such a line used to end the relay of its stream: on the
  game's standard output, every later protocol message was lost.

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
  plugin reloads; none of them reach only the browser's developer tools any more.
- Removed: `logToConsole` from `@ludenea/editor-builtins` and the `log` option of
  `createEditor` with its `DocumentLog` type; use `editor.console`.

### Formats

No changes yet.
