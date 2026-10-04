//! The game built from the sailing sample (architecture.md 4.8; threads.md 5): the scene and the
//! scripts load and sail, commands apply at boundaries and are recorded in their canonical form, a
//! refused command changes nothing and says why, forks match their source, the crew's rule takes a
//! crate aboard, and a reload is a recorded Host swap.
#![cfg(feature = "transpile")]
mod common;

use pocket_link::Source;
use pocket_persist::replay::{MemorySink, Record, RecordOptions, Replay};
use pocket_runtime::{Command, Game};
use serde_json::{Value, json};

fn cmd(source: Source, seq: u64, name: &str, params: Value) -> Command {
    Command::new(source, seq, name, params)
}

fn get(g: &mut Game, entity: &str, component: &str) -> Value {
    let r = g
        .apply(&cmd(
            Source::Developer(0),
            1,
            "world_get",
            json!({"entity": entity, "components": [component]}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
    r["components"][component].clone()
}

fn set(entity: &str, component: &str, value: Value) -> Value {
    json!({"edits": [{"op": "set", "entity": entity, "component": component, "value": value}]})
}

/// Charter 10, slice 1: the headless sailing scene runs; master's `sailboat` check holds on it
/// (five units along +x in 300 ticks before the wind, afloat), and its rules keep their state in
/// components.
#[test]
fn the_sailing_scene_sails() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let x0 = get(&mut g, "Sloop", "Transform")["position"][0]
            .as_f64()
            .unwrap();
        let mut events = Vec::new();
        for t in 1..=300 {
            let r = g.step().unwrap_or_else(|e| panic!("tick {t}: {e:#?}"));
            assert!(r.errors.is_empty(), "tick {t}: {:#?}", r.errors);
            events.extend(r.events.iter().map(|e| e.kind.as_str().to_owned()));
        }
        let x1 = get(&mut g, "Sloop", "Transform")["position"][0]
            .as_f64()
            .unwrap();
        let boat = get(&mut g, "Sloop", "Boat");
        assert!(x1 - x0 >= 5.0, "{x0} -> {x1}");
        assert_eq!(boat["afloat"], json!(true));
        let log = get(&mut g, "Sloop", "Log");
        assert!(log["distance"].as_f64().unwrap() > 5.0, "{log}");
        assert_eq!(log["sail_set"], json!(true));
        assert_eq!(get(&mut g, "Sloop", "Tally")["total"], json!(4));
        assert!(events.contains(&"sail.set".to_owned()), "{events:?}");
    });
}

