//! The answers of `observe`, `nearby`, `describe` and `events`, filled to their token budgets
//! (shared/contract/perception.md, Token budgets): the mandatory part, then the longest prefix of
//! the ranked candidates (percepts and events merged by alternation in `observe`) that fits
//! together with the omitted section for what is left, so a larger budget never chooses fewer
//! items and events always come as a contiguous run from `since`. Sizes are summed from the
//! pieces each projection writes and the chosen answer is then written once.

use std::collections::BTreeMap;

use bevy_ecs::prelude::World;
use pocket_contract::codes::{budget_too_small, cursor_ahead, not_applicable, unknown_kind};
use pocket_contract::{Candidate, Pointer, Problem};
use pocket_sim::EntityId;

use super::defs::SIGHTED;
use super::geometry::relative_deg;
use super::query::{
    AffordanceStatus, Affordances, Answer, Caller, DESCRIBE_EVENTS, DescribeRequest, EVENTS_LIMIT,
    EventsRequest, NEARBY_LIMIT, NearbyRequest, ObserveRequest, RAW_BUDGET, SeatParts, Source,
    bind, check_budget, check_limit, resolve,
};
use super::state::Named;
use super::view::{EventView, Percept, Reading};
use crate::projection::json::{self, Obj, array_len};
use crate::projection::{
    Header, Omitted, Projection, Rendered, TOKENS_RESERVE, tensor, text, tokens,
};

/// One candidate: its text line, its JSON value, and its kind (a percept) or seq (an event).
struct Item {
    text: String,
    json: String,
    kind: String,
    seq: u64,
}

/// An answer before filling.
struct Doc {
    projection: Projection,
    budget: u32,
    head_text: String,
    head_json: String,
    entities: Option<Vec<Item>>,
    events: Option<Vec<Item>>,
    /// The cursor member's value when no event is chosen; `None`: no cursor member.
    cursor: Option<u64>,
    /// Whether the answer has an omitted section.
    omitted: bool,
    lost: u32,
    /// Matching events and percepts past the candidates (a `limit`).
    beyond_events: u32,
    beyond_entities: BTreeMap<String, u32>,
}

