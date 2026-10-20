use std::collections::BTreeSet;

use ludenea_app::edit::{
    EDIT_LOG_VERSION, Edit, EditError, EditLog, EntityRef, TimedEdit, ValueKind,
};
use ludenea_app::{App, AssetTexts, SimError, Simulation};
use ludenea_ecs::{Asset, AssetId, Component, Entity, Handle, Prefab, PrefabEntity};
use ludenea_math::{Rot, Vec2};
use ludenea_physics::{Body, Collider, ContactCache, PhysicsPlugin, Polygon, Pose, Shape};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Tag(u32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Weight(f32);

#[derive(Asset, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Stats {
    damage: u32,
    next: Option<Handle<Stats>>,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Armed(Handle<Stats>);

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin)
        .component::<Tag>()
        .component::<Weight>()
        .component::<Armed>()
        .asset::<Stats>();
    app.build().unwrap()
}

fn tag(value: u32) -> Vec<u8> {
    postcard::to_allocvec(&Tag(value)).unwrap()
}

fn spawn_tagged(simulation: &mut Simulation, value: u32) -> Entity {
    let edit = Edit::Spawn {
        components: vec![("edit::Tag".to_string(), tag(value))],
    };
    simulation.apply_edit(&edit).unwrap().unwrap()
}

fn tag_of(simulation: &Simulation, entity: Entity) -> Option<u32> {
    simulation.world().get::<Tag>(entity).map(|tag| tag.0)
}

#[test]
fn insert_replaces_existing_component() {
    let mut simulation = simulation();
    let entity = spawn_tagged(&mut simulation, 1);
    let edit = Edit::Insert {
        entity: EntityRef::Runtime(entity),
        component: "edit::Tag".to_string(),
        value: tag(2),
    };
    assert_eq!(simulation.apply_edit(&edit).unwrap(), None);
    assert_eq!(tag_of(&simulation, entity), Some(2));
}

#[test]
fn spawn_returns_next_free_entity_deterministically() {
    let mut first = simulation();
    let mut second = simulation();
    let a = spawn_tagged(&mut first, 1);
    let b = spawn_tagged(&mut first, 2);
    assert_eq!((a.index(), b.index()), (0, 1));
    assert_eq!(spawn_tagged(&mut second, 1), a);
    assert_eq!(spawn_tagged(&mut second, 2), b);

    first
        .apply_edit(&Edit::Despawn {
            entity: EntityRef::Runtime(a),
        })
        .unwrap();
    let reused = spawn_tagged(&mut first, 3);
    assert_eq!((reused.index(), reused.generation()), (0, 1));
    assert_eq!(tag_of(&first, reused), Some(3));
    assert_eq!(tag_of(&first, a), None);
}

#[test]
fn removing_a_component_drops_it() {
    let mut simulation = simulation();
    let entity = spawn_tagged(&mut simulation, 1);
    let edit = Edit::Remove {
        entity: EntityRef::Runtime(entity),
        component: "edit::Tag".to_string(),
    };
    simulation.apply_edit(&edit).unwrap();
    assert_eq!(tag_of(&simulation, entity), None);
    assert!(simulation.world().is_alive(entity));
}

#[test]
fn despawn_of_dead_entity_is_no_such_entity() {
    let mut simulation = simulation();
    let entity = spawn_tagged(&mut simulation, 1);
    simulation
        .apply_edit(&Edit::Despawn {
            entity: EntityRef::Runtime(entity),
        })
        .unwrap();
    let error = simulation
        .apply_edit(&Edit::Despawn {
            entity: EntityRef::Runtime(entity),
        })
        .unwrap_err();
    assert!(matches!(error, EditError::NoSuchEntity(dead) if dead == entity));
}

fn invalid_pose_bytes() -> Vec<u8> {
    let pose = Pose {
        position: Vec2::ZERO,
        rotation: Rot { cos: 2.0, sin: 0.0 },
    };
    postcard::to_allocvec(&pose).unwrap()
}

