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
//! A replacement that cannot be put in force leaves what is in force: the
//! failure of a new part is not posted to a world that holds the asset
//! available, and is a report. Where the composition of a world gives an
//! admission check to the entry of an asset type, the capability runs it for
//! that world on each part of the type a batch would bring, before the
//! arrival is posted: a part the check refuses is posted to that world as a
//! failure only where the world does not hold the asset available, a first
//! load, and the rest of the batch is posted. The check is a world's, and
//! the resident store the instance's: a part one world refuses is resident,
//! and another world receives it. The report holds each part that was
//! obtained for a world and not posted to it.
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

use content_store::{
    AssetId, Domain, DomainEntry, Mounted, Package, Presentation, PresentationEntry, closure, content_hash, decode_domain, decode_presentation, decode_root_whole, hex, set_hash,
};
use prototype_core::forms::{Preparer, grind, local_platform, stamp};
use prototype_core::table::{
    AdmissionCheck, Arrival, ArrivalQueue, AssetType, Availability, EntryView, Outcome, Owner, Part, Policy, REASON_ADMISSION_REFUSED, REASON_ANOTHER_TYPE,
    REASON_ARTIFACT_GONE, REASON_DEPENDENCIES_DIFFER, REASON_HASH_MISMATCH, REASON_INVALID_PART, REASON_PREPARATION_FAILED, REASON_UNKNOWN_ASSET, Record, Resident, Residents,
};
use prototype_core::world::Extraction;

mod bound;
mod decoder;

pub use bound::{BUFFER as RATE_BUFFER, Bounded, LoadBound, Place, Pool, PoolHandle, Port, Ran, Rates, Served, Work};
pub use decoder::{BlockedMount, Decoded, LoadWatch, NotExpanded, expand};

// ---------------------------------------------------------------------------
// The resident store
// ---------------------------------------------------------------------------

/// The resident store of a runtime instance: the canonical parts by content
/// hash, immutable once inserted, shared by the worlds of the instance.
#[derive(Default)]
pub struct ResidentStore {
    entries: Mutex<BTreeMap<u64, Arc<Resident>>>,
    /// The prepared forms, by the hash they are held under, immutable once
    /// inserted. A form built once is held under the hash the content names,
    /// against which its bytes were verified; a form a machine builds, under
    /// the hash of its bytes.
    forms: Mutex<BTreeMap<u64, Arc<Vec<u8>>>>,
    /// The form last obtained for a part, by the content hash of the part:
    /// what is handed where an entry names no form, which is the case of a
    /// form that is the same computation on every cell, a cache of the
    /// machine that no table names.
    obtained: Mutex<BTreeMap<u64, u64>>,
    /// Under one control, and filled under it alone: what the machine built
    /// for a part, by the content hash of the part, handed wherever the
    /// machine holds one, whatever form the entry names.
    local: Mutex<BTreeMap<u64, Arc<Vec<u8>>>>,
    /// The presentation payloads, by content hash, immutable once inserted:
    /// loaded on the request of a consumer, and read by no world.
    payloads: Mutex<BTreeMap<u64, Arc<Vec<u8>>>>,
}

impl Residents for ResidentStore {
    fn get(&self, hash: u64) -> Option<Arc<Resident>> {
        self.entries.lock().expect("the store is never poisoned").get(&hash).cloned()
    }

    /// The prepared form of a part, as a system reads it inside the domain:
    /// the bytes held under the hash the entry names beside the hash of its
    /// part, and for an entry that names none, the form last obtained for
    /// the part.
    fn form(&self, part: u64, named: Option<u64>) -> Option<Arc<Vec<u8>>> {
        if let Some(local) = self.local.lock().expect("the store is never poisoned").get(&part) {
            return Some(local.clone());
        }
        let hash = named.or_else(|| self.obtained.lock().expect("the store is never poisoned").get(&part).copied())?;
        self.forms.lock().expect("the store is never poisoned").get(&hash).cloned()
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

    /// Inserts a part whose bytes were verified against the hash and
    /// validated where a bound placed that work: on a file thread, or as an
    /// action of the background pool. The bytes are moved.
    fn insert_checked(&self, hash: u64, bytes: Vec<u8>, part: Part) -> Arc<Resident> {
        let mut entries = self.entries.lock().expect("the store is never poisoned");
        entries.entry(hash).or_insert_with(|| Arc::new(Resident { bytes, part })).clone()
    }

    /// Whether a prepared form is resident under a hash.
    pub fn holds_form(&self, hash: u64) -> bool {
        self.forms.lock().expect("the store is never poisoned").contains_key(&hash)
    }

    /// Inserts the prepared form obtained for a part under a hash; the bytes
    /// are moved. A form already there under that hash is left as it is. It
    /// becomes the form last obtained for the part.
    pub fn insert_form(&self, part: u64, hash: u64, bytes: Vec<u8>) {
        self.forms.lock().expect("the store is never poisoned").entry(hash).or_insert_with(|| Arc::new(bytes));
        self.obtained_for(part, hash);
    }

    /// Notes the form last obtained for a part, a form already resident.
    fn obtained_for(&self, part: u64, hash: u64) {
        self.obtained.lock().expect("the store is never poisoned").insert(part, hash);
    }

    /// Under one control: what the machine built for a part is what a system
    /// is handed for it from now on.
    fn prefer_local(&self, part: u64, bytes: Vec<u8>) {
        self.local.lock().expect("the store is never poisoned").entry(part).or_insert_with(|| Arc::new(bytes));
    }

    /// A presentation payload resident under a hash.
    fn payload(&self, hash: u64) -> Option<Arc<Vec<u8>>> {
        self.payloads.lock().expect("the store is never poisoned").get(&hash).cloned()
    }

    /// Inserts a presentation payload, verified by its reader; the bytes
    /// are moved.
    fn insert_payload(&self, hash: u64, bytes: Vec<u8>) -> Arc<Vec<u8>> {
        self.payloads.lock().expect("the store is never poisoned").entry(hash).or_insert_with(|| Arc::new(bytes)).clone()
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

/// What a file thread is asked, in the order received: to read one artifact
/// whole from a store, or to run what its caller gives it on that thread,
/// knowing which file thread it is. The second is how a read is followed, on
/// the thread that made it and before that thread takes its next request, by
/// the work at load a bound leaves on the executor.
enum Job {
    Read(ReadJob),
    Run(Box<dyn FnOnce(usize) + Send>),
    /// The file thread that takes it ends: a host may still hold the port
    /// when the instance stops, so the threads do not wait for it to close.
    Stop,
}

/// Plain file threads behind the store port: the stand-in for the I/O
/// executor. Every read of an artifact goes through one of them.
struct FileThreads {
    jobs: Option<Sender<Job>>,
    threads: Vec<JoinHandle<()>>,
}

impl FileThreads {
    fn new(count: usize) -> FileThreads {
        let (jobs, inbox) = channel::<Job>();
        let inbox = Arc::new(Mutex::new(inbox));
        let threads = (0..count)
            .map(|index| {
                let inbox = inbox.clone();
                let work = move || {
                    loop {
                        let job = inbox.lock().expect("the inbox is never poisoned").recv();
                        match job {
                            Ok(Job::Read((store, hash, answer))) => {
                                let _ = answer.send(store.read(hash));
                            }
                            Ok(Job::Run(run)) => run(index),
                            Ok(Job::Stop) | Err(_) => break,
                        }
                    }
                };
                thread::Builder::new().name(format!("file-{index}")).spawn(work).expect("spawn a file thread")
            })
            .collect();
        FileThreads { jobs: Some(jobs), threads }
    }

    fn read(&self, store: &Arc<dyn Store>, hash: u64) -> Option<Vec<u8>> {
        let (answer, bytes) = channel();
        self.jobs.as_ref()?.send(Job::Read((store.clone(), hash, answer))).ok()?;
        bytes.recv().ok()?
    }

    /// Asks a file thread to run something, after the requests it received
    /// before this one.
    fn run(&self, run: Box<dyn FnOnce(usize) + Send>) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Run(run));
        }
    }

