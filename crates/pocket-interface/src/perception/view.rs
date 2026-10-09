//! One observer's perception, read-only (shared/contract/perception.md, Queries,
//! `PerceptionView`): its instruments, the entities it knows (seen, remembered or charted) as
//! percepts with the facts their visibility and detail allow, ranked, and its perceived events.
//! Queries, intent executors and NPC rules all read through it, so an answer can only contain what
//! the observer's memory, chart and instruments grant.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use bevy_ecs::prelude::{Entity, World};
use pocket_physics::Transform;
use pocket_physics::geom::V3;
use pocket_sim::{EntityId, EntityIndex, SimClock, Tick};

use super::defs::{Exposure, FactDef, ObserverProfile, PerceptionDefs};
use super::facts::{self, Body, FactCx, NO_FACTS};
use super::filter::keeps;
use super::geometry::{bearing_range, heading_deg, relative_deg};
use super::query::{NEARBY_LIMIT, NearbyRequest};
use super::state::{
    Detail, FactValue, MemoryEntry, Named, Observer, ObserverEvents, ObserverMemory, Perceivable,
    PerceivedEvent, RingEntry, Sense, Visibility,
};
use crate::projection::round::{self, Format};

/// A named value with the format it is written in.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub name: String,
    pub value: FactValue,
    pub format: Format,
}

/// An entity as the observer knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Percept {
    pub id: EntityId,
    pub name: Option<String>,
    pub kind: String,
    pub visibility: Visibility,
    pub detail: Detail,
    /// Horizontal, from the body's origin to the live, remembered or chart position.
    pub bearing_deg: f64,
    pub range_m: f64,
    /// When the profile has `positions` (always in an omniscient view).
    pub pos_m: Option<V3>,
    /// Remembered only: seconds since it was last seen.
    pub age_s: Option<f64>,
    /// In the kind's declared order.
    pub facts: Vec<Reading>,
    /// Verbs available now (actions.md, Affordances), filled by the action layer.
    pub can: Vec<String>,
    /// Ranking only: its priority as known.
    pub priority: u8,
}

/// A perceived event with its data's formats.
#[derive(Clone, Debug, PartialEq)]
pub struct EventView {
    pub seq: u64,
    pub tick: Tick,
    pub kind: String,
    pub subject: Option<Named>,
    pub sense: Sense,
    pub bearing_deg: Option<f64>,
    pub range_m: Option<f64>,
    pub data: Vec<Reading>,
    pub cause: Option<u64>,
}

/// Whether a fact of `exposure` shows at `detail` (perception.md, The perception update).
pub fn shows(exposure: Exposure, detail: Detail, omniscient: bool) -> bool {
    match exposure {
        Exposure::Chart => true,
        Exposure::Coarse => detail >= Detail::Coarse,
        Exposure::Full => detail == Detail::Full,
        Exposure::Owner => omniscient,
        Exposure::Hidden => omniscient,
    }
}

/// The ranking key (perception.md, Token budgets): intent targets first, then seen before
/// remembered before charted, higher priority, nearer by the shown range, lower id.
pub fn rank(percepts: &mut [Percept], targets: &[EntityId]) {
    percepts.sort_by(|a, b| {
        let key = |p: &Percept| (!targets.contains(&p.id), p.visibility, Reverse(p.priority));
        let shown = |p: &Percept| round::stored(p.range_m, round::range_precision(p.range_m));
        key(a)
            .cmp(&key(b))
            .then(shown(a).total_cmp(&shown(b)))
            .then(a.id.cmp(&b.id))
    });
}

/// One observer's view of the world at a boundary (or inside a tick, for executors and NPC rules,
/// where what it sees and remembers is the last update's, at the end of the previous tick).
pub struct PerceptionView<'w> {
    pub(crate) world: &'w World,
    pub(crate) defs: &'w PerceptionDefs,
    pub(crate) observer: &'w Observer,
    pub(crate) profile: &'w ObserverProfile,
    pub(crate) body: Body,
    pub(crate) heading: f64,
    pub(crate) memory: Option<&'w ObserverMemory>,
    pub(crate) events: Option<&'w ObserverEvents>,
    pub(crate) clock: SimClock,
}

