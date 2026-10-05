//! The worker's time model: the slice 1 subset of `pocket_interface::TimeModel` (stepped pacing and
//! real time with pauses; threads.md 3.3), the same answers to the same questions. It is a copy
//! because architecture.md 5 gives `pocket-web` no edge to `pocket-interface` and `pocket-runtime`
//! exports the time model only with its native feature `thread` (threads-slice1.md 13); the copy
//! goes when `pocket-runtime` exports it on every target. The wall clock is read by the loop and
//! passed in as `now_ms`; nothing here reaches a tick.

use pocket_contract::codes::{Range, out_of_range};
use pocket_contract::{Pointer, Problem};
use pocket_sim::TickRate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Ticks run back to back when the game falls behind real time (threads.md 3.3, chosen).
pub const MAX_CATCH_UP: u32 = 5;

/// The fastest real-time pacing, as a multiple of real time (pocket-interface's).
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
    /// `request.out_of_range {path: "/pacing/real_time/speed"}` for a real-time speed that is not
    /// a finite number above 0 and at most [`MAX_SPEED`].
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

/// What the loop does next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pace {
    RunTick,
    /// Real time: the next tick is due at this instant of the loop's clock, in milliseconds.
    WaitUntil(f64),
    WaitForCommand,
    Quit,
}

/// The time model of one world.
#[derive(Clone, Debug)]
pub struct TimeModel {
    pacing: Pacing,
    paused: bool,
    quit: bool,
    steps_due: u64,
    tick_ms: f64,
    next_due: Option<f64>,
    behind_ms: f64,
}

impl TimeModel {
    /// A model at `rate`, paced by `pacing` (checked first), not paused.
    pub fn new(rate: TickRate, pacing: Pacing) -> TimeModel {
        TimeModel {
            pacing,
            paused: false,
            quit: false,
            steps_due: 0,
            tick_ms: 1000.0 / f64::from(rate.0),
            next_due: None,
            behind_ms: 0.0,
        }
    }

    pub fn pacing(&self) -> Pacing {
        self.pacing
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

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

    /// Whether ticks run without a further command (steps due, or real time not paused).
    pub fn running(&self) -> bool {
        self.steps_due > 0 || (!self.paused && matches!(self.pacing, Pacing::RealTime { .. }))
    }

    fn interval(&self) -> f64 {
        match self.pacing {
            Pacing::RealTime { speed } => self.tick_ms / speed,
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
            if late > f64::from(MAX_CATCH_UP) * interval {
                self.behind_ms = late;
                *due = now_ms;
            } else {
                self.behind_ms = pocket_sim::math::max(late, 0.0);
            }
        }
    }

    /// `step {ticks}`: that many more ticks, whatever the pacing.
    pub fn step(&mut self, ticks: u64) {
        self.steps_due = self.steps_due.saturating_add(ticks);
    }

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

    pub fn set_pacing(&mut self, pacing: Pacing) {
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
    fn stepped_runs_what_is_asked_and_real_time_follows_the_clock() {
        let mut m = TimeModel::new(TickRate::DEFAULT, Pacing::Stepped);
        assert_eq!(m.pace(0.0), Pace::WaitForCommand);
        m.step(2);
        assert_eq!(m.pace(0.0), Pace::RunTick);
        m.ran_tick(0.0);
        m.ran_tick(0.0);
        assert_eq!(m.pace(0.0), Pace::WaitForCommand);
        m.set_pacing(Pacing::RealTime { speed: 2.0 });
        let interval = 1000.0 / f64::from(TickRate::DEFAULT.0) / 2.0;
        assert_eq!(m.pace(100.0), Pace::WaitUntil(100.0 + interval));
        assert_eq!(m.pace(100.0 + interval), Pace::RunTick);
        m.ran_tick(100.0 + interval);
        // Far behind: the excess is dropped rather than run back to back.
        m.ran_tick(10_000.0);
        assert!(m.behind_ms().unwrap() > 0.0);
        assert_eq!(m.pace(10_000.0), Pace::RunTick);
        m.pause();
        assert_eq!(m.pace(20_000.0), Pace::WaitForCommand);
        assert!(!m.running());
        assert!(Pacing::RealTime { speed: 0.0 }.check().is_err());
        assert!(Pacing::RealTime { speed: f64::NAN }.check().is_err());
        assert!(Pacing::RealTime { speed: 8.0 }.check().is_ok());
    }
}
