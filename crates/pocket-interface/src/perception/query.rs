//! Perception's queries (shared/contract/perception.md, Queries): their requests, the caller's
//! role and seat, which view answers (the seat's observer, or the marked omniscient view), entity
//! references resolved among what the caller may know, and the hooks the action and time layers
//! give (affordances, active intents, a pending decision). Every query is read-only: it takes the
//! world by shared reference and changes nothing (`contract.perception.read_only`). The answers
//! themselves, filled to their token budgets, are [`super::fill`]'s.

use bevy_ecs::prelude::World;
use pocket_contract::codes::{
    Range, ambiguous_ref, missing_field, omniscient_forbidden, out_of_range, seat_not_yours,
    seat_unknown, unknown_entity,
};
use pocket_contract::{Candidate, Pointer, Problem};
use pocket_sim::EntityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::omniscient::Raw;
use super::state::Visibility;
use super::view::{Percept, PerceptionView, seat_body, seats};
use crate::projection::{Projection, Rendered};

/// The largest budget a query takes (perception.md, Token budgets).
pub const MAX_BUDGET: u32 = 16000;
/// The raw omniscient view's default budget, which has no profile to take one from.
pub const RAW_BUDGET: u32 = 2000;
/// `nearby`'s default and largest `limit`.
pub const NEARBY_LIMIT: u32 = 100;
/// `events`' default and largest `limit`.
pub const EVENTS_LIMIT: u32 = 200;
/// How many of an entity's latest events `describe` shows.
pub const DESCRIBE_EVENTS: usize = 5;

/// `observe`: the default call, a summary of the observer's situation ranked for relevance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveRequest {
    /// Player role: omitted or its own seat. Developer role: required unless `omniscient`.
    #[serde(default)]
    pub seat: Option<String>,
    /// Default: the profile's `budget_tokens`; at most 16000.
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    /// Default: json.
    #[serde(default)]
    pub projection: Option<Projection>,
    /// Events after this per-observer seq are included; default: none.
    #[serde(default)]
    pub since: Option<u64>,
    /// Developer and checker roles only: the whole world, marked.
    #[serde(default)]
    pub omniscient: Option<bool>,
}

/// An arc of directions: clockwise from `from_deg` to `to_deg`, inclusive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sector {
    pub from_deg: f64,
    pub to_deg: f64,
    /// true: angles off the heading, -30..30 is ahead.
    #[serde(default)]
    pub relative: bool,
}

/// `nearby`: percepts filtered by kind, distance, sector and visibility, ranked.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NearbyRequest {
    #[serde(default)]
    pub seat: Option<String>,
    /// Default: every kind.
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Default: no limit beyond perception's own.
    #[serde(default)]
    pub within_m: Option<f64>,
    /// Default: all round.
    #[serde(default)]
    pub sector: Option<Sector>,
    /// Default: all three.
    #[serde(default)]
    pub visibility: Option<Vec<Visibility>>,
    /// Default and maximum: 100.
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    #[serde(default)]
    pub projection: Option<Projection>,
    #[serde(default)]
    pub omniscient: Option<bool>,
}

/// An entity reference (README, Positions and entity references): an id, `"Name#id"` or `"#id"`,
/// `{"id", "name"}`, or a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EntityRef {
    Id(u64),
    Named {
        id: u64,
        #[serde(default)]
        name: Option<String>,
    },
    Text(String),
}

/// `describe` of one entity: every fact its visibility and detail allow, its affordances, and
/// the last five perceived events whose subject it is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeRequest {
    #[serde(default)]
    pub seat: Option<String>,
    pub entity: EntityRef,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    #[serde(default)]
    pub projection: Option<Projection>,
    #[serde(default)]
    pub omniscient: Option<bool>,
}

