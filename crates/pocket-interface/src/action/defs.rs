//! What a game declares about acting (shared/contract/actions.md): its controls, its intents with
//! their parameters, targets, channels and failures, the affordances its kinds offer, and its own
//! codes (errors.md, Game codes). Declarations are data the engine validates at load and shows in
//! `describe`; the code behind them (a control's binding, an intent's executor) sits beside them in
//! the [`super::catalog::ActionCatalog`].

use bevy_ecs::prelude::World;
use pocket_contract::{Problem, Shape, Use};
use pocket_sim::{EntityId, PlainData};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::state::ControlValue;

/// How a control takes values (actions.md, Controls).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlKind {
    /// Latched number in [min, max].
    Axis { min: f64, max: f64, default: f64 },
    /// Latched boolean.
    Toggle { default: bool },
    /// Latched choice among names.
    Choice {
        values: Vec<String>,
        default: String,
    },
    /// Acts for one tick; may name a target entity of the given kinds.
    Pulse { target_kinds: Option<Vec<String>> },
}

impl ControlKind {
    pub fn is_pulse(&self) -> bool {
        matches!(self, ControlKind::Pulse { .. })
    }

    /// The latched default, `None` for a pulse.
    pub fn default_value(&self) -> Option<ControlValue> {
        match self {
            ControlKind::Axis { default, .. } => Some(ControlValue::Number(*default)),
            ControlKind::Toggle { default } => Some(ControlValue::Bool(*default)),
            ControlKind::Choice { default, .. } => Some(ControlValue::Name(default.clone())),
            ControlKind::Pulse { .. } => None,
        }
    }
}

/// Where a latched value or a delivered pulse goes in the world: the field a system reads.
#[derive(Clone, Copy)]
pub enum ControlBinding {
    /// Kept in the body's `Controls.values` only (a game rule reads it there).
    Unbound,
    /// An engine component's field, written and read by these functions (the sailing controls on
    /// `Boat`). A pulse's write gets its target as `ControlValue::Number(id)`, or `None` for none.
    Engine {
        write: fn(&mut World, EntityId, Option<&ControlValue>) -> Result<(), Problem>,
        read: fn(&World, EntityId) -> Option<ControlValue>,
        /// Whether the body can take the control (a sailing control needs a `Boat`): a control
        /// its body cannot take is not the seat's (`action.unknown_control`).
        writable: fn(&World, EntityId) -> bool,
    },
    /// A field of a component by name (a project component), written through the world's
    /// [`super::catalog::FieldWriter`]: a pulse's target id at the start of the tick it acts in.
    Field {
        component: &'static str,
        field: &'static str,
        /// Whether the body can take the control, as for [`ControlBinding::Engine`].
        writable: fn(&World, EntityId) -> bool,
    },
}

impl ControlBinding {
    /// Whether `body` can take a control bound here: an unbound control any body can.
    pub fn writable(&self, world: &World, body: EntityId) -> bool {
        match self {
            ControlBinding::Unbound => true,
            ControlBinding::Engine { writable, .. } | ControlBinding::Field { writable, .. } => {
                writable(world, body)
            }
        }
    }
}

/// One input of a seat's body (actions.md, Controls).
#[derive(Clone)]
pub struct ControlDef {
    pub name: String,
    /// Says the sign: "positive turns the bow to starboard".
    pub doc: String,
    pub kind: ControlKind,
    /// Setting the control supersedes the intent holding its channel.
    pub channel: String,
    pub binding: ControlBinding,
}

impl ControlDef {
    /// The declaration as `describe` shows it.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"name": self.name, "doc": self.doc, "channel": self.channel});
        if let (Some(m), Ok(Value::Object(k))) =
            (v.as_object_mut(), serde_json::to_value(&self.kind))
        {
            m.extend(k);
        }
        v
    }

    /// `true` when `v` fits the control (its type and range); the reason otherwise.
    pub fn fits(&self, v: &ControlValue) -> Result<(), String> {
        match (&self.kind, v) {
            (ControlKind::Axis { min, max, .. }, ControlValue::Number(x)) => {
                if x.is_finite() && *x >= *min && *x <= *max {
                    Ok(())
                } else {
                    Err(format!("from {min} to {max}"))
                }
            }
            (ControlKind::Toggle { .. }, ControlValue::Bool(_)) => Ok(()),
            (ControlKind::Choice { values, .. }, ControlValue::Name(n)) => {
                if values.contains(n) {
                    Ok(())
                } else {
                    Err(format!("one of {}", values.join(", ")))
                }
            }
            (ControlKind::Axis { .. }, _) => Err("a number".into()),
            (ControlKind::Toggle { .. }, _) => Err("true or false".into()),
            (ControlKind::Choice { .. }, _) => Err("a name".into()),
            (ControlKind::Pulse { .. }, _) => Err("a pulse, sent with \"do\": \"pulse\"".into()),
        }
    }
}

/// What an intent targets (actions.md, Intents).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "takes", rename_all = "snake_case")]
pub enum TargetSpec {
    None,
    Entity { kinds: Vec<String> },
    Point,
    EntityOrPoint { kinds: Vec<String> },
}

impl TargetSpec {
    /// What it takes, said as an agent would (`action.target_required`'s `takes`).
    pub fn takes(&self) -> String {
        match self {
            TargetSpec::None => "no target".into(),
            TargetSpec::Entity { kinds } => format!("an entity of kind {}", kinds.join(" or ")),
            TargetSpec::Point => "a point {\"x\": .., \"z\": ..}".into(),
            TargetSpec::EntityOrPoint { kinds } => format!(
                "an entity of kind {} or a point {{\"x\": .., \"z\": ..}}",
                kinds.join(" or ")
            ),
        }
    }

