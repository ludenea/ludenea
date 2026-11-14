//! The loading capability of the prototype. Throwaway, never product code.
//!
//! It turns an identifier into bytes for the worlds of one runtime instance.
//! It resolves every want in the manifest the world stands under: the
//! instance's current manifest (the revision in place, with the mounted
//! packages merged in their order), or the manifest of the content set the
//! world names, which the instance keeps for as long as that world is
//! attached. It reads artifacts through a store port, on file threads that
//! stand in for the I/O executor, verifies and validates them, keeps the
//! instance's resident store, and posts the arrivals of a want's closure as
//! one batch on the world's queue. It links no importer and does not know
//! the pipeline: a new revision reaches it as a notification of the store.
//!
//! Every behavior a scenario's control changes on purpose is a field of
//! `Options`, and so is one place where the design text was read one way
//! among two.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use content_store::{AssetId, Domain, Mounted, Package, closure, content_hash, decode_domain, decode_root, hex, set_hash};
use prototype_core::table::{
    Arrival, ArrivalQueue, Outcome, Part, REASON_ARTIFACT_GONE, REASON_HASH_MISMATCH, REASON_INVALID_PART, REASON_UNKNOWN_ASSET, Resident, Residents,
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

/// The controls of the scenarios, and one reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
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
    /// Reading: a new want of an asset the world already holds is answered
    /// by no arrival. The default answers every want a command makes, the
    /// load path stating no exemption.
    pub a_want_of_an_asset_in_force_is_not_answered: bool,
    /// Reading: the answers to a restore are posted for the assets the table
    /// holds as available and for nothing else. The default follows the
    /// closure of each answer in the manifest at hand, and posts an asset of
    /// it that the table does not hold as an ordinary arrival in the same
    /// batch: without it, a parent whose part at hand lists another
    /// dependency leaves the world gated on a want that never leaves it.
    pub restore_answers_do_not_follow_the_closure: bool,
}

/// What the capability knows of one attached world.
struct WorldLink {
    queue: Arc<ArrivalQueue>,
    set: Option<u64>,
    /// The assets a want of the world names, as last extracted.
    named: BTreeSet<AssetId>,
    /// Per asset answered, what was posted and the hash the manifest held
    /// for it then (none where the manifest did not hold it).
    posted: BTreeMap<AssetId, (Outcome, Option<u64>)>,
    /// The world waits for its first tick to apply the answers to a restore.
    awaiting_restore: bool,
    /// A move of the manifest landed meanwhile and is held for it.
    held: bool,
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
}

