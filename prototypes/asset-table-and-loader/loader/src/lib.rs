//! The loading capability of the prototype. Throwaway, never product code.
//!
//! It turns an identifier into bytes for the worlds of one runtime instance.
//! It resolves every want that leaves a world in the manifest the world
//! stands under: the instance's current manifest (the revision in place, with
//! the mounted packages merged in their order), or the manifest of the
//! content set the world names, which the instance keeps for as long as that
//! world is attached. It reads artifacts through a store port, on file
//! threads that stand in for the I/O executor, verifies and validates them,
//! keeps the instance's resident store, and posts the arrivals of a want's
//! closure as one batch on the world's queue. It links no importer and does
//! not know the pipeline: a new revision reaches it as a notification of the
//! store.
//!
//! What it knows of a world's table is the state the world extracts at the
//! end of a tick, or gives by a query where no tick has ended: per entry,
//! the asset type the entry holds, its availability and whether a want names
//! it. Every batch is complete against that state, and a rejection or a fall
//! is learned from it, never from what was posted. A batch holds no arrival
//! for an asset the table holds available; an asset the manifest holds under
//! another type than its entry holds fails, but a dependency failed for that
//! reason below a parent whose part the batch brings; and a part whose
//! dependencies are not those the manifest lists for it, each under the type
//! the manifest holds that dependency under, fails. An arrival to a hash
//! names the size the manifest gives of what it puts in force.
//!
//! Every behavior a scenario's control changes on purpose is a field of
//! `Options`, and so is every place where the design text was read one way
//! among two.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use content_store::{AssetId, Domain, DomainEntry, Mounted, Package, closure, content_hash, decode_domain, decode_root, hex, set_hash};
use prototype_core::table::{
    Arrival, ArrivalQueue, AssetType, Availability, EntryView, Outcome, Owner, Part, Policy, REASON_ANOTHER_TYPE, REASON_ARTIFACT_GONE,
    REASON_DEPENDENCIES_DIFFER, REASON_HASH_MISMATCH, REASON_INVALID_PART, REASON_UNKNOWN_ASSET, Resident, Residents,
};
use prototype_core::world::Extraction;

// ---------------------------------------------------------------------------
// The resident store
// ---------------------------------------------------------------------------

/// The resident store of a runtime instance: the canonical parts by content
/// hash, immutable once inserted, shared by the worlds of the instance.
#[derive(Default)]
pub struct ResidentStore {
    entries: Mutex<BTreeMap<u64, Arc<Resident>>>,
}

impl Residents for ResidentStore {
    fn get(&self, hash: u64) -> Option<Arc<Resident>> {
        self.entries.lock().expect("the store is never poisoned").get(&hash).cloned()
    }
}

impl ResidentStore {
    /// Verifies the bytes against the hash that names them, validates the
    /// part and inserts it; the bytes are moved, never copied. An entry
    /// already there is returned as is: a resident hash is never read again.
    pub fn insert(&self, hash: u64, bytes: Vec<u8>) -> Result<Arc<Resident>, u32> {
        if content_hash(&bytes) != hash {
            return Err(REASON_HASH_MISMATCH);
        }
        let part = Part::decode(&bytes)?;
        let mut entries = self.entries.lock().expect("the store is never poisoned");
        Ok(entries.entry(hash).or_insert_with(|| Arc::new(Resident { bytes, part })).clone())
    }

    pub fn hashes(&self) -> Vec<u64> {
        self.entries.lock().expect("the store is never poisoned").keys().copied().collect()
    }
}

// ---------------------------------------------------------------------------
// The store port and the file threads
// ---------------------------------------------------------------------------

/// A store answers one question: the bytes of a hash.
pub trait Store: Send + Sync {
    fn read(&self, hash: u64) -> Option<Vec<u8>>;
}

/// The loose store: the artifact cache's directory.
pub struct LooseStore {
    directory: PathBuf,
}

impl LooseStore {
    pub fn new(directory: &Path) -> LooseStore {
        LooseStore { directory: directory.to_path_buf() }
    }

    fn root(&self) -> Option<Vec<u8>> {
        fs::read(self.directory.join("root")).ok()
    }
}

impl Store for LooseStore {
    fn read(&self, hash: u64) -> Option<Vec<u8>> {
        fs::read(self.directory.join("objects").join(hex(hash))).ok()
    }
}

/// A mounted package file is a store too.
struct PackageStore(Package);

impl Store for PackageStore {
    fn read(&self, hash: u64) -> Option<Vec<u8>> {
        self.0.object(hash).map(<[u8]>::to_vec)
    }
}

type ReadJob = (Arc<dyn Store>, u64, Sender<Option<Vec<u8>>>);

/// Plain file threads behind the store port: the stand-in for the I/O
/// executor. Every read of an artifact goes through one of them.
struct FileThreads {
    jobs: Option<Sender<ReadJob>>,
    threads: Vec<JoinHandle<()>>,
}

impl FileThreads {
    fn new(count: usize) -> FileThreads {
        let (jobs, inbox) = channel::<ReadJob>();
        let inbox = Arc::new(Mutex::new(inbox));
        let threads = (0..count)
            .map(|index| {
                let inbox = inbox.clone();
                let work = move || {
                    loop {
                        let job = inbox.lock().expect("the inbox is never poisoned").recv();
                        let Ok((store, hash, answer)) = job else { break };
                        let _ = answer.send(store.read(hash));
                    }
                };
                thread::Builder::new().name(format!("file-{index}")).spawn(work).expect("spawn a file thread")
            })
            .collect();
        FileThreads { jobs: Some(jobs), threads }
    }

    fn read(&self, store: &Arc<dyn Store>, hash: u64) -> Option<Vec<u8>> {
        let (answer, bytes) = channel();
        self.jobs.as_ref()?.send((store.clone(), hash, answer)).ok()?;
        bytes.recv().ok()?
    }
}

