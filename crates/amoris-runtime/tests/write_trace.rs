//! The write trace (docs/spec/debugger.md 13; docs/bench/agent-debug.md, finding 5): a
//! `time.step {trace}` names, tick by tick, every write that changed a field without stopping the
//! game: a script system with the statement that wrote (the debugger's test project), an engine
//! system that `debug.watch` cannot see (`interface.intents` steering for `sail_to` on the sailing
//! course) beside a script helm, and the boundary's recorded writes; the world hash is the same
//! with the trace on and off, also under the game thread's profiler; a statement that stores a
//! helper's result is named rather than the helper's `return`, and a row of many statements keeps
//! the last; a component scripts cannot read leaves the script host's recorder off; and a misspelt
//! entity, component or field is refused with suggestions.
#![cfg(feature = "transpile")]
mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};

fn load(dir: &Path) -> Arc<GameSetup> {
    let p = Project::load(dir).unwrap_or_else(|e| panic!("{e:#?}"));
    Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")))
}

fn debugme_dir() -> PathBuf {
    common::repo().join("crates/amoris-debug/tests/fixtures/debugme")
}

fn course_dir() -> PathBuf {
    common::repo().join("samples/sailing-course")
}

fn debugme() -> Arc<GameSetup> {
    static SETUP: OnceLock<Arc<GameSetup>> = OnceLock::new();
    SETUP.get_or_init(|| load(&debugme_dir())).clone()
}

fn course() -> Arc<GameSetup> {
    static SETUP: OnceLock<Arc<GameSetup>> = OnceLock::new();
    SETUP.get_or_init(|| load(&course_dir())).clone()
}

/// The 1-based line of `text` holding `MARK <name>`.
fn mark(text: &str, name: &str) -> usize {
    let tag = format!("MARK {name}");
    text.lines()
        .position(|l| l.trim_end().ends_with(&tag))
        .unwrap_or_else(|| panic!("no {tag}"))
        + 1
}

/// A script helm added to the sailing course: it puts the helm hard over every 20th tick, on top
/// of whatever the engine's intents steer.
const HELM: &str = r#"import { system } from "amoris";

export const helm = system({
    name: "helm", phase: "update", doc: "Puts the helm over every 20th tick.",
    queries: { boats: { with: ["Boat"], fields: ["Boat.rudder"] } },
    run(ctx, { boats }) {
        if (ctx.tick % 20 !== 0) return;
        const b = boats.cols.Boat;
        boats.each((r) => {
            b.rudder[r] = -0.25; // MARK helm-write
        });
    },
});
"#;

/// A helm whose writes test the statement attribution: at tick 2 it stores a helper's result
/// (seen after the helper's `return`), at tick 3 a helper writes as its last statement, at tick 4
/// eleven statements change the rudder in one call.
const HELM_CALLS: &str = r#"import { system } from "amoris";

function amount(t: number): number {
    const a = -0.25 - t * 0;
    return a; // MARK helper-return
}

function steer(b: any, r: number) {
    b.rudder[r] = 0.1; // MARK steer-write
}

export const helm = system({
    name: "helm", phase: "update", doc: "Writes the rudder in three ways.",
    queries: { boats: { with: ["Boat"], fields: ["Boat.rudder"] } },
    run(ctx, { boats }) {
        const b = boats.cols.Boat;
        if (ctx.tick === 2) {
            boats.each((r) => {
                b.rudder[r] = amount(ctx.tick); // MARK call-write
            });
        } else if (ctx.tick === 3) {
            boats.each((r) => {
                steer(b, r);
                const z = 1; // MARK after-steer
            });
        } else if (ctx.tick === 4) {
            boats.each((r) => {
                for (let i = 0; i < 10; i++) {
                    b.rudder[r] = -0.01 * (i + 1); // MARK loop-write
                }
                b.rudder[r] = 0.3; // MARK final-write
            });
        }
    },
});
"#;

struct Driver {
    game: Game,
    seq: u64,
}

