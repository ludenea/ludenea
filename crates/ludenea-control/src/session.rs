//! The editor session: a scene plus recorded inputs and edits, simulated along a linear
//! timeline that can be stepped, sought, edited at any tick, and resimulated.
//!
//! State invariant: the world at tick `t` always includes the edits recorded at `t`.
//! `hashes[t]` is the hash after `t` ticks, before the edits recorded at `t`; the
//! checkpoint at `t` is taken at that same moment. Running tick `t + 1` therefore never
//! applies the edits of `t` again; after it runs, the session records `hashes[t + 1]` and
//! the checkpoint, then applies the edits recorded at `t + 1`. Seeking to `t` restores the
//! nearest checkpoint `c <= t`, applies the edits of `c`, and repeats "run tick, apply its
//! edits" up to `t`.
//!
//! Fork rule: an edit past tick 0, or a take-over, at tick `N` while the current branch has
//! recorded ticks after `N` creates a child branch forked at `(N, edits already recorded at
//! N)` and makes it current; the parent keeps its future. With nothing recorded after `N`,
//! the edit or input extends the current branch, so playing forward never forks. The
//! current branch lives flattened in the session's logs; the others are stored in the
//! branch tree.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ludenea_app::{
    Edit, EditError, EditLog, InputLog, NoAssets, SceneError, SimError, Simulation, TimedEdit,
};
use ludenea_ecs::{Entity, Snapshot, SnapshotError};
use serde::{Deserialize, Serialize};

use crate::branches::{BranchId, BranchInfo, BranchTree, BranchesError, Segment};
use crate::checkpoints::{CHECKPOINT_BUDGET, Checkpoints};
use crate::subscriptions::Introspection;
use crate::undo::{UndoEntry, UndoHistory};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("tick {tick} is past the recorded head {head}; seek to a recorded tick")]
    InvalidTick { tick: u64, head: u64 },
    #[error("the session is resimulated up to tick {up_to} only; wait for resimulation to finish")]
    Resimulating { up_to: u64 },
    #[error("the game panicked while running tick {tick}: {message}")]
    Panicked { tick: u64, message: String },
    #[error("there is nothing to undo")]
    NothingToUndo,
    #[error("there is nothing to redo")]
    NothingToRedo,
    #[error("the undo history cannot be decoded: {0}")]
    UndoDecode(#[from] postcard::Error),
    #[error(transparent)]
    Edit(#[from] EditError),
    /// The message leaves the entity out: callers name it in their context.
    #[error("the entity has no `{component}`; insert the component first")]
    MissingComponent { entity: Entity, component: String },
    #[error(
        "the entity was spawned after tick 0, so the scene does not have it; only entities of the scene can be applied to it"
    )]
    NotInScene(Entity),
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error(transparent)]
    Scene(#[from] SceneError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error("there is no branch {}", .0.0)]
    NoSuchBranch(BranchId),
    #[error("the edit to undo, at tick {tick}, was recorded on another branch; switch to it first")]
    NotOnBranch { tick: u64 },
    #[error("the root branch cannot be deleted")]
    RootBranch,
    #[error("the branch tree cannot be decoded: {0}")]
    BranchesDecode(#[from] BranchesError),
}

/// What the timeline shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Timeline {
    pub head: u64,
    pub simulated: u64,
    pub current: u64,
    pub checkpoints: Vec<u64>,
    pub edit_ticks: Vec<u64>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub scene_dirty: bool,
    pub resimulating: bool,
    pub branch: BranchId,
    pub branches: Vec<BranchInfo>,
}

/// One report of `Session::resimulate`.
#[derive(Clone, Debug, PartialEq)]
pub enum Resimulation {
    /// Nothing to resimulate.
    Idle,
    Progress {
        up_to: u64,
    },
    /// The first tick whose new hash differs from the recorded one; reported once.
    Diverged {
        tick: u64,
        expected: u64,
        got: u64,
    },
    /// A recorded edit failed on the new state and was removed from the log.
    EditDropped {
        tick: u64,
        reason: String,
    },
    Done,
}

/// Everything needed to replay a session elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionExport {
    pub scene: String,
    /// The scene text as last loaded or saved, when it differs from `scene`: the unsaved
    /// edits stay unsaved after a resume.
    pub saved_scene: Option<String>,
    pub inputs: Vec<u8>,
    pub edits: Vec<u8>,
    /// The undo and redo stacks, so a rebuild keeps the history.
    pub undo: Vec<u8>,
    pub hashes: Vec<u64>,
    /// The other branches, the current one, and the names; empty for a linear session.
    /// `inputs`, `edits`, and `hashes` are the current branch's flattened run.
    pub branches: Vec<u8>,
}

