//! The host's agent sessions (docs/spec/server.md 3.5), on the sailing course served as `amoris
//! serve` serves it. The host lists who calls it as an agent and keeps what they called: an MCP
//! client on `/mcp` is a session from its `initialize` to the end of its HTTP session; a client of
//! MCP 2026-07-28, which keeps no session, is one row for all its requests; a bridge registers
//! with `agent.session.open` and calls with the handle it got; a seat's calls with no session are
//! the row `player:<seat>`. An editor that connects later reads the list and the stream
//! (`agent.session.list`, `agent.session.events`) and is pushed what follows. `amoris mcp`
//! attached to a running host is such a bridge: a session of its own, not the shared `api`
//! client. Sessions are host state: opening, listing and closing them leaves the world as it was.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use amoris_link::Source;
use amoris_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use amoris_runtime::{GameBuilder, Project};
use amoris_server::{GameAccess, Host, ServeOptions};
use serde_json::{Value, json};

fn course() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing-course")
}

/// The sailing course on a game thread started as `amoris serve` starts it (real time at speed 1,
/// paused: the edit world), behind a host that opens player clients.
fn start() -> (GameHandle, Host) {
    let root = course();
    let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
    let t0 = Instant::now();
    let clock: amoris_runtime::thread::Clock =
        Arc::new(move || t0.elapsed().as_secs_f64() * 1000.0);
    let dir = root.clone();
    let handle = GameThread::spawn(
        move || GameBuilder::new(setup).project(dir).build(),
        ThreadOptions {
            pacing: Pacing::RealTime { speed: 1.0 },
            paused: true,
            ..ThreadOptions::new(clock)
        },
    )
    .unwrap();
    let clients = handle.clients();
    let players = clients.clone();
    let access = GameAccess {
        reader: handle.reader(),
        editor: clients.client(Source::Editor).unwrap(),
        api: clients.developer(),
        developer: Arc::new(move || clients.developer()),
    };
    let host = Host::new(access, root, None);
    host.set_players(Arc::new(move |i| players.client(Source::Player(i))));
    (handle, host)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// `POST /api/call` with `body`: the answer, `{id, result}` or `{id, error}`.
fn post(url: &str, body: &Value) -> Value {
    let mut r = agent()
        .post(format!("{url}/api/call"))
        .content_type("application/json")
        .send(&body.to_string())
        .unwrap();
    serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap()
}

/// A developer's call through the HTTP API's shared client, which is no session.
fn result(url: &str, method: &str, params: Value) -> Value {
    let a = post(url, &json!({"id": 1, "method": method, "params": params}));
    assert!(a.get("error").is_none(), "{method} {params}: {a:#}");
    a["result"].clone()
}

fn error(url: &str, body: &Value) -> Value {
    let a = post(url, body);
    assert!(a.get("result").is_none(), "{body}: {a:#}");
    a["error"].clone()
}

fn sessions(url: &str) -> Vec<Value> {
    result(url, "agent.session.list", json!({}))["sessions"]
        .as_array()
        .unwrap()
        .clone()
}

fn events(url: &str, params: Value) -> Value {
    result(url, "agent.session.events", params)
}

/// The kinds and methods of an `agent.session.events` answer, in order.
fn told(answer: &Value) -> Vec<String> {
    answer["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| match e["method"].as_str() {
            Some(m) => format!("{} {m}", e["kind"].as_str().unwrap()),
            None => e["kind"].as_str().unwrap().to_owned(),
        })
        .collect()
}

