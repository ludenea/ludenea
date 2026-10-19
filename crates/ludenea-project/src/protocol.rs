//! The project protocol: what the editor asks of the project service, one JSON message per
//! frame, in the envelope of the control protocol.

use ludenea_control::{Bytes, GameInfo, Hash64};
use ludenea_ecs::AssetId;
use serde::{Deserialize, Serialize};

use crate::{
    AssetKind, BuildEvent, Diagnostic, FileChange, FileEntry, GameOutput, PluginBundle,
    ProjectError,
};

/// The project protocol version this build speaks: the editor and the service connect only
/// when theirs are equal.
pub const PROJECT_PROTOCOL_VERSION: u32 = 3;

/// The wire shape of `PROJECT_PROTOCOL_VERSION`: a hash of the TypeScript declarations of
/// every type the messages reach, without their documentation. The editor and the service
/// connect only when theirs are equal too. `tests/typescript.rs` pins it.
pub const PROJECT_PROTOCOL_FINGERPRINT: Hash64 = Hash64(0x227a_1de4_ad22_d2f3);

/// What to do about a refused handshake: editor and service come from different checkouts.
const REBUILD: &str =
    "rebuild the editor (pnpm -C editor build) and the service from the same checkout";

/// Serde helper: `Result<ProjectReply, ProjectFailure>` as `{"ok": reply}` or `{"error": error}`.
pub mod outcome {
    use super::{ProjectFailure, ProjectReply};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[cfg_attr(
        feature = "ts",
        derive(ts_rs::TS),
        ts(rename = "ProjectOutcome", export_to = "project/")
    )]
    #[serde(rename_all = "snake_case")]
    pub enum Wire {
        Ok(ProjectReply),
        Error(ProjectFailure),
    }

