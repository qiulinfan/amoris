//! Perception's geometry (shared/contract/perception.md, Geometry), expression by expression as the
//! contract writes it, with the deterministic math library, so a value at the edge of a range or a
//! field of view falls on the same side on every target and on both lines.

use bevy_ecs::prelude::World;
use pocket_physics::Transform;
use pocket_physics::geom::{self, V3};
use pocket_physics::query::cast_ray_among;
use pocket_sim::{EntityId, EntityIndex, math};

use super::defs::Sight;
use super::state::Occluder;

/// The bearing of a ground offset: `atan2(dx, -dz)` in degrees, normalized to [0, 360); 0 for a
/// zero offset.
pub fn bearing_deg(dx: f64, dz: f64) -> f64 {
    geom::bearing_deg(dx, dz)
}

/// The relative angle of bearing `b` to heading `h`, in (-180, 180].
pub fn relative_deg(b: f64, h: f64) -> f64 {
    let a = b - h;
    if a > 180.0 {
        a - 360.0
    } else if a <= -180.0 {
        a + 360.0
    } else {
        a
    }
}

/// `dx*dx + dy*dy + dz*dz <= r*r`, the sum in that order.
pub fn within(d: V3, r: f64) -> bool {
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r * r
}

/// The horizontal bearing and range from `from` to `to` (perception.md, Geometry: reported
/// measures are from the body's origin).
pub fn bearing_range(from: V3, to: V3) -> (f64, f64) {
    let dx = to[0] - from[0];
    let dz = to[2] - from[2];
    (bearing_deg(dx, dz), math::sqrt(dx * dx + dz * dz))
}

/// The heading of a body: the bearing of its forward axis (-z) projected on the ground.
pub fn heading_deg(tf: &Transform) -> f64 {
    let f = geom::rotate(tf.rotation, [0.0, 0.0, -1.0]);
    bearing_deg(f[0], f[2])
}

/// Where an observer looks from: the eye in the world and the body's heading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eye {
    pub at: V3,
    pub heading: f64,
}

impl Eye {
    pub fn of(tf: &Transform, sight: &Sight) -> Eye {
        Eye {
            at: geom::to_world(tf.position, tf.rotation, sight.eye_m.array()),
            heading: heading_deg(tf),
        }
    }
}

/// Steps 2 to 4 of the perception update for a point `p` with the range `r` (already scaled by the
/// visibility): in range, in the field of view, and (with occlusion) at least one of `samples`
/// in clear sight. `exclude` are the observer's body and the target, which never occlude.
pub fn sees(
    world: &World,
    eye: &Eye,
    sight: &Sight,
    r: f64,
    p: V3,
    samples: &[V3],
    exclude: [Option<EntityId>; 2],
) -> bool {
    let d = geom::sub(p, eye.at);
    if !within(d, r) {
        return false;
    }
    if sight.fov_deg < 360.0 {
        let b = bearing_deg(d[0], d[2]);
        if relative_deg(b, eye.heading).abs() > sight.fov_deg / 2.0 {
            return false;
        }
    }
    !sight.occlusion || samples.iter().any(|s| clear(world, eye.at, *s, exclude))
}

/// Whether the segment from `from` to `to` meets no occluder's collider other than `exclude`.
pub fn clear(world: &World, from: V3, to: V3, exclude: [Option<EntityId>; 2]) -> bool {
    let d = geom::sub(to, from);
    let len = math::sqrt(geom::dot(d, d));
    if len <= 0.0 {
        return true;
    }
    let Some(index) = world.get_resource::<EntityIndex>() else {
        return true;
    };
    let accept = |id: EntityId| {
        !exclude.contains(&Some(id))
            && index
                .get(id)
                .is_some_and(|e| world.get::<Occluder>(e).is_some())
    };
    // A segment the solver cannot hold (an end past ±3.4e38, a length that overflows) is refused
    // by the ray, and a sight line that could not be tested is not clear: nothing is seen through it.
    matches!(
        cast_ray_among(world, from, geom::scale(d, 1.0 / len), len, &accept),
        Ok(None)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearings_and_angles() {
        assert_eq!(bearing_deg(0.0, 0.0), 0.0);
        assert_eq!(bearing_deg(1.0, 0.0), 90.0);
        assert_eq!(bearing_deg(0.0, 1.0), 180.0);
        assert_eq!(bearing_deg(-1.0, 0.0), 270.0);
        // atan2 of -0 lands on -0, which is not below 0: the bearing stays -0 and rounds to 0.
        let b = bearing_deg(-0.0, -1.0);
        assert_eq!(b, 0.0);
        // Just west of north lands just below 360.
        let b = bearing_deg(-1e-9, -1.0);
        assert!(b < 360.0 && b > 359.9, "{b}");
        assert_eq!(relative_deg(10.0, 350.0), 20.0);
        assert_eq!(relative_deg(350.0, 10.0), -20.0);
        assert_eq!(relative_deg(180.0, 0.0), 180.0);
        assert_eq!(relative_deg(0.0, 180.0), 180.0);
        assert_eq!(relative_deg(0.0, 179.0), -179.0);
    }

    /// The fixes review's probe: the occlusion ray met nothing (`None`) with an argument it
    /// refused, so a sight line from a NaN eye counted as clear. A ray that could not be cast
    /// is refused, and the sight line is not clear.
    #[test]
    fn a_sight_line_the_ray_cannot_test_is_not_clear() {
        let mut world = World::new();
        world.insert_resource(EntityIndex::default());
        let none = [None, None];
        assert!(clear(&world, [0.0; 3], [0.0, 0.0, 10.0], none));
        assert!(!clear(&world, [f64::NAN, 0.0, 0.0], [0.0, 0.0, 10.0], none));
        assert!(!clear(&world, [0.0; 3], [0.0, 1e39, 0.0], none));
    }

    #[test]
    fn range_edges() {
        // Exactly r*r is in range; one ulp beyond is not.
        let r = 120.0;
        assert!(within([120.0, 0.0, 0.0], r));
        assert!(!within([f64::from_bits(120f64.to_bits() + 1), 0.0, 0.0], r));
        assert!(within([72.0, 0.0, 96.0], r));
    }
}
