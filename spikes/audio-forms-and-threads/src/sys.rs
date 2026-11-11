//! What the operating system says about this process: its threads, the
//! modules it has loaded, and the scheduling of the calling thread. No
//! third-party crate, so that the probe adds nothing to what it measures.

/// The words that mark a loaded module as part of an audio stack.
const AUDIO_WORDS: [&str; 15] = [
    "asound", "pulse", "pipewire", "jack", "audio", "mmdevapi", "avrt", "dsound", "winmm", "ksuser", "wdmaud", "coreaudio", "audiotoolbox", "audiounit", "sndio",
];

fn is_audio(module: &str) -> bool {
    let lower = module.to_lowercase();
    let name = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    // The spike's own executable carries "audio" in its name.
    !name.starts_with("audio-forms-and-threads") && !name.starts_with("audio_forms_and_threads") && AUDIO_WORDS.iter().any(|word| name.contains(word))
}

/// The loaded modules whose name marks them as audio, sorted, each once.
pub fn loaded_audio_modules() -> Vec<String> {
    let mut modules: Vec<String> = platform::loaded_modules().into_iter().filter(|module| is_audio(module)).collect();
    modules.sort();
    modules.dedup();
    modules
}

pub fn loaded_module_count() -> usize {
    let mut modules = platform::loaded_modules();
    modules.sort();
    modules.dedup();
    modules.len()
}

pub fn thread_count() -> i64 {
    platform::thread_count()
}

/// The names of this process's threads where the platform gives them cheaply.
pub fn thread_names() -> Vec<String> {
    platform::thread_names()
}

/// The scheduling of the calling thread, as two numbers whose meaning the
/// platform fixes: (policy, priority) on Linux and macOS, (0, priority) on
/// Windows.
pub fn scheduling_of_this_thread() -> (i32, i32) {
    platform::scheduling_of_this_thread()
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;

    pub fn thread_count() -> i64 {
        fs::read_dir("/proc/self/task").map_or(-1, |entries| entries.count() as i64)
    }

    pub fn thread_names() -> Vec<String> {
        let Ok(entries) = fs::read_dir("/proc/self/task") else { return Vec::new() };
        let mut names: Vec<String> = entries.filter_map(|entry| fs::read_to_string(entry.ok()?.path().join("comm")).ok()).map(|name| name.trim().to_string()).collect();
        names.sort();
        names
    }

    pub fn loaded_modules() -> Vec<String> {
        let maps = fs::read_to_string("/proc/self/maps").unwrap_or_default();
        maps.lines().filter_map(|line| line.split_whitespace().nth(5)).filter(|path| path.starts_with('/')).map(str::to_string).collect()
    }

    #[repr(C)]
    struct SchedulingParameter {
        priority: i32,
    }

    unsafe extern "C" {
        fn sched_getscheduler(process: i32) -> i32;
        fn sched_getparam(process: i32, parameter: *mut SchedulingParameter) -> i32;
    }

    pub fn scheduling_of_this_thread() -> (i32, i32) {
        let mut parameter = SchedulingParameter { priority: -1 };
        // SAFETY: zero names the calling thread; the parameter is a valid
        // place for the one field the call writes.
        unsafe {
            let policy = sched_getscheduler(0);
            sched_getparam(0, &mut parameter);
            (policy, parameter.priority)
        }
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
        fn GetCurrentProcess() -> isize;
        fn GetCurrentThread() -> isize;
        fn GetThreadPriority(thread: isize) -> i32;
        fn K32EnumProcessModules(process: isize, modules: *mut isize, bytes: u32, needed: *mut u32) -> i32;
        fn K32GetModuleBaseNameW(process: isize, module: isize, name: *mut u16, length: u32) -> u32;
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

    pub fn thread_names() -> Vec<String> {
        Vec::new()
    }

    pub fn loaded_modules() -> Vec<String> {
        let mut handles = [0isize; 1024];
        let mut needed = 0u32;
        // SAFETY: the buffer's size in bytes is passed with it, and each name
        // is read into a buffer whose length is passed with it.
        unsafe {
            let process = GetCurrentProcess();
            if K32EnumProcessModules(process, handles.as_mut_ptr(), size_of_val(&handles) as u32, &mut needed) == 0 {
                return Vec::new();
            }
            let count = (needed as usize / size_of::<isize>()).min(handles.len());
            handles[..count]
                .iter()
                .map(|module| {
                    let mut name = [0u16; 260];
                    let length = K32GetModuleBaseNameW(process, *module, name.as_mut_ptr(), name.len() as u32);
                    String::from_utf16_lossy(&name[..length as usize])
                })
                .collect()
        }
    }

    pub fn scheduling_of_this_thread() -> (i32, i32) {
        // SAFETY: the pseudo handle of the calling thread is always valid.
        (0, unsafe { GetThreadPriority(GetCurrentThread()) })
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{CStr, c_char};

    #[repr(C)]
    struct SchedulingParameter {
        priority: i32,
        opaque: [u8; 4],
    }

    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, threads: *mut *mut u32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(index: u32) -> *const c_char;
        fn pthread_self() -> usize;
        fn pthread_getschedparam(thread: usize, policy: *mut i32, parameter: *mut SchedulingParameter) -> i32;
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

    pub fn thread_names() -> Vec<String> {
        Vec::new()
    }

    pub fn loaded_modules() -> Vec<String> {
        // SAFETY: an index below the count names an image whose name, when
        // not null, is a string that lives as long as the image is loaded.
        unsafe {
            (0.._dyld_image_count())
                .filter_map(|index| {
                    let name = _dyld_get_image_name(index);
                    (!name.is_null()).then(|| CStr::from_ptr(name).to_string_lossy().into_owned())
                })
                .collect()
        }
    }

    pub fn scheduling_of_this_thread() -> (i32, i32) {
        let mut policy = -1;
        let mut parameter = SchedulingParameter { priority: -1, opaque: [0; 4] };
        // SAFETY: both places are valid for the values the call writes.
        unsafe {
            pthread_getschedparam(pthread_self(), &mut policy, &mut parameter);
        }
        (policy, parameter.priority)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod platform {
    pub fn thread_count() -> i64 {
        -1
    }

    pub fn thread_names() -> Vec<String> {
        Vec::new()
    }

    pub fn loaded_modules() -> Vec<String> {
        Vec::new()
    }

    pub fn scheduling_of_this_thread() -> (i32, i32) {
        (-1, -1)
    }
}
