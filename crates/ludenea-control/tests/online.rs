//! Online matches through the host: hosts over an in-memory network, behind a link
//! conditioner when a test says so, stepped 1/120 s per frame, each fed scripted live devices
//! from a seeded generator: a gamepad's left stick and jump button per local player.

use std::collections::BTreeSet;
use std::path::Path;

use arena::ArenaInput;
use ludenea::prelude::Schema;
use ludenea_app::players::{Claim, LiveDevices, automatic_claims};
use ludenea_app::project::project_assets;
use ludenea_app::replay::{Replay, ReplayOutcome, play_replay};
use ludenea_app::{App, InputLog, Inputs, RawGamepad, Rng, Sim};
use ludenea_control::host::RESIMULATION_BUDGET;
use ludenea_control::{
    Bytes, Command, Document, ErrorCode, Event, Hash64, Host, MatchEnd, MatchEndKind,
    OnlineSettings, OnlineState, PauseReason, ProtocolError, Reply,
};
use ludenea_ecs::Runtime;
use ludenea_net::{Conditioned, LinkConditions, MemoryNetwork, PeerId, Transport};
use serde::{Deserialize, Serialize};

/// The arena with a spawn point for each of four players.
const SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/arena/tests/fixtures/arena-four.scene.ron"
);
/// The arena's own level, two spawn points: another scene than [`SCENE`].
const OTHER_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/arena/assets/arena.scene.ron"
);
const FRAMES_PER_SECOND: u64 = 120;
const FRAME: f64 = 1.0 / 120.0;
/// The most frames a test waits for something before it fails.
const PATIENCE: u64 = 6_000;
/// The runs a stick takes.
const STICKS: [f32; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];

/// One host of the match with its link.
struct Peer {
    host: Host,
    transport: Box<dyn Transport>,
    /// Draws the devices.
    devices: Rng,
    /// The local players of this host.
    locals: usize,
    /// Every event since the host joined.
    events: Vec<Event>,
    /// Every input the mapping gave for the devices fed while the match ran.
    fed: BTreeSet<Vec<u8>>,
    /// A peer stopped for good: never stepped again.
    stopped: bool,
    /// Its packets are dropped while set.
    muted: bool,
    /// Whether it is fed random devices.
    feeding: bool,
}

impl Peer {
    fn handle(&mut self, command: Command) -> Reply {
        self.host
            .handle(command)
            .unwrap_or_else(|error| panic!("{error:?}"))
    }

    /// The latest online state the host announced.
    fn state(&self) -> Option<&OnlineState> {
        self.events.iter().rev().find_map(|event| match event {
            Event::Online { state } => Some(state),
            _ => None,
        })
    }

    fn running(&self) -> bool {
        matches!(self.state(), Some(OnlineState::Running))
    }

    fn end(&self) -> Option<&MatchEnd> {
        match self.state() {
            Some(OnlineState::Ended { end }) => Some(end),
            _ => None,
        }
    }

    /// New devices now and then, mapped as the arena maps them.
    fn feed(&mut self) {
        if !self.feeding || self.devices.range_u32(0, 8) != 0 {
            return;
        }
        let pads: Vec<Option<RawGamepad>> = (0..self.locals)
            .map(|_| {
                let stick = STICKS[self.devices.range_u32(0, 5) as usize];
                let jump = self.devices.range_u32(0, 2) == 1;
                Some(RawGamepad {
                    buttons: vec![if jump { 1.0 } else { 0.0 }],
                    axes: vec![stick],
                })
            })
            .collect();
        if self.running() {
            for pad in pads.iter().flatten() {
                let raw = ludenea_app::RawInput {
                    keys: BTreeSet::new(),
                    gamepads: vec![pad.clone()],
                };
                let input = arena::map_raw_input(&raw, 0);
                self.fed.insert(postcard::to_allocvec(&input).unwrap());
            }
        }
        self.handle(Command::SetLiveInput {
            devices: LiveDevices {
                keys: BTreeSet::new(),
                gamepads: pads,
            },
        });
    }
}

/// Hosts playing one match.
struct Match {
    peers: Vec<Peer>,
    frame: u64,
}

impl Match {
    /// One arena host per owner of `players`, on `threads` threads, each joined.
    fn arena(players: &[u8], threads: u32, conditions: Option<LinkConditions>) -> Match {
        Match::of(players, conditions, |_| {
            let mut host = arena_host(SCENE);
            host.handle(Command::SetThreads { threads }).unwrap();
            host
        })
    }

