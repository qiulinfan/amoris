//! MCP sessions on this host (docs/spec/server.md, MCP): each session opens its own developer
//! client, and its calls go through [`Host::call`] like any other, pushed to editors as `agent`
//! events. A session bound to a seat (`pocket mcp --seat`, docs/spec/player.md) calls as that
//! seat's player instead, and lists the player tool alone.

use std::sync::Arc;

use pocket_contract::Problem;
use pocket_mcp::{Backend, BoxFuture, Caller};
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::{Host, Via};

/// The host as `pocket-mcp` reaches it.
pub struct McpBackend {
    host: Host,
    seat: Option<String>,
}

impl McpBackend {
    pub fn new(host: Host) -> McpBackend {
        McpBackend { host, seat: None }
    }

    /// Sessions that play `seat`: their calls are the seat's player's.
    pub fn player(host: Host, seat: &str) -> McpBackend {
        McpBackend {
            host,
            seat: Some(seat.to_owned()),
        }
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

/// A session that plays a seat: its player client is found on the first call.
struct PlayerSession {
    host: Host,
    seat: String,
    via: Arc<OnceCell<Via>>,
}

impl Caller for PlayerSession {
    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let host = self.host.clone();
        let seat = self.seat.clone();
        let cell = self.via.clone();
        Box::pin(async move {
            let via = cell
                .get_or_try_init(|| async { host.player_via(&seat).await })
                .await?
                .clone();
            host.call(&via, &method, params).await
        })
    }

    fn seat(&self) -> Option<String> {
        Some(self.seat.clone())
    }
}

impl Backend for McpBackend {
    fn open(&self) -> Arc<dyn Caller> {
        match &self.seat {
            Some(seat) => Arc::new(PlayerSession {
                host: self.host.clone(),
                seat: seat.clone(),
                via: Arc::new(OnceCell::new()),
            }),
            None => Arc::new(Session {
                host: self.host.clone(),
                via: self.host.open_session(),
            }),
        }
    }
}