impl Drop for FileThreads {
    fn drop(&mut self) {
        self.jobs.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------
// The loading capability
// ---------------------------------------------------------------------------

/// How long a batch is held back before it is posted. A delay moves the
/// moment a batch lands and nothing else: the host waits, before it steps,
/// until the capability has nothing left to post.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delay {
    #[default]
    None,
    /// Five milliseconds times the count of worlds attached after the world
    /// the batch answers: the batches of one round are ready in the reverse
    /// of the order in which their wants left.
    ReverseOfTheOrder,
    /// A duration drawn from the clock, between 20 and 150 milliseconds.
    Clock,
}

/// The controls of the scenarios, the readings the amended text did not
/// take, and the readings the build took. The default is the design text as
/// amended; every field set runs another behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// How long each batch is held back before it is posted.
    pub delay: Delay,
    /// Every arrival to a hash names a root, the asset of the want it
    /// answers: the mark of the candidate unit of a budget that rejects the
    /// closure of one want. No other candidate writes it.
    pub roots_marked: bool,
    // --- controls ---
    /// Control: the answers to a restore are posted unmarked.
    pub answers_unmarked: bool,
    /// Control: a revision that lands while a world waits for the answers to
    /// its restore is posted at once, and may replace an answer in the queue.
    pub revisions_not_held: bool,
    /// Control: a world under a named content set is resolved in the
    /// instance's current manifest.
    pub current_manifest_for_sets: bool,
    /// Control: no answer is posted to a restore for an asset the content at
    /// hand does not hold.
    pub no_answer_for_an_absent_asset: bool,
    /// Control: no asset fails for its type, and the types a part lists for
    /// its dependencies are not checked (the capability as it stood before
    /// an entry held a type).
    pub types_not_checked: bool,
    /// Control: the dependencies a part lists are not checked against those
    /// the manifest lists for it.
    pub dependencies_not_checked: bool,
    /// Control: the batch of a move of the manifest holds no arrival for an
    /// entry that holds no want.
    pub moves_leave_entries_without_a_want: bool,
    /// Control: the batch of a move of the manifest holds an arrival for
    /// each asset the move changes that the table names, and for no other:
    /// no closure of a changed part.
    pub moves_without_the_closure: bool,
    /// Control: the arrivals of a move of the manifest are posted to a world
    /// one per tick, in identifier order, in place of one batch.
    pub moves_posted_one_per_tick: bool,
    /// Control: the batch of a want's closure holds an arrival for every
    /// asset of the closure, one the table holds available included.
    pub closure_posts_assets_in_force: bool,
    /// Control: a batch is posted on the queue of every world of the
    /// instance, and not on the queue of the world it answers alone.
    pub batches_posted_to_every_world: bool,
    // --- the readings the amended text did not take ---
    /// Not taken: every want a command makes is answered, an asset the table
    /// holds available included. The default answers a want only where the
    /// asset it names was unanswered or failed.
    pub every_want_answered: bool,
    /// Not taken: the answers to a restore are posted for the assets the
    /// table holds as available and for nothing else. The default follows
    /// the closure of each answer in the manifest at hand: without it, a
    /// parent whose part at hand lists another dependency leaves the world
    /// gated on a want that never leaves it.
    pub restore_answers_do_not_follow_the_closure: bool,
    // --- the readings the build took; each field runs the other one ---
    /// With the answers to a restore, an ordinary arrival is posted for
    /// every asset of an answer's closure that the table does not hold
    /// available, an entry held failed or unanswered under a want included,
    /// which is what a complete batch is for every other source. The default
    /// is the sentence on a restore as it reads: an asset of the closure
    /// that receives no marked answer and that the saved table holds under
    /// no want.
    pub restore_answers_complete_the_batch: bool,
    /// The closure of a want is not followed below an asset that fails. The
    /// default follows the whole closure the manifest holds, and posts an
    /// arrival for each asset of it that the table does not hold available.
    pub nothing_posted_below_a_failed_asset: bool,
    /// A move of the manifest posts nothing for an entry the table holds
    /// failed: a failed entry moves by a want alone. The default posts an
    /// arrival for every asset whose manifest entry the move changes that
    /// the table names, an asset that enters the manifest included.
    pub moves_leave_failed_entries: bool,
    /// A move of the manifest that holds an asset in force under another
    /// type than its entry holds fails the asset. The default treats it as
    /// any replacement that cannot be put in force: the asset stays at its
    /// hash, and the refusal is a report.
    pub a_changed_type_fails_an_asset_in_force: bool,
    // --- the readings taken on the same-harness stage: each field runs the
    // former default of the build, or the reading not taken ---
    /// Not taken, the build's former default: with the answers to a restore,
    /// an ordinary arrival is posted for each asset of an answer's closure
    /// that receives no marked answer and that the saved table holds under
    /// no want. The default is the narrow form: an ordinary arrival for each
    /// asset that a want the core adds from the part of an answer creates or
    /// returns to unavailable, an entry failed under another want included
    /// and an entry failed under its parent's own standing want left as it
    /// is, read from the owners of the wants the query gives; and for an
    /// entry held unavailable under no want that such a part lists.
    pub restore_answers_for_what_no_want_names: bool,
    /// Not taken, the build's former default: following a closure, a
    /// dependency whose entry the table holds failed for the reason of
    /// another type fails anew for its type, as any asset the manifest
    /// holds under another type than its entry. The default does not fail
    /// it where the part of its parent, brought by the batch, lists it under
    /// the manifest's type, and loads it under that type, which is the type
    /// the core's want gives the entry before the arrival is applied.
    pub dependency_failed_for_its_type_fails_anew: bool,
    /// The other reading of that exception, its letter: it holds as well
    /// below a parent that the table holds available and that the batch
    /// does not bring, on which the core adds no want.
    pub type_exception_below_a_parent_in_force: bool,
    /// Not taken, run as a control: the capability trusts what it posted. An
    /// asset it posted to a hash counts as held available by that world from
    /// then on, whatever the state the world extracts: a rejection over a
    /// budget, or an entry that left the table, is never learned, and a
    /// later batch leaves the asset out. The default reads the state
    /// extracted at the end of the last tick, and nothing else.
    pub trusts_what_it_posted: bool,
    // --- the controls of the scenarios on the batch of a restore ---
    /// Control: at a restore, no failed arrival is posted for an entry the
    /// save holds as failed. The default posts the arrival its load gives,
    /// a failure included.
    pub restore_posts_no_failure_for_a_failed_entry: bool,
    /// Control: an entry the save holds as failed under a want the core
    /// added receives nothing, whichever parent owns that want. The default
    /// leaves it only where the parent whose part lists it owns the want.
    pub restore_leaves_a_failed_entry_under_any_core_want: bool,
    /// Control: the batch of a restore is completed for an entry the save
    /// holds as failed under a want, and not for one it holds as failed
    /// under none.
    pub restore_completes_for_a_failed_entry_under_a_want_alone: bool,
    /// Control: the batch of a restore holds no ordinary arrival for an
    /// entry the save already holds unavailable. The default posts one for
    /// an entry held unavailable under no want that a part of an answer
    /// lists.
    pub restore_posts_nothing_for_an_unavailable_entry: bool,
    /// Control: the batch of a restore is completed one level under an
    /// answer alone, the parts of the ordinary arrivals it holds not being
    /// followed. The default follows them, to any depth.
    pub restore_completes_one_level_alone: bool,
}