    /// One host per owner of `players`, as `make` makes it, each joined.
    fn of(players: &[u8], conditions: Option<LinkConditions>, make: impl Fn(u8) -> Host) -> Match {
        let count = players.iter().copied().max().unwrap() + 1;
        let peers = MemoryNetwork::new(count)
            .into_iter()
            .enumerate()
            .map(|(index, transport)| {
                let local = u8::try_from(index).unwrap();
                let transport: Box<dyn Transport> = match conditions {
                    Some(conditions) => Box::new(
                        Conditioned::new(
                            transport,
                            LinkConditions {
                                seed: conditions.seed + u64::from(local),
                                ..conditions
                            },
                        )
                        .unwrap(),
                    ),
                    None => Box::new(transport),
                };
                let mut peer = Peer {
                    host: make(local),
                    transport,
                    devices: Rng::new(17, u64::from(local)),
                    locals: players.iter().filter(|&&owner| owner == local).count(),
                    events: Vec::new(),
                    fed: BTreeSet::new(),
                    stopped: false,
                    muted: false,
                    feeding: true,
                };
                let reply = peer.host.handle(join(local, players));
                assert_eq!(reply, Ok(Reply::Done));
                peer.events.extend(peer.host.advance(0.0));
                peer
            })
            .collect();
        Match { peers, frame: 0 }
    }

    fn now(&self) -> u64 {
        self.frame * 1_000_000 / FRAMES_PER_SECOND
    }

    /// One frame of every peer still stepped: devices, the packets that arrived, an advance,
    /// and the packets it sends.
    fn step(&mut self) {
        self.frame += 1;
        let now = self.now();
        for peer in self.peers.iter_mut().filter(|peer| !peer.stopped) {
            peer.feed();
            while let Some((from, packet)) = peer.transport.receive(now) {
                let reply = peer.host.handle(Command::NetReceive {
                    from: from.0,
                    packet: Bytes(packet),
                });
                assert_eq!(reply, Ok(Reply::Done));
            }
            let events = peer.host.advance(FRAME);
            let ended = events.iter().any(|event| {
                matches!(
                    event,
                    Event::Online {
                        state: OnlineState::Ended { .. }
                    }
                )
            });
            if ended {
                peer.muted = false;
            }
            for event in &events {
                if let Event::NetSend { packets } = event
                    && !peer.muted
                {
                    for packet in packets {
                        peer.transport
                            .send(PeerId(packet.to), packet.packet.0.clone(), now)
                            .unwrap();
                    }
                }
            }
            peer.events.extend(events);
        }
    }

    /// Steps until `done`, failing after [`PATIENCE`] frames.
    fn run_until(&mut self, what: &str, done: impl Fn(&Match) -> bool) {
        let start = self.frame;
        while !done(self) {
            assert!(
                self.frame - start < PATIENCE,
                "{what}: still waiting after {PATIENCE} frames, states {:?}",
                self.peers.iter().map(Peer::state).collect::<Vec<_>>()
            );
            self.step();
        }
    }

    fn run_until_all_run(&mut self) {
        self.run_until("every peer runs", |game| {
            game.peers.iter().all(Peer::running)
        });
    }

    fn run_until_all_end(&mut self) {
        self.run_until("every peer ends", |game| {
            game.peers
                .iter()
                .filter(|peer| !peer.stopped)
                .all(|peer| peer.end().is_some())
        });
    }

    /// Runs until `peer` reaches `ticks`, then has it leave.
    fn leave_at(&mut self, peer: usize, ticks: u64) {
        self.run_until("the leaver reaches its tick", |game| {
            game.peers[peer].host.tick() >= ticks
        });
        assert_eq!(
            self.peers[peer].host.handle(Command::LeaveMatch),
            Ok(Reply::Done)
        );
        let events = self.peers[peer].host.advance(0.0);
        self.peers[peer].events.extend(events);
        assert_eq!(self.peers[peer].state(), Some(&OnlineState::Leaving));
    }
}

fn join(local: u8, players: &[u8]) -> Command {
    let locals = players.iter().filter(|&&owner| owner == local).count();
    Command::JoinMatch {
        settings: OnlineSettings {
            local,
            match_id: Hash64(0x5eed),
            players: players.to_vec(),
        },
        claims: automatic_claims(locals),
    }
}

fn arena_host(scene: &str) -> Host {
    let mut host = Host::new(arena::app).unwrap();
    let text = std::fs::read_to_string(scene).unwrap();
    let assets = project_assets(Path::new(scene), None).unwrap();
    host.handle(Command::LoadScene {
        scene: text,
        assets: assets.texts.into_iter().collect(),
        document: Document::Scene,
    })
    .unwrap();
    host
}

/// Advances by zero until the adopted session no longer resimulates; returns the advances.
fn resimulate(host: &mut Host) -> u32 {
    let mut advances = 0;
    while timeline_resimulating(host) {
        host.advance(0.0);
        advances += 1;
        assert!(advances < 1_000, "the session never stops resimulating");
    }
    advances
}

fn timeline_resimulating(host: &mut Host) -> bool {
    let Ok(Reply::Timeline(timeline)) = host.handle(Command::GetTimeline) else {
        panic!("a timeline");
    };
    timeline.resimulating
}

