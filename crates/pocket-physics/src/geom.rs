//! Vectors and quaternions in `f64` for the force systems, and the one place where values cross
//! between the `f64` components and the `f32` solver (docs/spec/numeric.md 3.1): `as f32` going in
//! (IEEE round to nearest, ties to even) and `as f64` coming out (exact).
//!
//! Every operation here is `+ - * /` and `sqrt` in a fixed order, so it gives the same bits on every
//! target. Vectors are `[f64; 3]`, quaternions `[x, y, z, w]`.

use rapier3d::math::{Pose, Rotation, Vector};

pub type V3 = [f64; 3];
pub type Q4 = [f64; 4];

pub const ZERO: V3 = [0.0; 3];
pub const IDENTITY: Q4 = [0.0, 0.0, 0.0, 1.0];

#[inline]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
pub fn length(a: V3) -> f64 {
    pocket_sim::math::sqrt(dot(a, a))
}

/// `v` rotated by the unit quaternion `q`: `v + 2w (u x v) + 2 u x (u x v)` with `u = (x, y, z)`.
#[inline]
pub fn rotate(q: Q4, v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    let t = scale(cross(u, v), 2.0);
    add(add(v, scale(t, q[3])), cross(u, t))
}

/// `v` rotated by the inverse of the unit quaternion `q` (into the body's frame).
#[inline]
pub fn rotate_inv(q: Q4, v: V3) -> V3 {
    rotate([-q[0], -q[1], -q[2], q[3]], v)
}

/// A point of the body's frame in the world: `position + rotate(rotation, local)`.
#[inline]
pub fn to_world(position: V3, rotation: Q4, local: V3) -> V3 {
    add(position, rotate(rotation, local))
}

/// The ground bearing of a direction (degrees, 0 toward -z, 90 toward +x), as
/// shared/contract/perception.md (Geometry) computes it: `atan2(dx, -dz)` in [0, 360).
pub fn bearing_deg(dx: f64, dz: f64) -> f64 {
    if dx == 0.0 && dz == 0.0 {
        return 0.0;
    }
    let mut b = pocket_sim::math::atan2(dx, -dz).to_degrees();
    if b < 0.0 {
        b += 360.0;
    }
    if b >= 360.0 {
        b -= 360.0;
    }
    b
}

/// The ground direction of a bearing (degrees): `(sin b, 0, -cos b)`.
pub fn from_bearing(deg: f64) -> V3 {
    let (s, c) = pocket_sim::math::sin_cos(deg.to_radians());
    [s, 0.0, -c]
}

/// `x` to the solver's `f32`, rounded to nearest (numeric.md 3.1).
#[inline]
#[allow(clippy::cast_possible_truncation)] // the one rounding into the solver, by design
pub fn f32_of(x: f64) -> f32 {
    x as f32
}

#[inline]
pub fn vec_in(v: V3) -> Vector {
    Vector::new(f32_of(v[0]), f32_of(v[1]), f32_of(v[2]))
}

#[inline]
pub fn vec_out(v: Vector) -> V3 {
    [f64::from(v.x), f64::from(v.y), f64::from(v.z)]
}

#[inline]
pub fn quat_in(q: Q4) -> Rotation {
    Rotation::from_xyzw(f32_of(q[0]), f32_of(q[1]), f32_of(q[2]), f32_of(q[3]))
}

#[inline]
pub fn quat_out(q: Rotation) -> Q4 {
    [
        f64::from(q.x),
        f64::from(q.y),
        f64::from(q.z),
        f64::from(q.w),
    ]
}

#[inline]
pub fn pose_in(position: V3, rotation: Q4) -> Pose {
    Pose::from_parts(vec_in(position), quat_in(rotation))
}

/// The quaternion of a turn by `deg` degrees about +y (a yaw; positive turns +x toward -z).
pub fn yaw(deg: f64) -> Q4 {
    let (s, c) = pocket_sim::math::sin_cos(deg.to_radians() * 0.5);
    [0.0, s, 0.0, c]
}

/// Whether every component is finite.
pub fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// Whether every component stays finite as the solver receives it, rounded to `f32` (1e39 is
/// finite in `f64` and infinite in `f32`).
pub fn finite32(v: &[f64]) -> bool {
    v.iter().all(|x| f32_of(*x).is_finite())
}

/// `q` scaled to unit length, or `None` when it is not finite or its length is 0 or overflows.
pub fn normalized(q: Q4) -> Option<Q4> {
    let n = pocket_sim::math::sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
    (n.is_finite() && n > 0.0 && finite(&q)).then(|| [q[0] / n, q[1] / n, q[2] / n, q[3] / n])
}

/// A count as a double (exact below 2^53).
#[inline]
#[allow(clippy::cast_precision_loss)] // counts of points and bodies stay far below 2^53
pub fn f64_of(n: usize) -> f64 {
    n as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_and_bearings() {
        let q = yaw(90.0);
        let v = rotate(q, [0.0, 0.0, -1.0]);
        assert!((v[0] + 1.0).abs() < 1e-15 && v[2].abs() < 1e-15, "{v:?}");
        let back = rotate_inv(q, v);
        assert!((back[2] + 1.0).abs() < 1e-15);
        assert_eq!(bearing_deg(1.0, 0.0), 90.0);
        assert_eq!(bearing_deg(0.0, -1.0), 0.0);
        assert_eq!(bearing_deg(-1.0, 0.0), 270.0);
        assert_eq!(bearing_deg(0.0, 0.0), 0.0);
        let d = from_bearing(90.0);
        assert!((d[0] - 1.0).abs() < 1e-15 && d[2].abs() < 1e-15);
        assert_eq!(f32_of(0.1), 0.1f32);
        assert_eq!(vec_out(vec_in([1.5, -2.0, 0.25])), [1.5, -2.0, 0.25]);
        assert!(finite32(&[1e38, -1e38]) && !finite32(&[1e39]) && !finite32(&[f64::NAN]));
        assert_eq!(normalized([0.0, 2.0, 0.0, 0.0]), Some([0.0, 1.0, 0.0, 0.0]));
        assert_eq!(normalized([1e200, 0.0, 0.0, 0.0]), None);
        assert_eq!(normalized([0.0; 4]), None);
    }
}
