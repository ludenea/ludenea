//! A window through winit, pumped from a loop the host owns. Throwaway, never
//! product code.

use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

use crate::clock::now_ns;
use crate::host::{HostEvent, NativeWindow, WindowHost, WindowSpec};

pub const VERSION: &str = "0.30.13";
pub const LICENSE: &str = "Apache-2.0";

struct App {
    wanted: Option<WindowSpec>,
    window: Option<Window>,
    error: Option<String>,
    events: Vec<HostEvent>,
}

impl App {
    fn create_wanted(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let Some(spec) = self.wanted.take() else { return };
        let mut attributes = Window::default_attributes()
            .with_title(spec.title.clone())
            .with_inner_size(PhysicalSize::new(spec.width, spec.height))
            .with_position(PhysicalPosition::new(60, 60))
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_active(spec.activate);
        if spec.borderless {
            // Not above the other windows: DXGI refuses a swapchain for a
            // winit window that is both full screen and above the others
            // (measured on three devices), and a window that covers its
            // display needs no such level.
            attributes = attributes.with_decorations(false).with_fullscreen(Some(Fullscreen::Borderless(None))).with_window_level(WindowLevel::Normal);
        }
        match event_loop.create_window(attributes) {
            Ok(window) => self.window = Some(window),
            Err(error) => self.error = Some(format!("winit created no window: {error}")),
        }
    }

    fn push(&mut self, name: &'static str, key_down: bool) {
        // winit's window events carry no time: the host takes one here.
        self.events.push(HostEvent { name, delivered_ns: now_ns(), library_ns: None, key_down });
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.create_wanted(event_loop);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.create_wanted(event_loop);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _window: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                // Published here, since during a resize by the system this
                // callback runs and the pump does not return.
                crate::host::publish_size(size.width, size.height);
                self.push("resized", false)
            }
            WindowEvent::Moved(_) => self.push("moved", false),
            WindowEvent::Focused(_) => self.push("focus", false),
            WindowEvent::Occluded(true) => self.push("occluded", false),
            WindowEvent::Occluded(false) => self.push("shown again", false),
            WindowEvent::RedrawRequested => self.push("redraw requested", false),
            WindowEvent::CloseRequested => self.push("close requested", false),
            WindowEvent::ScaleFactorChanged { .. } => self.push("scale factor changed", false),
            WindowEvent::KeyboardInput { event, .. } => match event.state {
                ElementState::Pressed => self.push("key down", true),
                ElementState::Released => self.push("key up", false),
            },
            _ => self.push("other", false),
        }
    }
}

pub struct WinitHost {
    // The application is declared first so that the window is dropped before
    // the event loop that created it.
    app: App,
    event_loop: EventLoop<()>,
}

impl WinitHost {
    pub fn start(force_x11: bool) -> Result<WinitHost, String> {
        let mut builder = EventLoop::builder();
        #[cfg(windows)]
        {
            use winit::platform::windows::EventLoopBuilderExtWindows;
            builder.with_msg_hook(|message| {
                // SAFETY: winit hands the hook the message it just retrieved.
                unsafe { crate::host::record_windows_message(message as *const crate::host::WindowsMessage) };
                false
            });
        }
        #[cfg(target_os = "linux")]
        if force_x11 {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }
        #[cfg(not(target_os = "linux"))]
        let _ = force_x11;
        let event_loop = builder.build().map_err(|error| format!("winit built no event loop: {error}"))?;
        Ok(WinitHost { app: App { wanted: None, window: None, error: None, events: Vec::new() }, event_loop })
    }

    fn pump_once(&mut self) {
        let _ = self.event_loop.pump_app_events(Some(Duration::ZERO), &mut self.app);
    }
}

impl WindowHost for WinitHost {
    fn library(&self) -> String {
        format!("winit {VERSION}")
    }

    fn open_window(&mut self, spec: &WindowSpec) -> Result<(), String> {
        self.app.wanted = Some(spec.clone());
        let started = Instant::now();
        while self.app.window.is_none() && self.app.error.is_none() {
            if started.elapsed() > Duration::from_secs(10) {
                return Err("winit created no window within ten seconds of pumping".to_string());
            }
            self.pump_once();
        }
        match self.app.error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn close_window(&mut self) {
        self.app.window = None;
        self.pump_once();
    }

    fn native(&self) -> Result<NativeWindow, String> {
        let window = self.app.window.as_ref().ok_or("no window is open")?;
        let handle = window.window_handle().map_err(|error| format!("no window handle: {error}"))?.as_raw();
        let display = window.display_handle().map_err(|error| format!("no display handle: {error}"))?.as_raw();
        match (handle, display) {
            (RawWindowHandle::Win32(handle), _) => {
                Ok(NativeWindow::Win32 { hwnd: handle.hwnd.get() as usize, hinstance: handle.hinstance.map_or(0, |instance| instance.get() as usize) })
            }
            (RawWindowHandle::Xlib(handle), RawDisplayHandle::Xlib(display)) => {
                Ok(NativeWindow::Xlib { display: display.display.map_or(0, |pointer| pointer.as_ptr() as usize), window: handle.window as u64 })
            }
            (RawWindowHandle::Xcb(handle), RawDisplayHandle::Xcb(display)) => {
                Ok(NativeWindow::Xcb { connection: display.connection.map_or(0, |pointer| pointer.as_ptr() as usize), window: handle.window.get() })
            }
            (RawWindowHandle::Wayland(handle), RawDisplayHandle::Wayland(display)) => {
                Ok(NativeWindow::Wayland { display: display.display.as_ptr() as usize, surface: handle.surface.as_ptr() as usize })
            }
            (RawWindowHandle::AppKit(handle), _) => Ok(NativeWindow::AppKit { view: handle.ns_view.as_ptr() as usize }),
            (other, _) => Err(format!("a window handle the spike does not present to: {other:?}")),
        }
    }

    fn pump(&mut self, sink: &mut dyn FnMut(HostEvent)) {
        self.pump_once();
        for event in self.app.events.drain(..) {
            sink(event);
        }
    }

    fn pixel_size(&self) -> (u32, u32) {
        self.app.window.as_ref().map_or((0, 0), |window| {
            let size = window.inner_size();
            (size.width, size.height)
        })
    }

    fn request_size(&mut self, width: u32, height: u32) {
        if let Some(window) = &self.app.window {
            let _ = window.request_inner_size(PhysicalSize::new(width, height));
        }
    }

    fn set_minimized(&mut self, minimized: bool) {
        if let Some(window) = &self.app.window {
            window.set_minimized(minimized);
        }
    }

    fn platform_event_access(&self) -> &'static str {
        if cfg!(windows) {
            "a hook the event loop's builder takes, called with each Windows message before it is translated"
        } else {
            "none found in the interface the spike uses"
        }
    }
}
