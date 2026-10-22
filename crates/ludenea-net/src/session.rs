//! The rollback session of one peer, without IO: the caller feeds it the packets it receives
//! and the current time, and sends the packets it returns.
//!
//! A session starts `Connecting`: it greets every other peer with a `Hello` carrying its view
//! of the match, answers each matching `Hello` with a `Welcome`, and runs once every peer has
//! both welcomed it and sent a matching `Hello`. Everything it receives comes from another
//! machine and is checked before it changes anything; an invalid packet is dropped, counted in
//! [`SessionStats`] and reported as [`SessionEvent::Dropped`].
//!
//! Once running, the caller adds an input for each local player and calls
//! [`Session::advance`] once per frame. The session predicts the inputs of remote players by
//! repeating their last confirmed input, and answers with [`Request`]s for the caller's
//! simulation: save a tick, load a tick, advance a tick. When a remote input proves a
//! prediction wrong, the next `advance` loads the first mispredicted tick and runs again up to
//! the current one; when the session is `rollback_window` ticks ahead of the last tick every
//! player's input is confirmed for, it stalls instead of predicting further.
//!
//! A packet that proves its sender is broken or hostile ends the session with
//! [`EndReason::Violation`], naming the sender, the [`ViolationRule`], and the player and tick
//! when there is one. The rules also bound what a hostile peer can make a session hold: the
//! remote inputs past the horizon of an honest peer, and the local inputs a peer leaves
//! unacknowledged.

use std::collections::BTreeMap;

use ludenea_app::PlayerInputSpec;

use crate::queue::{Confirmed, InputQueue, QueueError};
use crate::settings::{
    MAX_INPUT_BYTES, MAX_PACKET_BYTES, MAX_TICKS_PER_MESSAGE, MatchIdentity, PeerId,
    SessionSettings, SettingsError,
};
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
    /// `peer` broke `rule` of the protocol, for the input of `player` at `tick` when the rule
    /// is about one, or about the acknowledged `tick` of `player`.
    Violation {
        peer: PeerId,
        player: Option<u8>,
        tick: Option<u64>,
        rule: ViolationRule,
    },
}

/// A rule of the protocol no honest peer breaks, whatever the network does to its packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationRule {
    /// An input of a player the sender does not own, or that does not exist; or an
    /// acknowledgment of a player the receiver does not own.
    NotOwner,
    /// An input different from the one already confirmed for its tick, which is kept.
    ConflictingInput,
    /// An input the session's input check refuses.
    InvalidInput,
    /// An input for a tick past the furthest an honest peer can have reached.
    BeyondHorizon,
    /// An acknowledgment of an input or a hash report this peer never sent.
    AckNeverSent,
    /// An acknowledgment trailing this peer's inputs by more than an honest peer can.
    AckLagging,
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
    /// The packet holds more than the handshake while this session is not running yet, other
    /// than inputs and acknowledgments from a peer whose matching `Hello` arrived. Its
    /// handshake messages are still read.
    BeforeRunning,
    /// The inputs start after the next tick expected from that player, within the horizon.
    /// Not a violation: a sender resends from the tick acknowledged last, so a later packet
    /// fills the gap.
    Gap,
}

/// What happened in a session since the caller last asked, for the host to act on or log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Every peer agreed on the match: the session runs.
    Started,
    /// A packet from `peer` was dropped.
    Dropped { peer: PeerId, reason: DropReason },
    /// Every player's input of `tick` is confirmed: `inputs` holds one per player, in player
    /// order. Emitted by [`Session::advance`], once per tick, in tick order.
    Confirmed { tick: u64, inputs: Vec<Vec<u8>> },
}

/// What the caller's simulation does for the session, in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Save the state of `tick`, before its inputs. It comes right before the first
    /// `Advance { tick }` after it, and a `Load { tick }` loads the latest one. `hash` asks
    /// for the state hash too; it is always `false` for now.
    Save { tick: u64, hash: bool },
    /// Load the state of `tick` saved last.
    Load { tick: u64 },
    /// Step the state of `tick` to `tick + 1` with `inputs`, one per player, in player order.
    Advance { tick: u64, inputs: Vec<Vec<u8>> },
}

