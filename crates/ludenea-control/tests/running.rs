use std::fs;

use boxes::BoxesInput;
use ludenea::cli::Trace;
use ludenea::control::Event;
use ludenea::control::*;
use ludenea::prelude::*;

const SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.scene.ron"
);
const INPUTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.inputs"
);
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/boxes.trace");

/// `ticks` recorded inputs, with a nudge every 37 ticks.
fn recorded(ticks: u64) -> Bytes {
    let mut log = InputLog::new(1);
    for tick in 0..ticks {
        log.record(&[BoxesInput {
            velocity_change: (tick % 37 == 0).then(|| Vec2::new(1.0, 4.0)),
            ..BoxesInput::default()
        }]);
    }
    Bytes(log.encode())
}

fn scene_a() -> String {
    boxes::generate_scene(30, 7)
}

fn scene_b() -> String {
    boxes::generate_scene(31, 8)
}

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
}

fn error(host: &mut Host, command: Command) -> ErrorCode {
    host.handle(command).unwrap_err().code
}

/// A boxes host with `scene` and `inputs` loaded.
fn loaded_with(app: fn() -> App, scene: String, inputs: Bytes) -> Host {
    let mut host = Host::new(app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene,
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    ok(&mut host, Command::LoadInputs { log: inputs });
    host
}

fn loaded(ticks: u64) -> Host {
    loaded_with(boxes::app, scene_a(), recorded(ticks))
}

fn step(host: &mut Host, ticks: u32) -> Vec<Hash64> {
    match ok(
        host,
        Command::Step {
            ticks,
            record_hashes: true,
        },
    ) {
        Reply::Stepped { hashes, .. } => hashes,
        other => panic!("{other:?}"),
    }
}

fn timeline(host: &mut Host) -> Timeline {
    match ok(host, Command::GetTimeline) {
        Reply::Timeline(timeline) => timeline,
        other => panic!("{other:?}"),
    }
}

fn hash(host: &mut Host) -> Hash64 {
    match ok(host, Command::GetHash) {
        Reply::Hash { hash, .. } => hash,
        other => panic!("{other:?}"),
    }
}

struct Export {
    scene: String,
    saved_scene: Option<String>,
    inputs: Bytes,
    edits: Bytes,
    undo: Bytes,
    hashes: Vec<Hash64>,
}

fn export(host: &mut Host) -> Export {
    match ok(host, Command::ExportSession) {
        Reply::Session {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            hashes,
            ..
        } => Export {
            scene,
            saved_scene,
            inputs,
            edits,
            undo,
            hashes,
        },
        other => panic!("{other:?}"),
    }
}

fn load_session(export: Export, expected: Vec<Hash64>) -> Command {
    Command::LoadSession {
        scene: export.scene,
        saved_scene: export.saved_scene,
        inputs: export.inputs,
        edits: export.edits,
        undo: export.undo,
        expected,
        assets: Vec::new(),
        saved_assets: Vec::new(),
        branches: Bytes(Vec::new()),
        document: Document::Scene,
        keeps_assets: false,
    }
}

/// Advances while paused until resimulation is done, returning every event.
fn resimulate(host: &mut Host) -> Vec<Event> {
    let mut events = Vec::new();
    for _ in 0..1_000 {
        events.extend(host.advance(0.0));
        if !timeline(host).resimulating {
            events.extend(host.advance(0.0));
            return events;
        }
    }
    panic!("resimulation never finished: {events:?}");
}

#[test]
fn cli_style_run_through_host_matches_golden() {
    let mut host = loaded_with(
        boxes::app,
        fs::read_to_string(SCENE).unwrap(),
        Bytes(fs::read(INPUTS).unwrap()),
    );
    let mut hashes = vec![hash(&mut host).0];
    hashes.extend(step(&mut host, 10_000).iter().map(|hash| hash.0));
    let golden = Trace::decode(&fs::read(GOLDEN).unwrap()).unwrap();
    assert_eq!(hashes, golden.hashes);
}

#[test]
fn restore_then_step_reproduces_hashes() {
    let mut host = loaded(100);
    step(&mut host, 20);
    let Reply::Snapshot { snapshot, tick } = ok(&mut host, Command::TakeSnapshot) else {
        panic!()
    };
    assert_eq!(tick, 20);
    let first = step(&mut host, 30);
    ok(&mut host, Command::Restore { snapshot });
    assert_eq!(timeline(&mut host).current, 20);
    assert_eq!(step(&mut host, 30), first);
    assert_eq!(timeline(&mut host).head, 99);
}

#[test]
fn play_then_advance_runs_ticks_and_emits_ticked() {
    let mut host = loaded(100);
    // Announces the loaded state, before play.
    host.advance(0.0);
    ok(&mut host, Command::Play);
    let mut ticked = Vec::new();
    for _ in 0..10 {
        for event in host.advance(1.0 / 60.0) {
            if let Event::Ticked { tick, playing, .. } = event {
                assert!(playing);
                ticked.push(tick);
            }
        }
    }
    assert_eq!(ticked, (1..=10).collect::<Vec<_>>());
    assert_eq!(timeline(&mut host).current, 10);

    ok(&mut host, Command::Pause);
    let events = host.advance(1.0);
    assert!(
        events.contains(&Event::Paused {
            reason: PauseReason::User
        }),
        "{events:?}"
    );
    assert!(!events.iter().any(|e| matches!(e, Event::Ticked { .. })));
    assert_eq!(timeline(&mut host).current, 10);
}

#[test]
fn replaying_simulated_ticks_does_not_report_the_timeline() {
    let mut host = loaded(100);
    step(&mut host, 20);
    ok(&mut host, Command::Seek { tick: 0 });
    host.advance(0.0);
    ok(&mut host, Command::Play);
    let mut events = Vec::new();
    for _ in 0..10 {
        events.extend(host.advance(1.0 / 60.0));
    }
    assert_eq!(timeline(&mut host).current, 10);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::TimelineChanged { .. })),
        "{events:?}"
    );
}

