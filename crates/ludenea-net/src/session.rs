//! The rollback session of one peer, without IO: the caller feeds it the packets it receives
//! and the current time, and sends the packets it returns.
//!
//! A session starts `Connecting`: it greets every other peer with a `Hello` carrying its view
//! of the match, answers each matching `Hello` with a `Welcome`, and runs once every peer has
//! both welcomed it and sent a matching `Hello`. Everything it receives comes from another
//! machine and is checked before it changes anything; an invalid packet is dropped, counted in
//! [`SessionStats`] and reported as [`SessionEvent::Dropped`].

use std::collections::BTreeMap;

use ludenea_app::PlayerInputSpec;

use crate::settings::{MAX_INPUT_BYTES, MatchIdentity, PeerId, SessionSettings, SettingsError};
use crate::wire::{Message, Packet, WireError};

/// The least time between two `Hello`s to the same peer while it has not welcomed this one.
pub const HANDSHAKE_RESEND_MICROS: u64 = 100_000;

/// How long a session that ended keeps telling the other peers why, so a peer that missed the
/// cause ends for the same reason rather than by a timeout. After a mismatch, it keeps sending
/// its `Hello`, from which every other peer finds the mismatch itself.
pub const END_LINGER_MICROS: u64 = 500_000;

/// Where a session is in the life of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Greeting the other peers.
    Connecting,
    /// Every peer agreed on the match.
    Running,
    /// The match is over for this peer.
    Ended(EndReason),
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// `peer` plays another match: the first `field` that differs, as this peer and `peer`
    /// see it. `field` is one of `net_version`, `engine`, `fingerprint`,
    /// `scene_and_assets`, `tick_rate`, `players`, `input_delay`, `rollback_window`,
    /// `hash_interval` and `match_id`.
    Mismatch {
        peer: PeerId,
        field: &'static str,
        local: String,
        remote: String,
    },
    /// No packet came for `disconnect_after_micros` from `peer`, whose handshake was
    /// unfinished, while connecting.
    Unreachable { peer: PeerId },
}

/// Why a received packet was dropped. Ordered, so the counts of [`PeerStats::dropped`]
/// iterate the same way everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DropReason {
    /// The bytes are not a packet of this build, named after the [`WireError`] variant.
    Wire(&'static str),
    /// The packet belongs to another match.
    WrongMatch,
    /// The sender is not another peer of this match.
    UnknownPeer,
    /// The packet holds more than the handshake while this session is not running yet. Its
    /// handshake messages are still read.
    BeforeRunning,
}

/// What happened in a session since the caller last asked, for the host to act on or log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Every peer agreed on the match: the session runs.
    Started,
    /// A packet from `peer` was dropped.
    Dropped { peer: PeerId, reason: DropReason },
}

/// What a session counts about one sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStats {
    /// The packets dropped, by reason.
    pub dropped: BTreeMap<DropReason, u64>,
}

/// What a session counts, per sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// Every other peer of the match from the start, and any sender the transport named that
    /// is not one, from its first packet.
    pub peers: BTreeMap<PeerId, PeerStats>,
}

/// What this session knows of another peer.
#[derive(Debug)]
struct Remote {
    id: PeerId,
    /// A `Hello` matching the local view arrived.
    hello: bool,
    /// A `Welcome` arrived: the peer received a matching `Hello` from this one.
    welcomed: bool,
    /// A matching `Hello` arrived since the last `Welcome` sent.
    owes_welcome: bool,
    last_hello_micros: Option<u64>,
    /// When the last accepted packet arrived, or when the session first saw the time.
    last_heard_micros: u64,
}

/// The rollback session of one peer of a match.
#[derive(Debug)]
pub struct Session {
    settings: SessionSettings,
    #[expect(dead_code, reason = "read by the input queues, which come next")]
    input: PlayerInputSpec,
    state: SessionState,
    /// When the session ended.
    ended_micros: Option<u64>,
    /// Every other peer, in id order.
    remotes: Vec<Remote>,
    /// The latest time the caller passed, `None` before the first call.
    now_micros: Option<u64>,
    events: Vec<SessionEvent>,
    stats: SessionStats,
}

