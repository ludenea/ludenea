//! The versioned format of the packets peers exchange.
//!
//! A packet is [`NET_PROTOCOL_VERSION`] as a little-endian `u16`, then the postcard encoding
//! of a [`Packet`]. The two-byte prefix is frozen for every future version, so a peer can
//! always name the version it received ([`Packet::version_of`]). Everything decoded comes
//! from another machine: [`Packet::decode`] bounds every length before the session reads it.

use serde::{Deserialize, Serialize};

use crate::settings::{
    MAX_ENGINE_BYTES, MAX_INPUT_BYTES, MAX_MESSAGES_PER_PACKET, MAX_PACKET_BYTES, MAX_PLAYERS,
    MAX_TICKS_PER_MESSAGE, MatchIdentity, PeerId,
};

/// The version of the network wire format this build writes and reads.
pub const NET_PROTOCOL_VERSION: u16 = 1;

/// The bytes of the version prefix.
const PREFIX_BYTES: usize = 2;

/// One datagram between two peers of a match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Packet {
    /// Tells this match's packets from any other's.
    pub match_id: u64,
    /// The messages, read in order.
    pub messages: Vec<Message>,
}

/// One message of a packet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// The sender's view of the match, sent until the receiver welcomes it.
    Hello { identity: MatchIdentity },
    /// The receiver's `Hello` matched the sender's identity.
    Welcome,
    /// One player's inputs for consecutive ticks from `first_tick`, at least one.
    Inputs {
        player: u8,
        first_tick: u64,
        inputs: Vec<Vec<u8>>,
    },
    /// For each remote player of the receiver, the next tick the sender still needs; and the
    /// highest hash tick received from the receiver, `0` for none.
    Ack {
        players: Vec<(u8, u64)>,
        hashes_through: u64,
    },
    /// A time synchronization request: the sender's tick, clock, and frame advantage.
    Quality {
        tick: u64,
        sent_micros: u64,
        advantage: i32,
    },
    /// The answer to a `Quality`, echoing its `sent_micros`, with the replier's tick.
    QualityReply { echo_micros: u64, tick: u64 },
    /// The state hash of a confirmed tick.
    Hash { tick: u64, hash: u64 },
    /// The sender leaves the match after `last_tick`.
    Goodbye { last_tick: u64 },
    /// An ending one peer decided, relayed so every peer ends.
    End { reason: EndNotice },
}

/// Why a match ended, as one peer tells the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndNotice {
    /// The state hashes of `peer` differed at `tick`.
    Desync { tick: u64, peer: PeerId },
    /// `peer` stopped answering.
    Lost { peer: PeerId },
    /// The sender's game failed while stepping `tick`.
    LocalFailure { tick: u64 },
}

/// Why received bytes are not a packet this build accepts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// The datagram is longer than [`MAX_PACKET_BYTES`].
    #[error("packet of {len} bytes exceeds {MAX_PACKET_BYTES} bytes")]
    TooLarge { len: usize },
    /// The prefix names another version of the format.
    #[error("network protocol version {found}, expected {expected}")]
    Version { found: u16, expected: u16 },
    /// The bytes are not the encoding of a packet.
    #[error("malformed packet: {0}")]
    Malformed(String),
    /// More messages than [`MAX_MESSAGES_PER_PACKET`].
    #[error("packet holds {count} messages, at most {MAX_MESSAGES_PER_PACKET} allowed")]
    TooManyMessages { count: usize },
    /// An input message with more ticks than [`MAX_TICKS_PER_MESSAGE`].
    #[error("input message holds {count} ticks, at most {MAX_TICKS_PER_MESSAGE} allowed")]
    TooManyTicks { count: usize },
    /// An input message without any tick.
    #[error("input message holds no tick")]
    NoTicks,
    /// One tick's input is longer than [`MAX_INPUT_BYTES`].
    #[error("input of {len} bytes exceeds {MAX_INPUT_BYTES} bytes")]
    InputTooLarge { len: usize },
    /// A list of players longer than [`MAX_PLAYERS`].
    #[error("message lists {count} players, at most {MAX_PLAYERS} allowed")]
    TooManyPlayers { count: usize },
    /// A `Hello` whose engine string is longer than [`MAX_ENGINE_BYTES`].
    #[error("engine string of {len} bytes exceeds {MAX_ENGINE_BYTES} bytes")]
    EngineTooLong { len: usize },
}

