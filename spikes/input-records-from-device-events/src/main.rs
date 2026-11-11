//! Input records from device events spike: the measurements. Throwaway, never
//! product code. One JSON document on standard output.

mod probe;

use std::env;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use input_records_from_device_events::{
    BUTTONS, CONTEXT_ON_FOOT, DeviceEvent, Domain, EventKind, Frames, Mapping, RECORD_BYTES, Record, TICK_NS, TickInputs, Verdict, run_host,
};
use reference_workload::{Json, SplitMix64, write_platform_and_build};

const SCHEMA_VERSION: u64 = 1;
const SPIKE: &str = "input-records-from-device-events";
const TICKS: u64 = 6_000;
const SEED: u64 = 0x1a9c_75ee_d000_0001;
const KEYS: [u8; BUTTONS] = [10, 11, 12, 13, 14, 15, 16, 17];
const EPOCH_A: u64 = 0x5eed_0000_0000_000a;
const EPOCH_B: u64 = 0x5eed_0000_0000_000b;
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// What the event stream holds, counted where it is generated.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Truth {
    presses: [u64; BUTTONS],
    releases: [u64; BUTTONS],
    look: (i64, i64),
}

impl Truth {
    fn plus(self, other: Truth) -> Truth {
        let mut sum = self;
        for action in 0..BUTTONS {
            sum.presses[action] += other.presses[action];
            sum.releases[action] += other.releases[action];
        }
        sum.look.0 += other.look.0;
        sum.look.1 += other.look.1;
        sum
    }

    fn of(domain: &Domain) -> Truth {
        Truth { presses: domain.presses_seen, releases: domain.releases_seen, look: domain.look_seen }
    }
}

fn between(generator: &mut SplitMix64, low: u64, high: u64) -> u64 {
    low + generator.next() % (high - low + 1)
}

/// A stream of key, stick and mouse events over `ticks` ticks: three presses
/// in ten are taps shorter than a tick, the mouse reports every millisecond.
fn generate(seed: u64, ticks: u64) -> (Vec<DeviceEvent>, Truth) {
    const MS: u64 = 1_000_000;
    let total = ticks * TICK_NS;
    let mut generator = SplitMix64::new(seed);
    let mut events = Vec::new();
    let mut truth = Truth::default();
    for (action, key) in KEYS.iter().enumerate() {
        let mut time = between(&mut generator, 0, 50 * MS);
        while time < total {
            let held = if generator.next() % 10 < 3 { between(&mut generator, MS, 10 * MS) } else { between(&mut generator, 20 * MS, 400 * MS) };
            events.push(DeviceEvent { time_ns: time, kind: EventKind::KeyDown(*key) });
            events.push(DeviceEvent { time_ns: time + held, kind: EventKind::KeyUp(*key) });
            truth.presses[action] += 1;
            truth.releases[action] += 1;
            time += held + between(&mut generator, 5 * MS, 300 * MS);
        }
    }
    let mut time = 0;
    while time < total {
        time += 8 * MS;
        let x = generator.u01() * 2.0 - 1.0;
        let y = generator.u01() * 2.0 - 1.0;
        events.push(DeviceEvent { time_ns: time, kind: EventKind::Stick { x, y } });
    }
    let mut time = 0;
    while time < total {
        time += MS;
        let dx = between(&mut generator, 0, 10) as i32 - 5;
        let dy = between(&mut generator, 0, 10) as i32 - 5;
        events.push(DeviceEvent { time_ns: time, kind: EventKind::MouseMotion { dx, dy } });
        truth.look.0 += i64::from(dx);
        truth.look.1 += i64::from(dy);
    }
    events.sort_by_key(|event| event.time_ns);
    (events, truth)
}

/// The input log of a run whose writer binds at the first tick.
fn log_of(baseline: Record, records: &[Record]) -> Vec<TickInputs> {
    records
        .iter()
        .enumerate()
        .map(|(index, record)| TickInputs { bind: (index == 0).then_some(baseline), record: Some(*record), ..TickInputs::default() })
        .collect()
}

