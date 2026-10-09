//! The game thread (docs/spec/threads.md 3, 4.3 and 5; native, feature `thread`): a [`Game`] built
//! on its own thread, which alone steps the tick and mutates the world. Presenters and agents read
//! the snapshots it publishes after every tick and every boundary with a Write, and send commands
//! through clients; at each boundary the loop takes what is queued and what was held for this tick,
//! applies it in the canonical order, asks the time model what next, and runs a tick, waits for its
//! clock, waits for a command or quits. The wall clock is the injected one, read only here.
//!
//! The loop also owns what needs one (docs/spec/server.md): Play, which forks the edit world and
//! runs the fork in real time until Stop discards it; the kept snapshots (one every 60 ticks, the
//! last 120) that `snapshots.restore` returns to; `time.step`'s stop conditions, checked after each
//! tick; and the log stream (script `console.*` lines and failed invocations).

use std::collections::{BTreeMap, VecDeque};
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use pocket_contract::{Problem, detail};
use pocket_interface::{Pace, TimeModel};
use pocket_link::{
    Envelope, GameClient, Lease, LogRecord, LoopState, PacingStatus, Publisher, QUEUE_CAPACITY,
    QueueReceiver, QueueSender, Received, RegistryInfo, ReplyTo, ReplyValue, SnapshotReader,
    Source, TimeStatus, WorldInfo, WorldMode, WorldSnapshot, canonical_order, game_stopped,
    publication, queue, source_in_use, tick_passed,
};
use pocket_persist::Snapshot;
use pocket_sim::{ContentHash, Tick};
use serde_json::{Value, json};

use crate::catalog::{self, Command, NoParams};
use crate::control::{
    PlayParams, RestoreBundle, Sampler, SnapshotsRestoreParams, StepParams, StepStop,
    TimeControlParams,
};
use crate::game::{Game, PlayerRun, PlayerStart};

pub use crate::GAME_STACK_BYTES;
pub use pocket_interface::{MAX_SPEED, Pacing};

/// The loop's clock: milliseconds, monotonic (threads.md 3.1).
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// How the game thread runs.
#[derive(Clone)]
pub struct ThreadOptions {
    /// The thread's stack: room for the script host's limit (script-sandbox.md 4.3).
    pub stack_bytes: usize,
    pub clock: Clock,
    /// Called after each publication (a presenter's wake-up).
    pub on_publish: Arc<dyn Fn() + Send + Sync>,
    /// The pacing it starts with.
    pub pacing: Pacing,
    /// The render feed the game extracts visual changes into after each publication.
    pub feed: Option<pocket_assets::Feed>,
    /// Start paused (the editor's edit world: real time, paused until asked).
    pub paused: bool,
    /// Keep a snapshot every this many ticks (0: none).
    pub keep_every: u64,
    /// Kept snapshots held, the oldest dropped first.
    pub keep: usize,
}

impl ThreadOptions {
    /// Stepped pacing, the script host's thread stack, no wake-up.
    pub fn new(clock: Clock) -> ThreadOptions {
        ThreadOptions {
            stack_bytes: GAME_STACK_BYTES,
            clock,
            on_publish: Arc::new(|| {}),
            pacing: Pacing::Stepped,
            feed: None,
            paused: false,
            keep_every: 60,
            keep: 120,
        }
    }
}

/// Starts game threads.
pub struct GameThread;

/// A source as the handle keeps it: whether a client of it is alive, and its next seq, which the
/// source's clients continue one after another (threads.md 5.1: from 1, strictly increasing within
/// a run).
struct SourceSlot {
    in_use: bool,
    next: Arc<AtomicU64>,
}

/// The other threads' handle on a game thread.
pub struct GameHandle {
    queue: QueueSender,
    reader: SnapshotReader,
    attached: Arc<AtomicBool>,
    clients: Clients,
    host_seq: AtomicU64,
    join: Option<JoinHandle<()>>,
}

/// Hands out clients of a game thread; cheap to clone, so a server can open a client per session
/// while the handle stays with whoever shuts the game down.
#[derive(Clone)]
pub struct Clients {
    queue: QueueSender,
    sources: Arc<Mutex<BTreeMap<Source, SourceSlot>>>,
    next_developer: Arc<AtomicU32>,
}

