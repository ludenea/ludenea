use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ludenea_app::RawInput;
use ludenea_app::prefab::InstanceInfo;
use ludenea_app::trace::TraceComparison;
use ludenea_debug_draw::DebugDrawFrame;
use ludenea_ecs::hash::hex_u64;
use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{AssetId, Difference, Entity, HashTree};
use ludenea_math::Vec2;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

use crate::branches::BranchId;
use crate::divergence::DivergenceReport;
use crate::path::PathSegment;
use crate::session::{Document, Timeline};

/// The control protocol version this build speaks: a client and a host connect only when
/// theirs are equal.
pub const PROTOCOL_VERSION: u32 = 4;

/// The wire shape of `PROTOCOL_VERSION`: a hash of the TypeScript declarations of every type
/// the messages reach, without their documentation. A client and a host connect only when
/// theirs are equal too, so builds from different revisions of one version refuse each other
/// instead of misreading fields. `tests/typescript.rs` pins it.
pub const PROTOCOL_FINGERPRINT: Hash64 = Hash64(0xbbbb_eec8_251a_0960);

/// Binary data, serialized as a standard base64 string.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(type = "string"))]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    fn empty() -> Bytes {
        Bytes(Vec::new())
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text)
            .map(Bytes)
            .map_err(serde::de::Error::custom)
    }
}

/// A state hash, serialized as 16 lowercase hexadecimal digits because JavaScript
/// numbers lose `u64` precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(type = "string"))]
pub struct Hash64(pub u64);

impl Serialize for Hash64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        hex_u64::serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Hash64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        hex_u64::deserialize(deserializer).map(Hash64)
    }
}

/// Serde helper: `Result<Reply, ProtocolError>` as `{"ok": reply}` or `{"error": error}`.
pub mod outcome {
    use super::{ProtocolError, Reply};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "Outcome"))]
    #[serde(rename_all = "snake_case")]
    pub enum Wire {
        Ok(Reply),
        Error(ProtocolError),
    }

    pub fn serialize<S: Serializer>(
        result: &Result<Reply, ProtocolError>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match result {
            Ok(reply) => Wire::Ok(reply.clone()),
            Err(error) => Wire::Error(error.clone()),
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Result<Reply, ProtocolError>, D::Error> {
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Ok(reply) => Ok(reply),
            Wire::Error(error) => Err(error),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
// With 32-bit pointers, as in WebAssembly, the command is small enough for the lint.
#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::large_enum_variant,
        reason = "one message lives per request, briefly: boxing the command would buy nothing"
    )
)]
pub enum ClientMessage {
    /// Opens the connection; refused unless both are this build's (`PROTOCOL_VERSION`,
    /// `PROTOCOL_FINGERPRINT`).
    Hello {
        protocol_version: u32,
        protocol_fingerprint: Hash64,
    },
    Request {
        id: u64,
        command: Command,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
        protocol_fingerprint: Hash64,
        engine_version: String,
        game: GameInfo,
    },
    Refused {
        error: ProtocolError,
    },
    Response {
        id: u64,
        #[serde(with = "outcome")]
        #[cfg_attr(feature = "ts", ts(as = "outcome::Wire"))]
        result: Result<Reply, ProtocolError>,
    },
    Event {
        event: Event,
    },
}

/// What the client needs to know about the game: its types and how fast it ticks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GameInfo {
    pub components: Vec<NamedSchema>,
    pub resources: Vec<NamedSchema>,
    pub assets: Vec<NamedSchema>,
    pub events: Vec<NamedSchema>,
    /// Identifies the game build's types (`Simulation::schema_fingerprint`).
    pub fingerprint: Hash64,
    pub input: Option<TypeSchema>,
    pub tick_rate: u32,
    pub debug_layers: Vec<String>,
}

