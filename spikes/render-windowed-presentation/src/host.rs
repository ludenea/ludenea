//! What the spike asks of a windowing library, said once for winit and for
//! SDL3: a window, its native handles, a pump the host calls from its own
//! loop, and the events with the time the library gives them, if it gives
//! one. Throwaway, never product code.

use std::sync::Mutex;

/// The native handles of a window, as plain integers so that the presentation
/// thread can take them.
#[derive(Clone, Copy, Debug)]
pub enum NativeWindow {
    Win32 { hwnd: usize, hinstance: usize },
    Xlib { display: usize, window: u64 },
    Xcb { connection: usize, window: u32 },
    Wayland { display: usize, surface: usize },
    AppKit { view: usize },
}


/// The window the spike asks for.
#[derive(Clone, Debug)]
pub struct WindowSpec {
    pub title: String,
    /// A borderless window that covers its display, or a decorated window.
    pub borderless: bool,
    pub width: u32,
    pub height: u32,
    /// Whether the window takes the keyboard focus when shown. Off by
    /// default, so that a run does not take the focus of a person at work.
    pub activate: bool,
    /// Ask the library for X11 whatever the session says (Linux): the host's
    /// second request after a Wayland connection failed.
    pub force_x11: bool,
}

/// One event a library delivered.
#[derive(Clone, Debug)]
pub struct HostEvent {
    pub name: &'static str,
    /// When the host received it, on the spike's clock.
    pub delivered_ns: u64,
    /// The time the library gives the event, converted to the spike's clock,
    /// when it gives one.
    pub library_ns: Option<u64>,
    /// A key going down: the form of the events the spike injects.
    pub key_down: bool,
}

/// One windowing library started in the process, its window when one is open,
/// and the pump of its events. Dropping it shuts the library down.
pub trait WindowHost {
    fn library(&self) -> String;
    fn open_window(&mut self, spec: &WindowSpec) -> Result<(), String>;
    fn close_window(&mut self);
    fn native(&self) -> Result<NativeWindow, String>;
    /// Delivers the events that are waiting, without blocking.
    fn pump(&mut self, sink: &mut dyn FnMut(HostEvent));
    /// The size of the window's drawable area in pixels, as the library
    /// reports it.
    fn pixel_size(&self) -> (u32, u32);
    /// Resizes the window by a call (where no input can be injected).
    fn request_size(&mut self, width: u32, height: u32);
    /// Minimizes or restores the window by a call.
    fn set_minimized(&mut self, minimized: bool);
    /// How the host reaches the platform's own event beneath the library.
    fn platform_event_access(&self) -> &'static str;
}

/// The platform's own event for an injected key, as a hook beneath the
/// library saw it: the platform's time in its own unit (milliseconds of the
/// system's tick count on Windows) and when the hook ran on the spike's clock.
#[derive(Clone, Copy, Debug)]
pub struct PlatformKey {
    pub platform_time_ms: u32,
    pub seen_ns: u64,
}

pub static PLATFORM_KEYS: Mutex<Vec<PlatformKey>> = Mutex::new(Vec::new());

/// The virtual key the spike injects: a function key no keyboard carries, so
/// that it means nothing to any other program.
pub const INJECTED_VIRTUAL_KEY: usize = 0x87;

/// The Windows message structure both libraries hand their hook.
#[cfg(windows)]
#[repr(C)]
pub struct WindowsMessage {
    pub window: isize,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub point: [i32; 2],
}

/// Records the platform's event if it is the injected key going down.
///
/// # Safety
///
/// `message` must point to a live Windows message structure.
#[cfg(windows)]
pub unsafe fn record_windows_message(message: *const WindowsMessage) {
    const WM_KEYDOWN: u32 = 0x0100;
    // SAFETY: the caller states that the pointer is a live message.
    let message = unsafe { &*message };
    if message.message == WM_KEYDOWN && message.wparam == INJECTED_VIRTUAL_KEY {
        if let Ok(mut keys) = PLATFORM_KEYS.lock() {
            keys.push(PlatformKey { platform_time_ms: message.time, seen_ns: crate::clock::now_ns() });
        }
    }
}

/// Starts the library named, with no window yet.
pub fn start(library: &str, force_x11: bool) -> Result<Box<dyn WindowHost>, String> {
    match library {
        "winit" => Ok(Box::new(crate::host_winit::WinitHost::start(force_x11)?)),
        "sdl3" => Ok(Box::new(crate::host_sdl::SdlHost::start(force_x11)?)),
        other => Err(format!("unknown windowing library: {other}")),
    }
}

/// Starts the library named and opens the window.
pub fn open(library: &str, spec: &WindowSpec) -> Result<Box<dyn WindowHost>, String> {
    let mut host = start(library, spec.force_x11)?;
    host.open_window(spec)?;
    Ok(host)
}

/// The size of the window's drawable area in pixels, published by whichever
/// code learns it first: the pump, or a callback the library runs inside the
/// system's own resize loop, where the pump does not return. It is the part
/// of the surface source a presenter on another thread reads.
static LIVE_SIZE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn publish_size(width: u32, height: u32) {
    LIVE_SIZE.store((u64::from(width) << 32) | u64::from(height), std::sync::atomic::Ordering::Relaxed);
}

pub fn live_size() -> (u32, u32) {
    let size = LIVE_SIZE.load(std::sync::atomic::Ordering::Relaxed);
    ((size >> 32) as u32, size as u32)
}
