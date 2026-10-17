use std::fs;

use boxes::BoxesInput;
use ludenea::cli::Trace;
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
fn loaded_with(app: App, scene: String, inputs: Bytes) -> Host {
    let mut host = Host::new(app).unwrap();
    ok(&mut host, Command::LoadScene { ron: scene });
    ok(&mut host, Command::LoadInputs { log: inputs });
    host
}

fn loaded(ticks: u64) -> Host {
    loaded_with(boxes::app(), scene_a(), recorded(ticks))
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
        boxes::app(),
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

#[test]
fn panicking_system_pauses_with_panicked_and_keeps_session() {
    let mut app = boxes::app();
    app.system(Sim, panic_at_50);
    let inputs = recorded(100);
    let mut host = loaded_with(app, scene_a(), inputs.clone());
    ok(&mut host, Command::Play);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
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
    std::panic::set_hook(hook);

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
    assert!(message.contains("boom at fifty"), "{message}");
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Console {
            level: ConsoleLevel::Error,
            text
        } if text.contains("boom at fifty")
    )));
    assert_eq!(step_error, ErrorCode::SystemPanicked);
    assert_eq!(timeline(&mut host).current, 50);
    assert_eq!(export(&mut host).inputs, inputs);
}

#[test]
fn replace_scene_at_a_later_tick_returns_there_after_resimulation() {
    let mut host = loaded(100);
    step(&mut host, 40);
    ok(&mut host, Command::ReplaceScene { ron: scene_b() });
    let during = timeline(&mut host);
    assert!(during.resimulating && during.current == 0, "{during:?}");
    resimulate(&mut host);
    assert_eq!(timeline(&mut host).current, 40);

    let mut fresh = loaded_with(boxes::app(), scene_b(), recorded(100));
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
    ok(&mut host, Command::ReplaceScene { ron: scene_b() });
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

    let mut fresh = Host::new(boxes::app()).unwrap();
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

    let mut fresh = Host::new(boxes::app()).unwrap();
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
    ok(&mut host, Command::ReplaceScene { ron: scene_b() });
    resimulate(&mut host);
    let replaced = export(&mut host);
    assert_eq!(replaced.inputs, inputs);
    assert_eq!(replaced.scene, scene_b());
    assert!(!timeline(&mut host).scene_dirty);

    let mut direct = loaded_with(boxes::app(), scene_b(), inputs);
    let mut hashes = vec![hash(&mut direct)];
    hashes.extend(step(&mut direct, 100));
    assert_eq!(replaced.hashes, hashes);
}

#[test]
fn seek_while_resimulating_is_resimulating() {
    let mut host = loaded(100);
    step(&mut host, 100);
    ok(&mut host, Command::ReplaceScene { ron: scene_b() });
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
    let mut app = boxes::app();
    app.system(Sim, panic_at_50);
    let inputs = recorded(100);
    let mut host = loaded_with(app, scene_a(), inputs.clone());
    step(&mut host, 40);
    ok(&mut host, Command::ReplaceScene { ron: scene_b() });

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let events = host.advance(0.0);
    std::panic::set_hook(hook);

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
    let mut host = loaded_with(boxes::app(), scene, recorded(100));
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
    let mut host = Host::new(boxes::app()).unwrap();
    host.advance(0.0);
    ok(&mut host, Command::LoadScene { ron: scene_a() });
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
    let mut app = boxes::app();
    app.system(Sim, panic_at_50);
    let mut host = loaded_with(app, scene_a(), recorded(100));
    step(&mut host, 50);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let error = host
        .handle(Command::Step {
            ticks: 1,
            record_hashes: false,
        })
        .unwrap_err();
    std::panic::set_hook(hook);
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
    let mut host = Host::new(boxes::app()).unwrap();
    let load = Command::LoadSession {
        scene: scene_a(),
        saved_scene: Some(scene_b()),
        inputs: Bytes(Vec::new()),
        edits: Bytes(Vec::new()),
        undo: Bytes(Vec::new()),
        expected: Vec::new(),
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
