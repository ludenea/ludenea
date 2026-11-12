//! The threads of the process, from the system's list, with the processor
//! time each has taken, and a ledger that attributes each thread to the step
//! at which it appeared. Throwaway, never product code.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::json::Json;

/// One thread of the process: its system identifier and the processor time it
/// has taken so far.
#[derive(Clone, Copy, Debug)]
pub struct ThreadSample {
    pub id: u64,
    pub cpu_ns: u64,
}

/// A set of threads is said to wait when it takes less than this of an idle
/// second.
pub const WAITING_BELOW_NS: u64 = 1_000_000;

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
    pub const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        pub fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        pub fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        pub fn CloseHandle(handle: *mut c_void) -> i32;
        pub fn GetCurrentProcessId() -> u32;
        pub fn GetCurrentThread() -> *mut c_void;
        pub fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> *mut c_void;
        pub fn QueryThreadCycleTime(thread: *mut c_void, cycles: *mut u64) -> i32;
    }
}

/// The cycles the thread counter advances by in one second of processor time,
/// measured once on the calling thread: Windows accounts a thread's time in
/// scheduler ticks, far coarser than the millisecond the spike judges, and in
/// cycles, which this converts.
#[cfg(windows)]
fn cycles_per_second() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static RATE: OnceLock<f64> = OnceLock::new();
    *RATE.get_or_init(|| {
        let mut best = 0.0_f64;
        for _ in 0..3 {
            let mut before = 0_u64;
            let mut after = 0_u64;
            // SAFETY: the pseudo handle of the calling thread is always valid.
            unsafe { sys::QueryThreadCycleTime(sys::GetCurrentThread(), &mut before) };
            let started = Instant::now();
            while started.elapsed() < Duration::from_millis(30) {
                std::hint::spin_loop();
            }
            let elapsed = started.elapsed().as_secs_f64();
            // SAFETY: as above.
            unsafe { sys::QueryThreadCycleTime(sys::GetCurrentThread(), &mut after) };
            // A preemption during the spin loses cycles, so the largest of
            // the three rates is the closest to an uninterrupted second.
            best = best.max(after.saturating_sub(before) as f64 / elapsed);
        }
        best.max(1.0)
    })
}

#[cfg(windows)]
pub fn sample() -> Result<Vec<ThreadSample>, String> {
    let rate = cycles_per_second();
    let mut samples = Vec::new();
    // SAFETY: the snapshot and every thread handle are owned and closed here;
    // the entry is a plain structure whose size field the calls require.
    unsafe {
        let snapshot = sys::CreateToolhelp32Snapshot(sys::SNAPSHOT_THREADS, 0);
        if snapshot as isize == sys::INVALID_HANDLE {
            return Err("CreateToolhelp32Snapshot failed".to_string());
        }
        let process = sys::GetCurrentProcessId();
        let mut entry: sys::ThreadEntry = std::mem::zeroed();
        entry.size = std::mem::size_of::<sys::ThreadEntry>() as u32;
        let mut more = sys::Thread32First(snapshot, &mut entry);
        while more != 0 {
            if entry.owner_process_id == process {
                let handle = sys::OpenThread(sys::THREAD_QUERY_LIMITED_INFORMATION, 0, entry.thread_id);
                let mut cycles = 0_u64;
                if !handle.is_null() {
                    sys::QueryThreadCycleTime(handle, &mut cycles);
                    sys::CloseHandle(handle);
                }
                samples.push(ThreadSample { id: u64::from(entry.thread_id), cpu_ns: (cycles as f64 / rate * 1e9) as u64 });
            }
            more = sys::Thread32Next(snapshot, &mut entry);
        }
        sys::CloseHandle(snapshot);
    }
    Ok(samples)
}

