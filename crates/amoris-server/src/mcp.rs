//! MCP sessions on this host (docs/spec/server.md 3.5 and 7): a client is an agent session from
//! the moment it says who it is (`amoris-mcp`'s `Caller::hello`: its `initialize`, or the first
//! call of a client that sent none), with its own developer client; its calls go through
//! [`Host::call`] like any other, recorded and pushed to editors as `agent` events, and the
//! session closes when rmcp drops its handler. A session bound to a seat (`amoris mcp --seat`,
//! docs/spec/player.md) calls as that seat's player instead, and lists the player tool alone.

use std::sync::Arc;

use amoris_contract::Problem;
use amoris_mcp::{Backend, BoxFuture, Caller, Hello};
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::Host;
use crate::sessions::{Spec, Transport};

/// The host as `amoris-mcp` reaches it.
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

/// One MCP handler's caller. rmcp makes handlers it never calls through (`Backend::open`), so
/// the session opens at the hello, or at the first call of a caller driven without one.
struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    host: Host,
    seat: Option<String>,
    opened: OnceCell<Opened>,
}

/// The session a caller's calls are listed under.
struct Opened {
    id: String,
    /// This caller opened the session and closes it; the row the requests of a client that
    /// keeps no session share is nobody's to close.
    owned: bool,
}

impl Inner {
    fn open(&self, hello: Option<Hello>) -> Opened {
        match hello {
            Some(h) if h.per_request => Opened {
                id: self.host.session_shared(self.seat.as_deref(), &h.label).id,
                owned: false,
            },
            hello => {
                let (transport, label) = match hello {
                    Some(h) => (
                        match h.transport {
                            amoris_mcp::Transport::Http => Transport::Http,
                            amoris_mcp::Transport::Stdio => Transport::Stdio,
                        },
                        h.label,
                    ),
                    // A caller driven without the MCP layer (tests, an embedding host).
                    None => (Transport::Stdio, String::new()),
                };
                let spec = Spec {
                    transport,
                    seat: self.seat.clone(),
                    label,
                    live: true,
                    shared: false,
                };
                Opened {
                    id: self.host.session_open(spec).id,
                    owned: true,
                }
            }
        }
    }
}

impl Drop for Inner {
    /// rmcp dropped the handler (the client closed its session or its stream, an HTTP session
    /// idled out, the host ends) and every call in flight has answered: each holds this.
    fn drop(&mut self) {
        if let Some(opened) = self.opened.get()
            && opened.owned
        {
            self.host.session_close(&opened.id);
        }
    }
}

impl Caller for Session {
    fn hello(&self, hello: Hello) -> BoxFuture<()> {
        let inner = self.inner.clone();
        Box::pin(async move {
            inner
                .opened
                .get_or_init(|| async { inner.open(Some(hello)) })
                .await;
        })
    }

    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let id = inner
                .opened
                .get_or_init(|| async { inner.open(None) })
                .await
                .id
                .clone();
            inner.host.session_call(&id, &method, params).await
        })
    }

    fn seat(&self) -> Option<String> {
        self.inner.seat.clone()
    }
}

impl Backend for McpBackend {
    fn open(&self) -> Arc<dyn Caller> {
        Arc::new(Session {
            inner: Arc::new(Inner {
                host: self.host.clone(),
                seat: self.seat.clone(),
                opened: OnceCell::new(),
            }),
        })
    }
}