/// The schema of a component, resource, asset, or event type, by stable name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NamedSchema {
    pub name: String,
    pub schema: TypeSchema,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Loads `scene`, a scene or prefab file as `document` says, with the prefab and data
    /// asset texts it may reach, in file form; replies `Loaded`.
    LoadScene {
        scene: String,
        assets: Vec<(AssetId, String)>,
        document: Document,
    },
    /// Replaces the recorded inputs and moves to tick 0.
    LoadInputs {
        log: Bytes,
    },
    /// Replaces the recorded edits and moves to tick 0; failing edits are dropped by the
    /// resimulation that follows.
    LoadEdits {
        log: Bytes,
    },
    /// Runs the game on `threads` threads, from 1 to 256; results never depend on the count.
    /// A count outside that range, or one the system cannot start, is an `InvalidCommand`
    /// and the previous count stays.
    SetThreads {
        threads: u32,
    },
    /// Runs `ticks` ticks. When a tick fails, the ticks before it stay applied; the error
    /// message names the failing tick. With `record_hashes`, the reply holds the hash after
    /// each tick before the edits recorded at the new tick (the session's recorded hashes),
    /// so it differs from `GetHash` at a tick with a pending edit.
    Step {
        ticks: u32,
        record_hashes: bool,
    },
    /// The hash of the current state, edits recorded at the current tick included (see
    /// `pending`); `Step` with `record_hashes` reports hashes before those edits.
    GetHash,
    GetHashTree,
    TakeSnapshot,
    /// Restores a snapshot of a simulated tick; the recorded future replays on top of it.
    Restore {
        snapshot: Bytes,
    },
    /// The differences from snapshot `a` to snapshot `b`, or to the current tick when `b`
    /// is `None`.
    DiffSnapshots {
        a: Bytes,
        b: Option<Bytes>,
    },
    /// Compares the hashes of trace `a` with those of trace `b`, or with the current
    /// branch's recorded hashes when `b` is `None`.
    CompareTraces {
        a: Bytes,
        b: Option<Bytes>,
    },
    /// A page of the entities matching `filter` (see `EntityFilter::parse`), in ascending
    /// index order.
    QueryEntities {
        filter: String,
        offset: u32,
        limit: u32,
    },
    GetEntity {
        entity: Entity,
    },
    GetResources,
    /// Spawns an entity with these component values.
    Spawn {
        components: BTreeMap<String, JsonValue>,
    },
    Despawn {
        entity: Entity,
    },
    /// Adds a component the entity lacks; one it has fails with `InvalidEdit`, since
    /// changing it goes through `SetField`.
    InsertComponent {
        entity: Entity,
        component: String,
        value: JsonValue,
    },
    RemoveComponent {
        entity: Entity,
        component: String,
    },
    /// Replaces the value at `path` in a component or resource; recorded as the whole new
    /// value.
    SetField {
        target: FieldTarget,
        path: Vec<PathSegment>,
        value: JsonValue,
    },
    /// The session's scene, whatever the current tick, marked as saved.
    SaveScene,
    /// Copies the current value of a component into the scene, as a tick 0 edit: the
    /// session resimulates, then returns to the current tick. The entity must exist at
    /// tick 0.
    ApplyToScene {
        entity: Entity,
        component: String,
    },
    Play,
    Pause,
    Seek {
        tick: u64,
    },
    /// A play speed multiplier, clamped to `[0.1, 8.0]`.
    SetSpeed {
        speed: f32,
    },
    /// The raw input of each player, used while playing until the next one.
    SetLiveInput {
        players: Vec<RawInput>,
    },
    Undo,
    Redo,
    GetTimeline,
    ExportSession,
    /// A new version of the scene and its asset texts, as saved on disk; the recorded
    /// inputs and edits stay. Replies `Loaded`.
    ReplaceScene {
        scene: String,
        assets: Vec<(AssetId, String)>,
    },
    /// Restarts an exported session, comparing the new hashes with `expected`. Empty
    /// `inputs`, `edits`, or `undo` bytes stand for empty logs: with all three empty, only
    /// the scene loads. An `undo` history that does not decode, and its entries that do not
    /// fit the branch tree, are dropped with a warning in `Loaded`.
    LoadSession {
        scene: String,
        /// The scene as last saved, when `scene` holds unsaved edits.
        saved_scene: Option<String>,
        inputs: Bytes,
        edits: Bytes,
        undo: Bytes,
        expected: Vec<Hash64>,
        #[serde(default)]
        assets: Vec<(AssetId, String)>,
        /// The branch tree; empty for a linear session.
        #[serde(default = "Bytes::empty")]
        branches: Bytes,
        #[serde(default)]
        document: Document,
        /// Whether every scene load keeps every asset, as a replay's session does.
        #[serde(default)]
        keeps_assets: bool,
    },
    /// The current value of a loaded asset.
    GetAsset {
        id: AssetId,
    },
    /// Replaces the value at `path` in a data asset: at tick 0 it changes the asset text,
    /// later it is recorded as a `SetAsset` edit.
    SetAssetField {
        id: AssetId,
        path: Vec<PathSegment>,
        value: JsonValue,
    },
    /// The file text to save for an asset; replies `Text`.
    SaveAsset {
        id: AssetId,
    },
    /// Copies the current value of a data asset into its asset text, as a tick 0 edit.
    ApplyToAsset {
        id: AssetId,
    },
    /// The default value of the asset type `asset` in file form; replies `Text`.
    NewAssetValue {
        asset: String,
    },
    SwitchBranch {
        branch: BranchId,
    },
    RenameBranch {
        branch: BranchId,
        name: String,
    },
    /// Deletes a branch and its descendants, switching to its parent when the current
    /// branch is among them; replies `Deleted`.
    DeleteBranch {
        branch: BranchId,
    },
    /// A prefab file of these scene entities as the scene holds them, whatever the current
    /// tick; replies `Text`. An entity spawned after tick 0 is not in the scene: it is
    /// refused.
    CreatePrefab {
        entities: Vec<Entity>,
    },
    /// Appends an instance of `prefab` to the scene, like a tick 0 edit; replies `Placed`.
    PlaceInstance {
        prefab: AssetId,
    },
    /// Moves scene entities into the new prefab `prefab`, whose file text is `prefab_text`,
    /// and places an instance of it in their stead; replies `Placed`.
    ReplaceWithInstance {
        entities: Vec<Entity>,
        prefab: AssetId,
        prefab_text: String,
    },
    /// Overrides the value at `path` of `component` on `entity` of the instance at
    /// `instance` (both paths of local identifiers), like a tick 0 edit. An empty `path`
    /// overrides the whole component.
    ApplyOverride {
        instance: Vec<u32>,
        entity: Vec<u32>,
        component: String,
        path: Vec<PathSegment>,
        value: ComponentValue,
    },
    /// Removes that override; an empty `path` removes every override of the component.
    RevertOverride {
        instance: Vec<u32>,
        entity: Vec<u32>,
        component: String,
        path: Vec<PathSegment>,
    },
    /// Moves that override into the instance's prefab, like a tick 0 edit: the session's
    /// text of the prefab takes the override's value as the scene writes it, and the
    /// override is removed; one undo takes both back. An empty `path` moves every override
    /// of the component. Replies `Text`, the new prefab text, which `SaveAsset` gives to
    /// write.
    ApplyToPrefab {
        instance: Vec<u32>,
        entity: Vec<u32>,
        component: String,
        path: Vec<PathSegment>,
    },
    GetInstances,
    /// The replay of a branch, the current one when `branch` is `None`; replies `Replay`.
    ExportReplay {
        branch: Option<BranchId>,
    },
    /// Opens a replay as the session, its recorded hashes as the expected ones; replies
    /// `ReplayLoaded`. A value that no longer decodes fails, naming it.
    LoadReplay {
        replay: Bytes,
    },
    GetSchedule,
    /// The events sent during the tick that ended at `tick`, recorded while `Introspection`
    /// is subscribed.
    GetTickEvents {
        tick: u64,
    },
    /// The commands applied during the tick that ended at `tick`, recorded while
    /// `Introspection` is subscribed.
    GetTickCommands {
        tick: u64,
    },
    /// The debug drawing of the current tick, for these layers.
    GetDebugDraw {
        layers: Vec<String>,
    },
    /// The entities under a world point.
    Pick {
        point: Vec2,
    },
    /// Chooses the branch the ghost shows (the parent of the current branch while it is
    /// the current one), or turns the ghost off until the next fork.
    SetCompareBranch {
        branch: Option<BranchId>,
    },
    /// The debug drawing of the compare branch at `tick`; replies `GhostDraw`.
    GetGhostDraw {
        tick: u64,
        layers: Vec<String>,
    },
    /// The causal divergence report between the current and the compare branch, from the
    /// tick their runs part up to `up_to` (`DIVERGENCE_TICKS` after it by default), as far
    /// as `Host::advance` computed it; replies `Divergence`.
    GetDivergence {
        up_to: Option<u64>,
    },
    Subscribe {
        topic: Topic,
    },
    Unsubscribe {
        topic: Topic,
    },
}

