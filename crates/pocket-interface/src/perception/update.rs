//! The perception update (shared/contract/perception.md, The perception update): once per tick,
//! as `interface.perception` in the `Finish` phase before `sim.finish`, every observer (in
//! `EntityId` order) takes the candidates in range, in its field of view and in clear sight past
//! the occluders, writes what it sees into its memory with the facts of its detail level, pushes a
//! `sighted` event for a first sighting of a watched kind, forgets what it can see is gone or has
//! not seen for `memory_s`, evicts beyond its capacity, and pushes every event appended since the
//! last update whose scope admits it. It reads the world and writes only `ObserverMemory` and
//! `ObserverEvents`; it emits no event.

use std::collections::BTreeMap;

use bevy_ecs::prelude::{Entity, World};
use pocket_physics::Transform;
use pocket_physics::geom::{self, V3};
use pocket_sim::{
    EntityId, EntityIndex, Event, EventInbox, EventOutbox, SimClock, SystemCtx, Tick, math,
};

use super::defs::{Exposure, FactSource, ObserverProfile, PerceptionDefs, SIGHTED, Scope, Sight};
use super::facts::{self, Body, FactCx, NO_FACTS};
use super::geometry::{Eye, bearing_range, sees, within};
use super::state::{
    Detail, FactValue, MemoryEntry, Named, Observer, ObserverEvents, ObserverMemory, Perceivable,
    PerceivedEvent, RingEntry, Sense, VisibilityScale,
};
use super::view::shows;
use crate::projection::round::{self, Format};

/// The system: computes every observer's new memory and ring from the world, then writes them.
pub fn run(world: &mut World, _ctx: &mut SystemCtx<'_>) {
    let updates = compute(world);
    for (e, memory, events) in updates {
        let mut ent = world.entity_mut(e);
        ent.insert((memory, events));
    }
}

/// The new memory and ring of every observer, in `EntityId` order.
pub fn compute(world: &World) -> Vec<(Entity, ObserverMemory, ObserverEvents)> {
    let (Some(defs), Some(index), Some(clock)) = (
        world.get_resource::<PerceptionDefs>(),
        world.get_resource::<EntityIndex>(),
        world.get_resource::<SimClock>(),
    ) else {
        return Vec::new();
    };
    let vis = world
        .get_resource::<VisibilityScale>()
        .copied()
        .unwrap_or_default()
        .0;
    let mut appended: Vec<&Event> = Vec::new();
    if let Some(inbox) = world.get_resource::<EventInbox>() {
        appended.extend(inbox.boundary_events());
    }
    if let Some(outbox) = world.get_resource::<EventOutbox>() {
        appended.extend(outbox.events());
    }
    // Step 1's candidates, gathered once for every observer: the perceivable entities with a
    // place, in `EntityId` order.
    let candidates = index
        .iter()
        .filter_map(|(id, e)| {
            Some((
                id,
                e,
                world.get::<Perceivable>(e)?,
                facts::position(world, e)?,
            ))
        })
        .collect();
    let cx = Cx {
        world,
        defs,
        tick: clock.tick,
        rate: f64::from(clock.rate.0),
        vis,
        events: appended,
        candidates,
    };
    // Steps 1 to 5 for every observer, then what its team sees (Observer.team): an entity a
    // teammate sees is seen, at the best detail any member sees it.
    let mut own = Vec::new();
    for (id, e) in index.iter() {
        let Some(observer) = world.get::<Observer>(e) else {
            continue;
        };
        let (Some(profile), Some(tf)) =
            (defs.profile(&observer.profile), world.get::<Transform>(e))
        else {
            continue;
        };
        let body = Body {
            id,
            entity: e,
            position: tf.position,
        };
        let eye = profile.sight.as_ref().map(|s| (Eye::of(tf, s), s));
        let visible = cx.visible(body, eye, profile, observer.omniscient);
        own.push((body, tf, observer, profile, visible));
    }
    let mut out = Vec::new();
    for (i, (body, tf, observer, profile, visible)) in own.iter().enumerate() {
        let visible = match &observer.team {
            Some(team) if !observer.omniscient => {
                let mates = own
                    .iter()
                    .enumerate()
                    .filter(|(j, o)| *j != i && o.2.team.as_ref() == Some(team) && !o.2.omniscient)
                    .map(|(_, o)| o.4.as_slice());
                shared(body.id, visible, mates)
            }
            _ => visible.clone(),
        };
        let (m, ev) = cx.one(*body, tf, observer, profile, visible);
        out.push((body.entity, m, ev));
    }
    out
}