#[test]
fn speed_changes_ticks_per_second_and_rejects_non_finite() {
    let mut host = loaded(100);
    ok(&mut host, Command::SetSpeed { speed: 2.0 });
    ok(&mut host, Command::Play);
    for _ in 0..30 {
        host.advance(1.0 / 60.0);
    }
    assert_eq!(timeline(&mut host).current, 60);
    assert_eq!(
        error(&mut host, Command::SetSpeed { speed: f32::NAN }),
        ErrorCode::InvalidCommand
    );
}

#[test]
fn live_input_takes_over_the_recorded_future() {
    let mut host = loaded(100);
    let idle = RawInput::default();
    ok(
        &mut host,
        Command::SetLiveInput {
            players: vec![idle],
        },
    );
    ok(&mut host, Command::Play);
    host.advance(1.0 / 60.0);
    assert_eq!(
        timeline(&mut host).head,
        99,
        "idle input replays the recording"
    );

    let spawn = RawInput {
        keys: ["KeyS".to_string()].into(),
        ..RawInput::default()
    };
    ok(
        &mut host,
        Command::SetLiveInput {
            players: vec![spawn],
        },
    );
    host.advance(1.0 / 60.0);
    assert_eq!(timeline(&mut host).head, 1);
}

fn panic_at_50(time: Res<Time>) {
    if time.tick == 50 {
        panic!("boom at fifty");
    }
}

/// Boxes with a system that panics at tick 50.
fn panicking_app() -> App {
    let mut app = boxes::app();
    app.system(Sim, panic_at_50);
    app
}

