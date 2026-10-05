//! Checking a request against its type's JSON Schema before anything is applied (charter 3.4;
//! shared/contract/errors.md): unknown fields are refused with "did you mean" suggestions or as
//! misplaced, declared aliases and unit-stem aliases are accepted with a warning, and missing
//! fields, wrong types, fractions, ranges and enumeration values are refused, every problem of the
//! shape phase reported together (Several problems). Commands and decoders call [`decode`] (or a
//! cached [`Shape`]) and apply nothing unless it succeeds.
//!
//! A key that no `properties` entry takes is refused even when the schema omits
//! `additionalProperties: false`: a struct without `deny_unknown_fields` would otherwise drop it
//! silently, which the charter forbids. Only `additionalProperties` given as a schema (a map) admits
//! other keys.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Map, Number, Value};

mod object;
mod union;

use crate::codes::{self, Range};
use crate::pointer::Pointer;
use crate::problem::Problem;
use crate::render::json_type;
use crate::suggest::Candidate;
use object::aliases;

/// Names the object at a path, or `None` for the default name.
pub type OwnerOf = dyn Fn(&Pointer) -> Option<String>;

/// What [`Shape::check`] needs besides the value.
pub struct CheckOptions<'a> {
    /// What the top-level object is, said as an agent would: `the act request`.
    pub owner: &'a str,
    /// Where the value sits in the whole request; problems' paths start here.
    pub base: Pointer,
    /// Names the object at a path when the default (`actions[0].params`) is not what an agent
    /// would say: an intent's parameters are named by the intent (`come_to_heading`).
    pub owner_of: Option<&'a OwnerOf>,
}

impl<'a> CheckOptions<'a> {
    pub fn new(owner: &'a str) -> CheckOptions<'a> {
        CheckOptions {
            owner,
            base: Pointer::root(),
            owner_of: None,
        }
    }
}

/// A value that passed: aliases renamed to their fields and integral doubles written as integers
/// where integers go, with the warnings (`request.alias_used`) to return beside the answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    pub value: Value,
    pub warnings: Vec<Problem>,
}

/// A decoded value with its warnings.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded<T> {
    pub value: T,
    pub warnings: Vec<Problem>,
}

/// A request type's JSON Schema (draft 2020-12 as schemars writes it), kept so a command checks
/// many requests against one schema.
#[derive(Clone, Debug)]
pub struct Shape {
    root: Value,
}

impl Shape {
    /// The shape of `T` from its `JsonSchema`.
    pub fn of<T: JsonSchema>() -> Shape {
        Shape {
            root: schemars::schema_for!(T).to_value(),
        }
    }

    /// A shape from a schema written elsewhere (a game's declaration).
    pub fn from_schema(root: Value) -> Shape {
        Shape { root }
    }

    pub fn schema(&self) -> &Value {
        &self.root
    }

    /// Checks `value`; on success returns it normalized, else every problem of the shape phase
    /// combined into one (`Problem::combine`).
    pub fn check(&self, value: &Value, opts: &CheckOptions<'_>) -> Result<Checked, Problem> {
        let mut w = Walker::new(&self.root, opts);
        let mut v = value.clone();
        w.walk(&mut v, &self.root, &opts.base, None);
        match Problem::combine(w.problems) {
            Some(p) => Err(p),
            None => Ok(Checked {
                value: v,
                warnings: w.warnings,
            }),
        }
    }

    /// [`Shape::check`], then the typed decode. A decode that fails after the check passed means
    /// the schema and the decoder disagree, an engine bug: `internal.error`.
    pub fn decode<T: DeserializeOwned>(
        &self,
        value: &Value,
        opts: &CheckOptions<'_>,
    ) -> Result<Decoded<T>, Problem> {
        let checked = self.check(value, opts)?;
        match serde_json::from_value::<T>(checked.value) {
            Ok(v) => Ok(Decoded {
                value: v,
                warnings: checked.warnings,
            }),
            Err(e) => Err(codes::internal_error("decode", &e.to_string())),
        }
    }
}

/// Checks and decodes `value` as a `T` (builds `T`'s schema each call; cache a [`Shape`] where a
/// command is called often).
pub fn decode<T: DeserializeOwned + JsonSchema>(
    value: &Value,
    opts: &CheckOptions<'_>,
) -> Result<Decoded<T>, Problem> {
    Shape::of::<T>().decode(value, opts)
}

