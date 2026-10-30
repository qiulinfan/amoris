//! Characters seen from outside (docs/spec/animation.md 4 and 6): graphs evaluated each tick into
//! poses, notifies, sockets and root motion (`animate`); bodies that stand at a walker's feet and
//! turn toward where it goes (`Avatar`); things held at a joint (`attach`); and the spring arm that
//! follows a walker's look from behind (`CameraRig`).

use std::collections::BTreeMap;

use amoris_assets::Model;
use amoris_assets::anim::{self, AnimGraph, AnimPose, AnimRigs, AnimState, Attach, Socket, Xform};
use amoris_sim::math;
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::{Emit, EntityId, EntityIndex, SimClock};
use bevy_ecs::prelude::{Commands, Component, Entity, Query, Res, Without};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::body::Transform;
use crate::geom;
use crate::solver::Physics;
use crate::walker::Walker;

/// How an avatar turns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Facing {
    /// Toward where the walker's controls move it (a third-person body).
    #[default]
    Motion,
    /// Along the walker's look.
    Look,
    /// Toward `target` (lock-on).
    Target,
    /// Where it faces now (an attack's committed direction).
    Fixed,
}

/// A visible body standing at a walker's feet (animation.md 6).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Avatar {
    /// The walker it stands for.
    pub walker: Option<EntityId>,
    pub facing: Facing,
    /// What it turns toward when `facing` is `target`.
    pub target: Option<EntityId>,
    /// The fastest it turns, radians a second.
    pub turn_rate: f64,
    /// Whether its graph's root motion moves the walker (in place of its controls) while it has
    /// any.
    pub root_motion: bool,
    /// The way it faces, as a walker's yaw (0 along -z, positive turning left). Engine-written.
    pub yaw: f64,
    /// Its horizontal speed over the last tick, m/s, and that motion in its own frame (x to its
    /// right, y ahead), m/s: what a graph's locomotion parameters read. Engine-written.
    pub speed: f64,
    pub local: [f64; 2],
    /// Where it stood after the last tick (for `speed`). Engine-written.
    pub last: [f64; 3],
}

impl Default for Avatar {
    fn default() -> Self {
        Avatar {
            walker: None,
            facing: Facing::Motion,
            target: None,
            turn_rate: 10.0,
            root_motion: true,
            yaw: 0.0,
            speed: 0.0,
            local: [0.0; 2],
            last: [0.0; 3],
        }
    }
}

/// A camera on a spring arm behind a walker's look (animation.md 6; Unreal's spring arm, Godot's
/// `SpringArm3D`).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct CameraRig {
    /// The walker whose look the arm follows.
    pub target: Option<EntityId>,
    /// The arm's pivot over the walker's feet, metres.
    pub pivot_m: f64,
    /// The camera's offset to the right of the arm, metres (over the shoulder).
    pub shoulder_m: f64,
    /// The arm's length, metres, and the shortest it is pulled to.
    pub distance_m: f64,
    pub min_distance_m: f64,
    /// The probe sphere's radius: how far from what it meets the camera stays, metres.
    pub probe_m: f64,
    /// How fast the arm lengthens again after an obstacle, metres a second (it shortens at once).
    pub return_mps: f64,
    /// An entity the arm turns toward instead of following the look (lock-on).
    pub look_at: Option<EntityId>,
    /// The arm's length now. Engine-written.
    pub length_m: f64,
    /// Shake, 0 to 1 (trauma, as Unreal's camera shakes are triggered): scripts raise it on a blow,
    /// it falls by `shake_decay` a second, and the camera trembles by its square, up to
    /// `shake_m` metres and `shake_rad` radians of look, on fixed sines of the clock's time.
    pub shake: f64,
    pub shake_decay: f64,
    pub shake_m: f64,
    pub shake_rad: f64,
}

impl Default for CameraRig {
    fn default() -> Self {
        CameraRig {
            target: None,
            pivot_m: 1.55,
            shoulder_m: 0.45,
            distance_m: 4.2,
            min_distance_m: 0.6,
            probe_m: 0.25,
            return_mps: 6.0,
            look_at: None,
            length_m: 4.2,
            shake: 0.0,
            shake_decay: 1.8,
            shake_m: 0.12,
            shake_rad: 0.035,
        }
    }
}

/// The quaternion of a turn by `yaw` about +y.
fn yaw_q(yaw: f64) -> [f64; 4] {
    let (s, c) = math::sin_cos(0.5 * yaw);
    [0.0, s, 0.0, c]
}

/// A model's facing for an avatar's yaw: models face +z, a walker's yaw 0 looks along -z.
fn body_rotation(yaw: f64) -> [f64; 4] {
    yaw_q(yaw + std::f64::consts::PI)
}

