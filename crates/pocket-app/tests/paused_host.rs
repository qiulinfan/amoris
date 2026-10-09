//! The host while the script debugger holds the game (docs/spec/server.md 3.4; the first and
//! fourth findings of docs/bench/debug-eval.md), driven through `pocket_server::Host::call` as the
//! CLI, the editor and MCP sessions drive it, over the debugger's test project on a real game
//! thread started as `pocket serve` starts it: a `time.step` that a breakpoint or a data watch
//! stops answers at the stop and the rest of the step is dropped; reads answer from the last
//! publication exactly as the game answers them; every other call is refused at once with
//! `debug.paused`; the catalog describes each `debug.*` method; `debug.watch` takes entity names;
//! and the MCP `debug` tool lists every debugger parameter.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocket_contract::Problem;
use pocket_debug::{DebugHub, HubOptions};
use pocket_link::Source;
use pocket_runtime::thread::{GameThread, Pacing, ThreadOptions};
use pocket_runtime::{GameBuilder, Project};
use pocket_server::{BoxFuture, GameAccess, Host, Via};
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

/// The debugger as `pocket serve` installs it (pocket-app's `DebugBridge`).
struct Bridge(DebugHub);

impl pocket_server::DebugHub for Bridge {
    fn call(&self, method: &str, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let hub = self.0.clone();
        let method = method.to_owned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || hub.call(&method, &params))
                .await
                .unwrap()
        })
    }

    fn methods(&self) -> Vec<Value> {
        pocket_debug::methods()
            .iter()
            .map(pocket_debug::Method::entry)
            .collect()
    }
}

/// Every key named `paused_at` taken out.
fn unmarked(mut v: Value) -> Value {
    match &mut v {
        Value::Object(m) => {
            m.remove("paused_at");
            for x in m.values_mut() {
                *x = unmarked(x.take());
            }
        }
        Value::Array(a) => {
            for x in a.iter_mut() {
                *x = unmarked(x.take());
            }
        }
        _ => {}
    }
    v
}

