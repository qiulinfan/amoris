//! The hierarchy end to end (docs/spec/simulation.md 7.3 and 7.4; server.md 3 and 3.1;
//! script-host.md 5.4): `Parent` set by scenes, `world.edit` and scripts; `world.tree` nested by it;
//! a despawn that takes everything under the entity by every route, with an undo that brings it
//! all back under its ids; a `Parent` naming itself, an entity under it or nothing alive refused
//! where the world is in view and harmless elsewhere; and the same hash chains through a replay, a
//! fork, a restore and a shuffled storage.
#![cfg(feature = "transpile")]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amoris_contract::Problem;
use amoris_link::{Kind, Source};
use amoris_persist::replay::{
    MemorySink, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use amoris_runtime::{Command, Game, GameBuilder, GameSetup, Project, Scene};
use amoris_sim::{EntityId, Parent, StepReport};
use serde_json::{Value, json};

/// A project component with an entity field and a string, and no system.
const TAG: &str = r#"
import { component, field, game } from "amoris";

export const Tag = component("Tag", {
    version: 1, doc: "A note on an entity.",
    fields: {
        on: field.entity("An entity it names."),
        note: field.str("", "Its text."),
    },
});

export default game({ components: [Tag], systems: [] });
"#;

/// A temporary project of one script and a scene.
fn project(name: &str, main: &str, scene: &Value) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-hierarchy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = 'hierarchy-{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), main).unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": scene});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

fn load(root: &Path) -> Result<Arc<GameSetup>, Problem> {
    Ok(Arc::new(Project::load(root)?.setup(false)?))
}

/// The game of a project made for the test, with the test's own commands: `test.parent {child,
/// parent}` inserts a `Parent` with no check (what a snapshot made by hand could hold),
/// `test.scene` spawns a scene into the world as it is, and `test.shuffle {seed}` permutes the
/// storage.
fn build(root: &Path) -> Result<Game, Problem> {
    let id = |v: &Value| v.as_u64().and_then(EntityId::new).expect("an entity id");
    GameBuilder::new(load(root)?)
        .project(root)
        .command(
            "test.parent",
            Kind::Write,
            Arc::new(move |b, p| {
                let e = b.entity(id(&p["child"])).expect("a live child");
                let parent = id(&p["parent"]);
                b.world_mut().entity_mut(e).insert(Parent { parent });
                Ok((json!({}), p.clone()))
            }),
        )
        .command(
            "test.scene",
            Kind::Write,
            Arc::new(|b, p| {
                let scene: Scene = serde_json::from_value(p.clone()).expect("a scene");
                let ids = scene.spawn_into(b)?;
                Ok((json!({"spawned": ids.len()}), p.clone()))
            }),
        )
        .command(
            "test.shuffle",
            Kind::Read,
            Arc::new(|b, p| {
                amoris_sim::order::shuffle_storage(b.world_mut(), p["seed"].as_u64().unwrap_or(0));
                Ok((json!({}), json!({})))
            }),
        )
        .build()
}

fn game(name: &str, main: &str, scene: &Value) -> Game {
    build(&project(name, main, scene)).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn try_call(g: &mut Game, name: &str, params: Value) -> Result<Value, Problem> {
    g.apply(&Command::new(Source::Developer(9), 1, name, params))
}

fn call(g: &mut Game, name: &str, params: Value) -> Value {
    try_call(g, name, params).unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// One `world.edit` of `ops`.
fn edit(g: &mut Game, ops: Value) -> Value {
    call(g, "world.edit", json!({ "ops": ops }))
}

/// A `world.edit` that must be refused: its problem, and the world as it was.
fn refused(g: &mut Game, ops: Value) -> Problem {
    let before = g.world_hash().unwrap();
    let shown = ops.to_string();
    let e = try_call(g, "world.edit", json!({ "ops": ops })).expect_err(&shown);
    assert_eq!(g.world_hash().unwrap(), before, "{shown} changed the world");
    e
}

/// The id of the entity named `name`.
fn id(g: &Game, name: &str) -> u64 {
    let named = amoris_runtime::named(g.world());
    let found = named.iter().find(|(_, n)| n == name);
    found.unwrap_or_else(|| panic!("no {name}")).0.get()
}

/// A whole component as JSON, or null.
fn component(g: &mut Game, entity: impl Into<Value>, name: &str) -> Value {
    let r = call(
        g,
        "world.get",
        json!({"entity": entity.into(), "components": [name]}),
    );
    r["components"][name].clone()
}

/// A tree as text: `Rig(Arm(Hand) Leg) Loose`, an unnamed entity as `#7`, a node the depth cut
/// with its `more` (`Arm+1`).
fn shape(nodes: &Value) -> String {
    let node = |n: &Value| {
        let mut s = match n["name"].as_str() {
            Some(name) => name.to_owned(),
            None => format!("#{}", n["id"]),
        };
        if let Some(more) = n.get("more") {
            s.push_str(&format!("+{more}"));
        }
        match shape(&n["children"]).as_str() {
            "" => s,
            under => format!("{s}({under})"),
        }
    };
    let nodes = nodes.as_array().expect("a list of nodes");
    nodes.iter().map(node).collect::<Vec<_>>().join(" ")
}

fn tree(g: &mut Game, params: Value) -> String {
    shape(&call(g, "world.tree", params))
}

fn whole(g: &mut Game) -> String {
    tree(g, json!({}))
}

fn entities(g: &mut Game) -> u64 {
    call(g, "status", json!({}))["entities"].as_u64().unwrap()
}

fn undo_labels(g: &mut Game) -> Value {
    call(g, "history.list", json!({}))["undo"].clone()
}

/// Rig > (Arm > Hand, Leg), and Loose > Crate; ids 1 to 6 in that order. Two tags name entities
/// of their own subtree.
fn family() -> Value {
    json!([
        {"name": "Rig"},
        {"name": "Arm", "components": {"Parent": {"parent": 1}}},
        {"name": "Hand", "components": {"Parent": {"parent": 2},
                                        "Tag": {"on": 1, "note": "grip"}}},
        {"name": "Leg", "components": {"Parent": {"parent": 1}}},
        {"name": "Loose"},
        {"name": "Crate", "components": {"Parent": {"parent": 5}, "Tag": {"on": 5}}},
    ])
}

const FAMILY: &str = "Rig(Arm(Hand) Leg) Loose(Crate)";

fn set_parent(entity: &str, parent: u64) -> Value {
    json!({"set": {"entity": entity, "component": "Parent", "value": {"parent": parent}}})
}

fn destroy(entity: &str) -> Value {
    json!({"destroy": {"entity": entity}})
}

#[test]
fn world_edit_sets_and_removes_a_parent() {
    common::big_stack(|| {
        let mut g = game("edit", TAG, &family());
        assert_eq!(whole(&mut g), FAMILY);
        let start = g.world_hash().unwrap();
        // The registry shows the component with its one field.
        let schema = call(&mut g, "world.schema", json!({"component": "Parent"}));
        assert_eq!(schema["origin"], json!("Engine"), "{schema}");
        assert_eq!(schema["schema"]["required"], json!(["parent"]));
        assert_eq!(component(&mut g, "Hand", "Parent"), json!({"parent": 2}));

        edit(&mut g, json!([set_parent("Hand", 4)]));
        assert_eq!(whole(&mut g), "Rig(Arm Leg(Hand)) Loose(Crate)");
        assert_eq!(component(&mut g, "Hand", "Parent"), json!({"parent": 4}));
        edit(
            &mut g,
            json!([{"remove": {"entity": "Hand", "component": "Parent"}}]),
        );
        assert_eq!(whole(&mut g), "Rig(Arm Leg) Hand Loose(Crate)");
        assert_eq!(component(&mut g, "Hand", "Parent"), Value::Null);
        // A set adds the component to an entity that had none, with everything under it.
        edit(&mut g, json!([set_parent("Loose", 3)]));
        assert_eq!(whole(&mut g), "Rig(Arm Leg) Hand(Loose(Crate))");

        // Each edit is one history entry, undone to the world it started from and redone.
        assert_eq!(undo_labels(&mut g).as_array().unwrap().len(), 3);
        let end = g.world_hash().unwrap();
        for _ in 0..3 {
            call(&mut g, "history.undo", json!({}));
        }
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(g.world_hash().unwrap(), start);
        for _ in 0..3 {
            call(&mut g, "history.redo", json!({}));
        }
        assert_eq!(g.world_hash().unwrap(), end);

        // A spawn comes under its parent; its undo destroys it (the id stays spent).
        let r = edit(
            &mut g,
            json!([{"spawn": {"name": "Nail", "components": {"Parent": {"parent": 6}}}}]),
        );
        assert_eq!(r["spawned"], json!([7]));
        assert_eq!(whole(&mut g), "Rig(Arm Leg) Hand(Loose(Crate(Nail)))");
        call(&mut g, "history.undo", json!({}));
        assert_eq!(whole(&mut g), "Rig(Arm Leg) Hand(Loose(Crate))");
        call(&mut g, "history.redo", json!({}));
        assert_eq!(whole(&mut g), "Rig(Arm Leg) Hand(Loose(Crate(Nail)))");
        assert_eq!(id(&g, "Nail"), 7, "revived under its id");
    });
}

#[test]
fn a_parent_that_is_not_alive_or_closes_a_loop_is_refused_whole() {
    common::big_stack(|| {
        let mut g = game("refuse", TAG, &family());
        let cycle = |e: &Problem, path: &str, chain: Value| {
            assert_eq!(e.code, "sim.parent_cycle", "{e:#?}");
            assert_eq!(e.detail["path"], json!(path), "{e:#?}");
            assert_eq!(e.detail["chain"], chain, "{e:#?}");
        };
        // Itself.
        let e = refused(&mut g, json!([set_parent("Arm", 2)]));
        cycle(&e, "/ops/0/set/value/parent", json!([2]));
        assert_eq!(e.message, "Entity 2 cannot be its own parent.");
        // An entity under it, directly and two levels down.
        let e = refused(&mut g, json!([set_parent("Arm", 3)]));
        cycle(&e, "/ops/0/set/value/parent", json!([3, 2]));
        let e = refused(&mut g, json!([set_parent("Rig", 3)]));
        cycle(&e, "/ops/0/set/value/parent", json!([3, 2, 1]));
        assert_eq!(
            e.message,
            "Entity 1 cannot go under entity 3: 3 is under 1 (3 -> 2 -> 1). Nothing was applied."
        );
        assert_eq!(e.detail["entity"], json!(1));
        assert_eq!(e.detail["parent"], json!(3));
        // The loop a call's own edits leave, judged at its end: the second set closes it.
        let e = refused(
            &mut g,
            json!([
                {"remove": {"entity": "Arm", "component": "Parent"}},
                set_parent("Rig", 3),
                set_parent("Arm", 4)
            ]),
        );
        cycle(&e, "/ops/1/set/value/parent", json!([3, 2, 4, 1]));
        // An id never given out, in a set and in a spawn (the id the spawn itself will get).
        let e = refused(&mut g, json!([set_parent("Hand", 99)]));
        assert_eq!(e.code, "sim.entity_id_invalid", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/0/set/value/parent"));
        let spawn = |parent: u64| json!({"spawn": {"components": {"Parent": {"parent": parent}}}});
        let e = refused(&mut g, json!([spawn(7)]));
        assert_eq!(e.code, "sim.entity_id_invalid", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("/ops/0/spawn/components/Parent/parent")
        );
        // No parent at all: the component always names one.
        let e = refused(
            &mut g,
            json!([{"set": {"entity": "Hand", "component": "Parent", "value": {"parent": null}}}]),
        );
        assert_ne!(e.code, "sim.internal", "{e:#?}");
        let e = refused(
            &mut g,
            json!([{"set": {"entity": "Loose", "component": "Parent", "value": {}}}]),
        );
        assert_eq!(e.code, "request.missing_field", "{e:#?}");
        // A parent an earlier op of the call destroys, itself or with the entity over it.
        let e = refused(&mut g, json!([destroy("Leg"), set_parent("Hand", 4)]));
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/set/value/parent"));
        assert_eq!(e.detail["destroyed_by"], json!(0));
        let e = refused(&mut g, json!([destroy("Rig"), set_parent("Crate", 3)]));
        assert_eq!(e.detail["destroyed_by"], json!(0), "{e:#?}");
        let e = refused(&mut g, json!([destroy("Leg"), spawn(4)]));
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("/ops/1/spawn/components/Parent/parent")
        );
        assert_eq!(e.detail["destroyed_by"], json!(0));
        // An entity that is gone.
        edit(&mut g, json!([destroy("Leg")]));
        let e = refused(&mut g, json!([set_parent("Hand", 4)]));
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/0/set/value/parent"));
        let e = refused(&mut g, json!([spawn(4)]));
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(whole(&mut g), "Rig(Arm(Hand)) Loose(Crate)");
    });
}

/// Swapping a parent with its child takes two sets, and the hierarchy between them holds a loop:
/// a call may pass through one, and so may the undo, whose restores go in id order.
#[test]
fn a_call_may_pass_through_a_loop_but_not_leave_one() {
    common::big_stack(|| {
        // Top > Mid > Low, the child before its parent in the file.
        let scene = json!([
            {"name": "Low", "components": {"Parent": {"parent": 2}}},
            {"name": "Mid", "components": {"Parent": {"parent": 3}}},
            {"name": "Top"},
        ]);
        let mut g = game("swap", TAG, &scene);
        assert_eq!(whole(&mut g), "Top(Mid(Low))");
        let start = g.world_hash().unwrap();
        // Mid under Low while Low is still under Mid, then Low under Top.
        edit(&mut g, json!([set_parent("Mid", 1), set_parent("Low", 3)]));
        assert_eq!(whole(&mut g), "Top(Low(Mid))");
        let swapped = g.world_hash().unwrap();
        // The undo writes Low back under Mid first, while Mid is still under Low.
        call(&mut g, "history.undo", json!({}));
        assert_eq!(whole(&mut g), "Top(Mid(Low))");
        assert_eq!(g.world_hash().unwrap(), start);
        call(&mut g, "history.redo", json!({}));
        assert_eq!(g.world_hash().unwrap(), swapped);
        call(&mut g, "history.undo", json!({}));
        assert_eq!(g.world_hash().unwrap(), start);
        // The first set alone leaves the loop.
        let e = refused(&mut g, json!([set_parent("Mid", 1)]));
        assert_eq!(e.code, "sim.parent_cycle", "{e:#?}");
        assert_eq!(e.detail["chain"], json!([1, 2]));
        assert_eq!(e.detail["path"], json!("/ops/0/set/value/parent"));
    });
}

#[test]
fn destroying_a_parent_takes_what_is_under_it_and_an_undo_brings_it_all_back() {
    common::big_stack(|| {
        let mut g = game("destroy", TAG, &family());
        let start = g.world_hash().unwrap();
        let hand = component(&mut g, "Hand", "Tag");
        assert_eq!(hand, json!({"on": 1, "note": "grip"}));

        let r = edit(&mut g, json!([destroy("Rig")]));
        assert_eq!(
            r["results"],
            json!([{"op": "destroyed", "count": 4}]),
            "{r}"
        );
        assert_eq!(whole(&mut g), "Loose(Crate)");
        assert_eq!(entities(&mut g), 2);
        let gone = g.world_hash().unwrap();
        assert_eq!(undo_labels(&mut g), json!(["destroy Rig"]));

        // One undo: every entity is back under its id, with its name, its components (a tag
        // naming an entity the same undo revives included) and its parent.
        let u = call(&mut g, "history.undo", json!({}));
        assert_eq!(
            u["results"],
            json!([{"op": "revived", "id": 1, "count": 4}]),
            "{u}"
        );
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(g.world_hash().unwrap(), start);
        assert_eq!(id(&g, "Hand"), 3);
        assert_eq!(component(&mut g, "Hand", "Tag"), hand);
        assert_eq!(component(&mut g, "Leg", "Parent"), json!({"parent": 1}));
        let r = call(&mut g, "history.redo", json!({}));
        assert_eq!(
            r["results"],
            json!([{"op": "destroyed", "count": 4}]),
            "{r}"
        );
        assert_eq!(g.world_hash().unwrap(), gone);
        call(&mut g, "history.undo", json!({}));
        assert_eq!(g.world_hash().unwrap(), start);
        let h = call(&mut g, "history.list", json!({}));
        assert_eq!(h["undo"], json!([]), "{h}");
        assert_eq!(h["redo"], json!(["destroy Rig"]), "{h}");

        // After the destroy, the call cannot name what went with the entity.
        let e = refused(
            &mut g,
            json!([destroy("Rig"),
                   {"set": {"entity": "Hand", "component": "Tag", "value": {"note": "x"}}}]),
        );
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["destroyed_by"], json!(0));
        assert_eq!(e.detail["path"], json!("/ops/1/set/entity"));
        let e = refused(&mut g, json!([destroy("Rig"), destroy("Hand")]));
        assert_eq!(e.detail["destroyed_by"], json!(0), "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/destroy/entity"));
        // A tag naming an entity the destroy took: refused like one naming the entity itself.
        let e = refused(
            &mut g,
            json!([destroy("Rig"),
                   {"set": {"entity": "Crate", "component": "Tag", "value": {"on": 3}}}]),
        );
        assert_eq!(e.detail["destroyed_by"], json!(0), "{e:#?}");

        // The editor's order, children first: each destroy takes one entity, and the one undo
        // revives both.
        let r = edit(&mut g, json!([destroy("Crate"), destroy("Loose")]));
        let counts = json!([{"op": "destroyed", "count": 1}, {"op": "destroyed", "count": 1}]);
        assert_eq!(r["results"], counts, "{r}");
        assert_eq!(whole(&mut g), "Rig(Arm(Hand) Leg)");
        let two = g.world_hash().unwrap();
        let u = call(&mut g, "history.undo", json!({}));
        assert_eq!(u["applied"], json!(1), "{u}");
        assert_eq!(g.world_hash().unwrap(), start);
        call(&mut g, "history.redo", json!({}));
        assert_eq!(g.world_hash().unwrap(), two);
        call(&mut g, "history.undo", json!({}));
        assert_eq!(g.world_hash().unwrap(), start);
    });
}

/// Renames and the hierarchy in one call (server.md 3.1, Names): a later op finds an entity by
/// the name an earlier one gave it, a rename does not save an entity a destroy takes, and the
/// undo puts back the names, the parents and the entities whole.
#[test]
fn renames_and_a_destroy_in_one_call_undo_and_redo() {
    common::big_stack(|| {
        let mut g = game("renames", TAG, &family());
        let start = g.world_hash().unwrap();
        let rename = |entity: &str, name: &str| json!({"set": {"entity": entity, "component": "Name", "value": name}});
        // Renamed, then taken by its grandparent's destroy: its new name names a destroyed entity.
        let e = refused(
            &mut g,
            json!([rename("Hand", "Palm"), destroy("Rig"),
                   {"set": {"entity": "Palm", "component": "Tag", "value": {"note": "x"}}}]),
        );
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["destroyed_by"], json!(1), "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/2/set/entity"), "{e:#?}");
        // The old name names nothing once the rename is checked.
        let e = refused(
            &mut g,
            json!([rename("Hand", "Palm"), set_parent("Hand", 5)]),
        );
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/set/entity"), "{e:#?}");

        // Renamed, moved out by its new name, and so kept when Rig goes.
        let r = edit(
            &mut g,
            json!([
                rename("Hand", "Palm"),
                set_parent("Palm", 5),
                destroy("Rig")
            ]),
        );
        assert_eq!(
            r["results"][2],
            json!({"op": "destroyed", "count": 3}),
            "{r}"
        );
        assert_eq!(whole(&mut g), "Loose(Palm Crate)");
        assert_eq!(id(&g, "Palm"), 3);
        let after = g.world_hash().unwrap();
        call(&mut g, "history.undo", json!({}));
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(g.world_hash().unwrap(), start);
        call(&mut g, "history.redo", json!({}));
        assert_eq!(whole(&mut g), "Loose(Palm Crate)");
        assert_eq!(g.world_hash().unwrap(), after);
    });
}

