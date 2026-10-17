use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ludenea_app::RawInput;
use ludenea_ecs::hash::hex_u64;
use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::{Entity, HashTree};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::session::Timeline;

pub const PROTOCOL_VERSION: u32 = 2;

/// Binary data, serialized as a standard base64 string.
#[derive(Clone, Debug, PartialEq)]
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
mod outcome {
    use super::{ProtocolError, Reply};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Wire {
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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello { protocol_version: u32 },
    Request { id: u64, command: Command },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
        result: Result<Reply, ProtocolError>,
    },
    Event {
        event: Event,
    },
}

/// What the client needs to know about the game: its types and how fast it ticks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameInfo {
    pub components: Vec<NamedSchema>,
    pub resources: Vec<NamedSchema>,
    pub input: Option<TypeSchema>,
    pub tick_rate: u32,
    pub debug_layers: Vec<String>,
}

/// The schema of a component or resource type, by stable name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedSchema {
    pub name: String,
    pub schema: TypeSchema,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// message names the failing tick.
    Step {
        ticks: u32,
        record_hashes: bool,
    },
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
    ListEntities,
    GetEntity {
        entity: Entity,
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
    /// Restarts an exported session, comparing the new hashes with `expected`.
    LoadSession {
        scene: String,
        inputs: Bytes,
        edits: Bytes,
        undo: Bytes,
        expected: Vec<Hash64>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
        entities: Vec<Entity>,
    },
    /// Component values are RON strings keyed by stable name.
    Entity {
        entity: Entity,
        components: BTreeMap<String, String>,
    },
    Timeline(Timeline),
    Session {
        scene: String,
        inputs: Bytes,
        edits: Bytes,
        undo: Bytes,
        hashes: Vec<Hash64>,
    },
}

/// What the host reports on its own, from `Host::advance`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Ticked {
        tick: u64,
        hash: Hash64,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PauseReason {
    User,
    /// A system panicked while running `tick`; the session stays at `tick`.
    Panicked {
        tick: u64,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    VersionMismatch,
    NotConnected,
    InvalidScene,
    InvalidInputs,
    InvalidSnapshot,
    NoSuchEntity,
    /// A command argument the host cannot use, such as a non-finite speed.
    InvalidCommand,
    InvalidTick,
    Resimulating,
    NothingToUndo,
    NothingToRedo,
    NoInputMapping,
    SystemPanicked,
}

impl ProtocolError {
    pub(crate) fn new(code: ErrorCode, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}