#[test]
fn panicking_system_pauses_with_panicked_and_keeps_session() {
    let inputs = recorded(100);
    let mut host = loaded_with(panicking_app, scene_a(), inputs.clone());
    ok(&mut host, Command::Play);

    let mut events = Vec::new();
    for _ in 0..100 {
        events.extend(host.advance(1.0 / 60.0));
    }
    let step_error = error(
        &mut host,
        Command::Step {
            ticks: 1,
            record_hashes: false,
        },
    );

    let paused: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::Paused { reason } => Some(reason),
            _ => None,
        })
        .collect();
    let [PauseReason::Panicked { tick: 50, message }] = paused.as_slice() else {
        panic!("unexpected events {events:?}");
    };
    assert!(
        message.starts_with("boom at fifty (in system `running::panic_at_50`, "),
        "{message}"
    );
    assert!(message.contains("tests/running.rs:"), "{message}");
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Console {
            level: ConsoleLevel::Error,
            text
        } if text.starts_with(
            "the game panicked at tick 50 in system `running::panic_at_50`: boom at fifty ("
        )
    )));
    assert_eq!(step_error, ErrorCode::SystemPanicked);
    assert_eq!(timeline(&mut host).current, 50);
    assert_eq!(export(&mut host).inputs, inputs);
}

#[test]
fn replace_scene_at_a_later_tick_returns_there_after_resimulation() {
    let mut host = loaded(100);
    step(&mut host, 40);
    ok(
        &mut host,
        Command::ReplaceScene {
            scene: scene_b(),
            assets: Vec::new(),
        },
    );
    let during = timeline(&mut host);
    assert!(during.resimulating && during.current == 0, "{during:?}");
    resimulate(&mut host);
    assert_eq!(timeline(&mut host).current, 40);

    let mut fresh = loaded_with(boxes::app, scene_b(), recorded(100));
    step(&mut fresh, 40);
    assert_eq!(hash(&mut host), hash(&mut fresh));

    ok(&mut host, Command::Undo);
    resimulate(&mut host);
    assert_eq!(timeline(&mut host).current, 40);
    ok(&mut host, Command::Redo);
    resimulate(&mut host);
    assert_eq!(timeline(&mut host).current, 40);
    assert_eq!(hash(&mut host), hash(&mut fresh));
}

#[test]
fn undo_and_redo_through_protocol() {
    let mut host = loaded(60);
    step(&mut host, 60);
    ok(
        &mut host,
        Command::ReplaceScene {
            scene: scene_b(),
            assets: Vec::new(),
        },
    );
    assert!(timeline(&mut host).can_undo);
    assert_eq!(error(&mut host, Command::Undo), ErrorCode::Resimulating);
    resimulate(&mut host);

    ok(&mut host, Command::Undo);
    resimulate(&mut host);
    assert_eq!(export(&mut host).scene, scene_a());
    let after_undo = timeline(&mut host);
    assert!(after_undo.scene_dirty && after_undo.can_redo && !after_undo.can_undo);
    assert_eq!(error(&mut host, Command::Undo), ErrorCode::NothingToUndo);

    ok(&mut host, Command::Redo);
    resimulate(&mut host);
    assert_eq!(export(&mut host).scene, scene_b());
    assert!(!timeline(&mut host).scene_dirty);
    assert_eq!(error(&mut host, Command::Redo), ErrorCode::NothingToRedo);
}

#[test]
fn export_session_replays_in_fresh_host() {
    let mut host = loaded(100);
    step(&mut host, 80);
    ok(&mut host, Command::Seek { tick: 40 });
    let exported = export(&mut host);
    let hashes = exported.hashes.clone();
    assert_eq!(hashes.len(), 81);

    let mut fresh = Host::new(boxes::app).unwrap();
    ok(&mut fresh, load_session(exported, hashes.clone()));
    let events = resimulate(&mut fresh);
    assert!(
        !events.iter().any(|e| matches!(e, Event::Diverged { .. })),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Resimulated { up_to: 99 }),
        "{events:?}"
    );
    ok(&mut fresh, Command::Seek { tick: 80 });
    assert_eq!(hash(&mut fresh), hashes[80]);
    assert_eq!(export(&mut fresh).hashes[..=80], hashes[..]);
}

