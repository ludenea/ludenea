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

use asset_loader::{Attachment, Cause, Hold, Loader, Options, ResidentStore, Withheld};
use asset_pipeline::{Cache, Pass};
use content_store::set_hash;
use prototype_core::table::{AdmissionCheck, ArrivalQueue, AssetId, AssetType, Policy, Record, Residents, Rules, SystemName, decode_log_under, encode_log_under};
use prototype_core::world::{Act as SystemAct, Extraction, History, Links, Mode, Refusal, Restore, Size, System, Wait, World, WorldConfig, save_header};
use reference_workload::run_hash;

use crate::oracle::{Element, Expected, Observed, SCRIPTED_SYSTEM, SCRIPTED_SYSTEM_CONTRIBUTOR, Stated, Verdict, compare, element, observed_text, parse_each, stated_entries, system_act};
use crate::run::{ADMISSION_BOUND, BASE, Claim, Family, Outcome, Settings, Variant, import, pass_of_a_save};
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
    /// The other reading of a sheet's "Record of tick n: ... A step without
    /// waiting", where steps stand in a row: that step is the one that runs
    /// tick n, and no step asks for the next tick before what the scenario
    /// states next. The build reads it as the earlier sheets do: tick n runs,
    /// and the step asks for the tick after it.
    StepAfterARecordRunsItsTick,
}

/// A rule changed on purpose: of the core, of the loading capability, of the
/// stand-in pipeline, or of the harness.
#[derive(Clone, Copy)]
pub enum Change {
    Core(fn(&mut Rules)),
    Loader(fn(&mut Options)),
    /// What a pass of the stand-in pipeline does where an action fails.
    Pipeline(fn(&mut asset_pipeline::Options)),
    Harness(Tweak),
    /// A rule of the core changed for a world that replays an input log in
    /// a process of its own, and for that world alone.
    Replay(fn(&mut Rules)),
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
    /// One of several steps without waiting in a row: it asks for the next
    /// tick the world has not run, whatever tick that is, and either runs it
    /// or runs none and names what it awaits. What the scenario states next
    /// applies to the world as that step left it.
    StepInARow,
}

/// A save of a content root, in the manner of a development host: the
/// sources it writes, those it writes with an error and those it deletes.
/// One pass of the stand-in pipeline then runs over the root, and what that
/// pass gives is put in place on the instance.
#[derive(Clone, Debug, Default)]
pub struct Save {
    /// The contents whose sources are written, each with the parameters and
    /// the dependencies of its content. A source saved again as it was has
    /// the bytes it had before.
    pub writes: Vec<&'static str>,
    /// The contents whose sources are written with the word `error` where
    /// the importer reads the first parameter.
    pub with_an_error: Vec<&'static str>,
    /// The assets whose sources are deleted.
    pub deletes: Vec<&'static str>,
}

