//! The gestures a window lives through while frames are presented to it: a
//! move, a resize, a minimization and an occlusion, in cycles, each recorded
//! with how it was produced. Throwaway, never product code.

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::json::Json;
use crate::scenario::{COVERED, MINIMIZED, RESIZING, SHOWN, Shared};
#[cfg(windows)]
use crate::scenario::MOVING;

/// Sleeps in short steps and says whether the presenter is done meanwhile.
fn pause(shared: &Shared, duration: Duration) -> bool {
    let mut left = duration;
    while !left.is_zero() {
        if shared.presenter_done.load(Ordering::Relaxed) {
            return true;
        }
        let step = left.min(Duration::from_millis(25));
        std::thread::sleep(step);
        left -= step;
    }
    shared.presenter_done.load(Ordering::Relaxed)
}

/// Cycles of gestures injected on a Windows window: the move and the resize
/// through the system's own loop, the minimization as the caption's button
/// asks it, the occlusion by a window of the spike's own placed over it. The
/// reports of the first two cycles are kept, and the count of the others.
#[cfg(windows)]
pub fn injected_cycles(shared: &Shared, window: usize, cover: usize, borderless: bool) -> Vec<Json> {
    use crate::win32;
    let mut reports = Vec::new();
    let mut cycles = 0_u32;
    pause(shared, Duration::from_millis(300));
    loop {
        let keep = cycles < 2;
        if borderless {
            if cycles == 0 {
                reports.push(Json::object().with("gesture", "move and resize").with("not_run", "a borderless window that covers its display has no frame to move or to resize it by"));
            }
        } else {
            for resize in [false, true] {
                shared.phase.store(if resize { RESIZING } else { MOVING }, Ordering::Relaxed);
                let gesture = win32::modal_gesture(window, resize, 12, Duration::from_millis(40));
                shared.phase.store(SHOWN, Ordering::Relaxed);
                if keep {
                    reports.push(gesture.json.with("produced_by", "the system command posted to the window, then arrow keys and the return key posted to its thread"));
                }
                pause(shared, Duration::from_millis(150));
            }
        }
        shared.phase.store(MINIMIZED, Ordering::Relaxed);
        win32::minimize(window);
        pause(shared, Duration::from_millis(100));
        let minimized = win32::is_minimized(window);
        std::thread::sleep(Duration::from_millis(500));
        win32::restore(window);
        let mut restored = false;
        for _ in 0..100 {
            if !win32::is_minimized(window) {
                restored = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        shared.phase.store(SHOWN, Ordering::Relaxed);
        if keep {
            reports.push(Json::object().with("gesture", "minimize, then restore").with("minimized", minimized).with("restored", restored).with("produced_by", "the system command posted to the window, then a call that shows it again without giving it the focus"));
        }
        pause(shared, Duration::from_millis(200));
        shared.phase.store(COVERED, Ordering::Relaxed);
        win32::cover(cover, window);
        std::thread::sleep(Duration::from_millis(600));
        win32::uncover(cover);
        shared.phase.store(SHOWN, Ordering::Relaxed);
        if keep {
            reports.push(Json::object().with("gesture", "occlude, then uncover").with("produced_by", "a window of the spike's own shown over the whole of the window, above it, then hidden"));
        }
        cycles += 1;
        shared.gesture_cycles.store(cycles, Ordering::Relaxed);
        // A long run lives through a cycle every fifteen seconds.
        if pause(shared, Duration::from_secs(15)) {
            break;
        }
    }
    reports.push(Json::object().with("cycles_of_gestures", cycles));
    reports
}

/// The gestures where no input is injected: by calls of the library, by a
/// person, or none.
pub fn other_cycles(shared: &Shared, gestures: &str) -> Vec<Json> {
    let mut reports = Vec::new();
    match gestures {
        "calls" => {
            pause(shared, Duration::from_millis(300));
            shared.phase.store(RESIZING, Ordering::Relaxed);
            shared.call_request.store(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(500));
            shared.call_request.store(2, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(300));
            shared.phase.store(SHOWN, Ordering::Relaxed);
            reports.push(Json::object().with("gesture", "resize").with("produced_by", "two calls of the windowing library, which enter no loop of the system"));
            if cfg!(target_os = "macos") {
                shared.phase.store(MINIMIZED, Ordering::Relaxed);
                shared.call_request.store(3, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1500));
                shared.call_request.store(4, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1500));
                shared.phase.store(SHOWN, Ordering::Relaxed);
                reports.push(Json::object().with("gesture", "minimize, then restore").with("produced_by", "two calls of the windowing library"));
                reports.push(Json::object().with("gesture", "move and occlude").with("not_run", "no input is injected on this platform by the spike, and no second window is placed over the first"));
            } else {
                reports.push(Json::object().with("gesture", "move, minimize and occlude").with("not_run", "a virtual display has no window manager to minimize or occlude a window, and under a compositor a client cannot move itself"));
            }
            let _ = COVERED;
            let _ = MINIMIZED;
        }
        "hand" => {
            reports.push(Json::object().with("gesture", "by a person").with("produced_by", "the person at the machine, for thirty seconds; the states of the window are not told apart"));
            pause(shared, Duration::from_secs(30));
        }
        _ => {}
    }
    shared.gesture_cycles.store(1, Ordering::Relaxed);
    reports
}