impl Doc {
    /// The merged candidate sequence: (is an entity, index).
    fn order(&self) -> Vec<(bool, usize)> {
        let n = self.entities.as_ref().map_or(0, Vec::len);
        let m = self.events.as_ref().map_or(0, Vec::len);
        let mut out = Vec::with_capacity(n + m);
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n {
                out.push((true, i));
                i += 1;
            }
            if j < m {
                out.push((false, j));
                j += 1;
            }
        }
        out
    }

    /// How many entities and events a prefix of `k` chooses.
    fn split(order: &[(bool, usize)], k: usize) -> (usize, usize) {
        let e = order[..k].iter().filter(|x| x.0).count();
        (e, k - e)
    }

    fn omitted_for(&self, ne: usize, nv: usize) -> Omitted {
        let mut entities = self.beyond_entities.clone();
        for it in self.entities.iter().flatten().skip(ne) {
            *entities.entry(it.kind.clone()).or_insert(0) += 1;
        }
        let left = self.events.as_ref().map_or(0, |v| v.len() - nv);
        let cursor = match (&self.events, nv) {
            (Some(v), n) if n > 0 => v[n - 1].seq,
            _ => self.cursor.unwrap_or(0),
        };
        Omitted {
            entities,
            events: u32::try_from(left)
                .unwrap_or(u32::MAX)
                .saturating_add(self.beyond_events),
            lost: self.lost,
            cursor,
        }
    }

    /// The size of the answer choosing `ne` entities and `nv` events.
    fn size(&self, ne: usize, nv: usize) -> usize {
        let om = self.omitted_for(ne, nv);
        let chosen = |list: &Option<Vec<Item>>, n: usize, json: bool| -> usize {
            list.as_ref().map_or(0, |v| {
                let it = v[..n].iter();
                if json {
                    array_len(it.map(|x| x.json.len()))
                } else {
                    it.map(|x| x.text.len()).sum()
                }
            })
        };
        match self.projection {
            Projection::Json => {
                let mut n = self.head_json.len() + 1 + TOKENS_RESERVE;
                if self.entities.is_some() {
                    n += ",\"entities\":".len() + chosen(&self.entities, ne, true);
                }
                if self.events.is_some() {
                    n += ",\"events\":".len() + chosen(&self.events, nv, true);
                }
                if self.cursor.is_some() {
                    n += ",\"cursor\":".len() + om.cursor.to_string().len();
                }
                if self.omitted {
                    n += ",\"omitted\":".len() + json::omitted(&om).len();
                }
                n
            }
            _ => {
                let mut n = self.head_text.len() + chosen(&self.entities, ne, false);
                n += chosen(&self.events, nv, false);
                if self.omitted {
                    n += text::omitted(&om).len();
                }
                n
            }
        }
    }

    /// Fills to the budget and writes the answer: the longest prefix whose answer fits. An item
    /// line can be shorter than the omitted line it saves, so a size need not grow with the
    /// prefix: the query is refused only when no prefix fits, with the smallest budget that would
    /// answer (docs/spec/perception-slice2.md 5).
    fn finish(self) -> Result<Answer, Problem> {
        let limit = usize::try_from(self.budget)
            .unwrap_or(usize::MAX)
            .saturating_mul(4);
        let order = self.order();
        let size = |k: usize| {
            let (ne, nv) = Doc::split(&order, k);
            self.size(ne, nv)
        };
        let Some(k) = (0..=order.len()).rev().find(|k| size(*k) <= limit) else {
            let least = (0..=order.len()).map(size).min().unwrap_or(0);
            return Err(budget_too_small(
                u64::from(self.budget),
                u64::from(tokens(least)),
            ));
        };
        let (ne, nv) = Doc::split(&order, k);
        let om = self.omitted_for(ne, nv);
        let cursor = self.cursor.map(|_| om.cursor);
        let body = match self.projection {
            Projection::Json => {
                let mut s = self.head_json.clone();
                let list = |l: &Option<Vec<Item>>, n: usize| -> Vec<String> {
                    l.iter().flatten().take(n).map(|x| x.json.clone()).collect()
                };
                if self.entities.is_some() {
                    s.push_str(",\"entities\":");
                    s.push_str(&json::array(&list(&self.entities, ne)));
                }
                if self.events.is_some() {
                    s.push_str(",\"events\":");
                    s.push_str(&json::array(&list(&self.events, nv)));
                }
                if let Some(c) = cursor {
                    s.push_str(&format!(",\"cursor\":{c}"));
                }
                if self.omitted {
                    s.push_str(",\"omitted\":");
                    s.push_str(&json::omitted(&om));
                }
                let t = tokens(s.len() + 1);
                s.push_str(&format!(",\"tokens\":{t}}}"));
                debug_assert!(s.len() <= self.size(ne, nv));
                return Ok(Answer {
                    projection: Projection::Json,
                    body: s,
                    tokens: t,
                    cursor,
                });
            }
            _ => {
                let mut s = self.head_text.clone();
                for it in self.entities.iter().flatten().take(ne) {
                    s.push_str(&it.text);
                }
                for it in self.events.iter().flatten().take(nv) {
                    s.push_str(&it.text);
                }
                if self.omitted {
                    s.push_str(&text::omitted(&om));
                }
                debug_assert_eq!(s.len(), self.size(ne, nv));
                s
            }
        };
        Ok(Answer {
            projection: Projection::Text,
            tokens: tokens(body.len()),
            body,
            cursor,
        })
    }
}

fn percept_item(p: &Percept) -> Item {
    Item {
        text: text::entity(p),
        json: json::percept(p),
        kind: p.kind.clone(),
        seq: 0,
    }
}

fn event_item(e: &EventView) -> Item {
    Item {
        text: text::event(e),
        json: json::event(e),
        kind: e.kind.clone(),
        seq: e.seq,
    }
}

/// The projection asked for; tensors only where they apply.
fn projection_of(p: Option<Projection>, budget: Option<u32>) -> Result<Projection, Problem> {
    let p = p.unwrap_or(Projection::Json);
    if p == Projection::Tensor && budget.is_some() {
        return Err(not_applicable(
            &Pointer::root().key("budget_tokens"),
            "a budget does not apply to tensors",
        ));
    }
    Ok(p)
}

fn tensor_refused(why: &str) -> Problem {
    not_applicable(&Pointer::root().key("projection"), why)
}

