use ludenea_app::{App, Inputs, Rng, Sim, SimError, Simulation, Time};
use ludenea_ecs::schema::Schema;
use ludenea_ecs::{
    Commands, Component, IntoSystemConfig, Query, Res, ResMut, Resource, Runtime, SystemLabel,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Schema, Default, Clone, Debug, PartialEq)]
struct Pad {
    jump: bool,
    steer: i8,
}

#[derive(Component, Serialize, Deserialize)]
struct Score(u32);

#[derive(Resource, Serialize, Deserialize, Default)]
struct Log(Vec<String>);

struct First;

impl SystemLabel for First {
    fn name(&self) -> &'static str {
        "first"
    }
}

fn second(time: Res<Time>, inputs: Runtime<Inputs<Pad>>, mut log: ResMut<Log>) {
    let steer = inputs.player(0).steer;
    log.0.push(format!("second {} {steer}", time.tick));
}

fn first(time: Res<Time>, mut log: ResMut<Log>) {
    log.0.push(format!("first {}", time.tick));
}

#[test]
fn step_advances_tick_and_runs_systems_in_order() {
    let mut app = App::new();
    app.input::<Pad>()
        .resource(Log::default())
        .system(Sim, second.after(First))
        .system(Sim, first.in_set(First));
    let mut simulation = app.build().unwrap();
    assert_eq!(simulation.tick(), 0);
    assert_eq!(
        simulation.world().resource::<Time>().unwrap().dt,
        1.0 / 60.0
    );

    simulation.step();
    simulation
        .step_with(&[Pad {
            jump: true,
            steer: -1,
        }])
        .unwrap();

    assert_eq!(simulation.tick(), 2);
    let log = simulation.world().resource::<Log>().unwrap();
    assert_eq!(log.0, ["first 0", "second 0 0", "first 1", "second 1 -1"]);
}

fn spawn_random(mut commands: Commands, mut rng: ResMut<Rng>, inputs: Runtime<Inputs<Pad>>) {
    let bonus = u32::from(inputs.player(0).jump);
    commands
        .spawn()
        .insert(Score(rng.range_u32(0, 1000) + bonus));
}

fn grow(mut scores: Query<&mut Score>, mut rng: ResMut<Rng>) {
    for score in scores.iter_mut() {
        score.0 += rng.range_u32(1, 4);
    }
}

fn random_simulation() -> Simulation {
    let mut app = App::new();
    app.input::<Pad>()
        .component::<Score>()
        .resource(Rng::new(7, 3))
        .system(Sim, spawn_random)
        .system(Sim, grow);
    app.build().unwrap()
}

fn pad(tick: u64) -> [Pad; 1] {
    [Pad {
        jump: tick.is_multiple_of(3),
        steer: 0,
    }]
}

#[test]
fn same_inputs_same_hash_twice() {
    let mut first = random_simulation();
    let mut second = random_simulation();
    for tick in 0..30 {
        first.step_with(&pad(tick)).unwrap();
        second.step_with(&pad(tick)).unwrap();
    }
    assert_eq!(first.hash(), second.hash());
    assert_eq!(first.hash_tree(), second.hash_tree());
}

#[test]
fn snapshot_restore_then_replay_matches_uninterrupted_run() {
    let mut uninterrupted = random_simulation();
    let mut snapshot = None;
    for tick in 0..100 {
        if tick == 50 {
            snapshot = Some(uninterrupted.snapshot());
        }
        uninterrupted.step_with(&pad(tick)).unwrap();
    }

    let mut replay = random_simulation();
    replay.restore(&snapshot.unwrap()).unwrap();
    assert_eq!(replay.tick(), 50);
    for tick in 50..100 {
        replay.step_with(&pad(tick)).unwrap();
    }

    assert_eq!(replay.tick(), 100);
    assert_eq!(replay.hash(), uninterrupted.hash());
}

#[test]
fn step_with_wrong_input_type_is_type_mismatch() {
    let mut simulation = random_simulation();
    let error = simulation.step_with(&[0u8]).unwrap_err();
    assert!(
        matches!(error, SimError::InputTypeMismatch { expected } if expected.ends_with("Pad")),
        "{error:?}"
    );
    assert_eq!(simulation.tick(), 0);
}

#[test]
fn step_encoded_without_input_type_is_no_input_type() {
    let mut simulation = App::new().build().unwrap();
    let error = simulation.step_encoded(Some(&[0])).unwrap_err();
    assert!(matches!(error, SimError::NoInputType), "{error:?}");
    assert_eq!(simulation.tick(), 0);
}

#[test]
fn step_encoded_decodes_inputs_and_rejects_trailing_bytes() {
    let mut simulation = random_simulation();
    let mut bytes = postcard::to_allocvec(&vec![Pad::default()]).unwrap();
    simulation.step_encoded(Some(&bytes)).unwrap();
    simulation.step_encoded(None).unwrap();
    bytes.push(0);
    let error = simulation.step_encoded(Some(&bytes)).unwrap_err();
    assert!(matches!(error, SimError::InputDecode(_)), "{error:?}");
    assert_eq!(simulation.tick(), 2);
}

#[test]
fn duplicate_names_fail_at_build() {
    #[derive(Resource, Serialize, Deserialize)]
    #[resource(name = "ludenea::Time")]
    struct FakeTime;

    let mut app = App::new();
    app.resource(FakeTime);
    assert!(matches!(app.build(), Err(ludenea_app::AppError::Ecs(_))));
}

#[test]
fn simulation_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Simulation>();
}

#[test]
fn app_registers_asset_types_and_reports_name_clashes_at_build() {
    #[derive(ludenea_ecs::Asset, Serialize, Deserialize, Default)]
    #[asset(name = "test::Stats")]
    struct Stats(u32);

    #[derive(ludenea_ecs::Asset, Serialize, Deserialize)]
    #[asset(name = "test::Stats")]
    struct Clash(u32);

    let mut app = App::new();
    app.asset_with_default::<Stats>();
    let simulation = app.build().unwrap();
    let world = simulation.world();
    assert_eq!(world.default_asset_ron("test::Stats").unwrap(), "(0)");
    assert!(world.asset_codec("ludenea::Prefab").is_some());

    let mut app = App::new();
    app.asset::<Stats>().asset::<Clash>();
    assert!(matches!(app.build(), Err(ludenea_app::AppError::Ecs(_))));
}