impl Command {
    /// The `type` of the reply this command gets when it succeeds. The generated TypeScript
    /// bindings type each command's reply with it (`ReplyTypes`).
    pub fn reply_type(&self) -> &'static str {
        match self {
            Command::LoadScene { .. }
            | Command::ReplaceScene { .. }
            | Command::LoadSession { .. } => "loaded",
            Command::Step { .. } => "stepped",
            Command::GetHash => "hash",
            Command::GetHashTree => "hash_tree",
            Command::TakeSnapshot => "snapshot",
            Command::DiffSnapshots { .. } => "differences",
            Command::CompareTraces { .. } => "trace_comparison",
            Command::QueryEntities { .. } => "entities",
            Command::GetEntity { .. } => "entity",
            Command::GetResources => "resources",
            Command::Spawn { .. } => "spawned",
            Command::SaveScene => "scene",
            Command::GetTimeline => "timeline",
            Command::ExportSession => "session",
            Command::GetAsset { .. } => "asset",
            Command::SaveAsset { .. }
            | Command::NewAssetValue { .. }
            | Command::CreatePrefab { .. }
            | Command::ApplyToPrefab { .. } => "text",
            Command::DeleteBranch { .. } => "deleted",
            Command::PlaceInstance { .. } | Command::ReplaceWithInstance { .. } => "placed",
            Command::GetInstances => "instances",
            Command::ExportReplay { .. } => "replay",
            Command::LoadReplay { .. } => "replay_loaded",
            Command::GetSchedule => "schedule",
            Command::GetTickEvents { .. } => "tick_events",
            Command::GetTickCommands { .. } => "tick_commands",
            Command::GetDebugDraw { .. } => "debug_draw",
            Command::Pick { .. } => "picked",
            Command::GetGhostDraw { .. } => "ghost_draw",
            Command::GetDivergence { .. } => "divergence",
            Command::LoadInputs { .. }
            | Command::LoadEdits { .. }
            | Command::SetThreads { .. }
            | Command::Restore { .. }
            | Command::Despawn { .. }
            | Command::InsertComponent { .. }
            | Command::RemoveComponent { .. }
            | Command::SetField { .. }
            | Command::ApplyToScene { .. }
            | Command::Play
            | Command::Pause
            | Command::Seek { .. }
            | Command::SetSpeed { .. }
            | Command::SetLiveInput { .. }
            | Command::Undo
            | Command::Redo
            | Command::SetAssetField { .. }
            | Command::ApplyToAsset { .. }
            | Command::SwitchBranch { .. }
            | Command::RenameBranch { .. }
            | Command::ApplyOverride { .. }
            | Command::RevertOverride { .. }
            | Command::SetCompareBranch { .. }
            | Command::Subscribe { .. }
            | Command::Unsubscribe { .. } => "done",
        }
    }
}

