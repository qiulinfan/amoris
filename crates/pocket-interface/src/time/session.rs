//! The time requests of a session that drives its world tick by tick (shared/contract/time.md,
//! Requests): `step` (stepped pacing, the clock holder), `commit` (lockstep), `continue`, `pause`,
//! `resume` and `time.status`, each answered with a `TimeResult`. The world runs through a
//! [`Ticker`]; after every tick the [`Controller`] computes the decision points, and the run stops,
//! in this order, at the episode's end, a halt, `until`, or the wall limit. Nothing here writes
//! the world: pacing decides when ticks run, never what they compute.

use bevy_ecs::prelude::World;
use pocket_contract::{CheckOptions, Pointer, Problem, codes, decode};
use pocket_sim::{Sim, StepReport, Tick};
use serde_json::{Value, json};

use super::control::Controller;
use super::play::{
    CommitRequest, ContinueRequest, DecisionPoint, PlayPacing, StepRequest, StopReason, TimeAnswer,
    Until,
};
use super::{turns, until};
use crate::action::request::Caller;
use crate::action::state::seats;
use crate::action::{ActDone, ActionCatalog, IntentTable};

/// What a session steps: its world, one tick of it, and an act applied at its boundary through
/// the game's write path (which the recorder sees).
pub trait Ticker {
    fn world(&self) -> &World;
    /// Runs one tick (an error poisons or refuses, as the caller's game decides).
    fn tick(&mut self) -> Result<StepReport, Problem>;
    /// Validates and applies an `act` at the current boundary (actions.md, The act request).
    fn act(&mut self, caller: &Caller, raw: &Value) -> Result<ActDone, Problem>;
}

impl Ticker for Sim {
    fn world(&self) -> &World {
        Sim::world(self)
    }

    fn tick(&mut self) -> Result<StepReport, Problem> {
        self.step(&mut pocket_sim::NoHooks)
    }

    fn act(&mut self, caller: &Caller, raw: &Value) -> Result<ActDone, Problem> {
        crate::action::act(self.world_mut(), caller, raw)
    }
}

/// A lockstep act waiting for the boundary before its seat's next uncommitted tick (actions.md,
/// When an action takes effect): validated when submitted, validated again when applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub at: Tick,
    pub seat: String,
    pub request: Value,
}

/// The wall limit when a request names none (time.md, `StepRequest.max_wall_ms`), and its bound.
pub const WALL_MS: u32 = 30_000;
pub const MAX_WALL_MS: u32 = 600_000;

fn perception_caller(caller: &Caller) -> crate::perception::Caller<'_> {
    use crate::perception::Role;
    match caller {
        Caller::Player { seat } => crate::perception::Caller {
            role: Role::Player,
            seat: Some(seat),
        },
        Caller::Developer => crate::perception::Caller {
            role: Role::Developer,
            seat: None,
        },
        Caller::Checker => crate::perception::Caller {
            role: Role::Checker,
            seat: None,
        },
    }
}

/// The seat a time request follows: a player's own; the one a developer names, or the only seat;
/// none for a developer in a game of several seats who names none.
pub fn seat_of(
    world: &World,
    caller: &Caller,
    named: Option<&str>,
    request: &str,
) -> Result<Option<String>, Problem> {
    match (caller, named) {
        (Caller::Player { .. }, _) | (_, Some(_)) => {
            crate::action::validate::seat_for(world, caller, named, request).map(|r| Some(r.id))
        }
        (_, None) => {
            let all = seats(world)?;
            Ok((all.len() == 1).then(|| all[0].id.clone()))
        }
    }
}

