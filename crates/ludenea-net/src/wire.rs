//! The versioned format of the packets peers exchange.
//!
//! A packet is [`NET_PROTOCOL_VERSION`] as a little-endian `u16`, then the postcard encoding
//! of a [`Packet`]. The two-byte prefix is frozen for every future version, so a peer can
//! always name the version it received ([`Packet::version_of`]). Everything decoded comes
//! from another machine: [`Packet::decode`] bounds every length before the session reads it.

use serde::{Deserialize, Serialize};

use crate::link::{LinkId, MAX_ROOM_MESSAGE_BYTES};
use crate::settings::{
    MAX_ENGINE_BYTES, MAX_INPUT_BYTES, MAX_MESSAGES_PER_PACKET, MAX_PACKET_BYTES, MAX_PEERS,
    MAX_PLAYERS, MAX_TICKS_PER_MESSAGE, MatchIdentity, PeerId,
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
    /// The state hash of `tick` of `peer`, `remote`, differed from the sender's, `local`.
    Desync {
        tick: u64,
        peer: PeerId,
        local: u64,
        remote: u64,
    },
    /// `peer` stopped answering.
    Lost { peer: PeerId },
    /// The sender's game failed while stepping `tick`.
    LocalFailure { tick: u64 },
}

/// Why received bytes are not a packet this build accepts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// The bytes are longer than `max`: [`MAX_PACKET_BYTES`] for a packet,
    /// [`MAX_ROOM_MESSAGE_BYTES`] for a room message.
    #[error("{len} bytes exceed the {max} bytes allowed")]
    TooLarge { len: usize, max: usize },
    /// The prefix names another version of the format.
    #[error("network protocol version {found}, expected {expected}")]
    Version { found: u16, expected: u16 },
    /// The bytes are not the encoding of a packet.
    #[error("malformed packet: {0}")]
    Malformed(String),
    /// More messages than [`MAX_MESSAGES_PER_PACKET`].
    #[error("packet holds {count} messages, at most {MAX_MESSAGES_PER_PACKET} allowed")]
    TooManyMessages { count: usize },
    /// The input message of `player` holds more ticks than [`MAX_TICKS_PER_MESSAGE`].
    #[error(
        "input message of player {player} holds {count} ticks, at most \
         {MAX_TICKS_PER_MESSAGE} allowed"
    )]
    TooManyTicks { player: u8, count: usize },
    /// The input message of `player` holds no tick.
    #[error("input message of player {player} holds no tick")]
    NoTicks { player: u8 },
    /// One tick's input of `player` is longer than [`MAX_INPUT_BYTES`].
    #[error("input of player {player} takes {len} bytes, at most {MAX_INPUT_BYTES} allowed")]
    InputTooLarge { player: u8, len: usize },
    /// The list of players of a `message` (`"Ack"` or `"Hello"`) is longer than
    /// [`MAX_PLAYERS`].
    #[error("{message} message lists {count} players, at most {MAX_PLAYERS} allowed")]
    TooManyPlayers { message: &'static str, count: usize },
    /// A `Hello` whose engine string is longer than [`MAX_ENGINE_BYTES`].
    #[error("engine string of {len} bytes exceeds {MAX_ENGINE_BYTES} bytes")]
    EngineTooLong { len: usize },
    /// A list of peers in a room message is longer than [`MAX_PEERS`].
    #[error("room message lists {count} peers, at most {MAX_PEERS} allowed")]
    TooManyPeers { count: usize },
}

