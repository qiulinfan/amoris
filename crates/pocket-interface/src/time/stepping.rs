//! A `step` the game thread's loop drives one tick per boundary (shared/contract/time.md, Threads and
//! the web: "a long `step` or `wait` keeps the queue served between ticks"). [`begin`] checks the
//! request and readies its run as [`super::session::step`] does; the loop asks
//! [`Stepping::before_tick`] whether the next tick may run, runs it as it runs every tick, and
//! hands its report to [`Stepping::after_tick`], which computes the decision points and the stops
//! in the same order; [`Stepping::finish`] answers with the same `TimeResult`. Nothing here writes
//! the world.

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use pocket_sim::StepReport;
use serde_json::Value;

use super::control::Controller;
use super::play::StepRequest;
use super::session::{self, Progress, Run};
use crate::action::request::Caller;

/// A `step` under way on the loop.
pub struct Stepping {
    caller: Caller,
    req: StepRequest,
    run: Run,
    progress: Progress,
}

/// What [`begin`] gives: the answer at once (`ticks: 0`, a halted session), or a run to drive.
pub enum Begun {
    Answered(Value),
    Running(Box<Stepping>),
}

/// Checks a `step` for `caller` and readies its run (refused as [`super::session::step`] refuses
/// it), answering at once when no tick is to run.
pub fn begin(
    world: &World,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now_ms: f64,
) -> Result<Begun, Problem> {
    let (req, run) = session::prepare_step(world, ctl, caller, raw, now_ms)?;
    let (progress, stopped) = Progress::start(world, ctl, &run, now_ms)?;
    let s = Stepping {
        caller: caller.clone(),
        req,
        run,
        progress,
    };
    if stopped || s.run.max_ticks == 0 {
        return s.finish(world, ctl).map(Begun::Answered);
    }
    Ok(Begun::Running(Box::new(s)))
}

impl Stepping {
    /// The most ticks the run may still take: what the loop's time model is asked to run.
    pub fn ticks_left(&self) -> u64 {
        self.run.max_ticks.saturating_sub(self.progress.answer.ran)
    }

    /// Before the loop runs the next tick: whether it may (`false`: the run ends here, answered
    /// by [`Stepping::finish`]).
    pub fn before_tick(&mut self, world: &World) -> bool {
        self.progress.wants_tick(&self.run) && self.progress.can_tick(world)
    }

    /// After a tick the loop ran: the decision points and the stops; `true` when the run ends.
    pub fn after_tick(
        &mut self,
        world: &World,
        ctl: &mut Controller,
        report: &StepReport,
        now_ms: f64,
    ) -> Result<bool, Problem> {
        let stop = self.progress.after(world, ctl, &self.run, report, now_ms)?;
        Ok(stop || self.progress.answer.ran >= self.run.max_ticks)
    }

    /// The `TimeResult` of the run.
    pub fn finish(self, world: &World, ctl: &mut Controller) -> Result<Value, Problem> {
        let mut answer = self.progress.answer;
        answer.omniscient = self.run.omniscient;
        session::finish(
            world,
            ctl,
            &self.caller,
            self.run.seat.as_deref(),
            answer,
            self.req.observe.as_ref(),
            self.req.budget_tokens,
        )
    }
}