    /// The port itself, for a host that sends it requests of its own.
    fn port(&self) -> Sender<Job> {
        self.jobs.clone().expect("the file threads run")
    }
}

impl Drop for FileThreads {
    fn drop(&mut self) {
        // One stop for each thread, after the requests already received.
        if let Some(jobs) = self.jobs.take() {
            for _ in &self.threads {
                let _ = jobs.send(Job::Stop);
            }
        }
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
    /// asset it posted to a hash counts as held available by that world
    /// until it posts that asset again, whatever the state the world
    /// extracts: a rejection over a budget, or an entry that left the table,
    /// is never learned, and a later batch leaves the asset out. The default
    /// reads the state extracted at the end of the last tick, and nothing
    /// else.
    pub trusts_what_it_posted: bool,
    /// Control: the batch of a move of the manifest is built against a view
    /// older than one tick, the table as the world extracted it before its
    /// last extraction. The default builds it, as every batch, against the
    /// table extracted at the end of the last tick.
    pub moves_built_against_the_table_before_the_last: bool,
    // --- the admission check of a world's composition: its controls, and
    // one reading ---
    /// Control: no admission check is run. The default runs the check the
    /// composition of the world a batch answers gives to the entry of a
    /// type, on each part of that type the batch would bring, before the
    /// arrival is posted: a part it refuses is not posted to a world that
    /// holds the asset available, the asset stays at its hash and the
    /// refusal is a report; for an asset the world does not hold available,
    /// a first load, a failure is posted with the reason of the check.
    pub no_admission_check_run: bool,
    /// Control: a part that the check of one world of the instance refuses
    /// is posted to no world of that instance. The default has the check be
    /// a world's: two worlds may apply two batches from one revision.
    pub a_part_one_world_refuses_is_posted_to_no_world: bool,
    /// Control: the batch of a move of the manifest is held back while the
    /// world refuses one of its assets, and the next move owes the world
    /// this one too. The default posts the batch without the refused asset.
    pub move_held_back_while_an_asset_is_refused: bool,
    /// Control: a replacement the world's check refuses is posted to it as
    /// a failure, as a first load is.
    pub refused_replacement_posted_as_a_failure: bool,
    /// Control: an asset of a move's batch whose part lists a refused asset
    /// is withheld with it. The default posts it: a part names its
    /// dependencies by identifier, and reads the version in force.
    pub dependent_of_a_refused_asset_withheld: bool,
    /// The other reading of the batch of a move where the world refuses a
    /// changed part: the closure of that part is followed all the same, and
    /// an arrival is posted for each asset of it that the table does not
    /// hold available, which no want then names. The default follows the
    /// closure of a changed part the batch brings, and of no other.
    pub closure_of_a_refused_part_posted: bool,
    // --- a prepared form built once: its controls, and two readings ---
    /// Control: the hash of a prepared form is left out of the arrival, and
    /// so of the entry. The default has an arrival to a hash name the hash
    /// of each prepared form built once that the world's composition
    /// declares for the asset's type, as the manifest the world stands
    /// under lists it.
    pub forms_not_named: bool,
    /// Control: a prepared form is taken from what the machine built for
    /// the part, wherever the machine holds one. The default never reads the
    /// machine's cache of forms for a form built once: such a form is
    /// content, read under the hash that names it and verified against it.
    pub forms_taken_from_the_machine: bool,
    /// Control, at a load and at a replay: where the form named cannot be
    /// had, the store holding no bytes under its hash, the capability takes
    /// the form its machine built for the part, or builds it, and holds it
    /// under the hash named. The default fails the load, and refuses the
    /// replay at that record, naming the hash.
    pub local_form_where_the_named_one_cannot_be_had: bool,
    /// The other reading of the confirmation of a restore under the
    /// recorded content: an asset is confirmed where the store holds bytes
    /// under each recorded hash, whatever the manifest the restored world
    /// stands under holds. The default confirms it where that manifest holds
    /// the asset at the recorded hashes, of its canonical part and of its
    /// prepared form built once, and the store holds their bytes.
    pub confirmation_reads_the_store_alone: bool,
    /// Control: the size an arrival names is the size of the canonical part
    /// alone. The default names the size of what the arrival puts in force:
    /// the part with the prepared forms built once that the arrival names.
    pub arrivals_name_the_size_of_the_part_alone: bool,
    /// Control: an arrival names every form the domain manifest holds for
    /// its asset, with its size, whatever the world's composition declares.
    pub arrivals_name_every_form_of_the_manifest: bool,
    /// The other reading of the reason of a first load whose prepared form
    /// built once cannot be had: a preparation that fails. The default gives
    /// the reason of an artifact the store does not hold under the hash the
    /// manifest names, the form being read and never prepared at load.
    pub a_form_that_cannot_be_had_fails_as_a_preparation: bool,
    // --- a presentation manifest beside the domain manifest: its controls.
    // The default reads a presentation manifest for its consumers alone:
    // nothing of it reaches a world ---
    /// Control: the capability resolves an identifier in the presentation
    /// manifest where its instance holds one, and takes the payload it
    /// names for the canonical part. The default resolves every want in the
    /// domain manifest the world stands under.
    pub resolves_in_the_presentation_manifest: bool,
    /// Control: the closure of a want follows the presentation dependencies
    /// with the domain dependencies.
    pub closure_follows_presentation_dependencies: bool,
    /// Control: an arrival is posted once the payloads of its asset are
    /// resident, and is a failure where one cannot be loaded. The default
    /// loads a payload on the request of its consumer and never for a want.
    pub arrivals_wait_for_payloads: bool,
    /// Control: a revision changes an asset whose entry in a presentation
    /// manifest differs. The default posts a world the transitions of the
    /// assets whose entry in the domain manifest differs, and of no other.
    pub presentation_changes_are_revisions: bool,
    /// Control: the hash that names a content set covers every manifest the
    /// instance holds, the presentation manifest included. The default is
    /// the hash of the domain manifest.
    pub set_hash_covers_every_manifest: bool,
    /// Control: the confirmation of a restore under the recorded content
    /// confirms the presentation payloads of the assets the table holds:
    /// an instance that holds no presentation manifest confirms none.
    pub confirmation_confirms_payloads: bool,
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
    /// The other reading of the reason of a first load that the host's
    /// ceiling on a decoder stops, the artifact declaring more than the
    /// ceiling lets the decoder hold: the nearest reason the design text
    /// lists, the bytes do not verify or do not validate. The default gives
    /// that failure a reason of the ceiling's own, the text listing none
    /// that fits it.
    pub over_the_ceiling_fails_as_an_invalid_part: bool,
}

/// What the machine of a runtime instance gives its loading capability: the
/// cache of forms it keeps on the machine, a directory of its own that is
/// not the pipeline's artifact cache, where a form a machine builds for
/// itself is kept under the key of its part's hash and of what prepares it;
/// and the local platform name such a form is built under.
#[derive(Clone, Debug)]
pub struct Machine {
    pub forms: PathBuf,
    pub platform: String,
    /// The bound above which a piece of work at load leaves the executor
    /// for the background pool, where the host of the instance states one.
    /// An instance that is given none has no pool and estimates nothing:
    /// its work at load runs where it ran before a bound existed.
    pub bound: Option<LoadBound>,
}

impl Machine {
    /// The machine of a capability that is given none: its cache of forms
    /// is a directory of its own beside the artifacts of the loose store.
    pub fn beside(loose: &Path) -> Machine {
        Machine { forms: loose.join("forms"), platform: local_platform(), bound: None }
    }

