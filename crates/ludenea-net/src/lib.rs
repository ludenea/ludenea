//! Rollback networking for Ludenea: the settings every peer of a match agrees on, the wire
//! format of their packets, the room where peers agree on who plays, one peer's room then match over one link, the sans-IO session built
//! on them, the executor that applies a session's requests to a simulation, and SyncTest,
//! which catches game code a rollback makes diverge.
//!
//! The crate reads no clock and uses no floating point: the caller passes the current time.
#![deny(clippy::float_arithmetic)]

pub mod code;
pub mod execute;
pub mod link;
pub mod online;
mod queue;
pub mod room;
pub mod session;
pub mod settings;
pub mod sync_test;
pub mod transport;
pub mod wire;

pub use code::{ROOM_CODE_ALPHABET, ROOM_CODE_LENGTH, RoomCode, RoomCodeError};
pub use execute::{ExecuteError, Executor};
pub use link::{
    Channel, Link, LinkError, LinkEvent, LinkId, LinkTransport, MAX_ROOM_MESSAGE_BYTES, MemoryLink,
    MemoryMesh,
};
pub use online::{OnlineError, OnlinePeer, OnlineStatus};
pub use room::{MatchLayout, Room, RoomDrop, RoomError, RoomEvent, RoomRole, RoomState, RoomView};
pub use session::{
    Advanced, DropReason, END_LINGER_MICROS, EndReason, HANDSHAKE_RESEND_MICROS, PeerStats,
    QUALITY_HISTORY, QUALITY_INTERVAL_MICROS, Request, SYNC_MIN_REPORTS, Session, SessionError,
    SessionEvent, SessionState, SessionStats, ViolationRule,
};
pub use settings::{
    MAX_ENGINE_BYTES, MAX_INPUT_BYTES, MAX_MESSAGES_PER_PACKET, MAX_PACKET_BYTES, MAX_PEERS,
    MAX_PLAYERS, MAX_TICKS_PER_MESSAGE, MatchIdentity, MatchRules, PeerId, SessionSettings,
    SettingsError, Timeouts,
};
pub use sync_test::{LISTED_ENTITIES, SyncTestError, SyncTestReport, sync_test};
pub use transport::{
    Conditioned, ConditionsError, LinkConditions, MemoryNetwork, MemoryTransport, Transport,
    TransportError,
};
pub use wire::{
    EndNotice, Message, NET_PROTOCOL_VERSION, Packet, RoomMessage, RoomRefusal, WireError,
};
