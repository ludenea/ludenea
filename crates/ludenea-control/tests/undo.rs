use ludenea::control::{Bytes, Document, Resimulation, Session, SessionError, SessionExport};
use ludenea::prelude::*;
use ludenea_app::{AssetTexts, EntityRef, ValueKind};
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
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
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
    while !matches!(
        session.resimulate(sim, 256).unwrap(),
        Resimulation::Done | Resimulation::Idle
    ) {}
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
    // The edit forked a branch, which the undo deletes: the parent kept its future.
    assert_eq!((timeline.current, timeline.head), (300, 499));
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
    let mut session = Session::start(
        &mut sim,
        original.clone(),
        AssetTexts::new(),
        Document::Scene,
    )
    .unwrap();
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
fn a_fork_keeps_the_undo_entries_of_the_future_it_forks_from() {
    let (mut sim, mut session, _, _) = edited_at_300();
    session.seek(&mut sim, 100).unwrap();
    let edit = stats_edit(&sim, 7);
    session.edit(&mut sim, edit).unwrap();
    session.undo(&mut sim).unwrap();
    assert_eq!(session.timeline().current, 100);
    assert_eq!(session.current_branch(), BranchId(1));
    session.undo(&mut sim).unwrap();
    assert_eq!(session.timeline().current, 300);
    assert_eq!(session.current_branch(), ROOT);
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
    let mut session = Session::resume(&mut sim, export, Document::Scene).unwrap();
    assert!(matches!(
        session.undo(&mut sim),
        Err(SessionError::Resimulating { .. })
    ));
    resimulate_all(&mut session, &mut sim);
    assert!(session.timeline().can_undo);
    session.undo(&mut sim).unwrap();
    // The undo deletes the branch the edit forked; its parent is stale after the resume.
    resimulate_all(&mut session, &mut sim);
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
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
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
    // The edit at the head extends the branch: it has a plain entry, not a fork.
    let (mut sim, mut session) = started(100);
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
    session
        .replace_scene(&mut sim, disk.clone(), AssetTexts::new())
        .unwrap();
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
        SessionExport {
            undo: Vec::new(),
            branches: Vec::new(),
            ..export
        },
        Document::Scene,
    )
    .unwrap();
    assert!(!session.timeline().can_undo && !session.timeline().can_redo);
}

