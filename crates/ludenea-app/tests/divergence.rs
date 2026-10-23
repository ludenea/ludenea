//! The per-entity divergence report between two boxes simulations. An integration test:
//! `boxes` links this crate, so its simulation type is this one only here.

use ludenea_app::Simulation;
use ludenea_app::divergence::{
    Cause, ComparisonError, DivergenceKind, EntityComparison, TickRecord, TickRecorder,
};
use ludenea_app::pose::Pose;

/// Two simulations of the same boxes scene, at tick 0.
fn twins() -> [Simulation; 2] {
    let scene = boxes::generate_scene(4, 7);
    [(), ()].map(|()| {
        let mut sim = boxes::app().build().unwrap();
        sim.load_scene(&scene).unwrap();
        sim
    })
}

/// Steps `sim` once with `input`, recording the tick.
fn step(sim: &mut Simulation, input: &boxes::BoxesInput) -> TickRecord {
    let encoded = postcard::to_allocvec(&vec![input.clone()]).unwrap();
    let mut recorder = TickRecorder::default();
    sim.step_observed(Some(&encoded), &mut recorder).unwrap();
    recorder.into_record(sim.tick())
}

/// Two simulations one tick in, the first having spawned a crate, and their records.
fn spawned_on_one_side() -> ([Simulation; 2], [TickRecord; 2]) {
    let [mut current, mut compare] = twins();
    let spawn = boxes::BoxesInput {
        spawn: Some(0.0),
        ..boxes::BoxesInput::default()
    };
    let records = [
        step(&mut current, &spawn),
        step(&mut compare, &boxes::BoxesInput::default()),
    ];
    ([current, compare], records)
}

#[test]
fn a_moved_entity_is_changed_with_its_first_difference() {
    let [mut current, compare] = twins();
    let mut comparison = EntityComparison::new(&current);
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

    let divergences = comparison
        .compare([&current, &compare], [&[], &[]], 0)
        .unwrap();

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
    let again = comparison
        .compare([&current, &compare], [&[], &[]], 0)
        .unwrap();
    assert_eq!(again, []);
}

#[test]
fn an_entity_spawned_on_one_side_only_is_caused_by_its_command() {
    let ([current, compare], records) = spawned_on_one_side();
    let tick = current.tick();
    let records = [
        std::slice::from_ref(&records[0]),
        std::slice::from_ref(&records[1]),
    ];
    let spawn = Cause::Command {
        tick,
        system: "boxes::apply_inputs".to_string(),
        kind: "spawn".to_string(),
    };

    // The divergence at `explains_from` itself is explained.
    for explains_from in [0, tick] {
        let mut comparison = EntityComparison::new(&current);
        let divergences = comparison
            .compare([&current, &compare], records, explains_from)
            .unwrap();
        let spawned = divergences
            .iter()
            .find(|divergence| divergence.kind == DivergenceKind::OnlyInCurrent)
            .unwrap_or_else(|| panic!("no entity only in current: {divergences:?}"));
        assert_eq!(spawned.first_tick, tick);
        assert_eq!(spawned.first_difference, None);
        assert!(spawned.causes.contains(&spawn), "{:?}", spawned.causes);
        assert_eq!(comparison.first_divergent(), Some(tick));
    }

    // A divergence before `explains_from` has no cause.
    let mut late = EntityComparison::new(&current);
    let divergences = late
        .compare([&current, &compare], records, tick + 1)
        .unwrap();
    assert!(!divergences.is_empty());
    assert!(
        divergences
            .iter()
            .all(|divergence| divergence.causes.is_empty())
    );
}

#[test]
fn the_second_simulation_alone_is_only_in_compare() {
    let ([current, compare], _) = spawned_on_one_side();
    let mut comparison = EntityComparison::new(&current);
    let divergences = comparison
        .compare([&compare, &current], [&[], &[]], 0)
        .unwrap();
    assert!(
        divergences
            .iter()
            .any(|divergence| divergence.kind == DivergenceKind::OnlyInCompare),
        "{divergences:?}"
    );
}

#[test]
fn simulations_at_different_ticks_are_refused() {
    let [mut current, compare] = twins();
    current.step().unwrap();
    let mut comparison = EntityComparison::new(&current);
    let error = comparison
        .compare([&current, &compare], [&[], &[]], 0)
        .unwrap_err();
    assert!(matches!(
        error,
        ComparisonError::DifferentTicks {
            first: 1,
            second: 0
        }
    ));
    assert_eq!(
        error.to_string(),
        "the simulations to compare stand at different ticks: 1 and 0"
    );
    assert_eq!(comparison.first_divergent(), None);
}

#[test]
fn identical_simulations_do_not_diverge() {
    let [current, compare] = twins();
    let mut comparison = EntityComparison::new(&current);
    let divergences = comparison
        .compare([&current, &compare], [&[], &[]], 0)
        .unwrap();
    assert_eq!(divergences, []);
    assert_eq!(comparison.first_divergent(), None);
}
