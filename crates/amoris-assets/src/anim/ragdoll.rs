//! Ragdolls (animation.md 6b): a graph file's capsule bodies on chosen bones with the joints
//! between them (`RagdollDef`), and the same bound to the skeleton's rest pose (`RagdollRig`):
//! each body's frame relative to its bone, its parent body, and its joint's anchors, rest turn,
//! axis and limits, in the model's space. The physics crate makes the bodies and reads them back.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::clip::Skeleton;
use super::ik::{from_to, unit};
use super::math::{self, Q, V3, Xform};

/// One body of a graph's ragdoll, as the file gives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RagdollBodyDef {
    pub joint: String,
    /// The joint the capsule reaches (in the rest pose); none: `length` along `axis`.
    pub to: Option<String>,
    pub length: f64,
    pub axis: [f64; 3],
    pub radius: f64,
    pub mass: f64,
    /// A revolute joint's axis in the bone's frame; none: a spherical joint.
    pub hinge: Option<[f64; 3]>,
    /// A hinge's limits, degrees.
    pub limits_deg: [f64; 2],
    pub swing_deg: f64,
    pub twist_deg: [f64; 2],
}

/// A graph file's `ragdoll`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RagdollDef {
    pub bodies: Vec<RagdollBodyDef>,
    pub friction: f64,
    pub linear_damping: f64,
    pub angular_damping: f64,
    pub groups: u32,
    pub mask: u32,
}

fn num(v: &Value, what: &str) -> Result<f64, String> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("{what} must be a finite number"))
}

fn opt(
    o: &serde_json::Map<String, Value>,
    key: &str,
    default: f64,
    what: &str,
) -> Result<f64, String> {
    o.get(key)
        .map_or(Ok(default), |v| num(v, &format!("{what}'s {key}")))
}

