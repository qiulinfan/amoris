//! Boards a Walker rides: surfboards, rafts, bodyboards (water.md 50). The board is a dynamic
//! body with a Floater, so buoyancy and Froude-Krylov pressure come from `physics.buoyancy`;
//! `physics.board` adds the hull's own hydrodynamics on the same points, in the board's frame: a
//! flat plate's normal pressure (planing lift, linear in the angle of attack and quadratic in
//! speed), friction along it and the rails' and fin's grip across it. Its rider's inputs paddle
//! (lying), carve (standing) and pop up or lie down. `physics.mount` then carries the rider with
//! the integrated board and throws them off when the board turns over.
//!
//! The board's frame is the boat's: forward is -z, up +y, right +x.

use amoris_contract::Problem;
use amoris_sim::math::{self, clamp};
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::{EntityId, SimClock};
use bevy_ecs::prelude::{Component, Query, Res, Without};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::boat::Floater;
use crate::body::{ExternalForce, RigidBody, Transform, Velocity, local_com};
use crate::geom::{self, V3};
use crate::sea::Sea;
use crate::solver::velocity_at;
use crate::walker::Walker;

/// The rider's eye over the board's deck while lying down, metres.
pub const LYING_EYE_M: f64 = 0.4;
/// A board whose deck faces less upward than this (the y of its up axis) has thrown its rider.
pub const WIPEOUT_UP: f64 = 0.25;
/// The fastest an arm stroke pushes water past the board, m/s: paddling thrust falls to nothing
/// as the board's speed through the water approaches it.
pub const STROKE_MPS: f64 = 2.4;

/// A board and its rider's controls (water.md 50). Coefficients are per square metre of wetted
/// bottom; each Floater point stands for an equal share of `area_m2`.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Board {
    /// Bottom area, square metres.
    pub area_m2: f64,
    /// Normal-force coefficient of the bottom (a flat plate's is about 1 to 2).
    pub planing: f64,
    /// Friction coefficient along the board.
    pub glide: f64,
    /// Coefficient of the rails' grip across the board.
    pub rail: f64,
    /// The fin: where it sits in the board's frame and its area, square metres (0: none).
    pub fin_at: V3,
    pub fin_m2: f64,
    /// Paddling thrust at full forward input while lying down, newtons.
    pub paddle_n: f64,
    /// Yaw torque per unit of lean while standing, at speed, newton metres.
    pub carve_nm: f64,
    /// Roll torque per unit of lean, newton metres.
    pub roll_nm: f64,
    /// Where the rider's feet are, in the board's frame.
    pub seat: V3,
    /// Written by the engine: standing up (else lying down to paddle).
    pub standing: bool,
    /// Written by the engine: the rider, if any.
    pub rider: Option<EntityId>,
    /// Written by the engine: horizontal speed, m/s.
    pub speed: f64,
    /// Written by the engine: speed through the water along the board, m/s.
    pub through_water: f64,
    /// Written by the engine: how many times the board has thrown its rider.
    pub wipeouts: u32,
}

impl Default for Board {
    fn default() -> Self {
        // A 2.7 m longboard: 0.55 m^2 of bottom, a 0.02 m^2 fin near the tail.
        Board {
            area_m2: 0.55,
            planing: 1.6,
            glide: 0.02,
            rail: 0.4,
            fin_at: [0.0, -0.05, 1.1],
            fin_m2: 0.02,
            paddle_n: 140.0,
            carve_nm: 420.0,
            roll_nm: 60.0,
            seat: [0.0, 0.06, 0.2],
            standing: false,
            rider: None,
            speed: 0.0,
            through_water: 0.0,
            wipeouts: 0,
        }
    }
}

impl amoris_sim::Persisted for Board {
    const NAME: &'static str = "Board";
    const VERSION: u32 = 1;
}

fn valid(b: &Board) -> Result<(), Problem> {
    let numbers = [
        b.area_m2, b.planing, b.glide, b.rail, b.fin_m2, b.paddle_n, b.carve_nm, b.roll_nm,
    ];
    if numbers.iter().any(|v| !v.is_finite() || *v < 0.0)
        || b.fin_at.iter().chain(&b.seat).any(|v| !v.is_finite())
    {
        return Err(Problem::new(
            "board.invalid",
            "a board's coefficients are finite and not negative, its points finite",
            amoris_contract::detail([]),
        ));
    }
    Ok(())
}

