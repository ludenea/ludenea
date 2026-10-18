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

use std::collections::{BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};

use ludenea_app::assets::{asset_file, parse_asset_file};
use ludenea_app::path::PathSegment;
use ludenea_app::prefab;
use ludenea_app::replay::{Replay, ReplayError, keep_assets, load_replay, record_loaded_replay};
use ludenea_app::{
    AssetTexts, Edit, EditError, EditLog, EntityRef, InputLog, LoadAddress, SceneError, SimError,
    Simulation, TimedEdit,
};
use ludenea_ecs::{AssetId, Entity, Snapshot, SnapshotError};
use serde::{Deserialize, Serialize};

use crate::branches::{BranchId, BranchInfo, BranchTree, BranchesError, Segment, StoredBranch};
use crate::checkpoints::{CHECKPOINT_BUDGET, Checkpoints};
use crate::subscriptions::Introspection;
use crate::undo::{ForkCause, UndoEntry, UndoError, UndoHistory};

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
    #[error(transparent)]
    Undo(#[from] UndoError),
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
    /// The entry is dropped from the history, so the next undo goes on.
    #[error(
        "the edit to undo, at tick {tick}, is no longer on its branch; it was dropped from the history"
    )]
    NotOnBranch { tick: u64 },
    #[error("the root branch cannot be deleted")]
    RootBranch,
    #[error("the branch tree cannot be decoded: {0}")]
    BranchesDecode(#[from] BranchesError),
    #[error("a prefab has no resources; edit them in a scene")]
    PrefabHasNoResources,
    #[error("asset {0} is not a data asset; open a prefab as a document to edit it")]
    NotDataAsset(AssetId),
    #[error(transparent)]
    Replay(#[from] ReplayError),
}

/// What the session edits: its scene text is a scene file or a prefab file.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Document {
    #[default]
    Scene,
    Prefab,
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
    /// What undo does, naming the branches it removes, such as
    /// `Undo edit at tick 300 (removes Branch 3, Branch 4)`.
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub scene_dirty: bool,
    pub resimulating: bool,
    pub branch: BranchId,
    pub branches: Vec<BranchInfo>,
    /// The branch the ghost shows: the parent of the current branch unless another is
    /// chosen, none once the ghost is turned off.
    pub compare: Option<BranchId>,
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The prefab and data asset texts the scene loads with, in file form.
    #[serde(default)]
    pub assets: Vec<(AssetId, String)>,
    /// Whether every scene load keeps every asset, as a replay's session does.
    #[serde(default)]
    pub keeps_assets: bool,
}

/// The flattened run of a branch, for a simulation other than the session's.
pub(crate) struct BranchRun {
    pub inputs: InputLog,
    pub edits: EditLog,
    pub hashes: Vec<u64>,
    pub checkpoints: Checkpoints,
    /// The last tick whose recorded hash and checkpoints hold for the current scene and
    /// build; those after it are stale.
    pub fresh: u64,
    /// The first tick whose recorded edits were not applied on the current scene and build:
    /// a stored stale branch has not checked the edits of its first stale tick (its own
    /// edits at its fork tick, after a scene edit).
    pub checked: u64,
}

impl BranchRun {
    /// The last tick of the run.
    pub fn end(&self) -> u64 {
        let inputs = (self.inputs.ticks.len() as u64).saturating_sub(1);
        let edits = self.edits.last_tick().unwrap_or(0);
        (self.hashes.len() as u64 - 1).max(inputs).max(edits)
    }
}

pub struct Session {
    /// The scene file, or the prefab file of a `Document::Prefab`.
    scene: String,
    document: Document,
    assets: AssetTexts,
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
    /// The branch a switch goes to and the tick it asked for, while its stale ancestors
    /// resimulate first, one after the other, as the tree's current branch.
    switching: Option<(BranchId, u64)>,
    /// The warnings of the last load of a new scene or asset version.
    warnings: Vec<String>,
    /// Whether every scene load keeps every asset of `assets`, reached or not, as a
    /// replay's resolved scene needs: it reaches no prefab.
    keeps_assets: bool,
    /// What the ghost shows.
    compare: CompareChoice,
}

/// What the ghost shows: the parent of the current branch unless a branch is chosen.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CompareChoice {
    Parent,
    Off,
    /// Shown while it exists and is not the current branch; the parent otherwise.
    Branch(BranchId),
}

impl Session {
    /// Loads `scene`, a scene or prefab file as `document` says, at tick 0 with empty logs.
    /// `assets` holds the prefab and data asset texts the scene may reach, in file form.
    pub fn start(
        sim: &mut Simulation,
        scene: String,
        assets: AssetTexts,
        document: Document,
    ) -> Result<Session, SessionError> {
        let warnings = load(sim, document, &scene, &assets)?;
        Ok(Session::loaded(sim, scene, assets, document, warnings))
    }

    /// A session at tick 0 with empty logs on the scene `sim` just loaded.
    fn loaded(
        sim: &Simulation,
        scene: String,
        assets: AssetTexts,
        document: Document,
        warnings: Vec<String>,
    ) -> Session {
        let players = u16::try_from(sim.players()).expect("fewer than 2^16 players");
        Session {
            saved_scene: scene.clone(),
            scene,
            document,
            assets,
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
            switching: None,
            warnings,
            keeps_assets: false,
            compare: CompareChoice::Parent,
        }
    }

    /// Restarts an exported session on a new build: the new hashes are compared with the
    /// exported ones while it resimulates. Empty logs, undo history, or branch tree bytes
    /// mean empty logs, an empty history, and a linear session. The other branches are
    /// stale until switched to.
    pub fn resume(
        sim: &mut Simulation,
        export: SessionExport,
        document: Document,
    ) -> Result<Session, SessionError> {
        let mut history = if export.undo.is_empty() {
            UndoHistory::default()
        } else {
            UndoHistory::decode(&export.undo)?
        };
        history.mark_removed_stale()?;
        let inputs = if export.inputs.is_empty() {
            InputLog::new(u16::try_from(sim.players()).expect("fewer than 2^16 players"))
        } else {
            InputLog::decode(&export.inputs)?
        };
        let edits = if export.edits.is_empty() {
            EditLog::new()
        } else {
            EditLog::decode(&export.edits)?
        };
        sim.set_players(usize::from(inputs.players));
        let assets: AssetTexts = export.assets.into_iter().collect();
        let warnings = load_kept(sim, document, &export.scene, &assets, export.keeps_assets)?;
        let mut session = Session::loaded(sim, export.scene, assets, document, warnings);
        session.keeps_assets = export.keeps_assets;
        if let Some(saved) = export.saved_scene {
            session.scene_dirty = saved != session.scene;
            session.saved_scene = saved;
        }
        session.inputs = inputs;
        session.edits = edits;
        session.history = history;
        session.update_head();
        if !export.branches.is_empty() {
            session.tree = BranchTree::decode(&export.branches, &session.edits, session.head)?;
        }
        if !export.hashes.is_empty() {
            session.hashes = export.hashes;
        }
        session.resimulate_against_recorded(sim)?;
        Ok(session)
    }

    /// Opens a replay as a scene session whose recorded hashes are the replay's: they are
    /// compared with the new ones while it resimulates. Fails on the first recorded value
    /// that does not decode, leaving part of the replay loaded in `sim`.
    pub fn from_replay(sim: &mut Simulation, replay: &Replay) -> Result<Session, SessionError> {
        let (inputs, edits) = load_replay(sim, replay)?;
        let assets = replay.assets.iter().cloned().collect();
        let mut session = Session::loaded(
            sim,
            replay.scene.clone(),
            assets,
            Document::Scene,
            Vec::new(),
        );
        session.keeps_assets = true;
        session.inputs = inputs;
        session.edits = edits;
        session.update_head();
        session.hashes.clone_from(&replay.hashes);
        session.resimulate_against_recorded(sim)?;
        Ok(session)
    }

