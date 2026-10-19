//! The control protocol between a client (CLI, editor) and the engine, the host that
//! executes its commands, the in-process transport, and the editor session timeline.

pub mod branches;
pub mod checkpoints;
mod compare;
pub mod divergence;
pub mod host;
pub mod in_process;
pub mod json;
pub mod pacer;
pub mod path;
pub mod protocol;
pub mod session;
#[cfg(not(target_family = "wasm"))]
pub mod stdio;
mod subscriptions;
pub mod undo;
mod values;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub mod worker;

pub use branches::{BranchId, BranchInfo};
pub use compare::{COMPARE_BUDGET_PAUSED, COMPARE_BUDGET_PLAYING};
pub use divergence::{DIVERGENCE_TICKS, DivergenceReport};
pub use host::Host;
pub use in_process::{InProcess, Transport};
pub use json::{from_wire, to_wire, untyped_to_wire};
pub use path::{PathSegment, set_at_path};
pub use protocol::{
    Bytes, ClientMessage, Command, ComponentValue, ConsoleLevel, EntityRow, ErrorCode, Event,
    FieldTarget, GameInfo, Hash64, NamedSchema, PROTOCOL_FINGERPRINT, PROTOCOL_VERSION,
    PauseReason, ProtocolError, RecordedEvent, Reply, ServerMessage, SyncPoint, SystemRow, Topic,
};
pub use session::{Document, Resimulation, Session, SessionError, SessionExport, Timeline};
pub use subscriptions::{INTROSPECTION_TICKS, RESOURCE_PUSHES_PER_SECOND};
