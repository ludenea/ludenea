//! The `synctest` command: a game passes, or the first mismatch is reported with both
//! snapshots dumped; the arguments a replay or a scene cannot take are refused.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use ludenea_app::{App, InputLog, Inputs, RawInput, Sim};
use ludenea_cli::run;
use ludenea_ecs::{Component, Query, Runtime, Snapshot};
use serde::{Deserialize, Serialize};

#[derive(Clone, Component, Serialize, Deserialize)]
#[component(name = "synctest::Counter")]
struct Counter {
    value: u32,
}

fn count(inputs: Runtime<Inputs<u8>>, mut counters: Query<&mut Counter>) {
    let step = u32::from(*inputs.player(0));
    for counter in counters.iter_mut() {
        counter.value += step + 1;
    }
}

/// Every key held adds one, button 0 held adds two.
fn map(raw: &RawInput, _player: usize) -> u8 {
    let keys = u8::try_from(raw.keys.len()).unwrap_or(u8::MAX);
    let button = raw
        .gamepads
        .first()
        .and_then(|pad| pad.buttons.first())
        .is_some_and(|&value| value > 0.5);
    keys.saturating_add(2 * u8::from(button))
}

fn app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(map)
        .component::<Counter>()
        .system(Sim, count);
    app
}

/// The ticks this process ran, kept outside any world.
static RAN: AtomicU32 = AtomicU32::new(0);

fn ticks_ran(mut counters: Query<&mut Counter>) {
    let ran = RAN.fetch_add(1, Ordering::Relaxed) + 1;
    for counter in counters.iter_mut() {
        counter.value = ran;
    }
}

fn outside_app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .component::<Counter>()
        .system(Sim, ticks_ran);
    app
}

/// The input of the last tick this process ran, kept outside any world.
static LAST: AtomicU32 = AtomicU32::new(0);

/// Adds the input of the tick before, read from outside the world: a run that changes its
/// inputs diverges once rolled back, a run of constant inputs does not.
fn add_last_input(inputs: Runtime<Inputs<u8>>, mut counters: Query<&mut Counter>) {
    let last = LAST.swap(u32::from(*inputs.player(0)), Ordering::Relaxed);
    for counter in counters.iter_mut() {
        counter.value += last;
    }
}

fn last_input_app() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .input_mapping(map)
        .component::<Counter>()
        .system(Sim, add_last_input);
    app
}

/// A folder holding a scene and an input log of 20 ticks.
fn dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("scene.ron"),
        r#"Scene(version: 1, resources: {}, entities: [{ "synctest::Counter": (value: 3) }])"#,
    )
    .unwrap();
    let mut log = InputLog::new(1);
    for tick in 0..20u8 {
        log.record(&[tick / 4]);
    }
    fs::write(dir.path().join("inputs.bin"), log.encode()).unwrap();
    dir
}

/// Runs the command line of `app` with `args`, `{}` standing for `dir`.
fn cli(app: fn() -> App, dir: &Path, args: &[&str]) -> (i32, String) {
    let mut full = vec!["test".to_string()];
    full.extend(
        args.iter()
            .map(|arg| arg.replace("{}", dir.to_str().unwrap())),
    );
    let mut out = Vec::new();
    let code = run(app, &full, &mut out);
    (code, String::from_utf8(out).unwrap())
}

/// Records the scene and its inputs as `run.replay`.
fn record(dir: &Path) {
    let (code, out) = cli(
        app,
        dir,
        &[
            "run",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--ticks",
            "20",
            "--replay",
            "{}/run.replay",
        ],
    );
    assert_eq!(code, 0, "{out}");
}

#[test]
fn synctest_command_passes_on_a_replay_and_a_scene() {
    let dir = dir();
    record(dir.path());
    let (code, out) = cli(
        app,
        dir.path(),
        &["synctest", "{}/run.replay", "--threads", "2"],
    );
    assert_eq!(
        (code, out.as_str()),
        (0, "synctest passed: 20 ticks, 20 rollbacks\n")
    );
    let (code, out) = cli(
        app,
        dir.path(),
        &[
            "synctest",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--ticks",
            "12",
            "--window",
            "3",
        ],
    );
    assert_eq!(
        (code, out.as_str()),
        (0, "synctest passed: 12 ticks, 12 rollbacks\n")
    );
}

#[test]
fn synctest_command_plays_seeded_random_inputs() {
    let dir = dir();
    let random = [
        "synctest",
        "{}/scene.ron",
        "--ticks",
        "40",
        "--random-inputs",
        "7",
    ];
    let (code, out) = cli(app, dir.path(), &random);
    assert_eq!(
        (code, out.as_str()),
        (0, "synctest passed: 40 ticks, 40 rollbacks\n")
    );
    // The random inputs reach the game: they change, so state outside the world that follows
    // them diverges, where the default inputs of every tick do not.
    let defaults = [&random[..4], &["--dump-dir", "{}/dump"]].concat();
    let (code, out) = cli(last_input_app, dir.path(), &defaults);
    assert_eq!(code, 0, "{out}");
    let (code, out) = cli(
        last_input_app,
        dir.path(),
        &[&random[..], &["--dump-dir", "{}/dump"]].concat(),
    );
    assert_eq!(code, 1, "{out}");
}

#[test]
fn synctest_command_reports_a_mismatch_and_dumps_both_snapshots() {
    let dir = dir();
    let (code, out) = cli(
        outside_app,
        dir.path(),
        &[
            "synctest",
            "{}/scene.ron",
            "--ticks",
            "10",
            "--dump-dir",
            "{}/dump",
        ],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("tick 1"), "{out}");
    assert!(out.contains("synctest::Counter.value"), "{out}");
    let dump = dir.path().join("dump");
    let read = |name: &str| Snapshot::decode(&fs::read(dump.join(name)).unwrap()).unwrap();
    let (straight, rollback) = (read("tick-1-straight.snap"), read("tick-1-rollback.snap"));
    assert_ne!(straight.encode(), rollback.encode());
    assert!(
        out.contains("tick-1-straight.snap") && out.contains("tick-1-rollback.snap"),
        "{out}"
    );
}

#[test]
fn synctest_command_refuses_inputs_for_a_replay() {
    let dir = dir();
    record(dir.path());
    for extra in [
        ["--inputs", "{}/inputs.bin"],
        ["--edits", "{}/inputs.bin"],
        ["--project", "{}"],
        ["--random-inputs", "1"],
    ] {
        let mut args = vec!["synctest", "{}/run.replay"];
        args.extend(extra);
        let (code, out) = cli(app, dir.path(), &args);
        assert_eq!(code, 2, "{extra:?}: {out}");
        assert!(out.contains("a replay holds its own"), "{extra:?}: {out}");
    }
}

#[test]
fn synctest_command_refuses_a_scene_without_ticks_or_with_two_inputs() {
    let dir = dir();
    let (code, out) = cli(app, dir.path(), &["synctest", "{}/scene.ron"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--ticks"), "{out}");
    let (code, out) = cli(
        app,
        dir.path(),
        &[
            "synctest",
            "{}/scene.ron",
            "--ticks",
            "5",
            "--inputs",
            "{}/inputs.bin",
            "--random-inputs",
            "1",
        ],
    );
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("--random-inputs"), "{out}");
}

#[test]
fn synctest_command_refuses_a_window_past_the_ticks() {
    let dir = dir();
    record(dir.path());
    let (code, out) = cli(
        app,
        dir.path(),
        &["synctest", "{}/run.replay", "--window", "21"],
    );
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("21"), "{out}");
}