fn replay_of(host: &mut Host) -> Vec<u8> {
    let Ok(Reply::Replay { replay }) = host.handle(Command::ExportReplay { branch: None }) else {
        panic!("a replay");
    };
    replay.0
}

/// Checks that `replay` plays back on a fresh arena.
fn verify(replay: &[u8]) -> Replay {
    let replay = Replay::decode(replay).unwrap();
    let mut sim = arena::app().build().unwrap();
    let report = play_replay(&mut sim, &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    replay
}

/// The hash `Ticked` gave for `tick` last before the match ended.
fn landed_hash(peer: &Peer, tick: u64) -> Hash64 {
    let ended = peer
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                Event::Online {
                    state: OnlineState::Ended { .. }
                }
            )
        })
        .expect("the match ended");
    peer.events[..ended]
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::Ticked { tick: at, hash, .. } if *at == tick => Some(*hash),
            _ => None,
        })
        .expect("a ticked event of the landed tick")
}

/// Every player's input of every tick of `replay`.
fn replay_inputs(replay: &Replay) -> Vec<Vec<ArenaInput>> {
    let log = InputLog::decode(&replay.inputs).unwrap();
    log.ticks
        .iter()
        .map(|tick| postcard::from_bytes(tick).unwrap())
        .collect()
}

/// Plays a match of `players` where the last peer leaves at `ticks`; checks every peer ends
/// `left` at the same tick, lingers, resimulates, and records the same verifiable replay
/// holding the confirmed inputs. Returns the match.
fn play_and_leave(
    players: &[u8],
    threads: u32,
    conditions: Option<LinkConditions>,
    ticks: u64,
) -> Match {
    let mut game = Match::arena(players, threads, conditions);
    game.run_until_all_run();
    let leaver = game.peers.len() - 1;
    game.leave_at(leaver, ticks);
    game.run_until_all_end();

    let ends: Vec<MatchEnd> = game
        .peers
        .iter()
        .map(|peer| peer.end().unwrap().clone())
        .collect();
    for end in &ends {
        assert_eq!(end.kind, MatchEndKind::Left, "{end:?}");
        assert_eq!(end.peer, Some(u8::try_from(leaver).unwrap()));
        assert_eq!(end.tick, ends[0].tick);
    }
    let landed = ends[0].tick.unwrap() + 1;
    // Play stopped with the match, as every stop says.
    for peer in &game.peers {
        assert_eq!(match_ended_pauses(peer), 1);
    }

    // Lingering: each peer keeps sending.
    let ended_at: Vec<usize> = game.peers.iter().map(|peer| peer.events.len()).collect();
    for _ in 0..10 {
        game.step();
    }
    for (peer, from) in game.peers.iter().zip(ended_at) {
        assert!(
            peer.events[from..]
                .iter()
                .any(|event| matches!(event, Event::NetSend { .. })),
            "a lingering peer sends"
        );
    }

    let mut replays = Vec::new();
    for peer in &mut game.peers {
        resimulate(&mut peer.host);
        assert_eq!(peer.host.tick(), landed);
        replays.push(replay_of(&mut peer.host));
    }
    for replay in &replays {
        assert!(replay == &replays[0], "the replays differ");
    }
    let replay = verify(&replays[0]);
    assert_eq!(replay.hashes.len() as u64, landed + 1);
    for peer in &game.peers {
        assert_eq!(
            Hash64(replay.hashes[landed as usize]),
            landed_hash(peer, landed)
        );
    }
    let inputs = replay_inputs(&replay);
    assert_eq!(inputs.len() as u64, landed);
    let fed: BTreeSet<Vec<u8>> = game
        .peers
        .iter()
        .flat_map(|peer| peer.fed.iter().cloned())
        .chain([postcard::to_allocvec(&ArenaInput::default()).unwrap()])
        .collect();
    for tick in &inputs {
        assert_eq!(tick.len(), players.len());
        for input in tick {
            assert!(
                fed.contains(&postcard::to_allocvec(input).unwrap()),
                "{input:?}"
            );
        }
    }
    // The input delay's ticks hold the default inputs.
    assert!(
        inputs[..2]
            .iter()
            .flatten()
            .all(|input| *input == ArenaInput::default())
    );
    game
}

#[test]
fn two_hosts_play_and_record_the_same_replay() {
    for threads in [1, 8] {
        play_and_leave(&[0, 1], threads, None, 300);
    }
}

