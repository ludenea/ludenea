use ludenea_app::{App, SavedState, Sim, Simulation, StateError};
use ludenea_ecs::{Commands, Component, Query, ResMut, Resource};
use serde::{Deserialize, Serialize};

#[derive(Clone, Component, Serialize, Deserialize)]
struct Counter(u32);

#[derive(Clone, Component, Serialize, Deserialize)]
struct Extra;

#[derive(Clone, Resource, Serialize, Deserialize, Default)]
struct Steps(u64);

fn count(mut counters: Query<&mut Counter>, mut steps: ResMut<Steps>, mut commands: Commands) {
    for counter in counters.iter_mut() {
        counter.0 += 1;
    }
    steps.0 += 1;
    if steps.0.is_multiple_of(3) {
        commands.spawn().insert(Counter(0));
    }
}

fn app() -> App {
    let mut app = App::new();
    app.component::<Counter>()
        .resource(Steps::default())
        .system(Sim, count);
    app
}

fn simulation() -> Simulation {
    app().build().unwrap()
}

#[test]
fn load_state_restores_the_saved_tick() {
    let mut simulation = simulation();
    for _ in 0..4 {
        simulation.step().unwrap();
    }
    let mut saved = SavedState::default();
    simulation.save_state(&mut saved);
    let expected = simulation.snapshot().encode();
    for _ in 0..5 {
        simulation.step().unwrap();
    }
    simulation.load_state(&saved).unwrap();
    assert_eq!(simulation.tick(), 4);
    assert_eq!(simulation.snapshot().encode(), expected);
}

#[test]
fn load_state_refuses_another_game() {
    let mut other = app();
    other.component::<Extra>();
    let other = other.build().unwrap();
    let mut saved = SavedState::default();
    other.save_state(&mut saved);

    let mut simulation = simulation();
    simulation.step().unwrap();
    let before = simulation.snapshot();
    let error = simulation.load_state(&saved).unwrap_err();
    let (saved_fingerprint, current) = (
        other.initial_snapshot().fingerprint,
        simulation.initial_snapshot().fingerprint,
    );
    assert!(
        matches!(error, StateError::OtherWorld { saved, current: now }
            if saved == saved_fingerprint && now == current),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains(&format!("{saved_fingerprint:016x}"))
            && message.contains(&format!("{current:016x}")),
        "{message}"
    );
    assert_eq!(simulation.snapshot(), before);
}

#[test]
fn load_state_refuses_empty() {
    let mut simulation = simulation();
    simulation.step().unwrap();
    let before = simulation.snapshot();
    let error = simulation.load_state(&SavedState::default()).unwrap_err();
    assert!(matches!(error, StateError::Empty), "{error:?}");
    assert_eq!(simulation.snapshot(), before);
}
