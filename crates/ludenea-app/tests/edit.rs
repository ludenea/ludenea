use ludenea_app::edit::{Edit, EditError, EditLog, TimedEdit, ValueKind};
use ludenea_app::{App, SimError, Simulation};
use ludenea_ecs::{Component, Entity};
use ludenea_math::{Rot, Vec2};
use ludenea_physics::{Body, Collider, ContactCache, PhysicsPlugin, Polygon, Pose, Shape};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Tag(u32);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Weight(f32);

fn simulation() -> Simulation {
    let mut app = App::new();
    app.plugin(PhysicsPlugin)
        .component::<Tag>()
        .component::<Weight>();
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
        entity,
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

    first.apply_edit(&Edit::Despawn { entity: a }).unwrap();
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
        entity,
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
    simulation.apply_edit(&Edit::Despawn { entity }).unwrap();
    let error = simulation
        .apply_edit(&Edit::Despawn { entity })
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
        entity,
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
            entity,
            component: "edit::Nope".to_string(),
            value: tag(1),
        },
        Edit::Remove {
            entity,
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
            expected: 1
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
    simulation.step();
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
    simulation.apply_edit(&Edit::Despawn { entity }).unwrap();
    let insert = Edit::Insert {
        entity,
        component: "edit::Tag".to_string(),
        value: tag(2),
    };
    let remove = Edit::Remove {
        entity,
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
