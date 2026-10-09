//! References and values in an act and the requests beside it (shared/contract/actions.md, The
//! act request, phase 2; README, Positions and entity references, Seats and callers): an entity
//! named among what the seat knows, the seat a caller acts for, a control's value against its
//! control, and an intent's parameters against its schema with their defaults filled.

use pocket_contract::codes::{self, Range};
use pocket_contract::{CheckOptions, Pointer, Problem};
use pocket_sim::EntityId;
use serde_json::{Map, Value, json};

use super::defs::{ControlKind, IntentDef};
use super::request::{Caller, EntityRef, WireValue};
use super::state::{ControlValue, SeatRow, seats};
use super::view::{ActorView, Known};

pub(crate) fn param_names(def: &IntentDef) -> Vec<String> {
    def.params
        .schema()
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default()
}

/// An intent's parameters checked against its schema, defaults filled: the canonical map, and the
/// warnings (aliases used).
pub fn check_params(
    def: &IntentDef,
    params: &Map<String, Value>,
    at: &Pointer,
) -> Result<(Map<String, Value>, Vec<Problem>), Problem> {
    let owner = def.name.clone();
    let owner_of = move |p: &Pointer| (p.segments().len() <= 3).then(|| owner.clone());
    let opts = CheckOptions {
        owner: &def.name,
        base: at.clone(),
        owner_of: Some(&owner_of),
    };
    let checked = def.params.check(&Value::Object(params.clone()), &opts)?;
    let mut canonical = def.defaults.clone();
    if let Value::Object(m) = checked.value {
        canonical.extend(m);
    }
    Ok((canonical, checked.warnings))
}

/// Resolves an entity reference among what the actor knows: `perception.unknown_entity` (with
/// suggestions from known names only) or `request.ambiguous_ref`.
pub fn resolve(view: &dyn ActorView, r: &EntityRef, path: &Pointer) -> Result<Known, Problem> {
    let text = match r {
        EntityRef::Id(n) => n.to_string(),
        EntityRef::Text(s) => s.clone(),
        EntityRef::Named(n) => match &n.name {
            Some(name) => format!("{name}#{}", n.id),
            None => format!("#{}", n.id),
        },
    };
    let known = view.all_known();
    let unknown = || {
        let names: Vec<&str> = known.iter().filter_map(|k| k.name.as_deref()).collect();
        codes::unknown_entity(path, &text, names)
    };
    let (name, id): (Option<String>, Option<u64>) = match r {
        EntityRef::Id(n) => (None, Some(*n)),
        EntityRef::Named(n) => (n.name.clone(), Some(n.id)),
        EntityRef::Text(s) => match s.rsplit_once('#') {
            Some((name, id)) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => (
                (!name.is_empty()).then(|| name.to_owned()),
                id.parse().ok().or(Some(0)),
            ),
            _ => (Some(s.clone()), None),
        },
    };
    if let Some(id) = id {
        let found = EntityId::new(id).and_then(|e| view.known(e));
        return match found {
            Some(k) if name.as_ref().is_none_or(|n| k.name.as_ref() == Some(n)) => Ok(k),
            _ => Err(unknown()),
        };
    }
    let name = name.unwrap_or_default();
    let matches: Vec<&Known> = known
        .iter()
        .filter(|k| k.name.as_deref() == Some(name.as_str()))
        .collect();
    match matches.as_slice() {
        [] => Err(unknown()),
        [one] => Ok((*one).clone()),
        many => {
            let c: Vec<(u64, &str, &str)> = many
                .iter()
                .map(|k| (k.id.get(), k.name.as_deref().unwrap_or(""), k.kind.as_str()))
                .collect();
            Err(codes::ambiguous_ref(path, &text, &c))
        }
    }
}

/// The seat a caller acts for: a player its own (`seat.not_yours` for another), a developer the
/// one it names (`request.missing_field` without one), a checker none (`permission.denied`).
pub fn seat_for(
    world: &bevy_ecs::prelude::World,
    caller: &Caller,
    seat: Option<&str>,
    request: &str,
) -> Result<SeatRow, Problem> {
    let all = seats(world)?;
    let path = Pointer::root().key("seat");
    let unknown = |s: &str| {
        let c: Vec<pocket_contract::Candidate<'_>> = all
            .iter()
            .map(|r| pocket_contract::Candidate::new(&r.id))
            .collect();
        codes::seat_unknown(&path, s, &c)
    };
    match caller {
        Caller::Checker if request == "act" => Err(codes::permission_denied(
            request,
            "checker",
            "player or developer",
        )),
        Caller::Player { seat: mine } => {
            let own = all
                .iter()
                .find(|r| r.id == *mine)
                .ok_or_else(|| unknown(mine))?;
            match seat {
                Some(s) if s != own.id => Err(codes::seat_not_yours(&path, s, &own.id)),
                _ => Ok(own.clone()),
            }
        }
        Caller::Developer | Caller::Checker => match seat {
            None => Err(codes::missing_field(
                &path,
                &format!("the {request} request"),
                "a seat id",
            )),
            Some(s) => all
                .iter()
                .find(|r| r.id == s)
                .cloned()
                .ok_or_else(|| unknown(s)),
        },
    }
}

/// A control value as the world latches it. A number's negative zero becomes positive zero (`x +
/// 0.0`): the canonical record writes a control's value as the wire does, where `-0.0` is the
/// integer `0`, so the value applied live and the one a replay applies carry the same bits
/// (persistence.md hashes an `f64` by its bits; actions-slice2.md 11).
pub(crate) fn wire_value(v: &WireValue) -> ControlValue {
    match v {
        WireValue::Bool(b) => ControlValue::Bool(*b),
        WireValue::Number(x) => ControlValue::Number(*x + 0.0),
        WireValue::Name(s) => ControlValue::Name(s.clone()),
    }
}

pub(crate) fn value_problem(kind: &ControlKind, got: &WireValue, path: &Pointer) -> Problem {
    let json = match got {
        WireValue::Bool(b) => json!(b),
        WireValue::Number(x) => codes::num(*x),
        WireValue::Name(s) => json!(s),
    };
    match (kind, got) {
        (ControlKind::Axis { min, max, .. }, WireValue::Number(_)) => {
            let range = Range {
                min: serde_json::Number::from_f64(*min),
                max: serde_json::Number::from_f64(*max),
                ..Range::default()
            };
            codes::out_of_range(path, &json, &range, None)
        }
        (ControlKind::Choice { values, .. }, WireValue::Name(s)) => {
            let c: Vec<pocket_contract::Candidate<'_>> = values
                .iter()
                .map(|v| pocket_contract::Candidate::new(v))
                .collect();
            codes::invalid_value(path, s, &c)
        }
        (ControlKind::Axis { .. }, _) => codes::wrong_type(path, "number", json_type(&json)),
        (ControlKind::Toggle { .. }, _) => codes::wrong_type(path, "boolean", json_type(&json)),
        (ControlKind::Choice { .. }, _) => codes::wrong_type(path, "string", json_type(&json)),
        (ControlKind::Pulse { .. }, _) => codes::not_applicable(
            path,
            "it is a pulse control; send it with {\"do\": \"pulse\"}",
        ),
    }
}

fn json_type(v: &Value) -> &'static str {
    pocket_contract::render::json_type(v)
}
