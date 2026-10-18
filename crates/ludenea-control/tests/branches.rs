use boxes::BoxesInput;
use ludenea::control::Document;
use ludenea::control::checkpoints::CHECKPOINT_BUDGET;
use ludenea::control::{
    BranchId, BranchInfo, Reply, Resimulation, Session, SessionError, SessionExport,
};
use ludenea::prelude::*;
use ludenea_app::{AssetTexts, ValueKind};
use serde_json::json;

fn simulation() -> Simulation {
    boxes::app().build().unwrap()
}

fn small_scene() -> String {
    boxes::generate_scene(30, 7)
}

fn spawn_input(x: f32) -> Vec<u8> {
    let mut log = InputLog::new(1);
    log.record(&[BoxesInput {
        spawn: Some(x),
        ..BoxesInput::default()
    }]);
    log.ticks.remove(0)
}

/// `ticks` recorded inputs with a nudge every 37 ticks.
fn recorded(ticks: u64) -> InputLog {
    let mut log = InputLog::new(1);
    for tick in 0..ticks {
        log.record(&[BoxesInput {
            velocity_change: (tick % 37 == 0).then(|| Vec2::new(1.0, 4.0)),
            ..BoxesInput::default()
        }]);
    }
    log
}

/// A session with `ticks` recorded inputs, played up to `played`.
fn played(ticks: u64, played: u64) -> (Simulation, Session) {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    session.load_inputs(&mut sim, recorded(ticks)).unwrap();
    play(&mut session, &mut sim, played);
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

fn edits(session: &Session) -> Vec<TimedEdit> {
    EditLog::decode(&session.export().unwrap().edits)
        .unwrap()
        .edits
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

/// Seeks to `tick` and edits there.
fn edit_at(session: &mut Session, sim: &mut Simulation, tick: u64, begins: u64) {
    session.seek(sim, tick).unwrap();
    let edit = stats_edit(sim, begins);
    session.edit(sim, edit).unwrap();
}

fn branch(session: &Session, id: BranchId) -> BranchInfo {
    session
        .branches()
        .into_iter()
        .find(|branch| branch.id == id)
        .unwrap()
}

fn resimulate_all(session: &mut Session, sim: &mut Simulation) {
    while !matches!(
        session.resimulate(sim, 256).unwrap(),
        Resimulation::Done | Resimulation::Idle
    ) {}
}

/// Seeks to ticks without edits and checks the world against `expected`.
fn assert_seeks_match(session: &mut Session, sim: &mut Simulation, expected: &[u64], step: u64) {
    let edited: Vec<u64> = edits(session).iter().map(|edit| edit.tick).collect();
    let last = expected.len() as u64 - 1;
    for tick in (0..=last).step_by(step as usize).chain([last]) {
        if edited.contains(&tick) {
            continue;
        }
        session.seek(sim, tick).unwrap();
        assert_eq!(sim.hash(), expected[tick as usize], "tick {tick}");
    }
}

const ROOT: BranchId = BranchId::ROOT;

#[test]
fn playing_forward_never_creates_branches() {
    let (mut sim, mut session) = played(200, 200);
    for tick in 0..100 {
        let live = (tick % 30 == 0).then(|| spawn_input(1.0));
        session.step(&mut sim, live.as_deref()).unwrap();
    }
    assert_eq!(session.branches().len(), 1);
    assert_eq!(session.current_branch(), ROOT);
    assert_eq!(session.timeline().branch, ROOT);
}

#[test]
fn past_edit_forks_and_keeps_parent_future() {
    let (mut sim, mut session) = played(400, 300);
    let before = hashes(&session);
    edit_at(&mut session, &mut sim, 150, 1_000);

    let child = BranchId(1);
    assert_eq!(session.current_branch(), child);
    assert_eq!(
        branch(&session, child),
        BranchInfo {
            id: child,
            name: "Branch 2".to_string(),
            parent: Some(ROOT),
            fork_tick: 150,
            head: 150,
            stale: false,
            edit_ticks: vec![150],
        }
    );
    let timeline = session.timeline();
    assert_eq!(
        (timeline.branch, timeline.head, timeline.current),
        (child, 150, 150)
    );
    assert_eq!(timeline.branches, session.branches());
    assert_eq!(branch(&session, ROOT).head, 399);

    session.switch_branch(&mut sim, ROOT).unwrap();
    assert_eq!(hashes(&session), before);
    assert!(edits(&session).is_empty());
    assert_eq!(session.timeline().head, 399);
    // A branch that is not the current one still shows its edits.
    assert_eq!(branch(&session, child).edit_ticks, vec![150]);
    assert_eq!(branch(&session, ROOT).edit_ticks, Vec::<u64>::new());
}

#[test]
fn forking_edit_survives_switching_away_and_back() {
    let (mut sim, mut session) = played(400, 300);
    let parent = hashes(&session);
    edit_at(&mut session, &mut sim, 200, 1_000);
    let edited = sim.hash();
    play(&mut session, &mut sim, 50);
    let child = hashes(&session);

    session.switch_branch(&mut sim, ROOT).unwrap();
    assert_eq!(hashes(&session), parent);
    session.seek(&mut sim, 200).unwrap();
    assert_ne!(sim.hash(), edited);
    session.seek(&mut sim, 300).unwrap();
    assert_eq!(sim.hash(), parent[300]);

    session.switch_branch(&mut sim, BranchId(1)).unwrap();
    assert_eq!(hashes(&session), child);
    session.seek(&mut sim, 200).unwrap();
    assert_eq!(sim.hash(), edited, "the forking edit is applied");
    session.seek(&mut sim, 250).unwrap();
    assert_eq!(sim.hash(), child[250]);
}

#[test]
fn edits_recorded_before_the_fork_at_the_same_tick_stay_in_the_parent() {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    play(&mut session, &mut sim, 200);
    let edit = stats_edit(&sim, 1);
    session.edit(&mut sim, edit).unwrap();
    play(&mut session, &mut sim, 100);
    assert_eq!(session.branches().len(), 1);
    let parent = edits(&session);

    edit_at(&mut session, &mut sim, 200, 2);
    assert_eq!(
        branch(&session, BranchId(1)).fork_tick,
        200,
        "the second edit at 200 forks"
    );
    let child = edits(&session);
    assert_eq!(child.len(), 2);
    assert_eq!(child[0], parent[0]);

    session.switch_branch(&mut sim, ROOT).unwrap();
    assert_eq!(edits(&session), parent);
    session.switch_branch(&mut sim, BranchId(1)).unwrap();
    assert_eq!(edits(&session), child);
}

#[test]
fn take_over_forks() {
    let (mut sim, mut session) = played(400, 300);
    let before = hashes(&session);
    session.seek(&mut sim, 100).unwrap();
    session.step(&mut sim, Some(&spawn_input(2.0))).unwrap();

    let child = branch(&session, BranchId(1));
    assert_eq!(
        (child.parent, child.fork_tick, child.head),
        (Some(ROOT), 100, 100)
    );
    assert_eq!(session.timeline().current, 101);
    assert_ne!(sim.hash(), before[101]);

    session.switch_branch(&mut sim, ROOT).unwrap();
    assert_eq!(hashes(&session), before);
    assert_eq!(session.timeline().head, 399);
}

#[test]
fn edit_at_head_extends_current_branch() {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    play(&mut session, &mut sim, 200);
    let edit = stats_edit(&sim, 1);
    session.edit(&mut sim, edit).unwrap();
    assert_eq!(session.branches().len(), 1);
    assert_eq!(session.timeline().edit_ticks, vec![200]);
}

#[test]
fn switching_keeps_tick_when_covered() {
    let (mut sim, mut session) = played(400, 300);
    let root = hashes(&session);
    edit_at(&mut session, &mut sim, 150, 1_000);
    play(&mut session, &mut sim, 50);

    session.switch_branch(&mut sim, ROOT).unwrap();
    assert_eq!(session.timeline().current, 200);
    assert_eq!(sim.hash(), root[200]);
}

#[test]
fn switching_goes_to_head_when_not_covered() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 150, 1_000);
    play(&mut session, &mut sim, 50);
    let child = hashes(&session);
    session.switch_branch(&mut sim, ROOT).unwrap();
    session.seek(&mut sim, 350).unwrap();

    session.switch_branch(&mut sim, BranchId(1)).unwrap();
    let timeline = session.timeline();
    assert_eq!(timeline.current, timeline.head);
    assert_eq!(timeline.current, 199);
    assert_eq!(sim.hash(), child[199]);
}

