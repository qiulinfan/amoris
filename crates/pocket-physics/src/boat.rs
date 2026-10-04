//! The showcase's floating and sailing components (charter 2.4.1, 4.3): what floats, the boat's
//! controls and the instruments it reads, its sail, and its hull's foils. The forces they give are
//! the systems of `forces`; what shared/contract/sailing.md asks of the sailing simulation (the
//! actuator rates, `best_sheet`, the no-go and close-hauled angles, `afloat`, `aground` and `trim`)
//! is here.
//!
//! The boat's frame is master's: forward is -z, up +y, starboard +x.

use bevy_ecs::prelude::Component;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::geom::V3;

/// The rudder blade follows its control at this many units (of -1..1) a second.
pub const RUDDER_RATE: f64 = 2.0;
/// The sheet follows its control at this many units (of 0..1) a second.
pub const SHEET_RATE: f64 = 0.5;
/// The sail goes up or down at this many units (of 0..1) a second.
pub const HOIST_RATE: f64 = 1.0;
/// The boom's angle from the centreline with the sheet eased right out (sheet 1), degrees.
pub const BOOM_MAX_DEG: f64 = 85.0;
/// Within this many degrees of the apparent wind's eye the sail draws nothing.
pub const NO_GO_HALF_DEG: f64 = 45.0;
/// The sail's drive comes back over this many degrees beyond the no-go angle.
pub const NO_GO_RAMP_DEG: f64 = 10.0;
/// The heading a boat sails closest to the wind at, degrees off the true wind.
pub const CLOSE_HAULED_DEG: f64 = 50.0;
/// An angle of attack below this many degrees luffs; above `STALL_DEG` the sail is overtrimmed.
pub const LUFF_DEG: f64 = 3.0;
pub const STALL_DEG: f64 = 40.0;

/// The sheet that sets the boom at about half the apparent wind angle, the usual trim rule
/// (shared/contract/sailing.md, Controls): `clamp((|awa| / 2 - 5) / 80, 0, 1)`.
pub fn best_sheet(awa_deg: f64) -> f64 {
    pocket_sim::math::clamp((awa_deg.abs() / 2.0 - 5.0) / 80.0, 0.0, 1.0)
}

/// A point that displaces water: where it is in the body's frame, the volume it stands for (m^3)
/// and the height over which it goes from dry to fully under (metres).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuoyPoint {
    pub at: V3,
    pub volume: f64,
    pub size: f64,
}

/// A body the sea holds up and drags: Archimedes at each point under the surface, and drag toward
/// the water's own motion there (`drag` along the surface, `heave` up and down, per kilogram of
/// water displaced and m/s of relative speed). `submerged` is the share of the points' volume
/// under water at the start of the last tick (written by `physics.buoyancy`).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Floater {
    pub points: Vec<BuoyPoint>,
    pub drag: f64,
    pub heave: f64,
    /// Written by the engine: 0 dry, 1 wholly under.
    pub submerged: f64,
}

impl Default for Floater {
    fn default() -> Self {
        Floater {
            points: Vec::new(),
            drag: 1.0,
            heave: 6.0,
            submerged: 0.0,
        }
    }
}

impl Floater {
    /// 27 points across a box of half extents `h` (master's cells), each an equal share of its
    /// volume.
    pub fn solid_box(h: V3) -> Floater {
        let volume = 8.0 * h[0] * h[1] * h[2] / 27.0;
        let third = 2.0 / 3.0;
        let mut points = Vec::with_capacity(27);
        for i in [-1.0, 0.0, 1.0] {
            for j in [-1.0, 0.0, 1.0] {
                for k in [-1.0, 0.0, 1.0] {
                    points.push(BuoyPoint {
                        at: [h[0] * i * third, h[1] * j * third, h[2] * k * third],
                        volume,
                        size: h[1] * third,
                    });
                }
            }
        }
        Floater {
            points,
            ..Floater::default()
        }
    }

    /// The points' whole volume, m^3.
    pub fn volume(&self) -> f64 {
        self.points.iter().map(|p| p.volume).sum()
    }
}

/// How the sail is trimmed, from its angle of attack (shared/contract/sailing.md, instrument
/// `trim`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Trim {
    #[default]
    Furled,
    /// Eased too far: the sail streams with the wind and draws little.
    Luffing,
    Good,
    /// In too far: the sail stalls.
    Overtrimmed,
}

