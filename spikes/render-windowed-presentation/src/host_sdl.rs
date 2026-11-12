//! A window through SDL3, polled from a loop the host owns. The library is
//! reached through its C interface. Throwaway, never product code.

use std::ffi::{CStr, c_int, c_void};
use std::sync::Mutex;

use sdl3_sys::error::SDL_GetError;
use sdl3_sys::events::{
    SDL_AddEventWatch, SDL_EVENT_KEY_DOWN, SDL_EVENT_KEY_UP, SDL_EVENT_QUIT, SDL_EVENT_WINDOW_EXPOSED, SDL_EVENT_WINDOW_FOCUS_GAINED,
    SDL_EVENT_WINDOW_FOCUS_LOST, SDL_EVENT_WINDOW_MINIMIZED, SDL_EVENT_WINDOW_MOVED, SDL_EVENT_WINDOW_OCCLUDED,
    SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED, SDL_EVENT_WINDOW_RESIZED, SDL_EVENT_WINDOW_RESTORED, SDL_Event, SDL_PollEvent,
};
use sdl3_sys::hints::{SDL_HINT_VIDEO_DRIVER, SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN, SDL_SetHint};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_InitFlags, SDL_InitSubSystem, SDL_Quit};
use sdl3_sys::properties::{SDL_GetNumberProperty, SDL_GetPointerProperty};
use sdl3_sys::timer::SDL_GetTicksNS;
use sdl3_sys::version::SDL_GetVersion;
use sdl3_sys::video::{
    SDL_CreateWindow, SDL_DestroyWindow, SDL_GetCurrentVideoDriver, SDL_GetWindowProperties, SDL_GetWindowSizeInPixels, SDL_MinimizeWindow,
    SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER, SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER, SDL_PROP_WINDOW_WIN32_HWND_POINTER,
    SDL_PROP_WINDOW_WIN32_INSTANCE_POINTER, SDL_PROP_WINDOW_X11_DISPLAY_POINTER, SDL_PROP_WINDOW_X11_WINDOW_NUMBER, SDL_RestoreWindow,
    SDL_SetWindowFullscreen, SDL_SetWindowPosition, SDL_SetWindowSize, SDL_ShowWindow, SDL_WINDOW_ALWAYS_ON_TOP, SDL_WINDOW_HIDDEN,
    SDL_WINDOW_RESIZABLE, SDL_Window,
};

use crate::clock::now_ns;
use crate::host::{HostEvent, NativeWindow, WindowHost, WindowSpec};

pub const LICENSE: &str = "Zlib";
pub const BINDING: &str = "sdl3-sys 0.7.3 (Zlib), which builds SDL from its sources and links it statically";

/// The library's last error as text.
pub fn last_error() -> String {
    // SAFETY: SDL_GetError returns a NUL-terminated string that lives until
    // the next SDL call on this thread.
    unsafe { CStr::from_ptr(SDL_GetError()) }.to_string_lossy().into_owned()
}

/// The version of the SDL library linked, as it reports it.
pub fn version() -> String {
    let version = SDL_GetVersion();
    format!("SDL {}.{}.{}", version / 1_000_000, (version / 1000) % 1000, version % 1000)
}

/// The video driver SDL chose, once the video subsystem is started.
pub fn video_driver() -> String {
    // SAFETY: the call returns a static string or null.
    let name = unsafe { SDL_GetCurrentVideoDriver() };
    if name.is_null() {
        "none".to_string()
    } else {
        // SAFETY: a non-null result is a NUL-terminated static string.
        unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned()
    }
}

/// Starts one more subsystem (the gamepads, the audio) and says whether it
/// started.
pub fn start_subsystem(flags: SDL_InitFlags) -> Result<(), String> {
    // SAFETY: SDL_InitSubSystem may be called at any time on the main thread.
    if unsafe { SDL_InitSubSystem(flags) } { Ok(()) } else { Err(last_error()) }
}


/// SDL's own clock read against the spike's, so that an event's timestamp is
/// placed on the spike's clock: the spike's time at SDL's zero.
fn clock_offset_ns() -> i64 {
    let mut best = (u64::MAX, 0_i64);
    for _ in 0..16 {
        let before = now_ns();
        // SAFETY: the call reads SDL's monotonic clock.
        let sdl = unsafe { SDL_GetTicksNS() };
        let after = now_ns();
        let width = after - before;
        if width < best.0 {
            best = (width, (before + width / 2) as i64 - sdl as i64);
        }
    }
    best.1
}

/// The calls an event watch received, with the time and the type of each: the
/// watch is the remedy SDL offers a polling host for the system's own loops.
pub static WATCHED: Mutex<Vec<(u64, u32)>> = Mutex::new(Vec::new());

