//! The time model's answer at each boundary (docs/spec/threads.md 3.3; shared/contract/time.md,
//! Pacing): run a tick now, wait until an instant of the loop's clock, wait for a command, or quit.
//! The slice 1 subset: stepped pacing and real time with pauses; lockstep, turns, decisions and
//! thinking clocks come with the player interface (slice 2).
//!
//! The wall clock is read by the loop that asks and passed in as `now_ms`; nothing here reaches a
//! tick, so pacing changes when ticks run, never what they compute.
//!
//! Slice 2 adds the rest of shared/contract/time.md: the turn structure and the episode
//! ([`turns`], world state), decision points ([`decide`]), `until` ([`until`], with a developer's
//! omniscient form in [`omni`]), the session's time controller with pauses, thinking clocks and
//! lockstep commitments ([`control`]), and its requests ([`session`]; `pause` and `resume` in
//! [`pause`]).

pub mod control;
pub mod decide;
pub mod omni;
pub mod pause;
pub mod play;
pub mod session;
pub mod stepping;
pub mod turns;
pub mod until;

use pocket_contract::codes::{Range, out_of_range};
use pocket_contract::{Pointer, Problem};
use pocket_sim::TickRate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The catch-up limit (threads.md 3.3; chosen, not measured): ticks run back to back when the game
/// falls behind real time, after which the excess wall time is dropped.
pub const MAX_CATCH_UP: u32 = 5;

/// The fastest real-time pacing, as a multiple of real time (chosen, not measured: a 60 Hz game at
/// 1,000 times real time asks for a tick every 17 microseconds, beyond what a tick costs, so a
/// larger speed would only mean "as fast as it goes", which `step` already says).
pub const MAX_SPEED: u32 = 1000;

/// How ticks are paced (spec-contract's `Pacing`, slice 1 subset).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pacing {
    /// Ticks run only when a `step` asks for them.
    Stepped,
    /// Ticks follow the loop's clock at `speed` times real time.
    RealTime { speed: f64 },
}

impl Pacing {
    /// `request.out_of_range {path: "pacing.real_time.speed", got, min_exclusive: 0, max}` for a
    /// real-time speed that is not a finite number above 0 and at most [`MAX_SPEED`]: a speed of
    /// 0 or below has no interval between ticks, so it is refused rather than run at some other
    /// speed (charter 3.4).
    pub fn check(&self) -> Result<(), Problem> {
        match *self {
            Pacing::RealTime { speed }
                if !(speed.is_finite() && speed > 0.0 && speed <= f64::from(MAX_SPEED)) =>
            {
                let got = if speed.is_finite() {
                    json!(speed)
                } else if speed.is_nan() {
                    Value::from("NaN")
                } else if speed > 0.0 {
                    Value::from("Infinity")
                } else {
                    Value::from("-Infinity")
                };
                let range = Range {
                    min_exclusive: Some(0.into()),
                    max: Some(MAX_SPEED.into()),
                    ..Range::default()
                };
                let path = Pointer::root().key("pacing").key("real_time").key("speed");
                Err(out_of_range(&path, &got, &range, None))
            }
            _ => Ok(()),
        }
    }
}

/// What the loop does next (threads.md 3.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pace {
    /// Run the next tick now (stepping, or real time that is due).
    RunTick,
    /// Real time: the next tick is due at this instant of the loop's clock, in milliseconds.
    WaitUntil(f64),
    /// Paused or stepped with nothing asked: only a command can change the answer.
    WaitForCommand,
    /// Shut down after this boundary.
    Quit,
}

/// The time model of one world.
#[derive(Clone, Debug)]
pub struct TimeModel {
    pacing: Pacing,
    paused: bool,
    quit: bool,
    /// Ticks `step` asked for and not yet run.
    steps_due: u64,
    /// The tick's length in milliseconds of game time.
    tick_ms: f64,
    /// Real time: when the next tick is due on the loop's clock.
    next_due: Option<f64>,
    behind_ms: f64,
    /// Real time: catch-up ticks run back to back, each with the next one already due.
    burst: u32,
}