#[test]
fn a_held_host_never_blocks_its_callers() {
    let project = Project::load(&fixture()).unwrap();
    let setup = Arc::new(project.setup(false).unwrap());
    let hub = DebugHub::new(HubOptions::default());
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = hub.clone();
    let dir = fixture();
    // As pocket serve: real time at speed 1, paused (the edit world).
    let handle = GameThread::spawn(
        move || {
            let mut g = GameBuilder::new(setup).project(dir).build()?;
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
    let clients = handle.clients();
    let access = GameAccess {
        reader: handle.reader(),
        editor: clients.client(Source::Editor).unwrap(),
        api: clients.developer(),
        developer: Arc::new(move || clients.developer()),
    };
    let host = Host::new(access, fixture(), None);
    host.set_debug(Arc::new(Bridge(hub.clone())));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let via = Via::Api;
        let call = |m: &'static str, p: Value| {
            let host = host.clone();
            let via = via.clone();
            async move {
                host.call(&via, m, p)
                    .await
                    .unwrap_or_else(|e| panic!("{m}: {} {}", e.code, e.message))
            }
        };
        let refused = |m: &'static str, p: Value| {
            let host = host.clone();
            async move {
                let t = Instant::now();
                let e = host.call(&Via::Api, m, p).await.unwrap_err();
                (e, t.elapsed())
            }
        };

        // The catalog describes the debugger like every other command.
        let catalog = call("catalog.list", json!({})).await;
        let entry = |name: &str| {
            catalog
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == name)
                .cloned()
                .unwrap_or_else(|| panic!("no catalog entry {name}"))
        };
        for m in pocket_debug::methods() {
            let e = entry(m.name);
            assert_eq!(e["params"], m.params, "{}", m.name);
            assert_eq!(e["kind"], m.kind, "{}", m.name);
        }
        let watch = entry("debug.watch");
        for p in ["entity", "component", "field"] {
            assert!(watch["params"]["properties"].get(p).is_some(), "{watch:#}");
        }
        assert!(entry("debug.rewind")["params"]["properties"]["bundle"].is_object());
        assert!(
            entry("debug.breakpoints.set")["aliases"]
                .as_array()
                .unwrap()
                .contains(&json!("debug.break"))
        );

        // The world before any stop, read by the game.
        let stepped = call("time.step", json!({"ticks": 3})).await;
        assert_eq!(stepped["tick"], 3);
        let reads: Vec<(&'static str, Value)> = vec![
            ("world.get", json!({"entity": "Gauge"})),
            (
                "world.get",
                json!({"entity": 1, "fields": ["Gauge.level", "Gauge.peaks"]}),
            ),
            ("world.tree", json!({})),
            ("world.tree", json!({"filter": "gau", "with": ["Gauge"]})),
            (
                "world.query",
                json!({"with": ["Gauge"], "fields": ["Gauge.level"]}),
            ),
            ("world.schema", json!({"component": "Gauge"})),
            ("world.schema", json!({})),
            (
                "scripts.read",
                json!({"path": "rules.ts", "lines": "2-4", "numbered": true}),
            ),
        ];
        let mut live = Vec::new();
        for (m, p) in &reads {
            live.push(call(m, p.clone()).await);
        }
        let status = call("status", json!({})).await;

        // A breakpoint by the module's short name stops the step at once.
        let bp = call(
            "debug.breakpoints.set",
            json!({"file": "rules.ts", "line": mark("next")}),
        )
        .await;
        assert_eq!(bp["file"], "scripts/rules.ts");
        let t = Instant::now();
        let s = call("time.step", json!({"ticks": 5})).await;
        let took = t.elapsed();
        assert_eq!(s["paused"], true, "{s:#}");
        let why = &s["stopped_by"];
        assert_eq!(
            (
                why["reason"].as_str(),
                why["tick"].as_u64(),
                why["system"].as_str()
            ),
            (Some("breakpoint"), Some(4), Some("measure")),
            "{s:#}"
        );
        assert_eq!(why["location"]["line"].as_u64(), Some(mark("next")));
        assert_eq!(why["breakpoint"], bp["id"]);
        assert!(
            took < Duration::from_secs(2),
            "the step answered in {took:?}"
        );
        eprintln!("time.step answered at the breakpoint in {took:?}");

        // Reads answer from the last publication, as the game answered them.
        for ((m, p), want) in reads.iter().zip(&live) {
            let got = call(m, p.clone()).await;
            if m.starts_with("world.get") {
                assert_eq!(got["paused_at"]["tick"], 4, "{m} {p}: {got:#}");
                assert_eq!(got["paused_at"]["snapshot_tick"], 3, "{got:#}");
            }
            assert_eq!(&unmarked(got), want, "{m} {p}");
        }
        let held = call("status", json!({})).await;
        for k in [
            "tick",
            "world_hash",
            "entities",
            "bundle",
            "mode",
            "epoch",
            "paused",
        ] {
            assert_eq!(held[k], status[k], "status {k}: {held:#}");
        }
        assert_eq!(held["state"], "breakpoint");
        assert_eq!(
            held["paused_at"]["location"]["line"].as_u64(),
            Some(mark("next"))
        );
        let listed = call("scripts.list", json!({})).await;
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["path"] == "scripts/rules.ts")
        );
        let docs = call("docs.search", json!({"query": "gauge"})).await;
        assert!(
            docs.as_array()
                .unwrap()
                .iter()
                .any(|d| d["component"] == "Gauge"),
            "{docs:#}"
        );
        let st = call("scripts.status", json!({})).await;
        assert_eq!(st["bundle"], status["bundle"]);

        // Everything else that needs the game thread is refused at once, saying where it stands.
        for (m, p) in [
            ("scripts.apply", json!({})),
            ("time.step", json!({"ticks": 1})),
            ("snapshots.list", json!({})),
            ("history.list", json!({})),
            (
                "world.edit",
                json!({"ops": [{"destroy": {"entity": "Gauge"}}]}),
            ),
        ] {
            let (e, took) = refused(m, p).await;
            assert_eq!(e.code, "debug.paused", "{m}: {e:#?}");
            assert_eq!(e.detail["queued"], false);
            assert_eq!(e.detail["location"]["line"].as_u64(), Some(mark("next")));
            assert!(took < Duration::from_millis(500), "{m} refused in {took:?}");
        }

        // The debugger's own calls answer; entity names reach debug.watch.
        let w = call(
            "debug.watch",
            json!({"entity": "Gauge", "component": "Gauge", "field": "level"}),
        )
        .await;
        assert_eq!(w["entity"], 1, "{w:#}");
        let (e, _) = refused(
            "debug.watch",
            json!({"entity": "Gauje", "component": "Gauge"}),
        )
        .await;
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        assert_eq!(e.detail["suggestions"][0], "Gauge");
        call("debug.unwatch", json!({})).await;
        let brief = call("debug.state", json!({"brief": true})).await;
        let frames = brief["frames"].as_array().unwrap();
        assert!(
            frames[0]["locals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["name"] == "r")
        );
        assert!(
            frames[0]["locals"]
                .as_array()
                .unwrap()
                .iter()
                .all(|l| l["name"] != "ctx")
        );
        assert!(
            frames.iter().all(|f| f.get("closure").is_none()),
            "{brief:#}"
        );
        let full = call("debug.state", json!({})).await;
        assert!(brief.to_string().len() * 2 < full.to_string().len());

        // Continue: tick 4 ends and the rest of the step is dropped.
        call("debug.breakpoints.clear", json!({})).await;
        call("debug.continue", json!({})).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut now = call("status", json!({})).await;
        while now["tick"] != 4 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            now = call("status", json!({})).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let now = call("status", json!({})).await;
        assert_eq!(
            now["tick"], 4,
            "the step ended with the stopped tick: {now:#}"
        );
        assert_eq!(now["steps_due"], 0, "{now:#}");

        // A data watch, by name, stops a step the same way.
        let w = call(
            "debug.watch",
            json!({"entity": "Gauge", "component": "Gauge", "field": "level"}),
        )
        .await;
        let s = call("time.step", json!({"ticks": 3})).await;
        assert_eq!(s["stopped_by"]["reason"], "data_breakpoint", "{s:#}");
        assert_eq!(s["stopped_by"]["watch"]["id"], w["id"], "{s:#}");
        assert_eq!(
            s["stopped_by"]["watch"]["written_at"]["line"].as_u64(),
            Some(mark("write-level"))
        );
        call("debug.unwatch", json!({})).await;
        call("debug.continue", json!({})).await;

        // Time travel goes through the host and keeps the applied scripts.
        let deadline = Instant::now() + Duration::from_secs(5);
        while host.debug_stop().is_some() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let r = call("debug.rewind", json!({"tick": 2})).await;
        assert_eq!(
            (r["restored"].as_u64(), r["tick"].as_u64()),
            (Some(0), Some(2)),
            "{r:#}"
        );
        assert_eq!(r["scripts"]["kept"], "applied", "{r:#}");
        call("debug.detach", json!({})).await;
    });
    hub.shutdown();
    drop(host);
    handle.shutdown(5000).unwrap();
}

