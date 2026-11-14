//! The player of the scenarios. Throwaway, never product code.
//!
//! A scenario is the statement of a sheet as data: the worlds, what stands
//! before each tick in the sheet's own words, what moves the content, and
//! the controls. The player drives the worlds in rounds and observes them;
//! it reads no oracle. The judgment of what it observed against an oracle
//! is apart, so that a scenario can be driven with none.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, OnceLock};

use asset_loader::{Attachment, Hold, Loader, Options, ResidentStore};
use asset_pipeline::Cache;
use content_store::set_hash;
use prototype_core::table::{ArrivalQueue, AssetId, AssetType, Policy, Record, Residents, Rules, SystemName, decode_log, encode_log};
use prototype_core::world::{Act as SystemAct, Extraction, History, Links, Mode, Refusal, Restore, Size, System, Wait, World, WorldConfig, save_header};
use reference_workload::run_hash;

use crate::oracle::{Element, Expected, Observed, SCRIPTED_SYSTEM, SCRIPTED_SYSTEM_CONTRIBUTOR, Stated, Verdict, compare, element, observed_text, parse_each, stated_entries, system_act};
use crate::run::{BASE, Claim, Family, Outcome, Settings, Variant, import};
use crate::sequential::oracle_with_kills;

/// A change of the harness itself, for a control no rule of the build names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tweak {
    /// The hash of the content set is hashed with the state.
    SetUnderTheHash,
    /// A failed entry of the save is answered at the restore.
    AnswerTheFailedEntry(&'static str),
    /// A restore under the recorded content is made by the marked answers of
    /// a restore under the content at hand.
    AsContentAtHand,
    /// The type of an entry is read from the manifest at hand.
    TypesFromTheManifest,
    /// The entries of a type the composition lacks are dropped unnamed.
    DropInSilence,
}

/// A rule changed on purpose: of the core, of the loading capability, or of
/// the harness.
#[derive(Clone, Copy)]
pub enum Change {
    Core(fn(&mut Rules)),
    Loader(fn(&mut Options)),
    Harness(Tweak),
}

/// What stands before a tick, for one world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// An element in the words of the sheet: a command the host posts, or an
    /// arrival the scripted availability source posts.
    Post(&'static str),
    /// A step that asks for one tick and does not wait at a closed gate.
    Step,
    /// The same, before the loading capability has posted what the tick
    /// before asked of it.
    StepUnposted,
}