impl TimeModel {
    /// A model at `rate`, paced by `pacing` (checked by [`Pacing::check`] first), not paused.
    pub fn new(rate: TickRate, pacing: Pacing) -> TimeModel {
        debug_assert!(pacing.check().is_ok(), "an unchecked pacing: {pacing:?}");
        TimeModel {
            pacing,
            paused: false,
            quit: false,
            steps_due: 0,
            tick_ms: 1000.0 / f64::from(rate.0),
            next_due: None,
            behind_ms: 0.0,
            burst: 0,
        }
    }

    pub fn pacing(&self) -> Pacing {
        self.pacing
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Ticks asked for by `step` and not yet run.
    pub fn steps_due(&self) -> u64 {
        self.steps_due
    }

    /// Real time: how far the game is behind its clock; `None` otherwise.
    pub fn behind_ms(&self) -> Option<f64> {
        match self.pacing {
            Pacing::RealTime { .. } if !self.paused => Some(self.behind_ms),
            _ => None,
        }
    }

    /// The interval between ticks on the loop's clock.
    fn interval(&self) -> f64 {
        match self.pacing {
            Pacing::RealTime { speed } => {
                debug_assert!(
                    speed.is_finite() && speed > 0.0,
                    "an unchecked speed {speed}"
                );
                self.tick_ms / speed
            }
            Pacing::Stepped => self.tick_ms,
        }
    }

    /// The answer at a boundary.
    pub fn pace(&mut self, now_ms: f64) -> Pace {
        if self.quit {
            return Pace::Quit;
        }
        if self.steps_due > 0 {
            return Pace::RunTick;
        }
        if self.paused {
            return Pace::WaitForCommand;
        }
        match self.pacing {
            Pacing::Stepped => Pace::WaitForCommand,
            Pacing::RealTime { .. } => {
                let due = *self.next_due.get_or_insert(now_ms + self.interval());
                if now_ms >= due {
                    Pace::RunTick
                } else {
                    self.burst = 0;
                    Pace::WaitUntil(due)
                }
            }
        }
    }

    /// A tick ran, finishing at `now_ms`.
    pub fn ran_tick(&mut self, now_ms: f64) {
        if self.steps_due > 0 {
            self.steps_due -= 1;
            return;
        }
        let interval = self.interval();
        if let Some(due) = &mut self.next_due {
            *due += interval;
            let late = now_ms - *due;
            // A tick after which the next one is due already is a catch-up tick.
            self.burst = if late >= 0.0 {
                self.burst.saturating_add(1)
            } else {
                0
            };
            if self.burst >= MAX_CATCH_UP {
                // threads.md 3.3: at most MAX_CATCH_UP ticks back to back; with the next one due
                // already, the excess wall time is dropped and the next tick waits its interval,
                // so the game slows down instead of spiralling.
                self.behind_ms = late;
                *due = now_ms + interval;
                self.burst = 0;
            } else {
                self.behind_ms = pocket_sim::math::max(late, 0.0);
            }
        }
    }

    /// `step {ticks}`: run that many more ticks, whatever the pacing.
    pub fn step(&mut self, ticks: u64) {
        self.steps_due = self.steps_due.saturating_add(ticks);
    }

    /// Drops ticks asked for and not run (a step whose world stopped).
    pub fn cancel_steps(&mut self) {
        self.steps_due = 0;
    }

    pub fn pause(&mut self) {
        self.paused = true;
        self.next_due = None;
    }

    pub fn resume(&mut self) {
        self.paused = false;
        self.next_due = None;
    }

    /// Forgets when the next tick was due, keeping the pause: real time held by something else
    /// (a decision, time.md's pause-on-decision) resumes from now instead of catching up.
    pub fn rebase(&mut self) {
        self.next_due = None;
        self.behind_ms = 0.0;
    }

    /// Changes the pacing (checked by [`Pacing::check`] first).
    pub fn set_pacing(&mut self, pacing: Pacing) {
        debug_assert!(pacing.check().is_ok(), "an unchecked pacing: {pacing:?}");
        self.pacing = pacing;
        self.next_due = None;
        self.behind_ms = 0.0;
    }

    pub fn quit(&mut self) {
        self.quit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepped_runs_only_what_is_asked() {
        let mut m = TimeModel::new(TickRate::DEFAULT, Pacing::Stepped);
        assert_eq!(m.pace(0.0), Pace::WaitForCommand);
        m.step(2);
        assert_eq!(m.pace(0.0), Pace::RunTick);
        m.ran_tick(0.0);
        assert_eq!(m.pace(0.0), Pace::RunTick);
        m.ran_tick(0.0);
        assert_eq!(m.pace(0.0), Pace::WaitForCommand);
        assert_eq!(m.behind_ms(), None);
        m.quit();
        assert_eq!(m.pace(0.0), Pace::Quit);
    }

    #[test]
    fn speeds_that_are_not_above_zero_or_past_the_limit_are_refused() {
        for bad in [0.0, -4.0, f64::NAN, f64::INFINITY, 1000.5] {
            let e = Pacing::RealTime { speed: bad }.check().unwrap_err();
            assert_eq!(e.code, "request.out_of_range", "{bad}: {e:?}");
            assert_eq!(e.detail["path"], json!("/pacing/real_time/speed"), "{e:?}");
        }
        assert_eq!(
            Pacing::RealTime { speed: f64::NAN }
                .check()
                .unwrap_err()
                .detail["got"],
            json!("NaN")
        );
        for good in [0.001, 1.0, 1000.0] {
            Pacing::RealTime { speed: good }.check().unwrap();
        }
        Pacing::Stepped.check().unwrap();
    }

    #[test]
    fn real_time_waits_for_its_clock_and_drops_what_it_cannot_catch() {
        let mut m = TimeModel::new(TickRate(100), Pacing::RealTime { speed: 2.0 });
        // 100 Hz at twice real time: a tick every 5 ms.
        assert_eq!(m.pace(0.0), Pace::WaitUntil(5.0));
        assert_eq!(m.pace(5.0), Pace::RunTick);
        m.ran_tick(5.0);
        assert_eq!(m.pace(6.0), Pace::WaitUntil(10.0));
        // A stall of 100 ms, 20 ticks due: MAX_CATCH_UP of them run back to back, then the
        // excess is dropped and the next tick waits its interval (threads.md 3.3).
        for _ in 0..MAX_CATCH_UP {
            assert_eq!(m.pace(110.0), Pace::RunTick);
            m.ran_tick(110.0);
            assert!(m.behind_ms().unwrap() > 25.0);
        }
        assert_eq!(m.pace(110.0), Pace::WaitUntil(115.0));
        assert_eq!(m.behind_ms(), Some(75.0));
        // Behind by less than the limit, every due tick runs and the clock is kept.
        assert_eq!(m.pace(127.0), Pace::RunTick);
        m.ran_tick(127.0);
        assert_eq!((m.pace(127.0), m.behind_ms()), (Pace::RunTick, Some(7.0)));
        m.ran_tick(127.0);
        assert_eq!(m.pace(127.0), Pace::RunTick);
        m.ran_tick(127.0);
        assert_eq!(m.pace(127.0), Pace::WaitUntil(130.0));
        m.pause();
        assert_eq!(m.pace(200.0), Pace::WaitForCommand);
        m.step(1);
        assert_eq!(m.pace(200.0), Pace::RunTick);
        m.ran_tick(200.0);
        m.resume();
        assert_eq!(m.pace(200.0), Pace::WaitUntil(205.0));
    }
}
