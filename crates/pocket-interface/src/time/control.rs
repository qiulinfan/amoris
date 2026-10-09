//! The session's time controller (shared/contract/time.md, Pacing, Decision points, Halts): the
//! pacing, every seat's decision state and thinking clock, the sources of pause, a halt after a
//! script failure, and lockstep commitments. All of it is session state, never world state: it
//! decides at which boundary ticks run and actions land, which the replay records, and never what
//! a tick computes. The wall clock reaches it only as `now_ms` from the loop that asks.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::World;
use pocket_contract::{Problem, codes};
use pocket_sim::{DecisionRequest, SimClock, Tick};
use serde_json::{Value, json};

use super::decide::{self, DecisionDef, SeatDecisions};
use super::play::{DecisionFilter, DecisionPoint, PlayPacing, ThinkingClock};
use super::{Pace, TimeModel};
use crate::action::state::seats;

/// A seat's thinking clock: the time left, and since when it runs (wall milliseconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockState {
    pub left_s: f64,
    pub since_ms: Option<f64>,
}

impl ClockState {
    /// When a running clock runs out, wall milliseconds: compared with the loop's clock as it is,
    /// so a wait until it never falls short by a rounding (the time left at that instant is 0).
    pub fn out_at(&self) -> Option<f64> {
        self.since_ms.map(|t| t + self.left_s * 1000.0)
    }

    /// Whether the clock has time left at `now_ms`.
    pub fn has_time(&self, now_ms: f64) -> bool {
        match self.out_at() {
            Some(out) => now_ms < out,
            None => self.left_s > 0.0,
        }
    }

    /// The time left at `now_ms`.
    pub fn left_at(&self, now_ms: f64) -> f64 {
        match self.since_ms {
            Some(t) => pocket_sim::math::max(self.left_s - (now_ms - t) / 1000.0, 0.0),
            None => self.left_s,
        }
    }
}

/// Whether a tick's error is a script system's failure, which halts the world (time.md, Halts):
/// a `script.*` problem, or the simulation's `sim.system_failed` whose cause is one (how a rule
/// that throws reaches a tick's report: the invocation's effects discarded, the script's error as
/// its `cause`).
pub fn script_failure(e: &Problem) -> bool {
    let script = |code: &str| code.split('.').next() == Some("script");
    script(&e.code)
        || (e.code == "sim.system_failed"
            && e.detail
                .get("cause")
                .and_then(|c| c.get("code"))
                .and_then(Value::as_str)
                .is_some_and(script))
}

/// The time controller of one world's session.
#[derive(Clone, Debug)]
pub struct Controller {
    pub pacing: PlayPacing,
    /// Each seat's decision state, by seat id.
    pub seats: BTreeMap<String, SeatDecisions>,
    /// Thinking clocks (real-time pacing with a clock), by seat id.
    pub clocks: BTreeMap<String, ClockState>,
    pub paused_by_developer: bool,
    /// Seats the session allows to pause (time.md, Real time: `time.cannot_pause` for the others).
    pub may_pause: BTreeSet<String>,
    /// Seats the session allows to pause that paused.
    pub paused_by_seats: BTreeSet<String>,
    /// Every human seat's page is hidden.
    pub hidden_page: bool,
    /// The halt after a script failure, until a developer resumes or a hot update lands.
    pub halted: Option<Problem>,
    /// Lockstep: the last tick each seat committed through.
    pub commitments: BTreeMap<String, Tick>,
    /// Later warnings per seat, given with its next answer (`time.clock_out`, `action.dropped`).
    pub warnings: BTreeMap<String, Vec<Problem>>,
    pub declared: Vec<DecisionDef>,
    pub default_filter: DecisionFilter,
    /// Each seat's push cursor: the last perceived event an answer carried (perception.md, Push).
    pub push: BTreeMap<String, u64>,
    /// Lockstep acts waiting for their boundary.
    pub queued: Vec<super::session::Queued>,
    /// Stepped pacing: players hold the clock (a single-player session, time.md open choice 3);
    /// otherwise only the developer steps.
    pub players_hold_clock: bool,
    /// A developer's session: requested decisions a player would not receive (their event unseen)
    /// are kept (time.md, Decision points).
    pub all_requests: bool,
    /// Real time was held by decisions or pauses at the last boundary.
    held: bool,
}

impl Controller {
    pub fn new(
        pacing: PlayPacing,
        default_filter: DecisionFilter,
        declared: Vec<DecisionDef>,
    ) -> Controller {
        Controller {
            pacing,
            seats: BTreeMap::new(),
            clocks: BTreeMap::new(),
            paused_by_developer: false,
            may_pause: BTreeSet::new(),
            paused_by_seats: BTreeSet::new(),
            hidden_page: false,
            halted: None,
            commitments: BTreeMap::new(),
            warnings: BTreeMap::new(),
            declared,
            default_filter,
            push: BTreeMap::new(),
            queued: Vec::new(),
            players_hold_clock: true,
            all_requests: false,
            held: false,
        }
    }

