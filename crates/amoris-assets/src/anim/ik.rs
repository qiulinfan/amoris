//! Inverse kinematics on a graph's pose (animation.md 6a): the graph file's IK nodes (`IkDef`),
//! bound to a skeleton (`IkNode`), and the solvers that bend a pose in the model's space: two bones
//! reaching a target (`two_bone`), a chain turning toward one (`look_at`) and feet set on the
//! ground the caller found (`place_feet`). Like the rest of the pose, everything is `f32` `+ - * /`
//! and `sqrt` in a fixed order; the limits' sines and cosines come from `amoris_sim::math` once,
//! when a node is bound.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::clip::Skeleton;
use super::math::{self, IDENTITY, Q, V3, Xform};

/// A node's weight: a number, or a graph parameter read each tick; either clamped to 0..1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IkWeight {
    Value(f64),
    Param(String),
}

/// The feet node's settings (animation.md 6a; the defaults are its).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeetDef {
    pub pelvis: String,
    pub legs: Vec<String>,
    pub probe_up_m: f64,
    pub probe_down_m: f64,
    pub max_drop_m: f64,
    pub max_raise_m: f64,
    pub tilt_deg: f64,
    pub soft_m: f64,
    pub pole: [f64; 3],
    pub rate_mps: f64,
    pub blend_s: f64,
    pub still_mps: f64,
    pub moving_mps: f64,
    pub moving_weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IkKind {
    TwoBone {
        tip: String,
        pole: [f64; 3],
        soft_m: f64,
        keep_tip: bool,
    },
    LookAt {
        joints: Vec<String>,
        axis: [f64; 3],
        limit_deg: f64,
    },
    Feet(FeetDef),
}

/// One IK node of a graph file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IkDef {
    pub name: String,
    pub weight: IkWeight,
    pub kind: IkKind,
}

fn num(v: &Value, what: &str) -> Result<f64, String> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("{what} must be a finite number"))
}

fn text(v: &Value, what: &str) -> Result<String, String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{what} must be a string"))
}

fn vec3(v: &Value, what: &str) -> Result<[f64; 3], String> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or_else(|| format!("{what} must be [x, y, z]"))?;
    Ok([num(&a[0], what)?, num(&a[1], what)?, num(&a[2], what)?])
}

fn field(
    o: &serde_json::Map<String, Value>,
    key: &str,
    default: f64,
    what: &str,
) -> Result<f64, String> {
    o.get(key)
        .map_or(Ok(default), |v| num(v, &format!("{what}'s {key}")))
}

fn nonzero(v: [f64; 3], what: &str) -> Result<[f64; 3], String> {
    if v[0] * v[0] + v[1] * v[1] + v[2] * v[2] > 0.0 {
        Ok(v)
    } else {
        Err(format!("{what} must not be zero"))
    }
}

