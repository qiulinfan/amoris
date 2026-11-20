//! Ragdolls (animation.md 6b): a `Ragdoll` on an animated entity whose graph names one. While it
//! is `active`, `anim.ragdoll.bodies` makes a body entity per ragdoll bone where the animated pose
//! has the bone (capsule, rigid body, the joint to its parent body, `RagdollBone`); the solver moves
//! them like any body; `anim.ragdoll` draws the bones where the bodies are, mixed into the animated
//! pose by `weight`, and keeps the entity's place under the root body. Off, the weight falls over
//! `blend_s` and the bodies go when it reaches 0.

use amoris_assets::Model;
use amoris_assets::anim::math as am;
use amoris_assets::anim::{AnimGraph, AnimPose, AnimRigs, AnimState, Socket, Xform};
use amoris_sim::math;
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::{EntityId, EntityIndex, SimClock, SystemCtx};
use bevy_ecs::prelude::{Component, Query, Res, Without, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::avatar::{Avatar, frame};
use crate::body::{
    Collider, CollisionFilter, ExternalForce, RigidBody, Shape, Transform, Velocity,
};
use crate::geom::{self, V3};
use crate::joints::{Joint, JointKind};
use crate::walker::Walker;

/// An animated entity's ragdoll (animation.md 6b): scripts turn it on and off; the engine keeps its
/// blend weight and its root body.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Ragdoll {
    /// Limp: the bodies move the bones.
    pub active: bool,
    /// Seconds the pose takes to return to the animation once `active` is off.
    pub blend_s: f64,
    /// How much of the pose the bodies give, 0 to 1. Engine-written.
    pub weight: f64,
    /// The root body's entity while there are bodies. Engine-written.
    pub root: Option<EntityId>,
}

impl Default for Ragdoll {
    fn default() -> Self {
        Ragdoll {
            active: false,
            blend_s: 0.3,
            weight: 0.0,
            root: None,
        }
    }
}

/// A ragdoll's body: whose it is and which of its graph's bodies (in the ragdoll's joined order).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RagdollBone {
    pub owner: EntityId,
    pub body: u32,
}

/// Every ragdoll body as (owner, its entity, its index), ascending.
#[allow(clippy::disallowed_methods)] // collected, then sorted by id
fn bodies_of(world: &mut World) -> Vec<(EntityId, EntityId, u32)> {
    let mut q = world.query::<(&EntityId, &RagdollBone)>();
    let mut out: Vec<(EntityId, EntityId, u32)> = q
        .iter(world)
        .map(|(id, b)| (b.owner, *id, b.body))
        .collect();
    out.sort_unstable();
    out
}

/// A body's world velocity at spawn: an avatar's measured horizontal motion and its walker's
/// vertical speed, else still.
fn motion(avatar: Option<&Avatar>, walker: Option<&Walker>) -> V3 {
    let Some(a) = avatar else { return [0.0; 3] };
    // Ahead is (-sin yaw, -cos yaw), right is (cos, -sin) (avatar.rs).
    let (sy, cy) = math::sin_cos(a.yaw);
    let [x, y] = a.local;
    [
        x * cy - y * sy,
        walker.map_or(0.0, |w| w.vertical_mps),
        -x * sy - y * cy,
    ]
}

