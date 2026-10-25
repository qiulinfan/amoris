//! What layouts 2 and 3 of the physics cache (docs/spec/persistence.md 8) must keep, pinned.
//!
//! - **The shape builds its by-reference slots mean.** A slot by reference stands for the shape
//!   `solver::shared_shape` builds from the body's components, or `volumes::build` from a volume
//!   chunk's solid cells, so [`shape_builds`] digests the built shapes of every kind (a triangle
//!   mesh with its BVH, convex hulls, a compound, the primitives, a volume chunk) and
//!   [`SHAPE_BUILDS`] pins the digest: the web test `physics.shape_builds` fails
//!   natively or in WebAssembly when a build changes, until the cache identity's `shapes` token is
//!   bumped (versions.md 3.6) and the digest re-blessed. The web report carries the digest, so the
//!   native and WebAssembly builds compare the shapes byte for byte.
//! - **The trajectories of layout 1.** Layout 2 changed the cache's bytes and nothing else:
//!   [`trajectories`] folds, at every tick, a hash of every section but the cache (positions,
//!   velocities and every other component), for the sailing scene, the sailing scene forked
//!   before every tick, and a level scene (`level::level_world(100, 3)`: 196 bodies on a fixed
//!   terrain of 20,000 triangles, so its shape is by reference) forked before every tick or not.
//!   [`LAYOUT1_TRAJECTORIES`] are those folds as layout 1 (commit `c05f49a6`) gave them, blessed
//!   again where a change outside the cache moved them: the water checkpoint state registered in
//!   every world (`af4dd1d3`) changed every run's persisted bytes, Pioneer's implicit buoyancy
//!   (`babe544b`, water.md 47) the sailing scene's trajectory, and the Floater's added mass
//!   (version 2, water.md 56) and the Sea's wind sea and look (version 2, water.md 60) the sailing
//!   scene's persisted bytes, its forces unchanged. Layout 3 left them as they were: it adds the
//!   block volumes' chunk slots, and these scenes hold no volume.

use amoris_sim::rng::fnv1a64_extend;
use amoris_sim::{NoHooks, Sim};

use super::{Snap, fork, fresh, ledger, level, sailing_world};
use crate::PhysicsCache;
use crate::body::{Part, PartShape, Shape};
use crate::solver::shared_shape;
use amoris_sim::persisted::PersistedCache;

const FNV: u64 = 0xcbf2_9ce4_8422_2325;

/// `n` points in the cube [-1, 1]^3 from a fixed integer sequence (xorshift32).
fn points(n: usize, seed: u32) -> Vec<[f64; 3]> {
    let mut x = seed;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        f64::from(x % 2001) / 1000.0 - 1.0
    };
    (0..n).map(|_| [next(), next(), next()]).collect()
}

