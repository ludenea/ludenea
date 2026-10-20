//! The causal divergence report: two simulations run the current and the compare branch
//! from the tick their runs part, with introspection recording, and their entity hash
//! trees are compared tick by tick. Each entity that diverges is reported once, at its
//! first divergent tick, with its first differing field and the events and commands that
//! may have caused it.

use std::collections::BTreeMap;

use ludenea_app::{Edit, EntityRef, Simulation};
use ludenea_ecs::diff::entity_subject;
use ludenea_ecs::schema::TypeSchema;
use ludenea_ecs::schema::entities::entity_refs;
use ludenea_ecs::{
    CommandKind, CommandRecord, Difference, Entity, EntityHash, ScheduleObserver, Snapshot,
    SnapshotError, World,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::branches::BranchId;
use crate::compare::{apply_edits, restore, same_past};
use crate::session::{BranchRun, Session};

/// How many ticks after the fork a report covers unless asked for another range.
pub const DIVERGENCE_TICKS: u64 = 600;

/// How the current branch diverges from the compare branch, from the tick their runs part
/// up to `up_to`, the last tick compared so far.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DivergenceReport {
    pub fork_tick: u64,
    pub up_to: u64,
    /// Whether every tick of the range is compared.
    pub complete: bool,
    /// The first tick whose world hashes differ.
    pub first_divergent: Option<u64>,
    pub root_cause: Option<RootCause>,
    /// Ordered by first divergent tick, then entity.
    pub entities: Vec<EntityDivergence>,
}

/// What made the branches part, at the fork tick, and the earliest divergent entities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RootCause {
    pub tick: u64,
    pub kind: RootCauseKind,
    pub entities: Vec<Entity>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RootCauseKind {
    /// An edit only one branch records, such as `insert ludenea::Body on 3v0`.
    Edit { description: String },
    /// A different input: a live take-over.
    TakeOver,
}

/// One entity, matched across the branches by identifier, at its first divergent tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EntityDivergence {
    pub entity: Entity,
    pub kind: DivergenceKind,
    pub first_tick: u64,
    /// The first differing component and field of a `Changed` entity.
    pub first_difference: Option<Difference>,
    /// Empty when no recorded event or command links the entity to an already divergent
    /// one, such as a resting contact: touching bodies push each other without an event.
    pub causes: Vec<Cause>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    Changed,
    OnlyInCurrent,
    OnlyInCompare,
    /// Alive in both with different component sets: likely unrelated entities that got the
    /// same identifier.
    Replaced,
}

/// A cause candidate, in either branch, at the entity's first divergent tick or the tick
/// before. Only events and commands are recorded, so interactions without either (a
/// persistent contact, a system reading another entity) leave no candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Cause {
    /// An event by its stable name that references the entity at `path` and `other`, an
    /// entity already divergent at `tick`.
    Event {
        tick: u64,
        event: String,
        path: String,
        other: Entity,
    },
    /// A command targeting the entity, `kind` being `spawn`, `insert`, `remove`, or
    /// `despawn`, issued by `system`.
    Command {
        tick: u64,
        system: String,
        kind: String,
    },
}

/// The current branch's simulation first, then the compare branch's.
const CURRENT: usize = 0;
const COMPARE: usize = 1;

pub(crate) struct DivergenceRun {
    sims: [Simulation; 2],
    event_schemas: BTreeMap<&'static str, TypeSchema>,
    systems: Vec<&'static str>,
    /// The last tick asked for, or the default range.
    requested: Option<u64>,
    /// Whether the session may have changed the runs since they were composed.
    outdated: bool,
    progress: Option<Progress>,
    /// How many times the runs were composed.
    #[cfg(test)]
    pub composed: u32,
}

/// The report as far as it is computed, for one pair of runs.
struct Progress {
    branches: [BranchId; 2],
    runs: [BranchRun; 2],
    fork: u64,
    cause: RootCauseKind,
    target: u64,
    /// The tick both simulations stand at and compared; `None` until they get to the fork.
    at: Option<u64>,
    /// Snapshot parts every entity diff shares: its components are empty.
    skeleton: Option<Snapshot>,
    first_divergent: Option<u64>,
    entities: Vec<EntityDivergence>,
    /// The first divergent tick of every reported entity, by index and generation.
    divergent: BTreeMap<(u32, u32), (Entity, u64)>,
    /// Per simulation, the records of the last two ticks run.
    records: [Vec<TickRecord>; 2],
    /// Problems not yet reported, such as a value that failed to decode for a diff.
    warnings: Vec<String>,
}