/// A destroy with a reparent in the same call: an entity moved out from under the destroyed one
/// stays, one moved under it goes, and each call undoes and redoes whole.
#[test]
fn a_reparent_and_a_destroy_in_one_call_undo_and_redo() {
    common::big_stack(|| {
        let mut g = game("mixed", TAG, &family());
        let start = g.world_hash().unwrap();
        // Hand leaves Rig's subtree before Rig is destroyed.
        let r = edit(&mut g, json!([set_parent("Hand", 5), destroy("Rig")]));
        assert_eq!(
            r["results"][1],
            json!({"op": "destroyed", "count": 3}),
            "{r}"
        );
        assert_eq!(whole(&mut g), "Loose(Hand Crate)");
        let moved = g.world_hash().unwrap();
        call(&mut g, "history.undo", json!({}));
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(g.world_hash().unwrap(), start);
        // The redo sets Hand's parent before it destroys Rig, or the destroy would take Hand.
        call(&mut g, "history.redo", json!({}));
        assert_eq!(whole(&mut g), "Loose(Hand Crate)");
        assert_eq!(g.world_hash().unwrap(), moved);
        call(&mut g, "history.undo", json!({}));
        assert_eq!(g.world_hash().unwrap(), start);

        // Crate comes under Rig and goes with it.
        let r = edit(&mut g, json!([set_parent("Crate", 1), destroy("Rig")]));
        assert_eq!(
            r["results"][1],
            json!({"op": "destroyed", "count": 5}),
            "{r}"
        );
        assert_eq!(whole(&mut g), "Loose");
        let alone = g.world_hash().unwrap();
        call(&mut g, "history.undo", json!({}));
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(g.world_hash().unwrap(), start);
        call(&mut g, "history.redo", json!({}));
        assert_eq!(g.world_hash().unwrap(), alone);
        call(&mut g, "history.undo", json!({}));
        assert_eq!(g.world_hash().unwrap(), start);

        // An entity destroyed under one that then leaves the subtree, which is destroyed too:
        // the redo destroys both, though the first is no longer under the second by then.
        let r = edit(
            &mut g,
            json!([destroy("Hand"), set_parent("Arm", 5), destroy("Rig")]),
        );
        assert_eq!(
            r["results"][2],
            json!({"op": "destroyed", "count": 2}),
            "{r}"
        );
        assert_eq!(whole(&mut g), "Loose(Arm Crate)");
        let pruned = g.world_hash().unwrap();
        call(&mut g, "history.undo", json!({}));
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(g.world_hash().unwrap(), start);
        let r = call(&mut g, "history.redo", json!({}));
        assert_eq!(whole(&mut g), "Loose(Arm Crate)", "{r}");
        assert_eq!(g.world_hash().unwrap(), pruned);
        call(&mut g, "history.undo", json!({}));
        assert_eq!(g.world_hash().unwrap(), start);

        // A child spawned under an entity and the entity destroyed, in one call: the child goes
        // with it, and the undo brings back what was there before the call (the child's id stays
        // spent, so the world is the first one but for the allocator).
        let r = edit(
            &mut g,
            json!([{"spawn": {"name": "Nail", "components": {"Parent": {"parent": 5}}}},
                   destroy("Loose")]),
        );
        assert_eq!(r["spawned"], json!([7]), "{r}");
        assert_eq!(
            r["results"][1],
            json!({"op": "destroyed", "count": 3}),
            "{r}"
        );
        assert_eq!(whole(&mut g), "Rig(Arm(Hand) Leg)");
        let u = call(&mut g, "history.undo", json!({}));
        assert_eq!(
            u["results"],
            json!([{"op": "revived", "id": 5, "count": 2}]),
            "{u}"
        );
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(
            component(&mut g, "Crate", "Tag"),
            json!({"on": 5, "note": ""})
        );
        assert_eq!(undo_labels(&mut g), json!([]));
        call(&mut g, "history.redo", json!({}));
        assert_eq!(whole(&mut g), "Rig(Arm(Hand) Leg)");
    });
}

