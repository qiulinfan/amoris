//! The game role's loop in a Web Worker (docs/spec/threads.md 7.3), target-independent: the worker's
//! script calls [`WorkerCore::push`] for each `cmd`, [`WorkerCore::ack`] for each `ack` and
//! [`WorkerCore::run`] once per task, and posts what [`WorkerCore::take_out`] returns. Native tests
//! drive the same loop with their own clock.
//!
//! - **Commands.** A `cmd` from `"host"` is `source.in_use` (Host is the runtime's own producer and
//!   is never handed out, threads.md 5.1), and one whose `seq` is not above its source's last is
//!   `request.out_of_range {path: "/seq"}`: each source's seqs are strictly increasing, as a
//!   `GameClient` makes them natively.
//! - **Boundaries.** Commands that arrived since the last task, with the held ones whose `at` is the
//!   next tick, are one batch at the task's first boundary, applied in the canonical order
//!   (`amoris_link::canonical_order`, threads.md 5.2); ticks inside one task have empty batches
//!   between them. An `at` in the future is held (and counts against the queue's capacity), one in
//!   the past is `command.tick_passed`, and so is a held one whose tick a replaced world skipped
//!   (`amoris_runtime::take_held`, as on the game thread).
//! - **Controls** are the game thread's (threads-slice1.md 8): `step {ticks}`, answered after its
//!   last tick, and `time_control {pause?, pacing?}`; `snapshot` answers its tick, writes and hash.
//!   In a game with players a player sends none of them (`amoris_runtime::player::permitted`).
//! - **Players** (docs/spec/player.md 6) as on the game thread, through the loop core both share
//!   (`amoris_runtime::boundary`): `player.wait` runs its ticks through this loop, one per boundary
//!   in stepped pacing (so the hash stream, the publications and the queue go on between them) or
//!   as the long poll of real time; `player.pacing` sets the players' pacing and the time model;
//!   the decision points are computed after every tick, whatever ran it; and for a game with
//!   players the boundary's pace is their session's (pause-on-decision, a halt, the episode's
//!   end). The time model is `amoris_runtime::TimeModel`, the game thread's.
//! - **Pacing.** A task runs boundaries and ticks until the time model says wait or [`SLICE_MS`] of
//!   work is spent, then answers [`Next`]: run again now, at an instant, or when a message comes.
//! - **Publication.** After every tick, and after a boundary whose Write succeeded when no tick
//!   follows at once. At most [`MAX_IN_FLIGHT`] `snap`s are unacknowledged: past that the worker
//!   skips publication, never a tick, and posts the latest state when an `ack` comes. Each `snap`
//!   carries a lossless section-delta frame (transferred), the events and every tick's world
//!   hash since the previous `snap`, so a page that receives only the latest snapshot still has
//!   every `TickHash` in order (threads-slice1.md 14).
//! - **A poisoned world** halts the game, never stops it (threads.md 8): it has no snapshot of its
//!   own (it is not at a boundary), so wherever the loop would publish it republishes the last
//!   posted snapshot with `time.halted` set, carrying the hashes and events gathered since, and it
//!   keeps answering commands.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use amoris_check::Snapshot;
use amoris_contract::codes::{Range, out_of_range};
use amoris_contract::{Pointer, Problem, detail};
use amoris_link::{
    Envelope, PacingStatus, QUEUE_CAPACITY, ReplyTo, ReplyValue, Source, TimeStatus,
    canonical_order, game_stopped, queue_full, source_from_json, source_in_use, source_json,
    tick_passed,
};
use amoris_runtime::boundary::{Answer, PlayerWaits};
use amoris_runtime::{Command, Game, Pace, Pacing, StepParams, TimeModel};
use amoris_sim::{ContentHash, Tick};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

/// Snapshots posted and not acknowledged, at most (threads.md 7.3).
pub const MAX_IN_FLIGHT: u32 = 2;

/// Work per task before the loop yields to the worker's message queue (threads.md 7.3, chosen).
pub const SLICE_MS: f64 = 8.0;

/// Failed invocations a `step` answers with, at most (threads-slice1.md 8).
const STEP_ERRORS: usize = 20;

/// Tick timings kept for `perf` (the last ones).
const TIMINGS: usize = 16_384;

