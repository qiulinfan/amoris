//! One catalog call (docs/spec/server.md, dispatch): a method answered on the server side
//! (`local`), or sent to the game thread through the caller's client and awaited without blocking
//! the server's threads. Edits, undo, redo, restores and Play push the history to editors; an MCP
//! session's calls are pushed as `agent` events.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pocket_contract::{Problem, detail};
use pocket_link::{GameClient, game_stopped};
use serde_json::{Value, json};

use crate::{Host, paused};

/// How often a call waiting on the game looks for a stop of the script debugger.
const PAUSE_POLL: Duration = Duration::from_millis(5);

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

    /// Sends a command to the game thread and awaits its reply, never behind the script
    /// debugger (server.md 3.4): a game held at a breakpoint refuses the call at once with
    /// `debug.paused`; a stop that comes while the call waits answers a `time.step` with where it
    /// stopped (`{tick, stopped_by, paused: true}`; the loop ends the step with that tick) and
    /// any other call with `debug.paused {queued: true}` (it runs when the game goes on).
    pub(crate) async fn game(
        &self,
        via: &Via,
        method: &str,
        params: Value,
    ) -> Result<Value, Problem> {
        let reader = &self.0.reader;
        if let Some(stop) = reader.debug_stop() {
            return Err(paused::refusal(method, &stop, false));
        }
        let stops = reader.debug_stops();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
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
        let mut poll = tokio::time::interval(PAUSE_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                r = &mut rx => {
                    return match r {
                        Ok(r) => r.map(pocket_link::ReplyValue::into_json),
                        Err(_) => Err(reader.stop_reason().unwrap_or_else(|| {
                            game_stopped("the game ended before it answered", None)
                        })),
                    };
                }
                _ = poll.tick() => {
                    if reader.debug_stops() == stops {
                        continue;
                    }
                    let Some(stop) = reader.debug_stop() else {
                        continue;
                    };
                    if matches!(method, "time.step" | "step") {
                        return Ok(json!({"tick": stop["tick"], "world_hash": null, "errors": [],
                                         "paused": true, "stopped_by": stop}));
                    }
                    return Err(paused::refusal(method, &stop, true));
                }
            }
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
            None => match self.paused_read(method, &params) {
                Some(r) => r,
                None => self.game(via, method, params).await,
            },
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
