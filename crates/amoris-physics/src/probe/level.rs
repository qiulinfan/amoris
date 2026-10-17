//! The level-scale scene of the physics cache's measurements (docs/bench/physics.md, "Static
//! geometry and the cache"): the bodies of the physics bench's ConvexVsMesh scene (a box, a ball, a
//! capsule and a square pyramid, in a grid 7.5 m apart, dropped from 15 m and up) on a fixed sine
//! terrain of `cells` x `cells` cells of 3 m, `2 * cells^2` triangles. 100 cells give
//! ConvexVsMesh's 20,000 triangles; 707 give 999,698, a level.
//!
//! A small one is the web test `physics.level_fork` (checks.md 7.2, `tests`): the terrain's shape
//! is written by reference (persistence.md 8, layout 2), forks continue with their source's hashes,
//! and a snapshot restored after its world is gone, whose shape is built again from the restored
//! `Collider`, continues likewise; the report gives the scene's hash at every tick, which the
//! WebAssembly build must reproduce.

use amoris_sim::persisted::PersistedCache;
use amoris_sim::{EntityId, Name, NoHooks, Sim};

use super::{fork, fresh, ledger};
use crate::body::{Collider, ExternalForce, MassProps, RigidBody, Shape, Transform, Velocity};
use crate::geom::{self, V3};
use crate::{Physics, PhysicsCache};

/// A terrain cell's side, metres.
pub const CELL: f64 = 3.0;
/// The terrain's amplitude, metres.
pub const HEIGHT: f64 = 5.0;
/// The bodies' density, kg/m^3 (the physics bench's).
const DENSITY: f64 = 1000.0;

fn f(n: u32) -> f64 {
    f64::from(n)
}

/// The terrain's collider: `(cells + 1)^2` vertices on a 3 m grid, heights
/// `HEIGHT * sin(x / 2) * cos(z / 2)` in cells (ConvexVsMesh's wave at any size), two triangles per
/// cell, friction 0.5 and restitution 0.6. The heights use `amoris_sim::math`, so every platform
/// builds the same mesh.
pub fn terrain(cells: u32) -> Collider {
    let n = cells;
    let row = n + 1;
    let mut vertices = Vec::with_capacity(usize::try_from(row * row).unwrap_or(0));
    for z in 0..=n {
        for x in 0..=n {
            let (sx, _) = amoris_sim::math::sin_cos(f(x) * 0.5);
            let (_, cz) = amoris_sim::math::sin_cos(f(z) * 0.5);
            vertices.push([CELL * f(x), HEIGHT * sx * cz, CELL * f(z)]);
        }
    }
    let mut indices = Vec::with_capacity(usize::try_from(2 * n * n).unwrap_or(0));
    for x in 0..n {
        for z in 0..n {
            let start = row * z + x;
            indices.push([start, start + row, start + 1]);
            indices.push([start + 1, start + row, start + row + 1]);
        }
    }
    Collider::new(Shape::TriMesh { vertices, indices }).with_friction(0.5, 0.6)
}

/// The square pyramid of ConvexVsMesh: apex 1 m up, base corners 1 m out on x and z.
pub fn pyramid() -> Shape {
    Shape::ConvexHull {
        points: vec![
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ],
    }
}

/// One of the four bodies, `kind` 0 to 3, at `p`.
fn body(kind: usize, p: V3) -> (Transform, Velocity, RigidBody, Collider, ExternalForce) {
    let primitive = |s: Shape| Collider::new(s).with_density(DENSITY);
    let (rb, co) = match kind {
        0 => (
            RigidBody::dynamic(),
            primitive(Shape::Cuboid {
                half_extents: [0.5, 0.75, 1.0],
            }),
        ),
        1 => (RigidBody::dynamic(), primitive(Shape::Ball { radius: 0.5 })),
        2 => (
            RigidBody::dynamic(),
            primitive(Shape::Capsule {
                half_height: 0.75,
                radius: 0.5,
            }),
        ),
        // A hull states its mass (numeric.md 5): the pyramid's 2/3 m^3 at the density, its
        // centre of mass a quarter up, the inertia of a box of about its size.
        _ => (
            RigidBody::dynamic().with_mass(MassProps::solid_box(
                DENSITY * 2.0 / 3.0,
                [0.6, 0.4, 0.6],
                [0.0, 0.25, 0.0],
            )),
            Collider::new(pyramid()),
        ),
    };
    (
        Transform::at(p),
        Velocity::default(),
        rb,
        co.with_friction(0.5, 0.6),
        ExternalForce::default(),
    )
}

/// The scene: the terrain (entity 1, centred on the origin) and `4 * (2 * side + 1)^2` bodies,
/// layer by layer (box, ball, capsule, pyramid), the grid centred above the terrain's middle.
/// ConvexVsMesh has `side` 10: 1,764 bodies.
pub fn level_world(cells: u32, side: i32) -> Sim {
    let mut sim = super::fresh();
    let mut b = sim.boundary();
    let half = CELL * f(cells) / 2.0;
    b.spawn((
        Name::new("Terrain").expect("a short name"),
        Transform::at([-half, 0.0, -half]),
        RigidBody::fixed(),
        terrain(cells),
    ))
    .expect("ids remain");
    for kind in 0..4 {
        let y = 15.0 + 2.0 * geom::f64_of(kind);
        for x in -side..=side {
            for z in -side..=side {
                b.spawn(body(kind, [7.5 * f64::from(x), y, 7.5 * f64::from(z)]))
                    .expect("ids remain");
            }
        }
    }
    sim
}

