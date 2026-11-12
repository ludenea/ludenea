//! The scenario that presents frames to a window while the window lives
//! through gestures: a presenter on a thread that is not the platform thread
//! (or, as the fallback, inside the platform thread's loop), the platform
//! thread pumping events, a watchdog that calls a deadlock, and the times of
//! acquisition, submission and presentation per state of the window, with the
//! period of an offscreen stream the same presenter serves. Throwaway, never
//! product code.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::clock::now_ns;
use crate::host;
#[cfg(windows)]
use crate::host::NativeWindow;
use crate::json::Json;
use crate::presenter::{self, Request};
use crate::scenario::{Arguments, EventCounts, Shared, Turns};

/// A deadlock is no frame presented and no event pumped for this long.
const DEADLOCK_AFTER: Duration = Duration::from_secs(5);

/// A stall is the presenter finishing no turn for this long while the
/// platform thread still pumps: a call of the presenter that does not return.
const STALL_AFTER: Duration = Duration::from_secs(20);

/// Sets a flag when the thread that owns it ends, however it ends.
struct DoneOnDrop(Arc<Shared>);

impl Drop for DoneOnDrop {
    fn drop(&mut self) {
        self.0.presenter_done.store(true, Ordering::Relaxed);
    }
}

pub fn run(arguments: &Arguments) -> Json {
    let header = Json::object()
        .with("scenario", "frames presented while the window lives through gestures")
        .with("library", arguments.text("library", "winit"))
        .with("path", arguments.text("path", "direct3d12"))
        .with("device_index", arguments.number("device", 0))
        .with("window", arguments.text("window", "windowed"))
        .with("mode", arguments.text("mode", "unsynchronized"))
        .with("presenter_on", arguments.text("presenter", "presentation-thread"))
        .with("frames_asked", arguments.number("frames", 10_000));
    match run_inner(arguments, &header) {
        Ok(result) => result,
        Err(error) => header.with("completed", false).with("deadlock", false).with("error", error),
    }
}

