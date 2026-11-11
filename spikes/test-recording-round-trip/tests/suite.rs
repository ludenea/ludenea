//! Sixteen tests of the standard harness, each passing its name and its body
//! to the entry function, which writes the start marker and the record. One
//! fails on an invariant and leaves its recording in both forms; fifteen pass.

use std::fs;
use std::path::Path;

use test_recording_round_trip::run::{Failed, RUN_DIR_VARIABLE, Report, cell, entry};
use test_recording_round_trip::{Session, run_until_the_watched_group_thins, runtime, small_fixture};

/// The ticks a passing test steps, one per call.
const PASSING_TICKS: u32 = 120;
/// The bound of the failing test's run; its invariant fails long before.
const FAILING_BOUND: u32 = 2_000;

fn passing(name: &str, script: u32) {
    entry(name, |_| {
        let mut session = Session::start(small_fixture(), script);
        for _ in 0..PASSING_TICKS {
            session.step_one(runtime().pool());
        }
        let tick = session.world.tick();
        Ok(Report { final_tick: tick, final_hash: session.world.tick_hash(tick).expect("the last tick is hashed"), artifacts: Vec::new() })
    });
}

fn failing(name: &str, script: u32) {
    // Outside a run of the orchestrator the workspace's tests must pass: the
    // test then checks that the invariant fails, and leaves nothing.
    if std::env::var_os(RUN_DIR_VARIABLE).is_none() {
        let (_, failure) = run_until_the_watched_group_thins(small_fixture(), script, FAILING_BOUND, runtime().pool());
        assert!(failure.is_some(), "the invariant fails within the bound");
        return;
    }
    entry(name, |dir: &Path| {
        let fixture = small_fixture();
        let (session, failure) = run_until_the_watched_group_thins(fixture, script, FAILING_BOUND, runtime().pool());
        let tick = session.world.tick();
        let mut report = Report { final_tick: tick, final_hash: session.world.tick_hash(tick).expect("the last tick is hashed"), artifacts: Vec::new() };
        let Some(message) = failure else { return Ok(report) };
        for (reference_form, form) in [(false, "self-contained"), (true, "reference")] {
            let file = format!("{name}.{form}.rec");
            fs::write(dir.join(&file), session.recording(fixture, reference_form, name, &cell()).encode()).expect("the recording is writable");
            report.artifacts.push(file);
        }
        Err(Failed { message, report })
    });
}

macro_rules! passing_tests {
    ($($name:ident: $script:expr),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                passing(stringify!($name), $script);
            }
        )*
    };
}

passing_tests! {
    t00_a_scripted_session_steps_its_ticks: 1,
    t01_a_scripted_session_steps_its_ticks: 2,
    t02_a_scripted_session_steps_its_ticks: 3,
    t04_a_scripted_session_steps_its_ticks: 4,
    t05_a_scripted_session_steps_its_ticks: 5,
    t06_a_scripted_session_steps_its_ticks: 6,
    t07_a_scripted_session_steps_its_ticks: 7,
    t08_a_scripted_session_steps_its_ticks: 8,
    t09_a_scripted_session_steps_its_ticks: 9,
    t10_a_scripted_session_steps_its_ticks: 10,
    t11_a_scripted_session_steps_its_ticks: 11,
    t12_a_scripted_session_steps_its_ticks: 12,
    t13_a_scripted_session_steps_its_ticks: 13,
    t14_a_scripted_session_steps_its_ticks: 14,
    t15_a_scripted_session_steps_its_ticks: 15,
}

/// The one failing test: it sorts among the first so that a run ended from
/// outside after eight tests still holds its record.
#[test]
fn t03_the_watched_group_keeps_its_members() {
    failing("t03_the_watched_group_keeps_its_members", 3);
}
