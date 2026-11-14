//! The content and the loading capability of the asset table spike, outside
//! the domain, copied and ported onto the prototype's table. Throwaway, never
//! product code.
//!
//! The catalog names the spike's assets: eight leaves that carry repulsion
//! parameters and that the entities name, two groups and two roots that list
//! dependencies (a root lists a group, a group lists leaves: a closure of
//! three levels), a root whose artifact is corrupt, a root nothing ever
//! answers, an asset no entity names and a large asset. Their identifiers are
//! 128-bit values drawn once from a fixed stream, so that every machine names
//! the same assets and identifier order is no authoring order. The pool of
//! assets the spike drew after them, for the cost of a handle, is not drawn:
//! that measurement is not run again.
//!
//! The loader is a thread that stands for the loading capability: it receives
//! the wanted assets a world extracts, follows their closure in its manifest,
//! takes a real delay, makes every canonical part resident and posts the
//! arrivals of the closure on the world's queue as one complete batch. It is
//! the spike's own loader and not the prototype's; the parts it makes
//! resident live in the prototype's resident store.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use asset_loader::ResidentStore;
use prototype_core::table::{
    Arrival, ArrivalQueue, AssetId, Outcome, Part, REASON_UNKNOWN_ASSET, RepulsionTable, Residents, part_bytes, part_len,
};
use reference_workload::{SplitMix64, fnv1a};

pub const LEAVES: usize = 8;

pub struct Catalog {
    pub leaf: [AssetId; LEAVES],
    pub group_a: AssetId,
    pub group_b: AssetId,
    pub root_a: AssetId,
    pub root_b: AssetId,
    /// A root whose artifact does not decode.
    pub root_c: AssetId,
    /// A root the loader never answers.
    pub root_d: AssetId,
    /// An asset no entity names.
    pub extra: AssetId,
    pub big: AssetId,
}

impl Catalog {
    pub fn new() -> Catalog {
        let mut stream = SplitMix64::new(0x6173_7365_7420_6964);
        let mut draw = || (u128::from(stream.next()) << 64) | u128::from(stream.next() | 1);
        Catalog {
            leaf: std::array::from_fn(|_| draw()),
            group_a: draw(),
            group_b: draw(),
            root_a: draw(),
            root_b: draw(),
            root_c: draw(),
            root_d: draw(),
            extra: draw(),
            big: draw(),
        }
    }

    /// A name for the report.
    pub fn name(&self, id: AssetId) -> String {
        if let Some(leaf) = self.leaf.iter().position(|other| *other == id) {
            return format!("leaf {leaf}");
        }
        let named = [
            (self.group_a, "group A"),
            (self.group_b, "group B"),
            (self.root_a, "root A"),
            (self.root_b, "root B"),
            (self.root_c, "the corrupt root"),
            (self.root_d, "the unanswered root"),
            (self.extra, "the asset no entity names"),
            (self.big, "the large asset"),
        ];
        named.iter().find(|(other, _)| *other == id).map_or_else(|| format!("{id:032x}"), |(_, name)| (*name).to_string())
    }

    /// The parameters of a leaf at a version: the specification's at version
    /// 1, then a strength of its own per leaf and version.
    pub fn leaf_table(version: u32, leaf: usize) -> RepulsionTable {
        let strength = match version {
            1 => 64.0,
            2 => 40.0 + 2.0 * leaf as f32,
            3 => 30.0 + leaf as f32,
            _ => 20.0 + leaf as f32,
        };
        RepulsionTable { strength, radius: 8.0 }
    }

    pub fn leaf_part(version: u32, leaf: usize) -> Vec<u8> {
        part_bytes(Catalog::leaf_table(version, leaf), &[], 0, 0)
    }

    pub fn leaf_hash(version: u32, leaf: usize) -> u64 {
        fnv1a(&Catalog::leaf_part(version, leaf))
    }

    /// Group A lists the leaves 0 to 3; group B the leaves 5 to 7; leaf 4 is in neither.
    pub fn group_a_part(&self) -> Vec<u8> {
        part_bytes(RepulsionTable::DEFAULT, &self.leaf[0..4], 0, 0)
    }

    pub fn group_b_part(&self) -> Vec<u8> {
        part_bytes(RepulsionTable::DEFAULT, &self.leaf[5..8], 0, 0)
    }

    pub fn root_part(&self, group: AssetId) -> Vec<u8> {
        part_bytes(RepulsionTable::DEFAULT, &[group], 0, 0)
    }

    pub fn extra_part(version: u32) -> Vec<u8> {
        part_bytes(RepulsionTable { strength: version as f32, radius: 8.0 }, &[], 0, 0)
    }

    pub fn big_part(payload_len: usize) -> Vec<u8> {
        part_bytes(RepulsionTable::SPECIFICATION, &[], payload_len, 0)
    }

    pub fn big_len(payload_len: usize) -> usize {
        part_len(0, payload_len)
    }