fn f32x3(v: [f64; 3]) -> [f32; 3] {
    [geom::f32_of(v[0]), geom::f32_of(v[1]), geom::f32_of(v[2])]
}

fn f32x4(v: [f64; 4]) -> [f32; 4] {
    [
        geom::f32_of(v[0]),
        geom::f32_of(v[1]),
        geom::f32_of(v[2]),
        geom::f32_of(v[3]),
    ]
}

/// The entity's frame: its transform and its model's scale.
fn frame(tf: &Transform, scale: [f64; 3]) -> Xform {
    Xform {
        t: f32x3(tf.position),
        r: f32x4(tf.rotation),
        s: f32x3(scale),
    }
}

/// The horizontal moves the avatars' root motion gives their walkers this tick, by walker.
pub(crate) fn drives(
    avatars: &Query<(&EntityId, &Avatar, &AnimState)>,
) -> BTreeMap<EntityId, [f64; 2]> {
    let mut out = BTreeMap::new();
    for (_, a, st) in by_id(avatars) {
        let [x, _, z] = st.root_motion;
        if a.root_motion
            && let Some(w) = a.walker
            && x * x + z * z > 0.0
        {
            out.insert(w, [x, z]);
        }
    }
    out
}

type AnimRow<'a> = (
    Entity,
    &'a EntityId,
    &'a AnimGraph,
    Option<&'a mut AnimState>,
    Option<&'a mut AnimPose>,
    Option<&'a Transform>,
    Option<&'a Model>,
);

/// `anim.graph` (Forces, first): every graph advanced by the tick; its pose kept for drawing, its
/// sockets and root motion in the world, its notifies emitted as `anim.notify`.
pub(crate) fn animate(
    clock: Res<SimClock>,
    rigs: Option<Res<AnimRigs>>,
    mut q: Query<AnimRow<'static>>,
    mut emit: Emit,
    mut commands: Commands,
) {
    let Some(rigs) = rigs else { return };
    let dt = clock.dt();
    for (entity, id, graph, state, pose, tf, model) in by_id_mut(&mut q) {
        let Some(rig) = rigs.rig(&graph.graph) else {
            continue;
        };
        let mut fresh = None;
        let st: &mut AnimState = match state {
            Some(s) if s.graph == graph.graph => s.into_inner(),
            Some(mut s) => {
                // Before the first evaluation a script may have set parameters or queued
                // montages (ctx.anim inserts the state): keep them over the graph's defaults.
                let mut init = rig.initial(&graph.graph);
                if s.graph.is_empty() {
                    for p in &s.params {
                        if let Some(q) = init.params.iter_mut().find(|q| q.name == p.name) {
                            q.value = p.value;
                        }
                    }
                    init.commands = std::mem::take(&mut s.commands);
                }
                *s = init;
                s.into_inner()
            }
            None => fresh.insert(rig.initial(&graph.graph)),
        };
        let step = if graph.playing { dt * graph.speed } else { 0.0 };
        let out = anim::graph::step(rig, &rigs.sets, st, step);
        let world = tf.map(|t| frame(t, model.map_or([1.0; 3], |m| m.scale)));
        st.sockets = rig
            .sockets(&out.pose)
            .into_iter()
            .map(|(joint, x)| {
                let g = world.map_or(x, |w| w.mul(&x));
                Socket {
                    joint,
                    position: g.t.map(f64::from),
                    rotation: g.r.map(f64::from),
                }
            })
            .collect();
        st.root_motion = match world {
            Some(w) => {
                let v = anim::math::rotate(
                    w.r,
                    [
                        w.s[0] * out.root_motion[0],
                        0.0,
                        w.s[2] * out.root_motion[2],
                    ],
                );
                [f64::from(v[0]), 0.0, f64::from(v[2])]
            }
            None => [0.0; 3],
        };
        for n in out.notices {
            let data = amoris_sim::PlainData::object(vec![
                ("clip".into(), amoris_sim::PlainData::String(n.clip)),
                ("name".into(), amoris_sim::PlainData::String(n.name)),
                (
                    "phase".into(),
                    amoris_sim::PlainData::String(n.phase.into()),
                ),
            ]);
            if let (Ok(kind), Ok(data)) = (amoris_sim::EventKind::new("anim.notify"), data) {
                emit.emit(amoris_sim::NewEvent::new(kind).subject(*id).data(data));
            }
        }
        match pose {
            Some(mut p) => p.joints = out.pose,
            None => {
                commands
                    .entity(entity)
                    .insert(AnimPose { joints: out.pose });
            }
        }
        if let Some(s) = fresh {
            commands.entity(entity).insert(s);
        }
    }
}

/// Things an avatar may face that are neither walkers nor avatars.
type Placed = (Without<Walker>, Without<Avatar>);