#[test]
fn load_session_with_changed_code_reports_diverged() {
    let mut host = loaded(100);
    step(&mut host, 80);
    let exported = export(&mut host);
    let original = exported.hashes[30];
    let mut expected = exported.hashes.clone();
    expected[30] = Hash64(original.0 ^ 1);

    let mut fresh = Host::new(boxes::app).unwrap();
    ok(&mut fresh, load_session(exported, expected.clone()));
    let events = resimulate(&mut fresh);
    let diverged: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::Diverged { .. }))
        .collect();
    assert_eq!(
        diverged,
        [&Event::Diverged {
            tick: 30,
            expected: expected[30],
            got: original
        }]
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Console {
            level: ConsoleLevel::Warning,
            ..
        }
    )));
}

#[test]
fn replace_scene_keeps_recorded_inputs() {
    let inputs = recorded(100);
    let mut host = loaded(100);
    step(&mut host, 100);
    ok(
        &mut host,
        Command::ReplaceScene {
            scene: scene_b(),
            assets: Vec::new(),
        },
    );
    resimulate(&mut host);
    let replaced = export(&mut host);
    assert_eq!(replaced.inputs, inputs);
    assert_eq!(replaced.scene, scene_b());
    assert!(!timeline(&mut host).scene_dirty);

    let mut direct = loaded_with(boxes::app, scene_b(), inputs);
    let mut hashes = vec![hash(&mut direct)];
    hashes.extend(step(&mut direct, 100));
    assert_eq!(replaced.hashes, hashes);
}

#[test]
fn seek_while_resimulating_is_resimulating() {
    let mut host = loaded(100);
    step(&mut host, 100);
    ok(
        &mut host,
        Command::ReplaceScene {
            scene: scene_b(),
            assets: Vec::new(),
        },
    );
    assert_eq!(
        error(&mut host, Command::Seek { tick: 90 }),
        ErrorCode::Resimulating
    );
    assert_eq!(
        error(
            &mut host,
            Command::Step {
                ticks: 1,
                record_hashes: false
            }
        ),
        ErrorCode::Resimulating
    );
}

#[test]
fn panic_during_resimulation_ends_it_and_keeps_undo_and_export() {
    let inputs = recorded(100);
    let mut host = loaded_with(panicking_app, scene_a(), inputs.clone());
    step(&mut host, 40);
    ok(
        &mut host,
        Command::ReplaceScene {
            scene: scene_b(),
            assets: Vec::new(),
        },
    );

    let events = host.advance(0.0);

    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Paused {
                reason: PauseReason::Panicked { tick: 50, .. }
            }
        )),
        "{events:?}"
    );
    let after = timeline(&mut host);
    assert!(!after.resimulating);
    assert_eq!((after.simulated, after.head), (50, 99));
    let exported = export(&mut host);
    assert_eq!(exported.inputs, inputs);
    assert_eq!(exported.hashes.len(), 51);
    ok(&mut host, Command::Undo);
    assert_eq!(timeline(&mut host).current, 0);
}

fn snapshot_at(scene: String, tick: u32) -> (Bytes, Hash64) {
    let mut host = loaded_with(boxes::app, scene, recorded(100));
    step(&mut host, tick);
    let Reply::Snapshot { snapshot, .. } = ok(&mut host, Command::TakeSnapshot) else {
        panic!()
    };
    (snapshot, hash(&mut host))
}

#[test]
fn restore_foreign_snapshot_becomes_the_state_of_its_tick() {
    let (foreign, foreign_hash) = snapshot_at(scene_b(), 20);
    let mut host = loaded(100);
    step(&mut host, 50);
    ok(&mut host, Command::Restore { snapshot: foreign });
    assert_eq!(hash(&mut host), foreign_hash);
    let restored = timeline(&mut host);
    assert_eq!((restored.current, restored.simulated), (20, 20));

    let continued = step(&mut host, 60);
    ok(&mut host, Command::Seek { tick: 5 });
    ok(&mut host, Command::Seek { tick: 20 });
    assert_eq!(hash(&mut host), foreign_hash, "the checkpoint is kept");
    assert_eq!(step(&mut host, 60), continued);
    assert_eq!(export(&mut host).hashes[20], foreign_hash);
}

