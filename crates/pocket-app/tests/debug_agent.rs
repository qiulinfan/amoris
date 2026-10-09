//! The agents' debugger end to end (docs/spec/debugger.md 7 and 11): the debugger's test project on
//! a real game thread in real time, driven only through `DebugHub::call` as pocket-server and
//! pocket-mcp drive it: a `debugger;` statement, pause on uncaught and on caught exceptions, a
//! TypeScript breakpoint with locals, evaluation and set variables, step into and out, a data
//! breakpoint on a component field, console lines and the detach; and Play, whose fork takes the
//! debugger.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocket_debug::{CdpOptions, DebugEvent, DebugHub, HubOptions};
use pocket_runtime::thread::{GameThread, Pacing, ThreadOptions};
use pocket_runtime::{Game, Project};
use serde_json::{Value, json};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pocket-debug/tests/fixtures/debugme")
}

/// The 1-based line of rules.ts holding `MARK <name>`.
fn mark(name: &str) -> u64 {
    let text = std::fs::read_to_string(fixture().join("scripts/rules.ts")).unwrap();
    let tag = format!("MARK {name}");
    let i = text
        .lines()
        .position(|l| l.trim_end().ends_with(&tag))
        .unwrap_or_else(|| panic!("no {tag}"));
    i as u64 + 1
}

fn call(hub: &DebugHub, method: &str, params: Value) -> Value {
    hub.call(method, &params)
        .unwrap_or_else(|p| panic!("{method}: {} {}", p.code, p.message))
}

/// Waits for the next pause and checks where it is.
fn paused(hub: &DebugHub) -> Value {
    let s = call(hub, "debug.wait", json!({"timeout_ms": 10000}));
    assert_eq!(s["state"], "paused", "{s:#}");
    s
}

fn at(s: &Value) -> (String, u64) {
    let l = &s["location"];
    (
        l["file"].as_str().unwrap_or("").to_owned(),
        l["line"].as_u64().unwrap_or(0),
    )
}

fn local<'a>(s: &'a Value, frame: usize, name: &str) -> &'a Value {
    s["frames"][frame]["locals"]
        .as_array()
        .and_then(|v| v.iter().find(|x| x["name"] == name))
        .map(|x| &x["value"])
        .unwrap_or_else(|| panic!("no local {name} in frame {frame}: {s:#}"))
}

