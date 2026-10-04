//! The edit history (docs/spec/server.md, history): every world edit, from the editor or an agent,
//! leaves its inverse here; `history.undo` applies the last inverse as an edit and keeps that
//! edit's own inverse for `history.redo`. At most [`HISTORY_CAP`] entries; a restore clears it,
//! since the inverses name a world that is gone. The history is the editor's, not the world's: it
//! is never persisted or hashed, and a fork starts with none.

use std::collections::VecDeque;

use serde_json::{Value, json};

use crate::edit::Edit;

/// Entries kept on each stack.
pub const HISTORY_CAP: usize = 500;

/// One undoable step.
#[derive(Clone, Debug)]
pub struct Entry {
    pub label: String,
    /// The edits that undo it (or, on the redo stack, redo it).
    pub inverse: Vec<Edit>,
}

/// The undo and redo stacks.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
}

impl History {
    /// A new edit: its inverse goes on the undo stack and the redo stack empties.
    pub fn record(&mut self, label: String, inverse: Vec<Edit>) {
        if inverse.is_empty() {
            return;
        }
        self.redo.clear();
        self.push_undo(Entry { label, inverse });
    }

    pub(crate) fn push_undo(&mut self, e: Entry) {
        if self.undo.len() == HISTORY_CAP {
            self.undo.pop_front();
        }
        self.undo.push_back(e);
    }

    pub(crate) fn push_redo(&mut self, e: Entry) {
        self.redo.push(e);
    }

    pub(crate) fn pop_undo(&mut self) -> Option<Entry> {
        self.undo.pop_back()
    }

    pub(crate) fn pop_redo(&mut self) -> Option<Entry> {
        self.redo.pop()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// `{undo: [label], redo: [label]}`, most recent last.
    pub fn to_json(&self) -> Value {
        let undo: Vec<&str> = self.undo.iter().map(|e| e.label.as_str()).collect();
        let redo: Vec<&str> = self.redo.iter().map(|e| e.label.as_str()).collect();
        json!({"undo": undo, "redo": redo})
    }
}
