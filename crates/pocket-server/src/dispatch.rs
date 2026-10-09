//! One catalog call (docs/spec/server.md, dispatch): a method answered on the server side
//! (`local`), or sent to the game thread through the caller's client and awaited without blocking
//! the server's threads. Edits, undo, redo, restores and Play push the history to editors; an MCP
//! session's calls are pushed as `agent` events.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pocket_contract::{Problem, detail};
use pocket_link::{GameClient, Reply, WorldMode, game_stopped};
use serde_json::{Value, json};

use crate::{DebugHub, Host, paused};

/// How often a call waiting on the game looks for a stop of the script debugger.
const PAUSE_POLL: Duration = Duration::from_millis(5);

/// While a `play.stop` waits on a game the debugger holds: the debugger passes over every pause
/// (`DebugHub::pass`) until this drops, also when the caller goes away first.
struct Passing(Arc<dyn DebugHub>);

impl Drop for Passing {
    fn drop(&mut self) {
        self.0.pass(false);
    }
}

/// Whose client a call goes through.
#[derive(Clone)]
pub enum Via {
    /// The editor's (`Source::Editor`).
    Editor,
    /// The HTTP API's shared developer client (CLI calls, tests).
    Api,
    /// An MCP session's own developer client, and the session's name for `agent` events.
    Session(Arc<Mutex<GameClient>>, String),
    /// A seat's player client (`Source::Player`) and the seat (docs/spec/player.md): calls made
    /// as that seat, which send the player tools alone ([`seat_permits`]).
    Player(Arc<Mutex<GameClient>>, String),
}

/// Whether a call made as `seat`'s player may name `method`: the player tools alone. Every other
/// method is a developer's, whether the game answers it (and refuses it too, pocket-runtime's
/// `player::permitted`) or the server does (`events.since` lists every world event, a read
/// while the debugger holds the game answers from the last publication): refused with
/// `permission.denied` before anything runs.
pub fn seat_permits(method: &str) -> Result<(), Problem> {
    if method.starts_with("player.") {
        Ok(())
    } else {
        Err(pocket_contract::codes::permission_denied(
            method,
            "player",
            "developer",
        ))
    }
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
            Via::Session(c, _) | Via::Player(c, _) => c.clone(),
        }
    }

    /// Sends a command to the game thread and awaits its reply, never behind the script
    /// debugger (server.md 3.4). A game held at a breakpoint refuses the call at once with
    /// `debug.paused {queued: false}`, with two exceptions: `play.stop` in Play crosses the hold
    /// (Stop drops the fork, held tick and all, so the debugger passes over every pause until the
    /// Stop lands), and `time.control` is queued for the boundary after the held tick and answered
    /// `debug.paused {queued: true}` at once (how real time stops at a breakpoint the next tick
    /// hits again). A stop that comes while the call waits: the `time.step` the stopped tick
    /// belongs to has been answered at the stop by the game thread (`{tick, stopped_by, paused:
    /// true}`, pocket-link's `StateHandle::stopped`); any other call answers `debug.paused
    /// {queued: true}` (it stays queued and runs when the game goes on).
    pub(crate) async fn game(
        &self,
        via: &Via,
        method: &str,
        params: Value,
    ) -> Result<Value, Problem> {
        let reader = &self.0.reader;
        let hub = (method == "play.stop" && reader.world().mode == WorldMode::Play)
            .then(|| self.0.debug.read().ok().and_then(|h| h.clone()))
            .flatten();
        let held = reader.debug_stop().filter(|_| hub.is_none());
        let queue = matches!(method, "time.control" | "time_control");
        if let Some(stop) = &held
            && !queue
        {
            return Err(paused::refusal(method, stop, false));
        }
        let stops = reader.debug_stops();
        let (tx, mut rx) = tokio::sync::oneshot::channel::<Reply>();
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
        if let Some(stop) = held {
            return Err(paused::refusal(method, &stop, true));
        }
        let answer = |r: Option<Reply>| match r {
            Some(r) => r.map(pocket_link::ReplyValue::into_json),
            None => Err(reader
                .stop_reason()
                .unwrap_or_else(|| game_stopped("the game ended before it answered", None))),
        };
        let mut passing: Option<Passing> = None;
        let mut poll = tokio::time::interval(PAUSE_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                r = &mut rx => return answer(r.ok()),
                _ = poll.tick() => {
                    let Some(stop) = reader.debug_stop() else {
                        continue;
                    };
                    if let Some(hub) = &hub {
                        if passing.is_none() {
                            hub.pass(true);
                            passing = Some(Passing(hub.clone()));
                        } else {
                            // A pause that began before the debugger heard of the pass.
                            let _ = hub.call("debug.continue", json!({})).await;
                        }
                        continue;
                    }
                    if reader.debug_stops() == stops {
                        continue;
                    }
                    if let Ok(r) = rx.try_recv() {
                        return answer(Some(r));
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
        let session = match via {
            Via::Session(_, name) => Some(name.clone()),
            Via::Player(_, seat) => Some(format!("player:{seat}")),
            _ => None,
        };
        if let Some(session) = &session {
            self.push(
                "agent",
                json!({"session": session, "kind": "call", "method": method,
                       "summary": summary(&params)}),
            );
        }
        let permitted = match via {
            Via::Player(..) => seat_permits(method),
            _ => Ok(()),
        };
        let r = match permitted {
            Err(p) => Err(p),
            Ok(()) => self.respond(via, method, params).await,
        };
        if let Some(session) = &session {
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

    /// A call answered here or by the game, and the history pushed after one that changed it.
    async fn respond(&self, via: &Via, method: &str, params: Value) -> Result<Value, Problem> {
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
        r
    }
}
