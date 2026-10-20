use std::collections::BTreeMap;

use boxes::BoxesInput;
use ludenea::control::Event;
use ludenea::control::*;
use ludenea::prelude::*;
use serde_json::{Value, json};

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
}

/// A boxes host with the scene and the recorded `inputs` loaded.
fn with_inputs(inputs: &[BoxesInput]) -> Host {
    let mut host = Host::new(boxes::app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: boxes::generate_scene(30, 7),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let mut log = InputLog::new(1);
    for input in inputs {
        log.record(std::slice::from_ref(input));
    }
    ok(
        &mut host,
        Command::LoadInputs {
            log: Bytes(log.encode()),
        },
    );
    host
}

fn loaded(ticks: usize) -> Host {
    with_inputs(&vec![BoxesInput::default(); ticks])
}

fn step(host: &mut Host, ticks: u32) {
    ok(
        host,
        Command::Step {
            ticks,
            record_hashes: false,
        },
    );
}

fn subscribe(host: &mut Host, topic: Topic) {
    assert_eq!(ok(host, Command::Subscribe { topic }), Reply::Done);
}

fn query(host: &mut Host, filter: &str, offset: u32, limit: u32) -> (u32, Vec<EntityRow>) {
    match ok(
        host,
        Command::QueryEntities {
            filter: filter.to_string(),
            offset,
            limit,
        },
    ) {
        Reply::Entities { total, rows } => (total, rows),
        other => panic!("{other:?}"),
    }
}

/// The first crate of the scene, a circle.
fn first_crate(host: &mut Host) -> Entity {
    query(host, "With<Crate>", 0, 1).1[0].entity
}

fn schedule(host: &mut Host) -> Vec<SystemRow> {
    match ok(host, Command::GetSchedule) {
        Reply::Schedule { systems } => systems,
        other => panic!("{other:?}"),
    }
}

fn tick_events(host: &mut Host, tick: u64) -> Result<Vec<RecordedEvent>, ProtocolError> {
    host.handle(Command::GetTickEvents { tick })
        .map(|reply| match reply {
            Reply::TickEvents { events } => events,
            other => panic!("{other:?}"),
        })
}

fn pose(host: &mut Host, entity: Entity) -> Value {
    match ok(host, Command::GetEntity { entity }) {
        Reply::Entity { mut components, .. } => match components.remove("ludenea::Pose") {
            Some(ComponentValue::Json { value }) => value,
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

/// The `EntityChanged` events among `events`, as (changed component names, removed names).
fn entity_changes(events: &[Event]) -> Vec<(Vec<String>, Vec<String>)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::EntityChanged {
                components,
                removed,
                ..
            } => Some((components.keys().cloned().collect(), removed.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn query_entities_pages_and_reports_total() {
    let mut host = loaded(10);
    let (total, all) = query(&mut host, "", 0, u32::MAX);
    assert_eq!(total as usize, all.len());
    let (crates, page) = query(&mut host, "With<Crate>", 10, 5);
    assert_eq!(crates, 30);
    assert_eq!(page.len(), 5);
    let (_, every_crate) = query(&mut host, "With<boxes::Crate>", 0, 100);
    assert_eq!(page, every_crate[10..15]);
    assert_eq!(query(&mut host, "With<Crate>", 28, 5).1.len(), 2);
    assert!(query(&mut host, "With<Crate>", 40, 5).1.is_empty());

    let Reply::HashTree { tree, .. } = ok(&mut host, Command::GetHashTree) else {
        panic!()
    };
    for row in &page {
        let leaf = tree
            .entities
            .iter()
            .find(|e| e.entity == row.entity)
            .unwrap();
        assert_eq!(row.hash, Hash64(leaf.hash));
    }
    let walls = query(&mut host, "Without<Crate>", 0, 100).0;
    assert_eq!(walls + crates, total);

    subscribe(
        &mut host,
        Topic::Entities {
            filter: "With<Crate>".into(),
        },
    );
    host.advance(0.0);
    step(&mut host, 2);
    let entity = first_crate(&mut host);
    ok(&mut host, Command::Despawn { entity });
    let events = host.advance(0.0);
    let changes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::EntitiesChanged { filter, total } => Some((filter.as_str(), *total)),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [("With<Crate>", 30), ("With<Crate>", 29)]);
}

#[test]
fn invalid_filter_lists_candidates() {
    let mut host = loaded(1);
    let error = host
        .handle(Command::QueryEntities {
            filter: "With<Nope>".into(),
            offset: 0,
            limit: 10,
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidFilter);
    assert!(error.message.contains("Nope"), "{}", error.message);
    assert!(error.message.contains("boxes::Crate"), "{}", error.message);
    assert!(error.message.contains("ludenea::Pose"), "{}", error.message);

    let error = host
        .handle(Command::Subscribe {
            topic: Topic::Entities {
                filter: "Has<Pose>".into(),
            },
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidFilter);
}

#[test]
fn schedule_lists_accesses_and_timings_after_introspection() {
    let mut host = loaded(10);
    let systems = schedule(&mut host);
    let apply_inputs = systems
        .iter()
        .find(|system| system.name.ends_with("apply_inputs"))
        .unwrap();
    assert!(
        apply_inputs
            .writes
            .contains(&"component ludenea::Body".to_string()),
        "{apply_inputs:?}"
    );
    assert!(
        apply_inputs
            .reads
            .contains(&"component ludenea::Collider".to_string()),
        "{apply_inputs:?}"
    );
    assert!(systems.iter().all(|system| system.micros.is_none()));

    subscribe(&mut host, Topic::Introspection);
    step(&mut host, 3);
    let systems = schedule(&mut host);
    assert!(
        systems
            .iter()
            .all(|system| system.micros.is_some_and(|micros| micros >= 0.0)),
        "{systems:?}"
    );
}

#[test]
fn tick_events_contain_collision_begin() {
    let mut host = loaded(120);
    subscribe(&mut host, Topic::Introspection);
    step(&mut host, 120);
    let begins = (1..=120)
        .flat_map(|tick| tick_events(&mut host, tick).unwrap())
        .filter(|event| event.event_type.ends_with("Collision") && event.value["kind"] == "Begin")
        .count();
    assert!(begins > 0);
}

#[test]
fn tick_commands_name_spawn_after_apply_inputs() {
    let mut inputs = vec![BoxesInput::default(); 6];
    inputs[3].spawn = Some(1.0);
    let mut host = with_inputs(&inputs);
    subscribe(&mut host, Topic::Introspection);
    step(&mut host, 5);
    // The input recorded at tick 3 runs in the tick that ends at 4.
    let Reply::TickCommands { sync_points } = ok(&mut host, Command::GetTickCommands { tick: 4 })
    else {
        panic!()
    };
    assert_eq!(sync_points.len(), 1, "{sync_points:?}");
    let point = &sync_points[0];
    assert!(point.after_system.ends_with("apply_inputs"), "{point:?}");
    assert!(point.commands[0].starts_with("spawn "), "{point:?}");
    let spawned = point.commands[0].trim_start_matches("spawn ");
    assert!(
        point
            .commands
            .contains(&format!("insert ludenea::Body on {spawned}")),
        "{point:?}"
    );
    let Reply::TickCommands { sync_points } = ok(&mut host, Command::GetTickCommands { tick: 3 })
    else {
        panic!()
    };
    assert!(sync_points.is_empty(), "{sync_points:?}");
}

#[test]
fn tick_outside_window_is_invalid_tick() {
    let mut host = loaded(700);
    step(&mut host, 3);
    subscribe(&mut host, Topic::Introspection);
    step(&mut host, 2);
    assert!(tick_events(&mut host, 5).is_ok());
    assert_eq!(
        tick_events(&mut host, 3).unwrap_err().code,
        ErrorCode::InvalidTick
    );
    assert_eq!(
        tick_events(&mut host, 6).unwrap_err().code,
        ErrorCode::InvalidTick
    );
    let error = host
        .handle(Command::GetTickCommands { tick: 99 })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidTick);

    // Ticks 4 to 605 ran while subscribed; the window keeps the last 600.
    step(&mut host, 600);
    assert_eq!(
        tick_events(&mut host, 5).unwrap_err().code,
        ErrorCode::InvalidTick
    );
    assert!(tick_events(&mut host, 6).is_ok());

    // A tick whose recorded future was replaced is no longer the recorded one.
    ok(&mut host, Command::Seek { tick: 100 });
    let entity = first_crate(&mut host);
    ok(&mut host, Command::Despawn { entity });
    assert_eq!(
        tick_events(&mut host, 101).unwrap_err().code,
        ErrorCode::InvalidTick
    );

    ok(
        &mut host,
        Command::Unsubscribe {
            topic: Topic::Introspection,
        },
    );
    assert_eq!(
        tick_events(&mut host, 100).unwrap_err().code,
        ErrorCode::InvalidTick
    );
}

#[test]
fn entity_subscription_pushes_only_changed_components() {
    let mut host = loaded(20);
    step(&mut host, 5);
    let entity = first_crate(&mut host);
    subscribe(&mut host, Topic::Entity { entity });
    assert!(entity_changes(&host.advance(0.0)).is_empty());

    step(&mut host, 1);
    let changes = entity_changes(&host.advance(0.0));
    assert_eq!(changes.len(), 1, "{changes:?}");
    let (changed, removed) = &changes[0];
    assert!(
        changed.contains(&"ludenea::Pose".to_string()),
        "{changed:?}"
    );
    assert!(!changed.contains(&"ludenea::Collider".to_string()));
    assert!(!changed.contains(&"boxes::Crate".to_string()));
    assert!(removed.is_empty());
    assert!(entity_changes(&host.advance(0.0)).is_empty());

    ok(
        &mut host,
        Command::RemoveComponent {
            entity,
            component: "boxes::Crate".into(),
        },
    );
    assert_eq!(
        entity_changes(&host.advance(0.0)),
        [(vec![], vec!["boxes::Crate".to_string()])]
    );

    subscribe(
        &mut host,
        Topic::Resource {
            name: "boxes::ContactStats".into(),
        },
    );
    // Its value as subscribed, then nothing until it changes.
    assert_eq!(
        resource_changes(&host.advance(0.0)),
        [vec!["boxes::ContactStats".to_string()]]
    );
    assert_eq!(
        resource_changes(&host.advance(0.0)),
        Vec::<Vec<String>>::new()
    );
    ok(
        &mut host,
        Command::SetField {
            target: FieldTarget::Resource {
                resource: "boxes::ContactStats".into(),
            },
            path: vec![PathSegment::Field {
                name: "begins".into(),
            }],
            value: json!(1000),
        },
    );
    let events = host.advance(0.0);
    let resources: Vec<&BTreeMap<String, ComponentValue>> = events
        .iter()
        .filter_map(|event| match event {
            Event::ResourcesChanged { resources } => Some(resources),
            _ => None,
        })
        .collect();
    assert_eq!(resources.len(), 1, "{events:?}");
    assert_eq!(
        resources[0].keys().collect::<Vec<_>>(),
        ["boxes::ContactStats"]
    );
}

/// The names of the resources each `ResourcesChanged` among `events` carries.
fn resource_changes(events: &[Event]) -> Vec<Vec<String>> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ResourcesChanged { resources } => Some(resources.keys().cloned().collect()),
            _ => None,
        })
        .collect()
}

#[test]
fn resource_subscription_pushes_only_the_subscribed_resource() {
    let mut host = loaded(20);
    subscribe(
        &mut host,
        Topic::Resource {
            name: "ludenea::Time".into(),
        },
    );
    host.advance(0.0);

    // Every tick changes `ludenea::Time` and the contact cache: only the first is pushed.
    step(&mut host, 1);
    assert_eq!(
        resource_changes(&host.advance(0.0)),
        [vec!["ludenea::Time".to_string()]]
    );
    // A second subscriber gets the current value too.
    subscribe(
        &mut host,
        Topic::Resource {
            name: "ludenea::Time".into(),
        },
    );
    assert_eq!(
        resource_changes(&host.advance(0.0)),
        [vec!["ludenea::Time".to_string()]]
    );

    ok(
        &mut host,
        Command::Unsubscribe {
            topic: Topic::Resource {
                name: "ludenea::Time".into(),
            },
        },
    );
    step(&mut host, 1);
    assert_eq!(
        resource_changes(&host.advance(0.0)),
        Vec::<Vec<String>>::new()
    );
}

#[test]
fn resource_subscription_to_an_unknown_resource_fails() {
    let mut host = loaded(20);
    let error = host
        .handle(Command::Subscribe {
            topic: Topic::Resource {
                name: "boxes::Missing".into(),
            },
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownResource);
    assert!(
        error.message.contains("boxes::Missing"),
        "{}",
        error.message
    );
}

#[test]
fn resource_pushes_are_capped_while_playing_and_catch_up_once_paused() {
    let mut host = loaded(120);
    subscribe(
        &mut host,
        Topic::Resource {
            name: "ludenea::Time".into(),
        },
    );
    host.advance(0.0);
    ok(&mut host, Command::Play);
    let mut pushes = 0;
    for _ in 0..61 {
        pushes += resource_changes(&host.advance(1.0 / 60.0)).len();
    }
    // One second of play: `ludenea::Time` changes every tick, pushed at most 10 times.
    assert!(
        (1..=RESOURCE_PUSHES_PER_SECOND as usize).contains(&pushes),
        "{pushes} pushes"
    );

    ok(&mut host, Command::Pause);
    let tick = host.tick();
    let events = host.advance(0.0);
    let pushed = events.iter().find_map(|event| match event {
        Event::ResourcesChanged { resources } => resources.get("ludenea::Time").cloned(),
        _ => None,
    });
    let Some(ComponentValue::Json { value }) = pushed else {
        panic!("the last value is pushed once paused: {events:?}");
    };
    assert_eq!(value["tick"], json!(tick));
}

#[test]
fn seek_pushes_entity_despawned_for_entities_not_yet_spawned() {
    let mut host = loaded(20);
    step(&mut host, 5);
    let first = first_crate(&mut host);
    let crate_pose = pose(&mut host, first);
    let Reply::Spawned { entity } = ok(
        &mut host,
        Command::Spawn {
            components: BTreeMap::from([("ludenea::Pose".to_string(), crate_pose)]),
        },
    ) else {
        panic!()
    };
    subscribe(&mut host, Topic::Entity { entity });
    host.advance(0.0);

    ok(&mut host, Command::Seek { tick: 2 });
    let events = host.advance(0.0);
    assert!(
        events.contains(&Event::EntityDespawned { entity }),
        "{events:?}"
    );
    assert!(entity_changes(&events).is_empty());

    ok(&mut host, Command::Seek { tick: 5 });
    assert_eq!(
        entity_changes(&host.advance(0.0)),
        [(vec!["ludenea::Pose".to_string()], vec![])]
    );
}

#[test]
fn render_frame_and_pick_through_protocol() {
    let mut host = loaded(10);
    step(&mut host, 2);
    let debug_layers = host.game_info().debug_layers;
    let Reply::RenderFrame { frame } = ok(&mut host, Command::GetRenderFrame { debug_layers })
    else {
        panic!()
    };
    assert_eq!(frame.tick, 2);
    assert!(!frame.debug.layers.is_empty());
    assert!(frame.sprites.is_empty(), "{:?}", frame.sprites);

    let entity = first_crate(&mut host);
    let position = &pose(&mut host, entity)["position"];
    let point = Vec2::new(
        position["x"].as_f64().unwrap() as f32,
        position["y"].as_f64().unwrap() as f32,
    );
    let Reply::Picked { entities } = ok(&mut host, Command::Pick { point }) else {
        panic!()
    };
    assert!(entities.contains(&entity), "{entities:?}");
}

#[test]
fn render_frame_carries_sprites_of_a_render_plugin_game() {
    fn app() -> App {
        let mut app = App::new();
        app.plugin(RenderPlugin).render_layer("world", 1.0);
        app
    }
    let mut host = Host::new(app).unwrap();
    ok(
        &mut host,
        Command::LoadScene {
            scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".into(),
            assets: Vec::new(),
            document: Document::Scene,
        },
    );
    let pose = Pose {
        position: Vec2::new(1.0, 2.0),
        rotation: Rot::IDENTITY,
    };
    let sprite = Sprite {
        frame: 3,
        ..Sprite::default()
    };
    let Reply::Spawned { entity } = ok(
        &mut host,
        Command::Spawn {
            components: BTreeMap::from([
                (
                    "ludenea::Pose".to_string(),
                    serde_json::to_value(pose).unwrap(),
                ),
                (
                    "ludenea::Sprite".to_string(),
                    serde_json::to_value(sprite).unwrap(),
                ),
            ]),
        },
    ) else {
        panic!()
    };

    let Reply::RenderFrame { frame } = ok(
        &mut host,
        Command::GetRenderFrame {
            debug_layers: Vec::new(),
        },
    ) else {
        panic!()
    };
    assert_eq!(frame.layers.len(), 1);
    assert_eq!(frame.layers[0].name, "world");
    let [item] = frame.sprites.as_slice() else {
        panic!("{:?}", frame.sprites)
    };
    assert_eq!(
        (item.entity, item.frame, item.position),
        (entity, 3, Vec2::new(1.0, 2.0))
    );
    assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
}
