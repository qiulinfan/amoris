//! The `perception`, `action` and `intent` families (shared/contract/errors.md, `perception`,
//! `action` and `intent`).

use serde_json::Value;

use super::{EntityName, names, names_detail, num, problem};
use crate::pointer::Pointer;
use crate::problem::{Detail, Problem};
use crate::suggest::{Candidate, suggest_names};

fn at(path: &Pointer) -> Detail {
    let mut d = Detail::new();
    d.insert("path".into(), Value::from(path.to_string()));
    d
}

fn strs(items: &[&str]) -> Value {
    Value::from(items.to_vec())
}

/// A reference to an entity the caller does not know, existing or not. `known` are the names the
/// caller knows (perceives, remembers, has on its chart): suggestions come from them only.
pub fn unknown_entity<'a>(
    path: &Pointer,
    reference: &str,
    known: impl IntoIterator<Item = &'a str>,
) -> Problem {
    let mut d = at(path);
    d.insert("ref".into(), Value::from(reference));
    d.insert(
        "suggestions".into(),
        Value::from(suggest_names(reference, known)),
    );
    problem(
        names::PERCEPTION_UNKNOWN_ENTITY,
        "No entity '{ref}' is known to you; did you mean {suggestions}?",
        d,
    )
}

/// A fact or instrument the caller cannot know.
pub fn not_perceivable(path: &Pointer, name: &str, entity: Option<&EntityName>) -> Problem {
    let mut d = at(path);
    d.insert("name".into(), Value::from(name));
    if let Some(e) = entity {
        d.insert("entity".into(), e.json());
    }
    problem(
        names::PERCEPTION_NOT_PERCEIVABLE,
        "'{name}' is not something you can perceive.",
        d,
    )
}

