//! The `request` family: the request's shape (shared/contract/errors.md, `request`).

use serde_json::{Number, Value};

use super::{names, names_detail, problem};
use crate::pointer::Pointer;
use crate::problem::{Detail, Problem};
use crate::suggest::Candidate;

fn at(path: &Pointer) -> Detail {
    let mut d = Detail::new();
    d.insert("path".into(), Value::from(path.to_string()));
    d
}

/// The body is not JSON, or not a JSON object.
pub fn malformed(reason: &str, offset: u64) -> Problem {
    let mut d = Detail::new();
    d.insert("reason".into(), Value::from(reason));
    d.insert("offset".into(), Value::from(offset));
    problem(
        names::REQUEST_MALFORMED,
        "The request is not a JSON object: {reason} at byte {offset}.",
        d,
    )
}

/// No request or tool of that name.
pub fn unknown_method(method: &str, valid: &[Candidate<'_>], see: Option<&str>) -> Problem {
    let mut d = Detail::new();
    d.insert("method".into(), Value::from(method));
    names_detail(&mut d, method, valid, see);
    problem(
        names::REQUEST_UNKNOWN_METHOD,
        "There is no request '{method}'; did you mean {suggestions}?",
        d,
    )
}

/// A key the object at `path`'s parent does not take. `owner` is what the object is, said as an
/// agent would (`come_to_heading`, `the act request`, `actions[0]`).
pub fn unknown_field(
    path: &Pointer,
    owner: &str,
    valid: &[Candidate<'_>],
    see: Option<&str>,
) -> Problem {
    let field = path.field_name();
    let mut d = at(path);
    d.insert("field".into(), Value::from(field.as_str()));
    d.insert("owner".into(), Value::from(owner));
    names_detail(&mut d, &field, valid, see);
    problem(
        names::REQUEST_UNKNOWN_FIELD,
        "{owner} has no {parameter or field} '{field}'; did you mean {suggestions}?",
        d,
    )
}

/// A key this object does not take that a neighbouring object does, at `belongs_at`.
pub fn misplaced_field(path: &Pointer, belongs_at: &Pointer) -> Problem {
    let mut d = at(path);
    d.insert("field".into(), Value::from(path.field_name()));
    d.insert("belongs_at".into(), Value::from(belongs_at.to_string()));
    problem(
        names::REQUEST_MISPLACED_FIELD,
        "'{field}' does not go here ({path}); it belongs at {belongs_at}.",
        d,
    )
}

/// A required key is absent; `expected` says what it takes (`a number from -1 to 1`).
pub fn missing_field(path: &Pointer, owner: &str, expected: &str) -> Problem {
    let mut d = at(path);
    d.insert("field".into(), Value::from(path.field_name()));
    d.insert("owner".into(), Value::from(owner));
    d.insert("expected".into(), Value::from(expected));
    problem(
        names::REQUEST_MISSING_FIELD,
        "{owner} needs '{field}' ({expected}).",
        d,
    )
}

/// A value of the wrong JSON type: `expected` and `got` are JSON type names (`number`, `string`).
pub fn wrong_type(path: &Pointer, expected: &str, got: &str) -> Problem {
    let mut d = at(path);
    d.insert("expected".into(), Value::from(expected));
    d.insert("got".into(), Value::from(got));
    problem(
        names::REQUEST_WRONG_TYPE,
        "'{field}' must be {expected}; got {got}.",
        d,
    )
}

/// A number with a fraction where an integer goes.
pub fn not_integer(path: &Pointer, got: &Value) -> Problem {
    let mut d = at(path);
    d.insert("got".into(), got.clone());
    problem(
        names::REQUEST_NOT_INTEGER,
        "'{field}' must be a whole number; got {got}.",
        d,
    )
}

/// The bounds of a range, as the request's schema declares them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Range {
    pub min: Option<Number>,
    pub max: Option<Number>,
    pub min_exclusive: Option<Number>,
    pub max_exclusive: Option<Number>,
}

impl Range {
    /// Both ends inclusive.
    pub fn inclusive(min: impl Into<Number>, max: impl Into<Number>) -> Range {
        Range {
            min: Some(min.into()),
            max: Some(max.into()),
            ..Range::default()
        }
    }

    /// From `min` up to but not including `max`.
    pub fn half_open(min: impl Into<Number>, max: impl Into<Number>) -> Range {
        Range {
            min: Some(min.into()),
            max_exclusive: Some(max.into()),
            ..Range::default()
        }
    }

    /// `true` when `x` lies inside.
    pub fn contains(&self, x: f64) -> bool {
        let f = |n: &Option<Number>| n.as_ref().and_then(Number::as_f64);
        f(&self.min).is_none_or(|m| x >= m)
            && f(&self.max).is_none_or(|m| x <= m)
            && f(&self.min_exclusive).is_none_or(|m| x > m)
            && f(&self.max_exclusive).is_none_or(|m| x < m)
    }
}

/// A number outside its range, or a string or array of the wrong length (`got` is then the
/// length). The hint, when given, follows the message (`360 is 0.`).
pub fn out_of_range(path: &Pointer, got: &Value, range: &Range, hint: Option<&str>) -> Problem {
    let mut d = at(path);
    d.insert("got".into(), got.clone());
    for (k, v) in [
        ("min", &range.min),
        ("max", &range.max),
        ("min_exclusive", &range.min_exclusive),
        ("max_exclusive", &range.max_exclusive),
    ] {
        if let Some(n) = v {
            d.insert(k.into(), Value::Number(n.clone()));
        }
    }
    let template = match hint {
        Some(h) => {
            d.insert("hint".into(), Value::from(h));
            "'{field}' must be {range}; got {got}. {hint}"
        }
        None => "'{field}' must be {range}; got {got}.",
    };
    problem(names::REQUEST_OUT_OF_RANGE, template, d)
}

/// A name not among an enumeration's values.
pub fn invalid_value(path: &Pointer, got: &str, valid: &[Candidate<'_>]) -> Problem {
    let mut d = at(path);
    d.insert("got".into(), Value::from(got));
    names_detail(&mut d, got, valid, None);
    problem(
        names::REQUEST_INVALID_VALUE,
        "'{field}' must be one of {allowed}; got '{got}'.",
        d,
    )
}

/// Fields that exclude each other, or an alias given with its own field.
pub fn conflict(paths: &[Pointer]) -> Problem {
    let mut d = Detail::new();
    let list: Vec<String> = paths.iter().map(Pointer::to_string).collect();
    d.insert("paths".into(), Value::from(list));
    problem(
        names::REQUEST_CONFLICT,
        "{paths} cannot be given together.",
        d,
    )
}

/// A field that does not apply in this combination.
pub fn not_applicable(path: &Pointer, because: &str) -> Problem {
    let mut d = at(path);
    d.insert("because".into(), Value::from(because));
    problem(
        names::REQUEST_NOT_APPLICABLE,
        "'{field}' does not apply: {because}.",
        d,
    )
}

/// A name that several entities the caller knows share: `candidates` are `(id, name, kind)`.
pub fn ambiguous_ref(path: &Pointer, reference: &str, candidates: &[(u64, &str, &str)]) -> Problem {
    let mut d = at(path);
    d.insert("ref".into(), Value::from(reference));
    let list: Vec<Value> = candidates
        .iter()
        .map(|(id, name, kind)| serde_json::json!({"id": id, "name": name, "kind": kind}))
        .collect();
    d.insert("candidates".into(), Value::Array(list));
    problem(
        names::REQUEST_AMBIGUOUS_REF,
        "'{ref}' names {n} entities; give one of their ids: {candidates}.",
        d,
    )
}

/// `contract` names a version this engine does not serve.
pub fn unsupported_version(got: &str, served: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("got".into(), Value::from(got));
    d.insert("served".into(), Value::from(served));
    problem(
        names::REQUEST_UNSUPPORTED_VERSION,
        "This engine serves contract {served}; the request asked for {got}.",
        d,
    )
}

/// The warning that a declared alias was accepted as `field`.
pub fn alias_used(path: &Pointer, alias: &str, field: &str) -> Problem {
    let mut d = at(path);
    d.insert("alias".into(), Value::from(alias));
    d.insert("field".into(), Value::from(field));
    problem(
        names::REQUEST_ALIAS_USED,
        "'{alias}' was read as '{field}'.",
        d,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PARAMS: [&str; 6] = [
        "heading_deg",
        "tolerance_deg",
        "turn",
        "settle_s",
        "keep",
        "timeout_s",
    ];

    fn cands(names: &[&'static str]) -> Vec<Candidate<'static>> {
        names.iter().map(|n| Candidate::new(n)).collect()
    }

    #[test]
    fn unknown_field_as_the_contract_writes_it() {
        let path = Pointer::parse("/actions/0/params/headng");
        let p = unknown_field(&path, "come_to_heading", &cands(&PARAMS), None);
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            json!({"code": "request.unknown_field",
                   "message": "come_to_heading has no parameter 'headng'; did you mean 'heading_deg'?",
                   "detail": {"path": "/actions/0/params/headng", "field": "headng",
                              "owner": "come_to_heading", "suggestions": ["heading_deg"],
                              "allowed": PARAMS}})
        );
    }

    #[test]
    fn out_of_range_with_its_hint() {
        let path = Pointer::parse("/actions/0/params/heading_deg");
        let p = out_of_range(
            &path,
            &json!(360),
            &Range::half_open(0, 360),
            Some("360 is 0."),
        );
        assert_eq!(
            p.message,
            "'heading_deg' must be from 0 up to but not including 360; got 360. 360 is 0."
        );
        assert_eq!(p.detail["min"], json!(0));
        assert_eq!(p.detail["max_exclusive"], json!(360));
        let r = out_of_range(
            &Pointer::parse("/actions/0/controls/rudder"),
            &json!(1.5),
            &Range::inclusive(-1, 1),
            None,
        );
        assert_eq!(r.message, "'rudder' must be from -1 to 1; got 1.5.");
        assert!(Range::half_open(0, 360).contains(359.9));
        assert!(!Range::half_open(0, 360).contains(360.0));
    }

    #[test]
    fn other_templates() {
        let p = invalid_value(
            &Pointer::parse("/actions/0/params/turn"),
            "left",
            &cands(&["shortest", "port", "starboard"]),
        );
        assert_eq!(
            p.message,
            "'turn' must be one of shortest, port, starboard; got 'left'."
        );
        assert_eq!(p.detail["suggestions"], json!([]));
        let p = unknown_method("observ", &cands(&["observe", "act", "step"]), None);
        assert_eq!(
            p.message,
            "There is no request 'observ'; did you mean 'observe'?"
        );
        let p = wrong_type(
            &Pointer::parse("/actions/0/params/heading_deg"),
            "number",
            "string",
        );
        assert_eq!(p.detail["expected"], json!("number"));
        assert_eq!(p.detail["got"], json!("string"));
        let p = conflict(&[
            Pointer::parse("/a/heading"),
            Pointer::parse("/a/heading_deg"),
        ]);
        assert_eq!(
            p.message,
            "/a/heading, /a/heading_deg cannot be given together."
        );
        let p = ambiguous_ref(
            &Pointer::parse("/actions/0/target"),
            "Mark1",
            &[(4, "Mark1", "mark"), (9, "Mark1", "mark")],
        );
        assert_eq!(
            p.message,
            "'Mark1' names 2 entities; give one of their ids: Mark1#4, Mark1#9."
        );
        let p = misplaced_field(
            &Pointer::parse("/actions/0/heading_deg"),
            &Pointer::parse("/actions/0/params/heading_deg"),
        );
        assert_eq!(
            p.message,
            "'heading_deg' does not go here (/actions/0/heading_deg); it belongs at \
             /actions/0/params/heading_deg."
        );
        let p = missing_field(
            &Pointer::parse("/actions/0/params/heading_deg"),
            "come_to_heading",
            "a number",
        );
        assert_eq!(p.message, "come_to_heading needs 'heading_deg' (a number).");
        let p = alias_used(
            &Pointer::parse("/actions/0/params/heading"),
            "heading",
            "heading_deg",
        );
        assert_eq!(p.message, "'heading' was read as 'heading_deg'.");
    }

    #[test]
    fn long_lists_are_counted() {
        let many: Vec<String> = (0..40).map(|i| format!("name{i:02}")).collect();
        let c: Vec<Candidate<'_>> = many.iter().map(|n| Candidate::new(n)).collect();
        let p = unknown_method("zzz", &c, Some("describe {\"part\": \"requests\"}"));
        assert!(p.detail.get("allowed").is_none());
        assert_eq!(p.detail["allowed_count"], json!(40));
        assert_eq!(
            p.message,
            "There is no request 'zzz'; see describe {\"part\": \"requests\"}."
        );
    }
}
