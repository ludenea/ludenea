# Ludenea milestone 2b: the editor that changes the game

Date: 2026-10-18
Status: approved
Parent designs: `2026-10-17-ludenea-engine-design.md` (milestone 2) and
`2026-10-17-ludenea-editor-2a-design.md` (section 14, the 2b outline)

Marks used below: **(author)** means the author decided it in conversation
or in a parent design; **(builder's call)** means the designer chose it and
the author may revisit it. Unmarked text restates a parent design. The author
asked for this document to be written without a question round, so almost
every decision here is a builder's call.

## 1. Intent

Milestone 2a gave Ludenea an editor a developer can live in, with a linear
timeline whose edits are recorded as inputs. Milestone 2b turns determinism
into editor features no other engine has natively:

- **Reusable content:** prefabs and data assets, so a game is more than one
  flat scene file.
- **Runs as files:** a replay is an asset you open, scrub, share, and verify
  in CI.
- **Time as a tree:** editing the past forks the timeline instead of
  destroying the future, and the editor shows how and why the two futures
  differ, entity by entity, down to the field and the event that spread the
  change.
- **Desyncs you can read:** a CI trace or snapshot opens in the editor and
  points at the first divergent tick, entity, component, and field.

Determinism is what makes the last three cheap: a branch is just another
edit log from a fork tick, a replay is a run definition, and a divergence is
a hash tree comparison.

## 2. Goals, non-goals, anti-goals

### Goals

- Prefabs: reusable groups of entities with per-instance field overrides,
  placed in scenes by the editor and spawned at runtime by game code.
- Data assets: game types with `#[derive(Asset)]`, stored as RON, edited in
  the inspector through schemas, content hashed because the simulation reads
  them.
- Replays as assets: self-contained, verifiable by the CLI on every CI
  platform.
- A trace and snapshot viewer.
- Timeline branches: fork on a past edit or a take-over, switch between
  branches, no merging.
- Ghost future: one other branch drawn as outlines in the viewport, plus a
  causal divergence report.
- Everything above reachable through the control surface (parent anti-goal)
  and through the plugin API (2a anti-goal).

### Non-goals (2b)

- Merging branches. (author, from the 2a outline)
- Prefab variants (a prefab derived from another prefab with its own
  overrides). Nesting a prefab instance inside a prefab is in scope
  (section 4.4); variants come later if a game needs them. (builder's call)
- An entity hierarchy (parent and child transforms). The ECS has none, and
  prefabs do not introduce one: a prefab is a group of entities that can
  reference each other through `Entity` fields. (builder's call)
- Hot reload of non-data assets (images, audio): they arrive with
  milestone 3 and later.
- Ghost trails (drawing the other branch over several future ticks at once).
  The ghost shows the other branch at the current tick only. (builder's
  call)
- Editing a replay's input log by hand. Editing a replay forks a branch like
  any other session (section 7.3).
- Persisting undo history (unchanged from 2a).

### Anti-goals

- Any feature that breaks "the same run definition gives the same trace":
  a prefab, a data asset, or a branch the CLI cannot reproduce.
- A world hash that changes for projects not using the new features: the
  `boxes` golden trace must stay valid byte for byte.
- A divergence report that says "these entities differ" without saying since
  when, which field, and through which interaction.
- Two ways to do one thing: one way to spawn a prefab, one way to read a
  data asset, one viewer for traces and snapshots.
- Branches that make the editor heavy: UI below 60 frames per second, or
  memory growing with the length of every branch.

## 3. Constraints

- The determinism contract of the parent design holds. Asset loading,
  file access, and branch bookkeeping live in the host and the project
  service; the simulation sees only resolved, canonical values.
- The 2a architecture holds: project service and game host roles, the five
  transports, the plugin API, no Node.js on the user's machine.
- Protocol changes bump the control protocol to version 3 and the project
  protocol to version 2; the handshake refuses mismatches as before.
  Generated TypeScript types stay checked in CI.
- Every file format introduced or changed here is canonical and versioned.
  Readers accept the previous version of formats that already shipped
  (scene version 1, session export, edit log version 1). (builder's call)
- Sandbox project: relaxed craft standards, but determinism, format
  compatibility, and protocol compatibility are enforced by automated tests.

## 4. Prefabs

### 4.1 What a prefab is (builder's call)

A prefab is an asset file `*.prefab.ron` (meta kind `Prefab`) holding a list
of entities with their components, in the same shape as a scene's entity
list, plus a stable local identifier per entity:

```ron
Prefab(
    version: 1,
    entities: [
        (id: 1, components: {"ludenea::Body": (...), "my_game::Turret": (target: Local(2))}),
        (id: 2, components: {"ludenea::Collider": (...)}),
    ],
)
```

- Local identifiers are small integers assigned by the editor, never reused
  inside one prefab, so overrides survive reordering and deletion of other
  entities.
- `Entity` fields inside a prefab reference other entities of the same
  prefab by local identifier. At instantiation they are remapped to the new
  entities. A reference outside the prefab is refused when the prefab is
  saved, with an error naming the entity, component, and field.
- A prefab has no resources. Resources belong to scenes.

### 4.2 Instances in scenes (builder's call)

Scene version 2 adds an `instances` list beside `entities`:

```ron
Scene(
    version: 2,
    resources: {...},
    entities: [...],
    instances: [
        (prefab: "3f2a...e1", overrides: [
            (entity: 1, component: "ludenea::Body", path: ["linear_velocity", "x"], value: 2.0),
        ], added: [], removed: []),
    ],
)
```

- `prefab` is the asset identifier, never a path (2a section 7).
- An override sets one field by path, using the same path segments as the
  `SetField` command. `added` and `removed` add or remove whole components on
  one prefab entity. Overrides whose target no longer exists in the prefab
  are kept in the file, reported as warnings in the console, and ignored at
  load, so editing a prefab never silently destroys instance data.
- Scene version 1 files load unchanged (no instances).
- Scene version 2 also gives the scene's own entities a stable local
  identifier (`(id: 7, components: {...})`), like prefab entities, so every
  entity that exists at load has a stable **load address**: a scene entity
  identifier, or an instance index followed by a path of local identifiers.
  Version 1 files get identifiers from their positions on first save.

### 4.3 Resolution and determinism (builder's call)

- Loading a scene **resolves** it: every instance is expanded into plain
  entities appended after the scene's own entities, in instance order, each
  instance's entities in prefab order. The simulation never sees a prefab
  instance, only entities. The resolved scene is what the run definition
  contains, so the trace's scene hash is the hash of the resolved scene.
- Resolution is done by a pure function in `ludenea-app` that takes the
  scene text and a lookup from asset identifier to prefab text. The game
  host gets that lookup from the project service in the editor and from the
  project folder in the CLI (section 9).
- A scene without instances resolves to exactly itself, so the `boxes`
  golden trace is untouched.
- The editor keeps the link between entities and their instance: the host
  returns, with the resolved scene, a map from entity to load address. It
  lives outside the simulation state and the hash.
- **Edits and overrides address load-time entities by load address, not by
  entity index.** Adding an entity to a prefab shifts the indices of every
  later instance; addressing by load address keeps recorded edits and
  `Entity` values in overrides on the right entity. Edit log version 2
  stores, for each entity target, either a load address or, for an entity
  spawned at runtime, its entity identifier. A load address that no longer
  resolves drops the edit with the existing `EditDropped` event.
  (builder's call)

### 4.4 Nesting (builder's call)

A prefab can contain instances of other prefabs, with the same `instances`
syntax. Overrides on a nested entity address it by a path of local
identifiers (`entity: [3, 1]`: entity 1 of the instance whose local
identifier is 3). Cycles are refused at save and at load with the chain of
prefab names.

### 4.5 Spawning at runtime (builder's call)

Game code spawns a prefab with one command:

```rust
fn fire(mut commands: Commands, turrets: Query<&Turret>, prefabs: Res<Assets<Prefab>>) {
    for turret in turrets.iter() {
        commands.spawn_prefab(turret.bullet, |overrides| {
            overrides.set::<Velocity>(1, |velocity| velocity.linear = turret.aim);
        });
    }
}
```

- `turret.bullet` is a `Handle<Prefab>`, a typed asset identifier stored in a
  component and editable in the inspector like any field.
- The prefab must be loaded into the simulation as an asset (section 5.3),
  which makes its content part of the hashed state. Spawning is applied at
  the next sync point like any command.

### 4.6 Editing (builder's call)

- Open a prefab in the editor like a scene: the session loads it as a scene
  with only its entities, at tick 0, so the inspector, viewport, and undo
  work unchanged. Saving writes the prefab file.
- In a scene, the scene tree shows instances as collapsible groups. The
  inspector marks overridden fields, with "revert to prefab" and "apply to
  prefab" actions per field and per component.
- "Create prefab from selection" moves selected entities into a new prefab
  file and replaces them with an instance. `Entity` fields pointing from the
  selection to entities outside it are refused with the same error as 4.1.
- Saving a prefab reloads the open scene if it uses it, at tick 0, which
  replays the session like a scene edit (2a section 9.1) and shows the first
  divergent tick.
- **One document at a time** (builder's call): the game host keeps one
  session, as in 2a. Opening a prefab (or another scene, or a replay) saves
  the current session, branches and undo history included, under
  `.ludenea/` and loads the new document; coming back restores it. A stored
  session whose scene or prefabs changed meanwhile replays on restore and
  shows the first divergent tick. Several sessions in one host are out of
  scope for 2b.

## 5. Data assets

### 5.1 Declaring (builder's call)

```rust
#[derive(Asset, Schema, Serialize, Deserialize)]
struct WeaponStats {
    damage: u32,
    cooldown_ticks: u32,
}

impl Plugin for Combat {
    fn build(&self, app: &mut App) {
        app.asset::<WeaponStats>();
    }
}
```

- `derive(Asset)` gives the type a stable name (like components), canonical
  encoding, hashing, and a schema sent at the handshake.
- A data asset file is `*.<anything>.ron` whose meta has kind
  `Data { type: "my_game::WeaponStats" }`. The editor creates one from the
  asset browser by picking a registered asset type; the file starts with the
  type's `Default` if it has one, and with every field required otherwise.

### 5.2 Reading in the simulation (builder's call)

```rust
fn shoot(weapons: Query<&Weapon>, stats: Res<Assets<WeaponStats>>) {
    for weapon in weapons.iter() {
        let stats = stats.get(weapon.stats); // weapon.stats: Handle<WeaponStats>
    }
}
```

- `Assets<T>` is a read-only resource. Systems never mutate assets; an asset
  changes only through an edit (5.4).
- `get` on a handle that is not loaded panics with the asset identifier and
  type. A missing asset is caught before the first tick (5.3), so the panic
  only fires for handles built at runtime from nothing, which is a game bug.
- An edit past tick 0 that introduces a handle to an asset not yet loaded
  (setting a handle field, spawning, inserting a component, or editing an
  asset) is preceded in the edit log by an automatic
  `Edit::LoadAsset { id }`, which loads it (and what it reaches) at the
  same tick. The CLI resolves it like scene assets and replays embed its
  content, so such a run stays reproducible. (builder's call)

### 5.3 Which assets a run loads, and the hash (builder's call)

- A run loads **every data asset and prefab reachable from the scene**:
  handles and prefab identifiers found in the resolved scene's components
  and resources (found generically through `TypeSchema::AssetId` and handle
  schemas), then recursively in the loaded assets themselves. A game that
  needs an asset no field references at load (for example one picked by
  name at runtime) lists it in a scene resource of handles. Loading
  everything in the project was rejected: adding an unrelated file would
  change the hash of every run.
- Missing assets, wrong types, and RON parse errors refuse the scene load
  with the asset identifier, the path if known, and the referencing field.
- The loaded set becomes part of the state as one hash leaf
  `ludenea::Assets`: asset identifiers in order, each with the hash of its
  canonical encoding (not of the file text, so reformatting a file changes
  nothing). Snapshots include the assets' canonical bytes.
- **The leaf is present only when at least one asset is loaded**, so worlds
  without assets hash exactly as in 2a and `golden/boxes.trace` stays valid.

### 5.4 Editing (builder's call)

- At tick 0, editing a data asset in the inspector writes its file and
  replays the session, like a scene edit.
- Past tick 0, an asset edit is an `Edit::SetAsset { id, path, value }` in the
  edit log (edit log version 2), applied at the start of the next tick like
  every edit. It does not write the file; "apply to asset" copies the value
  back, mirroring "apply to scene" (2a section 9.1).
- A file changed on disk outside the editor reloads like a scene file change.

## 6. Timeline branches

### 6.1 Model (builder's call)

The session holds a tree of branches instead of one linear run:

- A **branch** has a name, a parent branch, a fork tick, and its own input
  log and edit log from the fork tick on, plus its own hashes and
  checkpoints after the fork tick. Ticks up to the fork tick are shared with
  the parent and stored once.
- The root branch starts at tick 0 and is named after the scene.
- The run definition of a branch is the scene plus the concatenation of its
  ancestors' logs up to each fork tick plus its own. Flattening a branch
  gives exactly a 2a session, which is what the CLI verifies.

### 6.2 When a branch is created (builder's call)

The two actions that discarded the future in 2a now fork instead:

- an edit at tick N, past tick 0, while the current branch has recorded
  ticks after N;
- a take-over (a live input different from the recorded one at tick N).

The new branch forks at N, becomes current, and is named "Branch 2",
"Branch 3", and so on (renamable). The old future stays intact in the
parent. When nothing is recorded after N, the edit or input extends the
current branch as before, so playing forward never creates branches.

### 6.3 Switching, deleting, limits (builder's call)

- The timeline shows the branch tree as lanes, with fork points and edit
  markers. Switching to a branch keeps the current tick if the branch covers
  it, otherwise goes to the branch's last recorded tick.
- A branch can be renamed and deleted (with its descendants, after a
  confirmation naming them). The root cannot be deleted.
- Memory stays bounded: checkpoints are per branch only after its fork tick
  and follow the same logarithmic spacing; a session-wide checkpoint budget
  evicts checkpoints of branches other than the current one and its parent
  first. An evicted region replays from the nearest remaining checkpoint
  when visited. Hashes are kept for every tick of every branch (8 bytes per
  tick).

### 6.4 Undo and redo (builder's call)

- One undo history per session, across branches. Undoing an entry switches
  to the branch it belongs to first.
- Undoing the edit (or take-over) that created a branch deletes that branch
  and switches back to the parent at the same tick; redo recreates it with
  the same name. Fork and branch deletion are thus undoable.
- Undoing any other edit inside a branch behaves as in 2a, within that
  branch only: the edit is removed and the branch's future after it is
  discarded. Descendant branches forked after that edit's tick depend on it,
  so they are discarded too; the undo entry keeps them and redo restores
  them unchanged. The timeline names the branches an undo will remove
  before it happens (in the command's label), without a confirmation
  dialog. Branches forked before the edit's tick, and unrelated branches,
  are untouched.

### 6.5 Scene edits and rebuilds (builder's call)

A tick 0 scene edit, a prefab or data asset save, or a game rebuild
resimulates the **current branch** at once (2a behaviour, with the first
divergent tick shown). Other branches are marked stale and resimulated when
switched to or compared. A branch edit whose target no longer exists after
resimulation is reported with the existing `EditDropped` event, per branch.

### 6.6 Persistence (builder's call)

The session export (and the browser crash recovery that uses it) carries the
whole branch tree. Branches are also saved in `.ludenea/` per scene so
closing and reopening the editor keeps them; they are local editor state,
not committed. Making a branch shareable is what replays are for (section
7).

## 7. Replays as assets

### 7.1 The file (builder's call)

A replay is `*.replay` (meta kind `Replay`), postcard, versioned, holding:

- the resolved scene text and every loaded asset as canonical RON text, so a
  replay is self-contained, verifiable without the project, and still
  readable after the game's types change (text survives an added field,
  postcard bytes do not);
- the flattened input log and edit log of one branch;
- the hash of every tick (the trace);
- sparse checkpoints (snapshots every 1,024 ticks) so opening and seeking a
  long replay is fast; they are optional and regenerated when missing;
- the game's schema fingerprint (a hash of the component, resource, asset,
  and input schemas) and the engine version, to warn when the replay was
  recorded with different game types.

### 7.2 Creating and verifying

- "Save branch as replay" in the timeline, and `ludenea run ... --replay
  <file>` in the CLI. (builder's call)
- `verify <file.replay>` replays it and compares against its own hashes;
  non-zero exit code on mismatch, with the snapshot dumps of milestone 1.
  (builder's call)
- `examples/boxes` commits a second scenario as a replay using a prefab, a
  data asset, and an edit past tick 0, verified on every CI platform and
  thread count like the golden trace. This is the cross-platform proof for
  2b. (builder's call)

### 7.3 Opening in the editor (builder's call)

Opening a replay starts a session on its content as the root branch,
read-only with respect to the file: play, pause, seek, and inspect work as
usual; an edit or take-over forks a branch as in section 6, which can be
saved as a new replay. When the schema fingerprint differs, the editor
replays anyway when the scene, assets, and logs still decode, and shows the
first divergent tick against the recorded hashes; when they do not decode
(for example the input type changed), it refuses the replay with the first
value that failed and why. (builder's call)

## 8. Divergence: trace viewer, ghost future, causal report

### 8.1 Trace and snapshot viewer (builder's call)

One panel, opened from the asset browser (traces, snapshots, and replays
placed in the project) or from a file picker:

- **Trace against trace or against the current branch:** a strip of ticks
  with the first divergent tick marked; selecting it, when both sides are
  runnable (a replay or the current session), seeks there and opens the
  snapshot comparison.
- **Snapshot against snapshot or against the current tick:** a tree of
  differences grouped by entity, then component, then field, with left and
  right values rendered through the schemas, and enum variant changes shown
  as variant names instead of whole values.
- The `DiffSnapshots` command returns structured differences (subject,
  component, path, left and right as JSON values) instead of display
  strings; the CLI formats them as before.
- Snapshot decoding needs the game's codecs, so the viewer always goes
  through the game host; a snapshot from a different schema fingerprint is
  refused with that reason.

### 8.2 Ghost future (builder's call)

- The timeline has a **compare branch**: by default the parent of the
  current branch, changeable to any branch. While one is set, the viewport
  draws the compare branch's state at the current tick as outlines (debug
  draw with a ghost style) on top of the current branch, and a toggle hides
  it.
- The game host runs the compare branch in a second simulation instance, on
  a separate thread on native and interleaved with the main one in
  WebAssembly. It seeks with the compare branch's own checkpoints, so
  following the current tick costs about one extra tick per tick.
- The ghost never blocks the UI: if the compare simulation lags behind, the
  viewport shows the last ghost frame it has and marks it as lagging.

### 8.3 Causal divergence report (builder's call)

Between the current branch and the compare branch, from their common fork
tick up to the current tick (capped at 600 ticks after the fork by default,
like introspection, and extendable on demand), the host compares entity
hash trees tick by tick and reports:

- the **first divergent tick** overall, and for each divergent entity its own
  first divergent tick, the first differing component and field (through
  `diff_snapshots` restricted to that entity), and its left and right
  values;
- entities present in only one branch (spawned or despawned differently);
- **cause candidates** for each entity's first divergence: the events
  emitted and commands applied at that tick, and at the tick before, that
  reference both that entity and an entity already divergent at that point.
  Entity references in events are found generically through the `Entity`
  fields of event schemas, so a game's own events (a damage event, a
  trigger) explain divergence without any code. This requires events to
  derive `Schema`: `app.event::<T>()` gains a `Schema` bound, a breaking
  change to the public API accepted because the engine is unpublished and
  it matches components, resources, and assets. A command (spawn, insert,
  remove, despawn) is a cause candidate when it targets the divergent
  entity at that tick; the report names the issuing system.
- The compare simulation records introspection (events and commands) over
  the report's range, like the main one, only while the report is open. The edit that created the
  branch is the root cause of the earliest divergent entities.
- Entities are matched across branches by identifier (index and
  generation). Different spawn histories can pair unrelated entities under
  one identifier; the report flags an entity whose component set differs as
  "replaced" rather than inventing a field diff.

The report shows as a panel (a tree from the root cause outward, ordered by
tick) and as a column "diverged at" in the ECS explorer; selecting an entry
seeks to its tick and selects the entity. It is computed only while the
panel or the column is open, so normal play pays nothing.

## 9. Control surface and CLI

### 9.1 Control protocol version 3 (exact shapes are the builder's call)

- **Assets:** the handshake sends asset schemas; `LoadScene` and
  `LoadSession` take the asset texts they need; a `SetAsset` editing command;
  `GetAsset`.
- **Prefabs:** `CreatePrefab` from entities, `ApplyOverride`,
  `RevertOverride`, `ApplyToPrefab`, `GetInstances` (entity to instance map).
- **Branches:** `ListBranches`, `SwitchBranch`, `RenameBranch`,
  `DeleteBranch`, `SetCompareBranch`; the `Timeline` gains the branch tree;
  `TimelineChanged` covers branch changes.
- **Replays:** `ExportReplay`, `LoadReplay`.
- **Divergence:** structured `DiffSnapshots`, `CompareTraces`,
  `GetDivergence` with a tick range, `GetGhostDraw` for a tick.
- Binary payloads (snapshots, traces, replays) travel base64 encoded in
  JSON. (builder's call)

### 9.2 Project protocol version 2

- Sessions stored per document under `.ludenea/` (section 4.6): `StoreSession`
  and `LoadStoredSession`. (builder's call)


- Asset kinds `Prefab`, `Data { type }`, `Replay`, `Trace`, `Snapshot`;
`ReadBinary` and `WriteBinary`; `ResolveAssets` (identifiers to texts) used
by the editor when loading a scene into the game host. (builder's call)

### 9.3 CLI (builder's call)

- `run` and `verify` take `--project <dir>` to resolve prefabs and data
  assets by identifier; it defaults to the nearest folder above the scene
  containing `ludenea.toml`, and to the scene's folder plus its metas
  otherwise (which is how `examples/boxes` works without a manifest).
- `run --replay <file>` writes a replay; `verify <file.replay>` verifies one.
- `diff` keeps its output; it now goes through the structured differences.

## 10. Editor (builder's call)

New built-in plugins, written only against the plugin API like the 2a ones:

- `prefabs`: instance groups in the scene tree, override marks and actions
  in the inspector (through a registration point the inspector exposes to
  every plugin, not a private hook), "create prefab from selection".
- `assets` (extended): create data assets by type, open prefabs, replays,
  traces, and snapshots.
- `timeline` (extended): branch lanes, compare branch picker, save as replay.
- `divergence`: the trace and snapshot viewer and the causal report.
- `viewport` (extended): the ghost layer as a viewport overlay.

The plugin API gains `editor.branches` (list, current, compare, switch) and
`editor.assets` (typed read and edit of data assets through the generated
types). Project plugins get typed access to their asset types like
components.

## 11. Performance (builder's call)

- UI at 60 frames per second while playing with the ghost on, on the 2a
  benchmark scene, measured in the end-to-end suite like 2a.
- Switching branches on a 10,000-tick session: under 200 ms on native when
  a checkpoint is within 64 ticks.
- Divergence report over 600 ticks of the `boxes` scenario: under 1 s on
  native, computed in the background with progress.
- Memory: a session with 20 branches of 10,000 ticks each stays under the
  same checkpoint budget as one branch, plus 8 bytes per tick for hashes and
  the logs themselves.

## 12. Proof of done

- `golden/boxes.trace` unchanged and still verified everywhere.
- The `boxes` replay (prefab, data asset, past edit) verified on macOS,
  Linux, Windows at 1, 2, and 8 threads, under wasmtime, and in the headless
  browser.
- Snapshot and restore at a tick after a past asset edit and a runtime prefab
  spawn give the same remaining trace.
- End-to-end in the browser editor on the template project (which gains one
  prefab and one data asset):
  - place a prefab instance, override a field, edit the prefab, see the
    override kept and the session replayed;
  - play, seek back, edit an entity: a branch appears, the parent future is
    intact, switching back and forth shows both;
  - the ghost shows the parent branch; the causal report lists the edited
    entity at the fork tick and an entity it later collided with, with the
    collision event as cause;
  - save the branch as a replay; the CLI `verify` passes on it;
  - open a deliberately altered trace and snapshot pair in the viewer and see
    the expected entity, component, and field.
- Protocol and format compatibility: scene version 1, edit log version 1,
  and 2a session exports load in 2b; generated TypeScript types are current.

## 13. Left to the builder

- Exact protocol and file shapes, within "canonical and versioned".
- Checkpoint budget and eviction details, compare simulation scheduling.
- Visual design of branch lanes, override marks, the ghost style, and the
  divergence panel.
- Whether resolution of prefab instances happens in the host or in the
  project service, as long as the CLI and the editor use the same function.