unsafe extern "C" fn watch(_userdata: *mut c_void, event: *mut SDL_Event) -> bool {
    // SAFETY: SDL hands the watch a live event.
    let kind = unsafe { (*event).r#type };
    if kind == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED.0 {
        // Published here, since during a resize by the system the watch
        // runs and the poll does not return.
        // SAFETY: an event of this type is a window event.
        let (width, height) = unsafe { ((*event).window.data1, (*event).window.data2) };
        crate::host::publish_size(width.max(0) as u32, height.max(0) as u32);
    }
    if let Ok(mut watched) = WATCHED.lock() {
        watched.push((now_ns(), kind));
    }
    true
}

/// Installs the event watch that fills `WATCHED`.
pub fn add_event_watch() -> Result<(), String> {
    // SAFETY: the callback is a function of this module and takes no data.
    if unsafe { SDL_AddEventWatch(Some(watch), std::ptr::null_mut()) } { Ok(()) } else { Err(last_error()) }
}

#[cfg(windows)]
unsafe extern "C" fn message_hook(_userdata: *mut c_void, message: *mut sdl3_sys::system::MSG) -> bool {
    // SAFETY: SDL hands the hook the message it just retrieved.
    unsafe { crate::host::record_windows_message(message as *const crate::host::WindowsMessage) };
    true
}

/// Applies the window's attributes the two libraries are asked alike, and
/// creates it.
pub fn create_window(spec: &WindowSpec) -> Result<*mut SDL_Window, String> {
    let title = std::ffi::CString::new(spec.title.clone()).map_err(|error| error.to_string())?;
    // SAFETY: the hint names and values are NUL-terminated; the window is
    // created hidden, placed, then shown, on the thread that started video.
    unsafe {
        SDL_SetHint(SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN, if spec.activate { c"1".as_ptr() } else { c"0".as_ptr() });
        let window = SDL_CreateWindow(title.as_ptr(), spec.width as c_int, spec.height as c_int, SDL_WINDOW_HIDDEN | SDL_WINDOW_RESIZABLE | SDL_WINDOW_ALWAYS_ON_TOP);
        if window.is_null() {
            return Err(format!("SDL created no window: {}", last_error()));
        }
        SDL_SetWindowPosition(window, 60, 60);
        if spec.borderless {
            // With no exclusive mode set, SDL's full screen is a borderless
            // window that covers the display.
            SDL_SetWindowFullscreen(window, true);
        }
        SDL_ShowWindow(window);
        Ok(window)
    }
}

/// The native handles of an SDL window, from its properties.
pub fn native_of(window: *mut SDL_Window) -> Result<NativeWindow, String> {
    let driver = video_driver();
    // SAFETY: the window is live; the property names are the library's own.
    unsafe {
        let properties = SDL_GetWindowProperties(window);
        let pointer = |name| SDL_GetPointerProperty(properties, name, std::ptr::null_mut()) as usize;
        match driver.as_str() {
            "windows" => Ok(NativeWindow::Win32 { hwnd: pointer(SDL_PROP_WINDOW_WIN32_HWND_POINTER), hinstance: pointer(SDL_PROP_WINDOW_WIN32_INSTANCE_POINTER) }),
            "x11" => Ok(NativeWindow::Xlib { display: pointer(SDL_PROP_WINDOW_X11_DISPLAY_POINTER), window: SDL_GetNumberProperty(properties, SDL_PROP_WINDOW_X11_WINDOW_NUMBER, 0) as u64 }),
            "wayland" => Ok(NativeWindow::Wayland { display: pointer(SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER), surface: pointer(SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER) }),
            #[cfg(target_os = "macos")]
            "cocoa" => {
                let window = pointer(sdl3_sys::video::SDL_PROP_WINDOW_COCOA_WINDOW_POINTER);
                Ok(NativeWindow::AppKit { view: crate::metal::content_view(window) })
            }
            other => Err(format!("a video driver the spike does not present to: {other}")),
        }
    }
}

/// Translates one SDL event, placing its timestamp on the spike's clock.
pub fn translate(event: &SDL_Event, offset_ns: i64) -> HostEvent {
    // SAFETY: every SDL event starts with the common fields.
    let (kind, timestamp) = unsafe { (event.r#type, event.common.timestamp) };
    let named = |name: &'static str, key_down: bool| HostEvent { name, delivered_ns: now_ns(), library_ns: Some((timestamp as i64 + offset_ns).max(0) as u64), key_down };
    match kind {
        kind if kind == SDL_EVENT_KEY_DOWN.0 => named("key down", true),
        kind if kind == SDL_EVENT_KEY_UP.0 => named("key up", false),
        kind if kind == SDL_EVENT_WINDOW_RESIZED.0 => named("resized", false),
        kind if kind == SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED.0 => named("pixel size changed", false),
        kind if kind == SDL_EVENT_WINDOW_MOVED.0 => named("moved", false),
        kind if kind == SDL_EVENT_WINDOW_MINIMIZED.0 => named("minimized", false),
        kind if kind == SDL_EVENT_WINDOW_RESTORED.0 => named("restored", false),
        kind if kind == SDL_EVENT_WINDOW_OCCLUDED.0 => named("occluded", false),
        kind if kind == SDL_EVENT_WINDOW_EXPOSED.0 => named("exposed", false),
        kind if kind == SDL_EVENT_WINDOW_FOCUS_GAINED.0 || kind == SDL_EVENT_WINDOW_FOCUS_LOST.0 => named("focus", false),
        kind if kind == SDL_EVENT_QUIT.0 => named("close requested", false),
        _ => named("other", false),
    }
}

pub struct SdlHost {
    window: *mut SDL_Window,
    offset_ns: i64,
}

impl SdlHost {
    pub fn start(force_x11: bool) -> Result<SdlHost, String> {
        // SAFETY: called once on the main thread before any other SDL call;
        // the hint is NUL-terminated.
        unsafe {
            sdl3_sys::main::SDL_SetMainReady();
            if force_x11 {
                SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"x11".as_ptr());
            }
            if !SDL_InitSubSystem(SDL_INIT_VIDEO) {
                return Err(format!("SDL started no video subsystem: {}", last_error()));
            }
            #[cfg(windows)]
            sdl3_sys::system::SDL_SetWindowsMessageHook(Some(message_hook), std::ptr::null_mut());
        }
        // The watch publishes the window's size from inside the system's
        // resize loop and counts what SDL delivers there.
        add_event_watch()?;
        Ok(SdlHost { window: std::ptr::null_mut(), offset_ns: clock_offset_ns() })
    }
}

impl Drop for SdlHost {
    fn drop(&mut self) {
        self.close_window();
        // SAFETY: the library was started by `start` on this thread.
        unsafe { SDL_Quit() };
    }
}

impl WindowHost for SdlHost {
    fn library(&self) -> String {
        format!("{} ({}, through {BINDING}), video driver {}", version(), LICENSE, video_driver())
    }

    fn open_window(&mut self, spec: &WindowSpec) -> Result<(), String> {
        self.window = create_window(spec)?;
        Ok(())
    }

    fn close_window(&mut self) {
        if !self.window.is_null() {
            // SAFETY: the window was created by this host and is destroyed once.
            unsafe { SDL_DestroyWindow(self.window) };
            self.window = std::ptr::null_mut();
        }
    }

    fn native(&self) -> Result<NativeWindow, String> {
        if self.window.is_null() {
            return Err("no window is open".to_string());
        }
        native_of(self.window)
    }

    fn pump(&mut self, sink: &mut dyn FnMut(HostEvent)) {
        // SAFETY: an SDL event is plain data for which zero is valid; the
        // call fills it when it returns true.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            while SDL_PollEvent(&mut event) {
                sink(translate(&event, self.offset_ns));
            }
        }
    }

    fn pixel_size(&self) -> (u32, u32) {
        if self.window.is_null() {
            return (0, 0);
        }
        let (mut width, mut height) = (0, 0);
        // SAFETY: the window is live; the call writes two integers.
        unsafe { SDL_GetWindowSizeInPixels(self.window, &mut width, &mut height) };
        (width.max(0) as u32, height.max(0) as u32)
    }

    fn request_size(&mut self, width: u32, height: u32) {
        if !self.window.is_null() {
            // SAFETY: the window is live.
            unsafe { SDL_SetWindowSize(self.window, width as c_int, height as c_int) };
        }
    }

    fn set_minimized(&mut self, minimized: bool) {
        if !self.window.is_null() {
            // SAFETY: the window is live.
            unsafe {
                if minimized {
                    SDL_MinimizeWindow(self.window);
                } else {
                    SDL_RestoreWindow(self.window);
                }
            }
        }
    }

    fn platform_event_access(&self) -> &'static str {
        if cfg!(windows) {
            "a hook set by a call of the library, called with each Windows message before it is translated"
        } else if cfg!(target_os = "linux") {
            "a hook set by a call of the library for X11 events; none found for Wayland"
        } else {
            "none found in the interface the spike uses"
        }
    }
}
