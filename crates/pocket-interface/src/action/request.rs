//! The requests of the action layer on the wire (shared/contract/actions.md, The act request,
//! Intent instances, Affordances): `act`, `intents` and `affordances`, decoded strictly (unknown
//! fields refused with suggestions, charter 3.4), and the answers' forms.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// An entity as a request names it (README, Positions and entity references): an id, a name,
/// `Name#id` or `#id`, or the JSON projection's `{"id": .., "name": ..}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EntityRef {
    Id(u64),
    Text(String),
    Named(EntityNameRef),
}

/// `{"id": .., "name": ..}`, the name optional.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityNameRef {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
}

/// A point in the contract frame; `y` may be left out where the intent works on the sea surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PointRef {
    pub x: f64,
    #[serde(default)]
    pub y: Option<f64>,
    pub z: f64,
}

/// An intent's target: an entity or a point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Target {
    Point(PointRef),
    Entity(EntityRef),
}

/// A control's value on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum WireValue {
    Bool(bool),
    Number(f64),
    Name(String),
}

/// An intent id: the number `3` or the text projection's `"#3"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IntentIdRef {
    Number(u64),
    Text(String),
}

impl IntentIdRef {
    /// The id, or `None` for text that is not `#n`.
    pub fn id(&self) -> Option<u64> {
        match self {
            IntentIdRef::Number(n) => Some(*n),
            IntentIdRef::Text(t) => t.strip_prefix('#').and_then(|n| n.parse().ok()),
        }
    }
}

/// One action of an `act` call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Latch control values.
    Set {
        controls: BTreeMap<String, WireValue>,
    },
    /// One pulse of a pulse control, with a target if it takes one.
    Pulse {
        control: String,
        #[serde(default)]
        target: Option<EntityRef>,
    },
    /// Start an intent.
    Start {
        intent: String,
        #[serde(default)]
        params: Map<String, Value>,
        #[serde(default)]
        target: Option<Target>,
        /// The caller's own label, at most 64 bytes, echoed.
        #[serde(default)]
        tag: Option<String>,
    },
    /// Use an affordance of an entity: starts its intent or sends its pulse.
    Use {
        entity: EntityRef,
        verb: String,
        #[serde(default)]
        params: Map<String, Value>,
        #[serde(default)]
        tag: Option<String>,
    },
    /// Cancel an intent; cancelling a finished one answers its final status.
    Cancel { intent_id: IntentIdRef },
    /// End this seat's turn (turn-based games).
    EndTurn,
}

impl Action {
    /// The `do` of the action.
    pub fn verb(&self) -> &'static str {
        match self {
            Action::Set { .. } => "set",
            Action::Pulse { .. } => "pulse",
            Action::Start { .. } => "start",
            Action::Use { .. } => "use",
            Action::Cancel { .. } => "cancel",
            Action::EndTurn => "end_turn",
        }
    }
}

/// `act`: one or more actions for one seat, validated whole and applied whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActRequest {
    /// Player role: omitted or its own seat. Developer role: required.
    #[serde(default)]
    pub seat: Option<String>,
    /// 1 to 32 actions.
    #[schemars(length(min = 1, max = 32))]
    pub actions: Vec<Action>,
    /// Budget for the events delta in the answer.
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    /// After applying, answer the seat's pending decision as `continue` does.
    #[serde(default)]
    pub resume: Option<bool>,
}

/// `intents`: a seat's intents, newest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntentsRequest {
    #[serde(default)]
    pub seat: Option<String>,
    #[serde(default)]
    pub ids: Option<Vec<IntentIdRef>>,
    #[serde(default)]
    pub active_only: Option<bool>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

/// `affordances`: what the seat can do with the entities it perceives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AffordancesRequest {
    #[serde(default)]
    pub seat: Option<String>,
    #[serde(default)]
    pub entity: Option<EntityRef>,
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    #[serde(default)]
    pub within_m: Option<f64>,
    #[serde(default)]
    pub available_only: Option<bool>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

/// Who calls (README, Seats and callers): a player plays one seat; a developer may act for any;
/// a checker only reads. A replay acts as a developer naming the recorded seat (actions.md, What
/// a replay records: session-level checks are skipped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// A player and the id of the seat it plays.
    Player {
        seat: String,
    },
    Developer,
    Checker,
}

impl Caller {
    pub fn role(&self) -> &'static str {
        match self {
            Caller::Player { .. } => "player",
            Caller::Developer => "developer",
            Caller::Checker => "checker",
        }
    }
}
