//! The host's agent sessions (docs/spec/server.md 3.5): who calls the catalog as an agent, and the
//! stream of what they called. A session is an MCP client on `/mcp` or on stdio, a bridge that
//! registered itself (`agent.session.open`, as `amoris mcp` attached to this host does) or the
//! calls made as a seat with no session (`/api/call {seat}`). Each has a row, and every call and
//! its result is an event in a bounded ring, pushed on the `agent` topic and read back with
//! `agent.session.events`, so an editor that connects later shows what happened before it came.
//!
//! All of it is host state: the rows, the handles, the counters and the ring live in the server,
//! which holds no game. Nothing here is in the world, its hash, a snapshot or a recording, and a
//! restarted host starts at `mcp-1` with an empty stream. The four `agent.session.*` methods
//! never reach the game thread, so they answer while the debugger holds it.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use amoris_contract::{Candidate, CheckOptions, Pointer, Problem, codes, detail};
use amoris_link::GameClient;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::dispatch::summary;
use crate::{Host, Via, seat_permits};

/// The events the ring keeps: the newest this many.
pub(crate) const RING: usize = 2000;

/// The longest label a row keeps, in characters.
const LABEL_MAX: usize = 80;

/// `agent.session.events`'s `limit` when none is given.
const EVENTS_LIMIT: usize = 100;

/// A game client a session's calls go through.
type Client = Arc<Mutex<GameClient>>;

/// How a session reaches the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Transport {
    /// An MCP client on `POST /mcp`.
    Http,
    /// An MCP client on the stdin and stdout of a process that runs the game itself.
    Stdio,
    /// A process that registered with `agent.session.open` and calls `/api/call {handle}`.
    Bridge,
    /// Calls made as a seat with no session (`/api/call {seat}`).
    Api,
}

/// What a session may call: everything, or its seat's player tools alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Developer,
    Player,
}

/// A session as `agent.session.list` lists it. Every key is always present; `seat` and
/// `last_method` are null when there is none. Times are ms since the Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Row {
    /// `mcp-<n>`, or `player:<seat>` for the calls made as a seat with no session.
    pub id: String,
    pub transport: Transport,
    pub role: Role,
    pub seat: Option<String>,
    /// What the client calls itself (`clientInfo`'s name and version, a bridge's `label`).
    pub label: String,
    pub calls: u64,
    pub failed_calls: u64,
    pub last_method: Option<String>,
    /// A connection or a registration holds the session.
    pub live: bool,
    pub opened_ts: u64,
    pub last_ts: u64,
}

/// What a new session is.
#[derive(Clone, Debug)]
pub(crate) struct Spec {
    pub transport: Transport,
    /// The seat a player session plays; a developer's has none.
    pub seat: Option<String>,
    pub label: String,
    /// Whether something holds the session open (a connection, a registration). The rows of
    /// clients that keep no session are never live.
    pub live: bool,
    /// The row every request of a client that keeps no session shares, found again by its seat
    /// and label.
    pub shared: bool,
}

/// A session just opened, or found.
#[derive(Clone, Debug)]
pub(crate) struct Opened {
    pub id: String,
    pub row: Row,
    /// A bridge's address for its calls, unique to this host run.
    pub handle: Option<String>,
}

/// A call or its result, as the stream records it.
pub(crate) enum Step<'a> {
    Call {
        method: &'a str,
        summary: String,
    },
    Result {
        method: &'a str,
        ok: bool,
        summary: String,
    },
}

struct Entry {
    row: Row,
    /// A developer's own client, or a player's seat client once its first call found it.
    client: Option<Client>,
    handle: Option<String>,
    /// How many of the ring's events are this session's.
    in_ring: usize,
    /// The seat and label a client that keeps no session is found by.
    shared: Option<(Option<String>, String)>,
    /// Closed for good: its calls are refused. A row that was never live is not closed.
    closed: bool,
}

/// The sessions and their stream. It holds no host and reads no clock: the caller passes the
/// time, so tests make their own.
pub(crate) struct Registry {
    /// This host run, in handles: a bridge that outlives its host is not taken for a session of
    /// the next one.
    run: String,
    /// The last `n` of an `mcp-<n>`; never reused.
    numbered: u64,
    /// The last key of `entries`.
    opened: u64,
    /// The rows in opening order.
    entries: BTreeMap<u64, Entry>,
    ids: BTreeMap<String, u64>,
    ring: VecDeque<Value>,
    /// The last event's stream number.
    seq: u64,
}

