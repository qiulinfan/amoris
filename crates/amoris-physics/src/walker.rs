//! The first-person body (docs/spec/voxels.md 5): `Walker`, moved in the tick by `physics.walk`
//! against the solid cells of every block volume, its box swept one axis at a time as the block
//! game does. World state: the first_person catalog's controls write its inputs, game rules read its
//! look and pulses, the eye is the entity's `Transform`.

use amoris_sim::math;
use amoris_sim::num::to_i32;
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::voxels::{BlockShape, FALLING, LEVEL, Voxels};
use amoris_sim::{EntityId, SimClock};
use bevy_ecs::prelude::{Component, Query, Res, Without};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::body::Transform;

/// The furthest a walker's look may tilt up or down, radians.
pub const MAX_PITCH: f64 = 1.55;
/// The fastest a walker falls, m/s.
pub const TERMINAL_MPS: f64 = 60.0;
/// How far a box may sweep along one axis in one tick, cells (a guard, never reached by walking).
const MAX_SWEEP_CELLS: i32 = 256;
/// The gap, in cells, that keeps a box off the faces it rests against.
const SKIN: f64 = 1e-7;
/// In a liquid (liquids.md 5): the share of gravity, the fastest sinking, the share of horizontal
/// speed, the current's speed, and the share of a jump that lifts a swimmer rising at a wall.
const LIQUID_GRAVITY: f64 = 0.25;
const LIQUID_SINK_MPS: f64 = 2.0;
const LIQUID_SPEED: f64 = 0.6;
const CURRENT_MPS: f64 = 1.4;
const WALL_LIFT: f64 = 0.6;

/// A first-person character (voxels.md 5.2). Its entity's `Transform` is the eye.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Walker {
    /// The eye above the feet, metres.
    pub eye_m: f64,
    /// The box's height, metres.
    pub height_m: f64,
    /// Half the box's width and depth, metres.
    pub half_width_m: f64,
    /// Walking speed, m/s.
    pub speed_mps: f64,
    /// Running speed, m/s.
    pub run_mps: f64,
    /// Take-off speed of a jump, m/s.
    pub jump_mps: f64,
    /// Gravity, m/s2.
    pub gravity_mps2: f64,
    /// Forward input, -1 to 1.
    pub forward: f64,
    /// Rightward input, -1 to 1.
    pub strafe: f64,
    /// The look's turn about +y, radians (0 looks along -z, positive turns left).
    pub yaw: f64,
    /// The look's tilt, radians, positive up; within `MAX_PITCH`.
    pub pitch: f64,
    pub run: bool,
    /// A one-tick pulse: jump if grounded.
    pub jump: bool,
    /// A one-tick pulse for the game's rules (the block game breaks a block).
    pub primary: bool,
    /// A one-tick pulse for the game's rules (the block game places a block).
    pub secondary: bool,
    /// The selected hotbar slot, 0 to 8.
    pub slot: u32,
    /// Vertical speed, m/s, positive up.
    pub vertical_mps: f64,
    /// Whether the last move down stopped on a solid cell.
    pub grounded: bool,
    /// Held input: swim up in a liquid (liquids.md 5).
    pub rise: bool,
    /// Swimming speed up, m/s.
    pub swim_mps: f64,
    /// Whether a liquid overlaps the box.
    pub in_liquid: bool,
    /// Whether the eye is under a liquid's surface.
    pub eye_in_liquid: bool,
}

impl Default for Walker {
    fn default() -> Self {
        Walker {
            eye_m: 1.62,
            height_m: 1.8,
            half_width_m: 0.3,
            speed_mps: 4.3,
            run_mps: 5.6,
            jump_mps: 8.9,
            gravity_mps2: 32.0,
            forward: 0.0,
            strafe: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            run: false,
            jump: false,
            primary: false,
            secondary: false,
            slot: 0,
            vertical_mps: 0.0,
            grounded: false,
            rise: false,
            swim_mps: 2.0,
            in_liquid: false,
            eye_in_liquid: false,
        }
    }
}