#[test]
fn the_adopted_session_resimulates_in_the_background() {
    let mut game = Match::arena(&[0, 1], 1, None);
    game.run_until_all_run();
    game.leave_at(1, 600);
    game.run_until_all_end();
    let peer = &mut game.peers[0];
    let landed = peer.end().unwrap().tick.unwrap() + 1;
    assert!(landed > u64::from(RESIMULATION_BUDGET));

    // Right after the end, the world shows tick 0.
    assert!(timeline_resimulating(&mut peer.host));
    let Ok(Reply::Hash { tick, .. }) = peer.host.handle(Command::GetHash) else {
        panic!("a hash");
    };
    assert_eq!(tick, 0);

    let advances = resimulate(&mut peer.host);
    assert!(
        u64::from(advances) <= landed / u64::from(RESIMULATION_BUDGET) + 2,
        "{advances} advances"
    );
    let Ok(Reply::Hash { tick, hash, .. }) = peer.host.handle(Command::GetHash) else {
        panic!("a hash");
    };
    assert_eq!(tick, landed);
    assert_eq!(hash, landed_hash(peer, landed));
}

fn lossy(seed: u64) -> LinkConditions {
    LinkConditions {
        latency_micros: 100_000,
        jitter_micros: 0,
        loss_per_mille: 50,
        duplicate_per_mille: 0,
        reorder_per_mille: 0,
        reorder_extra_micros: 0,
        seed,
    }
}

#[test]
fn three_hosts_under_latency_and_loss_record_the_same_replay() {
    play_and_leave(&[0, 1, 2], 1, Some(lossy(3)), 300);
}

#[test]
fn two_local_players_and_one_remote_play() {
    play_and_leave(&[0, 0, 1], 1, None, 200);
    play_and_leave(&[0, 1, 1], 1, None, 200);
}

#[test]
fn rollbacks_are_announced_and_ticks_stay_in_order() {
    let game = play_and_leave(
        &[0, 1],
        1,
        Some(LinkConditions {
            loss_per_mille: 0,
            ..lossy(5)
        }),
        300,
    );
    let mut rolled_back = 0;
    for peer in &game.peers {
        let running = peer
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    Event::Online {
                        state: OnlineState::Running
                    }
                )
            })
            .unwrap();
        let ended = peer
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    Event::Online {
                        state: OnlineState::Ended { .. }
                    }
                )
            })
            .unwrap();
        let mut ticks = Vec::new();
        for event in &peer.events[running..ended] {
            match event {
                Event::Ticked { tick, .. } => ticks.push(*tick),
                Event::RolledBack { from, to } => {
                    assert!(from < to, "rolled back from {from} to {to}");
                    rolled_back += 1;
                }
                _ => {}
            }
        }
        // The last one is the landing.
        ticks.pop();
        for pair in ticks.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "{ticks:?}");
        }
    }
    assert!(rolled_back > 0);
}

#[test]
fn a_mismatched_scene_ends_both_hosts_while_connecting() {
    let mut game = Match::of(&[0, 1], None, |local| {
        arena_host(if local == 0 { SCENE } else { OTHER_SCENE })
    });
    game.run_until_all_end();
    for peer in &mut game.peers {
        let end = peer.end().unwrap();
        assert_eq!(end.kind, MatchEndKind::Mismatch, "{end:?}");
        assert!(end.message.contains("scene_and_assets"), "{}", end.message);
        assert_eq!(end.tick, None);
        let text = std::fs::read_to_string(OTHER_SCENE).unwrap();
        let assets = project_assets(Path::new(OTHER_SCENE), None).unwrap();
        peer.handle(Command::LoadScene {
            scene: text,
            assets: assets.texts.into_iter().collect(),
            document: Document::Scene,
        });
    }
    assert_eq!(game.peers[0].end().unwrap().peer, Some(1));
    assert_eq!(game.peers[1].end().unwrap().peer, Some(0));
}

#[test]
fn an_unreachable_peer_ends_connecting() {
    let mut game = Match::arena(&[0, 1], 1, None);
    game.peers[1].stopped = true;
    game.run_until_all_end();
    let end = game.peers[0].end().unwrap();
    assert_eq!(end.kind, MatchEndKind::Unreachable);
    assert_eq!(end.peer, Some(1));
    assert_eq!(
        end.message,
        "peer 1 never answered while the match was starting"
    );
}

#[test]
fn leaving_while_connecting_drops_the_match() {
    let mut game = Match::arena(&[0, 1], 1, None);
    let peer = &mut game.peers[0];
    assert_eq!(peer.host.handle(Command::LeaveMatch), Ok(Reply::Done));
    peer.events.extend(peer.host.advance(0.0));
    let end = peer.end().unwrap();
    assert_eq!(end.kind, MatchEndKind::Left);
    assert_eq!(end.message, "peer 0 left before the match started");
    assert_eq!(end.peer, Some(0));
    assert_eq!(end.tick, None);
    assert_eq!(
        peer.host.handle(Command::LeaveMatch).unwrap_err().code,
        ErrorCode::InvalidCommand
    );

    game.run_until_all_end();
    let end = game.peers[1].end().unwrap();
    assert_eq!(end.kind, MatchEndKind::Unreachable, "{end:?}");
    assert_eq!(end.peer, Some(0));
}

