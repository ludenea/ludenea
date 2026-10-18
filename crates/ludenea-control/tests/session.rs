use std::fs;

use boxes::{BoxesInput, Crate};
use ludenea::cli::Trace;
use ludenea::control::{Document, Resimulation, Session, SessionError, SessionExport};
use ludenea::prelude::*;
use ludenea_app::{AssetTexts, EntityRef, ValueKind};
use serde_json::json;

const SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.scene.ron"
);
const INPUTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.inputs"
);
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../golden/boxes.trace");

fn simulation() -> Simulation {
    boxes::app().build().unwrap()
}

fn small_scene() -> String {
    boxes::generate_scene(30, 7)
}

fn encode(input: BoxesInput) -> Vec<u8> {
    let mut log = InputLog::new(1);
    log.record(&[input]);
    log.ticks.remove(0)
}

fn spawn_input(x: f32) -> Vec<u8> {
    encode(BoxesInput {
        spawn: Some(x),
        ..BoxesInput::default()
    })
}

/// `ticks` recorded inputs: a nudge every 37 ticks and a spawn at `spawn_at`.
fn recorded(ticks: u64, spawn_at: Option<u64>) -> InputLog {
    let mut log = InputLog::new(1);
    for tick in 0..ticks {
        log.record(&[BoxesInput {
            velocity_change: (tick % 37 == 0).then(|| Vec2::new(1.0, 4.0)),
            spawn: (spawn_at == Some(tick)).then_some(3.0),
            despawn: false,
        }]);
    }
    log
}

fn started(log: InputLog) -> (Simulation, Session) {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    session.load_inputs(&mut sim, log).unwrap();
    (sim, session)
}

fn play(session: &mut Session, sim: &mut Simulation, ticks: u64) {
    for _ in 0..ticks {
        session.step(sim, None).unwrap();
    }
}

fn hashes(session: &Session) -> Vec<u64> {
    session.export().unwrap().hashes
}

fn crate_count(sim: &Simulation) -> usize {
    Query::<&Pose, With<Crate>>::new(sim.world())
        .unwrap()
        .iter()
        .count()
}

fn stats_edit(sim: &Simulation, begins: u64) -> Edit {
    let value = sim
        .encode_json_value(
            ValueKind::Resource,
            "boxes::ContactStats",
            json!({ "begins": begins, "ends": 0 }),
        )
        .unwrap();
    Edit::SetResource {
        resource: "boxes::ContactStats".to_string(),
        value,
    }
}

fn last_entity(sim: &Simulation) -> Entity {
    sim.world().entities().last().unwrap()
}

/// Runs resimulation to the end and returns every report other than `Progress`.
fn resimulate_all(session: &mut Session, sim: &mut Simulation) -> Vec<Resimulation> {
    let mut reports = Vec::new();
    loop {
        match session.resimulate(sim, 64).unwrap() {
            Resimulation::Progress { .. } => {}
            Resimulation::Idle => panic!("resimulation ended without Done"),
            Resimulation::Done => {
                reports.push(Resimulation::Done);
                return reports;
            }
            report => reports.push(report),
        }
    }
}

/// Replays an export on a fresh simulation, tick by tick, against its hashes.
fn assert_fresh_replay_matches(export: &SessionExport) {
    let mut fresh = simulation();
    fresh.load_scene(&export.scene).unwrap();
    let inputs = InputLog::decode(&export.inputs).unwrap();
    let edits = EditLog::decode(&export.edits).unwrap();
    fresh.set_players(usize::from(inputs.players));
    assert_eq!(fresh.hash(), export.hashes[0]);
    for (tick, expected) in export.hashes.iter().enumerate().skip(1) {
        let tick = tick as u64 - 1;
        for timed in edits.at(tick) {
            fresh.apply_edit(&timed.edit).unwrap();
        }
        fresh.step_encoded(inputs.tick(tick)).unwrap();
        assert_eq!(fresh.hash(), *expected, "tick {}", tick + 1);
    }
}