impl IkDef {
    /// Parses a graph file's `ik` list; `params` are the graph's parameters (a weight may name one).
    pub fn parse_list(v: &Value, params: &[(String, f64)]) -> Result<Vec<IkDef>, String> {
        let list = v.as_array().ok_or("ik must be a list")?;
        let mut out: Vec<IkDef> = Vec::new();
        for n in list {
            let o = n.as_object().ok_or("an ik node must be an object")?;
            let name = text(o.get("name").unwrap_or(&Value::Null), "an ik node's name")?;
            if out.iter().any(|d| d.name == name) {
                return Err(format!("ik node {name} is declared twice"));
            }
            let what = format!("ik node {name}");
            let weight = match o.get("weight") {
                None => IkWeight::Value(1.0),
                Some(Value::String(p)) => {
                    if !params.iter().any(|(n, _)| n == p) {
                        return Err(format!("{what}'s weight names no parameter {p}"));
                    }
                    IkWeight::Param(p.clone())
                }
                Some(w) => IkWeight::Value(num(w, &format!("{what}'s weight"))?),
            };
            let kinds = ["two_bone", "look_at", "feet"];
            let given: Vec<&str> = kinds
                .iter()
                .copied()
                .filter(|k| o.contains_key(*k))
                .collect();
            if given.len() != 1 {
                return Err(format!("{what} needs one of two_bone, look_at or feet"));
            }
            let body = o
                .get(given[0])
                .and_then(Value::as_object)
                .ok_or(format!("{what}'s {} must be an object", given[0]))?;
            let kind = match given[0] {
                "two_bone" => IkKind::TwoBone {
                    tip: text(body.get("tip").unwrap_or(&Value::Null), "a two_bone's tip")?,
                    pole: match body.get("pole") {
                        Some(p) => nonzero(vec3(p, "a two_bone's pole")?, "a two_bone's pole")?,
                        None => [0.0, 0.0, 1.0],
                    },
                    soft_m: field(body, "soft_m", 0.02, &what)?,
                    keep_tip: body.get("keep_tip").map_or(Ok(true), |v| {
                        v.as_bool()
                            .ok_or(format!("{what}'s keep_tip must be a boolean"))
                    })?,
                },
                "look_at" => {
                    let joints = match body.get("joints") {
                        Some(Value::Array(a)) if !a.is_empty() => a
                            .iter()
                            .map(|j| text(j, "a look_at's joint"))
                            .collect::<Result<Vec<_>, _>>()?,
                        _ => return Err(format!("{what}'s joints must be a list of joints")),
                    };
                    IkKind::LookAt {
                        joints,
                        axis: match body.get("axis") {
                            Some(a) => nonzero(vec3(a, "a look_at's axis")?, "a look_at's axis")?,
                            None => [0.0, 0.0, 1.0],
                        },
                        limit_deg: field(body, "limit_deg", 60.0, &what)?,
                    }
                }
                _ => {
                    if out.iter().any(|d| matches!(d.kind, IkKind::Feet(_))) {
                        return Err("a graph has at most one feet node".into());
                    }
                    let legs = match body.get("legs") {
                        Some(Value::Array(a)) if !a.is_empty() => a
                            .iter()
                            .map(|j| text(j, "a feet node's leg"))
                            .collect::<Result<Vec<_>, _>>()?,
                        _ => return Err(format!("{what}'s legs must be a list of foot joints")),
                    };
                    let f = |k: &str, d: f64| field(body, k, d, &what);
                    let def = FeetDef {
                        pelvis: body
                            .get("pelvis")
                            .map_or(Ok("pelvis".to_owned()), |p| text(p, "a feet node's pelvis"))?,
                        legs,
                        probe_up_m: f("probe_up_m", 0.5)?,
                        probe_down_m: f("probe_down_m", 0.6)?,
                        max_drop_m: f("max_drop_m", 0.45)?,
                        max_raise_m: f("max_raise_m", 0.0)?,
                        tilt_deg: f("tilt_deg", 30.0)?,
                        soft_m: f("soft_m", 0.02)?,
                        pole: match body.get("pole") {
                            Some(p) => nonzero(vec3(p, "a feet node's pole")?, "its pole")?,
                            None => [0.0, 0.0, 1.0],
                        },
                        rate_mps: f("rate_mps", 1.5)?,
                        blend_s: f("blend_s", 0.2)?,
                        still_mps: f("still_mps", 0.3)?,
                        moving_mps: f("moving_mps", 2.5)?,
                        moving_weight: f("moving_weight", 0.5)?,
                    };
                    if def.probe_up_m < 0.0 || def.probe_down_m < 0.0 || def.max_drop_m < 0.0 {
                        return Err(format!("{what}'s probes and max_drop_m must be 0 or more"));
                    }
                    if def.moving_mps <= def.still_mps {
                        return Err(format!("{what}'s moving_mps must exceed still_mps"));
                    }
                    IkKind::Feet(def)
                }
            };
            out.push(IkDef { name, weight, kind });
        }
        Ok(out)
    }
}

/// Two bones bound to a skeleton: the root (upper arm, thigh), the middle joint and the tip.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub root: usize,
    pub mid: usize,
    pub tip: usize,
    /// The bend direction in the model's space when neither a goal nor the pose gives one.
    pub pole: V3,
    pub soft: f32,
    pub keep_tip: bool,
}

