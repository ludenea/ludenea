//! SyncTest on small test apps: a deterministic one passes, and state kept outside the world
//! fails naming the tick, the entities or resources, and the fields that differ.

use std::sync::atomic::{AtomicU32, Ordering};

use ludenea_app::divergence::DivergenceKind;
use ludenea_app::{App, EditLog, InputLog, Inputs, Sim, Simulation};
use ludenea_ecs::{Component, Query, ResMut, Resource, Runtime};
use ludenea_net::{LISTED_ENTITIES, SyncTestError, SyncTestReport, sync_test};
use serde::{Deserialize, Serialize};

#[derive(Clone, Component, Serialize, Deserialize)]
#[component(name = "sync_test::Counter")]
struct Counter {
    value: u32,
}

#[derive(Clone, Resource, Serialize, Deserialize)]
#[resource(name = "sync_test::Total")]
struct Total {
    value: u32,
}

/// Adds each tick's input to every counter: deterministic.
fn count(inputs: Runtime<Inputs<u8>>, mut counters: Query<&mut Counter>) {
    let step = u32::from(*inputs.player(0));
    for counter in counters.iter_mut() {
        counter.value += step + 1;
    }
}

fn deterministic() -> App {
    let mut app = App::new();
    app.input::<u8>().component::<Counter>().system(Sim, count);
    app
}

/// Counts the ticks the process ran, outside any world: a rollback runs more ticks than a
/// straight run, so the state it writes differs. Systems cannot read a runtime resource
/// inserted after `App::build`, and the app has no other place for state outside the world
/// than a static.
static RAN_IN_COMPONENT: AtomicU32 = AtomicU32::new(0);
static RAN_IN_RESOURCE: AtomicU32 = AtomicU32::new(0);

fn ticks_ran_into_component(mut counters: Query<&mut Counter>) {
    let ran = RAN_IN_COMPONENT.fetch_add(1, Ordering::Relaxed) + 1;
    for counter in counters.iter_mut() {
        counter.value = ran;
    }
}

fn ticks_ran_into_resource(mut total: ResMut<Total>) {
    total.value = RAN_IN_RESOURCE.fetch_add(1, Ordering::Relaxed) + 1;
}

fn outside_in_component() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .component::<Counter>()
        .system(Sim, ticks_ran_into_component);
    app
}

fn outside_in_resource() -> App {
    let mut app = App::new();
    app.input::<u8>()
        .component::<Counter>()
        .resource(Total { value: 0 })
        .system(Sim, ticks_ran_into_resource);
    app
}

const SCENE: &str = concat!(
    r#"Scene(version: 1, resources: {}, entities: [{ "sync_test::Counter": (value: 0) }, "#,
    r#"{ "sync_test::Counter": (value: 7) }])"#,
);

/// Two simulations of `app` with the test scene loaded.
fn pair(app: fn() -> App) -> (Simulation, Simulation) {
    pair_with(app, SCENE)
}

fn pair_with(app: fn() -> App, scene: &str) -> (Simulation, Simulation) {
    let load = || {
        let mut sim = app().build().unwrap();
        sim.load_scene(scene).unwrap();
        sim
    };
    (load(), load())
}

/// Inputs that change every few ticks, so a misprediction differs from the real input.
fn inputs(ticks: u8) -> InputLog {
    let mut log = InputLog::new(1);
    for tick in 0..ticks {
        log.record(&[tick / 3 % 4]);
    }
    log
}

#[test]
fn a_deterministic_game_passes_synctest() {
    for window in [1, 8, 30] {
        let (mut sim, mut other) = pair(deterministic);
        let report = sync_test(
            &mut sim,
            &mut other,
            &inputs(30),
            &EditLog::new(),
            30,
            window,
        )
        .unwrap_or_else(|error| panic!("window {window}: {error}"));
        assert_eq!(
            report,
            SyncTestReport {
                ticks: 30,
                rollbacks: 30
            }
        );
        assert_eq!(sim.tick(), 30);
        assert_eq!(sim.hash(), other.hash(), "window {window}");
    }
}

