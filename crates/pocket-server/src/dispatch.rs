//! One catalog call (docs/spec/server.md, dispatch): a method answered on the server side
//! (`local`), or sent to the game thread through the caller's client and awaited without blocking
//! the server's threads. Edits, undo, redo, restores and Play push the history to editors; an MCP
//! session's calls are pushed as `agent` events.

use std::sync::{Arc, Mutex};

use pocket_contract::{Problem, detail};
use pocket_link::{GameClient, game_stopped};
use serde_json::{Value, json};

use crate::Host;

/// Whose client a call goes through.
#[derive(Clone)]
pub enum Via {
    /// The editor's (`Source::Editor`).
    Editor,
    /// The HTTP API's shared developer client (CLI calls, tests).
    Api,
    /// An MCP session's own developer client, and the session's name for `agent` events.
    Session(Arc<Mutex<GameClient>>, String),
}

/// Methods after which the history may have changed.
const HISTORY: &[&str] = &[
    "world.edit",
    "world_edit",
    "history.undo",
    "history.redo",
    "snapshots.restore",
    "play.start",
    "play.stop",
];

/// A short text of a value for `agent` events.
fn summary(v: &Value) -> String {
    let mut s = v.to_string();
    if s.len() > 160 {
        let mut cut = 157;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("...");
    }
    s
}

impl Host {
    fn client(&self, via: &Via) -> Arc<Mutex<GameClient>> {
        match via {
            Via::Editor => self.0.editor.clone(),
            Via::Api => self.0.api.clone(),
            Via::Session(c, _) => c.clone(),
        }
    }

    /// Sends a command to the game thread and awaits its reply.
    pub(crate) async fn game(
        &self,
        via: &Via,
        method: &str,
        params: Value,
    ) -> Result<Value, Problem> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let client = self.client(via);
            let mut c = client.lock().map_err(|_| {
                Problem::new(
                    "internal.error",
                    "A game client's lock is poisoned.",
                    detail([]),
                )
            })?;
            c.send(method, params, None, move |r| {
                let _ = tx.send(r);
            })?;
        }
        match rx.await {
            Ok(r) => r.map(pocket_link::ReplyValue::into_json),
            Err(_) => Err(self
                .0
                .reader
                .stop_reason()
                .unwrap_or_else(|| game_stopped("the game ended before it answered", None))),
        }
    }

    /// A new MCP session's client.
    pub(crate) fn open_session(&self) -> Via {
        let n = self
            .0
            .sessions
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        Via::Session(
            Arc::new(Mutex::new((self.0.developer)())),
            format!("mcp-{n}"),
        )
    }

    /// One catalog call: answered here or by the game, with the pushes it causes.
    pub async fn call(&self, via: &Via, method: &str, params: Value) -> Result<Value, Problem> {
        if let Via::Session(_, session) = via {
            self.push(
                "agent",
                json!({"session": session, "kind": "call", "method": method,
                       "summary": summary(&params)}),
            );
        }
        let r = match self.local(via, method, params.clone()).await {
            Some(r) => r,
            None => self.game(via, method, params).await,
        };
        if r.is_ok()
            && HISTORY.contains(&method)
            && let Ok(h) = self.game(&Via::Api, "history.list", json!({})).await
        {
            self.push("history", h);
        }
        if let Via::Session(_, session) = via {
            let (ok, text) = match &r {
                Ok(v) => (true, summary(v)),
                Err(p) => (false, format!("{}: {}", p.code, p.message)),
            };
            self.push(
                "agent",
                json!({"session": session, "kind": "result", "method": method, "ok": ok,
                       "summary": text}),
            );
        }
        r
    }
}