fn vec3(
    o: &serde_json::Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<Option<[f64; 3]>, String> {
    let Some(v) = o.get(key) else { return Ok(None) };
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or(format!("{what}'s {key} must be [x, y, z]"))?;
    let mut out = [0.0; 3];
    for (o, x) in out.iter_mut().zip(a) {
        *o = num(x, &format!("{what}'s {key}"))?;
    }
    if out == [0.0; 3] {
        return Err(format!("{what}'s {key} must not be zero"));
    }
    Ok(Some(out))
}

/// A pair `[min, max]` or a number `x` for `[-x, x]`.
fn range(
    o: &serde_json::Map<String, Value>,
    key: &str,
    default: [f64; 2],
    what: &str,
) -> Result<[f64; 2], String> {
    let r = match o.get(key) {
        None => default,
        Some(Value::Array(a)) if a.len() == 2 => [
            num(&a[0], &format!("{what}'s {key}"))?,
            num(&a[1], &format!("{what}'s {key}"))?,
        ],
        Some(v) => {
            let x = num(v, &format!("{what}'s {key}"))?;
            [-x, x]
        }
    };
    if r[0] > r[1] {
        return Err(format!("{what}'s {key} has its min above its max"));
    }
    Ok(r)
}

fn bits(o: &serde_json::Map<String, Value>, key: &str) -> Result<u32, String> {
    match o.get(key) {
        None => Ok(u32::MAX),
        Some(v) => v
            .as_u64()
            .and_then(|x| u32::try_from(x).ok())
            .ok_or(format!("ragdoll's {key} must be a 32-bit mask")),
    }
}

impl RagdollDef {
    /// Parses a graph file's `ragdoll`.
    pub fn parse(v: &Value) -> Result<RagdollDef, String> {
        let o = v.as_object().ok_or("ragdoll must be an object")?;
        let list = o
            .get("bodies")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .ok_or("ragdoll needs bodies")?;
        let mut bodies: Vec<RagdollBodyDef> = Vec::new();
        for b in list {
            let b = b.as_object().ok_or("a ragdoll body must be an object")?;
            let joint = b
                .get("joint")
                .and_then(Value::as_str)
                .ok_or("a ragdoll body needs its joint")?
                .to_owned();
            let what = format!("ragdoll body {joint}");
            if bodies.iter().any(|x| x.joint == joint) {
                return Err(format!("{what} is declared twice"));
            }
            let to = b.get("to").map(|t| {
                t.as_str()
                    .map(str::to_owned)
                    .ok_or(format!("{what}'s to must be a joint's name"))
            });
            let to = to.transpose()?;
            let length = opt(b, "length", 0.0, &what)?;
            if to.is_none() && length <= 0.0 {
                return Err(format!("{what} needs to, or a length above 0"));
            }
            let radius = opt(b, "radius", 0.0, &what)?;
            let mass = opt(b, "mass", 0.0, &what)?;
            if radius <= 0.0 || mass <= 0.0 {
                return Err(format!("{what} needs a radius and a mass above 0"));
            }
            let swing_deg = opt(b, "swing_deg", 45.0, &what)?;
            if swing_deg < 0.0 {
                return Err(format!("{what}'s swing_deg is below 0"));
            }
            bodies.push(RagdollBodyDef {
                joint,
                to,
                length,
                axis: vec3(b, "axis", &what)?.unwrap_or([0.0, 1.0, 0.0]),
                radius,
                mass,
                hinge: vec3(b, "hinge", &what)?,
                limits_deg: range(b, "limits_deg", [-90.0, 90.0], &what)?,
                swing_deg,
                twist_deg: range(b, "twist_deg", [-30.0, 30.0], &what)?,
            });
        }
        Ok(RagdollDef {
            bodies,
            friction: opt(o, "friction", 0.6, "ragdoll")?,
            linear_damping: opt(o, "linear_damping", 0.05, "ragdoll")?,
            angular_damping: opt(o, "angular_damping", 0.8, "ragdoll")?,
            groups: bits(o, "groups")?,
            mask: bits(o, "mask")?,
        })
    }
}

/// One body bound to the skeleton. Frames are in the model's space without the bones' scale: a
/// body's is its bone's turn and place times (`turn`, `offset`), the offset in the bone's turned
/// frame, so the body's y runs along the bone's segment and its origin is the segment's middle.
#[derive(Clone, Debug, PartialEq)]
pub struct RagdollBody {
    /// The bone.
    pub joint: usize,
    /// The parent body's index in `RagdollRig::bodies` (none for the root).
    pub parent: Option<usize>,
    pub turn: Q,
    pub offset: V3,
    pub half_height: f32,
    pub radius: f32,
    pub mass: f32,
    /// The joint to the parent: its place in this body's frame and in the parent's, the parent's
    /// turn relative to this body at rest, the axis (this body's frame) and the limits, radians.
    pub anchor: V3,
    pub other_anchor: V3,
    pub rest: Q,
    pub axis: V3,
    pub hinge: bool,
    pub limits: [f64; 2],
    pub swing: f64,
}

impl RagdollBody {
    /// The body's frame (no scale) from its bone's global transform.
    pub fn frame(&self, bone: &Xform) -> Xform {
        Xform {
            t: math::add(bone.t, math::rotate(bone.r, self.offset)),
            r: math::normalize(math::qmul(bone.r, self.turn)),
            s: [1.0; 3],
        }
    }

    /// The bone's turn and place from the body's frame.
    pub fn bone(&self, body: &Xform) -> (Q, V3) {
        let r = math::normalize(math::qmul(body.r, math::qconj(self.turn)));
        (r, math::sub(body.t, math::rotate(r, self.offset)))
    }
}

/// A graph's ragdoll bound to its skeleton: its bodies, parents before children.
#[derive(Clone, Debug, PartialEq)]
pub struct RagdollRig {
    pub bodies: Vec<RagdollBody>,
    /// The root body's frame in the rest pose (where it puts the entity, animation.md 6b).
    pub root_rest: Xform,
    pub friction: f64,
    pub linear_damping: f64,
    pub angular_damping: f64,
    pub groups: u32,
    pub mask: u32,
}

const DEG: f64 = std::f64::consts::PI / 180.0;

impl RagdollRig {
    /// Binds `def` to `skel`'s rest pose.
    pub fn bind(def: &RagdollDef, skel: &Skeleton) -> Result<RagdollRig, String> {
        let find = |name: &str, what: &str| {
            skel.find(name)
                .ok_or(format!("ragdoll body {what} names no joint {name}"))
        };
        let rest = skel.globals(&skel.rest());
        // The bodies in the skeleton's order, so a parent comes before its children.
        let mut order: Vec<(usize, &RagdollBodyDef)> = Vec::new();
        for b in &def.bodies {
            order.push((find(&b.joint, &b.joint)?, b));
        }
        let rank = |j: usize| {
            skel.order
                .iter()
                .position(|&x| x as usize == j)
                .unwrap_or(0)
        };
        order.sort_by_key(|(j, _)| rank(*j));
        let mut bodies: Vec<RagdollBody> = Vec::new();
        for (j, b) in order {
            let g = &rest[j];
            let (dir, len) = match &b.to {
                Some(to) => {
                    let t = find(to, &b.joint)?;
                    let v = math::sub(rest[t].t, g.t);
                    let (d, len) = unit(v);
                    if len <= 0.0 {
                        return Err(format!("ragdoll body {}'s to {to} sits on it", b.joint));
                    }
                    (math::rotate(math::qconj(g.r), d), len)
                }
                None => (unit(b.axis.map(|x| x as f32)).0, b.length as f32),
            };
            let turn = from_to([0.0, 1.0, 0.0], dir);
            let offset = math::scale(dir, len * 0.5);
            // The nearest ancestor with a body.
            let mut parent = None;
            let mut up = skel.joints[j].parent.map(|p| p as usize);
            while let Some(a) = up {
                if let Some(i) = bodies.iter().position(|x| x.joint == a) {
                    parent = Some(i);
                    break;
                }
                up = skel.joints[a].parent.map(|p| p as usize);
            }
            if parent.is_none() && !bodies.is_empty() {
                return Err(format!(
                    "ragdoll body {} has no ancestor with a body: a ragdoll has one root",
                    b.joint
                ));
            }
            let mut body = RagdollBody {
                joint: j,
                parent,
                turn,
                offset,
                half_height: len * 0.5,
                radius: b.radius as f32,
                mass: b.mass as f32,
                anchor: [0.0, -len * 0.5, 0.0],
                other_anchor: [0.0; 3],
                rest: math::IDENTITY,
                axis: [0.0, 1.0, 0.0],
                hinge: b.hinge.is_some(),
                limits: if b.hinge.is_some() {
                    [b.limits_deg[0] * DEG, b.limits_deg[1] * DEG]
                } else {
                    [b.twist_deg[0] * DEG, b.twist_deg[1] * DEG]
                },
                swing: b.swing_deg * DEG,
            };
            if let Some(h) = b.hinge {
                let h = unit(h.map(|x| x as f32)).0;
                body.axis = unit(math::rotate(math::qconj(turn), h)).0;
            }
            if let Some(p) = parent {
                let me = body.frame(g);
                let pf = bodies[p].frame(&rest[bodies[p].joint]);
                body.other_anchor = math::rotate(math::qconj(pf.r), math::sub(g.t, pf.t));
                body.rest = math::normalize(math::qmul(math::qconj(me.r), pf.r));
            }
            bodies.push(body);
        }
        let root_rest = bodies[0].frame(&rest[bodies[0].joint]);
        Ok(RagdollRig {
            root_rest,
            bodies,
            friction: def.friction,
            linear_damping: def.linear_damping,
            angular_damping: def.angular_damping,
            groups: def.groups,
            mask: def.mask,
        })
    }

    /// The bones' locals with the bodied ones turned and placed where their bodies are (`bodies`,
    /// model-space frames in this rig's order); the others keep `pose`'s locals under their parents.
    pub fn pose(&self, skel: &Skeleton, pose: &[Xform], bodies: &[Xform]) -> Vec<Xform> {
        let mut globals = vec![Xform::IDENTITY; pose.len()];
        let mut out = pose.to_vec();
        let body_of: Vec<Option<usize>> = (0..pose.len())
            .map(|j| self.bodies.iter().position(|b| b.joint == j))
            .collect();
        for &j in &skel.order {
            let j = j as usize;
            let parent = skel.joints[j]
                .parent
                .map(|p| p as usize)
                .filter(|&p| p < pose.len());
            let pg = parent.map(|p| globals[p]);
            match body_of[j] {
                Some(b) => {
                    let (r, t) = self.bodies[b].bone(&bodies[b]);
                    // The animated global scale stays.
                    let s = pg.map_or(pose[j].s, |p| {
                        [
                            p.s[0] * pose[j].s[0],
                            p.s[1] * pose[j].s[1],
                            p.s[2] * pose[j].s[2],
                        ]
                    });
                    globals[j] = Xform { t, r, s };
                    out[j] = match pg {
                        Some(p) => {
                            let d = math::rotate(math::qconj(p.r), math::sub(t, p.t));
                            Xform {
                                t: [d[0] / p.s[0], d[1] / p.s[1], d[2] / p.s[2]],
                                r: math::normalize(math::qmul(math::qconj(p.r), r)),
                                s: pose[j].s,
                            }
                        }
                        None => Xform { t, r, s: pose[j].s },
                    };
                }
                None => {
                    globals[j] = match pg {
                        Some(p) => p.mul(&pose[j]),
                        None => pose[j],
                    };
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::clip::Joint;
    use super::*;

    fn joint(name: &str, parent: Option<u32>, t: V3) -> Joint {
        Joint {
            name: name.into(),
            parent,
            rest: Xform {
                t,
                ..Xform::IDENTITY
            },
            targets: Vec::new(),
            weights: Vec::new(),
        }
    }

    fn skeleton() -> Skeleton {
        Skeleton::new(vec![
            joint("root", None, [0.0; 3]),
            joint("pelvis", Some(0), [0.0, 1.0, 0.0]),
            joint("spine", Some(1), [0.0, 0.5, 0.0]),
            joint("neck", Some(2), [0.0, 0.4, 0.0]),
            joint("arm", Some(2), [0.3, 0.3, 0.0]),
            joint("hand", Some(4), [0.5, 0.0, 0.0]),
        ])
    }

    fn def(json: &str) -> RagdollDef {
        RagdollDef::parse(&serde_json::from_str(json).unwrap()).unwrap()
    }

    const DOLL: &str = r#"{"bodies": [
        {"joint": "arm", "to": "hand", "radius": 0.05, "mass": 2, "hinge": [0, 0, 1], "limits_deg": [0, 120]},
        {"joint": "pelvis", "to": "spine", "radius": 0.12, "mass": 10},
        {"joint": "spine", "to": "neck", "radius": 0.12, "mass": 8, "swing_deg": 20, "twist_deg": 10}
    ]}"#;

    #[test]
    fn bodies_bind_parents_first_along_their_segments() {
        let skel = skeleton();
        let r = RagdollRig::bind(&def(DOLL), &skel).unwrap();
        let joints: Vec<usize> = r.bodies.iter().map(|b| b.joint).collect();
        assert_eq!(joints, [1, 2, 4], "parents before children");
        assert_eq!(r.bodies[0].parent, None);
        assert_eq!(r.bodies[1].parent, Some(0));
        assert_eq!(r.bodies[2].parent, Some(1), "the arm hangs from the spine");
        let rest = skel.globals(&skel.rest());
        // The arm's body lies along the arm, its middle halfway to the hand, its y along it.
        let arm = r.bodies[2].frame(&rest[4]);
        assert!(
            (arm.t[0] - 0.55).abs() < 1e-6 && (arm.t[1] - 1.8).abs() < 1e-6,
            "{:?}",
            arm.t
        );
        let y = math::rotate(arm.r, [0.0, 1.0, 0.0]);
        assert!((y[0] - 1.0).abs() < 1e-6, "{y:?}");
        assert!((r.bodies[2].half_height - 0.25).abs() < 1e-6);
        // Its joint is at the arm's origin, seen from both bodies.
        let spine = r.bodies[1].frame(&rest[2]);
        let a = arm.apply(r.bodies[2].anchor);
        let b = spine.apply(r.bodies[2].other_anchor);
        assert!(math::length(math::sub(a, b)) < 1e-6, "{a:?} {b:?}");
        assert!(math::length(math::sub(a, rest[4].t)) < 1e-6);
        // At rest the spine is the arm turned by `rest`; the hinge's z stays z.
        let turned = math::qmul(arm.r, r.bodies[2].rest);
        assert!((math::qmul(turned, math::qconj(spine.r))[3].abs() - 1.0).abs() < 1e-6);
        assert!(r.bodies[2].hinge);
        assert!(
            (r.bodies[2].axis[2] - 1.0).abs() < 1e-6,
            "{:?}",
            r.bodies[2].axis
        );
        assert!((r.bodies[2].limits[1] - 120.0 * DEG).abs() < 1e-12);
        assert!((r.bodies[1].swing - 20.0 * DEG).abs() < 1e-12);
    }

    #[test]
    fn the_bodies_rest_frames_give_back_the_rest_pose() {
        let skel = skeleton();
        let r = RagdollRig::bind(&def(DOLL), &skel).unwrap();
        let pose = skel.rest();
        let g = skel.globals(&pose);
        let frames: Vec<Xform> = r.bodies.iter().map(|b| b.frame(&g[b.joint])).collect();
        let back = r.pose(&skel, &pose, &frames);
        for (a, b) in back.iter().zip(&pose) {
            assert!(math::length(math::sub(a.t, b.t)) < 1e-6, "{a:?} {b:?}");
            assert!((a.r[3].abs() - 1.0).abs() < 1e-6, "{a:?}");
        }
        // A pelvis body turned a quarter about z turns the bone, and the spine with it.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let mut moved = frames.clone();
        moved[0].r = math::qmul([0.0, 0.0, h, h], moved[0].r);
        moved[0].t = math::add(
            g[1].t,
            math::rotate([0.0, 0.0, h, h], math::sub(frames[0].t, g[1].t)),
        );
        let back = r.pose(&skel, &pose, &moved);
        assert!((back[1].r[2] - h).abs() < 1e-6, "{:?}", back[1].r);
        assert!(
            math::length(math::sub(back[1].t, pose[1].t)) < 1e-6,
            "{:?}",
            back[1].t
        );
    }

    #[test]
    fn a_ragdoll_parses_and_refuses_what_is_wrong() {
        let d = def(DOLL);
        assert_eq!(d.bodies[2].twist_deg, [-10.0, 10.0]);
        assert_eq!(d.friction, 0.6);
        assert_eq!(d.groups, u32::MAX);
        let bad = |json: &str, says: &str| {
            let e = RagdollDef::parse(&serde_json::from_str(json).unwrap())
                .and_then(|d| RagdollRig::bind(&d, &skeleton()))
                .unwrap_err();
            assert!(e.contains(says), "{e}");
        };
        bad(r#"{"bodies": []}"#, "needs bodies");
        bad(
            r#"{"bodies": [{"joint": "arm", "radius": 0.1, "mass": 1}]}"#,
            "length",
        );
        bad(
            r#"{"bodies": [{"joint": "arm", "length": 1, "radius": 0, "mass": 1}]}"#,
            "radius",
        );
        bad(
            r#"{"bodies": [{"joint": "tail", "length": 1, "radius": 0.1, "mass": 1}]}"#,
            "tail",
        );
        bad(
            r#"{"bodies": [{"joint": "arm", "length": 1, "radius": 0.1, "mass": 1},
                           {"joint": "neck", "length": 1, "radius": 0.1, "mass": 1}]}"#,
            "one root",
        );
        bad(
            r#"{"bodies": [{"joint": "arm", "length": 1, "radius": 0.1, "mass": 1, "twist_deg": [5, -5]}]}"#,
            "min above",
        );
    }
}
