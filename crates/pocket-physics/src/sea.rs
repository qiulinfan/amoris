//! The sea and the wind (charter 2.4.1, 4.3): their plain-data components and the fields they
//! define. The water's surface and velocity and the wind's velocity are pure functions of the
//! point, the simulated time and the components' parameters, built from `pocket_sim::math`, so
//! every target computes the same field at the same tick; they are never stored (Derived,
//! persistence.md 8).
//!
//! One sea and one wind act at a time: the `Sea` and the `Wind` with the lowest `EntityId`.

use bevy_ecs::prelude::Component;
use pocket_sim::math::{self, PI, TAU};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::geom::{self, V3};

/// Standard gravity, m/s^2: the solver's and the waves'.
pub const GRAVITY: f64 = 9.81;

/// One sine wave of the sea, running at the deep-water speed of its length.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Wave {
    /// Crest to crest, metres (above 0).
    pub length: f64,
    /// Crest to trough, metres.
    pub height: f64,
    /// The bearing the crests travel toward, degrees (0 toward -z, 90 toward +x).
    pub toward_deg: f64,
    /// Phase at time 0 and the origin, radians.
    pub phase: f64,
}

/// A body of water to the horizon: its level at rest, its density (kg/m^3) and its waves.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Sea {
    /// The surface at rest, metres.
    pub level: f64,
    /// kg/m^3.
    pub density: f64,
    pub waves: Vec<Wave>,
}

impl Default for Sea {
    fn default() -> Self {
        Sea {
            level: 0.0,
            density: 1000.0,
            waves: Vec::new(),
        }
    }
}

impl Sea {
    /// Master's island sea (`samples/island`): a swell 9 long and 0.35 high running 10 degrees
    /// south of east, and three smaller waves crossing it, at the given density.
    pub fn island(density: f64) -> Sea {
        let parts: [(f64, f64, f64, f64); 4] = [
            (1.0, 1.0, 0.0, 0.0),
            (0.62, 0.5, 26.0, 1.3),
            (0.38, 0.28, -34.0, 2.1),
            (0.24, 0.16, 63.0, 4.0),
        ];
        Sea {
            level: 0.0,
            density,
            waves: parts
                .iter()
                .map(|&(len, height, turn, phase)| Wave {
                    length: 9.0 * len,
                    height: 0.35 * height,
                    toward_deg: 100.0 + turn,
                    phase,
                })
                .collect(),
        }
    }

    /// The field with each wave's constants worked out once.
    pub fn field(&self) -> WaveField {
        WaveField {
            level: self.level,
            density: self.density,
            waves: self
                .waves
                .iter()
                .filter(|w| w.length > 0.0)
                .map(|w| {
                    let k = TAU / w.length;
                    let d = geom::from_bearing(w.toward_deg);
                    WaveConst {
                        amplitude: 0.5 * w.height,
                        k,
                        dx: d[0],
                        dz: d[2],
                        omega: math::sqrt(GRAVITY * k),
                        phase: w.phase,
                    }
                })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct WaveConst {
    amplitude: f64,
    k: f64,
    dx: f64,
    dz: f64,
    omega: f64,
    phase: f64,
}

/// The water at one point: the surface height there and the water's velocity at the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Water {
    pub height: f64,
    pub velocity: V3,
}

/// A sea's surface as a function of the point and the time.
#[derive(Clone, Debug)]
pub struct WaveField {
    pub level: f64,
    pub density: f64,
    waves: Vec<WaveConst>,
}

impl WaveField {
    /// Height and velocity over (x, z) at time `t`. For a wave `a sin(theta)` with
    /// `theta = k (d . p) - omega t + phase`, the surface water moves `a omega sin(theta)` along d
    /// and `-a omega cos(theta)` up.
    pub fn at(&self, x: f64, z: f64, t: f64) -> Water {
        let mut w = Water {
            height: self.level,
            velocity: geom::ZERO,
        };
        for c in &self.waves {
            let theta = c.k * (c.dx * x + c.dz * z) - c.omega * t + c.phase;
            let (s, co) = math::sin_cos(theta);
            w.height += c.amplitude * s;
            let h = c.amplitude * c.omega * s;
            w.velocity[0] += h * c.dx;
            w.velocity[2] += h * c.dz;
            w.velocity[1] -= c.amplitude * c.omega * co;
        }
        w
    }
}

/// The true wind: from a bearing at a speed, swelling and easing by `gust` of itself in gusts that
/// sweep downwind, `gust_length` metres apart, once every `gust_period` seconds at a point.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Wind {
    /// Where the wind blows from, degrees (270: a westerly, blowing toward +x).
    pub from_deg: f64,
    /// m/s.
    pub speed: f64,
    /// The gusts' share of the speed, 0 for a steady wind.
    pub gust: f64,
    /// Seconds between gusts at a point (above 0).
    pub gust_period: f64,
    /// Metres between gusts along the wind (above 0).
    pub gust_length: f64,
}

impl Default for Wind {
    fn default() -> Self {
        Wind {
            from_deg: 270.0,
            speed: 6.0,
            gust: 0.0,
            gust_period: 20.0,
            gust_length: 120.0,
        }
    }
}

impl Wind {
    /// A steady wind from `from_deg` at `speed`.
    pub fn steady(from_deg: f64, speed: f64) -> Wind {
        Wind {
            from_deg,
            speed,
            ..Wind::default()
        }
    }

    /// The wind's velocity at `p` at time `t`.
    pub fn at(&self, p: V3, t: f64) -> V3 {
        let toward = geom::from_bearing(self.from_deg + 180.0);
        let mut speed = self.speed;
        if self.gust != 0.0 && self.gust_period > 0.0 && self.gust_length > 0.0 {
            let along = geom::dot(toward, p);
            let phase = 2.0 * PI * (t / self.gust_period - along / self.gust_length);
            speed *= 1.0 + self.gust * math::sin(phase);
        }
        geom::scale(toward, speed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fields() {
        let calm = Sea::default().field();
        assert_eq!(calm.at(3.0, 4.0, 5.0).height, 0.0);
        let sea = Sea::island(2.0).field();
        let a = sea.at(1.0, 2.0, 3.0);
        assert!(a.height.abs() < 0.35 && a.height != 0.0);
        assert_eq!(a, sea.at(1.0, 2.0, 3.0));
        let w = Wind::steady(270.0, 6.0).at([5.0, 0.0, 5.0], 1.0);
        assert!((w[0] - 6.0).abs() < 1e-12 && w[2].abs() < 1e-12, "{w:?}");
        let gusty = Wind {
            gust: 0.2,
            ..Wind::steady(270.0, 6.0)
        };
        let s = geom::length(gusty.at([0.0; 3], 5.0));
        assert!(s > 4.7 && s < 7.3);
    }
}