#[test]
fn restore_past_the_simulated_ticks_extends_them() {
    let (own, own_hash) = snapshot_at(scene_a(), 40);
    let mut host = loaded(100);
    ok(&mut host, Command::Restore { snapshot: own });
    let restored = timeline(&mut host);
    assert_eq!((restored.current, restored.simulated), (40, 40));
    assert_eq!(export(&mut host).hashes[40], own_hash);

    let (foreign, foreign_hash) = snapshot_at(scene_b(), 30);
    let mut host = loaded(100);
    ok(&mut host, Command::Restore { snapshot: foreign });
    assert_eq!(timeline(&mut host).simulated, 30);
    ok(&mut host, Command::Seek { tick: 0 });
    ok(&mut host, Command::Seek { tick: 30 });
    assert_eq!(hash(&mut host), foreign_hash);
}

fn ticked(events: &[Event]) -> Vec<(u64, Hash64)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Ticked { tick, hash, .. } => Some((*tick, *hash)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_command_that_changes_the_state_announces_it_once() {
    let mut host = Host::new(boxes::app).unwrap();
    host.advance(0.0);
    ok(
        &mut host,
        Command::LoadScene {
            scene: scene_a(),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let loaded = hash(&mut host);
    assert_eq!(ticked(&host.advance(0.0)), vec![(0, loaded)]);
    assert!(ticked(&host.advance(0.0)).is_empty());

    ok(&mut host, Command::LoadInputs { log: recorded(100) });
    step(&mut host, 5);
    host.advance(0.0);
    ok(&mut host, Command::Seek { tick: 2 });
    let seeked = hash(&mut host);
    assert_eq!(ticked(&host.advance(0.0)), vec![(2, seeked)]);

    let entity = match ok(
        &mut host,
        Command::QueryEntities {
            filter: "With<Crate>".to_string(),
            offset: 0,
            limit: 1,
        },
    ) {
        Reply::Entities { rows, .. } => rows[0].entity,
        other => panic!("{other:?}"),
    };
    ok(
        &mut host,
        Command::SetField {
            target: FieldTarget::Component {
                entity,
                component: "ludenea::Pose".to_string(),
            },
            path: vec![
                PathSegment::Field {
                    name: "position".to_string(),
                },
                PathSegment::Field {
                    name: "x".to_string(),
                },
            ],
            value: serde_json::json!(0.25),
        },
    );
    let edited = hash(&mut host);
    assert_eq!(ticked(&host.advance(0.0)), vec![(2, edited)]);
    ok(&mut host, Command::Undo);
    assert_eq!(ticked(&host.advance(0.0)), vec![(2, seeked)]);
}

#[test]
fn a_panicking_step_names_its_tick_once() {
    let mut host = loaded_with(panicking_app, scene_a(), recorded(100));
    step(&mut host, 50);
    let error = host
        .handle(Command::Step {
            ticks: 1,
            record_hashes: false,
        })
        .unwrap_err();
    assert_eq!(
        error.message.matches("tick 50").count(),
        1,
        "{}",
        error.message
    );
}

#[test]
fn reads_announce_nothing() {
    let mut host = loaded(100);
    host.advance(0.0);
    ok(&mut host, Command::GetHash);
    ok(&mut host, Command::GetTimeline);
    ok(&mut host, Command::TakeSnapshot);
    assert!(ticked(&host.advance(0.0)).is_empty());
}

#[test]
fn load_session_with_empty_logs_loads_only_its_scene() {
    let mut host = Host::new(boxes::app).unwrap();
    let load = Command::LoadSession {
        scene: scene_a(),
        saved_scene: Some(scene_b()),
        inputs: Bytes(Vec::new()),
        edits: Bytes(Vec::new()),
        undo: Bytes(Vec::new()),
        expected: Vec::new(),
        assets: Vec::new(),
        saved_assets: Vec::new(),
        branches: Bytes(Vec::new()),
        document: Document::Scene,
        keeps_assets: false,
    };
    ok(&mut host, load);
    let timeline = timeline(&mut host);
    assert_eq!(
        (timeline.head, timeline.simulated, timeline.current),
        (0, 0, 0)
    );
    assert!(timeline.scene_dirty && !timeline.resimulating);
    assert_eq!(export(&mut host).scene, scene_a());
}

/// Boxes whose input mapping reads the first gamepad, which a keyboard player lacks.
fn gamepad_app() -> App {
    let mut app = boxes::app();
    app.input_mapping(|raw: &RawInput, _| BoxesInput {
        spawn: Some(raw.gamepads[0].axes[0]),
        ..BoxesInput::default()
    });
    app
}

#[test]
fn a_panicking_input_mapping_fails_set_live_input_and_keeps_the_host() {
    let mut host = loaded_with(gamepad_app, scene_a(), recorded(100));
    let keyboard = RawInput {
        keys: ["KeyS".to_string()].into(),
        ..RawInput::default()
    };

    let error = host
        .handle(Command::SetLiveInput {
            players: vec![keyboard],
        })
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::SystemPanicked);
    assert!(
        error.message.contains("index out of bounds"),
        "{}",
        error.message
    );
    ok(&mut host, Command::GetHash);
    assert_eq!(step(&mut host, 1).len(), 1);
}

static SUPERSTITIOUS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Asserts that a luck is never 13 once `SUPERSTITIOUS` is on: a later build of the game
/// whose type panics on a value an earlier build recorded.
#[derive(Component, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u32")]
struct Luck(u32);

impl TryFrom<u32> for Luck {
    type Error = String;

    fn try_from(luck: u32) -> Result<Self, String> {
        let superstitious = SUPERSTITIOUS.load(std::sync::atomic::Ordering::SeqCst);
        assert!(!(superstitious && luck == 13), "13 is unlucky");
        Ok(Luck(luck))
    }
}

fn luck_app() -> App {
    let mut app = App::new();
    app.component::<Luck>();
    app
}

#[test]
fn a_recorded_edit_whose_value_now_panics_is_dropped_while_resimulating() {
    let mut host = Host::new(luck_app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: r#"Scene(version: 1, resources: {}, entities: [{ "running::Luck": 0 }])"#.into(),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    step(&mut host, 3);
    let Reply::Entities { rows, .. } = ok(
        &mut host,
        Command::QueryEntities {
            filter: String::new(),
            offset: 0,
            limit: 1,
        },
    ) else {
        panic!()
    };
    ok(
        &mut host,
        Command::SetField {
            target: FieldTarget::Component {
                entity: rows[0].entity,
                component: "running::Luck".to_string(),
            },
            path: Vec::new(),
            value: serde_json::json!(13),
        },
    );
    step(&mut host, 2);
    let exported = export(&mut host);
    let hashes = exported.hashes.clone();

    SUPERSTITIOUS.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut fresh = Host::new(luck_app).unwrap();
    ok(&mut fresh, load_session(exported, hashes));
    let events = resimulate(&mut fresh);
    SUPERSTITIOUS.store(false, std::sync::atomic::Ordering::SeqCst);

    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::EditDropped { tick: 3, reason }
                if reason.starts_with("the game panicked at tick 3: 13 is unlucky (")
        )),
        "{events:?}"
    );
    assert_eq!(timeline(&mut fresh).simulated, 5);
}