/// What holds an attached thing: its graph's state, its place, its model's scale and its graph.
type Holder<'a> = (
    &'a AnimState,
    &'a Transform,
    Option<&'a Model>,
    Option<&'a AnimGraph>,
);

/// The avatar's walker's motion this tick in the world, horizontal, per second: from its controls
/// as `physics.walk` reads them.
fn wanted(w: &Walker) -> [f64; 2] {
    let (f, s) = (
        math::clamp(w.forward, -1.0, 1.0),
        math::clamp(w.strafe, -1.0, 1.0),
    );
    let (sy, cy) = math::sin_cos(w.yaw);
    [-sy * f + cy * s, -cy * f - sy * s]
}

/// `physics.avatar` (Physics, after the walk): every avatar at its walker's feet, turned as its
/// `facing` says, its speed measured.
pub(crate) fn avatars(
    clock: Res<SimClock>,
    index: Res<EntityIndex>,
    walkers: Query<(&EntityId, &Walker, &Transform)>,
    mut avatars: Query<(&EntityId, &mut Avatar, &mut Transform), Without<Walker>>,
    others: Query<(&EntityId, &Transform), Placed>,
) {
    let dt = clock.dt();
    let walker = |id: EntityId| {
        index
            .get(id)
            .and_then(|e| walkers.get(e).ok())
            .map(|(_, w, t)| (*w, t.position))
    };
    let target_pos = |id: EntityId| -> Option<[f64; 3]> {
        let e = index.get(id)?;
        walkers
            .get(e)
            .map(|(_, _, t)| t.position)
            .ok()
            .or_else(|| others.get(e).ok().map(|(_, t)| t.position))
    };
    for (_, mut a, mut tf) in by_id_mut(&mut avatars) {
        let Some((w, eye)) = a.walker.and_then(walker) else {
            continue;
        };
        let feet = [eye[0], eye[1] - w.eye_m, eye[2]];
        let goal = match a.facing {
            Facing::Look => Some(w.yaw),
            Facing::Fixed => None,
            Facing::Motion => {
                let [vx, vz] = wanted(&w);
                (vx * vx + vz * vz > 0.01).then(|| math::atan2(-vx, -vz))
            }
            Facing::Target => a.target.and_then(target_pos).and_then(|p| {
                let (dx, dz) = (p[0] - feet[0], p[2] - feet[2]);
                (dx * dx + dz * dz > 1e-6).then(|| math::atan2(-dx, -dz))
            }),
        };
        let mut yaw = a.yaw;
        if let Some(goal) = goal {
            let d = math::wrap_angle(goal - yaw);
            let step = a.turn_rate * dt;
            yaw = math::wrap_angle(yaw + math::clamp(d, -step, step));
        }
        let moved = [feet[0] - a.last[0], feet[2] - a.last[2]];
        let (vx, vz) = (moved[0] / dt, moved[1] / dt);
        let speed = math::sqrt(vx * vx + vz * vz);
        // The motion in the body's frame: ahead is (-sin yaw, -cos yaw), right is (cos, -sin).
        let (sy, cy) = math::sin_cos(yaw);
        let local = [vx * cy - vz * sy, -vx * sy - vz * cy];
        let next = Avatar {
            yaw,
            speed,
            local,
            last: feet,
            ..a.clone()
        };
        if *a != next {
            *a = next;
        }
        let pose = Transform {
            position: feet,
            rotation: body_rotation(yaw),
        };
        if *tf != pose {
            *tf = pose;
        }
    }
}

/// `anim.attach` (Finish): every attached entity at its holder's joint, after the holder moved.
pub(crate) fn attach(
    index: Res<EntityIndex>,
    holders: Query<Holder<'static>, Without<Attach>>,
    rigs: Option<Res<AnimRigs>>,
    poses: Query<&AnimPose>,
    mut held: Query<(&EntityId, &Attach, &mut Transform)>,
) {
    let Some(rigs) = rigs else { return };
    for (_, at, mut tf) in by_id_mut(&mut held) {
        let Some(e) = at.to.and_then(|id| index.get(id)) else {
            continue;
        };
        let Ok((_, htf, model, graph)) = holders.get(e) else {
            continue;
        };
        let (Some(graph), Ok(pose)) = (graph, poses.get(e)) else {
            continue;
        };
        let Some(rig) = rigs.rig(&graph.graph) else {
            continue;
        };
        let Some(j) = rig.skeleton.find(&at.joint) else {
            continue;
        };
        if pose.joints.len() != rig.skeleton.joints.len() {
            continue;
        }
        let world = frame(htf, model.map_or([1.0; 3], |m| m.scale));
        let joint = world.mul(&rig.skeleton.global(&pose.joints, j));
        let offset = Xform {
            t: f32x3(at.position),
            r: f32x4(at.rotation),
            s: [1.0; 3],
        };
        // The joint's scale (the model's) is left out of what it holds.
        let held_at = Xform {
            s: [1.0; 3],
            ..joint
        }
        .mul(&offset);
        let next = Transform {
            position: held_at.t.map(f64::from),
            rotation: held_at.r.map(f64::from),
        };
        if *tf != next {
            *tf = next;
        }
    }
}

