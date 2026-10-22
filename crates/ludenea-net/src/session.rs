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
//!
//! Peers keep their ticks in step without a shared clock. Every [`QUALITY_INTERVAL_MICROS`],
//! a session sends each peer a `Quality` request with its tick and its frame advantage over
//! that peer; the peer answers with a `QualityReply` echoing the request's time, which gives
//! the ping. A session ahead of the others skips ticks, GGPO style: an advance that runs no
//! tick, so the others catch up without stalling, a few ticks at a time.
//!
//! Peers compare their states every `hash_interval` ticks. The `Save` of each such tick asks
//! the caller for the state hash, which it answers with [`Session::report_hash`]; once every
//! input below that tick is confirmed and the state was built from them, the session sends
//! the hash to every peer until acknowledged. When every peer's hash matches, the session
//! emits [`SessionEvent::HashMatched`] and releases the states kept for earlier reports. A
//! difference ends the session with [`EndReason::Desync`], and an ending a session decides
//! alone is told to the other peers through an end notice for [`END_LINGER_MICROS`], so they
//! end for the same reason.
//!
//! A peer silent for `notice_after_micros` raises [`SessionEvent::PeerSilent`], and
//! [`SessionEvent::PeerBack`] when it speaks again; silent for `disconnect_after_micros`, it
//! ends the match with [`EndReason::Lost`] for every peer. A peer that leaves on purpose calls
//! [`Session::leave`]: its `Goodbye` names the last tick it has its own inputs for, and every
//! peer, the leaver included, stops at that tick with the same confirmed inputs and ends with
//! [`EndReason::Left`]. Whatever the ending, the next [`Session::advance`] lands the caller's
//! simulation on the state the confirmed inputs give.

use std::collections::{BTreeMap, VecDeque};

use ludenea_app::PlayerInputSpec;

use crate::queue::{Confirmed, InputQueue, QueueError};
use crate::settings::{
    MAX_INPUT_BYTES, MAX_PACKET_BYTES, MAX_TICKS_PER_MESSAGE, MatchIdentity, PeerId,
    SessionSettings, SettingsError,
};
use crate::wire::{EndNotice, Message, Packet, WireError};

/// The least time between two `Hello`s to the same peer while it has not welcomed this one.
pub const HANDSHAKE_RESEND_MICROS: u64 = 100_000;

/// How long a session that ended keeps telling the other peers why, so a peer that missed the
/// cause ends for the same reason rather than by a timeout. After an ending it decided alone
/// that an end notice tells of, it sends that notice to every peer, at most once a tick. After
/// a mismatch while connecting, it keeps sending its `Hello`, from which every other peer
/// finds the mismatch itself.
pub const END_LINGER_MICROS: u64 = 500_000;

/// The time between two `Quality` requests to the same peer while the session runs.
pub const QUALITY_INTERVAL_MICROS: u64 = 200_000;

/// How many unanswered `Quality` requests a session keeps per peer, the oldest forgotten
/// first, and how many of the latest round trips the ping and the jitter are measured over.
pub const QUALITY_HISTORY: usize = 16;

/// How many fresh reports of a peer a time correction needs, and is the mean of.
pub const SYNC_MIN_REPORTS: usize = 4;

/// Where a session is in the life of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Greeting the other peers.
    Connecting,
    /// Every peer agreed on the match.
    Running,
    /// [`Session::leave`] was called: the session runs up to the last tick it has its own
    /// inputs for and takes no more local input. It ends once every player's inputs through
    /// that tick are confirmed and every peer acknowledged the local ones.
    Leaving,
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
    /// The state hashes of `tick` differ: `local` is this peer's, `remote` that of `peer`. A
    /// peer that learned it from another's end notice holds the view of the peer that found
    /// it, as the notice tells: `local` is the hash of the peer that found it, and `peer`,
    /// with its hash `remote`, the peer whose hash differed from it, which may be this one.
    Desync {
        tick: u64,
        peer: PeerId,
        local: u64,
        remote: u64,
    },
    /// No packet came from `peer` for `disconnect_after_micros` while the match ran, here or,
    /// as its end notice told, on another peer.
    Lost { peer: PeerId },
    /// The match ended on purpose at `last_tick`: `peer` left, and every player's inputs
    /// through `last_tick` are confirmed, on this peer and on every other one, unless a peer
    /// stopped answering first: the leaver ends anyway `disconnect_after_micros` after
    /// [`Session::leave`], and a remaining peer whose confirmed inputs reach `last_tick` ends
    /// when a peer is silent that long. When several peers leave, the lowest `last_tick` wins,
    /// and `peer` is the lowest peer naming it, the same on every peer that heard them all.
    Left { peer: PeerId, last_tick: u64 },
    /// A game failed while stepping `tick`, as the end notice of the peer it ran on told.
    LocalFailure { tick: u64 },
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
    /// An input, or a goodbye's last tick, past the furthest an honest peer can have reached.
    BeyondHorizon,
    /// An acknowledgment of an input or a hash report this peer never sent.
    AckNeverSent,
    /// An acknowledgment trailing this peer's inputs by more than an honest peer can.
    AckLagging,
    /// A hash report of a tick past the local horizon plus one, which no honest peer can have
    /// confirmed: its confirmed tick needs the local inputs.
    HashBeyondHorizon,
    /// Hash reports, or acknowledgments of the local ones, trailing the newest final local
    /// hash by more than an honest peer can (see [`Session`]'s hash lag bounds). They bound
    /// the local reports a session keeps and the states its caller keeps for them.
    HashLagging,
    /// An end notice no honest peer can send: one naming a peer outside the match, a peer lost
    /// that is its own sender, a desync of a tick past the local horizon plus one, or a game
    /// failure past the furthest tick an honest peer steps.
    ImplausibleEnd,
    /// A goodbye whose last tick is below the last tick this session confirmed: the sender
    /// sent an input past it, and an honest leaver's last tick is the last input it has.
    GoodbyeBehind,
}

/// Why a received packet, or one message of it, was dropped. Ordered, so the counts of
/// [`PeerStats::dropped`] iterate the same way everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DropReason {
    /// The bytes are not a packet of this build, named after the [`WireError`] variant.
    Wire(&'static str),
    /// The packet belongs to another match.
    WrongMatch,
    /// The sender is not another peer of this match.
    UnknownPeer,
    /// The packet holds more than the handshake while this session is not running yet, other
    /// than inputs, acknowledgments and quality messages from a peer whose matching `Hello`
    /// arrived. Its handshake messages are still read.
    BeforeRunning,
    /// The inputs start after the next tick expected from that player, within the horizon.
    /// Not a violation: a sender resends from the tick acknowledged last, so a later packet
    /// fills the gap.
    Gap,
    /// A `QualityReply` echoes no `Quality` request still pending: answered already (a
    /// duplicate), forgotten, or never sent. Not a violation: the network duplicates packets.
    /// Only that message is dropped: the rest of its packet is read.
    UnknownEcho,
    /// A hash report of tick 0 or of a tick that is not a multiple of `hash_interval`: no peer
    /// reports one. Only that message is dropped.
    HashOffInterval,
    /// A hash report of a tick whose report from that peer arrived already with another hash.
    /// The first one is kept. A resend of the same hash, which a peer sends until
    /// acknowledged, is ignored without a drop. Only that message is dropped.
    HashAlreadyReceived,
    /// A hash report that finds the reports of that peer waiting for a local hash at their
    /// bound, `(rollback_window + input_delay) / hash_interval + 2`. Not acknowledged, so the
    /// peer sends it again. Only that message is dropped.
    HashBacklog,
    /// A hash report of a tick after one of that peer's reports still missing. A peer sends
    /// every report not acknowledged, oldest first, so an honest one sends this one only after
    /// a report dropped as [`DropReason::HashBacklog`] and resends both. Not acknowledged, and
    /// a peer that never sends the missing one ends up [`ViolationRule::HashLagging`]. Only
    /// that message is dropped.
    HashGap,
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
    /// Every peer's hash of the state of `tick` matched the local one. In tick order.
    HashMatched { tick: u64 },
    /// No packet came from `peer` for `for_micros`, at least `notice_after_micros`, while the
    /// match runs. Once per silence.
    PeerSilent { peer: PeerId, for_micros: u64 },
    /// `peer`, silent since its [`SessionEvent::PeerSilent`], spoke again.
    PeerBack { peer: PeerId },
}

