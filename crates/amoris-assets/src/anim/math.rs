//! The transforms a pose is made of, in f32 with `+ - * /` and `sqrt` alone, in a fixed order, so a
//! pose has the same bits on every target (numeric.md): rotations interpolate by normalized linear
//! interpolation, never by `slerp`, and every product is written out.

/// A vector.
pub type V3 = [f32; 3];
/// A unit quaternion, x, y, z, w.
pub type Q = [f32; 4];

pub const IDENTITY: Q = [0.0, 0.0, 0.0, 1.0];

/// A joint's transform: scale, then rotation, then translation.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Xform {
    pub t: V3,
    pub r: Q,
    pub s: V3,
}

impl Default for Xform {
    fn default() -> Self {
        Xform::IDENTITY
    }
}

impl Xform {
    pub const IDENTITY: Xform = Xform {
        t: [0.0; 3],
        r: IDENTITY,
        s: [1.0; 3],
    };

    /// `self` applied after `child` (a parent's transform times its child's).
    pub fn mul(&self, child: &Xform) -> Xform {
        let scaled = [
            self.s[0] * child.t[0],
            self.s[1] * child.t[1],
            self.s[2] * child.t[2],
        ];
        Xform {
            t: add(self.t, rotate(self.r, scaled)),
            r: qmul(self.r, child.r),
            s: [
                self.s[0] * child.s[0],
                self.s[1] * child.s[1],
                self.s[2] * child.s[2],
            ],
        }
    }

    /// A point in this frame, in the parent's.
    pub fn apply(&self, p: V3) -> V3 {
        add(
            self.t,
            rotate(
                self.r,
                [self.s[0] * p[0], self.s[1] * p[1], self.s[2] * p[2]],
            ),
        )
    }

    /// The transforms `a` and `b` mixed by `u` (0 is `a`).
    pub fn mix(a: &Xform, b: &Xform, u: f32) -> Xform {
        Xform {
            t: lerp(a.t, b.t, u),
            r: nlerp(a.r, b.r, u),
            s: lerp(a.s, b.s, u),
        }
    }
}

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn lerp(a: V3, b: V3, u: f32) -> V3 {
    [
        a[0] + (b[0] - a[0]) * u,
        a[1] + (b[1] - a[1]) * u,
        a[2] + (b[2] - a[2]) * u,
    ]
}

pub fn length(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// `a` then `b`... as rotations compose: the rotation `b` first, then `a`.
pub fn qmul(a: Q, b: Q) -> Q {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// The inverse of a unit quaternion.
pub fn qconj(q: Q) -> Q {
    [-q[0], -q[1], -q[2], q[3]]
}

/// `v` rotated by the unit quaternion `q`: v + 2w (u x v) + 2 u x (u x v).
pub fn rotate(q: Q, v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    let c = cross(u, v);
    let c = [c[0] + c[0], c[1] + c[1], c[2] + c[2]];
    let d = cross(u, c);
    [
        v[0] + q[3] * c[0] + d[0],
        v[1] + q[3] * c[1] + d[1],
        v[2] + q[3] * c[2] + d[2],
    ]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `q` scaled to unit length (the identity for a zero quaternion).
pub fn normalize(q: Q) -> Q {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n > 0.0 && n.is_finite() {
        [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
    } else {
        IDENTITY
    }
}

/// Normalized linear interpolation along the shorter arc.
pub fn nlerp(a: Q, b: Q, u: f32) -> Q {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let k = if dot < 0.0 { -u } else { u };
    let v = 1.0 - u;
    normalize([
        a[0] * v + b[0] * k,
        a[1] * v + b[1] * k,
        a[2] * v + b[2] * k,
        a[3] * v + b[3] * k,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_turn_about_y_takes_x_to_minus_z() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let q = [0.0, h, 0.0, h];
        let v = rotate(q, [1.0, 0.0, 0.0]);
        assert!((v[0]).abs() < 1e-6 && (v[2] + 1.0).abs() < 1e-6, "{v:?}");
        let back = rotate(qconj(q), v);
        assert!((back[0] - 1.0).abs() < 1e-6, "{back:?}");
    }

    #[test]
    fn composing_transforms_applies_the_child_first() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let parent = Xform {
            t: [1.0, 0.0, 0.0],
            r: [0.0, h, 0.0, h],
            s: [2.0; 3],
        };
        let child = Xform {
            t: [1.0, 0.0, 0.0],
            ..Xform::IDENTITY
        };
        let g = parent.mul(&child);
        // The child's offset, doubled and turned to -z, from the parent's place.
        assert!(
            (g.t[0] - 1.0).abs() < 1e-6 && (g.t[2] + 2.0).abs() < 1e-6,
            "{:?}",
            g.t
        );
        let p = parent.apply([1.0, 0.0, 0.0]);
        assert_eq!(p, g.t);
    }

    #[test]
    fn nlerp_takes_the_shorter_arc_and_stays_unit() {
        let a = IDENTITY;
        let b = [0.0, 0.0, 0.0, -1.0]; // the same rotation, the other sign
        let m = nlerp(a, b, 0.5);
        assert!((m[3].abs() - 1.0).abs() < 1e-6, "{m:?}");
    }
}