impl DivergenceRun {
    pub fn new(current: Simulation, compare: Simulation) -> DivergenceRun {
        let world = current.world();
        DivergenceRun {
            event_schemas: world.event_schemas().into_iter().collect(),
            systems: current
                .systems()
                .into_iter()
                .map(|info| info.name)
                .collect(),
            sims: [current, compare],
            requested: None,
            outdated: false,
            progress: None,
            #[cfg(test)]
            composed: 0,
        }
    }

    /// Composes the runs again before the next use; the report so far stays when the new
    /// runs share their past up to it.
    pub fn invalidate(&mut self) {
        self.outdated = true;
    }

    /// Like `invalidate` after play ran ticks: play only records the current branch's
    /// future, which matters only while the range is capped at the current branch's end. A
    /// take-over that forks changes the current branch, which every use checks.
    pub fn played(&mut self) {
        if let Some(progress) = &self.progress
            && progress.target < self.full_target(progress.fork)
            && progress.runs[COMPARE].end() > progress.target
        {
            self.outdated = true;
        }
    }

    /// The end of the range asked for, before capping it at the end of the runs.
    fn full_target(&self, fork: u64) -> u64 {
        self.requested
            .unwrap_or(fork.saturating_add(DIVERGENCE_TICKS))
            .max(fork)
    }

    /// The problems found since the last call, for the console.
    pub fn take_warnings(&mut self) -> Vec<String> {
        self.progress
            .as_mut()
            .map(|progress| std::mem::take(&mut progress.warnings))
            .unwrap_or_default()
    }

    /// Asks for the range up to `up_to`, or the default one.
    pub fn request(&mut self, up_to: Option<u64>) {
        if up_to != self.requested {
            self.requested = up_to;
            self.outdated = true;
        }
    }

    /// The report as far as it is computed. Fails when there is no compare branch or the
    /// branches run the same ticks.
    pub fn report(&mut self, session: &Session) -> Result<DivergenceReport, String> {
        self.refresh(session)?;
        let progress = self.progress.as_ref().expect("refreshed");
        let first_divergent = progress.first_divergent;
        Ok(DivergenceReport {
            fork_tick: progress.fork,
            up_to: progress.at.unwrap_or(progress.fork),
            complete: progress.complete(),
            first_divergent,
            root_cause: first_divergent.map(|_| RootCause {
                tick: progress.fork,
                kind: progress.cause.clone(),
                // The earliest divergent entities: a resource or asset edit diverges the
                // world before any entity.
                entities: progress
                    .entities
                    .iter()
                    .take_while(|divergence| {
                        Some(divergence.first_tick)
                            == progress.entities.first().map(|first| first.first_tick)
                    })
                    .map(|divergence| divergence.entity)
                    .collect(),
            }),
            entities: progress.entities.clone(),
        })
    }

    /// Compares at most `budget` more ticks. Returns the last tick compared when it moved,
    /// and whether the report is complete or there is nothing to compare. Fails on a panic
    /// of the game in a tick or an edit, or a tick that cannot run; the run is then
    /// unusable. Any other panic unwinds to the host's boundary, which builds it again.
    pub fn advance(
        &mut self,
        session: &Session,
        budget: u32,
    ) -> Result<(Option<u64>, bool), String> {
        // Nothing to compare is no failure: `report` says why.
        if self.refresh(session).is_err() {
            return Ok((None, true));
        }
        let Self {
            sims,
            event_schemas,
            systems,
            progress,
            ..
        } = self;
        let progress = progress.as_mut().expect("refreshed");
        if progress.complete() {
            return Ok((None, true));
        }
        let mut now = match progress.at {
            Some(at) => at,
            None => {
                let fork = progress.fork;
                progress.start(sims, session)?;
                progress.at = Some(fork);
                progress.compare(sims, event_schemas, systems, fork);
                fork
            }
        };
        for _ in 0..budget {
            if now >= progress.target {
                break;
            }
            for (side, sim) in sims.iter_mut().enumerate() {
                let run = &progress.runs[side];
                let mut recorder = Recorder::default();
                sim.step_observed(run.inputs.tick(now), &mut recorder)
                    .map_err(|error| error.to_string())?;
                apply_edits(sim, run, now + 1)?;
                let records = &mut progress.records[side];
                if records.len() == 2 {
                    records.remove(0);
                }
                records.push(TickRecord {
                    tick: now + 1,
                    ..recorder.record
                });
            }
            now += 1;
            progress.at = Some(now);
            progress.compare(sims, event_schemas, systems, now);
        }
        Ok((Some(now), progress.complete()))
    }

