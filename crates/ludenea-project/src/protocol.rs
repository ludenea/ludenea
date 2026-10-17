//! The project protocol: what the editor asks of the project service, one JSON message per
//! frame, in the envelope of the control protocol.

use ludenea_control::GameInfo;
use serde::{Deserialize, Serialize};

use crate::{
    BuildEvent, Diagnostic, FileChange, FileEntry, GameOutput, PluginBundle, ProjectError,
};

pub const PROJECT_PROTOCOL_VERSION: u32 = 1;

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
    Hello { protocol_version: u32 },
    Request { id: u64, request: ProjectRequest },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export_to = "project/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectServerMessage {
    Welcome {
        protocol_version: u32,
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
            ProjectError::NoEnginePath => FailureCode::NoEnginePath,
        };
        Self::new(code, error)
    }
}

/// Answers a `Hello`: welcome for this protocol version, refusal for any other.
pub fn handshake(protocol_version: u32) -> ProjectServerMessage {
    if protocol_version == PROJECT_PROTOCOL_VERSION {
        ProjectServerMessage::Welcome {
            protocol_version: PROJECT_PROTOCOL_VERSION,
        }
    } else {
        ProjectServerMessage::Refused {
            error: ProjectFailure::new(
                FailureCode::VersionMismatch,
                format!(
                    "client speaks project protocol {protocol_version}, service speaks \
                     {PROJECT_PROTOCOL_VERSION}"
                ),
            ),
        }
    }
}
