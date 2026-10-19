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

    let mut host = Host::new(boxes::app).unwrap();
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
        keeps_assets: false,
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
use ludenea::control::branches::BranchesError;
use ludenea::control::undo::UndoError;
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

/// Asserts that a fresh session of `session`'s scene and logs simulates every hash it
/// recorded, and that resuming its export reports no divergence.
fn assert_recorded_hashes_true(session: &Session) {
    let export = session.export().unwrap();
    let mut sim = simulation();
    let mut fresh = Session::start(
        &mut sim,
        export.scene.clone(),
        AssetTexts::new(),
        Document::Scene,
    )
    .unwrap();
    fresh
        .load_inputs(&mut sim, InputLog::decode(&export.inputs).unwrap())
        .unwrap();
    fresh
        .load_edits(&mut sim, EditLog::decode(&export.edits).unwrap())
        .unwrap();
    resimulate_all(&mut fresh, &mut sim);
    fresh
        .seek(&mut sim, export.hashes.len() as u64 - 1)
        .unwrap();
    assert_eq!(hashes(&fresh)[..export.hashes.len()], export.hashes);

    let mut resumed = resume(&mut sim, export);
    loop {
        match resumed.resimulate(&mut sim, 256).unwrap() {
            Resimulation::Done | Resimulation::Idle => break,
            report => assert!(
                !matches!(report, Resimulation::Diverged { .. }),
                "{report:?}"
            ),
        }
    }
}

#[test]
fn redo_after_a_scene_edit_on_a_fork_keeps_recorded_hashes_true() {
    let (mut sim, mut session) = started(200);
    play(&mut session, &mut sim, 40);
    // Branch 1 forks at tick 40 and plays on; the root keeps its future.
    edit_at(&mut session, &mut sim, 40, 7);
    play(&mut session, &mut sim, 20);
    // A tick 0 edit on the branch: every stored branch, the root included, is stale.
    edit_at(&mut session, &mut sim, 0, 99);
    resimulate_all(&mut session, &mut sim);
    // An edit at the branch's end extends it.
    edit_at(&mut session, &mut sim, 60, 3);
    session.undo(&mut sim).unwrap();

    session.redo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);

    assert_eq!(session.current_branch(), BranchId(1));
    assert_eq!(session.timeline().current, 60);
    assert_recorded_hashes_true(&session);
}

#[test]
fn a_redone_segment_of_a_previous_build_resimulates_from_its_fork() {
    let (mut sim, mut session) = started(200);
    play(&mut session, &mut sim, 40);
    edit_at(&mut session, &mut sim, 40, 7);
    play(&mut session, &mut sim, 20);
    edit_at(&mut session, &mut sim, 60, 3);
    session.undo(&mut sim).unwrap();
    // A resume stands for a rebuild: the segment the undo stored holds hashes of the
    // previous build.
    let mut session = resume(&mut sim, session.export().unwrap());
    resimulate_all(&mut session, &mut sim);

    session.redo(&mut sim).unwrap();

    // The run before the fork at tick 40 is resimulated already.
    let timeline = session.timeline();
    assert!(timeline.resimulating);
    assert_eq!(timeline.current, 40);
    resimulate_all(&mut session, &mut sim);
    assert_eq!(session.timeline().current, 60);
    assert_recorded_hashes_true(&session);
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

    // A history of a later version is dropped by name, not misread as 2a bytes, and the
    // session resumes without it.
    export.undo = vec![4];
    let mut session = resume(&mut sim, export);
    resimulate_all(&mut session, &mut sim);
    assert_eq!(
        session.warnings(),
        [format!(
            "{}; it was dropped, so undo and redo start empty",
            UndoError::Version { version: 4 }
        )]
    );
    assert!(!session.timeline().can_undo && !session.timeline().can_redo);
    assert_eq!(edits(&session).len(), 1);
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

#[test]
fn redo_label_names_the_branches_an_edit_undo_removed() {
    let (mut sim, mut session, _) = descendants_of_an_edit();
    session.undo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);
    let label = Some("Redo edit at tick 300 (restores Branch 3, Branch 4)");
    assert_eq!(session.timeline().redo_label.as_deref(), label);
    let session = resume(&mut sim, session.export().unwrap());
    assert_eq!(session.timeline().redo_label.as_deref(), label);
}

