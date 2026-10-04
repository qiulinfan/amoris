//! `anyOf` and `oneOf`: optional values, untagged and tagged enums.

use serde_json::Value;

use super::{Walker, types_of};
use crate::codes;
use crate::pointer::Pointer;
use crate::render::json_type;
use crate::suggest::Candidate;

impl<'a> Walker<'a> {
    pub(super) fn branches(
        &mut self,
        value: &mut Value,
        branches: &'a [Value],
        path: &Pointer,
        parent: Option<&'a Value>,
    ) {
        let mut tried: Vec<(usize, Walker<'a>, Value)> = Vec::new();
        for (i, b) in branches.iter().enumerate() {
            let mut w = Walker::new(self.root, self.opts);
            let mut v = value.clone();
            w.walk(&mut v, b, path, parent);
            if w.problems.is_empty() {
                *value = v;
                self.warnings.extend(w.warnings);
                return;
            }
            tried.push((i, w, v));
        }
        // A string matched against unit variants: one invalid_value over all of them.
        if let Some(got) = value.as_str() {
            let consts: Vec<&str> = branches
                .iter()
                .filter_map(|b| self.resolve(b).get("const").and_then(Value::as_str))
                .chain(branches.iter().flat_map(|b| {
                    self.resolve(b)
                        .get("enum")
                        .and_then(Value::as_array)
                        .map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                        .unwrap_or_default()
                }))
                .collect();
            if !consts.is_empty() {
                let c: Vec<Candidate<'_>> = consts.iter().map(|n| Candidate::new(n)).collect();
                self.problems.push(codes::invalid_value(path, got, &c));
                return;
            }
        }
        // An externally tagged variant: an object of one key naming the variant.
        if let Some(map) = value.as_object()
            && map.len() == 1
        {
            let tags: Vec<&str> = branches
                .iter()
                .filter_map(|b| single_key(self.resolve(b)))
                .collect();
            if tags.len() == branches.len() {
                let key = map.keys().next().map_or("", String::as_str);
                if !tags.contains(&key) {
                    let c: Vec<Candidate<'_>> = tags.iter().map(|n| Candidate::new(n)).collect();
                    let owner = self.owner(path);
                    self.problems
                        .push(codes::unknown_field(&path.key(key), &owner, &c, None));
                    return;
                }
            }
        }
        // Prefer branches whose constant fields (a tag) the value matches, then fewest problems.
        let matches_tag = |b: &Value| -> bool {
            let s = self.resolve(b);
            let Some(props) = s.get("properties").and_then(Value::as_object) else {
                return false;
            };
            let consts: Vec<(&String, &Value)> = props
                .iter()
                .filter_map(|(k, p)| self.resolve(p).get("const").map(|c| (k, c)))
                .collect();
            !consts.is_empty()
                && consts
                    .iter()
                    .all(|(k, c)| value.get(k.as_str()) == Some(*c))
        };
        let type_only = |w: &Walker<'a>| -> Option<String> {
            match w.problems.as_slice() {
                [p] if p.is("request.wrong_type")
                    && p.path() == Some(path.to_string().as_str()) =>
                {
                    p.detail
                        .get("expected")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }
                _ => None,
            }
        };
        let unions: Option<Vec<String>> = tried.iter().map(|(_, w, _)| type_only(w)).collect();
        if let Some(unions) = unions {
            let mut expected: Vec<&str> = Vec::new();
            for t in unions.iter().flat_map(|u| u.split(" or ")) {
                if !expected.contains(&t) {
                    expected.push(t);
                }
            }
            self.problems.push(codes::wrong_type(
                path,
                &expected.join(" or "),
                json_type(value),
            ));
            return;
        }
        // Prefer a branch of the value's own JSON type, then one whose tag matches, then the one
        // with fewest problems, then the first.
        let fits = |b: &Value| -> bool {
            let Some(o) = self.resolve(b).as_object() else {
                return true;
            };
            let types = types_of(o);
            let got = json_type(value);
            if types.is_empty() {
                let consts: Vec<&Value> = o
                    .get("enum")
                    .and_then(Value::as_array)
                    .map(|e| e.iter().collect())
                    .or_else(|| o.get("const").map(|c| vec![c]))
                    .unwrap_or_default();
                return consts.is_empty() || consts.iter().any(|c| json_type(c) == got);
            }
            types.contains(&got) || (got == "number" && types.contains(&"integer"))
        };
        let best = tried.into_iter().min_by_key(|(i, w, _)| {
            let b = &branches[*i];
            (!fits(b), !matches_tag(b), w.problems.len(), *i)
        });
        if let Some((_, w, _)) = best {
            self.problems.extend(w.problems);
        }
    }
}

fn single_key(s: &Value) -> Option<&str> {
    let props = s.get("properties")?.as_object()?;
    if props.len() != 1 {
        return None;
    }
    props.keys().next().map(String::as_str)
}
