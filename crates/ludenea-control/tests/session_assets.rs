use std::fs;
use std::path::Path;

use boxes::SpawnerStats;
use ludenea::app::prefab::{create_prefab, place_instance, save_prefab_checked};
use ludenea::cli::assets::project_assets;
use ludenea::control::session::Document;
use ludenea::control::{Resimulation, Session, SessionError};
use ludenea::ecs::{Asset, AssetId, Component};
use ludenea::prelude::*;
use ludenea_app::path::PathSegment;
use ludenea_app::{AssetTexts, EditError, EntityRef, SceneError, ValueKind};
use serde_json::json;

const SPAWNER_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/spawner.scene.ron"
);
const CRATE_PREFAB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/crate.prefab.ron"
);
/// The identifiers in `crate.prefab.ron.meta` and `spawner.ron.meta`.
const PREFAB: AssetId = AssetId(0x38b3_6be4_f8a1_40d6_95e5_cdac_24c8_2f90);
const STATS: AssetId = AssetId(0xbef6_17c9_e94d_41c4_b0e0_7882_849c_ef18);

fn simulation() -> Simulation {
    boxes::app().build().unwrap()
}

fn assets() -> AssetTexts {
    project_assets(Path::new(SPAWNER_SCENE), None)
        .unwrap()
        .texts
}

fn spawner_session() -> (Simulation, Session) {
    let mut sim = simulation();
    let scene = fs::read_to_string(SPAWNER_SCENE).unwrap();
    let session = Session::start(&mut sim, scene, assets(), Document::Scene).unwrap();
    (sim, session)
}

fn play(session: &mut Session, sim: &mut Simulation, ticks: u64) {
    for _ in 0..ticks {
        session.step(sim, None).unwrap();
    }
}

fn resimulate_all(session: &mut Session, sim: &mut Simulation) -> Vec<Resimulation> {
    let mut reports = Vec::new();
    loop {
        let report = session.resimulate(sim, 256).unwrap();
        let done = matches!(report, Resimulation::Done | Resimulation::Idle);
        reports.push(report);
        if done {
            return reports;
        }
    }
}

fn edits(session: &Session) -> Vec<TimedEdit> {
    EditLog::decode(&session.export().unwrap().edits)
        .unwrap()
        .edits
}

fn hashes(session: &Session) -> Vec<u64> {
    session.export().unwrap().hashes
}

fn component(sim: &Simulation, name: &str, value: serde_json::Value) -> Vec<u8> {
    sim.encode_json_value(ValueKind::Component, name, value)
        .unwrap()
}

fn pose(sim: &Simulation, x: f32, y: f32) -> Vec<u8> {
    component(
        sim,
        Pose::NAME,
        json!({ "position": { "x": x, "y": y }, "rotation": { "cos": 1.0, "sin": 0.0 } }),
    )
}

fn insert_pose(sim: &Simulation, address: Vec<u32>, x: f32) -> Edit {
    Edit::Insert {
        entity: EntityRef::Loaded(address),
        component: Pose::NAME.to_string(),
        value: pose(sim, x, 26.0),
    }
}

fn stats_value(sim: &Simulation, every_ticks: u32) -> Vec<u8> {
    let codec = sim.world().asset_codec(SpawnerStats::NAME).unwrap();
    (codec.encode)(&SpawnerStats {
        every_ticks,
        impulse: Vec2::new(4.0, 2.0),
    })
}

fn pose_at(sim: &Simulation, address: &[u32]) -> String {
    let entity = sim.entity_at_address(address).unwrap();
    sim.world().component_ron(entity, Pose::NAME).unwrap()
}

#[test]
fn edit_introducing_a_handle_records_load_asset_first() {
    let mut sim = simulation();
    let scene = boxes::generate_scene(10, 3);
    let mut session = Session::start(&mut sim, scene, assets(), Document::Scene).unwrap();
    play(&mut session, &mut sim, 10);
    let spawner = component(
        &sim,
        "boxes::Spawner",
        json!({ "prefab": PREFAB.to_string(), "stats": STATS.to_string() }),
    );
    let spawn = Edit::Spawn {
        components: vec![("boxes::Spawner".to_string(), spawner)],
    };
    session.edit(&mut sim, spawn.clone()).unwrap();

    let recorded = edits(&session);
    assert_eq!(recorded.len(), 3, "{recorded:?}");
    assert!(recorded.iter().all(|edit| edit.tick == 10));
    assert!(
        matches!(&recorded[0].edit, Edit::LoadAsset { id, asset, .. } if *id == PREFAB && asset == "ludenea::Prefab")
    );
    assert!(
        matches!(&recorded[1].edit, Edit::LoadAsset { id, asset, .. } if *id == STATS && asset == SpawnerStats::NAME)
    );
    assert_eq!(recorded[2].edit, spawn);
    // The spawner spawns at tick 60 from the loaded assets.
    play(&mut session, &mut sim, 60);

    session.seek(&mut sim, 10).unwrap();
    session.undo(&mut sim).unwrap();
    assert!(edits(&session).is_empty(), "undo takes the loads back too");
}

