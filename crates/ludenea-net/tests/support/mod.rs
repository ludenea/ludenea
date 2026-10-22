//! The multi-peer harness of the session tests: one session per peer of a match, over a
//! [`MemoryNetwork`], optionally behind [`Conditioned`], driven in steps of
//! [`STEP_MICROS`] of `now_micros`. Each step, every peer that is not muted takes every packet
//! that has arrived, advances its session once per frame while it runs, with [`model_byte`]
//! for each local player, applies the requests to its [`Model`], then sends what its session
//! has to send. A frame lasts [`FRAME_MICROS`] unless [`Harness::set_frame_micros`] says
//! otherwise. The model answers every `Save { hash: true }` through [`Session::report_hash`].

#![allow(
    dead_code,
    reason = "each test binary that includes the harness uses a different part of it"
)]

use std::collections::{BTreeMap, BTreeSet};

use ludenea_app::{PlayerInputSpec, SimError};
use ludenea_net::{
    Conditioned, LinkConditions, MAX_MESSAGES_PER_PACKET, MAX_PACKET_BYTES, MAX_PLAYERS,
    MatchIdentity, MatchRules, MemoryNetwork, Message, Packet, PeerId, Request, Session,
    SessionError, SessionEvent, SessionSettings, SessionState, Timeouts, Transport,
};

/// The time between two steps of the harness.
pub const STEP_MICROS: u64 = 1_000;

/// The tick rate of every match of the model game.
pub const TICK_RATE: u32 = 60;

/// The time between two frames, each advancing a session once.
pub const FRAME_MICROS: u64 = 1_000_000 / TICK_RATE as u64;

/// The input the harness adds for `player` at the `frame`-th frame of its peer: it changes
/// every few frames, so a prediction is right most of the time but not always.
pub fn model_byte(player: u8, frame: u64) -> u8 {
    u8::try_from((frame / 5 + u64::from(player) * 3) % 4).unwrap()
}

/// The hash of the model game's state at tick 0.
const MODEL_START: u64 = 0xcbf2_9ce4_8422_2325;

/// The state of the model game after the tick `tick` ran with `inputs` from `hash`: the
/// inputs folded into a running FNV-1a hash.
pub fn fold(hash: u64, tick: u64, inputs: &[Vec<u8>]) -> u64 {
    let mut hash = hash;
    let bytes = tick
        .to_le_bytes()
        .into_iter()
        .chain(inputs.iter().flat_map(|input| {
            std::iter::once(u8::try_from(input.len()).unwrap()).chain(input.iter().copied())
        }));
    for byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The hash of the state of every tick `1..=n` when `confirmed` holds the inputs of the ticks
/// `0..n`, in order from tick 0.
pub fn offline_hashes(confirmed: &[(u64, Vec<Vec<u8>>)]) -> Vec<u64> {
    let mut hash = MODEL_START;
    confirmed
        .iter()
        .map(|(tick, inputs)| {
            hash = fold(hash, *tick, inputs);
            hash
        })
        .collect()
}

/// A game whose state is a hash of every input it ran with, executing a session's requests.
/// It checks the session's promises as it goes: every `Save` and `Advance` is at the model's
/// tick, and every `Load` restores the latest `Save` of its tick, which must be the state the
/// inputs the ticks before it last ran with give: no input below a loaded tick changed since
/// it was saved. A state saved with `hash: true` is kept until its `Release`, which must name
/// a kept state; a released state is gone, so a later `Load` of it fails.
#[derive(Debug)]
pub struct Model {
    tick: u64,
    hash: u64,
    saves: BTreeMap<u64, u64>,
    /// The inputs each tick last ran with.
    used: BTreeMap<u64, Vec<Vec<u8>>>,
    /// The latest hash of the state of each tick.
    hashes: BTreeMap<u64, u64>,
    loads: u64,
    /// The ticks whose state was saved with `hash: true` and not released yet.
    kept: BTreeSet<u64>,
    /// Every released tick, in order.
    released: Vec<u64>,
    /// The tick whose reported hash is flipped, as a broken game would.
    flip: Option<u64>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            tick: 0,
            hash: MODEL_START,
            saves: BTreeMap::new(),
            used: BTreeMap::new(),
            hashes: BTreeMap::from([(0, MODEL_START)]),
            loads: 0,
            kept: BTreeSet::new(),
            released: Vec::new(),
            flip: None,
        }
    }
}

