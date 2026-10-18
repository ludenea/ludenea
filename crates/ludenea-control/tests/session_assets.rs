use std::fs;
use std::path::Path;

use boxes::SpawnerStats;
use ludenea::app::prefab::{create_prefab, save_prefab_checked};
use ludenea::cli::assets::project_assets;
use ludenea::control::session::Document;
use ludenea::control::{Resimulation, Session, SessionError};
use ludenea::ecs::{Asset, AssetId, Component};
use ludenea::prelude::*;
use ludenea_app::{AssetTexts, EntityRef, SceneError, ValueKind};
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
    let session = Session::start(&mut sim, prefab, assets(), Document::Prefab).unwrap();
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
