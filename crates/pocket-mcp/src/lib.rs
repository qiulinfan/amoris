//! The MCP server (docs/spec/host-protocol.md 7; docs/spec/server.md, MCP; architecture.md 4.12): a
//! projection of the command catalog into a few grouped tools ([`tools`]), served over stdio
//! (`pocket mcp <project>`) and Streamable HTTP (`POST /mcp` inside `pocket-server`).
//!
//! The crate holds no game: every tool call becomes one catalog call through a [`Caller`], which
//! the host gives each MCP session ([`Backend::open`]); the host decides which game client the
//! session uses and pushes each call to editors as an `agent` event. Errors come back as tool
//! results flagged `isError` whose text is the problem `{code, message, detail}`, so the agent
//! reads the code and the "did you mean" suggestions.

pub mod tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pocket_contract::Problem;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};

/// A boxed future the host's callers return.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// One MCP session's way to call the catalog.
pub trait Caller: Send + Sync {
    /// Calls a catalog method with JSON parameters.
    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>>;

    /// The seat a player session plays (docs/spec/player.md): it lists the `player` tool alone,
    /// and the runtime restricts its calls to the seat's own perception and actions.
    fn seat(&self) -> Option<String> {
        None
    }
}

/// The host side: a caller for each new MCP session.
pub trait Backend: Send + Sync {
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
pub struct PocketMcp {
    caller: Arc<dyn Caller>,
}

impl PocketMcp {
    pub fn new(caller: Arc<dyn Caller>) -> PocketMcp {
        PocketMcp { caller }
    }
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

impl ServerHandler for PocketMcp {
    fn get_info(&self) -> ServerConfig {
        let instructions = if self.caller.seat().is_some() {
            PLAYER_INSTRUCTIONS
        } else {
            INSTRUCTIONS
        };
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("pocket", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
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
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.caller.seat().is_some() && request.name != "player" {
            let p = pocket_contract::codes::permission_denied(&request.name, "player", "developer");
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
                    pocket_contract::detail([("tools", json!(valid))]),
                );
                return Ok(
                    CallToolResult::error(vec![ContentBlock::text(problem_text(&p))]).into(),
                );
            }
        };
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
/// until the client closes it: stdin and stdout for `pocket mcp`, an in-process pipe for tests.
pub async fn serve_io<R, W>(backend: Arc<dyn Backend>, read: R, write: W) -> Result<(), Problem>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let fail = |e: String| {
        Problem::new(
            "mcp.transport",
            format!("The MCP session over stdio failed: {e}"),
            pocket_contract::detail([]),
        )
    };
    let running = PocketMcp::new(backend.open())
        .serve((read, write))
        .await
        .map_err(|e| fail(e.to_string()))?;
    running.waiting().await.map_err(|e| fail(e.to_string()))?;
    Ok(())
}

/// The Streamable HTTP service a host mounts at `/mcp`: one session per MCP client, loopback hosts
/// only (rmcp's default `allowed_hosts`).
pub fn http_service(
    backend: Arc<dyn Backend>,
) -> StreamableHttpService<PocketMcp, LocalSessionManager> {
    let config = StreamableHttpServerConfig::default();
    StreamableHttpService::new(
        move || Ok(PocketMcp::new(backend.open())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}
