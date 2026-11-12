//! `amoris serve <project> [--port 7878]` and `amoris mcp <project>` (docs/spec/server.md): the
//! project's game on its game thread (real time, paused: the edit world) with the host's server on
//! 127.0.0.1, or one MCP session over stdio, against a host already serving the project when there
//! is one (as an agent session of that host, server.md 3.5) and otherwise against a game of its
//! own. `amoris serve --grants <file>` locks the host (server.md 3.6): every request then needs a
//! grant's token, and `amoris mcp --attach <url> --grant-token <token>` is the stdio session of
//! one grant.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use amoris_contract::{Problem, detail};
use amoris_link::Source;
use amoris_mcp::{Backend, BoxFuture, Caller, Hello};
use amoris_runtime::Project;
use amoris_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use amoris_server::{GameAccess, Grants, Host, McpBackend, ServeOptions, hostfile};
use serde_json::{Value, json};

use crate::check::Outcome;
use crate::cli::{Args, Flag};

const SERVE_FLAGS: &[Flag] = &[
    ("port", true),
    ("editor", true),
    ("seed", true),
    ("grants", true),
    ("agents", true),
];
const MCP_FLAGS: &[Flag] = &[
    ("seed", true),
    ("own", false),
    ("seat", true),
    ("attach", true),
    ("grant-token", true),
];

fn fail(p: &Problem, code: i32) -> Outcome {
    Outcome {
        stdout: serde_json::to_string_pretty(&json!({"error": p})).unwrap_or_default(),
        code,
    }
}

/// The project directory, canonical and in the plain form other programs resolve it to (on
/// Windows without the `\\?\` prefix where it can do without, `hostfile::project_root`): the host
/// file, the readiness line and `project.info` report it, and the desktop compares it with
/// `fs.realpath`.
fn project_dir(args: &Args) -> Result<PathBuf, Problem> {
    let dir = PathBuf::from(args.positional.first().map_or(".", String::as_str));
    hostfile::project_root(&dir).map_err(|e| {
        Problem::new(
            "project.missing_file",
            format!("{} is not a project directory: {e}.", dir.display()),
            detail([("path", json!(dir.display().to_string()))]),
        )
    })
}

/// The project's game on its thread, real time and paused, and the host over it.
/// A served game: its thread, the host over it, and the script debugger (CDP on 127.0.0.1:9229
/// when the port is free).
struct Served {
    handle: GameHandle,
    host: Host,
    hub: amoris_debug::DebugHub,
    cdp: Option<amoris_debug::CdpServer>,
}

impl Served {
    fn shutdown(self) {
        self.hub.shutdown();
        drop(self.host);
        let _ = self.handle.shutdown(2000);
        if let Some(c) = self.cdp {
            c.stop();
        }
    }
}

fn start(root: &Path, seed: Option<u64>, editor: Option<PathBuf>) -> Result<Served, Problem> {
    start_with(root, seed, editor, true)
}