/// The events after `since` the source holds, its latest seq and how many after `since` were
/// lost; `perception.cursor_ahead` when `since` is past the latest.
fn events_after(source: &Source<'_>, since: u64) -> Result<(Vec<EventView>, u64, u32), Problem> {
    let (events, latest, oldest) = match source {
        Source::Seat(v) => (
            v.events_since(since).map(|e| v.event_view(e)).collect(),
            v.latest(),
            v.oldest(),
        ),
        Source::Raw(raw) => (raw.events(since), raw.latest(), raw.oldest()),
    };
    if since > latest {
        return Err(cursor_ahead(since, latest));
    }
    let lost = oldest.saturating_sub(1).saturating_sub(since);
    Ok((events, latest, u32::try_from(lost).unwrap_or(u32::MAX)))
}

fn default_budget(source: &Source<'_>) -> u32 {
    match source {
        Source::Seat(v) => v.profile().budget_tokens,
        Source::Raw(_) => RAW_BUDGET,
    }
}

fn header(source: &Source<'_>) -> Header {
    match source {
        Source::Seat(v) => Header {
            tick: v.tick(),
            t_s: v.clock.time(),
            seat: v.seat().map(str::to_owned),
            observer: Some(Named {
                id: v.body(),
                name: super::facts::name_of(v.world(), v.body.entity),
            }),
            omniscient: v.omniscient(),
        },
        Source::Raw(raw) => Header {
            tick: raw.clock.tick,
            t_s: raw.clock.time(),
            seat: None,
            observer: None,
            omniscient: true,
        },
    }
}

fn percepts(source: &Source<'_>, targets: &[EntityId], aff: &dyn Affordances) -> Vec<Percept> {
    match source {
        Source::Seat(v) => {
            let mut ps = v.percepts(targets);
            for p in &mut ps {
                p.can = aff.can(v, p);
            }
            ps
        }
        Source::Raw(raw) => raw.percepts(targets),
    }
}

/// `observe` (perception.md, Queries): the header, every instrument, the seat's active intents, a
/// pending decision, the ranked percepts and the perceived events after `since`, and what was
/// left out.
pub fn observe(
    world: &World,
    caller: &Caller<'_>,
    req: &ObserveRequest,
    parts: &SeatParts,
    aff: &dyn Affordances,
) -> Result<Answer, Problem> {
    let source = bind(world, caller, req.seat.as_deref(), req.omniscient)?;
    let projection = projection_of(req.projection, req.budget_tokens)?;
    if projection == Projection::Tensor {
        let Source::Seat(v) = &source else {
            return Err(tensor_refused("the omniscient view has no tensor layout"));
        };
        let Some(spec) = &v.profile().tensor else {
            return Err(tensor_refused(
                "this seat's profile declares no tensor layout",
            ));
        };
        let body = tensor::observation(v, spec, &parts.targets);
        return Ok(Answer {
            projection,
            tokens: tokens(body.len()),
            body,
            cursor: None,
        });
    }
    let budget = check_budget(req.budget_tokens, default_budget(&source))?;
    let instruments = match &source {
        Source::Seat(v) => v.instruments(),
        Source::Raw(_) => Vec::new(),
    };
    let (events, cursor, lost) = match req.since {
        Some(since) => {
            let (ev, _, lost) = events_after(&source, since)?;
            (ev, since, lost)
        }
        None => {
            let latest = match &source {
                Source::Seat(v) => v.latest(),
                Source::Raw(raw) => raw.latest(),
            };
            (Vec::new(), latest, 0)
        }
    };
    let o = Observation {
        header: header(&source),
        instruments,
        intents: parts.intents.clone(),
        decision: parts.decision.clone(),
        percepts: percepts(&source, &parts.targets, aff),
        events,
        cursor,
        lost,
    };
    write_observation(&o, projection, budget)
}

/// An observation's content before it is filled to a budget: what [`observe`] gathers from the
/// world, and what the golden cases build by hand (projection.md, Checks).
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub header: Header,
    /// In the profile's order.
    pub instruments: Vec<Reading>,
    /// The seat's active intents and a pending decision, as their layers render them.
    pub intents: Vec<Rendered>,
    pub decision: Option<Rendered>,
    /// Ranked.
    pub percepts: Vec<Percept>,
    /// The perceived events after `since`, oldest first.
    pub events: Vec<EventView>,
    /// The cursor when no event is chosen: `since`, or the latest seq when none was asked for.
    pub cursor: u64,
    /// Events after `since` already overwritten in the ring.
    pub lost: u32,
}