    /// Resimulates the recorded run, reporting the first tick whose new hash differs from
    /// the recorded one, tick 0 included.
    fn resimulate_against_recorded(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        let (expected, got) = (self.hashes[0], sim.hash());
        self.start_resimulation(sim)?;
        if expected != got {
            self.diverged = true;
            self.reports.push_front(Resimulation::Diverged {
                tick: 0,
                expected,
                got,
            });
        }
        Ok(())
    }

    /// Records the replay of `branch`, the current one when `None`, over its recorded
    /// ticks. Recording runs on `sim`, which then comes back to the current tick, also when
    /// the game panics (a stored branch may run ticks this build never ran). Returns the
    /// recording, and the error of coming back when that failed after it ran.
    pub fn export_replay(
        &mut self,
        sim: &mut Simulation,
        branch: Option<BranchId>,
    ) -> (Result<Replay, SessionError>, Option<SessionError>) {
        if let Err(error) = self.ensure_resimulated() {
            return (Err(error), None);
        }
        let (inputs, edits, simulated) = match branch {
            Some(id) if id != self.tree.current() => match self.run_of(sim, id) {
                Ok(run) => run,
                Err(error) => return (Err(error), None),
            },
            _ => (self.inputs.clone(), self.edits.clone(), self.simulated()),
        };
        let inputs_end = inputs.ticks.len() as u64;
        let edits_end = edits.last_tick().map_or(0, |tick| tick.saturating_add(1));
        let ticks = simulated.max(inputs_end).max(edits_end);
        let record = || -> Result<Replay, SessionError> {
            self.load_scene(sim, &self.scene, &self.assets)?;
            sim.set_players(usize::from(inputs.players));
            Ok(record_loaded_replay(sim, &inputs, &edits, ticks)?)
        };
        let recorded = catch_unwind(AssertUnwindSafe(record));
        let tick = sim.tick();
        let reloaded = self.reload(sim).err();
        let recorded = recorded.unwrap_or_else(|payload| {
            Err(SessionError::Panicked {
                tick,
                message: panic_message(&*payload),
            })
        });
        (recorded, reloaded)
    }

    /// The input and edit logs of the stored branch `id`'s run, and its simulated ticks.
    fn run_of(
        &self,
        sim: &Simulation,
        id: BranchId,
    ) -> Result<(InputLog, EditLog, u64), SessionError> {
        if self.tree.get(id).is_none() {
            return Err(SessionError::NoSuchBranch(id));
        }
        let defaults = sim.encoded_default_inputs();
        let (inputs, edits, hashes, _) =
            self.composed(id, &self.current_segment(), defaults.as_deref());
        Ok((inputs, edits, hashes.len() as u64 - 1))
    }

    /// Loads the session's scene into `sim` again and comes back to the current tick, after
    /// `sim` loaded something else.
    pub fn reload(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.load_into(sim)?;
        self.replay_to(sim, self.current)
    }

    /// Loads the session's scene and assets into `sim`, as every scene load of the session
    /// does, with its player count, so load addresses and kept assets match.
    pub(crate) fn load_into(&self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.load_scene(sim, &self.scene, &self.assets)?;
        sim.set_players(usize::from(self.inputs.players));
        Ok(())
    }

    /// The branch the ghost shows: the chosen one while it exists and is not the current
    /// branch, otherwise the parent of the current branch; none once the ghost is turned off.
    pub fn compare_branch(&self) -> Option<BranchId> {
        let current = self.current_branch();
        match self.compare {
            CompareChoice::Off => None,
            CompareChoice::Branch(id) if id != current && self.tree.get(id).is_some() => Some(id),
            _ => self.tree.get(current).and_then(|branch| branch.parent),
        }
    }

    /// Chooses the branch the ghost shows; `None` turns the ghost off until the next fork.
    pub fn set_compare_branch(&mut self, branch: Option<BranchId>) -> Result<(), SessionError> {
        self.compare = match branch {
            Some(id) if self.tree.get(id).is_none() => return Err(SessionError::NoSuchBranch(id)),
            Some(id) => CompareChoice::Branch(id),
            None => CompareChoice::Off,
        };
        Ok(())
    }

    /// The flattened run of branch `id`, the current one included. Its recorded hashes and
    /// checkpoints are fresh up to the first stale tick on its way: a stored branch's
    /// staleness, or the current branch's resimulation. Its edits are checked up to there
    /// too, except those of a stored branch's first stale tick.
    // ponytail: copies the logs and hashes on the way on every call (checkpoints are
    // shared); hand out only what a seek needs if composing shows in profiles.
    pub(crate) fn branch_run(&self, id: BranchId, defaults: Option<&[u8]>) -> BranchRun {
        let (inputs, edits, hashes, checkpoints) =
            self.composed(id, &self.current_segment(), defaults);
        let path = self.tree.path(id);
        let (mut fresh, mut checked) = (u64::MAX, u64::MAX);
        for (at, &branch) in path.iter().enumerate() {
            // Resimulation checks the edits of the tick it got to; a stored stale branch
            // checks those of its first stale tick once resimulated.
            let (stale, unchecked) = if branch == self.tree.current() {
                (self.resimulating, self.resimulating.map(|from| from + 1))
            } else {
                let stale = self.tree.get(branch).and_then(|branch| branch.stale);
                (stale, stale)
            };
            let until = path
                .get(at + 1)
                .and_then(|&child| self.tree.get(child))
                .map_or(u64::MAX, |child| child.fork_tick);
            if let Some(from) = stale.filter(|&from| from < until) {
                fresh = fresh.min(from);
            }
            if let Some(from) = unchecked.filter(|&from| from <= until) {
                checked = checked.min(from);
            }
        }
        let fresh = fresh.min(hashes.len() as u64 - 1);
        BranchRun {
            inputs,
            edits,
            hashes,
            checkpoints,
            fresh,
            checked: checked.min(fresh + 1),
        }
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
        self.history.keep_on_root(&[], &self.edits);
        self.reset_future(sim)
    }

