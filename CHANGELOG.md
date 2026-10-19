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
| Snapshot (`.snap`)     | `SNAPSHOT_VERSION`         | 3       | 2                             |
| Input log (`.inputs`)  | `INPUT_LOG_VERSION`        | 1       | 1                             |
| Edit log               | `EDIT_LOG_VERSION`         | 2       | 1                             |
| Undo history           | `UNDO_VERSION`             | 4       | the unversioned form before 2 |
| Stored session         | `STORED_SESSION_VERSION`   | 1       | the unversioned form before 1 |

The stored session is the editor's copy of each open document's session, in
`.ludenea/sessions/`; its version is a TypeScript constant of `@ludenea/editor-shell/app`.

Not versioned yet: data assets (`.ron` files holding a value of the game type their meta
names), the project manifest `ludenea.toml`, the branch tree of a session, and the plugin
API.

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
- `Session::resume` refuses a branch tree that holds the branch identifier `u32::MAX` with
  `SessionError::BranchesDecode` and the new `BranchesError::Exhausted`; it panicked. Once
  a session has handed out its last branch identifier, an edit or take-over that would
  fork is refused with the new `SessionError::CannotFork` and changes nothing; it
  panicked, or, built without overflow checks, reused an identifier.
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
  a scene.
- The new `Session::create_prefab` writes the prefab file of scene entities as the scene
  holds them, whatever tick the session shows, and refuses an entity spawned after tick 0
  with `SceneError::MissingTarget` naming it. The new `Session::apply_to_prefab` moves an
  override into the session's text of the instance's prefab and out of the scene, as one
  tick 0 edit that one undo takes back; `ludenea_app::prefab::move_overrides_to_prefab` is
  its text operation.
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
- `Project::set_aside_stored_session` moves the stored session of a document to the first
  free `.ludenea/sessions/<id>.<n>.rejected.session`, from 1, and returns that path, or
  `None` when none is stored.

### Control protocol

- Control protocol 4, fingerprint `341dc87d6714af21`.
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
  the scene. It refuses an entity spawned after tick 0 with `invalid_scene`, naming it.
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

### Project protocol

- Project protocol 3, fingerprint `70a550015903ce41`.
- `hello` and `welcome` carry `protocol_fingerprint`, as in the control protocol, and the
  service refuses a client of another version or fingerprint the same way.
- `game_output` events and the `line` events of a build carry a line that is not UTF-8
  with replacement characters. Such a line used to end the relay of its stream: on the
  game's standard output, every later protocol message was lost.
- `TypeSchema` in `GameInfo` (`write_game_types`) has the `recursive` kind of the control
  protocol; the game types declare it as the alias of the type it names, or `unknown`
  when that type is not among them.
- `set_aside_stored_session { document }` moves the stored session of a document, which the
  editor could not restore, where no later `store_session` overwrites it, and answers
  `set_aside` with its new path, or a null path when none is stored.

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
- "Apply to prefab" (`Prefabs.apply`) no longer writes the prefab file. The game marks the
  prefab unsaved, the scene tree shows each group of an unsaved prefab with a "Save
  prefab" button, and undo clears the mark. The `inspector.save-asset` command saves the
  asset whose identifier it is given, a prefab included, and the selected data asset
  without one.
- "Save scene" (`scene.save`) and `inspector.save-asset` mark the scene or asset saved only
  after the file is written. A failed scene write fails the command with
  `Cannot save <path>: <reason>`, which the palette and Mod+S show in the editor console,
  and the scene stays unsaved.
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

### Formats

- Stored session 1: `.ludenea/sessions/<id>.session` holds `{ version: 1, saved }`, the
  `SavedSession` in an envelope. The unversioned form before it still reads, and the
  assets it lists in `unsavedAssets` stay unsaved. A stored session that cannot be read
  (not base64, not JSON, a shape the editor does not know, or another version), or that
  the game refuses, no longer blocks opening its document and is never overwritten: the
  document opens from its file, a warning names the session file and the reason, and the
  session is set aside as `.ludenea/sessions/<id>.<n>.rejected.session`. Until it is, no
  session of that document is stored.
- Undo history 4: an "Apply to prefab" entry holds the prefab text and the scene text, each
  before and after it. Version 3 histories still read.
- Snapshot 3: a `ludenea::Prefab` asset holds only its entities; version 2 also listed the
  component types that hold an `Entity`. Version 2 snapshots still read. The schema of
  `ludenea::Prefab` changed with it, so every game's schema fingerprint changed, and the
  tick hashes of a replay holding a prefab, recorded before, no longer match from tick
  0: record such replays again. Replays and their version are otherwise unchanged.
- Scene 2: loading refuses an entity or instance identifier, or the first identifier of an
  entity's address, that is not below the scene's `next_id`, naming both. Such a scene
  loaded, and the next tick 0 edit could end the game process or be refused; raise
  `next_id` above every identifier. A scene whose entities have addresses now reports its
  own fault (an empty or repeated address, instances) instead of a missing `id`.