#[test]
fn tick_0_edit_on_instance_entity_becomes_override() {
    let (mut sim, mut session) = spawner_session();
    let original = session.scene().to_string();

    let edit = insert_pose(&sim, vec![6, 0], 7.0);
    session.edit(&mut sim, edit).unwrap();
    let scene = session.scene().to_string();
    assert!(
        scene.contains("(id: 6, prefab:"),
        "the instance stays: {scene}"
    );
    assert!(
        scene.contains(r#"component: "ludenea::Pose", path: [], value: (position:(x:7.0,y:26.0)"#),
        "{scene}"
    );
    assert!(pose_at(&sim, &[6, 0]).contains("x:7.0"));
    assert_eq!(pose_at(&sim, &[5, 0]), {
        let mut fresh = simulation();
        fresh.load_scene_with(&original, &assets()).unwrap();
        pose_at(&fresh, &[5, 0])
    });

    // A prefab component removed, then given back: a whole-component override.
    let remove = Edit::Remove {
        entity: EntityRef::Loaded(vec![5, 0]),
        component: "boxes::Crate".to_string(),
    };
    session.edit(&mut sim, remove).unwrap();
    assert!(
        session
            .scene()
            .contains("removed: [(entity: [0], component: \"boxes::Crate\")")
    );
    let crate_value = component(&sim, "boxes::Crate", json!(null));
    let insert = Edit::Insert {
        entity: EntityRef::Loaded(vec![5, 0]),
        component: "boxes::Crate".to_string(),
        value: crate_value,
    };
    session.edit(&mut sim, insert).unwrap();
    let scene = session.scene().to_string();
    assert!(!scene.contains("removed: [(entity"), "{scene}");
    assert!(
        scene.contains(r#"component: "boxes::Crate", path: [], value: ()"#),
        "{scene}"
    );

    let despawn = Edit::Despawn {
        entity: EntityRef::Loaded(vec![5, 0]),
    };
    assert!(matches!(
        session.edit(&mut sim, despawn),
        Err(SessionError::Scene(SceneError::InstanceEntity(address))) if address == [5, 0]
    ));

    session.undo(&mut sim).unwrap();
    session.undo(&mut sim).unwrap();
    session.undo(&mut sim).unwrap();
    assert_eq!(session.scene(), original);
}

#[test]
fn tick_0_despawn_keeps_load_addresses_of_later_entities() {
    let mut sim = simulation();
    let scene = boxes::generate_scene(30, 7);
    let mut session = Session::start(&mut sim, scene, AssetTexts::new(), Document::Scene).unwrap();
    let mut inputs = InputLog::new(1);
    for _ in 0..40 {
        inputs.record(&[boxes::BoxesInput::default()]);
    }
    session.load_inputs(&mut sim, inputs).unwrap();
    resimulate_all(&mut session, &mut sim);
    let original_20 = pose_at(&sim, &[20]);
    play(&mut session, &mut sim, 10);
    let edit = insert_pose(&sim, vec![20], -3.0);
    session.edit(&mut sim, edit).unwrap();

    session.seek(&mut sim, 0).unwrap();
    let despawn = Edit::Despawn {
        entity: EntityRef::Loaded(vec![3]),
    };
    session.edit(&mut sim, despawn).unwrap();
    assert!(!session.scene().contains("(id: 3,"));
    assert!(session.scene().contains("(id: 33,"), "no renumbering");
    assert_eq!(pose_at(&sim, &[20]), original_20, "[20] is the same entity");

    let reports = resimulate_all(&mut session, &mut sim);
    assert!(
        !reports
            .iter()
            .any(|report| matches!(report, Resimulation::EditDropped { .. })),
        "{reports:?}"
    );
    session.seek(&mut sim, 10).unwrap();
    assert!(pose_at(&sim, &[20]).contains("x:-3.0"));
}

#[test]
fn place_instance_appends_and_undoes() {
    let (mut sim, mut session) = spawner_session();
    let original = session.scene().to_string();
    let entities = sim.world().entities().count();

    let id = session.place_instance(&mut sim, PREFAB).unwrap();
    assert_eq!(id, 7);
    assert!(
        session
            .scene()
            .contains("(id: 7, prefab: \"38b36be4f8a140d695e5cdac24c82f90\"")
    );
    assert!(session.scene().contains("next_id: 8"));
    assert_eq!(sim.world().entities().count(), entities + 1);
    assert!(
        sim.instances()
            .iter()
            .any(|instance| instance.address == [7])
    );
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo scene edit")
    );

    session.undo(&mut sim).unwrap();
    assert_eq!(session.scene(), original);
    assert_eq!(sim.world().entities().count(), entities);
}

#[test]
fn replace_with_instance_moves_entities_into_a_prefab() {
    let (mut sim, mut session) = spawner_session();
    let original = session.scene().to_string();
    let wall = sim.entity_at_address(&[0]).unwrap();
    let prefab_text = create_prefab(&sim, &[wall]).unwrap();
    let wall_prefab = AssetId(0x1234);

    let id = session
        .replace_with_instance(&mut sim, &[wall], wall_prefab, prefab_text.clone())
        .unwrap();
    assert_eq!(id, 7);
    assert!(!session.scene().contains("(id: 0,"));
    assert_eq!(session.assets().get(&wall_prefab), Some(&prefab_text));
    // The new prefab's file holds its text.
    assert!(session.timeline().unsaved_assets.is_empty());
    let moved = sim.entity_at_address(&[7, 0]).unwrap();
    assert_eq!(
        sim.world().entity_components_ron(moved),
        simulation_with(&original)
            .world()
            .entity_components_ron(simulation_with(&original).entity_at_address(&[0]).unwrap())
    );
    let scene_entity = sim.entity_at_address(&[7, 0]).unwrap();
    assert!(matches!(
        session.replace_with_instance(&mut sim, &[scene_entity], wall_prefab, prefab_text),
        Err(SessionError::Scene(SceneError::InstanceEntity(_)))
    ));

    session.undo(&mut sim).unwrap();
    assert_eq!(session.scene(), original);
}

#[test]
fn create_prefab_after_playing_keeps_the_tick_0_values() {
    let mut sim = simulation();
    let scene = boxes::generate_scene(10, 3);
    let mut session = Session::start(&mut sim, scene, assets(), Document::Scene).unwrap();
    // The first crate, which falls once played.
    let crate_entity = sim.entity_at_address(&[4]).unwrap();
    let at_tick_0 = create_prefab(&sim, &[crate_entity]).unwrap();
    play(&mut session, &mut sim, 120);
    let (hash, played) = (sim.hash(), create_prefab(&sim, &[crate_entity]).unwrap());
    assert_ne!(played, at_tick_0);

    let text = session.create_prefab(&mut sim, &[crate_entity]).unwrap();

    assert_eq!(text, at_tick_0);
    assert_eq!(session.timeline().current, 120);
    assert_eq!(sim.hash(), hash);
    // An entity spawned after tick 0 is not in the scene.
    let spawned = session
        .edit(
            &mut sim,
            Edit::Spawn {
                components: Vec::new(),
            },
        )
        .unwrap()
        .unwrap();
    let error = session.create_prefab(&mut sim, &[spawned]).unwrap_err();
    assert!(
        matches!(&error, SessionError::NotInScene(entity) if *entity == spawned),
        "{error:?}"
    );
    assert_eq!(session.timeline().current, 120);
    // An entity that never was is no such entity.
    let unknown: Entity =
        serde_json::from_value(json!({ "index": 4000, "generation": 0 })).unwrap();
    let error = session.create_prefab(&mut sim, &[unknown]).unwrap_err();
    assert!(
        matches!(&error, SessionError::Edit(EditError::NoSuchEntity(entity)) if *entity == unknown),
        "{error:?}"
    );
}

fn field(name: &str) -> PathSegment {
    PathSegment::Field {
        name: name.to_string(),
    }
}

/// The spawner scene with instance 6's override list emptied.
fn without_instance_6_overrides(scene: &str) -> String {
    let start = scene.find("(id: 6,").unwrap();
    let end = start + scene[start..].find('\n').unwrap();
    let instance_6 = format!("(id: 6, prefab: \"{PREFAB}\", overrides: []),");
    [&scene[..start], &instance_6, &scene[end..]].concat()
}

#[test]
fn apply_to_prefab_moves_the_override_into_the_prefab() {
    let (mut sim, mut session) = spawner_session();
    let (scene, prefab) = (
        session.scene().to_string(),
        session.assets()[&PREFAB].clone(),
    );
    play(&mut session, &mut sim, 120);
    let x = [field("position"), field("x")];

    // Instance 6 overrides the position's `x` with 5.0.
    let applied = session
        .apply_to_prefab(&mut sim, &[6], &[0], Pose::NAME, &x)
        .unwrap();

    let expected = prefab.replace("position:(x:0.0,y:26.0)", "position:(x:5.0,y:26.0)");
    assert_eq!(applied, expected);
    assert_eq!(session.assets()[&PREFAB], expected);
    assert_eq!(session.scene(), without_instance_6_overrides(&scene));
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo apply to prefab")
    );
    assert_eq!(session.timeline().unsaved_assets, [PREFAB]);
    resimulate_all(&mut session, &mut sim);
    assert_eq!(session.timeline().current, 120);
    session.seek(&mut sim, 0).unwrap();
    // Every instance follows the prefab.
    assert!(pose_at(&sim, &[5, 0]).contains("x:5.0"));
    // Nothing is left to apply.
    assert!(matches!(
        session.apply_to_prefab(&mut sim, &[6], &[0], Pose::NAME, &x),
        Err(SessionError::Scene(SceneError::MissingTarget(_)))
    ));

    // One undo takes both back, and the history keeps the entry across a resume.
    let mut session =
        Session::resume(&mut sim, session.export().unwrap(), Document::Scene).unwrap();
    assert_eq!(session.timeline().unsaved_assets, [PREFAB]);
    resimulate_all(&mut session, &mut sim);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.scene(), scene);
    assert_eq!(session.assets()[&PREFAB], prefab);
    assert!(pose_at(&sim, &[5, 0]).contains("x:0.0"));
    assert!(session.timeline().unsaved_assets.is_empty());
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    assert_eq!(session.scene(), without_instance_6_overrides(&scene));
    assert_eq!(session.assets()[&PREFAB], expected);
    assert_eq!(session.timeline().unsaved_assets, [PREFAB]);
}

