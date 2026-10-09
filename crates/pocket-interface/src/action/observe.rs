//! What the action layer adds to perception's answers (shared/contract/perception.md, Queries;
//! projection.md, Text projection): the verbs available on a percept (`can`) and every verb with
//! why it is not available (`describe`), evaluated through the seat's own view; and the seat's
//! active and holding intents as `intent` lines and `IntentView`s, with their target entities,
//! which rank first in an observation.

use bevy_ecs::prelude::World;
use pocket_sim::{EntityId, PlainData};
use serde_json::Value;

use super::affordance;
use super::catalog::ActionCatalog;
use super::defs::ProgressDef;
use super::state::{IntentInstance, IntentTable, ResolvedTarget, SeatRow, seats, shown};
use super::view::ActorView;
use crate::perception::query::{AffordanceStatus, Affordances, SeatParts};
use crate::perception::state::FactValue;
use crate::perception::view::{Percept, PerceptionView};
use crate::projection::round::{self, Format};
use crate::projection::{Rendered, text};

/// The world's action catalog as perception's [`Affordances`]: a percept's `can` and `describe`'s
/// verbs, through the seat's own view (actions.md, Affordances).
pub struct CatalogAffordances<'w> {
    world: &'w World,
}

impl<'w> CatalogAffordances<'w> {
    pub fn new(world: &'w World) -> CatalogAffordances<'w> {
        CatalogAffordances { world }
    }

    fn seat(&self, view: &PerceptionView<'_>) -> Option<SeatRow> {
        let seat = view.seat()?;
        seats(self.world).ok()?.into_iter().find(|r| r.id == seat)
    }
}

impl Affordances for CatalogAffordances<'_> {
    fn can(&self, view: &PerceptionView<'_>, percept: &Percept) -> Vec<String> {
        let (Some(catalog), Some(seat)) =
            (self.world.get_resource::<ActionCatalog>(), self.seat(view))
        else {
            return Vec::new();
        };
        match ActorView::known(view, percept.id) {
            Some(k) => affordance::available(catalog, view, &seat, &k),
            None => Vec::new(),
        }
    }

    fn describe(&self, view: &PerceptionView<'_>, percept: &Percept) -> Vec<AffordanceStatus> {
        let (Some(catalog), Some(seat)) =
            (self.world.get_resource::<ActionCatalog>(), self.seat(view))
        else {
            return Vec::new();
        };
        let Some(k) = ActorView::known(view, percept.id) else {
            return Vec::new();
        };
        catalog
            .affordances_of(&k.kind)
            .iter()
            .map(|a| {
                let unmet = affordance::unmet(view, &seat, &k, a);
                AffordanceStatus {
                    verb: a.verb.clone(),
                    available: unmet.is_empty(),
                    unmet,
                }
            })
            .collect()
    }
}

fn fact(v: &PlainData) -> Option<FactValue> {
    Some(match v {
        PlainData::Bool(b) => FactValue::Bool(*b),
        PlainData::Number(x) => FactValue::Number(*x),
        PlainData::String(s) => FactValue::Text(s.clone()),
        _ => return None,
    })
}

fn format_of(defs: &[ProgressDef], name: &str) -> Format {
    defs.iter()
        .find(|d| d.name == name)
        .map_or(Format::number(2), ProgressDef::format)
}

/// The target as the seat knows it: `Name#id` (the name only when the seat knows the entity), or
/// a point `(x,z)` at one decimal.
fn target_text(view: &dyn ActorView, t: &ResolvedTarget) -> String {
    match t {
        ResolvedTarget::Entity(e) => {
            let name = view.known(*e).and_then(|k| k.name);
            text::reference(e.get(), name.as_deref())
        }
        ResolvedTarget::Point(p) => {
            format!("({},{})", round::fixed(p[0], 1), round::fixed(p[2], 1))
        }
    }
}