/// An asset type whose default panics.
#[derive(Asset, serde::Serialize, serde::Deserialize)]
struct Calm {
    level: u32,
}

impl Default for Calm {
    fn default() -> Self {
        panic!("no calm by default")
    }
}

/// An asset type whose `try_from` panics on a level of 13.
#[derive(Asset, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "MoodLevel")]
struct Mood {
    level: u32,
}

#[derive(serde::Deserialize)]
struct MoodLevel {
    level: u32,
}

impl TryFrom<MoodLevel> for Mood {
    type Error = String;

    fn try_from(MoodLevel { level }: MoodLevel) -> Result<Self, String> {
        assert!(level != 13, "13 is a bad mood");
        Ok(Mood { level })
    }
}

/// Holds the `Mood` its scene loads.
#[derive(Component, serde::Serialize, serde::Deserialize)]
struct Moody {
    mood: Handle<Mood>,
}

const MOOD: ludenea::ecs::AssetId = ludenea::ecs::AssetId(0xd);

/// An app whose editor hooks and asset types panic.
fn hooks_app() -> App {
    let mut app = App::new();
    app.component::<Moody>()
        .asset::<Mood>()
        .asset_with_default::<Calm>()
        .picker(|_, _| panic!("no picking here"))
        .debug_draw(&["broken"], |_, _, _| panic!("no drawing here"));
    app
}