/// The loop's clock in milliseconds, monotonic: `performance.now()` in a worker. The players'
/// session reads it too (wall limits, thinking clocks), through the game's wall clock.
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// When the worker runs the loop again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Next {
    /// A system suspended this tick for host computation. No boundary commands run until resumed.
    External,
    /// More work is due: yield to the message queue, then run (a `MessageChannel` post, since
    /// browsers clamp nested `setTimeout` to 4 ms).
    Now,
    /// Real time: at this instant of the clock (`setTimeout`), or at the next message.
    At(f64),
    /// Only a message can change the answer.
    Command,
    /// The game stopped.
    Quit,
}

impl Next {
    pub fn to_json(self) -> Value {
        match self {
            Next::External => json!({"next":"external"}),
            Next::Now => json!({"next": "now"}),
            Next::At(t) => json!({"next": "at", "at": t}),
            Next::Command => json!({"next": "command"}),
            Next::Quit => json!({"next": "quit"}),
        }
    }
}

/// A message for the page, in order.
#[derive(Debug)]
pub enum Out {
    Message(Value),
    /// A `snap` and its section-delta wire bytes, which the worker transfers.
    Snap(Value, Vec<u8>),
}

/// `cmd` (threads.md 7.2).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CmdMessage {
    /// "cmd".
    #[allow(dead_code)]
    t: String,
    /// Per source, from 1, strictly increasing.
    seq: u64,
    /// An input of this tick, applied at boundary `at - 1`.
    #[serde(default)]
    at: Option<u64>,
    name: String,
    /// The parameters as a JSON text.
    #[serde(default)]
    params: Option<String>,
}

/// `time_control`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimeControl {
    #[serde(default)]
    pause: Option<bool>,
    #[serde(default)]
    pacing: Option<Pacing>,
}

/// One tick's cost: the tick itself, and its hash with the publication when one was posted.
#[derive(Clone, Copy, Debug)]
pub struct TickTiming {
    pub step_ms: f64,
    pub publish_ms: f64,
    pub posted: bool,
    pub external_ms: f64,
}

#[derive(Default)]
struct WaitingTick {
    active_ms: f64,
    external_ms: f64,
    since: Option<f64>,
}

/// The game role.
pub struct WorkerCore {
    game: Game,
    model: TimeModel,
    clock: Clock,
    /// Commands queued for the next boundary and held for a later one. Each is answered by a
    /// `reply` message keyed by its source and seq, so their `ReplyTo` is none.
    incoming: Vec<Envelope>,
    held: Vec<Envelope>,
    /// The last seq queued from each source.
    last_seq: BTreeMap<Source, u64>,
    /// `step`s: ticks still to run and whom to answer.
    steps: VecDeque<(u64, Source, u64)>,
    /// Players' `wait`s under way, answered by source and seq.
    players: PlayerWaits<(Source, u64)>,
    errors: Vec<Problem>,
    out: VecDeque<Out>,
    version: u64,
    in_flight: u32,
    dirty: bool,
    /// The last snapshot posted: what a halted world republishes.
    last: Option<Snapshot>,
    snapshots: amoris_runtime::SnapshotCache,
    transfer: amoris_link::snapshot_transfer::SnapshotSender,
    /// Every tick's world hash since the last `snap` posted.
    hashes: Vec<(u64, u64, String)>,
    epoch: u64,
    kept: amoris_runtime::kept::KeptSnapshots,
    last_hash: String,
    /// Events since the last `snap` posted, and the stream's last number.
    events: Vec<Value>,
    stream: u64,
    /// The `EventSeq` of the last inbox event streamed: the stream takes the inbox's events past
    /// it, so an inbox a tick did not replace (a poisoned or refused one) streams nothing twice.
    streamed: Option<u64>,
    /// The bundle whose registry the page has.
    sent_bundle: Option<ContentHash>,
    running: Option<bool>,
    since_ms: f64,
    timings: VecDeque<TickTiming>,
    stopped: Option<Problem>,
    waiting_tick: WaitingTick,
}