struct Instance {
    loose: Arc<LooseStore>,
    stores: Vec<Arc<dyn Store>>,
    files: FileThreads,
    residents: Arc<ResidentStore>,
    options: Options,
    revision: Domain,
    mounts: Vec<(String, Domain, bool)>,
    current: Domain,
    /// The manifests of the content sets the attached worlds name.
    sets: BTreeMap<u64, Domain>,
    worlds: BTreeMap<u32, WorldLink>,
    next_world: u32,
    report: LoaderReport,
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
        self.current = current;
        mounted
    }

    /// The manifest a world stands under.
    fn manifest_of(&self, set: Option<u64>) -> Domain {
        match set {
            Some(hash) if !self.options.current_manifest_for_sets => self.sets[&hash].clone(),
            _ => self.current.clone(),
        }
    }

    /// One asset of a closure: resolved in the manifest, looked up in the
    /// resident store, and on a miss read, verified, validated and checked
    /// against the dependencies the manifest lists for it.
    fn load(&mut self, manifest: &Domain, asset: AssetId) -> Outcome {
        let Some(entry) = manifest.get(&asset) else { return Outcome::Failure(REASON_UNKNOWN_ASSET) };
        let resident = match self.residents.get(entry.hash) {
            Some(resident) => resident,
            None => {
                let Some(bytes) = self.read(entry.hash) else { return Outcome::Failure(REASON_ARTIFACT_GONE) };
                match self.residents.insert(entry.hash, bytes) {
                    Ok(resident) => resident,
                    Err(_) => return Outcome::Failure(REASON_INVALID_PART),
                }
            }
        };
        if resident.part.dependencies != entry.dependencies {
            return Outcome::Failure(REASON_INVALID_PART);
        }
        Outcome::Hash(entry.hash)
    }

    /// Posts one batch to a world, leaving out a failure for an asset the
    /// world holds available: a replacement that cannot be put in force
    /// leaves what is in force, and is a report. An asset that left the
    /// manifest is a different event and does fail.
    fn post(&mut self, world: u32, manifest: &Domain, mut batch: Vec<Arrival>) {
        let link = self.worlds.get_mut(&world).expect("an attached world");
        batch.retain(|arrival| match (arrival.outcome, link.posted.get(&arrival.asset)) {
            (Outcome::Failure(code), Some((Outcome::Hash(_), _))) if code != REASON_UNKNOWN_ASSET && !arrival.marked => {
                self.report.refusals.push((arrival.asset, code));
                false
            }
            _ => true,
        });
        for arrival in &batch {
            link.posted.insert(arrival.asset, (arrival.outcome, manifest.get(&arrival.asset).map(|entry| entry.hash)));
        }
        if !batch.is_empty() {
            link.queue.post(&batch);
        }
    }

    /// Answers wants: the closure of the roots in the manifest the world
    /// stands under, as one complete batch.
    fn answer(&mut self, world: u32, roots: &[AssetId]) {
        if roots.is_empty() {
            return;
        }
        let manifest = self.manifest_of(self.worlds[&world].set);
        let batch = closure(&manifest, roots).into_iter().map(|asset| Arrival { asset, outcome: self.load(&manifest, asset), marked: false }).collect();
        self.post(world, &manifest, batch);
    }

    /// Answers a restore under the content at hand: one marked answer per
    /// asset the world's table holds as available under a want.
    fn answer_restore(&mut self, world: u32, held: &[(AssetId, u64)]) {
        let manifest = self.manifest_of(self.worlds[&world].set);
        let marked = !self.options.answers_unmarked;
        let mut batch = Vec::new();
        for (asset, _) in held {
            let outcome = self.load(&manifest, *asset);
            if self.options.no_answer_for_an_absent_asset && outcome == Outcome::Failure(REASON_UNKNOWN_ASSET) {
                continue;
            }
            batch.push(Arrival { asset: *asset, outcome, marked });
        }
        if !self.options.restore_answers_do_not_follow_the_closure {
            // An asset the table already names is answered, loaded as any
            // want, or stays failed, by its own rule; only an asset the table
            // does not name yet is posted here.
            let answered: Vec<AssetId> = batch.iter().map(|arrival| arrival.asset).collect();
            let named = self.worlds[&world].named.clone();
            for asset in closure(&manifest, &answered) {
                if !answered.contains(&asset) && !named.contains(&asset) {
                    batch.push(Arrival { asset, outcome: self.load(&manifest, asset), marked: false });
                }
            }
        }
        let link = self.worlds.get_mut(&world).expect("an attached world");
        link.awaiting_restore = !batch.is_empty();
        for arrival in &batch {
            link.posted.insert(arrival.asset, (arrival.outcome, manifest.get(&arrival.asset).map(|entry| entry.hash)));
        }
        if !batch.is_empty() {
            link.queue.post(&batch);
        }
    }

    /// The transitions a move of the manifest owes one world: one batch of
    /// the assets the world names whose entry moved, with the closure of
    /// what the batch changes.
    fn post_move(&mut self, world: u32) {
        let manifest = self.manifest_of(self.worlds[&world].set);
        let link = &self.worlds[&world];
        let moved: Vec<AssetId> = link
            .named
            .iter()
            .filter(|asset| link.posted.get(asset).is_some_and(|(_, resolved)| *resolved != manifest.get(asset).map(|entry| entry.hash)))
            .copied()
            .collect();
        if moved.is_empty() {
            return;
        }
        let answered: BTreeSet<AssetId> = link.posted.keys().copied().collect();
        let mut batch = Vec::new();
        for asset in closure(&manifest, &moved) {
            // An asset of the closure that the world already holds at the manifest's hash is not posted again.
            if !moved.contains(&asset) && answered.contains(&asset) {
                continue;
            }
            batch.push(Arrival { asset, outcome: self.load(&manifest, asset), marked: false });
        }
        self.post(world, &manifest, batch);
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
                link.held = true;
                self.report.held_moves += 1;
                continue;
            }
            self.post_move(world);
        }
    }

    fn extracted(&mut self, world: u32, extraction: Extraction) {
        let Some(link) = self.worlds.get_mut(&world) else { return };
        link.named = extraction.named.iter().copied().collect();
        link.posted.retain(|asset, _| link.named.contains(asset));
        let mut roots: BTreeSet<AssetId> = extraction.unanswered.iter().copied().collect();
        if !self.options.a_want_of_an_asset_in_force_is_not_answered {
            roots.extend(extraction.wanted.iter().copied());
        }
        let held = extraction.restore_applied && std::mem::take(&mut link.held);
        if extraction.restore_applied {
            link.awaiting_restore = false;
        }
        self.answer(world, &roots.into_iter().collect::<Vec<_>>());
        if held {
            self.post_move(world);
        }
    }
}