#[test]
fn applying_a_whole_component_moves_each_of_its_overrides() {
    let (mut sim, mut session) = spawner_session();
    let (scene, prefab) = (
        session.scene().to_string(),
        session.assets()[&PREFAB].clone(),
    );
    let y = [field("position"), field("y")];
    session
        .set_override(&mut sim, &[6], &[0], Pose::NAME, &y, "3.0")
        .unwrap();

    session
        .apply_to_prefab(&mut sim, &[6], &[0], Pose::NAME, &[])
        .unwrap();

    assert_eq!(
        session.assets()[&PREFAB],
        prefab.replace("position:(x:0.0,y:26.0)", "position:(x:5.0,y:3.0)")
    );
    assert_eq!(session.scene(), without_instance_6_overrides(&scene));
}

fn simulation_with(scene: &str) -> Simulation {
    let mut sim = simulation();
    sim.load_scene_with(scene, &assets()).unwrap();
    sim
}

fn recorded(ticks: u64) -> InputLog {
    let mut log = InputLog::new(1);
    for _ in 0..ticks {
        log.record(&[boxes::BoxesInput::default()]);
    }
    log
}

#[test]
fn asset_edit_at_tick_0_replays_session_and_undoes() {
    let (mut sim, mut session) = spawner_session();
    session.load_inputs(&mut sim, recorded(200)).unwrap();
    resimulate_all(&mut session, &mut sim);
    play(&mut session, &mut sim, 150);
    let original_text = session.assets()[&STATS].clone();
    let original = hashes(&session);

    session.seek(&mut sim, 0).unwrap();
    let edit = Edit::SetAsset {
        id: STATS,
        value: stats_value(&sim, 30),
    };
    session.edit(&mut sim, edit).unwrap();
    assert!(session.assets()[&STATS].contains("every_ticks:30"));
    assert!(session.timeline().resimulating);
    assert!(
        !session.timeline().scene_dirty,
        "the scene itself is unchanged"
    );
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo asset edit")
    );
    let reports = resimulate_all(&mut session, &mut sim);
    assert!(
        reports
            .iter()
            .any(|report| matches!(report, Resimulation::Diverged { .. })),
        "{reports:?}"
    );
    assert_ne!(hashes(&session)[..original.len()], original);

    session.undo(&mut sim).unwrap();
    assert_eq!(session.assets()[&STATS], original_text);
    resimulate_all(&mut session, &mut sim);
    assert_eq!(hashes(&session)[..original.len()], original);
    assert_eq!(
        session.timeline().redo_label.as_deref(),
        Some("Redo asset edit")
    );
}

