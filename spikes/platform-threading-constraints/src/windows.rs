//! Windows probes: the thread affinity of Win32 windows and message queues,
//! and Direct3D 11 device creation and release across threads.

use crate::{ProbeSpec, Report};
use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

type Hwnd = *mut c_void;
type Handle = *mut c_void;
type WindowProcedure = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WindowClass {
    style: u32,
    procedure: Option<WindowProcedure>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Message {
    hwnd: Hwnd,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    point: Point,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WindowClass) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        extended_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        parameter: *const c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, message: u32, w_param: usize, l_param: isize) -> isize;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn PostMessageW(hwnd: Hwnd, message: u32, w_param: usize, l_param: isize) -> i32;
    fn PostThreadMessageW(thread: u32, message: u32, w_param: usize, l_param: isize) -> i32;
    fn PeekMessageW(message: *mut Message, hwnd: Hwnd, filter_min: u32, filter_max: u32, remove: u32) -> i32;
    fn GetMessageW(message: *mut Message, hwnd: Hwnd, filter_min: u32, filter_max: u32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: Hwnd, process: *mut u32) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[link(name = "d3d11")]
unsafe extern "system" {
    #[allow(clippy::too_many_arguments)]
    fn D3D11CreateDevice(
        adapter: *mut c_void,
        driver_type: i32,
        software: Handle,
        flags: u32,
        feature_levels: *const i32,
        feature_level_count: u32,
        sdk_version: u32,
        device: *mut *mut c_void,
        feature_level: *mut i32,
        context: *mut *mut c_void,
    ) -> i32;
}

const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const CW_USEDEFAULT: i32 = i32::MIN;
const HWND_MESSAGE: isize = -3;
const WM_QUIT: u32 = 0x0012;
const WM_APP: u32 = 0x8000;
const PM_REMOVE: u32 = 1;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const SM_REMOTESESSION: i32 = 0x1000;
const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
const D3D_DRIVER_TYPE_HARDWARE: i32 = 1;
const D3D_DRIVER_TYPE_WARP: i32 = 5;
const D3D11_SDK_VERSION: u32 = 7;
const DXGI_ERROR_NOT_FOUND: u32 = 0x887a_0002;
const DXGI_ERROR_UNSUPPORTED: u32 = 0x887a_0004;

pub const PROBES: &[ProbeSpec] = &[
    ProbeSpec {
        name: "display_present",
        question: "Does the session have a screen, and is it a remote session?",
        needs_display: false,
    },
    ProbeSpec {
        name: "window_create_off_main",
        question: "Can a worker thread create an ordinary top-level window?",
        needs_display: false,
    },
    ProbeSpec {
        name: "message_only_window_off_main",
        question: "Can a worker thread create a message-only window?",
        needs_display: false,
    },
    ProbeSpec {
        name: "destroy_window_from_other_thread",
        question: "Can a thread destroy a window that another thread created?",
        needs_display: false,
    },
    ProbeSpec {
        name: "post_and_peek_across_threads",
        question: "Can a thread post a message to another thread's window, and can it read that window's queue?",
        needs_display: false,
    },
    ProbeSpec {
        name: "message_loop_off_main",
        question: "Can a worker thread run the message loop of its own window and be stopped by another thread?",
        needs_display: false,
    },
    ProbeSpec {
        name: "d3d11_warp_device_off_main",
        question: "Can a worker thread create a Direct3D 11 WARP (software) device, and can another thread release it?",
        needs_display: false,
    },
    ProbeSpec {
        name: "d3d11_hardware_device_off_main",
        question: "Does a hardware Direct3D 11 adapter exist on this machine, created from a worker thread?",
        needs_display: false,
    },
];

pub struct Session;

impl Session {
    pub fn open() -> Self {
        Session
    }

