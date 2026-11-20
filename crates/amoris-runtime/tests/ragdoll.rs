//! Ragdolls in the game (docs/spec/animation.md 6b), on `tests/fixtures/scenes/ragdoll` (its
//! `gen_ragdoll.py` writes the figure and the scene): a walker's body goes limp under a shove,
//! falls onto the floor with its joints holding and nothing sinking, follows its root body and is
//! left there by its avatar, and gets up again over its blend; a body on a block's edge is shoved
//! off and lies below it; the bodies come and go with the ragdoll; a restore runs on to the same
//! poses, which have the recorded digest.
#![cfg(feature = "transpile")]
use crate::common;

use std::sync::Arc;

use amoris_assets::anim::{AnimPose, AnimState};
use amoris_physics::body::{Collider, Shape, Transform};
use amoris_physics::{Joint, Ragdoll, RagdollBone};
use amoris_runtime::{Game, Project};
use amoris_sim::EntityId;

// The scene's entities (gen_ragdoll.py).
const WALKER: u64 = 4;
const BODY: u64 = 5;
const EDGE: u64 = 6;
const BODIES: usize = 11;

fn get<C: bevy_ecs::component::Component + Clone>(g: &Game, id: u64) -> Option<C> {
    let w = g.world();
    amoris_sim::entity::entity(w, EntityId::new(id).unwrap())
        .and_then(|e| w.get::<C>(e))
        .cloned()
}

fn socket(g: &Game, id: u64, joint: &str) -> [f64; 3] {
    let st = get::<AnimState>(g, id).expect("an anim state");
    st.sockets
        .iter()
        .find(|s| s.joint == joint)
        .unwrap_or_else(|| panic!("no socket {joint}"))
        .position
}

fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let c = cross([q[0], q[1], q[2]], v);
    let c = [2.0 * c[0], 2.0 * c[1], 2.0 * c[2]];
    let d = cross([q[0], q[1], q[2]], c);
    [
        v[0] + q[3] * c[0] + d[0],
        v[1] + q[3] * c[1] + d[1],
        v[2] + q[3] * c[2] + d[2],
    ]
}

fn apply(t: &Transform, p: [f64; 3]) -> [f64; 3] {
    let r = rotate(t.rotation, p);
    [
        t.position[0] + r[0],
        t.position[1] + r[1],
        t.position[2] + r[2],
    ]
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    amoris_sim::math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
}

/// A ragdoll's bodies, in body order: entity, place, collider, joint.
#[allow(clippy::disallowed_methods)] // collected, then sorted by body
fn bodies(g: &Game, owner: u64) -> Vec<(u64, Transform, Collider, Option<Joint>)> {
    let mut out: Vec<(u32, u64, Transform, Collider, Option<Joint>)> = g
        .world()
        .iter_entities()
        .filter_map(|e| {
            let b = e.get::<RagdollBone>()?;
            (b.owner.get() == owner).then(|| {
                (
                    b.body,
                    e.get::<EntityId>().expect("an id").get(),
                    *e.get::<Transform>().expect("a place"),
                    e.get::<Collider>().expect("a collider").clone(),
                    e.get::<Joint>().copied(),
                )
            })
        })
        .collect();
    out.sort_by_key(|r| r.0);
    out.into_iter().map(|r| (r.1, r.2, r.3, r.4)).collect()
}

