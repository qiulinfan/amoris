//! Inverse kinematics in the game (docs/spec/animation.md 6a), on `tests/fixtures/scenes/ik` (its
//! `gen_ik.py` writes the figure and the scene): feet stand on a slope, astride a step and on
//! stairs a walker climbed, the pelvis drops for the lower foot, a feet node turned off leaves its
//! foot hanging, a hand reaches a script's target and a head turns toward one no further than its
//! limit; the poses have the recorded digest, and a restore runs on to the same poses.
#![cfg(feature = "transpile")]
mod common;

use std::sync::Arc;

use amoris_assets::anim::{AnimIk, AnimPose, AnimState};
use amoris_link::Source;
use amoris_runtime::{Command, Game, Project};
use amoris_sim::EntityId;
use serde_json::json;

// The scene's entities (gen_ik.py): the bodies and the walker that climbs.
const SLOPE: u64 = 10;
const STEP: u64 = 12;
const CLIMBER: u64 = 14;
const ANKLE_M: f64 = 0.05;
const SLOPE_DEG: f64 = 15.0;

fn get<C: bevy_ecs::component::Component + Clone>(g: &Game, id: u64) -> Option<C> {
    let w = g.world();
    amoris_sim::entity::entity(w, EntityId::new(id).unwrap())
        .and_then(|e| w.get::<C>(e))
        .cloned()
}

fn socket(g: &Game, id: u64, joint: &str) -> ([f64; 3], [f64; 4]) {
    let st = get::<AnimState>(g, id).expect("an anim state");
    let s = st
        .sockets
        .iter()
        .find(|s| s.joint == joint)
        .unwrap_or_else(|| panic!("no socket {joint}"));
    (s.position, s.rotation)
}

fn origin(g: &Game, id: u64) -> [f64; 3] {
    get::<amoris_physics::body::Transform>(g, id)
        .expect("a transform")
        .position
}

