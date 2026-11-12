//! A walker played from behind (docs/spec/player.md 5.1): the `third_person` catalog's controls
//! land in [`ThirdPerson`], and `physics.third_person` (Control phase) turns them into the walker's
//! own before the rules and the walk read it: the move from its frame into the look's, the look
//! turned toward a lock, and the lock picked among [`Lockable`] entities or let go.

use std::collections::BTreeMap;

use amoris_sim::math;
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::{EntityId, EntityIndex, SimClock};
use bevy_ecs::prelude::{Component, Query, Res, With};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::avatar::Avatar;
use crate::body::Transform;
use crate::geom;
use crate::solver::Physics;
use crate::walker::Walker;

/// Within this distance a target need not be in front to be picked, metres.
const CLOSE_M: f64 = 4.0;
/// The least cosine of a target's bearing off the look's yaw for it to be in front.
const FRONT: f64 = 0.2;
/// A lock ends beyond this share of `lock_range_m`.
const KEEP: f64 = 1.5;

/// Which way a third-person move's `forward` points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoveFrame {
    /// Away from the camera: along the look's yaw.
    #[default]
    Camera,
    /// North (-z); `strafe` east (+x).
    World,
    /// Along the way the body faces.
    Body,
}

/// A third-person seat's inputs and lock (player.md 5.1), on its walker.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ThirdPerson {
    /// The move, -1 to 1 each, in `frame`; the engine writes the walker's `forward` and `strafe`
    /// from them every tick.
    pub forward: f64,
    pub strafe: f64,
    pub frame: MoveFrame,
    /// The camera arm's length as a share of its rig's `distance_m`, 0.5 to 2.
    pub zoom: f64,
    /// Held: guard or aim, as the game's rules say.
    pub guard: bool,
    /// One-tick pulses for the game's rules.
    pub dodge: bool,
    pub interact: bool,
    /// What `interact_with` named this tick, for the game's rules.
    pub interact_target: Option<EntityId>,
    /// One-tick pulse: lock on to the best target in front, or let go.
    pub lock_toggle: bool,
    /// One-tick pulse: lock on to this entity, if it is `Lockable`.
    pub lock_request: Option<EntityId>,
    /// The entity locked on to: engine-written, and rules may set or clear it.
    pub lock: Option<EntityId>,
    /// How far a lock reaches, metres; it ends at 1.5 times this.
    pub lock_range_m: f64,
    /// The fastest a lock turns the look, radians a second.
    pub lock_turn_rate: f64,
    /// The way the body faced at the tick's start, as a walker's yaw: its avatar's, else the
    /// look's. Engine-written.
    pub facing: f64,
}

impl Default for ThirdPerson {
    fn default() -> Self {
        ThirdPerson {
            forward: 0.0,
            strafe: 0.0,
            frame: MoveFrame::Camera,
            zoom: 1.0,
            guard: false,
            dodge: false,
            interact: false,
            interact_target: None,
            lock_toggle: false,
            lock_request: None,
            lock: None,
            lock_range_m: 20.0,
            lock_turn_rate: 8.0,
            facing: 0.0,
        }
    }
}

impl ThirdPerson {
    /// Whether a pulse is set (they are cleared after the tick they act in).
    pub fn pulsed(&self) -> bool {
        self.dodge
            || self.interact
            || self.lock_toggle
            || self.lock_request.is_some()
            || self.interact_target.is_some()
    }

    /// The same with every pulse cleared.
    pub fn cleared(&self) -> ThirdPerson {
        ThirdPerson {
            dodge: false,
            interact: false,
            lock_toggle: false,
            lock_request: None,
            interact_target: None,
            ..*self
        }
    }
}

/// Something a third-person seat may lock on to (player.md 5.1). A game removes it from what can
/// no longer be fought, which ends a lock on it.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lockable {}

/// The walker's `(forward, strafe)` for a move `(forward, strafe)` in a frame turned by `frame_yaw`,
/// for a look turned by `yaw` (both walker yaws: 0 along -z, positive turning left).
pub fn turned(forward: f64, strafe: f64, frame_yaw: f64, yaw: f64) -> (f64, f64) {
    let (sf, cf) = math::sin_cos(frame_yaw);
    let v = [-sf * forward + cf * strafe, -cf * forward - sf * strafe];
    let (sy, cy) = math::sin_cos(yaw);
    (
        math::clamp(-sy * v[0] - cy * v[1], -1.0, 1.0),
        math::clamp(cy * v[0] - sy * v[1], -1.0, 1.0),
    )
}