pub struct Session {
    scene: String,
    inputs: InputLog,
    edits: EditLog,
    hashes: Vec<u64>,
    checkpoints: Checkpoints,
    /// The last tick with a recorded input or edit.
    head: u64,
    current: u64,
    /// The scene text as last loaded or saved; the scene is dirty while it differs.
    saved_scene: String,
    /// Whether `scene` differs from `saved_scene`, kept by `set_scene`: comparing large
    /// scenes on every timeline report would cost more than the tick.
    scene_dirty: bool,
    history: UndoHistory,
    /// The last resimulated tick, while the recorded future is stale.
    resimulating: Option<u64>,
    /// The tick the session was at when its scene changed: resimulation seeks back to it
    /// once it gets there, unless the session moves first.
    return_to: Option<u64>,
    diverged: bool,
    reports: VecDeque<Resimulation>,
    /// Records every tick run while the `Introspection` topic is subscribed.
    pub(crate) introspection: Option<Introspection>,
    tree: BranchTree,
}

impl Session {
    /// Loads `scene` at tick 0 with empty logs.
    pub fn start(sim: &mut Simulation, scene: String) -> Result<Session, SessionError> {
        sim.load_scene_with(&scene, &NoAssets)?;
        let players = u16::try_from(sim.players()).expect("fewer than 2^16 players");
        Ok(Session {
            saved_scene: scene.clone(),
            scene,
            scene_dirty: false,
            inputs: InputLog::new(players),
            edits: EditLog::new(),
            hashes: vec![sim.hash()],
            checkpoints: Checkpoints::new(sim.snapshot()),
            head: 0,
            current: 0,
            history: UndoHistory::default(),
            resimulating: None,
            return_to: None,
            diverged: false,
            reports: VecDeque::new(),
            introspection: None,
            tree: BranchTree::default(),
        })
    }

    /// Restarts a recorded session on a new build: the new hashes are compared with
    /// `expected` while it resimulates. `undo` is the exported undo history and `branches`
    /// the exported branch tree; empty bytes mean an empty history and a linear session.
    /// `saved_scene` is the scene as last saved when it differs from `scene`. The other
    /// branches are stale until switched to.
    #[allow(clippy::too_many_arguments)]
    pub fn resume(
        sim: &mut Simulation,
        scene: String,
        saved_scene: Option<String>,
        inputs: InputLog,
        edits: EditLog,
        undo: &[u8],
        branches: &[u8],
        expected: Vec<u64>,
    ) -> Result<Session, SessionError> {
        let history = if undo.is_empty() {
            UndoHistory::default()
        } else {
            UndoHistory::decode(undo)?
        };
        sim.set_players(usize::from(inputs.players));
        let mut session = Session::start(sim, scene)?;
        if let Some(saved) = saved_scene {
            session.scene_dirty = saved != session.scene;
            session.saved_scene = saved;
        }
        session.inputs = inputs;
        session.edits = edits;
        session.history = history;
        if !branches.is_empty() {
            session.tree = BranchTree::decode(branches)?;
        }
        if !expected.is_empty() {
            session.hashes = expected;
        }
        session.update_head();
        let (expected, got) = (session.hashes[0], sim.hash());
        session.start_resimulation(sim);
        if expected != got {
            session.diverged = true;
            session.reports.push_front(Resimulation::Diverged {
                tick: 0,
                expected,
                got,
            });
        }
        Ok(session)
    }

    /// Replaces the recorded inputs and seeks to tick 0.
    pub fn load_inputs(&mut self, sim: &mut Simulation, log: InputLog) -> Result<(), SessionError> {
        sim.set_players(usize::from(log.players));
        self.inputs = log;
        self.reset_future(sim)
    }

    /// Replaces the recorded edits and seeks to tick 0. The edits are checked by
    /// resimulation: those that fail are dropped and reported by `resimulate`.
    pub fn load_edits(&mut self, sim: &mut Simulation, log: EditLog) -> Result<(), SessionError> {
        self.edits = log;
        // Undo entries past tick 0 name edits of the replaced log.
        self.history.discard_after(0);
        self.reset_future(sim)
    }

    /// Runs one tick and returns the new tick. Below the head it replays the recorded input
    /// unless `live` differs from it, which takes over: a branch forks at the current tick
    /// and records `live`. Past the end of the recorded inputs, `live` or
    /// default inputs are recorded.
    pub fn step(&mut self, sim: &mut Simulation, live: Option<&[u8]>) -> Result<u64, SessionError> {
        let tick = self.current;
        if let Some(up_to) = self.resimulating
            && tick >= up_to
        {
            return Err(SessionError::Resimulating { up_to });
        }
        self.return_to = None;
        let recorded = self.inputs.tick(tick).map(<[u8]>::to_vec);
        let take_over = live.filter(|live| recorded.as_deref() != Some(*live));
        self.run_tick(sim, take_over.or(recorded.as_deref()))?;
        if let Some(live) = take_over {
            self.fork_or_discard_after(tick);
            self.record_input(sim, tick, live.to_vec());
        } else if recorded.is_none()
            && tick >= self.head
            && let Some(defaults) = sim.encoded_default_inputs()
        {
            self.record_input(sim, tick, defaults);
        }
        self.record_tick(sim);
        if take_over.is_some() {
            // No edits are recorded at the new tick: the take-over discarded them.
            self.keep_checkpoint_if_sparse(sim);
        }
        self.current = tick + 1;
        self.apply_edits(sim, self.current)?;
        Ok(self.current)
    }