impl Model {
    /// Applies `requests` in order, panicking on any broken promise. Returns the hash of every
    /// state saved with `hash: true`, in order, as `(tick, hash)`.
    pub fn execute(&mut self, requests: &[Request]) -> Vec<(u64, u64)> {
        let mut reports = Vec::new();
        for request in requests {
            match request {
                Request::Save { tick, hash } => {
                    assert_eq!(*tick, self.tick, "Save out of step");
                    self.saves.insert(*tick, self.hash);
                    if *hash {
                        self.kept.insert(*tick);
                        let flip = u64::from(self.flip == Some(*tick));
                        reports.push((*tick, self.hash ^ flip));
                    }
                }
                Request::Release { tick } => {
                    assert!(
                        self.kept.remove(tick),
                        "Release {{ tick: {tick} }} of a state not kept"
                    );
                    self.saves.remove(tick);
                    self.released.push(*tick);
                }
                Request::Load { tick } => {
                    let saved = *self
                        .saves
                        .get(tick)
                        .unwrap_or_else(|| panic!("Load {{ tick: {tick} }} without a Save"));
                    let replayed = (0..*tick).fold(MODEL_START, |hash, earlier| {
                        fold(hash, earlier, &self.used[&earlier])
                    });
                    assert_eq!(
                        saved, replayed,
                        "Load {{ tick: {tick} }} restores a state some input below it no \
                         longer gives"
                    );
                    self.tick = *tick;
                    self.hash = saved;
                    self.loads += 1;
                }
                Request::Advance { tick, inputs } => {
                    assert_eq!(*tick, self.tick, "Advance out of step");
                    assert_eq!(
                        self.saves.get(tick),
                        Some(&self.hash),
                        "Advance without Save"
                    );
                    self.hash = fold(self.hash, *tick, inputs);
                    self.used.insert(*tick, inputs.clone());
                    self.tick += 1;
                    self.hashes.insert(self.tick, self.hash);
                }
            }
        }
        reports
    }

    /// Executes `requests`, then answers each hash asked through `session`.
    pub fn execute_for(&mut self, session: &mut Session, requests: &[Request]) {
        for (tick, hash) in self.execute(requests) {
            session
                .report_hash(tick, hash)
                .unwrap_or_else(|error| panic!("report_hash of tick {tick}: {error}"));
        }
    }

    /// Reports the hash of the state of `tick` flipped, as a broken game would.
    pub fn flip_hash(&mut self, tick: u64) {
        self.flip = Some(tick);
    }

    /// The ticks whose state is kept, saved with `hash: true` and not released yet.
    pub fn kept(&self) -> &BTreeSet<u64> {
        &self.kept
    }

    /// Every released tick, in order.
    pub fn released(&self) -> &[u64] {
        &self.released
    }

    /// The next tick the model runs.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The latest hash of the state of `tick`.
    pub fn hash_at(&self, tick: u64) -> Option<u64> {
        self.hashes.get(&tick).copied()
    }

    /// How many `Load`s the model executed.
    pub fn loads(&self) -> u64 {
        self.loads
    }
}

/// The input of the model game: one byte per player and tick.
pub fn model_input() -> PlayerInputSpec {
    PlayerInputSpec {
        default: vec![0],
        check: |bytes| match bytes {
            [_] => Ok(()),
            _ => Err(SimError::InputDecode(format!(
                "a model input takes 1 byte, got {}",
                bytes.len()
            ))),
        },
    }
}