#[test]
fn writes_are_recorded_with_names_resolved_and_refusals_change_nothing() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.step().unwrap();
        let before = g.world_hash().unwrap();
        // A misspelt component: refused with the nearest name, nothing applied.
        let e = g
            .apply(&cmd(
                Source::Player(0),
                1,
                "world_edit",
                set("Sloop", "Bot", json!({"rudder": 0.5})),
            ))
            .unwrap_err();
        assert_eq!(e.code, "sim.component_unknown", "{e:#?}");
        assert!(
            e.detail["suggestions"].to_string().contains("Boat"),
            "{e:#?}"
        );
        // A misspelt field of an engine component.
        let e = g
            .apply(&cmd(
                Source::Player(0),
                2,
                "world_edit",
                set("Sloop", "Boat", json!({"ruder": 0.5})),
            ))
            .unwrap_err();
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        // A misspelt field of a project component.
        let e = g
            .apply(&cmd(
                Source::Player(0),
                3,
                "world_edit",
                set("Sloop", "Crew", json!({"tak": "Crate1"})),
            ))
            .unwrap_err();
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        // An unknown command.
        let e = g
            .apply(&cmd(Source::Player(0), 4, "world_edits", json!({})))
            .unwrap_err();
        assert_eq!(e.code, "request.unknown_method");
        // An unknown entity.
        let e = g
            .apply(&cmd(
                Source::Player(0),
                5,
                "world_edit",
                set("Slop", "Boat", json!({"rudder": 0.5})),
            ))
            .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found");
        assert_eq!(g.world_hash().unwrap(), before);
        assert_eq!(g.writes(), 0);
        // A Write that succeeds: recorded with the name resolved to the Sloop's id.
        g.apply(&cmd(
            Source::Player(0),
            6,
            "world_edit",
            set("Sloop", "Crew", json!({"take": "Crate1"})),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(g.writes(), 1);
        assert_ne!(g.world_hash().unwrap(), before);
        // Host writes are the runtime's own.
        let e = g
            .apply(&cmd(
                Source::Player(0),
                7,
                "scripts.swap",
                json!({"bundle": g.bundle().to_hex()}),
            ))
            .unwrap_err();
        assert_eq!(e.code, "command.host_only");
        g.step().unwrap();
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let writes: Vec<_> = replay
            .records()
            .iter()
            .filter_map(|r| match r {
                Record::Tick { tick, writes, .. } if !writes.is_empty() => {
                    Some((tick.0, writes.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(writes.len(), 1, "{writes:#?}");
        let (tick, w) = &writes[0];
        assert_eq!(*tick, 2);
        assert_eq!(w[0].source, Source::Player(0));
        assert_eq!(w[0].seq, 6);
        let p = w[0].params_json();
        // Every entity named by name is recorded by its id: the Sloop is 3, the crate 4.
        assert_eq!(p["edits"][0]["entity"], json!(3), "{p}");
        assert_eq!(p["edits"][0]["value"]["take"], json!(4), "{p}");
        // The four refused Writes are kept as notes; an unknown command and a Host write from a
        // player are refused before they are Writes of this world.
        let refused = replay
            .records()
            .iter()
            .filter(|r| matches!(r, Record::Refused { .. }))
            .count();
        assert_eq!(refused, 4);
    });
}

#[test]
fn a_fork_matches_its_source_and_stays_apart() {
    common::big_stack(|| {
        let mut a = Game::new(common::sailing(), 7).unwrap();
        a.run(45).unwrap();
        let mut b = a.fork().unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(a.world_hash().unwrap(), b.world_hash().unwrap());
        assert_eq!(a.tick(), b.tick());
        for _ in 0..60 {
            a.step().unwrap();
            b.step().unwrap();
            assert_eq!(
                a.world_hash().unwrap(),
                b.world_hash().unwrap(),
                "tick {}",
                a.tick().0
            );
        }
        let mut c = a.fork().unwrap();
        c.apply(&cmd(
            Source::Player(0),
            1,
            "world_edit",
            set("Sloop", "Boat", json!({"rudder": 1.0})),
        ))
        .unwrap();
        let mut d = Game::new(common::sailing(), 7).unwrap();
        d.run(105).unwrap();
        for _ in 0..30 {
            a.step().unwrap();
            c.step().unwrap();
            d.step().unwrap();
            assert_eq!(a.world_hash().unwrap(), d.world_hash().unwrap());
        }
        assert_ne!(a.world_hash().unwrap(), c.world_hash().unwrap());
    });
}

#[test]
fn the_crew_takes_a_crate_aboard_within_reach() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        g.step().unwrap();
        // Out of reach at the start: the crew says why.
        g.apply(&cmd(
            Source::Player(0),
            1,
            "world_edit",
            set("Sloop", "Crew", json!({"take": "Crate3"})),
        ))
        .unwrap();
        let r = g.step().unwrap();
        let ignored = r
            .events
            .iter()
            .find(|e| e.kind.as_str() == "interact.ignored")
            .expect("a refusal");
        assert!(
            format!("{:?}", ignored.data).contains("sail.out_of_reach"),
            "{:?}",
            ignored.data
        );
        // Brought alongside, it comes aboard.
        let at = get(&mut g, "Crate1", "Transform")["position"].clone();
        let alongside = json!([at[0].as_f64().unwrap(), 0.0, at[2].as_f64().unwrap() - 1.5]);
        g.apply(&cmd(
            Source::Developer(0),
            1,
            "world_edit",
            json!({"edits": [
                {"op": "set", "entity": "Sloop", "component": "Transform", "value": {"position": alongside}},
                {"op": "set", "entity": "Sloop", "component": "Crew", "value": {"take": "Crate1"}}
            ]}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let r = g.step().unwrap();
        assert!(
            r.events.iter().any(|e| e.kind.as_str() == "crate.taken"),
            "{:?}",
            r.events
        );
        let tally = get(&mut g, "Sloop", "Tally");
        assert_eq!(
            (tally["taken"].clone(), tally["worth"].clone()),
            (json!(1), json!(1))
        );
        assert_eq!(get(&mut g, "Sloop", "Crew")["take"], Value::Null);
        let e = g
            .apply(&cmd(
                Source::Developer(0),
                2,
                "world_get",
                json!({"entity": "Crate1"}),
            ))
            .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found");
    });
}

#[test]
fn a_reload_is_a_recorded_host_swap_and_an_unforced_one_is_nothing() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.run(3).unwrap();
        let r = g
            .apply(&cmd(Source::Developer(0), 1, "scripts.apply", json!({})))
            .unwrap();
        assert_eq!(r["outcome"], json!("unchanged"));
        assert_eq!(g.writes(), 0);
        let r = g
            .apply(&cmd(
                Source::Developer(0),
                2,
                "scripts.apply",
                json!({"force": true}),
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(r["outcome"], json!("applied"), "{r}");
        assert_eq!(r["applied_at"], json!(4));
        assert_eq!(g.writes(), 1);
        g.run(2).unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let swap = replay.records().iter().find_map(|r| match r {
            Record::Tick { tick, writes, .. } => writes
                .iter()
                .find(|w| w.name == "scripts.swap")
                .map(|w| (tick.0, w.source)),
            _ => None,
        });
        assert_eq!(swap, Some((4, Source::Host)));
        // `step` through the catalog runs ticks.
        let r = g
            .apply(&cmd(Source::Developer(0), 3, "step", json!({"ticks": 2})))
            .unwrap();
        assert_eq!(r["tick"], json!(7));
    });
}

/// Spawns, removals and destroys are recorded in a form their replay reproduces tick for tick.
#[test]
fn every_kind_of_edit_replays() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 3).unwrap();
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.run(2).unwrap();
        let r = g
            .apply(&cmd(
                Source::Developer(0),
                1,
                "world_edit",
                json!({"edits": [
                    {"op": "spawn", "name": "Buoy", "prefab": {"kind": "crate", "position": [3, 0, 3]},
                     "components": {"Cargo": {"value": 5}}},
                    {"op": "spawn", "name": "Marker",
                     "components": {"Transform": {"position": [1, 2, 3]}}}
                ]}),
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(r["results"][0]["op"], json!("spawned"));
        g.run(3).unwrap();
        g.apply(&cmd(
            Source::Developer(0),
            2,
            "world_edit",
            json!({"edits": [
                {"op": "remove", "entity": "Buoy", "component": "Cargo"},
                {"op": "destroy", "entity": "Crate4"},
                {"op": "set", "entity": "Sloop", "component": "Crew", "value": {"take": "Buoy"}}
            ]}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
        g.run(5).unwrap();
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let outcome = {
            let mut fresh = Game::for_replay(&replay).unwrap();
            pocket_persist::replay::verify(
                &replay,
                &mut fresh,
                pocket_persist::replay::VerifyOptions::mode(
                    pocket_persist::replay::ReplayMode::Verify,
                ),
            )
            .outcome
        };
        assert_eq!(outcome, pocket_persist::replay::ReplayOutcome::Identical);
    });
}