    /// Where the cache of forms keeps the form a machine built for a part.
    pub fn form_path(&self, part: u64, preparer: Preparer) -> PathBuf {
        self.forms.join(format!("{}-{}-{}", hex(part), preparer.name(), preparer.version()))
    }

    /// The form the machine built for a part, where its cache holds one.
    fn form(&self, part: u64, preparer: Preparer) -> Option<Vec<u8>> {
        fs::read(self.form_path(part, preparer)).ok()
    }

    /// The form the machine holds for a part, whatever prepares it.
    fn any_form(&self, part: u64) -> Option<Vec<u8>> {
        [Preparer::Stamp, Preparer::Grind].into_iter().find_map(|preparer| self.form(part, preparer))
    }

    /// Keeps a form the machine built for a part.
    fn keep(&self, part: u64, preparer: Preparer, bytes: &[u8]) {
        let _ = fs::create_dir_all(&self.forms);
        let _ = fs::write(self.form_path(part, preparer), bytes);
    }
}

/// What the capability knows of one attached world.
struct WorldLink {
    queue: Arc<ArrivalQueue>,
    set: Option<u64>,
    /// The world's table as last extracted.
    table: BTreeMap<AssetId, EntryView>,
    /// The table as extracted before that, or as the query gave it where the
    /// world has extracted once at most. Read under one control alone.
    before: BTreeMap<AssetId, EntryView>,
    /// The instance's manifest as this world was last brought up to date
    /// with it: a move of the manifest is posted as what differs from it.
    seen: Arc<Domain>,
    /// The presentation manifest the instance held then. Read under one
    /// control alone.
    seen_presentation: Option<Arc<Presentation>>,
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
    /// alone: the assets whose last arrival posted was to a hash.
    posted: BTreeSet<AssetId>,
    trusted: bool,
    /// The admission checks the world's composition carries, each with the
    /// asset type whose entry it gives it to.
    checks: Vec<(AssetType, AdmissionCheck)>,
    /// The prepared forms the world's composition declares, each with the
    /// asset type it is declared for and what prepares it.
    forms: Vec<(AssetType, Preparer)>,
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

/// Why a part that was obtained for a world was not posted to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The admission check of the world's composition refuses the part, and
    /// the world holds the asset available: the asset stays at its hash.
    RefusedReplacement,
    /// The check refuses the part, and the world does not hold the asset
    /// available: a failure is posted in its place, a first load.
    RefusedFirstLoad,
    /// The check refuses the part of an asset the world holds available,
    /// and a failure is posted all the same: under a control, or as an
    /// answer to a restore.
    RefusedAndPostedAsAFailure,
    /// Under a control: the check of another world of the instance refuses
    /// the part.
    RefusedByAnotherWorld,
    /// Under a control: the part lists this asset, whose part is refused.
    ListsARefusedAsset(AssetId),
    /// Under a control: the batch is held back while the world refuses the
    /// part of this asset.
    HeldBackFor(AssetId),
}

/// A part that was obtained for a world and not posted to it: the world as
/// the capability numbers it, in the order attached, the asset, the content
/// hash of the part and the cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Withheld {
    pub world: u32,
    pub asset: AssetId,
    pub hash: u64,
    pub cause: Cause,
}

