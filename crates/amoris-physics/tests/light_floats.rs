//! Light floats on still water settle instead of bouncing (water.md 47): the hydrostatic spring of
//! a float a few kilograms heavy is stiff at 60 Hz, and its one-sided contact gained energy on
//! every bounce while it was evaluated at the tick's start.
use amoris_physics::{
    BuoyPoint, Collider, ExternalForce, Floater, RigidBody, Shape, Transform, Velocity,
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, entity};

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    sim
}

/// Still water 2 m deep, its surface at 0, over 64 x 64 m.
fn pool(sim: &mut Sim) {
    let mut region = amoris_physics::water::dam_scene(4);
    region.cell_m = 1.0;
    for tile in &mut region.tiles {
        tile.bed.fill(-2.0);
        tile.depth.fill(2.0);
        tile.u.fill(0.0);
        tile.w.fill(0.0);
    }
    sim.boundary().spawn((region,)).unwrap();
}

/// A float of `mass` kg from a cuboid's density, its buoyancy points at `points`, dropped at `at`.
fn float(
    sim: &mut Sim,
    at: [f64; 3],
    half: [f64; 3],
    mass: f64,
    points: Vec<BuoyPoint>,
) -> EntityId {
    let density = mass / (8.0 * half[0] * half[1] * half[2]);
    sim.boundary()
        .spawn((
            Transform::at(at),
            Velocity::default(),
            RigidBody {
                linear_damping: 0.05,
                angular_damping: 0.4,
                ..RigidBody::dynamic()
            },
            Collider::new(Shape::Cuboid { half_extents: half }).with_density(density),
            ExternalForce::default(),
            Floater {
                points,
                drag: 1.2,
                heave: 2.5,
                added_mass: 0.0,
                submerged: 0.0,
            },
        ))
        .unwrap()
}

fn heights(sim: &mut Sim, id: EntityId, ticks: u32) -> Vec<(f64, f64)> {
    (0..ticks)
        .map(|_| {
            let report = sim.step(&mut NoHooks).unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            let e = entity::require(sim.world(), id).unwrap();
            let y = sim.world().get::<Transform>(e).unwrap().position[1];
            let vy = sim.world().get::<Velocity>(e).unwrap().linear[1];
            (y, vy)
        })
        .collect()
}

#[test]
fn a_swim_ring_settles_at_its_waterline() {
    let mut sim = world();
    pool(&mut sim);
    // 13 kg over 0.45 m^3 of float in eight 0.28 m layers: it floats 2.9% under, its centre
    // (0.5 - 13 / 450) * 0.28 = 0.132 m above the surface.
    let ring = (0..8)
        .map(|k| {
            let (s, c) = amoris_sim::math::sin_cos(std::f64::consts::TAU * f64::from(k) / 8.0);
            BuoyPoint {
                at: [0.7 * c, 0.0, 0.7 * s],
                volume: 0.45 / 8.0,
                size: 0.28,
            }
        })
        .collect();
    let id = float(&mut sim, [32.0, 0.2, 32.0], [0.95, 0.15, 0.95], 13.0, ring);
    let track = heights(&mut sim, id, 240);
    let rest = (0.5 - 13.0 / 450.0) * 0.28;
    // No bounce rises above the drop: energy only leaves.
    assert!(track.iter().all(|(y, _)| *y <= 0.2 + 1e-6), "{track:?}");
    for (y, vy) in &track[120..] {
        assert!(
            (y - rest).abs() < 0.004 && vy.abs() < 0.02,
            "{y} {vy} vs {rest}"
        );
    }
}

#[test]
fn a_beach_ball_settles_at_its_waterline() {
    let mut sim = world();
    pool(&mut sim);
    let axes = [
        [0.0, 0.0, 0.0],
        [0.18, 0.0, 0.0],
        [-0.18, 0.0, 0.0],
        [0.0, 0.18, 0.0],
        [0.0, -0.18, 0.0],
        [0.0, 0.0, 0.18],
        [0.0, 0.0, -0.18],
    ];
    let points = axes
        .iter()
        .map(|&at| BuoyPoint {
            at,
            volume: 0.137 / 7.0,
            size: 0.2,
        })
        .collect();
    // 3.4 kg: the lowest point alone holds it, 17.5% of its layer under water.
    let id = float(&mut sim, [32.0, 0.6, 32.0], [0.2, 0.2, 0.2], 3.4, points);
    let track = heights(&mut sim, id, 300);
    let rest = 0.18 + (0.5 - 3.4 / (1000.0 * 0.137 / 7.0)) * 0.2;
    assert!(track.iter().all(|(y, _)| *y <= 0.6 + 1e-6), "{track:?}");
    for (y, vy) in &track[180..] {
        assert!(
            (y - rest).abs() < 0.004 && vy.abs() < 0.02,
            "{y} {vy} vs {rest}"
        );
    }
}
