use ludenea::control::{Resimulation, Session, SessionError};
use ludenea::prelude::*;
use ludenea_app::{EntityRef, ValueKind};
use serde_json::json;

fn simulation() -> Simulation {
    boxes::app().build().unwrap()
}

fn small_scene() -> String {
    boxes::generate_scene(30, 7)
}

/// `ticks` recorded inputs with a nudge every 37 ticks.
fn recorded(ticks: u64) -> InputLog {
    let mut log = InputLog::new(1);
    for tick in 0..ticks {
        log.record(&[boxes::BoxesInput {
            velocity_change: (tick % 37 == 0).then(|| Vec2::new(1.0, 4.0)),
            ..boxes::BoxesInput::default()
        }]);
    }
    log
}

fn started(ticks: u64) -> (Simulation, Session) {
    let mut sim = simulation();
    let mut session = Session::start(&mut sim, small_scene()).unwrap();
    session.load_inputs(&mut sim, recorded(ticks)).unwrap();
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

fn edits(session: &Session) -> Vec<TimedEdit> {
    EditLog::decode(&session.export().unwrap().edits)
        .unwrap()
        .edits
}

fn resimulate_all(session: &mut Session, sim: &mut Simulation) {
    while session.resimulate(sim, 64).unwrap() != Resimulation::Done {}
}

/// A session with an edit at tick 300, played on to tick 400.
fn edited_at_300() -> (Simulation, Session, Vec<u64>, Edit) {
    let (mut sim, mut session) = started(500);
    play(&mut session, &mut sim, 300);
    let before = hashes(&session);
    let edit = stats_edit(&sim, 1_000);
    session.edit(&mut sim, edit.clone()).unwrap();
    play(&mut session, &mut sim, 100);
    (sim, session, before, edit)
}

#[test]
fn undo_run_edit_restores_exact_hash() {
    let (mut sim, mut session, before, _) = edited_at_300();
    assert!(session.timeline().can_undo);
    session.undo(&mut sim).unwrap();
    let timeline = session.timeline();
    assert_eq!((timeline.current, timeline.head), (300, 300));
    assert_eq!(sim.hash(), before[300]);
    assert!(!session.is_pending());
    assert!(edits(&session).is_empty());
    assert!(!timeline.can_undo);
    assert!(timeline.can_redo);
}

#[test]
fn redo_reapplies_same_edit_and_hash() {
    let (mut sim, mut session, _, edit) = edited_at_300();
    session.seek(&mut sim, 300).unwrap();
    let edited = sim.hash();
    session.undo(&mut sim).unwrap();
    session.seek(&mut sim, 150).unwrap();
    session.redo(&mut sim).unwrap();
    assert_eq!(session.timeline().current, 300);
    assert_eq!(sim.hash(), edited);
    assert!(session.is_pending());
    assert_eq!(edits(&session), vec![TimedEdit { tick: 300, edit }]);
    let timeline = session.timeline();
    assert!(timeline.can_undo);
    assert!(!timeline.can_redo);
    assert!(matches!(
        session.redo(&mut sim),
        Err(SessionError::NothingToRedo)
    ));
}

#[test]
fn undo_scene_edit_restores_scene_text() {
    let mut sim = simulation();
    let original = small_scene();
    let mut session = Session::start(&mut sim, original.clone()).unwrap();
    let original_hash = sim.hash();
    let victim = sim.world().entities().nth(5).unwrap();
    session
        .edit(
            &mut sim,
            Edit::Despawn {
                entity: EntityRef::Runtime(victim),
            },
        )
        .unwrap();
    let edited = session.scene().to_string();
    let edited_hash = sim.hash();
    assert_ne!(edited, original);

    session.undo(&mut sim).unwrap();
    assert_eq!(session.scene(), original);
    assert_eq!(sim.hash(), original_hash);
    session.redo(&mut sim).unwrap();
    assert_eq!(session.scene(), edited);
    assert_eq!(sim.hash(), edited_hash);
}

#[test]
fn new_edit_clears_redo() {
    let (mut sim, mut session, _, _) = edited_at_300();
    session.undo(&mut sim).unwrap();
    session.seek(&mut sim, 200).unwrap();
    let edit = stats_edit(&sim, 7);
    session.edit(&mut sim, edit).unwrap();
    assert!(!session.timeline().can_redo);
    assert!(matches!(
        session.redo(&mut sim),
        Err(SessionError::NothingToRedo)
    ));
}

#[test]
fn discarded_future_removes_its_undo_entries() {
    let (mut sim, mut session, _, _) = edited_at_300();
    session.seek(&mut sim, 100).unwrap();
    let edit = stats_edit(&sim, 7);
    session.edit(&mut sim, edit).unwrap();
    session.undo(&mut sim).unwrap();
    assert_eq!(session.timeline().current, 100);
    assert!(matches!(
        session.undo(&mut sim),
        Err(SessionError::NothingToUndo)
    ));
}

#[test]
fn undo_survives_export_and_resume() {
    let (mut sim, session, before, _) = edited_at_300();
    let export = session.export().unwrap();
    drop(session);
    let mut session = Session::resume(
        &mut sim,
        export.scene,
        export.saved_scene,
        InputLog::decode(&export.inputs).unwrap(),
        EditLog::decode(&export.edits).unwrap(),
        &export.undo,
        export.hashes,
    )
    .unwrap();
    assert!(matches!(
        session.undo(&mut sim),
        Err(SessionError::Resimulating { .. })
    ));
    resimulate_all(&mut session, &mut sim);
    assert!(session.timeline().can_undo);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.timeline().current, 300);
    assert_eq!(sim.hash(), before[300]);
    assert!(edits(&session).is_empty());
}

