//! Runners playing online through `OnlineMatch`, over an in-memory mesh of links, stepped
//! 1/120 s per frame.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

use ludenea::app::players::{LiveDevices, automatic_claims};
use ludenea::app::project::project_assets;
use ludenea::app::replay::{Replay, ReplayOutcome, play_replay};
use ludenea::app::{RawGamepad, Rng};
use ludenea::control::{
    Bytes, Command, ConsoleLevel, Host, MatchEnd, MatchEndKind, OnlineState, Reply,
};
use ludenea::net::{
    Channel, Link, LinkConditions, LinkError, LinkEvent, LinkId, MemoryMesh, RoomCode, RoomRole,
};
use ludenea::prelude::*;
use ludenea_runtime::online::{
    OnlineMatch, OnlineOptions, OnlinePhase, OnlineRole, write_desync_snapshot,
};
use ludenea_runtime::{GameFiles, Runner, RuntimeError};
use serde::{Deserialize, Serialize};

/// The arena with a spawn point for each of four players.
const SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/arena/tests/fixtures/arena-four.scene.ron"
);
const FRAME: f64 = 1.0 / 120.0;
const FRAME_MICROS: u64 = 1_000_000 / 120;
/// The most frames a test waits for something before it fails.
const PATIENCE: u64 = 6_000;
const SIGNALING: &str = "ws://127.0.0.1:3536/rooms";

fn arena_files() -> GameFiles {
    let assets = project_assets(Path::new(SCENE), None).expect("the fixture's assets read");
    GameFiles {
        scene: std::fs::read_to_string(SCENE).expect("the fixture is committed"),
        assets: assets.texts.into_iter().collect(),
        edits: None,
    }
}

fn options(role: OnlineRole) -> OnlineOptions {
    OnlineOptions {
        role,
        signaling: SIGNALING.to_string(),
        stun: Vec::new(),
        conditions: LinkConditions {
            latency_micros: 0,
            jitter_micros: 0,
            loss_per_mille: 0,
            duplicate_per_mille: 0,
            reorder_per_mille: 0,
            reorder_extra_micros: 0,
            seed: 0,
        },
    }
}

/// One peer: its runner, its match, every phase it reported, and the devices it plays.
struct Side {
    runner: Runner,
    online: OnlineMatch,
    phases: Vec<OnlinePhase>,
    devices: Rng,
    /// What the players hold, in place of the random devices.
    held: Option<LiveDevices>,
}

impl Side {
    fn new(app: fn() -> App, files: &GameFiles, link: Box<dyn Link>, role: OnlineRole) -> Side {
        let runner =
            Runner::load(app, files, Some(1), Vec::new()).unwrap_or_else(|e| panic!("{e}"));
        let plan = options(role)
            .plan(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210)
            .unwrap_or_else(|e| panic!("{e}"));
        let mut online = OnlineMatch::new(link, &plan).unwrap_or_else(|e| panic!("{e}"));
        online
            .ready(automatic_claims(1))
            .unwrap_or_else(|e| panic!("{e}"));
        Side {
            runner,
            online,
            phases: Vec::new(),
            devices: Rng::new(5, 0),
            held: None,
        }
    }

    /// One frame at `now`: the room and the packets received, the runner's frame once it
    /// joined, and the packets it sends.
    fn frame(&mut self, now: u64) {
        let phase = self.online.before_frame(&mut self.runner, now);
        if matches!(phase, OnlinePhase::Playing | OnlinePhase::Ended(_)) {
            let devices = self.next_devices();
            self.runner
                .frame(FRAME, &devices)
                .unwrap_or_else(|e| panic!("{e}"));
        }
        self.online.after_frame(&mut self.runner, now);
        self.phases.push(phase);
    }

    fn next_devices(&mut self) -> LiveDevices {
        if let Some(held) = &self.held {
            return held.clone();
        }
        let stick = [-1.0, 0.0, 1.0][self.devices.range_u32(0, 3) as usize];
        LiveDevices {
            keys: BTreeSet::new(),
            gamepads: vec![Some(RawGamepad {
                buttons: vec![if self.devices.range_u32(0, 2) == 1 {
                    1.0
                } else {
                    0.0
                }],
                axes: vec![stick],
            })],
        }
    }

    fn end(&self) -> Option<&MatchEnd> {
        self.phases.iter().rev().find_map(|phase| match phase {
            OnlinePhase::Ended(end) => Some(end),
            _ => None,
        })
    }