/// Checks `until` for the seat (refused at the request: time.md, `until`), and the values its
/// `changes` conditions start from.
fn check_until(
    world: &World,
    seat: Option<&str>,
    u: Option<&Until>,
    omniscient: bool,
) -> Result<Vec<Option<Value>>, Problem> {
    let Some(u) = u else { return Ok(Vec::new()) };
    let Some(seat) = seat else {
        return Err(codes::missing_field(
            &Pointer::root().key("seat"),
            "a time request with until",
            "the seat whose perception until reads",
        ));
    };
    let (Some(catalog), Some(row)) = (
        world.get_resource::<ActionCatalog>(),
        seats(world)?.into_iter().find(|r| r.id == seat),
    ) else {
        return Ok(Vec::new());
    };
    let view = super::omni::view(world, catalog, &row, omniscient);
    let defs = world.get_resource::<crate::perception::PerceptionDefs>();
    let table = world.resource::<IntentTable>();
    if !omniscient {
        until::check(&*view, defs, table, seat, u, &Pointer::root().key("until"))?;
    }
    Ok(until::baseline(&*view, u))
}

/// The run of ticks a `step` or a `commit` makes.
pub(super) struct Run {
    pub(super) seat: Option<String>,
    pub(super) until: Option<Until>,
    pub(super) base: Vec<Option<Value>>,
    pub(super) max_ticks: u64,
    pub(super) max_wall_ms: f64,
    /// Lockstep: the last tick the world may run (every seat's commitment).
    pub(super) through: Option<Tick>,
    /// A developer's `omniscient: true`: `until` reads the whole world ([`super::omni`]).
    pub(super) omniscient: bool,
}

/// The sequence number of the last event `seat` perceived (the world's last, when `omniscient`).
fn last_seq(world: &World, seat: &str, omniscient: bool) -> Result<u64, Problem> {
    let (Some(catalog), Some(row)) = (
        world.get_resource::<ActionCatalog>(),
        seats(world)?.into_iter().find(|r| r.id == seat),
    ) else {
        return Ok(0);
    };
    let view = super::omni::view(world, catalog, &row, omniscient);
    Ok(view.events_since(0).last().map_or(0, |e| e.seq))
}

/// A run under way: its answer so far, the last event its seat perceived, and when it started. A
/// session's `step` drives it in one call ([`run_ticks`]); the game thread's loop drives it one
/// tick per boundary ([`super::stepping`]), so its queue is served between ticks (time.md, Threads
/// and the web). Both stop on the same conditions in the same order.
pub(super) struct Progress {
    pub(super) answer: TimeAnswer,
    seen: u64,
    pub(super) started: f64,
}

impl Progress {
    /// A run starting at the world's tick; already stopped (`true`) when the session is halted.
    pub(super) fn start(
        world: &World,
        ctl: &Controller,
        run: &Run,
        now_ms: f64,
    ) -> Result<(Progress, bool), Problem> {
        let from = world.resource::<pocket_sim::SimClock>().tick;
        let mut answer = TimeAnswer {
            from_tick: from,
            tick: from,
            ran: 0,
            stopped: StopReason::Ticks,
            decision: None,
            passed_decisions: 0,
            until: None,
            waiting_for: Vec::new(),
            outcome: None,
            halted: None,
            omniscient: false,
            warnings: Vec::new(),
        };
        if ctl.halted.is_some() {
            answer.stopped = StopReason::Halted;
            let p = Progress {
                answer,
                seen: 0,
                started: now_ms,
            };
            return Ok((p, true));
        }
        // The seat's perceived events are read from where they stood before each tick.
        let seen = match &run.seat {
            Some(seat) => last_seq(world, seat, run.omniscient)?,
            None => 0,
        };
        let p = Progress {
            answer,
            seen,
            started: now_ms,
        };
        Ok((p, false))
    }

    /// Whether the run wants another tick: not past the ticks asked for, nor past every seat's
    /// lockstep commitment (`stopped: "waiting"`).
    pub(super) fn wants_tick(&mut self, run: &Run) -> bool {
        if self.answer.ran >= run.max_ticks {
            return false;
        }
        let next = Tick(self.answer.tick.0 + 1);
        if run.through.is_some_and(|through| next > through) {
            self.answer.stopped = StopReason::Waiting;
            return false;
        }
        true
    }

    /// Whether the tick can run now: a turn's deciding phase holds every tick until the seats end
    /// their turns (`stopped: "waiting"`).
    pub(super) fn can_tick(&mut self, world: &World) -> bool {
        if turns::deciding(world) {
            self.answer.stopped = StopReason::Waiting;
            return false;
        }
        true
    }