impl Walker {
    /// The eye's orientation: `yaw` about +y, then `pitch` about +x, as `[x, y, z, w]`.
    pub fn rotation(&self) -> [f64; 4] {
        let (sy, cy) = math::sin_cos(self.yaw * 0.5);
        let (sp, cp) = math::sin_cos(self.pitch * 0.5);
        [cy * sp, sy * cp, -sy * sp, cy * cp]
    }

    /// The unit direction the eye looks along.
    pub fn look(&self) -> [f64; 3] {
        let (sy, cy) = math::sin_cos(self.yaw);
        let (sp, cp) = math::sin_cos(self.pitch);
        [-sy * cp, sp, -cy * cp]
    }

    /// The box's lower and upper corners for an eye at `eye`.
    pub fn bounds(&self, eye: [f64; 3]) -> ([f64; 3], [f64; 3]) {
        let feet = eye[1] - self.eye_m;
        (
            [eye[0] - self.half_width_m, feet, eye[2] - self.half_width_m],
            [
                eye[0] + self.half_width_m,
                feet + self.height_m,
                eye[2] + self.half_width_m,
            ],
        )
    }
}

/// A volume as the sweep sees it.
struct Volume<'a> {
    origin: [f64; 3],
    voxels: &'a Voxels,
}

fn cell_of(x: f64) -> Option<i32> {
    to_i32(x.floor()).ok()
}

/// How far the box `lo..hi` can move by `d` along `axis` before a solid cell of `v` stops it.
fn sweep(v: &Volume, lo: [f64; 3], hi: [f64; 3], axis: usize, d: f64) -> f64 {
    let c = v.voxels.cell_m;
    let g = |p: [f64; 3]| [0, 1, 2].map(|a| (p[a] - v.origin[a]) / c);
    let (glo, ghi) = (g(lo), g(hi));
    let gd = d / c;
    let others: Vec<usize> = (0..3).filter(|&b| b != axis).collect();
    let mut span = [[0i32; 2]; 3];
    for &b in &others {
        let (Some(a), Some(z)) = (cell_of(glo[b] + SKIN), cell_of(ghi[b] - SKIN)) else {
            return d;
        };
        span[b] = [a, z];
    }
    let blocked = |k: i32, span: &[[i32; 2]; 3]| {
        let (b0, b1) = (others[0], others[1]);
        for i in span[b0][0]..=span[b0][1] {
            for j in span[b1][0]..=span[b1][1] {
                let mut cell = [0; 3];
                cell[axis] = k;
                cell[b0] = i;
                cell[b1] = j;
                if v.voxels.solid(v.voxels.get(cell)) {
                    return true;
                }
            }
        }
        false
    };
    if gd > 0.0 {
        let (Some(first), Some(last)) = (cell_of(ghi[axis] + SKIN), cell_of(ghi[axis] + gd)) else {
            return d;
        };
        for k in first..=last.min(first + MAX_SWEEP_CELLS) {
            if blocked(k, &span) {
                return math::min(d, math::max(0.0, f64::from(k) - ghi[axis] - SKIN) * c);
            }
        }
    } else if gd < 0.0 {
        let (Some(first), Some(last)) = (cell_of(glo[axis] - SKIN), cell_of(glo[axis] + gd)) else {
            return d;
        };
        let mut k = first;
        while k >= last.max(first - MAX_SWEEP_CELLS) {
            if blocked(k, &span) {
                return math::max(d, math::min(0.0, f64::from(k) + 1.0 - glo[axis] + SKIN) * c);
            }
            k -= 1;
        }
    }
    d
}

/// How a liquid meets a box: whether one overlaps it, whether the eye is under a surface, and the
/// current's horizontal direction, unit or zero (liquids.md 5).
struct Contact {
    body: bool,
    eye: bool,
    current: [f64; 2],
}

