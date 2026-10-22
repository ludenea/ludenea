//! The settings of a session, validated before any peer talks to another.

use serde::{Deserialize, Serialize};

/// The most peers in one match.
pub const MAX_PEERS: usize = 4;
/// The most players in one match.
pub const MAX_PLAYERS: usize = 4;
/// The most bytes of one player's input for one tick.
pub const MAX_INPUT_BYTES: usize = 32;
/// The most bytes of one packet: a safe WebRTC payload.
pub const MAX_PACKET_BYTES: usize = 1200;
/// The most ticks a decoder accepts in one input message.
pub const MAX_TICKS_PER_MESSAGE: u32 = 32;
/// The most messages in one packet.
pub const MAX_MESSAGES_PER_PACKET: usize = 16;

/// A participant of a match: a machine that runs the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerId(pub u8);

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
    pub rules: MatchRules,
}

/// How long silence lasts before a peer is noticed, then disconnected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeouts {
    pub notice_after_micros: u64,
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

/// The settings of one peer's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSettings {
    pub local: PeerId,
    pub match_id: u64,
    pub identity: MatchIdentity,
    pub timeouts: Timeouts,
}

/// Why settings were refused. Each message names the field and its value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("players has {count} entries, expected 1 to {MAX_PLAYERS}")]
    PlayerCount { count: usize },
    #[error(
        "players is owned by peer {} at most, but a match has at most {MAX_PEERS} peers (0 to {})",
        highest_owner.0,
        MAX_PEERS - 1
    )]
    PeerCount { highest_owner: PeerId },
    #[error("peer {} owns no player: the owners of players must be exactly the peers 0 to n", peer.0)]
    PeerWithoutPlayer { peer: PeerId },
    #[error("local peer {} is not among the {peers} peers of the match", local.0)]
    LocalOutOfRange { local: PeerId, peers: usize },
    #[error("tick_rate is 0, expected at least 1")]
    TickRate,
    #[error("rollback_window is {value}, expected at least 1")]
    RollbackWindow { value: u32 },
    #[error(
        "rollback_window {rollback_window} plus input_delay {input_delay} plus 1 exceeds \
         {MAX_TICKS_PER_MESSAGE} ticks"
    )]
    WindowAndDelay {
        rollback_window: u32,
        input_delay: u32,
    },
    #[error("hash_interval is {value}, expected at least 1")]
    HashInterval { value: u32 },
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
            tick_rate,
            players,
            rules,
            ..
        } = &self.identity;
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
        for peer in (0..peers)
            .filter_map(|peer| u8::try_from(peer).ok())
            .map(PeerId)
        {
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
        if rules.hash_interval == 0 {
            return Err(SettingsError::HashInterval {
                value: rules.hash_interval,
            });
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
}