#[test]
fn invalid_pose_rotation_is_rejected_and_world_unchanged() {
    let mut simulation = simulation();
    let entity = spawn_tagged(&mut simulation, 1);
    let before = simulation.hash();
    let edit = Edit::Spawn {
        components: vec![
            ("edit::Tag".to_string(), tag(9)),
            ("ludenea::Pose".to_string(), invalid_pose_bytes()),
        ],
    };
    let error = simulation.apply_edit(&edit).unwrap_err();
    assert!(matches!(error, EditError::InvalidValue { ref name, .. } if name == "ludenea::Pose"));
    assert_eq!(simulation.hash(), before);
    assert_eq!(
        simulation.world().entities().collect::<Vec<_>>(),
        vec![entity]
    );

    let edit = Edit::Insert {
        entity: EntityRef::Runtime(entity),
        component: "ludenea::Pose".to_string(),
        value: invalid_pose_bytes(),
    };
    assert!(simulation.apply_edit(&edit).is_err());
    assert_eq!(simulation.hash(), before);

    let json =
        json!({ "position": { "x": 0.0, "y": 0.0 }, "rotation": { "cos": 2.0, "sin": 0.0 } });
    let error = simulation
        .encode_json_value(ValueKind::Component, "ludenea::Pose", json)
        .unwrap_err();
    assert!(matches!(error, EditError::InvalidValue { .. }));
}

#[test]
fn unknown_component_name_is_rejected() {
    let mut simulation = simulation();
    let entity = spawn_tagged(&mut simulation, 1);
    let before = simulation.hash();
    for edit in [
        Edit::Insert {
            entity: EntityRef::Runtime(entity),
            component: "edit::Nope".to_string(),
            value: tag(1),
        },
        Edit::Remove {
            entity: EntityRef::Runtime(entity),
            component: "edit::Nope".to_string(),
        },
        Edit::Spawn {
            components: vec![("edit::Nope".to_string(), tag(1))],
        },
    ] {
        let error = simulation.apply_edit(&edit).unwrap_err();
        assert!(matches!(error, EditError::UnknownComponent(ref name) if name == "edit::Nope"));
    }
    let error = simulation
        .apply_edit(&Edit::SetResource {
            resource: "edit::Nope".to_string(),
            value: vec![],
        })
        .unwrap_err();
    assert!(matches!(error, EditError::UnknownResource(_)));
    assert_eq!(simulation.hash(), before);
}

#[test]
fn time_resource_is_read_only() {
    let mut simulation = simulation();
    let value = simulation.world().resource_codec("ludenea::Time").unwrap();
    let bytes = (value.encode)(&*simulation.world().resource::<ludenea_app::Time>().unwrap());
    let error = simulation
        .apply_edit(&Edit::SetResource {
            resource: "ludenea::Time".to_string(),
            value: bytes,
        })
        .unwrap_err();
    assert!(matches!(error, EditError::ReadOnly(ref name) if name == "ludenea::Time"));
}

#[test]
fn set_resource_replaces_the_value() {
    let mut simulation = simulation();
    let mut settings = *simulation
        .world()
        .resource::<ludenea_physics::PhysicsSettings>()
        .unwrap();
    settings.gravity = Vec2::new(0.0, -1.0);
    let value = postcard::to_allocvec(&settings).unwrap();
    let edit = Edit::SetResource {
        resource: "ludenea::PhysicsSettings".to_string(),
        value,
    };
    simulation.apply_edit(&edit).unwrap();
    let gravity = simulation
        .world()
        .resource::<ludenea_physics::PhysicsSettings>()
        .unwrap()
        .gravity;
    assert_eq!(gravity, Vec2::new(0.0, -1.0));
}

#[test]
fn edit_log_round_trips_and_rejects_other_versions() {
    let mut log = EditLog::new();
    log.edits.push(TimedEdit {
        tick: 3,
        edit: Edit::Spawn {
            components: vec![("edit::Tag".to_string(), tag(1))],
        },
    });
    assert_eq!(EditLog::decode(&log.encode()).unwrap(), log);

    log.version = 9;
    let error = EditLog::decode(&log.encode()).unwrap_err();
    assert!(matches!(
        error,
        SimError::EditLogVersion {
            found: 9,
            expected: EDIT_LOG_VERSION
        }
    ));

    let mut bytes = EditLog::new().encode();
    bytes.push(0);
    assert!(EditLog::decode(&bytes).is_err());
}