fn hooks_host() -> Host {
    let mut host = Host::new(hooks_app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: format!(
                r#"Scene(version: 1, resources: {{}}, entities: [{{ "running::Moody": (mood: "{MOOD}") }}])"#
            ),
            assets: vec![(
                MOOD,
                format!("(type: \"{}\", value: (level: 1))", Mood::NAME),
            )],
            document: Document::Scene,
        },
    );
    host
}

/// The panic of game code run outside a tick, named as a system panic is, and caught where it
/// ran: the host neither recovers the session nor pauses.
fn assert_panicked(host: &mut Host, error: &ProtocolError, message: &str) {
    assert_eq!(error.code, ErrorCode::SystemPanicked, "{}", error.message);
    // After the command's context, when it gives one.
    let panicked = error.message.find("the game panicked").unwrap_or(0);
    let located = error.message[panicked..]
        .strip_prefix(&format!("the game panicked at tick 0: {message} ("))
        .and_then(|rest| rest.strip_suffix(')'));
    assert!(
        located.is_some_and(|location| location.contains("running.rs:")),
        "{}",
        error.message
    );
    let events = host.drain_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Paused { .. })),
        "{events:?}"
    );
}

#[test]
fn a_panicking_picker_or_debug_drawer_fails_its_command_and_keeps_the_host() {
    let mut host = hooks_host();

    let picked = host
        .handle(Command::Pick {
            point: Vec2::new(0.0, 0.0),
        })
        .unwrap_err();
    let drawn = host
        .handle(Command::GetDebugDraw {
            layers: vec!["broken".to_string()],
        })
        .unwrap_err();

    assert_panicked(&mut host, &picked, "no picking here");
    assert_panicked(&mut host, &drawn, "no drawing here");
    assert_eq!(step(&mut host, 1).len(), 1);
}

#[test]
fn a_panicking_asset_default_fails_new_asset_value_and_keeps_the_host() {
    let mut host = hooks_host();

    let error = host
        .handle(Command::NewAssetValue {
            asset: Calm::NAME.to_string(),
        })
        .unwrap_err();

    assert_panicked(&mut host, &error, "no calm by default");
    assert_eq!(step(&mut host, 1).len(), 1);
}

#[test]
fn a_panicking_asset_try_from_fails_set_asset_field_and_changes_nothing() {
    let mut host = hooks_host();
    let before = ok(&mut host, Command::GetAsset { id: MOOD });

    let error = host
        .handle(Command::SetAssetField {
            id: MOOD,
            path: vec![PathSegment::Field {
                name: "level".to_string(),
            }],
            value: serde_json::json!(13),
        })
        .unwrap_err();

    assert_panicked(&mut host, &error, "13 is a bad mood");
    assert!(
        error.message.contains(&format!("asset {MOOD}")),
        "{}",
        error.message
    );
    assert_eq!(ok(&mut host, Command::GetAsset { id: MOOD }), before);
    assert!(!timeline(&mut host).can_undo);
}