    fn playing(&self) -> bool {
        matches!(self.phases.last(), Some(OnlinePhase::Playing))
    }

    fn done(&self, now: u64) -> bool {
        self.end().is_some() && !self.online.lingering(now)
    }
}

/// Frames every side until `done`, failing after [`PATIENCE`] frames.
fn run_until(sides: &mut [Side], frame: &mut u64, what: &str, done: impl Fn(&[Side], u64) -> bool) {
    let start = *frame;
    while !done(sides, *frame * FRAME_MICROS) {
        assert!(
            *frame - start < PATIENCE,
            "{what}: still waiting after {PATIENCE} frames, last phases {:?}",
            sides
                .iter()
                .map(|side| side.phases.last())
                .collect::<Vec<_>>()
        );
        *frame += 1;
        for side in sides.iter_mut() {
            side.frame(*frame * FRAME_MICROS);
        }
    }
}

/// A host and a joiner of the same room on `mesh`, running `host_app` and `join_app`.
fn pair(
    mesh: &MemoryMesh,
    files: &GameFiles,
    host_app: fn() -> App,
    join_app: fn() -> App,
) -> Vec<Side> {
    let host = Side::new(
        host_app,
        files,
        Box::new(mesh.join()),
        OnlineRole::Host {
            peers: 2,
            code: None,
        },
    );
    let code = host.online.code().clone();
    let joiner = Side::new(
        join_app,
        files,
        Box::new(mesh.join()),
        OnlineRole::Join { code },
    );
    vec![host, joiner]
}

fn verify_arena(replay: &[u8]) -> Replay {
    let replay = Replay::decode(replay).unwrap();
    let mut sim = arena::app().build().unwrap();
    let report = play_replay(&mut sim, &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    replay
}

#[test]
fn two_runners_play_online_and_export_the_same_replay() {
    let mesh = MemoryMesh::new();
    let files = arena_files();
    let mut sides = pair(&mesh, &files, arena::app, arena::app);
    let mut frame = 0;
    run_until(&mut sides, &mut frame, "both play", |sides, _| {
        sides.iter().all(Side::playing)
    });
    run_until(
        &mut sides,
        &mut frame,
        "the joiner reaches tick 300",
        |sides, _| sides[1].runner.tick() >= 300,
    );
    let joiner = &mut sides[1];
    joiner.online.leave(&mut joiner.runner);
    run_until(
        &mut sides,
        &mut frame,
        "both end and stop lingering",
        |sides, now| sides.iter().all(|side| side.done(now)),
    );

    for side in &mut sides {
        let end = side.end().unwrap().clone();
        assert_eq!(end.kind, MatchEndKind::Left, "{end:?}");
        assert_eq!(end.peer, Some(1));
        assert_eq!(side.runner.online(), Some(&OnlineState::Ended { end }));
        assert!(side.runner.silent_peers().is_empty());
        let assigned = side
            .phases
            .iter()
            .filter(|phase| matches!(phase, OnlinePhase::Assigned))
            .count();
        assert_eq!(assigned, 1, "Assigned is reported once");
        assert!(side.online.messages().is_empty());
    }
    assert_eq!(sides[0].end(), sides[1].end());
    let replays: Vec<Vec<u8>> = sides
        .iter_mut()
        .map(|side| {
            side.runner
                .export_replay()
                .unwrap_or_else(|e| panic!("{e}"))
        })
        .collect();
    assert!(replays[0] == replays[1], "the replays differ");
    let replay = verify_arena(&replays[0]);
    let last = sides[0].end().unwrap().tick.unwrap();
    assert_eq!(replay.hashes.len() as u64, last + 2);
}

/// A link that hides its own id until `assign` is set.
struct Unassigned {
    link: Box<dyn Link>,
    assign: Rc<Cell<bool>>,
    held: Vec<LinkEvent>,
}

impl Link for Unassigned {
    fn poll(&mut self) -> Vec<LinkEvent> {
        self.held.extend(self.link.poll());
        if self.assign.get() {
            return std::mem::take(&mut self.held);
        }
        let (assigned, others) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|event| matches!(event, LinkEvent::Assigned(_)));
        self.held = assigned;
        others
    }

    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
        self.link.send(channel, to, bytes)
    }

    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
        self.link.receive(channel)
    }
}