impl WireError {
    /// The name of the variant, which a session counts dropped packets under.
    pub fn variant(&self) -> &'static str {
        match self {
            WireError::TooLarge { .. } => "TooLarge",
            WireError::Version { .. } => "Version",
            WireError::Malformed(_) => "Malformed",
            WireError::TooManyMessages { .. } => "TooManyMessages",
            WireError::TooManyTicks { .. } => "TooManyTicks",
            WireError::NoTicks { .. } => "NoTicks",
            WireError::InputTooLarge { .. } => "InputTooLarge",
            WireError::TooManyPlayers { .. } => "TooManyPlayers",
            WireError::EngineTooLong { .. } => "EngineTooLong",
            WireError::TooManyPeers { .. } => "TooManyPeers",
        }
    }
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
        check_version(bytes)?;
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(WireError::TooLarge {
                len: bytes.len(),
                max: MAX_PACKET_BYTES,
            });
        }
        // What postcard allocates is bounded by the bytes: it gives a sequence's declared
        // length as the size hint only when at least that many bytes remain, none otherwise
        // (`SeqAccess::size_hint` of postcard 1.1.3, pinned by Cargo.lock), and every element
        // takes at least one byte. With the length check above, no vector holds or reserves
        // more than MAX_PACKET_BYTES elements, whatever length a hostile peer declares.
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
                check_players("Hello", identity.players.len())
            }
            Message::Inputs { player, inputs, .. } => {
                let player = *player;
                if inputs.is_empty() {
                    return Err(WireError::NoTicks { player });
                }
                // A usize holds every u32 on the targets Ludenea builds for (64-bit and wasm32).
                if inputs.len() > MAX_TICKS_PER_MESSAGE as usize {
                    return Err(WireError::TooManyTicks {
                        player,
                        count: inputs.len(),
                    });
                }
                match inputs.iter().find(|input| input.len() > MAX_INPUT_BYTES) {
                    Some(input) => Err(WireError::InputTooLarge {
                        player,
                        len: input.len(),
                    }),
                    None => Ok(()),
                }
            }
            Message::Ack { players, .. } => check_players("Ack", players.len()),
            Message::Welcome
            | Message::Quality { .. }
            | Message::QualityReply { .. }
            | Message::Hash { .. }
            | Message::Goodbye { .. }
            | Message::End { .. } => Ok(()),
        }
    }
}

fn check_players(message: &'static str, count: usize) -> Result<(), WireError> {
    if count > MAX_PLAYERS {
        Err(WireError::TooManyPlayers { message, count })
    } else {
        Ok(())
    }
}

/// Refuses bytes that do not start with this build's version prefix. The version comes first,
/// so bytes of another version name it whatever their length.
fn check_version(bytes: &[u8]) -> Result<(), WireError> {
    let found = Packet::version_of(bytes).ok_or_else(|| {
        WireError::Malformed(format!(
            "{} bytes, shorter than the {PREFIX_BYTES}-byte version prefix",
            bytes.len()
        ))
    })?;
    if found == NET_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(WireError::Version {
            found,
            expected: NET_PROTOCOL_VERSION,
        })
    }
}

/// A message of the reliable room channel, which peers exchange before a match to agree on
/// who plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoomMessage {
    /// The host opens a room for `peers` peers, itself included.
    Host { peers: u8 },
    /// The room is complete: `players` players in all, and the peers `connected` to the sender.
    Ready { players: u8, connected: Vec<LinkId> },
    /// The match the host decided: its id, the peers in peer order, and the local players of
    /// each, in the same order.
    Layout {
        match_id: u64,
        links: Vec<LinkId>,
        counts: Vec<u8>,
    },
    /// The host refuses the room.
    Refuse { reason: RoomRefusal },
}

/// Why a host refuses a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoomRefusal {
    /// The room already holds all its peers.
    Full,
    /// The peers bring `total` players, more than a match allows.
    TooManyPlayers { total: u8 },
    /// The game cannot be played with the players that came.
    Unplayable,
}

impl RoomMessage {
    /// The version prefix followed by the postcard encoding of the message. The sender keeps
    /// the result within [`MAX_ROOM_MESSAGE_BYTES`]; this does not check it.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_extend(self, NET_PROTOCOL_VERSION.to_le_bytes().to_vec())
            .expect("a room message always encodes")
    }

    /// Decodes untrusted bytes, refusing anything but one message of this version within
    /// every limit of the format.
    ///
    /// # Errors
    ///
    /// The first limit the bytes break, as a [`WireError`].
    pub fn decode(bytes: &[u8]) -> Result<RoomMessage, WireError> {
        check_version(bytes)?;
        if bytes.len() > MAX_ROOM_MESSAGE_BYTES {
            return Err(WireError::TooLarge {
                len: bytes.len(),
                max: MAX_ROOM_MESSAGE_BYTES,
            });
        }
        // Bounded like a packet: the length check above caps what a declared length reserves.
        let message: RoomMessage =
            ludenea_ecs::decode_exact(&bytes[PREFIX_BYTES..]).map_err(WireError::Malformed)?;
        message.check()?;
        Ok(message)
    }

    fn check(&self) -> Result<(), WireError> {
        let count = match self {
            RoomMessage::Ready { connected, .. } => connected.len(),
            RoomMessage::Layout { links, counts, .. } => links.len().max(counts.len()),
            RoomMessage::Host { .. } | RoomMessage::Refuse { .. } => return Ok(()),
        };
        if count > MAX_PEERS {
            Err(WireError::TooManyPeers { count })
        } else {
            Ok(())
        }
    }
}