/// A boat: its controls, the actuators that follow them, and what it reads. Scripts, intents and
/// boundary writes set the controls; the engine moves the actuators at `HOIST_RATE`, `SHEET_RATE`
/// and `RUDDER_RATE` and writes the readings.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Boat {
    /// Control, 0..1: how much sail to set; 0 furled.
    pub hoist: f64,
    /// Control, 0..1: 0 hard in (the boom on the centreline), 1 eased right out.
    pub sheet: f64,
    /// Control, -1..1: positive turns the bow to starboard.
    pub rudder: f64,
    /// The sail as set now (follows `hoist`).
    pub hoist_now: f64,
    /// The sheet as eased now (follows `sheet`).
    pub sheet_now: f64,
    /// The rudder blade now (follows `rudder`).
    pub rudder_now: f64,
    /// Written by the engine: speed through the water along the heading, m/s.
    pub speed: f64,
    /// Written by the engine: where the bow points, degrees (0 toward -z, 90 toward +x).
    pub heading_deg: f64,
    /// Written by the engine: positive heeled to starboard, degrees.
    pub heel_deg: f64,
    /// Written by the engine: the hull is in the water, not sunk beneath it, and not aground.
    pub afloat: bool,
    /// Written by the engine: the boat touches land (a fixed body) after the step.
    pub aground: bool,
    /// Written by the engine: apparent wind angle off the bow, positive over the starboard side.
    pub awa_deg: f64,
    /// Written by the engine: apparent wind speed, m/s.
    pub aws: f64,
    /// Written by the engine: the boom's angle from the centreline, positive to starboard.
    pub boom_deg: f64,
    /// Written by the engine: the sail's drive as a share of the best at this apparent wind.
    pub drive: f64,
    /// Written by the engine.
    pub trim: Trim,
}

impl Default for Boat {
    fn default() -> Self {
        Boat {
            hoist: 0.0,
            sheet: 1.0,
            rudder: 0.0,
            hoist_now: 0.0,
            sheet_now: 1.0,
            rudder_now: 0.0,
            speed: 0.0,
            heading_deg: 0.0,
            heel_deg: 0.0,
            afloat: false,
            aground: false,
            awa_deg: 0.0,
            aws: 0.0,
            boom_deg: 0.0,
            drive: 0.0,
            trim: Trim::Furled,
        }
    }
}

impl Boat {
    /// A boat with its sail set and the sheet eased out, the actuators already there.
    pub fn sail_set() -> Boat {
        Boat {
            hoist: 1.0,
            hoist_now: 1.0,
            ..Boat::default()
        }
    }
}

/// The boat's sail: a flat plate on a boom pivoting at the mast, whose force is
/// `coef (v . n) |v . n|` along its normal n for the apparent wind v across it, reaching its
/// centre of effort `boom / 2` along the boom and `rise` above the mast's foot. Positions are in
/// the boat's frame.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Sail {
    /// The mast's foot, metres in the boat's frame.
    pub mast: V3,
    /// The boom's length, metres.
    pub boom: f64,
    /// N per (m/s)^2.
    pub coef: f64,
    /// The centre of effort above the mast's foot, metres.
    pub rise: f64,
    /// Written by `physics.wind`: the true wind at the mast this tick, m/s.
    pub wind: V3,
}

impl Default for Sail {
    fn default() -> Self {
        Sail {
            mast: [0.0, 0.35, -0.95],
            boom: 1.4,
            coef: 70.0,
            rise: 0.8,
            wind: [0.0; 3],
        }
    }
}

/// The hull's foils and resistance: a keel fin and a rudder blade, flat plates in the water's flow
/// (`coef (v . n) |v . n|` along their normals), and quadratic drag along the hull. Positions are
/// in the boat's frame.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Hull {
    pub keel_at: V3,
    /// N per (m/s)^2.
    pub keel_coef: f64,
    pub rudder_at: V3,
    /// N per (m/s)^2.
    pub rudder_coef: f64,
    /// The blade's angle at full rudder, degrees.
    pub rudder_max_deg: f64,
    /// N per (m/s)^2 along the hull.
    pub drag_coef: f64,
}

impl Default for Hull {
    fn default() -> Self {
        Hull {
            keel_at: [0.0, -0.55, 0.0],
            keel_coef: 4000.0,
            rudder_at: [0.0, -0.3, 1.78],
            rudder_coef: 2500.0,
            rudder_max_deg: 34.0,
            drag_coef: 40.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_rule_and_box() {
        assert_eq!(best_sheet(0.0), 0.0);
        assert_eq!(best_sheet(90.0), 0.5);
        assert_eq!(best_sheet(-180.0), 1.0);
        let f = Floater::solid_box([0.3; 3]);
        assert_eq!(f.points.len(), 27);
        assert!((f.volume() - 0.216).abs() < 1e-12);
    }
}
