//! A probe that links winit and nothing else of the spike: what the library
//! does when asked for a window on this machine, with a display or without
//! one, and, when its first request fails, what a second request for X11
//! does in the same process. Its binary is also the one whose imports say
//! what linking winit brings. Throwaway, never product code.

use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId};

#[derive(Default)]
struct App {
    window: Option<Window>,
    error: Option<String>,
}

impl App {
    fn create(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() && self.error.is_none() {
            match event_loop.create_window(Window::default_attributes().with_title("windowing probe").with_active(false)) {
                Ok(window) => self.window = Some(window),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.create(event_loop);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.create(event_loop);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _window: WindowId, _event: WindowEvent) {}
}

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            control if (control as u32) < 0x20 => out.push(' '),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// One request for an event loop and a window, and what winit answered.
fn request(force_x11: bool) -> (bool, String) {
    let asked = if force_x11 { "X11, named by the host" } else { "the library's own choice" };
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "linux")]
    if force_x11 {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }
    let mut event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(error) => return (false, format!("{{\"asked\":{},\"event_loop\":{},\"window\":null}}", quoted(asked), quoted(&format!("error: {error}")))),
    };
    let mut app = App::default();
    let started = Instant::now();
    while app.window.is_none() && app.error.is_none() && started.elapsed() < Duration::from_secs(5) {
        let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut app);
    }
    let (opened, window) = match (&app.window, &app.error) {
        (Some(window), _) => {
            let system = match window.window_handle().map(|handle| handle.as_raw()) {
                Ok(RawWindowHandle::Win32(_)) => "win32",
                Ok(RawWindowHandle::Xlib(_)) | Ok(RawWindowHandle::Xcb(_)) => "x11",
                Ok(RawWindowHandle::Wayland(_)) => "wayland",
                Ok(RawWindowHandle::AppKit(_)) => "appkit",
                _ => "another display system",
            };
            (true, format!("opened on {system}"))
        }
        (None, Some(error)) => (false, format!("error: {error}")),
        (None, None) => (false, "error: no window within five seconds of pumping".to_string()),
    };
    app.window = None;
    let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut app);
    (opened, format!("{{\"asked\":{},\"event_loop\":\"created\",\"window\":{}}}", quoted(asked), quoted(&window)))
}

fn main() {
    let force_x11 = std::env::args().any(|argument| argument == "--x11");
    let (opened, first) = request(force_x11);
    let mut requests = vec![first];
    if !opened && !force_x11 && cfg!(target_os = "linux") {
        requests.push(request(true).1);
    }
    println!("{{\"library\":\"winit 0.30.13\",\"requests\":[{}],\"the_process_went_on\":true}}", requests.join(","));
}