#[test]
fn unsaved_scene_edits_stay_unsaved_across_export_and_resume() {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
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
        Session::resume(sim, export, Document::Scene).unwrap()
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
fn take_over_forks_and_keeps_the_undo_entries_of_the_future_it_forks_from() {
    let (mut sim, mut session, _, _) = edited_at_300();
    session.seek(&mut sim, 200).unwrap();
    session.step(&mut sim, Some(&live_spawn())).unwrap();
    assert!(edits(&session).is_empty());
    session.undo(&mut sim).unwrap();
    assert_eq!(session.current_branch(), BranchId(1));
    assert_eq!(session.timeline().current, 200);
    assert_eq!(edits(&session).len(), 1);
    session.undo(&mut sim).unwrap();
    assert!(!session.timeline().can_undo);
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
        .replace_scene(&mut sim, boxes::generate_scene(31, 8), AssetTexts::new())
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
        ..
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
        assets: Vec::new(),
        branches: Bytes(Vec::new()),
        document: Document::Scene,
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

use ludenea::control::BranchId;
use serde::Serialize;

const ROOT: BranchId = BranchId::ROOT;

/// Seeks to `tick` and edits there.
fn edit_at(session: &mut Session, sim: &mut Simulation, tick: u64, begins: u64) {
    session.seek(sim, tick).unwrap();
    let edit = stats_edit(sim, begins);
    session.edit(sim, edit).unwrap();
}

fn branch_ids(session: &Session) -> Vec<BranchId> {
    session.branches().iter().map(|branch| branch.id).collect()
}

fn resume(sim: &mut Simulation, export: SessionExport) -> Session {
    Session::resume(sim, export, Document::Scene).unwrap()
}

/// Switches to `id` and returns its hashes up to its head.
fn branch_hashes(session: &mut Session, sim: &mut Simulation, id: BranchId) -> Vec<u64> {
    session.switch_branch(sim, id).unwrap();
    resimulate_all(session, sim);
    let timeline = session.timeline();
    session
        .seek(sim, timeline.head.max(timeline.simulated))
        .unwrap();
    hashes(session)
}

/// The undo entry milestone 2b wrote before branches had their own entries.
#[derive(Serialize)]
#[allow(dead_code)]
enum UndoEntryV2 {
    Scene { before: String, after: String },
    Run { edit: TimedEdit },
}

fn undo_history_v2(undo: Vec<UndoEntryV2>) -> Vec<u8> {
    postcard::to_allocvec(&(2u32, undo, Vec::<UndoEntryV2>::new())).unwrap()
}

#[test]
fn undo_fork_returns_to_parent_with_future() {
    let (mut sim, mut session) = started(400);
    play(&mut session, &mut sim, 300);
    let root = hashes(&session);
    edit_at(&mut session, &mut sim, 150, 1_000);
    play(&mut session, &mut sim, 20);
    assert_eq!(session.current_branch(), BranchId(1));

    session.undo(&mut sim).unwrap();
    assert_eq!(branch_ids(&session), vec![ROOT]);
    assert_eq!(session.current_branch(), ROOT);
    let timeline = session.timeline();
    assert_eq!((timeline.current, timeline.head), (150, 399));
    assert_eq!(hashes(&session), root);
    assert_eq!(sim.hash(), root[150]);
}

#[test]
fn redo_fork_recreates_same_branch() {
    let (mut sim, mut session) = started(400);
    play(&mut session, &mut sim, 300);
    edit_at(&mut session, &mut sim, 150, 1_000);
    play(&mut session, &mut sim, 20);
    let forked = hashes(&session);
    let forked_edits = edits(&session);
    session
        .rename_branch(BranchId(1), "Try".to_string())
        .unwrap();

    session.undo(&mut sim).unwrap();
    session.seek(&mut sim, 30).unwrap();
    session.redo(&mut sim).unwrap();
    let info = session.branches()[1].clone();
    assert_eq!(
        (
            info.id,
            info.name.as_str(),
            info.parent,
            info.fork_tick,
            info.head
        ),
        (BranchId(1), "Try", Some(ROOT), 150, 169)
    );
    assert_eq!(session.current_branch(), BranchId(1));
    assert_eq!(session.timeline().current, 150);
    assert!(session.is_pending());
    assert_eq!(edits(&session), forked_edits);
    session.seek(&mut sim, 170).unwrap();
    assert_eq!(hashes(&session), forked);
    assert!(!session.timeline().can_redo);
}

#[test]
fn undo_switches_to_entry_branch_first() {
    let (mut sim, mut session) = started(400);
    play(&mut session, &mut sim, 300);
    let root = hashes(&session);
    edit_at(&mut session, &mut sim, 150, 1_000);
    play(&mut session, &mut sim, 20);
    // At the branch's head, an edit extends it.
    let edit = stats_edit(&sim, 2_000);
    session.edit(&mut sim, edit.clone()).unwrap();
    assert_eq!(branch_ids(&session), vec![ROOT, BranchId(1)]);
    session.switch_branch(&mut sim, ROOT).unwrap();

    session.undo(&mut sim).unwrap();
    assert_eq!(session.current_branch(), BranchId(1));
    assert_eq!(session.timeline().current, 170);
    assert_eq!(edits(&session).len(), 1);
    assert_eq!(edits(&session)[0].tick, 150);
    session.redo(&mut sim).unwrap();
    assert_eq!(edits(&session)[1], TimedEdit { tick: 170, edit });

    session.switch_branch(&mut sim, ROOT).unwrap();
    assert_eq!(hashes(&session), root);
}

/// A root with an edit at tick 300 and three branches: forked at 200 (before the edit),
/// at 300 (inheriting it), and at 350. The history holds only the edit at 300, as a
/// history written before forks had their own entries.
fn descendants_of_an_edit() -> (Simulation, Session, Vec<Vec<u64>>) {
    let (mut sim, mut session) = started(300);
    play(&mut session, &mut sim, 300);
    edit_at(&mut session, &mut sim, 300, 1_000);
    play(&mut session, &mut sim, 100);
    assert_eq!(branch_ids(&session), vec![ROOT]);
    let edit = edits(&session)[0].clone();
    for (tick, begins) in [(200, 1), (300, 2), (350, 3)] {
        session.switch_branch(&mut sim, ROOT).unwrap();
        edit_at(&mut session, &mut sim, tick, begins);
        play(&mut session, &mut sim, 10);
    }
    let fork_ticks: Vec<u64> = session.branches().iter().map(|b| b.fork_tick).collect();
    assert_eq!(fork_ticks, vec![0, 200, 300, 350]);
    session.switch_branch(&mut sim, ROOT).unwrap();
    let mut export = session.export().unwrap();
    export.undo = undo_history_v2(vec![UndoEntryV2::Run { edit }]);
    let mut session = resume(&mut sim, export);
    resimulate_all(&mut session, &mut sim);
    let expected = (0..4)
        .map(|id| branch_hashes(&mut session, &mut sim, BranchId(id)))
        .collect();
    session.switch_branch(&mut sim, ROOT).unwrap();
    (sim, session, expected)
}

#[test]
fn undo_run_removes_descendants_forked_at_or_after_and_redo_restores_them() {
    let (mut sim, mut session, expected) = descendants_of_an_edit();
    session.undo(&mut sim).unwrap();
    assert_eq!(branch_ids(&session), vec![ROOT, BranchId(1)]);
    assert_eq!(session.current_branch(), ROOT);
    let timeline = session.timeline();
    assert_eq!((timeline.current, timeline.head), (300, 300));
    assert!(edits(&session).is_empty());

    session.redo(&mut sim).unwrap();
    assert_eq!(
        branch_ids(&session),
        vec![ROOT, BranchId(1), BranchId(2), BranchId(3)]
    );
    assert_eq!(session.current_branch(), ROOT);
    assert_eq!(session.timeline().current, 300);
    let names: Vec<String> = session.branches().into_iter().map(|b| b.name).collect();
    assert_eq!(names, ["Branch 1", "Branch 2", "Branch 3", "Branch 4"]);
    for id in [2, 3, 0] {
        let got = branch_hashes(&mut session, &mut sim, BranchId(id));
        assert_eq!(got, expected[id as usize], "branch {id}");
    }
}

#[test]
fn undo_run_keeps_descendants_forked_before_its_tick() {
    let (mut sim, mut session, expected) = descendants_of_an_edit();
    session.undo(&mut sim).unwrap();
    let kept = session.branches()[1].clone();
    assert_eq!(
        (kept.id, kept.fork_tick, kept.head),
        (BranchId(1), 200, 209)
    );
    assert_eq!(
        branch_hashes(&mut session, &mut sim, BranchId(1)),
        expected[1]
    );

    // Natively, an edit at a branch's head extends it: no branch depends on it.
    let (mut sim, mut session) = started(400);
    play(&mut session, &mut sim, 300);
    edit_at(&mut session, &mut sim, 100, 1);
    session.switch_branch(&mut sim, ROOT).unwrap();
    edit_at(&mut session, &mut sim, 399, 2);
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo edit at tick 399")
    );
    session.undo(&mut sim).unwrap();
    assert_eq!(branch_ids(&session), vec![ROOT, BranchId(1)]);
    assert!(edits(&session).is_empty());
}

#[test]
fn undo_label_names_removed_branches() {
    let (_, session, _) = descendants_of_an_edit();
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo edit at tick 300 (removes Branch 3, Branch 4)")
    );

    let (mut sim, mut session) = started(400);
    let timeline = session.timeline();
    assert_eq!((timeline.undo_label, timeline.redo_label), (None, None));
    play(&mut session, &mut sim, 300);
    edit_at(&mut session, &mut sim, 150, 1);
    play(&mut session, &mut sim, 20);
    edit_at(&mut session, &mut sim, 160, 2);
    session.switch_branch(&mut sim, ROOT).unwrap();
    session
        .rename_branch(BranchId(2), "Nested".to_string())
        .unwrap();
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo edit at tick 160 (removes Nested)")
    );
    session.undo(&mut sim).unwrap();
    let timeline = session.timeline();
    assert_eq!(
        timeline.undo_label.as_deref(),
        Some("Undo edit at tick 150 (removes Branch 2)")
    );
    assert_eq!(
        timeline.redo_label.as_deref(),
        Some("Redo edit at tick 160 (restores Nested)")
    );

    session.seek(&mut sim, 40).unwrap();
    session.step(&mut sim, Some(&live_spawn())).unwrap();
    let timeline = session.timeline();
    assert_eq!(
        timeline.undo_label.as_deref(),
        Some("Undo take-over at tick 40 (removes Branch 4)")
    );
    assert_eq!(timeline.redo_label, None);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.current_branch(), BranchId(1));
    assert_eq!(session.timeline().current, 40);
}