/// Every body of `owner` lies above the floor (its capsule's lowest point at most a little in it)
/// and every joint holds its two anchors together.
fn lying_whole(g: &Game, owner: u64, tick: u64) {
    let all = bodies(g, owner);
    assert_eq!(all.len(), BODIES, "tick {tick}");
    for (id, t, c, j) in &all {
        let Shape::Capsule {
            half_height,
            radius,
        } = c.shape
        else {
            panic!("a capsule")
        };
        let ends = [
            apply(t, [0.0, half_height, 0.0]),
            apply(t, [0.0, -half_height, 0.0]),
        ];
        let low = amoris_sim::math::min(ends[0][1], ends[1][1]) - radius;
        assert!(
            low > -0.03,
            "tick {tick}: body {id} reaches {low} below the floor"
        );
        if let Some(j) = j {
            let other = all
                .iter()
                .find(|b| b.0 == j.with.get())
                .expect("its parent");
            let gap = dist(apply(t, j.anchor), apply(&other.1, j.other_anchor));
            assert!(gap < 0.03, "tick {tick}: body {id}'s joint opens by {gap}");
        }
    }
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

/// The poses' digest over the run (the ragdolls' `f32` arithmetic in a fixed order and Rapier's
/// deterministic steps give these bits on every target).
const DIGEST: u64 = 0xc641_ac40_0f5b_dca0;

#[test]
fn bodies_go_limp_fall_hold_together_and_get_up() {
    common::big_stack(|| {
        let root = common::repo().join("tests/fixtures/scenes/ragdoll");
        let setup = Project::load(&root)
            .and_then(|p| p.setup(false))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let mut g = Game::new(Arc::new(setup), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        let mut snap = None;
        let mut seen = Vec::new();
        let start = get::<Transform>(&g, BODY).unwrap().position;
        for _ in 0..360 {
            let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            let tick = r.tick.0;
            assert!(r.errors.is_empty(), "tick {tick}: {:#?}", r.errors);
            let poses: Vec<AnimPose> = [BODY, EDGE]
                .iter()
                .map(|id| get::<AnimPose>(&g, *id).expect("a pose"))
                .collect();
            poses.iter().for_each(|p| digest(&mut h, p));
            match tick {
                29 => {
                    assert!(bodies(&g, BODY).is_empty(), "no bodies before");
                    assert_eq!(get::<Ragdoll>(&g, BODY).unwrap().root, None);
                }
                30 => {
                    // Limp: the bodies are made where the bones are, the pose is theirs.
                    let r = get::<Ragdoll>(&g, BODY).unwrap();
                    assert_eq!(r.weight, 1.0);
                    let all = bodies(&g, BODY);
                    assert_eq!(all.len(), BODIES);
                    assert_eq!(r.root.map(|e| e.get()), Some(all[0].0));
                    assert!(all[0].3.is_none() && all[1..].iter().all(|b| b.3.is_some()));
                    let pelvis = socket(&g, BODY, "pelvis");
                    assert!((pelvis[1] - 0.97).abs() < 0.05, "{pelvis:?}");
                }
                200 => {
                    // Down on the floor, shoved along +z; the body follows its root and the
                    // avatar leaves it there.
                    lying_whole(&g, BODY, tick);
                    lying_whole(&g, EDGE, tick);
                    let pelvis = socket(&g, BODY, "pelvis");
                    assert!(pelvis[1] < 0.35, "the pelvis at {pelvis:?}");
                    assert!(pelvis[2] > 0.3, "shoved along +z: {pelvis:?}");
                    let at = get::<Transform>(&g, BODY).unwrap().position;
                    assert!(dist(at, start) > 0.3, "the body at {at:?}");
                    assert!(dist(at, pelvis) < 1.5, "{at:?} under {pelvis:?}");
                    // Off the block's edge and below its top.
                    let pelvis = socket(&g, EDGE, "pelvis");
                    assert!(pelvis[0] < 2.0 && pelvis[1] < 0.6, "{pelvis:?}");
                }
                250 => {
                    // Getting up: the weight falls, the bodies stay until it is 0.
                    let r = get::<Ragdoll>(&g, BODY).unwrap();
                    assert!(r.weight > 0.0 && r.weight < 1.0, "{r:?}");
                    assert_eq!(bodies(&g, BODY).len(), BODIES);
                }
                300 => {
                    let r = get::<Ragdoll>(&g, BODY).unwrap();
                    assert_eq!((r.weight, r.root), (0.0, None), "{r:?}");
                    assert!(bodies(&g, BODY).is_empty(), "the bodies are gone");
                    // Standing at its walker's feet, the pose its graph's.
                    let at = get::<Transform>(&g, BODY).unwrap().position;
                    let eye = get::<Transform>(&g, WALKER).unwrap().position;
                    assert!(
                        dist(at, [eye[0], eye[1] - 1.6, eye[2]]) < 1e-9,
                        "{at:?} {eye:?}"
                    );
                    let pelvis = socket(&g, BODY, "pelvis");
                    assert!(
                        (pelvis[1] - at[1] - 0.95).abs() < 0.03,
                        "{pelvis:?} over {at:?}"
                    );
                    // The edge's body is still down.
                    assert_eq!(get::<Ragdoll>(&g, EDGE).unwrap().weight, 1.0);
                    lying_whole(&g, EDGE, tick);
                }
                _ => {}
            }
            if tick == 100 {
                snap = Some(g.snapshot().unwrap_or_else(|e| panic!("{e:#?}")));
            }
            if (100..=280).contains(&tick) {
                seen.push(poses);
            }
        }
        assert_eq!(h, DIGEST, "poses digest {h:#018x}");
        // A restore runs on to the same poses, through the get-up and the bodies' removal.
        g.restore(snap.as_ref().unwrap())
            .unwrap_or_else(|e| panic!("{e:#?}"));
        for (i, want) in seen.iter().enumerate().skip(1) {
            g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            let got: Vec<AnimPose> = [BODY, EDGE]
                .iter()
                .map(|id| get::<AnimPose>(&g, *id).expect("a pose"))
                .collect();
            assert_eq!(&got, want, "{i} ticks after the restore");
        }
    });
}
