//! The two pool kinds behind one small interface. Throwaway, never product code.
//!
//! A job (the world's tick graph) exposes claimable tasks and tells, after
//! each task, what the executor should wake. An executor runs a job to
//! completion with the calling thread as worker 0 of the step:
//!
//! - the step-scoped team (the control) is what the skeleton and harness
//!   spikes do today: `workers - 1` threads spawned for the step and joined
//!   before `run` returns;
//! - the persistent pool belongs to a runtime instance for the process's
//!   life: `workers - 1` threads wait with the measured policy (a bounded spin,
//!   then park) when no job has claimable work, and claim tasks from any
//!   active job, the jobs in submission order and each job in its own claim
//!   order. Several calling threads may have a job active at once.
//!
//! A registered job is reached by the pool's workers through a raw pointer
//! that the calling thread publishes in a slot and retires before `run`
//! returns; a hazard pointer per worker (the sequence number of the job it is
//! reading) lets the caller wait until no worker reads the job any more. That
//! is the one `unsafe` construct of the pool.

use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

/// The measured waiting policy's spin bound before a worker parks.
pub const DEFAULT_SPIN_BOUND: Duration = Duration::from_micros(50);
/// The ring of job slots: at most this many jobs active at once on one pool
/// (one per calling thread; a later caller waits for a free slot).
const JOB_SLOTS: u64 = 64;

/// One claimable task of a job: a chunk of a node of the tick graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    pub node: usize,
    pub chunk: usize,
}

/// What the executor should do after a task: nothing, wake up to that many
/// workers for the tasks the completed node armed, or wake the caller
/// because the job's last task completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Nothing,
    Armed(usize),
    Finished,
}

/// A step of a world as the executor sees it.
pub trait Job: Sync {
    /// Whether a task is claimable right now.
    fn has_work(&self) -> bool;
    /// Whether the job's last task has completed.
    fn is_done(&self) -> bool;
    /// Claims one task, in the job's own claim order.
    fn try_claim(&self) -> Option<Task>;
    /// Runs a claimed task as `worker` and completes its node when it was the last.
    fn execute(&self, task: Task, worker: usize) -> Signal;
}

/// What steps a world: runs a job to completion, the calling thread being
/// worker 0 of the step.
pub trait Executor: Sync {
    fn run(&self, job: &dyn Job);
    /// The workers of a step, the calling thread included.
    fn workers(&self) -> usize;
}

/// Spins until `ready` holds or the bound elapses; true when `ready` held.
fn spin(bound: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    let mut spins = 0_u32;
    loop {
        if ready() {
            return true;
        }
        std::hint::spin_loop();
        spins += 1;
        if spins % 32 == 0 && started.elapsed() >= bound {
            return false;
        }
    }
}

// ---------------------------------------------------------------------------
// The step-scoped team: the control
// ---------------------------------------------------------------------------

/// The pool every earlier spike used: threads spawned at every step and
/// joined before it returns, the calling thread being worker 0.
pub struct StepScoped {
    workers: usize,
    spin_bound: Duration,
}

impl StepScoped {
    pub fn new(workers: usize) -> StepScoped {
        assert!(workers >= 1, "a step is run by at least the calling thread");
        StepScoped { workers, spin_bound: DEFAULT_SPIN_BOUND }
    }
}

/// The threads of one step. Each worker publishes its own handle before it
/// looks for work, so that a parked worker always has a handle to unpark:
/// the job is armed before the threads are spawned, and a wake that found no
/// handle would be lost.
struct Team {
    spin_bound: Duration,
    parked: Vec<AtomicBool>,
    threads: Vec<OnceLock<Thread>>,
}

impl Team {
    /// Wakes up to `count` parked workers of the step.
    fn wake(&self, count: usize) {
        if count == 0 {
            return;
        }
        let mut woken = 0;
        for (worker, thread) in self.threads.iter().enumerate() {
            if self.parked[worker].load(Ordering::SeqCst)
                && let Some(thread) = thread.get()
            {
                thread.unpark();
                woken += 1;
                if woken == count {
                    return;
                }
            }
        }
    }

    fn park(&self, job: &dyn Job, worker: usize) {
        self.parked[worker].store(true, Ordering::SeqCst);
        if !job.has_work() && !job.is_done() {
            thread::park();
        }
        self.parked[worker].store(false, Ordering::SeqCst);
    }

