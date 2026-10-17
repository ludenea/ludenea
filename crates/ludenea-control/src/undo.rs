//! The undo and redo history of an editor session.

use ludenea_app::TimedEdit;
use serde::{Deserialize, Serialize};

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
        postcard::to_allocvec(self).expect("an undo history always encodes")
    }

    pub fn decode(bytes: &[u8]) -> Result<UndoHistory, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}
