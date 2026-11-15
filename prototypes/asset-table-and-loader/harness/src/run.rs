//! What the scenarios share: the settings of a run, the names of a family
//! and its content, and the verdict of a scenario against its oracle and its
//! controls. Throwaway, never product code.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asset_loader::ResidentStore;
use asset_pipeline::{Asked, Beside, Cache, Checks, Pass, Revision, Source};
use content_store::{content_hash, encode_domain, encode_root};
use prototype_core::forms::{PRODUCER, Preparer, local_platform, stamp};
use prototype_core::table::{AssetId, AssetType, Policy, RepulsionTable};
use prototype_core::world::{Parameters, Size};
use reference_workload::{DEFAULT_SEED, SplitMix64};

use crate::oracle::Names;
use crate::sequential::first_kill_ticks;

/// The identity of the composition's one contributor.
pub const BASE: u64 = 7;
/// The last tick any scenario with an oracle reaches.
pub const LAST_TICK: u32 = 24;
/// The bound of the admission check a sheet defines for the type of
/// parameters: the check refuses a part whose first parameter is greater.
/// In a family that states a content as above the bound, that content
/// carries a first parameter over it and every other content one within it.
pub const ADMISSION_BOUND: f32 = 96.0;

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
    /// The size the domain manifest holds for the content, where the family
    /// gives one that the length of the part does not give.
    pub held_as: Option<u64>,
    /// The version of the same asset whose parameters the content carries,
    /// where it differs from that version in its list of dependencies and
    /// in nothing else a world reads.
    pub parameters_of: Option<u32>,
    pub listed: &'static [(&'static str, AssetType)],
    /// The content stands above the bound of the admission check: the first
    /// parameter its part carries is greater than the bound.
    pub above_the_bound: bool,
    /// The size the prepared form built once of the content is padded to,
    /// where the family states a size for the form.
    pub form_size: Option<usize>,
    /// The revision number of the payload the asset has with this content,
    /// where it has one: the payload is made for the part of the content.
    pub payload: Option<u32>,
    /// The presentation dependencies of the asset with this content.
    pub presents: &'static [&'static str],
}

