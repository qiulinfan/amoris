//! The first-person body (docs/spec/voxels.md 5): `Walker`, moved in the tick by `physics.walk`
//! against the solid cells of every block volume, its box swept one axis at a time as the block
//! game does. World state: the first_person catalog's controls write its inputs, game rules read its
//! look and pulses, the eye is the entity's `Transform`.

use amoris_sim::math;
use amoris_sim::num::to_i32;
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::voxels::Voxels;
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

/// Moves a walker one tick of `dt` seconds among `volumes`; returns the new eye.
fn advance(w: &mut Walker, eye: [f64; 3], dt: f64, volumes: &[Volume]) -> [f64; 3] {
    let (mut f, mut s) = (
        math::clamp(w.forward, -1.0, 1.0),
        math::clamp(w.strafe, -1.0, 1.0),
    );
    let len = math::sqrt(f * f + s * s);
    if len > 1.0 {
        f /= len;
        s /= len;
    }
    let speed = if w.run { w.run_mps } else { w.speed_mps };
    let (sy, cy) = math::sin_cos(w.yaw);
    let vx = (-sy * f + cy * s) * speed;
    let vz = (-cy * f - sy * s) * speed;
    if w.jump && w.grounded {
        w.vertical_mps = w.jump_mps;
    }
    w.vertical_mps = math::max(w.vertical_mps - w.gravity_mps2 * dt, -TERMINAL_MPS);
    let mut eye = eye;
    let wanted = [vx * dt, w.vertical_mps * dt, vz * dt];
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
        }
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