#[test]
fn golden_inputs_through_session_match_golden_trace() {
    let mut sim = simulation();
    let mut session = Session::start(
        &mut sim,
        fs::read_to_string(SCENE).unwrap(),
        AssetTexts::new(),
        Document::Scene,
    )
    .unwrap();
    let log = InputLog::decode(&fs::read(INPUTS).unwrap()).unwrap();
    session.load_inputs(&mut sim, log).unwrap();
    for tick in 1..=10_000 {
        assert_eq!(session.step(&mut sim, None).unwrap(), tick);
    }
    let golden = Trace::decode(&fs::read(GOLDEN).unwrap()).unwrap();
    assert_eq!(hashes(&session), golden.hashes);
}

#[test]
fn replaying_recorded_ticks_matches_original_hashes() {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    for tick in 0..300 {
        let live = (tick % 50 == 0).then(|| spawn_input(tick as f32 / 50.0));
        session.step(&mut sim, live.as_deref()).unwrap();
    }
    let recorded = hashes(&session);
    assert_eq!(
        session.timeline().head,
        299,
        "defaults are recorded past the head"
    );
    session.seek(&mut sim, 0).unwrap();
    for (tick, &expected) in recorded.iter().enumerate().skip(1) {
        session.step(&mut sim, None).unwrap();
        assert_eq!(sim.hash(), expected, "tick {tick}");
    }
}

#[test]
fn seek_back_then_forward_reproduces_hashes() {
    let (mut sim, mut session) = started(recorded(400, Some(200)));
    play(&mut session, &mut sim, 400);
    let recorded = hashes(&session);
    for tick in [123, 377, 5, 400, 0, 250] {
        session.seek(&mut sim, tick).unwrap();
        assert_eq!(session.timeline().current, tick);
        assert_eq!(sim.hash(), recorded[tick as usize], "tick {tick}");
    }
}

#[test]
fn past_spawn_survives_seek_back_and_forth_without_duplicates() {
    let (mut sim, mut session) = started(recorded(400, Some(200)));
    play(&mut session, &mut sim, 300);
    let (hash, count) = (sim.hash(), crate_count(&sim));
    session.step(&mut sim, None).unwrap();
    let next = sim.hash();
    session.seek(&mut sim, 100).unwrap();
    session.seek(&mut sim, 300).unwrap();
    assert_eq!(sim.hash(), hash);
    assert_eq!(crate_count(&sim), count);
    session.step(&mut sim, None).unwrap();
    assert_eq!(sim.hash(), next);
}

#[test]
fn edit_in_past_discards_future_and_records_edit() {
    let (mut sim, mut session) = started(recorded(400, None));
    play(&mut session, &mut sim, 300);
    session.seek(&mut sim, 150).unwrap();
    let edit = stats_edit(&sim, 1_000);
    assert_eq!(session.edit(&mut sim, edit.clone()).unwrap(), None);
    let timeline = session.timeline();
    assert_eq!(
        (timeline.head, timeline.simulated, timeline.current),
        (150, 150, 150)
    );
    assert_eq!(timeline.edit_ticks, vec![150]);
    assert!(timeline.checkpoints.iter().all(|&tick| tick <= 150));
    let export = session.export().unwrap();
    let edits = EditLog::decode(&export.edits).unwrap();
    assert_eq!(edits.edits, vec![TimedEdit { tick: 150, edit }]);
    assert_eq!(InputLog::decode(&export.inputs).unwrap().ticks.len(), 151);
    assert_eq!(export.hashes.len(), 151);
}

#[test]
fn pending_edit_shows_in_world_before_next_tick() {
    let (mut sim, mut session) = started(recorded(400, None));
    play(&mut session, &mut sim, 100);
    let edit = stats_edit(&sim, 1_000);
    session.edit(&mut sim, edit).unwrap();
    assert!(session.is_pending());
    assert_ne!(sim.hash(), hashes(&session)[100]);
    session.step(&mut sim, None).unwrap();
    assert!(!session.is_pending());
}