pub const fn content(asset: &'static str, version: u32) -> Content {
    Content { asset, version, asset_type: AssetType::Parameters, dependencies: &[], size: None, held_as: None, parameters_of: None, listed: &[], above_the_bound: false, form_size: None, payload: None, presents: &[] }
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

    /// The size the domain manifest holds for the content, where it is not
    /// the length its part is padded to: the stand-in pipeline writes it
    /// into the manifest, and the loading capability names it.
    pub const fn held_as(mut self, size: u64) -> Content {
        self.held_as = Some(size);
        self
    }

    /// The content carries the parameters of another version of its asset:
    /// it differs from it in the dependencies its part lists, and in its hash.
    pub const fn with_the_parameters_of(mut self, version: u32) -> Content {
        self.parameters_of = Some(version);
        self
    }

    pub const fn listed_as(mut self, listed: &'static [(&'static str, AssetType)]) -> Content {
        self.listed = listed;
        self
    }

    /// The content stands above the bound of the admission check, which
    /// refuses its part: a valid part, the bound standing inside the values
    /// the type's validator accepts.
    pub const fn above_the_bound(mut self) -> Content {
        self.above_the_bound = true;
        self
    }

    /// The size of the prepared form built once of the content, where the
    /// family states one: the stand-in preparing function pads the form to it.
    pub const fn with_a_form_of(mut self, size: usize) -> Content {
        self.form_size = Some(size);
        self
    }

    /// The asset has a payload with this content, made for its part, at the
    /// first revision of the payload.
    pub const fn with_a_payload(mut self) -> Content {
        self.payload = Some(1);
        self
    }

    /// The presentation dependencies of the asset with this content.
    pub const fn presenting(mut self, dependencies: &'static [&'static str]) -> Content {
        self.presents = dependencies;
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
    /// What the source of each content states beside its canonical part,
    /// for the actions that are not its canonical one: the size of its
    /// prepared form built once, its payload, its presentation dependencies.
    pub beside: BTreeMap<String, Beside>,
    /// The prepared forms built once that the compositions of the family
    /// declare, each with the asset type it is declared for: the stand-in
    /// pipeline runs a preparing action for each part of such a type. None
    /// in a family that has no such form.
    pub forms: Vec<(AssetType, Preparer)>,
    /// The target profiles a pass over the content of the family is asked a
    /// presentation manifest for: none in a family that has no payload.
    pub profiles: Vec<String>,
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
        let mut beside = BTreeMap::new();
        // The type an asset is imported under: the one its contents state.
        let type_of = |name: &str| contents.iter().find(|content| content.asset == name).map_or(AssetType::Parameters, |content| content.asset_type);
        // Where the family states a content as above the bound of the
        // admission check, every content stands on its side of the bound.
        let states_the_bound = contents.iter().any(|content| content.above_the_bound);
        for content in contents {
            let index = assets.iter().position(|name| *name == content.asset).expect("a content of a listed asset");
            // The first parameter of the part: of the content's own, and
            // moved over the bound where the family states the content so.
            let within = 24.0 + 8.0 * index as f32 + 2.5 * content.parameters_of.unwrap_or(content.version) as f32;
            let strength = if content.above_the_bound { ADMISSION_BOUND + within / 4.0 } else { within };
            assert!(!states_the_bound || content.above_the_bound == (strength > ADMISSION_BOUND), "the content {}.{} stands on the wrong side of the bound", content.asset, content.version);
            let source = Source {
                asset: identifiers[index],
                asset_type: content.asset_type,
                strength,
                radius: 8.0,
                salt: content.version,
                size: content.size,
                manifest_size: content.held_as,
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
            // The size the domain manifest gives the content: the length of
            // its part, but where the family gives another.
            names.sizes.insert(hash, content.held_as.unwrap_or(part.len() as u64));
            names.tables.insert(hash, RepulsionTable { strength: source.strength, radius: source.radius });
            parts.insert(name.clone(), part);
            // What the source states beside its canonical part: a payload
            // is made for the part of its own content.
            let stated = Beside {
                form_size: content.form_size,
                payload: content.payload.map(|revision| (name.clone(), revision)),
                presents: content.presents.iter().map(|other| names.assets[*other]).collect(),
            };
            beside.insert(name.clone(), stated);
            sources.insert(name, source);
        }
        names.pool = pool.iter().map(|name| names.assets[*name]).collect();
        names.instances = instances.iter().map(|(name, policy, reload)| ((*name).to_string(), *policy, *reload)).collect();
        let kills = first_kill_ticks(&settings.parameters(LAST_TICK + 2));
        let mut survivors = kills.iter().enumerate().filter(|(_, tick)| tick.is_none()).map(|(slot, _)| slot as u32);
        names.entities.insert("E1".to_string(), (survivors.next().expect("a slot survives"), 0));
        names.entities.insert("E2".to_string(), (survivors.next().expect("a second slot survives"), 0));
        Family { names, sources, parts, by_hand, beside, forms: Vec::new(), profiles: Vec::new() }
    }

    /// The compositions of the family declare these prepared forms built
    /// once, each for an asset type: every pass of the stand-in pipeline
    /// over the content of the family runs a preparing action for each part
    /// of such a type. The hashes of the forms are named after the part and
    /// after who built the form: `<asset>.<version>:built` is the form
    /// built once, the producer's, and `<asset>.<version>:local` the form
    /// the machine that runs the harness builds for itself from the same
    /// part, computed here, on this machine, under its local platform name.
    pub fn preparing(mut self, forms: &[(AssetType, Preparer)]) -> Family {
        self.forms = forms.to_vec();
        let local = local_platform();
        assert_ne!(local, PRODUCER, "the local platform name of a machine is never the producer's");
        for (name, source) in &self.sources {
            if !forms.iter().any(|(asset_type, preparer)| *asset_type == source.asset_type && preparer.built_once()) {
                continue;
            }
            for (who, platform) in [("built", PRODUCER), ("local", local.as_str())] {
                let form = stamp(&self.parts[name], platform, self.beside[name].form_size);
                let hash = content_hash(&form);
                self.names.hashes.insert(format!("{name}:{who}"), hash);
                self.names.sizes.insert(hash, form.len() as u64);
            }
        }
        self
    }

    /// A pass over the content of the family is asked a presentation
    /// manifest for this target profile.
    pub fn presenting_for(mut self, profile: &str) -> Family {
        self.profiles.push(profile.to_string());
        self
    }

    /// What a pass of the stand-in pipeline over the content of the family
    /// is asked beyond the canonical parts, under a platform name.
    pub fn asked(&self, platform: &str) -> Asked {
        Asked { forms: self.forms.clone(), platform: platform.to_string(), profiles: self.profiles.clone() }
    }

    /// The text of the source of a content: its canonical part's, then what
    /// it states beside it.
    pub fn source_text(&self, name: &str) -> String {
        self.sources[name].text() + &self.beside[name].text()
    }

    /// A content root imported into a loose store: one pass of the stand-in
    /// pipeline, which prepares the content built once, under the
    /// producer's platform name, where the family has a form built once.
    /// Returns the cache and the hash of the revision's manifest.
    pub fn import(&self, root: &Path, store: &Path) -> (Cache, u64) {
        let (cache, revision) = self.import_as(root, store, PRODUCER, asset_pipeline::Options::default());
        (cache, revision.domain_hash)
    }

    /// The same under a platform name: a machine's own build of the same
    /// sources is prepared under its local platform name.
    pub fn import_as(&self, root: &Path, store: &Path, platform: &str, options: asset_pipeline::Options) -> (Cache, Revision) {
        let cache = Cache::on_disk(store).expect("a loose store is created");
        let pass = asset_pipeline::pass_asked(root, &cache, Checks::default(), options, &self.asked(platform)).expect("the pass runs");
        assert!(pass.failures.is_empty(), "no source of a scenario fails its import: {:?}", pass.failures);
        (cache, pass.revision)
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
            let asset = name.split('.').next().expect("a hash is named <asset>.<version>");
            fs::write(root.join(format!("{asset}.asset")), self.source_text(name)).expect("a source is written");
        }
    }

    /// Writes the source of a content with its payload at another revision:
    /// the canonical part of the content does not change, and the payload,
    /// made for the same part, is another.
    pub fn write_source_with_its_payload_at(&self, root: &Path, name: &str, revision: u32) {
        let mut beside = self.beside[name].clone();
        assert!(beside.payload.is_some(), "the content {name} has a payload");
        beside.payload = Some((name.to_string(), revision));
        let asset = name.split('.').next().expect("a hash is named <asset>.<version>");
        fs::write(root.join(format!("{asset}.asset")), self.sources[name].text() + &beside.text()).expect("a source is written");
    }

    /// Writes the sources of the named contents with an error: the text each
    /// content gives, with the word `error` where the importer reads the
    /// first parameter. The importer reads no number there.
    pub fn write_sources_with_an_error(&self, root: &Path, contents: &[&str]) {
        fs::create_dir_all(root).expect("a content root is created");
        for name in contents {
            let erroneous = |line: &str| if line.starts_with("strength ") { "strength error".to_string() } else { line.to_string() };
            let text: String = self.source_text(name).lines().map(|line| erroneous(line) + "\n").collect();
            let asset = name.split('.').next().expect("a hash is named <asset>.<version>");
            fs::write(root.join(format!("{asset}.asset")), text).expect("a source is written");
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

/// One pass of the stand-in pipeline over a content root whose sources a
/// save changed, in the manner of a development host: an action of the pass
/// may fail, and what the pass does then is the pipeline's rule.
pub fn pass_of_a_save(root: &Path, store: &Path, options: asset_pipeline::Options) -> Pass {
    let cache = Cache::on_disk(store).expect("a loose store is created");
    asset_pipeline::pass_under(root, &cache, Checks::default(), options).expect("the pass runs")
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
    /// What the harness reports beside the oracle where a sheet asks for it,
    /// compared with nothing: of the stand-in pipeline after each save, and
    /// of the loading capability for each world.
    pub reports: Vec<String>,
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
