//! The device layer probe: what a windowing library's event loop and a gamepad
//! library do when they start on this machine, and the threads each creates.
//! Runs in a child process, so that a crash inside a platform library is a
//! recorded fact. Prints one JSON object on standard output.

use std::thread::sleep;
use std::time::Duration;

use reference_workload::Json;

/// The number of threads of this process, as the operating system counts them.
pub fn thread_count() -> i64 {
    platform::thread_count()
}

#[cfg(target_os = "linux")]
mod platform {
    pub fn thread_count() -> i64 {
        std::fs::read_dir("/proc/self/task").map_or(-1, |entries| entries.count() as i64)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    #[repr(C)]
    struct ThreadEntry {
        size: u32,
        usage: u32,
        thread_id: u32,
        owner_process_id: u32,
        base_priority: i32,
        delta_priority: i32,
        flags: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
        fn Thread32First(snapshot: isize, entry: *mut ThreadEntry) -> i32;
        fn Thread32Next(snapshot: isize, entry: *mut ThreadEntry) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetCurrentProcessId() -> u32;
    }

    const SNAPSHOT_THREADS: u32 = 0x0000_0004;

    pub fn thread_count() -> i64 {
        // SAFETY: the snapshot handle is checked, the entry carries its own
        // size as the interface asks, and the handle is closed.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(SNAPSHOT_THREADS, 0);
            if snapshot == -1 {
                return -1;
            }
            let process = GetCurrentProcessId();
            let mut entry = ThreadEntry { size: size_of::<ThreadEntry>() as u32, usage: 0, thread_id: 0, owner_process_id: 0, base_priority: 0, delta_priority: 0, flags: 0 };
            let mut count = 0;
            let mut more = Thread32First(snapshot, &mut entry);
            while more != 0 {
                if entry.owner_process_id == process {
                    count += 1;
                }
                more = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            count
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, threads: *mut *mut u32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
    }

    pub fn thread_count() -> i64 {
        // SAFETY: the list the kernel returns is read for `count` entries,
        // every port in it is released and the list itself is deallocated.
        unsafe {
            let task = mach_task_self_;
            let mut threads: *mut u32 = std::ptr::null_mut();
            let mut count: u32 = 0;
            if task_threads(task, &mut threads, &mut count) != 0 {
                return -1;
            }
            for index in 0..count as usize {
                mach_port_deallocate(task, *threads.add(index));
            }
            vm_deallocate(task, threads as usize, count as usize * size_of::<u32>());
            i64::from(count)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod platform {
    pub fn thread_count() -> i64 {
        -1
    }
}

const SETTLE: Duration = Duration::from_millis(300);

pub fn run() -> String {
    let mut out = Json::new();
    out.object_start();
    out.bool("display_variable_set", std::env::var_os("DISPLAY").is_some());
    out.bool("wayland_display_variable_set", std::env::var_os("WAYLAND_DISPLAY").is_some());
    out.i64("threads_at_start", thread_count());

    // The gamepad library first: it needs no display.
    out.key("gamepad_library");
    out.object_start();
    out.string("library", "gilrs");
    let before = thread_count();
    match gilrs::Gilrs::new() {
        Ok(mut gilrs) => {
            sleep(SETTLE);
            out.string("outcome", "started");
            out.u64("gamepads_listed", gilrs.gamepads().count() as u64);
            let mut events = 0u64;
            while gilrs.next_event().is_some() {
                events += 1;
            }
            out.u64("events_waiting", events);
            out.i64("threads_before", before);
            out.i64("threads_while_running", thread_count());
            drop(gilrs);
            sleep(SETTLE);
            out.i64("threads_after_the_drop", thread_count());
        }
        Err(error) => {
            out.string("outcome", "failed");
            out.string("error", &error.to_string());
            out.i64("threads_before", before);
            out.i64("threads_after_the_failure", thread_count());
        }
    }
    out.object_end();

    // The windowing library's event loop, created on this process's main
    // thread and dropped without a window.
    out.key("windowing_library");
    out.object_start();
    out.string("library", "winit");
    let before = thread_count();
    match winit::event_loop::EventLoop::new() {
        Ok(event_loop) => {
            sleep(SETTLE);
            out.string("outcome", "event loop created");
            out.i64("threads_before", before);
            out.i64("threads_while_it_exists", thread_count());
            drop(event_loop);
            sleep(SETTLE);
            out.i64("threads_after_the_drop", thread_count());
        }
        Err(error) => {
            out.string("outcome", "failed");
            out.string("error", &error.to_string());
            out.i64("threads_before", before);
            out.i64("threads_after_the_failure", thread_count());
        }
    }
    out.object_end();
    out.i64("threads_at_end", thread_count());
    out.object_end();
    out.finish()
}