    /// Applies `edit` at the current tick. At tick 0 it changes the scene and marks the
    /// recorded future stale; past tick 0 it records the edit, forking a branch when the
    /// current one has recorded ticks after it.
    /// Returns the entity a `Spawn` created.
    pub fn edit(
        &mut self,
        sim: &mut Simulation,
        edit: Edit,
    ) -> Result<Option<Entity>, SessionError> {
        self.ensure_resimulated()?;
        if self.current == 0 {
            return self.record_scene_edit(sim, &edit);
        }
        let edit = TimedEdit {
            tick: self.current,
            edit,
        };
        let spawned = self.record_edit(sim, edit.clone())?;
        self.history.record(UndoEntry::Run { edit });
        Ok(spawned)
    }

    /// Takes back the last edit. A scene edit reloads the scene before it and marks the
    /// recorded future stale; a recorded edit is removed from the log, the future after its
    /// tick is discarded, and the session moves to its tick.
    pub fn undo(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let entry = self
            .history
            .take_undo()
            .ok_or(SessionError::NothingToUndo)?;
        match &entry {
            UndoEntry::Scene { before, .. } => {
                let result = self.load_scene_version(sim, before.clone());
                self.history.undone(entry, result.is_ok());
                result
            }
            UndoEntry::Run { edit } if self.is_forking_edit(edit) => {
                // The branch it created goes, and the parent is shown at its tick.
                self.current = edit.tick;
                let result = self.delete_branch(sim, self.tree.current()).map(drop);
                self.history.undone(entry, result.is_ok());
                result
            }
            UndoEntry::Run { edit } => {
                let tick = edit.tick;
                // An edit recorded on another branch is refused, leaving the timeline and
                // the entry as they are: discarding here would cut this branch's future.
                let Some(index) = self.edits.edits.iter().rposition(|e| e == edit) else {
                    self.history.undone(entry, false);
                    return Err(SessionError::NotOnBranch { tick });
                };
                self.remove_edit(index);
                self.discard_after(tick);
                self.history.undone(entry, true);
                // The ticks up to the edit ran before with these same inputs and edits.
                self.current = tick;
                self.replay_to(sim, tick)?;
                if self.edits.at(tick).is_empty() {
                    self.keep_checkpoint_if_sparse(sim);
                }
                Ok(())
            }
        }
    }

    /// Applies the last undone edit again: a scene edit reloads the scene after it, a
    /// recorded edit is applied and recorded again at its tick. Redoing a recorded edit
    /// discards the future after its tick, so play recorded since the undo is replaced
    /// without warning.
    pub fn redo(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let entry = self
            .history
            .take_redo()
            .ok_or(SessionError::NothingToRedo)?;
        let result = match &entry {
            UndoEntry::Scene { after, .. } => self.load_scene_version(sim, after.clone()),
            UndoEntry::Run { edit } => self
                .seek(sim, edit.tick)
                .and_then(|()| self.record_edit(sim, edit.clone()).map(drop)),
        };
        self.history.redone(entry, result.is_ok());
        result
    }

