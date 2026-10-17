//! The profile stream (docs/spec/server.md, `profile`): what each tick cost the game thread, by
//! system and phase, in a ring beside the event and log streams, timed on the loop's clock by the
//! game thread's step hooks (pocket-runtime's `profile`) while a host serves the stream. The ring
//! keeps one world's ticks: a tick of another world (Play, Stop, a restore) empties it first, and
//! a read for a world that has not ticked since it was shown finds nothing, so a summary never
//! mixes worlds or shows one world's ticks as another's. The server pushes a summary of the ticks since its last
//! push to the editor four times a second and answers `profile.frame` with one of the last ticks,
//! so the editor, the CLI and MCP read the same numbers. Presentation data: never part of the
//! world or its hash.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, TryLockError};

use serde_json::{Value, json};

/// The ring's size: ten seconds at 60 ticks a second (chosen, not measured).
pub const PROFILE_TICKS: usize = 600;

/// What one tick cost, in milliseconds of the loop's clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickProfile {
    /// Stream number, from 1 per run; given by the ring.
    pub seq: u64,
    pub tick: u64,
    /// The world's epoch (pocket-link's `WorldInfo`): the ring keeps one world's ticks.
    pub epoch: u64,
    /// The script debugger held the game thread inside this tick: its times measure the hold, so
    /// summaries count the tick and leave its times out.
    pub held: bool,
    /// The whole step (`Game::step`): the systems, the step's own work and the recorder.
    pub tick_ms: f64,
    /// Each system's own time in the order they finished: a system that runs others inside it
    /// (`script.update` runs the script systems) keeps only the time outside them and finishes
    /// after them.
    pub systems: Vec<(Arc<str>, f64)>,
    /// Each phase's time (its systems' time; the step between them is `tick_ms`'s), in order.
    pub phases: Vec<(&'static str, f64)>,
    /// The game thread's work for presenters after the tick: the render feed's extraction, and
    /// the snapshot it publishes (with the event and log streams).
    pub present_ms: f64,
    pub publish_ms: f64,
    /// The loop's work after the tick before, once that tick's profile was published: the
    /// players' decision points and `time.step`'s stop conditions and samples. It is carried into
    /// this tick's profile because the replies that work sends must not overtake the profile of
    /// the tick they answer.
    pub after_ms: f64,
}

impl TickProfile {
    /// Empties it for reuse, keeping its allocations.
    pub fn clear(&mut self) {
        self.seq = 0;
        self.tick = 0;
        self.epoch = 0;
        self.held = false;
        self.tick_ms = 0.0;
        self.systems.clear();
        self.phases.clear();
        self.present_ms = 0.0;
        self.publish_ms = 0.0;
        self.after_ms = 0.0;
    }
}

/// The ring.
#[derive(Debug)]
pub struct ProfileRing {
    ticks: VecDeque<TickProfile>,
    capacity: usize,
    next: u64,
    /// The epoch of the ticks held, and the stream number of its first tick.
    epoch: Option<u64>,
    start: u64,
}

impl Default for ProfileRing {
    fn default() -> Self {
        ProfileRing::with_capacity(PROFILE_TICKS)
    }
}

impl ProfileRing {
    pub fn with_capacity(capacity: usize) -> ProfileRing {
        ProfileRing {
            ticks: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
            next: 1,
            epoch: None,
            start: 1,
        }
    }

    /// Appends a tick, numbering it; returns the oldest when the ring was full, for the caller to
    /// fill again (the game thread allocates nothing once the ring is full). A tick of another
    /// world empties the ring first.
    pub fn push(&mut self, mut t: TickProfile) -> Option<TickProfile> {
        if self.epoch != Some(t.epoch) {
            self.ticks.clear();
            self.epoch = Some(t.epoch);
            self.start = self.next;
        }
        t.seq = self.next;
        self.next += 1;
        let old = (self.ticks.len() == self.capacity)
            .then(|| self.ticks.pop_front())
            .flatten();
        self.ticks.push_back(t);
        old
    }

    /// The stream number the next tick gets.
    pub fn next_seq(&self) -> u64 {
        self.next
    }

    /// Whether the ring holds world `epoch`'s ticks (it holds the last world's that ticked).
    fn holds(&self, epoch: u64) -> bool {
        self.epoch.is_none_or(|e| e == epoch)
    }

