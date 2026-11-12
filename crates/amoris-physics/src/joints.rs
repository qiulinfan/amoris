//! Joints (simulation.md 8.5): a `Joint` on a body's entity ties that body to another's with one
//! of Rapier's impulse joints, fixed, revolute, prismatic or spherical. `physics.step` keeps the
//! solver's joints in step with the components after the bodies' sync, in `EntityId` order: a joint
//! whose component changed, whose entity lost it, or whose bodies were rebuilt is removed, then the
//! missing ones are inserted. Each joint carries its entity in the low 64 bits of its `user_data`
//! and a hash of its `Joint` in the high 64 bits, inside Rapier's serialized state, so the physics
//! cache carries the joints and nothing derived needs rebuilding. A world without a `Joint`
//! builds, steps and hashes as before.

use amoris_contract::{Problem, detail};
use amoris_sim::EntityId;
use bevy_ecs::prelude::Component;
use rapier3d::prelude::{
    FixedJointBuilder, GenericJoint, ImpulseJointHandle, PhysicsWorld, PrismaticJointBuilder,
    RevoluteJointBuilder, RigidBodyHandle, SphericalJointBuilder,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::geom::{self, V3};
use crate::solver::{BodyIndex, entity_of, spec_of};

/// What a joint lets the two bodies do relative to each other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum JointKind {
    /// Nothing: the two move as one.
    #[default]
    Fixed,
    /// Turn about `axis` (a hinge, a wheel's axle).
    Revolute,
    /// Slide along `axis` (a piston, a drawer).
    Prismatic,
    /// Turn every way about the anchor (a ball and socket, a rope's link).
    Spherical,
}

/// Ties the body on this entity to the body on `with`. The anchors are points in each body's own
/// frame where the joint sits; `axis` is in this body's frame. Rotations are held as the bodies'
/// frames align: a `Fixed` joint keeps the two bodies turned alike, so place them turned alike.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Joint {
    #[serde(default)]
    pub kind: JointKind,
    /// The other body's entity.
    pub with: EntityId,
    /// The joint in this body's frame, metres.
    #[serde(default)]
    pub anchor: V3,
    /// The joint in the other body's frame, metres.
    #[serde(default)]
    pub other_anchor: V3,
    /// A revolute joint's turning axis or a prismatic joint's sliding axis, in this body's frame
    /// (default +x); its length does not matter, but it is not zero.
    #[serde(default = "x_axis")]
    pub axis: V3,
    /// A revolute joint's turn (radians) or a prismatic joint's travel (metres) from the frames'
    /// alignment, `[min, max]`; none: free.
    #[serde(default)]
    pub limits: Option<[f64; 2]>,
    /// Whether the two bodies still collide with each other (default no).
    #[serde(default)]
    pub contacts: bool,
}

fn x_axis() -> V3 {
    [1.0, 0.0, 0.0]
}

impl Joint {
    /// A joint of `kind` to `with`, at both bodies' origins.
    pub fn new(kind: JointKind, with: EntityId) -> Joint {
        Joint {
            kind,
            with,
            anchor: geom::ZERO,
            other_anchor: geom::ZERO,
            axis: x_axis(),
            limits: None,
            contacts: false,
        }
    }
}

fn joint_problem(entity: EntityId, reason: &str) -> Problem {
    Problem::new(
        "sim.joint_invalid",
        format!("Entity {entity}: its Joint {reason}."),
        detail([("entity", json!(entity.get())), ("reason", json!(reason))]),
    )
}

/// The joint's checks, as the solver receives its numbers (`f32`): anchors finite, an axis that is
/// not zero for the kinds that use one, limits finite with `min <= max`, never the entity itself.
pub fn validate(entity: EntityId, j: &Joint) -> Result<(), Problem> {
    if j.with == entity {
        return Err(joint_problem(entity, "names its own entity"));
    }
    if !(geom::finite32(&j.anchor) && geom::finite32(&j.other_anchor)) {
        return Err(joint_problem(
            entity,
            "has an anchor the solver's f32 cannot hold",
        ));
    }
    if matches!(j.kind, JointKind::Revolute | JointKind::Prismatic) {
        let a = j.axis;
        let len2 = a[0] * a[0] + a[1] * a[1] + a[2] * a[2];
        if !(geom::finite32(&a) && geom::f32_of(len2) > 0.0) {
            return Err(joint_problem(entity, "needs a non-zero, finite axis"));
        }
    }
    if let Some([lo, hi]) = j.limits {
        let (lo, hi) = (geom::f32_of(lo), geom::f32_of(hi));
        if !(lo.is_finite() && hi.is_finite() && lo <= hi) {
            return Err(joint_problem(
                entity,
                "has limits that are not finite with min at most max",
            ));
        }
    }
    Ok(())
}