/// Writes an observation filled to `budget` in `projection` (text or JSON): the mandatory part,
/// the longest prefix of the percepts and events merged by alternation that fits with the omitted
/// section, and the omitted section.
pub fn write_observation(
    o: &Observation,
    projection: Projection,
    budget: u32,
) -> Result<Answer, Problem> {
    let h = &o.header;
    let mut head_text = text::header(h) + &text::instruments(&o.instruments);
    let mut obj = Obj::new();
    json::header(&mut obj, h);
    let readings: Vec<String> = o.instruments.iter().map(json::reading).collect();
    obj.list("instruments", &readings);
    let intents: Vec<String> = o.intents.iter().map(|r| r.json.clone()).collect();
    obj.list("intents", &intents);
    for r in &o.intents {
        head_text.push_str(&text::line(&r.text));
    }
    if let Some(d) = &o.decision {
        obj.raw("decision", &d.json);
        head_text.push_str(&text::line(&d.text));
    }
    let mut head_json = obj.finish();
    head_json.pop();
    Doc {
        projection,
        budget,
        head_text,
        head_json,
        entities: Some(o.percepts.iter().map(percept_item).collect()),
        events: Some(o.events.iter().map(event_item).collect()),
        cursor: Some(o.cursor),
        omitted: true,
        lost: o.lost,
        beyond_events: 0,
        beyond_entities: BTreeMap::new(),
    }
    .finish()
}

/// `perception.unknown_kind` for a kind the game does not declare.
fn check_kinds(source: &Source<'_>, kinds: &[String], events: bool) -> Result<(), Problem> {
    let defs = match source {
        Source::Seat(v) => Some(v.defs()),
        Source::Raw(raw) => raw.defs,
    };
    let Some(defs) = defs else {
        return Ok(());
    };
    let mut valid: Vec<&str> = if events {
        defs.decl.events.iter().map(|e| e.kind.as_str()).collect()
    } else {
        defs.decl.kinds.iter().map(|k| k.kind.as_str()).collect()
    };
    if events {
        valid.push(SIGHTED);
    }
    for (i, k) in kinds.iter().enumerate() {
        let prefix = events && k.ends_with('.');
        if !prefix && !valid.contains(&k.as_str()) && !matches!(source, Source::Raw(_)) {
            let cands: Vec<Candidate<'_>> = valid.iter().map(|v| Candidate::new(v)).collect();
            let path = Pointer::root().key("kinds").index(i);
            return Err(unknown_kind(&path, k, &cands, None));
        }
    }
    Ok(())
}

/// `nearby`: percepts filtered by kind, distance, sector and visibility, ranked the same way.
pub fn nearby(
    world: &World,
    caller: &Caller<'_>,
    req: &NearbyRequest,
    targets: &[EntityId],
    aff: &dyn Affordances,
) -> Result<Answer, Problem> {
    let source = bind(world, caller, req.seat.as_deref(), req.omniscient)?;
    let projection = projection_of(req.projection, req.budget_tokens)?;
    if projection == Projection::Tensor {
        return Err(tensor_refused("only observe answers in tensors"));
    }
    let budget = check_budget(req.budget_tokens, default_budget(&source))?;
    let limit = check_limit(req.limit, NEARBY_LIMIT)?;
    if let Some(k) = &req.kinds {
        check_kinds(&source, k, false)?;
    }
    super::filter::check_nearby(req)?;
    let heading = match &source {
        Source::Seat(v) => v.heading(),
        Source::Raw(_) => 0.0,
    };
    let matching: Vec<Percept> = percepts(&source, targets, aff)
        .into_iter()
        .filter(|p| super::filter::keeps(req, p, |b| relative_deg(b, heading)))
        .collect();
    let h = header(&source);
    write_nearby(&h, &matching, limit, projection, budget)
}