#[test]
fn sibling_switch_reproduces_each_branch_hashes() {
    let (mut sim, mut session) = played(400, 300);
    let root = hashes(&session);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    let first = hashes(&session);
    session.switch_branch(&mut sim, ROOT).unwrap();
    edit_at(&mut session, &mut sim, 100, 2_000);
    play(&mut session, &mut sim, 150);
    let second = hashes(&session);
    assert_eq!(branch(&session, BranchId(2)).parent, Some(ROOT));
    assert_ne!(first[250], second[250]);

    for _ in 0..2 {
        session.switch_branch(&mut sim, BranchId(1)).unwrap();
        assert_eq!(hashes(&session), first);
        assert_seeks_match(&mut session, &mut sim, &first, 23);
        session.switch_branch(&mut sim, BranchId(2)).unwrap();
        assert_eq!(hashes(&session), second);
        assert_seeks_match(&mut session, &mut sim, &second, 23);
        session.switch_branch(&mut sim, ROOT).unwrap();
        assert_seeks_match(&mut session, &mut sim, &root, 23);
    }
}

#[test]
fn nested_branch_composes_ancestors() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    edit_at(&mut session, &mut sim, 200, 2_000);
    play(&mut session, &mut sim, 80);
    let nested = hashes(&session);
    let nested_edits = edits(&session);
    assert_eq!(branch(&session, BranchId(2)).parent, Some(BranchId(1)));
    assert_eq!(nested_edits.len(), 2);

    session.switch_branch(&mut sim, ROOT).unwrap();
    session.switch_branch(&mut sim, BranchId(2)).unwrap();
    assert_eq!(hashes(&session), nested);
    assert_eq!(edits(&session), nested_edits);
    assert_seeks_match(&mut session, &mut sim, &nested, 31);

    // Flattened, the branch replays as one linear run.
    let export = session.export().unwrap();
    let mut fresh = simulation();
    fresh.load_scene(&export.scene).unwrap();
    let inputs = InputLog::decode(&export.inputs).unwrap();
    let log = EditLog::decode(&export.edits).unwrap();
    fresh.set_players(usize::from(inputs.players));
    for tick in 0..nested.len() as u64 - 1 {
        for timed in log.at(tick) {
            fresh.apply_edit(&timed.edit).unwrap();
        }
        fresh.step_encoded(inputs.tick(tick)).unwrap();
        assert_eq!(fresh.hash(), nested[tick as usize + 1], "tick {}", tick + 1);
    }
}

