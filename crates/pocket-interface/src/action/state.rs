//! What actions keep in the world (shared/contract/actions.md, Controls and Intent instances): the
//! controls waiting on a seat's body, and every intent instance with the counter that names them.
//! All of it is world state, persisted in its own form (README, World state and wire forms): enums externally tagged, a failure as its code and detail only, free data as
//! [`PlainData`], so a fork, a restore and a replay carry it and the world hash covers it.

use std::collections::{BTreeMap, VecDeque};

use bevy_ecs::prelude::{Component, Resource, World};
use pocket_contract::{Problem, detail};
use pocket_sim::persisted::{Persisted, RegisterPersisted};
use pocket_sim::{EntityId, PlainData, Tick};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_reflection::{Samples, Tracer};

/// The most recently finished intents kept per seat (actions.md, Intent instances).
pub const KEEP_FINISHED: usize = 32;

/// A control's value: a latched number, boolean or choice (actions.md, Controls). The wire form is
/// untagged ([`ControlValue::to_json`]); this persisted form is externally tagged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ControlValue {
    Bool(bool),
    Number(f64),
    Name(String),
}

impl ControlValue {
    /// The wire form: `true`, `0.25` or `"best"`.
    pub fn to_json(&self) -> Value {
        match self {
            ControlValue::Bool(b) => Value::Bool(*b),
            ControlValue::Number(x) => pocket_contract::codes::num(*x),
            ControlValue::Name(s) => Value::String(s.clone()),
        }
    }
}

/// One pulse waiting: it acts in exactly one tick, on its target if it takes one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Pulse {
    pub target: Option<EntityId>,
}

/// The controls a body holds beyond those bound to a component field: latched values of unbound
/// controls, and the pulses waiting per control, oldest first. Each tick `interface.intents`
/// delivers the front pulse of each control and removes it, so two pulses sent before one tick act
/// in two ticks (actions.md, Controls).
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Controls {
    pub values: BTreeMap<String, ControlValue>,
    pub pulses: BTreeMap<String, VecDeque<Pulse>>,
}

/// An intent's status (actions.md, The lifecycle).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    Active,
    Holding,
    Succeeded,
    Failed,
    Cancelled,
    Superseded,
}

impl IntentStatus {
    /// Active or holding: its executor runs and it occupies its channels.
    pub fn is_live(self) -> bool {
        matches!(self, IntentStatus::Active | IntentStatus::Holding)
    }

    pub fn name(self) -> &'static str {
        match self {
            IntentStatus::Active => "active",
            IntentStatus::Holding => "holding",
            IntentStatus::Succeeded => "succeeded",
            IntentStatus::Failed => "failed",
            IntentStatus::Cancelled => "cancelled",
            IntentStatus::Superseded => "superseded",
        }
    }
}

/// An intent's target with its references resolved.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ResolvedTarget {
    Entity(EntityId),
    /// A point in the contract frame, metres.
    Point([f64; 3]),
}

impl ResolvedTarget {
    pub fn entity(&self) -> Option<EntityId> {
        match self {
            ResolvedTarget::Entity(e) => Some(*e),
            ResolvedTarget::Point(_) => None,
        }
    }
}

/// A problem as world state keeps it: its code and detail, the message rendered when it is shown
/// (README, World state and wire forms), so no English enters the world hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StoredProblem {
    pub code: String,
    pub detail: PlainData,
}

impl StoredProblem {
    pub fn of(p: &Problem) -> StoredProblem {
        StoredProblem {
            code: p.code.clone(),
            detail: PlainData::from_json(&Value::Object(p.detail.clone())),
        }
    }
}

/// One intent: what was asked, of which seat, and how it stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IntentInstance {
    pub id: u64,
    pub seat: String,
    /// The seat's body when the intent started: the actor its executor perceives through and acts
    /// for.
    pub actor: EntityId,
    pub intent: String,
    pub tag: Option<String>,
    /// Canonical: aliases resolved and defaults filled.
    pub params: PlainData,
    pub target: Option<ResolvedTarget>,
    /// The first tick its executor runs.
    pub started_tick: Tick,
    pub deadline_tick: Option<Tick>,
    pub status: IntentStatus,
    pub finished_tick: Option<Tick>,
    pub failure: Option<StoredProblem>,
    pub superseded_by: Option<u64>,
    /// The progress readings, by name, in the order the intent declares them.
    pub progress: Vec<(String, PlainData)>,
    /// The executor's own state between ticks.
    pub state: PlainData,
}

/// Every intent instance and the counter that names them, from 1, never reused; `by_id` iterates
/// in `IntentId` order.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IntentTable {
    pub next_id: u64,
    pub by_id: BTreeMap<u64, IntentInstance>,
}

impl Default for IntentTable {
    fn default() -> Self {
        IntentTable {
            next_id: 1,
            by_id: BTreeMap::new(),
        }
    }
}

