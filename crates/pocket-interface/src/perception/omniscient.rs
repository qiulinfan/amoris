//! The raw omniscient view (shared/contract/perception.md, The omniscient view): every entity,
//! `Perceivable` or not, with every fact including hidden ones, and the world's events including
//! hidden and undeclared ones, with no range, field of view, occlusion or memory. For developers,
//! tools and benchmark checkers only; every answer it shapes is marked `omniscient: true`.
//!
//! It has no observer: bearings and ranges are measured from the named seat's body when the
//! request names one, else from the world's origin (docs/spec/perception-slice2.md 5). Its events
//! are those the world still holds, the inbox (simulation.md 5): the last tick's and the
//! boundary's since; their `seq` is spec-sim's `EventSeq`, the number `why` takes.

use std::collections::BTreeMap;

use bevy_ecs::prelude::{Entity, World};
use pocket_physics::geom::{V3, ZERO};
use pocket_sim::{EntityId, EntityIndex, Event, EventInbox, SimClock};

use super::defs::PerceptionDefs;
use super::facts::{self, Body, FactCx, NO_FACTS};
use super::geometry::bearing_range;
use super::state::{Detail, FactValue, Named, Perceivable, Sense, Visibility};
use super::view::{EventView, Percept, Reading, rank};
use crate::projection::round::Format;

/// The world seen whole.
pub struct Raw<'w> {
    pub(crate) world: &'w World,
    pub(crate) defs: Option<&'w PerceptionDefs>,
    /// The seat's body the measures are taken from, if the request named a seat.
    pub(crate) origin: Option<Body>,
    pub(crate) clock: SimClock,
}