    fn worker_loop(&self, job: &dyn Job, worker: usize) {
        self.threads[worker].set(thread::current()).ok().expect("each worker publishes its handle once");
        loop {
            match job.try_claim() {
                Some(task) => match job.execute(task, worker) {
                    Signal::Nothing => {}
                    Signal::Armed(count) => self.wake(count),
                    Signal::Finished => self.wake(usize::MAX),
                },
                None => {
                    if job.is_done() {
                        return;
                    }
                    if !spin(self.spin_bound, || job.has_work() || job.is_done()) {
                        self.park(job, worker);
                    }
                }
            }
        }
    }
}

impl Executor for StepScoped {
    fn run(&self, job: &dyn Job) {
        let team = Team {
            spin_bound: self.spin_bound,
            parked: (0..self.workers).map(|_| AtomicBool::new(false)).collect(),
            threads: (0..self.workers).map(|_| OnceLock::new()).collect(),
        };
        let team = &team;
        thread::scope(|scope| {
            let handles: Vec<_> = (1..self.workers)
                .map(|worker| {
                    thread::Builder::new()
                        .name(format!("step-worker-{worker}"))
                        .spawn_scoped(scope, move || team.worker_loop(job, worker))
                        .expect("spawn a worker")
                })
                .collect();
            team.worker_loop(job, 0);
            for handle in handles {
                handle.join().expect("a worker never panics outside a task");
            }
        });
    }

    fn workers(&self) -> usize {
        self.workers
    }
}

// ---------------------------------------------------------------------------
// The persistent pool of a runtime instance
// ---------------------------------------------------------------------------

/// The idle instrument of one persistent worker since the last reset: the
/// time spent spinning before finding work or parking, the parks, the
/// unparks it received, the parks that returned without work (a stale unpark
/// token), and whether the worker is parked right now. A worker parked
/// before the reset (a step's last tasks run alone, so the other workers
/// park before the step ends) parks no more while it stays idle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdleStats {
    pub worker: usize,
    pub spin: Duration,
    pub parks: u64,
    pub unparks: u64,
    pub spurious_wakes: u64,
    pub parked: bool,
}

/// A job registered for the pool's workers, on the calling thread's stack
/// for the duration of `run`.
struct JobEntry {
    job: *const (dyn Job + 'static),
    caller: Thread,
    caller_parked: AtomicBool,
}

struct Slot {
    /// The sequence of the job in the slot, 0 when empty.
    sequence: AtomicU64,
    /// The sequence of the last job completed in the slot.
    completed: AtomicU64,
    entry: AtomicPtr<JobEntry>,
}

/// One persistent worker's shared state, on its own cache lines.
#[repr(align(128))]
struct WorkerState {
    /// The sequence of the job the worker is reading, 0 when none.
    hazard: AtomicU64,
    parked: AtomicBool,
    thread: OnceLock<Thread>,
    spin_nanos: AtomicU64,
    parks: AtomicU64,
    unparks: AtomicU64,
    spurious_wakes: AtomicU64,
}

struct Shared {
    spin_bound: Duration,
    shutdown: AtomicBool,
    /// The next job sequence to hand out; sequences start at 1.
    next: AtomicU64,
    /// A lower bound of the active jobs' sequences.
    oldest: AtomicU64,
    slots: Vec<Slot>,
    /// The persistent workers, worker `index + 1` of a step.
    workers: Vec<WorkerState>,
}

impl Shared {
    fn new(workers: usize, spin_bound: Duration) -> Shared {
        Shared {
            spin_bound,
            shutdown: AtomicBool::new(false),
            next: AtomicU64::new(1),
            oldest: AtomicU64::new(1),
            slots: (0..JOB_SLOTS)
                .map(|_| Slot { sequence: AtomicU64::new(0), completed: AtomicU64::new(0), entry: AtomicPtr::new(ptr::null_mut()) })
                .collect(),
            workers: (1..workers)
                .map(|_| WorkerState {
                    hazard: AtomicU64::new(0),
                    parked: AtomicBool::new(false),
                    thread: OnceLock::new(),
                    spin_nanos: AtomicU64::new(0),
                    parks: AtomicU64::new(0),
                    unparks: AtomicU64::new(0),
                    spurious_wakes: AtomicU64::new(0),
                })
                .collect(),
        }
    }

    fn slot(&self, sequence: u64) -> &Slot {
        &self.slots[(sequence % JOB_SLOTS) as usize]
    }

    // --- the registration of jobs ------------------------------------------

    /// Publishes a job in the next slot of the ring and returns its sequence.
    fn publish(&self, entry: &JobEntry) -> u64 {
        let sequence = self.next.fetch_add(1, Ordering::SeqCst);
        let slot = self.slot(sequence);
        // The slot is free once the job that used it one ring ago completed
        // (sequence 0 stands for no previous job).
        let previous = sequence.saturating_sub(JOB_SLOTS);
        while slot.completed.load(Ordering::SeqCst) != previous {
            thread::yield_now();
        }
        slot.entry.store(ptr::from_ref(entry).cast_mut(), Ordering::SeqCst);
        slot.sequence.store(sequence, Ordering::SeqCst);
        sequence
    }