#[test]
fn edit_log_at_returns_edits_of_one_tick_in_record_order() {
    let edit = |tick: u64, value: u32| TimedEdit {
        tick,
        edit: Edit::Spawn {
            components: vec![("edit::Tag".to_string(), tag(value))],
        },
    };
    let mut log = EditLog::new();
    assert_eq!(log.last_tick(), None);
    log.edits = vec![edit(1, 10), edit(1, 11), edit(4, 12)];
    assert_eq!(log.at(0), &[] as &[TimedEdit]);
    assert_eq!(log.at(1), &[edit(1, 10), edit(1, 11)]);
    assert_eq!(log.at(4), &[edit(4, 12)]);
    assert_eq!(log.at(5), &[] as &[TimedEdit]);
    assert_eq!(log.last_tick(), Some(4));
}

#[test]
fn json_round_trip_is_exact_for_f32_fields() {
    let mut simulation = simulation();
    let pose = Pose {
        position: Vec2::new(0.1, -3.4028235e38),
        rotation: Rot::from_angle(0.7),
    };
    let entity = simulation.world_mut().spawn();
    simulation.world_mut().insert(entity, pose).unwrap();
    let value = simulation
        .world()
        .component_json(entity, "ludenea::Pose")
        .unwrap()
        .unwrap();
    let bytes = simulation
        .encode_json_value(ValueKind::Component, "ludenea::Pose", value)
        .unwrap();
    assert_eq!(bytes, postcard::to_allocvec(&pose).unwrap());
}

#[test]
fn non_finite_float_has_no_json_form() {
    let mut simulation = simulation();
    let entity = simulation.world_mut().spawn();
    simulation
        .world_mut()
        .insert(entity, Weight(f32::NAN))
        .unwrap();
    let result = simulation
        .world()
        .component_json(entity, "edit::Weight")
        .unwrap();
    assert!(result.is_err());
    let all = simulation.world().entity_components_json(entity).unwrap();
    assert!(all["edit::Weight"].is_err());
}

#[test]
fn contact_cache_has_no_json_form_but_ron_works() {
    let mut simulation = simulation();
    for y in [0.0, 0.5] {
        let entity = simulation.world_mut().spawn();
        let world = simulation.world_mut();
        world
            .insert(
                entity,
                Pose {
                    position: Vec2::new(0.0, y),
                    rotation: Rot::IDENTITY,
                },
            )
            .unwrap();
        world.insert(entity, Body::dynamic()).unwrap();
        world
            .insert(
                entity,
                Collider::new(Shape::Polygon(Polygon::make_box(0.5, 0.5))),
            )
            .unwrap();
    }
    simulation.step().unwrap();
    let cache = simulation
        .world()
        .resource::<ContactCache>()
        .unwrap()
        .clone();
    assert_ne!(cache, ContactCache::default());
    assert!(
        simulation
            .world()
            .resource_json("ludenea::ContactCache")
            .unwrap()
            .is_err()
    );
    let codec = simulation
        .world()
        .resource_codec("ludenea::ContactCache")
        .unwrap();
    assert!(!(codec.encode_ron)(&cache).is_empty());
    let restored = (codec.decode_ron)(&(codec.encode_ron)(&cache)).unwrap();
    assert_eq!(restored.downcast_ref::<ContactCache>(), Some(&cache));
}

#[test]
fn edit_log_decode_rejects_out_of_order_ticks() {
    let edit = |tick: u64| TimedEdit {
        tick,
        edit: Edit::Spawn {
            components: vec![("edit::Tag".to_string(), tag(1))],
        },
    };
    let mut log = EditLog::new();
    log.edits = vec![edit(4), edit(2)];
    assert!(matches!(
        EditLog::decode(&log.encode()),
        Err(SimError::EditLogDecode(_))
    ));
}

#[test]
fn insert_and_remove_on_dead_entity_are_no_such_entity() {
    let mut simulation = simulation();
    let entity = spawn_tagged(&mut simulation, 1);
    simulation
        .apply_edit(&Edit::Despawn {
            entity: EntityRef::Runtime(entity),
        })
        .unwrap();
    let insert = Edit::Insert {
        entity: EntityRef::Runtime(entity),
        component: "edit::Tag".to_string(),
        value: tag(2),
    };
    let remove = Edit::Remove {
        entity: EntityRef::Runtime(entity),
        component: "edit::Tag".to_string(),
    };
    for edit in [insert, remove] {
        let error = simulation.apply_edit(&edit).unwrap_err();
        assert!(matches!(error, EditError::NoSuchEntity(dead) if dead == entity));
    }
}

