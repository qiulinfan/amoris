//! The event stream (docs/spec/threads.md 4.4): every event the world emits, tick events and the
//! events boundary writes append, in an append-only ring beside the snapshots, so a reader that
//! draws slower than the game ticks skips snapshots but never events; a reader that falls more than
//! the ring behind is told how many it lost.

use std::collections::VecDeque;
use std::sync::Arc;

use pocket_sim::EventSeq;

/// The ring's size (threads.md 4.4; chosen, not measured).
pub const RING_RECORDS: usize = 65_536;

/// One event in the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRecord {
    /// Stream number, from 1 per run; a restore or fork does not reset it.
    pub seq: u64,
    /// spec-sim's world-wide sequence number (it can recur after a restore).
    pub id: EventSeq,
    /// The event in spec-persist's canonical encoding.
    pub bytes: Arc<[u8]>,
}

/// A reader's place in the stream: the next stream number it wants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventCursor {
    pub next: u64,
}

impl EventCursor {
    /// A cursor at the start of the stream.
    pub fn start() -> EventCursor {
        EventCursor { next: 1 }
    }
}

/// What a read of the stream gave: the records, and how many the reader lost because the ring had
/// moved past them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventBatch {
    pub records: Vec<EventRecord>,
    pub missed: u64,
}

/// The ring.
#[derive(Debug)]
pub struct EventRing {
    records: VecDeque<EventRecord>,
    capacity: usize,
    next: u64,
}

impl Default for EventRing {
    fn default() -> Self {
        EventRing::with_capacity(RING_RECORDS)
    }
}

impl EventRing {
    pub fn with_capacity(capacity: usize) -> EventRing {
        EventRing {
            records: VecDeque::new(),
            capacity: capacity.max(1),
            next: 1,
        }
    }

    /// Appends an event; the oldest goes when the ring is full. Returns its stream number.
    pub fn push(&mut self, id: EventSeq, bytes: Arc<[u8]>) -> u64 {
        let seq = self.next;
        self.next += 1;
        if self.records.len() == self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(EventRecord { seq, id, bytes });
        seq
    }

    /// The stream number of the last event appended, 0 before any.
    pub fn last(&self) -> u64 {
        self.next - 1
    }

    /// Up to `max` records from the cursor on; the cursor moves past them. A cursor before the
    /// oldest record held gets the oldest ones and `missed`, the count it lost.
    pub fn read(&self, cursor: &mut EventCursor, max: usize) -> EventBatch {
        let oldest = self.records.front().map_or(self.next, |r| r.seq);
        let from = cursor.next.max(1);
        let missed = oldest.saturating_sub(from);
        let start = from.max(oldest);
        let skip = usize::try_from(start - oldest).unwrap_or(usize::MAX);
        let records: Vec<EventRecord> = self.records.iter().skip(skip).take(max).cloned().collect();
        cursor.next = records.last().map_or(start, |r| r.seq + 1);
        EventBatch { records, missed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(n: u64) -> Arc<[u8]> {
        Arc::from(n.to_le_bytes().to_vec())
    }

    /// threads.md 11, events: a reader that falls `n` records past the ring gets exactly
    /// `missed = n`.
    #[test]
    fn a_reader_past_the_ring_is_told_what_it_missed() {
        let mut ring = EventRing::with_capacity(8);
        let mut early = EventCursor::start();
        for i in 0..5 {
            ring.push(EventSeq(i + 1), bytes(i));
        }
        let b = ring.read(&mut early, 100);
        assert_eq!((b.records.len(), b.missed), (5, 0));
        assert_eq!(early.next, 6);
        let mut late = EventCursor::start();
        for n in [1u64, 3, 13] {
            let mut ring = EventRing::with_capacity(8);
            for i in 0..(8 + n) {
                ring.push(EventSeq(i + 1), bytes(i));
            }
            let mut c = late;
            let b = ring.read(&mut c, 100);
            assert_eq!(b.missed, n, "fell {n} past");
            assert_eq!(b.records.len(), 8);
            assert_eq!(b.records[0].seq, n + 1);
            assert_eq!(c.next, 8 + n + 1);
            let again = ring.read(&mut c, 100);
            assert_eq!((again.records.len(), again.missed), (0, 0));
        }
        late.next = 0;
        assert_eq!(ring.read(&mut late, 2).records.len(), 2);
        assert_eq!(ring.last(), 5);
    }
}
