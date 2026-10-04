//! The game thread (docs/spec/threads.md 3, 4.3 and 5; native, feature `thread`): a [`Game`] built
//! on its own thread, which alone steps the tick and mutates the world. Presenters and agents read
//! the snapshots it publishes after every tick and every boundary with a Write, and send commands
//! through clients; at each boundary the loop takes what is queued and what was held for this tick,
//! applies it in the canonical order, asks the time model what next, and runs a tick, waits for its
//! clock, waits for a command or quits. The wall clock is the injected one, read only here.

use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use pocket_contract::{Problem, detail};
use pocket_interface::{Pace, TimeModel};
use pocket_link::{
    Envelope, GameClient, Lease, LoopState, PacingStatus, Publisher, QUEUE_CAPACITY, QueueReceiver,
    QueueSender, Received, RegistryInfo, ReplyTo, ReplyValue, SnapshotReader, Source, TimeStatus,
    WorldSnapshot, canonical_order, game_stopped, publication, queue, source_in_use, tick_passed,
};
use pocket_sim::{ContentHash, Tick};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::catalog::Command;
use crate::game::{Game, StepParams};

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
}

impl ThreadOptions {
    /// Stepped pacing, the script host's thread stack, no wake-up.
    pub fn new(clock: Clock) -> ThreadOptions {
        ThreadOptions {
            stack_bytes: GAME_STACK_BYTES,
            clock,
            on_publish: Arc::new(|| {}),
            pacing: Pacing::Stepped,
        }
    }
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
    sources: Arc<Mutex<BTreeMap<Source, SourceSlot>>>,
    next_developer: AtomicU32,
    host_seq: AtomicU64,
    join: Option<JoinHandle<()>>,
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
                let model = TimeModel::new(game.sim().clock().rate, options.pacing);
                let first = match first_snapshot(&mut game, &model, &options) {
                    Ok(s) => s,
                    Err(p) => {
                        let _ = ready_tx.send(Err(p));
                        return;
                    }
                };
                let (publisher, reader) = publication(first, options.clock.clone());
                let registry = Arc::new(game.registry_info().unwrap_or_default());
                if ready_tx.send(Ok(reader)).is_err() {
                    return;
                }
                let mut l = Loop {
                    bundle: game.bundle(),
                    game,
                    rx,
                    publisher,
                    model,
                    options,
                    held: Vec::new(),
                    pending: VecDeque::new(),
                    steps: VecDeque::new(),
                    errors: Vec::new(),
                    attached: seen,
                    version: 1,
                    pushed: 0,
                    registry,
                };
                l.run();
            })
            .map_err(|e| game_stopped(&format!("the game thread did not start: {e}"), None))?;
        let reader = ready_rx
            .recv()
            .unwrap_or_else(|_| Err(game_stopped("the game thread ended while starting", None)))?;
        Ok(GameHandle {
            queue: tx,
            reader,
            attached,
            sources: Arc::new(Mutex::new(BTreeMap::new())),
            next_developer: AtomicU32::new(0),
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

impl GameHandle {
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

struct Loop {
    game: Game,
    rx: QueueReceiver,
    publisher: Publisher,
    model: TimeModel,
    options: ThreadOptions,
    /// Envelopes for a later tick.
    held: Vec<Envelope>,
    /// Envelopes a wait received, for the next batch.
    pending: VecDeque<Envelope>,
    /// `step` requests: ticks still to run, and where the answer goes.
    steps: VecDeque<(u64, ReplyTo)>,
    /// Failed invocations of the ticks the current `step` ran (at most 20).
    errors: Vec<Problem>,
    attached: Arc<AtomicBool>,
    version: u64,
    /// Inbox events already in the stream at this boundary.
    pushed: usize,
    registry: Arc<RegistryInfo>,
    bundle: ContentHash,
}

enum Next {
    Go,
    Quit,
}

impl Loop {
    fn now(&self) -> f64 {
        (self.options.clock)()
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
        for (_, r) in self.steps.drain(..) {
            r.send(Err(why.clone()));
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
        let (due, later): (Vec<Envelope>, Vec<Envelope>) =
            self.held.drain(..).partition(|e| e.at == Some(next));
        self.held = later;
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
        let mut quit = false;
        for e in batch {
            self.rx.done(1);
            quit |= self.handle(e);
        }
        let wrote = self.game.writes() != writes;
        self.push_events();
        if quit {
            return Next::Quit;
        }
        let now = self.now();
        let pace = self.model.pace(now);
        if wrote && pace != Pace::RunTick {
            self.publish();
        }
        match pace {
            Pace::RunTick => self.tick(),
            Pace::WaitUntil(t) => self.wait_until(t),
            Pace::WaitForCommand => {
                self.publisher
                    .set_state(LoopState::Waiting, self.game.tick());
                match self.rx.next() {
                    Received::One(e) => self.pending.push_back(e),
                    _ => return Next::Quit,
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
        let r = self.game.step();
        let now = self.now();
        self.model.ran_tick(now);
        self.pushed = 0;
        self.push_events();
        match r {
            Ok(report) => {
                if !self.steps.is_empty() {
                    let room = 20usize.saturating_sub(self.errors.len());
                    self.errors.extend(report.errors.into_iter().take(room));
                }
                self.publish();
                if let Some((left, _)) = self.steps.front_mut() {
                    *left -= 1;
                    if *left == 0
                        && let Some((_, reply)) = self.steps.pop_front()
                    {
                        reply.send(self.answer_step());
                    }
                }
            }
            Err(p) => {
                self.model.cancel_steps();
                self.errors.clear();
                for (_, reply) in self.steps.drain(..) {
                    reply.send(Err(p.clone()));
                }
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

    /// A finished `step`: the tick, the hash and the failed invocations of its ticks.
    fn answer_step(&mut self) -> Result<ReplyValue, Problem> {
        let hash = self.game.world_hash()?;
        let errors = std::mem::take(&mut self.errors);
        Ok(ReplyValue::Json(json!({
            "tick": self.game.tick().0,
            "world_hash": hash.to_string(),
            "errors": errors,
        })))
    }

    /// Applies one command; `true` when it asks the loop to quit.
    fn handle(&mut self, e: Envelope) -> bool {
        match e.name.as_str() {
            "shutdown" if e.source == Source::Host => {
                self.model.quit();
                e.reply.send(Ok(ReplyValue::Json(json!({}))));
                return true;
            }
            "step" => match crate::decode::<StepParams>(&e.params, "step") {
                Ok(p) if p.ticks == 0 => e.reply.send(self.answer_step()),
                Ok(p) => match self.game.sim().poisoned() {
                    Some(poison) => e
                        .reply
                        .send(Err(pocket_sim::sim::world_poisoned(poison.tick))),
                    None => {
                        self.model.step(p.ticks);
                        self.steps.push_back((p.ticks, e.reply));
                    }
                },
                Err(p) => e.reply.send(Err(p)),
            },
            "time_control" => {
                let r = crate::decode::<TimeControl>(&e.params, "time_control").and_then(|p| {
                    if let Some(pacing) = p.pacing {
                        pacing.check()?;
                        self.model.set_pacing(pacing);
                    }
                    match p.pause {
                        Some(true) => self.model.pause(),
                        Some(false) => self.model.resume(),
                        None => {}
                    }
                    Ok(ReplyValue::Json(
                        serde_json::to_value(time_status(&self.game, &self.model))
                            .unwrap_or(Value::Null),
                    ))
                });
                e.reply.send(r);
            }
            "snapshot" => {
                let r = self.game.snapshot().map(ReplyValue::Snapshot);
                e.reply.send(r);
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

    /// Publishes the world as it is, when a reader is attached (threads.md 4.3).
    fn publish(&mut self) {
        if !self.attached.load(Ordering::Acquire) {
            return;
        }
        let Ok(snapshot) = self.game.snapshot() else {
            return;
        };
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

/// `game.stopped` when the thread could not be reached; used by callers that want the reason.
pub fn stopped_detail(reader: &SnapshotReader) -> Problem {
    reader
        .stop_reason()
        .unwrap_or_else(|| Problem::new("game.stopped", "The game has stopped.", detail([])))
}
