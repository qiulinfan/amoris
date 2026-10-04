//! Simulated time (docs/spec/simulation.md 3): the tick count, the fixed rate and the clock derived
//! from them. Time is never accumulated, so it is the same on every target and never drifts.

use bevy_ecs::prelude::Resource;
use pocket_contract::{Problem, detail};
use schemars::JsonSchema;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The number of ticks the world has completed; `Tick(0)` is the world as loaded. At most
/// 2^53 - 1, so it crosses to TypeScript as an exact number (decoding refuses more).
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, JsonSchema,
)]
pub struct Tick(pub u64);

impl<'de> Deserialize<'de> for Tick {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Tick, D::Error> {
        /// The encoded shape, `Tick`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "Tick")]
        struct Repr(u64);
        let Repr(n) = Repr::deserialize(d)?;
        if n > Tick::MAX.0 {
            return Err(de::Error::custom(format!(
                "tick {n} is past the last tick, 2^53 - 1"
            )));
        }
        Ok(Tick(n))
    }
}

impl Tick {
    /// The last tick a world can reach: 2^53 - 1.
    pub const MAX: Tick = Tick((1 << 53) - 1);

    /// The tick as a double (exact below 2^53).
    #[allow(clippy::cast_precision_loss)] // ticks stay below 2^53 (simulation.md 3.1)
    pub fn to_f64(self) -> f64 {
        self.0 as f64
    }
}

/// Ticks per simulated second, fixed for a world's whole history: 1 to 1000, projects default 60.
/// Decoding refuses a rate outside that range with `sim.tick_rate_invalid`'s message.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, JsonSchema)]
pub struct TickRate(pub u32);

impl<'de> Deserialize<'de> for TickRate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<TickRate, D::Error> {
        /// The encoded shape, `TickRate`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "TickRate")]
        struct Repr(u32);
        let Repr(rate) = Repr::deserialize(d)?;
        TickRate::new(rate).map_err(de::Error::custom)
    }
}

impl TickRate {
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 1000;
    /// The showcase's rate, master's and Rapier's default step.
    pub const DEFAULT: TickRate = TickRate(60);

    /// A rate, or `sim.tick_rate_invalid` outside 1..=1000.
    pub fn new(rate: u32) -> Result<TickRate, Problem> {
        if (Self::MIN..=Self::MAX).contains(&rate) {
            Ok(TickRate(rate))
        } else {
            Err(Problem::new(
                "sim.tick_rate_invalid",
                format!("A tick rate must be from 1 to 1000 ticks a second; got {rate}."),
                detail([
                    ("rate", json!(rate)),
                    ("min", json!(Self::MIN)),
                    ("max", json!(Self::MAX)),
                ]),
            ))
        }
    }
}

/// The simulation clock, a persisted resource: the tick the world shows and the rate. During
/// tick n it already reads `Tick(n)` (`sim.begin` advances it first).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SimClock {
    pub tick: Tick,
    pub rate: TickRate,
}

impl SimClock {
    /// The step length in seconds: `1.0 / rate`, correctly rounded, the same on every target.
    pub fn dt(&self) -> f64 {
        1.0 / f64::from(self.rate.0)
    }

    /// Simulated seconds at the end of the current tick: `tick / rate`, one exact conversion and
    /// one correctly rounded division.
    pub fn time(&self) -> f64 {
        self.tick.to_f64() / f64::from(self.rate.0)
    }
}

/// When a system runs (simulation.md 4.5). It depends on the tick number alone, so a hot update
/// at tick 500 does not rerun a `Start` system and reloading unchanged scripts changes nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub enum RunCondition {
    /// Every tick.
    #[default]
    Always,
    /// Ticks n with n % period == offset.
    Every { period: u64, offset: u64 },
    /// Tick 1 only.
    Start,
}

impl RunCondition {
    /// `Every { period, offset }` with period at least 1 and offset below it, else
    /// `sim.run_condition_invalid`.
    pub fn every(period: u64, offset: u64) -> Result<RunCondition, Problem> {
        if period >= 1 && offset < period {
            Ok(RunCondition::Every { period, offset })
        } else {
            Err(Problem::new(
                "sim.run_condition_invalid",
                format!(
                    "A run condition needs a period of at least 1 and an offset below it; got \
                     period {period}, offset {offset}."
                ),
                detail([("period", json!(period)), ("offset", json!(offset))]),
            ))
        }
    }

    /// Whether the system runs on tick `tick`.
    pub fn runs_on(&self, tick: Tick) -> bool {
        match *self {
            RunCondition::Always => true,
            RunCondition::Every { period, offset } => period != 0 && tick.0 % period == offset,
            RunCondition::Start => tick.0 == 1,
        }
    }

    /// The schedule listing's form: `always`, `every 4 + 1`, `start`.
    pub fn label(&self) -> String {
        match *self {
            RunCondition::Always => "always".into(),
            RunCondition::Every { period, offset } => format!("every {period} + {offset}"),
            RunCondition::Start => "start".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_is_derived_exactly() {
        let c = SimClock {
            tick: Tick(216_000),
            rate: TickRate::DEFAULT,
        };
        assert_eq!(c.time(), 3600.0);
        assert_eq!(c.dt(), 1.0 / 60.0);
        assert_eq!(TickRate::new(0).unwrap_err().code, "sim.tick_rate_invalid");
        assert_eq!(TickRate::new(1001).unwrap_err().detail["max"], json!(1000));
        assert!(TickRate::new(1).is_ok() && TickRate::new(1000).is_ok());
    }

    #[test]
    fn run_conditions_fire_on_their_ticks() {
        let every = RunCondition::every(4, 1).unwrap();
        let fired: Vec<u64> = (1..=12).filter(|&n| every.runs_on(Tick(n))).collect();
        assert_eq!(fired, [1, 5, 9]);
        let start: Vec<u64> = (1..=5)
            .filter(|&n| RunCondition::Start.runs_on(Tick(n)))
            .collect();
        assert_eq!(start, [1]);
        assert!((1..=5).all(|n| RunCondition::Always.runs_on(Tick(n))));
        assert!(RunCondition::every(0, 0).is_err());
        assert!(RunCondition::every(3, 3).is_err());
    }
}
