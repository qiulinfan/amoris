//! A `world_edit` checked in order (charter 3.4; threads.md 5.4): a call refused partway changes
//! nothing and replays, two sets of one component keep both; scene entities that name each other;
//! and a recording that goes on through a restore and records one fault once.
#![cfg(feature = "transpile")]
mod common;

use std::sync::Arc;

use pocket_link::Source;
use pocket_persist::replay::{
    MemorySink, Record, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use pocket_runtime::{Command, Game, GameBuilder, Scene};
use pocket_sim::{RunCondition, TickPhase};
use serde_json::{Value, json};

fn edit(g: &mut Game, seq: u64, edits: Value) -> Result<Value, pocket_contract::Problem> {
    g.apply(&Command::new(
        Source::Player(0),
        seq,
        "world_edit",
        json!({ "edits": edits }),
    ))
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
        assert_eq!(get(&mut g, "Mate", "Crew")["take"], json!(8));
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

fn panics_at_three(sim: &mut pocket_sim::Sim) -> Result<(), pocket_contract::Problem> {
    sim.add_system(
        "test.panic",
        TickPhase::Update,
        RunCondition::Always,
        |clock: bevy_ecs::prelude::Res<pocket_sim::SimClock>| {
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