#[test]
fn a_host_shows_its_code_only_once_assigned() {
    let mesh = MemoryMesh::new();
    let assign = Rc::new(Cell::new(false));
    let link = Unassigned {
        link: Box::new(mesh.join()),
        assign: Rc::clone(&assign),
        held: Vec::new(),
    };
    let mut host = Side::new(
        arena::app,
        &arena_files(),
        Box::new(link),
        OnlineRole::Host {
            peers: 2,
            code: Some(RoomCode::parse("23456A").unwrap()),
        },
    );
    assert_eq!(host.online.code().to_string(), "23456A");
    for frame in 1..=10 {
        host.frame(frame * FRAME_MICROS);
    }
    assert!(
        host.phases
            .iter()
            .all(|phase| matches!(phase, OnlinePhase::Room(_))),
        "{:?}",
        host.phases
    );
    assign.set(true);
    for frame in 11..=20 {
        host.frame(frame * FRAME_MICROS);
    }
    assert!(matches!(host.phases[10], OnlinePhase::Assigned));
    assert!(
        host.phases[11..]
            .iter()
            .all(|phase| matches!(phase, OnlinePhase::Room(_)))
    );
}

#[test]
fn a_plan_derives_the_code_the_match_and_the_seed_from_the_entropy() {
    let entropy = (u128::from(0xaaaa_bbbb_u64) << 64) | 0x1234_5678;
    let host = options(OnlineRole::Host {
        peers: 3,
        code: None,
    })
    .plan(entropy)
    .unwrap();
    assert_eq!(host.code, RoomCode::from_entropy(0x1234_5678));
    assert_eq!(
        host.role,
        RoomRole::Host {
            peers: 3,
            match_id: 0xaaaa_bbbb
        }
    );
    assert_eq!(host.conditions.seed, 0x1234_5678);
    assert_eq!(host.link.host(), Some(3));
    assert_eq!(host.link.code(), &host.code);

    let given = RoomCode::parse("23456A").unwrap();
    let hosted = options(OnlineRole::Host {
        peers: 2,
        code: Some(given.clone()),
    })
    .plan(entropy)
    .unwrap();
    assert_eq!(hosted.code, given);

    let joined = options(OnlineRole::Join {
        code: given.clone(),
    })
    .plan(entropy)
    .unwrap();
    assert_eq!((joined.role, joined.code), (RoomRole::Join, given));
    assert_eq!(joined.link.host(), None);

    let mut bad = options(OnlineRole::Join {
        code: RoomCode::parse("23456A").unwrap(),
    });
    bad.stun = vec!["http://x".to_string()];
    let error = bad.plan(entropy).unwrap_err().to_string();
    assert_eq!(
        error,
        "the online match cannot go on: the STUN URL 'http://x' must start with stun: or stuns:"
    );
}

/// A link that closes, with a reason, once `close` is set: from then on it carries nothing.
struct Closing {
    link: Box<dyn Link>,
    close: Rc<Cell<bool>>,
    closed: bool,
}

impl Link for Closing {
    fn poll(&mut self) -> Vec<LinkEvent> {
        if self.closed {
            return Vec::new();
        }
        if self.close.get() {
            self.closed = true;
            return vec![LinkEvent::Closed("the cable was cut".to_string())];
        }
        self.link.poll()
    }

    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        self.link.send(channel, to, bytes)
    }

    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
        if self.closed {
            return None;
        }
        self.link.receive(channel)
    }
}

#[test]
fn a_closed_link_is_named_in_the_lost_ending() {
    let mesh = MemoryMesh::new();
    let files = arena_files();
    let close = Rc::new(Cell::new(false));
    let host_link = Closing {
        link: Box::new(mesh.join()),
        close: Rc::clone(&close),
        closed: false,
    };
    let host = Side::new(
        arena::app,
        &files,
        Box::new(host_link),
        OnlineRole::Host {
            peers: 2,
            code: None,
        },
    );
    let code = host.online.code().clone();
    let joiner = Side::new(
        arena::app,
        &files,
        Box::new(mesh.join()),
        OnlineRole::Join { code },
    );
    let mut sides = vec![host, joiner];
    let mut frame = 0;
    run_until(&mut sides, &mut frame, "both play", |sides, _| {
        sides.iter().all(Side::playing)
    });
    run_until(&mut sides, &mut frame, "some ticks", |sides, _| {
        sides[0].runner.tick() >= 60
    });
    close.set(true);
    run_until(&mut sides, &mut frame, "both end", |sides, now| {
        sides.iter().all(|side| side.done(now))
    });
    let end = sides[0].end().unwrap();
    assert_eq!(end.kind, MatchEndKind::Lost, "{end:?}");
    assert!(
        end.message.ends_with("; the cable was cut"),
        "{}",
        end.message
    );
    assert_eq!(
        sides[0].online.messages(),
        vec!["the link closed: the cable was cut".to_string()]
    );
    assert!(sides[0].online.messages().is_empty(), "said once");
    assert_eq!(sides[1].end().unwrap().kind, MatchEndKind::Lost);
    for side in &mut sides {
        verify_arena(&side.runner.export_replay().unwrap());
    }
}

