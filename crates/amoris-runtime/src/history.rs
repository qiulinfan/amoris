//! The edit history (docs/spec/server.md, history): every world edit, from the editor or an agent,
//! leaves its inverse here with its source; `history.undo` applies the last inverse as an edit and
//! keeps that edit's own inverse for `history.redo`. At most [`HISTORY_CAP`] entries; a restore
//! clears it, since the inverses name a world that is gone. The history is the editor's, not the
//! world's: it is never persisted or hashed, and a fork starts with none.
//!
//! Consecutive edits of one `group` from one source are one entry (a gizmo drag streams its edits
//! and undoes at once): [`merged`] folds each later inverse into the entry's. Only the entry: the
//! recorder still records every edit, so a replay is the same with or without groups.

use std::collections::{BTreeSet, VecDeque};

use amoris_link::Source;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::edit::Edit;
use crate::values::EntityRef;

/// Entries kept on each stack.
pub const HISTORY_CAP: usize = 500;

/// The most edits one `world_edit` takes; a merged inverse stays within it.
const MAX_EDITS: usize = 64;

/// One undoable step.
#[derive(Clone, Debug)]
pub struct Entry {
    pub label: String,
    /// Who made the edit (an undo or a redo keeps the edit's source).
    pub source: Source,
    /// The group whose edits merge into it; `None` for an ungrouped edit and once the entry has
    /// been undone or redone (a later edit of its group starts a new entry).
    pub group: Option<String>,
    /// The edits that undo it (or, on the redo stack, redo it).
    pub inverse: Vec<Edit>,
}

impl Entry {
    /// `{label, source, group?}`; `source` as amoris-link's `source_json` writes it.
    fn to_json(&self) -> Value {
        let mut v = json!({"label": self.label, "source": amoris_link::source_json(self.source)});
        if let Some(g) = &self.group {
            v["group"] = json!(g);
        }
        v
    }
}

/// `history.undo`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryUndoParams {
    /// Undo only when the next entry to undo is the whole of this group (it carries the group and
    /// no other undo entry does: a drag no other source's edit split); otherwise refused with
    /// `history.group`, which counts the group's entries. An editor cancelling a drag sends it.
    #[serde(default)]
    pub group: Option<String>,
}

/// The undo and redo stacks.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
}

/// The `(entity, component)` a set or remove of the inverse restores.
fn restores(e: &Edit) -> Option<(u64, &str)> {
    match e {
        Edit::Set {
            entity: EntityRef::Id(n),
            component,
            ..
        }
        | Edit::Remove {
            entity: EntityRef::Id(n),
            component,
        } => Some((*n, component.as_str())),
        _ => None,
    }
}

/// One inverse for two consecutive edits of a group: the later edit's inverse first, without the
/// components the earlier inverse restores anyway (it writes them whole, or removes them, after),
/// then the earlier inverse. A drag's later inverses all drop out, so the entry keeps the value
/// from before the drag. `None` when the later edit spawned, destroyed or revived an entity (its
/// inverse would destroy or revive one, and an edit cannot name an entity an earlier edit of the
/// same call revives) or when the result would pass the 64 edits of one `world_edit`: the edit
/// then starts an entry of its own.
pub fn merged(earlier: &[Edit], later: Vec<Edit>) -> Option<Vec<Edit>> {
    let covered: BTreeSet<(u64, &str)> = earlier.iter().filter_map(restores).collect();
    let mut out = Vec::new();
    for e in later {
        let pair = restores(&e)?;
        if !covered.contains(&pair) {
            out.push(e);
        }
    }
    if out.len() + earlier.len() > MAX_EDITS {
        return None;
    }
    out.extend(earlier.iter().cloned());
    Some(out)
}

