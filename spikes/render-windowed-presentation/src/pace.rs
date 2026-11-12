//! The scenario of the pace: a fixed-step loop at 60 ticks a second, alone
//! (the control), beside a presentation thread that presents in step with
//! the display, and presenting by itself in that mode, while the window is
//! shown, covered and minimized. A tick's lateness is the time between its
//! due instant on the pacing clock and its start. Throwaway, never product
//! code.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::clock::percentile;
use crate::host;
use crate::json::Json;
use crate::presenter::{self, Mode, Request};
use crate::scenario::{Arguments, SendPresenter};

const TICKS_PER_SECOND: u64 = 60;
const WARM_UP_TICKS: u64 = 60;

/// The four loops compared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Loop {
    /// Ticks, and presents nothing.
    Control,
    /// Ticks, and hands each frame to the presentation thread.
    Beside,
    /// Ticks, and presents each frame itself, from an empty queue of frames.
    Itself,
    /// As `Itself`, the swapchain's queue of frames filled first.
    ItselfQueueFull,
}

impl Loop {
    const ALL: [Loop; 4] = [Loop::Control, Loop::Beside, Loop::Itself, Loop::ItselfQueueFull];

    fn name(self) -> &'static str {
        match self {
            Loop::Control => "control, presenting nothing",
            Loop::Beside => "beside a presentation thread",
            Loop::Itself => "presenting itself",
            Loop::ItselfQueueFull => "presenting itself, the queue of frames filled first",
        }
    }
}

/// The frame the stepping loop hands the presentation thread: one place,
/// taken or free.
#[derive(Default)]
struct HandOff {
    frame: Option<u64>,
    stop: bool,
}

type Outcomes = Arc<Mutex<BTreeMap<String, u64>>>;

/// A tick's own work: a fixed computation, the same in every loop.
fn tick_work(seed: u64) -> u64 {
    let mut value = seed | 1;
    for step in 0..40_000_u64 {
        value = value.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(step);
        value ^= value >> 29;
    }
    std::hint::black_box(value)
}

pub fn run(arguments: &Arguments) -> Json {
    let header = Json::object()
        .with("scenario", "a fixed-step loop alone, beside a presentation thread, and presenting itself")
        .with("library", arguments.text("library", "winit"))
        .with("path", arguments.text("path", "direct3d12"))
        .with("device_index", arguments.number("device", 0))
        .with("ticks_per_second", TICKS_PER_SECOND)
        .with("seconds_per_segment", arguments.number("seconds", 20))
        .with("rounds", arguments.number("rounds", 3));
    match run_inner(arguments, &header) {
        Ok(result) => result,
        Err(error) => header.with("completed", false).with("error", error),
    }
}