impl Driver {
    fn new(setup: Arc<GameSetup>) -> Driver {
        Driver {
            game: Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}")),
            seq: 0,
        }
    }

    fn call(
        &mut self,
        source: Source,
        name: &str,
        params: Value,
    ) -> Result<Value, amoris_contract::Problem> {
        self.seq += 1;
        self.game
            .apply(&Command::new(source, self.seq, name, params))
    }

    fn dev(&mut self, name: &str, params: Value) -> Value {
        self.call(Source::Developer(0), name, params)
            .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
    }

    fn hash(&self) -> String {
        self.game.world_hash().unwrap().to_string()
    }

    /// The sailing course with `helm` (a system `helm` in `scripts/helm.ts`) added to its scripts.
    fn course_with_helm(helm: &str) -> Driver {
        let mut d = Driver::new(course());
        let mut files = BTreeMap::new();
        for name in ["components.ts", "rules.ts"] {
            let text = std::fs::read_to_string(course_dir().join("scripts").join(name)).unwrap();
            files.insert(format!("scripts/{name}"), text);
        }
        let main = std::fs::read_to_string(course_dir().join("scripts/main.ts")).unwrap();
        let main = main
            .replace(
                "import { muster, rounding, takeAboard } from \"./rules\";",
                "import { muster, rounding, takeAboard } from \"./rules\";\nimport { helm } from \"./helm\";",
            )
            .replace("systems: [muster, takeAboard, rounding]", "systems: [muster, takeAboard, rounding, helm]");
        assert!(main.contains("helm]"), "{main}");
        files.insert("scripts/main.ts".into(), main);
        files.insert("scripts/helm.ts".into(), helm.into());
        let applied = d.dev("scripts.apply", json!({"files": files}));
        assert_eq!(applied["outcome"], "applied", "{applied:#}");
        d
    }
}

fn rows_of(w: &Value) -> Vec<Value> {
    w["rows"].as_array().cloned().unwrap_or_default()
}

/// The debugger's test project: `measure` raises `Gauge.level` through a query column every tick;
/// each tick's row names the system, its phase, the values and the statement that wrote.
#[test]
fn a_script_write_is_named_with_its_statement() {
    common::big_stack(|| {
        let rules = std::fs::read_to_string(debugme_dir().join("scripts/rules.ts")).unwrap();
        let line = mark(&rules, "write-level");
        let mut d = Driver::new(debugme());
        let trace = json!({"entity": "Gauge", "component": "Gauge", "field": "level"});
        let r = d.dev("time.step", json!({"ticks": 5, "trace": trace}));
        let w = &r["writes"];
        assert_eq!(w["trace"], "Gauge.Gauge.level", "{w:#}");
        assert_eq!(w["ticks"], json!([1, 5]), "{w:#}");
        assert_eq!(w["traced"], 5, "{w:#}");
        assert_eq!(w["changes"], 5, "{w:#}");
        assert_eq!(
            (w["start"].as_f64(), w["end"].as_f64()),
            (Some(0.0), Some(5.0))
        );
        let rows = rows_of(w);
        assert_eq!(rows.len(), 5, "{w:#}");
        for (i, row) in rows.iter().enumerate() {
            let tick = i as u64 + 1;
            assert_eq!(row["tick"], tick, "{row:#}");
            assert_eq!(row["by"], "script:measure", "{row:#}");
            assert_eq!(row["phase"], "Update", "{row:#}");
            assert_eq!(row["before"].as_f64(), Some(tick as f64 - 1.0), "{row:#}");
            assert_eq!(row["after"].as_f64(), Some(tick as f64), "{row:#}");
            let at = row["at"].as_str().unwrap();
            assert!(
                at.starts_with(&format!("scripts/rules.ts:{line}:")),
                "{at} is not rules.ts:{line}"
            );
        }
        // The whole component: the same writer; `peaks` changes on multiples of five too.
        let trace = json!({"entity": "Gauge", "component": "Gauge"});
        let r = d.dev("time.step", json!({"ticks": 5, "trace": trace}));
        let rows = rows_of(&r["writes"]);
        assert_eq!(rows.len(), 5, "{r:#}");
        assert_eq!(
            rows[4]["after"],
            json!({"level": 10.0, "peaks": 2}),
            "{r:#}"
        );
        assert!(
            rows.iter().all(|r| r["by"] == "script:measure"),
            "{rows:#?}"
        );
    });
}