    fn clock_spec(&self) -> Option<ThinkingClock> {
        match &self.pacing {
            PlayPacing::RealTime { clock, .. } => *clock,
            _ => None,
        }
    }

    fn pause_on_decision(&self) -> bool {
        matches!(
            self.pacing,
            PlayPacing::RealTime {
                pause_on_decision: true,
                ..
            }
        )
    }

    /// Makes sure every seat of the world has its decision state (and clock); at tick 0 a new seat
    /// gets its `start` decision point. The points that arose.
    pub fn attach(&mut self, world: &World) -> Vec<DecisionPoint> {
        let tick = world.resource::<SimClock>().tick;
        let mut fresh = BTreeMap::new();
        for row in seats(world).unwrap_or_default() {
            if !self.seats.contains_key(&row.id) {
                fresh.insert(
                    row.id.clone(),
                    SeatDecisions::new(self.default_filter.clone(), tick),
                );
            }
            if let Some(c) = self.clock_spec() {
                self.clocks.entry(row.id.clone()).or_insert(ClockState {
                    left_s: c.initial_s,
                    since_ms: None,
                });
            }
        }
        let points = if tick == Tick(0) {
            decide::start(world, &mut fresh)
        } else {
            Vec::new()
        };
        self.seats.extend(fresh);
        points
    }

    /// Changes the pacing at a boundary; a thinking clock starts afresh.
    pub fn set_pacing(&mut self, pacing: PlayPacing) {
        self.pacing = pacing;
        self.clocks.clear();
        self.held = false;
    }

    /// After a tick: the decision points of every seat (pending until answered; one already pending
    /// is passed over and counted by the caller), clocks started for them, and a halt when a script
    /// system failed.
    pub fn after_tick(
        &mut self,
        world: &World,
        requests: &[DecisionRequest],
        errors: &[Problem],
        now_ms: f64,
    ) -> Vec<DecisionPoint> {
        if self.halted.is_none()
            && let Some(e) = errors.iter().find(|e| script_failure(e))
        {
            self.halted = Some(e.clone());
        }
        let mut points = decide::after_tick(
            world,
            &mut self.seats,
            requests,
            &self.declared,
            self.all_requests,
        );
        for p in &mut points {
            if let Some(c) = self.clocks.get_mut(&p.seat) {
                if c.since_ms.is_none() {
                    c.since_ms = Some(now_ms);
                }
                p.clock_s = Some(c.left_at(now_ms));
                if let Some(st) = self.seats.get_mut(&p.seat) {
                    st.pending = Some(p.clone());
                }
            }
        }
        points
    }

    /// A hot update landed (time.md, Halts): it ends a halt, as a developer's `resume` does.
    pub fn hot_update_landed(&mut self) {
        self.halted = None;
    }

    /// A developer resumed (time.md, `resume`): its pause ends, and a halt in any pacing.
    pub fn developer_resumed(&mut self) {
        self.paused_by_developer = false;
        self.halted = None;
    }

    /// The world was replaced under the session (a restore, a rewind): every seat's decision
    /// state is rebased on it ([`decide::rebase`]), so decision points and push deltas follow the
    /// restored timeline rather than the abandoned one: the event cursor is the last event the
    /// restored observer perceived (its ring restarts there), the push cursor comes back to it
    /// at the latest, the pending decision is dropped; at tick 0 every seat has the episode's
    /// `start` point again. Seats the world no longer declares are forgotten; running thinking
    /// clocks stop, queued lockstep acts are dropped and commitments come back to the restored
    /// tick. A halt ends, as mcp.md 7.1 has a `restore` end it: the restored world has not failed
    /// (if the same rule fails again at the same tick, the world halts again there).
    pub fn world_replaced(&mut self, world: &World) {
        let clock = *world.resource::<SimClock>();
        let tick = clock.tick;
        let rows = seats(world).unwrap_or_default();
        let catalog = world.get_resource::<crate::action::ActionCatalog>();
        self.seats.retain(|s, _| rows.iter().any(|r| r.id == *s));
        for row in &rows {
            let latest = catalog.map_or(0, |c| {
                (c.view)(world, row)
                    .events_since(0)
                    .last()
                    .map_or(0, |e| e.seq)
            });
            if let Some(st) = self.seats.get_mut(&row.id) {
                decide::rebase(st, clock, latest);
            }
            if let Some(push) = self.push.get_mut(&row.id) {
                *push = (*push).min(latest);
            }
        }
        self.push.retain(|s, _| rows.iter().any(|r| r.id == *s));
        if tick == Tick(0) {
            decide::start(world, &mut self.seats);
        }
        for c in self.clocks.values_mut() {
            c.since_ms = None;
        }
        self.queued.clear();
        for t in self.commitments.values_mut() {
            *t = (*t).min(tick);
        }
        self.held = false;
        self.halted = None;
    }

