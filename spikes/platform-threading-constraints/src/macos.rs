//! macOS probes: AppKit's main-thread rules for the application object,
//! windows and events, run loops on worker threads, and Metal device creation
//! off the main thread. AppKit is reached through the Objective-C runtime.

use crate::{ProbeSpec, Report};
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::sync::mpsc;
use std::thread;

type Id = *mut c_void;
type Sel = *mut c_void;

#[repr(C)]
struct NsRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
}

// Declared with the unwinding ABI: an Objective-C exception thrown by AppKit
// then propagates through the probe's frames to the top of a raw thread, where
// the runtime reports it ("Terminating app due to uncaught exception") and
// aborts, instead of being stopped at the first Rust frame with no reason.
#[link(name = "objc", kind = "dylib")]
unsafe extern "C-unwind" {
    fn objc_msgSend();
}

unsafe extern "C" {
    fn pthread_create(
        thread: *mut usize,
        attributes: *const c_void,
        start: unsafe extern "C-unwind" fn(*mut c_void) -> *mut c_void,
        argument: *mut c_void,
    ) -> i32;
    fn pthread_join(thread: usize, value: *mut *mut c_void) -> i32;
}

// Linked so that the AppKit classes exist in the process; they are looked up
// by name at run time.
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSDefaultRunLoopMode: Id;
}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(max_displays: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
    fn CFRelease(object: *const c_void);
    static kCFRunLoopDefaultMode: *const c_void;
}

const NS_WINDOW_STYLE_MASK_TITLED: u64 = 1;
const NS_BACKING_STORE_BUFFERED: u64 = 2;
const NS_APPLICATION_ACTIVATION_POLICY_PROHIBITED: i64 = 2;
const NS_EVENT_MASK_ANY: u64 = u64::MAX;

pub const PROBES: &[ProbeSpec] = &[
    ProbeSpec {
        name: "display_present",
        question: "Does the process have a window server session and an active display?",
        needs_display: false,
    },
    ProbeSpec {
        name: "nsapplication_shared_main",
        question: "Can the main thread create the shared NSApplication (with the prohibited activation policy)?",
        needs_display: false,
    },
    ProbeSpec {
        name: "nsapplication_shared_off_main",
        question: "Can a worker thread create the shared NSApplication?",
        needs_display: false,
    },
    ProbeSpec {
        name: "nswindow_create_main",
        question: "Can the main thread create an NSWindow once the application exists?",
        needs_display: false,
    },
    ProbeSpec {
        name: "nswindow_create_off_main",
        question: "Can a worker thread create an NSWindow once the application exists on the main thread?",
        needs_display: false,
    },
    ProbeSpec {
        name: "nsevent_next_main",
        question: "Does polling the application's next event return on the main thread?",
        needs_display: false,
    },
    ProbeSpec {
        name: "nsevent_next_off_main",
        question: "Does polling the application's next event return on a worker thread?",
        needs_display: false,
    },
    ProbeSpec {
        name: "runloop_off_main",
        question: "Can a worker thread run its own Core Foundation run loop?",
        needs_display: false,
    },
    ProbeSpec {
        name: "metal_device_off_main",
        question: "Can a worker thread create the default Metal device, and can the main thread release it?",
        needs_display: false,
    },
];

pub struct Session;

impl Session {
    pub fn open() -> Self {
        Session
    }

    pub fn facts(&self) -> Vec<(String, String)> {
        ["SSH_CONNECTION", "TERM_SESSION_ID", "XPC_SERVICE_NAME", "USER"]
            .iter()
            .map(|name| {
                (
                    name.to_string(),
                    std::env::var(name).unwrap_or_else(|_| "(unset)".to_string()),
                )
            })
            .collect()
    }

    pub fn environment_for(&self, _spec: &ProbeSpec) -> Vec<(String, String)> {
        Vec::new()
    }
}

pub fn run(name: &str) -> Report {
    match name {
        "display_present" => display_present(),
        "nsapplication_shared_main" => nsapplication_shared(false),
        "nsapplication_shared_off_main" => nsapplication_shared(true),
        "nswindow_create_main" => nswindow_create(false),
        "nswindow_create_off_main" => nswindow_create(true),
        "nsevent_next_main" => nsevent_next(false),
        "nsevent_next_off_main" => nsevent_next(true),
        "runloop_off_main" => runloop_off_main(),
        "metal_device_off_main" => metal_device_off_main(),
        other => Report::new("unsupported").detail("unknown_probe", other),
    }
}