#[test]
fn rename_branch_keeps_identifier() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 150, 1_000);
    session
        .rename_branch(BranchId(1), "Higher jump".to_string())
        .unwrap();
    let renamed = branch(&session, BranchId(1));
    assert_eq!(
        (renamed.id, renamed.name.as_str()),
        (BranchId(1), "Higher jump")
    );
    assert_eq!(session.current_branch(), BranchId(1));
    assert!(matches!(
        session.rename_branch(BranchId(7), "Nope".to_string()),
        Err(SessionError::NoSuchBranch(BranchId(7)))
    ));
}

#[test]
fn delete_removes_descendants_and_switches_to_parent() {
    let (mut sim, mut session) = played(400, 300);
    let root = hashes(&session);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    edit_at(&mut session, &mut sim, 200, 2_000);
    play(&mut session, &mut sim, 80);
    session.switch_branch(&mut sim, ROOT).unwrap();
    edit_at(&mut session, &mut sim, 50, 3_000);
    session.switch_branch(&mut sim, BranchId(2)).unwrap();
    session.seek(&mut sim, 280).unwrap();

    let deleted = session.delete_branch(&mut sim, BranchId(1)).unwrap();
    assert_eq!(deleted, vec![BranchId(2)]);
    let ids: Vec<BranchId> = session.branches().iter().map(|b| b.id).collect();
    assert_eq!(ids, vec![ROOT, BranchId(3)]);
    assert_eq!(session.current_branch(), ROOT);
    assert_eq!(session.timeline().current, 280);
    assert_eq!(sim.hash(), root[280]);
    assert!(matches!(
        session.switch_branch(&mut sim, BranchId(1)),
        Err(SessionError::NoSuchBranch(BranchId(1)))
    ));
    assert!(matches!(
        session.delete_branch(&mut sim, BranchId(2)),
        Err(SessionError::NoSuchBranch(BranchId(2)))
    ));
}