fn seqs(answer: &Value) -> Vec<u64> {
    answer["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect()
}

/// The sessions of the undo entries, oldest first.
fn history_sessions(url: &str) -> Vec<Value> {
    result(url, "history.list", json!({}))["entries"]["undo"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["session"].clone())
        .collect()
}

fn set_cargo(value: u64) -> Value {
    json!({"action": "edit",
           "ops": [{"set": {"entity": "Crate4", "component": "Cargo", "value": {"value": value}}}]})
}

/// One answer of `POST /mcp`: the HTTP status, the session id it named, and the JSON-RPC message
/// (from the event stream's `data:` lines, or the body itself).
struct McpAnswer {
    status: u16,
    session: Option<String>,
    message: Value,
    body: String,
}

/// `POST /mcp` with `message` and `headers` beside the ones every MCP request carries.
fn mcp_post(url: &str, message: &Value, headers: &[(&str, &str)]) -> McpAnswer {
    let mut req = agent()
        .post(format!("{url}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .content_type("application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut r = req.send(&message.to_string()).unwrap();
    let status = r.status().as_u16();
    let session = r
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = r.body_mut().read_to_string().unwrap();
    let answered = |v: &Value| v.get("id").is_some_and(|id| *id == message["id"]);
    let message = body
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(answered)
        .or_else(|| serde_json::from_str::<Value>(&body).ok())
        .unwrap_or(Value::Null);
    McpAnswer {
        status,
        session,
        message,
        body,
    }
}

/// A `tools/call` answer: whether the tool failed, and its text.
fn tool_result(a: &McpAnswer) -> (bool, String) {
    let res = &a.message["result"];
    assert!(
        res.is_object(),
        "HTTP {}: no tool result in {}",
        a.status,
        a.body
    );
    (
        res["isError"].as_bool().unwrap_or(false),
        res["content"][0]["text"].as_str().unwrap_or("").to_owned(),
    )
}

/// The `agent` events an editor's socket is pushed, from when it subscribed.
fn agent_pushes(addr: std::net::SocketAddr) -> mpsc::Receiver<Value> {
    let (mut ws, _) = tungstenite::connect(format!("ws://{addr}/ws")).unwrap();
    ws.send(tungstenite::Message::Text(
        json!({"id": 1, "method": "subscribe", "params": {"topics": ["agent"]}})
            .to_string()
            .into(),
    ))
    .unwrap();
    // The subscription is in place once it is answered.
    loop {
        let tungstenite::Message::Text(t) = ws.read().unwrap() else {
            continue;
        };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        if v["id"] == json!(1) {
            assert!(v["result"]["topics"].is_array(), "{v:#}");
            break;
        }
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(msg) = ws.read() {
            let tungstenite::Message::Text(t) = msg else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else {
                continue;
            };
            if v["event"] == json!("agent") && tx.send(v["data"].clone()).is_err() {
                return;
            }
        }
    });
    rx
}

/// The keys of a session row, sorted.
const ROW_KEYS: [&str; 11] = [
    "calls",
    "failed_calls",
    "id",
    "label",
    "last_method",
    "last_ts",
    "live",
    "opened_ts",
    "role",
    "seat",
    "transport",
];

fn keys(v: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

#[test]
fn the_host_lists_its_sessions_and_keeps_their_calls_for_a_late_editor() {
    let (handle, host) = start();
    let rt = runtime();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (addr, task) = rt
        .block_on(host.serve(&ServeOptions { port: 0 }, async {
            let _ = stopped.await;
        }))
        .unwrap();
    let url = format!("http://{addr}");

    // A host nobody called as an agent.
    assert_eq!(
        result(&url, "agent.session.list", json!({})),
        json!({"sessions": [], "last": 0})
    );
    assert_eq!(
        events(&url, json!({})),
        json!({"events": [], "last": 0, "missed": 0})
    );

    // An MCP client on /mcp, while no editor listens: a session from its `initialize` on.
    let init = mcp_post(
        &url,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "probe", "version": "1"}}}),
        &[],
    );
    assert_eq!(init.status, 200, "{}", init.body);
    assert_eq!(
        init.message["result"]["protocolVersion"], "2025-06-18",
        "{}",
        init.body
    );
    let id = init.session.clone().expect("an Mcp-Session-Id");
    let rows = sessions(&url);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(
        (rows[0]["id"].as_str(), rows[0]["calls"].as_u64()),
        (Some("mcp-1"), Some(0))
    );
    let in_session = [("Mcp-Session-Id", id.as_str())];
    let noted = mcp_post(
        &url,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        &in_session,
    );
    assert_eq!(noted.status, 202, "{}", noted.body);
    let call = |n: u64, args: Value| {
        mcp_post(
            &url,
            &json!({"jsonrpc": "2.0", "id": n, "method": "tools/call",
                    "params": {"name": "world", "arguments": args}}),
            &in_session,
        )
    };
    let (failed, text) = tool_result(&call(2, set_cargo(41)));
    assert!(!failed, "{text}");
    let (failed, text) = tool_result(&call(3, json!({"action": "tree"})));
    assert!(!failed, "{text}");
    let (failed, text) = tool_result(&call(4, json!({"action": "get", "entity": "Nobody"})));
    assert!(failed, "{text}");

    // The list: one row, with exactly the row's keys.
    let rows = sessions(&url);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!(keys(row), ROW_KEYS, "{row:#}");
    for (k, want) in [
        ("id", json!("mcp-1")),
        ("transport", json!("http")),
        ("role", json!("developer")),
        ("seat", Value::Null),
        ("label", json!("probe 1")),
        ("calls", json!(3)),
        ("failed_calls", json!(1)),
        ("last_method", json!("world.get")),
        ("live", json!(true)),
    ] {
        assert_eq!(row[k], want, "{k}: {row:#}");
    }
    assert!(row["opened_ts"].as_u64().unwrap() > 1_600_000_000_000);
    assert!(row["last_ts"].as_u64() >= row["opened_ts"].as_u64());
    assert_eq!(
        result(&url, "agent.session.list", json!({}))["last"],
        json!(7)
    );

    // The stream, read back by a client that heard none of it.
    let all = events(&url, json!({}));
    assert_eq!(
        told(&all),
        [
            "opened",
            "call world.edit",
            "result world.edit",
            "call world.tree",
            "result world.tree",
            "call world.get",
            "result world.get"
        ],
        "{all:#}"
    );
    assert_eq!(seqs(&all), [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(
        (all["last"].as_u64(), all["missed"].as_u64()),
        (Some(7), Some(0))
    );
    let list = all["events"].as_array().unwrap();
    assert!(list.iter().all(|e| e["session"] == "mcp-1"), "{all:#}");
    assert!(list.iter().all(|e| e["ts"].as_u64().is_some()), "{all:#}");
    assert_eq!(list[0]["row"]["id"], "mcp-1", "{all:#}");
    assert_eq!(list[0]["row"]["label"], "probe 1", "{all:#}");
    assert_eq!(
        keys(&list[1]),
        ["kind", "method", "seq", "session", "summary", "ts"],
        "{all:#}"
    );
    assert_eq!(
        keys(&list[2]),
        ["kind", "method", "ok", "seq", "session", "summary", "ts"],
        "{all:#}"
    );
    assert_eq!(
        (list[2]["ok"].as_bool(), list[6]["ok"].as_bool()),
        (Some(true), Some(false)),
        "{all:#}"
    );
    // A refusal's summary is its code and message.
    assert!(
        list[6]["summary"].as_str().unwrap().contains("Nobody"),
        "{all:#}"
    );
    assert_eq!(seqs(&events(&url, json!({"since": 5}))), [6, 7]);
    assert_eq!(seqs(&events(&url, json!({"limit": 2}))), [6, 7]);
    let first = events(&url, json!({"since": 0, "limit": 2}));
    assert_eq!(seqs(&first), [1, 2]);
    assert_eq!(
        first["last"],
        json!(2),
        "a cut answer ends where it stopped"
    );
    assert_eq!(
        seqs(&events(&url, json!({"since": 2, "limit": 100}))).len(),
        5
    );
    assert_eq!(seqs(&events(&url, json!({"session": "mcp-1"}))).len(), 7);
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.events", "params": {"session": "nope"}}),
    );
    assert_eq!(e["code"], json!("request.invalid_value"), "{e:#}");
    assert_eq!(e["detail"]["path"], json!("/session"), "{e:#}");
    assert_eq!(e["detail"]["allowed"], json!(["mcp-1"]), "{e:#}");
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.events", "params": {"sinse": 3}}),
    );
    assert_eq!(e["code"], json!("request.unknown_field"), "{e:#}");
    // The methods are in the catalog, the server's own; nothing starts an agent from here.
    let catalog = result(&url, "catalog.list", json!({}));
    for name in [
        "agent.session.list",
        "agent.session.events",
        "agent.session.open",
        "agent.session.close",
    ] {
        let entry = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no catalog entry {name}"));
        assert_eq!(entry["server"], json!(true), "{entry:#}");
    }
    // A host whose owner registered no agent program starts none.
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.start",
                "params": {"agent": "claude", "goal": "Sail."}}),
    );
    assert_eq!(e["code"], json!("host.no_agents"), "{e:#}");

    // Its edit is its own in the history.
    assert_eq!(history_sessions(&url), [json!("mcp-1")]);

    // An editor connects now: it is pushed what follows, and nothing older.
    let pushes = agent_pushes(addr);
    let (failed, text) = tool_result(&call(5, json!({"action": "tree"})));
    assert!(!failed, "{text}");
    let wait = Duration::from_secs(5);
    let pushed = [
        pushes.recv_timeout(wait).unwrap(),
        pushes.recv_timeout(wait).unwrap(),
    ];
    assert_eq!(
        (pushed[0]["seq"].as_u64(), pushed[0]["kind"].as_str()),
        (Some(8), Some("call")),
        "{pushed:#?}"
    );
    assert_eq!(
        (pushed[1]["seq"].as_u64(), pushed[1]["kind"].as_str()),
        (Some(9), Some("result")),
        "{pushed:#?}"
    );
    assert_eq!(pushed[1], events(&url, json!({"limit": 1}))["events"][0]);

    // The client ends its HTTP session: the session closes, and says so.
    let ended = agent()
        .delete(format!("{url}/mcp"))
        .header("Mcp-Session-Id", id.as_str())
        .call()
        .unwrap();
    assert!(ended.status().is_success(), "HTTP {}", ended.status());
    let closed = pushes.recv_timeout(wait).expect("a closed event");
    assert_eq!(closed["kind"], json!("closed"), "{closed:#}");
    assert_eq!(closed["seq"], json!(10), "{closed:#}");
    assert_eq!(
        (
            closed["row"]["id"].as_str(),
            closed["row"]["live"].as_bool(),
            closed["row"]["calls"].as_u64()
        ),
        (Some("mcp-1"), Some(false), Some(4)),
        "{closed:#}"
    );
    let rows = sessions(&url);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["live"], json!(false), "{rows:#?}");

    // A client of MCP 2026-07-28 keeps no session: each request comes alone and names its
    // client itself. Listing the tools opens nothing; its calls share one row, never live.
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "stateless", "version": "2"},
    });
    let alone = [("MCP-Protocol-Version", "2026-07-28")];
    let listed = mcp_post(
        &url,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta}}),
        &[alone[0], ("Mcp-Method", "tools/list")],
    );
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert!(
        listed.message["result"]["tools"].is_array(),
        "{}",
        listed.body
    );
    assert!(listed.session.is_none(), "{}", listed.body);
    let quiet = result(&url, "agent.session.list", json!({}));
    assert_eq!(quiet["sessions"].as_array().unwrap().len(), 1, "{quiet:#}");
    assert_eq!(quiet["last"], json!(10), "{quiet:#}");
    for (n, value) in [(2, 42), (3, 43)] {
        let a = mcp_post(
            &url,
            &json!({"jsonrpc": "2.0", "id": n, "method": "tools/call",
                    "params": {"name": "world", "arguments": set_cargo(value), "_meta": meta}}),
            &[
                alone[0],
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "world"),
            ],
        );
        assert_eq!(a.status, 200, "{}", a.body);
        let (failed, text) = tool_result(&a);
        assert!(!failed, "{text}");
    }
    let rows = sessions(&url);
    assert_eq!(rows.len(), 2, "{rows:#?}");
    let row = &rows[1];
    for (k, want) in [
        ("id", json!("mcp-2")),
        ("transport", json!("http")),
        ("role", json!("developer")),
        ("label", json!("stateless 2")),
        ("calls", json!(2)),
        ("failed_calls", json!(0)),
        ("live", json!(false)),
    ] {
        assert_eq!(row[k], want, "{k}: {row:#}");
    }
    assert_eq!(
        history_sessions(&url),
        [json!("mcp-1"), json!("mcp-2"), json!("mcp-2")]
    );
    assert_eq!(
        told(&events(&url, json!({"session": "mcp-2"}))),
        [
            "opened",
            "call world.edit",
            "result world.edit",
            "call world.edit",
            "result world.edit"
        ]
    );

    let _ = stop.send(());
    rt.block_on(task).unwrap();
    drop(rt);
    drop(host);
    handle.shutdown(5000).unwrap();
}