    /// The summary of world `epoch`'s ticks from stream number `*next` on, and `*next` moved past
    /// them; ticks of that world that left the ring before this read are counted as `missed`
    /// (another world's are not: they were dropped with it).
    pub fn since(&self, next: &mut u64, epoch: u64) -> Value {
        if !self.holds(epoch) {
            *next = self.next;
            let mut out = summary(std::iter::empty());
            out["missed"] = json!(0);
            return out;
        }
        let first = self.ticks.front().map_or(self.next, |t| t.seq);
        let missed = first.saturating_sub((*next).max(self.start));
        let from = (*next).max(first);
        let skip = usize::try_from(from - first).unwrap_or(usize::MAX);
        let mut out = summary(self.ticks.iter().skip(skip));
        out["missed"] = json!(missed);
        *next = self.next;
        out
    }

    /// The summary of world `epoch`'s last `n` ticks.
    pub fn last(&self, n: usize, epoch: u64) -> Value {
        if !self.holds(epoch) {
            return summary(std::iter::empty());
        }
        let skip = self.ticks.len().saturating_sub(n);
        summary(self.ticks.iter().skip(skip))
    }
}

/// The game thread's push (pocket-link's `Publisher::profile`): the profiles in `ready` (oldest
/// first) into `ring`, emptying `ready`, unless a reader holds the ring; then nothing waits and the
/// profiles stay in `ready` for the next tick. Returns the oldest profile the ring let go.
pub fn push_ready(ring: &Mutex<ProfileRing>, ready: &mut Vec<TickProfile>) -> Option<TickProfile> {
    let mut ring = match ring.try_lock() {
        Ok(r) => r,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return None,
    };
    let mut reuse = None;
    for t in ready.drain(..) {
        reuse = ring.push(t).or(reuse);
    }
    reuse
}

/// Milliseconds to the tenth of a microsecond.
fn ms(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Sums by name in the order the ticks list them: (name, total, largest in one tick).
#[derive(Default)]
struct Totals {
    rows: Vec<(Arc<str>, f64, f64)>,
    /// One tick's sums: (row, time).
    tick: Vec<(usize, f64)>,
}

impl Totals {
    /// Adds one tick's times, in the order the tick lists them. A name new to the summary goes
    /// right after the name listed before it in this tick (first when it leads the tick), so the
    /// rows keep the tick's order whichever tick a system first ran in. A name that recurs within
    /// the tick counts as one sum.
    fn add<'a>(&mut self, items: impl Iterator<Item = (&'a str, f64)>) {
        self.tick.clear();
        let mut before: Option<usize> = None;
        for (name, v) in items {
            let i = match self.rows.iter().position(|r| &*r.0 == name) {
                Some(i) => i,
                None => {
                    let at = before.map_or(0, |b| b + 1);
                    self.rows.insert(at, (Arc::from(name), 0.0, 0.0));
                    for s in &mut self.tick {
                        if s.0 >= at {
                            s.0 += 1;
                        }
                    }
                    at
                }
            };
            self.rows[i].1 += v;
            match self.tick.iter_mut().find(|s| s.0 == i) {
                Some(s) => s.1 += v,
                None => self.tick.push((i, v)),
            }
            before = Some(i);
        }
        for &(i, v) in &self.tick {
            let r = &mut self.rows[i];
            r.2 = r.2.max(v);
        }
    }

    fn json(&self, n: f64) -> Vec<Value> {
        self.rows
            .iter()
            .map(|(name, sum, max)| json!({"name": &**name, "ms": ms(sum / n), "max_ms": ms(*max)}))
            .collect()
    }
}

