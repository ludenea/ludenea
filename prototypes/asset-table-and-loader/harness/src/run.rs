//! What the scenarios share: the names of a family and its content, a world
//! driven tick by tick, and the verdict of a scenario against its oracle and
//! its controls. Throwaway, never product code.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use asset_loader::ResidentStore;
use asset_pipeline::{Cache, Source};
use content_store::content_hash;
use prototype_core::table::{ArrivalQueue, AssetId, Policy, Record, RepulsionTable, decode_log, encode_log};
use prototype_core::world::{Extraction, History, Links, Mode, Parameters, Size, Wait, World, WorldConfig};
use reference_workload::{DEFAULT_SEED, SplitMix64, run_hash};

use crate::oracle::{Element, Expected, Names, Observed, Verdict, compare, element, stated_entries};
use crate::sequential::{first_kill_ticks, oracle_with_kills};

/// The identity of the composition's one contributor.
pub const BASE: u64 = 7;
/// The last tick any scenario of this stage reaches.
pub const LAST_TICK: u32 = 24;

/// What a run is made with.
#[derive(Clone)]
pub struct Settings {
    pub size: Size,
    pub workers: Vec<usize>,
    /// A directory of this run's own, for stores, saves and content roots.
    pub scratch: PathBuf,
}

impl Settings {
    pub fn parameters(&self, ticks: u32) -> Parameters {
        Parameters { seed: DEFAULT_SEED, size: self.size, tick_count: ticks, wander: false }
    }

    pub fn directory(&self, name: &str) -> PathBuf {
        let directory = self.scratch.join(name.replace('/', "-"));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("a scratch directory is created");
        directory
    }
}

/// One content of one asset, as a family declares it: the asset, its
/// version, its type and the assets its part lists.
pub struct Content {
    pub asset: &'static str,
    pub version: u32,
    pub asset_type: u32,
    pub dependencies: &'static [&'static str],
}

pub const fn content(asset: &'static str, version: u32) -> Content {
    Content { asset, version, asset_type: 1, dependencies: &[] }
}

/// The names of a family and the sources of its contents.
#[derive(Clone)]
pub struct Family {
    pub names: Names,
    /// The synthetic source of each content, by its hash's name.
    pub sources: BTreeMap<String, Source>,
}

impl Family {
    /// Draws the identifiers from a fixed stream, so that their order is the
    /// order the family lists its assets in, gives every content parameters
    /// of its own, and names the two entities the workload never despawns
    /// itself during a scenario.
    pub fn new(assets: &[&str], pool: &[&str], contents: &[Content], instances: &[(&str, Policy, bool)], settings: &Settings) -> Family {
        let mut stream = SplitMix64::new(0x6173_7365_7473_0001 ^ fnv_of(assets));
        let mut identifiers: Vec<AssetId> = (0..assets.len()).map(|_| (u128::from(stream.next()) << 64) | u128::from(stream.next() | 1)).collect();
        identifiers.sort_unstable();
        let mut names = Names::default();
        for (name, identifier) in assets.iter().zip(&identifiers) {
            names.assets.insert((*name).to_string(), *identifier);
        }
        let mut sources = BTreeMap::new();
        for content in contents {
            let index = assets.iter().position(|name| *name == content.asset).expect("a content of a listed asset");
            let source = Source {
                asset: identifiers[index],
                asset_type: content.asset_type,
                strength: 24.0 + 8.0 * index as f32 + 2.5 * content.version as f32,
                radius: 8.0,
                salt: content.version,
                dependencies: content.dependencies.iter().map(|name| names.assets[*name]).collect(),
            };
            let hash = content_hash(&source.part());
            let name = format!("{}.{}", content.asset, content.version);
            names.hashes.insert(name.clone(), hash);
            names.tables.insert(hash, RepulsionTable { strength: source.strength, radius: source.radius });
            sources.insert(name, source);
        }
        names.pool = pool.iter().map(|name| names.assets[*name]).collect();
        names.instances = instances.iter().map(|(name, policy, reload)| ((*name).to_string(), *policy, *reload)).collect();
        let kills = first_kill_ticks(&settings.parameters(LAST_TICK + 2));
        let mut survivors = kills.iter().enumerate().filter(|(_, tick)| tick.is_none()).map(|(slot, _)| slot as u32);
        names.entities.insert("E1".to_string(), (survivors.next().expect("a slot survives"), 0));
        names.entities.insert("E2".to_string(), (survivors.next().expect("a second slot survives"), 0));
        Family { names, sources }
    }

