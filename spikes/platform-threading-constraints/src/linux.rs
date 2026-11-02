//! Linux probes: whether a display server exists on the machine, whether X11
//! connections and windows may be created and read from any thread, whether a
//! Wayland compositor is reachable, and whether a Vulkan loader and device
//! exist. The libraries are loaded at run time so that the probe records their
//! absence instead of failing to start.

use crate::{ProbeSpec, Report};
use std::ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
const VIRTUAL_DISPLAY: &str = ":99";
const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;
const MAP_NOTIFY: i32 = 19;
const EVENT_WAIT: Duration = Duration::from_secs(3);

pub const PROBES: &[ProbeSpec] = &[
    ProbeSpec {
        name: "display_present",
        question: "Is an X11 server or a Wayland compositor reachable, and are their client libraries installed?",
        needs_display: true,
    },
    ProbeSpec {
        name: "x11_window_off_main",
        question: "Can a worker thread open its own X11 connection, create a window, map it and receive the map event?",
        needs_display: true,
    },
    ProbeSpec {
        name: "x11_shared_connection_across_threads",
        question: "With XInitThreads, can the main thread read events from, and close, a connection another thread opened and created a window on?",
        needs_display: true,
    },
    ProbeSpec {
        name: "x11_two_connections_two_threads",
        question: "Can two worker threads each hold their own X11 connection and window at the same time, without XInitThreads?",
        needs_display: true,
    },
    ProbeSpec {
        name: "wayland_connect",
        question: "Is a Wayland compositor reachable from a worker thread?",
        needs_display: false,
    },
    ProbeSpec {
        name: "vulkan_instance_off_main",
        question: "Does a Vulkan loader exist, can a worker thread create an instance and list physical devices, and can the main thread destroy the instance?",
        needs_display: false,
    },
];

/// The display session of the parent: the environment as found, and a virtual
/// X server started for the probes that need a display when the image has one
/// and no display is set.
pub struct Session {
    xvfb_path: Option<PathBuf>,
    xvfb: Option<Child>,
    xvfb_started: bool,
    xvfb_error: String,
}

impl Session {
    pub fn open() -> Self {
        let mut session = Session {
            xvfb_path: find_executable("Xvfb"),
            xvfb: None,
            xvfb_started: false,
            xvfb_error: String::new(),
        };
        if std::env::var_os("DISPLAY").is_none() {
            session.start_virtual_display();
        }
        session
    }