#[test]
fn root_cannot_be_deleted() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 150, 1_000);
    assert!(matches!(
        session.delete_branch(&mut sim, ROOT),
        Err(SessionError::RootBranch)
    ));
    assert_eq!(session.branches().len(), 2);
}

#[test]
fn undoing_a_forking_edit_deletes_its_branch() {
    let (mut sim, mut session) = played(400, 300);
    let root = hashes(&session);
    edit_at(&mut session, &mut sim, 150, 1_000);
    play(&mut session, &mut sim, 20);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.branches().len(), 1);
    assert_eq!(session.current_branch(), ROOT);
    assert_eq!(session.timeline().current, 150);
    assert_eq!(hashes(&session), root);
}

#[test]
fn pinned_checkpoints_are_never_evicted() {
    let (mut sim, mut session) = played(400, 300);
    // A snapshot of another state restored at tick 40 is pinned there.
    let mut other = simulation();
    let mut elsewhere = Session::start(
        &mut other,
        boxes::generate_scene(12, 3),
        AssetTexts::new(),
        Document::Scene,
    )
    .unwrap();
    play(&mut elsewhere, &mut other, 40);
    edit_at(&mut session, &mut sim, 20, 1_000);
    play(&mut session, &mut sim, 20);
    session.restore(&mut sim, &other.snapshot()).unwrap();
    let pinned = BranchId(1);
    assert!(session.timeline().checkpoints.contains(&40));
    play(&mut session, &mut sim, 300);
    let pinned_checkpoints = session.timeline().checkpoints;

    // Many siblings of the pinned branch push the session past its budget.
    let mut forks = 0;
    while session.checkpoint_count() < CHECKPOINT_BUDGET || forks < 8 {
        session.switch_branch(&mut sim, ROOT).unwrap();
        edit_at(&mut session, &mut sim, 10, forks);
        play(&mut session, &mut sim, 390);
        forks += 1;
        assert!(session.checkpoint_count() <= CHECKPOINT_BUDGET);
        assert!(forks < 60, "the budget is never reached");
    }

    session.switch_branch(&mut sim, pinned).unwrap();
    let kept = session.timeline().checkpoints;
    assert!(kept.contains(&40), "{kept:?}");
    assert!(
        kept.len() < pinned_checkpoints.len(),
        "the budget evicted others of the branch: {kept:?}"
    );
}

#[test]
fn undo_of_a_fork_from_another_branch_keeps_the_parent_timeline() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 150, 1_000);
    session.switch_branch(&mut sim, ROOT).unwrap();
    let before = hashes(&session);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.branches().len(), 1);
    let timeline = session.timeline();
    assert_eq!((timeline.current, timeline.head), (150, 399));
    assert_eq!(hashes(&session), before);
}

#[test]
fn restore_at_the_fork_tick_keeps_the_branch_own_later_edits() {
    let (mut sim, mut session) = played(400, 300);
    // The root records A at 200 (forking branch 1), then B at 200 forks branch 2 at
    // (200, 1), which records D at 220.
    edit_at(&mut session, &mut sim, 200, 1);
    play(&mut session, &mut sim, 20);
    edit_at(&mut session, &mut sim, 200, 2);
    play(&mut session, &mut sim, 20);
    let edit = stats_edit(&sim, 4);
    session.edit(&mut sim, edit).unwrap();
    let nested = edits(&session);
    assert_eq!(
        nested.iter().map(|e| e.tick).collect::<Vec<_>>(),
        [200, 200, 220]
    );

    let mut other = simulation();
    let mut elsewhere = Session::start(
        &mut other,
        boxes::generate_scene(12, 3),
        AssetTexts::new(),
        Document::Scene,
    )
    .unwrap();
    play(&mut elsewhere, &mut other, 200);
    session.restore(&mut sim, &other.snapshot()).unwrap();
    assert_eq!(edits(&session), [nested[2].clone()]);

    session.switch_branch(&mut sim, ROOT).unwrap();
    session.switch_branch(&mut sim, BranchId(2)).unwrap();
    // The branch no longer inherits A: the edits stay as the restore left them.
    assert_eq!(edits(&session), [nested[2].clone()]);
    // Every entry still undoes, the forks with their branches.
    while session.timeline().can_undo {
        session.undo(&mut sim).unwrap();
    }
    assert_eq!(session.branches().len(), 1);
}

