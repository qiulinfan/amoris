//! Objects: aliases, unknown and misplaced keys, missing fields (shared/contract/errors.md,
//! Unknown names and suggestions, Misplaced fields, Aliases).

use serde_json::{Map, Value};

use super::{Walker, number_range, types_of};
use crate::codes;
use crate::pointer::Pointer;
use crate::suggest::{Candidate, UNIT_SUFFIXES, close_match, stem};

impl<'a> Walker<'a> {
    pub(super) fn object(
        &mut self,
        value: &mut Value,
        s: &'a Value,
        obj: &'a Map<String, Value>,
        path: &Pointer,
        parent: Option<&'a Value>,
    ) {
        static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
        let props: &'a Map<String, Value> = match obj.get("properties").and_then(Value::as_object) {
            Some(p) => p,
            None => EMPTY.get_or_init(Map::new),
        };
        let extra = obj.get("additionalProperties");
        let Some(map) = value.as_object_mut() else {
            return;
        };
        // Aliases first: the canonical name is what the rest of the check, and the engine, see.
        let mut conflicted: Vec<String> = Vec::new();
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in &keys {
            if props.contains_key(k) {
                continue;
            }
            if let Some(field) = alias_target(k, props) {
                let alias_path = path.key(k);
                if map.contains_key(field) {
                    self.problems
                        .push(codes::conflict(&[alias_path, path.key(field)]));
                    conflicted.push(k.clone());
                } else if let Some(v) = map.remove(k) {
                    map.insert(field.to_owned(), v);
                    self.warnings.push(codes::alias_used(&alias_path, k, field));
                }
            }
        }
        let names = candidates(props);
        let own: Vec<Candidate<'_>> = names
            .iter()
            .map(|(n, a)| Candidate::with_aliases(n, a))
            .collect();
        // Required fields an unknown key stands for (its first suggestion, or the place it
        // belongs under) are not reported missing as well: it is one mistake.
        let mut explained: Vec<String> = Vec::new();
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in &keys {
            if props.contains_key(k) || conflicted.contains(k) {
                continue;
            }
            match extra {
                Some(Value::Bool(true)) => continue,
                Some(sch @ Value::Object(_)) => {
                    if let Some(v) = map.get_mut(k) {
                        self.walk(v, sch, &path.key(k), Some(s));
                    }
                    continue;
                }
                _ => {}
            }
            let here = path.key(k);
            let close_here = close_match(k, own.iter().copied()).is_some();
            match self.misplaced(k, props, path, parent) {
                Some(belongs_at) if !close_here => {
                    if let Some(crate::pointer::Segment::Key(first)) =
                        belongs_at.segments().get(path.segments().len())
                    {
                        explained.push(first.clone());
                    }
                    self.problems
                        .push(codes::misplaced_field(&here, &belongs_at));
                }
                _ => {
                    if let Some(first) = crate::suggest::suggest(k, own.iter().copied()).first() {
                        explained.push(first.clone());
                    }
                    let owner = self.owner(path);
                    self.problems
                        .push(codes::unknown_field(&here, &owner, &own, None));
                }
            }
        }
        if let Some(req) = obj.get("required").and_then(Value::as_array) {
            for r in req.iter().filter_map(Value::as_str) {
                if !map.contains_key(r) && !explained.iter().any(|e| e == r) {
                    let expected = props.get(r).map_or("a value".into(), |p| self.describe(p));
                    let owner = self.owner(path);
                    self.problems
                        .push(codes::missing_field(&path.key(r), &owner, &expected));
                }
            }
        }
        for (k, sch) in props {
            if let Some(v) = map.get_mut(k) {
                self.walk(v, sch, &path.key(k), Some(s));
            }
        }
    }

    /// Where an unknown key would be valid one level away: in a child object (or the first element
    /// of a child list of objects), or in the parent object.
    fn misplaced(
        &self,
        key: &str,
        props: &'a Map<String, Value>,
        path: &Pointer,
        parent: Option<&'a Value>,
    ) -> Option<Pointer> {
        for (child, sch) in props {
            for (inner, list) in self.object_props(sch) {
                let names = candidates(inner);
                let c: Vec<Candidate<'_>> = names
                    .iter()
                    .map(|(n, a)| Candidate::with_aliases(n, a))
                    .collect();
                if let Some(name) = close_match(key, c.iter().copied()) {
                    let base = path.key(child);
                    let base = if list { base.index(0) } else { base };
                    return Some(base.key(name));
                }
            }
        }
        let parent = self.resolve(parent?);
        let pprops = parent.get("properties").and_then(Value::as_object)?;
        let names = candidates(pprops);
        let c: Vec<Candidate<'_>> = names
            .iter()
            .map(|(n, a)| Candidate::with_aliases(n, a))
            .collect();
        close_match(key, c.iter().copied()).map(|n| path.parent().key(n))
    }

    /// The `properties` of the objects a schema admits (through `$ref`, `anyOf`/`oneOf` and list
    /// items), each with whether it sits inside a list.
    fn object_props(&self, sch: &'a Value) -> Vec<(&'a Map<String, Value>, bool)> {
        let mut out = Vec::new();
        self.collect_props(sch, false, 0, &mut out);
        out
    }

    fn collect_props(
        &self,
        sch: &'a Value,
        list: bool,
        depth: u32,
        out: &mut Vec<(&'a Map<String, Value>, bool)>,
    ) {
        if depth > 4 {
            return;
        }
        let s = self.resolve(sch);
        if let Some(p) = s.get("properties").and_then(Value::as_object) {
            out.push((p, list));
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(bs) = s.get(key).and_then(Value::as_array) {
                for b in bs {
                    self.collect_props(b, list, depth + 1, out);
                }
            }
        }
        if !list && let Some(items) = s.get("items") {
            self.collect_props(items, true, depth + 1, out);
        }
    }

    /// What a missing field takes, in words: `a number from -1 to 1`, `one of port, starboard`.
    pub(super) fn describe(&self, sch: &'a Value) -> String {
        let s = self.resolve(sch);
        let values: Option<Vec<String>> = s
            .get("enum")
            .and_then(Value::as_array)
            .map(|e| e.iter().map(|v| crate::render::value(v, false)).collect())
            .or_else(|| {
                let bs = s.get("oneOf").or_else(|| s.get("anyOf"))?.as_array()?;
                bs.iter()
                    .map(|b| {
                        self.resolve(b)
                            .get("const")
                            .map(|c| crate::render::value(c, false))
                    })
                    .collect()
            });
        if let Some(v) = values {
            return format!("one of {}", v.join(", "));
        }
        let Some(o) = s.as_object() else {
            return "a value".into();
        };
        let types = types_of(o);
        let word = match types.iter().find(|t| **t != "null").copied() {
            Some("number") => "a number",
            Some("integer") => "a whole number",
            Some("string") => "a string",
            Some("boolean") => "true or false",
            Some("array") => "a list",
            Some("object") => "an object",
            _ => "a value",
        };
        let mut d = Map::new();
        let r = number_range(o);
        for (k, v) in [
            ("min", r.min),
            ("max", r.max),
            ("min_exclusive", r.min_exclusive),
            ("max_exclusive", r.max_exclusive),
        ] {
            if let Some(n) = v {
                d.insert(k.into(), Value::Number(n));
            }
        }
        if matches!(word, "a number" | "a whole number") && !d.is_empty() {
            format!("{word} {}", crate::render::render("{range}", &d))
        } else {
            word.to_owned()
        }
    }
}

pub(super) fn aliases(obj: &Map<String, Value>) -> Vec<&str> {
    obj.get("x-aliases")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The names of an object's properties with the aliases each declares.
pub(super) fn candidates(props: &Map<String, Value>) -> Vec<(&str, Vec<&str>)> {
    props
        .iter()
        .map(|(k, p)| (k.as_str(), p.as_object().map(aliases).unwrap_or_default()))
        .collect()
}

/// The field an unknown key is an alias of: a declared `x-aliases` entry, or the stem of a field
/// with a unit suffix, unless that stem is itself a field or the stem of another field
/// (errors.md, Aliases).
fn alias_target<'p>(key: &str, props: &'p Map<String, Value>) -> Option<&'p str> {
    for (name, p) in props {
        if p.as_object().is_some_and(|o| aliases(o).contains(&key)) {
            return Some(name);
        }
    }
    if props.contains_key(key) {
        return None;
    }
    let mut found: Option<&str> = None;
    for name in props.keys() {
        let has_suffix = UNIT_SUFFIXES.iter().any(|s| name.ends_with(s));
        if has_suffix && stem(name) == key {
            if found.is_some() {
                return None;
            }
            found = Some(name);
        }
    }
    found
}