/// The summary of `ticks` (docs/spec/host-protocol.md 3, `profile`): `{tick, epoch, ticks, held,
/// tick_ms, tick_max_ms, busy_ms, systems: [{name, ms, max_ms}], phases: [{name, ms, max_ms}]}`.
/// Averages are per tick counted (a system that ran on one tick in four shows a quarter of its
/// time), maxima the largest in one tick. `systems` lists each system's own time in the order
/// they finish in a tick, then `tick.other` (the step's time outside its systems),
/// `loop.present`, `loop.publish` and `loop.after`; together they are `busy_ms`, the game
/// thread's time per tick (all but the commands it applies between ticks).
pub fn summary<'a>(ticks: impl Iterator<Item = &'a TickProfile>) -> Value {
    let mut systems = Totals::default();
    let mut phases = Totals::default();
    let mut extra = Totals::default();
    let (mut counted, mut held, mut last, mut epoch) = (0u64, 0u64, None, None);
    let (mut tick_sum, mut tick_max, mut busy_sum) = (0.0, 0.0f64, 0.0);
    for t in ticks {
        last = Some(t.tick);
        epoch = Some(t.epoch);
        if t.held {
            held += 1;
            continue;
        }
        counted += 1;
        systems.add(t.systems.iter().map(|(k, v)| (&**k, *v)));
        phases.add(t.phases.iter().map(|(k, v)| (*k, *v)));
        let own: f64 = t.systems.iter().map(|s| s.1).sum();
        let other = (t.tick_ms - own).max(0.0);
        extra.add(
            [
                ("tick.other", other),
                ("loop.present", t.present_ms),
                ("loop.publish", t.publish_ms),
                ("loop.after", t.after_ms),
            ]
            .into_iter(),
        );
        tick_sum += t.tick_ms;
        tick_max = tick_max.max(t.tick_ms);
        busy_sum += t.tick_ms + t.present_ms + t.publish_ms + t.after_ms;
    }
    let n = (counted as f64).max(1.0);
    let mut rows = systems.json(n);
    if counted > 0 {
        rows.extend(extra.json(n));
    }
    json!({
        "tick": last,
        "epoch": epoch,
        "ticks": counted,
        "held": held,
        "tick_ms": ms(tick_sum / n),
        "tick_max_ms": ms(tick_max),
        "busy_ms": ms(busy_sum / n),
        "systems": rows,
        "phases": phases.json(n),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(tick: u64, held: bool, systems: &[(&str, f64)]) -> TickProfile {
        TickProfile {
            tick,
            held,
            tick_ms: systems.iter().map(|s| s.1).sum::<f64>() + 0.5,
            systems: systems.iter().map(|(k, v)| (Arc::from(*k), *v)).collect(),
            phases: vec![("Update", 1.0)],
            present_ms: 0.25,
            publish_ms: 0.75,
            after_ms: 0.5,
            ..TickProfile::default()
        }
    }

    fn names(s: &Value) -> Vec<&str> {
        s["systems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn summaries_average_per_tick_and_leave_out_held_ticks() {
        let mut ring = ProfileRing::with_capacity(4);
        ring.push(tick(1, false, &[("a", 1.0), ("b", 2.0)]));
        ring.push(tick(2, false, &[("a", 3.0)]));
        ring.push(tick(3, true, &[("a", 100.0)]));
        let mut next = 1;
        let s = ring.since(&mut next, 0);
        assert_eq!(next, 4);
        assert_eq!(s["tick"], 3);
        assert_eq!(
            (s["ticks"].clone(), s["held"].clone()),
            (json!(2), json!(1))
        );
        assert_eq!(s["missed"], 0);
        let rows = s["systems"].as_array().unwrap();
        assert_eq!(
            names(&s),
            [
                "a",
                "b",
                "tick.other",
                "loop.present",
                "loop.publish",
                "loop.after"
            ]
        );
        // a: (1 + 3) / 2 ticks, at most 3; b ran once: 2 / 2.
        assert_eq!(
            (rows[0]["ms"].clone(), rows[0]["max_ms"].clone()),
            (json!(2.0), json!(3.0))
        );
        assert_eq!(rows[1]["ms"], 1.0);
        assert_eq!(rows[2]["ms"], 0.5);
        // tick 3.5 + 3.5 over two ticks, plus the loop's 1.5 each.
        assert_eq!(s["tick_ms"], 3.5);
        assert_eq!(s["busy_ms"], 5.0);
        let sum: f64 = rows.iter().map(|r| r["ms"].as_f64().unwrap()).sum();
        assert!((sum - 5.0).abs() < 1e-9, "{sum}");
        // Nothing new: an empty summary.
        let s = ring.since(&mut next, 0);
        assert_eq!(
            (s["ticks"].clone(), s["systems"].clone()),
            (json!(0), json!([]))
        );
    }

    #[test]
    fn a_reader_that_falls_behind_is_told_what_it_missed() {
        let mut ring = ProfileRing::with_capacity(2);
        let mut reused = 0;
        for t in 1..=5 {
            if ring.push(tick(t, false, &[("a", 1.0)])).is_some() {
                reused += 1;
            }
        }
        assert_eq!(reused, 3);
        let mut next = 1;
        let s = ring.since(&mut next, 0);
        assert_eq!(
            (s["ticks"].clone(), s["missed"].clone()),
            (json!(2), json!(3))
        );
        assert_eq!(next, 6);
        assert_eq!(ring.last(1, 0)["tick"], 5);
        assert_eq!(ring.last(10, 0)["ticks"], 2);
    }

    /// The game thread never waits for a reader: while one holds the ring, a tick's profile waits
    /// in `ready` and goes in with the next.
    #[test]
    fn a_reader_holding_the_ring_never_stops_the_game_thread() {
        let ring = Mutex::new(ProfileRing::with_capacity(2));
        let mut ready = vec![tick(1, false, &[("a", 1.0)])];
        {
            let held = ring.lock().unwrap();
            assert!(push_ready(&ring, &mut ready).is_none());
            assert_eq!(ready.len(), 1);
            drop(held);
        }
        ready.push(tick(2, false, &[("a", 1.0)]));
        ready.push(tick(3, false, &[("a", 1.0)]));
        let reused = push_ready(&ring, &mut ready);
        assert!(ready.is_empty());
        assert_eq!(
            reused.map(|t| t.tick),
            Some(1),
            "the oldest comes back for reuse"
        );
        assert_eq!(ring.lock().unwrap().last(10, 0)["tick"], 3);
    }

    /// Rows keep the order systems finish in a tick, whichever tick a system first ran in (a
    /// script system that skipped the window's first tick is not listed last).
    #[test]
    fn rows_keep_the_order_of_the_tick() {
        let mut ring = ProfileRing::with_capacity(8);
        ring.push(tick(1, false, &[("a", 1.0), ("c", 1.0)]));
        ring.push(tick(2, false, &[("a", 1.0), ("b", 1.0), ("c", 1.0)]));
        ring.push(tick(
            3,
            false,
            &[("z", 1.0), ("a", 1.0), ("c", 1.0), ("d", 1.0)],
        ));
        let s = ring.last(8, 0);
        assert_eq!(&names(&s)[..5], ["z", "a", "b", "c", "d"]);
        // A name twice in one tick is one sum there: its largest tick is that sum.
        let mut ring = ProfileRing::with_capacity(8);
        ring.push(tick(1, false, &[("a", 1.0), ("b", 1.0), ("a", 2.0)]));
        let s = ring.last(8, 0);
        assert_eq!(&names(&s)[..2], ["a", "b"]);
        assert_eq!(s["systems"][0]["max_ms"], 3.0);
    }

    /// A tick of another world empties the ring: summaries never mix the edit world with Play's,
    /// and the ticks dropped with the old world are not counted as missed.
    #[test]
    fn a_new_world_starts_the_ring_again() {
        let mut ring = ProfileRing::with_capacity(4);
        let mut next = ring.next_seq();
        for t in 1..=3 {
            ring.push(tick(t, false, &[("edit", 1.0)]));
        }
        // Play is shown before its first tick: nothing of it yet, and the edit world's ticks
        // are not Play's.
        assert_eq!(ring.last(10, 1)["ticks"], 0);
        assert_eq!(ring.last(10, 0)["ticks"], 3);
        for t in 1..=2 {
            let mut p = tick(t, false, &[("play", 1.0)]);
            p.epoch = 1;
            ring.push(p);
        }
        let s = ring.since(&mut next, 1);
        assert_eq!(
            (s["ticks"].clone(), s["missed"].clone(), s["epoch"].clone()),
            (json!(2), json!(0), json!(1))
        );
        assert_eq!(names(&s)[0], "play");
        assert_eq!(ring.last(10, 1)["ticks"], 2);
        assert_eq!(ring.last(10, 1)["tick"], 2);
        // Stop shows the edit world again (epoch 2): until it ticks, nothing.
        let s = ring.since(&mut next, 2);
        assert_eq!(
            (s["ticks"].clone(), s["missed"].clone()),
            (json!(0), json!(0))
        );
    }
}