fn cut(label: &str) -> String {
    label.trim().chars().take(LABEL_MAX).collect()
}

impl Registry {
    /// A registry of the host run that started at `run` (any number no other run shares).
    pub(crate) fn new(run: u128) -> Registry {
        Registry {
            run: format!("{run:x}"),
            numbered: 0,
            opened: 0,
            entries: BTreeMap::new(),
            ids: BTreeMap::new(),
            ring: VecDeque::new(),
            seq: 0,
        }
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.ids.get(id).and_then(|k| self.entries.get(k))
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut Entry> {
        let key = *self.ids.get(id)?;
        self.entries.get_mut(&key)
    }

    /// Opens a session and records it; the event to push. A seat's row with no session
    /// (`player:<seat>`) is opened only while none is listed ([`Host::seat_row`] looks first,
    /// under the registry's lock); every other id is new.
    pub(crate) fn open(&mut self, spec: Spec, client: Option<Client>, ts: u64) -> (Opened, Value) {
        let label = cut(&spec.label);
        let id = match (&spec.transport, &spec.seat) {
            (Transport::Api, Some(seat)) => format!("player:{seat}"),
            _ => {
                self.numbered += 1;
                format!("mcp-{}", self.numbered)
            }
        };
        let handle = (spec.transport == Transport::Bridge)
            .then(|| format!("{}-{}", self.run, self.numbered));
        let row = Row {
            id: id.clone(),
            transport: spec.transport,
            role: if spec.seat.is_some() {
                Role::Player
            } else {
                Role::Developer
            },
            seat: spec.seat.clone(),
            label: label.clone(),
            calls: 0,
            failed_calls: 0,
            last_method: None,
            live: spec.live,
            opened_ts: ts,
            last_ts: ts,
        };
        self.opened += 1;
        self.ids.insert(id.clone(), self.opened);
        self.entries.insert(
            self.opened,
            Entry {
                row: row.clone(),
                client,
                handle: handle.clone(),
                in_ring: 0,
                shared: spec.shared.then_some((spec.seat, label)),
                closed: false,
            },
        );
        let event = self.record(&id, "opened", json!({}), ts);
        (Opened { id, row, handle }, event)
    }

    /// The row the requests of a client that keeps no session share, if it is listed.
    pub(crate) fn shared(&self, seat: Option<&str>, label: &str) -> Option<Opened> {
        let label = cut(label);
        self.entries
            .values()
            .find(|e| {
                e.shared
                    .as_ref()
                    .is_some_and(|(s, l)| s.as_deref() == seat && *l == label)
            })
            .map(|e| Opened {
                id: e.row.id.clone(),
                row: e.row.clone(),
                handle: None,
            })
    }

    /// Records a call or its result under `session`, counted in its row when it is listed (an
    /// id with no row is still recorded); the event to push.
    pub(crate) fn step(&mut self, session: &str, step: Step<'_>, ts: u64) -> Value {
        match step {
            Step::Call { method, summary } => {
                if let Some(e) = self.entry_mut(session) {
                    e.row.calls += 1;
                    e.row.last_method = Some(method.to_owned());
                }
                self.record(
                    session,
                    "call",
                    json!({"method": method, "summary": summary}),
                    ts,
                )
            }
            Step::Result {
                method,
                ok,
                summary,
            } => {
                if !ok && let Some(e) = self.entry_mut(session) {
                    e.row.failed_calls += 1;
                }
                self.record(
                    session,
                    "result",
                    json!({"method": method, "ok": ok, "summary": summary}),
                    ts,
                )
            }
        }
    }

    /// Stamps an event, puts it in the ring and drops the rows the ring no longer speaks of.
    /// `opened` and `closed` carry the row as the event leaves it.
    fn record(&mut self, session: &str, kind: &str, mut event: Value, ts: u64) -> Value {
        self.seq += 1;
        event["seq"] = json!(self.seq);
        event["ts"] = json!(ts);
        event["session"] = json!(session);
        event["kind"] = json!(kind);
        if let Some(e) = self.entry_mut(session) {
            e.row.last_ts = ts;
            e.in_ring += 1;
            if matches!(kind, "opened" | "closed") {
                event["row"] = json!(e.row);
            }
        }
        self.ring.push_back(event.clone());
        while self.ring.len() > RING {
            let Some(old) = self.ring.pop_front() else {
                break;
            };
            let Some(key) = old["session"]
                .as_str()
                .and_then(|s| self.ids.get(s))
                .copied()
            else {
                continue;
            };
            let Some(e) = self.entries.get_mut(&key) else {
                continue;
            };
            e.in_ring = e.in_ring.saturating_sub(1);
            if !e.row.live && e.in_ring == 0 {
                // Nothing holds it and the stream no longer speaks of it.
                let id = e.row.id.clone();
                self.entries.remove(&key);
                self.ids.remove(&id);
            }
        }
        event
    }

    /// Closes a session: no longer live, its calls refused from now on, a developer's client
    /// dropped. The event to push, or none when it was closed already or is not listed.
    pub(crate) fn close(&mut self, id: &str, ts: u64) -> Option<Value> {
        let e = self.entry_mut(id)?;
        if e.closed {
            return None;
        }
        e.closed = true;
        e.row.live = false;
        e.client = None;
        Some(self.record(id, "closed", json!({}), ts))
    }

    pub(crate) fn row(&self, id: &str) -> Option<&Row> {
        self.entry(id).map(|e| &e.row)
    }

    /// The session a handle was given to, closed or not, while its row is listed.
    pub(crate) fn by_handle(&self, handle: &str) -> Option<&Row> {
        self.entries
            .values()
            .find(|e| e.handle.as_deref() == Some(handle))
            .map(|e| &e.row)
    }

    /// Who an open session calls as: its seat and the client it has; none when it is closed or
    /// not listed.
    fn caller(&self, id: &str) -> Option<(Option<String>, Option<Client>)> {
        let e = self.entry(id).filter(|e| !e.closed)?;
        Some((e.row.seat.clone(), e.client.clone()))
    }

    fn set_client(&mut self, id: &str, client: Client) {
        if let Some(e) = self.entry_mut(id).filter(|e| !e.closed) {
            e.client = Some(client);
        }
    }

    /// `agent.session.list`'s answer: the rows in opening order and the newest stream number.
    pub(crate) fn list(&self) -> Value {
        let rows: Vec<&Row> = self.entries.values().map(|e| &e.row).collect();
        json!({"sessions": rows, "last": self.seq})
    }

    /// `agent.session.events`'s answer. With `since` the first `limit` events after it, oldest
    /// first; without it the newest `limit`. `last` is the cursor the answer reaches: the last
    /// event returned when `limit` cut the answer, else the newest stream number, so a reader
    /// that asks again with `since: last` loses nothing. `missed` counts the events after
    /// `since` that left the ring, whatever their session.
    pub(crate) fn events(
        &self,
        since: Option<u64>,
        limit: usize,
        session: Option<&str>,
    ) -> Result<Value, Problem> {
        if let Some(s) = session
            && self.entry(s).is_none()
        {
            let listed: Vec<Candidate<'_>> = self
                .entries
                .values()
                .map(|e| Candidate::new(&e.row.id))
                .collect();
            return Err(codes::invalid_value(
                &Pointer::root().key("session"),
                s,
                &listed,
            ));
        }
        let limit = limit.clamp(1, RING);
        let wanted = |e: &&Value| session.is_none_or(|s| e["session"].as_str() == Some(s));
        let oldest = self.seq + 1 - self.ring.len() as u64;
        let (events, last, missed): (Vec<&Value>, u64, u64) = match since {
            Some(since) => {
                let mut after = self
                    .ring
                    .iter()
                    .filter(|e| e["seq"].as_u64().is_some_and(|n| n > since))
                    .filter(wanted);
                let events: Vec<&Value> = after.by_ref().take(limit).collect();
                let last = match (after.next(), events.last()) {
                    (Some(_), Some(e)) => e["seq"].as_u64().unwrap_or(self.seq),
                    _ => self.seq,
                };
                (events, last, (oldest - 1).saturating_sub(since))
            }
            None => {
                let all: Vec<&Value> = self.ring.iter().filter(wanted).collect();
                let from = all.len().saturating_sub(limit);
                (all[from..].to_vec(), self.seq, 0)
            }
        };
        Ok(json!({"events": events, "last": last, "missed": missed}))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// `agent.session.events`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsParams {
    /// Events after this stream number, oldest first; without it, the newest.
    #[serde(default)]
    since: Option<u64>,
    /// At most this many, default 100 (the host keeps the newest 2,000).
    #[serde(default)]
    limit: Option<u32>,
    /// One session's events, by its id (`mcp-3`).
    #[serde(default)]
    session: Option<String>,
}

/// `agent.session.open`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenParams {
    /// What the session is called in the list: the agent program's name and version.
    #[serde(default)]
    label: Option<String>,
    /// The seat the session plays (docs/spec/player.md): its calls are that seat's player's, the
    /// player tools alone. Without it the session is a developer's.
    #[serde(default)]
    seat: Option<String>,
}

/// `agent.session.close`'s parameters: one of the two.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloseParams {
    /// The handle `agent.session.open` answered: the opener closes its own session.
    #[serde(default)]
    handle: Option<String>,
    /// A bridge session's id (`mcp-3`), for a row whose process is gone.
    #[serde(default)]
    session: Option<String>,
}