/// The export of a session forked twice, nested, with the nested branch current.
fn branched_export() -> (Vec<Vec<u64>>, Vec<BranchInfo>, SessionExport) {
    let (mut sim, mut session) = played(400, 300);
    let root = hashes(&session);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    let first = hashes(&session);
    edit_at(&mut session, &mut sim, 200, 2_000);
    play(&mut session, &mut sim, 80);
    let nested = hashes(&session);
    session.rename_branch(ROOT, "Main".to_string()).unwrap();
    (
        vec![root, first, nested],
        session.branches(),
        session.export().unwrap(),
    )
}

fn resume(sim: &mut Simulation, export: SessionExport) -> Session {
    Session::resume(sim, export, Document::Scene).unwrap()
}

#[test]
fn export_resume_keeps_the_tree() {
    let (expected, branches, export) = branched_export();
    assert!(!export.branches.is_empty());
    let mut sim = simulation();
    let mut session = resume(&mut sim, export);
    let shape = |branches: Vec<BranchInfo>| -> Vec<_> {
        branches
            .into_iter()
            .map(|b| (b.id, b.name, b.parent, b.fork_tick, b.head))
            .collect()
    };
    assert_eq!(shape(session.branches()), shape(branches));
    assert_eq!(session.current_branch(), BranchId(2));
    resimulate_all(&mut session, &mut sim);
    assert_eq!(hashes(&session), expected[2]);

    for (id, hashes_of) in [(ROOT, &expected[0]), (BranchId(1), &expected[1])] {
        session.switch_branch(&mut sim, id).unwrap();
        assert!(session.timeline().resimulating, "a resumed branch is stale");
        resimulate_all(&mut session, &mut sim);
        assert!(!branch(&session, id).stale);
        // Resimulation also runs the recorded inputs past the ticks played before.
        assert!(hashes(&session).starts_with(hashes_of));
        assert_seeks_match(&mut session, &mut sim, hashes_of, 41);
    }
}

#[test]
fn linear_session_exports_no_branches() {
    let (_, session) = played(100, 50);
    assert!(session.export().unwrap().branches.is_empty());
}

#[test]
fn export_from_2a_resumes_as_root() {
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
    let mut sim = simulation();
    let export = SessionExport {
        scene,
        saved_scene,
        inputs: inputs.0,
        edits: edits.0,
        undo: undo.0,
        hashes: hashes.into_iter().map(|hash| hash.0).collect(),
        branches: Vec::new(),
        assets: Vec::new(),
        keeps_assets: false,
    };
    let mut session = Session::resume(&mut sim, export, Document::Scene).unwrap();
    resimulate_all(&mut session, &mut sim);
    let branches = session.branches();
    assert_eq!(branches.len(), 1);
    let root = &branches[0];
    assert_eq!((root.id, root.parent, root.fork_tick), (ROOT, None, 0));
    assert_eq!(root.head, session.timeline().head);
    assert_eq!(session.current_branch(), ROOT);
}

#[test]
#[ignore = "release only: 21 runs of 10,000 ticks"]
fn checkpoint_budget_with_20_branches_of_10000_ticks() {
    let (mut sim, mut session) = played(10_000, 10_000);
    let mut expected = vec![(ROOT, hashes(&session))];
    for branch in 1..=20 {
        session.switch_branch(&mut sim, ROOT).unwrap();
        edit_at(
            &mut session,
            &mut sim,
            100 + 450 * u64::from(branch),
            branch.into(),
        );
        play(&mut session, &mut sim, 10_000);
        expected.push((BranchId(branch), hashes(&session)));
        assert!(session.checkpoint_count() <= CHECKPOINT_BUDGET);
    }
    assert_eq!(session.branches().len(), 21);
    for (id, hashes_of) in &expected {
        session.switch_branch(&mut sim, *id).unwrap();
        assert_eq!(&hashes(&session), hashes_of);
        assert_seeks_match(&mut session, &mut sim, hashes_of, 997);
        assert!(session.checkpoint_count() <= CHECKPOINT_BUDGET);
    }
}