fn play(domain: &mut Domain, log: &[TickInputs]) -> Vec<u64> {
    log.iter()
        .map(|inputs| {
            domain.step(inputs);
            domain.hash()
        })
        .collect()
}

struct Checks {
    every_check_holds: bool,
}

impl Checks {
    fn record(&mut self, out: &mut Json, name: &str, holds: bool) {
        out.bool(name, holds);
        self.every_check_holds &= holds;
    }
}

fn write_truth(out: &mut Json, name: &str, truth: &Truth) {
    out.key(name);
    out.object_start();
    out.u64("presses", truth.presses.iter().sum());
    out.u64("releases", truth.releases.iter().sum());
    out.u64("most_presses_of_one_action", *truth.presses.iter().max().unwrap_or(&0));
    out.i64("look_x", truth.look.0);
    out.i64("look_y", truth.look.1);
    out.object_end();
}

/// Presses counted exactly once at every ratio of frames to ticks, and where
/// each press lands with and without event timestamps.
fn frames_and_ticks(out: &mut Json, checks: &mut Checks, events: &[DeviceEvent], truth: &Truth) {
    let schedules: [(&str, Frames); 7] = [
        ("one frame per four ticks", Frames::Fixed { period_ns: TICK_NS * 4 }),
        ("one frame per two ticks", Frames::Fixed { period_ns: TICK_NS * 2 }),
        ("one frame per tick", Frames::Fixed { period_ns: TICK_NS }),
        ("two frames per tick", Frames::Fixed { period_ns: TICK_NS / 2 }),
        ("four frames per tick", Frames::Fixed { period_ns: TICK_NS / 4 }),
        ("eight frames per tick", Frames::Fixed { period_ns: TICK_NS / 8 }),
        ("irregular frames", Frames::Irregular { seed: 0x0f4a_3e5 }),
    ];
    out.key("frames_and_ticks");
    out.array_start();
    for (name, frames) in schedules {
        for with_timestamps in [true, false] {
            let mut mapping = Mapping::new(KEYS);
            let baseline = mapping.record(EPOCH_A);
            let run = run_host(events, frames, with_timestamps, &mut mapping, EPOCH_A);
            let mut domain = Domain::new();
            play(&mut domain, &log_of(baseline, &run.records));
            let seen = Truth::of(&domain);
            let delays: Vec<i64> = run.placements.iter().map(|placement| placement.applied_in_tick as i64 - placement.happened_in_tick as i64).collect();
            let earliest = delays.iter().copied().min().unwrap_or(0);
            let latest = delays.iter().copied().max().unwrap_or(0);
            let mean = delays.iter().sum::<i64>() as f64 / delays.len().max(1) as f64;
            out.object_start();
            out.string("schedule", name);
            out.bool("event_timestamps", with_timestamps);
            out.u64("ticks", run.records.len() as u64);
            out.u64("frames", run.frames);
            out.u64("frames_without_a_tick", run.frames_without_a_tick);
            out.u64("most_ticks_in_a_frame", run.most_ticks_in_a_frame);
            checks.record(out, "presses_releases_and_motion_equal_the_stream", seen == *truth);
            out.u64("rejections", domain.rejections);
            // A press is applied in the tick during which it happened (0), in
            // a later one (positive) or in an earlier one (negative: the
            // record of a tick the host ran late took an event that happened
            // after that tick's end).
            out.i64("earliest_tick_of_application_against_the_tick_of_the_press", earliest);
            out.i64("latest_tick_of_application_against_the_tick_of_the_press", latest);
            out.decimal("mean_ticks_between_the_press_and_its_application", mean);
            if with_timestamps {
                // The last tick of a frame takes the events up to the frame's
                // instant, which may lie in the next tick's interval.
                checks.record(out, "a_press_is_applied_in_its_tick_or_one_tick_ahead_and_never_late", earliest >= -1 && latest <= 0);
            }
            out.object_end();
        }
    }
    out.array_end();
}