    /// Copies the current value of `component` on `entity` into the scene. Like every scene
    /// edit, it marks the recorded future stale; resimulation brings the session back to
    /// the current tick. The entity must exist at tick 0.
    pub fn apply_to_scene(
        &mut self,
        sim: &mut Simulation,
        entity: Entity,
        component: &str,
    ) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let world = sim.world();
        if !world.is_alive(entity) {
            return Err(EditError::NoSuchEntity(entity).into());
        }
        if world.component_codec(component).is_none() {
            return Err(EditError::UnknownComponent(component.to_string()).into());
        }
        let value = world.component_bytes(entity, component).ok_or_else(|| {
            SessionError::MissingComponent {
                entity,
                component: component.to_string(),
            }
        })?;
        let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
        let index = entity.index() as usize;
        let in_scene = scene_state.entities.alive.get(index) == Some(&true)
            && scene_state.entities.generations[index] == entity.generation();
        if !in_scene {
            return Err(SessionError::NotInScene(entity));
        }
        let in_scene_value = scene_state
            .components
            .iter()
            .find(|(name, _)| name == component)
            .and_then(|(_, values)| values.iter().find(|(owner, _)| *owner == entity))
            .map(|(_, bytes)| bytes);
        if in_scene_value == Some(&value) {
            // The scene already holds this value: nothing to change or undo.
            return Ok(());
        }
        let edit = Edit::Insert {
            entity: sim.entity_ref(entity),
            component: component.to_string(),
            value,
        };
        self.record_scene_edit(sim, &edit).map(drop)
    }

    /// Marks the current scene text as saved.
    pub fn mark_scene_saved(&mut self) {
        self.saved_scene = self.scene.clone();
        self.scene_dirty = false;
    }

    /// Moves to `tick`, simulating the recorded future on the way when needed.
    pub fn seek(&mut self, sim: &mut Simulation, tick: u64) -> Result<(), SessionError> {
        if let Some(up_to) = self.resimulating
            && tick > up_to
        {
            return Err(SessionError::Resimulating { up_to });
        }
        if tick > self.simulated().max(self.head) {
            return Err(SessionError::InvalidTick {
                tick,
                head: self.head,
            });
        }
        self.replay_to(sim, tick)?;
        self.current = tick;
        self.return_to = None;
        Ok(())
    }

    /// The hash recorded after `tick` ticks, before the edits of `tick`.
    pub(crate) fn recorded_hash(&self, tick: u64) -> Option<u64> {
        self.hashes.get(usize::try_from(tick).ok()?).copied()
    }

    /// Whether edits recorded at the current tick are applied but not yet simulated.
    pub fn is_pending(&self) -> bool {
        !self.edits.at(self.current).is_empty()
    }

    /// The scene text, that is the state at tick 0.
    pub fn scene(&self) -> &str {
        &self.scene
    }

    /// Loads a new version of the scene at tick 0, as saved, and marks the recorded future
    /// stale. Undo brings the previous version back.
    pub fn replace_scene(
        &mut self,
        sim: &mut Simulation,
        scene: String,
    ) -> Result<(), SessionError> {
        sim.load_scene_with(&scene, &NoAssets)?;
        self.saved_scene = scene.clone();
        let before = std::mem::replace(&mut self.scene, scene.clone());
        self.scene_dirty = false;
        self.history.record(UndoEntry::Scene {
            before,
            after: scene,
        });
        self.start_resimulation(sim);
        Ok(())
    }

    /// Moves to the snapshot's tick, simulating the recorded future up to it when needed,
    /// and drops the hashes and checkpoints after it; the recorded inputs and edits after it
    /// stay as the future. A snapshot of another state (another session or build) becomes
    /// the whole state of its tick: the edits recorded at that tick are dropped, and its hash
    /// and checkpoint replace the recorded ones, so seeking back to it returns to it.
    pub fn restore(
        &mut self,
        sim: &mut Simulation,
        snapshot: &Snapshot,
    ) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        sim.restore(snapshot)?;
        let (tick, restored) = (sim.tick(), sim.hash());
        if let Err(error) = self.replay_to(sim, tick) {
            // The ticks up to the failing one were recorded on the way: stay there.
            if let SessionError::Panicked { tick: failed, .. } = &error {
                self.current = *failed;
                self.replay_to(sim, *failed)?;
            }
            return Err(error);
        }
        let kept = usize::try_from(tick + 1).expect("tick fits in memory");
        self.hashes.truncate(kept);
        self.checkpoints.truncate_after(tick);
        self.current = tick;
        if sim.hash() != restored {
            sim.restore(snapshot)
                .expect("the snapshot restored into this world before");
            let first = self.edits.edits.partition_point(|edit| edit.tick < tick);
            let end = self.edits.edits.partition_point(|edit| edit.tick <= tick);
            let dropped: Vec<TimedEdit> = self.edits.edits.drain(first..end).collect();
            // ponytail: at the fork tick the branch stops inheriting the dropped edits, but
            // the restored state and its hash sit in the prefix, which switching away and
            // back recomposes from the parent, undoing the restore (before the fork tick,
            // the dropped edits come back too). Store the restored state in the branch if
            // it must survive switching.
            self.tree.edits_dropped_at(tick, dropped.len());
            for edit in &dropped {
                self.history.forget(edit);
            }
            self.update_head();
            self.hashes[kept - 1] = restored;
            self.checkpoints.pin(tick, snapshot.clone());
        }
        Ok(())
    }

    /// Replays at most `budget` stale recorded ticks on the new state, then restores the
    /// current tick. Returns one report per call. A tick that fails (a panic) ends
    /// resimulation there: the stale hashes after it are dropped and the recorded inputs and
    /// edits stay as the future.
    pub fn resimulate(
        &mut self,
        sim: &mut Simulation,
        budget: u32,
    ) -> Result<Resimulation, SessionError> {
        let result = self.resimulate_within(sim, budget);
        if result.is_err()
            && let Some(failed) = self.resimulating.take()
        {
            let kept = usize::try_from(failed + 1).expect("tick fits in memory");
            self.hashes.truncate(kept);
            self.checkpoints.truncate_after(failed);
            if let Some(tick) = self.return_to.take() {
                self.current = tick.min(failed);
            }
            self.replay_to(sim, self.current)?;
        }
        result
    }

    fn resimulate_within(
        &mut self,
        sim: &mut Simulation,
        budget: u32,
    ) -> Result<Resimulation, SessionError> {
        if let Some(report) = self.reports.pop_front() {
            return Ok(report);
        }
        let Some(mut up_to) = self.resimulating else {
            return Ok(Resimulation::Idle);
        };
        let at_current = sim.snapshot();
        self.replay_to(sim, up_to)?;
        for _ in 0..budget {
            if up_to >= self.end() {
                break;
            }
            self.run_tick(sim, self.inputs.tick(up_to).map(<[u8]>::to_vec).as_deref())?;
            up_to += 1;
            self.resimulating = Some(up_to);
            let got = sim.hash();
            if let Some(introspection) = &mut self.introspection {
                introspection.hashed(up_to, got);
            }
            match self.hashes.get_mut(up_to as usize) {
                Some(expected) => {
                    if *expected != got && !self.diverged {
                        self.diverged = true;
                        self.reports.push_back(Resimulation::Diverged {
                            tick: up_to,
                            expected: *expected,
                            got,
                        });
                    }
                    *expected = got;
                }
                None => self.hashes.push(got),
            }
            if self.checkpoints.wants(up_to) {
                self.checkpoints.insert(up_to, sim.snapshot());
                self.enforce_checkpoint_budget();
            }
            self.apply_edits_or_drop(sim, up_to);
        }
        if up_to >= self.end() {
            self.resimulating = None;
            self.reports.push_back(Resimulation::Done);
        }
        let returned = self
            .return_to
            .filter(|&tick| tick <= up_to || self.resimulating.is_none());
        if let Some(tick) = returned {
            self.current = tick.min(up_to);
            self.return_to = None;
            self.replay_to(sim, self.current)?;
        } else {
            // The ticks resimulated here are all past the current one, whose state stays.
            sim.restore(&at_current)
                .expect("a snapshot restores into the world it was taken from");
        }
        Ok(self
            .reports
            .pop_front()
            .unwrap_or(Resimulation::Progress { up_to }))
    }

    pub fn export(&self) -> Result<SessionExport, SessionError> {
        self.ensure_resimulated()?;
        Ok(SessionExport {
            scene: self.scene.clone(),
            saved_scene: self.scene_dirty.then(|| self.saved_scene.clone()),
            inputs: self.inputs.encode(),
            edits: self.edits.encode(),
            undo: self.history.encode(),
            hashes: self.hashes.clone(),
            branches: if self.tree.is_branched() {
                self.tree.encode(self.inputs.players)
            } else {
                Vec::new()
            },
        })
    }

    pub fn timeline(&self) -> Timeline {
        let mut edit_ticks: Vec<u64> = self.edits.edits.iter().map(|edit| edit.tick).collect();
        edit_ticks.dedup();
        Timeline {
            head: self.head,
            simulated: self.simulated(),
            current: self.current,
            checkpoints: self.checkpoints.ticks(),
            edit_ticks,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            scene_dirty: self.scene_dirty,
            resimulating: self.resimulating.is_some(),
            branch: self.tree.current(),
            branches: self.branches(),
        }
    }

    pub fn branches(&self) -> Vec<BranchInfo> {
        self.tree.infos(self.head)
    }

    pub fn current_branch(&self) -> BranchId {
        self.tree.current()
    }

    /// Makes `id` the current branch, at the current tick when it covers it, otherwise at
    /// its head. A stale branch is resimulated first, as after a scene edit.
    pub fn switch_branch(
        &mut self,
        sim: &mut Simulation,
        id: BranchId,
    ) -> Result<(), SessionError> {
        if self.tree.get(id).is_none() {
            return Err(SessionError::NoSuchBranch(id));
        }
        if id == self.tree.current() {
            return Ok(());
        }
        let tick = self.current;
        let defaults = sim.encoded_default_inputs();
        let stale = self
            .tree
            .path(id)
            .iter()
            .any(|&b| self.tree.get(b).is_some_and(|b| b.stale));
        let segment = self.current_segment();
        let target = self
            .tree
            .make_current(segment, self.resimulating.is_some(), id)
            .expect("a branch other than the current one is stored");
        self.compose(&target, defaults.as_deref());
        self.enforce_checkpoint_budget();
        self.resimulating = None;
        self.return_to = None;
        self.reports.clear();
        self.current = if tick <= self.end() { tick } else { self.head };
        if stale {
            let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
            sim.restore(scene_state)
                .expect("a checkpoint restores into the world it was taken from");
            self.resimulate_from_scene(sim);
            Ok(())
        } else {
            self.replay_to(sim, self.current)
        }
    }

    pub fn rename_branch(&mut self, id: BranchId, name: String) -> Result<(), SessionError> {
        let branch = self
            .tree
            .get_mut(id)
            .ok_or(SessionError::NoSuchBranch(id))?;
        branch.name = name;
        Ok(())
    }

    /// Deletes `id` and its descendants, and returns the descendants. Deleting the current
    /// branch, or one of its ancestors, switches to the parent of `id` first.
    pub fn delete_branch(
        &mut self,
        sim: &mut Simulation,
        id: BranchId,
    ) -> Result<Vec<BranchId>, SessionError> {
        let branch = self.tree.get(id).ok_or(SessionError::NoSuchBranch(id))?;
        let parent = branch.parent.ok_or(SessionError::RootBranch)?;
        let mut removed = self.tree.subtree(id);
        if removed.contains(&self.tree.current()) {
            self.switch_branch(sim, parent)?;
        }
        self.tree.remove(&removed);
        removed.retain(|&branch| branch != id);
        Ok(removed)
    }

    /// Every checkpoint the session holds, across its branches.
    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len() + self.tree.checkpoint_count()
    }

    /// The current branch's part of the flattened run.
    fn current_segment(&self) -> Segment {
        Segment::split(
            self.tree.current_branch().fork(),
            &self.inputs,
            &self.edits,
            &self.hashes,
            &self.checkpoints,
        )
    }

    /// Flattens the run of the current branch, whose segment is `target`, from its
    /// ancestors' segments. Tick 0 is common to every branch.
    fn compose(&mut self, target: &Segment, defaults: Option<&[u8]>) {
        let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
        let mut checkpoints = Checkpoints::new(scene_state.clone());
        let mut inputs = InputLog::new(self.inputs.players);
        let mut edits = EditLog::new();
        let mut hashes = vec![self.hashes[0]];
        let current = self.tree.current();
        for id in self.tree.path(current) {
            let branch = self.tree.get(id).expect("a branch of the path");
            let segment = branch.segment.as_ref().unwrap_or(target);
            segment.append_to(
                branch.fork(),
                &mut inputs,
                &mut edits,
                &mut hashes,
                &mut checkpoints,
                defaults,
            );
        }
        (self.inputs, self.edits, self.hashes, self.checkpoints) =
            (inputs, edits, hashes, checkpoints);
        self.update_head();
    }

    /// Removes an edit from the run, keeping the count of edits the current branch inherits
    /// at its fork tick.
    fn remove_edit(&mut self, index: usize) -> TimedEdit {
        let removed = self.edits.edits.remove(index);
        let first = self
            .edits
            .edits
            .partition_point(|edit| edit.tick < removed.tick);
        let branch = self.tree.current_branch();
        if removed.tick == branch.fork_tick && index - first < branch.fork_edits {
            self.tree.edits_dropped_at(removed.tick, 1);
        }
        removed
    }

    /// Whether `edit` created the current branch: its first own edit at its fork tick.
    fn is_forking_edit(&self, edit: &TimedEdit) -> bool {
        let branch = self.tree.current_branch();
        branch.parent.is_some()
            && edit.tick == branch.fork_tick
            && self.edits.at(edit.tick).get(branch.fork_edits) == Some(edit)
    }

    /// Before recording at `tick`: forks a branch when the current one has recorded ticks
    /// after it (the fork rule), then drops the future after `tick` from the run.
    fn fork_or_discard_after(&mut self, tick: u64) {
        if self.end() > tick {
            let segment = self.current_segment();
            let fork = (tick, self.edits.at(tick).len());
            self.tree.fork(segment, self.resimulating.is_some(), fork);
        }
        self.discard_after(tick);
        self.enforce_checkpoint_budget();
    }

    /// Evicts checkpoints past `CHECKPOINT_BUDGET`, oldest first: those of stored branches
    /// other than the current branch's parent, then the parent's, then the current
    /// branch's. Tick 0, fork ticks, and pinned checkpoints stay.
    fn enforce_checkpoint_budget(&mut self) {
        let mut over = self.checkpoint_count().saturating_sub(CHECKPOINT_BUDGET);
        if over == 0 {
            return;
        }
        let parent = self.tree.current_branch().parent;
        let fork_ticks = self.tree.fork_ticks();
        while over > 0 && self.tree.evict_oldest(|id| Some(id) != parent, &fork_ticks) {
            over -= 1;
        }
        while over > 0 && self.tree.evict_oldest(|id| Some(id) == parent, &fork_ticks) {
            over -= 1;
        }
        while over > 0 {
            let Some(tick) = self.checkpoints.oldest_evictable(&fork_ticks) else {
                break;
            };
            self.checkpoints.evict(tick);
            over -= 1;
        }
    }

    fn simulated(&self) -> u64 {
        self.hashes.len() as u64 - 1
    }

    /// The last tick resimulation has to reach.
    fn end(&self) -> u64 {
        self.head.max(self.simulated())
    }

    fn ensure_resimulated(&self) -> Result<(), SessionError> {
        match self.resimulating {
            Some(up_to) => Err(SessionError::Resimulating { up_to }),
            None => Ok(()),
        }
    }

    fn update_head(&mut self) {
        let inputs = (self.inputs.ticks.len() as u64).saturating_sub(1);
        self.head = inputs.max(self.edits.last_tick().unwrap_or(0));
    }

    /// Steps once, recorded while introspection is on, catching a panic before the session
    /// touches its logs: on a panic the state of the current tick is restored and
    /// `Panicked` names the tick that failed.
    fn run_tick(&mut self, sim: &mut Simulation, input: Option<&[u8]>) -> Result<(), SessionError> {
        let tick = sim.tick();
        let introspection = &mut self.introspection;
        let step = || match introspection {
            Some(introspection) => introspection.step(sim, input),
            None => sim.step_encoded(input),
        };
        match catch_unwind(AssertUnwindSafe(step)) {
            Ok(result) => Ok(result?),
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|message| message.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic without a message".to_string());
                self.replay_to(sim, self.current)?;
                Err(SessionError::Panicked { tick, message })
            }
        }
    }

    /// Restores the nearest checkpoint at or before `tick` and replays up to `tick`,
    /// recording the hashes and checkpoints of ticks simulated for the first time. Leaves
    /// `current` unchanged.
    fn replay_to(&mut self, sim: &mut Simulation, tick: u64) -> Result<(), SessionError> {
        let (mut at, snapshot) = self.checkpoints.nearest_at_or_before(tick);
        sim.restore(snapshot)
            .expect("a checkpoint restores into the world it was taken from");
        self.apply_edits(sim, at)?;
        while at < tick {
            self.run_tick(sim, self.inputs.tick(at).map(<[u8]>::to_vec).as_deref())?;
            at += 1;
            self.record_tick(sim);
            self.apply_edits(sim, at)?;
        }
        Ok(())
    }

    /// Records the hash and checkpoint of a tick just simulated for the first time, and
    /// passes the hash of the tick to introspection.
    fn record_tick(&mut self, sim: &Simulation) {
        let tick = sim.tick();
        if tick == self.hashes.len() as u64 {
            self.hashes.push(sim.hash());
            if self.checkpoints.wants(tick) {
                self.checkpoints.insert(tick, sim.snapshot());
                self.enforce_checkpoint_budget();
            }
        }
        // Ticks replayed here are never stale: seeking past resimulation is refused.
        if let (Some(introspection), Some(&hash)) =
            (&mut self.introspection, self.hashes.get(tick as usize))
        {
            introspection.hashed(tick, hash);
        }
    }

    /// Keeps the world as a checkpoint when the nearest one is far behind, as after the
    /// future of a tick far behind the head was discarded. The world must be at a tick
    /// without applied edits.
    fn keep_checkpoint_if_sparse(&mut self, sim: &Simulation) {
        let tick = sim.tick();
        if self.checkpoints.is_sparse_at(tick) {
            self.checkpoints.pin(tick, sim.snapshot());
        }
    }

    fn apply_edits(&self, sim: &mut Simulation, tick: u64) -> Result<(), SessionError> {
        for timed in self.edits.at(tick) {
            sim.apply_edit(&timed.edit)?;
        }
        Ok(())
    }

    /// Applies the edits of `tick`, removing and reporting those that fail.
    fn apply_edits_or_drop(&mut self, sim: &mut Simulation, tick: u64) {
        let mut index = self.edits.edits.partition_point(|edit| edit.tick < tick);
        while self
            .edits
            .edits
            .get(index)
            .is_some_and(|edit| edit.tick == tick)
        {
            match sim.apply_edit(&self.edits.edits[index].edit) {
                Ok(_) => index += 1,
                Err(error) => {
                    let dropped = self.remove_edit(index);
                    self.history.forget(&dropped);
                    self.reports.push_back(Resimulation::EditDropped {
                        tick,
                        reason: error.to_string(),
                    });
                }
            }
        }
        self.update_head();
    }

    /// Records `bytes` as the input of `tick`, padding any gap with default inputs.
    fn record_input(&mut self, sim: &Simulation, tick: u64, bytes: Vec<u8>) {
        let tick = usize::try_from(tick).expect("tick fits in memory");
        self.inputs.ticks.truncate(tick);
        if self.inputs.ticks.len() < tick {
            let defaults = sim
                .encoded_default_inputs()
                .expect("recorded inputs imply an input type");
            self.inputs.ticks.resize(tick, defaults);
        }
        self.inputs.ticks.push(bytes);
        self.update_head();
    }

    /// Drops every input, edit, hash, and checkpoint after `tick`; the inputs and edits of
    /// `tick` itself stay.
    fn discard_after(&mut self, tick: u64) {
        let kept = usize::try_from(tick + 1).expect("tick fits in memory");
        self.inputs.ticks.truncate(kept);
        self.edits.edits.retain(|edit| edit.tick <= tick);
        self.hashes.truncate(kept);
        self.checkpoints.truncate_after(tick);
        self.history.discard_after(tick);
        self.update_head();
        // A live take-over below the resimulated point discards the whole stale future.
        if self.resimulating.is_some_and(|up_to| up_to > tick) {
            self.resimulating = None;
        }
    }

    /// Applies `edit` at the current tick, forks or discards the future after it, and
    /// records it.
    fn record_edit(
        &mut self,
        sim: &mut Simulation,
        edit: TimedEdit,
    ) -> Result<Option<Entity>, SessionError> {
        let checkpoint = (self.edits.at(edit.tick).is_empty()
            && self.checkpoints.is_sparse_at(edit.tick))
        .then(|| sim.snapshot());
        let spawned = sim.apply_edit(&edit.edit)?;
        self.fork_or_discard_after(edit.tick);
        if let Some(checkpoint) = checkpoint {
            self.checkpoints.pin(edit.tick, checkpoint);
        }
        self.edits.edits.push(edit);
        self.update_head();
        Ok(spawned)
    }

    /// Loads a scene version from the undo history, like a tick 0 edit.
    fn load_scene_version(
        &mut self,
        sim: &mut Simulation,
        scene: String,
    ) -> Result<(), SessionError> {
        sim.load_scene_with(&scene, &NoAssets)?;
        self.set_scene(scene);
        self.start_resimulation(sim);
        Ok(())
    }

    /// After new logs, which define a new linear run: only tick 0 stays simulated, the
    /// session is at tick 0, and recorded edits are checked on this state by resimulation.
    fn reset_future(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.tree = BranchTree::default();
        self.hashes.truncate(1);
        self.update_head();
        self.current = 0;
        let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
        sim.restore(scene_state)
            .expect("a checkpoint restores into the world it was taken from");
        self.start_resimulation(sim);
        Ok(())
    }

    fn set_scene(&mut self, scene: String) {
        self.scene_dirty = scene != self.saved_scene;
        self.scene = scene;
    }

    /// Applies `edit` to the scene and records it in the undo history.
    fn record_scene_edit(
        &mut self,
        sim: &mut Simulation,
        edit: &Edit,
    ) -> Result<Option<Entity>, SessionError> {
        let before = self.scene.clone();
        let spawned = self.edit_scene(sim, edit)?;
        let after = self.scene.clone();
        self.history.record(UndoEntry::Scene { before, after });
        Ok(spawned)
    }

    /// Applies `edit` to the scene: the scene is saved and reloaded so entity indices and
    /// generations match any later load of it.
    fn edit_scene(
        &mut self,
        sim: &mut Simulation,
        edit: &Edit,
    ) -> Result<Option<Entity>, SessionError> {
        let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
        sim.restore(scene_state)
            .expect("a checkpoint restores into the world it was taken from");
        let spawned = match sim.apply_edit(edit) {
            Ok(spawned) => spawned,
            Err(error) => {
                self.replay_to(sim, self.current)?;
                return Err(error.into());
            }
        };
        // Saving renumbers the alive entities in index order.
        let rank =
            spawned.map(|entity| sim.world().entities().take_while(|&e| e != entity).count());
        let scene = sim.save_scene();
        sim.load_scene_with(&scene, &NoAssets)?;
        self.set_scene(scene);
        let spawned = rank.map(|rank| sim.world().entities().nth(rank).expect("saved entity"));
        self.start_resimulation(sim);
        Ok(spawned)
    }

    /// The world holds a new tick 0 state, before the edits of tick 0: the recorded future
    /// is kept but its checkpoints are dropped, and it is resimulated from tick 0 when it has
    /// hashes to replace or edits past tick 0 to check (a recorded edit that fails on the new
    /// state is dropped and reported; inputs alone always replay). The new tick 0 hash is not
    /// compared, since the state changed on purpose. The session shows tick 0 while it
    /// resimulates, then returns to the tick it was at (or to the last resimulated tick when
    /// the recorded future ends before it).
    /// The other branches are marked stale.
    fn start_resimulation(&mut self, sim: &mut Simulation) {
        self.tree.mark_stale();
        self.resimulate_from_scene(sim);
    }

    /// `start_resimulation` for the current branch only.
    fn resimulate_from_scene(&mut self, sim: &mut Simulation) {
        self.checkpoints = Checkpoints::new(sim.snapshot());
        self.hashes[0] = sim.hash();
        let was_at = self.current;
        self.current = 0;
        self.diverged = false;
        self.reports.clear();
        self.apply_edits_or_drop(sim, 0);
        let stale = self.simulated() > 0 || self.edits.last_tick().is_some_and(|tick| tick > 0);
        self.resimulating = stale.then_some(0);
        // Without a stale future there is nothing to return through.
        self.return_to = (stale && was_at > 0).then_some(was_at);
    }
}