    pub fn facts(&self) -> Vec<(String, String)> {
        ["SESSIONNAME", "USERNAME"]
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
        "window_create_off_main" => window_off_main(ptr::null_mut(), WS_OVERLAPPEDWINDOW),
        "message_only_window_off_main" => window_off_main(HWND_MESSAGE as Hwnd, 0),
        "destroy_window_from_other_thread" => destroy_window_from_other_thread(),
        "post_and_peek_across_threads" => post_and_peek_across_threads(),
        "message_loop_off_main" => message_loop_off_main(),
        "d3d11_warp_device_off_main" => d3d11_device(D3D_DRIVER_TYPE_WARP),
        "d3d11_hardware_device_off_main" => d3d11_device(D3D_DRIVER_TYPE_HARDWARE),
        other => Report::new("unsupported").detail("unknown_probe", other),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn window_procedure(hwnd: Hwnd, message: u32, w_param: usize, l_param: isize) -> isize {
    unsafe { DefWindowProcW(hwnd, message, w_param, l_param) }
}

/// Registers the probe's window class (once per process) and creates a window
/// on the calling thread. Returns the window or the last error.
unsafe fn create_window(parent: Hwnd, style: u32) -> Result<Hwnd, u32> {
    let class_name = wide("LudeneaThreadingProbe");
    let title = wide("Ludenea threading probe");
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let class = WindowClass {
            style: 0,
            procedure: Some(window_procedure),
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: ptr::null_mut(),
            cursor: ptr::null_mut(),
            background: ptr::null_mut(),
            menu_name: ptr::null(),
            class_name: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            let error = GetLastError();
            if error != ERROR_CLASS_ALREADY_EXISTS {
                return Err(error);
            }
        }
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            320,
            240,
            parent,
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() { Err(GetLastError()) } else { Ok(hwnd) }
    }
}

fn thread_id() -> u32 {
    unsafe { GetCurrentThreadId() }
}

fn display_present() -> Report {
    let (width, height, remote) = unsafe {
        (
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
            GetSystemMetrics(SM_REMOTESESSION),
        )
    };
    Report::new(if width > 0 && height > 0 { "succeeded" } else { "unavailable" })
        .detail("screen_width", width)
        .detail("screen_height", height)
        .detail("remote_session", remote != 0)
        .detail("main_thread_id", thread_id())
}

fn window_off_main(parent: Hwnd, style: u32) -> Report {
    let parent = parent as usize;
    let main_thread = thread_id();
    let worker = thread::spawn(move || unsafe {
        let created = create_window(parent as Hwnd, style);
        match created {
            Ok(hwnd) => {
                let mut owner_process = 0u32;
                let owner_thread = GetWindowThreadProcessId(hwnd, &mut owner_process);
                let destroyed = DestroyWindow(hwnd);
                Ok((thread_id(), owner_thread, destroyed))
            }
            Err(error) => Err((thread_id(), error)),
        }
    });
    match worker.join().expect("the worker thread does not panic") {
        Ok((worker_thread, owner_thread, destroyed)) => Report::new("succeeded")
            .detail("main_thread_id", main_thread)
            .detail("created_on_thread", worker_thread)
            .detail("window_owner_thread", owner_thread)
            .detail("destroyed_by_creator_returned", destroyed),
        Err((worker_thread, error)) => Report::new("failed")
            .detail("main_thread_id", main_thread)
            .detail("created_on_thread", worker_thread)
            .detail("last_error", error),
    }
}

fn destroy_window_from_other_thread() -> Report {
    let (to_main, from_worker) = mpsc::channel::<Result<usize, u32>>();
    let (to_worker, from_main) = mpsc::channel::<()>();
    let worker = thread::spawn(move || unsafe {
        match create_window(HWND_MESSAGE as Hwnd, 0) {
            Ok(hwnd) => {
                let _ = to_main.send(Ok(hwnd as usize));
                let _ = from_main.recv();
                DestroyWindow(hwnd)
            }
            Err(error) => {
                let _ = to_main.send(Err(error));
                -1
            }
        }
    });
    let report = match from_worker.recv() {
        Ok(Ok(hwnd)) => {
            let destroyed = unsafe { DestroyWindow(hwnd as Hwnd) };
            let error = if destroyed == 0 { unsafe { GetLastError() } } else { 0 };
            let _ = to_worker.send(());
            let by_creator = worker.join().expect("the worker thread does not panic");
            Report::new(if destroyed != 0 { "succeeded" } else { "failed" })
                .detail("destroy_from_other_thread_returned", destroyed)
                .detail("last_error", error)
                .detail("destroy_by_creating_thread_returned", by_creator)
        }
        Ok(Err(error)) => {
            let _ = worker.join();
            Report::new("unavailable").detail("window_creation_error", error)
        }
        Err(_) => Report::new("unavailable").detail("worker", "ended without a report"),
    };
    report
}

fn post_and_peek_across_threads() -> Report {
    let (to_main, from_worker) = mpsc::channel::<Result<(usize, u32), u32>>();
    let (to_worker, from_main) = mpsc::channel::<()>();
    let worker = thread::spawn(move || unsafe {
        match create_window(HWND_MESSAGE as Hwnd, 0) {
            Ok(hwnd) => {
                let _ = to_main.send(Ok((hwnd as usize, thread_id())));
                let _ = from_main.recv();
                let mut message: Message = std::mem::zeroed();
                let found = PeekMessageW(&mut message, hwnd, 0, 0, PM_REMOVE);
                let seen = message.message;
                DestroyWindow(hwnd);
                (found, seen)
            }
            Err(error) => {
                let _ = to_main.send(Err(error));
                (-1, 0)
            }
        }
    });
    match from_worker.recv() {
        Ok(Ok((hwnd, worker_thread))) => {
            let hwnd = hwnd as Hwnd;
            let (posted, peeked_from_other_thread, owner_thread) = unsafe {
                let posted = PostMessageW(hwnd, WM_APP, 1, 2);
                let mut message: Message = std::mem::zeroed();
                let peeked = PeekMessageW(&mut message, hwnd, 0, 0, PM_REMOVE);
                let mut process = 0u32;
                (posted, peeked, GetWindowThreadProcessId(hwnd, &mut process))
            };
            let _ = to_worker.send(());
            let (found_by_creator, seen) = worker.join().expect("the worker thread does not panic");
            let outcome = if posted != 0 && peeked_from_other_thread == 0 && found_by_creator != 0 && seen == WM_APP {
                "succeeded"
            } else {
                "failed"
            };
            Report::new(outcome)
                .detail("post_from_other_thread_returned", posted)
                .detail("peek_from_other_thread_found", peeked_from_other_thread)
                .detail("peek_by_creating_thread_found", found_by_creator)
                .detail("message_seen_by_creator", format!("0x{seen:04x}"))
                .detail("window_owner_thread", owner_thread)
                .detail("creating_thread", worker_thread)
                .detail("main_thread_id", thread_id())
        }
        Ok(Err(error)) => {
            let _ = worker.join();
            Report::new("unavailable").detail("window_creation_error", error)
        }
        Err(_) => Report::new("unavailable").detail("worker", "ended without a report"),
    }
}

fn message_loop_off_main() -> Report {
    let (to_main, from_worker) = mpsc::channel::<Result<u32, u32>>();
    let worker = thread::spawn(move || unsafe {
        match create_window(HWND_MESSAGE as Hwnd, 0) {
            Ok(hwnd) => {
                let _ = to_main.send(Ok(thread_id()));
                let mut messages = 0u32;
                let mut message: Message = std::mem::zeroed();
                let mut last = 1;
                while last > 0 {
                    last = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
                    messages += 1;
                }
                DestroyWindow(hwnd);
                Ok((messages, last, message.message))
            }
            Err(error) => {
                let _ = to_main.send(Err(error));
                Err(error)
            }
        }
    });
    match from_worker.recv() {
        Ok(Ok(worker_thread)) => {
            thread::sleep(Duration::from_millis(50));
            let posted = unsafe { PostThreadMessageW(worker_thread, WM_QUIT, 0, 0) };
            let error = if posted == 0 { unsafe { GetLastError() } } else { 0 };
            if posted == 0 {
                // The loop would never end: the parent's timeout records it.
                return Report::new("failed")
                    .detail("post_thread_message_returned", posted)
                    .detail("last_error", error);
            }
            match worker.join().expect("the worker thread does not panic") {
                Ok((messages, last, final_message)) => Report::new("succeeded")
                    .detail("loop_thread", worker_thread)
                    .detail("stopped_from_thread", thread_id())
                    .detail("messages_received", messages)
                    .detail("get_message_final_return", last)
                    .detail("final_message", format!("0x{final_message:04x}")),
                Err(error) => Report::new("failed").detail("window_creation_error", error),
            }
        }
        Ok(Err(error)) => Report::new("unavailable").detail("window_creation_error", error),
        Err(_) => Report::new("unavailable").detail("worker", "ended without a report"),
    }
}

/// Calls `IUnknown::Release` through the object's vtable (third slot).
unsafe fn release(object: usize) -> u32 {
    if object == 0 {
        return 0;
    }
    unsafe {
        let object = object as *mut c_void;
        let vtable = *(object as *const *const usize);
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtable.add(2));
        release(object)
    }
}