    /// The closure of root A at version 1, as arrivals: the root, its group, the leaves 0 to 3.
    pub fn closure_a(&self) -> Vec<Arrival> {
        let mut arrivals = vec![
            Arrival::at(self.root_a, fnv1a(&self.root_part(self.group_a))),
            Arrival::at(self.group_a, fnv1a(&self.group_a_part())),
        ];
        arrivals.extend((0..4).map(|leaf| Arrival::at(self.leaf[leaf], Catalog::leaf_hash(1, leaf))));
        arrivals
    }

    /// The closure of root B at version 1: the root, its group, the leaves 5 to 7.
    pub fn closure_b(&self) -> Vec<Arrival> {
        let mut arrivals = vec![
            Arrival::at(self.root_b, fnv1a(&self.root_part(self.group_b))),
            Arrival::at(self.group_b, fnv1a(&self.group_b_part())),
        ];
        arrivals.extend((5..8).map(|leaf| Arrival::at(self.leaf[leaf], Catalog::leaf_hash(1, leaf))));
        arrivals
    }

    /// The content set: every leaf at the version given (none: a corrupt
    /// artifact), the groups, the two roots, the corrupt root, the root that
    /// is never answered, and the asset no entity names.
    pub fn manifest(&self, leaf_versions: [Option<u32>; LEAVES]) -> Manifest {
        let mut manifest = Manifest::default();
        for (leaf, version) in leaf_versions.iter().enumerate() {
            match version {
                Some(version) => manifest.artifact(self.leaf[leaf], Catalog::leaf_part(*version, leaf)),
                None => manifest.corrupt(self.leaf[leaf], part_bytes(Catalog::leaf_table(1, leaf), &[], 8, 0xbad)),
            }
        }
        manifest.artifact(self.group_a, self.group_a_part());
        manifest.artifact(self.group_b, self.group_b_part());
        manifest.artifact(self.root_a, self.root_part(self.group_a));
        manifest.artifact(self.root_b, self.root_part(self.group_b));
        manifest.corrupt(self.root_c, part_bytes(RepulsionTable::DEFAULT, &[], 16, 0xbad));
        manifest.sources.insert(self.root_d, Source::Never);
        manifest.artifact(self.extra, Catalog::extra_part(1));
        manifest
    }
}

type Read = Arc<dyn Fn() -> Vec<u8> + Send + Sync>;

/// What a manifest holds per asset: the hash of its canonical part, its
/// dependencies, and how its artifact is read; or nothing that ever answers.
pub enum Source {
    Artifact { hash: u64, dependencies: Vec<AssetId>, read: Read },
    Never,
}

#[derive(Default)]
pub struct Manifest {
    pub sources: BTreeMap<AssetId, Source>,
}

impl Manifest {
    pub fn artifact(&mut self, id: AssetId, bytes: Vec<u8>) {
        let dependencies = Part::decode(&bytes).expect("a valid part").dependencies;
        self.sources.insert(id, Source::Artifact { hash: fnv1a(&bytes), dependencies, read: Arc::new(move || bytes.clone()) });
    }

    /// An artifact whose bytes on disk are cut short: the manifest names the
    /// hash of the whole part, the read returns less.
    pub fn corrupt(&mut self, id: AssetId, bytes: Vec<u8>) {
        let hash = fnv1a(&bytes);
        let cut = bytes[..bytes.len() - 3].to_vec();
        self.sources.insert(id, Source::Artifact { hash, dependencies: Vec::new(), read: Arc::new(move || cut.clone()) });
    }

    /// The domain closure of the wanted assets, from the dependencies the manifest holds.
    pub fn closure(&self, roots: &[AssetId]) -> BTreeSet<AssetId> {
        let mut closure = BTreeSet::new();
        let mut open = roots.to_vec();
        while let Some(id) = open.pop() {
            if closure.insert(id)
                && let Some(Source::Artifact { dependencies, .. }) = self.sources.get(&id)
            {
                open.extend_from_slice(dependencies);
            }
        }
        closure
    }

    /// Makes the canonical part of an asset resident and gives its arrival's
    /// outcome: the hash is looked up in the resident store first, and the
    /// artifact is read on a miss only. None where nothing ever answers. A
    /// part the store refuses fails with the reason code the store gives.
    pub fn load(&self, store: &ResidentStore, id: AssetId, reads: &AtomicUsize) -> Option<Outcome> {
        match self.sources.get(&id) {
            None => Some(Outcome::Failure(REASON_UNKNOWN_ASSET)),
            Some(Source::Never) => None,
            Some(Source::Artifact { hash, read, .. }) => {
                if store.get(*hash).is_some() {
                    return Some(Outcome::Hash(*hash));
                }
                reads.fetch_add(1, Ordering::Relaxed);
                Some(match store.insert(*hash, read()) {
                    Ok(_) => Outcome::Hash(*hash),
                    Err(code) => Outcome::Failure(code),
                })
            }
        }
    }
}

/// The real delay a load takes.
#[derive(Clone, Copy, Debug)]
pub enum Delay {
    Fixed(Duration),
    /// Drawn from the clock in this range of milliseconds, different between runs.
    Clock(u64, u64),
}

