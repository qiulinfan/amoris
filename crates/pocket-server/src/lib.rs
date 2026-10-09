//! The host's server (docs/spec/host-protocol.md; docs/spec/server.md): the editor's WebSocket,
//! the HTTP command API the CLI and agents use, the project's assets, the editor's static files and
//! MCP over Streamable HTTP, all on one loopback port.
//!
//! The server holds no game. It reaches the game thread only through `pocket-link`: a
//! [`SnapshotReader`] for what it shows and pushes, and [`GameClient`]s for commands (the editor's,
//! one shared by the HTTP API, one per MCP session). Every request is a catalog call
//! ([`Host::call`]); a few are answered here because they read the presenter's side of the link (the
//! event and log streams), the project's files (`assets.list`) or plug-ins the integrator installs
//! (the debugger, captures, the render feed).

mod dispatch;
pub mod hostfile;
mod http;
mod local;
mod mcp;
mod paused;
mod push;
pub mod typecheck;
mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use pocket_contract::{Problem, detail};
use pocket_link::{GameClient, SnapshotReader};
use serde_json::{Value, json};
use tokio::sync::broadcast;

pub use dispatch::Via;
pub use http::{find_editor_dist, router};
pub use local::{RewindParams, debug_catalog, server_catalog};
pub use mcp::McpBackend;
pub use paused::PAUSED_READS;

/// A boxed future, as the plug-in traits return them.
pub type BoxFuture<T> = pocket_mcp::BoxFuture<T>;

/// The script debugger as the server reaches it (`pocket-debug`'s hub, installed by the
/// integrator): `debug.*` methods by name (`debug.state`, `debug.breakpoints.set`, ...). The
/// server serves `debug.rewind` itself and resolves `debug.watch`'s entity names first.
pub trait DebugHub: Send + Sync {
    fn call(&self, method: &str, params: Value) -> BoxFuture<Result<Value, Problem>>;

    /// The methods' catalog entries, `{name, kind, doc, aliases, params}` with `params` their
    /// JSON Schema (pocket-debug's `methods()`): `catalog.list`, `pocket help debug.watch` and
    /// `docs.search` describe the debugger like every other command.
    fn methods(&self) -> Vec<Value> {
        Vec::new()
    }
}

/// Renders a camera view for `capture` (an image or the id buffer's summary).
pub trait CaptureHub: Send + Sync {
    fn capture(&self, params: Value) -> BoxFuture<Result<Value, Problem>>;
}

/// The render feed on `/render` (host-protocol.md 5): the integrator serves the socket.
pub trait RenderFeed: Send + Sync {
    fn serve(&self, socket: axum::extract::ws::WebSocket) -> BoxFuture<()>;
}

/// What the server gets of the game.
pub struct GameAccess {
    pub reader: SnapshotReader,
    /// The editor's client (`Source::Editor`), shared by every editor socket.
    pub editor: GameClient,
    /// The HTTP API's client (a developer source), shared by CLI calls.
    pub api: GameClient,
    /// Opens the next developer client, one per MCP session.
    pub developer: Arc<dyn Fn() -> GameClient + Send + Sync>,
}

/// One pushed event: its topic and its frame, serialized once for every socket.
#[derive(Clone, Debug)]
pub struct Push {
    pub topic: &'static str,
    pub frame: Arc<str>,
}

/// The server's shared state; cheap to clone.
#[derive(Clone)]
pub struct Host(Arc<Inner>);

struct Inner {
    reader: SnapshotReader,
    editor: Arc<Mutex<GameClient>>,
    api: Arc<Mutex<GameClient>>,
    developer: Arc<dyn Fn() -> GameClient + Send + Sync>,
    project: PathBuf,
    editor_dist: Option<PathBuf>,
    push: broadcast::Sender<Push>,
    catalog: OnceLock<Value>,
    debug: RwLock<Option<Arc<dyn DebugHub>>>,
    capture: RwLock<Option<Arc<dyn CaptureHub>>>,
    render: RwLock<Option<Arc<dyn RenderFeed>>>,
    sessions: AtomicU64,
}

/// Where the server is.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// 0: any free port.
    pub port: u16,
}

impl Host {
    /// A host over `game` for the project at `project`; `editor_dist` holds the built editor.
    pub fn new(game: GameAccess, project: PathBuf, editor_dist: Option<PathBuf>) -> Host {
        let (push, _) = broadcast::channel(1024);
        Host(Arc::new(Inner {
            reader: game.reader,
            editor: Arc::new(Mutex::new(game.editor)),
            api: Arc::new(Mutex::new(game.api)),
            developer: game.developer,
            project,
            editor_dist,
            push,
            catalog: OnceLock::new(),
            debug: RwLock::new(None),
            capture: RwLock::new(None),
            render: RwLock::new(None),
            sessions: AtomicU64::new(0),
        }))
    }

    pub fn reader(&self) -> &SnapshotReader {
        &self.0.reader
    }

    pub fn project(&self) -> &PathBuf {
        &self.0.project
    }

    /// Installs the debugger (`debug.*`).
    pub fn set_debug(&self, hub: Arc<dyn DebugHub>) {
        if let Ok(mut d) = self.0.debug.write() {
            *d = Some(hub);
        }
    }

    /// Installs the capture renderer (`capture`).
    pub fn set_capture(&self, hub: Arc<dyn CaptureHub>) {
        if let Ok(mut c) = self.0.capture.write() {
            *c = Some(hub);
        }
    }

    /// Installs the render feed (`/render`).
    pub fn set_render(&self, feed: Arc<dyn RenderFeed>) {
        if let Ok(mut r) = self.0.render.write() {
            *r = Some(feed);
        }
    }

    /// Pushes an event to the editors subscribed to its topic.
    pub fn push(&self, topic: &'static str, data: Value) {
        if self.0.push.receiver_count() == 0 {
            return;
        }
        let frame = json!({"event": topic, "data": data}).to_string();
        let _ = self.0.push.send(Push {
            topic,
            frame: Arc::from(frame),
        });
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Push> {
        self.0.push.subscribe()
    }

    /// Starts the background pushers (status, world.changed, events, log) on the current runtime.
    pub fn start_pushers(&self) {
        tokio::spawn(push::run(self.clone()));
    }

    /// Binds `127.0.0.1:<port>` and serves until `shutdown` resolves; returns the bound address
    /// and the server's task.
    pub async fn serve(
        &self,
        opts: &ServeOptions,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), Problem> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", opts.port))
            .await
            .map_err(|e| {
                Problem::new(
                    "host.bind_failed",
                    format!("127.0.0.1:{} cannot be bound: {e}.", opts.port),
                    detail([("port", json!(opts.port)), ("error", json!(e.to_string()))]),
                )
            })?;
        let addr = listener
            .local_addr()
            .map_err(|e| Problem::new("host.bind_failed", e.to_string(), detail([])))?;
        // The runtime's catalog, fetched while the game answers: a host held at a breakpoint
        // still lists its commands.
        let _ = self.catalog().await;
        self.start_pushers();
        let app = router(self.clone(), addr.port());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await;
        });
        Ok((addr, task))
    }
}