/// Why a call on a [`Session`] was refused. Nothing changed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The session is connecting or has ended.
    #[error("the session is not running")]
    NotRunning,
    /// `player` is not a player of this peer.
    #[error("player {player} is not a local player of this peer")]
    NotLocal { player: u8 },
    /// The input of `player` is longer than [`MAX_INPUT_BYTES`].
    #[error("the input of player {player} takes {len} bytes, at most {MAX_INPUT_BYTES} allowed")]
    InputTooLarge { player: u8, len: usize },
    /// The input check refuses the input of `player`.
    #[error("the input of player {player} is refused: {reason}")]
    InvalidInput { player: u8, reason: String },
    /// The tick would advance, but local `player` has no input added since the last one.
    #[error("local player {player} has no input for the next tick")]
    MissingLocalInput { player: u8 },
    /// The match reached the last tick a `u64` numbers.
    #[error("tick {tick} is the last tick of a match")]
    LastTick { tick: u64 },
}

/// What a session counts about one sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStats {
    /// The packets dropped, by reason. Each dropped packet counts once, under the reason that
    /// stopped reading it: an `Inputs` message with a gap stops the packet there and is the
    /// reason, even when an earlier message of the same packet came before the session ran;
    /// otherwise such a message makes the reason [`DropReason::BeforeRunning`].
    pub dropped: BTreeMap<DropReason, u64>,
    /// The rollbacks a misprediction of this peer's inputs caused.
    pub rollbacks: u64,
    /// The most ticks one of those rollbacks ran again.
    pub deepest_rollback: u64,
    /// The inputs of this peer confirmed different from the input their tick ran with.
    pub prediction_misses: u64,
    /// The advances that stalled waiting for this peer's inputs.
    pub stalls: u64,
}

/// What a session counts, per sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// Every other peer of the match from the start, and any sender the transport named that
    /// is not one, from its first packet.
    pub peers: BTreeMap<PeerId, PeerStats>,
}

/// Why a message stops the reading of its packet.
#[derive(Debug)]
enum Refusal {
    /// The packet is dropped, the session goes on.
    Drop(DropReason),
    /// The sender broke a rule: the session ends.
    Violation {
        player: Option<u8>,
        tick: Option<u64>,
        rule: ViolationRule,
    },
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
    /// Per global player, the first tick of that player's inputs the peer still needs; only
    /// local players' entries are used. Only moves forward.
    acked: Vec<u64>,
    /// Per global player, the tick after the last input of that player sent to the peer; only
    /// local players' entries are used. Only moves forward.
    sent: Vec<u64>,
    /// An advance ran since the last packet: the next one carries inputs and an `Ack`.
    inputs_due: bool,
}