fn selector(name: &str) -> Sel {
    let name = CString::new(name).expect("selector names have no interior nul");
    unsafe { sel_registerName(name.as_ptr()) }
}

fn class(name: &str) -> Id {
    let name = CString::new(name).expect("class names have no interior nul");
    unsafe { objc_getClass(name.as_ptr()) }
}

/// `[receiver selector]` returning an object pointer.
unsafe fn send(receiver: Id, name: &str) -> Id {
    let call: unsafe extern "C-unwind" fn(Id, Sel) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    unsafe { call(receiver, selector(name)) }
}

/// `[receiver selector]` returning nothing.
unsafe fn send_void(receiver: Id, name: &str) {
    let call: unsafe extern "C-unwind" fn(Id, Sel) =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    unsafe { call(receiver, selector(name)) }
}

/// Runs the body on a raw thread whose entry has the unwinding ABI, so that an
/// Objective-C exception escaping the body reaches the top of the thread and
/// is reported by the runtime before the process aborts; the parent records
/// the report's absence and the standard error text as the fact.
fn on_raw_thread(body: Box<dyn FnOnce() -> Report + Send>) -> Report {
    struct Job {
        body: Option<Box<dyn FnOnce() -> Report + Send>>,
        result: Option<Report>,
    }
    unsafe extern "C-unwind" fn start(argument: *mut c_void) -> *mut c_void {
        let job = unsafe { &mut *(argument as *mut Job) };
        if let Some(body) = job.body.take() {
            job.result = Some(body());
        }
        ptr::null_mut()
    }
    let mut job = Job {
        body: Some(body),
        result: None,
    };
    let mut thread = 0usize;
    let created = unsafe { pthread_create(&mut thread, ptr::null(), start, (&mut job as *mut Job).cast()) };
    if created != 0 {
        return Report::new("failed").detail("pthread_create", created);
    }
    unsafe { pthread_join(thread, ptr::null_mut()) };
    job.result.unwrap_or_else(|| Report::new("failed").detail("thread", "ended without a report"))
}

struct AutoreleasePool(Id);

impl AutoreleasePool {
    fn new() -> Self {
        AutoreleasePool(unsafe { send(class("NSAutoreleasePool"), "new") })
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { send_void(self.0, "drain") };
        }
    }
}

fn is_main_thread() -> bool {
    let call: unsafe extern "C-unwind" fn(Id, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    unsafe { call(class("NSThread"), selector("isMainThread")) != 0 }
}

/// Creates the shared application with the prohibited activation policy (no
/// dock icon, no menu bar) and returns it.
unsafe fn shared_application() -> Result<Id, &'static str> {
    let application_class = class("NSApplication");
    if application_class.is_null() {
        return Err("NSApplication class not found");
    }
    let application = unsafe { send(application_class, "sharedApplication") };
    if application.is_null() {
        return Err("sharedApplication returned nil");
    }
    let set_policy: unsafe extern "C-unwind" fn(Id, Sel, i64) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    unsafe {
        set_policy(
            application,
            selector("setActivationPolicy:"),
            NS_APPLICATION_ACTIVATION_POLICY_PROHIBITED,
        )
    };
    Ok(application)
}

unsafe fn create_window() -> Result<Id, &'static str> {
    let window_class = class("NSWindow");
    if window_class.is_null() {
        return Err("NSWindow class not found");
    }
    let window = unsafe { send(window_class, "alloc") };
    if window.is_null() {
        return Err("alloc returned nil");
    }
    let init: unsafe extern "C-unwind" fn(Id, Sel, NsRect, u64, u64, i8) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    let window = unsafe {
        init(
            window,
            selector("initWithContentRect:styleMask:backing:defer:"),
            NsRect {
                x: 0.0,
                y: 0.0,
                width: 320.0,
                height: 240.0,
            },
            NS_WINDOW_STYLE_MASK_TITLED,
            NS_BACKING_STORE_BUFFERED,
            1,
        )
    };
    if window.is_null() { Err("init returned nil") } else { Ok(window) }
}