pub fn clock_nanos() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos() as u64)
}

impl Delay {
    fn draw(&self) -> Duration {
        match *self {
            Delay::Fixed(duration) => duration,
            Delay::Clock(low, high) => Duration::from_millis(low + clock_nanos() % (high - low + 1)),
        }
    }

    pub fn name(&self) -> String {
        match *self {
            Delay::Fixed(duration) => format!("{} ms", duration.as_millis()),
            Delay::Clock(low, high) => format!("clock, {low} to {high} ms"),
        }
    }
}

/// What reaches the loading capability.
pub enum Request {
    /// The wanted assets a world extracted as unanswered. The host hands
    /// them over: what the world's table asks at its creation, then what
    /// each tick's extraction names as newly unanswered.
    Wanted(Vec<AssetId>),
    /// The assets a save loaded as a new run holds as available, to answer.
    Restore(Vec<(AssetId, u64)>),
    /// A new revision of the content set: the parts that change, whose
    /// payloads land one by one in a random order over the spread, and are
    /// posted as one batch once every one of them is resident.
    Revision { parts: Vec<(AssetId, Vec<u8>)>, spread: Duration },
    Stop,
}

/// What the loader observed, for the report.
#[derive(Default)]
pub struct LoaderReport {
    pub batches: u32,
    pub delays_ms: Vec<f64>,
    /// The assets of the revision in the order their payloads landed, with the moment each did.
    pub landing: Vec<(AssetId, f64)>,
    /// The artifacts read: one per miss of the resident store.
    pub reads: usize,
}

pub struct Loader {
    pub requests: Sender<Request>,
    /// How many payloads of the revision are resident, and whether its batch is posted.
    pub landed: Arc<AtomicUsize>,
    pub posted: Arc<AtomicBool>,
    thread: JoinHandle<LoaderReport>,
}

impl Loader {
    pub fn spawn(manifest: Manifest, store: Arc<ResidentStore>, queue: Arc<ArrivalQueue>, delay: Delay) -> Loader {
        let (requests, inbox) = channel::<Request>();
        let landed = Arc::new(AtomicUsize::new(0));
        let posted = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let (landed_in, posted_in, reads_in) = (landed.clone(), posted.clone(), reads);
        let thread = thread::spawn(move || {
            let mut report = LoaderReport::default();
            for request in inbox {
                match request {
                    Request::Stop => break,
                    Request::Wanted(roots) => {
                        let closure = manifest.closure(&roots);
                        if closure.iter().any(|id| matches!(manifest.sources.get(id), Some(Source::Never))) {
                            continue;
                        }
                        let started = Instant::now();
                        thread::sleep(delay.draw());
                        let load = |id: &AssetId| manifest.load(&store, *id, &reads_in).map(|outcome| Arrival { asset: *id, outcome, marked: false });
                        let batch: Vec<Arrival> = closure.iter().filter_map(load).collect();
                        queue.post(&batch);
                        report.batches += 1;
                        report.delays_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
                    }
                    Request::Restore(held) => {
                        let started = Instant::now();
                        thread::sleep(delay.draw());
                        let answer = |(id, _): &(AssetId, u64)| manifest.load(&store, *id, &reads_in).map(|outcome| Arrival { asset: *id, outcome, marked: true });
                        let batch: Vec<Arrival> = held.iter().filter_map(answer).collect();
                        queue.post(&batch);
                        report.batches += 1;
                        report.delays_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
                    }
                    Request::Revision { parts, spread } => {
                        let mut stream = SplitMix64::new(clock_nanos());
                        let mut landings: Vec<(Duration, AssetId, Vec<u8>)> = parts
                            .into_iter()
                            .map(|(id, bytes)| (Duration::from_nanos(stream.next() % (spread.as_nanos() as u64 + 1)), id, bytes))
                            .collect();
                        landings.sort_by_key(|(offset, id, _)| (*offset, *id));
                        let started = Instant::now();
                        let mut batch = Vec::new();
                        for (offset, id, bytes) in landings {
                            thread::sleep(offset.saturating_sub(started.elapsed()));
                            let hash = fnv1a(&bytes);
                            store.insert(hash, bytes).expect("a valid part");
                            batch.push(Arrival::at(id, hash));
                            report.landing.push((id, started.elapsed().as_secs_f64() * 1_000.0));
                            landed_in.fetch_add(1, Ordering::SeqCst);
                        }
                        // No tick runs with half of a revision: the batch is
                        // posted once every part of it is resident.
                        queue.post(&batch);
                        report.batches += 1;
                        posted_in.store(true, Ordering::SeqCst);
                    }
                }
            }
            report.reads = reads_in.load(Ordering::Relaxed);
            report
        });
        Loader { requests, landed, posted, thread }
    }

    pub fn finish(self) -> LoaderReport {
        let _ = self.requests.send(Request::Stop);
        self.thread.join().expect("the loader ends")
    }
}
