//! The host protocol's commands on the game (docs/spec/server.md): `world.edit` with undo and redo
//! (a destroy comes back under its id with every component), the readers (`world.tree`,
//! `world.query`, `world.schema`), `time.step` with stop conditions, and on the game thread Play,
//! Stop and the kept snapshots.
#![cfg(all(feature = "thread", feature = "transpile"))]
#![allow(clippy::disallowed_methods)]
mod common;

use std::sync::Arc;
use std::time::Instant;

use pocket_link::Source;
use pocket_runtime::thread::{GameThread, ThreadOptions};
use pocket_runtime::{Command, Game};
use serde_json::{Value, json};

fn call(g: &mut Game, name: &str, params: Value) -> Result<Value, pocket_contract::Problem> {
    g.apply(&Command::new(Source::Editor, 1, name, params))
}

fn ok(g: &mut Game, name: &str, params: Value) -> Value {
    call(g, name, params).unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

#[test]
fn edits_undo_and_redo_exactly() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let start = g.world_hash().unwrap();
        let r = ok(
            &mut g,
            "world.edit",
            json!({"ops": [
                {"set": {"entity": "Crate1", "component": "Cargo", "value": {"value": 9}}},
                {"spawn": {"name": "Extra", "prefab": {"kind": "crate", "position": [1, 0, 1]}}},
                {"destroy": {"entity": "Crate2"}},
            ]}),
        );
        assert_eq!(r["applied"], json!(3), "{r}");
        assert!(r["label"].as_str().unwrap().contains("Cargo"), "{r}");
        let h = ok(&mut g, "history.list", json!({}));
        assert_eq!(h["undo"].as_array().unwrap().len(), 1, "{h}");
        let edited = g.world_hash().unwrap();
        assert_ne!(edited, start);
        let u = ok(&mut g, "history.undo", json!({}));
        assert_eq!(u["label"], r["label"]);
        // The spawned crate's id stays allocated, so the allocator differs; the entities and their
        // components are back as they were.
        let crate2 = ok(&mut g, "world.get", json!({"entity": "Crate2"}));
        assert_eq!(crate2["id"], json!(5), "{crate2}");
        assert_eq!(crate2["components"]["Cargo"]["value"], json!(2));
        let crate1 = ok(
            &mut g,
            "world.get",
            json!({"entity": "Crate1", "components": ["Cargo"]}),
        );
        assert_eq!(crate1["components"]["Cargo"]["value"], json!(1));
        assert!(call(&mut g, "world.get", json!({"entity": "Extra"})).is_err());
        ok(&mut g, "history.redo", json!({}));
        let crate1 = ok(
            &mut g,
            "world.get",
            json!({"entity": "Crate1", "components": ["Cargo"]}),
        );
        assert_eq!(crate1["components"]["Cargo"]["value"], json!(9));
        assert!(call(&mut g, "world.get", json!({"entity": "Crate2"})).is_err());
        let e = call(&mut g, "history.redo", json!({})).unwrap_err();
        assert_eq!(e.code, "history.empty");
        // A typo is refused with a suggestion and changes nothing.
        let e = call(
            &mut g,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Sloop", "component": "Boat", "value": {}, "replce": true}}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        // A restore clears the history.
        let snap = g.snapshot().unwrap();
        g.restore(&snap).unwrap();
        let h = ok(&mut g, "history.list", json!({}));
        assert_eq!(h, json!({"undo": [], "redo": []}));
    });
}