/// What moves the content of the first instance, before the round of a tick.
#[derive(Clone, Debug)]
pub enum Move {
    /// These contents replace those of the same assets, and the revision is
    /// put in place.
    Revision(Vec<&'static str>),
    /// The whole artifacts of these contents are put in the store, under the
    /// same hashes.
    Repair(Vec<&'static str>),
    /// The artifact of this content is removed from the loose store of
    /// every instance.
    Remove(&'static str),
    /// A package of these contents is mounted, with leave to override.
    Mount(Vec<&'static str>),
    Unmount,
}

/// The content set a world is created under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetName {
    None,
    /// The set of the first revision of the world's instance.
    First,
    /// The set of the manifest at hand when the world is created.
    AtHand,
}

/// One system of a world that makes wants: its stable name, the contributor
/// of the composition that brings it, and what it does per tick, in the
/// words of the sheet.
#[derive(Clone, Debug)]
pub struct SystemPlan {
    pub name: &'static str,
    pub contributor: &'static str,
    pub does: Vec<(u32, &'static str)>,
}

/// One world of a scenario.
#[derive(Clone)]
pub struct WorldPlan {
    pub name: String,
    /// The world of the oracle that states this one, where it is not its own.
    pub stated_as: Option<String>,
    /// Whether an oracle judges the world.
    pub judged: bool,
    pub instance: usize,
    /// The wants the composition's contributor makes at tick 0.
    pub wants: Vec<(&'static str, AssetType, Policy)>,
    /// The asset types the composition declares.
    pub types: Vec<AssetType>,
    pub set: SetName,
    /// The systems that make wants, in the order the composition lists them.
    pub systems: Vec<SystemPlan>,
    /// The budget of the world for its asset data, in bytes.
    pub budget: Option<u64>,
    pub acts: Vec<(u32, Act)>,
    pub to: u32,
    /// The world runs its pattern this many ticks later.
    pub offset: u32,
    /// The ticks after which the world is saved.
    pub saves: Vec<u32>,
    /// The world replays the input log of this other world, once it has run.
    pub replays_the_log_of: Option<String>,
}

impl WorldPlan {
    pub fn new(to: u32) -> WorldPlan {
        WorldPlan {
            name: "world".to_string(),
            stated_as: None,
            judged: true,
            instance: 0,
            wants: Vec::new(),
            types: vec![AssetType::Parameters, AssetType::Second],
            set: SetName::None,
            systems: Vec::new(),
            budget: None,
            acts: Vec::new(),
            to,
            offset: 0,
            saves: Vec::new(),
            replays_the_log_of: None,
        }
    }

    pub fn named(mut self, name: &str) -> WorldPlan {
        self.name = name.to_string();
        self
    }

    pub fn wanting(mut self, asset: &'static str, policy: Policy) -> WorldPlan {
        self.wants.push((asset, AssetType::Parameters, policy));
        self
    }

    pub fn declaring(mut self, types: &[AssetType]) -> WorldPlan {
        self.types = types.to_vec();
        self
    }

    pub fn under(mut self, set: SetName) -> WorldPlan {
        self.set = set;
        self
    }

    /// Elements of the record of a tick, in order.
    pub fn record(mut self, tick: u32, elements: &[&'static str]) -> WorldPlan {
        self.acts.extend(elements.iter().map(|text| (tick, Act::Post(text))));
        self
    }

    /// A step without waiting before a tick, after what was stated so far.
    pub fn step(mut self, tick: u32) -> WorldPlan {
        self.acts.push((tick, Act::Step));
        self
    }

    pub fn step_unposted(mut self, tick: u32) -> WorldPlan {
        self.acts.push((tick, Act::StepUnposted));
        self
    }

    /// What the one scripted system of a sheet that names none does in a tick.
    pub fn system(self, tick: u32, does: &[&'static str]) -> WorldPlan {
        self.system_of(SCRIPTED_SYSTEM, SCRIPTED_SYSTEM_CONTRIBUTOR, tick, does)
    }

    /// What a named system does in a tick. A system is listed, with the
    /// contributor that brings it, where the plan first names it.
    pub fn system_of(mut self, name: &'static str, contributor: &'static str, tick: u32, does: &[&'static str]) -> WorldPlan {
        if !self.systems.iter().any(|system| system.name == name) {
            self.systems.push(SystemPlan { name, contributor, does: Vec::new() });
        }
        let system = self.systems.iter_mut().find(|system| system.name == name).expect("the system is listed");
        system.does.extend(does.iter().map(|text| (tick, *text)));
        self
    }

    pub fn saved(mut self, ticks: &[u32]) -> WorldPlan {
        self.saves = ticks.to_vec();
        self
    }

    /// The instance the world stands on, where it is not the first.
    pub fn on(mut self, instance: usize) -> WorldPlan {
        self.instance = instance;
        self
    }

    /// A world no oracle states: the harness compares it with another.
    pub fn unjudged(mut self) -> WorldPlan {
        self.judged = false;
        self
    }

    /// The world replays the input log of another, whose statement is its own.
    pub fn replaying(mut self, other: &str) -> WorldPlan {
        self.replays_the_log_of = Some(other.to_string());
        self.stated_as = Some(other.to_string());
        self
    }

    pub fn with_a_budget(mut self, bytes: u64) -> WorldPlan {
        self.budget = Some(bytes);
        self
    }

    /// The world runs its pattern `offset` ticks later.
    pub fn later(mut self, offset: u32) -> WorldPlan {
        self.offset = offset;
        self
    }
}

/// The store a live loading capability answers from.
#[derive(Clone, Default)]
pub struct Store {
    /// The contents of the first revision.
    pub contents: Vec<&'static str>,
    /// The contents whose artifact is cut short under its name.
    pub cut: Vec<&'static str>,
    /// A later revision put in place before any world is created.
    pub then: Vec<&'static str>,
}

/// The save a reader loads, and the request it loads it by.
#[derive(Clone)]
pub struct Read {
    pub of: &'static str,
    pub at: u32,
    /// The request states that the content is the recorded one.
    pub recorded: bool,
    /// The asset types the request names as dropped.
    pub dropped: Vec<AssetType>,
}

/// What the harness compares beside the oracle.
#[derive(Clone, Copy, Debug)]
pub enum Check {
    /// Two worlds of the scenario have the same state hashes.
    SameState(&'static str, &'static str),
    /// The harness reports, tick by tick, whether the state hashes of two
    /// worlds are equal: a note, and no difference.
    ReportSameState(&'static str, &'static str),
    /// The reader's state hashes are those of the run that saved, from its
    /// first tick on.
    ContinuesTheSavedRun,
}

/// The same scenario with a rule changed: a control, which must differ from
/// its oracle, or a reading the build did not take, run beside it.
#[derive(Clone)]
pub struct VariantPlan {
    pub name: String,
    pub control: bool,
    pub changes: Vec<Change>,
    /// The worlds a control must differ on; any world where none is named.
    pub worlds: Vec<&'static str>,
}

#[derive(Clone)]
pub struct Scenario {
    pub name: String,
    /// The scenario of the oracle that judges it, where it is not its own
    /// name: one scenario run several ways.
    pub oracle: String,
    /// A live loading capability over this store; a scripted source without.
    pub live: Option<Store>,
    pub worlds: Vec<WorldPlan>,
    pub moves: Vec<(u32, Move)>,
    pub read: Option<Read>,
    /// What every run of the scenario changes: a candidate, a delay.
    pub base: Vec<Change>,
    /// The controls, which must differ from the oracle, and the readings.
    pub variants: Vec<VariantPlan>,
    pub checks: Vec<Check>,
}

impl Scenario {
    pub fn scripted(name: &str, world: WorldPlan) -> Scenario {
        Scenario {
            name: name.to_string(),
            oracle: name.to_string(),
            live: None,
            worlds: vec![world],
            moves: Vec::new(),
            read: None,
            base: Vec::new(),
            variants: Vec::new(),
            checks: Vec::new(),
        }
    }

    pub fn live(name: &str, contents: &[&'static str], world: WorldPlan) -> Scenario {
        let mut scenario = Scenario::scripted(name, world);
        scenario.live = Some(Store { contents: contents.to_vec(), ..Store::default() });
        scenario
    }

    pub fn cut(mut self, contents: &[&'static str]) -> Scenario {
        self.live.as_mut().expect("a live scenario").cut = contents.to_vec();
        self
    }

    pub fn also(mut self, world: WorldPlan) -> Scenario {
        self.worlds.push(world);
        self
    }

    pub fn moving(mut self, before: u32, change: Move) -> Scenario {
        self.moves.push((before, change));
        self
    }

    pub fn reading(mut self, of: &'static str, at: u32, recorded: bool, dropped: &[AssetType]) -> Scenario {
        self.read = Some(Read { of, at, recorded, dropped: dropped.to_vec() });
        self
    }

    pub fn checking(mut self, check: Check) -> Scenario {
        self.checks.push(check);
        self
    }

    /// What every run of the scenario changes.
    pub fn always(mut self, change: Change) -> Scenario {
        self.base.push(change);
        self
    }

    /// A control: the same scenario with a rule changed, which must differ.
    pub fn control(self, name: &str, change: Change) -> Scenario {
        self.control_on(&[], name, change)
    }

    /// A control that must differ on the worlds named.
    pub fn control_on(mut self, worlds: &[&'static str], name: &str, change: Change) -> Scenario {
        self.variants.push(VariantPlan { name: name.to_string(), control: true, changes: vec![change], worlds: worlds.to_vec() });
        self
    }

}

/// The save a reader is given, with the state hashes of the run that saved.
#[derive(Clone)]
pub struct Saved {
    pub bytes: Vec<u8>,
    pub hashes: Vec<u64>,
}

/// What one world of a scenario gave.
pub struct PlayedWorld {
    pub name: String,
    pub stated_as: String,
    pub judged: bool,
    pub to: u32,
    pub observed: Observed,
    pub replays: Option<bool>,
    pub saves: Vec<(u32, Vec<u8>)>,
    /// The entities the scripted system despawned: the tick and the slot.
    pub kills: Vec<(u32, usize)>,
}

#[derive(Default)]
pub struct Played {
    pub worlds: Vec<PlayedWorld>,
    pub notes: Vec<String>,
    /// Whether the header of a save of a world that replays a log under a
    /// set holds the set's hash.
    pub header_holds_the_set: Option<bool>,
}

struct Instance {
    directory: PathBuf,
    cache: Cache,
    loader: Loader,
    /// The set of the first revision.
    first: u64,
}

type Extract = Arc<dyn Fn(Extraction) + Send + Sync>;

/// A slot for the function a world extracts its wants through, filled once
/// the availability source is attached, after the world exists.
fn late_extract() -> (Extract, Arc<OnceLock<Extract>>) {
    let slot: Arc<OnceLock<Extract>> = Arc::new(OnceLock::new());
    let filled = slot.clone();
    let extract = move |extraction: Extraction| {
        if let Some(extract) = filled.get() {
            extract(extraction);
        }
    };
    (Arc::new(extract), slot)
}

struct Running {
    plan: WorldPlan,
    world: World,
    queue: Arc<ArrivalQueue>,
    set: Option<u64>,
    observed: Observed,
    hold: Option<Hold>,
    stopped: bool,
    saves: Vec<(u32, Vec<u8>)>,
}

impl Running {
    /// Asks the world for one tick without waiting. Returns what keeps the
    /// gate closed where no tick ran.
    fn advance(&mut self, tick: u32) -> Option<Vec<AssetId>> {
        let result = self.world.step(1, Wait::No).expect("the world does not fail");
        self.observed.names_failed.extend(result.failed_required.iter().map(|(asset, code)| (tick, *asset, *code)));
        self.observed.rejected.extend(result.rejected.iter().map(|(asset, hash)| (tick, *asset, *hash)));
        if result.ticks_ran == 0 {
            return Some(result.unanswered);
        }
        self.observed.tables.push(self.world.table_bytes());
        None
    }

    fn finish(&mut self) {
        self.observed.state = self.world.state_hashes();
        self.observed.log = self.world.log().to_vec();
        self.observed.events = self.world.events().to_vec();
        self.observed.refusals = self.world.refusals().to_vec();
        self.observed.verdict = match self.world.history() {
            History::Undecided => None,
            History::Continues => Some(Verdict::Continues),
            History::New(named) => Some(Verdict::NewHistory(named.clone())),
        };
    }
}

/// What a refusal of a restore names, as an oracle states it.
fn refused(refusal: &Refusal) -> Verdict {
    match refusal {
        Refusal::Content { asset, hash, .. } => Verdict::RefusedHash(*asset, *hash),
        Refusal::TypeLacking(named) | Refusal::TypeNotLacking(named) | Refusal::TypeWithoutAnEntry(named) => Verdict::RefusedType(*named),
        Refusal::TypeListedByAKeptEntry { kept, dropped } => Verdict::RefusedTypes(*dropped, *kept),
    }
}

/// A system of a plan as the world holds it: what it does read from the words
/// of the sheet, each tick moved by the offset of its world.
fn system_of(plan: &SystemPlan, offset: u32, names: &crate::oracle::Names) -> System {
    let act = |(tick, text): &(u32, &'static str)| (tick + offset, system_act(text, names).expect("what a system does, in the words of the sheet"));
    System {
        name: SystemName::new(plan.name),
        contributor: names.contributor(plan.contributor).expect("a system is brought by a contributor the sheets name"),
        does: plan.does.iter().map(act).collect(),
    }
}

/// Everything a play is made with.
struct Player<'a> {
    settings: &'a Settings,
    family: &'a Family,
    scenario: &'a Scenario,
    rules: Rules,
    tweaks: Vec<Tweak>,
    saved: Option<&'a Saved>,
}

impl Player<'_> {
    fn tweak(&self, tweak: Tweak) -> bool {
        self.tweaks.contains(&tweak)
    }

    fn configure(&self, plan: &WorldPlan, workers: usize, set: Option<u64>) -> WorldConfig {
        let names = &self.family.names;
        let wants: Vec<_> = plan.wants.iter().map(|(asset, asset_type, policy)| (names.assets[*asset], *asset_type, *policy)).collect();
        let mut config = WorldConfig::new(self.settings.size, workers, Mode::Identifiers, names.pool.clone()).wanting(BASE, &wants);
        config.asset_types = plan.types.clone();
        config.content_set = set;
        config.budget = plan.budget;
        config.systems = plan.systems.iter().map(|system| system_of(system, plan.offset, names)).collect();
        config.rules = self.rules;
        config.set_under_the_hash = self.tweak(Tweak::SetUnderTheHash);
        config
    }

    /// Makes the world of a plan: a new one, or the save a reader loads.
    fn make(&self, plan: &WorldPlan, workers: usize, set: Option<u64>, script: Vec<Record>, links: Links, loader: Option<&Loader>) -> Result<World, Refusal> {
        let config = self.configure(plan, workers, set);
        let Some(read) = &self.scenario.read else { return Ok(World::new(config, script, links)) };
        let saved = self.saved.expect("a reader is given the save it reads");
        let loader = loader.expect("a reader loads its save over a store");
        if read.recorded && !self.tweak(Tweak::AsContentAtHand) {
            let confirm = |asset: AssetId, hash: u64| loader.confirm(set, asset, hash);
            return World::restore(config, &saved.bytes, script, links, Restore::Continue { confirm: &confirm, dropped: &read.dropped });
        }
        let manifest = loader.manifest(None);
        let types_from = |asset: AssetId| manifest.get(&asset).and_then(|entry| AssetType::named(&entry.asset_type));
        let request = Restore::ContentAtHand {
            dropped: &read.dropped,
            drop_in_silence: self.tweak(Tweak::DropInSilence),
            types_from: if self.tweak(Tweak::TypesFromTheManifest) { Some(&types_from) } else { None },
        };
        World::restore(config, &saved.bytes, script, links, request)
    }
}

/// Plays a scenario at one worker count, with the changes of a variant.
/// `tag` tells apart the scratch directories of the plays of one scenario.
pub fn play(settings: &Settings, family: &Family, scenario: &Scenario, workers: usize, variant: &[Change], tag: &str, saved: Option<&Saved>) -> Played {
    let names = &family.names;
    let (mut rules, mut options, mut tweaks) = (Rules::default(), Options::default(), Vec::new());
    for change in scenario.base.iter().chain(variant) {
        match change {
            Change::Core(change) => change(&mut rules),
            Change::Loader(change) => change(&mut options),
            Change::Harness(tweak) => tweaks.push(*tweak),
        }
    }
    let player = Player { settings, family, scenario, rules, tweaks, saved };
    let mut notes = Vec::new();

    // The instances: each a loose store written by the stand-in pipeline and
    // a loading capability over it.
    let mut instances: Vec<Instance> = Vec::new();
    if let Some(store) = &scenario.live {
        let count = scenario.worlds.iter().map(|plan| plan.instance).max().unwrap_or(0) + 1;
        for index in 0..count {
            let directory = settings.directory(&format!("{}-{workers}-{tag}-{index}", scenario.name));
            family.write_sources(&directory.join("content"), &store.contents);
            let (cache, _) = import(&directory.join("content"), &directory.join("store"));
            family.place_by_hand(&cache, &store.contents);
            let first = cache.revision().expect("a revision is in place").domain_hash;
            for name in &store.cut {
                cache.cut_object(family.hash(name)).expect("the artifact to cut is in the store");
            }
            if index == 0 && !store.then.is_empty() {
                family.write_sources(&directory.join("content"), &store.then);
                import(&directory.join("content"), &directory.join("store"));
            }
            let loader = Loader::start(&directory.join("store"), options).expect("the loading capability starts");
            instances.push(Instance { directory, cache, loader, first });
        }
    }
    let scripted: Arc<ResidentStore> = family.residents();
    let residents_of = |instance: usize| -> Arc<dyn Residents> {
        match instances.get(instance) {
            Some(instance) => instance.loader.residents(),
            None => scripted.clone(),
        }
    };

    // The worlds, in the order the scenario lists them.
    let mut running: Vec<Running> = Vec::new();
    let mut refused_worlds: Vec<PlayedWorld> = Vec::new();
    for plan in scenario.worlds.iter().filter(|plan| plan.replays_the_log_of.is_none()) {
        let instance = instances.get(plan.instance);
        let set = match plan.set {
            SetName::None => None,
            SetName::First => Some(instance.expect("a set is the set of a store").first),
            SetName::AtHand => Some(set_hash(&instance.expect("a set is the set of a store").loader.manifest(None))),
        };
        let (extract, slot) = late_extract();
        let mut links = Links::new(residents_of(plan.instance));
        links.extract = instance.is_some().then_some(extract);
        let queue = links.queue.clone();
        let world = match player.make(plan, workers, set, Vec::new(), links, instance.map(|instance| &instance.loader)) {
            Ok(world) => world,
            Err(refusal) => {
                notes.push(format!("{}: the restore is refused: {refusal}", scenario.name));
                let first = scenario.read.as_ref().map_or(0, |read| read.at);
                let observed = Observed { first, verdict: Some(refused(&refusal)), ..Observed::default() };
                let stated_as = plan.stated_as.clone().unwrap_or_else(|| plan.name.clone());
                refused_worlds.push(PlayedWorld { name: plan.name.clone(), stated_as, judged: plan.judged, to: plan.to, observed, replays: None, saves: Vec::new(), kills: Vec::new() });
                continue;
            }
        };
        let observed = Observed {
            first: world.tick(),
            creation: world.table_bytes(),
            creation_entries: stated_entries(&world.typed_table_bytes()),
            ..Observed::default()
        };
        let mut world = Running { plan: plan.clone(), world, queue, set, observed, hold: None, stopped: false, saves: Vec::new() };
        // A reader steps once without waiting, before the availability
        // source has answered anything.
        if scenario.read.is_some() {
            let tick = world.observed.first + 1;
            match world.advance(tick) {
                Some(awaited) => world.observed.blocked.push((tick, awaited)),
                None => notes.push(format!("{}: the first tick ran before any answer", scenario.name)),
            }
        }
        if let Some(instance) = instance {
            // The query of the table at the boundary where no tick has
            // ended: the entries held available, which a restore under the
            // content at hand answers, and the wants whose asset is unanswered.
            let mut restore: Vec<AssetId> = world.world.restore_query().into_iter().map(|(asset, _)| asset).collect();
            for tweak in &player.tweaks {
                if let Tweak::AnswerTheFailedEntry(asset) = tweak {
                    restore.push(names.assets[*asset]);
                }
            }
            let standing = world.world.standing_wants();
            let attachment = Attachment { queue: world.queue.clone(), set, entries: world.world.entries(), unanswered: world.world.unanswered(), restore, standing };
            let _ = slot.set(instance.loader.attach(attachment).expect("the world is attached"));
        }
        running.push(world);
    }
    // The host waits until the capability has posted what the worlds asked
    // of it at their creation, the answers to a restore included, before
    // anything moves the content.
    for instance in &instances {
        instance.loader.settle();
    }

    // The rounds: before each, what moves the content, then the host waits
    // until the capability has nothing left to post; in it, every world is
    // stepped one tick in the order listed, without waiting between two.
    let last = scenario.worlds.iter().map(|plan| plan.to).max().unwrap_or(0);
    let first_round = running.iter().map(|world| world.observed.first).min().unwrap_or(0) + 1;
    let mut packages = 0;
    for tick in first_round..=last {
        for (_, change) in scenario.moves.iter().filter(|(before, _)| *before == tick) {
            let first = instances.first().expect("the content of a scripted scenario does not move");
            match change {
                Move::Revision(contents) => {
                    family.write_sources(&first.directory.join("content"), contents);
                    import(&first.directory.join("content"), &first.directory.join("store"));
                    first.loader.revision_in_place().expect("the revision is read");
                }
                Move::Repair(contents) => {
                    for name in contents {
                        let _ = first.cache.remove_object(family.hash(name));
                        first.cache.put_object(&family.parts[*name]).expect("the whole artifact is written");
                    }
                }
                Move::Remove(name) => {
                    for instance in &instances {
                        instance.cache.remove_object(family.hash(name)).expect("the artifact to remove is in the store");
                    }
                }
                Move::Mount(contents) => {
                    packages += 1;
                    let directory = first.directory.join(format!("package-{packages}"));
                    family.write_sources(&directory.join("content"), contents);
                    let (cache, _) = import(&directory.join("content"), &directory.join("store"));
                    let domain = cache.revision().expect("the package's revision is in place").domain;
                    let entries = domain.keys().copied().collect();
                    let file = asset_pipeline::package(&cache, &domain, &entries).expect("the package is built");
                    first.loader.mount("package", file, true).expect("the package mounts");
                }
                Move::Unmount => first.loader.unmount("package").expect("the package unmounts"),
            }
        }
        for (index, instance) in instances.iter().enumerate() {
            if !running.iter().any(|world| world.plan.instance == index && world.hold.is_some()) {
                instance.loader.settle();
            }
        }
        for world in running.iter_mut() {
            if world.stopped || tick > world.plan.to || tick <= world.observed.first {
                continue;
            }
            let offset = world.plan.offset;
            let acts: Vec<Act> = world.plan.acts.iter().filter(|(before, _)| before + offset == tick).map(|(_, act)| *act).collect();
            for act in acts {
                match act {
                    Act::Post(text) => match element(text, names).expect("an element of a scenario sheet") {
                        Element::Command(command) => world.world.post(command),
                        Element::Arrival(arrival) => world.queue.post(&[arrival]),
                    },
                    Act::Step | Act::StepUnposted => {
                        if world.world.tick() < tick
                            && let Some(awaited) = world.advance(tick)
                        {
                            world.observed.blocked.push((tick, awaited));
                        }
                        // The capability goes on, and the host waits for it.
                        if world.hold.take().is_some() {
                            instances[world.plan.instance].loader.settle();
                        }
                    }
                }
            }
            if world.world.tick() < tick {
                // A step before the capability has posted follows this tick:
                // the capability is held still while the tick runs.
                let unposted = world.plan.acts.iter().any(|(before, act)| before + offset == tick + 1 && *act == Act::StepUnposted);
                if let (true, Some(instance)) = (unposted, instances.get(world.plan.instance)) {
                    world.hold = Some(instance.loader.hold());
                }
                if let Some(awaited) = world.advance(tick) {
                    world.observed.stuck = Some((tick, awaited));
                    world.stopped = true;
                    world.hold = None;
                }
            }
            if world.plan.saves.contains(&tick) && world.world.tick() == tick {
                world.saves.push((tick, world.world.save()));
            }
        }
    }
    for instance in &instances {
        instance.loader.settle();
    }

    // What each world left, and the replay of its input log, read back from
    // its bytes, at another worker count.
    let other = if workers == 1 { 4 } else { 1 };
    let mut played: Vec<PlayedWorld> = Vec::new();
    let mut header_holds_the_set = None;
    for mut world in running {
        world.hold = None;
        world.finish();
        let plan = &world.plan;
        let replays = world.observed.stuck.is_none().then(|| {
            let records = decode_log(&encode_log(&world.observed.log)).expect("a log the world wrote decodes");
            let links = Links::new(residents_of(plan.instance));
            let loader = instances.get(plan.instance).map(|instance| &instance.loader);
            let mut replayed = player.make(plan, other, world.set, records, links, loader).expect("the world is made again");
            let from = replayed.tick();
            replayed.step(plan.to - from, Wait::No).expect("the replay does not fail");
            replayed.state_hashes() == world.observed.state
        });
        let despawns = |system: &SystemPlan| system_of(system, plan.offset, names).does;
        let kills = plan
            .systems
            .iter()
            .flat_map(despawns)
            .filter_map(|(tick, act)| match act {
                SystemAct::Despawn { slot, .. } => Some((tick, slot as usize)),
                _ => None,
            })
            .collect();
        let stated_as = plan.stated_as.clone().unwrap_or_else(|| plan.name.clone());
        played.push(PlayedWorld { name: plan.name.clone(), stated_as, judged: plan.judged, to: plan.to, observed: world.observed, replays, saves: world.saves, kills });
    }
    played.extend(refused_worlds);

    // A world that replays the input log of another, under the set.
    for plan in scenario.worlds.iter().filter(|plan| plan.replays_the_log_of.is_some()) {
        let source = plan.replays_the_log_of.as_ref().expect("filtered");
        let log = played.iter().find(|world| world.name == *source).expect("the world whose log is replayed ran").observed.log.clone();
        let instance = instances.get(plan.instance).expect("a set is the set of a store");
        let set = Some(instance.first);
        let links = Links::new(residents_of(plan.instance));
        let queue = links.queue.clone();
        let world = World::new(player.configure(plan, other, set), log, links);
        let observed = Observed { first: 0, creation: world.table_bytes(), creation_entries: stated_entries(&world.typed_table_bytes()), ..Observed::default() };
        let mut world = Running { plan: plan.clone(), world, queue, set, observed, hold: None, stopped: false, saves: Vec::new() };
        for tick in 1..=plan.to {
            if let Some(awaited) = world.advance(tick) {
                world.observed.stuck = Some((tick, awaited));
                break;
            }
        }
        world.finish();
        header_holds_the_set = Some(save_header(&world.world.save()).is_ok_and(|header| header.content_set == set));
        let stated_as = plan.stated_as.clone().unwrap_or_else(|| plan.name.clone());
        played.push(PlayedWorld { name: plan.name.clone(), stated_as, judged: plan.judged, to: plan.to, observed: world.observed, replays: None, saves: Vec::new(), kills: Vec::new() });
    }
    Played { worlds: played, notes, header_holds_the_set }
}

/// What an oracle states of the world that saved, for the history before a
/// reader's first tick.
pub struct Before<'a> {
    pub expected: &'a Expected,
    pub saved: &'a Saved,
}

/// Judges what a play observed against the oracle of its scenario. Returns,
/// per judged world, every difference in words.
pub fn judge(settings: &Settings, family: &Family, scenario: &Scenario, played: &Played, stated: &Stated, before: Option<&Before>) -> Vec<(String, Vec<String>)> {
    let names = &family.names;
    let mut judged: Vec<(String, Vec<String>)> = Vec::new();
    for world in played.worlds.iter().filter(|world| world.judged) {
        let Some(expected) = stated.get(&scenario.oracle).and_then(|worlds| worlds.get(&world.stated_as)) else {
            judged.push((world.name.clone(), vec![format!("the oracle states nothing of the world {} of {}", world.stated_as, scenario.oracle)]));
            continue;
        };
        let observed = &world.observed;
        if observed.verdict.as_ref().is_some_and(Verdict::refused) {
            judged.push((world.name.clone(), compare(expected, observed, world.to, &[], names)));
            continue;
        }
        let (mut switches, mut kills) = expected.schedule(names, observed.first + 1, world.to, &observed.creation_entries);
        kills.extend_from_slice(&world.kills);
        if let Some(before) = before {
            let (earlier, earlier_kills) = before.expected.schedule(names, 1, observed.first, &[]);
            switches.extend(earlier);
            kills.extend(earlier_kills);
        }
        let hashes = oracle_with_kills(&settings.parameters(world.to), names.pool.len(), &switches, &kills);
        judged.push((world.name.clone(), compare(expected, observed, world.to, &hashes, names)));
    }
    let state = |name: &str| played.worlds.iter().find(|world| world.name == name).map(|world| world.observed.state.clone());
    let mut beside = Vec::new();
    for check in &scenario.checks {
        match check {
            Check::SameState(one, other) => {
                if state(one) != state(other) {
                    beside.push(format!("the state hashes of the world {one} are not those of the world {other}"));
                }
            }
            // A report, and no difference: the notes of the play hold it.
            Check::ReportSameState(_, _) => {}
            Check::ContinuesTheSavedRun => {
                let Some(saved) = before.map(|before| before.saved) else {
                    beside.push("the oracle states nothing of the run that saved".to_string());
                    continue;
                };
                for world in played.worlds.iter().filter(|world| world.judged) {
                    let from = world.observed.first as usize;
                    if saved.hashes.get(from..) != Some(world.observed.state.as_slice()) {
                        beside.push(format!("the state hashes are not those of the run that saved, from tick {} on", from + 1));
                    }
                }
            }
        }
    }
    if played.header_holds_the_set == Some(false) {
        beside.push("the header of a save of the world under the set does not hold the set's hash".to_string());
    }
    if let Some((_, differences)) = judged.first_mut() {
        differences.extend(beside);
    }
    judged
}

/// A family: its content, its scenarios in the order they run, and the
/// text of its oracle file.
pub struct FamilyPlan {
    pub name: &'static str,
    pub claim: &'static str,
    pub family: Family,
    pub scenarios: Vec<Scenario>,
    pub oracle: &'static str,
}

fn save_path(settings: &Settings, scenario: &str, tick: u32, workers: usize) -> PathBuf {
    let safe: String = scenario.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    settings.scratch.join("saves").join(format!("{safe}-{tick}-{workers}.save"))
}

fn write_saved(path: &Path, saved: &Saved) {
    fs::create_dir_all(path.parent().expect("a save stands in a directory")).expect("the directory of the saves is created");
    fs::write(path, &saved.bytes).expect("the save is written");
    let hashes: String = saved.hashes.iter().map(|hash| format!("{hash:016x}\n")).collect();
    fs::write(path.with_extension("hashes"), hashes).expect("the hashes are written");
}

fn read_saved(path: &Path) -> Saved {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("the save {} is read: {error}", path.display()));
    let hashes = fs::read_to_string(path.with_extension("hashes")).expect("the hashes are read");
    Saved { bytes, hashes: hashes.lines().filter_map(|line| u64::from_str_radix(line, 16).ok()).collect() }
}

/// What one world of one play gave, once judged.
#[derive(Default)]
struct JudgedWorld {
    name: String,
    differences: Vec<String>,
    run_hash: u64,
    replays: Option<bool>,
    observed: Vec<String>,
}

/// What one play of one scenario gave, once judged.
#[derive(Default)]
struct Judged {
    worlds: Vec<JudgedWorld>,
    notes: Vec<String>,
}

/// The save a reader loads. The run of the scenario as it stands reads the
/// save its saver wrote; the build changed is the build of every process, so
/// under a change the run that saved runs under it too. None where that run
/// never reached its save.
fn saved_for(settings: &Settings, plan: &FamilyPlan, read: &Read, workers: usize, variant: &[Change], tag: &str) -> Option<Saved> {
    if variant.is_empty() {
        return Some(read_saved(&save_path(settings, read.of, read.at, workers)));
    }
    let saver = plan.scenarios.iter().find(|other| other.name == read.of).expect("the scenario that saved is of the family");
    let played = play(settings, &plan.family, saver, workers, variant, &format!("{tag}-saver"), None);
    let world = played.worlds.iter().find(|world| world.judged)?;
    let (_, bytes) = world.saves.iter().find(|(tick, _)| *tick == read.at)?;
    Some(Saved { bytes: bytes.clone(), hashes: world.observed.state.clone() })
}

/// Plays one scenario and judges it, in this process. Without an oracle the
/// scenario is driven and nothing is compared.
fn play_and_judge(settings: &Settings, plan: &FamilyPlan, scenario: &Scenario, workers: usize, variant: &[Change], tag: &str, stated: Option<&Stated>) -> (Judged, Played) {
    let saved = match &scenario.read {
        None => None,
        Some(read) => match saved_for(settings, plan, read, workers, variant, tag) {
            Some(saved) => Some(saved),
            None => {
                let world = JudgedWorld { name: "world".to_string(), differences: vec!["under this change the run that saved never reached its save".to_string()], ..JudgedWorld::default() };
                return (Judged { worlds: vec![world], notes: Vec::new() }, Played::default());
            }
        },
    };
    let played = play(settings, &plan.family, scenario, workers, variant, tag, saved.as_ref());
    let mut judged = Judged { notes: played.notes.clone(), ..Judged::default() };
    let differences: BTreeMap<String, Vec<String>> = match stated {
        Some(stated) => {
            let saver = scenario.read.as_ref().and_then(|read| stated.get(read.of)).and_then(|worlds| worlds.get("world"));
            let before = saver.zip(saved.as_ref()).map(|(expected, saved)| Before { expected, saved });
            judge(settings, &plan.family, scenario, &played, stated, before.as_ref()).into_iter().collect()
        }
        None => BTreeMap::new(),
    };
    for check in &scenario.checks {
        let Check::ReportSameState(one, other) = check else { continue };
        let state = |name: &str| played.worlds.iter().find(|world| world.name == name).map(|world| world.observed.state.clone()).unwrap_or_default();
        let (one_state, other_state) = (state(one), state(other));
        let differing: Vec<String> = (0..one_state.len().max(other_state.len())).filter(|tick| one_state.get(*tick) != other_state.get(*tick)).map(|tick| tick.to_string()).collect();
        let text = if differing.is_empty() { "are equal at every tick".to_string() } else { format!("differ at the ticks {}", differing.join(", ")) };
        judged.notes.push(format!("{}: the state hashes of the worlds {one} and {other} {text}", scenario.name));
    }
    for world in played.worlds.iter().filter(|world| world.judged) {
        judged.worlds.push(JudgedWorld {
            name: world.name.clone(),
            differences: differences.get(&world.name).cloned().unwrap_or_default(),
            run_hash: if world.observed.state.is_empty() { 0 } else { run_hash(&world.observed.state) },
            replays: world.replays,
            observed: observed_text(&world.observed, &plan.family.names),
        });
    }
    (judged, played)
}

/// The changes of a control or of a reading, by its name: one of the
/// scenario's own, or one of the readings the harness runs on a scenario
/// that differs.
fn variant_named(scenario: &Scenario, readings: &[VariantPlan], name: &str) -> Vec<Change> {
    if name.is_empty() {
        return Vec::new();
    }
    let named = scenario.variants.iter().chain(readings).find(|variant| variant.name == name).expect("a control or a reading of the scenario");
    named.changes.clone()
}

/// The reader of a save as a process of its own:
/// `--reader <family> <scenario> <workers> <scratch> <size> <variant>`.
/// It prints what it observed, a line per fact.
pub fn child(arguments: &[String], family: &dyn Fn(&Settings, &str) -> Option<FamilyPlan>, readings: &[VariantPlan]) -> String {
    let [name, scenario, workers, scratch, size, variant] = arguments else { return "difference world\tthe reader was not given its six arguments\n".to_string() };
    let size = if size == "small" { Size::Small } else { Size::Default };
    let settings = Settings { size, workers: Vec::new(), scratch: Path::new(scratch).to_path_buf() };
    let plan = family(&settings, name).expect("a family of the sheets");
    let scenario = plan.scenarios.iter().find(|other| other.name == *scenario).expect("a scenario of the family");
    let (stated, _) = parse_each(plan.oracle, &plan.family.names);
    let changes = variant_named(scenario, readings, variant);
    let tag = if variant.is_empty() { "run".to_string() } else { "variant".to_string() };
    let (judged, _) = play_and_judge(&settings, &plan, scenario, workers.parse().expect("a worker count"), &changes, &tag, Some(&stated));
    let mut out = String::new();
    for world in &judged.worlds {
        for difference in &world.differences {
            out.push_str(&format!("difference {}\t{difference}\n", world.name));
        }
        out.push_str(&format!("hash {}\t{:016x}\n", world.name, world.run_hash));
        if let Some(replays) = world.replays {
            out.push_str(&format!("replays {}\t{replays}\n", world.name));
        }
        for line in &world.observed {
            out.push_str(&format!("observed {}\t{line}\n", world.name));
        }
    }
    for note in &judged.notes {
        out.push_str(&format!("note {note}\n"));
    }
    out
}

/// Runs one reader as a process of its own and reads what it prints.
fn read_in_a_process(settings: &Settings, plan: &FamilyPlan, scenario: &Scenario, workers: usize, variant: &str) -> Judged {
    let exe = std::env::current_exe().expect("the harness knows its own path");
    let size = settings.size.name();
    let output = Process::new(exe)
        .args(["--reader", plan.name, &scenario.name, &workers.to_string(), &settings.scratch.to_string_lossy(), size, variant])
        .output()
        .expect("a reader starts");
    let mut judged = Judged::default();
    let mut worlds: BTreeMap<String, JudgedWorld> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((kind, rest)) = line.split_once(' ') else { continue };
        if kind == "note" {
            judged.notes.push(rest.to_string());
            continue;
        }
        let Some((world, text)) = rest.split_once('\t') else { continue };
        let entry = worlds.entry(world.to_string()).or_insert_with(|| JudgedWorld { name: world.to_string(), ..JudgedWorld::default() });
        match kind {
            "difference" => entry.differences.push(text.to_string()),
            "hash" => entry.run_hash = u64::from_str_radix(text, 16).unwrap_or(0),
            "replays" => entry.replays = Some(text == "true"),
            "observed" => entry.observed.push(text.to_string()),
            _ => {}
        }
    }
    if !output.status.success() || worlds.is_empty() {
        let error = String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("").to_string();
        let entry = worlds.entry("world".to_string()).or_insert_with(|| JudgedWorld { name: "world".to_string(), ..JudgedWorld::default() });
        entry.differences.push(format!("the reader ended with {}: {error}", output.status));
    }
    judged.worlds = worlds.into_values().collect();
    judged
}

/// Runs a family: each scenario at every worker count, then each of its
/// controls at one worker count and, where the scenario differs from its
/// oracle or where `every_reading` is set, each of the readings the build
/// did not take. A reader of a save
/// runs as a process of its own where `processes` is set. Without an oracle,
/// every scenario and every control is driven and nothing is compared.
pub fn run_family(settings: &Settings, plan: &FamilyPlan, readings: &[VariantPlan], every_reading: bool, processes: bool, against_the_oracle: bool) -> Claim {
    let mut claim = Claim { name: format!("{}: {}", plan.name, plan.claim), ..Claim::default() };
    let stated = against_the_oracle.then(|| {
        let (stated, unread) = parse_each(plan.oracle, &plan.family.names);
        claim.notes.extend(unread);
        stated
    });
    let variant_workers = if settings.workers.contains(&4) { 4 } else { settings.workers[0] };
    for scenario in &plan.scenarios {
        let as_a_process = processes && scenario.read.is_some() && stated.is_some();
        let mut differs = false;
        for &workers in &settings.workers {
            let judged = if as_a_process {
                read_in_a_process(settings, plan, scenario, workers, "")
            } else {
                let (judged, played) = play_and_judge(settings, plan, scenario, workers, &[], "run", stated.as_ref());
                for world in &played.worlds {
                    for (tick, bytes) in &world.saves {
                        write_saved(&save_path(settings, &scenario.name, *tick, workers), &Saved { bytes: bytes.clone(), hashes: world.observed.state.clone() });
                    }
                    if stated.is_none() && world.judged {
                        // Driven with no oracle: a world that stays gated,
                        // or a log that does not replay, is all that shows.
                        if let Some((tick, _)) = &world.observed.stuck {
                            claim.notes.push(format!("{} [{}] at {workers} workers: the world never ran tick {tick}", scenario.name, world.name));
                        }
                        if world.replays == Some(false) {
                            claim.notes.push(format!("{} [{}] at {workers} workers: the log does not replay", scenario.name, world.name));
                        }
                    }
                }
                judged
            };
            let first = workers == settings.workers[0];
            for world in judged.worlds {
                differs |= !world.differences.is_empty() || world.replays == Some(false);
                let observed = if first { world.observed } else { Vec::new() };
                claim.outcomes.push(Outcome { scenario: scenario.name.clone(), world: world.name, workers, differences: world.differences, run_hash: world.run_hash, replays: world.replays, observed });
            }
            if first {
                claim.notes.extend(judged.notes);
            }
        }
        if stated.is_none() {
            // Every control is driven, so that none fails the harness itself.
            for (index, variant) in scenario.variants.iter().enumerate() {
                play_and_judge(settings, plan, scenario, variant_workers, &variant.changes, &format!("v{index}"), None);
            }
            continue;
        }
        // The scenario's controls and, where it differs from its oracle, the
        // readings the build did not take, each run alone.
        let mut variants = scenario.variants.clone();
        if differs || every_reading {
            variants.extend(readings.iter().filter(|reading| !scenario.variants.iter().any(|variant| variant.name == reading.name)).cloned());
        }
        for (index, variant) in variants.iter().enumerate() {
            let judged = if as_a_process {
                read_in_a_process(settings, plan, scenario, variant_workers, &variant.name)
            } else {
                play_and_judge(settings, plan, scenario, variant_workers, &variant.changes, &format!("v{index}"), stated.as_ref()).0
            };
            let differing: Vec<&JudgedWorld> = judged.worlds.iter().filter(|world| !world.differences.is_empty()).collect();
            let equal_where_it_must_differ: Vec<&str> = variant.worlds.iter().filter(|name| !differing.iter().any(|world| world.name == **name)).copied().collect();
            let differs = !differing.is_empty() && equal_where_it_must_differ.is_empty();
            let first_difference = match differing.first() {
                Some(_) if !equal_where_it_must_differ.is_empty() => format!("equal on the worlds {}", equal_where_it_must_differ.join(", ")),
                Some(world) => format!("[{}] {}", world.name, world.differences[0]),
                None => String::new(),
            };
            claim.variants.push(Variant { scenario: scenario.name.clone(), name: variant.name.clone(), control: variant.control, differs, first_difference });
        }
    }
    claim
}