/// `v` turned by the unit quaternion `q`.
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let u = [q[0], q[1], q[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let c = cross(u, v);
    let c = [2.0 * c[0], 2.0 * c[1], 2.0 * c[2]];
    let d = cross(u, c);
    [
        v[0] + q[3] * c[0] + d[0],
        v[1] + q[3] * c[1] + d[1],
        v[2] + q[3] * c[2] + d[2],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt();
    [a[0] / n, a[1] / n, a[2] / n]
}

/// The height of the fixture's level under (x, z): the slope, the step, the stairs or the floor.
fn ground(x: f64, z: f64) -> f64 {
    if (-5.5..=-2.5).contains(&x) && (-1.5..=1.5).contains(&z) {
        return 0.3 + (x + 4.0) * amoris_sim::math::tan(SLOPE_DEG.to_radians());
    }
    if (0.0..=2.0).contains(&x) && (-1.5..=1.5).contains(&z) {
        return 0.2;
    }
    if (3.0..=5.0).contains(&x) {
        let steps = (1..=4).filter(|k| z <= 1.2 - 0.4 * f64::from(*k)).count();
        return 0.15 * steps as f64;
    }
    0.0
}

/// How far a foot's ankle is from where it should stand on the level.
fn off_ground(g: &Game, id: u64, joint: &str) -> f64 {
    let (p, _) = socket(g, id, joint);
    p[1] - (ground(p[0], p[2]) + ANKLE_M)
}

fn digest(h: &mut u64, pose: &AnimPose) {
    for x in &pose.joints {
        for v in x.t.iter().chain(&x.r).chain(&x.s) {
            for b in v.to_bits().to_le_bytes() {
                *h ^= u64::from(b);
                *h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
}

/// The poses' digest over the run (the solvers' `f32` arithmetic in a fixed order and Rapier's
/// rays give these bits on every target).
const DIGEST: u64 = 0xd05a_01f4_30a2_7f76;

#[test]
fn feet_stand_on_the_level_and_goals_bend_the_figure() {
    common::big_stack(|| {
        let root = common::repo().join("tests/fixtures/scenes/ik");
        let setup = Project::load(&root)
            .and_then(|p| p.setup(false))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let mut g = Game::new(Arc::new(setup), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        let mut snap = None;
        let mut seen = Vec::new();
        let mut reach_target = None;
        let mut look_target = None;
        for _ in 0..360 {
            let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            let tick = r.tick.0;
            assert!(r.errors.is_empty(), "tick {tick}: {:#?}", r.errors);
            let poses: Vec<AnimPose> = [SLOPE, STEP, CLIMBER]
                .iter()
                .map(|id| get::<AnimPose>(&g, *id).expect("a pose"))
                .collect();
            poses.iter().for_each(|p| digest(&mut h, p));
            let at = origin(&g, SLOPE);
            if tick == 120 {
                reach_target = Some([at[0] + 0.3, at[1] + 1.15, at[2] - 0.3]);
            }
            if tick == 60 {
                look_target = Some([at[0] + 1.5, at[1] + 1.6, at[2] - 2.0]);
            }
            match tick {
                150 => {
                    // Settled and still: every foot on its ground, the step's pelvis down by the
                    // step's height for the foot over the floor.
                    for (id, joint) in [
                        (SLOPE, "foot_l"),
                        (SLOPE, "foot_r"),
                        (STEP, "foot_l"),
                        (STEP, "foot_r"),
                    ] {
                        let off = off_ground(&g, id, joint);
                        assert!(off.abs() < 0.01, "tick {tick}: {id} {joint} {off}");
                    }
                    let (pelvis, _) = socket(&g, STEP, "pelvis");
                    let drop = pelvis[1] - origin(&g, STEP)[1];
                    assert!((drop - 0.75).abs() < 0.02, "the step's pelvis at {drop}");
                    let ik = get::<AnimIk>(&g, STEP).expect("the feet's state");
                    assert!((ik.weight - 1.0).abs() < 1e-12, "{ik:?}");
                    let lowest = ik
                        .ground_m
                        .iter()
                        .copied()
                        .fold(f64::INFINITY, amoris_sim::math::min);
                    assert!((lowest + 0.2).abs() < 0.01, "{ik:?}");
                    // Walking, the climber's feet act by less.
                    let ik = get::<AnimIk>(&g, CLIMBER).expect("the feet's state");
                    assert!(ik.weight > 0.5 && ik.weight < 0.95, "walking: {ik:?}");
                }
                170 => {
                    // The head looks at the first target, inside its limit.
                    let (head, r) = socket(&g, SLOPE, "head");
                    let aim = rotate(r, [0.0, 0.0, 1.0]);
                    let t = look_target.unwrap();
                    let want = unit([t[0] - head[0], t[1] - head[1], t[2] - head[2]]);
                    assert!(dot(aim, want) > 0.999, "{aim:?} {want:?}");
                }
                200 => {
                    let (hand, _) = socket(&g, SLOPE, "hand_r");
                    let t = reach_target.unwrap();
                    let miss = [hand[0] - t[0], hand[1] - t[1], hand[2] - t[2]];
                    assert!(
                        dot(miss, miss).sqrt() < 0.005,
                        "the hand at {hand:?} for {t:?}"
                    );
                }
                230 => {
                    // A target behind: the head turns 70 degrees from the body's front at most.
                    let (_, r) = socket(&g, SLOPE, "head");
                    let aim = rotate(r, [0.0, 0.0, 1.0]);
                    let front = [0.0, 0.0, -1.0];
                    let c = dot(aim, front);
                    let limit = amoris_sim::math::cos(70f64.to_radians());
                    assert!((c - limit).abs() < 0.05, "{aim:?}: {c}");
                }
                255 => {
                    // The step's feet off: its foot over the floor hangs at the body's height.
                    let off = amoris_sim::math::max(
                        off_ground(&g, STEP, "foot_l"),
                        off_ground(&g, STEP, "foot_r"),
                    );
                    assert!((off - 0.2).abs() < 0.02, "{off}");
                    assert_eq!(get::<AnimIk>(&g, STEP).unwrap().weight, 0.0);
                }
                330 => {
                    // On again, and the climber stands on the stairs.
                    for (id, joint) in [
                        (STEP, "foot_l"),
                        (STEP, "foot_r"),
                        (CLIMBER, "foot_l"),
                        (CLIMBER, "foot_r"),
                    ] {
                        let off = off_ground(&g, id, joint);
                        assert!(off.abs() < 0.01, "tick {tick}: {id} {joint} {off}");
                    }
                    assert!(origin(&g, CLIMBER)[1] > 0.4, "{:?}", origin(&g, CLIMBER));
                }
                _ => {}
            }
            if tick == 150 {
                snap = Some(g.snapshot().unwrap_or_else(|e| panic!("{e:#?}")));
            }
            if tick >= 150 {
                seen.push(poses);
            }
        }
        // The script read its reach goal back on every tick from setting it to releasing it, and
        // saw the hand where the goal put it.
        let gauge = g
            .apply(&Command::new(
                Source::Developer(0),
                1,
                "world_get",
                json!({"entity": "Gauge", "components": ["Gauge"]}),
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"))["components"]["Gauge"]
            .clone();
        assert_eq!(gauge["reads"], json!(120), "{gauge}");
        assert_eq!(h, DIGEST, "poses digest {h:#018x}");
        // A restore runs on to the same poses.
        g.restore(snap.as_ref().unwrap())
            .unwrap_or_else(|e| panic!("{e:#?}"));
        for (i, want) in seen.iter().enumerate().skip(1) {
            g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            let got: Vec<AnimPose> = [SLOPE, STEP, CLIMBER]
                .iter()
                .map(|id| get::<AnimPose>(&g, *id).expect("a pose"))
                .collect();
            assert_eq!(&got, want, "{i} ticks after the restore");
        }
    });
}
