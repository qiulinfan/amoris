//! A host started with grants is locked (docs/spec/server.md 3.6; shared/contract/mcp.md 14), on
//! the sailing course served as `amoris serve` serves it. Each token binds its client to one role,
//! a seat's player, a checker or a developer, and no client raises its role by asking: every route
//! needs a registered token, a missing and an unknown one are refused alike; a player's token is
//! its seat whatever the request says, over HTTP and MCP; a checker's calls are the catalog's
//! reads alone; a developer's everything. A granted client is a session of the host's list under
//! its grant's label. A host without grants takes no token and behaves as before. The lock is
//! host state: a locked and an unlocked host of one project and seed hash the same world.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use amoris_contract::{Problem, detail};
use amoris_link::Source;
use amoris_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use amoris_runtime::{GameBuilder, Project};
use amoris_server::{GameAccess, Grants, Host, ServeOptions, Via};
use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;

const PLAYER: &str = "0123456789abcdef0123456789abcdef";
const CHECKER: &str = "fedcba9876543210fedcba9876543210";
const DEVELOPER: &str = "00112233445566778899aabbccddeeff";
/// 32 hex digits no grant has.
const UNKNOWN: &str = "11111111111111111111111111111111";
const TOKENS: [&str; 3] = [PLAYER, CHECKER, DEVELOPER];

/// The grants file of these tests: the skipper's agent, a harness that checks, and its setup.
fn grants_file() -> String {
    json!({"grants": [
        {"token": PLAYER, "role": "player", "seat": "skipper", "label": "agent"},
        {"token": CHECKER, "role": "checker", "label": "harness"},
        {"token": DEVELOPER, "role": "developer", "label": "setup"},
    ]})
    .to_string()
}

fn course() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing-course")
}

/// A debugger that is installed and answers nothing: the catalog then lists the debugger's
/// methods with their kinds, as under `amoris serve`, which is what a checker is gated by.
struct StubDebugger;

impl amoris_server::DebugHub for StubDebugger {
    fn call(&self, method: &str, _: Value) -> amoris_server::BoxFuture<Result<Value, Problem>> {
        let method = method.to_owned();
        Box::pin(async move {
            Err(Problem::new(
                "debug.not_available",
                format!("{method} reached the test's debugger, which answers nothing."),
                detail([]),
            ))
        })
    }

    fn methods(&self) -> Vec<Value> {
        amoris_debug::methods()
            .iter()
            .map(amoris_debug::Method::entry)
            .collect()
    }
}

/// The sailing course on a game thread started as `amoris serve` starts it (real time at speed 1,
/// paused: the edit world), behind a host that opens player clients and has a debugger.
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
    host.set_debug(Arc::new(StubDebugger));
    (handle, host)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A served host of these tests, locked with [`grants_file`] or not, stopped when the test ends.
struct Served {
    rt: tokio::runtime::Runtime,
    host: Host,
    handle: Option<GameHandle>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    addr: SocketAddr,
    url: String,
}

impl Served {
    fn start(locked: bool) -> Served {
        Served::with(locked.then(grants_file))
    }

    /// A host locked with the grants file `grants`, or not locked.
    fn with(grants: Option<String>) -> Served {
        let (handle, host) = start();
        let rt = runtime();
        let locked = grants.is_some();
        if let Some(file) = grants {
            rt.block_on(host.lock(Grants::parse(&file).unwrap()))
                .unwrap();
        }
        assert_eq!(host.locked(), locked);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (addr, task) = rt
            .block_on(host.serve(&ServeOptions { port: 0 }, async {
                let _ = stopped.await;
            }))
            .unwrap();
        Served {
            rt,
            host,
            handle: Some(handle),
            stop: Some(stop),
            task: Some(task),
            addr,
            url: format!("http://{addr}"),
        }
    }

    /// A call past every token: the process owner's, through the host itself.
    fn owner(&self, method: &str, params: Value) -> Value {
        self.rt
            .block_on(self.host.call(&Via::Api, method, params))
            .unwrap_or_else(|p| panic!("{method}: {p:#?}"))
    }

    /// The world as a change would show in it: its tick, its hash, its count of writes, and the
    /// crate the tests try to change.
    fn world(&self) -> Value {
        let status = self.owner("status", json!({}));
        json!({
            "tick": status["tick"],
            "world_hash": status["world_hash"],
            "writes": self.owner("snapshot", json!({}))["writes"],
            "crate": self.owner("world.get", json!({"entity": "Crate4", "components": ["Cargo"]})),
        })
    }

