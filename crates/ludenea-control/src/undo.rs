//! The undo and redo history of an editor session.

use ludenea_app::TimedEdit;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum UndoEntry {
    /// A tick 0 edit: the scene text before and after it.
    Scene { before: String, after: String },
    /// An edit recorded past tick 0.
    Run { edit: TimedEdit },
}

impl UndoEntry {
    fn is_after(&self, tick: u64) -> bool {
        matches!(self, UndoEntry::Run { edit } if edit.tick > tick)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct UndoHistory {
    pub undo: Vec<UndoEntry>,
    pub redo: Vec<UndoEntry>,
}

impl UndoHistory {
    /// A new edit: pushed on the undo stack, and the redo stack is cleared.
    pub fn record(&mut self, entry: UndoEntry) {
        self.undo.push(entry);
        self.redo.clear();
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