/// The rollback session of one peer of a match.
#[derive(Debug)]
pub struct Session {
    settings: SessionSettings,
    input: PlayerInputSpec,
    /// One queue per global player.
    queues: Vec<InputQueue>,
    /// Per global player, the input a local player added for the next advanced tick.
    pending: Vec<Option<Vec<u8>>>,
    /// The next tick to advance: every tick below it has run.
    tick: u64,
    /// The next tick to emit [`SessionEvent::Confirmed`] for.
    next_event: u64,
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
                acked: vec![0; settings.identity.players.len()],
                sent: vec![0; settings.identity.players.len()],
                inputs_due: false,
            })
            .collect();
        // Each local player's ticks before the input delay hold the default input, confirmed
        // and sent like any other, so every peer knows them.
        let queues = settings
            .identity
            .players
            .iter()
            .map(|owner| {
                let mut queue = InputQueue::new(input.default.clone());
                if *owner == settings.local {
                    for tick in 0..u64::from(settings.identity.rules.input_delay) {
                        // A new queue expects tick 0, then each next one.
                        let confirmed = queue.confirm(tick, input.default.clone());
                        debug_assert_eq!(confirmed, Ok(Confirmed::New));
                    }
                }
                queue
            })
            .collect();
        let stats = SessionStats {
            peers: remotes
                .iter()
                .map(|remote| (remote.id, PeerStats::default()))
                .collect(),
        };
        let pending = vec![None; settings.identity.players.len()];
        Ok(Self {
            settings,
            input,
            queues,
            pending,
            tick: 0,
            next_event: 0,
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
                    // be reported through the end notice once end notices exist. Until then
                    // the other peer ends by its timeout.
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
                // Only a running peer sends inputs and acknowledgments, and a peer runs once
                // every peer agreed on the match: once its matching `Hello` arrived, they are
                // read even while this session still waits for a third peer.
                Message::Inputs { .. } | Message::Ack { .. }
                    if self.state == SessionState::Connecting && !self.remotes[index].hello =>
                {
                    before_running = true;
                }
                Message::Inputs {
                    player,
                    first_tick,
                    inputs,
                } => {
                    self.welcomed_by_running_peer(index);
                    let read = self
                        .read_inputs(from, player, first_tick, inputs)
                        .and_then(|()| self.check_lag(index));
                    if let Err(refusal) = read {
                        self.refuse(now, from, refusal);
                        return;
                    }
                }
                Message::Ack {
                    players,
                    hashes_through,
                } => {
                    self.welcomed_by_running_peer(index);
                    let read = self
                        .read_ack(index, &players, hashes_through)
                        .and_then(|()| self.check_lag(index));
                    if let Err(refusal) = read {
                        self.refuse(now, from, refusal);
                        return;
                    }
                }
                _ if self.state == SessionState::Connecting => before_running = true,
                // Quality, hash, goodbye and end messages are read by the layers built on the
                // inputs.
                _ => {}
            }
        }
        if before_running {
            self.drop_packet(from, DropReason::BeforeRunning);
        }
    }

    /// Sets the input of local `player` for the next tick the session advances. A second
    /// call before that advance replaces it, so a caller that adds one input per frame never
    /// errs while the session stalls. The input applies `input_delay` ticks after the tick
    /// advanced.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotRunning`], [`SessionError::NotLocal`] for a player this peer does not
    /// own, [`SessionError::InputTooLarge`] beyond [`MAX_INPUT_BYTES`], and
    /// [`SessionError::InvalidInput`] for bytes the input check refuses.
    pub fn add_local_input(&mut self, player: u8, input: Vec<u8>) -> Result<(), SessionError> {
        if self.state != SessionState::Running {
            return Err(SessionError::NotRunning);
        }
        let index = usize::from(player);
        if self.settings.identity.players.get(index) != Some(&self.settings.local) {
            return Err(SessionError::NotLocal { player });
        }
        if input.len() > MAX_INPUT_BYTES {
            return Err(SessionError::InputTooLarge {
                player,
                len: input.len(),
            });
        }
        (self.input.check)(&input).map_err(|error| SessionError::InvalidInput {
            player,
            reason: error.to_string(),
        })?;
        self.pending[index] = Some(input);
        Ok(())
    }

    /// Runs the session one frame: first the rollback a misprediction calls for, a `Load` of
    /// the first mispredicted tick then a `Save` and an `Advance` of every tick from it to the
    /// current one; then at most one new tick, taking every local player's pending input, or a
    /// stall when the current tick is `rollback_window` ticks past the confirmed tick. Every
    /// call, stalled or not, has the next packet to each peer carry the local inputs it has
    /// not acknowledged and an `Ack` of its players' inputs.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotRunning`], [`SessionError::MissingLocalInput`] when the tick would
    /// advance and a local player has no pending input, and [`SessionError::LastTick`]. A
    /// refused call changes nothing.
    pub fn advance(&mut self, now_micros: u64) -> Result<Vec<Request>, SessionError> {
        self.observe(now_micros);
        if self.state != SessionState::Running {
            return Err(SessionError::NotRunning);
        }
        let confirmed = self.confirmed_tick();
        let window = u64::from(self.settings.identity.rules.rollback_window);
        let stall = self.tick.saturating_sub(confirmed) >= window;
        // Every refusal comes before anything changes.
        if !stall {
            if let Some(player) = self.local_players().find(|p| self.pending[*p].is_none()) {
                return Err(SessionError::MissingLocalInput {
                    player: u8::try_from(player).unwrap_or(u8::MAX),
                });
            }
            let delay = u64::from(self.settings.identity.rules.input_delay);
            if self.tick.checked_add(delay.max(1)).is_none() {
                return Err(SessionError::LastTick { tick: self.tick });
            }
        }
        let mut requests = Vec::new();
        self.roll_back(&mut requests);
        if stall {
            self.count_stall(confirmed);
        } else {
            self.advance_tick(&mut requests);
        }
        for remote in &mut self.remotes {
            remote.inputs_due = true;
        }
        self.emit_confirmed();
        self.release();
        Ok(requests)
    }

    /// The packets to send now, at most one per peer. While connecting, a session that heard
    /// nothing for `disconnect_after_micros` from a peer whose handshake is unfinished ends
    /// `Unreachable`. While running, the packet after an advance carries the local inputs the
    /// peer has not acknowledged, oldest first, and an `Ack` of the peer's players; when they
    /// do not all fit in [`MAX_PACKET_BYTES`], the newest ticks wait for a later packet.
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
            if self.state == SessionState::Running && remote.inputs_due {
                remote.inputs_due = false;
                messages.extend(inputs_for(
                    &self.settings,
                    &self.queues,
                    remote.id,
                    &remote.acked,
                ));
            }
            if !messages.is_empty() {
                let packet = Packet {
                    match_id: self.settings.match_id,
                    messages,
                };
                let (packet, bytes) = fit(packet);
                for message in &packet.messages {
                    if let Message::Inputs {
                        player,
                        first_tick,
                        inputs,
                    } = message
                    {
                        // A local input held is below `u64::MAX`, the next confirmed tick.
                        let end = u64::try_from(inputs.len())
                            .ok()
                            .and_then(|count| first_tick.checked_add(count))
                            .unwrap_or(u64::MAX);
                        let sent = &mut remote.sent[usize::from(*player)];
                        *sent = (*sent).max(end);
                    }
                }
                packets.push((remote.id, bytes));
            }
        }
        packets
    }

    /// The global indices of the local players, in order.
    fn local_players(&self) -> impl Iterator<Item = usize> + use<'_> {
        let local = self.settings.local;
        self.settings
            .identity
            .players
            .iter()
            .enumerate()
            .filter(move |(_, owner)| **owner == local)
            .map(|(index, _)| index)
    }

    /// The first tick some player lacks a confirmed input for; `u64::MAX` once every player's
    /// last tick is confirmed.
    fn confirmed_tick(&self) -> u64 {
        self.queues
            .iter()
            .map(|queue| queue.next_confirmed().unwrap_or(u64::MAX))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// The input of every player for `tick`, confirmed or predicted.
    fn inputs_at(&self, tick: u64) -> Vec<Vec<u8>> {
        self.queues
            .iter()
            .map(|queue| {
                // Queues release only ticks below the confirmed tick that ran with their
                // confirmed inputs, which no rollback, event or resend reads again.
                queue
                    .input_for(tick)
                    .expect("the inputs of a tick the session reads are held")
            })
            .collect()
    }

    /// Records that `tick` ran with `inputs`.
    fn record(&mut self, tick: u64, inputs: &[Vec<u8>]) {
        for (queue, input) in self.queues.iter_mut().zip(inputs) {
            // Ticks run in order from 0, and a rollback runs again only ticks already run.
            queue
                .record_used(tick, input)
                .expect("the session runs ticks in order");
        }
    }

    /// Loads the first tick below the current one whose input a player now has confirmed
    /// different from the one it ran with, and runs every tick from it up to the current one
    /// again with the inputs known now.
    fn roll_back(&mut self, requests: &mut Vec<Request>) {
        let mut first_by_peer: BTreeMap<PeerId, u64> = BTreeMap::new();
        for (queue, owner) in self.queues.iter().zip(&self.settings.identity.players) {
            if let Some(tick) = queue.first_misprediction(self.tick) {
                let first = first_by_peer.entry(*owner).or_insert(tick);
                *first = (*first).min(tick);
            }
        }
        let Some(first) = first_by_peer.values().copied().min() else {
            return;
        };
        for (peer, tick) in &first_by_peer {
            let stats = self.stats.peers.entry(*peer).or_default();
            stats.rollbacks = stats.rollbacks.saturating_add(1);
            stats.deepest_rollback = stats.deepest_rollback.max(self.tick - tick);
        }
        requests.push(Request::Load { tick: first });
        for tick in first..self.tick {
            let inputs = self.inputs_at(tick);
            self.record(tick, &inputs);
            requests.push(Request::Save { tick, hash: false });
            requests.push(Request::Advance { tick, inputs });
        }
    }

    /// Runs the current tick with the prediction, and confirms every local player's pending
    /// input for the tick `input_delay` later. The caller checked that every local player has
    /// one and that both ticks exist.
    fn advance_tick(&mut self, requests: &mut Vec<Request>) {
        let tick = self.tick;
        let target = tick + u64::from(self.settings.identity.rules.input_delay);
        let local: Vec<usize> = self.local_players().collect();
        for player in local {
            let input = self.pending[player]
                .take()
                .expect("the caller checked every local player has an input");
            // Local inputs are confirmed in order: the queue expects exactly `target`.
            let confirmed = self.queues[player].confirm(target, input);
            debug_assert_eq!(confirmed, Ok(Confirmed::New));
        }
        let inputs = self.inputs_at(tick);
        self.record(tick, &inputs);
        requests.push(Request::Save { tick, hash: false });
        requests.push(Request::Advance { tick, inputs });
        self.tick = tick + 1;
    }

    /// Counts a stall against every peer owning a player whose input of the confirmed tick is
    /// missing.
    fn count_stall(&mut self, confirmed: u64) {
        for (queue, owner) in self.queues.iter().zip(&self.settings.identity.players) {
            if queue.next_confirmed() == Some(confirmed) {
                let stats = self.stats.peers.entry(*owner).or_default();
                stats.stalls = stats.stalls.saturating_add(1);
            }
        }
    }

    /// Emits [`SessionEvent::Confirmed`] for every tick up to the confirmed one not emitted yet.
    fn emit_confirmed(&mut self) {
        let confirmed = self.confirmed_tick();
        while self.next_event < confirmed {
            let tick = self.next_event;
            let inputs = self.inputs_at(tick);
            self.events.push(SessionEvent::Confirmed { tick, inputs });
            self.next_event = tick + 1;
        }
    }

    /// Releases the inputs nothing can read again: those below the confirmed tick, once every
    /// tick below it ran with them, and for a local player only those every peer acknowledged.
    fn release(&mut self) {
        for (player, queue) in self.queues.iter_mut().enumerate() {
            // Every tick below the next event is confirmed, and emitted.
            let mut below = self.next_event;
            if self.settings.identity.players[player] == self.settings.local {
                for remote in &self.remotes {
                    below = below.min(remote.acked[player]);
                }
            }
            queue.release_before(below);
        }
    }

    /// The tick after the local horizon `h`, the highest tick a local player has an input
    /// for: an advance of tick `t` confirms the local inputs of `t + input_delay`, so the
    /// current tick plus `input_delay` is `h + 1`. Kept as `h + 1` because with no input
    /// delay a session at tick 0 holds no local input yet and `h` would be `-1`.
    fn after_horizon(&self) -> u64 {
        let delay = u64::from(self.settings.identity.rules.input_delay);
        self.tick.saturating_add(delay)
    }

    /// The highest tick an honest peer can send an input for: `h + 1 + rollback_window +
    /// input_delay`. Above it is [`ViolationRule::BeyondHorizon`].
    ///
    /// A peer advances only while its tick is less than `rollback_window` past its confirmed
    /// tick, so its tick is at most its confirmed tick plus `rollback_window`, and its highest
    /// input, at its tick plus `input_delay` minus one, at most its confirmed tick plus
    /// `rollback_window + input_delay - 1`. Its confirmed tick needs the inputs of the local
    /// players, which go no further than `h`, so it is at most `h + 1`. An honest peer's inputs
    /// thus reach `h + rollback_window + input_delay` at most, one tick below the limit.
    fn input_limit(&self) -> u64 {
        let rules = self.settings.identity.rules;
        self.after_horizon()
            .saturating_add(u64::from(rules.rollback_window))
            .saturating_add(u64::from(rules.input_delay))
    }

    /// Confirms the inputs of `player` from `first_tick` that `from` sent. Every received tick
    /// is checked before it is computed on, and every input before any is confirmed: the owner
    /// ([`ViolationRule::NotOwner`]), the horizon, which a tick range past the last tick a
    /// `u64` numbers also breaks ([`ViolationRule::BeyondHorizon`]), and the input check
    /// ([`ViolationRule::InvalidInput`]). Then each input is confirmed in order: one that
    /// differs from the confirmed input of its tick is [`ViolationRule::ConflictingInput`],
    /// and inputs that start after the next expected tick are dropped as a [`DropReason::Gap`].
    fn read_inputs(
        &mut self,
        from: PeerId,
        player: u8,
        first_tick: u64,
        inputs: Vec<Vec<u8>>,
    ) -> Result<(), Refusal> {
        let violation = |tick, rule| Refusal::Violation {
            player: Some(player),
            tick: Some(tick),
            rule,
        };
        let index = usize::from(player);
        if self.settings.identity.players.get(index) != Some(&from) {
            return Err(violation(first_tick, ViolationRule::NotOwner));
        }
        // A decoded message holds at least one tick: its last is `first_tick + count - 1`,
        // `None` when no `u64` numbers it. The ticks are an inclusive range up to it: an open
        // range from `u64::MAX` would overflow computing the tick after it.
        let limit = self.input_limit();
        let last_tick = u64::try_from(inputs.len())
            .ok()
            .and_then(|count| first_tick.checked_add(count.saturating_sub(1)));
        match last_tick {
            Some(last_tick) if last_tick <= limit => {
                let ticks = (first_tick..=last_tick).zip(&inputs);
                if let Some((tick, _)) = ticks
                    .clone()
                    .find(|(_, input)| (self.input.check)(input).is_err())
                {
                    return Err(violation(tick, ViolationRule::InvalidInput));
                }
                for (tick, input) in ticks {
                    let queue = &mut self.queues[index];
                    let used = queue.used(tick).map(<[u8]>::to_vec);
                    match queue.confirm(tick, input.clone()) {
                        Ok(Confirmed::New) => {
                            if used.is_some_and(|used| used != *input) {
                                let stats = self.stats.peers.entry(from).or_default();
                                stats.prediction_misses = stats.prediction_misses.saturating_add(1);
                            }
                        }
                        Ok(Confirmed::Same | Confirmed::Released) => {}
                        Err(QueueError::Conflict { .. }) => {
                            return Err(violation(tick, ViolationRule::ConflictingInput));
                        }
                        Err(QueueError::Gap { .. } | QueueError::NotNextSimulated { .. }) => {
                            return Err(Refusal::Drop(DropReason::Gap));
                        }
                    }
                }
                Ok(())
            }
            // The first tick beyond the limit; the limit is below `u64::MAX` whenever a tick
            // range runs past it, as the current tick is.
            _ => Err(violation(
                first_tick.max(limit.saturating_add(1)),
                ViolationRule::BeyondHorizon,
            )),
        }
    }

    /// Moves forward what the remote at `index` acknowledged of each local player. Every entry
    /// is checked before any is used: a player this peer does not own is
    /// [`ViolationRule::NotOwner`], and a tick after the last input sent to that peer, or a
    /// hash report, as none is sent yet, is [`ViolationRule::AckNeverSent`]. An older
    /// acknowledgment, arriving reordered, is ignored.
    fn read_ack(
        &mut self,
        index: usize,
        players: &[(u8, u64)],
        hashes_through: u64,
    ) -> Result<(), Refusal> {
        let remote = &self.remotes[index];
        for (player, next) in players {
            let violation = |rule| Refusal::Violation {
                player: Some(*player),
                tick: Some(*next),
                rule,
            };
            let player = usize::from(*player);
            if self.settings.identity.players.get(player) != Some(&self.settings.local) {
                return Err(violation(ViolationRule::NotOwner));
            }
            if *next > remote.sent[player] {
                return Err(violation(ViolationRule::AckNeverSent));
            }
        }
        if hashes_through != 0 {
            return Err(Refusal::Violation {
                player: None,
                tick: Some(hashes_through),
                rule: ViolationRule::AckNeverSent,
            });
        }
        let remote = &mut self.remotes[index];
        for (player, next) in players {
            let acked = &mut remote.acked[usize::from(*player)];
            *acked = (*acked).max(*next);
        }
        Ok(())
    }

    /// Checks that the newest acknowledgment of each local player from the remote at `index`
    /// trails the local horizon `h` by at most `2 * (rollback_window + input_delay) + 1` ticks.
    /// Beyond it is [`ViolationRule::AckLagging`].
    ///
    /// Each packet carries an `Ack` with the inputs, both written at the same time, when that
    /// remote is at tick `r` and acknowledges `a`. Its highest input is `r + input_delay - 1`,
    /// so this peer's confirmed tick is at most `r + input_delay`, its tick at most
    /// `rollback_window` more, and `h` at most `input_delay - 1` more:
    /// `h <= r + rollback_window + 2 * input_delay - 1`. That remote's tick is at most
    /// `rollback_window` past its confirmed tick, which is at most `a`. So an honest `Ack`
    /// trails `h` by at most `2 * (rollback_window + input_delay) - 1`, two ticks below the
    /// bound. Checked after every `Inputs` and `Ack`, so the local inputs kept for a peer that
    /// never acknowledges stay bounded too: a peer that sends nothing stalls this one, as it
    /// owns a player.
    fn check_lag(&self, index: usize) -> Result<(), Refusal> {
        let rules = self.settings.identity.rules;
        let bound = 2 * (u64::from(rules.rollback_window) + u64::from(rules.input_delay)) + 1;
        // `h - acked > bound`, in terms of `h + 1`, which never underflows.
        let after_horizon = self.after_horizon();
        let remote = &self.remotes[index];
        match self
            .local_players()
            .find(|player| after_horizon.saturating_sub(remote.acked[*player]) > bound + 1)
        {
            Some(player) => Err(Refusal::Violation {
                player: u8::try_from(player).ok(),
                tick: Some(remote.acked[player]),
                rule: ViolationRule::AckLagging,
            }),
            None => Ok(()),
        }
    }

    /// Drops the packet `from` sent, or ends the session on a violation of `from`.
    fn refuse(&mut self, now: u64, from: PeerId, refusal: Refusal) {
        match refusal {
            Refusal::Drop(reason) => self.drop_packet(from, reason),
            Refusal::Violation { player, tick, rule } => self.end(
                now,
                EndReason::Violation {
                    peer: from,
                    player,
                    tick,
                    rule,
                },
            ),
        }
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

    /// A running peer received this one's matching `Hello`, as it runs only once every peer
    /// sent one: its running messages welcome this peer, even when every `Welcome` it sent was
    /// lost.
    fn welcomed_by_running_peer(&mut self, index: usize) {
        if !self.remotes[index].welcomed {
            self.remotes[index].welcomed = true;
            self.start_when_agreed();
        }
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

    /// Counts one dropped packet of `peer` under `reason` and reports it. Called once per
    /// dropped packet, as [`PeerStats::dropped`] describes.
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

/// The `Ack` of `peer`'s players and the inputs of every local player from what `peer`
/// acknowledged on, at most [`MAX_TICKS_PER_MESSAGE`] ticks each.
fn inputs_for(
    settings: &SessionSettings,
    queues: &[InputQueue],
    peer: PeerId,
    acked: &[u64],
) -> Vec<Message> {
    let players = &settings.identity.players;
    let owned_by = |owner: PeerId| {
        players
            .iter()
            .zip(0u8..)
            .filter(move |(player_owner, _)| **player_owner == owner)
            .map(|(_, player)| player)
    };
    let mut messages = vec![Message::Ack {
        players: owned_by(peer)
            .map(|player| {
                let next = queues[usize::from(player)].next_confirmed();
                (player, next.unwrap_or(u64::MAX))
            })
            .collect(),
        hashes_through: 0,
    }];
    for player in owned_by(settings.local) {
        let queue = &queues[usize::from(player)];
        let first_tick = acked[usize::from(player)];
        let end = queue.next_confirmed().unwrap_or(u64::MAX);
        let count = end
            .saturating_sub(first_tick)
            .min(u64::from(MAX_TICKS_PER_MESSAGE));
        if count == 0 {
            continue;
        }
        // `count` is at most `end - first_tick`, so the sum never passes `end`, whatever the
        // peer acknowledged.
        let inputs = (first_tick..first_tick + count)
            .map(|tick| {
                // A local input is released only once every peer acknowledged it.
                queue
                    .input_for(tick)
                    .expect("unacknowledged local inputs are held")
            })
            .collect();
        messages.push(Message::Inputs {
            player,
            first_tick,
            inputs,
        });
    }
    messages
}

/// Encodes `packet`, giving it back with its bytes, leaving out the newest tick of its longest
/// input message until it fits in [`MAX_PACKET_BYTES`]: the oldest unacknowledged ticks go
/// first, the rest in a later packet. The settings bound the usual packet within the limit;
/// this only acts for a peer that has fallen behind by more than the rollback window and the
/// input delay.
fn fit(mut packet: Packet) -> (Packet, Vec<u8>) {
    loop {
        let bytes = packet.encode();
        if bytes.len() <= MAX_PACKET_BYTES {
            return (packet, bytes);
        }
        let longest = packet
            .messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| match message {
                Message::Inputs { inputs, .. } => Some((inputs.len(), index)),
                _ => None,
            })
            .max();
        match longest {
            Some((1, index)) => {
                packet.messages.remove(index);
            }
            Some((_, index)) => {
                if let Message::Inputs { inputs, .. } = &mut packet.messages[index] {
                    inputs.pop();
                }
            }
            // The handshake and the acknowledgment alone always fit: their lengths are
            // bounded by the settings.
            None => return (packet, bytes),
        }
    }
}
