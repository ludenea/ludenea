//! Writes the fixtures of the editor's divergence viewer test into a directory:
//!
//! - `a.trace` and `b.trace`: 120 ticks of a small boxes scene; in `b`, one crate is moved
//!   0.5 m right at tick 60, so the traces first diverge at tick 60;
//! - `a.snap` and `b.snap`: both worlds at tick 60, which differ only in that crate's
//!   `ludenea::Pose` `position.x`;
//! - `broken-input.replay`: a replay of the same scene whose input of tick 5 no longer
//!   decodes, as if the input type had changed since it was recorded.
//!
//! The output is deterministic. The tests never regenerate it; after a change to the
//! formats, regenerate and commit it with
//! `cargo run -p boxes --bin divergence-fixtures editor/e2e/fixtures/divergence`.

use std::fs;
use std::path::Path;

use boxes::{BoxesInput, Crate};
use ludenea::app::replay::record_replay;
use ludenea::app::trace::{TRACE_VERSION, Trace, inputs_hash, scene_hash};
use ludenea::app::{EditLog, InputLog, NoAssets};
use ludenea::prelude::*;

const TICKS: u64 = 120;
const MOVED_AT: u64 = 60;
const BROKEN_TICK: usize = 5;

fn main() {
    let output = std::env::args()
        .nth(1)
        .expect("usage: divergence-fixtures <output directory>");
    let output = Path::new(&output);
    fs::create_dir_all(output).expect("cannot create the output directory");
    let scene = boxes::generate_scene(30, 7);
    for (name, moved) in [("a", false), ("b", true)] {
        let (trace, snapshot) = run(&scene, moved);
        fs::write(output.join(format!("{name}.trace")), trace.encode()).expect("cannot write");
        fs::write(output.join(format!("{name}.snap")), snapshot).expect("cannot write");
    }
    fs::write(output.join("broken-input.replay"), broken_replay(&scene)).expect("cannot write");
}

/// The trace of `TICKS` ticks of `scene`, and the snapshot at `MOVED_AT`, where `moved`
/// moves the first crate.
fn run(scene: &str, moved: bool) -> (Trace, Vec<u8>) {
    let mut sim = boxes::app().build().expect("the boxes app builds");
    sim.load_scene(scene).expect("the scene loads");
    let mut trace = Trace {
        version: TRACE_VERSION,
        scene_hash: scene_hash(scene.as_bytes(), &sim),
        inputs_hash: inputs_hash(None, None),
        hashes: vec![sim.hash()],
    };
    let mut snapshot = None;
    for tick in 1..=TICKS {
        sim.step().expect("the boxes game steps");
        if tick == MOVED_AT {
            if moved {
                let world = sim.world_mut();
                let first = world
                    .entities()
                    .find(|&entity| world.get::<Crate>(entity).is_some())
                    .expect("the scene has crates");
                world
                    .get_mut::<Pose>(first)
                    .expect("a crate has a pose")
                    .position
                    .x += 0.5;
            }
            snapshot = Some(sim.snapshot().encode());
        }
        trace.hashes.push(sim.hash());
    }
    (trace, snapshot.expect("the run reaches the snapshot tick"))
}

/// A replay of `scene` whose input of `BROKEN_TICK` is an `Option` tag no input decodes.
fn broken_replay(scene: &str) -> Vec<u8> {
    let mut sim = boxes::app().build().expect("the boxes app builds");
    let mut inputs = InputLog::new(1);
    for _ in 0..TICKS {
        inputs.record(&[BoxesInput::default()]);
    }
    let mut replay = record_replay(&mut sim, scene, &NoAssets, &inputs, &EditLog::new(), TICKS)
        .expect("the scene records");
    // One player, then a tag postcard reads as neither `None` nor `Some`.
    inputs.ticks[BROKEN_TICK] = vec![1, 7];
    replay.inputs = inputs.encode();
    replay.encode()
}