pub(super) struct Walker<'a> {
    root: &'a Value,
    opts: &'a CheckOptions<'a>,
    problems: Vec<Problem>,
    warnings: Vec<Problem>,
}

pub(super) const SAFE_INT: f64 = 9_007_199_254_740_991.0;

impl<'a> Walker<'a> {
    pub(super) fn new(root: &'a Value, opts: &'a CheckOptions<'a>) -> Walker<'a> {
        Walker {
            root,
            opts,
            problems: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Follows `$ref`s into `$defs` (or `definitions`).
    pub(super) fn resolve(&self, mut s: &'a Value) -> &'a Value {
        for _ in 0..32 {
            let Some(r) = s.get("$ref").and_then(Value::as_str) else {
                return s;
            };
            let target = r
                .strip_prefix("#/$defs/")
                .and_then(|n| self.root.get("$defs").and_then(|d| d.get(n)))
                .or_else(|| {
                    r.strip_prefix("#/definitions/")
                        .and_then(|n| self.root.get("definitions").and_then(|d| d.get(n)))
                })
                .or_else(|| (r == "#").then_some(self.root));
            match target {
                Some(t) => s = t,
                None => return &Value::Bool(true),
            }
        }
        s
    }

    pub(super) fn owner(&self, path: &Pointer) -> String {
        if let Some(f) = self.opts.owner_of
            && let Some(o) = f(path)
        {
            return o;
        }
        if *path == self.opts.base {
            return self.opts.owner.to_owned();
        }
        let rel: Vec<_> =
            path.segments()[self.opts.base.segments().len().min(path.segments().len())..].to_vec();
        let mut p = Pointer::root();
        for s in rel {
            p = match s {
                crate::pointer::Segment::Key(k) => p.key(&k),
                crate::pointer::Segment::Index(i) => p.index(i),
            };
        }
        p.dotted()
    }

    pub(super) fn walk(
        &mut self,
        value: &mut Value,
        schema: &'a Value,
        path: &Pointer,
        parent: Option<&'a Value>,
    ) {
        let s = self.resolve(schema);
        let obj = match s {
            Value::Bool(true) => return,
            Value::Bool(false) => {
                self.problems
                    .push(codes::not_applicable(path, "this place takes no value"));
                return;
            }
            Value::Object(o) => o,
            _ => return,
        };
        if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
            for b in all {
                self.walk(value, b, path, parent);
            }
        }
        if let Some(branches) = obj
            .get("oneOf")
            .or_else(|| obj.get("anyOf"))
            .and_then(Value::as_array)
        {
            self.branches(value, branches, path, parent);
            return;
        }
        if let Some(c) = obj.get("const") {
            if value != c && !self.alias_const(value, c, obj, path) {
                self.bad_value(value, std::slice::from_ref(c), path);
            }
            return;
        }
        if let Some(e) = obj.get("enum").and_then(Value::as_array) {
            if !e.contains(value) {
                self.bad_value(value, e, path);
            }
            return;
        }
        if !self.type_ok(value, obj, path) {
            return;
        }
        match value {
            Value::Number(_) => self.number(value, obj, path),
            Value::String(st) => {
                let len = st.chars().count();
                self.length(len, obj, "minLength", "maxLength", path);
            }
            Value::Array(items) => {
                self.length(items.len(), obj, "minItems", "maxItems", path);
                let prefix = obj.get("prefixItems").and_then(Value::as_array);
                let rest = obj.get("items");
                for (i, item) in items.iter_mut().enumerate() {
                    let sch = prefix.and_then(|p| p.get(i)).or(rest);
                    if let Some(sch) = sch {
                        self.walk(item, sch, &path.index(i), None);
                    }
                }
            }
            Value::Object(_) => self.object(value, s, obj, path, parent),
            _ => {}
        }
    }

    /// A const branch that declares the value as an alias (`x-aliases`) accepts it as the const.
    fn alias_const(
        &mut self,
        value: &mut Value,
        c: &Value,
        obj: &Map<String, Value>,
        path: &Pointer,
    ) -> bool {
        let (Some(v), Some(name)) = (value.as_str(), c.as_str()) else {
            return false;
        };
        if aliases(obj).contains(&v) {
            self.warnings.push(codes::alias_used(path, v, name));
            *value = c.clone();
            return true;
        }
        false
    }

    fn bad_value(&mut self, value: &Value, allowed: &[Value], path: &Pointer) {
        match value.as_str() {
            Some(got) if allowed.iter().all(Value::is_string) => {
                let names: Vec<&str> = allowed.iter().filter_map(Value::as_str).collect();
                let c: Vec<Candidate<'_>> = names.iter().map(|n| Candidate::new(n)).collect();
                self.problems.push(codes::invalid_value(path, got, &c));
            }
            _ => {
                let expected: Vec<&str> = allowed.iter().map(json_type).collect();
                let mut expected = expected;
                expected.dedup();
                if expected.contains(&json_type(value)) {
                    let text: Vec<String> = allowed.iter().map(Value::to_string).collect();
                    let c: Vec<Candidate<'_>> = text.iter().map(|n| Candidate::new(n)).collect();
                    self.problems
                        .push(codes::invalid_value(path, &value.to_string(), &c));
                } else {
                    self.problems.push(codes::wrong_type(
                        path,
                        &expected.join(" or "),
                        json_type(value),
                    ));
                }
            }
        }
    }

    fn type_ok(&mut self, value: &mut Value, obj: &Map<String, Value>, path: &Pointer) -> bool {
        let types = types_of(obj);
        if types.is_empty() {
            return true;
        }
        let got = json_type(value);
        if types.contains(&got) {
            return true;
        }
        if got == "number" && types.contains(&"integer") {
            let f = value.as_f64().unwrap_or(f64::NAN);
            if value.is_i64() || value.is_u64() {
                return true;
            }
            if f.fract() == 0.0 && f.abs() <= SAFE_INT {
                *value = Value::from(f as i64);
                return true;
            }
            self.problems.push(codes::not_integer(path, value));
            return false;
        }
        self.problems
            .push(codes::wrong_type(path, &types.join(" or "), got));
        false
    }

    fn number(&mut self, value: &Value, obj: &Map<String, Value>, path: &Pointer) {
        let x = value.as_f64().unwrap_or(0.0);
        let range = number_range(obj);
        if !range.contains(x) {
            self.problems
                .push(codes::out_of_range(path, value, &range, None));
        }
    }

    fn length(
        &mut self,
        len: usize,
        obj: &Map<String, Value>,
        min: &str,
        max: &str,
        path: &Pointer,
    ) {
        let range = Range {
            min: obj.get(min).and_then(Value::as_number).cloned(),
            max: obj.get(max).and_then(Value::as_number).cloned(),
            ..Range::default()
        };
        if !range.contains(len as f64) {
            let hint = "That is its length.";
            self.problems.push(codes::out_of_range(
                path,
                &Value::from(len),
                &range,
                Some(hint),
            ));
        }
    }
}

pub(super) fn types_of(obj: &Map<String, Value>) -> Vec<&str> {
    match obj.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The range a number schema admits: its bounds, else those its integer `format` implies, with
/// 64-bit integers limited to ±(2^53 - 1) so JSON readers keep them exact (README, Numbers).
pub(super) fn number_range(obj: &Map<String, Value>) -> Range {
    let n = |k: &str| obj.get(k).and_then(Value::as_number).cloned();
    let mut r = Range {
        min: n("minimum"),
        max: n("maximum"),
        min_exclusive: n("exclusiveMinimum"),
        max_exclusive: n("exclusiveMaximum"),
    };
    let (lo, hi): (i64, i64) = match obj.get("format").and_then(Value::as_str) {
        Some("uint8") => (0, 255),
        Some("uint16") => (0, 65_535),
        Some("uint32") => (0, 4_294_967_295),
        Some("uint64" | "uint" | "uint128") => (0, SAFE_INT as i64),
        Some("int8") => (-128, 127),
        Some("int16") => (-32_768, 32_767),
        Some("int32") => (-2_147_483_648, 2_147_483_647),
        Some("int64" | "int" | "int128") => (-(SAFE_INT as i64), SAFE_INT as i64),
        _ => return r,
    };
    if r.min.is_none() && r.min_exclusive.is_none() {
        r.min = Some(Number::from(lo));
    }
    if r.max.is_none() && r.max_exclusive.is_none() {
        r.max = Some(Number::from(hi));
    }
    r
}
