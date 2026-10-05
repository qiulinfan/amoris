//! Commands (docs/spec/threads.md 5): who sends them, their envelope, what kind each is, the reply,
//! the canonical order a boundary applies a batch in, and the errors of the queue.

use pocket_contract::{Problem, detail};
use pocket_persist::Snapshot;
use pocket_sim::Tick;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use pocket_persist::Source;

/// What a command does to the world (threads.md 2). The catalog says which a command is, never the
/// sender.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Never changes the world.
    Read,
    /// May change the world; recorded when it succeeds.
    Write,
    /// Changes when ticks run, not what they compute (`step`, `time_control`).
    Control,
    /// Routed to a worker (a hot update's stages), answered when the work is done; the Host Write
    /// it produces is what is recorded.
    Request,
}

/// What a command answers.
pub type Reply = Result<ReplyValue, Problem>;

/// A successful answer. A fork for a worker (threads.md 5.1) never crosses through here in slice 1:
/// the runtime has no worker pool yet.
#[derive(Clone, Debug)]
pub enum ReplyValue {
    Json(Value),
    Snapshot(Snapshot),
}

impl ReplyValue {
    /// The answer as JSON: a snapshot as its tick, writes and world hash.
    pub fn into_json(self) -> Value {
        match self {
            ReplyValue::Json(v) => v,
            ReplyValue::Snapshot(s) => json!({
                "tick": s.header().tick.0,
                "writes": s.header().writes,
                "world_hash": s.world_hash().to_string(),
            }),
        }
    }
}

/// Where a reply goes: called once, on the game thread. Dropped unanswered (the game stopped), a
/// blocking [`crate::GameClient::call`] gets `game.stopped`.
pub struct ReplyTo(Option<Box<dyn FnOnce(Reply) + Send + 'static>>);

impl ReplyTo {
    pub fn new(f: impl FnOnce(Reply) + Send + 'static) -> ReplyTo {
        ReplyTo(Some(Box::new(f)))
    }

    /// A command whose answer nobody reads.
    pub fn none() -> ReplyTo {
        ReplyTo(None)
    }

    /// Answers the command.
    pub fn send(mut self, reply: Reply) {
        if let Some(f) = self.0.take() {
            f(reply);
        }
    }
}

impl std::fmt::Debug for ReplyTo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "ReplyTo(..)"
        } else {
            "ReplyTo(none)"
        })
    }
}

/// One command on its way to the game thread (threads.md 5.1).
#[derive(Debug)]
pub struct Envelope {
    pub source: Source,
    /// Per source, from 1, strictly increasing.
    pub seq: u64,
    /// An input of this tick, applied at boundary `at - 1`; `None`: the next boundary.
    pub at: Option<Tick>,
    /// A command of the runtime's catalog.
    pub name: String,
    pub params: Value,
    pub reply: ReplyTo,
}

/// Sorts a boundary's batch into the order it is applied in (threads.md 5.2): Host, then Editor,
/// then Developer sessions by number, then players by seat, each source's commands in its own
/// order. The order depends on the batch's content only, never on how producers interleaved.
pub fn canonical_order(batch: &mut [Envelope]) {
    batch.sort_by_key(|e| (e.source, e.seq));
}

/// A source as JSON: `"host"`, `"editor"`, `{"developer": n}`, `{"player": n}` (the form input
/// files and the web form use).
pub fn source_json(source: Source) -> Value {
    match source {
        Source::Host => json!("host"),
        Source::Editor => json!("editor"),
        Source::Developer(n) => json!({"developer": n}),
        Source::Player(n) => json!({"player": n}),
    }
}

/// Reads [`source_json`]'s form (`request.invalid_value` otherwise).
pub fn source_from_json(v: &Value) -> Result<Source, Problem> {
    let index = |n: &Value| n.as_u64().and_then(|n| u32::try_from(n).ok());
    let parsed = match v {
        Value::String(s) if s == "host" => Some(Source::Host),
        Value::String(s) if s == "editor" => Some(Source::Editor),
        Value::Object(m) if m.len() == 1 => match m.iter().next() {
            Some((k, n)) if k == "developer" => index(n).map(Source::Developer),
            Some((k, n)) if k == "player" => index(n).map(Source::Player),
            _ => None,
        },
        _ => None,
    };
    parsed.ok_or_else(|| {
        Problem::new(
            "request.invalid_value",
            format!(
                "{v} is not a source; give \"host\", \"editor\", {{\"developer\": n}} or {{\"player\": n}}."
            ),
            detail([("got", v.clone())]),
        )
    })
}