fn display_present() -> Report {
    let mut count = 0u32;
    let status = unsafe { CGGetActiveDisplayList(0, ptr::null_mut(), &mut count) };
    let session = unsafe { CGSessionCopyCurrentDictionary() };
    let has_session = !session.is_null();
    if has_session {
        unsafe { CFRelease(session) };
    }
    Report::new(if has_session || count > 0 { "succeeded" } else { "unavailable" })
        .detail("active_display_count", count)
        .detail("display_list_status", status)
        .detail("window_server_session", has_session)
        .detail("main_thread", is_main_thread())
}

fn nsapplication_shared(off_main: bool) -> Report {
    let body = move || {
        let _pool = AutoreleasePool::new();
        let on_main = is_main_thread();
        match unsafe { shared_application() } {
            Ok(_) => Report::new("succeeded").detail("created_on_main_thread", on_main),
            Err(reason) => Report::new("failed")
                .detail("created_on_main_thread", on_main)
                .detail("reason", reason),
        }
    };
    if off_main { on_raw_thread(Box::new(body)) } else { body() }
}

fn nswindow_create(off_main: bool) -> Report {
    let _pool = AutoreleasePool::new();
    if let Err(reason) = unsafe { shared_application() } {
        return Report::new("unavailable").detail("application", reason);
    }
    let body = move || {
        let _pool = AutoreleasePool::new();
        let on_main = is_main_thread();
        match unsafe { create_window() } {
            Ok(window) => {
                unsafe { send_void(window, "release") };
                Report::new("succeeded").detail("created_on_main_thread", on_main)
            }
            Err(reason) => Report::new("failed")
                .detail("created_on_main_thread", on_main)
                .detail("reason", reason),
        }
    };
    if off_main { on_raw_thread(Box::new(body)) } else { body() }
}

fn nsevent_next(off_main: bool) -> Report {
    let _pool = AutoreleasePool::new();
    let application = match unsafe { shared_application() } {
        Ok(application) => application as usize,
        Err(reason) => return Report::new("unavailable").detail("application", reason),
    };
    let body = move || {
        let _pool = AutoreleasePool::new();
        let on_main = is_main_thread();
        let next_event: unsafe extern "C-unwind" fn(Id, Sel, u64, Id, Id, i8) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
        let event = unsafe {
            next_event(
                application as Id,
                selector("nextEventMatchingMask:untilDate:inMode:dequeue:"),
                NS_EVENT_MASK_ANY,
                ptr::null_mut(),
                NSDefaultRunLoopMode,
                1,
            )
        };
        Report::new("succeeded")
            .detail("polled_on_main_thread", on_main)
            .detail("event_returned", !event.is_null())
    };
    if off_main { on_raw_thread(Box::new(body)) } else { body() }
}

fn runloop_off_main() -> Report {
    let result = thread::spawn(|| {
        let on_main = is_main_thread();
        let code = unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 1) };
        (on_main, code)
    })
    .join()
    .expect("the worker thread does not panic");
    // 1: finished (no sources), 2: stopped, 3: timed out, 4: handled a source.
    Report::new(if (1..=4).contains(&result.1) { "succeeded" } else { "failed" })
        .detail("run_on_main_thread", result.0)
        .detail("run_loop_result", result.1)
}

fn metal_device_off_main() -> Report {
    let (sender, receiver) = mpsc::channel::<(usize, String, bool)>();
    let worker = thread::spawn(move || {
        let _pool = AutoreleasePool::new();
        let device = unsafe { MTLCreateSystemDefaultDevice() };
        let name = if device.is_null() {
            String::new()
        } else {
            let name = unsafe { send(device, "name") };
            if name.is_null() {
                String::new()
            } else {
                let utf8: unsafe extern "C-unwind" fn(Id, Sel) -> *const c_char =
                    unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
                let text = unsafe { utf8(name, selector("UTF8String")) };
                if text.is_null() {
                    String::new()
                } else {
                    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
                }
            }
        };
        let _ = sender.send((device as usize, name, is_main_thread()));
    });
    let (device, name, worker_on_main) = receiver.recv().expect("the worker reports");
    worker.join().expect("the worker thread does not panic");
    if device == 0 {
        return Report::new("unavailable").detail("device", "MTLCreateSystemDefaultDevice returned nil");
    }
    unsafe { send_void(device as Id, "release") };
    Report::new("succeeded")
        .detail("device_name", name)
        .detail("created_on_main_thread", worker_on_main)
        .detail("released_on_main_thread", is_main_thread())
}