    /// After a tick ran: the decision points, then, in order, the episode's end, a halt, `until`
    /// and the wall limit; `true` when the run stops there.
    pub(super) fn after(
        &mut self,
        world: &World,
        ctl: &mut Controller,
        run: &Run,
        report: &StepReport,
        now_ms: f64,
    ) -> Result<bool, Problem> {
        let points = ctl.after_tick(world, &report.decisions, &report.errors, now_ms);
        self.after_points(world, ctl, run, report, &points, now_ms)
    }

    /// [`Progress::after`] with the tick's decision points already computed (once per tick, for
    /// every run under way: the game thread's several waiting seats).
    pub(super) fn after_points(
        &mut self,
        world: &World,
        ctl: &mut Controller,
        run: &Run,
        report: &StepReport,
        points: &[DecisionPoint],
        now_ms: f64,
    ) -> Result<bool, Problem> {
        let answer = &mut self.answer;
        answer.ran += 1;
        answer.tick = report.tick;
        let mine: Option<&DecisionPoint> = run
            .seat
            .as_deref()
            .and_then(|s| points.iter().find(|p| p.seat == s));
        if let Some(o) = turns::outcome(world) {
            answer.outcome = Some(o);
            answer.stopped = StopReason::Done;
            return Ok(true);
        }
        if ctl.halted.is_some() {
            answer.stopped = StopReason::Halted;
            return Ok(true);
        }
        if let (Some(u), Some(seat)) = (&run.until, run.seat.as_deref()) {
            let catalog = world.get_resource::<ActionCatalog>();
            let row = seats(world)?.into_iter().find(|r| r.id == seat);
            if let (Some(catalog), Some(row)) = (catalog, row) {
                let view = super::omni::view(world, catalog, &row, run.omniscient);
                let events = view.events_since(self.seen);
                if let Some(e) = events.last() {
                    self.seen = e.seq;
                }
                let news = until::TickNews {
                    tick: report.tick,
                    decision: mine.is_some(),
                    events: &events,
                };
                let defs = world.get_resource::<crate::perception::PerceptionDefs>();
                let table = world.resource::<IntentTable>();
                if let Some(m) = until::met(&*view, defs, table, u, &run.base, &news) {
                    answer.stopped = if matches!(m.condition, Until::Decision) {
                        StopReason::Decision
                    } else {
                        StopReason::Until
                    };
                    answer.until = Some(m);
                    return Ok(true);
                }
            }
        }
        if mine.is_some() {
            answer.passed_decisions += 1;
        }
        if now_ms - self.started >= run.max_wall_ms {
            answer.stopped = StopReason::WallLimit;
            return Ok(true);
        }
        Ok(false)
    }
}

/// Runs ticks for `run`, after each one computing the decision points and checking, in order, the
/// episode's end, a halt, `until` and the wall limit.
fn run_ticks(
    t: &mut dyn Ticker,
    ctl: &mut Controller,
    run: &Run,
    now: &mut dyn FnMut() -> f64,
) -> Result<TimeAnswer, Problem> {
    let (mut p, stopped) = Progress::start(t.world(), ctl, run, now())?;
    if stopped {
        return Ok(p.answer);
    }
    while p.wants_tick(run) {
        apply_queued(t, ctl, Tick(p.answer.tick.0 + 1));
        if !p.can_tick(t.world()) {
            break;
        }
        let report = t.tick()?;
        if p.after(t.world(), ctl, run, &report, now())? {
            break;
        }
    }
    Ok(p.answer)
}

