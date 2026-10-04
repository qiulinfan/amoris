//! The problem object `{code, message, detail}` (shared/contract/errors.md, The problem object) and
//! how several problems of one call become one answer (Several problems).

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::pointer::Pointer;
use crate::render;

/// A problem's detail: always an object, its fields fixed per code.
pub type Detail = Map<String, Value>;

/// At most this many further problems ride in `detail.also` (errors.md, Several problems).
pub const MAX_ALSO: usize = 9;

/// One problem: a refusal, a warning (now or later), or an intent's failure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Problem {
    /// Dotted snake_case, `<family>.<reason>`: `request.unknown_field`.
    pub code: String,
    /// One or two English sentences for the agent, built from the code's template and the detail.
    pub message: String,
    /// Always an object (possibly empty); its fields are fixed per code.
    pub detail: Detail,
}

/// How a code is used (errors.md, Where problems appear and Game codes).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Use {
    /// The whole call is refused and nothing of it is applied.
    Refuse,
    /// The call succeeded, with something worth knowing.
    Warn,
    /// An intent ended unsuccessfully.
    Fail,
}

impl Problem {
    /// A problem whose message is rendered from `template` over `detail` (render::render). This is
    /// how every code's constructor builds its problem; game codes use it with their own template.
    pub fn from_template(code: &str, template: &str, detail: Detail) -> Problem {
        let message = render::render(template, &detail);
        Problem {
            code: code.to_owned(),
            message,
            detail,
        }
    }

    /// A problem with a message written by the caller, cut to 300 bytes. Engine families whose
    /// specifications give no template (`sim`, `persist`, ...) build theirs with this, inside one
    /// constructor per code, so no call site writes a message of its own.
    pub fn new(code: &str, message: impl Into<String>, detail: Detail) -> Problem {
        Problem {
            code: code.to_owned(),
            message: render::limit(message.into()),
            detail,
        }
    }

    /// `true` when the code is `code`.
    pub fn is(&self, code: &str) -> bool {
        self.code == code
    }

    /// The code's family: `request` of `request.unknown_field`.
    pub fn family(&self) -> &str {
        self.code.split('.').next().unwrap_or("")
    }

    /// The JSON Pointer `path`, when the problem has one.
    pub fn path(&self) -> Option<&str> {
        self.detail.get("path").and_then(Value::as_str)
    }

    /// The further problems carried in `detail.also`.
    pub fn also(&self) -> Vec<Problem> {
        self.detail
            .get("also")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Several problems found by one validation phase as one answer: ordered by `path` (segments
    /// compared one by one, indices as numbers, keys by bytes; no path last), then by code; the
    /// first is the answer, the next nine ride in its `detail.also`, and `detail.also_more` counts
    /// any beyond. `None` for no problem.
    pub fn combine(problems: impl IntoIterator<Item = Problem>) -> Option<Problem> {
        let mut all: Vec<Problem> = problems.into_iter().collect();
        all.sort_by(|a, b| {
            let key = |p: &Problem| p.path().map(Pointer::parse);
            match (key(a), key(b)) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then_with(|| a.code.cmp(&b.code))
        });
        let mut iter = all.into_iter();
        let mut head = iter.next()?;
        let rest: Vec<Problem> = iter
            .map(|mut p| {
                p.detail.remove("also");
                p.detail.remove("also_more");
                p
            })
            .collect();
        if !rest.is_empty() {
            let more = rest.len().saturating_sub(MAX_ALSO);
            let also: Vec<Value> = rest
                .into_iter()
                .take(MAX_ALSO)
                .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
                .collect();
            head.detail.insert("also".into(), Value::Array(also));
            if more > 0 {
                head.detail.insert("also_more".into(), Value::from(more));
            }
        }
        Some(head)
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Problem {}

/// Builds a detail object: `detail([("seat", json!("skipper"))])`.
pub fn detail<const N: usize>(fields: [(&str, Value); N]) -> Detail {
    let mut map = Map::new();
    for (k, v) in fields {
        map.insert(k.to_owned(), v);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(code: &str, path: Option<&str>) -> Problem {
        let mut d = Detail::new();
        if let Some(path) = path {
            d.insert("path".into(), json!(path));
        }
        Problem::new(code, "m.", d)
    }

    #[test]
    fn combine_orders_by_path_then_code() {
        let one = Problem::combine([
            p("request.unknown_field", Some("/actions/10/params/x")),
            p("request.wrong_type", None),
            p("request.unknown_field", Some("/actions/2/params/y")),
            p("request.missing_field", Some("/actions/2/params/y")),
        ])
        .unwrap();
        assert_eq!(one.code, "request.missing_field");
        let also = one.also();
        let order: Vec<(&str, Option<&str>)> =
            also.iter().map(|p| (p.code.as_str(), p.path())).collect();
        assert_eq!(
            order,
            [
                ("request.unknown_field", Some("/actions/2/params/y")),
                ("request.unknown_field", Some("/actions/10/params/x")),
                ("request.wrong_type", None),
            ]
        );
        assert!(one.detail.get("also_more").is_none());
    }

    #[test]
    fn combine_caps_also_at_nine() {
        let many = (0..15).map(|i| p("request.unknown_field", Some(&format!("/f{i:02}"))));
        let one = Problem::combine(many).unwrap();
        assert_eq!(one.path(), Some("/f00"));
        assert_eq!(one.also().len(), MAX_ALSO);
        assert_eq!(one.detail["also_more"], json!(5));
        assert!(Problem::combine([]).is_none());
    }

    #[test]
    fn wire_form() {
        let pr = Problem::new("x.y", "Hello.", detail([("a", json!(1))]));
        let text = serde_json::to_string(&pr).unwrap();
        assert_eq!(
            text,
            r#"{"code":"x.y","message":"Hello.","detail":{"a":1}}"#
        );
        assert_eq!(serde_json::from_str::<Problem>(&text).unwrap(), pr);
        assert!(
            serde_json::from_str::<Problem>(r#"{"code":"a","message":"","detail":{},"x":1}"#)
                .is_err()
        );
        assert_eq!(pr.family(), "x");
    }
}