#[test]
fn idle_replay_keeps_future() {
    let log = recorded(400, Some(120));
    let (mut sim, mut session) = started(log.clone());
    play(&mut session, &mut sim, 300);
    let before = hashes(&session);
    session.seek(&mut sim, 100).unwrap();
    for tick in 100..150 {
        let same = log.tick(tick).unwrap();
        let live = (tick % 2 == 0).then_some(same);
        session.step(&mut sim, live).unwrap();
    }
    let timeline = session.timeline();
    assert_eq!(
        (timeline.head, timeline.simulated, timeline.current),
        (399, 300, 150)
    );
    assert_eq!(hashes(&session), before);
}

#[test]
fn live_input_differing_from_recorded_takes_over() {
    let (mut sim, mut session) = started(recorded(400, None));
    play(&mut session, &mut sim, 300);
    session.seek(&mut sim, 100).unwrap();
    let live = spawn_input(-2.0);
    session.step(&mut sim, Some(&live)).unwrap();
    let timeline = session.timeline();
    assert_eq!(
        (timeline.head, timeline.simulated, timeline.current),
        (100, 101, 101)
    );
    let export = session.export().unwrap();
    let inputs = InputLog::decode(&export.inputs).unwrap();
    assert_eq!(inputs.ticks.len(), 101);
    assert_eq!(inputs.tick(100), Some(live.as_slice()));
    assert_eq!(export.hashes.last(), Some(&sim.hash()));
}

#[test]
fn seek_beyond_head_is_invalid_tick() {
    let (mut sim, mut session) = started(recorded(400, None));
    assert!(matches!(
        session.seek(&mut sim, 400),
        Err(SessionError::InvalidTick {
            tick: 400,
            head: 399
        })
    ));
    session.seek(&mut sim, 399).unwrap();
    assert_eq!(session.timeline().simulated, 399);
}

#[test]
fn tick_zero_despawn_reloads_scene_with_matching_indices() {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    let victim = sim.world().entities().nth(5).unwrap();
    session
        .edit(
            &mut sim,
            Edit::Despawn {
                entity: EntityRef::Runtime(victim),
            },
        )
        .unwrap();
    let mut fresh = simulation();
    fresh.load_scene(session.scene()).unwrap();
    assert_eq!(sim.hash(), fresh.hash());
    assert_eq!(hashes(&session), vec![fresh.hash()]);

    let value = sim
        .encode_json_value(ValueKind::Component, "boxes::Crate", json!(null))
        .unwrap();
    let spawn = Edit::Spawn {
        components: vec![("boxes::Crate".to_string(), value)],
    };
    let spawned = session.edit(&mut sim, spawn).unwrap().unwrap();
    assert_eq!(spawned, last_entity(&sim));
    let components = sim.world().entity_components_ron(spawned).unwrap();
    assert_eq!(
        components.keys().collect::<Vec<_>>(),
        ["boxes::Crate"],
        "the returned entity holds the spawned components"
    );
    fresh.load_scene(session.scene()).unwrap();
    assert_eq!(sim.hash(), fresh.hash());
}

#[test]
fn tick_zero_edit_resimulates_and_reports_first_divergence() {
    let (mut sim, mut session) = started(recorded(200, Some(50)));
    play(&mut session, &mut sim, 200);
    let old = hashes(&session);
    session.seek(&mut sim, 0).unwrap();
    let edit = stats_edit(&sim, 1_000);
    session.edit(&mut sim, edit).unwrap();
    assert!(session.timeline().resimulating);
    let reports = resimulate_all(&mut session, &mut sim);
    let [
        Resimulation::Diverged {
            tick: 1,
            expected,
            got,
        },
        Resimulation::Done,
    ] = reports[..]
    else {
        panic!("unexpected reports {reports:?}");
    };
    assert_eq!(expected, old[1]);
    assert_ne!(got, old[1]);
    let timeline = session.timeline();
    assert!(!timeline.resimulating);
    assert_eq!((timeline.current, timeline.simulated), (0, 200));
    let new = hashes(&session);
    assert_eq!(sim.hash(), new[0]);
    assert!(matches!(
        session.resimulate(&mut sim, 64).unwrap(),
        Resimulation::Idle
    ));
    session.seek(&mut sim, 200).unwrap();
    assert_eq!(sim.hash(), new[200]);
}