#[test]
fn spawn_with_duplicate_component_is_rejected_and_world_unchanged() {
    let mut simulation = simulation();
    let edit = Edit::Spawn {
        components: vec![
            ("edit::Tag".to_string(), tag(1)),
            ("edit::Tag".to_string(), tag(2)),
        ],
    };
    let error = simulation.apply_edit(&edit).unwrap_err();
    assert!(matches!(error, EditError::InvalidValue { ref name, .. } if name == "edit::Tag"));
    assert_eq!(simulation.world().entities().count(), 0);
}

const A: u128 = 0xa;
const B: u128 = 0xb;
const C: u128 = 0xc;

fn entities(entities: &[(u32, u32)]) -> String {
    entities
        .iter()
        .map(|(id, tag)| format!("(id: {id}, components: {{\"edit::Tag\": ({tag})}}),"))
        .collect()
}

fn scene(own: &[(u32, u32)], instances: &str) -> String {
    format!(
        "Scene(version: 2, next_id: 10, resources: {{}}, entities: [{}], instances: [{instances}])",
        entities(own)
    )
}

fn prefab(own: &[(u32, u32)]) -> String {
    format!(
        "Prefab(version: 1, next_id: 10, entities: [{}], instances: [])",
        entities(own)
    )
}

fn instance(id: u32, prefab: u128) -> String {
    format!(
        "(id: {id}, prefab: \"{}\", overrides: [], added: [], removed: []),",
        AssetId(prefab)
    )
}

fn stats_file(damage: u32, next: Option<u128>) -> String {
    let next = match next {
        Some(id) => format!("Some(\"{}\")", AssetId(id)),
        None => "None".to_string(),
    };
    format!("AssetFile(type: \"edit::Stats\", value: (damage: {damage}, next: {next}))")
}

fn stats(damage: u32, next: Option<u128>) -> Vec<u8> {
    let next = next.map(|id| Handle::new(AssetId(id)));
    postcard::to_allocvec(&Stats { damage, next }).unwrap()
}

fn texts(assets: &[(u128, String)]) -> AssetTexts {
    assets
        .iter()
        .map(|(id, text)| (AssetId(*id), text.clone()))
        .collect()
}

fn tagged(simulation: &Simulation, value: u32) -> Entity {
    let world = simulation.world();
    world
        .entities()
        .find(|&entity| world.get::<Tag>(entity).is_some_and(|tag| tag.0 == value))
        .unwrap()
}

/// A simulation with the scene entity `[3]` and an `Armed` entity loading asset `A`.
fn armed() -> Simulation {
    let mut simulation = simulation();
    let ron = format!(
        "Scene(version: 2, next_id: 10, resources: {{}}, entities: [(id: 3, components: {{\"edit::Tag\": (3)}}), (id: 4, components: {{\"edit::Armed\": (\"{}\")}})])",
        AssetId(A)
    );
    simulation
        .load_scene_with(&ron, &texts(&[(A, stats_file(1, None))]))
        .unwrap();
    simulation
}

#[test]
fn edit_on_loaded_entity_records_its_address() {
    let mut simulation = simulation();
    let texts = texts(&[(A, prefab(&[(1, 10), (2, 20)]))]);
    simulation
        .load_scene_with(&scene(&[(3, 3)], &instance(5, A)), &texts)
        .unwrap();
    let own = tagged(&simulation, 3);
    let inside = tagged(&simulation, 20);
    assert_eq!(simulation.entity_ref(own), EntityRef::Loaded(vec![3]));
    assert_eq!(simulation.entity_ref(inside), EntityRef::Loaded(vec![5, 2]));
    assert_eq!(
        simulation
            .resolve_ref(&EntityRef::Loaded(vec![5, 2]))
            .unwrap(),
        inside
    );
}

