//! Decision points (shared/contract/time.md, Decision points): after every tick, once perception
//! has run, the reasons each attached seat should decide now: perceived events its filter watches,
//! idleness, the heartbeat, its turn, and the requests systems raised that it may receive. They are
//! session state: they decide where a run stops or pauses, never what the world computes.

use std::collections::BTreeMap;

use bevy_ecs::prelude::World;
use pocket_sim::{DecisionRequest, SimClock, Tick};

use super::play::{DecisionFilter, DecisionPoint, DecisionReason};
use super::turns::{TurnPhase, TurnState};
use crate::action::catalog::ActionCatalog;
use crate::action::state::{IntentTable, SeatRow, seats};

/// A decision a game's rules may request (time.md, `DecisionDef`).
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionDef {
    pub reason: String,
    pub doc: String,
    /// The rule decides from the seat's own instruments only, so the request reveals nothing.
    pub from_instruments: bool,
}

/// One seat's decision state in a session.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatDecisions {
    pub filter: DecisionFilter,
    /// The point waiting for an answer.
    pub pending: Option<DecisionPoint>,
    /// The tick of the seat's last decision point (the heartbeat counts from it).
    pub last_point: Tick,
    /// Since when nothing has driven the seat: no live intent, no action applied.
    pub idle_from: Tick,
    pub idle_raised: bool,
    /// The last turn whose deciding phase raised a point.
    pub turn_raised: u64,
    /// The observer sequence number of the last perceived event looked at.
    pub cursor: u64,
    /// Decision points that arose for the seat, and how many of them were answered.
    pub arisen: u64,
    pub answered: u64,
}

impl SeatDecisions {
    pub fn new(filter: DecisionFilter, tick: Tick) -> SeatDecisions {
        SeatDecisions {
            filter,
            pending: None,
            last_point: tick,
            idle_from: tick,
            idle_raised: false,
            turn_raised: 0,
            cursor: 0,
            arisen: 0,
            answered: 0,
        }
    }
}

/// The decision points of every seat after a tick: the per-seat state, the tick's decision
/// requests, and the game's declared decisions.
pub fn after_tick(
    world: &World,
    states: &mut BTreeMap<String, SeatDecisions>,
    requests: &[DecisionRequest],
    declared: &[DecisionDef],
    all_requests: bool,
) -> Vec<DecisionPoint> {
    let Some(catalog) = world.get_resource::<ActionCatalog>() else {
        return Vec::new();
    };
    let Ok(rows) = seats(world) else {
        return Vec::new();
    };
    let clock = *world.resource::<SimClock>();
    let mut out = Vec::new();
    for row in &rows {
        let Some(st) = states.get_mut(&row.id) else {
            continue;
        };
        let reasons = reasons(
            world,
            catalog,
            row,
            st,
            requests,
            declared,
            clock,
            all_requests,
        );
        if reasons.is_empty() {
            continue;
        }
        let point = DecisionPoint {
            id: format!("{}.{}", clock.tick.0, row.id),
            tick: clock.tick,
            seat: row.id.clone(),
            reasons,
            clock_s: None,
        };
        st.last_point = clock.tick;
        st.arisen += 1;
        st.pending = Some(point.clone());
        out.push(point);
    }
    out
}

/// The reasons `row`'s seat should decide after this tick. A requested decision reaches a player
/// only when it perceived the request's event or the request is declared `from_instruments`;
/// `all_requests` (a developer's session) keeps every declared one.
#[allow(clippy::too_many_arguments)]
fn reasons(
    world: &World,
    catalog: &ActionCatalog,
    row: &SeatRow,
    st: &mut SeatDecisions,
    requests: &[DecisionRequest],
    declared: &[DecisionDef],
    clock: SimClock,
    all_requests: bool,
) -> Vec<DecisionReason> {
    let mut out = Vec::new();
    let view = (catalog.view)(world, row);
    let events = view.events_since(st.cursor);
    if let Some(last) = events.last() {
        st.cursor = last.seq;
    }
    for e in &events {
        if st.filter.watches(&e.kind) {
            out.push(DecisionReason::Event {
                seq: e.seq,
                kind: e.kind.clone(),
            });
        }
    }
    let secs = |from: Tick| Tick(clock.tick.0.saturating_sub(from.0)).to_f64() * clock.dt();
    let driven = world
        .get_resource::<IntentTable>()
        .is_some_and(|t| t.live_of(&row.id).next().is_some());
    if driven {
        st.idle_from = clock.tick;
    }
    if let Some(idle_s) = st.filter.idle_s
        && !st.idle_raised
        && secs(st.idle_from) >= idle_s
    {
        st.idle_raised = true;
        out.push(DecisionReason::Idle { idle_s });
    }
    if let Some(every_s) = st.filter.every_s
        && secs(st.last_point) >= every_s
    {
        out.push(DecisionReason::Interval { every_s });
    }
    if let Some(t) = world.get_resource::<TurnState>()
        && t.phase == TurnPhase::Deciding
        && t.to_move.contains(&row.id)
        && t.turn > st.turn_raised
    {
        st.turn_raised = t.turn;
        out.push(DecisionReason::Turn { turn: t.turn });
    }
    for r in requests.iter().filter(|r| r.observer == row.body) {
        let Some(def) = declared.iter().find(|d| d.reason == r.reason) else {
            continue;
        };
        let perceived = r.event.and_then(|seq| {
            events
                .iter()
                .find(|e| e.world_seq == Some(seq.0))
                .map(|e| e.seq)
        });
        if def.from_instruments || perceived.is_some() || all_requests {
            out.push(DecisionReason::Requested {
                name: r.reason.clone(),
                event: perceived,
            });
        }
    }
    out
}

/// The seat acted: it is driven from now, and may be idle again later.
pub fn acted(st: &mut SeatDecisions, tick: Tick) {
    st.idle_from = tick;
    st.idle_raised = false;
}

/// The `start` decision point of each seat at the first boundary of an episode.
pub fn start(world: &World, states: &mut BTreeMap<String, SeatDecisions>) -> Vec<DecisionPoint> {
    let tick = world.resource::<SimClock>().tick;
    let mut out = Vec::new();
    for (seat, st) in states.iter_mut() {
        let p = DecisionPoint {
            id: format!("{}.{seat}", tick.0),
            tick,
            seat: seat.clone(),
            reasons: vec![DecisionReason::Start],
            clock_s: None,
        };
        st.pending = Some(p.clone());
        st.arisen += 1;
        st.last_point = tick;
        out.push(p);
    }
    out
}
