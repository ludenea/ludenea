//! The undo and redo history of an editor session.

use ludenea_app::TimedEdit;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The version `UndoHistory::encode` writes first. Milestone 2a wrote no version.
pub const UNDO_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum UndoEntry {
    /// A tick 0 edit: the scene text before and after it.
    // ponytail: each entry holds two full scene texts (the next entry's `before` repeats
    // this `after`); store only `before` and derive `after` if large scenes make the
    // history too big.
    Scene { before: String, after: String },
    /// An edit recorded past tick 0.
    Run { edit: TimedEdit },
}

impl UndoEntry {
    fn is_after(&self, tick: u64) -> bool {
        matches!(self, UndoEntry::Run { edit } if edit.tick > tick)
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

    /// Drops the entries of edits recorded after `tick`, whose future was discarded.
    pub fn discard_after(&mut self, tick: u64) {
        self.undo.retain(|entry| !entry.is_after(tick));
        self.redo.retain(|entry| !entry.is_after(tick));
    }

    /// Drops the entries of an edit removed from the log.
    pub fn forget(&mut self, edit: &TimedEdit) {
        let other = |entry: &UndoEntry| !matches!(entry, UndoEntry::Run { edit: e } if e == edit);
        self.undo.retain(other);
        self.redo.retain(other);
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(&(UNDO_VERSION, self)).expect("an undo history always encodes")
    }

    /// Decodes the current version, or the unversioned 2a form with every edit target as
    /// `EntityRef::Runtime`. Both start with a varint, so the current version is tried
    /// first and must use every byte.
    pub fn decode(bytes: &[u8]) -> Result<UndoHistory, postcard::Error> {
        match decode_exact::<(u32, UndoHistory)>(bytes) {
            Ok((UNDO_VERSION, history)) => Ok(history),
            _ => decode_exact::<version_2a::UndoHistory>(bytes).map(Into::into),
        }
    }
}

fn decode_exact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, postcard::Error> {
    match postcard::take_from_bytes(bytes)? {
        (value, []) => Ok(value),
        _ => Err(postcard::Error::DeserializeBadEncoding),
    }
}

/// The undo history milestone 2a wrote: no version, edits of edit log version 1.
mod version_2a {
    use ludenea_app::edit::version_1;
    use serde::Deserialize;

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

    impl From<UndoEntry> for super::UndoEntry {
        fn from(entry: UndoEntry) -> Self {
            match entry {
                UndoEntry::Scene { before, after } => super::UndoEntry::Scene { before, after },
                UndoEntry::Run { edit } => super::UndoEntry::Run { edit: edit.into() },
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
