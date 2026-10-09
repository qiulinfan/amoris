//! The time requests and their answers on the wire (shared/contract/time.md, Pacing, Decision
//! points, Requests): pacings, thinking clocks, decision filters and points, `until`, and the
//! `step`, `commit`, `wait` and `continue` requests, decoded strictly (charter 3.4).

use pocket_contract::Problem;
use pocket_sim::Tick;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::action::request::{EntityRef, IntentIdRef};

/// A chess clock with a Fischer increment, per agent seat, in wall-clock seconds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThinkingClock {
    /// Time available at the start.
    #[schemars(range(min = 0.0, max = 86400.0))]
    pub initial_s: f64,
    /// Added after each answered decision.
    #[schemars(range(min = 0.0, max = 86400.0))]
    pub increment_s: f64,
    /// The clock never holds more than this.
    #[schemars(range(min = 0.0, max = 86400.0))]
    pub max_s: f64,
}

/// How ticks are paced (time.md, Pacing): chosen per session, changeable at any boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "pacing", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlayPacing {
    /// Ticks run only when the clock holder asks (`step`).
    Stepped,
    /// Tick T runs when every listed seat has committed through T.
    Lockstep {
        seats: Vec<String>,
        #[serde(default)]
        delay_ticks: u32,
    },
    /// Ticks follow the wall clock at `speed` times real time, with pauses.
    RealTime {
        #[schemars(range(min = 0.001, max = 1000.0))]
        speed: f64,
        #[serde(default)]
        pause_on_decision: bool,
        #[serde(default)]
        clock: Option<ThinkingClock>,
    },
}

impl PlayPacing {
    pub fn name(&self) -> &'static str {
        match self {
            PlayPacing::Stepped => "stepped",
            PlayPacing::Lockstep { .. } => "lockstep",
            PlayPacing::RealTime { .. } => "real_time",
        }
    }

    /// The slice 1 loop pacing that times its ticks.
    pub fn loop_pacing(&self) -> super::Pacing {
        match self {
            PlayPacing::RealTime { speed, .. } => super::Pacing::RealTime { speed: *speed },
            _ => super::Pacing::Stepped,
        }
    }
}

/// Which perceived events and silences raise a seat's decision points; session state, set from
/// the game's default for the seat (time.md, Decision points).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionFilter {
    /// Perceived event kinds, or prefixes ending in ".".
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub idle_s: Option<f64>,
    #[serde(default)]
    pub every_s: Option<f64>,
}

impl DecisionFilter {
    /// Whether `kind` matches one of the filter's kinds or prefixes.
    pub fn watches(&self, kind: &str) -> bool {
        self.events.iter().any(|k| kind_matches(k, kind))
    }
}

/// `kind` against a pattern: equal, or a prefix ending in ".".
pub fn kind_matches(pattern: &str, kind: &str) -> bool {
    if pattern.ends_with('.') {
        kind.starts_with(pattern)
    } else {
        pattern == kind
    }
}

/// Why a seat should decide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DecisionReason {
    /// The first boundary of an episode.
    Start,
    /// A perceived event of a kind the filter watches.
    Event { seq: u64, kind: String },
    /// Nothing driving the seat for `idle_s`.
    Idle { idle_s: f64 },
    /// A heartbeat since the seat's last decision point.
    Interval { every_s: f64 },
    /// Its turn to decide.
    Turn { turn: u64 },
    /// A system asked for one: `name` is the reason the game declares (Slice 2: the contract's
    /// listing names it `reason`, which the variant's tag already is; time.md's open choices).
    Requested { name: String, event: Option<u64> },
}

/// A decision point: this seat should decide now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionPoint {
    /// "<tick>.<seat>", unique in a session.
    pub id: String,
    /// The tick after which it arose.
    pub tick: Tick,
    pub seat: String,
    /// Every reason that arose in that tick, in order.
    pub reasons: Vec<DecisionReason>,
    /// Thinking time left (real-time pacing with a clock).
    pub clock_s: Option<f64>,
}

impl DecisionPoint {
    /// The point in both projections (projection.md: `decision := "decision " DECISION_ID
    /// (" " REASON)* NL`; the JSON is the `DecisionPoint`).
    pub fn rendered(&self) -> crate::projection::Rendered {
        let raw = crate::projection::round::raw;
        let mut text = format!("decision {}", self.id);
        for r in &self.reasons {
            text.push(' ');
            text.push_str(&match r {
                DecisionReason::Start => "start".to_owned(),
                DecisionReason::Event { seq, kind } => format!("event:{seq}:{kind}"),
                DecisionReason::Idle { idle_s } => format!("idle:{}", raw(*idle_s)),
                DecisionReason::Interval { every_s } => format!("interval:{}", raw(*every_s)),
                DecisionReason::Turn { turn } => format!("turn:{turn}"),
                DecisionReason::Requested { name, .. } => format!("requested:{name}"),
            });
        }
        crate::projection::Rendered {
            json: serde_json::to_string(self).unwrap_or_else(|_| "null".to_owned()),
            text,
        }
    }
}

/// A comparison `until` makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CondOp {
    Equals,
    NotEquals,
    Above,
    Below,
    AtLeast,
    AtMost,
    Changes,
}

