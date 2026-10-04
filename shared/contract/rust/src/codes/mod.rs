//! One constructor per code of the contract's own families (shared/contract/errors.md, Codes): each
//! fills the code's detail and renders its message from the template errors.md gives, so no call
//! site writes a message of its own. Engine families owned by other specifications (`sim`,
//! `persist`, `script`, ...) build theirs in their own crates with [`Problem::new`].

mod play;
mod request;
mod session;

pub use play::*;
pub use request::*;
pub use session::*;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::problem::{Detail, Problem};
use crate::suggest::{Candidate, suggest};

/// Lists of at most this many names go into `detail.allowed`; longer ones are counted in
/// `allowed_count` and pointed at by `see` (errors.md, Codes).
pub const MAX_ALLOWED: usize = 30;

/// An entity as a problem names it: the JSON projection's `EntityName`, rendered `Name#id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EntityName {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl EntityName {
    pub fn new(id: u64, name: Option<&str>) -> EntityName {
        EntityName {
            id,
            name: name.map(str::to_owned),
        }
    }

    fn json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// A double for a detail field: integral values below 2^53 as JSON integers (`3`, not `3.0`), so a
/// detail reads as the contract's tables write it and compares equal on both lines.
pub fn num(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        Value::from(x as i64)
    } else {
        Value::from(x)
    }
}

/// Code names, so a caller can branch on them without a typo.
pub mod names {
    pub const REQUEST_MALFORMED: &str = "request.malformed";
    pub const REQUEST_UNKNOWN_METHOD: &str = "request.unknown_method";
    pub const REQUEST_UNKNOWN_FIELD: &str = "request.unknown_field";
    pub const REQUEST_MISPLACED_FIELD: &str = "request.misplaced_field";
    pub const REQUEST_MISSING_FIELD: &str = "request.missing_field";
    pub const REQUEST_WRONG_TYPE: &str = "request.wrong_type";
    pub const REQUEST_NOT_INTEGER: &str = "request.not_integer";
    pub const REQUEST_OUT_OF_RANGE: &str = "request.out_of_range";
    pub const REQUEST_INVALID_VALUE: &str = "request.invalid_value";
    pub const REQUEST_CONFLICT: &str = "request.conflict";
    pub const REQUEST_NOT_APPLICABLE: &str = "request.not_applicable";
    pub const REQUEST_AMBIGUOUS_REF: &str = "request.ambiguous_ref";
    pub const REQUEST_UNSUPPORTED_VERSION: &str = "request.unsupported_version";
    pub const REQUEST_ALIAS_USED: &str = "request.alias_used";
    pub const SEAT_UNKNOWN: &str = "seat.unknown";
    pub const SEAT_NOT_YOURS: &str = "seat.not_yours";
    pub const SEAT_NOT_ALLOWED: &str = "seat.not_allowed";
    pub const PERMISSION_DENIED: &str = "permission.denied";
    pub const PERCEPTION_UNKNOWN_ENTITY: &str = "perception.unknown_entity";
    pub const PERCEPTION_NOT_PERCEIVABLE: &str = "perception.not_perceivable";
    pub const PERCEPTION_UNKNOWN_INSTRUMENT: &str = "perception.unknown_instrument";
    pub const PERCEPTION_UNKNOWN_KIND: &str = "perception.unknown_kind";
    pub const PERCEPTION_BUDGET_TOO_SMALL: &str = "perception.budget_too_small";
    pub const PERCEPTION_OMNISCIENT_FORBIDDEN: &str = "perception.omniscient_forbidden";
    pub const PERCEPTION_CURSOR_AHEAD: &str = "perception.cursor_ahead";
    pub const PERCEPTION_TARGET_LOST: &str = "perception.target_lost";
    pub const ACTION_UNKNOWN_CONTROL: &str = "action.unknown_control";
    pub const ACTION_UNKNOWN_INTENT: &str = "action.unknown_intent";
    pub const ACTION_UNKNOWN_VERB: &str = "action.unknown_verb";
    pub const ACTION_TARGET_REQUIRED: &str = "action.target_required";
    pub const ACTION_TARGET_NOT_TAKEN: &str = "action.target_not_taken";
    pub const ACTION_WRONG_TARGET_KIND: &str = "action.wrong_target_kind";
    pub const ACTION_CONFLICT: &str = "action.conflict";
    pub const ACTION_UNAVAILABLE: &str = "action.unavailable";
    pub const ACTION_NOT_SEEN: &str = "action.not_seen";
    pub const ACTION_OUT_OF_REACH: &str = "action.out_of_reach";
    pub const ACTION_NOT_FACING: &str = "action.not_facing";
    pub const ACTION_REQUIREMENT_UNMET: &str = "action.requirement_unmet";
    pub const ACTION_DROPPED: &str = "action.dropped";
    pub const INTENT_UNKNOWN_ID: &str = "intent.unknown_id";
    pub const INTENT_TIMEOUT: &str = "intent.timeout";
    pub const TIME_WRONG_MODE: &str = "time.wrong_mode";
    pub const TIME_NOT_CLOCK_HOLDER: &str = "time.not_clock_holder";
    pub const TIME_NOT_YOUR_TURN: &str = "time.not_your_turn";
    pub const TIME_EPISODE_OVER: &str = "time.episode_over";
    pub const TIME_BUSY: &str = "time.busy";
    pub const TIME_HALTED: &str = "time.halted";
    pub const TIME_DESYNC: &str = "time.desync";
    pub const TIME_CANNOT_PAUSE: &str = "time.cannot_pause";
    pub const TIME_NO_DECISION: &str = "time.no_decision";
    pub const TIME_CLOCK_OUT: &str = "time.clock_out";
    pub const DEFINITION_INVALID: &str = "definition.invalid";
    pub const INTERNAL_ERROR: &str = "internal.error";
}