/// The identity of a match of the model game where player `i` is owned by `owners[i]`.
pub fn identity(owners: &[u8]) -> MatchIdentity {
    MatchIdentity {
        engine: "ludenea-test".to_string(),
        fingerprint: 7,
        scene_and_assets: 9,
        tick_rate: 60,
        players: owners.iter().copied().map(PeerId).collect(),
        rules: MatchRules::default(),
    }
}

/// The settings of `local` in a match of `identity`.
pub fn settings(local: u8, identity: &MatchIdentity) -> SessionSettings {
    SessionSettings {
        local: PeerId(local),
        match_id: 1,
        identity: identity.clone(),
        timeouts: Timeouts::default(),
    }
}

/// One session per peer, `0..peers`, each owning one player, of the same match.
pub fn sessions(peers: u8) -> Vec<Session> {
    let owners: Vec<u8> = (0..peers).collect();
    sessions_of(&identity(&owners))
}

/// One session per peer of a match of `identity`.
pub fn sessions_of(identity: &MatchIdentity) -> Vec<Session> {
    let peers = identity.players.iter().map(|owner| owner.0).max().unwrap() + 1;
    (0..peers)
        .map(|local| Session::new(settings(local, identity), model_input()).unwrap())
        .collect()
}

/// The sessions, run through the handshake by passing every packet at once, without any
/// advance: each is `Running`, at tick 0, with its events taken.
pub fn handshake(mut sessions: Vec<Session>) -> Vec<Session> {
    for _ in 0..8 {
        for from in 0..sessions.len() {
            for (to, packet) in sessions[from].outgoing(0) {
                let sender = PeerId(u8::try_from(from).unwrap());
                sessions[usize::from(to.0)].receive(sender, &packet, 0);
            }
        }
        if sessions
            .iter()
            .all(|session| session.state() == SessionState::Running)
        {
            for session in &mut sessions {
                session.events();
            }
            return sessions;
        }
    }
    panic!("the handshake did not finish");
}

struct Peer {
    session: Session,
    transport: Box<dyn Transport>,
    events: Vec<SessionEvent>,
    muted: bool,
    /// Receives and sends, but runs no frame: a machine too slow to keep up.
    frozen: bool,
    model: Model,
    /// The time between two frames of this peer.
    frame_micros: u64,
    /// When the next frame is due, from the first step the session runs.
    next_frame_micros: Option<u64>,
    frames: u64,
    sent_packets: u64,
    /// The packets carrying a hash report of this tick are lost.
    lose_hash_of: Option<u64>,
    /// Every frame uses the default input, so every prediction is right.
    default_inputs: bool,
}

impl Peer {
    /// One frame: an input for every local player, then one advance, executed by the model.
    fn frame(&mut self, now: u64) {
        for player in 0..u8::try_from(MAX_PLAYERS).unwrap() {
            let byte = if self.default_inputs {
                0
            } else {
                model_byte(player, self.frames)
            };
            match self.session.add_local_input(player, vec![byte]) {
                Ok(()) | Err(SessionError::NotLocal { .. }) => {}
                Err(error) => panic!("add_local_input of player {player}: {error}"),
            }
        }
        let requests = self
            .session
            .advance(now)
            .expect("a running session advances");
        self.model.execute_for(&mut self.session, &requests);
        self.frames += 1;
    }
}

/// The peers of one match and the time they share.
pub struct Harness {
    peers: Vec<Peer>,
    now_micros: u64,
    sent_packets: u64,
    drop_every_other: bool,
    /// The most hash reports one sent packet carried.
    most_hash_reports: usize,
}

impl Harness {
    /// The sessions over a lossless in-memory network. `sessions[i]` must run for peer `i`.
    pub fn new(sessions: Vec<Session>) -> Self {
        let transports = network(sessions.len())
            .into_iter()
            .map(|transport| Box::new(transport) as Box<dyn Transport>)
            .collect();
        Self::over(sessions, transports)
    }