/// The walker yaw that looks from `from` toward `to`, or `None` when they stand on one vertical.
pub fn yaw_toward(from: [f64; 3], to: [f64; 3]) -> Option<f64> {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (dx * dx + dz * dz > 1e-6).then(|| math::atan2(-dx, -dz))
}

/// Whether a ray from `from` to `to` meets no fixed collider short of `to`.
fn in_sight(physics: &Physics, from: [f64; 3], to: [f64; 3]) -> bool {
    let d = geom::sub(to, from);
    let len = math::sqrt(geom::dot(d, d));
    if len < 1e-6 {
        return true;
    }
    let dir = geom::scale(d, 1.0 / len);
    let ray = rapier3d::prelude::Ray::new(geom::vec_in(from), geom::vec_in(dir));
    physics
        .world
        .cast_ray_and_get_normal(
            &ray,
            geom::f32_of(len),
            true,
            rapier3d::prelude::QueryFilter::only_fixed().exclude_sensors(),
        )
        .is_none_or(|(_, hit)| f64::from(hit.time_of_impact) >= len - 0.1)
}

/// The target `lock_toggle` picks for a walker whose eye is at `eye` looking along `yaw`: the
/// `Lockable` within range, in front and in sight with the least `distance * (1.6 - cosine)`.
fn pick(
    me: EntityId,
    eye: [f64; 3],
    yaw: f64,
    range_m: f64,
    own: &[EntityId],
    lockables: &[(EntityId, [f64; 3])],
    physics: Option<&Physics>,
) -> Option<EntityId> {
    let (sy, cy) = math::sin_cos(yaw);
    let (fx, fz) = (-sy, -cy);
    let mut best: Option<(f64, EntityId)> = None;
    for &(id, at) in lockables {
        if id == me || own.contains(&id) {
            continue;
        }
        let (dx, dz) = (at[0] - eye[0], at[2] - eye[2]);
        let d = math::sqrt(dx * dx + dz * dz);
        if d > range_m {
            continue;
        }
        let front = (dx * fx + dz * fz) / math::max(d, 0.01);
        if front < FRONT && d > CLOSE_M {
            continue;
        }
        if let Some(p) = physics
            && !in_sight(p, eye, at)
        {
            continue;
        }
        let score = d * (1.6 - front);
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id)
}

type Seat<'a> = (
    &'a EntityId,
    &'a mut ThirdPerson,
    &'a mut Walker,
    &'a Transform,
);

/// `physics.third_person` (Control, after `interface.intents`): every third-person walker's facing
/// read, its lock settled, its look turned toward the lock and its move written as the walker's.
pub(crate) fn drive(
    clock: Res<SimClock>,
    index: Res<EntityIndex>,
    physics: Option<Res<Physics>>,
    mut seats: Query<Seat<'static>>,
    avatars: Query<(&EntityId, &Avatar)>,
    lockables: Query<(&EntityId, &Transform), With<Lockable>>,
) {
    let dt = clock.dt();
    // Each walker's avatars, by id: the first gives its facing.
    let mut bodies: BTreeMap<EntityId, Vec<(EntityId, f64)>> = BTreeMap::new();
    for (id, a) in by_id(&avatars) {
        if let Some(w) = a.walker {
            bodies.entry(w).or_default().push((*id, a.yaw));
        }
    }
    let targets: Vec<(EntityId, [f64; 3])> =
        by_id(&lockables).map(|(id, t)| (*id, t.position)).collect();
    let target_at = |id: EntityId| -> Option<[f64; 3]> {
        let e = index.get(id)?;
        lockables.get(e).ok().map(|(_, t)| t.position)
    };
    for (id, mut tp, mut w, tf) in by_id_mut(&mut seats) {
        let mine = bodies.get(id);
        let own: Vec<EntityId> = mine.map_or(Vec::new(), |b| b.iter().map(|(a, _)| *a).collect());
        let facing = mine.and_then(|b| b.first()).map_or(w.yaw, |(_, yaw)| *yaw);
        let eye = tf.position;
        let mut lock = tp.lock;
        if tp.lock_toggle {
            lock = match lock {
                Some(_) => None,
                None => pick(
                    *id,
                    eye,
                    w.yaw,
                    tp.lock_range_m,
                    &own,
                    &targets,
                    physics.as_deref(),
                ),
            };
        }
        if let Some(t) = tp.lock_request
            && t != *id
            && target_at(t).is_some()
        {
            lock = Some(t);
        }
        // A lock ends when its target is gone, no longer lockable, or out of reach.
        let at = lock.and_then(target_at);
        let at = at.filter(|p| {
            let (dx, dz) = (p[0] - eye[0], p[2] - eye[2]);
            dx * dx + dz * dz <= (KEEP * tp.lock_range_m) * (KEEP * tp.lock_range_m)
        });
        if at.is_none() {
            lock = None;
        }
        let mut yaw = w.yaw;
        if let Some(goal) = at.and_then(|p| yaw_toward(eye, p)) {
            let d = math::wrap_angle(goal - yaw);
            let step = tp.lock_turn_rate * dt;
            yaw = math::wrap_angle(yaw + math::clamp(d, -step, step));
        }
        let (forward, strafe) = match tp.frame {
            MoveFrame::Camera => (
                math::clamp(tp.forward, -1.0, 1.0),
                math::clamp(tp.strafe, -1.0, 1.0),
            ),
            MoveFrame::World => turned(tp.forward, tp.strafe, 0.0, yaw),
            MoveFrame::Body => turned(tp.forward, tp.strafe, facing, yaw),
        };
        if w.yaw != yaw || w.forward != forward || w.strafe != strafe {
            w.yaw = yaw;
            w.forward = forward;
            w.strafe = strafe;
        }
        if tp.lock != lock || tp.facing != facing {
            tp.lock = lock;
            tp.facing = facing;
        }
    }
}