impl<'w> Raw<'w> {
    pub fn new(world: &'w World, origin: Option<EntityId>) -> Option<Raw<'w>> {
        let origin = origin.and_then(|id| {
            let e = facts::live(world, id)?;
            Some(Body {
                id,
                entity: e,
                position: facts::position(world, e).unwrap_or(ZERO),
            })
        });
        Some(Raw {
            world,
            defs: world.get_resource::<PerceptionDefs>(),
            origin,
            clock: *world.get_resource::<SimClock>()?,
        })
    }

    fn from(&self) -> V3 {
        self.origin.map_or(ZERO, |b| b.position)
    }

    /// The entity whole.
    pub fn percept(&self, id: EntityId, e: Entity) -> Percept {
        let w = self.world;
        let at = facts::position(w, e).unwrap_or(ZERO);
        let (bearing_deg, range_m) = bearing_range(self.from(), at);
        let perc = w.get::<Perceivable>(e);
        let mut p = Percept {
            id,
            name: facts::name_of(w, e),
            kind: perc.map_or_else(|| "entity".to_owned(), |p| p.kind.clone()),
            visibility: Visibility::Seen,
            detail: Detail::Full,
            bearing_deg,
            range_m,
            pos_m: Some(at),
            age_s: None,
            facts: Vec::new(),
            can: Vec::new(),
            priority: perc.map_or(0, |p| p.priority),
        };
        match (perc, self.defs) {
            (Some(perc), Some(defs)) => {
                if let Some(kind) = defs.kind(&perc.kind) {
                    let cx = FactCx {
                        world: w,
                        id,
                        entity: Some(e),
                        at,
                        known: &NO_FACTS,
                        observer: self.origin,
                    };
                    for f in &kind.facts {
                        if let Some(v) = facts::evaluate(defs, &f.source, &cx) {
                            p.facts.push(Reading {
                                name: f.name.clone(),
                                value: v,
                                format: Format::of(&f.unit, f.precision),
                            });
                        }
                    }
                }
            }
            _ => {
                p.facts = facts::component_facts(w, e)
                    .into_iter()
                    .map(|(name, value)| Reading {
                        name,
                        value,
                        format: Format::RAW,
                    })
                    .collect();
            }
        }
        p
    }

    /// Every entity, ranked (intent targets first, then priority, range and id).
    pub fn percepts(&self, targets: &[EntityId]) -> Vec<Percept> {
        let Some(index) = self.world.get_resource::<EntityIndex>() else {
            return Vec::new();
        };
        let mut out: Vec<Percept> = index.iter().map(|(id, e)| self.percept(id, e)).collect();
        rank(&mut out, targets);
        out
    }

    /// The world's events after `since` (an `EventSeq`), oldest first.
    pub fn events(&self, since: u64) -> Vec<EventView> {
        let Some(inbox) = self.world.get_resource::<EventInbox>() else {
            return Vec::new();
        };
        inbox
            .events()
            .iter()
            .filter(|e| e.seq.0 > since)
            .map(|e| self.event(e))
            .collect()
    }

    /// The latest world event's number the world still holds (0 before any).
    pub fn latest(&self) -> u64 {
        self.world
            .get_resource::<pocket_sim::EventCounter>()
            .map_or(0, |c| c.next().saturating_sub(1))
    }

    /// The oldest world event's number the world still holds.
    pub fn oldest(&self) -> u64 {
        self.world
            .get_resource::<EventInbox>()
            .and_then(|i| i.events().first().map(|e| e.seq.0))
            .unwrap_or(self.latest() + 1)
    }

    fn event(&self, e: &Event) -> EventView {
        let w = self.world;
        let at = facts::data_field(&e.data, "at_m")
            .and_then(|v| match v {
                FactValue::Position(p) => Some(p),
                _ => None,
            })
            .or_else(|| {
                e.subject
                    .and_then(|s| facts::position(w, facts::live(w, s)?))
            });
        let (bearing_deg, range_m) = match at {
            Some(at) => {
                let (b, r) = bearing_range(self.from(), at);
                (Some(b), Some(r))
            }
            None => (None, None),
        };
        let def = self.defs.and_then(|d| d.event(e.kind.as_str()));
        let data: Vec<Reading> = match def {
            Some(def) => def
                .data
                .iter()
                .filter_map(|f| {
                    let super::defs::FactSource::Data { path } = &f.source else {
                        return None;
                    };
                    Some(Reading {
                        name: f.name.clone(),
                        value: facts::data_field(&e.data, path)?,
                        format: Format::of(&f.unit, f.precision),
                    })
                })
                .collect(),
            None => plain_members(&e.data),
        };
        EventView {
            seq: e.seq.0,
            tick: e.tick,
            kind: e.kind.as_str().to_owned(),
            subject: e.subject.map(|id| Named {
                id,
                name: facts::live(w, id).and_then(|x| facts::name_of(w, x)),
            }),
            sense: Sense::Global,
            bearing_deg,
            range_m,
            data,
            cause: e.cause.map(|c| c.0),
        }
    }

    /// The names of every named entity.
    pub fn known_names(&self) -> Vec<(EntityId, String, String)> {
        let Some(index) = self.world.get_resource::<EntityIndex>() else {
            return Vec::new();
        };
        let mut out: Vec<(EntityId, String, String)> = index
            .iter()
            .filter_map(|(id, e)| {
                let name = facts::name_of(self.world, e)?;
                let kind = self
                    .world
                    .get::<Perceivable>(e)
                    .map_or_else(|| "entity".to_owned(), |p| p.kind.clone());
                Some((id, name, kind))
            })
            .collect();
        out.sort();
        out
    }
}

/// An undeclared event's data members, unrounded, in key order.
fn plain_members(data: &pocket_sim::PlainData) -> Vec<Reading> {
    let pocket_sim::PlainData::Object(members) = data else {
        return Vec::new();
    };
    let mut out: BTreeMap<String, FactValue> = BTreeMap::new();
    for (k, v) in members {
        let json = v.to_json();
        let value = facts::json_fact(&json).unwrap_or_else(|| FactValue::Text(json.to_string()));
        out.insert(k.clone(), value);
    }
    out.into_iter()
        .map(|(name, value)| Reading {
            name,
            value,
            format: Format::RAW,
        })
        .collect()
}