fn reply_json(source: Source, seq: u64, r: Result<Value, Problem>) -> Value {
    match r {
        Ok(v) => json!({"t": "reply", "source": source_json(source), "seq": seq, "ok": v}),
        Err(p) => json!({"t": "reply", "source": source_json(source), "seq": seq, "error": p}),
    }
}

fn problem_json(p: &Problem) -> Value {
    serde_json::to_value(p).unwrap_or(Value::Null)
}

impl WorkerCore {
    /// The loop over `game`, paced by `pacing`; posts `ready` and the first snapshot (version 1,
    /// with tick 0's hash and the registry).
    pub fn new(mut game: Game, pacing: Pacing, clock: Clock) -> Result<WorkerCore, Problem> {
        pacing.check()?;
        // The players' session measures wall limits and thinking clocks with the loop's clock.
        game.set_wall_clock(clock.clone());
        let model = TimeModel::new(game.sim().clock().rate, pacing);
        let since_ms = clock();
        let mut w = WorkerCore {
            game,
            model,
            clock,
            incoming: Vec::new(),
            held: Vec::new(),
            last_seq: BTreeMap::new(),
            steps: VecDeque::new(),
            players: PlayerWaits::default(),
            errors: Vec::new(),
            out: VecDeque::new(),
            version: 0,
            in_flight: 0,
            dirty: false,
            last: None,
            snapshots: Default::default(),
            transfer: Default::default(),
            hashes: Vec::new(),
            epoch: 0,
            kept: amoris_runtime::kept::KeptSnapshots::new(60, 120),
            last_hash: String::new(),
            events: Vec::new(),
            stream: 0,
            streamed: None,
            sent_bundle: None,
            running: None,
            since_ms,
            timings: VecDeque::new(),
            stopped: None,
            waiting_tick: Default::default(),
        };
        let snap = w.snapshot()?;
        w.last_hash = snap.world_hash().to_string();
        w.hashes
            .push((w.epoch, w.game.tick().0, w.last_hash.clone()));
        w.kept.offer(&snap, true);
        w.out.push_back(Out::Message(
            json!({"t": "ready", "tick": w.game.tick().0, "version": 1}),
        ));
        w.push_events();
        w.post(snap);
        Ok(w)
    }

    /// Extracts the render feed's changes since the last call (presentation only).
    pub fn present(
        &mut self,
        extractor: &mut amoris_runtime::Extractor,
        feed: &amoris_assets::Feed,
    ) {
        self.game.present(extractor, feed);
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    pub fn now(&self) -> f64 {
        (self.clock)()
    }

    /// Snapshots posted and not acknowledged.
    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// The messages for the page since the last call, in order.
    pub fn take_out(&mut self) -> Vec<Out> {
        self.out.drain(..).collect()
    }

    /// The tick timings kept.
    pub fn timings(&self) -> Vec<TickTiming> {
        self.timings.iter().copied().collect()
    }

    /// A `cmd` message: queued for the next boundary, or answered at once when it does not decode,
    /// comes from Host, repeats or goes back on its source's seq, the queue holds its capacity or
    /// the game stopped.
    pub fn push(&mut self, msg: &Value) {
        let echo_source = msg.get("source").cloned().unwrap_or(Value::Null);
        let echo_seq = msg.get("seq").cloned().unwrap_or(Value::Null);
        let refuse =
            |p: Problem| json!({"t": "reply", "source": echo_source, "seq": echo_seq, "error": p});
        // The source is read by its own decoder, which names the forms it takes; the rest of the
        // envelope strictly, unknown fields refused with suggestions (charter 3.4).
        let mut rest = msg.clone();
        if let Some(o) = rest.as_object_mut() {
            o.remove("source");
        }
        let m: CmdMessage = match amoris_runtime::decode(&rest, "a cmd message") {
            Ok(m) => m,
            Err(p) => return self.out.push_back(Out::Message(refuse(p))),
        };
        let source = match source_from_json(&echo_source) {
            Ok(s) => s,
            Err(p) => return self.out.push_back(Out::Message(refuse(p))),
        };
        // Host is the runtime's own producer, never handed out (threads.md 5.1): the page cannot
        // send as Host, which would pass `command.host_only`.
        if source == Source::Host {
            return self
                .out
                .push_back(Out::Message(refuse(source_in_use(Source::Host))));
        }
        let last = self.last_seq.get(&source).copied().unwrap_or(0);
        if m.seq <= last {
            let range = Range {
                min_exclusive: Some(last.into()),
                ..Range::default()
            };
            let hint =
                format!("Each source's seq rises strictly from 1; this source's last was {last}.");
            let p = out_of_range(
                &Pointer::root().key("seq"),
                &json!(m.seq),
                &range,
                Some(&hint),
            );
            return self.out.push_back(Out::Message(refuse(p)));
        }
        let params = match m.params.as_deref().map(serde_json::from_str::<Value>) {
            None => Value::Object(serde_json::Map::new()),
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                let p = Problem::new(
                    "request.malformed",
                    format!("The parameters of {} are not JSON: {e}.", m.name),
                    detail([("name", json!(m.name))]),
                );
                return self.out.push_back(Out::Message(refuse(p)));
            }
        };
        if let Some(why) = &self.stopped {
            return self
                .out
                .push_back(Out::Message(reply_json(source, m.seq, Err(why.clone()))));
        }
        if self.incoming.len() + self.held.len() >= QUEUE_CAPACITY {
            let p = queue_full(QUEUE_CAPACITY);
            return self
                .out
                .push_back(Out::Message(reply_json(source, m.seq, Err(p))));
        }
        // As natively, a seq the queue refused is not taken (`GameClient::send`).
        self.last_seq.insert(source, m.seq);
        self.incoming.push(Envelope {
            source,
            seq: m.seq,
            at: m.at.map(Tick),
            name: m.name,
            params,
            reply: ReplyTo::none(),
        });
    }

