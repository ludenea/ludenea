//! The per-entity divergence report between two boxes simulations. An integration test:
//! `boxes` links this crate, so its simulation type is this one only here.

use std::collections::BTreeMap;

use ludenea_app::Simulation;
use ludenea_app::divergence::{Cause, DivergenceKind, EntityComparison, TickRecord, TickRecorder};
use ludenea_app::pose::Pose;
use ludenea_ecs::schema::TypeSchema;

/// Two simulations of the same boxes scene, at tick 0.
fn twins() -> [Simulation; 2] {
    let scene = boxes::generate_scene(4, 7);
    [(), ()].map(|()| {
        let mut sim = boxes::app().build().unwrap();
        sim.load_scene(&scene).unwrap();
        sim
    })
}

/// The stable names of the app's events and its systems, in run order.
fn context(sim: &Simulation) -> (BTreeMap<&'static str, TypeSchema>, Vec<&'static str>) {
    let events = sim.world().event_schemas().into_iter().collect();
    let systems = sim.systems().into_iter().map(|info| info.name).collect();
    (events, systems)
}

/// Steps `sim` once with `input`, recording the tick.
fn step(sim: &mut Simulation, input: &boxes::BoxesInput) -> TickRecord {
    let encoded = postcard::to_allocvec(&vec![input.clone()]).unwrap();
    let mut recorder = TickRecorder::default();
    sim.step_observed(Some(&encoded), &mut recorder).unwrap();
    recorder.into_record(sim.tick())
}

#[test]
fn a_moved_entity_is_changed_with_its_first_difference() {
    let [mut current, compare] = twins();
    let mut comparison = EntityComparison::new(&current);
    let (events, systems) = context(&current);
    let moved = current
        .world()
        .entities()
        .find(|&entity| current.world().get::<Pose>(entity).is_some())
        .unwrap();
    current
        .world_mut()
        .get_mut::<Pose>(moved)
        .unwrap()
        .position
        .x += 1.0;

    let divergences = comparison.compare([&current, &compare], 0, [&[], &[]], 0, &events, &systems);

    assert_eq!(divergences.len(), 1, "{divergences:?}");
    let divergence = &divergences[0];
    assert_eq!(divergence.entity, moved);
    assert_eq!(divergence.kind, DivergenceKind::Changed);
    assert_eq!(divergence.first_tick, 0);
    let difference = divergence.first_difference.as_ref().unwrap();
    assert_eq!(difference.component, "ludenea::Pose");
    assert_eq!(difference.path, ["position", "x"]);
    assert_eq!(divergence.causes, []);
    assert_eq!(comparison.first_divergent(), Some(0));
    assert_eq!(comparison.take_warnings(), Vec::<String>::new());

    // An entity is reported once, at its first divergent tick.
    let again = comparison.compare([&current, &compare], 0, [&[], &[]], 0, &events, &systems);
    assert_eq!(again, []);
}

#[test]
fn an_entity_spawned_on_one_side_only_is_caused_by_its_command() {
    let [mut current, mut compare] = twins();
    let mut comparison = EntityComparison::new(&current);
    let (events, systems) = context(&current);
    let spawn = boxes::BoxesInput {
        spawn: Some(0.0),
        ..boxes::BoxesInput::default()
    };
    let records = [
        step(&mut current, &spawn),
        step(&mut compare, &boxes::BoxesInput::default()),
    ];
    let tick = current.tick();
    let records = [
        std::slice::from_ref(&records[0]),
        std::slice::from_ref(&records[1]),
    ];

    let divergences = comparison.compare([&current, &compare], tick, records, 0, &events, &systems);

    let spawned = divergences
        .iter()
        .find(|divergence| divergence.kind == DivergenceKind::OnlyInCurrent)
        .unwrap_or_else(|| panic!("no entity only in current: {divergences:?}"));
    assert_eq!(spawned.first_tick, tick);
    assert_eq!(spawned.first_difference, None);
    assert!(
        spawned.causes.contains(&Cause::Command {
            tick,
            system: "boxes::apply_inputs".to_string(),
            kind: "spawn".to_string(),
        }),
        "{:?}",
        spawned.causes
    );
    assert_eq!(comparison.first_divergent(), Some(tick));

    // A divergence before `explains_from` has no cause.
    let mut late = EntityComparison::new(&current);
    let divergences = late.compare(
        [&current, &compare],
        tick,
        records,
        tick + 1,
        &events,
        &systems,
    );
    assert!(
        divergences
            .iter()
            .all(|divergence| divergence.causes.is_empty())
    );
}

#[test]
fn identical_simulations_do_not_diverge() {
    let [current, compare] = twins();
    let mut comparison = EntityComparison::new(&current);
    let (events, systems) = context(&current);
    let divergences = comparison.compare([&current, &compare], 0, [&[], &[]], 0, &events, &systems);
    assert_eq!(divergences, []);
    assert_eq!(comparison.first_divergent(), None);
}