/// [`start`], with the debugger's CDP endpoint or without it. A locked host starts none: that
/// endpoint has no admission, and any local process that reached it could evaluate in a paused
/// frame, past every grant (server.md 3.6). The debugger itself stays, behind `debug.*`.
fn start_with(
    root: &Path,
    seed: Option<u64>,
    editor: Option<PathBuf>,
    cdp: bool,
) -> Result<Served, Problem> {
    let feed = amoris_assets::Feed::new();
    let project = Project::load(root)?;
    let seed = seed.unwrap_or(project.manifest.seed);
    let setup = Arc::new(project.setup(false)?);
    let start = Instant::now();
    let clock: amoris_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    // `/render` sends a frame when the game thread publishes (host-protocol.md 5).
    let (on_publish, wake) = crate::present::feed_wake();
    let options = ThreadOptions {
        pacing: Pacing::RealTime { speed: 1.0 },
        paused: true,
        feed: Some(feed.clone()),
        on_publish,
        ..ThreadOptions::new(clock)
    };
    let dir = root.to_path_buf();
    let hub = amoris_debug::DebugHub::new(amoris_debug::HubOptions {
        title: format!("Amoris: {}", project.manifest.name),
        ..amoris_debug::HubOptions::default()
    });
    let cdp = match cdp.then(|| hub.serve_cdp(amoris_debug::CdpOptions::default())) {
        Some(Ok(c)) => Some(c),
        Some(Err(e)) => {
            eprintln!(
                "amoris: the debugger's CDP endpoint did not start ({e}); debug.* still works"
            );
            None
        }
        None => None,
    };
    let h = hub.clone();
    let handle = GameThread::spawn(
        move || {
            let mut game = crate::water_compute::builder(setup, seed)
                .project(dir)
                .build()?;
            game.set_script_debugger(Some(h.hook()));
            Ok(game)
        },
        options,
    )?;
    hub.set_loop_state(handle.loop_state());
    let clients = handle.clients();
    let players = clients.clone();
    let access = GameAccess {
        reader: handle.reader(),
        editor: clients.client(Source::Editor)?,
        api: clients.developer(),
        developer: Arc::new(move || clients.developer()),
    };
    let host = Host::new(access, root.to_path_buf(), editor);
    // Calls made as a seat (`/api/call` with `seat`, `amoris mcp --seat`) come from that seat's
    // player source (docs/spec/player.md).
    host.set_players(Arc::new(move |i| players.client(Source::Player(i))));
    host.set_capture(Arc::new(crate::present::CaptureServer::start(
        &feed,
        root.to_path_buf(),
    )));
    host.set_render(Arc::new(crate::present::FeedServer { feed, wake }));
    host.set_debug(Arc::new(crate::present::DebugBridge { hub: hub.clone() }));
    crate::present::forward_debug_events(&hub, host.clone());
    Ok(Served {
        handle,
        host,
        hub,
        cdp,
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, Problem> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("amoris-server")
        .build()
        .map_err(|e| Problem::new("host.runtime", e.to_string(), detail([])))
}

/// Resolves when the host is asked to stop: Ctrl-C, SIGTERM on Unix, and on Windows Ctrl-Break,
/// the console event a parent can send a child in its own process group (what
/// `tools/host/smoke_server.py` sends; SIGTERM has no Windows counterpart a process can catch).
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    #[cfg(windows)]
    {
        if let Ok(mut brk) = tokio::signal::windows::ctrl_break() {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = brk.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// `amoris serve`.
pub fn serve(raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, SERVE_FLAGS) {
        Ok(a) => a,
        Err(p) => return fail(&p, 2),
    };
    match serving(&args) {
        Ok(()) => Outcome {
            stdout: String::new(),
            code: 0,
        },
        Err(p) => fail(&p, 1),
    }
}

fn serving(args: &Args) -> Result<(), Problem> {
    let root = project_dir(args)?;
    let port = args.number::<u16>("port")?.unwrap_or(7878);
    let editor = args
        .value("editor")
        .map(PathBuf::from)
        .or_else(amoris_server::find_editor_dist);
    // The grants are read before the game starts: a file that is refused starts nothing.
    let grants = match args.value("grants") {
        Some(file) => Some(Grants::load(Path::new(file), &root)?),
        None => None,
    };
    let locked = grants.as_ref().map(Grants::counts);
    // The agent programs the host may start (server.md 3.7), read before the game starts too.
    let launchers = match args.value("agents") {
        Some(file) => Some(amoris_server::Launchers::load(Path::new(file), &root)?),
        None => None,
    };
    let served = start_with(
        &root,
        args.number::<u64>("seed")?,
        editor.clone(),
        locked.is_none(),
    )?;
    let host = served.host.clone();
    let agents = launchers.as_ref().map_or(0, amoris_server::Launchers::len);
    if let Some(l) = launchers {
        host.set_launchers(l);
    }
    let rt = runtime()?;
    let r = rt.block_on(async {
        if let Some(grants) = grants {
            host.lock(grants).await?;
        }
        let (addr, task) = host.serve(&ServeOptions { port }, stopped()).await?;
        let file = hostfile::write_as(&root, addr.port(), locked.is_some())
            .map_err(|e| Problem::new("host.file", e.to_string(), detail([])))?;
        let mut ready = json!({"url": file.url, "port": addr.port(), "pid": file.pid,
                               "project": file.project, "mcp": format!("{}/mcp", file.url),
                               "editor": editor.as_ref().map(|e| e.display().to_string())});
        if locked.is_some() {
            // Said only by a locked host; no line, file or log of the host says a token.
            ready["locked"] = json!(true);
        }
        println!("{ready}");
        eprintln!(
            "amoris: serving {} at {} (editor {}, MCP {}/mcp); Ctrl-C stops",
            root.display(),
            file.url,
            if editor.is_some() {
                "built"
            } else {
                "not built"
            },
            file.url
        );
        if agents > 0 {
            eprintln!("amoris: {agents} agent programs may be started (agent.session.start)");
        }
        if let Some((players, checkers, developers)) = locked {
            eprintln!(
                "amoris: locked: grants for {players} player, {checkers} checker and \
                 {developers} developer tokens, and every request needs one; the debugger's \
                 CDP endpoint is off"
            );
        }
        let _ = task.await;
        Ok::<(), Problem>(())
    });
    hostfile::remove(&root);
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    drop(host);
    served.shutdown();
    r
}

/// How long the bridge waits for the host to register its session.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long it waits, on its way out, for the host to close it.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// `amoris mcp` attached to a host serving the project (server.md 3.5): one agent session of
/// that host. It registers with `agent.session.open` when its MCP client says who it is (with a
/// seat, as that seat's player), makes its calls with the handle the host answered, registers
/// again when the host no longer knows the handle (it started again), and closes its session on
/// its way out. So the host lists it, its calls are in the `agent` stream and its edits are its
/// own in the history, not the shared `api` client's.
struct Bridge {
    url: String,
    seat: Option<String>,
    /// What the MCP client calls itself, from its hello.
    label: std::sync::Mutex<String>,
    /// The session's id and handle on the host, once registered. Held while registering, so
    /// calls that find it missing or stale together open one session.
    registered: tokio::sync::Mutex<Option<(String, String)>>,
    /// The host opened the session for the program that runs this bridge ([`Bridge::given`]).
    given: bool,
}

/// Runs a blocking HTTP call off the runtime's workers.
async fn blocking(
    call: impl FnOnce() -> Result<Value, Problem> + Send + 'static,
) -> Result<Value, Problem> {
    tokio::task::spawn_blocking(call)
        .await
        .unwrap_or_else(|e| Err(Problem::new("host.unreachable", e.to_string(), detail([]))))
}

impl Bridge {
    fn new(url: String, seat: Option<String>) -> Bridge {
        Bridge {
            url,
            seat,
            label: std::sync::Mutex::new(String::new()),
            registered: tokio::sync::Mutex::new(None),
            given: false,
        }
    }

    /// The bridge of a program the host started (`agent.session.start`, server.md 3.7): the
    /// session the host opened for it, from `AMORIS_AGENT_SESSION` and `AMORIS_AGENT_HANDLE`,
    /// with its seat from `AMORIS_AGENT_SEAT`. It registers nothing and closes nothing: the host
    /// closes the session when the program exits.
    fn given(url: String) -> Option<Bridge> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (session, handle) = (var("AMORIS_AGENT_SESSION")?, var("AMORIS_AGENT_HANDLE")?);
        Some(Bridge {
            url,
            seat: var("AMORIS_AGENT_SEAT"),
            label: std::sync::Mutex::new(String::new()),
            registered: tokio::sync::Mutex::new(Some((session, handle))),
            given: true,
        })
    }

    /// Asks the host for a session: its id and its handle.
    async fn register(&self) -> Result<(String, String), Problem> {
        let label = self.label.lock().map(|l| l.clone()).unwrap_or_default();
        let mut params = json!({"label": label});
        if let Some(seat) = &self.seat {
            params["seat"] = json!(seat);
        }
        let url = self.url.clone();
        let row = blocking(move || {
            crate::client::call_url_within(&url, "agent.session.open", params, REGISTER_TIMEOUT)
        })
        .await?;
        match (row["id"].as_str(), row["handle"].as_str()) {
            (Some(id), Some(handle)) => Ok((id.to_owned(), handle.to_owned())),
            _ => Err(Problem::new(
                "host.error",
                "agent.session.open answered no session handle.",
                detail([("answer", row)]),
            )),
        }
    }

    /// The session's handle, registering first when there is none.
    async fn handle(&self) -> Result<String, Problem> {
        let mut registered = self.registered.lock().await;
        if let Some((_, handle)) = registered.as_ref() {
            return Ok(handle.clone());
        }
        let session = self.register().await?;
        let handle = session.1.clone();
        *registered = Some(session);
        Ok(handle)
    }

    /// A handle in place of `stale`, which the host refused: registers again, unless another
    /// call already did.
    async fn renewed(&self, stale: &str) -> Result<String, Problem> {
        let mut registered = self.registered.lock().await;
        if let Some((_, handle)) = registered.as_ref()
            && handle != stale
        {
            return Ok(handle.clone());
        }
        let session = self.register().await?;
        eprintln!(
            "amoris mcp: the host knew this session no longer; registered again as {}",
            session.0
        );
        let handle = session.1.clone();
        *registered = Some(session);
        Ok(handle)
    }

    async fn post(&self, handle: String, method: String, params: Value) -> Result<Value, Problem> {
        let url = self.url.clone();
        blocking(move || {
            let timeout = crate::client::timeout_for(None, &method, &params);
            crate::client::call_url_in(&url, &method, params, &handle, timeout)
        })
        .await
    }

    /// One call as the session; a host that forgot the session gets it again, once.
    async fn call(&self, method: String, params: Value) -> Result<Value, Problem> {
        let handle = self.handle().await?;
        match self
            .post(handle.clone(), method.clone(), params.clone())
            .await
        {
            // A given session is the host's: when it is gone, so is the program's place.
            Err(p) if p.code == "host.session_unknown" && !self.given => {
                let handle = self.renewed(&handle).await?;
                self.post(handle, method, params).await
            }
            answer => answer,
        }
    }

    /// Closes the session on the host, if it has one: best effort, the bridge is leaving.
    async fn close(&self) {
        if self.given {
            return;
        }
        let Some((_, handle)) = self.registered.lock().await.take() else {
            return;
        };
        let url = self.url.clone();
        let _ = blocking(move || {
            crate::client::call_url_within(
                &url,
                "agent.session.close",
                json!({"handle": handle}),
                CLOSE_TIMEOUT,
            )
        })
        .await;
    }
}

/// `amoris mcp --attach <url> --grant-token <token>` (server.md 3.6; shared/contract/mcp.md 10):
/// the stdio session of one grant of a locked host. Each call is `POST /api/call` with the
/// grant's token as its bearer, so it is the grant's session on the host, listed under the
/// grant's label. It registers nothing, holds no handle and closes nothing: the grant's session
/// is the host's, open for the host's life.
#[derive(Clone)]
struct Granted {
    url: Arc<str>,
    token: Arc<str>,
    /// The seat of a player grant: the session then lists the `player` tool alone.
    seat: Option<String>,
}

impl Caller for Granted {
    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let (url, token) = (self.url.clone(), self.token.clone());
        Box::pin(blocking(move || {
            let timeout = crate::client::timeout_for(None, &method, &params);
            crate::client::call_url_granted(&url, &method, params, None, Some(&token), timeout)
        }))
    }

    fn seat(&self) -> Option<String> {
        self.seat.clone()
    }
}

impl Backend for Granted {
    fn open(&self) -> Arc<dyn Caller> {
        Arc::new(self.clone())
    }
}

/// A host serving the project over HTTP, as `amoris mcp` reaches it: the bridge's session.
struct Remote(Arc<Bridge>);

impl Caller for Remote {
    /// The client said who it is: the bridge registers under that name. A failure is left for
    /// the first call to report.
    fn hello(&self, hello: Hello) -> BoxFuture<()> {
        let bridge = self.0.clone();
        Box::pin(async move {
            if let Ok(mut label) = bridge.label.lock() {
                *label = hello.label;
            }
            let _ = bridge.handle().await;
        })
    }

    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let bridge = self.0.clone();
        Box::pin(async move { bridge.call(method, params).await })
    }

    fn seat(&self) -> Option<String> {
        self.0.seat.clone()
    }
}