impl History {
    /// A new edit by `source`: its inverse goes on the undo stack and the redo stack empties. An
    /// edit of `group` right after an edit of the same group by the same source merges into that
    /// entry (taking its label) when [`merged`] allows it; anything else starts a new entry.
    pub fn record(
        &mut self,
        label: String,
        source: Source,
        group: Option<String>,
        inverse: Vec<Edit>,
    ) {
        if inverse.is_empty() {
            return;
        }
        self.redo.clear();
        if let Some(g) = &group
            && let Some(top) = self.undo.back_mut()
            && top.source == source
            && top.group.as_ref() == Some(g)
            && let Some(inv) = merged(&top.inverse, inverse.clone())
        {
            top.inverse = inv;
            top.label = label;
            return;
        }
        self.push_undo(Entry {
            label,
            source,
            group,
            inverse,
        });
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

    /// How many undo entries carry `group`, and whether the next to undo is one of them.
    pub(crate) fn group_on_top(&self, group: &str) -> (usize, bool) {
        let of = |e: &Entry| e.group.as_deref() == Some(group);
        let n = self.undo.iter().filter(|e| of(e)).count();
        (n, self.undo.back().is_some_and(of))
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// `{undo: [label], redo: [label], entries: {undo: [entry], redo: [entry]}}`: each stack with
    /// the next entry to take last (most recent last, as host-protocol.md Draft 1 has it); `entries`
    /// lists the same entries in the same order as `{label, source, group?}`.
    pub fn to_json(&self) -> Value {
        let undo: Vec<&str> = self.undo.iter().map(|e| e.label.as_str()).collect();
        let redo: Vec<&str> = self.redo.iter().map(|e| e.label.as_str()).collect();
        let undo_entries: Vec<Value> = self.undo.iter().map(Entry::to_json).collect();
        let redo_entries: Vec<Value> = self.redo.iter().map(Entry::to_json).collect();
        json!({"undo": undo, "redo": redo,
               "entries": {"undo": undo_entries, "redo": redo_entries}})
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    fn set(id: u64, component: &str, x: f64) -> Edit {
        let mut value = Map::new();
        value.insert("position".into(), json!([x, 0.0, 0.0]));
        Edit::Set {
            entity: EntityRef::Id(id),
            component: component.into(),
            value,
            replace: true,
        }
    }

    fn remove(id: u64, component: &str) -> Edit {
        Edit::Remove {
            entity: EntityRef::Id(id),
            component: component.into(),
        }
    }

    fn destroy(id: u64) -> Edit {
        Edit::Destroy {
            entity: EntityRef::Id(id),
        }
    }

    fn labels(h: &History) -> Value {
        let v = h.to_json();
        json!([v["undo"], v["redo"]])
    }

    /// A drag: every later inverse names the component the first restores, so the entry keeps
    /// the first inverse (the value from before the drag) and the last label.
    #[test]
    fn a_group_merges_into_one_entry_with_the_first_inverse() {
        let mut h = History::default();
        let g = Some("drag-1".to_owned());
        h.record(
            "move 1".into(),
            Source::Editor,
            g.clone(),
            vec![set(3, "Transform", 0.0)],
        );
        h.record(
            "move 2".into(),
            Source::Editor,
            g.clone(),
            vec![set(3, "Transform", 1.0)],
        );
        h.record(
            "move 3".into(),
            Source::Editor,
            g,
            vec![set(3, "Transform", 2.0)],
        );
        assert_eq!(labels(&h), json!([["move 3"], []]));
        let top = h.pop_undo().unwrap();
        assert_eq!(
            format!("{:?}", top.inverse),
            format!("{:?}", vec![set(3, "Transform", 0.0)])
        );
    }

    /// A component first touched later in the group is restored too, before the earlier inverse.
    #[test]
    fn a_component_first_touched_later_joins_the_inverse() {
        let earlier = vec![destroy(9), set(3, "Transform", 0.0)];
        let later = vec![
            set(3, "Transform", 1.0),
            remove(4, "Boat"),
            set(9, "Model", 0.5),
        ];
        let m = merged(&earlier, later).unwrap();
        let want = vec![
            remove(4, "Boat"),
            set(9, "Model", 0.5),
            destroy(9),
            set(3, "Transform", 0.0),
        ];
        assert_eq!(format!("{m:?}"), format!("{want:?}"));
    }

    /// A different group, an ungrouped edit, another source, an edit that spawns or destroys, an
    /// undo or a redo in between: each starts a new entry.
    #[test]
    fn what_breaks_a_merge() {
        let g = |s: &str| Some(s.to_owned());
        let mut h = History::default();
        h.record("a".into(), Source::Editor, g("one"), vec![set(1, "T", 0.0)]);
        h.record("b".into(), Source::Editor, g("two"), vec![set(1, "T", 1.0)]);
        h.record("c".into(), Source::Editor, None, vec![set(1, "T", 2.0)]);
        h.record("d".into(), Source::Editor, None, vec![set(1, "T", 3.0)]);
        h.record("e".into(), Source::Editor, g("two"), vec![set(1, "T", 4.0)]);
        h.record(
            "f".into(),
            Source::Developer(0),
            g("two"),
            vec![set(1, "T", 5.0)],
        );
        h.record("g".into(), Source::Editor, g("two"), vec![set(1, "T", 6.0)]);
        h.record("h".into(), Source::Editor, g("two"), vec![destroy(7)]);
        assert_eq!(
            labels(&h),
            json!([["a", "b", "c", "d", "e", "f", "g", "h"], []])
        );
        // An entry undone and redone keeps its source and loses its group.
        let mut e = h.pop_undo().unwrap();
        e.group = None;
        h.push_redo(e);
        let mut e = h.pop_redo().unwrap();
        assert_eq!(e.source, Source::Editor);
        e.group = None;
        h.push_undo(e);
        h.record("i".into(), Source::Editor, g("two"), vec![set(1, "T", 7.0)]);
        assert_eq!(h.to_json()["undo"].as_array().unwrap().len(), 9);
        assert_eq!(
            h.to_json()["entries"]["undo"][5]["source"],
            json!({"developer": 0})
        );
        assert_eq!(h.to_json()["entries"]["undo"][8]["group"], json!("two"));
        assert!(h.to_json()["entries"]["undo"][7].get("group").is_none());
    }

    /// Past 64 edits the merged inverse would not apply as one call: a new entry starts.
    #[test]
    fn a_merge_stays_within_one_world_edit() {
        let mut h = History::default();
        let g = Some("wide".to_owned());
        h.record(
            "a".into(),
            Source::Editor,
            g.clone(),
            (0..40).map(|i| set(i, "T", 0.0)).collect(),
        );
        h.record(
            "b".into(),
            Source::Editor,
            g.clone(),
            (40..64).map(|i| set(i, "T", 0.0)).collect(),
        );
        assert_eq!(h.to_json()["undo"].as_array().unwrap().len(), 1);
        h.record("c".into(), Source::Editor, g, vec![set(64, "T", 0.0)]);
        assert_eq!(h.to_json()["undo"].as_array().unwrap().len(), 2);
    }

    /// Both stacks list the next entry last, `entries` in the same order as the labels.
    #[test]
    fn each_stack_lists_the_next_last() {
        let mut h = History::default();
        for l in ["a", "b", "c"] {
            h.record(l.into(), Source::Editor, None, vec![set(1, "T", 0.0)]);
        }
        for _ in 0..2 {
            let e = h.pop_undo().unwrap();
            h.push_redo(e);
        }
        assert_eq!(labels(&h), json!([["a"], ["c", "b"]]));
        assert_eq!(h.to_json()["entries"]["redo"][1]["label"], json!("b"));
    }

    /// Whether the next entry to undo is the whole of a group: split by another source's edit,
    /// the group has two entries.
    #[test]
    fn a_group_on_top() {
        let g = |s: &str| Some(s.to_owned());
        let mut h = History::default();
        assert_eq!(h.group_on_top("d"), (0, false));
        h.record("a".into(), Source::Editor, g("d"), vec![set(1, "T", 0.0)]);
        h.record("b".into(), Source::Editor, g("d"), vec![set(1, "T", 1.0)]);
        assert_eq!(h.group_on_top("d"), (1, true));
        h.record(
            "c".into(),
            Source::Developer(1),
            None,
            vec![set(2, "T", 0.0)],
        );
        assert_eq!(h.group_on_top("d"), (1, false));
        h.record("d".into(), Source::Editor, g("d"), vec![set(1, "T", 2.0)]);
        assert_eq!(h.group_on_top("d"), (2, true));
    }
}