/// The answer of a run: the seat's pending decision and warnings, the halt as the caller may see
/// it, the episode's outcome, the seats lockstep waits for, the events delta and an observation.
pub(super) fn finish(
    world: &World,
    ctl: &mut Controller,
    caller: &Caller,
    seat: Option<&str>,
    mut answer: TimeAnswer,
    observe: Option<&crate::perception::ObserveRequest>,
    budget_tokens: Option<u32>,
) -> Result<Value, Problem> {
    if let Some(seat) = seat {
        answer.decision = ctl.pending(seat).cloned();
        answer.warnings.extend(ctl.take_warnings(seat));
    }
    if let Some(h) = &answer.halted.clone().or_else(|| ctl.halted.clone()) {
        answer.halted = Some(match caller {
            Caller::Player { .. } => codes::time_halted(),
            _ => h.clone(),
        });
    }
    if answer.outcome.is_none() {
        answer.outcome = turns::outcome(world);
    }
    answer.waiting_for = ctl.waiting_for(Tick(answer.tick.0 + 1));
    let mut v = answer.to_json();
    if let Some(seat) = seat {
        push_delta(world, ctl, caller, seat, budget_tokens, &mut v);
    }
    if let Some(o) = observe {
        v["observation"] = observation(world, ctl, caller, seat, o)?;
    }
    Ok(v)
}

/// The `observe` answer a time request carries (perception.md, Queries), with the seat's intents
/// and pending decision.
pub fn observation(
    world: &World,
    ctl: &Controller,
    caller: &Caller,
    seat: Option<&str>,
    req: &crate::perception::ObserveRequest,
) -> Result<Value, Problem> {
    let mut req = req.clone();
    if req.seat.is_none() && !matches!(caller, Caller::Player { .. }) {
        req.seat = seat.map(str::to_owned);
    }
    let pc = perception_caller(caller);
    let decision = seat
        .and_then(|s| ctl.pending(s))
        .map(DecisionPoint::rendered);
    let parts = match seat {
        Some(s) => crate::action::observe::seat_parts(world, s, decision),
        None => crate::perception::SeatParts::default(),
    };
    let aff = crate::action::observe::CatalogAffordances::new(world);
    let a = crate::perception::observe(world, &pc, &req, &parts, &aff)?;
    Ok(match a.projection {
        crate::projection::Projection::Json => {
            serde_json::from_str(&a.body).unwrap_or(Value::String(a.body))
        }
        _ => Value::String(a.body),
    })
}

fn wall(max_wall_ms: Option<u32>) -> f64 {
    f64::from(max_wall_ms.unwrap_or(WALL_MS).min(MAX_WALL_MS))
}

/// Checks a `step` and readies its run (time.md, Requests): stepped pacing, the clock holder, the
/// episode not over, `until` on what the seat can know; it answers the seat's pending decision
/// (moving time on answers it).
pub(super) fn prepare_step(
    world: &World,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now_ms: f64,
) -> Result<(StepRequest, Run), Problem> {
    let req: StepRequest = decode(raw, &CheckOptions::new("the step request"))?.value;
    ctl.attach(world);
    if !matches!(ctl.pacing, PlayPacing::Stepped) {
        return Err(codes::time_wrong_mode(
            "step",
            ctl.pacing.name(),
            "continuous",
            &["commit", "wait", "continue"],
        ));
    }
    let omniscient = req.omniscient == Some(true);
    match caller {
        Caller::Player { .. } if omniscient => {
            return Err(codes::omniscient_forbidden("player"));
        }
        Caller::Player { .. } if !ctl.players_hold_clock => {
            return Err(codes::time_not_clock_holder("the developer"));
        }
        Caller::Checker => {
            return Err(codes::permission_denied(
                "step",
                "checker",
                "player or developer",
            ));
        }
        _ => {}
    }
    turns::check_episode(world)?;
    let seat = seat_of(world, caller, req.seat.as_deref(), "step")?;
    let base = check_until(world, seat.as_deref(), req.until.as_ref(), omniscient)?;
    if let Some(s) = &seat {
        ctl.answer(s, now_ms);
    }
    let run = Run {
        seat,
        until: req.until.clone(),
        base,
        max_ticks: req.ticks,
        max_wall_ms: wall(req.max_wall_ms),
        through: None,
        omniscient,
    };
    Ok((req, run))
}