/// The prepared forms built once that a world's composition declares, by
/// what prepares each: none, no asset type of this prototype having one.
/// The size an arrival names is that of the part with the forms named here.
const DECLARED_FORMS: [&str; 0] = [];

/// What the capability knows of one attached world.
struct WorldLink {
    queue: Arc<ArrivalQueue>,
    set: Option<u64>,
    /// The world's table as last extracted.
    table: BTreeMap<AssetId, EntryView>,
    /// The instance's manifest as this world was last brought up to date
    /// with it: a move of the manifest is posted as what differs from it.
    seen: Arc<Domain>,
    /// The world waits for its first tick to apply the answers to a restore.
    awaiting_restore: bool,
    /// A move of the manifest landed meanwhile and is held for it.
    held: bool,
    /// Under the control that posts a move one arrival per tick: what is
    /// left to post.
    backlog: VecDeque<Arrival>,
    /// For a restore under the content at hand: the owners of the wants
    /// that stand on each entry the table does not hold available, as the
    /// query before the first tick gave them.
    standing: BTreeMap<AssetId, Vec<Owner>>,
    /// Under the control that trusts what was posted, and read under it
    /// alone: the assets posted to a hash.
    posted: BTreeSet<AssetId>,
    trusted: bool,
}

impl WorldLink {
    /// Whether the world's table holds an asset available, read from the
    /// state the world last extracted: a batch is complete against it.
    fn holds_available(&self, asset: &AssetId) -> bool {
        let extracted = self.table.get(asset).is_some_and(|entry| matches!(entry.availability, Availability::Available(_)));
        extracted || (self.trusted && self.posted.contains(asset))
    }

    fn entry_type(&self, asset: &AssetId) -> Option<AssetType> {
        self.table.get(asset).map(|entry| entry.asset_type)
    }

    /// Whether a parent already owns a want on an entry the table does not
    /// hold available, as the query of a restore gave it.
    fn owns(&self, asset: &AssetId, parent: AssetId) -> bool {
        self.standing.get(asset).is_some_and(|owners| owners.contains(&Owner::Asset(parent)))
    }
}

/// What a closure is followed for, below one asset of a batch.
struct Follow<'a> {
    /// The wants that left, each with the type it left with.
    left: &'a [(AssetId, AssetType)],
    /// An arrival is posted for an asset the table holds available too.
    every_asset: bool,
    /// The closure is the closure of a want: under one reading it is not
    /// followed below an asset that fails.
    of_a_want: bool,
    /// The asset the arrivals name as their root, where roots are marked.
    root: AssetId,
}

/// Whether a batch holds an arrival to a hash for an asset: its part is
/// brought by the batch.
fn brought(batch: &[Arrival], asset: &AssetId) -> bool {
    batch.iter().any(|arrival| arrival.asset == *asset && matches!(arrival.outcome, Outcome::Hash(_)))
}

/// What happened outside the domain, for the report.
#[derive(Clone, Debug, Default)]
pub struct LoaderReport {
    /// The artifacts read from a store: a resident hash is never read again.
    pub reads: usize,
    /// The refusals that are reports and never inputs: a replacement that
    /// cannot be put in force, with the asset and the reason code.
    pub refusals: Vec<(AssetId, u32)>,
    /// The moves of the manifest held for a world that waited for its answers.
    pub held_moves: usize,
    /// The batches posted, over every world.
    pub batches: usize,
}

/// The batches held back by a delay and not posted yet.
type Delayed = Arc<(Mutex<usize>, Condvar)>;

struct Instance {
    loose: Arc<LooseStore>,
    stores: Vec<Arc<dyn Store>>,
    files: FileThreads,
    residents: Arc<ResidentStore>,
    options: Options,
    revision: Domain,
    mounts: Vec<(String, Domain, bool)>,
    current: Arc<Domain>,
    /// The manifests of the content sets the attached worlds name.
    sets: BTreeMap<u64, Arc<Domain>>,
    worlds: BTreeMap<u32, WorldLink>,
    next_world: u32,
    report: LoaderReport,
    delayed: Delayed,
}

impl Instance {
    fn read(&mut self, hash: u64) -> Option<Vec<u8>> {
        for store in &self.stores {
            if let Some(bytes) = self.files.read(store, hash) {
                self.report.reads += 1;
                return Some(bytes);
            }
        }
        None
    }

    fn read_revision(&mut self) -> Result<Domain, String> {
        let root = self.loose.root().ok_or("no revision is in place")?;
        let domain_hash = decode_root(&root)?;
        let bytes = self.read(domain_hash).ok_or_else(|| format!("the domain manifest {} is absent", hex(domain_hash)))?;
        if content_hash(&bytes) != domain_hash {
            return Err(format!("the domain manifest {} does not hash to its name", hex(domain_hash)));
        }
        decode_domain(&bytes)
    }

