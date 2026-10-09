//! The game's turn structure and its episode (shared/contract/time.md, Turn structure, Episodes):
//! both are world state, moved only by actions (`end_turn`) and by `interface.turns` in the
//! `Finish` phase, which ends a turn's resolving phase when its rule holds and latches the end of
//! the episode the tick the game's `done` predicate holds.

use std::collections::BTreeSet;

use bevy_ecs::prelude::{Resource, World};
use pocket_contract::{Problem, codes};
use pocket_sim::persisted::{Persisted, RegisterPersisted};
use pocket_sim::{PlainData, RunCondition, Sim, SimClock, Tick, TickPhase};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_reflection::{Samples, Tracer};

use crate::action::request::{ActRequest, Action};

/// Who decides in a turn (time.md, Turn structure).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "order", rename_all = "snake_case")]
pub enum TurnOrder {
    /// One seat decides at a time, in this order.
    RoundRobin { seats: Vec<String> },
    /// All these seats decide; the turn resolves when all have ended it.
    Simultaneous { seats: Vec<String> },
}

/// How a turn resolves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "resolve", rename_all = "snake_case")]
pub enum Resolve {
    /// Exactly this many ticks.
    Ticks { ticks: u32 },
    /// Until the game's `quiet` predicate holds, at most `max_ticks`.
    UntilQuiet { max_ticks: u32 },
}

/// The game's turn structure, declared and never changed at run time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "structure", rename_all = "snake_case")]
pub enum TurnStructure {
    /// Time runs on (the sailing game).
    Continuous,
    Turns {
        order: TurnOrder,
        resolve: Resolve,
    },
}

/// A turn's phase (persisted externally tagged).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TurnPhase {
    Deciding,
    Resolving { started: Tick, end_by: Tick },
}

/// The turn state, world state in a turn-based game.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnState {
    pub turn: u64,
    pub phase: TurnPhase,
    /// Seats that have not ended this turn.
    pub to_move: BTreeSet<String>,
}

/// How an episode ended (time.md, Episodes), world state latched by `interface.turns`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EpisodeEnd {
    pub tick: Tick,
    pub result: Vec<(String, PlainData)>,
}

/// The episode's state: `None` while it runs.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Episode {
    pub ended: Option<EpisodeEnd>,
}

/// The game's `done` predicate: the episode's result readings in the tick it holds, a pure
/// function of the world at the end of that tick (its events are in the outbox until `sim.finish`).
pub type DoneFn = fn(&World) -> Option<Vec<(String, PlainData)>>;

/// A turn's `quiet` predicate.
pub type QuietFn = fn(&World) -> bool;

/// The game's time rules: its structure and predicates. Class Ignored, installed with the game.
#[derive(Resource, Clone)]
pub struct TimeRules {
    pub structure: TurnStructure,
    pub done: Option<DoneFn>,
    pub quiet: Option<QuietFn>,
    /// The episode's result readings' names, for `describe`.
    pub result: Vec<String>,
    pub doc: String,
}

impl Default for TimeRules {
    fn default() -> Self {
        TimeRules {
            structure: TurnStructure::Continuous,
            done: None,
            quiet: None,
            result: Vec::new(),
            doc: String::new(),
        }
    }
}

impl Persisted for TurnState {
    const NAME: &'static str = "TurnState";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<TurnPhase>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Episode {
    const NAME: &'static str = "Episode";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<PlainData>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

/// Declares the time state's persistence classes.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.resource::<Episode>()
        .resource::<TurnState>()
        .ignore::<TimeRules>("the game's turn structure and predicates, installed with the game");
}

fn first_to_move(order: &TurnOrder) -> BTreeSet<String> {
    match order {
        TurnOrder::RoundRobin { seats } => seats.first().cloned().into_iter().collect(),
        TurnOrder::Simultaneous { seats } => seats.iter().cloned().collect(),
    }
}

/// Installs the game's time rules: the episode, the first turn's deciding phase in a turn-based
/// game, and `interface.turns` (simulation.md 4.4, row 11).
pub fn plugin(sim: &mut Sim, rules: TimeRules) -> Result<(), Problem> {
    let w = sim.world_mut();
    if !w.contains_resource::<Episode>() {
        w.insert_resource(Episode::default());
    }
    if let TurnStructure::Turns { order, .. } = &rules.structure
        && !w.contains_resource::<TurnState>()
    {
        w.insert_resource(TurnState {
            turn: 1,
            phase: TurnPhase::Deciding,
            to_move: first_to_move(order),
        });
    }
    w.insert_resource(rules);
    sim.add_exclusive(
        "interface.turns",
        TickPhase::Finish,
        RunCondition::Always,
        |world, _ctx| turns_system(world),
    )
}

