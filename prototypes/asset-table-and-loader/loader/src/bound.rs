//! The bound above which work at load leaves the executor. Throwaway, never
//! product code.
//!
//! The design text places the work at load in two rows: reading an artifact,
//! verifying the hash of a small one and validating a small canonical part
//! run on the I/O executor's file threads; verifying the hash of a large
//! artifact, validating a large canonical part and building a prepared form
//! absent from the machine's cache are each an action of the background
//! pool. What separates the two rows is a setting of the instance, a
//! duration.
//!
//! The stand-ins are those a scenario sheet defines. The executor is the two
//! file threads behind the store port, which serve its requests in the order
//! received. The background pool is a set of worker threads that run actions
//! in the order submitted, each action given the worker that runs it; it
//! keeps one central queue under one lock, as the pool of the import graph
//! spike keeps its ready set, and none of that pool's dependency counting: a
//! load chains its own pieces, and no action adds an action from inside a
//! worker. A bound is a duration, and a piece of work is placed before it
//! runs, so the instance estimates: when it starts it measures, on a buffer
//! of 1 048 576 bytes, its rate in bytes per millisecond for each kind of
//! work at load, and the estimate of a piece of work is its size divided by
//! that rate. A piece whose estimate is above the bound is one action of the
//! pool; any other runs on the file thread that read the artifact, before
//! that thread takes its next request.
//!
//! An instance that is given no bound has no pool and estimates nothing.

use std::collections::VecDeque;
use std::hint::black_box;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use content_store::{AssetId, Domain, closure, content_hash};
use prototype_core::forms::{Preparer, grind};
use prototype_core::table::{AssetType, Part, RepulsionTable, Residents, part_bytes};

use super::{Instance, Job, Store};

/// The length of the buffer on which an instance measures its rates, and
/// over which a synthetic import action runs its passes.
pub const BUFFER: usize = 1_048_576;

/// The setting of an instance: the bound, and the worker threads of its
/// background pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadBound {
    /// A piece of work at load whose estimate is above the bound is an
    /// action of the background pool; any other stays on the file thread
    /// that read the artifact.
    pub bound: Duration,
    pub workers: usize,
}

/// A kind of work at load that a bound places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// Verifying the hash of an artifact.
    Verification,
    /// Validating a canonical part.
    Validation,
    /// Building a prepared form that is the same computation on every cell
    /// and that the machine's cache of forms does not hold.
    Form,
}

impl Work {
    pub const ALL: [Work; 3] = [Work::Verification, Work::Validation, Work::Form];

    pub fn name(self) -> &'static str {
        match self {
            Work::Verification => "verification",
            Work::Validation => "validation",
            Work::Form => "form",
        }
    }
}

/// Where a piece of work at load ran: on a file thread of the executor, or
/// as an action of the background pool, on one of its workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    FileThread(usize),
    Action(usize),
}

/// One piece of work at load, as it ran: its kind, the artifact by its hash
/// and its size, the estimate that placed it, where it ran and when.
#[derive(Clone, Copy, Debug)]
pub struct Ran {
    pub work: Work,
    pub hash: u64,
    pub size: usize,
    pub estimate: Duration,
    pub place: Place,
    pub started: Instant,
    pub ended: Instant,
}

/// The rates an instance measured when it started, in bytes per millisecond,
/// one for each kind of work at load; and how many passes of the content
/// hash over the buffer make a computation of about two milliseconds, which
/// is what a synthetic import action runs.
#[derive(Clone, Copy, Debug)]
pub struct Rates {
    pub verification: f64,
    pub validation: f64,
    pub form: f64,
    pub import_passes: usize,
}

/// The duration of the fastest of three runs: the one other work on the
/// machine disturbed least.
fn fastest(mut run: impl FnMut()) -> Duration {
    let one = |_| {
        let started = Instant::now();
        run();
        started.elapsed()
    };
    (0..3).map(one).min().unwrap_or(Duration::ZERO)
}

impl Rates {
    /// Measures the rates on a buffer of 1 048 576 bytes, which is a
    /// canonical part of that length, so that it validates.
    pub fn measure() -> Rates {
        let buffer = part_bytes(RepulsionTable::DEFAULT, &[], BUFFER - 20, 7);
        debug_assert_eq!(buffer.len(), BUFFER);
        let per_millisecond = |taken: Duration| BUFFER as f64 / (taken.as_secs_f64() * 1_000.0).max(1e-9);
        let verification = fastest(|| {
            black_box(content_hash(black_box(&buffer)));
        });
        let validation = fastest(|| {
            black_box(Part::decode(black_box(&buffer)).is_ok());
        });
        let form = fastest(|| {
            black_box(grind(black_box(&buffer)));
        });
        let import_passes = ((0.002 / verification.as_secs_f64().max(1e-9)).round() as usize).max(1);
        Rates { verification: per_millisecond(verification), validation: per_millisecond(validation), form: per_millisecond(form), import_passes }
    }

