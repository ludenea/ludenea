use std::fs;
use std::path::{Path, PathBuf};

use ludenea_app::replay::Replay;
use ludenea_app::{App, InputLog, Rng, Sim};
use ludenea_cli::{Trace, run};
use ludenea_control::{
    Bytes, Command, Document, Host, InProcess, PROTOCOL_VERSION, Reply, Transport,
};
use ludenea_ecs::{Asset, Component, Handle, Query, ResMut};
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

#[derive(Asset, Serialize, Deserialize, Clone)]
struct Tuning {
    speed: u32,
}

#[derive(Component, Serialize, Deserialize)]
struct Tuned {
    tuning: Handle<Tuning>,
}

fn tuned_app() -> App {
    let mut app = App::new();
    app.component::<Tuned>().asset::<Tuning>();
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

fn golden_hashes(dir: &Path) -> Vec<u64> {
    Trace::decode(&fs::read(dir.join("golden.bin")).unwrap())
        .unwrap()
        .hashes
}

/// The state hash of a dumped snapshot, after restoring it into a fresh host.
fn snapshot_hash(path: &Path) -> u64 {
    let mut client = InProcess::connect(Host::new(app).unwrap(), PROTOCOL_VERSION).unwrap();
    let snapshot = Bytes(fs::read(path).unwrap());
    client.request(Command::Restore { snapshot }).unwrap();
    match client.request(Command::GetHash) {
        Ok(Reply::Hash { hash, .. }) => hash.0,
        other => panic!("{other:?}"),
    }
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
    let expected = golden_hashes(&dir);
    tamper(&dir, 7);
    let (code, out) = verify(&dir);
    assert_eq!(code, 1);
    assert!(out.contains("mismatch at tick 7"), "{out}");
    assert!(
        out.contains("tick-6.snap and ") && out.contains("tick-7.snap"),
        "{out}"
    );
    assert_eq!(snapshot_hash(&dir.join("dump/tick-6.snap")), expected[6]);
    assert_eq!(snapshot_hash(&dir.join("dump/tick-7.snap")), expected[7]);
}

#[test]
fn verify_mismatch_at_tick_zero_writes_one_dump() {
    let dir = dir("verify_tick_zero");
    record(&dir);
    let expected = golden_hashes(&dir);
    tamper(&dir, 0);
    let (code, out) = verify(&dir);
    assert_eq!(code, 1);
    assert!(out.contains("mismatch at tick 0"), "{out}");
    assert_eq!(snapshot_hash(&dir.join("dump/tick-0.snap")), expected[0]);
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
    assert!(out.contains("entity 0v0 cli::Counter [0]: "), "{out}");
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

/// A session with a recorded edit, exported through the protocol and written as files.
fn export_session_with_edits(dir: &Path) -> Vec<u64> {
    let mut client = InProcess::connect(Host::new(app).unwrap(), PROTOCOL_VERSION).unwrap();
    let ron = fs::read_to_string(dir.join("scene.ron")).unwrap();
    let log = Bytes(fs::read(dir.join("inputs.bin")).unwrap());
    client
        .request(Command::LoadScene {
            scene: ron,
            assets: Vec::new(),
            document: Document::Scene,
        })
        .unwrap();
    client.request(Command::LoadInputs { log }).unwrap();
    client
        .request(Command::Step {
            ticks: 8,
            record_hashes: false,
        })
        .unwrap();
    client
        .request(Command::Spawn {
            components: [("cli::Counter".to_string(), serde_json::json!(3))].into(),
        })
        .unwrap();
    client
        .request(Command::Step {
            ticks: 6,
            record_hashes: false,
        })
        .unwrap();
    let Ok(Reply::Session {
        inputs,
        edits,
        hashes,
        ..
    }) = client.request(Command::ExportSession)
    else {
        panic!("export failed");
    };
    fs::write(dir.join("edits.bin"), edits.0).unwrap();
    fs::write(dir.join("inputs.bin"), inputs.0).unwrap();
    hashes.iter().map(|hash| hash.0).collect()
}

#[test]
fn verify_with_edits_replays_exported_session() {
    let dir = dir("verify_with_edits");
    let exported = export_session_with_edits(&dir);
    let (code, out) = cli(
        &dir,
        &[
            "run",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--edits",
            "{}/edits.bin",
            "--ticks",
            "14",
            "--trace",
            "{}/golden.bin",
        ],
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(golden_hashes(&dir), exported);
    let (code, out) = cli(
        &dir,
        &[
            "verify",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--edits",
            "{}/edits.bin",
            "--golden",
            "{}/golden.bin",
        ],
    );
    assert_eq!((code, out.as_str()), (0, "ok: 15 hashes match\n"));
    // The same trace does not verify against a run without the edits.
    assert_eq!(verify(&dir).0, 2);
}

#[test]
fn golden_trace_still_verifies_without_edits() {
    let dir = dir("golden_without_edits");
    record(&dir);
    assert_eq!(verify(&dir).0, 0);
    let trace = Trace::decode(&fs::read(dir.join("golden.bin")).unwrap()).unwrap();
    assert_eq!(
        trace.inputs_hash,
        xxhash_rust::xxh3::xxh3_64(&fs::read(dir.join("inputs.bin")).unwrap())
    );
}

fn record_replay(dir: &Path) {
    let (code, out) = cli(
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
            "--replay",
            "{}/run.replay",
        ],
    );
    assert_eq!(code, 0, "{out}");
}

fn verify_replay(dir: &Path, threads: &str) -> (i32, String) {
    cli(
        dir,
        &[
            "verify",
            "{}/run.replay",
            "--threads",
            threads,
            "--dump-dir",
            "{}/dump",
        ],
    )
}

#[test]
fn run_writes_a_replay_that_verifies() {
    let dir = dir("replay_verifies");
    record_replay(&dir);
    let replay = Replay::decode(&fs::read(dir.join("run.replay")).unwrap()).unwrap();
    assert_eq!(replay.hashes, golden_hashes(&dir));
    for threads in ["1", "2"] {
        assert_eq!(
            verify_replay(&dir, threads),
            (0, "ok: 21 hashes match\n".to_string())
        );
    }
}

#[test]
fn verify_replay_fails_on_altered_hash_and_dumps_snapshots() {
    let dir = dir("replay_altered");
    record_replay(&dir);
    let path = dir.join("run.replay");
    let mut replay = Replay::decode(&fs::read(&path).unwrap()).unwrap();
    let expected = replay.hashes.clone();
    replay.hashes[7] ^= 1;
    fs::write(&path, replay.encode()).unwrap();
    let (code, out) = verify_replay(&dir, "1");
    assert_eq!(code, 1);
    assert!(out.contains("mismatch at tick 7"), "{out}");
    assert!(
        out.contains("tick-6.snap and ") && out.contains("tick-7.snap"),
        "{out}"
    );
    assert_eq!(snapshot_hash(&dir.join("dump/tick-6.snap")), expected[6]);
    assert_eq!(snapshot_hash(&dir.join("dump/tick-7.snap")), expected[7]);
}

static FLAKY_BUILDS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Counts twice as fast in the second app built, and only there.
fn flaky_app() -> App {
    let mut app = app();
    if FLAKY_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
        app.system(Sim, count);
    }
    app
}

#[test]
fn verify_reports_a_divergence_that_a_second_run_does_not_repeat() {
    let dir = dir("verify_flaky");
    let args = |args: &[&str]| -> Vec<String> {
        ["test"]
            .iter()
            .chain(args)
            .map(|arg| arg.replace("{}", dir.to_str().unwrap()))
            .collect()
    };
    let mut out = Vec::new();
    let record = args(&[
        "run",
        "{}/scene.ron",
        "--inputs",
        "{}/inputs.bin",
        "--ticks",
        "5",
        "--trace",
        "{}/golden.bin",
    ]);
    assert_eq!(run(flaky_app, &record, &mut out), 0);
    let verify = args(&[
        "verify",
        "{}/scene.ron",
        "--inputs",
        "{}/inputs.bin",
        "--golden",
        "{}/golden.bin",
        "--dump-dir",
        "{}/dump",
    ]);
    let mut out = Vec::new();
    assert_eq!(run(flaky_app, &verify, &mut out), 1);
    let out = String::from_utf8(out).unwrap();
    assert!(out.starts_with("mismatch at tick 1: "), "{out}");
    assert!(out.contains("not deterministic"), "{out}");
}

/// Run in a child process started in the scene's folder: the current directory is global
/// to the test process.
#[test]
fn project_assets_resolve_from_a_bare_scene_file_name() {
    const CHILD: &str = "LUDENEA_CLI_TEST_SCENE_FOLDER";
    if let Ok(folder) = std::env::var(CHILD) {
        assert_eq!(std::env::current_dir().unwrap(), PathBuf::from(folder));
        let scene = Path::new("main.scene.ron");
        let assets = ludenea_cli::assets::project_assets(scene, None).unwrap();
        assert_eq!(
            assets.texts.keys().copied().collect::<Vec<_>>(),
            [ludenea_ecs::AssetId(11)]
        );
        return;
    }
    let root = dir("bare_scene");
    fs::write(root.join("ludenea.toml"), "").unwrap();
    let folder = root.join("scenes/levels");
    fs::create_dir_all(&folder).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(folder.join("main.scene.ron"), "").unwrap();
    fs::write(root.join("assets/a.prefab.ron"), "Prefab(version: 1)").unwrap();
    fs::write(
        root.join("assets/a.prefab.ron.meta"),
        "Meta(version: 1, id: \"0000000000000000000000000000000b\", kind: Prefab)",
    )
    .unwrap();
    let folder = folder.canonicalize().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "project_assets_resolve_from_a_bare_scene_file_name",
        ])
        .current_dir(&folder)
        .env(CHILD, &folder)
        .status()
        .unwrap();
    assert!(status.success());
}