impl Session {
    /// A session for `settings.local`, `Connecting`.
    ///
    /// # Errors
    ///
    /// The first rule `settings` break, or a default input longer than [`MAX_INPUT_BYTES`] or
    /// refused by `input.check`.
    pub fn new(settings: SessionSettings, input: PlayerInputSpec) -> Result<Self, SettingsError> {
        settings.validate()?;
        if input.default.len() > MAX_INPUT_BYTES {
            return Err(SettingsError::DefaultInputTooLarge {
                len: input.default.len(),
            });
        }
        (input.check)(&input.default).map_err(|error| SettingsError::DefaultInputRefused {
            reason: error.to_string(),
        })?;
        // Validated: the owners of the players are exactly the peers 0 to the highest one.
        let highest = settings
            .identity
            .players
            .iter()
            .map(|owner| owner.0)
            .max()
            .unwrap_or(0);
        let remotes: Vec<Remote> = (0..=highest)
            .map(PeerId)
            .filter(|peer| *peer != settings.local)
            .map(|id| Remote {
                id,
                hello: false,
                welcomed: false,
                owes_welcome: false,
                last_hello_micros: None,
                last_heard_micros: 0,
            })
            .collect();
        let stats = SessionStats {
            peers: remotes
                .iter()
                .map(|remote| (remote.id, PeerStats::default()))
                .collect(),
        };
        Ok(Self {
            settings,
            input,
            state: SessionState::Connecting,
            ended_micros: None,
            remotes,
            now_micros: None,
            events: Vec::new(),
            stats,
        })
    }

    /// Where the session is in the life of the match.
    pub fn state(&self) -> SessionState {
        self.state.clone()
    }