#[test]
fn edit_on_runtime_entity_records_its_identifier() {
    let mut simulation = simulation();
    simulation.load_scene(&scene(&[(3, 3)], "")).unwrap();
    let loaded = tagged(&simulation, 3);
    simulation
        .apply_edit(&Edit::Despawn {
            entity: EntityRef::Runtime(loaded),
        })
        .unwrap();
    // The new entity reuses the slot with another generation: it was not loaded.
    let spawned = spawn_tagged(&mut simulation, 4);
    assert_eq!(spawned.index(), loaded.index());
    assert_eq!(simulation.entity_ref(spawned), EntityRef::Runtime(spawned));
    assert_eq!(
        simulation
            .resolve_ref(&EntityRef::Runtime(spawned))
            .unwrap(),
        spawned
    );
}

#[test]
fn address_survives_an_entity_added_earlier_in_a_prefab() {
    let mut simulation = simulation();
    let ron = scene(&[(3, 3)], &instance(5, A));
    simulation
        .load_scene_with(&ron, &texts(&[(A, prefab(&[(1, 10), (2, 20)]))]))
        .unwrap();
    let target = tagged(&simulation, 20);
    let edit = Edit::Insert {
        entity: simulation.entity_ref(target),
        component: "edit::Tag".to_string(),
        value: tag(21),
    };

    let grown = texts(&[(A, prefab(&[(7, 70), (1, 10), (2, 20)]))]);
    simulation.load_scene_with(&ron, &grown).unwrap();
    assert_ne!(tagged(&simulation, 20), target);
    simulation.apply_edit(&edit).unwrap();
    let mut tags: Vec<u32> = simulation
        .world()
        .entities()
        .filter_map(|entity| tag_of(&simulation, entity))
        .collect();
    tags.sort_unstable();
    assert_eq!(tags, vec![3, 10, 21, 70]);
}

#[test]
fn unresolved_address_is_no_such_address() {
    let mut simulation = simulation();
    simulation.load_scene(&scene(&[(3, 3)], "")).unwrap();
    let before = simulation.hash();
    let error = simulation
        .apply_edit(&Edit::Despawn {
            entity: EntityRef::Loaded(vec![9]),
        })
        .unwrap_err();
    assert!(matches!(error, EditError::NoSuchAddress(ref address) if address == &vec![9]));
    assert_eq!(simulation.hash(), before);
}

#[test]
fn set_asset_replaces_value_and_changes_hash() {
    let mut simulation = armed();
    let before = simulation.hash();
    let value = stats(5, None);
    let edit = Edit::SetAsset {
        id: AssetId(A),
        value: value.clone(),
    };
    simulation.apply_edit(&edit).unwrap();
    assert_eq!(
        simulation.world().asset_bytes(AssetId(A)),
        Some(("edit::Stats", value.clone()))
    );
    assert_ne!(simulation.hash(), before);

    let missing = Edit::SetAsset {
        id: AssetId(B),
        value,
    };
    let error = simulation.apply_edit(&missing).unwrap_err();
    assert!(matches!(error, EditError::MissingAsset(AssetId(B))));
    let invalid = Edit::SetAsset {
        id: AssetId(A),
        value: vec![0xff],
    };
    assert!(matches!(
        simulation.apply_edit(&invalid),
        Err(EditError::InvalidValue { .. })
    ));
}

#[test]
fn assets_introduced_by_lists_missing_handles() {
    let mut simulation = armed();
    let sources = texts(&[
        (A, stats_file(1, None)),
        (B, stats_file(2, Some(C))),
        (C, stats_file(3, Some(A))),
    ]);
    let armed_with_b = Edit::Spawn {
        components: vec![(
            "edit::Armed".to_string(),
            postcard::to_allocvec(&Armed(Handle::new(AssetId(B)))).unwrap(),
        )],
    };
    let loads = simulation
        .assets_introduced_by(&armed_with_b, &sources)
        .unwrap();
    let load = |id: u128, value| Edit::LoadAsset {
        id: AssetId(id),
        asset: "edit::Stats".to_string(),
        value,
    };
    assert_eq!(
        loads,
        vec![load(B, stats(2, Some(C))), load(C, stats(3, Some(A)))]
    );
    for edit in &loads {
        simulation.apply_edit(edit).unwrap();
    }
    assert_eq!(
        simulation
            .assets_introduced_by(&armed_with_b, &sources)
            .unwrap(),
        vec![]
    );

    let without_c = texts(&[(B, stats_file(2, Some(C)))]);
    let fresh = armed();
    let error = fresh
        .assets_introduced_by(&armed_with_b, &without_c)
        .unwrap_err();
    assert!(matches!(error, EditError::MissingAsset(AssetId(C))));
}