    fn merge(&mut self) -> Vec<Mounted> {
        let mut current = self.revision.clone();
        let mounted = self.mounts.iter().map(|(_, slice, may_override)| content_store::mount(&mut current, slice, *may_override)).collect();
        self.current = Arc::new(current);
        mounted
    }

    /// The manifest of a content set a world names, which the instance keeps
    /// from then on: the current one where its hash is the set's, or the
    /// manifest the store holds under that hash.
    fn keep_set(&mut self, hash: u64) -> Result<(), String> {
        if self.sets.contains_key(&hash) {
            return Ok(());
        }
        let manifest = if set_hash(&self.current) == hash {
            self.current.clone()
        } else {
            let bytes = self.read(hash).filter(|bytes| content_hash(bytes) == hash);
            Arc::new(decode_domain(&bytes.ok_or_else(|| format!("the manifest of the content set {} cannot be had", hex(hash)))?)?)
        };
        self.sets.insert(hash, manifest);
        Ok(())
    }

    /// The manifest a world stands under.
    fn manifest_of(&self, set: Option<u64>) -> Arc<Domain> {
        match set {
            Some(hash) if !self.options.current_manifest_for_sets => self.sets[&hash].clone(),
            _ => self.current.clone(),
        }
    }

    /// Whether the dependencies a part lists are those the manifest lists
    /// for it, each under the type the manifest holds that dependency under.
    fn dependencies_agree(&self, manifest: &Domain, entry: &DomainEntry, part: &Part) -> bool {
        if self.options.dependencies_not_checked {
            return true;
        }
        if part.dependencies.len() != entry.dependencies.len() {
            return false;
        }
        part.dependencies.iter().zip(&entry.dependencies).all(|((listed, listed_type), (held, held_type))| {
            if listed != held {
                return false;
            }
            if self.options.types_not_checked {
                return true;
            }
            let own = manifest.get(listed).map(|dependency| dependency.asset_type.as_str());
            listed_type.stable_name() == held_type && own.is_none_or(|own| own == held_type)
        })
    }

    /// The part the manifest names under a hash: looked up in the resident
    /// store, and on a miss read, verified and validated.
    fn part(&mut self, hash: u64) -> Result<Arc<Resident>, u32> {
        if let Some(resident) = self.residents.get(hash) {
            return Ok(resident);
        }
        let bytes = self.read(hash).ok_or(REASON_ARTIFACT_GONE)?;
        self.residents.insert(hash, bytes).map_err(|_| REASON_INVALID_PART)
    }

    /// One asset of a closure: resolved in the manifest, failed where the
    /// manifest holds it under another type than the one the world's entry
    /// holds, looked up in the resident store, and on a miss read, verified,
    /// validated and checked against the dependencies the manifest lists for
    /// it. `held` is the type the world's entry holds, where it holds one.
    fn load(&mut self, manifest: &Domain, asset: AssetId, held: Option<AssetType>) -> Outcome {
        let Some(entry) = manifest.get(&asset) else { return Outcome::Failure(REASON_UNKNOWN_ASSET) };
        if !self.options.types_not_checked
            && let Some(held) = held
            && entry.asset_type != held.stable_name()
        {
            return Outcome::Failure(REASON_ANOTHER_TYPE);
        }
        let resident = match self.part(entry.hash) {
            Ok(resident) => resident,
            Err(code) => return Outcome::Failure(code),
        };
        if !self.dependencies_agree(manifest, entry, &resident.part) {
            return Outcome::Failure(REASON_DEPENDENCIES_DIFFER);
        }
        Outcome::Hash(entry.hash)
    }

