//! MCP sessions on this host (docs/spec/server.md, MCP): each session opens its own developer
//! client, and its calls go through [`Host::call`] like any other, pushed to editors as `agent`
//! events.

use std::sync::Arc;

use pocket_contract::Problem;
use pocket_mcp::{Backend, BoxFuture, Caller};
use serde_json::Value;

use crate::{Host, Via};

/// The host as `pocket-mcp` reaches it.
pub struct McpBackend {
    host: Host,
}

impl McpBackend {
    pub fn new(host: Host) -> McpBackend {
        McpBackend { host }
    }
}

struct Session {
    host: Host,
    via: Via,
}

impl Caller for Session {
    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let (host, via) = (self.host.clone(), self.via.clone());
        Box::pin(async move { host.call(&via, &method, params).await })
    }
}

impl Backend for McpBackend {
    fn open(&self) -> Arc<dyn Caller> {
        Arc::new(Session {
            host: self.host.clone(),
            via: self.host.open_session(),
        })
    }
}