/// Edit log version 1, as milestone 2a wrote it.
mod version_1 {
    use ludenea_ecs::Entity;
    use serde::Serialize;

    #[derive(Serialize)]
    pub enum Edit {
        #[allow(dead_code)]
        Spawn {
            components: Vec<(String, Vec<u8>)>,
        },
        Despawn {
            entity: Entity,
        },
        Insert {
            entity: Entity,
            component: String,
            value: Vec<u8>,
        },
        Remove {
            entity: Entity,
            component: String,
        },
        SetResource {
            resource: String,
            value: Vec<u8>,
        },
    }

    #[derive(Serialize)]
    pub struct TimedEdit {
        pub tick: u64,
        pub edit: Edit,
    }

    #[derive(Serialize)]
    pub struct EditLog {
        pub version: u32,
        pub edits: Vec<TimedEdit>,
    }
}

#[test]
fn version_1_edit_log_decodes() {
    let mut simulation = simulation();
    let entity = spawn_tagged(&mut simulation, 1);
    let at = |tick, edit| version_1::TimedEdit { tick, edit };
    let old = version_1::EditLog {
        version: 1,
        edits: vec![
            at(
                1,
                version_1::Edit::Insert {
                    entity,
                    component: "edit::Tag".to_string(),
                    value: tag(2),
                },
            ),
            at(
                2,
                version_1::Edit::Remove {
                    entity,
                    component: "edit::Tag".to_string(),
                },
            ),
            at(
                3,
                version_1::Edit::SetResource {
                    resource: "edit::Nope".to_string(),
                    value: vec![1],
                },
            ),
            at(4, version_1::Edit::Despawn { entity }),
        ],
    };
    let log = EditLog::decode(&postcard::to_allocvec(&old).unwrap()).unwrap();
    assert_eq!(log.version, EDIT_LOG_VERSION);
    let target = EntityRef::Runtime(entity);
    let at = |tick, edit| TimedEdit { tick, edit };
    assert_eq!(
        log.edits,
        vec![
            at(
                1,
                Edit::Insert {
                    entity: target.clone(),
                    component: "edit::Tag".to_string(),
                    value: tag(2),
                }
            ),
            at(
                2,
                Edit::Remove {
                    entity: target.clone(),
                    component: "edit::Tag".to_string(),
                }
            ),
            at(
                3,
                Edit::SetResource {
                    resource: "edit::Nope".to_string(),
                    value: vec![1],
                }
            ),
            at(4, Edit::Despawn { entity: target }),
        ]
    );
}

#[test]
fn version_2_edit_log_decodes_as_the_current_version() {
    let mut log = EditLog::new();
    log.version = 2;
    log.edits.push(TimedEdit {
        tick: 3,
        edit: Edit::Spawn {
            components: vec![("edit::Tag".to_string(), tag(1))],
        },
    });
    let decoded = EditLog::decode(&log.encode()).unwrap();
    assert_eq!(decoded.version, EDIT_LOG_VERSION);
    assert_eq!(decoded.edits, log.edits);
}

#[test]
fn a_version_2_edit_log_holding_a_group_is_refused() {
    let mut log = EditLog::new();
    log.version = 2;
    log.edits.push(TimedEdit {
        tick: 3,
        edit: Edit::Group(vec![Edit::Spawn {
            components: vec![("edit::Tag".to_string(), tag(1))],
        }]),
    });
    let error = EditLog::decode(&log.encode()).unwrap_err();
    assert!(
        matches!(&error, SimError::EditLogDecode(reason)
            if reason == "the edit at tick 3 is a group, which version 2 edit logs do not hold"),
        "{error}"
    );
}