/// What the caller's simulation does for the session, in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Save the state of `tick`, before its inputs. It comes right before the first
    /// `Advance { tick }` after it, and a `Load { tick }` loads the latest one. `hash` is
    /// `true` for every tick that is a positive multiple of `hash_interval`: the caller answers
    /// with [`Session::report_hash`], and keeps the state it saved last for that tick until
    /// `Release { tick }`.
    Save { tick: u64, hash: bool },
    /// Load the state of `tick` saved last. Always a tick at most `rollback_window` before the
    /// current one, so a caller keeping the states of the last `rollback_window + 1` ticks it
    /// saved holds it, for a rollback as for the landing of an ended session.
    Load { tick: u64 },
    /// Step the state of `tick` to `tick + 1` with `inputs`, one per player, in player order.
    Advance { tick: u64, inputs: Vec<Vec<u8>> },
    /// The state kept for `tick`, saved with `hash: true`, is no longer needed: a later report
    /// matched on every peer. No `Load { tick }` follows.
    Release { tick: u64 },
}

/// Why a call on a [`Session`] was refused. Nothing changed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The session is connecting, leaving (for an input or a second leave), or has ended
    /// (once its simulation landed, for an advance).
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
    /// A hash reported for `tick` answers no `Save { tick, hash: true }` still unanswered.
    #[error("no Save {{ tick: {tick}, hash: true }} waits for a hash")]
    HashNotAsked { tick: u64 },
}

/// What a session counts about one sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStats {
    /// The drops, by reason. A [`DropReason::UnknownEcho`] counts each dropped reply, and each
    /// of `HashOffInterval`, `HashAlreadyReceived`, `HashBacklog` and `HashGap` each dropped
    /// hash report: the rest of their packet is still read. Every other reason counts a dropped
    /// packet once, under the reason that stopped reading it: an `Inputs` message with a gap
    /// stops the packet there and is the reason, even when an earlier message of the same
    /// packet came before the session ran; otherwise such a message makes the reason
    /// [`DropReason::BeforeRunning`].
    pub dropped: BTreeMap<DropReason, u64>,
    /// The rollbacks a misprediction of this peer's inputs caused.
    pub rollbacks: u64,
    /// The most ticks one of those rollbacks ran again.
    pub deepest_rollback: u64,
    /// The inputs of this peer confirmed different from the input their tick ran with.
    pub prediction_misses: u64,
    /// The advances that stalled waiting for this peer's inputs.
    pub stalls: u64,
    /// The mean of the latest [`QUALITY_HISTORY`] round trips to this peer, `0` before the
    /// first.
    pub ping_micros: u64,
    /// The mean absolute deviation of those round trips from `ping_micros`.
    pub jitter_micros: u64,
    /// How many ticks this session is ahead of this peer, negative when behind, as of the
    /// peer's latest report: the local tick minus the peer's estimated tick.
    pub frame_advantage: i32,
}

/// What a session counts, per sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// Every other peer of the match from the start, and any sender the transport named that
    /// is not one, from its first packet.
    pub peers: BTreeMap<PeerId, PeerStats>,
    /// The advances that skipped a tick so the other peers catch up, apart from the stalls.
    pub skips: u64,
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

/// The latest time report of a peer: a `Quality` request it sent.
#[derive(Debug, Clone, Copy)]
struct Report {
    /// The peer's tick when it sent the report.
    tick: u64,
    /// When the report arrived, on the local clock.
    received_micros: u64,
}

/// A state hash the caller was asked for.
#[derive(Debug, Default)]
struct Asked {
    /// The `Save { hash: true }` of the tick not answered yet.
    unanswered: u32,
    /// The hash of the latest answer.
    hash: Option<u64>,
}

/// The local hash of a tick whose state was built from confirmed inputs.
#[derive(Debug)]
struct HashReport {
    hash: u64,
    /// Per remote, in the order of [`Session::remotes`], the hash it reported for the tick.
    received: Vec<Option<u64>>,
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
    /// When the last `Quality` request was sent to the peer.
    last_quality_micros: Option<u64>,
    /// The times of the `Quality` requests the peer has not answered, oldest first, at most
    /// [`QUALITY_HISTORY`].
    pending_quality: VecDeque<u64>,
    /// The latest round trips to the peer, oldest first, at most [`QUALITY_HISTORY`].
    round_trips: VecDeque<u64>,
    /// The `sent_micros` of the latest `Quality` request of the peer, on its clock.
    last_request_micros: Option<u64>,
    /// The next packet answers that request.
    reply_due: bool,
    /// The latest report counted.
    report: Option<Report>,
    /// A [`SessionEvent::PeerSilent`] told of the peer's current silence.
    silent: bool,
    /// The hash reports of the peer that wait for the local hash of their tick, at most
    /// [`Session::hash_backlog`].
    hashes: BTreeMap<u64, u64>,
    /// The highest tick of a hash report received from the peer, `0` for none: what the `Ack`
    /// to it acknowledges.
    hashes_received: u64,
    /// The highest tick of a local hash report the peer acknowledged, `0` for none.
    hashes_acked: u64,
    /// The highest tick of a local hash report sent to the peer, `0` for none.
    hashes_sent: u64,
    /// Twice the corrections of the latest fresh reports, at most [`SYNC_MIN_REPORTS`]: each
    /// `local_advantage - remote_advantage`, the ticks this session should skip by the report
    /// being half of it. Kept doubled so their mean is rounded once, not each of them. Cleared
    /// by every skip, so no report counted before a skip counts after it. The reports after a
    /// skip still lag it: the peer's `remote_advantage` reflects the state before the skip
    /// until the peer has heard of it, about one [`QUALITY_INTERVAL_MICROS`] plus a round trip.
    /// That lag is inherent to GGPO-style synchronization; ignoring the reports for that long
    /// leaves the drift uncorrected for too long and was measured to break the gap the
    /// synchronization keeps.
    corrections: VecDeque<i64>,
}

/// The agreed last tick of a match ending on purpose.
#[derive(Debug, Clone, Copy)]
struct Closing {
    last_tick: u64,
    /// The lowest peer heard naming `last_tick`.