/// Checks a real-time `wait` and readies its run (time.md, Requests: `wait` answers when `until`
/// holds, by default when the seat has a decision point, at the episode's end, or at
/// `max_wall_ms`). Ticks follow the clock, so the run has no tick limit; waiting does not answer
/// the seat's pending decision (`continue` or `act {resume}` does).
pub(super) fn prepare_wait(
    world: &World,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
) -> Result<(StepRequest, Run), Problem> {
    let req: StepRequest = decode(raw, &CheckOptions::new("the wait request"))?.value;
    ctl.attach(world);
    if !matches!(ctl.pacing, PlayPacing::RealTime { .. }) {
        return Err(codes::time_wrong_mode(
            "wait",
            ctl.pacing.name(),
            "continuous",
            &["step"],
        ));
    }
    let omniscient = req.omniscient == Some(true);
    match caller {
        Caller::Player { .. } if omniscient => {
            return Err(codes::omniscient_forbidden("player"));
        }
        Caller::Checker => {
            return Err(codes::permission_denied(
                "wait",
                "checker",
                "player or developer",
            ));
        }
        _ => {}
    }
    turns::check_episode(world)?;
    let seat = seat_of(world, caller, req.seat.as_deref(), "wait")?;
    let base = check_until(world, seat.as_deref(), req.until.as_ref(), omniscient)?;
    let run = Run {
        seat,
        until: req.until.clone(),
        base,
        max_ticks: u64::MAX,
        max_wall_ms: wall(req.max_wall_ms),
        through: None,
        omniscient,
    };
    Ok((req, run))
}

/// `step` (time.md, Requests): stepped pacing, the clock holder; it answers the seat's pending
/// decision (moving time on answers it) and runs up to `ticks` ticks.
pub fn step(
    t: &mut dyn Ticker,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now: &mut dyn FnMut() -> f64,
) -> Result<Value, Problem> {
    let (req, run) = prepare_step(t.world(), ctl, caller, raw, now())?;
    let mut answer = run_ticks(t, ctl, &run, now)?;
    answer.omniscient = run.omniscient;
    finish(
        t.world(),
        ctl,
        caller,
        run.seat.as_deref(),
        answer,
        req.observe.as_ref(),
        req.budget_tokens,
    )
}

/// `commit` (time.md, Lockstep): extends the seat's commitment by `ticks` and runs the world as far
/// as every seat has committed; `until` ends the commitment early. A world that cannot reach the
/// caller's commitment yet answers `stopped: "waiting"` with the seats it waits for.
pub fn commit(
    t: &mut dyn Ticker,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now: &mut dyn FnMut() -> f64,
) -> Result<Value, Problem> {
    let req: CommitRequest = decode(raw, &CheckOptions::new("the commit request"))?.value;
    ctl.attach(t.world());
    let PlayPacing::Lockstep {
        seats: committing, ..
    } = ctl.pacing.clone()
    else {
        return Err(codes::time_wrong_mode(
            "commit",
            ctl.pacing.name(),
            "continuous",
            &["step", "continue"],
        ));
    };
    turns::check_episode(t.world())?;
    let Some(seat) = seat_of(t.world(), caller, req.seat.as_deref(), "commit")? else {
        return Err(codes::missing_field(
            &Pointer::root().key("seat"),
            "the commit request",
            "a seat id",
        ));
    };
    if !committing.contains(&seat) {
        return Err(codes::time_wrong_mode(
            "commit",
            "lockstep",
            "continuous",
            &["act"],
        ));
    }
    let base = check_until(t.world(), Some(&seat), req.until.as_ref(), false)?;
    ctl.answer(&seat, now());
    let tick = t.world().resource::<pocket_sim::SimClock>().tick;
    let mine = ctl
        .commitments
        .get(&seat)
        .copied()
        .unwrap_or(tick)
        .max(tick);
    let target = Tick(mine.0 + req.ticks);
    ctl.commitments.insert(seat.clone(), target);
    let through = committing
        .iter()
        .map(|s| ctl.commitments.get(s).copied().unwrap_or(tick))
        .min()
        .unwrap_or(tick);
    let run = Run {
        seat: Some(seat.clone()),
        until: req.until.clone(),
        base,
        max_ticks: through.0.saturating_sub(tick.0),
        max_wall_ms: wall(req.max_wall_ms),
        through: Some(through),
        omniscient: false,
    };
    let mut answer = run_ticks(t, ctl, &run, now)?;
    if answer.stopped == StopReason::Until || answer.stopped == StopReason::Decision {
        // `until` ends the commitment early: the world stops there for everyone.
        ctl.commitments.insert(seat.clone(), answer.tick);
    } else if answer.tick < target && answer.stopped == StopReason::Ticks {
        answer.stopped = StopReason::Waiting;
    }
    finish(
        t.world(),
        ctl,
        caller,
        Some(&seat),
        answer,
        req.observe.as_ref(),
        req.budget_tokens,
    )
}