/// The same record applied twice, a record missing, a record rejected: the
/// totals at the end equal the stream's, neither more nor less.
fn idempotence(out: &mut Json, checks: &mut Checks, events: &[DeviceEvent], truth: &Truth) {
    let mut mapping = Mapping::new(KEYS);
    let baseline = mapping.record(EPOCH_A);
    let run = run_host(events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, EPOCH_A);
    let plain = log_of(baseline, &run.records);
    let last = plain.len() - 1;
    out.key("idempotence");
    out.object_start();

    let mut twice = Vec::new();
    for inputs in &plain {
        twice.push(*inputs);
        twice.push(TickInputs { bind: None, ..*inputs });
    }
    let mut domain = Domain::new();
    play(&mut domain, &twice);
    checks.record(out, "every_record_applied_twice_gives_the_stream_totals", Truth::of(&domain) == *truth && domain.rejections == 0);

    let variants: [(&str, fn(Record) -> Option<Record>); 3] = [
        ("one_record_in_ten_missing", |_| None),
        ("one_record_in_ten_not_finite", |record| Some(Record { move_x: f32::NAN, ..record })),
        ("one_record_in_ten_of_another_epoch", |record| Some(Record { epoch: EPOCH_B, ..record })),
    ];
    for (name, change) in variants {
        let mut log = plain.clone();
        let mut replaced = 0u64;
        for (index, inputs) in log.iter_mut().enumerate() {
            if index % 10 == 9 && index != last {
                inputs.record = change(inputs.record.expect("the plain log has a record every tick"));
                replaced += 1;
            }
        }
        let mut domain = Domain::new();
        play(&mut domain, &log);
        let seen = Truth::of(&domain);
        out.key(name);
        out.object_start();
        out.u64("records_replaced", replaced);
        out.u64("rejections", domain.rejections);
        checks.record(out, "totals_equal_the_stream_once_the_next_record_is_applied", seen == *truth);
        out.object_end();
    }

    // A record whose counter jumped beyond the bound is refused whole.
    let mut domain = Domain::new();
    domain.step(&TickInputs { bind: Some(baseline), ..TickInputs::default() });
    let mut jumped = baseline;
    jumped.presses[0] = jumped.presses[0].wrapping_add(100);
    let verdict = domain.step(&TickInputs { record: Some(jumped), ..TickInputs::default() });
    checks.record(out, "a_jump_beyond_the_bound_is_rejected_and_presses_nothing", verdict == Verdict::RejectedBound && domain.presses_seen[0] == 0);
    out.object_end();
}