#[test]
fn a_bridge_registers_a_session_and_calls_as_it() {
    let (handle, host) = start();
    let rt = runtime();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (addr, task) = rt
        .block_on(host.serve(&ServeOptions { port: 0 }, async {
            let _ = stopped.await;
        }))
        .unwrap();
    let url = format!("http://{addr}");
    let dev = |method: &str, params: Value| json!({"id": 1, "method": method, "params": params});
    let as_session = |handle: &Value, method: &str, params: Value| json!({"id": 1, "method": method, "params": params, "handle": handle});
    let world = |url: &str| {
        let s = result(url, "status", json!({}));
        (
            s["tick"].clone(),
            s["world_hash"].clone(),
            result(url, "snapshot", json!({})),
        )
    };
    let before = world(&url);

    // A bridge registers: a developer session of its own, live, with a handle only it is told.
    let opened = result(
        &url,
        "agent.session.open",
        json!({"label": "bridge-test 1"}),
    );
    let handle_a = opened["handle"].clone();
    assert!(
        handle_a.as_str().is_some_and(|h| !h.is_empty()),
        "{opened:#}"
    );
    for (k, want) in [
        ("id", json!("mcp-1")),
        ("transport", json!("bridge")),
        ("role", json!("developer")),
        ("seat", Value::Null),
        ("label", json!("bridge-test 1")),
        ("calls", json!(0)),
        ("live", json!(true)),
    ] {
        assert_eq!(opened[k], want, "{k}: {opened:#}");
    }
    let rows = sessions(&url);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(keys(&rows[0]), ROW_KEYS, "the list tells no handle");
    // A second one, closed by its id as a bridge whose process is gone is.
    let second = result(&url, "agent.session.open", json!({}));
    assert_eq!(second["id"], json!("mcp-2"), "{second:#}");
    assert_ne!(second["handle"], handle_a);
    let closed = result(&url, "agent.session.close", json!({"session": "mcp-2"}));
    assert_eq!(
        (closed["id"].as_str(), closed["live"].as_bool()),
        (Some("mcp-2"), Some(false)),
        "{closed:#}"
    );
    let e = error(&url, &as_session(&second["handle"], "status", json!({})));
    assert_eq!(e["code"], json!("host.session_unknown"), "{e:#}");
    // A call with a closed session's handle is nobody's: refused, and not in the stream.
    assert_eq!(
        told(&events(&url, json!({}))),
        ["opened", "opened", "closed"]
    );
    // Sessions opened, listed, read and closed that sent the world nothing left it as it was:
    // its tick, its hash and its count of writes.
    assert_eq!(world(&url), before);

    // Its calls are its own: in the history and in the stream.
    let edit = json!({"ops": [{"set": {"entity": "Crate4", "component": "Cargo",
                                        "value": {"value": 51}}}]});
    let a = post(&url, &as_session(&handle_a, "world.edit", edit));
    assert!(a.get("error").is_none(), "{a:#}");
    assert_eq!(a["id"], json!(1), "{a:#}");
    assert_eq!(history_sessions(&url), [json!("mcp-1")]);
    assert_eq!(
        told(&events(&url, json!({"session": "mcp-1"}))),
        ["opened", "call world.edit", "result world.edit"]
    );
    // The session methods are a developer's, also through a session.
    let a = post(
        &url,
        &as_session(&handle_a, "agent.session.list", json!({})),
    );
    assert_eq!(
        a["result"]["sessions"].as_array().unwrap().len(),
        2,
        "{a:#}"
    );

    // What is no handle.
    let e = error(&url, &as_session(&json!("0-1"), "status", json!({})));
    assert_eq!(e["code"], json!("host.session_unknown"), "{e:#}");
    assert_eq!(e["detail"]["handle"], json!("0-1"), "{e:#}");
    let e = error(&url, &as_session(&json!(3), "status", json!({})));
    assert_eq!(e["code"], json!("request.wrong_type"), "{e:#}");
    assert_eq!(e["detail"]["path"], json!("/handle"), "{e:#}");
    let e = error(
        &url,
        &json!({"id": 1, "method": "status", "params": {}, "handle": handle_a,
                "seat": "skipper"}),
    );
    assert_eq!(e["code"], json!("request.conflict"), "{e:#}");
    assert_eq!(e["detail"]["paths"], json!(["/handle", "/seat"]), "{e:#}");
    let e = error(&url, &dev("agent.session.open", json!({"lable": "x"})));
    assert_eq!(e["code"], json!("request.unknown_field"), "{e:#}");

    // A session that plays a seat: the player tools alone, refused before the seat is looked up
    // and counted as failed calls.
    let player = result(&url, "agent.session.open", json!({"seat": "skipper"}));
    assert_eq!(
        (
            player["id"].as_str(),
            player["role"].as_str(),
            player["seat"].as_str(),
            player["transport"].as_str()
        ),
        (
            Some("mcp-3"),
            Some("player"),
            Some("skipper"),
            Some("bridge")
        ),
        "{player:#}"
    );
    let seat = player["handle"].clone();
    let o = post(
        &url,
        &as_session(&seat, "player.observe", json!({"projection": "json"})),
    );
    assert_eq!(o["result"]["omniscient"], json!(false), "{o:#}");
    for (method, params) in [
        ("world.get", json!({"entity": "Crate4"})),
        ("events.since", json!({})),
        ("agent.session.list", json!({})),
    ] {
        let e = error(&url, &as_session(&seat, method, params));
        assert_eq!(e["code"], json!("permission.denied"), "{method}: {e:#}");
        assert_eq!(e["detail"]["role"], json!("player"), "{method}: {e:#}");
    }
    let row = sessions(&url)[2].clone();
    assert_eq!(
        (row["calls"].as_u64(), row["failed_calls"].as_u64()),
        (Some(4), Some(3)),
        "{row:#}"
    );
    assert_eq!(row["last_method"], json!("agent.session.list"), "{row:#}");
    // A seat the game does not declare is found out by the session's first call.
    let lost = result(&url, "agent.session.open", json!({"seat": "lookout"}));
    assert_eq!(lost["id"], json!("mcp-4"), "{lost:#}");
    let e = error(
        &url,
        &as_session(&lost["handle"], "player.observe", json!({})),
    );
    assert_eq!(e["code"], json!("seat.unknown"), "{e:#}");
    let row = sessions(&url)[3].clone();
    assert_eq!(
        (row["calls"].as_u64(), row["failed_calls"].as_u64()),
        (Some(1), Some(1)),
        "{row:#}"
    );
    // A seat registers no session, and its calls with none are the seat's own row.
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.open", "params": {}, "seat": "skipper"}),
    );
    assert_eq!(e["code"], json!("permission.denied"), "{e:#}");
    assert_eq!(sessions(&url).len(), 4);
    let o = post(
        &url,
        &json!({"id": 1, "method": "player.observe", "params": {"projection": "json"},
                "seat": "skipper"}),
    );
    assert_eq!(o["result"]["omniscient"], json!(false), "{o:#}");
    let rows = sessions(&url);
    assert_eq!(rows.len(), 5, "{rows:#?}");
    for (k, want) in [
        ("id", json!("player:skipper")),
        ("transport", json!("api")),
        ("role", json!("player")),
        ("seat", json!("skipper")),
        ("calls", json!(1)),
        ("live", json!(false)),
    ] {
        assert_eq!(rows[4][k], want, "{k}: {:#}", rows[4]);
    }

    // Closing: by the handle, which answers the row and may be said again.
    let closed = result(&url, "agent.session.close", json!({"handle": handle_a}));
    assert_eq!(
        (closed["id"].as_str(), closed["live"].as_bool()),
        (Some("mcp-1"), Some(false)),
        "{closed:#}"
    );
    assert_eq!(keys(&closed), ROW_KEYS);
    assert_eq!(
        result(&url, "agent.session.close", json!({"handle": handle_a})),
        closed
    );
    let e = error(&url, &as_session(&handle_a, "status", json!({})));
    assert_eq!(e["code"], json!("host.session_unknown"), "{e:#}");
    assert_eq!(
        told(&events(&url, json!({"session": "mcp-1"})))
            .last()
            .map(String::as_str),
        Some("closed")
    );
    // A closed bridge row answers again by its id too; rows that are no bridge's do not close.
    let again = result(&url, "agent.session.close", json!({"session": "mcp-2"}));
    assert_eq!(again["live"], json!(false), "{again:#}");
    let e = error(
        &url,
        &dev("agent.session.close", json!({"session": "player:skipper"})),
    );
    assert_eq!(e["code"], json!("request.not_applicable"), "{e:#}");
    assert_eq!(e["detail"]["path"], json!("/session"), "{e:#}");
    let e = error(
        &url,
        &dev("agent.session.close", json!({"session": "mcp-9"})),
    );
    assert_eq!(e["code"], json!("host.session_unknown"), "{e:#}");
    let e = error(&url, &dev("agent.session.close", json!({"handle": "0-9"})));
    assert_eq!(e["code"], json!("host.session_unknown"), "{e:#}");
    let e = error(
        &url,
        &dev(
            "agent.session.close",
            json!({"handle": handle_a, "session": "mcp-1"}),
        ),
    );
    assert_eq!(e["code"], json!("request.conflict"), "{e:#}");
    let e = error(&url, &dev("agent.session.close", json!({})));
    assert_eq!(e["code"], json!("request.missing_field"), "{e:#}");
    let live: Vec<bool> = sessions(&url)
        .iter()
        .map(|r| r["live"].as_bool().unwrap())
        .collect();
    assert_eq!(live, [false, false, true, true, false]);

    // No session ran a tick.
    assert_eq!(world(&url).0, before.0);

    let _ = stop.send(());
    rt.block_on(task).unwrap();
    drop(rt);
    drop(host);
    handle.shutdown(5000).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A child process, killed when the test ends however it ends.
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Process {
    /// Its exit code, within `within`.
    fn exit(&mut self, within: Duration) -> Option<i32> {
        let end = Instant::now() + within;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status.code();
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The lines a child writes to stdout, as they come.
fn lines(child: &mut Child) -> mpsc::Receiver<String> {
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                return;
            };
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    rx
}

/// `amoris mcp <project>` as an MCP client drives it: JSON-RPC lines on its stdin and stdout.
struct Bridge {
    process: Process,
    stdin: Option<ChildStdin>,
    out: mpsc::Receiver<String>,
    id: u64,
}

impl Bridge {
    fn start(project: &Path, more: &[&str]) -> Bridge {
        let mut child = Command::new(env!("CARGO_BIN_EXE_amoris"))
            .arg("mcp")
            .arg(project)
            .args(more)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = lines(&mut child);
        let stdin = child.stdin.take();
        Bridge {
            process: Process(child),
            stdin,
            out,
            id: 0,
        }
    }

    fn write(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{msg}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|e| panic!("no answer to {method}: {e}"));
            let r: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
            if r["id"] == json!(id) {
                return r;
            }
        }
    }

    fn initialize(&mut self, name: &str, version: &str) -> Value {
        let init = self.send(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": name, "version": version}}),
        );
        assert!(init["result"]["serverInfo"].is_object(), "{init:#}");
        self.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        init
    }

    /// A `tools/call`: whether it failed, and its text.
    fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self.send("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        assert!(res.is_object(), "{r:#}");
        (
            res["isError"].as_bool().unwrap_or(false),
            res["content"][0]["text"].as_str().unwrap_or("").to_owned(),
        )
    }

    /// The client closes the stream; the bridge's exit code.
    fn close(mut self) -> Option<i32> {
        self.stdin = None;
        self.process.exit(Duration::from_secs(20))
    }
}