impl Backend for Remote {
    fn open(&self) -> Arc<dyn Caller> {
        Arc::new(Remote(self.0.clone()))
    }
}

/// What `amoris mcp`'s flags ask for, settled before anything starts.
struct McpPlan {
    /// `--own`: a game of this process's own, whatever host serves the project.
    own: bool,
    /// `--attach <url|port>`: the host to be a session of. No game of its own is started then.
    attach: Option<String>,
    /// A grant's token (`--grant-token`, else `AMORIS_TOKEN`): the session is that grant's.
    token: Option<String>,
    seat: Option<String>,
}

fn mcp_plan(args: &Args) -> Result<McpPlan, Problem> {
    let flags: Vec<&str> = MCP_FLAGS.iter().map(|(n, _)| *n).collect();
    let own = args.has("own");
    let attach = args.value("attach").map(crate::client::host_of);
    for other in ["attach", "grant-token"] {
        if own && args.has(other) {
            return Err(crate::cli::usage(
                format!("--own runs a game of this process's own; --{other} is for a running host"),
                other,
                &flags,
            ));
        }
    }
    // `AMORIS_TOKEN` is the token of a running host's grant: a game of its own takes none.
    let token = if own {
        None
    } else {
        crate::client::token_of(args.value("grant-token"))?
    };
    let seat = args.value("seat").map(str::to_owned);
    if token.is_some() && seat.is_some() {
        return Err(crate::cli::usage(
            "a grant fixes its role and its seat: --seat is not given with a token \
             (--grant-token, AMORIS_TOKEN)"
                .to_owned(),
            "seat",
            &flags,
        ));
    }
    Ok(McpPlan {
        own,
        attach,
        token,
        seat,
    })
}