/// The baseline of the counters: a writer whose counters are not zero, a
/// change of writer, a restore under a live writer, and the replay of it all.
fn writers(out: &mut Json, checks: &mut Checks, events: &[DeviceEvent], truth: &Truth) {
    out.key("writers");
    out.object_start();

    // A writer that has counted before it binds.
    let (earlier, _) = generate(SEED ^ 1, 500);
    let mut mapping = Mapping::new(KEYS);
    for event in &earlier {
        mapping.feed(event);
    }
    let baseline = mapping.record(EPOCH_A);
    out.u64("presses_counted_before_the_bind", baseline.presses.iter().map(|count| u64::from(*count)).sum());
    let run = run_host(events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, EPOCH_A);
    let mut log = log_of(baseline, &run.records);
    let mut domain = Domain::new();
    play(&mut domain, &log);
    checks.record(out, "a_writer_with_counters_already_running_presses_nothing_at_its_bind", Truth::of(&domain) == *truth);

    // The writer holds an action and is lost: the engine unbinds, which
    // releases the held parts without a release being counted.
    let end = events.last().map_or(0, |event| event.time_ns);
    mapping.feed(&DeviceEvent { time_ns: end, kind: EventKind::KeyDown(KEYS[0]) });
    log.push(TickInputs { record: Some(mapping.record(EPOCH_A)), ..TickInputs::default() });
    log.push(TickInputs::default());
    log.push(TickInputs::default());
    let held_before_the_unbind = {
        let mut domain = Domain::new();
        play(&mut domain, &log);
        domain.held_ticks[0]
    };
    log.push(TickInputs { unbind: true, ..TickInputs::default() });
    for _ in 0..10 {
        log.push(TickInputs::default());
    }
    // A late record of the lost writer is refused.
    log.push(TickInputs { record: Some(mapping.record(EPOCH_A)), ..TickInputs::default() });
    let mut domain = Domain::new();
    play(&mut domain, &log);
    let mut expected = *truth;
    expected.presses[0] += 1;
    checks.record(
        out,
        "an_unbind_releases_what_was_held_without_counting_a_release",
        Truth::of(&domain) == expected && domain.held_ticks[0] == held_before_the_unbind && domain.applied.held == 0,
    );
    checks.record(out, "a_record_of_the_unbound_writer_is_rejected", domain.rejections == 1);

    // Another writer, with unrelated counters, takes the slot.
    let (second_events, second_truth) = generate(SEED ^ 2, 1_000);
    let (junk, _) = generate(SEED ^ 3, 300);
    let mut second = Mapping::new(KEYS);
    for event in &junk {
        second.feed(event);
    }
    let second_baseline = second.record(EPOCH_B);
    let second_run = run_host(&second_events, Frames::Fixed { period_ns: TICK_NS / 2 }, true, &mut second, EPOCH_B);
    log.extend(log_of(second_baseline, &second_run.records));
    let mut domain = Domain::new();
    let hashes = play(&mut domain, &log);
    checks.record(out, "a_second_writer_with_unrelated_counters_presses_nothing_at_its_bind", Truth::of(&domain) == expected.plus(second_truth));

    // The whole log, binds and unbind included, replays to the same hashes.
    let mut replayed = Domain::new();
    checks.record(out, "the_log_replays_to_the_same_hash_at_every_tick", play(&mut replayed, &log) == hashes);
    out.u64("ticks_of_the_replayed_log", log.len() as u64);
    out.object_end();
}

/// A world restored to an earlier tick while its writer kept counting.
fn restore(out: &mut Json, checks: &mut Checks, events: &[DeviceEvent]) {
    let mut mapping = Mapping::new(KEYS);
    let baseline = mapping.record(EPOCH_A);
    let run = run_host(events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, EPOCH_A);
    let log = log_of(baseline, &run.records);
    let mut domain = Domain::new();
    play(&mut domain, &log[..100]);
    let snapshot = domain.clone();
    play(&mut domain, &log[100..400]);

    out.key("restore_under_a_live_writer");
    out.object_start();
    // What one tick of the writer is worth at tick 400.
    let honest: u64 = (0..BUTTONS).map(|action| u64::from(run.records[400].presses[action].wrapping_sub(run.records[399].presses[action]))).sum();

    // Nothing done at the restore: the writer's epoch is still the slot's.
    let mut naive = snapshot.clone();
    let verdict = naive.step(&log[400]);
    let burst: u64 = naive.presses_seen.iter().sum::<u64>() - snapshot.presses_seen.iter().sum::<u64>();
    out.string("with_nothing_done_the_next_record_is", &format!("{verdict:?}"));
    out.u64("presses_that_record_applies", burst);
    out.u64("presses_the_writer_counted_in_that_tick", honest);
    out.bool("the_epoch_alone_stops_the_stale_record", verdict == Verdict::RejectedEpoch);

    // The engine unbinds every slot at a restore; the writer binds again.
    let mut restored = snapshot.clone();
    restored.step(&TickInputs { unbind: true, ..TickInputs::default() });
    let stale = restored.step(&log[400]);
    let before = restored.presses_seen.iter().sum::<u64>();
    restored.step(&TickInputs { bind: Some(run.records[401]), record: Some(run.records[401]), ..TickInputs::default() });
    let at_the_bind = restored.presses_seen.iter().sum::<u64>() - before;
    for inputs in &log[402..] {
        restored.step(inputs);
    }
    let mut reference = Domain::new();
    reference.step(&TickInputs { bind: Some(run.records[401]), record: Some(run.records[401]), ..TickInputs::default() });
    for inputs in &log[402..] {
        reference.step(inputs);
    }
    let continued = (0..BUTTONS).all(|action| restored.presses_seen[action] == snapshot.presses_seen[action] + reference.presses_seen[action]);
    checks.record(out, "after_an_unbind_at_the_restore_the_stale_record_is_rejected", stale == Verdict::RejectedEpoch);
    checks.record(out, "the_new_bind_presses_nothing_and_the_run_continues_exactly", at_the_bind == 0 && continued);
    out.object_end();
}

