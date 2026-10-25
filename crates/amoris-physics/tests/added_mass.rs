//! Added mass and water entry (water.md 56): a float's heave follows the inertia of the body and
//! the water it carries, a fast entry gives its momentum to the water it newly carries (von
//! Kármán's momentum theory), and neither ever gives the body energy.
use amoris_physics::{
    BuoyPoint, Collider, ExternalForce, Floater, RigidBody, Shape, Transform, Velocity,
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, entity};

const RHO: f64 = 1000.0;
const G: f64 = 9.81;
/// A 1 m x 0.5 m x 1 m box of 250 kg: it floats half under, 0.25 m deep.
const HALF: [f64; 3] = [0.5, 0.25, 0.5];
const MASS: f64 = 250.0;
const LAYER: f64 = 0.5;

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    // Still water 10 m deep, its surface at 0, over 64 x 64 m: a plunge never reaches the bed.
    let mut region = amoris_physics::water::dam_scene(4);
    region.cell_m = 1.0;
    for tile in &mut region.tiles {
        tile.bed.fill(-10.0);
        tile.depth.fill(10.0);
        tile.u.fill(0.0);
        tile.w.fill(0.0);
    }
    sim.boundary().spawn((region,)).unwrap();
    sim
}

/// Four buoyancy points over the box's plan, each a quarter of its volume in one layer as tall as
/// the box: its lift rises by rho g (1 m^2) per metre it sinks.
fn points() -> Vec<BuoyPoint> {
    [[-0.25, -0.25], [0.25, -0.25], [-0.25, 0.25], [0.25, 0.25]]
        .iter()
        .map(|&[x, z]| BuoyPoint {
            at: [x, 0.0, z],
            volume: 0.125,
            size: LAYER,
        })
        .collect()
}

fn float(sim: &mut Sim, y: f64, added_mass: f64) -> EntityId {
    sim.boundary()
        .spawn((
            Transform::at([32.0, y, 32.0]),
            Velocity::default(),
            RigidBody {
                linear_damping: 0.0,
                angular_damping: 0.0,
                ..RigidBody::dynamic()
            },
            Collider::new(Shape::Cuboid { half_extents: HALF })
                .with_density(MASS / (8.0 * HALF[0] * HALF[1] * HALF[2])),
            ExternalForce::default(),
            Floater {
                points: points(),
                drag: 0.0,
                heave: 0.0,
                added_mass,
                submerged: 0.0,
            },
        ))
        .unwrap()
}

/// Height and vertical velocity after each of `ticks` ticks.
fn track(sim: &mut Sim, id: EntityId, ticks: u32) -> Vec<(f64, f64)> {
    (0..ticks)
        .map(|_| {
            let report = sim.step(&mut NoHooks).unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            let e = entity::require(sim.world(), id).unwrap();
            let y = sim.world().get::<Transform>(e).unwrap().position[1];
            let v = sim.world().get::<Velocity>(e).unwrap().linear;
            assert!(
                v[0].abs() < 1e-6 && v[2].abs() < 1e-6,
                "the box drifted: {v:?}"
            );
            (y, v[1])
        })
        .collect()
}

/// The mean period between upward crossings of `level`.
fn period(track: &[(f64, f64)], level: f64) -> f64 {
    let dt = 1.0 / 60.0;
    let mut ups = Vec::new();
    for (n, w) in track.windows(2).enumerate() {
        if w[0].0 < level && w[1].0 >= level {
            ups.push((n as f64 + (level - w[0].0) / (w[1].0 - w[0].0)) * dt);
        }
    }
    assert!(ups.len() >= 3, "{ups:?}");
    (ups[ups.len() - 1] - ups[0]) / (ups.len() - 1) as f64
}

/// The body's energy over its rest: kinetic, its weight's potential and the buoyancy points'
/// potential, the work their lift does from where they leave the water.
fn energy(y: f64, v: f64) -> f64 {
    // A point's lift is rho g V clamp(0.5 - y / s, 0, 1) with its layer centred at y.
    let lift = |y: f64| {
        let s = LAYER;
        if y >= 0.5 * s {
            0.0
        } else if y >= -0.5 * s {
            amoris_sim::math::powi(0.5 * s - y, 2) / (2.0 * s)
        } else {
            -y
        }
    };
    0.5 * MASS * v * v + MASS * G * y + RHO * G * 0.5 * lift(y)
}

#[test]
fn added_mass_slows_a_float_s_heave_to_its_and_its_water_s_inertia() {
    // 5 cm over its waterline, without damping: m_a = C_a rho V under = C_a 125 kg at rest.
    let rest = LAYER * (0.5 - MASS / (RHO * 0.5));
    for added in [0.0, 0.5, 1.0] {
        let mut sim = world();
        let id = float(&mut sim, rest + 0.05, added);
        let t = track(&mut sim, id, 600);
        let measured = period(&t, rest);
        let theory = std::f64::consts::TAU * ((MASS + added * RHO * 0.25) / (RHO * G)).sqrt();
        eprintln!("added mass {added}: heave period {measured:.4} s against {theory:.4} s");
        assert!(
            (measured / theory - 1.0).abs() < 0.03,
            "added mass {added}: {measured} s against {theory} s"
        );
    }
}

#[test]
fn a_fast_entry_gives_its_momentum_to_the_water_it_carries() {
    // Dropped from 2 m, the box meets the water at 6.3 m/s. Entering, its points carry up to
    // C_a rho V = 500 kg of water: momentum alone would leave m v / (m + m_a) = 2.1 m/s when the
    // box is wholly in, against the plain box's plunge.
    let mut deepest = Vec::new();
    for added in [0.0, 1.0] {
        let mut sim = world();
        let id = float(&mut sim, 2.0, added);
        let t = track(&mut sim, id, 240);
        let entry = t.iter().find(|(y, _)| *y < 0.25).unwrap().1;
        // Once the box's top meets the surface, its points are wholly in.
        let under = t.iter().find(|(y, _)| *y < -0.25).map(|p| p.1);
        let low = t.iter().map(|p| p.0).fold(f64::MAX, amoris_sim::math::min);
        eprintln!(
            "added mass {added}: meets the water at {entry:.2} m/s, {under:?} m/s wholly in, deepest {low:.3} m"
        );
        if added > 0.0 {
            let under = under.expect("the box goes under");
            let momentum = entry * MASS / (MASS + added * RHO * 0.5);
            assert!(
                (under / momentum - 1.0).abs() < 0.25,
                "{under} m/s wholly in against momentum's {momentum}"
            );
        }
        deepest.push(low);
        // Energy only leaves: none of it comes back above the drop (to the step's 0.5%).
        let start = energy(t[0].0, t[0].1);
        for (n, &(y, v)) in t.iter().enumerate() {
            assert!(
                energy(y, v) <= start * 1.005,
                "added mass {added}, tick {n}: energy {} J over the drop's {start} J",
                energy(y, v)
            );
        }
    }
    assert!(
        deepest[1] > deepest[0] + 0.3,
        "the carried water should stop the box well short of its plunge: {deepest:?}"
    );
}
