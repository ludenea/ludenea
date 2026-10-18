//! The compare simulation: a second simulation of the game that follows the current tick on
//! the compare branch, so the editor draws that branch's state as a ghost. It runs inside
//! `Host::advance`, a budget of ticks per call, and seeks with the compare branch's own
//! checkpoints.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ludenea_app::Simulation;
use ludenea_debug_draw::DebugDrawFrame;

use crate::branches::BranchId;
use crate::session::{BranchRun, Session, SessionError, panic_message};

/// Most ticks the compare simulation runs per `advance` while playing.
pub const COMPARE_BUDGET_PLAYING: u32 = 4;
/// Most ticks the compare simulation runs per `advance` while paused.
pub const COMPARE_BUDGET_PAUSED: u32 = 240;

/// Where the compare simulation stands after following a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompareState {
    /// The tick of its state, or the tick followed when that is outside the branch's run.
    pub tick: u64,
    /// Whether it has not reached the tick followed yet.
    pub lagging: bool,
    /// Whether the tick followed is past the end of the branch's run.
    pub outside: bool,
}

pub(crate) struct CompareRun {
    sim: Simulation,
    branch: Option<BranchId>,
    run: Option<BranchRun>,
    /// Whether the session may have changed the run since it was composed.
    outdated: bool,
    /// The tick of `sim`'s state, edits recorded at that tick applied; `None` until a
    /// checkpoint is restored.
    at: Option<u64>,
    /// How many times a checkpoint was restored.
    #[cfg(test)]
    restores: u32,
}

impl CompareRun {
    pub fn new(sim: Simulation) -> CompareRun {
        CompareRun {
            sim,
            branch: None,
            run: None,
            outdated: false,
            at: None,
            #[cfg(test)]
            restores: 0,
        }
    }

    /// Composes the run again before the next use; the position stays when the new run
    /// shares its past.
    pub fn invalidate(&mut self) {
        self.outdated = true;
    }

    /// Runs at most `budget` ticks of the session's compare branch toward `tick`. Fails on
    /// a panic of the game, a tick that cannot run, or a fresh tick whose hash differs from
    /// the recorded one; the compare simulation is then unusable.
    pub fn follow(
        &mut self,
        session: &Session,
        tick: u64,
        budget: u32,
    ) -> Result<CompareState, String> {
        let mut running = self.sim.tick();
        let followed = catch_unwind(AssertUnwindSafe(|| {
            self.follow_within(session, tick, budget, &mut running)
        }));
        followed.unwrap_or_else(|payload| {
            Err(SessionError::Panicked {
                tick: running,
                message: panic_message(&*payload),
            }
            .to_string())
        })
    }

    fn follow_within(
        &mut self,
        session: &Session,
        tick: u64,
        budget: u32,
        running: &mut u64,
    ) -> Result<CompareState, String> {
        self.refresh(session);
        let Self {
            sim,
            run,
            at,
            #[cfg(test)]
            restores,
            ..
        } = self;
        let run = run.as_ref().expect("refreshed");
        if tick > run.end() {
            return Ok(CompareState {
                tick,
                lagging: false,
                outside: true,
            });
        }
        let (checkpoint, snapshot) = run.checkpoints.nearest_at_or_before(tick.min(run.fresh));
        // Stepping on beats a checkpoint unless the budget cannot get there by stepping.
        let mut now = match *at {
            Some(now) if now <= tick && (now >= checkpoint || tick - now <= u64::from(budget)) => {
                now
            }
            _ => {
                #[cfg(test)]
                {
                    *restores += 1;
                }
                *at = None;
                // Load addresses and kept assets come from the scene load, not the snapshot.
                session.load_into(sim).map_err(|error| error.to_string())?;
                sim.restore(snapshot).map_err(|error| error.to_string())?;
                apply_edits(sim, run, checkpoint);
                *at = Some(checkpoint);
                checkpoint
            }
        };
        for _ in 0..budget {
            if now >= tick {
                break;
            }
            *running = now;
            sim.step_encoded(run.inputs.tick(now))
                .map_err(|error| error.to_string())?;
            now += 1;
            *at = Some(now);
            let got = sim.hash();
            if now <= run.fresh && run.hashes.get(now as usize) != Some(&got) {
                return Err(format!(
                    "the compare simulation diverged from its branch's recorded hash at tick \
                     {now}, got {got:016x}"
                ));
            }
            apply_edits(sim, run, now);
        }
        Ok(CompareState {
            tick: now,
            lagging: now < tick,
            outside: false,
        })
    }