/// What a client can subscribe to. Changes are pushed as events from `Host::advance`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Topic {
    /// `EntitiesChanged` when the matching entities or their hashes change.
    Entities { filter: String },
    /// `EntityChanged` with the components that changed, `EntityDespawned` when it dies.
    Entity { entity: Entity },
    /// `ResourcesChanged` with the resource `name`: its value as subscribed, then each time it
    /// changes, at most `RESOURCE_PUSHES_PER_SECOND` times a second while playing.
    Resource { name: String },
    /// Records the events, commands, and system timings of the ticks run while subscribed.
    Introspection,
    /// Runs the compare simulation toward the current tick; `GhostAdvanced` when it moves.
    Ghost,
    /// Computes the divergence report; `DivergenceProgress` when it moves.
    Divergence,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Done,
    Stepped {
        tick: u64,
        hashes: Vec<Hash64>,
    },
    /// `pending` is true when edits recorded at `tick` are applied but not yet simulated.
    Hash {
        tick: u64,
        hash: Hash64,
        pending: bool,
    },
    HashTree {
        tick: u64,
        tree: HashTree,
    },
    Snapshot {
        tick: u64,
        snapshot: Bytes,
    },
    Differences {
        differences: Vec<Difference>,
    },
    TraceComparison(TraceComparison),
    Entities {
        total: u32,
        rows: Vec<EntityRow>,
    },
    /// Component values keyed by stable name.
    Entity {
        entity: Entity,
        components: BTreeMap<String, ComponentValue>,
    },
    /// Resource values keyed by stable name.
    Resources {
        resources: BTreeMap<String, ComponentValue>,
    },
    Spawned {
        entity: Entity,
    },
    Scene {
        ron: String,
    },
    Timeline(Timeline),
    Session {
        scene: String,
        /// The scene as last saved, when `scene` holds unsaved edits.
        saved_scene: Option<String>,
        inputs: Bytes,
        edits: Bytes,
        undo: Bytes,
        hashes: Vec<Hash64>,
        #[serde(default)]
        assets: Vec<(AssetId, String)>,
        #[serde(default = "Bytes::empty")]
        branches: Bytes,
        #[serde(default)]
        document: Document,
        #[serde(default)]
        keeps_assets: bool,
    },
    /// A scene load: the warnings of stale overrides and of a dropped undo history, the
    /// prefab instances, and where each entity of the scene came from.
    Loaded {
        warnings: Vec<String>,
        instances: Vec<InstanceInfo>,
        load_addresses: Vec<(Entity, Vec<u32>)>,
    },
    /// An asset's type name and value.
    Asset {
        asset: String,
        value: ComponentValue,
    },
    Text {
        text: String,
    },
    /// The deleted branch first, then its descendants.
    Deleted {
        branches: Vec<BranchId>,
    },
    /// The local identifier of a placed instance.
    Placed {
        instance: u32,
    },
    /// The prefab instances of the scene as it is now, and where each of its entities came
    /// from.
    Instances {
        instances: Vec<InstanceInfo>,
        load_addresses: Vec<(Entity, Vec<u32>)>,
    },
    Replay {
        replay: Bytes,
    },
    /// `fingerprint_differs` when the replay was recorded with other game types.
    ReplayLoaded {
        fingerprint_differs: bool,
    },
    Schedule {
        systems: Vec<SystemRow>,
    },
    TickEvents {
        events: Vec<RecordedEvent>,
    },
    TickCommands {
        sync_points: Vec<SyncPoint>,
    },
    DebugDraw {
        tick: u64,
        frame: DebugDrawFrame,
    },
    Picked {
        entities: Vec<Entity>,
    },
    /// The compare branch at `tick`: the tick asked for, or the last one the compare
    /// simulation reached while `lagging` behind. Empty outside the compare branch's run.
    GhostDraw {
        frame: DebugDrawFrame,
        tick: u64,
        lagging: bool,
    },
    Divergence(DivergenceReport),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EntityRow {
    pub entity: Entity,
    pub hash: Hash64,
}