impl Save {
    /// A save that writes the sources of these contents.
    pub fn of(contents: &[&'static str]) -> Save {
        Save { writes: contents.to_vec(), ..Save::default() }
    }

    /// A save that deletes the sources of these assets.
    pub fn deleting(assets: &[&'static str]) -> Save {
        Save { deletes: assets.to_vec(), ..Save::default() }
    }

    /// The same save, the sources of these contents written with an error.
    pub fn with_an_error(mut self, contents: &[&'static str]) -> Save {
        self.with_an_error.extend_from_slice(contents);
        self
    }
}

/// What moves the content of the first instance, before the round of a
/// tick; or of every instance, where each world of the scenario stands on an
/// instance of its own. On an instance whose capability is held still for a
/// step taken before it has posted, the content moves once that step is
/// answered and the capability let go, before the world steps.
#[derive(Clone, Debug)]
pub enum Move {
    /// A save of the content root and one pass of the stand-in pipeline, of
    /// which an action may fail.
    Save(Save),
    /// These contents replace those of the same assets, and the revision is
    /// put in place.
    Revision(Vec<&'static str>),
    /// The same, the revision holding no entry for the assets named last:
    /// each leaves the instance's manifest, as by a deletion.
    RevisionWithout(Vec<&'static str>, Vec<&'static str>),
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
    /// The wants another contributor of the composition makes at tick 0:
    /// the contributor, then the asset, its type and the policy.
    pub contributed: Vec<(&'static str, &'static str, AssetType, Policy)>,
    /// The asset types the composition declares.
    pub types: Vec<AssetType>,
    /// The composition carries the admission check a sheet defines: it
    /// gives it to the entry of the type of parameters, with the bound.
    pub carries_the_check: bool,
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
    /// It replays that log as a new process does: created as the world that
    /// wrote the log was, with no loading capability and no scripted source,
    /// its records the log's, read back from its bytes.
    pub alone: bool,
}

impl WorldPlan {
    pub fn new(to: u32) -> WorldPlan {
        WorldPlan {
            name: "world".to_string(),
            stated_as: None,
            judged: true,
            instance: 0,
            wants: Vec::new(),
            contributed: Vec::new(),
            types: vec![AssetType::Parameters, AssetType::Second],
            carries_the_check: false,
            set: SetName::None,
            systems: Vec::new(),
            budget: None,
            acts: Vec::new(),
            to,
            offset: 0,
            saves: Vec::new(),
            replays_the_log_of: None,
            alone: false,
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

    /// A want a named contributor of the composition makes at tick 0, after
    /// those of the contributor `base`.
    pub fn wanting_by(mut self, contributor: &'static str, asset: &'static str, policy: Policy) -> WorldPlan {
        self.contributed.push((contributor, asset, AssetType::Parameters, policy));
        self
    }

    /// The systems of the composition that make wants, each with the
    /// contributor that brings it, in the order of the schedule. A system
    /// that the plan gives nothing to do stands in the schedule all the same.
    pub fn scheduling(mut self, systems: &[(&'static str, &'static str)]) -> WorldPlan {
        self.systems.extend(systems.iter().map(|&(name, contributor)| SystemPlan { name, contributor, does: Vec::new() }));
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

    /// The world's composition carries the admission check.
    pub fn carrying_the_check(mut self) -> WorldPlan {
        self.carries_the_check = true;
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

    /// Elements that join the pending record once tick `after` has run,
    /// after what was stated so far: the record of the next tick the world
    /// has not run, whatever tick that is.
    pub fn joins(mut self, after: u32, elements: &[&'static str]) -> WorldPlan {
        self.acts.extend(elements.iter().map(|text| (after + 1, Act::Post(text))));
        self
    }

    /// A step without waiting once tick `after` has run, after what was
    /// stated so far, one of several in a row: it asks for the next tick the
    /// world has not run.
    pub fn step_in_a_row(mut self, after: u32) -> WorldPlan {
        self.acts.push((after + 1, Act::StepInARow));
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

    /// The world replays the input log of another in a new process: it is
    /// created as that world was, with the same composition and the same
    /// budget at creation, with no loading capability and no scripted
    /// source, and runs at another worker count. Its records are the log's,
    /// read back from its bytes, and its store holds every part the log
    /// names. An oracle states it under its own name.
    pub fn replaying_alone(mut self, other: &str) -> WorldPlan {
        self.replays_the_log_of = Some(other.to_string());
        self.alone = true;
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
    /// The world of that scenario that was saved.
    pub world: &'static str,
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
    /// Each world of the scenario stands on an instance of its own, with its
    /// own content root, its own store and its own capability: what moves
    /// the content moves it on every instance.
    pub own_instances: bool,
    /// The harness reports, beside any oracle and compared with nothing,
    /// what the stand-in pipeline gave after each save and what the loading
    /// capability obtained for a world and did not post to it.
    pub reported: bool,
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
            own_instances: false,
            reported: false,
        }
    }

    /// Each world of the scenario stands on an instance of its own: the
    /// worlds are numbered in the order listed, and what moves the content
    /// moves it on every instance.
    pub fn on_instances_of_their_own(mut self) -> Scenario {
        for (index, world) in self.worlds.iter_mut().enumerate() {
            world.instance = index;
        }
        self.own_instances = true;
        self
    }

    /// The harness reports what the stand-in pipeline gave after each save
    /// and what the loading capability withheld from each world.
    pub fn reported(mut self) -> Scenario {
        self.reported = true;
        self
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
        self.read = Some(Read { of, world: "world", at, recorded, dropped: dropped.to_vec() });
        self
    }

    /// The save the scenario reads is the save of this world of the scenario
    /// that saved, where that scenario holds several worlds.
    pub fn of_the_world(mut self, world: &'static str) -> Scenario {
        self.read.as_mut().expect("a scenario that reads a save").world = world;
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

    /// A control that changes several rules together, of the core and of
    /// the loading capability.
    pub fn control_of(mut self, name: &str, changes: Vec<Change>) -> Scenario {
        self.variants.push(VariantPlan { name: name.to_string(), control: true, changes, worlds: Vec::new() });
        self
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
    /// What the harness reports beside any oracle, compared with nothing:
    /// of the stand-in pipeline after each save, and of the loading
    /// capability for each world.
    pub reports: Vec<String>,
    /// Whether the header of a save of a world that replays a log under a
    /// set holds the set's hash.
    pub header_holds_the_set: Option<bool>,
    /// The play holds a part of the scenario's worlds: another process
    /// plays the others.
    pub partial: bool,
    /// The state hashes of worlds that another process played, by name.
    pub beside: Vec<(String, Vec<u64>)>,
    /// For each world that another process is to replay a log in: its name,
    /// and the log it replays as bytes, with the state hashes of the world
    /// that wrote it.
    pub logs: Vec<(String, Saved)>,
}

/// Which worlds of a scenario a play runs. A world that replays an input
/// log in a new process is run by a process of its own in the full run, and
/// inside the process that plays the scenario otherwise.
pub enum Part<'a> {
    /// Every world of the scenario.
    Whole,
    /// Every world but those that replay a log in a new process: the play
    /// gives the logs for them.
    WithoutTheReplay,
    /// Those worlds alone, from the log given, with the state hashes of the
    /// world that wrote it.
    TheReplayAlone(&'a Saved),
}

struct Instance {
    directory: PathBuf,
    cache: Cache,
    loader: Loader,
    /// The set of the first revision.
    first: u64,
}

/// What a play gathers as its content moves and at its end: the count of
/// the packages mounted, and what the harness reports beside any oracle.
#[derive(Default)]
struct Moved {
    packages: usize,
    reports: Vec<String>,
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
    /// The rules of a world that replays a log in a new process: those of
    /// the play, with what a control changes for that world alone.
    replay_rules: Rules,
    /// What a pass of the stand-in pipeline does where an action fails.
    pipeline: asset_pipeline::Options,
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
        for (contributor, asset, asset_type, policy) in &plan.contributed {
            let contributor = names.contributor(contributor).expect("a want of the composition is made by a contributor the sheets name");
            config = config.wanting(contributor, &[(names.assets[*asset], *asset_type, *policy)]);
        }
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

    /// A world that replays an input log as a new process does: created as
    /// the world that wrote the log was, with the same composition and the
    /// same budget at creation, with no loading capability and no scripted
    /// source, over a store that holds every part the log names. Its records
    /// are the log's, read back from its bytes.
    fn replay_alone(&self, plan: &WorldPlan, log: &[u8], workers: usize) -> PlayedWorld {
        let records = decode_log_under(log, &self.rules).expect("a log a world wrote decodes");
        let links = Links::new(self.family.residents());
        let queue = links.queue.clone();
        let mut config = self.configure(plan, workers, None);
        config.rules = self.replay_rules;
        let world = World::new(config, records, links);
        let observed = Observed { first: 0, creation: world.table_bytes(), creation_entries: stated_entries(&world.typed_table_bytes()), ..Observed::default() };
        let mut world = Running { plan: plan.clone(), world, queue, set: None, observed, hold: None, stopped: false, saves: Vec::new() };
        for tick in 1..=plan.to {
            if let Some(awaited) = world.advance(tick) {
                world.observed.stuck = Some((tick, awaited));
                break;
            }
        }
        world.finish();
        let stated_as = plan.stated_as.clone().unwrap_or_else(|| plan.name.clone());
        PlayedWorld { name: plan.name.clone(), stated_as, judged: plan.judged, to: plan.to, observed: world.observed, replays: None, saves: Vec::new(), kills: Vec::new() }
    }

    /// Moves the content of one instance: a save and its pass, a revision,
    /// the repair or the removal of an artifact, a mount or an unmount.
    /// `tick` is the tick whose round the move stands before, and `worlds`
    /// the worlds attached to each instance, for the report of a save.
    fn move_content(&self, change: &Move, tick: u32, at: usize, instances: &[Instance], worlds: &[Vec<String>], moved: &mut Moved) {
        let family = self.family;
        let instance = &instances[at];
        let root = instance.directory.join("content");
        match change {
            Move::Save(save) => {
                family.write_sources(&root, &save.writes);
                family.write_sources_with_an_error(&root, &save.with_an_error);
                for asset in &save.deletes {
                    fs::remove_file(root.join(format!("{asset}.asset"))).expect("the source a save deletes is in the content root");
                }
                let pass = pass_of_a_save(&root, &instance.directory.join("store"), self.pipeline);
                if self.scenario.reported {
                    moved.reports.push(self.report_of_a_pass(&pass, tick, &worlds[at]));
                }
                // What the pass gives is put in place on the instance: the
                // store's notification, where the pass put a revision in place.
                if pass.in_place {
                    instance.loader.revision_in_place().expect("the revision is read");
                }
            }
            Move::Revision(contents) => {
                family.write_sources(&root, contents);
                import(&root, &instance.directory.join("store"));
                instance.loader.revision_in_place().expect("the revision is read");
            }
            Move::RevisionWithout(contents, without) => {
                family.write_sources(&root, contents);
                for asset in without {
                    fs::remove_file(root.join(format!("{asset}.asset"))).expect("the source of an asset the revision takes out is in the content root");
                }
                import(&root, &instance.directory.join("store"));
                instance.loader.revision_in_place().expect("the revision is read");
            }
            Move::Repair(contents) => {
                for name in contents {
                    let _ = instance.cache.remove_object(family.hash(name));
                    instance.cache.put_object(&family.parts[*name]).expect("the whole artifact is written");
                }
            }
            Move::Remove(name) => {
                // From the loose store of every instance; of this one alone
                // where the move reaches each instance in its turn.
                let stores = if self.scenario.own_instances { std::slice::from_ref(instance) } else { instances };
                for instance in stores {
                    instance.cache.remove_object(family.hash(name)).expect("the artifact to remove is in the store");
                }
            }
            Move::Mount(contents) => {
                moved.packages += 1;
                let directory = instance.directory.join(format!("package-{}", moved.packages));
                family.write_sources(&directory.join("content"), contents);
                let (cache, _) = import(&directory.join("content"), &directory.join("store"));
                let domain = cache.revision().expect("the package's revision is in place").domain;
                let entries = domain.keys().copied().collect();
                let file = asset_pipeline::package(&cache, &domain, &entries).expect("the package is built");
                instance.loader.mount("package", file, true).expect("the package mounts");
            }
            Move::Unmount => instance.loader.unmount("package").expect("the package unmounts"),
        }
    }

    /// One part the loading capability obtained for a world and did not
    /// post to it, in words: the asset, the content and the cause.
    fn withheld_text(&self, part: &Withheld) -> String {
        let names = &self.family.names;
        let cause = match part.cause {
            Cause::RefusedReplacement => "the admission check of the world's composition refuses the part, and the asset stays as the world holds it".to_string(),
            Cause::RefusedFirstLoad => "the admission check of the world's composition refuses the part on a first load, and a failure is posted in its place".to_string(),
            Cause::RefusedAndPostedAsAFailure => "the admission check of the world's composition refuses the part, and a failure is posted for an asset the world holds available".to_string(),
            Cause::RefusedByAnotherWorld => "the admission check of another world of the instance refuses the part".to_string(),
            Cause::ListsARefusedAsset(listed) => format!("its part lists {}, whose part is refused", names.asset_name(listed)),
            Cause::HeldBackFor(refused) => format!("the batch is held back while the world refuses the part of {}", names.asset_name(refused)),
        };
        format!("the part of {} under {} ({cause})", names.asset_name(part.asset), names.hash_name(part.hash))
    }

    /// What the harness reports of the stand-in pipeline after a save:
    /// whether its pass put a revision in place, the content under which the
    /// domain manifest in place holds each asset of the family, an asset it
    /// does not hold being named as absent, and each action that failed,
    /// with its source and its diagnostic. Compared with nothing.
    fn report_of_a_pass(&self, pass: &Pass, tick: u32, worlds: &[String]) -> String {
        let names = &self.family.names;
        let put = if pass.in_place { "its pass put a revision in place" } else { "its pass put no revision in place" };
        let held = |(name, asset): (&String, &AssetId)| match pass.revision.domain.get(asset) {
            Some(entry) => format!("{name} under {}", names.hash_name(entry.hash)),
            None => format!("{name} absent"),
        };
        let held: Vec<String> = names.assets.iter().map(held).collect();
        let failed: Vec<String> = pass.failures.iter().map(|(source, diagnostic)| format!("the action of the source {source} failed, with the diagnostic [{diagnostic}]")).collect();
        let failed = if failed.is_empty() { "no action failed".to_string() } else { failed.join("; ") };
        format!(
            "{} [{}] the stand-in pipeline, after the save before tick {tick}: {put}; the domain manifest in place holds {}; {failed}",
            self.scenario.name,
            worlds.join(", "),
            held.join(", ")
        )
    }
}

/// The worker count of a replay: another than the one of the run it replays.
fn other_count(workers: usize) -> usize {
    if workers == 1 { 4 } else { 1 }
}

/// Plays a scenario at one worker count, with the changes of a variant.
/// `tag` tells apart the scratch directories of the plays of one scenario.
#[allow(clippy::too_many_arguments)]
pub fn play(settings: &Settings, family: &Family, scenario: &Scenario, workers: usize, variant: &[Change], tag: &str, saved: Option<&Saved>, part: &Part) -> Played {
    let names = &family.names;
    let (mut rules, mut options, mut tweaks) = (Rules::default(), Options::default(), Vec::new());
    let mut pipeline = asset_pipeline::Options::default();
    let mut of_the_replay: Vec<fn(&mut Rules)> = Vec::new();
    for change in scenario.base.iter().chain(variant) {
        match change {
            Change::Core(change) => change(&mut rules),
            Change::Loader(change) => change(&mut options),
            Change::Pipeline(change) => change(&mut pipeline),
            Change::Harness(tweak) => tweaks.push(*tweak),
            Change::Replay(change) => of_the_replay.push(*change),
        }
    }
    let mut replay_rules = rules;
    for change in of_the_replay {
        change(&mut replay_rules);
    }
    let player = Player { settings, family, scenario, rules, replay_rules, pipeline, tweaks, saved };
    let mut notes = Vec::new();
    let mut moved = Moved::default();
    let other = other_count(workers);
    // The worlds that replay a log in a new process, where this process is
    // that one: nothing else of the scenario is played here.
    if let Part::TheReplayAlone(logged) = part {
        let mut played = Played { partial: true, ..Played::default() };
        for plan in scenario.worlds.iter().filter(|plan| plan.alone) {
            let source = plan.replays_the_log_of.clone().expect("a world that replays names the world whose log it replays");
            played.worlds.push(player.replay_alone(plan, &logged.bytes, other));
            played.beside.push((source, logged.hashes.clone()));
        }
        return played;
    }
    let without_the_replay = matches!(part, Part::WithoutTheReplay);

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

    // The worlds, in the order the scenario lists them; and, per instance,
    // the worlds attached to its capability, in the order attached.
    let mut running: Vec<Running> = Vec::new();
    let mut refused_worlds: Vec<PlayedWorld> = Vec::new();
    let mut attached: Vec<Vec<String>> = vec![Vec::new(); instances.len()];
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
            // The admission checks of the world's composition: the one the
            // sheet defines, for the type of parameters, where it carries it.
            let checks = if plan.carries_the_check { vec![(AssetType::Parameters, AdmissionCheck { bound: ADMISSION_BOUND })] } else { Vec::new() };
            let attachment = Attachment { queue: world.queue.clone(), set, entries: world.world.entries(), unanswered: world.world.unanswered(), restore, standing, checks };
            let _ = slot.set(instance.loader.attach(attachment).expect("the world is attached"));
            attached[plan.instance].push(plan.name.clone());
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
    // The instances a move reaches: the first, or each where every world of
    // the scenario stands on an instance of its own.
    let reached = if scenario.own_instances { instances.len() } else { 1 };
    for tick in first_round..=last {
        // What moves the content of an instance whose capability is held
        // still waits: it happens once the step taken before the capability
        // has posted is answered, and before the world steps.
        let mut waiting: Vec<(usize, &Move)> = Vec::new();
        for (_, change) in scenario.moves.iter().filter(|(before, _)| *before == tick) {
            assert!(!instances.is_empty(), "the content of a scripted scenario does not move");
            for at in 0..reached {
                if running.iter().any(|world| world.plan.instance == at && world.hold.is_some()) {
                    waiting.push((at, change));
                } else {
                    player.move_content(change, tick, at, &instances, &attached, &mut moved);
                }
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
            for (place, act) in acts.into_iter().enumerate() {
                // Under the other reading of a step that follows the record
                // of a tick, that step ran the tick before this round.
                if place == 0 && act == Act::StepInARow && player.tweak(Tweak::StepAfterARecordRunsItsTick) {
                    continue;
                }
                match act {
                    Act::Post(text) => match element(text, names).expect("an element of a scenario sheet") {
                        Element::Event(event) => world.world.post_event(event),
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
                            let at = world.plan.instance;
                            instances[at].loader.settle();
                            // What was to move the content of the instance
                            // moves it now, and the host waits again.
                            let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut waiting).into_iter().partition(|(other, _)| *other == at);
                            waiting = later;
                            for (_, change) in &now {
                                player.move_content(change, tick, at, &instances, &attached, &mut moved);
                            }
                            if !now.is_empty() {
                                instances[at].loader.settle();
                            }
                        }
                    }
                    Act::StepInARow => {
                        let next = world.world.tick() + 1;
                        if next <= world.plan.to
                            && let Some(awaited) = world.advance(next)
                        {
                            world.observed.blocked.push((next, awaited));
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
        assert!(waiting.is_empty(), "{}: before tick {tick}, a move of the content stands behind a capability that no step let go", scenario.name);
    }
    for instance in &instances {
        instance.loader.settle();
    }
    // What the harness reports of the loading capability, for each world:
    // each part that was obtained for that world and not posted to it.
    if scenario.reported {
        for (at, instance) in instances.iter().enumerate() {
            let withheld = instance.loader.report().withheld;
            for (index, world) in attached[at].iter().enumerate() {
                let parts: Vec<String> = withheld.iter().filter(|part| part.world as usize == index).map(|part| player.withheld_text(part)).collect();
                let parts = if parts.is_empty() { "no part was obtained for the world and not posted to it".to_string() } else { format!("obtained for the world and not posted to it: {}", parts.join("; ")) };
                moved.reports.push(format!("{} [{world}] the loading capability: {parts}", scenario.name));
            }
        }
    }

    // What each world left, and the replay of its input log, read back from
    // its bytes, at another worker count.
    let mut played: Vec<PlayedWorld> = Vec::new();
    let mut header_holds_the_set = None;
    for mut world in running {
        world.hold = None;
        world.finish();
        let plan = &world.plan;
        let replays = world.observed.stuck.is_none().then(|| {
            let records = decode_log_under(&encode_log_under(&world.observed.log, &player.rules), &player.rules).expect("a log the world wrote decodes");
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

    // A world that replays the input log of another in a new process: here
    // where the whole scenario is played in one process, and otherwise its
    // log is handed over, as bytes, to the process that replays it.
    let mut logs = Vec::new();
    for plan in scenario.worlds.iter().filter(|plan| plan.alone) {
        let source = plan.replays_the_log_of.as_ref().expect("a world that replays names the world whose log it replays");
        let wrote = played.iter().find(|world| world.name == *source).expect("the world whose log is replayed ran");
        let log = encode_log_under(&wrote.observed.log, &player.rules);
        if without_the_replay {
            logs.push((plan.name.clone(), Saved { bytes: log, hashes: wrote.observed.state.clone() }));
        } else {
            let replayed = player.replay_alone(plan, &log, other);
            played.push(replayed);
        }
    }

    // A world that replays the input log of another, under the set.
    for plan in scenario.worlds.iter().filter(|plan| plan.replays_the_log_of.is_some() && !plan.alone) {
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
    Played { worlds: played, notes, reports: moved.reports, header_holds_the_set, partial: without_the_replay, beside: Vec::new(), logs }
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
    let mut beside = beside_any_oracle(scenario, played, before.map(|before| before.saved));
    if before.is_none() && scenario.checks.iter().any(|check| matches!(check, Check::ContinuesTheSavedRun)) {
        beside.push("the oracle states nothing of the run that saved".to_string());
    }
    if let Some((_, differences)) = judged.first_mut() {
        differences.extend(beside);
    }
    judged
}

/// What the harness compares beside an oracle, and with none: the state
/// hashes of two worlds of a scenario, those of a reader with the run that
/// saved, and the header of a save. `saved` is the save a reader read, with
/// the state hashes of the run that wrote it. Returns what does not hold.
fn beside_any_oracle(scenario: &Scenario, played: &Played, saved: Option<&Saved>) -> Vec<String> {
    let state = |name: &str| {
        let here = played.worlds.iter().find(|world| world.name == name).map(|world| world.observed.state.clone());
        here.or_else(|| played.beside.iter().find(|(other, _)| other == name).map(|(_, state)| state.clone()))
    };
    let mut beside = Vec::new();
    for check in &scenario.checks {
        match check {
            Check::SameState(one, other) => {
                // Where another process plays one of the two worlds, the
                // check is made by the process that knows both.
                let known = state(one).is_some() && state(other).is_some();
                if state(one) != state(other) && (known || !played.partial) {
                    beside.push(format!("the state hashes of the world {one} are not those of the world {other}"));
                }
            }
            // A report, and no difference: the notes of the play hold it.
            Check::ReportSameState(_, _) => {}
            Check::ContinuesTheSavedRun => {
                let Some(saved) = saved else { continue };
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
    beside
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
    /// What the harness reports beside any oracle, compared with nothing.
    reports: Vec<String>,
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
    let played = play(settings, &plan.family, saver, workers, variant, &format!("{tag}-saver"), None, &Part::WithoutTheReplay);
    let world = played.worlds.iter().find(|world| world.name == read.world)?;
    let (_, bytes) = world.saves.iter().find(|(tick, _)| *tick == read.at)?;
    Some(Saved { bytes: bytes.clone(), hashes: world.observed.state.clone() })
}

/// Where the worlds of a scenario that replay a log in a new process run.
enum Replays<'a> {
    /// In the process that plays the scenario: the self-checks.
    Here,
    /// Each in a process of its own, given the name of the control or of
    /// the reading the scenario runs under, none for its own run.
    InAProcess(&'a str),
    /// This process is that one: it plays them alone, from the log at this
    /// path, which the process that played the scenario wrote.
    Alone(&'a Path),
}

/// In the full run, each in a process of its own; in this one otherwise.
fn replays_in(processes: bool, variant: &str) -> Replays<'_> {
    if processes { Replays::InAProcess(variant) } else { Replays::Here }
}

fn log_path(settings: &Settings, scenario: &str, world: &str, tag: &str, workers: usize) -> PathBuf {
    let safe: String = format!("{scenario}-{world}-{tag}").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    settings.scratch.join("logs").join(format!("{safe}-{workers}.log"))
}

/// Plays one scenario and judges it, in this process but for the worlds that
/// replay a log in a process of their own. Without an oracle the scenario is
/// driven and nothing is compared.
#[allow(clippy::too_many_arguments)]
fn play_and_judge(
    settings: &Settings,
    plan: &FamilyPlan,
    scenario: &Scenario,
    workers: usize,
    variant: &[Change],
    tag: &str,
    stated: Option<&Stated>,
    replays: &Replays,
) -> (Judged, Played) {
    let handed;
    let part = match replays {
        Replays::Here => Part::Whole,
        Replays::InAProcess(_) => Part::WithoutTheReplay,
        Replays::Alone(path) => {
            handed = read_saved(path);
            Part::TheReplayAlone(&handed)
        }
    };
    let saved = match &scenario.read {
        None => None,
        Some(read) => match saved_for(settings, plan, read, workers, variant, tag) {
            Some(saved) => Some(saved),
            None => {
                let world = JudgedWorld { name: "world".to_string(), differences: vec!["under this change the run that saved never reached its save".to_string()], ..JudgedWorld::default() };
                return (Judged { worlds: vec![world], ..Judged::default() }, Played::default());
            }
        },
    };
    let played = play(settings, &plan.family, scenario, workers, variant, tag, saved.as_ref(), &part);
    let mut judged = Judged { notes: played.notes.clone(), reports: played.reports.clone(), ..Judged::default() };
    let differences: BTreeMap<String, Vec<String>> = match stated {
        Some(stated) => {
            let saver = scenario.read.as_ref().and_then(|read| stated.get(read.of).and_then(|worlds| worlds.get(read.world)));
            let before = saver.zip(saved.as_ref()).map(|(expected, saved)| Before { expected, saved });
            judge(settings, &plan.family, scenario, &played, stated, before.as_ref()).into_iter().collect()
        }
        None => {
            // With no oracle, what the harness compares beside one is
            // compared all the same: none of it reads an oracle.
            for unheld in beside_any_oracle(scenario, &played, saved.as_ref()) {
                judged.notes.push(format!("{}: compared beside any oracle, and not held: {unheld}", scenario.name));
            }
            BTreeMap::new()
        }
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
    // Each world that replays a log in a new process: the log is written,
    // as bytes, with the state hashes of the world that wrote it, and a
    // process of its own replays and judges it.
    if let Replays::InAProcess(variant) = replays {
        for (world, logged) in &played.logs {
            let path = log_path(settings, &scenario.name, world, tag, workers);
            write_saved(&path, logged);
            let replayed = in_a_process(settings, plan, scenario, workers, variant, Some((world, &path)), stated.is_none());
            judged.worlds.extend(replayed.worlds);
            judged.notes.extend(replayed.notes);
            judged.reports.extend(replayed.reports);
        }
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
/// `--reader <family> <scenario> <workers> <scratch> <size> <variant>`; or a
/// world that replays an input log in a process of its own, the same with
/// the path of the log last: `--replayer <family> ... <variant> <log>`.
/// It prints what it observed, a line per fact. A process that is `driven`
/// reads no oracle and compares nothing.
pub fn child(arguments: &[String], driven: bool, family: &dyn Fn(&Settings, &str) -> Option<FamilyPlan>, readings: &[VariantPlan]) -> String {
    let (arguments, log) = match arguments {
        [six @ .., log] if arguments.len() == 7 => (six, Some(Path::new(log))),
        all => (all, None),
    };
    let [name, scenario, workers, scratch, size, variant] = arguments else { return "difference world\tthe reader was not given its six arguments\n".to_string() };
    let size = if size == "small" { Size::Small } else { Size::Default };
    let settings = Settings { size, workers: Vec::new(), scratch: Path::new(scratch).to_path_buf() };
    let plan = family(&settings, name).expect("a family of the sheets");
    let scenario = plan.scenarios.iter().find(|other| other.name == *scenario).expect("a scenario of the family");
    let stated = (!driven).then(|| parse_each(plan.oracle, &plan.family.names).0);
    let changes = variant_named(scenario, readings, variant);
    let tag = if variant.is_empty() { "run".to_string() } else { "variant".to_string() };
    let replays = log.map_or(Replays::Here, Replays::Alone);
    let (judged, _) = play_and_judge(&settings, &plan, scenario, workers.parse().expect("a worker count"), &changes, &tag, stated.as_ref(), &replays);
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
    for report in &judged.reports {
        out.push_str(&format!("report {report}\n"));
    }
    out
}

/// Runs one reader as a process of its own and reads what it prints; or,
/// given a world that replays a log and the path of that log, that world.
fn in_a_process(settings: &Settings, plan: &FamilyPlan, scenario: &Scenario, workers: usize, variant: &str, replay: Option<(&String, &Path)>, driven: bool) -> Judged {
    let exe = std::env::current_exe().expect("the harness knows its own path");
    let size = settings.size.name();
    // A process told that it is driven reads no oracle and compares nothing.
    let mode = match (replay.is_some(), driven) {
        (false, false) => "--reader",
        (false, true) => "--reader-driven",
        (true, false) => "--replayer",
        (true, true) => "--replayer-driven",
    };
    let mut process = Process::new(exe);
    process.args([mode, plan.name, &scenario.name, &workers.to_string(), &settings.scratch.to_string_lossy(), size, variant]);
    if let Some((_, log)) = replay {
        process.arg(log);
    }
    let output = process.output().expect("a reader starts");
    // The world a process that ended badly is charged to.
    let charged = replay.map_or("world", |(world, _)| world.as_str());
    let mut judged = Judged::default();
    let mut worlds: BTreeMap<String, JudgedWorld> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((kind, rest)) = line.split_once(' ') else { continue };
        if kind == "note" {
            judged.notes.push(rest.to_string());
            continue;
        }
        if kind == "report" {
            judged.reports.push(rest.to_string());
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
        let entry = worlds.entry(charged.to_string()).or_insert_with(|| JudgedWorld { name: charged.to_string(), ..JudgedWorld::default() });
        entry.differences.push(format!("the process ended with {}: {error}", output.status));
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
        let as_a_process = processes && scenario.read.is_some();
        let mut differs = false;
        // What the scenario's own run gave at the worker count of the
        // controls: per world, its run hash and what it gave in words.
        let mut own: Vec<(String, u64, Vec<String>)> = Vec::new();
        for &workers in &settings.workers {
            let judged = if as_a_process {
                in_a_process(settings, plan, scenario, workers, "", None, stated.is_none())
            } else {
                let (judged, played) = play_and_judge(settings, plan, scenario, workers, &[], "run", stated.as_ref(), &replays_in(processes, ""));
                for world in &played.worlds {
                    for (tick, bytes) in &world.saves {
                        write_saved(&save_path(settings, &scenario.name, *tick, workers), &Saved { bytes: bytes.clone(), hashes: world.observed.state.clone() });
                    }
                }
                judged
            };
            if stated.is_none() {
                // Driven with no oracle: a world that stays gated, or a log
                // that does not replay, is all that shows.
                for world in &judged.worlds {
                    if let Some(line) = world.observed.iter().find(|line| line.starts_with("never ran tick")) {
                        claim.notes.push(format!("{} [{}] at {workers} workers: the world {line}", scenario.name, world.name));
                    }
                    if world.replays == Some(false) {
                        claim.notes.push(format!("{} [{}] at {workers} workers: the log does not replay", scenario.name, world.name));
                    }
                    // With no oracle a world differs from nothing: what is
                    // left here is a process that failed, or a save that
                    // was never reached.
                    for difference in &world.differences {
                        claim.notes.push(format!("{} [{}] at {workers} workers: {difference}", scenario.name, world.name));
                    }
                }
            }
            let first = workers == settings.workers[0];
            if workers == variant_workers {
                own = judged.worlds.iter().map(|world| (world.name.clone(), world.run_hash, world.observed.clone())).collect();
            }
            for world in judged.worlds {
                differs |= !world.differences.is_empty() || world.replays == Some(false);
                let observed = if first { world.observed } else { Vec::new() };
                claim.outcomes.push(Outcome { scenario: scenario.name.clone(), world: world.name, workers, differences: world.differences, run_hash: world.run_hash, replays: world.replays, observed });
            }
            if first {
                claim.notes.extend(judged.notes);
                claim.reports.extend(judged.reports);
            }
        }
        if stated.is_none() {
            // Every control is driven, so that none fails the harness itself.
            // With no oracle a control shows against the scenario's own run
            // alone: one that gives that very run changes nothing a
            // comparison could see, and the note says so; of one that gives
            // another run, the note holds the first thing it gave otherwise.
            for (index, variant) in scenario.variants.iter().enumerate() {
                let (judged, _) = play_and_judge(settings, plan, scenario, variant_workers, &variant.changes, &format!("v{index}"), None, &Replays::Here);
                let gave: Vec<(String, u64, Vec<String>)> = judged.worlds.iter().map(|world| (world.name.clone(), world.run_hash, world.observed.clone())).collect();
                let unreached = judged.worlds.iter().flat_map(|world| &world.differences).next();
                let named = |name: &str| variant.worlds.is_empty() || variant.worlds.contains(&name);
                let another = |(name, hash, lines): &(String, u64, Vec<String>)| {
                    let Some((_, own_hash, own_lines)) = own.iter().find(|(other, _, _)| other == name) else { return Some(format!("[{name}] a world the scenario's own run does not hold")) };
                    let line = lines.iter().zip(own_lines).find(|(given, held)| given != held).map(|(given, _)| given.clone()).or_else(|| lines.get(own_lines.len()).cloned());
                    match line {
                        Some(line) => Some(format!("[{name}] {line}")),
                        None if lines.len() < own_lines.len() => Some(format!("[{name}] the run stops before: {}", own_lines[lines.len()])),
                        None if hash != own_hash => Some(format!("[{name}] the state hashes alone")),
                        None => None,
                    }
                };
                let first = unreached.cloned().or_else(|| gave.iter().filter(|(name, _, _)| named(name)).find_map(another));
                let every_named_world = variant.worlds.iter().all(|name| gave.iter().filter(|(other, _, _)| other == name).any(|world| another(world).is_some()));
                let text = match first {
                    Some(first) if every_named_world => format!("gives another run: {first}"),
                    Some(first) => format!("GIVES THE RUN OF THE SCENARIO ITSELF on a world it names, and another run elsewhere: {first}"),
                    None => "GIVES THE RUN OF THE SCENARIO ITSELF".to_string(),
                };
                claim.notes.push(format!("{} control [{}] {text}", scenario.name, variant.name));
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
                in_a_process(settings, plan, scenario, variant_workers, &variant.name, None, false)
            } else {
                play_and_judge(settings, plan, scenario, variant_workers, &variant.changes, &format!("v{index}"), stated.as_ref(), &replays_in(processes, &variant.name)).0
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
