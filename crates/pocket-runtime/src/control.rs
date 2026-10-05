//! Time commands' parameters and stop conditions (docs/spec/host-protocol.md 4; docs/spec/server.md):
//! `time.control`, `time.step` with `until` (an event is emitted, a tick is reached) and `watch` (a
//! component field changes, crosses a value or meets a comparison), `play.start` and
//! `snapshots.restore`. A stop condition is checked after each tick, on the world the tick left; it
//! reads the world and never changes it.

use pocket_contract::{Pointer, Problem, detail};
use pocket_sim::{EntityId, Event};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Game;
use crate::edit::component_json;
use crate::inspect::field_at;
use crate::scene::unknown_component;
use crate::values::{EntityRef, resolve};

pub use pocket_interface::Pacing;

/// The most ticks one `time.step` runs.
pub const MAX_STEP_TICKS: u64 = 1_000_000;

/// The ticks a `time.step` with a stop condition runs at most when `ticks` is not given.
pub const DEFAULT_UNTIL_TICKS: u64 = 3600;

/// `time.control`'s parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeControlParams {
    /// Pause (true) or resume (false).
    #[serde(default)]
    pub pause: Option<bool>,
    /// Real time at this multiple of real time (0 < speed <= 1000).
    #[serde(default)]
    pub speed: Option<f64>,
    /// `"stepped"` or `{"real_time": {"speed": x}}`.
    #[serde(default)]
    pub pacing: Option<Pacing>,
}

/// `time.step`'s parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepParams {
    /// Ticks to run; with `until` or `watch`, the most to run (default 3600).
    #[serde(default)]
    pub ticks: Option<u64>,
    /// Stop after the tick that emits an event, or at a tick.
    #[serde(default)]
    pub until: Option<Until>,
    /// Stop after the tick that changes a component field so its test holds.
    #[serde(default)]
    pub watch: Option<Watch>,
}

/// An event or tick stop condition.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Until {
    /// An event name (`crate.taken`), or a prefix ending in `.*` (`crate.*`).
    #[serde(default)]
    pub event: Option<String>,
    /// Only events about this entity.
    #[serde(default)]
    pub subject: Option<EntityRef>,
    /// Stop when the world reaches this tick.
    #[serde(default)]
    pub tick: Option<u64>,
}

/// A field stop condition.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub entity: EntityRef,
    pub component: String,
    /// A field or a path into the component (`speed`, `position.y`).
    pub field: String,
    /// Default: `crosses` when `value` is given, else `changes`.
    #[serde(default)]
    pub op: Option<WatchOp>,
    #[serde(default)]
    pub value: Option<Value>,
}

/// A watch's test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum WatchOp {
    #[serde(rename = "changes")]
    Changes,
    /// The field passes `value` in either direction.
    #[serde(rename = "crosses")]
    Crosses,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
}

/// `play.start`'s parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayParams {
    /// Real-time speed, default 1.
    #[serde(default)]
    pub speed: Option<f64>,
    /// Start paused.
    #[serde(default)]
    pub paused: bool,
}

/// `snapshots.restore`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotsRestoreParams {
    /// The kept snapshot at or before this tick is restored.
    pub tick: u64,
}

fn bad(path: &str, why: &str) -> Problem {
    Problem::new(
        "request.invalid_value",
        format!("'{path}': {why}."),
        detail([("path", json!(path))]),
    )
}

impl StepParams {
    /// The ticks to run at most.
    pub fn limit(&self) -> Result<u64, Problem> {
        let conditional = self.until.is_some() || self.watch.is_some();
        let n = self
            .ticks
            .unwrap_or(if conditional { DEFAULT_UNTIL_TICKS } else { 1 });
        if n > MAX_STEP_TICKS {
            return Err(pocket_contract::codes::out_of_range(
                &Pointer::root().key("ticks"),
                &json!(n),
                &pocket_contract::codes::Range::inclusive(0, MAX_STEP_TICKS),
                None,
            ));
        }
        Ok(n)
    }
}

struct WatchState {
    id: EntityId,
    component: String,
    field: String,
    op: WatchOp,
    value: Option<Value>,
    previous: Value,
}

/// A `time.step`'s stop conditions, checked after each tick.
pub struct StepStop {
    event: Option<String>,
    subject: Option<EntityId>,
    tick: Option<u64>,
    watch: Option<WatchState>,
}

fn read_field(game: &Game, id: EntityId, component: &str, field: &str) -> Option<Value> {
    let e = pocket_sim::entity::entity(game.world(), id)?;
    let v = component_json(game.world(), e, component)?;
    field_at(&v, field).cloned()
}

