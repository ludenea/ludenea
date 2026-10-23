mod support;

use ludenea_app::{App, StateError, join_player_inputs};
use ludenea_net::{ExecuteError, Executor, Request, Session, SessionSettings};
use support::{handshake, identity, settings, test_simulation};

/// The settings of peer 0 in a match of the test app where player `i` is owned by
/// `owners[i]`.
fn test_settings(owners: &[u8]) -> SessionSettings {
    settings(0, &identity(owners))
}

/// Peer 0's session of a two-peer match of the test app, running at tick 0.
fn running_session() -> Session {
    let identity = identity(&[0, 1]);
    let spec = test_simulation().player_input_spec().unwrap();
    let sessions = (0..2)
        .map(|local| Session::new(settings(local, &identity), spec.clone()).unwrap())
        .collect();
    handshake(sessions).remove(0)
}

#[test]
fn joined_inputs_equal_the_postcard_vec() {
    // Inputs of several bytes, and enough of them for the length to take two varint bytes.
    let cases: Vec<Vec<u16>> = vec![
        Vec::new(),
        vec![7],
        vec![0, 300, u16::MAX, 128],
        (0..200).collect(),
    ];
    for values in cases {
        let encoded: Vec<Vec<u8>> = values
            .iter()
            .map(|value| postcard::to_allocvec(value).unwrap())
            .collect();
        assert_eq!(
            join_player_inputs(&encoded),
            postcard::to_allocvec(&values).unwrap(),
            "{} inputs",
            values.len()
        );
    }
}

#[test]
fn executor_sets_the_player_count() {
    let mut simulation = test_simulation();
    assert_eq!(simulation.players(), 1);
    Executor::new(&mut simulation, &test_settings(&[0, 1, 1])).unwrap();
    assert_eq!(simulation.players(), 3);
}

#[test]
fn executor_refuses_an_app_without_an_input_type() {
    let mut simulation = App::new().build().unwrap();
    assert!(matches!(
        Executor::new(&mut simulation, &test_settings(&[0, 1])),
        Err(ExecuteError::NoInputType)
    ));
}

#[test]
fn execute_refuses_out_of_step_and_missing_states() {
    let mut simulation = test_simulation();
    let mut session = running_session();
    let mut executor = Executor::new(&mut simulation, session.settings()).unwrap();
    let mut execute = |simulation: &mut _, requests: Vec<Request>| {
        executor.execute(simulation, &mut session, requests)
    };

    // A save or an advance of another tick than the simulation's.
    let error = execute(
        &mut simulation,
        vec![Request::Save {
            tick: 1,
            hash: false,
        }],
    );
    assert!(
        matches!(
            error,
            Err(ExecuteError::OutOfStep {
                expected: 1,
                found: 0
            })
        ),
        "{error:?}"
    );
    let advance = |tick| Request::Advance {
        tick,
        inputs: vec![vec![1], vec![2]],
    };
    let error = execute(&mut simulation, vec![advance(3)]);
    assert!(
        matches!(
            error,
            Err(ExecuteError::OutOfStep {
                expected: 3,
                found: 0
            })
        ),
        "{error:?}"
    );
    assert_eq!(simulation.tick(), 0);

    // A load of a tick never saved.
    let error = execute(&mut simulation, vec![Request::Load { tick: 0 }]);
    assert!(
        matches!(error, Err(ExecuteError::MissingState { tick: 0 })),
        "{error:?}"
    );

    // Saving more ticks than the ring holds reuses the slot of tick 0 for a later tick.
    let capacity = executor_capacity();
    let saved: Vec<Request> = (0..=u64::try_from(capacity).unwrap())
        .flat_map(|tick| [Request::Save { tick, hash: false }, advance(tick)])
        .collect();
    execute(&mut simulation, saved).unwrap();
    let error = execute(&mut simulation, vec![Request::Load { tick: 0 }]);
    assert!(
        matches!(error, Err(ExecuteError::MissingState { tick: 0 })),
        "{error:?}"
    );
    // The latest tick saved is held: loading it goes back to its state.
    let latest = u64::try_from(capacity).unwrap();
    let mut replay = test_simulation();
    replay.set_players(2);
    for _ in 0..latest {
        replay
            .step_encoded(Some(&join_player_inputs(&[vec![1], vec![2]])))
            .unwrap();
    }
    execute(&mut simulation, vec![Request::Load { tick: latest }]).unwrap();
    assert_eq!(simulation.tick(), latest);
    assert_eq!(simulation.hash(), replay.hash());

    // A state saved by another simulation is refused when loaded, as a `StateError`.
    let mut other = App::new();
    other.input::<u16>();
    let mut other = other.build().unwrap();
    let error = execute(&mut other, vec![Request::Load { tick: latest }]);
    assert!(
        matches!(
            error,
            Err(ExecuteError::State(StateError::OtherWorld { .. }))
        ),
        "{error:?}"
    );
}

/// The states an executor of [`running_session`]'s match holds at most.
fn executor_capacity() -> usize {
    Executor::new(&mut test_simulation(), running_session().settings())
        .unwrap()
        .capacity()
}

#[test]
fn executor_capacity_follows_the_rules() {
    // `rollback_window + 2` states, plus the reported states the hash bounds let a session
    // keep: `(2 * window + delay + 3 * interval - 2) / interval + 3`.
    let mut identity = identity(&[0, 1]);
    for (window, delay, interval, expected) in [(8, 2, 30, 10 + 6), (1, 0, 1, 3 + 6)] {
        identity.rules.rollback_window = window;
        identity.rules.input_delay = delay;
        identity.rules.hash_interval = interval;
        let executor = Executor::new(&mut test_simulation(), &settings(0, &identity)).unwrap();
        assert_eq!(
            executor.capacity(),
            expected,
            "window {window}, delay {delay}, interval {interval}"
        );
    }
}
