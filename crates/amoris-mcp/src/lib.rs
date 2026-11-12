//! The MCP server (docs/spec/host-protocol.md 7; docs/spec/server.md, MCP; architecture.md 4.12): a
//! projection of the command catalog into a few grouped tools ([`tools`]), served over stdio
//! (`amoris mcp <project>`) and Streamable HTTP (`POST /mcp` inside `amoris-server`).
//!
//! The crate holds no game: every tool call becomes one catalog call through a [`Caller`], which
//! the host gives each MCP handler ([`Backend::open`]) and which is told once who its client is
//! ([`Caller::hello`]); the host decides which game client the session uses and pushes each call
//! to editors as an `agent` event. Errors come back as tool results flagged `isError` whose text
//! is the problem `{code, message, detail}`, so the agent reads the code and the "did you mean"
//! suggestions.

pub mod tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use amoris_contract::Problem;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use tokio::sync::OnceCell;

/// A boxed future the host's callers return.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// How an MCP client reaches the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Newline-delimited JSON-RPC over a byte stream (`amoris mcp`).
    Stdio,
    /// Streamable HTTP (`POST /mcp`).
    Http,
}

/// Who a caller's client is ([`Caller::hello`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The client's `clientInfo` as `name version`, trimmed; empty when it gave none.
    pub label: String,
    pub transport: Transport,
    /// This handler serves one request of a client that keeps no session (MCP 2026-07-28 over
    /// HTTP): the client's next request comes to another handler and says hello again.
    pub per_request: bool,
}

/// One MCP session's way to call the catalog.
pub trait Caller: Send + Sync {
    /// Who the client is. Told once, before the caller's first call: when the client sends
    /// `initialize`, or with the first tool call of a client that sent none. A caller no client
    /// ever called through is told nothing.
    fn hello(&self, _hello: Hello) -> BoxFuture<()> {
        Box::pin(std::future::ready(()))
    }

    /// Calls a catalog method with JSON parameters.
    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>>;

    /// The seat a player session plays (docs/spec/player.md): it lists the `player` tool alone,
    /// and the runtime restricts its calls to the seat's own perception and actions.
    fn seat(&self) -> Option<String> {
        None
    }
}

/// The host side: a caller for each MCP handler.
pub trait Backend: Send + Sync {
    /// A caller for a new handler. Over HTTP rmcp asks for a handler for every `initialize`,
    /// for every request of a client that keeps no session (MCP 2026-07-28), for
    /// `server/discover` and for its own look-ups of a tool's schema, so this must be cheap and
    /// open nothing: a session opens at its [`Caller::hello`], or at its first call.
    fn open(&self) -> Arc<dyn Caller>;
}

const INSTRUCTIONS: &str = "Amoris host: one authoritative world shared with the editor. \
Read with world tree/get/query, change it with world edit (undoable), run time with time step \
(until/watch stop early), test in play start/stop. Game rules are TypeScript in scripts/: \
scripts guide says how to write them, scripts check type checks them against the project's \
components. A game with players is played with the player tool (describe, observe, act, wait). \
Errors are {code, message, detail} with did-you-mean suggestions.";

const PLAYER_INSTRUCTIONS: &str = "You play one seat of an Amoris game through the player tool. \
Read the rules once with describe (part intents/kinds/events for details), then loop: observe \
(what your seat perceives: instruments, intents, percepts, events), act (start intents such as \
sail_to, use affordances such as take_aboard, set controls), wait (time runs until your next \
decision point: an intent finished, something sighted, a mark rounded). Entities are Name#id; \
you know only what your seat perceives. Errors are {code, message, detail} with suggestions.";

/// One session's MCP handler.
#[derive(Clone)]
pub struct AmorisMcp {
    caller: Arc<dyn Caller>,
    /// Served over Streamable HTTP, where a client may keep no session.
    http: bool,
    /// Set once the caller has been told who its client is.
    greeted: Arc<OnceCell<()>>,
}

impl AmorisMcp {
    /// A handler over a byte stream (stdio): one client for as long as the stream lasts.
    pub fn new(caller: Arc<dyn Caller>) -> AmorisMcp {
        AmorisMcp {
            caller,
            http: false,
            greeted: Arc::new(OnceCell::new()),
        }
    }

    /// A handler of the Streamable HTTP service.
    fn over_http(caller: Arc<dyn Caller>) -> AmorisMcp {
        AmorisMcp {
            http: true,
            ..AmorisMcp::new(caller)
        }
    }

    /// Tells the caller who its client is, the first time only; two first requests racing on
    /// one stream wait for the same hello.
    async fn greet(&self, client: Option<&Implementation>, per_request: bool) {
        self.greeted
            .get_or_init(|| {
                self.caller.hello(Hello {
                    label: client.map(label_of).unwrap_or_default(),
                    transport: if self.http {
                        Transport::Http
                    } else {
                        Transport::Stdio
                    },
                    per_request,
                })
            })
            .await;
    }
}

/// A client's `clientInfo` as one label: `claude-code 2.1.0`.
fn label_of(client: &Implementation) -> String {
    format!("{} {}", client.name.trim(), client.version.trim())
        .trim()
        .to_owned()
}