impl Packet {
    /// The version prefix followed by the postcard encoding of the packet. The sender keeps
    /// the result within [`MAX_PACKET_BYTES`]; this does not check it.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_extend(self, NET_PROTOCOL_VERSION.to_le_bytes().to_vec())
            .expect("a packet always encodes")
    }

    /// The version a packet of any version was written with: its frozen two-byte prefix, or
    /// `None` when the bytes are shorter than it.
    pub fn version_of(bytes: &[u8]) -> Option<u16> {
        let prefix = bytes.first_chunk::<PREFIX_BYTES>()?;
        Some(u16::from_le_bytes(*prefix))
    }

    /// Decodes untrusted bytes, refusing anything but one packet of this version within
    /// every limit of the format.
    ///
    /// # Errors
    ///
    /// The first limit the bytes break, as a [`WireError`].
    pub fn decode(bytes: &[u8]) -> Result<Packet, WireError> {
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(WireError::TooLarge { len: bytes.len() });
        }
        let found = Self::version_of(bytes).ok_or_else(|| {
            WireError::Malformed(format!(
                "{} bytes, shorter than the {PREFIX_BYTES}-byte version prefix",
                bytes.len()
            ))
        })?;
        if found != NET_PROTOCOL_VERSION {
            return Err(WireError::Version {
                found,
                expected: NET_PROTOCOL_VERSION,
            });
        }
        // The length check above bounds what postcard reads: every element takes at least
        // one byte, so no vector holds more than MAX_PACKET_BYTES elements.
        let packet: Packet =
            ludenea_ecs::decode_exact(&bytes[PREFIX_BYTES..]).map_err(WireError::Malformed)?;
        packet.check()?;
        Ok(packet)
    }

    /// Checks the lengths postcard does not bound.
    fn check(&self) -> Result<(), WireError> {
        if self.messages.len() > MAX_MESSAGES_PER_PACKET {
            return Err(WireError::TooManyMessages {
                count: self.messages.len(),
            });
        }
        self.messages.iter().try_for_each(Message::check)
    }
}

impl Message {
    fn check(&self) -> Result<(), WireError> {
        match self {
            Message::Hello { identity } => {
                if identity.engine.len() > MAX_ENGINE_BYTES {
                    return Err(WireError::EngineTooLong {
                        len: identity.engine.len(),
                    });
                }
                check_players(identity.players.len())
            }
            Message::Inputs { inputs, .. } => {
                if inputs.is_empty() {
                    return Err(WireError::NoTicks);
                }
                // A usize holds every u32 on the targets Ludenea builds for (64-bit and wasm32).
                if inputs.len() > MAX_TICKS_PER_MESSAGE as usize {
                    return Err(WireError::TooManyTicks {
                        count: inputs.len(),
                    });
                }
                match inputs.iter().find(|input| input.len() > MAX_INPUT_BYTES) {
                    Some(input) => Err(WireError::InputTooLarge { len: input.len() }),
                    None => Ok(()),
                }
            }
            Message::Ack { players, .. } => check_players(players.len()),
            Message::Welcome
            | Message::Quality { .. }
            | Message::QualityReply { .. }
            | Message::Hash { .. }
            | Message::Goodbye { .. }
            | Message::End { .. } => Ok(()),
        }
    }
}

fn check_players(count: usize) -> Result<(), WireError> {
    if count > MAX_PLAYERS {
        Err(WireError::TooManyPlayers { count })
    } else {
        Ok(())
    }
}
