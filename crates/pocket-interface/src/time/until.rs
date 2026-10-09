//! `until` (shared/contract/time.md, `until`): checked at the request against what the caller's
//! seat can know, and evaluated after each tick through its perception, at the precision a fact
//! declares, so a player can wait only on what it can know.

use pocket_contract::codes::{self, unknown_intent_id};
use pocket_contract::{Candidate, Pointer, Problem};
use pocket_sim::Tick;
use serde_json::{Value, json};

use super::play::{CondOp, FactCondition, Until, UntilMet, kind_matches};
use crate::action::request::EntityRef;
use crate::action::state::{IntentStatus, IntentTable};
use crate::action::validate::resolve;
use crate::action::view::{ActorView, KnownEvent};
use crate::perception::{Exposure, PerceptionDefs, Unit};

/// What a condition reads now: the value of its fact, or `None` when the seat does not know it.
fn read(view: &dyn ActorView, c: &FactCondition) -> Option<Value> {
    match &c.entity {
        None => view.instrument(&c.name),
        Some(r) => {
            let k = resolve(view, r, &Pointer::root()).ok()?;
            k.fact(&c.name).cloned()
        }
    }
}

/// The fact's declared precision and unit, when the game declares it.
fn declared<'a>(
    defs: Option<&'a PerceptionDefs>,
    kind: Option<&str>,
    name: &str,
) -> Option<(&'a Unit, u8, Exposure)> {
    let defs = defs?;
    match kind {
        None => defs
            .instrument(name)
            .map(|i| (&i.unit, i.precision, Exposure::Owner)),
        Some(k) => defs
            .kind(k)?
            .facts
            .iter()
            .find(|f| f.name == name)
            .map(|f| (&f.unit, f.precision, f.exposure)),
    }
}

fn rounded(v: Value, precision: Option<u8>) -> Value {
    match (v.as_f64(), precision) {
        (Some(x), Some(p)) => json!(crate::projection::round::stored(x, p)),
        _ => v,
    }
}

fn numeric_op(op: CondOp) -> bool {
    matches!(
        op,
        CondOp::Above | CondOp::Below | CondOp::AtLeast | CondOp::AtMost
    )
}

fn op_names() -> [&'static str; 7] {
    [
        "equals",
        "not_equals",
        "above",
        "below",
        "at_least",
        "at_most",
        "changes",
    ]
}

/// Checks `until` at the request: an intent the seat had, an entity it knows, a fact it may know,
/// an operation the fact's type takes and a value of its type.
pub fn check(
    view: &dyn ActorView,
    defs: Option<&PerceptionDefs>,
    table: &IntentTable,
    seat: &str,
    u: &Until,
    path: &Pointer,
) -> Result<(), Problem> {
    match u {
        Until::Decision | Until::Event(_) => Ok(()),
        Until::Intent(r) => {
            let at = path.key("intent");
            match r.id() {
                Some(id) if table.by_id.get(&id).is_some_and(|i| i.seat == seat) => Ok(()),
                Some(id) => Err(unknown_intent_id(&at, id)),
                None => Err(codes::wrong_type(&at, "an intent id", "string")),
            }
        }
        Until::Any(list) => {
            for (i, x) in list.iter().enumerate() {
                check(view, defs, table, seat, x, &path.key("any").index(i))?;
            }
            Ok(())
        }
        Until::Fact(c) => {
            let at = path.key("fact");
            let known = match &c.entity {
                None => None,
                Some(r) => Some(resolve(view, r, &at.key("entity"))?),
            };
            let kind = known.as_ref().map(|k| k.kind.clone());
            // An `Owner` fact is shown only to its owner (perception.md, Exposure): of another
            // entity it is as hidden as a `Hidden` one, and a run waiting on it would never stop.
            let own = known.as_ref().is_some_and(|k| k.id == view.body());
            let decl = declared(defs, kind.as_deref(), &c.name);
            let hidden = match (&decl, defs) {
                (Some((_, _, Exposure::Hidden)), _) => true,
                (Some((_, _, Exposure::Owner)), _) if known.is_some() => !own,
                (Some(_), _) => false,
                (None, Some(_)) => true,
                (None, None) => read(view, c).is_none(),
            };
            if hidden || (c.entity.is_none() && view.instrument(&c.name).is_none()) {
                let entity = match &c.entity {
                    Some(r) => resolve(view, r, &at.key("entity"))
                        .ok()
                        .map(|k| k.entity_name()),
                    None => None,
                };
                return Err(codes::not_perceivable(
                    &at.key("name"),
                    &c.name,
                    entity.as_ref(),
                ));
            }
            let current = read(view, c);
            let kind_of = |v: &Value| match v {
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                _ => "object",
            };
            let fact_type = match decl.map(|d| d.0) {
                Some(Unit::Bool) => Some("boolean"),
                Some(Unit::Text | Unit::Enum { .. }) => Some("string"),
                Some(Unit::Position) => Some("object"),
                Some(_) => Some("number"),
                None => current.as_ref().map(kind_of),
            };
            if numeric_op(c.op) && fact_type.is_some_and(|t| t != "number") {
                let names = op_names();
                let ok: Vec<Candidate<'_>> = names
                    .iter()
                    .filter(|n| matches!(**n, "equals" | "not_equals" | "changes"))
                    .map(|n| Candidate::new(n))
                    .collect();
                let got = serde_json::to_value(c.op)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                return Err(codes::invalid_value(&at.key("op"), &got, &ok));
            }
            match (&c.value, c.op) {
                (None, CondOp::Changes) => Ok(()),
                (None, _) => Err(codes::missing_field(
                    &at.key("value"),
                    "the fact condition",
                    "the value to compare with",
                )),
                (Some(v), _) => match fact_type {
                    Some(t) if t != kind_of(v) => {
                        Err(codes::wrong_type(&at.key("value"), t, kind_of(v)))
                    }
                    _ => Ok(()),
                },
            }
        }
    }
}

