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

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ludenea_app::{
    Edit, EditError, EditLog, InputLog, SceneError, SimError, Simulation, TimedEdit,
};
use ludenea_ecs::{Entity, Snapshot, SnapshotError};
use serde::{Deserialize, Serialize};

use crate::checkpoints::Checkpoints;
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
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error(transparent)]
    Scene(#[from] SceneError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
}

/// What the timeline shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    pub inputs: Vec<u8>,
    pub edits: Vec<u8>,
    /// The undo and redo stacks, so a rebuild keeps the history.
    pub undo: Vec<u8>,
    pub hashes: Vec<u64>,
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
    history: UndoHistory,
    /// The last resimulated tick, while the recorded future is stale.
    resimulating: Option<u64>,
    diverged: bool,
    reports: VecDeque<Resimulation>,
}

impl Session {
    /// Loads `scene` at tick 0 with empty logs.
    pub fn start(sim: &mut Simulation, scene: String) -> Result<Session, SessionError> {
        sim.load_scene(&scene)?;
        let players = u16::try_from(sim.players()).expect("fewer than 2^16 players");
        Ok(Session {
            saved_scene: scene.clone(),
            scene,
            inputs: InputLog::new(players),
            edits: EditLog::new(),
            hashes: vec![sim.hash()],
            checkpoints: Checkpoints::new(sim.snapshot()),
            head: 0,
            current: 0,
            history: UndoHistory::default(),
            resimulating: None,
            diverged: false,
            reports: VecDeque::new(),
        })
    }

    /// Restarts a recorded session on a new build: the new hashes are compared with
    /// `expected` while it resimulates. `undo` is the exported undo history; empty bytes
    /// mean an empty one.
    pub fn resume(
        sim: &mut Simulation,
        scene: String,
        inputs: InputLog,
        edits: EditLog,
        undo: &[u8],
        expected: Vec<u64>,
    ) -> Result<Session, SessionError> {
        let history = if undo.is_empty() {
            UndoHistory::default()
        } else {
            UndoHistory::decode(undo)?
        };
        sim.set_players(usize::from(inputs.players));
        let mut session = Session::start(sim, scene)?;
        session.inputs = inputs;
        session.edits = edits;
        session.history = history;
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
    /// unless `live` differs from it, which takes over: the future after the current tick is
    /// discarded and `live` is recorded. Past the end of the recorded inputs, `live` or
    /// default inputs are recorded.
    pub fn step(&mut self, sim: &mut Simulation, live: Option<&[u8]>) -> Result<u64, SessionError> {
        let tick = self.current;
        if let Some(up_to) = self.resimulating
            && tick >= up_to
        {
            return Err(SessionError::Resimulating { up_to });
        }
        let recorded = self.inputs.tick(tick).map(<[u8]>::to_vec);
        let take_over = live.filter(|live| recorded.as_deref() != Some(*live));
        self.run_tick(sim, take_over.or(recorded.as_deref()))?;
        if let Some(live) = take_over {
            self.discard_after(tick);
            self.record_input(sim, tick, live.to_vec());
        } else if recorded.is_none()
            && tick >= self.head
            && let Some(defaults) = sim.encoded_default_inputs()
        {
            self.record_input(sim, tick, defaults);
        }
        self.record_tick(sim);
        self.current = tick + 1;
        self.apply_edits(sim, self.current)?;
        Ok(self.current)
    }

    /// Applies `edit` at the current tick. At tick 0 it changes the scene and marks the
    /// recorded future stale; past tick 0 it discards the future and records the edit.
    /// Returns the entity a `Spawn` created.
    pub fn edit(
        &mut self,
        sim: &mut Simulation,
        edit: Edit,
    ) -> Result<Option<Entity>, SessionError> {
        self.ensure_resimulated()?;
        if self.current == 0 {
            let before = self.scene.clone();
            let spawned = self.edit_scene(sim, &edit)?;
            let after = self.scene.clone();
            self.history.record(UndoEntry::Scene { before, after });
            return Ok(spawned);
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
        let entry = self.history.undo.pop().ok_or(SessionError::NothingToUndo)?;
        match entry.clone() {
            UndoEntry::Scene { before, .. } => {
                if let Err(error) = self.load_scene_version(sim, before) {
                    self.history.undo.push(entry);
                    return Err(error);
                }
                self.history.redo.push(entry);
                Ok(())
            }
            UndoEntry::Run { edit } => {
                if let Some(index) = self.edits.edits.iter().rposition(|e| *e == edit) {
                    self.edits.edits.remove(index);
                }
                self.discard_after(edit.tick);
                self.history.redo.push(entry);
                // The ticks up to the edit ran before with these same inputs and edits.
                self.current = edit.tick;
                self.replay_to(sim, edit.tick)
            }
        }
    }

    /// Applies the last undone edit again: a scene edit reloads the scene after it, a
    /// recorded edit is applied and recorded again at its tick.
    pub fn redo(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let entry = self.history.redo.pop().ok_or(SessionError::NothingToRedo)?;
        let result = match &entry {
            UndoEntry::Scene { after, .. } => self.load_scene_version(sim, after.clone()),
            UndoEntry::Run { edit } => self
                .seek(sim, edit.tick)
                .and_then(|()| self.record_edit(sim, edit.clone()).map(drop)),
        };
        match result {
            Ok(()) => self.history.undo.push(entry),
            Err(_) => self.history.redo.push(entry),
        }
        result
    }

    /// Marks the current scene text as saved.
    pub fn mark_scene_saved(&mut self) {
        self.saved_scene = self.scene.clone();
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
        Ok(())
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
        sim.load_scene(&scene)?;
        self.saved_scene = scene.clone();
        let before = std::mem::replace(&mut self.scene, scene.clone());
        self.history.record(UndoEntry::Scene {
            before,
            after: scene,
        });
        self.start_resimulation(sim);
        Ok(())
    }

    /// Replaces the state at the snapshot's tick: the hashes and checkpoints after it are
    /// dropped, and the recorded inputs and edits after it stay as the future that replays
    /// on top of it. A snapshot past the simulated ticks (taken elsewhere) is restored as is;
    /// the ticks run from it add no hashes.
    pub fn restore(
        &mut self,
        sim: &mut Simulation,
        snapshot: &Snapshot,
    ) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        sim.restore(snapshot)?;
        let tick = sim.tick();
        let kept = usize::try_from(tick + 1).expect("tick fits in memory");
        self.hashes.truncate(kept);
        self.checkpoints.truncate_after(tick);
        self.current = tick;
        Ok(())
    }

    /// Replays at most `budget` stale recorded ticks on the new state, then restores the
    /// current tick. Returns one report per call.
    pub fn resimulate(
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
        self.replay_to(sim, up_to)?;
        for _ in 0..budget {
            if up_to >= self.end() {
                break;
            }
            self.run_tick(sim, self.inputs.tick(up_to).map(<[u8]>::to_vec).as_deref())?;
            up_to += 1;
            self.resimulating = Some(up_to);
            let got = sim.hash();
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
            }
            self.apply_edits_or_drop(sim, up_to);
        }
        if up_to >= self.end() {
            self.resimulating = None;
            self.reports.push_back(Resimulation::Done);
        }
        self.replay_to(sim, self.current)?;
        Ok(self
            .reports
            .pop_front()
            .unwrap_or(Resimulation::Progress { up_to }))
    }

