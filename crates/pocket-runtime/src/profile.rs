//! The tick profiler (docs/spec/server.md, `profile`; threads.md 3.1): step hooks that time every
//! system of a tick on a clock the caller injects (the game thread's loop clock), outside the
//! simulation, into pocket-link's [`TickProfile`]; a phase's time is its systems'. The game thread
//! profiles each tick while a host serves the profile stream (`SnapshotReader::serve_profile`) and
//! publishes the result in the profile ring; benchmarks use the same timer through
//! [`Game::step_with`](crate::Game::step_with).
//!
//! Most of the cost is the clock: two readings per system (docs/bench/profiler.md). Phases read
//! none, and a system's name is found by the order the systems ended in the tick before, without a
//! lookup.

use std::sync::Arc;

use pocket_link::TickProfile;
use pocket_sim::{StepHooks, SystemKey, TickPhase};

/// A clock in milliseconds, monotonic (the loop's, threads.md 3.1).
pub type ProfileClock = Arc<dyn Fn() -> f64 + Send + Sync>;

/// Times the phases and systems of one tick at a time.
pub struct TickTimer {
    clock: ProfileClock,
    /// Open systems (a script system runs inside `script.update`): when each began, and the time
    /// of the systems that ran inside it so far.
    open: Vec<(f64, f64)>,
    /// The systems' names in the order they last ended, and where the next is expected.
    names: Vec<(SystemKey, Arc<str>)>,
    next: usize,
    tick: TickProfile,
}

impl TickTimer {
    pub fn new(clock: ProfileClock) -> TickTimer {
        TickTimer {
            clock,
            open: Vec::new(),
            names: Vec::new(),
            next: 0,
            tick: TickProfile::default(),
        }
    }

    /// The loop's clock now.
    pub fn now(&self) -> f64 {
        (self.clock)()
    }

    /// Starts a tick, filling `reuse` (a profile the ring gave back) when there is one.
    pub fn begin(&mut self, reuse: Option<TickProfile>) {
        self.open.clear();
        self.next = 0;
        self.tick = reuse.unwrap_or_default();
        self.tick.clear();
    }

    /// The tick being timed (the game thread adds the step's and its own times).
    pub fn tick_mut(&mut self) -> &mut TickProfile {
        &mut self.tick
    }

    /// Takes the tick timed since [`TickTimer::begin`].
    pub fn take(&mut self) -> TickProfile {
        std::mem::take(&mut self.tick)
    }

    /// The interned name of `key`: systems end in the same order every tick, so the expected one
    /// almost always matches; otherwise a scan, or a new name.
    fn name(&mut self, key: &SystemKey) -> Arc<str> {
        let at = match self.names.get(self.next) {
            Some((k, _)) if k == key => self.next,
            _ => match self.names.iter().position(|(k, _)| k == key) {
                Some(i) => i,
                None => {
                    self.names.push((key.clone(), Arc::from(key.as_str())));
                    self.names.len() - 1
                }
            },
        };
        self.next = at + 1;
        self.names[at].1.clone()
    }
}

impl StepHooks for TickTimer {
    fn phase(&mut self, phase: TickPhase, begin: bool) {
        if begin {
            self.tick.phases.push((phase.name(), 0.0));
        }
    }

    fn system(&mut self, key: &SystemKey, begin: bool) {
        let now = self.now();
        if begin {
            self.open.push((now, 0.0));
            return;
        }
        let Some((began, inner)) = self.open.pop() else {
            return;
        };
        let elapsed = now - began;
        match self.open.last_mut() {
            Some(outer) => outer.1 += elapsed,
            None => {
                if let Some(p) = self.tick.phases.last_mut() {
                    p.1 += elapsed;
                }
            }
        }
        let name = self.name(key);
        self.tick.systems.push((name, elapsed - inner));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A clock that moves one millisecond each time it is read.
    fn ticking() -> ProfileClock {
        let n = Arc::new(AtomicU64::new(0));
        Arc::new(move || {
            let v = n.fetch_add(1, Ordering::AcqRel);
            f64::from(u32::try_from(v).unwrap_or(u32::MAX))
        })
    }

    #[test]
    fn nested_systems_keep_their_own_time() {
        let mut t = TickTimer::new(ticking());
        let outer = SystemKey::new("script.update").unwrap();
        let inner = SystemKey::new("script:helm").unwrap();
        t.begin(None);
        t.phase(TickPhase::Update, true);
        t.system(&outer, true); // 0
        t.system(&inner, true); // 1
        t.system(&inner, false); // 2: inner 1 ms
        t.system(&outer, false); // 3: outer 3 ms, 2 of its own
        t.phase(TickPhase::Update, false);
        let p = t.take();
        let got: Vec<(&str, f64)> = p.systems.iter().map(|(k, v)| (&**k, *v)).collect();
        assert_eq!(got, [("script:helm", 1.0), ("script.update", 2.0)]);
        // The phase is its systems' time.
        assert_eq!(p.phases, [("Update", 3.0)]);
        // A reused profile comes back empty.
        t.begin(Some(p));
        assert!(t.take().systems.is_empty());
    }

    /// Names follow the keys when a system skips a tick or a new one appears.
    #[test]
    fn names_follow_the_keys_whatever_the_order() {
        let mut t = TickTimer::new(ticking());
        let key = |k: &str| SystemKey::new(k).unwrap();
        let mut run = |keys: &[&str]| {
            t.begin(None);
            for k in keys {
                t.system(&key(k), true);
                t.system(&key(k), false);
            }
            let p = t.take();
            p.systems
                .iter()
                .map(|(k, _)| k.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(run(&["a", "b", "c"]), ["a", "b", "c"]);
        assert_eq!(run(&["a", "c"]), ["a", "c"]);
        assert_eq!(run(&["c", "d", "a", "b"]), ["c", "d", "a", "b"]);
        assert_eq!(run(&["a", "b", "c"]), ["a", "b", "c"]);
    }
}