    /// Composes the runs when they may have changed, and starts over unless the new runs
    /// share the past up to the tick compared.
    fn refresh(&mut self, session: &Session) -> Result<(), String> {
        let compare = session
            .compare_branch()
            .ok_or("there is no compare branch to compare the current branch with")?;
        let branches = [session.current_branch(), compare];
        let same_branches = self
            .progress
            .as_ref()
            .is_some_and(|progress| progress.branches == branches);
        if same_branches && !self.outdated {
            return Ok(());
        }
        self.outdated = false;
        let defaults = self.sims[CURRENT].encoded_default_inputs();
        let runs = branches.map(|id| session.branch_run(id, defaults.as_deref()));
        let (fork, cause) = fork_point(&runs).ok_or_else(|| {
            self.progress = None;
            "the current and compare branches run the same ticks".to_string()
        })?;
        let end = runs[CURRENT].end().min(runs[COMPARE].end());
        #[cfg(test)]
        {
            self.composed += 1;
        }
        let target = self.full_target(fork).min(end).max(fork);
        if let Some(progress) = &mut self.progress
            && same_branches
            && progress.fork == fork
            && progress.at.is_none_or(|at| {
                at <= target
                    && same_past(&progress.runs[CURRENT], &runs[CURRENT], at)
                    && same_past(&progress.runs[COMPARE], &runs[COMPARE], at)
            })
        {
            progress.runs = runs;
            progress.target = target;
            return Ok(());
        }
        self.progress = Some(Progress {
            branches,
            runs,
            fork,
            cause,
            target,
            at: None,
            skeleton: None,
            first_divergent: None,
            entities: Vec::new(),
            divergent: BTreeMap::new(),
            records: [Vec::new(), Vec::new()],
            warnings: Vec::new(),
        });
        Ok(())
    }
}

impl Progress {
    fn complete(&self) -> bool {
        self.at == Some(self.target)
    }

    /// Brings both simulations to the fork tick, edits recorded there applied. The tick
    /// that ends at the fork runs observed: its events may explain a divergence one tick
    /// after the fork.
    fn start(&mut self, sims: &mut [Simulation; 2], session: &Session) -> Result<(), String> {
        let observed = self.fork.saturating_sub(1);
        for (side, sim) in sims.iter_mut().enumerate() {
            let run = &self.runs[side];
            let (checkpoint, snapshot) = run.checkpoints.nearest_at_or_before(observed);
            restore(sim, session, run, checkpoint, snapshot)?;
            for tick in checkpoint..self.fork {
                let input = run.inputs.tick(tick);
                if tick == observed {
                    let mut recorder = Recorder::default();
                    sim.step_observed(input, &mut recorder)
                        .map_err(|error| error.to_string())?;
                    self.records[side].push(TickRecord {
                        tick: self.fork,
                        ..recorder.record
                    });
                } else {
                    sim.step_encoded(input).map_err(|error| error.to_string())?;
                }
                apply_edits(sim, run, tick + 1)?;
            }
        }
        let mut skeleton = sims[CURRENT].world().snapshot();
        for (_, values) in &mut skeleton.components {
            values.clear();
        }
        self.skeleton = Some(skeleton);
        Ok(())
    }