impl GameThread {
    /// Builds the game with `make` on a new thread named `pocket-game` and starts its loop; returns
    /// once the game is built and its first snapshot published, or with the problem that stopped
    /// `make` (or refused the options' pacing).
    pub fn spawn(
        make: impl FnOnce() -> Result<Game, Problem> + Send + 'static,
        options: ThreadOptions,
    ) -> Result<GameHandle, Problem> {
        options.pacing.check()?;
        let (tx, rx) = queue(QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let attached = Arc::new(AtomicBool::new(false));
        let seen = attached.clone();
        let join = std::thread::Builder::new()
            .name("pocket-game".into())
            .stack_size(options.stack_bytes)
            .spawn(move || {
                let built = catch_unwind(AssertUnwindSafe(make))
                    .unwrap_or_else(|p| Err(game_stopped(&panic_text(&*p), None)));
                let mut game = match built {
                    Ok(g) => g,
                    Err(p) => {
                        let _ = ready_tx.send(Err(p));
                        return;
                    }
                };
                // The players' session measures wall limits and thinking clocks with the loop's
                // clock (docs/spec/player.md).
                game.set_wall_clock(options.clock.clone());
                let mut model = TimeModel::new(game.sim().clock().rate, options.pacing);
                if options.paused {
                    model.pause();
                }
                let first = match first_snapshot(&mut game, &model, &options) {
                    Ok(s) => s,
                    Err(p) => {
                        let _ = ready_tx.send(Err(p));
                        return;
                    }
                };
                let (publisher, reader) = publication(first.clone(), options.clock.clone());
                let registry = Arc::new(game.registry_info().unwrap_or_default());
                let mut kept = Kept::new(options.keep_every, options.keep);
                kept.offer(&first.snapshot, true);
                if ready_tx.send(Ok(reader)).is_err() {
                    return;
                }
                publisher.set_world(WorldInfo {
                    mode: WorldMode::Edit,
                    epoch: 0,
                });
                let mut l = Loop {
                    bundle: game.bundle(),
                    game,
                    rx,
                    publisher,
                    model,
                    kept,
                    parked: None,
                    epoch: 0,
                    options,
                    held: Vec::new(),
                    pending: VecDeque::new(),
                    steps: VecDeque::new(),
                    player_runs: Vec::new(),
                    player_replies: Vec::new(),
                    errors: Vec::new(),
                    attached: seen,
                    version: 1,
                    pushed: 0,
                    registry,
                    extractor: crate::present::Extractor::new(),
                };
                l.run();
            })
            .map_err(|e| game_stopped(&format!("the game thread did not start: {e}"), None))?;
        let reader = ready_rx
            .recv()
            .unwrap_or_else(|_| Err(game_stopped("the game thread ended while starting", None)))?;
        Ok(GameHandle {
            queue: tx.clone(),
            reader,
            attached,
            clients: Clients {
                queue: tx.clone(),
                sources: Arc::new(Mutex::new(BTreeMap::new())),
                next_developer: Arc::new(AtomicU32::new(0)),
            },
            host_seq: AtomicU64::new(0),
            join: Some(join),
        })
    }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".into())
}

fn time_status(game: &Game, model: &TimeModel) -> TimeStatus {
    let clock = game.sim().clock();
    TimeStatus {
        tick: clock.tick.0,
        t_s: clock.time(),
        pacing: match model.pacing() {
            Pacing::Stepped => PacingStatus::Stepped,
            Pacing::RealTime { speed } => PacingStatus::RealTime { speed },
        },
        paused: model.paused(),
        halted: game.sim().poisoned().is_some(),
        behind_ms: model.behind_ms(),
    }
}

fn first_snapshot(
    game: &mut Game,
    model: &TimeModel,
    o: &ThreadOptions,
) -> Result<WorldSnapshot, Problem> {
    Ok(WorldSnapshot {
        version: 1,
        snapshot: game.snapshot()?,
        registry: Arc::new(game.registry_info()?),
        time: time_status(game, model),
        last_event: 0,
        published_at_ms: (o.clock)(),
    })
}

impl Clients {
    /// A client of `source`: one live client per source (`source.in_use` while another is held).
    /// A client handed out again continues the source's sequence numbers. `Source::Host` is the
    /// runtime's own and is not handed out.
    pub fn client(&self, source: Source) -> Result<GameClient, Problem> {
        if source == Source::Host {
            return Err(source_in_use(source));
        }
        let mut sources = self.sources.lock().map_err(|_| source_in_use(source))?;
        let slot = sources.entry(source).or_insert_with(|| SourceSlot {
            in_use: false,
            next: Arc::new(AtomicU64::new(1)),
        });
        if slot.in_use {
            return Err(source_in_use(source));
        }
        slot.in_use = true;
        let next = slot.next.clone();
        let all = self.sources.clone();
        let lease = Lease::new(move || {
            if let Ok(mut s) = all.lock()
                && let Some(slot) = s.get_mut(&source)
            {
                slot.in_use = false;
            }
        });
        Ok(GameClient::new(
            source,
            next,
            self.queue.clone(),
            Some(lease),
        ))
    }

    /// The next developer session's client (`Developer(0)`, `Developer(1)`, ...).
    pub fn developer(&self) -> GameClient {
        loop {
            let n = self.next_developer.fetch_add(1, Ordering::AcqRel);
            if let Ok(c) = self.client(Source::Developer(n)) {
                return c;
            }
        }
    }
}

impl GameHandle {
    /// A client of `source` ([`Clients::client`]).
    pub fn client(&self, source: Source) -> Result<GameClient, Problem> {
        self.clients.client(source)
    }

    /// The next developer session's client (`Developer(0)`, `Developer(1)`, ...).
    pub fn developer(&self) -> GameClient {
        self.clients.developer()
    }

    /// What hands out clients, apart from the handle.
    pub fn clients(&self) -> Clients {
        self.clients.clone()
    }

    /// The loop state's handle: what a script debugger sets while it holds the game thread inside
    /// a tick (`pocket_debug::DebugHub::set_loop_state`; threads.md 3.5).
    pub fn loop_state(&self) -> pocket_link::StateHandle {
        self.reader.state_handle()
    }

    /// A reader of the snapshots; from now on the game publishes after every tick.
    pub fn reader(&self) -> SnapshotReader {
        self.attached.store(true, Ordering::Release);
        self.reader.clone()
    }

    /// Asks the game to quit after the tick it is in, answering what is queued with
    /// `game.stopped`, and waits for its thread at most `timeout_ms` (threads.md 8).
    pub fn shutdown(mut self, timeout_ms: u64) -> Result<(), Problem> {
        let seq = self.host_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.queue.push_unbounded(Envelope {
            source: Source::Host,
            seq,
            at: None,
            name: "shutdown".into(),
            params: Value::Null,
            reply: ReplyTo::none(),
        });
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        let mut waited = 0;
        while !join.is_finished() && waited < timeout_ms {
            std::thread::sleep(Duration::from_millis(1));
            waited += 1;
        }
        if join.is_finished() {
            let _ = join.join();
            Ok(())
        } else {
            Err(game_stopped(
                &format!("the game thread did not stop within {timeout_ms} ms"),
                None,
            ))
        }
    }
}

/// The kept snapshots of one world (docs/spec/server.md, snapshots): one every `every` ticks, the
/// last `keep` of them.
struct Kept {
    every: u64,
    keep: usize,
    items: VecDeque<Snapshot>,
}

impl Kept {
    fn new(every: u64, keep: usize) -> Kept {
        Kept {
            every,
            keep,
            items: VecDeque::new(),
        }
    }