/// A liquid cell's surface, as a share of the cell: a source at 8/9, level n at (8 - n) / 9, the
/// whole cell when falling or under the same liquid (liquids.md 6).
fn surface(v: &Voxels, c: [i32; 3], b: u8, s: u8) -> f64 {
    if s & FALLING != 0 || v.get([c[0], c[1] + 1, c[2]]) == b {
        1.0
    } else {
        f64::from(8 - (s & LEVEL)) / 9.0
    }
}

fn contact(w: &Walker, eye: [f64; 3], volumes: &[Volume]) -> Contact {
    let (lo, hi) = w.bounds(eye);
    let mut out = Contact {
        body: false,
        eye: false,
        current: [0.0; 2],
    };
    for vol in volumes {
        let v = vol.voxels;
        let g = |p: f64, a: usize| (p - vol.origin[a]) / v.cell_m;
        let span = |a: usize| (cell_of(g(lo[a], a) + SKIN), cell_of(g(hi[a], a) - SKIN));
        let (Some(x0), Some(x1)) = span(0) else {
            continue;
        };
        let (Some(y0), Some(y1)) = span(1) else {
            continue;
        };
        let (Some(z0), Some(z1)) = span(2) else {
            continue;
        };
        let liquid = |b: u8| b != 0 && v.shape(b) == BlockShape::Liquid;
        for y in y0..=y1 {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let c = [x, y, z];
                    let (b, s) = v.cell(c);
                    if !liquid(b) {
                        continue;
                    }
                    let top = f64::from(y) + surface(v, c, b, s);
                    if g(lo[1], 1) >= top {
                        continue;
                    }
                    out.body = true;
                    if v.flow(b).is_none() || s & (LEVEL | FALLING) == 0 {
                        continue;
                    }
                    // The current runs toward lower neighbours, and over edges it falls from.
                    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let n = [x + dx, y, z + dz];
                        let (nb, ns) = v.cell(n);
                        let lower = nb == b && ns & FALLING == 0 && ns & LEVEL > s & LEVEL;
                        let drop = (nb == 0 || v.shape(nb) == BlockShape::Cross)
                            && !v.solid(v.get([n[0], n[1] - 1, n[2]]));
                        if lower || drop {
                            out.current[0] += f64::from(dx);
                            out.current[1] += f64::from(dz);
                        }
                    }
                }
            }
        }
        let e = [0, 1, 2].map(|a| cell_of(g(eye[a], a)));
        if let [Some(ex), Some(ey), Some(ez)] = e {
            let (b, s) = v.cell([ex, ey, ez]);
            if liquid(b) && g(eye[1], 1) < f64::from(ey) + surface(v, [ex, ey, ez], b, s) {
                out.eye = true;
            }
        }
    }
    let len = math::sqrt(out.current[0] * out.current[0] + out.current[1] * out.current[1]);
    if len > 0.0 {
        out.current = [out.current[0] / len, out.current[1] / len];
    }
    out
}