/// `interface.turns`: ends a resolving phase whose rule holds and begins the next turn, then
/// latches the episode's end when `done` holds.
fn turns_system(world: &mut World) {
    let Some(rules) = world.get_resource::<TimeRules>().cloned() else {
        return;
    };
    let tick = world.resource::<SimClock>().tick;
    if let TurnStructure::Turns { order, resolve } = &rules.structure
        && let Some(state) = world.get_resource::<TurnState>().cloned()
        && let TurnPhase::Resolving { end_by, .. } = state.phase
    {
        let quiet =
            matches!(resolve, Resolve::UntilQuiet { .. }) && rules.quiet.is_some_and(|q| q(world));
        if tick >= end_by || quiet {
            let to_move = match order {
                TurnOrder::RoundRobin { seats } => {
                    // The seat after the one that just moved.
                    let n = seats.len().max(1);
                    let idx = usize::try_from(state.turn).unwrap_or(0) % n;
                    seats.get(idx).cloned().into_iter().collect()
                }
                TurnOrder::Simultaneous { seats } => seats.iter().cloned().collect(),
            };
            world.insert_resource(TurnState {
                turn: state.turn + 1,
                phase: TurnPhase::Deciding,
                to_move,
            });
        }
    }
    if world.resource::<Episode>().ended.is_none()
        && let Some(done) = rules.done
        && let Some(result) = done(world)
    {
        world.resource_mut::<Episode>().ended = Some(EpisodeEnd { tick, result });
    }
}

/// The episode's outcome as `time.md`'s `EpisodeOutcome`, when it ended.
pub fn outcome(world: &World) -> Option<Value> {
    let end = world.get_resource::<Episode>()?.ended.as_ref()?;
    Some(outcome_json(end, false))
}

/// `EpisodeOutcome` of an end: terminated by `done`, or truncated by a session's tick limit.
pub fn outcome_json(end: &EpisodeEnd, truncated: bool) -> Value {
    let result: Vec<Value> = end
        .result
        .iter()
        .map(|(n, v)| json!({"name": n, "value": v.to_json()}))
        .collect();
    json!({"terminated": !truncated, "truncated": truncated, "tick": end.tick.0, "result": result})
}

/// `time.episode_over` once the episode ended.
pub fn check_episode(world: &World) -> Result<(), Problem> {
    match outcome(world) {
        Some(o) => Err(codes::time_episode_over(o)),
        None => Ok(()),
    }
}

/// Whether the world is in a turn's deciding phase, when no tick runs.
pub fn deciding(world: &World) -> bool {
    world
        .get_resource::<TurnState>()
        .is_some_and(|s| s.phase == TurnPhase::Deciding)
}

/// The turn rules on an act (time.md, Turn structure): `end_turn` in a continuous game is
/// `time.wrong_mode`; in the deciding phase, a seat not in `to_move` that acts is
/// `time.not_your_turn`.
pub fn check_act(world: &World, seat: &str, req: &ActRequest) -> Result<(), Problem> {
    let ends = req.actions.iter().any(|a| matches!(a, Action::EndTurn));
    match world.get_resource::<TurnState>() {
        None if ends => Err(codes::time_wrong_mode(
            "end_turn",
            "any",
            "continuous",
            &["act without end_turn"],
        )),
        None => Ok(()),
        Some(s) => {
            let mine = s.to_move.contains(seat);
            let deciding = s.phase == TurnPhase::Deciding;
            if (ends || deciding) && !(deciding && mine) {
                let to_move: Vec<&str> = s.to_move.iter().map(String::as_str).collect();
                Err(codes::time_not_your_turn(s.turn, &to_move.join(", ")))
            } else {
                Ok(())
            }
        }
    }
}

/// Ends `seat`'s turn (validated by [`check_act`]): when no seat is left to move, the turn begins
/// resolving from the next tick. The turn number.
pub fn end_turn(world: &mut World, seat: &str) -> Result<u64, Problem> {
    let next = Tick(world.resource::<SimClock>().tick.0 + 1);
    let resolve = match world
        .get_resource::<TimeRules>()
        .map(|r| r.structure.clone())
    {
        Some(TurnStructure::Turns { resolve, .. }) => resolve,
        _ => {
            return Err(crate::action::state::internal(
                "end_turn",
                "a turn ended in a continuous game",
            ));
        }
    };
    let mut s = world.resource_mut::<TurnState>();
    s.to_move.remove(seat);
    if s.to_move.is_empty() {
        let ticks = match resolve {
            Resolve::Ticks { ticks } => ticks,
            Resolve::UntilQuiet { max_ticks } => max_ticks,
        };
        s.phase = TurnPhase::Resolving {
            started: next,
            end_by: Tick(next.0 + u64::from(ticks.max(1)) - 1),
        };
    }
    Ok(s.turn)
}