    /// Keeps `s` if its tick is due (or `always`, the world's first state).
    fn offer(&mut self, s: &Snapshot, always: bool) {
        let tick = s.header().tick.0;
        let due = self.every > 0 && tick.is_multiple_of(self.every);
        if !(due || always) || self.keep == 0 {
            return;
        }
        if self.items.back().is_some_and(|b| b.header().tick.0 >= tick) {
            self.drop_after(tick.saturating_sub(1));
        }
        if self.items.len() == self.keep {
            self.items.pop_front();
        }
        self.items.push_back(s.clone());
    }

    fn at_or_before(&self, tick: u64) -> Option<Snapshot> {
        self.items
            .iter()
            .rev()
            .find(|s| s.header().tick.0 <= tick)
            .cloned()
    }

    fn drop_after(&mut self, tick: u64) {
        while self.items.back().is_some_and(|s| s.header().tick.0 > tick) {
            self.items.pop_back();
        }
    }

    fn ticks(&self) -> Vec<u64> {
        self.items.iter().map(|s| s.header().tick.0).collect()
    }

    fn to_json(&self) -> Value {
        let list: Vec<Value> = self
            .items
            .iter()
            .map(|s| json!({"tick": s.header().tick.0, "world_hash": s.world_hash().to_string()}))
            .collect();
        json!({"every": self.every, "keep": self.keep, "snapshots": list})
    }
}

/// The edit world while Play runs its fork.
struct Parked {
    game: Game,
    model: TimeModel,
    kept: Kept,
}

/// A `time.step` being run: ticks left, where the answer goes, its stop conditions and sampler.
struct StepJob {
    left: u64,
    reply: ReplyTo,
    stop: Option<StepStop>,
    sample: Option<Sampler>,
}

struct Loop {
    game: Game,
    rx: QueueReceiver,
    publisher: Publisher,
    model: TimeModel,
    kept: Kept,
    /// The edit world, while Play runs.
    parked: Option<Parked>,
    /// Changes whenever the world shown is replaced (restore, Play, Stop).
    epoch: u64,
    options: ThreadOptions,
    /// Envelopes for a later tick.
    held: Vec<Envelope>,
    /// Envelopes a wait received, for the next batch.
    pending: VecDeque<Envelope>,
    /// `time.step` requests in order.
    steps: VecDeque<StepJob>,
    /// Players' `wait`s under way (docs/spec/player.md), each with where its answer goes: stepped
    /// ones run their ticks one per boundary, real-time ones wait for their seat's decision.
    player_runs: Vec<PlayerRun>,
    player_replies: Vec<ReplyTo>,
    /// Failed invocations of the ticks the current `step` ran (at most 20).
    errors: Vec<Problem>,
    attached: Arc<AtomicBool>,
    version: u64,
    /// Inbox events already in the stream at this boundary.
    pushed: usize,
    registry: Arc<RegistryInfo>,
    bundle: ContentHash,
    extractor: crate::present::Extractor,
}

enum Next {
    Go,
    Quit,
}

fn problem(code: &str, message: String) -> Problem {
    Problem::new(code, message, detail([]))
}

/// A console location `file:line:column` split.
fn location(loc: &str) -> (Option<String>, Option<u64>) {
    let mut parts = loc.rsplitn(3, ':');
    let col = parts.next();
    let line = parts.next();
    match (parts.next(), line, col) {
        (Some(file), Some(line), Some(_)) => (Some(file.to_owned()), line.parse().ok()),
        _ => (Some(loc.to_owned()), None),
    }
}

impl Loop {
    fn now(&self) -> f64 {
        (self.options.clock)()
    }

    fn mode(&self) -> WorldMode {
        if self.parked.is_some() {
            WorldMode::Play
        } else {
            WorldMode::Edit
        }
    }

    fn run(&mut self) {
        let outcome = catch_unwind(AssertUnwindSafe(|| while let Next::Go = self.boundary() {}));
        let why = match outcome {
            Ok(()) => game_stopped("the game was shut down", None),
            Err(p) => {
                let text = panic_text(&*p);
                game_stopped(
                    &format!(
                        "the game thread panicked at tick {}: {text}",
                        self.game.tick().0
                    ),
                    None,
                )
            }
        };
        self.stop(why);
    }

    /// Answers everything queued, held and waiting with `why`, and marks the game stopped; the
    /// last snapshot stays readable.
    fn stop(&mut self, why: Problem) {
        self.publisher.stopped(self.game.tick(), Some(why.clone()));
        let answer = |e: Envelope| e.reply.send(Err(why.clone()));
        for e in self.held.drain(..).chain(self.pending.drain(..)) {
            answer(e);
        }
        for j in self.steps.drain(..) {
            j.reply.send(Err(why.clone()));
        }
        while let Received::One(e) = self.rx.try_next() {
            answer(e);
        }
    }

    fn next_tick(&self) -> Tick {
        Tick(self.game.tick().0 + 1)
    }