#[test]
fn a_lost_peer_leaves_each_host_a_verifiable_replay() {
    let mut game = Match::arena(&[0, 1], 1, None);
    game.run_until_all_run();
    game.run_until("some ticks", |game| game.peers[0].host.tick() >= 120);
    game.peers[1].stopped = true;
    game.run_until_all_end();
    let end = game.peers[0].end().unwrap();
    assert_eq!(end.kind, MatchEndKind::Lost, "{end:?}");
    assert_eq!(end.peer, Some(1));
    let host = &mut game.peers[0].host;
    resimulate(host);
    verify(&replay_of(host));
}

/// The input byte the failing game cannot run.
const POISON: u8 = 0xFF;

fn fail_on_poison(inputs: Runtime<Inputs<u8>>) {
    assert!(
        (0..inputs.len()).all(|player| *inputs.player(player) != POISON),
        "poisoned input"
    );
}

/// `KeyX` gives the poison.
fn failing_app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(|raw, _| if raw.keys.contains("KeyX") { POISON } else { 1 })
        .system(Sim, fail_on_poison);
    app
}

#[test]
fn a_failing_game_ends_both_hosts() {
    let mut game = Match::of(&[0, 1], None, |_| Host::new(failing_app).unwrap());
    game.run_until_all_run();
    game.run_until("some ticks", |game| game.peers[0].host.tick() >= 30);
    // Peer 0's packets are lost until its game fails, so the other one never runs the
    // poison and learns of the failure from the end notice alone.
    game.peers[0].muted = true;
    game.peers[0].handle(Command::SetLiveInput {
        devices: LiveDevices {
            keys: ["KeyX".to_string()].into(),
            gamepads: Vec::new(),
        },
    });
    game.peers[0].feeding = false;
    game.run_until_all_end();

    let failing = &game.peers[0];
    let end = failing.end().unwrap();
    assert_eq!(end.kind, MatchEndKind::LocalFailure, "{end:?}");
    assert_eq!(end.peer, Some(0));
    assert!(failing.events.iter().any(|event| matches!(
        event,
        Event::Paused {
            reason: PauseReason::Panicked { .. }
        }
    )));
    let other = game.peers[1].end().unwrap();
    assert_eq!(other.kind, MatchEndKind::LocalFailure, "{other:?}");
    assert_eq!(other.peer, Some(0));
    assert_eq!(other.tick, end.tick);
    // The failure paused the failing host already; the other one stops as the match ends.
    assert_eq!(match_ended_pauses(failing), 0);
    assert_eq!(match_ended_pauses(&game.peers[1]), 1);
}

/// How many times `peer` paused because its match ended.
fn match_ended_pauses(peer: &Peer) -> usize {
    peer.events
        .iter()
        .filter(|event| {
            matches!(
                event,
                Event::Paused {
                    reason: PauseReason::MatchEnded
                }
            )
        })
        .count()
}

#[derive(Serialize, Deserialize, Schema, Default, Clone, Debug, PartialEq)]
struct Big {
    bytes: Vec<u8>,
}

/// `KeyB` gives an input of 31 bytes.
fn big_app() -> App {
    let mut app = App::new();
    app.input::<Big>().input_mapping(|raw, _| Big {
        bytes: if raw.keys.contains("KeyB") {
            vec![7; 30]
        } else {
            Vec::new()
        },
    });
    app
}

#[test]
fn an_input_too_large_for_the_network_is_refused() {
    let mut game = Match::of(&[0, 1], None, |_| Host::new(big_app).unwrap());
    game.run_until_all_run();
    let error = game.peers[0]
        .host
        .handle(Command::SetLiveInput {
            devices: LiveDevices {
                keys: ["KeyB".to_string()].into(),
                gamepads: Vec::new(),
            },
        })
        .unwrap_err();
    assert_eq!(
        error,
        ProtocolError {
            code: ErrorCode::InvalidInputs,
            message: "player 0's input takes 31 bytes, a network match carries at most 24"
                .to_string(),
        }
    );
    // The match goes on with the previous inputs.
    let tick = game.peers[0].host.tick();
    game.run_until("more ticks", |game| game.peers[0].host.tick() >= tick + 60);
    assert!(game.peers.iter().all(Peer::running));
}

#[derive(Serialize, Deserialize, Schema, Clone, Debug, PartialEq)]
struct Wide {
    a: u64,
    b: u64,
    c: u64,
}

impl Default for Wide {
    fn default() -> Self {
        Wide {
            a: u64::MAX,
            b: u64::MAX,
            c: u64::MAX,
        }
    }
}

fn wide_app() -> App {
    let mut app = App::new();
    app.input::<Wide>().input_mapping(|_, _| Wide::default());
    app
}

fn unmapped_app() -> App {
    let mut app = App::new();
    app.input::<u8>();
    app
}