/// Sets the spawner stats' `every_ticks` at the current tick.
fn edit_stats(session: &mut Session, sim: &mut Simulation, every_ticks: u32) {
    let edit = Edit::SetAsset {
        id: STATS,
        value: stats_value(sim, every_ticks),
    };
    session.edit(sim, edit).unwrap();
}

#[test]
fn an_asset_stays_unsaved_until_its_written_text_is_marked_saved() {
    let (mut sim, mut session) = spawner_session();
    edit_stats(&mut session, &mut sim, 30);
    assert_eq!(session.timeline().unsaved_assets, [STATS]);
    let written = session.save_asset(STATS).unwrap();
    // The text is not on disk yet: a failed write leaves the edit unsaved.
    assert_eq!(session.timeline().unsaved_assets, [STATS]);

    // An edit made while the text is written stays unsaved.
    resimulate_all(&mut session, &mut sim);
    edit_stats(&mut session, &mut sim, 40);
    session.mark_asset_saved(STATS, &written).unwrap();
    assert_eq!(session.timeline().unsaved_assets, [STATS]);

    let latest = session.save_asset(STATS).unwrap();
    session.mark_asset_saved(STATS, &latest).unwrap();
    assert!(session.timeline().unsaved_assets.is_empty());
    // Undo goes back to a text the file no longer holds, redo to the one it holds.
    resimulate_all(&mut session, &mut sim);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.timeline().unsaved_assets, [STATS]);
    resimulate_all(&mut session, &mut sim);
    session.redo(&mut sim).unwrap();
    assert!(session.timeline().unsaved_assets.is_empty());

    assert_eq!(session.mark_asset_saved(AssetId(7), &latest), None);
}