    fn start_virtual_display(&mut self) {
        let Some(path) = &self.xvfb_path else {
            self.xvfb_error = "Xvfb not found on PATH".to_string();
            return;
        };
        match Command::new(path)
            .args([VIRTUAL_DISPLAY, "-screen", "0", "320x240x24", "-nolisten", "tcp"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.xvfb = Some(child);
                let socket = Path::new("/tmp/.X11-unix/X99");
                let deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < deadline {
                    if socket.exists() {
                        self.xvfb_started = true;
                        return;
                    }
                    if let Some(Ok(Some(status))) = self.xvfb.as_mut().map(Child::try_wait) {
                        self.xvfb_error = format!("Xvfb exited early: {status}");
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                self.xvfb_error = "Xvfb socket did not appear within 10 s".to_string();
            }
            Err(error) => self.xvfb_error = format!("Xvfb failed to start: {error}"),
        }
    }

    pub fn facts(&self) -> Vec<(String, String)> {
        let mut facts: Vec<(String, String)> = ["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "XDG_RUNTIME_DIR"]
            .iter()
            .map(|name| {
                (
                    name.to_string(),
                    std::env::var(name).unwrap_or_else(|_| "(unset)".to_string()),
                )
            })
            .collect();
        facts.push((
            "xvfb_found".to_string(),
            self.xvfb_path
                .as_ref()
                .map_or("no".to_string(), |path| path.display().to_string()),
        ));
        facts.push(("xvfb_started".to_string(), self.xvfb_started.to_string()));
        facts.push((
            "xvfb_display".to_string(),
            if self.xvfb_started { VIRTUAL_DISPLAY.to_string() } else { "(none)".to_string() },
        ));
        facts.push(("xvfb_error".to_string(), self.xvfb_error.clone()));
        facts
    }

    pub fn environment_for(&self, spec: &ProbeSpec) -> Vec<(String, String)> {
        if spec.needs_display && self.xvfb_started {
            vec![("DISPLAY".to_string(), VIRTUAL_DISPLAY.to_string())]
        } else {
            Vec::new()
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut child) = self.xvfb.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

pub fn run(name: &str) -> Report {
    match name {
        "display_present" => display_present(),
        "x11_window_off_main" => x11_window_off_main(),
        "x11_shared_connection_across_threads" => x11_shared_connection_across_threads(),
        "x11_two_connections_two_threads" => x11_two_connections_two_threads(),
        "wayland_connect" => wayland_connect(),
        "vulkan_instance_off_main" => vulkan_instance_off_main(),
        other => Report::new("unsupported").detail("unknown_probe", other),
    }
}

/// A shared library loaded at run time; never closed, so that the function
/// pointers taken from it stay valid for the probe's life.
struct Library(*mut c_void);

impl Library {
    fn open(name: &str) -> Option<Library> {
        let name = CString::new(name).expect("library names have no interior nul");
        let handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
        if handle.is_null() { None } else { Some(Library(handle)) }
    }

    /// Looks a function up and reinterprets it as `T`, a function pointer type
    /// the caller states; the only unsafety is the stated signature.
    unsafe fn function<T: Copy>(&self, name: &str) -> Option<T> {
        let name = CString::new(name).expect("symbol names have no interior nul");
        let pointer = unsafe { dlsym(self.0, name.as_ptr()) };
        if pointer.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute_copy(&pointer) })
        }
    }
}

type Display = c_void;
type Window = c_ulong;

#[repr(C)]
struct XEvent {
    data: [c_long; 24],
}

impl XEvent {
    fn zeroed() -> Self {
        XEvent { data: [0; 24] }
    }

    fn kind(&self) -> i32 {
        self.data[0] as i32
    }
}

/// The Xlib functions the probes use, loaded once per process.
#[derive(Clone, Copy)]
struct X11 {
    init_threads: unsafe extern "C" fn() -> c_int,
    open_display: unsafe extern "C" fn(*const c_char) -> *mut Display,
    close_display: unsafe extern "C" fn(*mut Display) -> c_int,
    default_screen: unsafe extern "C" fn(*mut Display) -> c_int,
    root_window: unsafe extern "C" fn(*mut Display, c_int) -> Window,
    create_simple_window:
        unsafe extern "C" fn(*mut Display, Window, c_int, c_int, c_uint, c_uint, c_uint, c_ulong, c_ulong) -> Window,
    select_input: unsafe extern "C" fn(*mut Display, Window, c_long) -> c_int,
    map_window: unsafe extern "C" fn(*mut Display, Window) -> c_int,
    flush: unsafe extern "C" fn(*mut Display) -> c_int,
    pending: unsafe extern "C" fn(*mut Display) -> c_int,
    next_event: unsafe extern "C" fn(*mut Display, *mut XEvent) -> c_int,
    destroy_window: unsafe extern "C" fn(*mut Display, Window) -> c_int,
}

impl X11 {
    fn load() -> Result<X11, String> {
        let library = Library::open("libX11.so.6").ok_or("libX11.so.6 not found")?;
        unsafe {
            Ok(X11 {
                init_threads: library.function("XInitThreads").ok_or("XInitThreads")?,
                open_display: library.function("XOpenDisplay").ok_or("XOpenDisplay")?,
                close_display: library.function("XCloseDisplay").ok_or("XCloseDisplay")?,
                default_screen: library.function("XDefaultScreen").ok_or("XDefaultScreen")?,
                root_window: library.function("XRootWindow").ok_or("XRootWindow")?,
                create_simple_window: library.function("XCreateSimpleWindow").ok_or("XCreateSimpleWindow")?,
                select_input: library.function("XSelectInput").ok_or("XSelectInput")?,
                map_window: library.function("XMapWindow").ok_or("XMapWindow")?,
                flush: library.function("XFlush").ok_or("XFlush")?,
                pending: library.function("XPending").ok_or("XPending")?,
                next_event: library.function("XNextEvent").ok_or("XNextEvent")?,
                destroy_window: library.function("XDestroyWindow").ok_or("XDestroyWindow")?,
            })
        }
    }

    /// Opens the display named by the environment, creates and maps a small
    /// window that asks for structure events, and flushes.
    unsafe fn open_and_map(&self) -> Result<(*mut Display, Window), &'static str> {
        unsafe {
            let display = (self.open_display)(std::ptr::null());
            if display.is_null() {
                return Err("XOpenDisplay returned null");
            }
            let screen = (self.default_screen)(display);
            let root = (self.root_window)(display, screen);
            let window = (self.create_simple_window)(display, root, 0, 0, 64, 64, 0, 0, 0);
            if window == 0 {
                (self.close_display)(display);
                return Err("XCreateSimpleWindow returned 0");
            }
            (self.select_input)(display, window, STRUCTURE_NOTIFY_MASK);
            (self.map_window)(display, window);
            (self.flush)(display);
            Ok((display, window))
        }
    }

    /// Reads events until a MapNotify arrives or the wait expires; returns the
    /// number of events read and whether MapNotify was among them.
    unsafe fn wait_for_map(&self, display: *mut Display) -> (u32, bool) {
        let deadline = Instant::now() + EVENT_WAIT;
        let mut events = 0u32;
        while Instant::now() < deadline {
            unsafe {
                if (self.pending)(display) > 0 {
                    let mut event = XEvent::zeroed();
                    (self.next_event)(display, &mut event);
                    events += 1;
                    if event.kind() == MAP_NOTIFY {
                        return (events, true);
                    }
                } else {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        (events, false)
    }
}

fn display_name() -> String {
    std::env::var("DISPLAY").unwrap_or_else(|_| "(unset)".to_string())
}

fn display_present() -> Report {
    let mut report = Report::new("unavailable")
        .detail("DISPLAY", display_name())
        .detail(
            "WAYLAND_DISPLAY",
            std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "(unset)".to_string()),
        );
    let mut reachable = false;
    match X11::load() {
        Ok(x11) => {
            report = report.detail("libX11", "found");
            let display = unsafe { (x11.open_display)(std::ptr::null()) };
            report = report.detail("x11_server_reachable", !display.is_null());
            if !display.is_null() {
                reachable = true;
                unsafe { (x11.close_display)(display) };
            }
        }
        Err(error) => report = report.detail("libX11", error),
    }
    match Library::open("libwayland-client.so.0") {
        Some(library) => {
            report = report.detail("libwayland_client", "found");
            let connect: Option<unsafe extern "C" fn(*const c_char) -> *mut c_void> =
                unsafe { library.function("wl_display_connect") };
            let disconnect: Option<unsafe extern "C" fn(*mut c_void)> =
                unsafe { library.function("wl_display_disconnect") };
            match (connect, disconnect) {
                (Some(connect), Some(disconnect)) => {
                    let display = unsafe { connect(std::ptr::null()) };
                    report = report.detail("wayland_compositor_reachable", !display.is_null());
                    if !display.is_null() {
                        reachable = true;
                        unsafe { disconnect(display) };
                    }
                }
                _ => report = report.detail("wayland_compositor_reachable", "symbols missing"),
            }
        }
        None => report = report.detail("libwayland_client", "not found"),
    }
    if reachable {
        report.outcome = "succeeded";
    }
    report
}

fn x11_window_off_main() -> Report {
    let x11 = match X11::load() {
        Ok(x11) => x11,
        Err(error) => return Report::new("unavailable").detail("libX11", error),
    };
    unsafe { (x11.init_threads)() };
    let display = display_name();
    thread::spawn(move || unsafe {
        match x11.open_and_map() {
            Ok((connection, window)) => {
                let (events, mapped) = x11.wait_for_map(connection);
                (x11.destroy_window)(connection, window);
                (x11.close_display)(connection);
                Report::new(if mapped { "succeeded" } else { "failed" })
                    .detail("DISPLAY", display)
                    .detail("events_read", events)
                    .detail("map_notify_received", mapped)
            }
            Err(reason) => Report::new("unavailable").detail("DISPLAY", display).detail("reason", reason),
        }
    })
    .join()
    .expect("the worker thread does not panic")
}

fn x11_shared_connection_across_threads() -> Report {
    let x11 = match X11::load() {
        Ok(x11) => x11,
        Err(error) => return Report::new("unavailable").detail("libX11", error),
    };
    let init_threads = unsafe { (x11.init_threads)() };
    let (to_main, from_worker) = mpsc::channel::<Result<(usize, Window), &'static str>>();
    let (to_worker, from_main) = mpsc::channel::<()>();
    let worker = thread::spawn(move || unsafe {
        match x11.open_and_map() {
            Ok((connection, window)) => {
                let _ = to_main.send(Ok((connection as usize, window)));
                let _ = from_main.recv();
            }
            Err(reason) => {
                let _ = to_main.send(Err(reason));
            }
        }
    });
    let report = match from_worker.recv() {
        Ok(Ok((connection, window))) => {
            let connection = connection as *mut Display;
            let (events, mapped) = unsafe { x11.wait_for_map(connection) };
            let destroyed = unsafe { (x11.destroy_window)(connection, window) };
            let closed = unsafe { (x11.close_display)(connection) };
            Report::new(if mapped { "succeeded" } else { "failed" })
                .detail("DISPLAY", display_name())
                .detail("xinitthreads_returned", init_threads)
                .detail("events_read_by_main_thread", events)
                .detail("map_notify_received_by_main_thread", mapped)
                .detail("destroy_from_main_thread_returned", destroyed)
                .detail("close_from_main_thread_returned", closed)
        }
        Ok(Err(reason)) => Report::new("unavailable").detail("DISPLAY", display_name()).detail("reason", reason),
        Err(_) => Report::new("unavailable").detail("worker", "ended without a report"),
    };
    let _ = to_worker.send(());
    let _ = worker.join();
    report
}

fn x11_two_connections_two_threads() -> Report {
    let x11 = match X11::load() {
        Ok(x11) => x11,
        Err(error) => return Report::new("unavailable").detail("libX11", error),
    };
    let workers: Vec<_> = (0..2)
        .map(|_| {
            thread::spawn(move || unsafe {
                match x11.open_and_map() {
                    Ok((connection, window)) => {
                        let (events, mapped) = x11.wait_for_map(connection);
                        (x11.destroy_window)(connection, window);
                        (x11.close_display)(connection);
                        Ok((events, mapped))
                    }
                    Err(reason) => Err(reason),
                }
            })
        })
        .collect();
    let mut report = Report::new("succeeded").detail("DISPLAY", display_name());
    for (index, worker) in workers.into_iter().enumerate() {
        match worker.join().expect("the worker thread does not panic") {
            Ok((events, mapped)) => {
                if !mapped {
                    report.outcome = "failed";
                }
                report = report
                    .detail(&format!("thread_{index}_events_read"), events)
                    .detail(&format!("thread_{index}_map_notify_received"), mapped);
            }
            Err(reason) => {
                report.outcome = "unavailable";
                report = report.detail(&format!("thread_{index}_reason"), reason);
            }
        }
    }
    report
}

fn wayland_connect() -> Report {
    thread::spawn(|| {
        let Some(library) = Library::open("libwayland-client.so.0") else {
            return Report::new("unavailable").detail("libwayland_client", "not found");
        };
        let connect: Option<unsafe extern "C" fn(*const c_char) -> *mut c_void> =
            unsafe { library.function("wl_display_connect") };
        let disconnect: Option<unsafe extern "C" fn(*mut c_void)> = unsafe { library.function("wl_display_disconnect") };
        let (Some(connect), Some(disconnect)) = (connect, disconnect) else {
            return Report::new("unavailable").detail("libwayland_client", "symbols missing");
        };
        let display = unsafe { connect(std::ptr::null()) };
        let connected = !display.is_null();
        if connected {
            unsafe { disconnect(display) };
        }
        Report::new(if connected { "succeeded" } else { "unavailable" })
            .detail("libwayland_client", "found")
            .detail(
                "WAYLAND_DISPLAY",
                std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "(unset)".to_string()),
            )
            .detail("compositor_reachable", connected)
    })
    .join()
    .expect("the worker thread does not panic")
}

#[repr(C)]
struct VkInstanceCreateInfo {
    s_type: u32,
    next: *const c_void,
    flags: u32,
    application_info: *const c_void,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
}

/// Large enough for `VkPhysicalDeviceProperties` (816 bytes in the 1.x
/// headers), aligned for its 64-bit limits.
#[repr(C, align(8))]
struct VkPhysicalDeviceProperties([u8; 1024]);

const VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO: u32 = 1;
const VK_SUCCESS: i32 = 0;
const VK_ERROR_INCOMPATIBLE_DRIVER: i32 = -9;
const VK_DEVICE_TYPE_OFFSET: usize = 16;
const VK_DEVICE_NAME_OFFSET: usize = 20;
const VK_DEVICE_NAME_LENGTH: usize = 256;

fn vulkan_instance_off_main() -> Report {
    type CreateInstance = unsafe extern "C" fn(*const VkInstanceCreateInfo, *const c_void, *mut *mut c_void) -> i32;
    type DestroyInstance = unsafe extern "C" fn(*mut c_void, *const c_void);
    type EnumeratePhysicalDevices = unsafe extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32;
    type GetPhysicalDeviceProperties = unsafe extern "C" fn(*mut c_void, *mut VkPhysicalDeviceProperties);

    let Some(library) = Library::open("libvulkan.so.1") else {
        return Report::new("unavailable").detail("libvulkan", "not found");
    };
    let functions = unsafe {
        (
            library.function::<CreateInstance>("vkCreateInstance"),
            library.function::<DestroyInstance>("vkDestroyInstance"),
            library.function::<EnumeratePhysicalDevices>("vkEnumeratePhysicalDevices"),
            library.function::<GetPhysicalDeviceProperties>("vkGetPhysicalDeviceProperties"),
        )
    };
    let (Some(create), Some(destroy), Some(enumerate), Some(properties)) = functions else {
        return Report::new("unavailable").detail("libvulkan", "symbols missing");
    };
    let worker = thread::spawn(move || {
        let info = VkInstanceCreateInfo {
            s_type: VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
            next: std::ptr::null(),
            flags: 0,
            application_info: std::ptr::null(),
            enabled_layer_count: 0,
            enabled_layer_names: std::ptr::null(),
            enabled_extension_count: 0,
            enabled_extension_names: std::ptr::null(),
        };
        let mut instance: *mut c_void = std::ptr::null_mut();
        let result = unsafe { create(&info, std::ptr::null(), &mut instance) };
        if result != VK_SUCCESS {
            return (result, 0usize, Vec::new());
        }
        let mut count = 0u32;
        unsafe { enumerate(instance, &mut count, std::ptr::null_mut()) };
        let mut devices: Vec<*mut c_void> = vec![std::ptr::null_mut(); count as usize];
        if count > 0 {
            unsafe { enumerate(instance, &mut count, devices.as_mut_ptr()) };
        }
        let mut names = Vec::new();
        for device in devices.iter().take(count as usize) {
            let mut buffer = VkPhysicalDeviceProperties([0; 1024]);
            unsafe { properties(*device, &mut buffer) };
            let device_type = u32::from_ne_bytes(
                buffer.0[VK_DEVICE_TYPE_OFFSET..VK_DEVICE_TYPE_OFFSET + 4]
                    .try_into()
                    .expect("four bytes"),
            );
            let name_bytes = &buffer.0[VK_DEVICE_NAME_OFFSET..VK_DEVICE_NAME_OFFSET + VK_DEVICE_NAME_LENGTH];
            let end = name_bytes.iter().position(|byte| *byte == 0).unwrap_or(VK_DEVICE_NAME_LENGTH);
            let name = String::from_utf8_lossy(&name_bytes[..end]).into_owned();
            names.push(format!("{name} (type {device_type})"));
        }
        (result, instance as usize, names)
    });
    let (result, instance, names) = worker.join().expect("the worker thread does not panic");
    if result != VK_SUCCESS {
        let outcome = if result == VK_ERROR_INCOMPATIBLE_DRIVER { "unavailable" } else { "failed" };
        return Report::new(outcome).detail("libvulkan", "found").detail("vk_create_instance_result", result);
    }
    unsafe { destroy(instance as *mut c_void, std::ptr::null()) };
    Report::new(if names.is_empty() { "unavailable" } else { "succeeded" })
        .detail("libvulkan", "found")
        .detail("vk_create_instance_result", result)
        .detail("physical_device_count", names.len())
        .detail("physical_devices", names.join("; "))
        .detail("instance_destroyed_from", "main thread")
}
