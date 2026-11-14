//! What the scenarios share: the settings of a run, the names of a family
//! and its content, and the verdict of a scenario against its oracle and its
//! controls. Throwaway, never product code.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asset_loader::ResidentStore;
use asset_pipeline::{Cache, Source};
use content_store::{content_hash, encode_domain, encode_root};
use prototype_core::table::{AssetId, AssetType, Policy, RepulsionTable};
use prototype_core::world::{Parameters, Size};
use reference_workload::{DEFAULT_SEED, SplitMix64};

use crate::oracle::Names;
use crate::sequential::first_kill_ticks;

/// The identity of the composition's one contributor.
pub const BASE: u64 = 7;
/// The last tick any scenario with an oracle reaches.
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
        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
        let directory = self.scratch.join(safe);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("a scratch directory is created");
        directory
    }
}

/// One content of one asset, as a family declares it: the asset, its
/// version, its type, the assets its part lists and, where the family gives
/// sizes, the size of its canonical part in bytes. A part lists each
/// dependency under the type the family's contents give that asset, but
/// where `listed` names another: such a part is one the stand-in pipeline
/// refuses to write, and the harness puts it in place itself.
#[derive(Clone, Copy)]
pub struct Content {
    pub asset: &'static str,
    pub version: u32,
    pub asset_type: AssetType,
    pub dependencies: &'static [&'static str],
    pub size: Option<usize>,
    pub listed: &'static [(&'static str, AssetType)],
}

pub const fn content(asset: &'static str, version: u32) -> Content {
    Content { asset, version, asset_type: AssetType::Parameters, dependencies: &[], size: None, listed: &[] }
}

impl Content {
    pub const fn of(mut self, asset_type: AssetType) -> Content {
        self.asset_type = asset_type;
        self
    }

    pub const fn listing(mut self, dependencies: &'static [&'static str]) -> Content {
        self.dependencies = dependencies;
        self
    }

    pub const fn sized(mut self, size: usize) -> Content {
        self.size = Some(size);
        self
    }

    pub const fn listed_as(mut self, listed: &'static [(&'static str, AssetType)]) -> Content {
        self.listed = listed;
        self
    }
}

/// The names of a family and the sources of its contents.
#[derive(Clone)]
pub struct Family {
    pub names: Names,
    /// The synthetic source of each content, by its hash's name: what the
    /// stand-in pipeline imports.
    pub sources: BTreeMap<String, Source>,
    /// The canonical part of each content: what the source's import writes,
    /// but for a part put in place by hand.
    pub parts: BTreeMap<String, Vec<u8>>,
    /// The contents whose part the pipeline would refuse.
    pub by_hand: Vec<String>,
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
        let (mut sources, mut parts, mut by_hand) = (BTreeMap::new(), BTreeMap::new(), Vec::new());
        // The type an asset is imported under: the one its contents state.
        let type_of = |name: &str| contents.iter().find(|content| content.asset == name).map_or(AssetType::Parameters, |content| content.asset_type);
        for content in contents {
            let index = assets.iter().position(|name| *name == content.asset).expect("a content of a listed asset");
            let source = Source {
                asset: identifiers[index],
                asset_type: content.asset_type,
                strength: 24.0 + 8.0 * index as f32 + 2.5 * content.version as f32,
                radius: 8.0,
                salt: content.version,
                size: content.size,
                dependencies: content.dependencies.iter().map(|name| (names.assets[*name], type_of(name))).collect(),
            };
            // The part as it stands in the store: the import's, or one that
            // lists a dependency under a type the manifest contradicts.
            let mut placed = source.clone();
            for (name, listed) in content.listed {
                let dependency = names.assets[*name];
                placed.dependencies.iter_mut().filter(|(other, _)| *other == dependency).for_each(|(_, asset_type)| *asset_type = *listed);
            }
            let part = placed.part();
            let hash = content_hash(&part);
            let name = format!("{}.{}", content.asset, content.version);
            if !content.listed.is_empty() {
                by_hand.push(name.clone());
            }
            names.hashes.insert(name.clone(), hash);
            names.tables.insert(hash, RepulsionTable { strength: source.strength, radius: source.radius });
            parts.insert(name.clone(), part);
            sources.insert(name, source);
        }
        names.pool = pool.iter().map(|name| names.assets[*name]).collect();
        names.instances = instances.iter().map(|(name, policy, reload)| ((*name).to_string(), *policy, *reload)).collect();
        let kills = first_kill_ticks(&settings.parameters(LAST_TICK + 2));
        let mut survivors = kills.iter().enumerate().filter(|(_, tick)| tick.is_none()).map(|(slot, _)| slot as u32);
        names.entities.insert("E1".to_string(), (survivors.next().expect("a slot survives"), 0));
        names.entities.insert("E2".to_string(), (survivors.next().expect("a second slot survives"), 0));
        Family { names, sources, parts, by_hand }
    }

    /// Makes every content of the family resident, for a scripted availability source.
    pub fn residents(&self) -> Arc<ResidentStore> {
        let store = Arc::new(ResidentStore::default());
        for part in self.parts.values() {
            store.insert(content_hash(part), part.clone()).expect("a part of the family is valid");
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

    /// Puts in the revision in place the parts the pipeline would refuse to
    /// write: the artifact under its own hash, and the manifest's entry of
    /// its asset naming that hash and that size, its list of dependencies
    /// left as the pipeline wrote it.
    pub fn place_by_hand(&self, cache: &Cache, contents: &[&str]) {
        let placed: Vec<&&str> = contents.iter().filter(|name| self.by_hand.iter().any(|other| other == **name)).collect();
        if placed.is_empty() {
            return;
        }
        let mut domain = cache.revision().expect("a revision is in place").domain;
        for name in placed {
            let part = &self.parts[*name];
            let entry = domain.get_mut(&self.sources[*name].asset).expect("the manifest holds the asset");
            entry.hash = cache.put_object(part).expect("the part is written");
            entry.size = part.len() as u64;
        }
        let domain_hash = cache.put_object(&encode_domain(&domain)).expect("the manifest is written");
        asset_pipeline::write_atomically(&cache.directory().join("root"), &encode_root(domain_hash)).expect("the revision is put in place");
    }

    pub fn hash(&self, name: &str) -> u64 {
        self.names.hashes[name]
    }
}

fn fnv_of(names: &[&str]) -> u64 {
    content_hash(names.join(",").as_bytes())
}

/// A content root imported into a loose store: one pass of the stand-in
/// pipeline. Returns the cache and the hash of the revision's manifest.
pub fn import(root: &Path, store: &Path) -> (Cache, u64) {
    let cache = Cache::on_disk(store).expect("a loose store is created");
    let pass = asset_pipeline::pass(root, &cache).expect("the pass runs");
    assert!(pass.failures.is_empty(), "no source of a scenario fails its import: {:?}", pass.failures);
    (cache, pass.revision.domain_hash)
}

/// What one world of one scenario gave at one worker count.
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
    /// What the run gave, in words, for the log.
    pub observed: Vec<String>,
}

/// A control, or a reading: the same scenario with one rule changed.
#[derive(Clone, Debug)]
pub struct Variant {
    pub scenario: String,
    pub name: String,
    /// A control must differ from the oracle; a reading is reported either way.
    pub control: bool,
    pub differs: bool,
    pub first_difference: String,
}

/// Everything the scenarios of one family gave.
#[derive(Clone, Debug, Default)]
pub struct Claim {
    pub name: String,
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
