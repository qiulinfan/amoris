//! Vessels (water.md 62): bodies the water holds up by their collider's hull, cut at the water's
//! surface every tick, with no buoyancy points placed by hand. Checked against hydrostatics: the
//! draft Archimedes gives a box and a ball, a box's roll at its metacentric period, a tall box
//! capsizing, a face's drag, and a holed boat filling through its hole and sinking.
use amoris_physics::{
    Collider, ExternalForce, Hole, RigidBody, Shape, Transform, Velocity, Vessel,
};
use amoris_sim::math;
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, entity};

const G: f64 = 9.81;

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    sim
}

/// Still water 3 m deep, its surface at 0, over 64 x 64 m, and a floor under it.
fn pool(sim: &mut Sim) {
    sim.boundary()
        .spawn((
            Transform::at([32.0, -3.5, 32.0]),
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid {
                half_extents: [32.0, 0.5, 32.0],
            }),
        ))
        .unwrap();
    let mut region = amoris_physics::water::dam_scene(4);
    region.cell_m = 1.0;
    for tile in &mut region.tiles {
        tile.bed.fill(-3.0);
        tile.depth.fill(3.0);
        tile.u.fill(0.0);
        tile.w.fill(0.0);
    }
    sim.boundary().spawn((region,)).unwrap();
}

fn vessel(sim: &mut Sim, at: Transform, shape: Shape, density: f64, v: Vessel) -> EntityId {
    sim.boundary()
        .spawn((
            at,
            Velocity::default(),
            RigidBody {
                linear_damping: 0.0,
                angular_damping: 0.0,
                ..RigidBody::dynamic()
            },
            Collider::new(shape).with_density(density),
            ExternalForce::default(),
            v,
        ))
        .unwrap()
}

fn run(sim: &mut Sim, ticks: u32, mut each: impl FnMut(&Sim)) {
    for _ in 0..ticks {
        let report = sim.step(&mut NoHooks).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        each(sim);
    }
}

/// A vessel's pose, velocity and state.
type State = (Transform, Velocity, Vessel);

fn pose(sim: &Sim, id: EntityId) -> State {
    let e = entity::require(sim.world(), id).unwrap();
    (
        *sim.world().get::<Transform>(e).unwrap(),
        *sim.world().get::<Velocity>(e).unwrap(),
        sim.world().get::<Vessel>(e).unwrap().clone(),
    )
}

/// The roll about z, degrees, of a pose: the body's up axis leaning toward -x is positive.
fn roll(t: &Transform) -> f64 {
    let up = amoris_physics::geom::rotate(t.rotation, [0.0, 1.0, 0.0]);
    math::atan2(-up[0], up[1]).to_degrees()
}

#[test]
fn a_box_and_a_ball_float_at_archimedes_draft() {
    let mut sim = world();
    pool(&mut sim);
    // A 2 x 1 x 4 m box half as dense as water floats half under: its centre at the surface.
    let block = vessel(
        &mut sim,
        Transform::at([20.0, 0.3, 32.0]),
        Shape::Cuboid {
            half_extents: [1.0, 0.5, 2.0],
        },
        500.0,
        Vessel::default(),
    );
    // A ball a quarter as dense: the cap under the surface, pi h^2 (3 r - h) / 3, holds a quarter
    // of its volume.
    let r = 0.5;
    let ball = vessel(
        &mut sim,
        Transform::at([44.0, 0.6, 32.0]),
        Shape::Ball { radius: r },
        250.0,
        Vessel::default(),
    );
    // Without the waves a body makes in a region that takes its displaced volume, little damps
    // its heave: the mean over the last 3 s, over two periods, is where it floats.
    let mut track = Vec::new();
    run(&mut sim, 1200, |sim| {
        track.push((pose(sim, block), pose(sim, ball)))
    });
    let last = &track[track.len() - 180..];
    let mean = |f: &dyn Fn(&(State, State)) -> f64| last.iter().map(f).sum::<f64>() / 180.0;
    let (y, submerged) = (mean(&|p| p.0.0.position[1]), mean(&|p| p.0.2.submerged));
    eprintln!("box: centre {y:.4} m, submerged {submerged:.4}");
    assert!(
        y.abs() < 0.005 && (submerged - 0.5).abs() < 0.005,
        "{y} {submerged}"
    );
    let (mut lo, mut hi) = (0.0, 2.0 * r);
    for _ in 0..60 {
        let h = 0.5 * (lo + hi);
        if h * h * (3.0 * r - h) < r * r * r {
            lo = h;
        } else {
            hi = h;
        }
    }
    let rest = r - 0.5 * (lo + hi);
    let (y, submerged) = (mean(&|p| p.1.0.position[1]), mean(&|p| p.1.2.submerged));
    eprintln!("ball: centre {y:.4} m against {rest:.4}, submerged {submerged:.4}");
    assert!(
        (y - rest).abs() < 0.005 && (submerged - 0.25).abs() < 0.005,
        "{y} {submerged}"
    );
}

