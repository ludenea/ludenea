//! Reports tick, snapshot, and restore timings of the reference scenario against the targets.

use std::time::Instant;

use boxes::{app, generate_scene};
use ludenea::prelude::Simulation;

const WARMUP_TICKS: u32 = 120;
const MEASURED_TICKS: u32 = 600;
const SNAPSHOTS: u32 = 100;

#[allow(clippy::disallowed_methods)]
fn millis_per_iteration(iterations: u32, mut work: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..iterations {
        work();
    }
    start.elapsed().as_secs_f64() * 1000.0 / f64::from(iterations)
}

fn ms_per_tick(scene: &str, threads: usize) -> f64 {
    let mut simulation = app().build().expect("the app must build");
    simulation.load_scene(scene).expect("the scene must load");
    simulation.set_threads(threads);
    for _ in 0..WARMUP_TICKS {
        simulation.step();
    }
    millis_per_iteration(MEASURED_TICKS, || simulation.step())
}

fn snapshot_and_restore(scene: &str) -> (f64, f64) {
    let mut simulation: Simulation = app().build().expect("the app must build");
    simulation.load_scene(scene).expect("the scene must load");
    let snapshot = simulation.snapshot();
    let snapshot_ms = millis_per_iteration(SNAPSHOTS, || {
        simulation.snapshot();
    });
    let restore_ms = millis_per_iteration(SNAPSHOTS, || {
        simulation
            .restore(&snapshot)
            .expect("the snapshot must restore");
    });
    (snapshot_ms, restore_ms)
}

fn main() {
    let scene = generate_scene(1000, 11);
    let single = ms_per_tick(&scene, 1);
    let parallel = ms_per_tick(&scene, 8);
    let (snapshot, restore) = snapshot_and_restore(&scene);
    println!(
        "ticks: {single:.3} ms/tick (1 thread), {parallel:.3} ms/tick (8 threads); snapshot {snapshot:.3} ms; restore {restore:.3} ms"
    );
    println!(
        "targets: 6 ms/tick (1 thread), 2 ms/tick (8 threads); snapshot 0.5 ms; restore 0.5 ms"
    );
}