/// Writes a `nearby` answer: the first `limit` of the ranked `matching` percepts that fit
/// `budget`, and the omitted section for the rest.
pub fn write_nearby(
    h: &Header,
    matching: &[Percept],
    limit: usize,
    projection: Projection,
    budget: u32,
) -> Result<Answer, Problem> {
    let mut beyond = BTreeMap::new();
    for p in matching.iter().skip(limit) {
        *beyond.entry(p.kind.clone()).or_insert(0u32) += 1;
    }
    let mut o = Obj::new();
    o.uint("tick", h.tick.0).bool("omniscient", h.omniscient);
    let mut head_json = o.finish();
    head_json.pop();
    Doc {
        projection,
        budget,
        head_text: text::header_short(h.tick.0, h.omniscient),
        head_json,
        entities: Some(matching.iter().take(limit).map(percept_item).collect()),
        events: None,
        cursor: None,
        omitted: true,
        lost: 0,
        beyond_events: 0,
        beyond_entities: beyond,
    }
    .finish()
}

/// `describe` of an entity: its percept, every verb of its kind with whether it is available
/// now, and its last five perceived events (newest kept first when the budget is short).
pub fn describe(
    world: &World,
    caller: &Caller<'_>,
    req: &DescribeRequest,
    aff: &dyn Affordances,
) -> Result<Answer, Problem> {
    let source = bind(world, caller, req.seat.as_deref(), req.omniscient)?;
    let projection = projection_of(req.projection, req.budget_tokens)?;
    if projection == Projection::Tensor {
        return Err(tensor_refused("only observe answers in tensors"));
    }
    let budget = check_budget(req.budget_tokens, default_budget(&source))?;
    let id = resolve(&source, &req.entity)?;
    let (percept, affordances, events) = match &source {
        Source::Seat(v) => {
            let mut p = v.percept(id).ok_or_else(|| {
                pocket_contract::codes::unknown_entity(&Pointer::root().key("entity"), "", [])
            })?;
            p.can = aff.can(v, &p);
            let a = aff.describe(v, &p);
            let ev: Vec<EventView> = v
                .events_since(0)
                .filter(|e| e.subject.as_ref().is_some_and(|s| s.id == id))
                .map(|e| v.event_view(e))
                .collect();
            (p, a, ev)
        }
        Source::Raw(raw) => {
            let e = super::facts::live(raw.world, id).ok_or_else(|| {
                pocket_contract::codes::unknown_entity(&Pointer::root().key("entity"), "", [])
            })?;
            let ev: Vec<EventView> = raw
                .events(0)
                .into_iter()
                .filter(|x| x.subject.as_ref().is_some_and(|s| s.id == id))
                .collect();
            (raw.percept(id, e), Vec::new(), ev)
        }
    };
    let h = header(&source);
    write_describe(&h, &percept, &affordances, &events, projection, budget)
}

/// Writes a `describe` answer: the entity, its affordances, and the last five of `events` (its
/// perceived events, oldest first) that fit `budget`, the newest kept first and written oldest
/// first.
pub fn write_describe(
    h: &Header,
    percept: &Percept,
    affordances: &[AffordanceStatus],
    events: &[EventView],
    projection: Projection,
    budget: u32,
) -> Result<Answer, Problem> {
    let keep = events.len().saturating_sub(DESCRIBE_EVENTS);
    let mut events: Vec<&EventView> = events[keep..].iter().collect();
    events.reverse();
    let mut head_text = text::header_short(h.tick.0, h.omniscient) + &text::entity(percept);
    for a in affordances {
        head_text.push_str(&text::affordance(a));
    }
    let mut o = Obj::new();
    let affs: Vec<String> = affordances.iter().map(json::affordance).collect();
    o.uint("tick", h.tick.0)
        .bool("omniscient", h.omniscient)
        .raw("entity", &json::percept(percept))
        .list("affordances", &affs);
    let mut head_json = o.finish();
    head_json.pop();
    // Newest first decides what fits; the chosen ones are written oldest first.
    let mut doc = Doc {
        projection,
        budget,
        head_text,
        head_json,
        entities: None,
        events: Some(events.iter().map(|e| event_item(e)).collect()),
        cursor: None,
        omitted: false,
        lost: 0,
        beyond_events: 0,
        beyond_entities: BTreeMap::new(),
    };
    let order_len = doc.events.as_ref().map_or(0, Vec::len);
    let limit = usize::try_from(budget)
        .unwrap_or(usize::MAX)
        .saturating_mul(4);
    let k = (0..=order_len)
        .rev()
        .find(|k| doc.size(0, *k) <= limit)
        .unwrap_or(0);
    if let Some(ev) = &mut doc.events {
        ev.truncate(k);
        ev.reverse();
    }
    doc.finish()
}