fn decode<T: DeserializeOwned + JsonSchema>(v: &Value, owner: &str) -> Result<T, Problem> {
    let empty = json!({});
    let v = if v.is_null() { &empty } else { v };
    amoris_contract::decode::<T>(v, &CheckOptions::new(owner)).map(|d| d.value)
}

/// `host.session_unknown`: no open session has this handle or id.
fn session_unknown(key: &str, value: &str) -> Problem {
    Problem::new(
        "host.session_unknown",
        format!(
            "This host has no open agent session with the {key} '{value}': it was closed, or the \
             host started again since it opened. agent.session.open registers one and \
             agent.session.list lists them."
        ),
        detail([(key, json!(value))]),
    )
}

/// Milliseconds since the Unix epoch, for the rows and the events: presentation only (the server
/// holds no game and nothing here reaches the world).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A new host's registry, its handles marked with the time it started.
pub(crate) fn registry() -> Mutex<Registry> {
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    Mutex::new(Registry::new(run))
}

impl Host {
    /// The registry. Its lock is never held across an await, and never taken while `names` is
    /// held; events are pushed while it is held, so they leave in the order of their numbers.
    fn sessions(&self) -> MutexGuard<'_, Registry> {
        self.0
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a session in a registry already locked: a developer's gets a client of its own,
    /// its source named after the session in the history; a player's seat is not looked up
    /// until its first call, so opening never asks the game.
    fn open_in(&self, sessions: &mut Registry, spec: Spec) -> Opened {
        let client = spec.seat.is_none().then(|| (self.0.developer)());
        let source = client.as_ref().map(GameClient::source);
        let client = client.map(|c| Arc::new(Mutex::new(c)));
        let (opened, event) = sessions.open(spec, client, now_ms());
        if let Some(source) = source
            && let Ok(mut names) = self.0.names.write()
        {
            names.insert(source, opened.id.clone());
        }
        self.push("agent", event);
        opened
    }

