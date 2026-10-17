//! The JSON projection (shared/contract/projection.md, JSON projection): perception.md's answer
//! types in field declaration order, compact, optional fields omitted when `None` (but
//! `omniscient`, always present), every value rounded at its precision where it is written.

use super::round::{self, Format};
use super::{Header, Omitted};
use crate::perception::query::AffordanceStatus;
use crate::perception::state::Named;
use crate::perception::view::{EventView, Percept, Reading};

/// A JSON object written member by member.
pub struct Obj {
    s: String,
    empty: bool,
}

impl Default for Obj {
    fn default() -> Self {
        Obj::new()
    }
}

impl Obj {
    pub fn new() -> Obj {
        Obj {
            s: "{".to_owned(),
            empty: true,
        }
    }

    /// A member whose value is already JSON.
    pub fn raw(&mut self, key: &str, json: &str) -> &mut Obj {
        if !self.empty {
            self.s.push(',');
        }
        self.empty = false;
        self.s.push_str(&round::quote(key));
        self.s.push(':');
        self.s.push_str(json);
        self
    }

    pub fn str(&mut self, key: &str, v: &str) -> &mut Obj {
        self.raw(key, &round::quote(v))
    }

    pub fn uint(&mut self, key: &str, v: u64) -> &mut Obj {
        self.raw(key, &v.to_string())
    }

    pub fn bool(&mut self, key: &str, v: bool) -> &mut Obj {
        self.raw(key, if v { "true" } else { "false" })
    }

    /// A member holding an array of JSON values.
    pub fn list(&mut self, key: &str, items: &[String]) -> &mut Obj {
        self.raw(key, &array(items))
    }

    pub fn finish(&mut self) -> String {
        let mut s = std::mem::take(&mut self.s);
        s.push('}');
        s
    }
}

/// `[a,b,c]`.
pub fn array(items: &[String]) -> String {
    let mut s = String::with_capacity(2 + items.iter().map(|i| i.len() + 1).sum::<usize>());
    s.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(item);
    }
    s.push(']');
    s
}

/// The bytes an array of items of these lengths takes: brackets and commas included.
pub fn array_len(lengths: impl Iterator<Item = usize>) -> usize {
    let mut n = 0usize;
    let mut total = 2usize;
    for l in lengths {
        total += l;
        n += 1;
    }
    total + n.saturating_sub(1)
}

/// `EntityName`: `{"id": .., "name": ..}`, `name` omitted when it has none.
pub fn named(n: &Named) -> String {
    let mut o = Obj::new();
    o.uint("id", n.id.get());
    if let Some(name) = &n.name {
        o.str("name", name);
    }
    o.finish()
}

/// `Reading`: `{"name": .., "value": ..}`, the value untagged and rounded.
pub fn reading(r: &Reading) -> String {
    Obj::new()
        .str("name", &r.name)
        .raw("value", &round::json(&r.value, r.format))
        .finish()
}

/// A reported bearing in JSON: at the precision of its range (projection.md, Rounding).
fn bearing(b: f64, range: f64) -> String {
    round::json_bearing(b, round::range_precision(range))
}

fn range(r: f64) -> String {
    round::json_fixed(r, round::range_precision(r))
}

/// `Percept`.
pub fn percept(p: &Percept) -> String {
    let mut o = Obj::new();
    o.uint("id", p.id.get());
    if let Some(n) = &p.name {
        o.str("name", n);
    }
    o.str("kind", &p.kind)
        .str("visibility", p.visibility.name())
        .str("detail", p.detail.name())
        .raw("bearing_deg", &bearing(p.bearing_deg, p.range_m))
        .raw("range_m", &range(p.range_m));
    if let Some(pos) = p.pos_m {
        o.raw("pos_m", &round::json_position(pos, 1));
    }
    if let Some(age) = p.age_s {
        o.raw("age_s", &round::json_fixed(age, 1));
    }
    let facts: Vec<String> = p.facts.iter().map(reading).collect();
    let can: Vec<String> = p.can.iter().map(|v| round::quote(v)).collect();
    o.list("facts", &facts).list("can", &can).finish()
}

/// `PerceivedEvent`.
pub fn event(e: &EventView) -> String {
    let mut o = Obj::new();
    o.uint("seq", e.seq)
        .uint("tick", e.tick.0)
        .str("kind", &e.kind);
    if let Some(n) = &e.subject {
        o.raw("subject", &named(n));
    }
    o.str("sense", e.sense.name());
    if let (Some(b), Some(r)) = (e.bearing_deg, e.range_m) {
        o.raw("bearing_deg", &bearing(b, r))
            .raw("range_m", &range(r));
    }
    let data: Vec<String> = e.data.iter().map(reading).collect();
    o.list("data", &data);
    if let Some(c) = e.cause {
        o.uint("cause", c);
    }
    o.finish()
}

/// `Omitted`: always written in JSON.
pub fn omitted(om: &Omitted) -> String {
    let mut kinds = Obj::new();
    for (k, n) in &om.entities {
        kinds.uint(k, u64::from(*n));
    }
    Obj::new()
        .raw("entities", &kinds.finish())
        .uint("events", u64::from(om.events))
        .uint("lost", u64::from(om.lost))
        .finish()
}

/// One verb's state in a `describe` answer: `{"verb", "available", "unmet"}`.
pub fn affordance(a: &AffordanceStatus) -> String {
    let unmet: Vec<String> = a
        .unmet
        .iter()
        .map(|p| serde_json::to_string(p).unwrap_or_else(|_| "null".to_owned()))
        .collect();
    Obj::new()
        .str("verb", &a.verb)
        .bool("available", a.available)
        .list("unmet", &unmet)
        .finish()
}

/// The members every observation starts with: `tick`, `t_s`, `seat`, `observer`, `omniscient`.
pub fn header(o: &mut Obj, h: &Header) {
    o.uint("tick", h.tick.0)
        .raw("t_s", &round::json_fixed(h.t_s, 3));
    if let Some(s) = &h.seat {
        o.str("seat", s);
    }
    if let Some(n) = &h.observer {
        o.raw("observer", &named(n));
    }
    o.bool("omniscient", h.omniscient);
}

/// A number at its format, for parts other layers write (an intent's progress).
pub fn value(r: &crate::perception::state::FactValue, f: Format) -> String {
    round::json(r, f)
}
