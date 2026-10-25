//! Boards a Walker rides (water.md 50): paddling, standing up and carving, and wipeouts, on a CPU
//! water field.
use amoris_physics::{
    Board, BuoyPoint, Collider, ExternalForce, Floater, MassProps, RigidBody, Shape, Transform,
    Velocity, Walker,
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
/// Still water 2 m deep over 64 x 64 m.
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
/// A 2.7 m longboard and its rider, 85 kg, 0.13 m^3 of buoyancy over 15 bottom points.
fn board(sim: &mut Sim, at: [f64; 3], velocity: [f64; 3]) -> (EntityId, EntityId) {
    let mut points = Vec::new();
    for z in [-1.1, -0.55, 0.0, 0.55, 1.1] {
        for x in [-0.2, 0.0, 0.2] {
            points.push(BuoyPoint {
                at: [x, 0.0, z],
                volume: 0.13 / 15.0,
                size: 0.1,
            });
        }
    }
    let id = sim
        .boundary()
        .spawn((
            Transform::at(at),
            Velocity {
                linear: velocity,
                angular: [0.0; 3],
            },
            RigidBody::dynamic().with_mass(MassProps::solid_box(
                85.0,
                [0.28, 0.25, 1.35],
                [0.0; 3],
            )),
            Collider::new(Shape::Cuboid {
                half_extents: [0.28, 0.05, 1.35],
            }),
            ExternalForce::default(),
            Floater {
                points,
                drag: 0.0,
                heave: 0.6,
                added_mass: 0.0,
                submerged: 0.0,
            },
            Board::default(),
        ))
        .unwrap();
    let rider = sim
        .boundary()
        .spawn((
            Transform::at([at[0], at[1] + 0.5, at[2]]),
            Walker {
                mount: Some(id),
                ..Walker::default()
            },
        ))
        .unwrap();
    (id, rider)
}
fn run(sim: &mut Sim, ticks: u32, rider: EntityId, input: impl Fn(&mut Walker)) {
    for _ in 0..ticks {
        let e = entity::require(sim.world(), rider).unwrap();
        let mut w = *sim.world().get::<Walker>(e).unwrap();
        input(&mut w);
        *sim.world_mut().get_mut::<Walker>(e).unwrap() = w;
        let report = sim.step(&mut NoHooks).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
}
fn get<C: bevy_ecs::component::Component + Clone>(sim: &Sim, id: EntityId) -> C {
    let e = entity::require(sim.world(), id).unwrap();
    sim.world().get::<C>(e).unwrap().clone()
}

#[test]
fn a_rider_lying_down_paddles_to_a_stroke_limited_speed_and_rides_level() {
    let mut sim = world();
    pool(&mut sim);
    let (id, rider) = board(&mut sim, [32.0, 0.05, 32.0], [0.0; 3]);
    run(&mut sim, 600, rider, |w| w.forward = 1.0);
    let b: Board = get(&sim, id);
    let tf: Transform = get(&sim, id);
    let up = amoris_physics::geom::rotate(tf.rotation, [0.0, 1.0, 0.0]);
    eprintln!(
        "paddled 10 s: speed {:.2} m/s, through water {:.2}, at {:?}, up {:?}, submerged {:.2}",
        b.speed,
        b.through_water,
        tf.position,
        up,
        get::<Floater>(&sim, id).submerged
    );
    assert_eq!(b.rider, Some(rider));
    assert!(!b.standing);
    assert!(
        b.speed > 1.2 && b.speed < 2.6,
        "paddling speed {} m/s",
        b.speed
    );
    assert!(up[1] > 0.95, "the board should ride level: up {up:?}");
    // It paddled toward -z, its nose.
    assert!(tf.position[2] < 26.0, "{:?}", tf.position);
    // The rider's eye rides the deck.
    let eye: Transform = get(&sim, rider);
    assert!(
        (eye.position[1] - tf.position[1] - 0.4 - 0.06).abs() < 0.2,
        "{eye:?} {tf:?}"
    );
    assert!((eye.position[2] - tf.position[2]).abs() < 0.5);
}

#[test]
fn standing_up_and_leaning_carves_toward_the_rail() {
    for (lean, sign) in [(1.0, -1.0), (-1.0, 1.0)] {
        let mut sim = world();
        pool(&mut sim);
        let (id, rider) = board(&mut sim, [32.0, 0.05, 50.0], [0.0, 0.0, -5.0]);
        // Pop up on the first tick, then lean.
        run(&mut sim, 1, rider, |w| w.jump = true);
        assert!(get::<Board>(&sim, id).standing);
        run(&mut sim, 30, rider, |w| {
            w.jump = false;
            w.strafe = lean;
        });
        let v: Velocity = get(&sim, id);
        eprintln!(
            "lean {lean}: yaw rate {:.2} rad/s, speed {:.2} m/s, submerged {:.2}",
            v.angular[1],
            get::<Board>(&sim, id).speed,
            get::<Floater>(&sim, id).submerged
        );
        assert!(
            v.angular[1] * sign > 0.2,
            "lean {lean}: yaw rate {}",
            v.angular[1]
        );
    }
}

#[test]
fn a_board_turned_over_throws_its_rider_into_the_water() {
    let mut sim = world();
    pool(&mut sim);
    let (id, rider) = board(&mut sim, [32.0, 0.05, 32.0], [0.0; 3]);
    let e = entity::require(sim.world(), id).unwrap();
    // Upside down.
    sim.world_mut().get_mut::<Transform>(e).unwrap().rotation = [0.0, 0.0, 1.0, 0.0];
    run(&mut sim, 2, rider, |_| {});
    let w: Walker = get(&sim, rider);
    let b: Board = get(&sim, id);
    assert_eq!(w.mount, None);
    assert_eq!(b.wipeouts, 1);
    assert_eq!(b.rider, None);
    // The thrown rider swims afterwards.
    run(&mut sim, 30, rider, |_| {});
    assert!(get::<Walker>(&sim, rider).in_liquid);
}

#[test]
fn a_light_board_landing_flat_from_a_height_stops_on_the_water() {
    let mut sim = world();
    pool(&mut sim);
    // A 12 kg bodyboard over a waterfall's drop: flat onto still water at 12 m/s, where the
    // plate's pressure over one tick would throw it back up several times faster.
    let points = (0..9)
        .map(|n| BuoyPoint {
            at: [
                0.2 * f64::from(n % 3) - 0.2,
                0.0,
                0.4 * f64::from(n / 3) - 0.4,
            ],
            volume: 0.04 / 9.0,
            size: 0.06,
        })
        .collect();
    let id = sim
        .boundary()
        .spawn((
            Transform::at([32.0, 0.6, 32.0]),
            Velocity {
                linear: [0.0, -12.0, 0.0],
                angular: [0.0; 3],
            },
            RigidBody::dynamic().with_mass(MassProps::solid_box(12.0, [0.3, 0.03, 0.5], [0.0; 3])),
            Collider::new(Shape::Cuboid {
                half_extents: [0.3, 0.03, 0.5],
            }),
            ExternalForce::default(),
            Floater {
                points,
                drag: 0.0,
                heave: 0.6,
                added_mass: 0.0,
                submerged: 0.0,
            },
            Board {
                area_m2: 0.6,
                ..Board::default()
            },
        ))
        .unwrap();
    let mut highest_after = f64::MIN;
    let mut landed = false;
    for _ in 0..180 {
        let report = sim.step(&mut NoHooks).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let tf: Transform = get(&sim, id);
        let v: Velocity = get(&sim, id);
        landed |= v.linear[1] > -0.5;
        if landed {
            highest_after = amoris_sim::math::max(highest_after, tf.position[1]);
        }
    }
    let tf: Transform = get(&sim, id);
    let up = amoris_physics::geom::rotate(tf.rotation, [0.0, 1.0, 0.0]);
    // The water takes its momentum: it never springs back much over its waterline.
    assert!(landed && highest_after < 0.25, "rose to {highest_after}");
    assert!(up[1] > 0.95, "{up:?}");
}