    /// One boundary: gather, sort, apply, pace.
    fn boundary(&mut self) -> Next {
        let next = self.next_tick();
        let mut batch: Vec<Envelope> = Vec::new();
        let (due, passed) = take_held(&mut self.held, next);
        // A world replaced at a later tick (a restore or Play's fork swapped in) can carry the
        // game past a held envelope's tick: it is answered and its place freed (threads.md 5.2).
        for e in passed {
            self.rx.done(1);
            let at = e.at.unwrap_or(next);
            e.reply.send(Err(tick_passed(at, self.game.tick())));
        }
        batch.extend(due);
        let mut incoming: Vec<Envelope> = self.pending.drain(..).collect();
        loop {
            match self.rx.try_next() {
                Received::One(e) => incoming.push(e),
                Received::Empty => break,
                Received::Closed => return Next::Quit,
            }
        }
        for e in incoming {
            match e.at {
                Some(t) if t > next => self.held.push(e),
                Some(t) if t < next => {
                    self.rx.done(1);
                    e.reply.send(Err(tick_passed(t, self.game.tick())));
                }
                _ => batch.push(e),
            }
        }
        canonical_order(&mut batch);
        self.publisher
            .set_state(LoopState::Applying, self.game.tick());
        let writes = self.game.writes();
        let epoch = self.epoch;
        let mut quit = false;
        for e in batch {
            self.rx.done(1);
            quit |= self.handle(e);
        }
        let wrote = self.game.writes() != writes && self.epoch == epoch;
        self.push_events();
        if quit {
            return Next::Quit;
        }
        let now = self.now();
        let pace = self.pace(now);
        if wrote && pace != Pace::RunTick {
            self.publish();
        }
        match pace {
            Pace::RunTick => self.tick(),
            Pace::WaitUntil(t) => self.wait_until(t),
            Pace::WaitForCommand => {
                self.publisher
                    .set_state(LoopState::Waiting, self.game.tick());
                // With a render feed, wake every 50 ms so a viewport that subscribes while the
                // game is paused (the edit world) still gets its first full frame.
                loop {
                    let got = match &self.options.feed {
                        Some(_) => self.rx.next_within(Duration::from_millis(50)),
                        None => self.rx.next(),
                    };
                    match got {
                        Received::One(e) => {
                            self.pending.push_back(e);
                            break;
                        }
                        Received::Empty => {
                            if let Some(feed) = &self.options.feed
                                && feed.wants_reset()
                            {
                                self.game.present(&mut self.extractor, feed);
                            }
                        }
                        Received::Closed => return Next::Quit,
                    }
                }
            }
            Pace::Quit => return Next::Quit,
        }
        Next::Go
    }

    /// Waits in slices of at most 1 ms (threads.md 3.2: Windows' timed waits overshoot), taking any
    /// command that arrives meanwhile.
    fn wait_until(&mut self, t: f64) {
        self.publisher
            .set_state(LoopState::Waiting, self.game.tick());
        loop {
            match self.rx.try_next() {
                Received::One(e) => {
                    self.pending.push_back(e);
                    return;
                }
                Received::Closed => return,
                Received::Empty => {}
            }
            if let Some(feed) = &self.options.feed
                && feed.wants_reset()
            {
                self.game.present(&mut self.extractor, feed);
            }
            let left = t - self.now();
            if left <= 0.0 {
                return;
            }
            std::thread::sleep(Duration::from_secs_f64(
                pocket_sim::math::min(left, 1.0) / 1000.0,
            ));
        }
    }

    fn tick(&mut self) {
        self.publisher
            .set_state(LoopState::Ticking, self.next_tick());
        // A stop of the script debugger inside this tick answers the `time.step` it belongs to
        // and ends it with the tick (pocket-link's `StateHandle::stopped`).
        let held = self.hold_step_reply();
        let r = self.game.step();
        let stopped = held && !self.release_step_reply();
        let now = self.now();
        self.model.ran_tick(now);
        self.pushed = 0;
        self.push_events();
        self.push_log(r.as_ref().ok().map(|rep| rep.errors.as_slice()));
        match r {
            Ok(report) => {
                if !self.steps.is_empty() {
                    let room = 20usize.saturating_sub(self.errors.len());
                    self.errors.extend(report.errors.iter().take(room).cloned());
                }
                self.present();
                let tick = self.game.tick().0;
                let keep = self.kept.every > 0 && tick.is_multiple_of(self.kept.every);
                if (keep || self.attached.load(Ordering::Acquire))
                    && let Ok(snap) = self.game.snapshot()
                {
                    if keep {
                        self.kept.offer(&snap, false);
                    }
                    self.publish_snapshot(snap);
                }
                self.after_tick_players(&report, now);
                self.after_tick_steps(stopped);
            }
            Err(p) => {
                self.model.cancel_steps();
                self.errors.clear();
                for j in self.steps.drain(..) {
                    j.reply.send(Err(p.clone()));
                }
                self.drop_player_runs(&p);
                if self.game.sim().poisoned().is_some() {
                    // Nothing ticks until a restore: real time stops asking for ticks (a resume
                    // asks once more and is refused), and the status says halted.
                    self.model.pause();
                    self.publish_halted();
                } else {
                    self.publish();
                }
            }
        }
    }

    /// Before a tick of a `time.step` with a script debugger attached: hands the step's reply to
    /// the loop state, where a stop of the debugger inside the tick answers it at once.
    fn hold_step_reply(&mut self) -> bool {
        if self.game.script_debugger().is_none() {
            return false;
        }
        let Some(job) = self.steps.front_mut() else {
            return false;
        };
        let reply = mem::replace(&mut job.reply, ReplyTo::none());
        self.publisher.hold_step_reply(reply);
        true
    }

    /// After that tick: the reply back to its step, or false when a stop answered it.
    fn release_step_reply(&mut self) -> bool {
        let Some(reply) = self.publisher.take_step_reply() else {
            return false;
        };
        if let Some(job) = self.steps.front_mut() {
            job.reply = reply;
        }
        true
    }