#[test]
fn edits_are_refused_while_resimulating() {
    let (mut sim, mut session) = started(recorded(200, None));
    play(&mut session, &mut sim, 200);
    session.seek(&mut sim, 0).unwrap();
    let edit = stats_edit(&sim, 1_000);
    session.edit(&mut sim, edit).unwrap();
    let refused = |result: Result<(), SessionError>, up_to: u64| {
        assert!(
            matches!(result, Err(SessionError::Resimulating { up_to: at }) if at == up_to),
            "{result:?}"
        );
    };
    let edit = stats_edit(&sim, 1);
    refused(session.edit(&mut sim, edit).map(drop), 0);
    refused(session.seek(&mut sim, 100), 0);
    refused(session.export().map(drop), 0);
    assert!(matches!(
        session.resimulate(&mut sim, 64).unwrap(),
        Resimulation::Diverged { tick: 1, .. }
    ));
    assert_eq!(session.timeline().current, 0);
    session.seek(&mut sim, 50).unwrap();
    refused(session.seek(&mut sim, 100), 64);
    resimulate_all(&mut session, &mut sim);
    let edit = stats_edit(&sim, 1);
    session.edit(&mut sim, edit).unwrap();
}

#[test]
fn failed_recorded_edit_is_dropped_during_resimulation() {
    let (mut sim, mut session) = started(recorded(200, None));
    play(&mut session, &mut sim, 100);
    let target = last_entity(&sim);
    let remove = Edit::Remove {
        entity: EntityRef::Runtime(target),
        component: "boxes::Crate".to_string(),
    };
    session.edit(&mut sim, remove).unwrap();
    session.seek(&mut sim, 0).unwrap();
    session
        .edit(
            &mut sim,
            Edit::Despawn {
                entity: EntityRef::Runtime(target),
            },
        )
        .unwrap();
    let reports = resimulate_all(&mut session, &mut sim);
    assert!(
        reports
            .iter()
            .any(|report| matches!(report, Resimulation::EditDropped { tick: 100, .. })),
        "{reports:?}"
    );
    let export = session.export().unwrap();
    assert!(EditLog::decode(&export.edits).unwrap().edits.is_empty());
    assert_eq!(session.timeline().edit_ticks, Vec::<u64>::new());
}

#[test]
fn export_then_fresh_replay_matches() {
    let (mut sim, mut session) = started(recorded(300, Some(40)));
    play(&mut session, &mut sim, 120);
    let edit = stats_edit(&sim, 1_000);
    session.edit(&mut sim, edit).unwrap();
    let entity = last_entity(&sim);
    session
        .edit(
            &mut sim,
            Edit::Despawn {
                entity: EntityRef::Runtime(entity),
            },
        )
        .unwrap();
    for tick in 0..80 {
        let live = (tick % 20 == 0).then(|| spawn_input(-4.0));
        session.step(&mut sim, live.as_deref()).unwrap();
    }
    let export = session.export().unwrap();
    assert_eq!(export.hashes.len(), 201);

    assert_fresh_replay_matches(&export);
}

fn panic_at_50(time: Res<Time>) {
    if time.tick == 50 {
        panic!("boom at fifty");
    }
}

#[test]
fn panic_restores_state_and_keeps_logs() {
    let mut app = boxes::app();
    app.system(Sim, panic_at_50);
    let mut sim = app.build().unwrap();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    session.load_inputs(&mut sim, recorded(100, None)).unwrap();
    play(&mut session, &mut sim, 50);
    let before = session.export().unwrap();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = session.step(&mut sim, None);
    std::panic::set_hook(hook);

    let Err(SessionError::Panicked { tick: 50, message }) = result else {
        panic!("unexpected result {result:?}");
    };
    assert!(message.contains("boom at fifty"), "{message}");
    assert_eq!(session.timeline().current, 50);
    assert_eq!(sim.hash(), before.hashes[50]);
    assert_eq!(session.export().unwrap(), before);
}