/// The tools as rmcp declares them.
pub fn tool_list() -> Vec<Tool> {
    tool_list_for(false)
}

/// The tools a session lists: a player session the `player` tool alone (mcp.md 3.3).
pub fn tool_list_for(player: bool) -> Vec<Tool> {
    tools::tools()
        .into_iter()
        .filter(|t| !player || t.name == "player")
        .map(|t| {
            let schema = match t.schema {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            Tool::new(t.name, t.description, Arc::new(schema))
        })
        .collect()
}

fn problem_text(p: &Problem) -> String {
    serde_json::to_string(&json!({"error": p})).unwrap_or_else(|_| p.message.clone())
}

impl ServerHandler for AmorisMcp {
    fn get_info(&self) -> ServerConfig {
        let instructions = if self.caller.seat().is_some() {
            PLAYER_INSTRUCTIONS
        } else {
            INSTRUCTIONS
        };
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("amoris", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }

    /// rmcp's default (keep the client's handshake, negotiate the protocol version), and the
    /// session's hello before the answer leaves: a client that has its `initialize` answer is
    /// a session the host lists.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        let result = self.negotiate_initialize(&request)?;
        self.greet(Some(&request.client_info), false).await;
        Ok(result)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tool_list_for(
            self.caller.seat().is_some(),
        )))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.caller.seat().is_some() && request.name != "player" {
            let p = amoris_contract::codes::permission_denied(&request.name, "player", "developer");
            return Ok(CallToolResult::error(vec![ContentBlock::text(problem_text(&p))]).into());
        }
        let args = request.arguments.unwrap_or_default();
        let (method, params) = match tools::route(&request.name, args) {
            Ok(r) => r,
            Err(why) => {
                let valid: Vec<&str> = tools::tools().iter().map(|t| t.name).collect();
                let p = Problem::new(
                    "request.unknown_method",
                    why,
                    amoris_contract::detail([("tools", json!(valid))]),
                );
                return Ok(
                    CallToolResult::error(vec![ContentBlock::text(problem_text(&p))]).into(),
                );
            }
        };
        // A client that sent no `initialize` (MCP 2026-07-28 names itself in each request's
        // `_meta`) is greeted by its first call; over HTTP such a handler serves this request
        // alone. Only the request's own `_meta` names it: on that path rmcp fills the peer's
        // handshake with its own name.
        self.greet(context.meta.client_info().as_ref(), self.http)
            .await;
        let result = match self.caller.call(method, params).await {
            // A text projection (an observation an LLM reads) goes as the text itself, not as a
            // JSON string full of escaped newlines (mcp.md 4.3).
            Ok(Value::String(text)) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string(&v).unwrap_or_default(),
            )]),
            Err(p) => CallToolResult::error(vec![ContentBlock::text(problem_text(&p))]),
        };
        Ok(result.into())
    }
}

/// Serves one MCP session over stdin and stdout until the client closes it.
pub async fn serve_stdio(backend: Arc<dyn Backend>) -> Result<(), Problem> {
    let (read, write) = rmcp::transport::stdio();
    serve_io(backend, read, write).await
}

/// Serves one MCP session over a byte stream pair (newline-delimited JSON-RPC, as over stdio)
/// until the client closes it: stdin and stdout for `amoris mcp`, an in-process pipe for tests.
pub async fn serve_io<R, W>(backend: Arc<dyn Backend>, read: R, write: W) -> Result<(), Problem>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let fail = |e: String| {
        Problem::new(
            "mcp.transport",
            format!("The MCP session over stdio failed: {e}"),
            amoris_contract::detail([]),
        )
    };
    let running = AmorisMcp::new(backend.open())
        .serve((read, write))
        .await
        .map_err(|e| fail(e.to_string()))?;
    running.waiting().await.map_err(|e| fail(e.to_string()))?;
    Ok(())
}

/// The Streamable HTTP service of [`http_service`], named so that a host can keep several (a
/// locked host keeps one per grant, docs/spec/server.md 3.6) and drive each from a handler of
/// its own (`handle`).
pub type HttpService = StreamableHttpService<AmorisMcp, LocalSessionManager>;