#[test]
fn undo_survives_export_and_resume_with_branches() {
    let (mut sim, mut session) = started(400);
    play(&mut session, &mut sim, 300);
    edit_at(&mut session, &mut sim, 150, 1);
    play(&mut session, &mut sim, 20);
    edit_at(&mut session, &mut sim, 160, 2);
    play(&mut session, &mut sim, 10);
    let nested = hashes(&session);
    let export = session.export().unwrap();
    drop(session);

    let mut session = resume(&mut sim, export);
    resimulate_all(&mut session, &mut sim);
    session.undo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);
    assert_eq!(branch_ids(&session), vec![ROOT, BranchId(1)]);
    assert_eq!(
        (session.current_branch(), session.timeline().current),
        (BranchId(1), 160)
    );
    session.undo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);
    assert_eq!(branch_ids(&session), vec![ROOT]);
    assert_eq!(session.timeline().current, 150);

    // The redo stack survives an export too.
    let export = session.export().unwrap();
    let mut session = resume(&mut sim, export);
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);
    assert_eq!(session.current_branch(), BranchId(2));
    assert_eq!(branch_hashes(&mut session, &mut sim, BranchId(2)), nested);
}

#[test]
fn undo_history_version_2_decodes() {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 100);
    let before = sim.hash();
    let edit = stats_edit(&sim, 1_000);
    session.edit(&mut sim, edit.clone()).unwrap();
    let mut export = session.export().unwrap();
    export.undo = undo_history_v2(vec![UndoEntryV2::Run {
        edit: TimedEdit { tick: 100, edit },
    }]);
    let mut session = resume(&mut sim, export.clone());
    resimulate_all(&mut session, &mut sim);
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo edit at tick 100")
    );
    session.undo(&mut sim).unwrap();
    assert!(edits(&session).is_empty());
    assert_eq!(sim.hash(), before);
    assert_eq!(session.current_branch(), ROOT);

    // Redone after a rebuild, the branch resimulates on the new build.
    let mut session = resume(&mut sim, session.export().unwrap());
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    assert!(session.timeline().resimulating);
    resimulate_all(&mut session, &mut sim);
    assert_eq!(session.timeline().current, 100);
    assert_eq!(edits(&session).len(), 1);
    assert!(session.is_pending());

    // A history of a later version is refused by name, not misread as 2a bytes.
    export.undo = vec![4];
    assert!(matches!(
        Session::resume(&mut sim, export, Document::Scene),
        Err(SessionError::UndoVersion { version: 4 })
    ));
}

