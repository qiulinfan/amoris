//! The points that stir the water regions, for the renderer's ripples (water.md 57) and entry
//! crowns (water.md 52): each buoyancy point of a Floater, a walker's body over a region, in the
//! air too, and the hands of a swimmer's strokes or of a rider paddling a board. Presentation
//! only: the renderer measures each point's immersion against its drawn surface and makes ripples
//! from its motion relative to the water, and a crown where it goes in fast; nothing here is read
//! by the simulation, and the strokes' hands are a drawing of the stroke the walker rules already
//! apply as a speed.

// Presentation, as the extractor that calls it (present.rs).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use amoris_assets::frame::{MAX_STIRS, StirView};
use amoris_physics::water::displacement::WALKER_BODY_M3;
use amoris_physics::{Board, Floater, Transform, Walker, WaterRegion, geom};
use amoris_sim::{EntityId, SimClock};
use bevy_ecs::prelude::World;

/// How far outside a region's rectangle a body still counts as in it, metres.
const MARGIN_M: f64 = 2.0;
/// A hand and wrist: its volume (m^3) and height (m).
const HAND_M3: f64 = 0.0012;
const HAND_M: f64 = 0.12;
/// A swimmer's eye over the water while swimming, as treading water holds it (water.md 50).
const EYE_OVER_M: f64 = 0.3;
/// One cycle of both arms of a crawl, and of a paddler's, seconds.
const CRAWL_S: f64 = 1.3;
const PADDLE_S: f64 = 1.2;
/// Sculling hands while treading water: their period, s.
const SCULL_S: f64 = 0.9;

fn f(x: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        x as f32
    }
}

fn view(id: u64, point: u32, at: [f64; 3], size: f64, volume: f64) -> StirView {
    StirView {
        id,
        point,
        at: at.map(f),
        size: f(size),
        volume: f(volume),
    }
}

/// A hand's place over one stroke, `s` in [0, 1): the pull (the first 55 percent) sweeps it from
/// `reach` metres ahead to `reach - sweep` under the water, as deep as `dip`; the recovery
/// carries it forward again through the air. Returns (ahead, out to the side, up), metres, from
/// the shoulder line at the waterline.
fn stroke(s: f64, reach: f64, sweep: f64, side: f64, dip: f64) -> [f64; 3] {
    const PULL: f64 = 0.55;
    if s < PULL {
        let u = s / PULL;
        let arc = (std::f64::consts::PI * u).sin();
        [reach - sweep * u, side + 0.08 * arc, 0.06 - dip * arc]
    } else {
        let u = (s - PULL) / (1.0 - PULL);
        let arc = (std::f64::consts::PI * u).sin();
        [
            reach - sweep * (1.0 - u),
            side + 0.08 * arc,
            0.06 + 0.3 * arc,
        ]
    }
}

/// The rectangles (x0, z0, x1, z1) of the water regions, widened by [`MARGIN_M`].
fn regions(world: &mut World) -> Vec<[f64; 4]> {
    world
        .query::<(&WaterRegion, Option<&Transform>)>()
        .iter(world)
        .map(|(r, t)| {
            let o = t.map_or([0.0; 3], |t| t.position);
            let size = r.size.map(|n| f64::from(n) * r.cell_m);
            [
                o[0] - MARGIN_M,
                o[2] - MARGIN_M,
                o[0] + size[0] + MARGIN_M,
                o[2] + size[1] + MARGIN_M,
            ]
        })
        .collect()
}

