use std::collections::BTreeSet;

use ludenea::app::assets::asset_file;
use ludenea::app::players::{LiveDevices, automatic_claims};
use ludenea::app::replay::{Replay, ReplayOutcome, play_replay, record_replay};
use ludenea::app::{AssetTexts, EditLog, InputLog, RawGamepad};
use ludenea::control::ConsoleLevel;
use ludenea::control::pacer::MAX_TICKS_PER_FRAME;
use ludenea::ecs::AssetId;
use ludenea::prelude::*;
use ludenea_runtime::{GameFiles, Runner, RuntimeError};
use serde::{Deserialize, Serialize};
use swarm::{BUG_SHEET, SwarmInput};

const SWARM_SCENE: &str = include_str!("../../../examples/swarm/assets/swarm.scene.ron");
const SWARM_SHEET: &str = include_str!("../../../examples/swarm/assets/bug.sheet.ron");
const EMPTY_SCENE: &str = "Scene(version: 2, next_id: 0, resources: {}, entities: [])";
/// The tick whose step panics in `panicking_app`.
const PANIC_TICK: u64 = 30;

fn swarm_files() -> GameFiles {
    let sheet = AssetId(u128::from_str_radix(BUG_SHEET, 16).expect("a hexadecimal identifier"));
    GameFiles {
        scene: SWARM_SCENE.to_string(),
        assets: vec![(sheet, asset_file("ludenea::SpriteSheet", SWARM_SHEET))],
        edits: None,
    }
}

fn keys(codes: &[&str]) -> LiveDevices {
    LiveDevices {
        keys: codes.iter().map(|code| code.to_string()).collect(),
        gamepads: Vec::new(),
    }
}

/// What one player holds at frame `frame`: keys, then a gamepad, then nothing, in turns.
fn scripted(frame: u64) -> LiveDevices {
    match (frame / 50) % 4 {
        0 => keys(&["KeyD", "KeyW"]),
        1 => LiveDevices {
            keys: BTreeSet::new(),
            gamepads: vec![Some(RawGamepad {
                buttons: vec![0.0; 17],
                axes: vec![-1.0, 0.5, 0.0, 0.0],
            })],
        },
        2 => keys(&["ArrowDown"]),
        _ => LiveDevices::default(),
    }
}

fn started(app: fn() -> App, files: &GameFiles, players: usize) -> Runner {
    let mut runner = Runner::load(app, files, None, Vec::new()).unwrap_or_else(|e| panic!("{e}"));
    runner
        .start(automatic_claims(players))
        .unwrap_or_else(|e| panic!("{e}"));
    runner
}

#[test]
fn runner_matches_headless_hashes() {
    let files = swarm_files();
    let mut runner = started(swarm::app, &files, 1);
    let mut frame = 0;
    while runner.tick() < 600 {
        runner
            .frame(1.0 / 144.0, &scripted(frame))
            .unwrap_or_else(|e| panic!("{e}"));
        frame += 1;
    }
    assert_eq!(runner.tick(), 600, "a 1/144 s frame runs at most one tick");
    let replay = Replay::decode(&runner.export_replay().unwrap()).unwrap();
    assert_eq!(replay.hashes.len(), 601);

    let mut simulation = swarm::app().build().unwrap();
    let report = play_replay(&mut simulation, &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);

    let inputs = InputLog::decode(&replay.inputs).unwrap();
    let edits = EditLog::decode(&replay.edits).unwrap();
    let assets: AssetTexts = files.assets.iter().cloned().collect();
    let mut headless = swarm::app().build().unwrap();
    let recorded =
        record_replay(&mut headless, &files.scene, &assets, &inputs, &edits, 600).unwrap();
    assert_eq!(recorded, replay, "the headless run records the same replay");
    let directions: BTreeSet<_> = (0..600)
        .map(|tick| {
            let bytes = inputs.tick(tick).expect("every tick is recorded");
            let input: Vec<SwarmInput> = postcard::from_bytes(bytes).unwrap();
            format!("{:?}", input[0].direction)
        })
        .collect();
    assert!(
        directions.len() >= 4,
        "the script moves the player: {directions:?}"
    );
}

#[test]
fn two_players_record_their_halves() {
    let mut runner = started(swarm::app, &swarm_files(), 2);
    while runner.tick() < 10 {
        runner
            .frame(1.0 / 60.0, &keys(&["KeyD", "ArrowLeft"]))
            .unwrap();
    }
    let replay = Replay::decode(&runner.export_replay().unwrap()).unwrap();
    let inputs = InputLog::decode(&replay.inputs).unwrap();
    assert_eq!(inputs.players, 2);
    let input: Vec<SwarmInput> = postcard::from_bytes(inputs.tick(0).unwrap()).unwrap();
    assert_eq!(
        input[0].direction,
        Vec2::new(1.0, 0.0),
        "KeyD is on the left half"
    );
    assert_eq!(
        input[1].direction,
        Vec2::new(-1.0, 0.0),
        "ArrowLeft is on the right half"
    );
}

