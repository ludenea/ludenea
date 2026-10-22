use ludenea_ecs::{
    BuildError, IntoSystemConfig, ResMut, Resource, ScheduleBuilder, SystemLabel, World,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Resource, Serialize, Deserialize, Default)]
struct Log(Vec<String>);

enum Stage {
    Early,
    Late,
    Missing,
}

impl SystemLabel for Stage {
    fn name(&self) -> &'static str {
        match self {
            Stage::Early => "early",
            Stage::Late => "late",
            Stage::Missing => "missing",
        }
    }
}

fn alpha(mut log: ResMut<Log>) {
    log.0.push("alpha".into());
}

fn beta(mut log: ResMut<Log>) {
    log.0.push("beta".into());
}

fn gamma(mut log: ResMut<Log>) {
    log.0.push("gamma".into());
}

fn world() -> World {
    let mut world = World::new();
    world.register_resource(Log::default()).unwrap();
    world
}

fn run_log(systems: impl FnOnce(&mut ScheduleBuilder)) -> Vec<String> {
    let mut world = world();
    let mut builder = ScheduleBuilder::default();
    systems(&mut builder);
    let Ok(mut schedule) = builder.build(&world) else {
        panic!("schedule should build");
    };
    schedule.run(&mut world);
    world.resource_mut::<Log>().unwrap().0.clone()
}

fn build_error(systems: impl FnOnce(&mut ScheduleBuilder)) -> BuildError {
    let mut builder = ScheduleBuilder::default();
    systems(&mut builder);
    builder.build(&world()).err().expect("build should fail")
}

#[test]
fn unordered_systems_run_in_registration_order() {
    let log = run_log(|builder| {
        builder.add(gamma);
        builder.add(alpha);
        builder.add(beta);
    });
    assert_eq!(log, ["gamma", "alpha", "beta"]);
}

#[test]
fn after_constraint_reorders() {
    let log = run_log(|builder| {
        builder.add(alpha.after(Stage::Late));
        builder.add(beta);
        builder.add(gamma.in_set(Stage::Late));
    });
    assert_eq!(log, ["beta", "gamma", "alpha"]);
}

#[test]
fn cycle_is_reported_with_system_names() {
    let error = build_error(|builder| {
        builder.add(gamma);
        builder.add(alpha.in_set(Stage::Early).after(Stage::Late));
        builder.add(beta.in_set(Stage::Late).after(Stage::Early));
        builder.add(gamma.after(Stage::Late));
    });
    let BuildError::OrderingCycle { systems } = &error else {
        panic!("expected an ordering cycle, got {error}");
    };
    assert_eq!(systems.len(), 2, "{systems:?}");
    assert!(systems[0].ends_with("alpha") && systems[1].ends_with("beta"));
    let message = error.to_string();
    assert!(
        message.contains("alpha") && message.contains("beta"),
        "{message}"
    );
}

#[test]
fn after_if_present_orders_after_a_present_label() {
    let log = run_log(|builder| {
        builder.add(alpha.after_if_present(Stage::Late));
        builder.add(beta);
        builder.add(gamma.in_set(Stage::Late));
    });
    assert_eq!(log, ["beta", "gamma", "alpha"]);
}

#[test]
fn after_if_present_ignores_an_absent_label() {
    let log = run_log(|builder| {
        builder.add(gamma);
        builder.add(alpha.after_if_present(Stage::Missing));
        builder.add(beta);
    });
    assert_eq!(log, ["gamma", "alpha", "beta"]);
}

#[test]
fn after_unknown_label_is_empty_label_error() {
    let error = build_error(|builder| builder.add(alpha.after(Stage::Missing)));
    assert!(matches!(error, BuildError::EmptyLabel { label: "missing" }));
    assert_eq!(
        error.to_string(),
        "label `missing` has no systems. Did you forget to add the plugin that provides it?"
    );
}

#[test]
fn schedule_and_world_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<ludenea_ecs::Schedule>();
    assert_send::<World>();
}