    /// How long a batch for a world is held back.
    fn delay_for(&self, world: u32) -> Duration {
        match self.options.delay {
            Delay::None => Duration::ZERO,
            Delay::ReverseOfTheOrder => Duration::from_millis(5 * self.worlds.range(world + 1..).count() as u64),
            Delay::Clock => {
                let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos() as u64);
                Duration::from_millis(20 + nanos % 131)
            }
        }
    }

    /// Puts one batch on the queue of the world it answers, once its delay
    /// has passed. A batch is posted under one lock, so that a drain never
    /// sees a part of it.
    fn deliver(&mut self, world: u32, batch: Vec<Arrival>) {
        if batch.is_empty() {
            return;
        }
        let queues: Vec<Arc<ArrivalQueue>> = if self.options.batches_posted_to_every_world {
            self.worlds.values().map(|link| link.queue.clone()).collect()
        } else {
            vec![self.worlds[&world].queue.clone()]
        };
        self.report.batches += 1;
        // Under the control that trusts what was posted, and read under it alone.
        let posted = batch.iter().filter(|arrival| matches!(arrival.outcome, Outcome::Hash(_))).map(|arrival| arrival.asset);
        self.worlds.get_mut(&world).expect("an attached world").posted.extend(posted);
        let delay = self.delay_for(world);
        if delay.is_zero() {
            for queue in queues {
                queue.post(&batch);
            }
            return;
        }
        *self.delayed.0.lock().expect("the count is never poisoned") += 1;
        let delayed = self.delayed.clone();
        let post = move || {
            thread::sleep(delay);
            for queue in queues {
                queue.post(&batch);
            }
            *delayed.0.lock().expect("the count is never poisoned") -= 1;
            delayed.1.notify_all();
        };
        thread::Builder::new().name("delayed-batch".to_string()).spawn(post).expect("spawn the delay of a batch");
    }

    /// Posts one batch to a world, leaving out a failure for an asset the
    /// world holds available: a replacement that cannot be put in force
    /// leaves what is in force, and is a report. An asset that left the
    /// manifest is a different event and does fail, and so does an answer to
    /// a restore.
    fn post(&mut self, world: u32, mut batch: Vec<Arrival>) {
        let link = self.worlds.get_mut(&world).expect("an attached world");
        let fails_for_its_type = self.options.a_changed_type_fails_an_asset_in_force;
        batch.retain(|arrival| match arrival.outcome {
            Outcome::Failure(REASON_ANOTHER_TYPE) if fails_for_its_type => true,
            Outcome::Failure(code) if code != REASON_UNKNOWN_ASSET && !arrival.marked && link.holds_available(&arrival.asset) => {
                self.report.refusals.push((arrival.asset, code));
                false
            }
            _ => true,
        });
        self.deliver(world, batch);
    }

    /// The arrival of one asset of a batch: to the hash the manifest names,
    /// with the size of what it puts in force, which the manifest gives, or
    /// a failure with its reason, which names no size.
    fn arrival(&mut self, manifest: &Domain, asset: AssetId, held: Option<AssetType>) -> Arrival {
        match self.load(manifest, asset, held) {
            Outcome::Hash(hash) => Arrival::at(asset, hash).sized(manifest.get(&asset).map_or(0, |entry| entry.size_in_force(&DECLARED_FORMS))),
            Outcome::Failure(code) => Arrival::failed(asset, code),
        }
    }

    /// The type a dependency is loaded under where a closure reaches it from
    /// a parent: the type the world's entry holds, where it holds one. With
    /// one exception: a dependency whose entry the table holds failed for
    /// the reason of another type, and that the part of its parent, brought
    /// by the batch, lists under the manifest's type, is not failed for its
    /// type and is loaded under the manifest's type, which is the type the
    /// core's want gives the entry before the arrival is applied.
    fn type_below(&self, world: u32, manifest: &Domain, parent: AssetId, asset: AssetId, batch: &[Arrival]) -> Option<AssetType> {
        let entry = self.worlds[&world].table.get(&asset)?;
        if entry.availability == Availability::Failed(REASON_ANOTHER_TYPE) && !self.options.dependency_failed_for_its_type_fails_anew {
            let lists = |(above, own): (&DomainEntry, &DomainEntry)| above.dependencies.iter().any(|(dependency, listed)| *dependency == asset && *listed == own.asset_type);
            let listed = manifest.get(&parent).zip(manifest.get(&asset)).is_some_and(lists);
            if listed && (brought(batch, &parent) || self.options.type_exception_below_a_parent_in_force) {
                return None;
            }
        }
        Some(entry.asset_type)
    }

    /// Follows a closure below one asset of a batch, in the manifest, each
    /// parent before what its part lists: an arrival is added to the batch
    /// for each asset reached that the world's table does not hold available
    /// and that the batch does not hold yet. A want that left is loaded
    /// under the type it left with wherever the closure reaches its asset.
    /// A dependency that failed for its type above and is reached again
    /// below a parent whose part the batch brings is loaded again under the
    /// exception of `type_below`.
    fn follow_below(&mut self, world: u32, manifest: &Domain, batch: &mut Vec<Arrival>, followed: &mut BTreeSet<AssetId>, from: AssetId, follow: Follow) {
        let below = |asset: AssetId| -> Vec<(AssetId, AssetId)> {
            manifest.get(&asset).map_or_else(Vec::new, |entry| entry.dependencies.iter().map(|(dependency, _)| (asset, *dependency)).collect())
        };
        let mut open = below(from);
        while let Some((parent, asset)) = open.pop() {
            let left_as = follow.left.iter().find(|(other, _)| *other == asset).map(|(_, asset_type)| *asset_type);
            let failed_for_its_type = |arrival: &Arrival| arrival.asset == asset && arrival.outcome == Outcome::Failure(REASON_ANOTHER_TYPE) && !arrival.marked;
            if let Some(at) = batch.iter().position(failed_for_its_type)
                && left_as.is_none()
                && self.type_below(world, manifest, parent, asset, batch).is_none()
            {
                let again = self.arrival(manifest, asset, None);
                batch[at] = Arrival { root: self.root_of(again.outcome, follow.root), ..again };
            }
            if !followed.insert(asset) {
                continue;
            }
            let link = &self.worlds[&world];
            let posts = !batch.iter().any(|arrival| arrival.asset == asset) && (follow.every_asset || !link.holds_available(&asset));
            let mut stops = false;
            if posts {
                let held = if left_as.is_some() { left_as } else { self.type_below(world, manifest, parent, asset, batch) };
                let arrival = self.arrival(manifest, asset, held);
                stops = follow.of_a_want && self.options.nothing_posted_below_a_failed_asset && matches!(arrival.outcome, Outcome::Failure(_));
                batch.push(Arrival { root: self.root_of(arrival.outcome, follow.root), ..arrival });
            }
            if !stops {
                open.extend(below(asset));
            }
        }
    }

    /// Answers the wants that left a world: the closure of each in the
    /// manifest the world stands under, as one complete batch that holds no
    /// arrival for an asset the world's table holds available, read from the
    /// state the world extracted at the end of its last tick. A want leaves
    /// with the asset type its entry holds. Where roots are marked, an
    /// arrival names the asset of the want it answers, the first in
    /// identifier order where it stands in the closures of several.
    fn answer(&mut self, world: u32, roots: &[(AssetId, AssetType)], every_asset: bool) {
        if roots.is_empty() {
            return;
        }
        let manifest = self.manifest_of(self.worlds[&world].set);
        let every_asset = every_asset || self.options.closure_posts_assets_in_force;
        let mut sorted: Vec<(AssetId, AssetType)> = roots.to_vec();
        sorted.sort_by_key(|(asset, _)| *asset);
        sorted.dedup_by_key(|(asset, _)| *asset);
        let mut followed = BTreeSet::new();
        let mut batch = Vec::new();
        for (root, left_as) in &sorted {
            // A want reached before, in the closure of another, was followed there.
            if !followed.insert(*root) {
                continue;
            }
            let mut stops = false;
            if every_asset || !self.worlds[&world].holds_available(root) {
                let arrival = self.arrival(&manifest, *root, Some(*left_as));
                stops = self.options.nothing_posted_below_a_failed_asset && matches!(arrival.outcome, Outcome::Failure(_));
                batch.push(Arrival { root: self.root_of(arrival.outcome, *root), ..arrival });
            }
            if !stops {
                let follow = Follow { left: &sorted, every_asset, of_a_want: true, root: *root };
                self.follow_below(world, &manifest, &mut batch, &mut followed, *root, follow);
            }
        }
        self.post(world, batch);
    }

    /// The root an arrival names: none but where roots are marked, and none
    /// for an arrival to failed, which has no size to test.
    fn root_of(&self, outcome: Outcome, root: AssetId) -> Option<AssetId> {
        (self.options.roots_marked && matches!(outcome, Outcome::Hash(_))).then_some(root)
    }

    /// Answers a restore under the content at hand: one marked answer per
    /// asset the world's table holds as available, an entry with no want
    /// included; and, following the closure of each answer in the manifest
    /// the world stands under, the ordinary arrivals that complete the batch
    /// of each closure, so that the gate opens on a complete batch.
    fn answer_restore(&mut self, world: u32, held: &[AssetId]) {
        let manifest = self.manifest_of(self.worlds[&world].set);
        let marked = !self.options.answers_unmarked;
        let mut batch = Vec::new();
        for asset in held {
            let arrival = self.arrival(&manifest, *asset, self.worlds[&world].entry_type(asset));
            if self.options.no_answer_for_an_absent_asset && arrival.outcome == Outcome::Failure(REASON_UNKNOWN_ASSET) {
                continue;
            }
            batch.push(Arrival { marked, ..arrival });
        }
        let wider = self.options.restore_answers_complete_the_batch;
        if self.options.restore_answers_do_not_follow_the_closure {
            // Not taken: the answers and nothing else.
        } else if wider || self.options.restore_answers_for_what_no_want_names {
            // The two forms not taken: an ordinary arrival for every asset
            // of a closure that the table does not hold available, or for
            // every one the saved table holds under no want.
            let answered: Vec<AssetId> = batch.iter().map(|arrival| arrival.asset).collect();
            for asset in closure(&manifest, &answered) {
                let link = &self.worlds[&world];
                let wanted = link.table.get(&asset).is_some_and(|entry| entry.wanted);
                let posted = if wider { !link.holds_available(&asset) } else { !wanted };
                if !answered.contains(&asset) && posted {
                    let held = link.entry_type(&asset);
                    batch.push(self.arrival(&manifest, asset, held));
                }
            }
        } else {
            self.complete_the_closures_of_a_restore(world, &manifest, &mut batch);
        }
        let link = self.worlds.get_mut(&world).expect("an attached world");
        // The world waits for its first tick to apply these answers.
        link.awaiting_restore = !held.is_empty();
        self.deliver(world, batch);
    }

    /// The narrow form of the batch of a restore: with the answers, an
    /// ordinary arrival, unmarked, for each asset that a want the core adds
    /// from the part of an answer creates or returns to unavailable, and for
    /// an entry the save holds unavailable under no want that such a part
    /// lists.
    ///
    /// The core adds its wants at the first step of the first tick, starting
    /// from the part of an asset that holds a want at that step and
    /// following it downwards, to any depth: the parents here are the assets
    /// the saved table holds under a want whose arrival in the batch is to a
    /// hash, and every asset reached below one whose arrival is to a hash,
    /// the core's want then naming it. For each dependency such a part lists
    /// that receives no arrival yet: an asset the saved table does not name
    /// is created by the want; an entry the save holds as failed returns to
    /// unavailable where the want is added on it, which it is but where its
    /// parent's own want already stands on it, as the owners the query gave
    /// tell, and such an entry receives nothing and stays failed; an entry
    /// the save holds unavailable under no want is left unanswered by the
    /// want; one it holds unavailable under a want is answered as any
    /// unanswered want is, by the batch of its own closure.
    fn complete_the_closures_of_a_restore(&mut self, world: u32, manifest: &Domain, batch: &mut Vec<Arrival>) {
        let link = &self.worlds[&world];
        let wanted = |asset: &AssetId| link.table.get(asset).is_some_and(|entry| entry.wanted);
        let mut open: Vec<AssetId> = batch.iter().map(|arrival| arrival.asset).filter(|asset| brought(batch, asset) && wanted(asset)).collect();
        let mut parents = BTreeSet::new();
        while let Some(parent) = open.pop() {
            if !parents.insert(parent) {
                continue;
            }
            let dependencies: Vec<AssetId> = manifest.get(&parent).map_or_else(Vec::new, |entry| entry.dependencies.iter().map(|(dependency, _)| *dependency).collect());
            let options = self.options;
            for dependency in dependencies {
                if let Some(held) = batch.iter().find(|arrival| arrival.asset == dependency) {
                    // An answer, or an arrival another parent brought: where
                    // it is to a hash, its part adds wants in its turn. One
                    // control follows the part of an answer alone.
                    let ordinary = !held.marked && !self.options.answers_unmarked;
                    if brought(batch, &dependency) && !(ordinary && options.restore_completes_one_level_alone) {
                        open.push(dependency);
                    }
                    continue;
                }
                let link = &self.worlds[&world];
                let held_failed = link.table.get(&dependency).is_some_and(|entry| matches!(entry.availability, Availability::Failed(_)));
                let posted = match link.table.get(&dependency) {
                    None => true,
                    Some(entry) => match entry.availability {
                        Availability::Available(_) => false,
                        // A failed entry returns to unavailable where the
                        // want is added on it: everywhere but where this
                        // parent's own want already stands on it. Two
                        // controls post for fewer entries.
                        Availability::Failed(_) => {
                            let under_a_core_want = link.standing.get(&dependency).is_some_and(|owners| owners.iter().any(|owner| matches!(owner, Owner::Asset(_))));
                            let left = if options.restore_leaves_a_failed_entry_under_any_core_want { under_a_core_want } else { link.owns(&dependency, parent) };
                            !left && (entry.wanted || !options.restore_completes_for_a_failed_entry_under_a_want_alone)
                        }
                        Availability::Unavailable => !entry.wanted && !options.restore_posts_nothing_for_an_unavailable_entry,
                    },
                };
                if posted {
                    let held = self.type_below(world, manifest, parent, dependency, batch);
                    let arrival = self.arrival(manifest, dependency, held);
                    // Under one control a load that fails again is not
                    // posted for an entry the save holds as failed.
                    let failed_again = held_failed && matches!(arrival.outcome, Outcome::Failure(_));
                    if failed_again && options.restore_posts_no_failure_for_a_failed_entry {
                        continue;
                    }
                    batch.push(arrival);
                    if brought(batch, &dependency) && !options.restore_completes_one_level_alone {
                        open.push(dependency);
                    }
                }
            }
        }
    }

    /// The transitions a move of the manifest owes one world, as one batch:
    /// an arrival for each asset the move changes that the world's table
    /// names, an entry with no want included, with an arrival for each asset
    /// of the closure of a changed part that the table does not hold
    /// available, and for no other asset. The table is the state the world
    /// extracted at the end of its last tick. Where roots are marked, an
    /// arrival names the asset the move changed that brought it: itself, or
    /// the changed asset whose closure holds it, the first in identifier
    /// order.
    fn post_move(&mut self, world: u32) {
        let manifest = self.manifest_of(self.worlds[&world].set);
        let link = self.worlds.get_mut(&world).expect("an attached world");
        let seen = std::mem::replace(&mut link.seen, manifest.clone());
        let unwanted_left = self.options.moves_leave_entries_without_a_want;
        let failed_left = self.options.moves_leave_failed_entries;
        let changed: Vec<(AssetId, AssetType)> = link
            .table
            .values()
            .filter(|entry| entry.wanted || !unwanted_left)
            .filter(|entry| !(failed_left && matches!(entry.availability, Availability::Failed(_))))
            .filter(|entry| seen.get(&entry.asset) != manifest.get(&entry.asset))
            .map(|entry| (entry.asset, entry.asset_type))
            .collect();
        let mut batch = Vec::new();
        let mut parts = Vec::new();
        for (asset, held) in &changed {
            let arrival = self.arrival(&manifest, *asset, Some(*held));
            if matches!(arrival.outcome, Outcome::Hash(_)) {
                parts.push(*asset);
            }
            batch.push(Arrival { root: self.root_of(arrival.outcome, *asset), ..arrival });
        }
        if !self.options.moves_without_the_closure {
            let mut followed = BTreeSet::new();
            for part in &parts {
                let follow = Follow { left: &[], every_asset: false, of_a_want: false, root: *part };
                self.follow_below(world, &manifest, &mut batch, &mut followed, *part, follow);
            }
        }
        if self.options.moves_posted_one_per_tick {
            batch.sort_by_key(|arrival| arrival.asset);
            self.worlds.get_mut(&world).expect("an attached world").backlog.extend(batch);
            return;
        }
        self.post(world, batch);
    }

    /// The manifest moved: every world that names no content set receives
    /// its transitions as one batch, but a world that waits for the answers
    /// to its restore, for which the move is held.
    fn moved(&mut self) {
        let worlds: Vec<u32> = self.worlds.keys().copied().collect();
        for world in worlds {
            let link = self.worlds.get_mut(&world).expect("an attached world");
            if link.set.is_some() && !self.options.current_manifest_for_sets {
                continue;
            }
            if link.awaiting_restore && !self.options.revisions_not_held {
                if !link.held {
                    self.report.held_moves += 1;
                }
                link.held = true;
                continue;
            }
            self.post_move(world);
        }
    }

    fn extracted(&mut self, world: u32, extraction: Extraction) {
        let Some(link) = self.worlds.get_mut(&world) else { return };
        link.table = extraction.entries.iter().map(|entry| (entry.asset, *entry)).collect();
        let held = extraction.restore_applied && std::mem::take(&mut link.held);
        if extraction.restore_applied {
            link.awaiting_restore = false;
        }
        if self.options.every_want_answered {
            let mut roots = extraction.left;
            roots.extend(extraction.wanted);
            self.answer(world, &roots, true);
        } else {
            self.answer(world, &extraction.left, false);
        }
        if held {
            self.post_move(world);
        }
    }

    /// The host waits before it steps: under the control that posts a move
    /// one arrival per tick, the next arrival of each world is posted now.
    fn settled(&mut self) -> Delayed {
        let worlds: Vec<u32> = self.worlds.keys().copied().collect();
        for world in worlds {
            if let Some(arrival) = self.worlds.get_mut(&world).and_then(|link| link.backlog.pop_front()) {
                self.post(world, vec![arrival]);
            }
        }
        self.delayed.clone()
    }
}

