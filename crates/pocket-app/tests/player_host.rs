//! Calls made as a seat through the host (docs/spec/player.md 7; shared/contract/mcp.md 3.3), on
//! the sailing course served as `pocket serve` serves it: `/api/call` with `seat` and a seat-bound
//! MCP session play the seat's player, through the player tools alone. Every other method is
//! refused with `permission.denied`, whether the game answers it (`world.get`, `world.edit`,
//! `time.step`) or the host does (`events.since`, which lists every world event), and the world
//! does not change; the omniscient view is refused; an unknown seat says so. The MCP session
//! lists the `player` tool alone, refuses the others, and its caller is refused developer methods
//! by the host even past the MCP layer's own check.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use pocket_link::Source;
use pocket_mcp::Backend as _;
use pocket_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use pocket_runtime::{GameBuilder, Project};
use pocket_server::{GameAccess, Host, McpBackend, ServeOptions, Via};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn course() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing-course")
}

/// The sailing course on a game thread started as `pocket serve` starts it (real time at speed 1,
/// paused: the edit world), behind a host that opens player clients.
fn start() -> (GameHandle, Host) {
    let root = course();
    let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
    let t0 = Instant::now();
    let clock: pocket_runtime::thread::Clock =
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

/// `POST /api/call` with `body`: the answer, `{id, result}` or `{id, error}`.
fn post(url: &str, body: &Value) -> Value {
    let mut r = ureq::post(format!("{url}/api/call"))
        .content_type("application/json")
        .send(&body.to_string())
        .unwrap();
    serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap()
}

fn result(url: &str, body: &Value) -> Value {
    let a = post(url, body);
    assert!(a.get("error").is_none(), "{body}: {a:#}");
    a["result"].clone()
}

fn error(url: &str, body: &Value) -> Value {
    let a = post(url, body);
    assert!(a.get("result").is_none(), "{body}: {a:#}");
    a["error"].clone()
}

#[test]
fn a_seat_over_http_plays_through_the_player_tools_alone() {
    let (handle, host) = start();
    let rt = runtime();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (addr, task) = rt
        .block_on(host.serve(&ServeOptions { port: 0 }, async {
            let _ = stopped.await;
        }))
        .unwrap();
    let url = format!("http://{addr}");
    let seat = |method: &str, params: Value| json!({"id": 1, "method": method, "params": params, "seat": "skipper"});
    let dev = |method: &str, params: Value| json!({"id": 1, "method": method, "params": params});

    // The seat's player: its role and seat, its perception, no omniscient view.
    let s = result(&url, &seat("player.session", json!({})));
    assert_eq!(
        (s["role"].clone(), s["seat"].clone()),
        (json!("player"), json!("skipper")),
        "{s:#}"
    );
    let o = result(&url, &seat("player.observe", json!({"projection": "json"})));
    assert_eq!(o["omniscient"], json!(false), "{o:#}");
    let e = error(&url, &seat("player.observe", json!({"omniscient": true})));
    assert_eq!(e["code"], json!("perception.omniscient_forbidden"), "{e:#}");
    let w = result(&url, &seat("player.wait", json!({"ticks": 2})));
    assert!(
        w.get("world_hash").is_none(),
        "a player sees no hash: {w:#}"
    );
    let e = error(&url, &seat("player.describe", json!({"entity": "Crate4"})));
    assert_eq!(e["code"], json!("perception.unknown_entity"), "{e:#}");

    // A developer's methods, the game's and the host's, are refused and change nothing.
    let before = result(
        &url,
        &dev(
            "world.get",
            json!({"entity": "Crate4", "components": ["Cargo"]}),
        ),
    );
    let status = result(&url, &dev("status", json!({})));
    for (method, params) in [
        ("world.get", json!({"entity": "Crate4"})),
        ("world.query", json!({"with": ["Perceivable"]})),
        ("world.tree", json!({})),
        (
            "world.edit",
            json!({"ops": [{"set": {"entity": "Crate4", "component": "Cargo",
                                     "value": {"value": 99}}}]}),
        ),
        ("time.step", json!({"ticks": 5})),
        ("time.control", json!({"pause": false})),
        ("snapshots.restore", json!({"tick": 0})),
        ("status", json!({})),
        ("events.since", json!({})),
        ("log.since", json!({})),
        ("catalog.list", json!({})),
        ("debug.rewind", json!({"tick": 0})),
    ] {
        let e = error(&url, &seat(method, params));
        assert_eq!(e["code"], json!("permission.denied"), "{method}: {e:#}");
        assert_eq!(e["detail"]["role"], json!("player"), "{method}: {e:#}");
    }
    let after = result(
        &url,
        &dev(
            "world.get",
            json!({"entity": "Crate4", "components": ["Cargo"]}),
        ),
    );
    assert_eq!(after, before);
    let now = result(&url, &dev("status", json!({})));
    assert_eq!(
        (now["tick"].clone(), now["world_hash"].clone()),
        (status["tick"].clone(), status["world_hash"].clone())
    );

    // An unknown seat, and a seat that is not a name.
    let e = error(
        &url,
        &json!({"id": 1, "method": "player.observe", "params": {}, "seat": "lookout"}),
    );
    assert_eq!(e["code"], json!("seat.unknown"), "{e:#}");
    let e = error(
        &url,
        &json!({"id": 1, "method": "player.observe", "params": {}, "seat": 0}),
    );
    assert_eq!(e["code"], json!("request.wrong_type"), "{e:#}");

    let _ = stop.send(());
    rt.block_on(task).unwrap();
    drop(host);
    handle.shutdown(5000).unwrap();
}

/// One MCP client over an in-process pipe: requests as newline-delimited JSON-RPC.
struct McpClient {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    read: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    id: u64,
}

impl McpClient {
    async fn send(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        self.write
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.read.read_line(&mut line).await.unwrap() > 0,
                "the session closed"
            );
            let r: Value = serde_json::from_str(&line).unwrap();
            if r["id"] == json!(self.id) {
                return r;
            }
        }
    }

    async fn notify(&mut self, method: &str) {
        let msg = json!({"jsonrpc": "2.0", "method": method});
        self.write
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
    }

    /// A `tools/call`: whether it failed, and its text.
    async fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self
            .send("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let res = &r["result"];
        (
            res["isError"].as_bool().unwrap_or(false),
            res["content"][0]["text"].as_str().unwrap_or("").to_owned(),
        )
    }
}