#[test]
fn a_mark_made_after_a_later_edit_still_records_the_written_asset_text() {
    let (mut sim, mut session) = spawner_session();
    let file = session.assets()[&STATS].clone();
    edit_stats(&mut session, &mut sim, 30);
    let edited = session.assets()[&STATS].clone();
    let written = session.save_asset(STATS).unwrap();
    resimulate_all(&mut session, &mut sim);
    edit_stats(&mut session, &mut sim, 40);
    session.mark_asset_saved(STATS, &written).unwrap();
    assert_eq!(session.timeline().unsaved_assets, [STATS]);
    resimulate_all(&mut session, &mut sim);
    assert_eq!(session.export().unwrap().saved_assets, [(STATS, edited)]);

    // Back to the written text, which the file holds.
    session.undo(&mut sim).unwrap();
    assert!(session.timeline().unsaved_assets.is_empty());

    // Back to the text loaded before the save, which the file no longer holds.
    resimulate_all(&mut session, &mut sim);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.assets()[&STATS], file);
    assert_eq!(session.timeline().unsaved_assets, [STATS]);
}

#[test]
fn unsaved_assets_stay_unsaved_after_a_resume() {
    let (mut sim, mut session) = spawner_session();
    let file = session.assets()[&STATS].clone();
    edit_stats(&mut session, &mut sim, 30);
    resimulate_all(&mut session, &mut sim);
    let export = session.export().unwrap();
    assert_eq!(export.saved_assets, [(STATS, file.clone())]);

    let mut resumed = Session::resume(&mut sim, export.clone(), Document::Scene).unwrap();
    assert_eq!(resumed.timeline().unsaved_assets, [STATS]);
    resimulate_all(&mut resumed, &mut sim);
    resumed.undo(&mut sim).unwrap();
    assert!(
        resumed.timeline().unsaved_assets.is_empty(),
        "back to the saved text"
    );
    assert!(resumed.export().unwrap().saved_assets.is_empty());

    // A saved text equal to the session's, or of an asset the session does not hold, marks
    // nothing.
    let mut export = export;
    let edited = session.assets()[&STATS].clone();
    export.saved_assets = vec![(STATS, edited), (AssetId(7), file)];
    let resumed = Session::resume(&mut sim, export, Document::Scene).unwrap();
    assert!(resumed.timeline().unsaved_assets.is_empty());
}

#[test]
fn a_new_scene_version_keeps_the_text_of_an_unsaved_asset() {
    let (mut sim, mut session) = spawner_session();
    let scene = session.scene().to_string();
    edit_stats(&mut session, &mut sim, 30);
    resimulate_all(&mut session, &mut sim);
    let edited = session.assets()[&STATS].clone();

    session
        .replace_scene(&mut sim, scene.clone(), assets())
        .unwrap();
    assert_eq!(session.assets()[&STATS], edited);
    assert_eq!(session.timeline().unsaved_assets, [STATS]);

    // Its file now holds the session's text.
    let mut written = assets();
    written.insert(STATS, edited.clone());
    resimulate_all(&mut session, &mut sim);
    session.replace_scene(&mut sim, scene, written).unwrap();
    assert_eq!(session.assets()[&STATS], edited);
    assert!(session.timeline().unsaved_assets.is_empty());
}

#[test]
fn asset_edit_past_tick_0_is_recorded() {
    let (mut sim, mut session) = spawner_session();
    play(&mut session, &mut sim, 50);
    let original_text = session.assets()[&STATS].clone();
    let edit = Edit::SetAsset {
        id: STATS,
        value: stats_value(&sim, 30),
    };
    session.edit(&mut sim, edit.clone()).unwrap();
    assert_eq!(edits(&session), [TimedEdit { tick: 50, edit }]);
    assert_eq!(session.assets()[&STATS], original_text);
    assert!(
        sim.world()
            .asset_ron(STATS)
            .unwrap()
            .contains("every_ticks:30")
    );
}

#[test]
fn apply_to_asset_copies_value() {
    let (mut sim, mut session) = spawner_session();
    play(&mut session, &mut sim, 50);
    let edit = Edit::SetAsset {
        id: STATS,
        value: stats_value(&sim, 30),
    };
    session.edit(&mut sim, edit).unwrap();

    session.apply_to_asset(&mut sim, STATS).unwrap();
    assert!(session.assets()[&STATS].contains("every_ticks:30"));
    let saved = session.save_asset(STATS).unwrap();
    assert!(saved.starts_with("(every_ticks:30"), "{saved}");
    resimulate_all(&mut session, &mut sim);
    assert_eq!(session.timeline().current, 50);
    assert_eq!(
        session.timeline().undo_label.as_deref(),
        Some("Undo asset edit")
    );

    assert!(matches!(
        session.apply_to_asset(&mut sim, PREFAB),
        Err(SessionError::NotDataAsset(id)) if id == PREFAB
    ));
}

fn prefab_session() -> (Simulation, Session) {
    let mut sim = simulation();
    let prefab = fs::read_to_string(CRATE_PREFAB).unwrap();
    let session =
        Session::start(&mut sim, prefab, assets(), Document::Prefab { id: PREFAB }).unwrap();
    (sim, session)
}