/// An observer's visible set with its teammates': every entity any of them sees (its own body
/// left out), at the best detail one of them sees it, in `EntityId` order.
fn shared<'w, 'a>(
    me: EntityId,
    own: &[Seen<'w>],
    mates: impl Iterator<Item = &'a [Seen<'w>]>,
) -> Vec<Seen<'w>>
where
    'w: 'a,
{
    let mut all: BTreeMap<EntityId, Seen<'w>> = own.iter().map(|s| (s.id, *s)).collect();
    for list in mates {
        for s in list.iter().filter(|s| s.id != me) {
            all.entry(s.id)
                .and_modify(|m| {
                    if s.detail > m.detail {
                        m.detail = s.detail;
                    }
                })
                .or_insert(*s);
        }
    }
    all.into_values().collect()
}

struct Cx<'w> {
    world: &'w World,
    defs: &'w PerceptionDefs,
    tick: Tick,
    rate: f64,
    vis: f64,
    events: Vec<&'w Event>,
    candidates: Vec<(EntityId, Entity, &'w Perceivable, V3)>,
}

/// One visible candidate.
#[derive(Clone, Copy)]
struct Seen<'w> {
    id: EntityId,
    entity: Entity,
    perc: &'w Perceivable,
    at: V3,
    detail: Detail,
}

/// Bearing and range from the body's origin, rounded by the range rule (projection.md, Stored
/// values).
fn stored_measures(from: V3, to: V3) -> (f64, f64) {
    let (b, r) = bearing_range(from, to);
    let p = round::range_precision(r);
    (round::stored_bearing(b, p), round::stored(r, p))
}