    /// Counts the tick against the front `time.step` and answers it when it is done or its stop
    /// condition holds; an early stop gives back the ticks the time model still owes it.
    /// `stopped`: the script debugger stopped the game inside the tick and answered the step there
    /// (`stopped_by` the stop's summary), so the step ends with the tick and its rest is dropped.
    fn after_tick_steps(&mut self, stopped: bool) {
        let Some(job) = self.steps.front_mut() else {
            return;
        };
        job.left = job.left.saturating_sub(1);
        if stopped {
            self.errors.clear();
            self.end_front_step();
            return;
        }
        if let Some(s) = &mut job.sample {
            s.after_tick(&self.game);
        }
        let why = job.stop.as_mut().and_then(|s| s.after_tick(&self.game));
        if job.left > 0 && why.is_none() {
            return;
        }
        let Some(job) = self.end_front_step() else {
            return;
        };
        let why = why.or_else(|| job.stop.as_ref().map(|_| json!({"reason": "limit"})));
        let mut answer = self.answer_step();
        if let Ok(ReplyValue::Json(v)) = &mut answer {
            if let Some(why) = why {
                v["stopped_by"] = why;
            }
            if let Some(s) = job.sample {
                v["samples"] = s.finish(&self.game);
            }
        }
        job.reply.send(answer);
    }

    /// Takes the front `time.step` off the queue; ticks it had left are no longer owed (the time
    /// model keeps owing the steps behind it theirs).
    fn end_front_step(&mut self) -> Option<StepJob> {
        let job = self.steps.pop_front()?;
        if job.left > 0 {
            self.reowe();
        }
        Some(job)
    }

    /// The ticks the model owes: those the `time.step`s and the players' stepped `wait`s still ask
    /// for (a step and a wait under way at once share the ticks the model runs).
    fn reowe(&mut self) {
        let steps: u64 = self.steps.iter().map(|j| j.left).sum();
        let players = self
            .player_runs
            .iter()
            .map(PlayerRun::ticks_left)
            .max()
            .unwrap_or(0);
        self.model.cancel_steps();
        self.model.step(steps.max(players));
    }

    /// The answer at this boundary: for a game with players their session decides (a halt, the
    /// episode's end, real time held by a pending decision), else the model; a developer's
    /// `time.step` under way runs its ticks whatever holds the players. Players' runs whose wall
    /// limit passed are answered first, and the earliest wall limit of those still waiting wakes
    /// the loop.
    fn pace(&mut self, now: f64) -> Pace {
        self.end_player_runs(now);
        let pace = if self.game.has_players() && self.steps.is_empty() {
            self.game.player_pace(&mut self.model, now)
        } else {
            self.model.pace(now)
        };
        let deadline = self
            .player_runs
            .iter()
            .map(PlayerRun::wall_deadline)
            .reduce(pocket_sim::math::min);
        match (pace, deadline) {
            (Pace::WaitForCommand, Some(d)) => Pace::WaitUntil(d),
            (Pace::WaitUntil(t), Some(d)) => Pace::WaitUntil(pocket_sim::math::min(t, d)),
            (p, _) => p,
        }
    }

    /// Answers the players' runs that end at this boundary: past their wall limit, or (stepped)
    /// unable to take the next tick.
    fn end_player_runs(&mut self, now: f64) {
        let mut ended = Vec::new();
        for (i, r) in self.player_runs.iter_mut().enumerate() {
            if now >= r.wall_deadline() {
                r.stop_at_wall();
                ended.push(i);
            } else if !r.real_time() && !r.before_tick(&self.game) {
                ended.push(i);
            }
        }
        self.finish_player_runs(ended);
    }

    /// After a tick: the players' decision points, once, and every run's stop.
    fn after_tick_players(&mut self, report: &pocket_sim::StepReport, now: f64) {
        let ended = self
            .game
            .player_after_tick(report, &mut self.player_runs, now);
        self.finish_player_runs(ended);
    }

    /// Answers the runs at these indices (ascending) and gives back what they no longer owe.
    fn finish_player_runs(&mut self, ended: Vec<usize>) {
        if ended.is_empty() {
            return;
        }
        for i in ended.into_iter().rev() {
            let run = self.player_runs.remove(i);
            let reply = self.player_replies.remove(i);
            let answer = self.game.finish_player_run(run).map(ReplyValue::Json);
            reply.send(answer);
        }
        self.reowe();
    }

    /// Answers the players' runs with `why` (their world went away or a tick failed).
    fn drop_player_runs(&mut self, why: &Problem) {
        self.player_runs.clear();
        for reply in self.player_replies.drain(..) {
            reply.send(Err(why.clone()));
        }
    }

    /// `player.wait` on the loop (docs/spec/player.md): stepped pacing asks the model for the
    /// run's ticks and counts them as they run, so the queue is served between ticks; real time
    /// waits for the seat's decision as the clock runs the ticks.
    fn player_wait(&mut self, e: Envelope) {
        if let Some(poison) = self.game.sim().poisoned() {
            return e
                .reply
                .send(Err(pocket_sim::sim::world_poisoned(poison.tick)));
        }
        let cmd = Command::new(e.source, e.seq, &e.name, e.params);
        match self.game.begin_player_wait(&cmd) {
            Ok(PlayerStart::Answered(v)) => e.reply.send(Ok(ReplyValue::Json(v))),
            Ok(PlayerStart::Running(run)) => {
                self.player_runs.push(run);
                self.player_replies.push(e.reply);
                self.reowe();
            }
            Err(p) => e.reply.send(Err(p)),
        }
    }