#[test]
fn a_seat_bound_mcp_session_lists_and_calls_the_player_tool_alone() {
    let (handle, host) = start();
    let rt = runtime();
    rt.block_on(async {
        let backend: Arc<dyn pocket_mcp::Backend> =
            Arc::new(McpBackend::player(host.clone(), "skipper"));
        let (client, server) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server);
        let session = tokio::spawn(pocket_mcp::serve_io(backend.clone(), sr, sw));
        let (cr, cw) = tokio::io::split(client);
        let mut c = McpClient {
            write: cw,
            read: BufReader::new(cr),
            id: 0,
        };
        let init = c
            .send(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "test", "version": "0"}}),
            )
            .await;
        let instructions = init["result"]["instructions"].as_str().unwrap_or("");
        assert!(instructions.starts_with("You play one seat"), "{init:#}");
        c.notify("notifications/initialized").await;
        let tools = c.send("tools/list", json!({})).await;
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, ["player"], "{tools:#}");
        let (failed, text) = c
            .tool("world", json!({"action": "get", "entity": "Crate4"}))
            .await;
        assert!(failed && text.contains("permission.denied"), "{text}");
        let (failed, text) = c.tool("player", json!({"action": "observe"})).await;
        assert!(!failed && text.starts_with("tick "), "{text}");
        let (failed, text) = c
            .tool("player", json!({"action": "observe", "omniscient": true}))
            .await;
        assert!(
            failed && text.contains("perception.omniscient_forbidden"),
            "{text}"
        );
        let (failed, text) = c.tool("player", json!({"action": "session"})).await;
        assert!(!failed && text.contains("\"role\":\"player\""), "{text}");
        drop(c);
        let _ = session.await;

        // Past the MCP layer's own check, the host refuses the session's caller a developer's
        // methods: the game's and its own.
        let caller = backend.open();
        assert_eq!(caller.seat().as_deref(), Some("skipper"));
        for (method, params) in [
            ("world.get", json!({"entity": "Crate4"})),
            ("time.step", json!({"ticks": 1})),
            ("events.since", json!({})),
        ] {
            let e = caller.call(method.to_owned(), params).await.unwrap_err();
            assert_eq!(e.code, "permission.denied", "{method}: {e:#?}");
        }
        let o = caller
            .call("player.observe".to_owned(), json!({"projection": "json"}))
            .await
            .unwrap();
        assert_eq!(o["omniscient"], json!(false), "{o:#}");
        // A session bound to a seat the game does not declare says so on its first call.
        let lost = McpBackend::player(host.clone(), "lookout").open();
        let e = lost
            .call("player.observe".to_owned(), json!({}))
            .await
            .unwrap_err();
        assert_eq!(e.code, "seat.unknown", "{e:#?}");
        // A developer's session is answered the same read.
        let dev = host
            .call(&Via::Api, "world.get", json!({"entity": "Crate4"}))
            .await
            .unwrap();
        assert!(dev["components"]["Cargo"].is_object(), "{dev:#}");
    });
    drop(host);
    handle.shutdown(5000).unwrap();
}
