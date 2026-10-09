//! What an actor knows, as the action layer asks it (actions.md, Executors, rule 1; Affordances):
//! its instruments, and the entities it perceives, remembers or has on its chart. Validation,
//! affordance requirements and executors read the world only through an [`ActorView`], so nothing
//! an intent does or a refusal says depends on what the seat cannot perceive.
//!
//! The view is perception's (shared/contract/perception.md, `PerceptionView`); this trait is the
//! part of it actions use, so the action layer builds and is tested apart from the perception
//! update. The world's [`super::catalog::ActionCatalog`] says which implementation a game uses.

use pocket_sim::{EntityId, Tick};
use serde_json::Value;

/// How an entity is known (perception.md, The perception update).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    /// Visible at the end of this tick.
    Seen,
    /// Seen before, in memory, not visible now.
    Remembered,
    /// Never seen (or forgotten) but on the chart.
    Charted,
}

impl Visibility {
    pub fn name(self) -> &'static str {
        match self {
            Visibility::Seen => "seen",
            Visibility::Remembered => "remembered",
            Visibility::Charted => "charted",
        }
    }
}

/// One entity as the actor knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Known {
    pub id: EntityId,
    pub name: Option<String>,
    pub kind: String,
    pub visibility: Visibility,
    /// The live position when seen, the remembered one when remembered, the chart's when charted;
    /// metres in the contract frame.
    pub pos_m: [f64; 3],
    /// Horizontal, from the actor's body origin to `pos_m` (perception.md, Geometry).
    pub range_m: f64,
    pub bearing_deg: f64,
    /// The facts the actor may know of it, by name, as JSON values.
    pub facts: Vec<(String, Value)>,
}

impl Known {
    pub fn fact(&self, name: &str) -> Option<&Value> {
        self.facts.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// The text projection's form, `Name#id`.
    pub fn label(&self) -> String {
        pocket_contract::render::entity(self.id.get(), self.name.as_deref())
    }

    pub fn entity_name(&self) -> pocket_contract::codes::EntityName {
        pocket_contract::codes::EntityName::new(self.id.get(), self.name.as_deref())
    }
}

/// An event the actor perceived: its perceived kind and data, and the world event's sequence
/// number when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct KnownEvent {
    /// The observer's own sequence number.
    pub seq: u64,
    pub world_seq: Option<u64>,
    pub tick: Tick,
    pub kind: String,
    pub subject: Option<EntityId>,
    pub data: Vec<(String, Value)>,
}

/// What one actor knows at the current tick.
pub trait ActorView {
    fn tick(&self) -> Tick;
    /// The actor's body.
    fn body(&self) -> EntityId;
    /// One of the actor's instruments, as a JSON value (a position as `{x, y, z}`).
    fn instrument(&self, name: &str) -> Option<Value>;
    /// An entity the actor knows; `None` for one it does not, existing or not.
    fn known(&self, id: EntityId) -> Option<Known>;
    /// Every entity the actor knows, ascending by id.
    fn all_known(&self) -> Vec<Known>;
    /// The events the actor perceived with an observer sequence number above `seq`, oldest first.
    fn events_since(&self, seq: u64) -> Vec<KnownEvent>;
}

/// A number instrument.
pub fn number(view: &dyn ActorView, name: &str) -> Option<f64> {
    view.instrument(name).and_then(|v| v.as_f64())
}

/// A position instrument (`{x, y, z}`).
pub fn position(view: &dyn ActorView, name: &str) -> Option<[f64; 3]> {
    let v = view.instrument(name)?;
    Some([
        v.get("x")?.as_f64()?,
        v.get("y")?.as_f64()?,
        v.get("z")?.as_f64()?,
    ])
}

/// A position fact or JSON value `{x, y?, z}`.
pub fn point(v: &Value) -> Option<[f64; 3]> {
    Some([
        v.get("x")?.as_f64()?,
        v.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        v.get("z")?.as_f64()?,
    ])
}

/// The view of an actor that perceives nothing: only the tick and its body.
pub struct Nothing {
    tick: Tick,
    body: EntityId,
}

impl Nothing {
    pub fn new(world: &bevy_ecs::prelude::World, body: EntityId) -> Nothing {
        Nothing {
            tick: world
                .get_resource::<pocket_sim::SimClock>()
                .map_or(Tick(0), |c| c.tick),
            body,
        }
    }
}

impl ActorView for Nothing {
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