#[test]
fn a_box_rolls_at_its_metacentric_period_and_a_tall_one_capsizes() {
    let mut sim = world();
    pool(&mut sim);
    // Beam 2 m, depth 1 m, half as dense as water: draft 0.5 m, GM = KB + BM - KG = 0.25 +
    // 2^2 / (12 x 0.5) - 0.5 = 0.417 m, and its roll period 2 pi k / sqrt(g GM), k^2 = (2^2 +
    // 1^2) / 12 about its long axis.
    let (s, c) = math::sin_cos(math::PI / 180.0 * 2.5);
    let block = vessel(
        &mut sim,
        Transform {
            position: [20.0, 0.0, 32.0],
            rotation: [0.0, 0.0, s, c],
        },
        Shape::Cuboid {
            half_extents: [1.0, 0.5, 2.0],
        },
        500.0,
        Vessel {
            pressure: 0.0,
            friction: 0.0,
            ..Vessel::default()
        },
    );
    // Beam 1 m, depth 2 m: draft 1 m, GM = 0.5 + 1 / 12 - 1 = -0.417 m: it lies down.
    let (s1, c1) = math::sin_cos(math::PI / 180.0 * 0.5);
    let tall = vessel(
        &mut sim,
        Transform {
            position: [44.0, 0.0, 32.0],
            rotation: [0.0, 0.0, s1, c1],
        },
        Shape::Cuboid {
            half_extents: [0.5, 1.0, 1.0],
        },
        500.0,
        Vessel::default(),
    );
    let mut angles = Vec::new();
    let mut lying = Vec::new();
    run(&mut sim, 1200, |sim| {
        angles.push(roll(&pose(sim, block).0));
        lying.push(roll(&pose(sim, tall).0).abs());
    });
    // Downward crossings of zero, interpolated within the tick.
    let crossings: Vec<f64> = angles
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] > 0.0 && w[1] <= 0.0)
        .map(|(n, w)| (n as f64 + w[0] / (w[0] - w[1])) / 60.0)
        .collect();
    let gm = 0.25 + 4.0 / 6.0 - 0.5;
    let k = math::sqrt(5.0 / 12.0);
    let expected = 2.0 * math::PI * k / math::sqrt(G * gm);
    let periods: Vec<f64> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
    eprintln!("roll periods {periods:?} against {expected:.3} s");
    assert!(periods.len() >= 3, "{angles:?}");
    for p in &periods {
        assert!((p / expected - 1.0).abs() < 0.03, "{p} against {expected}");
    }
    let peak = angles.iter().copied().fold(0.0, math::max);
    assert!(peak < 5.1, "an undamped roll gains no energy: {peak}");
    let lying = lying[lying.len() - 180..].iter().sum::<f64>() / 180.0;
    eprintln!("the tall box lies at {lying:.1} degrees");
    assert!((lying - 90.0).abs() < 2.0, "{lying}");
}

#[test]
fn a_face_meeting_the_water_drags_like_a_plate() {
    let mut sim = world();
    pool(&mut sim);
    // The box afloat, pushed along x at 2 m/s: its 2 m^2 end under the water meets
    // half rho C u^2 with C = 1, its skin the ITTC line's friction.
    let block = vessel(
        &mut sim,
        Transform::at([20.0, 0.0, 32.0]),
        Shape::Cuboid {
            half_extents: [2.0, 0.5, 1.0],
        },
        500.0,
        Vessel::default(),
    );
    run(&mut sim, 120, |_| {});
    {
        let e = entity::require(sim.world(), block).unwrap();
        sim.world_mut().get_mut::<Velocity>(e).unwrap().linear = [2.0, 0.0, 0.0];
    }
    let before = pose(&sim, block).1.linear[0];
    run(&mut sim, 1, |_| {});
    let after = pose(&sim, block).1.linear[0];
    let mass = 500.0 * 8.0;
    let deceleration = (before - after) * 60.0;
    let re = 2.0 * 4.0 / 1.0e-6;
    let cf = 0.075 / (math::log10(re) - 2.0) * (1.0 / (math::log10(re) - 2.0));
    // The end 2 x 0.5 m, the bottom 4 x 2 and the sides 2 x 4 x 0.5 m.
    let drag = 0.5 * 1000.0 * (1.0 * 1.0 + cf * 12.0) * 4.0;
    eprintln!(
        "deceleration {deceleration:.4} m/s^2 against {:.4}",
        drag / mass
    );
    assert!(
        (deceleration / (drag / mass) - 1.0).abs() < 0.1,
        "{deceleration}"
    );
}

