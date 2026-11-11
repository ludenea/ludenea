//! The nested instances and overrides spike of the scenes and levels
//! research. Throwaway, never product code.
//!
//! The scenario and the measurements: see the README for the hypotheses they
//! serve. `measure` imports the generated scenes with their planted
//! conflicts, runs the scripted families by every application path at every
//! worker count against the sequential oracle (the self-check runs it on the
//! small scenes) and `report::write` prints the outcome as one JSON document,
//! a short summary going to standard error and the per-tick log to a file.

// The tick graph copied from the fixtures needs the one unsafe construct of
// the runtime skeleton (the split of a column into disjoint chunk slices);
// every other module refuses unsafe code.
#![deny(unsafe_code)]

mod generate;
mod oracle;
mod report;
mod scene;
mod state;
mod world;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reference_workload::first_divergent_tick;

use generate::{BUILDING, LEVEL, Planted, ROOM, SceneSet, Sizes, VAULT, asset_id, generate, plant, replace_room};
use oracle::{Allocation, OracleRun, TreeNode, family_tree};
use report::TickLog;
use scene::{Change, Payload, Prepared, STATED_ORDER, Store, apply_list, effective, payloads_digest};
use state::{Actual, Event, FailReason, InstancePath, Logged, ReadCounts, Resolution, resolve};
use world::{Application, Command, Instruments, World, WorldConfig};

pub const SPIKE: &str = "workload-nested-overrides";
pub const SEED: u64 = 0x0e57_ed0e_11de_2026;
pub const WORKER_COUNTS: [usize; 3] = [1, 4, 8];
pub const TICKS: u32 = 72;
/// The ticks after which every entity is addressed by its full reference:
/// with every family loaded, and at the end, after the unloads and reloads.
pub const CHECKPOINTS: [u32; 2] = [28, TICKS];

#[derive(Clone, Debug)]
pub struct Settings {
    pub sizes: Vec<Sizes>,
    pub worker_counts: Vec<usize>,
    /// Where the per-tick log goes; none keeps the summary alone.
    pub log_path: Option<PathBuf>,
}

impl Settings {
    pub fn full(log_path: PathBuf) -> Settings {
        Settings { sizes: vec![Sizes::STANDARD, Sizes::DENSE], worker_counts: WORKER_COUNTS.to_vec(), log_path: Some(log_path) }
    }

    /// The small case of the self-checks.
    pub fn small(log_path: Option<PathBuf>) -> Settings {
        Settings { sizes: vec![Sizes::SMALL], worker_counts: vec![1, 3], log_path }
    }
}

/// How a scripted family is expected to appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    /// Whole, at the structural fixed point of its command's tick.
    OneBatch,
    /// The parent when its payload arrives, each child when its own does.
    AsPayloadsArrive,
}

/// One load of the script.
#[derive(Clone, Copy, Debug)]
pub struct FamilyPlan {
    pub name: &'static str,
    pub root: u64,
    pub tick: u32,
    pub set: u64,
    pub scene: u64,
    pub required: bool,
    pub appearance: Appearance,
}