#[test]
fn an_entry_whose_branch_was_deleted_never_blocks_the_history() {
    let (mut sim, mut session) = started(400);
    play(&mut session, &mut sim, 300);
    edit_at(&mut session, &mut sim, 100, 1);
    session.switch_branch(&mut sim, ROOT).unwrap();
    edit_at(&mut session, &mut sim, 200, 2);
    session.undo(&mut sim).unwrap();
    assert!(session.timeline().can_redo);
    // Deleting a branch drops its entries and the redo stack: the undo of the edit
    // that forked it goes, the one of the other fork stays.
    session.delete_branch(&mut sim, BranchId(1)).unwrap();
    let timeline = session.timeline();
    assert!(!timeline.can_redo);
    assert_eq!(timeline.undo_label, None);
    assert!(matches!(
        session.undo(&mut sim),
        Err(SessionError::NothingToUndo)
    ));

    let (mut sim, mut session) = started(400);
    play(&mut session, &mut sim, 300);
    edit_at(&mut session, &mut sim, 100, 1);
    session.switch_branch(&mut sim, ROOT).unwrap();
    edit_at(&mut session, &mut sim, 200, 2);
    session.delete_branch(&mut sim, BranchId(1)).unwrap();
    session.undo(&mut sim).unwrap();
    assert_eq!(branch_ids(&session), vec![ROOT]);
}
