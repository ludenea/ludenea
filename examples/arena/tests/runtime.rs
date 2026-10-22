//! The arena played through the runtime the native window and the standalone page use: the
//! lobby's claims start a two-player match, and live keys recorded by the runner replay
//! headless to the same hashes.

use std::fs;
use std::path::Path;

use arena::{Fighter, SpawnPoint};
use ludenea::app::players::{Lobby, LobbyEvent, LobbyPress, automatic_claims};
use ludenea::app::project::project_assets;
use ludenea::app::replay::{Replay, ReplayOutcome, load_replay, play_replay, play_tick};
use ludenea::prelude::*;
use ludenea::runtime::{GameFiles, Runner};

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/arena.scene.ron");
/// A displayed frame at the arena's 60 ticks per second: one tick each.
const FRAME_SECONDS: f64 = 1.0 / 60.0;

fn files() -> GameFiles {
    let assets = project_assets(Path::new(SCENE), None).expect("the project's assets read");
    GameFiles {
        scene: fs::read_to_string(SCENE).expect("the scene is committed"),
        assets: assets.texts.into_iter().collect(),
        edits: None,
    }
}

fn keys(codes: &[&str]) -> LiveDevices {
    LiveDevices {
        keys: codes.iter().map(|code| code.to_string()).collect(),
        gamepads: Vec::new(),
    }
}

/// The world after the first `ticks` ticks of `replay`, played headless.
fn replayed(replay: &Replay, ticks: u64) -> Simulation {
    let mut simulation = arena::app().build().unwrap();
    let (inputs, edits) = load_replay(&mut simulation, replay).unwrap_or_else(|e| panic!("{e}"));
    for tick in 0..ticks {
        play_tick(&mut simulation, &inputs, &edits, tick).unwrap_or_else(|e| panic!("{e}"));
    }
    simulation
}

/// Each fighter and its position, by player.
fn fighters(world: &World) -> Vec<(Fighter, Vec2)> {
    let mut fighters: Vec<(Fighter, Vec2)> = world
        .entities()
        .filter_map(|entity| {
            Some((
                world.get::<Fighter>(entity)?.clone(),
                world.get::<Pose>(entity)?.position,
            ))
        })
        .collect();
    fighters.sort_by_key(|(fighter, _)| fighter.player);
    fighters
}

fn spawn_point(world: &World, player: u8) -> Vec2 {
    world
        .entities()
        .find(|&entity| {
            world
                .get::<SpawnPoint>(entity)
                .is_some_and(|point| point.player == player)
        })
        .and_then(|entity| Some(world.get::<Pose>(entity)?.position))
        .unwrap_or_else(|| panic!("no spawn point for player {player}"))
}

#[test]
fn lobby_claims_two_keyboard_halves_and_spawns_two_fighters() {
    let mut lobby = Lobby::new();
    assert_eq!(
        lobby.press(LobbyPress::Key("KeyA")),
        LobbyEvent::Claimed { player: 0 }
    );
    assert_eq!(
        lobby.press(LobbyPress::Key("ArrowLeft")),
        LobbyEvent::Claimed { player: 1 }
    );
    let LobbyEvent::Started { claims } = lobby.press(LobbyPress::Key("Enter")) else {
        panic!("Enter does not start the match");
    };
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0].keyboard, Some(KeyboardPart::Left));
    assert_eq!(claims[1].keyboard, Some(KeyboardPart::Right));

    let mut runner =
        Runner::load(arena::app, &files(), None, Vec::new()).unwrap_or_else(|e| panic!("{e}"));
    runner.start(claims).unwrap_or_else(|e| panic!("{e}"));
    // Long enough to fall from the spawn points onto the ground.
    while runner.tick() < 60 {
        runner
            .frame(FRAME_SECONDS, &LiveDevices::default())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let replay = Replay::decode(&runner.export_replay().unwrap()).unwrap();
    let simulation = replayed(&replay, runner.tick());
    let world = simulation.world();
    let fighters = fighters(world);
    assert_eq!(
        fighters
            .iter()
            .map(|(fighter, _)| fighter.player)
            .collect::<Vec<_>>(),
        [0, 1],
        "one fighter per claimed half"
    );
    for (fighter, position) in fighters {
        let player = fighter.player;
        assert_eq!(
            position.x,
            spawn_point(world, player).x,
            "player {player} spawned at its spawn point"
        );
        assert!(fighter.grounded, "player {player} landed on the level");
    }
}

/// What both players hold at frame `frame`: player 0 (the left half) runs right and jumps
/// twice early on, player 1 (the right half) runs left, then they turn around and jump, then
/// let go.
fn scripted(frame: u64) -> LiveDevices {
    match (frame / 100) % 3 {
        0 if frame % 40 < 2 && frame % 100 < 60 => keys(&["KeyD", "KeyW", "ArrowLeft"]),
        0 => keys(&["KeyD", "ArrowLeft"]),
        1 => keys(&["KeyA", "ArrowRight", "ArrowUp"]),
        _ => LiveDevices::default(),
    }
}

#[test]
fn scripted_keys_move_both_fighters_and_the_replay_verifies() {
    let mut runner =
        Runner::load(arena::app, &files(), None, Vec::new()).unwrap_or_else(|e| panic!("{e}"));
    runner
        .start(automatic_claims(2))
        .unwrap_or_else(|e| panic!("{e}"));
    runner.stop_at(600);
    let mut frame = 0;
    while runner.tick() < 600 {
        runner
            .frame(FRAME_SECONDS, &scripted(frame))
            .unwrap_or_else(|e| panic!("{e}"));
        frame += 1;
    }
    let replay = Replay::decode(&runner.export_replay().unwrap()).unwrap();
    assert_eq!(replay.hashes.len(), 601);

    let mut simulation = arena::app().build().unwrap();
    let report = play_replay(&mut simulation, &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);

    // Where each fighter was at frame 100, the end of the first run, played headless.
    let simulation = replayed(&replay, 100);
    let world = simulation.world();
    let moved = fighters(world);
    assert_eq!(moved.len(), 2);
    assert!(
        moved[0].1.x > spawn_point(world, 0).x + 5.0,
        "player 0 ran right on KeyD: {moved:?}"
    );
    assert!(
        moved[1].1.x < spawn_point(world, 1).x - 5.0,
        "player 1 ran left on ArrowLeft: {moved:?}"
    );
    assert!(
        moved.iter().all(|(fighter, _)| fighter.grounded),
        "both ran on the level's ground: {moved:?}"
    );
}