/// The scripted loads: families of depth three, two and one, required and
/// optional, on scenes already loaded and on scenes that arrive after the
/// command, two of them on a set whose room is at another content hash than
/// the one its building names, and two reloads.
pub const PLAN: [FamilyPlan; 10] = [
    FamilyPlan { name: "a required level on scenes not yet loaded", root: 0, tick: 1, set: 0, scene: LEVEL, required: true, appearance: Appearance::OneBatch },
    FamilyPlan { name: "an optional level on scenes already loaded", root: 1, tick: 3, set: 0, scene: LEVEL, required: false, appearance: Appearance::OneBatch },
    FamilyPlan { name: "an optional building on scenes already loaded", root: 2, tick: 3, set: 0, scene: BUILDING, required: false, appearance: Appearance::OneBatch },
    FamilyPlan { name: "an optional room on a scene already loaded", root: 3, tick: 3, set: 0, scene: ROOM, required: false, appearance: Appearance::OneBatch },
    FamilyPlan { name: "an optional level on scenes not yet loaded", root: 4, tick: 5, set: 1, scene: LEVEL, required: false, appearance: Appearance::AsPayloadsArrive },
    FamilyPlan { name: "a required building on scenes not yet loaded", root: 5, tick: 8, set: 2, scene: BUILDING, required: true, appearance: Appearance::OneBatch },
    FamilyPlan { name: "an optional level whose rooms are at another hash", root: 6, tick: 10, set: 3, scene: LEVEL, required: false, appearance: Appearance::AsPayloadsArrive },
    FamilyPlan { name: "a required level whose rooms are at another hash", root: 7, tick: 22, set: 3, scene: LEVEL, required: true, appearance: Appearance::OneBatch },
    FamilyPlan { name: "the optional level loaded again after its unload", root: 1, tick: 40, set: 0, scene: LEVEL, required: false, appearance: Appearance::OneBatch },
    FamilyPlan { name: "the required level loaded again after its unload", root: 0, tick: 56, set: 0, scene: LEVEL, required: true, appearance: Appearance::OneBatch },
];
pub const SET_COUNT: u64 = 4;
/// The set whose room is replaced under an unchanged building.
pub const REPLACED_SET: u64 = 3;

/// The script: the loads of the plan, the unloads between them, and three
/// commands the validation must refuse or ignore.
pub fn script() -> (Vec<(u32, Command)>, BTreeMap<u64, u32>) {
    let mut commands: Vec<(u32, Command)> = PLAN.iter().map(|family| (family.tick, Command::Load { root: family.root, asset: asset_id(family.set, family.scene), required: family.required })).collect();
    commands.extend([
        (30, Command::Unload { root: 1 }),
        (50, Command::Unload { root: 0 }),
        // A record that names another scene, an instance that has no record,
        // an instance already wanted: none changes anything.
        (60, Command::Load { root: 1, asset: asset_id(0, BUILDING), required: false }),
        (62, Command::Unload { root: 9 }),
        (64, Command::Load { root: 0, asset: asset_id(0, LEVEL), required: true }),
        (66, Command::Unload { root: 4 }),
    ]);
    commands.sort_by_key(|(tick, _)| *tick);
    // The polls between a request and its arrival: the scenes of the second
    // set arrive one after the other, so that its family appears over ticks.
    let delays = BTreeMap::from([
        (asset_id(0, ROOM), 1),
        (asset_id(0, VAULT), 4),
        (asset_id(1, LEVEL), 3),
        (asset_id(1, BUILDING), 4),
        (asset_id(1, ROOM), 5),
        (asset_id(1, VAULT), 9),
        (asset_id(2, ROOM), 3),
    ]);
    (commands, delays)
}

/// Generates the sets of a size into a new store and replaces the room of
/// the last one under its unchanged building.
pub fn build_store(sizes: &Sizes, overrides: bool) -> (Arc<Store>, Vec<SceneSet>) {
    let store = Store::new();
    let sets: Vec<SceneSet> = (0..SET_COUNT).map(|set| generate(&store, set, SEED, sizes, overrides).expect("the generated sources import")).collect();
    replace_room(&store, &sets[REPLACED_SET as usize]);
    (store, sets)
}

/// One world run.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub application: Application,
    pub workers: usize,
    pub hashes: Vec<u64>,
    pub content: Vec<u64>,
    pub log: Vec<Logged>,
    pub events: Vec<Event>,
    pub reads: ReadCounts,
    pub instruments: Instruments,
    pub gate_polls: u64,
    pub required_missing: u64,
    pub prepared_misses: u64,
    pub slots: usize,
    pub entities: usize,
    pub wall: Duration,
    pub matches_oracle: bool,
}

/// Every entity addressed by its full reference, at the checkpoints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identities {
    pub instances: u64,
    pub entities: u64,
    /// Entities an override changed (a component or a field) and kept.
    pub overridden: u64,
    /// Full references that resolve to the entity that holds them.
    pub kept: u64,
    /// Anything else: a full reference the sources state and the world does
    /// not resolve, a member the sources do not state, a live slot outside
    /// every table.
    pub changed: u64,
    /// Instances whose payload is at another hash, found failed.
    pub failed_as_expected: u64,
    /// Instances loaded although the available payload is at another hash
    /// than the one their parent's payload names.
    pub spawned_at_another_hash: u64,
}

