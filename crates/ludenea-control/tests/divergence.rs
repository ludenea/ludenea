use std::fs;
use std::path::Path;

use ludenea::cli::assets::project_assets;
use ludenea::control::divergence::{
    Cause, DivergenceKind, DivergenceReport, EntityDivergence, RootCauseKind,
};
use ludenea::control::{
    Bytes, Command, Document, Event, FieldTarget, Hash64, Host, PathSegment, Reply, Topic,
};
use ludenea::ecs::Entity;
use ludenea::prelude::RawInput;
use serde_json::json;

const BOXES_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.scene.ron"
);
const BOXES_INPUTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/boxes.inputs"
);
const SPAWNER_SCENE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/boxes/assets/spawner.scene.ron"
);

const FORK: u64 = 200;

fn ok(host: &mut Host, command: Command) -> Reply {
    host.handle(command).unwrap()
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

fn seek(host: &mut Host, tick: u64) {
    ok(host, Command::Seek { tick });
}

fn hash(host: &mut Host) -> Hash64 {
    match ok(host, Command::GetHash) {
        Reply::Hash { hash, .. } => hash,
        other => panic!("{other:?}"),
    }
}

fn load(host: &mut Host, scene: String, assets: Vec<(ludenea::ecs::AssetId, String)>) {
    ok(
        host,
        Command::LoadScene {
            scene,
            assets,
            document: Document::Scene,
        },
    );
}

/// A boxes host on a small generated scene, run to tick 260.
fn small_host() -> Host {
    let mut host = Host::new(boxes::app).unwrap();
    load(&mut host, boxes::generate_scene(30, 7), Vec::new());
    step(&mut host, 260);
    host
}

fn first_crate(host: &mut Host) -> Entity {
    match ok(
        host,
        Command::QueryEntities {
            filter: "With<boxes::Crate>".to_string(),
            offset: 0,
            limit: 1,
        },
    ) {
        Reply::Entities { rows, .. } => rows[0].entity,
        other => panic!("{other:?}"),
    }
}

/// Sets the horizontal velocity of `entity`, an edit at the current tick.
fn push(host: &mut Host, entity: Entity) {
    ok(
        host,
        Command::SetField {
            target: FieldTarget::Component {
                entity,
                component: "ludenea::Body".to_string(),
            },
            path: ["linear_velocity", "x"]
                .into_iter()
                .map(|name| PathSegment::Field {
                    name: name.to_string(),
                })
                .collect(),
            value: json!(12.0),
        },
    );
}

/// The small host forked at `FORK` by pushing its first crate, the new branch run to 260.
/// Returns the pushed crate.
fn pushed() -> (Host, Entity) {
    let mut host = small_host();
    seek(&mut host, FORK);
    let pushed = first_crate(&mut host);
    push(&mut host, pushed);
    step(&mut host, 60);
    (host, pushed)
}

fn report(host: &mut Host, up_to: Option<u64>) -> DivergenceReport {
    match ok(host, Command::GetDivergence { up_to }) {
        Reply::Divergence(report) => report,
        other => panic!("{other:?}"),
    }
}

/// Asks for the report and advances, paused, until it completes.
fn complete(host: &mut Host, up_to: Option<u64>) -> DivergenceReport {
    for _ in 0..100 {
        let report = report(host, up_to);
        if report.complete {
            return report;
        }
        host.advance(0.0);
    }
    panic!("the report never completed");
}

fn progress(events: &[Event]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::DivergenceProgress { up_to } => Some(*up_to),
            _ => None,
        })
        .collect()
}

fn entity(report: &DivergenceReport, entity: Entity) -> &EntityDivergence {
    report
        .entities
        .iter()
        .find(|divergence| divergence.entity == entity)
        .unwrap_or_else(|| panic!("{entity} is not reported: {report:?}"))
}

