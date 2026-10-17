//! The history of `world.edit` (docs/spec/server.md 3.1): edits of one `group` from one source are
//! one undo entry (a gizmo drag), which undoes and redoes whole; another group, an ungrouped edit or
//! an edit by another source in between starts a new entry; every entry names its source; an undo
//! of a `group` takes back only a whole drag; and the recording holds every edit of a group, so it
//! replays as without groups.
#![cfg(feature = "transpile")]
#![allow(clippy::disallowed_methods)]
mod common;

use pocket_link::Source;
use pocket_persist::replay::{
    MemorySink, Record, RecordOptions, RecordedWrite, Replay, ReplayMode, ReplayOutcome,
    VerifyOptions, verify,
};
use pocket_runtime::{Command, Game};
use serde_json::{Value, json};

struct Caller {
    source: Source,
    seq: u64,
}

impl Caller {
    fn new(source: Source) -> Caller {
        Caller { source, seq: 0 }
    }

    fn call(&mut self, g: &mut Game, name: &str, params: Value) -> Value {
        self.seq += 1;
        g.apply(&Command::new(self.source, self.seq, name, params))
            .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
    }

    /// A gizmo step: the Sloop's x as one `world.edit` of `group`.
    fn drag(&mut self, g: &mut Game, group: &str, x: f64) -> Value {
        self.call(
            g,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Sloop", "component": "Transform",
                                     "value": {"position.0": x}}}],
                   "label": "Move Sloop", "group": group}),
        )
    }
}

fn sloop_x(g: &mut Game) -> f64 {
    let v = g
        .apply(&Command::new(
            Source::Developer(9),
            1,
            "world.get",
            json!({"entity": "Sloop", "fields": ["Transform.position.0"]}),
        ))
        .unwrap();
    v["fields"]["Transform.position.0"].as_f64().unwrap()
}

fn history(g: &mut Game) -> Value {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "history.list",
        json!({}),
    ))
    .unwrap()
}

fn undo_labels(g: &mut Game) -> Vec<String> {
    history(g)["undo"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_drag_is_one_entry_that_undoes_and_redoes_whole() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let start = sloop_x(&mut g);
        let hash = g.world_hash().unwrap();
        let mut editor = Caller::new(Source::Editor);
        for i in 1..=20 {
            let r = editor.drag(&mut g, "drag-1", start + f64::from(i) * 0.25);
            assert_eq!(r["label"], json!("Move Sloop"), "{r}");
        }
        assert_eq!(sloop_x(&mut g), start + 5.0);
        let h = history(&mut g);
        assert_eq!(
            h["entries"]["undo"],
            json!([{"label": "Move Sloop", "source": "editor", "group": "drag-1"}]),
            "{h}"
        );
        // Undo takes the whole drag back, redo brings its end again.
        let u = editor.call(&mut g, "history.undo", json!({}));
        assert_eq!(u["label"], json!("Move Sloop"));
        assert_eq!(sloop_x(&mut g), start);
        assert_eq!(g.world_hash().unwrap(), hash);
        let h = history(&mut g);
        assert_eq!(h["undo"], json!([]), "{h}");
        assert_eq!(
            h["entries"]["redo"],
            json!([{"label": "Move Sloop", "source": "editor"}]),
            "{h}"
        );
        editor.call(&mut g, "history.redo", json!({}));
        assert_eq!(sloop_x(&mut g), start + 5.0);
        // Redone, the entry is a step of its own: the drag's group does not merge into it.
        editor.drag(&mut g, "drag-1", start + 6.0);
        assert_eq!(undo_labels(&mut g).len(), 2);
        g.run(3).unwrap();
        g.finish_recording().unwrap();
        // Every edit of the drag is recorded on its own (20, an undo, a redo and one more), and
        // the recording replays exactly.
        let replay = Replay::read(&sink.bytes()).unwrap();
        let writes: Vec<&RecordedWrite> = replay
            .records()
            .iter()
            .flat_map(|r| match r {
                Record::Tick { writes, .. } => writes.iter().collect(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(writes.len(), 23);
        assert!(writes.iter().all(|w| w.name == "world_edit"));
        assert!(writes.iter().all(|w| !w.params.contains("group")));
        let mut fresh = Game::for_replay(&replay).unwrap();
        let out = verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome;
        assert_eq!(out, ReplayOutcome::Identical);
    });
}

#[test]
fn another_group_an_ungrouped_edit_or_another_source_starts_an_entry() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let start = sloop_x(&mut g);
        let mut editor = Caller::new(Source::Editor);
        let mut agent = Caller::new(Source::Developer(2));
        editor.drag(&mut g, "drag-1", start + 1.0);
        editor.drag(&mut g, "drag-1", start + 2.0);
        // A second drag right after the first: its own entry.
        editor.drag(&mut g, "drag-2", start + 3.0);
        editor.drag(&mut g, "drag-2", start + 4.0);
        assert_eq!(undo_labels(&mut g).len(), 2);
        // An agent edits during a drag: the drag goes on as a new entry after the agent's.
        editor.drag(&mut g, "drag-3", start + 5.0);
        agent.call(
            &mut g,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Crate1", "component": "Cargo",
                                     "value": {"value": 9}}}]}),
        );
        editor.drag(&mut g, "drag-3", start + 6.0);
        editor.drag(&mut g, "drag-3", start + 7.0);
        // The agent's own edits with the editor's group do not merge into the editor's entry.
        agent.drag(&mut g, "drag-3", start + 8.0);
        // An ungrouped edit after a group is one entry.
        editor.call(
            &mut g,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Sloop", "component": "Transform",
                                     "value": {"position.0": start + 9.0}}}]}),
        );
        let h = history(&mut g);
        let sources: Vec<Value> = h["entries"]["undo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["source"].clone())
            .collect();
        assert_eq!(
            sources,
            [
                json!("editor"),
                json!("editor"),
                json!("editor"),
                json!({"developer": 2}),
                json!("editor"),
                json!({"developer": 2}),
                json!("editor"),
            ],
            "{h}"
        );
        assert!(h["entries"]["undo"][6].get("group").is_none(), "{h}");
        // Undoing walks back through them in order: the ungrouped edit, the agent's move, the
        // drag's second part (to where the agent's edit found it), the agent's cargo, the first part.
        let mut xs = Vec::new();
        for _ in 0..5 {
            editor.call(&mut g, "history.undo", json!({}));
            xs.push(sloop_x(&mut g) - start);
        }
        assert_eq!(xs, [8.0, 7.0, 5.0, 5.0, 4.0]);
        let cargo = g
            .apply(&Command::new(
                Source::Editor,
                99,
                "world.get",
                json!({"entity": "Crate1", "fields": ["Cargo.value"]}),
            ))
            .unwrap();
        assert_eq!(cargo["fields"]["Cargo.value"], json!(1), "{cargo}");
    });
}

