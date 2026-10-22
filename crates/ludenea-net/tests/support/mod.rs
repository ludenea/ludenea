//! The multi-peer harness of the session tests: one session per peer of a match, over a
//! [`MemoryNetwork`], optionally behind [`Conditioned`], driven in steps of
//! [`STEP_MICROS`] of `now_micros`. Each step, every peer that is not muted takes every packet
//! that has arrived, then sends what its session has to send.

use ludenea_app::{PlayerInputSpec, SimError};
use ludenea_net::{
    Conditioned, LinkConditions, MatchIdentity, MatchRules, MemoryNetwork, PeerId, Session,
    SessionEvent, SessionSettings, SessionState, Timeouts, Transport,
};

/// The time between two steps of the harness.
pub const STEP_MICROS: u64 = 1_000;

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
    let identity = identity(&owners);
    (0..peers)
        .map(|local| Session::new(settings(local, &identity), model_input()).unwrap())
        .collect()
}

struct Peer {
    session: Session,
    transport: Box<dyn Transport>,
    events: Vec<SessionEvent>,
    muted: bool,
}

/// The peers of one match and the time they share.
pub struct Harness {
    peers: Vec<Peer>,
    now_micros: u64,
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
            })
            .collect();
        Self {
            peers,
            now_micros: 0,
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

    /// One step at the current time, then the time moves on by [`STEP_MICROS`]. Every send
    /// must succeed: a session never builds a packet a transport refuses.
    pub fn step(&mut self) {
        let now = self.now_micros;
        for peer in self.peers.iter_mut().filter(|peer| !peer.muted) {
            while let Some((from, packet)) = peer.transport.receive(now) {
                peer.session.receive(from, &packet, now);
            }
            for (to, packet) in peer.session.outgoing(now) {
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

    /// The peers of the match.
    pub fn peers(&self) -> impl Iterator<Item = PeerId> + use<> {
        (0..self.peers.len()).map(|peer| PeerId(u8::try_from(peer).unwrap()))
    }
}

fn network(peers: usize) -> Vec<ludenea_net::MemoryTransport> {
    MemoryNetwork::new(u8::try_from(peers).unwrap())
}
