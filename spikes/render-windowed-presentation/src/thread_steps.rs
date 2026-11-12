//! The scenario of the threads: the process's threads counted from the
//! system's list at each step (a windowing library started, a window opened,
//! a device and a swapchain created, frames presented, each released), each
//! thread attributed to the step at which it appeared, with the processor
//! time each set takes over an idle second. Throwaway, never product code.

use std::sync::mpsc;
use std::time::Duration;

use crate::host::{self, WindowHost};
use crate::json::Json;
use crate::presenter::{self, Mode, Presenter, Request};
use crate::scenario::Arguments;
use crate::threads::Ledger;

/// What the platform thread asks of the presentation thread.
enum Command {
    Open(Request, presenter::Prepared),
    Present(u64),
    Release,
}

pub fn run(arguments: &Arguments) -> Json {
    let header = Json::object()
        .with("scenario", "the threads of the process at each step")
        .with("library", arguments.text("library", "winit"))
        .with("path", arguments.text("path", "none"))
        .with("device_index", arguments.number("device", 0))
        .with("waiting_means_under_us_of_an_idle_second", crate::threads::WAITING_BELOW_NS / 1000);
    let mut ledger = Ledger::start("the process at its start");
    let outcome = run_inner(arguments, &mut ledger);
    let live = ledger.live_sets();
    let mut result = header.with("threads_left_at_the_end", live).with("steps", ledger.into_json());
    match outcome {
        Ok(facts) => {
            result.set("completed", true);
            if let Json::Object(members) = facts {
                for (key, value) in members {
                    result.set(&key, value);
                }
            }
        }
        Err(error) => {
            result.set("completed", false);
            result.set("error", error);
        }
    }
    result
}

fn settle(host: &mut dyn WindowHost) {
    for _ in 0..20 {
        host.pump(&mut |_| {});
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn run_inner(arguments: &Arguments, ledger: &mut Ledger) -> Result<Json, String> {
    let library = arguments.text("library", "winit");
    let path = arguments.text("path", "none");
    let mut facts = Json::object();
    let mut host = host::start(&library, false)?;
    ledger.step("the windowing library started", true);
    host.open_window(&arguments.window("render-windowed-presentation"))?;
    settle(host.as_mut());
    ledger.step("a window opened", true);
    facts.set("library_version", host.library());

    if library == "sdl3" && arguments.flag("subsystems") {
        // The two other ports SDL could serve, each started apart.
        for (name, flags) in [("SDL's gamepad subsystem started", sdl3_sys::init::SDL_INIT_GAMEPAD), ("SDL's audio subsystem started", sdl3_sys::init::SDL_INIT_AUDIO)] {
            match crate::host_sdl::start_subsystem(flags) {
                Ok(()) => {
                    settle(host.as_mut());
                    ledger.step(name, true);
                }
                Err(error) => facts.set(if flags == sdl3_sys::init::SDL_INIT_AUDIO { "audio_subsystem_error" } else { "gamepad_subsystem_error" }, error),
            }
        }
        facts.set("gamepads_listed", gamepads());
    }

    if path != "none" {
        let native = host.native()?;
        let size = host.pixel_size();
        host::publish_size(size.0, size.1);
        let request = Request { path: path.clone(), device: arguments.number("device", 0) as usize, mode: Mode::parse(&arguments.text("mode", "synchronized"))?, window: native, width: size.0, height: size.1, acquire_wait_ms: 1000 };
        let (commands, received) = mpsc::channel::<Command>();
        let (replies, answered) = mpsc::channel::<Result<Json, String>>();
        let thread = std::thread::Builder::new()
            .name("presentation".to_string())
            .spawn(move || {
                let mut presenter: Option<Box<dyn Presenter>> = None;
                for command in received {
                    let reply = match command {
                        Command::Open(request, prepared) => presenter::open(&request, prepared).map(|opened| {
                            let described = opened.describe();
                            presenter = Some(opened);
                            described
                        }),
                        Command::Present(frames) => match presenter.as_mut() {
                            Some(presenter) => (0..frames).try_for_each(|index| presenter.frame(index).map(|_| ())).map(|()| Json::Null),
                            None => Err("no presenter is open".to_string()),
                        },
                        Command::Release => {
                            presenter = None;
                            Ok(Json::Null)
                        }
                    };
                    if replies.send(reply).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        ledger.step("the spike's presentation thread started", false);
        // The platform thread pumps while the presentation thread works.
        let ask = |command: Command, host: &mut dyn WindowHost| -> Result<Json, String> {
            commands.send(command).map_err(|_| "the presentation thread is gone".to_string())?;
            loop {
                host.pump(&mut |_| {});
                match answered.try_recv() {
                    Ok(reply) => return reply,
                    Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(1)),
                    Err(mpsc::TryRecvError::Disconnected) => return Err("the presentation thread ended".to_string()),
                }
            }
        };
        let opened = match presenter::prepare(&request) {
            Ok(prepared) => ask(Command::Open(request, prepared), host.as_mut()),
            Err(error) => Err(error),
        };
        ledger.step("the device and the swapchain created", true);
        match opened {
            Ok(described) => {
                facts.set("presenter", described);
                let presented = ask(Command::Present(120), host.as_mut());
                ledger.step("120 frames presented", true);
                if let Err(error) = presented {
                    facts.set("presentation_error", error);
                }
            }
            Err(error) => facts.set("presenter_error", error),
        }
        let _ = ask(Command::Release, host.as_mut());
        ledger.step("the swapchain and the device released", true);
        drop(commands);
        let _ = thread.join();
        ledger.step("the spike's presentation thread ended", false);
    }

    host.close_window();
    settle(host.as_mut());
    ledger.step("the window closed", false);
    drop(host);
    ledger.step("the windowing library shut down", true);
    Ok(facts)
}

/// How many gamepads SDL lists once its gamepad subsystem is started.
fn gamepads() -> Json {
    let mut count = 0;
    // SAFETY: the call returns a list the caller frees, or null, and writes
    // the count.
    unsafe {
        let list = sdl3_sys::gamepad::SDL_GetGamepads(&mut count);
        if list.is_null() {
            return Json::Text(format!("none listed: {}", crate::host_sdl::last_error()));
        }
        sdl3_sys::stdinc::SDL_free(list.cast());
    }
    Json::from(i64::from(count))
}