#[test]
fn readers_and_stop_conditions() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let tree = ok(&mut g, "world.tree", json!({"filter": "crate"}));
        assert_eq!(tree.as_array().unwrap().len(), 4, "{tree}");
        assert!(
            tree[0]["components"]
                .as_array()
                .unwrap()
                .contains(&json!("Cargo"))
        );
        let rows = ok(
            &mut g,
            "world.query",
            json!({"with": ["Boat"], "fields": ["Boat.speed", "Transform.position.x"]}),
        );
        assert_eq!(rows[0]["name"], json!("Sloop"), "{rows}");
        assert!(rows[0]["Transform.position.x"].is_number(), "{rows}");
        let s = ok(&mut g, "world.schema", json!({"component": "Cargo"}));
        assert_eq!(s["name"], json!("Cargo"));
        let e = call(&mut g, "world.schema", json!({"component": "Cargo2"})).unwrap_err();
        assert_eq!(e.code, "sim.component_unknown");
        let r = ok(
            &mut g,
            "time.step",
            json!({"ticks": 600, "until": {"event": "sail.*"}}),
        );
        assert_eq!(r["stopped_by"]["reason"], json!("event"), "{r}");
        let r = ok(
            &mut g,
            "time.step",
            json!({"ticks": 600, "watch": {"entity": "Sloop", "component": "Boat",
                                           "field": "speed", "op": ">", "value": 0.5}}),
        );
        assert_eq!(r["stopped_by"]["reason"], json!("watch"), "{r}");
        assert!(r["stopped_by"]["value"].as_f64().unwrap() > 0.5);
        let e = call(&mut g, "play.start", json!({})).unwrap_err();
        assert_eq!(e.code, "command.thread_only");
    });
}

#[test]
fn play_stop_and_kept_snapshots_on_the_thread() {
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = GameThread::spawn(
        || Game::new(common::sailing(), 1),
        ThreadOptions::new(clock),
    )
    .unwrap();
    let reader = h.reader();
    let mut dev = h.developer();
    let mut c = |n: &str, p: Value| {
        dev.call(n, p)
            .unwrap_or_else(|e| panic!("{n}: {e:#?}"))
            .into_json()
    };
    c("time.step", json!({"ticks": 130}));
    let kept = c("snapshots.list", json!({}));
    let ticks: Vec<u64> = kept["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["tick"].as_u64().unwrap())
        .collect();
    assert_eq!(ticks, [0, 60, 120]);
    let edit_hash = c("status", json!({}))["world_hash"].clone();
    let s = c("play.start", json!({"speed": 50}));
    assert_eq!(s["mode"], json!("play"), "{s}");
    assert_eq!(reader.world().mode, pocket_link::WorldMode::Play);
    std::thread::sleep(std::time::Duration::from_millis(200));
    c(
        "world.edit",
        json!({"ops": [{"destroy": {"entity": "Crate4"}}]}),
    );
    let s = c("play.stop", json!({}));
    assert_eq!(s["mode"], json!("edit"));
    assert_eq!(s["tick"], json!(130), "{s}");
    assert_eq!(s["world_hash"], edit_hash);
    assert!(
        c("world.tree", json!({"filter": "Crate4"}))
            .as_array()
            .unwrap()
            .len()
            == 1
    );
    let r = c("snapshots.restore", json!({"tick": 100}));
    assert_eq!(r["restored"], json!(60), "{r}");
    assert_eq!(r["tick"], json!(60));
    drop(dev);
    h.shutdown(2000).unwrap();
}

#[test]
fn script_types_declare_engine_and_game_components() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let r = ok(&mut g, "scripts.types", json!({"text": true}));
        let names = |k: &str| r["components"][k].as_array().unwrap().clone();
        assert!(names("engine").contains(&json!("Boat")), "{r}");
        assert!(names("engine").contains(&json!("Transform")), "{r}");
        assert_eq!(
            names("project"),
            vec![json!("Crew"), json!("Tally"), json!("Log"), json!("Cargo")]
        );
        assert!(names("unavailable").contains(&json!("Model")), "{r}");
        assert_eq!(r["project_from"], json!("scripts"), "{r}");
        // A game built from data has no project directory: the text only.
        assert_eq!(r["dir"], Value::Null);
        let components = r["text"]["components.d.ts"].as_str().unwrap();
        assert!(components.contains("declare module \"pocket\""));
        assert!(
            components.contains("readonly take: Entity | null;"),
            "{components}"
        );
        assert!(
            components.contains("readonly take: EntityColumn;"),
            "{components}"
        );
        assert!(
            components.contains("readonly position: Vec3Columns;"),
            "{components}"
        );
        let pocket = r["text"]["pocket.d.ts"].as_str().unwrap();
        assert!(pocket.starts_with("/// <reference path=\"./components.d.ts\" />"));
        assert!(pocket.contains("export declare function system<"));
    });
}
