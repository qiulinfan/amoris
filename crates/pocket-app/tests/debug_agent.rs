//! The agents' debugger end to end (docs/spec/debugger.md 7 and 11): the debugger's test project on
//! a real game thread in real time, driven only through `DebugHub::call` as pocket-server and
//! pocket-mcp drive it: a `debugger;` statement, pause on uncaught and on caught exceptions, a
//! TypeScript breakpoint with locals and evaluation, step into and out, a data breakpoint on a
//! component field, console lines and the detach.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocket_debug::{DebugEvent, DebugHub, HubOptions};
use pocket_runtime::thread::{GameThread, ThreadOptions};
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
    let gauge = call(&hub, "debug.eval", json!({"expr": "gauges.id(0)"}))["value"]
        .as_u64()
        .unwrap();
    let bad = hub.call("debug.eval", &json!({"exp": "1"})).unwrap_err();
    assert_eq!(bad.code, "request.unknown_field");
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
    let s = call(&hub, "debug.step", json!({"kind": "out"}));
    assert_eq!(s["frames"][0]["function"], "run", "{s:#}");
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
    let heard: Vec<Value> = events.try_iter().filter_map(|e| e.json()).collect();
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