    pub fn export(&self) -> Result<SessionExport, SessionError> {
        self.ensure_resimulated()?;
        Ok(SessionExport {
            scene: self.scene.clone(),
            inputs: self.inputs.encode(),
            edits: self.edits.encode(),
            undo: self.history.encode(),
            hashes: self.hashes.clone(),
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
            can_undo: !self.history.undo.is_empty(),
            can_redo: !self.history.redo.is_empty(),
            scene_dirty: self.scene != self.saved_scene,
            resimulating: self.resimulating.is_some(),
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

    /// Steps once, catching a panic before the session touches its logs: on a panic the
    /// state of the current tick is restored and `Panicked` names the tick that failed.
    fn run_tick(&mut self, sim: &mut Simulation, input: Option<&[u8]>) -> Result<(), SessionError> {
        let tick = sim.tick();
        match catch_unwind(AssertUnwindSafe(|| sim.step_encoded(input))) {
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

    /// Records the hash and checkpoint of a tick just simulated for the first time.
    fn record_tick(&mut self, sim: &Simulation) {
        let tick = sim.tick();
        if tick == self.hashes.len() as u64 {
            self.hashes.push(sim.hash());
            if self.checkpoints.wants(tick) {
                self.checkpoints.insert(tick, sim.snapshot());
            }
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
                    let dropped = self.edits.edits.remove(index);
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
    }

    /// Applies `edit` at the current tick, discards the future after it, and records it.
    fn record_edit(
        &mut self,
        sim: &mut Simulation,
        edit: TimedEdit,
    ) -> Result<Option<Entity>, SessionError> {
        let spawned = sim.apply_edit(&edit.edit)?;
        self.discard_after(edit.tick);
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
        sim.load_scene(&scene)?;
        self.scene = scene;
        self.start_resimulation(sim);
        Ok(())
    }

    /// After new logs: only tick 0 stays simulated, the session is at tick 0, and recorded
    /// edits are checked on this state by resimulation.
    fn reset_future(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.hashes.truncate(1);
        self.update_head();
        let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
        sim.restore(scene_state)
            .expect("a checkpoint restores into the world it was taken from");
        self.start_resimulation(sim);
        Ok(())
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
                self.apply_edits(sim, 0)?;
                return Err(error.into());
            }
        };
        // Saving renumbers the alive entities in index order.
        let rank =
            spawned.map(|entity| sim.world().entities().take_while(|&e| e != entity).count());
        let scene = sim.save_scene();
        sim.load_scene(&scene)?;
        self.scene = scene;
        let spawned = rank.map(|rank| sim.world().entities().nth(rank).expect("saved entity"));
        self.start_resimulation(sim);
        Ok(spawned)
    }

    /// The world holds a new tick 0 state, before the edits of tick 0: the recorded future
    /// is kept but its checkpoints are dropped, and it is resimulated from tick 0 when it has
    /// hashes to replace or edits past tick 0 to check (a recorded edit that fails on the new
    /// state is dropped and reported; inputs alone always replay). The new tick 0 hash is not
    /// compared, since the state changed on purpose.
    fn start_resimulation(&mut self, sim: &mut Simulation) {
        self.checkpoints = Checkpoints::new(sim.snapshot());
        self.hashes[0] = sim.hash();
        self.current = 0;
        self.diverged = false;
        self.reports.clear();
        self.apply_edits_or_drop(sim, 0);
        let stale = self.simulated() > 0 || self.edits.last_tick().is_some_and(|tick| tick > 0);
        self.resimulating = stale.then_some(0);
    }
}