#[test]
fn prefab_document_saves_prefab_text() {
    let (mut sim, mut session) = prefab_session();
    assert_eq!(sim.world().entities().count(), 1);

    let edit = insert_pose(&sim, vec![0], 2.5);
    session.edit(&mut sim, edit).unwrap();
    let text = session.scene().to_string();
    assert!(text.starts_with("Prefab("), "{text}");
    assert!(!text.contains("resources"), "{text}");
    assert!(text.contains("x:2.5"), "{text}");
    save_prefab_checked(PREFAB, &text, &assets()).unwrap();
    assert!(pose_at(&sim, &[0]).contains("x:2.5"));
}

#[test]
fn placing_a_prefab_inside_itself_is_refused() {
    let (mut sim, mut session) = prefab_session();
    let text = session.scene().to_string();

    let error = session.place_instance(&mut sim, PREFAB).unwrap_err();

    assert!(
        matches!(&error, SessionError::Scene(SceneError::PrefabCycle(chain))
            if *chain == [PREFAB.to_string(), PREFAB.to_string()]),
        "{error}"
    );
    assert_eq!(session.scene(), text);
    assert!(!session.timeline().can_undo);
}

#[test]
fn prefab_document_refuses_resources() {
    let (mut sim, mut session) = prefab_session();
    let value = sim
        .encode_json_value(
            ValueKind::Resource,
            "boxes::ContactStats",
            json!({ "begins": 1, "ends": 0 }),
        )
        .unwrap();
    let edit = Edit::SetResource {
        resource: "boxes::ContactStats".to_string(),
        value,
    };
    assert!(matches!(
        session.edit(&mut sim, edit),
        Err(SessionError::PrefabHasNoResources)
    ));
}

#[test]
fn undo_keeps_loads_a_later_edit_at_the_tick_needs() {
    let mut sim = simulation();
    let scene = boxes::generate_scene(10, 3);
    let mut session = Session::start(&mut sim, scene, assets(), Document::Scene).unwrap();
    play(&mut session, &mut sim, 10);
    let spawner = component(
        &sim,
        "boxes::Spawner",
        json!({ "prefab": PREFAB.to_string(), "stats": STATS.to_string() }),
    );
    let spawn = Edit::Spawn {
        components: vec![("boxes::Spawner".to_string(), spawner.clone())],
    };
    session.edit(&mut sim, spawn).unwrap();
    play(&mut session, &mut sim, 5);
    // A second spawner at the same tick forks a branch and needs no load of its own.
    session.seek(&mut sim, 10).unwrap();
    let insert = Edit::Insert {
        entity: EntityRef::Loaded(vec![0]),
        component: "boxes::Spawner".to_string(),
        value: spawner,
    };
    session.edit(&mut sim, insert.clone()).unwrap();
    assert_eq!(edits(&session).len(), 4);
    // New inputs flatten the tree: the fork's undo entry goes, the spawn's stays.
    let inputs = InputLog::decode(&session.export().unwrap().inputs).unwrap();
    session.load_inputs(&mut sim, inputs).unwrap();
    resimulate_all(&mut session, &mut sim);

    session.undo(&mut sim).unwrap();
    let left: Vec<Edit> = edits(&session).into_iter().map(|e| e.edit).collect();
    assert_eq!(left.len(), 3, "{left:?}");
    assert!(matches!(left[0], Edit::LoadAsset { id, .. } if id == PREFAB));
    assert!(matches!(left[1], Edit::LoadAsset { id, .. } if id == STATS));
    assert_eq!(left[2], insert);
    // The remaining spawner spawns at tick 60 from the kept assets.
    play(&mut session, &mut sim, 60);
}

#[test]
fn edit_at_a_load_address_despawned_at_tick_0_is_dropped() {
    let mut sim = simulation();
    let scene = boxes::generate_scene(30, 7);
    let mut session = Session::start(&mut sim, scene, AssetTexts::new(), Document::Scene).unwrap();
    let mut inputs = InputLog::new(1);
    for _ in 0..40 {
        inputs.record(&[boxes::BoxesInput::default()]);
    }
    session.load_inputs(&mut sim, inputs).unwrap();
    resimulate_all(&mut session, &mut sim);
    play(&mut session, &mut sim, 10);
    let edit = insert_pose(&sim, vec![20], -3.0);
    session.edit(&mut sim, edit).unwrap();

    session.seek(&mut sim, 0).unwrap();
    let despawn = Edit::Despawn {
        entity: EntityRef::Loaded(vec![20]),
    };
    session.edit(&mut sim, despawn).unwrap();
    let reports = resimulate_all(&mut session, &mut sim);
    assert!(
        reports
            .iter()
            .any(|report| matches!(report, Resimulation::EditDropped { tick: 10, .. })),
        "{reports:?}"
    );
}

/// Whether `panic_once` panics the next time it runs.
static ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn panic_once() {
    if ARMED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        panic!("boom once");
    }
}