#[test]
fn stall_of_a_minute_runs_at_most_the_pacer_cap() {
    let mut runner = started(swarm::app, &swarm_files(), 1);
    let idle = LiveDevices::default();
    for _ in 0..3 {
        runner.frame(1.0 / 60.0, &idle).unwrap();
    }
    let before = runner.tick();
    let update = runner.frame(60.0, &idle).unwrap();
    assert!(update.tick > before, "the stall runs ticks");
    assert!(update.tick - before <= u64::from(MAX_TICKS_PER_FRAME));
    assert!((0.0..=1.0).contains(&update.alpha), "{}", update.alpha);
    assert!(update.new_frames);
    // The debt beyond the cap is dropped: play goes on at its pace.
    let next = runner.frame(1.0 / 60.0, &idle).unwrap();
    assert_eq!(next.tick, update.tick + 1);
}

#[test]
fn alpha_stays_between_zero_and_one() {
    let mut runner = started(swarm::app, &swarm_files(), 1);
    let idle = LiveDevices::default();
    let elapsed = [
        0.0,
        1.0 / 144.0,
        0.5 / 60.0,
        1e-12,
        7.3 / 60.0,
        1.0 / 30.0,
        0.999 / 60.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        3600.0,
    ];
    let mut previous = runner.frames().1.cloned();
    assert!(previous.is_some(), "start fetches the frame of tick 0");
    for seconds in elapsed {
        let update = runner.frame(seconds, &idle).unwrap();
        assert!(
            (0.0..=1.0).contains(&update.alpha),
            "alpha {} after {seconds} s",
            update.alpha
        );
        let (before, current) = runner.frames();
        if update.new_frames {
            assert_eq!(
                before,
                previous.as_ref(),
                "the current frame becomes the previous one"
            );
        } else {
            assert_eq!(current, previous.as_ref(), "no tick keeps the frames");
        }
        previous = current.cloned();
    }
}

#[derive(Serialize, Deserialize, Schema, Default, Clone)]
struct Pressed {
    any: bool,
}

fn boom(time: Res<Time>) {
    if time.tick == PANIC_TICK {
        panic!("boom");
    }
}

/// A game whose system panics on the step from tick `PANIC_TICK`.
fn panicking_app() -> App {
    let mut app = App::new();
    app.input::<Pressed>()
        .input_mapping(|raw, _| Pressed {
            any: !raw.keys.is_empty(),
        })
        .system(Sim, boom);
    app
}

fn run_until_failure(runner: &mut Runner) -> RuntimeError {
    for frame in 0..1_000 {
        let devices = keys(if frame % 2 == 0 { &["Space"] } else { &[] });
        if let Err(error) = runner.frame(1.0 / 60.0, &devices) {
            return error;
        }
    }
    panic!("the game never failed");
}

fn empty_files() -> GameFiles {
    GameFiles {
        scene: EMPTY_SCENE.to_string(),
        assets: Vec::new(),
        edits: None,
    }
}

#[test]
fn game_panic_is_a_game_error_naming_the_tick() {
    let mut runner = started(panicking_app, &empty_files(), 1);
    let error = run_until_failure(&mut runner);
    let text = error.to_string();
    assert!(text.contains(&format!("tick {PANIC_TICK}")), "{text}");
    match error {
        RuntimeError::Game { tick, message } => {
            assert_eq!(tick, PANIC_TICK);
            assert!(message.contains("boom"), "{message}");
        }
        other => panic!("expected a game error, got {other:?}"),
    }
}

#[test]
fn replay_after_a_failure_holds_the_ticks_that_ran() {
    let mut runner = started(panicking_app, &empty_files(), 1);
    run_until_failure(&mut runner);
    let replay = Replay::decode(&runner.export_replay().unwrap()).unwrap();
    assert_eq!(replay.hashes.len() as u64, PANIC_TICK + 1);
    let mut simulation = panicking_app().build().unwrap();
    let report = play_replay(&mut simulation, &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
}

#[test]
fn a_bad_scene_is_a_load_error() {
    let files = GameFiles {
        scene: "Scene(".to_string(),
        assets: Vec::new(),
        edits: None,
    };
    match Runner::load(swarm::app, &files, None, Vec::new()) {
        Err(RuntimeError::Load(message)) => assert!(!message.is_empty()),
        Err(other) => panic!("expected a load error, got {other:?}"),
        Ok(_) => panic!("a bad scene loads"),
    }
}

/// What the loaded edit log spawns.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Marker {
    value: u32,
}

/// A game with a component an edit can spawn, and nothing else.
fn marked_app() -> App {
    let mut app = App::new();
    app.input::<Pressed>()
        .input_mapping(|raw, _| Pressed {
            any: !raw.keys.is_empty(),
        })
        .component::<Marker>();
    app
}

