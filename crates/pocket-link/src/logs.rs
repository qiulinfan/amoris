//! The log stream (docs/spec/server.md, `log`): script `console.*` lines, failed system
//! invocations and the host's own messages, in a ring beside the event stream, so a presenter that
//! reads slower than the game ticks loses none silently. Presentation data: never part of the world
//! or its hash.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// The ring's size (chosen, not measured).
pub const LOG_RECORDS: usize = 4096;

/// One log line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    /// Stream number, from 1 per run.
    pub seq: u64,
    /// `log`, `info`, `warn`, `error`, `debug`.
    pub level: String,
    /// `script`, `host`, or a system's name.
    pub source: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
}

impl LogRecord {
    /// A line without a location or tick; `seq` is given by the ring.
    pub fn new(level: &str, source: &str, message: impl Into<String>) -> LogRecord {
        LogRecord {
            seq: 0,
            level: level.to_owned(),
            source: source.to_owned(),
            message: message.into(),
            file: None,
            line: None,
            tick: None,
        }
    }
}

/// The ring.
#[derive(Debug)]
pub struct LogRing {
    records: VecDeque<LogRecord>,
    capacity: usize,
    next: u64,
}

impl Default for LogRing {
    fn default() -> Self {
        LogRing {
            records: VecDeque::new(),
            capacity: LOG_RECORDS,
            next: 1,
        }
    }
}

impl LogRing {
    /// Appends a line, numbering it; the oldest goes when the ring is full.
    pub fn push(&mut self, mut r: LogRecord) -> u64 {
        r.seq = self.next;
        self.next += 1;
        if self.records.len() == self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(r);
        self.next - 1
    }

    /// Up to `max` lines with a stream number at least `*next`; `*next` moves past them.
    pub fn read(&self, next: &mut u64, max: usize) -> Vec<LogRecord> {
        let out: Vec<LogRecord> = self
            .records
            .iter()
            .filter(|r| r.seq >= *next)
            .take(max)
            .cloned()
            .collect();
        if let Some(last) = out.last() {
            *next = last.seq + 1;
        }
        out
    }
}
