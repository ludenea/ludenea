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
- `PhysicsSettings` refuses to load, from a scene, a snapshot, or a `set_field`, a gravity
  component that is not finite or whose magnitude is above the new `MAX_GRAVITY` (1e6),
  `substeps` outside 1 to the new `MAX_SUBSTEPS` (64), a contact damping ratio above the
  new `MAX_CONTACT_DAMPING_RATIO` (1000), and a contact hertz, damping ratio, push
  velocity, or restitution threshold that is not finite or below zero. NaN gravity or a
  huge damping ratio ended the game process at the next step, and 400,000,000 substeps
  hung it. The step brings substeps and a damping ratio that game code writes back within
  these limits.
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
  that do not fit the tree, such as one forked past its parent's run. `Session::warnings`
  names each. Redo puts branches back only where they fork on their parent's run, and a
  fork never takes the identifier of a branch redo put back. A redo entry of a version 2
  history redoes within its branch only where cutting the run after its tick leaves every
  child forking on it; otherwise it is dropped.
- `Session::redo` of a recorded edit puts back only its branch's own part of the run,
  after the run it forks from as it is now. After a tick 0 edit on a forked branch, it
  rebuilt the ticks before the fork from the other branches' stale hashes. A branch part
  recorded by a previous build resimulates from the branch's fork tick, not from tick 0,
  and a game panic while replaying up to that tick refuses the redo and changes nothing.

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
- A command that has to restore a checkpoint the game's own types refuse, such as `seek`,
  fails with `invalid_snapshot` and names the tick, the entity, and why its type refuses
  the value; it ended the game process before. A seek, undo, or redo that fails this way
  changes nothing, and a branch switch stays on the current branch, or on the target at its
  first stale tick. When the session cannot come back to its tick after a game panic, a
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
  version 2 history redoes again where it cuts no branch off; it failed to decode.
- `redo` of an edit on a branch keeps the recorded hashes of the ticks before the branch's
  fork true. After a tick 0 edit on a forked branch they were stale, so a later
  `load_session` on the same build reported a divergence that did not exist.

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