    /// Opens a session that a connection or a registration holds.
    pub(crate) fn session_open(&self, spec: Spec) -> Opened {
        let mut sessions = self.sessions();
        self.open_in(&mut sessions, spec)
    }

    /// The row the requests of an MCP client that keeps no session share (MCP 2026-07-28 over
    /// HTTP: every request comes alone), found by its seat and label or opened: one developer
    /// client for all of them, and never live.
    pub(crate) fn session_shared(&self, seat: Option<&str>, label: &str) -> Opened {
        let mut sessions = self.sessions();
        if let Some(found) = sessions.shared(seat, label) {
            return found;
        }
        let spec = Spec {
            transport: Transport::Http,
            seat: seat.map(str::to_owned),
            label: label.to_owned(),
            live: false,
            shared: true,
        };
        self.open_in(&mut sessions, spec)
    }

    /// The row of the calls made as `seat` with no session (`/api/call {seat}`), opened on the
    /// first: `player:<seat>`, never live.
    pub(crate) fn seat_row(&self, seat: &str) -> String {
        let id = format!("player:{seat}");
        let mut sessions = self.sessions();
        if sessions.row(&id).is_none() {
            let spec = Spec {
                transport: Transport::Api,
                seat: Some(seat.to_owned()),
                label: String::new(),
                live: false,
                shared: false,
            };
            self.open_in(&mut sessions, spec);
        }
        id
    }