    /// Clears the job's slot and waits until no worker reads the job.
    fn retire(&self, sequence: u64) {
        let slot = self.slot(sequence);
        slot.entry.store(ptr::null_mut(), Ordering::SeqCst);
        slot.sequence.store(0, Ordering::SeqCst);
        for worker in &self.workers {
            while worker.hazard.load(Ordering::SeqCst) == sequence {
                std::hint::spin_loop();
            }
        }
        slot.completed.store(sequence, Ordering::SeqCst);
        self.advance_oldest();
    }

    fn advance_oldest(&self) {
        let mut oldest = self.oldest.load(Ordering::SeqCst);
        loop {
            if oldest >= self.next.load(Ordering::SeqCst) || self.slot(oldest).completed.load(Ordering::SeqCst) < oldest {
                return;
            }
            match self.oldest.compare_exchange(oldest, oldest + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => oldest += 1,
                Err(actual) => oldest = actual,
            }
        }
    }

    /// Visits the active jobs in submission order, each under the worker's
    /// hazard, until `visit` returns a value.
    fn scan<T>(&self, me: &WorkerState, mut visit: impl FnMut(&JobEntry, &dyn Job) -> Option<T>) -> Option<T> {
        let (oldest, next) = (self.oldest.load(Ordering::SeqCst), self.next.load(Ordering::SeqCst));
        for sequence in oldest..next {
            let slot = self.slot(sequence);
            if slot.sequence.load(Ordering::SeqCst) != sequence {
                continue;
            }
            me.hazard.store(sequence, Ordering::SeqCst);
            if slot.sequence.load(Ordering::SeqCst) != sequence {
                me.hazard.store(0, Ordering::SeqCst);
                continue;
            }
            let entry = slot.entry.load(Ordering::SeqCst);
            if entry.is_null() {
                me.hazard.store(0, Ordering::SeqCst);
                continue;
            }
            // SAFETY: the hazard pins the entry and its job. The calling
            // thread keeps both alive until `retire` returns, and `retire`
            // clears the slot's sequence before waiting until no hazard names
            // this sequence; this worker published its hazard before reading
            // the sequence a second time, so a retire that did not see the
            // hazard happened before that read and the read would have failed.
            let entry = unsafe { &*entry };
            let job = unsafe { &*entry.job };
            let result = visit(entry, job);
            me.hazard.store(0, Ordering::SeqCst);
            if result.is_some() {
                return result;
            }
        }
        None
    }

    // --- the wake-ups ------------------------------------------------------

    fn wake_caller(&self, entry: &JobEntry) -> bool {
        if entry.caller_parked.load(Ordering::SeqCst) {
            entry.caller.unpark();
            true
        } else {
            false
        }
    }

    /// Wakes up to `count` parked persistent workers.
    fn wake_workers(&self, count: usize) {
        if count == 0 {
            return;
        }
        let mut woken = 0;
        for worker in &self.workers {
            if worker.parked.load(Ordering::SeqCst)
                && let Some(thread) = worker.thread.get()
            {
                thread.unpark();
                worker.unparks.fetch_add(1, Ordering::Relaxed);
                woken += 1;
                if woken == count {
                    return;
                }
            }
        }
    }

    /// What a persistent worker does with a task's signal: the job's caller
    /// is worker 0 of its step and is woken first.
    fn dispatch(&self, entry: &JobEntry, signal: Signal) {
        match signal {
            Signal::Nothing => {}
            Signal::Armed(count) => {
                let caller_woken = usize::from(self.wake_caller(entry));
                self.wake_workers(count.saturating_sub(caller_woken));
            }
            Signal::Finished => {
                self.wake_caller(entry);
            }
        }
    }

    // --- the persistent workers -------------------------------------------

    fn claim_and_run(&self, me: &WorkerState, index: usize) -> bool {
        self.scan(me, |entry, job| {
            let task = job.try_claim()?;
            let signal = job.execute(task, index);
            self.dispatch(entry, signal);
            Some(())
        })
        .is_some()
    }

    fn any_work(&self, me: &WorkerState) -> bool {
        self.scan(me, |_, job| job.has_work().then_some(())).is_some()
    }