#[test]
fn new_logs_keep_only_the_undo_entries_of_edits_on_the_current_run() {
    let (mut sim, mut session) = started(0);
    for begins in [1, 2] {
        play(&mut session, &mut sim, 100);
        let edit = stats_edit(&sim, begins);
        session.edit(&mut sim, edit).unwrap();
    }
    play(&mut session, &mut sim, 50);
    // Branch 1 inherits the root's edit at tick 100, not the one at tick 200.
    edit_at(&mut session, &mut sim, 150, 3);
    assert_eq!(session.current_branch(), BranchId(1));

    session.load_inputs(&mut sim, recorded(10)).unwrap();
    resimulate_all(&mut session, &mut sim);
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo edit at tick 100")
    );
    // Its undo discards the future after tick 100, Branch 1's edit at 150 included.
    session.undo(&mut sim).unwrap();
    assert!(edits(&session).is_empty());
}

/// The undo entries of the current version, to write histories a session never records.
#[derive(Serialize)]
#[expect(
    dead_code,
    reason = "`Scene` keeps the variant indices of the encoded entries"
)]
enum UndoEntryV3 {
    Scene {
        before: String,
        after: String,
    },
    Run {
        branch: BranchId,
        edit: TimedEdit,
        removed: Vec<u8>,
    },
    Fork {
        branch: BranchId,
        cause: ForkCauseV3,
        name: String,
        removed: Vec<u8>,
    },
}

#[derive(Serialize)]
enum ForkCauseV3 {
    Edit(TimedEdit),
    TakeOver { tick: u64, input: Vec<u8> },
}

/// A branch as an undone entry stores it, with nothing recorded after its fork point.
#[derive(Serialize)]
struct StoredBranchV3 {
    id: BranchId,
    name: String,
    parent: Option<BranchId>,
    fork_tick: u64,
    fork_edits: u64,
    stale: bool,
    inputs: Vec<u8>,
    edits: Vec<u8>,
    hashes: Vec<u64>,
}

fn removed_branch(id: u32, parent: BranchId, fork_tick: u64) -> Vec<u8> {
    let branch = StoredBranchV3 {
        id: BranchId(id),
        name: format!("Branch {}", id + 1),
        parent: Some(parent),
        fork_tick,
        fork_edits: 0,
        stale: false,
        inputs: InputLog::new(1).encode(),
        edits: EditLog::new().encode(),
        hashes: Vec::new(),
    };
    postcard::to_allocvec(&vec![branch]).unwrap()
}

fn undo_history_v3(undo: Vec<UndoEntryV3>, redo: Vec<UndoEntryV3>) -> Vec<u8> {
    postcard::to_allocvec(&(3u32, undo, redo)).unwrap()
}

/// A linear session of 100 ticks resumed with the undo history `undo`.
fn resumed_with_undo(undo: Vec<u8>) -> (Simulation, Session) {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 50);
    let export = SessionExport {
        undo,
        ..session.export().unwrap()
    };
    let mut session = resume(&mut sim, export);
    resimulate_all(&mut session, &mut sim);
    (sim, session)
}

#[test]
fn a_fork_entry_naming_the_root_is_dropped_not_a_panic() {
    let take_over = UndoEntryV3::Fork {
        branch: ROOT,
        cause: ForkCauseV3::TakeOver {
            tick: 5,
            input: Vec::new(),
        },
        name: String::new(),
        removed: Vec::new(),
    };
    let undo = undo_history_v3(vec![take_over], Vec::new());
    // The bytes of a crafted history that ended the game process at its undo.
    assert_eq!(undo, [3, 1, 2, 0, 1, 5, 0, 0, 0, 0]);
    let (mut sim, mut session) = resumed_with_undo(undo);
    let timeline = session.timeline();
    assert!(!timeline.can_undo, "{:?}", timeline.undo_label);
    assert_eq!(
        session.warnings(),
        [
            "the undo history's `Undo take-over at tick 5` names the root branch as the branch it forked; it was dropped"
        ]
    );
    assert!(matches!(
        session.undo(&mut sim),
        Err(SessionError::NothingToUndo)
    ));
}