    /// The grants' rows, and whatever rows follow them.
    fn rows(&self) -> Vec<Value> {
        self.owner("agent.session.list", json!({}))["sessions"]
            .as_array()
            .unwrap()
            .clone()
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = self.rt.block_on(task);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown(5000);
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// An HTTP answer: its status, its `WWW-Authenticate` and its body.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: u16,
    challenge: Option<String>,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    /// The problem of an answer `{error}` or `{id, error}`.
    fn error(&self) -> Value {
        let v = self.json();
        assert!(v.get("result").is_none(), "{}", self.body);
        v["error"].clone()
    }
}

fn answer(mut r: ureq::http::Response<ureq::Body>) -> Answer {
    Answer {
        status: r.status().as_u16(),
        challenge: r
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        body: r.body_mut().read_to_string().unwrap(),
    }
}

/// `GET <url>` with `headers`.
fn get(url: &str, headers: &[(&str, &str)]) -> Answer {
    let mut req = agent().get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    answer(req.call().unwrap())
}

/// `POST <url>` with `headers` and a JSON `body`.
fn post(url: &str, headers: &[(&str, &str)], body: &str) -> Answer {
    let mut req = agent().post(url).content_type("application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    answer(req.send(body).unwrap())
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// `POST /api/call` with `body` and `token` as its bearer: the answer, `{id, result}` or
/// `{id, error}`.
fn call(url: &str, token: &str, body: &Value) -> Value {
    let a = post(
        &format!("{url}/api/call"),
        &[("Authorization", &bearer(token))],
        &body.to_string(),
    );
    assert_eq!(a.status, 200, "{body}: {}", a.body);
    a.json()
}

fn request(method: &str, params: Value) -> Value {
    json!({"id": 1, "method": method, "params": params})
}

fn result(url: &str, token: &str, method: &str, params: Value) -> Value {
    let a = call(url, token, &request(method, params));
    assert!(a.get("error").is_none(), "{method}: {a:#}");
    a["result"].clone()
}

fn error(url: &str, token: &str, body: &Value) -> Value {
    let a = call(url, token, body);
    assert!(a.get("result").is_none(), "{body}: {a:#}");
    a["error"].clone()
}

/// A WebSocket upgrade of `path` with `headers`: the socket, or the HTTP answer that refused it.
fn upgrade(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>, Answer> {
    let mut req = format!("ws://{addr}{path}").into_client_request().unwrap();
    for (k, v) in headers {
        req.headers_mut().insert(
            tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    match tungstenite::connect(req) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(r)) => Err(Answer {
            status: r.status().as_u16(),
            challenge: r
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            body: String::from_utf8_lossy(r.body().as_deref().unwrap_or_default()).into_owned(),
        }),
        Err(e) => panic!("{path}: {e}"),
    }
}

/// A WebSocket upgrade the host is to refuse: the HTTP answer that refused it.
fn refused_upgrade(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Answer {
    match upgrade(addr, path, headers) {
        Ok(_) => panic!("the upgrade of {path} was accepted"),
        Err(a) => a,
    }
}

/// One answer of an MCP request: the HTTP status, the session id it named, and the JSON-RPC
/// message (from the event stream's `data:` lines, or the body itself).
struct McpAnswer {
    status: u16,
    session: Option<String>,
    message: Value,
    body: String,
}

/// `POST <url><path>` with the JSON-RPC `message` and `headers` beside the ones every MCP
/// request carries.
fn mcp_post(url: &str, path: &str, message: &Value, headers: &[(&str, &str)]) -> McpAnswer {
    let mut req = agent()
        .post(format!("{url}{path}"))
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

/// One MCP client over HTTP: where it posts, what it presents, and its session once it has one.
struct McpHttp {
    url: String,
    path: String,
    /// The bearer, when the token is not in the path.
    token: Option<String>,
    session: Option<String>,
    id: u64,
}

impl McpHttp {
    /// A client that presents `token` as its bearer on `/mcp`.
    fn by_header(url: &str, token: &str) -> McpHttp {
        McpHttp {
            url: url.to_owned(),
            path: "/mcp".to_owned(),
            token: Some(token.to_owned()),
            session: None,
            id: 0,
        }
    }

    /// A client that cannot set a header: `/mcp/<token>`.
    fn by_path(url: &str, token: &str) -> McpHttp {
        McpHttp {
            url: url.to_owned(),
            path: format!("/mcp/{token}"),
            token: None,
            session: None,
            id: 0,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> McpAnswer {
        self.id += 1;
        let message = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let bearer = self.token.as_deref().map(bearer);
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(b) = &bearer {
            headers.push(("Authorization", b));
        }
        if let Some(s) = &self.session {
            headers.push(("Mcp-Session-Id", s));
        }
        mcp_post(&self.url, &self.path, &message, &headers)
    }

    /// `initialize` as `name version`; the instructions the server gave.
    fn initialize(&mut self, name: &str) -> String {
        let init = self.send(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": name, "version": "1"}}),
        );
        assert_eq!(init.status, 200, "{}", init.body);
        self.session = Some(init.session.clone().expect("an Mcp-Session-Id"));
        let noted = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        let bearer = self.token.as_deref().map(bearer);
        let mut headers: Vec<(&str, &str)> =
            vec![("Mcp-Session-Id", self.session.as_deref().unwrap())];
        if let Some(b) = &bearer {
            headers.push(("Authorization", b));
        }
        let noted = mcp_post(&self.url, &self.path, &noted, &headers);
        assert_eq!(noted.status, 202, "{}", noted.body);
        init.message["result"]["instructions"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }

    fn tools(&mut self) -> Vec<String> {
        let listed = self.send("tools/list", json!({}));
        listed.message["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("HTTP {}: {}", listed.status, listed.body))
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_owned))
            .collect()
    }

    /// A `tools/call`: whether it failed, and its text.
    fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let a = self.send("tools/call", json!({"name": name, "arguments": args}));
        let res = &a.message["result"];
        assert!(res.is_object(), "HTTP {}: {}", a.status, a.body);
        (
            res["isError"].as_bool().unwrap_or(false),
            res["content"][0]["text"].as_str().unwrap_or("").to_owned(),
        )
    }
}

/// One raw HTTP/1.1 request: the status line and everything after it (a client library would
/// not send another `Host`).
fn raw(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

fn set_cargo(value: u64) -> Value {
    json!({"ops": [{"set": {"entity": "Crate4", "component": "Cargo", "value": {"value": value}}}]})
}

/// Asserts that `text` says none of the grants' tokens, in either case.
fn says_no_token(text: &str, what: &str) {
    for t in TOKENS {
        assert!(!text.contains(t), "{what} says a token: {text}");
        assert!(
            !text.contains(&t.to_uppercase()),
            "{what} says a token: {text}"
        );
    }
}

#[test]
fn a_locked_host_admits_registered_tokens_alone() {
    let served = Served::start(true);
    let (url, addr) = (served.url.clone(), served.addr);
    let before = served.world();
    let status = request("status", json!({})).to_string();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                      "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "probe", "version": "1"}}})
    .to_string();
    // A body that offers a token, and says it is a developer's.
    let in_body = json!({"id": 1, "method": "status", "params": {"token": DEVELOPER},
                         "token": DEVELOPER, "role": "developer",
                         "authorization": bearer(DEVELOPER)})
    .to_string();

    // What is no admission: no token, 32 hex digits no grant has, a token cut short or made
    // long, one that is not hex, one bit off a registered one, another scheme, a bare token.
    let one_off = format!("{}e", &DEVELOPER[..31]);
    let credentials: Vec<Vec<(&str, String)>> = vec![
        vec![],
        vec![("Authorization", bearer(UNKNOWN))],
        vec![("Authorization", bearer(&DEVELOPER[..31]))],
        vec![("Authorization", bearer(&format!("{DEVELOPER}0")))],
        vec![("Authorization", bearer("zz112233445566778899aabbccddeeff"))],
        vec![("Authorization", bearer(&one_off))],
        vec![("Authorization", bearer(""))],
        vec![("Authorization", format!("Basic {DEVELOPER}"))],
        vec![("Authorization", DEVELOPER.to_owned())],
        vec![("X-Amoris-Token", DEVELOPER.to_owned())],
        vec![("Cookie", format!("token={DEVELOPER}"))],
    ];
    let mut refusals: Vec<Answer> = Vec::new();
    for given in &credentials {
        let headers: Vec<(&str, &str)> = given.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let what = format!("{given:?}");
        for path in [
            "/api/call",
            "/api/catalog",
            "/api/grant",
            "/assets/scene.json",
            "/assets/project.toml",
            "/wasm/x",
            "/",
            "/index.html",
            "/render",
            "/ws",
            "/mcp",
            "/nowhere/at/all",
        ] {
            let a = get(&format!("{url}{path}"), &headers);
            assert_eq!(a.status, 401, "GET {path} with {what}: {}", a.body);
            refusals.push(a);
        }
        for (path, body) in [
            ("/api/call", status.as_str()),
            ("/api/call", in_body.as_str()),
            ("/api/call", "not json"),
            ("/api/catalog", status.as_str()),
            ("/mcp", init.as_str()),
        ] {
            let a = post(&format!("{url}{path}"), &headers, body);
            assert_eq!(a.status, 401, "POST {path} with {what}: {}", a.body);
            refusals.push(a);
        }
        let unknown_path = format!("/mcp/{UNKNOWN}");
        let a = post(&format!("{url}{unknown_path}"), &headers, &init);
        assert_eq!(a.status, 401, "POST /mcp/<unknown> with {what}: {}", a.body);
        refusals.push(a);
        for path in ["/ws", "/render"] {
            let a = refused_upgrade(addr, path, &headers);
            assert_eq!(a.status, 401, "an upgrade of {path} with {what}");
            // tungstenite keeps a refusal's headers; its body is checked by the GETs above.
            assert_eq!(a.challenge.as_deref(), Some("Bearer"), "{path} with {what}");
        }
    }
    // A token in the query is no token, whatever it is called.
    for query in ["token", "access_token", "grant", "authorization"] {
        for path in ["/api/catalog", "/api/grant", "/mcp", "/"] {
            let a = get(&format!("{url}{path}?{query}={DEVELOPER}"), &[]);
            assert_eq!(a.status, 401, "GET {path}?{query}=: {}", a.body);
            refusals.push(a);
        }
        let a = post(&format!("{url}/api/call?{query}={DEVELOPER}"), &[], &status);
        assert_eq!(a.status, 401, "POST /api/call?{query}=: {}", a.body);
        refusals.push(a);
    }
    // A path's token and a bearer that are two grants' present nothing; nor does a registered
    // token where no route reads one.
    let a = post(
        &format!("{url}/mcp/{PLAYER}"),
        &[("Authorization", &bearer(CHECKER))],
        &init,
    );
    assert_eq!(a.status, 401, "{}", a.body);
    refusals.push(a);
    for path in [
        format!("/api/call/{DEVELOPER}"),
        format!("/{DEVELOPER}"),
        format!("/mcp/{DEVELOPER}/x"),
        format!("/assets/{DEVELOPER}"),
    ] {
        let a = get(&format!("{url}{path}"), &[]);
        assert_eq!(a.status, 401, "GET a path with a token in it: {}", a.body);
        refusals.push(a);
    }

    // Every refusal is the same answer, byte for byte: nothing tells a client what was wrong
    // with what it presented, or what the route would have been.
    let first = &refusals[0];
    assert_eq!(first.challenge.as_deref(), Some("Bearer"));
    assert_eq!(
        first.error()["code"],
        json!("host.unauthorized"),
        "{}",
        first.body
    );
    assert_eq!(first.error()["detail"], json!({}), "{}", first.body);
    says_no_token(&first.body, "the refusal");
    assert!(refusals.len() > 200, "{}", refusals.len());
    for a in &refusals {
        assert_eq!(a, first);
    }

    // The loopback guard stands before the admission: a registered token from another Host is
    // refused as any request from there.
    let rebound = raw(
        addr,
        &format!(
            "GET /api/grant HTTP/1.1\r\nHost: evil.example\r\nAuthorization: Bearer {DEVELOPER}\r\n\
             Connection: close\r\n\r\n"
        ),
    );
    assert!(rebound.starts_with("HTTP/1.1 403"), "{rebound}");
    assert!(rebound.contains("host.forbidden"), "{rebound}");
    let with_origin = get(
        &format!("{url}/api/grant"),
        &[
            ("Authorization", &bearer(DEVELOPER)),
            ("Origin", "http://evil.example"),
        ],
    );
    assert_eq!(with_origin.status, 403, "{}", with_origin.body);
    assert_eq!(with_origin.error()["code"], json!("host.forbidden"));
    // The same request from this machine is admitted.
    let admitted = raw(
        addr,
        &format!(
            "GET /api/grant HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {DEVELOPER}\r\n\
             Connection: close\r\n\r\n",
            addr.port()
        ),
    );
    assert!(admitted.starts_with("HTTP/1.1 200"), "{admitted}");

    // None of it was anybody's call, and none of it reached the world.
    let rows = served.rows();
    assert_eq!(rows.len(), 3, "{rows:#?}");
    for row in &rows {
        assert_eq!(
            (row["calls"].as_u64(), row["failed_calls"].as_u64()),
            (Some(0), Some(0)),
            "{row:#}"
        );
    }
    assert_eq!(served.world(), before);
}

/// The developer's methods `player_host.rs` refuses a seat, the game's and the host's.
fn developer_methods() -> Vec<(&'static str, Value)> {
    vec![
        ("world.get", json!({"entity": "Crate4"})),
        ("world.query", json!({"with": ["Perceivable"]})),
        ("world.tree", json!({})),
        ("world.edit", set_cargo(99)),
        ("time.step", json!({"ticks": 5})),
        ("time.control", json!({"pause": false})),
        ("snapshots.restore", json!({"tick": 0})),
        ("status", json!({})),
        ("events.since", json!({})),
        ("log.since", json!({})),
        ("catalog.list", json!({})),
        ("agent.session.list", json!({})),
        ("agent.session.events", json!({})),
        ("agent.session.open", json!({"label": "a seat's"})),
        ("agent.session.close", json!({"session": "mcp-1"})),
        ("debug.rewind", json!({"tick": 0})),
        (
            "debug.writes",
            json!({"entity": "Sloop", "component": "Boat", "field": "rudder"}),
        ),
        ("debug.eval", json!({"expr": "1"})),
        ("scripts.apply", json!({})),
        ("project.save", json!({})),
        ("saves.write", json!({"name": "a"})),
        ("saves.load", json!({"name": "a"})),
        ("scene.load", json!({"path": "scene.json"})),
        ("assets.upload", json!({"path": "a.txt", "data": "YQ=="})),
        ("capture", json!({})),
    ]
}

#[test]
fn a_player_token_is_its_seat_whatever_the_body_says() {
    let served = Served::start(true);
    let (url, addr) = (served.url.clone(), served.addr);
    let before = served.world();
    let auth = bearer(PLAYER);
    let as_player = [("Authorization", auth.as_str())];

    // What the token is: asked of the host, never chosen.
    let grant = get(&format!("{url}/api/grant"), &as_player);
    assert_eq!(grant.status, 200, "{}", grant.body);
    assert_eq!(
        grant.json(),
        json!({"role": "player", "seat": "skipper", "label": "agent", "session": "mcp-1"})
    );
    let s = result(&url, PLAYER, "player.session", json!({}));
    assert_eq!(
        (s["role"].clone(), s["seat"].clone()),
        (json!("player"), json!("skipper")),
        "{s:#}"
    );
    // Its seat's perception, and no omniscient view.
    let o = result(
        &url,
        PLAYER,
        "player.observe",
        json!({"projection": "json"}),
    );
    assert_eq!(o["omniscient"], json!(false), "{o:#}");
    let e = error(
        &url,
        PLAYER,
        &request("player.observe", json!({"omniscient": true})),
    );
    assert_eq!(e["code"], json!("perception.omniscient_forbidden"), "{e:#}");
    let e = error(
        &url,
        PLAYER,
        &request("player.describe", json!({"entity": "Crate4"})),
    );
    assert_eq!(e["code"], json!("perception.unknown_entity"), "{e:#}");
    // The screen's UI is every player's (docs/spec/ui.md 6): the seat reads it, as it presses it.
    let ui = result(&url, PLAYER, "ui.list", json!({}));
    assert!(ui["elements"].is_array(), "{ui:#}");

    // The body's `seat`: its own is taken, another is not its to name, and nothing else is one.
    let seated =
        |seat: Value| json!({"id": 1, "method": "player.session", "params": {}, "seat": seat});
    let own = call(&url, PLAYER, &seated(json!("skipper")));
    assert_eq!(own["result"]["seat"], json!("skipper"), "{own:#}");
    for other in ["lookout", "crew", "", "Skipper"] {
        let e = error(&url, PLAYER, &seated(json!(other)));
        assert_eq!(e["code"], json!("seat.not_yours"), "{other:?}: {e:#}");
        assert_eq!(
            (e["detail"]["path"].clone(), e["detail"]["yours"].clone()),
            (json!("/seat"), json!("skipper")),
            "{e:#}"
        );
    }
    for not_a_seat in [
        json!(0),
        json!(["skipper"]),
        json!({"id": "skipper"}),
        json!(true),
    ] {
        let e = error(&url, PLAYER, &seated(not_a_seat));
        assert_eq!(e["code"], json!("request.wrong_type"), "{e:#}");
        assert_eq!(e["detail"]["path"], json!("/seat"), "{e:#}");
    }
    // A session handle is a developer's to call with: a bridge's, opened by the setup grant.
    let bridge = result(
        &url,
        DEVELOPER,
        "agent.session.open",
        json!({"label": "bridge"}),
    );
    let handle = bridge["handle"].clone();
    assert!(handle.is_string(), "{bridge:#}");
    for method in ["status", "player.session"] {
        let e = error(
            &url,
            PLAYER,
            &json!({"id": 1, "method": method, "params": {}, "handle": handle}),
        );
        assert_eq!(e["code"], json!("permission.denied"), "{method}: {e:#}");
        assert_eq!(e["detail"]["role"], json!("player"), "{e:#}");
    }
    // Keys the envelope does not have change nothing: the call is still the seat's player's.
    let e = error(
        &url,
        PLAYER,
        &json!({"id": 1, "method": "status", "params": {}, "role": "developer",
                "token": DEVELOPER, "grant": {"role": "developer"}, "session": "mcp-3"}),
    );
    assert_eq!(e["code"], json!("permission.denied"), "{e:#}");
    let s = call(
        &url,
        PLAYER,
        &json!({"id": 1, "method": "player.session", "params": {}, "role": "developer",
                "token": DEVELOPER}),
    );
    assert_eq!(s["result"]["role"], json!("player"), "{s:#}");
    // Nor does naming another seat in the parameters: the game refuses it its own way.
    let e = error(
        &url,
        PLAYER,
        &request("player.observe", json!({"seat": "lookout"})),
    );
    assert!(
        e["code"].as_str().is_some_and(|c| c.starts_with("seat.")),
        "{e:#}"
    );

    // Every developer's method, the game's and the host's, is refused and changes nothing.
    for (method, params) in developer_methods() {
        let e = error(&url, PLAYER, &request(method, params));
        assert_eq!(e["code"], json!("permission.denied"), "{method}: {e:#}");
        assert_eq!(
            (e["detail"]["role"].clone(), e["detail"]["needs"].clone()),
            (json!("player"), json!("developer")),
            "{method}: {e:#}"
        );
    }

    // The routes that are no player's: the catalog and the project's files (a checker's), the
    // editor's socket, the render feed, the viewport and the editor's files (a developer's).
    for (path, needs) in [
        ("/api/catalog", "checker"),
        ("/assets/scene.json", "checker"),
        ("/assets/project.toml", "checker"),
        ("/", "developer"),
        ("/index.html", "developer"),
        ("/wasm/x", "developer"),
        ("/render", "developer"),
        ("/ws", "developer"),
    ] {
        let a = get(&format!("{url}{path}"), &as_player);
        assert_eq!(a.status, 403, "GET {path}: {}", a.body);
        let e = a.error();
        assert_eq!(e["code"], json!("permission.denied"), "{path}: {e:#}");
        assert_eq!(
            (e["detail"]["role"].clone(), e["detail"]["needs"].clone()),
            (json!("player"), json!(needs)),
            "{path}: {e:#}"
        );
    }
    for path in ["/ws", "/render"] {
        let a = refused_upgrade(addr, path, &as_player);
        assert_eq!(a.status, 403, "an upgrade of {path}");
    }

    // Over MCP, by its header and by its path: the player tool alone, player instructions.
    let mut clients = [
        McpHttp::by_header(&url, PLAYER),
        McpHttp::by_path(&url, PLAYER),
    ];
    for c in &mut clients {
        let instructions = c.initialize("escalator");
        assert!(
            instructions.starts_with("You play one seat"),
            "{instructions}"
        );
        assert_eq!(c.tools(), ["player"]);
        for (tool, args) in [
            ("world", json!({"action": "get", "entity": "Crate4"})),
            ("world", set_cargo(99)),
            ("time", json!({"action": "step", "ticks": 5})),
            ("debug", json!({"action": "eval", "expr": "1"})),
        ] {
            let (failed, text) = c.tool(tool, args);
            assert!(
                failed && text.contains("permission.denied"),
                "{tool}: {text}"
            );
        }
        let (failed, text) = c.tool("player", json!({"action": "observe"}));
        assert!(!failed && text.starts_with("tick "), "{text}");
        let (failed, text) = c.tool("player", json!({"action": "observe", "omniscient": true}));
        assert!(
            failed && text.contains("perception.omniscient_forbidden"),
            "{text}"
        );
        let (failed, text) = c.tool("player", json!({"action": "session"}));
        assert!(!failed && text.contains("\"role\":\"player\""), "{text}");
        let (failed, text) = c.tool("player", json!({"action": "ui"}));
        assert!(!failed && text.contains("\"elements\""), "{text}");
    }
    // An MCP session belongs to its grant: the player's under the checker's token is no
    // session, nor a developer's under the player's.
    let player_session = clients[0].session.clone().unwrap();
    let mut developer = McpHttp::by_header(&url, DEVELOPER);
    developer.initialize("setup");
    assert_eq!(developer.tools().len(), 11);
    let list = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list", "params": {}});
    let call_world = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                            "params": {"name": "world", "arguments": {"action": "tree"}}});
    for (token, session) in [
        (CHECKER, player_session.as_str()),
        (DEVELOPER, player_session.as_str()),
        (PLAYER, developer.session.as_deref().unwrap()),
    ] {
        for message in [&list, &call_world] {
            let a = mcp_post(
                &url,
                "/mcp",
                message,
                &[
                    ("Authorization", &bearer(token)),
                    ("Mcp-Session-Id", session),
                ],
            );
            assert!(
                a.status >= 400 && a.message["result"].is_null(),
                "HTTP {}: {}",
                a.status,
                a.body
            );
        }
    }

    // The grant's one row took every call, under the file's label, whatever the MCP clients
    // called themselves; the world is as it was.
    let rows = served.rows();
    let row = &rows[0];
    assert_eq!(
        (
            row["id"].as_str(),
            row["label"].as_str(),
            row["role"].as_str()
        ),
        (Some("mcp-1"), Some("agent"), Some("player")),
        "{row:#}"
    );
    assert!(row["calls"].as_u64().unwrap() > 40, "{row:#}");
    assert!(row["failed_calls"].as_u64().unwrap() > 30, "{row:#}");
    assert_eq!(served.world(), before);
}

/// A player looks ahead only when its grant's policy has `fork` (shared/contract/mcp.md 3.1,
/// 5.5; docs/spec/player.md 7.1): its branches are its own, played by `world` through the player
/// tool over HTTP and MCP, and main does not move while they run.
#[test]
fn a_player_forks_when_its_grant_allows_and_main_stays() {
    let served = Served::start(true);
    let url = served.url.clone();
    let e = error(&url, PLAYER, &request("player.fork", json!({})));
    assert_eq!(e["code"], json!("session.policy_denies"), "{e:#}");
    assert_eq!(e["detail"]["policy"], json!("fork"), "{e:#}");
    drop(served);

    let mut file: Value = serde_json::from_str(&grants_file()).unwrap();
    file["grants"][0]["policy"] = json!({"fork": true});
    let served = Served::with(Some(file.to_string()));
    let url = served.url.clone();
    let before = served.world();
    let f = result(&url, PLAYER, "player.fork", json!({"label": "try"}));
    assert_eq!(f["world"], json!("b1"), "{f:#}");
    assert!(f.get("world_hash").is_none(), "{f:#}");
    let w = result(
        &url,
        PLAYER,
        "player.wait",
        json!({"world": "b1", "ticks": 120}),
    );
    assert_eq!(w["world"], json!("b1"), "{w:#}");
    assert!(w["tick"].as_u64().unwrap() > 0, "{w:#}");
    // The checker and the developer have their own sources: the player's branch is none of theirs.
    let e = error(
        &url,
        DEVELOPER,
        &request("player.observe", json!({"world": "b1", "seat": "skipper"})),
    );
    assert_eq!(e["code"], json!("session.unknown_world"), "{e:#}");
    let e = error(&url, CHECKER, &request("player.fork", json!({})));
    assert_eq!(e["code"], json!("permission.denied"), "{e:#}");

    // The player tool carries the same over MCP.
    let mut mcp = McpHttp::by_path(&url, PLAYER);
    mcp.initialize("agent");
    let (failed, text) = mcp.tool("player", json!({"action": "branches"}));
    assert!(!failed, "{text}");
    assert!(text.contains("\"b1\""), "{text}");
    let (failed, text) = mcp.tool("player", json!({"action": "observe", "world": "b1"}));
    assert!(!failed, "{text}");
    assert!(text.starts_with("world b1\n"), "{text}");
    let (failed, text) = mcp.tool("player", json!({"action": "discard", "branch": "b1"}));
    assert!(!failed, "{text}");
    let (failed, text) = mcp.tool("player", json!({"action": "wait", "world": "b1"}));
    assert!(failed && text.contains("session.unknown_world"), "{text}");
    // A locked host starts no agent program (server.md 3.7), not even for a developer.
    let e = error(
        &url,
        DEVELOPER,
        &request("agent.session.start", json!({"agent": "a", "goal": "g"})),
    );
    assert_eq!(e["code"], json!("host.not_supported"), "{e:#}");
    assert_eq!(served.world(), before);
}

#[test]
fn a_checker_token_reads_and_changes_nothing() {
    let served = Served::start(true);
    let (url, addr) = (served.url.clone(), served.addr);
    let before = served.world();
    let auth = bearer(CHECKER);
    let as_checker = [("Authorization", auth.as_str())];
    let grant = get(&format!("{url}/api/grant"), &as_checker);
    assert_eq!(
        grant.json(),
        json!({"role": "checker", "seat": null, "label": "harness", "session": "mcp-2"})
    );

    // The catalog's reads answer, the game's and the host's.
    for (method, params) in [
        ("status", json!({})),
        ("project.info", json!({})),
        ("world.get", json!({"entity": "Crate4"})),
        ("world_get", json!({"entity": "Crate4"})),
        ("world.query", json!({"with": ["Perceivable"]})),
        ("world.tree", json!({})),
        ("world.schema", json!({})),
        ("events.since", json!({})),
        ("log.since", json!({})),
        ("catalog.list", json!({})),
        ("history.list", json!({})),
        ("snapshots.list", json!({})),
        ("snapshot", json!({})),
        ("scripts.list", json!({})),
        ("scripts.status", json!({})),
        ("scripts.guide", json!({})),
        ("docs.search", json!({"query": "crate"})),
        ("assets.list", json!({})),
        ("saves.list", json!({})),
        ("agent.session.list", json!({})),
        ("agent.session.events", json!({})),
        ("player.session", json!({"seat": "skipper"})),
    ] {
        let a = call(&url, CHECKER, &request(method, params));
        assert!(a.get("error").is_none(), "{method}: {a:#}");
    }
    // Any seat's view, and the omniscient one, marked: a checker sees everything.
    let o = result(
        &url,
        CHECKER,
        "player.observe",
        json!({"seat": "skipper", "omniscient": true, "projection": "json"}),
    );
    assert_eq!(o["omniscient"], json!(true), "{o:#}");
    // The game sees a developer source, so that is what its session answer says.
    let s = result(&url, CHECKER, "player.session", json!({"seat": "skipper"}));
    assert_eq!(s["role"], json!("developer"), "{s:#}");

    // Everything the catalog does not call a read, by its name and by every alias, is a
    // developer's: asked of the catalog itself, so a method added later is covered.
    let catalog = get(
        &format!("{url}/api/catalog"),
        &[("Authorization", &bearer(DEVELOPER))],
    );
    assert_eq!(catalog.status, 200, "{}", catalog.body);
    let catalog = catalog.json();
    let mut refused: Vec<String> = Vec::new();
    let mut reads: Vec<String> = Vec::new();
    for entry in catalog.as_array().unwrap() {
        let names = std::iter::once(&entry["name"])
            .chain(entry["aliases"].as_array().into_iter().flatten())
            .filter_map(|n| n.as_str().map(str::to_owned));
        for name in names {
            if entry["kind"] == json!("read") {
                reads.push(name);
                continue;
            }
            let e = error(&url, CHECKER, &request(&name, json!({})));
            assert_eq!(e["code"], json!("permission.denied"), "{name}: {e:#}");
            assert_eq!(
                (
                    e["detail"]["request"].clone(),
                    e["detail"]["role"].clone(),
                    e["detail"]["needs"].clone()
                ),
                (json!(name), json!("checker"), json!("developer")),
                "{name}: {e:#}"
            );
            refused.push(name);
        }
    }
    for must in [
        "world.edit",
        "world_edit",
        "history.undo",
        "history.redo",
        "time.step",
        "step",
        "time.control",
        "time_control",
        "play.start",
        "play.stop",
        "snapshots.restore",
        "saves.write",
        "saves.load",
        "scene.load",
        "project.save",
        "assets.upload",
        "scripts.write",
        "scripts.apply",
        "player.act",
        "player.wait",
        "player.continue",
        "player.pacing",
        "agent.session.open",
        "agent.session.close",
        "debug.eval",
        "debug.set",
        "debug.rewind",
        "debug.writes",
        "debug.attach",
        "debug.pause",
        "debug.continue",
        "debug.step",
        "debug.breakpoints.set",
        "debug.break",
        "debug.watch",
    ] {
        assert!(
            refused.iter().any(|r| r == must),
            "{must} was not refused: {refused:?}"
        );
    }
    assert!(refused.len() >= 30, "{refused:?}");
    // With parameters that would apply, too: refused before anything is decoded.
    for (method, params) in [
        ("world.edit", set_cargo(99)),
        ("time.step", json!({"ticks": 5})),
        ("player.act", json!({"seat": "skipper", "actions": []})),
        ("debug.eval", json!({"expr": "1"})),
    ] {
        let e = error(&url, CHECKER, &request(method, params));
        assert_eq!(e["code"], json!("permission.denied"), "{method}: {e:#}");
    }
    // Two entries the catalog calls reads compile through a request or write project files:
    // withheld. The other reads of the debugger pass the gate and reach the debugger.
    for method in ["scripts.check", "scripts.types"] {
        assert!(
            reads.iter().any(|r| r == method),
            "{method} is no read any more: drop it from CHECKER_WITHHELD"
        );
        let e = error(&url, CHECKER, &request(method, json!({})));
        assert_eq!(e["code"], json!("permission.denied"), "{method}: {e:#}");
        assert_eq!(e["detail"]["role"], json!("checker"), "{method}: {e:#}");
    }
    for method in ["debug.state", "debug.breakpoints.list"] {
        let e = error(&url, CHECKER, &request(method, json!({})));
        assert_eq!(e["code"], json!("debug.not_available"), "{method}: {e:#}");
    }
    // A name the catalog does not have is no method, said with what a checker has: its reads
    // alone are counted and suggested, never a method it would be refused.
    let checker_reads = reads
        .iter()
        .filter(|r| !["scripts.check", "scripts.types"].contains(&r.as_str()))
        .count();
    for (method, suggested) in [
        ("world.gett", Some("world.get")),
        ("scripts.swap", None),
        ("world.edi", None),
        ("agent.session.begin", None),
        ("", None),
    ] {
        let e = error(&url, CHECKER, &request(method, json!({})));
        assert_eq!(
            e["code"],
            json!("request.unknown_method"),
            "{method:?}: {e:#}"
        );
        assert_eq!(e["detail"]["allowed_count"], json!(checker_reads), "{e:#}");
        let suggestions = e["detail"]["suggestions"].as_array().unwrap();
        if let Some(read) = suggested {
            assert!(suggestions.contains(&json!(read)), "{e:#}");
        }
        for s in suggestions {
            let name = s.as_str().unwrap();
            assert!(reads.iter().any(|r| r == name), "{method:?}: {e:#}");
            assert!(!name.starts_with("scripts.check") && name != "scripts.types");
        }
    }

    // A checker is nobody's player and holds no session handle.
    let bridge = result(&url, DEVELOPER, "agent.session.open", json!({}));
    for body in [
        json!({"id": 1, "method": "player.act", "params": {"actions": []}, "seat": "skipper"}),
        json!({"id": 1, "method": "status", "params": {}, "seat": "skipper"}),
        json!({"id": 1, "method": "world.edit", "params": set_cargo(99), "handle": bridge["handle"]}),
        json!({"id": 1, "method": "status", "params": {}, "handle": bridge["handle"]}),
        json!({"id": 1, "method": "status", "params": {}, "seat": 0}),
    ] {
        let e = error(&url, CHECKER, &body);
        assert_eq!(e["code"], json!("permission.denied"), "{body}: {e:#}");
        assert_eq!(
            (e["detail"]["role"].clone(), e["detail"]["needs"].clone()),
            (json!("checker"), json!("developer")),
            "{body}: {e:#}"
        );
    }

    // Its routes: the catalog and the project's files; no socket, no feed, no editor.
    for path in ["/api/catalog", "/assets/scene.json", "/assets/project.toml"] {
        let a = get(&format!("{url}{path}"), &as_checker);
        assert_eq!(a.status, 200, "GET {path}: {}", a.body);
    }
    for path in ["/", "/index.html", "/wasm/x", "/render", "/ws"] {
        let a = get(&format!("{url}{path}"), &as_checker);
        assert_eq!(a.status, 403, "GET {path}: {}", a.body);
        let e = a.error();
        assert_eq!(
            (
                e["code"].clone(),
                e["detail"]["role"].clone(),
                e["detail"]["needs"].clone()
            ),
            (
                json!("permission.denied"),
                json!("checker"),
                json!("developer")
            ),
            "{path}: {e:#}"
        );
    }
    for path in ["/ws", "/render"] {
        let a = refused_upgrade(addr, path, &as_checker);
        assert_eq!(a.status, 403, "an upgrade of {path}");
    }

    // Over MCP it lists the developer's tools and the host refuses what is no read.
    let mut c = McpHttp::by_path(&url, CHECKER);
    let instructions = c.initialize("checker-client");
    assert!(instructions.starts_with("Amoris host"), "{instructions}");
    assert_eq!(c.tools().len(), 11);
    let (failed, text) = c.tool("world", json!({"action": "get", "entity": "Crate4"}));
    assert!(!failed && text.contains("Cargo"), "{text}");
    let (failed, text) = c.tool(
        "player",
        json!({"action": "observe", "seat": "skipper", "omniscient": true}),
    );
    assert!(!failed, "{text}");
    for (tool, args) in [
        ("world", {
            let mut a = set_cargo(99);
            a["action"] = json!("edit");
            a
        }),
        ("time", json!({"action": "step", "ticks": 5})),
        ("time", json!({"action": "pause"})),
        ("play", json!({"action": "start"})),
        ("history", json!({"action": "undo"})),
        ("scripts", json!({"action": "apply"})),
        ("scripts", json!({"action": "check"})),
        ("debug", json!({"action": "eval", "expr": "1"})),
        (
            "player",
            json!({"action": "act", "seat": "skipper", "actions": []}),
        ),
    ] {
        let (failed, text) = c.tool(tool, args);
        assert!(
            failed && text.contains("permission.denied"),
            "{tool}: {text}"
        );
        assert!(text.contains("\"role\":\"checker\""), "{tool}: {text}");
    }

    // The row counted them, and the world's tick, hash and writes are what they were.
    let row = &served.rows()[1];
    assert_eq!(
        (
            row["id"].as_str(),
            row["label"].as_str(),
            row["role"].as_str()
        ),
        (Some("mcp-2"), Some("harness"), Some("checker")),
        "{row:#}"
    );
    assert!(row["failed_calls"].as_u64().unwrap() > 50, "{row:#}");
    assert_eq!(served.world(), before);
    assert_eq!(served.owner("history.list", json!({}))["undo"], json!([]));
}

#[test]
fn a_developer_token_does_everything_and_grants_are_listed_sessions() {
    let served = Served::start(true);
    let (url, addr) = (served.url.clone(), served.addr);
    let auth = bearer(DEVELOPER);
    let as_developer = [("Authorization", auth.as_str())];

    // The grants are sessions from the moment the host locked: in the file's order, live,
    // under the file's labels, before any of them called.
    let rows = served.rows();
    let listed: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!([
                r["id"],
                r["transport"],
                r["role"],
                r["seat"],
                r["label"],
                r["live"],
                r["calls"]
            ])
        })
        .collect();
    assert_eq!(
        listed,
        [
            json!(["mcp-1", "grant", "player", "skipper", "agent", true, 0]),
            json!(["mcp-2", "grant", "checker", null, "harness", true, 0]),
            json!(["mcp-3", "grant", "developer", null, "setup", true, 0]),
        ]
    );
    let grant = get(&format!("{url}/api/grant"), &as_developer);
    assert_eq!(
        grant.json(),
        json!({"role": "developer", "seat": null, "label": "setup", "session": "mcp-3"})
    );

    // An edit applies, and is its session's in the history; time steps.
    let edited = result(&url, DEVELOPER, "world.edit", set_cargo(71));
    assert_eq!(edited["applied"], json!(1), "{edited:#}");
    let history = result(&url, DEVELOPER, "history.list", json!({}));
    assert_eq!(
        history["entries"]["undo"][0]["session"],
        json!("mcp-3"),
        "{history:#}"
    );
    let tick = result(&url, DEVELOPER, "status", json!({}))["tick"]
        .as_u64()
        .unwrap();
    let stepped = result(&url, DEVELOPER, "time.step", json!({"ticks": 2}));
    assert_eq!(stepped["tick"], json!(tick + 2), "{stepped:#}");
    // What a checker is refused reaches the debugger.
    let e = error(
        &url,
        DEVELOPER,
        &request("debug.eval", json!({"expr": "1"})),
    );
    assert_eq!(e["code"], json!("debug.not_available"), "{e:#}");

    // It names a seat, or a session of its own, as on a host without grants.
    let s = call(
        &url,
        DEVELOPER,
        &json!({"id": 1, "method": "player.session", "params": {}, "seat": "skipper"}),
    );
    assert_eq!(s["result"]["role"], json!("player"), "{s:#}");
    let e = error(
        &url,
        DEVELOPER,
        &json!({"id": 1, "method": "status", "params": {}, "seat": "skipper"}),
    );
    assert_eq!(e["code"], json!("permission.denied"), "{e:#}");
    let e = error(
        &url,
        DEVELOPER,
        &json!({"id": 1, "method": "player.session", "params": {}, "seat": "lookout"}),
    );
    assert_eq!(e["code"], json!("seat.unknown"), "{e:#}");
    let bridge = result(
        &url,
        DEVELOPER,
        "agent.session.open",
        json!({"label": "bridge"}),
    );
    assert_eq!(
        (bridge["id"].as_str(), bridge["transport"].as_str()),
        (Some("mcp-4"), Some("bridge")),
        "{bridge:#}"
    );
    let through = call(
        &url,
        DEVELOPER,
        &json!({"id": 1, "method": "world.get", "params": {"entity": "Crate4"},
                "handle": bridge["handle"]}),
    );
    assert_eq!(
        through["result"]["components"]["Cargo"]["value"],
        json!(71),
        "{through:#}"
    );
    // A handle is an address, not an admission: with no token it is nobody's.
    let a = post(
        &format!("{url}/api/call"),
        &[],
        &json!({"id": 1, "method": "status", "params": {}, "handle": bridge["handle"]}).to_string(),
    );
    assert_eq!(a.status, 401, "{}", a.body);

    // The editor's socket, with the header: `status` on connect, and a request answered.
    let mut ws = upgrade(addr, "/ws", &as_developer).expect("the developer's socket");
    let frame = |ws: &mut tungstenite::WebSocket<_>| loop {
        if let tungstenite::Message::Text(t) = ws.read().unwrap() {
            return serde_json::from_str::<Value>(t.as_str()).unwrap();
        }
    };
    assert_eq!(frame(&mut ws)["event"], json!("status"));
    ws.send(tungstenite::Message::Text(
        request("world.get", json!({"entity": "Crate4"}))
            .to_string()
            .into(),
    ))
    .unwrap();
    let answered = loop {
        let v = frame(&mut ws);
        if v["id"] == json!(1) {
            break v;
        }
    };
    assert_eq!(
        answered["result"]["components"]["Cargo"]["value"],
        json!(71),
        "{answered:#}"
    );
    drop(ws);
    // The render feed's route reaches its handler (this host installs no feed).
    let a = refused_upgrade(addr, "/render", &as_developer);
    assert_eq!(a.status, 501, "{}", a.body);
    // The catalog, the project's files and the editor's page.
    for path in ["/api/catalog", "/assets/scene.json", "/"] {
        let a = get(&format!("{url}{path}"), &as_developer);
        assert_eq!(a.status, 200, "GET {path}: {}", a.body);
    }
    // A route's method is what it was: the admission adds nothing to it.
    let a = get(&format!("{url}/api/call"), &as_developer);
    assert_eq!(a.status, 405, "{}", a.body);
    let a = post(&format!("{url}/api/call"), &as_developer, "not json");
    assert_eq!(a.status, 400, "{}", a.body);
    assert_eq!(a.error()["code"], json!("request.malformed"));
    let e = error(&url, DEVELOPER, &json!({"id": 1, "params": {}}));
    assert_eq!(e["code"], json!("request.malformed"), "{e:#}");

    // Over MCP: every tool, as the grant's session, whatever the client calls itself.
    let mut c = McpHttp::by_header(&url, DEVELOPER);
    let instructions = c.initialize("claude-code");
    assert!(instructions.starts_with("Amoris host"), "{instructions}");
    assert_eq!(c.tools().len(), 11);
    let (failed, text) = c.tool("world", {
        let mut a = set_cargo(72);
        a["action"] = json!("edit");
        a
    });
    assert!(!failed, "{text}");
    let mut by_path = McpHttp::by_path(&url, DEVELOPER);
    by_path.initialize("another");
    let (failed, text) = by_path.tool("world", json!({"action": "get", "entity": "Crate4"}));
    assert!(!failed && text.contains("72"), "{text}");
    // The client ends its MCP session; the grant's row stays open.
    let ended = agent()
        .delete(format!("{url}/mcp"))
        .header("Authorization", &auth)
        .header("Mcp-Session-Id", c.session.as_deref().unwrap())
        .call()
        .unwrap();
    assert!(ended.status().is_success(), "HTTP {}", ended.status());

    // The list: the three grants first, in the file's order, counting what each was asked.
    let _ = error(&url, PLAYER, &request("status", json!({})));
    let _ = result(&url, PLAYER, "player.session", json!({}));
    let _ = error(&url, CHECKER, &request("world.edit", set_cargo(99)));
    let rows = result(&url, DEVELOPER, "agent.session.list", json!({}))["sessions"]
        .as_array()
        .unwrap()
        .clone();
    let listed: Vec<Value> = rows
        .iter()
        .take(3)
        .map(|r| {
            json!([
                r["id"],
                r["transport"],
                r["role"],
                r["label"],
                r["live"],
                r["calls"],
                r["failed_calls"]
            ])
        })
        .collect();
    let developer_calls = rows[2]["calls"].as_u64().unwrap();
    assert_eq!(
        listed,
        [
            json!(["mcp-1", "grant", "player", "agent", true, 2, 1]),
            json!(["mcp-2", "grant", "checker", "harness", true, 1, 1]),
            json!([
                "mcp-3",
                "grant",
                "developer",
                "setup",
                true,
                developer_calls,
                1
            ]),
        ]
    );
    assert!(developer_calls >= 8, "{rows:#?}");
    assert_eq!(rows[2]["last_method"], json!("agent.session.list"));
    // What followed them: the seat's calls a developer made with no session, and the bridge.
    let after: Vec<(&str, &str)> = rows[3..]
        .iter()
        .map(|r| (r["id"].as_str().unwrap(), r["transport"].as_str().unwrap()))
        .collect();
    assert_eq!(after, [("player:skipper", "api"), ("mcp-4", "bridge")]);
    let keys = |v: &Value| {
        let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort_unstable();
        keys
    };
    assert_eq!(
        keys(&rows[0]),
        [
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
            "transport"
        ]
    );
    // A grant's session is the host's to hold: not closed by its id.
    for id in ["mcp-1", "mcp-2", "mcp-3"] {
        let e = error(
            &url,
            DEVELOPER,
            &request("agent.session.close", json!({"session": id})),
        );
        assert_eq!(e["code"], json!("request.not_applicable"), "{id}: {e:#}");
    }
    assert!(
        served
            .rows()
            .iter()
            .take(3)
            .all(|r| r["live"] == json!(true))
    );

    // No answer of the host says a token: not the list, the stream, the status, the project,
    // the catalog or the grant.
    for (method, params) in [
        ("agent.session.list", json!({})),
        ("agent.session.events", json!({"since": 0, "limit": 2000})),
        ("status", json!({})),
        ("project.info", json!({})),
        ("catalog.list", json!({})),
        ("history.list", json!({})),
        ("log.since", json!({})),
    ] {
        says_no_token(&result(&url, DEVELOPER, method, params).to_string(), method);
    }
    for token in TOKENS {
        let a = get(
            &format!("{url}/api/grant"),
            &[("Authorization", &bearer(token))],
        );
        says_no_token(&a.body, "/api/grant");
    }
    // A token in upper-case digits is the same grant.
    let a = get(
        &format!("{url}/api/grant"),
        &[(
            "Authorization",
            &format!("bearer {}", DEVELOPER.to_uppercase()),
        )],
    );
    assert_eq!(a.json()["session"], json!("mcp-3"), "{}", a.body);
    // A host is locked once.
    let again = served
        .rt
        .block_on(served.host.lock(Grants::parse(&grants_file()).unwrap()))
        .unwrap_err();
    assert_eq!(again.code, "host.grants_invalid", "{again:#?}");
}

#[test]
fn an_unlocked_host_takes_no_token_and_behaves_as_before() {
    let served = Served::start(false);
    let (url, addr) = (served.url.clone(), served.addr);
    // Nobody is a session before anybody called.
    assert_eq!(served.rows(), Vec::<Value>::new());

    // Every route answers without a token, and the same with one: a host without grants reads
    // no `Authorization`.
    let with_any = bearer(UNKNOWN);
    for headers in [vec![], vec![("Authorization", with_any.as_str())]] {
        let a = post(
            &format!("{url}/api/call"),
            &headers,
            &request(
                "world.get",
                json!({"entity": "Crate4", "components": ["Cargo"]}),
            )
            .to_string(),
        );
        assert_eq!(a.status, 200, "{}", a.body);
        assert!(
            a.json()["result"]["components"]["Cargo"].is_object(),
            "{}",
            a.body
        );
        for path in ["/api/catalog", "/assets/scene.json", "/"] {
            let a = get(&format!("{url}{path}"), &headers);
            assert_eq!(a.status, 200, "GET {path}: {}", a.body);
            assert_eq!(a.challenge, None);
        }
        let mut ws = upgrade(addr, "/ws", &headers).expect("the editor's socket");
        assert!(matches!(ws.read().unwrap(), tungstenite::Message::Text(_)));
        let a = refused_upgrade(addr, "/render", &headers);
        assert_eq!(a.status, 501, "{}", a.body);
    }
    let plain = get(&format!("{url}/api/catalog"), &[]);
    let tokened = get(
        &format!("{url}/api/catalog"),
        &[("Authorization", &with_any)],
    );
    assert_eq!(plain, tokened);

    // A call is a developer's unless its body names a seat; a seat's is its player's, refused
    // a developer's methods, as before.
    let dev = post(
        &format!("{url}/api/call"),
        &[],
        &request("world.edit", set_cargo(81)).to_string(),
    );
    assert_eq!(dev.json()["result"]["applied"], json!(1), "{}", dev.body);
    let seat = |method: &str| json!({"id": 1, "method": method, "params": {}, "seat": "skipper"});
    let s = post(
        &format!("{url}/api/call"),
        &[],
        &seat("player.session").to_string(),
    );
    assert_eq!(s.json()["result"]["role"], json!("player"), "{}", s.body);
    for method in ["world.tree", "status", "agent.session.list"] {
        let e = post(&format!("{url}/api/call"), &[], &seat(method).to_string()).error();
        assert_eq!(e["code"], json!("permission.denied"), "{method}: {e:#}");
        assert_eq!(e["detail"]["role"], json!("player"), "{method}: {e:#}");
    }
    // There is no checker without grants: the catalog's writes are a plain caller's.
    let stepped = post(
        &format!("{url}/api/call"),
        &[],
        &request("time.step", json!({"ticks": 1})).to_string(),
    );
    assert!(
        stepped.json()["result"]["tick"].is_u64(),
        "{}",
        stepped.body
    );

    // `/mcp` is a developer session with every tool, named by its client; `/mcp/<token>` is
    // no route of an unlocked host's MCP.
    let mut c = McpHttp {
        url: url.clone(),
        path: "/mcp".to_owned(),
        token: None,
        session: None,
        id: 0,
    };
    let instructions = c.initialize("plain");
    assert!(instructions.starts_with("Amoris host"), "{instructions}");
    assert_eq!(c.tools().len(), 11);
    let (failed, text) = c.tool("world", json!({"action": "tree"}));
    assert!(!failed, "{text}");
    let rows = served.rows();
    let listed: Vec<Value> = rows
        .iter()
        .map(|r| json!([r["id"], r["transport"], r["role"], r["label"]]))
        .collect();
    assert_eq!(
        listed,
        [
            json!(["player:skipper", "api", "player", ""]),
            json!(["mcp-1", "http", "developer", "plain 1"]),
        ]
    );
    // It has no grants to tell of: `/api/grant` is not a route, and answers no grant.
    let a = get(&format!("{url}/api/grant"), &[("Authorization", &with_any)]);
    assert!(
        serde_json::from_str::<Value>(&a.body).map_or(true, |v| v.get("role").is_none()),
        "{}",
        a.body
    );
    assert_eq!(a.challenge, None);
    // And it is not locked afterwards: a host that serves without grants stays so.
    let late = served
        .rt
        .block_on(served.host.lock(Grants::parse(&grants_file()).unwrap()))
        .unwrap_err();
    assert_eq!(late.code, "host.grants_invalid", "{late:#?}");
    assert!(!served.host.locked());
    let a = get(&format!("{url}/api/catalog"), &[]);
    assert_eq!(a.status, 200, "{}", a.body);
}

#[test]
fn the_lock_is_not_in_the_world() {
    let locked = Served::start(true);
    let open = Served::start(false);
    let hash = |s: &Served| {
        let status = s.owner("status", json!({}));
        (status["tick"].clone(), status["world_hash"].clone())
    };
    assert!(hash(&locked).1.is_string());
    assert_eq!(hash(&locked), hash(&open), "the same project and seed");
    // The same edit and the same 60 ticks: by the setup grant there, by a plain caller here.
    let e = result(&locked.url, DEVELOPER, "world.edit", set_cargo(55));
    assert_eq!(e["applied"], json!(1), "{e:#}");
    let stepped = result(&locked.url, DEVELOPER, "time.step", json!({"ticks": 60}));
    let plain = |method: &str, params: Value| {
        post(
            &format!("{}/api/call", open.url),
            &[],
            &request(method, params).to_string(),
        )
        .json()["result"]
            .clone()
    };
    assert_eq!(plain("world.edit", set_cargo(55))["applied"], json!(1));
    let stepped_open = plain("time.step", json!({"ticks": 60}));
    assert_eq!(stepped["tick"], stepped_open["tick"]);
    assert_eq!(stepped["world_hash"], stepped_open["world_hash"]);
    assert_eq!(hash(&locked), hash(&open));
    // The grants' refused calls, and their reads, moved nothing meanwhile.
    let _ = error(
        &locked.url,
        CHECKER,
        &request("time.step", json!({"ticks": 5})),
    );
    let _ = error(&locked.url, PLAYER, &request("world.edit", set_cargo(1)));
    let _ = result(&locked.url, CHECKER, "world.tree", json!({}));
    assert_eq!(hash(&locked), hash(&open));
}

/// A copy of a project, without what a host or a check left in it (`.amoris/`).
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".amoris" {
            continue;
        }
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

/// The built `amoris`, with no token and no host from the test's own environment.
fn amoris() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_amoris"));
    c.env_remove("AMORIS_TOKEN")
        .env_remove("AMORIS_HOST")
        .env_remove("AMORIS_TIMEOUT");
    c
}

/// The lines a stream writes, as they come.
fn lines(stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
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

/// Everything a stream writes, kept as it comes.
fn collect(stream: impl Read + Send + 'static) -> Arc<Mutex<String>> {
    let text = Arc::new(Mutex::new(String::new()));
    let kept = text.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                return;
            };
            let mut t = kept.lock().unwrap();
            t.push_str(&line);
            t.push('\n');
        }
    });
    text
}