/// `anim.ragdoll.bodies` (Physics, after the step and the walkers, before `physics.avatar`): removes
/// the bodies a ragdoll no longer has a use for and makes those an active one lacks, from this
/// tick's animated pose; the next tick's step takes the new ones in.
#[allow(clippy::disallowed_methods)] // the owners and walkers are collected, then sorted by id
pub(crate) fn bodies(world: &mut World, ctx: &mut SystemCtx<'_>) {
    let bones = bodies_of(world);
    let Some(rigs) = world.get_resource::<AnimRigs>().cloned() else {
        // Without rigs no ragdoll has bodies to keep.
        for (_, id, _) in bones {
            if let Err(p) = amoris_sim::entity::despawn(world, id) {
                ctx.fault(p);
                return;
            }
        }
        return;
    };
    // The owners, ascending.
    struct Owner {
        id: EntityId,
        active: bool,
        weight: f64,
        graph: String,
        world: Option<Xform>,
        pose: Option<Vec<Xform>>,
        velocity: V3,
        root: Option<EntityId>,
    }
    let mut owners: Vec<Owner> = Vec::new();
    {
        let mut walkers: Vec<(EntityId, Walker)> = world
            .query::<(&EntityId, &Walker)>()
            .iter(world)
            .map(|(id, w)| (*id, *w))
            .collect();
        walkers.sort_unstable_by_key(|w| w.0);
        let mut q = world.query::<(
            &EntityId,
            &Ragdoll,
            &AnimGraph,
            Option<&AnimPose>,
            Option<&Transform>,
            Option<&Model>,
            Option<&Avatar>,
        )>();
        for (id, r, g, pose, tf, model, avatar) in q.iter(world) {
            let walker = avatar
                .and_then(|a| a.walker)
                .and_then(|w| walkers.binary_search_by_key(&w, |x| x.0).ok())
                .map(|i| &walkers[i].1);
            owners.push(Owner {
                id: *id,
                active: r.active,
                weight: r.weight,
                graph: g.graph.clone(),
                world: tf.map(|t| frame(t, model.map_or([1.0; 3], |m| m.scale))),
                pose: pose.map(|p| p.joints.clone()),
                velocity: motion(avatar, walker),
                root: r.root,
            });
        }
    }
    owners.sort_unstable_by_key(|o| o.id);
    let owner_of = |id: EntityId| owners.binary_search_by_key(&id, |o| o.id).ok();
    // Which owners keep their bodies: a ragdoll in use whose set is whole.
    let mut keep = vec![false; owners.len()];
    for (i, o) in owners.iter().enumerate() {
        let Some(rd) = rigs.rig(&o.graph).and_then(|r| r.ragdoll()) else {
            continue;
        };
        let mine: Vec<u32> = bones.iter().filter(|b| b.0 == o.id).map(|b| b.2).collect();
        let whole =
            mine.len() == rd.bodies.len() && mine.iter().enumerate().all(|(k, &b)| b as usize == k);
        keep[i] = whole && (o.active || o.weight > 0.0);
    }
    for &(owner, id, _) in &bones {
        if owner_of(owner).is_none_or(|i| !keep[i])
            && let Err(p) = amoris_sim::entity::despawn(world, id)
        {
            ctx.fault(p);
            return;
        }
    }
    for (i, o) in owners.iter().enumerate() {
        let made = if keep[i] {
            o.root
        } else if o.active
            && let (Some(rd), Some(w), Some(pose)) = (
                rigs.rig(&o.graph).and_then(|r| r.ragdoll().map(|d| (r, d))),
                o.world,
                o.pose.as_ref(),
            )
            && pose.len() == rd.0.skeleton.joints.len()
        {
            let (rig, rd) = rd;
            let g = rig.skeleton.globals(pose);
            let s = f64::from(w.s[0]);
            let mut ids: Vec<EntityId> = Vec::with_capacity(rd.bodies.len());
            for (k, b) in rd.bodies.iter().enumerate() {
                let f = b.frame(&g[b.joint]);
                let at = w.apply(f.t);
                let turn = am::normalize(am::qmul(w.r, f.r));
                let (r, h) = (f64::from(b.radius) * s, f64::from(b.half_height) * s);
                let volume = std::f64::consts::PI * r * r * (2.0 * h + 4.0 / 3.0 * r);
                let collider = Collider {
                    shape: Shape::Capsule {
                        half_height: h,
                        radius: r,
                    },
                    density: Some(f64::from(b.mass) / volume),
                    friction: rd.friction,
                    restitution: 0.0,
                };
                let body = RigidBody {
                    linear_damping: rd.linear_damping,
                    angular_damping: rd.angular_damping,
                    ..RigidBody::dynamic()
                };
                let place = Transform {
                    position: at.map(f64::from),
                    rotation: turn.map(f64::from),
                };
                let bone = RagdollBone {
                    owner: o.id,
                    body: u32::try_from(k).unwrap_or(u32::MAX),
                };
                let velocity = Velocity {
                    linear: o.velocity,
                    angular: [0.0; 3],
                };
                let filter = CollisionFilter {
                    sensor: false,
                    groups: rd.groups,
                    mask: rd.mask,
                };
                let spawned = match b.parent {
                    None => amoris_sim::entity::spawn(
                        world,
                        (
                            place,
                            velocity,
                            ExternalForce::default(),
                            body,
                            collider,
                            filter,
                            bone,
                        ),
                    ),
                    Some(p) => {
                        let joint = Joint {
                            kind: if b.hinge {
                                JointKind::Revolute
                            } else {
                                JointKind::Spherical
                            },
                            with: ids[p],
                            anchor: b.anchor.map(|x| f64::from(x) * s),
                            other_anchor: b.other_anchor.map(|x| f64::from(x) * s),
                            axis: b.axis.map(f64::from),
                            limits: Some(b.limits),
                            contacts: false,
                            rest: b.rest.map(f64::from),
                            swing: (!b.hinge).then_some(b.swing),
                        };
                        amoris_sim::entity::spawn(
                            world,
                            (
                                place,
                                velocity,
                                ExternalForce::default(),
                                body,
                                collider,
                                filter,
                                bone,
                                joint,
                            ),
                        )
                    }
                };
                match spawned {
                    Ok(id) => ids.push(id),
                    Err(p) => {
                        ctx.fault(p);
                        return;
                    }
                }
            }
            ids.first().copied()
        } else {
            None
        };
        let Some(e) = world.resource::<EntityIndex>().get(o.id) else {
            continue;
        };
        if let Some(mut r) = world.get_mut::<Ragdoll>(e) {
            let weight = if made.is_some() && o.active {
                1.0
            } else {
                r.weight
            };
            let weight = if made.is_none() { 0.0 } else { weight };
            if r.root != made || r.weight != weight {
                r.root = made;
                r.weight = weight;
            }
        }
    }
}