#[test]
fn undo_on_empty_stack_is_nothing_to_undo() {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 50);
    let timeline = session.timeline();
    assert!(!timeline.can_undo && !timeline.can_redo);
    assert!(matches!(
        session.undo(&mut sim),
        Err(SessionError::NothingToUndo)
    ));
    assert!(matches!(
        session.redo(&mut sim),
        Err(SessionError::NothingToRedo)
    ));
}

#[test]
fn scene_dirty_until_saved() {
    let mut sim = simulation();
    let mut session = Session::start(&mut sim, small_scene()).unwrap();
    assert!(!session.timeline().scene_dirty);
    let victim = sim.world().entities().nth(5).unwrap();
    session
        .edit(
            &mut sim,
            Edit::Despawn {
                entity: EntityRef::Runtime(victim),
            },
        )
        .unwrap();
    assert!(session.timeline().scene_dirty);
    session.mark_scene_saved();
    assert!(!session.timeline().scene_dirty);
    session.undo(&mut sim).unwrap();
    assert!(
        session.timeline().scene_dirty,
        "differs from the saved text"
    );
    session.redo(&mut sim).unwrap();
    assert!(!session.timeline().scene_dirty, "back to the saved text");
}

#[test]
fn edit_dropped_by_resimulation_leaves_undo_history() {
    let (mut sim, mut session) = started(200);
    play(&mut session, &mut sim, 100);
    let target = sim.world().entities().last().unwrap();
    let remove = Edit::Remove {
        entity: EntityRef::Runtime(target),
        component: "boxes::Crate".to_string(),
    };
    session.edit(&mut sim, remove).unwrap();
    session.seek(&mut sim, 0).unwrap();
    let original = session.scene().to_string();
    session
        .edit(
            &mut sim,
            Edit::Despawn {
                entity: EntityRef::Runtime(target),
            },
        )
        .unwrap();
    resimulate_all(&mut session, &mut sim);
    assert!(edits(&session).is_empty());
    session.undo(&mut sim).unwrap();
    assert_eq!(session.scene(), original);
    resimulate_all(&mut session, &mut sim);
    assert!(matches!(
        session.undo(&mut sim),
        Err(SessionError::NothingToUndo)
    ));
}

#[test]
fn replace_scene_is_undoable() {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 50);
    let original = session.scene().to_string();
    let disk = boxes::generate_scene(31, 8);
    session.replace_scene(&mut sim, disk.clone()).unwrap();
    resimulate_all(&mut session, &mut sim);
    assert!(session.timeline().can_undo && !session.timeline().scene_dirty);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.scene(), original);
    assert!(session.timeline().scene_dirty);
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    assert_eq!(session.scene(), disk);
    assert!(!session.timeline().can_redo);
}

#[test]
fn resume_with_empty_undo_bytes_has_empty_history() {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 50);
    let export = session.export().unwrap();
    let session = Session::resume(
        &mut sim,
        export.scene,
        export.saved_scene,
        InputLog::decode(&export.inputs).unwrap(),
        EditLog::decode(&export.edits).unwrap(),
        &[],
        export.hashes,
    )
    .unwrap();
    assert!(!session.timeline().can_undo && !session.timeline().can_redo);
}