/// A hash of the joint, kept in its `user_data` to see a change.
fn spec_hash(j: &Joint) -> u64 {
    let bytes = bincode::serialize(j).unwrap_or_default();
    amoris_sim::rng::fnv1a64_extend(0xcbf2_9ce4_8422_2325, &bytes)
}

/// The solver's joint for a `Joint` of `entity`.
fn build(entity: EntityId, j: &Joint) -> GenericJoint {
    let (a1, a2) = (geom::vec_in(j.anchor), geom::vec_in(j.other_anchor));
    let axis = || {
        let a = j.axis;
        let len = amoris_sim::math::sqrt(a[0] * a[0] + a[1] * a[1] + a[2] * a[2]);
        geom::vec_in([a[0] / len, a[1] / len, a[2] / len])
    };
    let limits = j
        .limits
        .map(|[lo, hi]| [geom::f32_of(lo), geom::f32_of(hi)]);
    let mut g: GenericJoint = match j.kind {
        JointKind::Fixed => FixedJointBuilder::new()
            .local_anchor1(a1)
            .local_anchor2(a2)
            .contacts_enabled(j.contacts)
            .into(),
        JointKind::Revolute => {
            let mut b = RevoluteJointBuilder::new(axis())
                .local_anchor1(a1)
                .local_anchor2(a2)
                .contacts_enabled(j.contacts);
            if let Some(l) = limits {
                b = b.limits(l);
            }
            b.into()
        }
        JointKind::Prismatic => {
            let mut b = PrismaticJointBuilder::new(axis())
                .local_anchor1(a1)
                .local_anchor2(a2)
                .contacts_enabled(j.contacts);
            if let Some(l) = limits {
                b = b.limits(l);
            }
            b.into()
        }
        JointKind::Spherical => SphericalJointBuilder::new()
            .local_anchor1(a1)
            .local_anchor2(a2)
            .contacts_enabled(j.contacts)
            .into(),
    };
    g.user_data = u128::from(entity.get()) | (u128::from(spec_hash(j)) << 64);
    g
}

/// `physics.step`'s joint sync, after the bodies': `joints` are the entities with a `Joint`,
/// ascending by id. Removes the solver's joints that no longer match (their entity lost its
/// `Joint`, the component changed, a body was rebuilt), then inserts the missing ones in id order.
/// A joint whose bodies are not both in the solver is not built and reported with the entity
/// every tick, like a body that cannot be built.
pub(crate) fn sync(
    world: &mut PhysicsWorld,
    index: &BodyIndex,
    joints: &[(EntityId, &Joint)],
    report: &mut dyn FnMut(EntityId, Problem),
) {
    // What the solver holds, by entity.
    let mut held: Vec<(
        EntityId,
        ImpulseJointHandle,
        u64,
        RigidBodyHandle,
        RigidBodyHandle,
    )> = world
        .impulse_joints()
        .filter_map(|(h, j)| {
            entity_of(j.data.user_data)
                .map(|id| (id, h, spec_of(j.data.user_data), j.body1(), j.body2()))
        })
        .collect();
    if held.is_empty() && joints.is_empty() {
        return;
    }
    held.sort_unstable_by_key(|h| h.0);
    let mut keep = Vec::with_capacity(joints.len());
    let mut stale = Vec::new();
    for (id, h, spec, b1, b2) in &held {
        let want = joints
            .binary_search_by_key(id, |j| j.0)
            .ok()
            .map(|i| joints[i].1);
        let same = want.is_some_and(|j| {
            spec_hash(j) == *spec && index.get(*id) == Some(*b1) && index.get(j.with) == Some(*b2)
        });
        if same {
            keep.push(*id);
        } else {
            stale.push(*h);
        }
    }
    for h in stale {
        world.remove_impulse_joint(h);
    }
    for (id, j) in joints {
        if keep.binary_search(id).is_ok() {
            continue;
        }
        if let Err(p) = validate(*id, j) {
            report(*id, p);
            continue;
        }
        let (Some(b1), Some(b2)) = (index.get(*id), index.get(j.with)) else {
            report(
                *id,
                joint_problem(
                    *id,
                    "ties bodies the solver does not have: both entities need a \
                                    Transform and a Collider",
                ),
            );
            continue;
        };
        world.insert_impulse_joint(b1, b2, build(*id, j));
    }
}