    pub fn serialize<S: Serializer>(
        result: &Result<ProjectReply, ProjectFailure>,
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
    ) -> Result<Result<ProjectReply, ProjectFailure>, D::Error> {
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Ok(reply) => Ok(reply),
            Wire::Error(error) => Err(error),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectClientMessage {
    /// Opens the connection; refused unless both are this build's
    /// (`PROJECT_PROTOCOL_VERSION`, `PROJECT_PROTOCOL_FINGERPRINT`).
    Hello {
        protocol_version: u32,
        protocol_fingerprint: Hash64,
    },
    Request {
        id: u64,
        request: ProjectRequest,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectServerMessage {
    Welcome {
        protocol_version: u32,
        protocol_fingerprint: Hash64,
    },
    Refused {
        error: ProjectFailure,
    },
    Response {
        id: u64,
        #[serde(with = "outcome")]
        #[cfg_attr(feature = "ts", ts(as = "outcome::Wire"))]
        result: Result<ProjectReply, ProjectFailure>,
    },
    Event {
        event: ProjectEvent,
    },
}

/// Paths are relative to the project root unless they name a project (`Create`, `Open`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectRequest {
    /// Creates `parent/name` and opens it. Without `engine_path`, the service's own is used.
    Create {
        parent: String,
        name: String,
        engine_path: Option<String>,
    },
    Open {
        path: String,
    },
    Info,
    ListFiles,
    ReadText {
        path: String,
    },
    WriteText {
        path: String,
        text: String,
    },
    ReadBinary {
        path: String,
    },
    WriteBinary {
        path: String,
        bytes: Bytes,
    },
    /// Creates a file with its meta, both or neither; an existing file is refused.
    /// Replies `File`.
    CreateAsset {
        path: String,
        kind: AssetKind,
        text: String,
    },
    /// Every prefab and data asset, for loading a scene into the game.
    ReadAssetTexts,
    /// Keeps the session of `document` (an export) for the next time it is opened.
    StoreSession {
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        document: AssetId,
        bytes: Bytes,
    },
    /// The session `StoreSession` kept for `document`, if any.
    LoadStoredSession {
        #[cfg_attr(feature = "ts", ts(type = "string"))]
        document: AssetId,
    },
    Rename {
        from: String,
        to: String,
    },
    Delete {
        path: String,
    },
    /// Builds for the service's mode in the background; progress arrives as `Build` events.
    Build,
    ListPlugins,
    GameEndpoint,
    /// Sent after each control handshake so plugins see the game's component types.
    WriteGameTypes {
        info: GameInfo,
    },
    /// Desktop: replaces the running game with the one `GameRebuilt` announced, once the
    /// editor has exported its session. `GameReady` follows.
    RestartGame,
}

impl ProjectRequest {
    /// The `type` of each reply this request may get when it succeeds: one, except where the
    /// service mode decides. The generated TypeScript bindings type each request's reply with
    /// them (`ProjectReplyTypes`).
    pub fn reply_types(&self) -> &'static [&'static str] {
        match self {
            ProjectRequest::Create { .. } | ProjectRequest::Open { .. } | ProjectRequest::Info => {
                &["info"]
            }
            ProjectRequest::ListFiles => &["files"],
            ProjectRequest::ReadText { .. } => &["text"],
            ProjectRequest::ReadBinary { .. } => &["bytes"],
            ProjectRequest::CreateAsset { .. } => &["file"],
            ProjectRequest::ReadAssetTexts => &["asset_texts"],
            ProjectRequest::LoadStoredSession { .. } => &["stored_session"],
            ProjectRequest::ListPlugins => &["plugins"],
            ProjectRequest::GameEndpoint => &["native", "wasm"],
            ProjectRequest::WriteText { .. }
            | ProjectRequest::WriteBinary { .. }
            | ProjectRequest::StoreSession { .. }
            | ProjectRequest::Rename { .. }
            | ProjectRequest::Delete { .. }
            | ProjectRequest::Build
            | ProjectRequest::WriteGameTypes { .. }
            | ProjectRequest::RestartGame => &["done"],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectReply {
    Done,
    /// Answers `Info`, `Open`, and `Create`.
    Info(ProjectInfo),
    Files {
        entries: Vec<FileEntry>,
    },
    Text {
        text: String,
    },
    Bytes {
        bytes: Bytes,
    },
    File(FileEntry),
    AssetTexts(AssetTexts),
    StoredSession {
        bytes: Option<Bytes>,
    },
    Plugins {
        plugins: Vec<PluginBundle>,
    },
    /// Desktop: the backend relays the control protocol to the game process.
    Native,
    /// Browser: the game is a WebAssembly module whose glue the editor loads in a worker.
    Wasm {
        glue_url: String,
    },
}

/// Every prefab and data asset of the project, in path order; data assets in file form
/// (`AssetFile(type: .., value: ..)`). `paths` names the file of each.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct AssetTexts {
    pub assets: Vec<(AssetId, String)>,
    pub paths: Vec<(AssetId, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct ProjectInfo {
    pub name: String,
    pub root: String,
    pub default_scene: Option<String>,
    pub mode: ServiceMode,
}

/// Where the editor runs, which decides how the game is built and run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(rename_all = "snake_case")]
pub enum ServiceMode {
    /// A native game process next to the service.
    Desktop,
    /// A WebAssembly game in the browser tab.
    Browser,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectEvent {
    /// Another project is open (`Open` or `Create`): the editor switches to its plugins.
    ProjectOpened {
        info: ProjectInfo,
    },
    FilesChanged {
        changes: Vec<FileChange>,
    },
    /// `BuildEvent` is tagged by `type` itself, so it travels in a field.
    Build {
        event: BuildEvent,
    },
    /// A new game is running (desktop) or its WebAssembly is ready (browser).
    GameReady,
    /// Desktop, with an editor connected: a rebuilt game waits for `RestartGame`, so the
    /// editor can export the session from the game still running.
    GameRebuilt,
    /// `code` is `None` when a signal ended the game.
    GameExited {
        code: Option<i32>,
    },
    GameOutput(GameOutput),
    PluginRebuilt(PluginBundle),
    PluginFailed {
        name: String,
        diagnostics: Vec<Diagnostic>,
    },
    Warning {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
pub struct ProjectFailure {
    pub code: FailureCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    VersionMismatch,
    NotConnected,
    InvalidRequest,
    NoProject,
    /// Browser mode, before the first successful build.
    NoGame,
    InvalidName,
    OutsideProject,
    AlreadyExists,
    NoEnginePath,
    Io,
    InvalidManifest,
    Watch,
    PluginFailed,
    /// A prefab or data asset that cannot be read, or two with one identifier.
    InvalidAssets,
}

impl ProjectFailure {
    pub fn new(code: FailureCode, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl From<ProjectError> for ProjectFailure {
    fn from(error: ProjectError) -> Self {
        let code = match &error {
            ProjectError::InvalidName(_) => FailureCode::InvalidName,
            ProjectError::OutsideProject(_) => FailureCode::OutsideProject,
            ProjectError::AlreadyExists(_) => FailureCode::AlreadyExists,
            ProjectError::Io { .. } => FailureCode::Io,
            ProjectError::Manifest { .. } => FailureCode::InvalidManifest,
            ProjectError::Watch(_) => FailureCode::Watch,
            ProjectError::Assets(_) => FailureCode::InvalidAssets,
            ProjectError::KindMismatch { .. } => FailureCode::InvalidRequest,
            // The setting names no usable engine either way.
            ProjectError::NoEnginePath | ProjectError::NotAnEngine(_) => FailureCode::NoEnginePath,
        };
        Self::new(code, error)
    }
}

/// Answers a `Hello`: welcome for this protocol version and fingerprint, refusal for any
/// other.
pub fn handshake(protocol_version: u32, protocol_fingerprint: Hash64) -> ProjectServerMessage {
    if protocol_version != PROJECT_PROTOCOL_VERSION {
        ProjectServerMessage::Refused {
            error: version_mismatch(protocol_version),
        }
    } else if protocol_fingerprint != PROJECT_PROTOCOL_FINGERPRINT {
        ProjectServerMessage::Refused {
            error: ProjectFailure::new(
                FailureCode::VersionMismatch,
                format!(
                    "client speaks another revision of project protocol \
                     {PROJECT_PROTOCOL_VERSION} (fingerprint {:016x}, service {:016x}); \
                     {REBUILD}",
                    protocol_fingerprint.0, PROJECT_PROTOCOL_FINGERPRINT.0
                ),
            ),
        }
    } else {
        ProjectServerMessage::Welcome {
            protocol_version: PROJECT_PROTOCOL_VERSION,
            protocol_fingerprint: PROJECT_PROTOCOL_FINGERPRINT,
        }
    }
}

/// The version of a hello of another shape, such as one from a protocol version before
/// `protocol_fingerprint`, so it is refused as such.
pub(crate) fn hello_version(text: &str) -> Option<u32> {
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum AnyHello {
        Hello { protocol_version: u32 },
    }
    let AnyHello::Hello { protocol_version } = serde_json::from_str(text).ok()?;
    Some(protocol_version)
}

/// The refusal of a client of another project protocol version.
pub(crate) fn version_mismatch(protocol_version: u32) -> ProjectFailure {
    ProjectFailure::new(
        FailureCode::VersionMismatch,
        format!(
            "client speaks project protocol {protocol_version}, service speaks \
             {PROJECT_PROTOCOL_VERSION}; {REBUILD}"
        ),
    )
}