enum Message {
    Run(Box<dyn FnOnce(&mut Instance) + Send>),
    Stop,
}

/// What a host attaches a world with: where its arrivals are posted, the
/// content set it names, and what a query of its table gives at the boundary
/// where no tick has ended.
pub struct Attachment {
    pub queue: Arc<ArrivalQueue>,
    pub set: Option<u64>,
    /// The table: per entry, its type, its availability and whether a want
    /// names it.
    pub entries: Vec<EntryView>,
    /// The wants whose asset is unanswered, each with the type its entry holds.
    pub unanswered: Vec<(AssetId, AssetType)>,
    /// For a restore under the content at hand: the entries the table holds
    /// available, which that restore answers.
    pub restore: Vec<AssetId>,
    /// For the same restore: for each entry the table does not hold
    /// available, the wants that stand on it with their owners.
    pub standing: Vec<(AssetId, Vec<(Owner, Policy)>)>,
}

/// The capability held still: while it lives, the capability handles nothing
/// and posts nothing. A host takes it to act before the capability has
/// posted, and drops it to let the capability go on.
pub struct Hold {
    _release: Sender<()>,
}

/// The loading capability of one runtime instance, on a thread of its own.
pub struct Loader {
    inbox: Sender<Message>,
    thread: Option<JoinHandle<()>>,
    residents: Arc<ResidentStore>,
}

