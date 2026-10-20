//! The undo and redo history of an editor session.

use ludenea_app::{Edit, EditLog, TimedEdit};
use ludenea_ecs::AssetId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::branches::{BranchId, BranchTree, BranchesError, StoredBranch};

/// The version `UndoHistory::encode` writes first. Milestone 2a wrote no version.
pub const UNDO_VERSION: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum UndoError {
    #[error("the undo history cannot be decoded: {0}")]
    Decode(#[from] postcard::Error),
    #[error("the undo history has version {version}; this editor reads up to {UNDO_VERSION}")]
    Version { version: u32 },
    #[error("the undo history has version {version}, which cannot hold {content}")]
    NotInVersion { version: u32, content: String },
}

/// One history per session, across branches: undoing or redoing an entry first switches to
/// the branch it belongs to. `removed` holds the branches the undo took out of the tree,
/// encoded, and is empty until the entry is undone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum UndoEntry {
    /// A tick 0 edit: the scene text before and after it.
    // ponytail: each entry holds two full scene texts (the next entry's `before` repeats
    // this `after`); store only `before` and derive `after` if large scenes make the
    // history too big.
    Scene { before: String, after: String },
    /// An edit recorded past tick 0 on `branch`. Its undo also removes the descendants
    /// forked at or after its tick, and `removed` keeps `branch`'s own segment first, so
    /// redo puts back exactly what the undo took.
    Run {
        branch: BranchId,
        edit: TimedEdit,
        removed: Vec<u8>,
        /// The names of the descendants in `removed`, for the redo label; not encoded,
        /// read back from `removed` on decode.
        #[serde(skip)]
        restores: Vec<String>,
    },
    /// An edit or take-over that forked `branch`, named `name` when last undone.
    Fork {
        branch: BranchId,
        cause: ForkCause,
        name: String,
        removed: Vec<u8>,
    },
    /// A tick 0 asset edit: the asset text before and after it. After the variants of
    /// earlier versions, so histories written before it still decode.
    Asset {
        id: AssetId,
        before: String,
        after: String,
    },
    /// An override moved into its prefab at tick 0: the text of the prefab `id` and the
    /// scene text, each before and after it. Last, so version 3 histories still decode.
    AppliedToPrefab {
        id: AssetId,
        before: String,
        after: String,
        scene_before: String,
        scene_after: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ForkCause {
    Edit(TimedEdit),
    TakeOver { tick: u64, input: Vec<u8> },
}

impl ForkCause {
    pub fn tick(&self) -> u64 {
        match self {
            ForkCause::Edit(edit) => edit.tick,
            ForkCause::TakeOver { tick, .. } => *tick,
        }
    }

    /// What the timeline calls it, as in "Undo edit at tick 300".
    pub fn describe(&self) -> String {
        match self {
            ForkCause::Edit(edit) => format!("edit at tick {}", edit.tick),
            ForkCause::TakeOver { tick, .. } => format!("take-over at tick {tick}"),
        }
    }
}

impl UndoEntry {
    fn branch(&self) -> Option<BranchId> {
        match self {
            UndoEntry::Scene { .. }
            | UndoEntry::Asset { .. }
            | UndoEntry::AppliedToPrefab { .. } => None,
            UndoEntry::Run { branch, .. } | UndoEntry::Fork { branch, .. } => Some(*branch),
        }
    }

    /// What the timeline calls it, as in "Undo edit at tick 300".
    pub fn describe(&self) -> String {
        match self {
            UndoEntry::Scene { .. } => "scene edit".to_string(),
            UndoEntry::Asset { .. } => "asset edit".to_string(),
            UndoEntry::AppliedToPrefab { .. } => "apply to prefab".to_string(),
            UndoEntry::Run { edit, .. } => format!("edit at tick {}", edit.tick),
            UndoEntry::Fork { cause, .. } => cause.describe(),
        }
    }

    /// Whether undoing it works on `tree`, or why not.
    fn undo_fits(&self, tree: &BranchTree) -> Result<(), String> {
        match self {
            UndoEntry::Scene { .. }
            | UndoEntry::Asset { .. }
            | UndoEntry::AppliedToPrefab { .. } => Ok(()),
            UndoEntry::Run { branch, .. } => known(tree, *branch),
            UndoEntry::Fork { branch, cause, .. } => not_root(*branch)
                .and_then(|()| known(tree, *branch))
                .and_then(|()| forked_at(tree, *branch, cause.tick())),
        }
    }

    /// Whether redoing it works on `tree`, whose current branch's run records
    /// `current_edits` up to `current_head`, or why not. When it works, `tree` is left with
    /// the branches the redo puts back.
    fn redo_fits(
        &self,
        tree: &mut BranchTree,
        current_edits: &EditLog,
        current_head: u64,
    ) -> Result<(), String> {
        match self {
            UndoEntry::Scene { .. }
            | UndoEntry::Asset { .. }
            | UndoEntry::AppliedToPrefab { .. } => Ok(()),
            UndoEntry::Fork {
                branch,
                cause,
                removed,
                ..
            } => {
                not_root(*branch)?;
                let stored = StoredBranch::decode(removed).map_err(unfit)?;
                let ids: Vec<BranchId> = stored.iter().map(StoredBranch::id).collect();
                tree.put_back(stored, current_edits, current_head)
                    .map_err(unfit)?;
                known(tree, *branch)
                    .and_then(|()| forked_at(tree, *branch, cause.tick()))
                    .inspect_err(|_| tree.remove(&ids))
            }
            UndoEntry::Run {
                branch,
                edit,
                removed,
                ..
            } => {
                let mut stored = removed_by_run(removed).map_err(unfit)?.into_iter();
                let Some(target) = tree.get_mut(*branch) else {
                    return Err(unknown(*branch));
                };
                // An entry of a history written before branches, or one `keep_on_root`
                // moved to the root, redoes within its branch.
                let Some(own) = stored.next() else {
                    return tree
                        .check_redo_within(*branch, edit.tick)
                        .map_err(not_within);
                };
                edit_after_fork(*branch, target.fork_tick, edit.tick)?;
                let (segment, _) = own.into_segment().map_err(unfit)?;
                // Redo brings the branch's run back as `own` holds it, and puts back the
                // descendants forked on that run.
                let undone = target.segment.replace(segment);
                let result = tree.put_back(stored.collect(), current_edits, current_head);
                if result.is_err()
                    && let Some(target) = tree.get_mut(*branch)
                {
                    target.segment = undone;
                }
                result.map_err(unfit)
            }
        }
    }
}

/// The branches the undo of a `Run` entry removed, its branch's own first. An entry of a
/// history written before branches holds none, and so does one `UndoHistory::keep_on_root`
/// moved to the root; both redo within their branch.
pub(crate) fn removed_by_run(removed: &[u8]) -> Result<Vec<StoredBranch>, BranchesError> {
    if removed.is_empty() {
        return Ok(Vec::new());
    }
    StoredBranch::decode(removed)
}

fn known(tree: &BranchTree, branch: BranchId) -> Result<(), String> {
    match tree.get(branch) {
        Some(_) => Ok(()),
        None => Err(unknown(branch)),
    }
}

/// Refuses a fork entry whose tick is not the fork tick of `branch`, a branch of `tree`:
/// undoing or redoing it would show that tick, however far.
pub(crate) fn forked_at(tree: &BranchTree, branch: BranchId, tick: u64) -> Result<(), String> {
    let fork_tick = tree.get(branch).map_or(tick, |branch| branch.fork_tick);
    if tick == fork_tick {
        return Ok(());
    }
    Err(format!(
        "forked at tick {tick}, but branch {} forks at tick {fork_tick}",
        branch.0
    ))
}

/// Refuses an edit at `tick` on `branch`, which forks at `fork_tick`, before the fork: the
/// branch's own part of the run starts there.
pub(crate) fn edit_after_fork(branch: BranchId, fork_tick: u64, tick: u64) -> Result<(), String> {
    if tick >= fork_tick {
        return Ok(());
    }
    Err(format!(
        "records its edit at tick {tick}, before branch {} forks at tick {fork_tick}",
        branch.0
    ))
}

fn unknown(branch: BranchId) -> String {
    format!("names branch {}, which the session does not have", branch.0)
}

fn not_root(branch: BranchId) -> Result<(), String> {
    match branch {
        BranchId::ROOT => Err(ROOT_FORKED.to_string()),
        _ => Ok(()),
    }
}

/// Why a `Fork` entry naming the root cannot be undone or redone.
pub(crate) const ROOT_FORKED: &str =
    "names the root branch as the branch it forked, but the root forks from nothing";

/// Why a redo cannot put back the branches it restores.
pub(crate) fn unfit(error: BranchesError) -> String {
    format!("restores branches that do not fit: {error}")
}

/// Why an entry of a history written before branches cannot be redone within its branch.
pub(crate) fn not_within(error: BranchesError) -> String {
    format!("cannot be redone within its branch: {error}")
}

/// An entry moves from `undo` to `redo` when undone, and back when redone; an entry whose
/// undo or redo fails goes back where it came from.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct UndoHistory {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
}

impl UndoHistory {
    /// A new edit: pushed on the undo stack, and the redo stack is cleared.
    pub fn record(&mut self, entry: UndoEntry) {
        self.undo.push(entry);
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn next_undo(&self) -> Option<&UndoEntry> {
        self.undo.last()
    }

    pub fn next_redo(&self) -> Option<&UndoEntry> {
        self.redo.last()
    }

    /// Takes the entry to undo; `undone` puts it back.
    pub fn take_undo(&mut self) -> Option<UndoEntry> {
        self.undo.pop()
    }

    /// Ends the undo of `entry`: redoable when `succeeded`, otherwise still undoable.
    pub fn undone(&mut self, entry: UndoEntry, succeeded: bool) {
        if succeeded {
            self.redo.push(entry);
        } else {
            self.undo.push(entry);
        }
    }

    /// Takes the entry to redo; `redone` puts it back.
    pub fn take_redo(&mut self) -> Option<UndoEntry> {
        self.redo.pop()
    }

    /// Ends the redo of `entry`: undoable when `succeeded`, otherwise still redoable.
    pub fn redone(&mut self, entry: UndoEntry, succeeded: bool) {
        if succeeded {
            self.undo.push(entry);
        } else {
            self.redo.push(entry);
        }
    }

    /// Drops the entries of an edit removed from `branch`'s run.
    pub fn forget(&mut self, branch: BranchId, edit: &TimedEdit) {
        let other = |entry: &UndoEntry| !matches!(entry, UndoEntry::Run { branch: b, edit: e, .. } if *b == branch && e == edit);
        self.undo.retain(other);
        self.redo.retain(other);
    }

    /// After a user deleted `branches`: their entries go, and so does the redo stack, whose
    /// removed branches may hang off them. Deleting is a new action, like an edit.
    pub fn forget_branches(&mut self, branches: &[BranchId]) {
        self.undo
            .retain(|entry| entry.branch().is_none_or(|b| !branches.contains(&b)));
        self.redo.clear();
    }

    /// After the tree was reset to a single root holding `run`, the run of `path`'s last
    /// branch: the edits of `path` that `run` holds now belong to the root, and every other
    /// entry past tick 0 goes. The branches their undo removed are gone with the tree, so
    /// redo works within the root, as in 2a.
    pub fn keep_on_root(&mut self, path: &[BranchId], run: &EditLog) {
        let keep = |entry: &mut UndoEntry| match entry {
            UndoEntry::Scene { .. }
            | UndoEntry::Asset { .. }
            | UndoEntry::AppliedToPrefab { .. } => true,
            UndoEntry::Run {
                branch,
                edit,
                removed,
                restores,
            } if path.contains(branch) && run.edits.contains(edit) => {
                *branch = BranchId::ROOT;
                removed.clear();
                restores.clear();
                true
            }
            _ => false,
        };
        self.undo.retain_mut(keep);
        self.redo.retain_mut(keep);
    }

    /// Drops, after a resume, the entries the session cannot undo or redo: those naming a
    /// branch the session does not have or the root as the branch a fork made, and the redo
    /// entries whose branches do not fit back, such as one forked past its parent's run,
    /// which redo would pad without bound, or that would cut a branch off by redoing within
    /// theirs. `tree` is a copy of the session's tree, whose current branch's run records
    /// `current_edits` up to `current_head`. Redo takes the last entry first, so each redo
    /// entry is checked on `tree` as the redo entries after it leave it. Returns a warning
    /// naming each dropped entry.
    pub fn drop_unfit(
        &mut self,
        mut tree: BranchTree,
        current_edits: &EditLog,
        current_head: u64,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        let mut keep = |action: &str, entry: &UndoEntry, fits: Result<(), String>| {
            fits.map_err(|reason| {
                let what = entry.describe();
                warnings.push(format!(
                    "the undo history's `{action} {what}` {reason}; it was dropped"
                ));
            })
            .is_ok()
        };
        self.undo
            .retain(|entry| keep("Undo", entry, entry.undo_fits(&tree)));
        self.redo.reverse();
        self.redo.retain(|entry| {
            let fits = entry.redo_fits(&mut tree, current_edits, current_head);
            keep("Redo", entry, fits)
        });
        self.redo.reverse();
        warnings
    }

    /// After a rebuild: the branches undo removed hold hashes of the previous build, so
    /// they resimulate when redo brings them back. Only redo entries hold removed branches.
    pub fn mark_removed_stale(&mut self) -> Result<(), BranchesError> {
        for entry in &mut self.redo {
            if let UndoEntry::Run { removed, .. } | UndoEntry::Fork { removed, .. } = entry
                && !removed.is_empty()
            {
                *removed = StoredBranch::mark_stale(removed)?;
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(&(UNDO_VERSION, self)).expect("an undo history always encodes")
    }

    /// Decodes the current version, version 3 (the current entries but the last), version
    /// 2, or the unversioned 2a form, the last two with every entry on the root branch (2a
    /// edit targets as `EntityRef::Runtime`): an entry of version 2 recorded on another
    /// branch then fails to undo as `NotOnBranch` and is dropped. All start with a varint,
    /// so each versioned form is tried first and must use every byte.
    pub fn decode(bytes: &[u8]) -> Result<UndoHistory, UndoError> {
        if let Ok((version @ (3 | UNDO_VERSION), mut history)) =
            decode_exact::<(u32, UndoHistory)>(bytes)
        {
            if version == 3 {
                history.refuse_version_4_content(version)?;
            }
            for entry in history.undo.iter_mut().chain(&mut history.redo) {
                if let UndoEntry::Run {
                    removed, restores, ..
                } = entry
                {
                    *restores = StoredBranch::names_of_descendants(removed);
                }
            }
            return Ok(history);
        }
        if let Ok((2, history)) = decode_exact::<(u32, version_2::UndoHistory)>(bytes) {
            let history = UndoHistory::from(history);
            history.refuse_version_4_content(2)?;
            return Ok(history);
        }
        match decode_exact::<version_2a::UndoHistory>(bytes) {
            Ok(history) => Ok(history.into()),
            // Not 2a either: a version number above ours names a newer editor.
            Err(error) => match postcard::take_from_bytes::<u32>(bytes) {
                Ok((version, _)) if version > UNDO_VERSION => Err(UndoError::Version { version }),
                _ => Err(error.into()),
            },
        }
    }

    /// Versions 2 and 3 decode with the current entries and edits, but groups and "Apply to
    /// prefab" entries arrived with version 4.
    fn refuse_version_4_content(&self, version: u32) -> Result<(), UndoError> {
        for entry in self.undo.iter().chain(&self.redo) {
            let content = match entry {
                UndoEntry::Run { edit, .. }
                | UndoEntry::Fork {
                    cause: ForkCause::Edit(edit),
                    ..
                } if matches!(edit.edit, Edit::Group(_)) => {
                    format!("a group (the edit at tick {})", edit.tick)
                }
                UndoEntry::AppliedToPrefab { .. } => "an \"Apply to prefab\" entry".to_string(),
                _ => continue,
            };
            return Err(UndoError::NotInVersion { version, content });
        }
        Ok(())
    }
}

fn decode_exact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, postcard::Error> {
    match postcard::take_from_bytes(bytes)? {
        (value, []) => Ok(value),
        _ => Err(postcard::Error::DeserializeBadEncoding),
    }
}

/// The undo history version 2 wrote: no branches, every entry on the root.
mod version_2 {
    use ludenea_app::TimedEdit;
    use serde::Deserialize;

    use crate::branches::BranchId;

    #[derive(Deserialize)]
    pub(super) enum UndoEntry {
        Scene { before: String, after: String },
        Run { edit: TimedEdit },
    }

    #[derive(Deserialize)]
    pub(super) struct UndoHistory {
        pub undo: Vec<UndoEntry>,
        pub redo: Vec<UndoEntry>,
    }

    impl From<UndoEntry> for super::UndoEntry {
        fn from(entry: UndoEntry) -> Self {
            match entry {
                UndoEntry::Scene { before, after } => super::UndoEntry::Scene { before, after },
                UndoEntry::Run { edit } => super::UndoEntry::Run {
                    branch: BranchId::ROOT,
                    edit,
                    removed: Vec::new(),
                    restores: Vec::new(),
                },
            }
        }
    }

    impl From<UndoHistory> for super::UndoHistory {
        fn from(history: UndoHistory) -> Self {
            let entries = |entries: Vec<UndoEntry>| entries.into_iter().map(Into::into).collect();
            super::UndoHistory {
                undo: entries(history.undo),
                redo: entries(history.redo),
            }
        }
    }
}

/// The undo history milestone 2a wrote: no version, edits of edit log version 1.
mod version_2a {
    use ludenea_app::edit::version_1;
    use serde::Deserialize;

    use super::version_2;

    #[derive(Deserialize)]
    enum UndoEntry {
        Scene { before: String, after: String },
        Run { edit: version_1::TimedEdit },
    }

    #[derive(Deserialize)]
    pub(super) struct UndoHistory {
        undo: Vec<UndoEntry>,
        redo: Vec<UndoEntry>,
    }

    impl From<UndoEntry> for version_2::UndoEntry {
        fn from(entry: UndoEntry) -> Self {
            match entry {
                UndoEntry::Scene { before, after } => version_2::UndoEntry::Scene { before, after },
                UndoEntry::Run { edit } => version_2::UndoEntry::Run { edit: edit.into() },
            }
        }
    }

    impl From<UndoHistory> for super::UndoHistory {
        fn from(history: UndoHistory) -> Self {
            let entries = |entries: Vec<UndoEntry>| {
                entries
                    .into_iter()
                    .map(|entry| version_2::UndoEntry::from(entry).into())
                    .collect()
            };
            super::UndoHistory {
                undo: entries(history.undo),
                redo: entries(history.redo),
            }
        }
    }
}
