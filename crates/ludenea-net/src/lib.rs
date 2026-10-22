//! Rollback networking for Ludenea: the settings every peer of a match agrees on, and the
//! sans-IO session built on them.
//!
//! The crate reads no clock and uses no floating point: the caller passes the current time.
#![deny(clippy::float_arithmetic)]

pub mod settings;

pub use settings::{
    MAX_INPUT_BYTES, MAX_MESSAGES_PER_PACKET, MAX_PACKET_BYTES, MAX_PEERS, MAX_PLAYERS,
    MAX_TICKS_PER_MESSAGE, MatchIdentity, MatchRules, PeerId, SessionSettings, SettingsError,
    Timeouts,
};