    /// `player.pacing` on the loop: the players' pacing, and the loop's model to time it (real
    /// time runs, unpaused; stepped waits for steps and waits).
    fn player_pacing(&mut self, e: Envelope) {
        let cmd = Command::new(e.source, e.seq, &e.name, e.params);
        let r = self.game.set_player_pacing(&cmd).map(|(pacing, v)| {
            match pacing {
                pocket_interface::time::play::PlayPacing::RealTime { speed, .. } => {
                    self.model.set_pacing(Pacing::RealTime { speed });
                    self.model.resume();
                }
                _ => self.model.set_pacing(Pacing::Stepped),
            }
            v
        });
        e.reply.send(r.map(ReplyValue::Json));
    }

    /// A finished `step`: the tick, the hash and the failed invocations of its ticks.
    fn answer_step(&mut self) -> Result<ReplyValue, Problem> {
        let hash = self.game.world_hash()?;
        let errors = mem::take(&mut self.errors);
        Ok(ReplyValue::Json(json!({
            "tick": self.game.tick().0,
            "world_hash": hash.to_string(),
            "errors": errors,
        })))
    }

    /// The status the loop answers `status` and `time.control` with: spec-contract's
    /// `TimeStatus` and what the editor shows beside it.
    fn status(&self) -> Value {
        let mut v = serde_json::to_value(time_status(&self.game, &self.model)).unwrap_or(json!({}));
        let world = self.game.world();
        v["mode"] = json!(self.mode().as_str());
        v["epoch"] = json!(self.epoch);
        v["world_hash"] = json!(self.game.world_hash().ok().map(|h| h.to_string()));
        v["entities"] = json!(world.resource::<pocket_sim::EntityIndex>().len());
        v["bundle"] = json!(self.game.bundle().to_hex());
        v["writes"] = json!(self.game.writes());
        v["poisoned"] = json!(self.game.sim().poisoned().map(|p| p.tick.0));
        v["steps_due"] = json!(self.model.steps_due());
        v["kept"] = json!(self.kept.items.len());
        // The first tick a script debugger evaluated in (script-host.md 13; debugger.md 5).
        v["tainted"] = json!(self.game.tainted());
        v
    }

    /// Answers the step requests and the players' waits with `why` (their world went away).
    fn drop_steps(&mut self, why: &Problem) {
        self.model.cancel_steps();
        self.errors.clear();
        for j in self.steps.drain(..) {
            j.reply.send(Err(why.clone()));
        }
        self.drop_player_runs(why);
    }

    /// The world shown was replaced: a new epoch, the stream continues from its inbox, and the
    /// presenters get a publication at once.
    fn world_replaced(&mut self) {
        self.epoch += 1;
        self.publisher.set_world(WorldInfo {
            mode: self.mode(),
            epoch: self.epoch,
        });
        self.pushed = self.game.inbox().len();
        self.bundle = ContentHash([0; 32]);
        self.publish();
    }

    fn play_start(&mut self, params: &Value) -> Result<Value, Problem> {
        let p: PlayParams = crate::decode(params, "play.start")?;
        if self.parked.is_some() {
            return Err(problem(
                "play.running",
                "Play is already running; play.stop returns to the edit world.".into(),
            ));
        }
        let speed = p.speed.unwrap_or(1.0);
        let pacing = Pacing::RealTime { speed };
        pacing.check()?;
        let mut fork = self.game.fork()?;
        // The script debugger follows the world that runs (docs/spec/debugger.md 8): Play's fork
        // takes it, Stop hands it back.
        if let Some(hook) = self.game.script_debugger() {
            self.game.set_script_debugger(None);
            fork.set_script_debugger(Some(hook));
        }
        let mut model = TimeModel::new(fork.sim().clock().rate, pacing);
        if p.paused {
            model.pause();
        }
        self.drop_steps(&problem(
            "play.started",
            "Play started; the edit world's step was dropped.".into(),
        ));
        let mut kept = Kept::new(self.options.keep_every, self.options.keep);
        if let Ok(s) = fork.snapshot() {
            kept.offer(&s, true);
        }
        let game = mem::replace(&mut self.game, fork);
        let model = mem::replace(&mut self.model, model);
        let kept = mem::replace(&mut self.kept, kept);
        self.parked = Some(Parked { game, model, kept });
        self.world_replaced();
        Ok(self.status())
    }

    fn play_stop(&mut self, params: &Value) -> Result<Value, Problem> {
        crate::decode::<NoParams>(params, "play.stop")?;
        let Some(mut parked) = self.parked.take() else {
            return Err(problem(
                "play.not_running",
                "Play is not running; the edit world is shown.".into(),
            ));
        };
        self.drop_steps(&problem(
            "play.stopped",
            "Play stopped; its step was dropped.".into(),
        ));
        if let Some(hook) = self.game.script_debugger() {
            self.game.set_script_debugger(None);
            parked.game.set_script_debugger(Some(hook));
        }
        self.game = parked.game;
        self.model = parked.model;
        self.kept = parked.kept;
        self.world_replaced();
        Ok(self.status())
    }