    /// The ghost at `tick`: the state there, or the last state reached and `lagging`, or an
    /// empty frame outside the compare branch's run or before any state.
    pub fn draw(
        &mut self,
        session: &Session,
        tick: u64,
        layers: &BTreeSet<String>,
    ) -> (DebugDrawFrame, u64, bool) {
        self.refresh(session);
        let run = self.run.as_ref().expect("refreshed");
        match self.at {
            _ if tick > run.end() => (DebugDrawFrame::default(), tick, false),
            Some(at) => (self.sim.debug_draw(layers), at, at != tick),
            None => (DebugDrawFrame::default(), tick, true),
        }
    }

    /// Composes the compare branch's run when it changed, and forgets the position unless
    /// the new run shares the past up to it.
    fn refresh(&mut self, session: &Session) {
        let branch = session.compare_branch();
        if branch != self.branch {
            self.branch = branch;
            self.run = None;
            self.at = None;
        }
        if self.run.is_some() && !self.outdated {
            return;
        }
        self.outdated = false;
        let id = branch.expect("a compare branch is set");
        let defaults = self.sim.encoded_default_inputs();
        let run = session.branch_run(id, defaults.as_deref());
        if let (Some(at), Some(old)) = (self.at, &self.run)
            && !same_past(old, &run, at)
        {
            self.at = None;
        }
        self.run = Some(run);
    }
}

/// Applies the edits recorded at `tick`. One that fails is skipped, as resimulation drops
/// it from a stale run.
fn apply_edits(sim: &mut Simulation, run: &BranchRun, tick: u64) {
    for timed in run.edits.at(tick) {
        let _ = sim.apply_edit(&timed.edit);
    }
}

/// Whether runs `a` and `b` reach the same state at `tick`: the same tick 0 state, recorded
/// hashes, inputs, and edits up to it.
fn same_past(a: &BranchRun, b: &BranchRun, tick: u64) -> bool {
    let at = usize::try_from(tick).expect("tick fits in memory");
    let edits = |run: &BranchRun| run.edits.edits.partition_point(|edit| edit.tick <= tick);
    a.inputs.players == b.inputs.players
        && first(&a.hashes, at + 1) == first(&b.hashes, at + 1)
        && first(&a.inputs.ticks, at) == first(&b.inputs.ticks, at)
        && a.edits.edits[..edits(a)] == b.edits.edits[..edits(b)]
}

/// The first `count` items, or all of them when there are fewer.
fn first<T>(items: &[T], count: usize) -> &[T] {
    &items[..count.min(items.len())]
}

#[cfg(test)]
mod tests {
    use ludenea_app::{AssetTexts, Edit, EntityRef};

    use super::*;
    use crate::session::Document;

    #[test]
    fn following_play_steps_across_checkpoints_without_restoring() {
        let mut sim = boxes::app().build().unwrap();
        let scene = boxes::generate_scene(10, 3);
        let mut session =
            Session::start(&mut sim, scene, AssetTexts::new(), Document::Scene).unwrap();
        for _ in 0..100 {
            session.step(&mut sim, None).unwrap();
        }
        // An edit at tick 10 forks; the root, ahead to tick 100, becomes the compare branch.
        session.seek(&mut sim, 10).unwrap();
        let entity = *sim.load_addresses().keys().next().unwrap();
        let value = sim
            .world()
            .component_bytes(entity, "ludenea::Pose")
            .unwrap();
        let edit = Edit::Insert {
            entity: EntityRef::Runtime(entity),
            component: "ludenea::Pose".to_string(),
            value,
        };
        session.edit(&mut sim, edit).unwrap();
        assert_eq!(session.compare_branch(), Some(BranchId::ROOT));

        let mut compare = CompareRun::new(boxes::app().build().unwrap());
        compare.follow(&session, 10, COMPARE_BUDGET_PAUSED).unwrap();
        assert_eq!(compare.restores, 1);
        // Play at one tick per advance crosses the checkpoints at 16, 32, and 48.
        for tick in 11..=50 {
            let state = compare
                .follow(&session, tick, COMPARE_BUDGET_PLAYING)
                .unwrap();
            assert_eq!((state.tick, state.lagging), (tick, false));
        }
        assert_eq!(compare.restores, 1);
    }
}