/// The world with overrides beside the same world without any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Control {
    pub plain_entities: u64,
    pub overridden_entities: u64,
    /// Full references both worlds hold.
    pub kept: u64,
    /// Among them, entities a component or a field override targets.
    pub kept_overridden: u64,
    pub removed_by_override: u64,
    pub inside_removed_instances: u64,
    pub added_by_override: u64,
    /// A full reference one world holds and the other does not, that no
    /// override explains.
    pub unexplained: u64,
}

#[derive(Clone, Debug)]
pub struct FamilyReport {
    pub plan: FamilyPlan,
    pub instances: u64,
    pub depth: u64,
    pub entities: u64,
    pub failed_children: u64,
    pub first_tick: u32,
    pub last_tick: u32,
    pub distinct_ticks: u64,
    pub parents_first: bool,
    pub holds: bool,
}

/// The import of one size: what was accepted, what was planted and refused.
#[derive(Clone, Debug)]
pub struct ImportReport {
    pub payloads: Vec<Arc<Payload>>,
    pub digest: u64,
    pub lists: u64,
    pub items_by_kind: [u64; 5],
    pub items_by_path_length: [u64; 2],
    pub outer_on_inner: u64,
    pub planted: Vec<Planted>,
    /// Lists applied in every order of the five kinds, forward and backward.
    pub orders_tried: u64,
    pub orders_differing: u64,
}

impl ImportReport {
    pub fn named(&self) -> impl Iterator<Item = &Planted> {
        self.planted.iter().filter(|case| case.named)
    }

    pub fn conflicts_refused(&self) -> bool {
        self.planted.iter().all(Planted::refused_by_name)
    }
}

#[derive(Clone, Debug)]
pub struct ScenarioReport {
    pub sizes: Sizes,
    pub import: ImportReport,
    pub runs: Vec<RunReport>,
    pub oracle_payload: OracleRun,
    pub oracle_in_world: OracleRun,
    pub families: Vec<FamilyReport>,
    pub identities: Vec<(Application, Identities)>,
    pub control: Control,
    pub prepared_payloads: usize,
    pub prepared_instances: u64,
    pub prepare: Duration,
    pub expected_failed_children: u64,
}

impl ScenarioReport {
    pub fn of(&self, application: Application) -> impl Iterator<Item = &RunReport> {
        self.runs.iter().filter(move |run| run.application == application)
    }

    pub fn first(&self, application: Application) -> &RunReport {
        self.of(application).next().expect("every application ran")
    }

    pub fn identical_across_workers(&self, application: Application) -> bool {
        let first = self.first(application);
        self.of(application).all(|run| run.hashes == first.hashes && run.content == first.content && run.log == first.log && run.events == first.events && run.reads == first.reads)
    }

    pub fn matches_oracle(&self, application: Application) -> bool {
        self.of(application).all(|run| run.matches_oracle)
    }

    /// One history at every worker count, equal to the oracle's at every tick.
    pub fn determinism_holds(&self) -> bool {
        Application::ALL.iter().all(|application| self.identical_across_workers(*application) && self.matches_oracle(*application))
            && self.runs.iter().all(|run| run.reads.stale == 0)
            && [&self.oracle_payload, &self.oracle_in_world].iter().all(|oracle| oracle.check_failures == 0 && oracle.record_mismatches == 0)
    }

    /// The overrides applied to the payload outside the tick against the
    /// ordered application in a staging area: both hashes at every tick.
    pub fn payload_equals_staging(&self) -> bool {
        let (prepared, staged) = (self.first(Application::Prepared), self.first(Application::Staged));
        prepared.hashes == staged.hashes && prepared.content == staged.content && prepared.events == staged.events
    }