#[test]
fn edited_entity_diverges_first_with_the_edit_as_root_cause() {
    let (mut host, pushed) = pushed();
    let report = complete(&mut host, None);
    assert_eq!(report.fork_tick, FORK);
    assert_eq!(report.up_to, 260, "capped at the end of the runs");
    assert_eq!(report.first_divergent, Some(FORK));
    let root = report.root_cause.as_ref().expect("a root cause");
    assert_eq!(root.tick, FORK);
    assert_eq!(root.entities, vec![pushed]);
    let RootCauseKind::Edit { description } = &root.kind else {
        panic!("{root:?}");
    };
    assert!(description.contains("ludenea::Body"), "{description}");

    let first = &report.entities[0];
    assert_eq!(
        (first.entity, first.kind, first.first_tick),
        (pushed, DivergenceKind::Changed, FORK)
    );
    let difference = first.first_difference.as_ref().expect("a difference");
    assert_eq!(difference.component, "ludenea::Body");
    assert_eq!(difference.path, ["linear_velocity", "x"]);
    assert!(report.entities.len() > 1, "the push spreads: {report:?}");
    assert!(
        report
            .entities
            .windows(2)
            .all(|pair| (pair[0].first_tick, pair[0].entity) < (pair[1].first_tick, pair[1].entity)),
        "ordered by tick, then entity"
    );
}

#[test]
fn take_over_is_root_cause() {
    let mut host = small_host();
    seek(&mut host, FORK);
    let spawn = RawInput {
        keys: ["KeyS".to_string()].into(),
        ..RawInput::default()
    };
    ok(
        &mut host,
        Command::SetLiveInput {
            players: vec![spawn],
        },
    );
    ok(&mut host, Command::Play);
    host.advance(1.0 / 60.0);
    ok(&mut host, Command::Pause);
    ok(
        &mut host,
        Command::SetLiveInput {
            players: vec![RawInput::default()],
        },
    );
    step(&mut host, 59);

    let report = complete(&mut host, None);
    assert_eq!(report.fork_tick, FORK);
    let root = report.root_cause.expect("a root cause");
    assert_eq!((root.tick, root.kind), (FORK, RootCauseKind::TakeOver));
    let spawned = report
        .entities
        .iter()
        .find(|divergence| divergence.kind == DivergenceKind::OnlyInCurrent)
        .expect("the crate the take-over dropped");
    assert_eq!(spawned.first_tick, FORK + 1);
    assert!(
        spawned.causes.iter().any(|cause| matches!(
            cause,
            Cause::Command { tick, system, kind }
                if *tick == FORK + 1 && system.ends_with("apply_inputs") && kind == "spawn"
        )),
        "{spawned:?}"
    );
}

#[test]
fn collided_entity_lists_the_collision_event_as_cause() {
    let (mut host, pushed) = pushed();
    let report = complete(&mut host, None);
    let collided = report.entities.iter().find(|divergence| {
        divergence.entity != pushed
            && divergence.first_tick > FORK
            && divergence.causes.iter().any(|cause| {
                matches!(
                    cause,
                    Cause::Event { tick, event, other, .. }
                        if event == "ludenea::Collision"
                            && *other == pushed
                            && (divergence.first_tick - 1..=divergence.first_tick).contains(tick)
                )
            })
    });
    assert!(collided.is_some(), "{report:?}");
}

#[test]
fn prefab_spawn_shows_as_command_cause() {
    let mut host = Host::new(boxes::app).unwrap();
    let assets = project_assets(Path::new(SPAWNER_SCENE), None)
        .unwrap()
        .texts
        .into_iter()
        .collect();
    load(
        &mut host,
        fs::read_to_string(SPAWNER_SCENE).unwrap(),
        assets,
    );
    step(&mut host, 260);
    seek(&mut host, FORK);
    // The spawned entity takes the identifier the spawner's next crate gets on the root.
    ok(
        &mut host,
        Command::Spawn {
            components: [("boxes::Crate".to_string(), json!(null))].into(),
        },
    );
    step(&mut host, 60);

    let report = complete(&mut host, None);
    let spawned = report
        .entities
        .iter()
        .filter(|divergence| divergence.first_tick > FORK)
        .find(|divergence| divergence.kind == DivergenceKind::OnlyInCurrent)
        .unwrap_or_else(|| panic!("{report:?}"));
    assert!(
        spawned.causes.iter().any(|cause| matches!(
            cause,
            Cause::Command { system, kind, .. }
                if system.ends_with("spawn_crates") && kind == "spawn"
        )),
        "{spawned:?}"
    );
}

#[test]
fn entity_only_in_one_branch_is_reported() {
    let mut host = small_host();
    seek(&mut host, FORK);
    let despawned = first_crate(&mut host);
    ok(&mut host, Command::Despawn { entity: despawned });
    step(&mut host, 60);

    let report = complete(&mut host, Some(FORK + 10));
    let divergence = entity(&report, despawned);
    assert_eq!(
        (divergence.kind, divergence.first_tick),
        (DivergenceKind::OnlyInCompare, FORK)
    );
    assert_eq!(divergence.first_difference, None);
}