/// A subtree of 100 entities, three levels of 33 under one root, each naming its parent in a tag
/// too: one destroy, one undo and one redo, each one call within the 64 edits of a `world_edit`.
#[test]
fn a_hundred_entities_under_one_are_destroyed_and_revived_in_one_call() {
    common::big_stack(|| {
        let mut g = game("hundred", TAG, &json!([{"name": "Keep"}]));
        let under = |parent: u64, name: String| {
            json!({"spawn": {"name": name, "components": {
                "Parent": {"parent": parent}, "Tag": {"on": parent, "note": name}}}})
        };
        let root = edit(&mut g, json!([{"spawn": {"name": "Root"}}]))["spawned"][0]
            .as_u64()
            .unwrap();
        let mut level = vec![root; 33];
        for depth in 0..3 {
            let ops: Vec<Value> = level
                .iter()
                .enumerate()
                .map(|(i, p)| under(*p, format!("n{depth}-{i}")))
                .collect();
            let r = edit(&mut g, Value::Array(ops));
            level = r["spawned"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect();
        }
        assert_eq!(entities(&mut g), 101);
        let nested = call(&mut g, "world.tree", json!({"root": "Root"}));
        assert_eq!(nested[0]["children"].as_array().unwrap().len(), 33);
        let built = g.world_hash().unwrap();
        let entries = undo_labels(&mut g).as_array().unwrap().len();

        let r = edit(&mut g, json!([destroy("Root")]));
        assert_eq!(r["results"], json!([{"op": "destroyed", "count": 100}]));
        assert_eq!(whole(&mut g), "Keep");
        let empty = g.world_hash().unwrap();
        assert_eq!(undo_labels(&mut g).as_array().unwrap().len(), entries + 1);
        let u = call(&mut g, "history.undo", json!({}));
        assert_eq!(u["applied"], json!(1), "{u}");
        assert_eq!(
            u["results"],
            json!([{"op": "revived", "id": root, "count": 100}])
        );
        assert_eq!(g.world_hash().unwrap(), built);
        let r = call(&mut g, "history.redo", json!({}));
        assert_eq!(r["applied"], json!(1), "{r}");
        assert_eq!(g.world_hash().unwrap(), empty);
        call(&mut g, "history.undo", json!({}));
        assert_eq!(g.world_hash().unwrap(), built);
        assert_eq!(
            call(&mut g, "history.list", json!({}))["redo"],
            json!(["destroy Root"])
        );
    });
}

#[test]
fn a_scene_brings_its_hierarchy_and_a_bad_one_is_refused_whole() {
    common::big_stack(|| {
        let parent = |n: u64| json!({"Parent": {"parent": n}});
        let entity = |name: &str, p: u64| json!({"name": name, "components": parent(p)});
        let bad = |name: &str, scene: Value| build(&project(name, TAG, &scene)).err().unwrap();
        // An entity that names itself.
        let e = bad("self", json!([{"name": "Free"}, entity("Own", 2)]));
        assert_eq!(e.code, "sim.parent_cycle", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("/entities/1/components/Parent/parent")
        );
        assert_eq!(e.message, "Entity 2 cannot be its own parent.");
        // A loop through an entity later in the file: named at the first entity in it.
        let e = bad(
            "loop",
            json!([entity("A", 3), {"name": "Free"}, entity("C", 4), entity("D", 1)]),
        );
        assert_eq!(e.code, "sim.parent_cycle", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("/entities/0/components/Parent/parent")
        );
        assert_eq!(e.detail["chain"], json!([3, 4, 1]));
        // A place past the file, and no parent.
        let e = bad("past", json!([{"name": "Free"}, entity("Far", 3)]));
        assert_eq!(e.code, "sim.entity_id_invalid", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("/entities/1/components/Parent/parent")
        );
        let e = bad(
            "none",
            json!([{"name": "No", "components": {"Parent": {}}}]),
        );
        assert_eq!(e.code, "request.missing_field", "{e:#?}");

        // A refused scene leaves nothing: no entity, and the first id is still to give.
        let mut g = game("into", TAG, &json!([]));
        let looped = json!({"format": "amoris-scene", "version": 1,
                            "entities": [entity("A", 2), entity("B", 1)]});
        let e = try_call(&mut g, "test.scene", looped).unwrap_err();
        assert_eq!(e.code, "sim.parent_cycle", "{e:#?}");
        assert_eq!(entities(&mut g), 0);
        let good = json!({"format": "amoris-scene", "version": 1,
                          "entities": [entity("Low", 2), {"name": "Top"}]});
        call(&mut g, "test.scene", good);
        assert_eq!(whole(&mut g), "Top(Low)");
        assert_eq!(id(&g, "Low"), 1);
        // A scene spawned into a world that has entities: its places are the ids it gets, so a
        // parent given as a place of the file names an entity of the world.
        let more = json!({"format": "amoris-scene", "version": 1,
                          "entities": [entity("Under", 2)]});
        call(&mut g, "test.scene", more);
        assert_eq!(whole(&mut g), "Top(Low Under)");
    });
}

#[test]
fn a_saved_scene_keeps_the_hierarchy_by_place() {
    common::big_stack(|| {
        let scene = json!([
            {"name": "Old"},
            {"name": "Rig"},
            {"name": "Arm", "components": {"Parent": {"parent": 2}}},
            {"name": "Hand", "components": {"Parent": {"parent": 3}}},
            {"name": "Stray", "components": {"Parent": {"parent": 1}}},
        ]);
        let root = project("save", TAG, &scene);
        let mut g = build(&root).unwrap();
        assert_eq!(whole(&mut g), "Old(Stray) Rig(Arm(Hand))");
        // Old goes behind the checks, so Stray is left naming an entity that is gone.
        call(&mut g, "test.parent", json!({"child": 2, "parent": 1}));
        edit(
            &mut g,
            json!([{"remove": {"entity": "Rig", "component": "Parent"}}]),
        );
        let stray = id(&g, "Stray");
        edit(
            &mut g,
            json!([{"remove": {"entity": "Stray", "component": "Parent"}}]),
        );
        edit(&mut g, json!([destroy("Old")]));
        call(&mut g, "test.parent", json!({"child": stray, "parent": 1}));
        assert_eq!(component(&mut g, "Stray", "Parent"), json!({"parent": 1}));
        assert_eq!(
            whole(&mut g),
            "Rig(Arm(Hand)) Stray",
            "a root in the listing"
        );

        call(&mut g, "project.save", json!({}));
        let text = std::fs::read_to_string(root.join("scene.json")).unwrap();
        let file: Value = serde_json::from_str(&text).unwrap();
        let parent = |i: usize| file["entities"][i]["components"]["Parent"].clone();
        assert_eq!(file["entities"][0]["name"], json!("Rig"));
        assert_eq!(parent(0), Value::Null);
        assert_eq!(parent(1), json!({"parent": 1}), "Arm under Rig's new place");
        assert_eq!(parent(2), json!({"parent": 2}));
        assert_eq!(parent(3), Value::Null, "a parent that is gone is left out");
        let mut again = build(&root).unwrap();
        assert_eq!(whole(&mut again), "Rig(Arm(Hand)) Stray");
        let _ = std::fs::remove_dir_all(&root);
    });
}

/// What a world may hold that no checked write makes: every read and every despawn ends, and the
/// listing places each live entity once.
#[test]
fn an_unchecked_hierarchy_is_listed_once_and_despawned_without_looping() {
    common::big_stack(|| {
        let names: Vec<Value> = ["A", "B", "C", "D", "E"]
            .iter()
            .map(|n| json!({ "name": n }))
            .collect();
        let mut g = game("unchecked", TAG, &Value::Array(names));
        // A and B are each other's parent, C hangs under B, D is its own, E names nothing live.
        for (child, parent) in [(1, 2), (2, 1), (3, 2), (4, 4), (5, 77)] {
            call(
                &mut g,
                "test.parent",
                json!({"child": child, "parent": parent}),
            );
        }
        assert_eq!(whole(&mut g), "A(B(C)) D E");
        assert_eq!(tree(&mut g, json!({"root": "B"})), "B(A C)");
        assert_eq!(tree(&mut g, json!({"root": "D"})), "D");
        assert_eq!(tree(&mut g, json!({"filter": "c"})), "A(B(C))");
        g.run(2).unwrap();
        // A destroy inside the loop takes the loop and what hangs under it; the undo is refused
        // (it would write the loop back) and dropped, and the world stays as the destroy left it.
        let r = edit(&mut g, json!([destroy("B")]));
        assert_eq!(
            r["results"],
            json!([{"op": "destroyed", "count": 3}]),
            "{r}"
        );
        assert_eq!(whole(&mut g), "D E");
        let e = try_call(&mut g, "history.undo", json!({})).unwrap_err();
        assert_eq!(e.code, "history.stale", "{e:#?}");
        let r = edit(&mut g, json!([destroy("D"), destroy("E")]));
        assert_eq!(r["applied"], json!(2));
        assert_eq!(entities(&mut g), 0);
    });
}

// --- Scripts -------------------------------------------------------------------------------------

/// A project whose one system runs `body` once, in the first tick, over the family scene:
/// `RIG` to `CRATE` are its entities.
fn script(body: &str) -> String {
    format!(
        r#"import {{ component, field, game, system }} from "amoris";
import type {{ Entity }} from "amoris";

export const Tag = component("Tag", {{
    version: 1, doc: "A note on an entity.",
    fields: {{
        on: field.entity("An entity it names."),
        note: field.str("", "Its text."),
    }},
}});

const [RIG, ARM, HAND, LEG, LOOSE, CRATE] = [1, 2, 3, 4, 5, 6] as unknown as Entity[];

export const probe = system({{
    name: "probe", phase: "update", doc: "The case under test.", when: "start",
    queries: {{ parented: {{ with: ["Parent"] }} }},
    run(ctx, {{ parented }}) {{
        const parent = (e: Entity) => ctx.world.get(e, "Parent")?.parent ?? null;
        const live = (...es: Entity[]) => es.map((e) => (ctx.world.exists(e) ? 1 : 0)).join("");
{body}
    }},
}});

export default game({{ components: [Tag], systems: [probe] }});
"#
    )
}

/// The 1-based line of `text` holding `MARK <name>`.
fn mark(text: &str, name: &str) -> u64 {
    let tag = format!("MARK {name}");
    let at = text.lines().position(|l| l.trim_end().ends_with(&tag));
    u64::try_from(at.unwrap_or_else(|| panic!("no {tag}"))).unwrap() + 1
}

/// The first tick of the family scene under a script made of `body`: the game after it, the
/// tick's report and the script's text.
fn first_tick(name: &str, body: &str) -> (Game, StepReport, String) {
    let text = script(body);
    let mut g = game(name, &text, &family());
    let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
    (g, r, text)
}

/// What the script emitted as `game.seen`, in order.
fn seen(r: &StepReport) -> Vec<Value> {
    r.events
        .iter()
        .filter(|e| e.kind.as_str() == "game.seen")
        .map(|e| e.data.to_json())
        .collect()
}

#[test]
fn a_script_sets_inserts_removes_and_spawns_with_a_parent() {
    common::big_stack(|| {
        let (mut g, r, _) = first_tick(
            "script-writes",
            r#"        ctx.world.set(HAND, "Parent", { parent: LEG });
        ctx.world.insert(LOOSE, "Parent", { parent: HAND });
        ctx.world.remove(CRATE, "Parent");
        const nail = ctx.world.spawn({ Parent: { parent: CRATE }, Tag: { note: "nail" } });
        ctx.world.set(ARM, "Parent", { parent: RIG });
        ctx.emit("game.seen", {
            hand: parent(HAND), loose: parent(LOOSE), crate: parent(CRATE), nail: parent(nail),
            has: ctx.world.has(CRATE, "Parent"),
            rows: parented.len, first: parented.cols.Parent.parent[0],
        });"#,
        );
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        // The call reads its own writes; the query's rows are the world's as the tick began.
        assert_eq!(
            seen(&r),
            [
                json!({"hand": 4, "loose": 3, "crate": null, "nail": 6, "has": false,
                    "rows": 4, "first": 1})
            ]
        );
        assert_eq!(whole(&mut g), "Rig(Arm Leg(Hand(Loose))) Crate(#7)");
        assert_eq!(component(&mut g, 7, "Parent"), json!({"parent": 6}));
        assert_eq!(component(&mut g, "Crate", "Parent"), Value::Null);
        g.run(3).unwrap();
        assert_eq!(whole(&mut g), "Rig(Arm Leg(Hand(Loose))) Crate(#7)");
    });
}

#[test]
fn a_script_despawn_takes_the_entities_under_it() {
    common::big_stack(|| {
        // A child spawned in the call, and a child of that one, go with the parent; the call
        // sees them gone at once.
        let (mut g, r, _) = first_tick(
            "script-despawn",
            r#"        const nail = ctx.world.spawn({ Parent: { parent: HAND } });
        const head = ctx.world.spawn({ Parent: { parent: nail } });
        const before = live(RIG, ARM, HAND, LEG, nail, head, LOOSE, CRATE);
        ctx.world.despawn(RIG);
        ctx.world.despawn(HAND);
        ctx.emit("game.seen", { before, after: live(RIG, ARM, HAND, LEG, nail, head, LOOSE, CRATE) });"#,
        );
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        assert_eq!(
            seen(&r),
            [json!({"before": "11111111", "after": "00000011"})]
        );
        assert_eq!(whole(&mut g), "Loose(Crate)");
        assert_eq!(entities(&mut g), 2);
        // The ids the call spawned are spent: the next spawn gets a new one.
        let r = edit(&mut g, json!([{"spawn": {"name": "Next"}}]));
        assert_eq!(r["spawned"], json!([9]));
    });
}

/// One call that despawns several subtrees and moves entities between the despawns: what the
/// call saw go is what the commit removes, so no staged write names an entity that vanished.
#[test]
fn despawns_and_reparents_in_one_call_commit_as_the_call_saw_them() {
    common::big_stack(|| {
        let (mut g, r, _) = first_tick(
            "script-mixed",
            r#"        ctx.world.set(HAND, "Parent", { parent: LOOSE });   // out from under Rig
        ctx.world.set(CRATE, "Tag", { note: "kept" });
        ctx.world.despawn(ARM);                              // takes nothing with it now
        const a = live(RIG, ARM, HAND, LEG, LOOSE, CRATE);
        ctx.world.set(HAND, "Tag", { note: "moved" });
        ctx.world.set(CRATE, "Parent", { parent: RIG });     // under Rig
        ctx.world.despawn(RIG);                              // takes Leg and Crate
        const b = live(RIG, ARM, HAND, LEG, LOOSE, CRATE);
        const pup = ctx.world.spawn({ Parent: { parent: HAND }, Tag: { on: LOOSE } });
        ctx.world.remove(HAND, "Parent");                    // a root again, with pup
        ctx.world.despawn(LOOSE);                            // takes nobody
        ctx.emit("game.seen", { a, b, c: live(RIG, ARM, HAND, LEG, LOOSE, CRATE, pup) });"#,
        );
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        assert_eq!(
            seen(&r),
            [json!({"a": "101111", "b": "001010", "c": "0010001"})]
        );
        assert_eq!(whole(&mut g), "Hand(#7)");
        assert_eq!(component(&mut g, "Hand", "Tag")["note"], json!("moved"));
        assert_eq!(component(&mut g, 7, "Tag")["on"], json!(5));
        g.run(2).unwrap();
        assert_eq!(whole(&mut g), "Hand(#7)");
    });
}

/// A script's refusal: its code, at its line, on its entity, with the system's effects discarded
/// (the family is as the scene made it).
fn script_refused(name: &str, body: &str, code: &str, entity: Option<u64>) -> Value {
    let (mut g, r, text) = first_tick(name, body);
    assert_eq!(r.errors.len(), 1, "{body}: {:#?}", r.errors);
    let p = &r.errors[0];
    assert_eq!(p.code, "sim.system_failed", "{p:#?}");
    assert_eq!(p.detail["system"], json!("script:probe"));
    let cause = p.detail["cause"].clone();
    assert_eq!(cause["code"], json!(code), "{body}: {cause:#}");
    let on = cause["detail"]["entity"].as_f64();
    assert_eq!(
        on,
        entity.map(|e| EntityId::new(e).unwrap().to_f64()),
        "{cause:#}"
    );
    let at = &cause["detail"]["location"];
    assert_eq!(at["file"], json!("scripts/main.ts"), "{cause:#}");
    assert_eq!(at["line"], json!(mark(&text, "here")), "{cause:#}");
    assert_eq!(whole(&mut g), FAMILY, "{body}");
    assert_eq!(
        component(&mut g, "Crate", "Tag")["note"],
        json!(""),
        "{body}"
    );
    assert!(seen(&r).is_empty(), "{body}");
    cause
}

#[test]
fn a_script_cannot_close_a_loop_name_no_parent_or_a_dead_one() {
    common::big_stack(|| {
        // Itself, and an entity under it.
        let c = script_refused(
            "script-self",
            r#"        ctx.world.set(CRATE, "Tag", { note: "discarded" });
        ctx.world.set(ARM, "Parent", { parent: ARM }); // MARK here"#,
            "script.parent_cycle",
            Some(2),
        );
        assert_eq!(c["message"], json!("Entity 2 cannot be its own parent."));
        assert_eq!(c["detail"]["component"], json!("Parent"));
        assert_eq!(c["detail"]["field"], json!("parent"));
        assert_eq!(c["detail"]["phase"], json!("run"));
        let c = script_refused(
            "script-under",
            r#"        ctx.world.insert(RIG, "Parent", { parent: HAND }); // MARK here"#,
            "script.parent_cycle",
            Some(1),
        );
        assert_eq!(
            c["message"],
            json!("Entity 1 cannot go under entity 3: 3 is under 1 (3 -> 2 -> 1).")
        );
        assert_eq!(c["detail"]["chain"], json!([3, 2, 1]));
        assert_eq!(c["detail"]["value"], json!(3));
        // The hierarchy the call's own writes made: Loose is under Hand by the first line.
        script_refused(
            "script-staged",
            r#"        ctx.world.insert(LOOSE, "Parent", { parent: HAND });
        ctx.world.set(ARM, "Parent", { parent: CRATE }); // MARK here"#,
            "script.parent_cycle",
            Some(2),
        );
        // No parent: the component always names one.
        let c = script_refused(
            "script-null",
            r#"        ctx.world.set(HAND, "Parent", { parent: null }); // MARK here"#,
            "script.write_not_held",
            Some(3),
        );
        assert_eq!(c["detail"]["field"], json!("parent"));
        assert!(
            c["message"].as_str().unwrap().contains("ctx.world.remove"),
            "{c:#}"
        );
        script_refused(
            "script-bare",
            r#"        ctx.world.insert(LOOSE, "Parent"); // MARK here"#,
            "script.write_not_held",
            Some(5),
        );
        script_refused(
            "script-spawn-bare",
            r#"        ctx.world.spawn({ Parent: {} }); // MARK here"#,
            "script.write_not_held",
            None,
        );
        // A parent that is gone: despawned by the call, itself or with the entity over it.
        script_refused(
            "script-dead",
            r#"        ctx.world.despawn(RIG);
        ctx.world.set(CRATE, "Parent", { parent: HAND }); // MARK here"#,
            "script.entity_missing",
            Some(6),
        );
        script_refused(
            "script-dead-spawn",
            r#"        ctx.world.despawn(LOOSE);
        ctx.world.spawn({ Parent: { parent: CRATE } }); // MARK here"#,
            "script.entity_missing",
            None,
        );
    });
}

/// A column cannot write `Parent.parent`: columns are written back before every command, so a
/// write there would change the hierarchy behind the call's despawns. The system fails at its
/// write-back and changes nothing.
#[test]
fn a_column_write_to_a_parent_fails_the_system() {
    common::big_stack(|| {
        let text = script(
            r#"        ctx.world.set(CRATE, "Tag", { note: "discarded" });
        parented.cols.Parent.parent[parented.row(HAND)] = LEG;"#,
        );
        let mut g = game("script-column", &text, &family());
        let r = g.step().unwrap();
        assert_eq!(r.errors.len(), 1, "{:#?}", r.errors);
        let cause = &r.errors[0].detail["cause"];
        assert_eq!(cause["code"], json!("script.write_not_held"), "{cause:#}");
        assert_eq!(cause["detail"]["phase"], json!("write_back"), "{cause:#}");
        assert_eq!(cause["detail"]["component"], json!("Parent"));
        assert_eq!(cause["detail"]["field"], json!("parent"));
        assert_eq!(cause["detail"]["entity"], json!(3.0));
        assert!(
            cause["message"].as_str().unwrap().contains("ctx.world.set"),
            "{cause:#}"
        );
        assert_eq!(whole(&mut g), FAMILY);
        assert_eq!(component(&mut g, "Crate", "Tag")["note"], json!(""));
        // The declarations give scripts the component with its entity field.
        let types = call(&mut g, "scripts.types", json!({"text": true}));
        let engine = types["components"]["engine"].as_array().unwrap();
        assert!(engine.contains(&json!("Parent")), "{types}");
        let dts = types["text"]["components.d.ts"].as_str().unwrap();
        assert!(dts.contains("readonly parent: Entity | null;"), "{dts}");
    });
}

// --- Replay, fork, restore and the storage's order -----------------------------------------------

/// Rules that keep no state of their own, so that the storage shuffle, which copies Rust
/// components alone, can run under them: every second tick an entity is spawned under the rig or
/// under an entity that has a parent, every fifth one entity with a parent is moved under another
/// unless that would close a loop, and every seventh one is despawned with what is under it. The
/// rows are drawn from each system's stream.
const RULES: &str = r#"
import { game, system } from "amoris";
import type { Entity } from "amoris";

const RIG = 1 as unknown as Entity;

export const grow = system({
    name: "grow", phase: "update", doc: "Spawns under the rig and moves entities about.",
    queries: { kids: { with: ["Parent"] } },
    run(ctx, { kids }) {
        if (!ctx.world.exists(RIG)) return;
        const row = () => kids.id(ctx.rng.int(0, kids.len - 1));
        if (ctx.tick % 2 === 0) {
            const under = kids.len > 0 && ctx.rng.chance(0.6) ? row() : RIG;
            ctx.world.spawn({ Parent: { parent: under } });
        }
        if (ctx.tick % 5 === 0 && kids.len >= 2) {
            const [a, b] = [row(), row()];
            // b must not be a or under it.
            let at: Entity | null = b;
            while (at !== null && at !== a) at = ctx.world.get(at, "Parent")?.parent ?? null;
            if (at === null) ctx.world.set(a, "Parent", { parent: b });
        }
    },
});

export const fell = system({
    name: "fell", phase: "update", doc: "Despawns an entity with what is under it.",
    queries: { kids: { with: ["Parent"] } },
    run(ctx, { kids }) {
        if (ctx.tick % 7 !== 0 || kids.len === 0) return;
        const e = kids.id(ctx.rng.int(0, kids.len - 1));
        const before = kids.len;
        ctx.world.despawn(e);
        let left = 0;
        for (let i = 0; i < kids.len; i++) if (ctx.world.exists(kids.id(i))) left++;
        ctx.emit("game.felled", { took: before - left });
    },
});

export default game({ components: [], systems: [grow, fell] });
"#;

fn rules_scene() -> Value {
    json!([
        {"name": "Rig"},
        {"name": "Arm", "components": {"Parent": {"parent": 1}}},
        {"name": "Hand", "components": {"Parent": {"parent": 2}}},
        {"name": "Finger", "components": {"Parent": {"parent": 3}}},
        {"name": "Leg", "components": {"Parent": {"parent": 1}}},
        {"name": "Base"},
        {"name": "Post", "components": {"Parent": {"parent": 6}}},
        {"name": "Sign", "components": {"Parent": {"parent": 7}}},
    ])
}

/// What a run saw: the hash after each step, and how many entities each of the script's despawns
/// took.
#[derive(Debug, PartialEq, Default)]
struct Run {
    hashes: Vec<String>,
    felled: Vec<u64>,
}

/// The edits and ticks of the run every check below repeats: a reparent, a destroy of a root with
/// its undo and redo, and ticks in which the scripts spawn, move and despawn.
fn drive(g: &mut Game, before_tick: &mut dyn FnMut(&mut Game)) -> Run {
    let mut out = Run::default();
    let mut run = |g: &mut Game, ticks: u64, out: &mut Run| {
        for _ in 0..ticks {
            before_tick(g);
            let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            assert!(r.errors.is_empty(), "tick {}: {:#?}", r.tick.0, r.errors);
            for e in r.events.iter().filter(|e| e.kind.as_str() == "game.felled") {
                out.felled.push(e.data.to_json()["took"].as_u64().unwrap());
            }
            out.hashes.push(format!("{:?}", g.world_hash().unwrap()));
        }
    };
    let hash = |g: &Game, out: &mut Run| out.hashes.push(format!("{:?}", g.world_hash().unwrap()));
    run(g, 3, &mut out);
    edit(g, json!([set_parent("Leg", 6)]));
    hash(g, &mut out);
    run(g, 5, &mut out);
    // Base is a root, which the scripts never despawn: it is there, with whatever is under it.
    edit(g, json!([destroy("Base")]));
    hash(g, &mut out);
    call(g, "history.undo", json!({}));
    hash(g, &mut out);
    call(g, "history.redo", json!({}));
    hash(g, &mut out);
    run(g, 60, &mut out);
    out
}

#[test]
fn a_recording_a_fork_a_restore_and_a_shuffled_storage_agree() {
    common::big_stack(|| {
        let root = project("rules", RULES, &rules_scene());
        let mut g = build(&root).unwrap();
        assert_eq!(whole(&mut g), "Rig(Arm(Hand(Finger)) Leg) Base(Post(Sign))");
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut fork = g.fork().unwrap();
        let first = drive(&mut g, &mut |_| {});
        g.finish_recording().unwrap();
        // The scripts did their part: the rig grew, and despawns took entities with what was
        // under them.
        assert_eq!(first.felled.len(), 9, "{:?}", first.felled);
        assert!(first.felled.iter().any(|n| *n > 1), "{:?}", first.felled);
        assert!(entities(&mut g) > 8, "{}", whole(&mut g));
        let nested = call(&mut g, "world.tree", json!({"root": "Rig"}));
        assert!(!nested[0]["children"].as_array().unwrap().is_empty());

        // The recording replays to the same hash chain.
        let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut fresh = Game::for_replay(&replay).unwrap_or_else(|e| panic!("{e:#?}"));
        let out = verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome;
        assert_eq!(out, ReplayOutcome::Identical);
        // A fork taken before the run gives the same hashes for the same commands.
        assert_eq!(drive(&mut fork, &mut |_| {}), first);
        // So does a game whose storage is permuted before every tick.
        let mut shuffled = build(&root).unwrap();
        let mut seed = 0;
        let permuted = drive(&mut shuffled, &mut |g| {
            seed += 1;
            call(g, "test.shuffle", json!({ "seed": seed }));
        });
        assert_eq!(permuted, first);

        // A snapshot keeps the hierarchy: restored into the game after more edits, the tree and
        // the hash are the snapshot's, and the run goes on as the original would.
        let mut a = build(&root).unwrap();
        a.run(5).unwrap();
        let snap = a.snapshot().unwrap();
        let (tree_then, hash_then) = (whole(&mut a), a.world_hash().unwrap());
        let mut b = a.fork().unwrap();
        edit(&mut a, json!([set_parent("Base", 1), destroy("Arm")]));
        a.run(3).unwrap();
        assert_ne!(whole(&mut a), tree_then);
        a.restore(&snap).unwrap();
        assert_eq!(whole(&mut a), tree_then);
        assert_eq!(a.world_hash().unwrap(), hash_then);
        a.run(20).unwrap();
        b.run(20).unwrap();
        assert_eq!(a.world_hash().unwrap(), b.world_hash().unwrap());
        assert_eq!(whole(&mut a), whole(&mut b));
        let _ = std::fs::remove_dir_all(&root);
    });
}

// --- world.tree ----------------------------------------------------------------------------------

#[test]
fn world_tree_nests_and_honours_root_depth_filter_with_and_limit() {
    common::big_stack(|| {
        let mut g = game("tree", TAG, &family());
        let all = call(&mut g, "world.tree", json!({}));
        assert_eq!(shape(&all), FAMILY);
        // A node: its id, name, component names (`Name` left out) and children, ascending by id.
        assert_eq!(
            all[0]["children"][0],
            json!({"id": 2, "name": "Arm", "components": ["Parent"], "children": [
                {"id": 3, "name": "Hand", "components": ["Parent", "Tag"], "children": []}]})
        );
        assert_eq!(all[0]["components"], json!([]));
        assert!(all[0].get("more").is_none());

        // One subtree, by id, by name and by `Name#id`.
        assert_eq!(tree(&mut g, json!({"root": 2})), "Arm(Hand)");
        assert_eq!(tree(&mut g, json!({"root": "Loose"})), "Loose(Crate)");
        assert_eq!(tree(&mut g, json!({"root": "Hand#3"})), "Hand");
        let e = try_call(&mut g, "world.tree", json!({"root": "Nobody"})).unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found");
        let e = try_call(&mut g, "world.tree", json!({"root": 99})).unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found");

        // Depth: levels below where the listing starts; a node it cut says how many it left.
        assert_eq!(tree(&mut g, json!({"depth": 0})), "Rig+2 Loose+1");
        assert_eq!(
            tree(&mut g, json!({"depth": 1})),
            "Rig(Arm+1 Leg) Loose(Crate)"
        );
        assert_eq!(tree(&mut g, json!({"depth": 2})), FAMILY);
        assert_eq!(tree(&mut g, json!({"depth": 4000000000u32})), FAMILY);
        assert_eq!(tree(&mut g, json!({"root": "Rig", "depth": 0})), "Rig+2");
        let cut = call(&mut g, "world.tree", json!({"depth": 0}));
        assert_eq!(cut[0]["more"], json!(2));
        assert_eq!(cut[0]["children"], json!([]));

        // A filter and `with` keep what matches with the entities it is under.
        assert_eq!(tree(&mut g, json!({"filter": "HAN"})), "Rig(Arm(Hand))");
        assert_eq!(tree(&mut g, json!({"filter": "l"})), "Rig(Leg) Loose");
        assert_eq!(
            tree(&mut g, json!({"with": ["Tag"]})),
            "Rig(Arm(Hand)) Loose(Crate)"
        );
        assert_eq!(
            tree(&mut g, json!({"with": ["Tag", "Parent"], "filter": "cr"})),
            "Loose(Crate)"
        );
        assert_eq!(tree(&mut g, json!({"filter": "nobody"})), "");
        assert_eq!(tree(&mut g, json!({"root": "Loose", "filter": "hand"})), "");
        assert_eq!(tree(&mut g, json!({"filter": "hand", "depth": 1})), "");
        let e = try_call(&mut g, "world.tree", json!({"with": ["Tga"]})).unwrap_err();
        assert_eq!(e.code, "sim.component_unknown");

        // A limit counts listed entities, a parent before its children.
        assert_eq!(tree(&mut g, json!({"limit": 3})), "Rig(Arm(Hand))");
        assert_eq!(tree(&mut g, json!({"limit": 4})), "Rig(Arm(Hand) Leg)");
        assert_eq!(tree(&mut g, json!({"limit": 0})), "");
        assert_eq!(
            tree(&mut g, json!({"filter": "crate", "limit": 1})),
            "Loose"
        );

        // Past 48 levels a chain is cut, and `root` reads on from there.
        let mut parent = id(&g, "Crate");
        for _ in 0..60 {
            let r = edit(
                &mut g,
                json!([{"spawn": {"components": {"Parent": {"parent": parent}}}}]),
            );
            parent = r["spawned"][0].as_u64().unwrap();
        }
        let deep = call(&mut g, "world.tree", json!({"root": "Loose"}));
        let mut at = &deep[0];
        let mut levels = 0;
        while let Some(child) = at["children"].get(0) {
            at = child;
            levels += 1;
        }
        assert_eq!(levels, 48);
        assert_eq!(at["more"], json!(1));
        let rest = call(&mut g, "world.tree", json!({"root": at["id"]}));
        let mut at = &rest[0];
        let mut below = 0;
        while let Some(child) = at["children"].get(0) {
            at = child;
            below += 1;
        }
        // Loose, Crate and 60 more: 61 levels below Loose.
        assert_eq!(levels + below, 61);
        assert_eq!(at["id"], json!(parent));
        // The whole answer is JSON a reader with the usual depth limit takes.
        let text = serde_json::to_string(&deep).unwrap();
        assert!(serde_json::from_str::<Value>(&text).is_ok());
    });
}