/// `amoris mcp`.
pub fn mcp(raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, MCP_FLAGS) {
        Ok(a) => a,
        Err(p) => return fail(&p, 2),
    };
    let plan = match mcp_plan(&args) {
        Ok(p) => p,
        Err(p) => {
            // To stderr: stdout is the MCP client's.
            eprintln!("{}", json!({"error": p}));
            return Outcome {
                stdout: String::new(),
                code: 2,
            };
        }
    };
    match mcp_session(&args, plan) {
        Ok(()) => Outcome {
            stdout: String::new(),
            code: 0,
        },
        Err(p) => {
            eprintln!("{}", json!({"error": p}));
            Outcome {
                stdout: String::new(),
                code: 1,
            }
        }
    }
}

/// The stdio session of the grant `token` is on the locked host at `url`. The grant is asked of
/// the host first, so a token the host does not know, a host without grants and a host that
/// does not answer end the process before anything goes to stdout.
fn granted_session(rt: &tokio::runtime::Runtime, url: &str, token: &str) -> Result<(), Problem> {
    let grant = crate::client::grant(url, token)?;
    let role = grant["role"].as_str().unwrap_or("");
    eprintln!(
        "amoris mcp: using the host at {url} as its {role} grant '{}' (session {})",
        grant["label"].as_str().unwrap_or(""),
        grant["session"].as_str().unwrap_or("")
    );
    let backend: Arc<dyn Backend> = Arc::new(Granted {
        url: Arc::from(url),
        token: Arc::from(token),
        seat: (role == "player")
            .then(|| grant["seat"].as_str().map(str::to_owned))
            .flatten(),
    });
    // Until the client closes stdin, or the bridge is asked to stop.
    rt.block_on(async {
        tokio::select! {
            r = amoris_mcp::serve_stdio(backend) => r,
            () = stopped() => Ok(()),
        }
    })
}