/// `camera.rig` (Finish): every rigged camera behind its walker's look, its arm shortened to the
/// first fixed collider a probe sphere meets on the way out.
pub(crate) fn rigs(
    clock: Res<SimClock>,
    index: Res<EntityIndex>,
    walkers: Query<(&Walker, &Transform)>,
    targets: Query<&Transform, Without<CameraRig>>,
    mut cams: Query<(&EntityId, &mut CameraRig, &mut Transform), Without<Walker>>,
    physics: Option<Res<Physics>>,
) {
    let dt = clock.dt();
    for (_, mut rig, mut tf) in by_id_mut(&mut cams) {
        let Some(e) = rig.target.and_then(|id| index.get(id)) else {
            continue;
        };
        let Ok((w, wtf)) = walkers.get(e) else {
            continue;
        };
        let feet = [wtf.position[0], wtf.position[1] - w.eye_m, wtf.position[2]];
        let mut look = Walker {
            pitch: w.pitch,
            yaw: w.yaw,
            ..*w
        };
        if let Some(p) = rig
            .look_at
            .and_then(|id| index.get(id))
            .and_then(|e| targets.get(e).ok())
        {
            let (dx, dz) = (p.position[0] - feet[0], p.position[2] - feet[2]);
            if dx * dx + dz * dz > 1e-6 {
                look.yaw = math::atan2(-dx, -dz);
                look.pitch = -0.25;
            }
        }
        let dir = look.look();
        let (sy, cy) = math::sin_cos(look.yaw);
        let right = [cy, 0.0, -sy];
        let pivot = [
            feet[0] + right[0] * rig.shoulder_m,
            feet[1] + rig.pivot_m,
            feet[2] + right[2] * rig.shoulder_m,
        ];
        let back = [-dir[0], -dir[1], -dir[2]];
        let free = physics.as_deref().map_or(rig.distance_m, |p| {
            probe(p, pivot, back, rig.distance_m, rig.probe_m)
        });
        let want = math::clamp(free, rig.min_distance_m, rig.distance_m);
        let length = if want < rig.length_m {
            want
        } else {
            math::min(want, rig.length_m + rig.return_mps * dt)
        };
        if rig.length_m != length {
            rig.length_m = length;
        }
        // The shake: incommensurate sines of the clock's time, so it never repeats visibly and
        // replays the same.
        let mut jolt = [0.0; 3];
        if rig.shake > 0.0 {
            let k = rig.shake * rig.shake;
            let t = clock.time();
            let wave = |a: f64, b: f64| 0.6 * math::sin(a * t) + 0.4 * math::sin(b * t + 1.3);
            jolt = [
                k * rig.shake_m * wave(37.0, 61.0),
                k * rig.shake_m * wave(43.0, 71.0),
                k * rig.shake_m * wave(31.0, 53.0),
            ];
            look.yaw += k * rig.shake_rad * wave(29.0, 47.0);
            look.pitch += k * rig.shake_rad * wave(41.0, 67.0);
            rig.shake = math::max(0.0, rig.shake - rig.shake_decay * dt);
        }
        let pose = Transform {
            position: [
                pivot[0] + back[0] * length + jolt[0],
                pivot[1] + back[1] * length + jolt[1],
                pivot[2] + back[2] * length + jolt[2],
            ],
            rotation: look.rotation(),
        };
        if *tf != pose {
            *tf = pose;
        }
    }
}

/// How far a sphere of radius `r` moves from `from` along the unit `dir` before it meets a fixed
/// collider, up to `max` metres.
fn probe(physics: &Physics, from: [f64; 3], dir: [f64; 3], max: f64, r: f64) -> f64 {
    let options = rapier3d::parry::query::ShapeCastOptions {
        max_time_of_impact: geom::f32_of(max),
        target_distance: 0.0,
        stop_at_penetration: false,
        compute_impact_geometry_on_penetration: false,
    };
    let shape = rapier3d::parry::shape::Ball::new(geom::f32_of(r));
    let pose = rapier3d::math::Pose::from_translation(geom::vec_in(from));
    match physics.world.cast_shape(
        &pose,
        geom::vec_in(dir),
        &shape,
        options,
        rapier3d::prelude::QueryFilter::only_fixed(),
    ) {
        Some((_, hit)) => f64::from(hit.time_of_impact),
        None => max,
    }
}