/// The tick the loaded edit is recorded at.
const EDIT_TICK: u64 = 10;

fn files_with_edit(edit: Edit) -> GameFiles {
    let mut log = EditLog::new();
    log.edits.push(TimedEdit {
        tick: EDIT_TICK,
        edit,
    });
    GameFiles {
        edits: Some(log.encode()),
        ..empty_files()
    }
}

fn spawn_marker() -> Edit {
    Edit::Spawn {
        components: vec![(
            "runner::Marker".to_string(),
            postcard::to_allocvec(&Marker { value: 7 }).unwrap(),
        )],
    }
}

fn markers(simulation: &Simulation) -> Vec<Marker> {
    let world = simulation.world();
    world
        .entities()
        .filter_map(|entity| world.get::<Marker>(entity).map(|marker| marker.clone()))
        .collect()
}

/// The players press keys only after the edit's tick: the match follows the recorded future
/// up to the edit, which applies and stays in the replay of the branch played.
#[test]
fn loaded_edits_apply_and_stay_in_the_replay() {
    let mut runner = started(marked_app, &files_with_edit(spawn_marker()), 1);
    let mut frame = 0;
    while runner.tick() < 2 * EDIT_TICK {
        let pressing = runner.tick() > EDIT_TICK && frame % 3 == 0;
        let devices = keys(if pressing { &["Space"] } else { &[] });
        runner.frame(1.0 / 60.0, &devices).unwrap();
        frame += 1;
    }
    let replay = Replay::decode(&runner.export_replay().unwrap()).unwrap();
    let edits = EditLog::decode(&replay.edits).unwrap();
    assert_eq!(edits.edits.len(), 1, "{edits:?}");
    assert_eq!(edits.edits[0].tick, EDIT_TICK);
    let mut simulation = marked_app().build().unwrap();
    let report = play_replay(&mut simulation, &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    assert_eq!(markers(&simulation), [Marker { value: 7 }]);
    let inputs = InputLog::decode(&replay.inputs).unwrap();
    let pressed = (0..2 * EDIT_TICK).filter(|&tick| {
        let input: Vec<Pressed> = postcard::from_bytes(inputs.tick(tick).unwrap()).unwrap();
        input[0].any
    });
    assert!(pressed.count() > 0, "live input after the edit is recorded");
    assert!(runner.take_messages().is_empty());
}

#[test]
fn an_edit_that_no_longer_applies_is_a_warning_naming_its_tick() {
    let unknown = Edit::Spawn {
        components: vec![("runner::Missing".to_string(), Vec::new())],
    };
    let mut runner = started(marked_app, &files_with_edit(unknown), 1);
    let mut messages = runner.take_messages();
    while runner.tick() < 2 * EDIT_TICK {
        runner.frame(1.0 / 60.0, &LiveDevices::default()).unwrap();
        messages.extend(runner.take_messages());
    }
    let warning = format!("the edit recorded at tick {EDIT_TICK} was dropped");
    assert!(
        messages
            .iter()
            .any(|(level, text)| *level == ConsoleLevel::Warning && text.contains(&warning)),
        "{messages:?}"
    );
    assert!(runner.take_messages().is_empty(), "messages are taken once");
}

/// A game whose render frame cannot be extracted.
fn undrawable_app() -> App {
    let mut app = marked_app();
    app.extractor(|_, _| panic!("no frame"));
    app
}

#[test]
fn a_first_frame_that_cannot_draw_is_a_load_error() {
    let mut runner = Runner::load(undrawable_app, &empty_files(), None, Vec::new()).unwrap();
    match runner.start(automatic_claims(1)) {
        Err(RuntimeError::Load(message)) => assert!(message.contains("no frame"), "{message}"),
        other => panic!("expected a load error, got {other:?}"),
    }
}

/// The players take over at their first key press, which comes before the edit's tick: the
/// session forks a branch at that press, as editing the past does, and the branch played
/// holds none of the edits after it, the loaded one included.
#[test]
fn input_before_a_loaded_edit_forks_and_the_replay_leaves_it_out() {
    let mut runner = started(marked_app, &files_with_edit(spawn_marker()), 1);
    let mut frame = 0;
    while runner.tick() < 2 * EDIT_TICK {
        let pressing = runner.tick() < EDIT_TICK / 2 && frame % 3 == 0;
        let devices = keys(if pressing { &["Space"] } else { &[] });
        runner.frame(1.0 / 60.0, &devices).unwrap();
        frame += 1;
    }
    let replay = Replay::decode(&runner.export_replay().unwrap()).unwrap();
    let edits = EditLog::decode(&replay.edits).unwrap();
    assert_eq!(edits.edits, [], "the forked branch holds no later edit");
    let mut simulation = marked_app().build().unwrap();
    let report = play_replay(&mut simulation, &replay, |_, _| {}).unwrap();
    assert_eq!(report.outcome, ReplayOutcome::Matched);
    assert_eq!(markers(&simulation), []);
}
