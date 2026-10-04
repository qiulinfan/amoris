//! Publication and reading (docs/spec/threads.md 3.5, 4.3 and 4.4): the game thread swaps each new
//! snapshot into a slot whose readers take no lock, appends events to the stream and keeps its
//! status in atomics; a reader takes the latest snapshot, waits for a newer one, reads its own
//! cursor of the stream and the status.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use pocket_sim::Tick;

use crate::events::{EventBatch, EventCursor, EventRing};
use crate::snapshot::WorldSnapshot;

/// What the game thread is doing (threads.md 3.5; not spec-sim's `TickPhase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopState {
    Waiting,
    Applying,
    Ticking,
    Breakpoint,
    Stopped,
}

impl LoopState {
    fn code(self) -> u8 {
        match self {
            LoopState::Waiting => 0,
            LoopState::Applying => 1,
            LoopState::Ticking => 2,
            LoopState::Breakpoint => 3,
            LoopState::Stopped => 4,
        }
    }

    fn of(code: u8) -> LoopState {
        match code {
            1 => LoopState::Applying,
            2 => LoopState::Ticking,
            3 => LoopState::Breakpoint,
            4 => LoopState::Stopped,
            _ => LoopState::Waiting,
        }
    }
}

/// The game's status: presentation data, never part of the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameStatus {
    pub state: LoopState,
    /// The tick running, or the last one that ran.
    pub tick: Tick,
    /// How long the game has been in `state`, on the loop's clock.
    pub since_ms: f64,
}

struct Shared {
    slot: ArcSwap<WorldSnapshot>,
    version: Mutex<u64>,
    newer: Condvar,
    state: AtomicU8,
    tick: AtomicU64,
    since: AtomicU64,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    events: Mutex<EventRing>,
    stopped: Mutex<Option<pocket_contract::Problem>>,
}

/// The game thread's side: publishes snapshots, events and status.
pub struct Publisher {
    shared: Arc<Shared>,
}

/// A presenter's side; cheap to clone.
#[derive(Clone)]
pub struct SnapshotReader {
    shared: Arc<Shared>,
}

/// A publisher and its reader around the first snapshot. `clock` is the loop's clock in
/// milliseconds (threads.md 3.1), which `since_ms` is measured on.
pub fn publication(
    first: WorldSnapshot,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
) -> (Publisher, SnapshotReader) {
    let now = clock();
    let tick = first.snapshot.header().tick.0;
    let version = first.version;
    let shared = Arc::new(Shared {
        slot: ArcSwap::from_pointee(first),
        version: Mutex::new(version),
        newer: Condvar::new(),
        state: AtomicU8::new(LoopState::Waiting.code()),
        tick: AtomicU64::new(tick),
        since: AtomicU64::new(now.to_bits()),
        clock,
        events: Mutex::new(EventRing::default()),
        stopped: Mutex::new(None),
    });
    (
        Publisher {
            shared: shared.clone(),
        },
        SnapshotReader { shared },
    )
}

impl Publisher {
    /// Swaps `snap` in (it never waits for a reader) and wakes those waiting for a newer one.
    pub fn publish(&self, snap: WorldSnapshot) {
        let version = snap.version;
        self.shared.slot.store(Arc::new(snap));
        if let Ok(mut v) = self.shared.version.lock() {
            *v = version;
        }
        self.shared.newer.notify_all();
    }

    /// The version of the latest publication.
    pub fn version(&self) -> u64 {
        self.shared.slot.load().version
    }

    /// The latest publication (to publish its world again with a new status).
    pub fn latest(&self) -> Arc<WorldSnapshot> {
        self.shared.slot.load_full()
    }

    /// Records a change of state at the loop's clock.
    pub fn set_state(&self, state: LoopState, tick: Tick) {
        let s = &self.shared;
        if s.state.swap(state.code(), Ordering::AcqRel) != state.code() {
            s.since.store((s.clock)().to_bits(), Ordering::Release);
        }
        s.tick.store(tick.0, Ordering::Release);
    }

    /// Appends events to the stream; returns the last stream number.
    pub fn events(
        &self,
        items: impl IntoIterator<Item = (pocket_sim::EventSeq, Arc<[u8]>)>,
    ) -> u64 {
        let Ok(mut ring) = self.shared.events.lock() else {
            return 0;
        };
        for (id, bytes) in items {
            ring.push(id, bytes);
        }
        ring.last()
    }

    /// The stream number of the last event.
    pub fn last_event(&self) -> u64 {
        self.shared.events.lock().map_or(0, |r| r.last())
    }

    /// The game stopped: the status says so and readers waiting for a newer snapshot wake.
    pub fn stopped(&self, tick: Tick, why: Option<pocket_contract::Problem>) {
        if let Ok(mut s) = self.shared.stopped.lock() {
            *s = why;
        }
        self.set_state(LoopState::Stopped, tick);
        self.shared.newer.notify_all();
    }
}

impl SnapshotReader {
    /// The latest publication; keep it as long as you like.
    pub fn latest(&self) -> Arc<WorldSnapshot> {
        self.shared.slot.load_full()
    }

    /// Blocks until a publication newer than `version` or the timeout (tests, headless clients).
    pub fn wait_newer(&self, version: u64, timeout_ms: u64) -> Option<Arc<WorldSnapshot>> {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut v = self.shared.version.lock().ok()?;
        while *v <= version {
            if self.status().state == LoopState::Stopped {
                return None;
            }
            let left = deadline.checked_duration_since(std::time::Instant::now())?;
            let (next, timeout) = self.shared.newer.wait_timeout(v, left).ok()?;
            v = next;
            if timeout.timed_out() && *v <= version {
                return None;
            }
        }
        drop(v);
        Some(self.latest())
    }

    /// The game's status, read without a lock.
    pub fn status(&self) -> GameStatus {
        let s = &self.shared;
        let state = LoopState::of(s.state.load(Ordering::Acquire));
        let since = f64::from_bits(s.since.load(Ordering::Acquire));
        GameStatus {
            state,
            tick: Tick(s.tick.load(Ordering::Acquire)),
            since_ms: ((s.clock)() - since).max(0.0),
        }
    }

    /// Why the game stopped, once it has.
    pub fn stop_reason(&self) -> Option<pocket_contract::Problem> {
        self.shared.stopped.lock().ok().and_then(|s| s.clone())
    }

    /// Up to `max` events from the cursor on (threads.md 4.4).
    pub fn events(&self, cursor: &mut EventCursor, max: usize) -> EventBatch {
        self.shared
            .events
            .lock()
            .map(|r| r.read(cursor, max))
            .unwrap_or_default()
    }
}