#[test]
#[ignore = "timing, run in release"]
// Measuring time is outside the simulation.
#[allow(clippy::disallowed_methods)]
fn switch_branch_under_200_ms() {
    let (mut sim, mut session) = played(10_000, 10_000);
    let root_hashes = hashes(&session);
    edit_at(&mut session, &mut sim, 9_000, 1_000);
    play(&mut session, &mut sim, 1_000);

    let started = std::time::Instant::now();
    session.switch_branch(&mut sim, ROOT).unwrap();
    let elapsed = started.elapsed();
    let timeline = session.timeline();
    assert_eq!(timeline.current, 10_000);
    assert!(
        timeline
            .checkpoints
            .iter()
            .any(|&tick| tick <= timeline.current && timeline.current - tick <= 64),
        "{:?}",
        timeline.checkpoints
    );
    assert_eq!(sim.hash(), root_hashes[10_000]);
    println!("switching branches took {elapsed:?}");
    assert!(elapsed.as_secs_f64() < 0.2, "{elapsed:?}");
}

/// Edits the scene at tick 0, where the session must be.
fn scene_edit(session: &mut Session, sim: &mut Simulation, begins: u64) {
    session.seek(sim, 0).unwrap();
    let edit = stats_edit(sim, begins);
    session.edit(sim, edit).unwrap();
}

/// The ticks of the progress reports, checked to increase strictly.
fn progress(reports: &[Resimulation]) -> Vec<u64> {
    let ticks: Vec<u64> = reports
        .iter()
        .filter_map(|report| match report {
            Resimulation::Progress { up_to } => Some(*up_to),
            _ => None,
        })
        .collect();
    assert!(ticks.windows(2).all(|pair| pair[0] < pair[1]), "{ticks:?}");
    ticks
}

/// Every report until resimulation is done.
fn reports_until_done(session: &mut Session, sim: &mut Simulation) -> Vec<Resimulation> {
    let mut reports = Vec::new();
    loop {
        let report = session.resimulate(sim, 64).unwrap();
        let done = matches!(report, Resimulation::Done | Resimulation::Idle);
        reports.push(report);
        if done {
            return reports;
        }
    }
}

/// Checks the current branch's hashes against a fresh session of its run and scene, which
/// simulates up to the head.
fn assert_replays(session: &Session) {
    let hashes = hashes(session);
    let replayed = replayed_hashes(session);
    assert_eq!(replayed.len() as u64, session.timeline().head + 1);
    assert_eq!(hashes[..replayed.len()], replayed);
}

fn replayed_hashes(session: &Session) -> Vec<u64> {
    let export = session.export().unwrap();
    let mut sim = simulation();
    let mut fresh =
        Session::start(&mut sim, export.scene, AssetTexts::new(), Document::Scene).unwrap();
    fresh
        .load_inputs(&mut sim, InputLog::decode(&export.inputs).unwrap())
        .unwrap();
    fresh
        .load_edits(&mut sim, EditLog::decode(&export.edits).unwrap())
        .unwrap();
    resimulate_all(&mut fresh, &mut sim);
    // Inputs alone are simulated on the way to a tick.
    let head = fresh.timeline().head;
    fresh.seek(&mut sim, head).unwrap();
    hashes(&fresh)[..=head as usize].to_vec()
}

#[test]
fn scene_edit_marks_other_branches_stale_and_drops_their_checkpoints() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    session.switch_branch(&mut sim, ROOT).unwrap();
    assert!(session.checkpoint_count() > 2);

    scene_edit(&mut session, &mut sim, 5);
    assert!(branch(&session, BranchId(1)).stale);
    assert!(!branch(&session, ROOT).stale);
    // Only the new tick 0 remains: the current branch resimulates from it.
    assert_eq!(session.checkpoint_count(), 1);
    assert_eq!(session.timeline().checkpoints, [0]);
}