/// The values the `changes` conditions start from, in the order `met` visits them.
pub fn baseline(view: &dyn ActorView, u: &Until) -> Vec<Option<Value>> {
    let mut out = Vec::new();
    collect(view, u, &mut out);
    out
}

fn collect(view: &dyn ActorView, u: &Until, out: &mut Vec<Option<Value>>) {
    match u {
        Until::Fact(c) => out.push(read(view, c)),
        Until::Any(list) => list.iter().for_each(|x| collect(view, x, out)),
        _ => {}
    }
}

/// What the tick brought the caller's seat.
pub struct TickNews<'a> {
    pub tick: Tick,
    /// A decision point arose for the seat in this tick.
    pub decision: bool,
    /// The events the seat perceived in this tick.
    pub events: &'a [KnownEvent],
}

/// Whether `u` holds after the tick: the condition (of an `any`, the first that held) with its
/// value or event.
pub fn met(
    view: &dyn ActorView,
    defs: Option<&PerceptionDefs>,
    table: &IntentTable,
    u: &Until,
    base: &[Option<Value>],
    news: &TickNews<'_>,
) -> Option<UntilMet> {
    let mut i = 0;
    met_at(view, defs, table, u, base, &mut i, news)
}

fn met_at(
    view: &dyn ActorView,
    defs: Option<&PerceptionDefs>,
    table: &IntentTable,
    u: &Until,
    base: &[Option<Value>],
    i: &mut usize,
    news: &TickNews<'_>,
) -> Option<UntilMet> {
    let hit = |value: Option<Value>, event: Option<Value>| {
        Some(UntilMet {
            tick: news.tick,
            condition: u.clone(),
            value,
            event,
        })
    };
    match u {
        Until::Decision => news.decision.then(|| hit(None, None)).flatten(),
        Until::Intent(r) => {
            let i = table.by_id.get(&r.id()?)?;
            (i.status != IntentStatus::Active)
                .then(|| hit(None, None))
                .flatten()
        }
        Until::Event(kind) => {
            let e = news.events.iter().find(|e| kind_matches(kind, &e.kind))?;
            hit(None, Some(event_json(e)))
        }
        Until::Any(list) => {
            for x in list {
                if let Some(m) = met_at(view, defs, table, x, base, i, news) {
                    return Some(m);
                }
            }
            None
        }
        Until::Fact(c) => {
            let start = base.get(*i).cloned().flatten();
            *i += 1;
            let kind = c
                .entity
                .as_ref()
                .and_then(|r| resolve(view, r, &Pointer::root()).ok())
                .map(|k| k.kind);
            let precision = declared(defs, kind.as_deref(), &c.name).map(|d| d.1);
            let now = rounded(read(view, c)?, precision);
            let holds = match (c.op, &c.value) {
                (CondOp::Changes, _) => Some(&now) != start.map(|s| rounded(s, precision)).as_ref(),
                (op, Some(v)) => compare(op, &now, v),
                (_, None) => false,
            };
            holds.then(|| hit(Some(now), None)).flatten()
        }
    }
}

fn compare(op: CondOp, now: &Value, v: &Value) -> bool {
    match (now.as_f64(), v.as_f64()) {
        (Some(a), Some(b)) => match op {
            CondOp::Equals => a == b,
            CondOp::NotEquals => a != b,
            CondOp::Above => a > b,
            CondOp::Below => a < b,
            CondOp::AtLeast => a >= b,
            CondOp::AtMost => a <= b,
            CondOp::Changes => false,
        },
        _ => match op {
            CondOp::Equals => now == v,
            CondOp::NotEquals => now != v,
            _ => false,
        },
    }
}

/// A perceived event as `until`'s answer names it.
pub fn event_json(e: &KnownEvent) -> Value {
    let data: serde_json::Map<String, Value> = e.data.iter().cloned().collect();
    let mut v = json!({"seq": e.seq, "tick": e.tick.0, "kind": e.kind, "data": data});
    if let Some(s) = e.subject {
        v["subject"] = json!({"id": s.get()});
    }
    v
}

/// An entity reference for a fact condition, as text, for messages.
pub fn entity_text(r: &EntityRef) -> String {
    match r {
        EntityRef::Id(n) => n.to_string(),
        EntityRef::Text(s) => s.clone(),
        EntityRef::Named(n) => format!("#{}", n.id),
    }
}
