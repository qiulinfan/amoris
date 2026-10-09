//! What the LLM debug evaluation asked of the runtime (docs/bench/debug-eval.md, "What the engine
//! should add"): field-level reads (`world.get` fields, `scripts.read` lines, `time.step`'s
//! sampler) and restores that keep the scripts applied since the snapshot, recorded as they ran.
#![cfg(all(feature = "thread", feature = "transpile"))]
#![allow(clippy::disallowed_methods)]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use pocket_link::Source;
use pocket_persist::replay::{
    MemorySink, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use pocket_runtime::thread::{GameThread, ThreadOptions};
use pocket_runtime::{Command, Game, GameBuilder};
use serde_json::{Value, json};

fn call(g: &mut Game, name: &str, params: Value) -> Result<Value, pocket_contract::Problem> {
    g.apply(&Command::new(Source::Developer(0), 1, name, params))
}

fn ok(g: &mut Game, name: &str, params: Value) -> Value {
    call(g, name, params).unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// A copy of the sailing sample under the target directory, to edit its scripts.
fn sailing_copy(name: &str) -> PathBuf {
    let from = common::repo().join("samples/sailing");
    let to = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&to);
    std::fs::create_dir_all(to.join("scripts")).unwrap();
    for f in ["project.toml", "scene.json"] {
        std::fs::copy(from.join(f), to.join(f)).unwrap();
    }
    for e in std::fs::read_dir(from.join("scripts")).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), to.join("scripts").join(e.file_name())).unwrap();
    }
    to
}

fn game_in(dir: &Path) -> Game {
    let setup = pocket_runtime::Project::load(dir)
        .and_then(|p| p.setup(false))
        .unwrap_or_else(|e| panic!("{e:#?}"));
    GameBuilder::new(Arc::new(setup))
        .project(dir)
        .build()
        .unwrap_or_else(|e| panic!("{e:#?}"))
}

/// The fix the restore tests apply: the log counts twice the distance, which changes the world
/// from the next tick on.
fn double_the_log(dir: &Path) {
    let path = dir.join("scripts/rules.ts");
    let text = std::fs::read_to_string(&path).unwrap();
    let from = "l.distance[r] + speed * ctx.dt;";
    assert!(text.contains(from));
    std::fs::write(
        &path,
        text.replacen(from, "l.distance[r] + 2 * speed * ctx.dt;", 1),
    )
    .unwrap();
}

#[test]
fn world_get_reads_fields_alone() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        g.run(30).unwrap();
        let whole = ok(&mut g, "world.get", json!({"entity": "Sloop"}));
        let r = ok(
            &mut g,
            "world.get",
            json!({"entity": "Sloop",
                   "fields": ["Boat.heading_deg", "Transform.position.y", "Cargo.value"]}),
        );
        assert_eq!(r["components"], json!({}), "fields alone: {r}");
        assert_eq!(
            r["fields"]["Boat.heading_deg"],
            whole["components"]["Boat"]["heading_deg"]
        );
        assert_eq!(
            r["fields"]["Transform.position.y"], whole["components"]["Transform"]["position"][1],
            "{r}"
        );
        assert_eq!(
            r["fields"]["Cargo.value"],
            Value::Null,
            "the Sloop has no Cargo"
        );
        // A dotted entry of `components` is a field; components and fields together.
        let r = ok(
            &mut g,
            "world.get",
            json!({"entity": "Sloop", "components": ["Crew", "Boat.speed"]}),
        );
        assert!(r["components"]["Crew"].is_object(), "{r}");
        assert!(r["components"].get("Boat").is_none(), "{r}");
        assert_eq!(
            r["fields"]["Boat.speed"],
            whole["components"]["Boat"]["speed"]
        );
        // A field the component lacks is refused with its fields and a suggestion.
        let e = call(
            &mut g,
            "world.get",
            json!({"entity": "Sloop", "fields": ["Boat.heading"]}),
        )
        .unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(e.detail["suggestions"][0], "heading_deg", "{e:#?}");
        let e = call(
            &mut g,
            "world.get",
            json!({"entity": "Sloop", "fields": ["Bote.speed"]}),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.component_unknown", "{e:#?}");
    });
}