/// Moves a walker one tick of `dt` seconds among `volumes`; returns the new eye.
fn advance(w: &mut Walker, eye: [f64; 3], dt: f64, volumes: &[Volume]) -> [f64; 3] {
    let wet = contact(w, eye, volumes);
    let (mut f, mut s) = (
        math::clamp(w.forward, -1.0, 1.0),
        math::clamp(w.strafe, -1.0, 1.0),
    );
    let len = math::sqrt(f * f + s * s);
    if len > 1.0 {
        f /= len;
        s /= len;
    }
    let mut speed = if w.run { w.run_mps } else { w.speed_mps };
    if wet.body {
        speed *= LIQUID_SPEED;
    }
    let (sy, cy) = math::sin_cos(w.yaw);
    let mut vx = (-sy * f + cy * s) * speed;
    let mut vz = (-cy * f - sy * s) * speed;
    if wet.body {
        vx += wet.current[0] * CURRENT_MPS;
        vz += wet.current[1] * CURRENT_MPS;
    }
    if w.jump && w.grounded {
        w.vertical_mps = w.jump_mps;
    }
    if wet.body {
        w.vertical_mps = math::max(
            w.vertical_mps - w.gravity_mps2 * LIQUID_GRAVITY * dt,
            -LIQUID_SINK_MPS,
        );
        if w.rise {
            w.vertical_mps = math::max(w.vertical_mps, w.swim_mps);
        }
    } else {
        w.vertical_mps = math::max(w.vertical_mps - w.gravity_mps2 * dt, -TERMINAL_MPS);
    }
    w.in_liquid = wet.body;
    w.eye_in_liquid = wet.eye;
    let mut eye = eye;
    let wanted = [vx * dt, w.vertical_mps * dt, vz * dt];
    let mut walled = false;
    for axis in [1, 0, 2] {
        let (lo, hi) = w.bounds(eye);
        let mut d = wanted[axis];
        for v in volumes {
            d = sweep(v, lo, hi, axis, d);
        }
        eye[axis] += d;
        if axis == 1 {
            let stopped = d != wanted[1];
            w.grounded = stopped && wanted[1] < 0.0;
            if stopped {
                w.vertical_mps = 0.0;
            }
        } else if d != wanted[axis] {
            walled = true;
        }
    }
    // A swimmer rising against a wall is lifted, as by a jump, onto a bank one cell high.
    if wet.body && w.rise && walled {
        w.vertical_mps = math::max(w.vertical_mps, w.jump_mps * WALL_LIFT);
    }
    eye
}

/// `physics.walk`: moves every walker, in id order, against every volume.
pub(crate) fn walk(
    clock: Res<SimClock>,
    mut walkers: Query<(&EntityId, &mut Walker, &mut Transform)>,
    volumes: Query<(&EntityId, &Voxels, Option<&Transform>), Without<Walker>>,
) {
    let dt = clock.dt();
    let volumes: Vec<Volume> = by_id(&volumes)
        .map(|(_, voxels, tf)| Volume {
            origin: tf.map_or([0.0; 3], |t| t.position),
            voxels,
        })
        .collect();
    for (_, mut walker, mut tf) in by_id_mut(&mut walkers) {
        let mut w = *walker;
        w.yaw = math::wrap_angle(w.yaw);
        w.pitch = math::clamp(w.pitch, -MAX_PITCH, MAX_PITCH);
        let eye = advance(&mut w, tf.position, dt, &volumes);
        let pose = Transform {
            position: eye,
            rotation: w.rotation(),
        };
        if *tf != pose {
            *tf = pose;
        }
        if *walker != w {
            *walker = w;
        }
    }
}