    fn park(&self, me: &WorkerState) {
        me.parked.store(true, Ordering::SeqCst);
        let mut parks = 0_u64;
        while !self.shutdown.load(Ordering::SeqCst) && !self.any_work(me) {
            // Counted before the park, so that a reader sees it while the worker sleeps.
            me.parks.fetch_add(1, Ordering::Relaxed);
            thread::park();
            parks += 1;
        }
        me.parked.store(false, Ordering::SeqCst);
        if parks > 1 {
            me.spurious_wakes.fetch_add(parks - 1, Ordering::Relaxed);
        }
    }

    fn worker_loop(&self, index: usize) {
        let me = &self.workers[index - 1];
        // The handle is published before the first look for work, so that a
        // parked worker always has one to unpark.
        me.thread.set(thread::current()).ok().expect("each worker publishes its handle once");
        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                return;
            }
            if self.claim_and_run(me, index) {
                continue;
            }
            // Nothing claimable in any job: the measured waiting policy.
            let started = Instant::now();
            let found = spin(self.spin_bound, || self.shutdown.load(Ordering::SeqCst) || self.claim_and_run(me, index));
            me.spin_nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            if !found {
                self.park(me);
            }
        }
    }
}

/// The worker pool of a runtime instance, alive for the instance's life.
pub struct Pool {
    shared: Arc<Shared>,
    handles: Vec<JoinHandle<()>>,
}

impl Pool {
    fn new(workers: usize, spin_bound: Duration) -> Pool {
        assert!(workers >= 1, "a step is run by at least the calling thread");
        let shared = Arc::new(Shared::new(workers, spin_bound));
        let handles: Vec<JoinHandle<()>> = (1..workers)
            .map(|index| {
                let shared = Arc::clone(&shared);
                thread::Builder::new().name(format!("pool-worker-{index}")).spawn(move || shared.worker_loop(index)).expect("spawn a pool worker")
            })
            .collect();
        Pool { shared, handles }
    }

    /// The idle instrument of every persistent worker, worker 1 first.
    pub fn idle_stats(&self) -> Vec<IdleStats> {
        self.shared
            .workers
            .iter()
            .enumerate()
            .map(|(index, worker)| IdleStats {
                worker: index + 1,
                spin: Duration::from_nanos(worker.spin_nanos.load(Ordering::Relaxed)),
                parks: worker.parks.load(Ordering::Relaxed),
                unparks: worker.unparks.load(Ordering::Relaxed),
                spurious_wakes: worker.spurious_wakes.load(Ordering::Relaxed),
                parked: worker.parked.load(Ordering::SeqCst),
            })
            .collect()
    }

    pub fn reset_idle_stats(&self) {
        for worker in &self.shared.workers {
            worker.spin_nanos.store(0, Ordering::Relaxed);
            worker.parks.store(0, Ordering::Relaxed);
            worker.unparks.store(0, Ordering::Relaxed);
            worker.spurious_wakes.store(0, Ordering::Relaxed);
        }
    }
}

impl Executor for Pool {
    fn run(&self, job: &dyn Job) {
        let shared = &*self.shared;
        // SAFETY: the lifetime is erased for the slot only. The entry and the
        // job outlive their registration: `retire` clears the slot and waits
        // until no worker's hazard names the job before this function returns.
        let erased: *const (dyn Job + 'static) = unsafe { std::mem::transmute::<*const (dyn Job + '_), *const (dyn Job + 'static)>(job) };
        let entry = JobEntry { job: erased, caller: thread::current(), caller_parked: AtomicBool::new(false) };
        let sequence = shared.publish(&entry);
        loop {
            match job.try_claim() {
                Some(task) => match job.execute(task, 0) {
                    Signal::Nothing | Signal::Finished => {}
                    Signal::Armed(count) => shared.wake_workers(count),
                },
                None => {
                    if job.is_done() {
                        break;
                    }
                    if !spin(shared.spin_bound, || job.has_work() || job.is_done()) {
                        entry.caller_parked.store(true, Ordering::SeqCst);
                        while !job.has_work() && !job.is_done() {
                            thread::park();
                        }
                        entry.caller_parked.store(false, Ordering::SeqCst);
                    }
                }
            }
        }
        shared.retire(sequence);
    }

    fn workers(&self) -> usize {
        self.shared.workers.len() + 1
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        for worker in &self.shared.workers {
            if let Some(thread) = worker.thread.get() {
                thread.unpark();
            }
        }
        for handle in self.handles.drain(..) {
            handle.join().expect("a pool worker never panics outside a task");
        }
    }
}

/// A runtime instance: it owns one pool for its life (one per process in
/// the runtime proposal; the prototype creates one per scenario in turn).
pub struct Runtime {
    pool: Pool,
}

impl Runtime {
    pub fn new(workers: usize) -> Runtime {
        Runtime { pool: Pool::new(workers, DEFAULT_SPIN_BOUND) }
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }
}

