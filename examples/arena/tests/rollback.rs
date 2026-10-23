//! The property test of rollback on the arena: two to four peers, each driving its own arena
//! simulation through an `Executor`, play over seeded degraded links, and one leaves. Every
//! peer ends holding the same confirmed inputs, and stepping them offline, joined, gives every
//! peer's executed state hash at every confirmed tick, at 1 and 8 threads.
//!
//! `LUDENEA_NET_SEEDS` sets the seeds per peer count and thread count, 8 by default.

use std::collections::BTreeMap;
use std::path::Path;

use arena::ArenaInput;
use ludenea::app::project::project_assets;
use ludenea::app::{Rng, Simulation, join_player_inputs};
use ludenea::net::{
    Conditioned, EndReason, Executor, LinkConditions, MatchIdentity, MatchRules, MemoryNetwork,
    MemoryTransport, PeerId, Request, Session, SessionEvent, SessionSettings, SessionState,
    Timeouts, Transport,
};

/// The arena with a spawn point for each of four players.
const SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/arena-four.scene.ron"
);
const ONE_SECOND: u64 = 1_000_000;
/// The time between two steps of the match.
const STEP_MICROS: u64 = 1_000;
/// The ticks the leaver runs before it leaves.
const TICKS: u64 = 300;
/// The runs a player's input picks from.
const RUNS: [f32; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];

fn seeds() -> u64 {
    match std::env::var("LUDENEA_NET_SEEDS") {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("LUDENEA_NET_SEEDS is {value:?}, expected a count")),
        Err(_) => 8,
    }
}

/// The arena simulation of the fixture scene, on `threads` threads.
fn simulation(threads: usize) -> Simulation {
    let scene = std::fs::read_to_string(SCENE).expect("the fixture scene reads");
    let assets = project_assets(Path::new(SCENE), None).expect("the project's assets read");
    let mut simulation = arena::app().build().expect("the arena builds");
    simulation
        .load_scene_with(&scene, &assets)
        .unwrap_or_else(|error| panic!("{error}"));
    simulation.set_threads(threads).unwrap();
    simulation
}

/// The link conditions of `seed`: latency 0 to 150 ms, jitter 0 to 30 ms, loss 0 to 100 per
/// mille, duplication 0 to 20 per mille, reordering 0 to 50 per mille by 0 to 60 ms more.
fn conditions(seed: u64) -> LinkConditions {
    // The players draw their inputs from the streams 0 to 3.
    let mut rng = Rng::new(seed, 100);
    let mut draw = |low: u32, high: u32| rng.range_u32(low, high + 1);
    LinkConditions {
        latency_micros: draw(0, 150_000),
        jitter_micros: draw(0, 30_000),
        loss_per_mille: u16::try_from(draw(0, 100)).unwrap(),
        duplicate_per_mille: u16::try_from(draw(0, 20)).unwrap(),
        reorder_per_mille: u16::try_from(draw(0, 50)).unwrap(),
        reorder_extra_micros: draw(0, 60_000),
        seed,
    }
}

/// A random canonical input: a run from [`RUNS`], a jump or not.
fn random_input(rng: &mut Rng) -> Vec<u8> {
    let run = RUNS[usize::try_from(rng.range_u32(0, 5)).unwrap()];
    let jump = rng.range_u32(0, 2) == 1;
    postcard::to_allocvec(&ArenaInput { run, jump }).unwrap()
}

/// One peer of the match: its session, link and simulation, owning player `id`.
struct Peer {
    id: u8,
    session: Session,
    transport: Conditioned<MemoryTransport>,
    simulation: Simulation,
    executor: Executor,
    inputs: Rng,
    input: Vec<u8>,
    /// The hash of the state of each tick, as last executed.
    hashes: BTreeMap<u64, u64>,
    confirmed: Vec<(u64, Vec<Vec<u8>>)>,
    next_frame_micros: Option<u64>,
    /// The session ended and its simulation landed on the confirmed state.
    landed: bool,
}

impl Peer {
    /// Executes `requests` one at a time, recording the hash of every state an advance makes.
    fn execute(&mut self, requests: Vec<Request>) {
        for request in requests {
            let advance = matches!(request, Request::Advance { .. });
            self.executor
                .execute(&mut self.simulation, &mut self.session, vec![request])
                .unwrap_or_else(|error| panic!("peer {}: {error}", self.id));
            if advance {
                self.hashes
                    .insert(self.simulation.tick(), self.simulation.hash());
            }
        }
    }

    /// One frame: the local player's input while the session runs, then one advance.
    fn frame(&mut self, now: u64) {
        if self.session.state() == SessionState::Running {
            // A new input now and then, so predictions are right most of the time but not
            // always.
            if self.inputs.range_u32(0, 6) == 0 {
                self.input = random_input(&mut self.inputs);
            }
            self.session
                .add_local_input(self.id, self.input.clone())
                .unwrap();
        }
        let requests = self
            .session
            .advance(now)
            .expect("an active session advances");
        self.execute(requests);
        self.landed = matches!(self.session.state(), SessionState::Ended(_));
    }

    /// Once the session ended, the advance that lands the simulation, unless the advance
    /// that ended it did.
    fn land(&mut self, now: u64) {
        if self.landed || !matches!(self.session.state(), SessionState::Ended(_)) {
            return;
        }
        self.landed = true;
        let requests = self
            .session
            .advance(now)
            .expect("an ended session lands once");
        self.execute(requests);
    }
}