impl<'w> Cx<'w> {
    /// Steps 6 and 7 for one observer, given what it sees (its own and its team's).
    fn one(
        &self,
        body: Body,
        tf: &Transform,
        observer: &Observer,
        profile: &ObserverProfile,
        visible: Vec<Seen<'w>>,
    ) -> (ObserverMemory, ObserverEvents) {
        let w = self.world;
        let omni = observer.omniscient;
        let old = w
            .get::<ObserverMemory>(body.entity)
            .cloned()
            .unwrap_or_default();
        let mut ring = w
            .get::<ObserverEvents>(body.entity)
            .cloned()
            .unwrap_or_default();
        let eye = profile.sight.as_ref().map(|s| (Eye::of(tf, s), s));
        // Memory: every visible entity, then what is forgotten, then the capacity.
        let mut entries = if omni {
            BTreeMap::new()
        } else {
            old.entries.clone()
        };
        let mut sightings = Vec::new();
        for s in &visible {
            let had = old.entries.contains_key(&s.id);
            entries.insert(s.id, self.entry(s, body, omni));
            if !had && profile.sightings.contains(&s.perc.kind) {
                sightings.push(s);
            }
        }
        if !omni {
            let ttl = profile.memory_s * self.rate;
            entries.retain(|id, m| {
                if m.seen_tick == self.tick {
                    return true;
                }
                if let Some((eye, sight)) = &eye {
                    let r = math::min(sight.range_m, m.detect_m) * self.vis;
                    let top = geom::add(m.pos_m, [0.0, m.height_m, 0.0]);
                    if sees(
                        w,
                        eye,
                        sight,
                        r,
                        m.pos_m,
                        &[m.pos_m, top],
                        [Some(body.id), Some(*id)],
                    ) {
                        return false;
                    }
                }
                Tick(self.tick.0.saturating_sub(m.seen_tick.0)).to_f64() <= ttl
            });
            // The capacity bounds what is remembered unseen: an entity seen this tick is never
            // evicted, so a visible set larger than the capacity is kept whole and nothing in it
            // is sighted again on the next tick (docs/spec/perception-slice2.md 4).
            let cap = usize::try_from(profile.memory_capacity).unwrap_or(usize::MAX);
            while entries.len() > cap {
                let oldest = entries
                    .iter()
                    .filter(|(_, m)| m.seen_tick != self.tick)
                    .min_by_key(|(id, m)| (m.seen_tick, **id))
                    .map(|(id, _)| *id);
                match oldest {
                    Some(id) => entries.remove(&id),
                    None => break,
                };
            }
        }
        let memory = ObserverMemory {
            updated: self.tick,
            entries,
        };
        // Events: sightings first (in EntityId order), then the appended events by sequence.
        let mut pushed: Vec<RingEntry> = Vec::new();
        let mut next = ring.next_seq;
        for s in sightings {
            let (b, r) = stored_measures(body.position, s.at);
            next += 1;
            pushed.push(RingEntry {
                world: None,
                event: PerceivedEvent {
                    seq: next,
                    tick: self.tick,
                    kind: SIGHTED.to_owned(),
                    subject: Some(Named {
                        id: s.id,
                        name: facts::name_of(w, s.entity),
                    }),
                    sense: Sense::Sight,
                    bearing_deg: Some(b),
                    range_m: Some(r),
                    data: vec![("kind".to_owned(), FactValue::Text(s.perc.kind.clone()))],
                    cause: None,
                },
            });
        }
        for ev in &self.events {
            let Some(def) = self.defs.event(ev.kind.as_str()) else {
                continue;
            };
            let place = at_of(w, ev);
            let at = place.map(|p| p.0);
            let Some(sense) = self.admits(&def.scope, ev, at, body, eye, profile, omni) else {
                continue;
            };
            let known = |id: EntityId| {
                id == body.id
                    || memory.entries.contains_key(&id)
                    || (profile.chart
                        && facts::live(w, id)
                            .and_then(|e| w.get::<Perceivable>(e))
                            .is_some_and(|p| p.chart_m.is_some()))
            };
            // A remembered subject goes by the name it had when last seen, as its percept does.
            let subject = ev.subject.filter(|id| known(*id)).map(|id| Named {
                id,
                name: match memory.entries.get(&id) {
                    Some(m) if id != body.id => m.name.clone(),
                    _ => facts::live(w, id).and_then(|e| facts::name_of(w, e)),
                },
            });
            let (bearing_deg, range_m) = match place {
                Some((at, from_data))
                    if omni || self.place_known(sense, ev, from_data, body, &memory) =>
                {
                    let (b, r) = stored_measures(body.position, at);
                    (Some(b), Some(r))
                }
                _ => (None, None),
            };
            // Event data shows at `chart` and `coarse` exposure (validation refuses `full`,
            // `owner` and `relative` there; docs/spec/perception-slice2.md 3), `hidden` only to
            // the omniscient.
            let data = def
                .data
                .iter()
                .filter(|f| omni || f.exposure != Exposure::Hidden)
                .filter_map(|f| {
                    let FactSource::Data { path } = &f.source else {
                        return None;
                    };
                    let v = facts::data_field(&ev.data, path)?;
                    Some((
                        f.name.clone(),
                        round::store(&v, Format::of(&f.unit, f.precision)),
                    ))
                })
                .collect();
            let cause = ev.cause.and_then(|c| {
                ring.ring
                    .iter()
                    .chain(pushed.iter())
                    .find(|r| r.world == Some(c))
                    .map(|r| r.event.seq)
            });
            next += 1;
            pushed.push(RingEntry {
                world: Some(ev.seq),
                event: PerceivedEvent {
                    seq: next,
                    // The world event's own tick: a boundary's event the boundary's
                    // (docs/spec/perception-slice2.md 4).
                    tick: ev.tick,
                    kind: ev.kind.as_str().to_owned(),
                    subject,
                    sense,
                    bearing_deg,
                    range_m,
                    data,
                    cause,
                },
            });
        }
        ring.next_seq = next;
        ring.ring.extend(pushed);
        let cap = usize::try_from(profile.event_capacity).unwrap_or(usize::MAX);
        while ring.ring.len() > cap {
            ring.ring.pop_front();
        }
        (memory, ring)
    }

