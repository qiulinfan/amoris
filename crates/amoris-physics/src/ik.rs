//! `anim.ik` (animation.md 6a): a graph's IK nodes bend the pose `anim.graph` evaluated, after the
//! body stands where this tick put it: two bones and look-ats toward scripts' goals, feet set on
//! the fixed colliders found by rays down from each foot. Graphs without IK nodes are left alone.

use amoris_assets::Model;
use amoris_assets::anim::ik::{self, FootGround, IkBound, IkWeight};
use amoris_assets::anim::math::{self as am, V3, Xform};
use amoris_assets::anim::{AnimGraph, AnimIk, AnimPose, AnimRigs, AnimState, IkGoal, Socket};
use amoris_sim::math;
use amoris_sim::order::by_id_mut;
use amoris_sim::{EntityIndex, SimClock};
use bevy_ecs::prelude::{Commands, Entity, Query, Res};

use crate::avatar::{Avatar, f32x3, frame};
use crate::body::Transform;
use crate::geom;
use crate::solver::Physics;
use crate::walker::Walker;

type IkRow<'a> = (
    Entity,
    &'a amoris_sim::EntityId,
    &'a AnimGraph,
    &'a mut AnimState,
    &'a mut AnimPose,
    &'a Transform,
    Option<&'a Model>,
    Option<&'a Avatar>,
    Option<&'a mut AnimIk>,
);

/// A world point in the model's space of `w`.
fn point_in(w: &Xform, p: V3) -> V3 {
    let d = am::rotate(am::qconj(w.r), am::sub(p, w.t));
    [d[0] / w.s[0], d[1] / w.s[1], d[2] / w.s[2]]
}

/// A world offset in the model's space of `w`.
fn offset_in(w: &Xform, v: V3) -> V3 {
    let d = am::rotate(am::qconj(w.r), v);
    [d[0] / w.s[0], d[1] / w.s[1], d[2] / w.s[2]]
}

/// A world direction in the model's space of `w`, unit.
fn direction_in(w: &Xform, v: V3) -> V3 {
    ik::unit(am::rotate(am::qconj(w.r), v)).0
}

fn goal<'a>(ik: Option<&'a AnimIk>, node: &str) -> Option<&'a IkGoal> {
    ik.and_then(|k| k.goals.iter().rev().find(|g| g.node == node))
}

/// A node's own weight, its parameter's value when it names one, clamped to 0..1.
fn own_weight(w: &IkWeight, st: &AnimState) -> f64 {
    let v = match w {
        IkWeight::Value(v) => *v,
        IkWeight::Param(p) => st
            .params
            .iter()
            .find(|x| &x.name == p)
            .map_or(0.0, |x| x.value),
    };
    math::clamp(v, 0.0, 1.0)
}

#[allow(clippy::cast_possible_truncation)]
fn f32_of(x: f64) -> f32 {
    x as f32
}

/// The feet's state for this tick: what its speed and grounding want, moved toward by the blend;
/// each leg's ground moved toward what its ray found by the rate. `fresh` takes both at once.
struct FeetTick {
    weight: f64,
    ground: Vec<f64>,
    normals: Vec<Option<V3>>,
}

#[allow(clippy::too_many_arguments)]
fn feet_tick(
    f: &ik::Feet,
    skel: &amoris_assets::anim::Skeleton,
    pose: &[Xform],
    world: &Xform,
    origin_y: f64,
    physics: Option<&Physics>,
    wanted: f64,
    last: Option<&AnimIk>,
    dt: f64,
) -> FeetTick {
    let d = &f.def;
    let n = f.legs.len();
    let fresh = last.is_none_or(|k| k.ground_m.len() != n);
    let mut ground = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    for (i, leg) in f.legs.iter().enumerate() {
        let foot = world.apply(skel.global(pose, leg.tip).t);
        let from = [
            f64::from(foot[0]),
            origin_y + d.probe_up_m,
            f64::from(foot[2]),
        ];
        let reach = d.probe_up_m + d.probe_down_m;
        let hit = physics.and_then(|p| {
            let ray =
                rapier3d::prelude::Ray::new(geom::vec_in(from), geom::vec_in([0.0, -1.0, 0.0]));
            p.world.cast_ray_and_get_normal(
                &ray,
                geom::f32_of(reach),
                true,
                rapier3d::prelude::QueryFilter::only_fixed().exclude_sensors(),
            )
        });
        let (found, normal) = match hit {
            Some((_, h)) => {
                let nv = geom::vec_out(h.normal);
                let normal = (nv[1] > 0.0).then(|| f32x3(nv));
                (d.probe_up_m - f64::from(h.time_of_impact), normal)
            }
            None => (0.0, None),
        };
        let g = match last {
            Some(k) if !fresh => {
                let step = d.rate_mps * dt;
                k.ground_m[i] + math::clamp(found - k.ground_m[i], -step, step)
            }
            _ => found,
        };
        ground.push(g);
        normals.push(normal);
    }
    let weight = match last {
        Some(k) if !fresh => {
            if d.blend_s > 0.0 {
                let step = dt / d.blend_s;
                k.weight + math::clamp(wanted - k.weight, -step, step)
            } else {
                wanted
            }
        }
        _ => wanted,
    };
    FeetTick {
        weight: math::clamp(weight, 0.0, 1.0),
        ground,
        normals,
    }
}