/// Plays one match and checks it. Returns the rollbacks and the prediction misses its
/// sessions counted.
fn play(peers: u8, threads: usize, seed: u64) -> (u64, u64) {
    let conditions = conditions(seed);
    let case = format!("{peers} peers, {threads} threads, seed {seed}, {conditions:?}");
    let reference = simulation(threads);
    let identity = MatchIdentity {
        engine: "arena-rollback-test".to_string(),
        fingerprint: reference.schema_fingerprint(),
        scene_and_assets: 0,
        tick_rate: reference.tick_rate(),
        players: (0..peers).map(PeerId).collect(),
        rules: MatchRules::default(),
    };
    let frame_micros = ONE_SECOND / u64::from(reference.tick_rate());
    let mut players: Vec<Peer> = MemoryNetwork::new(peers)
        .into_iter()
        .zip(0..peers)
        .map(|(transport, id)| {
            let settings = SessionSettings {
                local: PeerId(id),
                match_id: 1,
                identity: identity.clone(),
                timeouts: Timeouts::default(),
            };
            let mut simulation = simulation(threads);
            let executor = Executor::new(&mut simulation, &settings).unwrap();
            let session = Session::new(settings, simulation.player_input_spec().unwrap()).unwrap();
            let link = LinkConditions {
                seed: conditions.seed.wrapping_add(u64::from(id)),
                ..conditions
            };
            let mut inputs = Rng::new(seed, u64::from(id));
            Peer {
                id,
                session,
                transport: Conditioned::new(transport, link).unwrap(),
                hashes: BTreeMap::from([(0, simulation.hash())]),
                simulation,
                executor,
                input: random_input(&mut inputs),
                inputs,
                confirmed: Vec::new(),
                next_frame_micros: None,
                landed: false,
            }
        })
        .collect();

    let leaver = usize::try_from(seed % u64::from(peers)).unwrap();
    let deadline = 20 * ONE_SECOND;
    let mut now = 0;
    let mut left = false;
    while !players
        .iter()
        .all(|peer| peer.landed && matches!(peer.session.state(), SessionState::Ended(_)))
    {
        assert!(now < deadline, "{case}: the match did not end");
        if !left && players[leaver].simulation.tick() >= TICKS {
            players[leaver].session.leave().unwrap();
            left = true;
        }
        for peer in &mut players {
            while let Some((from, packet)) = peer.transport.receive(now) {
                peer.session.receive(from, &packet, now);
            }
            let active = matches!(
                peer.session.state(),
                SessionState::Running | SessionState::Leaving
            );
            if active {
                let due = *peer.next_frame_micros.get_or_insert(now);
                if now >= due {
                    peer.frame(now);
                    peer.next_frame_micros = Some(due + frame_micros);
                }
            }
            for (to, packet) in peer.session.outgoing(now) {
                peer.transport.send(to, packet, now).unwrap();
            }
            peer.land(now);
            for event in peer.session.events() {
                if let SessionEvent::Confirmed { tick, inputs } = event {
                    peer.confirmed.push((tick, inputs));
                }
            }
        }
        now += STEP_MICROS;
    }

    let SessionState::Ended(EndReason::Left { last_tick, .. }) = players[leaver].session.state()
    else {
        panic!(
            "{case}: the leaver ended {:?}",
            players[leaver].session.state()
        );
    };
    let confirmed = players[leaver].confirmed.clone();
    assert_eq!(
        confirmed.len(),
        usize::try_from(last_tick + 1).unwrap(),
        "{case}"
    );
    let mut rollbacks = 0;
    let mut misses = 0;
    for peer in &players {
        assert!(
            matches!(
                peer.session.state(),
                SessionState::Ended(EndReason::Left { last_tick: tick, .. }) if tick == last_tick
            ),
            "{case}: peer {} ended {:?}",
            peer.id,
            peer.session.state()
        );
        assert_eq!(
            peer.confirmed, confirmed,
            "{case}: peer {} confirmed other inputs",
            peer.id
        );
        for stats in peer.session.stats().peers.values() {
            rollbacks += stats.rollbacks;
            misses += stats.prediction_misses;
        }
    }

    // Offline, the confirmed inputs joined give every peer's state at every confirmed tick.
    let mut offline = reference;
    offline.set_players(usize::from(peers));
    let check = |offline: &Simulation, tick: u64| {
        for peer in &players {
            assert_eq!(
                peer.hashes.get(&tick),
                Some(&offline.hash()),
                "{case}: peer {} at tick {tick}",
                peer.id
            );
        }
    };
    check(&offline, 0);
    for (tick, inputs) in &confirmed {
        offline
            .step_encoded(Some(&join_player_inputs(inputs)))
            .unwrap_or_else(|error| panic!("{case}: tick {tick}: {error}"));
        check(&offline, tick + 1);
    }
    (rollbacks, misses)
}

#[test]
fn two_three_and_four_peers_match_an_offline_run() {
    let mut rollbacks = 0;
    let mut misses = 0;
    for threads in [1, 8] {
        for peers in 2..=4 {
            for seed in 0..seeds() {
                let (case_rollbacks, case_misses) = play(peers, threads, seed);
                rollbacks += case_rollbacks;
                misses += case_misses;
            }
        }
    }
    // Not vacuous: the matches rolled back on prediction misses.
    assert!(rollbacks > 0, "no rollback in any match");
    assert!(misses > 0, "no prediction miss in any match");
}
