//! Two facts about the process the measurements need: how many threads it
//! holds, and a way to restrict it to a number of processors. Throwaway,
//! never product code.

/// The threads of this process, or nothing where the platform's list could
/// not be read.
#[cfg(target_os = "linux")]
pub fn thread_count() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| line.strip_prefix("Threads:")).and_then(|count| count.trim().parse().ok())
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct ThreadEntry {
        pub size: u32,
        pub usage: u32,
        pub thread_id: u32,
        pub owner_process_id: u32,
        pub base_priority: i32,
        pub delta_priority: i32,
        pub flags: u32,
    }

    pub const SNAPSHOT_THREADS: u32 = 0x4;
    pub const INVALID_HANDLE: isize = -1;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        pub fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        pub fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        pub fn CloseHandle(handle: *mut c_void) -> i32;
        pub fn GetCurrentProcessId() -> u32;
        pub fn GetCurrentProcess() -> *mut c_void;
        pub fn SetProcessAffinityMask(process: *mut c_void, mask: usize) -> i32;
    }
}

#[cfg(windows)]
pub fn thread_count() -> Option<u64> {
    // SAFETY: the snapshot is a handle this function owns and closes; the
    // entry is a plain structure whose size field the calls require.
    unsafe {
        let snapshot = sys::CreateToolhelp32Snapshot(sys::SNAPSHOT_THREADS, 0);
        if snapshot as isize == sys::INVALID_HANDLE {
            return None;
        }
        let process = sys::GetCurrentProcessId();
        let mut entry: sys::ThreadEntry = std::mem::zeroed();
        entry.size = std::mem::size_of::<sys::ThreadEntry>() as u32;
        let mut count = 0;
        let mut more = sys::Thread32First(snapshot, &mut entry);
        while more != 0 {
            if entry.owner_process_id == process {
                count += 1;
            }
            more = sys::Thread32Next(snapshot, &mut entry);
        }
        sys::CloseHandle(snapshot);
        Some(count)
    }
}

#[cfg(target_os = "macos")]
pub fn thread_count() -> Option<u64> {
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, threads: *mut *mut u32, count: *mut u32) -> i32;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
    }
    // SAFETY: the kernel fills the list and its length; the list is returned
    // to the kernel once counted.
    unsafe {
        let mut threads = std::ptr::null_mut();
        let mut count = 0;
        if task_threads(mach_task_self_, &mut threads, &mut count) != 0 {
            return None;
        }
        vm_deallocate(mach_task_self_, threads as usize, count as usize * std::mem::size_of::<u32>());
        Some(u64::from(count))
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub fn thread_count() -> Option<u64> {
    None
}

/// Restricts the process to its first `cpus` processors and says what
/// happened. Only Windows offers it here: the software device there has no
/// setting of its own for its thread count.
#[cfg(windows)]
pub fn limit_affinity(cpus: u32) -> String {
    let mask = if cpus >= usize::BITS { usize::MAX } else { (1_usize << cpus) - 1 };
    // SAFETY: the pseudo-handle of the current process needs no closing.
    let done = unsafe { sys::SetProcessAffinityMask(sys::GetCurrentProcess(), mask) };
    if done != 0 { format!("restricted to {cpus} processor(s)") } else { format!("the restriction to {cpus} processor(s) was refused") }
}

#[cfg(not(windows))]
pub fn limit_affinity(cpus: u32) -> String {
    format!("the restriction to {cpus} processor(s) is not tried on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_counts_its_threads_where_the_platform_lists_them() {
        let before = thread_count();
        if cfg!(any(target_os = "linux", windows, target_os = "macos")) {
            let before = before.expect("a thread count");
            assert!(before >= 1);
            let (ready, wait) = (std::sync::mpsc::channel::<()>(), std::sync::mpsc::channel::<()>());
            let handle = std::thread::spawn(move || {
                ready.0.send(()).unwrap();
                let _ = wait.1.recv();
            });
            ready.1.recv().unwrap();
            assert!(thread_count().expect("a thread count") >= 2, "the spawned thread is counted");
            drop(wait.0);
            handle.join().unwrap();
        }
    }
}