#[test]
fn an_agent_debugs_the_test_game() {
    let project = Project::load(&fixture()).unwrap();
    let setup = Arc::new(project.setup(false).unwrap());
    let hub = DebugHub::new(HubOptions::default());
    let events = hub.subscribe();
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = hub.clone();
    let handle = GameThread::spawn(
        move || {
            let mut g = Game::new(setup, 1)?;
            g.set_script_debugger(Some(h.hook()));
            Ok(g)
        },
        ThreadOptions::new(clock),
    )
    .unwrap();
    hub.set_loop_state(handle.loop_state());
    let reader = handle.reader();
    let mut dev = handle.developer();
    const RULES: &str = "scripts/rules.ts";

    // Attached before the first tick: debugger statements and uncaught exceptions stop it.
    let s = call(&hub, "debug.exceptions", json!({"mode": "uncaught"}));
    assert_eq!(s["mode"], "uncaught");
    dev.call(
        "time_control",
        json!({"pacing": {"real_time": {"speed": 1.0}}}),
    )
    .unwrap();

    let s = paused(&hub);
    assert_eq!(s["reason"], "debugger_statement", "{s:#}");
    assert_eq!(at(&s), (RULES.to_owned(), mark("debugger")));
    assert_eq!(
        (s["tick"].as_u64(), s["system"].as_str()),
        (Some(8), Some("probe"))
    );
    assert!(s["instrumented"].as_bool().unwrap());
    // Presenters reading the status see the game thread held at a breakpoint (threads.md 3.5).
    assert_eq!(reader.status().state, pocket_link::LoopState::Breakpoint);
    call(&hub, "debug.continue", json!({}));

    // Tick 10's exception is caught in measure: no stop. Tick 12's escapes probe's run.
    let s = paused(&hub);
    assert_eq!(s["reason"], "exception", "{s:#}");
    assert_eq!(s["exception"]["caught"], false);
    assert!(
        s["exception"]["text"]
            .as_str()
            .unwrap()
            .contains("probe fails at tick 12"),
        "{s:#}"
    );
    assert_eq!(at(&s), (RULES.to_owned(), mark("uncaught-throw")));
    assert_eq!(s["tick"], 12);
    call(&hub, "debug.continue", json!({}));

    // Every exception: tick 15's, which risky throws and measure catches.
    call(&hub, "debug.exceptions", json!({"mode": "all"}));
    let s = paused(&hub);
    assert_eq!(
        (s["reason"].as_str(), s["exception"]["caught"].as_bool()),
        (Some("exception"), Some(true)),
        "{s:#}"
    );
    assert_eq!(at(&s), (RULES.to_owned(), mark("caught-throw")));
    assert_eq!(s["frames"][0]["function"], "risky");
    assert_eq!(local(&s, 0, "n"), &json!(15));
    assert_eq!(s["frames"][1]["function"], "run");
    assert_eq!(
        s["frames"][1]["location"]["line"].as_u64(),
        Some(mark("call-risky"))
    );
    call(&hub, "debug.exceptions", json!({"mode": "none"}));
    call(&hub, "debug.continue", json!({}));

    // A breakpoint on a TypeScript line, a condition, locals, evaluation.
    let bp = call(
        &hub,
        "debug.breakpoints.set",
        json!({"file": RULES, "line": mark("call-twice"), "condition": "next === 20"}),
    );
    assert_eq!(bp["line"].as_u64(), Some(mark("call-twice")), "{bp:#}");
    let s = paused(&hub);
    assert_eq!(
        (s["reason"].as_str(), at(&s)),
        (Some("breakpoint"), (RULES.to_owned(), mark("call-twice"))),
        "{s:#}"
    );
    assert_eq!(s["hit_breakpoints"][0], bp["id"]);
    assert_eq!(local(&s, 0, "next"), &json!(20));
    assert_eq!(local(&s, 0, "r"), &json!(0));
    let e = call(
        &hub,
        "debug.eval",
        json!({"expr": "next * 10 + gauges.len"}),
    );
    assert_eq!(e["value"], json!(201), "{e:#}");
    // An evaluation may not write: world edits, events and random draws refuse.
    for expr in [
        "ctx.world.set(gauges.id(0), 'Gauge', { level: 0 })",
        "ctx.rng.next()",
        "Math.random()",
    ] {
        let refused = hub
            .call("debug.eval", &json!({ "expr": expr }))
            .unwrap_err();
        assert!(
            refused.message.contains("cannot write"),
            "{expr}: {}",
            refused.message
        );
    }
    let gauge = call(&hub, "debug.eval", json!({"expr": "gauges.id(0)"}))["value"]
        .as_u64()
        .unwrap();
    let bad = hub.call("debug.eval", &json!({"exp": "1"})).unwrap_err();
    assert_eq!(bad.code, "request.unknown_field");
    // debug.set changes a frame's variable: the loop's `let r` here (an assignment evaluated with
    // debug.eval would not reach the frame); a const refuses.
    let set = call(&hub, "debug.set", json!({"name": "r", "value": "r + 7"}));
    assert_eq!(
        (set["type"].as_str(), &set["value"]),
        (Some("number"), &json!(7)),
        "{set:#}"
    );
    assert_eq!(
        call(&hub, "debug.eval", json!({"expr": "r"}))["value"],
        json!(7)
    );
    let refused = hub
        .call("debug.set", &json!({"name": "next", "value": "1"}))
        .unwrap_err();
    assert_eq!(refused.code, "debug.set_failed");
    assert!(refused.message.contains("constant"), "{}", refused.message);
    let unknown = hub
        .call("debug.set", &json!({"name": "nowhere", "value": "1"}))
        .unwrap_err();
    assert_eq!(unknown.code, "debug.set_failed", "{}", unknown.message);
    call(&hub, "debug.breakpoints.clear", json!({}));

    // Step into twice, out again, and over.
    let s = call(&hub, "debug.step", json!({"kind": "into"}));
    assert_eq!(
        (s["reason"].as_str(), at(&s)),
        (Some("step"), (RULES.to_owned(), mark("twice-body"))),
        "{s:#}"
    );
    assert_eq!(s["frames"][0]["function"], "twice");
    assert_eq!(local(&s, 0, "x"), &json!(20));
    // The caller's frame captured the r set above; set it back there, and twice's argument here:
    // the call returns twice the new x.
    assert_eq!(local(&s, 1, "r"), &json!(7), "{s:#}");
    call(
        &hub,
        "debug.set",
        json!({"name": "r", "value": "0", "frame": 1}),
    );
    call(&hub, "debug.set", json!({"name": "x", "value": "21"}));
    let s = call(&hub, "debug.step", json!({"kind": "out"}));
    assert_eq!(s["frames"][0]["function"], "run", "{s:#}");
    assert_eq!(local(&s, 0, "r"), &json!(0), "{s:#}");
    assert_eq!(local(&s, 0, "doubled"), &json!(42), "{s:#}");
    let (_, line) = at(&s);
    assert!(
        line > mark("call-twice") && line <= mark("call-risky"),
        "{s:#}"
    );
    let s = call(&hub, "debug.step", json!({"kind": "over"}));
    assert_eq!(s["reason"], "step");
    assert!(at(&s).1 >= line, "{s:#}");

    // A data breakpoint on Gauge.level: the stop after the write, which it names.
    let w = call(
        &hub,
        "debug.watch",
        json!({"entity": gauge, "component": "Gauge", "field": "level"}),
    );
    call(&hub, "debug.continue", json!({}));
    let s = paused(&hub);
    assert_eq!(s["reason"], "data_breakpoint", "{s:#}");
    assert_eq!(s["data"]["watch"], w["id"]);
    assert_eq!(
        s["data"]["written_at"]["line"].as_u64(),
        Some(mark("write-level")),
        "{s:#}"
    );
    let (before, after) = (
        s["data"]["before"][0].as_f64().unwrap(),
        s["data"]["after"][0].as_f64().unwrap(),
    );
    assert_eq!(after, before + 1.0, "{s:#}");
    call(&hub, "debug.unwatch", json!({}));
    call(&hub, "debug.continue", json!({}));

    // Conditions and evaluations ran inside ticks: the run is tainted from the first of them.
    let status = dev.call("status", json!({})).unwrap().into_json();
    let tainted = status["tainted"].as_u64().expect("a tainted tick");
    assert!((13..=20).contains(&tainted), "{status:#}");

    // A hot update while attached: the new rules.ts is a new script, instrumented, and a
    // breakpoint set on it stops at its (shifted) TypeScript line.
    let dir = fixture().join("scripts");
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
    let edited = format!("// A line added by a hot update.\n{}", read("rules.ts"));
    let applied = dev
        .call(
            "scripts.apply",
            json!({"files": {"scripts/main.ts": read("main.ts"),
                              "scripts/components.ts": read("components.ts"),
                              "scripts/rules.ts": edited}}),
        )
        .unwrap()
        .into_json();
    assert_eq!(applied["outcome"], "applied", "{applied:#}");
    let mut seen: Vec<DebugEvent> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let fresh = loop {
        if let Ok(e) = events.recv_timeout(Duration::from_millis(100)) {
            seen.push(e);
        }
        let found = seen.iter().find_map(|e| match e {
            DebugEvent::Scripts(s) => s
                .iter()
                .find(|s| s.module == RULES && s.ts.starts_with("// A line added"))
                .cloned(),
            _ => None,
        });
        if let Some(f) = found {
            break f;
        }
        assert!(
            Instant::now() < deadline,
            "the hot update announces the new rules.ts"
        );
    };
    assert!(fresh.js.contains("twice"));
    let bp = call(
        &hub,
        "debug.breakpoints.set",
        json!({"file": RULES, "line": mark("write-level") + 1}),
    );
    let s = paused(&hub);
    assert_eq!(s["hit_breakpoints"][0], bp["id"], "{s:#}");
    assert_eq!(at(&s), (RULES.to_owned(), mark("write-level") + 1));
    call(&hub, "debug.breakpoints.clear", json!({}));

    // Pause on request, then detach: the game runs on, uninstrumented.
    call(&hub, "debug.continue", json!({}));
    let s = call(&hub, "debug.pause", json!({"timeout_ms": 5000}));
    assert_eq!(s["state"], "paused", "{s:#}");
    assert_eq!(s["reason"], "pause");
    call(&hub, "debug.detach", json!({}));
    std::thread::sleep(Duration::from_millis(300));
    let s = call(&hub, "debug.state", json!({}));
    assert_eq!(
        (s["state"].as_str(), s["attached"].as_bool()),
        (Some("running"), Some(false)),
        "{s:#}"
    );
    assert_eq!(s["instrumented"], false, "{s:#}");

    // What the frontends heard: the debug events and the failed system's log line.
    seen.extend(events.try_iter());
    let heard: Vec<Value> = seen.iter().filter_map(|e| e.json()).collect();
    assert!(
        heard
            .iter()
            .any(|e| e["event"] == "debug" && e["data"]["state"] == "paused")
    );
    assert!(
        heard.iter().any(|e| e["event"] == "log"
            && e["data"]["system"] == "probe"
            && e["data"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("tick 12")),
        "{heard:#?}"
    );
    let unknown = hub
        .call(
            "debug.breakpoints.set",
            &json!({"file": "scripts/rulez.ts", "line": 3}),
        )
        .unwrap_err();
    assert_eq!(unknown.code, "debug.unknown_file");
    assert_eq!(unknown.detail["suggestions"][0], "scripts/rules.ts");
    drop(events);
    hub.shutdown();
    let _ = dev.call("time_control", json!({"pause": true}));
    drop(dev);
    handle.shutdown(5000).unwrap();
    let _ = DebugEvent::Resumed;
}

/// Play runs a fork of the edit world (docs/spec/server.md 3.3); the game thread hands the script
/// debugger to the fork and back on Stop, so a breakpoint set in Edit stops Play's scripts (the
/// editor's Play, docs/spec/editor.md 8.1) and, after Stop, a step of the edit world's.
#[test]
fn play_takes_the_debugger_to_its_fork() {
    const RULES: &str = "scripts/rules.ts";
    let project = Project::load(&fixture()).unwrap();
    let setup = Arc::new(project.setup(false).unwrap());
    let hub = DebugHub::new(HubOptions::default());
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = hub.clone();
    let handle = GameThread::spawn(
        move || {
            let mut g = Game::new(setup, 1)?;
            g.set_script_debugger(Some(h.hook()));
            Ok(g)
        },
        ThreadOptions {
            pacing: Pacing::RealTime { speed: 1.0 },
            paused: true,
            ..ThreadOptions::new(clock)
        },
    )
    .unwrap();
    hub.set_loop_state(handle.loop_state());
    let mut dev = handle.developer();

    // In Edit, before any tick: the modules are known, the breakpoint binds.
    let bp = call(
        &hub,
        "debug.breakpoints.set",
        json!({"file": RULES, "line": mark("next")}),
    );
    assert_eq!(bp["verified"], true, "{bp:#}");
    let s = call(&hub, "debug.state", json!({}));
    assert_eq!(s["state"], "running");
    assert!(s["cdp"].is_null(), "no CDP endpoint served: {s:#}");

    let st = dev.call("play.start", json!({})).unwrap().into_json();
    assert_eq!(st["mode"], "play", "{st:#}");
    let s = paused(&hub);
    assert_eq!(s["reason"], "breakpoint", "{s:#}");
    assert_eq!(at(&s), (RULES.to_owned(), mark("next")));
    assert_eq!(s["system"], "measure");
    assert_eq!(s["tick"], 1, "Play's first tick: {s:#}");
    // Setting a local's column through an evaluation writes the component the call commits.
    call(&hub, "debug.eval", json!({"expr": "g.level[r] = 100"}));
    // Detached, the fork runs on (its `debugger;` statement at tick 8 does not stop it).
    call(&hub, "debug.detach", json!({}));
    let deadline = Instant::now() + Duration::from_secs(5);
    let level = loop {
        let q = dev
            .call(
                "world.query",
                json!({"with": ["Gauge"], "fields": ["Gauge.level"]}),
            )
            .unwrap()
            .into_json();
        let level = q[0]["Gauge.level"].as_f64().unwrap_or(0.0);
        if level > 100.0 || Instant::now() > deadline {
            break level;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(level > 100.0, "the set column was committed: {level}");
    let st = dev.call("play.stop", json!({})).unwrap().into_json();
    assert_eq!(st["mode"], "edit", "{st:#}");

    // Back in Edit, the edit world has the debugger again: a step stops at a new breakpoint.
    call(
        &hub,
        "debug.breakpoints.set",
        json!({"file": RULES, "line": mark("write-level")}),
    );
    let stepper = std::thread::spawn(move || {
        let r = dev
            .call("time.step", json!({"ticks": 1}))
            .map(|r| r.into_json());
        (dev, r)
    });
    let s = paused(&hub);
    assert_eq!(at(&s), (RULES.to_owned(), mark("write-level")), "{s:#}");
    assert_eq!(s["tick"], 1, "the edit world's first tick: {s:#}");
    assert_eq!(
        local(&s, 0, "next"),
        &json!(1),
        "the edit world's gauge: {s:#}"
    );
    call(&hub, "debug.detach", json!({}));
    let (dev, stepped) = stepper.join().unwrap();
    assert_eq!(stepped.unwrap()["tick"], 1);
    drop(dev);
    hub.shutdown();
    handle.shutdown(5000).unwrap();
}

/// P11 (docs/bench/debug-eval.md, finding 3): an evaluation sees the block-scoped locals in scope
/// at the statement its frame stands at, every one declared before it in its block and the
/// enclosing ones, in the stopped frame and in a caller's; not a sibling block's. PR #1421 started
/// from the function's deepest block, which showed the evaluation's helm.ts `dx` and not `dz` or
/// `bearing` declared after it, and here would show `hidden`.
#[test]
fn eval_sees_the_block_scoped_locals_in_scope() {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pocket-debug/tests/fixtures/scopes");
    let mark_at = |name: &str| -> u64 {
        let text = std::fs::read_to_string(dir.join("scripts/rules.ts")).unwrap();
        let tag = format!("MARK {name}");
        text.lines()
            .position(|l| l.trim_end().ends_with(&tag))
            .unwrap_or_else(|| panic!("no {tag}")) as u64
            + 1
    };
    let project = Project::load(&dir).unwrap();
    let setup = Arc::new(project.setup(false).unwrap());
    let hub = DebugHub::new(HubOptions::default());
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = hub.clone();
    let handle = GameThread::spawn(
        move || {
            let mut g = Game::new(setup, 1)?;
            g.set_script_debugger(Some(h.hook()));
            Ok(g)
        },
        ThreadOptions::new(clock),
    )
    .unwrap();
    hub.set_loop_state(handle.loop_state());
    let mut dev = handle.developer();
    let eval =
        |expr: &str, frame: usize| hub.call("debug.eval", &json!({"expr": expr, "frame": frame}));
    let num = |expr: &str, frame: usize| -> f64 {
        let v = eval(expr, frame).unwrap_or_else(|p| panic!("{expr}: {}", p.message));
        v["value"]
            .as_f64()
            .unwrap_or_else(|| panic!("{expr}: {v:#}"))
    };

    call(
        &hub,
        "debug.breakpoints.set",
        json!({"file": "rules.ts", "line": mark_at("after-block")}),
    );
    call(
        &hub,
        "debug.breakpoints.set",
        json!({"file": "rules.ts", "line": mark_at("inner")}),
    );
    call(
        &hub,
        "debug.breakpoints.set",
        json!({"file": "rules.ts", "line": mark_at("aim")}),
    );
    let stepper = std::thread::spawn(move || {
        let r = dev
            .call("time.step", json!({"ticks": 1}))
            .map(|r| r.into_json());
        (dev, r)
    });

    // In bearingOf, called from the loop's block: its own local, and in the caller's frame the
    // block's locals declared before the call.
    let s = paused(&hub);
    assert_eq!(at(&s).1, mark_at("inner"), "{s:#}");
    assert_eq!(num("b", 0), (3.0f64).atan2(-5.0));
    assert_eq!(num("dx + dz", 0), 8.0, "bearingOf's arguments");
    assert_eq!(s["frames"][1]["function"], "run", "{s:#}");
    assert_eq!(num("dx * 10 + dz", 1), 35.0, "the caller's block");
    assert_eq!(num("r", 1), 0.0);
    let turn = eval("turn", 1).unwrap_err();
    assert!(
        turn.message.contains("turn"),
        "declared after the call's statement: {}",
        turn.message
    );
    call(&hub, "debug.continue", json!({}));

    // After the sibling block: dx, dz and bearing, declared one after another as in helm.ts, the
    // module's constant through the closure; not the if block's `hidden`.
    let s = paused(&hub);
    assert_eq!(at(&s).1, mark_at("after-block"), "{s:#}");
    assert_eq!(num("dz", 0), 5.0);
    assert_eq!(num("bearing", 0), (3.0f64).atan2(-5.0));
    assert_eq!(
        num("bearing * GAIN + dx", 0),
        (3.0f64).atan2(-5.0) * 0.5 + 3.0
    );
    let hidden = eval("hidden", 0).unwrap_err();
    assert!(hidden.message.contains("hidden"), "{}", hidden.message);
    call(&hub, "debug.continue", json!({}));

    // helm.ts:67 as the evaluation had it: PR #1421 saw `dx` here and not `dz` or `bearing`.
    let s = paused(&hub);
    assert_eq!(at(&s).1, mark_at("aim"), "{s:#}");
    assert_eq!(num("dx", 0), 3.0);
    assert_eq!(num("dz", 0), 5.0);
    assert_eq!(num("bearing", 0), (3.0f64).atan2(-5.0));
    assert_eq!(num("i + tick", 0), 1.0);
    call(&hub, "debug.detach", json!({}));
    let (dev, stepped) = stepper.join().unwrap();
    assert_eq!(stepped.unwrap()["tick"], 1);
    drop(dev);
    hub.shutdown();
    handle.shutdown(5000).unwrap();
}

/// `debug.state` names the CDP endpoint while the hub serves one (the editor's Copy Chrome
/// DevTools URL), and `null` once it stopped.
#[test]
fn debug_state_names_the_cdp_endpoint() {
    let hub = DebugHub::new(HubOptions::default());
    assert!(call(&hub, "debug.state", json!({}))["cdp"].is_null());
    let cdp = hub
        .serve_cdp(CdpOptions {
            port: 0,
            ..CdpOptions::default()
        })
        .unwrap();
    let s = call(&hub, "debug.state", json!({}));
    let ws = format!("ws://{}/devtools/game", cdp.addr());
    assert_eq!(s["cdp"]["ws"], json!(ws), "{s:#}");
    assert_eq!(s["cdp"]["devtools"], json!(cdp.devtools_url()), "{s:#}");
    cdp.stop();
    assert!(call(&hub, "debug.state", json!({}))["cdp"].is_null());
    hub.shutdown();
}