    /// `snapshots.restore`: the kept snapshot at or before the tick, under the scripts applied now
    /// unless `bundle: "snapshot"` asks for the snapshot's own. The world is restored under the
    /// bundle it was kept with and, when that is not the applied one, swapped back to it by the
    /// hot update's Host write (`scripts.swap`, which migrates components a newer bundle changed),
    /// so a recording sees the restore and then the swap, as they ran.
    fn restore(&mut self, params: &Value) -> Result<Value, Problem> {
        let p: SnapshotsRestoreParams = crate::decode(params, "snapshots.restore")?;
        let Some(snap) = self.kept.at_or_before(p.tick) else {
            return Err(Problem::new(
                "snapshots.none",
                format!("No kept snapshot is at or before tick {}.", p.tick),
                detail([("tick", json!(p.tick)), ("kept", json!(self.kept.ticks()))]),
            ));
        };
        let applied = self.game.bundle();
        self.game.restore_any(&snap)?;
        let at = snap.header().tick.0;
        let kept_with = snap.header().bundle;
        let mut scripts = json!({
            "bundle": kept_with.to_hex(),
            "kept": "snapshot",
            "snapshot_bundle": kept_with.to_hex(),
        });
        if p.bundle == RestoreBundle::Applied {
            if kept_with == applied {
                scripts["kept"] = json!("applied");
            } else {
                match self.game.swap_bundle(applied) {
                    Ok(_) => {
                        scripts["kept"] = json!("applied");
                        scripts["bundle"] = json!(applied.to_hex());
                        scripts["swapped"] = json!(true);
                    }
                    // The applied scripts do not load on this world: it runs the snapshot's.
                    Err(e) => {
                        scripts["swap_refused"] = serde_json::to_value(&e).unwrap_or(Value::Null);
                    }
                }
            }
        }
        self.kept.drop_after(at);
        self.drop_steps(&problem(
            "snapshots.restored",
            "The world was restored; the step was dropped.".into(),
        ));
        self.world_replaced();
        let mut v = self.status();
        v["restored"] = json!(at);
        v["scripts"] = scripts;
        Ok(v)
    }

    fn time_control(&mut self, params: &Value) -> Result<Value, Problem> {
        let p: TimeControlParams = crate::decode(params, "time.control")?;
        if let Some(speed) = p.speed {
            let pacing = Pacing::RealTime { speed };
            pacing.check()?;
            self.model.set_pacing(pacing);
        }
        if let Some(pacing) = p.pacing {
            pacing.check()?;
            self.model.set_pacing(pacing);
        }
        match p.pause {
            Some(true) => self.model.pause(),
            Some(false) => {
                self.model.resume();
                // A developer's resume also ends the players' halt after a script failure
                // (time.md, Halts); a game's player never gets here (`player::permitted`).
                self.game.player_mut().developer_resumed();
            }
            None => {}
        }
        Ok(self.status())
    }

    fn step(&mut self, e: Envelope) {
        let p = match crate::decode::<StepParams>(&e.params, "time.step") {
            Ok(p) => p,
            Err(p) => return e.reply.send(Err(p)),
        };
        let limit = match p.limit() {
            Ok(n) => n,
            Err(p) => return e.reply.send(Err(p)),
        };
        if limit == 0 {
            return e.reply.send(self.answer_step());
        }
        if let Some(poison) = self.game.sim().poisoned() {
            return e
                .reply
                .send(Err(pocket_sim::sim::world_poisoned(poison.tick)));
        }
        let checked = StepStop::new(&self.game, &p)
            .and_then(|stop| Ok((stop, Sampler::new(&self.game, &p, limit)?)));
        match checked {
            Ok((stop, sample)) => {
                self.model.step(limit);
                self.steps.push_back(StepJob {
                    left: limit,
                    reply: e.reply,
                    stop,
                    sample,
                });
            }
            Err(p) => e.reply.send(Err(p)),
        }
    }

    /// Applies one command; `true` when it asks the loop to quit.
    fn handle(&mut self, e: Envelope) -> bool {
        let def = catalog::find(&e.name);
        let name = def.map_or(e.name.as_str(), |d| d.name);
        let json = |r: Result<Value, Problem>| r.map(ReplyValue::Json);
        // A player sends the player tools alone (`crate::player::permitted`): the commands the
        // loop answers itself (time, Play, the kept snapshots, the status) are refused here, the
        // others again by the game.
        if def.is_some()
            && let Err(p) = crate::player::permitted(self.game.sim().world(), name, e.source)
        {
            e.reply.send(Err(p));
            return false;
        }
        match name {
            "shutdown" if e.source == Source::Host => {
                self.model.quit();
                e.reply.send(Ok(ReplyValue::Json(json!({}))));
                return true;
            }
            "time.step" => self.step(e),
            "player.wait" => self.player_wait(e),
            "player.pacing" => self.player_pacing(e),
            "time.control" => {
                let r = self.time_control(&e.params);
                e.reply.send(json(r));
            }
            "status" => {
                let r = crate::decode::<NoParams>(&e.params, "status").map(|_| self.status());
                e.reply.send(json(r));
            }
            "play.start" => {
                let r = self.play_start(&e.params);
                e.reply.send(json(r));
            }
            "play.stop" => {
                let r = self.play_stop(&e.params);
                e.reply.send(json(r));
            }
            "snapshots.list" => {
                let r = crate::decode::<NoParams>(&e.params, "snapshots.list")
                    .map(|_| self.kept.to_json());
                e.reply.send(json(r));
            }
            "snapshots.restore" => {
                let r = self.restore(&e.params);
                e.reply.send(json(r));
            }
            "snapshot" => {
                let r = self.game.snapshot().map(ReplyValue::Snapshot);
                e.reply.send(r);
            }
            "project.save" if self.parked.is_some() => {
                // Play's world is never saved: the edit world is.
                let cmd = Command::new(e.source, e.seq, &e.name, e.params);
                let r = match &mut self.parked {
                    Some(p) => p.game.apply(&cmd),
                    None => Err(catalog::thread_only("project.save")),
                };
                e.reply.send(json(r));
            }
            _ => {
                let cmd = Command::new(e.source, e.seq, &e.name, e.params);
                let r = self.game.apply(&cmd).map(ReplyValue::Json);
                e.reply.send(r);
            }
        }
        false
    }