fn d3d11_device(driver_type: i32) -> Report {
    let worker = thread::spawn(move || unsafe {
        let mut device: *mut c_void = ptr::null_mut();
        let mut context: *mut c_void = ptr::null_mut();
        let mut feature_level = 0i32;
        let result = D3D11CreateDevice(
            ptr::null_mut(),
            driver_type,
            ptr::null_mut(),
            0,
            ptr::null(),
            0,
            D3D11_SDK_VERSION,
            &mut device,
            &mut feature_level,
            &mut context,
        );
        (result, feature_level, device as usize, context as usize, thread_id())
    });
    let (result, feature_level, device, context, worker_thread) =
        worker.join().expect("the worker thread does not panic");
    let hresult = format!("0x{:08x}", result as u32);
    if result < 0 {
        let outcome = if result as u32 == DXGI_ERROR_UNSUPPORTED || result as u32 == DXGI_ERROR_NOT_FOUND {
            "unavailable"
        } else {
            "failed"
        };
        return Report::new(outcome)
            .detail("hresult", hresult)
            .detail("driver_type", driver_type)
            .detail("created_on_thread", worker_thread);
    }
    let (context_references, device_references) = unsafe { (release(context), release(device)) };
    Report::new("succeeded")
        .detail("hresult", hresult)
        .detail("driver_type", driver_type)
        .detail("feature_level", format!("0x{feature_level:04x}"))
        .detail("created_on_thread", worker_thread)
        .detail("released_on_thread", thread_id())
        .detail("context_references_after_release", context_references)
        .detail("device_references_after_release", device_references)
}