fn mcp_session(args: &Args, plan: McpPlan) -> Result<(), Problem> {
    // With `--attach` the host is named, and the project argument is not needed.
    let root = match plan.attach {
        Some(_) => None,
        None => Some(project_dir(args)?),
    };
    let rt = runtime()?;
    // The host to be a session of: the one `--attach` names, else the one serving the project.
    let target = plan.attach.clone().or_else(|| {
        root.as_ref()
            .filter(|_| !plan.own)
            .and_then(|root| hostfile::read(&hostfile::path(root)))
            .map(|f| f.url)
    });
    if let Some(token) = &plan.token {
        // A grant's session is a running host's: never a game of this process's own.
        let Some(url) = target else {
            return Err(Problem::new(
                "host.not_found",
                "A grant's token needs the host it is a grant of: no host serves this project \
                 (.amoris/host.json); name one with --attach <url|port>.",
                detail([]),
            ));
        };
        let r = granted_session(&rt, &url, token);
        rt.shutdown_timeout(Duration::from_secs(2));
        return r;
    }
    let running = match target {
        None => None,
        Some(url) => match crate::client::call_url(&url, "status", json!({})) {
            Ok(_) => Some(url),
            // The host `--attach` names must answer: no game is started in its place.
            Err(p) if plan.attach.is_some() => return Err(p),
            // A locked host serving the project: a private game here would not be the world
            // the client was pointed at.
            Err(p) if p.code == "host.unauthorized" => {
                eprintln!(
                    "amoris mcp: the host at {url} is locked: give its grant's token with \
                     --grant-token or AMORIS_TOKEN, or --own for a game of this process's own"
                );
                return Err(p);
            }
            Err(_) => None,
        },
    };
    let seat = plan.seat;
    if let Some(url) = running {
        eprintln!("amoris mcp: using the host at {url}");
        // A program the host started calls as the session the host opened for it.
        let bridge = match (&seat, plan.attach.is_some()) {
            (None, true) => Bridge::given(url.clone()),
            _ => None,
        }
        .unwrap_or_else(|| Bridge::new(url, seat));
        let bridge = Arc::new(bridge);
        let backend: Arc<dyn Backend> = Arc::new(Remote(bridge.clone()));
        // Until the client closes stdin, or the bridge is asked to stop; either way its session
        // on the host is closed before it leaves.
        let r = rt.block_on(async {
            let r = tokio::select! {
                r = amoris_mcp::serve_stdio(backend) => r,
                () = stopped() => Ok(()),
            };
            bridge.close().await;
            r
        });
        rt.shutdown_timeout(Duration::from_secs(2));
        return r;
    }
    // Reached without `--attach` only: the project was named.
    let root = match root {
        Some(root) => root,
        None => project_dir(args)?,
    };
    let served = start(&root, args.number::<u64>("seed")?, None)?;
    eprintln!("amoris mcp: running {} in this process", root.display());
    let backend: Arc<dyn Backend> = match &seat {
        Some(s) => Arc::new(McpBackend::player(served.host.clone(), s)),
        None => Arc::new(McpBackend::new(served.host.clone())),
    };
    let r = rt.block_on(amoris_mcp::serve_stdio(backend));
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    served.shutdown();
    r
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use amoris_assets::RenderFrame;
    use amoris_server::{ServeOptions, Via};
    use serde_json::json;

    use super::{Bridge, runtime, start, start_with};
    use crate::present::FEED_GAP;

    /// Each frame `/render` sends, with the time it was read and its tick.
    fn read_frames(addr: SocketAddr) -> mpsc::Receiver<(Instant, u64)> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut ws, _) = tungstenite::connect(format!("ws://{addr}/render")).unwrap();
            while let Ok(msg) = ws.read() {
                if let tungstenite::Message::Binary(b) = msg {
                    let frame = RenderFrame::decode(&b).unwrap();
                    if tx.send((Instant::now(), frame.tick)).is_err() {
                        return;
                    }
                }
            }
        });
        rx
    }

    /// A game stepped faster than real time publishes after every tick; a `/render` socket sends
    /// at most one frame every `FEED_GAP`, each merging what was published since the one before,
    /// so the last frame describes the last tick (host-protocol.md 5).
    #[test]
    fn render_merges_publications_closer_than_the_gap() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing");
        let served = start(&root, None, None).unwrap();
        let host = served.host.clone();
        let rt = runtime().unwrap();
        let (addr, _task) = rt
            .block_on(host.serve(&ServeOptions { port: 0 }, std::future::pending()))
            .unwrap();
        let frames = read_frames(addr);
        // The new subscriber's reset frame, sent while the edit world waits.
        let (_, first) = frames.recv_timeout(Duration::from_secs(20)).unwrap();
        let ticks = 240;
        let from = Instant::now();
        let stepped = rt
            .block_on(host.call(&Via::Api, "time.step", json!({"ticks": ticks})))
            .unwrap();
        let last_tick = stepped["tick"].as_u64().unwrap();
        assert_eq!(last_tick, first + ticks);
        let mut got = Vec::new();
        while got.last().is_none_or(|&(_, tick)| tick < last_tick) {
            got.push(frames.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        let (read, tick) = *got.last().unwrap();
        assert_eq!(tick, last_tick, "the merged frame ends on the last tick");
        // Frames were sent from `from` on, each at least FEED_GAP after the one before, and the
        // last was read at `read`: no more than this many fit, whatever the reader's delays.
        let span = read.duration_since(from).as_secs_f64();
        let most = (span / FEED_GAP.as_secs_f64()).floor() as usize + 1;
        assert!(
            got.len() <= most,
            "{} frames in {:.1} ms for {ticks} publications: at most {most} fit",
            got.len(),
            span * 1000.0
        );
        assert!(
            got.windows(2).all(|w| w[0].1 < w[1].1),
            "ticks only go forward"
        );
        drop(rt);
        served.shutdown();
    }
    /// A locked host starts no CDP endpoint (server.md 3.6): the debugger stays, behind the
    /// host's `debug.*`, and says it serves none.
    #[test]
    fn a_host_started_without_cdp_serves_none() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing");
        let served = start_with(&root, None, None, false).unwrap();
        assert!(served.cdp.is_none());
        let rt = runtime().unwrap();
        let state = rt
            .block_on(served.host.call(&Via::Api, "debug.state", json!({})))
            .unwrap();
        assert_eq!(state["cdp"], serde_json::Value::Null, "{state:#}");
        assert_eq!(state["state"], json!("running"), "{state:#}");
        drop(rt);
        served.shutdown();
    }

    /// `amoris mcp` attached to a host that started again holds a handle the new host never
    /// gave: its calls are refused with `host.session_unknown`, the bridge registers again and
    /// retries, and two calls that find the handle stale together open one session.
    #[test]
    fn a_bridge_registers_again_after_its_host_forgot_it() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing");
        let served = start(&root, None, None).unwrap();
        let host = served.host.clone();
        let rt = runtime().unwrap();
        let (addr, _task) = rt
            .block_on(host.serve(&ServeOptions { port: 0 }, std::future::pending()))
            .unwrap();
        let bridge = Bridge::new(format!("http://{addr}"), None);
        *bridge.label.lock().unwrap() = "bridge-test 1".to_owned();
        let (a, b) = rt.block_on(async {
            // A session of another host run.
            *bridge.registered.lock().await = Some(("mcp-7".to_owned(), "0-7".to_owned()));
            tokio::join!(
                bridge.call("status".to_owned(), json!({})),
                bridge.call("status".to_owned(), json!({}))
            )
        });
        assert!(a.is_ok(), "{a:#?}");
        assert!(b.is_ok(), "{b:#?}");
        let list = rt
            .block_on(host.call(&Via::Api, "agent.session.list", json!({})))
            .unwrap();
        let rows = list["sessions"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{list:#}");
        let row = &rows[0];
        assert_eq!(
            (
                row["transport"].as_str(),
                row["label"].as_str(),
                row["live"].as_bool(),
                row["calls"].as_u64()
            ),
            (Some("bridge"), Some("bridge-test 1"), Some(true), Some(2)),
            "{list:#}"
        );
        // On its way out the bridge closes its session, once.
        rt.block_on(bridge.close());
        rt.block_on(bridge.close());
        let list = rt
            .block_on(host.call(&Via::Api, "agent.session.list", json!({})))
            .unwrap();
        assert_eq!(list["sessions"][0]["live"], json!(false), "{list:#}");
        drop(rt);
        served.shutdown();
    }
}