/// The terrain's entity in a world made by [`level_world`].
pub fn terrain_id() -> EntityId {
    EntityId::FIRST
}

/// The terrain's spec hash as `physics.step` computes it for every body at every tick, and the
/// cache's encode for every fixed body (`solver::spec_hash`), or 0 without a terrain.
pub fn terrain_spec_hash(sim: &Sim) -> u64 {
    let w = sim.world();
    amoris_sim::entity::entity(w, terrain_id())
        .and_then(|e| {
            Some(crate::solver::spec_hash(
                w.get::<RigidBody>(e),
                w.get::<Collider>(e)?,
            ))
        })
        .unwrap_or(0)
}

/// The web test's scene: 20 x 20 cells (800 triangles) and 36 bodies, run for [`LEVEL_TICKS`].
const SMALL: (u32, i32) = (20, 1);

/// Ticks of the web test's level scene.
pub const LEVEL_TICKS: u64 = 120;

fn step(sim: &mut Sim) -> Result<(), String> {
    let r = sim.step(&mut NoHooks).map_err(|p| p.message)?;
    match r.errors.first() {
        Some(e) => Err(e.message.clone()),
        None => Ok(()),
    }
}

/// The small level scene's hash at tick 0 and after each of `ticks` ticks.
pub fn level_chain(ticks: u64) -> Result<Vec<u64>, String> {
    let l = ledger();
    let mut sim = level_world(SMALL.0, SMALL.1);
    let mut out = vec![l.snapshot(sim.world())?.hash()];
    for _ in 0..ticks {
        step(&mut sim)?;
        out.push(l.snapshot(sim.world())?.hash());
    }
    Ok(out)
}

/// The physics cache's bytes and the terrain shape's bytes as Rapier writes it.
pub fn cache_and_terrain_bytes(sim: &Sim) -> Result<(usize, usize), String> {
    let mut cache = Vec::new();
    PhysicsCache::encode(sim.world(), &mut cache).map_err(|p| p.message)?;
    let p = sim.world().get_resource::<Physics>().ok_or("no solver")?;
    let terrain = p
        .world
        .colliders
        .iter()
        .find(|(_, c)| {
            c.parent()
                .and_then(|b| p.world.bodies.get(b))
                .is_some_and(|b| b.is_fixed())
        })
        .ok_or("no fixed collider")?;
    let shape = bincode::serialize(terrain.1.shared_shape()).map_err(|e| e.to_string())?;
    Ok((cache.len(), shape.len()))
}

/// `physics.level_fork`: the terrain's shape is not in the cache; a fork at every tick 0 to 59 of
/// the small level scene continues 5 ticks with the original's hashes; a snapshot at tick 60
/// restored after every world holding the terrain is dropped has the snapshot's bytes and continues
/// with the original's hashes to [`LEVEL_TICKS`].
pub fn check_level_forks() -> Result<(), String> {
    let (l, forks, run) = (ledger(), 60usize, 5usize);
    let expected = level_chain(LEVEL_TICKS)?;
    let mut sim = level_world(SMALL.0, SMALL.1);
    for t in 0..forks {
        let mut f = fork(&l, &sim, fresh)?;
        if l.snapshot(f.world())?.hash() != expected[t] {
            return Err(format!("the fork at tick {t} differs from its source"));
        }
        for k in 1..=run {
            step(&mut f)?;
            if l.snapshot(f.world())?.hash() != expected[t + k] {
                return Err(format!(
                    "the fork made at tick {t} diverged at tick {}",
                    t + k
                ));
            }
        }
        step(&mut sim)?;
    }
    let (cache, terrain) = cache_and_terrain_bytes(&sim)?;
    if cache >= terrain {
        return Err(format!(
            "the cache ({cache} bytes) holds the terrain's shape ({terrain} bytes)"
        ));
    }
    let snap = l.snapshot(sim.world())?;
    drop(sim);
    let mut cold = fresh();
    l.restore(&mut cold, &snap)?;
    if l.snapshot(cold.world())? != snap {
        return Err("a cold restore changed the snapshot's bytes".into());
    }
    for (t, want) in expected.iter().enumerate().skip(forks + 1) {
        step(&mut cold)?;
        if l.snapshot(cold.world())?.hash() != *want {
            return Err(format!("the cold restore diverged at tick {t}"));
        }
    }
    Ok(())
}

/// The report's level lines: `level tick hash` for every tick of the small scene.
pub fn level_report() -> String {
    match level_chain(LEVEL_TICKS) {
        Ok(h) => h
            .iter()
            .enumerate()
            .map(|(t, x)| format!("level {t} {x:016x}\n"))
            .collect(),
        Err(e) => format!("FAIL level chain: {e}\n"),
    }
}