    /// The seat's pending decision.
    pub fn pending(&self, seat: &str) -> Option<&DecisionPoint> {
        self.seats.get(seat)?.pending.as_ref()
    }

    /// Answers the seat's pending decision (`continue`, `act {resume}`, or moving time on): its
    /// clock stops and gains the increment, capped at the maximum. `None` when none was pending.
    pub fn answer(&mut self, seat: &str, now_ms: f64) -> Option<DecisionPoint> {
        let st = self.seats.get_mut(seat)?;
        let p = st.pending.take()?;
        st.answered += 1;
        let spec = self.clock_spec();
        if let (Some(c), Some(spec)) = (self.clocks.get_mut(seat), spec) {
            let left = c.left_at(now_ms);
            c.left_s = pocket_sim::math::min(left + spec.increment_s, spec.max_s);
            c.since_ms = None;
        }
        Some(p)
    }

    /// The seat acted at the boundary before `tick`.
    pub fn acted(&mut self, seat: &str, tick: Tick) {
        if let Some(st) = self.seats.get_mut(seat) {
            decide::acted(st, tick);
        }
    }

    /// Answers for the seats whose clock ran out by `now_ms`, each with the later warning
    /// `time.clock_out`; the seats.
    pub fn expire_clocks(&mut self, now_ms: f64) -> Vec<String> {
        let out: Vec<String> = self
            .clocks
            .iter()
            .filter(|(s, c)| {
                c.since_ms.is_some()
                    && !c.has_time(now_ms)
                    && self.seats.get(*s).is_some_and(|st| st.pending.is_some())
            })
            .map(|(s, _)| s.clone())
            .collect();
        for s in &out {
            if let Some(p) = self.answer(s, now_ms) {
                let w = codes::time_clock_out(json!(p.id), p.tick.0);
                self.warnings.entry(s.clone()).or_default().push(w);
            }
        }
        out
    }

    /// The later warnings for `seat`, taken.
    pub fn take_warnings(&mut self, seat: &str) -> Vec<Problem> {
        self.warnings.remove(seat).unwrap_or_default()
    }

    /// The seats whose pending decisions hold the world (real time with pause-on-decision, time
    /// left on their clock or no clock).
    pub fn paused_for(&self, now_ms: f64) -> Vec<String> {
        if !self.pause_on_decision() {
            return Vec::new();
        }
        self.seats
            .iter()
            .filter(|(s, st)| {
                st.pending.is_some() && self.clocks.get(*s).is_none_or(|c| c.has_time(now_ms))
            })
            .map(|(s, _)| s.clone())
            .collect()
    }

    /// Starts the clock of every seat with a pending decision whose clock is not running (the
    /// episode's `start` point, which arises before any wall time is known).
    fn start_clocks(&mut self, now_ms: f64) {
        for (seat, c) in &mut self.clocks {
            if c.since_ms.is_none() && self.seats.get(seat).is_some_and(|s| s.pending.is_some()) {
                c.since_ms = Some(now_ms);
            }
        }
    }

    /// The earliest instant a running clock runs out.
    fn next_clock_out(&self, now_ms: f64) -> Option<f64> {
        self.clocks
            .values()
            .filter_map(ClockState::out_at)
            .map(|t| pocket_sim::math::max(t, now_ms))
            .reduce(pocket_sim::math::min)
    }