    /// The sessions over an in-memory network where every peer's sends go through
    /// [`Conditioned`] with `conditions`, seeded differently per peer.
    pub fn conditioned(sessions: Vec<Session>, conditions: LinkConditions) -> Self {
        let transports = network(sessions.len())
            .into_iter()
            .zip(0u64..)
            .map(|(transport, peer)| {
                let conditions = LinkConditions {
                    seed: conditions.seed.wrapping_add(peer),
                    ..conditions
                };
                Box::new(Conditioned::new(transport, conditions).unwrap()) as Box<dyn Transport>
            })
            .collect();
        Self::over(sessions, transports)
    }

    fn over(sessions: Vec<Session>, transports: Vec<Box<dyn Transport>>) -> Self {
        let peers = sessions
            .into_iter()
            .zip(transports)
            .map(|(session, transport)| Peer {
                session,
                transport,
                events: Vec::new(),
                muted: false,
                frozen: false,
                model: Model::default(),
                frame_micros: FRAME_MICROS,
                next_frame_micros: None,
                frames: 0,
                sent_packets: 0,
                lose_hash_of: None,
                default_inputs: false,
            })
            .collect();
        Self {
            peers,
            now_micros: 0,
            sent_packets: 0,
            drop_every_other: false,
            most_hash_reports: 0,
        }
    }

    /// Loses every other packet each peer sends from now on, before the transport sees it.
    pub fn drop_every_other_packet(&mut self) {
        self.drop_every_other = true;
    }

    /// Has the model of `peer` report the hash of `tick` flipped, as a broken game would.
    pub fn flip_hash(&mut self, peer: PeerId, tick: u64) {
        self.peers[usize::from(peer.0)].model.flip_hash(tick);
    }

    /// Loses every packet `peer` sends that carries its hash report of `tick`.
    pub fn lose_hash_reports(&mut self, peer: PeerId, tick: u64) {
        self.peers[usize::from(peer.0)].lose_hash_of = Some(tick);
    }

    /// Has every peer add the default input every frame, so every prediction is right.
    pub fn default_inputs(&mut self) {
        for peer in &mut self.peers {
            peer.default_inputs = true;
        }
    }

    /// The time of the next step.
    pub fn now_micros(&self) -> u64 {
        self.now_micros
    }

    /// Stops `peer` from receiving and sending, as a machine that is down.
    pub fn mute(&mut self, peer: PeerId) {
        self.peers[usize::from(peer.0)].muted = true;
    }

    /// Lets a muted `peer` receive and send again.
    pub fn unmute(&mut self, peer: PeerId) {
        self.peers[usize::from(peer.0)].muted = false;
    }

    /// Stops `peer` from running frames while it still receives and sends. When it thaws, it
    /// runs one frame per step until it catches up with the time.
    pub fn freeze(&mut self, peer: PeerId) {
        self.peers[usize::from(peer.0)].frozen = true;
    }

    /// Lets a frozen `peer` run frames again.
    pub fn thaw(&mut self, peer: PeerId) {
        self.peers[usize::from(peer.0)].frozen = false;
    }

    /// Runs the frames of `peer` every `frame_micros` instead of [`FRAME_MICROS`]: a machine
    /// whose clock runs fast or slow.
    pub fn set_frame_micros(&mut self, peer: PeerId, frame_micros: u64) {
        self.peers[usize::from(peer.0)].frame_micros = frame_micros;
    }

    /// The most hash reports one packet any session sent carried so far.
    pub fn most_hash_reports(&self) -> usize {
        self.most_hash_reports
    }

    /// How many packets the sessions sent so far.
    pub fn sent_packets(&self) -> u64 {
        self.sent_packets
    }