    peer: PeerId,
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
    /// The time correction in progress: the index of the remote it skips toward, and the
    /// skips still to run, at least one.
    skipping: Option<(usize, u64)>,
    /// The next tick to emit [`SessionEvent::Confirmed`] for.
    next_event: u64,
    state: SessionState,
    /// When the session ended.
    ended_micros: Option<u64>,
    /// Every other peer, in id order.
    remotes: Vec<Remote>,
    /// The hashes asked of the caller, by tick, until the tick's hash is final and every save
    /// of it answered.
    asked: BTreeMap<u64, Asked>,
    /// The final local hashes, by tick: sent to every peer until acknowledged, compared with
    /// every peer's, and forgotten once matched and acknowledged by every peer.
    reports: BTreeMap<u64, HashReport>,
    /// The highest tick whose local hash is final, `0` for none.
    final_through: u64,
    /// The highest tick whose hash matched on every peer, `0` for none.
    matched_through: u64,
    /// The lowest tick whose state the caller keeps for a report and that is not released.
    kept_from: u64,
    /// The `Release`s the next advance returns.
    releases: Vec<u64>,
    /// The notice of an ending this session decided alone, sent while it lingers.
    end_notice: Option<EndNotice>,
    /// When the end notice, or the farewell of a session that left, was sent last.
    last_notice_micros: Option<u64>,
    /// The lowest last tick of every goodbye, the local one included: no tick past it is
    /// confirmed, and no tick past the tick after it runs.
    closing: Option<Closing>,
    /// The local goodbye's last tick, and when [`Session::leave`] was called.
    leaving: Option<(u64, u64)>,
    /// The session ended while running or leaving, and the advance that lands the caller's
    /// simulation on the confirmed state has not been returned yet.
    land_due: bool,
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
                last_quality_micros: None,
                pending_quality: VecDeque::new(),
                round_trips: VecDeque::new(),
                last_request_micros: None,
                reply_due: false,
                report: None,
                silent: false,
                corrections: VecDeque::new(),
                hashes: BTreeMap::new(),
                hashes_received: 0,
                hashes_acked: 0,
                hashes_sent: 0,
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
            skips: 0,
        };
        let pending = vec![None; settings.identity.players.len()];
        let kept_from = u64::from(settings.identity.rules.hash_interval);
        Ok(Self {
            settings,
            input,
            queues,
            pending,
            tick: 0,
            skipping: None,
            next_event: 0,
            state: SessionState::Connecting,
            ended_micros: None,
            remotes,
            asked: BTreeMap::new(),
            reports: BTreeMap::new(),
            final_through: 0,
            matched_through: 0,
            kept_from,
            releases: Vec::new(),
            end_notice: None,
            last_notice_micros: None,
            closing: None,
            leaving: None,
            land_due: false,

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
        let remote = &mut self.remotes[index];
        remote.last_heard_micros = now;
        if remote.silent {
            remote.silent = false;
            self.events.push(SessionEvent::PeerBack { peer: from });
        }
        let mut before_running = false;
        for message in packet.messages {
            match message {
                Message::Hello { identity } => {
                    // A different identity ends the session in any state. While connecting,
                    // the other peer finds the mismatch from the lingering `Hello`. Once
                    // running, the peer that changed its identity is the one breaking the
                    // protocol, and no end notice tells of a mismatch: the other peers find
                    // the change from that peer's own `Hello`, or stop hearing from this one.
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
                Message::Inputs { .. }
                | Message::Ack { .. }
                | Message::Quality { .. }
                | Message::QualityReply { .. }
                | Message::End { .. }
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
                Message::Quality {
                    tick,
                    sent_micros,
                    advantage,
                } => {
                    self.welcomed_by_running_peer(index);
                    // A session still waiting for another peer has no tick to report yet: it
                    // answers the requests once it runs.
                    if self.active() {
                        self.read_quality(index, tick, sent_micros, advantage, now);
                    }
                }
                // The replier's tick is not read: its requests carry its tick with its view of
                // the advantage, which the correction needs both of.
                // A reply answering no request is dropped alone: the rest of the packet is read.
                Message::QualityReply { echo_micros, .. } => {
                    self.welcomed_by_running_peer(index);
                    if !self.read_reply(index, echo_micros, now) {
                        self.drop_packet(from, DropReason::UnknownEcho);
                    }
                }
                Message::End { reason } => {
                    match self.check_notice(from, reason) {
                        Ok(()) => self.end_by_notice(now, reason),
                        Err(refusal) => self.refuse(now, from, refusal),
                    }
                    return;
                }
                _ if self.state == SessionState::Connecting => before_running = true,
                // A report dropped alone leaves the rest of the packet to read.
                Message::Hash { tick, hash } => match self.read_hash(index, tick, hash) {
                    Ok(()) => {}
                    Err(Refusal::Drop(reason)) => self.drop_packet(from, reason),
                    Err(violation) => {
                        self.refuse(now, from, violation);
                        return;
                    }
                },
                Message::Goodbye { last_tick } => {
                    if let Err(refusal) = self.read_goodbye(from, last_tick) {
                        self.refuse(now, from, refusal);
                        return;
                    }
                }
            }
        }
        if before_running {
            self.drop_packet(from, DropReason::BeforeRunning);
        }
        // Confirmed inputs or a peer's report may complete a comparison, and inputs or
        // acknowledgments the end of a match on purpose.
        self.check_hashes(now);
        self.check_done(now);
    }

    /// Leaves the match on purpose. Its last tick is the last tick the local players have
    /// inputs for, the local input horizon: no peer confirms a tick past it. The session goes
    /// [`SessionState::Leaving`]: it takes no more local input, and the caller keeps calling
    /// [`Session::advance`] and [`Session::outgoing`] each frame. It runs up to that tick,
    /// and every packet tells the other peers `Goodbye` with the inputs they have not
    /// acknowledged. It ends with [`EndReason::Left`] once every player's inputs through that
    /// tick are confirmed and every peer acknowledged the local ones, or
    /// `disconnect_after_micros` after this call, counted from the latest time the session saw.
    /// The pending local inputs are dropped.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotRunning`] unless the session runs. Nothing changed.
    pub fn leave(&mut self) -> Result<(), SessionError> {
        if self.state != SessionState::Running {
            return Err(SessionError::NotRunning);
        }
        if self.after_horizon() == 0 {
            // With no input delay, a session that ran no tick has no input of its own yet: its
            // last one is the default input of tick 0, as for the ticks an input delay covers.
            let local: Vec<usize> = self.local_players().collect();
            for player in local {
                let confirmed = self.queues[player].confirm(0, self.input.default.clone());
                debug_assert_eq!(confirmed, Ok(Confirmed::New));
            }
        }
        let last_tick = self.after_horizon().saturating_sub(1);
        let now = self.now_micros.unwrap_or(0);
        self.pending.fill(None);
        self.leaving = Some((last_tick, now));
        self.close(last_tick, self.settings.local);
        self.state = SessionState::Leaving;
        self.check_done(now);
        Ok(())
    }

    /// Answers the `Save { tick, hash: true }` executed first among those not answered yet
    /// with the hash of the state it saved. An ended session ignores the answer. The caller
    /// answers every hash an advance asks for before the next [`Session::outgoing`]: the lag
    /// bounds of [`ViolationRule::HashLagging`] rely on it, on every peer, and a session whose
    /// own answers come late may find an honest peer lagging. Reports waiting for a late answer
    /// are bounded by [`DropReason::HashBacklog`].
    ///
    /// # Errors
    ///
    /// [`SessionError::HashNotAsked`] when no `Save` of `tick` waits for a hash. Nothing
    /// changed.
    pub fn report_hash(&mut self, tick: u64, hash: u64) -> Result<(), SessionError> {
        if matches!(self.state, SessionState::Ended(_)) {
            return Ok(());
        }
        let Some(asked) = self
            .asked
            .get_mut(&tick)
            .filter(|asked| asked.unanswered > 0)
        else {
            return Err(SessionError::HashNotAsked { tick });
        };
        asked.unanswered -= 1;
        if tick > self.final_through {
            asked.hash = Some(hash);
        } else if asked.unanswered == 0 {
            // A final hash never changes: a rollback that saves its tick again loads that
            // tick's own state, built from confirmed inputs.
            self.asked.remove(&tick);
        }
        let now = self.now_micros.unwrap_or(0);
        self.check_hashes(now);
        Ok(())
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

    /// Runs the session one frame, while it runs or leaves: first the rollback a misprediction
    /// calls for, a `Load` of the first mispredicted tick then a `Save` and an `Advance` of
    /// every tick from it to the current one; then at most one new tick, taking every local
    /// player's pending input, or a stall when the current tick is `rollback_window` ticks past
    /// the confirmed tick. Every call, stalled or not, has the next packet to each peer carry
    /// the local inputs it has not acknowledged and an `Ack` of its players' inputs.
    ///
    /// Once a goodbye set the last tick of the match, no tick past the tick after it runs, and
    /// no local input is taken for a tick past it. A leaving session takes none.
    ///
    /// Once the session ended, after it ran, the next call, or the call that ended it, lands
    /// the simulation on the state of the end tick built only from confirmed inputs: a `Load`
    /// of the latest tick whose state was built only from them, unless it is the current one,
    /// then a `Save` and an `Advance` with the confirmed inputs of every tick from it to the
    /// end tick. The end tick is the confirmed tick when the session ended, the first tick some
    /// player's input is missing for, and at most the tick after the agreed last tick: for
    /// [`EndReason::Left`], the tick after `last_tick` unless a timeout ended it. The
    /// [`SessionEvent::Confirmed`] of every tick below it are emitted first. Later calls are
    /// [`SessionError::NotRunning`].
    ///
    /// When the session is ahead of the other peers, a call that would run a tick skips it
    /// instead, keeping the pending local inputs: when the mean correction of the latest
    /// [`SYNC_MIN_REPORTS`] fresh reports of some peer, rounded to the nearest tick, is at
    /// least one tick, the session skips that many ticks for the largest one, at most
    /// `rollback_window / 2`, one per call, each counted in [`SessionStats::skips`]. Only a
    /// peer this session is ahead of right now counts, and the skips stop once it is no longer
    /// ahead of it. Each skip, and each stop, forgets the reports so far.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotRunning`], [`SessionError::MissingLocalInput`] when the tick would
    /// advance and takes a local input that a local player has not added, and
    /// [`SessionError::LastTick`]. A refused call changes nothing.
    pub fn advance(&mut self, now_micros: u64) -> Result<Vec<Request>, SessionError> {
        let now = self.observe(now_micros);
        if self.land_due {
            return Ok(self.land());
        }
        if !self.active() {
            return Err(SessionError::NotRunning);
        }
        let confirmed = self.confirmed_tick();
        let window = u64::from(self.settings.identity.rules.rollback_window);
        let at_end = self
            .closing
            .is_some_and(|closing| self.tick > closing.last_tick);
        let stall = !at_end && self.tick.saturating_sub(confirmed) >= window;
        // A correction in progress goes on only while this session is still ahead of the peer
        // it skips toward: a correction from a stale view of the peer could otherwise skip this
        // session behind it. A cancelled correction starts no other in the same call.
        let planned = match self.skipping {
            _ if stall || at_end => None,
            Some((toward, left)) if self.ahead_of(toward, now) => Some((toward, left)),
            Some(_) => None,
            None => self.correction(now),
        };
        let cancelled = !stall && !at_end && self.skipping.is_some() && planned.is_none();
        let skip = planned.is_some();
        let takes_input = self.takes_local_input();
        // Every refusal comes before anything changes.
        if !stall && !skip && !at_end {
            let missing = self.local_players().find(|p| self.pending[*p].is_none());
            if let Some(player) = missing.filter(|_| takes_input) {
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
        if cancelled {
            self.skipping = None;
            self.forget_reports();
        }
        if at_end {
            // The match runs no tick past the agreed end.
        } else if stall {
            self.count_stall(confirmed);
        } else if let Some((toward, left)) = planned {
            self.skipping = (left > 1).then(|| (toward, left - 1));
            self.stats.skips = self.stats.skips.saturating_add(1);
            self.forget_reports();
        } else {
            self.advance_tick(&mut requests, takes_input);
        }
        for remote in &mut self.remotes {
            remote.inputs_due = true;
        }
        self.emit_confirmed();
        self.release();
        self.check_hashes(now);
        self.check_done(now);
        requests.extend(
            self.releases
                .drain(..)
                .map(|tick| Request::Release { tick }),
        );
        if self.land_due {
            requests.extend(self.land());
        }
        Ok(requests)
    }

    /// The packets to send now, at most one per peer. While connecting, a session that heard
    /// nothing for `disconnect_after_micros` from a peer whose handshake is unfinished ends
    /// `Unreachable`. While running or leaving, it raises the silences and ends on them, as
    /// [`SessionEvent::PeerSilent`], [`EndReason::Lost`] and [`EndReason::Left`] tell. While
    /// running, the packet after an advance carries the local inputs the
    /// peer has not acknowledged, oldest first, and an `Ack` of the peer's players; when they
    /// do not all fit in [`MAX_PACKET_BYTES`], the newest ticks wait for a later packet. A
    /// running session also sends each peer a `Quality` request every
    /// [`QUALITY_INTERVAL_MICROS`], and answers the peer's latest request once. Every packet of
    /// a running session carries every local hash report the peer has not acknowledged,
    /// oldest first, which the settings leave room for. A leaving session sends the same, and
    /// its `Goodbye` in every packet. A session that ended for a reason an end notice tells
    /// of, decided alone, sends every peer that notice and nothing else, at most once a tick,
    /// for [`END_LINGER_MICROS`]. One that ended with [`EndReason::Left`] sends every peer an
    /// `Ack`, and its `Goodbye` if it left, the same way: a peer whose last packets from it were
    /// lost still learns that it holds that peer's inputs, and that the match ends.
    pub fn outgoing(&mut self, now_micros: u64) -> Vec<(PeerId, Vec<u8>)> {
        let now = self.observe(now_micros);
        if self.active() {
            self.check_silence(now);
        }
        let lingering = self
            .ended_micros
            .is_some_and(|ended| now.saturating_sub(ended) < END_LINGER_MICROS);
        let tick_micros = self.tick_micros();
        let linger_due = lingering
            && self
                .last_notice_micros
                .is_none_or(|sent| now.saturating_sub(sent) >= tick_micros);
        let notice = self.end_notice.filter(|_| linger_due);
        let farewell =
            linger_due && matches!(self.state, SessionState::Ended(EndReason::Left { .. }));
        if notice.is_some() || farewell {
            self.last_notice_micros = Some(now);
        }
        let greeting = match &self.state {
            SessionState::Connecting => {
                if let Some(peer) = self.silent_peer(now) {
                    self.end(now, EndReason::Unreachable { peer });
                    return Vec::new();
                }
                true
            }
            SessionState::Running | SessionState::Leaving => false,
            SessionState::Ended(EndReason::Mismatch { .. }) => lingering,
            SessionState::Ended(_) => false,
        };
        let running = !matches!(self.state, SessionState::Ended(_));
        let active = self.active();
        let goodbye = self
            .leaving
            .map(|(last_tick, _)| Message::Goodbye { last_tick });
        // Before its first report and round trip from a peer, the session sends an advantage of
        // 0, which the peer cannot tell from a measured 0: the format is frozen. The bias is
        // negligible: it lasts one round trip at the start of a match, and the peer's
        // correction averages it with later reports.
        let advantages: Vec<i32> = (0..self.remotes.len())
            .map(|index| self.advantage(index, now).unwrap_or(0))
            .collect();
        let mut packets = Vec::new();
        for (remote, advantage) in self.remotes.iter_mut().zip(advantages) {
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
            if active && remote.inputs_due {
                remote.inputs_due = false;
                messages.extend(inputs_for(
                    &self.settings,
                    &self.queues,
                    remote.id,
                    &remote.acked,
                    remote.hashes_received,
                ));
            }
            if active {
                let quality_due = remote
                    .last_quality_micros
                    .is_none_or(|sent| now.saturating_sub(sent) >= QUALITY_INTERVAL_MICROS);
                if quality_due {
                    messages.push(Message::Quality {
                        tick: self.tick,
                        sent_micros: now,
                        advantage,
                    });
                    remote.last_quality_micros = Some(now);
                    push_bounded(&mut remote.pending_quality, now, QUALITY_HISTORY);
                }
                if let Some(echo_micros) = remote.last_request_micros.filter(|_| remote.reply_due) {
                    messages.push(Message::QualityReply {
                        echo_micros,
                        tick: self.tick,
                    });
                    remote.reply_due = false;
                }
                // Reports ride on the packets sent anyway, so they never make one of their own.
                // All of them: at most `MatchRules::unacknowledged_hash_reports`, which the
                // settings leave room for, as more breaks `ViolationRule::HashLagging`.
                if !messages.is_empty() {
                    let unacknowledged =
                        self.reports.range(remote.hashes_acked.saturating_add(1)..);
                    messages.extend(unacknowledged.map(|(tick, report)| Message::Hash {
                        tick: *tick,
                        hash: report.hash,
                    }));
                    messages.extend(goodbye.clone());
                }
            }
            if farewell {
                messages.push(ack_for(
                    &self.settings,
                    &self.queues,
                    remote.id,
                    remote.hashes_received,
                ));
                messages.extend(goodbye.clone());
            }

            if let Some(reason) = notice {
                messages.push(Message::End { reason });
            }
            if !messages.is_empty() {
                let packet = Packet {
                    match_id: self.settings.match_id,
                    messages,
                };
                let (packet, bytes) = fit(packet);
                for message in &packet.messages {
                    match message {
                        Message::Inputs {
                            player,
                            first_tick,
                            inputs,
                        } => {
                            // A local input held is below `u64::MAX`, the next confirmed tick.
                            let end = u64::try_from(inputs.len())
                                .ok()
                                .and_then(|count| first_tick.checked_add(count))
                                .unwrap_or(u64::MAX);
                            let sent = &mut remote.sent[usize::from(*player)];
                            *sent = (*sent).max(end);
                        }
                        Message::Hash { tick, .. } => {
                            remote.hashes_sent = remote.hashes_sent.max(*tick);
                        }
                        _ => {}
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
    /// last tick is confirmed. At most the tick after the agreed last tick: no tick past it is
    /// confirmed.
    fn confirmed_tick(&self) -> u64 {
        let confirmed = self
            .queues
            .iter()
            .map(|queue| queue.next_confirmed().unwrap_or(u64::MAX))
            .min()
            .unwrap_or(u64::MAX);
        self.closing.map_or(confirmed, |closing| {
            confirmed.min(closing.last_tick.saturating_add(1))
        })
    }

    /// Whether the session runs or leaves.
    fn active(&self) -> bool {
        matches!(self.state, SessionState::Running | SessionState::Leaving)
    }

    /// Whether advancing the current tick takes the local players' inputs, for the tick
    /// `input_delay` later: only while running, and not for a tick past the agreed last tick.
    fn takes_local_input(&self) -> bool {
        let delay = u64::from(self.settings.identity.rules.input_delay);
        let target = self.tick.saturating_add(delay);
        self.state == SessionState::Running
            && self
                .closing
                .is_none_or(|closing| target <= closing.last_tick)
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
            requests.push(self.save(tick));
            requests.push(Request::Advance { tick, inputs });
        }
    }

    /// The `Save` of `tick`, asking for the hash of every positive multiple of
    /// `hash_interval`, which then waits for its answer.
    fn save(&mut self, tick: u64) -> Request {
        let interval = u64::from(self.settings.identity.rules.hash_interval);
        let hash = tick > 0 && tick.is_multiple_of(interval);
        if hash {
            let asked = self.asked.entry(tick).or_default();
            asked.unanswered = asked.unanswered.saturating_add(1);
        }
        Request::Save { tick, hash }
    }

    /// Runs the current tick with the prediction, and, when it `takes_input`, confirms every
    /// local player's pending input for the tick `input_delay` later. The caller checked that
    /// every local player has one then and that both ticks exist.
    fn advance_tick(&mut self, requests: &mut Vec<Request>, takes_input: bool) {
        let tick = self.tick;
        let target = tick + u64::from(self.settings.identity.rules.input_delay);
        let local: Vec<usize> = self.local_players().collect();
        for player in local.into_iter().filter(|_| takes_input) {
            let input = self.pending[player]
                .take()
                .expect("the caller checked every local player has an input");
            // Local inputs are confirmed in order: the queue expects exactly `target`.
            let confirmed = self.queues[player].confirm(target, input);
            debug_assert_eq!(confirmed, Ok(Confirmed::New));
        }
        let inputs = self.inputs_at(tick);
        self.record(tick, &inputs);
        requests.push(self.save(tick));
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
    /// Inputs past the agreed last tick are ignored.
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
                let last = self.closing.map_or(u64::MAX, |closing| closing.last_tick);
                for (tick, input) in ticks.take_while(|(tick, _)| *tick <= last) {
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

    /// Moves forward what the remote at `index` acknowledged of each local player and of the
    /// local hash reports. Every entry is checked before any is used: a player this peer does
    /// not own is [`ViolationRule::NotOwner`], and a tick after the last input sent to that
    /// peer, or the tick of no hash report sent to it, is [`ViolationRule::AckNeverSent`]. The
    /// reports sent to a peer are every multiple of `hash_interval` up to the highest sent, as
    /// they are sent oldest first. An older acknowledgment, arriving reordered, is ignored.
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
        let interval = u64::from(self.settings.identity.rules.hash_interval);
        if !hashes_through.is_multiple_of(interval) || hashes_through > remote.hashes_sent {
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
        remote.hashes_acked = remote.hashes_acked.max(hashes_through);
        Ok(())
    }

    /// The most hash reports of one peer kept waiting for the local hash of their tick:
    /// `(rollback_window + input_delay) / hash_interval + 2`. A peer reports a tick once its
    /// confirmed tick reaches it, at most the local horizon plus one, and the local hash is
    /// final from the advance after the local confirmed tick reaches it, at most
    /// `rollback_window` behind the current tick: the honest reports waiting span
    /// `rollback_window + input_delay` ticks, plus one report for the rounding and one for the
    /// advance still to run.
    fn hash_backlog(&self) -> usize {
        let rules = self.settings.identity.rules;
        let ticks = u64::from(rules.rollback_window) + u64::from(rules.input_delay);
        let reports = ticks / u64::from(rules.hash_interval) + 2;
        usize::try_from(reports).unwrap_or(usize::MAX)
    }

    /// Reads the hash report of `tick` from the remote at `index`. A tick past the local
    /// horizon plus one is [`ViolationRule::HashBeyondHorizon`], checked first, so any tick
    /// past it is a violation; tick 0 or a tick off the interval is dropped as
    /// [`DropReason::HashOffInterval`], a report of a tick at most the latest received from that
    /// peer is ignored when it holds the same hash, or when the first one is forgotten,
    /// and dropped as [`DropReason::HashAlreadyReceived`] otherwise, one after a missing report as
    /// [`DropReason::HashGap`], and one beyond [`Self::hash_backlog`] as
    /// [`DropReason::HashBacklog`].
    fn read_hash(&mut self, index: usize, tick: u64, hash: u64) -> Result<(), Refusal> {
        if tick > self.after_horizon() {
            return Err(Refusal::Violation {
                player: None,
                tick: Some(tick),
                rule: ViolationRule::HashBeyondHorizon,
            });
        }
        let interval = u64::from(self.settings.identity.rules.hash_interval);
        if tick == 0 || !tick.is_multiple_of(interval) {
            return Err(Refusal::Drop(DropReason::HashOffInterval));
        }
        if tick <= self.remotes[index].hashes_received {
            // A resend is ignored. A report forgotten since was matched: nothing to compare.
            let known = self
                .reports
                .get(&tick)
                .and_then(|report| report.received[index])
                .or_else(|| self.remotes[index].hashes.get(&tick).copied());
            return match known {
                Some(known) if known != hash => Err(Refusal::Drop(DropReason::HashAlreadyReceived)),
                _ => Ok(()),
            };
        }
        // Received in tick order: `hashes_received` is the newest of an unbroken run.
        if tick > self.remotes[index].hashes_received.saturating_add(interval) {
            return Err(Refusal::Drop(DropReason::HashGap));
        }
        if let Some(report) = self.reports.get_mut(&tick) {
            report.received[index] = Some(hash);
        } else {
            // No local report: the tick is above every final local hash, as a report is
            // forgotten only once every peer's report of its tick or a later one arrived.
            if self.remotes[index].hashes.len() >= self.hash_backlog() {
                return Err(Refusal::Drop(DropReason::HashBacklog));
            }
            self.remotes[index].hashes.insert(tick, hash);
        }
        self.remotes[index].hashes_received = tick;
        Ok(())
    }

    /// Makes final, in tick order, the local hash of each asked tick whose state was built only
    /// from confirmed inputs: every input below it confirmed, none of them used differently
    /// than confirmed (the rollback a correction calls for has run), and every save of it
    /// answered. The reports of that tick waiting from the peers join it.
    fn finalize_hashes(&mut self) {
        let confirmed = self.confirmed_tick();
        let mispredicted = self
            .queues
            .iter()
            .filter_map(|queue| queue.first_misprediction(self.tick))
            .min();
        loop {
            let Some((&tick, asked)) = self
                .asked
                .range(self.final_through.saturating_add(1)..)
                .next()
            else {
                return;
            };
            let built = tick <= confirmed && mispredicted.is_none_or(|first| first >= tick);
            let hash = match asked.hash {
                Some(hash) if built && asked.unanswered == 0 => hash,
                _ => return,
            };
            self.asked.remove(&tick);
            let received = self
                .remotes
                .iter_mut()
                .map(|remote| remote.hashes.remove(&tick))
                .collect();
            self.reports.insert(tick, HashReport { hash, received });
            self.final_through = tick;
        }
    }

    /// The first peer whose hash reports lag the newest final local hash `F` beyond what an
    /// honest peer can, with the tick that lags: [`ViolationRule::HashLagging`]. With `W` the
    /// rollback window, `D` the input delay and `I` the hash interval:
    ///
    /// - Its newest report trails `F` by at most `W + D + I - 1`. `F` is final, so this
    ///   session holds that peer's input of `F - 1`, which the peer sent once its tick `r` was
    ///   at least `F - D`. The peer reached `r` by an advance that did not stall, so its
    ///   confirmed tick was then at least `r - W`, and that advance ran its rollback. Its
    ///   caller answers the hashes of an advance before the next packet, so every multiple of
    ///   `I` up to `F - D - W` was final there, and the packet carrying that input carries
    ///   every report not acknowledged yet: one of a tick at least `F - D - W - (I - 1)`
    ///   arrived with it, or before.
    /// - Its acknowledgment of the local reports trails `F` by at most `2 * (W + D) + I - 1`.
    ///   The packet above carries an `Ack` of the newest report received from this session.
    ///   At that advance, the peer used this session's inputs below `r - W`, sent once this
    ///   session's tick was at least `r - W - D`; by the same argument, the packet carrying
    ///   them carried every local report up to at least `F - 2 * (W + D) - (I - 1)`.
    ///
    /// A report dropped as [`DropReason::HashBacklog`] keeps both: it is above a report
    /// already waiting, above the receiver's own newest final hash. Checked whenever a hash
    /// becomes final, so a session never holds more than
    /// [`MatchRules::unacknowledged_hash_reports`](crate::MatchRules::unacknowledged_hash_reports)
    /// unacknowledged reports, which every packet carries, and the reports of every peer up to
    /// `F - (W + D + I - 1)` have arrived: the states kept for reported ticks span at most
    /// `W + D + I - 1` ticks below `F` plus the ticks from `F` to the current one.
    fn hash_lagging(&self) -> Option<(PeerId, u64)> {
        let rules = self.settings.identity.rules;
        let window_and_delay = u64::from(rules.rollback_window) + u64::from(rules.input_delay);
        let interval = u64::from(rules.hash_interval);
        let reports_bound = window_and_delay + interval - 1;
        let acks_bound = 2 * window_and_delay + interval - 1;
        let newest = self.final_through;
        self.remotes.iter().find_map(|remote| {
            if newest > remote.hashes_received.saturating_add(reports_bound) {
                Some((remote.id, remote.hashes_received))
            } else if newest > remote.hashes_acked.saturating_add(acks_bound) {
                Some((remote.id, remote.hashes_acked))
            } else {
                None
            }
        })
    }

    /// Compares every final local hash not matched yet with the peers' reports of its tick, in
    /// tick order: a difference ends the session with [`EndReason::Desync`]; each tick every
    /// peer matched, up to the first one a report is missing for, emits
    /// [`SessionEvent::HashMatched`] and releases the states kept for the reported ticks
    /// before it. Then forgets the reports matched and acknowledged by every peer.
    fn check_hashes(&mut self, now: u64) {
        if !self.active() {
            return;
        }
        self.finalize_hashes();
        let mut matched = Vec::new();
        let mut in_order = true;
        let mut desync = None;
        for (&tick, report) in self.reports.range(self.matched_through.saturating_add(1)..) {
            let differing =
                report
                    .received
                    .iter()
                    .zip(&self.remotes)
                    .find_map(|(received, remote)| {
                        received
                            .filter(|hash| *hash != report.hash)
                            .map(|hash| (remote.id, hash))
                    });
            if let Some((peer, remote)) = differing {
                desync = Some(EndReason::Desync {
                    tick,
                    peer,
                    local: report.hash,
                    remote,
                });
                break;
            }
            // Matched in tick order: a tick some peer's report of is missing stops the matching
            // there, while the reports of later ticks are still compared.
            in_order &= report.received.iter().all(Option::is_some);
            if in_order {
                matched.push(tick);
            }
        }
        let interval = usize::try_from(self.settings.identity.rules.hash_interval)
            .expect("a u32 fits in a usize on every supported target");
        for tick in matched {
            self.events.push(SessionEvent::HashMatched { tick });
            self.releases
                .extend((self.kept_from..tick).step_by(interval));
            self.kept_from = tick;
            self.matched_through = tick;
        }
        if let Some(reason) = desync {
            self.end(now, reason);
            return;
        }
        if let Some((peer, tick)) = self.hash_lagging() {
            let rule = ViolationRule::HashLagging;
            let reason = EndReason::Violation {
                peer,
                player: None,
                tick: Some(tick),
                rule,
            };
            self.end(now, reason);
            return;
        }
        let matched_through = self.matched_through;
        let remotes = &self.remotes;
        self.reports.retain(|tick, _| {
            *tick > matched_through || remotes.iter().any(|remote| remote.hashes_acked < *tick)
        });
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

    /// The length of a tick in microseconds, at least one.
    fn tick_micros(&self) -> u64 {
        (1_000_000 / u64::from(self.settings.identity.tick_rate)).max(1)
    }

    /// The local advantage over the remote at `index` at `now`: the local tick minus the
    /// peer's estimated tick, its latest reported tick plus the ticks elapsed since the report
    /// plus half the ping in ticks, those two rounded together to the nearest tick, so the
    /// estimate is not biased low; saturated to an `i32`. `None` before a report and a round
    /// trip. The reported tick is untrusted: the arithmetic is wide enough for any.
    fn advantage(&self, index: usize, now: u64) -> Option<i32> {
        let remote = &self.remotes[index];
        let report = remote.report?;
        let ping = mean(remote.round_trips.iter().copied())?;
        let elapsed = now.saturating_sub(report.received_micros);
        let ahead_micros = i128::from(elapsed) + i128::from(ping / 2);
        let tick_micros = i128::from(self.tick_micros());
        let ahead_ticks = (ahead_micros + tick_micros / 2) / tick_micros;
        let advantage = i128::from(self.tick) - (i128::from(report.tick) + ahead_ticks);
        Some(i32::try_from(advantage).unwrap_or(if advantage < 0 { i32::MIN } else { i32::MAX }))
    }

    /// Reads a `Quality` request of the remote at `index`: answered in the next packet unless
    /// an equal or later one came already (a duplicate or a reordered request), and counted as
    /// a report unless it comes within half a [`QUALITY_INTERVAL_MICROS`] of the last one
    /// counted, so a peer cannot hurry a correction by flooding reports. A report counted once
    /// a ping is known adds the correction it calls for, `(local_advantage -
    /// remote_advantage) / 2`, where `remote_advantage` is the peer's own view of the
    /// advantage it has over this session.
    fn read_quality(
        &mut self,
        index: usize,
        tick: u64,
        sent_micros: u64,
        advantage: i32,
        now: u64,
    ) {
        let remote = &mut self.remotes[index];
        if remote
            .last_request_micros
            .is_some_and(|last| sent_micros <= last)
        {
            return;
        }
        remote.last_request_micros = Some(sent_micros);
        remote.reply_due = true;
        let fresh = remote.report.is_none_or(|report| {
            now.saturating_sub(report.received_micros) >= QUALITY_INTERVAL_MICROS / 2
        });
        if !fresh {
            return;
        }
        remote.report = Some(Report {
            tick,
            received_micros: now,
        });
        let Some(local) = self.advantage(index, now) else {
            return;
        };
        let id = self.remotes[index].id;
        self.stats.peers.entry(id).or_default().frame_advantage = local;
        push_bounded(
            &mut self.remotes[index].corrections,
            i64::from(local) - i64::from(advantage),
            SYNC_MIN_REPORTS,
        );
    }

    /// Reads a `QualityReply` of the remote at `index`: a reply echoing a pending request adds
    /// its round trip and updates the ping and the jitter. Whether the echo matched one: any
    /// other echo changes nothing.
    fn read_reply(&mut self, index: usize, echo_micros: u64, now: u64) -> bool {
        let remote = &mut self.remotes[index];
        let Some(position) = remote
            .pending_quality
            .iter()
            .position(|sent| *sent == echo_micros)
        else {
            return false;
        };
        remote.pending_quality.remove(position);
        // A pending request was sent at a time this session saw, so at most `now`.
        push_bounded(
            &mut remote.round_trips,
            now.saturating_sub(echo_micros),
            QUALITY_HISTORY,
        );
        let ping = mean(remote.round_trips.iter().copied()).unwrap_or(0);
        let deviations = remote.round_trips.iter().map(|trip| trip.abs_diff(ping));
        let jitter = mean(deviations).unwrap_or(0);
        let stats = self.stats.peers.entry(remote.id).or_default();
        stats.ping_micros = ping;
        stats.jitter_micros = jitter;
        true
    }

    /// The correction to start now, as the index of the remote it skips toward and the ticks
    /// to skip: the largest mean correction over the peers with [`SYNC_MIN_REPORTS`] fresh
    /// reports, rounded to the nearest tick, when it is at least one tick, at most
    /// `rollback_window / 2`; otherwise none. Rounded rather than truncated, so a peer half a
    /// tick or more ahead corrects at once instead of drifting a tick further first.
    ///
    /// Only a peer this session is ahead of at `now`, by its own estimate, counts: the peer
    /// behind never skips. Once the round trip passes `rollback_window + input_delay` ticks,
    /// both peers run at the edge of their window and the stalls, not the clocks, set their
    /// pace: each stall of the faster peer leaves the slower one briefly ahead in both views,
    /// and a skip of the slower one would only make the faster one stall more.
    fn correction(&self, now: u64) -> Option<(usize, u64)> {
        let cap = u64::from(self.settings.identity.rules.rollback_window / 2);
        let (toward, mean) = self
            .remotes
            .iter()
            .enumerate()
            .filter(|(index, remote)| {
                remote.corrections.len() >= SYNC_MIN_REPORTS && self.ahead_of(*index, now)
            })
            .map(|(index, remote)| {
                // At most `SYNC_MIN_REPORTS` doubled corrections are kept, each within twice an
                // `i32`: their sum fits an `i64`. `doubled / (2 * count)`, halves rounded up.
                let doubled: i64 = remote.corrections.iter().sum();
                let count = i64::try_from(remote.corrections.len())
                    .expect("at most SYNC_MIN_REPORTS corrections are kept");
                (index, (doubled + count).div_euclid(2 * count))
            })
            .max_by_key(|(_, mean)| *mean)?;
        let skips = u64::try_from(mean).ok()?.min(cap);
        (skips >= 1).then_some((toward, skips))
    }

    /// Whether this session is ahead of the remote at `index` at `now`, by its own estimate.
    fn ahead_of(&self, index: usize, now: u64) -> bool {
        self.advantage(index, now).is_some_and(|ahead| ahead > 0)
    }

    /// Forgets the reports of every peer, so no correction acts on them again.
    fn forget_reports(&mut self) {
        for remote in &mut self.remotes {
            remote.corrections.clear();
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

    /// Runs once every peer has welcomed this one and sent a matching `Hello`. The silence of
    /// every peer starts then: a peer done with its handshake has nothing to send before.
    fn start_when_agreed(&mut self) {
        let agreed = self
            .remotes
            .iter()
            .all(|remote| remote.hello && remote.welcomed);
        if self.state == SessionState::Connecting && agreed {
            self.state = SessionState::Running;
            self.events.push(SessionEvent::Started);
            let now = self.now_micros.unwrap_or(0);
            for remote in &mut self.remotes {
                remote.last_heard_micros = now;
            }
        }
    }

    /// Raises [`SessionEvent::PeerSilent`] for every peer newly silent for
    /// `notice_after_micros`, then ends the session on a silence of `disconnect_after_micros`:
    /// a leaving session `disconnect_after_micros` after [`Session::leave`], with
    /// [`EndReason::Left`]; a running one when some peer has been silent that long, with
    /// [`EndReason::Left`] when the match ends on purpose and every player's input through its
    /// last tick is confirmed, as only acknowledgments were missing, and with
    /// [`EndReason::Lost`] naming the first such peer otherwise.
    fn check_silence(&mut self, now: u64) {
        let timeouts = self.settings.timeouts;
        for remote in &mut self.remotes {
            let silence = now.saturating_sub(remote.last_heard_micros);
            if silence >= timeouts.notice_after_micros && !remote.silent {
                remote.silent = true;
                self.events.push(SessionEvent::PeerSilent {
                    peer: remote.id,
                    for_micros: silence,
                });
            }
        }
        let left = |closing: Closing| EndReason::Left {
            peer: closing.peer,
            last_tick: closing.last_tick,
        };
        if let Some((_, since)) = self.leaving {
            // A leaving session has agreed on a last tick: its own at least.
            if let Some(closing) = self.closing
                && now.saturating_sub(since) >= timeouts.disconnect_after_micros
            {
                self.end(now, left(closing));
            }
            return;
        }
        let Some(lost) = self
            .remotes
            .iter()
            .find(|remote| {
                now.saturating_sub(remote.last_heard_micros) >= timeouts.disconnect_after_micros
            })
            .map(|remote| remote.id)
        else {
            return;
        };
        let confirmed = self.confirmed_tick();
        let reason = match self.closing {
            Some(closing) if confirmed > closing.last_tick => left(closing),
            _ => EndReason::Lost { peer: lost },
        };
        self.end(now, reason);
    }

    /// Reads the goodbye of `from`, which leaves after `last_tick`. A last tick past the
    /// furthest input an honest peer can have is [`ViolationRule::BeyondHorizon`], one below
    /// the last confirmed tick [`ViolationRule::GoodbyeBehind`]: the leaver's last tick is its
    /// input horizon, which every input it sent is at most, and every confirmed tick holds one
    /// of its inputs. Otherwise the lowest last tick heard wins.
    fn read_goodbye(&mut self, from: PeerId, last_tick: u64) -> Result<(), Refusal> {
        let violation = |rule| Refusal::Violation {
            player: None,
            tick: Some(last_tick),
            rule,
        };
        if last_tick > self.input_limit() {
            return Err(violation(ViolationRule::BeyondHorizon));
        }
        if last_tick.saturating_add(1) < self.confirmed_tick() {
            return Err(violation(ViolationRule::GoodbyeBehind));
        }
        self.close(last_tick, from);
        Ok(())
    }

    /// Takes `last_tick`, named by `peer`, as the last tick of the match unless a lower one is
    /// agreed already. On a tie the lowest peer is kept, so every peer names the same one.
    fn close(&mut self, last_tick: u64, peer: PeerId) {
        if self
            .closing
            .is_none_or(|closing| (last_tick, peer) < (closing.last_tick, closing.peer))
        {
            self.closing = Some(Closing { last_tick, peer });
        }
    }

    /// Ends the session with [`EndReason::Left`] once the match ending on purpose is over for
    /// it: every player's input through the agreed last tick is confirmed, and every peer
    /// acknowledged every local player's inputs through it, so no peer needs anything more
    /// from this one. The peers that did not hear the acknowledgments of this one yet hear them
    /// while it lingers.
    fn check_done(&mut self, now: u64) {
        let Some(closing) = self.closing.filter(|_| self.active()) else {
            return;
        };
        let through = closing.last_tick.saturating_add(1);
        let acknowledged = self.local_players().all(|player| {
            self.remotes
                .iter()
                .all(|remote| remote.acked[player] >= through)
        });
        if acknowledged && self.confirmed_tick() >= through {
            let reason = EndReason::Left {
                peer: closing.peer,
                last_tick: closing.last_tick,
            };
            self.end(now, reason);
        }
    }

    /// The requests that land the caller's simulation on the state of the end tick, as
    /// [`Session::advance`] describes, after emitting the confirmed ticks below it.
    ///
    /// The tick loaded is at most `rollback_window` before the current one, like the tick of a
    /// rollback: it is at least the confirmed tick of the last advance, as an input confirmed
    /// since is of a tick at least that one, and the end tick is too (a goodbye below it is
    /// [`ViolationRule::GoodbyeBehind`]). The current tick is at most `rollback_window` past
    /// that confirmed tick. Every input read from it on is held: a queue releases only ticks
    /// below the confirmed tick of an advance that ran with their confirmed inputs.
    fn land(&mut self) -> Vec<Request> {
        self.land_due = false;
        self.emit_confirmed();
        let end = self.confirmed_tick();
        let built = self
            .queues
            .iter()
            .filter_map(|queue| queue.first_misprediction(self.tick))
            .min()
            .unwrap_or(self.tick)
            .min(end);
        let mut requests = Vec::new();
        if built < self.tick {
            requests.push(Request::Load { tick: built });
        }
        for tick in built..end {
            let inputs = self.inputs_at(tick);
            self.record(tick, &inputs);
            requests.push(self.save(tick));
            requests.push(Request::Advance { tick, inputs });
        }
        self.tick = end;
        requests
    }

    /// Ends the session for a reason it decided alone: the end notice telling of it, if any,
    /// lingers.
    fn end(&mut self, now: u64, reason: EndReason) {
        self.end_notice = match reason {
            EndReason::Desync {
                tick,
                peer,
                local,
                remote,
            } => Some(EndNotice::Desync {
                tick,
                peer,
                local,
                remote,
            }),
            EndReason::Lost { peer } => Some(EndNotice::Lost { peer }),
            EndReason::LocalFailure { tick } => Some(EndNotice::LocalFailure { tick }),
            EndReason::Mismatch { .. }
            | EndReason::Unreachable { .. }
            | EndReason::Violation { .. }
            | EndReason::Left { .. } => None,
        };
        self.land_due = self.active();
        self.state = SessionState::Ended(reason);
        self.ended_micros = Some(now);
    }

    /// Checks that an honest peer can send `notice`, or refuses it as
    /// [`ViolationRule::ImplausibleEnd`], naming its tick when it has one. A peer it names
    /// must be one of the match. A desync is of a tick whose hash was final on the sender,
    /// which needs the local input of the tick before: at most the local horizon plus one, as
    /// for [`ViolationRule::HashBeyondHorizon`]. A game fails while stepping a tick its
    /// session ran, which is below the sender's tick, at most `rollback_window - 1` past its
    /// confirmed tick, itself at most the local horizon plus one. A lost peer is not its
    /// sender, `from`. Nothing else is checkable: an honest peer may lose this one, or any
    /// other, while this one still hears it, and any peer can end the match for every peer
    /// anyway by going silent, so a forged loss only ends it sooner.
    fn check_notice(&self, from: PeerId, notice: EndNotice) -> Result<(), Refusal> {
        let implausible = |tick| {
            Err(Refusal::Violation {
                player: None,
                tick,
                rule: ViolationRule::ImplausibleEnd,
            })
        };
        let of_match = |peer: PeerId| {
            peer == self.settings.local || self.remotes.iter().any(|remote| remote.id == peer)
        };
        // Validated to be at least one.
        let window = u64::from(self.settings.identity.rules.rollback_window);
        match notice {
            EndNotice::Desync { tick, peer, .. } if !of_match(peer) => implausible(Some(tick)),
            EndNotice::Desync { tick, .. } if tick > self.after_horizon() => {
                implausible(Some(tick))
            }
            EndNotice::Lost { peer } if !of_match(peer) || peer == from => implausible(None),
            EndNotice::LocalFailure { tick }
                if tick > self.after_horizon().saturating_add(window - 1) =>
            {
                implausible(Some(tick))
            }
            _ => Ok(()),
        }
    }

    /// Ends the session for the reason an end notice tells, which is not relayed again.
    fn end_by_notice(&mut self, now: u64, notice: EndNotice) {
        let reason = match notice {
            EndNotice::Desync {
                tick,
                peer,
                local,
                remote,
            } => EndReason::Desync {
                tick,
                peer,
                local,
                remote,
            },
            EndNotice::Lost { peer } => EndReason::Lost { peer },
            EndNotice::LocalFailure { tick } => EndReason::LocalFailure { tick },
        };
        self.land_due = self.active();
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

/// Appends `value`, forgetting the oldest values beyond `capacity`.
fn push_bounded<T>(values: &mut VecDeque<T>, value: T, capacity: usize) {
    values.push_back(value);
    while values.len() > capacity {
        values.pop_front();
    }
}

/// The mean of `values`, rounded down; `None` when there are none.
fn mean(values: impl Iterator<Item = u64>) -> Option<u64> {
    let (sum, count) = values.fold((0u128, 0u128), |(sum, count), value| {
        (sum + u128::from(value), count + 1)
    });
    // The mean of `u64`s is a `u64`.
    (count > 0).then(|| u64::try_from(sum / count).unwrap_or(u64::MAX))
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

/// The players `owner` owns, in order.
fn owned_by(settings: &SessionSettings, owner: PeerId) -> impl Iterator<Item = u8> + use<'_> {
    settings
        .identity
        .players
        .iter()
        .zip(0u8..)
        .filter(move |(player_owner, _)| **player_owner == owner)
        .map(|(_, player)| player)
}

/// The `Ack` of `peer`'s players and of its hash reports through `hashes_through`.
fn ack_for(
    settings: &SessionSettings,
    queues: &[InputQueue],
    peer: PeerId,
    hashes_through: u64,
) -> Message {
    Message::Ack {
        players: owned_by(settings, peer)
            .map(|player| {
                let next = queues[usize::from(player)].next_confirmed();
                (player, next.unwrap_or(u64::MAX))
            })
            .collect(),
        hashes_through,
    }
}

/// The `Ack` of `peer`'s players and of its hash reports through `hashes_through`, and the
/// inputs of every local player from what `peer` acknowledged on, at most
/// [`MAX_TICKS_PER_MESSAGE`] ticks each.
fn inputs_for(
    settings: &SessionSettings,
    queues: &[InputQueue],
    peer: PeerId,
    acked: &[u64],
    hashes_through: u64,
) -> Vec<Message> {
    let mut messages = vec![ack_for(settings, queues, peer, hashes_through)];
    for player in owned_by(settings, settings.local) {
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
/// first, the rest in a later packet. The settings bound the usual packet, every unacknowledged
/// hash report included, within the limits; this only acts for a peer that has fallen behind
/// by more than the rollback window and the input delay.
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