/// One run of a client command in `dir`: its exit code, stdout and stderr.
fn cli(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let mut c = amoris();
    c.current_dir(dir).args(args).stdin(Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `amoris mcp ...` as an MCP client drives it: JSON-RPC lines on its stdin and stdout.
struct StdioSession {
    process: Process,
    stdin: Option<ChildStdin>,
    out: mpsc::Receiver<String>,
    err: Arc<Mutex<String>>,
    id: u64,
}

impl StdioSession {
    fn start(args: &[&str], env: &[(&str, &str)]) -> StdioSession {
        let mut c = amoris();
        c.arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        let out = lines(child.stdout.take().unwrap());
        let err = collect(child.stderr.take().unwrap());
        let stdin = child.stdin.take();
        StdioSession {
            process: Process(child),
            stdin,
            out,
            err,
            id: 0,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write(&msg);
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|e| {
                    panic!("no answer to {method}: {e}; {}", self.err.lock().unwrap())
                });
            let r: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
            if r["id"] == json!(id) {
                return r;
            }
        }
    }

    fn write(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{msg}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    /// `initialize`; the instructions the session gave.
    fn initialize(&mut self, name: &str) -> String {
        let init = self.send(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": name, "version": "9"}}),
        );
        assert!(init["result"]["serverInfo"].is_object(), "{init:#}");
        self.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        init["result"]["instructions"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }

    fn tools(&mut self) -> Vec<String> {
        let listed = self.send("tools/list", json!({}));
        listed["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{listed:#}"))
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_owned))
            .collect()
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

    /// The client closes the stream; the process's exit code and what it wrote to stderr.
    fn close(mut self) -> (Option<i32>, String) {
        self.stdin = None;
        let code = self.process.exit(Duration::from_secs(20));
        let err = self.err.lock().unwrap().clone();
        (code, err)
    }
}

/// `amoris mcp` with `args`, which is to end before it serves: its exit code, stdout, stderr.
fn mcp_ends(args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let mut c = amoris();
    c.arg("mcp")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().unwrap();
    // Held open: a session that did start would wait here, and the test would say so.
    let stdin = child.stdin.take();
    let out = collect(child.stdout.take().unwrap());
    let err = collect(child.stderr.take().unwrap());
    let mut process = Process(child);
    let code = process.exit(Duration::from_secs(60));
    drop(stdin);
    assert!(
        code.is_some(),
        "amoris mcp {args:?} did not end: {}",
        err.lock().unwrap()
    );
    // The readers end with the process's streams.
    std::thread::sleep(Duration::from_millis(100));
    let (out, err) = (out.lock().unwrap().clone(), err.lock().unwrap().clone());
    (code, out, err)
}

/// A scratch directory of this test run under the target's temporary directory.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn amoris_serve_with_grants_and_the_clients_that_carry_a_token() {
    let dir = scratch("locked-host");
    let project = dir.join("course");
    copy_dir(&course(), &project);
    // The grants file lies beside the project, never in it.
    let grants = dir.join("grants.json");
    std::fs::write(&grants, grants_file()).unwrap();
    let mut child = amoris()
        .args([
            "serve",
            project.to_str().unwrap(),
            "--port",
            "0",
            "--grants",
        ])
        .arg(&grants)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let ready = lines(child.stdout.take().unwrap());
    let serve_err = collect(child.stderr.take().unwrap());
    let _served = Process(child);
    let line = ready
        .recv_timeout(Duration::from_secs(120))
        .unwrap_or_else(|e| panic!("no readiness line: {e}; {}", serve_err.lock().unwrap()));
    let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let url = ready["url"].as_str().unwrap().to_owned();

    // The line and the host file say the host is locked, and say no token.
    assert_eq!(ready["locked"], json!(true), "{line}");
    says_no_token(&line, "the readiness line");
    let file = std::fs::read_to_string(project.join(".amoris/host.json")).unwrap();
    let host_file: Value = serde_json::from_str(&file).unwrap();
    assert_eq!(host_file["locked"], json!(true), "{file}");
    assert_eq!(host_file["url"], json!(url), "{file}");
    says_no_token(&file, ".amoris/host.json");
    // The debugger's CDP endpoint, which has no admission, is not started.
    let state = result(&url, DEVELOPER, "debug.state", json!({}));
    assert_eq!(state["cdp"], Value::Null, "{state:#}");
    let end = Instant::now() + Duration::from_secs(10);
    while !serve_err.lock().unwrap().contains("CDP endpoint is off") {
        assert!(Instant::now() < end, "{}", serve_err.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }

    // The CLI, from the project's directory: without a token it is nobody.
    let (code, out, err) = cli(&project, &["status"], &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(
        err.contains("host.unauthorized") && out.is_empty(),
        "{out}{err}"
    );
    let (code, out, err) = cli(&project, &["status", "--json"], &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("host.unauthorized"), "{out}{err}");
    // A developer's token, by the flag and by the environment.
    let (code, out, err) = cli(&project, &["status", "--token", DEVELOPER], &[]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.starts_with("tick "), "{out}");
    let (code, out, err) = cli(&project, &["status"], &[("AMORIS_TOKEN", DEVELOPER)]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.starts_with("tick "), "{out}");
    let (code, out, err) = cli(
        &project,
        &[
            "world", "set", "Crate4", "Cargo", "value=61", "--token", DEVELOPER,
        ],
        &[],
    );
    assert_eq!(code, Some(0), "{out}{err}");
    // The flag wins over the environment.
    let (code, out, err) = cli(
        &project,
        &["status", "--token", DEVELOPER],
        &[("AMORIS_TOKEN", UNKNOWN)],
    );
    assert_eq!(code, Some(0), "{out}{err}");
    let (code, _, err) = cli(&project, &["status"], &[("AMORIS_TOKEN", UNKNOWN)]);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("host.unauthorized"), "{err}");

    // A checker's token reads, and is refused the rest by the real catalog and debugger.
    let checker = [("AMORIS_TOKEN", CHECKER)];
    let (code, out, err) = cli(&project, &["world", "get", "Crate4", "Cargo"], &checker);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("61"), "{out}");
    for args in [
        &["world", "set", "Crate4", "Cargo", "value=3"][..],
        &["step", "5"],
        &["call", "debug.eval", "{\"expr\": \"1\"}"],
        &["debug", "eval", "1"],
        &["debug", "set", "x", "1"],
        &["scripts", "check"],
        &["scripts", "types"],
        &["scripts", "apply"],
        &["save"],
        &["undo"],
    ] {
        let (code, out, err) = cli(&project, args, &checker);
        assert_eq!(code, Some(1), "{args:?}: {out}{err}");
        assert!(err.contains("permission.denied"), "{args:?}: {err}");
        assert!(err.contains("this caller is a checker"), "{args:?}: {err}");
    }
    // The checker's reads of the debugger reach it.
    let (code, out, err) = cli(&project, &["debug", "state", "--json"], &checker);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("\"state\":\"running\""), "{out}");
    // Nothing a checker asked wrote the project: no declarations, the same tsconfig.json.
    assert!(!project.join(".amoris/types").exists());
    assert_eq!(
        std::fs::read(project.join("tsconfig.json")).unwrap(),
        std::fs::read(course().join("tsconfig.json")).unwrap()
    );

    // A player's token is its seat.
    let (code, out, err) = cli(&project, &["player", "observe", "--token", PLAYER], &[]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.starts_with("tick "), "{out}");
    let (code, out, err) = cli(&project, &["status", "--token", PLAYER], &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(
        err.contains("permission.denied") && err.contains("this caller is a player"),
        "{err}"
    );
    let (code, out, err) = cli(
        &project,
        &["player", "observe", "--seat", "lookout", "--token", PLAYER],
        &[],
    );
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("seat.not_yours"), "{err}");
    let (code, out, err) = cli(
        &project,
        &[
            "player", "session", "--seat", "skipper", "--token", PLAYER, "--json",
        ],
        &[],
    );
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("\"role\":\"player\""), "{out}");

    // What is no token is a usage error that does not repeat it.
    for bad in [
        "nothex-nothex-nothex-nothex-nope",
        &DEVELOPER[..31],
        "hunter2",
    ] {
        let (code, out, err) = cli(&project, &["status", "--token", bad], &[]);
        assert_eq!(code, Some(2), "{out}{err}");
        assert!(!err.contains(bad) && !out.contains(bad), "{out}{err}");
        let (code, out, err) = cli(&project, &["status"], &[("AMORIS_TOKEN", bad)]);
        assert_eq!(code, Some(2), "{out}{err}");
        assert!(!err.contains(bad) && !out.contains(bad), "{out}{err}");
    }
    // Help needs no token: without one it is the catalog this binary would serve.
    let (code, out, err) = cli(&project, &["help"], &[]);
    assert_eq!(code, Some(0), "{err}");
    for said in [
        "world.edit",
        "debug.eval",
        "agent.session.list",
        "--token",
        "AMORIS_TOKEN",
        "--grants",
    ] {
        assert!(out.contains(said), "help does not say {said}: {out}");
    }
    let (code, out, _) = cli(&project, &["help", "debug.eval"], &[]);
    assert_eq!(code, Some(0));
    assert!(out.contains("debug.eval (write)"), "{out}");

    // `amoris mcp --attach <url> --grant-token <token>`: the stdio session of one grant.
    let mut player = StdioSession::start(&["--attach", &url, "--grant-token", PLAYER], &[]);
    let instructions = player.initialize("claude-code");
    assert!(
        instructions.starts_with("You play one seat"),
        "{instructions}"
    );
    assert_eq!(player.tools(), ["player"]);
    let (failed, text) = player.tool("world", json!({"action": "tree"}));
    assert!(failed && text.contains("permission.denied"), "{text}");
    let (failed, text) = player.tool("player", json!({"action": "observe"}));
    assert!(!failed && text.starts_with("tick "), "{text}");
    let (failed, text) = player.tool("player", json!({"action": "session"}));
    assert!(!failed && text.contains("\"seat\":\"skipper\""), "{text}");
    // Its calls are the grant's row's, under the file's label, and it opened no row of its own.
    let rows = result(&url, DEVELOPER, "agent.session.list", json!({}))["sessions"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rows.len(), 3, "{rows:#?}");
    assert_eq!(
        (
            rows[0]["id"].as_str(),
            rows[0]["label"].as_str(),
            rows[0]["transport"].as_str()
        ),
        (Some("mcp-1"), Some("agent"), Some("grant")),
        "{rows:#?}"
    );
    assert_eq!(rows[0]["last_method"], json!("player.session"), "{rows:#?}");
    let (code, err) = player.close();
    assert_eq!(code, Some(0), "{err}");
    says_no_token(&err, "amoris mcp's stderr");
    assert!(err.contains("player grant 'agent'"), "{err}");
    // The grant's row outlives the bridge.
    let rows = result(&url, DEVELOPER, "agent.session.list", json!({}))["sessions"].clone();
    assert_eq!(rows[0]["live"], json!(true), "{rows:#}");

    // The checker's, by the environment and the project's host file.
    let mut checker_mcp = StdioSession::start(&[project.to_str().unwrap()], &checker);
    let instructions = checker_mcp.initialize("harness");
    assert!(instructions.starts_with("Amoris host"), "{instructions}");
    let (failed, text) = checker_mcp.tool("world", json!({"action": "get", "entity": "Crate4"}));
    assert!(!failed && text.contains("61"), "{text}");
    let (failed, text) = checker_mcp.tool("world", {
        let mut a = set_cargo(9);
        a["action"] = json!("edit");
        a
    });
    assert!(failed && text.contains("permission.denied"), "{text}");
    let (failed, text) = checker_mcp.tool("time", json!({"action": "step", "ticks": 3}));
    assert!(failed && text.contains("permission.denied"), "{text}");
    let (code, err) = checker_mcp.close();
    assert_eq!(code, Some(0), "{err}");
    assert!(!err.contains("in this process"), "{err}");

    // A token the host does not know ends the bridge before anything goes to stdout.
    let (code, out, err) = mcp_ends(&["--attach", &url, "--grant-token", UNKNOWN], &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("host.unauthorized"), "{err}");
    // Without a token the bridge is nobody, and starts no game of its own in the host's place.
    for args in [&[project.to_str().unwrap()][..], &["--attach", &url]] {
        let (code, out, err) = mcp_ends(args, &[]);
        assert_eq!(code, Some(1), "{args:?}: {out}{err}");
        assert!(out.is_empty(), "{out}");
        assert!(err.contains("host.unauthorized"), "{err}");
        assert!(!err.contains("in this process"), "{err}");
    }
    // A grant fixes its seat, and a game of its own takes no token.
    for args in [
        &[
            "--attach",
            &url,
            "--grant-token",
            PLAYER,
            "--seat",
            "skipper",
        ][..],
        &["--attach", &url, "--own"],
        &[project.to_str().unwrap(), "--own", "--grant-token", PLAYER],
        &["--attach", &url, "--grant-token", "hunter2"],
    ] {
        let (code, out, err) = mcp_ends(args, &[]);
        assert_eq!(code, Some(2), "{args:?}: {out}{err}");
        assert!(out.is_empty(), "{args:?}: {out}");
        assert!(err.contains("check.usage"), "{args:?}: {err}");
        assert!(!err.contains("hunter2"), "{err}");
    }
    let (code, _, err) = mcp_ends(
        &[project.to_str().unwrap(), "--seat", "skipper"],
        &[("AMORIS_TOKEN", PLAYER)],
    );
    assert_eq!(code, Some(2), "{err}");
    // No host where the token points: no game in its place.
    let nowhere = dir.join("nowhere");
    copy_dir(&course(), &nowhere);
    let (code, out, err) = mcp_ends(&[nowhere.to_str().unwrap(), "--grant-token", PLAYER], &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(
        err.contains("host.not_found") && !err.contains("in this process"),
        "{err}"
    );

    // A host without grants takes no token: the bridge says so and leaves.
    let open = Served::start(false);
    let (code, out, err) = mcp_ends(&["--attach", &open.url, "--grant-token", PLAYER], &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("host.not_locked"), "{err}");
    assert_eq!(open.rows(), Vec::<Value>::new());
    // By its port alone, as `--host` takes it.
    let port = open.addr.port().to_string();
    let (code, _, err) = mcp_ends(&["--attach", &port, "--grant-token", PLAYER], &[]);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("host.not_locked"), "{err}");
    // Without a token `--attach` is the bridge it always was, of the host it names: a session
    // that registers itself, a developer's, or with `--seat` that seat's player.
    let mut bridge = StdioSession::start(&["--attach", &open.url], &[]);
    bridge.initialize("attached");
    assert_eq!(bridge.tools().len(), 11);
    let mut seated = StdioSession::start(&["--attach", &port, "--seat", "skipper"], &[]);
    seated.initialize("seated");
    assert_eq!(seated.tools(), ["player"]);
    let rows = open.rows();
    let listed: Vec<Value> = rows
        .iter()
        .map(|r| json!([r["id"], r["transport"], r["role"], r["label"], r["live"]]))
        .collect();
    assert_eq!(
        listed,
        [
            json!(["mcp-1", "bridge", "developer", "attached 9", true]),
            json!(["mcp-2", "bridge", "player", "seated 9", true]),
        ]
    );
    for session in [bridge, seated] {
        let (code, err) = session.close();
        assert_eq!(code, Some(0), "{err}");
    }
    assert!(open.rows().iter().all(|r| r["live"] == json!(false)));
    // A host `--attach` names that does not answer is an error, never a game in its place.
    let (code, out, err) = mcp_ends(&["--attach", "127.0.0.1:9"], &[]);
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(
        err.contains("host.unreachable") && !err.contains("in this process"),
        "{err}"
    );
    drop(open);

    // Nothing the host wrote says a token.
    says_no_token(&serve_err.lock().unwrap(), "amoris serve's stderr");
    drop(_served);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `amoris serve <project> --grants <file>`, which is to refuse the file: the problem it prints.