fn run_inner(arguments: &Arguments, header: &Json) -> Result<Json, String> {
    let seconds = arguments.number("seconds", 20);
    let rounds = arguments.number("rounds", 3);
    let spec = arguments.window("render-windowed-presentation");
    let mut host = host::open(&arguments.text("library", "winit"), &spec)?;
    let native = host.native()?;
    for _ in 0..20 {
        host.pump(&mut |_| {});
        std::thread::sleep(Duration::from_millis(5));
    }
    let size = host.pixel_size();
    host::publish_size(size.0, size.1);
    let request = Request {
        path: arguments.text("path", "direct3d12"),
        device: arguments.number("device", 0) as usize,
        mode: Mode::Synchronized,
        window: native,
        width: size.0,
        height: size.1,
        // In step with the display, an acquisition may wait for an image.
        acquire_wait_ms: 1000,
    };
    #[cfg(windows)]
    let (window, cover) = {
        let host::NativeWindow::Win32 { hwnd, .. } = native else { return Err("a Windows window was expected".to_string()) };
        (hwnd, crate::win32::Cover::create()?)
    };
    #[cfg(windows)]
    let cover_window = cover.0;

    let slot: Arc<Mutex<Option<SendPresenter>>> = Arc::default();
    let hand_off: Arc<(Mutex<HandOff>, Condvar)> = Arc::default();
    let outcomes: Outcomes = Arc::default();
    let (opened_sender, opened) = std::sync::mpsc::channel::<Result<Json, String>>();

    let prepared = presenter::prepare(&request)?;
    // The presentation thread: it opens the presenter, then presents each
    // frame the stepping loop hands it.
    let presentation = {
        let (slot, hand_off, outcomes) = (slot.clone(), hand_off.clone(), outcomes.clone());
        std::thread::Builder::new()
            .name("presentation".to_string())
            .spawn(move || {
                match presenter::open(&request, prepared) {
                    Ok(presenter) => {
                        let described = presenter.describe();
                        *slot.lock().unwrap() = Some(SendPresenter(presenter));
                        let _ = opened_sender.send(Ok(described));
                    }
                    Err(error) => {
                        let _ = opened_sender.send(Err(error));
                        return;
                    }
                }
                loop {
                    let frame = {
                        let (lock, wake) = &*hand_off;
                        let mut place = lock.lock().unwrap();
                        while place.frame.is_none() && !place.stop {
                            place = wake.wait(place).unwrap();
                        }
                        if place.stop {
                            break;
                        }
                        place.frame.unwrap_or(0)
                    };
                    if let Some(presenter) = slot.lock().unwrap().as_mut() {
                        record(&outcomes, presenter.0.frame(frame));
                    }
                    // The place is freed once the frame is presented: the
                    // loop makes no frame while its place is taken.
                    hand_off.0.lock().unwrap().frame = None;
                }
                // The presenter is released on the thread that opened it.
                drop(slot.lock().unwrap().take());
            })
            .map_err(|error| error.to_string())?
    };
    let presenter_facts = loop {
        host.pump(&mut |_| {});
        match opened.try_recv() {
            Ok(result) => break result?,
            Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(1)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return Err("the presentation thread ended before it opened a presenter".to_string()),
        }
    };

    // The stepping thread runs every segment; the platform thread pumps.
    let done = Arc::new(AtomicBool::new(false));
    let stepping = {
        let (slot, hand_off, outcomes, done) = (slot.clone(), hand_off.clone(), outcomes.clone(), done.clone());
        std::thread::Builder::new()
            .name("stepping".to_string())
            .spawn(move || {
                let mut segments = Vec::new();
                let states: &[&str] = if cfg!(windows) { &["shown", "covered by another window", "minimized"] } else { &["shown"] };
                for round in 0..rounds {
                    for (state_index, state) in states.iter().enumerate() {
                        #[cfg(windows)]
                        match state_index {
                            1 => crate::win32::cover(cover_window, window),
                            2 => crate::win32::minimize(window),
                            _ => {}
                        }
                        std::thread::sleep(Duration::from_millis(400));
                        // The order of the loops turns with the round, so
                        // that no loop always follows the same one.
                        for offset in 0..Loop::ALL.len() {
                            let kind = Loop::ALL[(offset + round as usize) % Loop::ALL.len()];
                            outcomes.lock().unwrap().clear();
                            let mut segment = segment(kind, seconds, &slot, &hand_off, &outcomes);
                            segment.set("round", round + 1);
                            segment.set("window", *state);
                            segments.push(segment);
                        }
                        #[cfg(windows)]
                        match state_index {
                            1 => crate::win32::uncover(cover_window),
                            2 => crate::win32::restore(window),
                            _ => {}
                        }
                        let _ = state_index;
                    }
                }
                done.store(true, Ordering::Relaxed);
                segments
            })
            .map_err(|error| error.to_string())?
    };
    while !done.load(Ordering::Relaxed) && !stepping.is_finished() {
        host.pump(&mut |_| {});
        std::thread::sleep(Duration::from_millis(1));
    }
    let segments = stepping.join().map_err(|_| "the stepping thread panicked".to_string())?;
    {
        let (lock, wake) = &*hand_off;
        lock.lock().unwrap().stop = true;
        wake.notify_all();
    }
    while !presentation.is_finished() {
        host.pump(&mut |_| {});
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = presentation.join();
    host.close_window();
    let verdicts = verdicts(&segments);
    Ok(header.clone().with("completed", true).with("presenter", presenter_facts).with("library_version", host.library()).with("verdicts", verdicts).with("segments", Json::List(segments)))
}

fn record(outcomes: &Outcomes, outcome: Result<presenter::FrameOutcome, String>) {
    let status = match outcome {
        Ok(outcome) => outcome.status,
        Err(error) => format!("error: {error}"),
    };
    *outcomes.lock().unwrap().entry(status).or_default() += 1;
}

/// One loop for `seconds`, after a second of warm-up, with the lateness of
/// each of its ticks.
fn segment(kind: Loop, seconds: u64, slot: &Mutex<Option<SendPresenter>>, hand_off: &(Mutex<HandOff>, Condvar), outcomes: &Outcomes) -> Json {
    let period = Duration::from_nanos(1_000_000_000 / TICKS_PER_SECOND);
    let total = WARM_UP_TICKS + seconds * TICKS_PER_SECOND;
    let mut lateness: Vec<u64> = Vec::with_capacity(total as usize);
    let mut work: Vec<u64> = Vec::with_capacity(total as usize);
    let (mut frames_made, mut frames_not_made) = (0_u64, 0_u64);
    if kind == Loop::ItselfQueueFull {
        // Four frames presented back to back: the swapchain's queue is full,
        // as it is in the steady state of a loop that outruns the display.
        if let Some(presenter) = slot.lock().unwrap().as_mut() {
            for index in 0..4 {
                let _ = presenter.0.frame(index);
            }
        }
    }
    #[cfg(windows)]
    let busy_before = system_times();
    let start = Instant::now() + Duration::from_millis(5);
    for tick in 0..total {
        let due = start + period * tick as u32;
        let now = Instant::now();
        if now < due {
            std::thread::sleep(due - now);
        }
        let begun = Instant::now();
        if tick >= WARM_UP_TICKS {
            lateness.push(begun.saturating_duration_since(due).as_nanos() as u64);
        }
        tick_work(tick);
        if tick >= WARM_UP_TICKS {
            work.push(begun.elapsed().as_nanos() as u64);
        }
        match kind {
            Loop::Control => {}
            Loop::Beside => {
                let (lock, wake) = hand_off;
                let mut place = lock.lock().unwrap();
                if place.frame.is_none() {
                    place.frame = Some(tick);
                    wake.notify_one();
                    frames_made += 1;
                } else {
                    frames_not_made += 1;
                }
            }
            Loop::Itself | Loop::ItselfQueueFull => {
                // A tick that is due runs before a frame is made.
                if Instant::now() < start + period * (tick as u32 + 1) {
                    if let Some(presenter) = slot.lock().unwrap().as_mut() {
                        record(outcomes, presenter.0.frame(tick));
                    }
                    frames_made += 1;
                } else {
                    frames_not_made += 1;
                }
            }
        }
    }
    if kind == Loop::Beside {
        // The last frame handed is presented before the next loop starts.
        while hand_off.0.lock().unwrap().frame.is_some() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    lateness.sort_unstable();
    work.sort_unstable();
    let micro = |nanoseconds: u64| Json::Float(nanoseconds as f64 / 1000.0);
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut json = Json::object()
        .with("loop", kind.name())
        .with("ticks", lateness.len())
        .with("lateness_median_us", micro(percentile(&lateness, 0.5)))
        .with("lateness_p99_us", micro(percentile(&lateness, 0.99)))
        .with("lateness_max_us", micro(lateness.last().copied().unwrap_or(0)))
        .with("tick_work_median_us", micro(percentile(&work, 0.5)))
        .with("frames_made", frames_made)
        .with("frames_not_made", frames_not_made)
        .with("presenter_outcomes", Json::List(outcomes.lock().unwrap().iter().map(|(status, count)| Json::object().with("outcome", status.clone()).with("turns", *count)).collect()));
    #[cfg(windows)]
    if let (Some(before), Some(after)) = (busy_before, system_times()) {
        let total = (after.1 - before.1) as f64;
        if total > 0.0 {
            json.set("machine_busy_percent", Json::Float(100.0 * (1.0 - (after.0 - before.0) as f64 / total)));
        }
    }
    json
}

/// The machine's idle time and its total processor time so far, in the
/// system's unit: how busy the machine was during a segment says whether it
/// was at rest.
#[cfg(windows)]
fn system_times() -> Option<(u64, u64)> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemTimes(idle: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    }
    let (mut idle, mut kernel, mut user) = (0_u64, 0_u64, 0_u64);
    // SAFETY: the call writes three 64-bit times; the kernel time includes
    // the idle time.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 { None } else { Some((idle, kernel + user)) }
}

/// Per state of the window, the comparison the hypothesis states, by the
/// definitions the README gives before the run: the control's spread is the
/// interval between the smallest and the largest 99th percentile of its
/// rounds; a loop is within it when the median of its rounds' 99th
/// percentiles is not above the control's largest.
fn verdicts(segments: &[Json]) -> Json {
    let field = |segment: &Json, key: &str| -> Option<Json> {
        if let Json::Object(members) = segment { members.iter().find(|(name, _)| name == key).map(|(_, value)| value.clone()) } else { None }
    };
    let text = |segment: &Json, key: &str| match field(segment, key) {
        Some(Json::Text(text)) => text,
        _ => String::new(),
    };
    let number = |segment: &Json, key: &str| match field(segment, key) {
        Some(Json::Float(value)) => value,
        _ => 0.0,
    };
    let mut states: Vec<String> = Vec::new();
    for segment in segments {
        let state = text(segment, "window");
        if !states.contains(&state) {
            states.push(state);
        }
    }
    let median = |values: &mut Vec<f64>| -> f64 {
        values.sort_by(|a, b| a.total_cmp(b));
        if values.is_empty() { 0.0 } else { values[(values.len() - 1) / 2] }
    };
    Json::List(
        states
            .iter()
            .map(|state| {
                let of = |kind: Loop| -> Vec<f64> { segments.iter().filter(|segment| text(segment, "window") == *state && text(segment, "loop") == kind.name()).map(|segment| number(segment, "lateness_p99_us")).collect() };
                let list = |values: &[f64]| Json::List(values.iter().map(|&value| Json::Float(value)).collect());
                let control = of(Loop::Control);
                let largest = control.iter().copied().fold(0.0_f64, f64::max);
                let smallest = control.iter().copied().fold(f64::INFINITY, f64::min);
                let spread = if control.is_empty() { 0.0 } else { largest - smallest };
                let mut verdict = Json::object().with("window", state.clone()).with("control_p99_us", list(&control)).with("control_spread_us", Json::Float(spread)).with("control_largest_p99_us", Json::Float(largest));
                let mut beside = of(Loop::Beside);
                let beside_median = median(&mut beside);
                verdict.set("beside_a_presentation_thread_p99_us", list(&beside));
                verdict.set("beside_a_presentation_thread_median_p99_us", Json::Float(beside_median));
                verdict.set("beside_a_presentation_thread_is_within_the_control", beside_median <= largest);
                for (kind, key) in [(Loop::Itself, "presenting_itself"), (Loop::ItselfQueueFull, "presenting_itself_queue_full")] {
                    let mut values = of(kind);
                    let own_median = median(&mut values);
                    verdict.set(&format!("{key}_p99_us"), list(&values));
                    verdict.set(&format!("{key}_median_p99_us"), Json::Float(own_median));
                    verdict.set(&format!("{key}_is_beyond_the_control"), own_median > largest);
                    verdict.set(&format!("{key}_minus_beside_us"), Json::Float(own_median - beside_median));
                    verdict.set(&format!("{key}_gain_of_a_thread_exceeds_the_spread"), own_median - beside_median > spread);
                }
                verdict
            })
            .collect(),
    )
}
