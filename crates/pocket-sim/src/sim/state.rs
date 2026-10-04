//! The tick's own bookkeeping resources: whether a tick or an invocation runs, what the tick
//! reports, and the poison mark (docs/spec/simulation.md 4.1, 4.5, 4.6 and 6). All Derived: none
//! of it survives a boundary or enters a snapshot.

use bevy_ecs::prelude::Resource;
use pocket_contract::{Problem, detail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::entity::EntityId;
use crate::event::EventSeq;
use crate::schedule::{SystemKey, TickPhase};
use crate::time::Tick;

/// Whether a tick runs (and which), and whether an invocation is open.
#[derive(Resource, Debug, Default)]
pub struct TickState {
    pub(crate) running: Option<Tick>,
    pub(crate) invocation: Option<SystemKey>,
}

impl TickState {
    /// The tick running now, `None` at a boundary.
    pub fn running(&self) -> Option<Tick> {
        self.running
    }

    /// The system whose invocation is open, if any.
    pub fn invocation(&self) -> Option<&SystemKey> {
        self.invocation.as_ref()
    }
}

/// Raised by a system during a tick: an observer needs a decision (charter 3.5). `reason` is a
/// name the game declares (shared/contract/time.md, Decision points), never free text.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DecisionRequest {
    pub observer: EntityId,
    pub reason: String,
    pub event: Option<EventSeq>,
}

/// What the running tick reports beside its events: failed invocations and decision requests, in
/// the order they happened. Systems write it through `ResMut<TickOutput>`; the step takes it.
#[derive(Resource, Debug, Default)]
pub struct TickOutput {
    pub(crate) errors: Vec<Problem>,
    pub(crate) decisions: Vec<DecisionRequest>,
}

impl TickOutput {
    /// Records a failed invocation (`sim.system_failed`, from [`system_failed`]).
    pub fn report(&mut self, problem: Problem) {
        self.errors.push(problem);
    }

    /// Raises a decision request.
    pub fn decide(&mut self, request: DecisionRequest) {
        self.decisions.push(request);
    }

    pub fn errors(&self) -> &[Problem] {
        &self.errors
    }

    pub fn decisions(&self) -> &[DecisionRequest] {
        &self.decisions
    }
}

/// Present when the world's last tick stopped halfway (a panic or a fault): `step` refuses with
/// `sim.world_poisoned` until a snapshot is restored into the world.
#[derive(Resource, Clone, Debug)]
pub struct Poisoned {
    pub tick: Tick,
    pub problem: Problem,
}

/// `sim.system_failed`: an invocation failed and its effects were discarded; `cause` is its error.
pub fn system_failed(
    tick: Tick,
    phase: TickPhase,
    system: &SystemKey,
    entity: Option<EntityId>,
    cause: &Problem,
) -> Problem {
    let mut d = detail([
        ("tick", json!(tick.0)),
        ("phase", json!(phase.name())),
        ("system", json!(system.as_str())),
        ("cause", serde_json::to_value(cause).unwrap_or_default()),
    ]);
    if let Some(e) = entity {
        d.insert("entity".into(), json!(e.get()));
    }
    Problem::new(
        "sim.system_failed",
        format!(
            "System {system} failed at tick {} and its effects were discarded: {}",
            tick.0, cause.message
        ),
        d,
    )
}

/// `sim.internal`: a system panicked or broke an invariant.
pub fn internal(tick: Tick, phase: TickPhase, system: &str, message: &str) -> Problem {
    Problem::new(
        "sim.internal",
        format!(
            "System {system} failed inside tick {}: {message}. This is an engine bug; the world is \
             poisoned until a snapshot is restored.",
            tick.0
        ),
        detail([
            ("tick", json!(tick.0)),
            ("phase", json!(phase.name())),
            ("system", json!(system)),
            ("message", json!(message)),
        ]),
    )
}

/// `sim.world_poisoned`: `step` on a world whose last tick stopped halfway.
pub fn world_poisoned(tick: Tick) -> Problem {
    Problem::new(
        "sim.world_poisoned",
        format!(
            "Tick {} stopped halfway, so the world cannot step; restore a snapshot into it.",
            tick.0
        ),
        detail([("tick", json!(tick.0))]),
    )
}