#[test]
fn a_holed_boat_fills_and_sinks_and_a_sound_one_stays_afloat() {
    let mut sim = world();
    pool(&mut sim);
    // Open boats 2 m wide, 4 m long, their rim 0.9 m over the bottom, of 400 kg: afloat 5 cm
    // deep. One has a hole of 0.1 m^2 in its bottom. As it fills it sinks so that the water
    // outside stays 400 kg / (1000 kg/m^3 x 8 m^2) = 5 cm over the water inside, and the hole
    // takes 0.61 a sqrt(2 g 0.05) a second. The water inside has a free surface as wide as the
    // waterplane, which takes away the stability the waterplane gave: the swamped boat heels, its
    // rim goes under and it fills over it and sinks.
    let boat = |holes| Vessel {
        rim: Some(0.4),
        holes,
        ..Vessel::default()
    };
    let shape = Shape::Cuboid {
        half_extents: [1.0, 0.5, 2.0],
    };
    let sound = vessel(
        &mut sim,
        Transform::at([20.0, 0.5, 32.0]),
        shape.clone(),
        50.0,
        boat(vec![]),
    );
    let holed = vessel(
        &mut sim,
        Transform::at([44.0, 0.5, 32.0]),
        shape,
        50.0,
        boat(vec![Hole {
            at: [0.0, -0.5, 0.0],
            area: 0.1,
        }]),
    );
    let mut track = Vec::new();
    run(&mut sim, 60 * 180, |sim| {
        let (t, _, v) = pose(sim, holed);
        track.push((t.position[1], v.flooded));
    });
    let (t, _, v) = pose(&sim, sound);
    eprintln!(
        "sound: centre {:.3} m, flooded {}",
        t.position[1], v.flooded
    );
    assert!(
        (t.position[1] - 0.45).abs() < 0.01 && v.flooded == 0.0,
        "{t:?} {v:?}"
    );
    let rate = 0.61 * 0.1 * math::sqrt(2.0 * G * 0.05);
    let upright = (track[60 * 20].1 - track[60 * 2].1) / 18.0;
    eprintln!("filling upright at {upright:.5} m^3/s against {rate:.5}");
    assert!((upright / rate - 1.0).abs() < 0.02, "{upright}");
    let full = track.iter().position(|(_, f)| *f > 7.19);
    let sunk = track.iter().position(|(y, _)| *y < -2.0);
    let (y, flooded) = *track.last().unwrap();
    eprintln!(
        "holed: full at {:?} s, on the bed at {:?} s, centre {y:.3} m, flooded {flooded:.3} m^3",
        full.map(|n| n as f64 / 60.0),
        sunk.map(|n| n as f64 / 60.0)
    );
    assert!(
        full.is_some() && sunk.is_some(),
        "{:?}",
        &track[track.len() - 5..]
    );
    // On the floor, 3 m down: on its bottom, its centre half its height over it, or capsized on
    // its side, half its beam.
    let resting = (y + 2.5).abs() < 0.05 || (y + 2.0).abs() < 0.05;
    assert!((flooded - 7.2).abs() < 0.01 && resting, "{y} {flooded}");
}

/// The pool party's dinghy hull (samples/pool-party/build_scene.py): a flat bottom, a raked bow.
fn dinghy() -> Shape {
    let bottom = [
        (-0.9, -0.4),
        (0.2, -0.42),
        (0.9, -0.3),
        (0.9, 0.3),
        (0.2, 0.42),
        (-0.9, 0.4),
    ];
    let top = [
        (-1.0, -0.5),
        (0.2, -0.55),
        (1.2, -0.32),
        (1.2, 0.32),
        (0.2, 0.55),
        (-1.0, 0.5),
    ];
    let mut points: Vec<[f64; 3]> = bottom.iter().map(|&(x, z)| [x, -0.21, z]).collect();
    points.extend(top.iter().map(|&(x, z)| [x, 0.21, z]));
    Shape::ConvexHull { points }
}

/// The time a tick takes with eight dinghies afloat against the same world without them.
#[test]
#[ignore]
// A timing, outside the ticks it measures.
#[allow(clippy::disallowed_methods)]
fn time_vessels() {
    let build = |boats: bool| {
        let mut sim = world();
        pool(&mut sim);
        if boats {
            for n in 0..8 {
                let at = [
                    8.0 + 6.0 * f64::from(n % 4),
                    0.1,
                    20.0 + 8.0 * f64::from(n / 4),
                ];
                sim.boundary()
                    .spawn((
                        Transform::at(at),
                        Velocity::default(),
                        RigidBody {
                            mass: Some(amoris_physics::MassProps {
                                mass: 60.0,
                                center: [0.0, -0.1, 0.0],
                                inertia: [6.0, 30.0, 25.0],
                            }),
                            ..RigidBody::dynamic()
                        },
                        Collider::new(dinghy()),
                        ExternalForce::default(),
                        Vessel {
                            rim: Some(0.2),
                            holes: if n % 2 == 1 {
                                vec![Hole {
                                    at: [0.0, -0.21, 0.0],
                                    area: 0.002,
                                }]
                            } else {
                                vec![]
                            },
                            ..Vessel::default()
                        },
                    ))
                    .unwrap();
            }
        }
        sim
    };
    for _ in 0..2 {
        let mut ms = [0.0; 2];
        for (k, boats) in [false, true].into_iter().enumerate() {
            let mut sim = build(boats);
            run(&mut sim, 60, |_| {});
            let start = std::time::Instant::now();
            run(&mut sim, 600, |_| {});
            ms[k] = start.elapsed().as_secs_f64() * 1000.0 / 600.0;
        }
        eprintln!(
            "a tick: {:.3} ms without, {:.3} ms with eight dinghies, {:.4} ms a dinghy",
            ms[0],
            ms[1],
            (ms[1] - ms[0]) / 8.0
        );
    }
}