fn inputless_app() -> App {
    App::new()
}

fn refusal(host: &mut Host, command: Command) -> ProtocolError {
    host.handle(command).unwrap_err()
}

#[test]
fn join_match_refuses() {
    let mut host = arena_host(SCENE);
    // A match is on.
    host.handle(join(0, &[0, 1])).unwrap();
    let error = refusal(&mut host, join(0, &[0, 1]));
    assert_eq!(error.code, ErrorCode::OnlineMatch);
    assert_eq!(
        error.message,
        "`join_match` is refused while an online match runs: it would change this peer's game \
         alone or read the session the match records at its end; leave the match first"
    );

    // Claims.
    let mut host = arena_host(SCENE);
    let Command::JoinMatch { settings, .. } = join(0, &[0, 1]) else {
        unreachable!()
    };
    let error = refusal(
        &mut host,
        Command::JoinMatch {
            settings: settings.clone(),
            claims: automatic_claims(2),
        },
    );
    assert_eq!(error.code, ErrorCode::InvalidInputs);
    assert_eq!(
        error.message,
        "2 claims for 1 local players: give one claim per local player"
    );
    let twice = Claim {
        keyboard: None,
        gamepad: Some(0),
    };
    let error = refusal(
        &mut host,
        Command::JoinMatch {
            settings: OnlineSettings {
                players: vec![0, 0, 1],
                ..settings.clone()
            },
            claims: vec![twice.clone(), twice],
        },
    );
    assert_eq!(error.code, ErrorCode::InvalidInputs);
    assert_eq!(error.message, "players 0 and 1 both claim gamepad 0");

    // Settings.
    let error = refusal(&mut host, join(0, &[0, 0]));
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert_eq!(
        error.message,
        "every player is owned by peer 0: a rollback session needs at least two peers, a \
         single-peer game steps its Simulation directly"
    );
    let error = refusal(&mut host, join(2, &[0, 1]));
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert_eq!(
        error.message,
        "local peer 2 is not among the 2 peers of the match"
    );

    // Edits after tick 0.
    host.handle(Command::Step {
        ticks: 3,
        record_hashes: false,
    })
    .unwrap();
    host.handle(Command::Spawn {
        components: Default::default(),
    })
    .unwrap();
    let error = refusal(&mut host, join(0, &[0, 1]));
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert_eq!(
        error.message,
        "the session holds edits after tick 0, which the other peers do not have; load the \
         scene without them"
    );

    // The game.
    let mut host = Host::new(wide_app).unwrap();
    let error = refusal(&mut host, join(0, &[0, 1]));
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert_eq!(
        error.message,
        "the default input takes 30 bytes, at most 24 allowed"
    );
    let mut host = Host::new(inputless_app).unwrap();
    let error = refusal(&mut host, join(0, &[0, 1]));
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert_eq!(
        error.message,
        "this app has no input type; register one with `App::input` before running a session"
    );
    let mut host = Host::new(unmapped_app).unwrap();
    let error = refusal(&mut host, join(0, &[0, 1]));
    assert_eq!(error.code, ErrorCode::NoInputMapping);

    // A refused join changes nothing: the session plays on.
    let mut host = arena_host(SCENE);
    host.handle(Command::Step {
        ticks: 5,
        record_hashes: false,
    })
    .unwrap();
    refusal(&mut host, join(0, &[0, 0]));
    assert_eq!(host.tick(), 5);
}

#[test]
fn packets_without_a_match_are_ignored() {
    let mut host = arena_host(SCENE);
    host.advance(0.0);
    let Ok(Reply::Hash { tick, hash, .. }) = host.handle(Command::GetHash) else {
        panic!("a hash");
    };
    assert_eq!(
        host.handle(Command::NetReceive {
            from: 1,
            packet: Bytes(vec![1, 2, 3]),
        }),
        Ok(Reply::Done)
    );
    assert_eq!(
        host.handle(Command::GetHash),
        Ok(Reply::Hash {
            tick,
            hash,
            pending: false
        })
    );
    assert_eq!(host.advance(FRAME), Vec::new());
    let error = refusal(&mut host, Command::LeaveMatch);
    assert_eq!(error.code, ErrorCode::InvalidCommand);
    assert_eq!(error.message, "there is no online match to leave");
}

#[test]
fn local_changes_are_refused_during_a_match() {
    let mut game = Match::arena(&[0, 1], 1, None);
    game.run_until_all_run();
    let refused = [
        Command::SetPlayers {
            players: 2,
            claims: None,
        },
        Command::Play,
        Command::GetTimeline,
    ];
    for command in refused.clone() {
        let error = refusal(&mut game.peers[0].host, command);
        assert_eq!(error.code, ErrorCode::OnlineMatch, "{}", error.message);
        assert!(error.message.ends_with("leave the match first"));
    }
    // Reads of the world are allowed.
    game.peers[0].handle(Command::GetHash);
    game.peers[0].handle(Command::GetResources);

    game.leave_at(1, 60);
    game.run_until_all_end();
    for command in refused.into_iter().rev() {
        game.peers[0].handle(command);
    }
}

