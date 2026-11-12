//! The scenario of the system's own move and resize loop on Windows: whether
//! a library returns to the host's loop while the window is moved or resized,
//! and what each of its remedies runs meanwhile. Throwaway, never product
//! code.

use crate::json::Json;
use crate::scenario::Arguments;

pub fn run(arguments: &Arguments) -> Json {
    let header = Json::object()
        .with("scenario", "the host's loop during a move and a resize by the system")
        .with("library", arguments.text("library", "winit"))
        .with("host_loop", arguments.text("host_loop", "the host pumps"));
    #[cfg(windows)]
    {
        let outcome = if arguments.text("host_loop", "the host pumps") == "the library calls back" { windows::run_main_callbacks(arguments) } else { windows::run_pumped(arguments) };
        match outcome {
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
        header.with("completed", false).with("not_run", "the loop in question is the one Windows runs while a window is moved or resized")
    }
}

#[cfg(windows)]
mod windows {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::time::Duration;

    use sdl3_sys::events::SDL_Event;
    use sdl3_sys::init::{SDL_APP_CONTINUE, SDL_APP_FAILURE, SDL_APP_SUCCESS, SDL_AppResult};

    use crate::clock::now_ns;
    use crate::host::{self, NativeWindow, WindowHost};
    use crate::json::Json;
    use crate::scenario::Arguments;
    use crate::win32::{self, ModalGesture};

    /// The move, then the resize, from a thread that is not the platform
    /// thread.
    fn gestures(window: usize) -> std::thread::JoinHandle<Vec<ModalGesture>> {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let moved = win32::modal_gesture(window, false, 15, Duration::from_millis(60));
            std::thread::sleep(Duration::from_millis(300));
            let resized = win32::modal_gesture(window, true, 15, Duration::from_millis(60));
            std::thread::sleep(Duration::from_millis(200));
            vec![moved, resized]
        })
    }

    fn sdl_event_name(kind: u32) -> &'static str {
        use sdl3_sys::events::*;
        match kind {
            kind if kind == SDL_EVENT_WINDOW_EXPOSED.0 => "exposed",
            kind if kind == SDL_EVENT_WINDOW_MOVED.0 => "moved",
            kind if kind == SDL_EVENT_WINDOW_RESIZED.0 => "resized",
            kind if kind == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED.0 => "pixel size changed",
            kind if kind == SDL_EVENT_KEY_DOWN.0 => "key down",
            kind if kind == SDL_EVENT_KEY_UP.0 => "key up",
            _ => "other",
        }
    }

    fn counted(names: impl Iterator<Item = &'static str>) -> Json {
        let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
        for name in names {
            *counts.entry(name).or_default() += 1;
        }
        Json::List(counts.into_iter().map(|(name, count)| Json::object().with("event", name).with("count", count)).collect())
    }

    /// What ran between the entry and the exit of the system's loop: the
    /// passes of the host's loop, the longest time without one, and the
    /// calls the library made meanwhile.
    fn during(gesture: &ModalGesture, iterations: &[u64], callbacks: &[(u64, &'static str)], watched: &[(u64, u32)]) -> Json {
        let inside = |time: u64| time > gesture.entered_ns && time < gesture.exited_ns;
        let mut longest = 0_u64;
        for pair in iterations.windows(2) {
            if pair[1] > gesture.entered_ns && pair[0] < gesture.exited_ns {
                longest = longest.max(pair[1] - pair[0]);
            }
        }
        let mut report = gesture.json.clone();
        report.set("passes_of_the_host_loop_during_the_gesture", iterations.iter().filter(|&&time| inside(time)).count());
        report.set("longest_time_without_a_pass_ms", Json::Float(longest as f64 / 1e6));
        report.set("events_the_host_received_during_the_gesture", counted(callbacks.iter().filter(|(time, _)| inside(*time)).map(|(_, name)| *name)));
        report.set("calls_of_the_event_watch_during_the_gesture", counted(watched.iter().filter(|(time, _)| inside(*time)).map(|(_, kind)| sdl_event_name(*kind))));
        report
    }

    /// The host owns the loop and pumps the library: winit's pump, SDL's poll
    /// (with its event watch installed, which is its remedy for a polling
    /// host).
    pub fn run_pumped(arguments: &Arguments) -> Result<Json, String> {
        let library = arguments.text("library", "winit");
        let mut host = host::open(&library, &arguments.window("render-windowed-presentation"))?;
        let NativeWindow::Win32 { hwnd, .. } = host.native()? else { return Err("a Windows window was expected".to_string()) };
        let _observer = win32::Observer::install(hwnd)?;
        for _ in 0..20 {
            host.pump(&mut |_| {});
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::host_sdl::WATCHED.lock().unwrap().clear();
        let thread = gestures(hwnd);
        let mut iterations: Vec<u64> = Vec::new();
        let mut callbacks: Vec<(u64, &'static str)> = Vec::new();
        while !thread.is_finished() {
            iterations.push(now_ns());
            host.pump(&mut |event| callbacks.push((event.delivered_ns, event.name)));
            std::thread::sleep(Duration::from_millis(1));
        }
        let done = thread.join().map_err(|_| "the gesture thread panicked".to_string())?;
        let watched = crate::host_sdl::WATCHED.lock().unwrap().clone();
        let version = host.library();
        host.close_window();
        Ok(Json::object()
            .with("library_version", version)
            .with("remedy", if library == "sdl3" { "an event watch, called by the library where the event arises" } else { "the application handler, which the library calls from its window procedure" })
            .with("gestures", Json::List(done.iter().map(|gesture| during(gesture, &iterations, &callbacks, &watched)).collect())))
    }

    /// The state of the callbacks SDL runs, all on the main thread.
    #[derive(Default)]
    struct Callbacks {
        host: Option<Box<dyn WindowHost>>,
        observer: Option<win32::Observer>,
        thread: Option<std::thread::JoinHandle<Vec<ModalGesture>>>,
        iterations: Vec<u64>,
        events: Vec<(u64, &'static str)>,
        error: Option<String>,
        version: String,
    }

    thread_local! {
        static CALLBACKS: RefCell<Callbacks> = RefCell::new(Callbacks::default());
    }

    unsafe extern "C" fn app_init(_state: *mut *mut c_void, _argc: i32, _argv: *mut *mut std::ffi::c_char) -> SDL_AppResult {
        let spec = crate::host::WindowSpec { title: "render-windowed-presentation".to_string(), borderless: false, width: 640, height: 360, activate: false, force_x11: false };
        let opened = host::open("sdl3", &spec).and_then(|host| {
            let NativeWindow::Win32 { hwnd, .. } = host.native()? else { return Err("a Windows window was expected".to_string()) };
            Ok((host, win32::Observer::install(hwnd)?, hwnd))
        });
        CALLBACKS.with(|callbacks| {
            let mut callbacks = callbacks.borrow_mut();
            match opened {
                Ok((host, observer, hwnd)) => {
                    callbacks.host = Some(host);
                    callbacks.observer = Some(observer);
                    callbacks.thread = Some(gestures(hwnd));
                    SDL_APP_CONTINUE
                }
                Err(error) => {
                    callbacks.error = Some(error);
                    SDL_APP_FAILURE
                }
            }
        })
    }

    unsafe extern "C" fn app_iterate(_state: *mut c_void) -> SDL_AppResult {
        let finished = CALLBACKS.with(|callbacks| {
            let mut callbacks = callbacks.borrow_mut();
            callbacks.iterations.push(now_ns());
            callbacks.thread.as_ref().is_none_or(|thread| thread.is_finished())
        });
        if finished {
            return SDL_APP_SUCCESS;
        }
        std::thread::sleep(Duration::from_millis(1));
        SDL_APP_CONTINUE
    }

    unsafe extern "C" fn app_event(_state: *mut c_void, event: *mut SDL_Event) -> SDL_AppResult {
        // SAFETY: SDL hands the callback a live event.
        let kind = unsafe { (*event).r#type };
        CALLBACKS.with(|callbacks| callbacks.borrow_mut().events.push((now_ns(), sdl_event_name(kind))));
        SDL_APP_CONTINUE
    }

    unsafe extern "C" fn app_quit(_state: *mut c_void, _result: SDL_AppResult) {
        // The window and the library are released here, before SDL shuts
        // itself down on its way out of the callbacks.
        CALLBACKS.with(|callbacks| {
            let mut callbacks = callbacks.borrow_mut();
            callbacks.version = callbacks.host.as_ref().map(|host| host.library()).unwrap_or_default();
            callbacks.observer = None;
            callbacks.host = None;
        });
    }

    /// The library owns the loop and calls the host back: SDL's model of
    /// main callbacks.
    pub fn run_main_callbacks(arguments: &Arguments) -> Result<Json, String> {
        if arguments.text("library", "sdl3") != "sdl3" {
            return Err("only SDL3 is measured with a loop it owns".to_string());
        }
        crate::host_sdl::WATCHED.lock().unwrap().clear();
        // SAFETY: called once on the main thread with four callbacks of
        // this module; it returns when a callback ends the application.
        let status = unsafe { sdl3_sys::main::SDL_EnterAppMainCallbacks(0, std::ptr::null_mut(), Some(app_init), Some(app_iterate), Some(app_event), Some(app_quit)) };
        let mut callbacks = CALLBACKS.with(|callbacks| std::mem::take(&mut *callbacks.borrow_mut()));
        if let Some(error) = callbacks.error.take() {
            return Err(error);
        }
        let done = callbacks.thread.take().ok_or("the gestures were never started")?.join().map_err(|_| "the gesture thread panicked".to_string())?;
        let watched = crate::host_sdl::WATCHED.lock().unwrap().clone();
        let version = std::mem::take(&mut callbacks.version);
        Ok(Json::object()
            .with("library_version", version)
            .with("remedy", "the library's main callbacks: it owns the loop and calls the host's iteration")
            .with("exit_status", status)
            .with("gestures", Json::List(done.iter().map(|gesture| during(gesture, &callbacks.iterations, &callbacks.events, &watched)).collect())))
    }
}