/// `history.undo {group}` (an editor cancelling a drag): undone only when the next entry is the
/// whole of the group; refused with `history.group`, changing nothing, when another source's edit
/// split the drag or came after it.
#[test]
fn an_undo_of_a_group_takes_back_only_a_whole_drag() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let start = sloop_x(&mut g);
        let mut editor = Caller::new(Source::Editor);
        let mut agent = Caller::new(Source::Developer(2));
        let refused = |g: &mut Game, editor: &mut Caller, group: &str| {
            editor.seq += 1;
            let before = history(g);
            let e = g
                .apply(&Command::new(
                    editor.source,
                    editor.seq,
                    "history.undo",
                    json!({"group": group}),
                ))
                .unwrap_err();
            assert_eq!(e.code, "history.group", "{e:#?}");
            assert_eq!(history(g), before);
            e.detail["entries"].clone()
        };
        // Nothing of the group yet.
        assert_eq!(refused(&mut g, &mut editor, "drag-1"), json!(0));
        editor.drag(&mut g, "drag-1", start + 1.0);
        editor.drag(&mut g, "drag-1", start + 2.0);
        // An agent's edit after the drag: the drag is not the next to undo.
        let cargo = json!({"ops": [{"set": {"entity": "Crate1", "component": "Cargo",
                                              "value": {"value": 9}}}]});
        agent.call(&mut g, "world.edit", cargo.clone());
        assert_eq!(refused(&mut g, &mut editor, "drag-1"), json!(1));
        // The drag goes on after the agent's edit: two entries of it.
        editor.drag(&mut g, "drag-1", start + 3.0);
        assert_eq!(refused(&mut g, &mut editor, "drag-1"), json!(2));
        assert_eq!(sloop_x(&mut g), start + 3.0);
        // A drag no one split is taken back whole.
        editor.drag(&mut g, "drag-2", start + 4.0);
        editor.drag(&mut g, "drag-2", start + 5.0);
        let u = editor.call(&mut g, "history.undo", json!({"group": "drag-2"}));
        assert_eq!(u["label"], json!("Move Sloop"));
        assert_eq!(sloop_x(&mut g), start + 3.0);
        assert_eq!(undo_labels(&mut g).len(), 3);
    });
}