/// A broken single-line data asset is refused with its file's own line and column, not
/// those of the wrapped form the scan reads it in.
#[test]
fn broken_data_asset_names_its_file_line_and_column() {
    let dir = dir("broken_data_asset");
    let id = format!("{:032x}", 42);
    fs::write(
        dir.join("scene.ron"),
        format!(r#"Scene(version: 1, resources: {{}}, entities: [{{ "cli::Tuned": (tuning: "{id}") }}])"#),
    )
    .unwrap();
    fs::write(
        dir.join("tuning.ron.meta"),
        format!(r#"Meta(version: 1, id: "{id}", kind: Data(type: "cli::Tuning"))"#),
    )
    .unwrap();
    for (text, position) in [
        (r#"(speed: "fast")"#, "1:8-1:9: Expected integer"),
        ("(speed: 3 4)", "1:11: Expected"),
    ] {
        fs::write(dir.join("tuning.ron"), text).unwrap();
        let arguments = ["test", "run", "{}/scene.ron", "--ticks", "1"]
            .map(|argument| argument.replace("{}", dir.to_str().unwrap()));
        let mut out = Vec::new();
        assert_eq!(run(tuned_app, &arguments, &mut out), 2);
        let out = String::from_utf8(out).unwrap();
        let path = dir.join("tuning.ron");
        assert!(out.contains(path.to_str().unwrap()), "{out}");
        assert!(out.contains(&format!(" is invalid: {position}")), "{out}");
    }
}

#[test]
fn undecodable_input_log_is_named() {
    let dir = dir("undecodable_inputs");
    fs::write(dir.join("inputs.bin"), [0xff, 0xff]).unwrap();
    let (code, out) = cli(
        &dir,
        &[
            "run",
            "{}/scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--ticks",
            "1",
        ],
    );
    assert_eq!(code, 2);
    let path = dir.join("inputs.bin");
    assert!(
        out.contains(&format!("cannot decode {}", path.display())),
        "{out}"
    );
}

#[test]
fn undecodable_edit_log_is_named() {
    let dir = dir("undecodable_edits");
    fs::write(dir.join("edits.bin"), [0xff, 0xff]).unwrap();
    let (code, out) = cli(
        &dir,
        &[
            "run",
            "{}/scene.ron",
            "--edits",
            "{}/edits.bin",
            "--ticks",
            "1",
        ],
    );
    assert_eq!(code, 2);
    let path = dir.join("edits.bin");
    assert!(
        out.contains(&format!("cannot decode {}", path.display())),
        "{out}"
    );
}

#[test]
fn verifying_a_replay_refuses_run_definition_options() {
    let dir = dir("replay_options");
    record_replay(&dir);
    for option in [
        ["--inputs", "{}/inputs.bin"],
        ["--golden", "{}/golden.bin"],
        ["--edits", "{}/edits.bin"],
        ["--project", "{}"],
    ] {
        let (code, out) = cli(&dir, &["verify", "{}/run.replay", option[0], option[1]]);
        assert_eq!(code, 2, "{out}");
        assert!(out.contains("a replay holds its own"), "{out}");
    }
}

#[test]
fn empty_golden_trace_is_refused() {
    let dir = dir("empty_golden");
    record(&dir);
    let mut golden = Trace::decode(&fs::read(dir.join("golden.bin")).unwrap()).unwrap();
    golden.hashes.clear();
    fs::write(dir.join("golden.bin"), golden.encode()).unwrap();
    let (code, out) = verify(&dir);
    assert_eq!(code, 2);
    assert!(out.contains("has no hashes"), "{out}");
}

#[test]
fn scene_with_a_prefab_instance_runs_and_its_replay_verifies() {
    let dir = dir("prefab_scene");
    let id = format!("{:032x}", 21);
    fs::write(
        dir.join("main.scene.ron"),
        format!(
            r#"Scene(version: 2, next_id: 1, resources: {{}}, entities: [], instances: [(id: 0, prefab: "{id}")])"#
        ),
    )
    .unwrap();
    fs::write(
        dir.join("counter.prefab.ron"),
        r#"Prefab(version: 1, next_id: 2, entities: [(id: 0, components: {"cli::Counter": (3)}), (id: 1, components: {"cli::Counter": (9)})])"#,
    )
    .unwrap();
    fs::write(
        dir.join("counter.prefab.ron.meta"),
        format!(r#"Meta(version: 1, id: "{id}", kind: Prefab)"#),
    )
    .unwrap();
    let (code, out) = cli(
        &dir,
        &[
            "run",
            "{}/main.scene.ron",
            "--inputs",
            "{}/inputs.bin",
            "--ticks",
            "10",
            "--replay",
            "{}/run.replay",
        ],
    );
    assert_eq!(code, 0, "{out}");
    let replay = Replay::decode(&fs::read(dir.join("run.replay")).unwrap()).unwrap();
    assert_eq!(replay.assets.len(), 1);
    assert!(replay.scene.contains("address: [0, 1]"), "{}", replay.scene);
    assert_eq!(
        verify_replay(&dir, "1"),
        (0, "ok: 11 hashes match\n".to_string())
    );
}
