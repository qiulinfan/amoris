//! Plain data: the one canonical form of event data and of the data scripts pass
//! (docs/spec/persistence.md 3.4). Numbers are finite and object keys ascend by their UTF-8 bytes
//! without duplicates, so an object's bytes do not depend on the order its keys were set in.
//! Declared here, below persistence, because events carry it (simulation.md 5.1).

use pocket_contract::{Problem, detail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// A plain value; variant indices 0 to 5 in this order (persistence.md 3.4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum PlainData {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<PlainData>),
    /// Keys ascending by UTF-8 bytes, no duplicates.
    Object(Vec<(String, PlainData)>),
}

fn not_finite(path: &str, x: f64) -> Problem {
    Problem::new(
        "number.not_finite",
        format!("'{path}' is {x}; plain data holds finite numbers only."),
        detail([("field", json!(path)), ("value", json!(x.to_string()))]),
    )
}

impl PlainData {
    /// A finite number, else `number.not_finite`.
    pub fn number(x: f64) -> Result<PlainData, Problem> {
        if x.is_finite() {
            Ok(PlainData::Number(x))
        } else {
            Err(not_finite("", x))
        }
    }

    /// An object from entries in any order: sorted by key bytes; a repeated key is
    /// `sim.data_invalid`.
    pub fn object(mut entries: Vec<(String, PlainData)>) -> Result<PlainData, Problem> {
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        for w in entries.windows(2) {
            if w[0].0 == w[1].0 {
                return Err(Problem::new(
                    "sim.data_invalid",
                    format!("The key '{}' is given twice.", w[0].0),
                    detail([("reason", json!("duplicate key")), ("key", json!(w[0].0))]),
                ));
            }
        }
        Ok(PlainData::Object(entries))
    }

    /// A field of an object.
    pub fn get(&self, key: &str) -> Option<&PlainData> {
        match self {
            PlainData::Object(entries) => entries
                .binary_search_by(|e| e.0.as_bytes().cmp(key.as_bytes()))
                .ok()
                .map(|i| &entries[i].1),
            _ => None,
        }
    }

    /// Checks the canonical form: finite numbers, object keys strictly ascending. The first fault,
    /// with the JSON Pointer of where it is.
    pub fn validate(&self) -> Result<(), Problem> {
        self.validate_at(&mut String::new())
    }

    fn validate_at(&self, path: &mut String) -> Result<(), Problem> {
        match self {
            PlainData::Number(x) if !x.is_finite() => Err(not_finite(path, *x)),
            PlainData::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    let len = path.len();
                    path.push_str(&format!("/{i}"));
                    v.validate_at(path)?;
                    path.truncate(len);
                }
                Ok(())
            }
            PlainData::Object(entries) => {
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 && entries[i - 1].0.as_bytes() >= k.as_bytes() {
                        return Err(Problem::new(
                            "sim.data_invalid",
                            format!("Object keys at '{path}' are not in ascending byte order."),
                            detail([("reason", json!("keys out of order")), ("key", json!(k))]),
                        ));
                    }
                    let len = path.len();
                    path.push('/');
                    path.push_str(&k.replace('~', "~0").replace('/', "~1"));
                    v.validate_at(path)?;
                    path.truncate(len);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// From JSON: every number must be finite (JSON has no other) and every key is kept; serde_json
    /// maps iterate in key order, which is the byte order this form needs.
    pub fn from_json(v: &Value) -> PlainData {
        match v {
            Value::Null => PlainData::Null,
            Value::Bool(b) => PlainData::Bool(*b),
            Value::Number(n) => PlainData::Number(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => PlainData::String(s.clone()),
            Value::Array(a) => PlainData::Array(a.iter().map(PlainData::from_json).collect()),
            Value::Object(o) => {
                let mut entries: Vec<(String, PlainData)> = o
                    .iter()
                    .map(|(k, v)| (k.clone(), PlainData::from_json(v)))
                    .collect();
                entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                PlainData::Object(entries)
            }
        }
    }

    /// To JSON, the form agents read: an integral number below 2^53 as an integer (so `-0` reads
    /// `0`, as JavaScript's `JSON.stringify` writes it).
    pub fn to_json(&self) -> Value {
        match self {
            PlainData::Null => Value::Null,
            PlainData::Bool(b) => Value::Bool(*b),
            PlainData::Number(x) => pocket_contract::codes::num(*x),
            PlainData::String(s) => Value::String(s.clone()),
            PlainData::Array(a) => Value::Array(a.iter().map(PlainData::to_json).collect()),
            PlainData::Object(entries) => {
                let mut m = Map::new();
                for (k, v) in entries {
                    m.insert(k.clone(), v.to_json());
                }
                Value::Object(m)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form() {
        let o = PlainData::object(vec![
            ("b".into(), PlainData::Bool(true)),
            ("a".into(), PlainData::Number(1.0)),
        ])
        .unwrap();
        assert_eq!(o.get("a"), Some(&PlainData::Number(1.0)));
        assert!(o.validate().is_ok());
        assert_eq!(o.to_json(), json!({"a": 1, "b": true}));
        assert_eq!(PlainData::from_json(&o.to_json()), o);
        let dup = PlainData::object(vec![
            ("a".into(), PlainData::Null),
            ("a".into(), PlainData::Null),
        ]);
        assert_eq!(dup.unwrap_err().code, "sim.data_invalid");
        let bad = PlainData::Array(vec![PlainData::Number(f64::NAN)]);
        let e = bad.validate().unwrap_err();
        assert_eq!(e.code, "number.not_finite");
        assert_eq!(e.detail["field"], json!("/0"));
        let unsorted = PlainData::Object(vec![
            ("b".into(), PlainData::Null),
            ("a".into(), PlainData::Null),
        ]);
        assert_eq!(unsorted.validate().unwrap_err().code, "sim.data_invalid");
        assert!(PlainData::number(f64::INFINITY).is_err());
    }
}