    /// Against the ordered application in the world: what every spawn
    /// produced, by full reference.
    pub fn in_world_spawns_equal(&self) -> bool {
        self.first(Application::Prepared).events == self.first(Application::InPlace).events
    }

    pub fn in_world_first_state_divergence(&self) -> Option<usize> {
        first_divergent_tick(&self.first(Application::Prepared).hashes, &self.first(Application::InPlace).hashes)
    }

    pub fn in_world_first_content_divergence(&self) -> Option<usize> {
        first_divergent_tick(&self.first(Application::Prepared).content, &self.first(Application::InPlace).content)
    }

    pub fn failed_children(&self) -> u64 {
        self.first(Application::Prepared).events.iter().filter(|event| matches!(event, Event::Failed { reason: FailReason::AnotherContentHash, .. })).count() as u64
    }

    pub fn families_hold(&self) -> bool {
        self.families.iter().all(|family| family.holds)
            && self.runs.iter().all(|run| run.required_missing == 0 && run.prepared_misses == 0)
            && self.oracle_payload.required_missing == 0
            && self.oracle_in_world.required_missing == 0
    }

    pub fn another_hash_fails(&self) -> bool {
        self.expected_failed_children > 0 && self.failed_children() == self.expected_failed_children && self.identities.iter().all(|(_, identities)| identities.spawned_at_another_hash == 0)
    }

    pub fn identities_hold(&self) -> bool {
        self.identities.iter().all(|(_, identities)| identities.changed == 0 && identities.kept == identities.entities && identities.entities > 0) && self.control.unexplained == 0 && self.control.kept_overridden > 0
    }