/// `continue`: answers the seat's pending decision; without one, the warning `time.no_decision`.
pub fn continue_(
    t: &dyn Ticker,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now: &mut dyn FnMut() -> f64,
) -> Result<Value, Problem> {
    let req: ContinueRequest = decode(raw, &CheckOptions::new("the continue request"))?.value;
    ctl.attach(t.world());
    let Some(seat) = seat_of(t.world(), caller, req.seat.as_deref(), "continue")? else {
        return Err(codes::missing_field(
            &Pointer::root().key("seat"),
            "the continue request",
            "a seat id",
        ));
    };
    let tick = t.world().resource::<pocket_sim::SimClock>().tick;
    let answered = ctl.answer(&seat, now());
    let mut answer = TimeAnswer {
        from_tick: tick,
        tick,
        ran: 0,
        stopped: StopReason::Answered,
        decision: None,
        passed_decisions: 0,
        until: None,
        waiting_for: Vec::new(),
        outcome: None,
        halted: None,
        omniscient: false,
        warnings: Vec::new(),
    };
    if answered.is_none() {
        answer.warnings.push(codes::time_no_decision());
    }
    finish(
        t.world(),
        ctl,
        caller,
        Some(&seat),
        answer,
        None,
        req.budget_tokens,
    )
}

/// After an `act` applied for `seat` outside [`act`] (a replay's): the seat is driven (idle
/// restarts), and with `resume` its pending decision is answered as `continue` answers it.
pub fn acted(
    ctl: &mut Controller,
    seat: &str,
    applied_at: Tick,
    resume: bool,
    now_ms: f64,
) -> Vec<Problem> {
    ctl.acted(seat, applied_at);
    if resume && ctl.answer(seat, now_ms).is_none() {
        return vec![codes::time_no_decision()];
    }
    Vec::new()
}

/// Applies the lockstep acts queued for the boundary before `next`, in seat order then
/// submission order (threads.md 5.2); one that no longer validates is dropped and its seat gets
/// the warning `action.dropped` with its next answer (session state: nothing enters the world).
fn apply_queued(t: &mut dyn Ticker, ctl: &mut Controller, next: Tick) {
    if ctl.queued.is_empty() {
        return;
    }
    let order: Vec<String> = seats(t.world())
        .map(|rows| rows.into_iter().map(|r| r.id).collect())
        .unwrap_or_default();
    let mut due: Vec<Queued> = Vec::new();
    ctl.queued.retain(|q| {
        if q.at <= next {
            due.push(q.clone());
            false
        } else {
            true
        }
    });
    due.sort_by_key(|q| {
        order
            .iter()
            .position(|s| *s == q.seat)
            .unwrap_or(usize::MAX)
    });
    for q in due {
        let caller = Caller::Player {
            seat: q.seat.clone(),
        };
        match t.act(&caller, &q.request) {
            Ok(done) => ctl.acted(&q.seat, done.applied_at),
            Err(p) => {
                let w = codes::dropped(next.0, q.request.clone(), &p);
                ctl.warnings.entry(q.seat.clone()).or_default().push(w);
            }
        }
    }
}

