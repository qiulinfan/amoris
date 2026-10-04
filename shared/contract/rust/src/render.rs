//! Message templates (shared/contract/errors.md, Codes, Placeholders, Messages): one renderer for the
//! engine's codes and a game's own, so the same problem gives the same bytes on both lines.

use serde_json::{Map, Number, Value};

use crate::pointer::{Pointer, Segment};

/// Longest message in bytes (errors.md, Messages).
pub const MAX_MESSAGE_BYTES: usize = 300;

/// Lists of at most this many valid names are written out in a message (errors.md, Messages).
pub const MAX_NAMES_IN_MESSAGE: usize = 8;

/// Renders `template` over a problem's detail.
///
/// Placeholders are detail fields, with the derived ones of errors.md: `{field}` (the detail's
/// `field`, else the last segment of `path`), `{owner}`, `{parameter or field}` (`parameter` when
/// the path's parent key is `params`), `{range}` (from `min`, `max`, `max_exclusive` and
/// `min_exclusive`), `{n}` (the number of `candidates`), `{channel or control}`, `{first unmet
/// message}`, `{problem message}` and dotted names (`{outcome.tick}`). Lists render as `'a'`,
/// `'a' or 'b'`, `'a', 'b' or 'c'` for `suggestions` and as `a, b, c` otherwise; an entity (an
/// object with an `id`) as `Name#id`. A clause `did you mean {suggestions}?` with no suggestion
/// becomes `it takes {allowed}.` when at most eight names are allowed, `see {see}.` when the
/// detail names where to look, and is dropped otherwise.
pub fn render(template: &str, detail: &Map<String, Value>) -> String {
    const CLAUSE: &str = "did you mean {suggestions}?";
    let mut template = template.to_owned();
    let no_suggestions = detail
        .get("suggestions")
        .and_then(Value::as_array)
        .is_some_and(|a| a.is_empty());
    if no_suggestions && template.contains(CLAUSE) {
        let allowed = detail.get("allowed").and_then(Value::as_array);
        let replacement = match (allowed, detail.get("see").and_then(Value::as_str)) {
            (Some(a), _) if !a.is_empty() && a.len() <= MAX_NAMES_IN_MESSAGE => {
                "it takes {allowed}.".to_owned()
            }
            (_, Some(_)) => "see {see}.".to_owned(),
            (Some(a), None) if !a.is_empty() => {
                format!("it takes one of {} names, listed in the detail.", a.len())
            }
            _ => String::new(),
        };
        if replacement.is_empty() {
            template = template.replace(&format!("; {CLAUSE}"), ".");
            template = template.replace(CLAUSE, "");
        } else {
            template = template.replace(CLAUSE, &replacement);
        }
    }
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template.as_str();
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                out.push_str(&placeholder(&after[..end], detail));
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    limit(out)
}

/// Cuts a message to [`MAX_MESSAGE_BYTES`] at a character boundary, keeping a final full stop.
pub fn limit(mut message: String) -> String {
    if message.len() <= MAX_MESSAGE_BYTES {
        return message;
    }
    let mut cut = MAX_MESSAGE_BYTES - 4;
    while !message.is_char_boundary(cut) {
        cut -= 1;
    }
    message.truncate(cut);
    message.push_str("...");
    message.push('.');
    message
}

fn placeholder(name: &str, detail: &Map<String, Value>) -> String {
    let path = detail
        .get("path")
        .and_then(Value::as_str)
        .map(Pointer::parse)
        .unwrap_or_default();
    match name {
        "field" => match detail.get("field").and_then(Value::as_str) {
            Some(f) => f.to_owned(),
            None => path.field_name(),
        },
        "parameter or field" => {
            let parent = path.parent();
            match parent.last() {
                Some(Segment::Key(k)) if k == "params" => "parameter".into(),
                _ => "field".into(),
            }
        }
        "range" => range(detail),
        "n" => detail
            .get("candidates")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
            .to_string(),
        "channel or control" => detail
            .get("channel")
            .or_else(|| detail.get("control"))
            .map(|v| value(v, false))
            .unwrap_or_default(),
        "first unmet message" => detail
            .get("unmet")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|p| p.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        "problem message" => detail
            .get("problem")
            .and_then(|p| p.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => {
            let mut parts = name.split('.');
            let mut v = parts.next().and_then(|k| detail.get(k));
            for k in parts {
                v = v.and_then(|x| x.get(k));
            }
            match v {
                Some(v) => value(v, name == "suggestions"),
                None => String::new(),
            }
        }
    }
}

/// `from -1 to 1`, `from 0 up to but not including 360`, `at least 1`, `at most 600`.
fn range(detail: &Map<String, Value>) -> String {
    let get = |k: &str| detail.get(k).and_then(Value::as_number).map(number);
    let (min, max, max_ex, min_ex) = (
        get("min"),
        get("max"),
        get("max_exclusive"),
        get("min_exclusive"),
    );
    match (min, min_ex, max, max_ex) {
        (Some(a), _, Some(b), _) => format!("from {a} to {b}"),
        (Some(a), _, None, Some(b)) => format!("from {a} up to but not including {b}"),
        (None, Some(a), Some(b), _) => format!("above {a} and at most {b}"),
        (None, Some(a), None, Some(b)) => format!("above {a} and below {b}"),
        (Some(a), _, None, None) => format!("at least {a}"),
        (None, Some(a), None, None) => format!("above {a}"),
        (None, None, Some(b), _) => format!("at most {b}"),
        (None, None, None, Some(b)) => format!("below {b}"),
        (None, None, None, None) => String::new(),
    }
}

/// A detail value as prose.
pub fn value(v: &Value, quoted_or: bool) -> String {
    match v {
        Value::Null => "none".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number(n),
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(|x| value(x, false)).collect();
            if quoted_or {
                list_or(&parts)
            } else {
                parts.join(", ")
            }
        }
        Value::Object(o) => match o.get("id").and_then(Value::as_u64) {
            Some(id) => entity(id, o.get("name").and_then(Value::as_str)),
            None => Value::Object(o.clone()).to_string(),
        },
    }
}