/// `events`: perceived events after `since`, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventsRequest {
    #[serde(default)]
    pub seat: Option<String>,
    /// Events with seq > since.
    pub since: u64,
    /// Exact kinds, or prefixes ending in ".".
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Default and maximum: 200.
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    #[serde(default)]
    pub projection: Option<Projection>,
    /// Developer and checker roles only: the world's events, marked; `seq` and `cursor` are then
    /// spec-sim's `EventSeq`.
    #[serde(default)]
    pub omniscient: Option<bool>,
}

/// Who asks (README, Seats and callers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Player,
    Developer,
    Checker,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Player => "player",
            Role::Developer => "developer",
            Role::Checker => "checker",
        }
    }
}

/// The caller of a query: its role and, for a player, its seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caller<'a> {
    pub role: Role,
    pub seat: Option<&'a str>,
}

/// One verb's state for `describe` (actions.md, Affordances).
#[derive(Clone, Debug, PartialEq)]
pub struct AffordanceStatus {
    pub verb: String,
    pub available: bool,
    /// Every unmet requirement, not only the first.
    pub unmet: Vec<Problem>,
}

/// What the action layer adds to percepts: the verbs available now (`can`) and, for `describe`,
/// every verb with why it is not available. Evaluated through the actor's view, so it can only
/// depend on what the actor perceives.
pub trait Affordances {
    fn can(&self, _view: &PerceptionView<'_>, _percept: &Percept) -> Vec<String> {
        Vec::new()
    }

    fn describe(&self, _view: &PerceptionView<'_>, _percept: &Percept) -> Vec<AffordanceStatus> {
        Vec::new()
    }
}

/// No affordances: perception alone.
pub struct NoAffordances;

impl Affordances for NoAffordances {}

/// What the action and time layers add to an observation's mandatory part: the seat's active
/// intents (actions.md), a pending decision (time.md), and the intents' target entities, which
/// rank first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeatParts {
    pub intents: Vec<Rendered>,
    pub decision: Option<Rendered>,
    pub targets: Vec<EntityId>,
}

/// A projected answer: its bytes, the token estimate of them, and the cursor after the events
/// it carries (for the push cursor, perception.md, Push).
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub projection: Projection,
    pub body: String,
    pub tokens: u32,
    pub cursor: Option<u64>,
}

/// Which view answers a query.
pub enum Source<'w> {
    /// The seat's observer (marked when it is bound to `omniscient_player`).
    Seat(PerceptionView<'w>),
    /// The raw omniscient view.
    Raw(Raw<'w>),
}

impl Source<'_> {
    pub fn omniscient(&self) -> bool {
        match self {
            Source::Seat(v) => v.omniscient(),
            Source::Raw(_) => true,
        }
    }
}

/// The view a caller's request is answered from (README, Seats and callers): a player gets its own
/// seat's perception, `seat.not_yours` for another seat and `perception.omniscient_forbidden` for
/// the omniscient view; a developer or checker names a seat or asks for the omniscient view.
pub fn bind<'w>(
    world: &'w World,
    caller: &Caller<'_>,
    seat: Option<&str>,
    omniscient: Option<bool>,
) -> Result<Source<'w>, Problem> {
    let path = Pointer::root().key("seat");
    let wants_omni = omniscient == Some(true);
    let seat = match caller.role {
        Role::Player => {
            if wants_omni {
                return Err(omniscient_forbidden(caller.role.name()));
            }
            let yours = caller.seat.unwrap_or("");
            match seat {
                Some(s) if s != yours => return Err(seat_not_yours(&path, s, yours)),
                _ => yours,
            }
        }
        Role::Developer | Role::Checker => {
            if wants_omni {
                let origin = seat.and_then(|s| seat_body(world, s));
                return Raw::new(world, origin)
                    .map(Source::Raw)
                    .ok_or_else(|| unknown_seat(world, &path, seat.unwrap_or("")));
            }
            match seat {
                Some(s) => s,
                None => {
                    return Err(missing_field(
                        &path,
                        "a developer's or checker's perception request",
                        "a seat, or \"omniscient\": true",
                    ));
                }
            }
        }
    };
    PerceptionView::for_seat(world, seat)
        .map(Source::Seat)
        .ok_or_else(|| unknown_seat(world, &path, seat))
}

