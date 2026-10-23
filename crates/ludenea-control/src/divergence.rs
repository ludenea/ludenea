//! The causal divergence report: two simulations run the current and the compare branch
//! from the tick their runs part, with introspection recording, and their entity hash
//! trees are compared tick by tick. Each entity that diverges is reported once, at its
//! first divergent tick, with its first differing field and the events and commands that
//! may have caused it.

use ludenea_app::divergence::{EntityComparison, TickRecord, TickRecorder};
use ludenea_app::{Edit, EntityRef, Simulation};
use ludenea_ecs::Entity;
use serde::{Deserialize, Serialize};

use crate::branches::BranchId;
use crate::compare::{apply_edits, restore, same_past};
use crate::session::{BranchRun, Session};

pub use ludenea_app::divergence::{Cause, DivergenceKind, EntityDivergence};

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

/// The current branch's simulation first, then the compare branch's.
const CURRENT: usize = 0;
const COMPARE: usize = 1;

pub(crate) struct DivergenceRun {
    sims: [Simulation; 2],
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
    /// The entities compared so far; `None` until the simulations get to the fork.
    comparison: Option<EntityComparison>,
    entities: Vec<EntityDivergence>,
    /// Per simulation, the records of the last two ticks run.
    records: [Vec<TickRecord>; 2],
}

impl DivergenceRun {
    pub fn new(current: Simulation, compare: Simulation) -> DivergenceRun {
        DivergenceRun {
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

    /// The tick of the simulation furthest ahead.
    pub fn tick(&self) -> u64 {
        self.sims[CURRENT].tick().max(self.sims[COMPARE].tick())
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
            .and_then(|progress| progress.comparison.as_mut())
            .map(EntityComparison::take_warnings)
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
        let first_divergent = progress
            .comparison
            .as_ref()
            .and_then(EntityComparison::first_divergent);
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
    /// unusable. Any other panic unwinds to the caller, which stops the report.
    pub fn advance(
        &mut self,
        session: &Session,
        budget: u32,
    ) -> Result<(Option<u64>, bool), String> {
        // Nothing to compare is no failure: `report` says why.
        if self.refresh(session).is_err() {
            return Ok((None, true));
        }
        let Self { sims, progress, .. } = self;
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
                progress.compare(sims)?;
                fork
            }
        };
        for _ in 0..budget {
            if now >= progress.target {
                break;
            }
            for (side, sim) in sims.iter_mut().enumerate() {
                let run = &progress.runs[side];
                let mut recorder = TickRecorder::default();
                sim.step_observed(run.inputs.tick(now), &mut recorder)
                    .map_err(|error| error.to_string())?;
                apply_edits(sim, run, now + 1)?;
                let records = &mut progress.records[side];
                if records.len() == 2 {
                    records.remove(0);
                }
                records.push(recorder.into_record(now + 1));
            }
            now += 1;
            progress.at = Some(now);
            progress.compare(sims)?;
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
            comparison: None,
            entities: Vec::new(),
            records: [Vec::new(), Vec::new()],
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
                    let mut recorder = TickRecorder::default();
                    sim.step_observed(input, &mut recorder)
                        .map_err(|error| error.to_string())?;
                    self.records[side].push(recorder.into_record(self.fork));
                } else {
                    sim.step_encoded(input).map_err(|error| error.to_string())?;
                }
                apply_edits(sim, run, tick + 1)?;
            }
        }
        self.comparison = Some(EntityComparison::new(&sims[CURRENT]));
        Ok(())
    }

    /// Compares the simulations, both at the tick compared, and reports the entities that
    /// diverge there for the first time. Fails only if they stand at different ticks, which
    /// `advance` never lets happen.
    fn compare(&mut self, sims: &[Simulation; 2]) -> Result<(), String> {
        let comparison = self.comparison.as_mut().expect("started");
        let [current, compare] = &self.records;
        // The tick ending at the fork ran before the edits that part the branches: it
        // explains only later divergences.
        let found = comparison
            .compare(
                [&sims[CURRENT], &sims[COMPARE]],
                [current, compare],
                self.fork.saturating_add(1),
            )
            .map_err(|error| error.to_string())?;
        self.entities.extend(found);
        Ok(())
    }
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
