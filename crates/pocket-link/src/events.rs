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

impl EventRecord {
    /// The event as JSON (docs/spec/server.md, `events`): `{seq, id, tick, name, subject, cause,
    /// data}`, `seq` the stream number and `id` and `cause` world-wide sequence numbers.
    pub fn to_json(&self) -> Result<serde_json::Value, pocket_contract::Problem> {
        let e: pocket_sim::Event =
            pocket_persist::pce::from_bytes(&self.bytes, true).map_err(|e| e.problem(0))?;
        Ok(serde_json::json!({
            "seq": self.seq,
            "id": e.seq.0,
            "tick": e.tick.0,
            "name": e.kind.as_str(),
            "subject": e.subject.map(|s| s.get()),
            "cause": e.cause.map(|c| c.0),
            "data": e.data.to_json(),
        }))
    }
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

/// `events.since` over a reader's stream (docs/spec/server.md): with `after`, the first `limit`
/// events whose stream number is above it; without, the last `limit`. `name` keeps one event name,
/// or a prefix written `crate.*`. Returns `{events, last, missed}`; `last` is the stream number of
/// the newest event, the cursor for the next call.
pub fn events_since(
    reader: &crate::SnapshotReader,
    after: Option<u64>,
    limit: usize,
    name: Option<&str>,
) -> serde_json::Value {
    let mut cursor = EventCursor {
        next: after.map_or(1, |a| a + 1),
    };
    let batch = reader.events(&mut cursor, usize::MAX);
    let matches = |v: &serde_json::Value| match name {
        None => true,
        Some(n) => {
            let got = v["name"].as_str().unwrap_or("");
            match n.strip_suffix(".*") {
                Some(prefix) => got.strip_prefix(prefix).is_some_and(|r| r.starts_with('.')),
                None => got == n,
            }
        }
    };
    let decoded = batch.records.iter().filter_map(|r| r.to_json().ok());
    let mut events: Vec<serde_json::Value> = if after.is_some() {
        decoded.filter(matches).take(limit).collect()
    } else {
        let all: Vec<serde_json::Value> = decoded.filter(matches).collect();
        all[all.len().saturating_sub(limit)..].to_vec()
    };
    events.shrink_to_fit();
    let last = batch.records.last().map_or(after.unwrap_or(0), |r| r.seq);
    serde_json::json!({"events": events, "last": last, "missed": batch.missed})
}

/// `events.why` over a reader's stream: the event with stream number `seq` and the chain of the
/// events that caused it, nearest first, each found as the latest earlier event with the cause's
/// sequence number. `complete` is false when a cause is no longer in the ring.
pub fn events_why(
    reader: &crate::SnapshotReader,
    seq: u64,
) -> Result<serde_json::Value, pocket_contract::Problem> {
    let mut cursor = EventCursor::start();
    let batch = reader.events(&mut cursor, usize::MAX);
    let records = &batch.records;
    let Some(at) = records.iter().position(|r| r.seq == seq) else {
        let range = (
            records.first().map(|r| r.seq),
            records.last().map(|r| r.seq),
        );
        return Err(pocket_contract::Problem::new(
            "events.not_found",
            format!("No event with stream number {seq} is held; the ring holds {range:?}."),
            pocket_contract::detail([
                ("seq", serde_json::json!(seq)),
                ("oldest", serde_json::json!(range.0)),
                ("newest", serde_json::json!(range.1)),
            ]),
        ));
    };
    let event = records[at].to_json()?;
    let mut causes = Vec::new();
    let mut cause = event["cause"].as_u64();
    let mut upto = at;
    let mut complete = true;
    while let Some(c) = cause {
        match records[..upto].iter().rposition(|r| r.id.0 == c) {
            Some(i) => {
                let e = records[i].to_json()?;
                cause = e["cause"].as_u64();
                causes.push(e);
                upto = i;
            }
            None => {
                complete = false;
                break;
            }
        }
        if causes.len() >= 64 {
            break;
        }
    }
    Ok(serde_json::json!({"event": event, "causes": causes, "complete": complete}))
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