    pub fn of(&self, work: Work) -> f64 {
        match work {
            Work::Verification => self.verification,
            Work::Validation => self.validation,
            Work::Form => self.form,
        }
    }

    /// The estimate of a piece of work, in milliseconds: its size divided
    /// by the rate. It is what a bound is compared with.
    pub fn milliseconds(&self, work: Work, size: usize) -> f64 {
        size as f64 / self.of(work)
    }

    /// The same as a duration, to the nanosecond: what a report writes.
    pub fn estimate(&self, work: Work, size: usize) -> Duration {
        Duration::from_secs_f64(self.milliseconds(work, size) / 1_000.0)
    }
}

// ---------------------------------------------------------------------------
// The background pool
// ---------------------------------------------------------------------------

/// An action of the pool: it is given the worker that runs it.
pub type Action = Box<dyn FnOnce(usize) + Send>;

struct Queue {
    actions: VecDeque<Action>,
    stopped: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

/// The stand-in background pool: worker threads that run actions in the
/// order submitted.
pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Pool {
    pub fn new(workers: usize) -> Pool {
        let shared = Arc::new(Shared { queue: Mutex::new(Queue { actions: VecDeque::new(), stopped: false }), wake: Condvar::new() });
        let threads = (0..workers)
            .map(|worker| {
                let shared = shared.clone();
                let work = move || {
                    loop {
                        let mut queue = shared.queue.lock().expect("the pool's queue is never poisoned");
                        let action = loop {
                            if queue.stopped {
                                return;
                            }
                            match queue.actions.pop_front() {
                                Some(action) => break action,
                                None => queue = shared.wake.wait(queue).expect("the pool's queue is never poisoned"),
                            }
                        };
                        drop(queue);
                        action(worker);
                    }
                };
                thread::Builder::new().name(format!("pool-{worker}")).spawn(work).expect("spawn a worker of the pool")
            })
            .collect();
        Pool { shared, threads }
    }

    pub fn handle(&self) -> PoolHandle {
        PoolHandle { shared: self.shared.clone() }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.queue.lock().expect("the pool's queue is never poisoned").stopped = true;
        self.shared.wake.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// What submits actions to a pool: the loading capability, for the pieces
/// of work at load above the bound, and a host, for the actions of an import.
#[derive(Clone)]
pub struct PoolHandle {
    shared: Arc<Shared>,
}

impl PoolHandle {
    /// Submits an action: it runs after those submitted before it.
    pub fn submit(&self, action: Action) {
        self.shared.queue.lock().expect("the pool's queue is never poisoned").actions.push_back(action);
        self.shared.wake.notify_one();
    }

    /// How many actions are queued behind the running ones.
    pub fn queued(&self) -> usize {
        self.shared.queue.lock().expect("the pool's queue is never poisoned").actions.len()
    }
}

// ---------------------------------------------------------------------------
// The store port, for a host that sends it requests of its own
// ---------------------------------------------------------------------------

/// A read request of a host as the executor served it: when it was sent,
/// when a file thread started the read, which one, and whether the store
/// holds the artifact.
#[derive(Clone, Copy, Debug)]
pub struct Served {
    pub requested: Instant,
    pub started: Instant,
    pub thread: usize,
    pub found: bool,
}

/// The store port of an instance, over its loose store. A request reads one
/// artifact whole; the file threads serve the requests in the order
/// received, a host's among the capability's.
#[derive(Clone)]
pub struct Port {
    jobs: Sender<Job>,
    store: Arc<dyn Store>,
}

impl Port {
    pub(super) fn new(jobs: Sender<Job>, store: Arc<dyn Store>) -> Port {
        Port { jobs, store }
    }

    /// Sends one read request and returns at once: what the executor made
    /// of it comes back on `reply`. Returns whether the request was sent.
    pub fn request(&self, hash: u64, reply: &Sender<Served>) -> bool {
        let (store, reply, requested) = (self.store.clone(), reply.clone(), Instant::now());
        let read = move |thread: usize| {
            let started = Instant::now();
            let found = store.read(hash).is_some();
            let _ = reply.send(Served { requested, started, thread, found });
        };
        self.jobs.send(Job::Run(Box::new(read))).is_ok()
    }
}

// ---------------------------------------------------------------------------
// What an instance under a bound measured, and ran
// ---------------------------------------------------------------------------

/// An instance under a bound, as its host reads it while the capability
/// works: the setting, the rates measured at the start, each piece of work
/// at load that ran, and the moment each batch stood on a world's queue.
pub struct Bounded {
    pub setting: LoadBound,
    pub rates: Rates,
    pool: PoolHandle,
    ran: Mutex<Vec<Ran>>,
    posted: Mutex<Vec<Instant>>,
}

/// What a piece of work gave.
enum Progress {
    /// The artifact was read: its bytes, or none where no store holds it.
    Read(Option<Arc<Vec<u8>>>),
    Verified(bool),
    Validated(Result<Part, u32>),
    Formed(Vec<u8>),
    /// The file thread that read the artifact ran every piece the bound
    /// leaves on the executor, and took its next request.
    Left,
}

impl Bounded {
    /// Measures the rates and starts the background pool.
    pub(super) fn start(setting: LoadBound) -> (Arc<Bounded>, Pool) {
        let pool = Pool::new(setting.workers);
        let bounded = Bounded { setting, rates: Rates::measure(), pool: pool.handle(), ran: Mutex::new(Vec::new()), posted: Mutex::new(Vec::new()) };
        (Arc::new(bounded), pool)
    }

    /// The background pool of the instance, for a host that submits the
    /// actions of an import.
    pub fn pool(&self) -> PoolHandle {
        self.pool.clone()
    }

    /// Each piece of work at load that ran, in the order it ended.
    pub fn ran(&self) -> Vec<Ran> {
        self.ran.lock().expect("the log is never poisoned").clone()
    }

    /// The moment each batch stood on a world's queue, in order.
    pub fn posted(&self) -> Vec<Instant> {
        self.posted.lock().expect("the log is never poisoned").clone()
    }

    /// How many batches stood on a world's queue so far.
    pub fn batches(&self) -> usize {
        self.posted.lock().expect("the log is never poisoned").len()
    }

    pub(super) fn posted_now(&self) {
        self.posted.lock().expect("the log is never poisoned").push(Instant::now());
    }

    /// Whether the bound leaves a piece of work on the executor.
    fn stays(&self, work: Work, size: usize) -> bool {
        self.rates.milliseconds(work, size) <= self.setting.bound.as_secs_f64() * 1_000.0
    }

    /// Runs one piece of work where its caller stands, and keeps what ran.
    fn run(&self, work: Work, hash: u64, bytes: &[u8], place: Place) -> Progress {
        let estimate = self.rates.estimate(work, bytes.len());
        let started = Instant::now();
        let progress = match work {
            Work::Verification => Progress::Verified(content_hash(bytes) == hash),
            Work::Validation => Progress::Validated(Part::decode(bytes)),
            Work::Form => Progress::Formed(grind(bytes)),
        };
        let ended = Instant::now();
        self.ran.lock().expect("the log is never poisoned").push(Ran { work, hash, size: bytes.len(), estimate, place, started, ended });
        progress
    }
}

/// One artifact a load obtains under a bound.
struct Loading {
    hash: u64,
    /// The form the machine builds for the part, where the world's
    /// composition declares one that its cache of forms does not hold.
    form: Option<Preparer>,
    bytes: Option<Arc<Vec<u8>>>,
    verified: Option<bool>,
    part: Option<Result<Part, u32>>,
    formed: Option<Vec<u8>>,
    /// The file thread that read the artifact has taken its next request.
    left: bool,
}

impl Loading {
    /// The next piece of work the artifact still needs, in order: none once
    /// it is whole, or once a piece has failed.
    fn next(&self) -> Option<Work> {
        match (&self.verified, &self.part) {
            (Some(false), _) | (_, Some(Err(_))) => None,
            (None, _) => Some(Work::Verification),
            (_, None) => Some(Work::Validation),
            _ if self.form.is_some() && self.formed.is_none() => Some(Work::Form),
            _ => None,
        }
    }
}

impl Instance {
    /// Under a bound: obtains, at once, the artifacts of the closures of the
    /// wants that left a world. Every read is asked of the executor before
    /// any is answered. The file thread that reads an artifact then runs, in
    /// order, each piece of work whose estimate the bound leaves on the
    /// executor, before it takes its next request; each piece above the
    /// bound is one action of the background pool, submitted from the
    /// capability's thread once the file thread has left the artifact, the
    /// pieces of one artifact one after the other. An artifact whose pieces
    /// all succeed is made resident, with the form built for it; one that
    /// cannot be had is left to the load path, which fails it. Without a
    /// bound this does nothing.
    pub(super) fn obtain_ahead(&mut self, world: u32, manifest: &Domain, roots: &[(AssetId, AssetType)]) {
        let Some(bounded) = self.bound.as_ref().map(|(bounded, _)| bounded.clone()) else { return };
        let roots: Vec<AssetId> = roots.iter().map(|(asset, _)| *asset).collect();
        let mut loading: Vec<Loading> = Vec::new();
        for asset in closure(manifest, &roots) {
            let Some(entry) = manifest.get(&asset) else { continue };
            let held = self.worlds.get(&world).is_some_and(|link| link.holds_available(&asset));
            if held || self.residents.get(entry.hash).is_some() || loading.iter().any(|other| other.hash == entry.hash) {
                continue;
            }
            let builds = |preparer: &Preparer| !preparer.built_once() && !self.machine.form_path(entry.hash, *preparer).exists();
            let form = self.declared(world, &entry.asset_type).into_iter().find(builds);
            loading.push(Loading { hash: entry.hash, form, bytes: None, verified: None, part: None, formed: None, left: false });
        }
        let (done, progress) = channel::<(usize, Progress)>();
        for (index, artifact) in loading.iter().enumerate() {
            let (stores, done, bounded, hash, form) = (self.stores.clone(), done.clone(), bounded.clone(), artifact.hash, artifact.form);
            self.files.run(Box::new(move |thread| {
                let Some(bytes) = stores.iter().find_map(|store| store.read(hash)).map(Arc::new) else {
                    let _ = done.send((index, Progress::Read(None)));
                    return;
                };
                let _ = done.send((index, Progress::Read(Some(bytes.clone()))));
                for work in Work::ALL {
                    if (work != Work::Form || form.is_some()) && bounded.stays(work, bytes.len()) {
                        let _ = done.send((index, bounded.run(work, hash, &bytes, Place::FileThread(thread))));
                    }
                }
                drop(bytes);
                let _ = done.send((index, Progress::Left));
            }));
        }
        let mut open = loading.len();
        while open > 0 {
            let Ok((index, given)) = progress.recv() else { break };
            let artifact = &mut loading[index];
            match given {
                Progress::Read(None) => {
                    open -= 1;
                    continue;
                }
                Progress::Read(Some(bytes)) => {
                    self.report.reads += 1;
                    artifact.bytes = Some(bytes);
                }
                Progress::Verified(verified) => artifact.verified = Some(verified),
                Progress::Validated(part) => artifact.part = Some(part),
                Progress::Formed(form) => artifact.formed = Some(form),
                Progress::Left => artifact.left = true,
            }
            // While the file thread holds the artifact, its pieces are its
            // own. Once it has left, one piece is in flight at most: the
            // action submitted here, whose answer brings the next.
            if !artifact.left {
                continue;
            }
            let Some(bytes) = artifact.bytes.clone() else { continue };
            match artifact.next() {
                Some(work) => {
                    let (done, bounded, hash) = (done.clone(), bounded.clone(), artifact.hash);
                    bounded.pool().submit(Box::new(move |worker| {
                        let progress = bounded.run(work, hash, &bytes, Place::Action(worker));
                        drop(bytes);
                        let _ = done.send((index, progress));
                    }));
                }
                None => {
                    open -= 1;
                    drop(bytes);
                    let whole = artifact.bytes.take().map(|bytes| Arc::try_unwrap(bytes).unwrap_or_else(|shared| (*shared).clone()));
                    if let (Some(bytes), Some(true), Some(Ok(part))) = (whole, artifact.verified, artifact.part.take()) {
                        self.residents.insert_checked(artifact.hash, bytes, part);
                        if let (Some(preparer), Some(form)) = (artifact.form, artifact.formed.take()) {
                            self.residents.insert_form(artifact.hash, content_hash(&form), form);
                            if let Some(resident) = self.residents.form(artifact.hash, None) {
                                self.keeps.push((artifact.hash, preparer, resident));
                            }
                        }
                    }
                }
            }
        }
    }
}