#[cfg(target_os = "linux")]
pub fn sample() -> Result<Vec<ThreadSample>, String> {
    let mut samples = Vec::new();
    let entries = std::fs::read_dir("/proc/self/task").map_err(|error| format!("/proc/self/task: {error}"))?;
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().and_then(|name| name.parse::<u64>().ok()) else { continue };
        // The first field of schedstat is the time spent on a processor, in
        // nanoseconds. A thread that ends between the listing and the read is
        // left out.
        let Ok(text) = std::fs::read_to_string(entry.path().join("schedstat")) else { continue };
        let cpu_ns = text.split_whitespace().next().and_then(|field| field.parse::<u64>().ok()).unwrap_or(0);
        samples.push(ThreadSample { id, cpu_ns });
    }
    Ok(samples)
}

#[cfg(target_os = "macos")]
pub fn sample() -> Result<Vec<ThreadSample>, String> {
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, threads: *mut *mut u32, count: *mut u32) -> i32;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
        fn thread_info(thread: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
    }
    const THREAD_BASIC_INFO: u32 = 3;
    const THREAD_IDENTIFIER_INFO: u32 = 4;
    let mut samples = Vec::new();
    // SAFETY: the kernel fills the list and its length; each port and the
    // list itself are returned to the kernel once read. The two flavors fill
    // ten and six 32-bit words.
    unsafe {
        let mut threads = std::ptr::null_mut();
        let mut count = 0_u32;
        if task_threads(mach_task_self_, &mut threads, &mut count) != 0 {
            return Err("task_threads failed".to_string());
        }
        for index in 0..count as usize {
            let thread = *threads.add(index);
            let mut basic = [0_i32; 10];
            let mut basic_count = 10_u32;
            let mut identifier = [0_i32; 6];
            let mut identifier_count = 6_u32;
            let basic_ok = thread_info(thread, THREAD_BASIC_INFO, basic.as_mut_ptr(), &mut basic_count) == 0;
            let identifier_ok = thread_info(thread, THREAD_IDENTIFIER_INFO, identifier.as_mut_ptr(), &mut identifier_count) == 0;
            if basic_ok && identifier_ok {
                let micros = |seconds: i32, microseconds: i32| seconds as u64 * 1_000_000 + microseconds as u64;
                let cpu_ns = (micros(basic[0], basic[1]) + micros(basic[2], basic[3])) * 1000;
                let id = (identifier[0] as u32 as u64) | ((identifier[1] as u32 as u64) << 32);
                samples.push(ThreadSample { id, cpu_ns });
            }
            mach_port_deallocate(mach_task_self_, thread);
        }
        vm_deallocate(mach_task_self_, threads as usize, count as usize * std::mem::size_of::<u32>());
    }
    Ok(samples)
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub fn sample() -> Result<Vec<ThreadSample>, String> {
    Err("no thread list on this platform".to_string())
}

/// The system identifier of the calling thread, in the unit of `sample`.
#[cfg(windows)]
fn current_thread() -> u64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    // SAFETY: the call reads the identifier of the calling thread.
    u64::from(unsafe { GetCurrentThreadId() })
}