#[test]
fn different_component_set_is_replaced() {
    let mut host = small_host();
    seek(&mut host, FORK);
    let changed = first_crate(&mut host);
    ok(
        &mut host,
        Command::RemoveComponent {
            entity: changed,
            component: "boxes::Crate".to_string(),
        },
    );
    step(&mut host, 60);

    // A range of the fork tick alone.
    let report = complete(&mut host, Some(FORK));
    assert_eq!(report.up_to, FORK);
    let divergence = entity(&report, changed);
    assert_eq!(
        (divergence.kind, divergence.first_tick),
        (DivergenceKind::Replaced, FORK)
    );
    assert_eq!(divergence.first_difference, None);
}

#[test]
fn report_is_incremental_and_completes() {
    let (mut host, _) = pushed();
    let first = report(&mut host, Some(FORK + 20));
    assert_eq!((first.fork_tick, first.up_to), (FORK, FORK));
    assert!(!first.complete);
    assert!(first.entities.is_empty());

    // While playing, one advance compares 4 ticks at most.
    ok(&mut host, Command::Play);
    let mut reached = Vec::new();
    for _ in 0..10 {
        reached.extend(progress(&host.advance(1.0 / 60.0)));
    }
    ok(&mut host, Command::Pause);
    assert!(reached.len() >= 2, "{reached:?}");
    assert!(
        reached.windows(2).all(|pair| pair[0] < pair[1]),
        "{reached:?}"
    );
    assert_eq!(reached.last(), Some(&(FORK + 20)));
    let done = report(&mut host, Some(FORK + 20));
    assert!(done.complete);
    assert_eq!(done.up_to, FORK + 20);
    assert!(
        progress(&host.advance(0.0)).is_empty(),
        "nothing left to run"
    );

    // A longer range goes on from there, up to the end of the runs at 260.
    let longer = report(&mut host, Some(FORK + 600));
    assert_eq!((longer.up_to, longer.complete), (FORK + 20, false));
    assert_eq!(longer.entities, done.entities);
    assert_eq!(progress(&host.advance(0.0)), [260]);
    let all = report(&mut host, Some(FORK + 600));
    assert!(all.complete);
    assert_eq!(all.entities[..done.entities.len()], done.entities[..]);
}

#[test]
fn resource_edit_roots_the_earliest_divergent_entities() {
    let mut host = small_host();
    seek(&mut host, FORK);
    ok(
        &mut host,
        Command::SetField {
            target: FieldTarget::Resource {
                resource: "ludenea::PhysicsSettings".to_string(),
            },
            path: ["gravity", "y"]
                .into_iter()
                .map(|name| PathSegment::Field {
                    name: name.to_string(),
                })
                .collect(),
            value: json!(-5.0),
        },
    );
    step(&mut host, 60);

    let report = complete(&mut host, Some(FORK + 10));
    assert_eq!(
        report.first_divergent,
        Some(FORK),
        "the resource differs first"
    );
    let root = report.root_cause.clone().expect("a root cause");
    let RootCauseKind::Edit { description } = &root.kind else {
        panic!("{root:?}");
    };
    assert!(
        description.contains("ludenea::PhysicsSettings"),
        "{description}"
    );
    let earliest = report.entities[0].first_tick;
    assert!(earliest > FORK, "{report:?}");
    let expected: Vec<Entity> = report
        .entities
        .iter()
        .filter(|divergence| divergence.first_tick == earliest)
        .map(|divergence| divergence.entity)
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(root.entities, expected);
}

#[test]
fn a_batch_is_one_root_cause_naming_each_edit() {
    let mut host = small_host();
    seek(&mut host, FORK);
    let crates: Vec<Entity> = match ok(
        &mut host,
        Command::QueryEntities {
            filter: "With<boxes::Crate>".to_string(),
            offset: 0,
            limit: 2,
        },
    ) {
        Reply::Entities { rows, .. } => rows.into_iter().map(|row| row.entity).collect(),
        other => panic!("{other:?}"),
    };
    let push = |entity| Command::SetField {
        target: FieldTarget::Component {
            entity,
            component: "ludenea::Body".to_string(),
        },
        path: vec![PathSegment::Field {
            name: "angular_velocity".to_string(),
        }],
        value: json!(3.0),
    };
    let commands = crates.iter().map(|&entity| push(entity)).collect();
    ok(&mut host, Command::Batch { commands });
    step(&mut host, 60);

    let report = complete(&mut host, Some(FORK + 10));
    let root = report.root_cause.expect("a root cause");
    assert_eq!(root.tick, FORK);
    let RootCauseKind::Edit { description } = &root.kind else {
        panic!("{root:?}");
    };
    assert!(
        description.starts_with("2 edits: insert ludenea::Body on "),
        "{description}"
    );
    assert_eq!(
        description.matches("insert ludenea::Body").count(),
        2,
        "{description}"
    );
}