    /// What the session counted so far.
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// The events since the last call, in order.
    pub fn events(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.events)
    }

    /// Reads one packet `from` sent. Bytes that are not a packet of this match from another
    /// of its peers are dropped and counted; an ended session ignores everything.
    pub fn receive(&mut self, from: PeerId, packet: &[u8], now_micros: u64) {
        let now = self.observe(now_micros);
        if matches!(self.state, SessionState::Ended(_)) {
            return;
        }
        let Some(index) = self.remotes.iter().position(|remote| remote.id == from) else {
            self.drop_packet(from, DropReason::UnknownPeer);
            return;
        };
        let packet = match Packet::decode(packet) {
            Ok(packet) => packet,
            // Before the match runs, another version is another match; once it runs, the
            // peer has agreed on this one, so the packet is dropped like any broken one.
            Err(WireError::Version { found, expected })
                if self.state == SessionState::Connecting =>
            {
                let reason = EndReason::Mismatch {
                    peer: from,
                    field: "net_version",
                    local: expected.to_string(),
                    remote: found.to_string(),
                };
                self.end(now, reason);
                return;
            }
            Err(error) => {
                self.drop_packet(from, DropReason::Wire(error.variant()));
                return;
            }
        };
        if packet.match_id != self.settings.match_id {
            // A peer still connecting tells its match in its `Hello`; anything else from
            // another match, or anything once the match runs, is only dropped.
            let hello = packet.messages.iter().find_map(|message| match message {
                Message::Hello { identity } => Some(identity),
                _ => None,
            });
            match hello.filter(|_| self.state == SessionState::Connecting) {
                Some(identity) => {
                    if let Some(reason) = self.compare(from, identity, packet.match_id) {
                        self.end(now, reason);
                    }
                }
                None => self.drop_packet(from, DropReason::WrongMatch),
            }
            return;
        }
        self.remotes[index].last_heard_micros = now;
        let mut before_running = false;
        for message in packet.messages {
            match message {
                Message::Hello { identity } => {
                    // A different identity ends the session in any state. While connecting,
                    // the other peer finds the mismatch from the lingering `Hello`. Once
                    // running, a peer that changes its identity breaks the protocol: it is to
                    // be reported through the end notice once violations and end notices
                    // exist. Until then the other peer ends by its timeout.
                    if let Some(reason) = self.compare(from, &identity, packet.match_id) {
                        self.end(now, reason);
                        return;
                    }
                    let remote = &mut self.remotes[index];
                    remote.hello = true;
                    remote.owes_welcome = true;
                    self.start_when_agreed();
                }
                Message::Welcome => {
                    self.remotes[index].welcomed = true;
                    self.start_when_agreed();
                }
                _ if self.state == SessionState::Connecting => before_running = true,
                // The messages of a running match are read by the layers built on the
                // handshake.
                _ => {}
            }
        }
        if before_running {
            self.drop_packet(from, DropReason::BeforeRunning);
        }
    }

    /// The packets to send now, at most one per peer. While connecting, a session that heard
    /// nothing for `disconnect_after_micros` from a peer whose handshake is unfinished ends
    /// `Unreachable`.
    pub fn outgoing(&mut self, now_micros: u64) -> Vec<(PeerId, Vec<u8>)> {
        let now = self.observe(now_micros);
        let greeting = match &self.state {
            SessionState::Connecting => {
                if let Some(peer) = self.silent_peer(now) {
                    self.end(now, EndReason::Unreachable { peer });
                    return Vec::new();
                }
                true
            }
            SessionState::Running => false,
            SessionState::Ended(EndReason::Mismatch { .. }) => self
                .ended_micros
                .is_some_and(|ended| now.saturating_sub(ended) < END_LINGER_MICROS),
            SessionState::Ended(_) => false,
        };
        let running = !matches!(self.state, SessionState::Ended(_));
        let mut packets = Vec::new();
        for remote in &mut self.remotes {
            let mut messages = Vec::new();
            let hello_due = remote
                .last_hello_micros
                .is_none_or(|sent| now.saturating_sub(sent) >= HANDSHAKE_RESEND_MICROS);
            if greeting && !remote.welcomed && hello_due {
                messages.push(Message::Hello {
                    identity: self.settings.identity.clone(),
                });
                remote.last_hello_micros = Some(now);
            }
            if running && remote.owes_welcome {
                messages.push(Message::Welcome);
                remote.owes_welcome = false;
            }
            if !messages.is_empty() {
                let packet = Packet {
                    match_id: self.settings.match_id,
                    messages,
                };
                packets.push((remote.id, packet.encode()));
            }
        }
        packets
    }

    /// Takes `now_micros` as the current time, or the latest one if it is lower. The first
    /// time seen starts the silence of every peer.
    fn observe(&mut self, now_micros: u64) -> u64 {
        let now = match self.now_micros {
            Some(last) => last.max(now_micros),
            None => {
                for remote in &mut self.remotes {
                    remote.last_heard_micros = now_micros;
                }
                now_micros
            }
        };
        self.now_micros = Some(now);
        now
    }

    /// The first peer whose handshake is unfinished and that has been silent for
    /// `disconnect_after_micros`. A peer that finished its handshake with this one has
    /// nothing more to send until the match runs, so its silence says nothing.
    fn silent_peer(&self, now: u64) -> Option<PeerId> {
        let limit = self.settings.timeouts.disconnect_after_micros;
        self.remotes
            .iter()
            .filter(|remote| !(remote.hello && remote.welcomed))
            .find(|remote| now.saturating_sub(remote.last_heard_micros) >= limit)
            .map(|remote| remote.id)
    }

    /// The first field where `peer`'s view of the match differs from the local one, in the
    /// order of [`EndReason::Mismatch`], or `None` when they agree.
    fn compare(&self, peer: PeerId, remote: &MatchIdentity, match_id: u64) -> Option<EndReason> {
        let local = &self.settings.identity;
        let local_match = self.settings.match_id;
        if local == remote && local_match == match_id {
            return None;
        }
        let (local_rules, remote_rules) = (local.rules, remote.rules);
        let fields = [
            ("engine", local.engine.clone(), remote.engine.clone()),
            (
                "fingerprint",
                hex(local.fingerprint),
                hex(remote.fingerprint),
            ),
            (
                "scene_and_assets",
                hex(local.scene_and_assets),
                hex(remote.scene_and_assets),
            ),
            (
                "tick_rate",
                local.tick_rate.to_string(),
                remote.tick_rate.to_string(),
            ),
            ("players", owners(&local.players), owners(&remote.players)),
            (
                "input_delay",
                local_rules.input_delay.to_string(),
                remote_rules.input_delay.to_string(),
            ),
            (
                "rollback_window",
                local_rules.rollback_window.to_string(),
                remote_rules.rollback_window.to_string(),
            ),
            (
                "hash_interval",
                local_rules.hash_interval.to_string(),
                remote_rules.hash_interval.to_string(),
            ),
            ("match_id", hex(local_match), hex(match_id)),
        ];
        fields
            .into_iter()
            .find(|(_, local, remote)| local != remote)
            .map(|(field, local, remote)| EndReason::Mismatch {
                peer,
                field,
                local,
                remote,
            })
    }

    /// Runs once every peer has welcomed this one and sent a matching `Hello`.
    fn start_when_agreed(&mut self) {
        let agreed = self
            .remotes
            .iter()
            .all(|remote| remote.hello && remote.welcomed);
        if self.state == SessionState::Connecting && agreed {
            self.state = SessionState::Running;
            self.events.push(SessionEvent::Started);
        }
    }

    fn end(&mut self, now: u64, reason: EndReason) {
        self.state = SessionState::Ended(reason);
        self.ended_micros = Some(now);
    }

    fn drop_packet(&mut self, peer: PeerId, reason: DropReason) {
        let count = self
            .stats
            .peers
            .entry(peer)
            .or_default()
            .dropped
            .entry(reason)
            .or_insert(0);
        *count = count.saturating_add(1);
        self.events.push(SessionEvent::Dropped { peer, reason });
    }
}

fn hex(value: u64) -> String {
    format!("{value:016x}")
}

fn owners(players: &[PeerId]) -> String {
    players
        .iter()
        .map(PeerId::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
