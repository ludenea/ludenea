use std::fs;
use std::path::{Path, PathBuf};

use ludenea_app::{App, InputLog, Rng, Sim};
use ludenea_cli::{Trace, run};
use ludenea_ecs::{Component, Query, ResMut};
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize)]
struct Counter(u32);

fn count(mut counters: Query<&mut Counter>, mut rng: ResMut<Rng>) {
    for counter in counters.iter_mut() {
        counter.0 += rng.range_u32(1, 4);
    }
}

fn app() -> App {
    let mut app = App::new();
    app.input::<u8>().component::<Counter>().system(Sim, count);
    app
}

fn scene(value: u32) -> String {
    format!(r#"Scene(version: 1, resources: {{}}, entities: [{{ "cli::Counter": ({value}) }}])"#)
}

/// A fresh directory holding a scene, an input log, and room for outputs.
fn dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludenea-cli-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("scene.ron"), scene(5)).unwrap();
    let mut log = InputLog::new(1);
    for tick in 0..20u8 {
        log.record(&[tick]);
    }
    fs::write(dir.join("inputs.bin"), log.encode()).unwrap();
    dir
}

fn cli(dir: &Path, args: &[&str]) -> (i32, String) {
    let mut full = vec!["test".to_string()];
    full.extend(
        args.iter()
            .map(|arg| arg.replace("{}", dir.to_str().unwrap())),
    );
    let mut out = Vec::new();
    let code = run(app, &full, &mut out);
    (code, String::from_utf8(out).unwrap())
}

fn record(dir: &Path) {
    let (code, _) = cli(
        dir,
        &[
            "run",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--ticks",
            "20",
            "--trace",
            "{}/golden.bin",
        ],
    );
    assert_eq!(code, 0);
}

fn verify(dir: &Path) -> (i32, String) {
    cli(
        dir,
        &[
            "verify",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--golden",
            "{}/golden.bin",
            "--dump-dir",
            "{}/dump",
        ],
    )
}

#[test]
fn run_prints_final_hash() {
    let dir = dir("run_prints_final_hash");
    let (code, out) = cli(&dir, &["run", "{}/scene.ron", "--ticks", "3", "--hash"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("tick 3 hash "), "{out}");
    assert_eq!(out.trim_end().len(), "tick 3 hash ".len() + 16);
}

#[test]
fn run_then_verify_passes() {
    let dir = dir("run_then_verify_passes");
    record(&dir);
    assert_eq!(verify(&dir), (0, "ok: 21 hashes match\n".to_string()));
    let (code, _) = cli(
        &dir,
        &[
            "verify",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--golden",
            "{}/golden.bin",
            "--threads",
            "2",
        ],
    );
    assert_eq!(code, 0);
}

fn tamper(dir: &Path, index: usize) {
    let path = dir.join("golden.bin");
    let mut trace = Trace::decode(&fs::read(&path).unwrap()).unwrap();
    trace.hashes[index] ^= 1;
    fs::write(path, trace.encode()).unwrap();
}

#[test]
fn verify_against_tampered_golden_reports_tick_and_writes_dumps() {
    let dir = dir("verify_tampered");
    record(&dir);
    tamper(&dir, 7);
    let (code, out) = verify(&dir);
    assert_eq!(code, 1);
    assert!(out.contains("mismatch at tick 7"), "{out}");
    assert!(dir.join("dump/tick-6.snap").exists());
    assert!(dir.join("dump/tick-7.snap").exists());
}

#[test]
fn verify_mismatch_at_tick_zero_writes_one_dump() {
    let dir = dir("verify_tick_zero");
    record(&dir);
    tamper(&dir, 0);
    let (code, out) = verify(&dir);
    assert_eq!(code, 1);
    assert!(out.contains("mismatch at tick 0"), "{out}");
    assert!(dir.join("dump/tick-0.snap").exists());
    assert_eq!(fs::read_dir(dir.join("dump")).unwrap().count(), 1);
}

#[test]
fn verify_with_other_scene_reports_different_scene() {
    let dir = dir("verify_other_scene");
    record(&dir);
    fs::write(dir.join("scene.ron"), scene(6)).unwrap();
    assert_eq!(
        verify(&dir),
        (
            2,
            "golden trace was recorded for a different scene or input log\n".to_string()
        )
    );
}

/// Dumps written by a failed verification: the snapshots after ticks 6 and 7.
fn dumps(dir: &Path) -> (String, String) {
    record(dir);
    tamper(dir, 7);
    verify(dir);
    let dump = |tick| format!("{}/dump/tick-{tick}.snap", dir.display());
    (dump(6), dump(7))
}

#[test]
fn diff_of_dumps_names_entity_component_and_field() {
    let dir = dir("diff_dumps");
    let (a, b) = dumps(&dir);
    let (code, out) = cli(&dir, &["diff", &a, &b]);
    assert_eq!(code, 1);
    assert!(out.contains("Counter"), "{out}");
}

#[test]
fn diff_of_identical_snapshots() {
    let dir = dir("diff_identical");
    let (a, _) = dumps(&dir);
    assert_eq!(
        cli(&dir, &["diff", &a, &a]),
        (0, "snapshots are identical\n".to_string())
    );
}

#[test]
fn missing_file_prints_error_and_exits_2() {
    let dir = dir("missing_file");
    let (code, out) = cli(&dir, &["run", "{}/nope.ron", "--ticks", "1"]);
    assert_eq!(code, 2);
    assert!(out.starts_with("error: cannot read "), "{out}");
}

#[test]
fn trace_parent_directory_is_created() {
    let dir = dir("trace_parent");
    let (code, _) = cli(
        &dir,
        &[
            "run",
            "{}/scene.ron",
            "--ticks",
            "2",
            "--trace",
            "{}/a/b/golden.bin",
        ],
    );
    assert_eq!(code, 0);
    assert!(dir.join("a/b/golden.bin").exists());
}