/// The Streamable HTTP service a host mounts at `/mcp`: one session per MCP client, loopback hosts
/// only (rmcp's default `allowed_hosts`).
pub fn http_service(backend: Arc<dyn Backend>) -> HttpService {
    let config = StreamableHttpServerConfig::default();
    StreamableHttpService::new(
        move || Ok(AmorisMcp::over_http(backend.open())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;

    /// What the callers of a backend were told and asked, in order.
    type Heard = Arc<Mutex<Vec<String>>>;

    struct Recorder(Heard);

    impl Caller for Recorder {
        fn hello(&self, hello: Hello) -> BoxFuture<()> {
            self.0.lock().unwrap().push(format!("hello {hello:?}"));
            Box::pin(std::future::ready(()))
        }

        fn call(&self, method: String, _params: Value) -> BoxFuture<Result<Value, Problem>> {
            self.0.lock().unwrap().push(format!("call {method}"));
            Box::pin(std::future::ready(Ok(json!({"ok": true}))))
        }
    }

    struct Recording(Heard);

    impl Backend for Recording {
        fn open(&self) -> Arc<dyn Caller> {
            Arc::new(Recorder(self.0.clone()))
        }
    }

    /// One client of a session served over an in-process pipe.
    struct Client {
        write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
        read: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    }

    impl Client {
        fn connect(heard: &Heard) -> (Client, tokio::task::JoinHandle<Result<(), Problem>>) {
            let backend: Arc<dyn Backend> = Arc::new(Recording(heard.clone()));
            let (client, server) = tokio::io::duplex(1 << 16);
            let (sr, sw) = tokio::io::split(server);
            let session = tokio::spawn(serve_io(backend, sr, sw));
            let (cr, cw) = tokio::io::split(client);
            (
                Client {
                    write: cw,
                    read: BufReader::new(cr),
                },
                session,
            )
        }

        async fn send(&mut self, id: u64, method: &str, params: Value) -> Value {
            let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            self.write
                .write_all(format!("{msg}\n").as_bytes())
                .await
                .unwrap();
            loop {
                let mut line = String::new();
                assert!(
                    self.read.read_line(&mut line).await.unwrap() > 0,
                    "the session closed before it answered {method}"
                );
                let r: Value = serde_json::from_str(&line).unwrap();
                if r["id"] == json!(id) {
                    return r;
                }
            }
        }
    }

    fn heard(h: &Heard) -> Vec<String> {
        h.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn initialize_says_hello_once_before_any_call() {
        let h = Heard::default();
        let (mut c, session) = Client::connect(&h);
        let init = c
            .send(
                1,
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "test", "version": "0"}}),
            )
            .await;
        // The handshake is rmcp's own: the instructions and the version the client asked for.
        let result = &init["result"];
        assert!(
            result["instructions"]
                .as_str()
                .is_some_and(|i| i.starts_with("Amoris host")),
            "{init:#}"
        );
        assert_eq!(result["protocolVersion"], "2025-06-18", "{init:#}");
        assert_eq!(result["serverInfo"]["name"], "amoris", "{init:#}");
        let hello = Hello {
            label: "test 0".to_owned(),
            transport: Transport::Stdio,
            per_request: false,
        };
        assert_eq!(heard(&h), [format!("hello {hello:?}")]);
        c.write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .unwrap();
        let tools = c.send(2, "tools/list", json!({})).await;
        assert!(tools["result"]["tools"].is_array(), "{tools:#}");
        assert_eq!(heard(&h).len(), 1, "listing the tools says nothing");
        for id in [3, 4] {
            let r = c
                .send(
                    id,
                    "tools/call",
                    json!({"name": "world", "arguments": {"action": "tree"}}),
                )
                .await;
            assert_eq!(r["result"]["isError"], json!(false), "{r:#}");
        }
        assert_eq!(
            heard(&h),
            [
                format!("hello {hello:?}"),
                "call world.tree".to_owned(),
                "call world.tree".to_owned()
            ]
        );
        drop(c);
        session.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_client_that_sends_no_initialize_is_greeted_by_its_first_call() {
        let h = Heard::default();
        let (mut c, session) = Client::connect(&h);
        // MCP 2026-07-28: no handshake; every request names its client in `_meta`.
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": " inline ", "version": "7"},
        });
        let r = c
            .send(
                1,
                "tools/call",
                json!({"name": "world", "arguments": {"action": "tree"}, "_meta": meta}),
            )
            .await;
        assert_eq!(r["result"]["isError"], json!(false), "{r:#}");
        let hello = Hello {
            label: "inline 7".to_owned(),
            transport: Transport::Stdio,
            per_request: false,
        };
        assert_eq!(
            heard(&h),
            [format!("hello {hello:?}"), "call world.tree".to_owned()]
        );
        let r = c
            .send(
                2,
                "tools/call",
                json!({"name": "world", "arguments": {"action": "tree"}, "_meta": meta}),
            )
            .await;
        assert_eq!(r["result"]["isError"], json!(false), "{r:#}");
        assert_eq!(heard(&h).len(), 3, "one hello for the stream");
        drop(c);
        session.await.unwrap().unwrap();
    }

    #[test]
    fn a_caller_nobody_called_through_is_told_nothing() {
        let h = Heard::default();
        let backend = Recording(h.clone());
        // rmcp asks for handlers it then only reads a tool's schema or the server's info from.
        let handler = AmorisMcp::over_http(backend.open());
        let _ = handler.get_tool("world");
        assert!(handler.get_info().instructions.is_some());
        drop(handler);
        assert!(heard(&h).is_empty());
    }

    #[test]
    fn a_label_is_the_clients_name_and_version() {
        assert_eq!(
            label_of(&Implementation::new("claude-code", "2.1.0")),
            "claude-code 2.1.0"
        );
        assert_eq!(label_of(&Implementation::new("probe", "")), "probe");
        assert_eq!(label_of(&Implementation::new("", "")), "");
    }
}