/// The trace changes nothing: the hash after traced steps, and after untraced ones that follow
/// them (the program instrumented for the trace and reloaded after it), is the untraced run's.
#[test]
fn the_hash_is_the_same_with_the_trace_on() {
    common::big_stack(|| {
        for (setup, trace) in [
            (
                debugme(),
                json!({"entity": "Gauge", "component": "Gauge", "field": "level"}),
            ),
            (
                course(),
                json!({"entity": "Sloop", "component": "Boat", "field": "rudder"}),
            ),
        ] {
            let mut plain = Driver::new(setup.clone());
            let mut traced = Driver::new(setup);
            for ticks in [3, 1, 7] {
                let a = plain.dev("time.step", json!({"ticks": ticks}));
                let b = traced.dev("time.step", json!({"ticks": ticks, "trace": trace}));
                assert_eq!(a["world_hash"], b["world_hash"], "{b:#}");
                assert!(b["writes"].is_object(), "{b:#}");
                let a = plain.dev("time.step", json!({"ticks": 2}));
                let b = traced.dev("time.step", json!({"ticks": 2}));
                assert_eq!(a["world_hash"], b["world_hash"]);
            }
            assert_eq!(plain.hash(), traced.hash());
        }
    });
}

/// The sailing course with a script helm: `sail_to` steers through `interface.intents` (an engine
/// system in the Control phase, which `debug.watch` cannot see) every tick, the helm puts the
/// rudder over every 20th tick (with its statement), and the boundary's `player.act` is listed
/// with the tick it was an input of.
#[test]
fn engine_and_script_writers_of_the_rudder_are_named() {
    common::big_stack(|| {
        let mut d = Driver::course_with_helm(HELM);
        d.call(
            Source::Player(0),
            "player.act",
            json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                                 "params": {"arrive_m": 8}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let trace = json!({"entity": "Sloop", "component": "Boat", "field": "rudder"});
        let r = d.dev("time.step", json!({"ticks": 40, "trace": trace}));
        let w = &r["writes"];
        let rows = rows_of(w);
        let by = |who: &str| -> Vec<&Value> { rows.iter().filter(|r| r["by"] == who).collect() };
        // The boundary before tick 1: the swap and the player's act, as recorded.
        let boundary = by("boundary");
        assert_eq!(boundary.len(), 1, "{w:#}");
        assert_eq!(boundary[0]["tick"], 1, "{w:#}");
        let names: Vec<&str> = boundary[0]["writes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x["name"].as_str())
            .collect();
        assert_eq!(names, ["scripts.swap", "player.act"], "{w:#}");
        assert_eq!(boundary[0]["writes"][1]["source"], "player:0", "{w:#}");
        // sail_to's steering, in the Control phase, without a statement.
        let intents = by("interface.intents");
        assert!(intents.len() >= 10, "{w:#}");
        assert!(
            intents
                .iter()
                .all(|r| r["phase"] == "Control" && r["at"].is_null()),
            "{w:#}"
        );
        // The helm at ticks 20 and 40, at its statement.
        let line = mark(HELM, "helm-write");
        let helm = by("script:helm");
        let ticks: Vec<u64> = helm.iter().filter_map(|r| r["tick"].as_u64()).collect();
        assert_eq!(ticks, [20, 40], "{w:#}");
        for r in &helm {
            assert_eq!(r["phase"], "Update", "{r:#}");
            assert_eq!(r["after"].as_f64(), Some(-0.25), "{r:#}");
            let at = r["at"].as_str().unwrap();
            assert!(at.starts_with(&format!("scripts/helm.ts:{line}:")), "{at}");
        }
        // Nothing else wrote it, and the end is the last writer's value.
        assert_eq!(
            rows.len(),
            boundary.len() + intents.len() + helm.len(),
            "{w:#}"
        );
        assert_eq!(rows.last().unwrap()["after"], w["end"], "{w:#}");
        // A write at the boundary before the trace was asked is listed at its first tick; it
        // changed the field before `start` was read, so it has no values of its own.
        d.dev(
            "world.edit",
            json!({"ops": [{"set": {"entity": "Sloop", "component": "Boat",
                                    "value": {"rudder": 0.5}}}]}),
        );
        let r = d.dev("time.step", json!({"ticks": 1, "trace": trace}));
        let first = &r["writes"]["rows"][0];
        assert_eq!(first["by"], "boundary", "{r:#}");
        assert!(first["before"].is_null(), "{r:#}");
        assert_eq!(first["writes"][0]["name"], "world_edit", "{r:#}");
        assert_eq!(r["writes"]["start"].as_f64(), Some(0.5), "{r:#}");
    });
}

/// Where a script's write is named: a statement that stores a helper's result is the writer, not
/// the helper's `return` (which the row names as `via`); a helper that writes as its last
/// statement is the writer itself; and a row whose call changed the field eleven times lists the
/// first seven statements and the last, counts the rest, and is `at` the last.
#[test]
fn script_writes_are_named_at_the_statement_that_wrote() {
    common::big_stack(|| {
        let mut d = Driver::course_with_helm(HELM_CALLS);
        let trace = json!({"entity": "Sloop", "component": "Boat", "field": "rudder"});
        let r = d.dev("time.step", json!({"ticks": 4, "trace": trace}));
        let w = &r["writes"];
        let rows: Vec<Value> = rows_of(w)
            .into_iter()
            .filter(|r| r["by"] == "script:helm")
            .collect();
        let ticks: Vec<u64> = rows.iter().filter_map(|r| r["tick"].as_u64()).collect();
        assert_eq!(ticks, [2, 3, 4], "{w:#}");
        let at = |line: usize| format!("scripts/helm.ts:{line}:");
        let starts = |v: &Value, line: usize| v.as_str().is_some_and(|a| a.starts_with(&at(line)));
        // Tick 2: `b.rudder[r] = amount(ctx.tick)`.
        assert!(
            starts(&rows[0]["at"], mark(HELM_CALLS, "call-write")),
            "{:#}",
            rows[0]
        );
        assert!(
            starts(&rows[0]["via"], mark(HELM_CALLS, "helper-return")),
            "{:#}",
            rows[0]
        );
        assert_eq!(rows[0]["after"].as_f64(), Some(-0.25), "{:#}", rows[0]);
        // Tick 3: `steer` writes as its last statement, seen at the caller's next one.
        assert!(
            starts(&rows[1]["at"], mark(HELM_CALLS, "steer-write")),
            "{:#}",
            rows[1]
        );
        assert!(rows[1].get("via").is_none(), "{:#}", rows[1]);
        // Tick 4: ten writes in a loop, then the last one.
        let row = &rows[2];
        assert_eq!(row["after"].as_f64(), Some(0.3), "{row:#}");
        assert!(
            starts(&row["at"], mark(HELM_CALLS, "final-write")),
            "{row:#}"
        );
        let statements = row["statements"].as_array().unwrap();
        assert_eq!(statements.len(), 8, "{row:#}");
        assert_eq!(row["statements_more"], 3, "{row:#}");
        let loop_line = mark(HELM_CALLS, "loop-write");
        assert!(
            statements[..7].iter().all(|s| starts(&s["at"], loop_line)),
            "{row:#}"
        );
        assert_eq!(statements[6]["value"].as_f64(), Some(-0.07), "{row:#}");
        assert_eq!(statements[7]["value"].as_f64(), Some(0.3), "{row:#}");
        assert!(
            starts(&statements[7]["at"], mark(HELM_CALLS, "final-write")),
            "{row:#}"
        );
    });
}

/// A component scripts cannot read (`Camera`) is traced through the world alone: the script host's
/// recorder stays off (a probe of it would fail at every statement), while a field scripts read
/// arms it for the trace's ticks only.
#[test]
fn the_recorder_runs_only_for_fields_scripts_can_read() {
    use amoris_runtime::writes::{Trace, WriteTrace};
    common::big_stack(|| {
        let mut d = Driver::new(common::sailing());
        let trace = |component: &str, field: Option<&str>| Trace {
            entity: amoris_runtime::EntityRef::Name("Sloop".into()),
            component: component.into(),
            field: field.map(str::to_owned),
        };
        for (t, scripted) in [
            (trace("Camera", None), false),
            (trace("Boat", Some("rudder")), true),
            (trace("Log", Some("distance")), true),
        ] {
            let mut w = WriteTrace::new(&d.game, &t).unwrap_or_else(|e| panic!("{e:#?}"));
            for _ in 0..3 {
                w.step(&mut d.game, &mut amoris_sim::NoHooks).unwrap();
                assert_eq!(
                    d.game.recording_script_writes(),
                    scripted,
                    "{}",
                    t.component
                );
            }
            let answer = w.finish(&mut d.game);
            assert_eq!(answer["traced"], 3, "{answer:#}");
            assert!(!d.game.recording_script_writes());
        }
        // A failed hash after the ticks of a direct `time.step` cannot leave it armed either: the
        // trace is answered first.
        let r = d.dev(
            "time.step",
            json!({"ticks": 2, "trace": {"entity": "Sloop", "component": "Boat"}}),
        );
        assert!(r["writes"].is_object(), "{r:#}");
        assert!(!d.game.recording_script_writes());
    });
}

/// On the game thread a write that lands between two ticks of a traced step (here an input of
/// tick 5, held for its boundary) is a boundary change of that tick, with the write.
#[cfg(feature = "thread")]
#[test]
// The test's own clock stands outside every tick, as a presenter's does.
#[allow(clippy::disallowed_methods)]
fn a_boundary_write_inside_the_step_is_its_change() {
    use amoris_runtime::thread::{GameThread, ThreadOptions};
    use amoris_sim::Tick;
    let start = std::time::Instant::now();
    let clock: amoris_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = GameThread::spawn(
        || Game::new(common::sailing(), 1),
        ThreadOptions::new(clock),
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let mut dev = h.developer();
    let mut player = h.client(Source::Player(0)).unwrap();
    player
        .send(
            "world_edit",
            json!({"edits": [{"op": "set", "entity": "Sloop", "component": "Boat",
                              "value": {"rudder": 0.5}}]}),
            Some(Tick(5)),
            |_| {},
        )
        .unwrap();
    let trace = json!({"entity": "Sloop", "component": "Boat", "field": "rudder"});
    let r = dev
        .call("time.step", json!({"ticks": 10, "trace": trace}))
        .unwrap_or_else(|e| panic!("{e:#?}"))
        .into_json();
    let w = &r["writes"];
    assert_eq!(w["ticks"], json!([1, 10]), "{w:#}");
    let rows = rows_of(w);
    assert_eq!(rows.len(), 1, "{w:#}");
    assert_eq!(rows[0]["tick"], 5, "{w:#}");
    assert_eq!(rows[0]["by"], "boundary", "{w:#}");
    assert_eq!(rows[0]["before"].as_f64(), Some(0.0), "{w:#}");
    assert_eq!(rows[0]["after"].as_f64(), Some(0.5), "{w:#}");
    assert_eq!(rows[0]["writes"][0]["source"], "player:0", "{w:#}");
    drop((dev, player));
    h.shutdown(2000).unwrap();
}

/// On the game thread while a host serves the profile stream (every tick profiled): traced steps
/// name a script writer with its statement and engine writers, every tick is still profiled, and
/// the world hash is a plain run's.
#[cfg(feature = "thread")]
#[test]
#[allow(clippy::disallowed_methods)] // the test's own clock stands outside every tick
fn the_trace_runs_under_the_profiler() {
    use amoris_runtime::thread::{GameThread, ThreadOptions};
    let start = std::time::Instant::now();
    let clock: amoris_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = GameThread::spawn(
        || Game::new(common::sailing(), 1),
        ThreadOptions::new(clock),
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let reader = h.reader();
    reader.serve_profile();
    let mut next = 1;
    let mut dev = h.developer();
    let mut step = |params: Value| {
        dev.call("time.step", params)
            .unwrap_or_else(|e| panic!("{e:#?}"))
            .into_json()
    };
    step(json!({"ticks": 60}));
    let r = step(
        json!({"ticks": 10, "trace": {"entity": "Sloop", "component": "Log",
                                                "field": "distance"}}),
    );
    let rows = rows_of(&r["writes"]);
    assert!(!rows.is_empty(), "{r:#}");
    for row in &rows {
        assert_eq!(row["by"], "script:log", "{row:#}");
        assert!(
            row["at"]
                .as_str()
                .is_some_and(|a| a.starts_with("scripts/rules.ts:")),
            "{row:#}"
        );
    }
    let r = step(
        json!({"ticks": 10, "trace": {"entity": "Sloop", "component": "Boat",
                                                "field": "speed"}}),
    );
    let rows = rows_of(&r["writes"]);
    assert!(
        rows.iter().any(|row| row["by"]
            .as_str()
            .is_some_and(|b| !b.starts_with("script:") && b != "boundary")),
        "an engine writer of the speed: {r:#}"
    );
    let p = reader.profile_since(&mut next);
    assert_eq!(p["ticks"], 80, "{p}");
    let names: Vec<&str> = p["systems"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert!(names.contains(&"script:log"), "{names:?}");
    let mut plain = Game::new(common::sailing(), 1).unwrap();
    plain.run(80).unwrap();
    assert_eq!(r["tick"], 80, "{r:#}");
    assert_eq!(
        r["world_hash"].as_str(),
        Some(plain.world_hash().unwrap().to_string().as_str())
    );
    drop(dev);
    h.shutdown(2000).unwrap();
}

/// A misspelt entity, component or field is refused with suggestions, before any tick runs.
#[test]
fn refused_typos_get_suggestions() {
    common::big_stack(|| {
        let mut d = Driver::new(course());
        let step = |d: &mut Driver, trace: Value| {
            d.call(
                Source::Developer(0),
                "time.step",
                json!({"ticks": 5, "trace": trace}),
            )
            .unwrap_err()
        };
        let e = step(
            &mut d,
            json!({"entity": "Sloop", "component": "Boot", "field": "rudder"}),
        );
        assert_eq!(e.code, "sim.component_unknown", "{e:#?}");
        assert!(
            e.detail["suggestions"].to_string().contains("Boat"),
            "{e:#?}"
        );
        assert_eq!(e.detail["path"], "/trace/component", "{e:#?}");
        let e = step(
            &mut d,
            json!({"entity": "Sloop", "component": "Boat", "field": "ruder"}),
        );
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(e.detail["suggestions"], json!(["rudder"]), "{e:#?}");
        assert_eq!(e.detail["path"], "/trace/field", "{e:#?}");
        let e = step(
            &mut d,
            json!({"entity": "Slop", "component": "Boat", "field": "rudder"}),
        );
        assert!(format!("{e:?}").contains("Sloop"), "{e:#?}");
        let e = step(
            &mut d,
            json!({"entity": "Sloop", "component": "Boat", "feild": "rudder"}),
        );
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        // An entity without the component: the field's name against the component's fields.
        let e = step(
            &mut d,
            json!({"entity": "Mark1", "component": "Boat", "field": "ruder"}),
        );
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(e.detail["suggestions"], json!(["rudder"]), "{e:#?}");
        assert_eq!(d.game.tick().0, 0);
        // Spelt right it is traced: nothing writes it, the entity never has it.
        let r = d.dev(
            "time.step",
            json!({"ticks": 2, "trace": {"entity": "Mark1", "component": "Boat",
                                         "field": "rudder"}}),
        );
        assert_eq!(r["writes"]["changes"], 0, "{r:#}");
    });
}