#[test]
fn no_work_without_request() {
    // Without a fork there is nothing to compare, subscribed or not.
    let mut host = small_host();
    let error = host
        .handle(Command::GetDivergence { up_to: None })
        .unwrap_err();
    assert!(error.message.contains("compare branch"), "{error:?}");
    host.advance(0.0);
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Divergence,
        },
    );
    assert!(host.advance(0.0).is_empty());

    let (mut host, _) = pushed();
    let before = hash(&mut host);
    for _ in 0..5 {
        assert!(progress(&host.advance(0.0)).is_empty());
    }
    assert_eq!(hash(&mut host), before);
    let report = report(&mut host, None);
    assert_eq!(report.up_to, FORK, "nothing ran before the request");
    assert!(!report.complete);

    // Subscribed, it runs without being asked.
    let (mut host, _) = pushed();
    let before = hash(&mut host);
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Divergence,
        },
    );
    let mut events = Vec::new();
    for _ in 0..5 {
        events.extend(host.advance(0.0));
    }
    assert_eq!(progress(&events).last(), Some(&260));
    assert_eq!(hash(&mut host), before, "the main session is untouched");
}

#[test]
#[ignore = "timing, run in release"]
// Measuring time is outside the simulation.
#[allow(clippy::disallowed_methods)]
fn report_over_600_ticks_under_one_second() {
    let mut host = Host::new(boxes::app).unwrap();
    load(
        &mut host,
        fs::read_to_string(BOXES_SCENE).unwrap(),
        Vec::new(),
    );
    ok(
        &mut host,
        Command::LoadInputs {
            log: Bytes(fs::read(BOXES_INPUTS).unwrap()),
        },
    );
    step(&mut host, 800);
    seek(&mut host, FORK);
    let pushed = first_crate(&mut host);
    push(&mut host, pushed);
    step(&mut host, 600);

    let started = std::time::Instant::now();
    let report = complete(&mut host, None);
    let elapsed = started.elapsed();
    println!("the report over 600 ticks took {elapsed:?}");
    assert_eq!(report.up_to, FORK + 600);
    assert!(report.entities.len() > 1);
    assert!(elapsed.as_secs_f64() < 1.0, "{elapsed:?}");
}

/// The `ludenea::Collision` events that begin during the tick ending at `tick`, as pairs.
fn collisions(host: &mut Host, tick: u64) -> Vec<(Entity, Entity)> {
    let Reply::TickEvents { events } = ok(host, Command::GetTickEvents { tick }) else {
        panic!("get_tick_events replies tick events");
    };
    events
        .into_iter()
        .filter(|event| event.value.get("kind") == Some(&json!("Begin")))
        .filter_map(|event| {
            let entity = |key| serde_json::from_value(event.value.get(key)?.clone()).ok();
            Some((entity("a")?, entity("b")?))
        })
        .collect()
}

#[test]
fn a_collision_at_the_fork_tick_explains_a_divergence_one_tick_later() {
    let mut host = Host::new(boxes::app).unwrap();
    load(&mut host, boxes::generate_scene(30, 7), Vec::new());
    ok(
        &mut host,
        Command::Subscribe {
            topic: Topic::Introspection,
        },
    );
    step(&mut host, 260);
    // A crate that starts touching another body during the tick ending at the fork tick:
    // pushing it there moves the other one at the next tick.
    let mut found = None;
    for tick in 100..250 {
        for (a, b) in collisions(&mut host, tick) {
            found.get_or_insert((tick, a, b));
        }
    }
    let (fork, a, b) = found.expect("a collision begins");
    seek(&mut host, fork);
    push(&mut host, a);
    step(&mut host, 30);
    let report = complete(&mut host, None);
    let other = entity(&report, b);
    assert_eq!(other.first_tick, fork + 1, "{report:?}");
    assert!(
        other.causes.iter().any(|cause| matches!(
            cause,
            Cause::Event { tick, event, other, .. }
                if *tick == fork && event == "ludenea::Collision" && *other == a
        )),
        "{other:?}"
    );
}