/// The engine's families and their owners (errors.md, Codes). A game may not declare a code in one.
pub const ENGINE_FAMILIES: [&str; 40] = [
    "request",
    "seat",
    "permission",
    "perception",
    "action",
    "intent",
    "time",
    "definition",
    "internal",
    "sim",
    "rng",
    "number",
    "script",
    "lint",
    "types",
    "scripts",
    "persist",
    "replay",
    "version",
    "migrate",
    "queue",
    "source",
    "command",
    "game",
    "lines",
    "deps",
    "fmt",
    "clippy",
    "build",
    "gen",
    "test",
    "wasm",
    "determinism",
    "fork",
    "reload",
    "web",
    "perf",
    "check",
    "contract",
    "session",
];

/// `true` for a family the engine owns.
pub fn is_engine_family(family: &str) -> bool {
    ENGINE_FAMILIES.contains(&family)
}

/// Puts `suggestions` for `unknown` and the valid names into `d`: `allowed` when there are at most
/// thirty, else `allowed_count` and `see`.
pub(crate) fn names_detail(
    d: &mut Detail,
    unknown: &str,
    valid: &[Candidate<'_>],
    see: Option<&str>,
) {
    d.insert(
        "suggestions".into(),
        Value::from(suggest(unknown, valid.iter().copied())),
    );
    if valid.len() <= MAX_ALLOWED {
        let allowed: Vec<&str> = valid.iter().map(|c| c.name).collect();
        d.insert("allowed".into(), Value::from(allowed));
    } else {
        d.insert("allowed_count".into(), Value::from(valid.len()));
    }
    if let Some(see) = see {
        d.insert("see".into(), Value::from(see));
    }
}

pub(crate) fn problem(code: &str, template: &str, d: Detail) -> Problem {
    Problem::from_template(code, template, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_families() {
        assert_eq!(num(3.0), serde_json::json!(3));
        assert_eq!(num(14.2), serde_json::json!(14.2));
        assert_eq!(num(-0.0), serde_json::json!(0));
        assert!(is_engine_family("sim"));
        assert!(is_engine_family("game"));
        assert!(!is_engine_family("sail"));
    }
}
