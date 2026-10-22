//! Rollback networking for Ludenea: the settings every peer of a match agrees on, the wire
//! format of their packets, and the sans-IO session built on them.
//!
//! The crate reads no clock and uses no floating point: the caller passes the current time.
#![deny(clippy::float_arithmetic)]

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the rollback session, which comes next")
)]
mod queue;
pub mod settings;
pub mod transport;
pub mod wire;

pub use settings::{
    MAX_ENGINE_BYTES, MAX_INPUT_BYTES, MAX_MESSAGES_PER_PACKET, MAX_PACKET_BYTES, MAX_PEERS,
    MAX_PLAYERS, MAX_TICKS_PER_MESSAGE, MatchIdentity, MatchRules, PeerId, SessionSettings,
    SettingsError, Timeouts,
};
pub use transport::{
    Conditioned, ConditionsError, LinkConditions, MemoryNetwork, MemoryTransport, Transport,
    TransportError,
};
pub use wire::{EndNotice, Message, NET_PROTOCOL_VERSION, Packet, WireError};