/// A system in run order, its accesses as `"kind name"` (for example
/// `"component ludenea::Body"`), and its mean time over the last observed ticks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SystemRow {
    pub name: String,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    /// `None` until a tick ran while `Introspection` is subscribed, and on every WebAssembly
    /// target (the browser and `wasm32-wasip1`), where time is not measured. The time
    /// includes applying the commands the system issued.
    pub micros: Option<f32>,
}

/// An event by its Rust type name (for display only) and its JSON value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecordedEvent {
    pub event_type: String,
    pub value: JsonValue,
}

/// The commands a system issued, applied right after it, such as
/// `"insert ludenea::Body on 12v0"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SyncPoint {
    pub after_system: String,
    pub commands: Vec<String>,
}

/// What `SetField` changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldTarget {
    Component { entity: Entity, component: String },
    Resource { resource: String },
}

/// A component or resource value: JSON in its wire form, or RON text when the value has
/// no exact JSON form (maps with non-string keys, non-finite floats), which is read-only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComponentValue {
    Json { value: JsonValue },
    Ron { text: String },
}

/// What the host reports on its own, from `Host::advance`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Ticked {
        tick: u64,
        hash: Hash64,
        /// True when edits recorded at `tick` are applied but not yet simulated.
        pending: bool,
        playing: bool,
        effective_rate: f32,
    },
    Paused {
        reason: PauseReason,
    },
    TimelineChanged {
        timeline: Timeline,
    },
    Resimulated {
        up_to: u64,
    },
    /// The first tick whose new hash differs from the recorded one.
    Diverged {
        tick: u64,
        expected: Hash64,
        got: Hash64,
    },
    EditDropped {
        tick: u64,
        reason: String,
    },
    Console {
        level: ConsoleLevel,
        text: String,
    },
    EntitiesChanged {
        filter: String,
        total: u32,
    },
    /// The components whose value changed since the last push, and those removed.
    EntityChanged {
        entity: Entity,
        components: BTreeMap<String, ComponentValue>,
        removed: Vec<String>,
    },
    EntityDespawned {
        entity: Entity,
    },
    /// The resources whose value changed since the last push.
    ResourcesChanged {
        resources: BTreeMap<String, ComponentValue>,
    },
    /// The compare simulation reached `tick`, or follows a tick outside the compare
    /// branch's run.
    GhostAdvanced {
        tick: u64,
    },
    /// The divergence report is compared up to `up_to`.
    DivergenceProgress {
        up_to: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PauseReason {
    User,
    /// A system panicked while running `tick`; the session stays at `tick`.
    Panicked {
        tick: u64,
        message: String,
    },
    /// Running `tick` failed without a panic (such as an input the game cannot read); the
    /// session stays at `tick`.
    Failed {
        tick: u64,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    VersionMismatch,
    NotConnected,
    InvalidScene,
    InvalidInputs,
    InvalidSnapshot,
    InvalidReplay,
    NoSuchEntity,
    /// An edit the target type rejects, or one on a read-only value.
    InvalidEdit,
    UnknownComponent,
    UnknownResource,
    /// A field path the value does not have.
    InvalidPath,
    /// A command argument the host cannot use, such as a non-finite speed.
    InvalidCommand,
    InvalidTick,
    Resimulating,
    NothingToUndo,
    NothingToRedo,
    NoInputMapping,
    SystemPanicked,
    InvalidFilter,
}

impl ProtocolError {
    pub(crate) fn new(code: ErrorCode, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}