/// `events`: perceived events after `since`, oldest first, up to `limit` and the budget.
pub fn events(world: &World, caller: &Caller<'_>, req: &EventsRequest) -> Result<Answer, Problem> {
    let source = bind(world, caller, req.seat.as_deref(), req.omniscient)?;
    let projection = projection_of(req.projection, req.budget_tokens)?;
    if projection == Projection::Tensor {
        return Err(tensor_refused("only observe answers in tensors"));
    }
    let budget = check_budget(req.budget_tokens, default_budget(&source))?;
    let limit = check_limit(req.limit, EVENTS_LIMIT)?;
    if let Some(k) = &req.kinds {
        check_kinds(&source, k, true)?;
    }
    let (all, _, lost) = events_after(&source, req.since)?;
    let kinds = req.kinds.as_deref();
    let matching: Vec<EventView> = all
        .into_iter()
        .filter(|e| {
            kinds.is_none_or(|ks| {
                ks.iter().any(|k| {
                    if k.ends_with('.') {
                        e.kind.starts_with(k.as_str())
                    } else {
                        e.kind == *k
                    }
                })
            })
        })
        .collect();
    let h = header(&source);
    write_events(&h, &matching, req.since, lost, limit, projection, budget)
}

/// Writes an `events` answer: the first `limit` of `matching` (the perceived events after
/// `since`, oldest first) that fit `budget`, the cursor after them and what was left out.
pub fn write_events(
    h: &Header,
    matching: &[EventView],
    since: u64,
    lost: u32,
    limit: usize,
    projection: Projection,
    budget: u32,
) -> Result<Answer, Problem> {
    let beyond = u32::try_from(matching.len().saturating_sub(limit)).unwrap_or(u32::MAX);
    let mut o = Obj::new();
    o.uint("tick", h.tick.0).bool("omniscient", h.omniscient);
    let mut head_json = o.finish();
    head_json.pop();
    Doc {
        projection,
        budget,
        head_text: text::header_short(h.tick.0, h.omniscient),
        head_json,
        entities: None,
        events: Some(matching.iter().take(limit).map(event_item).collect()),
        cursor: Some(since),
        omitted: true,
        lost,
        beyond_events: beyond,
        beyond_entities: BTreeMap::new(),
    }
    .finish()
}

/// The perceived events an act or time answer carries to a seat (perception.md, Push): those
/// after `since` that fit `limit_bytes`, as event lines (text) or JSON values, the cursor after
/// them, and what was left out (written as `omitted-events` in text).
#[derive(Clone, Debug, PartialEq)]
pub struct Delta {
    pub events: Vec<String>,
    pub text: String,
    pub cursor: u64,
    pub omitted: Omitted,
}

/// The events delta of `seat` after `since` in `projection` within `limit_bytes`.
pub fn delta(
    world: &World,
    seat: &str,
    since: u64,
    projection: Projection,
    limit_bytes: usize,
) -> Result<Delta, Problem> {
    let caller = Caller {
        role: super::query::Role::Player,
        seat: Some(seat),
    };
    let source = bind(world, &caller, None, None)?;
    let (events, _, lost) = events_after(&source, since)?;
    let items: Vec<Item> = events.iter().map(event_item).collect();
    let om = |n: usize| Omitted {
        entities: BTreeMap::new(),
        events: u32::try_from(items.len() - n).unwrap_or(u32::MAX),
        lost,
        cursor: if n > 0 { items[n - 1].seq } else { since },
    };
    let size = |n: usize| match projection {
        Projection::Json => array_len(items[..n].iter().map(|i| i.json.len())),
        _ => {
            items[..n].iter().map(|i| i.text.len()).sum::<usize>()
                + text::omitted_events(&om(n)).len()
        }
    };
    // The longest prefix that fits; when not even the omitted line fits (text), nothing is
    // written and the cursor stays at `since` (docs/spec/perception-slice2.md 5).
    let fits = (0..=items.len()).rev().find(|n| size(*n) <= limit_bytes);
    let n = fits.unwrap_or(0);
    let omitted = om(n);
    let mut lines: String = items[..n].iter().map(|i| i.text.as_str()).collect();
    if fits.is_some() {
        lines.push_str(&text::omitted_events(&omitted));
    }
    Ok(Delta {
        events: items[..n].iter().map(|i| i.json.clone()).collect(),
        text: lines,
        cursor: omitted.cursor,
        omitted,
    })
}