    /// Appends the inbox's events not yet in the stream (threads.md 4.4).
    fn push_events(&mut self) {
        if !self.attached.load(Ordering::Acquire) {
            self.pushed = self.game.inbox().len();
            return;
        }
        let inbox = self.game.inbox();
        let new: Vec<(pocket_sim::EventSeq, Arc<[u8]>)> = inbox[self.pushed.min(inbox.len())..]
            .iter()
            .map(|ev| {
                let bytes = pocket_persist::pce::to_bytes(ev).unwrap_or_default();
                (ev.seq, Arc::from(bytes))
            })
            .collect();
        self.pushed = inbox.len();
        self.publisher.events(new);
    }

    /// Moves the scripts' console lines and the tick's failed invocations into the log stream.
    fn push_log(&mut self, errors: Option<&[Problem]>) {
        let tick = self.game.tick().0;
        let lines = self.game.take_log();
        if !self.attached.load(Ordering::Acquire) {
            return;
        }
        let mut out: Vec<LogRecord> = lines
            .into_iter()
            .map(|l| {
                let (file, line) = l.location.as_deref().map_or((None, None), location);
                LogRecord {
                    seq: 0,
                    level: l.level,
                    source: l.system.unwrap_or_else(|| "script".into()),
                    message: l.text,
                    file,
                    line,
                    tick: Some(l.tick),
                }
            })
            .collect();
        for p in errors.unwrap_or_default() {
            let system = p.detail.get("system").and_then(Value::as_str);
            let mut r = LogRecord::new(
                "error",
                system.unwrap_or("host"),
                format!("{}: {}", p.code, p.message),
            );
            r.tick = Some(tick);
            out.push(r);
        }
        if !out.is_empty() {
            self.publisher.log(out);
        }
    }

    /// A poisoned world has no snapshot of its own (it is not at a boundary): publishes the last
    /// published world again with the halted status, when a reader is attached.
    fn publish_halted(&mut self) {
        if !self.attached.load(Ordering::Acquire) {
            return;
        }
        let last = self.publisher.latest();
        self.version += 1;
        self.publisher.publish(WorldSnapshot {
            version: self.version,
            snapshot: last.snapshot.clone(),
            registry: last.registry.clone(),
            time: time_status(&self.game, &self.model),
            last_event: self.publisher.last_event(),
            published_at_ms: self.now(),
        });
        (self.options.on_publish)();
    }

    /// Publishes the world as it is, when a reader is attached (threads.md 4.3), and the visual
    /// changes to the render feed's subscribers.
    fn publish(&mut self) {
        self.present();
        if !self.attached.load(Ordering::Acquire) {
            return;
        }
        if let Ok(snapshot) = self.game.snapshot() {
            self.publish_snapshot(snapshot);
        }
    }

    /// Hands the render feed's subscribers the visual changes (a switch of world, Play or Stop,
    /// is a new `World`, so the extractor sends a full frame).
    fn present(&mut self) {
        if let Some(feed) = &self.options.feed {
            self.game.present(&mut self.extractor, feed);
        }
    }

    fn publish_snapshot(&mut self, snapshot: Snapshot) {
        if !self.attached.load(Ordering::Acquire) {
            return;
        }
        if self.game.bundle() != self.bundle {
            self.bundle = self.game.bundle();
            self.registry = Arc::new(self.game.registry_info().unwrap_or_default());
        }
        self.version += 1;
        self.publisher.publish(WorldSnapshot {
            version: self.version,
            snapshot,
            registry: self.registry.clone(),
            time: time_status(&self.game, &self.model),
            last_event: self.publisher.last_event(),
            published_at_ms: self.now(),
        });
        (self.options.on_publish)();
    }
}

/// Takes from `held` the envelopes due at the boundary before tick `next` and those whose tick has
/// passed, leaving the later ones held (threads.md 5.2: held envelopes are matched by tick like
/// queued ones, so one whose tick a replaced world skipped is answered rather than stranded).
fn take_held(held: &mut Vec<Envelope>, next: Tick) -> (Vec<Envelope>, Vec<Envelope>) {
    let mut due = Vec::new();
    let mut passed = Vec::new();
    for e in mem::take(held) {
        match e.at {
            Some(t) if t > next => held.push(e),
            Some(t) if t < next => passed.push(e),
            _ => due.push(e),
        }
    }
    (due, passed)
}

/// `game.stopped` when the thread could not be reached; used by callers that want the reason.
pub fn stopped_detail(reader: &SnapshotReader) -> Problem {
    reader
        .stop_reason()
        .unwrap_or_else(|| Problem::new("game.stopped", "The game has stopped.", detail([])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(seq: u64, at: Option<u64>) -> Envelope {
        Envelope {
            source: Source::Player(0),
            seq,
            at: at.map(Tick),
            name: "status".into(),
            params: json!({}),
            reply: ReplyTo::none(),
        }
    }

    /// threads.md 5.2: at the boundary before tick 10, a held envelope of tick 10 is due, one of a
    /// later tick stays held, and one of an earlier tick, which a world replaced at a later tick
    /// would otherwise strand, is taken out to be answered `command.tick_passed`.
    #[test]
    fn held_envelopes_whose_tick_passed_are_taken_out() {
        let mut held = vec![
            envelope(1, Some(4)),
            envelope(2, Some(10)),
            envelope(3, Some(12)),
            envelope(4, Some(9)),
        ];
        let (due, passed) = take_held(&mut held, Tick(10));
        let seqs = |v: &[Envelope]| v.iter().map(|e| e.seq).collect::<Vec<_>>();
        assert_eq!(seqs(&due), [2]);
        assert_eq!(seqs(&passed), [1, 4]);
        assert_eq!(seqs(&held), [3]);
        let (due, passed) = take_held(&mut held, Tick(12));
        assert_eq!(
            (seqs(&due), seqs(&passed), held.len()),
            (vec![3], vec![], 0)
        );
    }
}
