//! The command queue and the clients that send into it (docs/spec/threads.md 5.1 and 5.2): a
//! multi-producer, single-consumer channel whose capacity counts both queued envelopes and those the
//! game thread holds for a later tick, so producers never block and a full queue answers
//! `queue.full`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use pocket_contract::Problem;
use pocket_sim::Tick;
use serde_json::Value;

use crate::command::{Envelope, Reply, ReplyTo, Source, game_stopped, queue_full};

/// The queue's capacity (threads.md 5.2; chosen, not measured).
pub const QUEUE_CAPACITY: usize = 4096;

/// The producing side of the queue; cloned into every client.
#[derive(Clone)]
pub struct QueueSender {
    tx: mpsc::Sender<Envelope>,
    count: Arc<AtomicUsize>,
    capacity: usize,
}

/// The game thread's side of the queue.
pub struct QueueReceiver {
    rx: mpsc::Receiver<Envelope>,
    count: Arc<AtomicUsize>,
}

/// A new queue of `capacity` envelopes, queued and held together.
pub fn queue(capacity: usize) -> (QueueSender, QueueReceiver) {
    let (tx, rx) = mpsc::channel();
    let count = Arc::new(AtomicUsize::new(0));
    (
        QueueSender {
            tx,
            count: count.clone(),
            capacity,
        },
        QueueReceiver { rx, count },
    )
}

impl QueueSender {
    /// Enqueues without blocking: `queue.full` when the queue holds its capacity, `game.stopped`
    /// when the game thread is gone.
    pub fn push(&self, env: Envelope) -> Result<(), Problem> {
        let taken = self.count.fetch_add(1, Ordering::AcqRel);
        if taken >= self.capacity {
            self.count.fetch_sub(1, Ordering::AcqRel);
            return Err(queue_full(self.capacity));
        }
        self.tx.send(env).map_err(|_| {
            self.count.fetch_sub(1, Ordering::AcqRel);
            game_stopped("the game thread has ended", None)
        })
    }

    /// Enqueues past the capacity: only the runtime's own shutdown, which a full queue must not
    /// keep out (threads.md 8).
    pub fn push_unbounded(&self, env: Envelope) -> Result<(), Problem> {
        self.count.fetch_add(1, Ordering::AcqRel);
        self.tx.send(env).map_err(|_| {
            self.count.fetch_sub(1, Ordering::AcqRel);
            game_stopped("the game thread has ended", None)
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// What a wait on the queue gave.
pub enum Received {
    One(Envelope),
    Empty,
    /// Every sender is gone.
    Closed,
}

impl QueueReceiver {
    /// The next envelope if one is queued.
    pub fn try_next(&self) -> Received {
        match self.rx.try_recv() {
            Ok(e) => Received::One(e),
            Err(mpsc::TryRecvError::Empty) => Received::Empty,
            Err(mpsc::TryRecvError::Disconnected) => Received::Closed,
        }
    }

    /// Waits for an envelope.
    pub fn next(&self) -> Received {
        match self.rx.recv() {
            Ok(e) => Received::One(e),
            Err(_) => Received::Closed,
        }
    }

    /// Waits for an envelope at most `timeout`.
    pub fn next_within(&self, timeout: Duration) -> Received {
        match self.rx.recv_timeout(timeout) {
            Ok(e) => Received::One(e),
            Err(mpsc::RecvTimeoutError::Timeout) => Received::Empty,
            Err(mpsc::RecvTimeoutError::Disconnected) => Received::Closed,
        }
    }

    /// `n` envelopes left the queue for good (applied, refused or answered): their places are
    /// free again. Held envelopes keep theirs until then.
    pub fn done(&self, n: usize) {
        self.count.fetch_sub(n, Ordering::AcqRel);
    }

    /// Queued and held envelopes.
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Released when a client drops: its source can be handed out again.
pub struct Lease(Option<Box<dyn FnOnce() + Send + 'static>>);

impl Lease {
    pub fn new(release: impl FnOnce() + Send + 'static) -> Lease {
        Lease(Some(Box::new(release)))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// One source's way to send commands (threads.md 5.1). `Send`, not `Clone`; `send` takes `&mut
/// self`, so the source's sequence numbers follow its own order of sending. The next seq lives in a
/// counter the source's clients share one after another (one live client per source), so a client
/// handed out again continues the source's sequence instead of starting at 1.
pub struct GameClient {
    source: Source,
    next: Arc<AtomicU64>,
    queue: QueueSender,
    _lease: Option<Lease>,
}

impl std::fmt::Debug for GameClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameClient")
            .field("source", &self.source)
            .field("next", &self.next.load(Ordering::Acquire))
            .finish()
    }
}

impl GameClient {
    /// A client of `source` sending into `queue`. `next` is the source's next seq (1 for a source
    /// never handed out), shared with the source's earlier and later clients and with no live one
    /// beside this; `lease` is released when it drops.
    pub fn new(
        source: Source,
        next: Arc<AtomicU64>,
        queue: QueueSender,
        lease: Option<Lease>,
    ) -> GameClient {
        GameClient {
            source,
            next,
            queue,
            _lease: lease,
        }
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// Enqueues without blocking and returns the command's seq; `queue.full` when the queue holds
    /// its capacity (nothing is sent and the seq is not used).
    pub fn send(
        &mut self,
        name: &str,
        params: Value,
        at: Option<Tick>,
        reply: impl FnOnce(Reply) + Send + 'static,
    ) -> Result<u64, Problem> {
        let seq = self.next.fetch_add(1, Ordering::AcqRel);
        let pushed = self.queue.push(Envelope {
            source: self.source,
            seq,
            at,
            name: name.to_owned(),
            params,
            reply: ReplyTo::new(reply),
        });
        if let Err(p) = pushed {
            // No other client of this source is alive, so the seq is still the last one taken.
            self.next.fetch_sub(1, Ordering::AcqRel);
            return Err(p);
        }
        Ok(seq)
    }

    /// Sends and blocks until the reply (the CLI, tests, the check). Never call it on a presenter
    /// thread: a stalled game would freeze it (threads.md 5.1).
    pub fn call(&mut self, name: &str, params: Value) -> Reply {
        self.call_at(name, params, None)
    }

    /// [`GameClient::call`] for an input of tick `at`.
    pub fn call_at(&mut self, name: &str, params: Value, at: Option<Tick>) -> Reply {
        let (tx, rx) = mpsc::channel();
        self.send(name, params, at, move |r| {
            let _ = tx.send(r);
        })?;
        rx.recv()
            .unwrap_or_else(|_| Err(game_stopped("the game ended before it answered", None)))
    }
}