fn panicking_app() -> App {
    let mut app = boxes::app();
    app.system(Sim, panic_once);
    app
}

#[test]
fn undo_whose_replay_panics_still_takes_the_edit_back() {
    let mut sim = panicking_app().build().unwrap();
    let scene = boxes::generate_scene(10, 3);
    let mut session = Session::start(&mut sim, scene, assets(), Document::Scene).unwrap();
    play(&mut session, &mut sim, 10);
    let spawner = component(
        &sim,
        "boxes::Spawner",
        json!({ "prefab": PREFAB.to_string(), "stats": STATS.to_string() }),
    );
    let spawn = Edit::Spawn {
        components: vec![("boxes::Spawner".to_string(), spawner)],
    };
    session.edit(&mut sim, spawn).unwrap();
    let recorded = edits(&session);

    // The replay before the loads panics once: the undo still completes, keeping them.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    ARMED.store(true, std::sync::atomic::Ordering::Relaxed);
    let undone = session.undo(&mut sim);
    std::panic::set_hook(hook);
    undone.unwrap();
    assert_eq!(edits(&session), recorded[..2]);
    assert!(session.timeline().can_redo);
    session.redo(&mut sim).unwrap();
    assert_eq!(edits(&session), recorded);
}

#[test]
fn warnings_follow_every_scene_load() {
    // The override of instance 6 targets an entity the prefab does not have.
    let scene = fs::read_to_string(SPAWNER_SCENE)
        .unwrap()
        .replace("(entity: [0], component", "(entity: [9], component");
    let mut sim = simulation();
    let mut session = Session::start(&mut sim, scene, assets(), Document::Scene).unwrap();
    assert_eq!(session.warnings().len(), 1);
    session
        .revert_override(&mut sim, &[6], &[9], Pose::NAME, &[])
        .unwrap();
    assert_eq!(session.warnings(), [] as [String; 0]);
    session.undo(&mut sim).unwrap();
    assert_eq!(session.warnings().len(), 1);
}

