//! What every loop over a [`crate::Game`] does at its boundaries, on every target: the native game
//! thread ([`crate::thread`]) and the browser's worker (`pocket-web`'s `worker.rs`) share it, so the
//! two loops keep one rule (docs/spec/threads.md 5.2 and 7.3).

use std::mem;

use pocket_link::Envelope;
use pocket_sim::Tick;

/// Takes from `held` the envelopes due at the boundary before tick `next` and those whose tick has
/// passed, leaving the later ones held (threads.md 5.2: held envelopes are matched by tick like
/// queued ones, so one whose tick a replaced world skipped is answered `command.tick_passed` and
/// its place freed rather than stranded).
pub fn take_held(held: &mut Vec<Envelope>, next: Tick) -> (Vec<Envelope>, Vec<Envelope>) {
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

#[cfg(test)]
mod tests {
    use pocket_link::{ReplyTo, Source};
    use serde_json::json;

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