impl IntentTable {
    /// The live (active or holding) intents of `seat` that occupy `channel`, given each intent's
    /// channels.
    pub fn live_of<'a>(&'a self, seat: &'a str) -> impl Iterator<Item = &'a IntentInstance> + 'a {
        self.by_id
            .values()
            .filter(move |i| i.seat == seat && i.status.is_live())
    }

    /// Drops the oldest finished intents of each seat beyond [`KEEP_FINISHED`]: oldest
    /// `finished_tick` first, then lowest id.
    pub fn prune(&mut self) {
        let mut per_seat: BTreeMap<&str, Vec<(Tick, u64)>> = BTreeMap::new();
        for i in self.by_id.values() {
            if let Some(t) = i.finished_tick {
                per_seat.entry(i.seat.as_str()).or_default().push((t, i.id));
            }
        }
        let mut drop = Vec::new();
        for (_, mut done) in per_seat {
            if done.len() > KEEP_FINISHED {
                done.sort();
                let extra = done.len() - KEEP_FINISHED;
                drop.extend(done.into_iter().take(extra).map(|(_, id)| id));
            }
        }
        for id in drop {
            self.by_id.remove(&id);
        }
    }
}

impl Persisted for Controls {
    const NAME: &'static str = "Controls";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<ControlValue>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for IntentTable {
    const NAME: &'static str = "IntentTable";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<PlainData>(s)?;
        t.trace_type::<IntentStatus>(s)?;
        t.trace_type::<ResolvedTarget>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

/// Declares the action state's persistence classes (persistence.md 2).
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.component::<Controls>().resource::<IntentTable>();
}

/// A seat as the action layer reads it: its id, its place in the order and its body.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatRow {
    pub id: String,
    /// The seat's place in the catalog's declared seats, which `Source::Player(index)` names: a
    /// player's identity, stable while the game runs. `None` for a seat the game does not declare
    /// (an observer naming another seat): a developer may act and read for it, no player plays it
    /// (actions-slice2.md 12).
    pub index: Option<u32>,
    pub body: EntityId,
    pub controls: Option<Vec<String>>,
    pub intents: Option<Vec<String>>,
}

impl SeatRow {
    pub fn may_control(&self, name: &str) -> bool {
        self.controls
            .as_ref()
            .is_none_or(|c| c.iter().any(|n| n == name))
    }

    pub fn may_intend(&self, name: &str) -> bool {
        self.intents
            .as_ref()
            .is_none_or(|c| c.iter().any(|n| n == name))
    }
}

fn seat_name_ok(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The game's seats in seat order: the seats the catalog declares, in its order, then any other
/// seat a body names, in `EntityId` order, without an index (README, Seats and callers; an index
/// that moved when an earlier undeclared seat's body went would hand a player another seat's
/// identity). A seat's body is the entity
/// whose observer names it (perception.md, Observers), found through the catalog's `bodies`.
/// `definition.invalid` for a malformed id or two bodies naming one seat.
pub fn seats(world: &World) -> Result<Vec<SeatRow>, Problem> {
    match world.get_resource::<super::catalog::ActionCatalog>() {
        Some(catalog) => seats_with(world, catalog),
        None => Ok(Vec::new()),
    }
}

/// [`seats`] with the catalog given: inside `interface.intents`, which holds the catalog out of
/// the world while it runs.
pub fn seats_with(
    world: &World,
    catalog: &super::catalog::ActionCatalog,
) -> Result<Vec<SeatRow>, Problem> {
    let bodies = (catalog.bodies)(world);
    let invalid = |i: usize, reason: String| {
        pocket_contract::codes::definition_invalid(
            &pocket_contract::Pointer::root().key("seats").index(i),
            &reason,
        )
    };
    for (i, (id, body)) in bodies.iter().enumerate() {
        if !seat_name_ok(id) {
            return Err(invalid(i, format!("seat id '{id}' is not [a-z][a-z0-9_]*")));
        }
        if let Some((_, other)) = bodies[..i].iter().find(|(s, _)| s == id) {
            return Err(invalid(
                i,
                format!(
                    "seat '{id}' has two bodies, #{} and #{}",
                    other.get(),
                    body.get()
                ),
            ));
        }
    }
    let mut rows = Vec::new();
    for (i, d) in catalog.seats.iter().enumerate() {
        if let Some((_, body)) = bodies.iter().find(|(s, _)| *s == d.id) {
            rows.push(SeatRow {
                id: d.id.clone(),
                index: u32::try_from(i).ok(),
                body: *body,
                controls: d.controls.clone(),
                intents: d.intents.clone(),
            });
        }
    }
    for (id, body) in &bodies {
        if !catalog.seats.iter().any(|d| d.id == *id) {
            rows.push(SeatRow {
                id: id.clone(),
                index: None,
                body: *body,
                controls: None,
                intents: None,
            });
        }
    }
    Ok(rows)
}

/// The declared seat whose index is `index` (`Source::Player(index)`), when its body is in the
/// world.
pub fn seat_by_index(world: &World, index: u32) -> Result<Option<SeatRow>, Problem> {
    Ok(seats(world)?.into_iter().find(|s| s.index == Some(index)))
}

/// `internal.error {where, report}` with an action-layer report.
pub fn internal(where_: &str, report: &str) -> Problem {
    pocket_contract::codes::internal_error(where_, report)
}

/// `Problem` from a stored one, given its message.
pub fn shown(stored: &StoredProblem, message: String) -> Problem {
    let detail = match stored.detail.to_json() {
        Value::Object(m) => m,
        other => detail([("value", other)]),
    };
    Problem {
        code: stored.code.clone(),
        message,
        detail,
    }
}

/// An instance's progress as the wire's readings: `[{"name": .., "value": ..}]`.
pub fn readings_json(progress: &[(String, PlainData)]) -> Value {
    Value::Array(
        progress
            .iter()
            .map(|(n, v)| json!({"name": n, "value": v.to_json()}))
            .collect(),
    )
}