    /// Lockstep: the seats not yet committed through `tick`.
    pub fn waiting_for(&self, tick: Tick) -> Vec<String> {
        match &self.pacing {
            PlayPacing::Lockstep { seats, .. } => seats
                .iter()
                .filter(|s| self.commitments.get(*s).is_none_or(|c| *c < tick))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The answer at a boundary (time.md, Threads and the web): the halt, the episode's end and a
    /// turn's deciding phase stop ticks in every pacing; lockstep runs a tick when every seat has
    /// committed through it; real time holds while a decision or a pause source holds, waking for
    /// the next thinking clock to run out; otherwise the loop's model decides.
    pub fn pace(&mut self, world: &World, model: &mut TimeModel, now_ms: f64) -> Pace {
        if self.halted.is_some()
            || super::turns::outcome(world).is_some()
            || super::turns::deciding(world)
        {
            return match model.pace(now_ms) {
                Pace::Quit => Pace::Quit,
                _ => Pace::WaitForCommand,
            };
        }
        match &self.pacing {
            PlayPacing::Stepped => model.pace(now_ms),
            PlayPacing::Lockstep { .. } => {
                let next = Tick(world.resource::<SimClock>().tick.0 + 1);
                match model.pace(now_ms) {
                    Pace::Quit => Pace::Quit,
                    Pace::RunTick => Pace::RunTick,
                    _ if self.waiting_for(next).is_empty() => Pace::RunTick,
                    _ => Pace::WaitForCommand,
                }
            }
            PlayPacing::RealTime { .. } => {
                self.start_clocks(now_ms);
                self.expire_clocks(now_ms);
                let hold = !self.paused_for(now_ms).is_empty()
                    || self.paused_by_developer
                    || !self.paused_by_seats.is_empty()
                    || self.hidden_page;
                if hold && model.steps_due() == 0 {
                    self.held = true;
                    return match self.next_clock_out(now_ms) {
                        Some(t) if !self.paused_for(now_ms).is_empty() => Pace::WaitUntil(t),
                        _ => Pace::WaitForCommand,
                    };
                }
                if self.held {
                    self.held = false;
                    model.rebase();
                }
                model.pace(now_ms)
            }
        }
    }

    /// `time.status` (time.md, Requests), filtered for the caller: a developer or checker sees every
    /// seat's decisions, clocks and commitments; a player its own, and others by name only.
    pub fn status(
        &self,
        world: &World,
        model: &TimeModel,
        seat: Option<&str>,
        now_ms: f64,
    ) -> Value {
        let clock = *world.resource::<SimClock>();
        let mine = |s: &str| seat.is_none_or(|x| x == s);
        let decisions: Vec<Value> = self
            .seats
            .iter()
            .filter(|(s, _)| mine(s))
            .filter_map(|(_, st)| st.pending.as_ref())
            .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
            .collect();
        let clocks: Vec<Value> = self
            .clocks
            .iter()
            .filter(|(s, _)| mine(s))
            .map(|(s, c)| json!({"seat": s, "clock_s": c.left_at(now_ms), "running": c.since_ms.is_some()}))
            .collect();
        let commitments: Vec<Value> = self
            .commitments
            .iter()
            .filter(|(s, _)| mine(s))
            .map(|(s, t)| json!({"seat": s, "through_tick": t.0}))
            .collect();
        let mut paused_by = Vec::new();
        if self.paused_by_developer || model.paused() {
            paused_by.push("developer");
        }
        if !self.paused_by_seats.is_empty() {
            paused_by.push("seat");
        }
        let paused_for = self.paused_for(now_ms);
        if !paused_for.is_empty() {
            paused_by.push("decision");
        }
        if self.hidden_page {
            paused_by.push("hidden_page");
        }
        let structure = world
            .get_resource::<super::turns::TimeRules>()
            .map(|r| serde_json::to_value(&r.structure).unwrap_or(Value::Null))
            .unwrap_or(json!({"structure": "continuous"}));
        let turn = world
            .get_resource::<super::turns::TurnState>()
            .map(|t| serde_json::to_value(t).unwrap_or(Value::Null));
        json!({
            "tick": clock.tick.0,
            "t_s": clock.time(),
            "structure": structure,
            "turn": turn,
            "pacing": self.pacing,
            "paused": !paused_by.is_empty(),
            "paused_for": paused_for,
            "paused_by": paused_by,
            "halted": self.halted.is_some(),
            "decisions": decisions,
            "clocks": clocks,
            "commitments": commitments,
            "waiting_for": self.waiting_for(Tick(clock.tick.0 + 1)),
            "episode": super::turns::outcome(world),
            "behind_ms": model.behind_ms(),
        })
    }
}

#[cfg(test)]
mod tests {
    use pocket_contract::{Problem, detail};
    use pocket_sim::sim::system_failed;
    use pocket_sim::{SystemKey, Tick, TickPhase};

    use super::script_failure;

    /// A rule that throws reaches a tick's report as `sim.system_failed` with the script's error as
    /// its cause (pocket-script's `program.rs`), which halts like a `script.*` problem; a failed
    /// system of another family does not.
    #[test]
    fn a_failed_script_system_is_a_script_failure() {
        let key = SystemKey::new("script:rounding").unwrap();
        let cause = Problem::new("script.exception", "planted", detail([]));
        let failed = system_failed(Tick(3), TickPhase::Update, &key, None, &cause);
        assert!(script_failure(&failed));
        assert!(script_failure(&cause));
        let physics = Problem::new("physics.solver_diverged", "too fast", detail([]));
        let key = SystemKey::new("physics.step").unwrap();
        let failed = system_failed(Tick(3), TickPhase::Update, &key, None, &physics);
        assert!(!script_failure(&failed));
        assert!(!script_failure(&physics));
    }
}