/// What happened outside the domain, for the report.
#[derive(Clone, Debug, Default)]
pub struct LoaderReport {
    /// The artifacts read from a store: a resident hash is never read again.
    pub reads: usize,
    /// The refusals that are reports and never inputs: a replacement that
    /// cannot be put in force, with the asset and the reason code.
    pub refusals: Vec<(AssetId, u32)>,
    /// The parts obtained for a world and not posted to it, in the order
    /// the batches were built.
    pub withheld: Vec<Withheld>,
    /// The moves of the manifest held for a world that waited for its answers.
    pub held_moves: usize,
    /// The batches posted, over every world.
    pub batches: usize,
    /// How many times the capability read a presentation manifest: none on
    /// an instance whose selection holds none.
    pub presentation_manifests_read: usize,
    /// How many times it read a presentation payload from a store.
    pub payloads_read: usize,
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
    /// The parts an admission check refused while the batch in the making
    /// was built: the asset, the content hash, and whether the check of
    /// another world alone refused it.
    refused: Vec<(AssetId, u64, bool)>,
    /// The mounted packages whose artifacts are stored in blocks, by the
    /// name of their mount: a load expands such an artifact through the
    /// decoder, under the ceiling of the mount.
    decoders: Vec<(String, Arc<decoder::Blocked>)>,
    /// What the machine gives the capability: its cache of forms and its
    /// local platform name.
    machine: Machine,
    /// The presentation manifest of the revision in place, with its hash,
    /// where the selection the instance stands over holds one: a server's
    /// holds none.
    presentation: Option<(u64, Arc<Presentation>)>,
    /// Where the host states a bound on the work at load: what the instance
    /// measured when it started and what it ran since, with its background
    /// pool.
    bound: Option<(Arc<Bounded>, Pool)>,
    /// The forms the machine built at a load, to keep in its cache of forms
    /// once the batch of that load is posted.
    keeps: Vec<(u64, Preparer, Arc<Vec<u8>>)>,
}

impl Instance {
    fn read(&mut self, hash: u64) -> Option<Vec<u8>> {
        let bytes = self.fetch(hash);
        self.report.reads += usize::from(bytes.is_some());
        bytes
    }

    /// The bytes of a hash, read through the store port from the first
    /// store that holds them, and counted by whoever asks.
    fn fetch(&self, hash: u64) -> Option<Vec<u8>> {
        self.stores.iter().find_map(|store| self.files.read(store, hash))
    }

    fn read_revision(&mut self) -> Result<Domain, String> {
        let root = self.loose.root().ok_or("no revision is in place")?;
        let (domain_hash, presentation) = decode_root_whole(&root)?;
        let bytes = self.read(domain_hash).ok_or_else(|| format!("the domain manifest {} is absent", hex(domain_hash)))?;
        if content_hash(&bytes) != domain_hash {
            return Err(format!("the domain manifest {} does not hash to its name", hex(domain_hash)));
        }
        self.read_presentation(&presentation)?;
        decode_domain(&bytes)
    }

    /// The presentation manifest of the revision in place, where its root
    /// document names one: read where the instance does not hold it yet. An
    /// instance whose selection holds none reads none, and holds none.
    fn read_presentation(&mut self, named: &[(String, u64)]) -> Result<(), String> {
        let Some((profile, hash)) = named.first() else {
            self.presentation = None;
            return Ok(());
        };
        if self.presentation.as_ref().is_some_and(|(held, _)| held == hash) {
            return Ok(());
        }
        let bytes = self.fetch(*hash).filter(|bytes| content_hash(bytes) == *hash);
        let bytes = bytes.ok_or_else(|| format!("the presentation manifest {} of the profile {profile} cannot be had", hex(*hash)))?;
        self.report.presentation_manifests_read += 1;
        self.presentation = Some((*hash, Arc::new(decode_presentation(&bytes)?)));
        Ok(())
    }

    /// The entry the presentation manifest of the instance holds for an
    /// asset: none where the instance holds no presentation manifest, and
    /// none for an asset that has no payload.
    fn presented(&self, asset: &AssetId) -> Option<PresentationEntry> {
        self.presentation.as_ref().and_then(|(_, presentation)| presentation.get(asset).cloned())
    }

    /// A payload, by the hash the presentation manifest names: looked up in
    /// the resident store, and on a miss read through the store port and
    /// verified against that hash. The domain observes none of it.
    fn payload_at(&mut self, hash: u64) -> Result<Arc<Vec<u8>>, u32> {
        if let Some(resident) = self.residents.payload(hash) {
            return Ok(resident);
        }
        let bytes = self.fetch(hash).ok_or(REASON_ARTIFACT_GONE)?;
        self.report.payloads_read += 1;
        if content_hash(&bytes) != hash {
            return Err(REASON_INVALID_PART);
        }
        Ok(self.residents.insert_payload(hash, bytes))
    }

