//! A `world_edit` checked in order (charter 3.4; threads.md 5.4): a call refused partway changes
//! nothing and replays, two sets of one component keep both; scene entities that name each other;
//! a recording that goes on through a restore and records one fault once; and names (server.md
//! 3.1): a `set` of `Name` from a string renames, a `remove` leaves the entity unnamed, and the
//! later edits of a call find entities by the names the earlier ones leave.
#![cfg(feature = "transpile")]
mod common;

use std::sync::Arc;

use amoris_link::Source;
use amoris_persist::replay::{
    MemorySink, Record, RecordOptions, RecordedWrite, Replay, ReplayMode, ReplayOutcome,
    VerifyOptions, verify,
};
use amoris_runtime::{Command, Game, GameBuilder, Scene};
use amoris_sim::{RunCondition, TickPhase};
use serde_json::{Value, json};

fn edit(g: &mut Game, seq: u64, edits: Value) -> Result<Value, amoris_contract::Problem> {
    g.apply(&Command::new(
        Source::Player(0),
        seq,
        "world_edit",
        json!({ "edits": edits }),
    ))
}

/// `world.edit` as the editor sends it: `ops`, externally tagged.
fn ops(g: &mut Game, seq: u64, ops: Value) -> Result<Value, amoris_contract::Problem> {
    g.apply(&Command::new(
        Source::Editor,
        seq,
        "world.edit",
        json!({ "ops": ops }),
    ))
}

/// The editor's rename as an op: a `set` of `Name` whose value is the name.
fn rename(entity: u64, name: &str) -> Value {
    json!({"set": {"entity": entity, "component": "Name", "value": name}})
}

/// A `set` of a crate's `Cargo.value`, as an op.
fn cargo(entity: Value, value: u32) -> Value {
    json!({"set": {"entity": entity, "component": "Cargo", "value": {"value": value}}})
}

/// A read (`world.get`, `world.tree`, `history.list`), answered or refused.
fn read(g: &mut Game, name: &str, params: Value) -> Result<Value, amoris_contract::Problem> {
    g.apply(&Command::new(Source::Developer(0), 1, name, params))
}

/// The entity's name as `world.get` answers it (`null` for an entity without one).
fn name_of(g: &mut Game, id: u64) -> Value {
    read(g, "world.get", json!({"entity": id})).unwrap_or_else(|e| panic!("{e:#?}"))["name"].clone()
}