type BoardItem<'a> = (
    &'a EntityId,
    &'a Transform,
    Option<&'a Velocity>,
    (Option<&'a RigidBody>, Option<&'a crate::body::Collider>),
    &'a Floater,
    &'a mut Board,
    Option<&'a mut ExternalForce>,
);

/// `physics.board` (Forces, after `physics.hull`): each board's hull forces and its rider's.
#[allow(clippy::too_many_arguments)]
pub(crate) fn board(
    clock: Res<SimClock>,
    seas: Query<(&EntityId, &Sea)>,
    winds: Query<(&EntityId, &crate::Wind)>,
    regions: Query<(&EntityId, Option<&Transform>, &crate::water::WaterRegion)>,
    samples: Res<crate::water::buoyancy::Samples>,
    riders: Query<(&EntityId, &Walker)>,
    mut boards: Query<BoardItem<'static>>,
) -> Result<(), Problem> {
    if boards.is_empty() {
        return Ok(());
    }
    let t = crate::forces::start_time(&clock);
    let field = crate::forces::sea_field(&seas);
    let wind = by_id(&winds).next().map(|(_, w)| w);
    let regions: Vec<_> = by_id(&regions)
        .filter_map(|(id, t, r)| {
            crate::water::buoyancy::Cover::new(*id, t.map_or([0.0; 3], |t| t.position), r)
                .map(|cover| (cover, r))
        })
        .collect();
    let riders: Vec<(EntityId, Walker)> = by_id(&riders).map(|(id, w)| (*id, *w)).collect();
    let dt = clock.dt();
    for (id, tf, vel, (body, collider), floater, mut board, ext) in by_id_mut(&mut boards) {
        valid(&board)?;
        let rider = riders.iter().find(|(_, w)| w.mount == Some(*id));
        let v = vel.copied().unwrap_or_default();
        let com = geom::to_world(tf.position, tf.rotation, local_com(body));
        let up = geom::rotate(tf.rotation, [0.0, 1.0, 0.0]);
        let ahead = geom::rotate(tf.rotation, [0.0, 0.0, -1.0]);
        let right = geom::rotate(tf.rotation, [1.0, 0.0, 0.0]);
        let mut out = ExternalForce::default();
        let count = f64::from(u32::try_from(floater.points.len().max(1)).unwrap_or(u32::MAX));
        let share = board.area_m2 / count;
        // The plate's pressure grows with the square of its speed through the water: implicit
        // over each point's share of the board's mass, c / (1 + c dt / m), so a board landing flat
        // from a height stops against the water instead of bouncing off it.
        let mass = crate::body::body_mass(body, collider).filter(|m| *m > 0.0);
        let implicit = |c: f64| match mass {
            Some(m) => c / (1.0 + c * dt * count / m),
            None => c,
        };
        let mut wet = 0.0;
        let mut through = 0.0;
        let mut waters = Vec::with_capacity(floater.points.len());
        for (point, p) in floater.points.iter().enumerate() {
            let at = geom::to_world(tf.position, tf.rotation, p.at);
            waters.push(crate::forces::point_water(
                *id,
                crate::water::buoyancy::Role::Floater(point),
                at,
                &regions,
                &samples,
                &field,
                wind,
                t,
                clock.tick.0,
            )?);
        }
        for (p, here) in floater.points.iter().zip(&waters) {
            let at = geom::to_world(tf.position, tf.rotation, p.at);
            let Some(here) = here else {
                continue;
            };
            let under = crate::forces::immersion(at[1], p.size, here.water.height, here.bed);
            if under <= 0.0 {
                continue;
            }
            wet += under;
            let rel = geom::sub(here.water.velocity, velocity_at(&v, com, at));
            let speed = geom::length(rel);
            let q = 0.5 * here.density * share * under * speed;
            // Water striking the bottom presses along the deck's normal; it never pulls.
            let normal = math::max(geom::dot(rel, up), 0.0) * implicit(board.planing * q);
            let along = geom::dot(rel, ahead) * implicit(board.glide * q);
            let across = geom::dot(rel, right) * implicit(board.rail * q);
            through -= geom::dot(rel, ahead) * under;
            let force = geom::add(
                geom::add(geom::scale(up, normal), geom::scale(ahead, along)),
                geom::scale(right, across),
            );
            out.add_at(force, at, com);
        }
        board.through_water = if wet > 0.0 { through / wet } else { 0.0 };
        board.speed = math::sqrt(v.linear[0] * v.linear[0] + v.linear[2] * v.linear[2]);
        // The fin grips across the board where it sits, in the water of its nearest Floater point
        // (only those points have current-tick water samples).
        let nearest = floater
            .points
            .iter()
            .enumerate()
            .map(|(n, p)| {
                let d = geom::sub(p.at, board.fin_at);
                (geom::dot(d, d), n)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .and_then(|(_, n)| waters[n].as_ref());
        if board.fin_m2 > 0.0
            && let Some(here) = nearest
        {
            let at = geom::to_world(tf.position, tf.rotation, board.fin_at);
            if at[1] < here.water.height && here.bed.is_none_or(|bed| at[1] > bed) {
                let rel = geom::sub(here.water.velocity, velocity_at(&v, com, at));
                let across = geom::dot(rel, right);
                let force = 0.5 * here.density * board.fin_m2 * 1.8 * across * across.abs();
                out.add_at(geom::scale(right, force), at, com);
            }
        }
        let wetted = wet / count;
        if let Some((_, w)) = rider {
            if w.jump {
                board.standing = !board.standing;
            }
            let lean = clamp(w.strafe, -1.0, 1.0);
            if board.standing {
                // Weight on a rail rolls the board and, with speed, turns it toward that rail.
                let grip = clamp(board.speed / 3.0, 0.0, 1.0) * clamp(wetted * 4.0, 0.0, 1.0);
                out.torque = geom::add(out.torque, geom::scale(up, -lean * board.carve_nm * grip));
                out.torque = geom::add(out.torque, geom::scale(ahead, lean * board.roll_nm));
            } else if wetted > 0.0 {
                // Arms in the water push the board along its heading, level.
                let flat = [ahead[0], 0.0, ahead[2]];
                let size = geom::length(flat);
                if size > 0.0 {
                    let stroke = clamp(1.0 - board.through_water / STROKE_MPS, 0.0, 1.0);
                    let thrust = clamp(w.forward, -0.5, 1.0) * board.paddle_n * stroke;
                    out.force = geom::add(out.force, geom::scale(flat, thrust / size));
                }
                out.torque = geom::add(out.torque, geom::scale(up, -lean * 0.25 * board.carve_nm));
            }
            board.rider = rider.map(|(id, _)| *id);
        } else {
            board.rider = None;
            board.standing = false;
        }
        if let Some(mut e) = ext {
            e.force = geom::add(e.force, out.force);
            e.torque = geom::add(e.torque, out.torque);
        }
    }
    Ok(())
}

/// `physics.mount` (Physics, after `physics.step`): each mounted Walker's eye rides its board's
/// integrated pose; a board that has turned over throws its rider into the water.
pub(crate) fn mount(
    mut walkers: Query<(&EntityId, &mut Walker, &mut Transform), Without<Board>>,
    mut boards: Query<(&EntityId, &Transform, &mut Board), Without<Walker>>,
) {
    for (_, mut w, mut tf) in by_id_mut(&mut walkers) {
        let Some(mount) = w.mount else {
            continue;
        };
        let Some((_, pose, mut board)) = by_id_mut(&mut boards).find(|(id, _, _)| **id == mount)
        else {
            w.mount = None;
            continue;
        };
        let up = geom::rotate(pose.rotation, [0.0, 1.0, 0.0]);
        if up[1] < WIPEOUT_UP {
            w.mount = None;
            w.vertical_mps = 0.0;
            board.standing = false;
            board.wipeouts = board.wipeouts.saturating_add(1);
            let eye = geom::add(pose.position, [0.0, 0.9, 0.0]);
            *tf = Transform {
                position: eye,
                rotation: w.rotation(),
            };
            continue;
        }
        let eye_m = if board.standing { w.eye_m } else { LYING_EYE_M };
        let eye = geom::to_world(
            pose.position,
            pose.rotation,
            geom::add(board.seat, [0.0, eye_m, 0.0]),
        );
        let next = Transform {
            position: eye,
            rotation: w.rotation(),
        };
        if *tf != next {
            *tf = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coefficients_are_checked() {
        valid(&Board::default()).unwrap();
        let b = Board {
            planing: -1.0,
            ..Board::default()
        };
        assert!(valid(&b).is_err());
        let b = Board {
            seat: [f64::NAN, 0.0, 0.0],
            ..Board::default()
        };
        assert!(valid(&b).is_err());
    }
}