impl Loader {
    /// Starts the capability over a loose store, with the revision in place.
    pub fn start(loose: &Path, options: Options) -> Result<Loader, String> {
        let residents = Arc::new(ResidentStore::default());
        let loose = Arc::new(LooseStore::new(loose));
        let mut instance = Instance {
            stores: vec![loose.clone() as Arc<dyn Store>],
            loose,
            files: FileThreads::new(2),
            residents: residents.clone(),
            options,
            revision: Domain::new(),
            mounts: Vec::new(),
            current: Arc::new(Domain::new()),
            sets: BTreeMap::new(),
            worlds: BTreeMap::new(),
            next_world: 0,
            report: LoaderReport::default(),
            delayed: Delayed::default(),
        };
        instance.revision = instance.read_revision()?;
        instance.merge();
        let (inbox, messages): (Sender<Message>, Receiver<Message>) = channel();
        let work = move || {
            while let Ok(Message::Run(run)) = messages.recv() {
                run(&mut instance);
            }
        };
        let thread = thread::Builder::new().name("loader".to_string()).spawn(work).map_err(|error| error.to_string())?;
        Ok(Loader { inbox, thread: Some(thread), residents })
    }

    /// Runs on the capability's thread and waits for the result: everything
    /// sent before it has been handled when it returns.
    fn call<R: Send + 'static>(&self, run: impl FnOnce(&mut Instance) -> R + Send + 'static) -> R {
        let (answer, result) = channel();
        let run = move |instance: &mut Instance| {
            let _ = answer.send(run(instance));
        };
        self.inbox.send(Message::Run(Box::new(run))).expect("the loader runs");
        result.recv().expect("the loader answers")
    }

    /// The resident store of the instance: the extension point a world reads parts through.
    pub fn residents(&self) -> Arc<ResidentStore> {
        self.residents.clone()
    }

    /// Attaches a world and answers what its table asks at the boundary.
    /// Returns the function the world extracts its wants through.
    pub fn attach(&self, attachment: Attachment) -> Result<Arc<dyn Fn(Extraction) + Send + Sync>, String> {
        let world = self.call(move |instance| -> Result<u32, String> {
            if let Some(hash) = attachment.set {
                instance.keep_set(hash)?;
            }
            let world = instance.next_world;
            instance.next_world += 1;
            let link = WorldLink {
                queue: attachment.queue,
                set: attachment.set,
                table: attachment.entries.iter().map(|entry| (entry.asset, *entry)).collect(),
                seen: instance.current.clone(),
                awaiting_restore: false,
                held: false,
                backlog: VecDeque::new(),
                standing: attachment.standing.iter().map(|(asset, wants)| (*asset, wants.iter().map(|(owner, _)| *owner).collect())).collect(),
                posted: BTreeSet::new(),
                trusted: instance.options.trusts_what_it_posted,
            };
            instance.worlds.insert(world, link);
            instance.answer_restore(world, &attachment.restore);
            instance.answer(world, &attachment.unanswered, false);
            Ok(world)
        })?;
        let inbox = Mutex::new(self.inbox.clone());
        Ok(Arc::new(move |extraction: Extraction| {
            let run = move |instance: &mut Instance| instance.extracted(world, extraction);
            let _ = inbox.lock().expect("the inbox is never poisoned").send(Message::Run(Box::new(run)));
        }))
    }

    /// Confirms, for a restore under the recorded content, that the content
    /// a world would stand under holds an asset at the recorded hash, and
    /// has the part: the precondition of the restore's first tick, outside
    /// any input record. An asset the content does not hold, holds at
    /// another hash, or whose part cannot be had, is an error.
    pub fn confirm(&self, set: Option<u64>, asset: AssetId, hash: u64) -> Result<Arc<Resident>, String> {
        self.call(move |instance| {
            if let Some(set) = set {
                instance.keep_set(set)?;
            }
            let manifest = instance.manifest_of(set);
            let entry = manifest.get(&asset).ok_or("the content does not hold the asset")?;
            if entry.hash != hash {
                return Err(format!("the content holds the asset at another hash, {}", hex(entry.hash)));
            }
            instance.part(hash).map_err(|_| "the part cannot be had".to_string())
        })
    }

    /// Holds the capability still until the returned value is dropped.
    pub fn hold(&self) -> Hold {
        let (release, held) = channel::<()>();
        let wait = move |_: &mut Instance| {
            let _ = held.recv();
        };
        self.inbox.send(Message::Run(Box::new(wait))).expect("the loader runs");
        Hold { _release: release }
    }

    /// Waits until the capability has nothing left to post, the batches a
    /// delay holds back included.
    pub fn settle(&self) {
        let delayed = self.call(Instance::settled);
        let mut count = delayed.0.lock().expect("the count is never poisoned");
        while *count > 0 {
            count = delayed.1.wait(count).expect("the count is never poisoned");
        }
    }

    /// The store's notification: a new revision is in place.
    pub fn revision_in_place(&self) -> Result<(), String> {
        self.call(|instance| {
            instance.revision = instance.read_revision()?;
            instance.merge();
            instance.moved();
            Ok(())
        })
    }

    /// Mounts a package after those already mounted. A mount while worlds
    /// exist is a revision.
    pub fn mount(&self, name: &str, file: Vec<u8>, may_override: bool) -> Result<Mounted, String> {
        let name = name.to_string();
        self.call(move |instance| {
            let package = Package::read(file)?;
            instance.mounts.push((name, package.domain.clone(), may_override));
            instance.stores.push(Arc::new(PackageStore(package)));
            let mounted = instance.merge().pop().expect("one result per mount");
            instance.moved();
            Ok(mounted)
        })
    }

    pub fn unmount(&self, name: &str) -> Result<(), String> {
        let name = name.to_string();
        self.call(move |instance| {
            let at = instance.mounts.iter().position(|(mounted, _, _)| *mounted == name).ok_or("no such mount")?;
            instance.mounts.remove(at);
            // The loose store is the first store; the packages follow in mount order.
            instance.stores.remove(at + 1);
            instance.merge();
            instance.moved();
            Ok(())
        })
    }

    /// The instance's current manifest, or the manifest of a content set an
    /// attached world names.
    pub fn manifest(&self, set: Option<u64>) -> Domain {
        self.call(move |instance| match set {
            Some(hash) => instance.sets.get(&hash).map(|manifest| (**manifest).clone()).unwrap_or_default(),
            None => (*instance.current).clone(),
        })
    }

    pub fn report(&self) -> LoaderReport {
        self.call(|instance| instance.report.clone())
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        let _ = self.inbox.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests;