#[test]
fn a_version_2_edit_log_reads_its_prefab_loads() {
    const PREFAB: AssetId = AssetId(0x50);
    let flat = Prefab {
        entities: vec![PrefabEntity {
            address: vec![0],
            components: vec![("edit::Tag".to_string(), tag(1))],
        }],
    };
    let load = |value: Vec<u8>| TimedEdit {
        tick: 3,
        edit: Edit::LoadAsset {
            id: PREFAB,
            asset: Prefab::NAME.to_string(),
            value,
        },
    };
    // Version 2 wrote each prefab followed by the names of its component types that hold an
    // `Entity`, as an edit after tick 0 loaded the prefab it introduced.
    let mut old = EditLog::new();
    old.version = 2;
    old.edits.push(load(
        postcard::to_allocvec(&(&flat.entities, BTreeSet::from(["edit::Armed"]))).unwrap(),
    ));

    let decoded = EditLog::decode(&old.encode()).unwrap();

    assert_eq!(decoded.version, EDIT_LOG_VERSION);
    assert_eq!(decoded.edits, [load(postcard::to_allocvec(&flat).unwrap())]);
    let mut simulation = simulation();
    simulation.apply_edit(&decoded.edits[0].edit).unwrap();
    assert!(simulation.world().asset_ids().contains(&PREFAB));

    // A version 2 prefab without that list does not decode.
    old.edits = vec![load(postcard::to_allocvec(&flat).unwrap())];
    let error = EditLog::decode(&old.encode()).unwrap_err();
    assert!(
        matches!(&error, SimError::EditLogDecode(reason) if reason.starts_with(&format!("the prefab asset {PREFAB} loaded at tick 3: "))),
        "{error}"
    );
}

#[test]
fn a_group_round_trips_in_the_edit_log() {
    let mut log = EditLog::new();
    log.edits.push(TimedEdit {
        tick: 3,
        edit: Edit::Group(vec![
            Edit::Spawn {
                components: vec![("edit::Tag".to_string(), tag(1))],
            },
            Edit::Remove {
                entity: EntityRef::Loaded(vec![4]),
                component: "edit::Tag".to_string(),
            },
        ]),
    });
    assert_eq!(EditLog::decode(&log.encode()).unwrap(), log);
}

#[test]
fn a_group_applies_every_edit_in_order() {
    let mut simulation = simulation();
    let (a, b) = (
        spawn_tagged(&mut simulation, 1),
        spawn_tagged(&mut simulation, 2),
    );
    let group = Edit::Group(vec![
        Edit::Insert {
            entity: EntityRef::Runtime(a),
            component: "edit::Tag".to_string(),
            value: tag(5),
        },
        Edit::Despawn {
            entity: EntityRef::Runtime(b),
        },
        Edit::Spawn {
            components: vec![("edit::Tag".to_string(), tag(7))],
        },
    ]);
    assert_eq!(simulation.apply_edit(&group).unwrap(), None);
    assert_eq!(tag_of(&simulation, a), Some(5));
    assert!(!simulation.world().is_alive(b));
    // The spawned entity holds its tag.
    tagged(&simulation, 7);
}

#[test]
fn a_group_with_a_failing_edit_changes_nothing() {
    let mut simulation = simulation();
    let (a, b) = (
        spawn_tagged(&mut simulation, 1),
        spawn_tagged(&mut simulation, 2),
    );
    let hash = simulation.hash();
    let insert = |entity, value| Edit::Insert {
        entity: EntityRef::Runtime(entity),
        component: "edit::Tag".to_string(),
        value: tag(value),
    };
    let despawn = |entity| Edit::Despawn {
        entity: EntityRef::Runtime(entity),
    };
    // Each edit is checked as the ones before it leave the world: `a` is gone for the third.
    let after_despawn = Edit::Group(vec![insert(b, 5), despawn(a), insert(a, 6)]);
    let error = simulation.apply_edit(&after_despawn).unwrap_err();
    assert!(
        matches!(error, EditError::NoSuchEntity(entity) if entity == a),
        "{error}"
    );
    let invalid_last = Edit::Group(vec![
        despawn(b),
        Edit::Insert {
            entity: EntityRef::Runtime(a),
            component: "edit::Tag".to_string(),
            value: vec![0xff; 9],
        },
    ]);
    let error = simulation.apply_edit(&invalid_last).unwrap_err();
    assert!(matches!(error, EditError::InvalidValue { .. }), "{error}");
    assert_eq!(simulation.hash(), hash);
    assert_eq!(
        (tag_of(&simulation, a), tag_of(&simulation, b)),
        (Some(1), Some(2))
    );
}