#[test]
fn unsaved_scene_edits_stay_unsaved_across_export_and_resume() {
    let mut sim = simulation();
    let mut session = Session::start(&mut sim, small_scene()).unwrap();
    let saved = session.scene().to_string();
    let victim = sim.world().entities().nth(5).unwrap();
    session
        .edit(
            &mut sim,
            Edit::Despawn {
                entity: EntityRef::Runtime(victim),
            },
        )
        .unwrap();
    let export = session.export().unwrap();
    assert_eq!(export.saved_scene.as_deref(), Some(saved.as_str()));
    let resume = |sim: &mut Simulation, export: ludenea::control::SessionExport| {
        Session::resume(
            sim,
            export.scene,
            export.saved_scene,
            InputLog::decode(&export.inputs).unwrap(),
            EditLog::decode(&export.edits).unwrap(),
            &export.undo,
            export.hashes,
        )
        .unwrap()
    };
    let mut resumed = resume(&mut sim, export);
    assert!(resumed.timeline().scene_dirty);
    resumed.undo(&mut sim).unwrap();
    assert!(!resumed.timeline().scene_dirty, "back to the saved text");

    resumed.mark_scene_saved();
    let export = resumed.export().unwrap();
    assert_eq!(export.saved_scene, None, "a saved scene travels once");
    assert!(!resume(&mut sim, export).timeline().scene_dirty);
}

fn live_spawn() -> Vec<u8> {
    let mut log = InputLog::new(1);
    log.record(&[boxes::BoxesInput {
        spawn: Some(2.0),
        ..boxes::BoxesInput::default()
    }]);
    log.ticks.remove(0)
}

#[test]
fn take_over_drops_the_undo_entries_of_its_discarded_future() {
    let (mut sim, mut session, _, _) = edited_at_300();
    session.seek(&mut sim, 200).unwrap();
    session.step(&mut sim, Some(&live_spawn())).unwrap();
    assert!(!session.timeline().can_undo);
    assert!(edits(&session).is_empty());
}

#[test]
fn take_over_drops_the_redo_entries_of_its_discarded_future() {
    let (mut sim, mut session, _, _) = edited_at_300();
    session.undo(&mut sim).unwrap();
    assert!(session.timeline().can_redo);
    session.seek(&mut sim, 100).unwrap();
    session.step(&mut sim, Some(&live_spawn())).unwrap();
    assert!(!session.timeline().can_redo);
}

#[test]
fn replace_scene_clears_redo() {
    let (mut sim, mut session, _, _) = edited_at_300();
    session.undo(&mut sim).unwrap();
    assert!(session.timeline().can_redo);
    session
        .replace_scene(&mut sim, boxes::generate_scene(31, 8))
        .unwrap();
    assert!(!session.timeline().can_redo);
}

#[test]
fn session_export_from_2a_with_undo_history_resumes() {
    use ludenea::control::{Command, Host, Reply};

    let fixture = include_str!("fixtures/session-2a.json");
    let Reply::Session {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        hashes,
    } = serde_json::from_str(fixture).unwrap()
    else {
        panic!("the fixture is a session export");
    };
    let log = EditLog::decode(&edits.0).unwrap();
    assert!(matches!(
        &log.edits[..],
        [TimedEdit {
            tick: 60,
            edit: Edit::Despawn {
                entity: EntityRef::Runtime(_)
            }
        }]
    ));

    let mut host = Host::new(boxes::app()).unwrap();
    host.handle(Command::LoadSession {
        scene,
        saved_scene,
        inputs,
        edits,
        undo,
        expected: hashes,
    })
    .unwrap();
    let timeline = |host: &mut Host| match host.handle(Command::GetTimeline).unwrap() {
        Reply::Timeline(timeline) => timeline,
        other => panic!("{other:?}"),
    };
    while timeline(&mut host).resimulating {
        host.advance(0.0);
    }
    assert!(timeline(&mut host).can_undo);
    // The run edit at tick 60, then the scene edit at tick 0.
    host.handle(Command::Undo).unwrap();
    let Reply::Session { edits, .. } = host.handle(Command::ExportSession).unwrap() else {
        panic!("a session export");
    };
    assert!(EditLog::decode(&edits.0).unwrap().edits.is_empty());
    host.handle(Command::Undo).unwrap();
    assert!(!timeline(&mut host).scene_dirty);
    assert!(!timeline(&mut host).can_undo);
}