/// A link whose match packets are lost while `mute` is set.
struct Muting {
    link: Box<dyn Link>,
    mute: Rc<Cell<bool>>,
}

impl Link for Muting {
    fn poll(&mut self) -> Vec<LinkEvent> {
        self.link.poll()
    }

    fn send(&mut self, channel: Channel, to: LinkId, bytes: Vec<u8>) -> Result<(), LinkError> {
        if self.mute.get() && channel == Channel::Match {
            return Ok(());
        }
        self.link.send(channel, to, bytes)
    }

    fn receive(&mut self, channel: Channel) -> Option<(LinkId, Vec<u8>)> {
        self.link.receive(channel)
    }
}

#[test]
fn a_silent_peer_is_announced_then_back() {
    let mesh = MemoryMesh::new();
    let files = arena_files();
    let mute = Rc::new(Cell::new(false));
    let host = Side::new(
        arena::app,
        &files,
        Box::new(mesh.join()),
        OnlineRole::Host {
            peers: 2,
            code: None,
        },
    );
    let code = host.online.code().clone();
    let link = Muting {
        link: Box::new(mesh.join()),
        mute: Rc::clone(&mute),
    };
    let joiner = Side::new(
        arena::app,
        &files,
        Box::new(link),
        OnlineRole::Join { code },
    );
    let mut sides = vec![host, joiner];
    let mut frame = 0;
    run_until(&mut sides, &mut frame, "both play", |sides, _| {
        sides.iter().all(Side::playing)
    });
    mute.set(true);
    run_until(
        &mut sides,
        &mut frame,
        "the joiner is silent",
        |sides, _| sides[0].runner.silent_peers().contains_key(&1),
    );
    assert!(sides[0].runner.silent_peers()[&1] >= 750);
    mute.set(false);
    run_until(&mut sides, &mut frame, "the joiner is back", |sides, _| {
        sides[0].runner.silent_peers().is_empty()
    });
    assert!(sides.iter().all(|side| side.end().is_none()));
}

#[derive(Serialize, Deserialize, Schema, Default, Clone)]
struct Big {
    bytes: Vec<u8>,
}

/// `KeyB` gives an input of 31 bytes, more than a network match carries.
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
fn an_input_too_long_for_the_network_warns_and_the_match_goes_on() {
    let mesh = MemoryMesh::new();
    let files = GameFiles {
        scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
        assets: Vec::new(),
        edits: None,
    };
    let mut sides = pair(&mesh, &files, big_app, big_app);
    let mut frame = 0;
    run_until(&mut sides, &mut frame, "both play", |sides, _| {
        sides.iter().all(Side::playing)
    });
    sides[0].held = Some(LiveDevices {
        keys: ["KeyB".to_string()].into(),
        gamepads: Vec::new(),
    });
    let tick = sides[0].runner.tick();
    // Past the 3 s after which a silent peer is lost: the host still sends.
    run_until(
        &mut sides,
        &mut frame,
        "five more seconds of ticks",
        |sides, _| sides[0].runner.tick() >= tick + 300,
    );
    assert!(
        sides.iter().all(Side::playing),
        "{:?}",
        sides[1].phases.last()
    );
    let warnings: Vec<String> = sides[0]
        .runner
        .take_messages()
        .into_iter()
        .filter(|(level, _)| *level == ConsoleLevel::Warning)
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        warnings,
        vec![
            "the devices held are not played, the previous input stays: player 0's input \
             takes 31 bytes, a network match carries at most 24"
                .to_string()
        ]
    );
}