#[test]
fn a_group_holds_entity_and_resource_edits_only() {
    let mut simulation = armed();
    let hash = simulation.hash();
    let tag_3 = Edit::Remove {
        entity: EntityRef::Loaded(vec![3]),
        component: "edit::Tag".to_string(),
    };
    let set_asset = Edit::SetAsset {
        id: AssetId(A),
        value: stats(2, None),
    };
    for member in [set_asset, Edit::Group(Vec::new())] {
        let group = Edit::Group(vec![tag_3.clone(), member]);
        let error = simulation.apply_edit(&group).unwrap_err();
        assert!(matches!(error, EditError::NotGroupable), "{error}");
        assert!(
            error
                .to_string()
                .contains("record an asset edit or a group on its own")
        );
        assert_eq!(simulation.hash(), hash);
    }
    let nested = Edit::Group(vec![Edit::Group(Vec::new())]);
    let error = simulation
        .assets_introduced_by(&nested, &texts(&[]))
        .unwrap_err();
    assert!(matches!(error, EditError::NotGroupable), "{error}");
}

#[test]
fn an_empty_group_is_refused() {
    let mut simulation = armed();
    let hash = simulation.hash();

    let error = simulation.apply_edit(&Edit::Group(Vec::new())).unwrap_err();

    assert!(matches!(error, EditError::EmptyGroup), "{error}");
    assert_eq!(
        error.to_string(),
        "a group holds no edit; record at least one edit in it"
    );
    assert_eq!(simulation.hash(), hash);
}

#[test]
fn a_group_introduces_the_assets_of_every_edit() {
    let simulation = armed();
    let sources = texts(&[(B, stats_file(2, None)), (C, stats_file(3, None))]);
    let armed_with = |id: u128| Edit::Spawn {
        components: vec![(
            "edit::Armed".to_string(),
            postcard::to_allocvec(&Armed(Handle::new(AssetId(id)))).unwrap(),
        )],
    };
    let group = Edit::Group(vec![
        armed_with(B),
        armed_with(A),
        armed_with(C),
        armed_with(B),
    ]);
    let load = |id: u128, value| Edit::LoadAsset {
        id: AssetId(id),
        asset: "edit::Stats".to_string(),
        value,
    };
    assert_eq!(
        simulation.assets_introduced_by(&group, &sources).unwrap(),
        vec![load(B, stats(2, None)), load(C, stats(3, None))]
    );
}

/// Asserts that a count is never 13, as a game in development may, instead of refusing it.
#[derive(Component, Serialize, Deserialize)]
#[serde(try_from = "u32")]
struct Unlucky(u32);

impl TryFrom<u32> for Unlucky {
    type Error = String;

    fn try_from(count: u32) -> Result<Self, String> {
        assert!(count != 13, "13 is unlucky");
        Ok(Unlucky(count))
    }
}

#[test]
fn a_value_its_type_panics_on_is_refused_and_the_world_is_unchanged() {
    let mut app = App::new();
    app.component::<Unlucky>();
    let mut simulation = app.build().unwrap();
    let before = simulation.hash();
    let spawn = Edit::Spawn {
        components: vec![(
            "edit::Unlucky".to_string(),
            postcard::to_allocvec(&13u32).unwrap(),
        )],
    };

    let Err(EditError::Panicked(applied)) = simulation.apply_edit(&spawn) else {
        panic!("the edit must fail");
    };
    let Err(EditError::Panicked(encoded)) =
        simulation.encode_json_value(ValueKind::Component, "edit::Unlucky", json!(13))
    else {
        panic!("the value must fail");
    };

    assert_eq!(
        (applied.tick, applied.message.as_str()),
        (0, "13 is unlucky")
    );
    assert_eq!(encoded.message, "13 is unlucky");
    assert_eq!(simulation.hash(), before);
    assert_eq!(simulation.world().entities().count(), 0);
}