/// `act` in a session (actions.md, The act request and When an action takes effect): applied at
/// the boundary now, or, in lockstep pacing for a seat committed beyond the world's next tick,
/// validated now and queued for the boundary before its next uncommitted tick (`pending: true`;
/// the intent ids arrive with `intent.started`). With `resume` it answers the seat's pending
/// decision. The answer is `ActResult` with the events delta; the `ActDone` is what was applied
/// now, `None` for a pending act.
pub fn act(
    t: &mut dyn Ticker,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now: &mut dyn FnMut() -> f64,
) -> Result<(Value, Option<ActDone>), Problem> {
    ctl.attach(t.world());
    let named = raw.get("seat").and_then(Value::as_str);
    let seat = crate::action::validate::seat_for(t.world(), caller, named, "act")?.id;
    let resume = raw.get("resume").and_then(Value::as_bool) == Some(true);
    let budget = raw
        .get("budget_tokens")
        .and_then(Value::as_u64)
        .and_then(|b| u32::try_from(b).ok());
    let tick = t.world().resource::<pocket_sim::SimClock>().tick;
    let lockstep_at = match &ctl.pacing {
        PlayPacing::Lockstep {
            seats: committing,
            delay_ticks,
        } if committing.contains(&seat) => {
            let c = ctl.commitments.get(&seat).copied().unwrap_or(tick);
            let at = Tick(c.0 + 1 + u64::from(*delay_ticks));
            (at.0 > tick.0 + 1).then_some(at)
        }
        _ => None,
    };
    let (mut v, done) = match lockstep_at {
        Some(at) => {
            let catalog = t
                .world()
                .get_resource::<ActionCatalog>()
                .cloned()
                .ok_or_else(|| {
                    crate::action::state::internal("act", "the game declares no actions")
                })?;
            turns::check_episode(t.world())?;
            let (_, v) = crate::action::validate::validate(t.world(), &catalog, caller, raw)?;
            ctl.queued.push(Queued {
                at,
                seat: seat.clone(),
                request: raw.clone(),
            });
            let out = json!({"tick": tick.0, "applied_at": at.0, "pending": true, "outcomes": [],
                             "warnings": v.warnings});
            (out, None)
        }
        None => {
            let done = t.act(caller, raw)?;
            ctl.acted(&seat, done.applied_at);
            let out = json!({"tick": tick.0, "applied_at": done.applied_at.0, "pending": false,
                             "outcomes": done.outcomes, "warnings": done.warnings});
            (out, Some(done))
        }
    };
    let mut warnings: Vec<Problem> =
        serde_json::from_value(v["warnings"].clone()).unwrap_or_default();
    if resume && ctl.answer(&seat, now()).is_none() {
        warnings.push(codes::time_no_decision());
    }
    warnings.extend(ctl.take_warnings(&seat));
    v["warnings"] = serde_json::to_value(&warnings).unwrap_or(Value::Null);
    push_delta(t.world(), ctl, caller, &seat, budget, &mut v);
    Ok((v, done))
}

/// The events delta of `seat` since its push cursor within `budget` (default 400) tokens, into `v`
/// as `events` and `cursor` (perception.md, Push). The cursor is the seat's: it moves past what was
/// shown only when the seat's own player called. A developer who names the seat reads the same
/// delta and leaves the cursor where the seat's calls put it, so the player still receives those
/// events.
pub fn push_delta(
    world: &World,
    ctl: &mut Controller,
    caller: &Caller,
    seat: &str,
    budget: Option<u32>,
    v: &mut Value,
) {
    let since = ctl.push.get(seat).copied().unwrap_or(0);
    let limit = usize::try_from(budget.unwrap_or(400)).unwrap_or(usize::MAX) * 4;
    let Ok(d) = crate::perception::delta(
        world,
        seat,
        since,
        crate::projection::Projection::Json,
        limit,
    ) else {
        return;
    };
    v["events"] = Value::Array(
        d.events
            .iter()
            .filter_map(|e| serde_json::from_str(e).ok())
            .collect(),
    );
    v["cursor"] = json!(d.cursor);
    if matches!(caller, Caller::Player { seat: own } if own == seat) {
        ctl.push.insert(seat.to_owned(), d.cursor);
    }
}
