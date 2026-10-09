//! Perception as the action layer reads it (actions.md, Executors, rule 1; Affordances): an
//! [`ActorView`] over [`PerceptionView`], the action catalog's view factory, and the seats' bodies.
//! Validation, affordance requirements and executors therefore see exactly what the seat
//! perceives: facts at their visibility and detail, instruments, and the events in its ring.

use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_sim::{EntityId, EntityIndex, Tick};
use serde_json::{Value, json};

use super::state::{FactValue, Observer, Visibility};
use super::view::PerceptionView;
use crate::action::catalog::ViewFactory;
use crate::action::view::{ActorView, Known, KnownEvent, Visibility as Seen};

/// A fact value as the action layer reads it: numbers unrounded, a position as `{x, y, z}`.
pub fn fact_json(v: &FactValue) -> Value {
    match v {
        FactValue::Bool(b) => Value::Bool(*b),
        FactValue::Number(x) => serde_json::Number::from_f64(*x).map_or(Value::Null, Value::Number),
        FactValue::Text(s) => Value::String(s.clone()),
        FactValue::Position(p) => json!({"x": p[0], "y": p[1], "z": p[2]}),
    }
}

fn seen(v: Visibility) -> Seen {
    match v {
        Visibility::Seen => Seen::Seen,
        Visibility::Remembered => Seen::Remembered,
        Visibility::Charted => Seen::Charted,
    }
}

impl ActorView for PerceptionView<'_> {
    fn tick(&self) -> Tick {
        PerceptionView::tick(self)
    }

    fn body(&self) -> EntityId {
        PerceptionView::body(self)
    }

    fn instrument(&self, name: &str) -> Option<Value> {
        PerceptionView::instrument(self, name).map(|v| fact_json(&v))
    }

    fn known(&self, id: EntityId) -> Option<Known> {
        let p = self.percept(id)?;
        Some(Known {
            id,
            pos_m: self.position_of(id)?,
            name: p.name,
            kind: p.kind,
            visibility: seen(p.visibility),
            range_m: p.range_m,
            bearing_deg: p.bearing_deg,
            facts: p
                .facts
                .into_iter()
                .map(|r| (r.name, fact_json(&r.value)))
                .collect(),
        })
    }

    fn all_known(&self) -> Vec<Known> {
        let mut ids: Vec<EntityId> = self.percepts(&[]).into_iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.into_iter().filter_map(|id| self.known(id)).collect()
    }

    fn events_since(&self, seq: u64) -> Vec<KnownEvent> {
        self.ring_since(seq)
            .map(|r| KnownEvent {
                seq: r.event.seq,
                world_seq: r.world.map(|s| s.0),
                tick: r.event.tick,
                kind: r.event.kind.clone(),
                subject: r.event.subject.as_ref().map(|s| s.id),
                data: r
                    .event
                    .data
                    .iter()
                    .map(|(n, v)| (n.clone(), fact_json(v)))
                    .collect(),
            })
            .collect()
    }
}

/// The view of a body that observes nothing: it knows only itself.
struct Blind {
    tick: Tick,
    body: EntityId,
}

impl ActorView for Blind {
    fn tick(&self) -> Tick {
        self.tick
    }

    fn body(&self) -> EntityId {
        self.body
    }

    fn instrument(&self, _: &str) -> Option<Value> {
        None
    }

    fn known(&self, _: EntityId) -> Option<Known> {
        None
    }

    fn all_known(&self) -> Vec<Known> {
        Vec::new()
    }

    fn events_since(&self, _: u64) -> Vec<KnownEvent> {
        Vec::new()
    }
}

/// The action catalog's view factory: each seat acts through its observer's perception.
pub fn view_factory() -> ViewFactory {
    Arc::new(
        |world: &World, row: &crate::action::SeatRow| -> Box<dyn ActorView + '_> {
            match PerceptionView::of(world, row.body) {
                Some(v) => Box::new(v),
                None => Box::new(Blind {
                    tick: world
                        .get_resource::<pocket_sim::SimClock>()
                        .map_or(Tick(0), |c| c.tick),
                    body: row.body,
                }),
            }
        },
    )
}

/// The seats' bodies, `(seat, body)` in `EntityId` order: the entities whose observer names a
/// seat (perception.md, Observers).
pub fn bodies(world: &World) -> Vec<(String, EntityId)> {
    let Some(index) = world.get_resource::<EntityIndex>() else {
        return Vec::new();
    };
    index
        .iter()
        .filter_map(|(id, e)| Some((world.get::<Observer>(e)?.seat.clone()?, id)))
        .collect()
}