/// A perceived fact or instrument crossing a line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactCondition {
    /// None: one of the seat's instruments; Some: a fact of that entity.
    #[serde(default)]
    pub entity: Option<EntityRef>,
    pub name: String,
    pub op: CondOp,
    /// Required except with "changes".
    #[serde(default)]
    pub value: Option<Value>,
}

/// When a time request stops early (time.md, `until`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Until {
    /// "decision": the caller's seat has a decision point.
    Decision,
    /// {"intent": 3}: the intent has finished or started holding.
    Intent(IntentIdRef),
    /// {"event": "crate.taken"}: a perceived event of this kind, or of a prefix ending in ".".
    Event(String),
    /// {"fact": {...}}: a perceived fact or instrument crosses a line.
    Fact(FactCondition),
    /// {"any": [...]}: the first of several.
    Any(Vec<Until>),
}

/// `step` (stepped pacing, the clock holder): up to `ticks` ticks, stopping early on the episode's
/// end, a halt, `until` or the wall limit. Slice 1's `step {ticks}` is the same request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepRequest {
    /// 1 to 36000 (ten minutes at 60 Hz); 0 answers at once.
    #[serde(default = "one")]
    #[schemars(range(max = 36000))]
    pub ticks: u64,
    #[serde(default)]
    pub until: Option<Until>,
    /// Stop early, at a boundary, after this much wall time (at most 600000).
    #[serde(default)]
    #[schemars(range(min = 1, max = 600000))]
    pub max_wall_ms: Option<u32>,
    /// An observation at the stop, in the same answer (perception's `observe` request).
    #[serde(default)]
    pub observe: Option<crate::perception::ObserveRequest>,
    /// For the events delta.
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    /// Developer and checker roles only: evaluate `until` against the whole world.
    #[serde(default)]
    pub omniscient: Option<bool>,
    /// The seat whose decisions and perception `until` follows: a player's own; a developer names
    /// one, or follows the only seat (Slice 2, time.md's open choices).
    #[serde(default)]
    pub seat: Option<String>,
}

fn one() -> u64 {
    1
}

/// `continue`: answers the seat's pending decision.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContinueRequest {
    #[serde(default)]
    pub seat: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

/// `wait` (real time and lockstep): answers when `until` holds (by default, a decision point), at
/// the episode's end, or at `max_wall_ms`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitRequest {
    #[serde(default)]
    pub seat: Option<String>,
    #[serde(default)]
    pub until: Option<Until>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 600000))]
    pub max_wall_ms: Option<u32>,
    #[serde(default)]
    pub observe: Option<crate::perception::ObserveRequest>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

/// `commit` (lockstep): extends the seat's commitment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    #[serde(default)]
    pub seat: Option<String>,
    #[schemars(range(min = 1, max = 36000))]
    pub ticks: u64,
    #[serde(default)]
    pub until: Option<Until>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 600000))]
    pub max_wall_ms: Option<u32>,
    #[serde(default)]
    pub observe: Option<crate::perception::ObserveRequest>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

/// Why a time request answered (time.md, The answer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Ticks,
    Until,
    Decision,
    Done,
    Halted,
    WallLimit,
    Waiting,
    Answered,
}

/// The condition of an `until` that held.
#[derive(Clone, Debug, PartialEq)]
pub struct UntilMet {
    pub tick: Tick,
    pub condition: Until,
    pub value: Option<Value>,
    /// The perceived event that met it, as JSON.
    pub event: Option<Value>,
}

impl UntilMet {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"tick": self.tick.0, "condition": self.condition});
        if let Some(x) = &self.value {
            v["value"] = x.clone();
        }
        if let Some(e) = &self.event {
            v["event"] = e.clone();
        }
        v
    }
}

/// What a time request did, before the events delta and the observation are added (time.md,
/// `TimeResult`).
#[derive(Clone, Debug, PartialEq)]
pub struct TimeAnswer {
    pub from_tick: Tick,
    pub tick: Tick,
    pub ran: u64,
    pub stopped: StopReason,
    pub decision: Option<DecisionPoint>,
    pub passed_decisions: u32,
    pub until: Option<UntilMet>,
    pub waiting_for: Vec<String>,
    pub outcome: Option<Value>,
    pub halted: Option<Problem>,
    pub omniscient: bool,
    pub warnings: Vec<Problem>,
}

impl TimeAnswer {
    /// The answer as JSON, without `events`, `cursor` and `observation`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "from_tick": self.from_tick.0,
            "tick": self.tick.0,
            "ran": self.ran,
            "stopped": self.stopped,
            "passed_decisions": self.passed_decisions,
            "waiting_for": self.waiting_for,
            "omniscient": self.omniscient,
            "warnings": self.warnings,
        });
        if let Some(d) = &self.decision {
            v["decision"] = serde_json::to_value(d).unwrap_or(Value::Null);
        }
        if let Some(u) = &self.until {
            v["until"] = u.to_json();
        }
        if let Some(o) = &self.outcome {
            v["outcome"] = o.clone();
        }
        if let Some(h) = &self.halted {
            v["halted"] = serde_json::to_value(h).unwrap_or(Value::Null);
        }
        v
    }
}