/// `physics.walk.clear`: ends the tick's pulses, so each acts in exactly one tick.
pub(crate) fn clear(mut walkers: Query<&mut Walker>) {
    #[allow(clippy::disallowed_methods)] // per-entity writes: the order changes nothing
    for mut w in walkers.iter_mut() {
        if w.jump || w.primary || w.secondary {
            w.jump = false;
            w.primary = false;
            w.secondary = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amoris_sim::voxels::Block;

    fn ground() -> Voxels {
        let mut v = Voxels::default();
        v.palette.push(Block {
            name: "stone".into(),
            solid: true,
            faces: ["s".into(), "s".into(), "s".into()],
            ..Block::default()
        });
        v.palette.push(Block {
            name: "grass".into(),
            shape: amoris_sim::voxels::BlockShape::Cross,
            faces: ["g".into(), String::new(), String::new()],
            ..Block::default()
        });
        v.fill([-20, -1, -20], [20, -1, 20], 1, 0).unwrap();
        v
    }

    fn run(w: &mut Walker, eye: [f64; 3], v: &Voxels, ticks: usize) -> [f64; 3] {
        let vols = [Volume {
            origin: [0.0; 3],
            voxels: v,
        }];
        let mut eye = eye;
        for _ in 0..ticks {
            eye = advance(w, eye, 1.0 / 60.0, &vols);
            w.jump = false;
        }
        eye
    }

    #[test]
    fn falls_and_lands_on_the_ground() {
        let v = ground();
        let mut w = Walker::default();
        let eye = run(&mut w, [0.5, 5.0, 0.5], &v, 120);
        assert!(w.grounded);
        assert!((eye[1] - w.eye_m).abs() < 1e-5, "{eye:?}");
        assert_eq!(w.vertical_mps, 0.0);
    }

    #[test]
    fn walks_where_it_looks_and_stops_at_a_wall() {
        let mut v = ground();
        v.fill([-20, 0, -5], [20, 1, -5], 1, 0).unwrap();
        let mut w = Walker {
            forward: 1.0,
            ..Walker::default()
        };
        let start = [0.5, w.eye_m, 0.5];
        let eye = run(&mut w, start, &v, 240);
        assert!(
            (eye[2] - (-4.0 + 0.3)).abs() < 1e-5,
            "stopped at the wall's face: {eye:?}"
        );
        assert!((eye[0] - 0.5).abs() < 1e-9);
        w.yaw = std::f64::consts::FRAC_PI_2; // turned left: walks along -x
        let eye = run(&mut w, eye, &v, 30);
        assert!(eye[0] < -1.5, "{eye:?}");
    }

    #[test]
    fn a_cell_takes_a_jump_and_plants_do_not_block() {
        let mut v = ground();
        v.fill([-5, 0, -20], [5, 0, -3], 1, 0).unwrap(); // a step one cell high
        v.set([0, 0, -1], 2, 0).unwrap(); // a plant on the way
        let mut w = Walker {
            forward: 1.0,
            ..Walker::default()
        };
        let start = [0.5, w.eye_m, 0.5];
        let eye = run(&mut w, start, &v, 120);
        assert!((eye[2] - (-2.0 + 0.3)).abs() < 1e-5, "no step up: {eye:?}");
        assert!((eye[1] - w.eye_m).abs() < 1e-5);
        w.jump = true;
        let eye = run(&mut w, eye, &v, 60);
        assert!(
            (eye[1] - (1.0 + w.eye_m)).abs() < 1e-5,
            "on top after a jump: {eye:?}"
        );
        assert!(w.grounded);
    }

    #[test]
    fn a_jump_under_a_ceiling_stops_at_the_head() {
        let mut v = ground();
        v.set([0, 2, 0], 1, 0).unwrap();
        let mut w = Walker {
            jump: true,
            grounded: true,
            ..Walker::default()
        };
        let vols = [Volume {
            origin: [0.0; 3],
            voxels: &v,
        }];
        let mut eye = [0.5, w.eye_m, 0.5];
        let mut top: f64 = 0.0;
        for _ in 0..30 {
            eye = advance(&mut w, eye, 1.0 / 60.0, &vols);
            w.jump = false;
            top = math::max(top, eye[1] - w.eye_m + w.height_m);
        }
        assert!(
            top <= 2.0 + 1e-9,
            "the head stayed under the ceiling: {top}"
        );
        assert!(top > 1.99, "{top}");
    }

    /// The ground, and a pool of still water (sources) over it from x = -6 to 2, `depth` deep, with
    /// a bank one cell high from x = 3.
    fn pool(depth: i32) -> Voxels {
        let mut v = ground();
        v.palette.push(Block {
            name: "water".into(),
            shape: BlockShape::Liquid,
            faces: ["w".into(), String::new(), String::new()],
            ..Block::default()
        });
        let water = 3;
        v.fill([-6, 1 - depth, -6], [2, 0, 6], water, 0).unwrap();
        v.fill([-7, -depth, -7], [3, -depth, 7], 1, 0).unwrap();
        v.fill([3, 0 - depth + 1, -6], [6, 0, 6], 1, 0).unwrap();
        v
    }

    #[test]
    fn a_walker_sinks_slowly_and_swims_up_in_a_liquid() {
        let v = pool(6);
        let mut w = Walker::default();
        let mut eye = [-2.5, -1.0, 0.5];
        let vols = [Volume {
            origin: [0.0; 3],
            voxels: &v,
        }];
        for _ in 0..60 {
            eye = advance(&mut w, eye, 1.0 / 60.0, &vols);
        }
        assert!(w.in_liquid && w.eye_in_liquid);
        assert!(
            w.vertical_mps >= -LIQUID_SINK_MPS - 1e-9,
            "{}",
            w.vertical_mps
        );
        assert!(eye[1] > -3.0, "sank at most about 2 m in a second: {eye:?}");
        w.rise = true;
        for _ in 0..240 {
            eye = advance(&mut w, eye, 1.0 / 60.0, &vols);
        }
        // At the surface it bobs: the feet just in and out of the top cell (surface at 8/9).
        let mut feet = Vec::new();
        for _ in 0..120 {
            eye = advance(&mut w, eye, 1.0 / 60.0, &vols);
            feet.push(eye[1] - w.eye_m);
        }
        let (lo, hi) = feet.iter().fold((f64::MAX, f64::MIN), |(a, b), &f| {
            (math::min(a, f), math::max(b, f))
        });
        assert!(
            lo > 0.5 && hi < 1.3,
            "bobs at the surface: feet {lo:.3} to {hi:.3}"
        );
    }

    #[test]
    fn a_swimmer_climbs_out_onto_a_bank_one_cell_high() {
        let v = pool(1);
        let mut w = Walker {
            forward: 1.0,
            yaw: -std::f64::consts::FRAC_PI_2, // looks along +x
            rise: true,
            ..Walker::default()
        };
        let vols = [Volume {
            origin: [0.0; 3],
            voxels: &v,
        }];
        // Feet on the pool's floor (y = 0) in a layer of water; the bank's top is at y = 1.
        let mut eye = [0.5, w.eye_m, 0.5];
        let mut top = f64::MIN;
        for _ in 0..90 {
            eye = advance(&mut w, eye, 1.0 / 60.0, &vols);
            if (3.0..7.0).contains(&eye[0]) {
                top = math::max(top, eye[1] - w.eye_m);
            }
        }
        assert!(eye[0] > 4.0, "out of the water and along the bank: {eye:?}");
        assert!(top >= 1.0 - 1e-6, "walked on the bank's top: feet {top}");
    }

    #[test]
    fn a_current_pushes_the_walker_downstream() {
        let mut v = ground();
        v.palette.push(Block {
            name: "water".into(),
            shape: BlockShape::Liquid,
            faces: ["w".into(), String::new(), String::new()],
            flow: Some(amoris_sim::voxels::Flow::default()),
            ..Block::default()
        });
        // A run of flowing water along +x, its level rising (its surface falling) cell by cell.
        for x in 0..7 {
            v.set_with_state([x, 0, 0], 3, u8::try_from(x).unwrap(), 0)
                .unwrap();
        }
        let mut w = Walker::default();
        let vols = [Volume {
            origin: [0.0; 3],
            voxels: &v,
        }];
        let mut eye = [1.5, w.eye_m, 0.5];
        for _ in 0..30 {
            eye = advance(&mut w, eye, 1.0 / 60.0, &vols);
        }
        assert!(eye[0] > 1.5 + 0.5, "carried downstream: {eye:?}");
        assert!((eye[2] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_look_and_the_rotation_agree() {
        let w = Walker {
            yaw: 0.7,
            pitch: -0.4,
            ..Walker::default()
        };
        let q = w.rotation();
        // Rotate -z by q.
        let [x, y, z, s] = q;
        let v = [0.0, 0.0, -1.0];
        let t = [
            2.0 * (y * v[2] - z * v[1]),
            2.0 * (z * v[0] - x * v[2]),
            2.0 * (x * v[1] - y * v[0]),
        ];
        let r = [
            v[0] + s * t[0] + (y * t[2] - z * t[1]),
            v[1] + s * t[1] + (z * t[0] - x * t[2]),
            v[2] + s * t[2] + (x * t[1] - y * t[0]),
        ];
        let l = w.look();
        for a in 0..3 {
            assert!((r[a] - l[a]).abs() < 1e-12, "{r:?} {l:?}");
        }
    }
}