#[test]
fn a_redo_entry_forking_past_its_parent_is_refused() {
    let far = 1 << 36;
    let fork = UndoEntryV3::Fork {
        branch: BranchId(1),
        cause: ForkCauseV3::TakeOver {
            tick: far,
            input: Vec::new(),
        },
        name: "Branch 2".to_string(),
        removed: removed_branch(1, ROOT, far),
    };
    let (mut sim, mut session) = resumed_with_undo(undo_history_v3(Vec::new(), vec![fork]));
    let timeline = session.timeline();
    assert!(!timeline.can_redo, "{:?}", timeline.redo_label);
    assert_eq!(
        session.warnings(),
        [format!(
            "the undo history's `Redo take-over at tick {far}` restores branches that do not fit: a branch forks outside its parent's run; it was dropped"
        )]
    );
    assert!(matches!(
        session.redo(&mut sim),
        Err(SessionError::NothingToRedo)
    ));
}

#[test]
fn a_redone_fork_after_a_resume_keeps_branch_identifiers_unique() {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 100);
    edit_at(&mut session, &mut sim, 50, 1);
    session.undo(&mut sim).unwrap();
    // The tree is linear again, so the export carries none.
    let mut session = resume(&mut sim, session.export().unwrap());
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);
    edit_at(&mut session, &mut sim, 20, 2);
    let tree: Vec<(BranchId, Option<BranchId>)> = session
        .branches()
        .iter()
        .map(|branch| (branch.id, branch.parent))
        .collect();
    assert_eq!(
        tree,
        [
            (ROOT, None),
            (BranchId(1), Some(ROOT)),
            (BranchId(2), Some(BranchId(1)))
        ]
    );
    assert_eq!(session.timeline().branch, BranchId(2));
}

#[test]
fn entries_naming_a_branch_the_session_does_not_have_are_dropped_on_resume() {
    let (sim, _) = started(0);
    let edit = TimedEdit {
        tick: 30,
        edit: stats_edit(&sim, 1),
    };
    let run = |branch| UndoEntryV3::Run {
        branch,
        edit: edit.clone(),
        removed: Vec::new(),
    };
    // A redo of a fork restores branch 1 under a parent the tree does not have.
    let orphan = UndoEntryV3::Fork {
        branch: BranchId(1),
        cause: ForkCauseV3::Edit(edit.clone()),
        name: "Branch 2".to_string(),
        removed: removed_branch(1, BranchId(9), 30),
    };
    let undo = undo_history_v3(
        vec![run(ROOT), run(BranchId(7))],
        vec![orphan, run(BranchId(8))],
    );
    let (_, session) = resumed_with_undo(undo);
    assert_eq!(
        session.warnings(),
        [
            "the undo history's `Undo edit at tick 30` names branch 7, which the session does not have; it was dropped",
            "the undo history's `Redo edit at tick 30` names branch 8, which the session does not have; it was dropped",
            "the undo history's `Redo edit at tick 30` restores branches that do not fit: a branch's parent is missing or not older than it, or its identifier is taken; it was dropped",
        ]
    );
    let timeline = session.timeline();
    assert_eq!(timeline.undo_label.as_deref(), Some("Undo edit at tick 30"));
    assert!(!timeline.can_redo);
}