/// How much the feet want to act for a body moving at `speed` m/s.
fn speed_share(d: &amoris_assets::anim::ik::FeetDef, speed: f64) -> f64 {
    if speed <= d.still_mps {
        1.0
    } else if speed >= d.moving_mps {
        d.moving_weight
    } else {
        math::lerp(
            1.0,
            d.moving_weight,
            (speed - d.still_mps) / (d.moving_mps - d.still_mps),
        )
    }
}

/// `anim.ik` (Physics, after `physics.avatar`): every graph with IK nodes bends its pose by them,
/// in order, and keeps its sockets from the bent pose (animation.md 6a).
#[allow(clippy::too_many_arguments)]
pub(crate) fn ik(
    clock: Res<SimClock>,
    index: Res<EntityIndex>,
    rigs: Option<Res<AnimRigs>>,
    physics: Option<Res<Physics>>,
    walkers: Query<&Walker>,
    mut q: Query<IkRow<'static>>,
    mut commands: Commands,
) {
    let Some(rigs) = rigs else { return };
    let dt = clock.dt();
    for (entity, _, graph, mut st, mut pose, tf, model, avatar, mut state) in by_id_mut(&mut q) {
        let Some(rig) = rigs.rig(&graph.graph) else {
            continue;
        };
        if rig.ik().is_empty() || st.graph != graph.graph {
            continue;
        }
        if pose.joints.len() != rig.skeleton.joints.len() {
            continue;
        }
        let world = frame(tf, model.map_or([1.0; 3], |m| m.scale));
        let skel = &rig.skeleton;
        let mut joints = pose.joints.clone();
        let mut feet_state: Option<(f64, Vec<f64>)> = None;
        let last = state.as_deref();
        for node in rig.ik() {
            let g = goal(last, &node.name);
            let w =
                own_weight(&node.weight, &st) * g.map_or(1.0, |g| math::clamp(g.weight, 0.0, 1.0));
            match &node.kind {
                IkBound::TwoBone(chain) => {
                    let Some(t) = g.and_then(|g| g.target) else {
                        continue;
                    };
                    let pole = g.and_then(|g| g.pole).map(|p| point_in(&world, f32x3(p)));
                    let target = point_in(&world, f32x3(t));
                    ik::two_bone(skel, &mut joints, chain, target, pole, None, f32_of(w));
                }
                IkBound::LookAt {
                    joints: chain,
                    axis,
                    cos,
                    sin,
                } => {
                    let Some(t) = g.and_then(|g| g.target) else {
                        continue;
                    };
                    let target = point_in(&world, f32x3(t));
                    ik::look_at(
                        skel,
                        &mut joints,
                        chain,
                        *axis,
                        *cos,
                        *sin,
                        target,
                        f32_of(w),
                    );
                }
                IkBound::Feet(f) => {
                    let (speed, grounded) = match avatar {
                        Some(a) => (
                            a.speed,
                            a.walker
                                .and_then(|id| index.get(id))
                                .and_then(|e| walkers.get(e).ok())
                                .is_none_or(|w| w.grounded),
                        ),
                        None => (0.0, true),
                    };
                    let wanted = if grounded {
                        w * speed_share(&f.def, speed)
                    } else {
                        0.0
                    };
                    let t = feet_tick(
                        f,
                        skel,
                        &joints,
                        &world,
                        tf.position[1],
                        physics.as_deref(),
                        wanted,
                        last,
                        dt,
                    );
                    if t.weight > 0.0 {
                        let lowest = t.ground.iter().copied().fold(f64::INFINITY, math::min);
                        let pelvis = math::clamp(lowest, -f.def.max_drop_m, f.def.max_raise_m);
                        let up = direction_in(&world, [0.0, 1.0, 0.0]);
                        let grounds: Vec<FootGround> = t
                            .ground
                            .iter()
                            .zip(&t.normals)
                            .map(|(g, n)| FootGround {
                                raise: offset_in(&world, [0.0, f32_of(*g), 0.0]),
                                normal: n.map(|n| direction_in(&world, n)),
                            })
                            .collect();
                        ik::place_feet(
                            skel,
                            &mut joints,
                            f,
                            offset_in(&world, [0.0, f32_of(pelvis), 0.0]),
                            up,
                            &grounds,
                            f32_of(t.weight),
                        );
                    }
                    feet_state = Some((t.weight, t.ground));
                }
            }
        }
        if let Some((weight, ground_m)) = feet_state {
            match state.as_mut() {
                Some(k) => {
                    if k.weight != weight || k.ground_m != ground_m {
                        k.weight = weight;
                        k.ground_m = ground_m;
                    }
                }
                None => {
                    commands.entity(entity).insert(AnimIk {
                        goals: Vec::new(),
                        weight,
                        ground_m,
                    });
                }
            }
        }
        let sockets: Vec<Socket> = rig
            .sockets(&joints)
            .into_iter()
            .map(|(joint, x)| {
                let g = world.mul(&x);
                Socket {
                    joint,
                    position: g.t.map(f64::from),
                    rotation: g.r.map(f64::from),
                }
            })
            .collect();
        if st.sockets != sockets {
            st.sockets = sockets;
        }
        if pose.joints != joints {
            pose.joints = joints;
        }
    }
}