    /// Runs one tick and returns the new tick. Below the head it replays the recorded input
    /// unless `live` differs from it, which takes over: a branch forks at the current tick
    /// and records `live`. Past the end of the recorded inputs, `live` or
    /// default inputs are recorded.
    pub fn step(&mut self, sim: &mut Simulation, live: Option<&[u8]>) -> Result<u64, SessionError> {
        let tick = self.current;
        // While stale ancestors resimulate, the world is theirs: it does not move on.
        if let Some(up_to) = self.resimulated_to()
            && (tick >= up_to || self.switching.is_some())
        {
            return Err(SessionError::Resimulating { up_to });
        }
        self.return_to = None;
        let recorded = self.inputs.tick(tick).map(<[u8]>::to_vec);
        let take_over = live.filter(|live| recorded.as_deref() != Some(*live));
        self.run_tick(sim, take_over.or(recorded.as_deref()))?;
        if let Some(live) = take_over {
            let before = self.tree.current();
            self.fork_or_discard_after(tick);
            self.record_input(sim, tick, live.to_vec());
            if self.tree.current() != before {
                let input = live.to_vec();
                self.record_entry(before, ForkCause::TakeOver { tick, input });
            }
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

    /// Applies `edit` at the current tick. At tick 0 it changes the scene (an asset edit
    /// changes the asset text, an edit on an entity of a prefab instance becomes an
    /// override of the instance) and marks the recorded future stale; past tick 0 it
    /// records the edit, forking a branch when the current one has recorded ticks after
    /// it, after a `LoadAsset` edit for each asset its values reach that is not loaded.
    /// Returns the entity a `Spawn` created.
    pub fn edit(
        &mut self,
        sim: &mut Simulation,
        edit: Edit,
    ) -> Result<Option<Entity>, SessionError> {
        self.ensure_resimulated()?;
        if self.current == 0 {
            if let Edit::SetAsset { id, value } = &edit {
                return self.edit_asset(sim, *id, value).map(|()| None);
            }
            return self.record_scene_edit(sim, &edit);
        }
        let loads = sim.assets_introduced_by(&edit, &self.assets)?;
        let edit = TimedEdit {
            tick: self.current,
            edit,
        };
        let before = self.tree.current();
        let spawned = self.record_edit(sim, loads, edit.clone())?;
        self.record_entry(before, ForkCause::Edit(edit));
        Ok(spawned)
    }

    /// Takes back the last edit, on any branch: it first switches to the entry's branch.
    /// A scene edit reloads the scene before it and marks the recorded future stale. An
    /// edit or take-over that forked a branch deletes it and its descendants and shows the
    /// parent at its tick. Any other recorded edit is removed from its branch's log, the
    /// branch's future after its tick is discarded along with the descendants forked at or
    /// after it, which inherit it, and the session moves to its tick.
    /// An entry whose edit or branch is gone (an edit dropped from a resumed history, a
    /// branch deleted) is dropped with the error, so the history never gets stuck. An
    /// entry on a stale branch switches to it and waits: undo again once resimulated.
    pub fn undo(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let mut entry = self
            .history
            .take_undo()
            .ok_or(SessionError::NothingToUndo)?;
        let result = self.undo_entry(sim, &mut entry);
        if !is_dropped(&result) {
            self.history.undone(entry, result.is_ok());
        }
        self.show(sim, result?)
    }

    /// Applies the last undone entry again: a scene edit reloads the scene after it, a fork
    /// brings back the branches it removed with the same identifiers and names and switches
    /// to the forked one, and a recorded edit puts its branch back as it was before the
    /// undo, with the descendants the undo removed. Play recorded on that branch since the
    /// undo is replaced without warning.
    pub fn redo(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let mut entry = self
            .history
            .take_redo()
            .ok_or(SessionError::NothingToRedo)?;
        let result = self.redo_entry(sim, &mut entry);
        if !is_dropped(&result) {
            self.history.redone(entry, result.is_ok());
        }
        self.show(sim, result?)
    }

    /// Undoes `entry`, leaving the world to `show`: returns the branch and tick to show.
    fn undo_entry(
        &mut self,
        sim: &mut Simulation,
        entry: &mut UndoEntry,
    ) -> Result<Option<(BranchId, u64)>, SessionError> {
        let players = self.inputs.players;
        match entry {
            UndoEntry::Scene { before, .. } => {
                self.load_scene_version(sim, before.clone())?;
                Ok(None)
            }
            UndoEntry::Asset { id, before, .. } => {
                self.load_asset_version(sim, *id, before.clone())?;
                Ok(None)
            }
            UndoEntry::Fork {
                branch,
                cause,
                name,
                removed,
            } => {
                let forked = self
                    .tree
                    .get(*branch)
                    .ok_or(SessionError::NoSuchBranch(*branch))?;
                let parent = forked.parent.expect("a forked branch has a parent");
                name.clone_from(&forked.name);
                let subtree = self.tree.subtree(*branch);
                if subtree.contains(&self.tree.current()) {
                    self.switch_to(sim, parent, cause.tick())?;
                }
                *removed = StoredBranch::encode(&self.tree.take(&subtree, players));
                Ok(Some((parent, cause.tick())))
            }
            UndoEntry::Run {
                branch,
                edit,
                removed,
                restores,
            } => {
                let tick = edit.tick;
                self.switch_to(sim, *branch, tick)?;
                self.ensure_resimulated()?;
                let index = self
                    .own_edit_index(edit)
                    .ok_or(SessionError::NotOnBranch { tick })?;
                let segment = self.current_segment();
                let mut stored = vec![self.tree.store_current(&segment, players)];
                let dependents = self.tree.forked_at_or_after(*branch, tick);
                *restores = self
                    .tree
                    .names(&dependents)
                    .into_iter()
                    .map(String::from)
                    .collect();
                stored.extend(self.tree.take(&dependents, players));
                *removed = StoredBranch::encode(&stored);
                self.remove_edit(index);
                self.remove_loads_before(sim, index);
                self.discard_after(tick);
                Ok(Some((*branch, tick)))
            }
        }
    }

    /// Redoes `entry`, leaving the world to `show`: returns the branch and tick to show.
    fn redo_entry(
        &mut self,
        sim: &mut Simulation,
        entry: &mut UndoEntry,
    ) -> Result<Option<(BranchId, u64)>, SessionError> {
        match entry {
            UndoEntry::Scene { after, .. } => {
                self.load_scene_version(sim, after.clone())?;
                Ok(None)
            }
            UndoEntry::Asset { id, after, .. } => {
                self.load_asset_version(sim, *id, after.clone())?;
                Ok(None)
            }
            UndoEntry::Fork {
                branch,
                cause,
                removed,
                ..
            } => {
                let stored = StoredBranch::decode(removed)?;
                self.tree.put_back(stored)?;
                removed.clear();
                Ok(Some((*branch, cause.tick())))
            }
            UndoEntry::Run {
                branch,
                edit,
                removed,
                restores,
            } => {
                let tick = edit.tick;
                self.switch_to(sim, *branch, tick)?;
                self.ensure_resimulated()?;
                let mut stored = StoredBranch::decode(removed)?.into_iter();
                match stored.next() {
                    Some(own) => {
                        let (segment, stale) = own.into_segment()?;
                        self.tree.put_back(stored.collect())?;
                        let defaults = sim.encoded_default_inputs();
                        self.compose(&segment, defaults.as_deref());
                        if stale {
                            self.resimulate_from(sim, 0, self.current)?;
                        }
                    }
                    // An entry of a history written before branches: as in 2a, within
                    // the branch.
                    None => {
                        self.seek(sim, tick)?;
                        sim.apply_edit(&edit.edit)?;
                        self.discard_after(tick);
                        self.edits.edits.push(edit.clone());
                        self.update_head();
                    }
                }
                removed.clear();
                restores.clear();
                Ok(Some((*branch, tick)))
            }
        }
    }

    /// Switches to `branch` and moves to `tick` once undo or redo changed the logs; a
    /// stale branch moves there once resimulated.
    fn show(
        &mut self,
        sim: &mut Simulation,
        at: Option<(BranchId, u64)>,
    ) -> Result<(), SessionError> {
        let Some((branch, tick)) = at else {
            return Ok(());
        };
        self.switch_to(sim, branch, tick)?;
        if self.switching.is_some() {
            // The switch goes to `tick` once the ancestors are resimulated.
            return Ok(());
        }
        if self.resimulating.is_some() {
            self.return_to = Some(tick);
            return Ok(());
        }
        // The ticks up to `tick` ran before with these same inputs and edits.
        self.current = tick;
        self.replay_to(sim, tick)?;
        if self.edits.at(tick).is_empty() {
            self.keep_checkpoint_if_sparse(sim);
        }
        Ok(())
    }

    /// Records the undo entry of an edit or take-over made on `before`: a fork when it
    /// forked a branch, otherwise a plain recorded edit. A take-over that extends the
    /// branch records nothing, as in 2a.
    fn record_entry(&mut self, before: BranchId, cause: ForkCause) {
        let branch = self.tree.current();
        let entry = match cause {
            ForkCause::Edit(edit) if branch == before => UndoEntry::Run {
                branch,
                edit,
                removed: Vec::new(),
                restores: Vec::new(),
            },
            cause => UndoEntry::Fork {
                branch,
                cause,
                name: self.tree.current_branch().name.clone(),
                removed: Vec::new(),
            },
        };
        self.history.record(entry);
    }

    /// The index of `edit` in the current branch's own part of the run.
    fn own_edit_index(&self, edit: &TimedEdit) -> Option<usize> {
        let first = self.own_first();
        let own = self.edits.edits.get(first..)?;
        Some(first + own.iter().rposition(|e| e == edit)?)
    }

    /// After the edit at `index` was undone: removes the `LoadAsset` edits recorded just
    /// before it, at its tick, unless a later edit of that tick reaches their asset. Leaves
    /// the world at that tick, before its edits. A replay that fails keeps every load: the
    /// undo is done either way, and showing the tick reports the failure.
    fn remove_loads_before(&mut self, sim: &mut Simulation, index: usize) {
        let tick = match self.edits.edits.get(index.wrapping_sub(1)) {
            Some(edit) if matches!(edit.edit, Edit::LoadAsset { .. }) => edit.tick,
            _ => return,
        };
        let mut first = index;
        while first > self.own_first()
            && self.edits.edits[first - 1].tick == tick
            && matches!(self.edits.edits[first - 1].edit, Edit::LoadAsset { .. })
        {
            first -= 1;
        }
        // The assets those loads brought are not loaded before the tick's edits.
        if self.replay_before_edits(sim, tick).is_err() {
            return;
        }
        let mut needed = BTreeSet::new();
        let later = self.edits.edits[index..]
            .iter()
            .take_while(|edit| edit.tick == tick);
        for edit in later {
            match sim.assets_introduced_by(&edit.edit, &self.assets) {
                Ok(loads) => needed.extend(loads.into_iter().filter_map(|load| match load {
                    Edit::LoadAsset { id, .. } => Some(id),
                    _ => None,
                })),
                // A later edit that cannot tell what it reaches keeps every load.
                Err(_) => return,
            }
        }
        for at in (first..index).rev() {
            if let Edit::LoadAsset { id, .. } = self.edits.edits[at].edit
                && !needed.contains(&id)
            {
                self.remove_edit(at);
            }
        }
    }

    /// The index of the first edit of the current branch's own part of the run.
    fn own_first(&self) -> usize {
        let (fork_tick, fork_edits) = self.tree.current_branch().fork();
        self.edits.edits.partition_point(|e| e.tick < fork_tick) + fork_edits
    }

    fn undo_label(&self) -> Option<String> {
        let (what, removes) = match self.history.next_undo()? {
            UndoEntry::Scene { .. } => return Some("Undo scene edit".to_string()),
            UndoEntry::Asset { .. } => return Some("Undo asset edit".to_string()),
            UndoEntry::Run { branch, edit, .. } => (
                format!("edit at tick {}", edit.tick),
                self.tree.forked_at_or_after(*branch, edit.tick),
            ),
            UndoEntry::Fork { branch, cause, .. } => {
                let removes = match self.tree.get(*branch) {
                    Some(_) => self.tree.subtree(*branch),
                    None => Vec::new(),
                };
                (cause.describe(), removes)
            }
        };
        let names = self.tree.names(&removes).join(", ");
        Some(if names.is_empty() {
            format!("Undo {what}")
        } else {
            format!("Undo {what} (removes {names})")
        })
    }

    fn redo_label(&self) -> Option<String> {
        Some(match self.history.next_redo()? {
            UndoEntry::Scene { .. } => "Redo scene edit".to_string(),
            UndoEntry::Asset { .. } => "Redo asset edit".to_string(),
            UndoEntry::Run { edit, restores, .. } if restores.is_empty() => {
                format!("Redo edit at tick {}", edit.tick)
            }
            UndoEntry::Run { edit, restores, .. } => {
                format!(
                    "Redo edit at tick {} (restores {})",
                    edit.tick,
                    restores.join(", ")
                )
            }
            UndoEntry::Fork { cause, name, .. } => {
                format!("Redo {} (restores {name})", cause.describe())
            }
        })
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
        if let Some(up_to) = self.resimulated_to()
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
        // A switch whose ancestors resimulate ends where the session was sought to.
        if let Some((_, at)) = &mut self.switching {
            *at = tick;
        }
        Ok(())
    }

    /// The hash recorded after `tick` ticks, before the edits of `tick`.
    pub(crate) fn recorded_hash(&self, tick: u64) -> Option<u64> {
        self.hashes.get(usize::try_from(tick).ok()?).copied()
    }

    /// The hash recorded after each tick of the current branch, from tick 0, once
    /// resimulated: stale hashes are never compared.
    pub(crate) fn hashes(&self) -> Result<&[u64], SessionError> {
        self.ensure_resimulated()?;
        Ok(&self.hashes)
    }

    /// Whether edits recorded at the current tick are applied but not yet simulated.
    pub fn is_pending(&self) -> bool {
        !self.edits.at(self.current).is_empty()
    }

    /// The scene text, that is the state at tick 0: a prefab file for `Document::Prefab`.
    pub fn scene(&self) -> &str {
        &self.scene
    }

    pub fn document(&self) -> Document {
        self.document
    }

    /// The warnings of the last load of a new scene or asset version, such as stale
    /// overrides.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The prefab and data asset texts the scene loads with, in file form.
    pub fn assets(&self) -> &AssetTexts {
        &self.assets
    }

    /// The file text to save for asset `id`: a data asset's value, or a prefab file.
    pub fn save_asset(&self, id: AssetId) -> Option<String> {
        let text = self.assets.get(&id)?;
        Some(parse_asset_file(text).map_or_else(|_| text.clone(), |(_, value)| value))
    }

    /// Loads a new version of the scene at tick 0 with new asset texts, as saved, and marks
    /// the recorded future stale. Undo brings the previous scene version back.
    pub fn replace_scene(
        &mut self,
        sim: &mut Simulation,
        scene: String,
        assets: AssetTexts,
    ) -> Result<(), SessionError> {
        self.warnings = self.load_scene(sim, &scene, &assets)?;
        self.assets = assets;
        self.saved_scene = scene.clone();
        let before = std::mem::replace(&mut self.scene, scene.clone());
        self.scene_dirty = false;
        if before != scene {
            self.history.record(UndoEntry::Scene {
                before,
                after: scene,
            });
        }
        self.start_resimulation(sim)
    }

    /// Appends an instance of `prefab` to the scene, like a tick 0 edit, and returns its
    /// local identifier.
    pub fn place_instance(
        &mut self,
        sim: &mut Simulation,
        prefab: AssetId,
    ) -> Result<u32, SessionError> {
        self.ensure_resimulated()?;
        let (scene, id) = prefab::place_instance(&self.scene, prefab)?;
        self.change_scene(sim, scene)?;
        Ok(id)
    }

    /// Moves the scene entities `entities` into the new prefab `prefab`, whose file text is
    /// `prefab_text`, and places an instance of it in their stead, like a tick 0 edit.
    /// Returns the instance's local identifier.
    pub fn replace_with_instance(
        &mut self,
        sim: &mut Simulation,
        entities: &[Entity],
        prefab: AssetId,
        prefab_text: String,
    ) -> Result<u32, SessionError> {
        self.ensure_resimulated()?;
        let locals = entities
            .iter()
            .map(
                |entity| match sim.load_addresses().get(entity).map(Vec::as_slice) {
                    Some(&[local]) => Ok(local),
                    Some(address) => Err(SceneError::InstanceEntity(address.to_vec()).into()),
                    None => Err(SessionError::NotInScene(*entity)),
                },
            )
            .collect::<Result<Vec<u32>, SessionError>>()?;
        prefab::save_prefab_checked(prefab, &prefab_text, &self.assets)?;
        let (scene, id) = prefab::replace_with_instance(&self.scene, &locals, prefab)?;
        let previous = self.assets.insert(prefab, prefab_text);
        if let Err(error) = self.change_scene(sim, scene) {
            match previous {
                Some(text) => self.assets.insert(prefab, text),
                None => self.assets.remove(&prefab),
            };
            return Err(error);
        }
        Ok(id)
    }

    /// Sets the override of `component` at `path` on `entity` of the instance at `instance`
    /// to the RON `value`, like a tick 0 edit.
    pub fn set_override(
        &mut self,
        sim: &mut Simulation,
        instance: &[u32],
        entity: &[u32],
        component: &str,
        path: &[PathSegment],
        value: &str,
    ) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let scene = prefab::set_override(&self.scene, instance, entity, component, path, value)?;
        self.change_scene(sim, scene)
    }

    /// Removes that override, like a tick 0 edit; an empty `path` removes every override of
    /// the component.
    pub fn revert_override(
        &mut self,
        sim: &mut Simulation,
        instance: &[u32],
        entity: &[u32],
        component: &str,
        path: &[PathSegment],
    ) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let scene = prefab::revert_override(&self.scene, instance, entity, component, path)?;
        self.change_scene(sim, scene)
    }