    /// Steps 1 to 5: the visible candidates with their detail, in `EntityId` order.
    fn visible(
        &self,
        body: Body,
        eye: Option<(Eye, &Sight)>,
        profile: &ObserverProfile,
        omni: bool,
    ) -> Vec<Seen<'w>> {
        let w = self.world;
        let mut out = Vec::new();
        for &(id, e, perc, at) in &self.candidates {
            if id == body.id {
                continue;
            }
            let detail = if omni {
                Detail::Full
            } else {
                let Some((eye, sight)) = &eye else {
                    continue;
                };
                let r = math::min(sight.range_m, perc.detect_m) * self.vis;
                let top = geom::add(at, [0.0, perc.height_m, 0.0]);
                if !sees(w, eye, sight, r, at, &[at, top], [Some(body.id), Some(id)]) {
                    continue;
                }
                if within(geom::sub(at, eye.at), profile.attention_m) {
                    Detail::Full
                } else {
                    Detail::Coarse
                }
            };
            out.push(Seen {
                id,
                entity: e,
                perc,
                at,
                detail,
            });
        }
        out
    }

    /// The memory entry of a visible entity: its facts of its detail level, rounded, without the
    /// relative ones.
    fn entry(&self, s: &Seen<'_>, body: Body, omni: bool) -> MemoryEntry {
        let mut facts_now = BTreeMap::new();
        if let Some(kind) = self.defs.kind(&s.perc.kind) {
            let cx = FactCx {
                world: self.world,
                id: s.id,
                entity: Some(s.entity),
                at: s.at,
                known: &NO_FACTS,
                observer: Some(body),
            };
            for f in kind
                .facts
                .iter()
                .filter(|f| !f.relative && shows(f.exposure, s.detail, omni))
            {
                if let Some(v) = facts::evaluate(self.defs, &f.source, &cx) {
                    facts_now.insert(
                        f.name.clone(),
                        round::store(&v, Format::of(&f.unit, f.precision)),
                    );
                }
            }
        }
        MemoryEntry {
            kind: s.perc.kind.clone(),
            name: facts::name_of(self.world, s.entity),
            seen_tick: self.tick,
            pos_m: s.at,
            facts: facts_now,
            detail: s.detail,
            detect_m: s.perc.detect_m,
            height_m: s.perc.height_m,
            priority: s.perc.priority,
        }
    }

    /// Step 7: how the observer perceives the event, if its scope admits it.
    #[allow(clippy::too_many_arguments)]
    fn admits(
        &self,
        scope: &Scope,
        ev: &Event,
        at: Option<V3>,
        body: Body,
        eye: Option<(Eye, &Sight)>,
        profile: &ObserverProfile,
        omni: bool,
    ) -> Option<Sense> {
        let private = ev.subject == Some(body.id);
        if omni {
            return Some(match scope {
                Scope::Private => Sense::Private,
                Scope::Sight => Sense::Sight,
                Scope::Sound { .. } => Sense::Sound,
                Scope::Global | Scope::Hidden => Sense::Global,
            });
        }
        match scope {
            Scope::Private => private.then_some(Sense::Private),
            Scope::Global => Some(Sense::Global),
            Scope::Hidden => None,
            Scope::Sight => {
                let (at, (eye, sight)) = (at?, eye?);
                let r = sight.range_m * self.vis;
                sees(
                    self.world,
                    &eye,
                    sight,
                    r,
                    at,
                    &[at],
                    [Some(body.id), ev.subject],
                )
                .then_some(Sense::Sight)
            }
            Scope::Sound { radius_m } => {
                let (at, hearing) = (at?, profile.hearing.as_ref()?);
                let from = eye.map_or(body.position, |(e, _)| e.at);
                within(geom::sub(at, from), radius_m * hearing.scale).then_some(Sense::Sound)
            }
        }
    }

    /// Whether the observer perceives where a perceived event happened, so that its bearing and
    /// range may be shown: what it saw or heard there, and a private event on its own body; a
    /// global event only when the place is its subject's position (no `at_m` member) and the
    /// subject is the observer's body or an entity it sees this tick. A global event about an
    /// entity it does not see tells that it happened, not where (docs/spec/perception-slice2.md
    /// 4).
    fn place_known(
        &self,
        sense: Sense,
        ev: &Event,
        from_data: bool,
        body: Body,
        memory: &ObserverMemory,
    ) -> bool {
        match sense {
            Sense::Sight | Sense::Sound | Sense::Private => true,
            Sense::Global => {
                !from_data
                    && ev.subject.is_some_and(|s| {
                        s == body.id
                            || memory
                                .entries
                                .get(&s)
                                .is_some_and(|m| m.seen_tick == self.tick)
                    })
            }
        }
    }
}

/// Where an event happened, and whether from its `at_m` data member: that member, else its
/// subject's position at the end of the tick, else nowhere.
fn at_of(world: &World, ev: &Event) -> Option<(V3, bool)> {
    if let Some(FactValue::Position(p)) = facts::data_field(&ev.data, "at_m") {
        return Some((p, true));
    }
    let e = facts::live(world, ev.subject?)?;
    facts::position(world, e).map(|p| (p, false))
}