#[test]
fn tick_0_insert_on_an_added_component_updates_the_added_entry() {
    // A crate prefab without a pose: the instance adds it.
    let mut assets = assets();
    let crate_text = assets.get(&PREFAB).unwrap().clone();
    let start = crate_text.find(r#", "ludenea::Pose""#).unwrap();
    let end = start + crate_text[start..].find(")}").unwrap() + 1;
    let without_pose = format!("{}{}", &crate_text[..start], &crate_text[end..]);
    assets.insert(PREFAB, without_pose);
    let mut sim = simulation();
    let scene = fs::read_to_string(SPAWNER_SCENE).unwrap();
    let mut session = Session::start(&mut sim, scene, assets, Document::Scene).unwrap();

    for x in [7.0, 8.0] {
        let edit = insert_pose(&sim, vec![5, 0], x);
        session.edit(&mut sim, edit).unwrap();
    }
    let scene = session.scene();
    assert_eq!(
        scene.matches(r#"component: "ludenea::Pose""#).count(),
        2,
        "{scene}"
    );
    assert!(
        scene.contains(
            r#"added: [(entity: [0], component: "ludenea::Pose", value: (position:(x:8.0"#
        ),
        "{scene}"
    );
    assert!(pose_at(&sim, &[5, 0]).contains("x:8.0"));
}

/// A prefab holding one instance of the crate prefab, with the identifier `OUTER`.
const OUTER: AssetId = AssetId(0x0007);

fn outer_prefab() -> String {
    format!(
        "Prefab(\n    version: 1,\n    next_id: 1,\n    entities: [],\n    instances: [\n    (id: 0, prefab: \"{PREFAB}\"),\n    ],\n)\n"
    )
}

#[test]
fn tick_0_edit_on_a_nested_instance_entity_overrides_the_outer_instance() {
    let mut assets = assets();
    assets.insert(OUTER, outer_prefab());
    let mut sim = simulation();
    let scene = fs::read_to_string(SPAWNER_SCENE).unwrap();
    let mut session = Session::start(&mut sim, scene, assets, Document::Scene).unwrap();
    let outer = session.place_instance(&mut sim, OUTER).unwrap();

    let edit = insert_pose(&sim, vec![outer, 0, 0], 7.0);
    session.edit(&mut sim, edit).unwrap();
    let scene = session.scene();
    assert!(
        scene.contains(
            r#"(entity: [0, 0], component: "ludenea::Pose", path: [], value: (position:(x:7.0"#
        ),
        "{scene}"
    );
    assert!(pose_at(&sim, &[outer, 0, 0]).contains("x:7.0"));
    assert!(!pose_at(&sim, &[5, 0]).contains("x:7.0"));
}

#[test]
fn tick_0_edit_on_an_instance_entity_in_a_prefab_document_overrides_it() {
    let mut sim = simulation();
    let document = Document::Prefab { id: OUTER };
    let mut session = Session::start(&mut sim, outer_prefab(), assets(), document).unwrap();

    let edit = insert_pose(&sim, vec![0, 0], 2.5);
    session.edit(&mut sim, edit).unwrap();
    let text = session.scene().to_string();
    assert!(text.starts_with("Prefab("), "{text}");
    assert!(
        text.contains(
            r#"(entity: [0], component: "ludenea::Pose", path: [], value: (position:(x:2.5"#
        ),
        "{text}"
    );
    let mut assets = assets();
    assets.insert(OUTER, text.clone());
    save_prefab_checked(OUTER, &text, &assets).unwrap();
    assert!(pose_at(&sim, &[0, 0]).contains("x:2.5"));
}

#[test]
fn a_prefab_document_reaching_itself_through_another_prefab_is_refused() {
    // The outer prefab holds an instance of the crate prefab, opened as the document.
    let mut assets = assets();
    assets.insert(OUTER, outer_prefab());
    let text = fs::read_to_string(CRATE_PREFAB).unwrap();
    let document = Document::Prefab { id: PREFAB };
    let mut sim = simulation();
    let mut session = Session::start(&mut sim, text.clone(), assets.clone(), document).unwrap();

    let error = session.place_instance(&mut sim, OUTER).unwrap_err();

    assert!(
        matches!(&error, SessionError::Scene(SceneError::PrefabCycle(chain))
            if *chain == [OUTER.to_string(), PREFAB.to_string(), OUTER.to_string()]),
        "{error}"
    );
    assert_eq!(session.scene(), text);
    // A prefab file that already holds such an instance does not load as its document.
    let (cyclic, _) = place_instance(&text, OUTER).unwrap();
    let Err(error) = Session::start(&mut sim, cyclic, assets, document) else {
        panic!("a prefab document holding itself loaded");
    };
    assert!(
        matches!(error, SessionError::Scene(SceneError::PrefabCycle(_))),
        "{error}"
    );
}

/// Game code writing a rotation that is not unit length, which loading refuses, from tick 10.
fn zero_rotation_from_10(time: Res<Time>, mut poses: Query<&mut Pose, Without<Body>>) {
    if time.tick >= 10 {
        for pose in poses.iter_mut() {
            pose.rotation = Rot { cos: 0.0, sin: 0.0 };
        }
    }
}

#[test]
fn an_edit_whose_loads_are_taken_back_where_the_state_does_not_restore_replays_the_tick() {
    let mut loader = simulation();
    loader.load_scene(&boxes::generate_scene(10, 3)).unwrap();
    let marker = loader.world_mut().spawn();
    loader.world_mut().insert(marker, Pose::default()).unwrap();
    let mut app = boxes::app();
    app.system(Sim, zero_rotation_from_10);
    let mut sim = app.build().unwrap();
    let mut session = Session::start(
        &mut sim,
        loader.save_scene().unwrap(),
        assets(),
        Document::Scene,
    )
    .unwrap();
    play(&mut session, &mut sim, 12);
    let before = sim.hash();
    let spawner = component(
        &sim,
        "boxes::Spawner",
        json!({ "prefab": PREFAB.to_string(), "stats": STATS.to_string() }),
    );
    // The loads apply, then the edit fails: no entity has that address.
    let insert = Edit::Insert {
        entity: EntityRef::Loaded(vec![999]),
        component: "boxes::Spawner".to_string(),
        value: spawner,
    };

    let error = session.edit(&mut sim, insert).unwrap_err();

    assert!(matches!(error, SessionError::Edit(_)), "{error:?}");
    assert!(edits(&session).is_empty());
    assert_eq!(session.timeline().current, 12);
    assert_eq!(sim.tick(), 12);
    assert_eq!(sim.hash(), before);
}

#[test]
fn a_redo_after_new_asset_texts_resimulates_the_run_it_brings_back() {
    let (mut sim, mut session) = spawner_session();
    let scene = session.scene().to_string();
    play(&mut session, &mut sim, 100);
    let value = sim
        .encode_json_value(
            ValueKind::Resource,
            "boxes::ContactStats",
            json!({ "begins": 1, "ends": 0 }),
        )
        .unwrap();
    let resource = "boxes::ContactStats".to_string();
    session
        .edit(&mut sim, Edit::SetResource { resource, value })
        .unwrap();
    session.undo(&mut sim).unwrap();
    // The same scene with a new text of the spawner's asset: no undo entry, so the redo
    // stays, and the run it brings back was recorded with the previous text.
    let mut changed = assets();
    let text = changed[&STATS].replace("every_ticks: 60", "every_ticks: 20");
    assert_ne!(text, changed[&STATS]);
    changed.insert(STATS, text);
    session.replace_scene(&mut sim, scene, changed).unwrap();
    resimulate_all(&mut session, &mut sim);

    session.redo(&mut sim).unwrap();
    resimulate_all(&mut session, &mut sim);

    let resumed = Session::resume(&mut sim, session.export().unwrap(), Document::Scene);
    let reports = resimulate_all(&mut resumed.unwrap(), &mut sim);
    assert!(
        !reports
            .iter()
            .any(|report| matches!(report, Resimulation::Diverged { .. })),
        "{reports:?}"
    );
}
