//! The settings of a session, validated before any peer talks to another.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::wire::{Message, Packet};

/// The most peers in one match.
pub const MAX_PEERS: usize = 4;
/// The most players in one match.
pub const MAX_PLAYERS: usize = 4;
/// The most bytes of one player's input for one tick: the example games' inputs take a few
/// bytes, and a gamepad's two stick axes and its buttons fit.
pub const MAX_INPUT_BYTES: usize = 24;
/// The most bytes of one packet: a safe WebRTC payload.
pub const MAX_PACKET_BYTES: usize = 1200;
/// The most ticks a decoder accepts in one input message.
pub const MAX_TICKS_PER_MESSAGE: u32 = 32;
/// The most messages in one packet.
pub const MAX_MESSAGES_PER_PACKET: usize = 16;
/// The most bytes of [`MatchIdentity::engine`]: a version string such as
/// `ludenea 1.2.3+abcdef0` takes far fewer, and the bound keeps a `Hello` within one packet.
pub const MAX_ENGINE_BYTES: usize = 64;

/// A participant of a match: a machine that runs the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerId(pub u8);

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "peer {}", self.0)
    }
}

/// The rules of a match, equal on every peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchRules {
    /// Ticks between a local input being read and the tick it applies to.
    pub input_delay: u32,
    /// How many ticks a session may predict beyond the last confirmed one.
    pub rollback_window: u32,
    /// Confirmed ticks between two state hash reports.
    pub hash_interval: u32,
}

impl MatchRules {
    /// The most local hash reports a peer leaves unacknowledged without breaking
    /// `ViolationRule::HashLagging`: `(2 * (rollback_window + input_delay) + hash_interval - 1)
    /// / hash_interval`. The reports not acknowledged are the multiples of `hash_interval`
    /// after the newest acknowledged one, itself a multiple, up to the newest final one, and
    /// the session checks that the two are at most `2 * (rollback_window + input_delay) +
    /// hash_interval - 1` ticks apart whenever a hash becomes final, before any packet carries
    /// it. Every packet of a running session carries all of them. Expects a positive
    /// `hash_interval`.
    pub fn unacknowledged_hash_reports(&self) -> u64 {
        let interval = u64::from(self.hash_interval);
        let ticks = 2 * (u64::from(self.rollback_window) + u64::from(self.input_delay));
        ticks.div_ceil(interval)
    }
}

impl Default for MatchRules {
    fn default() -> Self {
        Self {
            input_delay: 2,
            rollback_window: 8,
            hash_interval: 30,
        }
    }
}

/// Everything the peers must agree on before a match starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchIdentity {
    /// The engine version string.
    pub engine: String,
    /// The fingerprint of the game's registrations.
    pub fingerprint: u64,
    /// A hash of the scene and the assets the match starts from.
    pub scene_and_assets: u64,
    /// Simulation ticks per second.
    pub tick_rate: u32,
    /// The owner of each global player index, in order.
    pub players: Vec<PeerId>,
    /// The rules of the match.
    pub rules: MatchRules,
}

/// How long silence lasts before a peer is noticed, then disconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeouts {
    /// Silence from a peer, in microseconds, after which the host is told the peer is quiet.
    pub notice_after_micros: u64,
    /// Silence from a peer, in microseconds, after which the peer is disconnected.
    pub disconnect_after_micros: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            notice_after_micros: 750_000,
            disconnect_after_micros: 3_000_000,
        }
    }
}

/// The settings of one peer's session. `Session::new` validates them with
/// [`SessionSettings::validate`] and refuses invalid ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSettings {
    /// The peer this session runs for.
    pub local: PeerId,
    /// Tells this match's packets from any other's.
    pub match_id: u64,
    /// What every peer of the match agrees on.
    pub identity: MatchIdentity,
    /// The silence limits for the other peers.
    pub timeouts: Timeouts,
}