fn serve_refuses(project: &Path, grants: &Path) -> Value {
    let child = amoris()
        .args([
            "serve",
            project.to_str().unwrap(),
            "--port",
            "0",
            "--grants",
        ])
        .arg(grants)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut process = Process(child);
    let code = process.exit(Duration::from_secs(120));
    let mut out = String::new();
    process
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let mut err = String::new();
    process
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert_eq!(code, Some(1), "{out}{err}");
    says_no_token(&out, "the refusal");
    says_no_token(&err, "the refusal's stderr");
    // No readiness line and no host file: nothing was served.
    assert!(!out.lines().any(|l| l.contains("\"url\"")), "{out}");
    assert!(!project.join(".amoris/host.json").exists());
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}{err}"));
    assert_eq!(v["error"]["code"], json!("host.grants_invalid"), "{out}");
    v["error"].clone()
}

#[test]
fn amoris_serve_refuses_a_bad_grants_file() {
    let dir = scratch("locked-host-refused");
    let project = dir.join("course");
    copy_dir(&course(), &project);
    let outside = dir.join("grants.json");

    // A grants file inside the project would be served to every checker and developer token.
    let inside = project.join("grants.json");
    std::fs::write(&inside, grants_file()).unwrap();
    let p = serve_refuses(&project, &inside);
    assert!(
        p["message"]
            .as_str()
            .unwrap()
            .contains("inside the project"),
        "{p:#}"
    );
    std::fs::remove_file(&inside).unwrap();

    // A field no grant takes yet is refused, never accepted and ignored.
    std::fs::write(
        &outside,
        json!({"grants": [{"token": DEVELOPER, "role": "developer", "profile": "omniscient_player"}]})
            .to_string(),
    )
    .unwrap();
    let p = serve_refuses(&project, &outside);
    assert_eq!(
        p["detail"]["problem"]["code"],
        json!("request.unknown_field"),
        "{p:#}"
    );
    assert_eq!(
        p["detail"]["problem"]["detail"]["path"],
        json!("/grants/0/profile"),
        "{p:#}"
    );
    // A policy is a player grant's alone (a developer always forks, a checker never does).
    std::fs::write(
        &outside,
        json!({"grants": [{"token": DEVELOPER, "role": "developer", "policy": {"fork": true}}]})
            .to_string(),
    )
    .unwrap();
    let p = serve_refuses(&project, &outside);
    assert_eq!(p["detail"]["field"], json!("policy"), "{p:#}");

    // A seat the game does not declare: found when the host locks, before it serves.
    std::fs::write(
        &outside,
        json!({"grants": [
            {"token": DEVELOPER, "role": "developer"},
            {"token": PLAYER, "role": "player", "seat": "lookout", "label": "nobody's"},
        ]})
        .to_string(),
    )
    .unwrap();
    let p = serve_refuses(&project, &outside);
    assert_eq!(
        p["detail"]["problem"]["code"],
        json!("seat.unknown"),
        "{p:#}"
    );
    assert_eq!(
        (
            p["detail"]["grant"].clone(),
            p["detail"]["label"].clone(),
            p["detail"]["field"].clone()
        ),
        (json!(1), json!("nobody's"), json!("seat")),
        "{p:#}"
    );

    // Two tokens for one seat, a token that is none, and a file that is not there.
    std::fs::write(
        &outside,
        json!({"grants": [
            {"token": PLAYER, "role": "player", "seat": "skipper"},
            {"token": CHECKER, "role": "player", "seat": "skipper"},
        ]})
        .to_string(),
    )
    .unwrap();
    let p = serve_refuses(&project, &outside);
    assert_eq!(
        p["detail"]["problem"]["code"],
        json!("session.seat_taken"),
        "{p:#}"
    );
    std::fs::write(
        &outside,
        json!({"grants": [{"token": &DEVELOPER[..30], "role": "developer"}]}).to_string(),
    )
    .unwrap();
    let p = serve_refuses(&project, &outside);
    assert_eq!(p["detail"]["field"], json!("token"), "{p:#}");
    assert!(!p.to_string().contains(&DEVELOPER[..30]), "{p:#}");
    let p = serve_refuses(&project, &dir.join("none.json"));
    assert!(
        p["detail"]["file"].as_str().unwrap().ends_with("none.json"),
        "{p:#}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
