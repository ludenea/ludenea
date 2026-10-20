//! The golden trace was recorded in release; these tests run in the debug profile, so
//! passing them also proves that debug and release builds agree.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use boxes::{ContactStats, Crate};
use ludenea::app::replay::{Replay, load_replay};
use ludenea::cli::Trace;
use ludenea::control::{
    Bytes, Command, Document, Host, InProcess, PROTOCOL_VERSION, Reply, Transport,
};
use ludenea::prelude::*;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.scene.ron");
const INPUTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.inputs");
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/boxes.trace");
const SPAWNER_REPLAY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/spawner.replay");

/// Where the golden runs write the snapshots around a divergence, given the value of
/// `LUDENEA_DUMP_DIR`: that folder when it is set and not empty (CI uploads it), else the
/// temporary directory.
fn dump_dir(ludenea_dump_dir: Option<OsString>) -> PathBuf {
    ludenea_dump_dir
        .filter(|dir| !dir.is_empty())
        .map_or_else(std::env::temp_dir, PathBuf::from)
}

/// The variable that names the dump folder, which CI's native jobs set.
const DUMP_DIR_VARIABLE: &str = "LUDENEA_DUMP_DIR";

/// The dump folder of the golden run `name`, as a command line argument.
fn run_dump_dir(name: &str) -> String {
    let dir = dump_dir(std::env::var_os(DUMP_DIR_VARIABLE)).join(name);
    dir.into_os_string()
        .into_string()
        .expect("LUDENEA_DUMP_DIR or the temporary directory is UTF-8")
}

#[test]
fn dump_dir_follows_ludenea_dump_dir() {
    assert_eq!(dump_dir(Some("ci/dump".into())), PathBuf::from("ci/dump"));
    assert_eq!(dump_dir(Some(OsString::new())), std::env::temp_dir());
    assert_eq!(dump_dir(None), std::env::temp_dir());
}

#[test]
fn ci_sets_the_dump_dir_variable_for_the_native_golden_tests() {
    let workflow = include_str!("../../../.github/workflows/determinism.yml");
    let key = format!("{DUMP_DIR_VARIABLE}: ");
    // The `env` of the native job, which runs these tests, not of another job or a step.
    let mut native_env = workflow
        .lines()
        .skip_while(|line| line.trim_end() != "  native:")
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with("   "))
        .skip_while(|line| line.trim_end() != "    env:")
        .skip(1)
        .take_while(|line| line.starts_with("      "));
    assert!(
        native_env.any(|line| line.trim_start().starts_with(&key)),
        "the native job's env does not set {DUMP_DIR_VARIABLE}"
    );
}

fn verify(threads: &str) {
    let dump_dir = run_dump_dir(&format!("boxes-golden-{threads}-threads"));
    let mut out = Vec::new();
    let args = [
        "boxes",
        "verify",
        SCENE,
        "--inputs",
        INPUTS,
        "--golden",
        GOLDEN,
        "--threads",
        threads,
        "--dump-dir",
        &dump_dir,
    ];
    let code = ludenea::cli::run(boxes::app, &args, &mut out);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
}

#[test]
fn golden_matches_with_1_thread() {
    verify("1");
}

#[test]
fn golden_matches_with_2_threads() {
    verify("2");
}

#[test]
fn golden_matches_with_8_threads() {
    verify("8");
}

fn connect() -> InProcess {
    let host = Host::new(boxes::app).unwrap();
    let mut client = InProcess::connect(host, PROTOCOL_VERSION).unwrap();
    let ron = fs::read_to_string(SCENE).unwrap();
    client
        .request(Command::LoadScene {
            scene: ron,
            assets: Vec::new(),
            document: Document::Scene,
        })
        .unwrap();
    let log = Bytes(fs::read(INPUTS).unwrap());
    client.request(Command::LoadInputs { log }).unwrap();
    client
}

fn step(client: &mut InProcess, ticks: u32) -> Vec<u64> {
    let command = Command::Step {
        ticks,
        record_hashes: true,
    };
    match client.request(command).unwrap() {
        Reply::Stepped { hashes, .. } => hashes.into_iter().map(|hash| hash.0).collect(),
        other => panic!("unexpected reply {other:?}"),
    }
}

