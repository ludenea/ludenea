//! The golden trace was recorded in release; these tests run in the debug profile, so
//! passing them also proves that debug and release builds agree.

use std::fs;

use boxes::{ContactStats, Crate};
use ludenea::cli::Trace;
use ludenea::control::{Bytes, Command, Host, InProcess, PROTOCOL_VERSION, Reply, Transport};
use ludenea::prelude::*;

const SCENE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.scene.ron");
const INPUTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/boxes.inputs");
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/boxes.trace");

fn verify(threads: &str) {
    let dump_dir = std::env::temp_dir().join(format!("boxes-golden-{threads}-threads"));
    let dump_dir = dump_dir.to_str().expect("UTF-8 temporary directory");
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
        dump_dir,
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
    let host = Host::new(boxes::app()).unwrap();
    let mut client = InProcess::connect(host, PROTOCOL_VERSION).unwrap();
    let ron = fs::read_to_string(SCENE).unwrap();
    client.request(Command::LoadScene { ron }).unwrap();
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