impl<'w> PerceptionView<'w> {
    /// The view of the observer on `body`, if it is one with a declared profile and a transform.
    pub fn of(world: &'w World, body: EntityId) -> Option<PerceptionView<'w>> {
        let defs = world.get_resource::<PerceptionDefs>()?;
        let entity = world.get_resource::<EntityIndex>()?.get(body)?;
        let observer = world.get::<Observer>(entity)?;
        let profile = defs.profile(&observer.profile)?;
        let tf = world.get::<Transform>(entity)?;
        Some(PerceptionView {
            world,
            defs,
            observer,
            profile,
            body: Body {
                id: body,
                entity,
                position: tf.position,
            },
            heading: heading_deg(tf),
            memory: world.get::<ObserverMemory>(entity),
            events: world.get::<ObserverEvents>(entity),
            clock: *world.get_resource::<SimClock>()?,
        })
    }

    /// The view of the seat `seat`'s observer.
    pub fn for_seat(world: &'w World, seat: &str) -> Option<PerceptionView<'w>> {
        PerceptionView::of(world, seat_body(world, seat)?)
    }

    pub fn tick(&self) -> Tick {
        self.clock.tick
    }

    pub fn body(&self) -> EntityId {
        self.body.id
    }

    /// The body's heading, degrees.
    pub fn heading(&self) -> f64 {
        self.heading
    }

    pub fn profile(&self) -> &'w ObserverProfile {
        self.profile
    }

    pub fn defs(&self) -> &'w PerceptionDefs {
        self.defs
    }