/// `KeyP` makes the mapping panic.
fn panicking_mapping_app() -> App {
    let mut app = App::new();
    app.input::<u8>().input_mapping(|raw, _| {
        assert!(!raw.keys.contains("KeyP"), "the mapping broke");
        1u8
    });
    app
}

#[test]
fn a_mapping_that_panics_online_fails_the_game_and_ends_the_match() {
    let mesh = MemoryMesh::new();
    let files = GameFiles {
        scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
        assets: Vec::new(),
        edits: None,
    };
    let mut sides = pair(&mesh, &files, panicking_mapping_app, panicking_mapping_app);
    let mut frame = 0;
    run_until(&mut sides, &mut frame, "both run some ticks", |sides, _| {
        sides.iter().all(|side| {
            matches!(side.runner.online(), Some(OnlineState::Running)) && side.runner.tick() >= 30
        })
    });
    let pressed = LiveDevices {
        keys: ["KeyP".to_string()].into(),
        gamepads: Vec::new(),
    };
    let host = &mut sides[0];
    let error = host.runner.frame(FRAME, &pressed).unwrap_err();
    assert!(
        matches!(&error, RuntimeError::Game { message, .. } if message.contains("the mapping broke")),
        "{error:?}"
    );
    // The frames after it end the match as this peer's failure, and keep telling the other.
    for side in &mut sides {
        side.held = Some(LiveDevices::default());
    }
    run_until(&mut sides, &mut frame, "both end", |sides, now| {
        sides.iter().all(|side| side.done(now))
    });
    for side in &sides {
        let end = side.end().unwrap();
        assert_eq!(end.kind, MatchEndKind::LocalFailure, "{end:?}");
        assert_eq!(end.peer, Some(0));
    }
}

/// A component both desync apps register; only their systems differ.
#[derive(Clone, Component, Serialize, Deserialize)]
#[component(name = "online::Mover")]
struct Mover {
    x: i64,
}

fn walk(mut movers: Query<&mut Mover>) {
    for mover in movers.iter_mut() {
        mover.x += 1;
    }
}

/// From tick 50, moves twice as far.
fn stride(time: Res<Time>, mut movers: Query<&mut Mover>) {
    let step = if time.tick >= 50 { 2 } else { 1 };
    for mover in movers.iter_mut() {
        mover.x += step;
    }
}

fn walking_app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(|_, _| 0u8)
        .component::<Mover>()
        .system(Sim, walk);
    app
}

fn striding_app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(|_, _| 0u8)
        .component::<Mover>()
        .system(Sim, stride);
    app
}

#[test]
fn a_desync_writes_each_peers_snapshot() {
    let mesh = MemoryMesh::new();
    let files = GameFiles {
        scene: r#"Scene(version: 1, resources: {}, entities: [{ "online::Mover": (x: 0) }])"#
            .to_string(),
        assets: Vec::new(),
        edits: None,
    };
    let mut sides = pair(&mesh, &files, walking_app, striding_app);
    let mut frame = 0;
    run_until(&mut sides, &mut frame, "both end", |sides, now| {
        sides.iter().all(|side| side.done(now))
    });
    let dump = tempfile::tempdir().unwrap();
    let mut snapshots = Vec::new();
    for (local, side) in (0u8..).zip(&mut sides) {
        let end = side.end().unwrap().clone();
        assert_eq!(end.kind, MatchEndKind::Desync, "{end:?}");
        assert_eq!(end.tick, Some(60));
        let path = write_desync_snapshot(&mut side.runner, &end, local, dump.path())
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            path,
            dump.path()
                .join(format!("desync-tick-60-peer-{local}.snap"))
        );
        snapshots.push(std::fs::read(path).unwrap());
    }
    let mut host = Host::new(walking_app).unwrap();
    let Ok(Reply::Differences { differences }) = host.handle(Command::DiffSnapshots {
        a: Bytes(snapshots[0].clone()),
        b: Some(Bytes(snapshots[1].clone())),
    }) else {
        panic!("differences");
    };
    assert_eq!(differences.len(), 1, "{differences:?}");
    let difference = &differences[0];
    assert!(difference.subject.starts_with("entity "), "{difference:?}");
    assert_eq!(difference.component, "online::Mover");
    assert_eq!(difference.path, vec!["x".to_string()]);
    // 60 steps of 1, against 50 steps of 1 then 10 of 2 on the striding side.
    assert_eq!(
        (
            difference.left_text.as_str(),
            difference.right_text.as_str()
        ),
        ("60", "70")
    );
}