/// Why settings were refused. Each message names the field and its value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// `engine` is longer than [`MAX_ENGINE_BYTES`].
    #[error("engine has {len} bytes, at most {MAX_ENGINE_BYTES} allowed")]
    EngineTooLong { len: usize },
    /// `players` is empty or longer than [`MAX_PLAYERS`].
    #[error("players has {count} entries, expected 1 to {MAX_PLAYERS}")]
    PlayerCount { count: usize },
    /// A player is owned by a peer beyond the last allowed one.
    #[error(
        "player owner {highest_owner} is out of range: a match has at most {MAX_PEERS} peers \
         (0 to {})",
        MAX_PEERS - 1
    )]
    PeerCount { highest_owner: PeerId },
    /// Every player is owned by peer 0. A rollback session needs at least two peers; a
    /// single-peer game steps its `Simulation` directly.
    #[error(
        "every player is owned by peer 0: a rollback session needs at least two peers, a \
         single-peer game steps its Simulation directly"
    )]
    SinglePeer,
    /// The owners of the players leave a gap: they must be exactly the peers 0 to n.
    #[error("{peer} owns no player: the owners of players must be exactly the peers 0 to n")]
    PeerWithoutPlayer { peer: PeerId },
    /// `local` is not one of the `peers` peers of the match.
    #[error("local {local} is not among the {peers} peers of the match")]
    LocalOutOfRange { local: PeerId, peers: usize },
    /// `tick_rate` is zero.
    #[error("tick_rate is 0, expected at least 1")]
    TickRate,
    /// `rollback_window` is zero.
    #[error("rollback_window is {value}, expected at least 1")]
    RollbackWindow { value: u32 },
    /// A sender's worst-case message would carry more ticks than a decoder accepts.
    #[error(
        "rollback_window {rollback_window} plus input_delay {input_delay} plus 1 exceeds \
         {MAX_TICKS_PER_MESSAGE} ticks"
    )]
    WindowAndDelay {
        rollback_window: u32,
        input_delay: u32,
    },
    /// The largest packet `peer` may have to send exceeds [`MAX_PACKET_BYTES`]. Every peer
    /// checks every peer's layout, so all of them refuse the same settings.
    #[error(
        "the worst-case packet of {peer} takes {worst} bytes, more than the {max} bytes of one \
         packet: fewer players per peer, a smaller rollback_window or input_delay fit"
    )]
    PacketTooLarge {
        peer: PeerId,
        worst: usize,
        max: usize,
    },
    /// The worst-case packet of `peer` holds more messages than [`MAX_MESSAGES_PER_PACKET`],
    /// most often because a small `hash_interval` leaves many hash reports unacknowledged.
    #[error(
        "the worst-case packet of {peer} holds {messages} messages, more than the {max} of one \
         packet: a larger hash_interval, a smaller rollback_window or input_delay, or fewer \
         players per peer fit"
    )]
    PacketTooManyMessages {
        peer: PeerId,
        messages: usize,
        max: usize,
    },
    /// `hash_interval` is zero.
    #[error("hash_interval is {value}, expected at least 1")]
    HashInterval { value: u32 },
    /// The default input of the session's [`ludenea_app::PlayerInputSpec`] is longer than
    /// [`MAX_INPUT_BYTES`].
    #[error("the default input takes {len} bytes, at most {MAX_INPUT_BYTES} allowed")]
    DefaultInputTooLarge { len: usize },
    /// The check of the session's [`ludenea_app::PlayerInputSpec`] refuses its own default
    /// input: the two come from different input types.
    #[error("the input check refuses the default input: {reason}")]
    DefaultInputRefused { reason: String },
    /// The timeouts do not satisfy `0 < notice < disconnect`.
    #[error(
        "timeouts notice_after_micros {notice_after_micros} and disconnect_after_micros \
         {disconnect_after_micros} must satisfy 0 < notice < disconnect"
    )]
    Timeouts {
        notice_after_micros: u64,
        disconnect_after_micros: u64,
    },
}