    pub fn world(&self) -> &'w World {
        self.world
    }

    /// The seat whose body this is.
    pub fn seat(&self) -> Option<&'w str> {
        self.observer.seat.as_deref()
    }

    /// Bound to `omniscient_player`: every answer is marked.
    pub fn omniscient(&self) -> bool {
        self.observer.omniscient
    }

    /// An instrument's value now, if the profile reads it.
    pub fn instrument(&self, name: &str) -> Option<FactValue> {
        if !self.profile.instruments.iter().any(|n| n == name) {
            return None;
        }
        let def = self.defs.instrument(name)?;
        let cx = FactCx {
            world: self.world,
            id: self.body.id,
            entity: Some(self.body.entity),
            at: self.body.position,
            known: &NO_FACTS,
            observer: Some(self.body),
        };
        facts::evaluate(self.defs, &def.source, &cx)
    }

    /// Every instrument of the profile, in its order (an absent value is left out).
    pub fn instruments(&self) -> Vec<Reading> {
        self.profile
            .instruments
            .iter()
            .filter_map(|n| {
                let def = self.defs.instrument(n)?;
                Some(Reading {
                    name: n.clone(),
                    value: self.instrument(n)?,
                    format: Format::of(&def.unit, def.precision),
                })
            })
            .collect()
    }

    fn entry(&self, id: EntityId) -> Option<&'w MemoryEntry> {
        self.memory?.entries.get(&id)
    }

    /// Whether the observer knows the entity: its body, remembered or seen, or on its chart.
    pub fn knows(&self, id: EntityId) -> bool {
        id == self.body.id || self.entry(id).is_some() || self.chart(id).is_some()
    }

    /// The entity's chart position, if the profile reads the chart and it is charted.
    fn chart(&self, id: EntityId) -> Option<(Entity, &'w Perceivable, V3)> {
        if !self.profile.chart {
            return None;
        }
        let e = facts::live(self.world, id)?;
        let p = self.world.get::<Perceivable>(e)?;
        Some((e, p, p.chart_m?.array()))
    }

    /// The entity as the observer knows it, or `None` when it does not know it.
    pub fn percept(&self, id: EntityId) -> Option<Percept> {
        if id == self.body.id {
            return self.own_body();
        }
        if let Some(m) = self.entry(id) {
            return Some(self.remembered(id, m));
        }
        let (e, p, at) = self.chart(id)?;
        Some(self.charted(id, e, p, at))
    }

    fn base(&self, id: EntityId, name: Option<String>, kind: &str, at: V3) -> Percept {
        let (bearing_deg, range_m) = bearing_range(self.body.position, at);
        Percept {
            id,
            name,
            kind: kind.to_owned(),
            visibility: Visibility::Seen,
            detail: Detail::Full,
            bearing_deg,
            range_m,
            pos_m: (self.profile.positions || self.omniscient()).then_some(at),
            age_s: None,
            facts: Vec::new(),
            can: Vec::new(),
            priority: 0,
        }
    }

    fn relative(
        &self,
        f: &FactDef,
        id: EntityId,
        at: V3,
        known: &BTreeMap<String, FactValue>,
    ) -> Option<FactValue> {
        let cx = FactCx {
            world: self.world,
            id,
            entity: None,
            at,
            known,
            observer: Some(self.body),
        };
        facts::evaluate(self.defs, &f.source, &cx)
    }

    fn reading(f: &FactDef, value: FactValue) -> Reading {
        Reading {
            name: f.name.clone(),
            value,
            format: Format::of(&f.unit, f.precision),
        }
    }

    /// The tick of the last perception update: the clock's at a boundary, the one before inside a
    /// tick (docs/spec/perception-slice2.md 2).
    fn updated(&self) -> Tick {
        self.memory.map_or(self.clock.tick, |m| m.updated)
    }

    fn remembered(&self, id: EntityId, m: &MemoryEntry) -> Percept {
        let mut p = self.base(id, m.name.clone(), &m.kind, m.pos_m);
        let updated = self.updated();
        let seen = m.seen_tick == updated;
        p.visibility = if seen {
            Visibility::Seen
        } else {
            Visibility::Remembered
        };
        p.detail = m.detail;
        p.priority = m.priority;
        if !seen {
            let ticks = updated.0.saturating_sub(m.seen_tick.0);
            p.age_s = Some(Tick(ticks).to_f64() / f64::from(self.clock.rate.0));
        }
        if let Some(kind) = self.defs.kind(&m.kind) {
            for f in &kind.facts {
                if !shows(f.exposure, m.detail, self.omniscient()) {
                    continue;
                }
                let v = if f.relative {
                    self.relative(f, id, m.pos_m, &m.facts)
                } else {
                    m.facts.get(&f.name).cloned()
                };
                if let Some(v) = v {
                    p.facts.push(Self::reading(f, v));
                }
            }
        }
        p
    }

    fn charted(&self, id: EntityId, e: Entity, perc: &Perceivable, at: V3) -> Percept {
        let mut p = self.base(id, facts::name_of(self.world, e), &perc.kind, at);
        p.visibility = Visibility::Charted;
        p.detail = Detail::Chart;
        p.priority = perc.priority;
        if let Some(kind) = self.defs.kind(&perc.kind) {
            let mut known = BTreeMap::new();
            for f in kind
                .facts
                .iter()
                .filter(|f| f.exposure == Exposure::Chart && !f.relative)
            {
                let cx = FactCx {
                    world: self.world,
                    id,
                    entity: Some(e),
                    at,
                    known: &NO_FACTS,
                    observer: Some(self.body),
                };
                if let Some(v) = facts::evaluate(self.defs, &f.source, &cx) {
                    known.insert(
                        f.name.clone(),
                        round::store(&v, Format::of(&f.unit, f.precision)),
                    );
                }
            }
            for f in kind.facts.iter().filter(|f| f.exposure == Exposure::Chart) {
                let v = if f.relative {
                    self.relative(f, id, at, &known)
                } else {
                    known.get(&f.name).cloned()
                };
                if let Some(v) = v {
                    p.facts.push(Self::reading(f, v));
                }
            }
        }
        p
    }

    /// The seat's own body: always known, with every fact but hidden ones, evaluated now.
    fn own_body(&self) -> Option<Percept> {
        let w = self.world;
        let perc = w.get::<Perceivable>(self.body.entity);
        let kind = perc.map_or("self", |p| p.kind.as_str());
        let mut p = self.base(
            self.body.id,
            facts::name_of(w, self.body.entity),
            kind,
            self.body.position,
        );
        p.priority = perc.map_or(0, |x| x.priority);
        if let Some(k) = self.defs.kind(kind) {
            for f in &k.facts {
                if f.exposure == Exposure::Hidden && !self.omniscient() {
                    continue;
                }
                let cx = FactCx {
                    world: w,
                    id: self.body.id,
                    entity: Some(self.body.entity),
                    at: self.body.position,
                    known: &NO_FACTS,
                    observer: Some(self.body),
                };
                if let Some(v) = facts::evaluate(self.defs, &f.source, &cx) {
                    let fmt = Format::of(&f.unit, f.precision);
                    p.facts.push(Reading {
                        name: f.name.clone(),
                        value: round::store(&v, fmt),
                        format: fmt,
                    });
                }
            }
        }
        Some(p)
    }

    /// Every entity the observer knows but its own body, ranked (intent targets first).
    pub fn percepts(&self, targets: &[EntityId]) -> Vec<Percept> {
        let mut out: Vec<Percept> = Vec::new();
        if let Some(m) = self.memory {
            out.extend(m.entries.iter().map(|(id, e)| self.remembered(*id, e)));
        }
        if self.profile.chart
            && let Some(index) = self.world.get_resource::<EntityIndex>()
        {
            for (id, e) in index.iter() {
                if id == self.body.id || self.entry(id).is_some() {
                    continue;
                }
                if let Some(p) = self.world.get::<Perceivable>(e)
                    && let Some(at) = p.chart_m
                {
                    out.push(self.charted(id, e, p, at.array()));
                }
            }
        }
        rank(&mut out, targets);
        out
    }

    /// `nearby` unprojected (perception.md, `PerceptionView`), for NPC rules and executors: the
    /// entities the observer knows, but its own body, that pass the request's filters, ranked, at
    /// most its `limit` (default and maximum 100); no budget. The request is taken as checked.
    pub fn nearby(&self, req: &NearbyRequest) -> Vec<Percept> {
        let limit = req.limit.map_or(NEARBY_LIMIT, |l| l.min(NEARBY_LIMIT));
        let heading = self.heading;
        self.percepts(&[])
            .into_iter()
            .filter(|p| keeps(req, p, |b| relative_deg(b, heading)))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect()
    }

    /// The angle of a percept off the heading.
    pub fn relative_bearing(&self, p: &Percept) -> f64 {
        relative_deg(p.bearing_deg, self.heading)
    }

    /// The perceived events after `seq`, oldest first.
    pub fn events_since(&self, seq: u64) -> impl Iterator<Item = &'w PerceivedEvent> + 'w {
        self.ring_since(seq).map(|r| &r.event)
    }

    /// The ring's entries after `seq`, oldest first, each with its world event's number.
    pub fn ring_since(&self, seq: u64) -> impl Iterator<Item = &'w RingEntry> + 'w {
        self.events
            .into_iter()
            .flat_map(|e| e.ring.iter())
            .filter(move |r| r.event.seq > seq)
    }

    /// Where the observer knows the entity to be: its body's position, the remembered position,
    /// or the chart's.
    pub fn position_of(&self, id: EntityId) -> Option<V3> {
        if id == self.body.id {
            return Some(self.body.position);
        }
        if let Some(m) = self.entry(id) {
            return Some(m.pos_m);
        }
        self.chart(id).map(|c| c.2)
    }

    /// The seq of the last event perceived (0 before any).
    pub fn latest(&self) -> u64 {
        self.events.map_or(0, ObserverEvents::latest)
    }

    /// The oldest seq still kept.
    pub fn oldest(&self) -> u64 {
        self.events.map_or(1, ObserverEvents::oldest)
    }

    /// A perceived event with its data's formats (from the event's declaration; a sighting's
    /// `kind` is text).
    pub fn event_view(&self, e: &PerceivedEvent) -> EventView {
        let def = self.defs.event(&e.kind);
        let data = e
            .data
            .iter()
            .map(|(name, value)| {
                // A sighting's `kind`, and anything undeclared, is text.
                let format = def
                    .and_then(|d| d.data.iter().find(|f| &f.name == name))
                    .map_or(Format::TEXT, |f| Format::of(&f.unit, f.precision));
                Reading {
                    name: name.clone(),
                    value: value.clone(),
                    format,
                }
            })
            .collect();
        EventView {
            seq: e.seq,
            tick: e.tick,
            kind: e.kind.clone(),
            subject: e.subject.clone(),
            sense: e.sense,
            bearing_deg: e.bearing_deg,
            range_m: e.range_m,
            data,
            cause: e.cause,
        }
    }

    /// The names of the entities the observer knows (suggestions for an unknown reference).
    pub fn known_names(&self) -> Vec<(EntityId, String, String)> {
        let mut out = Vec::new();
        if let Some(p) = self.own_body()
            && let Some(n) = p.name
        {
            out.push((p.id, n, p.kind));
        }
        for p in self.percepts(&[]) {
            if let Some(n) = p.name {
                out.push((p.id, n, p.kind));
            }
        }
        out.sort();
        out
    }
}

/// The body of the seat `seat`: the entity whose `Observer` names it, lowest id first.
pub fn seat_body(world: &World, seat: &str) -> Option<EntityId> {
    let index = world.get_resource::<EntityIndex>()?;
    index.iter().find_map(|(id, e)| {
        world
            .get::<Observer>(e)
            .filter(|o| o.seat.as_deref() == Some(seat))
            .map(|_| id)
    })
}

/// Every seat an observer names, in its body's id order.
pub fn seats(world: &World) -> Vec<String> {
    let Some(index) = world.get_resource::<EntityIndex>() else {
        return Vec::new();
    };
    index
        .iter()
        .filter_map(|(_, e)| world.get::<Observer>(e)?.seat.clone())
        .collect()
}