type Row<'a> = (
    &'a EntityId,
    &'a mut Ragdoll,
    &'a AnimGraph,
    &'a mut AnimState,
    &'a mut AnimPose,
    &'a mut Transform,
    Option<&'a Model>,
);

/// `anim.ragdoll` (Physics, after `anim.ik`): the bones with bodies turned and placed where their
/// bodies are, mixed into the animated pose by the weight; the entity kept under its root body;
/// the sockets from the result; the weight moved for the next tick.
pub(crate) fn pose(
    clock: Res<SimClock>,
    rigs: Option<Res<AnimRigs>>,
    parts: Query<(&EntityId, &RagdollBone, &Transform)>,
    mut q: Query<Row<'static>, Without<RagdollBone>>,
) {
    let Some(rigs) = rigs else { return };
    let dt = clock.dt();
    // Every body's place, by owner and index.
    let mut all: Vec<(EntityId, u32, Transform)> = by_id(&parts)
        .map(|(_, b, t)| (b.owner, b.body, *t))
        .collect();
    if all.is_empty() {
        return;
    }
    all.sort_unstable_by_key(|x| (x.0, x.1));
    for (id, mut r, graph, mut st, mut pose, mut tf, model) in by_id_mut(&mut q) {
        if r.root.is_none() {
            continue;
        }
        let Some(rig) = rigs.rig(&graph.graph) else {
            continue;
        };
        let Some(rd) = rig.ragdoll() else { continue };
        if pose.joints.len() != rig.skeleton.joints.len() {
            continue;
        }
        let from = all.partition_point(|x| x.0 < *id);
        let frames: Vec<Transform> = all[from..]
            .iter()
            .take_while(|x| x.0 == *id)
            .map(|x| x.2)
            .collect();
        if frames.len() != rd.bodies.len() {
            continue;
        }
        let scale = model.map_or([1.0; 3], |m| m.scale);
        // The entity under its root body: where the root body's rest frame would put it.
        let rest0 = rd.root_rest;
        let w0 = frame(&tf, scale);
        let off = am::rotate(
            w0.r,
            [
                rest0.t[0] * w0.s[0],
                rest0.t[1] * w0.s[1],
                rest0.t[2] * w0.s[2],
            ],
        );
        let p = frames[0].position;
        let at = [
            p[0] - f64::from(off[0]),
            p[1] - f64::from(off[1]),
            p[2] - f64::from(off[2]),
        ];
        if tf.position != at {
            tf.position = at;
        }
        let w = frame(&tf, scale);
        // Each body's frame in the model's space.
        let inv = am::qconj(w.r);
        let bodies: Vec<Xform> = frames
            .iter()
            .map(|t| {
                let d = am::rotate(inv, am::sub(geom_f32(t.position), w.t));
                Xform {
                    t: [d[0] / w.s[0], d[1] / w.s[1], d[2] / w.s[2]],
                    r: am::normalize(am::qmul(inv, geom_f32q(t.rotation))),
                    s: [1.0; 3],
                }
            })
            .collect();
        let limp = rd.pose(&rig.skeleton, &pose.joints, &bodies);
        let u = geom::f32_of(r.weight);
        let joints: Vec<Xform> = if u >= 1.0 {
            limp
        } else {
            pose.joints
                .iter()
                .zip(&limp)
                .map(|(a, b)| Xform::mix(a, b, u))
                .collect()
        };
        let sockets: Vec<Socket> = rig
            .sockets(&joints)
            .into_iter()
            .map(|(joint, x)| {
                let g = w.mul(&x);
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
        let next = if r.active {
            1.0
        } else if r.blend_s > 0.0 {
            math::max(r.weight - dt / r.blend_s, 0.0)
        } else {
            0.0
        };
        if r.weight != next {
            r.weight = next;
        }
    }
}

fn geom_f32(v: V3) -> am::V3 {
    [geom::f32_of(v[0]), geom::f32_of(v[1]), geom::f32_of(v[2])]
}

fn geom_f32q(q: [f64; 4]) -> am::Q {
    [
        geom::f32_of(q[0]),
        geom::f32_of(q[1]),
        geom::f32_of(q[2]),
        geom::f32_of(q[3]),
    ]
}
