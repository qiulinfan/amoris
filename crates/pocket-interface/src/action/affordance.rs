//! Affordances (shared/contract/actions.md, Affordances): which verbs an entity's kind offers, and
//! whether each is available to an actor now, with every unmet requirement as its own problem.
//! Requirements are evaluated through the actor's view only, so they depend on nothing it cannot
//! perceive.

use pocket_contract::codes::{self, EntityName};
use pocket_contract::{Pointer, Problem};
use serde_json::{Value, json};

use super::catalog::ActionCatalog;
use super::defs::{AffordanceDef, Requirement, Side};
use super::request::AffordancesRequest;
use super::state::SeatRow;
use super::view::{ActorView, Known, Visibility};

/// Rounds a distance or an angle as the contract shows it (one decimal).
fn tenth(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// The relative angle of `bearing` off `heading`, in (-180, 180] (perception.md, Geometry).
pub fn relative(bearing: f64, heading: f64) -> f64 {
    let a = bearing - heading;
    if a > 180.0 {
        a - 360.0
    } else if a <= -180.0 {
        a + 360.0
    } else {
        a
    }
}

/// Every requirement of `aff` that `target` does not meet for the actor, in declared order.
pub fn unmet(
    view: &dyn ActorView,
    seat: &SeatRow,
    target: &Known,
    aff: &AffordanceDef,
) -> Vec<Problem> {
    let name = target.entity_name();
    let mut out = Vec::new();
    for r in &aff.requires {
        match r {
            Requirement::Seen => {
                if target.visibility != Visibility::Seen {
                    out.push(codes::not_seen(&name));
                }
            }
            Requirement::Within { max_m } => {
                if target.range_m > *max_m {
                    out.push(codes::out_of_reach(&name, tenth(target.range_m), *max_m));
                }
            }
            Requirement::Facing { max_off_deg } => {
                let heading = super::view::number(view, "heading_deg").unwrap_or(0.0);
                let off = relative(target.bearing_deg, heading).abs();
                if off > *max_off_deg {
                    out.push(codes::not_facing(&name, tenth(off), *max_off_deg));
                }
            }
            Requirement::Fact {
                of,
                fact,
                op,
                value,
            } => {
                let (who, actual) = match of {
                    Side::Target => (name.clone(), target.fact(fact).cloned()),
                    Side::Actor => (
                        EntityName::new(view.body().get(), None),
                        view.instrument(fact),
                    ),
                };
                let actual = actual.unwrap_or(Value::Null);
                if !op.holds(&actual, value) {
                    out.push(codes::requirement_unmet(
                        &aff.verb,
                        &who,
                        fact,
                        op.name(),
                        value.clone(),
                        actual,
                    ));
                }
            }
            Requirement::Seats { seats } => {
                if !seats.contains(&seat.id) {
                    let list: Vec<&str> = seats.iter().map(String::as_str).collect();
                    out.push(codes::seat_not_allowed(&seat.id, &list));
                }
            }
        }
    }
    out
}

/// `action.unavailable` for `aff` on `target` when a requirement is unmet.
pub fn check(
    view: &dyn ActorView,
    seat: &SeatRow,
    target: &Known,
    aff: &AffordanceDef,
    path: &Pointer,
) -> Result<(), Problem> {
    let unmet = unmet(view, seat, target, aff);
    if unmet.is_empty() {
        Ok(())
    } else {
        Err(codes::unavailable(
            path,
            &target.entity_name(),
            &aff.verb,
            &unmet,
        ))
    }
}

/// The verbs available now on `target` (a percept's `can`).
pub fn available(
    catalog: &ActionCatalog,
    view: &dyn ActorView,
    seat: &SeatRow,
    target: &Known,
) -> Vec<String> {
    catalog
        .affordances_of(&target.kind)
        .iter()
        .filter(|a| unmet(view, seat, target, a).is_empty())
        .map(|a| a.verb.clone())
        .collect()
}

/// The `affordances` answer's list (actions.md, Affordances): for each known entity matching the
/// request, ranked seen before remembered before charted, then nearer first, then by id, each verb
/// its kind offers with `available` and every unmet requirement.
pub fn list(
    catalog: &ActionCatalog,
    view: &dyn ActorView,
    seat: &SeatRow,
    req: &AffordancesRequest,
    only: Option<&Known>,
) -> Vec<Value> {
    let mut entities: Vec<Known> = match only {
        Some(k) => vec![k.clone()],
        None => view.all_known(),
    };
    entities.retain(|k| {
        req.kinds.as_ref().is_none_or(|ks| ks.contains(&k.kind))
            && req.within_m.is_none_or(|w| k.range_m <= w)
            && !catalog.affordances_of(&k.kind).is_empty()
    });
    entities.sort_by(|a, b| {
        a.visibility
            .cmp(&b.visibility)
            .then(tenth(a.range_m).total_cmp(&tenth(b.range_m)))
            .then(a.id.cmp(&b.id))
    });
    let mut out = Vec::new();
    for k in &entities {
        for aff in catalog.affordances_of(&k.kind) {
            let unmet = unmet(view, seat, k, aff);
            if req.available_only == Some(true) && !unmet.is_empty() {
                continue;
            }
            out.push(json!({
                "entity": k.entity_name(),
                "kind": k.kind,
                "verb": aff.verb,
                "available": unmet.is_empty(),
                "unmet": unmet,
            }));
        }
    }
    out
}