    /// Copies the current value of the data asset `id` into its asset text, like a tick 0
    /// asset edit: resimulation brings the session back to the current tick.
    pub fn apply_to_asset(
        &mut self,
        sim: &mut Simulation,
        id: AssetId,
    ) -> Result<(), SessionError> {
        self.ensure_resimulated()?;
        let (asset, value) = sim
            .world()
            .asset_bytes(id)
            .ok_or(EditError::MissingAsset(id))?;
        let text = self.assets.get(&id).ok_or(EditError::MissingAsset(id))?;
        let (_, file_value) = parse_asset_file(text).map_err(|_| SessionError::NotDataAsset(id))?;
        let codec = sim
            .world()
            .asset_codec(asset)
            .expect("a loaded asset's type");
        if (codec.decode_ron)(&file_value).is_ok_and(|file| (codec.encode)(&*file) == value) {
            // The asset text already holds this value: nothing to change or undo.
            return Ok(());
        }
        self.edit_asset(sim, id, &value)
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
            let branch = self.tree.current();
            // ponytail: at the fork tick the branch stops inheriting the dropped edits, but
            // the restored state and its hash sit in the prefix, which switching away and
            // back recomposes from the parent, undoing the restore (before the fork tick,
            // the dropped edits come back too). Store the restored state in the branch if
            // it must survive switching.
            self.tree.edits_dropped_at(tick, dropped.len());
            for edit in &dropped {
                self.history.forget(branch, edit);
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
        if result.is_err() {
            // The switch stops on the branch that failed.
            self.switching = None;
        }
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
        // A stale ancestor of a switch's target resimulates only the part they share.
        let stop = self.chain_stop().unwrap_or(u64::MAX).min(self.end());
        for _ in 0..budget {
            if up_to >= stop {
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
        // A switch goes on with the next stale branch on its way.
        self.continue_switch(sim)?;
        if self.resimulating.is_none() {
            self.reports.push_back(Resimulation::Done);
        }
        let up_to = self.resimulated_to().unwrap_or(up_to);
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
            assets: self
                .assets
                .iter()
                .map(|(&id, text)| (id, text.clone()))
                .collect(),
            keeps_assets: self.keeps_assets,
        })
    }

    /// While a switch resimulates stale ancestors, the target branch as far as it shares
    /// the run of the ancestor resimulating, with the target's head.
    pub fn timeline(&self) -> Timeline {
        let cap = self.chain_cap().unwrap_or(u64::MAX);
        let mut edit_ticks: Vec<u64> = self
            .edits
            .edits
            .iter()
            .map(|edit| edit.tick)
            .take_while(|&tick| tick <= cap)
            .collect();
        edit_ticks.dedup();
        let mut checkpoints = self.checkpoints.ticks();
        checkpoints.retain(|&tick| tick <= cap);
        let branches = self.branches();
        let branch = self.current_branch();
        let head = match self.switching {
            Some(_) => branches
                .iter()
                .find(|info| info.id == branch)
                .map(|info| info.head),
            None => None,
        };
        Timeline {
            head: head.unwrap_or(self.head),
            // While resimulating, the recorded future stays for comparison: only the ticks
            // resimulated count.
            simulated: self.resimulated_to().unwrap_or(self.simulated()).min(cap),
            current: self.current,
            checkpoints,
            edit_ticks,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            undo_label: self.undo_label(),
            redo_label: self.redo_label(),
            scene_dirty: self.scene_dirty,
            resimulating: self.resimulating.is_some(),
            branch,
            branches,
            compare: self.compare_branch(),
        }
    }

    pub fn branches(&self) -> Vec<BranchInfo> {
        self.tree.infos(self.head, &self.edits)
    }

    /// The current branch, or the branch a switch goes to while its ancestors resimulate.
    pub fn current_branch(&self) -> BranchId {
        self.switching
            .map_or(self.tree.current(), |(target, _)| target)
    }

    /// Makes `id` the current branch, at the current tick when it covers it, otherwise at
    /// its head. A stale branch resimulates from its first stale tick, after its stale
    /// ancestors whose part of its run is stale, each from its own first stale tick: until
    /// then the session shows the ticks it shares with them and seeking further fails with
    /// `Resimulating`.
    pub fn switch_branch(
        &mut self,
        sim: &mut Simulation,
        id: BranchId,
    ) -> Result<(), SessionError> {
        self.switch_to(sim, id, self.current)
    }

    /// `switch_branch`, at `tick` instead of the current tick.
    fn switch_to(
        &mut self,
        sim: &mut Simulation,
        id: BranchId,
        tick: u64,
    ) -> Result<(), SessionError> {
        if self.tree.get(id).is_none() {
            return Err(SessionError::NoSuchBranch(id));
        }
        if id == self.current_branch() {
            return Ok(());
        }
        self.drop_reports();
        self.switching = Some((id, tick));
        let result = self
            .switch_step(sim)
            .and_then(|()| self.continue_switch(sim));
        if result.is_err() {
            // The session stays on the branch that failed.
            self.switching = None;
        }
        result
    }

    /// Drops the reports of a resimulation that starts over, except dropped edits: those
    /// edits are gone either way.
    fn drop_reports(&mut self) {
        self.reports
            .retain(|report| matches!(report, Resimulation::EditDropped { .. }));
    }

    /// Enters the next branch on the way of the switch: the first stale ancestor whose
    /// part of the target's run is stale, otherwise the target, which ends the switch.
    fn switch_step(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        let Some((target, tick)) = self.switching else {
            return Ok(());
        };
        match self.first_stale_ancestor(target) {
            Some((ancestor, shared)) => self.enter(sim, ancestor, tick.min(shared)),
            None => {
                self.switching = None;
                self.enter(sim, target, tick)
            }
        }
    }

    /// Takes the switch on while the current branch has nothing left to resimulate.
    /// A stale ancestor counts as done once resimulated up to the child's fork tick: it is
    /// stored fresh up to there.
    fn continue_switch(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        while self.switching.is_some()
            && self
                .resimulating
                .is_none_or(|up_to| self.chain_stop().is_some_and(|stop| up_to >= stop))
        {
            self.switch_step(sim)?;
        }
        Ok(())
    }

    /// The first branch on the path to `target`, other than `target`, whose part of
    /// `target`'s run is stale, with the last tick `target`'s run shares with it.
    fn first_stale_ancestor(&self, target: BranchId) -> Option<(BranchId, u64)> {
        let path = self.tree.path(target);
        path.windows(2).find_map(|pair| {
            let stale = if pair[0] == self.tree.current() {
                self.resimulating
            } else {
                self.tree.get(pair[0])?.stale
            };
            let fork_tick = self.tree.get(pair[1])?.fork_tick;
            stale
                .filter(|&from| from < fork_tick)
                .map(|_| (pair[0], fork_tick.saturating_sub(1)))
        })
    }

    /// While a switch resimulates an ancestor: the fork tick of the next branch on the way,
    /// where the ancestor's resimulation stops.
    fn chain_stop(&self) -> Option<u64> {
        let (target, _) = self.switching?;
        let path = self.tree.path(target);
        let at = path.iter().position(|&id| id == self.tree.current())?;
        Some(self.tree.get(*path.get(at + 1)?)?.fork_tick)
    }

    /// While a switch resimulates an ancestor: the last tick the target's run shares with it.
    fn chain_cap(&self) -> Option<u64> {
        Some(self.chain_stop()?.saturating_sub(1))
    }

    /// The last resimulated tick that may be shown, while resimulating.
    fn resimulated_to(&self) -> Option<u64> {
        let up_to = self.resimulating?;
        Some(self.chain_cap().map_or(up_to, |cap| up_to.min(cap)))
    }

    /// Makes `id` the tree's current branch at `tick` (its head when it ends before), and
    /// resimulates it from its first stale tick when it is stale.
    fn enter(&mut self, sim: &mut Simulation, id: BranchId, tick: u64) -> Result<(), SessionError> {
        let defaults = sim.encoded_default_inputs();
        let segment = self.current_segment();
        let (target, stale) = self.tree.make_current(segment, self.resimulating, id);
        self.compose(&target, defaults.as_deref());
        self.enforce_checkpoint_budget();
        self.resimulating = None;
        self.return_to = None;
        let at = if tick <= self.end() { tick } else { self.head };
        match stale {
            Some(from) => self.resimulate_from(sim, from, at),
            None => {
                self.current = at;
                self.replay_to(sim, at)
            }
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
    /// branch, or one of its ancestors, switches to the parent of `id` first. The undo
    /// entries of the deleted branches go, and so does the redo stack: deleting is not
    /// undoable, and the history never waits on a branch that is gone.
    pub fn delete_branch(
        &mut self,
        sim: &mut Simulation,
        id: BranchId,
    ) -> Result<Vec<BranchId>, SessionError> {
        let branch = self.tree.get(id).ok_or(SessionError::NoSuchBranch(id))?;
        let parent = branch.parent.ok_or(SessionError::RootBranch)?;
        let mut removed = self.tree.subtree(id);
        if removed.contains(&self.current_branch()) {
            self.switch_branch(sim, parent)?;
        }
        self.tree.remove(&removed);
        self.history.forget_branches(&removed);
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
        (self.inputs, self.edits, self.hashes, self.checkpoints) =
            self.composed(self.tree.current(), target, defaults);
        self.update_head();
    }

    /// The flattened run of branch `id`, from its ancestors' segments; `current` stands for
    /// the segment of the branch whose run lives in the session.
    fn composed(
        &self,
        id: BranchId,
        current: &Segment,
        defaults: Option<&[u8]>,
    ) -> (InputLog, EditLog, Vec<u64>, Checkpoints) {
        let mut checkpoints = self.checkpoints.tick_zero();
        let mut inputs = InputLog::new(self.inputs.players);
        let mut edits = EditLog::new();
        let mut hashes = vec![self.hashes[0]];
        for id in self.tree.path(id) {
            let branch = self.tree.get(id).expect("a branch of the path");
            let segment = branch.segment.as_ref().unwrap_or(current);
            segment.append_to(
                branch.fork(),
                &mut inputs,
                &mut edits,
                &mut hashes,
                &mut checkpoints,
                defaults,
            );
        }
        (inputs, edits, hashes, checkpoints)
    }

    /// Removes an edit from the run, keeping the count of edits the current branch inherits
    /// at its fork tick.
    fn remove_edit(&mut self, index: usize) -> TimedEdit {
        let removed = self.edits.edits.remove(index);
        let first = self
            .edits
            .edits
            .partition_point(|edit| edit.tick < removed.tick);
        self.tree.edit_removed_at(removed.tick, index - first);
        removed
    }

    /// Before recording at `tick`: forks a branch when the current one has recorded ticks
    /// after it (the fork rule), then drops the future after `tick` from the run.
    fn fork_or_discard_after(&mut self, tick: u64) {
        if self.end() > tick {
            let segment = self.current_segment();
            let fork = (tick, self.edits.at(tick).len());
            self.compare = CompareChoice::Parent;
            self.tree.fork(segment, self.resimulating, fork);
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
        match self.resimulated_to() {
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
                let message = panic_message(&*payload);
                self.replay_to(sim, self.current)?;
                Err(SessionError::Panicked { tick, message })
            }
        }
    }

    /// Restores the nearest checkpoint at or before `tick` and replays up to `tick`,
    /// recording the hashes and checkpoints of ticks simulated for the first time. Leaves
    /// `current` unchanged.
    fn replay_to(&mut self, sim: &mut Simulation, tick: u64) -> Result<(), SessionError> {
        self.replay_before_edits(sim, tick)?;
        self.apply_edits(sim, tick)
    }

    /// `replay_to`, without applying the edits of `tick` itself.
    fn replay_before_edits(&mut self, sim: &mut Simulation, tick: u64) -> Result<(), SessionError> {
        let (mut at, snapshot) = self.checkpoints.nearest_at_or_before(tick);
        sim.restore(snapshot)
            .expect("a checkpoint restores into the world it was taken from");
        while at < tick {
            self.apply_edits(sim, at)?;
            self.run_tick(sim, self.inputs.tick(at).map(<[u8]>::to_vec).as_deref())?;
            at += 1;
            self.record_tick(sim);
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
                    self.history.forget(self.tree.current(), &dropped);
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
        self.update_head();
        // A live take-over below the resimulated point discards the whole stale future.
        if self.resimulating.is_some_and(|up_to| up_to > tick) {
            self.resimulating = None;
        }
    }

    /// Applies `loads`, then `edit`, at the current tick, forks or discards the future after
    /// it, and records them. Nothing changes when one fails.
    fn record_edit(
        &mut self,
        sim: &mut Simulation,
        loads: Vec<Edit>,
        edit: TimedEdit,
    ) -> Result<Option<Entity>, SessionError> {
        let tick = edit.tick;
        let checkpoint = (self.edits.at(tick).is_empty() && self.checkpoints.is_sparse_at(tick))
            .then(|| sim.snapshot());
        let before_loads = (!loads.is_empty()).then(|| sim.snapshot());
        let applied = loads
            .iter()
            .chain([&edit.edit])
            .map(|edit| sim.apply_edit(edit))
            .collect::<Result<Vec<_>, EditError>>();
        let spawned = match applied {
            Ok(applied) => applied.last().copied().flatten(),
            Err(error) => {
                if let Some(state) = before_loads {
                    sim.restore(&state)
                        .expect("a snapshot restores into the world it was taken from");
                }
                return Err(error.into());
            }
        };
        self.fork_or_discard_after(tick);
        if let Some(checkpoint) = checkpoint {
            self.checkpoints.pin(tick, checkpoint);
        }
        let loads = loads.into_iter().map(|edit| TimedEdit { tick, edit });
        self.edits.edits.extend(loads);
        self.edits.edits.push(edit);
        self.update_head();
        Ok(spawned)
    }

    /// Loads `text`, the session's scene or a version of it, with `assets`; returns the load
    /// warnings.
    fn load_scene(
        &self,
        sim: &mut Simulation,
        text: &str,
        assets: &AssetTexts,
    ) -> Result<Vec<String>, SessionError> {
        load_kept(sim, self.document, text, assets, self.keeps_assets)
    }

    /// Loads a scene version from the undo history, like a tick 0 edit.
    fn load_scene_version(
        &mut self,
        sim: &mut Simulation,
        scene: String,
    ) -> Result<(), SessionError> {
        self.warnings = self.load_scene(sim, &scene, &self.assets)?;
        self.set_scene(scene);
        self.start_resimulation(sim)
    }

    /// Loads `scene` like a tick 0 edit, recorded in the undo history; the same scene
    /// changes nothing.
    fn change_scene(&mut self, sim: &mut Simulation, scene: String) -> Result<(), SessionError> {
        if scene == self.scene {
            return Ok(());
        }
        let before = self.scene.clone();
        self.load_scene_version(sim, scene)?;
        let after = self.scene.clone();
        self.history.record(UndoEntry::Scene { before, after });
        Ok(())
    }

    /// Sets the value of the data asset `id` in its asset text and reloads the scene with
    /// it, like a tick 0 edit, recorded in the undo history.
    fn edit_asset(
        &mut self,
        sim: &mut Simulation,
        id: AssetId,
        value: &[u8],
    ) -> Result<(), SessionError> {
        let before = self.assets.get(&id).ok_or(EditError::MissingAsset(id))?;
        let (asset, _) = parse_asset_file(before).map_err(|_| SessionError::NotDataAsset(id))?;
        let invalid = |reason: String| EditError::InvalidValue {
            name: asset.clone(),
            reason,
        };
        let codec = sim
            .world()
            .asset_codec(&asset)
            .ok_or_else(|| invalid("the asset type is not registered".to_string()))?;
        let decoded = (codec.decode)(value).map_err(invalid)?;
        let after = asset_file(&asset, &(codec.encode_ron)(&*decoded));
        let before = before.clone();
        self.load_asset_version(sim, id, after.clone())?;
        self.history.record(UndoEntry::Asset { id, before, after });
        Ok(())
    }

    /// Replaces the asset text of `id` and reloads the scene with it, like a tick 0 edit.
    fn load_asset_version(
        &mut self,
        sim: &mut Simulation,
        id: AssetId,
        text: String,
    ) -> Result<(), SessionError> {
        let previous = self.assets.insert(id, text);
        match self.load_scene(sim, &self.scene, &self.assets) {
            Ok(warnings) => self.warnings = warnings,
            Err(error) => {
                match previous {
                    Some(text) => self.assets.insert(id, text),
                    None => self.assets.remove(&id),
                };
                return Err(error);
            }
        }
        self.start_resimulation(sim)
    }

    /// After new logs, which define a new linear run: only tick 0 stays simulated, the
    /// session is at tick 0, and recorded edits are checked on this state by resimulation.
    fn reset_future(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.history
            .keep_on_root(&self.tree.path(self.tree.current()), &self.edits);
        self.tree = BranchTree::default();
        self.switching = None;
        self.hashes.truncate(1);
        self.update_head();
        self.current = 0;
        let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
        sim.restore(scene_state)
            .expect("a checkpoint restores into the world it was taken from");
        self.start_resimulation(sim)
    }

    fn set_scene(&mut self, scene: String) {
        self.scene_dirty = scene != self.saved_scene;
        self.scene = scene;
    }

    /// Applies `edit` to the scene and records it in the undo history when it changed it.
    fn record_scene_edit(
        &mut self,
        sim: &mut Simulation,
        edit: &Edit,
    ) -> Result<Option<Entity>, SessionError> {
        let before = self.scene.clone();
        let spawned = self.edit_scene(sim, edit)?;
        if self.scene != before {
            let after = self.scene.clone();
            self.history.record(UndoEntry::Scene { before, after });
        }
        Ok(spawned)
    }

    /// Applies `edit` to the scene: on an entity of a prefab instance it becomes an override
    /// of the instance, otherwise the scene is saved, keeping its local identifiers, and
    /// reloaded so entity indices and generations match any later load of it.
    fn edit_scene(
        &mut self,
        sim: &mut Simulation,
        edit: &Edit,
    ) -> Result<Option<Entity>, SessionError> {
        if self.document == Document::Prefab && matches!(edit, Edit::SetResource { .. }) {
            return Err(SessionError::PrefabHasNoResources);
        }
        let (_, scene_state) = self.checkpoints.nearest_at_or_before(0);
        sim.restore(scene_state)
            .expect("a checkpoint restores into the world it was taken from");
        let loaded: BTreeSet<LoadAddress> = sim.load_addresses().values().cloned().collect();
        let scene = match self.scene_after(sim, edit) {
            Ok(scene) if scene != self.scene => scene,
            result => {
                self.replay_to(sim, self.current)?;
                return result.map(|_| None);
            }
        };
        match self.load_scene(sim, &scene, &self.assets) {
            Ok(warnings) => self.warnings = warnings,
            Err(error) => {
                self.replay_to(sim, self.current)?;
                return Err(error);
            }
        }
        self.set_scene(scene);
        // The scene keeps every other entity's address: a spawned one has a new address.
        let spawned = sim
            .load_addresses()
            .iter()
            .find(|(_, address)| !loaded.contains(*address))
            .map(|(&entity, _)| entity);
        self.start_resimulation(sim)?;
        Ok(spawned)
    }

    /// The scene text after `edit`, which the world, at tick 0, may hold applied.
    fn scene_after(&self, sim: &mut Simulation, edit: &Edit) -> Result<String, SessionError> {
        if let Some(scene) = self.instance_edit(sim, edit)? {
            return Ok(scene);
        }
        sim.apply_edit(edit)?;
        let scene = sim.save_scene_keeping(&scene_form(self.document, &self.scene)?);
        Ok(match self.document {
            Document::Scene => scene,
            Document::Prefab => prefab::scene_as_prefab(&scene)?,
        })
    }

    /// The scene text after `edit` when it targets an entity of a prefab instance: an
    /// override, an added or a removed component of the instance. Despawning such an
    /// entity is refused.
    fn instance_edit(&self, sim: &Simulation, edit: &Edit) -> Result<Option<String>, SessionError> {
        let (target, component) = match edit {
            Edit::Insert {
                entity, component, ..
            }
            | Edit::Remove { entity, component } => (entity, Some(component)),
            Edit::Despawn { entity } => (entity, None),
            _ => return Ok(None),
        };
        let address = match target {
            EntityRef::Loaded(address) => address.clone(),
            EntityRef::Runtime(entity) => match sim.load_addresses().get(entity) {
                Some(address) => address.clone(),
                None => return Ok(None),
            },
        };
        let [instance, inner @ ..] = &address[..] else {
            return Ok(None);
        };
        if inner.is_empty() {
            return Ok(None);
        }
        let entity = sim.resolve_ref(&EntityRef::Loaded(address.clone()))?;
        let Some(component) = component else {
            return Err(SceneError::InstanceEntity(address).into());
        };
        if sim.world().component_codec(component).is_none() {
            return Err(EditError::UnknownComponent(component.clone()).into());
        }
        let has = sim.world().has_component_named(entity, component);
        let scene = match edit {
            Edit::Insert { value, .. } => {
                let value = override_value(sim, &address, component, value)?;
                if has && self.prefab_has(sim, &address, component)? {
                    prefab::set_override(&self.scene, &[*instance], inner, component, &[], &value)?
                } else {
                    prefab::add_component_override(
                        &self.scene,
                        &[*instance],
                        inner,
                        component,
                        &value,
                    )?
                }
            }
            _ if has => {
                prefab::remove_component_override(&self.scene, &[*instance], inner, component)?
            }
            _ => self.scene.clone(),
        };
        Ok(Some(scene))
    }

    /// Whether the prefab of the instance holding the entity at `address` gives it
    /// `component`, before the instance's own changes.
    fn prefab_has(
        &self,
        sim: &Simulation,
        address: &[u32],
        component: &str,
    ) -> Result<bool, SessionError> {
        let instance = sim
            .instances()
            .into_iter()
            .find(|instance| instance.address == address[..1])
            .expect("an instance entity's instance");
        let text = self
            .assets
            .get(&instance.prefab)
            .ok_or(EditError::MissingAsset(instance.prefab))?;
        let resolved = ludenea_app::resolve(&prefab::prefab_as_scene(text)?, &self.assets)?;
        Ok(resolved
            .entities
            .iter()
            .find(|entity| entity.address == address[1..])
            .is_some_and(|entity| entity.components.iter().any(|(name, _)| name == component)))
    }

    /// The world holds a new tick 0 state, before the edits of tick 0: the recorded future
    /// is kept but its checkpoints are dropped, and it is resimulated from tick 0 when it has
    /// hashes to replace or edits past tick 0 to check (a recorded edit that fails on the new
    /// state is dropped and reported; inputs alone always replay). The new tick 0 hash is not
    /// compared, since the state changed on purpose. The session shows tick 0 while it
    /// resimulates, then returns to the tick it was at (or to the last resimulated tick when
    /// the recorded future ends before it).
    /// The other branches are marked stale, and so is the rest of a switch's way.
    fn start_resimulation(&mut self, sim: &mut Simulation) -> Result<(), SessionError> {
        self.tree.mark_stale();
        self.drop_reports();
        self.checkpoints = Checkpoints::new(sim.snapshot());
        self.hashes[0] = sim.hash();
        self.resimulate_from(sim, 0, self.current)
    }

    /// Resimulates the current branch from `from`, whose state and edits are known good,
    /// up to its end: the recorded future after `from` is kept, its checkpoints dropped, and
    /// it is resimulated when it has hashes to replace or edits to check. The session shows
    /// `at` when it is before `from`, otherwise `from` until resimulation gets to `at`.
    fn resimulate_from(
        &mut self,
        sim: &mut Simulation,
        from: u64,
        at: u64,
    ) -> Result<(), SessionError> {
        self.checkpoints.truncate_after(from);
        self.replay_before_edits(sim, from)?;
        self.diverged = false;
        self.apply_edits_or_drop(sim, from);
        let stale =
            self.simulated() > from || self.edits.last_tick().is_some_and(|tick| tick > from);
        self.resimulating = stale.then_some(from);
        self.return_to = None;
        if stale && at > from {
            self.current = from;
            self.return_to = Some(at);
            Ok(())
        } else {
            self.current = at;
            if at == from {
                Ok(())
            } else {
                self.replay_to(sim, at)
            }
        }
    }
}

/// `load`, keeping every asset of `assets` when `keeps_assets`.
fn load_kept(
    sim: &mut Simulation,
    document: Document,
    text: &str,
    assets: &AssetTexts,
    keeps_assets: bool,
) -> Result<Vec<String>, SessionError> {
    let warnings = load(sim, document, text, assets)?;
    if keeps_assets {
        keep_assets(sim, assets.iter().map(|(&id, text)| (id, text.as_str())))?;
    }
    Ok(warnings)
}

/// Loads `text`, a scene or prefab file as `document` says, with `assets`; returns the
/// load warnings.
fn load(
    sim: &mut Simulation,
    document: Document,
    text: &str,
    assets: &AssetTexts,
) -> Result<Vec<String>, SessionError> {
    Ok(sim
        .load_scene_with(&scene_form(document, text)?, assets)?
        .warnings)
}

/// The scene a simulation loads for `text`: a prefab file becomes a scene.
fn scene_form(document: Document, text: &str) -> Result<String, SessionError> {
    Ok(match document {
        Document::Scene => text.to_string(),
        Document::Prefab => prefab::prefab_as_scene(text)?,
    })
}

/// The RON of `component`'s `value` as an override value of the instance entity at
/// `address`, checked to load back, so a bad value never reaches the scene text.
fn override_value(
    sim: &Simulation,
    address: &[u32],
    component: &str,
    value: &[u8],
) -> Result<String, SessionError> {
    let codec = sim
        .world()
        .component_codec(component)
        .ok_or_else(|| EditError::UnknownComponent(component.to_string()))?;
    let invalid = |reason: String| EditError::InvalidValue {
        name: component.to_string(),
        reason,
    };
    (codec.decode)(value).map_err(invalid)?;
    let ron = sim.override_value_ron(address, component, value)?;
    (codec.decode_ron)(&ron).map_err(invalid)?;
    Ok(ron)
}

/// The message of a panic's payload.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| message.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// Whether an undo or redo failed for good: its entry is dropped. That includes removed
/// branches that do not decode or do not fit the tree (`BranchesDecode`, such as
/// `BranchesError::Tree` or `Fork`), which only corrupt data holds.
fn is_dropped<T>(result: &Result<T, SessionError>) -> bool {
    matches!(
        result,
        Err(SessionError::NotOnBranch { .. }
            | SessionError::NoSuchBranch(_)
            | SessionError::BranchesDecode(_))
    )
}

#[cfg(test)]
mod tests {
    use ludenea_app::ValueKind;
    use serde_json::json;

    use super::*;

    #[test]
    fn redo_whose_removed_branches_do_not_decode_is_dropped() {
        let mut sim = boxes::app().build().unwrap();
        let scene = boxes::generate_scene(10, 3);
        let mut session =
            Session::start(&mut sim, scene, AssetTexts::new(), Document::Scene).unwrap();
        for _ in 0..50 {
            session.step(&mut sim, None).unwrap();
        }
        let value = sim
            .encode_json_value(
                ValueKind::Resource,
                "boxes::ContactStats",
                json!({ "begins": 1, "ends": 0 }),
            )
            .unwrap();
        let resource = "boxes::ContactStats".to_string();
        session
            .edit(&mut sim, Edit::SetResource { resource, value })
            .unwrap();
        session.undo(&mut sim).unwrap();
        let mut entry = session.history.take_redo().unwrap();
        if let UndoEntry::Run { removed, .. } = &mut entry {
            *removed = vec![0xff; 3];
        }
        session.history.redone(entry, false);

        assert!(matches!(
            session.redo(&mut sim),
            Err(SessionError::BranchesDecode(_))
        ));
        assert!(!session.timeline().can_redo);
    }
}
