//! The control protocol between a client (CLI, editor) and the engine, the host that
//! executes its commands, and the in-process transport.

pub mod host;
pub mod in_process;
pub mod protocol;

pub use host::Host;
pub use in_process::{InProcess, Transport};
pub use protocol::{
    Bytes, ClientMessage, Command, ErrorCode, Hash64, PROTOCOL_VERSION, ProtocolError, Reply,
    ServerMessage,
};
