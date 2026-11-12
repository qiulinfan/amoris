//! Saves through the command catalog (docs/spec/server.md 3.3; versions.md 6): `saves.write`,
//! `saves.list` and `saves.load` on a game driven directly and on the game thread. A loaded save
//! continues bit for bit (the physics cache with it), a recording that loads one replays, names
//! are checked, a missing or damaged save changes nothing, and a save holding an older version
//! of a project component waits for its migration steps.
#![cfg(all(feature = "thread", feature = "transpile"))]
#![allow(clippy::disallowed_methods)]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use amoris_link::Source;
use amoris_persist::replay::{
    MemorySink, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use amoris_runtime::thread::{GameThread, ThreadOptions};
use amoris_runtime::{Command, Game, GameBuilder, Project};
use serde_json::{Value, json};

/// A ball on the ground and a counter a rule advances: physics and a project component.
fn main_ts(version: u32) -> String {
    format!(
        r#"
import {{ component, field, game, system }} from "amoris";

export const Counter = component("Counter", {{
    version: {version}, doc: "Counts ticks.",
    fields: {{ n: field.u32(0, "Ticks counted.") }},
}});

export const count = system({{
    name: "count", phase: "update", doc: "Advances the counters.",
    queries: {{ counters: {{ with: ["Counter"] }} }},
    run(ctx, {{ counters }}) {{
        for (let r = 0; r < counters.len; r++) counters.cols.Counter.n[r] += 1;
    }},
}});

export default game({{ components: [Counter], systems: [count] }});
"#
    )
}

fn scene() -> Value {
    json!([
        {"name": "Ground", "components": {
            "Transform": {"position": [0.0, -0.5, 0.0]},
            "Collider": {"shape": {"Cuboid": {"half_extents": [10.0, 0.5, 10.0]}}}}},
        {"name": "Ball", "components": {
            "Transform": {"position": [0.0, 3.0, 0.0]}, "Velocity": {},
            "RigidBody": {"kind": "Dynamic"},
            "Collider": {"shape": {"Ball": {"radius": 0.5}}, "density": 800.0},
            "Counter": {}}},
    ])
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-saves-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = 'saves-{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), main_ts(1)).unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": scene()});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

fn build(root: &Path) -> Result<Game, amoris_contract::Problem> {
    let p = Project::load(root)?;
    let setup = Arc::new(p.setup(false)?);
    GameBuilder::new(setup).project(root).build()
}

fn try_call(g: &mut Game, name: &str, params: Value) -> Result<Value, amoris_contract::Problem> {
    g.apply(&Command::new(Source::Developer(9), 1, name, params))
}

fn call(g: &mut Game, name: &str, params: Value) -> Value {
    try_call(g, name, params).unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

fn hashes(g: &mut Game, ticks: u32) -> Vec<String> {
    (0..ticks)
        .map(|_| {
            let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            assert!(r.errors.is_empty(), "{:#?}", r.errors);
            g.world_hash().unwrap().to_string()
        })
        .collect()
}

#[test]
fn a_loaded_save_continues_as_the_saved_world_did() {
    common::big_stack(|| {
        let root = project("continue");
        let mut g = build(&root).unwrap();
        hashes(&mut g, 30);
        let w = call(
            &mut g,
            "saves.write",
            json!({"name": "slot-1", "label": "falling"}),
        );
        assert_eq!(w["tick"], json!(30), "{w}");
        assert_eq!(w["path"], json!(".amoris/saves/slot-1.save"));
        let saved = w["world_hash"].clone();
        assert_eq!(saved, json!(g.world_hash().unwrap().to_string()));
        let ahead = hashes(&mut g, 30);
        call(&mut g, "saves.write", json!({"name": "later"}));

        let list = call(&mut g, "saves.list", json!({}));
        let saves = list["saves"].as_array().unwrap();
        assert_eq!(saves.len(), 2, "{list}");
        assert_eq!(saves[0]["name"], json!("later"));
        assert_eq!(saves[1]["name"], json!("slot-1"));
        assert_eq!(saves[1]["label"], json!("falling"));
        assert_eq!(saves[1]["tick"], json!(30));
        assert_eq!(saves[1]["world_hash"], saved);
        assert!(saves[1]["bytes"].as_u64().unwrap() > 0);

        // The edit history goes with the world it was about.
        call(
            &mut g,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Ball", "component": "Counter",
                                    "value": {"n": 1000}}}]}),
        );
        let r = call(&mut g, "saves.load", json!({"name": "slot-1"}));
        assert_eq!(r["loaded"]["tick"], json!(30), "{r}");
        assert_eq!(r["loaded"]["world_hash"], saved);
        assert_eq!(r["loaded"]["name"], json!("slot-1"));
        assert_eq!(g.tick().0, 30);
        let h = call(&mut g, "history.list", json!({}));
        assert_eq!(h["undo"], json!([]));
        // The run from the save is the run that followed it, physics included.
        assert_eq!(hashes(&mut g, 30), ahead);
        // So is a new game's: a save loads into a fresh world from the same project.
        let mut fresh = build(&root).unwrap();
        call(&mut fresh, "saves.load", json!({"name": "slot-1"}));
        assert_eq!(hashes(&mut fresh, 30), ahead);
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn names_missing_and_damaged_saves_are_refused_and_change_nothing() {
    common::big_stack(|| {
        let root = project("refused");
        let mut g = build(&root).unwrap();
        hashes(&mut g, 5);
        call(&mut g, "saves.write", json!({"name": "slot-1"}));
        let before = g.world_hash().unwrap();
        for bad in ["", "../escape", "a/b", "a.b", "with space"] {
            let e = try_call(&mut g, "saves.write", json!({"name": bad})).unwrap_err();
            assert_eq!(e.code, "request.invalid_value", "{bad}: {e:#?}");
            let e = try_call(&mut g, "saves.load", json!({"name": bad})).unwrap_err();
            assert_eq!(e.code, "request.invalid_value", "{bad}: {e:#?}");
        }
        let e = try_call(&mut g, "saves.load", json!({"name": "slot-2"})).unwrap_err();
        assert_eq!(e.code, "saves.not_found", "{e:#?}");
        assert_eq!(e.detail["saves"], json!(["slot-1"]));
        assert_eq!(e.detail["suggestion"], json!("slot-1"));
        // A damaged file: its header does not read, and loading it leaves the world.
        let dir = root.join(".amoris/saves");
        std::fs::write(dir.join("junk.save"), b"not a save at all").unwrap();
        let mut cut = std::fs::read(dir.join("slot-1.save")).unwrap();
        cut.truncate(cut.len() / 2);
        std::fs::write(dir.join("cut.save"), &cut).unwrap();
        let list = call(&mut g, "saves.list", json!({}));
        assert!(list["saves"][1]["unreadable"].is_string(), "{list}");
        for name in ["junk", "cut"] {
            let e = try_call(&mut g, "saves.load", json!({"name": name})).unwrap_err();
            assert!(!e.code.is_empty());
            assert_eq!(g.world_hash().unwrap(), before, "{name}");
        }
        assert!(try_call(&mut g, "saves.write", json!({"name": "x", "slot": 1})).is_err());
        // A game built from data has no folder for saves.
        let mut data = Game::new(common::sailing(), 1).unwrap();
        let e = try_call(&mut data, "saves.write", json!({"name": "a"})).unwrap_err();
        assert_eq!(e.code, "project.no_root");
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn an_older_project_component_waits_for_its_migration() {
    common::big_stack(|| {
        let root = project("older");
        let mut g = build(&root).unwrap();
        hashes(&mut g, 5);
        call(&mut g, "saves.write", json!({"name": "v1"}));
        std::fs::write(root.join("scripts/main.ts"), main_ts(2)).unwrap();
        let mut newer = build(&root).unwrap();
        let before = newer.world_hash().unwrap();
        let e = try_call(&mut newer, "saves.load", json!({"name": "v1"})).unwrap_err();
        assert_eq!(e.code, "migrate.missing_step", "{e:#?}");
        assert_eq!(newer.world_hash().unwrap(), before);
        assert_eq!(newer.tick().0, 0);
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn a_recording_that_loads_a_save_replays() {
    common::big_stack(|| {
        let root = project("replay");
        let mut g = build(&root).unwrap();
        hashes(&mut g, 10);
        call(&mut g, "saves.write", json!({"name": "early"}));
        hashes(&mut g, 10);
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        hashes(&mut g, 10);
        call(&mut g, "saves.load", json!({"name": "early"}));
        hashes(&mut g, 20);
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut fresh = Game::for_replay(&replay).unwrap_or_else(|e| panic!("{e:#?}"));
        let out = verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome;
        assert_eq!(out, ReplayOutcome::Identical);
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn the_game_thread_loads_a_save_as_it_restores_a_snapshot() {
    let root = project("thread");
    let start = Instant::now();
    let clock: amoris_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let made = root.clone();
    let h = GameThread::spawn(move || build(&made), ThreadOptions::new(clock)).unwrap();
    let reader = h.reader();
    let mut dev = h.developer();
    let mut c = |n: &str, p: Value| {
        dev.call(n, p)
            .unwrap_or_else(|e| panic!("{n}: {e:#?}"))
            .into_json()
    };
    c("time.step", json!({"ticks": 70}));
    let saved = c("saves.write", json!({"name": "at-70"}));
    c("time.step", json!({"ticks": 60}));
    let ticks = |v: &Value| -> Vec<u64> {
        v["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["tick"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(ticks(&c("snapshots.list", json!({}))), [0, 60, 120]);
    let epoch = reader.world().epoch;
    let s = c("saves.load", json!({"name": "at-70"}));
    assert_eq!(s["tick"], json!(70), "{s}");
    assert_eq!(s["world_hash"], saved["world_hash"]);
    assert_eq!(s["loaded"]["name"], json!("at-70"));
    assert!(reader.world().epoch > epoch);
    assert_eq!(ticks(&c("snapshots.list", json!({}))), [0, 60]);
    drop(dev);
    h.shutdown(2000).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