/// The target a third-person walker is locked on to while it does not run: what a `motion` avatar
/// faces.
pub(crate) fn lock_of(tp: Option<&ThirdPerson>, w: &Walker) -> Option<EntityId> {
    tp.and_then(|t| t.lock).filter(|_| !w.run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12
    }

    #[test]
    fn moves_turn_from_their_frame_into_the_looks() {
        // The same frame as the look: unchanged.
        assert!(close(turned(0.6, -0.3, 1.1, 1.1), (0.6, -0.3)));
        // North with the look turned to the west (yaw pi/2 looks along -x): north is to the right.
        assert!(close(
            turned(1.0, 0.0, 0.0, std::f64::consts::FRAC_PI_2),
            (0.0, 1.0)
        ));
        // East with the look to the south (yaw pi): east is to the left.
        assert!(close(
            turned(0.0, 1.0, 0.0, std::f64::consts::PI),
            (0.0, -1.0)
        ));
        // Ahead of a body facing the camera: toward the camera.
        assert!(close(
            turned(1.0, 0.0, std::f64::consts::PI, 0.0),
            (-1.0, 0.0)
        ));
    }

    #[test]
    fn a_toggle_picks_the_nearest_target_in_front() {
        let eye = [0.0, 1.6, 0.0];
        let ahead = (EntityId::new(5).unwrap(), [0.0, 1.6, -8.0]);
        let nearer_aside = (EntityId::new(6).unwrap(), [7.0, 1.6, -1.0]);
        let behind = (EntityId::new(7).unwrap(), [0.0, 1.6, 6.0]);
        let close_behind = (EntityId::new(8).unwrap(), [0.0, 1.6, 3.0]);
        let far = (EntityId::new(9).unwrap(), [0.0, 1.6, -30.0]);
        let me = EntityId::new(1).unwrap();
        let all = [ahead, nearer_aside, behind, far];
        assert_eq!(pick(me, eye, 0.0, 20.0, &[], &all, None), Some(ahead.0));
        // Behind but within 4 m counts, at a worse score (3 * 2.6) than the one ahead (8 * 0.6).
        let all = [ahead, close_behind];
        assert_eq!(pick(me, eye, 0.0, 20.0, &[], &all, None), Some(ahead.0));
        // Turned to the east, the one aside is in front.
        let all = [ahead, nearer_aside];
        assert_eq!(
            pick(me, eye, -std::f64::consts::FRAC_PI_2, 20.0, &[], &all, None),
            Some(nearer_aside.0)
        );
        // Nothing in range and in front.
        assert_eq!(pick(me, eye, 0.0, 20.0, &[], &[behind, far], None), None);
        // Never itself or its own body.
        let mine = (EntityId::new(2).unwrap(), [0.0, 0.0, -1.0]);
        assert_eq!(
            pick(me, eye, 0.0, 20.0, &[mine.0], &[mine, (me, eye)], None),
            None
        );
    }
}