    /// The page rebuilt snapshot `version`: one fewer in flight, and the latest state posted if a
    /// publication was skipped meanwhile (a halted republication when the world is poisoned).
    pub fn ack(&mut self, _version: u64) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if self.dirty && self.stopped.is_none() && !self.game.waiting() {
            self.publish();
        }
    }

    /// `close`: every queued, held and stepping command is answered `game.stopped`.
    pub fn close(&mut self) {
        self.stop(game_stopped("the page closed the game", None));
    }

    fn stop(&mut self, why: Problem) {
        let pending: Vec<Envelope> = self.incoming.drain(..).chain(self.held.drain(..)).collect();
        for e in pending {
            self.reply(e.source, e.seq, Err(why.clone()));
        }
        for (_, source, seq) in std::mem::take(&mut self.steps) {
            self.reply(source, seq, Err(why.clone()));
        }
        for (source, seq) in self.players.drain() {
            self.reply(source, seq, Err(why.clone()));
        }
        self.model.quit();
        self.stopped = Some(why);
    }

    fn fatal(&mut self, p: &Problem) {
        self.out
            .push_back(Out::Message(json!({"t": "fatal", "error": p})));
        self.stop(game_stopped("the worker failed", Some(p)));
    }

    fn reply(&mut self, source: Source, seq: u64, r: Result<Value, Problem>) {
        self.out.push_back(Out::Message(reply_json(source, seq, r)));
    }

    /// One task: boundaries and ticks until the time model says wait or `budget_ms` is spent.
    pub fn run(&mut self, budget_ms: f64) -> Next {
        if self.stopped.is_some() {
            return Next::Quit;
        }
        let start = self.now();
        let next = loop {
            if self.game.waiting() {
                self.tick();
                if self.stopped.is_some() {
                    break Next::Quit;
                }
                if self.game.waiting() {
                    break Next::External;
                }
                // Let the host extract the completed boundary before another GPU tick suspends.
                // Otherwise a continuously catching-up game could keep its render feed frozen.
                break Next::Now;
            }
            if let Some(next) = self.boundary() {
                break next;
            }
            if self.game.waiting() {
                break Next::External;
            }
            if self.stopped.is_some() {
                break Next::Quit;
            }
            if self.now() - start >= budget_ms {
                break Next::Now;
            }
        };
        self.status();
        next
    }

    /// Posts `status` when the loop starts or stops running ticks on its own (threads.md 3.5).
    fn status(&mut self) {
        let held = self.game.has_players() && self.game.player().holds();
        let running = self.model.steps_due() > 0
            || (!self.model.paused()
                && !held
                && matches!(self.model.pacing(), Pacing::RealTime { .. }));
        if self.running == Some(running) {
            return;
        }
        let now = self.now();
        let since = self.since_ms;
        self.running = Some(running);
        self.since_ms = now;
        let state = if self.game.sim().poisoned().is_some() {
            "halted"
        } else if running {
            "ticking"
        } else {
            "waiting"
        };
        self.out.push_back(Out::Message(json!({
            "t": "status", "state": state, "tick": self.game.tick().0.saturating_sub(u64::from(self.game.waiting())), "since_ms": now - since,
        })));
    }

    /// One boundary: gather, sort, apply, pace; `None` when a tick ran and the task may go on.
    fn boundary(&mut self) -> Option<Next> {
        let next = Tick(self.game.tick().0 + 1);
        let (mut batch, passed) = amoris_runtime::take_held(&mut self.held, next);
        // A world replaced at a later tick can carry the game past a held command's tick: it is
        // answered and its place in the queue freed, as on the game thread (threads.md 5.2).
        for e in passed {
            let p = tick_passed(e.at.unwrap_or(next), self.game.tick());
            self.reply(e.source, e.seq, Err(p));
        }
        for e in std::mem::take(&mut self.incoming) {
            match e.at {
                Some(t) if t > next => self.held.push(e),
                Some(t) if t < next => {
                    let p = tick_passed(t, self.game.tick());
                    self.reply(e.source, e.seq, Err(p));
                }
                _ => batch.push(e),
            }
        }
        canonical_order(&mut batch);
        let writes = self.game.writes();
        for e in batch {
            self.handle(e);
        }
        let wrote = self.game.writes() != writes;
        self.push_events();
        let now = self.now();
        let pace = self.pace(now);
        if wrote && pace != Pace::RunTick {
            self.publish();
        }
        match pace {
            Pace::RunTick => {
                self.tick();
                None
            }
            Pace::WaitUntil(t) => Some(Next::At(t)),
            Pace::WaitForCommand => Some(Next::Command),
            Pace::Quit => Some(Next::Quit),
        }
    }

    fn handle(&mut self, e: Envelope) {
        let (source, seq) = (e.source, e.seq);
        // A player sends the player tools alone: the commands this loop answers itself are refused
        // here, the others again by the game (as on the game thread).
        let name = amoris_runtime::CATALOG
            .iter()
            .find(|c| c.name == e.name || c.aliases.contains(&e.name.as_str()))
            .map_or(e.name.as_str(), |c| c.name);
        if let Err(p) = amoris_runtime::player::permitted(self.game.sim().world(), name, source) {
            return self.reply(source, seq, Err(p));
        }
        match e.name.as_str() {
            // The web loop runs plain step counts; stop conditions (until, watch) need the native
            // game thread's loop (docs/spec/server.md). A write trace is refused, not ignored: its
            // caller asked for an answer (`writes`) this loop does not give.
            "step" | "time.step" => match amoris_runtime::decode::<StepParams>(&e.params, "step")
                .and_then(|p| match p.trace {
                    Some(_) => Err(Problem::new(
                        "time.trace_unavailable",
                        "The browser's game loop runs plain steps: a write trace                          (time.step {trace}, debug.writes) needs the native host (amoris serve).",
                        detail([("path", json!("/trace"))]),
                    )),
                    None => p.limit(),
                })
            {
                Ok(0) => {
                    let r = self.answer_step();
                    self.reply(source, seq, r);
                }
                Ok(ticks) => match self.game.sim().poisoned() {
                    Some(poison) => {
                        let p = amoris_sim::sim::world_poisoned(poison.tick);
                        self.reply(source, seq, Err(p));
                    }
                    None => {
                        self.steps.push_back((ticks, source, seq));
                        self.reowe();
                    }
                },
                Err(p) => self.reply(source, seq, Err(p)),
            },
            "time_control" => {
                let r = amoris_runtime::decode::<TimeControl>(&e.params, "time_control").and_then(
                    |p| {
                        if let Some(pacing) = p.pacing {
                            pacing.check()?;
                            self.model.set_pacing(pacing);
                        }
                        match p.pause {
                            Some(true) => self.model.pause(),
                            Some(false) => {
                                self.model.resume();
                                // A developer's resume also ends the players' halt after a script
                                // failure (time.md, Halts); a player never gets here.
                                self.game.player_mut().developer_resumed();
                            }
                            None => {}
                        }
                        Ok(serde_json::to_value(self.time_status()).unwrap_or(Value::Null))
                    },
                );
                self.reply(source, seq, r);
            }
            "snapshots.list" => {
                let result = amoris_runtime::decode::<amoris_runtime::NoParams>(&e.params, "snapshots.list")
                    .map(|_| self.kept.to_json());
                self.reply(source, seq, result);
            }
            "snapshots.restore" => {
                let result = amoris_runtime::decode::<amoris_runtime::SnapshotsRestoreParams>(&e.params, "snapshots.restore")
                    .and_then(|p| self.kept.restore(&mut self.game, &p));
                let result = result.and_then(|mut restored| {
                    let why = Problem::new("snapshots.restored", "The world was restored; the step was dropped.", detail([]));
                    self.cancel_steps(&why);
                    self.epoch += 1;
                    self.last_hash = self.game.world_hash()?.to_string();
                    self.streamed = self.game.inbox().last().map(|e| e.seq.0);
                    self.sent_bundle = None;
                    self.running = None;
                    self.timings.clear();
                    self.waiting_tick = Default::default();
                    self.publish();
                    restored["epoch"] = json!(self.epoch);
                    restored["tick"] = json!(self.game.tick().0);
                    restored["world_hash"] = json!(self.last_hash);
                    Ok(restored)
                });
                self.reply(source, seq, result);
            }
            "snapshot" => {
                let r = self
                    .snapshot()
                    .map(|s| ReplyValue::Snapshot(s).into_json());
                self.reply(source, seq, r);
            }
            "player.wait" => {
                let cmd = Command::new(source, seq, &e.name, e.params);
                match self.players.begin(&mut self.game, &cmd, (source, seq)) {
                    Some(answer) => self.answer_players(vec![answer]),
                    None => self.reowe(),
                }
            }
            "player.pacing" => {
                let cmd = Command::new(source, seq, &e.name, e.params);
                let r =
                    amoris_runtime::boundary::player_pacing(&mut self.game, &mut self.model, &cmd);
                self.reply(source, seq, r);
            }
            _ => {
                let cmd = Command::new(source, seq, &e.name, e.params);
                let r = self.game.apply(&cmd);
                self.reply(source, seq, r);
            }
        }
    }

    fn answer_step(&mut self) -> Result<Value, Problem> {
        let hash = self.game.world_hash()?.to_string();
        let errors = std::mem::take(&mut self.errors);
        Ok(json!({"tick": self.game.tick().0, "world_hash": hash, "errors": errors}))
    }

    fn tick(&mut self) {
        // Already poisoned (a resume asks once): no tick runs, so nothing counts as one.
        if let Some(poison) = self.game.sim().poisoned() {
            let p = amoris_sim::sim::world_poisoned(poison.tick);
            return self.halt(&p);
        }
        let t0 = self.now();
        if let Some(since) = self.waiting_tick.since.take() {
            self.waiting_tick.external_ms += t0 - since;
        }
        let r = self.game.poll_step();
        let t1 = self.now();
        self.waiting_tick.active_ms += t1 - t0;
        if matches!(r, Ok(std::task::Poll::Pending)) {
            self.waiting_tick.since = Some(t1);
            return;
        }
        let timing = std::mem::take(&mut self.waiting_tick);
        // The tick ran, or began and poisoned the world: it counts against the model and the front
        // step at once, before the players' hooks ask the model again for what is owed.
        let front = self.steps.front_mut().map(|(left, _, _)| left);
        amoris_runtime::boundary::ran_tick(&mut self.model, front, t1);
        self.push_events();
        match r {
            Ok(std::task::Poll::Ready(report)) => {
                if !self.steps.is_empty() {
                    let room = STEP_ERRORS.saturating_sub(self.errors.len());
                    self.errors.extend(report.errors.iter().take(room).cloned());
                }
                let posted = self.after_tick();
                let mut ended = Vec::new();
                self.players
                    .after_tick(&mut self.game, &report, t1, &mut ended);
                self.answer_players(ended);
                let t2 = self.now();
                if self.timings.len() == TIMINGS {
                    self.timings.pop_front();
                }
                self.timings.push_back(TickTiming {
                    step_ms: timing.active_ms,
                    external_ms: timing.external_ms,
                    publish_ms: t2 - t1,
                    posted,
                });
                if self.steps.front().is_some_and(|(left, _, _)| *left == 0)
                    && let Some((_, source, seq)) = self.steps.pop_front()
                {
                    let r = Ok(json!({"tick": self.game.tick().0,
                        "world_hash": self.last_hash.clone(),
                        "errors": std::mem::take(&mut self.errors)}));
                    self.reply(source, seq, r);
                }
            }
            Ok(std::task::Poll::Pending) => unreachable!(),
            Err(p) if self.game.sim().poisoned().is_some() => self.halt(&p),
            Err(p) => {
                self.cancel_steps(&p);
                self.publish();
            }
        }
    }

    /// Ends every `step` and every players' wait with `p`.
    fn cancel_steps(&mut self, p: &Problem) {
        self.model.cancel_steps();
        self.errors.clear();
        for (_, source, seq) in std::mem::take(&mut self.steps) {
            self.reply(source, seq, Err(p.clone()));
        }
        for (source, seq) in self.players.drain() {
            self.reply(source, seq, Err(p.clone()));
        }
    }

    /// The ticks the model owes: those the `step`s and the players' stepped `wait`s still ask for
    /// (`boundary::owe`).
    fn reowe(&mut self) {
        let steps: u64 = self.steps.iter().map(|(left, _, _)| left).sum();
        amoris_runtime::boundary::owe(&mut self.model, steps, &self.players);
    }

    /// The answer at this boundary (`PlayerWaits::pace`): players' waits whose wall limit passed
    /// are answered first, then their session decides for a game with players unless a `step` is
    /// under way, else the model.
    fn pace(&mut self, now: f64) -> Pace {
        let mut ended = Vec::new();
        self.players.end_due(&mut self.game, now, &mut ended);
        self.answer_players(ended);
        let stepping = !self.steps.is_empty();
        self.players
            .pace(&mut self.game, &mut self.model, stepping, now)
    }

    /// Answers the players' waits that ended and gives back the ticks they no longer owe.
    fn answer_players(&mut self, ended: Vec<Answer<(Source, u64)>>) {
        if ended.is_empty() {
            return;
        }
        for ((source, seq), answer) in ended {
            self.reply(source, seq, answer);
        }
        self.reowe();
    }

    /// A poisoned world halts (threads.md 8): its steps end with `p`, nothing ticks until a
    /// restore (real time stops asking; a resume asks once, is refused and pauses again), the last
    /// snapshot is republished as halted with the hashes and events up to the last good tick, and
    /// the status says halted. The loop keeps answering commands.
    fn halt(&mut self, p: &Problem) {
        self.cancel_steps(p);
        self.model.pause();
        self.publish();
        self.out.push_back(Out::Message(json!({
            "t": "status", "state": "halted", "tick": self.game.tick().0,
            "since_ms": 0.0, "error": problem_json(p),
        })));
        self.running = Some(false);
    }

    fn snapshot(&mut self) -> Result<Snapshot, Problem> {
        self.snapshots
            .snapshot(self.game.world(), self.game.registry())
    }

    /// The tick's hash into the stream, and a publication: posted when fewer than two are in
    /// flight, else skipped (the latest state goes at the next `ack`). Whether one was posted.
    fn after_tick(&mut self) -> bool {
        let tick = self.game.tick().0;
        if self.in_flight < MAX_IN_FLIGHT || self.kept.due(tick) {
            match self.snapshot() {
                Ok(snap) => {
                    self.last_hash = snap.world_hash().to_string();
                    self.hashes.push((self.epoch, tick, self.last_hash.clone()));
                    self.kept.offer(&snap, false);
                    if self.in_flight < MAX_IN_FLIGHT {
                        self.post(snap);
                        true
                    } else {
                        self.dirty = true;
                        false
                    }
                }
                Err(p) => {
                    self.fatal(&p);
                    false
                }
            }
        } else {
            match self.snapshot().map(|s| s.world_hash()) {
                Ok(h) => {
                    self.last_hash = h.to_string();
                    self.hashes.push((self.epoch, tick, self.last_hash.clone()));
                    self.dirty = true;
                }
                Err(p) => self.fatal(&p),
            }
            false
        }
    }

    /// A publication of the world as it is (after a boundary's writes, a failed tick, or an `ack`
    /// that lets a skipped one go), or skipped while two are in flight. A poisoned world has no
    /// snapshot of its own (`persist.not_at_boundary`), so the last snapshot posted goes again,
    /// with the status's `halted` set and what was gathered since; no snapshot is ever built from
    /// a poisoned world.
    fn publish(&mut self) {
        if self.game.waiting() {
            self.dirty = true;
            return;
        }
        if self.in_flight >= MAX_IN_FLIGHT {
            self.dirty = true;
            return;
        }
        if self.game.sim().poisoned().is_some() {
            self.dirty = false;
            if let Some(last) = self.last.take() {
                self.post(last);
            }
            return;
        }
        match self.snapshot() {
            Ok(snap) => self.post(snap),
            Err(p) => self.fatal(&p),
        }
    }

    fn time_status(&self) -> TimeStatus {
        let clock = self.game.sim().clock();
        TimeStatus {
            tick: clock.tick.0,
            t_s: clock.time(),
            pacing: match self.model.pacing() {
                Pacing::Stepped => PacingStatus::Stepped,
                Pacing::RealTime { speed } => PacingStatus::RealTime { speed },
            },
            paused: self.model.paused(),
            halted: self.game.sim().poisoned().is_some(),
            behind_ms: self.model.behind_ms(),
        }
    }

    /// The inbox's events not yet streamed (threads.md 4.4), as JSON records. Event sequence
    /// numbers rise through the inbox and across ticks (a rolled-back Write's numbers return to
    /// the counter before anything streams them), so the events past the last one streamed are
    /// exactly the new ones, whatever the last step did to the inbox.
    fn push_events(&mut self) {
        let last = self.streamed;
        let mut fresh = Vec::new();
        for ev in self.game.inbox() {
            if last.is_some_and(|l| ev.seq.0 <= l) {
                continue;
            }
            self.stream += 1;
            self.streamed = Some(ev.seq.0);
            fresh.push(json!({"seq": self.stream, "id": ev.seq, "event": ev, "epoch": self.epoch}));
        }
        self.events.extend(fresh);
    }

    /// Posts `snap` with what was gathered since the last `snap`, and keeps it as the last.
    fn post(&mut self, snap: Snapshot) {
        self.version += 1;
        self.in_flight += 1;
        self.dirty = false;
        let h = snap.header();
        let timeline = std::mem::take(&mut self.hashes);
        let hash_epochs: Vec<u64> = timeline.iter().map(|(epoch, _, _)| *epoch).collect();
        let hashes: Vec<Value> = timeline
            .into_iter()
            .map(|(_, t, hash)| json!([t, hash]))
            .collect();
        let mut meta = json!({
            "t": "snap",
            "version": self.version,
            "header": {"tick": h.tick.0, "writes": h.writes, "bundle": h.bundle.to_hex(),
                       "engine": h.engine},
            "world_hash": snap.world_hash().to_string(),
            "time": self.time_status(),
            "published_at_ms": self.now(),
            "last_event": self.stream,
            "events": std::mem::take(&mut self.events),
            "missed": 0,
            "hashes": hashes,
            "epoch": self.epoch,
            "hash_epochs": hash_epochs,
        });
        if self.sent_bundle != Some(h.bundle) {
            match self.game.registry_info() {
                Ok(info) => {
                    meta["registry"] =
                        json!({"components": info.components, "formats": info.formats});
                    self.sent_bundle = Some(h.bundle);
                }
                Err(p) => return self.fatal(&p),
            }
        }
        let bytes = match self.transfer.encode(self.version, &snap) {
            Ok(bytes) => bytes,
            Err(p) => return self.fatal(&p),
        };
        meta["snapshot_encoding"] = json!("sections-v1");
        self.out.push_back(Out::Snap(meta, bytes));
        self.last = Some(snap);
    }
}