#[test]
fn switching_to_stale_branch_resimulates_it() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    session.switch_branch(&mut sim, ROOT).unwrap();
    scene_edit(&mut session, &mut sim, 5);
    assert!(session.timeline().resimulating);

    // While the root still resimulates.
    session.switch_branch(&mut sim, BranchId(1)).unwrap();
    assert_eq!(session.current_branch(), BranchId(1));
    assert_eq!(session.timeline().branch, BranchId(1));
    assert!(session.timeline().resimulating);
    assert!(matches!(
        session.seek(&mut sim, 200),
        Err(SessionError::Resimulating { .. })
    ));
    let first = session.resimulate(&mut sim, 30).unwrap();
    assert!(
        matches!(
            first,
            Resimulation::Progress { up_to: 30 } | Resimulation::Diverged { .. }
        ),
        "{first:?}"
    );
    assert!(matches!(
        session.seek(&mut sim, 31),
        Err(SessionError::Resimulating { up_to: 30 })
    ));
    session.seek(&mut sim, 20).unwrap();

    let mut reports = vec![first];
    reports.extend(reports_until_done(&mut session, &mut sim));
    assert!(progress(&reports).len() > 2, "{reports:?}");
    assert_eq!(session.current_branch(), BranchId(1));
    assert!(!branch(&session, BranchId(1)).stale);
    assert_replays(&session);
    session.seek(&mut sim, 200).unwrap();

    // The root resimulated up to the fork only: the rest resimulates from there.
    assert!(branch(&session, ROOT).stale);
    session.switch_branch(&mut sim, ROOT).unwrap();
    assert!(session.timeline().resimulating);
    let rest = progress(&reports_until_done(&mut session, &mut sim));
    assert!(rest[0] > 100, "{rest:?}");
    assert_replays(&session);
}

#[test]
fn stale_ancestor_resimulates_first() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    edit_at(&mut session, &mut sim, 200, 2_000);
    play(&mut session, &mut sim, 80);
    session.switch_branch(&mut sim, ROOT).unwrap();
    scene_edit(&mut session, &mut sim, 5);
    resimulate_all(&mut session, &mut sim);
    assert!(branch(&session, BranchId(1)).stale);
    assert!(branch(&session, BranchId(2)).stale);

    session.seek(&mut sim, 250).unwrap();
    session.switch_branch(&mut sim, BranchId(2)).unwrap();
    assert_eq!(session.current_branch(), BranchId(2));
    // The ticks Branch 2 shares with its stale parent show while the parent resimulates.
    assert_eq!(session.timeline().current, 100);
    let reports = reports_until_done(&mut session, &mut sim);
    assert!(progress(&reports).len() > 2, "{reports:?}");
    assert!(!branch(&session, BranchId(2)).stale);
    assert_eq!(session.timeline().current, 250);
    assert_replays(&session);

    // Branch 1 is fresh up to the fork of Branch 2, and resimulates from there.
    session.switch_branch(&mut sim, BranchId(1)).unwrap();
    assert!(session.timeline().resimulating);
    session.seek(&mut sim, 200).unwrap();
    assert!(matches!(
        session.seek(&mut sim, 201),
        Err(SessionError::Resimulating { up_to: 200 })
    ));
    reports_until_done(&mut session, &mut sim);
    assert_replays(&session);
    session.switch_branch(&mut sim, BranchId(2)).unwrap();
    assert!(!session.timeline().resimulating, "Branch 2 stays fresh");
    assert_replays(&session);
}