    /// The hash that names the content set of the manifest the instance
    /// holds: the hash of its domain manifest, which holds nothing of a
    /// target. Under one control it covers every manifest the instance
    /// holds, the presentation manifest included.
    fn content_set(&self) -> u64 {
        match &self.presentation {
            Some((presentation, _)) if self.options.set_hash_covers_every_manifest => {
                content_hash(&[set_hash(&self.current).to_le_bytes(), presentation.to_le_bytes()].concat())
            }
            _ => set_hash(&self.current),
        }
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
        let manifest = if self.content_set() == hash {
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
        // An artifact no store holds whole may stand in blocks in a mounted
        // package: it is then expanded by the decoder, under the ceiling.
        let Some(bytes) = self.read(hash) else { return self.expanded(hash).unwrap_or(Err(REASON_ARTIFACT_GONE)) };
        self.residents.insert(hash, bytes).map_err(|_| REASON_INVALID_PART)
    }

    /// Whether an admission check refuses a part of a type for a world:
    /// none where the world's composition carries no check for the type or
    /// its check accepts the part. The check is a world's, and the resident
    /// store the instance's: the check is run for the world a batch
    /// answers, on a part already resident as on one just read, and a part
    /// one world refuses stays resident for the others. Under one control
    /// no check is run; under another a part that the check of any world of
    /// the instance refuses is refused for every world, and the answer says
    /// whether the check of another world alone refused it.
    fn refuses(&self, world: u32, asset_type: &str, part: &Part) -> Option<bool> {
        if self.options.no_admission_check_run {
            return None;
        }
        let refuses = |link: &WorldLink| link.checks.iter().any(|(checked, check)| checked.stable_name() == asset_type && !check.admits(part));
        if refuses(&self.worlds[&world]) {
            return Some(false);
        }
        (self.options.a_part_one_world_refuses_is_posted_to_no_world && self.worlds.values().any(refuses)).then_some(true)
    }

    /// One asset of a closure, for one world: resolved in the manifest,
    /// failed where the manifest holds it under another type than the one
    /// the world's entry holds, looked up in the resident store, and on a
    /// miss read, verified, validated and checked against the dependencies
    /// the manifest lists for it; then put to the admission check, where the
    /// world's composition gives one to the entry of its type. `held` is
    /// the type the world's entry holds, where it holds one.
    fn load(&mut self, world: u32, manifest: &Domain, asset: AssetId, held: Option<AssetType>) -> Outcome {
        // Control: where the instance holds a presentation manifest, the
        // identifier is resolved in it, and the payload it names is taken
        // for the part. An asset that has no payload has no entry there.
        if self.options.resolves_in_the_presentation_manifest && self.presentation.is_some() {
            let Some(presented) = self.presented(&asset) else { return Outcome::Failure(REASON_UNKNOWN_ASSET) };
            let hash = presented.variants.first().map_or(0, |variant| variant.hash);
            return self.part(hash).map_or_else(Outcome::Failure, |_| Outcome::Hash(hash));
        }
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
        if let Some(by_another_world) = self.refuses(world, &entry.asset_type, &resident.part) {
            // The part was obtained and is not posted: what becomes of its
            // asset is decided where the batch is posted.
            self.refused.push((asset, entry.hash, by_another_world));
            return Outcome::Failure(REASON_ADMISSION_REFUSED);
        }
        // An asset is available to a world only when its canonical part and
        // every prepared form the world's composition declares for its type
        // are resident.
        if let Err(code) = self.obtain_forms(world, entry, &resident) {
            return Outcome::Failure(code);
        }
        // Control: the arrival waits for the payloads of its asset, and is
        // a failure where one cannot be loaded.
        if self.options.arrivals_wait_for_payloads
            && let Some(presented) = self.presented(&asset)
        {
            for variant in &presented.variants {
                if let Err(code) = self.payload_at(variant.hash) {
                    return Outcome::Failure(code);
                }
            }
        }
        Outcome::Hash(entry.hash)
    }

    /// The prepared forms the composition of a world declares for an asset
    /// type, by what prepares each.
    fn declared(&self, world: u32, asset_type: &str) -> Vec<Preparer> {
        let forms = self.worlds.get(&world).map_or(&[][..], |link| link.forms.as_slice());
        forms.iter().filter(|(declared, _)| declared.stable_name() == asset_type).map(|(_, preparer)| *preparer).collect()
    }

    /// The form the machine built for a part: taken from the cache of forms
    /// it keeps, or built now from the part and kept there. A form that is
    /// the same computation on every cell is the same bytes wherever it is
    /// built; one that is not is the machine's own, under its local
    /// platform name, and has another hash than the form built once.
    fn own_form(&self, part: u64, resident: &Resident, preparer: Preparer) -> Vec<u8> {
        if let Some(bytes) = self.machine.form(part, preparer) {
            return bytes;
        }
        let bytes = match preparer {
            Preparer::Stamp => stamp(&resident.bytes, &self.machine.platform, None),
            Preparer::Grind => grind(&resident.bytes),
        };
        self.machine.keep(part, preparer, &bytes);
        bytes
    }

    /// A prepared form built once, by the hash that names it: looked up in
    /// the resident store, and on a miss read through the store port and
    /// verified against that hash. It is content, and the machine's cache of
    /// forms is never read for it: a form a machine built for itself has
    /// another hash, and never passes for the producer's. Two controls read
    /// that cache all the same.
    fn form_at(&mut self, part: u64, form: u64) -> Result<(), u32> {
        if self.options.forms_taken_from_the_machine
            && let Some(local) = self.machine.any_form(part)
        {
            self.residents.prefer_local(part, local);
        }
        if self.residents.holds_form(form) {
            self.residents.obtained_for(part, form);
            return Ok(());
        }
        match self.read(form) {
            Some(bytes) if content_hash(&bytes) == form => self.residents.insert_form(part, form, bytes),
            Some(_) => return Err(REASON_INVALID_PART),
            None if self.options.local_form_where_the_named_one_cannot_be_had => {
                // Control: the machine's own form, held under the hash named.
                let resident = self.part(part)?;
                let own = self.own_form(part, &resident, Preparer::Stamp);
                self.residents.insert_form(part, form, own);
            }
            None if self.options.a_form_that_cannot_be_had_fails_as_a_preparation => return Err(REASON_PREPARATION_FAILED),
            None => return Err(REASON_ARTIFACT_GONE),
        }
        Ok(())
    }

    /// Obtains the prepared forms the composition of a world declares for
    /// the type of an asset whose part is resident. A form built once is
    /// the one the manifest lists under what prepares it, read under its
    /// hash; a form that is the same computation on every cell is taken
    /// from the machine's cache of forms or built, and no manifest names it.
    fn obtain_forms(&mut self, world: u32, entry: &DomainEntry, resident: &Resident) -> Result<(), u32> {
        for preparer in self.declared(world, &entry.asset_type) {
            if preparer.built_once() {
                let label = preparer.label();
                let listed = entry.forms.iter().find(|listed| listed.preparer == label).ok_or(REASON_PREPARATION_FAILED)?;
                self.form_at(entry.hash, listed.hash)?;
            } else if self.residents.form(entry.hash, None).is_none() {
                let own = self.own_form(entry.hash, resident, preparer);
                self.residents.insert_form(entry.hash, content_hash(&own), own);
            }
        }
        Ok(())
    }

    /// The arrival of an asset to the hash the manifest names: with the hash
    /// of the prepared form built once that the world's composition declares
    /// for its type, and the size of what the arrival puts in force, that
    /// part with that form, as the manifest gives the size of each.
    fn named(&self, world: u32, manifest: &Domain, asset: AssetId, hash: u64) -> Arrival {
        let Some(entry) = manifest.get(&asset) else { return Arrival::at(asset, hash) };
        let every = self.options.arrivals_name_every_form_of_the_manifest;
        let declared: Vec<String> = self.declared(world, &entry.asset_type).into_iter().filter(|preparer| preparer.built_once()).map(Preparer::label).collect();
        let names: Vec<&str> = entry.forms.iter().map(|form| form.preparer.as_str()).filter(|preparer| every || declared.iter().any(|label| label == preparer)).collect();
        let size = if self.options.arrivals_name_the_size_of_the_part_alone { entry.size } else { entry.size_in_force(&names) };
        let arrival = Arrival::at(asset, hash).sized(size);
        match entry.forms.iter().find(|form| names.contains(&form.preparer.as_str())) {
            Some(form) if !self.options.forms_not_named => arrival.with_form(form.hash),
            _ => arrival,
        }
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
        // Under the control that trusts what was posted, and read under it
        // alone: an asset posted to a hash counts as held available until
        // it is posted again, and a failure posted for it ends that.
        let link = self.worlds.get_mut(&world).expect("an attached world");
        for arrival in &batch {
            match arrival.outcome {
                Outcome::Hash(_) => link.posted.insert(arrival.asset),
                Outcome::Failure(_) => link.posted.remove(&arrival.asset),
            };
        }
        let delay = self.delay_for(world);
        if delay.is_zero() {
            for queue in queues {
                queue.post(&batch);
            }
            // Where the instance stands under a bound on its work at load,
            // the moment the batch stands on the world's queue is kept.
            if let Some((bounded, _)) = &self.bound {
                bounded.posted_now();
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
        self.withhold(world, &mut batch);
        self.deliver(world, batch);
    }

    /// Takes out of a batch the failure of a replacement that cannot be put
    /// in force, a part the world's admission check refuses included, and
    /// reports the parts that check refused. One control posts the failure
    /// of a refused replacement all the same.
    fn withhold(&mut self, world: u32, batch: &mut Vec<Arrival>) {
        let link = self.worlds.get_mut(&world).expect("an attached world");
        let fails_for_its_type = self.options.a_changed_type_fails_an_asset_in_force;
        let refused_as_a_failure = self.options.refused_replacement_posted_as_a_failure;
        batch.retain(|arrival| match arrival.outcome {
            Outcome::Failure(REASON_ANOTHER_TYPE) if fails_for_its_type => true,
            Outcome::Failure(REASON_ADMISSION_REFUSED) if refused_as_a_failure => true,
            Outcome::Failure(code) if code != REASON_UNKNOWN_ASSET && !arrival.marked && link.holds_available(&arrival.asset) => {
                self.report.refusals.push((arrival.asset, code));
                false
            }
            _ => true,
        });
        self.report_the_refused(world, batch);
    }

    /// Reports each part an admission check refused while a batch was
    /// built: obtained for the world and not posted to it, with what the
    /// batch as posted holds for its asset.
    fn report_the_refused(&mut self, world: u32, batch: &[Arrival]) {
        let link = &self.worlds[&world];
        for (asset, hash, by_another_world) in std::mem::take(&mut self.refused) {
            let posted = batch.iter().any(|arrival| arrival.asset == asset);
            let cause = match (by_another_world, posted) {
                (true, _) => Cause::RefusedByAnotherWorld,
                (false, false) => Cause::RefusedReplacement,
                (false, true) if link.holds_available(&asset) => Cause::RefusedAndPostedAsAFailure,
                (false, true) => Cause::RefusedFirstLoad,
            };
            self.report.withheld.push(Withheld { world, asset, hash, cause });
        }
    }

    /// The arrival of one asset of a batch for a world: to the hash the
    /// manifest names, with the size of what it puts in force, which the
    /// manifest gives, or a failure with its reason, which names no size.
    fn arrival(&mut self, world: u32, manifest: &Domain, asset: AssetId, held: Option<AssetType>) -> Arrival {
        match self.load(world, manifest, asset, held) {
            Outcome::Hash(hash) => self.named(world, manifest, asset, hash),
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
        // Control: the closure follows the presentation dependencies with
        // the domain dependencies, where the instance holds a presentation
        // manifest. The default follows the domain manifest alone.
        let presentation = self.presentation.as_ref().filter(|_| self.options.closure_follows_presentation_dependencies).map(|(_, presentation)| presentation.clone());
        let below = |asset: AssetId| -> Vec<(AssetId, AssetId)> {
            let mut listed: Vec<AssetId> = manifest.get(&asset).map_or_else(Vec::new, |entry| entry.dependencies.iter().map(|(dependency, _)| *dependency).collect());
            if let Some(presented) = presentation.as_ref().and_then(|presentation| presentation.get(&asset)) {
                listed.extend(presented.dependencies.iter().filter(|dependency| !listed.contains(dependency)).copied().collect::<Vec<_>>());
            }
            listed.into_iter().map(|dependency| (asset, dependency)).collect()
        };
        let mut open = below(from);
        while let Some((parent, asset)) = open.pop() {
            let left_as = follow.left.iter().find(|(other, _)| *other == asset).map(|(_, asset_type)| *asset_type);
            let failed_for_its_type = |arrival: &Arrival| arrival.asset == asset && arrival.outcome == Outcome::Failure(REASON_ANOTHER_TYPE) && !arrival.marked;
            if let Some(at) = batch.iter().position(failed_for_its_type)
                && left_as.is_none()
                && self.type_below(world, manifest, parent, asset, batch).is_none()
            {
                let again = self.arrival(world, manifest, asset, None);
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
                let arrival = self.arrival(world, manifest, asset, held);
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
        // Under a bound on the work at load, the artifacts of the closures
        // are asked of the executor at once, and each piece of work is
        // placed by its estimate: what follows then finds them resident.
        self.obtain_ahead(world, &manifest, &sorted);
        let mut followed = BTreeSet::new();
        let mut batch = Vec::new();
        for (root, left_as) in &sorted {
            // A want reached before, in the closure of another, was followed there.
            if !followed.insert(*root) {
                continue;
            }
            let mut stops = false;
            if every_asset || !self.worlds[&world].holds_available(root) {
                let arrival = self.arrival(world, &manifest, *root, Some(*left_as));
                stops = self.options.nothing_posted_below_a_failed_asset && matches!(arrival.outcome, Outcome::Failure(_));
                batch.push(Arrival { root: self.root_of(arrival.outcome, *root), ..arrival });
            }
            if !stops {
                let follow = Follow { left: &sorted, every_asset, of_a_want: true, root: *root };
                self.follow_below(world, &manifest, &mut batch, &mut followed, *root, follow);
            }
        }
        self.post(world, batch);
        // A form the machine built for this load is kept in its cache of
        // forms once the batch is posted: no world waits on that write.
        for (part, preparer, form) in std::mem::take(&mut self.keeps) {
            self.machine.keep(part, preparer, &form);
        }
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
            let arrival = self.arrival(world, &manifest, *asset, self.worlds[&world].entry_type(asset));
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
                    batch.push(self.arrival(world, &manifest, asset, held));
                }
            }
        } else {
            self.complete_the_closures_of_a_restore(world, &manifest, &mut batch);
        }
        let link = self.worlds.get_mut(&world).expect("an attached world");
        // The world waits for its first tick to apply these answers.
        link.awaiting_restore = !held.is_empty();
        // An answer whose part the world's admission check refuses is posted
        // as the failure it is: the part was obtained, and is reported.
        self.report_the_refused(world, &batch);
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
                    let arrival = self.arrival(world, manifest, dependency, held);
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
        // Under one control the batch is built against a view older than
        // one tick: the table extracted before the last stands in the place
        // of the last while the batch is built, and is put back after.
        let older = self.options.moves_built_against_the_table_before_the_last;
        let swap = |instance: &mut Instance| {
            let link = instance.worlds.get_mut(&world).expect("an attached world");
            std::mem::swap(&mut link.table, &mut link.before);
        };
        if older {
            swap(self);
        }
        self.post_move_against_the_table(world);
        if older {
            swap(self);
        }
    }

    fn post_move_against_the_table(&mut self, world: u32) {
        let manifest = self.manifest_of(self.worlds[&world].set);
        let link = self.worlds.get_mut(&world).expect("an attached world");
        let seen = std::mem::replace(&mut link.seen, manifest.clone());
        // Control: a revision changes an asset whose entry in a
        // presentation manifest differs. The default reads the domain
        // manifest alone: a presentation swap is outside the domain.
        let presented = self.presentation.as_ref().map(|(_, presentation)| presentation.clone());
        let seen_presented = std::mem::replace(&mut link.seen_presentation, presented.clone());
        let presented_differently = |asset: &AssetId| {
            let entry = |presentation: &Option<Arc<Presentation>>| presentation.as_ref().and_then(|presentation| presentation.get(asset).cloned());
            self.options.presentation_changes_are_revisions && entry(&seen_presented) != entry(&presented)
        };
        let unwanted_left = self.options.moves_leave_entries_without_a_want;
        let failed_left = self.options.moves_leave_failed_entries;
        let changed: Vec<(AssetId, AssetType)> = link
            .table
            .values()
            .filter(|entry| entry.wanted || !unwanted_left)
            .filter(|entry| !(failed_left && matches!(entry.availability, Availability::Failed(_))))
            .filter(|entry| seen.get(&entry.asset) != manifest.get(&entry.asset) || presented_differently(&entry.asset))
            .map(|entry| (entry.asset, entry.asset_type))
            .collect();
        let mut batch = Vec::new();
        let mut parts = Vec::new();
        for (asset, held) in &changed {
            let arrival = self.arrival(world, &manifest, *asset, Some(*held));
            // The closure of a changed part is followed where the batch
            // brings the part: a part the world's admission check refuses
            // is not put in force, and the batch is posted without it.
            // Under the other reading its closure is followed all the same.
            let refused = arrival.outcome == Outcome::Failure(REASON_ADMISSION_REFUSED) && self.options.closure_of_a_refused_part_posted;
            if matches!(arrival.outcome, Outcome::Hash(_)) || refused {
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
        // Control: the batch is held back while the world refuses one of
        // its assets. The world stays where it was last brought up to date,
        // so that the next move owes it this one too; every part the batch
        // held was obtained, and none is posted.
        if self.options.move_held_back_while_an_asset_is_refused
            && let Some((refused, _, _)) = self.refused.first().copied()
        {
            self.worlds.get_mut(&world).expect("an attached world").seen = seen;
            let parts = batch.iter().filter_map(|arrival| match arrival.outcome {
                Outcome::Hash(hash) => Some((arrival.asset, hash)),
                Outcome::Failure(_) => None,
            });
            let obtained: Vec<(AssetId, u64)> = parts.chain(self.refused.drain(..).map(|(asset, hash, _)| (asset, hash))).collect();
            self.report.withheld.extend(obtained.into_iter().map(|(asset, hash)| Withheld { world, asset, hash, cause: Cause::HeldBackFor(refused) }));
            return;
        }
        // Control: an asset whose part lists a refused asset is withheld
        // with it.
        if self.options.dependent_of_a_refused_asset_withheld && !self.refused.is_empty() {
            let refused: Vec<AssetId> = self.refused.iter().map(|(asset, _, _)| *asset).collect();
            let lists = |asset: &AssetId| manifest.get(asset).and_then(|entry| entry.dependencies.iter().map(|(dependency, _)| *dependency).find(|dependency| refused.contains(dependency)));
            let withheld = &mut self.report.withheld;
            batch.retain(|arrival| match (arrival.outcome, lists(&arrival.asset)) {
                (Outcome::Hash(hash), Some(listed)) => {
                    withheld.push(Withheld { world, asset: arrival.asset, hash, cause: Cause::ListsARefusedAsset(listed) });
                    false
                }
                _ => true,
            });
        }
        self.withhold(world, &mut batch);
        if self.options.moves_posted_one_per_tick {
            batch.sort_by_key(|arrival| arrival.asset);
            self.worlds.get_mut(&world).expect("an attached world").backlog.extend(batch);
            return;
        }
        self.deliver(world, batch);
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
        let extracted = extraction.entries.iter().map(|entry| (entry.asset, *entry)).collect();
        link.before = std::mem::replace(&mut link.table, extracted);
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
    /// The admission checks the world's composition carries, each with the
    /// asset type whose schema entry it gives it to. The capability runs
    /// them before an arrival is posted to this world.
    pub checks: Vec<(AssetType, AdmissionCheck)>,
    /// The prepared forms the world's composition declares, each with the
    /// asset type it is declared for and what prepares it. An asset of such
    /// a type is available to this world only when the form is resident,
    /// and an arrival to a hash names the hash of a form built once.
    pub forms: Vec<(AssetType, Preparer)>,
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
    /// The store port of the instance, over its loose store: a host sends
    /// it read requests of its own, which the file threads serve in the
    /// order received, among the capability's.
    port: Port,
    /// Where the host states a bound on the work at load: what the instance
    /// measured and ran, which a host reads while the capability works.
    bounded: Option<Arc<Bounded>>,
}

impl Loader {
    /// Starts the capability over a loose store, with the revision in place.
    pub fn start(loose: &Path, options: Options) -> Result<Loader, String> {
        Loader::start_on(loose, Machine::beside(loose), options)
    }

    /// The same, on a machine whose cache of forms and whose local platform
    /// name the host states.
    pub fn start_on(loose: &Path, machine: Machine, options: Options) -> Result<Loader, String> {
        let residents = Arc::new(ResidentStore::default());
        let loose = Arc::new(LooseStore::new(loose));
        let files = FileThreads::new(2);
        let port = Port::new(files.port(), loose.clone());
        // The bound is a setting of the instance: where its host states
        // one, the instance measures its rates when it starts, and has a
        // background pool.
        let bound = machine.bound.map(Bounded::start);
        let bounded = bound.as_ref().map(|(bounded, _)| bounded.clone());
        let mut instance = Instance {
            stores: vec![loose.clone() as Arc<dyn Store>],
            loose,
            files,
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
            refused: Vec::new(),
            decoders: Vec::new(),
            machine,
            presentation: None,
            bound,
            keeps: Vec::new(),
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
        Ok(Loader { inbox, thread: Some(thread), residents, port, bounded })
    }

    /// The store port of the instance, for a host that sends it read
    /// requests of its own.
    pub fn port(&self) -> Port {
        self.port.clone()
    }

    /// What the instance measured when it started and ran since under the
    /// bound its host states; none on an instance that is given no bound.
    pub fn bounded(&self) -> Option<Arc<Bounded>> {
        self.bounded.clone()
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
            let table: BTreeMap<AssetId, EntryView> = attachment.entries.iter().map(|entry| (entry.asset, *entry)).collect();
            let link = WorldLink {
                queue: attachment.queue,
                set: attachment.set,
                before: table.clone(),
                table,
                seen: instance.current.clone(),
                seen_presentation: instance.presentation.as_ref().map(|(_, presentation)| presentation.clone()),
                awaiting_restore: false,
                held: false,
                backlog: VecDeque::new(),
                standing: attachment.standing.iter().map(|(asset, wants)| (*asset, wants.iter().map(|(owner, _)| *owner).collect())).collect(),
                posted: BTreeSet::new(),
                trusted: instance.options.trusts_what_it_posted,
                checks: attachment.checks,
                forms: attachment.forms,
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
            // Under the other reading the store alone is asked for the bytes
            // of the recorded hash, whatever the manifest holds.
            if !instance.options.confirmation_reads_the_store_alone {
                let manifest = instance.manifest_of(set);
                let entry = manifest.get(&asset).ok_or("the content does not hold the asset")?;
                if entry.hash != hash {
                    return Err(format!("the content holds the asset at another hash, {}", hex(entry.hash)));
                }
            }
            let part = instance.part(hash).map_err(|_| "the part cannot be had".to_string())?;
            // Control: the payloads of the asset are confirmed too. The
            // default confirms what a world reads, and a server holds no
            // presentation manifest to confirm a payload with.
            if instance.options.confirmation_confirms_payloads {
                if instance.presentation.is_none() {
                    return Err("the instance holds no presentation manifest to confirm the payloads of the asset with".to_string());
                }
                for variant in instance.presented(&asset).map_or_else(Vec::new, |presented| presented.variants) {
                    instance.payload_at(variant.hash).map_err(|_| "a payload of the asset cannot be had".to_string())?;
                }
            }
            Ok(part)
        })
    }

    /// The hash that names the content set of the manifest the instance
    /// holds, for a world created under the content at hand.
    pub fn content_set(&self) -> u64 {
        self.call(|instance| instance.content_set())
    }

    /// The presentation manifest the instance holds, for a presentation
    /// consumer: none on an instance whose selection holds none.
    pub fn presentation(&self) -> Option<Presentation> {
        self.call(|instance| instance.presentation.as_ref().map(|(_, presentation)| (**presentation).clone()))
    }

    /// A presentation payload, on the request of its consumer, by the hash
    /// the presentation manifest names: resident, or read and verified. A
    /// payload that cannot be loaded is reported to the consumer, outside
    /// the domain: the error says why.
    pub fn payload(&self, hash: u64) -> Result<Arc<Vec<u8>>, String> {
        self.call(move |instance| {
            instance.payload_at(hash).map_err(|code| match code {
                REASON_ARTIFACT_GONE => "the store holds no bytes under the hash of the payload".to_string(),
                _ => "the bytes of the payload do not hash to its name".to_string(),
            })
        })
    }

    /// Confirms, for the same restore, the prepared form built once that an
    /// entry holds beside the hash of its canonical part: the content the
    /// world would stand under lists that form for the asset, at the
    /// recorded hash, and the store holds its bytes. A form the content
    /// holds at another hash, the form a machine built for itself for
    /// example, or one whose bytes cannot be had, is an error.
    pub fn confirm_form(&self, set: Option<u64>, asset: AssetId, part: u64, form: u64) -> Result<(), String> {
        self.call(move |instance| {
            if let Some(set) = set {
                instance.keep_set(set)?;
            }
            if !instance.options.confirmation_reads_the_store_alone {
                let manifest = instance.manifest_of(set);
                let entry = manifest.get(&asset).ok_or("the content does not hold the asset")?;
                if !entry.forms.iter().any(|listed| listed.hash == form) {
                    let held: Vec<String> = entry.forms.iter().map(|listed| hex(listed.hash)).collect();
                    return Err(format!("the content holds no prepared form of the asset at that hash, and holds [{}]", held.join(", ")));
                }
            }
            instance.form_at(part, form).map_err(|_| "the prepared form cannot be had".to_string())
        })
    }

    /// Makes resident what one record of a replay names: for each arrival
    /// to a hash, the canonical part and the prepared form built once, each
    /// by the hash the record names, read from the store whatever manifest
    /// the instance holds. A replaying world receives its arrivals from its
    /// recording, and from no loading capability. Where a part or a form
    /// cannot be had the replay is refused at that record: the error is the
    /// hash. `forms` are the prepared forms the replaying world's
    /// composition declares: where a record names no form for an asset of
    /// such a type, which a control alone gives, the form is the one the
    /// manifest the world stands under lists.
    pub fn replay(&self, set: Option<u64>, record: &Record, forms: &[(AssetType, Preparer)]) -> Result<(), u64> {
        let (arrivals, forms) = (record.arrivals.clone(), forms.to_vec());
        self.call(move |instance| {
            if let Some(set) = set {
                instance.keep_set(set).map_err(|_| set)?;
            }
            for arrival in &arrivals {
                let Outcome::Hash(part) = arrival.outcome else { continue };
                instance.part(part).map_err(|_| part)?;
                if let Some(form) = arrival.form {
                    instance.form_at(part, form).map_err(|_| form)?;
                    continue;
                }
                let manifest = instance.manifest_of(set);
                let Some(entry) = manifest.get(&arrival.asset).filter(|entry| entry.hash == part) else { continue };
                for (_, preparer) in forms.iter().filter(|(declared, preparer)| declared.stable_name() == entry.asset_type && preparer.built_once()) {
                    let label = preparer.label();
                    if let Some(listed) = entry.forms.iter().find(|listed| listed.preparer == label) {
                        instance.form_at(part, listed.hash).map_err(|_| listed.hash)?;
                    }
                }
            }
            Ok(())
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
            instance.decoders.retain(|(mounted, _)| *mounted != name);
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
