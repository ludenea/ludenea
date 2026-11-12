//! The scenario of the injected events: a key injected at the platform's
//! level, the time the library gives the event, the time the host receives
//! it, and the platform's own event as a hook beneath the library sees it,
//! at a pump period the host chooses. Throwaway, never product code.

use crate::json::Json;
use crate::scenario::Arguments;

pub fn run(arguments: &Arguments) -> Json {
    let header = Json::object()
        .with("scenario", "the delay between an injected event and the time a library gives it")
        .with("library", arguments.text("library", "winit"))
        .with("pump_period_ms", arguments.number("pump_ms", 1))
        .with("injection", arguments.text("injection", "posted"));
    #[cfg(windows)]
    {
        match windows::run(arguments) {
            Ok(Json::Object(members)) => {
                let mut result = header.with("completed", true);
                for (key, value) in members {
                    result.set(&key, value);
                }
                result
            }
            Ok(_) => header.with("completed", true),
            Err(error) => header.with("completed", false).with("error", error),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = arguments;
        header.with("completed", false).with("not_run", "the spike injects input on Windows only: no injection is written for X11, for Wayland or for macOS")
    }
}

#[cfg(windows)]
mod windows {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::clock::{now_ns, percentile};
    use crate::host::{self, NativeWindow, PLATFORM_KEYS};
    use crate::json::Json;
    use crate::scenario::Arguments;
    use crate::win32;

    /// Smallest, median and largest of signed nanoseconds, in microseconds.
    fn signed_summary(values: &[i64]) -> Json {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let micro = |value: i64| Json::Float(value as f64 / 1000.0);
        if sorted.is_empty() {
            return Json::object().with("count", 0_u64);
        }
        Json::object().with("count", sorted.len()).with("min_us", micro(sorted[0])).with("median_us", micro(sorted[(sorted.len() - 1) / 2])).with("max_us", micro(sorted[sorted.len() - 1]))
    }

    /// The step by which the system's tick count advances on this machine now.
    fn tick_step_ms() -> u32 {
        let started = Instant::now();
        let mut last = win32::tick_count_ms();
        let mut smallest = u32::MAX;
        while started.elapsed() < Duration::from_millis(120) {
            let now = win32::tick_count_ms();
            if now != last {
                smallest = smallest.min(now.wrapping_sub(last));
                last = now;
            }
            std::hint::spin_loop();
        }
        smallest
    }

    pub fn run(arguments: &Arguments) -> Result<Json, String> {
        let pump_period = Duration::from_millis(arguments.number("pump_ms", 1).max(1));
        let system_input = arguments.text("injection", "posted") == "system";
        let count = arguments.number("count", 200);
        let mut spec = arguments.window("render-windowed-presentation");
        // Input of the system goes to the window that has the keyboard
        // focus, so that injection asks for the focus; a posted message does
        // not need it.
        spec.activate = system_input;
        let mut host = host::open(&arguments.text("library", "winit"), &spec)?;
        let NativeWindow::Win32 { hwnd, .. } = host.native()? else { return Err("a Windows window was expected".to_string()) };
        for _ in 0..40 {
            host.pump(&mut |_| {});
            std::thread::sleep(Duration::from_millis(5));
        }
        PLATFORM_KEYS.lock().unwrap().clear();

        let injections: Arc<Mutex<Vec<(u64, u32)>>> = Arc::default();
        let delivered = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let refused = Arc::new(AtomicU64::new(0));
        let injector = {
            let (injections, delivered, done, refused) = (injections.clone(), delivered.clone(), done.clone(), refused.clone());
            std::thread::Builder::new()
                .name("injector".to_string())
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    for index in 0..count {
                        // The interval varies so that injections do not
                        // fall in step with the pump.
                        std::thread::sleep(Duration::from_millis(11) + Duration::from_micros((index * 7919) % 9000));
                        let before = delivered.load(Ordering::Relaxed);
                        let stamp = (now_ns(), win32::tick_count_ms());
                        if system_input {
                            if win32::send_injected_key(hwnd).is_err() {
                                refused.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                        } else {
                            win32::post_injected_key(hwnd);
                        }
                        injections.lock().unwrap().push(stamp);
                        let waited = Instant::now();
                        while delivered.load(Ordering::Relaxed) == before && waited.elapsed() < Duration::from_millis(500) {
                            std::thread::sleep(Duration::from_micros(200));
                        }
                    }
                    done.store(true, Ordering::Relaxed);
                })
                .map_err(|error| error.to_string())?
        };

        // The host's loop: one pump per period.
        let mut deliveries: Vec<(u64, Option<u64>)> = Vec::new();
        let started = Instant::now();
        let mut pumps = 0_u32;
        while !done.load(Ordering::Relaxed) {
            host.pump(&mut |event| {
                if event.key_down {
                    deliveries.push((event.delivered_ns, event.library_ns));
                    delivered.fetch_add(1, Ordering::Relaxed);
                }
            });
            pumps += 1;
            let next = started + pump_period * pumps;
            let now = Instant::now();
            if now < next {
                std::thread::sleep(next - now);
            }
        }
        host.pump(&mut |event| {
            if event.key_down {
                deliveries.push((event.delivered_ns, event.library_ns));
            }
        });
        let _ = injector.join();
        let access = host.platform_event_access();
        let library = host.library();
        host.close_window();

        let injections = injections.lock().unwrap().clone();
        let hooked = PLATFORM_KEYS.lock().unwrap().clone();
        let mut result = Json::object()
            .with("library_version", library)
            .with("injected", injections.len())
            .with("injections_refused", refused.load(Ordering::Relaxed))
            .with("delivered", deliveries.len())
            .with("seen_by_the_hook_beneath_the_library", hooked.len())
            .with("access_to_the_platform_event", access)
            .with("tick_count_step_ms", tick_step_ms());
        if refused.load(Ordering::Relaxed) > 0 {
            result.set("refusal", "the spike's window did not have the keyboard focus when the key was to be injected");
        }
        if injections.len() != deliveries.len() || injections.is_empty() {
            result.set("matched", false);
            return Ok(result);
        }
        result.set("matched", true);
        let delivery: Vec<u64> = injections.iter().zip(&deliveries).map(|(injection, delivery)| delivery.0.saturating_sub(injection.0)).collect();
        let mut sorted = delivery.clone();
        sorted.sort_unstable();
        let micro = |nanoseconds: u64| Json::Float(nanoseconds as f64 / 1000.0);
        result.set("host_receives_after_injection", Json::object().with("median_us", micro(percentile(&sorted, 0.5))).with("p99_us", micro(percentile(&sorted, 0.99))).with("max_us", micro(sorted.last().copied().unwrap_or(0))));
        let library_times: Vec<i64> = injections.iter().zip(&deliveries).filter_map(|(injection, delivery)| delivery.1.map(|time| time as i64 - injection.0 as i64)).collect();
        result.set("library_gives_a_time", !library_times.is_empty());
        if !library_times.is_empty() {
            result.set("library_time_minus_injection", signed_summary(&library_times));
            let before_delivery: Vec<i64> = deliveries.iter().filter_map(|delivery| delivery.1.map(|time| delivery.0 as i64 - time as i64)).collect();
            result.set("host_receives_after_the_library_time", signed_summary(&before_delivery));
        }
        if hooked.len() == injections.len() {
            let seen: Vec<i64> = injections.iter().zip(&hooked).map(|(injection, key)| key.seen_ns as i64 - injection.0 as i64).collect();
            result.set("hook_runs_after_injection", signed_summary(&seen));
            // The platform's own time, in its unit: milliseconds of the
            // tick count, against the tick count read at the injection.
            let mut platform: Vec<i64> = injections.iter().zip(&hooked).map(|(injection, key)| i64::from(key.platform_time_ms.wrapping_sub(injection.1) as i32)).collect();
            platform.sort_unstable();
            let mut distinct = platform.clone();
            distinct.dedup();
            result.set(
                "platform_time_minus_tick_count_at_injection_ms",
                Json::object().with("min", platform[0]).with("median", platform[(platform.len() - 1) / 2]).with("max", platform[platform.len() - 1]).with("distinct_values", distinct.len()),
            );
        }
        Ok(result)
    }
}