    /// Closes a session; the row as it is then, when it is listed.
    pub(crate) fn session_close(&self, id: &str) -> Option<Row> {
        let mut sessions = self.sessions();
        if let Some(event) = sessions.close(id, now_ms()) {
            self.push("agent", event);
        }
        sessions.row(id).cloned()
    }

    /// Records a session's call or its result and pushes it as an `agent` event.
    pub(crate) fn agent_event(&self, session: &str, step: Step<'_>) {
        let mut sessions = self.sessions();
        let event = sessions.step(session, step, now_ms());
        self.push("agent", event);
    }

    /// A call refused before it has a client to go through: recorded as a call and its failed
    /// result, as [`Host::call`] records the others.
    fn agent_refused(&self, id: &str, method: &str, params: &Value, problem: Problem) -> Problem {
        self.agent_event(
            id,
            Step::Call {
                method,
                summary: summary(params),
            },
        );
        self.agent_event(
            id,
            Step::Result {
                method,
                ok: false,
                summary: format!("{}: {}", problem.code, problem.message),
            },
        );
        problem
    }

    /// One call of the session `id`. A developer's goes through its own client. A player's is
    /// first checked against the player tools ([`seat_permits`]), before the seat is looked
    /// up; the seat's client is found on the first permitted call and kept.
    pub(crate) async fn session_call(
        &self,
        id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, Problem> {
        let caller = self.sessions().caller(id);
        let Some((seat, client)) = caller else {
            return Err(session_unknown("session", id));
        };
        let Some(seat) = seat else {
            let Some(client) = client else {
                return Err(session_unknown("session", id));
            };
            return self
                .call(&Via::Session(client, id.to_owned()), method, params)
                .await;
        };
        if let Err(p) = seat_permits(method) {
            return Err(self.agent_refused(id, method, &params, p));
        }
        let client = match client {
            Some(c) => c,
            None => match self.player_client(&seat).await {
                Ok(c) => {
                    self.sessions().set_client(id, c.clone());
                    c
                }
                Err(p) => return Err(self.agent_refused(id, method, &params, p)),
            },
        };
        self.call(&Via::Player(client, id.to_owned()), method, params)
            .await
    }

    /// One call of the bridge session `handle` was given to (`/api/call {handle}`).
    pub(crate) async fn call_as(
        &self,
        handle: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, Problem> {
        let id = self
            .sessions()
            .by_handle(handle)
            .filter(|row| row.live)
            .map(|row| row.id.clone());
        match id {
            Some(id) => self.session_call(&id, method, params).await,
            None => Err(session_unknown("handle", handle)),
        }
    }

    /// Answers `agent.session.list`, `.events`, `.open` and `.close`: the server's own, never
    /// the game's.
    pub(crate) fn agent_session(&self, method: &str, params: &Value) -> Result<Value, Problem> {
        match method {
            "agent.session.list" => {
                decode::<NoParams>(params, method).map(|_| self.sessions().list())
            }
            "agent.session.events" => {
                let p: EventsParams = decode(params, method)?;
                let limit = p
                    .limit
                    .map_or(EVENTS_LIMIT, |n| usize::try_from(n).unwrap_or(RING));
                self.sessions().events(p.since, limit, p.session.as_deref())
            }
            "agent.session.open" => {
                let p: OpenParams = decode(params, method)?;
                let opened = self.session_open(Spec {
                    transport: Transport::Bridge,
                    seat: p.seat,
                    label: p.label.unwrap_or_default(),
                    live: true,
                    shared: false,
                });
                let mut row = json!(opened.row);
                row["handle"] = json!(opened.handle);
                Ok(row)
            }
            _ => {
                let p: CloseParams = decode(params, method)?;
                let id = self.closing(method, &p)?;
                self.session_close(&id)
                    .map(|row| json!(row))
                    .ok_or_else(|| session_unknown("session", &id))
            }
        }
    }

    /// The session `agent.session.close` names: by the handle its opener holds, or a bridge row
    /// by its id.
    fn closing(&self, method: &str, p: &CloseParams) -> Result<String, Problem> {
        let root = Pointer::root();
        let sessions = self.sessions();
        match (&p.handle, &p.session) {
            (Some(_), Some(_)) => Err(codes::conflict(&[root.key("handle"), root.key("session")])),
            (None, None) => Err(codes::missing_field(
                &root.key("handle"),
                method,
                "the handle agent.session.open answered, or 'session': a bridge session's id",
            )),
            (Some(handle), None) => sessions
                .by_handle(handle)
                .map(|row| row.id.clone())
                .ok_or_else(|| session_unknown("handle", handle)),
            (None, Some(id)) => match sessions.row(id) {
                None => Err(session_unknown("session", id)),
                Some(row) if row.transport != Transport::Bridge => Err(codes::not_applicable(
                    &root.key("session"),
                    "only a bridge session is closed by its id; an MCP session ends with its \
                     connection, and a seat's calls with no session hold nothing open",
                )),
                Some(row) => Ok(row.id.clone()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(transport: Transport) -> Spec {
        Spec {
            transport,
            seat: None,
            label: "test 1".to_owned(),
            live: true,
            shared: false,
        }
    }

    fn call(r: &mut Registry, id: &str, method: &str, ok: bool, ts: u64) {
        r.step(
            id,
            Step::Call {
                method,
                summary: "{}".to_owned(),
            },
            ts,
        );
        r.step(
            id,
            Step::Result {
                method,
                ok,
                summary: String::new(),
            },
            ts + 1,
        );
    }

    fn seqs(answer: &Value) -> Vec<u64> {
        answer["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn rows_are_numbered_in_opening_order_and_count_their_calls() {
        let mut r = Registry::new(7);
        assert_eq!(r.list(), json!({"sessions": [], "last": 0}));
        let (a, opened) = r.open(spec(Transport::Http), None, 100);
        let (b, _) = r.open(spec(Transport::Stdio), None, 101);
        assert_eq!((a.id.as_str(), b.id.as_str()), ("mcp-1", "mcp-2"));
        assert_eq!(opened["kind"], "opened");
        assert_eq!(opened["row"]["id"], "mcp-1");
        assert_eq!(
            (opened["seq"].as_u64(), opened["ts"].as_u64()),
            (Some(1), Some(100))
        );
        call(&mut r, "mcp-1", "world.tree", true, 110);
        call(&mut r, "mcp-1", "world.get", false, 120);
        let listed = r.list();
        assert_eq!(listed["last"], 6);
        let row = &listed["sessions"][0];
        assert_eq!(
            row,
            &json!({"id": "mcp-1", "transport": "http", "role": "developer", "seat": null,
                    "label": "test 1", "calls": 2, "failed_calls": 1,
                    "last_method": "world.get", "live": true, "opened_ts": 100,
                    "last_ts": 121})
        );
        assert_eq!(listed["sessions"][1]["id"], "mcp-2");
        assert_eq!(listed["sessions"][1]["last_method"], Value::Null);
        // A closed session's number is not given again.
        assert!(r.close("mcp-2", 130).is_some());
        let (c, _) = r.open(spec(Transport::Http), None, 131);
        assert_eq!(c.id, "mcp-3");
        // A seat's calls with no session are named after the seat, and a player's row has one.
        let (s, _) = r.open(
            Spec {
                transport: Transport::Api,
                seat: Some("skipper".to_owned()),
                label: String::new(),
                live: false,
                shared: false,
            },
            None,
            140,
        );
        assert_eq!(s.id, "player:skipper");
        assert_eq!(
            (s.row.role, s.row.seat.as_deref()),
            (Role::Player, Some("skipper"))
        );
        // An event of an id with no row is still in the stream.
        call(&mut r, "ghost", "status", true, 150);
        let e = r.events(None, 1, None).unwrap();
        assert_eq!(e["events"][0]["session"], "ghost");
    }

    #[test]
    fn the_ring_keeps_the_newest_events_and_says_what_it_missed() {
        let mut r = Registry::new(7);
        r.open(spec(Transport::Http), None, 0);
        for i in 0..(RING as u64 + 9) {
            r.step(
                "mcp-1",
                Step::Call {
                    method: "status",
                    summary: i.to_string(),
                },
                i,
            );
        }
        // 2,010 events: the first 10 left the ring.
        let all = r.events(Some(0), RING, None).unwrap();
        assert_eq!(all["missed"], 10);
        assert_eq!(all["last"], RING as u64 + 10);
        let got = seqs(&all);
        assert_eq!(got.len(), RING);
        assert_eq!((got[0], got[RING - 1]), (11, RING as u64 + 10));
        // A limit above the ring is the ring.
        assert_eq!(
            seqs(&r.events(Some(0), usize::MAX, None).unwrap()).len(),
            RING
        );
        // A reader at the newest missed nothing; one ahead of the stream reads nothing.
        let none = r.events(Some(RING as u64 + 10), 100, None).unwrap();
        assert_eq!(
            none,
            json!({"events": [], "last": RING as u64 + 10, "missed": 0})
        );
        let ahead = r.events(Some(1_000_000), 100, None).unwrap();
        assert_eq!(ahead["events"], json!([]));
    }

    #[test]
    fn a_reader_pages_with_last_and_loses_nothing() {
        let mut r = Registry::new(7);
        r.open(spec(Transport::Http), None, 0);
        r.open(spec(Transport::Bridge), None, 0);
        for i in 0..5 {
            call(&mut r, "mcp-1", "world.get", true, i);
            call(&mut r, "mcp-2", "world.tree", true, i);
        }
        // 22 events. Without `since`: the newest.
        let newest = r.events(None, 3, None).unwrap();
        assert_eq!(seqs(&newest), [20, 21, 22]);
        assert_eq!(
            (newest["last"].as_u64(), newest["missed"].as_u64()),
            (Some(22), Some(0))
        );
        // With `since`: the first after it, and `last` where the answer stopped.
        let mut since = 0;
        let mut read = Vec::new();
        loop {
            let page = r.events(Some(since), 5, None).unwrap();
            let got = seqs(&page);
            let last = page["last"].as_u64().unwrap();
            if got.len() == 5 && last < 22 {
                assert_eq!(last, *got.last().unwrap());
            }
            read.extend(got);
            if last == since {
                break;
            }
            since = last;
        }
        assert_eq!(read, (1..=22).collect::<Vec<u64>>());
        // An answer the limit did not cut ends on the newest, also when a filter left events out.
        let one = r.events(Some(0), 100, Some("mcp-1")).unwrap();
        assert_eq!(seqs(&one).len(), 11);
        assert_eq!(one["last"], 22);
        assert!(
            one["events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["session"] == "mcp-1")
        );
        // A filtered answer the limit cut ends on its last event, and the next read goes on.
        let cut = r.events(Some(0), 4, Some("mcp-2")).unwrap();
        assert_eq!(seqs(&cut), [2, 5, 6, 9]);
        assert_eq!(cut["last"], 9);
        let rest = r.events(Some(9), 100, Some("mcp-2")).unwrap();
        assert_eq!(seqs(&rest), [10, 13, 14, 17, 18, 21, 22]);
        // A session that is not listed is refused with the ones that are.
        let e = r.events(None, 10, Some("nope")).unwrap_err();
        assert_eq!(e.code, "request.invalid_value");
        assert_eq!(e.detail["path"], "/session");
        assert_eq!(e.detail["allowed"], json!(["mcp-1", "mcp-2"]));
    }

    #[test]
    fn a_row_nothing_holds_stays_while_the_ring_speaks_of_it() {
        let mut r = Registry::new(7);
        r.open(spec(Transport::Http), None, 0);
        r.open(spec(Transport::Bridge), None, 0);
        call(&mut r, "mcp-1", "status", true, 1);
        assert!(r.close("mcp-1", 2).is_some());
        // Closed, it is listed while one of its events is in the ring.
        for i in 0..(RING as u64 - 1) {
            r.step(
                "mcp-2",
                Step::Call {
                    method: "status",
                    summary: String::new(),
                },
                i,
            );
        }
        // The ring now starts at mcp-1's `closed` (its fifth event of this stream).
        assert_eq!(
            r.events(Some(0), 1, None).unwrap()["events"][0]["kind"],
            "closed"
        );
        assert_eq!(r.row("mcp-1").map(|row| row.live), Some(false));
        r.step(
            "mcp-2",
            Step::Call {
                method: "status",
                summary: String::new(),
            },
            9,
        );
        // Its last event left: the row goes. The live row stays, whatever the ring holds.
        assert!(r.row("mcp-1").is_none());
        assert_eq!(r.list()["sessions"].as_array().unwrap().len(), 1);
        let mut quiet = Registry::new(7);
        quiet.open(spec(Transport::Http), None, 0);
        quiet.open(spec(Transport::Bridge), None, 0);
        for i in 0..(2 * RING as u64) {
            quiet.step(
                "mcp-2",
                Step::Call {
                    method: "status",
                    summary: String::new(),
                },
                i,
            );
        }
        assert_eq!(quiet.row("mcp-1").map(|row| row.live), Some(true));
    }

    #[test]
    fn a_client_without_a_session_finds_its_row_again() {
        let mut r = Registry::new(7);
        let shared = |seat: Option<&str>, label: &str| Spec {
            transport: Transport::Http,
            seat: seat.map(str::to_owned),
            label: label.to_owned(),
            live: false,
            shared: true,
        };
        assert!(r.shared(None, "stateless 2").is_none());
        let (first, _) = r.open(shared(None, "stateless 2"), None, 0);
        assert!(!first.row.live);
        assert_eq!(
            r.shared(None, " stateless 2 ").map(|o| o.id),
            Some(first.id.clone())
        );
        // Another label, or the same label playing a seat, is another row.
        assert!(r.shared(None, "stateless 3").is_none());
        assert!(r.shared(Some("skipper"), "stateless 2").is_none());
        let (seat, _) = r.open(shared(Some("skipper"), "stateless 2"), None, 0);
        assert_ne!(seat.id, first.id);
        assert_eq!(
            r.shared(Some("skipper"), "stateless 2").map(|o| o.id),
            Some(seat.id)
        );
        // A session of its own with the same label is not the shared row.
        r.open(spec(Transport::Http), None, 0);
        assert_eq!(r.shared(None, "test 1").map(|o| o.id), None);
        // Labels are cut to 80 characters, at a character's boundary.
        let long = "é".repeat(200);
        let (cut, _) = r.open(shared(None, &long), None, 0);
        assert_eq!(cut.row.label.chars().count(), 80);
        assert_eq!(r.shared(None, &long).map(|o| o.id), Some(cut.id));
    }

    #[test]
    fn handles_belong_to_one_host_run_and_close_twice() {
        let mut a = Registry::new(1_000);
        let mut b = Registry::new(2_000);
        let (ha, _) = a.open(spec(Transport::Bridge), None, 0);
        let (hb, _) = b.open(spec(Transport::Bridge), None, 0);
        assert_eq!(ha.id, hb.id);
        let (ha, hb) = (ha.handle.unwrap(), hb.handle.unwrap());
        assert_ne!(ha, hb);
        assert!(b.by_handle(&ha).is_none());
        // Only a bridge has a handle.
        let (http, _) = a.open(spec(Transport::Http), None, 0);
        assert!(http.handle.is_none());
        // Closing answers the event once; the handle still finds the row, which is not live.
        let closed = a.close("mcp-1", 5).unwrap();
        assert_eq!(
            (closed["kind"].as_str(), closed["row"]["live"].as_bool()),
            (Some("closed"), Some(false))
        );
        assert!(a.close("mcp-1", 6).is_none());
        assert_eq!(a.by_handle(&ha).map(|row| row.live), Some(false));
        assert!(a.caller("mcp-1").is_none());
        assert!(a.caller("mcp-2").is_some());
        assert!(a.close("nope", 7).is_none());
    }
}
