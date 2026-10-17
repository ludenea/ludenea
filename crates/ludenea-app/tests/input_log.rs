use ludenea_app::{App, INPUT_LOG_VERSION, InputLog, Inputs, Sim, SimError, Simulation};
use ludenea_ecs::{ResMut, Resource, Runtime};
use serde::{Deserialize, Serialize};

#[derive(Resource, Serialize, Deserialize, Default)]
struct Seen(Vec<u8>);

fn record(inputs: Runtime<Inputs<u8>>, mut seen: ResMut<Seen>) {
    seen.0.push(*inputs.player(0));
}

fn simulation() -> Simulation {
    let mut app = App::new();
    app.input::<u8>()
        .resource(Seen::default())
        .system(Sim, record);
    app.build().unwrap()
}

fn seen(simulation: &Simulation) -> Vec<u8> {
    simulation.world().resource::<Seen>().unwrap().0.clone()
}

#[test]
fn input_log_round_trips() {
    let mut log = InputLog::new(2);
    log.record(&[1u8, 2]);
    log.record(&[3u8, 4]);
    let decoded = InputLog::decode(&log.encode()).unwrap();
    assert_eq!(decoded, log);
    assert_eq!(decoded.version, INPUT_LOG_VERSION);
    assert_eq!(decoded.tick(1), Some(&[2, 3, 4][..]));
    assert_eq!(decoded.tick(2), None);
}

#[test]
fn input_log_with_another_version_is_rejected() {
    let mut log = InputLog::new(1);
    log.version = 9;
    let error = InputLog::decode(&log.encode()).unwrap_err();
    assert!(matches!(
        error,
        SimError::InputLogVersion {
            found: 9,
            expected: 1
        }
    ));
}

#[test]
fn input_log_shorter_than_run_uses_defaults() {
    let mut log = InputLog::new(1);
    for value in [7u8, 8, 9] {
        log.record(&[value]);
    }
    let mut simulation = simulation();
    for tick in 0..5 {
        simulation.step_encoded(log.tick(tick)).unwrap();
    }
    assert_eq!(seen(&simulation), [7, 8, 9, 0, 0]);
}

#[test]
fn input_log_for_another_type_is_decode_error() {
    let mut log = InputLog::new(1);
    log.record(&[u64::MAX]);
    let mut simulation = simulation();
    let error = simulation.step_encoded(log.tick(0)).unwrap_err();
    assert!(matches!(error, SimError::InputDecode(_)), "{error:?}");
    assert_eq!(simulation.tick(), 0);
}

#[test]
fn input_log_for_a_bool_reads_u64_max_as_error() {
    // `bool` rejects the value 0xFF, so the log recorded as `u64` fails instead of panicking.
    let mut log = InputLog::new(1);
    log.record(&[u64::MAX]);
    let mut app = App::new();
    app.input::<bool>();
    let mut simulation = app.build().unwrap();
    let error = simulation.step_encoded(log.tick(0)).unwrap_err();
    assert!(matches!(error, SimError::InputDecode(_)), "{error:?}");
}

#[test]
fn wrong_player_count_is_decode_error_and_changes_nothing() {
    let mut simulation = simulation();
    let before = simulation.hash();
    let error = simulation.step_with(&[1u8, 2]).unwrap_err();
    assert!(
        matches!(&error, SimError::InputDecode(message) if message.contains("expected 1 players, got 2")),
        "{error:?}"
    );
    let bytes = postcard::to_allocvec(&Vec::<u8>::new()).unwrap();
    let error = simulation.step_encoded(Some(&bytes)).unwrap_err();
    assert!(matches!(error, SimError::InputDecode(_)), "{error:?}");
    assert_eq!((simulation.tick(), simulation.hash()), (0, before));
}