#[test]
fn an_undo_history_that_does_not_decode_resumes_empty_with_a_warning() {
    // Version 3 and an entry of a variant this build does not have.
    let (_, session) = resumed_with_undo(vec![3, 1, 9]);
    let timeline = session.timeline();
    assert!(!timeline.can_undo && !timeline.can_redo);
    let [warning] = session.warnings() else {
        panic!("one warning: {:?}", session.warnings());
    };
    assert!(
        warning.starts_with("the undo history cannot be decoded: ")
            && warning.ends_with("; it was dropped, so undo and redo start empty"),
        "{warning}"
    );
}

#[test]
fn a_redo_entry_of_version_2_redoes_within_its_branch() {
    let edit = TimedEdit {
        tick: 40,
        edit: stats_edit(&simulation(), 1_000),
    };
    let redo = vec![UndoEntryV2::Run { edit: edit.clone() }];
    let undo = postcard::to_allocvec(&(2u32, Vec::<UndoEntryV2>::new(), redo)).unwrap();
    let (mut sim, mut session) = resumed_with_undo(undo);
    assert_eq!(
        session.timeline().redo_label.as_deref(),
        Some("Redo edit at tick 40")
    );
    session.redo(&mut sim).unwrap();
    assert_eq!(edits(&session), [edit]);
    assert_eq!(session.timeline().current, 40);
}

#[test]
fn removed_branches_of_an_entry_not_undone_never_refuse_the_session() {
    // Only an undone entry holds the branches its undo removed: the undo writes them, so
    // bytes there that do not decode are never read.
    let entry = UndoEntryV3::Run {
        branch: ROOT,
        edit: TimedEdit {
            tick: 30,
            edit: stats_edit(&simulation(), 1),
        },
        removed: vec![0xff; 3],
    };
    let (_, session) = resumed_with_undo(undo_history_v3(vec![entry], Vec::new()));
    assert!(session.warnings().is_empty(), "{:?}", session.warnings());
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo edit at tick 30")
    );
}

#[test]
fn a_redo_entry_redoing_within_its_branch_never_cuts_a_branch_off() {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 100);
    // Branch 1 forks from the root at tick 50.
    edit_at(&mut session, &mut sim, 50, 1);
    session.switch_branch(&mut sim, ROOT).unwrap();
    resimulate_all(&mut session, &mut sim);
    let run = |branch, tick| UndoEntryV3::Run {
        branch,
        edit: TimedEdit {
            tick,
            edit: stats_edit(&sim, 2),
        },
        removed: Vec::new(),
    };
    // Redo takes the last entry first. Each redoes within its branch, cutting the run
    // after its tick: at 40 the root's run would end before Branch 1 forks, and Branch 1's
    // run would record an edit before its fork; at 50 nothing is cut off.
    let redo = vec![run(ROOT, 50), run(BranchId(1), 40), run(ROOT, 40)];
    let export = SessionExport {
        undo: undo_history_v3(Vec::new(), redo),
        ..session.export().unwrap()
    };
    let mut session = resume(&mut sim, export);
    assert_eq!(
        session.warnings(),
        [
            "the undo history's `Redo edit at tick 40` cannot be redone within its branch: a branch forks outside its parent's run; it was dropped",
            "the undo history's `Redo edit at tick 40` cannot be redone within its branch: a branch records an edit outside its run; it was dropped",
        ]
    );
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);
    assert!(!session.timeline().can_redo);
    assert_eq!(session.current_branch(), ROOT);
    // The tree keeps every fork on its parent's run, so the session loads again.
    let session = resume(&mut sim, session.export().unwrap());
    assert_eq!(branch_ids(&session), [ROOT, BranchId(1)]);
}