fn target_json(view: &dyn ActorView, t: &ResolvedTarget) -> String {
    match t {
        ResolvedTarget::Entity(e) => {
            let mut o = serde_json::Map::new();
            o.insert("id".into(), Value::from(e.get()));
            if let Some(n) = view.known(*e).and_then(|k| k.name) {
                o.insert("name".into(), Value::String(n));
            }
            Value::Object(o).to_string()
        }
        ResolvedTarget::Point(p) => format!(
            "{{\"x\":{},\"z\":{}}}",
            round::json_fixed(p[0], 1),
            round::json_fixed(p[2], 1)
        ),
    }
}

/// One live intent in both projections (projection.md: `intent := "intent #" ID " " INTENT " "
/// STATUS [" target=" (REF | POINT)] [" tag=" VALUE] (" " NAME "=" VALUE)* NL`; the JSON is its
/// `IntentView`).
pub fn rendered(view: &dyn ActorView, catalog: &ActionCatalog, i: &IntentInstance) -> Rendered {
    let defs = catalog
        .intent(&i.intent)
        .map(|e| e.def.progress.as_slice())
        .unwrap_or_default();
    let mut line = format!("intent #{} {} {}", i.id, i.intent, i.status.name());
    if let Some(t) = &i.target {
        line.push_str(" target=");
        line.push_str(&target_text(view, t));
    }
    if let Some(t) = &i.tag {
        line.push_str(" tag=");
        line.push_str(&round::text_word(t));
    }
    let mut readings = Vec::new();
    for (name, v) in &i.progress {
        let Some(f) = fact(v) else { continue };
        let format = format_of(defs, name);
        line.push(' ');
        line.push_str(name);
        line.push('=');
        line.push_str(&round::text(&f, format));
        readings.push(format!(
            "{{\"name\":{},\"value\":{}}}",
            Value::String(name.clone()),
            round::json(&f, format)
        ));
    }
    let mut json = format!(
        "{{\"id\":{},\"intent\":{},\"status\":\"{}\"",
        i.id,
        Value::String(i.intent.clone()),
        i.status.name()
    );
    if let Some(t) = &i.tag {
        json.push_str(&format!(",\"tag\":{}", Value::String(t.clone())));
    }
    json.push_str(&format!(",\"params\":{}", i.params.to_json()));
    if let Some(t) = &i.target {
        json.push_str(&format!(",\"target\":{}", target_json(view, t)));
    }
    json.push_str(&format!(
        ",\"started_tick\":{},\"progress\":[{}]}}",
        i.started_tick.0,
        readings.join(",")
    ));
    Rendered { json, text: line }
}

/// The seat's part of an observation: its active and holding intents, oldest first, and their
/// target entities; `decision` is the time layer's rendering of a pending decision point.
pub fn seat_parts(world: &World, seat: &str, decision: Option<Rendered>) -> SeatParts {
    let (Some(catalog), Some(table)) = (
        world.get_resource::<ActionCatalog>(),
        world.get_resource::<IntentTable>(),
    ) else {
        return SeatParts {
            decision,
            ..SeatParts::default()
        };
    };
    let Some(row) = seats(world)
        .ok()
        .and_then(|rows| rows.into_iter().find(|r| r.id == seat))
    else {
        return SeatParts {
            decision,
            ..SeatParts::default()
        };
    };
    let view = (catalog.view)(world, &row);
    let live: Vec<&IntentInstance> = table.live_of(seat).collect();
    let mut targets: Vec<EntityId> = live
        .iter()
        .filter_map(|i| i.target.and_then(|t| t.entity()))
        .filter(|e| view.known(*e).is_some())
        .collect();
    targets.dedup();
    SeatParts {
        intents: live.iter().map(|i| rendered(&*view, catalog, i)).collect(),
        decision,
        targets,
    }
}

/// A failure kept in an instance, shown: its message rendered from its code's template.
pub fn failure_shown(
    catalog: &ActionCatalog,
    i: &IntentInstance,
) -> Option<pocket_contract::Problem> {
    i.failure.as_ref().map(|s| {
        let p = catalog.render(s);
        if p.message.is_empty() {
            shown(s, format!("The intent failed with {}.", s.code))
        } else {
            p
        }
    })
}