/// No such instrument on the caller's profile.
pub fn unknown_instrument(
    path: &Pointer,
    name: &str,
    valid: &[Candidate<'_>],
    see: Option<&str>,
) -> Problem {
    let mut d = at(path);
    d.insert("name".into(), Value::from(name));
    names_detail(&mut d, name, valid, see);
    problem(
        names::PERCEPTION_UNKNOWN_INSTRUMENT,
        "There is no instrument '{name}'; did you mean {suggestions}?",
        d,
    )
}

/// No such kind.
pub fn unknown_kind(
    path: &Pointer,
    kind: &str,
    valid: &[Candidate<'_>],
    see: Option<&str>,
) -> Problem {
    let mut d = at(path);
    d.insert("kind".into(), Value::from(kind));
    names_detail(&mut d, kind, valid, see);
    problem(
        names::PERCEPTION_UNKNOWN_KIND,
        "There is no kind '{kind}'; did you mean {suggestions}?",
        d,
    )
}

/// The mandatory part does not fit the budget.
pub fn budget_too_small(budget_tokens: u64, min_tokens: u64) -> Problem {
    let mut d = Detail::new();
    d.insert("budget_tokens".into(), Value::from(budget_tokens));
    d.insert("min_tokens".into(), Value::from(min_tokens));
    problem(
        names::PERCEPTION_BUDGET_TOO_SMALL,
        "A budget of {budget_tokens} tokens cannot hold this answer's header; give at least {min_tokens}.",
        d,
    )
}

/// A player asks for the omniscient view.
pub fn omniscient_forbidden(role: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("role".into(), Value::from(role));
    problem(
        names::PERCEPTION_OMNISCIENT_FORBIDDEN,
        "The omniscient view is for developers and checkers, not players.",
        d,
    )
}

/// `since` is beyond the last perceived event.
pub fn cursor_ahead(since: u64, latest: u64) -> Problem {
    let mut d = Detail::new();
    d.insert("since".into(), Value::from(since));
    d.insert("latest".into(), Value::from(latest));
    problem(
        names::PERCEPTION_CURSOR_AHEAD,
        "No event {since} yet; the latest is {latest}.",
        d,
    )
}

/// An intent's target entity is no longer known to the seat.
pub fn target_lost(target: &EntityName) -> Problem {
    let mut d = Detail::new();
    d.insert("target".into(), target.json());
    problem(
        names::PERCEPTION_TARGET_LOST,
        "{target} is no longer known to you.",
        d,
    )
}

/// No such control on the seat.
pub fn unknown_control(
    path: &Pointer,
    seat: &str,
    control: &str,
    valid: &[Candidate<'_>],
) -> Problem {
    let mut d = at(path);
    d.insert("seat".into(), Value::from(seat));
    d.insert("control".into(), Value::from(control));
    names_detail(&mut d, control, valid, None);
    problem(
        names::ACTION_UNKNOWN_CONTROL,
        "Seat {seat} has no control '{control}'; did you mean {suggestions}?",
        d,
    )
}

/// No such intent for the seat.
pub fn unknown_intent(
    path: &Pointer,
    intent: &str,
    valid: &[Candidate<'_>],
    see: Option<&str>,
) -> Problem {
    let mut d = at(path);
    d.insert("intent".into(), Value::from(intent));
    names_detail(&mut d, intent, valid, see);
    problem(
        names::ACTION_UNKNOWN_INTENT,
        "There is no intent '{intent}'; did you mean {suggestions}?",
        d,
    )
}

/// The entity's kind offers no such verb.
pub fn unknown_verb(
    path: &Pointer,
    entity: &EntityName,
    verb: &str,
    valid: &[Candidate<'_>],
) -> Problem {
    let mut d = at(path);
    d.insert("entity".into(), entity.json());
    d.insert("verb".into(), Value::from(verb));
    names_detail(&mut d, verb, valid, None);
    problem(
        names::ACTION_UNKNOWN_VERB,
        "{entity} cannot be '{verb}'; it offers {allowed}.",
        d,
    )
}

/// The intent needs a target and none was given; `takes` says what it takes.
pub fn target_required(path: &Pointer, intent: &str, takes: &str) -> Problem {
    let mut d = at(path);
    d.insert("intent".into(), Value::from(intent));
    d.insert("takes".into(), Value::from(takes));
    problem(
        names::ACTION_TARGET_REQUIRED,
        "{intent} needs a target: {takes}.",
        d,
    )
}

/// A target given to an intent that takes none.
pub fn target_not_taken(path: &Pointer, intent: &str) -> Problem {
    let mut d = at(path);
    d.insert("intent".into(), Value::from(intent));
    problem(
        names::ACTION_TARGET_NOT_TAKEN,
        "{intent} takes no target.",
        d,
    )
}

/// The target's kind is not one the intent takes.
pub fn wrong_target_kind(
    path: &Pointer,
    intent: &str,
    target: &EntityName,
    kind: &str,
    kinds: &[&str],
) -> Problem {
    let mut d = at(path);
    d.insert("intent".into(), Value::from(intent));
    d.insert("target".into(), target.json());
    d.insert("kind".into(), Value::from(kind));
    d.insert("kinds".into(), strs(kinds));
    problem(
        names::ACTION_WRONG_TARGET_KIND,
        "{intent} cannot target {target}, a {kind}; it takes {kinds}.",
        d,
    )
}

/// What two parts of one call both drive.
#[derive(Clone, Copy, Debug)]
pub enum Driven<'a> {
    Channel(&'a str),
    Control(&'a str),
}

/// Two parts of one call drive the same channel or control.
pub fn action_conflict(paths: &[Pointer], driven: Driven<'_>) -> Problem {
    let mut d = Detail::new();
    let list: Vec<String> = paths.iter().map(Pointer::to_string).collect();
    d.insert("paths".into(), Value::from(list));
    match driven {
        Driven::Channel(c) => d.insert("channel".into(), Value::from(c)),
        Driven::Control(c) => d.insert("control".into(), Value::from(c)),
    };
    problem(
        names::ACTION_CONFLICT,
        "{paths} both drive {channel or control}; send them in separate calls.",
        d,
    )
}

/// An affordance whose requirements are not all met; `unmet` are the unmet requirements.
pub fn unavailable(path: &Pointer, entity: &EntityName, verb: &str, unmet: &[Problem]) -> Problem {
    let mut d = at(path);
    d.insert("entity".into(), entity.json());
    d.insert("verb".into(), Value::from(verb));
    let list: Vec<Value> = unmet
        .iter()
        .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
        .collect();
    d.insert("unmet".into(), Value::Array(list));
    problem(
        names::ACTION_UNAVAILABLE,
        "{entity} cannot be '{verb}' now: {first unmet message}",
        d,
    )
}

/// The entity is not seen now (an unmet requirement).
pub fn not_seen(entity: &EntityName) -> Problem {
    let mut d = Detail::new();
    d.insert("entity".into(), entity.json());
    problem(names::ACTION_NOT_SEEN, "{entity} is not in sight.", d)
}

/// The entity is too far (an unmet requirement); distances at the fact's precision.
pub fn out_of_reach(entity: &EntityName, range_m: f64, max_m: f64) -> Problem {
    let mut d = Detail::new();
    d.insert("entity".into(), entity.json());
    d.insert("range_m".into(), num(range_m));
    d.insert("max_m".into(), num(max_m));
    problem(
        names::ACTION_OUT_OF_REACH,
        "{entity} is {range_m} m away; it must be within {max_m} m.",
        d,
    )
}

/// The entity is too far off the heading (an unmet requirement).
pub fn not_facing(entity: &EntityName, off_deg: f64, max_off_deg: f64) -> Problem {
    let mut d = Detail::new();
    d.insert("entity".into(), entity.json());
    d.insert("off_deg".into(), num(off_deg));
    d.insert("max_off_deg".into(), num(max_off_deg));
    problem(
        names::ACTION_NOT_FACING,
        "{entity} is {off_deg} degrees off the bow; it must be within {max_off_deg}.",
        d,
    )
}

/// A declared fact requirement fails: `of`'s `fact` must be `op value`, and is `actual`.
pub fn requirement_unmet(
    verb: &str,
    of: &EntityName,
    fact: &str,
    op: &str,
    value: Value,
    actual: Value,
) -> Problem {
    let mut d = Detail::new();
    d.insert("verb".into(), Value::from(verb));
    d.insert("of".into(), of.json());
    d.insert("fact".into(), Value::from(fact));
    d.insert("op".into(), Value::from(op));
    d.insert("value".into(), value);
    d.insert("actual".into(), actual);
    problem(
        names::ACTION_REQUIREMENT_UNMET,
        "{verb} needs {of}'s {fact} {op} {value}; it is {actual}.",
        d,
    )
}

/// A lockstep action that was valid when submitted failed at its boundary (a later warning).
pub fn dropped(tick: u64, action: Value, why: &Problem) -> Problem {
    let mut d = Detail::new();
    d.insert("tick".into(), Value::from(tick));
    d.insert("action".into(), action);
    d.insert(
        "problem".into(),
        serde_json::to_value(why).unwrap_or(Value::Null),
    );
    problem(
        names::ACTION_DROPPED,
        "Your action for tick {tick} was dropped: {problem message}",
        d,
    )
}

/// An intent id this seat never had (or that was pruned).
pub fn unknown_intent_id(path: &Pointer, intent_id: u64) -> Problem {
    let mut d = at(path);
    d.insert("intent_id".into(), Value::from(intent_id));
    problem(
        names::INTENT_UNKNOWN_ID,
        "You have no intent #{intent_id}.",
        d,
    )
}

/// The deadline passed while the intent was active.
pub fn intent_timeout(intent: &str, timeout_s: f64) -> Problem {
    let mut d = Detail::new();
    d.insert("intent".into(), Value::from(intent));
    d.insert("timeout_s".into(), num(timeout_s));
    problem(
        names::INTENT_TIMEOUT,
        "{intent} did not finish within {timeout_s} s.",
        d,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_contract_cases() {
        let controls: Vec<Candidate<'_>> = ["rudder", "sheet", "hoist", "interact"]
            .iter()
            .map(|n| Candidate::new(n))
            .collect();
        let p = unknown_control(
            &Pointer::parse("/actions/0/controls/tiller"),
            "skipper",
            "tiller",
            &controls,
        );
        assert_eq!(
            p.message,
            "Seat skipper has no control 'tiller'; it takes rudder, sheet, hoist, interact."
        );
        assert_eq!(p.detail["suggestions"], json!([]));
        let p = unknown_control(
            &Pointer::parse("/actions/0/controls/ruder"),
            "skipper",
            "ruder",
            &controls,
        );
        assert_eq!(p.detail["suggestions"], json!(["rudder"]));

        let intents: Vec<Candidate<'_>> = ["come_to_heading", "trim_sail", "sail_to"]
            .iter()
            .map(|n| Candidate::new(n))
            .collect();
        let p = unknown_intent(
            &Pointer::parse("/actions/0/intent"),
            "sail_toward",
            &intents,
            None,
        );
        assert_eq!(p.detail["suggestions"], json!(["sail_to", "trim_sail"]));
        assert_eq!(
            p.message,
            "There is no intent 'sail_toward'; did you mean 'sail_to' or 'trim_sail'?"
        );

        let p = unknown_entity(
            &Pointer::parse("/actions/0/target"),
            "Mark9",
            ["Mark1", "Island"],
        );
        assert_eq!(p.detail["suggestions"], json!(["Mark1"]));
        let p = unknown_entity(&Pointer::parse("/actions/0/target"), "Zzz", ["Mark1"]);
        assert_eq!(p.message, "No entity 'Zzz' is known to you.");

        let crate7 = EntityName::new(25, Some("Crate7"));
        let unmet = out_of_reach(&crate7, 14.2, 3.0);
        assert_eq!(unmet.detail["max_m"], json!(3));
        let p = unavailable(
            &Pointer::parse("/actions/0"),
            &crate7,
            "take_aboard",
            &[unmet],
        );
        assert_eq!(
            p.message,
            "Crate7#25 cannot be 'take_aboard' now: Crate7#25 is 14.2 m away; it must be within 3 m."
        );
        let p = action_conflict(
            &[Pointer::parse("/actions/0"), Pointer::parse("/actions/1")],
            Driven::Channel("helm"),
        );
        assert_eq!(p.detail["channel"], json!("helm"));
        assert_eq!(
            p.message,
            "/actions/0, /actions/1 both drive helm; send them in separate calls."
        );
        let p = dropped(70, json!({"set": {}}), &p);
        assert!(
            p.message
                .starts_with("Your action for tick 70 was dropped: /actions/0")
        );
    }
}