#[test]
fn a_redo_entry_of_a_fork_restoring_no_branch_is_dropped() {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 100);
    edit_at(&mut session, &mut sim, 50, 1);
    session.switch_branch(&mut sim, ROOT).unwrap();
    resimulate_all(&mut session, &mut sim);
    // An undone fork holds the branches it removed: the forked one at least.
    let fork = UndoEntryV3::Fork {
        branch: BranchId(1),
        cause: ForkCauseV3::TakeOver {
            tick: 30,
            input: Vec::new(),
        },
        name: "Branch 2".to_string(),
        removed: Vec::new(),
    };
    let export = SessionExport {
        undo: undo_history_v3(Vec::new(), vec![fork]),
        ..session.export().unwrap()
    };
    let session = resume(&mut sim, export);
    let [warning] = session.warnings() else {
        panic!("one warning: {:?}", session.warnings());
    };
    assert!(
        warning.starts_with(
            "the undo history's `Redo take-over at tick 30` restores branches that do not fit: "
        ),
        "{warning}"
    );
    assert!(!session.timeline().can_redo);
}

/// The branch tree a session export carries.
#[derive(Serialize)]
struct StoredTree {
    current: BranchId,
    next: u32,
    branches: Vec<StoredBranchV3>,
}

/// A branch of a crafted tree, with nothing recorded after its fork point.
fn tree_branch(id: u32, parent: Option<BranchId>, fork_tick: u64) -> StoredBranchV3 {
    StoredBranchV3 {
        id: BranchId(id),
        name: format!("Branch {}", u64::from(id) + 1),
        parent,
        fork_tick,
        fork_edits: 0,
        stale: false,
        inputs: InputLog::new(1).encode(),
        edits: EditLog::new().encode(),
        hashes: Vec::new(),
    }
}

/// A linear session of 100 ticks played to tick 50, exported with the branch tree `tree`.
fn exported_with_tree(tree: StoredTree) -> (Simulation, SessionExport) {
    let (mut sim, mut session) = started(100);
    play(&mut session, &mut sim, 50);
    let export = SessionExport {
        branches: postcard::to_allocvec(&tree).unwrap(),
        ..session.export().unwrap()
    };
    (sim, export)
}

#[test]
fn a_branch_tree_holding_the_last_identifier_is_refused_not_a_panic() {
    let tree = StoredTree {
        current: ROOT,
        next: 1,
        branches: vec![
            tree_branch(0, None, 0),
            tree_branch(u32::MAX, Some(ROOT), 20),
        ],
    };
    let (mut sim, export) = exported_with_tree(tree);

    let result = Session::resume(&mut sim, export, Document::Scene);

    let Err(error) = result else {
        panic!("the branch tree was accepted");
    };
    assert!(
        matches!(
            error,
            SessionError::BranchesDecode(BranchesError::Exhausted)
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "the branch tree cannot be decoded: branch identifiers are exhausted: they are never reused, and none is left for a new branch"
    );
}

#[test]
fn an_edit_or_a_take_over_that_would_fork_once_identifiers_are_exhausted_changes_nothing() {
    let tree = StoredTree {
        current: ROOT,
        next: u32::MAX,
        branches: vec![tree_branch(0, None, 0)],
    };
    let (mut sim, export) = exported_with_tree(tree);
    let mut session = resume(&mut sim, export);
    resimulate_all(&mut session, &mut sim);
    session.seek(&mut sim, 20).unwrap();
    let (before, hash) = (session.export().unwrap(), sim.hash());
    let edit = stats_edit(&sim, 1_000);
    type Attempt<'a> = &'a dyn Fn(&mut Session, &mut Simulation) -> Result<(), SessionError>;
    let edit: Attempt = &|session, sim| session.edit(sim, edit.clone()).map(|_| ());
    let take_over: Attempt = &|session, sim| session.step(sim, Some(&live_spawn())).map(|_| ());

    for attempt in [edit, take_over] {
        let result = attempt(&mut session, &mut sim);

        let Err(error) = result else {
            panic!("a branch forked");
        };
        assert!(
            matches!(
                error,
                SessionError::CannotFork {
                    tick: 20,
                    source: BranchesError::Exhausted
                }
            ),
            "{error:?}"
        );
        assert_eq!(session.export().unwrap(), before);
        assert_eq!(branch_ids(&session), [ROOT]);
        assert_eq!(session.timeline().current, 20);
        assert_eq!(sim.hash(), hash);
    }
}