/// A lost focus and the contexts.
fn forced_release_and_contexts(out: &mut Json, checks: &mut Checks) {
    const MS: u64 = 1_000_000;
    let events = [
        DeviceEvent { time_ns: MS, kind: EventKind::KeyDown(KEYS[0]) },
        DeviceEvent { time_ns: 40 * MS, kind: EventKind::FocusLost },
        DeviceEvent { time_ns: 200 * MS, kind: EventKind::Stick { x: 0.0, y: 0.0 } },
    ];
    let mut mapping = Mapping::new(KEYS);
    let baseline = mapping.record(EPOCH_A);
    let run = run_host(&events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, EPOCH_A);
    let mut domain = Domain::new();
    play(&mut domain, &log_of(baseline, &run.records));
    checks.record(
        out,
        "a_lost_focus_releases_the_held_action_without_a_release_being_read",
        domain.presses_seen[0] == 1 && domain.releases_seen[0] == 0 && domain.applied.held == 0 && domain.held_ticks[0] <= 3,
    );

    // Actions 6 and 7 belong to a context that is not active: their values
    // are neutral at application, whoever wrote them.
    let events = [
        DeviceEvent { time_ns: MS, kind: EventKind::KeyDown(KEYS[0]) },
        DeviceEvent { time_ns: 2 * MS, kind: EventKind::KeyDown(KEYS[7]) },
        DeviceEvent { time_ns: 60 * MS, kind: EventKind::KeyUp(KEYS[0]) },
        DeviceEvent { time_ns: 61 * MS, kind: EventKind::KeyUp(KEYS[7]) },
    ];
    let mut mapping = Mapping::new(KEYS);
    let baseline = mapping.record(EPOCH_A);
    let run = run_host(&events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, EPOCH_A);
    let mut log = log_of(baseline, &run.records);
    log[0].contexts = Some(CONTEXT_ON_FOOT);
    let mut domain = Domain::new();
    play(&mut domain, &log);
    checks.record(
        out,
        "an_action_outside_the_active_contexts_is_neutral_at_application",
        domain.presses_seen[0] == 1 && domain.releases_seen[0] == 1 && domain.presses_seen[7] == 0 && domain.releases_seen[7] == 0 && domain.held_ticks[7] == 0,
    );
}

/// The size of a record and what building and applying one costs.
fn size_and_cost(out: &mut Json, events: &[DeviceEvent]) {
    let mut mapping = Mapping::new(KEYS);
    let baseline = mapping.record(EPOCH_A);
    let run = run_host(events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, EPOCH_A);
    let mut changed = 0u64;
    let mut identical = 0u64;
    for pair in run.records.windows(2) {
        let (a, b) = (pair[0].to_bytes(), pair[1].to_bytes());
        let differing = a.iter().zip(&b).filter(|(x, y)| x != y).count() as u64;
        changed += differing;
        identical += u64::from(differing == 0);
    }
    out.key("record_size");
    out.object_start();
    out.string("action_set", "eight buttons, one two-axis value, one relative motion, one integrated two-axis value");
    out.u64("bytes_with_an_epoch_of_8_bytes", RECORD_BYTES as u64);
    out.u64("bytes_with_an_epoch_of_16_bytes", RECORD_BYTES as u64 + 8);
    out.u64("bytes_without_the_epoch", RECORD_BYTES as u64 - 8);
    out.decimal("mean_bytes_that_differ_from_the_previous_record", changed as f64 / (run.records.len() - 1) as f64);
    out.u64("records_identical_to_the_previous_one", identical);
    out.u64("records", run.records.len() as u64);
    out.object_end();

    let log = log_of(baseline, &run.records);
    let mut best_host = u128::MAX;
    let mut best_domain = u128::MAX;
    for _ in 0..5 {
        let mut mapping = Mapping::new(KEYS);
        let started = Instant::now();
        let run = run_host(events, Frames::Fixed { period_ns: TICK_NS }, true, &mut mapping, EPOCH_A);
        best_host = best_host.min(started.elapsed().as_nanos() / run.records.len() as u128);
        let mut domain = Domain::new();
        let started = Instant::now();
        for inputs in &log {
            domain.step(inputs);
        }
        best_domain = best_domain.min(started.elapsed().as_nanos() / log.len() as u128);
        std::hint::black_box(&domain);
    }
    out.key("cost_per_tick_best_of_five_rounds");
    out.object_start();
    out.u64("events_per_tick", events.len() as u64 / TICKS);
    out.u64("mapping_the_events_and_taking_the_record_ns", best_host as u64);
    out.u64("applying_the_record_ns", best_domain as u64);
    out.object_end();
}