/// The MCP `debug` tool takes every debugger method as an action and lists every parameter they
/// take (the evaluation's agents called it with `action` alone).
#[test]
fn the_mcp_debug_tool_lists_every_parameter() {
    let tools = pocket_mcp::tools::tools();
    let debug = tools.iter().find(|t| t.name == "debug").unwrap();
    let props = debug.schema["properties"].as_object().unwrap();
    let actions: Vec<&str> = props["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    // The host's catalog of the debugger: pocket-debug's methods and the host's debug.rewind.
    let bridge = Bridge(DebugHub::new(HubOptions::default()));
    let methods: Vec<(String, Value)> = pocket_server::debug_catalog(Some(&bridge))
        .into_iter()
        .map(|m| (m["name"].as_str().unwrap().to_owned(), m["params"].clone()))
        .collect();
    assert!(methods.iter().any(|(n, _)| n == "debug.rewind"));
    for (name, params) in &methods {
        let action = name.strip_prefix("debug.").unwrap();
        assert!(actions.contains(&action), "the debug tool lacks {action}");
        for p in params["properties"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.keys())
        {
            assert!(props.contains_key(p), "the debug tool lacks {name}'s {p}");
        }
    }
    assert_eq!(actions.len(), methods.len(), "{actions:?}");
}
