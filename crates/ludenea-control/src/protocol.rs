use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ludenea_app::RawInput;
use ludenea_debug_draw::DebugDrawFrame;
use ludenea_ecs::hash::hex_u64;
use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{Entity, HashTree};
use ludenea_math::Vec2;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

use crate::path::PathSegment;
use crate::session::Timeline;

pub const PROTOCOL_VERSION: u32 = 2;

/// Binary data, serialized as a standard base64 string.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(type = "string"))]
pub struct Bytes(pub Vec<u8>);

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
pub enum ClientMessage {
    Hello { protocol_version: u32 },
    Request { id: u64, command: Command },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
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
    pub input: Option<TypeSchema>,
    pub tick_rate: u32,
    pub debug_layers: Vec<String>,
}

/// The schema of a component or resource type, by stable name.
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
    LoadScene {
        ron: String,
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
    DiffSnapshots {
        a: Bytes,
        b: Bytes,
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
    /// A new version of the scene, as saved on disk; the recorded inputs and edits stay.
    ReplaceScene {
        ron: String,
    },
    /// Restarts an exported session, comparing the new hashes with `expected`. Empty
    /// `inputs`, `edits`, or `undo` bytes stand for empty logs: with all three empty, only
    /// the scene loads.
    LoadSession {
        scene: String,
        /// The scene as last saved, when `scene` holds unsaved edits.
        saved_scene: Option<String>,
        inputs: Bytes,
        edits: Bytes,
        undo: Bytes,
        expected: Vec<Hash64>,
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
    Subscribe {
        topic: Topic,
    },
    Unsubscribe {
        topic: Topic,
    },
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
    /// `ResourcesChanged` with the resources that changed.
    Resources,
    /// Records the events, commands, and system timings of the ticks run while subscribed.
    Introspection,
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
        lines: Vec<String>,
    },
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