/// Every write a recording holds, in order.
fn writes(sink: &MemorySink) -> Vec<RecordedWrite> {
    let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
    replay
        .records()
        .iter()
        .flat_map(|r| match r {
            Record::Tick { writes, .. } => writes.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn get(g: &mut Game, entity: &str, component: &str) -> Value {
    let r = g
        .apply(&Command::new(
            Source::Developer(0),
            1,
            "world_get",
            json!({"entity": entity, "components": [component]}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
    r["components"][component].clone()
}

fn verified(sink: &MemorySink) -> ReplayOutcome {
    let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
    let mut fresh = Game::for_replay(&replay).unwrap_or_else(|e| panic!("{e:#?}"));
    verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome
}

/// The review's probe: a set, a destroy, then a set of the destroyed entity. The third edit is
/// refused at its path, the first two are not applied, and a recording with the refusal replays.
#[test]
fn an_edit_of_an_entity_destroyed_earlier_in_the_call_refuses_the_whole_call() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.run(1).unwrap();
        let before = g.world_hash().unwrap();
        let rudder = get(&mut g, "Sloop", "Boat")["rudder"].clone();
        let e = edit(
            &mut g,
            1,
            json!([
                {"op": "set", "entity": "Sloop", "component": "Boat", "value": {"rudder": 0.5}},
                {"op": "destroy", "entity": "Crate4"},
                {"op": "set", "entity": "Crate4", "component": "Cargo", "value": {"value": 9}}
            ]),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/edits/2/entity"), "{e:#?}");
        assert_eq!(e.detail["destroyed_by"], json!(1), "{e:#?}");
        assert_eq!(g.world_hash().unwrap(), before);
        assert_eq!(get(&mut g, "Sloop", "Boat")["rudder"], rudder);
        assert_eq!(get(&mut g, "Crate4", "Cargo")["value"], json!(3));
        // A second destroy of the same entity, and an entity field naming it, are refused alike.
        for (seq, edits, path) in [
            (
                2,
                json!([{"op": "destroy", "entity": "Crate4"}, {"op": "destroy", "entity": 7}]),
                "/edits/1/entity",
            ),
            (
                3,
                json!([
                    {"op": "destroy", "entity": "Crate4"},
                    {"op": "set", "entity": "Sloop", "component": "Crew",
                     "value": {"take": "Crate4"}}
                ]),
                "/edits/1/value/take",
            ),
            (
                4,
                json!([
                    {"op": "destroy", "entity": "Crate4"},
                    {"op": "spawn", "name": "Mate", "components": {"Crew": {"take": "Crate4"}}}
                ]),
                "/edits/1/components/Crew/take",
            ),
        ] {
            let e = edit(&mut g, seq, edits).unwrap_err();
            assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
            assert_eq!(e.detail["path"], json!(path), "{e:#?}");
            assert_eq!(g.world_hash().unwrap(), before);
        }
        // What is allowed in one call: destroying one crate and taking another.
        edit(
            &mut g,
            5,
            json!([
                {"op": "destroy", "entity": "Crate4"},
                {"op": "set", "entity": "Sloop", "component": "Crew", "value": {"take": "Crate1"}}
            ]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        g.run(5).unwrap();
        g.finish_recording().unwrap();
        assert_eq!(verified(&sink), ReplayOutcome::Identical);
    });
}

/// mcp.md 6.1: other fields keep their values and edits apply in order, also within one call.
#[test]
fn two_sets_of_one_component_in_one_call_keep_both() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let y = get(&mut g, "Crate4", "Transform")["position"][1].clone();
        let total = get(&mut g, "Sloop", "Tally")["total"].clone();
        edit(
            &mut g,
            1,
            json!([
                {"op": "set", "entity": "Crate4", "component": "Transform",
                 "value": {"position.0": 100}},
                {"op": "set", "entity": "Crate4", "component": "Transform",
                 "value": {"position.2": 50}},
                {"op": "set", "entity": "Sloop", "component": "Tally", "value": {"taken": 2}},
                {"op": "set", "entity": "Sloop", "component": "Tally", "value": {"worth": 7}},
                {"op": "remove", "entity": "Sloop", "component": "Log"},
                {"op": "set", "entity": "Sloop", "component": "Log", "value": {"top_speed": 3}}
            ]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(
            get(&mut g, "Crate4", "Transform")["position"],
            json!([100.0, y, 50.0])
        );
        let tally = get(&mut g, "Sloop", "Tally");
        assert_eq!((&tally["taken"], &tally["worth"]), (&json!(2), &json!(7)));
        assert_eq!(tally["total"], total, "{tally}");
        // A set after a remove starts from the defaults.
        let log = get(&mut g, "Sloop", "Log");
        assert_eq!(
            log,
            json!({"distance": 0.0, "top_speed": 3.0, "sail_set": false})
        );
        g.run(3).unwrap();
        g.finish_recording().unwrap();
        assert_eq!(verified(&sink), ReplayOutcome::Identical);
    });
}

/// Charter 3.2: entities refer to each other by id, so a scene's entity field may name any entity
/// of the scene, before or after it in the file, itself included.
#[test]
fn scene_entities_name_each_other() {
    common::big_stack(|| {
        let mut setup = (*common::sailing()).clone();
        let mut scene: Value = serde_json::to_value(&setup.scene).unwrap();
        scene["entities"][2]["components"]["Crew"] = json!({"take": "Crate4"});
        scene["entities"].as_array_mut().unwrap().push(json!({
            "name": "Mate", "components": {"Crew": {"take": "Mate"}}
        }));
        setup.scene = Scene::from_json(&scene.to_string()).unwrap();
        let mut g = Game::new(Arc::new(setup.clone()), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(get(&mut g, "Sloop", "Crew")["take"], json!(7));
        // Ids are allocated in scene order from 1: Mate, appended last, has the last id.
        let mate = scene["entities"].as_array().unwrap().len();
        assert_eq!(get(&mut g, "Mate", "Crew")["take"], json!(mate));
        // A name no entity has is refused with the scene's names to choose from.
        scene["entities"][2]["components"]["Crew"] = json!({"take": "Crate9"});
        setup.scene = Scene::from_json(&scene.to_string()).unwrap();
        let e = Game::new(Arc::new(setup), 1).err().expect("refused");
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert!(
            e.detail["suggestions"].to_string().contains("Crate"),
            "{e:#?}"
        );
    });
}

/// replay.md 2.5: a restore while recording ends the segment and rebases, so the recording goes
/// on and replays over both segments.
#[test]
fn a_restore_while_recording_rebases_and_replays() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 2).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.run(5).unwrap();
        let snap = g.snapshot().unwrap();
        g.run(3).unwrap();
        g.restore(&snap).unwrap();
        g.step().unwrap_or_else(|e| panic!("{e:#?}"));
        edit(
            &mut g,
            1,
            json!([{"op": "set", "entity": "Sloop", "component": "Boat", "value": {"rudder": 0.3}}]),
        )
        .unwrap();
        g.run(4).unwrap();
        let summary = g.finish_recording().unwrap().unwrap();
        assert_eq!(summary.segments, 2);
        let replay = Replay::read(&sink.bytes()).unwrap();
        assert_eq!(replay.segments().len(), 2);
        assert_eq!(verified(&sink), ReplayOutcome::Identical);
    });
}

fn panics_at_three(sim: &mut amoris_sim::Sim) -> Result<(), amoris_contract::Problem> {
    sim.add_system(
        "test.panic",
        TickPhase::Update,
        RunCondition::Always,
        |clock: bevy_ecs::prelude::Res<amoris_sim::SimClock>| {
            assert!(clock.tick.0 != 3, "the test system panics at tick 3");
        },
    )
}

/// replay.md 2.3: one fault is one `Fault` record however often a poisoned world is stepped, and a
/// restore after it opens the next segment.
#[test]
fn a_fault_is_recorded_once_and_a_restore_records_on() {
    common::big_stack(|| {
        let mut g = GameBuilder::new(common::sailing())
            .seed(1)
            .system(panics_at_three)
            .build()
            .unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.run(1).unwrap();
        let snap = g.snapshot().unwrap();
        g.run(1).unwrap();
        assert_eq!(g.step().unwrap_err().code, "sim.internal");
        for _ in 0..8 {
            assert_eq!(g.step().unwrap_err().code, "sim.world_poisoned");
        }
        g.restore(&snap).unwrap();
        g.run(1).unwrap_or_else(|e| panic!("{e:#?}"));
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let faults = replay
            .records()
            .iter()
            .filter(|r| matches!(r, Record::Fault { .. }))
            .count();
        assert_eq!(faults, 1);
        assert_eq!(replay.segments().len(), 2);
    });
}

/// editor.md 8 item 2: the editor's rename, a `set` of `Name` whose value is the name. The entity
/// is found by the new name and no longer by the old one, the write is recorded in the canonical
/// form with the same value, and the recording replays.
#[test]
fn a_set_of_name_renames_the_entity_and_replays() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.run(1).unwrap();
        let before = g.world_hash().unwrap();
        // What editor/src/actions/world.ts sends for F2 and for the inspector's name field.
        let r = g
            .apply(&Command::new(
                Source::Editor,
                1,
                "world.edit",
                json!({"ops": [{"set": {"entity": 4, "component": "Name", "value": "CrateA"}}],
                       "label": "Rename Crate1 → CrateA"}),
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(r["applied"], json!(1), "{r}");
        assert_eq!(r["label"], json!("Rename Crate1 → CrateA"), "{r}");
        assert_ne!(g.world_hash().unwrap(), before);
        let got = read(&mut g, "world.get", json!({"entity": "CrateA"})).unwrap();
        assert_eq!((&got["id"], &got["name"]), (&json!(4), &json!("CrateA")));
        assert_eq!(got["components"]["Name"], json!("CrateA"), "{got}");
        // The other components are the crate's own still.
        assert_eq!(got["components"]["Cargo"], json!({"value": 1}), "{got}");
        let e = read(&mut g, "world.get", json!({"entity": "Crate1"})).unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        let tree = read(&mut g, "world.tree", json!({"filter": "CrateA"})).unwrap();
        assert_eq!(tree.as_array().map(Vec::len), Some(1), "{tree}");
        assert_eq!(tree[0]["id"], json!(4), "{tree}");
        // The canonical form renames too, here an entity named by its name; `replace` changes
        // nothing for a name, which is always written whole.
        edit(
            &mut g,
            1,
            json!([{"op": "set", "entity": "Crate2", "component": "Name", "value": "CrateB",
                    "replace": true}]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(name_of(&mut g, 5), json!("CrateB"));
        // A name the entity already has is written again: no refusal.
        ops(
            &mut g,
            2,
            json!([{"set": {"entity": "CrateB", "component": "Name", "value": "CrateB"}}]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        g.run(3).unwrap();
        g.finish_recording().unwrap();
        // Recorded as `world_edit` with the entity's id and the name as the value.
        let recorded: Vec<(String, Value)> = writes(&sink)
            .iter()
            .map(|w| (w.name.clone(), serde_json::from_str(&w.params).unwrap()))
            .collect();
        let set = |entity: u64, name: &str, replace: bool| {
            let mut edit = rename(entity, name)["set"].clone();
            edit["op"] = json!("set");
            if replace {
                edit["replace"] = json!(true);
            }
            ("world_edit".to_owned(), json!({ "edits": [edit] }))
        };
        assert_eq!(
            recorded,
            [
                set(4, "CrateA", false),
                set(5, "CrateB", true),
                set(5, "CrateB", false),
            ]
        );
        assert_eq!(verified(&sink), ReplayOutcome::Identical);
    });
}

/// `Name` takes a string of at most 64 bytes of UTF-8 and nothing else takes a string: each
/// refusal names the op's `value` and changes nothing.
#[test]
fn a_name_is_a_string_of_at_most_64_bytes() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let before = g.world_hash().unwrap();
        let set = |component: &str, value: Value| {
            let op = json!({"entity": 4, "component": component, "value": value});
            json!([{ "set": op }])
        };
        let mut seq = 0;
        let mut refused = |g: &mut Game, ops_: Value| {
            seq += 1;
            let e = ops(g, seq, ops_).unwrap_err();
            assert_eq!(g.world_hash().unwrap(), before, "{e:#?}");
            assert_eq!(name_of(g, 4), json!("Crate1"), "{e:#?}");
            e
        };
        // An object where the name goes.
        let e = refused(&mut g, set("Name", json!({"name": "CrateA"})));
        assert_eq!(e.code, "request.wrong_type", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/0/set/value"), "{e:#?}");
        assert_eq!(
            (&e.detail["expected"], &e.detail["got"]),
            (&json!("string"), &json!("object")),
            "{e:#?}"
        );
        // Neither an object nor a string: refused by the shape.
        for bad in [json!(7), json!(null), json!(["CrateA"])] {
            let e = refused(&mut g, set("Name", bad));
            assert_eq!(e.code, "request.wrong_type", "{e:#?}");
            assert_eq!(e.detail["path"], json!("/ops/0/set/value"), "{e:#?}");
            assert_eq!(e.detail["expected"], json!("object or string"), "{e:#?}");
        }
        // 65 bytes; and 33 two-byte characters, which are 66 bytes though 33 characters.
        for (name, bytes) in [("x".repeat(65), 65), ("é".repeat(33), 66)] {
            let e = refused(&mut g, set("Name", json!(name)));
            assert_eq!(e.code, "sim.name_too_long", "{e:#?}");
            assert_eq!(e.detail["path"], json!("/ops/0/set/value"), "{e:#?}");
            assert_eq!(
                (&e.detail["bytes"], &e.detail["max"]),
                (&json!(bytes), &json!(64)),
                "{e:#?}"
            );
        }
        // A string for a component that is not `Name`, project or engine: its fields go in an
        // object, as before.
        for component in ["Cargo", "Transform"] {
            let e = refused(&mut g, set(component, json!("nine")));
            assert_eq!(e.code, "request.wrong_type", "{component}: {e:#?}");
            assert_eq!(e.detail["path"], json!("/ops/0/set/value"), "{e:#?}");
            assert_eq!(
                (&e.detail["expected"], &e.detail["got"]),
                (&json!("object"), &json!("string")),
                "{e:#?}"
            );
        }
        // A string for a component that does not exist: the registry's nearest names, `Name`
        // among them now that it can be set.
        let e = refused(&mut g, set("name", json!("CrateA")));
        assert_eq!(e.code, "sim.component_unknown", "{e:#?}");
        assert_eq!(e.detail["suggestions"], json!(["Name"]), "{e:#?}");
        // In the canonical form the path is the edit's.
        let e = edit(
            &mut g,
            1,
            json!([{"op": "set", "entity": 4, "component": "Name", "value": "x".repeat(65)}]),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.name_too_long", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/edits/0/value"), "{e:#?}");
        // A name too long in the second op refuses the whole call: the first rename is not applied.
        let e = refused(
            &mut g,
            json!([
                {"set": {"entity": 4, "component": "Name", "value": "CrateA"}},
                {"set": {"entity": 5, "component": "Name", "value": "y".repeat(65)}}
            ]),
        );
        assert_eq!(e.code, "sim.name_too_long", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/set/value"), "{e:#?}");
        // 64 bytes fit, as 32 two-byte characters do.
        for (id, name) in [(4, "x".repeat(64)), (5, "é".repeat(32))] {
            ops(
                &mut g,
                90 + id,
                json!([{"set": {"entity": id, "component": "Name", "value": name}}]),
            )
            .unwrap_or_else(|e| panic!("{e:#?}"));
            assert_eq!(name_of(&mut g, id), json!(name));
        }
    });
}

/// The decision on `remove` of `Name` (server.md 3.1): the entity goes on unnamed, as one spawned
/// without a name, found by its id alone; an undo names it again.
#[test]
fn a_remove_of_name_leaves_the_entity_unnamed() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let before = g.world_hash().unwrap();
        let undo_entries = |g: &mut Game| {
            read(g, "history.list", json!({})).unwrap()["undo"]
                .as_array()
                .unwrap()
                .len()
        };
        let r = ops(
            &mut g,
            1,
            json!([{"remove": {"entity": "Crate2", "component": "Name"}}]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(r["label"], json!("remove Name from Crate2"), "{r}");
        assert_ne!(g.world_hash().unwrap(), before);
        let got = read(&mut g, "world.get", json!({"entity": 5})).unwrap();
        assert_eq!(got["name"], Value::Null, "{got}");
        assert!(got["components"].get("Name").is_none(), "{got}");
        assert_eq!(got["components"]["Cargo"], json!({"value": 2}), "{got}");
        let tree = read(&mut g, "world.tree", json!({})).unwrap();
        let row = tree
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == json!(5))
            .unwrap();
        assert_eq!(row["name"], Value::Null, "{row}");
        let e = read(&mut g, "world.get", json!({"entity": "Crate2"})).unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        // One history entry; its undo names the entity again and its redo takes the name away.
        assert_eq!(undo_entries(&mut g), 1);
        g.apply(&Command::new(Source::Editor, 2, "history.undo", json!({})))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(name_of(&mut g, 5), json!("Crate2"));
        assert_eq!(g.world_hash().unwrap(), before);
        g.apply(&Command::new(Source::Editor, 3, "history.redo", json!({})))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(name_of(&mut g, 5), Value::Null);
        // Removing the name of an entity without one changes nothing and leaves no entry.
        let unnamed = g.world_hash().unwrap();
        ops(
            &mut g,
            4,
            json!([{"remove": {"entity": 5, "component": "Name"}}]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(g.world_hash().unwrap(), unnamed);
        assert_eq!(undo_entries(&mut g), 1);
        // A `set` names it again, and an undo of that naming leaves it unnamed as before.
        ops(
            &mut g,
            5,
            json!([{"set": {"entity": 5, "component": "Name", "value": "Box"}}]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(name_of(&mut g, 5), json!("Box"));
        g.apply(&Command::new(Source::Editor, 6, "history.undo", json!({})))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(name_of(&mut g, 5), Value::Null);
        assert_eq!(g.world_hash().unwrap(), unnamed);
        g.run(3).unwrap();
        g.finish_recording().unwrap();
        let recorded = writes(&sink);
        assert_eq!(recorded.len(), 6);
        assert!(recorded.iter().all(|w| w.name == "world_edit"));
        assert_eq!(verified(&sink), ReplayOutcome::Identical);
    });
}

/// server.md 3.1: a call is checked against the world as its earlier ops leave it, names
/// included. After a rename the new name finds the entity and the old one does not, as an op's
/// `entity` and through an entity field; a refusal names the op and nothing is applied.
#[test]
fn later_edits_of_a_call_see_the_names_the_earlier_ones_leave() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let cargo_of = |g: &mut Game, id: u64| {
            let got = read(
                g,
                "world.get",
                json!({"entity": id, "fields": ["Cargo.value"]}),
            );
            got.unwrap()["fields"]["Cargo.value"].clone()
        };
        let mut seq = 0;
        let mut call = |g: &mut Game, ops_: Value| {
            seq += 1;
            ops(g, seq, ops_)
        };

        // A reference that finds no entity names its place in the call, whether or not an
        // earlier op changed a name.
        let hash = g.world_hash().unwrap();
        for (ops_, path) in [
            (
                json!([cargo(json!(4), 1), cargo(json!("Nobody"), 1)]),
                "/ops/1/set/entity",
            ),
            (
                json!([cargo(json!(4), 1), cargo(json!(99), 1)]),
                "/ops/1/set/entity",
            ),
            (
                json!([{"set": {"entity": "Sloop", "component": "Crew",
                                "value": {"take": "Nobody"}}}]),
                "/ops/0/set/value/take",
            ),
            (
                json!([{"spawn": {"components": {"Crew": {"take": 99}}}}]),
                "/ops/0/spawn/components/Crew/take",
            ),
            (
                json!([rename(4, "CrateA"), cargo(json!(99), 1)]),
                "/ops/1/set/entity",
            ),
        ] {
            let e = call(&mut g, ops_).unwrap_err();
            assert_eq!(e.code, "sim.entity_not_found", "{path}: {e:#?}");
            assert_eq!(e.detail["path"], json!(path), "{e:#?}");
            assert_eq!(g.world_hash().unwrap(), hash);
        }

        // The new name finds the entity in the same call.
        call(
            &mut g,
            json!([rename(4, "CrateA"), cargo(json!("CrateA"), 9)]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(cargo_of(&mut g, 4), json!(9));
        // The old one no longer does: refused at the second op, with the names as the first op
        // leaves them to choose from, and the rename is not applied.
        let hash = g.world_hash().unwrap();
        let e = call(
            &mut g,
            json!([rename(4, "CrateB"), cargo(json!("CrateA"), 5)]),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/set/entity"), "{e:#?}");
        assert_eq!(e.detail["ref"], json!("CrateA"), "{e:#?}");
        let suggested = e.detail["suggestions"].to_string();
        assert!(
            suggested.contains("CrateB") && !suggested.contains("CrateA"),
            "{e:#?}"
        );
        assert_eq!(g.world_hash().unwrap(), hash);
        assert_eq!(name_of(&mut g, 4), json!("CrateA"));
        // Unless another entity carries the old name by then.
        call(
            &mut g,
            json!([
                rename(4, "CrateB"),
                rename(5, "CrateA"),
                cargo(json!("CrateA"), 5)
            ]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(
            (cargo_of(&mut g, 4), cargo_of(&mut g, 5)),
            (json!(9), json!(5))
        );

        // An entity field given by name follows the same names: the new one is found (and
        // recorded as the id), the old one refused at the field.
        call(
            &mut g,
            json!([
                rename(5, "Lead"),
                {"set": {"entity": "Sloop", "component": "Crew", "value": {"take": "Lead"}}}
            ]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(get(&mut g, "Sloop", "Crew")["take"], json!(5));
        let hash = g.world_hash().unwrap();
        let e = call(
            &mut g,
            json!([
                rename(6, "Tail"),
                {"set": {"entity": "Sloop", "component": "Crew", "value": {"take": "Crate3"}}}
            ]),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/set/value/take"), "{e:#?}");
        let e = call(
            &mut g,
            json!([
                rename(6, "Tail"),
                {"spawn": {"name": "Mate", "components": {"Crew": {"take": "Crate3"}}}}
            ]),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("/ops/1/spawn/components/Crew/take"),
            "{e:#?}"
        );
        assert_eq!(g.world_hash().unwrap(), hash);
        // A spawn's entity field by the new name.
        let r = call(
            &mut g,
            json!([
                rename(6, "Tail"),
                {"spawn": {"name": "Mate", "components": {"Crew": {"take": "Tail"}}}}
            ]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let mate = r["spawned"][0].as_u64().unwrap();
        let crew = read(
            &mut g,
            "world.get",
            json!({"entity": mate, "components": ["Crew"]}),
        )
        .unwrap();
        assert_eq!(crew["components"]["Crew"]["take"], json!(6), "{crew}");

        // Two entities swap names in one call; each later op sees the names as they stand then.
        // Now: #4 CrateB, #5 Lead, #6 Tail, #7 Crate4.
        let hash = g.world_hash().unwrap();
        call(
            &mut g,
            json!([rename(6, "Crate4"), rename(7, "Tail"), {"destroy": {"entity": "Tail"}}]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let e = read(&mut g, "world.get", json!({"entity": 7})).unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(name_of(&mut g, 6), json!("Crate4"));
        // One undo revives #7 under its name and gives #6 its name back.
        g.apply(&Command::new(
            Source::Editor,
            100,
            "history.undo",
            json!({}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(
            (name_of(&mut g, 6), name_of(&mut g, 7)),
            (json!("Tail"), json!("Crate4"))
        );
        assert_eq!(g.world_hash().unwrap(), hash);

        // A name two entities share by then is ambiguous; `Name#id` or the id says which.
        let e = call(&mut g, json!([rename(4, "Lead"), cargo(json!("Lead"), 2)])).unwrap_err();
        assert_eq!(e.code, "request.ambiguous_ref", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/set/entity"), "{e:#?}");
        let ids: Vec<u64> = e.detail["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, [4, 5], "{e:#?}");
        call(
            &mut g,
            json!([rename(4, "Lead"), cargo(json!("Lead#4"), 2)]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(
            (cargo_of(&mut g, 4), cargo_of(&mut g, 5)),
            (json!(2), json!(5))
        );
        // A removed name is no longer there to share, or to find.
        call(
            &mut g,
            json!([{"remove": {"entity": 4, "component": "Name"}}, cargo(json!("Lead"), 8)]),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(
            (cargo_of(&mut g, 4), cargo_of(&mut g, 5)),
            (json!(2), json!(8))
        );
        let hash = g.world_hash().unwrap();
        let e = call(
            &mut g,
            json!([{"remove": {"entity": 5, "component": "Name"}}, cargo(json!("Lead"), 1)]),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/1/set/entity"), "{e:#?}");
        assert_eq!(g.world_hash().unwrap(), hash);
        // An entity destroyed earlier in the call is still refused as destroyed, by the name an
        // op before gave it and by the name it had.
        for (first, name) in [(rename(6, "Gone"), "Gone"), (rename(5, "Head"), "Tail")] {
            let e = call(
                &mut g,
                json!([first, {"destroy": {"entity": 6}}, cargo(json!(name), 1)]),
            )
            .unwrap_err();
            assert_eq!(e.code, "sim.entity_not_found", "{name}: {e:#?}");
            assert_eq!(e.detail["destroyed_by"], json!(1), "{name}: {e:#?}");
            assert_eq!(
                e.detail["path"],
                json!("/ops/2/set/entity"),
                "{name}: {e:#?}"
            );
            assert_eq!(g.world_hash().unwrap(), hash);
        }
        // An id is an id whatever the names.
        call(&mut g, json!([rename(5, "Head"), cargo(json!(5), 4)]))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(cargo_of(&mut g, 5), json!(4));

        g.run(3).unwrap();
        g.finish_recording().unwrap();
        // The recorded forms hold ids, so a replay looks no name up.
        let recorded = writes(&sink);
        assert!(recorded.iter().all(|w| w.name == "world_edit"));
        let take: Value = serde_json::from_str(&recorded[2].params).unwrap();
        assert_eq!(
            take["edits"][1],
            json!({"op": "set", "entity": 3, "component": "Crew", "value": {"take": 5}}),
            "{take}"
        );
        assert_eq!(verified(&sink), ReplayOutcome::Identical);
    });
}

/// The registry holds `Name` and the state the engine keeps itself (a seat's `Controls`, what an
/// observer remembers), so nothing may answer "there is no component X; did you mean X?": every
/// registered component can be named in a `remove` or is refused for what it is, and `Name` among
/// a spawn's or a scene's `components` is refused as the entity's `name`.
#[test]
fn no_registered_component_is_called_unknown() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let schema = read(&mut g, "world.schema", json!({})).unwrap();
        let names: Vec<String> = schema
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_owned())
            .collect();
        let spawned = ops(&mut g, 1, json!([{"spawn": {"name": "Scratch"}}])).unwrap();
        let scratch = spawned["spawned"][0].as_u64().unwrap();
        let mut seq = 1;
        let mut refused = |g: &mut Game, op: Value| {
            seq += 1;
            let hash = g.world_hash().unwrap();
            let e = ops(g, seq, json!([op])).unwrap_err();
            assert_eq!(g.world_hash().unwrap(), hash, "{e:#?}");
            e
        };
        // Every component the registry lists: removed, or refused as state the engine keeps.
        let mut kept = Vec::new();
        for name in &names {
            let remove = json!({"remove": {"entity": scratch, "component": name}});
            if ops(&mut g, 500, json!([remove.clone()])).is_ok() {
                continue;
            }
            let e = refused(&mut g, remove);
            assert_eq!(e.code, "request.invalid_value", "{name}: {e:#?}");
            assert_eq!(
                e.detail["path"],
                json!("ops[0].remove.component"),
                "{name}: {e:#?}"
            );
            assert!(
                e.message
                    .contains(&format!("{name} is state the engine keeps")),
                "{e:#?}"
            );
            kept.push(name.as_str());
        }
        assert!(names.iter().any(|n| n == "Name"), "{names:?}");
        assert!(
            kept.contains(&"Controls") && !kept.contains(&"Name"),
            "{kept:?}"
        );
        // The same for a `set`, of fields or of a string, and among a spawn's components.
        for (op, path) in [
            (
                json!({"set": {"entity": scratch, "component": "Controls", "value": {}}}),
                "ops[0].set.component",
            ),
            (
                json!({"set": {"entity": scratch, "component": "Controls", "value": "text"}}),
                "ops[0].set.component",
            ),
            (
                json!({"spawn": {"components": {"Controls": {}}}}),
                "ops[0].spawn.components.Controls",
            ),
        ] {
            let e = refused(&mut g, op);
            assert_eq!(e.code, "request.invalid_value", "{e:#?}");
            assert_eq!(e.detail["path"], json!(path), "{e:#?}");
        }
        // A name is `name`, beside `components`: said so, not answered "no component 'Name'; did
        // you mean Name?".
        let e = refused(
            &mut g,
            json!({"spawn": {"name": "Box", "components": {"Name": "Box2"}}}),
        );
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("ops[0].spawn.components.Name"),
            "{e:#?}"
        );
        assert!(e.message.contains("`name`"), "{e:#?}");
        let mut setup = (*common::sailing()).clone();
        let mut scene: Value = serde_json::to_value(&setup.scene).unwrap();
        scene["entities"][3]["components"]["Name"] = json!("Box2");
        setup.scene = Scene::from_json(&scene.to_string()).unwrap();
        let e = Game::new(Arc::new(setup), 1).err().expect("refused");
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(
            e.detail["path"],
            json!("entities[3].components.Name"),
            "{e:#?}"
        );
        // What the registry does not hold is unknown, and never suggests itself.
        for unknown in ["name", "Nam", "Cargoo", "Transfrom", "Nothing"] {
            for op in [
                json!({"remove": {"entity": scratch, "component": unknown}}),
                json!({"set": {"entity": scratch, "component": unknown, "value": {}}}),
                json!({"set": {"entity": scratch, "component": unknown, "value": "text"}}),
                json!({"spawn": {"components": {unknown: {}}}}),
            ] {
                let e = refused(&mut g, op);
                assert_eq!(e.code, "sim.component_unknown", "{unknown}: {e:#?}");
                assert_eq!(e.detail["component"], json!(unknown), "{e:#?}");
                let suggested = e.detail["suggestions"].as_array().unwrap();
                assert!(!suggested.contains(&json!(unknown)), "{e:#?}");
            }
        }
    });
}