#[test]
fn amoris_mcp_attached_to_a_host_is_a_session_of_its_own() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("agent-sessions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("course");
    copy_dir(&course(), &project);
    let mut child = Command::new(env!("CARGO_BIN_EXE_amoris"))
        .args(["serve", project.to_str().unwrap(), "--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let ready = lines(&mut child);
    let _served = Process(child);
    let line = ready
        .recv_timeout(Duration::from_secs(120))
        .expect("a readiness line");
    let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let url = ready["url"].as_str().unwrap().to_owned();
    assert_eq!(sessions(&url), Vec::<Value>::new());

    // The bridge registers when its client says who it is: listed before any tool call.
    let mut bridge = Bridge::start(&project, &[]);
    bridge.initialize("bridge-test", "9");
    let rows = sessions(&url);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    for (k, want) in [
        ("id", json!("mcp-1")),
        ("transport", json!("bridge")),
        ("role", json!("developer")),
        ("label", json!("bridge-test 9")),
        ("calls", json!(0)),
        ("live", json!(true)),
    ] {
        assert_eq!(rows[0][k], want, "{k}: {:#}", rows[0]);
    }
    // Its edit is its own in the history and in the stream, not the shared API client's.
    let (failed, text) = bridge.tool("world", set_cargo(61));
    assert!(!failed, "{text}");
    assert_eq!(history_sessions(&url), [json!("mcp-1")]);
    assert_eq!(
        told(&events(&url, json!({}))),
        ["opened", "call world.edit", "result world.edit"]
    );
    // The client closes the stream: the bridge closes its session and leaves.
    assert_eq!(bridge.close(), Some(0));
    let rows = sessions(&url);
    assert_eq!(rows[0]["live"], json!(false), "{rows:#?}");
    assert_eq!(rows[0]["calls"], json!(1), "{rows:#?}");
    assert_eq!(
        told(&events(&url, json!({}))).last().map(String::as_str),
        Some("closed")
    );

    // With a seat it is that seat's player.
    let mut player = Bridge::start(&project, &["--seat", "skipper"]);
    let init = player.initialize("seat-test", "1");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .is_some_and(|i| i.starts_with("You play one seat")),
        "{init:#}"
    );
    let rows = sessions(&url);
    assert_eq!(rows.len(), 2, "{rows:#?}");
    for (k, want) in [
        ("id", json!("mcp-2")),
        ("transport", json!("bridge")),
        ("role", json!("player")),
        ("seat", json!("skipper")),
        ("label", json!("seat-test 1")),
        ("live", json!(true)),
    ] {
        assert_eq!(rows[1][k], want, "{k}: {:#}", rows[1]);
    }
    let (failed, text) = player.tool("player", json!({"action": "observe"}));
    assert!(!failed && text.starts_with("tick "), "{text}");
    let (failed, text) = player.tool("world", json!({"action": "tree"}));
    assert!(failed && text.contains("permission.denied"), "{text}");
    assert_eq!(player.close(), Some(0));
    let rows = sessions(&url);
    assert_eq!(
        (rows[1]["live"].as_bool(), rows[1]["calls"].as_u64()),
        (Some(false), Some(1)),
        "{rows:#?}"
    );

    drop(_served);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The events of one session, in order: `kind method` or `kind` (a message with its stream).
fn session_told(url: &str, session: &str) -> Vec<Value> {
    events(url, json!({"session": session, "limit": 2000}))["events"]
        .as_array()
        .unwrap()
        .clone()
}

/// Waits until the session's row is no longer live; its events then.
fn ended(url: &str, session: &str) -> Vec<Value> {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(30) {
        let live = sessions(url)
            .iter()
            .any(|r| r["id"] == json!(session) && r["live"] == json!(true));
        if !live {
            return session_told(url, session);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{session} did not end: {:#?}", session_told(url, session));
}

/// The test's agent programs as the owner of a Unix machine registers them: shell scripts, a
/// sleeper and a program that is not there.
#[cfg(not(windows))]
fn programs(bin: &str) -> Value {
    let script = r#"echo "goal: $AMORIS_AGENT_GOAL"
curl -s -X POST "$AMORIS_HOST/api/call" -H 'Content-Type: application/json' \
  -d "{\"id\":1,\"method\":\"status\",\"params\":{},\"handle\":\"$AMORIS_AGENT_HANDLE\"}" >/dev/null
echo "seat: ${AMORIS_AGENT_SEAT:-none}"
echo "to stderr" >&2"#;
    let mcp = format!(
        r#"(printf '%s\n' \
  '{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"fake","version":"1"}}}}}}' \
  '{{"jsonrpc":"2.0","method":"notifications/initialized"}}' \
  '{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"player","arguments":{{"action":"session"}}}}}}'; sleep 3) \
  | "{bin}" mcp --attach "$AMORIS_HOST" >/dev/null"#
    );
    json!([
        {"name": "scripted", "label": "a scripted agent", "command": ["/bin/sh", "-c", script]},
        {"name": "mcp", "command": ["/bin/sh", "-c", mcp]},
        {"name": "sleeper", "command": ["/bin/sleep", "30"]},
        {"name": "missing", "command": ["/no/such/program"]},
    ])
}

/// The same programs as the owner of a Windows machine registers them: Windows PowerShell, which
/// every Windows has, in place of the shell, curl and sleep. It hands its pipeline to a program
/// only once the pipeline ends, so the bridge's stdin is held open through a process of its own.
#[cfg(windows)]
fn programs(bin: &str) -> Value {
    let ps = |script: &str| {
        json!([
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script
        ])
    };
    let script = "$ProgressPreference = 'SilentlyContinue'; \
        Write-Output ('goal: ' + $env:AMORIS_AGENT_GOAL); \
        $body = ConvertTo-Json -Compress @{id = 1; method = 'status'; params = @{}; \
                                           handle = $env:AMORIS_AGENT_HANDLE}; \
        Invoke-RestMethod -Method Post -Uri ($env:AMORIS_HOST + '/api/call') \
          -ContentType 'application/json' -Body $body | Out-Null; \
        $seat = $env:AMORIS_AGENT_SEAT; if (-not $seat) { $seat = 'none' }; \
        Write-Output ('seat: ' + $seat); \
        [Console]::Error.WriteLine('to stderr')";
    let lines = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fake","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"player","arguments":{"action":"session"}}}"#,
    ]
    .map(|l| format!("'{l}'"))
    .join(", ");
    let mcp = format!(
        "$p = New-Object System.Diagnostics.Process; $s = $p.StartInfo; $s.FileName = '{bin}'; \
         $s.Arguments = 'mcp --attach ' + $env:AMORIS_HOST; $s.UseShellExecute = $false; \
         $s.RedirectStandardInput = $true; $s.RedirectStandardOutput = $true; \
         [void]$p.Start(); $out = $p.StandardOutput.ReadToEndAsync(); \
         foreach ($l in @({lines})) {{ $p.StandardInput.WriteLine($l) }}; \
         $p.StandardInput.Flush(); Start-Sleep 3; $p.StandardInput.Close(); $p.WaitForExit(); \
         [void]$out.Result"
    );
    json!([
        {"name": "scripted", "label": "a scripted agent", "command": ps(script)},
        {"name": "mcp", "command": ps(&mcp)},
        {"name": "sleeper", "command": ps("Start-Sleep 30")},
        {"name": "missing", "command": ["/no/such/program"]},
    ])
}

/// The process owner registers agent programs (`amoris serve --agents`, server.md 3.7) and the
/// editor starts one by name with a goal: a session of transport `launch`, opened before the
/// program runs, whose environment names the host, the session's handle and the goal; its
/// output lines are the session's `message` events, its calls through the handle are the
/// session's, and the session closes when it exits or is stopped. `amoris mcp --attach` run by
/// such a program calls as the session the host gave it.
#[test]
fn the_host_starts_a_registered_agent_program_as_a_session() {
    let (handle, host) = start();
    let file = json!({"agents": programs(env!("CARGO_BIN_EXE_amoris"))});
    host.set_launchers(amoris_server::Launchers::parse(&file.to_string(), "test").unwrap());
    let rt = runtime();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (addr, task) = rt
        .block_on(host.serve(&ServeOptions { port: 0 }, async {
            let _ = stopped.await;
        }))
        .unwrap();
    let url = format!("http://{addr}");
    let before = result(&url, "status", json!({}))["world_hash"].clone();

    let listed = result(&url, "agent.session.agents", json!({}));
    assert_eq!(
        listed["agents"][0],
        json!({"name": "scripted", "label": "a scripted agent"}),
        "{listed:#}"
    );
    assert!(
        listed["agents"][0].get("command").is_none(),
        "commands are not told"
    );

    // A program the file names, with a goal.
    let row = result(
        &url,
        "agent.session.start",
        json!({"agent": "scripted", "goal": "Report the tick."}),
    );
    assert_eq!(row["transport"], json!("launch"), "{row:#}");
    assert_eq!(row["label"], json!("a scripted agent"), "{row:#}");
    assert_eq!(row["live"], json!(true), "{row:#}");
    assert!(
        row.get("handle").is_none(),
        "the handle is the program's alone: {row:#}"
    );
    let id = row["id"].as_str().unwrap().to_owned();
    let told = ended(&url, &id);
    let words: Vec<String> = told
        .iter()
        .map(
            |e| match (e["kind"].as_str().unwrap(), e["stream"].as_str()) {
                ("message", Some(s)) => format!("{s}: {}", e["summary"].as_str().unwrap()),
                (k, _) => format!("{k} {}", e["method"].as_str().unwrap_or(""))
                    .trim()
                    .to_owned(),
            },
        )
        .collect();
    assert_eq!(words[0], "opened", "{words:#?}");
    assert!(
        words[1].starts_with("host: started scripted (pid "),
        "{words:#?}"
    );
    for want in [
        "stdout: goal: Report the tick.",
        "call status",
        "result status",
        "stdout: seat: none",
        "stderr: to stderr",
        "host: exit status: 0",
        "closed",
    ] {
        assert!(words.iter().any(|w| w == want), "{want}: {words:#?}");
    }
    assert_eq!(words.last().unwrap(), "closed", "{words:#?}");

    // `amoris mcp --attach` run by the program is the session the host opened, as its seat.
    let row = result(
        &url,
        "agent.session.start",
        json!({"agent": "mcp", "goal": "Say your seat.", "seat": "skipper"}),
    );
    assert_eq!(
        (row["role"].clone(), row["seat"].clone()),
        (json!("player"), json!("skipper"))
    );
    let id = row["id"].as_str().unwrap().to_owned();
    let told = ended(&url, &id);
    assert!(
        told.iter()
            .any(|e| e["kind"] == json!("call") && e["method"] == json!("player.session")),
        "{told:#?}"
    );
    assert!(
        told.iter().any(|e| e["kind"] == json!("result")
            && e["ok"] == json!(true)
            && e["summary"]
                .as_str()
                .unwrap()
                .contains("\"seat\":\"skipper\"")),
        "the seat's own session: {told:#?}"
    );
    let rows = sessions(&url);
    assert_eq!(
        rows.iter()
            .filter(|r| r["transport"] == json!("bridge"))
            .count(),
        0,
        "it registered no session of its own: {rows:#?}"
    );

    // Stop ends a program that would run on.
    let row = result(
        &url,
        "agent.session.start",
        json!({"agent": "sleeper", "goal": "Wait."}),
    );
    let id = row["id"].as_str().unwrap().to_owned();
    let stopped_row = result(&url, "agent.session.stop", json!({"session": id}));
    assert_eq!(stopped_row["live"], json!(false), "{stopped_row:#}");
    let told = session_told(&url, &id);
    assert!(
        told.iter().any(|e| e["summary"] == json!("stopping")),
        "{told:#?}"
    );

    // What the host refuses: an unregistered name, an empty goal, a seat the game lacks, a program
    // that does not start (its session opened and closed, saying why).
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.start",
                               "params": {"agent": "scriptd", "goal": "x"}}),
    );
    assert_eq!(e["code"], json!("request.invalid_value"), "{e:#}");
    assert_eq!(e["detail"]["suggestions"][0], json!("scripted"), "{e:#}");
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.start",
                               "params": {"agent": "scripted", "goal": "  "}}),
    );
    assert_eq!(e["code"], json!("request.out_of_range"), "{e:#}");
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.start",
                               "params": {"agent": "scripted", "goal": "x", "seat": "nobody"}}),
    );
    assert_eq!(e["code"], json!("seat.unknown"), "{e:#}");
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.start",
                               "params": {"agent": "missing", "goal": "x"}}),
    );
    assert_eq!(e["code"], json!("host.agent_failed"), "{e:#}");
    let e = error(
        &url,
        &json!({"id": 1, "method": "agent.session.stop",
                               "params": {"session": "mcp-99"}}),
    );
    assert_eq!(e["code"], json!("host.session_unknown"), "{e:#}");

    // Host state: the world is as it was.
    assert_eq!(result(&url, "status", json!({}))["world_hash"], before);
    let _ = stop.send(());
    let _ = rt.block_on(task);
    drop(host);
    handle.shutdown(5000).unwrap();
}