#[cfg(target_os = "linux")]
fn current_thread() -> u64 {
    std::fs::read_link("/proc/thread-self").ok().and_then(|path| path.file_name().and_then(|name| name.to_str()).and_then(|name| name.parse().ok())).unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn current_thread() -> u64 {
    unsafe extern "C" {
        fn pthread_threadid_np(thread: *mut std::ffi::c_void, id: *mut u64) -> i32;
    }
    let mut id = 0_u64;
    // SAFETY: a null thread names the calling thread; the call writes its identifier.
    unsafe { pthread_threadid_np(std::ptr::null_mut(), &mut id) };
    id
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn current_thread() -> u64 {
    0
}

/// The threads of the process, each attributed to the step at which it was
/// first seen.
pub struct Ledger {
    owner: BTreeMap<u64, String>,
    steps: Vec<Json>,
}

impl Ledger {
    /// Starts the ledger: every thread that exists now belongs to `label`.
    pub fn start(label: &str) -> Ledger {
        let mut ledger = Ledger { owner: BTreeMap::new(), steps: Vec::new() };
        ledger.step(label, false);
        ledger
    }

    /// Records a step: the threads that appeared since the last one belong to
    /// `label`. With `idle_second`, the calling thread then sleeps one second
    /// and the processor time each set of threads took meanwhile is recorded.
    pub fn step(&mut self, label: &str, idle_second: bool) {
        let mut step = Json::object().with("step", label);
        let samples = match sample() {
            Ok(samples) => samples,
            Err(error) => {
                self.steps.push(step.with("error", error));
                return;
            }
        };
        let mut appeared = 0_u64;
        for thread in &samples {
            if !self.owner.contains_key(&thread.id) {
                self.owner.insert(thread.id, label.to_string());
                appeared += 1;
            }
        }
        let mut ended: BTreeMap<String, u64> = BTreeMap::new();
        let gone: Vec<u64> = self.owner.keys().copied().filter(|id| !samples.iter().any(|thread| thread.id == *id)).collect();
        for id in gone {
            if let Some(owner) = self.owner.remove(&id) {
                *ended.entry(owner).or_default() += 1;
            }
        }
        step.set("threads", samples.len());
        step.set("appeared", appeared);
        step.set("ended", Json::List(ended.into_iter().map(|(set, count)| Json::object().with("set", set).with("threads", count)).collect()));
        if idle_second {
            std::thread::sleep(Duration::from_secs(1));
            match sample() {
                Ok(after) => step.set("idle_second", self.idle_sets(&samples, &after)),
                Err(error) => step.set("idle_second_error", error),
            }
        }
        self.steps.push(step);
    }

    /// Per set of threads, the processor time taken between two samples.
    fn idle_sets(&self, before: &[ThreadSample], after: &[ThreadSample]) -> Json {
        // Per set: threads, total nanoseconds, busiest thread's nanoseconds.
        let mut sets: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        // The thread that takes the samples is left out: listing the threads
        // of the system is its own work, not a wait.
        let measuring = current_thread();
        for thread in after {
            if thread.id == measuring {
                continue;
            }
            let Some(earlier) = before.iter().find(|earlier| earlier.id == thread.id) else { continue };
            let Some(owner) = self.owner.get(&thread.id) else { continue };
            let taken = thread.cpu_ns.saturating_sub(earlier.cpu_ns);
            let set = sets.entry(owner.clone()).or_default();
            set.0 += 1;
            set.1 += taken;
            set.2 = set.2.max(taken);
        }
        Json::List(
            sets.into_iter()
                .map(|(set, (threads, total, busiest))| {
                    Json::object()
                        .with("set", set)
                        .with("threads", threads)
                        .with("processor_us", Json::Float(total as f64 / 1000.0))
                        .with("busiest_thread_us", Json::Float(busiest as f64 / 1000.0))
                        .with("waits", total < WAITING_BELOW_NS)
                })
                .collect(),
        )
    }

    /// How many live threads each set holds now, by the ledger's attribution.
    pub fn live_sets(&self) -> Json {
        let mut sets: BTreeMap<&str, u64> = BTreeMap::new();
        for owner in self.owner.values() {
            *sets.entry(owner.as_str()).or_default() += 1;
        }
        Json::List(sets.into_iter().map(|(set, threads)| Json::object().with("set", set).with("threads", threads)).collect())
    }

    pub fn into_json(self) -> Json {
        Json::List(self.steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_started_after_the_ledger_belongs_to_the_next_step_and_waits_while_parked() {
        let mut ledger = Ledger::start("at start");
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = stopped.recv();
        });
        ledger.step("a parked thread", true);
        let live = ledger.live_sets().print();
        assert!(live.contains(r#"{"set":"a parked thread","threads":1}"#), "{live}");
        drop(stop);
        worker.join().unwrap();
        ledger.step("after the thread ended", false);
        let steps = ledger.into_json().print();
        assert!(steps.contains(r#""set":"a parked thread","threads":1,"processor_us":"#), "{steps}");
        assert!(steps.contains(r#""ended":[{"set":"a parked thread","threads":1}]"#), "{steps}");
        let parked = steps.split(r#""set":"a parked thread""#).nth(1).unwrap();
        assert!(parked.contains(r#""waits":true"#), "{steps}");
    }
}
