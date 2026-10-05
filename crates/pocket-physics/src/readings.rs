//! What a boat reads after the step (shared/contract/sailing.md, the skipper's instruments): its
//! speed through the water along its heading, its heading, its heel, whether it is aground and
//! whether it is afloat. The sail's readings (apparent wind, boom, drive, trim) are written by
//! `physics.sail`.

use crate::boat::{Boat, Floater};
use crate::body::{Transform, Velocity};
use crate::geom;
use crate::sea::WaveField;

/// The boat's forward direction on the ground and its starboard side, from its rotation.
pub fn ahead_and_starboard(rotation: geom::Q4) -> Option<(geom::V3, geom::V3)> {
    let fwd = geom::rotate(rotation, [0.0, 0.0, -1.0]);
    let flat = pocket_sim::math::sqrt(fwd[0] * fwd[0] + fwd[2] * fwd[2]);
    if flat <= 1e-9 {
        return None;
    }
    let ahead = [fwd[0] / flat, 0.0, fwd[2] / flat];
    Some((ahead, [-ahead[2], 0.0, ahead[0]]))
}

/// Writes `speed`, `heading_deg`, `heel_deg`, `aground` and `afloat` from the boat's state after
/// the step. `aground`: the step left it touching land (a fixed body). `afloat` (the contract's "in
/// the water and not aground"): some but not all of its floating volume was under water at the
/// start of the tick, and it is not aground.
pub fn after_step(
    boat: &mut Boat,
    tf: &Transform,
    v: &Velocity,
    floater: Option<&Floater>,
    sea: Option<&WaveField>,
    time: f64,
    aground: bool,
) {
    let up = geom::rotate(tf.rotation, [0.0, 1.0, 0.0]);
    if let Some((ahead, starboard)) = ahead_and_starboard(tf.rotation) {
        let water = sea.map_or(geom::ZERO, |s| {
            s.at(tf.position[0], tf.position[2], time).velocity
        });
        boat.speed = geom::dot(geom::sub(v.linear, water), ahead);
        boat.heading_deg = geom::bearing_deg(ahead[0], ahead[2]);
        boat.heel_deg = pocket_sim::math::atan2(geom::dot(up, starboard), up[1]).to_degrees();
    }
    boat.aground = aground;
    let in_water = floater.is_some_and(|f| f.submerged > 0.0 && f.submerged < 1.0);
    boat.afloat = sea.is_some() && in_water && !aground;
}