    /// One step at the current time, then the time moves on by [`STEP_MICROS`]. Every packet
    /// a session sends must decode within the limits of one packet, and every send succeed.
    pub fn step(&mut self) {
        let now = self.now_micros;
        for peer in self.peers.iter_mut().filter(|peer| !peer.muted) {
            while let Some((from, packet)) = peer.transport.receive(now) {
                peer.session.receive(from, &packet, now);
            }
            if peer.session.state() == SessionState::Running && !peer.frozen {
                let due = *peer.next_frame_micros.get_or_insert(now);
                if now >= due {
                    peer.frame(now);
                    peer.next_frame_micros = Some(due + peer.frame_micros);
                }
            }
            for (to, packet) in peer.session.outgoing(now) {
                let reports = check_limits(&packet);
                self.most_hash_reports = self.most_hash_reports.max(reports);
                self.sent_packets += 1;
                peer.sent_packets += 1;
                if self.drop_every_other && peer.sent_packets % 2 == 0 {
                    continue;
                }
                if peer
                    .lose_hash_of
                    .is_some_and(|lost| carries_hash_of(&packet, lost))
                {
                    continue;
                }
                peer.transport
                    .send(to, packet, now)
                    .expect("a session sends only packets a transport accepts");
            }
            peer.events.extend(peer.session.events());
        }
        self.now_micros += STEP_MICROS;
    }

    /// Steps until `done` holds, for at most `max_micros`; whether it held.
    pub fn run_until(&mut self, max_micros: u64, done: impl Fn(&Harness) -> bool) -> bool {
        let end = self.now_micros + max_micros;
        while self.now_micros < end {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    /// Steps until every peer that is not muted runs, for at most `max_micros`.
    pub fn run_until_running(&mut self, max_micros: u64) -> bool {
        self.run_until(max_micros, |harness| {
            harness
                .peers
                .iter()
                .filter(|peer| !peer.muted)
                .all(|peer| peer.session.state() == SessionState::Running)
        })
    }

    /// The session of `peer`.
    pub fn session(&mut self, peer: PeerId) -> &mut Session {
        &mut self.peers[usize::from(peer.0)].session
    }

    /// The state of `peer`.
    pub fn state(&self, peer: PeerId) -> SessionState {
        self.peers[usize::from(peer.0)].session.state()
    }

    /// Every event `peer` emitted so far, in order.
    pub fn events(&self, peer: PeerId) -> &[SessionEvent] {
        &self.peers[usize::from(peer.0)].events
    }

    /// The model game of `peer`.
    pub fn model(&self, peer: PeerId) -> &Model {
        &self.peers[usize::from(peer.0)].model
    }

    /// The inputs of every tick `peer` confirmed so far, in the order of its
    /// [`SessionEvent::Confirmed`] events.
    pub fn confirmed(&self, peer: PeerId) -> Vec<(u64, Vec<Vec<u8>>)> {
        self.events(peer)
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Confirmed { tick, inputs } => Some((*tick, inputs.clone())),
                _ => None,
            })
            .collect()
    }

    /// The peers of the match.
    pub fn peers(&self) -> impl Iterator<Item = PeerId> + use<> {
        (0..self.peers.len()).map(|peer| PeerId(u8::try_from(peer).unwrap()))
    }
}

/// A packet a session sends decodes, within [`MAX_PACKET_BYTES`] and
/// [`MAX_MESSAGES_PER_PACKET`]. Returns how many hash reports it carries.
fn check_limits(packet: &[u8]) -> usize {
    assert!(packet.len() <= MAX_PACKET_BYTES, "{} bytes", packet.len());
    let decoded = Packet::decode(packet).expect("a session sends packets that decode");
    assert!(
        decoded.messages.len() <= MAX_MESSAGES_PER_PACKET,
        "{} messages",
        decoded.messages.len()
    );
    decoded
        .messages
        .iter()
        .filter(|message| matches!(message, Message::Hash { .. }))
        .count()
}

/// Whether `packet` carries a hash report of `tick`.
fn carries_hash_of(packet: &[u8], tick: u64) -> bool {
    let decoded = Packet::decode(packet).expect("a session sends packets that decode");
    decoded
        .messages
        .iter()
        .any(|message| matches!(message, Message::Hash { tick: reported, .. } if *reported == tick))
}

fn network(peers: usize) -> Vec<ludenea_net::MemoryTransport> {
    MemoryNetwork::new(u8::try_from(peers).unwrap())
}