#[test]
fn live_input_after_the_end_uses_the_full_claims() {
    let mut game = Match::arena(&[0, 0, 1], 1, None);
    game.run_until_all_run();
    game.leave_at(1, 60);
    game.run_until_all_end();
    let peer = &mut game.peers[1];
    resimulate(&mut peer.host);
    let at = peer.host.tick();

    // Under the automatic claims of the three players, gamepad 2 is the last player's.
    let pad = Some(RawGamepad {
        buttons: vec![0.0],
        axes: vec![1.0],
    });
    peer.handle(Command::SetLiveInput {
        devices: LiveDevices {
            keys: BTreeSet::new(),
            gamepads: vec![None, None, pad],
        },
    });
    peer.handle(Command::Play);
    while peer.host.tick() == at {
        peer.host.advance(FRAME);
    }
    let Ok(Reply::Session { inputs, .. }) = peer.host.handle(Command::ExportSession) else {
        panic!("a session");
    };
    let log = InputLog::decode(&inputs.0).unwrap();
    let inputs: Vec<ArenaInput> = postcard::from_bytes(&log.ticks[at as usize]).unwrap();
    assert_eq!(
        inputs,
        vec![
            ArenaInput::default(),
            ArenaInput::default(),
            ArenaInput {
                run: 1.0,
                jump: false
            }
        ]
    );
}

thread_local! {
    /// The next serialization of a `Bomb` on this thread panics.
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A component whose serialization panics once armed, as a type's own code may: the host
/// catches it where nothing reports it more precisely.
#[derive(Clone, ludenea_ecs::Component, Deserialize)]
struct Bomb(u32);

impl Serialize for Bomb {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        assert!(
            !ARMED.with(|armed| armed.replace(false)),
            "the bomb went off"
        );
        serializer.serialize_newtype_struct("Bomb", &self.0)
    }
}

fn bomb_app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(|_, _| 1_u8)
        .component::<Bomb>();
    app
}

fn bomb_host(_: u8) -> Host {
    let mut host = Host::new(bomb_app).unwrap();
    host.handle(Command::LoadScene {
        scene: r#"Scene(version: 1, resources: {}, entities: [{ "online::Bomb": (3) }])"#.into(),
        assets: Vec::new(),
        document: Document::Scene,
    })
    .unwrap();
    host
}