    /// Compares the simulations, both at `tick`, and reports the entities that diverge
    /// there for the first time.
    fn compare(
        &mut self,
        sims: &[Simulation; 2],
        event_schemas: &BTreeMap<&'static str, TypeSchema>,
        systems: &[&'static str],
        tick: u64,
    ) {
        let trees = [sims[CURRENT].hash_tree(), sims[COMPARE].hash_tree()];
        if trees[CURRENT].root == trees[COMPARE].root {
            return;
        }
        self.first_divergent.get_or_insert(tick);
        let mut sides: BTreeMap<Entity, [Option<&EntityHash>; 2]> = BTreeMap::new();
        for (side, tree) in trees.iter().enumerate() {
            for leaf in &tree.entities {
                sides.entry(leaf.entity).or_default()[side] = Some(leaf);
            }
        }
        let mut found = Vec::new();
        for (entity, sides) in sides {
            if self.divergent.contains_key(&key(entity)) {
                continue;
            }
            let kind = match sides {
                [Some(current), Some(compare)] if current.hash == compare.hash => continue,
                [Some(current), Some(compare)] => {
                    let names = |leaf: &EntityHash| {
                        leaf.components
                            .iter()
                            .map(|component| component.name.clone())
                            .collect::<Vec<_>>()
                    };
                    if names(current) == names(compare) {
                        DivergenceKind::Changed
                    } else {
                        DivergenceKind::Replaced
                    }
                }
                [Some(_), None] => DivergenceKind::OnlyInCurrent,
                [None, Some(_)] => DivergenceKind::OnlyInCompare,
                [None, None] => unreachable!("listed by a tree"),
            };
            self.divergent.insert(key(entity), (entity, tick));
            found.push((entity, kind));
        }
        let changed: Vec<Entity> = found
            .iter()
            .filter(|(_, kind)| *kind == DivergenceKind::Changed)
            .map(|(entity, _)| *entity)
            .collect();
        let mut differences = self.differences(sims, &changed).unwrap_or_else(|error| {
            self.warnings.push(format!(
                "tick {tick}: the differences could not be listed: {error}"
            ));
            BTreeMap::new()
        });
        for (entity, kind) in found {
            let causes = self.causes(entity, tick, event_schemas, systems);
            self.entities.push(EntityDivergence {
                entity,
                kind,
                first_tick: tick,
                first_difference: differences.remove(&entity),
                causes,
            });
        }
    }

    /// The first difference of each of `entities`, through snapshots restricted to them.
    fn differences(
        &self,
        sims: &[Simulation; 2],
        entities: &[Entity],
    ) -> Result<BTreeMap<Entity, Difference>, SnapshotError> {
        if entities.is_empty() {
            return Ok(BTreeMap::new());
        }
        let skeleton = self.skeleton.as_ref().expect("started");
        let restricted = |world: &World| Snapshot {
            components: skeleton
                .components
                .iter()
                .map(|(name, _)| {
                    let values = entities
                        .iter()
                        .filter_map(|&entity| Some((entity, world.component_bytes(entity, name)?)))
                        .collect();
                    (name.clone(), values)
                })
                .collect(),
            ..skeleton.clone()
        };
        let world = sims[CURRENT].world();
        let (current, compare) = (restricted(world), restricted(sims[COMPARE].world()));
        let subjects: BTreeMap<String, Entity> = entities
            .iter()
            .map(|&entity| (entity_subject(entity), entity))
            .collect();
        let mut first = BTreeMap::new();
        for difference in world.diff_snapshots(&current, &compare)? {
            if let Some(&entity) = subjects.get(&difference.subject) {
                first.entry(entity).or_insert(difference);
            }
        }
        Ok(first)
    }

    /// The events and commands of either branch, at `tick` and the tick before, that
    /// explain why `entity` diverges at `tick`.
    fn causes(
        &self,
        entity: Entity,
        tick: u64,
        event_schemas: &BTreeMap<&'static str, TypeSchema>,
        systems: &[&'static str],
    ) -> Vec<Cause> {
        let mut causes = Vec::new();
        let mut add = |cause: Cause| {
            if !causes.contains(&cause) {
                causes.push(cause);
            }
        };
        // The tick ending at the fork ran before the edits that part the branches: it
        // explains only later divergences.
        let explains = |record: &&TickRecord| {
            tick > self.fork && record.tick + 1 >= tick && record.tick <= tick
        };
        let records = self.records.iter().flatten();
        for record in records.filter(explains) {
            for (name, value) in &record.events {
                let Some(schema) = event_schemas.get(name) else {
                    continue;
                };
                let refs = entity_refs(schema, value);
                let Some(own) = refs
                    .iter()
                    .find(|at| (at.index, at.generation) == key(entity))
                else {
                    continue;
                };
                for at in &refs {
                    match self.divergent.get(&(at.index, at.generation)) {
                        Some(&(other, since)) if other != entity && since <= record.tick => {
                            add(Cause::Event {
                                tick: record.tick,
                                event: name.to_string(),
                                path: own.path.clone(),
                                other,
                            });
                        }
                        _ => {}
                    }
                }
            }
            for (index, command) in &record.commands {
                if command.entity == Some(entity) {
                    add(Cause::Command {
                        tick: record.tick,
                        system: systems[*index].to_string(),
                        kind: command_kind(command.kind).to_string(),
                    });
                }
            }
        }
        causes
    }
}

fn key(entity: Entity) -> (u32, u32) {
    (entity.index(), entity.generation())
}

/// The first tick at which `runs` differ and why: an edit only one of them records there,
/// otherwise a different input. `None` when they run the same ticks.
fn fork_point(runs: &[BranchRun; 2]) -> Option<(u64, RootCauseKind)> {
    let [current, compare] = [&runs[CURRENT].edits.edits, &runs[COMPARE].edits.edits];
    let edit = match current.iter().zip(compare).position(|(a, b)| a != b) {
        Some(at) if current[at].tick <= compare[at].tick => Some(&current[at]),
        Some(at) => Some(&compare[at]),
        None => current
            .get(compare.len())
            .or_else(|| compare.get(current.len())),
    };
    let inputs = [&runs[CURRENT].inputs.ticks, &runs[COMPARE].inputs.ticks];
    let input = inputs[CURRENT]
        .iter()
        .zip(inputs[COMPARE])
        .position(|(a, b)| a != b)
        .map(|at| at as u64);
    // An edit at the tick of a different input applies before that input runs.
    let edit = edit.filter(|edit| input.is_none_or(|input| edit.tick <= input));
    match (edit, input) {
        (Some(edit), _) => Some((
            edit.tick,
            RootCauseKind::Edit {
                description: describe(&edit.edit),
            },
        )),
        (None, Some(input)) => Some((input, RootCauseKind::TakeOver)),
        (None, None) => None,
    }
}

/// An edit as text, such as `"insert ludenea::Body on 3v0"`.
fn describe(edit: &Edit) -> String {
    let target = |entity: &EntityRef| match entity {
        EntityRef::Runtime(entity) => entity.to_string(),
        EntityRef::Loaded(address) => {
            let address: Vec<String> = address.iter().map(u32::to_string).collect();
            format!("the scene entity at {}", address.join("/"))
        }
    };
    match edit {
        Edit::Spawn { components } => {
            let names: Vec<&str> = components.iter().map(|(name, _)| name.as_str()).collect();
            format!("spawn with {}", names.join(", "))
        }
        Edit::Despawn { entity } => format!("despawn {}", target(entity)),
        Edit::Insert {
            entity, component, ..
        } => format!("insert {component} on {}", target(entity)),
        Edit::Remove { entity, component } => format!("remove {component} from {}", target(entity)),
        Edit::SetResource { resource, .. } => format!("set {resource}"),
        Edit::SetAsset { id, .. } => format!("set asset {id}"),
        Edit::LoadAsset { id, .. } => format!("load asset {id}"),
        Edit::Group(edits) => {
            let members: Vec<String> = edits.iter().map(describe).collect();
            format!("{} edits: {}", edits.len(), members.join("; "))
        }
    }
}

fn command_kind(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Spawn => "spawn",
        CommandKind::Insert => "insert",
        CommandKind::Remove => "remove",
        CommandKind::Despawn => "despawn",
    }
}

/// The events and commands of the tick that ended at `tick`, in one simulation.
#[derive(Default)]
struct TickRecord {
    tick: u64,
    events: Vec<(&'static str, Value)>,
    /// Each command with the index of the system that issued it.
    commands: Vec<(usize, CommandRecord)>,
}

#[derive(Default)]
struct Recorder {
    record: TickRecord,
}

impl ScheduleObserver for Recorder {
    fn before_system(&mut self, _: usize) {}

