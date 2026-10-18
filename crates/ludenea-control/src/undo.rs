//! The undo and redo history of an editor session.

use ludenea_app::TimedEdit;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::branches::{BranchId, BranchesError, StoredBranch};
use crate::session::SessionError;

/// The version `UndoHistory::encode` writes first. Milestone 2a wrote no version.
pub const UNDO_VERSION: u32 = 3;

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
    },
    /// An edit or take-over that forked `branch`, named `name` when last undone.
    Fork {
        branch: BranchId,
        cause: ForkCause,
        name: String,
        removed: Vec<u8>,
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
            UndoEntry::Scene { .. } => None,
            UndoEntry::Run { branch, .. } | UndoEntry::Fork { branch, .. } => Some(*branch),
        }
    }
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
        let other = |entry: &UndoEntry| {
            !matches!(entry, UndoEntry::Run { branch: b, edit: e, .. } if *b == branch && e == edit)
        };
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

    /// After the tree was reset to a single root holding the run of `path`'s last branch:
    /// the edits of `path` now belong to the root, and every other entry past tick 0 goes.
    /// The branches their undo removed are gone with the tree, so redo works within the
    /// root, as in 2a.
    pub fn keep_on_root(&mut self, path: &[BranchId]) {
        let keep = |entry: &mut UndoEntry| match entry {
            UndoEntry::Scene { .. } => true,
            UndoEntry::Run {
                branch, removed, ..
            } if path.contains(branch) => {
                *branch = BranchId::ROOT;
                removed.clear();
                true
            }
            _ => false,
        };
        self.undo.retain_mut(keep);
        self.redo.retain_mut(keep);
    }

    /// After a rebuild: the branches undo removed hold hashes of the previous build, so
    /// they resimulate when redo brings them back.
    pub fn mark_removed_stale(&mut self) -> Result<(), BranchesError> {
        for entry in self.undo.iter_mut().chain(&mut self.redo) {
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

    /// Decodes the current version, version 2, or the unversioned 2a form, the last two
    /// with every entry on the root branch (2a edit targets as `EntityRef::Runtime`). All
    /// start with a varint, so each versioned form is tried first and must use every byte.
    pub fn decode(bytes: &[u8]) -> Result<UndoHistory, SessionError> {
        if let Ok((UNDO_VERSION, history)) = decode_exact::<(u32, UndoHistory)>(bytes) {
            return Ok(history);
        }
        if let Ok((2, history)) = decode_exact::<(u32, version_2::UndoHistory)>(bytes) {
            return Ok(history.into());
        }
        match decode_exact::<version_2a::UndoHistory>(bytes) {
            Ok(history) => Ok(history.into()),
            // Not 2a either: a version number above ours names a newer editor.
            Err(error) => match postcard::take_from_bytes::<u32>(bytes) {
                Ok((version, _)) if version > UNDO_VERSION => {
                    Err(SessionError::UndoVersion { version })
                }
                _ => Err(error.into()),
            },
        }
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