    pub fn kinds(&self) -> &[String] {
        match self {
            TargetSpec::Entity { kinds } | TargetSpec::EntityOrPoint { kinds } => kinds,
            _ => &[],
        }
    }

    pub fn takes_entity(&self) -> bool {
        matches!(
            self,
            TargetSpec::Entity { .. } | TargetSpec::EntityOrPoint { .. }
        )
    }

    pub fn takes_point(&self) -> bool {
        matches!(self, TargetSpec::Point | TargetSpec::EntityOrPoint { .. })
    }
}

/// A progress reading an intent reports, declared like a fact (name with its unit suffix): stored
/// rounded at its precision (projection.md, Rounding), a bearing in [0, 360).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProgressDef {
    pub name: String,
    pub doc: String,
    pub unit: crate::perception::Unit,
    /// Decimal places in projections; 0 makes the value an integer.
    pub precision: u8,
}

impl ProgressDef {
    /// `v` as an instance keeps it: a number rounded at the precision through its formatted
    /// decimal, so both projections and every replay store the same bits.
    pub fn store(&self, v: PlainData) -> PlainData {
        match v {
            PlainData::Number(x) if x.is_finite() => PlainData::Number(match self.unit {
                crate::perception::Unit::Bearing => {
                    crate::projection::round::stored_bearing(x, self.precision)
                }
                _ => crate::projection::round::stored(x, self.precision),
            }),
            other => other,
        }
    }

    /// The projections' format of the reading.
    pub fn format(&self) -> crate::projection::round::Format {
        crate::projection::round::Format::of(&self.unit, self.precision)
    }
}

/// A named goal the engine carries out tick by tick (actions.md, Intents).
#[derive(Clone)]
pub struct IntentDef {
    /// A verb phrase: "come_to_heading".
    pub name: String,
    /// What it does, when it succeeds, how it can fail.
    pub doc: String,
    /// The parameters' JSON Schema, from a Rust type through schemars.
    pub params: Shape,
    /// Each parameter's default, filled into the canonical parameters.
    pub defaults: serde_json::Map<String, Value>,
    pub target: TargetSpec,
    /// The channels it occupies while active or holding (`sail_to` also takes `sail` with `trim:
    /// "auto"`: see [`super::executor::IntentExecutor::channels`]).
    pub channels: Vec<String>,
    /// Whether `keep: true` is accepted.
    pub can_hold: bool,
    pub default_timeout_s: f64,
    /// The failure codes it can end with.
    pub failures: Vec<String>,
    pub progress: Vec<ProgressDef>,
}

impl IntentDef {
    /// The declaration as `describe` shows it.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "doc": self.doc,
            "params": self.params.schema(),
            "target": self.target,
            "channels": self.channels,
            "can_hold": self.can_hold,
            "default_timeout_s": pocket_contract::codes::num(self.default_timeout_s),
            "failures": self.failures,
            "progress": self.progress,
        })
    }
}

/// What an affordance does (actions.md, Affordances).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Starts the intent with the entity as its target; `params` are fixed values merged under the
    /// caller's.
    Intent {
        intent: String,
        params: serde_json::Map<String, Value>,
    },
    /// Sends one pulse of the control with the entity as its target.
    Pulse { control: String },
}

/// Which side a fact requirement reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Target,
    Actor,
}

/// A comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Equals,
    NotEquals,
    Above,
    Below,
    AtLeast,
    AtMost,
}

impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Op::Equals => "equals",
            Op::NotEquals => "not_equals",
            Op::Above => "above",
            Op::Below => "below",
            Op::AtLeast => "at_least",
            Op::AtMost => "at_most",
        }
    }

    /// Whether `actual op value` holds: numbers compare as numbers; other values only for
    /// (in)equality.
    pub fn holds(self, actual: &Value, value: &Value) -> bool {
        match (actual.as_f64(), value.as_f64()) {
            (Some(a), Some(b)) => match self {
                Op::Equals => a == b,
                Op::NotEquals => a != b,
                Op::Above => a > b,
                Op::Below => a < b,
                Op::AtLeast => a >= b,
                Op::AtMost => a <= b,
            },
            _ => match self {
                Op::Equals => actual == value,
                Op::NotEquals => actual != value,
                _ => false,
            },
        }
    }
}

/// What an affordance requires; all must hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "requires", rename_all = "snake_case")]
pub enum Requirement {
    /// The entity is seen now (not only remembered or charted).
    Seen,
    /// The entity is within this range of the actor's body.
    Within { max_m: f64 },
    /// The entity's bearing is within this angle of the actor's heading.
    Facing { max_off_deg: f64 },
    /// A fact of the entity or an instrument of the actor compares as given.
    Fact {
        of: Side,
        fact: String,
        op: Op,
        value: Value,
    },
    /// Only these seats may use it.
    Seats { seats: Vec<String> },
}

/// An interaction an entity's kind offers (actions.md, Affordances).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AffordanceDef {
    pub verb: String,
    pub doc: String,
    pub effect: Effect,
    pub requires: Vec<Requirement>,
}

/// A game's own code (errors.md, Game codes): its use, the detail fields it carries and the
/// template its message is rendered from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CodeDef {
    pub code: String,
    #[serde(rename = "use")]
    pub uses: Vec<Use>,
    pub when: String,
    pub detail: Vec<String>,
    pub template: String,
}

impl CodeDef {
    /// The problem with this code over `detail`, its message rendered from the template.
    pub fn problem(&self, detail: pocket_contract::Detail) -> Problem {
        Problem::from_template(&self.code, &self.template, detail)
    }
}
