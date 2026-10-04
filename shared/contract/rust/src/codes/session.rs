//! The `seat`, `permission`, `time`, `definition` and `internal` families (shared/contract/errors.md).

use serde_json::Value;

use super::{names, names_detail, problem};
use crate::pointer::Pointer;
use crate::problem::{Detail, Problem};
use crate::suggest::Candidate;

fn at(path: &Pointer) -> Detail {
    let mut d = Detail::new();
    d.insert("path".into(), Value::from(path.to_string()));
    d
}

/// No seat of that id.
pub fn seat_unknown(path: &Pointer, seat: &str, valid: &[Candidate<'_>]) -> Problem {
    let mut d = at(path);
    d.insert("seat".into(), Value::from(seat));
    names_detail(&mut d, seat, valid, None);
    problem(
        names::SEAT_UNKNOWN,
        "There is no seat '{seat}'; did you mean {suggestions}?",
        d,
    )
}

/// A player names a seat other than its own.
pub fn seat_not_yours(path: &Pointer, seat: &str, yours: &str) -> Problem {
    let mut d = at(path);
    d.insert("seat".into(), Value::from(seat));
    d.insert("yours".into(), Value::from(yours));
    problem(
        names::SEAT_NOT_YOURS,
        "You play seat '{yours}', not '{seat}'.",
        d,
    )
}

/// An affordance limited to other seats (an unmet requirement).
pub fn seat_not_allowed(seat: &str, seats: &[&str]) -> Problem {
    let mut d = Detail::new();
    d.insert("seat".into(), Value::from(seat));
    d.insert("seats".into(), Value::from(seats.to_vec()));
    problem(names::SEAT_NOT_ALLOWED, "Only {seats} may do this.", d)
}

/// The caller's role may not make this request.
pub fn permission_denied(request: &str, role: &str, needs: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("request".into(), Value::from(request));
    d.insert("role".into(), Value::from(role));
    d.insert("needs".into(), Value::from(needs));
    problem(
        names::PERMISSION_DENIED,
        "{request} needs the {needs} role; this caller is a {role}.",
        d,
    )
}

/// A request the current pacing or structure does not take; `takes` are the requests it does.
pub fn time_wrong_mode(request: &str, pacing: &str, structure: &str, takes: &[&str]) -> Problem {
    let mut d = Detail::new();
    d.insert("request".into(), Value::from(request));
    d.insert("pacing".into(), Value::from(pacing));
    d.insert("structure".into(), Value::from(structure));
    d.insert("takes".into(), Value::from(takes.to_vec()));
    problem(
        names::TIME_WRONG_MODE,
        "{request} is not available in {pacing} pacing; use {takes}.",
        d,
    )
}

/// `step` from someone who does not hold the clock.
pub fn time_not_clock_holder(holder: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("holder".into(), Value::from(holder));
    problem(
        names::TIME_NOT_CLOCK_HOLDER,
        "Only {holder} steps this session.",
        d,
    )
}

/// An action or `end_turn` outside the seat's turn.
pub fn time_not_your_turn(turn: u64, to_move: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("turn".into(), Value::from(turn));
    d.insert("to_move".into(), Value::from(to_move));
    problem(
        names::TIME_NOT_YOUR_TURN,
        "It is {to_move}'s turn ({turn}).",
        d,
    )
}

/// A time request or an `act` after the episode ended; `outcome` holds at least its `tick`.
pub fn time_episode_over(outcome: Value) -> Problem {
    let mut d = Detail::new();
    d.insert("outcome".into(), outcome);
    problem(
        names::TIME_EPISODE_OVER,
        "The episode ended at tick {outcome.tick}.",
        d,
    )
}

/// A `step` while another clock holder's `step` runs on the same world.
pub fn time_busy(tick: u64) -> Problem {
    let mut d = Detail::new();
    d.insert("tick".into(), Value::from(tick));
    problem(
        names::TIME_BUSY,
        "Another step is running (at tick {tick}); wait for it to answer.",
        d,
    )
}

/// The player's form of a halt after a script error.
pub fn time_halted() -> Problem {
    problem(
        names::TIME_HALTED,
        "The game stopped on an internal error; only a developer can resume it.",
        Detail::new(),
    )
}

/// Lockstep peers' tick hashes differ; the match stops.
pub fn time_desync(tick: u64, peers: Value) -> Problem {
    let mut d = Detail::new();
    d.insert("tick".into(), Value::from(tick));
    d.insert("peers".into(), peers);
    problem(
        names::TIME_DESYNC,
        "The peers' games diverged at tick {tick}; the match stopped.",
        d,
    )
}

/// `pause` or `resume` from a seat not allowed to.
pub fn time_cannot_pause(seat: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("seat".into(), Value::from(seat));
    problem(
        names::TIME_CANNOT_PAUSE,
        "Seat {seat} may not pause the game.",
        d,
    )
}

/// The warning for `continue` with no pending decision.
pub fn time_no_decision() -> Problem {
    problem(
        names::TIME_NO_DECISION,
        "There was no decision to answer.",
        Detail::new(),
    )
}

/// The later warning that the seat's thinking clock ran out before it answered.
pub fn time_clock_out(decision: Value, tick: u64) -> Problem {
    let mut d = Detail::new();
    d.insert("decision".into(), decision);
    d.insert("tick".into(), Value::from(tick));
    problem(
        names::TIME_CLOCK_OUT,
        "Your thinking time ran out at tick {tick}; the game went on.",
        d,
    )
}

/// The game definition fails validation at `path` (into the definition).
pub fn definition_invalid(path: &Pointer, reason: &str) -> Problem {
    let mut d = at(path);
    d.insert("reason".into(), Value::from(reason));
    problem(
        names::DEFINITION_INVALID,
        "The game definition is invalid at {path}: {reason}.",
        d,
    )
}

/// A bug: a handler failed or an executor broke its contract. `report` names where it is logged.
pub fn internal_error(where_: &str, report: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("where".into(), Value::from(where_));
    d.insert("report".into(), Value::from(report));
    problem(
        names::INTERNAL_ERROR,
        "The engine failed in {where}; this is a bug, reported as {report}.",
        d,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn templates() {
        let p = time_wrong_mode(
            "step",
            "real-time",
            "free",
            &["wait", "continue", "pause", "resume"],
        );
        assert_eq!(
            p.message,
            "step is not available in real-time pacing; use wait, continue, pause, resume."
        );
        let p = time_episode_over(json!({"tick": 3600, "result": "won"}));
        assert_eq!(p.message, "The episode ended at tick 3600.");
        let p = seat_not_yours(&Pointer::parse("/seat"), "rival", "skipper");
        assert_eq!(p.message, "You play seat 'skipper', not 'rival'.");
        let p = internal_error("act", "log 7");
        assert_eq!(
            p.message,
            "The engine failed in act; this is a bug, reported as log 7."
        );
        let seats: Vec<Candidate<'_>> = ["skipper", "crew"]
            .iter()
            .map(|n| Candidate::new(n))
            .collect();
        let p = seat_unknown(&Pointer::parse("/seat"), "skiper", &seats);
        assert_eq!(
            p.message,
            "There is no seat 'skiper'; did you mean 'skipper'?"
        );
    }
}