/// Has peer 0 read its bomb, armed: the host catches the panic.
fn set_off(game: &mut Match) {
    let host = &mut game.peers[0].host;
    let Ok(Reply::Entities { rows, .. }) = host.handle(Command::QueryEntities {
        filter: String::new(),
        offset: 0,
        limit: 1,
    }) else {
        panic!("entities");
    };
    ARMED.with(|armed| armed.set(true));
    let error = host
        .handle(Command::GetEntity {
            entity: rows[0].entity,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::SystemPanicked, "{}", error.message);
}

#[test]
fn a_panic_the_host_catches_while_starting_drops_the_match() {
    let mut game = Match::of(&[0, 1], None, bomb_host);
    set_off(&mut game);
    game.step();
    let end = game.peers[0].end().unwrap();
    assert_eq!(end.kind, MatchEndKind::LocalFailure);
    assert_eq!(
        end.message,
        "the game of peer 0 failed while the match was starting"
    );
    assert_eq!((end.peer, end.tick), (Some(0), None));
}

#[test]
fn a_panic_the_host_catches_while_running_ends_both_hosts() {
    let mut game = Match::of(&[0, 1], None, bomb_host);
    game.run_until_all_run();
    game.run_until("some ticks", |game| game.peers[0].host.tick() >= 40);
    let last = game.peers[0].host.tick() - 1;
    set_off(&mut game);
    // The host finished the match after it recovered: the confirmed inputs resimulate.
    assert!(timeline_resimulating(&mut game.peers[0].host));
    game.run_until_all_end();

    let end = game.peers[0].end().unwrap();
    assert_eq!(end.kind, MatchEndKind::LocalFailure, "{end:?}");
    assert_eq!((end.peer, end.tick), (Some(0), Some(last)));
    let other = game.peers[1].end().unwrap();
    assert_eq!(other.kind, MatchEndKind::LocalFailure, "{other:?}");
    assert_eq!((other.peer, other.tick), (Some(0), Some(last)));

    // The confirmed inputs resimulated to their end.
    let host = &mut game.peers[0].host;
    resimulate(host);
    let Ok(Reply::Session { inputs, .. }) = host.handle(Command::ExportSession) else {
        panic!("a session");
    };
    let confirmed = InputLog::decode(&inputs.0).unwrap().ticks.len() as u64;
    assert!(confirmed > 0);
    assert_eq!(host.tick(), confirmed);

    host.handle(Command::LoadScene {
        scene: r#"Scene(version: 1, resources: {}, entities: [])"#.into(),
        assets: Vec::new(),
        document: Document::Scene,
    })
    .unwrap();
}

#[test]
fn a_fork_during_the_adopted_resimulation_keeps_the_match_on_its_branch() {
    let mut game = Match::arena(&[0, 1], 1, None);
    game.run_until_all_run();
    game.leave_at(1, 600);
    game.run_until_all_end();
    let peer = &mut game.peers[0];
    let landed = peer.end().unwrap().tick.unwrap() + 1;
    let hash = landed_hash(peer, landed);
    let host = &mut peer.host;
    let Ok(Reply::Timeline(timeline)) = host.handle(Command::GetTimeline) else {
        panic!("a timeline");
    };
    let root = timeline.branch;

    // A take-over at tick 0 while the confirmed inputs resimulate forks a branch.
    host.advance(0.0);
    assert!(timeline_resimulating(host));
    host.handle(Command::SetLiveInput {
        devices: LiveDevices {
            keys: BTreeSet::new(),
            gamepads: vec![Some(RawGamepad {
                buttons: vec![0.0],
                axes: vec![1.0],
            })],
        },
    })
    .unwrap();
    host.handle(Command::Play).unwrap();
    // One tick due: the advance resimulates first, then the take-over runs tick 0.
    host.advance(1.0 / 60.0);
    assert_eq!(host.tick(), 1);
    host.handle(Command::Pause).unwrap();
    let Ok(Reply::Timeline(timeline)) = host.handle(Command::GetTimeline) else {
        panic!("a timeline");
    };
    assert_ne!(timeline.branch, root);

    // Back on the match's branch, it resimulates through its last confirmed tick.
    host.handle(Command::SwitchBranch { branch: root }).unwrap();
    resimulate(host);
    host.handle(Command::Seek { tick: landed }).unwrap();
    let Ok(Reply::Hash {
        tick, hash: got, ..
    }) = host.handle(Command::GetHash)
    else {
        panic!("a hash");
    };
    assert_eq!((tick, got), (landed, hash));
}

/// An input whose encoding panics once armed, as a type's own code may.
#[derive(Deserialize, Schema, Clone, Default, Debug, PartialEq)]
struct Fragile(u8);

impl Serialize for Fragile {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        assert!(!ARMED.with(|armed| armed.replace(false)), "the input broke");
        serializer.serialize_newtype_struct("Fragile", &self.0)
    }
}

fn fragile_app() -> App {
    let mut app = App::new();
    app.input::<Fragile>().input_mapping(|_, _| Fragile(1));
    app
}

#[test]
fn a_join_whose_default_input_fails_changes_nothing() {
    let mut host = Host::new(fragile_app).unwrap();
    host.handle(Command::SetPlayers {
        players: 3,
        claims: None,
    })
    .unwrap();
    host.advance(0.0);
    ARMED.with(|armed| armed.set(true));
    let error = refusal(&mut host, join(0, &[0, 1]));
    ARMED.with(|armed| armed.set(false));
    assert_eq!(error.code, ErrorCode::SystemPanicked, "{}", error.message);
    assert!(
        error.message.contains("the input broke"),
        "{}",
        error.message
    );
    // Refused before anything changed: no recovery, no pause, the players stay.
    let events = host.advance(0.0);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Online { .. } | Event::Paused { .. })),
        "{events:?}"
    );
    let Ok(Reply::Session { inputs, .. }) = host.handle(Command::ExportSession) else {
        panic!("a session");
    };
    assert_eq!(InputLog::decode(&inputs.0).unwrap().players, 3);
}

#[test]
fn frames_are_pushed_while_the_match_plays_and_at_its_end() {
    let mut game = Match::arena(&[0, 1], 1, None);
    game.run_until_all_run();
    game.peers[0].handle(Command::Subscribe {
        topic: ludenea_control::Topic::RenderFrames,
    });
    game.leave_at(1, 120);
    game.run_until_all_end();
    let peer = &game.peers[0];
    let landed = peer.end().unwrap().tick.unwrap() + 1;
    let ended = peer
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                Event::Online {
                    state: OnlineState::Ended { .. }
                }
            )
        })
        .unwrap();
    let pushed: Vec<u64> = peer.events[..ended]
        .iter()
        .filter_map(|event| match event {
            Event::RenderFrames { frames, .. } => frames.last().map(|frame| frame.tick),
            _ => None,
        })
        .collect();
    assert!(pushed.len() > 100, "{} pushes", pushed.len());
    // The landed state is drawn too.
    assert_eq!(pushed.last(), Some(&landed), "{pushed:?}");
}