    pub fn import_holds(&self) -> bool {
        self.import.conflicts_refused() && self.import.orders_differing == 0
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub scenarios: Vec<ScenarioReport>,
    pub log_lines: u64,
    pub log_path: Option<PathBuf>,
}

fn permutations() -> Vec<[u8; 5]> {
    fn extend(prefix: &mut Vec<u8>, out: &mut Vec<[u8; 5]>) {
        if prefix.len() == 5 {
            out.push(prefix.as_slice().try_into().expect("five kinds"));
            return;
        }
        for kind in STATED_ORDER {
            if !prefix.contains(&kind) {
                prefix.push(kind);
                extend(prefix, out);
                prefix.pop();
            }
        }
    }
    let mut out = Vec::new();
    extend(&mut Vec::new(), &mut out);
    out
}

/// The import of a size: the sets accepted, the conflicts planted in a
/// store of their own, and every list of a family applied in every order of
/// the five kinds.
fn measure_import(sizes: &Sizes, store: &Store, sets: &[SceneSet]) -> ImportReport {
    let payloads = store.assets();
    let mut report = ImportReport {
        digest: payloads_digest(&payloads),
        lists: 0,
        items_by_kind: [0; 5],
        items_by_path_length: [0; 2],
        outer_on_inner: sets.iter().map(|set| set.outer_on_inner).sum(),
        planted: Vec::new(),
        orders_tried: 0,
        orders_differing: 0,
        payloads,
    };
    for nested in report.payloads.iter().flat_map(|payload| &payload.nested) {
        report.lists += 1;
        for item in &nested.overrides {
            report.items_by_kind[item.kind() as usize] += 1;
            report.items_by_path_length[item.path.len().min(1)] += 1;
        }
    }
    let laboratory = Store::new();
    let set = generate(&laboratory, 0, SEED, sizes, true).expect("the generated sources import");
    report.planted = plant(&laboratory, &set);
    let level = store.current(asset_id(0, LEVEL)).expect("the level of the first set");
    for node in family_tree(store, &level) {
        let stated = effective(&node.payload, &node.lists).expect("a validated list applies");
        for order in permutations() {
            for reversed in [false, true] {
                let mut members: BTreeMap<u64, scene::Proto> = node.payload.protos.iter().map(|proto| (proto.local, *proto)).collect();
                let applied = node.lists.iter().all(|list| apply_list(&mut members, list, &order, reversed).is_ok());
                report.orders_tried += 1;
                if !applied || members.into_values().collect::<Vec<_>>() != stated {
                    report.orders_differing += 1;
                }
            }
        }
    }
    report
}

/// The instances of every loaded family as the sources state them, by identity.
fn family_nodes(store: &Store, world: &World) -> BTreeMap<InstancePath, TreeNode> {
    let instances = world.instances();
    let mut nodes = BTreeMap::new();
    for record in instances.loaded().filter(|record| record.path.locals.is_empty()) {
        let payload = &instances.available[&record.asset.expect("a loaded instance names its scene")];
        for node in family_tree(store, payload) {
            nodes.insert(InstancePath { root: record.path.root, locals: node.locals.clone() }, node);
        }
    }
    nodes
}

/// Addresses every entity the sources state by its full reference and
/// verifies the world resolves it to the entity that holds it, and holds
/// nothing else.
fn verify_identities(store: &Store, world: &World, identities: &mut Identities) {
    let instances = world.instances();
    let state = world.state();
    let nodes = family_nodes(store, world);
    for (path, node) in &nodes {
        let record = instances.index.get(path).map(|index| (*index, &instances.records[*index as usize]));
        match (node.failed, record) {
            (true, Some((_, record))) if record.actual == Actual::Failed(FailReason::AnotherContentHash) => identities.failed_as_expected += 1,
            (true, _) => identities.changed += 1,
            (false, Some((index, record))) if record.actual == Actual::Loaded => {
                identities.instances += 1;
                identities.entities += node.ids.len() as u64;
                let held: BTreeSet<u64> = record.members.iter().map(|member| member.0).collect();
                identities.changed += held.symmetric_difference(&node.ids).count() as u64;
                for local in node.ids.intersection(&held) {
                    match resolve(state, true, &record.members, index, *local) {
                        Resolution::Present(..) => identities.kept += 1,
                        _ => identities.changed += 1,
                    }
                }
                let changed: BTreeSet<u64> = node.lists.iter().flatten().filter(|item| item.kind() >= 2 && node.ids.contains(&item.target)).map(|item| item.target).collect();
                identities.overridden += changed.len() as u64;
                let available = record.asset.and_then(|asset| instances.available.get(&asset)).map(|payload| payload.content_hash);
                if record.expected_hash.is_some_and(|hash| available != Some(hash)) {
                    identities.spawned_at_another_hash += 1;
                }
            }
            // A child whose payload has not arrived yet is not present.
            (false, _) => {}
        }
    }
    identities.changed += instances.loaded().filter(|record| !nodes.contains_key(&record.path)).count() as u64;
    let members: usize = instances.loaded().map(|record| record.members.len()).sum();
    identities.changed += state.alive_count().abs_diff(members) as u64;
}

fn full_references(world: &World) -> BTreeSet<(InstancePath, u64)> {
    world.instances().loaded().flat_map(|record| record.members.iter().map(|member| (record.path.clone(), member.0))).collect()
}

/// Compares the full references of the world with overrides and of the
/// world without: every difference must be an override's doing.
fn compare_with_plain(store: &Store, overridden: &World, plain: &World) -> Control {
    let nodes = family_nodes(store, overridden);
    let (with, without) = (full_references(overridden), full_references(plain));
    let mut control = Control { plain_entities: without.len() as u64, overridden_entities: with.len() as u64, ..Control::default() };
    let speaks = |node: &TreeNode, local: u64, wanted: &dyn Fn(&Change) -> bool| node.lists.iter().flatten().any(|item| item.target == local && wanted(&item.change));
    for (path, local) in &with {
        let node = &nodes[path];
        if without.contains(&(path.clone(), *local)) {
            control.kept += 1;
            if speaks(node, *local, &|change| !matches!(change, Change::RemoveEntity | Change::AddEntity(_))) {
                control.kept_overridden += 1;
            }
        } else if speaks(node, *local, &|change| matches!(change, Change::AddEntity(_))) {
            control.added_by_override += 1;
        } else {
            control.unexplained += 1;
        }
    }
    for (path, local) in without.difference(&with) {
        match nodes.get(path) {
            Some(node) if speaks(node, *local, &|change| *change == Change::RemoveEntity) => control.removed_by_override += 1,
            Some(_) => control.unexplained += 1,
            None => {
                // The instance does not exist with overrides: the instancing
                // entity of the first missing level must have been removed.
                let missing = (1..=path.locals.len()).find(|depth| !nodes.contains_key(&InstancePath { root: path.root, locals: path.locals[..*depth].to_vec() })).expect("the instance is missing");
                let above = nodes.get(&InstancePath { root: path.root, locals: path.locals[..missing - 1].to_vec() });
                if above.is_some_and(|node| speaks(node, path.locals[missing - 1], &|change| *change == Change::RemoveEntity)) {
                    control.inside_removed_instances += 1;
                } else {
                    control.unexplained += 1;
                }
            }
        }
    }
    control
}

/// How each scripted family appeared, from the events of one run.
fn family_reports(events: &[Event]) -> Vec<FamilyReport> {
    PLAN.iter()
        .map(|plan| {
            let until = PLAN.iter().filter(|other| other.root == plan.root && other.tick > plan.tick).map(|other| other.tick).min().unwrap_or(u32::MAX);
            let window = |tick: u32, path: &InstancePath| path.root == plan.root && tick >= plan.tick && tick < until;
            let spawned: BTreeMap<&InstancePath, (u32, u32)> = events
                .iter()
                .filter_map(|event| match event {
                    Event::Spawned { tick, path, members, .. } if window(*tick, path) => Some((path, (*tick, *members))),
                    _ => None,
                })
                .collect();
            let failed_children = events.iter().filter(|event| matches!(event, Event::Failed { tick, path, .. } if window(*tick, path))).count() as u64;
            let ticks: BTreeSet<u32> = spawned.values().map(|(tick, _)| *tick).collect();
            let parents_first = spawned.iter().all(|(path, (tick, _))| match path.locals.split_last() {
                Some((_, above)) => spawned.get(&InstancePath { root: path.root, locals: above.to_vec() }).is_some_and(|(parent, _)| parent <= tick),
                None => true,
            });
            let (first_tick, last_tick) = (ticks.first().copied().unwrap_or(0), ticks.last().copied().unwrap_or(0));
            let holds = !spawned.is_empty()
                && parents_first
                && match plan.appearance {
                    Appearance::OneBatch => ticks.len() == 1 && first_tick == plan.tick,
                    Appearance::AsPayloadsArrive => ticks.len() >= 2 && first_tick > plan.tick,
                };
            FamilyReport {
                plan: *plan,
                instances: spawned.len() as u64,
                depth: spawned.keys().map(|path| path.locals.len() as u64 + 1).max().unwrap_or(0),
                entities: spawned.values().map(|(_, members)| u64::from(*members)).sum(),
                failed_children,
                first_tick,
                last_tick,
                distinct_ticks: ticks.len() as u64,
                parents_first,
                holds,
            }
        })
        .collect()
}

fn measure_scenario(sizes: &Sizes, settings: &Settings, tick_log: &mut TickLog) -> ScenarioReport {
    let (store, sets) = build_store(sizes, true);
    let import = measure_import(sizes, &store, &sets);
    eprintln!(
        "{}: {} payloads, {} lists, {} overrides; planted conflicts refused by name {} of {}; {} orders of application, {} differing",
        sizes.name,
        import.payloads.len(),
        import.lists,
        import.items_by_kind.iter().sum::<u64>(),
        import.planted.iter().filter(|case| case.refused_by_name()).count(),
        import.planted.len(),
        import.orders_tried,
        import.orders_differing
    );
    let (commands, delays) = script();

    // The members of every instance of the script, prepared outside the tick.
    let prepared = Prepared::new();
    let started = Instant::now();
    let prepared_instances = PLAN.iter().map(|plan| prepared.prepare_family(&store, &store.current(asset_id(plan.set, plan.scene)).expect("generated"), &[])).sum();
    let prepare = started.elapsed();

    let mut runs = Vec::new();
    let mut identities = Vec::new();
    let mut overridden_world = None;
    for application in Application::ALL {
        for (at, &workers) in settings.worker_counts.iter().enumerate() {
            let mut world = World::new(WorldConfig::new(commands.clone(), delays.clone(), application, workers), store.clone(), prepared.clone());
            let started = Instant::now();
            let mut verified = Identities::default();
            for checkpoint in CHECKPOINTS {
                world.advance_to(checkpoint).expect("the world steps");
                if at == 0 {
                    verify_identities(&store, &world, &mut verified);
                }
            }
            let wall = started.elapsed();
            let instances = world.instances();
            runs.push(RunReport {
                application,
                workers,
                hashes: world.hashes(),
                content: world.content_hashes(),
                log: instances.log.clone(),
                events: instances.events.clone(),
                reads: world.reads(),
                instruments: world.instruments(),
                gate_polls: instances.gate_polls,
                required_missing: instances.required_missing,
                prepared_misses: instances.prepared_misses,
                slots: world.state().len(),
                entities: world.state().alive_count(),
                wall,
                matches_oracle: false,
            });
            if at == 0 {
                identities.push((application, verified));
                if application == Application::Prepared {
                    overridden_world = Some(world);
                }
            }
        }
    }

    let log = runs[0].log.clone();
    let oracle_payload = oracle::run(&store, &log, TICKS, Allocation::Payload);
    let oracle_in_world = oracle::run(&store, &log, TICKS, Allocation::InWorld);
    for run in &mut runs {
        let oracle = if run.application == Application::InPlace { &oracle_in_world } else { &oracle_payload };
        run.matches_oracle = run.hashes == oracle.hashes && run.content == oracle.content && run.events == oracle.events && run.reads == oracle.reads && run.log == log;
        tick_log.record(sizes, run, oracle);
        eprintln!(
            "{} {} at {} workers: oracle {}, state {:#018x}, content {:#018x}, {} entities in {} slots, {:.0} ticks per second",
            sizes.name,
            run.application.name(),
            run.workers,
            if run.matches_oracle { "matches" } else { "DIFFERS" },
            reference_workload::run_hash(&run.hashes),
            reference_workload::run_hash(&run.content),
            run.entities,
            run.slots,
            f64::from(TICKS) / run.instruments.stepping.as_secs_f64()
        );
    }

    // The same script on the same scenes without any override.
    let (plain_store, _) = build_store(sizes, false);
    let mut plain = World::new(WorldConfig::new(commands, delays, Application::Prepared, settings.worker_counts[0]), plain_store, Prepared::new());
    plain.advance_to(TICKS).expect("the world steps");
    let overridden_world = overridden_world.expect("the prepared application ran");
    let control = compare_with_plain(&store, &overridden_world, &plain);

    let replaced = store.current(asset_id(REPLACED_SET, LEVEL)).expect("generated");
    let expected_failed_children = family_tree(&store, &replaced).iter().filter(|node| node.failed).count() as u64 * PLAN.iter().filter(|plan| plan.set == REPLACED_SET).count() as u64;
    let families = family_reports(&runs[0].events);
    let report = ScenarioReport {
        sizes: *sizes,
        import,
        runs,
        oracle_payload,
        oracle_in_world,
        families,
        identities,
        control,
        prepared_payloads: prepared.len(),
        prepared_instances,
        prepare,
        expected_failed_children,
    };
    tick_log.events(&report);
    eprintln!(
        "{}: determinism {}; payload equals ordered in staging {}; ordered in the world: spawns equal {}, state hash diverges at tick {:?}, content at tick {:?}; families {}; another hash fails {} ({} of {}); identities {} ({} kept of {}, {} overridden, {} changed)",
        sizes.name,
        report.determinism_holds(),
        report.payload_equals_staging(),
        report.in_world_spawns_equal(),
        report.in_world_first_state_divergence(),
        report.in_world_first_content_divergence(),
        report.families_hold(),
        report.another_hash_fails(),
        report.failed_children(),
        report.expected_failed_children,
        report.identities_hold(),
        report.identities[0].1.kept,
        report.identities[0].1.entities,
        report.identities[0].1.overridden,
        report.identities[0].1.changed
    );
    report
}

/// Runs the whole measurement of `settings`.
pub fn measure(settings: &Settings) -> Report {
    let mut tick_log = TickLog::new(settings.log_path.clone());
    let scenarios = settings.sizes.iter().map(|sizes| measure_scenario(sizes, settings, &mut tick_log)).collect();
    tick_log.finish();
    Report { scenarios, log_lines: tick_log.lines, log_path: settings.log_path.clone() }
}

/// The directory the research CI publishes: `out/<cell>/` under the
/// workspace when the spike runs from there, the temporary directory otherwise.
fn output_dir(cell: &str) -> PathBuf {
    let candidate = FilePath::new("out").join(cell);
    if fs::create_dir_all(&candidate).is_ok() {
        return candidate;
    }
    let fallback = std::env::temp_dir().join(format!("{SPIKE}-{cell}"));
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn main() {
    let cell = std::env::args().nth(1).unwrap_or_else(|| "unspecified".to_string());
    let log_path = output_dir(&cell).join(format!("{SPIKE}-ticks.log"));
    // A build with debug assertions measures the small scenes: the report
    // says which settings ran.
    let settings = if cfg!(debug_assertions) { Settings::small(Some(log_path)) } else { Settings::full(log_path) };
    let report = measure(&settings);
    println!("{}", report::write(&cell, &settings, &report));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Record;

    #[test]
    fn the_report_on_the_small_scenes_passes_every_check_but_the_state_hash_of_the_ordered_application_in_the_world() {
        let report = measure(&Settings::small(None));
        assert_eq!(report.scenarios.len(), 1);
        let scenario = &report.scenarios[0];
        assert_eq!(scenario.runs.len(), 6, "three applications at 1 and 3 workers");
        for application in Application::ALL {
            assert!(scenario.identical_across_workers(application), "{} gives one history at 1 and 3 workers", application.name());
            assert!(scenario.matches_oracle(application), "{} equals its oracle at every tick", application.name());
        }
        assert!(scenario.determinism_holds());
        assert!(scenario.import_holds(), "{:?}", scenario.import.planted);
        assert_eq!(scenario.import.named().count(), 13);
        assert!(scenario.import.items_by_kind.iter().all(|count| *count > 0) && scenario.import.items_by_path_length[1] > 0);
        assert!(scenario.payload_equals_staging(), "the ordered application in a staging area gives the prepared payload's world");
        assert!(scenario.in_world_spawns_equal(), "the ordered application in the world spawns the same content by full reference");
        assert_eq!(scenario.in_world_first_state_divergence(), Some(1), "and another allocation from the first spawn on");
        assert!(scenario.families_hold(), "{:#?}", scenario.families);
        assert_eq!(scenario.families.iter().filter(|family| family.plan.appearance == Appearance::OneBatch).map(|family| family.depth).collect::<BTreeSet<_>>(), BTreeSet::from([1, 2, 3]));
        assert!(scenario.another_hash_fails(), "{} of {}", scenario.failed_children(), scenario.expected_failed_children);
        assert!(scenario.identities_hold(), "{:?} {:?}", scenario.identities, scenario.control);
        assert!(scenario.control.removed_by_override > 0 && scenario.control.added_by_override > 0 && scenario.control.inside_removed_instances > 0);
        let first = scenario.first(Application::Prepared);
        assert!(first.gate_polls > 0, "the loading gate held the world");
        assert_eq!(first.log.iter().filter(|logged| !logged.accepted).count(), 3, "the three commands that change nothing");
        assert!(first.log.iter().any(|logged| matches!(logged.record, Record::Arrival { .. })));
        assert!(first.reads.present > 0 && first.reads.absent > 0);
        assert!(scenario.prepared_payloads < scenario.prepared_instances as usize, "instances of one scene under one list share a prepared payload");
        let json = report::write("test", &Settings::small(None), &report);
        assert!(json.starts_with('{') && json.ends_with('}'));
    }
}