impl SessionSettings {
    /// Checks the settings, in an order where no check overflows.
    ///
    /// # Errors
    ///
    /// The first rule the settings break, as a [`SettingsError`].
    pub fn validate(&self) -> Result<(), SettingsError> {
        let MatchIdentity {
            engine,
            tick_rate,
            players,
            rules,
            ..
        } = &self.identity;
        if engine.len() > MAX_ENGINE_BYTES {
            return Err(SettingsError::EngineTooLong { len: engine.len() });
        }
        if players.is_empty() || players.len() > MAX_PLAYERS {
            return Err(SettingsError::PlayerCount {
                count: players.len(),
            });
        }
        // Not empty, so there is a highest owner. Spectators are out of scope: the owners are
        // exactly the peers 0..n.
        let highest_owner = players.iter().copied().max().unwrap_or(PeerId(0));
        let peers = usize::from(highest_owner.0) + 1;
        if peers > MAX_PEERS {
            return Err(SettingsError::PeerCount { highest_owner });
        }
        if peers < 2 {
            return Err(SettingsError::SinglePeer);
        }
        for peer in (0..=highest_owner.0).map(PeerId) {
            if !players.contains(&peer) {
                return Err(SettingsError::PeerWithoutPlayer { peer });
            }
        }
        if usize::from(self.local.0) >= peers {
            return Err(SettingsError::LocalOutOfRange {
                local: self.local,
                peers,
            });
        }
        if *tick_rate == 0 {
            return Err(SettingsError::TickRate);
        }
        if rules.rollback_window == 0 {
            return Err(SettingsError::RollbackWindow {
                value: rules.rollback_window,
            });
        }
        // A sender's worst-case input message carries the window, the delay and the current
        // tick (the +1), which a decoder bounds by MAX_TICKS_PER_MESSAGE. Added, never subtracted.
        let fits = rules
            .rollback_window
            .checked_add(rules.input_delay)
            .and_then(|ticks| ticks.checked_add(1))
            .is_some_and(|ticks| ticks <= MAX_TICKS_PER_MESSAGE);
        if !fits {
            return Err(SettingsError::WindowAndDelay {
                rollback_window: rules.rollback_window,
                input_delay: rules.input_delay,
            });
        }
        // Before the worst case, which divides by it.
        if rules.hash_interval == 0 {
            return Err(SettingsError::HashInterval {
                value: rules.hash_interval,
            });
        }
        for peer in (0..=highest_owner.0).map(PeerId) {
            let packet = self.worst_case_packet(peer);
            let worst = packet.encode().len();
            if worst > MAX_PACKET_BYTES {
                return Err(SettingsError::PacketTooLarge {
                    peer,
                    worst,
                    max: MAX_PACKET_BYTES,
                });
            }
            if packet.messages.len() > MAX_MESSAGES_PER_PACKET {
                return Err(SettingsError::PacketTooManyMessages {
                    peer,
                    messages: packet.messages.len(),
                    max: MAX_MESSAGES_PER_PACKET,
                });
            }
        }
        let Timeouts {
            notice_after_micros,
            disconnect_after_micros,
        } = self.timeouts;
        if notice_after_micros == 0 || notice_after_micros >= disconnect_after_micros {
            return Err(SettingsError::Timeouts {
                notice_after_micros,
                disconnect_after_micros,
            });
        }
        Ok(())
    }

    /// The largest packet `peer` sends while the match runs. One packet holds, each field at
    /// its largest encoding for the whole life of a match: the inputs of every player `peer`
    /// owns at [`MAX_INPUT_BYTES`] over `rollback_window + input_delay + 1` ticks, the
    /// acknowledgment of every other player, a quality request and its reply, a welcome (owed
    /// to a peer that greets this one again), every hash report a peer may leave
    /// unacknowledged ([`MatchRules::unacknowledged_hash_reports`]), and a goodbye.
    ///
    /// A session that has ended sends no inputs, so a packet carrying an `End` is small and
    /// `End` is not counted. The handshake's `Hello` travels without inputs, and fits whatever
    /// the identity, as the engine string and the players are bounded. Expects the settings to
    /// have passed the peer, tick count and hash interval checks, so another peer owns a
    /// player.
    fn worst_case_packet(&self, peer: PeerId) -> Packet {
        let players = &self.identity.players;
        let owned = players.iter().filter(|owner| **owner == peer).count();
        let remote = players.len() - owned;
        let rules = self.identity.rules;
        // Checked by the caller to be at most MAX_TICKS_PER_MESSAGE.
        let ticks = rules.rollback_window as usize + rules.input_delay as usize + 1;
        let player = u8::MAX;
        let mut messages: Vec<Message> = (0..owned)
            .map(|_| Message::Inputs {
                player,
                first_tick: u64::MAX,
                inputs: vec![vec![u8::MAX; MAX_INPUT_BYTES]; ticks],
            })
            .collect();
        messages.extend([
            Message::Ack {
                players: vec![(player, u64::MAX); remote],
                hashes_through: u64::MAX,
            },
            Message::Quality {
                tick: u64::MAX,
                sent_micros: u64::MAX,
                advantage: i32::MIN,
            },
            Message::QualityReply {
                echo_micros: u64::MAX,
                tick: u64::MAX,
            },
            Message::Welcome,
            Message::Goodbye {
                last_tick: u64::MAX,
            },
        ]);
        let reports = rules.unacknowledged_hash_reports();
        messages.extend((0..reports).map(|_| Message::Hash {
            tick: u64::MAX,
            hash: u64::MAX,
        }));
        Packet {
            match_id: u64::MAX,
            messages,
        }
    }
}