enum Message {
    Run(Box<dyn FnOnce(&mut Instance) + Send>),
    Stop,
}

/// What a host attaches a world with: where its arrivals are posted, the
/// content set it names, and what its table asks at the boundary where no
/// tick has ended.
pub struct Attachment {
    pub queue: Arc<ArrivalQueue>,
    pub set: Option<u64>,
    /// The wanted assets that are unanswered.
    pub unanswered: Vec<AssetId>,
    /// Every asset a want names.
    pub named: Vec<AssetId>,
    /// For a restore under the content at hand: the assets the table holds
    /// as available under a want, with the recorded hash.
    pub restore: Vec<(AssetId, u64)>,
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
            current: Domain::new(),
            sets: BTreeMap::new(),
            worlds: BTreeMap::new(),
            next_world: 0,
            report: LoaderReport::default(),
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
            if let Some(hash) = attachment.set
                && !instance.sets.contains_key(&hash)
            {
                let manifest = if set_hash(&instance.current) == hash {
                    instance.current.clone()
                } else {
                    let bytes = instance.read(hash).filter(|bytes| content_hash(bytes) == hash);
                    decode_domain(&bytes.ok_or_else(|| format!("the manifest of the content set {} cannot be had", hex(hash)))?)?
                };
                instance.sets.insert(hash, manifest);
            }
            let world = instance.next_world;
            instance.next_world += 1;
            let link = WorldLink {
                queue: attachment.queue,
                set: attachment.set,
                named: attachment.named.into_iter().collect(),
                posted: BTreeMap::new(),
                awaiting_restore: false,
                held: false,
            };
            instance.worlds.insert(world, link);
            instance.answer_restore(world, &attachment.restore);
            instance.answer(world, &attachment.unanswered);
            Ok(world)
        })?;
        let inbox = Mutex::new(self.inbox.clone());
        Ok(Arc::new(move |extraction: Extraction| {
            let run = move |instance: &mut Instance| instance.extracted(world, extraction);
            let _ = inbox.lock().expect("the inbox is never poisoned").send(Message::Run(Box::new(run)));
        }))
    }

    /// Waits until the capability has nothing left to post.
    pub fn settle(&self) {
        self.call(|_| ());
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
            Some(hash) => instance.sets.get(&hash).cloned().unwrap_or_default(),
            None => instance.current.clone(),
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
