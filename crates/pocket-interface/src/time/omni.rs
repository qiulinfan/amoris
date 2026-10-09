//! The omniscient `until` (shared/contract/time.md, `until`): a developer's
//! `step {omniscient: true}` evaluates its condition against the whole world, through perception's
//! raw omniscient view measured from the seat's body: every entity with every fact, hidden ones
//! included, and the world's events, numbered by their `EventSeq`. The seat's instruments are read
//! as the seat reads them. A player never reaches it: `step` refuses a player's `omniscient` with
//! `perception.omniscient_forbidden` before any view is built, and a player's answers are never
//! marked.

use bevy_ecs::prelude::World;
use pocket_sim::{EntityId, Tick};
use serde_json::Value;

use crate::action::catalog::ActionCatalog;
use crate::action::state::SeatRow;
use crate::action::view::{ActorView, Known, KnownEvent, Visibility};
use crate::perception::bridge::fact_json;
use crate::perception::omniscient::Raw;
use crate::perception::{EventView, Percept};

/// The seat's view, or the whole world seen from the seat's body when `omniscient`.
pub fn view<'w>(
    world: &'w World,
    catalog: &ActionCatalog,
    row: &SeatRow,
    omniscient: bool,
) -> Box<dyn ActorView + 'w> {
    let seat = (catalog.view)(world, row);
    match (omniscient, Raw::new(world, Some(row.body))) {
        (true, Some(raw)) => Box::new(Omni { world, seat, raw }),
        _ => seat,
    }
}

/// The world seen whole, from a seat's body.
struct Omni<'w> {
    world: &'w World,
    seat: Box<dyn ActorView + 'w>,
    raw: Raw<'w>,
}

fn known(p: Percept) -> Known {
    Known {
        id: p.id,
        name: p.name,
        kind: p.kind,
        visibility: Visibility::Seen,
        pos_m: p.pos_m.unwrap_or([0.0; 3]),
        range_m: p.range_m,
        bearing_deg: p.bearing_deg,
        facts: p
            .facts
            .into_iter()
            .map(|r| (r.name, fact_json(&r.value)))
            .collect(),
    }
}

fn event(e: EventView) -> KnownEvent {
    KnownEvent {
        seq: e.seq,
        world_seq: Some(e.seq),
        tick: e.tick,
        kind: e.kind,
        subject: e.subject.map(|s| s.id),
        data: e
            .data
            .into_iter()
            .map(|r| (r.name, fact_json(&r.value)))
            .collect(),
    }
}

impl ActorView for Omni<'_> {
    fn tick(&self) -> Tick {
        self.seat.tick()
    }

    fn body(&self) -> EntityId {
        self.seat.body()
    }

    fn instrument(&self, name: &str) -> Option<Value> {
        self.seat.instrument(name)
    }

    fn known(&self, id: EntityId) -> Option<Known> {
        let e = pocket_sim::entity::entity(self.world, id)?;
        Some(known(self.raw.percept(id, e)))
    }

    fn all_known(&self) -> Vec<Known> {
        let mut all: Vec<Known> = self.raw.percepts(&[]).into_iter().map(known).collect();
        all.sort_by_key(|k| k.id);
        all
    }

    fn events_since(&self, seq: u64) -> Vec<KnownEvent> {
        self.raw.events(seq).into_iter().map(event).collect()
    }
}