fn unknown_seat(world: &World, path: &Pointer, seat: &str) -> Problem {
    let all = seats(world);
    let valid: Vec<Candidate<'_>> = all.iter().map(|s| Candidate::new(s)).collect();
    seat_unknown(path, seat, &valid)
}

/// `request.out_of_range` for a budget above 16000 (or 0).
pub fn check_budget(budget: Option<u32>, default: u32) -> Result<u32, Problem> {
    let b = budget.unwrap_or(default);
    if b == 0 || b > MAX_BUDGET {
        let range = Range::inclusive(1, MAX_BUDGET);
        return Err(out_of_range(
            &Pointer::root().key("budget_tokens"),
            &Value::from(b),
            &range,
            None,
        ));
    }
    Ok(b)
}

/// `request.out_of_range` for a limit outside 1 to `max`.
pub fn check_limit(limit: Option<u32>, max: u32) -> Result<usize, Problem> {
    let l = limit.unwrap_or(max);
    if l == 0 || l > max {
        let range = Range::inclusive(1, max);
        return Err(out_of_range(
            &Pointer::root().key("limit"),
            &Value::from(l),
            &range,
            None,
        ));
    }
    Ok(usize::try_from(l).unwrap_or(usize::MAX))
}

/// Resolves an entity reference among what the source knows; an entity it does not know is
/// answered exactly as one that does not exist (`perception.unknown_entity`), and a name two
/// known entities share is `request.ambiguous_ref` with the candidates.
pub fn resolve(source: &Source<'_>, r: &EntityRef) -> Result<EntityId, Problem> {
    let path = Pointer::root().key("entity");
    let names = match source {
        Source::Seat(v) => v.known_names(),
        Source::Raw(raw) => raw.known_names(),
    };
    let knows = |id: EntityId| match source {
        Source::Seat(v) => v.knows(id),
        Source::Raw(raw) => super::facts::live(raw.world, id).is_some(),
    };
    let text = match r {
        EntityRef::Id(n) => format!("#{n}"),
        EntityRef::Named { id, name } => format!("{}#{id}", name.as_deref().unwrap_or("")),
        EntityRef::Text(s) => s.clone(),
    };
    let unknown = || unknown_entity(&path, &text, names.iter().map(|n| n.1.as_str()));
    let by_id = |id: u64, name: Option<&str>| -> Result<EntityId, Problem> {
        let id = EntityId::new(id)
            .filter(|id| knows(*id))
            .ok_or_else(unknown)?;
        if let Some(n) = name.filter(|n| !n.is_empty())
            && !names.iter().any(|k| k.0 == id && k.1 == n)
        {
            return Err(unknown());
        }
        Ok(id)
    };
    match r {
        EntityRef::Id(n) => by_id(*n, None),
        EntityRef::Named { id, name } => by_id(*id, name.as_deref()),
        EntityRef::Text(s) => {
            if let Some((name, id)) = s.rsplit_once('#')
                && !id.is_empty()
                && id.bytes().all(|c| c.is_ascii_digit())
                && let Ok(id) = id.parse::<u64>()
            {
                return by_id(id, Some(name));
            }
            let found: Vec<&(EntityId, String, String)> =
                names.iter().filter(|n| n.1 == *s).collect();
            match found.as_slice() {
                [] => Err(unknown()),
                [one] => Ok(one.0),
                many => {
                    let candidates: Vec<(u64, &str, &str)> = many
                        .iter()
                        .map(|n| (n.0.get(), n.1.as_str(), n.2.as_str()))
                        .collect();
                    Err(ambiguous_ref(&path, s, &candidates))
                }
            }
        }
    }
}