fn cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn event_json(e: &Event) -> Value {
    json!({"id": e.seq.0, "tick": e.tick.0, "name": e.kind.as_str(),
           "subject": e.subject.map(|s| s.get()), "cause": e.cause.map(|c| c.0),
           "data": e.data.to_json()})
}

impl StepStop {
    /// The stop conditions of `p`, checked against the world now; `None` without any.
    pub fn new(game: &Game, p: &StepParams) -> Result<Option<StepStop>, Problem> {
        if p.until.is_none() && p.watch.is_none() {
            return Ok(None);
        }
        let world = game.world();
        let mut stop = StepStop {
            event: None,
            subject: None,
            tick: None,
            watch: None,
        };
        if let Some(u) = &p.until {
            if u.event.is_none() && u.tick.is_none() {
                return Err(bad("/until", "give an event or a tick"));
            }
            if let Some(ev) = &u.event
                && !ev.ends_with(".*")
            {
                pocket_sim::EventKind::new(ev.as_str())?;
            }
            stop.event = u.event.clone();
            stop.subject = u
                .subject
                .as_ref()
                .map(|s| resolve(world, s, &Pointer::root().key("until").key("subject")))
                .transpose()?;
            stop.tick = u.tick;
        }
        if let Some(w) = &p.watch {
            let at = Pointer::root().key("watch");
            let id = resolve(world, &w.entity, &at.key("entity"))?;
            if world
                .resource::<pocket_sim::ComponentRegistry>()
                .get(&w.component)
                .is_none()
            {
                return Err(unknown_component(world, &w.component));
            }
            let previous = read_field(game, id, &w.component, &w.field).ok_or_else(|| {
                bad(
                    "/watch/field",
                    &format!(
                        "entity {} has no {}.{} to watch",
                        id.get(),
                        w.component,
                        w.field
                    ),
                )
            })?;
            let op = w.op.unwrap_or(if w.value.is_some() {
                WatchOp::Crosses
            } else {
                WatchOp::Changes
            });
            if op != WatchOp::Changes && w.value.is_none() {
                return Err(bad("/watch/value", "this test needs a value"));
            }
            stop.watch = Some(WatchState {
                id,
                component: w.component.clone(),
                field: w.field.clone(),
                op,
                value: w.value.clone(),
                previous,
            });
        }
        Ok(Some(stop))
    }

    fn event_matches(&self, e: &Event) -> bool {
        let Some(want) = &self.event else {
            return false;
        };
        let name_ok = match want.strip_suffix(".*") {
            Some(prefix) => e
                .kind
                .as_str()
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.')),
            None => e.kind.as_str() == want,
        };
        name_ok && self.subject.is_none_or(|s| e.subject == Some(s))
    }

    /// After a tick: why to stop, or `None` to go on.
    pub fn after_tick(&mut self, game: &Game) -> Option<Value> {
        if self.event.is_some()
            && let Some(e) = game.inbox().iter().find(|e| self.event_matches(e))
        {
            return Some(json!({"reason": "event", "event": event_json(e)}));
        }
        if let Some(t) = self.tick
            && game.tick().0 >= t
        {
            return Some(json!({"reason": "tick", "tick": t}));
        }
        if let Some(w) = &mut self.watch {
            let Some(now) = read_field(game, w.id, &w.component, &w.field) else {
                return Some(
                    json!({"reason": "watch", "gone": true, "entity": w.id.get(),
                                   "field": format!("{}.{}", w.component, w.field)}),
                );
            };
            let target = w.value.clone().unwrap_or(Value::Null);
            use std::cmp::Ordering::{Equal, Greater, Less};
            let hit = match w.op {
                WatchOp::Changes => now != w.previous,
                WatchOp::Crosses => {
                    let before = cmp(&w.previous, &target);
                    let after = cmp(&now, &target);
                    matches!(
                        (before, after),
                        (Some(Less), Some(Greater | Equal)) | (Some(Greater), Some(Less | Equal))
                    )
                }
                WatchOp::Gt => cmp(&now, &target) == Some(Greater),
                WatchOp::Ge => matches!(cmp(&now, &target), Some(Greater | Equal)),
                WatchOp::Lt => cmp(&now, &target) == Some(Less),
                WatchOp::Le => matches!(cmp(&now, &target), Some(Less | Equal)),
                WatchOp::Eq => now == target,
                WatchOp::Ne => now != target,
            };
            let previous = std::mem::replace(&mut w.previous, now.clone());
            if hit {
                return Some(json!({"reason": "watch", "entity": w.id.get(),
                                   "field": format!("{}.{}", w.component, w.field),
                                   "value": now, "previous": previous}));
            }
        }
        None
    }
}