/// The world's stirring points in entity order, at most [`MAX_STIRS`].
pub fn stirs(world: &mut World) -> Vec<StirView> {
    let rects = regions(world);
    if rects.is_empty() {
        return Vec::new();
    }
    let inside = |p: [f64; 3]| {
        rects
            .iter()
            .any(|r| p[0] >= r[0] && p[0] <= r[2] && p[2] >= r[1] && p[2] <= r[3])
    };
    let t = world.get_resource::<SimClock>().map_or(0.0, SimClock::time);
    let mut out = Vec::new();
    let mut floaters: Vec<_> = world
        .query::<(&EntityId, &Transform, &Floater)>()
        .iter(world)
        .filter(|(_, tf, _)| inside(tf.position))
        .map(|(id, tf, fl)| (id.get(), *tf, fl.points.clone()))
        .collect();
    floaters.sort_unstable_by_key(|r| r.0);
    for (id, tf, points) in floaters {
        for (n, p) in points.iter().enumerate() {
            let at = geom::to_world(tf.position, tf.rotation, p.at);
            #[allow(clippy::cast_possible_truncation)]
            out.push(view(id, n as u32, at, p.size, p.volume));
        }
    }
    let boards: Vec<_> = world
        .query::<(&EntityId, &Transform, &Board)>()
        .iter(world)
        .map(|(id, tf, b)| (*id, (*tf, b.standing, b.seat)))
        .collect();
    let mut walkers: Vec<_> = world
        .query::<(&EntityId, &Transform, &Walker)>()
        .iter(world)
        .map(|(id, tf, w)| (id.get(), *tf, *w))
        .collect();
    walkers.sort_unstable_by_key(|r| r.0);
    for (id, tf, w) in walkers {
        let eye = tf.position;
        // The look's heading along the water: forward and to the right.
        let (sin, cos) = w.yaw.sin_cos();
        let ahead = [-sin, 0.0, -cos];
        let right = [cos, 0.0, -sin];
        let input = w.forward.abs().max(w.strafe.abs());
        if let Some(mount) = w.mount {
            // A rider lying on a board paddles with both arms beside it while pushing forward;
            // standing, they make no strokes (the board's own points make its wake).
            let Some((board, standing, seat)) =
                boards.iter().find(|(b, _)| *b == mount).map(|(_, b)| *b)
            else {
                continue;
            };
            if standing || w.forward <= 0.05 || !inside(board.position) {
                continue;
            }
            for (arm, side) in [(1u32, -1.0), (2, 1.0)] {
                let s = (t / PADDLE_S + 0.5 * f64::from(arm - 1)).rem_euclid(1.0);
                let [along, across, up] = stroke(s, 0.75, 0.75, 0.4, 0.3);
                // The board's frame: forward -z, right +x; its deck stands at the waterline.
                let local = [side * across, up, seat[2] - along];
                let at = geom::to_world(board.position, board.rotation, local);
                out.push(view(id, arm, at, HAND_M, HAND_M3));
            }
            continue;
        }
        if !inside(eye) {
            continue;
        }
        // The body: its volume over its height, so its waterplane is a head and shoulders'. Over
        // the water too, so the renderer sees a jump go in.
        let feet = eye[1] - w.eye_m;
        let body = [eye[0], feet + 0.5 * w.height_m, eye[2]];
        out.push(view(id, 0, body, w.height_m, WALKER_BODY_M3));
        if !w.in_liquid || w.eye_in_liquid {
            continue;
        }
        let waterline = eye[1] - EYE_OVER_M;
        for (arm, side) in [(1u32, -1.0), (2, 1.0)] {
            let [along, across, up] = if input > 0.1 {
                let s = (t / CRAWL_S + 0.5 * f64::from(arm - 1)).rem_euclid(1.0);
                stroke(s, 0.6, 0.9, 0.22, 0.32)
            } else {
                // Treading water: the hands scull to and fro just under the surface.
                let phase =
                    std::f64::consts::TAU * t / SCULL_S + std::f64::consts::PI * f64::from(arm);
                [0.15 + 0.12 * phase.sin(), 0.38, -0.08]
            };
            let at = [
                eye[0] + ahead[0] * along + right[0] * side * across,
                waterline + up,
                eye[2] + ahead[2] * along + right[2] * side * across,
            ];
            out.push(view(id, arm, at, HAND_M, HAND_M3));
        }
    }
    out.truncate(MAX_STIRS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stroke_enters_ahead_pulls_under_and_recovers_through_the_air() {
        let mut lowest = f64::MAX;
        let mut previous = stroke(0.999_999, 0.6, 0.9, 0.22, 0.32);
        for n in 0..1000 {
            let s = f64::from(n) / 1000.0;
            let p = stroke(s, 0.6, 0.9, 0.22, 0.32);
            // Continuous over the whole cycle, the wrap included.
            let jump = (0..3)
                .map(|a| (p[a] - previous[a]).abs())
                .fold(0.0, f64::max);
            assert!(jump < 0.01, "{s}: {jump}");
            if s < 0.55 {
                lowest = lowest.min(p[2]);
            } else {
                assert!(
                    p[2] >= 0.06 - 1e-9,
                    "the recovery stays in the air: {s} {p:?}"
                );
            }
            previous = p;
        }
        assert!(
            (lowest + 0.26).abs() < 0.01,
            "the pull dips under: {lowest}"
        );
        assert!((stroke(0.0, 0.6, 0.9, 0.22, 0.32)[0] - 0.6).abs() < 1e-12);
        assert!((stroke(0.55, 0.6, 0.9, 0.22, 0.32)[0] + 0.3).abs() < 1e-12);
    }
}