/// Starts the device layer probe as a child process and returns its report,
/// or what became of it.
fn device_layer(out: &mut Json) {
    out.key("device_layer");
    out.object_start();
    let exe = env::current_exe().expect("the spike knows its own path");
    let started = Instant::now();
    match Command::new(exe).arg("--probe").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Err(error) => out.string("outcome", &format!("not started: {error}")),
        Ok(mut child) => {
            let mut stdout = child.stdout.take().expect("the child's output is piped");
            let mut stderr = child.stderr.take().expect("the child's errors are piped");
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let mut text = String::new();
                let mut errors = String::new();
                let _ = stdout.read_to_string(&mut text);
                let _ = stderr.read_to_string(&mut errors);
                let _ = sender.send((text, errors));
            });
            match receiver.recv_timeout(PROBE_TIMEOUT) {
                Ok((text, errors)) => {
                    let status = child.wait().ok();
                    let clean = status.is_some_and(|status| status.success()) && text.trim_start().starts_with('{');
                    out.string("outcome", if clean { "reported" } else { "ended without a report" });
                    out.string("exit_status", &status.map_or("unknown".to_string(), |status| status.to_string()));
                    if clean {
                        out.raw("report", &text);
                    }
                    let tail: String = errors.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
                    out.string("standard_error_tail", &tail);
                }
                Err(_) => {
                    let _ = child.kill();
                    out.string("outcome", "timed out");
                }
            }
        }
    }
    out.decimal("seconds", started.elapsed().as_secs_f64());
    out.object_end();
}

fn main() {
    let arguments: Vec<String> = env::args().collect();
    if arguments.get(1).map(String::as_str) == Some("--probe") {
        println!("{}", probe::run());
        return;
    }
    let cell = arguments.get(1).cloned().unwrap_or_else(|| "unnamed".to_string());
    let (events, truth) = generate(SEED, TICKS);
    let mut checks = Checks { every_check_holds: true };

    let mut out = Json::new();
    out.object_start();
    out.u64("schema_version", SCHEMA_VERSION);
    out.string("spike", SPIKE);
    out.string("cell", &cell);
    out.string("profile", if cfg!(debug_assertions) { "dev" } else { "release" });
    write_platform_and_build(&mut out);
    out.key("stream");
    out.object_start();
    out.u64("ticks", TICKS);
    out.u64("tick_ns", TICK_NS);
    out.u64("events", events.len() as u64);
    write_truth(&mut out, "counted_at_generation", &truth);
    out.object_end();
    frames_and_ticks(&mut out, &mut checks, &events, &truth);
    idempotence(&mut out, &mut checks, &events, &truth);
    writers(&mut out, &mut checks, &events, &truth);
    restore(&mut out, &mut checks, &events);
    out.key("forced_release_and_contexts");
    out.object_start();
    forced_release_and_contexts(&mut out, &mut checks);
    out.object_end();
    size_and_cost(&mut out, &events);
    device_layer(&mut out);
    out.bool("every_check_holds", checks.every_check_holds);
    out.object_end();
    println!("{}", out.finish());
}