/// `queue.full {capacity}`: the queue holds its capacity of queued and held envelopes.
pub fn queue_full(capacity: usize) -> Problem {
    Problem::new(
        "queue.full",
        format!("The game's command queue holds its {capacity} commands; try again later."),
        detail([("capacity", json!(capacity))]),
    )
}

/// `source.in_use {source}`: another client of that source is alive.
pub fn source_in_use(source: Source) -> Problem {
    Problem::new(
        "source.in_use",
        format!("A client of {source:?} is already held; drop it first."),
        detail([("source", source_json(source))]),
    )
}

/// `command.tick_passed {at, boundary}`: an input of a tick whose boundary has passed.
pub fn tick_passed(at: Tick, boundary: Tick) -> Problem {
    Problem::new(
        "command.tick_passed",
        format!(
            "The command is an input of tick {}, but the game is at boundary {}; nothing was applied.",
            at.0, boundary.0
        ),
        detail([("at", json!(at.0)), ("boundary", json!(boundary.0))]),
    )
}

/// `game.stopped {reason, error?}`: the game thread ended.
pub fn game_stopped(reason: &str, error: Option<&Problem>) -> Problem {
    let mut d = detail([("reason", json!(reason))]);
    if let Some(e) = error {
        d.insert(
            "error".into(),
            serde_json::to_value(e).unwrap_or(Value::Null),
        );
    }
    Problem::new(
        "game.stopped",
        format!("The game has stopped: {reason}."),
        d,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(source: Source, seq: u64) -> Envelope {
        Envelope {
            source,
            seq,
            at: None,
            name: "x".into(),
            params: Value::Null,
            reply: ReplyTo::none(),
        }
    }

    /// threads.md 11, canonical order: every permutation of a batch of up to 7 envelopes from 4
    /// sources sorts to one order, which keeps each source's seq order.
    #[test]
    fn every_permutation_sorts_to_one_order() {
        let items = [
            (Source::Player(1), 1),
            (Source::Developer(0), 1),
            (Source::Host, 1),
            (Source::Player(0), 2),
            (Source::Player(0), 1),
            (Source::Editor, 1),
            (Source::Developer(0), 2),
        ];
        let expected: Vec<(Source, u64)> = {
            let mut v = items.to_vec();
            v.sort();
            v
        };
        let mut idx: Vec<usize> = (0..items.len()).collect();
        let mut count = 0;
        // Heap's algorithm over the 5,040 permutations.
        let mut c = vec![0usize; idx.len()];
        let mut check = |idx: &[usize]| {
            let mut batch: Vec<Envelope> =
                idx.iter().map(|&i| env(items[i].0, items[i].1)).collect();
            canonical_order(&mut batch);
            let got: Vec<(Source, u64)> = batch.iter().map(|e| (e.source, e.seq)).collect();
            assert_eq!(got, expected);
            count += 1;
        };
        check(&idx);
        let mut i = 0;
        while i < idx.len() {
            if c[i] < i {
                if i % 2 == 0 {
                    idx.swap(0, i);
                } else {
                    idx.swap(c[i], i);
                }
                check(&idx);
                c[i] += 1;
                i = 0;
            } else {
                c[i] = 0;
                i += 1;
            }
        }
        assert_eq!(count, 5040);
        assert_eq!(expected[0].0, Source::Host);
        assert_eq!(expected[6], (Source::Player(1), 1));
    }

    #[test]
    fn sources_round_trip_as_json() {
        for s in [
            Source::Host,
            Source::Editor,
            Source::Developer(3),
            Source::Player(0),
        ] {
            assert_eq!(source_from_json(&source_json(s)).unwrap(), s);
        }
        assert!(source_from_json(&json!({"player": -1})).is_err());
        assert!(source_from_json(&json!("agent")).is_err());
    }
}