/// `Name#id`, or `#id` without a name: how the text projection writes an entity.
pub fn entity(id: u64, name: Option<&str>) -> String {
    format!("{}#{id}", name.unwrap_or(""))
}

/// `'a'`, `'a' or 'b'`, `'a', 'b' or 'c'`.
pub fn list_or(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    match quoted.len() {
        0 => String::new(),
        1 => quoted[0].clone(),
        n => format!("{} or {}", quoted[..n - 1].join(", "), quoted[n - 1]),
    }
}

/// A JSON number as the request had it; a double with an integral value below 2^53 is written
/// without a fraction, so `360.0` in a schema reads `360` (errors.md, Messages).
pub fn number(n: &Number) -> String {
    if n.is_f64() {
        let f = n.as_f64().unwrap_or(0.0);
        if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
            return format!("{}", f as i64);
        }
    }
    n.to_string()
}

/// The JSON type of a value, as `request.wrong_type` names it (`got`).
pub fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn derived_placeholders() {
        let d = obj(
            json!({"path": "/actions/0/params/heading_deg", "min": 0, "max_exclusive": 360,
                           "got": 360}),
        );
        assert_eq!(
            render("'{field}' must be {range}; got {got}.", &d),
            "'heading_deg' must be from 0 up to but not including 360; got 360."
        );
        let d = obj(json!({"path": "/actions/2"}));
        assert_eq!(render("{field}", &d), "item 2");
        let d = obj(json!({"min": -1.0, "max": 1}));
        assert_eq!(render("{range}", &d), "from -1 to 1");
        assert_eq!(render("{range}", &obj(json!({"min": 1}))), "at least 1");
        assert_eq!(render("{range}", &obj(json!({"max": 600}))), "at most 600");
        let d = obj(json!({"path": "/actions/0/params/x"}));
        assert_eq!(render("{parameter or field}", &d), "parameter");
        let d = obj(json!({"path": "/actions/0/x"}));
        assert_eq!(render("{parameter or field}", &d), "field");
    }

    #[test]
    fn lists_and_entities() {
        let d = obj(
            json!({"suggestions": ["a", "b", "c"], "allowed": ["a", "b", "c", "d"],
                           "candidates": [{"id": 4, "name": "Mark1"}, {"id": 9, "name": "Mark1"}],
                           "outcome": {"tick": 70}}),
        );
        assert_eq!(render("{suggestions}", &d), "'a', 'b' or 'c'");
        assert_eq!(render("{allowed}", &d), "a, b, c, d");
        assert_eq!(render("{n}: {candidates}", &d), "2: Mark1#4, Mark1#9");
        assert_eq!(render("at {outcome.tick}", &d), "at 70");
        let two = obj(json!({"suggestions": ["a", "b"]}));
        assert_eq!(render("{suggestions}", &two), "'a' or 'b'");
    }

    #[test]
    fn empty_suggestions_become_the_allowed_names() {
        let t = "Seat {seat} has no control '{control}'; did you mean {suggestions}?";
        let d = obj(
            json!({"seat": "skipper", "control": "tiller", "suggestions": [],
                           "allowed": ["rudder", "sheet", "hoist", "interact"]}),
        );
        assert_eq!(
            render(t, &d),
            "Seat skipper has no control 'tiller'; it takes rudder, sheet, hoist, interact."
        );
        let d = obj(json!({"seat": "s", "control": "c", "suggestions": [], "see": "describe"}));
        assert_eq!(render(t, &d), "Seat s has no control 'c'; see describe.");
        let d = obj(json!({"seat": "s", "control": "c", "suggestions": []}));
        assert_eq!(render(t, &d), "Seat s has no control 'c'.");
    }

    #[test]
    fn long_messages_are_cut() {
        let long = "x".repeat(400);
        let m = limit(long);
        assert!(m.len() <= MAX_MESSAGE_BYTES);
        assert!(m.ends_with('.'));
    }
}