/// The shapes digested by [`shape_builds`]: every kind `Shape` has.
fn sample_shapes() -> Vec<Shape> {
    let hull = |points| PartShape::ConvexHull { points };
    vec![
        Shape::Ball { radius: 0.5 },
        Shape::Cuboid {
            half_extents: [0.5, 0.75, 1.0],
        },
        Shape::Capsule {
            half_height: 0.75,
            radius: 0.5,
        },
        level::pyramid(),
        Shape::ConvexHull {
            points: points(64, 0x9e37_79b9),
        },
        level::terrain(20).shape,
        Shape::Compound {
            parts: vec![
                Part {
                    position: [0.0, 0.5, 0.0],
                    // Not unit length: the builder normalizes it.
                    rotation: [0.1, 0.2, 0.3, 0.9],
                    shape: PartShape::Cuboid {
                        half_extents: [1.0, 0.25, 0.5],
                    },
                },
                Part {
                    position: [0.0, -0.5, 1.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    shape: PartShape::Ball { radius: 0.4 },
                },
                Part {
                    position: [1.0, 0.0, -1.0],
                    rotation: [0.0, 0.6, 0.0, 0.8],
                    shape: PartShape::Capsule {
                        half_height: 0.5,
                        radius: 0.25,
                    },
                },
                Part {
                    position: [-1.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    shape: hull(points(24, 0x85eb_ca6b)),
                },
            ],
        },
    ]
}

/// A volume chunk's solid cells from a fixed integer sequence (xorshift32): about a third of them.
fn sample_mask() -> crate::volumes::Mask {
    let mut x = 0x2545_f491u32;
    let mut mask = [0u8; 512];
    for b in &mut mask {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = u8::try_from(x & 0xff).unwrap_or(0) & u8::try_from((x >> 8) & 0xff).unwrap_or(0);
    }
    mask
}

/// FNV-1a 64 over the bincode bytes (as the cache writes a shape inline) of every shape of
/// [`sample_shapes`] as `solver::shared_shape` builds it, then of a volume chunk of
/// [`sample_mask`]'s cells of 0.5 m as `volumes::build` builds it, each preceded by its length.
pub fn shape_builds() -> Result<u64, String> {
    let entity = amoris_sim::EntityId::FIRST;
    let mut built = Vec::new();
    for s in sample_shapes() {
        built.push(shared_shape(entity, &s).map_err(|p| p.message)?);
    }
    built.push(crate::volumes::build(0.5, &sample_mask()));
    let mut h = FNV;
    for shape in built {
        let bytes = bincode::serialize(&shape).map_err(|e| e.to_string())?;
        h = fnv1a64_extend(h, &(bytes.len() as u64).to_le_bytes());
        h = fnv1a64_extend(h, &bytes);
    }
    Ok(h)
}

/// The digest of [`shape_builds`] for the cache identity's `shapes 2`.
pub const SHAPE_BUILDS: u64 = 0x3f30_40cc_9f37_f207;

/// `physics.shape_builds`.
pub fn check_shape_builds() -> Result<(), String> {
    let h = shape_builds()?;
    if h == SHAPE_BUILDS {
        Ok(())
    } else {
        Err(format!(
            "the shapes build to {h:016x}, pinned {SHAPE_BUILDS:016x}: a change to the builds \
             changes what the physics cache's by-reference slots mean; bump the identity's \
             `shapes` token (versions.md 3.6) and re-bless SHAPE_BUILDS"
        ))
    }
}

/// The report's line: `shapes digest`.
pub fn shapes_report() -> String {
    match shape_builds() {
        Ok(h) => format!("shapes {h:016x}\n"),
        Err(e) => format!("FAIL shapes: {e}\n"),
    }
}

/// FNV-1a 64 over every section of `s` but the physics cache: each name, then its bytes.
fn trajectory_hash(s: &Snap) -> u64 {
    let mut h = FNV;
    for (name, b) in &s.sections {
        if *name == PhysicsCache::NAME {
            continue;
        }
        h = fnv1a64_extend(h, name.as_bytes());
        h = fnv1a64_extend(h, b);
    }
    h
}

fn fold(v: &[u64]) -> u64 {
    v.iter()
        .fold(FNV, |h, x| fnv1a64_extend(h, &x.to_le_bytes()))
}

/// One run's per-tick hashes, ticks 0 to `ticks`: of every section but the cache, and of all.
fn run(mut sim: Sim, ticks: u32, fork_each: bool) -> Result<(Vec<u64>, Vec<u64>), String> {
    let l = ledger();
    let s = l.snapshot(sim.world())?;
    let (mut traj, mut full) = (vec![trajectory_hash(&s)], vec![s.hash()]);
    for t in 0..ticks {
        if fork_each {
            sim = fork(&l, &sim, fresh)?;
        }
        let r = sim.step(&mut NoHooks).map_err(|p| p.message)?;
        if let Some(e) = r.errors.first() {
            return Err(format!("tick {}: {}", t + 1, e.message));
        }
        let s = l.snapshot(sim.world())?;
        traj.push(trajectory_hash(&s));
        full.push(s.hash());
    }
    Ok((traj, full))
}

/// The runs of [`trajectories`], with the folds layout 1 gave.
pub const LAYOUT1_TRAJECTORIES: [(&str, u64); 4] = [
    ("sailing, 1,200 ticks", 0x1792_9c93_b932_e5a3),
    (
        "sailing forked before every tick, 600 ticks",
        0x0835_4b2d_144f_b1bf,
    ),
    ("level 100 x 3, 300 ticks", 0xbdfb_a0ea_1b19_abe6),
    (
        "level 100 x 3 forked before every tick, 300 ticks",
        0xbdfb_a0ea_1b19_abe6,
    ),
];

/// The folds of the per-tick hashes of every section but the cache for the runs of
/// [`LAYOUT1_TRAJECTORIES`]. A forked run must give its unforked run's hashes, the cache's
/// included, at every tick.
pub fn trajectories() -> Result<[u64; 4], String> {
    let (sailing, sailing_full) = run(sailing_world(), 1200, false)?;
    let (forked, forked_full) = run(sailing_world(), 600, true)?;
    let (lvl, lvl_full) = run(level::level_world(100, 3), 300, false)?;
    let (lvl_forked, lvl_forked_full) = run(level::level_world(100, 3), 300, true)?;
    if sailing[..=600] != forked[..] || sailing_full[..=600] != forked_full[..] {
        return Err("the sailing scene forked before every tick left its run".into());
    }
    if lvl != lvl_forked || lvl_full != lvl_forked_full {
        return Err("the level scene forked before every tick left its run".into());
    }
    Ok([fold(&sailing), fold(&forked), fold(&lvl), fold(&lvl_forked)])
}