#[test]
fn scripts_read_numbers_the_lines_asked_for() {
    common::big_stack(|| {
        let dir = sailing_copy("agent-reads-lines");
        let mut g = game_in(&dir);
        let whole = ok(&mut g, "scripts.read", json!({"path": "rules.ts"}));
        let text = whole["text"].as_str().unwrap();
        assert!(whole.get("first").is_none(), "a whole read stays as it was");
        let r = ok(
            &mut g,
            "scripts.read",
            json!({"path": "scripts/rules.ts", "lines": "26-28", "numbered": true}),
        );
        let lines: Vec<&str> = text.lines().collect();
        let want = format!("26| {}\n27| {}\n28| {}\n", lines[25], lines[26], lines[27]);
        assert_eq!(r["text"], json!(want), "{r}");
        assert_eq!(
            (r["first"].as_u64(), r["last"].as_u64()),
            (Some(26), Some(28))
        );
        assert_eq!(r["total"].as_u64(), Some(lines.len() as u64));
        let r = ok(
            &mut g,
            "scripts.read",
            json!({"path": "rules.ts", "lines": "-2"}),
        );
        assert_eq!(r["text"], json!(format!("{}\n{}\n", lines[0], lines[1])));
        let tail = ok(
            &mut g,
            "scripts.read",
            json!({"path": "rules.ts", "lines": "70-"}),
        );
        assert_eq!(tail["last"].as_u64(), Some(lines.len() as u64));
        let e = call(
            &mut g,
            "scripts.read",
            json!({"path": "rules.ts", "lines": "900"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        let e = call(
            &mut g,
            "scripts.read",
            json!({"path": "rules.ts", "lines": "a-b"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
    });
}

#[test]
fn a_step_samples_fields_as_a_table() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let r = ok(
            &mut g,
            "time.step",
            json!({"ticks": 100, "sample": {"fields": ["Sloop.Boat.speed", "3.Log.distance",
                                                       "Sloop.Transform.position.x"],
                                            "every": 30}}),
        );
        let s = &r["samples"];
        assert_eq!(
            s["columns"],
            json!([
                "tick",
                "Sloop.Boat.speed",
                "3.Log.distance",
                "Sloop.Transform.position.x"
            ])
        );
        let ticks: Vec<u64> = s["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[0].as_u64().unwrap())
            .collect();
        assert_eq!(ticks, [30, 60, 90, 100], "every 30 ticks and the last: {s}");
        let last = &s["rows"][3];
        let now = ok(
            &mut g,
            "world.get",
            json!({"entity": "Sloop", "fields": ["Boat.speed"]}),
        );
        assert_eq!(last[1], now["fields"]["Boat.speed"], "{s}");
        // Refusals name the sample's path.
        let e = call(
            &mut g,
            "time.step",
            json!({"ticks": 10, "sample": {"fields": ["Slop.Boat.speed"]}}),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        let e = call(
            &mut g,
            "time.step",
            json!({"ticks": 100000, "sample": {"fields": ["Sloop.Boat.speed"]}}),
        )
        .unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
    });
}

/// docs/spec/server.md 3.3: `snapshots.restore` keeps the scripts applied since the snapshot
/// unless asked for the snapshot's own, and says which run; on the game thread, with a sampler on
/// its steps.
#[test]
fn a_restore_keeps_the_applied_scripts() {
    let dir = sailing_copy("agent-reads-restore");
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let project = dir.clone();
    let h = GameThread::spawn(move || Ok(game_in(&project)), ThreadOptions::new(clock)).unwrap();
    let mut dev = h.developer();
    let mut c = |n: &str, p: Value| {
        dev.call(n, p)
            .unwrap_or_else(|e| panic!("{n}: {e:#?}"))
            .into_json()
    };
    let old = c("status", json!({}))["bundle"].clone();
    let r = c(
        "time.step",
        json!({"ticks": 70, "sample": {"fields": ["Sloop.Log.distance"], "every": 10}}),
    );
    assert_eq!(r["samples"]["rows"].as_array().unwrap().len(), 7, "{r}");
    double_the_log(&dir);
    let applied = c("scripts.apply", json!({}));
    assert_eq!(applied["outcome"], "applied", "{applied}");
    let new = applied["bundle"].clone();
    assert_ne!(new, old);

    // The default: the snapshot's world under the scripts applied now.
    let r = c("snapshots.restore", json!({"tick": 65}));
    assert_eq!(r["restored"], json!(60), "{r}");
    assert_eq!(r["bundle"], new, "{r}");
    assert_eq!(r["scripts"]["kept"], "applied", "{r}");
    assert_eq!(r["scripts"]["snapshot_bundle"], old, "{r}");
    assert_eq!(r["scripts"]["swapped"], true, "{r}");
    assert_eq!(c("scripts.status", json!({}))["bundle"], new);
    let at60 = c(
        "world.get",
        json!({"entity": "Sloop", "fields": ["Log.distance"]}),
    );
    c("time.step", json!({"ticks": 1}));
    let fixed = c(
        "world.get",
        json!({"entity": "Sloop", "fields": ["Log.distance"]}),
    );

    // Asked for, the snapshot's own scripts.
    let r = c(
        "snapshots.restore",
        json!({"tick": 60, "bundle": "snapshot"}),
    );
    assert_eq!(r["bundle"], old, "{r}");
    assert_eq!(r["scripts"]["kept"], "snapshot", "{r}");
    assert_eq!(
        c(
            "world.get",
            json!({"entity": "Sloop", "fields": ["Log.distance"]})
        )["fields"],
        at60["fields"]
    );
    c("time.step", json!({"ticks": 1}));
    let buggy = c(
        "world.get",
        json!({"entity": "Sloop", "fields": ["Log.distance"]}),
    );
    let d = |v: &Value| v["fields"]["Log.distance"].as_f64().unwrap();
    let base = d(&at60);
    assert!(
        (d(&fixed) - base) > 1.9 * (d(&buggy) - base) && d(&buggy) > base,
        "the applied scripts doubled the step: {} {} {}",
        base,
        d(&buggy),
        d(&fixed)
    );
    let e = dev
        .call("snapshots.restore", json!({"tick": 60, "bundle": "latest"}))
        .unwrap_err();
    assert_eq!(e.code, "request.invalid_value", "{e:#?}");
    drop(dev);
    h.shutdown(2000).unwrap();
}

/// A restore under the applied scripts is the restore and then the hot update's swap, and a
/// recording holds both: it replays identically.
#[test]
fn a_restore_under_the_applied_scripts_replays() {
    let dir = sailing_copy("agent-reads-replay");
    let outcome = common::big_stack(move || {
        let mut g = game_in(&dir);
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.run(10).unwrap();
        let snap = g.snapshot().unwrap();
        double_the_log(&dir);
        let r = ok(&mut g, "scripts.apply", json!({}));
        assert_eq!(r["outcome"], "applied", "{r}");
        g.run(10).unwrap();
        let applied = g.bundle();
        g.restore_any(&snap).unwrap();
        assert_ne!(g.bundle(), applied, "the snapshot's scripts first");
        g.swap_bundle(applied).unwrap();
        assert_eq!(g.bundle(), applied);
        g.run(10).unwrap();
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut fresh = Game::for_replay(&replay).unwrap_or_else(|e| panic!("{e:#?}"));
        verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome
    });
    assert_eq!(outcome, ReplayOutcome::Identical);
}