    /// Makes every content of the family resident, for a scripted availability source.
    pub fn residents(&self) -> Arc<ResidentStore> {
        let store = Arc::new(ResidentStore::default());
        for source in self.sources.values() {
            let part = source.part();
            store.insert(content_hash(&part), part).expect("a part of the family is valid");
        }
        store
    }

    /// Writes the named contents as the sources of a content root, replacing
    /// the sources of the same assets, and leaves the others as they are.
    pub fn write_sources(&self, root: &Path, contents: &[&str]) {
        fs::create_dir_all(root).expect("a content root is created");
        for name in contents {
            let source = &self.sources[*name];
            let asset = name.split('.').next().expect("a hash is named <asset>.<version>");
            fs::write(root.join(format!("{asset}.asset")), source.text()).expect("a source is written");
        }
    }

    pub fn hash(&self, name: &str) -> u64 {
        self.names.hashes[name]
    }

    pub fn asset(&self, name: &str) -> AssetId {
        self.names.assets[name]
    }
}

fn fnv_of(names: &[&str]) -> u64 {
    content_hash(names.join(",").as_bytes())
}

/// A content root imported into a loose store: one pass of the stand-in pipeline.
pub fn import(root: &Path, store: &Path) -> (Cache, asset_pipeline::Revision) {
    let cache = Cache::on_disk(store).expect("a loose store is created");
    let pass = asset_pipeline::pass(root, &cache).expect("the pass runs");
    assert!(pass.failures.is_empty(), "no source of a scenario fails its import: {:?}", pass.failures);
    (cache, pass.revision)
}

/// The world configuration every scenario starts from.
pub fn config(settings: &Settings, workers: usize, names: &Names) -> WorldConfig {
    WorldConfig::new(settings.size, workers, Mode::Identifiers, names.pool.clone())
}

/// A slot for the function a world extracts its wants through, filled once
/// the availability source is attached, after the world exists.
pub fn late_extract() -> (Arc<dyn Fn(Extraction) + Send + Sync>, Arc<OnceLock<Arc<dyn Fn(Extraction) + Send + Sync>>>) {
    let slot: Arc<OnceLock<Arc<dyn Fn(Extraction) + Send + Sync>>> = Arc::new(OnceLock::new());
    let filled = slot.clone();
    let extract = move |extraction: Extraction| {
        if let Some(extract) = filled.get() {
            extract(extraction);
        }
    };
    (Arc::new(extract), slot)
}

/// What happens before a tick, in order.
pub enum Act {
    /// An element in the words of the scenario sheet: a command the host
    /// posts, or an arrival the scripted availability source posts.
    Post(&'static str),
    /// A step that asks for one tick and does not wait at a closed gate.
    StepWithoutWaiting,
    /// Something done outside the world: a revision, a mount, a removal.
    Call(Box<dyn Fn()>),
}

/// A world a scenario drives tick by tick, with what it observes of it.
pub struct Driven {
    pub world: World,
    pub queue: Arc<ArrivalQueue>,
    pub observed: Observed,
}

impl Driven {
    pub fn new(world: World, queue: Arc<ArrivalQueue>) -> Driven {
        let observed = Observed { first: world.tick(), creation: world.table_bytes(), ..Observed::default() };
        Driven { world, queue, observed }
    }

    fn step(&mut self, tick: u32, settle: &dyn Fn()) -> bool {
        settle();
        let result = self.world.step(1, Wait::No).expect("the world does not fail");
        for (asset, code) in &result.failed_required {
            self.observed.names_failed.push((tick, *asset, *code));
        }
        if result.ticks_ran == 0 {
            self.observed.stuck = Some((tick, result.unanswered));
            return false;
        }
        self.observed.stuck = None;
        self.observed.tables.push(self.world.table_bytes());
        true
    }

    /// Does what stands before tick `tick`, then runs it. Returns whether it
    /// ran: a world that stays gated is stuck there.
    pub fn tick(&mut self, tick: u32, acts: &[(u32, Act)], names: &Names, settle: &dyn Fn()) -> bool {
        for (_, act) in acts.iter().filter(|(before, _)| *before == tick) {
            match act {
                Act::Post(text) => match element(text, names).expect("an element of the scenario sheet") {
                    Element::Command(command) => self.world.post(command),
                    Element::Arrival(arrival) => self.queue.post(&[arrival]),
                },
                Act::Call(call) => call(),
                Act::StepWithoutWaiting => {
                    if self.world.tick() < tick && !self.step(tick, settle) {
                        let (_, awaited) = self.observed.stuck.take().expect("a step that ran no tick names what it awaits");
                        self.observed.blocked.push((tick, awaited));
                    }
                }
            }
        }
        self.world.tick() >= tick || self.step(tick, settle)
    }