#[test]
fn snapshot_at_5000_restore_replays_identically() {
    let mut client = connect();
    step(&mut client, 5_000);
    let Reply::Snapshot { snapshot, .. } = client.request(Command::TakeSnapshot).unwrap() else {
        panic!("unexpected reply to TakeSnapshot");
    };
    let first = step(&mut client, 5_000);
    client.request(Command::Restore { snapshot }).unwrap();
    let second = step(&mut client, 5_000);
    assert_eq!(first, second);
    let golden = Trace::decode(&fs::read(GOLDEN).unwrap()).unwrap();
    assert_eq!(first, golden.hashes[5_001..]);
}

#[test]
fn scenario_exercises_spawn_despawn_and_contacts() {
    let mut simulation = boxes::app().build().unwrap();
    simulation
        .load_scene(&fs::read_to_string(SCENE).unwrap())
        .unwrap();
    let log = InputLog::decode(&fs::read(INPUTS).unwrap()).unwrap();
    for tick in 0..10_000 {
        simulation.step_encoded(log.tick(tick)).unwrap();
    }
    let world = simulation.world();
    let stats = world.resource::<ContactStats>().unwrap();
    assert!(stats.begins > 0 && stats.ends > 0);
    let crates = Query::<&Pose, With<Crate>>::new(world).unwrap();
    let positions: Vec<Vec2> = crates.iter().map(|pose| pose.position).collect();
    // 300 scene crates, plus 20 spawns (every 500 ticks from tick 0), minus 15 despawns
    // (every 700 ticks from tick 0), over ticks 0 to 9,999.
    assert_eq!(positions.len(), 305);
    for position in positions {
        let inside = position.x > -20.0 && position.x < 20.0;
        assert!(
            inside && position.y > 0.0 && position.y < 30.0,
            "escaped to {position:?}"
        );
    }
}

#[test]
fn spawner_replay_matches_at_1_2_and_8_threads() {
    for threads in ["1", "2", "8"] {
        let dump_dir = run_dump_dir(&format!("boxes-spawner-{threads}-threads"));
        let args = [
            "boxes",
            "verify",
            SPAWNER_REPLAY,
            "--threads",
            threads,
            "--dump-dir",
            &dump_dir,
        ];
        let mut out = Vec::new();
        let code = ludenea::cli::run(boxes::app, &args, &mut out);
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
    }
}

/// Runs the edits and inputs from the current tick to `end`, returning the hash after each
/// tick.
fn play_to(
    simulation: &mut Simulation,
    (inputs, edits): &(InputLog, EditLog),
    end: u64,
) -> Vec<u64> {
    let mut hashes = Vec::new();
    for tick in simulation.tick()..end {
        for timed in edits.at(tick) {
            simulation.apply_edit(&timed.edit).unwrap();
        }
        simulation.step_encoded(inputs.tick(tick)).unwrap();
        hashes.push(simulation.hash());
    }
    hashes
}

#[test]
fn spawner_replay_survives_snapshot_restore_at_1500() {
    let replay = Replay::decode(&fs::read(SPAWNER_REPLAY).unwrap()).unwrap();
    let end = replay.hashes.len() as u64 - 1;
    let mut simulation = boxes::app().build().unwrap();
    let logs = load_replay(&mut simulation, &replay).unwrap();
    play_to(&mut simulation, &logs, 1_500);
    let snapshot = simulation.snapshot();
    let first = play_to(&mut simulation, &logs, end);
    simulation.restore(&snapshot).unwrap();
    let second = play_to(&mut simulation, &logs, end);
    assert_eq!(first, second);
    assert_eq!(first, replay.hashes[1_501..]);
    // 2 instances, plus a spawn every 60 ticks over ticks 0 to 999 (17), then every 45
    // ticks once the stats change, over ticks 1,000 to 2,999 (44).
    let crates = Query::<&Pose, With<Crate>>::new(simulation.world()).unwrap();
    let positions: Vec<Vec2> = crates.iter().map(|pose| pose.position).collect();
    assert_eq!(positions.len(), 63);
    for position in positions {
        let inside = position.x > -20.0 && position.x < 20.0;
        assert!(
            inside && position.y > 0.0 && position.y < 30.0,
            "escaped to {position:?}"
        );
    }
}