#[test]
fn load_edits_drops_recorded_edits_that_fail() {
    let (mut sim, mut session) = started(recorded(200, None));
    let target = last_entity(&sim);
    let at = |tick, edit| TimedEdit { tick, edit };
    let good = stats_edit(&sim, 1_000);
    let mut log = EditLog::new();
    log.edits = vec![
        at(
            0,
            Edit::Despawn {
                entity: EntityRef::Runtime(target),
            },
        ),
        at(
            0,
            Edit::SetResource {
                resource: "ludenea::Time".to_string(),
                value: Vec::new(),
            },
        ),
        at(
            50,
            Edit::Remove {
                entity: EntityRef::Runtime(target),
                component: "boxes::Crate".to_string(),
            },
        ),
        at(80, good.clone()),
    ];
    session.load_edits(&mut sim, log).unwrap();
    assert!(session.timeline().resimulating);
    assert!(matches!(
        session.seek(&mut sim, 100),
        Err(SessionError::Resimulating { .. })
    ));
    let reports = resimulate_all(&mut session, &mut sim);
    let ticks: Vec<_> = reports
        .iter()
        .map(|report| match report {
            Resimulation::EditDropped { tick, .. } => Some(*tick),
            Resimulation::Done => None,
            other => panic!("unexpected report {other:?}"),
        })
        .collect();
    assert_eq!(ticks, [Some(0), Some(50), None]);
    assert_eq!(session.timeline().edit_ticks, vec![0, 80]);

    session.seek(&mut sim, 150).unwrap();
    assert_eq!(sim.hash(), hashes(&session)[150]);
    play(&mut session, &mut sim, 10);
    let export = session.export().unwrap();
    let edits = EditLog::decode(&export.edits).unwrap();
    assert_eq!(edits.edits.last(), Some(&at(80, good)));
    assert_fresh_replay_matches(&export);
}

#[test]
fn live_take_over_during_resimulation_ends_it() {
    let (mut sim, mut session) = started(recorded(200, None));
    play(&mut session, &mut sim, 120);
    session
        .replace_scene(&mut sim, boxes::generate_scene(31, 8), AssetTexts::new())
        .unwrap();
    loop {
        match session.resimulate(&mut sim, 30).unwrap() {
            Resimulation::Progress { up_to } if up_to >= 30 => break,
            Resimulation::Done | Resimulation::Idle => panic!("resimulation ended early"),
            _ => {}
        }
    }
    session.seek(&mut sim, 20).unwrap();
    session.step(&mut sim, Some(&spawn_input(1.0))).unwrap();
    let timeline = session.timeline();
    assert!(!timeline.resimulating, "{timeline:?}");
    assert_eq!((timeline.head, timeline.simulated), (20, 21));
    play(&mut session, &mut sim, 5);
    assert_eq!(
        session.resimulate(&mut sim, 30).unwrap(),
        Resimulation::Idle
    );
    assert_fresh_replay_matches(&session.export().unwrap());
}

#[test]
fn take_over_far_behind_the_head_keeps_a_checkpoint_there() {
    let (mut sim, mut session) = started(recorded(2_000, None));
    play(&mut session, &mut sim, 2_000);
    session.seek(&mut sim, 1_000).unwrap();
    session.step(&mut sim, Some(&spawn_input(1.0))).unwrap();
    assert!(session.timeline().checkpoints.contains(&1_001));
    play(&mut session, &mut sim, 40);
    session.seek(&mut sim, 1_010).unwrap();
    assert_eq!(sim.hash(), hashes(&session)[1_010]);
    assert!(session.timeline().checkpoints.contains(&1_001));
}

/// Silences the panic message of `run` and returns its result.
fn quietly<T>(run: impl FnOnce() -> T) -> T {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = run();
    std::panic::set_hook(hook);
    result
}

static SEEK_TRAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn seek_trap(time: Res<Time>) {
    if time.tick == 50 && SEEK_TRAP.load(std::sync::atomic::Ordering::Relaxed) {
        panic!("seek trap");
    }
}