/// The feet node bound to a skeleton.
#[derive(Clone, Debug, PartialEq)]
pub struct Feet {
    pub pelvis: usize,
    pub legs: Vec<Chain>,
    pub tilt_cos: f32,
    pub tilt_sin: f32,
    pub def: FeetDef,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IkBound {
    TwoBone(Chain),
    LookAt {
        joints: Vec<usize>,
        axis: V3,
        cos: f32,
        sin: f32,
    },
    Feet(Feet),
}

/// An IK node bound to its skeleton.
#[derive(Clone, Debug, PartialEq)]
pub struct IkNode {
    pub name: String,
    pub weight: IkWeight,
    pub kind: IkBound,
}

#[allow(clippy::cast_possible_truncation)]
fn f32_of(x: f64) -> f32 {
    x as f32
}

fn v32(v: [f64; 3]) -> V3 {
    [f32_of(v[0]), f32_of(v[1]), f32_of(v[2])]
}

/// The cosine and sine of an angle in degrees (`amoris_sim::math`, once at binding).
fn cone(deg: f64) -> (f32, f32) {
    let a = amoris_sim::math::clamp(deg, 0.0, 180.0) * (amoris_sim::math::PI / 180.0);
    let (s, c) = amoris_sim::math::sin_cos(a);
    (f32_of(c), f32_of(s))
}

fn chain(
    skel: &Skeleton,
    tip: &str,
    pole: [f64; 3],
    soft: f64,
    keep_tip: bool,
) -> Result<Chain, String> {
    let t = skel
        .find(tip)
        .ok_or(format!("its ik joint {tip} is no joint"))?;
    let m = skel.joints[t]
        .parent
        .map(|p| p as usize)
        .ok_or(format!("its ik joint {tip} has no parent"))?;
    let r = skel.joints[m]
        .parent
        .map(|p| p as usize)
        .ok_or(format!("its ik joint {tip} has no grandparent"))?;
    Ok(Chain {
        root: r,
        mid: m,
        tip: t,
        pole: v32(pole),
        soft: f32_of(amoris_sim::math::max(soft, 0.0)),
        keep_tip,
    })
}

fn is_ancestor(skel: &Skeleton, a: usize, mut j: usize) -> bool {
    for _ in 0..skel.joints.len() {
        match skel.joints[j].parent {
            Some(p) if p as usize == a => return true,
            Some(p) => j = p as usize,
            None => return false,
        }
    }
    false
}

impl IkNode {
    /// Binds `def` to `skel`'s joints.
    pub fn bind(def: &IkDef, skel: &Skeleton) -> Result<IkNode, String> {
        let kind = match &def.kind {
            IkKind::TwoBone {
                tip,
                pole,
                soft_m,
                keep_tip,
            } => IkBound::TwoBone(chain(skel, tip, *pole, *soft_m, *keep_tip)?),
            IkKind::LookAt {
                joints,
                axis,
                limit_deg,
            } => {
                let mut js = Vec::new();
                for j in joints {
                    js.push(
                        skel.find(j)
                            .ok_or(format!("its ik joint {j} is no joint"))?,
                    );
                }
                for w in js.windows(2) {
                    if !is_ancestor(skel, w[0], w[1]) {
                        return Err(format!(
                            "ik node {}'s joint {} is no ancestor of {}",
                            def.name, skel.joints[w[0]].name, skel.joints[w[1]].name
                        ));
                    }
                }
                let (cos, sin) = cone(*limit_deg);
                IkBound::LookAt {
                    joints: js,
                    axis: unit(v32(*axis)).0,
                    cos,
                    sin,
                }
            }
            IkKind::Feet(f) => {
                let pelvis = skel
                    .find(&f.pelvis)
                    .ok_or(format!("its ik joint {} is no joint", f.pelvis))?;
                let mut legs = Vec::new();
                for l in &f.legs {
                    let c = chain(skel, l, f.pole, f.soft_m, true)?;
                    if !is_ancestor(skel, pelvis, c.root) && pelvis != c.root {
                        return Err(format!("its leg {l} does not hang from {}", f.pelvis));
                    }
                    legs.push(c);
                }
                let (tilt_cos, tilt_sin) = cone(f.tilt_deg);
                IkBound::Feet(Feet {
                    pelvis,
                    legs,
                    tilt_cos,
                    tilt_sin,
                    def: f.clone(),
                })
            }
        };
        Ok(IkNode {
            name: def.name.clone(),
            weight: def.weight.clone(),
            kind,
        })
    }
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `v` scaled to unit length and its length (zero stays zero).
pub fn unit(v: V3) -> (V3, f32) {
    let n = math::length(v);
    if n > 0.0 && n.is_finite() {
        ([v[0] / n, v[1] / n, v[2] / n], n)
    } else {
        ([0.0; 3], 0.0)
    }
}

/// A unit vector perpendicular to the unit `u`.
fn perpendicular(u: V3) -> V3 {
    let other = if u[0] < 0.9 && u[0] > -0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    unit(math::cross(u, other)).0
}

/// The shortest rotation taking the unit `u` to the unit `v`.
pub fn from_to(u: V3, v: V3) -> Q {
    let d = dot(u, v);
    if d < -0.999_999 {
        let a = perpendicular(u);
        return [a[0], a[1], a[2], 0.0];
    }
    let c = math::cross(u, v);
    math::normalize([c[0], c[1], c[2], 1.0 + d])
}

/// The unit `want` turned toward the unit `from` until it lies within the cone of cosine `cos` and
/// sine `sin` around `from`.
pub fn clamp_cone(from: V3, want: V3, cos: f32, sin: f32) -> V3 {
    let d = dot(from, want);
    if d >= cos {
        return want;
    }
    let side = math::sub(want, math::scale(from, d));
    let (side, n) = unit(side);
    let side = if n > 1e-6 { side } else { perpendicular(from) };
    unit(math::add(math::scale(from, cos), math::scale(side, sin))).0
}

/// The global (model-space) transform of a joint's parent; the identity for a root.
fn parent_global(skel: &Skeleton, pose: &[Xform], j: usize) -> Xform {
    match skel.joints[j].parent.map(|p| p as usize) {
        Some(p) if p < pose.len() => skel.global(pose, p),
        _ => Xform::IDENTITY,
    }
}

fn mixed(old: Q, new: Q, w: f32) -> Q {
    if w >= 1.0 {
        new
    } else {
        math::nlerp(old, new, w)
    }
}

/// Brings `chain`'s tip to `target` (the model's space) in `pose` (local transforms). `pole` is a
/// point the bend turns toward; without one, the pose's own bend, else the chain's pole. `tip` is
/// the tip's rotation in the model's space to take; without one it keeps its own (or turns with the
/// lower bone, `keep_tip` false). `w` mixes each joint's rotation from the pose toward the solution.
pub fn two_bone(
    skel: &Skeleton,
    pose: &mut [Xform],
    chain: &Chain,
    target: V3,
    pole: Option<V3>,
    tip: Option<Q>,
    w: f32,
) {
    if w <= 0.0 {
        return;
    }
    let gp = parent_global(skel, pose, chain.root);
    let ga = gp.mul(&pose[chain.root]);
    let gb = ga.mul(&pose[chain.mid]);
    let gc = gb.mul(&pose[chain.tip]);
    let (a, b, c) = (ga.t, gb.t, gc.t);
    let (ab, l1) = unit(math::sub(b, a));
    let l2 = math::length(math::sub(c, b));
    let (dir, d) = unit(math::sub(target, a));
    let reach = l1 + l2;
    if l1 <= 1e-6 || l2 <= 1e-6 || d <= 1e-6 * reach {
        return;
    }
    // The reach: no closer than the bones' difference, softly short of full extension.
    let near = (l1 - l2).abs() + 1e-4 * reach;
    let soft = if chain.soft < reach - near {
        chain.soft
    } else {
        reach - near
    };
    let knee = reach - soft;
    let mut dist = if soft > 0.0 && d > knee {
        let u = (d - knee) / soft;
        knee + soft * u / (1.0 + u)
    } else if d > reach {
        reach
    } else {
        d
    };
    if dist < near {
        dist = near;
    }
    // The bend's direction, square to the line from the root to the target.
    let animated = {
        let (ac, _) = unit(math::sub(c, a));
        let ba = math::sub(b, a);
        math::sub(ba, math::scale(ac, dot(ba, ac)))
    };
    let square = |v: V3| unit(math::sub(v, math::scale(dir, dot(v, dir))));
    let mut bend = match pole {
        Some(p) => square(math::sub(p, a)),
        None if math::length(animated) > 1e-4 * reach => square(animated),
        None => square(chain.pole),
    };
    if bend.1 <= 1e-6 {
        bend = square(chain.pole);
    }
    let bend = if bend.1 > 1e-6 {
        bend.0
    } else {
        perpendicular(dir)
    };
    let x = (dist * dist + l1 * l1 - l2 * l2) / (2.0 * dist);
    let h2 = l1 * l1 - x * x;
    let h = if h2 > 0.0 { h2.sqrt() } else { 0.0 };
    let m = math::add(a, math::add(math::scale(dir, x), math::scale(bend, h)));
    let e = math::add(a, math::scale(dir, dist));
    let q1 = from_to(ab, unit(math::sub(m, a)).0);
    let ra = math::normalize(math::qmul(q1, ga.r));
    let c1 = math::add(m, math::rotate(q1, math::sub(c, b)));
    let q2 = from_to(unit(math::sub(c1, m)).0, unit(math::sub(e, m)).0);
    let q21 = math::qmul(q2, q1);
    let rb = math::normalize(math::qmul(q21, gb.r));
    let rc = match tip {
        Some(r) => r,
        None if chain.keep_tip => gc.r,
        None => math::normalize(math::qmul(q21, gc.r)),
    };
    let la = math::normalize(math::qmul(math::qconj(gp.r), ra));
    let lb = math::normalize(math::qmul(math::qconj(ra), rb));
    let lc = math::normalize(math::qmul(math::qconj(rb), rc));
    pose[chain.root].r = mixed(pose[chain.root].r, la, w);
    pose[chain.mid].r = mixed(pose[chain.mid].r, lb, w);
    pose[chain.tip].r = mixed(pose[chain.tip].r, lc, w);
}

/// Turns `joints` (root first) so `axis` of the last points at `target` (the model's space),
/// clamped to the cone (`cos`, `sin`) around where the pose points it; each joint takes its share
/// of what remains. `w` mixes each joint's rotation from the pose toward the solution.
#[allow(clippy::too_many_arguments)]
pub fn look_at(
    skel: &Skeleton,
    pose: &mut [Xform],
    joints: &[usize],
    axis: V3,
    cos: f32,
    sin: f32,
    target: V3,
    w: f32,
) {
    let Some(&tip) = joints.last() else { return };
    if w <= 0.0 {
        return;
    }
    let before: Vec<Q> = joints.iter().map(|&j| pose[j].r).collect();
    let g = skel.global(pose, tip);
    let aim0 = unit(math::rotate(g.r, axis)).0;
    let n = joints.len();
    for (k, &j) in joints.iter().enumerate() {
        let g = skel.global(pose, tip);
        let (want, len) = unit(math::sub(target, g.t));
        if len <= 1e-6 {
            break;
        }
        let want = clamp_cone(aim0, want, cos, sin);
        let aim = unit(math::rotate(g.r, axis)).0;
        #[allow(clippy::cast_precision_loss)]
        let share = 1.0 / (n - k) as f32;
        let q = math::nlerp(IDENTITY, from_to(aim, want), share);
        let gp = parent_global(skel, pose, j);
        let gj = gp.mul(&pose[j]);
        let r = math::normalize(math::qmul(q, gj.r));
        pose[j].r = math::normalize(math::qmul(math::qconj(gp.r), r));
    }
    for (&j, old) in joints.iter().zip(before) {
        pose[j].r = mixed(old, pose[j].r, w);
    }
}

/// What the caller found under one foot, in the model's space: how far its ground raises it, and
/// the ground's unit normal (none: the foot keeps its rotation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootGround {
    pub raise: V3,
    pub normal: Option<V3>,
}

/// Sets `feet` on their ground in `pose`: the pelvis moved by `pelvis` (the model's space), each
/// foot brought to its animated place raised by its ground and turned toward its ground's normal
/// within the tilt limit, all scaled by `w`; `up` is the world's up in the model's space.
pub fn place_feet(
    skel: &Skeleton,
    pose: &mut [Xform],
    feet: &Feet,
    pelvis: V3,
    up: V3,
    grounds: &[FootGround],
    w: f32,
) {
    if w <= 0.0 {
        return;
    }
    let animated: Vec<Xform> = feet.legs.iter().map(|l| skel.global(pose, l.tip)).collect();
    let gp = parent_global(skel, pose, feet.pelvis);
    let g = gp.mul(&pose[feet.pelvis]);
    let at = math::add(g.t, math::scale(pelvis, w));
    let d = math::rotate(math::qconj(gp.r), math::sub(at, gp.t));
    pose[feet.pelvis].t = [d[0] / gp.s[0], d[1] / gp.s[1], d[2] / gp.s[2]];
    for ((leg, foot), ground) in feet.legs.iter().zip(&animated).zip(grounds) {
        let target = math::add(foot.t, math::scale(ground.raise, w));
        let turn = ground.normal.map_or(IDENTITY, |n| {
            let to = clamp_cone(up, n, feet.tilt_cos, feet.tilt_sin);
            math::nlerp(IDENTITY, from_to(up, to), w)
        });
        let r = math::normalize(math::qmul(turn, foot.r));
        two_bone(skel, pose, leg, target, None, Some(r), 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::clip::Joint;

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

    /// A leg: hip at 1 m, a knee 0.5 m below bent 5 cm forward, an ankle 0.5 m below that.
    fn leg() -> Skeleton {
        Skeleton::new(vec![
            joint("hip", None, [0.0, 1.0, 0.0]),
            joint("knee", Some(0), [0.0, -0.5, 0.05]),
            joint("ankle", Some(1), [0.0, -0.5, -0.05]),
        ])
    }

    fn chain_of(s: &Skeleton, soft: f32) -> Chain {
        Chain {
            soft,
            ..chain(s, "ankle", [0.0, 0.0, 1.0], 0.0, true).unwrap()
        }
    }

    fn close(a: V3, b: V3, eps: f32) -> bool {
        math::length(math::sub(a, b)) <= eps
    }

    #[test]
    fn two_bones_reach_a_reachable_target_and_keep_their_lengths() {
        let s = leg();
        let c = chain_of(&s, 0.0);
        let mut pose = s.rest();
        let rest_tip = s.global(&pose, 2).r;
        let target = [0.2, 0.4, 0.3];
        two_bone(&s, &mut pose, &c, target, None, None, 1.0);
        let g = s.globals(&pose);
        assert!(close(g[2].t, target, 1e-5), "{:?}", g[2].t);
        let l1 = math::length(math::sub(g[1].t, g[0].t));
        let l2 = math::length(math::sub(g[2].t, g[1].t));
        let r1 = math::length([0.0, -0.5, 0.05]);
        assert!(
            (l1 - r1).abs() < 1e-5 && (l2 - r1).abs() < 1e-5,
            "{l1} {l2}"
        );
        // The tip keeps its rotation in the model's space.
        let r = g[2].r;
        let d = (r[0] * rest_tip[0] + r[1] * rest_tip[1] + r[2] * rest_tip[2] + r[3] * rest_tip[3])
            .abs();
        assert!(d > 1.0 - 1e-5, "{r:?}");
        // The knee went forward, the way the pose bent it.
        assert!(g[1].t[2] > 0.3, "{:?}", g[1].t);
    }

    #[test]
    fn a_pole_turns_the_bend() {
        let s = leg();
        let c = chain_of(&s, 0.0);
        let mut pose = s.rest();
        // A target straight below and closer: the knee goes toward the pole on +x.
        two_bone(
            &s,
            &mut pose,
            &c,
            [0.0, 0.3, 0.0],
            Some([5.0, 0.5, 0.0]),
            None,
            1.0,
        );
        let g = s.globals(&pose);
        assert!(close(g[2].t, [0.0, 0.3, 0.0], 1e-5), "{:?}", g[2].t);
        assert!(g[1].t[0] > 0.3 && g[1].t[2].abs() < 1e-4, "{:?}", g[1].t);
        // Without one, a straight leg bends toward the chain's pole (ahead, +z).
        let straight = Skeleton::new(vec![
            joint("hip", None, [0.0, 1.0, 0.0]),
            joint("knee", Some(0), [0.0, -0.5, 0.0]),
            joint("ankle", Some(1), [0.0, -0.5, 0.0]),
        ]);
        let c = chain_of(&straight, 0.0);
        let mut pose = straight.rest();
        two_bone(&straight, &mut pose, &c, [0.0, 0.3, 0.0], None, None, 1.0);
        let g = straight.globals(&pose);
        assert!(g[1].t[2] > 0.3, "{:?}", g[1].t);
    }

    #[test]
    fn an_unreachable_target_is_reached_toward_and_softly() {
        let s = leg();
        let mut pose = s.rest();
        let reach = 2.0 * math::length([0.0, -0.5, 0.05]);
        // Hard: the limb straightens along the line to the target.
        two_bone(
            &s,
            &mut pose,
            &chain_of(&s, 0.0),
            [3.0, 1.0, 0.0],
            None,
            None,
            1.0,
        );
        let g = s.globals(&pose);
        assert!(close(g[2].t, [reach, 1.0, 0.0], 1e-4), "{:?}", g[2].t);
        // Soft: always short of full extension, the shortfall shrinking smoothly, never negative.
        let mut last = 0.0;
        for k in 0..40 {
            #[allow(clippy::cast_precision_loss)]
            let d = 0.9 + 0.01 * k as f32;
            let mut pose = s.rest();
            two_bone(
                &s,
                &mut pose,
                &chain_of(&s, 0.05),
                [d, 1.0, 0.0],
                None,
                None,
                1.0,
            );
            let g = s.globals(&pose);
            let got = g[2].t[0];
            assert!(got < reach, "{d}: {got}");
            assert!(got >= last - 1e-6, "{d}: {got} after {last}");
            if d <= reach - 0.05 {
                assert!((got - d).abs() < 1e-4, "{d}: {got}");
            }
            last = got;
        }
        assert!(reach - last < 0.025, "{last}");
        // Too near: no closer than the bones' difference (here a folded leg).
        let mut pose = s.rest();
        two_bone(
            &s,
            &mut pose,
            &chain_of(&s, 0.0),
            [0.0, 1.0, 0.0001],
            None,
            None,
            1.0,
        );
        let g = s.globals(&pose);
        assert!(g.iter().all(|x| x.t.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn a_weight_mixes_the_solution_and_zero_changes_nothing() {
        let s = leg();
        let c = chain_of(&s, 0.0);
        let mut none = s.rest();
        two_bone(&s, &mut none, &c, [0.3, 0.4, 0.0], None, None, 0.0);
        assert_eq!(none, s.rest());
        let mut full = s.rest();
        two_bone(&s, &mut full, &c, [0.3, 0.4, 0.0], None, None, 1.0);
        let mut half = s.rest();
        two_bone(&s, &mut half, &c, [0.3, 0.4, 0.0], None, None, 0.5);
        let x = |p: &[Xform]| s.global(p, 2).t[0];
        assert!(
            x(&half) > 0.0 && x(&half) < x(&full),
            "{} {}",
            x(&half),
            x(&full)
        );
    }

    /// A neck and a head facing +z.
    fn neck() -> Skeleton {
        Skeleton::new(vec![
            joint("chest", None, [0.0, 1.4, 0.0]),
            joint("neck", Some(0), [0.0, 0.1, 0.0]),
            joint("head", Some(1), [0.0, 0.1, 0.0]),
        ])
    }

    #[test]
    fn a_look_at_turns_within_its_limit_and_ends_on_a_target_inside_it() {
        let s = neck();
        let (cos, sin) = cone(60.0);
        let aim = |p: &[Xform]| unit(math::rotate(s.global(p, 2).r, [0.0, 0.0, 1.0])).0;
        // A target 45 degrees to the left (+x): reached.
        let mut pose = s.rest();
        let head = s.global(&pose, 2).t;
        let t = math::add(head, [1.0, 0.0, 1.0]);
        look_at(&s, &mut pose, &[1, 2], [0.0, 0.0, 1.0], cos, sin, t, 1.0);
        let want = unit(math::sub(t, s.global(&pose, 2).t)).0;
        assert!(dot(aim(&pose), want) > 0.9999, "{:?}", aim(&pose));
        // Both joints turned, the neck about half of it.
        let neck_turn = pose[1].r;
        assert!(neck_turn[1] > 0.05 && pose[2].r[1] > 0.05, "{pose:?}");
        // A target behind: the head stops at 60 degrees.
        let mut pose = s.rest();
        let t = math::add(head, [0.2, 0.0, -1.0]);
        look_at(&s, &mut pose, &[1, 2], [0.0, 0.0, 1.0], cos, sin, t, 1.0);
        let d = dot(aim(&pose), [0.0, 0.0, 1.0]);
        assert!((d - 0.5).abs() < 1e-4, "{d}");
        assert!(
            aim(&pose)[0] > 0.0,
            "turned the shorter way: {:?}",
            aim(&pose)
        );
        // Half the weight turns less; none changes nothing.
        let mut half = s.rest();
        look_at(&s, &mut half, &[1, 2], [0.0, 0.0, 1.0], cos, sin, t, 0.5);
        let dh = dot(aim(&half), [0.0, 0.0, 1.0]);
        assert!(dh > d && dh < 1.0, "{dh}");
        let mut none = s.rest();
        look_at(&s, &mut none, &[1, 2], [0.0, 0.0, 1.0], cos, sin, t, 0.0);
        assert_eq!(none, s.rest());
    }

    #[test]
    fn a_cone_clamps_and_from_to_turns_one_vector_onto_another() {
        let (cos, sin) = cone(30.0);
        let v = clamp_cone([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], cos, sin);
        assert!(
            (dot(v, [0.0, 1.0, 0.0]) - cos).abs() < 1e-6 && v[0] > 0.0,
            "{v:?}"
        );
        let inside = unit([0.1, 1.0, 0.0]).0;
        assert_eq!(clamp_cone([0.0, 1.0, 0.0], inside, cos, sin), inside);
        for (u, w) in [
            ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 1.0, 0.0], [0.0, -1.0, 0.0]),
            (unit([1.0, 2.0, 3.0]).0, unit([-3.0, 1.0, 0.5]).0),
        ] {
            let q = from_to(u, w);
            assert!(close(math::rotate(q, u), w, 1e-5), "{u:?} {w:?}");
        }
    }

    #[test]
    fn feet_go_down_to_their_ground_and_the_pelvis_with_the_lower() {
        // Two legs under a pelvis.
        let s = Skeleton::new(vec![
            joint("pelvis", None, [0.0, 1.0, 0.0]),
            joint("thigh_l", Some(0), [0.1, 0.0, 0.0]),
            joint("shin_l", Some(1), [0.0, -0.45, 0.03]),
            joint("foot_l", Some(2), [0.0, -0.45, -0.03]),
            joint("thigh_r", Some(0), [-0.1, 0.0, 0.0]),
            joint("shin_r", Some(4), [0.0, -0.45, 0.03]),
            joint("foot_r", Some(5), [0.0, -0.45, -0.03]),
        ]);
        let def = IkDef::parse_list(
            &serde_json::json!([{"name": "feet", "feet": {"legs": ["foot_l", "foot_r"], "soft_m": 0}}]),
            &[],
        )
        .unwrap();
        let IkBound::Feet(f) = IkNode::bind(&def[0], &s).unwrap().kind else {
            panic!()
        };
        let mut pose = s.rest();
        let up = [0.0, 1.0, 0.0];
        let grounds = [
            FootGround {
                raise: [0.0, 0.0, 0.0],
                normal: Some(up),
            },
            FootGround {
                raise: [0.0, -0.2, 0.0],
                normal: Some(unit([0.2, 1.0, 0.0]).0),
            },
        ];
        place_feet(&s, &mut pose, &f, [0.0, -0.2, 0.0], up, &grounds, 1.0);
        let g = s.globals(&pose);
        assert!((g[0].t[1] - 0.8).abs() < 1e-6, "{:?}", g[0].t);
        assert!(close(g[3].t, [0.1, 0.1, 0.0], 1e-5), "{:?}", g[3].t);
        assert!(close(g[6].t, [-0.1, -0.1, 0.0], 1e-5), "{:?}", g[6].t);
        // The right foot turned toward its slope's normal, the left kept flat.
        let n = math::rotate(g[6].r, up);
        assert!(dot(n, unit([0.2, 1.0, 0.0]).0) > 0.9999, "{n:?}");
        assert!(dot(math::rotate(g[3].r, up), up) > 0.9999);
    }

    #[test]
    fn the_ik_list_parses_and_refuses_what_is_wrong() {
        let params = vec![("reach".to_owned(), 0.0)];
        let ok = serde_json::json!([
            {"name": "feet", "feet": {"legs": ["foot_l"], "tilt_deg": 20}},
            {"name": "hand", "two_bone": {"tip": "hand", "pole": [0, 0, -1]}, "weight": "reach"},
            {"name": "look", "look_at": {"joints": ["neck", "head"], "limit_deg": 70}, "weight": 0.5}
        ]);
        let d = IkDef::parse_list(&ok, &params).unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[1].weight, IkWeight::Param("reach".into()));
        assert_eq!(d[2].weight, IkWeight::Value(0.5));
        let IkKind::Feet(f) = &d[0].kind else {
            panic!()
        };
        assert_eq!(
            (f.pelvis.as_str(), f.tilt_deg, f.probe_up_m),
            ("pelvis", 20.0, 0.5)
        );
        let bad = |v: serde_json::Value, want: &str| {
            let e = IkDef::parse_list(&v, &params).unwrap_err();
            assert!(e.contains(want), "{e}");
        };
        bad(
            serde_json::json!([{"name": "a", "two_bone": {"tip": "x"}}, {"name": "a", "two_bone": {"tip": "y"}}]),
            "twice",
        );
        bad(
            serde_json::json!([{"name": "a", "two_bone": {"tip": "x"}, "weight": "nope"}]),
            "no parameter nope",
        );
        bad(serde_json::json!([{"name": "a"}]), "needs one of");
        bad(
            serde_json::json!([{"name": "a", "feet": {"legs": ["x"]}}, {"name": "b", "feet": {"legs": ["y"]}}]),
            "at most one",
        );
        bad(
            serde_json::json!([{"name": "a", "look_at": {"joints": []}}]),
            "joints",
        );
        bad(
            serde_json::json!([{"name": "a", "two_bone": {"tip": "x", "pole": [0, 0, 0]}}]),
            "zero",
        );
        // Binding: a tip needs two ancestors; a look-at's joints run root to tip.
        let s = neck();
        let two = IkDef::parse_list(
            &serde_json::json!([{"name": "a", "two_bone": {"tip": "neck"}}]),
            &[],
        )
        .unwrap();
        assert!(
            IkNode::bind(&two[0], &s)
                .unwrap_err()
                .contains("grandparent")
        );
        let back = IkDef::parse_list(
            &serde_json::json!([{"name": "a", "look_at": {"joints": ["head", "neck"]}}]),
            &[],
        )
        .unwrap();
        assert!(IkNode::bind(&back[0], &s).unwrap_err().contains("ancestor"));
    }
}