fn run_inner(arguments: &Arguments, header: &Json) -> Result<Json, String> {
    let library = arguments.text("library", "winit");
    let frames = arguments.number("frames", 10_000);
    let on_platform_thread = arguments.text("presenter", "presentation-thread") == "platform-thread";
    let gestures = arguments.text("gestures", if cfg!(windows) { "injected" } else { "calls" });
    let limit = Duration::from_secs(arguments.number("limit_seconds", 300));
    let spec = arguments.window("render-windowed-presentation");

    let mut host = host::open(&library, &spec)?;
    let native = host.native()?;
    let shared = Arc::new(Shared::default());
    let mut counts = EventCounts::default();
    // The window settles (its first size, its first paint) before a
    // swapchain is made for it.
    for _ in 0..20 {
        host.pump(&mut |event| counts.add(event.name));
        std::thread::sleep(Duration::from_millis(5));
    }
    let size = host.pixel_size();
    host::publish_size(size.0, size.1);
    shared.pumped();
    shared.last_frame_ns.store(now_ns(), Ordering::Relaxed);
    shared.last_turn_ns.store(now_ns(), Ordering::Relaxed);
    let request = Request {
        path: arguments.text("path", "direct3d12"),
        device: arguments.number("device", 0) as usize,
        mode: arguments.mode()?,
        window: native,
        width: size.0,
        height: size.1,
        acquire_wait_ms: arguments.number("acquire_wait_ms", 0) as u32,
    };

    #[cfg(windows)]
    let (_observer, cover) = {
        let NativeWindow::Win32 { hwnd, .. } = native else { return Err("a Windows window was expected".to_string()) };
        (crate::win32::Observer::install(hwnd)?, crate::win32::Cover::create()?)
    };

    // The watchdog: a deadlock ends the process with what is known, and a
    // run that outlives its limit is told to stop.
    {
        let shared = shared.clone();
        let header = header.clone();
        std::thread::Builder::new()
            .name("watchdog".to_string())
            .spawn(move || {
                let started = now_ns();
                loop {
                    std::thread::sleep(Duration::from_millis(200));
                    let now = now_ns();
                    let since_frame = now.saturating_sub(shared.last_frame_ns.load(Ordering::Relaxed));
                    let since_pump = now.saturating_sub(shared.last_pump_ns.load(Ordering::Relaxed));
                    if since_frame > DEADLOCK_AFTER.as_nanos() as u64 && since_pump > DEADLOCK_AFTER.as_nanos() as u64 {
                        let report = header
                            .clone()
                            .with("completed", false)
                            .with("deadlock", true)
                            .with("frames_presented", shared.frames_presented.load(Ordering::Relaxed))
                            .with("window_was", crate::scenario::PHASES[shared.phase.load(Ordering::Relaxed).min(4) as usize])
                            .with("milliseconds_since_a_frame", since_frame / 1_000_000)
                            .with("milliseconds_since_a_pump", since_pump / 1_000_000);
                        println!("{}", report.print());
                        std::process::exit(0);
                    }
                    // A presenter held inside one call while the platform thread
                    // still pumps is no deadlock by the definition above, and no
                    // run either: it is reported as a stall.
                    let since_turn = now.saturating_sub(shared.last_turn_ns.load(Ordering::Relaxed));
                    if since_turn > STALL_AFTER.as_nanos() as u64 && !shared.presenter_done.load(Ordering::Relaxed) {
                        let report = header
                            .clone()
                            .with("completed", false)
                            .with("deadlock", false)
                            .with("presenter_stalled", true)
                            .with("error", format!("the presenter did not finish a turn for {} seconds while the platform thread pumped", STALL_AFTER.as_secs()))
                            .with("frames_presented", shared.frames_presented.load(Ordering::Relaxed))
                            .with("window_was", crate::scenario::PHASES[shared.phase.load(Ordering::Relaxed).min(4) as usize])
                            .with("milliseconds_since_a_turn", since_turn / 1_000_000)
                            .with("milliseconds_since_a_pump", since_pump / 1_000_000);
                        println!("{}", report.print());
                        std::process::exit(0);
                    }
                    if now - started > limit.as_nanos() as u64 {
                        shared.stop.store(true, Ordering::Relaxed);
                    }
                }
            })
            .map_err(|error| error.to_string())?;
    }

    // The gestures, from a thread of their own: the platform thread is held
    // inside the system's loop while a window is moved or resized.
    let gesture_thread = {
        let shared = shared.clone();
        let gestures = gestures.clone();
        let borderless = spec.borderless;
        #[cfg(windows)]
        let cover_window = cover.0;
        std::thread::Builder::new()
            .name("gestures".to_string())
            .spawn(move || {
                #[cfg(windows)]
                if gestures == "injected" {
                    let NativeWindow::Win32 { hwnd, .. } = native else { return Vec::new() };
                    return crate::gestures::injected_cycles(&shared, hwnd, cover_window, borderless);
                }
                let _ = borderless;
                crate::gestures::other_cycles(&shared, &gestures)
            })
            .map_err(|error| error.to_string())?
    };
    let gestures_wanted = gestures != "none";
    let finished = move |shared: &Shared, presented: u64| presented >= frames && (!gestures_wanted || shared.gesture_cycles.load(Ordering::Relaxed) >= 1);

    let started = now_ns();
    // What the path needs made on the platform thread is made here.
    let prepared = presenter::prepare(&request)?;
    let report = if on_platform_thread {
        // The first fallback: the platform thread serves the presenter
        // inside its own event loop.
        let outcome = (|| -> Result<Json, String> {
            let mut turns = Turns::new(presenter::open(&request, prepared)?, size);
            loop {
                pump(host.as_mut(), &shared, &mut counts);
                turns.turn(&shared)?;
                if finished(&shared, turns.presented) {
                    return Ok(report(&mut turns, started));
                }
                if shared.stop.load(Ordering::Relaxed) {
                    return Err(format!("the run reached its limit of {} seconds with {} of {frames} frames presented", limit.as_secs(), turns.presented));
                }
            }
        })();
        shared.presenter_done.store(true, Ordering::Relaxed);
        outcome
    } else {
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("presentation".to_string())
                .spawn(move || -> Result<Json, String> {
                    let _done = DoneOnDrop(shared.clone());
                    let mut turns = Turns::new(presenter::open(&request, prepared)?, size);
                    loop {
                        turns.turn(&shared)?;
                        if finished(&shared, turns.presented) {
                            return Ok(report(&mut turns, started));
                        }
                        if shared.stop.load(Ordering::Relaxed) {
                            return Err(format!("the run reached its limit of {} seconds with {} of {frames} frames presented", limit.as_secs(), turns.presented));
                        }
                    }
                })
                .map_err(|error| error.to_string())?
        };
        while !shared.presenter_done.load(Ordering::Relaxed) {
            pump(host.as_mut(), &shared, &mut counts);
            std::thread::sleep(Duration::from_millis(1));
        }
        thread.join().map_err(|payload| format!("the presentation thread panicked: {}", crate::scenario::panic_text(payload.as_ref())))?
    };
    // The gestures end on their own once the presenter is done; the platform
    // thread keeps pumping meanwhile, since a gesture may be waiting for it.
    while !gesture_thread.is_finished() {
        pump(host.as_mut(), &shared, &mut counts);
        std::thread::sleep(Duration::from_millis(1));
    }
    let gesture_reports = gesture_thread.join().map_err(|_| "the gesture thread panicked".to_string())?;
    host.close_window();
    let mut result = header.clone().with("deadlock", false).with("library_version", host.library()).with("gestures_by", gestures).with("gestures", Json::List(gesture_reports)).with("events_delivered", counts.json());
    match report {
        Ok(Json::Object(members)) => {
            result.set("completed", true);
            for (key, value) in members {
                result.set(&key, value);
            }
        }
        Ok(_) => result.set("completed", true),
        Err(error) => {
            result.set("completed", false);
            result.set("error", error);
        }
    }
    Ok(result)
}

/// One pass of the platform thread: the events, the window's size for the
/// presenter, and a gesture asked by a call.
fn pump(host: &mut dyn host::WindowHost, shared: &Shared, counts: &mut EventCounts) {
    host.pump(&mut |event| counts.add(event.name));
    let size = host.pixel_size();
    host::publish_size(size.0, size.1);
    shared.pumped();
    match shared.call_request.swap(0, Ordering::Relaxed) {
        1 => host.request_size(480, 270),
        2 => host.request_size(640, 360),
        3 => host.set_minimized(true),
        4 => host.set_minimized(false),
        _ => {}
    }
}

fn report(turns: &mut Turns, started: u64) -> Json {
    Json::object()
        .with("frames_presented", turns.presented)
        .with("turns", turns.turns)
        .with("duration_ms", (now_ns() - started) / 1_000_000)
        .with("sizes_followed", turns.sizes_followed)
        .with("presenter", turns.presenter.describe())
        .with("per_window_state", turns.phases_json())
        .with("presentation_times", turns.presenter.presentation_times())
}