#[test]
fn seeking_while_stale_ancestors_resimulate_is_where_the_switch_ends() {
    let (mut sim, mut session) = played(400, 300);
    edit_at(&mut session, &mut sim, 100, 1_000);
    play(&mut session, &mut sim, 150);
    edit_at(&mut session, &mut sim, 200, 2_000);
    play(&mut session, &mut sim, 80);
    session.switch_branch(&mut sim, ROOT).unwrap();
    scene_edit(&mut session, &mut sim, 5);
    resimulate_all(&mut session, &mut sim);
    session.seek(&mut sim, 250).unwrap();
    session.switch_branch(&mut sim, BranchId(2)).unwrap();

    // The timeline shows Branch 2 as far as it shares Branch 1's run.
    let timeline = session.timeline();
    assert_eq!(timeline.branch, BranchId(2));
    assert_eq!(timeline.head, branch(&session, BranchId(2)).head);
    let shown = timeline.simulated;
    assert!(shown < 200, "{timeline:?}");
    assert!(
        timeline.checkpoints.iter().all(|&tick| tick <= shown),
        "{timeline:?}"
    );
    assert!(
        timeline.edit_ticks.iter().all(|&tick| tick <= shown),
        "{timeline:?}"
    );

    session.resimulate(&mut sim, 64).unwrap();
    session.seek(&mut sim, 150).unwrap();
    reports_until_done(&mut session, &mut sim);
    assert_eq!(session.current_branch(), BranchId(2));
    assert_eq!(session.timeline().current, 150);
    assert_replays(&session);
}

/// Inserts a pose on the scene entity at `address` at the current tick.
fn pose_edit(session: &mut Session, sim: &mut Simulation, address: u32) {
    let value = sim
        .encode_json_value(
            ValueKind::Component,
            "ludenea::Pose",
            json!({ "position": { "x": 1.0, "y": 5.0 }, "rotation": { "cos": 1.0, "sin": 0.0 } }),
        )
        .unwrap();
    let edit = Edit::Insert {
        entity: ludenea_app::EntityRef::Loaded(vec![address]),
        component: "ludenea::Pose".to_string(),
        value,
    };
    session.edit(sim, edit).unwrap();
}

#[test]
fn dropped_edit_is_reported_per_branch() {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    play(&mut session, &mut sim, 120);
    pose_edit(&mut session, &mut sim, 20);
    play(&mut session, &mut sim, 180);
    session.seek(&mut sim, 100).unwrap();
    pose_edit(&mut session, &mut sim, 20);
    play(&mut session, &mut sim, 100);
    assert_eq!(session.current_branch(), BranchId(1));
    session.switch_branch(&mut sim, ROOT).unwrap();

    session.seek(&mut sim, 0).unwrap();
    let despawn = Edit::Despawn {
        entity: ludenea_app::EntityRef::Loaded(vec![20]),
    };
    session.edit(&mut sim, despawn).unwrap();
    // One call past tick 120 queues the root's dropped edit behind its divergence.
    let first = session.resimulate(&mut sim, 200).unwrap();
    assert!(matches!(first, Resimulation::Diverged { .. }), "{first:?}");
    session.switch_branch(&mut sim, BranchId(1)).unwrap();

    let dropped: Vec<u64> = reports_until_done(&mut session, &mut sim)
        .into_iter()
        .filter_map(|report| match report {
            Resimulation::EditDropped { tick, .. } => Some(tick),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, [120, 100]);
    assert!(edits(&session).is_empty());
    // The root resimulated past the fork before the switch: the rest resimulates now.
    session.switch_branch(&mut sim, ROOT).unwrap();
    reports_until_done(&mut session, &mut sim);
    assert!(edits(&session).is_empty());
}

#[test]
fn fork_below_the_parent_fork_inherits_ancestor_edits_and_resumes() {
    let mut sim = simulation();
    let mut session =
        Session::start(&mut sim, small_scene(), AssetTexts::new(), Document::Scene).unwrap();
    play(&mut session, &mut sim, 5);
    let edit = stats_edit(&sim, 1);
    session.edit(&mut sim, edit).unwrap();
    play(&mut session, &mut sim, 15);
    edit_at(&mut session, &mut sim, 10, 2);
    let b = session.current_branch();
    // Below B's own fork: the edit of the root at tick 5 is inherited.
    edit_at(&mut session, &mut sim, 5, 3);
    let c = session.current_branch();
    assert_eq!(branch(&session, c).parent, Some(b));
    assert_eq!(branch(&session, c).fork_tick, 5);
    play(&mut session, &mut sim, 3);

    let export = session.export().unwrap();
    let mut resumed_sim = simulation();
    let mut resumed = Session::resume(&mut resumed_sim, export, Document::Scene).unwrap();
    resimulate_all(&mut resumed, &mut resumed_sim);
    assert_eq!(resumed.current_branch(), c);
    assert_eq!(hashes(&resumed), hashes(&session));
}