#[test]
fn panic_during_seek_keeps_the_current_tick() {
    let mut app = boxes::app();
    app.system(Sim, seek_trap);
    let mut sim = app.build().unwrap();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    session.load_inputs(&mut sim, recorded(100, None)).unwrap();
    play(&mut session, &mut sim, 80);
    session.seek(&mut sim, 10).unwrap();
    let before = session.export().unwrap();

    SEEK_TRAP.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = quietly(|| session.seek(&mut sim, 55));
    SEEK_TRAP.store(false, std::sync::atomic::Ordering::Relaxed);

    assert!(matches!(
        result,
        Err(SessionError::Panicked { tick: 50, .. })
    ));
    assert_eq!(session.timeline().current, 10);
    assert_eq!(sim.hash(), before.hashes[10]);
    assert_eq!(session.export().unwrap(), before);
    session.seek(&mut sim, 55).unwrap();
    assert_eq!(sim.hash(), before.hashes[55]);
}

static RESIMULATION_TRAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn resimulation_trap(time: Res<Time>) {
    if time.tick == 50 && RESIMULATION_TRAP.load(std::sync::atomic::Ordering::Relaxed) {
        panic!("resimulation trap");
    }
}

#[test]
fn panic_during_resimulation_ends_it_at_the_failing_tick() {
    let mut app = boxes::app();
    app.system(Sim, resimulation_trap);
    let mut sim = app.build().unwrap();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    let inputs = recorded(100, None);
    session.load_inputs(&mut sim, inputs.clone()).unwrap();
    play(&mut session, &mut sim, 80);
    session
        .replace_scene(&mut sim, boxes::generate_scene(31, 8), AssetTexts::new())
        .unwrap();

    RESIMULATION_TRAP.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = quietly(|| session.resimulate(&mut sim, 240));
    RESIMULATION_TRAP.store(false, std::sync::atomic::Ordering::Relaxed);

    assert!(matches!(
        result,
        Err(SessionError::Panicked { tick: 50, .. })
    ));
    let timeline = session.timeline();
    assert!(!timeline.resimulating, "{timeline:?}");
    assert_eq!((timeline.current, timeline.simulated), (50, 50));
    assert_eq!(sim.tick(), 50);
    let export = session.export().unwrap();
    assert_eq!(InputLog::decode(&export.inputs).unwrap(), inputs);
    assert_fresh_replay_matches(&export);
}

static RESTORE_TRAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn restore_trap(time: Res<Time>) {
    if time.tick == 50 && RESTORE_TRAP.load(std::sync::atomic::Ordering::Relaxed) {
        panic!("restore trap");
    }
}

#[test]
fn panic_while_restoring_past_the_simulated_ticks_moves_to_the_failing_tick() {
    let mut app = boxes::app();
    app.system(Sim, restore_trap);
    let mut sim = app.build().unwrap();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    session.load_inputs(&mut sim, recorded(100, None)).unwrap();
    play(&mut session, &mut sim, 60);
    let snapshot = sim.snapshot();
    session.load_inputs(&mut sim, recorded(100, None)).unwrap();
    play(&mut session, &mut sim, 10);

    RESTORE_TRAP.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = quietly(|| session.restore(&mut sim, &snapshot));
    RESTORE_TRAP.store(false, std::sync::atomic::Ordering::Relaxed);

    assert!(matches!(
        result,
        Err(SessionError::Panicked { tick: 50, .. })
    ));
    assert_eq!(session.timeline().current, 50);
    assert_eq!(sim.tick(), 50);
    assert_eq!(sim.hash(), hashes(&session)[50]);
}

#[test]
fn hashes_of_a_tick_with_an_edit_are_taken_before_it() {
    let (mut sim, mut session) = started(recorded(20, None));
    let mut log = EditLog::new();
    log.edits = vec![TimedEdit {
        tick: 0,
        edit: stats_edit(&sim, 1_000),
    }];
    session.load_edits(&mut sim, log).unwrap();
    assert!(!session.timeline().resimulating);
    assert!(session.is_pending());
    assert_ne!(sim.hash(), hashes(&session)[0]);
}