    /// Drives the world to tick `to`, or to the tick it stays gated before.
    pub fn drive(&mut self, to: u32, acts: &[(u32, Act)], names: &Names, settle: &dyn Fn()) {
        for tick in self.world.tick() + 1..=to {
            if !self.tick(tick, acts, names, settle) {
                break;
            }
        }
        self.finish();
    }

    /// Reads what the run left: the hashes, the log, the events, the verdict of a restore.
    pub fn finish(&mut self) {
        self.observed.state = self.world.state_hashes();
        self.observed.log = self.world.log().to_vec();
        self.observed.events = self.world.events().to_vec();
        self.observed.verdict = match self.world.history() {
            History::Undecided => None,
            History::Continues => Some(Verdict::Continues),
            History::New(assets) => Some(Verdict::NewHistory(assets.clone())),
        };
    }
}

/// The workload's hashes of the sequential run an oracle's tables ask for.
/// `before` is what the oracle of the saved world states up to the tick a
/// restored world starts from, where there is one.
pub fn sequential(settings: &Settings, names: &Names, expected: &Expected, observed: &Observed, before: Option<&Expected>, to: u32) -> Vec<u64> {
    let creation = stated_entries(&observed.creation);
    let (mut switches, mut kills) = expected.schedule(names, observed.first + 1, to, &creation);
    if let Some(before) = before {
        let (earlier, earlier_kills) = before.schedule(names, 1, observed.first, &[]);
        switches.extend(earlier);
        kills.extend(earlier_kills);
    }
    oracle_with_kills(&settings.parameters(to), names.pool.len(), &switches, &kills)
}

/// Replays an input log, read back from its bytes, in a world built by
/// `world`, and returns its state hashes.
pub fn replay(log: &[Record], to: u32, world: impl FnOnce(Vec<Record>) -> World) -> Vec<u64> {
    let records = decode_log(&encode_log(log)).expect("a log the world wrote decodes");
    let mut world = world(records);
    let from = world.tick();
    world.step(to - from, Wait::No).expect("the replay does not fail");
    world.state_hashes()
}

/// What one scenario gave at one worker count.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub scenario: String,
    pub world: String,
    pub workers: usize,
    /// Every difference with the oracle, in words; none where the run equals it.
    pub differences: Vec<String>,
    /// The hash of the run's state hashes.
    pub run_hash: u64,
    /// Whether the input log, read back from its bytes, replays the run at
    /// another worker count; none where the scenario has no replay.
    pub replays: Option<bool>,
}

impl Outcome {
    pub fn new(scenario: &str, world: &str, workers: usize, observed: &Observed, differences: Vec<String>) -> Outcome {
        Outcome { scenario: scenario.to_string(), world: world.to_string(), workers, differences, run_hash: run_hash(&observed.state), replays: None }
    }
}

/// A control, or a reading: the same scenario with one rule changed.
#[derive(Clone, Debug)]
pub struct Variant {
    pub scenario: String,
    pub name: &'static str,
    /// A control must differ from the oracle; a reading is reported either way.
    pub control: bool,
    pub differs: bool,
    pub first_difference: String,
}

/// Everything the scenarios of one claim gave.
#[derive(Clone, Debug, Default)]
pub struct Claim {
    pub name: &'static str,
    pub outcomes: Vec<Outcome>,
    pub variants: Vec<Variant>,
    /// What a scenario observed beside its oracle, in words.
    pub notes: Vec<String>,
}

impl Claim {
    pub fn controls_differ(&self) -> bool {
        self.variants.iter().filter(|variant| variant.control).all(|variant| variant.differs)
    }

    /// One run hash per scenario and world across the worker counts.
    pub fn one_hash_per_scenario(&self) -> bool {
        let mut hashes: BTreeMap<(String, String), u64> = BTreeMap::new();
        self.outcomes.iter().all(|outcome| *hashes.entry((outcome.scenario.clone(), outcome.world.clone())).or_insert(outcome.run_hash) == outcome.run_hash)
    }
}

/// Compares a driven world with its oracle.
pub fn judge(settings: &Settings, names: &Names, expected: &Expected, observed: &Observed, before: Option<&Expected>, to: u32) -> Vec<String> {
    let hashes = sequential(settings, names, expected, observed, before, to);
    compare(expected, observed, to, &hashes, names)
}

/// A world with a scripted availability source: the parts are resident and
/// the scenario posts the arrivals itself.
pub fn scripted_world(config: WorldConfig, residents: &Arc<ResidentStore>, script: Vec<Record>) -> (World, Arc<ArrivalQueue>) {
    let links = Links::new(residents.clone());
    let queue = links.queue.clone();
    (World::new(config, script, links), queue)
}