    fn after_system(&mut self, index: usize, commands: &[CommandRecord]) {
        self.record
            .commands
            .extend(commands.iter().map(|command| (index, *command)));
    }

    fn tick_end(&mut self, world: &World) {
        for (name, values) in world.events_json() {
            self.record
                .events
                .extend(values.into_iter().map(|value| (name, value)));
        }
    }
}

#[cfg(test)]
mod tests {
    use ludenea_app::AssetTexts;
    use serde_json::json;

    use super::*;
    use crate::session::Document;

    #[test]
    fn the_tick_ending_at_the_fork_is_observed() {
        let mut sim = boxes::app().build().unwrap();
        let scene = boxes::generate_scene(10, 3);
        let mut session =
            Session::start(&mut sim, scene, AssetTexts::new(), Document::Scene).unwrap();
        for _ in 0..40 {
            session.step(&mut sim, None).unwrap();
        }
        session.seek(&mut sim, 20).unwrap();
        let value = sim
            .encode_json_value(
                ludenea_app::ValueKind::Component,
                "boxes::Crate",
                json!(null),
            )
            .unwrap();
        let components = vec![("boxes::Crate".to_string(), value)];
        session.edit(&mut sim, Edit::Spawn { components }).unwrap();
        for _ in 0..10 {
            session.step(&mut sim, None).unwrap();
        }

        let build = || boxes::app().build().unwrap();
        let mut divergence = DivergenceRun::new(build(), build());
        divergence.advance(&session, 1).unwrap();
        let progress = divergence.progress.as_ref().unwrap();
        for records in &progress.records {
            let ticks: Vec<u64> = records.iter().map(|record| record.tick).collect();
            assert_eq!(ticks, [20, 21]);
        }
    }
}