#[test]
fn state_kept_outside_the_world_fails_synctest() {
    let (mut sim, mut other) = pair(outside_in_component);
    let error = sync_test(&mut sim, &mut other, &inputs(20), &EditLog::new(), 20, 4).unwrap_err();
    let SyncTestError::Mismatch {
        tick,
        straight,
        rollback,
        entities,
        resources,
        snapshots,
        ..
    } = &error
    else {
        panic!("expected a mismatch, got {error}");
    };
    assert_eq!(*tick, 1);
    assert_ne!(straight, rollback);
    assert_eq!(entities.len(), 2, "{entities:?}");
    for divergence in entities {
        assert_eq!(divergence.kind, DivergenceKind::Changed);
        assert_eq!(divergence.first_tick, 1);
        let difference = divergence.first_difference.as_ref().unwrap();
        assert_eq!(difference.component, "sync_test::Counter");
        assert_eq!(difference.path, ["value"]);
    }
    // Only the counters differ: the resources, `Time` among them, agree.
    assert_eq!(resources, &[]);
    // Both simulations are left at their state of the divergent tick.
    assert_eq!((other.tick(), other.hash()), (1, *straight));
    assert_eq!((sim.tick(), sim.hash()), (1, *rollback));
    // The straight state of the tick first, then the rolled back one.
    for (snapshot, hash) in snapshots.iter().zip([*straight, *rollback]) {
        sim.restore(snapshot).unwrap();
        assert_eq!((sim.tick(), sim.hash()), (1, hash));
    }

    let message = error.to_string();
    let entity = entities[0].entity;
    assert!(message.contains("tick 1"), "{message}");
    assert!(message.contains(&format!("entity {entity}")), "{message}");
    assert!(message.contains("sync_test::Counter.value"), "{message}");
    assert!(!message.contains("more entities"), "{message}");
}

#[test]
fn a_mismatch_lists_ten_entities_and_counts_the_others() {
    let counters = [r#"{ "sync_test::Counter": (value: 0) }"#; LISTED_ENTITIES + 2];
    let scene = format!(
        "Scene(version: 1, resources: {{}}, entities: [{}])",
        counters.join(", ")
    );
    let (mut sim, mut other) = pair_with(outside_in_component, &scene);
    let error = sync_test(&mut sim, &mut other, &inputs(20), &EditLog::new(), 20, 4).unwrap_err();
    let SyncTestError::Mismatch { entities, .. } = &error else {
        panic!("expected a mismatch, got {error}");
    };
    assert_eq!(entities.len(), LISTED_ENTITIES + 2);
    let message = error.to_string();
    assert_eq!(
        message.matches("\n  entity ").count(),
        LISTED_ENTITIES,
        "{message}"
    );
    assert!(message.contains("and 2 more entities"), "{message}");
}

#[test]
fn state_kept_outside_the_world_in_a_resource_names_the_resource() {
    let (mut sim, mut other) = pair(outside_in_resource);
    let error = sync_test(&mut sim, &mut other, &inputs(20), &EditLog::new(), 20, 4).unwrap_err();
    let SyncTestError::Mismatch {
        tick,
        entities,
        resources,
        ..
    } = &error
    else {
        panic!("expected a mismatch, got {error}");
    };
    assert_eq!(*tick, 1);
    assert_eq!(entities, &[]);
    assert_eq!(resources.len(), 1, "{resources:?}");
    assert_eq!(resources[0].component, "sync_test::Total");
    assert_eq!(resources[0].path, ["value"]);
    let message = error.to_string();
    assert!(message.contains("sync_test::Total.value"), "{message}");
    assert!(!message.contains("hash differs"), "{message}");
}

#[test]
fn window_out_of_range_is_refused() {
    for (window, ticks) in [(0, 10), (11, 10), (1, 0)] {
        let (mut sim, mut other) = pair(deterministic);
        let error = sync_test(
            &mut sim,
            &mut other,
            &inputs(10),
            &EditLog::new(),
            ticks,
            window,
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                SyncTestError::Window { window: w, ticks: t } if w == window && t == ticks
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains(&window.to_string()), "{error}");
        assert_eq!(sim.tick(), 0, "nothing ran");
    }
}

#[test]
fn simulations_that_start_apart_are_refused() {
    let (mut sim, mut other) = pair(deterministic);
    other.step_encoded(None).unwrap();
    let error = sync_test(&mut sim, &mut other, &inputs(10), &EditLog::new(), 10, 2).unwrap_err();
    assert!(matches!(error, SyncTestError::Start { .. }), "{error:?}");
    assert_eq!(sim.tick(), 0, "nothing ran");
}
